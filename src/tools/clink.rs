use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClinkRequest {
    /// The prompt to send to the CLI agent
    pub prompt: String,
    /// CLI agent name: "gemini" or "codex"
    pub cli_name: String,
}
