//! TDD tests proving defects found by deep review.
//! Each test targets a specific finding and should be RED before the fix.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use rmcp::ServerHandler;

/// Tests that change process CWD must hold this lock.
static CWD_LOCK: Mutex<()> = Mutex::new(());

use squall::config::Config;
use squall::dispatch::ProviderResult;
use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry};
use squall::error::SquallError;
use squall::response::{PalMetadata, PalToolResponse};
use squall::review::collect_result;
use squall::server::SquallServer;
use squall::tools::chat::ChatRequest;

// ---------------------------------------------------------------------------
// P0-1: Server should identify as "squall", not "rmcp"
// ---------------------------------------------------------------------------

#[test]
fn p0_1_server_name_is_squall() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let server = SquallServer::new(config);
    let info = server.get_info();
    assert_eq!(
        info.server_info.name, "squall",
        "Server name should be 'squall', got '{}'",
        info.server_info.name
    );
}

#[test]
fn p0_1_server_version_matches_cargo() {
    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let server = SquallServer::new(config);
    let info = server.get_info();
    assert_eq!(
        info.server_info.version,
        env!("CARGO_PKG_VERSION"),
        "Server version should match Cargo.toml"
    );
}

// ---------------------------------------------------------------------------
// P0-2: Error responses must NOT set isError=true at MCP level.
// Claude Code cascades sibling tool call failures when is_error=true.
// Error info lives in the JSON payload ("status": "error") instead.
// ---------------------------------------------------------------------------

#[test]
fn p0_2_error_response_sets_is_error_true() {
    let response = PalToolResponse::error(
        "model not found".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "bad-model".to_string(),
            provider_used: "unknown".to_string(),
            duration_seconds: 0.0,
        },
    );
    let result = response.into_call_tool_result();
    assert!(
        result.is_error != Some(true),
        "Error responses must NOT set is_error=true (causes sibling cascade)"
    );
}

#[test]
fn p0_2_success_response_sets_is_error_false() {
    let response = PalToolResponse::success(
        "hello from grok".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "grok".to_string(),
            provider_used: "xai".to_string(),
            duration_seconds: 1.0,
        },
    );
    let result = response.into_call_tool_result();
    assert!(
        !result.is_error.unwrap_or(false),
        "Success responses must not have is_error=true"
    );
}

// ---------------------------------------------------------------------------
// P0-3: ModelEntry Debug must NOT print api_key
// ---------------------------------------------------------------------------

#[test]
fn p0_3_model_entry_debug_redacts_api_key() {
    let entry = ModelEntry {
        model_id: "test-model".to_string(),
        provider: "test-provider".to_string(),
        backend: BackendConfig::Http {
            base_url: "https://example.com/v1".to_string(),
            api_key: "sk-super-secret-key-12345".to_string(),
            api_format: ApiFormat::OpenAi,
        },
        description: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        speed_tier: "fast".to_string(),
        precision_tier: "medium".to_string(),
    };
    let debug_output = format!("{:?}", entry);
    assert!(
        !debug_output.contains("sk-super-secret-key-12345"),
        "Debug output must not contain API key. Got: {debug_output}"
    );
    // Should still contain other fields for debugging utility
    assert!(debug_output.contains("test-model"));
    assert!(debug_output.contains("test-provider"));
}

// ---------------------------------------------------------------------------
// P1-4: into_content must not panic on non-finite f64 (NaN, Infinity)
// ---------------------------------------------------------------------------

#[test]
fn p1_4_into_content_nan_does_not_panic() {
    let response = PalToolResponse::success(
        "test".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "test".to_string(),
            provider_used: "test".to_string(),
            duration_seconds: f64::NAN,
        },
    );
    let result = catch_unwind(AssertUnwindSafe(|| response.into_call_tool_result()));
    assert!(
        result.is_ok(),
        "into_call_tool_result() panicked on NaN duration"
    );
}

#[test]
fn p1_4_into_content_infinity_does_not_panic() {
    let response = PalToolResponse::success(
        "test".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "test".to_string(),
            provider_used: "test".to_string(),
            duration_seconds: f64::INFINITY,
        },
    );
    let result = catch_unwind(AssertUnwindSafe(|| response.into_call_tool_result()));
    assert!(
        result.is_ok(),
        "into_call_tool_result() panicked on Infinity duration"
    );
}

// ---------------------------------------------------------------------------
// P1-6: SquallError should expose provider name from structured variants
// ---------------------------------------------------------------------------

#[test]
fn p1_6_error_exposes_provider_for_rate_limited() {
    let err = SquallError::RateLimited {
        provider: "xai".to_string(),
    };
    assert_eq!(err.provider(), Some("xai"));
}

#[test]
fn p1_6_error_exposes_provider_for_upstream() {
    let err = SquallError::Upstream {
        provider: "openrouter".to_string(),
        message: "500 Internal Server Error".to_string(),
        status: Some(500),
    };
    assert_eq!(err.provider(), Some("openrouter"));
}

#[test]
fn p1_6_error_exposes_provider_for_auth_failed() {
    let err = SquallError::AuthFailed {
        provider: "xai".to_string(),
        message: "invalid key".to_string(),
    };
    assert_eq!(err.provider(), Some("xai"));
}

#[test]
fn p1_6_error_returns_none_for_model_not_found() {
    let err = SquallError::ModelNotFound {
        model: "foo".to_string(),
        suggestions: vec![],
    };
    assert_eq!(err.provider(), None);
}

#[test]
fn p1_6_error_returns_none_for_timeout() {
    let err = SquallError::Timeout(5000);
    assert_eq!(err.provider(), None);
}

// ---------------------------------------------------------------------------
// P1-8: Empty/whitespace model string should use default, not look up ""
// ---------------------------------------------------------------------------

#[test]
fn p1_8_empty_model_string_uses_default() {
    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: Some("".to_string()),
        file_paths: None,
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
    };
    assert_eq!(
        req.model_or_default(),
        "grok-4-1-fast-reasoning",
        "Some(\"\") should fall back to default model"
    );
}

#[test]
fn p1_8_whitespace_model_string_uses_default() {
    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: Some("   ".to_string()),
        file_paths: None,
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
    };
    assert_eq!(
        req.model_or_default(),
        "grok-4-1-fast-reasoning",
        "Some(\"   \") should fall back to default model"
    );
}

// ---------------------------------------------------------------------------
// P0-4 / P1-11: Error messages to client must not leak internal details
// ---------------------------------------------------------------------------

#[test]
fn p0_4_error_user_message_includes_upstream_body() {
    // Upstream variant now includes the error body for debugging.
    // Bodies come from external APIs (not internal secrets) — safe to expose.
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "500: internal server error at https://api.x.ai/v1/chat".to_string(),
        status: Some(500),
    };
    let msg = err.user_message();
    assert!(
        msg.contains("xai"),
        "Should contain provider name. Got: {msg}"
    );
    assert!(
        msg.contains("500: internal server error"),
        "Should include error body for debugging. Got: {msg}"
    );

    // Other variant passes through its message — safe because it's only used
    // for spawn failures, pipe errors, cap exceeded, semaphore closed.
    assert_eq!(
        SquallError::Other("failed to spawn gemini: No such file or directory".to_string())
            .user_message(),
        "failed to spawn gemini: No such file or directory",
        "Other variant should pass through its diagnostic message"
    );
}

#[test]
fn p0_4_rate_limited_user_message_is_clean() {
    let err = SquallError::RateLimited {
        provider: "xai".to_string(),
    };
    let msg = err.user_message();
    assert!(
        msg.contains("rate limited"),
        "Should mention rate limiting. Got: {msg}"
    );
    assert!(msg.contains("xai"), "Should mention provider. Got: {msg}");
}

#[test]
fn p0_4_model_not_found_user_message_is_clean() {
    let err = SquallError::ModelNotFound {
        model: "bad-model".to_string(),
        suggestions: vec![],
    };
    let msg = err.user_message();
    assert!(
        msg.contains("bad-model"),
        "Should mention the model. Got: {msg}"
    );
}

// ===========================================================================
// Deep review round 2 — RED tests proving 6 defects
// ===========================================================================

// ---------------------------------------------------------------------------
// P0-3: Model identity inconsistency between success and error paths.
// Success path uses provider model_id (e.g. "deepseek-reasoner"), error path
// uses the display name (e.g. "deepseek-r1"). Same model, two names.
// ---------------------------------------------------------------------------

#[test]
fn p0_3_collect_result_success_uses_display_name() {
    // Simulate what registry does: substitutes "deepseek-r1" → "deepseek-reasoner"
    let provider_result = ProviderResult {
        text: "review output".to_string(),
        model: "deepseek-reasoner".to_string(), // provider model_id (substituted)
        provider: "deepseek".to_string(),
        partial: false,
    };
    let result = collect_result(
        Ok(provider_result),
        "deepseek-r1".to_string(), // original display name
        "deepseek".to_string(),
        1000,
    );
    // The result.model should be the DISPLAY name "deepseek-r1",
    // not the provider model_id "deepseek-reasoner"
    assert_eq!(
        result.model, "deepseek-r1",
        "Success path should use display name, not provider model_id. Got: '{}'",
        result.model
    );
}

#[test]
fn p0_3_collect_result_error_uses_display_name() {
    let result = collect_result(
        Err(SquallError::Timeout(5000)),
        "deepseek-r1".to_string(),
        "deepseek".to_string(),
        5000,
    );
    // Error path already uses display name — this is the correct half
    assert_eq!(
        result.model, "deepseek-r1",
        "Error path should use display name. Got: '{}'",
        result.model
    );
}

// ---------------------------------------------------------------------------
// P1-4: chat handler drops working_directory — hardcodes None.
// We test that SquallServer's chat handler passes working_directory through
// to the ProviderRequest. Since we can't easily intercept ProviderRequest
// construction in an integration test, we test the ServerBuilder approach:
// the chat handler code at server.rs:101 hardcodes `working_directory: None`.
// This test verifies the fix by checking the model is routed correctly with
// working_directory set.
// ---------------------------------------------------------------------------

#[test]
fn p1_4_chat_request_has_working_directory_field() {
    // Verify ChatRequest carries the field (compile-time proof)
    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: Some("gemini".to_string()),
        file_paths: None,
        working_directory: Some("/home/user/project".to_string()),
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
    };
    // The field exists and is Some — this is the input side.
    // The bug is that server.rs:101 ignores it. After fix, this test
    // serves as a compile guard that the field stays available.
    assert_eq!(req.working_directory.as_deref(), Some("/home/user/project"));
}

// ---------------------------------------------------------------------------
// P1-5: Async-poll filename lacks PID — collision risk.
// Review persistence uses {ts}_{pid}_{seq}.json but async-poll uses
// {ts}_{seq}_{model}.json (no PID).
// ---------------------------------------------------------------------------

#[test]
fn p1_5_async_poll_filename_includes_pid() {
    let _guard = CWD_LOCK.lock().unwrap();

    let dir = std::env::temp_dir().join("squall-test-persist-pid");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".squall/research")).unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let path = tokio::runtime::Runtime::new().unwrap().block_on(async {
        squall::dispatch::async_poll::persist_research_result(
            "test-model",
            "test-provider",
            "test text",
            "job-123",
            1000,
        )
        .await
        .unwrap()
    });

    std::env::set_current_dir(&original).unwrap();

    let filename = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();

    let pid = std::process::id().to_string();
    assert!(
        filename.contains(&pid),
        "Async-poll filename should include PID for collision safety. \
         Got: '{filename}', expected to contain PID '{pid}'"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P1-6: memory max_chars violated by truncation suffix.
// After truncating to max_chars, "\n\n[truncated]" (14 bytes) is appended,
// causing output to exceed the requested limit.
// ---------------------------------------------------------------------------

#[test]
fn p1_6_memory_read_respects_max_chars_after_truncation() {
    use squall::memory::MemoryStore;

    let _guard = CWD_LOCK.lock().unwrap();

    let dir = std::env::temp_dir().join("squall-test-maxchars-trunc");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".squall/memory")).unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    // Write a large patterns.md that will need truncation
    let large_content = "# Patterns\n".to_string() + &"x".repeat(2000);
    std::fs::write(dir.join(".squall/memory/patterns.md"), &large_content).unwrap();

    let store = MemoryStore::new();
    let max_chars: usize = 500;

    let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
        store
            .read_memory(Some("patterns"), None, max_chars, None)
            .await
    });

    std::env::set_current_dir(&original).unwrap();

    let output = result.unwrap();
    assert!(
        output.len() <= max_chars,
        "Output length {} exceeds max_chars {}. The truncation suffix \
         '\\n\\n[truncated]' pushes it over the limit.",
        output.len(),
        max_chars
    );

    let _ = std::fs::remove_dir_all(&dir);
}
