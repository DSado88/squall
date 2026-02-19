use serde::Deserialize;

use crate::error::SquallError;
use crate::parsers::OutputParser;

/// Parses Codex CLI `--json` JSONL event stream.
/// Extracts text from `response.completed` events where `item.type == "message"`.
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
    content: Option<Vec<CodexContent>>,
}

#[derive(Deserialize)]
struct CodexContent {
    #[serde(rename = "type")]
    content_type: Option<String>,
    text: Option<String>,
}

impl OutputParser for CodexParser {
    fn parse(&self, stdout: &[u8]) -> Result<String, SquallError> {
        let text = String::from_utf8_lossy(stdout);
        let mut parts: Vec<String> = Vec::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: CodexEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let is_completed = event
                .event_type
                .as_deref()
                .is_some_and(|t| t == "response.completed");

            if !is_completed {
                continue;
            }

            let Some(item) = &event.item else {
                continue;
            };

            let is_message = item
                .item_type
                .as_deref()
                .is_some_and(|t| t == "message");

            if !is_message {
                continue;
            }

            let Some(content) = &item.content else {
                continue;
            };

            for c in content {
                let is_output_text = c
                    .content_type
                    .as_deref()
                    .is_some_and(|t| t == "output_text");

                if is_output_text
                    && let Some(text) = &c.text
                    && !text.is_empty()
                {
                    parts.push(text.clone());
                }
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
