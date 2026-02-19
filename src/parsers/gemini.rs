use serde::Deserialize;

use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Gemini CLI `--output-format json` output.
/// Expected shape: `{"response": "...", ...}`
pub struct GeminiParser;

#[derive(Deserialize)]
struct GeminiOutput {
    response: Option<String>,
}

impl OutputParser for GeminiParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let output: GeminiOutput = serde_json::from_slice(stdout)
            .map_err(|e| SquallError::SchemaParse(format!("gemini JSON parse failed: {e}")))?;

        output
            .response
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                SquallError::SchemaParse("gemini response field is empty or missing".to_string())
            })
    }
}
