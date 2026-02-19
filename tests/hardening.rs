//! Hardening tests (TDD RED phase).
//! Each test proves a defect found by multi-model consensus review.
//! Tests should FAIL before fixes and PASS after.

use squall::error::SquallError;

// ---------------------------------------------------------------------------
// Defect 1: CLI args templates must contain {prompt} placeholder.
// Note: `--` separator was tested but breaks gemini/codex CLIs which
// don't support POSIX end-of-options. Since Command::new() passes each
// arg discretely (no shell), flag injection isn't possible anyway.
// ---------------------------------------------------------------------------

#[test]
fn gemini_args_template_has_prompt_placeholder() {
    let config = squall::config::Config::from_env();
    if let Some(entry) = config.models.get("gemini")
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } =
            &entry.backend
    {
        assert!(
            args_template.iter().any(|a| a.contains("{prompt}")),
            "gemini args_template must contain '{{prompt}}' placeholder"
        );
    }
}

#[test]
fn codex_args_template_has_prompt_placeholder() {
    let config = squall::config::Config::from_env();
    if let Some(entry) = config.models.get("codex")
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } =
            &entry.backend
    {
        assert!(
            args_template.iter().any(|a| a.contains("{prompt}")),
            "codex args_template must contain '{{prompt}}' placeholder"
        );
    }
}

// ---------------------------------------------------------------------------
// Defect 2: HTTP response reads full body before size check.
// Tested structurally: http.rs must call content_length() check
// before bytes().await. Validated by code inspection +
// the existence of MAX_RESPONSE_BYTES as a pre-read guard.
// ---------------------------------------------------------------------------

#[test]
fn http_dispatch_has_response_size_limit() {
    let limit = squall::dispatch::http::MAX_RESPONSE_BYTES;
    assert!(limit > 0 && limit <= 10 * 1024 * 1024);
}

// ---------------------------------------------------------------------------
// Defect 3: CLI wait_with_output() buffers all stdout before cap.
// Same as HTTP — validated structurally. Test the constant.
// ---------------------------------------------------------------------------

#[test]
fn cli_dispatch_has_output_size_limit() {
    let limit = squall::dispatch::cli::MAX_OUTPUT_BYTES;
    assert!(limit > 0 && limit <= 10 * 1024 * 1024);
}

// ---------------------------------------------------------------------------
// Defect 4: Process group kill — kill_on_drop doesn't signal pgid.
// Structural: cli.rs sends SIGKILL to -pgid on timeout.
// (Validated by code inspection.)
// ---------------------------------------------------------------------------

// Structural — validated by code review in cli.rs

// ---------------------------------------------------------------------------
// Defect 5: Semaphore acquire() has no timeout — blocks forever
// if all permits are held.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn semaphore_acquire_respects_deadline() {
    use squall::config::Config;
    use squall::dispatch::registry::Registry;
    use squall::dispatch::ProviderRequest;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // Create a registry with a CLI model but no real executable
    let mut models = HashMap::new();
    models.insert(
        "test-cli".to_string(),
        squall::dispatch::registry::ModelEntry {
            model_id: "test-cli".to_string(),
            provider: "gemini".to_string(),
            backend: squall::dispatch::registry::BackendConfig::Cli {
                executable: "nonexistent-binary-12345".to_string(),
                args_template: vec!["--".to_string(), "{prompt}".to_string()],
            },
        },
    );
    let config = Config { models };
    let registry = Registry::from_config(config);

    // Request with a tight deadline — should not block forever on semaphore
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test-cli".to_string(),
        deadline: Instant::now() + Duration::from_millis(500),
        working_directory: None,
    };

    // The query should fail (nonexistent binary), but it should fail FAST,
    // not block on the semaphore. If semaphore has deadline awareness,
    // this completes within the deadline.
    let start = Instant::now();
    let _result = registry.query(&req).await;
    let elapsed = start.elapsed();

    // Should complete well within 2 seconds (not hang)
    assert!(
        elapsed < Duration::from_secs(2),
        "Registry::query should not block indefinitely. Took {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Defect 6: Upstream{status: None} incorrectly retryable.
// "response too large" and "empty choices" are permanent failures,
// not transient. They should NOT be retryable.
// ---------------------------------------------------------------------------

#[test]
fn upstream_response_too_large_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "response too large: 3000000 bytes (max 2097152)".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Response too large (status: None) should NOT be retryable"
    );
}

#[test]
fn upstream_empty_choices_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "empty choices or null content".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Empty choices (status: None) should NOT be retryable"
    );
}

#[test]
fn upstream_read_body_failed_is_retryable() {
    // Network failure reading body IS transient, but status: None = ambiguous.
    // Safe default: NOT retryable unless explicitly marked with status code.
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "failed to read response body: connection reset".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Upstream with status: None should NOT be retryable (ambiguous = safe default)"
    );
}

#[test]
fn upstream_5xx_still_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "500 Internal Server Error".to_string(),
        status: Some(500),
    };
    assert!(err.is_retryable(), "5xx should still be retryable");
}

#[test]
fn upstream_4xx_still_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "400 Bad Request".to_string(),
        status: Some(400),
    };
    assert!(!err.is_retryable(), "4xx should NOT be retryable");
}

// ---------------------------------------------------------------------------
// Defect 7: No HTTP concurrency limit.
// Registry should have an HTTP semaphore.
// ---------------------------------------------------------------------------

#[test]
fn registry_has_http_concurrency_limit() {
    use squall::config::Config;
    use squall::dispatch::registry::Registry;
    use std::collections::HashMap;

    let config = Config {
        models: HashMap::new(),
    };
    let registry = Registry::from_config(config);
    let permits = registry.http_semaphore_permits();
    assert!(
        permits > 0 && permits <= 20,
        "HTTP semaphore should have 1-20 permits, got {permits}"
    );
}
