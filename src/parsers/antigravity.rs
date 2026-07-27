use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Antigravity CLI (`agy --print`) output.
///
/// Unlike the Gemini, Codex, and Claude CLIs, `agy` has no JSON output mode — the
/// `--help` surface exposes no `-o`/`--output-format` flag and `--print` emits the
/// response as plain text. So this parser is a trimmed passthrough; the only failure
/// mode it can detect is empty output.
///
/// `agy` reports its own failures on stderr with a non-zero exit, which the CLI
/// dispatch layer surfaces before the parser ever runs.
pub struct AntigravityParser;

impl OutputParser for AntigravityParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let text = String::from_utf8_lossy(stdout).trim().to_string();
        if text.is_empty() {
            return Err(SquallError::SchemaParse(
                "agy CLI produced empty output".to_string(),
            ));
        }
        Ok(text)
    }
}

/// Clamp a Squall reasoning effort to a value `agy --effort` accepts.
///
/// Squall's `ReasoningEffort` enum spans `none|low|medium|high|xhigh`; `agy` accepts
/// only `low|medium|high` and hard-fails the whole invocation on anything else
/// ("invalid --effort \"bogus\" (valid: low, medium, high)"). The two out-of-range
/// values are mapped to the nearest supported neighbour rather than dropped, so a
/// caller asking for `xhigh` still gets the deepest setting available.
pub fn clamp_effort(effort: &str) -> &'static str {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" | "low" => "low",
        "medium" => "medium",
        _ => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_text() {
        let out = AntigravityParser.parse(b"ok").unwrap();
        assert_eq!(out, "ok");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let out = AntigravityParser.parse(b"\n  the answer  \n\n").unwrap();
        assert_eq!(out, "the answer");
    }

    /// Multi-line responses must survive intact — only the outer edges are trimmed.
    #[test]
    fn preserves_internal_newlines() {
        let out = AntigravityParser.parse(b"line one\nline two\n").unwrap();
        assert_eq!(out, "line one\nline two");
    }

    #[test]
    fn rejects_empty_output() {
        assert!(AntigravityParser.parse(b"").is_err());
        assert!(AntigravityParser.parse(b"   \n  ").is_err());
    }

    /// JSON is not special-cased — agy has no JSON mode, so a literal brace is just text.
    #[test]
    fn does_not_try_to_parse_json() {
        let out = AntigravityParser.parse(br#"{"response": "hi"}"#).unwrap();
        assert_eq!(out, r#"{"response": "hi"}"#);
    }

    /// The two efforts agy rejects must map to neighbours, never pass through.
    #[test]
    fn clamps_out_of_range_efforts() {
        assert_eq!(clamp_effort("none"), "low");
        assert_eq!(clamp_effort("xhigh"), "high");
    }

    #[test]
    fn passes_through_supported_efforts() {
        assert_eq!(clamp_effort("low"), "low");
        assert_eq!(clamp_effort("medium"), "medium");
        assert_eq!(clamp_effort("high"), "high");
    }

    #[test]
    fn clamp_is_case_and_whitespace_insensitive() {
        assert_eq!(clamp_effort("  HIGH "), "high");
        assert_eq!(clamp_effort("Medium"), "medium");
    }

    /// Anything unrecognised must still be a valid agy value, not a dispatch failure.
    #[test]
    fn clamps_unknown_effort_to_valid_value() {
        for v in ["", "bogus", "extreme"] {
            let got = clamp_effort(v);
            assert!(
                matches!(got, "low" | "medium" | "high"),
                "clamp_effort({v:?}) returned {got:?}, which agy would reject"
            );
        }
    }
}
