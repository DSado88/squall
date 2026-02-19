use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ParallelRequest {
    /// The prompt to send to all models
    pub prompt: String,
    /// List of model names to query concurrently
    pub models: Vec<String>,
    /// Max characters per response (default: 3000)
    pub max_chars_per_response: Option<u32>,
    /// Minimum number of successful responses required (default: 1)
    pub min_successes: Option<u32>,
    /// Deadline in milliseconds (default: 30000)
    pub deadline_ms: Option<u32>,
}
