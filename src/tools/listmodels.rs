use serde::Serialize;

use crate::dispatch::registry::ModelEntry;

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub backend: String,
}

impl From<&ModelEntry> for ModelInfo {
    fn from(entry: &ModelEntry) -> Self {
        Self {
            name: entry.model_id.clone(),
            provider: entry.provider.clone(),
            backend: entry.backend_name().to_string(),
        }
    }
}

#[derive(Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelInfo>,
}
