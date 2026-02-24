use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use serde::Deserialize;

use crate::dispatch::registry::{ApiFormat, AsyncPollProviderType, BackendConfig, ModelEntry};

// ---------------------------------------------------------------------------
// TOML schema types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct TomlConfig {
    #[serde(default)]
    providers: HashMap<String, TomlProvider>,
    #[serde(default)]
    models: HashMap<String, TomlModel>,
    #[serde(default)]
    settings: TomlSettings,
    #[serde(default)]
    review: TomlReviewConfig,
    #[cfg(feature = "global-memory")]
    #[serde(default)]
    global_memory: TomlGlobalMemoryConfig,
}

#[derive(Deserialize, Clone, Default)]
struct TomlSettings {
    #[serde(default)]
    persist_raw_output: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
struct TomlReviewConfig {
    /// Models dispatched when caller omits `models`. Claude adds more via the skill.
    #[serde(default)]
    default_models: Option<Vec<String>>,
}

#[cfg(feature = "global-memory")]
#[derive(Deserialize, Clone, Default)]
struct TomlGlobalMemoryConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    db_path: Option<String>,
}

#[derive(Deserialize, Clone)]
struct TomlProvider {
    base_url: String,
    api_key_env: String,
    #[serde(default)]
    api_format: Option<String>,
}

#[derive(Deserialize, Clone)]
struct TomlModel {
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    backend: String,
    // CLI-specific
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    args_template: Option<Vec<String>>,
    // AsyncPoll-specific
    #[serde(default)]
    async_poll_type: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    // Metadata
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    strengths: Option<Vec<String>>,
    #[serde(default)]
    weaknesses: Option<Vec<String>>,
    #[serde(default)]
    speed_tier: Option<String>,
    #[serde(default)]
    precision_tier: Option<String>,
}

impl TomlConfig {
    /// Merge another config on top of this one.
    /// Models: later layer fully replaces earlier entry with same name.
    /// Providers: later layer fully replaces earlier entry with same name.
    fn merge(&mut self, other: TomlConfig) {
        for (k, v) in other.providers {
            self.providers.insert(k, v);
        }
        for (k, v) in other.models {
            self.models.insert(k, v);
        }
        // Settings: later layer overrides if explicitly set
        if other.settings.persist_raw_output.is_some() {
            self.settings.persist_raw_output = other.settings.persist_raw_output;
        }
        // Review config: later layer overrides if explicitly set
        if other.review.default_models.is_some() {
            self.review.default_models = other.review.default_models;
        }
        // Global memory config: later layer overrides if explicitly set
        #[cfg(feature = "global-memory")]
        {
            if other.global_memory.enabled.is_some() {
                self.global_memory.enabled = other.global_memory.enabled;
            }
            if other.global_memory.db_path.is_some() {
                self.global_memory.db_path = other.global_memory.db_path;
            }
        }
    }

    /// Resolve TOML config into runtime Config by reading env vars and
    /// checking CLI tool availability.
    fn resolve(self) -> Config {
        let mut models = HashMap::new();
        let mut skipped: Vec<String> = Vec::new();

        for (name, model) in self.models {
            // Check env-var disable: SQUALL_MODEL_<NAME>_DISABLED=1
            let disable_key = format!(
                "SQUALL_MODEL_{}_DISABLED",
                name.to_uppercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect::<String>()
            );
            if env::var(&disable_key).is_ok_and(|v| v == "1") {
                tracing::info!("model {name} disabled via {disable_key}");
                continue;
            }

            let model_id = model.model_id.unwrap_or_else(|| name.clone());

            // Macro to skip a model and record the reason
            macro_rules! skip {
                ($reason:expr) => {{
                    let msg = format!("{name}: {}", $reason);
                    tracing::warn!("model {msg}");
                    skipped.push(msg);
                    continue;
                }};
            }

            let entry = match model.backend.as_str() {
                "http" => {
                    let provider_name = match &model.provider {
                        Some(p) => p,
                        None => skip!("http backend requires 'provider'"),
                    };
                    let provider = match self.providers.get(provider_name) {
                        Some(p) => p,
                        None => skip!(format!("provider '{provider_name}' not defined")),
                    };
                    // Model-level api_key_env overrides provider-level
                    let key_env = model
                        .api_key_env
                        .as_deref()
                        .unwrap_or(&provider.api_key_env);
                    let api_key = match env::var(key_env) {
                        Ok(k) if !k.trim().is_empty() => k,
                        _ => skip!(format!("{key_env} not set or empty")),
                    };
                    let api_format = match provider.api_format.as_deref().unwrap_or("openai") {
                        "openai" => ApiFormat::OpenAi,
                        "anthropic" => ApiFormat::Anthropic,
                        other => skip!(format!("unknown api_format '{other}'")),
                    };
                    ModelEntry {
                        model_id,
                        provider: provider_name.clone(),
                        backend: BackendConfig::Http {
                            base_url: provider.base_url.clone(),
                            api_key,
                            api_format,
                        },
                        description: model.description.unwrap_or_default(),
                        strengths: model.strengths.unwrap_or_default(),
                        weaknesses: model.weaknesses.unwrap_or_default(),
                        speed_tier: model.speed_tier.unwrap_or_else(|| "medium".to_string()),
                        precision_tier: model
                            .precision_tier
                            .unwrap_or_else(|| "medium".to_string()),
                    }
                }
                "cli" => {
                    let executable = model.executable.unwrap_or_else(|| name.clone());
                    if !which_exists(&executable) {
                        skip!(format!("{executable} not found in PATH"));
                    }
                    let cli_provider = model.provider.unwrap_or_else(|| name.clone());
                    // Validate that a parser exists for this CLI provider
                    if !matches!(cli_provider.as_str(), "gemini" | "codex") {
                        skip!(format!(
                            "no parser for CLI provider '{cli_provider}' \
                             (supported: gemini, codex)"
                        ));
                    }
                    let args = model.args_template.unwrap_or_default();
                    ModelEntry {
                        model_id,
                        provider: cli_provider,
                        backend: BackendConfig::Cli {
                            executable,
                            args_template: args,
                        },
                        description: model.description.unwrap_or_default(),
                        strengths: model.strengths.unwrap_or_default(),
                        weaknesses: model.weaknesses.unwrap_or_default(),
                        speed_tier: model.speed_tier.unwrap_or_else(|| "medium".to_string()),
                        precision_tier: model
                            .precision_tier
                            .unwrap_or_else(|| "medium".to_string()),
                    }
                }
                "async_poll" => {
                    let key_env = match &model.api_key_env {
                        Some(k) => k.as_str(),
                        None => skip!("async_poll backend requires 'api_key_env'"),
                    };
                    let api_key = match env::var(key_env) {
                        Ok(k) if !k.trim().is_empty() => k,
                        _ => skip!(format!("{key_env} not set or empty")),
                    };
                    let provider_type = match model.async_poll_type.as_deref().unwrap_or("") {
                        "openai_responses" => AsyncPollProviderType::OpenAiResponses,
                        "gemini_interactions" => AsyncPollProviderType::GeminiInteractions,
                        other => skip!(format!("unknown async_poll_type '{other}'")),
                    };
                    ModelEntry {
                        model_id,
                        provider: model.provider.unwrap_or_else(|| name.clone()),
                        backend: BackendConfig::AsyncPoll {
                            provider_type,
                            api_key,
                        },
                        description: model.description.unwrap_or_default(),
                        strengths: model.strengths.unwrap_or_default(),
                        weaknesses: model.weaknesses.unwrap_or_default(),
                        speed_tier: model.speed_tier.unwrap_or_else(|| "very_slow".to_string()),
                        precision_tier: model
                            .precision_tier
                            .unwrap_or_else(|| "medium".to_string()),
                    }
                }
                other => skip!(format!("unknown backend '{other}'")),
            };

            models.insert(name, entry);
        }

        if !skipped.is_empty() {
            tracing::warn!("skipped {} model(s): {}", skipped.len(), skipped.join(", "));
        }
        if models.is_empty() {
            tracing::error!("no models configured — set API keys or check config");
        }

        // Parse persist_raw_output setting
        let persist_raw_output = match self.settings.persist_raw_output.as_deref() {
            Some(val) => match PersistRawOutput::from_str_validated(val) {
                Some(mode) => mode,
                None => {
                    tracing::warn!(
                        "unknown persist_raw_output value '{val}', \
                         using default 'on_failure'"
                    );
                    PersistRawOutput::default()
                }
            },
            None => PersistRawOutput::default(),
        };

        // Parse review config
        let review = ReviewConfig {
            default_models: self
                .review
                .default_models
                .unwrap_or_else(|| ReviewConfig::default().default_models),
        };

        // Parse global memory config
        #[cfg(feature = "global-memory")]
        let global_memory = {
            let defaults = GlobalMemoryConfig::default();
            GlobalMemoryConfig {
                enabled: self.global_memory.enabled.unwrap_or(defaults.enabled),
                db_path: self.global_memory.db_path.unwrap_or(defaults.db_path),
            }
        };

        Config {
            models,
            skipped,
            persist_raw_output,
            review,
            #[cfg(feature = "global-memory")]
            global_memory,
        }
    }
}

// ---------------------------------------------------------------------------
// Public Config type (unchanged — Registry, server, tests all use this)
// ---------------------------------------------------------------------------

/// When to persist raw CLI output to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersistRawOutput {
    /// Always persist raw output for every CLI invocation.
    Always,
    /// Persist only when the CLI command fails (non-zero exit or parse error).
    #[default]
    OnFailure,
    /// Never persist raw output.
    Never,
}

impl PersistRawOutput {
    fn from_str_validated(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "always" => Some(Self::Always),
            "on_failure" => Some(Self::OnFailure),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Review dispatch defaults. Claude (the MCP client) handles intelligent model
/// selection via the unified review skill — this just sets the fallback when
/// `models` is omitted from a review request.
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    /// Models dispatched when caller omits `models`. Default: ["gemini", "codex", "grok"].
    pub default_models: Vec<String>,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            default_models: vec!["gemini".into(), "codex".into(), "grok".into()],
        }
    }
}

/// Cross-project global memory configuration.
#[cfg(feature = "global-memory")]
#[derive(Debug, Clone)]
pub struct GlobalMemoryConfig {
    /// Whether global memory is enabled. Default: true when feature is compiled in.
    pub enabled: bool,
    /// Path to the DuckDB database file.
    /// Default: `~/.squall/memory/global/global.duckdb`.
    pub db_path: String,
}

#[cfg(feature = "global-memory")]
impl Default for GlobalMemoryConfig {
    fn default() -> Self {
        let db_path = std::env::var("HOME")
            .map(|home| format!("{home}/.squall/memory/global/global.duckdb"))
            .unwrap_or_else(|_| ".squall/memory/global/global.duckdb".to_string());
        Self {
            enabled: true,
            db_path,
        }
    }
}

#[derive(Default)]
pub struct Config {
    pub models: HashMap<String, ModelEntry>,
    /// Models that were defined but failed to resolve (missing key, missing CLI, etc.).
    /// Each entry is a human-readable reason string like "grok: XAI_API_KEY not set".
    pub skipped: Vec<String>,
    /// When to persist raw CLI output to `.squall/raw/`.
    pub persist_raw_output: PersistRawOutput,
    /// Tiered model selection for automatic review dispatch.
    pub review: ReviewConfig,
    /// Cross-project global memory settings (DuckDB-backed).
    #[cfg(feature = "global-memory")]
    pub global_memory: GlobalMemoryConfig,
}

impl Config {
    /// Load config with layered merge:
    /// 1. Built-in defaults (BUILTIN_DEFAULTS)
    /// 2. User config (~/.config/squall/config.toml)
    /// 3. Project config (.squall/config.toml)
    /// 4. Env var overrides (SQUALL_MODEL_<NAME>_DISABLED=1)
    pub fn load() -> Self {
        let mut config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS)
            .expect("BUILTIN_DEFAULTS is invalid TOML — this is a build bug");

        // User config
        if let Some(path) = user_config_path()
            && path.exists()
        {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<TomlConfig>(&contents) {
                    Ok(user) => {
                        tracing::info!("loaded user config from {}", path.display());
                        config.merge(user);
                    }
                    Err(e) => {
                        tracing::warn!("failed to parse {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    tracing::warn!("failed to read {}: {e}", path.display());
                }
            }
        }

        // Project config — walk up from CWD to find .squall/config.toml
        if let Some(project_path) = find_project_config_from_cwd() {
            match std::fs::read_to_string(&project_path) {
                Ok(contents) => match toml::from_str::<TomlConfig>(&contents) {
                    Ok(project) => {
                        tracing::info!("loaded project config from {}", project_path.display());
                        config.merge(project);
                    }
                    Err(e) => {
                        tracing::warn!("failed to parse {}: {e}", project_path.display());
                    }
                },
                Err(e) => {
                    tracing::warn!("failed to read {}: {e}", project_path.display());
                }
            }
        }

        config.resolve()
    }

    /// Backward-compatible alias for `load()`.
    pub fn from_env() -> Self {
        Self::load()
    }

    /// Load config from a TOML string (for testing).
    #[cfg(test)]
    pub fn from_toml(toml_str: &str) -> Self {
        let config: TomlConfig = toml::from_str(toml_str).expect("invalid TOML in test");
        config.resolve()
    }
}

// ---------------------------------------------------------------------------
// Built-in defaults — all 12 models as TOML
// ---------------------------------------------------------------------------

const BUILTIN_DEFAULTS: &str = r#"
# --- Settings ---

[settings]
persist_raw_output = "on_failure"

# --- Providers ---

[providers.xai]
base_url = "https://api.x.ai/v1/chat/completions"
api_key_env = "XAI_API_KEY"

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1/chat/completions"
api_key_env = "OPENROUTER_API_KEY"

[providers.deepseek]
base_url = "https://api.deepseek.com/chat/completions"
api_key_env = "DEEPSEEK_API_KEY"

[providers.mistral]
base_url = "https://api.mistral.ai/v1/chat/completions"
api_key_env = "MISTRAL_API_KEY"

[providers.together]
base_url = "https://api.together.xyz/v1/chat/completions"
api_key_env = "TOGETHER_API_KEY"

# --- HTTP models ---

[models.grok]
model_id = "grok-4-1-fast-reasoning"
provider = "xai"
backend = "http"
description = "xAI's fast reasoning model, good for quick code review"
speed_tier = "fast"
precision_tier = "medium"
strengths = ["fast responses", "broad knowledge"]
weaknesses = ["XML escaping false positives", "edition 2024 false positives"]

[models."z-ai/glm-5"]
model_id = "z-ai/glm-5"
provider = "openrouter"
backend = "http"
description = "Zhipu's GLM-5, architectural framing via OpenRouter"
speed_tier = "medium"
precision_tier = "low"
strengths = ["clear architectural analysis", "structured output"]
weaknesses = ["rarely finds real bugs", "surface-level findings"]

[models.deepseek-r1]
model_id = "deepseek-ai/DeepSeek-R1"
provider = "together"
backend = "http"
description = "DeepSeek R1 reasoning model via Together (US-hosted), strong at logic-heavy analysis"
speed_tier = "medium"
precision_tier = "medium"
strengths = ["deep reasoning chains", "logic analysis"]
weaknesses = ["verbose output", "slow on complex prompts"]

[models.mistral-large]
model_id = "mistral-large-latest"
provider = "mistral"
backend = "http"
description = "Mistral Large, efficient European model with code expertise"
speed_tier = "fast"
precision_tier = "medium"
strengths = ["efficient token usage", "multilingual code review"]
weaknesses = ["less depth on niche Rust patterns"]

[models."kimi-k2.5"]
model_id = "moonshotai/Kimi-K2.5"
provider = "together"
backend = "http"
description = "Moonshot's Kimi K2.5 via Together (US-hosted), contrarian edge case reviewer"
speed_tier = "medium"
precision_tier = "medium"
strengths = ["contrarian perspective", "edge case detection"]
weaknesses = ["inconsistent quality"]

[models."deepseek-v3.1"]
model_id = "deepseek-ai/DeepSeek-V3.1"
provider = "together"
backend = "http"
description = "DeepSeek V3.1 via Together (US-hosted), strong open-source coder"
speed_tier = "medium"
precision_tier = "high"
strengths = ["strong reasoning", "finds real bugs"]
weaknesses = ["verbose output"]

[models."qwen-3.5"]
model_id = "Qwen/Qwen3.5-397B-A17B"
provider = "together"
backend = "http"
description = "Alibaba's Qwen 3.5 397B MoE via Together, strong multilingual code model"
speed_tier = "medium"
precision_tier = "medium"
strengths = ["multilingual understanding", "good at pattern matching"]
weaknesses = ["sometimes misses context"]

[models.qwen3-coder]
model_id = "Qwen/Qwen3-Coder-480B-A35B-Instruct"
provider = "together"
backend = "http"
description = "Qwen3 Coder 480B via Together, purpose-built for code review and generation"
speed_tier = "medium"
precision_tier = "high"
strengths = ["purpose-built for code", "strong at code review", "large context"]
weaknesses = ["new model, limited benchmarks"]

# --- CLI models ---

[models.gemini]
model_id = "gemini"
provider = "gemini"
backend = "cli"
executable = "gemini"
args_template = ["-m", "gemini-3-pro-preview", "-o", "json"]
description = "Google Gemini CLI, best at systems-level bug detection"
speed_tier = "medium"
precision_tier = "high"
strengths = ["systems-level bugs", "finds all real bugs"]
weaknesses = ["slower than HTTP models"]

[models.codex]
model_id = "codex"
provider = "codex"
backend = "cli"
executable = "codex"
args_template = ["exec", "--json"]
description = "OpenAI Codex CLI, highest precision with zero false positives"
speed_tier = "slow"
precision_tier = "high"
strengths = ["highest precision", "zero false positives", "exact line references"]
weaknesses = ["variable speed (50-300s)"]

# --- Async-poll models (deep research) ---

[models.o3-deep-research]
model_id = "o3-deep-research"
provider = "openai"
backend = "async_poll"
async_poll_type = "openai_responses"
api_key_env = "OPENAI_API_KEY"
description = "OpenAI o3 deep research, long-running web research"
speed_tier = "very_slow"
precision_tier = "high"
strengths = ["deep web research", "comprehensive analysis"]
weaknesses = ["very slow (minutes)", "expensive"]

[models.o4-mini-deep-research]
model_id = "o4-mini-deep-research"
provider = "openai"
backend = "async_poll"
async_poll_type = "openai_responses"
api_key_env = "OPENAI_API_KEY"
description = "OpenAI o4-mini deep research, faster variant of deep research"
speed_tier = "very_slow"
precision_tier = "medium"
strengths = ["faster than o3-deep-research", "good cost-quality tradeoff"]
weaknesses = ["still slow (minutes)", "less thorough than o3"]

[models.deep-research-pro]
model_id = "deep-research-pro-preview-12-2025"
provider = "gemini-api"
backend = "async_poll"
async_poll_type = "gemini_interactions"
api_key_env = "GOOGLE_API_KEY"
description = "Google Gemini deep research via Interactions API"
speed_tier = "very_slow"
precision_tier = "high"
strengths = ["comprehensive research", "Google search integration"]
weaknesses = ["very slow (minutes to hour)", "may need background job registry"]

# --- Review defaults ---

[review]
default_models = ["gemini", "codex", "grok"]
"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// XDG-compliant user config path: ~/.config/squall/config.toml
fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg).join("squall/config.toml"))
    } else if let Ok(home) = env::var("HOME") {
        Some(PathBuf::from(home).join(".config/squall/config.toml"))
    } else {
        None
    }
}

/// Walk up from `start` looking for `.squall/config.toml`.
/// Returns the first match, or None if the filesystem root is reached.
fn find_project_config(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".squall/config.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk up from the current working directory.
fn find_project_config_from_cwd() -> Option<PathBuf> {
    env::current_dir()
        .ok()
        .and_then(|cwd| find_project_config(&cwd))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_defaults_parse() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        assert!(config.providers.contains_key("xai"));
        assert!(config.providers.contains_key("together"));
        assert!(config.providers.contains_key("deepseek"));
        assert!(config.providers.contains_key("mistral"));
        assert!(config.providers.contains_key("openrouter"));
        assert_eq!(config.providers.len(), 5);
        assert_eq!(config.models.len(), 13);
        assert!(config.models.contains_key("grok"));
        assert!(config.models.contains_key("gemini"));
        assert!(config.models.contains_key("codex"));
        assert!(config.models.contains_key("o3-deep-research"));
        assert!(config.models.contains_key("deep-research-pro"));
    }

    #[test]
    fn builtin_grok_model_id_is_correct() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        let grok = &config.models["grok"];
        assert_eq!(grok.model_id.as_deref(), Some("grok-4-1-fast-reasoning"));
        assert_eq!(grok.provider.as_deref(), Some("xai"));
        assert_eq!(grok.backend, "http");
    }

    #[test]
    fn merge_overrides_model() {
        let mut base: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        let overlay: TomlConfig = toml::from_str(
            r#"
            [models.grok]
            model_id = "grok-custom"
            provider = "xai"
            backend = "http"
            "#,
        )
        .unwrap();
        base.merge(overlay);
        assert_eq!(base.models["grok"].model_id.as_deref(), Some("grok-custom"));
    }

    #[test]
    fn merge_adds_new_provider_and_model() {
        let mut base: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        let overlay: TomlConfig = toml::from_str(
            r#"
            [providers.custom]
            base_url = "https://custom.api.com/v1/chat/completions"
            api_key_env = "CUSTOM_API_KEY"

            [models.custom-model]
            model_id = "custom-v1"
            provider = "custom"
            backend = "http"
            "#,
        )
        .unwrap();
        let old_model_count = base.models.len();
        base.merge(overlay);
        assert!(base.providers.contains_key("custom"));
        assert!(base.models.contains_key("custom-model"));
        assert_eq!(base.models.len(), old_model_count + 1);
    }

    #[test]
    fn resolve_skips_model_with_missing_api_key() {
        let config: TomlConfig = toml::from_str(
            r#"
            [providers.fake]
            base_url = "https://fake.com/v1"
            api_key_env = "SQUALL_TEST_NONEXISTENT_KEY_12345"

            [models.fake-model]
            provider = "fake"
            backend = "http"
            "#,
        )
        .unwrap();
        let resolved = config.resolve();
        assert!(
            !resolved.models.contains_key("fake-model"),
            "Model with missing API key should be skipped"
        );
    }

    #[test]
    fn resolve_http_model_with_env_key() {
        let key = "SQUALL_TEST_RESOLVE_KEY_HTTP";
        unsafe {
            env::set_var(key, "test-secret");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.testprov]
            base_url = "https://test.com/v1"
            api_key_env = "{key}"

            [models.test-model]
            model_id = "test-v1"
            provider = "testprov"
            backend = "http"
            description = "a test model"
            speed_tier = "fast"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        let entry = resolved
            .models
            .get("test-model")
            .expect("model should exist");
        assert_eq!(entry.model_id, "test-v1");
        assert_eq!(entry.provider, "testprov");
        assert_eq!(entry.speed_tier, "fast");
        assert!(matches!(entry.backend, BackendConfig::Http { .. }));
        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn resolve_model_id_defaults_to_name() {
        let key = "SQUALL_TEST_RESOLVE_KEY_DEFAULT_ID";
        unsafe {
            env::set_var(key, "secret");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.p]
            base_url = "https://p.com/v1"
            api_key_env = "{key}"

            [models.my-model]
            provider = "p"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        let entry = resolved.models.get("my-model").unwrap();
        assert_eq!(
            entry.model_id, "my-model",
            "model_id should default to the model name"
        );
        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn resolve_disable_via_env() {
        let key = "SQUALL_TEST_RESOLVE_KEY_DISABLE";
        unsafe {
            env::set_var(key, "secret");
            env::set_var("SQUALL_MODEL_DISABLED_MODEL_DISABLED", "1");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.p]
            base_url = "https://p.com/v1"
            api_key_env = "{key}"

            [models.disabled-model]
            provider = "p"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        assert!(
            !resolved.models.contains_key("disabled-model"),
            "Model should be disabled via SQUALL_MODEL_DISABLED_MODEL_DISABLED=1"
        );
        unsafe {
            env::remove_var(key);
            env::remove_var("SQUALL_MODEL_DISABLED_MODEL_DISABLED");
        }
    }

    #[test]
    fn resolve_async_poll_model() {
        let key = "SQUALL_TEST_RESOLVE_KEY_ASYNC";
        unsafe {
            env::set_var(key, "secret");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [models.test-research]
            model_id = "test-research-v1"
            provider = "openai"
            backend = "async_poll"
            async_poll_type = "openai_responses"
            api_key_env = "{key}"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        let entry = resolved
            .models
            .get("test-research")
            .expect("async_poll model should exist");
        assert!(matches!(entry.backend, BackendConfig::AsyncPoll { .. }));
        unsafe {
            env::remove_var(key);
        }
    }

    #[test]
    fn resolve_unknown_backend_skipped() {
        let config: TomlConfig = toml::from_str(
            r#"
            [models.bad]
            backend = "quantum"
            "#,
        )
        .unwrap();
        let resolved = config.resolve();
        assert!(!resolved.models.contains_key("bad"));
    }

    #[test]
    fn empty_toml_produces_empty_config() {
        let config: TomlConfig = toml::from_str("").unwrap();
        let resolved = config.resolve();
        assert!(resolved.models.is_empty());
    }

    #[test]
    fn from_toml_convenience() {
        let key = "SQUALL_TEST_FROM_TOML";
        unsafe {
            env::set_var(key, "secret");
        }
        let config = Config::from_toml(&format!(
            r#"
            [providers.t]
            base_url = "https://t.com/v1"
            api_key_env = "{key}"

            [models.t-model]
            provider = "t"
            backend = "http"
            "#
        ));
        assert!(config.models.contains_key("t-model"));
        unsafe {
            env::remove_var(key);
        }
    }

    // -----------------------------------------------------------------------
    // RED tests — proving defects found by 5-model Squall review
    // -----------------------------------------------------------------------

    /// P0: Model names containing '/' produce invalid env var names for disable.
    /// "z-ai/glm-5" → "SQUALL_MODEL_Z_AI/GLM_5_DISABLED" — the '/' is not replaced.
    /// Env vars with '/' are non-portable and won't work in most shells.
    #[test]
    fn p0_slash_in_model_name_sanitized_for_disable_env() {
        let key = "SQUALL_TEST_P0_SLASH_KEY";
        // Set the disable env var with the CORRECT (sanitized) name
        unsafe {
            env::set_var(key, "secret");
            // If sanitization works, the disable key should be:
            // SQUALL_MODEL_Z_AI_GLM_5_DISABLED (slash → underscore)
            env::set_var("SQUALL_MODEL_Z_AI_GLM_5_DISABLED", "1");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.p]
            base_url = "https://p.com/v1"
            api_key_env = "{key}"

            [models."z-ai/glm-5"]
            provider = "p"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        // The model should be disabled because we set the sanitized env var
        assert!(
            !resolved.models.contains_key("z-ai/glm-5"),
            "Model with '/' in name should be disabled via sanitized env var \
             SQUALL_MODEL_Z_AI_GLM_5_DISABLED=1, but it was not"
        );
        unsafe {
            env::remove_var(key);
            env::remove_var("SQUALL_MODEL_Z_AI_GLM_5_DISABLED");
        }
    }

    /// P1: Unknown api_format silently defaults to OpenAI instead of warning.
    /// A typo like "anthrpoic" should NOT silently become OpenAI format.
    #[test]
    fn p1_unknown_api_format_is_rejected() {
        let key = "SQUALL_TEST_P1_API_FORMAT_KEY";
        unsafe {
            env::set_var(key, "secret");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.bad-format]
            base_url = "https://bad.com/v1"
            api_key_env = "{key}"
            api_format = "anthrpoic"

            [models.bad-fmt-model]
            provider = "bad-format"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        // Model with unknown api_format should be SKIPPED (not silently default to OpenAI)
        assert!(
            !resolved.models.contains_key("bad-fmt-model"),
            "Model with unknown api_format 'anthrpoic' should be skipped, \
             not silently default to OpenAI"
        );
        unsafe {
            env::remove_var(key);
        }
    }

    /// P1: Empty API key string (KEY="") is accepted and stored.
    /// An empty key will fail at the provider API, not at config time.
    #[test]
    fn p1_empty_api_key_is_rejected() {
        let key = "SQUALL_TEST_P1_EMPTY_KEY";
        unsafe {
            env::set_var(key, "");
        } // empty string
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.empty]
            base_url = "https://empty.com/v1"
            api_key_env = "{key}"

            [models.empty-key-model]
            provider = "empty"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        // Model with empty API key should be SKIPPED
        assert!(
            !resolved.models.contains_key("empty-key-model"),
            "Model with empty API key should be skipped, not accepted"
        );
        unsafe {
            env::remove_var(key);
        }
    }

    /// #2: resolve() should report skipped models in a summary, not just
    /// individual tracing::warn calls. The Config should carry a `skipped`
    /// list so callers can surface it to users.
    #[test]
    fn find2_resolve_reports_skipped_models() {
        let key = "SQUALL_TEST_F2_KEY";
        unsafe {
            env::set_var(key, "secret");
        }
        let config: TomlConfig = toml::from_str(&format!(
            r#"
            [providers.good]
            base_url = "https://good.com/v1"
            api_key_env = "{key}"

            [models.good-model]
            provider = "good"
            backend = "http"

            [models.bad-model]
            provider = "nonexistent"
            backend = "http"
            "#
        ))
        .unwrap();
        let resolved = config.resolve();
        assert!(resolved.models.contains_key("good-model"));
        assert!(!resolved.models.contains_key("bad-model"));
        // The skipped list should contain the bad model and why
        assert!(
            !resolved.skipped.is_empty(),
            "Config.skipped should report models that failed to resolve"
        );
        assert!(
            resolved.skipped.iter().any(|s| s.contains("bad-model")),
            "Skipped list should mention 'bad-model'"
        );
        unsafe {
            env::remove_var(key);
        }
    }

    /// #7: Project config should be found by walking up from CWD, not just
    /// checking ".squall/config.toml" relative to CWD.
    #[test]
    fn find7_project_config_found_from_subdirectory() {
        // find_project_config("/tmp/squall-test-f7/sub/deep") should find
        // "/tmp/squall-test-f7/.squall/config.toml"
        let base = std::path::PathBuf::from("/tmp/squall-test-f7");
        let sub = base.join("sub/deep");
        let squall_dir = base.join(".squall");
        let config_file = squall_dir.join("config.toml");
        // Setup
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&squall_dir).unwrap();
        std::fs::write(&config_file, "# test config\n").unwrap();
        // Test
        let found = find_project_config(&sub);
        assert_eq!(
            found,
            Some(config_file.clone()),
            "find_project_config should walk up to find .squall/config.toml"
        );
        // Cleanup
        let _ = std::fs::remove_dir_all(&base);
    }

    /// #8: CLI models with an unknown provider should be rejected at config
    /// resolve time, not produce a runtime error in parser_for().
    #[test]
    fn find8_cli_model_unknown_provider_rejected_at_resolve() {
        // A CLI model with provider "my-custom-cli" should be skipped
        // because parser_for() only knows "gemini" and "codex".
        let config: TomlConfig = toml::from_str(
            r#"
            [models.my-custom-cli]
            backend = "cli"
            executable = "gemini"
            provider = "unknown-cli-provider"
            "#,
        )
        .unwrap();
        let resolved = config.resolve();
        assert!(
            !resolved.models.contains_key("my-custom-cli"),
            "CLI model with unknown provider should be rejected at config time, \
             not cause a runtime error in parser_for()"
        );
    }

    // -----------------------------------------------------------------------
    // persist_raw_output setting tests
    // -----------------------------------------------------------------------

    #[test]
    fn persist_raw_output_default_is_on_failure() {
        let config = Config::from_toml("");
        assert_eq!(config.persist_raw_output, PersistRawOutput::OnFailure);
    }

    #[test]
    fn persist_raw_output_builtin_defaults_parse() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        assert_eq!(
            config.settings.persist_raw_output.as_deref(),
            Some("on_failure")
        );
    }

    #[test]
    fn persist_raw_output_all_valid_values() {
        for (input, expected) in [
            ("always", PersistRawOutput::Always),
            ("on_failure", PersistRawOutput::OnFailure),
            ("never", PersistRawOutput::Never),
        ] {
            let toml_str = format!(
                r#"
                [settings]
                persist_raw_output = "{input}"
                "#
            );
            let config = Config::from_toml(&toml_str);
            assert_eq!(
                config.persist_raw_output, expected,
                "persist_raw_output = '{input}' should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn persist_raw_output_invalid_value_falls_back_to_default() {
        let config = Config::from_toml(
            r#"
            [settings]
            persist_raw_output = "banana"
            "#,
        );
        assert_eq!(
            config.persist_raw_output,
            PersistRawOutput::OnFailure,
            "Invalid persist_raw_output value should fall back to on_failure"
        );
    }

    #[test]
    fn persist_raw_output_merge_override() {
        let mut base: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        let overlay: TomlConfig = toml::from_str(
            r#"
            [settings]
            persist_raw_output = "always"
            "#,
        )
        .unwrap();
        base.merge(overlay);
        let resolved = base.resolve();
        assert_eq!(resolved.persist_raw_output, PersistRawOutput::Always);
    }

    #[test]
    fn persist_raw_output_case_insensitive() {
        for (input, expected) in [
            ("Always", PersistRawOutput::Always),
            ("ALWAYS", PersistRawOutput::Always),
            ("ON_FAILURE", PersistRawOutput::OnFailure),
            ("On_Failure", PersistRawOutput::OnFailure),
            ("NEVER", PersistRawOutput::Never),
            ("Never", PersistRawOutput::Never),
        ] {
            let toml_str = format!(
                r#"
                [settings]
                persist_raw_output = "{input}"
                "#
            );
            let config = Config::from_toml(&toml_str);
            assert_eq!(
                config.persist_raw_output, expected,
                "'{input}' should parse case-insensitively to {expected:?}"
            );
        }
    }

    #[test]
    fn persist_raw_output_merge_preserves_base_when_overlay_omits() {
        let mut base: TomlConfig = toml::from_str(
            r#"
            [settings]
            persist_raw_output = "never"
            "#,
        )
        .unwrap();
        // Overlay with no settings section at all
        let overlay: TomlConfig = toml::from_str("").unwrap();
        base.merge(overlay);
        let resolved = base.resolve();
        assert_eq!(
            resolved.persist_raw_output,
            PersistRawOutput::Never,
            "Base setting should be preserved when overlay omits [settings]"
        );
    }
}
