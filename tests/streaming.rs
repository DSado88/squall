//! Tests for SSE streaming HTTP dispatch and cooperative review cancellation.

use squall::dispatch::http::HttpDispatch;
use squall::dispatch::registry::ApiFormat;
use squall::dispatch::ProviderRequest;
use squall::error::SquallError;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use squall::dispatch::http::{first_byte_timeout_for, stall_timeout_for, HEADERS_TIMEOUT};

/// Helper: bind a TCP listener on localhost and return (listener, port).
async fn mock_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Helper: format an SSE data event from a content string.
fn sse_chunk(content: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\n"
    )
}

/// Helper: format an SSE data event with reasoning_content.
fn sse_reasoning_chunk(reasoning: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"reasoning_content\":\"{reasoning}\"}}}}]}}\n\n"
    )
}

const SSE_HEADERS: &[u8] = b"HTTP/1.1 200 OK\r\n\
    Content-Type: text/event-stream\r\n\
    Connection: close\r\n\r\n";

const SSE_DONE: &[u8] = b"data: [DONE]\n\n";

fn make_req(deadline_secs: u64) -> ProviderRequest {
    ProviderRequest {
        prompt: "test".to_string(),
        model: "test-model".to_string(),
        deadline: Instant::now() + Duration::from_secs(deadline_secs),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    }
}

fn make_req_with_cancel(deadline_secs: u64, token: CancellationToken) -> ProviderRequest {
    ProviderRequest {
        prompt: "test".to_string(),
        model: "test-model".to_string(),
        deadline: Instant::now() + Duration::from_secs(deadline_secs),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: Some(token),
        stall_timeout: None,
    }
}

// ---------------------------------------------------------------------------
// Complete SSE streaming response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_complete_response() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("Hello ").as_bytes()).await.unwrap();
        socket.write_all(sse_chunk("world!").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "Hello world!");
    assert!(!result.partial);
    assert_eq!(result.model, "test-model");

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Partial result on deadline expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_partial_on_deadline() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("chunk1 ").as_bytes()).await.unwrap();
        socket.write_all(sse_chunk("chunk2 ").as_bytes()).await.unwrap();
        // Wait longer than deadline — simulates slow model
        tokio::time::sleep(Duration::from_secs(10)).await;
        socket.write_all(sse_chunk("never").as_bytes()).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(2); // 2s deadline

    let start = Instant::now();
    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert!(result.partial, "Should be partial");
    assert_eq!(result.text, "chunk1 chunk2 ");
    assert!(start.elapsed() < Duration::from_secs(5));

    server.abort();
}

// ---------------------------------------------------------------------------
// Partial result on cooperative cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_partial_on_cancellation() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("partial ").as_bytes()).await.unwrap();
        socket.write_all(sse_chunk("data").as_bytes()).await.unwrap();
        // Hold connection open
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let token = CancellationToken::new();
    let dispatch = HttpDispatch::new();
    let req = make_req_with_cancel(30, token.clone());

    // Cancel after 1 second
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        token.cancel();
    });

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert!(result.partial, "Should be partial on cancellation");
    assert_eq!(result.text, "partial data");

    cancel_handle.await.unwrap();
    server.abort();
}

// ---------------------------------------------------------------------------
// First-byte timeout (server hangs after headers)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_first_byte_timeout() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Send nothing — model queued but not generating
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let dispatch = HttpDispatch::new();
    // Short deadline — will hit the deadline before first-byte timeout
    let req = make_req(2);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SquallError::Timeout(_)),
        "Expected Timeout, got: {err:?}"
    );

    server.abort();
}

// ---------------------------------------------------------------------------
// Empty [DONE] immediately
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_empty_done() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SquallError::Upstream { .. }),
        "Expected Upstream error for empty stream, got: {err:?}"
    );

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Network error after partial data → returns partial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_network_error_with_partial_data() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("saved ").as_bytes()).await.unwrap();
        socket.write_all(sse_chunk("data").as_bytes()).await.unwrap();
        // Drop connection abruptly (no [DONE])
        drop(socket);
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    // Stream ends without [DONE] → incomplete, returned as partial
    assert_eq!(result.text, "saved data");
    assert!(result.partial, "Stream without [DONE] should be partial");

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Network error with no data → returns error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_network_error_no_data() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Drop immediately (no events at all)
        drop(socket);
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    assert!(result.is_err(), "Empty stream should be an error");

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Reasoning content (xAI Grok)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_reasoning_content() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Reasoning comes first (thinking phase)
        socket
            .write_all(sse_reasoning_chunk("thinking...").as_bytes())
            .await
            .unwrap();
        // Then content
        socket
            .write_all(sse_chunk("answer").as_bytes())
            .await
            .unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "thinking...answer");
    assert!(!result.partial);

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Cancellation with no data → returns Cancelled error (not Timeout)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_cancel_empty_returns_cancelled() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Hold connection but send nothing
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let token = CancellationToken::new();
    let dispatch = HttpDispatch::new();
    let req = make_req_with_cancel(30, token.clone());

    // Cancel immediately (no data received yet)
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        token.cancel();
    });

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    assert!(result.is_err(), "Cancel with no data should be error, not partial");
    assert!(
        matches!(result.unwrap_err(), SquallError::Cancelled(_)),
        "Expected Cancelled error, not Timeout"
    );

    cancel_handle.await.unwrap();
    server.abort();
}

// ---------------------------------------------------------------------------
// Stream sends `stream: true` in request body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_request_includes_stream_true() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);

        // Extract JSON body from HTTP request (after blank line)
        let body_start = request.find("\r\n\r\n").unwrap() + 4;
        let body = &request[body_start..];
        assert!(body.contains("\"stream\":true"), "Request body should include stream:true, got: {body}");

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("ok").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let _ = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Unparseable SSE events are silently ignored (keepalives, metadata)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_ignores_unparseable_events() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Keepalive / comment
        socket.write_all(b": keepalive\n\n").await.unwrap();
        // Valid chunk
        socket.write_all(sse_chunk("good").as_bytes()).await.unwrap();
        // Malformed JSON
        socket.write_all(b"data: {not valid json}\n\n").await.unwrap();
        // Another valid chunk
        socket.write_all(sse_chunk(" data").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "good data");
    assert!(!result.partial);

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// SSE stream error does not leak raw error details to user_message()
// ---------------------------------------------------------------------------

#[test]
fn sse_error_message_does_not_leak_internals() {
    // Simulate the Other error as constructed by the SSE error path.
    // The raw library error ({e}) must NOT appear in user_message().
    let err = SquallError::Other(
        "SSE stream error from test-provider".to_string()
    );
    let msg = err.user_message();

    // Should contain provider name
    assert!(msg.contains("test-provider"), "Should mention provider. Got: {msg}");
    // Should NOT contain raw error details (the old format included {e})
    assert!(
        !msg.contains("connection reset"),
        "Should not leak raw error details. Got: {msg}"
    );
    assert!(
        !msg.contains("http://"),
        "Should not leak URLs. Got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// HEADERS_TIMEOUT must be >= 60s for large prompts
// ---------------------------------------------------------------------------

#[test]
fn headers_timeout_is_at_least_60s() {
    assert!(
        HEADERS_TIMEOUT >= Duration::from_secs(60),
        "HEADERS_TIMEOUT must be >= 60s for large prompts (review tool context). Got: {HEADERS_TIMEOUT:?}"
    );
}

// ---------------------------------------------------------------------------
// Stall timeout extends for reasoning models
// ---------------------------------------------------------------------------

#[test]
fn stall_timeout_default_is_60s() {
    assert_eq!(stall_timeout_for(None), Duration::from_secs(60));
    assert_eq!(stall_timeout_for(Some("none")), Duration::from_secs(60));
}

#[test]
fn stall_timeout_extends_for_reasoning() {
    // "low" keeps default — model produces output quickly
    assert_eq!(stall_timeout_for(Some("low")), Duration::from_secs(60));
    // "medium"/"high"/"xhigh" — model may think silently for minutes
    assert!(stall_timeout_for(Some("medium")) >= Duration::from_secs(300));
    assert!(stall_timeout_for(Some("high")) >= Duration::from_secs(300));
    assert!(stall_timeout_for(Some("xhigh")) >= Duration::from_secs(300));
}

// ---------------------------------------------------------------------------
// Reasoning model survives silent thinking (stall timeout extended)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_reasoning_model_survives_stall() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Send one chunk, then go silent for 3s (simulating thinking)
        socket.write_all(sse_chunk("thinking...").as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        socket.write_all(sse_chunk(" done!").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    // reasoning_effort = "high" → stall timeout should be >= 300s, not 60s
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test-model".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: Some("high".to_string()),
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "thinking... done!");
    assert!(!result.partial);

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// First-byte timeout extends for reasoning models
// ---------------------------------------------------------------------------

#[test]
fn first_byte_timeout_default_is_60s() {
    // 60s accommodates OpenRouter-routed models (Kimi, GLM) that queue >30s.
    assert_eq!(first_byte_timeout_for(None), Duration::from_secs(60));
    assert_eq!(first_byte_timeout_for(Some("none")), Duration::from_secs(60));
    assert_eq!(first_byte_timeout_for(Some("low")), Duration::from_secs(60));
}

#[test]
fn first_byte_timeout_extends_for_reasoning() {
    // Reasoning models may think silently for minutes before first token
    assert!(first_byte_timeout_for(Some("medium")) >= Duration::from_secs(300));
    assert!(first_byte_timeout_for(Some("high")) >= Duration::from_secs(300));
    assert!(first_byte_timeout_for(Some("xhigh")) >= Duration::from_secs(300));
}

// ---------------------------------------------------------------------------
// Reasoning model survives long silence before first token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_reasoning_model_survives_pre_first_token_silence() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Reasoning model: 3s silence before ANY token (simulates thinking)
        // With FIRST_BYTE_TIMEOUT=30s this passes, but the unit test above
        // proves the function returns >= 300s for "high".
        tokio::time::sleep(Duration::from_secs(3)).await;
        socket.write_all(sse_chunk("answer").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test-model".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: Some("high".to_string()),
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "answer");
    assert!(!result.partial);

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Error body read respects deadline (no indefinite hang on stalled 500)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_error_body_respects_deadline() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        // Send 500 headers, then stall the body forever
        socket
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 10000\r\n\r\n")
            .await
            .unwrap();
        // Hold connection open — never send body
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(3); // 3s deadline

    let start = Instant::now();
    let result = dispatch
        .query_model(&req, "test", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    let elapsed = start.elapsed();

    // Must complete within deadline + margin, NOT hang for 60s
    assert!(
        elapsed < Duration::from_secs(8),
        "Error body read should respect deadline, took {elapsed:?}"
    );
    // Should still return an error (Upstream or Timeout)
    assert!(result.is_err(), "Should be an error, got: {result:?}");

    server.abort();
}

// ===========================================================================
// Phase 3: Anthropic SSE format tests
// ===========================================================================

/// Anthropic content_block_delta events should be parsed correctly.
#[tokio::test]
async fn anthropic_content_block_delta_parsing() {
    let (listener, port) = mock_listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        stream.read(&mut buf).await.unwrap();

        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
            data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n\
            data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n\
            data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
            data: {\"type\":\"message_stop\"}\n\n";
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let http = HttpDispatch::new();
    let req = ProviderRequest {
        model: "claude-opus-4-6".to_string(),
        prompt: "hi".to_string(),
        system_prompt: None,
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        temperature: None,
        max_tokens: Some(1024),
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = http
        .query_model(
            &req,
            "anthropic",
            &format!("http://127.0.0.1:{port}"),
            "test-key",
            &ApiFormat::Anthropic,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Hello world");
    assert!(!result.partial);
    assert_eq!(result.provider, "anthropic");

    server.abort();
}

/// Anthropic stream done on message_stop event.
#[tokio::test]
async fn anthropic_stream_done_on_message_stop() {
    let (listener, port) = mock_listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        stream.read(&mut buf).await.unwrap();

        // Send content then message_stop — should treat as complete (not partial)
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Done\"}}\n\n\
            data: {\"type\":\"message_stop\"}\n\n";
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let http = HttpDispatch::new();
    let req = ProviderRequest {
        model: "claude-opus-4-6".to_string(),
        prompt: "hi".to_string(),
        system_prompt: None,
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        temperature: None,
        max_tokens: Some(1024),
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = http
        .query_model(
            &req,
            "anthropic",
            &format!("http://127.0.0.1:{port}"),
            "test-key",
            &ApiFormat::Anthropic,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Done");
    assert!(!result.partial, "message_stop should mark as complete");

    server.abort();
}

/// Anthropic request should use x-api-key header (not Bearer token).
#[tokio::test]
async fn anthropic_request_uses_x_api_key_header() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);

        // Verify Anthropic headers
        assert!(
            request.contains("x-api-key: sk-test-123"),
            "Should use x-api-key header: {request}"
        );
        assert!(
            request.contains("anthropic-version: 2023-06-01"),
            "Should include anthropic-version: {request}"
        );
        assert!(
            !request.contains("Authorization: Bearer"),
            "Should NOT use Bearer token: {request}"
        );

        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
            data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
            data: {\"type\":\"message_stop\"}\n\n";
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let http = HttpDispatch::new();
    let req = ProviderRequest {
        model: "claude-opus-4-6".to_string(),
        prompt: "test".to_string(),
        system_prompt: None,
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        temperature: None,
        max_tokens: Some(1024),
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = http
        .query_model(
            &req,
            "anthropic",
            &format!("http://127.0.0.1:{port}"),
            "sk-test-123",
            &ApiFormat::Anthropic,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "ok");

    server.abort();
}

// ---------------------------------------------------------------------------
// Together AI reasoning field (Kimi, Qwen thinking tokens)
// ---------------------------------------------------------------------------

/// Helper: format an SSE data event with Together-style reasoning field.
fn sse_together_reasoning_chunk(reasoning: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"\",\"reasoning\":\"{reasoning}\"}}}}]}}\n\n"
    )
}

#[tokio::test]
async fn streaming_together_reasoning_field() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Together/Kimi sends thinking in "reasoning" field with empty "content"
        socket.write_all(sse_together_reasoning_chunk("thinking...").as_bytes()).await.unwrap();
        socket.write_all(sse_together_reasoning_chunk(" done").as_bytes()).await.unwrap();
        // Then actual answer in content field
        socket.write_all(sse_chunk("answer").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "together", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "thinking... doneanswer");
    assert!(!result.partial);

    server.await.unwrap();
}

/// Together-style reasoning-only response (no content field populated).
#[tokio::test]
async fn streaming_together_reasoning_only() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();
        // Kimi K2.5 sends ALL text as reasoning, content always empty
        socket.write_all(sse_together_reasoning_chunk("the answer is 42").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(&req, "together", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await
        .unwrap();

    assert_eq!(result.text, "the answer is 42");
    assert!(!result.partial);

    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// OpenAI max_completion_tokens parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_sends_max_completion_tokens() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);

        let body_start = request.find("\r\n\r\n").unwrap() + 4;
        let body = &request[body_start..];
        assert!(
            body.contains("\"max_completion_tokens\""),
            "OpenAI should use max_completion_tokens, got: {body}"
        );
        assert!(
            !body.contains("\"max_tokens\""),
            "OpenAI should NOT use max_tokens, got: {body}"
        );

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("ok").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "gpt-5".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: Some(1024),
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let _ = dispatch
        .query_model(&req, "openai", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    server.await.unwrap();
}

#[tokio::test]
async fn non_openai_sends_max_tokens() {
    let (listener, port) = mock_listener().await;

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);

        let body_start = request.find("\r\n\r\n").unwrap() + 4;
        let body = &request[body_start..];
        assert!(
            body.contains("\"max_tokens\""),
            "Non-OpenAI should use max_tokens, got: {body}"
        );
        assert!(
            !body.contains("\"max_completion_tokens\""),
            "Non-OpenAI should NOT use max_completion_tokens, got: {body}"
        );

        socket.write_all(SSE_HEADERS).await.unwrap();
        socket.write_all(sse_chunk("ok").as_bytes()).await.unwrap();
        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test-model".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: Some(1024),
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let _ = dispatch
        .query_model(&req, "together", &format!("http://127.0.0.1:{port}/v1/chat"), "fake", &ApiFormat::OpenAi)
        .await;

    server.await.unwrap();
}

/// OpenAI parser should still work after refactor (regression test).
#[tokio::test]
async fn openai_parser_unchanged_after_refactor() {
    let (listener, port) = mock_listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        stream.read(&mut buf).await.unwrap();

        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
            data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
            data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
            data: [DONE]\n\n";
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let http = HttpDispatch::new();
    let req = ProviderRequest {
        model: "grok-test".to_string(),
        prompt: "hi".to_string(),
        system_prompt: None,
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = http
        .query_model(
            &req,
            "xai",
            &format!("http://127.0.0.1:{port}"),
            "test-key",
            &ApiFormat::OpenAi,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Hello world");
    assert!(!result.partial);

    server.abort();
}

// ===========================================================================
// RED tests: bugs found by meta deep review (deep review of deep review)
// ===========================================================================

// ---------------------------------------------------------------------------
// DR2-3: HTTP SSE overflow discards partial results
//
// Bug: src/dispatch/http.rs:404-413 — when accumulated text exceeds
// MAX_RESPONSE_BYTES (2MB), returns hard Err(Upstream) discarding ALL
// accumulated text. But stream errors (lines 425-443) preserve partial
// results via Ok(ProviderResult { partial: true }). Inconsistent behavior:
// overflow should return the accumulated text as partial, not discard it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_overflow_returns_partial_not_error() {
    use squall::dispatch::http::MAX_RESPONSE_BYTES;

    let (listener, port) = mock_listener().await;

    // Send chunks that exceed MAX_RESPONSE_BYTES.
    // First send a large chunk (~1.5MB) that fits, then a chunk that overflows.
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        socket.write_all(SSE_HEADERS).await.unwrap();

        // Send a large chunk (~1.5MB) — fits within 2MB limit
        let big_text = "A".repeat(1_500_000);
        socket
            .write_all(sse_chunk(&big_text).as_bytes())
            .await
            .unwrap();

        // Send another chunk (~1MB) — this pushes total over 2MB
        let overflow_text = "B".repeat(1_000_000);
        socket
            .write_all(sse_chunk(&overflow_text).as_bytes())
            .await
            .unwrap();

        socket.write_all(SSE_DONE).await.unwrap();
    });

    let dispatch = HttpDispatch::new();
    let req = make_req(30);

    let result = dispatch
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}/v1/chat"),
            "fake",
            &ApiFormat::OpenAi,
        )
        .await;

    // BUG: Currently returns Err(Upstream) discarding 1.5MB of accumulated text.
    // Should return Ok with partial=true and the accumulated text preserved.
    assert!(
        result.is_ok(),
        "SSE overflow should return Ok(partial), not Err. Got: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert!(result.partial, "Overflow result should be marked partial");
    assert!(
        result.text.len() >= 1_000_000,
        "Should preserve accumulated text (>1MB). Got {} bytes",
        result.text.len()
    );
    assert!(
        result.text.len() <= MAX_RESPONSE_BYTES,
        "Should not exceed MAX_RESPONSE_BYTES"
    );

    server.abort();
}
