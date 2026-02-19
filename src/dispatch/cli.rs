use std::time::{Duration, Instant};

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

        let child = cmd.spawn().map_err(|e| SquallError::Other(
            format!("failed to spawn {executable}: {e}"),
        ))?;

        // Get the PID so we can kill the entire process group on timeout.
        // process_group(0) makes the child its own group leader (pgid == pid).
        let child_pid = child.id();

        // Wait with timeout — on expiry, kill the entire process group
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| SquallError::Other(
                format!("failed to wait for {executable}: {e}"),
            ))?,
            Err(_) => {
                // Timeout: kill the process group, not just the leader
                if let Some(pid) = child_pid {
                    // Send SIGKILL to the negative pgid to kill all processes in the group
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                }
                let elapsed_ms = start.elapsed().as_millis() as u64;
                return Err(SquallError::Timeout(elapsed_ms));
            }
        };

        // Cap output size
        let stdout = if output.stdout.len() > MAX_OUTPUT_BYTES {
            &output.stdout[..MAX_OUTPUT_BYTES]
        } else {
            &output.stdout
        };

        let stderr_text = String::from_utf8_lossy(
            &output.stderr[..output.stderr.len().min(MAX_OUTPUT_BYTES)],
        )
        .to_string();

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
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
        let text = parser.parse(stdout)?;

        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(ProviderResult {
            text,
            model: req.model.clone(),
            provider: provider.to_string(),
            latency_ms,
        })
    }
}
