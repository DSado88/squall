use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClinkRequest {
    /// The prompt to send to the CLI agent
    pub prompt: String,
    /// CLI agent name: "gemini" or "codex"
    pub cli_name: String,
    /// Relative file paths to include as context manifest (paths only for CLI)
    pub file_paths: Option<Vec<String>>,
    /// Working directory for resolving file_paths and as subprocess cwd
    pub working_directory: Option<String>,
}
