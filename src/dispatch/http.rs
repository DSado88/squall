use std::time::{Duration, Instant};

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;

use crate::dispatch::registry::ApiFormat;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;

pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2MB

/// Default duration without any SSE chunk before returning partial result.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Extended stall timeout for reasoning models that think silently.
const REASONING_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Returns the stall timeout for a given reasoning effort level.
/// Reasoning models with effort >= "medium" get an extended timeout
/// because they may think silently (no SSE chunks) for minutes.
pub fn stall_timeout_for(reasoning_effort: Option<&str>) -> Duration {
    match reasoning_effort {
        Some("medium" | "high" | "xhigh") => REASONING_STALL_TIMEOUT,
        _ => STALL_TIMEOUT,
    }
}

/// Default maximum time to wait for the first SSE event after headers arrive.
/// 60s accommodates OpenRouter-routed models (Kimi, GLM) where the intermediary
/// responds with headers immediately but the upstream model queues for 30-50s.
/// SSE keepalive comments (`: keepalive`) are dropped by eventsource-stream,
/// so they can't reset this timer — only real data events count.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(60);

/// Returns the first-byte timeout for a given reasoning effort level.
/// Reasoning models with effort >= "medium" may think silently for minutes
/// before emitting their first token — use the extended stall timeout.
pub fn first_byte_timeout_for(reasoning_effort: Option<&str>) -> Duration {
    match reasoning_effort {
        Some("medium" | "high" | "xhigh") => REASONING_STALL_TIMEOUT,
        _ => FIRST_BYTE_TIMEOUT,
    }
}

/// Maximum time to wait for response headers after sending the request.
pub const HEADERS_TIMEOUT: Duration = Duration::from_secs(60);

pub struct HttpDispatch {
    client: Client,
}

/// SSE streaming chunk from OpenAI chat completions API.
#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
    /// xAI Grok sends reasoning in a separate field alongside content.
    reasoning_content: Option<String>,
}

/// SSE streaming event from Anthropic Messages API.
#[derive(Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicDelta>,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
}

/// Result of parsing a single SSE event.
enum ParsedChunk {
    /// Text content to accumulate.
    Text(String),
    /// Stream is complete.
    Done,
    /// Non-content event (keepalive, metadata) — skip.
    Skip,
}

#[allow(clippy::new_without_default)]
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
    /// Used only for error response bodies (non-SSE).
    async fn stream_body_capped(
        response: &mut reqwest::Response,
        max_bytes: usize,
    ) -> Result<Vec<u8>, reqwest::Error> {
        let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let remaining = (max_bytes + 1).saturating_sub(body.len());
                    let to_copy = chunk.len().min(remaining);
                    body.extend_from_slice(&chunk[..to_copy]);
                    if body.len() > max_bytes {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(body)
    }

    pub async fn query_model(
        &self,
        req: &ProviderRequest,
        provider: &str,
        base_url: &str,
        api_key: &str,
        api_format: &ApiFormat,
    ) -> Result<ProviderResult, SquallError> {
        let start = Instant::now();

        // Check for expired deadline before making the request
        let remaining = req
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| *d > Duration::from_millis(100))
            .ok_or(SquallError::Timeout(0))?;

        let (body, request_builder) = match api_format {
            ApiFormat::OpenAi => {
                let mut messages = Vec::new();
                if let Some(ref system) = req.system_prompt {
                    messages.push(serde_json::json!({"role": "system", "content": system}));
                }
                messages.push(serde_json::json!({"role": "user", "content": req.prompt}));

                let mut body = serde_json::json!({
                    "model": req.model,
                    "messages": messages,
                    "stream": true,
                });
                if let Some(temp) = req.temperature {
                    body["temperature"] = serde_json::json!(temp);
                }
                if let Some(max) = req.max_tokens {
                    body["max_tokens"] = serde_json::json!(max);
                }
                if let Some(ref effort) = req.reasoning_effort {
                    body["reasoning"] = serde_json::json!({"effort": effort});
                }

                let builder = self
                    .client
                    .post(base_url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Content-Type", "application/json");
                (body, builder)
            }
            ApiFormat::Anthropic => {
                let messages =
                    vec![serde_json::json!({"role": "user", "content": req.prompt})];

                let mut body = serde_json::json!({
                    "model": req.model,
                    "messages": messages,
                    "stream": true,
                    "max_tokens": req.max_tokens.unwrap_or(16384),
                });
                if let Some(ref system) = req.system_prompt {
                    body["system"] = serde_json::json!(system);
                }
                if let Some(temp) = req.temperature {
                    body["temperature"] = serde_json::json!(temp);
                }

                let builder = self
                    .client
                    .post(base_url)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("Content-Type", "application/json");
                (body, builder)
            }
        };

        // [FIX #2] Scoped timeout around send() only — prevents hanging on headers.
        // Client-level connect_timeout(10s) handles TCP/TLS; this covers the gap
        // between connection and first response header.
        let headers_timeout = remaining.min(HEADERS_TIMEOUT);
        let send_future = request_builder.json(&body).send();

        let mut response = tokio::time::timeout(headers_timeout, send_future)
            .await
            .map_err(|_| SquallError::Timeout(start.elapsed().as_millis() as u64))?
            .map_err(SquallError::from)?;

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

        // Catch-all for any non-success status (4xx, 5xx, 3xx that wasn't followed).
        // Error responses are not SSE — read body with deadline to prevent hanging
        // on stalled upstream that sends headers but withholds body.
        if !status.is_success() {
            let body_timeout = req
                .deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::from_secs(5))
                .min(Duration::from_secs(5)); // at most 5s, never exceed deadline
            let error_body = tokio::time::timeout(
                body_timeout,
                Self::stream_body_capped(&mut response, MAX_RESPONSE_BYTES),
            )
            .await
            .unwrap_or(Ok(Vec::new())) // timeout → treat as empty body
            .unwrap_or_default(); // reqwest error → treat as empty body
            let text = String::from_utf8_lossy(&error_body);
            let truncated: String = text.chars().take(500).collect();
            let message = if truncated.len() < text.len() {
                format!("{status}: {truncated}... [{} bytes total]", error_body.len())
            } else {
                format!("{status}: {truncated}")
            };
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message,
                status: Some(status.as_u16()),
            });
        }

        // Success — read SSE stream
        self.read_sse_stream(response, req, provider, start, api_format)
            .await
    }

    /// Read SSE streaming response, accumulating text chunks.
    ///
    /// Returns partial result on cancellation, deadline, or stall timeout.
    /// Five timeout layers protect against different failure modes:
    /// - Connect (10s, client-level): dead endpoints, DNS hangs
    /// - Headers (30s, scoped around send()): server hangs on response
    /// - First-byte (30s/300s for reasoning, select! branch): model queued but not generating
    /// - Stall (60s/300s for reasoning, select! branch): model stopped mid-stream
    /// - Generation (MCP deadline, select! branch): slow model
    async fn read_sse_stream(
        &self,
        response: reqwest::Response,
        req: &ProviderRequest,
        provider: &str,
        start: Instant,
        api_format: &ApiFormat,
    ) -> Result<ProviderResult, SquallError> {
        let mut stream = response.bytes_stream().eventsource();
        let mut accumulated = String::new();

        // [FIX #3] Safe Instant conversion: compute remaining duration from std::time::Instant,
        // then add to tokio::time::Instant. Never cast across clock domains.
        let remaining = req
            .deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        let generation_deadline = tokio::time::Instant::now() + remaining;

        // Stall timeout: use explicit override if set, otherwise fall back to reasoning-based logic.
        // Clamp to remaining deadline — stall timer must never outlive the task.
        let stall_timeout = req
            .stall_timeout
            .unwrap_or_else(|| stall_timeout_for(req.reasoning_effort.as_deref()))
            .min(remaining);

        // First-byte deadline: use stall_timeout override if set (applies to pre-first-token
        // silence too), otherwise use reasoning-based first-byte logic.
        let first_byte_timeout = req
            .stall_timeout
            .map(|st| st.min(remaining))
            .unwrap_or_else(|| first_byte_timeout_for(req.reasoning_effort.as_deref()));
        let first_byte_deadline = tokio::time::Instant::now() + first_byte_timeout;

        // Cancel future: resolves on cooperative cancel, or pends forever if None
        let cancel = req.cancellation_token.clone();
        let cancel_fut = async {
            match &cancel {
                Some(t) => t.cancelled().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(cancel_fut);

        let mut received_first = false;
        let mut last_chunk_at = tokio::time::Instant::now();

        // Pin the deadline sleep outside the loop — reset() reuses the timer
        // entry instead of allocating a new Sleep future every iteration.
        let initial_deadline = generation_deadline.min(first_byte_deadline);
        let deadline_sleep = tokio::time::sleep_until(initial_deadline);
        tokio::pin!(deadline_sleep);

        loop {
            // Effective deadline: generation deadline + stall/first-byte guard
            let effective_deadline = if received_first {
                generation_deadline.min(last_chunk_at + stall_timeout)
            } else {
                generation_deadline.min(first_byte_deadline)
            };
            deadline_sleep.as_mut().reset(effective_deadline);

            // [FIX #1] No `biased;` — prevents cancel/deadline starvation when
            // stream is continuously ready (token burst).
            tokio::select! {
                _ = &mut cancel_fut => {
                    // [FIX #4] Empty on cancel = error, not partial success
                    if accumulated.is_empty() {
                        return Err(SquallError::Cancelled(start.elapsed().as_millis() as u64));
                    }
                    return Ok(ProviderResult {
                        text: accumulated,
                        partial: true,
                        model: req.model.clone(),
                        provider: provider.to_string(),
                    });
                }
                _ = &mut deadline_sleep => {
                    if accumulated.is_empty() {
                        return Err(SquallError::Timeout(start.elapsed().as_millis() as u64));
                    }
                    return Ok(ProviderResult {
                        text: accumulated,
                        partial: true,
                        model: req.model.clone(),
                        provider: provider.to_string(),
                    });
                }
                event = stream.next() => match event {
                    Some(Ok(ev)) => {
                        match parse_sse_event(&ev.data, api_format) {
                            ParsedChunk::Done => break,
                            ParsedChunk::Text(text) => {
                                received_first = true;
                                last_chunk_at = tokio::time::Instant::now();
                                if accumulated.len() + text.len() > MAX_RESPONSE_BYTES {
                                    return Err(SquallError::Upstream {
                                        provider: provider.to_string(),
                                        message: format!(
                                            "streaming response too large: >{}B",
                                            MAX_RESPONSE_BYTES
                                        ),
                                        status: None,
                                    });
                                }
                                accumulated.push_str(&text);
                            }
                            ParsedChunk::Skip => {
                                // Any SSE data event (even non-text) proves the server
                                // is alive: switch from first-byte to stall timer, and
                                // reset the stall timer to prevent false timeouts.
                                received_first = true;
                                last_chunk_at = tokio::time::Instant::now();
                            }
                        }
                    }
                    Some(Err(e)) => {
                        if accumulated.is_empty() {
                            tracing::warn!(provider, "SSE stream error with no data: {e}");
                            return Err(SquallError::Other(format!(
                                "SSE stream error from {provider}"
                            )));
                        }
                        // Have partial data — return it
                        tracing::warn!(
                            provider,
                            bytes = accumulated.len(),
                            "SSE stream error after partial data: {e}"
                        );
                        return Ok(ProviderResult {
                            text: accumulated,
                            partial: true,
                            model: req.model.clone(),
                            provider: provider.to_string(),
                        });
                    }
                    None => {
                        // Stream ended without [DONE] — this is an incomplete response.
                        if accumulated.is_empty() {
                            return Err(SquallError::Upstream {
                                provider: provider.to_string(),
                                message: "stream ended without [DONE] marker".to_string(),
                                status: None,
                            });
                        }
                        tracing::warn!(
                            provider,
                            bytes = accumulated.len(),
                            "SSE stream ended without [DONE] marker"
                        );
                        return Ok(ProviderResult {
                            text: accumulated,
                            partial: true,
                            model: req.model.clone(),
                            provider: provider.to_string(),
                        });
                    }
                },
            }
        }

        if accumulated.is_empty() {
            return Err(SquallError::Upstream {
                provider: provider.to_string(),
                message: "empty streaming response".to_string(),
                status: None,
            });
        }

        Ok(ProviderResult {
            text: accumulated,
            partial: false,
            model: req.model.clone(),
            provider: provider.to_string(),
        })
    }
}

/// Parse a single SSE event according to the API format.
fn parse_sse_event(data: &str, api_format: &ApiFormat) -> ParsedChunk {
    match api_format {
        ApiFormat::OpenAi => parse_openai_event(data),
        ApiFormat::Anthropic => parse_anthropic_event(data),
    }
}

/// Parse an OpenAI chat completions SSE event.
fn parse_openai_event(data: &str) -> ParsedChunk {
    if data.trim() == "[DONE]" {
        return ParsedChunk::Done;
    }

    let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) else {
        return ParsedChunk::Skip;
    };
    let Some(choice) = chunk.choices.first() else {
        return ParsedChunk::Skip;
    };

    let mut text = String::new();
    if let Some(ref rc) = choice.delta.reasoning_content
        && !rc.is_empty()
    {
        text.push_str(rc);
    }
    if let Some(ref c) = choice.delta.content
        && !c.is_empty()
    {
        text.push_str(c);
    }

    if text.is_empty() {
        ParsedChunk::Skip
    } else {
        ParsedChunk::Text(text)
    }
}

/// Parse an Anthropic Messages API SSE event.
fn parse_anthropic_event(data: &str) -> ParsedChunk {
    let Ok(event) = serde_json::from_str::<AnthropicEvent>(data) else {
        return ParsedChunk::Skip;
    };

    match event.event_type.as_str() {
        "message_stop" => ParsedChunk::Done,
        "content_block_delta" => {
            if let Some(delta) = &event.delta
                && delta.delta_type.as_deref() == Some("text_delta")
                && let Some(text) = &delta.text
                && !text.is_empty()
            {
                ParsedChunk::Text(text.clone())
            } else {
                ParsedChunk::Skip
            }
        }
        _ => ParsedChunk::Skip,
    }
}
