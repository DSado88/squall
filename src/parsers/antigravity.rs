use serde::Deserialize;

use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Antigravity CLI (`agy --output-format json`) output.
///
/// agy reports failure IN-BAND with exit code 0 — a run that errored still exits
/// cleanly and prints `"status": "ERROR"`. Parsing plain text therefore cannot tell
/// success from failure, and any stray CLI string (help text, a version number) reads
/// as a valid model answer. Requiring JSON makes both cases fail loudly.
pub struct AntigravityParser;

#[derive(Deserialize)]
struct AntigravityOutput {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    response: Option<String>,
}

impl OutputParser for AntigravityParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let raw = String::from_utf8_lossy(stdout);
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(SquallError::SchemaParse(
                "agy CLI produced empty output".to_string(),
            ));
        }

        let parsed: AntigravityOutput = serde_json::from_str(trimmed).map_err(|e| {
            SquallError::SchemaParse(format!(
                "agy CLI: invalid JSON output: {e} (first 120 bytes: {:.120})",
                trimmed
            ))
        })?;

        match parsed.status.as_deref() {
            Some("SUCCESS") | None => {}
            Some(other) => {
                return Err(SquallError::Other(format!(
                    "agy CLI reported status {other}"
                )));
            }
        }

        match parsed.response {
            Some(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
            _ => Err(SquallError::SchemaParse(
                "agy response field is empty or missing".to_string(),
            )),
        }
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
    fn parses_json_response() {
        let out = AntigravityParser
            .parse(br#"{"status":"SUCCESS","response":"ok\n","usage":{"total_tokens":5}}"#)
            .unwrap();
        assert_eq!(out, "ok");
    }

    /// agy reports failure in-band with exit code 0. Without checking `status`, a failed
    /// run is handed back as a successful model response.
    #[test]
    fn rejects_non_success_status() {
        let err = AntigravityParser
            .parse(br#"{"status":"ERROR","response":"boom"}"#)
            .expect_err("non-SUCCESS status must be an error");
        assert!(
            format!("{err}").contains("ERROR"),
            "error should name the status"
        );
    }

    /// A bare CLI string (help text, a version number) is not valid JSON and must not be
    /// mistaken for a model answer — this is how `--print "--version"` returned "1.1.8"
    /// as a successful response.
    #[test]
    fn rejects_non_json_output() {
        assert!(AntigravityParser.parse(b"1.1.8").is_err());
        assert!(AntigravityParser.parse(b"Usage of agy:").is_err());
    }

    #[test]
    fn rejects_empty_output() {
        assert!(AntigravityParser.parse(b"").is_err());
        assert!(AntigravityParser.parse(b"   \n  ").is_err());
    }

    #[test]
    fn rejects_empty_response_field() {
        assert!(
            AntigravityParser
                .parse(br#"{"status":"SUCCESS","response":""}"#)
                .is_err()
        );
    }

    /// Multi-line answers survive; only the outer edges are trimmed.
    #[test]
    fn preserves_internal_newlines() {
        let out = AntigravityParser
            .parse(br#"{"status":"SUCCESS","response":"line one\nline two\n"}"#)
            .unwrap();
        assert_eq!(out, "line one\nline two");
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
