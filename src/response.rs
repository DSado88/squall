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
    #[serde(serialize_with = "serialize_finite_f64")]
    pub duration_seconds: f64,
}

/// Serialize f64, clamping non-finite values (NaN, Inf) to 0.0.
fn serialize_finite_f64<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(if v.is_finite() { *v } else { 0.0 })
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
        match serde_json::to_string(&self) {
            Ok(json) => CallToolResult::success(vec![Content::text(json)]),
            Err(e) => {
                let escaped = e.to_string().replace('\\', "\\\\").replace('"', "\\\"");
                CallToolResult::success(vec![Content::text(format!(
                    r#"{{"status":"error","content":"serialization failed: {escaped}","content_type":"text","metadata":{{}}}}"#
                ))])
            }
        }
    }
}
