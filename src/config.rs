use std::collections::HashMap;
use std::env;

use crate::dispatch::registry::{AsyncPollProviderType, BackendConfig, ModelEntry};

pub struct Config {
    pub models: HashMap<String, ModelEntry>,
}

impl Config {
    pub fn from_env() -> Self {
        let xai_key = env::var("XAI_API_KEY").ok();
        let openrouter_key = env::var("OPENROUTER_API_KEY").ok();

        let mut models = HashMap::new();

        // --- HTTP models ---

        if let Some(key) = xai_key {
            models.insert(
                "grok-4-1-fast-reasoning".to_string(),
                ModelEntry {
                    model_id: "grok-4-1-fast-reasoning".to_string(),
                    provider: "xai".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://api.x.ai/v1/chat/completions".to_string(),
                        api_key: key,
                    },
                },
            );
        } else {
            tracing::warn!("XAI_API_KEY not set — grok models unavailable");
        }

        if let Some(key) = openrouter_key {
            let base_url = "https://openrouter.ai/api/v1/chat/completions".to_string();

            models.insert(
                "moonshotai/kimi-k2.5".to_string(),
                ModelEntry {
                    model_id: "moonshotai/kimi-k2.5".to_string(),
                    provider: "openrouter".to_string(),
                    backend: BackendConfig::Http {
                        base_url: base_url.clone(),
                        api_key: key.clone(),
                    },
                },
            );

            models.insert(
                "z-ai/glm-5".to_string(),
                ModelEntry {
                    model_id: "z-ai/glm-5".to_string(),
                    provider: "openrouter".to_string(),
                    backend: BackendConfig::Http {
                        base_url,
                        api_key: key,
                    },
                },
            );
        } else {
            tracing::warn!("OPENROUTER_API_KEY not set — openrouter models unavailable");
        }

        // --- CLI models ---
        // Gemini CLI: uses Google OAuth (free 1000 req/day), no API key needed
        if which_exists("gemini") {
            models.insert(
                "gemini".to_string(),
                ModelEntry {
                    model_id: "gemini".to_string(),
                    provider: "gemini".to_string(),
                    backend: BackendConfig::Cli {
                        executable: "gemini".to_string(),
                        args_template: vec![
                            "-m".to_string(),
                            "gemini-3-pro-preview".to_string(),
                            "-o".to_string(),
                            "json".to_string(),
                        ],
                    },
                },
            );
        } else {
            tracing::warn!("gemini CLI not found in PATH — gemini unavailable");
        }

        // Codex CLI: uses OpenAI auth
        if which_exists("codex") {
            models.insert(
                "codex".to_string(),
                ModelEntry {
                    model_id: "codex".to_string(),
                    provider: "codex".to_string(),
                    backend: BackendConfig::Cli {
                        executable: "codex".to_string(),
                        args_template: vec![
                            "exec".to_string(),
                            "--json".to_string(),
                        ],
                    },
                },
            );
        } else {
            tracing::warn!("codex CLI not found in PATH — codex unavailable");
        }

        // --- Async-poll models (deep research) ---

        // OpenAI Responses API: OPENAI_API_KEY is separate from Codex CLI (which uses consumer auth)
        if let Ok(key) = env::var("OPENAI_API_KEY") {
            models.insert(
                "o3-deep-research".to_string(),
                ModelEntry {
                    model_id: "o3-deep-research".to_string(),
                    provider: "openai".to_string(),
                    backend: BackendConfig::AsyncPoll {
                        provider_type: AsyncPollProviderType::OpenAiResponses,
                        api_key: key.clone(),
                    },
                },
            );
            models.insert(
                "o4-mini-deep-research".to_string(),
                ModelEntry {
                    model_id: "o4-mini-deep-research".to_string(),
                    provider: "openai".to_string(),
                    backend: BackendConfig::AsyncPoll {
                        provider_type: AsyncPollProviderType::OpenAiResponses,
                        api_key: key,
                    },
                },
            );
        } else {
            tracing::warn!("OPENAI_API_KEY not set — deep research models unavailable");
        }

        // Gemini Interactions API: GOOGLE_API_KEY is separate from Gemini CLI (which uses OAuth)
        if let Ok(key) = env::var("GOOGLE_API_KEY") {
            models.insert(
                "deep-research-pro".to_string(),
                ModelEntry {
                    model_id: "deep-research-pro-preview-12-2025".to_string(),
                    provider: "gemini-api".to_string(),
                    backend: BackendConfig::AsyncPoll {
                        provider_type: AsyncPollProviderType::GeminiInteractions,
                        api_key: key,
                    },
                },
            );
        } else {
            tracing::warn!("GOOGLE_API_KEY not set — Gemini deep research unavailable");
        }

        if models.is_empty() {
            tracing::error!("no models configured — no models available");
        }

        Config { models }
    }
}

/// Check if an executable exists in PATH.
fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
