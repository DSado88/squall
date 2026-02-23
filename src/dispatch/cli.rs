use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::OutputParser;

pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024; // 2MB

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
    ) -> Result<ProviderResult, SquallError> {
        let start = Instant::now();

        // Check for expired deadline before spawning
        let timeout = req
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| *d > Duration::from_millis(100))
            .ok_or(SquallError::Timeout(0))?;

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

        let mut child = cmd.spawn().map_err(|e| SquallError::Other(
            format!("failed to spawn {executable}: {e}"),
        ))?;

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
                Ok(result) => result.map_err(|e| {
                    SquallError::Other(format!("failed to read from {executable}: {e}"))
                })?,
                Err(_) => {
                    // Timeout: kill the process group, not just the leader
                    if let Some(pid) = child_pid {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    return Err(SquallError::Timeout(elapsed_ms));
                }
            };

        // Explicit overflow check: if either stream exceeded the cap (the +1 sentinel
        // byte was present), reject regardless of exit status. This handles the race
        // where the process exits cleanly before kill_on_cap's SIGKILL arrives.
        if stdout.len() > MAX_OUTPUT_BYTES || stderr_raw.len() > MAX_OUTPUT_BYTES {
            return Err(SquallError::Other(format!(
                "CLI output exceeded {MAX_OUTPUT_BYTES} byte limit"
            )));
        }

        let stderr_text = String::from_utf8_lossy(&stderr_raw).to_string();

        if !status.success() {
            let code = status.code().unwrap_or(-1);
            tracing::warn!(
                executable,
                code,
                "CLI process failed"
            );
            return Err(SquallError::ProcessExit {
                code,
                stderr: stderr_text,
            });
        }

        // Log stderr at debug level even on success (progress info, etc.)
        if !stderr_text.is_empty() {
            tracing::debug!(executable, stderr = %stderr_text, "CLI stderr output");
        }

        // Parse the stdout through the appropriate parser
        let text = parser.parse(&stdout)?;

        Ok(ProviderResult {
            text,
            model: req.model.clone(),
            provider: provider.to_string(),
            partial: false,
        })
    }
}
