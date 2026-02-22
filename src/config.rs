use std::collections::HashMap;
use std::env;

use crate::dispatch::registry::{ApiFormat, AsyncPollProviderType, BackendConfig, ModelEntry};

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
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "xAI's fast reasoning model, good for quick code review".to_string(),
                    strengths: vec!["fast responses".to_string(), "broad knowledge".to_string()],
                    weaknesses: vec!["XML escaping false positives".to_string(), "edition 2024 false positives".to_string()],
                    speed_tier: "fast".to_string(),
                    precision_tier: "medium".to_string(),
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
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "Moonshot's Kimi K2.5, contrarian reviewer with edge case focus".to_string(),
                    strengths: vec!["contrarian perspective".to_string(), "edge case detection".to_string()],
                    weaknesses: vec!["frequent timeouts at 300s".to_string(), "inconsistent quality".to_string()],
                    speed_tier: "slow".to_string(),
                    precision_tier: "medium".to_string(),
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
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "Zhipu's GLM-5, strong architectural framing".to_string(),
                    strengths: vec!["clear architectural analysis".to_string(), "structured output".to_string()],
                    weaknesses: vec!["rarely finds real bugs".to_string(), "surface-level findings".to_string()],
                    speed_tier: "medium".to_string(),
                    precision_tier: "low".to_string(),
                },
            );
        } else {
            tracing::warn!("OPENROUTER_API_KEY not set — openrouter models unavailable");
        }

        // DeepSeek R1: reasoning model, OpenAI-compatible API
        if let Ok(key) = env::var("DEEPSEEK_API_KEY") {
            models.insert(
                "deepseek-r1".to_string(),
                ModelEntry {
                    model_id: "deepseek-reasoner".to_string(),
                    provider: "deepseek".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://api.deepseek.com/chat/completions".to_string(),
                        api_key: key,
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "DeepSeek R1 reasoning model, strong at logic-heavy analysis".to_string(),
                    strengths: vec!["deep reasoning chains".to_string(), "logic analysis".to_string()],
                    weaknesses: vec!["verbose output".to_string(), "slow on complex prompts".to_string()],
                    speed_tier: "medium".to_string(),
                    precision_tier: "medium".to_string(),
                },
            );
        }

        // GPT-5: shares OPENAI_API_KEY with deep research models
        if let Ok(key) = env::var("OPENAI_API_KEY") {
            models.insert(
                "gpt-5".to_string(),
                ModelEntry {
                    model_id: "gpt-5".to_string(),
                    provider: "openai".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://api.openai.com/v1/chat/completions".to_string(),
                        api_key: key,
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "OpenAI GPT-5, general-purpose with strong code understanding".to_string(),
                    strengths: vec!["broad code understanding".to_string(), "good at refactoring suggestions".to_string()],
                    weaknesses: vec!["can be overly cautious".to_string()],
                    speed_tier: "medium".to_string(),
                    precision_tier: "high".to_string(),
                },
            );
        }

        // Mistral Large
        if let Ok(key) = env::var("MISTRAL_API_KEY") {
            models.insert(
                "mistral-large".to_string(),
                ModelEntry {
                    model_id: "mistral-large-latest".to_string(),
                    provider: "mistral".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://api.mistral.ai/v1/chat/completions".to_string(),
                        api_key: key,
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "Mistral Large, efficient European model with code expertise".to_string(),
                    strengths: vec!["efficient token usage".to_string(), "multilingual code review".to_string()],
                    weaknesses: vec!["less depth on niche Rust patterns".to_string()],
                    speed_tier: "fast".to_string(),
                    precision_tier: "medium".to_string(),
                },
            );
        }

        // Qwen 3.5 via Together AI
        if let Ok(key) = env::var("TOGETHER_API_KEY") {
            models.insert(
                "qwen-3.5".to_string(),
                ModelEntry {
                    model_id: "Qwen/Qwen3.5-72B".to_string(),
                    provider: "together".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://api.together.xyz/v1/chat/completions".to_string(),
                        api_key: key,
                        api_format: ApiFormat::OpenAi,
                    },
                    description: "Alibaba's Qwen 3.5 72B, strong multilingual code model".to_string(),
                    strengths: vec!["multilingual understanding".to_string(), "good at pattern matching".to_string()],
                    weaknesses: vec!["sometimes misses context".to_string()],
                    speed_tier: "medium".to_string(),
                    precision_tier: "medium".to_string(),
                },
            );
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
                    description: "Google Gemini CLI, best at systems-level bug detection".to_string(),
                    strengths: vec!["systems-level bugs".to_string(), "finds all real bugs".to_string()],
                    weaknesses: vec!["slower than HTTP models".to_string()],
                    speed_tier: "medium".to_string(),
                    precision_tier: "high".to_string(),
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
                    description: "OpenAI Codex CLI, highest precision with zero false positives".to_string(),
                    strengths: vec!["highest precision".to_string(), "zero false positives".to_string(), "exact line references".to_string()],
                    weaknesses: vec!["variable speed (50-300s)".to_string()],
                    speed_tier: "slow".to_string(),
                    precision_tier: "high".to_string(),
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
                    description: "OpenAI o3 deep research, long-running web research".to_string(),
                    strengths: vec!["deep web research".to_string(), "comprehensive analysis".to_string()],
                    weaknesses: vec!["very slow (minutes)".to_string(), "expensive".to_string()],
                    speed_tier: "very_slow".to_string(),
                    precision_tier: "high".to_string(),
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
                    description: "OpenAI o4-mini deep research, faster variant of deep research".to_string(),
                    strengths: vec!["faster than o3-deep-research".to_string(), "good cost-quality tradeoff".to_string()],
                    weaknesses: vec!["still slow (minutes)".to_string(), "less thorough than o3".to_string()],
                    speed_tier: "very_slow".to_string(),
                    precision_tier: "medium".to_string(),
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
                    description: "Google Gemini deep research, long-running web research via Interactions API".to_string(),
                    strengths: vec!["comprehensive research".to_string(), "Google search integration".to_string()],
                    weaknesses: vec!["very slow (minutes to hour)".to_string(), "may need background job registry".to_string()],
                    speed_tier: "very_slow".to_string(),
                    precision_tier: "high".to_string(),
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
