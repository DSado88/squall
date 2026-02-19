use std::collections::HashMap;

use crate::config::Config;
use crate::dispatch::cli::CliDispatch;
use crate::dispatch::http::HttpDispatch;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::OutputParser;

/// Backend-specific configuration. Prevents invalid states
/// (e.g., a CLI entry with an HTTP URL or vice versa).
#[derive(Clone)]
pub enum BackendConfig {
    Http {
        base_url: String,
        api_key: String,
    },
    Cli {
        executable: String,
        args_template: Vec<String>,
    },
}

#[derive(Clone)]
pub struct ModelEntry {
    pub model_id: String,
    pub provider: String,
    pub backend: BackendConfig,
}

impl ModelEntry {
    /// Returns true if this entry uses HTTP dispatch.
    pub fn is_http(&self) -> bool {
        matches!(self.backend, BackendConfig::Http { .. })
    }

    /// Returns true if this entry uses CLI dispatch.
    pub fn is_cli(&self) -> bool {
        matches!(self.backend, BackendConfig::Cli { .. })
    }

    /// Returns the backend type as a string for display purposes.
    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            BackendConfig::Http { .. } => "http",
            BackendConfig::Cli { .. } => "cli",
        }
    }
}

impl std::fmt::Debug for ModelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ModelEntry");
        s.field("model_id", &self.model_id)
            .field("provider", &self.provider);

        match &self.backend {
            BackendConfig::Http { base_url, .. } => {
                s.field("backend", &"http")
                    .field("base_url", base_url)
                    .field("api_key", &"[REDACTED]");
            }
            BackendConfig::Cli {
                executable,
                args_template,
            } => {
                s.field("backend", &"cli")
                    .field("executable", executable)
                    .field("args_template", args_template);
            }
        }

        s.finish()
    }
}

pub struct Registry {
    models: HashMap<String, ModelEntry>,
    http: HttpDispatch,
    cli: CliDispatch,
}

impl Registry {
    pub fn from_config(config: Config) -> Self {
        Self {
            models: config.models,
            http: HttpDispatch::new(),
            cli: CliDispatch::new(),
        }
    }

    pub fn get(&self, model: &str) -> Option<&ModelEntry> {
        self.models.get(model)
    }

    pub fn list_models(&self) -> Vec<&ModelEntry> {
        self.models.values().collect()
    }

    /// Resolve the appropriate parser for a CLI provider.
    fn parser_for(provider: &str) -> Box<dyn OutputParser> {
        match provider {
            "gemini" => Box::new(GeminiParser),
            "codex" => Box::new(CodexParser),
            _ => Box::new(GeminiParser), // fallback to Gemini JSON parser
        }
    }

    pub async fn query(&self, req: &ProviderRequest) -> Result<ProviderResult, SquallError> {
        let entry = self
            .models
            .get(&req.model)
            .ok_or_else(|| SquallError::ModelNotFound(req.model.clone()))?;

        match &entry.backend {
            BackendConfig::Http { base_url, api_key } => {
                self.http
                    .query_model(req, &entry.provider, base_url, api_key)
                    .await
            }
            BackendConfig::Cli {
                executable,
                args_template,
            } => {
                let parser = Self::parser_for(&entry.provider);
                self.cli
                    .query_model(req, &entry.provider, executable, args_template, &*parser)
                    .await
            }
        }
    }
}
