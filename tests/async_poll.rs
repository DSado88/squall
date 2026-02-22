//! Tests for the async-poll dispatch backend.
//! Covers request/response parsing for OpenAI Responses API and Gemini Interactions API,
//! dispatch integration, error handling, and registry wiring.

use squall::dispatch::async_poll::{
    AsyncPollApi, GeminiInteractionsApi, OpenAiResponsesApi, PollStatus,
};
use squall::dispatch::registry::{AsyncPollProviderType, BackendConfig, ModelEntry};
use squall::error::SquallError;

// ---------------------------------------------------------------------------
// OpenAI Responses API: request/response parsing
// ---------------------------------------------------------------------------

#[test]
fn openai_launch_request_has_required_fields() {
    let api = OpenAiResponsesApi;
    let (url, headers, body) =
        api.build_launch_request("What is Rust?", "o3-deep-research", "sk-test", None);

    assert_eq!(url, "https://api.openai.com/v1/responses");
    assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
    assert_eq!(body["model"], "o3-deep-research");
    assert_eq!(body["background"], true);
    assert_eq!(body["store"], true);
    assert_eq!(body["tools"][0]["type"], "web_search_preview");
    // User message should be present
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.last().unwrap()["role"], "user");
    assert_eq!(input.last().unwrap()["content"], "What is Rust?");
}

#[test]
fn openai_launch_request_with_system_prompt() {
    let api = OpenAiResponsesApi;
    let (_, _, body) = api.build_launch_request(
        "Research topic",
        "o3-deep-research",
        "sk-test",
        Some("You are a research assistant"),
    );

    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["role"], "developer");
    assert_eq!(input[0]["content"], "You are a research assistant");
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn openai_parse_launch_response_extracts_id() {
    let api = OpenAiResponsesApi;
    let body = br#"{"id": "resp_abc123", "status": "queued"}"#;
    let id = api.parse_launch_response(body).unwrap();
    assert_eq!(id, "resp_abc123");
}

#[test]
fn openai_parse_launch_response_missing_id() {
    let api = OpenAiResponsesApi;
    let body = br#"{"status": "queued"}"#;
    let err = api.parse_launch_response(body).unwrap_err();
    assert!(matches!(err, SquallError::SchemaParse(_)));
}

#[test]
fn openai_poll_completed() {
    let api = OpenAiResponsesApi;
    let body = br#"{"status": "completed", "output_text": "Research findings here"}"#;
    match api.parse_poll_response(body).unwrap() {
        PollStatus::Completed(text) => assert_eq!(text, "Research findings here"),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn openai_poll_in_progress() {
    let api = OpenAiResponsesApi;
    for status in ["queued", "in_progress"] {
        let body = format!(r#"{{"status": "{status}"}}"#);
        match api.parse_poll_response(body.as_bytes()).unwrap() {
            PollStatus::InProgress => {}
            other => panic!("expected InProgress for {status}, got {other:?}"),
        }
    }
}

#[test]
fn openai_poll_failed() {
    let api = OpenAiResponsesApi;
    for status in ["failed", "incomplete", "cancelled"] {
        let body = format!(r#"{{"status": "{status}"}}"#);
        match api.parse_poll_response(body.as_bytes()).unwrap() {
            PollStatus::Failed(msg) => assert!(msg.contains(status)),
            other => panic!("expected Failed for {status}, got {other:?}"),
        }
    }
}

#[test]
fn openai_poll_missing_status() {
    let api = OpenAiResponsesApi;
    let body = br#"{"id": "resp_123"}"#;
    let err = api.parse_poll_response(body).unwrap_err();
    assert!(matches!(err, SquallError::SchemaParse(_)));
}

// ---------------------------------------------------------------------------
// Gemini Interactions API: request/response parsing
// ---------------------------------------------------------------------------

#[test]
fn gemini_launch_request_has_required_fields() {
    let api = GeminiInteractionsApi;
    let (url, headers, body) = api.build_launch_request(
        "What is quantum computing?",
        "deep-research-pro-preview-12-2025",
        "AIza_testkey",
        None,
    );

    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/interactions"
    );
    assert!(headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "AIza_testkey"));
    assert_eq!(body["agent"], "deep-research-pro-preview-12-2025");
    assert_eq!(body["input"], "What is quantum computing?");
    assert_eq!(body["background"], true);
}

#[test]
fn gemini_launch_request_with_system_prompt_prepended() {
    let api = GeminiInteractionsApi;
    let (_, _, body) = api.build_launch_request(
        "Research topic",
        "deep-research-pro-preview-12-2025",
        "AIza_testkey",
        Some("You are a research advisor"),
    );

    let input = body["input"].as_str().unwrap();
    assert!(input.starts_with("You are a research advisor"));
    assert!(input.contains("Research topic"));
}

#[test]
fn gemini_parse_launch_response_extracts_id() {
    let api = GeminiInteractionsApi;
    let body = br#"{"id": "interactions/abc123", "status": "in_progress"}"#;
    let id = api.parse_launch_response(body).unwrap();
    assert_eq!(id, "interactions/abc123");
}

#[test]
fn gemini_poll_completed() {
    let api = GeminiInteractionsApi;
    let body = br#"{
        "status": "completed",
        "outputs": [
            {"text": "intermediate step"},
            {"text": "Final research report"}
        ]
    }"#;
    match api.parse_poll_response(body).unwrap() {
        PollStatus::Completed(text) => assert_eq!(text, "Final research report"),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn gemini_poll_in_progress() {
    let api = GeminiInteractionsApi;
    let body = br#"{"status": "in_progress"}"#;
    match api.parse_poll_response(body).unwrap() {
        PollStatus::InProgress => {}
        other => panic!("expected InProgress, got {other:?}"),
    }
}

#[test]
fn gemini_poll_failed() {
    let api = GeminiInteractionsApi;
    let body = br#"{"status": "failed", "error": "quota exceeded"}"#;
    match api.parse_poll_response(body).unwrap() {
        PollStatus::Failed(msg) => assert_eq!(msg, "quota exceeded"),
        other => panic!("expected Failed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Poll interval and backoff
// ---------------------------------------------------------------------------

#[test]
fn openai_poll_interval_is_5_seconds() {
    let api = OpenAiResponsesApi;
    assert_eq!(api.poll_interval(), std::time::Duration::from_secs(5));
}

#[test]
fn gemini_poll_interval_is_45_seconds() {
    let api = GeminiInteractionsApi;
    assert_eq!(api.poll_interval(), std::time::Duration::from_secs(45));
}

// ---------------------------------------------------------------------------
// Backoff calculation (exponential, not linear)
// ---------------------------------------------------------------------------

use squall::dispatch::async_poll::AsyncPollDispatch;

#[test]
fn backoff_is_exponential() {
    let api = OpenAiResponsesApi;
    // base=5s, factor=1.5x per attempt, cap=60s
    let d0 = AsyncPollDispatch::next_poll_delay(&api, 0);
    let d1 = AsyncPollDispatch::next_poll_delay(&api, 1);
    let d2 = AsyncPollDispatch::next_poll_delay(&api, 2);
    let d10 = AsyncPollDispatch::next_poll_delay(&api, 10);

    assert_eq!(d0, std::time::Duration::from_secs(5));                  // 5 * 1.5^0 = 5
    assert_eq!(d1, std::time::Duration::from_millis(7500));             // 5 * 1.5^1 = 7.5
    assert_eq!(d2, std::time::Duration::from_millis(11250));            // 5 * 1.5^2 = 11.25
    assert_eq!(d10, std::time::Duration::from_secs(60));                // capped at max
}

#[test]
fn gemini_backoff_caps_at_120s() {
    let api = GeminiInteractionsApi;
    // base=45s, cap=120s
    let d0 = AsyncPollDispatch::next_poll_delay(&api, 0);
    let d1 = AsyncPollDispatch::next_poll_delay(&api, 1);
    let d5 = AsyncPollDispatch::next_poll_delay(&api, 5);

    assert_eq!(d0, std::time::Duration::from_secs(45));                 // 45 * 1.5^0 = 45
    assert_eq!(d1, std::time::Duration::from_millis(67500));            // 45 * 1.5^1 = 67.5
    assert_eq!(d5, std::time::Duration::from_secs(120));                // capped at max
}

// ---------------------------------------------------------------------------
// Model name sanitization
// ---------------------------------------------------------------------------

use squall::dispatch::async_poll::sanitize_model_name;

#[test]
fn sanitize_model_name_preserves_normal_names() {
    assert_eq!(sanitize_model_name("o3-deep-research"), "o3-deep-research");
    assert_eq!(sanitize_model_name("gemini"), "gemini");
}

#[test]
fn sanitize_model_name_replaces_slashes() {
    assert_eq!(sanitize_model_name("moonshotai/kimi-k2.5"), "moonshotai_kimi-k2_5");
}

#[test]
fn sanitize_model_name_strips_traversal_and_special_chars() {
    assert_eq!(sanitize_model_name("../../etc/passwd"), "______etc_passwd");
    assert_eq!(sanitize_model_name("model\x00name"), "model_name");
    assert_eq!(sanitize_model_name("a\\b"), "a_b");
}

// ---------------------------------------------------------------------------
// BackendConfig::AsyncPoll and ModelEntry
// ---------------------------------------------------------------------------

#[test]
fn async_poll_model_entry_backend_name() {
    let entry = ModelEntry {
        model_id: "o3-deep-research".to_string(),
        provider: "openai".to_string(),
        backend: BackendConfig::AsyncPoll {
            provider_type: AsyncPollProviderType::OpenAiResponses,
            api_key: "sk-test".to_string(),
        },
        description: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        speed_tier: "fast".to_string(),
        precision_tier: "medium".to_string(),
    };
    assert_eq!(entry.backend_name(), "async_poll");
    assert!(entry.is_async_poll());
    assert!(!matches!(entry.backend, BackendConfig::Http { .. }));
    assert!(!matches!(entry.backend, BackendConfig::Cli { .. }));
}

#[test]
fn async_poll_model_entry_debug_redacts_key() {
    let entry = ModelEntry {
        model_id: "o3-deep-research".to_string(),
        provider: "openai".to_string(),
        backend: BackendConfig::AsyncPoll {
            provider_type: AsyncPollProviderType::OpenAiResponses,
            api_key: "sk-super-secret-key".to_string(),
        },
        description: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        speed_tier: "fast".to_string(),
        precision_tier: "medium".to_string(),
    };
    let debug = format!("{entry:?}");
    assert!(debug.contains("[REDACTED]"), "API key should be redacted in Debug output");
    assert!(!debug.contains("sk-super-secret"), "API key must not appear in Debug output");
}

// ---------------------------------------------------------------------------
// Error variants
// ---------------------------------------------------------------------------

#[test]
fn async_job_failed_not_retryable() {
    let err = SquallError::AsyncJobFailed {
        provider: "openai".to_string(),
        message: "job failed".to_string(),
    };
    assert!(!err.is_retryable());
    assert_eq!(err.provider(), Some("openai"));
}

#[test]
fn poll_failed_is_retryable() {
    let err = SquallError::PollFailed {
        provider: "openai".to_string(),
        job_id: "resp_123".to_string(),
        message: "connection reset".to_string(),
    };
    assert!(err.is_retryable());
    assert_eq!(err.provider(), Some("openai"));
}

#[test]
fn async_job_failed_user_message_clean() {
    let err = SquallError::AsyncJobFailed {
        provider: "openai".to_string(),
        message: "internal server error with details".to_string(),
    };
    let msg = err.user_message();
    assert_eq!(msg, "deep research job failed for openai");
    // Should NOT contain internal details
    assert!(!msg.contains("internal server error"));
}

#[test]
fn poll_failed_user_message_clean() {
    let err = SquallError::PollFailed {
        provider: "gemini-api".to_string(),
        job_id: "interactions/secret123".to_string(),
        message: "HTTP 500".to_string(),
    };
    let msg = err.user_message();
    assert_eq!(msg, "failed to check research status for gemini-api");
    // Should NOT leak job ID or HTTP details
    assert!(!msg.contains("secret123"));
    assert!(!msg.contains("500"));
}

// ---------------------------------------------------------------------------
// Config registration (env-dependent)
// ---------------------------------------------------------------------------

#[test]
fn config_registers_openai_deep_research_when_key_set() {
    // This test depends on OPENAI_API_KEY being set in the environment.
    // If not set, deep research models should NOT be registered.
    let config = squall::config::Config::from_env();
    if std::env::var("OPENAI_API_KEY").is_ok() {
        assert!(config.models.contains_key("o3-deep-research"));
        assert!(config.models.contains_key("o4-mini-deep-research"));
        let entry = &config.models["o3-deep-research"];
        assert!(entry.is_async_poll());
        assert_eq!(entry.provider, "openai");
    } else {
        assert!(!config.models.contains_key("o3-deep-research"));
        assert!(!config.models.contains_key("o4-mini-deep-research"));
    }
}

#[test]
fn config_registers_gemini_deep_research_when_key_set() {
    let config = squall::config::Config::from_env();
    if std::env::var("GOOGLE_API_KEY").is_ok() {
        assert!(config.models.contains_key("deep-research-pro"));
        let entry = &config.models["deep-research-pro"];
        assert!(entry.is_async_poll());
        assert_eq!(entry.provider, "gemini-api");
        // model_id should be the full agent name
        assert_eq!(entry.model_id, "deep-research-pro-preview-12-2025");
    } else {
        assert!(!config.models.contains_key("deep-research-pro"));
    }
}
