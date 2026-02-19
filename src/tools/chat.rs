use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChatRequest {
    /// The prompt to send to the model
    pub prompt: String,
    /// Model name (defaults to grok-4-1-fast-reasoning)
    pub model: Option<String>,
    /// Relative file paths to include as context (read server-side)
    pub file_paths: Option<Vec<String>>,
    /// Working directory for resolving file_paths (required when file_paths is set)
    pub working_directory: Option<String>,
}

pub const DEFAULT_MODEL: &str = "grok-4-1-fast-reasoning";

impl ChatRequest {
    pub fn model_or_default(&self) -> &str {
        self.model
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(DEFAULT_MODEL)
    }
}
