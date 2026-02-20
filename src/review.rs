use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::task::{Id as TaskId, JoinSet};

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
        // Fix #3: Clamp timeout to prevent Instant overflow from untrusted input
        let cutoff = Duration::from_secs(req.timeout_secs().min(MAX_TIMEOUT_SECS));
        let start = Instant::now();

        // Determine which models to query
        let target_models: Vec<String> = if let Some(ref specific) = req.models {
            specific.clone()
        } else {
            self.registry
                .list_models()
                .iter()
                .map(|m| m.model_id.clone())
                .collect()
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

        for (model_id, provider) in &model_providers {
            let registry = self.registry.clone();
            let model_id = model_id.clone();
            let provider = provider.clone();
            let prompt = prompt.clone();
            let system_prompt = req.system_prompt.clone();
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
                        // Fix #1: Attribute panics to the correct model via task ID
                        Some(Err(join_err)) => {
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

        // Fix #5: Drain any results that completed during abort_all().
        // A task can finish between the deadline arm winning and abort_all() completing.
        while let Some(join_result) = set.join_next().await {
            if let Ok((model_id, provider, query_result, latency_ms)) = join_result {
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
            // Aborted tasks return JoinError with is_cancelled — ignore them
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
        let results_file = match persist_results(&results, &not_started, req.timeout_secs(), elapsed_ms).await {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::warn!("failed to persist review results: {e}");
                None
            }
        };

        ReviewResponse {
            results,
            not_started,
            cutoff_seconds: req.timeout_secs(),
            elapsed_ms,
            results_file,
        }
    }
}

/// Classify a SquallError into a reason string for the review response.
fn error_reason(e: &SquallError) -> String {
    match e {
        SquallError::Timeout(_) => "timeout".to_string(),
        SquallError::RateLimited { .. } => "rate_limited".to_string(),
        SquallError::AuthFailed { .. } => "auth_failed".to_string(),
        SquallError::ModelNotFound(_) => "model_not_found".to_string(),
        SquallError::SchemaParse(_) => "parse_error".to_string(),
        SquallError::ProcessExit { .. } => "process_exit".to_string(),
        _ => "error".to_string(),
    }
}

/// Write review results to `.squall/reviews/{timestamp}_{pid}.json`.
/// Uses epoch nanos + PID for filename uniqueness across concurrent invocations.
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
        .as_nanos();
    let pid = std::process::id();
    let filename = format!("{ts}_{pid}.json");
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
    tokio::fs::rename(&tmp_path, &path).await?;

    Ok(format!(".squall/reviews/{filename}"))
}
