use serde::Serialize;

use crate::dispatch::registry::ModelEntry;

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub backend: String,
    pub description: String,
    pub strengths: Vec<String>,
    pub weaknesses: Vec<String>,
    pub speed_tier: String,
    pub precision_tier: String,
}

impl From<(&String, &ModelEntry)> for ModelInfo {
    fn from((key, entry): (&String, &ModelEntry)) -> Self {
        Self {
            name: key.clone(),
            provider: entry.provider.clone(),
            backend: entry.backend_name().to_string(),
            description: entry.description.clone(),
            strengths: entry.strengths.clone(),
            weaknesses: entry.weaknesses.clone(),
            speed_tier: entry.speed_tier.clone(),
            precision_tier: entry.precision_tier.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelInfo>,
}

/// Escape pipe, newline, and carriage-return characters for markdown table cells.
fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\r', "").replace('\n', " ")
}

impl ListModelsResponse {
    /// Render the model list as a markdown table.
    pub fn to_markdown(&self) -> String {
        let mut md = String::from(
            "| Model | Provider | Backend | Speed | Precision | Description |\n\
             |-------|----------|---------|-------|-----------|-------------|\n",
        );
        for m in &self.models {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                escape_cell(&m.name),
                escape_cell(&m.provider),
                escape_cell(&m.backend),
                escape_cell(&m.speed_tier),
                escape_cell(&m.precision_tier),
                escape_cell(&m.description),
            ));
        }
        md
    }
}
