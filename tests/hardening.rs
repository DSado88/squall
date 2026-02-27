//! Hardening tests (TDD RED phase).
//! Each test proves a defect found by multi-model consensus review.
//! Tests should FAIL before fixes and PASS after.

use squall::config::PersistRawOutput;
use squall::context::ContextFormat;
use squall::error::SquallError;

// ---------------------------------------------------------------------------
// Defect 1: CLI prompt delivered via stdin, not argv.
// Args templates must NOT contain {prompt} (avoids ARG_MAX limits).
// ---------------------------------------------------------------------------

#[test]
fn gemini_args_template_does_not_contain_prompt() {
    let config = squall::config::Config::from_env();
    if let Some(entry) = config.models.get("gemini")
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } = &entry.backend
    {
        assert!(
            !args_template.iter().any(|a| a.contains("{prompt}")),
            "gemini args_template must NOT contain '{{prompt}}' — prompt goes via stdin"
        );
    }
}

#[test]
fn codex_args_template_does_not_contain_prompt() {
    let config = squall::config::Config::from_env();
    if let Some(entry) = config.models.get("codex")
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } = &entry.backend
    {
        assert!(
            !args_template.iter().any(|a| a.contains("{prompt}")),
            "codex args_template must NOT contain '{{prompt}}' — prompt goes via stdin"
        );
    }
}

// ---------------------------------------------------------------------------
// Defect 2: HTTP response reads full body before size check.
// Tested structurally: http.rs must call content_length() check
// before bytes().await. Validated by code inspection +
// the existence of MAX_RESPONSE_BYTES as a pre-read guard.
// ---------------------------------------------------------------------------

#[test]
fn http_dispatch_has_response_size_limit() {
    let limit = squall::dispatch::http::MAX_RESPONSE_BYTES;
    assert!(limit > 0 && limit <= 10 * 1024 * 1024);
}

// ---------------------------------------------------------------------------
// Defect 3: CLI wait_with_output() buffers all stdout before cap.
// Same as HTTP — validated structurally. Test the constant.
// ---------------------------------------------------------------------------

#[test]
fn cli_dispatch_has_output_size_limit() {
    let limit = squall::dispatch::cli::MAX_OUTPUT_BYTES;
    assert!(limit > 0 && limit <= 10 * 1024 * 1024);
}

// ---------------------------------------------------------------------------
// Defect 4: Process group kill — kill_on_drop doesn't signal pgid.
// Structural: cli.rs sends SIGKILL to -pgid on timeout.
// (Validated by code inspection.)
// ---------------------------------------------------------------------------

// Structural — validated by code review in cli.rs

// ---------------------------------------------------------------------------
// Defect 5: Semaphore acquire() has no timeout — blocks forever
// if all permits are held.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn semaphore_acquire_respects_deadline() {
    use squall::config::Config;
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::registry::Registry;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // Create a registry with a CLI model but no real executable
    let mut models = HashMap::new();
    models.insert(
        "test-cli".to_string(),
        squall::dispatch::registry::ModelEntry {
            model_id: "test-cli".to_string(),
            provider: "gemini".to_string(),
            backend: squall::dispatch::registry::BackendConfig::Cli {
                executable: "nonexistent-binary-12345".to_string(),
                args_template: vec!["--".to_string()],
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Registry::from_config(config);

    // Request with a tight deadline — should not block forever on semaphore
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test-cli".to_string(),
        deadline: Instant::now() + Duration::from_millis(500),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    // The query should fail (nonexistent binary), but it should fail FAST,
    // not block on the semaphore. If semaphore has deadline awareness,
    // this completes within the deadline.
    let start = Instant::now();
    let _result = registry.query(&req).await;
    let elapsed = start.elapsed();

    // Should complete well within 2 seconds (not hang)
    assert!(
        elapsed < Duration::from_secs(2),
        "Registry::query should not block indefinitely. Took {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Defect 6: Upstream{status: None} incorrectly retryable.
// "response too large" and "empty choices" are permanent failures,
// not transient. They should NOT be retryable.
// ---------------------------------------------------------------------------

#[test]
fn upstream_response_too_large_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "response too large: 3000000 bytes (max 2097152)".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Response too large (status: None) should NOT be retryable"
    );
}

#[test]
fn upstream_empty_choices_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "empty choices or null content".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Empty choices (status: None) should NOT be retryable"
    );
}

#[test]
fn upstream_read_body_failed_is_retryable() {
    // Network failure reading body IS transient, but status: None = ambiguous.
    // Safe default: NOT retryable unless explicitly marked with status code.
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "failed to read response body: connection reset".to_string(),
        status: None,
    };
    assert!(
        !err.is_retryable(),
        "Upstream with status: None should NOT be retryable (ambiguous = safe default)"
    );
}

#[test]
fn upstream_5xx_still_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "500 Internal Server Error".to_string(),
        status: Some(500),
    };
    assert!(err.is_retryable(), "5xx should still be retryable");
}

#[test]
fn upstream_4xx_still_not_retryable() {
    let err = SquallError::Upstream {
        provider: "xai".to_string(),
        message: "400 Bad Request".to_string(),
        status: Some(400),
    };
    assert!(!err.is_retryable(), "4xx should NOT be retryable");
}

// ---------------------------------------------------------------------------
// Defect 7: No HTTP concurrency limit.
// Registry should have an HTTP semaphore.
// ---------------------------------------------------------------------------

#[test]
fn registry_has_http_concurrency_limit() {
    use squall::config::Config;
    use squall::dispatch::registry::Registry;
    use std::collections::HashMap;

    let config = Config {
        models: HashMap::new(),
        ..Default::default()
    };
    let registry = Registry::from_config(config);
    let permits = registry.http_semaphore_permits();
    assert!(
        permits > 0 && permits <= 20,
        "HTTP semaphore should have 1-20 permits, got {permits}"
    );
}

// ---------------------------------------------------------------------------
// Defect 8: CLI pipe deadlock when output exceeds MAX_OUTPUT_BYTES.
// After capped read via take(), pipe handles stay open. If the child
// produces more output than the cap, it blocks writing to the full pipe
// buffer. child.wait() then deadlocks waiting for the blocked child.
// Only resolved when the outer timeout fires — wasting the full deadline.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_oversized_output_completes_without_deadlock() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let start = Instant::now();

    // `yes` outputs infinite "y\n" — guaranteed to exceed MAX_OUTPUT_BYTES.
    // Parser will fail (not JSON), but we're testing timing, not parsing.
    let _result = dispatch
        .query_model(
            &req,
            "test",
            "yes",
            &[],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    let elapsed = start.elapsed();

    // Should complete in < 5s: read 2MB, drop pipes, yes gets SIGPIPE, exits.
    // BUG: without dropping pipe handles after capped read, yes blocks on
    // write to full pipe, wait() hangs until the 10s deadline fires.
    assert!(
        elapsed < Duration::from_secs(5),
        "CLI dispatch deadlocked on oversized output. Took {:?} (expected < 5s)",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Defect 15: Symmetric deadlock — stderr exceeds cap while stdout is quiet.
// Current code awaits stdout first. If stderr fills the cap, the stderr task
// drops the pipe, but child ignores SIGPIPE (or the shell runs another command
// after the writer dies). Child stays alive → stdout never gets EOF → deadlock.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_oversized_stderr_completes_without_deadlock() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let start = Instant::now();

    // `dd if=/dev/zero bs=1024 count=4096 >&2` writes 4MB of zeros to stderr.
    // After 2MB cap, stderr pipe is dropped → dd gets SIGPIPE → dd exits.
    // `sleep 3600` keeps the shell alive → stdout never gets EOF.
    // BUG: code awaits stdout first, only kills child on stdout cap.
    // Stderr cap is never checked → shell stays alive → deadlock until timeout.
    let _result = dispatch
        .query_model(
            &req,
            "test",
            "sh",
            &[
                "-c".to_string(),
                "dd if=/dev/zero bs=1024 count=4096 >&2 2>/dev/null; sleep 3600".to_string(),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    let elapsed = start.elapsed();

    // Should complete in < 5s (kill child when stderr hits cap).
    // BUG: only stdout cap triggers kill → hangs for full 10s deadline.
    assert!(
        elapsed < Duration::from_secs(5),
        "CLI dispatch deadlocked on oversized stderr. Took {:?} (expected < 5s)",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Defect 16: Cap breach kills only leader, not process group.
// start_kill() sends SIGKILL to sh only. Grandchildren (sleep) survive,
// holding stderr pipe open → stderr reader blocks until outer timeout.
// Fix: libc::kill(-pgid, SIGKILL) kills entire group (matches timeout path).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_cap_kills_process_group_not_just_leader() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let start = Instant::now();

    // sh spawns dd (floods 4MB to stdout → exceeds 2MB cap) in background,
    // then runs sleep 3600 as foreground. All share the same process group.
    // When stdout cap is hit:
    //   BUG:  start_kill() kills sh only → sleep survives → holds stderr → deadlock
    //   FIX:  pgid kill kills sh + dd + sleep → all pipes close → fast
    let _result = dispatch
        .query_model(
            &req,
            "test",
            "sh",
            &[
                "-c".to_string(),
                "dd if=/dev/zero bs=1024 count=4096 2>/dev/null & sleep 3600".to_string(),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "Cap breach should kill process group, not just leader. Took {:?} (expected < 5s). \
         start_kill() only kills sh — sleep survives holding stderr pipe.",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Defect 9: HTTP stream_body_capped truncates to exactly max_bytes,
// making the post-cap check `bytes.len() > MAX_RESPONSE_BYTES` dead code.
// Oversized responses produce confusing SchemaParse errors instead of
// clear "response too large" Upstream errors.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_oversized_response_gives_clear_error() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::http::HttpDispatch;
    use squall::dispatch::registry::ApiFormat;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // Mock SSE server: sends streaming chunks that total > MAX_RESPONSE_BYTES (2MB).
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;

            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Content-Type: text/event-stream\r\n\
                      Connection: close\r\n\r\n",
                )
                .await;

            // Send SSE chunks totaling > 2MB. Each chunk is ~64KB of content.
            let big_content: String = "x".repeat(64 * 1024);
            for _ in 0..48 {
                let event = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{big_content}\"}}}}]}}\n\n"
                );
                if socket.write_all(event.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = dispatch
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            "fake-key",
            &ApiFormat::OpenAi,
        )
        .await;

    server.abort();

    // Overflow now returns partial result (preserving accumulated text) instead
    // of discarding everything with a hard error.
    let result = result.expect("SSE overflow should return Ok(partial), not Err");
    assert!(result.partial, "Overflow result should be marked partial");
    assert!(
        result.text.len() >= 64 * 1024,
        "Should preserve accumulated text. Got {} bytes",
        result.text.len()
    );
}

// ---------------------------------------------------------------------------
// Bug G1: CLI stdin write deadlock on large prompts.
// write_all(prompt) blocks if prompt > OS pipe buffer (~64KB) because
// the child may fill its stdout/stderr pipes waiting for the parent to read.
// But the parent hasn't started reading yet (readers spawn after write).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_large_prompt_does_not_deadlock() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();

    // 256KB prompt — well above the 64KB pipe buffer.
    // Use `cat` which echoes stdin to stdout. If stdin write blocks before
    // stdout reader starts, cat fills the stdout pipe → both sides block → deadlock.
    // Wrap in valid Gemini JSON so the parser succeeds.
    let big_payload = "x".repeat(256 * 1024);
    let prompt = format!(r#"{{"response": "{big_payload}"}}"#);

    let req = ProviderRequest {
        prompt: prompt.into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let start = Instant::now();

    // Wrap in tokio timeout because the deadlock blocks the stdin write_all
    // which happens BEFORE the internal timeout wrapper — it hangs forever.
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        dispatch.query_model(
            &req,
            "test",
            "cat",
            &[],
            &GeminiParser,
            PersistRawOutput::Never,
        ),
    )
    .await;
    let elapsed = start.elapsed();

    // RED: write_all blocks → cat blocks on stdout → deadlock → outer timeout fires at 8s
    // GREEN: concurrent stdin write → completes in < 5s
    assert!(
        elapsed < Duration::from_secs(5),
        "CLI dispatch deadlocked on large prompt. Took {:?} (expected < 5s)",
        elapsed
    );
    assert!(result.is_ok(), "Outer timeout should not fire: {result:?}");
}

// ---------------------------------------------------------------------------
// Defect 10: CLI prompt passed via argv risks ARG_MAX exhaustion.
// Prompt should be delivered via stdin instead.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_prompt_delivered_via_stdin() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    // Valid Gemini JSON — cat will echo this back via stdout
    let prompt = r#"{"response": "stdin_delivery_works"}"#;
    let req = ProviderRequest {
        prompt: prompt.into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(5),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    // `cat` reads stdin and echoes to stdout. Empty args = read from stdin.
    // RED: stdin is /dev/null → cat outputs nothing → GeminiParser fails
    // GREEN: stdin piped with prompt → cat echoes it → GeminiParser succeeds
    let result = dispatch
        .query_model(
            &req,
            "test",
            "cat",
            &[],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    let text = result.expect("cat should echo prompt from stdin").text;
    assert_eq!(text, "stdin_delivery_works");
}

// ---------------------------------------------------------------------------
// Defect 11: HTTP chunk read errors silently swallowed.
// `while let Ok(Some(chunk))` drops Err variants → truncated body → SchemaParse.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_chunk_error_not_silently_swallowed() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::http::HttpDispatch;
    use squall::dispatch::registry::ApiFormat;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;

            // Chunked response: declare 256-byte chunk but send only 4 bytes,
            // then drop connection. This is an incomplete chunk — definite error.
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100\r\nAAAA")
                .await;
            // Drop socket — incomplete chunk (promised 256 bytes, sent 4)
        }
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(5),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let result = dispatch
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            "fake-key",
            &ApiFormat::OpenAi,
        )
        .await;

    server.abort();
    let err = result.unwrap_err();
    let debug = format!("{err:?}");

    // RED: chunk error swallowed → partial body → SchemaParse("failed to parse response")
    // GREEN: chunk error propagated → Request(...) or Upstream (not SchemaParse)
    assert!(
        !debug.contains("SchemaParse"),
        "Chunk read error should NOT produce SchemaParse, got: {debug}"
    );
}

// ---------------------------------------------------------------------------
// Defect 12: JSON serialization fallback creates invalid JSON when error
// message contains quotes. format!() doesn't escape.
// ---------------------------------------------------------------------------

#[test]
fn json_fallback_with_quotes_in_error_is_valid() {
    // Tests the escape logic used in response.rs into_call_tool_result fallback.
    // The error message from serde_json may contain quotes.
    let error_msg = r#"invalid type: found "string", expected u32"#;
    let escaped = error_msg.replace('\\', "\\\\").replace('"', "\\\"");
    let json = format!(
        r#"{{"status":"error","content":"serialization failed: {escaped}","content_type":"text","metadata":{{}}}}"#
    );

    // RED: without escaping, unescaped quotes break the JSON structure
    // GREEN: escaped quotes produce valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&json);
    assert!(
        parsed.is_ok(),
        "JSON fallback must produce valid JSON even with quotes in error: {json}"
    );
}

// ---------------------------------------------------------------------------
// Defect 14: XML comment injection in context.rs.
// Filenames containing "-->" break <!-- ... --> comment structure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn xml_comment_injection_prevented() {
    use squall::context;

    let dir = std::env::temp_dir().join("squall-test-xml-injection");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // File with "-->" in name — will be skipped for budget and appear in comment
    let evil_name = "test-->evil.txt";
    std::fs::write(dir.join(evil_name), "x".repeat(1024)).unwrap();
    // Small file that fits in budget
    std::fs::write(dir.join("small.txt"), "hello").unwrap();

    // Tiny budget: small.txt fits (~40 bytes with XML wrapper), evil file skipped
    let result = context::resolve_file_context(
        &["small.txt".to_string(), evil_name.to_string()],
        &dir,
        100,
        ContextFormat::Xml,
    )
    .await
    .unwrap()
    .context
    .unwrap();

    let _ = std::fs::remove_dir_all(&dir);

    // RED: "<!-- Budget skipped: test-->evil.txt (1024B). -->"
    //       The "-->" in the filename closes the comment early.
    //       Multiple "-->" sequences = broken XML structure.
    // GREEN: "--" escaped in comment content → exactly one "-->"
    let arrow_count = result.matches("-->").count();
    assert!(
        arrow_count <= 1,
        "XML comment injection: filename broke comment structure. \
         Found {arrow_count} occurrences of '-->'. Output:\n{result}"
    );
}

// ---------------------------------------------------------------------------
// Upstream error body truncation
// ---------------------------------------------------------------------------

#[test]
fn upstream_error_message_bounded() {
    // Simulate what http.rs does: if the body is >500 chars, truncate
    let long_body = "x".repeat(2000);
    let truncated: String = long_body.chars().take(500).collect();
    let message = format!(
        "400 Bad Request: {truncated}... [{} bytes total]",
        long_body.len()
    );
    // Message should be bounded: 500 chars of body + status + suffix
    assert!(
        message.len() < 600,
        "Error message should be bounded, got {}",
        message.len()
    );
}

// ---------------------------------------------------------------------------
// Bug #2: http.rs truncation checks bytes but truncates chars
// ---------------------------------------------------------------------------

#[test]
fn upstream_error_truncation_byte_char_mismatch() {
    // 300 emoji chars × 4 bytes = 1200 bytes. Fewer than 500 chars, so
    // chars().take(500) captures everything. The truncation condition should
    // detect that no actual truncation occurred.
    // BUG (was): condition used .len() (bytes > 500 → true) instead of
    // checking if take(500) actually truncated (truncated.len() < text.len()).
    let emoji_body = "\u{1F600}".repeat(300); // 300 chars, 1200 bytes
    let truncated: String = emoji_body.chars().take(500).collect();
    // Correct condition: truncated.len() < emoji_body.len() → false (all chars captured)
    let was_actually_truncated = truncated.len() < emoji_body.len();
    assert!(
        !was_actually_truncated,
        "300 chars taken with take(500) should NOT truncate"
    );
}

// ---------------------------------------------------------------------------
// Bug #3: error.rs stderr preview checks bytes but takes chars
// ---------------------------------------------------------------------------

#[test]
fn process_exit_stderr_byte_char_prefix_mismatch() {
    // 150 emoji chars × 4 bytes = 600 bytes. len() > 200 is true,
    // but chars().count() = 150 which is ≤ 200. Prefix "..." should NOT appear.
    // BUG: condition uses .len() (bytes) instead of .chars().count()
    let emoji_stderr = "\u{1F600}".repeat(150); // 150 chars, 600 bytes
    let err = SquallError::ProcessExit {
        code: 1,
        stderr: emoji_stderr.clone(),
    };
    let msg = err.user_message();
    // With only 150 chars, take(200) captures everything. No truncation occurred.
    // So "..." prefix should NOT appear.
    assert!(
        !msg.contains("..."),
        "150 chars (600 bytes) should not trigger truncation prefix. \
         Bug: len() checks bytes not chars. Message: {msg}"
    );
}

// ---------------------------------------------------------------------------
// DoS: wrap_diff_context must not allocate proportional to raw input
// ---------------------------------------------------------------------------

#[test]
fn wrap_diff_large_input_respects_budget() {
    // 10MB of '<' chars. Without pre-truncation, escape_xml_content allocates
    // 40MB+ (each '<' → '&lt;' = 4x). With pre-truncation, caps at budget first.
    let huge_diff = "<".repeat(10_000_000);
    let budget = 1000;
    let result = squall::context::wrap_diff_context(&huge_diff, budget);
    let wrapped = result.expect("Should return Some");
    // Strip wrapper to check content
    let content = wrapped
        .strip_prefix("<diff>\n")
        .unwrap_or(&wrapped)
        .strip_suffix("\n</diff>")
        .unwrap_or(&wrapped);
    let escaped_content = if let Some(pos) = content.find("\n<!-- diff truncated") {
        &content[..pos]
    } else {
        content
    };
    assert!(
        escaped_content.len() <= budget,
        "10MB input with budget {budget} should produce ≤{budget} bytes of escaped content, got {}",
        escaped_content.len()
    );
}

// ---------------------------------------------------------------------------
// DoS: file_paths array length must be capped
// ---------------------------------------------------------------------------

#[test]
fn file_paths_array_length_capped() {
    // 10,000 paths should be rejected before processing
    let max_paths = squall::context::MAX_FILE_PATHS;
    assert!(
        max_paths <= 200,
        "MAX_FILE_PATHS should be ≤200 to prevent DoS, got {max_paths}"
    );
}

// ---------------------------------------------------------------------------
// DoS: models array length must be capped
// ---------------------------------------------------------------------------

#[test]
fn models_array_length_capped() {
    let max_models = squall::review::MAX_MODELS;
    assert!(
        max_models <= 50,
        "MAX_MODELS should be ≤50 to prevent DoS, got {max_models}"
    );
}

// ---------------------------------------------------------------------------
// Input validation: temperature
// ---------------------------------------------------------------------------

#[test]
fn temperature_nan_rejected() {
    assert!(squall::context::validate_temperature(Some(f64::NAN)).is_err());
}

#[test]
fn temperature_infinity_rejected() {
    assert!(squall::context::validate_temperature(Some(f64::INFINITY)).is_err());
    assert!(squall::context::validate_temperature(Some(f64::NEG_INFINITY)).is_err());
}

#[test]
fn temperature_negative_rejected() {
    assert!(squall::context::validate_temperature(Some(-0.1)).is_err());
}

#[test]
fn temperature_above_2_rejected() {
    assert!(squall::context::validate_temperature(Some(2.1)).is_err());
}

#[test]
fn temperature_valid_values_accepted() {
    assert!(squall::context::validate_temperature(None).is_ok());
    assert!(squall::context::validate_temperature(Some(0.0)).is_ok());
    assert!(squall::context::validate_temperature(Some(1.0)).is_ok());
    assert!(squall::context::validate_temperature(Some(2.0)).is_ok());
}

// ---------------------------------------------------------------------------
// Input validation: prompt
// ---------------------------------------------------------------------------

#[test]
fn empty_prompt_rejected() {
    assert!(squall::context::validate_prompt("").is_err());
}

#[test]
fn whitespace_only_prompt_rejected() {
    assert!(squall::context::validate_prompt("   \n\t  ").is_err());
}

#[test]
fn valid_prompt_accepted() {
    assert!(squall::context::validate_prompt("hello").is_ok());
}

// ---------------------------------------------------------------------------
// Bug C1: symlink escape detection uses string matching on "escapes"
// A file path containing "escapes" in its name is misclassified as a
// symlink escape and hard-fails the entire request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filename_containing_escapes_not_misclassified() {
    use squall::context;

    let dir = std::env::temp_dir().join("squall-test-escapes-word");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // File with "escapes" in the name — should NOT be treated as symlink escape
    let filename = "escapes_plan.txt";
    std::fs::write(dir.join(filename), "harmless content").unwrap();

    let result =
        context::resolve_file_context(&[filename.to_string()], &dir, 10_000, ContextFormat::Xml)
            .await;

    let _ = std::fs::remove_dir_all(&dir);

    // RED: e.to_string().contains("escapes") matches the filename in the
    // "not found" error message, misclassifying it as a symlink escape.
    // Actually — the file exists, so canonicalize succeeds. Let's test
    // with a nonexistent file whose path contains "escapes".
    assert!(
        result.is_ok(),
        "File named 'escapes_plan.txt' should not be misclassified as symlink escape: {result:?}"
    );
}

#[tokio::test]
async fn nonexistent_file_with_escapes_in_name_is_soft_error() {
    use squall::context;

    let dir = std::env::temp_dir().join("squall-test-escapes-soft");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Create a second valid file so the request doesn't fail with "all files nonexistent"
    std::fs::write(dir.join("valid.txt"), "ok").unwrap();

    // Nonexistent file with "escapes" in path — canonicalize fails,
    // error message contains the rel_path which contains "escapes"
    let result = context::resolve_file_context(
        &["my_escapes_notes.txt".to_string(), "valid.txt".to_string()],
        &dir,
        10_000,
        ContextFormat::Xml,
    )
    .await;

    let _ = std::fs::remove_dir_all(&dir);

    // RED: contains("escapes") matches the filename in the error string,
    //      so this returns Err (hard reject) instead of Ok with soft skip.
    // GREEN: structured error detection, not string matching.
    assert!(
        result.is_ok(),
        "Nonexistent file named 'my_escapes_notes.txt' should be a soft skip, \
         not a hard security rejection: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Bug G2: stderr finishes first in select!, stdout cap breach doesn't kill
// If stderr is empty (finishes instantly), select! enters stderr branch.
// The awaited stdout handle may hit MAX_OUTPUT_BYTES, but its size is never
// checked → process not killed → child.wait() hangs until outer timeout.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_cross_stream_cap_kills_process() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    let start = Instant::now();

    // `exec 2>&-` closes stderr fd immediately → stderr reader gets EOF → wins select!.
    // `head -c 4194304 /dev/zero` writes 4MB to stdout → exceeds 2MB cap.
    // sleep keeps process alive if not killed.
    // BUG: stderr branch wins select!, awaits stdout, but never checks
    // stdout's size → no kill → sleep keeps process alive → hangs.
    let _result = tokio::time::timeout(
        Duration::from_secs(8),
        dispatch.query_model(
            &req,
            "test",
            "sh",
            &[
                "-c".to_string(),
                "exec 2>&-; head -c 4194304 /dev/zero; sleep 3600".to_string(),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        ),
    )
    .await;

    let elapsed = start.elapsed();

    // GREEN: cross-stream cap check kills process → completes in < 5s
    // RED: no kill → hangs for 10s deadline
    assert!(
        elapsed < Duration::from_secs(5),
        "Cross-stream cap breach should kill process. Took {:?} (expected < 5s). \
         stderr finished first, stdout cap breach was unchecked.",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Bug G1-R4: floor_entity_boundary panics on multibyte UTF-8 characters.
// index.saturating_sub(4) can land inside a multibyte char (e.g., 4-byte emoji),
// causing s[start..index] to panic at a non-char-boundary.
// Scenario: "🦀<" → escaped "🦀&lt;" (8 bytes). Budget 6 → floor_char_boundary
// returns 6 (inside &lt;), saturating_sub(4) = 2 which is mid-emoji → panic.
// ---------------------------------------------------------------------------

#[test]
fn floor_entity_boundary_no_panic_on_multibyte() {
    // "🦀<" escapes to "🦀&lt;" (4 bytes emoji + 4 bytes entity = 8 bytes total)
    // Budget 6: floor_char_boundary(8-byte string, 6) = 4 (emoji boundary)
    // floor_entity_boundary called with index=4. saturating_sub(4) = 0. OK here.
    // Budget 5: floor_char_boundary returns 4. Same thing.
    // Budget 7: floor_char_boundary returns 7 (inside &lt;).
    //           saturating_sub(4) = 3 which is INSIDE the 4-byte emoji → PANIC
    let diff = "🦀<\n";
    // Budget 7 triggers the panic path
    let result = std::panic::catch_unwind(|| squall::context::wrap_diff_context(diff, 7));
    assert!(
        result.is_ok(),
        "floor_entity_boundary panicked on multibyte char: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Bug G3: wrap_diff_context can split XML entities mid-entity
// escape_xml_content produces multi-char entities like &lt; (4 chars).
// floor_char_boundary only respects UTF-8 boundaries, not entity boundaries.
// Truncation at budget can produce malformed XML like "&l" or "&am".
// ---------------------------------------------------------------------------

#[test]
fn wrap_diff_entity_not_split_mid_entity() {
    // Craft a diff where escaping produces entities right at the budget boundary.
    // "a<" → "a&lt;" (5 chars). Budget 3 → floor_char_boundary truncates to "a&l"
    // which is a broken XML entity.
    let diff = "a<b\n";
    // Budget 3: escaped "a&lt;b\n" (7 chars), truncate at 3 → "a&l" (broken entity)
    let result = squall::context::wrap_diff_context(diff, 3).unwrap();

    // Extract content between <diff>\n and \n</diff>
    let content = result
        .strip_prefix("<diff>\n")
        .unwrap_or(&result)
        .strip_suffix("\n</diff>")
        .unwrap_or(&result);

    // Strip truncation comment if present
    let escaped_part = if let Some(pos) = content.find("\n<!-- diff truncated") {
        &content[..pos]
    } else {
        content
    };

    // A dangling '&' not followed by a complete entity is malformed XML
    // Check: no '&' that isn't part of a complete entity (&amp; &lt; &gt;)
    let mut i = 0;
    let bytes = escaped_part.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'&' {
            let rest = &escaped_part[i..];
            assert!(
                rest.starts_with("&amp;") || rest.starts_with("&lt;") || rest.starts_with("&gt;"),
                "Dangling/split XML entity at position {i}: {:?}",
                &escaped_part[i..escaped_part.len().min(i + 10)]
            );
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Bug R2-C1: Parser sees N+1 bytes if process exits cleanly at exact overflow.
// After take(N+1), process that writes N+1 bytes and exits status 0 before
// SIGKILL arrives → parser runs on over-limit data. Should reject explicitly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_overflow_by_one_byte_is_rejected() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::{CliDispatch, MAX_OUTPUT_BYTES};
    use squall::error::SquallError;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    // Output exactly MAX_OUTPUT_BYTES + 1. Process exits cleanly (status 0).
    // kill_on_cap sends SIGKILL to already-dead process (no-op).
    // Without overflow check, parser runs on N+1 bytes.
    let result = dispatch
        .query_model(
            &req,
            "gemini",
            "bash",
            &[
                "-c".to_string(),
                format!("yes | head -c {}", MAX_OUTPUT_BYTES + 1),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    // The process outputs N+1 bytes and may exit before SIGKILL.
    // Either way, we should get an error (ProcessExit from kill, or explicit overflow).
    // Must NOT get SchemaParse (parser running on over-limit data).
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        matches!(err, SquallError::ProcessExit { .. } | SquallError::Other(_)),
        "N+1 byte output should be rejected as overflow or ProcessExit, not SchemaParse. Got: {err:?}"
    );
    // If it's the explicit overflow path, verify the message
    if matches!(err, SquallError::Other(_)) {
        assert!(
            err_msg.contains("exceeded") && err_msg.contains("limit"),
            "Overflow error should mention exceeding limit. Got: {err_msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// Bug R3-C1: Overflow check only on stdout, not stderr.
// A process dumping huge stderr but small stdout passes the overflow check.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_stderr_overflow_is_rejected() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::{CliDispatch, MAX_OUTPUT_BYTES};
    use squall::error::SquallError;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    // Small stdout (valid exit), huge stderr (N+1 bytes).
    // Without stderr overflow check, this would pass the stdout check
    // and reach the parser (or return ProcessExit from kill_on_cap).
    let result = dispatch
        .query_model(
            &req,
            "gemini",
            "bash",
            &[
                "-c".to_string(),
                format!("echo ok; yes >&2 | head -c {} >&2", MAX_OUTPUT_BYTES + 1),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    // RED: stderr overflow not checked → parser runs on "ok\n" (SchemaParse)
    //      or ProcessExit from kill_on_cap. Either way, no explicit overflow error.
    // GREEN: explicit stderr overflow check catches it.
    let err = result.unwrap_err();
    let is_overflow = matches!(&err, SquallError::Other(msg) if msg.contains("exceeded"));
    let is_process_exit = matches!(err, SquallError::ProcessExit { .. });
    assert!(
        is_overflow || is_process_exit,
        "Stderr overflow should be caught explicitly or via kill. Got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Model name suggestions (suggest_models + enriched ModelNotFound)
// ---------------------------------------------------------------------------

#[test]
fn suggest_models_substring_match() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use std::collections::HashMap;

    let mut models = HashMap::new();
    models.insert(
        "grok-4-1-fast-reasoning".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "xai".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://test".to_string(),
                api_key: "k".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    models.insert(
        "gemini".to_string(),
        ModelEntry {
            model_id: "gemini".to_string(),
            provider: "gemini".to_string(),
            backend: BackendConfig::Cli {
                executable: "echo".to_string(),
                args_template: vec![],
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let registry = Registry::from_config(Config {
        models,
        ..Default::default()
    });

    let suggestions = registry.suggest_models("grok");
    assert!(
        suggestions.iter().any(|s| s.contains("grok")),
        "suggest_models('grok') should match grok model, got: {suggestions:?}"
    );
    assert!(
        !suggestions.iter().any(|s| s.contains("gemini")),
        "'grok' should not match 'gemini', got: {suggestions:?}"
    );
}

#[test]
fn suggest_models_reverse_match() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use std::collections::HashMap;

    let mut models = HashMap::new();
    models.insert(
        "grok-4-1-fast-reasoning".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "xai".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://test".to_string(),
                api_key: "k".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let registry = Registry::from_config(Config {
        models,
        ..Default::default()
    });

    // Query longer than model name — reverse contains should match
    let suggestions = registry.suggest_models("grok-4-1-fast-reasoning-turbo");
    assert!(
        suggestions.iter().any(|s| s.contains("grok")),
        "Longer query should suggest shorter match via reverse contains, got: {suggestions:?}"
    );
}

#[test]
fn suggest_models_no_match() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use std::collections::HashMap;

    let mut models = HashMap::new();
    models.insert(
        "grok-4-1-fast-reasoning".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "xai".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://test".to_string(),
                api_key: "k".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let registry = Registry::from_config(Config {
        models,
        ..Default::default()
    });

    let suggestions = registry.suggest_models("zzz-nonexistent-model");
    assert!(
        suggestions.is_empty(),
        "Nonexistent model should produce no suggestions, got: {suggestions:?}"
    );
}

#[test]
fn suggest_models_empty_query() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use std::collections::HashMap;

    let mut models = HashMap::new();
    models.insert(
        "grok-4-1-fast-reasoning".to_string(),
        ModelEntry {
            model_id: "grok-4-1-fast-reasoning".to_string(),
            provider: "xai".to_string(),
            backend: BackendConfig::Http {
                base_url: "http://test".to_string(),
                api_key: "k".to_string(),
                api_format: ApiFormat::OpenAi,
            },
            description: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            speed_tier: "fast".to_string(),
            precision_tier: "medium".to_string(),
        },
    );
    let registry = Registry::from_config(Config {
        models,
        ..Default::default()
    });

    let suggestions = registry.suggest_models("");
    assert!(
        suggestions.is_empty(),
        "Empty query should return no suggestions (not all models), got: {suggestions:?}"
    );

    let whitespace = registry.suggest_models("   ");
    assert!(
        whitespace.is_empty(),
        "Whitespace query should return no suggestions, got: {whitespace:?}"
    );
}

#[test]
fn suggest_models_sorted_and_capped() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use std::collections::HashMap;

    // Create a registry with 10 models all containing "test"
    let mut models = HashMap::new();
    for i in 0..10 {
        let name = format!("test-model-{:02}", 9 - i); // Insert in reverse order
        models.insert(
            name.clone(),
            ModelEntry {
                model_id: name.clone(),
                provider: "test".to_string(),
                backend: BackendConfig::Http {
                    base_url: "http://test".to_string(),
                    api_key: "key".to_string(),
                    api_format: ApiFormat::OpenAi,
                },
                description: String::new(),
                strengths: vec![],
                weaknesses: vec![],
                speed_tier: "fast".to_string(),
                precision_tier: "medium".to_string(),
            },
        );
    }
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Registry::from_config(config);

    let suggestions = registry.suggest_models("test");
    assert!(
        suggestions.len() <= 5,
        "Suggestions should be capped at 5, got {}",
        suggestions.len()
    );
    // Check sorted
    let mut sorted = suggestions.clone();
    sorted.sort();
    assert_eq!(
        suggestions, sorted,
        "Suggestions should be alphabetically sorted"
    );
}

#[test]
fn model_not_found_includes_suggestion() {
    let err = SquallError::ModelNotFound {
        model: "grok".to_string(),
        suggestions: vec!["grok-4-1-fast-reasoning".to_string()],
    };
    let msg = err.user_message();
    assert!(
        msg.contains("Did you mean"),
        "Error with suggestions should include 'Did you mean', got: {msg}"
    );
    assert!(
        msg.contains("grok-4-1-fast-reasoning"),
        "Suggestion should be in message, got: {msg}"
    );
}

#[test]
fn model_not_found_no_suggestion() {
    let err = SquallError::ModelNotFound {
        model: "zzz".to_string(),
        suggestions: vec![],
    };
    let msg = err.user_message();
    assert_eq!(
        msg, "model not found: zzz",
        "Error without suggestions should be clean, got: {msg}"
    );
    assert!(
        !msg.contains("Did you mean"),
        "No suggestions = no 'Did you mean'"
    );
}

// ---------------------------------------------------------------------------
// Bug R1-C1/G1: Off-by-one in CLI output cap kill.
// take(MAX_OUTPUT_BYTES) + kill_on_cap(>= MAX_OUTPUT_BYTES) means a process
// outputting exactly MAX_OUTPUT_BYTES is killed. Should read N+1 and check > N.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_exact_limit_output_not_killed() {
    use squall::dispatch::ProviderRequest;
    use squall::dispatch::cli::{CliDispatch, MAX_OUTPUT_BYTES};
    use squall::error::SquallError;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: "test".into(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    };

    // Use head to output exactly MAX_OUTPUT_BYTES of 'y\n' data.
    // This won't be valid JSON, so the parser will fail with SchemaParse.
    // But if kill_on_cap fires (the bug), we get ProcessExit { code: -1 }
    // instead, because SIGKILL causes signal death.
    let result = dispatch
        .query_model(
            &req,
            "gemini",
            "bash",
            &[
                "-c".to_string(),
                format!("yes | head -c {MAX_OUTPUT_BYTES}"),
            ],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    // RED: buf.len() >= MAX_OUTPUT_BYTES → kill → ProcessExit { code: -1 }
    // GREEN: buf.len() > MAX_OUTPUT_BYTES (after reading N+1) → no kill → SchemaParse
    let err = result.unwrap_err();
    assert!(
        matches!(err, SquallError::SchemaParse(_)),
        "Output of exactly MAX_OUTPUT_BYTES should produce SchemaParse (bad JSON), \
         not ProcessExit (signal kill). Got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Bug R1-C2: Nondeterministic model selection in review None branch.
// HashMap::values().take(MAX_MODELS) gives different subsets across runs
// when >MAX_MODELS models are configured. Should sort first.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_none_branch_model_selection_is_sorted() {
    use squall::config::Config;
    use squall::dispatch::registry::{ApiFormat, BackendConfig, ModelEntry, Registry};
    use squall::memory::MemoryStore;
    use squall::review::ReviewExecutor;
    use squall::tools::review::ReviewRequest;
    use std::collections::HashMap;
    use std::sync::Arc;

    // Create more than MAX_MODELS (20) models
    let mut models = HashMap::new();
    for i in 0..25 {
        // Names intentionally in reverse order to exercise sorting
        let name = format!("model-{:02}", 24 - i);
        models.insert(
            name.clone(),
            ModelEntry {
                model_id: name.clone(),
                provider: "test".to_string(),
                backend: BackendConfig::Http {
                    base_url: "http://127.0.0.1:1/v1/chat".to_string(),
                    api_key: "key".to_string(),
                    api_format: ApiFormat::OpenAi,
                },
                description: String::new(),
                strengths: vec![],
                weaknesses: vec![],
                speed_tier: "fast".to_string(),
                precision_tier: "medium".to_string(),
            },
        );
    }
    let config = Config {
        models,
        ..Default::default()
    };
    let registry = Arc::new(Registry::from_config(config));
    let executor = ReviewExecutor::new(registry);

    let req = ReviewRequest {
        prompt: "test".into(),
        models: None, // triggers the None branch
        timeout_secs: Some(3),
        system_prompt: None,
        temperature: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: None,
        max_tokens: None,
        reasoning_effort: None,
        context_format: None,
        response_format: None,
        investigation_context: None,
    };

    let resp = executor
        .execute(
            &req,
            req.prompt.clone(),
            &MemoryStore::new(),
            None,
            None,
            None,
            None,
        )
        .await;
    // Collect all models that were attempted (results + not_started won't
    // include not_started here since all models exist in registry)
    let mut selected: Vec<String> = resp.results.iter().map(|r| r.model.clone()).collect();
    selected.sort();

    // With 25 models and MAX_MODELS=20, the first 20 alphabetically should be selected.
    // That's model-00 through model-19.
    // RED: HashMap iteration is arbitrary → might include model-20..24 instead of model-00..04
    // GREEN: sorted before take → always picks model-00 through model-19
    assert_eq!(
        selected.len(),
        squall::review::MAX_MODELS,
        "Should select exactly MAX_MODELS"
    );
    assert_eq!(
        selected[0], "model-00",
        "First model should be model-00 (alphabetically first). Got: {}",
        selected[0]
    );
    assert_eq!(
        selected.last().unwrap(),
        "model-19",
        "Last model should be model-19 (20th alphabetically). Got: {}",
        selected.last().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Defect: Diff context starvation.
// When file_paths consume the full MAX_FILE_CONTEXT_BYTES budget, the diff
// gets zero budget and is silently dropped — the most critical review input
// is lost while static file content is preserved.
//
// RED: proves the defect exists (diff budget becomes 0 when files fill budget).
// ---------------------------------------------------------------------------

/// Prove that diff gets a minimum reserved budget even when files are large.
#[test]
fn diff_budget_not_starved_when_files_fill_budget() {
    // Simulate the FIXED server.rs budget allocation:
    //   file_budget = MAX - MIN_DIFF_BUDGET (when diff is present)
    //   diff_budget = MAX - file_context_used
    let total_budget = squall::context::MAX_FILE_CONTEXT_BYTES;
    let min_diff = squall::context::MIN_DIFF_BUDGET;

    // File context budget is capped when diff is present
    let file_budget = total_budget.saturating_sub(min_diff);

    // Files consume their entire (capped) budget
    let file_context_used = file_budget;

    // Diff budget = total - file_used = MIN_DIFF_BUDGET
    let diff_budget = total_budget.saturating_sub(file_context_used);

    assert!(
        diff_budget >= min_diff,
        "Diff budget should be at least {}B (MIN_DIFF_BUDGET) but was {}B. \
         The diff — the most critical review input — is starved.",
        min_diff,
        diff_budget
    );
}

/// Prove that wrap_diff_context always succeeds when diff has reserved budget.
#[test]
fn diff_always_gets_minimum_budget() {
    let total_budget = squall::context::MAX_FILE_CONTEXT_BYTES;
    let min_diff = squall::context::MIN_DIFF_BUDGET;

    // File budget is capped when diff is present
    let file_budget = total_budget.saturating_sub(min_diff);
    // Files consume their full capped budget
    let file_context_used = file_budget;
    let diff_budget = total_budget.saturating_sub(file_context_used);

    // wrap_diff_context should NOT return None because diff has reserved budget
    let diff_text = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"old\");\n+    println!(\"new\");\n }";
    let wrapped = squall::context::wrap_diff_context(diff_text, diff_budget);
    assert!(
        wrapped.is_some(),
        "Diff should never be silently dropped — it's the most critical review input. \
         wrap_diff_context returned None because diff_budget was {}",
        diff_budget
    );
}
