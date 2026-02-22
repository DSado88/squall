use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChatRequest {
    /// Model name (defaults to grok-4-1-fast-reasoning)
    pub model: Option<String>,
    /// The prompt to send to the model
    pub prompt: String,
    /// Relative file paths to include as context (read server-side)
    pub file_paths: Option<Vec<String>>,
    /// Working directory for resolving file_paths (required when file_paths is set)
    pub working_directory: Option<String>,
    /// System prompt to set model behavior (e.g. "You are an expert code reviewer")
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0 = deterministic (best for analysis), 1 = creative
    pub temperature: Option<f64>,
    /// Maximum tokens to generate (caps output length)
    pub max_tokens: Option<u64>,
    /// Reasoning effort for thinking models: "none" (fast), "low", "medium", "high" (deep).
    /// Non-reasoning models ignore this. Automatically extends the deadline for "medium"/"high".
    pub reasoning_effort: Option<String>,
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
