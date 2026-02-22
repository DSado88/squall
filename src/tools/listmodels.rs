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
