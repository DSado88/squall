use thiserror::Error;

#[derive(Debug, Error)]
pub enum SquallError {
    #[error("model not found: {model}")]
    ModelNotFound {
        model: String,
        suggestions: Vec<String>,
    },

    #[error("timeout after {0}ms")]
    Timeout(u64),

    #[error("cancelled after {0}ms")]
    Cancelled(u64),

    #[error("rate limited by {provider}")]
    RateLimited { provider: String },

    #[error("upstream error from {provider}: {message}")]
    Upstream {
        provider: String,
        message: String,
        status: Option<u16>,
    },

    #[error("auth failed for {provider}: {message}")]
    AuthFailed { provider: String, message: String },

    #[error("schema parse error: {0}")]
    SchemaParse(String),

    #[error("process exited with code {code}: {stderr}")]
    ProcessExit { code: i32, stderr: String },

    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("file context error: {0}")]
    FileContext(String),

    #[error("path escapes base directory: {0}")]
    SymlinkEscape(String),

    #[error("async job failed for {provider}: {message}")]
    AsyncJobFailed { provider: String, message: String },

    #[error("poll failed for {provider} job {job_id}: {message}")]
    PollFailed {
        provider: String,
        job_id: String,
        message: String,
    },

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
            Self::AsyncJobFailed { provider, .. } => Some(provider),
            Self::PollFailed { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// Returns true for transient errors that may succeed on retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } => true,
            Self::Timeout(_) => true,
            Self::Upstream { status, .. } => {
                // 5xx = server error (retryable), 4xx = client error (not retryable)
                // status: None = ambiguous (not from HTTP) → safe default: NOT retryable
                status.is_some_and(|s| s >= 500)
            }
            Self::Request(_) => true, // connection errors may be transient
            Self::PollFailed { .. } => true, // transient poll failure
            _ => false,
        }
    }

    /// Produce a sanitized error message safe for returning to MCP clients.
    /// Does not leak internal URLs, connection details, or upstream error bodies.
    pub fn user_message(&self) -> String {
        match self {
            Self::ModelNotFound { model, suggestions } => {
                if suggestions.is_empty() {
                    format!("model not found: {model}")
                } else {
                    format!(
                        "model not found: {model}. Did you mean: {}?",
                        suggestions.join(", ")
                    )
                }
            }
            Self::Timeout(ms) => format!("request timed out after {ms}ms"),
            Self::Cancelled(ms) => format!("cancelled after {ms}ms"),
            Self::RateLimited { provider } => {
                format!("rate limited by {provider} — try again shortly")
            }
            Self::Upstream {
                provider, message, ..
            } => {
                format!("upstream error from {provider}: {message}")
            }
            Self::AuthFailed { provider, message } => {
                format!("authentication failed for {provider}: {message}")
            }
            Self::SchemaParse(_) => "failed to parse provider response".to_string(),
            Self::ProcessExit { code, stderr } => {
                if stderr.trim().is_empty() {
                    format!("CLI process exited with code {code}")
                } else {
                    // Take tail (last 200 chars) — CLI tools dump banners first,
                    // the actual error is at the end.
                    let preview: String = stderr
                        .chars()
                        .rev()
                        .take(200)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let prefix = if preview.len() < stderr.len() {
                        "..."
                    } else {
                        ""
                    };
                    format!("CLI process exited with code {code}: {prefix}{preview}")
                }
            }
            Self::Request(_) => "request to provider failed".to_string(),
            Self::FileContext(msg) => format!("file context error: {msg}"),
            Self::SymlinkEscape(path) => format!("path escapes sandbox: {path}"),
            Self::AsyncJobFailed { provider, .. } => {
                format!("deep research job failed for {provider}")
            }
            Self::PollFailed { provider, .. } => {
                format!("failed to check research status for {provider}")
            }
            Self::Other(msg) => msg.clone(),
        }
    }
}
