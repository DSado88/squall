use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request to dispatch a prompt to multiple models with straggler cutoff.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviewRequest {
    /// The prompt to send to all models
    pub prompt: String,
    /// Specific models to query (defaults to all configured)
    pub models: Option<Vec<String>>,
    /// Straggler cutoff in seconds (default: 180)
    pub timeout_secs: Option<u64>,
    /// System prompt for all models (e.g. "You are an expert code reviewer")
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0 = deterministic, 1 = creative
    pub temperature: Option<f64>,
    /// Relative file paths to include as context
    pub file_paths: Option<Vec<String>>,
    /// Working directory for resolving file_paths
    pub working_directory: Option<String>,
}

impl ReviewRequest {
    pub const DEFAULT_TIMEOUT_SECS: u64 = 180;

    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(Self::DEFAULT_TIMEOUT_SECS)
    }
}

/// Per-model result in a review response.
#[derive(Debug, Serialize, Clone)]
pub struct ReviewModelResult {
    pub model: String,
    pub provider: String,
    pub status: ModelStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub latency_ms: u64,
}

/// Status of an individual model in a review.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    Success,
    Error,
}

/// Full review response (serialized to JSON for MCP and disk).
#[derive(Debug, Serialize)]
pub struct ReviewResponse {
    pub results: Vec<ReviewModelResult>,
    pub not_started: Vec<String>,
    pub cutoff_seconds: u64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_file: Option<String>,
}
