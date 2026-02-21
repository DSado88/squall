//! Tests for the review tool — multi-model dispatch with straggler cutoff.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use squall::config::Config;
use squall::dispatch::registry::{BackendConfig, ModelEntry, Registry};
use squall::review::ReviewExecutor;
use squall::tools::review::{ModelStatus, ReviewRequest, ReviewResponse};

// ---------------------------------------------------------------------------
// Helper: resolve per-model system prompt (mirrors executor logic exactly)
// ---------------------------------------------------------------------------
fn resolve_system_prompt(
    per_model: &Option<HashMap<String, String>>,
    model_id: &str,
    shared: &Option<String>,
) -> Option<String> {
    per_model
        .as_ref()
        .and_then(|map| map.get(model_id).cloned())
        .or_else(|| shared.clone())
}

// ---------------------------------------------------------------------------
// ReviewRequest defaults
// ---------------------------------------------------------------------------

#[test]
fn review_request_default_timeout() {
    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None,
        timeout_secs: None,
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };
    assert_eq!(req.timeout_secs(), 180);
}

#[test]
fn review_request_custom_timeout() {
    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None,
        timeout_secs: Some(60),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };
    assert_eq!(req.timeout_secs(), 60);
}

// ---------------------------------------------------------------------------
// ReviewResponse serialization
// ---------------------------------------------------------------------------

#[test]
fn review_response_serializes_to_json() {
    let resp = ReviewResponse {
        results: vec![squall::tools::review::ReviewModelResult {
            model: "grok".to_string(),
            provider: "xai".to_string(),
            status: ModelStatus::Success,
            response: Some("analysis here".to_string()),
            error: None,
            reason: None,
            latency_ms: 1234,
        }],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 1234,
        results_file: Some(".squall/reviews/test.json".to_string()),
        persist_error: None,
        files_skipped: None,
    };

    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"status\":\"success\""));
    assert!(json.contains("\"model\":\"grok\""));
    assert!(json.contains("\"results_file\""));
    assert!(!json.contains("persist_error"), "None persist_error should be omitted");
}

#[test]
fn review_response_omits_none_fields() {
    let resp = ReviewResponse {
        results: vec![squall::tools::review::ReviewModelResult {
            model: "test".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Error,
            response: None,
            error: Some("timeout".to_string()),
            reason: Some("cutoff".to_string()),
            latency_ms: 180000,
        }],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 180000,
        results_file: None,
        persist_error: None,
        files_skipped: None,
    };

    let json = serde_json::to_string(&resp).unwrap();
    // response field should be omitted (not "response":null)
    assert!(!json.contains("\"response\":null"), "None fields should be skipped: {json}");
    // results_file should be omitted
    assert!(!json.contains("\"results_file\":null"), "None fields should be skipped: {json}");
}

#[test]
fn review_response_includes_persist_error_when_set() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: Some("permission denied".to_string()),
        files_skipped: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"persist_error\":\"permission denied\""));
}

#[test]
fn review_response_includes_files_skipped_when_set() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: Some(vec!["large_file.rs (50000B)".to_string()]),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"files_skipped\""));
    assert!(json.contains("large_file.rs"));
}

#[test]
fn review_response_omits_files_skipped_when_none() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(!json.contains("files_skipped"), "None files_skipped should be omitted: {json}");
}

#[test]
fn model_status_serializes_as_snake_case() {
    let success = serde_json::to_string(&ModelStatus::Success).unwrap();
    let error = serde_json::to_string(&ModelStatus::Error).unwrap();
    assert_eq!(success, "\"success\"");
    assert_eq!(error, "\"error\"");
}

// ---------------------------------------------------------------------------
// ReviewExecutor: unknown models → not_started
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_unknown_models_go_to_not_started() {
    let config = Config {
        models: HashMap::new(),
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["nonexistent-model".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    assert!(resp.results.is_empty(), "No results for unknown models");
    assert_eq!(resp.not_started, vec!["nonexistent-model"]);
}

// ---------------------------------------------------------------------------
// ReviewExecutor: empty model list → uses all configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_none_models_uses_all_configured() {
    // Create a config with a model that will fail (no real API)
    let mut models = HashMap::new();
    models.insert(
        "test-model".to_string(),
        ModelEntry {
            model_id: "test-model".to_string(),
            provider: "test-provider".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake-key".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None, // should use all configured
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    // Should have tried test-model (and failed since the URL is bogus)
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].model, "test-model");
    assert_eq!(resp.results[0].status, ModelStatus::Error);
    assert!(resp.not_started.is_empty());
}

// ---------------------------------------------------------------------------
// ReviewExecutor: straggler cutoff aborts slow models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_cutoff_aborts_slow_models() {
    // Register a model pointing at a black-hole address — connection will hang
    let mut models = HashMap::new();
    models.insert(
        "slow-model".to_string(),
        ModelEntry {
            model_id: "slow-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                // 192.0.2.1 is TEST-NET-1 — packets are silently dropped (simulates slow)
                base_url: "http://192.0.2.1:80/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["slow-model".to_string()]),
        timeout_secs: Some(2), // 2 second cutoff
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let start = Instant::now();
    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    let elapsed = start.elapsed();

    // Should complete within ~2-3s (cutoff + small overhead), not hang forever
    assert!(
        elapsed.as_secs() < 5,
        "Cutoff should have fired within ~2s, took {elapsed:?}"
    );

    // The slow model should be marked as error with cutoff reason
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].status, ModelStatus::Error);
}

// ---------------------------------------------------------------------------
// ReviewExecutor: fast models complete before cutoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_fast_models_complete_before_cutoff() {
    // 127.0.0.1:1 will immediately refuse connection — fast error
    let mut models = HashMap::new();
    models.insert(
        "fast-fail".to_string(),
        ModelEntry {
            model_id: "fast-fail".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fast-fail".to_string()]),
        timeout_secs: Some(60), // generous cutoff
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let start = Instant::now();
    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    let elapsed = start.elapsed();

    // Should complete quickly (connection refused), NOT wait 60s for cutoff
    assert!(
        elapsed.as_secs() < 5,
        "Should not wait for cutoff when all models complete. Took {elapsed:?}"
    );
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].status, ModelStatus::Error);
}

// ---------------------------------------------------------------------------
// ReviewExecutor: mixed fast and slow models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_mixed_fast_and_slow() {
    let mut models = HashMap::new();
    // Fast fail (connection refused immediately)
    models.insert(
        "fast-fail".to_string(),
        ModelEntry {
            model_id: "fast-fail".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    // Slow (black-hole address)
    models.insert(
        "slow-model".to_string(),
        ModelEntry {
            model_id: "slow-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://192.0.2.1:80/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fast-fail".to_string(), "slow-model".to_string()]),
        timeout_secs: Some(2),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let start = Instant::now();
    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    let elapsed = start.elapsed();

    // Should take ~2s (cutoff), not more
    assert!(elapsed.as_secs() < 5, "Took {elapsed:?}");
    // Should have 2 results
    assert_eq!(resp.results.len(), 2, "Expected results for both models");
    // Both should be errors (one connection refused, one cutoff)
    assert!(
        resp.results.iter().all(|r| r.status == ModelStatus::Error),
        "Both should be errors"
    );
}

// ---------------------------------------------------------------------------
// ReviewExecutor: disk persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_persists_results_to_disk() {
    let config = Config {
        models: HashMap::new(),
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["nonexistent".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;

    // Should have a results_file path
    assert!(
        resp.results_file.is_some(),
        "Should persist results to disk"
    );

    let path = resp.results_file.as_ref().unwrap();
    assert!(path.starts_with(".squall/reviews/"));
    assert!(path.ends_with(".json"));

    // File should exist and be valid JSON
    let content = tokio::fs::read_to_string(path).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.get("results").is_some());
    assert!(parsed.get("not_started").is_some());
    assert!(parsed.get("cutoff_seconds").is_some());

    // Cleanup
    let _ = tokio::fs::remove_file(path).await;
}

// ---------------------------------------------------------------------------
// ReviewExecutor: MCP anti-cascade (always returns success)
// ---------------------------------------------------------------------------

#[test]
fn review_response_wraps_in_pal_without_is_error() {
    use squall::response::{PalMetadata, PalToolResponse};

    let review_json = serde_json::json!({
        "results": [],
        "not_started": ["bad-model"],
        "cutoff_seconds": 180,
        "elapsed_ms": 100,
    })
    .to_string();

    let response = PalToolResponse::success(
        review_json,
        PalMetadata {
            tool_name: "review".to_string(),
            model_used: "multi".to_string(),
            provider_used: "multi".to_string(),
            duration_seconds: 0.1,
        },
    );

    let result = response.into_call_tool_result();
    assert!(
        result.is_error != Some(true),
        "Review responses must NOT set is_error=true (anti-cascade)"
    );
}

// ---------------------------------------------------------------------------
// Server exposes review tool
// ---------------------------------------------------------------------------

#[test]
fn server_has_review_tool() {
    use rmcp::ServerHandler;
    use squall::server::SquallServer;

    let config = Config {
        models: HashMap::new(),
    };
    let server = SquallServer::new(config);
    let info = server.get_info();
    // Server should still initialize correctly with review tool
    assert_eq!(info.server_info.name, "squall");
}

// ---------------------------------------------------------------------------
// Fix #3: Timeout clamped to MAX_TIMEOUT_SECS (prevents Instant overflow)
// ---------------------------------------------------------------------------

#[test]
fn max_timeout_constant_is_reasonable() {
    let max = squall::review::MAX_TIMEOUT_SECS;
    assert!(max <= 600, "MAX_TIMEOUT_SECS should not exceed 600, got {max}");
    assert!(max >= 60, "MAX_TIMEOUT_SECS should be at least 60, got {max}");
}

#[tokio::test]
async fn executor_clamps_huge_timeout() {
    // u64::MAX would overflow Instant arithmetic without clamping
    let config = Config {
        models: HashMap::new(),
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["nonexistent".to_string()]),
        timeout_secs: Some(u64::MAX), // would panic without clamp
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    // Should not panic — timeout is clamped internally
    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    assert_eq!(
        resp.cutoff_seconds,
        squall::review::MAX_TIMEOUT_SECS,
        "Response should report the effective (clamped) cutoff, not the raw request"
    );
}

// ---------------------------------------------------------------------------
// Bug #1: Duplicate model IDs should be deduplicated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_deduplicates_model_ids() {
    // A fast-fail model (connection refused immediately)
    let mut models = HashMap::new();
    models.insert(
        "dupe-model".to_string(),
        ModelEntry {
            model_id: "dupe-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["dupe-model".to_string(), "dupe-model".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;

    // Should produce exactly 1 result, not 2
    assert_eq!(
        resp.results.len(),
        1,
        "Duplicate model IDs should be deduped — got {} results: {:?}",
        resp.results.len(),
        resp.results.iter().map(|r| &r.model).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Bug C2: MAX_MODELS not enforced on None branch (all configured models)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_caps_all_configured_models() {
    // Insert MAX_MODELS + 5 models into config
    let mut models = HashMap::new();
    for i in 0..(squall::review::MAX_MODELS + 5) {
        let id = format!("model-{i}");
        models.insert(
            id.clone(),
            ModelEntry {
                model_id: id,
                provider: "test".to_string(),
                backend: BackendConfig::Http {
                    base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                    api_key: "fake".to_string(),
                },
            },
        );
    }
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    // models: None → use all configured → should still be capped
    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None, // <-- the None branch
        timeout_secs: Some(2),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    let total = resp.results.len() + resp.not_started.len();

    // RED: None branch doesn't apply .take(MAX_MODELS), so all 25 models run
    // GREEN: .take(MAX_MODELS) applied → capped at 20
    assert!(
        total <= squall::review::MAX_MODELS,
        "models=None should be capped at MAX_MODELS ({}), got {total} total (results={}, not_started={})",
        squall::review::MAX_MODELS,
        resp.results.len(),
        resp.not_started.len(),
    );
}

// ---------------------------------------------------------------------------
// Bug #2: Persist filename should include PID for cross-process uniqueness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn persist_filename_includes_pid() {
    let config = Config {
        models: HashMap::new(),
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["nonexistent".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    let path = resp.results_file.expect("should persist results");
    let pid = std::process::id().to_string();

    assert!(
        path.contains(&pid),
        "Filename should include PID for cross-process safety, got: {path}"
    );

    // Cleanup
    let _ = tokio::fs::remove_file(&path).await;
}

// ---------------------------------------------------------------------------
// Per-model system prompt resolution
// ---------------------------------------------------------------------------

#[test]
fn per_model_system_prompt_overrides_shared() {
    let per_model = Some(HashMap::from([
        ("model-a".to_string(), "You are a security reviewer".to_string()),
    ]));
    let shared = Some("You are a code reviewer".to_string());

    let result = resolve_system_prompt(&per_model, "model-a", &shared);
    assert_eq!(
        result.as_deref(),
        Some("You are a security reviewer"),
        "Per-model prompt should override shared"
    );
}

#[test]
fn per_model_system_prompt_falls_back_to_shared() {
    let per_model = Some(HashMap::from([
        ("model-a".to_string(), "You are a security reviewer".to_string()),
    ]));
    let shared = Some("You are a code reviewer".to_string());

    let result = resolve_system_prompt(&per_model, "model-b", &shared);
    assert_eq!(
        result.as_deref(),
        Some("You are a code reviewer"),
        "Model not in per-model map should fall back to shared"
    );
}

#[test]
fn per_model_both_none_yields_none() {
    let result = resolve_system_prompt(&None, "model-a", &None);
    assert_eq!(result, None, "Both absent should yield None");
}

#[test]
fn per_model_empty_string_overrides() {
    // Intentional: explicit empty string means "no system prompt for this model"
    let per_model = Some(HashMap::from([
        ("model-a".to_string(), "".to_string()),
    ]));
    let shared = Some("You are a code reviewer".to_string());

    let result = resolve_system_prompt(&per_model, "model-a", &shared);
    assert_eq!(
        result.as_deref(),
        Some(""),
        "Empty string in per-model map should override shared (intentional: explicitly no system prompt)"
    );
}

// ---------------------------------------------------------------------------
// Per-model system prompts: executor integration (runs without panic)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_with_per_model_system_prompts() {
    let mut models = HashMap::new();
    models.insert(
        "model-a".to_string(),
        ModelEntry {
            model_id: "model-a".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
            },
        },
    );
    let config = Config { models };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "review this".to_string(),
        models: Some(vec!["model-a".to_string()]),
        timeout_secs: Some(5),
        system_prompt: Some("shared prompt".to_string()),
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: Some(HashMap::from([
            ("model-a".to_string(), "You are a security expert".to_string()),
        ])),
    };

    let resp = executor.execute(&req, req.prompt.clone(), None).await;
    // Model will fail (fake endpoint), but executor should not panic
    assert_eq!(resp.results.len(), 1, "Should have one result");
    assert_eq!(resp.results[0].status, ModelStatus::Error, "Should error on fake endpoint");
}
