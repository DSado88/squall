use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::ContextFormat;

/// Request to dispatch a prompt to multiple models with straggler cutoff.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviewRequest {
    /// The prompt to send to all models. File context and diff are prepended automatically.
    pub prompt: String,
    /// Model names to query (from `listmodels`). Defaults to all configured models if omitted.
    pub models: Option<Vec<String>>,
    /// Straggler cutoff in seconds (default: 180). Models still running after this are cancelled.
    pub timeout_secs: Option<u64>,
    /// Shared system prompt for all models (e.g. "You are an expert code reviewer").
    /// Overridden per-model by per_model_system_prompts.
    pub system_prompt: Option<String>,
    /// Sampling temperature: 0.0 = deterministic (best for analysis/code), 1.0 = creative/diverse.
    pub temperature: Option<f64>,
    /// Maximum tokens to generate per model. Caps output length for each model's response.
    pub max_tokens: Option<u64>,
    /// Reasoning effort for thinking models: "none" (fastest), "low", "medium", "high" (deepest).
    /// Non-reasoning models ignore this. "medium"/"high" automatically extend the deadline to 600s.
    pub reasoning_effort: Option<String>,
    /// Relative file paths to include as context (read and inlined server-side). Requires working_directory.
    pub file_paths: Option<Vec<String>>,
    /// Absolute path to the project root for resolving file_paths.
    pub working_directory: Option<String>,
    /// Unified diff text (e.g. `git diff` output) to include as review context. Shares budget with file_paths.
    pub diff: Option<String>,
    /// Per-model system prompt overrides for different review lenses. Key = exact model name from
    /// `listmodels`, value = system prompt. Models not in this map use the shared system_prompt.
    /// Example lenses: security auditor, architecture reviewer, correctness checker.
    /// Check `memory` category "tactic" for proven system prompts.
    #[schemars(description = "Per-model system prompt overrides for different review lenses. Key = exact model name, value = system prompt. Models not listed fall back to the shared system_prompt. Use different lenses (security, architecture, correctness) for diverse coverage.")]
    pub per_model_system_prompts: Option<HashMap<String, String>>,
    /// Per-model timeout overrides in seconds. Key = model name, value = timeout.
    /// Each model's task deadline is min(per_model_timeout, global cutoff).
    /// Values clamped to MAX_TIMEOUT_SECS (600s).
    pub per_model_timeout_secs: Option<HashMap<String, u64>>,
    /// Deep review mode: sets timeout=600s, reasoning_effort="high", max_tokens=16384.
    /// Use for security audits, complex architecture reviews, or high-stakes changes.
    /// Individual fields (timeout_secs, reasoning_effort, max_tokens) override deep defaults.
    pub deep: Option<bool>,
    /// File context format: "xml" (default, full content) or "hashline" (line_num:hash|content,
    /// compact for large files). Hashline lets models reference lines by number+hash.
    pub context_format: Option<ContextFormat>,
    /// Pre-review investigation context (code structure notes, hypotheses, areas of concern).
    /// Persist-only — NOT injected into model prompts. Models get context via per_model_system_prompts.
    /// Clamped to 32KB to prevent oversized persistence payloads.
    #[schemars(description = "Pre-review investigation notes for traceability. Persisted alongside results but not sent to models. Max 32KB.")]
    pub investigation_context: Option<String>,
}

/// Maximum size for investigation_context in bytes (32KB).
pub const MAX_INVESTIGATION_CONTEXT_BYTES: usize = 32 * 1024;

impl ReviewRequest {
    pub const DEFAULT_TIMEOUT_SECS: u64 = 180;
    pub const DEEP_TIMEOUT_SECS: u64 = 600;
    pub const DEEP_MAX_TOKENS: u64 = 16384;

    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(Self::DEFAULT_TIMEOUT_SECS)
    }

    /// Effective timeout accounting for deep mode.
    /// Deep mode raises the minimum to 600s unless explicitly overridden.
    pub fn effective_timeout_secs(&self) -> u64 {
        if self.deep == Some(true) {
            self.timeout_secs.unwrap_or(Self::DEEP_TIMEOUT_SECS).max(Self::DEEP_TIMEOUT_SECS)
        } else {
            self.timeout_secs()
        }
    }

    /// Effective reasoning effort: deep mode defaults to "high".
    pub fn effective_reasoning_effort(&self) -> Option<String> {
        if self.deep == Some(true) && self.reasoning_effort.is_none() {
            Some("high".to_string())
        } else {
            self.reasoning_effort.clone()
        }
    }

    /// Effective max tokens: deep mode defaults to 16384.
    pub fn effective_max_tokens(&self) -> Option<u64> {
        if self.deep == Some(true) && self.max_tokens.is_none() {
            Some(Self::DEEP_MAX_TOKENS)
        } else {
            self.max_tokens
        }
    }
}

/// Per-model result in a review response.
#[derive(Debug, Serialize, Clone)]
pub struct ReviewModelResult {
    pub model: String,
    pub provider: String,
    pub status: ModelStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub latency_ms: u64,
    /// True if the response was truncated (cancellation, deadline, or stall).
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Status of an individual model in a review.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    Success,
    Error,
}

/// Counts of model outcomes for quick quality assessment.
#[derive(Debug, Serialize, Default)]
pub struct ReviewSummary {
    /// Number of models attempted (post-dedup, post-MAX_MODELS truncation).
    pub models_requested: usize,
    /// Full successful responses (Success + not partial).
    pub models_succeeded: usize,
    /// Models that returned errors (excluding cutoff — timeout, auth, parse, etc.).
    pub models_failed: usize,
    /// Models that hit straggler cutoff with no response.
    pub models_cutoff: usize,
    /// Models that returned partial content (cooperative cancellation).
    pub models_partial: usize,
    /// Models not found in registry.
    pub models_not_started: usize,
}

/// Full review response (serialized to JSON for MCP and disk).
#[derive(Debug, Serialize)]
pub struct ReviewResponse {
    pub results: Vec<ReviewModelResult>,
    pub not_started: Vec<String>,
    pub cutoff_seconds: u64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persist_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_skipped: Option<Vec<String>>,
    /// Actionable warnings about the review execution (unknown keys, truncation, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Quick summary of model outcomes.
    pub summary: ReviewSummary,
}
