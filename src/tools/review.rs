use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::enums::{ReasoningEffort, ResponseFormat};
use crate::context::ContextFormat;

/// Request to dispatch a prompt to multiple models with straggler cutoff.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviewRequest {
    /// The prompt to send to all models. File context and diff are prepended automatically.
    pub prompt: String,
    /// Model names to query (from `listmodels`). Defaults to `[review] default_models` from config if omitted.
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
    /// Reasoning effort for thinking models. Non-reasoning models ignore this.
    /// Medium/high automatically extend the deadline to 600s.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Relative file paths to include as context (read and inlined server-side). Requires working_directory.
    pub file_paths: Option<Vec<String>>,
    /// Absolute path to the project root for resolving file_paths.
    pub working_directory: Option<String>,
    /// Unified diff text (e.g. `git diff` output) to include as review context. Shares budget with file_paths.
    pub diff: Option<String>,
    /// Per-model system prompt overrides for different review lenses. Key = exact model name from
    /// `listmodels`, value = system prompt. Models not in this map use the shared system_prompt.
    /// Example lenses: security auditor, architecture reviewer, correctness checker.
    /// Check `memory` category "tactics" for proven system prompts.
    #[schemars(
        description = "Per-model system prompt overrides for different review lenses. Key = exact model name, value = system prompt. Models not listed fall back to the shared system_prompt. Use different lenses (security, architecture, correctness) for diverse coverage."
    )]
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
    /// Response format: "detailed" (default, full per-model responses) or "concise" (summary only).
    pub response_format: Option<ResponseFormat>,
    /// Pre-review investigation context (code structure notes, hypotheses, areas of concern).
    /// Persist-only — NOT injected into model prompts. Models get context via per_model_system_prompts.
    /// Clamped to 32KB to prevent oversized persistence payloads.
    #[schemars(
        description = "Pre-review investigation notes for traceability. Persisted alongside results but not sent to models. Max 32KB."
    )]
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
    /// Deep mode defaults to 600s, but explicit timeout_secs overrides this.
    pub fn effective_timeout_secs(&self) -> u64 {
        if self.deep == Some(true) {
            self.timeout_secs.unwrap_or(Self::DEEP_TIMEOUT_SECS)
        } else {
            self.timeout_secs()
        }
    }

    /// Effective reasoning effort: deep mode defaults to High.
    pub fn effective_reasoning_effort(&self) -> Option<ReasoningEffort> {
        if self.deep == Some(true) && self.reasoning_effort.is_none() {
            Some(ReasoningEffort::High)
        } else {
            self.reasoning_effort
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
    /// Number of models attempted (pre-gate, post-dedup, post-MAX_MODELS truncation).
    pub models_requested: usize,
    /// Models excluded by hard gate (success rate below threshold).
    pub models_gated: usize,
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
    /// True if models were auto-selected via tiered selection (models omitted in request).
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_selected: bool,
    /// Human-readable explanation of how models were chosen (only when auto_selected).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_reasoning: Option<String>,
}

/// Full review response (rendered as markdown for MCP, persisted as JSON to disk).
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
    /// File read errors (non-existent files, permission errors). Non-fatal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_errors: Option<Vec<String>>,
    /// Actionable warnings about the review execution (unknown keys, truncation, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Quick summary of model outcomes.
    pub summary: ReviewSummary,
}

impl ReviewResponse {
    /// Render the review response as markdown for the MCP response.
    /// `concise` mode omits per-model response text (just summary + results_file).
    pub fn to_markdown(&self, concise: bool) -> String {
        let mut md = String::with_capacity(1024);

        // Summary line
        md.push_str(&format!(
            "## Review Summary\n{} succeeded, {} failed, {} cutoff, {} partial | {}ms elapsed\n",
            self.summary.models_succeeded,
            self.summary.models_failed,
            self.summary.models_cutoff,
            self.summary.models_partial,
            self.elapsed_ms,
        ));

        if let Some(ref file) = self.results_file {
            md.push_str(&format!("\nResults saved: `{file}`\n"));
        }

        // Persistence error — critical in concise mode where model text is omitted
        if let Some(ref err) = self.persist_error {
            md.push_str(&format!("\n**Persist error**: {err}\n"));
        }

        // File context issues
        if let Some(ref skipped) = self.files_skipped
            && !skipped.is_empty()
        {
            md.push_str(&format!(
                "\n**Files skipped (budget)**: {}\n",
                skipped.join(", ")
            ));
        }
        if let Some(ref errors) = self.files_errors
            && !errors.is_empty()
        {
            md.push_str(&format!("\n**File errors**: {}\n", errors.join(", ")));
        }

        // Warnings
        if !self.warnings.is_empty() {
            md.push_str("\n### Warnings\n");
            for w in &self.warnings {
                md.push_str(&format!("- {w}\n"));
            }
        }

        // Not started
        if !self.not_started.is_empty() {
            md.push_str(&format!(
                "\n**Not started**: {}\n",
                self.not_started.join(", ")
            ));
        }

        // Per-model responses (detailed only)
        if !concise {
            let succeeded: Vec<_> = self
                .results
                .iter()
                .filter(|r| r.status == ModelStatus::Success)
                .collect();
            let failed: Vec<_> = self
                .results
                .iter()
                .filter(|r| r.status != ModelStatus::Success)
                .collect();

            if !succeeded.is_empty() {
                for res in &succeeded {
                    md.push_str(&format!(
                        "\n### {} ({}ms{})\n",
                        res.model,
                        res.latency_ms,
                        if res.partial { ", partial" } else { "" },
                    ));
                    if let Some(ref text) = res.response {
                        md.push_str(text.trim());
                        md.push('\n');
                    }
                }
            }

            if !failed.is_empty() {
                md.push_str("\n### Failed Models\n");
                for res in &failed {
                    md.push_str(&format!("- **{}**: ", res.model));
                    if let Some(ref err) = res.error {
                        md.push_str(err);
                    }
                    if let Some(ref reason) = res.reason {
                        md.push_str(&format!(" ({reason})"));
                    }
                    md.push_str(&format!(" [{}ms]\n", res.latency_ms));
                }
            }
        }

        md
    }
}
