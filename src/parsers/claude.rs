use serde::Deserialize;

use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Claude Code CLI `--output-format json` output (single JSON object).
/// Schema (captured from `claude --print --output-format json`):
///   {"type":"result","subtype":"success","is_error":false,...,"result":"<text>", ...}
/// On error: `is_error` is true; `result` may be missing or contain partial text.
pub struct ClaudeParser;

#[derive(Deserialize)]
struct ClaudeResult {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
}

impl OutputParser for ClaudeParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let raw = String::from_utf8_lossy(stdout);
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(SquallError::SchemaParse(
                "claude CLI produced empty output".to_string(),
            ));
        }

        let parsed: ClaudeResult = serde_json::from_str(trimmed).map_err(|e| {
            SquallError::SchemaParse(format!("claude CLI: invalid JSON output: {e}"))
        })?;

        if parsed.is_error {
            return Err(SquallError::Other(format!(
                "claude CLI reported error (subtype: {})",
                parsed.subtype.as_deref().unwrap_or("unknown")
            )));
        }

        match parsed.result {
            Some(text) if !text.is_empty() => Ok(text),
            _ => Err(SquallError::SchemaParse(
                "claude CLI: no `result` text in output".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_result() {
        let input = br#"{"type":"result","subtype":"success","is_error":false,"result":"4","duration_ms":2489}"#;
        let out = ClaudeParser.parse(input).unwrap();
        assert_eq!(out, "4");
    }

    #[test]
    fn parses_multiline_result() {
        let input = br#"{"type":"result","is_error":false,"result":"line one\nline two"}"#;
        let out = ClaudeParser.parse(input).unwrap();
        assert_eq!(out, "line one\nline two");
    }

    #[test]
    fn rejects_is_error_true() {
        let input = br#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#;
        let err = ClaudeParser.parse(input).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("error_max_turns"), "got: {msg}");
    }

    #[test]
    fn rejects_empty_stdout() {
        assert!(ClaudeParser.parse(b"").is_err());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(ClaudeParser.parse(b"not json at all").is_err());
    }

    #[test]
    fn rejects_missing_result_field() {
        let input = br#"{"type":"result","is_error":false}"#;
        assert!(ClaudeParser.parse(input).is_err());
    }
}
