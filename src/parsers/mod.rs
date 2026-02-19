pub mod codex;
pub mod gemini;

use crate::error::SquallError;

/// Trait for parsing CLI subprocess output into a text response.
/// Each CLI tool (Gemini, Codex) has its own output format.
pub trait OutputParser: Send + Sync {
    /// Parse raw stdout bytes into a text response string.
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError>;
}
