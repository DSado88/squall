use rmcp::model::{CallToolResult, Content};
use serde::Serialize;

/// PAL-compatible tool response format.
/// The `/consensus` slash command and ori-v2 parse this JSON shape.
/// All tools return Content::text(json_string) — double-encoded JSON matching PAL's format.
#[derive(Debug, Serialize)]
pub struct PalToolResponse {
    pub status: &'static str,
    pub content: String,
    pub content_type: &'static str,
    pub metadata: PalMetadata,
}

#[derive(Debug, Serialize)]
pub struct PalMetadata {
    pub tool_name: String,
    pub model_used: String,
    pub provider_used: String,
    pub duration_seconds: f64,
}

impl PalToolResponse {
    pub fn success(content: String, metadata: PalMetadata) -> Self {
        Self {
            status: "success",
            content,
            content_type: "text",
            metadata,
        }
    }

    pub fn error(message: String, metadata: PalMetadata) -> Self {
        Self {
            status: "error",
            content: message,
            content_type: "text",
            metadata,
        }
    }

    /// Convert to MCP CallToolResult.
    /// Always returns success at the MCP transport level to prevent Claude Code
    /// from cascading sibling tool call failures. Error info is in the JSON payload
    /// (`"status": "error"`) where Claude can read it without triggering cascade.
    pub fn into_call_tool_result(self) -> CallToolResult {
        // Clamp non-finite f64 values before serialization to avoid serde_json panic
        let safe = PalToolResponseSafe {
            status: self.status,
            content: self.content,
            content_type: self.content_type,
            metadata: PalMetadataSafe {
                tool_name: self.metadata.tool_name,
                model_used: self.metadata.model_used,
                provider_used: self.metadata.provider_used,
                duration_seconds: clamp_finite(self.metadata.duration_seconds),
            },
        };

        match serde_json::to_string(&safe) {
            Ok(json) => CallToolResult::success(vec![Content::text(json)]),
            Err(e) => CallToolResult::success(vec![Content::text(format!(
                r#"{{"status":"error","content":"serialization failed: {e}","content_type":"text","metadata":{{}}}}"#
            ))]),
        }
    }
}

/// Clamp non-finite f64 to 0.0 for safe JSON serialization.
fn clamp_finite(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

// Internal safe copy with guaranteed-finite f64 for serialization
#[derive(Serialize)]
struct PalToolResponseSafe {
    status: &'static str,
    content: String,
    content_type: &'static str,
    metadata: PalMetadataSafe,
}

#[derive(Serialize)]
struct PalMetadataSafe {
    tool_name: String,
    model_used: String,
    provider_used: String,
    duration_seconds: f64,
}
