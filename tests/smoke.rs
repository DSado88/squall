use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry};
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
            api_format: ApiFormat::OpenAi,
        },
        description: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        speed_tier: "fast".to_string(),
        precision_tier: "medium".to_string(),
    };

    assert!(matches!(http_entry.backend, BackendConfig::Http { .. }));
    assert_eq!(http_entry.backend_name(), "http");

    let cli_entry = ModelEntry {
        model_id: "gemini".to_string(),
        provider: "google".to_string(),
        backend: BackendConfig::Cli {
            executable: "gemini".to_string(),
            args_template: vec!["-o".to_string(), "json".to_string()],
        },
        description: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        speed_tier: "fast".to_string(),
        precision_tier: "medium".to_string(),
    };

    assert!(matches!(cli_entry.backend, BackendConfig::Cli { .. }));
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
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
    };
    assert_eq!(req.model_or_default(), "grok-4-1-fast-reasoning");

    let req = ChatRequest {
        prompt: "hello".to_string(),
        model: Some("moonshotai/kimi-k2.5".to_string()),
        file_paths: None,
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
    };
    assert_eq!(req.model_or_default(), "moonshotai/kimi-k2.5");
}

// ---------------------------------------------------------------------------
// listmodels returns enriched capability fields
// ---------------------------------------------------------------------------

#[test]
fn listmodels_returns_capability_fields() {
    use squall::config::Config;
    use squall::dispatch::registry::Registry;
    use squall::tools::listmodels::ModelInfo;

    let config = Config::from_env();
    let registry = Registry::from_config(config);
    let entries = registry.list_models();

    // There should be at least one model registered (env-dependent, but
    // grok/gemini/codex are always present when respective env vars are set).
    // If no models are configured, the test is vacuously true.
    for (key, entry) in &entries {
        let info = ModelInfo::from((*key, *entry));

        // description must not be empty for any registered model
        assert!(
            !info.description.is_empty(),
            "Model '{}' has empty description",
            info.name
        );

        // speed_tier and precision_tier must be non-empty
        assert!(
            !info.speed_tier.is_empty(),
            "Model '{}' has empty speed_tier",
            info.name
        );
        assert!(
            !info.precision_tier.is_empty(),
            "Model '{}' has empty precision_tier",
            info.name
        );

        // strengths should have at least one entry
        assert!(
            !info.strengths.is_empty(),
            "Model '{}' has empty strengths",
            info.name
        );
    }
}
