use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::Semaphore;

use crate::config::{Config, PersistRawOutput};
use crate::dispatch::async_poll::AsyncPollDispatch;
use crate::dispatch::cli::CliDispatch;
use crate::dispatch::http::HttpDispatch;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::OutputParser;

/// Max concurrent CLI subprocesses per Squall instance.
const CLI_MAX_CONCURRENT: usize = 4;

/// Max concurrent HTTP requests per Squall instance.
const HTTP_MAX_CONCURRENT: usize = 8;

/// Max concurrent async-poll jobs per Squall instance.
/// Low limit since these are long-running (minutes to an hour).
const ASYNC_POLL_MAX_CONCURRENT: usize = 4;

/// Discriminant for async-poll API providers.
#[derive(Clone, Debug)]
pub enum AsyncPollProviderType {
    OpenAiResponses,
    GeminiInteractions,
}

/// API format for HTTP backends.
#[derive(Clone, Debug, Default)]
pub enum ApiFormat {
    /// OpenAI-compatible chat completions (default for most providers).
    #[default]
    OpenAi,
    /// Anthropic Messages API (different headers, SSE format).
    Anthropic,
}

/// Backend-specific configuration. Prevents invalid states
/// (e.g., a CLI entry with an HTTP URL or vice versa).
#[derive(Clone)]
pub enum BackendConfig {
    Http {
        base_url: String,
        api_key: String,
        api_format: ApiFormat,
    },
    Cli {
        executable: String,
        args_template: Vec<String>,
    },
    AsyncPoll {
        provider_type: AsyncPollProviderType,
        api_key: String,
    },
}

#[derive(Clone)]
pub struct ModelEntry {
    pub model_id: String,
    pub provider: String,
    pub backend: BackendConfig,
    /// One-line description of the model's purpose.
    pub description: String,
    /// What this model is best at (e.g., "systems-level bugs", "fast triage").
    pub strengths: Vec<String>,
    /// Known weaknesses or blind spots.
    pub weaknesses: Vec<String>,
    /// Speed tier: "fast", "medium", "slow", "very_slow".
    pub speed_tier: String,
    /// Precision tier: "high", "medium", "low".
    pub precision_tier: String,
}

impl ModelEntry {
    /// Returns true if this entry uses async-poll dispatch.
    pub fn is_async_poll(&self) -> bool {
        matches!(self.backend, BackendConfig::AsyncPoll { .. })
    }

    /// Returns the backend type as a string for display purposes.
    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            BackendConfig::Http { .. } => "http",
            BackendConfig::Cli { .. } => "cli",
            BackendConfig::AsyncPoll { .. } => "async_poll",
        }
    }
}

impl std::fmt::Debug for ModelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ModelEntry");
        s.field("model_id", &self.model_id)
            .field("provider", &self.provider);

        match &self.backend {
            BackendConfig::Http {
                base_url,
                api_format,
                ..
            } => {
                s.field("backend", &"http")
                    .field("base_url", base_url)
                    .field("api_format", api_format)
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
            BackendConfig::AsyncPoll { provider_type, .. } => {
                s.field("backend", &"async_poll")
                    .field("provider_type", provider_type)
                    .field("api_key", &"[REDACTED]");
            }
        }

        s.field("description", &self.description)
            .field("speed_tier", &self.speed_tier)
            .field("precision_tier", &self.precision_tier);

        s.finish()
    }
}

pub struct Registry {
    models: HashMap<String, ModelEntry>,
    http: HttpDispatch,
    cli: CliDispatch,
    async_poll: AsyncPollDispatch,
    cli_semaphore: Semaphore,
    http_semaphore: Semaphore,
    async_poll_semaphore: Semaphore,
    persist_raw_output: PersistRawOutput,
}

impl Registry {
    pub fn from_config(config: Config) -> Self {
        Self {
            models: config.models,
            http: HttpDispatch::new(),
            cli: CliDispatch::new(),
            async_poll: AsyncPollDispatch::new(),
            cli_semaphore: Semaphore::new(CLI_MAX_CONCURRENT),
            http_semaphore: Semaphore::new(HTTP_MAX_CONCURRENT),
            async_poll_semaphore: Semaphore::new(ASYNC_POLL_MAX_CONCURRENT),
            persist_raw_output: config.persist_raw_output,
        }
    }

    /// Returns the number of CLI semaphore permits (for testing).
    pub fn cli_semaphore_permits(&self) -> usize {
        self.cli_semaphore.available_permits()
    }

    /// Returns the number of HTTP semaphore permits (for testing).
    pub fn http_semaphore_permits(&self) -> usize {
        self.http_semaphore.available_permits()
    }

    pub fn get(&self, model: &str) -> Option<&ModelEntry> {
        self.models.get(model)
    }

    pub fn list_models(&self) -> Vec<(&String, &ModelEntry)> {
        self.models.iter().collect()
    }

    /// Returns a map of model_id → config_key for model identity normalization.
    /// Used by memory subsystem to normalize event log entries that may use
    /// provider model_ids instead of config keys.
    pub fn model_id_to_key(&self) -> HashMap<String, String> {
        self.models
            .iter()
            .map(|(key, entry)| (entry.model_id.clone(), key.clone()))
            .collect()
    }

    /// Suggest similar model names for a failed lookup (substring match).
    /// Sorted alphabetically, capped at 5 to keep error messages readable.
    pub fn suggest_models(&self, query: &str) -> Vec<String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        let mut suggestions: Vec<String> = self
            .models
            .keys()
            .filter(|k| {
                let k_lower = k.to_lowercase();
                k_lower.contains(&q) || q.contains(&k_lower)
            })
            .cloned()
            .collect();
        suggestions.sort();
        suggestions.truncate(5);
        suggestions
    }

    /// Resolve the appropriate parser for a CLI provider.
    /// Returns an error for unknown providers instead of silently falling back.
    pub fn parser_for(provider: &str) -> Result<Box<dyn OutputParser>, SquallError> {
        match provider {
            "gemini" => Ok(Box::new(GeminiParser)),
            "codex" => Ok(Box::new(CodexParser)),
            _ => Err(SquallError::ModelNotFound {
                model: format!("no parser for CLI provider: {provider}"),
                suggestions: vec![],
            }),
        }
    }

    /// Acquire a semaphore permit with a deadline-aware timeout.
    /// Returns Timeout if the deadline expires before a permit is available.
    async fn acquire_with_deadline(
        semaphore: &Semaphore,
        deadline: Instant,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, SquallError> {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .ok_or(SquallError::Timeout(0))?;

        tokio::time::timeout(timeout, semaphore.acquire())
            .await
            .map_err(|_| SquallError::Timeout(0))?
            .map_err(|_| SquallError::Other("semaphore closed".to_string()))
    }

    pub async fn query(&self, req: &ProviderRequest) -> Result<ProviderResult, SquallError> {
        let entry = self
            .models
            .get(&req.model)
            .ok_or_else(|| {
                let suggestions = self.suggest_models(&req.model);
                SquallError::ModelNotFound {
                    model: req.model.clone(),
                    suggestions,
                }
            })?;

        // Substitute the provider's model_id for the Squall model name.
        // e.g. "kimi-k2.5" → "moonshotai/Kimi-K2.5" for the API request body.
        let resolved = ProviderRequest {
            model: entry.model_id.clone(),
            ..(*req).clone()
        };
        let req = &resolved;

        match &entry.backend {
            BackendConfig::Http {
                base_url,
                api_key,
                api_format,
            } => {
                let _permit = Self::acquire_with_deadline(&self.http_semaphore, req.deadline).await?;
                self.http
                    .query_model(req, &entry.provider, base_url, api_key, api_format)
                    .await
            }
            BackendConfig::Cli {
                executable,
                args_template,
            } => {
                let parser = Self::parser_for(&entry.provider)?;
                let _permit = Self::acquire_with_deadline(&self.cli_semaphore, req.deadline).await?;
                self.cli
                    .query_model(
                        req,
                        &entry.provider,
                        executable,
                        args_template,
                        &*parser,
                        self.persist_raw_output,
                    )
                    .await
            }
            BackendConfig::AsyncPoll {
                provider_type,
                api_key,
            } => {
                let _permit =
                    Self::acquire_with_deadline(&self.async_poll_semaphore, req.deadline).await?;
                self.async_poll
                    .query_model(req, &entry.provider, provider_type, api_key)
                    .await
            }
        }
    }
}
