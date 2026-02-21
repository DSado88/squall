//! TDD tests proving defects found by deep review.
//! Each test targets a specific finding and should be RED before the fix.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use rmcp::ServerHandler;

use squall::config::Config;
use squall::dispatch::registry::{BackendConfig, ModelEntry};
use squall::error::SquallError;
use squall::response::{PalMetadata, PalToolResponse};
use squall::server::SquallServer;
use squall::tools::chat::ChatRequest;

// ---------------------------------------------------------------------------
// P0-1: Server should identify as "squall", not "rmcp"
// ---------------------------------------------------------------------------

#[test]
fn p0_1_server_name_is_squall() {
    let config = Config { models: HashMap::new() };
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
    let config = Config { models: HashMap::new() };
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
        },
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
    let result = catch_unwind(AssertUnwindSafe(|| {
        response.into_call_tool_result()
    }));
    assert!(result.is_ok(), "into_call_tool_result() panicked on NaN duration");
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
    let result = catch_unwind(AssertUnwindSafe(|| {
        response.into_call_tool_result()
    }));
    assert!(result.is_ok(), "into_call_tool_result() panicked on Infinity duration");
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
    let err = SquallError::ModelNotFound { model: "foo".to_string(), suggestions: vec![] };
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
fn p0_4_error_user_message_does_not_contain_url() {
    // Simulate a reqwest-style error message that contains a URL
    let err = SquallError::Other(
        "error sending request for url (https://api.x.ai/v1/chat/completions): connection refused".to_string()
    );
    let msg = err.user_message();
    assert!(
        !msg.contains("api.x.ai"),
        "User-facing error message should not contain internal URLs. Got: {msg}"
    );
}

#[test]
fn p0_4_rate_limited_user_message_is_clean() {
    let err = SquallError::RateLimited {
        provider: "xai".to_string(),
    };
    let msg = err.user_message();
    assert!(msg.contains("rate limited"), "Should mention rate limiting. Got: {msg}");
    assert!(msg.contains("xai"), "Should mention provider. Got: {msg}");
}

#[test]
fn p0_4_model_not_found_user_message_is_clean() {
    let err = SquallError::ModelNotFound { model: "bad-model".to_string(), suggestions: vec![] };
    let msg = err.user_message();
    assert!(msg.contains("bad-model"), "Should mention the model. Got: {msg}");
}
