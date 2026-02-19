use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;

use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;

pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2MB

pub struct HttpDispatch {
    client: Client,
}

#[derive(Deserialize)]
struct ChatCompletion {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

impl Default for HttpDispatch {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpDispatch {
    pub fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self { client }
    }

    /// Read response body in chunks, stopping at `max_bytes`.
    /// Prevents OOM on chunked-encoding responses that omit Content-Length.
    async fn stream_body_capped(
        response: &mut reqwest::Response,
        max_bytes: usize,
    ) -> Vec<u8> {
        let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
        while let Ok(Some(chunk)) = response.chunk().await {
            body.extend_from_slice(&chunk);
            if body.len() > max_bytes {
                body.truncate(max_bytes);
                break;
            }
        }
        body
    }

    pub async fn query_model(
        &self,
        req: &ProviderRequest,
        provider: &str,
        base_url: &str,
        api_key: &str,
    ) -> Result<ProviderResult, SquallError> {
        let start = Instant::now();

        // Check for expired deadline before making the request
        let timeout = req
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| *d > Duration::from_millis(100))
            .ok_or(SquallError::Timeout(0))?;

        let body = serde_json::json!({
            "model": req.model,
            "messages": [{"role": "user", "content": req.prompt}]
        });

        let mut response = self
            .client
            .post(base_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .timeout(timeout)
            .json(&body)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SquallError::RateLimited {
                provider: provider.to_string(),
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(SquallError::AuthFailed {
                provider: provider.to_string(),
                message: format!("{status}"),
            });
        }

        // Catch-all for any non-success status (4xx, 5xx, 3xx that wasn't followed)
        if !status.is_success() {
            let error_body = Self::stream_body_capped(&mut response, MAX_RESPONSE_BYTES).await;
            let text = String::from_utf8_lossy(&error_body);
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!("{status}: {text}"),
                status: Some(status.as_u16()),
            });
        }

        // Pre-read size guard: fast rejection for responses that declare Content-Length > cap
        if let Some(cl) = response.content_length()
            && cl > MAX_RESPONSE_BYTES as u64
        {
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!(
                    "response too large: {cl} bytes (max {MAX_RESPONSE_BYTES})"
                ),
                status: None,
            });
        }

        // Stream body with size cap — prevents OOM on chunked responses
        // that omit Content-Length. Aborts as soon as limit is exceeded.
        let bytes = Self::stream_body_capped(&mut response, MAX_RESPONSE_BYTES).await;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!(
                    "response too large: >{}B (max {})",
                    bytes.len(),
                    MAX_RESPONSE_BYTES
                ),
                status: None,
            });
        }

        let completion: ChatCompletion =
            serde_json::from_slice(&bytes).map_err(|e| {
                SquallError::SchemaParse(format!("failed to parse response: {e}"))
            })?;

        let text = completion
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| SquallError::Upstream {
                provider: provider.to_string(),
                message: "empty choices or null content".to_string(),
                status: None,
            })?;

        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(ProviderResult {
            text,
            model: req.model.clone(),
            provider: provider.to_string(),
            latency_ms,
        })
    }
}
