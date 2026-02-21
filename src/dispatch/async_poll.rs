use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Client;

use crate::dispatch::registry::AsyncPollProviderType;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;

/// Max response body size for poll responses (4MB — research can be large).
const MAX_POLL_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Max consecutive poll failures before giving up.
const MAX_POLL_FAILURES: u32 = 5;

/// Atomic counter for unique persist filenames (same pattern as review.rs).
static PERSIST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Result of polling an async job.
#[derive(Debug)]
pub enum PollStatus {
    /// Job is still running.
    InProgress,
    /// Job completed successfully with result text.
    Completed(String),
    /// Job failed with error message.
    Failed(String),
}

/// Provider-specific request/response handling for async-poll APIs.
pub trait AsyncPollApi: Send + Sync {
    /// Build the launch request. Returns (url, headers, body).
    fn build_launch_request(
        &self,
        prompt: &str,
        model: &str,
        api_key: &str,
        system_prompt: Option<&str>,
    ) -> (String, Vec<(String, String)>, serde_json::Value);

    /// Build the poll request. Returns (url, headers).
    fn build_poll_request(
        &self,
        job_id: &str,
        api_key: &str,
    ) -> (String, Vec<(String, String)>);

    /// Parse the launch response to extract job ID.
    fn parse_launch_response(&self, body: &[u8]) -> Result<String, SquallError>;

    /// Parse the poll response to determine status.
    fn parse_poll_response(&self, body: &[u8]) -> Result<PollStatus, SquallError>;

    /// Recommended base interval between polls.
    fn poll_interval(&self) -> Duration;

    /// Maximum poll interval (backoff cap).
    fn max_poll_interval(&self) -> Duration;
}

// ---------------------------------------------------------------------------
// OpenAI Responses API (o3-deep-research, o4-mini-deep-research)
// ---------------------------------------------------------------------------

pub struct OpenAiResponsesApi;

impl AsyncPollApi for OpenAiResponsesApi {
    fn build_launch_request(
        &self,
        prompt: &str,
        model: &str,
        api_key: &str,
        system_prompt: Option<&str>,
    ) -> (String, Vec<(String, String)>, serde_json::Value) {
        let url = "https://api.openai.com/v1/responses".to_string();
        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {api_key}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let mut input = Vec::new();
        if let Some(sys) = system_prompt {
            input.push(serde_json::json!({"role": "developer", "content": sys}));
        }
        input.push(serde_json::json!({"role": "user", "content": prompt}));

        let body = serde_json::json!({
            "model": model,
            "input": input,
            "tools": [{"type": "web_search_preview"}],
            "background": true,
            "store": true,
        });

        (url, headers, body)
    }

    fn build_poll_request(
        &self,
        job_id: &str,
        api_key: &str,
    ) -> (String, Vec<(String, String)>) {
        let url = format!("https://api.openai.com/v1/responses/{job_id}");
        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {api_key}")),
        ];
        (url, headers)
    }

    fn parse_launch_response(&self, body: &[u8]) -> Result<String, SquallError> {
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| SquallError::SchemaParse(format!("OpenAI launch response: {e}")))?;
        v["id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| SquallError::SchemaParse("OpenAI launch response missing 'id'".into()))
    }

    fn parse_poll_response(&self, body: &[u8]) -> Result<PollStatus, SquallError> {
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| SquallError::SchemaParse(format!("OpenAI poll response: {e}")))?;

        match v["status"].as_str() {
            Some("queued" | "in_progress") => Ok(PollStatus::InProgress),
            Some("completed") => {
                let text = v["output_text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                Ok(PollStatus::Completed(text))
            }
            Some(status @ ("failed" | "incomplete" | "cancelled")) => {
                Ok(PollStatus::Failed(format!("job {status}")))
            }
            Some(other) => Ok(PollStatus::Failed(format!("unknown status: {other}"))),
            None => Err(SquallError::SchemaParse(
                "OpenAI poll response missing 'status'".into(),
            )),
        }
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn max_poll_interval(&self) -> Duration {
        Duration::from_secs(60)
    }
}

// ---------------------------------------------------------------------------
// Gemini Interactions API (deep-research-pro-preview-12-2025)
// ---------------------------------------------------------------------------

pub struct GeminiInteractionsApi;

impl AsyncPollApi for GeminiInteractionsApi {
    fn build_launch_request(
        &self,
        prompt: &str,
        model: &str,
        api_key: &str,
        system_prompt: Option<&str>,
    ) -> (String, Vec<(String, String)>, serde_json::Value) {
        let url = "https://generativelanguage.googleapis.com/v1beta/interactions".to_string();
        let headers = vec![
            ("x-goog-api-key".to_string(), api_key.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let effective_prompt = match system_prompt {
            Some(sys) => format!("{sys}\n\n{prompt}"),
            None => prompt.to_string(),
        };

        let body = serde_json::json!({
            "agent": model,
            "input": effective_prompt,
            "background": true,
        });

        (url, headers, body)
    }

    fn build_poll_request(
        &self,
        job_id: &str,
        api_key: &str,
    ) -> (String, Vec<(String, String)>) {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/interactions/{job_id}"
        );
        let headers = vec![
            ("x-goog-api-key".to_string(), api_key.to_string()),
        ];
        (url, headers)
    }

    fn parse_launch_response(&self, body: &[u8]) -> Result<String, SquallError> {
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| SquallError::SchemaParse(format!("Gemini launch response: {e}")))?;
        // Gemini returns id like "interactions/abc123" — extract full value
        v["id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| SquallError::SchemaParse("Gemini launch response missing 'id'".into()))
    }

    fn parse_poll_response(&self, body: &[u8]) -> Result<PollStatus, SquallError> {
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| SquallError::SchemaParse(format!("Gemini poll response: {e}")))?;

        match v["status"].as_str() {
            Some("in_progress") => Ok(PollStatus::InProgress),
            Some("completed") => {
                // Result is in outputs array, last item's text field
                let text = v["outputs"]
                    .as_array()
                    .and_then(|arr| arr.last())
                    .and_then(|item| item["text"].as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(PollStatus::Completed(text))
            }
            Some(status @ ("failed" | "cancelled")) => {
                let msg = v["error"]
                    .as_str()
                    .unwrap_or(status);
                Ok(PollStatus::Failed(msg.to_string()))
            }
            Some(other) => Ok(PollStatus::Failed(format!("unknown status: {other}"))),
            None => Err(SquallError::SchemaParse(
                "Gemini poll response missing 'status'".into(),
            )),
        }
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(45)
    }

    fn max_poll_interval(&self) -> Duration {
        Duration::from_secs(120)
    }
}

// ---------------------------------------------------------------------------
// Async Poll Dispatcher
// ---------------------------------------------------------------------------

pub struct AsyncPollDispatch {
    client: Client,
}

impl Default for AsyncPollDispatch {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncPollDispatch {
    pub fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("failed to build async-poll HTTP client");
        Self { client }
    }

    fn api_for(provider_type: &AsyncPollProviderType) -> Box<dyn AsyncPollApi> {
        match provider_type {
            AsyncPollProviderType::OpenAiResponses => Box::new(OpenAiResponsesApi),
            AsyncPollProviderType::GeminiInteractions => Box::new(GeminiInteractionsApi),
        }
    }

    /// Compute next poll delay with exponential backoff: base × 1.5^attempt, capped.
    pub fn next_poll_delay(api: &dyn AsyncPollApi, attempt: u32) -> Duration {
        let base = api.poll_interval();
        let max = api.max_poll_interval();
        let delay = base.mul_f64(1.5_f64.powi(attempt as i32));
        delay.min(max)
    }

    pub async fn query_model(
        &self,
        req: &ProviderRequest,
        provider: &str,
        provider_type: &AsyncPollProviderType,
        api_key: &str,
    ) -> Result<ProviderResult, SquallError> {
        let api = Self::api_for(provider_type);
        let start = Instant::now();

        // Check deadline has enough time
        let remaining = req
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or(SquallError::Timeout(0))?;
        if remaining < Duration::from_secs(5) {
            return Err(SquallError::Timeout(remaining.as_millis() as u64));
        }

        // 1. Launch the job
        let (launch_url, launch_headers, launch_body) =
            api.build_launch_request(&req.prompt, &req.model, api_key, req.system_prompt.as_deref());

        let mut launch_req = self.client.post(&launch_url);
        for (k, v) in &launch_headers {
            launch_req = launch_req.header(k, v);
        }

        let launch_timeout = remaining.min(Duration::from_secs(30));
        let launch_resp = tokio::time::timeout(launch_timeout, async {
            launch_req
                .json(&launch_body)
                .send()
                .await
                .map_err(SquallError::Request)
        })
        .await
        .map_err(|_| SquallError::Timeout(start.elapsed().as_millis() as u64))??;

        let launch_status = launch_resp.status();
        if launch_status == reqwest::StatusCode::UNAUTHORIZED
            || launch_status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(SquallError::AuthFailed {
                provider: provider.to_string(),
                message: format!("HTTP {launch_status}"),
            });
        }
        if launch_status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SquallError::RateLimited {
                provider: provider.to_string(),
            });
        }
        if !launch_status.is_success() {
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: format!("launch failed with HTTP {launch_status}"),
                status: Some(launch_status.as_u16()),
            });
        }

        let launch_body_bytes = launch_resp
            .bytes()
            .await
            .map_err(SquallError::Request)?;
        let job_id = api.parse_launch_response(&launch_body_bytes)?;

        tracing::info!(
            provider = provider,
            model = req.model,
            job_id = job_id,
            "async-poll job launched"
        );

        // 2. Poll loop
        let mut attempt: u32 = 0;
        let mut consecutive_failures: u32 = 0;

        loop {
            let delay = Self::next_poll_delay(&*api, attempt);

            // Check if we have time for another poll
            let remaining = req
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| SquallError::Timeout(start.elapsed().as_millis() as u64))?;

            if remaining < delay {
                // Not enough time for another poll — timeout
                return Err(SquallError::Timeout(start.elapsed().as_millis() as u64));
            }

            tokio::time::sleep(delay).await;
            attempt += 1;

            // Recalculate remaining AFTER sleep to prevent deadline drift.
            // Without this, poll_timeout uses the stale pre-sleep value and
            // can overrun the deadline by up to (delay - actual_remaining).
            let remaining = req
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| SquallError::Timeout(start.elapsed().as_millis() as u64))?;

            let (poll_url, poll_headers) = api.build_poll_request(&job_id, api_key);

            let mut poll_req = self.client.get(&poll_url);
            for (k, v) in &poll_headers {
                poll_req = poll_req.header(k, v);
            }

            let poll_timeout = remaining.min(Duration::from_secs(30));
            let poll_result = tokio::time::timeout(poll_timeout, async {
                poll_req.send().await.map_err(SquallError::Request)
            })
            .await;

            let poll_resp = match poll_result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    consecutive_failures += 1;
                    tracing::warn!(
                        provider = provider,
                        job_id = job_id,
                        attempt = attempt,
                        failures = consecutive_failures,
                        "poll request failed: {e}"
                    );
                    if consecutive_failures >= MAX_POLL_FAILURES {
                        return Err(SquallError::PollFailed {
                            provider: provider.to_string(),
                            job_id: job_id.clone(),
                            message: format!("{consecutive_failures} consecutive failures: {e}"),
                        });
                    }
                    continue;
                }
                Err(_) => {
                    consecutive_failures += 1;
                    tracing::warn!(
                        provider = provider,
                        job_id = job_id,
                        attempt = attempt,
                        failures = consecutive_failures,
                        "poll request timed out"
                    );
                    if consecutive_failures >= MAX_POLL_FAILURES {
                        return Err(SquallError::Timeout(start.elapsed().as_millis() as u64));
                    }
                    continue;
                }
            };

            // Auth failures during poll are not transient — fail fast
            if poll_resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || poll_resp.status() == reqwest::StatusCode::FORBIDDEN
            {
                return Err(SquallError::AuthFailed {
                    provider: provider.to_string(),
                    message: format!("poll HTTP {}", poll_resp.status()),
                });
            }

            // Handle rate limiting on poll
            if poll_resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                consecutive_failures += 1;
                tracing::warn!(
                    provider = provider,
                    job_id = job_id,
                    "poll rate limited, backing off"
                );
                if consecutive_failures >= MAX_POLL_FAILURES {
                    return Err(SquallError::RateLimited {
                        provider: provider.to_string(),
                    });
                }
                continue;
            }

            if !poll_resp.status().is_success() {
                consecutive_failures += 1;
                tracing::warn!(
                    provider = provider,
                    job_id = job_id,
                    status = poll_resp.status().as_u16(),
                    "poll returned non-success status"
                );
                if consecutive_failures >= MAX_POLL_FAILURES {
                    return Err(SquallError::PollFailed {
                        provider: provider.to_string(),
                        job_id: job_id.clone(),
                        message: format!("HTTP {}", poll_resp.status()),
                    });
                }
                continue;
            }

            // Reset failure counter on successful response
            consecutive_failures = 0;

            let poll_body = poll_resp
                .bytes()
                .await
                .map_err(SquallError::Request)?;

            if poll_body.len() > MAX_POLL_RESPONSE_BYTES {
                return Err(SquallError::Upstream {
                    provider: provider.to_string(),
                    message: format!("poll response too large: {} bytes", poll_body.len()),
                    status: None,
                });
            }

            match api.parse_poll_response(&poll_body)? {
                PollStatus::InProgress => {
                    tracing::debug!(
                        provider = provider,
                        job_id = job_id,
                        attempt = attempt,
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        "job still in progress"
                    );
                    continue;
                }
                PollStatus::Completed(text) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    tracing::info!(
                        provider = provider,
                        model = req.model,
                        job_id = job_id,
                        elapsed_ms = elapsed_ms,
                        "async-poll job completed"
                    );

                    // Persist to disk
                    let file_path = persist_research_result(
                        &req.model,
                        provider,
                        &text,
                        &job_id,
                        elapsed_ms,
                    )
                    .await;

                    let response_text = match file_path {
                        Ok(path) => format!("{text}\n\n---\nFull result persisted to: {path}"),
                        Err(e) => {
                            tracing::warn!("failed to persist research result: {e}");
                            text
                        }
                    };

                    return Ok(ProviderResult {
                        text: response_text,
                        model: req.model.clone(),
                        provider: provider.to_string(),
                    });
                }
                PollStatus::Failed(msg) => {
                    return Err(SquallError::AsyncJobFailed {
                        provider: provider.to_string(),
                        message: msg,
                    });
                }
            }
        }
    }
}

/// Sanitize a model name for use in filenames. Only allows alphanumeric, `-`, `_`.
pub fn sanitize_model_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Persist deep research results to `.squall/research/{timestamp}_{seq}_{model}.json`.
async fn persist_research_result(
    model: &str,
    provider: &str,
    text: &str,
    job_id: &str,
    elapsed_ms: u64,
) -> Result<String, std::io::Error> {
    let dir = std::path::PathBuf::from(".squall/research");
    tokio::fs::create_dir_all(&dir).await?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = PERSIST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe_model = sanitize_model_name(model);
    let filename = format!("{ts}_{seq}_{safe_model}.json");
    let path = dir.join(&filename);

    let payload = serde_json::json!({
        "model": model,
        "provider": provider,
        "job_id": job_id,
        "elapsed_ms": elapsed_ms,
        "text": text,
    });

    let json = serde_json::to_string_pretty(&payload)
        .map_err(std::io::Error::other)?;

    // Atomic write: temp file + rename prevents partial reads
    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, json.as_bytes()).await?;
    if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(format!(".squall/research/{filename}"))
}
