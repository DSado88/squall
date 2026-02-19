use squall::dispatch::registry::{BackendConfig, ModelEntry};
use squall::response::{PalMetadata, PalToolResponse};

#[test]
fn pal_response_success_serializes_correctly() {
    let response = PalToolResponse::success(
        "hello from grok".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "grok-4-1-fast-reasoning".to_string(),
            provider_used: "xai".to_string(),
            duration_seconds: 4.2,
        },
    );

    let json_str = serde_json::to_string(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["content"], "hello from grok");
    assert_eq!(parsed["content_type"], "text");
    assert_eq!(parsed["metadata"]["tool_name"], "chat");
    assert_eq!(parsed["metadata"]["model_used"], "grok-4-1-fast-reasoning");
    assert_eq!(parsed["metadata"]["provider_used"], "xai");
    assert!(parsed["metadata"]["duration_seconds"].is_f64());
}

#[test]
fn pal_response_error_serializes_correctly() {
    let response = PalToolResponse::error(
        "model not found: foo".to_string(),
        PalMetadata {
            tool_name: "chat".to_string(),
            model_used: "foo".to_string(),
            provider_used: "unknown".to_string(),
            duration_seconds: 0.001,
        },
    );

    let json_str = serde_json::to_string(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["content"], "model not found: foo");
}

#[test]
fn model_entry_backend_types() {
    let http_entry = ModelEntry {
        model_id: "grok-4-1-fast-reasoning".to_string(),
        provider: "xai".to_string(),
        backend: BackendConfig::Http {
            base_url: "https://api.x.ai/v1/chat/completions".to_string(),
            api_key: "test-key".to_string(),
        },
    };

    assert!(http_entry.is_http());
    assert!(!http_entry.is_cli());
    assert_eq!(http_entry.backend_name(), "http");

    let cli_entry = ModelEntry {
        model_id: "gemini".to_string(),
        provider: "google".to_string(),
        backend: BackendConfig::Cli {
            executable: "gemini".to_string(),
            args_template: vec!["-o".to_string(), "json".to_string()],
        },
    };

    assert!(cli_entry.is_cli());
    assert!(!cli_entry.is_http());
    assert_eq!(cli_entry.backend_name(), "cli");
}

#[test]
fn chat_request_default_model() {
    use squall::tools::chat::ChatRequest;

    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: None,
        file_paths: None,
        working_directory: None,
    };
    assert_eq!(req.model_or_default(), "grok-4-1-fast-reasoning");

    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: Some("moonshotai/kimi-k2.5".to_string()),
        file_paths: None,
        working_directory: None,
    };
    assert_eq!(req.model_or_default(), "moonshotai/kimi-k2.5");
}
