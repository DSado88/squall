//! Tests for hard gate model filtering (Issue #8: Intelligent model selection).
//!
//! The hard gate excludes models with <70% success rate (MIN_SUCCESS_RATE)
//! when they have >= 5 samples (MIN_GATE_SAMPLES). Models with insufficient
//! data pass through. If ALL models would be gated, the original list is restored.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use squall::config::Config;
use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
use squall::memory::MemoryStore;
use squall::review::ReviewExecutor;
use squall::tools::review::ReviewRequest;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a MemoryStore in a unique temp dir with pre-populated models.md.
fn store_with_events(events: &str) -> (MemoryStore, PathBuf) {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("squall_gate_test_{id}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let models_path = dir.join("models.md");
    let content = format!(
        "# Model Performance Profiles\n\n\
         ## Summary\n(auto-generated)\n\n\
         ## Recent Events\n\
         | Timestamp | Model | Latency | Status | Partial | Reason | Tokens |\n\
         |-----------|-------|---------|--------|---------|--------|--------|\n\
         {events}"
    );
    std::fs::write(&models_path, content).unwrap();
    let store = MemoryStore::with_base_dir(dir.clone());
    (store, dir)
}

/// Build a minimal registry with the given model names (all HTTP backends).
fn test_registry(model_names: &[&str]) -> Arc<Registry> {
    let mut models = HashMap::new();
    for &name in model_names {
        models.insert(
            name.to_string(),
            ModelEntry {
                model_id: name.to_string(),
                provider: "test".to_string(),
                backend: BackendConfig::Http {
                    base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                    api_key: "key".to_string(),
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
    Arc::new(Registry::from_config(config))
}

fn make_request(models: Vec<&str>) -> ReviewRequest {
    ReviewRequest {
        prompt: "test".into(),
        models: Some(models.into_iter().map(String::from).collect()),
        timeout_secs: Some(3),
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    }
}

// ---------------------------------------------------------------------------
// Test: Model below threshold is excluded from dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_excludes_model_below_threshold() {
    // bad-model: 3 successes out of 10 = 30% success rate (below 70%)
    // good-model: 9 successes out of 10 = 90% success rate
    let events = "\
| 2026-02-23T10:00:00Z | bad-model | 10.0s | success | no | — | 1000 |
| 2026-02-23T10:01:00Z | bad-model | 10.0s | success | no | — | 1000 |
| 2026-02-23T10:02:00Z | bad-model | 10.0s | success | no | — | 1000 |
| 2026-02-23T10:03:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:04:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:05:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:06:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:07:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:08:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:09:00Z | bad-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:00:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:01:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:02:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:03:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:04:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:05:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:06:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:07:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:08:00Z | good-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:09:00Z | good-model | 5.0s | error | no | timeout | 1000 |";

    let (store, _dir) = store_with_events(events);
    let registry = test_registry(&["bad-model", "good-model"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["bad-model", "good-model"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // bad-model (30% success) should be excluded
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert!(
        !dispatched.contains(&"bad-model"),
        "bad-model (30% success) should be excluded by hard gate. Got: {dispatched:?}"
    );
    assert!(
        dispatched.contains(&"good-model"),
        "good-model (90% success) should pass hard gate. Got: {dispatched:?}"
    );

    // Warning should mention the exclusion
    let gate_warning = resp.warnings.iter().find(|w| w.contains("hard gate"));
    assert!(
        gate_warning.is_some(),
        "should have a hard gate warning. Warnings: {:?}",
        resp.warnings
    );
    assert!(
        gate_warning.unwrap().contains("bad-model"),
        "warning should name the excluded model"
    );
}

// ---------------------------------------------------------------------------
// Test: Model with insufficient samples bypasses gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_bypasses_model_with_insufficient_samples() {
    // new-model: 1 success out of 3 = 33% success, but only 3 samples (< MIN_GATE_SAMPLES)
    let events = "\
| 2026-02-23T10:00:00Z | new-model | 10.0s | success | no | — | 1000 |
| 2026-02-23T10:01:00Z | new-model | 10.0s | error | no | timeout | 1000 |
| 2026-02-23T10:02:00Z | new-model | 10.0s | error | no | timeout | 1000 |";

    let (store, _dir) = store_with_events(events);
    let registry = test_registry(&["new-model"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["new-model"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // new-model has only 3 samples (< MIN_GATE_SAMPLES=5), should NOT be gated
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert!(
        dispatched.contains(&"new-model"),
        "model with insufficient samples should bypass hard gate. Got: {dispatched:?}"
    );

    // No gate warning
    let gate_warning = resp.warnings.iter().any(|w| w.contains("hard gate"));
    assert!(
        !gate_warning,
        "should not have hard gate warning for insufficient samples. Warnings: {:?}",
        resp.warnings
    );
}

// ---------------------------------------------------------------------------
// Test: All models gated → fallback to original list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_fallback_when_all_models_gated() {
    // Both models have terrible success rates with sufficient samples
    let mut events = String::new();
    for i in 0..10 {
        events.push_str(&format!(
            "| 2026-02-23T10:{i:02}:00Z | model-a | 10.0s | error | no | timeout | 1000 |\n"
        ));
        events.push_str(&format!(
            "| 2026-02-23T10:{i:02}:00Z | model-b | 10.0s | error | no | timeout | 1000 |\n"
        ));
    }

    let (store, _dir) = store_with_events(&events);
    let registry = test_registry(&["model-a", "model-b"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["model-a", "model-b"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // Both models are 0% success, but since ALL would be gated,
    // the original list should be restored
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(
        dispatched.len(),
        2,
        "all-gated fallback should restore both models. Got: {dispatched:?}"
    );

    // Should have both the gate warning AND the fallback warning
    let has_gate_warning = resp.warnings.iter().any(|w| w.contains("hard gate"));
    let has_fallback_warning = resp
        .warnings
        .iter()
        .any(|w| w.contains("All requested models below"));
    assert!(has_gate_warning, "should have hard gate warning");
    assert!(has_fallback_warning, "should have fallback warning");
}

// ---------------------------------------------------------------------------
// Test: Model above threshold passes through
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_passes_model_above_threshold() {
    // model with exactly 70% success rate (boundary)
    let mut events = String::new();
    for i in 0..7 {
        events.push_str(&format!(
            "| 2026-02-23T10:{i:02}:00Z | border-model | 10.0s | success | no | — | 1000 |\n"
        ));
    }
    for i in 7..10 {
        events.push_str(&format!(
            "| 2026-02-23T10:{i:02}:00Z | border-model | 10.0s | error | no | timeout | 1000 |\n"
        ));
    }

    let (store, _dir) = store_with_events(&events);
    let registry = test_registry(&["border-model"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["border-model"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // 70% success = exactly at threshold (< 0.70 is the gate, so 70% passes)
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert!(
        dispatched.contains(&"border-model"),
        "model at exactly 70% should pass (gate is strictly less than). Got: {dispatched:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: Unknown model (not in memory) passes through
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_passes_unknown_model() {
    // Memory has data for model-a only. model-b is unknown to memory.
    let events = "\
| 2026-02-23T10:00:00Z | model-a | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:01:00Z | model-a | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:02:00Z | model-a | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:03:00Z | model-a | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:04:00Z | model-a | 5.0s | success | no | — | 1000 |";

    let (store, _dir) = store_with_events(events);
    let registry = test_registry(&["model-a", "model-b"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["model-a", "model-b"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // model-b has no stats in memory → should pass through (can't judge)
    // model-a is not in registry but IS in memory → not_started (registry miss, not gate)
    // Actually model-b is in registry but not memory → should be dispatched
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert!(
        dispatched.contains(&"model-b"),
        "model unknown to memory should pass hard gate. Got: {dispatched:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: No memory data → no gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hard_gate_noop_without_memory_data() {
    // Use default MemoryStore (no models.md exists)
    let store = MemoryStore::new();
    let registry = test_registry(&["some-model"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["some-model"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // No memory file → get_model_stats() returns None → no gating
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert!(
        dispatched.contains(&"some-model"),
        "without memory data, all models should pass. Got: {dispatched:?}"
    );

    let gate_warning = resp.warnings.iter().any(|w| w.contains("hard gate"));
    assert!(!gate_warning, "no gate warning without memory data");
}

// ---------------------------------------------------------------------------
// Test: get_model_stats returns correct stats
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_model_stats_computes_correctly() {
    let events = "\
| 2026-02-23T10:00:00Z | fast-model | 5.0s | success | no | — | 1000 |
| 2026-02-23T10:01:00Z | fast-model | 3.0s | success | no | — | 1000 |
| 2026-02-23T10:02:00Z | fast-model | 7.0s | error | no | timeout | 1000 |
| 2026-02-23T10:00:00Z | slow-model | 30.0s | success | no | — | 2000 |
| 2026-02-23T10:01:00Z | slow-model | 40.0s | success | no | — | 2000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let fast = stats
        .get("fast-model")
        .expect("fast-model should be present");
    assert_eq!(fast.sample_count, 3);
    assert!((fast.success_rate - 2.0 / 3.0).abs() < 0.01);
    assert!((fast.avg_latency_secs - 5.0).abs() < 0.01);

    let slow = stats
        .get("slow-model")
        .expect("slow-model should be present");
    assert_eq!(slow.sample_count, 2);
    assert!((slow.success_rate - 1.0).abs() < 0.01);
    assert!((slow.avg_latency_secs - 35.0).abs() < 0.01);
}

// ==========================================================================
// Bug fix tests: identity normalization, infrastructure exclusion, display
// rounding, partial inflation, models_requested accounting
// ==========================================================================

// Bug 1: Model identity normalization — stats keyed by model_id are
// remapped to config key when normalization map is provided.
#[tokio::test]
async fn normalization_maps_model_id_to_config_key() {
    let events = "\
| 2026-02-24T10:00:00Z | grok-4-1-fast-reasoning | 10.0s | success | no | — | 1000 |
| 2026-02-24T10:01:00Z | grok-4-1-fast-reasoning | 15.0s | success | no | — | 1000 |
| 2026-02-24T10:02:00Z | deepseek-ai/DeepSeek-V3.1 | 20.0s | success | no | — | 1000 |";

    let (store, _dir) = store_with_events(events);

    // Build normalization map: model_id → config_key
    let mut id_to_key = HashMap::new();
    id_to_key.insert("grok-4-1-fast-reasoning".to_string(), "grok".to_string());
    id_to_key.insert(
        "deepseek-ai/DeepSeek-V3.1".to_string(),
        "deepseek-v3.1".to_string(),
    );

    let stats = store
        .get_model_stats(Some(&id_to_key))
        .await
        .expect("should have stats");

    // Should be keyed by config key, not model_id
    assert!(
        stats.contains_key("grok"),
        "Should have 'grok' key, got: {:?}",
        stats.keys().collect::<Vec<_>>()
    );
    assert!(
        stats.contains_key("deepseek-v3.1"),
        "Should have 'deepseek-v3.1' key"
    );
    assert!(
        !stats.contains_key("grok-4-1-fast-reasoning"),
        "Should NOT have model_id key"
    );
    assert!(
        !stats.contains_key("deepseek-ai/DeepSeek-V3.1"),
        "Should NOT have model_id key"
    );

    let grok = stats.get("grok").unwrap();
    assert_eq!(grok.sample_count, 2);
}

// Bug 1: Without normalization map, raw names are preserved (backward compat).
#[tokio::test]
async fn normalization_none_uses_raw_names() {
    let events = "\
| 2026-02-24T10:00:00Z | grok-4-1-fast-reasoning | 10.0s | success | no | — | 1000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    assert!(
        stats.contains_key("grok-4-1-fast-reasoning"),
        "Without map, raw names preserved"
    );
}

// Bug 2: Auth failures are excluded from success rate calculation.
#[tokio::test]
async fn auth_failures_excluded_from_success_rate() {
    // New format events (10 elements after split): includes reason column
    let events = "\
| 2026-02-24T10:00:00Z | bad-model | 10.0s | success | no | — | — | 1000 |
| 2026-02-24T10:01:00Z | bad-model | 10.0s | success | no | — | — | 1000 |
| 2026-02-24T10:02:00Z | bad-model | 10.0s | success | no | — | — | 1000 |
| 2026-02-24T10:03:00Z | bad-model | 10.0s | success | no | — | — | 1000 |
| 2026-02-24T10:04:00Z | bad-model | 10.0s | success | no | — | — | 1000 |
| 2026-02-24T10:05:00Z | bad-model | 0.5s | error | no | auth_failed | 401 Unauthorized | 1000 |
| 2026-02-24T10:06:00Z | bad-model | 0.5s | error | no | auth_failed | 401 Unauthorized | 1000 |
| 2026-02-24T10:07:00Z | bad-model | 0.5s | error | no | auth_failed | 401 Unauthorized | 1000 |
| 2026-02-24T10:08:00Z | bad-model | 0.5s | error | no | auth_failed | 401 Unauthorized | 1000 |
| 2026-02-24T10:09:00Z | bad-model | 0.5s | error | no | auth_failed | 401 Unauthorized | 1000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let bad = stats.get("bad-model").unwrap();
    // 5 successes + 5 auth failures. Auth failures excluded from denominator.
    // Success rate should be 5/5 = 100%, NOT 5/10 = 50%.
    assert_eq!(bad.sample_count, 5, "Only quality events count as samples");
    assert!(
        (bad.success_rate - 1.0).abs() < 0.01,
        "5/5 = 100%, got {:.2}%",
        bad.success_rate * 100.0
    );
    assert_eq!(
        bad.infrastructure_failures, 5,
        "Should track 5 infra failures"
    );
}

// Bug 2: Old format events (without reason column) still parse correctly.
#[tokio::test]
async fn old_format_events_parse_without_reason() {
    // Old format: 7 columns (9 elements after split), no reason column
    let events = "\
| 2026-02-24T10:00:00Z | old-model | 10.0s | success | no | — | 1000 |
| 2026-02-24T10:01:00Z | old-model | 15.0s | error | no | timeout | 1000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let old = stats.get("old-model").unwrap();
    assert_eq!(
        old.sample_count, 2,
        "Both old-format events should be quality events"
    );
    assert!((old.success_rate - 0.5).abs() < 0.01, "1/2 = 50%");
    assert_eq!(
        old.infrastructure_failures, 0,
        "No infra failures in old format"
    );
}

// Bug 3: Display rounding shows one decimal place (69.9%, not 70%).
#[tokio::test]
async fn display_rounding_shows_one_decimal() {
    // Create events: 699 successes, 301 failures → 69.9% success rate (below 70% threshold)
    // Use new format with reason column
    let mut event_lines = Vec::new();
    for i in 0..699 {
        event_lines.push(format!(
            "| 2026-02-24T10:{:02}:{:02}Z | borderline | 10.0s | success | no | — | — | 1000 |",
            i / 60,
            i % 60
        ));
    }
    for i in 0..301 {
        event_lines.push(format!(
            "| 2026-02-24T11:{:02}:{:02}Z | borderline | 10.0s | error | no | error | fail | 1000 |",
            i / 60, i % 60
        ));
    }
    let events = event_lines.join("\n");

    let (store, _dir) = store_with_events(&events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let b = stats.get("borderline").unwrap();
    // Verify the model IS below threshold
    assert!(
        b.success_rate < squall::review::MIN_SUCCESS_RATE,
        "69.9% ({:.3}) should be below {:.1}% threshold",
        b.success_rate,
        squall::review::MIN_SUCCESS_RATE * 100.0
    );

    // Verify display format uses 1 decimal
    let display = format!("{:.1}%", b.success_rate * 100.0);
    assert_eq!(display, "69.9%", "Should show 69.9%, not 70%");
}

// ==========================================================================
// Diagnostic gate stats: timeout/cutoff/partial tracking (plan v4)
// ==========================================================================

// Diagnostic: timeout_count and cutoff_count are tracked independently.
#[tokio::test]
async fn gate_stats_track_timeout_and_cutoff_counts() {
    // New format: 3 timeouts + 2 cutoffs + 5 successes = 10 quality events
    let events = "\
| 2026-02-25T10:00:00Z | test-model | 10.0s | error | no | timeout | timed out | 5000 |
| 2026-02-25T10:01:00Z | test-model | 10.0s | error | no | timeout | timed out | 5000 |
| 2026-02-25T10:02:00Z | test-model | 10.0s | error | no | timeout | timed out | 5000 |
| 2026-02-25T10:03:00Z | test-model | 10.0s | error | no | cutoff | straggler | 5000 |
| 2026-02-25T10:04:00Z | test-model | 10.0s | error | no | cutoff | straggler | 5000 |
| 2026-02-25T10:05:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:06:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:07:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:08:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:09:00Z | test-model | 5.0s | success | no | — | — | 2000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let s = stats.get("test-model").unwrap();
    assert_eq!(s.timeout_count, 3, "3 timeout events");
    assert_eq!(s.cutoff_count, 2, "2 cutoff events");
    assert_eq!(s.sample_count, 10, "all 10 are quality events (not infra)");
    assert!(
        (s.success_rate - 0.5).abs() < 0.01,
        "5/10 = 50%, got {:.1}%",
        s.success_rate * 100.0
    );
}

// Diagnostic: partial_count tracks partial responses separately from success_rate.
#[tokio::test]
async fn gate_stats_track_partial_count() {
    // 3 full successes + 2 partial successes = 5 quality events
    let events = "\
| 2026-02-25T10:00:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:01:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:02:00Z | test-model | 5.0s | success | no | — | — | 2000 |
| 2026-02-25T10:03:00Z | test-model | 10.0s | success | yes | partial | — | 3000 |
| 2026-02-25T10:04:00Z | test-model | 10.0s | success | yes | partial | — | 3000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let s = stats.get("test-model").unwrap();
    assert_eq!(s.partial_count, 2, "2 partial responses");
    assert_eq!(s.sample_count, 5, "all 5 are quality events");
    // Partials are NOT counted as successes: 3/5 = 60%
    assert!(
        (s.success_rate - 0.6).abs() < 0.01,
        "3 full successes / 5 = 60%, got {:.1}%",
        s.success_rate * 100.0
    );
}

// Diagnostic: avg_failed_prompt_len only averages over timeout/cutoff events.
#[tokio::test]
async fn gate_stats_avg_failed_prompt_len() {
    // 2 timeouts (prompt 10000, 20000) + 3 successes (prompt 5000)
    let events = "\
| 2026-02-25T10:00:00Z | test-model | 10.0s | error | no | timeout | timed out | 10000 |
| 2026-02-25T10:01:00Z | test-model | 10.0s | error | no | timeout | timed out | 20000 |
| 2026-02-25T10:02:00Z | test-model | 5.0s | success | no | — | — | 5000 |
| 2026-02-25T10:03:00Z | test-model | 5.0s | success | no | — | — | 5000 |
| 2026-02-25T10:04:00Z | test-model | 5.0s | success | no | — | — | 5000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let s = stats.get("test-model").unwrap();
    // Only timeout/cutoff events: (10000 + 20000) / 2 = 15000
    assert_eq!(
        s.avg_failed_prompt_len, 15000,
        "avg of timeout prompts only: (10000+20000)/2"
    );
    // Success events' prompt_len should NOT be included
    assert_eq!(s.timeout_count, 2);
}

// Diagnostic: old format (9 cols) parses prompt_len from correct column.
#[tokio::test]
async fn gate_stats_old_format_prompt_len() {
    // Old format: 7 pipe-delimited fields → 9 elements after split (leading/trailing empty)
    // No reason column: | ts | model | lat | status | partial | error | prompt_len |
    let events = "\
| 2026-02-25T10:00:00Z | old-model | 10.0s | error | no | timeout | 8000 |
| 2026-02-25T10:01:00Z | old-model | 10.0s | error | no | timeout | 12000 |
| 2026-02-25T10:02:00Z | old-model | 5.0s | success | no | — | 3000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let s = stats.get("old-model").unwrap();
    // Old format: reason is inferred as "—" (not "timeout"), so timeouts from
    // old format events are NOT detected (no reason column).
    // Old format has 9 cols, which is < 10, so reason defaults to "—".
    // The "timeout" text in cols[6] is the error field, not reason.
    assert_eq!(
        s.timeout_count, 0,
        "old format has no reason column — timeouts not detected"
    );
    assert_eq!(
        s.avg_failed_prompt_len, 0,
        "no timeout/cutoff detected → no failed prompt avg"
    );
    assert_eq!(s.sample_count, 3, "all 3 are quality events");
}

// ==========================================================================
// Exploration slot tests
// ==========================================================================

// Exploration slot: re-adds a gated model only when >50% of failures are timeouts.
#[tokio::test]
async fn exploration_slot_only_for_timeout_dominant() {
    // timeout-model: 3/8 success, 5/8 errors — 4 timeout + 1 error
    //   failures=5, timing=4, 4*2=8 > 5 → >50% timing → gets exploration slot
    // error-model: 3/8 success, 5/8 errors — 0 timeout, 5 errors
    //   failures=5, timing=0, 0*2=0 < 5 → NOT timeout-dominant → no slot
    // good-model: 9/10 success → passes gate normally
    let mut events = String::new();
    // timeout-model: 3 successes + 4 timeouts + 1 plain error
    for i in 0..3 {
        events.push_str(&format!(
            "| 2026-02-25T10:{i:02}:00Z | timeout-model | 5.0s | success | no | — | — | 2000 |\n"
        ));
    }
    for i in 3..7 {
        events.push_str(&format!(
            "| 2026-02-25T10:{i:02}:00Z | timeout-model | 10.0s | error | no | timeout | timed out | 15000 |\n"
        ));
    }
    events.push_str(
        "| 2026-02-25T10:07:00Z | timeout-model | 10.0s | error | no | error | failed | 2000 |\n",
    );

    // error-model: 3 successes + 5 plain errors (no timeouts)
    for i in 0..3 {
        events.push_str(&format!(
            "| 2026-02-25T11:{i:02}:00Z | error-model | 5.0s | success | no | — | — | 2000 |\n"
        ));
    }
    for i in 3..8 {
        events.push_str(&format!(
            "| 2026-02-25T11:{i:02}:00Z | error-model | 10.0s | error | no | error | failed | 2000 |\n"
        ));
    }

    // good-model: 9 successes + 1 error
    for i in 0..9 {
        events.push_str(&format!(
            "| 2026-02-25T12:{i:02}:00Z | good-model | 5.0s | success | no | — | — | 2000 |\n"
        ));
    }
    events.push_str(
        "| 2026-02-25T12:09:00Z | good-model | 10.0s | error | no | error | fail | 2000 |\n",
    );

    let (store, _dir) = store_with_events(&events);
    let registry = test_registry(&["timeout-model", "error-model", "good-model"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["timeout-model", "error-model", "good-model"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    // good-model: passes gate (90% success)
    assert!(
        dispatched.contains(&"good-model"),
        "good-model should pass gate. Got: {dispatched:?}"
    );
    // timeout-model: gated (37.5%) but >50% timeouts → exploration slot
    assert!(
        dispatched.contains(&"timeout-model"),
        "timeout-model should get exploration slot (4/5 failures are timeouts). Got: {dispatched:?}"
    );
    // error-model: gated (37.5%) and 0% timeouts → NO exploration slot
    assert!(
        !dispatched.contains(&"error-model"),
        "error-model should stay gated (0 timeouts). Got: {dispatched:?}"
    );

    // Check exploration slot warning
    let explore_warning = resp
        .warnings
        .iter()
        .find(|w| w.contains("Exploration slot"));
    assert!(
        explore_warning.is_some(),
        "should have exploration slot warning. Warnings: {:?}",
        resp.warnings
    );
    assert!(
        explore_warning.unwrap().contains("timeout-model"),
        "exploration slot should name timeout-model"
    );
}

// Exploration slot: not applied when all models are gated (fallback already restores all).
#[tokio::test]
async fn exploration_slot_skipped_when_all_gated() {
    // Both models have terrible success rates — all will be gated
    let mut events = String::new();
    for i in 0..2 {
        events.push_str(&format!(
            "| 2026-02-25T10:{i:02}:00Z | model-a | 5.0s | success | no | — | — | 2000 |\n"
        ));
    }
    for i in 2..8 {
        events.push_str(&format!(
            "| 2026-02-25T10:{i:02}:00Z | model-a | 10.0s | error | no | timeout | timed out | 15000 |\n"
        ));
    }
    for i in 0..2 {
        events.push_str(&format!(
            "| 2026-02-25T11:{i:02}:00Z | model-b | 5.0s | success | no | — | — | 2000 |\n"
        ));
    }
    for i in 2..8 {
        events.push_str(&format!(
            "| 2026-02-25T11:{i:02}:00Z | model-b | 10.0s | error | no | timeout | timed out | 15000 |\n"
        ));
    }

    let (store, _dir) = store_with_events(&events);
    let registry = test_registry(&["model-a", "model-b"]);
    let executor = ReviewExecutor::new(registry);
    let req = make_request(vec!["model-a", "model-b"]);

    let resp = executor
        .execute(&req, req.prompt.clone(), &store, None, None, None, None)
        .await;

    // Both gated (25% success) → fallback restores both → no exploration slot
    let dispatched: Vec<&str> = resp.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(
        dispatched.len(),
        2,
        "all-gated fallback should restore both. Got: {dispatched:?}"
    );

    // Should NOT have exploration slot warning (fallback handles it)
    let explore_warning = resp.warnings.iter().any(|w| w.contains("Exploration slot"));
    assert!(
        !explore_warning,
        "no exploration slot when all models gated (fallback used). Warnings: {:?}",
        resp.warnings
    );

    // Should have the fallback warning
    let has_fallback = resp
        .warnings
        .iter()
        .any(|w| w.contains("All requested models below"));
    assert!(has_fallback, "should have fallback warning");
}

// Bug 5: Partial results are not counted as full successes.
#[tokio::test]
async fn partial_results_not_counted_as_success() {
    // New format events: 5 partials, 0 full successes
    let events = "\
| 2026-02-24T10:00:00Z | partial-model | 10.0s | success | yes | partial | — | 1000 |
| 2026-02-24T10:01:00Z | partial-model | 10.0s | success | yes | partial | — | 1000 |
| 2026-02-24T10:02:00Z | partial-model | 10.0s | success | yes | partial | — | 1000 |
| 2026-02-24T10:03:00Z | partial-model | 10.0s | success | yes | partial | — | 1000 |
| 2026-02-24T10:04:00Z | partial-model | 10.0s | success | yes | partial | — | 1000 |";

    let (store, _dir) = store_with_events(events);
    let stats = store
        .get_model_stats(None)
        .await
        .expect("should have stats");

    let p = stats.get("partial-model").unwrap();
    assert_eq!(p.sample_count, 5, "All 5 are quality events");
    assert!(
        (p.success_rate - 0.0).abs() < 0.01,
        "0 full successes / 5 = 0%, got {:.1}%",
        p.success_rate * 100.0
    );
}
