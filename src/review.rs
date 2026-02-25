use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static PERSIST_COUNTER: AtomicU64 = AtomicU64::new(0);

use tokio::task::{Id as TaskId, JoinSet};
use tokio_util::sync::CancellationToken;

/// Maximum number of models per review request (prevents DoS).
pub const MAX_MODELS: usize = 20;

use crate::dispatch::ProviderRequest;

/// Resolve a per-model key using fuzzy matching against target model names.
///
/// Resolution order:
/// 1. Exact match on config key (e.g., "grok")
/// 2. Case-insensitive match (e.g., "Grok" → "grok")
/// 3. Reverse lookup via provider model_id (e.g., "grok-4-1-fast-reasoning" → "grok")
fn resolve_per_model_key<'a>(
    key: &str,
    target_set: &HashSet<&'a String>,
    id_to_key: &HashMap<String, String>,
) -> Option<&'a String> {
    // 1. Exact match
    if let Some(m) = target_set.iter().find(|k| **k == key) {
        return Some(m);
    }
    // 2. Case-insensitive
    let key_lower = key.to_lowercase();
    if let Some(m) = target_set.iter().find(|k| k.to_lowercase() == key_lower) {
        return Some(m);
    }
    // 3. Reverse lookup: caller used provider model_id
    if let Some(config_key) = id_to_key.get(key)
        && let Some(m) = target_set.iter().find(|k| **k == config_key)
    {
        return Some(m);
    }
    None
}
use crate::dispatch::registry::Registry;
use crate::error::SquallError;
use crate::memory::MemoryStore;
use crate::tools::review::{
    MAX_INVESTIGATION_CONTEXT_BYTES, ModelStatus, ReviewModelResult, ReviewRequest, ReviewResponse,
    ReviewSummary,
};

/// Minimum success rate for a model to pass the hard gate (70%).
pub const MIN_SUCCESS_RATE: f64 = 0.70;

/// Minimum sample count before hard gate applies.
/// Models with fewer samples are allowed through (insufficient data to judge).
pub const MIN_GATE_SAMPLES: usize = 5;

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

    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        req: &ReviewRequest,
        prompt: String,
        memory: &MemoryStore,
        working_directory: Option<String>,
        files_skipped: Option<Vec<String>>,
        files_errors: Option<Vec<String>>,
        review_config: Option<&crate::config::ReviewConfig>,
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
            // When models omitted: use config default_models if provided,
            // otherwise fall back to all registry models.
            // Claude (the MCP client) handles intelligent Tier 2 selection via the
            // unified review skill — this is just the server-side fallback.
            let mut all: Vec<String> = if let Some(cfg) = review_config {
                cfg.default_models.clone()
            } else {
                self.registry
                    .list_models()
                    .iter()
                    .map(|(key, _)| (*key).clone())
                    .collect()
            };
            all.sort();
            if all.len() > MAX_MODELS {
                let dropped: Vec<&str> = all[MAX_MODELS..].iter().map(|s| s.as_str()).collect();
                let msg = format!(
                    "Auto-selected {} models but max is {MAX_MODELS}. Dropped: {:?}.",
                    all.len(),
                    dropped,
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
                all.truncate(MAX_MODELS);
            }
            all
        };

        // Track whether models were auto-selected (default_models used).
        let auto_selected = req.models.is_none() && review_config.is_some();

        // Capture pre-gate count for accurate API accounting (Bug #4).
        let mut target_models = target_models;
        let original_model_count = target_models.len();

        // Hard gate: exclude models below success threshold.
        // Models with insufficient samples pass through (can't judge on tiny data).
        // If ALL models would be gated, restore the original list (never dispatch to zero).
        // Diagnostic: gate warnings include timeout/cutoff breakdown + avg failed prompt size.
        let mut gated_count = 0usize;
        let id_to_key = self.registry.model_id_to_key();
        if let Some(stats) = memory.get_model_stats(Some(&id_to_key)).await {
            let original = target_models.clone();
            let mut gated = Vec::new();
            target_models.retain(|model| {
                if let Some(s) = stats.get(model)
                    && s.sample_count >= MIN_GATE_SAMPLES
                    && s.success_rate < MIN_SUCCESS_RATE
                {
                    // Diagnostic gate warning: break down WHY the model is failing
                    let timing = s.timeout_count + s.cutoff_count;
                    let mut detail = format!(
                        "{model}: {:.1}% success ({} samples",
                        s.success_rate * 100.0,
                        s.sample_count
                    );
                    if timing > 0 {
                        detail.push_str(&format!(", {}/{} timeout/cutoff", timing, s.sample_count));
                        if s.avg_failed_prompt_len > 0 {
                            detail.push_str(&format!(
                                ", avg failed prompt {}chars",
                                s.avg_failed_prompt_len
                            ));
                        }
                    }
                    if s.partial_count > 0 {
                        detail.push_str(&format!(", {} partial", s.partial_count));
                    }
                    detail.push(')');
                    gated.push(detail);
                    return false;
                }
                true
            });
            gated_count = gated.len();
            if !gated.is_empty() {
                let msg = format!(
                    "Models excluded by hard gate (<{:.1}% success, >={} samples): {}",
                    MIN_SUCCESS_RATE * 100.0,
                    MIN_GATE_SAMPLES,
                    gated.join("; ")
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
            }
            // Safety: never dispatch to zero models (only if gate actually excluded something)
            if target_models.is_empty() && !gated.is_empty() {
                let msg =
                    "All requested models below success threshold — proceeding with original list"
                        .to_string();
                tracing::warn!("{msg}");
                warnings.push(msg);
                target_models = original.clone();
                gated_count = 0; // reset: gate was overridden
            }

            // Exploration slot: re-add one gated model when >50% of its failures are
            // timeouts/cutoffs (suggests a config problem, not a quality problem).
            // Skip when all models were gated (fallback already restored full list).
            if gated_count > 0 && !target_models.is_empty() {
                let best_timeout_gated = original
                    .iter()
                    .filter(|m| !target_models.contains(m))
                    .filter_map(|m| stats.get(m).map(|s| (m, s)))
                    .filter(|(_, s)| {
                        let timing = s.timeout_count + s.cutoff_count;
                        let successes = (s.success_rate * s.sample_count as f64).round() as usize;
                        let failures = s.sample_count.saturating_sub(successes);
                        // >50% of failures are timing-related
                        failures > 0 && timing * 2 > failures
                    })
                    .max_by(|a, b| {
                        a.1.success_rate
                            .partial_cmp(&b.1.success_rate)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some((model, s)) = best_timeout_gated {
                    target_models.push(model.clone());
                    gated_count -= 1; // accurate accounting for ReviewSummary
                    let msg = format!(
                        "Exploration slot: re-adding {model} ({:.1}% success, \
                         {}/{} timeout/cutoff — likely config issue)",
                        s.success_rate * 100.0,
                        s.timeout_count + s.cutoff_count,
                        s.sample_count
                    );
                    tracing::info!("{msg}");
                    warnings.push(msg);
                }
            }
        }

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

        // Resolve per_model_system_prompts keys with fuzzy matching.
        // Builds a normalized map keyed by exact config keys.
        let target_set: HashSet<&String> = model_providers.iter().map(|(m, _)| m).collect();
        let id_to_key = self.registry.model_id_to_key();

        let resolved_per_model_prompts: Option<HashMap<String, String>> =
            req.per_model_system_prompts.as_ref().map(|per_model| {
                let mut resolved = HashMap::new();
                let mut unresolved: Vec<&String> = Vec::new();
                for (key, prompt) in per_model {
                    if let Some(matched) = resolve_per_model_key(key, &target_set, &id_to_key) {
                        if key != matched.as_str() {
                            warnings.push(format!(
                                "per_model_system_prompts key '{key}' resolved to '{matched}'"
                            ));
                        }
                        resolved.insert(matched.clone(), prompt.clone());
                    } else {
                        unresolved.push(key);
                    }
                }
                if !unresolved.is_empty() {
                    let msg = format!(
                        "per_model_system_prompts contains unknown models: {unresolved:?}. Check listmodels for valid names."
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                }
                resolved
            });

        // Resolve per_model_timeout_secs keys with fuzzy matching.
        let resolved_per_model_timeouts: Option<HashMap<String, u64>> =
            req.per_model_timeout_secs.as_ref().map(|per_model| {
                let mut resolved = HashMap::new();
                let mut unresolved: Vec<&String> = Vec::new();
                for (key, timeout) in per_model {
                    if let Some(matched) = resolve_per_model_key(key, &target_set, &id_to_key) {
                        if key != matched.as_str() {
                            warnings.push(format!(
                                "per_model_timeout_secs key '{key}' resolved to '{matched}'"
                            ));
                        }
                        resolved.insert(matched.clone(), *timeout);
                    } else {
                        unresolved.push(key);
                    }
                }
                if !unresolved.is_empty() {
                    let msg = format!(
                        "per_model_timeout_secs contains unknown models: {unresolved:?}. Check listmodels for valid names."
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                }
                // Warn on zero-value timeouts
                let zeros: Vec<&String> = resolved
                    .iter()
                    .filter(|&(_, v)| *v == 0)
                    .map(|(k, _)| k)
                    .collect();
                if !zeros.is_empty() {
                    let msg = format!(
                        "per_model_timeout_secs has 0 for {zeros:?} — this causes immediate timeout. Use at least 1."
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                }
                resolved
            });

        // Pin base timestamp before spawn loop to avoid per-model time skew.
        let base_now = Instant::now();

        // Share prompt across models via Arc — avoids cloning MB-scale buffers per model.
        let prompt: Arc<str> = Arc::from(prompt);

        for (model_id, provider) in &model_providers {
            let registry = self.registry.clone();
            let model_id = model_id.clone();
            let provider = provider.clone();
            let prompt = prompt.clone(); // Arc refcount bump, not a buffer copy
            // Per-model system prompt: use fuzzy-resolved map, fall back to shared
            let system_prompt = resolved_per_model_prompts
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
            let per_model_deadline = resolved_per_model_timeouts
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
        let selection_reasoning = if auto_selected {
            Some(format!(
                "Using default models from config: {:?}",
                review_config.map(|c| &c.default_models).unwrap_or(&vec![]),
            ))
        } else {
            None
        };
        let summary = ReviewSummary {
            models_requested: original_model_count,
            models_gated: gated_count,
            models_succeeded: results
                .iter()
                .filter(|r| r.status == ModelStatus::Success && !r.partial)
                .count(),
            models_failed: results
                .iter()
                .filter(|r| r.status == ModelStatus::Error && r.reason.as_deref() != Some("cutoff"))
                .count(),
            models_cutoff: results
                .iter()
                .filter(|r| r.reason.as_deref() == Some("cutoff"))
                .count(),
            models_partial: results
                .iter()
                .filter(|r| r.status == ModelStatus::Success && r.partial)
                .count(),
            models_not_started: not_started.len(),
            auto_selected,
            selection_reasoning,
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
            files_errors,
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
            reason: if pr.partial {
                Some("partial".to_string())
            } else {
                None
            },
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
    let mut payload = serde_json::to_value(response).map_err(std::io::Error::other)?;
    if let Some(ctx) = investigation_context {
        payload["investigation_context"] = serde_json::Value::String(ctx.to_string());
    }

    let json = serde_json::to_string_pretty(&payload).map_err(std::io::Error::other)?;

    // Atomic write: temp file + rename prevents partial reads.
    // Clean up temp file on ANY failure (write or rename).
    let tmp_path = path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp_path, json.as_bytes()).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(format!(".squall/reviews/{filename}"))
}
