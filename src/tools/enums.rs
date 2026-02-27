use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reasoning effort level for thinking models.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }
}

/// Category for the memorize tool (save a learning).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemorizeCategory {
    /// Recurring finding across reviews.
    #[serde(alias = "patterns")]
    Pattern,
    /// Prompt effectiveness learning.
    #[serde(alias = "tactics")]
    Tactic,
    /// Model recommendation based on observed performance.
    #[serde(alias = "recommendation", alias = "recommendations")]
    Recommend,
}

impl MemorizeCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pattern => "pattern",
            Self::Tactic => "tactic",
            Self::Recommend => "recommend",
        }
    }
}

/// Category for the memory read tool. Omit to read all categories.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Models,
    /// Read recurring patterns from memory. Also accepts "pattern".
    #[serde(alias = "pattern")]
    Patterns,
    /// Read prompt tactics from memory. Also accepts "tactic".
    #[serde(alias = "tactic")]
    Tactics,
    /// Read model recommendations from memory. Also accepts "recommendation"/"recommendations".
    #[serde(alias = "recommendation", alias = "recommendations")]
    Recommend,
}

impl MemoryCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Models => "models",
            Self::Patterns => "patterns",
            Self::Tactics => "tactics",
            Self::Recommend => "recommend",
        }
    }
}

/// Response format for review results.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResponseFormat {
    /// Full per-model responses with all details.
    #[default]
    Detailed,
    /// Summary + results_file path only (no per-model text).
    Concise,
}
