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
                    if !matches!(cli_provider.as_str(), "gemini" | "codex" | "claude") {
                        skip!(format!(
                            "no parser for CLI provider '{cli_provider}' \
                             (supported: gemini, codex, claude)"
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

        // Parse review config — fall back to client-mode-aware defaults so the
        // server's own fallback ensemble doesn't include the hosting client's
        // own CLI (which the recursion guard would refuse).
        let client_mode = ClientMode::from_env();
        let review = ReviewConfig {
            default_models: self
                .review
                .default_models
                .unwrap_or_else(|| ReviewConfig::defaults_for(client_mode).default_models),
        };
        // Defense-in-depth: warn if a user-configured default ensemble includes the
        // hosting client's own model. The recursion guard still fires per-call, so
        // failures are loud — but a config-time warning helps users notice the
        // mistake before they wonder why their review loses one model every time.
        if let Some(host_model) = host_model_for(client_mode)
            && review.default_models.iter().any(|m| m == host_model)
        {
            tracing::warn!(
                "[review].default_models contains '{host_model}' but SQUALL_CLIENT={} — \
                 the recursion guard will refuse it on every dispatch. Remove it from \
                 default_models or unset SQUALL_CLIENT.",
                client_mode_str(client_mode)
            );
        }

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
            client_mode,
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

impl ReviewConfig {
    /// Default ensemble adjusted for the hosting client.
    ///
    /// Codex mode: substitute `codex` → `claude` because the recursion guard refuses
    /// `clink(codex)` when SQUALL_CLIENT=codex, which would otherwise leave the server's
    /// own fallback ensemble silently broken.
    pub fn defaults_for(mode: ClientMode) -> Self {
        let default_models = match mode {
            ClientMode::Claude => vec!["gemini".into(), "codex".into(), "grok".into()],
            ClientMode::Codex => vec!["gemini".into(), "claude".into(), "grok".into()],
        };
        Self { default_models }
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

/// Which MCP client is hosting Squall. Set via `SQUALL_CLIENT` env var.
/// Affects error response shape, server instructions, and clink recursion guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientMode {
    /// Claude Code (default). Errors return as MCP success with `status:error` in payload
    /// to avoid Claude's sibling-cascade behavior.
    #[default]
    Claude,
    /// OpenAI Codex CLI. Errors return as MCP `is_error: true` (spec-compliant).
    Codex,
}

impl ClientMode {
    /// Resolve the client mode from the `SQUALL_CLIENT` env var.
    /// Defaults to `Claude` when unset, empty, or set to any value other than `"codex"`.
    pub fn from_env() -> Self {
        match std::env::var("SQUALL_CLIENT").as_deref() {
            Ok("codex") => Self::Codex,
            _ => Self::Claude,
        }
    }
}

/// Config-key of the model that represents the hosting client itself.
/// Used to warn when a default ensemble would include the host (which the recursion
/// guard always refuses).
fn host_model_for(mode: ClientMode) -> Option<&'static str> {
    match mode {
        ClientMode::Claude => Some("claude"),
        ClientMode::Codex => Some("codex"),
    }
}

fn client_mode_str(mode: ClientMode) -> &'static str {
    match mode {
        ClientMode::Claude => "claude",
        ClientMode::Codex => "codex",
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
    /// Which MCP client is hosting Squall.
    pub client_mode: ClientMode,
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
// Retired model identifiers
// ---------------------------------------------------------------------------

/// Maps retired model identifiers to the current config key that supersedes them.
///
/// Covers both old **config keys** (the public API callers pass in `models`) and old
/// provider **model_ids** (what historical memory rows were logged under). Both kinds
/// resolve to a current config key, which is exactly the normalization
/// `Registry::model_id_to_key` already performs — so one table serves three purposes:
///
/// 1. Old keys keep dispatching instead of silently landing in `not_started`.
/// 2. Memory stats survive a rename, so the hard gate isn't reset to cold-start.
/// 3. `suggest_models` can point at the successor, which plain substring matching
///    cannot do across a version bump (`kimi-k2.6` shares no substring with
///    `kimi-k2.7-code`).
///
/// Add an entry here whenever a model key or model_id changes. `retired_aliases_resolve`
/// asserts every target still exists, and `skill_files_have_no_retired_model_keys`
/// asserts no skill file still dispatches a retired name.
pub const RETIRED_MODEL_ALIASES: &[(&str, &str)] = &[
    // --- retired config keys ---
    ("kimi-k2.5", "kimi-k2.7-code"),
    ("kimi-k2.6", "kimi-k2.7-code"),
    ("glm-5.1", "glm-5.2"),
    ("z-ai/glm-5", "glm-5.2"),
    ("qwen-3.5", "qwen-3.7-max"),
    ("deepseek-v3.1", "deepseek-v4-pro"),
    // --- retired provider model_ids (memory continuity) ---
    ("moonshotai/Kimi-K2.5", "kimi-k2.7-code"),
    ("moonshotai/Kimi-K2.6", "kimi-k2.7-code"),
    ("zai-org/GLM-5.1", "glm-5.2"),
    ("deepseek-ai/DeepSeek-V3.1", "deepseek-v4-pro"),
    ("grok-4.3", "grok"),
];

/// Resolve a possibly-retired model identifier to its current config key.
/// Returns `None` when the identifier is not a known retired alias.
///
/// Case-insensitive: provider model_ids vary in casing between the config and what
/// providers echo back, and a casing mismatch here would silently skip the alias.
pub fn resolve_retired_alias(name: &str) -> Option<&'static str> {
    let needle = name.trim().to_lowercase();
    RETIRED_MODEL_ALIASES
        .iter()
        .find(|(old, _)| old.to_lowercase() == needle)
        .map(|(_, new)| *new)
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
model_id = "grok-4.5"
provider = "xai"
backend = "http"
description = "xAI Grok 4.5, fast reasoning with text+image input and 200K long-context threshold"
speed_tier = "fast"
precision_tier = "medium"
strengths = ["fast responses", "broad knowledge", "long context", "agentic tool calling"]
weaknesses = ["XML escaping false positives", "edition 2024 false positives"]

[models."glm-5.2"]
model_id = "zai-org/GLM-5.2"
provider = "together"
backend = "http"
description = "Zhipu GLM-5.2 via Together (US-hosted), 262K ctx, strong architectural and coding analysis"
speed_tier = "medium"
precision_tier = "medium"
strengths = ["clear architectural analysis", "structured output", "strong SWE-bench Pro performance"]
weaknesses = ["surface-level findings on simple bugs"]

[models.deepseek-r1]
model_id = "deepseek-ai/DeepSeek-R1-0528"
provider = "together"
backend = "http"
description = "DeepSeek R1-0528 reasoning model via Together (US-hosted), strong at logic-heavy analysis"
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

[models."kimi-k2.7-code"]
model_id = "moonshotai/Kimi-K2.7-Code"
provider = "together"
backend = "http"
description = "Moonshot's Kimi K2.7-Code via Together (US-hosted), code-specialized contrarian edge case reviewer with cached input"
speed_tier = "medium"
precision_tier = "medium"
strengths = ["contrarian perspective", "edge case detection", "cached input support"]
weaknesses = ["inconsistent quality"]

[models."deepseek-v4-pro"]
model_id = "deepseek-ai/DeepSeek-V4-Pro"
provider = "together"
backend = "http"
description = "DeepSeek V4-Pro via Together (US-hosted), frontier open-source coder with 512K ctx"
speed_tier = "medium"
precision_tier = "high"
strengths = ["strong reasoning", "finds real bugs", "512K context", "cached input support"]
weaknesses = ["verbose output"]


[models."qwen-3.7-max"]
model_id = "Qwen/Qwen3.7-Max"
provider = "together"
backend = "http"
description = "Alibaba's Qwen3.7-Max via Together, flagship agent-era model with 1M context"
speed_tier = "medium"
precision_tier = "high"
strengths = ["agentic tool calling", "1M context", "strong multilingual code understanding"]
weaknesses = ["higher cost than open Qwen variants"]

[models.qwen3-coder]
model_id = "Qwen/Qwen3-Coder-480B-A35B-Instruct-FP8"
provider = "together"
backend = "http"
description = "Qwen3 Coder 480B via Together, purpose-built for code review and generation"
speed_tier = "medium"
precision_tier = "high"
strengths = ["purpose-built for code", "strong at code review", "large context"]
weaknesses = ["new model, limited benchmarks"]

[models.llama4-maverick]
model_id = "meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8"
provider = "together"
backend = "http"
description = "Meta Llama 4 Maverick via Together, cheap with 1M context window"
speed_tier = "fast"
precision_tier = "medium"
strengths = ["very cheap", "1M context window", "fast inference"]
weaknesses = ["new model, unproven for code review"]

# --- CLI models ---

[models.gemini]
model_id = "gemini"
provider = "gemini"
backend = "cli"
executable = "gemini"
args_template = ["-m", "gemini-3.1-pro-preview", "-o", "json"]
description = "Google Gemini CLI, best at systems-level bug detection"
speed_tier = "medium"
precision_tier = "high"
strengths = ["systems-level bugs", "finds all real bugs"]
weaknesses = ["slower than HTTP models"]

[models.codex]
model_id = "gpt-5.5"
provider = "codex"
backend = "cli"
executable = "codex"
args_template = ["exec", "--json", "-m", "{model}", "-c", "model_reasoning_effort=\"{reasoning}\""]
description = "OpenAI Codex CLI (GPT-5.5), highest precision with zero false positives"
speed_tier = "slow"
precision_tier = "high"
strengths = ["highest precision", "zero false positives", "exact line references"]
weaknesses = ["variable speed (50-300s)"]

[models.claude]
model_id = "opus"
provider = "claude"
backend = "cli"
executable = "claude"
args_template = ["--print", "--output-format", "json", "--model", "{model}"]
description = "Anthropic Claude Code CLI (Opus), local investigator backend for Codex-hosted Squall"
speed_tier = "medium"
precision_tier = "high"
strengths = ["local file access", "deep reasoning", "full Claude Code toolset"]
weaknesses = ["requires claude CLI installed", "slower than HTTP"]

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
#
# `default_models` is intentionally NOT set here. The Rust-side fallback in
# resolve() picks the client-mode-appropriate ensemble (codex for Claude hosts,
# claude for Codex hosts) — embedding a hardcoded list here would shadow that
# logic and re-introduce the recursion-guard breakage.
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
        assert_eq!(config.models.len(), 15);
        assert!(config.models.contains_key("grok"));
        assert!(config.models.contains_key("gemini"));
        assert!(config.models.contains_key("codex"));
        assert!(config.models.contains_key("claude"));
        assert!(config.models.contains_key("o3-deep-research"));
        assert!(config.models.contains_key("deep-research-pro"));
    }

    /// Every retired alias must point at a model that actually exists, and no retired
    /// name may still be a live key. Guards the alias table itself from rotting.
    #[test]
    fn retired_aliases_resolve() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        for (old, new) in RETIRED_MODEL_ALIASES {
            assert!(
                config.models.contains_key(*new),
                "retired alias {old} points at {new}, which is not a defined model"
            );
            assert!(
                !config.models.contains_key(*old),
                "{old} is listed as retired but is still a live model key"
            );
        }
    }

    /// A dangling entry in `default_models` is a silent runtime failure — the model
    /// lands in `not_started` and the fallback ensemble quietly shrinks. Commit
    /// b479df9b fixed exactly this bug once already.
    #[test]
    fn default_models_resolve_to_defined_keys() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        for mode in [ClientMode::Claude, ClientMode::Codex] {
            for model in &ReviewConfig::defaults_for(mode).default_models {
                assert!(
                    config.models.contains_key(model),
                    "default_models for {mode:?} names {model}, which is not a defined model"
                );
            }
        }
        for model in &ReviewConfig::default().default_models {
            assert!(
                config.models.contains_key(model),
                "ReviewConfig::default() names {model}, which is not a defined model"
            );
        }
    }

    /// Skill files are executable contracts, not documentation: they instruct agents to
    /// pass exact model keys to `review`, so a stale key silently shrinks the ensemble
    /// rather than erroring. The Session Learnings archive is excluded — it is a
    /// historical record and legitimately names models that no longer exist.
    #[test]
    fn skill_files_have_no_retired_model_keys() {
        let mut stale: Vec<String> = Vec::new();

        for root in [".claude/skills", ".agents/skills"] {
            for path in markdown_files(std::path::Path::new(root)) {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (lineno, line) in strip_learnings_archive(&text).lines().enumerate() {
                    for (old, new) in RETIRED_MODEL_ALIASES {
                        if mentions_key(line, old) {
                            stale.push(format!(
                                "{}:{} references retired `{old}` (use `{new}`)",
                                path.display(),
                                lineno + 1
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            stale.is_empty(),
            "skill files dispatch retired model keys:\n  {}",
            stale.join("\n  ")
        );
    }

    /// Blank out the Session Learnings archive so historical mentions don't trip the scan.
    fn strip_learnings_archive(text: &str) -> String {
        const START: &str = "SENTINEL:SESSION_LEARNINGS_START";
        const END: &str = "SENTINEL:SESSION_LEARNINGS_END";
        let mut out = String::with_capacity(text.len());
        let mut skipping = false;
        for line in text.lines() {
            if line.contains(START) {
                skipping = true;
            }
            // Preserve line numbering so failure messages point at the real line.
            out.push_str(if skipping { "" } else { line });
            out.push('\n');
            if line.contains(END) {
                skipping = false;
            }
        }
        out
    }

    /// Substring match with identifier boundaries, so `glm-5.1` does not match
    /// `glm-5.12` and `kimi-k2.5` does not match `kimi-k2.5x`.
    fn mentions_key(line: &str, key: &str) -> bool {
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '/';
        let mut from = 0;
        while let Some(rel) = line[from..].find(key) {
            let start = from + rel;
            let end = start + key.len();
            let before_ok = start == 0 || !line[..start].chars().next_back().is_some_and(is_ident);
            let after_ok = end == line.len() || !line[end..].chars().next().is_some_and(is_ident);
            if before_ok && after_ok {
                return true;
            }
            from = start + key.len();
        }
        false
    }

    /// Recursively collect `*.md` files under `root`. Missing directories yield nothing.
    fn markdown_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(markdown_files(&path));
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn builtin_grok_model_id_is_correct() {
        let config: TomlConfig = toml::from_str(BUILTIN_DEFAULTS).unwrap();
        let grok = &config.models["grok"];
        assert_eq!(grok.model_id.as_deref(), Some("grok-4.5"));
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

    // Note: SQUALL_CLIENT env-var coverage lives in tests/client_mode_env.rs as an
    // integration test (separate test binary). Putting it here would race with the
    // ~20 unit tests that transitively read SQUALL_CLIENT via Config::from_toml →
    // resolve → from_env, since Cargo runs unit tests in parallel within one binary.
    // Each integration test file gets its own process, so reads can't collide.

    #[test]
    fn host_model_for_returns_self() {
        // Sanity: claude in Claude mode, codex in Codex mode.
        assert_eq!(host_model_for(ClientMode::Claude), Some("claude"));
        assert_eq!(host_model_for(ClientMode::Codex), Some("codex"));
    }

    #[test]
    fn review_defaults_for_codex_mode_excludes_codex() {
        // The recursion guard refuses clink(codex) when SQUALL_CLIENT=codex.
        // The server's own fallback ensemble must not contain "codex" in that case.
        let codex_defaults = ReviewConfig::defaults_for(ClientMode::Codex);
        assert!(
            !codex_defaults.default_models.iter().any(|m| m == "codex"),
            "Codex-mode default ensemble must not contain 'codex' (recursion guard refuses it). \
             Got: {:?}",
            codex_defaults.default_models
        );
        assert!(
            codex_defaults.default_models.iter().any(|m| m == "claude"),
            "Codex-mode default ensemble should substitute 'claude' for 'codex'. Got: {:?}",
            codex_defaults.default_models
        );
    }

    #[test]
    fn review_defaults_for_claude_mode_excludes_claude() {
        // Mirror invariant: Claude-mode default ensemble must not contain "claude".
        let claude_defaults = ReviewConfig::defaults_for(ClientMode::Claude);
        assert!(
            !claude_defaults.default_models.iter().any(|m| m == "claude"),
            "Claude-mode default ensemble must not contain 'claude' (recursion guard refuses it). \
             Got: {:?}",
            claude_defaults.default_models
        );
        assert!(
            claude_defaults.default_models.iter().any(|m| m == "codex"),
            "Claude-mode default ensemble should contain 'codex'. Got: {:?}",
            claude_defaults.default_models
        );
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
