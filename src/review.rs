use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static PERSIST_COUNTER: AtomicU64 = AtomicU64::new(0);

use tokio::task::{Id as TaskId, JoinSet};

/// Maximum number of models per review request (prevents DoS).
pub const MAX_MODELS: usize = 20;

use crate::dispatch::registry::Registry;
use crate::dispatch::ProviderRequest;
use crate::error::SquallError;
use crate::tools::review::{ModelStatus, ReviewModelResult, ReviewRequest, ReviewResponse};

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
    ) -> ReviewResponse {
        // Fix #3: Clamp timeout to prevent Instant overflow from untrusted input.
        // Report the effective (clamped) value in the response, not the raw request.
        let effective_cutoff_secs = req.timeout_secs().min(MAX_TIMEOUT_SECS);
        let cutoff = Duration::from_secs(effective_cutoff_secs);
        let start = Instant::now();

        // Determine which models to query (deduplicate, cap at MAX_MODELS)
        let target_models: Vec<String> = if let Some(ref specific) = req.models {
            let mut seen = HashSet::new();
            specific
                .iter()
                .filter(|m| seen.insert((*m).clone()))
                .take(MAX_MODELS)
                .cloned()
                .collect()
        } else {
            let mut all: Vec<String> = self.registry
                .list_models()
                .iter()
                .map(|m| m.model_id.clone())
                .collect();
            all.sort();
            all.truncate(MAX_MODELS);
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
        // JoinSet gives us abort_all() for cleanup on cutoff.
        let mut set = JoinSet::new();

        // Fix #1: Track task ID → model mapping for panic attribution
        let mut task_model_map: HashMap<TaskId, (String, String)> = HashMap::new();

        // Fix #4: Compute internal deadline once before loop (not per-iteration)
        let internal_deadline = Instant::now() + cutoff + Duration::from_secs(15);

        // Warn on unused per_model_system_prompts keys (likely caller typos)
        if let Some(ref per_model) = req.per_model_system_prompts {
            let target_set: HashSet<&String> = model_providers.iter().map(|(m, _)| m).collect();
            let unused: Vec<&String> = per_model.keys().filter(|k| !target_set.contains(k)).collect();
            if !unused.is_empty() {
                tracing::warn!(
                    unused_keys = ?unused,
                    "per_model_system_prompts contains keys not in target models"
                );
            }
        }

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
            // Fix #2: Thread working_directory through to CLI models
            let wd = working_directory.clone();

            // Clone before moving into async block — needed for task_model_map below
            let model_id_for_map = model_id.clone();
            let provider_for_map = provider.clone();

            let abort_handle = set.spawn(async move {
                let model_start = Instant::now();
                let provider_req = ProviderRequest {
                    prompt,
                    model: model_id.clone(),
                    deadline: internal_deadline,
                    working_directory: wd,
                    system_prompt,
                    temperature,
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
                            results.push(match query_result {
                                Ok(pr) => ReviewModelResult {
                                    model: pr.model,
                                    provider: pr.provider,
                                    status: ModelStatus::Success,
                                    response: Some(pr.text),
                                    error: None,
                                    reason: None,
                                    latency_ms,
                                },
                                Err(e) => ReviewModelResult {
                                    model: model_id,
                                    provider,
                                    status: ModelStatus::Error,
                                    response: None,
                                    error: Some(e.user_message()),
                                    reason: Some(error_reason(&e)),
                                    latency_ms,
                                },
                            });
                            if set.is_empty() { break; }
                        }
                        // Fix #1: Attribute panics to the correct model via task ID.
                        // Guard with is_panic() — cancelled tasks should not be
                        // reported as panics (defensive; cancellation is unexpected here).
                        Some(Err(join_err)) if join_err.is_panic() => {
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
                                });
                            }
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
                    // Straggler cutoff: abort all remaining tasks
                    set.abort_all();
                    break;
                }
            }
        }

        // Fix #5+#6: Drain results that completed during abort_all(), with a
        // grace-period timeout to prevent infinite hang if tasks ignore cancellation.
        let drain_grace = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(drain_grace);
        loop {
            tokio::select! {
                biased;
                join_result = set.join_next() => {
                    match join_result {
                        Some(Ok((model_id, provider, query_result, latency_ms))) => {
                            completed_models.insert(model_id.clone());
                            results.push(match query_result {
                                Ok(pr) => ReviewModelResult {
                                    model: pr.model,
                                    provider: pr.provider,
                                    status: ModelStatus::Success,
                                    response: Some(pr.text),
                                    error: None,
                                    reason: None,
                                    latency_ms,
                                },
                                Err(e) => ReviewModelResult {
                                    model: model_id,
                                    provider,
                                    status: ModelStatus::Error,
                                    response: None,
                                    error: Some(e.user_message()),
                                    reason: Some(error_reason(&e)),
                                    latency_ms,
                                },
                            });
                        }
                        Some(Err(join_err)) if join_err.is_panic() => {
                            // Panic during drain — attribute to correct model
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
                                });
                            }
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
                });
            }
        }

        // Persist to disk — failure must never lose in-memory results
        let (results_file, persist_error) =
            match persist_results(&results, &not_started, effective_cutoff_secs, elapsed_ms).await {
                Ok(path) => (Some(path), None),
                Err(e) => {
                    tracing::warn!("failed to persist review results: {e}");
                    (None, Some(e.to_string()))
                }
            };

        ReviewResponse {
            results,
            not_started,
            cutoff_seconds: effective_cutoff_secs,
            elapsed_ms,
            results_file,
            persist_error,
            files_skipped: None, // Set by server.rs after execute()
        }
    }
}

/// Classify a SquallError into a reason string for the review response.
fn error_reason(e: &SquallError) -> String {
    match e {
        SquallError::Timeout(_) => "timeout".to_string(),
        SquallError::RateLimited { .. } => "rate_limited".to_string(),
        SquallError::AuthFailed { .. } => "auth_failed".to_string(),
        SquallError::ModelNotFound { .. } => "model_not_found".to_string(),
        SquallError::SchemaParse(_) => "parse_error".to_string(),
        SquallError::ProcessExit { .. } => "process_exit".to_string(),
        _ => "error".to_string(),
    }
}

/// Write review results to `.squall/reviews/{timestamp}_{pid}_{seq}.json`.
/// Uses epoch millis + PID + atomic counter for filename uniqueness across
/// concurrent invocations and concurrent processes.
async fn persist_results(
    results: &[ReviewModelResult],
    not_started: &[String],
    cutoff_seconds: u64,
    elapsed_ms: u64,
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

    let payload = serde_json::json!({
        "results": results,
        "not_started": not_started,
        "cutoff_seconds": cutoff_seconds,
        "elapsed_ms": elapsed_ms,
    });

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
