use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

use crate::config::Config;
use crate::context;
use crate::dispatch::registry::Registry;
use crate::dispatch::ProviderRequest;
use crate::response::{PalMetadata, PalToolResponse};
use crate::review::ReviewExecutor;
use crate::tools::chat::ChatRequest;
use crate::tools::clink::ClinkRequest;
use crate::tools::listmodels::{ListModelsResponse, ModelInfo};
use crate::tools::review::ReviewRequest;


#[derive(Clone)]
pub struct SquallServer {
    registry: Arc<Registry>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SquallServer {
    pub fn new(config: Config) -> Self {
        let registry = Arc::new(Registry::from_config(config));
        Self {
            registry,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "chat",
        description = "Query a single AI model. Parameter is `prompt`, NOT `message`.",
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
            let file_result = context::resolve_file_context(
                file_paths,
                &base_dir,
                context::MAX_FILE_CONTEXT_BYTES,
            )
            .await
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            if let Some(ctx) = file_result.context {
                prompt = format!("{ctx}\n{prompt}");
            }
        }

        let provider_req = ProviderRequest {
            prompt,
            model: model.clone(),
            deadline: Instant::now() + Duration::from_secs(300), // HTTP gets 5 min
            working_directory: None,
            system_prompt: req.system_prompt,
            temperature: req.temperature,
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
        description = "Invoke a CLI agent (Gemini/Codex). Parameter is `prompt`, NOT `message`.",
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
        description = "Dispatch prompt to multiple models with straggler cutoff. Returns per-model results.",
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
            let file_result = context::resolve_file_context(
                file_paths,
                &base_dir,
                context::MAX_FILE_CONTEXT_BYTES,
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
        let mut review_response = executor.execute(&req, prompt, working_directory).await;
        review_response.files_skipped = files_skipped;

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
                "Squall: fast async dispatch to external AI models via HTTP and CLI subprocesses."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
