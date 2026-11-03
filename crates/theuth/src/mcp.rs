//! `t mcp --session <id>`: the stdio MCP server an ACP child gets in
//! `session/new.mcpServers` (design doc 75 §9). Three tools, each one RPC
//! call on the running engine with the child's theuth session id:
//! `list_skills`, `load_skill(name)`, `send_message(text)` → the parent's
//! `<message from=…>` input. Exits when stdin closes (the harness is gone)
//! or the engine's RPC socket drops, so nothing outlives either side.
//!
//! `t mcp` without `--session` (158 §12.3, W-h) bridges stdio to the same
//! catalogue the engine's HTTP MCP server exposes (`mcp_server::Catalog`),
//! over the same engine connection; audit lines go to the same file.

use std::path::Path;
use std::sync::Arc;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, ServiceExt};
use serde::Deserialize;

/// The proxy: one engine connection, one child session.
#[derive(Clone)]
pub struct Shim {
    session: String,
    client: Arc<WsClient>,
}

/// `load_skill` arguments.
#[derive(Deserialize, schemars::JsonSchema)]
struct LoadSkill {
    /// Skill name as listed by `list_skills`.
    name: String,
}

/// `send_message` arguments.
#[derive(Deserialize, schemars::JsonSchema)]
struct SendMessage {
    /// What the parent reads; it arrives as a `<message from=…>` input.
    text: String,
}

/// No arguments.
#[derive(Deserialize, schemars::JsonSchema)]
struct NoArgs {}

impl Shim {
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: jsonrpsee::core::params::ArrayParams,
    ) -> Result<T, String> {
        self.client
            .request(method, params)
            .await
            .map_err(|e| match e {
                jsonrpsee::core::ClientError::Call(e) => e.message().to_owned(),
                e => format!("engine unreachable: {e}"),
            })
    }

    async fn list_skills(&self) -> Result<String, String> {
        let skills: Vec<agent::SkillMeta> = self.call("list_skills", rpc_params![]).await?;
        if skills.is_empty() {
            return Ok("no skills".to_owned());
        }
        Ok(skills
            .iter()
            .map(|s| format!("{}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn load_skill(&self, p: LoadSkill) -> Result<String, String> {
        let b: agent::SkillBody = self.call("load_skill", rpc_params![p.name]).await?;
        let mut s = b.body;
        if !b.files.is_empty() {
            s.push_str("\n\nfiles:\n");
            for f in &b.files {
                s.push_str("- ");
                s.push_str(f);
                s.push('\n');
            }
        }
        Ok(s)
    }

    async fn send_message(&self, p: SendMessage) -> Result<String, String> {
        let r: serde_json::Value = self
            .call(
                "send_message",
                rpc_params![self.session.as_str(), "parent", p.text],
            )
            .await?;
        Ok(format!(
            "sent (id {})",
            r.get("id").and_then(|v| v.as_str()).unwrap_or("?")
        ))
    }
}

fn args<T: serde::de::DeserializeOwned>(req: &CallToolRequestParams) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::Object(
        req.arguments.clone().unwrap_or_default(),
    ))
    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))
}

impl ServerHandler for Shim {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Tools of the theuth session that spawned you. `send_message` reaches your parent.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new(
                "list_skills",
                "The parent theuth's skills: one `name: description` line each.",
                schema_for_type::<NoArgs>(),
            ),
            Tool::new(
                "load_skill",
                "The body of one parent theuth skill, plus its supporting file names.",
                schema_for_type::<LoadSkill>(),
            ),
            Tool::new(
                "send_message",
                "Send a message to the parent session (a question, a status, a result); it arrives as `<message from=…>` and the parent may answer with a new prompt.",
                schema_for_type::<SendMessage>(),
            ),
        ]))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let r = match req.name.as_ref() {
            "list_skills" => self.list_skills().await,
            "load_skill" => self.load_skill(args(&req)?).await,
            "send_message" => self.send_message(args(&req)?).await,
            other => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool {other}"),
                    None,
                ));
            }
        };
        Ok(match r {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
        }
        .into())
    }
}

/// Serve on stdio until the harness closes stdin or the engine drops: the
/// child shim with `session`; without, the catalogue bridge — every tool call
/// is `mcp_server_call` on the engine, which redacts and audits (158 §12.5).
pub async fn run(data_dir: &Path, listen: &str, session: Option<String>) -> anyhow::Result<()> {
    let client = crate::link_cli::connect(data_dir, listen)
        .await
        .map_err(|e| anyhow::anyhow!("no engine on {listen}: {e}"))?;
    let client = Arc::new(client);
    let watch = client.clone();
    match session {
        Some(session) => serve_until_gone(Shim { session, client }, watch).await,
        None => serve_until_gone(mcp_server::Bridge::new(client), watch).await,
    }
}

async fn serve_until_gone<H: ServerHandler>(
    handler: H,
    watch: Arc<WsClient>,
) -> anyhow::Result<()> {
    let running = handler.serve(rmcp::transport::stdio()).await?;
    let ct = running.cancellation_token();
    tokio::spawn(async move {
        watch.on_disconnect().await;
        ct.cancel();
    });
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::server::{RpcModule, Server};
    use jsonrpsee::ws_client::WsClientBuilder;
    use serde_json::json;

    /// The three tools over a real MCP connection against a fake engine:
    /// each maps to the right RPC method and params, and its text back.
    #[tokio::test]
    async fn tools_proxy_to_the_engine() {
        let server = Server::builder().build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let mut m = RpcModule::new(());
        m.register_method("list_skills", |_, _, _| {
            json!([{"name": "ponytail", "description": "lazy", "path": "/p", "always": false, "repo": null}])
        })
        .unwrap();
        m.register_method("load_skill", |p, _, _| {
            let (name,): (String,) = p.parse().unwrap();
            json!({"name": name, "body": format!("# {name}"), "files": ["a.md"]})
        })
        .unwrap();
        m.register_method("send_message", |p, _, _| {
            let (session, to, text): (String, String, String) = p.parse().unwrap();
            assert_eq!(
                (session.as_str(), to.as_str(), text.as_str()),
                ("s1", "parent", "hi")
            );
            json!({"id": "m1", "to": "session:x", "state": "accepted"})
        })
        .unwrap();
        let handle = server.start(m);

        let client = WsClientBuilder::default()
            .build(format!("ws://{addr}"))
            .await
            .unwrap();
        let (a, b) = tokio::io::duplex(1 << 16);
        // Both `serve`s block on the initialize handshake: run them together.
        let (server, mcp) = tokio::join!(
            Shim {
                session: "s1".into(),
                client: Arc::new(client)
            }
            .serve(tokio::io::split(a)),
            ().serve(tokio::io::split(b)),
        );
        let (server, mcp) = (server.unwrap(), mcp.unwrap());

        let mut names: Vec<_> = mcp
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["list_skills", "load_skill", "send_message"]);

        let text = |r: CallToolResult| {
            assert_ne!(r.is_error, Some(true), "{r:?}");
            r.content[0].as_text().unwrap().text.clone()
        };
        let call = |name: &'static str, a: serde_json::Value| {
            let mut p = CallToolRequestParams::new(name);
            if let serde_json::Value::Object(o) = a {
                p = p.with_arguments(o);
            }
            mcp.call_tool(p)
        };
        assert_eq!(
            text(call("list_skills", json!(null)).await.unwrap()),
            "ponytail: lazy"
        );
        assert_eq!(
            text(
                call("load_skill", json!({"name": "ponytail"}))
                    .await
                    .unwrap()
            ),
            "# ponytail\n\nfiles:\n- a.md\n"
        );
        assert_eq!(
            text(call("send_message", json!({"text": "hi"})).await.unwrap()),
            "sent (id m1)"
        );
        // Bad arguments are a protocol error, not a call on the engine.
        assert!(call("load_skill", json!({})).await.is_err());

        // An engine error is the tool's error text, not a protocol failure.
        handle.stop().unwrap();
        handle.stopped().await;
        let r = call("list_skills", json!(null)).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(
            r.content[0]
                .as_text()
                .unwrap()
                .text
                .starts_with("engine unreachable"),
            "{r:?}"
        );
        mcp.cancel().await.unwrap();
        server.waiting().await.unwrap();
    }
}
