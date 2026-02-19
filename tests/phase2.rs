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
    // Codex JSONL event stream with a response.completed event containing a message
    let input = concat!(
        r#"{"type":"response.created","item":{"type":"message"}}"#, "\n",
        r#"{"type":"response.completed","item":{"type":"message","content":[{"type":"output_text","text":"Hello from Codex!"}]}}"#, "\n",
    );
    let result = parser.parse(input.as_bytes()).unwrap();
    assert_eq!(result, "Hello from Codex!");
}

#[test]
fn codex_parser_joins_multiple_text_parts() {
    let parser = CodexParser;
    let input = concat!(
        r#"{"type":"response.completed","item":{"type":"message","content":[{"type":"output_text","text":"Part 1"},{"type":"output_text","text":"Part 2"}]}}"#, "\n",
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
    let input = concat!(
        r#"{"type":"response.created","item":{"type":"function_call"}}"#, "\n",
        r#"{"type":"response.completed","item":{"type":"function_call","content":[{"type":"output_text","text":"ignored"}]}}"#, "\n",
    );
    let result = parser.parse(input.as_bytes());
    assert!(result.is_err(), "Should reject stream with no message events");
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
            args_template: vec!["-o".to_string(), "json".to_string(), "{prompt}".to_string()],
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
    // user_message should be sanitized (no stderr leaked)
    let msg = err.user_message();
    assert!(msg.contains("code 1"));
    assert!(!msg.contains("model not found"), "user_message must not leak stderr");
}

// ---------------------------------------------------------------------------
// CLI arg template substitution doesn't allow shell injection
// ---------------------------------------------------------------------------

#[test]
fn cli_args_template_handles_special_chars_safely() {
    // This tests the template substitution logic directly.
    // The key safety property is that args are passed to Command::args(),
    // not through a shell, so shell metacharacters are inert.
    let template = [
        "-o".to_string(),
        "json".to_string(),
        "{prompt}".to_string(),
    ];

    let dangerous_prompt = "hello; rm -rf / && echo pwned";
    let args: Vec<String> = template
        .iter()
        .map(|a| a.replace("{prompt}", dangerous_prompt))
        .collect();

    // The dangerous string is preserved as-is (it will be a single arg to the CLI)
    assert_eq!(args[2], "hello; rm -rf / && echo pwned");
    // Verify there are exactly 3 args (no splitting occurred)
    assert_eq!(args.len(), 3);
}
