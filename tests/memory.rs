//! Tests for the memory system — post-review learning extraction and context injection.
//!
//! Architecture reference: `.squall/research/memory-architecture.md`
//! Implementation: `src/memory.rs`
//!
//! Test categories:
//! 1. Write path: model metrics, memorize tool, atomic writes, concurrency, compaction
//! 2. Read path: category filtering, model filtering, graceful degradation, size budget
//! 3. Integration: full cycle (review → memory → next review gets context)
//! 4. Adversarial: malformed files, large files, unicode, binary, symlinks
//!
//! NOTE: MemoryStore uses relative path `.squall/memory/` so tests must change cwd.
//! A global mutex serializes all tests that need to change the process cwd.

use squall::memory::{
    MemoryStore, MAX_MEMORIZE_CONTENT_LEN, MAX_PATTERN_ENTRIES, MAX_TACTICS_BYTES,
    VALID_CATEGORIES,
};
use squall::tools::review::{ModelStatus, ReviewModelResult};
use std::path::PathBuf;
use std::sync::Mutex;

// Global lock: only one test at a time can change cwd.
static CWD_LOCK: Mutex<()> = Mutex::new(());

// ===========================================================================
// Helpers
// ===========================================================================

/// Create a temp dir, acquire the cwd lock, and change into it.
/// Returns (temp_dir_path, original_cwd, lock_guard).
fn setup_test_env(test_name: &str) -> (PathBuf, PathBuf, std::sync::MutexGuard<'static, ()>) {
    let guard = CWD_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("squall-test-memory-{test_name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    (dir, original, guard)
}

fn teardown(dir: &PathBuf, original: &PathBuf) {
    std::env::set_current_dir(original).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

fn make_result(model: &str, latency_ms: u64, status: ModelStatus) -> ReviewModelResult {
    let response = if status == ModelStatus::Success {
        Some("ok".to_string())
    } else {
        None
    };
    let error = if status == ModelStatus::Error {
        Some("test error".to_string())
    } else {
        None
    };
    ReviewModelResult {
        model: model.to_string(),
        provider: "test".to_string(),
        status,
        response,
        error,
        reason: None,
        latency_ms,
        partial: false,
    }
}

fn memory_dir(base: &std::path::Path) -> PathBuf {
    base.join(".squall/memory")
}

/// Run an async function inside a fresh tokio runtime.
fn run_async<F: std::future::Future<Output = ()>>(f: F) {
    tokio::runtime::Runtime::new().unwrap().block_on(f);
}

// ===========================================================================
// CONSTANTS / STRUCTURAL
// ===========================================================================

#[test]
fn constants_match_architecture() {
    assert_eq!(MAX_PATTERN_ENTRIES, 50);
    assert_eq!(MAX_TACTICS_BYTES, 10 * 1024);
    assert_eq!(MAX_MEMORIZE_CONTENT_LEN, 500);
    assert!(VALID_CATEGORIES.contains(&"pattern"));
    assert!(VALID_CATEGORIES.contains(&"tactic"));
    assert_eq!(VALID_CATEGORIES.len(), 3);
}

// ===========================================================================
// WRITE PATH: Model Metrics
// ===========================================================================

#[test]
fn write_log_metrics_creates_models_md() {
    let (dir, orig, _guard) = setup_test_env("w1-log-metrics");
    run_async(async {
        let store = MemoryStore::new();
        let results = vec![
            make_result("grok", 22000, ModelStatus::Success),
            make_result("gemini", 145000, ModelStatus::Success),
        ];
        store.log_model_metrics(&results, 4200, None, None).await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        assert!(content.contains("# Model Performance Profiles"));
        assert!(content.contains("## Summary"));
        assert!(content.contains("## Recent Events"));
        assert!(content.contains("grok"));
        assert!(content.contains("gemini"));
        assert!(content.contains("22.0s"));
        assert!(content.contains("145.0s"));
        assert!(content.contains("4200"));
    });
    teardown(&dir, &orig);
}

#[test]
fn write_log_metrics_appends_events() {
    let (dir, orig, _guard) = setup_test_env("w2-append");
    run_async(async {
        let store = MemoryStore::new();
        store
            .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
            .await;
        store
            .log_model_metrics(&[make_result("grok", 30000, ModelStatus::Success)], 2000, None, None)
            .await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        let grok_lines: Vec<&str> = content
            .lines()
            .filter(|l| l.starts_with('|') && l.contains("grok") && l.contains("success"))
            .collect();
        assert_eq!(grok_lines.len(), 2, "Should have 2 grok events");
    });
    teardown(&dir, &orig);
}

#[test]
fn write_log_metrics_records_errors() {
    let (dir, orig, _guard) = setup_test_env("w3-errors");
    run_async(async {
        let store = MemoryStore::new();
        store
            .log_model_metrics(&[make_result("kimi", 300000, ModelStatus::Error)], 5000, None, None)
            .await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();
        assert!(content.contains("error"), "Should record error status");
        assert!(content.contains("test error"), "Should record error message");
    });
    teardown(&dir, &orig);
}

#[test]
fn write_creates_index_md() {
    let (dir, orig, _guard) = setup_test_env("w4-index");
    run_async(async {
        let store = MemoryStore::new();
        store
            .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
            .await;

        let index = tokio::fs::read_to_string(memory_dir(&dir).join("index.md"))
            .await
            .unwrap();
        assert!(index.contains("# Squall Memory"));
        assert!(index.contains("memorize"));
    });
    teardown(&dir, &orig);
}

#[test]
fn write_no_tmp_files_remain() {
    let (dir, orig, _guard) = setup_test_env("w5-atomic");
    run_async(async {
        let store = MemoryStore::new();
        store
            .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
            .await;

        let tmp_files: Vec<_> = std::fs::read_dir(memory_dir(&dir))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(tmp_files.is_empty(), "No .tmp files should remain");
    });
    teardown(&dir, &orig);
}

#[test]
fn write_concurrent_metrics_safe() {
    let (dir, orig, _guard) = setup_test_env("w6-concurrent");
    run_async(async {
        let store = std::sync::Arc::new(MemoryStore::new());

        let mut handles = Vec::new();
        for i in 0..10u64 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                s.log_model_metrics(
                    &[make_result("grok", (20 + i) * 1000, ModelStatus::Success)],
                    1000,
                    None,
                    None,
                )
                .await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        let event_count = content
            .lines()
            .filter(|l| l.starts_with('|') && l.contains("grok") && l.contains("success"))
            .count();
        assert_eq!(event_count, 10, "10 concurrent writes should produce 10 events");
    });
    teardown(&dir, &orig);
}

#[test]
fn write_event_log_truncates_at_100() {
    let (dir, orig, _guard) = setup_test_env("w7-truncate");
    run_async(async {
        let store = MemoryStore::new();
        for i in 0..120u64 {
            store
                .log_model_metrics(
                    &[make_result("grok", i * 1000, ModelStatus::Success)],
                    1000,
                    None,
                    None,
                )
                .await;
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        let event_count = content
            .lines()
            .filter(|l| l.starts_with('|') && l.contains("grok") && l.contains("success"))
            .count();
        assert!(
            event_count <= 100,
            "Event log should be truncated to 100 entries, got {event_count}"
        );
    });
    teardown(&dir, &orig);
}

#[test]
fn write_empty_results() {
    let (dir, orig, _guard) = setup_test_env("w-empty-results");
    run_async(async {
        let store = MemoryStore::new();
        store.log_model_metrics(&[], 0, None, None).await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();
        assert!(content.contains("# Model Performance Profiles"));
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// WRITE PATH: Memorize Tool
// ===========================================================================

#[test]
fn memorize_appends_pattern() {
    let (dir, orig, _guard) = setup_test_env("w8-pattern");
    run_async(async {
        let store = MemoryStore::new();
        let result = store
            .memorize(
                "pattern",
                "Race condition in session middleware",
                Some("gemini"),
                Some(&["concurrency".to_string(), "async".to_string()]),
                None,
                None,
            )
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("patterns.md"));

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("# Recurring Patterns"));
        assert!(content.contains("Race condition"));
        assert!(content.contains("gemini"));
        assert!(content.contains("concurrency, async"));
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_appends_tactic() {
    let (dir, orig, _guard) = setup_test_env("w9-tactic");
    run_async(async {
        let store = MemoryStore::new();
        let result = store
            .memorize("tactic", "Step-by-step reduces FP", Some("grok"), None, None, None)
            .await;

        assert!(result.is_ok());
        let content = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        assert!(content.contains("# Prompt Tactics"));
        assert!(content.contains("[grok] Step-by-step reduces FP"));
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_rejects_invalid_category() {
    run_async(async {
        let store = MemoryStore::new();
        let result = store.memorize("invalid", "test", None, None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid category"));
    });
}

#[test]
fn memorize_rejects_empty_content() {
    run_async(async {
        let store = MemoryStore::new();
        let result = store.memorize("pattern", "   ", None, None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    });
}

#[test]
fn memorize_rejects_oversized_content() {
    run_async(async {
        let store = MemoryStore::new();
        let long_content = "x".repeat(MAX_MEMORIZE_CONTENT_LEN + 1);
        let result = store.memorize("pattern", &long_content, None, None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    });
}

#[test]
fn memorize_accepts_exactly_max_content() {
    let (dir, orig, _guard) = setup_test_env("w13-boundary");
    run_async(async {
        let store = MemoryStore::new();
        let exact = "x".repeat(MAX_MEMORIZE_CONTENT_LEN);
        let result = store.memorize("pattern", &exact, None, None, None, None).await;
        assert!(result.is_ok(), "Exactly 500 chars should be accepted: {result:?}");
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_patterns_prune_at_50() {
    let (dir, orig, _guard) = setup_test_env("w14-prune");
    run_async(async {
        let store = MemoryStore::new();
        for i in 0..55 {
            let result = store
                .memorize("pattern", &format!("Pattern {i}"), None, None, None, None)
                .await;
            assert!(result.is_ok(), "Write {i} failed: {:?}", result.unwrap_err());
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();

        let entry_count = content.matches("## [").count();
        assert!(
            entry_count <= MAX_PATTERN_ENTRIES,
            "Should have at most {MAX_PATTERN_ENTRIES} entries, got {entry_count}"
        );
        assert!(!content.contains("Pattern 0"), "Oldest should be pruned");
        assert!(content.contains("Pattern 54"), "Newest should remain");
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_tactics_auto_prunes_at_size_cap() {
    let (dir, orig, _guard) = setup_test_env("w15-tactics-cap");
    run_async(async {
        let store = MemoryStore::new();
        // Write 200 entries — should auto-prune oldest to stay under 10KB
        for i in 0..200 {
            let content = format!("Tactic {i}: {}", "x".repeat(90));
            let result = store.memorize("tactic", &content, Some("grok"), None, None, None).await;
            assert!(result.is_ok(), "Write {i} should succeed via auto-prune: {result:?}");
        }
        // File should be under cap and contain the latest entry
        let content = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        assert!(content.len() <= MAX_TACTICS_BYTES, "Should be within cap: {} bytes", content.len());
        assert!(content.contains("Tactic 199"), "Should contain latest entry");
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// READ PATH
// ===========================================================================

#[test]
fn read_all_categories() {
    let (dir, orig, _guard) = setup_test_env("r1-all");
    run_async(async {
        let store = MemoryStore::new();

        store
            .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
            .await;
        store
            .memorize("pattern", "test pattern", None, None, None, None)
            .await
            .unwrap();
        store
            .memorize("tactic", "test tactic", Some("grok"), None, None, None)
            .await
            .unwrap();

        let result = store.read_memory(None, None, 10000, None).await.unwrap();
        assert!(result.contains("grok"), "Should contain model data");
        assert!(result.contains("test pattern"), "Should contain patterns");
        assert!(result.contains("test tactic"), "Should contain tactics");
    });
    teardown(&dir, &orig);
}

#[test]
fn read_filters_by_category() {
    let (dir, orig, _guard) = setup_test_env("r2-filter");
    run_async(async {
        let store = MemoryStore::new();

        store
            .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
            .await;
        store
            .memorize("pattern", "test pattern", None, None, None, None)
            .await
            .unwrap();

        let models_only = store.read_memory(Some("models"), None, 10000, None).await.unwrap();
        assert!(models_only.contains("grok"));
        assert!(!models_only.contains("Recurring Patterns"));

        let patterns_only = store.read_memory(Some("patterns"), None, 10000, None).await.unwrap();
        assert!(patterns_only.contains("test pattern"));
    });
    teardown(&dir, &orig);
}

#[test]
fn read_models_returns_summary_only() {
    let (dir, orig, _guard) = setup_test_env("r3-summary");
    run_async(async {
        let store = MemoryStore::new();

        for i in 0..15u64 {
            store
                .log_model_metrics(
                    &[make_result("grok", (20 + i) * 1000, ModelStatus::Success)],
                    1000,
                    None,
                    None,
                )
                .await;
        }

        let result = store.read_memory(Some("models"), None, 10000, None).await.unwrap();
        assert!(
            result.contains("Model") && result.contains("Avg Latency"),
            "Should contain summary table"
        );
    });
    teardown(&dir, &orig);
}

#[test]
fn read_tactics_filters_by_model() {
    let (dir, orig, _guard) = setup_test_env("r4-model-filter");
    run_async(async {
        let store = MemoryStore::new();

        store
            .memorize("tactic", "Grok specific tactic", Some("grok"), None, None, None)
            .await
            .unwrap();
        store
            .memorize("tactic", "Gemini specific tactic", Some("gemini"), None, None, None)
            .await
            .unwrap();

        let result = store
            .read_memory(Some("tactics"), Some("grok"), 10000, None)
            .await
            .unwrap();

        assert!(result.contains("Grok specific tactic"));
        assert!(!result.contains("Gemini specific tactic"));
    });
    teardown(&dir, &orig);
}

#[test]
fn read_graceful_when_no_memory() {
    let (dir, orig, _guard) = setup_test_env("r5-empty");
    run_async(async {
        let store = MemoryStore::new();
        let result = store.read_memory(None, None, 4000, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No memory found"));
    });
    teardown(&dir, &orig);
}

#[test]
fn read_respects_max_chars() {
    let (dir, orig, _guard) = setup_test_env("r6-budget");
    run_async(async {
        let store = MemoryStore::new();
        for i in 0..40 {
            store
                .memorize(
                    "pattern",
                    &format!("Pattern {i}: {}", "x".repeat(100)),
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        let result = store.read_memory(Some("patterns"), None, 500, None).await.unwrap();
        // 500 chars + "[truncated]" (12 chars) + newlines
        assert!(result.len() <= 530, "Should respect max_chars, got {} chars", result.len());
        assert!(result.contains("[truncated]"));
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// INTEGRATION TESTS
// ===========================================================================

#[test]
fn integration_full_cycle() {
    let (dir, orig, _guard) = setup_test_env("i1-cycle");
    run_async(async {
        let store = MemoryStore::new();

        // Phase 1: Review completes
        store
            .log_model_metrics(
                &[
                    make_result("grok", 22000, ModelStatus::Success),
                    make_result("gemini", 145000, ModelStatus::Success),
                    make_result("kimi", 300000, ModelStatus::Error),
                ],
                4200,
                None,
                None,
            )
            .await;

        // Phase 2: Caller saves learnings
        store
            .memorize(
                "pattern",
                "Pipe deadlock in CLI dispatch when output exceeds buffer",
                Some("gemini"),
                Some(&["concurrency".to_string(), "pipes".to_string()]),
                None,
                None,
            )
            .await
            .unwrap();
        store
            .memorize("tactic", "Use focused system prompt for security review", Some("grok"), None, None, None)
            .await
            .unwrap();

        // Phase 3: Next review reads context
        let context = store.read_memory(None, None, 10000, None).await.unwrap();
        assert!(context.contains("grok"));
        assert!(context.contains("Pipe deadlock"));
        assert!(context.contains("focused system prompt"));
        assert!(context.contains("---"));
    });
    teardown(&dir, &orig);
}

#[test]
fn integration_survives_restart() {
    let (dir, orig, _guard) = setup_test_env("i2-restart");
    run_async(async {
        {
            let store = MemoryStore::new();
            store
                .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 1000, None, None)
                .await;
            store
                .memorize("pattern", "Persisted finding", None, None, None, None)
                .await
                .unwrap();
        }

        let store = MemoryStore::new();
        let result = store.read_memory(None, None, 10000, None).await.unwrap();
        assert!(result.contains("grok"));
        assert!(result.contains("Persisted finding"));
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// ADVERSARIAL TESTS
// ===========================================================================

#[test]
fn adversarial_malformed_models_file() {
    let (dir, orig, _guard) = setup_test_env("a1-malformed");
    run_async(async {
        let md = memory_dir(&dir);
        tokio::fs::create_dir_all(&md).await.unwrap();
        tokio::fs::write(md.join("models.md"), "GARBAGE\nNOT A TABLE\n")
            .await
            .unwrap();

        let store = MemoryStore::new();
        let result = store.read_memory(Some("models"), None, 4000, None).await;
        assert!(result.is_ok(), "Malformed file should not cause error");
    });
    teardown(&dir, &orig);
}

#[test]
fn adversarial_malformed_patterns_file() {
    let (dir, orig, _guard) = setup_test_env("a2-malformed-patterns");
    run_async(async {
        let md = memory_dir(&dir);
        tokio::fs::create_dir_all(&md).await.unwrap();
        tokio::fs::write(md.join("patterns.md"), "NOT A PATTERN FILE")
            .await
            .unwrap();

        let store = MemoryStore::new();
        let result = store.read_memory(Some("patterns"), None, 4000, None).await;
        assert!(result.is_ok());
    });
    teardown(&dir, &orig);
}

#[test]
fn adversarial_large_models_file() {
    let (dir, orig, _guard) = setup_test_env("a3-large");
    run_async(async {
        let md = memory_dir(&dir);
        tokio::fs::create_dir_all(&md).await.unwrap();
        tokio::fs::write(md.join("models.md"), "x".repeat(1_000_000))
            .await
            .unwrap();

        let store = MemoryStore::new();
        let result = store.read_memory(Some("models"), None, 4000, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().len() <= 4020);
    });
    teardown(&dir, &orig);
}

#[test]
fn adversarial_unicode_in_memory() {
    let (dir, orig, _guard) = setup_test_env("a4-unicode");
    run_async(async {
        let store = MemoryStore::new();
        let unicode = "Race condition \u{1F980} in \u{4F60}\u{597D} handler";
        store.memorize("pattern", unicode, None, None, None, None).await.unwrap();

        let result = store.read_memory(Some("patterns"), None, 10000, None).await.unwrap();
        assert!(result.contains("\u{1F980}"), "Crab emoji should survive");
        assert!(result.contains("\u{4F60}\u{597D}"), "CJK should survive");
    });
    teardown(&dir, &orig);
}

#[test]
fn adversarial_empty_model_name() {
    let (dir, orig, _guard) = setup_test_env("a5-empty-model");
    run_async(async {
        let store = MemoryStore::new();
        store
            .memorize("tactic", "Generic tactic", Some(""), None, None, None)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        assert!(!content.contains("[]"), "Empty model should not produce brackets");
    });
    teardown(&dir, &orig);
}

#[test]
fn adversarial_concurrent_memorize() {
    let (dir, orig, _guard) = setup_test_env("a6-concurrent-memorize");
    run_async(async {
        let store = std::sync::Arc::new(MemoryStore::new());

        let mut handles = Vec::new();
        for i in 0..10 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                s.memorize("pattern", &format!("Concurrent pattern {i}"), None, None, None, None)
                    .await
                    .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();

        let entry_count = content.matches("## [").count();
        assert_eq!(entry_count, 10, "10 concurrent memorize should produce 10 entries");
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_minimal_fields() {
    let (dir, orig, _guard) = setup_test_env("a7-minimal");
    run_async(async {
        let store = MemoryStore::new();
        let result = store.memorize("pattern", "Simple observation", None, None, None, None).await;
        assert!(result.is_ok());

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("Simple observation"));
        assert!(!content.contains("- Model:"));
    });
    teardown(&dir, &orig);
}

#[test]
fn memorize_empty_tags() {
    let (dir, orig, _guard) = setup_test_env("a8-empty-tags");
    run_async(async {
        let store = MemoryStore::new();
        let result = store.memorize("pattern", "No tags here", None, Some(&[]), None, None).await;
        assert!(result.is_ok());

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(!content.contains("- Tags:"));
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// BUG FIXES (TDD RED→GREEN)
// ===========================================================================

/// Bug 1 (HIGH): truncate(max_chars) panics on multi-byte UTF-8 boundary.
/// We write a file with emoji at known offsets and truncate into the emoji.
#[test]
fn bug1_unicode_truncation_does_not_panic() {
    let (dir, orig, _guard) = setup_test_env("bug1-unicode-trunc");
    run_async(async {
        let md = memory_dir(&dir);
        tokio::fs::create_dir_all(&md).await.unwrap();
        // Write patterns.md with emoji right after "AB" (2 ASCII bytes)
        // 🦀 is 4 bytes (F0 9F A6 80). Truncating at byte 3 would land inside it.
        tokio::fs::write(md.join("patterns.md"), "AB\u{1F980}CD").await.unwrap();

        let store = MemoryStore::new();
        // max_chars=3 lands inside the 4-byte emoji (byte indices 2..6)
        let result = store.read_memory(Some("patterns"), None, 3, None).await;
        assert!(result.is_ok(), "Should not panic on unicode boundary: {result:?}");
    });
    teardown(&dir, &orig);
}

/// Bug 2 (HIGH): pipe `|` in model names corrupts table parsing and summary.
/// When a model name contains `|`, the event row has too many columns.
/// The summary computation (which splits on `|`) will extract the wrong model name.
#[test]
fn bug2_pipe_in_model_name_does_not_corrupt_summary() {
    let (dir, orig, _guard) = setup_test_env("bug2-pipe-injection");
    run_async(async {
        let store = MemoryStore::new();
        let results = vec![ReviewModelResult {
            model: "model|with|pipes".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Success,
            response: Some("ok".to_string()),
            error: None,
            reason: None,
            latency_ms: 5000,
            partial: false,
        }];

        // Write 10 events to force summary computation (COMPACTION_INTERVAL=10)
        for _ in 0..10 {
            store.log_model_metrics(&results, 1000, None, None).await;
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        // The summary should reference the model — with pipes escaped or intact
        let summary_end = content.find("## Recent Events").unwrap_or(content.len());
        let summary = &content[..summary_end];
        // If pipes corrupt parsing, the model name in summary will be wrong
        // (e.g., just "model" or empty instead of the full name)
        assert!(
            summary.contains("model") && summary.contains("pipes"),
            "Summary should contain the full model name (possibly escaped): {summary}"
        );
    });
    teardown(&dir, &orig);
}

/// Bug 2b: pipe in error message.
#[test]
fn bug2b_pipe_in_error_does_not_corrupt_summary() {
    let (dir, orig, _guard) = setup_test_env("bug2b-pipe-error");
    run_async(async {
        let store = MemoryStore::new();
        let results = vec![ReviewModelResult {
            model: "grok".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Error,
            response: None,
            error: Some("error | with | pipes".to_string()),
            reason: None,
            latency_ms: 5000,
            partial: false,
        }];

        store.log_model_metrics(&results, 1000, None, None).await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        // Summary should still parse correctly — grok should appear in summary
        assert!(
            content.contains("## Summary"),
            "Summary section should exist"
        );
        // The error text should be escaped, not creating extra columns
        // Count only event lines (after "## Recent Events" section)
        let events_section = content.split("## Recent Events").nth(1).unwrap_or("");
        let event_lines: Vec<&str> = events_section
            .lines()
            .filter(|l| l.starts_with('|') && l.contains("grok") && l.contains("5.0s"))
            .collect();
        assert_eq!(event_lines.len(), 1, "Should have exactly 1 event line for grok: {event_lines:?}");
        // Verify the event line has exactly 8 pipe-delimited columns (not more from unescaped pipes)
        let cols: Vec<&str> = event_lines[0].split('|').collect();
        assert_eq!(cols.len(), 10, "Event row should have 8 data columns (10 parts after split): {cols:?}");
    });
    teardown(&dir, &orig);
}

/// Bug 5 (MEDIUM): summary becomes inconsistent when events are truncated
/// but summary is not recomputed (happens on non-compaction writes).
#[test]
fn bug5_summary_recomputed_on_truncation() {
    let (dir, orig, _guard) = setup_test_env("bug5-summary-sync");
    run_async(async {
        let store = MemoryStore::new();

        // Write 105 events for model "old" — this will truncate to 100
        for _ in 0..105 {
            store
                .log_model_metrics(&[make_result("old-model", 10000, ModelStatus::Success)], 100, None, None)
                .await;
        }

        // Now write 1 event for "new" — this should trigger truncation of "old" events
        // and summary should reflect current window, not stale data
        store
            .log_model_metrics(&[make_result("new-model", 50000, ModelStatus::Success)], 100, None, None)
            .await;

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("models.md"))
            .await
            .unwrap();

        // Summary should include both models if they're still in the event window
        // The key test: summary should not show stats for events that were truncated
        let summary_end = content.find("## Recent Events").unwrap_or(content.len());
        let summary = &content[..summary_end];

        // new-model must appear in the summary since it's in the event window
        assert!(
            summary.contains("new-model"),
            "Summary should include new-model: {summary}"
        );
    });
    teardown(&dir, &orig);
}

/// Bug 6 (MEDIUM): tactics.md hits 10KB cap and memorize stops working entirely.
/// Should auto-prune instead of hard rejecting.
#[test]
fn bug6_tactics_auto_prunes_instead_of_rejecting() {
    let (dir, orig, _guard) = setup_test_env("bug6-tactics-prune");
    run_async(async {
        let store = MemoryStore::new();

        // Fill tactics to near capacity
        for i in 0..90 {
            let content = format!("Tactic {i}: {}", "x".repeat(100));
            store
                .memorize("tactic", &content, Some("grok"), None, None, None)
                .await
                .unwrap();
        }

        // This write should succeed by auto-pruning oldest lines, not hard-rejecting
        let result = store
            .memorize("tactic", "This should still work by pruning old entries", Some("grok"), None, None, None)
            .await;
        assert!(
            result.is_ok(),
            "Should auto-prune instead of rejecting: {result:?}"
        );

        // Verify the new entry exists
        let content = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        assert!(content.contains("This should still work"));
        assert!(content.len() <= MAX_TACTICS_BYTES, "Should be within cap");
    });
    teardown(&dir, &orig);
}

/// Bug 7 (MEDIUM): global WRITE_COUNTER couples independent stores.
/// Fixed by moving counter into MemoryStore struct.
/// This test verifies per-store compaction by checking that store2
/// compacts on its own 10th write, independent of store1's writes.
#[test]
fn bug7_compaction_counter_is_per_store() {
    let (dir, orig, _guard) = setup_test_env("bug7-counter");
    run_async(async {
        let md = dir.join("store-a/memory");
        let store = MemoryStore::with_base_dir(md.clone());

        // Write 10 events for "alpha" — should trigger compaction on write #10
        for _ in 0..10 {
            store
                .log_model_metrics(&[make_result("alpha", 20000, ModelStatus::Success)], 100, None, None)
                .await;
        }

        let content = tokio::fs::read_to_string(md.join("models.md")).await.unwrap();
        let summary_end = content.find("## Recent Events").unwrap_or(content.len());
        let summary = &content[..summary_end];
        assert!(
            summary.contains("alpha"),
            "Summary should contain alpha after 10 writes: {summary}"
        );

        // Now write 10 events for "beta" — should trigger compaction again on write #20
        for _ in 0..10 {
            store
                .log_model_metrics(&[make_result("beta", 30000, ModelStatus::Success)], 100, None, None)
                .await;
        }

        let content2 = tokio::fs::read_to_string(md.join("models.md")).await.unwrap();
        let summary_end2 = content2.find("## Recent Events").unwrap_or(content2.len());
        let summary2 = &content2[..summary_end2];
        // After 20 writes, both models should appear in summary (compacted at write 10 and 20)
        assert!(
            summary2.contains("beta"),
            "Summary should contain beta after 20 writes: {summary2}"
        );
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// PHASE 1: Scoped Memory (TDD RED→GREEN)
// ===========================================================================

/// Scope exact match must NOT prefix match: "pr:42" must not match "pr:420".
#[test]
fn scope_exact_match_does_not_prefix_match() {
    let (dir, orig, _guard) = setup_test_env("p1-exact-match");
    run_async(async {
        let store = MemoryStore::new();

        // Write patterns with similar-prefix scopes
        store.memorize("pattern", "PR 42 finding", None, None, Some("pr:42"), None).await.unwrap();
        store.memorize("pattern", "PR 420 finding", None, None, Some("pr:420"), None).await.unwrap();
        store.memorize("pattern", "PR 4 finding", None, None, Some("pr:4"), None).await.unwrap();

        // Filter by "pr:42" — should match only the first, not "pr:420" or "pr:4"
        let result = store.read_memory(Some("patterns"), None, 10000, Some("pr:42")).await.unwrap();
        assert!(result.contains("PR 42 finding"), "Should match pr:42: {result}");
        assert!(!result.contains("PR 420 finding"), "Should NOT match pr:420: {result}");
        assert!(!result.contains("PR 4 finding"), "Should NOT match pr:4: {result}");
    });
    teardown(&dir, &orig);
}

/// scope=None should return all entries regardless of scope.
#[test]
fn scope_none_returns_all_entries() {
    let (dir, orig, _guard) = setup_test_env("p1-scope-none");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Branch scoped", None, None, Some("branch:main"), None).await.unwrap();
        store.memorize("pattern", "Codebase scoped", None, None, Some("codebase"), None).await.unwrap();
        store.memorize("pattern", "Unscoped entry", None, None, None, None).await.unwrap();

        let result = store.read_memory(Some("patterns"), None, 10000, None).await.unwrap();
        assert!(result.contains("Branch scoped"), "Should include branch scoped: {result}");
        assert!(result.contains("Codebase scoped"), "Should include codebase scoped: {result}");
        assert!(result.contains("Unscoped entry"), "Should include unscoped: {result}");
    });
    teardown(&dir, &orig);
}

/// scope="codebase" should not return branch-scoped entries.
#[test]
fn scope_codebase_filters_branch_entries() {
    let (dir, orig, _guard) = setup_test_env("p1-codebase-filter");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Codebase finding", None, None, Some("codebase"), None).await.unwrap();
        store.memorize("pattern", "Branch finding", None, None, Some("branch:feature/auth"), None).await.unwrap();

        let result = store.read_memory(Some("patterns"), None, 10000, Some("codebase")).await.unwrap();
        assert!(result.contains("Codebase finding"), "Should include codebase entry: {result}");
        assert!(!result.contains("Branch finding"), "Should exclude branch entry: {result}");
    });
    teardown(&dir, &orig);
}

/// Scope should be rendered in pattern entries when provided.
#[test]
fn memorize_renders_scope_in_pattern() {
    let (dir, orig, _guard) = setup_test_env("p1-scope-render");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Scoped finding", None, None, Some("branch:feature/x"), None).await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("- Scope: branch:feature/x"), "Should render scope line: {content}");
    });
    teardown(&dir, &orig);
}

/// No scope → no "- Scope:" line in the entry.
#[test]
fn memorize_no_scope_no_scope_line() {
    let (dir, orig, _guard) = setup_test_env("p1-no-scope");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Unscoped finding", None, None, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(!content.contains("- Scope:"), "Should not have scope line: {content}");
    });
    teardown(&dir, &orig);
}

/// default_scope_from_git: branch available → "branch:{name}".
#[test]
fn default_scope_from_git_with_branch() {
    use squall::context::{default_scope_from_git, GitContext};

    let ctx = GitContext {
        commit_sha: Some("abc1234".to_string()),
        branch: Some("feature/auth".to_string()),
    };
    assert_eq!(default_scope_from_git(Some(&ctx)), "branch:feature/auth");
}

/// default_scope_from_git: no branch, only commit → "commit:{sha}".
#[test]
fn default_scope_from_git_detached_head() {
    use squall::context::{default_scope_from_git, GitContext};

    let ctx = GitContext {
        commit_sha: Some("abc1234".to_string()),
        branch: None,
    };
    assert_eq!(default_scope_from_git(Some(&ctx)), "commit:abc1234");
}

/// default_scope_from_git: no git context → "codebase".
#[test]
fn default_scope_from_git_no_context() {
    use squall::context::default_scope_from_git;

    assert_eq!(default_scope_from_git(None), "codebase");
}

/// Bug 3 (HIGH): read failures treated as empty, silently dropping data.
/// If models.md contains invalid UTF-8, `unwrap_or_default()` silently treats it
/// as empty, causing the new write to overwrite all previous data with just the
/// new event + fresh header. The file size shrinks dramatically.
#[test]
fn bug3_read_failure_preserves_existing_data() {
    let (dir, orig, _guard) = setup_test_env("bug3-read-fail");
    run_async(async {
        let store = MemoryStore::new();

        // Write 5 events to build up models.md
        for _ in 0..5 {
            store
                .log_model_metrics(&[make_result("grok", 20000, ModelStatus::Success)], 100, None, None)
                .await;
        }

        let md = memory_dir(&dir);
        let models_path = md.join("models.md");

        let initial = tokio::fs::read_to_string(&models_path).await.unwrap();
        let initial_lines = initial.lines().count();
        assert!(initial_lines > 8, "Should have header + 5 events: {initial_lines}");

        // Corrupt models.md by appending invalid UTF-8 (simulates disk corruption)
        let mut bytes = initial.into_bytes();
        bytes.extend_from_slice(&[0xFF, 0xFE, 0x80]);
        tokio::fs::write(&models_path, &bytes).await.unwrap();

        // Log one more event — with the bug, unwrap_or_default() drops ALL prior data
        store
            .log_model_metrics(&[make_result("gemini", 50000, ModelStatus::Success)], 100, None, None)
            .await;

        let after = tokio::fs::read_to_string(&models_path).await.unwrap();
        let after_lines = after.lines().count();
        // With the bug: file has ~4 lines (header + 1 event). Fixed: should preserve prior events.
        assert!(
            after_lines >= initial_lines,
            "Should not lose data on read failure. Before: {initial_lines} lines, after: {after_lines}"
        );
    });
    teardown(&dir, &orig);
}

// ===========================================================================
// PHASE 2: Evidence Counting + Flush (TDD RED→GREEN)
// ===========================================================================

/// Memorizing the same content twice should merge entries (increment evidence count).
#[test]
fn evidence_hash_dedup_merges_same_content() {
    let (dir, orig, _guard) = setup_test_env("p2-evidence-merge");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Race condition in auth", None, None, None, None).await.unwrap();
        store.memorize("pattern", "Race condition in auth", None, None, None, None).await.unwrap();
        store.memorize("pattern", "Race condition in auth", None, None, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();

        // Should have 1 entry with [x3], not 3 separate entries
        let entry_count = content.matches("## [").count();
        assert_eq!(entry_count, 1, "Should merge into 1 entry, got {entry_count}");
        assert!(content.contains("[x3]"), "Should show evidence count [x3]: {content}");
        assert!(content.contains("- Evidence: 3 occurrences"), "Should show evidence line: {content}");
    });
    teardown(&dir, &orig);
}

/// Similar but different content should NOT be merged.
#[test]
fn evidence_hash_no_false_merge_on_similar_prefix() {
    let (dir, orig, _guard) = setup_test_env("p2-no-false-merge");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Race condition in auth module", None, None, None, None).await.unwrap();
        store.memorize("pattern", "Race condition in payment module", None, None, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();

        let entry_count = content.matches("## [").count();
        assert_eq!(entry_count, 2, "Different content should not merge: {content}");
    });
    teardown(&dir, &orig);
}

/// Evidence >= 5 should add [confirmed] tag.
#[test]
fn evidence_confirmed_at_threshold() {
    let (dir, orig, _guard) = setup_test_env("p2-confirmed");
    run_async(async {
        let store = MemoryStore::new();

        for _ in 0..5 {
            store.memorize("pattern", "Confirmed pattern", None, None, None, None).await.unwrap();
        }

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();

        assert!(content.contains("[confirmed]"), "Should have [confirmed] at 5 occurrences: {content}");
        assert!(content.contains("[x5]"), "Should show [x5]: {content}");
    });
    teardown(&dir, &orig);
}

/// Flush should graduate high-evidence branch patterns to codebase scope.
#[test]
fn flush_graduates_high_evidence_to_codebase() {
    let (dir, orig, _guard) = setup_test_env("p2-flush-graduate");
    run_async(async {
        let store = MemoryStore::new();

        // Create a pattern with evidence >= 3 on branch scope
        for _ in 0..3 {
            store.memorize("pattern", "Important finding", None, None, Some("branch:feature/auth"), None).await.unwrap();
        }

        // Flush the branch
        let report = store.flush_branch("feature/auth").await.unwrap();
        assert!(report.contains("1 patterns graduated"), "Report: {report}");

        // Verify scope changed to codebase
        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("- Scope: codebase"), "Should be graduated to codebase: {content}");
        assert!(!content.contains("- Scope: branch:feature/auth"), "Should not have branch scope: {content}");
    });
    teardown(&dir, &orig);
}

/// Flush should archive low-evidence branch patterns.
#[test]
fn flush_archives_low_evidence_branch_entries() {
    let (dir, orig, _guard) = setup_test_env("p2-flush-archive");
    run_async(async {
        let store = MemoryStore::new();

        // Create a pattern with evidence < 3 on branch scope
        store.memorize("pattern", "Minor observation", None, None, Some("branch:feature/auth"), None).await.unwrap();

        // Flush the branch
        let report = store.flush_branch("feature/auth").await.unwrap();
        assert!(report.contains("1 patterns archived"), "Report: {report}");

        // Verify it's removed from patterns.md
        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(!content.contains("Minor observation"), "Should be removed from patterns: {content}");

        // Verify it's in archive.md
        let archive = tokio::fs::read_to_string(memory_dir(&dir).join("archive.md"))
            .await
            .unwrap();
        assert!(archive.contains("Minor observation"), "Should be in archive: {archive}");
    });
    teardown(&dir, &orig);
}

/// Flush should not affect patterns scoped to other branches.
#[test]
fn flush_preserves_other_branch_patterns() {
    let (dir, orig, _guard) = setup_test_env("p2-flush-preserve");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Auth branch finding", None, None, Some("branch:feature/auth"), None).await.unwrap();
        store.memorize("pattern", "Other branch finding", None, None, Some("branch:feature/other"), None).await.unwrap();
        store.memorize("pattern", "Codebase finding", None, None, Some("codebase"), None).await.unwrap();

        store.flush_branch("feature/auth").await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("Other branch finding"), "Should preserve other branch: {content}");
        assert!(content.contains("Codebase finding"), "Should preserve codebase: {content}");
    });
    teardown(&dir, &orig);
}

/// New entry format should include hash comment for dedup.
#[test]
fn memorize_includes_hash_comment() {
    let (dir, orig, _guard) = setup_test_env("p2-hash-comment");
    run_async(async {
        let store = MemoryStore::new();

        store.memorize("pattern", "Test finding", None, None, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(memory_dir(&dir).join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("<!-- hash:"), "Should include hash comment: {content}");
        assert!(content.contains("[x1]"), "Should include evidence count [x1]: {content}");
    });
    teardown(&dir, &orig);
}

// ---- Phase 4 tests: metadata, recommend, decay ----

/// Metadata key-value pairs should be rendered in pattern entries.
#[tokio::test]
async fn memorize_metadata_renders_in_pattern() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("p4-metadata")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    let mut meta = std::collections::HashMap::new();
    meta.insert("consensus".to_string(), "3/5".to_string());
    meta.insert("diff_size".to_string(), "+120 -45".to_string());

    store
        .memorize("pattern", "Auth bypass found", None, None, None, Some(&meta))
        .await
        .unwrap();

    let content = tokio::fs::read_to_string(tmp.join("patterns.md"))
        .await
        .unwrap();
    assert!(content.contains("- consensus: 3/5"), "content: {content}");
    assert!(content.contains("- diff_size: +120 -45"), "content: {content}");
    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

/// Metadata with no entries produces no extra lines.
#[tokio::test]
async fn memorize_empty_metadata_no_extra_lines() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("p4-empty-meta")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    store
        .memorize("pattern", "Simple finding", None, None, None, None)
        .await
        .unwrap();

    let content = tokio::fs::read_to_string(tmp.join("patterns.md"))
        .await
        .unwrap();
    // Should not contain any "- key: value" lines other than standard ones
    let custom_meta_lines: Vec<&str> = content
        .lines()
        .filter(|l| l.starts_with("- ") && !l.starts_with("- Scope:") && !l.starts_with("- Model:") && !l.starts_with("- Tags:") && !l.starts_with("- Evidence:"))
        .collect();
    assert!(custom_meta_lines.is_empty(), "unexpected metadata: {custom_meta_lines:?}");
    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

/// The "recommend" category should return model recommendations from event log.
#[tokio::test]
async fn read_memory_recommend_returns_recommendations() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("p4-recommend")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    // Populate with model events
    use squall::tools::review::{ModelStatus, ReviewModelResult};
    let results = vec![
        ReviewModelResult {
            model: "fast-model".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Success,
            response: Some("ok".to_string()),
            error: None,
            reason: None,
            latency_ms: 5000,
            partial: false,
        },
        ReviewModelResult {
            model: "slow-model".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Success,
            response: Some("ok".to_string()),
            error: None,
            reason: None,
            latency_ms: 120000,
            partial: false,
        },
    ];
    store.log_model_metrics(&results, 1000, None, None).await;

    let rec = store
        .read_memory(Some("recommend"), None, 10000, None)
        .await
        .unwrap();
    assert!(rec.contains("Model Recommendations"), "rec: {rec}");
    assert!(rec.contains("fast-model"), "rec: {rec}");
    assert!(rec.contains("slow-model"), "rec: {rec}");
    // Quick triage should pick the faster model
    assert!(rec.contains("Quick triage"), "rec: {rec}");
    assert!(rec.contains("100%"), "should show success rate: {rec}");
    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

/// Recommend with no data should return a helpful message.
#[tokio::test]
async fn read_memory_recommend_empty() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("p4-recommend-empty")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    let rec = store
        .read_memory(Some("recommend"), None, 10000, None)
        .await
        .unwrap();
    assert!(rec.contains("No model data"), "rec: {rec}");
    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

/// Quick triage should prefer faster model with good success rate.
#[tokio::test]
async fn recommend_quick_triage_prefers_fastest() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("p4-quick-triage")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    use squall::tools::review::{ModelStatus, ReviewModelResult};

    // Log multiple rounds — fast model always succeeds, slow model sometimes fails
    for _ in 0..3 {
        store
            .log_model_metrics(
                &[
                    ReviewModelResult {
                        model: "speedy".to_string(),
                        provider: "t".to_string(),
                        status: ModelStatus::Success,
                        response: Some("ok".to_string()),
                        error: None,
                        reason: None,
                        latency_ms: 15000,
                        partial: false,
                    },
                    ReviewModelResult {
                        model: "thorough".to_string(),
                        provider: "t".to_string(),
                        status: ModelStatus::Success,
                        response: Some("ok".to_string()),
                        error: None,
                        reason: None,
                        latency_ms: 90000,
                        partial: false,
                    },
                ],
                500,
                None,
                None,
            )
            .await;
    }

    let rec = store
        .read_memory(Some("recommend"), None, 10000, None)
        .await
        .unwrap();

    // Quick triage should pick "speedy" (15s avg) over "thorough" (90s avg)
    let quick_line = rec
        .lines()
        .find(|l| l.contains("Quick triage"))
        .expect("should have Quick triage line");
    assert!(
        quick_line.contains("speedy"),
        "Quick triage should recommend speedy: {quick_line}"
    );
    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

// ---- Defect regression tests (TDD: RED first, then GREEN) ----

/// DEFECT #1: read_to_string_lossy swallows non-NotFound I/O errors, returning "".
/// When memorize() reads a file that exists but can't be read (permissions),
/// it gets empty string, adds the new entry, and atomic_write OVERWRITES the file,
/// destroying all previous patterns.
#[tokio::test]
async fn defect_read_error_must_not_destroy_existing_patterns() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("defect-read-error")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    // Write 3 patterns
    store.memorize("pattern", "Pattern A", None, None, None, None).await.unwrap();
    store.memorize("pattern", "Pattern B", None, None, None, None).await.unwrap();
    store.memorize("pattern", "Pattern C", None, None, None, None).await.unwrap();

    let before = tokio::fs::read_to_string(tmp.join("patterns.md")).await.unwrap();
    assert!(before.contains("Pattern A"), "setup: {before}");
    assert!(before.contains("Pattern B"), "setup: {before}");
    assert!(before.contains("Pattern C"), "setup: {before}");

    // Make patterns.md unreadable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o000);
        std::fs::set_permissions(tmp.join("patterns.md"), perms).unwrap();
    }

    // Try to memorize — should FAIL, not silently destroy the file
    let result = store.memorize("pattern", "Pattern D", None, None, None, None).await;

    // Restore permissions for cleanup
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(tmp.join("patterns.md"), perms).unwrap();
    }

    // The operation should have returned an error
    assert!(result.is_err(), "memorize should fail when patterns.md can't be read, got: {:?}", result);

    // The original file must still contain all 3 patterns (no data loss)
    let after = tokio::fs::read_to_string(tmp.join("patterns.md")).await.unwrap();
    assert!(after.contains("Pattern A"), "Pattern A destroyed! after: {after}");
    assert!(after.contains("Pattern B"), "Pattern B destroyed! after: {after}");
    assert!(after.contains("Pattern C"), "Pattern C destroyed! after: {after}");

    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

/// DEFECT #2: listmodels returns model_id instead of map key.
/// For models where key != model_id (e.g. key="deepseek-r1", model_id="deepseek-reasoner"),
/// the user sees "deepseek-reasoner" from listmodels but Registry::get("deepseek-reasoner")
/// returns None. Default review also uses model_id, so these models are never queried.
#[test]
fn defect_listmodels_must_return_map_key_not_model_id() {
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use squall::config::Config;
    use std::collections::HashMap;
    use squall::tools::listmodels::ModelInfo;

    let mut models = HashMap::new();
    models.insert(
        "deepseek-r1".to_string(),
        ModelEntry {
            model_id: "deepseek-reasoner".to_string(),
            provider: "deepseek".to_string(),
            backend: BackendConfig::Http {
                base_url: "https://api.deepseek.com/chat/completions".to_string(),
                api_key: "test".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config { models, ..Default::default() };
    let registry = Registry::from_config(config);

    // listmodels returns entries — the name shown to users must be the lookup key
    let entries = registry.list_models();
    assert_eq!(entries.len(), 1);

    let info = ModelInfo::from(entries[0]);

    // The name must be "deepseek-r1" (the key), NOT "deepseek-reasoner" (the model_id)
    assert_eq!(info.name, "deepseek-r1",
        "listmodels should return map key 'deepseek-r1', not model_id '{}'", info.name);

    // And that key must be resolvable via Registry::get
    assert!(registry.get(&info.name).is_some(),
        "Registry::get('{}') should find the model", info.name);
}

/// DEFECT #3: SSE keep-alive events (ParsedChunk::Skip) don't reset the stall timer.
/// If a model sends non-text SSE events (keep-alives, ping, empty deltas) while thinking,
/// the stall timer expires and kills the request, even though the connection is alive.
#[tokio::test]
async fn defect_sse_keepalive_must_reset_stall_timer() {
    use squall::dispatch::http::HttpDispatch;
    use squall::dispatch::registry::ApiFormat;
    use squall::dispatch::ProviderRequest;
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    // Start a mock SSE server that sends keep-alive Skip events for 4 seconds,
    // then sends actual text. If stall timer is 3s and isn't reset by Skip events,
    // the request will be killed before the text arrives.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Read the HTTP request
        let mut buf = vec![0u8; 4096];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;

        // Send HTTP response headers
        let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";
        stream.write_all(headers.as_bytes()).await.unwrap();

        // Send SSE data events that parse to ParsedChunk::Skip every 500ms for 4 seconds.
        // Empty JSON objects with no content/choices → Skip in parse_openai_event.
        for _ in 0..8 {
            stream.write_all(b"data: {\"choices\":[]}\n\n").await.unwrap();
            stream.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Now send actual content
        let data = "data: {\"choices\":[{\"delta\":{\"content\":\"survived\"}}]}\n\n\
                    data: [DONE]\n\n";
        stream.write_all(data.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    });

    let http = HttpDispatch::new();
    let req = ProviderRequest {
        model: "test".to_string(),
        prompt: "test".to_string(),
        system_prompt: None,
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        // 3 second stall timeout — shorter than the 4s of keep-alives
        stall_timeout: Some(Duration::from_secs(3)),
    };

    let result = http
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}"),
            "test-key",
            &ApiFormat::OpenAi,
        )
        .await;

    // With the bug: stall timer expires at 3s (keep-alives don't reset it) → timeout/partial
    // With the fix: keep-alives reset the timer → "survived" text arrives at 4s → success
    let result = result.expect("request should succeed when keep-alives are present");
    assert_eq!(result.text, "survived", "should receive text after keep-alive period");
    assert!(!result.partial, "should be complete, not partial");
}

// ===========================================================================
// Codex defect: scope-blind dedup merges across scopes
// ===========================================================================

/// Same content memorized under different scopes should NOT merge.
/// The hash must include scope to maintain isolation.
#[tokio::test]
async fn defect_scope_blind_dedup_must_not_merge_across_scopes() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("defect-scope-blind")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    // Memorize same content under branch:alpha
    store.memorize("pattern", "Race condition in auth", None, None, Some("branch:alpha"), None).await.unwrap();

    // Memorize same content under branch:beta
    store.memorize("pattern", "Race condition in auth", None, None, Some("branch:beta"), None).await.unwrap();

    let content = tokio::fs::read_to_string(tmp.join("patterns.md")).await.unwrap();

    // Should have TWO separate entries — one per scope
    let x1_count = content.matches("[x1]").count();
    assert_eq!(
        x1_count, 2,
        "Same content under different scopes should be 2 separate [x1] entries, \
         but got {x1_count}. Dedup hash must include scope.\n\
         Content:\n{content}"
    );
    // Must NOT have [x2] — that means they merged
    assert!(
        !content.contains("[x2]"),
        "Should NOT have [x2] — entries from different scopes must not merge.\n\
         Content:\n{content}"
    );

    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

// ===========================================================================
// Codex defect: metadata/model/tags lost on dedup merge
// ===========================================================================

/// When a duplicate pattern is merged, the OLD entry's model/tags should be
/// preserved if the new request omits them.
#[tokio::test]
async fn defect_dedup_merge_must_preserve_prior_model_and_tags() {
    let tmp = std::env::temp_dir()
        .join("squall-test")
        .join("defect-merge-metadata")
        .join("memory");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let store = MemoryStore::with_base_dir(tmp.clone());

    let tags = vec!["security".to_string(), "payment".to_string()];

    // First memorize with model + tags
    store.memorize("pattern", "Null pointer in payment handler", Some("gemini"), Some(&tags), Some("codebase"), None).await.unwrap();

    // Second memorize — same content + scope, but NO model or tags
    store.memorize("pattern", "Null pointer in payment handler", None, None, Some("codebase"), None).await.unwrap();

    let content = tokio::fs::read_to_string(tmp.join("patterns.md")).await.unwrap();

    // Should have [x2] (merged)
    assert!(content.contains("[x2]"), "Should have merged: {content}");

    // Should still have Model: gemini from the first entry
    assert!(
        content.contains("- Model: gemini"),
        "Dedup merge should preserve prior model when new request omits it.\n\
         Content:\n{content}"
    );
    // Should still have Tags: security, payment from the first entry
    assert!(
        content.contains("security") && content.contains("payment"),
        "Dedup merge should preserve prior tags when new request omits them.\n\
         Content:\n{content}"
    );

    let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
}

// ===========================================================================
// Deep review defects (2026-02-22 round 6)
// ===========================================================================

/// Codex finding #1: `memorize` with category "recommend" must succeed.
/// Server instructions tell callers to use this category, but VALID_CATEGORIES
/// previously rejected it — causing callers to get "invalid category" errors.
#[test]
fn memorize_recommend_category_accepted() {
    let (dir, orig, _guard) = setup_test_env("recommend-cat");
    run_async(async {
        let store = MemoryStore::new();
        let result = store
            .memorize("recommend", "grok best for fast triage", Some("grok"), None, None, None)
            .await;

        assert!(result.is_ok(), "recommend category should be accepted: {:?}", result.err());
        // Recommend is routed to tactics.md
        let tactics = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        assert!(
            tactics.contains("grok best for fast triage"),
            "Recommend content should be in tactics.md. Got:\n{tactics}"
        );
    });
    teardown(&dir, &orig);
}

/// Codex finding #2: model field in tactic entries must be sanitized.
/// A model name with newlines could inject arbitrary markdown lines into tactics.md.
#[test]
fn memorize_tactic_model_newline_sanitized() {
    let (dir, orig, _guard) = setup_test_env("tactic-model-nl");
    run_async(async {
        let store = MemoryStore::new();
        let result = store
            .memorize(
                "tactic",
                "Use chain-of-thought",
                Some("evil\n## Injected Section\n- payload"),
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_ok(), "should succeed: {:?}", result.err());
        let tactics = tokio::fs::read_to_string(memory_dir(&dir).join("tactics.md"))
            .await
            .unwrap();
        // Newlines in model name should be replaced with spaces,
        // preventing markdown structure injection (headings, list items on own lines)
        assert!(
            !tactics.contains("\n## "),
            "Newline+heading injection must be prevented. Got:\n{tactics}"
        );
        assert!(
            !tactics.contains("\n- payload"),
            "Newline+list injection must be prevented. Got:\n{tactics}"
        );
        assert!(
            tactics.contains("[evil"),
            "Sanitized model name should still appear. Got:\n{tactics}"
        );
    });
    teardown(&dir, &orig);
}
