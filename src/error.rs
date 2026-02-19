use thiserror::Error;

#[derive(Debug, Error)]
pub enum SquallError {
    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("timeout after {0}ms")]
    Timeout(u64),

    #[error("rate limited by {provider}")]
    RateLimited { provider: String },

    #[error("upstream error from {provider}: {message}")]
    Upstream { provider: String, message: String },

    #[error("auth failed for {provider}: {message}")]
    AuthFailed { provider: String, message: String },

    #[error("schema parse error: {0}")]
    SchemaParse(String),

    #[error("process exited with code {code}: {stderr}")]
    ProcessExit { code: i32, stderr: String },

    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("{0}")]
    Other(String),
}

impl SquallError {
    /// Extract provider name from structured error variants.
    /// Returns None for variants that don't carry provider context.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::RateLimited { provider } => Some(provider),
            Self::Upstream { provider, .. } => Some(provider),
            Self::AuthFailed { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// Produce a sanitized error message safe for returning to MCP clients.
    /// Does not leak internal URLs, connection details, or upstream error bodies.
    pub fn user_message(&self) -> String {
        match self {
            Self::ModelNotFound(model) => format!("model not found: {model}"),
            Self::Timeout(ms) => format!("request timed out after {ms}ms"),
            Self::RateLimited { provider } => {
                format!("rate limited by {provider} — try again shortly")
            }
            Self::Upstream { provider, .. } => {
                format!("upstream error from {provider}")
            }
            Self::AuthFailed { provider, .. } => {
                format!("authentication failed for {provider}")
            }
            Self::SchemaParse(_) => {
                "failed to parse provider response".to_string()
            }
            Self::ProcessExit { code, .. } => format!("CLI process exited with code {code}"),
            Self::Request(_) => "request to provider failed".to_string(),
            Self::Other(_) => "an error occurred".to_string(),
        }
    }
}
