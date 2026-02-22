use schemars::JsonSchema;
use serde::Deserialize;

use crate::context::ContextFormat;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChatRequest {
    /// Model name from `listmodels` output (defaults to grok-4-1-fast-reasoning). Use exact names.
    pub model: Option<String>,
    /// The prompt to send to the model. File context from file_paths is prepended automatically.
    pub prompt: String,
    /// Relative file paths to include as context (read and inlined server-side). Requires working_directory.
    pub file_paths: Option<Vec<String>>,
    /// Absolute path to the project root for resolving file_paths. Required when file_paths is set.
    pub working_directory: Option<String>,
    /// System prompt to set model persona/behavior (e.g. "You are an expert security auditor").
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0.0 = deterministic (best for analysis/code), 1.0 = creative/diverse.
    pub temperature: Option<f64>,
    /// Maximum tokens to generate. Caps output length; useful for concise responses.
    pub max_tokens: Option<u64>,
    /// Reasoning effort for thinking models: "none" (fastest), "low", "medium", "high" (deepest).
    /// Non-reasoning models ignore this. "medium"/"high" automatically extend the deadline to 600s.
    pub reasoning_effort: Option<String>,
    /// File context format: "xml" (default, full content) or "hashline" (line_num:hash|content,
    /// compact for large files). Hashline lets models reference lines by number+hash.
    pub context_format: Option<ContextFormat>,
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
