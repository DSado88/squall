use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static PERSIST_COUNTER: AtomicU64 = AtomicU64::new(0);

use tokio::task::{Id as TaskId, JoinSet};
use tokio_util::sync::CancellationToken;

/// Maximum number of models per review request (prevents DoS).
pub const MAX_MODELS: usize = 20;

use crate::dispatch::registry::Registry;
use crate::dispatch::ProviderRequest;
use crate::error::SquallError;
use crate::tools::review::{
    ModelStatus, ReviewModelResult, ReviewRequest, ReviewResponse, ReviewSummary,
    MAX_INVESTIGATION_CONTEXT_BYTES,
};

/// Maximum allowed timeout to prevent Instant overflow from untrusted input.
/// 600s matches Claude Code's MCP tool timeout ceiling.
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// Orchestrates parallel model dispatch with straggler cutoff.
///
/// Unlike `Registry::query` (single model, single response), ReviewExecutor:
/// - Dispatches to ALL requested models in parallel
/// - Enforces a global cutoff timer (default 180s)
/// - Tracks per-model status (success/error/cutoff)
/// - Persists full results to disk for compaction resilience
pub struct ReviewExecutor {
    registry: Arc<Registry>,
}

impl ReviewExecutor {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }

    pub async fn execute(
        &self,
        req: &ReviewRequest,
        prompt: String,
        working_directory: Option<String>,
        files_skipped: Option<Vec<String>>,
    ) -> ReviewResponse {
        // Fix #3: Clamp timeout to prevent Instant overflow from untrusted input.
        // Use effective_timeout_secs() to account for deep mode (600s default).
        // Report the effective (clamped) value in the response, not the raw request.
        let effective_cutoff_secs = req.effective_timeout_secs().min(MAX_TIMEOUT_SECS);
        let cutoff = Duration::from_secs(effective_cutoff_secs);
        let start = Instant::now();

        // Collect warnings for quality gates (augments tracing — both logged and surfaced to caller).
        let mut warnings: Vec<String> = Vec::new();

        // Determine which models to query (deduplicate, cap at MAX_MODELS)
        let target_models: Vec<String> = if let Some(ref specific) = req.models {
            let mut seen = HashSet::new();
            let deduped: Vec<String> = specific
                .iter()
                .filter(|m| seen.insert((*m).clone()))
                .cloned()
                .collect();
            if deduped.len() > MAX_MODELS {
                let dropped: Vec<&str> = deduped[MAX_MODELS..].iter().map(|s| s.as_str()).collect();
                let msg = format!(
                    "Requested {} models but max is {MAX_MODELS}. Dropped: {:?}.",
                    deduped.len(),
                    dropped,
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
            }
            deduped.into_iter().take(MAX_MODELS).collect()
        } else {
            let mut all: Vec<String> = self.registry
                .list_models()
                .iter()
                .map(|(key, _)| (*key).clone())
                .collect();
            all.sort();
            if all.len() > MAX_MODELS {
                let dropped: Vec<&str> = all[MAX_MODELS..].iter().map(|s| s.as_str()).collect();
                let msg = format!(
                    "Registry has {} models but max is {MAX_MODELS}. Dropped: {:?}.",
                    all.len(),
                    dropped,
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
                all.truncate(MAX_MODELS);
            }
            all
        };

        // Build model→provider map for cutoff reporting
        let mut not_started = Vec::new();
        let mut model_providers: Vec<(String, String)> = Vec::new();

        for model_id in &target_models {
            if let Some(entry) = self.registry.get(model_id) {
                model_providers.push((model_id.clone(), entry.provider.clone()));
            } else {
                not_started.push(model_id.clone());
            }
        }

        // Spawn all model queries as independent tokio tasks.
        let mut set = JoinSet::new();

        // Fix #1: Track task ID → model mapping for panic attribution
        let mut task_model_map: HashMap<TaskId, (String, String)> = HashMap::new();

        // Fix #4: Compute internal deadline once before loop (not per-iteration).
        // Buffer covers cooperative grace (3s) + abort drain (5s) + margin (7s).
        const CUTOFF_BUFFER_SECS: u64 = 15;
        let internal_deadline = Instant::now() + cutoff + Duration::from_secs(CUTOFF_BUFFER_SECS);

        // Cooperative cancellation: cancel_token signals streaming tasks to return
        // partial results instead of being hard-aborted.
        let cancel_token = CancellationToken::new();

        // Warn on unused per_model_system_prompts keys (likely caller typos)
        if let Some(ref per_model) = req.per_model_system_prompts {
            let target_set: HashSet<&String> = model_providers.iter().map(|(m, _)| m).collect();
            let unused: Vec<&String> = per_model.keys().filter(|k| !target_set.contains(k)).collect();
            if !unused.is_empty() {
                let msg = format!(
                    "per_model_system_prompts contains unknown models: {unused:?}. Check listmodels for valid names."
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
            }
        }

        // Warn on unused per_model_timeout_secs keys (likely caller typos)
        if let Some(ref per_model) = req.per_model_timeout_secs {
            let target_set: HashSet<&String> = model_providers.iter().map(|(m, _)| m).collect();
            let unused: Vec<&String> = per_model.keys().filter(|k| !target_set.contains(k)).collect();
            if !unused.is_empty() {
                let msg = format!(
                    "per_model_timeout_secs contains unknown models: {unused:?}. Check listmodels for valid names."
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
            }
        }

        // Pin base timestamp before spawn loop to avoid per-model time skew.
        let base_now = Instant::now();

        for (model_id, provider) in &model_providers {
            let registry = self.registry.clone();
            let model_id = model_id.clone();
            let provider = provider.clone();
            let prompt = prompt.clone();
            // Per-model system prompt: check per_model map first, fall back to shared
            let system_prompt = req
                .per_model_system_prompts
                .as_ref()
                .and_then(|map| map.get(&model_id).cloned())
                .or_else(|| req.system_prompt.clone());
            let temperature = req.temperature;
            let max_tokens = req.effective_max_tokens();
            let reasoning_effort = req.effective_reasoning_effort();
            // Fix #2: Thread working_directory through to CLI models
            let wd = working_directory.clone();

            // Per-model deadline: min(per_model_timeout, internal_deadline).
            // Per-model timeouts are clamped to MAX_TIMEOUT_SECS.
            let per_model_deadline = req
                .per_model_timeout_secs
                .as_ref()
                .and_then(|map| map.get(&model_id))
                .map(|secs| {
                    let clamped = (*secs).min(MAX_TIMEOUT_SECS);
                    (base_now + Duration::from_secs(clamped)).min(internal_deadline)
                })
                .unwrap_or(internal_deadline);

            // Stall timeout: extend for deep mode or known slow models (non-reasoning)
            let stall_timeout = if req.deep == Some(true) {
                Some(Duration::from_secs(300))
            } else {
                None
            };

            // Clone before moving into async block — needed for task_model_map below
            let model_id_for_map = model_id.clone();
            let provider_for_map = provider.clone();
            let token = cancel_token.clone();

            let abort_handle = set.spawn(async move {
                let model_start = Instant::now();
                let provider_req = ProviderRequest {
                    prompt,
                    model: model_id.clone(),
                    deadline: per_model_deadline,
                    working_directory: wd,
                    system_prompt,
                    temperature,
                    max_tokens,
                    reasoning_effort,
                    cancellation_token: Some(token),
                    stall_timeout,
                };
                let result = registry.query(&provider_req).await;
                let latency_ms = model_start.elapsed().as_millis() as u64;
                (model_id, provider, result, latency_ms)
            });
            task_model_map.insert(abort_handle.id(), (model_id_for_map, provider_for_map));
        }

        // Collect results as they complete, racing against the cutoff timer.
        let mut results = Vec::new();
        let mut completed_models = HashSet::new();

        let deadline = tokio::time::sleep(cutoff);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                biased; // prefer results over cutoff — if both ready, take the result
                join_result = set.join_next() => {
                    match join_result {
                        Some(Ok((model_id, provider, query_result, latency_ms))) => {
                            completed_models.insert(model_id.clone());
                            results.push(collect_result(query_result, model_id, provider, latency_ms));
                            if set.is_empty() { break; }
                        }
                        // Fix #1: Attribute panics to the correct model via task ID.
                        // Guard with is_panic() — cancelled tasks should not be
                        // reported as panics (defensive; cancellation is unexpected here).
                        Some(Err(join_err)) if join_err.is_panic() => {
                            collect_panic(&join_err, &task_model_map, &mut completed_models, &mut results, &start);
                            if set.is_empty() { break; }
                        }
                        Some(Err(_)) => {
                            // Cancelled (unexpected pre-abort) — ignore
                            if set.is_empty() { break; }
                        }
                        None => break, // All tasks done
                    }
                }
                _ = &mut deadline => {
                    // Straggler cutoff: cooperative cancel first, then hard-abort.
                    // cancel_token signals streaming tasks to return partial results.
                    cancel_token.cancel();

                    // Grace period: collect partial results from tasks that respond
                    // to cancellation quickly (streaming tasks flush accumulated text).
                    let grace = tokio::time::sleep(Duration::from_secs(3));
                    tokio::pin!(grace);
                    loop {
                        tokio::select! {
                            biased;
                            join_result = set.join_next() => {
                                match join_result {
                                    Some(Ok((model_id, provider, query_result, latency_ms))) => {
                                        completed_models.insert(model_id.clone());
                                        results.push(collect_result(query_result, model_id, provider, latency_ms));
                                    }
                                    Some(Err(join_err)) if join_err.is_panic() => {
                                        collect_panic(&join_err, &task_model_map, &mut completed_models, &mut results, &start);
                                    }
                                    Some(Err(_)) => {} // Cancelled — unexpected before abort
                                    None => break,
                                }
                            }
                            _ = &mut grace => {
                                // Hard-abort stragglers that didn't respond to cancellation
                                set.abort_all();
                                break;
                            }
                        }
                    }

                    // Drain tasks that completed during abort_all()
                    let drain_grace = tokio::time::sleep(Duration::from_secs(5));
                    tokio::pin!(drain_grace);
                    loop {
                        tokio::select! {
                            biased;
                            join_result = set.join_next() => {
                                match join_result {
                                    Some(Ok((model_id, provider, query_result, latency_ms))) => {
                                        completed_models.insert(model_id.clone());
                                        results.push(collect_result(query_result, model_id, provider, latency_ms));
                                    }
                                    Some(Err(join_err)) if join_err.is_panic() => {
                                        collect_panic(&join_err, &task_model_map, &mut completed_models, &mut results, &start);
                                    }
                                    Some(Err(_)) => {} // Cancelled — expected after abort_all()
                                    None => break,
                                }
                            }
                            _ = &mut drain_grace => {
                                tracing::warn!("{} tasks hung after abort, abandoning drain", set.len());
                                break;
                            }
                        }
                    }

                    break;
                }
            }
        }

        // Mark cutoff models (spawned but didn't complete before deadline)
        let elapsed_ms = start.elapsed().as_millis() as u64;
        for (model_id, provider) in &model_providers {
            if !completed_models.contains(model_id) {
                results.push(ReviewModelResult {
                    model: model_id.clone(),
                    provider: provider.clone(),
                    status: ModelStatus::Error,
                    response: None,
                    error: Some("straggler cutoff".to_string()),
                    reason: Some("cutoff".to_string()),
                    latency_ms: elapsed_ms,
                    partial: false,
                });
            }
        }

        // Build summary from collected results.
        let summary = ReviewSummary {
            models_requested: target_models.len(),
            models_succeeded: results.iter().filter(|r| r.status == ModelStatus::Success && !r.partial).count(),
            models_failed: results.iter().filter(|r| r.status == ModelStatus::Error && r.reason.as_deref() != Some("cutoff")).count(),
            models_cutoff: results.iter().filter(|r| r.reason.as_deref() == Some("cutoff")).count(),
            models_partial: results.iter().filter(|r| r.status == ModelStatus::Success && r.partial).count(),
            models_not_started: not_started.len(),
        };

        // Construct response first (results_file: None), then persist.
        let mut response = ReviewResponse {
            results,
            not_started,
            cutoff_seconds: effective_cutoff_secs,
            elapsed_ms,
            results_file: None,
            persist_error: None,
            files_skipped,
            warnings,
            summary,
        };

        // Clamp investigation_context for persistence (prevent oversized payloads).
        // Truncate at a valid UTF-8 char boundary to avoid panicking on multi-byte characters.
        let investigation_context = req.investigation_context.as_deref().map(|ctx| {
            if ctx.len() > MAX_INVESTIGATION_CONTEXT_BYTES {
                // Walk back from MAX to find a valid char boundary.
                let mut boundary = MAX_INVESTIGATION_CONTEXT_BYTES;
                while boundary > 0 && !ctx.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                &ctx[..boundary]
            } else {
                ctx
            }
        });

        // Add warning if investigation_context was clamped (before persist so it's in the file).
        // Report actual retained byte count (may be < MAX due to UTF-8 char boundary walkback).
        if let Some(ref ctx) = req.investigation_context
            && ctx.len() > MAX_INVESTIGATION_CONTEXT_BYTES
        {
            let actual_bytes = investigation_context.as_ref().map(|c| c.len()).unwrap_or(0);
            let msg = format!(
                "investigation_context was truncated from {} to {} bytes.",
                ctx.len(),
                actual_bytes,
            );
            tracing::warn!("{msg}");
            response.warnings.push(msg);
        }

        // Persist to disk — failure must never lose in-memory results
        match persist_response(&response, investigation_context).await {
            Ok(path) => response.results_file = Some(path),
            Err(e) => {
                tracing::warn!("failed to persist review results: {e}");
                response.persist_error = Some(e.to_string());
            }
        }

        response
    }
}

/// Build a `ReviewModelResult` from a query outcome.
/// Partial results (from cooperative cancellation) are still Success with `reason: "partial"`.
pub fn collect_result(
    query_result: Result<crate::dispatch::ProviderResult, SquallError>,
    model_id: String,
    provider: String,
    latency_ms: u64,
) -> ReviewModelResult {
    match query_result {
        Ok(pr) => ReviewModelResult {
            model: model_id,
            provider: pr.provider,
            status: ModelStatus::Success,
            response: Some(pr.text),
            error: None,
            reason: if pr.partial { Some("partial".to_string()) } else { None },
            latency_ms,
            partial: pr.partial,
        },
        Err(e) => ReviewModelResult {
            model: model_id,
            provider,
            status: ModelStatus::Error,
            response: None,
            error: Some(e.user_message()),
            reason: Some(error_reason(&e)),
            latency_ms,
            partial: false,
        },
    }
}

/// Attribute a panicked task to the correct model via task ID.
fn collect_panic(
    join_err: &tokio::task::JoinError,
    task_model_map: &HashMap<TaskId, (String, String)>,
    completed_models: &mut HashSet<String>,
    results: &mut Vec<ReviewModelResult>,
    start: &Instant,
) {
    tracing::error!("review task panicked: {join_err}");
    if let Some((model_id, provider)) = task_model_map.get(&join_err.id()) {
        completed_models.insert(model_id.clone());
        results.push(ReviewModelResult {
            model: model_id.clone(),
            provider: provider.clone(),
            status: ModelStatus::Error,
            response: None,
            error: Some(format!("task panicked: {join_err}")),
            reason: Some("panic".to_string()),
            latency_ms: start.elapsed().as_millis() as u64,
            partial: false,
        });
    }
}

/// Classify a SquallError into a reason string for the review response.
fn error_reason(e: &SquallError) -> String {
    match e {
        SquallError::Timeout(_) => "timeout".to_string(),
        SquallError::Cancelled(_) => "cutoff".to_string(),
        SquallError::RateLimited { .. } => "rate_limited".to_string(),
        SquallError::AuthFailed { .. } => "auth_failed".to_string(),
        SquallError::ModelNotFound { .. } => "model_not_found".to_string(),
        SquallError::SchemaParse(_) => "parse_error".to_string(),
        SquallError::ProcessExit { .. } => "process_exit".to_string(),
        _ => "error".to_string(),
    }
}

/// Write review response to `.squall/reviews/{timestamp}_{pid}_{seq}.json`.
/// Uses epoch millis + PID + atomic counter for filename uniqueness across
/// concurrent invocations and concurrent processes.
///
/// Persists the full ReviewResponse plus optional investigation_context
/// (which lives on the request, not the response).
async fn persist_response(
    response: &ReviewResponse,
    investigation_context: Option<&str>,
) -> Result<String, std::io::Error> {
    let reviews_dir = PathBuf::from(".squall/reviews");
    tokio::fs::create_dir_all(&reviews_dir).await?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let seq = PERSIST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = format!("{ts}_{pid}_{seq}.json");
    let path = reviews_dir.join(&filename);

    // Serialize the response, then merge in investigation_context if present.
    let mut payload = serde_json::to_value(response)
        .map_err(std::io::Error::other)?;
    if let Some(ctx) = investigation_context {
        payload["investigation_context"] = serde_json::Value::String(ctx.to_string());
    }

    let json = serde_json::to_string_pretty(&payload)
        .map_err(std::io::Error::other)?;

    // Atomic write: temp file + rename prevents partial reads
    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, json.as_bytes()).await?;
    if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(format!(".squall/reviews/{filename}"))
}
