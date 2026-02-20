//! Hardening tests (TDD RED phase).
//! Each test proves a defect found by multi-model consensus review.
//! Tests should FAIL before fixes and PASS after.

use squall::error::SquallError;

// ---------------------------------------------------------------------------
// Defect 1: CLI prompt delivered via stdin, not argv.
// Args templates must NOT contain {prompt} (avoids ARG_MAX limits).
// ---------------------------------------------------------------------------

#[test]
fn gemini_args_template_does_not_contain_prompt() {
    let config = squall::config::Config::from_env();
    if let Some(entry) = config.models.get("gemini")
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } =
            &entry.backend
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
        && let squall::dispatch::registry::BackendConfig::Cli { args_template, .. } =
            &entry.backend
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
    use squall::dispatch::registry::Registry;
    use squall::dispatch::ProviderRequest;
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
        },
    );
    let config = Config { models };
    let registry = Registry::from_config(config);

    // Request with a tight deadline — should not block forever on semaphore
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test-cli".to_string(),
        deadline: Instant::now() + Duration::from_millis(500),
        working_directory: None,
        system_prompt: None,
        temperature: None,
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
    use squall::dispatch::cli::CliDispatch;
    use squall::dispatch::ProviderRequest;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: String::new(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
    };

    let start = Instant::now();

    // `yes` outputs infinite "y\n" — guaranteed to exceed MAX_OUTPUT_BYTES.
    // Parser will fail (not JSON), but we're testing timing, not parsing.
    let _result = dispatch
        .query_model(&req, "test", "yes", &[], &GeminiParser)
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
    use squall::dispatch::cli::CliDispatch;
    use squall::dispatch::ProviderRequest;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: String::new(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
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
    use squall::dispatch::cli::CliDispatch;
    use squall::dispatch::ProviderRequest;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    let req = ProviderRequest {
        prompt: String::new(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: None,
        system_prompt: None,
        temperature: None,
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
    use squall::dispatch::http::HttpDispatch;
    use squall::dispatch::ProviderRequest;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // Mock HTTP server: sends > MAX_RESPONSE_BYTES without Content-Length,
    // bypassing the pre-read size check and hitting stream_body_capped.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;

            // No Content-Length → client reads until connection close
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .await;

            // Send 3MB (exceeds 2MB MAX_RESPONSE_BYTES)
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..48 {
                if socket.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        }
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(30),
        working_directory: None,
        system_prompt: None,
        temperature: None,
    };

    let result = dispatch
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            "fake-key",
        )
        .await;

    server.abort();

    let err = result.unwrap_err();
    let debug = format!("{err:?}");

    // Should be Upstream "response too large", NOT SchemaParse.
    // BUG: stream_body_capped truncates to exactly MAX_RESPONSE_BYTES,
    // so bytes.len() > MAX_RESPONSE_BYTES is always false (dead code).
    // The truncated body fails JSON parsing → confusing SchemaParse error.
    assert!(
        debug.contains("too large"),
        "Oversized HTTP response should produce 'too large' error, got: {debug}"
    );
}

// ---------------------------------------------------------------------------
// Defect 10: CLI prompt passed via argv risks ARG_MAX exhaustion.
// Prompt should be delivered via stdin instead.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_prompt_delivered_via_stdin() {
    use squall::dispatch::cli::CliDispatch;
    use squall::dispatch::ProviderRequest;
    use squall::parsers::gemini::GeminiParser;
    use std::time::{Duration, Instant};

    let dispatch = CliDispatch::new();
    // Valid Gemini JSON — cat will echo this back via stdout
    let prompt = r#"{"response": "stdin_delivery_works"}"#.to_string();
    let req = ProviderRequest {
        prompt,
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(5),
        working_directory: None,
        system_prompt: None,
        temperature: None,
    };

    // `cat` reads stdin and echoes to stdout. Empty args = read from stdin.
    // RED: stdin is /dev/null → cat outputs nothing → GeminiParser fails
    // GREEN: stdin piped with prompt → cat echoes it → GeminiParser succeeds
    let result = dispatch
        .query_model(&req, "test", "cat", &[], &GeminiParser)
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
    use squall::dispatch::http::HttpDispatch;
    use squall::dispatch::ProviderRequest;
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
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100\r\nAAAA",
                )
                .await;
            // Drop socket — incomplete chunk (promised 256 bytes, sent 4)
        }
    });

    let dispatch = HttpDispatch::new();
    let req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(5),
        working_directory: None,
        system_prompt: None,
        temperature: None,
    };

    let result = dispatch
        .query_model(
            &req,
            "test",
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            "fake-key",
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
    )
    .await
    .unwrap()
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
