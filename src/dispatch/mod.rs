pub mod async_poll;
pub mod cli;
pub mod http;
pub mod registry;

use std::time::Instant;

use tokio_util::sync::CancellationToken;

/// Internal request type — both HTTP and CLI backends accept this.
pub struct ProviderRequest {
    pub prompt: String,
    pub model: String,
    pub deadline: Instant,
    /// Working directory for CLI subprocess cwd (None for HTTP backends).
    pub working_directory: Option<String>,
    /// System prompt to set model behavior (HTTP: separate message, CLI: prepended to stdin).
    pub system_prompt: Option<String>,
    /// Sampling temperature (0 = deterministic, 1 = creative). Passed to HTTP APIs only.
    pub temperature: Option<f64>,
    /// Maximum tokens to generate. Passed to HTTP APIs only.
    pub max_tokens: Option<u64>,
    /// Reasoning effort level for thinking models (e.g. "none", "low", "medium", "high").
    /// Passed to HTTP APIs as `reasoning.effort`. Non-reasoning models ignore it.
    pub reasoning_effort: Option<String>,
    /// Cooperative cancellation signal from review executor. When cancelled,
    /// streaming backends return accumulated partial text instead of aborting.
    pub cancellation_token: Option<CancellationToken>,
    /// Override stall timeout for non-reasoning slow models (Kimi, GLM).
    /// Clamped to min(stall_timeout, remaining deadline) at read time.
    pub stall_timeout: Option<std::time::Duration>,
}

/// Internal result type — all backends return this.
#[derive(Debug)]
pub struct ProviderResult {
    pub text: String,
    pub model: String,
    pub provider: String,
    /// True if the result was truncated due to cancellation, deadline, or stall.
    pub partial: bool,
}
