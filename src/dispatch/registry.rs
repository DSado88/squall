use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::Semaphore;

use crate::config::{
    ClientMode, Config, PersistRawOutput, RETIRED_MODEL_ALIASES, resolve_retired_alias,
};
use crate::dispatch::async_poll::AsyncPollDispatch;
use crate::dispatch::cli::CliDispatch;
use crate::dispatch::http::HttpDispatch;
use crate::dispatch::{ProviderRequest, ProviderResult};
use crate::error::SquallError;
use crate::parsers::OutputParser;
use crate::parsers::antigravity::AntigravityParser;
use crate::parsers::claude::ClaudeParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;

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
    client_mode: ClientMode,
    hard_gate: bool,
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
            client_mode: config.client_mode,
            hard_gate: config.hard_gate,
        }
    }

    /// Whether the success-rate hard gate may exclude models. See `Config::hard_gate`.
    pub fn hard_gate_enabled(&self) -> bool {
        self.hard_gate
    }

    /// Returns the number of CLI semaphore permits (for testing).
    pub fn cli_semaphore_permits(&self) -> usize {
        self.cli_semaphore.available_permits()
    }

    /// Returns the number of HTTP semaphore permits (for testing).
    pub fn http_semaphore_permits(&self) -> usize {
        self.http_semaphore.available_permits()
    }

    /// Look up a model by config key, falling back through retired aliases.
    ///
    /// Without the fallback, a caller pinned to a pre-rename key (a script, a skill
    /// ensemble table, a saved prompt) lands in `not_started` — a silent partial
    /// failure that shrinks the ensemble rather than erroring.
    pub fn get(&self, model: &str) -> Option<&ModelEntry> {
        if let Some(entry) = self.models.get(model) {
            return Some(entry);
        }
        resolve_retired_alias(model).and_then(|current| self.models.get(current))
    }

    /// Canonical config key for a possibly-retired identifier.
    ///
    /// Returns the successor key when `model` is a retired alias that resolves, and the
    /// input unchanged otherwise (so genuinely unknown names keep their original spelling
    /// when they surface in `not_started`).
    ///
    /// Callers must canonicalize BEFORE deduplicating or looking up per-model statistics:
    /// `["kimi-k2.6", "kimi-k2.7-code"]` are two spellings of one model, and stats are
    /// stored under the canonical key only.
    pub fn canonical_key(&self, model: &str) -> String {
        if self.models.contains_key(model) {
            return model.to_string();
        }
        match resolve_retired_alias(model) {
            Some(current) if self.models.contains_key(current) => current.to_string(),
            _ => model.to_string(),
        }
    }

    pub fn list_models(&self) -> Vec<(&String, &ModelEntry)> {
        self.models.iter().collect()
    }

    /// Returns a map of model_id → config_key for model identity normalization.
    /// Used by memory subsystem to normalize event log entries that may use
    /// provider model_ids instead of config keys.
    pub fn model_id_to_key(&self) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = self
            .models
            .iter()
            .map(|(key, entry)| (entry.model_id.clone(), key.clone()))
            .collect();

        // Carry retired identifiers forward so a rename doesn't orphan history. Without
        // this, renaming a key and its model_id together resets the model to zero samples
        // and the hard gate silently degrades to a cold-start no-op.
        //
        // `or_insert` so a live model_id always wins; aliases whose target isn't in this
        // registry are skipped, since normalizing onto a nonexistent key is worse than
        // leaving the row under its original name.
        for (old, new) in RETIRED_MODEL_ALIASES {
            if self.models.contains_key(*new) {
                map.entry((*old).to_string())
                    .or_insert_with(|| (*new).to_string());
            }
        }
        map
    }

    /// Suggest similar model names for a failed lookup (substring match).
    /// Sorted alphabetically, capped at 5 to keep error messages readable.
    pub fn suggest_models(&self, query: &str) -> Vec<String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }

        // Substring matching cannot bridge a version bump — `kimi-k2.6` shares no
        // substring with `kimi-k2.7-code` — so version bumps are invisible to the
        // fallback below. The alias table is the only thing that can suggest them.
        if let Some(current) = resolve_retired_alias(query)
            && self.models.contains_key(current)
        {
            return vec![current.to_string()];
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
            "antigravity" => Ok(Box::new(AntigravityParser)),
            "codex" => Ok(Box::new(CodexParser)),
            "claude" => Ok(Box::new(ClaudeParser)),
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
        // `self.get` (not `self.models.get`) so retired aliases resolve on the dispatch
        // path too — otherwise an old key works for `review` but not `chat`/`clink`.
        let entry = self.get(&req.model).ok_or_else(|| {
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
                let _permit =
                    Self::acquire_with_deadline(&self.http_semaphore, req.deadline).await?;
                self.http
                    .query_model(req, &entry.provider, base_url, api_key, api_format)
                    .await
            }
            BackendConfig::Cli {
                executable,
                args_template,
            } => {
                // Recursion guard: refuse to clink to the same CLI that's hosting Squall.
                // Inspects executable AND args so wrapper-script bypasses
                // (sh -c "codex ...", npx codex, env codex) are also blocked.
                if let Some(reason) = recursion_guard(self.client_mode, executable, args_template) {
                    return Err(SquallError::Other(reason));
                }
                let parser = Self::parser_for(&entry.provider)?;
                let _permit =
                    Self::acquire_with_deadline(&self.cli_semaphore, req.deadline).await?;
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

/// Block clink calls that would recurse back into the hosting client.
/// Returns `Some(reason)` to refuse, `None` to permit.
///
/// Inspects both the executable basename and every token in `args` so wrapper-script
/// configurations (`executable="sh"`, `args=["-c", "codex ..."]`) can't bypass the guard
/// by hiding the real client behind a shell, npx, or env runner. Case-insensitive on
/// basenames to handle Windows `.exe` quirks if Squall ever runs there.
pub fn recursion_guard(mode: ClientMode, executable: &str, args: &[String]) -> Option<String> {
    let blocked = match mode {
        ClientMode::Codex => "codex",
        ClientMode::Claude => "claude",
    };

    if token_matches(executable, blocked) {
        return Some(refusal_message(mode, executable));
    }

    for arg in args {
        // Each whitespace-split fragment of an arg is treated as a candidate command.
        // This catches `sh -c "codex ..."`, `npx codex`, `env codex`, and similar.
        for token in arg.split_whitespace() {
            if token_matches(token, blocked) {
                return Some(refusal_message(mode, &format!("{executable} {arg}")));
            }
        }
    }
    None
}

fn token_matches(token: &str, blocked: &str) -> bool {
    let basename = std::path::Path::new(token)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(token)
        .to_ascii_lowercase();
    basename == blocked
}

fn refusal_message(mode: ClientMode, surfaced_command: &str) -> String {
    match mode {
        ClientMode::Codex => format!(
            "clink to 'codex' refused: Codex is hosting Squall (SQUALL_CLIENT=codex). \
             Calling codex via clink (directly or via wrapper such as `{surfaced_command}`) \
             would recurse. Use a different model, or unset SQUALL_CLIENT if Codex is not \
             actually the host."
        ),
        ClientMode::Claude => format!(
            "clink to 'claude' refused: Claude Code is hosting Squall (SQUALL_CLIENT unset/claude). \
             Calling claude via clink (directly or via wrapper such as `{surfaced_command}`) \
             would recurse. Use a different model, or set SQUALL_CLIENT=codex if Codex is the host."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReviewConfig;

    /// Registry holding two current models whose keys and model_ids both changed in a
    /// past rename, so retired-alias behaviour can be exercised without env or API keys.
    fn test_registry() -> Registry {
        let mut models = HashMap::new();
        for (key, model_id) in [
            ("kimi-k2.7-code", "moonshotai/Kimi-K2.7-Code"),
            ("glm-5.2", "zai-org/GLM-5.2"),
            ("grok", "grok-4.5"),
        ] {
            models.insert(
                key.to_string(),
                ModelEntry {
                    model_id: model_id.to_string(),
                    provider: "together".to_string(),
                    backend: BackendConfig::Http {
                        base_url: "https://example.invalid/v1/chat/completions".to_string(),
                        api_key: "test".to_string(),
                        api_format: ApiFormat::OpenAi,
                    },
                    description: String::new(),
                    strengths: vec![],
                    weaknesses: vec![],
                    speed_tier: "medium".to_string(),
                    precision_tier: "medium".to_string(),
                },
            );
        }
        Registry::from_config(Config {
            models,
            skipped: vec![],
            persist_raw_output: PersistRawOutput::Never,
            hard_gate: false,
            review: ReviewConfig::default(),
            #[cfg(feature = "global-memory")]
            global_memory: crate::config::GlobalMemoryConfig {
                enabled: false,
                db_path: String::new(),
            },
            client_mode: ClientMode::Claude,
        })
    }

    /// A caller pinned to a pre-rename key must still dispatch instead of silently
    /// landing in `not_started`.
    #[test]
    fn get_resolves_retired_config_key() {
        let registry = test_registry();
        let entry = registry
            .get("kimi-k2.6")
            .expect("retired key kimi-k2.6 should resolve to its successor");
        assert_eq!(entry.model_id, "moonshotai/Kimi-K2.7-Code");

        let entry = registry
            .get("glm-5.1")
            .expect("retired key glm-5.1 should resolve to its successor");
        assert_eq!(entry.model_id, "zai-org/GLM-5.2");
    }

    /// Current keys must not be shadowed by the alias lookup.
    #[test]
    fn get_prefers_live_key_over_alias() {
        let registry = test_registry();
        assert_eq!(
            registry.get("kimi-k2.7-code").unwrap().model_id,
            "moonshotai/Kimi-K2.7-Code"
        );
        assert!(registry.get("no-such-model").is_none());
    }

    /// Historical memory rows were logged under old keys and old provider model_ids.
    /// Both must normalize forward, or the hard gate resets to cold-start after a rename.
    #[test]
    fn model_id_to_key_maps_retired_identifiers_forward() {
        let registry = test_registry();
        let map = registry.model_id_to_key();

        // Current model_ids still map to their key.
        assert_eq!(
            map.get("moonshotai/Kimi-K2.7-Code").map(String::as_str),
            Some("kimi-k2.7-code")
        );
        // Retired provider model_ids map forward.
        assert_eq!(
            map.get("moonshotai/Kimi-K2.6").map(String::as_str),
            Some("kimi-k2.7-code")
        );
        assert_eq!(
            map.get("zai-org/GLM-5.1").map(String::as_str),
            Some("glm-5.2")
        );
        assert_eq!(map.get("grok-4.3").map(String::as_str), Some("grok"));
        // Retired config keys map forward too.
        assert_eq!(
            map.get("kimi-k2.6").map(String::as_str),
            Some("kimi-k2.7-code")
        );
        assert_eq!(map.get("glm-5.1").map(String::as_str), Some("glm-5.2"));
    }

    /// Aliases whose target is absent from this registry must not appear in the map —
    /// normalizing onto a nonexistent key would be worse than leaving the row alone.
    #[test]
    fn model_id_to_key_omits_aliases_with_missing_targets() {
        let registry = test_registry();
        let map = registry.model_id_to_key();
        // qwen-3.7-max is not in this test registry, so qwen-3.5 must not map forward.
        assert!(!map.contains_key("qwen-3.5"));
    }

    /// Substring matching cannot bridge a version bump: `kimi-k2.6` shares no substring
    /// with `kimi-k2.7-code`. The alias table has to supply the suggestion.
    #[test]
    fn suggest_models_suggests_retired_key_successor() {
        let registry = test_registry();
        assert_eq!(
            registry.suggest_models("kimi-k2.6"),
            vec!["kimi-k2.7-code".to_string()]
        );
        assert_eq!(
            registry.suggest_models("glm-5.1"),
            vec!["glm-5.2".to_string()]
        );
    }

    /// Two spellings of one model must collapse to a single canonical key, or the
    /// dispatcher runs the same model twice and reports it as agreement between two.
    #[test]
    fn canonical_key_collapses_retired_aliases() {
        let registry = test_registry();
        assert_eq!(registry.canonical_key("kimi-k2.6"), "kimi-k2.7-code");
        assert_eq!(registry.canonical_key("kimi-k2.7-code"), "kimi-k2.7-code");
        assert_eq!(registry.canonical_key("glm-5.1"), "glm-5.2");
    }

    /// Unknown names keep their original spelling so `not_started` echoes what was asked.
    #[test]
    fn canonical_key_passes_through_unknown_names() {
        let registry = test_registry();
        assert_eq!(registry.canonical_key("no-such-model"), "no-such-model");
        // Alias whose target is absent from this registry must not be rewritten.
        assert_eq!(registry.canonical_key("qwen-3.5"), "qwen-3.5");
    }

    /// Substring matching must keep working for non-retired typos.
    #[test]
    fn suggest_models_still_does_substring_matching() {
        let registry = test_registry();
        assert_eq!(registry.suggest_models("glm"), vec!["glm-5.2".to_string()]);
        assert!(registry.suggest_models("totally-unrelated").is_empty());
    }

    fn test_request(model: &str) -> ProviderRequest {
        ProviderRequest {
            prompt: "hi".into(),
            model: model.to_string(),
            deadline: Instant::now() + std::time::Duration::from_secs(5),
            working_directory: None,
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            cancellation_token: None,
            stall_timeout: None,
        }
    }

    /// `query` is the actual dispatch path for `chat`/`clink`. It must honour retired
    /// aliases too — resolving them only in `get()` leaves the alias working for
    /// `review` while `chat` still reports "model not found".
    ///
    /// The backend URL is unroutable, so a resolved model fails with a transport error.
    /// Anything other than `ModelNotFound` proves the name resolved.
    #[tokio::test]
    async fn query_resolves_retired_config_key() {
        let registry = test_registry();
        let err = registry
            .query(&test_request("kimi-k2.6"))
            .await
            .expect_err("unroutable backend should fail");
        assert!(
            !matches!(err, SquallError::ModelNotFound { .. }),
            "retired key kimi-k2.6 should resolve before dispatch, got: {err:?}"
        );
    }

    /// A genuinely unknown model must still report `ModelNotFound` with suggestions.
    #[tokio::test]
    async fn query_still_rejects_unknown_model() {
        let registry = test_registry();
        let err = registry
            .query(&test_request("no-such-model"))
            .await
            .expect_err("unknown model should fail");
        assert!(matches!(err, SquallError::ModelNotFound { .. }));
    }

    /// The Antigravity CLI needs its own parser: agy's JSON envelope is
    /// `{status, response, usage}`, and GeminiParser reads only `response` with no
    /// status check — so it would accept a failed run as a valid answer.
    #[test]
    fn parser_for_resolves_antigravity() {
        let parser = Registry::parser_for("antigravity").expect("antigravity parser must exist");
        let out = parser
            .parse(br#"{"status":"SUCCESS","response":"hello"}"#)
            .unwrap();
        assert_eq!(out, "hello");
        // agy signals failure in-band with exit code 0; the parser must catch it.
        assert!(
            parser
                .parse(br#"{"status":"ERROR","response":"x"}"#)
                .is_err()
        );
    }

    #[test]
    fn parser_for_still_rejects_unknown_provider() {
        assert!(Registry::parser_for("no-such-cli").is_err());
    }

    /// The provider must receive the *successor's* model_id, not the retired name —
    /// otherwise the upstream API gets a model string it retired.
    #[test]
    fn retired_key_dispatches_successor_model_id() {
        let registry = test_registry();
        assert_eq!(
            registry.get("kimi-k2.6").unwrap().model_id,
            "moonshotai/Kimi-K2.7-Code"
        );
    }
}
