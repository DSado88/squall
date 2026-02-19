use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

use crate::config::Config;
use crate::dispatch::registry::Registry;
use crate::dispatch::ProviderRequest;
use crate::response::{PalMetadata, PalToolResponse};
use crate::tools::chat::ChatRequest;
use crate::tools::clink::ClinkRequest;
use crate::tools::listmodels::{ListModelsResponse, ModelInfo};


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
        let model = req.model_or_default().to_string();
        let start = Instant::now();

        let provider_req = ProviderRequest {
            prompt: req.prompt,
            model: model.clone(),
            deadline: Instant::now() + Duration::from_secs(120),
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
        let cli_name = req.cli_name.clone();
        let start = Instant::now();

        let provider_req = ProviderRequest {
            prompt: req.prompt,
            model: cli_name.clone(),
            deadline: Instant::now() + Duration::from_secs(300), // CLIs get 5 min
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
