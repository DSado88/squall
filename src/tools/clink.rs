use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClinkRequest {
    /// CLI agent name from `listmodels` (e.g. "gemini", "codex"). Must be a CLI-backend model.
    pub cli_name: String,
    /// The prompt to send to the CLI agent. File manifest from file_paths is prepended automatically.
    pub prompt: String,
    /// Relative file paths to include as a path manifest (listed but not inlined for CLI). Requires working_directory.
    pub file_paths: Option<Vec<String>>,
    /// Absolute path to the project root. Used as subprocess cwd and for resolving file_paths.
    pub working_directory: Option<String>,
    /// System prompt to set model persona/behavior (prepended to stdin for CLI agents).
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0.0 = deterministic (best for analysis/code), 1.0 = creative/diverse.
    pub temperature: Option<f64>,
    /// Maximum tokens to generate. Caps output length; useful for concise responses.
    pub max_tokens: Option<u64>,
    /// Reasoning effort for thinking models: "none" (fastest), "low", "medium", "high" (deepest).
    /// Non-reasoning models ignore this.
    pub reasoning_effort: Option<String>,
}
