use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::config::PersistRawOutput;
use crate::dispatch::async_poll::sanitize_model_name;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::OutputParser;

pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024; // 2MB

/// Atomic counter for unique persist filenames (same pattern as async_poll.rs).
static PERSIST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Drop guard that kills the entire process group (not just the leader PID).
///
/// `kill_on_drop(true)` only sends SIGKILL to the child PID. When the child is
/// a process group leader (via `process_group(0)`) and spawns grandchildren,
/// dropping the `Child` handle only kills the leader — grandchildren survive as
/// orphans. This guard sends SIGKILL to the negative PID (the process group).
struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

pub struct CliDispatch;

#[allow(clippy::new_without_default)]
impl CliDispatch {
    pub fn new() -> Self {
        Self
    }

    /// Execute a CLI subprocess and return parsed output.
    ///
    /// Safety features:
    /// - No shell interpolation (uses Command::new + args, not shell)
    /// - ProcessGroupGuard kills entire process group on drop (grandchildren too)
    /// - Timeout derived from ProviderRequest.deadline
    /// - Output capped at MAX_OUTPUT_BYTES
    /// - Piped stdin/stdout/stderr (no terminal leakage)
    pub async fn query_model(
        &self,
        req: &ProviderRequest,
        provider: &str,
        executable: &str,
        args_template: &[String],
        parser: &dyn OutputParser,
        persist_mode: PersistRawOutput,
    ) -> Result<ProviderResult, SquallError> {
        let start = Instant::now();

        // Compute persist directory from working_directory (fixes CWD-bound persistence).
        // When working_directory is set, raw output lands in the project dir, not the
        // daemon's CWD. Falls back to "." when working_directory is unset.
        let persist_dir = req
            .working_directory
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        // Check for expired deadline before spawning
        let timeout = match req
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| *d > Duration::from_millis(100))
        {
            Some(t) => t,
            None => {
                if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                    spawn_persist(
                        persist_dir, b"", b"", &req.model, provider, -1, 0,
                        "pre_spawn_timeout",
                    );
                }
                return Err(SquallError::Timeout(0));
            }
        };

        // Build args by substituting {model} in the template.
        // Prompt is delivered via stdin to avoid ARG_MAX limits (~128KB-2MB).
        // No shell — Command::new() + .args() prevents shell injection.
        let args: Vec<String> = args_template
            .iter()
            .map(|a| a.replace("{model}", &req.model))
            .collect();

        let mut cmd = Command::new(executable);
        cmd.args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0); // Child becomes its own process group leader

        // Set working directory for CLI subprocess if provided
        if let Some(ref wd) = req.working_directory {
            cmd.current_dir(wd);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                    let status_msg = format!("spawn_error: {e}");
                    spawn_persist(
                        persist_dir.clone(), b"", b"", &req.model, provider,
                        -1, elapsed_ms, &status_msg,
                    );
                }
                return Err(SquallError::Other(
                    format!("failed to spawn {executable}: {e}"),
                ));
            }
        };

        // ProcessGroupGuard kills the entire process group on drop (including
        // grandchildren). This replaces kill_on_drop(true) which only kills
        // the leader PID, leaving grandchild processes as orphans when the
        // tokio task is aborted by JoinSet::abort_all().
        let _pg_guard = ProcessGroupGuard::new(child.id());

        // Write prompt to stdin concurrently with stdout/stderr reading.
        // CRITICAL: must NOT await write_all before spawning pipe readers.
        // If prompt > OS pipe buffer (~64KB) and child echoes output, both sides
        // block: parent waiting for child to drain stdin, child waiting for parent
        // to drain stdout. Spawning a task avoids this deadlock.
        {
            let mut stdin = child.stdin.take().expect("stdin was piped");
            let system_prompt = req.system_prompt.clone();
            let prompt = req.prompt.clone();
            tokio::spawn(async move {
                if let Some(ref system) = system_prompt {
                    let _ = stdin.write_all(system.as_bytes()).await;
                    let _ = stdin.write_all(b"\n\n").await;
                }
                let _ = stdin.write_all(prompt.as_bytes()).await;
                // drop closes the pipe → child sees EOF on stdin
            });
        }

        // Get the PID so we can kill the entire process group on timeout.
        // process_group(0) makes the child its own group leader (pgid == pid).
        let child_pid = child.id();

        // Take pipe handles for capped reading — prevents OOM from runaway processes.
        // Unlike wait_with_output() which buffers ALL output, take() caps at MAX_OUTPUT_BYTES.
        let stdout_pipe = child.stdout.take().expect("stdout was piped");
        let stderr_pipe = child.stderr.take().expect("stderr was piped");

        let read_future = async {
            // Spawn pipe readers as separate tasks so they run concurrently.
            // select! on the handles: whichever finishes first, check if it hit
            // the cap. If so, kill the child to unblock the other reader
            // (which waits for EOF that only comes when the child exits).
            // Read one extra byte beyond the limit to distinguish "exactly at limit"
            // from "exceeded limit". Without +1, take(N) returns N bytes in both cases
            // and we can't tell them apart — causing false kills at the exact boundary.
            let read_limit = MAX_OUTPUT_BYTES as u64 + 1;

            let stdout_handle = tokio::spawn(async move {
                let mut buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(64 * 1024));
                let mut capped = stdout_pipe.take(read_limit);
                if let Err(e) = capped.read_to_end(&mut buf).await {
                    tracing::warn!("stdout pipe read error: {e}");
                }
                buf
            });

            let stderr_handle = tokio::spawn(async move {
                let mut buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(64 * 1024));
                let mut capped = stderr_pipe.take(read_limit);
                if let Err(e) = capped.read_to_end(&mut buf).await {
                    tracing::warn!("stderr pipe read error: {e}");
                }
                buf
            });

            // Wait for whichever stream finishes first. If EITHER hit the cap,
            // the child may be blocked writing to the full pipe — kill it to
            // unblock the other reader (which waits for EOF on child exit).
            let mut stdout_handle = stdout_handle;
            let mut stderr_handle = stderr_handle;

            // Helper: kill the process group if either buffer hit the cap.
            // Kill only when output strictly exceeds the limit (the extra byte
            // from read_limit proves the process tried to write more than MAX_OUTPUT_BYTES).
            let kill_on_cap = |buf: &[u8]| {
                if buf.len() > MAX_OUTPUT_BYTES
                    && let Some(pid) = child_pid
                {
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                }
            };

            let (stdout_buf, stderr_buf) = tokio::select! {
                result = &mut stdout_handle => {
                    let buf = result.unwrap_or_default();
                    kill_on_cap(&buf);
                    let stderr_buf = stderr_handle.await.unwrap_or_default();
                    kill_on_cap(&stderr_buf);
                    (buf, stderr_buf)
                }
                result = &mut stderr_handle => {
                    let buf = result.unwrap_or_default();
                    kill_on_cap(&buf);
                    let stdout_buf = stdout_handle.await.unwrap_or_default();
                    kill_on_cap(&stdout_buf);
                    (stdout_buf, buf)
                }
            };
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((stdout_buf, stderr_buf, status))
        };

        let (stdout, stderr_raw, status) =
            match tokio::time::timeout(timeout, read_future).await {
                Ok(result) => match result {
                    Ok(data) => data,
                    Err(e) => {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                            let status_msg = format!("read_error: {e}");
                            spawn_persist(
                                persist_dir, b"", b"", &req.model, provider,
                                -1, elapsed_ms, &status_msg,
                            );
                        }
                        return Err(SquallError::Other(
                            format!("failed to read from {executable}: {e}"),
                        ));
                    }
                },
                Err(_) => {
                    // Timeout: kill the process group, not just the leader
                    if let Some(pid) = child_pid {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                        spawn_persist(
                            persist_dir.clone(), b"", b"", &req.model, provider,
                            -1, elapsed_ms, "timeout",
                        );
                    }
                    return Err(SquallError::Timeout(elapsed_ms));
                }
            };

        // Explicit overflow check: if either stream exceeded the cap (the +1 sentinel
        // byte was present), reject regardless of exit status. This handles the race
        // where the process exits cleanly before kill_on_cap's SIGKILL arrives.
        if stdout.len() > MAX_OUTPUT_BYTES || stderr_raw.len() > MAX_OUTPUT_BYTES {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                // Truncate to MAX_OUTPUT_BYTES to avoid persisting oversized data
                let cap_stdout = &stdout[..stdout.len().min(MAX_OUTPUT_BYTES)];
                let cap_stderr = &stderr_raw[..stderr_raw.len().min(MAX_OUTPUT_BYTES)];
                spawn_persist(
                    persist_dir.clone(), cap_stdout, cap_stderr, &req.model, provider,
                    -1, elapsed_ms, "output_overflow",
                );
            }
            return Err(SquallError::Other(format!(
                "CLI output exceeded {MAX_OUTPUT_BYTES} byte limit"
            )));
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let stderr_text = String::from_utf8_lossy(&stderr_raw).to_string();
        let exit_code = status.code().unwrap_or(-1);

        if !status.success() {
            tracing::warn!(
                executable,
                code = exit_code,
                "CLI process failed"
            );
            // Persist on failure if mode is Always or OnFailure
            if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                spawn_persist(persist_dir.clone(), &stdout, &stderr_raw, &req.model, provider, exit_code, elapsed_ms, "process_exit_error");
            }
            return Err(SquallError::ProcessExit {
                code: exit_code,
                stderr: stderr_text,
            });
        }

        // Log stderr at debug level even on success (progress info, etc.)
        if !stderr_text.is_empty() {
            tracing::debug!(executable, stderr = %stderr_text, "CLI stderr output");
        }

        // Parse the stdout through the appropriate parser
        let parse_result = parser.parse(&stdout);

        match &parse_result {
            Ok(_) => {
                // Parse succeeded — persist only if mode is Always
                if persist_mode == PersistRawOutput::Always {
                    spawn_persist(persist_dir.clone(), &stdout, &stderr_raw, &req.model, provider, exit_code, elapsed_ms, "ok");
                }
            }
            Err(e) => {
                // Parse failed — persist if mode is Always or OnFailure
                if matches!(persist_mode, PersistRawOutput::Always | PersistRawOutput::OnFailure) {
                    let status_msg = format!("parse_error: {e}");
                    spawn_persist(persist_dir.clone(), &stdout, &stderr_raw, &req.model, provider, exit_code, elapsed_ms, &status_msg);
                }
            }
        }

        let text = parse_result?;

        Ok(ProviderResult {
            text,
            model: req.model.clone(),
            provider: provider.to_string(),
            partial: false,
        })
    }
}

/// Spawn a background task to persist CLI output without blocking the response.
#[allow(clippy::too_many_arguments)]
fn spawn_persist(
    base_dir: std::path::PathBuf,
    stdout: &[u8],
    stderr: &[u8],
    model: &str,
    provider: &str,
    exit_code: i32,
    timing_ms: u64,
    parse_status: &str,
) {
    // Clone into owned data for the 'static future
    let stdout = stdout.to_vec();
    let stderr = stderr.to_vec();
    let model = model.to_string();
    let provider = provider.to_string();
    let parse_status = parse_status.to_string();
    tokio::spawn(async move {
        if let Err(e) = persist_cli_output(
            &base_dir, &model, &provider, &stdout, &stderr, exit_code, timing_ms, &parse_status,
        )
        .await
        {
            tracing::warn!("failed to persist raw CLI output: {e}");
        }
    });
}

/// Persist raw CLI output to `{base_dir}/.squall/raw/{timestamp}_{pid}_{seq}_{model}.json`.
/// Follows the same atomic write pattern as `persist_research_result()` in async_poll.rs.
///
/// When called from production code, `base_dir` is `.` (current directory).
/// Tests can pass a custom directory for isolation.
#[allow(clippy::too_many_arguments)]
pub async fn persist_cli_output(
    base_dir: &std::path::Path,
    model: &str,
    provider: &str,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: i32,
    timing_ms: u64,
    parse_status: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    let dir = base_dir.join(".squall/raw");
    tokio::fs::create_dir_all(&dir).await?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let seq = PERSIST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut safe_model = sanitize_model_name(model);

    // Truncate model name to keep filename under 255 bytes (OS limit).
    // Prefix is "{ts}_{pid}_{seq}_" (~40 chars) + ".json" (5 chars) = ~45 overhead.
    let prefix_len = format!("{ts}_{pid}_{seq}_").len();
    let max_model_len = 255 - prefix_len - ".json".len();
    if safe_model.len() > max_model_len {
        // Find a char boundary at or before max_model_len to avoid panicking.
        // sanitize_model_name allows Unicode alphanumerics (is_alphanumeric()),
        // so the string may contain multi-byte chars like 'ñ' or '中'.
        let mut boundary = max_model_len;
        while boundary > 0 && !safe_model.is_char_boundary(boundary) {
            boundary -= 1;
        }
        safe_model.truncate(boundary);
    }

    let filename = format!("{ts}_{pid}_{seq}_{safe_model}.json");
    let path = dir.join(&filename);

    let stdout_text = String::from_utf8_lossy(stdout);
    let stderr_text = String::from_utf8_lossy(stderr);

    let payload = serde_json::json!({
        "model": model,
        "provider": provider,
        "stdout": stdout_text,
        "stderr": stderr_text,
        "exit_code": exit_code,
        "timing_ms": timing_ms,
        "parse_status": parse_status,
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

    tracing::debug!("persisted raw CLI output to {}", path.display());
    Ok(path)
}
