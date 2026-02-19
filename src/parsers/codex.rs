use serde::Deserialize;

use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Codex CLI `--json` JSONL event stream.
/// Real format (captured from `codex exec --json`):
///   {"type":"thread.started","thread_id":"..."}
///   {"type":"turn.started"}
///   {"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"..."}}
///   {"type":"turn.completed","usage":{...}}
pub struct CodexParser;

#[derive(Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    item: Option<CodexItem>,
}

#[derive(Deserialize)]
struct CodexItem {
    #[serde(rename = "type")]
    item_type: Option<String>,
    text: Option<String>,
}

impl OutputParser for CodexParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let raw = String::from_utf8_lossy(stdout);
        let mut parts: Vec<String> = Vec::new();

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: CodexEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Look for item.completed events with agent_message type
            let is_item_completed = event
                .event_type
                .as_deref()
                .is_some_and(|t| t == "item.completed");

            if !is_item_completed {
                continue;
            }

            let Some(item) = &event.item else {
                continue;
            };

            let is_agent_message = item
                .item_type
                .as_deref()
                .is_some_and(|t| t == "agent_message");

            if is_agent_message
                && let Some(text) = &item.text
                && !text.is_empty()
            {
                parts.push(text.clone());
            }
        }

        if parts.is_empty() {
            return Err(SquallError::SchemaParse(
                "no message content found in codex output".to_string(),
            ));
        }

        Ok(parts.join("\n"))
    }
}
