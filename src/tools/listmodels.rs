use serde::Serialize;

use crate::dispatch::registry::ModelEntry;

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub backend: String,
}

impl From<(&String, &ModelEntry)> for ModelInfo {
    fn from((key, entry): (&String, &ModelEntry)) -> Self {
        Self {
            name: key.clone(),
            provider: entry.provider.clone(),
            backend: entry.backend_name().to_string(),
        }
    }
}

#[derive(Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelInfo>,
}
