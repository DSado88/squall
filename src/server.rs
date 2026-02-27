use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};

use crate::config::Config;
use crate::context::{self, GitContextCache};
use crate::dispatch::ProviderRequest;
use crate::dispatch::registry::Registry;
use crate::memory::MemoryStore;
use crate::response::{PalMetadata, PalToolResponse};
use crate::review::ReviewExecutor;
use crate::tools::chat::ChatRequest;
use crate::tools::clink::ClinkRequest;
use crate::tools::enums::{ReasoningEffort, ResponseFormat};
use crate::tools::listmodels::{ListModelsResponse, ModelInfo};
use crate::tools::memory::{FlushRequest, MemorizeRequest, MemoryRequest};
use crate::tools::review::ReviewRequest;

#[derive(Clone)]
pub struct SquallServer {
    registry: Arc<Registry>,
    memory: Arc<MemoryStore>,
    git_cache: Arc<GitContextCache>,
    review_config: crate::config::ReviewConfig,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SquallServer {
    pub fn new(config: Config) -> Self {
        let review_config = config.review.clone(); // Clone BEFORE from_config() move

        // Build global writer before config is moved into Registry.
        #[cfg(feature = "global-memory")]
        let global_memory_config = config.global_memory.clone();

        let registry = Arc::new(Registry::from_config(config));

        #[cfg_attr(not(feature = "global-memory"), allow(unused_mut))]
        let mut store = MemoryStore::new().with_id_to_key(registry.model_id_to_key());

        #[cfg(feature = "global-memory")]
        if global_memory_config.enabled {
            match crate::memory::global::GlobalWriter::new(global_memory_config.db_path.into()) {
                Some(writer) => {
                    tracing::info!("global memory: enabled");
                    store = store.with_global(writer);
                }
                None => {
                    tracing::warn!(
                        "global memory: failed to initialize, continuing with local only"
                    );
                }
            }
        }

        let memory = Arc::new(store);
        let git_cache = Arc::new(GitContextCache::new());
        Self {
            registry,
            memory,
            git_cache,
            review_config,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "chat",
        description = "Ask one AI model a targeted question. Use for focused second opinions to complement your own analysis. Use `listmodels` for model names.",
        annotations(read_only_hint = true)
    )]
    async fn chat(
        &self,
        Parameters(req): Parameters<ChatRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt).map_err(|msg| McpError::invalid_params(msg, None))?;
        context::validate_temperature(req.temperature)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let model = req.model_or_default().to_string();
        let start = Instant::now();

        // Resolve file context if file_paths provided
        let mut prompt = req.prompt;
        if let Some(ref file_paths) = req.file_paths {
            let wd = req.working_directory.as_deref().ok_or_else(|| {
                McpError::invalid_params(
                    "working_directory is required when file_paths is set",
                    None,
                )
            })?;
            let base_dir = context::validate_working_directory(wd)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            let fmt = req.context_format.unwrap_or_default();
            let file_result = context::resolve_file_context(
                file_paths,
                &base_dir,
                context::MAX_FILE_CONTEXT_BYTES,
                fmt,
            )
            .await
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            if let Some(ctx) = file_result.context {
                prompt = format!("{ctx}\n{prompt}");
            }
        }

        let deadline_secs = if self.registry.get(&model).is_some_and(|e| e.is_async_poll())
            || reasoning_needs_extended_deadline(req.reasoning_effort.as_ref())
        {
            600 // async-poll or reasoning models get 10 min (MCP ceiling)
        } else {
            300 // HTTP gets 5 min
        };
        let provider_req = ProviderRequest {
            prompt: prompt.into(),
            model: model.clone(),
            deadline: Instant::now() + Duration::from_secs(deadline_secs),
            working_directory: req.working_directory,
            system_prompt: req.system_prompt,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            reasoning_effort: req.reasoning_effort.map(|e| e.as_str().to_string()),
            cancellation_token: None,
            stall_timeout: None,
        };

        let response = match self.registry.query(&provider_req).await {
            Ok(result) => PalToolResponse::success(
                result.text,
                PalMetadata {
                    tool_name: "chat".to_string(),
                    model_used: result.model,
                    provider_used: result.provider,
                    duration_seconds: start.elapsed().as_secs_f64(),
                },
            ),
            Err(e) => {
                tracing::warn!("chat query failed: {e}");
                let provider = e.provider().unwrap_or("unknown").to_string();
                PalToolResponse::error(
                    e.user_message(),
                    PalMetadata {
                        tool_name: "chat".to_string(),
                        model_used: model,
                        provider_used: provider,
                        duration_seconds: start.elapsed().as_secs_f64(),
                    },
                )
            }
        };

        Ok(response.into_call_tool_result())
    }

    #[tool(
        name = "listmodels",
        description = "List available AI models with provider, backend, and capability info.",
        annotations(read_only_hint = true)
    )]
    async fn listmodels(&self) -> Result<CallToolResult, McpError> {
        let mut models: Vec<ModelInfo> = self
            .registry
            .list_models()
            .into_iter()
            .map(ModelInfo::from)
            .collect();
        models.sort_by(|a, b| a.name.cmp(&b.name));

        let list = ListModelsResponse { models };
        let content = list.to_markdown();

        let response = PalToolResponse::success(
            content,
            PalMetadata {
                tool_name: "listmodels".to_string(),
                model_used: "none".to_string(),
                provider_used: "none".to_string(),
                duration_seconds: 0.0,
            },
        );

        Ok(response.into_call_tool_result())
    }

    #[tool(
        name = "clink",
        description = "Query a CLI-based AI model (codex, gemini) as a subprocess. Use for web search or deep repo analysis. Use `listmodels` for model names.",
        annotations(read_only_hint = true)
    )]
    async fn clink(
        &self,
        Parameters(req): Parameters<ClinkRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt).map_err(|msg| McpError::invalid_params(msg, None))?;
        context::validate_temperature(req.temperature)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let model = req.model.clone();
        let start = Instant::now();

        // Resolve file manifest and working directory for CLI.
        // Use canonical path from validate_working_directory() — not the original
        // string — to prevent TOCTOU (symlink retargeted between validation and use).
        let mut prompt = req.prompt;
        let working_directory = if let Some(ref file_paths) = req.file_paths {
            let wd = req.working_directory.as_deref().ok_or_else(|| {
                McpError::invalid_params(
                    "working_directory is required when file_paths is set",
                    None,
                )
            })?;
            let base_dir = context::validate_working_directory(wd)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            if let Some(manifest) = context::resolve_file_manifest(file_paths, &base_dir)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?
            {
                prompt = format!("{manifest}\n\n{prompt}");
            }
            Some(base_dir.to_string_lossy().to_string())
        } else if let Some(ref wd) = req.working_directory {
            let base_dir = context::validate_working_directory(wd)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            Some(base_dir.to_string_lossy().to_string())
        } else {
            None
        };

        let provider_req = ProviderRequest {
            prompt: prompt.into(),
            model: model.clone(),
            deadline: Instant::now() + Duration::from_secs(600), // CLIs get 10 min
            working_directory,
            system_prompt: req.system_prompt,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            reasoning_effort: req.reasoning_effort.map(|e| e.as_str().to_string()),
            cancellation_token: None,
            stall_timeout: None,
        };

        let response = match self.registry.query(&provider_req).await {
            Ok(result) => PalToolResponse::success(
                result.text,
                PalMetadata {
                    tool_name: "clink".to_string(),
                    model_used: result.model,
                    provider_used: result.provider,
                    duration_seconds: start.elapsed().as_secs_f64(),
                },
            ),
            Err(e) => {
                tracing::warn!("clink query failed: {e}");
                let provider = e.provider().unwrap_or(&model).to_string();
                PalToolResponse::error(
                    e.user_message(),
                    PalMetadata {
                        tool_name: "clink".to_string(),
                        model_used: model,
                        provider_used: provider,
                        duration_seconds: start.elapsed().as_secs_f64(),
                    },
                )
            }
        };

        Ok(response.into_call_tool_result())
    }

    #[tool(
        name = "review",
        description = "Consult multiple models in parallel with straggler cutoff. Assign expertise lenses via per_model_system_prompts — falsification framing ('attempt to PROVE X') produces the best results. Use `listmodels` for model names.",
        annotations(read_only_hint = true)
    )]
    async fn review(
        &self,
        Parameters(req): Parameters<ReviewRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt).map_err(|msg| McpError::invalid_params(msg, None))?;
        context::validate_temperature(req.temperature)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let start = std::time::Instant::now();

        // Resolve file context and working directory (same pattern as clink handler).
        // Use canonical path from validate_working_directory() to prevent TOCTOU.
        let mut prompt = req.prompt.clone();
        let mut files_skipped = None;
        let mut files_errors = None;
        // When both file_paths and diff are provided, reserve MIN_DIFF_BUDGET
        // for the diff so it's never starved by large file context.
        let file_budget = if req.diff.is_some() {
            context::MAX_FILE_CONTEXT_BYTES.saturating_sub(context::MIN_DIFF_BUDGET)
        } else {
            context::MAX_FILE_CONTEXT_BYTES
        };
        let working_directory = if let Some(ref file_paths) = req.file_paths {
            let wd = req.working_directory.as_deref().ok_or_else(|| {
                McpError::invalid_params(
                    "working_directory is required when file_paths is set",
                    None,
                )
            })?;
            let base_dir = context::validate_working_directory(wd)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            let fmt = req.context_format.unwrap_or_default();
            let file_result =
                context::resolve_file_context(file_paths, &base_dir, file_budget, fmt)
                    .await
                    .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            if !file_result.skipped.is_empty() {
                files_skipped = Some(
                    file_result
                        .skipped
                        .iter()
                        .map(|(name, sz)| format!("{name} ({sz}B)"))
                        .collect(),
                );
            }
            if !file_result.errors.is_empty() {
                files_errors = Some(file_result.errors);
            }
            if let Some(ctx) = file_result.context {
                prompt = format!("{ctx}\n{prompt}");
            }
            Some(base_dir.to_string_lossy().to_string())
        } else if let Some(ref wd) = req.working_directory {
            let base_dir = context::validate_working_directory(wd)
                .await
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            Some(base_dir.to_string_lossy().to_string())
        } else {
            None
        };

        // Inject diff context (shared budget with file context)
        if let Some(ref diff_text) = req.diff {
            let file_context_used = prompt.len() - req.prompt.len();
            let diff_budget = context::MAX_FILE_CONTEXT_BYTES.saturating_sub(file_context_used);
            if let Some(wrapped) = context::wrap_diff_context(diff_text, diff_budget) {
                prompt = format!("{wrapped}\n{prompt}");
            }
        }

        let executor = ReviewExecutor::new(self.registry.clone());
        let prompt_len = prompt.len();
        let wd_for_memory = working_directory.clone();
        let review_response = executor
            .execute(
                &req,
                prompt,
                &self.memory,
                working_directory,
                files_skipped,
                files_errors,
                Some(&self.review_config),
            )
            .await;

        // Log model metrics to memory (non-blocking, fire-and-forget)
        let memory = self.memory.clone();
        let results_for_memory = review_response.results.clone();
        let id_to_key = self.registry.model_id_to_key();
        tokio::spawn(async move {
            memory
                .log_model_metrics(
                    &results_for_memory,
                    prompt_len,
                    Some(&id_to_key),
                    wd_for_memory.as_deref(),
                )
                .await;
        });

        // Render the review response as markdown for MCP (disk file stays JSON)
        let concise = matches!(req.response_format, Some(ResponseFormat::Concise));
        let content = review_response.to_markdown(concise);

        let response = PalToolResponse::success(
            content,
            PalMetadata {
                tool_name: "review".to_string(),
                model_used: "multi".to_string(),
                provider_used: "multi".to_string(),
                duration_seconds: start.elapsed().as_secs_f64(),
            },
        );

        Ok(response.into_call_tool_result())
    }

    #[tool(
        name = "memorize",
        description = "Save your synthesized findings after a review: recurring patterns, effective tactics, and model recommendations."
    )]
    async fn memorize(
        &self,
        Parameters(req): Parameters<MemorizeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();

        // Resolve scope: explicit > auto-detected from git > default "codebase"
        let auto_scope;
        let scope = if req.scope.is_some() {
            req.scope.as_deref()
        } else if let Some(ref wd) = req.working_directory {
            // Validate working directory before using it for git detection.
            let validated = context::validate_working_directory(wd).await.map_err(|e| {
                McpError::invalid_params(format!("invalid working_directory: {e}"), None)
            })?;
            let git_ctx = self.git_cache.get_or_detect(&validated).await;
            auto_scope = context::default_scope_from_git(git_ctx.as_ref());
            Some(auto_scope.as_str())
        } else {
            None
        };

        match self
            .memory
            .memorize(
                req.category.as_str(),
                &req.content,
                req.model.as_deref(),
                req.tags.as_deref(),
                scope,
                req.metadata.as_ref(),
            )
            .await
        {
            Ok(path) => {
                let response = PalToolResponse::success(
                    format!("Saved to {path}"),
                    PalMetadata {
                        tool_name: "memorize".to_string(),
                        model_used: "none".to_string(),
                        provider_used: "none".to_string(),
                        duration_seconds: start.elapsed().as_secs_f64(),
                    },
                );
                Ok(response.into_call_tool_result())
            }
            Err(msg) => Err(McpError::invalid_params(msg, None)),
        }
    }

    #[tool(
        name = "memory",
        description = "Read prior patterns, tactics, and model recommendations to inform model selection and review lenses.",
        annotations(read_only_hint = true)
    )]
    async fn memory(
        &self,
        Parameters(req): Parameters<MemoryRequest>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();

        match self
            .memory
            .read_memory(
                req.category.as_ref().map(|c| c.as_str()),
                req.model.as_deref(),
                req.max_chars(),
                req.scope.as_deref(),
            )
            .await
        {
            Ok(content) => {
                let response = PalToolResponse::success(
                    content,
                    PalMetadata {
                        tool_name: "memory".to_string(),
                        model_used: "none".to_string(),
                        provider_used: "none".to_string(),
                        duration_seconds: start.elapsed().as_secs_f64(),
                    },
                );
                Ok(response.into_call_tool_result())
            }
            Err(msg) => Err(McpError::internal_error(msg, None)),
        }
    }

    #[tool(
        name = "flush",
        description = "Flush working memory after PR merge. Graduates high-evidence patterns (>=3 occurrences) from branch scope to codebase scope. Archives low-evidence branch patterns. Prunes model events older than 30 days."
    )]
    async fn flush(
        &self,
        Parameters(req): Parameters<FlushRequest>,
    ) -> Result<CallToolResult, McpError> {
        let start = Instant::now();

        if req.branch.trim().is_empty() {
            return Err(McpError::invalid_params("branch must not be empty", None));
        }

        match self.memory.flush_branch(&req.branch).await {
            Ok(report) => {
                let response = PalToolResponse::success(
                    report,
                    PalMetadata {
                        tool_name: "flush".to_string(),
                        model_used: "none".to_string(),
                        provider_used: "none".to_string(),
                        duration_seconds: start.elapsed().as_secs_f64(),
                    },
                );
                Ok(response.into_call_tool_result())
            }
            Err(msg) => Err(McpError::internal_error(msg, None)),
        }
    }
}

/// Returns true if reasoning_effort warrants an extended deadline.
fn reasoning_needs_extended_deadline(effort: Option<&ReasoningEffort>) -> bool {
    matches!(
        effort,
        Some(ReasoningEffort::Medium | ReasoningEffort::High | ReasoningEffort::Xhigh)
    )
}

#[tool_handler]
impl ServerHandler for SquallServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "squall".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Squall: parallel AI model dispatch. Each model is an independent consultant.\n\n\
                 Workflow:\n\
                 1. Call `memory` (recommend/patterns/tactics) to check past learnings.\n\
                 2. Call `listmodels` for exact model names and speed tiers.\n\
                 3. Call `review` with expertise `per_model_system_prompts` (security, correctness, etc.).\n\
                    - Use falsification framing: 'Attempt to PROVE [issue] exists. Report confidence.'\n\
                    - Set `deep: true` for security/architecture/high-stakes (600s, high reasoning).\n\
                    - `results_file` persists on disk — read it if context compaction loses the response.\n\
                 4. Triangulate model findings with your own investigation.\n\
                 5. Call `memorize` to capture patterns, tactics, and model recommendations.\n\
                 6. After PR merge: `flush` to graduate branch patterns to codebase scope.\n\n\
                 File context: pass `file_paths` + `working_directory` to include source files.\n\
                 For review, also pass `diff` with unified diff text.\n\
                 Research: `clink` with model \"codex\" for web search, or `review` with models as advisors."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
