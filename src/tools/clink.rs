use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClinkRequest {
    /// CLI agent name: "gemini" or "codex"
    pub cli_name: String,
    /// The prompt to send to the CLI agent
    pub prompt: String,
    /// Relative file paths to include as context manifest (paths only for CLI)
    pub file_paths: Option<Vec<String>>,
    /// Working directory for resolving file_paths and as subprocess cwd
    pub working_directory: Option<String>,
    /// System prompt to set model behavior (prepended to stdin for CLI agents)
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0 = deterministic (best for analysis), 1 = creative
    pub temperature: Option<f64>,
    /// Maximum tokens to generate (caps output length)
    pub max_tokens: Option<u64>,
    /// Reasoning effort for thinking models: "none" (fast), "low", "medium", "high" (deep).
    /// Non-reasoning models ignore this.
    pub reasoning_effort: Option<String>,
}
