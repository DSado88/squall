//! Tests for the review tool — multi-model dispatch with straggler cutoff.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use squall::config::Config;
use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
use squall::memory::MemoryStore;
use squall::review::ReviewExecutor;
use squall::tools::review::{
    ModelStatus, ReviewModelResult, ReviewRequest, ReviewResponse, ReviewSummary,
};

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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
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
            partial: false,
        }],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 1234,
        results_file: Some(".squall/reviews/test.json".to_string()),
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
    };

    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"status\":\"success\""));
    assert!(json.contains("\"model\":\"grok\""));
    assert!(json.contains("\"results_file\""));
    assert!(
        !json.contains("persist_error"),
        "None persist_error should be omitted"
    );
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
            partial: false,
        }],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 180000,
        results_file: None,
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
    };

    let json = serde_json::to_string(&resp).unwrap();
    // response field should be omitted (not "response":null)
    assert!(
        !json.contains("\"response\":null"),
        "None fields should be skipped: {json}"
    );
    // results_file should be omitted
    assert!(
        !json.contains("\"results_file\":null"),
        "None fields should be skipped: {json}"
    );
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
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
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
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
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
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(
        !json.contains("files_skipped"),
        "None files_skipped should be omitted: {json}"
    );
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
        ..Default::default()
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let start = Instant::now();
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let elapsed = start.elapsed();

    // Should complete within ~2s cutoff + 3s cooperative grace + overhead, not hang forever
    assert!(
        elapsed.as_secs() < 8,
        "Cutoff should have fired within ~5s (2s cutoff + 3s grace), took {elapsed:?}"
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let start = Instant::now();
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let start = Instant::now();
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let elapsed = start.elapsed();

    // Should take ~2s cutoff + 3s cooperative grace + overhead
    assert!(elapsed.as_secs() < 8, "Took {elapsed:?}");
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
        ..Default::default()
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

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
        ..Default::default()
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
    assert!(
        max <= 600,
        "MAX_TIMEOUT_SECS should not exceed 600, got {max}"
    );
    assert!(
        max >= 60,
        "MAX_TIMEOUT_SECS should be at least 60, got {max}"
    );
}

#[tokio::test]
async fn executor_clamps_huge_timeout() {
    // u64::MAX would overflow Instant arithmetic without clamping
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    // Should not panic — timeout is clamped internally
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

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
                    api_format: ApiFormat::OpenAi,
                },
                description: String::new(),
                strengths: vec![],
                weaknesses: vec![],
                speed_tier: "fast".to_string(),
                precision_tier: "medium".to_string(),
            },
        );
    }
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
        ..Default::default()
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
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
    let per_model = Some(HashMap::from([(
        "model-a".to_string(),
        "You are a security reviewer".to_string(),
    )]));
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
    let per_model = Some(HashMap::from([(
        "model-a".to_string(),
        "You are a security reviewer".to_string(),
    )]));
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
    let per_model = Some(HashMap::from([("model-a".to_string(), "".to_string())]));
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
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
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
        per_model_system_prompts: Some(HashMap::from([(
            "model-a".to_string(),
            "You are a security expert".to_string(),
        )])),
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    // Model will fail (fake endpoint), but executor should not panic
    assert_eq!(resp.results.len(), 1, "Should have one result");
    assert_eq!(
        resp.results[0].status,
        ModelStatus::Error,
        "Should error on fake endpoint"
    );
}

// ===========================================================================
// Phase 0: Deep mode + per-model timeout + stall timeout
// ===========================================================================

// ---------------------------------------------------------------------------
// 0A: Deep mode sets 600s timeout
// ---------------------------------------------------------------------------

#[test]
fn deep_mode_sets_600s_effective_timeout() {
    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None,
        timeout_secs: None, // no explicit timeout
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: Some(true),
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };
    assert_eq!(
        req.effective_timeout_secs(),
        600,
        "deep: true should default timeout to 600s"
    );
    assert_eq!(
        req.effective_reasoning_effort(),
        Some(squall::tools::enums::ReasoningEffort::High),
        "deep: true should default reasoning_effort to high"
    );
    assert_eq!(
        req.effective_max_tokens(),
        Some(16384),
        "deep: true should default max_tokens to 16384"
    );
}

#[test]
fn deep_mode_does_not_override_explicit_values() {
    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: None,
        timeout_secs: Some(300),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: Some(true),
        max_tokens: Some(4096),
        reasoning_effort: Some(squall::tools::enums::ReasoningEffort::Medium),
        context_format: None,
        response_format: None,
        investigation_context: None,
    };
    // Explicit timeout_secs overrides deep default (fix: was clamped to 600).
    assert_eq!(req.effective_timeout_secs(), 300);
    // explicit reasoning_effort should be kept
    assert_eq!(
        req.effective_reasoning_effort(),
        Some(squall::tools::enums::ReasoningEffort::Medium)
    );
    // explicit max_tokens should be kept
    assert_eq!(req.effective_max_tokens(), Some(4096));
}

#[test]
fn deep_false_uses_normal_defaults() {
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
        per_model_timeout_secs: None,
        deep: Some(false),
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };
    assert_eq!(req.effective_timeout_secs(), 180);
    assert_eq!(req.effective_reasoning_effort(), None);
    assert_eq!(req.effective_max_tokens(), None);
}

// ---------------------------------------------------------------------------
// 0A: Deep mode executor integration — uses effective_timeout_secs for cutoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deep_mode_executor_uses_effective_timeout() {
    // Register a model pointing at black-hole — will hang until cutoff
    let mut models = HashMap::new();
    models.insert(
        "slow-model".to_string(),
        ModelEntry {
            model_id: "slow-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://192.0.2.1:80/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["slow-model".to_string()]),
        timeout_secs: None, // would be 180 normally
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: Some(true), // should raise to 600s
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

    // RED: Currently executor uses req.timeout_secs() which returns 180 (ignoring deep).
    // GREEN: executor should use req.effective_timeout_secs() → 600.
    assert_eq!(
        resp.cutoff_seconds, 600,
        "deep: true should set effective cutoff to 600s, got {}",
        resp.cutoff_seconds
    );
}

// ---------------------------------------------------------------------------
// 0A: Per-model timeout does NOT extend global cutoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_model_timeout_does_not_extend_global_cutoff() {
    // Two models: fast-fail and slow (black-hole)
    let mut models = HashMap::new();
    models.insert(
        "fast-fail".to_string(),
        ModelEntry {
            model_id: "fast-fail".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    models.insert(
        "slow-model".to_string(),
        ModelEntry {
            model_id: "slow-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://192.0.2.1:80/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let mut per_model = HashMap::new();
    per_model.insert("slow-model".to_string(), 600u64); // per-model: 600s

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fast-fail".to_string(), "slow-model".to_string()]),
        timeout_secs: Some(3), // global cutoff: 3s
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: Some(per_model),
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let start = Instant::now();
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let elapsed = start.elapsed();

    // CRITICAL: Global cutoff at 3s should NOT be extended by per-model 600s.
    // The slow model should be cut off by the global timer, not its per-model timeout.
    assert!(
        elapsed.as_secs() < 10,
        "Per-model timeout should NOT extend global cutoff. Took {elapsed:?}"
    );
    assert_eq!(
        resp.cutoff_seconds, 3,
        "Global cutoff should remain at 3s, not extended by per-model timeout"
    );
}

// ---------------------------------------------------------------------------
// 0B: Stall timeout override on ProviderRequest
// ---------------------------------------------------------------------------

#[test]
fn stall_timeout_for_reasoning_unchanged() {
    use squall::dispatch::http::stall_timeout_for;
    use std::time::Duration;
    assert_eq!(stall_timeout_for(Some("high")), Duration::from_secs(300));
    assert_eq!(stall_timeout_for(Some("medium")), Duration::from_secs(300));
    assert_eq!(stall_timeout_for(None), Duration::from_secs(60));
    assert_eq!(stall_timeout_for(Some("low")), Duration::from_secs(60));
}

// ===========================================================================
// Quality Gates: warnings, summary, investigation_context
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. Unknown per_model_system_prompts keys surface as warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn warnings_surface_unknown_per_model_system_prompt_keys() {
    let mut models = HashMap::new();
    models.insert(
        "real-model".to_string(),
        ModelEntry {
            model_id: "real-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["real-model".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: Some(HashMap::from([
            ("real-model".to_string(), "valid lens".to_string()),
            ("typo-model".to_string(), "orphan lens".to_string()),
        ])),
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("per_model_system_prompts") && w.contains("typo-model")),
        "Should warn about unknown per_model_system_prompts key. Warnings: {:?}",
        resp.warnings,
    );
}

// ---------------------------------------------------------------------------
// 2. Unknown per_model_timeout_secs keys surface as warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn warnings_surface_unknown_per_model_timeout_keys() {
    let mut models = HashMap::new();
    models.insert(
        "real-model".to_string(),
        ModelEntry {
            model_id: "real-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["real-model".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: Some(HashMap::from([
            ("real-model".to_string(), 60u64),
            ("ghost-model".to_string(), 120u64),
        ])),
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("per_model_timeout_secs") && w.contains("ghost-model")),
        "Should warn about unknown per_model_timeout_secs key. Warnings: {:?}",
        resp.warnings,
    );
}

// ---------------------------------------------------------------------------
// 3. MAX_MODELS truncation surfaces as warning with dropped model names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn warnings_surface_max_models_truncation() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    // Request MAX_MODELS + 3 unique models (all unknown — that's fine, we're testing the warning)
    let model_names: Vec<String> = (0..squall::review::MAX_MODELS + 3)
        .map(|i| format!("model-{i:02}"))
        .collect();

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(model_names),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(
        resp.warnings.iter().any(|w| w.contains("Dropped")),
        "Should warn about MAX_MODELS truncation. Warnings: {:?}",
        resp.warnings,
    );
    // Verify the warning includes the dropped model names
    assert!(
        resp.warnings.iter().any(|w| w.contains("model-20")),
        "Warning should include the dropped model names. Warnings: {:?}",
        resp.warnings,
    );
}

// ---------------------------------------------------------------------------
// 4. Summary counts match actual results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn summary_counts_match_results() {
    // One real model (will fail — connection refused) + one unknown model (not_started)
    let mut models = HashMap::new();
    models.insert(
        "fail-model".to_string(),
        ModelEntry {
            model_id: "fail-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fail-model".to_string(), "ghost-model".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let s = &resp.summary;

    assert_eq!(s.models_requested, 2, "2 models requested (post-dedup)");
    assert_eq!(s.models_not_started, 1, "ghost-model is unknown");
    assert_eq!(s.models_failed, 1, "fail-model connection refused");
    assert_eq!(s.models_succeeded, 0, "no models succeeded");
    assert_eq!(s.models_cutoff, 0, "no cutoff");
    assert_eq!(s.models_partial, 0, "no partial results");
}

// ---------------------------------------------------------------------------
// 5. Summary buckets reconcile (invariant test)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn summary_buckets_reconcile() {
    // Two real models (both will fail) + one unknown
    let mut models = HashMap::new();
    for name in &["model-a", "model-b"] {
        models.insert(
            name.to_string(),
            ModelEntry {
                model_id: name.to_string(),
                provider: "test".to_string(),
                backend: BackendConfig::Http {
                    base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                    api_key: "fake".to_string(),
                    api_format: ApiFormat::OpenAi,
                },
                description: String::new(),
                strengths: vec![],
                weaknesses: vec![],
                speed_tier: "fast".to_string(),
                precision_tier: "medium".to_string(),
            },
        );
    }
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec![
            "model-a".to_string(),
            "model-b".to_string(),
            "unknown".to_string(),
        ]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let s = &resp.summary;

    // Invariant: gated + succeeded + failed + cutoff + partial + not_started == requested
    // (requested is pre-gate, post-dedup count)
    let total = s.models_gated
        + s.models_succeeded
        + s.models_failed
        + s.models_cutoff
        + s.models_partial
        + s.models_not_started;
    assert_eq!(
        total, s.models_requested,
        "Summary buckets must reconcile: {s:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. investigation_context persisted to disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn investigation_context_persisted() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: Some("Found potential race condition in auth flow".to_string()),
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    let path = resp.results_file.expect("should persist results");

    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(
        content.contains("Found potential race condition in auth flow"),
        "Persisted file should contain investigation_context. Content: {content}"
    );

    // Cleanup
    let _ = tokio::fs::remove_file(&path).await;
}

// ---------------------------------------------------------------------------
// 7. investigation_context clamped at 32KB with warning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn investigation_context_clamped() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    // Create a context larger than 32KB
    let big_context = "x".repeat(40_000);

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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: Some(big_context),
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

    // Should have a truncation warning
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("investigation_context was truncated")),
        "Should warn about clamped investigation_context. Warnings: {:?}",
        resp.warnings,
    );

    // Persisted file should contain truncated context (32KB, not 40KB)
    if let Some(ref path) = resp.results_file {
        let content = tokio::fs::read_to_string(path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let ctx = parsed["investigation_context"].as_str().unwrap();
        assert_eq!(ctx.len(), 32 * 1024, "Context should be clamped to 32KB");
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ---------------------------------------------------------------------------
// 7b. investigation_context clamped safely on multi-byte UTF-8 boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn investigation_context_clamped_utf8_boundary() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    // Build a string where the 32KB boundary falls inside a multi-byte char.
    // U+1F600 (😀) is 4 bytes in UTF-8. Fill up to just before 32KB, then add emoji
    // so the 4-byte char straddles the 32768 boundary.
    let max = 32 * 1024; // 32768
    let padding = "a".repeat(max - 2); // 32766 ASCII bytes
    let big_context = format!("{padding}😀😀😀"); // 32766 + 12 = 32778 bytes, boundary at 32768 is inside first emoji

    assert!(big_context.len() > max);

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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: Some(big_context),
    };

    // This should NOT panic (previously would on &ctx[..MAX])
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("investigation_context was truncated")),
        "Should warn about clamped investigation_context. Warnings: {:?}",
        resp.warnings,
    );

    // Persisted file should contain valid UTF-8 context truncated at char boundary
    if let Some(ref path) = resp.results_file {
        let content = tokio::fs::read_to_string(path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let ctx = parsed["investigation_context"].as_str().unwrap();
        // Should be truncated to the char boundary before 32768 (32766 in this case)
        assert!(
            ctx.len() <= max,
            "Context should be <= 32KB, got {}",
            ctx.len()
        );
        assert!(
            ctx.len() >= max - 4,
            "Should truncate near the boundary, got {}",
            ctx.len()
        );
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ---------------------------------------------------------------------------
// 8. Empty warnings omitted from serialized JSON
// ---------------------------------------------------------------------------

#[test]
fn empty_warnings_omitted_in_json() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(
        !json.contains("\"warnings\""),
        "Empty warnings should be omitted from JSON: {json}"
    );
}

// ===========================================================================
// RED tests: bugs found by 5-model deep review
// ===========================================================================

// ---------------------------------------------------------------------------
// DR-1: Persisted JSON should contain files_skipped (currently always None)
//
// Bug: server.rs sets files_skipped AFTER execute() returns, but persist
// happens INSIDE execute(). Persisted file never contains files_skipped.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn persisted_json_contains_files_skipped() {
    // We can't easily test through server.rs, so verify the persisted file
    // has files_skipped when the response has it. This tests that persist
    // captures the field. Currently fails because persist runs before
    // files_skipped is set.
    let mut models = HashMap::new();
    models.insert(
        "fast".to_string(),
        ModelEntry {
            model_id: "fast".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fast".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let skipped = Some(vec!["big_file.rs (50000B)".to_string()]);
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            skipped,
            None,
            None,
        )
        .await;

    // Verify persisted file contains files_skipped (previously lost because
    // server.rs set it AFTER execute, but persist ran INSIDE execute).
    if let Some(ref path) = resp.results_file {
        let content = tokio::fs::read_to_string(path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed.get("files_skipped").is_some() && !parsed["files_skipped"].is_null(),
            "Persisted JSON should contain files_skipped, but it's missing. \
             This proves the bug: persist runs before server.rs sets files_skipped. \
             Persisted: {}",
            parsed
                .get("files_skipped")
                .map(|v| v.to_string())
                .unwrap_or("ABSENT".into()),
        );
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ---------------------------------------------------------------------------
// DR-2: Truncation warning should report actual byte count after char walkback
//
// Bug: Warning says "truncated to {MAX}" but actual size may be less due to
// UTF-8 char boundary walkback. E.g., 32766 bytes retained but warning says 32768.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn truncation_warning_reports_actual_boundary() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    // Force a walkback: 32766 ASCII bytes + 4-byte emoji = 32770 bytes.
    // Truncation at 32768 falls inside the emoji, walks back to 32766.
    let max = 32 * 1024; // 32768
    let padding = "a".repeat(max - 2); // 32766
    let big_context = format!("{padding}😀😀"); // 32766 + 8 = 32774

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
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: Some(big_context.clone()),
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

    // The warning should mention the ACTUAL retained byte count (32766),
    // not the MAX (32768).
    let warning = resp
        .warnings
        .iter()
        .find(|w| w.contains("investigation_context was truncated"))
        .expect("Should have truncation warning");
    assert!(
        warning.contains("32766") || warning.contains(&format!("to {}", max - 2)),
        "Warning should report actual truncation boundary (32766), not MAX (32768). \
         Got: {warning}"
    );

    // Cleanup
    if let Some(ref path) = resp.results_file {
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ---------------------------------------------------------------------------
// DR-3: ProcessGroupGuard should be disarmed after successful child.wait()
//
// Bug: After child exits and is reaped, PID is freed for reuse. Guard still
// holds old PID and sends SIGKILL on drop, potentially killing wrong process.
//
// Proving the bug: spawn a short-lived child via CLI dispatch, wait for
// success, then check that no SIGKILL is sent to the (now-freed) PID.
// We prove this by spawning a process group, then checking if an unrelated
// process with the same PID would survive. Since PID reuse is hard to
// trigger deterministically, we verify the fix exists as a unit test
// inside src/dispatch/cli.rs.
// ---------------------------------------------------------------------------

// (DR-3 unit test lives in src/dispatch/cli.rs — cannot test private guard from here)

// ===========================================================================
// RED tests: bugs found by meta deep review (deep review of deep review)
// ===========================================================================

// ---------------------------------------------------------------------------
// DR2-1: per_model_timeout_secs=0 causes immediate Timeout(0)
//
// Bug: src/review.rs:174-182 — Duration::from_secs(0) creates an immediate
// deadline. Model is dispatched but times out instantly. No warning emitted.
// Should warn when per_model_timeout_secs contains a zero value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn zero_per_model_timeout_warns() {
    let mut models = HashMap::new();
    models.insert(
        "test-model".to_string(),
        ModelEntry {
            model_id: "test-model".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["test-model".to_string()]),
        timeout_secs: Some(30),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: Some(HashMap::from([("test-model".to_string(), 0u64)])),
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;

    // A per_model_timeout of 0 should surface a warning — silently timing out
    // with Duration::from_secs(0) is never the caller's intent.
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("timeout") && w.contains("0")),
        "Should warn about zero per_model_timeout_secs. Warnings: {:?}",
        resp.warnings,
    );
}

// ---------------------------------------------------------------------------
// DR2-2: file_result.errors not captured in review response
//
// Bug: src/server.rs:304 — only file_result.skipped is captured. File read
// errors (non-existent files, permission errors) are silently dropped.
// ReviewResponse should surface file errors for caller visibility.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn persisted_json_contains_files_errors() {
    let mut models = HashMap::new();
    models.insert(
        "fast".to_string(),
        ModelEntry {
            model_id: "fast".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["fast".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let file_errors = Some(vec![
        "nonexistent.rs: No such file or directory".to_string(),
        "secret.key: Permission denied".to_string(),
    ]);
    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            file_errors,
            None,
        )
        .await;

    // Verify files_errors field is present and correct in serialized JSON.
    let json = serde_json::to_string(&resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(
        parsed.get("files_errors").is_some(),
        "ReviewResponse JSON should have files_errors field. \
         Available fields: {:?}",
        parsed.as_object().unwrap().keys().collect::<Vec<_>>(),
    );
    let errors_array = parsed["files_errors"].as_array().unwrap();
    assert_eq!(errors_array.len(), 2, "Should have 2 file errors");
    assert!(errors_array[0].as_str().unwrap().contains("nonexistent.rs"));

    // Verify persisted to disk
    if let Some(ref path) = resp.results_file {
        let content = tokio::fs::read_to_string(path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed.get("files_errors").is_some() && !parsed["files_errors"].is_null(),
            "Persisted JSON should contain files_errors"
        );
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ---------------------------------------------------------------------------
// Fuzzy per_model_system_prompts key resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_model_system_prompt_case_insensitive_resolution() {
    let mut models = HashMap::new();
    models.insert(
        "grok".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["grok".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: Some(HashMap::from([(
            "Grok".to_string(),
            "security lens".to_string(),
        )])),
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    // Should warn about resolution, not about unknown key
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("resolved to 'grok'")),
        "Should warn about fuzzy resolution. Warnings: {:?}",
        resp.warnings,
    );
    assert!(
        !resp.warnings.iter().any(|w| w.contains("unknown models")),
        "Should NOT warn about unknown models. Warnings: {:?}",
        resp.warnings,
    );
}

#[tokio::test]
async fn per_model_system_prompt_resolves_provider_model_id() {
    let mut models = HashMap::new();
    models.insert(
        "grok".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["grok".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: Some(HashMap::from([(
            "grok-4-1-fast-reasoning".to_string(),
            "architecture lens".to_string(),
        )])),
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(
        resp.warnings
            .iter()
            .any(|w| w.contains("resolved to 'grok'")),
        "Should resolve provider model_id to config key. Warnings: {:?}",
        resp.warnings,
    );
}

#[tokio::test]
async fn per_model_system_prompt_exact_match_no_warning() {
    let mut models = HashMap::new();
    models.insert(
        "grok".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "test".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                api_key: "fake".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "hello".to_string(),
        models: Some(vec!["grok".to_string()]),
        timeout_secs: Some(5),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: Some(HashMap::from([(
            "grok".to_string(),
            "exact lens".to_string(),
        )])),
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    // Exact match should produce no resolution warnings
    assert!(
        !resp.warnings.iter().any(|w| w.contains("resolved to")),
        "Exact match should NOT produce resolution warning. Warnings: {:?}",
        resp.warnings,
    );
}

// ===========================================================================
// Markdown response format
// ===========================================================================

#[test]
fn review_to_markdown_returns_summary_header() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 1234,
        results_file: Some("/tmp/results.json".to_string()),
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary {
            models_requested: 3,
            models_gated: 0,
            models_succeeded: 2,
            models_failed: 1,
            models_cutoff: 0,
            models_partial: 0,
            models_not_started: 0,
            auto_selected: false,
            selection_reasoning: None,
        },
    };

    let md = resp.to_markdown(false);
    assert!(
        md.contains("## Review Summary"),
        "Should contain summary header"
    );
    assert!(md.contains("2 succeeded"), "Should include succeeded count");
    assert!(md.contains("1234ms elapsed"), "Should include elapsed time");
    assert!(
        md.contains("results.json"),
        "Should include results_file path"
    );
    // Should NOT contain JSON artifacts
    assert!(!md.contains('{'), "Markdown should not contain JSON braces");
}

#[test]
fn review_to_markdown_concise_omits_model_text() {
    let resp = ReviewResponse {
        results: vec![ReviewModelResult {
            model: "grok".to_string(),
            provider: "xai".to_string(),
            status: ModelStatus::Success,
            response: Some("Long model response text here".to_string()),
            error: None,
            reason: None,
            latency_ms: 500,
            partial: false,
        }],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 600,
        results_file: Some("/tmp/r.json".to_string()),
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary {
            models_requested: 1,
            models_gated: 0,
            models_succeeded: 1,
            models_failed: 0,
            models_cutoff: 0,
            models_partial: 0,
            models_not_started: 0,
            auto_selected: false,
            selection_reasoning: None,
        },
    };

    let concise = resp.to_markdown(true);
    assert!(
        !concise.contains("Long model response text"),
        "Concise mode should omit model response text"
    );
    assert!(
        concise.contains("1 succeeded"),
        "Concise should still have summary"
    );

    let detailed = resp.to_markdown(false);
    assert!(
        detailed.contains("Long model response text"),
        "Detailed mode should include model response text"
    );
    assert!(
        detailed.contains("### grok"),
        "Detailed should have per-model headers"
    );
}

#[test]
fn review_to_markdown_shows_warnings() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec!["missing-model".to_string()],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: None,
        files_errors: None,
        warnings: vec!["Unknown key 'typo' in per_model_system_prompts".to_string()],
        summary: ReviewSummary::default(),
    };

    let md = resp.to_markdown(false);
    assert!(md.contains("### Warnings"), "Should have warnings section");
    assert!(md.contains("Unknown key"), "Should include warning text");
    assert!(
        md.contains("missing-model"),
        "Should show not-started models"
    );
}

// ===========================================================================
// Defect: to_markdown() must surface files_skipped and files_errors
// ===========================================================================

#[test]
fn review_to_markdown_shows_files_skipped() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: Some(vec!["large_file.rs".to_string(), "huge.rs".to_string()]),
        files_errors: None,
        warnings: vec![],
        summary: ReviewSummary::default(),
    };

    let md = resp.to_markdown(false);
    assert!(
        md.contains("large_file.rs"),
        "Markdown should surface skipped files"
    );
    assert!(
        md.contains("huge.rs"),
        "Markdown should surface all skipped files"
    );
}

#[test]
fn review_to_markdown_shows_files_errors() {
    let resp = ReviewResponse {
        results: vec![],
        not_started: vec![],
        cutoff_seconds: 180,
        elapsed_ms: 100,
        results_file: None,
        persist_error: None,
        files_skipped: None,
        files_errors: Some(vec!["missing.rs: not found".to_string()]),
        warnings: vec![],
        summary: ReviewSummary::default(),
    };

    let md = resp.to_markdown(false);
    assert!(
        md.contains("missing.rs: not found"),
        "Markdown should surface file errors"
    );
}
