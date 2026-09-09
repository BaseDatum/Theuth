//! MCP client: one `rmcp` connection per configured server; each remote tool
//! becomes an [`crate::ErasedTool`] named `<server>__<tool>`.

use std::sync::Arc;

use futures::future::BoxFuture;
use kameo::actor::ActorRef;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};

use crate::registry::{RegisterTool, ToolRegistry};
use crate::{ErasedTool, McpServerConfig, McpTransport, ToolCx, ToolOutput, ToolSpec};

/// A remote tool. Holds the connection alive through `service`.
struct McpTool {
    spec: ToolSpec,
    remote_name: String,
    service: Arc<RunningService<RoleClient, ()>>,
}

impl ErasedTool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn summary(&self, args: &serde_json::Value) -> String {
        format!("{} {args}", self.spec.name)
    }

    fn call(
        &self,
        args: serde_json::Value,
        _: ToolCx,
    ) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let arguments = match args {
                serde_json::Value::Object(m) => m,
                serde_json::Value::Null => Default::default(),
                other => anyhow::bail!("arguments must be a JSON object, got {other}"),
            };
            let params =
                CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);
            let result = self.service.call_tool(params).await?;
            let content = result
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text(t) => t.text.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ToolOutput {
                content,
                is_error: result.is_error.unwrap_or(false),
            })
        })
    }
}

/// Connect to one server, list its tools and register each into `registry`.
/// Errors are returned for the caller to log; nothing panics.
pub async fn connect(cfg: McpServerConfig, registry: ActorRef<ToolRegistry>) -> anyhow::Result<()> {
    let service = match &cfg.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
            ().serve(TokioChildProcess::new(cmd)?).await?
        }
        McpTransport::Http { url } => {
            ().serve(StreamableHttpClientTransport::from_uri(url.as_str()))
                .await?
        }
    };
    let service = Arc::new(service);
    let tools = service.list_all_tools().await?;
    tracing::info!(server = %cfg.name, count = tools.len(), "mcp server connected");
    for t in tools {
        let tool = McpTool {
            spec: ToolSpec {
                name: format!("{}__{}", cfg.name, t.name),
                description: t.description.map(|d| d.into_owned()).unwrap_or_default(),
                schema: serde_json::Value::Object((*t.input_schema).clone()),
                risk: cfg.risk,
            },
            remote_name: t.name.into_owned(),
            service: Arc::clone(&service),
        };
        registry.ask(RegisterTool(Arc::new(tool))).await?;
    }
    Ok(())
}
