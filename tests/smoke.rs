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

// ===========================================================================
// Markdown responses
// ===========================================================================

#[test]
fn listmodels_returns_markdown_table() {
    use squall::tools::listmodels::{ListModelsResponse, ModelInfo};

    let response = ListModelsResponse {
        models: vec![ModelInfo {
            name: "test-model".to_string(),
            provider: "test-provider".to_string(),
            backend: "http".to_string(),
            description: "A test model".to_string(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        }],
    };

    let md = response.to_markdown();
    assert!(md.contains('|'), "Should be a markdown table");
    assert!(md.contains("test-model"), "Should contain model name");
    assert!(md.contains("test-provider"), "Should contain provider");
    assert!(!md.contains('{'), "Should not contain JSON braces");
}

#[test]
fn listmodels_markdown_escapes_pipes() {
    use squall::tools::listmodels::{ListModelsResponse, ModelInfo};

    let response = ListModelsResponse {
        models: vec![ModelInfo {
            name: "model|name".to_string(),
            provider: "provider".to_string(),
            backend: "http".to_string(),
            description: "desc with | pipe".to_string(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        }],
    };

    let md = response.to_markdown();
    assert!(
        md.contains(r"model\|name"),
        "Should escape pipes in model names"
    );
    assert!(
        md.contains(r"desc with \| pipe"),
        "Should escape pipes in descriptions"
    );
}

#[test]
fn listmodels_markdown_escapes_newlines() {
    use squall::tools::listmodels::{ListModelsResponse, ModelInfo};

    let response = ListModelsResponse {
        models: vec![ModelInfo {
            name: "test".to_string(),
            provider: "prov".to_string(),
            backend: "http".to_string(),
            description: "line one\nline two".to_string(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        }],
    };

    let md = response.to_markdown();
    // Each model must be a single table row — no raw newlines in cells
    let data_lines: Vec<&str> = md.lines().skip(2).collect(); // skip header + separator
    assert_eq!(data_lines.len(), 1, "Model entry should be a single row");
    assert!(
        !data_lines[0].contains('\n'),
        "Table row should not contain raw newlines"
    );
}

#[test]
fn listmodels_markdown_strips_carriage_returns() {
    use squall::tools::listmodels::{ListModelsResponse, ModelInfo};

    let response = ListModelsResponse {
        models: vec![ModelInfo {
            name: "test".to_string(),
            provider: "prov".to_string(),
            backend: "http".to_string(),
            description: "line one\r\nline two".to_string(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        }],
    };

    let md = response.to_markdown();
    assert!(
        !md.contains('\r'),
        "Markdown should not contain carriage returns"
    );
    let data_lines: Vec<&str> = md.lines().skip(2).collect();
    assert_eq!(
        data_lines.len(),
        1,
        "CRLF input should still be a single row"
    );
}

// ===========================================================================
// CLI name backward compatibility
// ===========================================================================

#[test]
fn clink_accepts_cli_name_alias() {
    use squall::tools::clink::ClinkRequest;

    let json = r#"{"cli_name": "gemini", "prompt": "hello"}"#;
    let req: ClinkRequest = serde_json::from_str(json).unwrap();
    assert_eq!(
        req.model, "gemini",
        "cli_name alias should deserialize into model field"
    );
}

#[test]
fn clink_accepts_model_field() {
    use squall::tools::clink::ClinkRequest;

    let json = r#"{"model": "codex", "prompt": "hello"}"#;
    let req: ClinkRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.model, "codex", "model field should work directly");
}

// ===========================================================================
// Enum serde aliases
// ===========================================================================

#[test]
fn memorize_category_accepts_plural_aliases() {
    use squall::tools::enums::MemorizeCategory;

    // "patterns" should deserialize to Pattern (common LLM mistake)
    let cat: MemorizeCategory = serde_json::from_str(r#""patterns""#).unwrap();
    assert_eq!(cat, MemorizeCategory::Pattern);

    // "tactics" should deserialize to Tactic
    let cat: MemorizeCategory = serde_json::from_str(r#""tactics""#).unwrap();
    assert_eq!(cat, MemorizeCategory::Tactic);

    // "recommendation" should deserialize to Recommend
    let cat: MemorizeCategory = serde_json::from_str(r#""recommendation""#).unwrap();
    assert_eq!(cat, MemorizeCategory::Recommend);
}

#[test]
fn memory_category_accepts_singular_aliases() {
    use squall::tools::enums::MemoryCategory;

    // "pattern" should deserialize to Patterns (common LLM mistake)
    let cat: MemoryCategory = serde_json::from_str(r#""pattern""#).unwrap();
    assert_eq!(cat, MemoryCategory::Patterns);

    // "tactic" should deserialize to Tactics
    let cat: MemoryCategory = serde_json::from_str(r#""tactic""#).unwrap();
    assert_eq!(cat, MemoryCategory::Tactics);
}

#[test]
fn memory_category_has_no_all_variant() {
    use squall::tools::enums::MemoryCategory;

    // "all" should NOT be a valid MemoryCategory — callers omit the field instead
    let result = serde_json::from_str::<MemoryCategory>(r#""all""#);
    assert!(
        result.is_err(),
        "MemoryCategory should not have an All variant — use Option::None instead"
    );
}

#[test]
fn reasoning_effort_enum_round_trips() {
    use squall::tools::enums::ReasoningEffort;

    for (json, variant) in [
        (r#""none""#, ReasoningEffort::None),
        (r#""low""#, ReasoningEffort::Low),
        (r#""medium""#, ReasoningEffort::Medium),
        (r#""high""#, ReasoningEffort::High),
        (r#""xhigh""#, ReasoningEffort::Xhigh),
    ] {
        let parsed: ReasoningEffort = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, variant);
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(serialized, json);
    }
}

#[test]
fn response_format_defaults_to_detailed() {
    use squall::tools::enums::ResponseFormat;

    assert_eq!(ResponseFormat::default(), ResponseFormat::Detailed);
}

// ===========================================================================
// reasoning_needs_extended_deadline coverage (mut-006 gap)
// ===========================================================================

#[test]
fn reasoning_extended_deadline_for_medium_and_above() {
    use squall::server::reasoning_needs_extended_deadline;
    use squall::tools::enums::ReasoningEffort;

    // None and Low should NOT trigger extended deadline
    assert!(!reasoning_needs_extended_deadline(None));
    assert!(!reasoning_needs_extended_deadline(Some(
        &ReasoningEffort::None
    )));
    assert!(!reasoning_needs_extended_deadline(Some(
        &ReasoningEffort::Low
    )));

    // Medium, High, Xhigh SHOULD trigger extended deadline
    assert!(reasoning_needs_extended_deadline(Some(
        &ReasoningEffort::Medium
    )));
    assert!(reasoning_needs_extended_deadline(Some(
        &ReasoningEffort::High
    )));
    assert!(reasoning_needs_extended_deadline(Some(
        &ReasoningEffort::Xhigh
    )));
}

// ===========================================================================
// Mutation survivors: enum→string wiring tests
// ===========================================================================

#[test]
fn memorize_category_as_str_matches_valid_categories() {
    use squall::tools::enums::MemorizeCategory;

    // These must match VALID_CATEGORIES = ["pattern", "tactic", "recommend"]
    assert_eq!(MemorizeCategory::Pattern.as_str(), "pattern");
    assert_eq!(MemorizeCategory::Tactic.as_str(), "tactic");
    assert_eq!(MemorizeCategory::Recommend.as_str(), "recommend");
}

#[test]
fn memory_category_as_str_matches_read_memory_checks() {
    use squall::tools::enums::MemoryCategory;

    // These must match the category string comparisons in local.rs read_memory()
    assert_eq!(MemoryCategory::Models.as_str(), "models");
    assert_eq!(MemoryCategory::Patterns.as_str(), "patterns");
    assert_eq!(MemoryCategory::Tactics.as_str(), "tactics");
    assert_eq!(MemoryCategory::Recommend.as_str(), "recommend");
}

#[test]
fn reasoning_effort_as_str_produces_lowercase() {
    use squall::tools::enums::ReasoningEffort;

    // Downstream dispatch/http.rs matches on lowercase strings like "high", "medium"
    // format!("{:?}", e) would produce "High" — must use as_str() which returns lowercase
    for (variant, expected) in [
        (ReasoningEffort::None, "none"),
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::Xhigh, "xhigh"),
    ] {
        assert_eq!(
            variant.as_str(),
            expected,
            "{:?} should produce {expected}",
            variant
        );
        // Verify it's truly lowercase (catches Debug format leak)
        assert_eq!(variant.as_str(), variant.as_str().to_lowercase());
    }
}
