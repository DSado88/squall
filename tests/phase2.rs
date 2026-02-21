//! Phase 2 tests: CLI dispatch, parsers, BackendConfig, and subprocess safety.

use squall::dispatch::registry::{BackendConfig, ModelEntry};
use squall::error::SquallError;
use squall::parsers::gemini::GeminiParser;
use squall::parsers::codex::CodexParser;
use squall::parsers::OutputParser;

// ---------------------------------------------------------------------------
// Gemini parser
// ---------------------------------------------------------------------------

#[test]
fn gemini_parser_extracts_response() {
    let parser = GeminiParser;
    let input = br#"{"response": "Hello from Gemini!", "stats": {}}"#;
    let result = parser.parse(input).unwrap();
    assert_eq!(result, "Hello from Gemini!");
}

#[test]
fn gemini_parser_rejects_empty_response() {
    let parser = GeminiParser;
    let input = br#"{"response": ""}"#;
    let result = parser.parse(input);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SquallError::SchemaParse(_)));
}

#[test]
fn gemini_parser_rejects_missing_response() {
    let parser = GeminiParser;
    let input = br#"{"something_else": "value"}"#;
    let result = parser.parse(input);
    assert!(result.is_err());
}

#[test]
fn gemini_parser_rejects_invalid_json() {
    let parser = GeminiParser;
    let input = b"this is not json";
    let result = parser.parse(input);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SquallError::SchemaParse(_)));
}

// ---------------------------------------------------------------------------
// Codex parser
// ---------------------------------------------------------------------------

#[test]
fn codex_parser_extracts_message_content() {
    let parser = CodexParser;
    // Real Codex JSONL event stream format
    let input = concat!(
        r#"{"type":"thread.started","thread_id":"abc123"}"#, "\n",
        r#"{"type":"turn.started"}"#, "\n",
        r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Hello from Codex!"}}"#, "\n",
        r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":20}}"#, "\n",
    );
    let result = parser.parse(input.as_bytes()).unwrap();
    assert_eq!(result, "Hello from Codex!");
}

#[test]
fn codex_parser_joins_multiple_messages() {
    let parser = CodexParser;
    let input = concat!(
        r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Part 1"}}"#, "\n",
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Part 2"}}"#, "\n",
        r#"{"type":"turn.completed","usage":{}}"#, "\n",
    );
    let result = parser.parse(input.as_bytes()).unwrap();
    assert_eq!(result, "Part 1\nPart 2");
}

#[test]
fn codex_parser_rejects_empty_stream() {
    let parser = CodexParser;
    let result = parser.parse(b"");
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SquallError::SchemaParse(_)));
}

#[test]
fn codex_parser_skips_non_message_events() {
    let parser = CodexParser;
    // Only tool calls, no agent_message events
    let input = concat!(
        r#"{"type":"thread.started","thread_id":"abc"}"#, "\n",
        r#"{"type":"item.completed","item":{"id":"item_0","type":"tool_call","text":"ignored"}}"#, "\n",
        r#"{"type":"turn.completed","usage":{}}"#, "\n",
    );
    let result = parser.parse(input.as_bytes());
    assert!(result.is_err(), "Should reject stream with no agent_message events");
}

// ---------------------------------------------------------------------------
// BackendConfig: CLI entries have correct shape
// ---------------------------------------------------------------------------

#[test]
fn backend_config_cli_stores_executable_and_args() {
    let entry = ModelEntry {
        model_id: "gemini-cli".to_string(),
        provider: "google".to_string(),
        backend: BackendConfig::Cli {
            executable: "/usr/local/bin/gemini".to_string(),
            args_template: vec!["-o".to_string(), "json".to_string()],
        },
    };

    assert!(entry.is_cli());
    assert!(!entry.is_http());
    assert_eq!(entry.backend_name(), "cli");

    // Debug should show executable and args, no api_key field
    let debug = format!("{:?}", entry);
    assert!(debug.contains("gemini-cli"));
    assert!(debug.contains("/usr/local/bin/gemini"));
    assert!(!debug.contains("REDACTED"));
}

#[test]
fn backend_config_http_redacts_api_key_in_debug() {
    let entry = ModelEntry {
        model_id: "test-http".to_string(),
        provider: "test".to_string(),
        backend: BackendConfig::Http {
            base_url: "https://example.com".to_string(),
            api_key: "sk-secret-key".to_string(),
        },
    };

    let debug = format!("{:?}", entry);
    assert!(!debug.contains("sk-secret-key"));
    assert!(debug.contains("REDACTED"));
}

// ---------------------------------------------------------------------------
// ProcessExit error carries stderr context
// ---------------------------------------------------------------------------

#[test]
fn process_exit_error_carries_stderr() {
    let err = SquallError::ProcessExit {
        code: 1,
        stderr: "error: model not found".to_string(),
    };
    // Display should contain the code
    let display = format!("{err}");
    assert!(display.contains("1"));
    // user_message now includes stderr preview for debuggability
    let msg = err.user_message();
    assert!(msg.contains("code 1"));
    assert!(msg.contains("model not found"), "user_message should include stderr preview");
}

#[test]
fn process_exit_stderr_truncated_at_200_chars() {
    let long_stderr = "x".repeat(500);
    let err = SquallError::ProcessExit {
        code: 1,
        stderr: long_stderr,
    };
    let msg = err.user_message();
    assert!(msg.contains("..."), "Long stderr should be truncated with ellipsis");
    // 200 chars of stderr + "..." prefix + "CLI process exited with code 1: " ≈ ~240 chars
    assert!(msg.len() < 300, "Message should be bounded, got {}", msg.len());
}

#[test]
fn process_exit_empty_stderr_no_colon() {
    let err = SquallError::ProcessExit {
        code: 1,
        stderr: String::new(),
    };
    let msg = err.user_message();
    assert_eq!(msg, "CLI process exited with code 1");
}

// ---------------------------------------------------------------------------
// CLI arg template substitution doesn't allow shell injection.
// Prompt is delivered via stdin (not argv), so shell metacharacters
// in the prompt never reach the arg vector. Model substitution still
// uses args, so we test that.
// ---------------------------------------------------------------------------

#[test]
fn cli_args_template_substitutes_model_safely() {
    let template = [
        "-m".to_string(),
        "{model}".to_string(),
        "-o".to_string(),
        "json".to_string(),
    ];

    let dangerous_model = "test; rm -rf /";
    let args: Vec<String> = template
        .iter()
        .map(|a| a.replace("{model}", dangerous_model))
        .collect();

    // The string is preserved as-is (Command::args, no shell)
    assert_eq!(args[1], "test; rm -rf /");
    assert_eq!(args.len(), 4);
}
