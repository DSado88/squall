use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

use crate::config::Config;
use crate::context::{self, GitContextCache};
use crate::dispatch::registry::Registry;
use crate::dispatch::ProviderRequest;
use crate::memory::MemoryStore;
use crate::response::{PalMetadata, PalToolResponse};
use crate::review::ReviewExecutor;
use crate::tools::chat::ChatRequest;
use crate::tools::clink::ClinkRequest;
use crate::tools::listmodels::{ListModelsResponse, ModelInfo};
use crate::tools::memory::{FlushRequest, MemorizeRequest, MemoryRequest};
use crate::tools::review::ReviewRequest;


#[derive(Clone)]
pub struct SquallServer {
    registry: Arc<Registry>,
    memory: Arc<MemoryStore>,
    git_cache: Arc<GitContextCache>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SquallServer {
    pub fn new(config: Config) -> Self {
        let registry = Arc::new(Registry::from_config(config));
        let memory = Arc::new(MemoryStore::new());
        let git_cache = Arc::new(GitContextCache::new());
        Self {
            registry,
            memory,
            git_cache,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "chat",
        description = "Query a single AI model via HTTP (OpenAI-compatible). Use for one-off questions to a specific model. Use `listmodels` first to see available model names.",
        annotations(read_only_hint = true)
    )]
    async fn chat(
        &self,
        Parameters(req): Parameters<ChatRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt)
            .map_err(|msg| McpError::invalid_params(msg, None))?;
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
            || reasoning_needs_extended_deadline(req.reasoning_effort.as_deref())
        {
            600 // async-poll or reasoning models get 10 min (MCP ceiling)
        } else {
            300 // HTTP gets 5 min
        };
        let provider_req = ProviderRequest {
            prompt,
            model: model.clone(),
            deadline: Instant::now() + Duration::from_secs(deadline_secs),
            working_directory: None,
            system_prompt: req.system_prompt,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            reasoning_effort: req.reasoning_effort,
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
        description = "List all available AI models with provider and backend info.",
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
        let content = serde_json::to_string(&list)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

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
        description = "Query a CLI-based AI model (e.g. gemini, codex). Use for tasks that benefit from the model's native CLI capabilities. Use `listmodels` first to see available model names.",
        annotations(read_only_hint = true)
    )]
    async fn clink(
        &self,
        Parameters(req): Parameters<ClinkRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt)
            .map_err(|msg| McpError::invalid_params(msg, None))?;
        context::validate_temperature(req.temperature)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let cli_name = req.cli_name.clone();
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
            if let Some(manifest) =
                context::resolve_file_manifest(file_paths, &base_dir)
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
            prompt,
            model: cli_name.clone(),
            deadline: Instant::now() + Duration::from_secs(600), // CLIs get 10 min
            working_directory,
            system_prompt: req.system_prompt,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            reasoning_effort: req.reasoning_effort,
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
                let provider = e.provider().unwrap_or(&cli_name).to_string();
                PalToolResponse::error(
                    e.user_message(),
                    PalMetadata {
                        tool_name: "clink".to_string(),
                        model_used: cli_name,
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
        description = "Fan out a prompt to multiple models in parallel with straggler cutoff. Use this instead of multiple chat/clink calls when you want responses from several models. Supports per-model system prompts via `per_model_system_prompts` for different review angles (security, architecture, etc). Use `listmodels` first to get exact model names.",
        annotations(read_only_hint = true)
    )]
    async fn review(
        &self,
        Parameters(req): Parameters<ReviewRequest>,
    ) -> Result<CallToolResult, McpError> {
        context::validate_prompt(&req.prompt)
            .map_err(|msg| McpError::invalid_params(msg, None))?;
        context::validate_temperature(req.temperature)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let start = std::time::Instant::now();

        // Resolve file context and working directory (same pattern as clink handler).
        // Use canonical path from validate_working_directory() to prevent TOCTOU.
        let mut prompt = req.prompt.clone();
        let mut files_skipped = None;
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
            let file_result = context::resolve_file_context(
                file_paths,
                &base_dir,
                file_budget,
                fmt,
            )
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
            let diff_budget =
                context::MAX_FILE_CONTEXT_BYTES.saturating_sub(file_context_used);
            if let Some(wrapped) = context::wrap_diff_context(diff_text, diff_budget) {
                prompt = format!("{wrapped}\n{prompt}");
            }
        }

        let executor = ReviewExecutor::new(self.registry.clone());
        let prompt_len = prompt.len();
        let mut review_response = executor.execute(&req, prompt, working_directory).await;
        review_response.files_skipped = files_skipped;

        // Log model metrics to memory (non-blocking, fire-and-forget)
        let memory = self.memory.clone();
        let results_for_memory = review_response.results.clone();
        tokio::spawn(async move {
            memory.log_model_metrics(&results_for_memory, prompt_len).await;
        });

        // Serialize the full review response as the MCP content
        let json = serde_json::to_string(&review_response)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let response = PalToolResponse::success(
            json,
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
        description = "Save a learning to Squall's memory. Use after reviewing results to record patterns, model blind spots, or effective prompt tactics. Categories: 'pattern' (recurring findings) or 'tactic' (prompt effectiveness)."
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
                &req.category,
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
        description = "Read Squall's memory: model performance stats, recurring patterns, and prompt tactics. Use before a review to inform model selection and system_prompt choices.",
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
                req.category.as_deref(),
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
fn reasoning_needs_extended_deadline(effort: Option<&str>) -> bool {
    matches!(effort, Some("medium" | "high" | "xhigh"))
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
                "Squall: parallel AI model dispatch via HTTP and CLI.\n\n\
                 Tools:\n\
                 - `listmodels`: List available models with provider, backend, and capability info.\n\
                 - `chat`: Single HTTP model query (OpenAI-compatible providers).\n\
                 - `clink`: Single CLI model query (gemini, codex — runs as subprocess).\n\
                 - `review`: Fan out a prompt to multiple models in parallel. ALWAYS prefer this over multiple chat/clink calls.\n\
                 - `memorize`: Save a learning (pattern, tactic, or model recommendation) to persistent memory.\n\
                 - `memory`: Read persistent memory (model stats, patterns, tactics, recommendations).\n\
                 - `flush`: Clean up branch-scoped memory after PR merge.\n\n\
                 Model Selection:\n\
                 1. Call `memory` with category \"recommend\" for past model recommendations.\n\
                 2. Call `listmodels` to see exact model names and capabilities.\n\
                 3. Pick an ensemble based on the task (e.g. fast models for drafts, reasoning models for analysis).\n\n\
                 Review Workflow:\n\
                 1. Select models (see Model Selection above).\n\
                 2. Set `per_model_system_prompts` to give each model a different review lens \
                 (e.g. security, architecture, correctness). Check `memory` category \"tactic\" for proven lenses.\n\
                 3. Call `review`. For security audits, complex architecture, or high-stakes changes, set `deep: true` \
                 (raises timeout to 600s, reasoning_effort to \"high\", max_tokens to 16384).\n\
                 4. The response includes a `results_file` path. This file persists on disk and survives context compaction — \
                 if you lose review details after a long conversation, read the results_file to recover them.\n\
                 5. ALWAYS call `memorize` after synthesizing review results to record patterns and model blind spots.\n\n\
                 File Context:\n\
                 - Pass `file_paths` + `working_directory` to include source files in the prompt.\n\
                 - For `review`, you can also pass `diff` with unified diff text (e.g. git diff output).\n\n\
                 Memory Workflow:\n\
                 - Before reviews: `memory` category \"recommend\" + \"pattern\" to check past learnings.\n\
                 - After reviews: `memorize` with category \"pattern\" for recurring findings, \"tactic\" for prompt strategies, \
                 \"recommend\" for model recommendations.\n\
                 - After PR merge: `flush` with the branch name to graduate patterns to codebase scope."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
