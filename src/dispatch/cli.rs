use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::OutputParser;

pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024; // 2MB

pub struct CliDispatch;

impl Default for CliDispatch {
    fn default() -> Self {
        Self
    }
}

impl CliDispatch {
    pub fn new() -> Self {
        Self
    }

    /// Execute a CLI subprocess and return parsed output.
    ///
    /// Safety features:
    /// - No shell interpolation (uses Command::new + args, not shell)
    /// - kill_on_drop(true) prevents zombie processes
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

        // Build args by substituting {prompt} and {model} in the template.
        // No shell — Command::new() + .args() prevents shell injection.
        let args: Vec<String> = args_template
            .iter()
            .map(|a| {
                a.replace("{prompt}", &req.prompt)
                    .replace("{model}", &req.model)
            })
            .collect();

        let mut cmd = Command::new(executable);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0) // Kill entire process tree on timeout, not just top-level
            .kill_on_drop(true);

        // Set working directory for CLI subprocess if provided
        if let Some(ref wd) = req.working_directory {
            cmd.current_dir(wd);
        }

        let mut child = cmd.spawn().map_err(|e| SquallError::Other(
            format!("failed to spawn {executable}: {e}"),
        ))?;

        // Get the PID so we can kill the entire process group on timeout.
        // process_group(0) makes the child its own group leader (pgid == pid).
        let child_pid = child.id();

        // Take pipe handles for capped reading — prevents OOM from runaway processes.
        // Unlike wait_with_output() which buffers ALL output, take() caps at MAX_OUTPUT_BYTES.
        let stdout_pipe = child.stdout.take().expect("stdout was piped");
        let stderr_pipe = child.stderr.take().expect("stderr was piped");

        let read_future = async {
            let mut stdout_buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(64 * 1024));
            let mut stderr_buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(64 * 1024));

            // Bind take() to named variables so they outlive the join!
            let mut stdout_capped = stdout_pipe.take(MAX_OUTPUT_BYTES as u64);
            let mut stderr_capped = stderr_pipe.take(MAX_OUTPUT_BYTES as u64);

            // Read stdout and stderr concurrently, each capped at MAX_OUTPUT_BYTES.
            let (_, _) = tokio::join!(
                stdout_capped.read_to_end(&mut stdout_buf),
                stderr_capped.read_to_end(&mut stderr_buf),
            );

            // Wait for process exit after pipes are drained
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

        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(ProviderResult {
            text,
            model: req.model.clone(),
            provider: provider.to_string(),
            latency_ms,
        })
    }
}
