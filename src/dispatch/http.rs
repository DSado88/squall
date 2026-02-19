use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;

use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2MB

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

        let response = self
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
        // Cap error body reads to MAX_RESPONSE_BYTES to prevent memory exhaustion
        if !status.is_success() {
            let error_bytes = response.bytes().await.unwrap_or_default();
            let truncated = &error_bytes[..error_bytes.len().min(MAX_RESPONSE_BYTES)];
            let text = String::from_utf8_lossy(truncated);
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!("{status}: {text}"),
                status: Some(status.as_u16()),
            });
        }

        // Enforce response size limit before parsing
        let bytes = response.bytes().await.map_err(|e| {
            SquallError::Upstream {
                provider: provider.to_string(),
                message: format!("failed to read response body: {e}"),
                status: None,
            }
        })?;

        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!(
                    "response too large: {} bytes (max {})",
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
