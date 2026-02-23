use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// Request to save a learning to Squall's memory.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemorizeRequest {
    /// Category: "pattern" (recurring finding), "tactic" (prompt effectiveness learning), or "recommend" (model recommendation)
    pub category: String,
    /// The insight to remember (max 500 characters)
    pub content: String,
    /// Which model this relates to (optional)
    pub model: Option<String>,
    /// Tags for future filtering (optional)
    pub tags: Option<Vec<String>>,
    /// Scope for this entry: "codebase", "branch:feature/x", "commit:abc1234", "pr:42".
    /// Auto-detected from git context if working_directory is set and scope is not provided.
    pub scope: Option<String>,
    /// Working directory for auto-detecting git context (branch/commit).
    /// Required for automatic scope detection.
    pub working_directory: Option<String>,
    /// Arbitrary key-value metadata (e.g. consensus: "3/5", diff_size: "+120 -45")
    pub metadata: Option<HashMap<String, String>>,
}

/// Request to read Squall's memory.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryRequest {
    /// Which memory to read: "all" (default), "models", "patterns", "tactics", "recommend"
    pub category: Option<String>,
    /// Filter by model name (optional, applies to tactics)
    pub model: Option<String>,
    /// Maximum characters to return (default 4000)
    pub max_chars: Option<usize>,
    /// Filter patterns by scope (exact match). E.g. "branch:feature/x", "codebase".
    /// None returns all entries.
    pub scope: Option<String>,
}

impl MemoryRequest {
    pub fn max_chars(&self) -> usize {
        self.max_chars.unwrap_or(4000)
    }
}

/// Request to flush branch-scoped memory after PR merge.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FlushRequest {
    /// Branch name to flush (e.g. "feature/auth")
    pub branch: String,
    /// PR number for context (optional, informational only)
    pub pr_number: Option<u32>,
}
