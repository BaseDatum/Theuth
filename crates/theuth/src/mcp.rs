//! `t mcp --session <id>`: the stdio MCP server an ACP child gets in
//! `session/new.mcpServers` (design doc 75 §9). Nine tools, each one RPC
//! call on the running engine with the child's theuth session id:
//! `list_skills`, `load_skill(name)`, `send_message(text)` → the parent's
//! `<message from=…>` input; and (193 §7) the registry's `read_file`, `edit`,
//! `move_text`, `copy_text`, `indent` and `clip`, listed from `list_tools`
//! with the registry's own specs and run by `session_call_tool` in that
//! session (its cwd, approval policy and secret handling). Nothing outlives
//! either side (176 A3): it exits
//! when stdin closes (the harness is gone), within [`PARENT_POLL`] of its
//! parent process going away, when the engine says the session does not
//! exist, or after the engine was unreachable for [`ENGINE_DOWN`] — it
//! redials with backoff until then.
//!
//! `t mcp` without `--session` (158 §12.3, W-h) bridges stdio to the same
//! catalogue the engine's HTTP MCP server exposes (`mcp_server::Catalog`),
//! over the same engine connection; audit lines go to the same file. It
//! exits on stdin EOF, parent death, or the engine connection dropping.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use rpc::api::SESSION_TOOLS;
use rpc::api::clients::ToolingApiClient;
use serde::Deserialize;
use tokio::sync::watch;

/// 176 A3 (b): how often the parent pid is checked.
const PARENT_POLL: Duration = Duration::from_secs(2);
/// 176 A3 (d): the engine unreachable this long, continuously → exit.
const ENGINE_DOWN: Duration = Duration::from_secs(60);
/// 193 B2: the shim's request timeout: above the 300 s default `tools.approval_timeout_secs` a
/// `session_call_tool` can wait on a human, +30 s (the shim cannot cheaply read the config).
const SESSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(330);
/// Longest pause between redials.
const REDIAL_MAX: Duration = Duration::from_secs(10);

/// The proxy: one engine connection (replaced on redial), one child session.
#[derive(Clone)]
pub struct Shim {
    session: String,
    client: watch::Receiver<Arc<WsClient>>,
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
        let client = self.client.borrow().clone();
        client.request(method, params).await.map_err(|e| match e {
            jsonrpsee::core::ClientError::Call(e) => e.message().to_owned(),
            e => format!("engine unreachable: {e}"),
        })
    }

    /// Map a typed-client result as [`Shim::call`] does.
    fn mapped<T>(r: Result<T, jsonrpsee::core::ClientError>) -> Result<T, String> {
        r.map_err(|e| match e {
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

    /// 193 §7: one registry tool in the child's theuth session (cwd, approvals, secrets as for
    /// the session's own calls); a refusal or failure is the tool's error text.
    async fn session_tool(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        let client = self.client.borrow().clone();
        let session = self
            .session
            .parse()
            .map_err(|e| format!("bad session id: {e}"))?;
        let out = Self::mapped(
            client
                .session_call_tool(session, name.to_owned(), serde_json::Value::Object(args))
                .await,
        )?;
        if out.is_error {
            Err(out.content)
        } else {
            Ok(out.content)
        }
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
            "Tools of the theuth session that spawned you. `send_message` reaches your parent; `read_file`, `edit`, `move_text`, `copy_text`, `indent` and `clip` run in that session (its working directory and approvals).",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let client = self.client.borrow().clone();
        // A failed `list_tools` still lists the shim's own three tools, never an error.
        let specs =
            Self::mapped(ToolingApiClient::list_tools(&*client).await).unwrap_or_else(|e| {
                eprintln!("t mcp: list_tools failed: {e}");
                Vec::new()
            });
        // 193 §7: the six registry tools, with the registry's own descriptions and schemas.
        let registry = specs
            .into_iter()
            .filter(|s| SESSION_TOOLS.contains(&s.name.as_str()))
            .filter_map(|s| match s.schema {
                serde_json::Value::Object(o) => Some(Tool::new(s.name, s.description, Arc::new(o))),
                _ => None,
            });
        let own = [
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
        ];
        Ok(ListToolsResult::with_all_items(
            own.into_iter().chain(registry).collect(),
        ))
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
            name if SESSION_TOOLS.contains(&name) => {
                self.session_tool(name, req.arguments.clone().unwrap_or_default())
                    .await
            }
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

/// Serve on stdio until the harness closes stdin, the parent goes or the
/// engine is gone for good: the child shim with `session`; without, the
/// catalogue bridge — every tool call is `mcp_server_call` on the engine,
/// which redacts and audits (158 §12.5).
pub async fn run(data_dir: &Path, listen: &str, session: Option<String>) -> anyhow::Result<()> {
    let timeout = session.is_some().then_some(SESSION_REQUEST_TIMEOUT);
    let client = Arc::new(dial_with(data_dir, listen, timeout).await?);
    let parent = parent_gone(ppid, PARENT_POLL);
    let stdio = rmcp::transport::stdio();
    match session {
        Some(session) => {
            if no_such_session(&client, &session).await {
                anyhow::bail!("session {session} does not exist");
            }
            let (tx, rx) = watch::channel(client);
            let engine = keep_engine(data_dir, listen, session.clone(), tx, ENGINE_DOWN);
            let gone = async move {
                tokio::select! {
                    why = parent => why,
                    why = engine => why,
                }
            };
            serve_until_gone(
                Shim {
                    session,
                    client: rx,
                },
                stdio,
                gone,
            )
            .await
        }
        None => {
            let watch = client.clone();
            let gone = async move {
                tokio::select! {
                    why = parent => why,
                    _ = watch.on_disconnect() => "engine connection dropped".to_owned(),
                }
            };
            serve_until_gone(mcp_server::Bridge::new(client), stdio, gone).await
        }
    }
}

/// The engine connection, tagged `client=mcp-bridge` (176 C3); `listen` is
/// re-read from the engine record so a restarted engine is found.
async fn dial(data_dir: &Path, listen: &str) -> anyhow::Result<WsClient> {
    dial_with(data_dir, listen, None).await
}

/// [`dial`]; `Some(timeout)` is the `--session` shim's request timeout (193 B2).
async fn dial_with(
    data_dir: &Path,
    listen: &str,
    timeout: Option<Duration>,
) -> anyhow::Result<WsClient> {
    let listen = rpc::engine::discover(data_dir)
        .and_then(|r| r.record.listen)
        .unwrap_or_else(|| listen.to_owned());
    let url = format!("{listen}/?client=mcp-bridge");
    match timeout {
        Some(t) => crate::link_cli::connect_with_timeout(data_dir, &url, t).await,
        None => crate::link_cli::connect(data_dir, &url).await,
    }
    .map_err(|e| anyhow::anyhow!("no engine on {listen}: {e}"))
}

/// 176 A3 (c): the engine answers that `session` has no row. Any other
/// failure (an old engine, a refusal) is not proof and keeps the bridge.
async fn no_such_session(client: &WsClient, session: &str) -> bool {
    match client
        .request::<serde_json::Value, _>("session_owner", rpc_params![session])
        .await
    {
        Err(jsonrpsee::core::ClientError::Call(e)) => e.message().ends_with("no such session"),
        _ => false,
    }
}

/// 176 A3 (d): redial with backoff whenever the connection drops, handing
/// each new client to the shim; resolves with why once the engine stayed
/// unreachable for `limit` or says the session is gone.
async fn keep_engine(
    data_dir: &Path,
    listen: &str,
    session: String,
    tx: watch::Sender<Arc<WsClient>>,
    limit: Duration,
) -> String {
    loop {
        let client = tx.borrow().clone();
        client.on_disconnect().await;
        let down = Instant::now();
        let mut pause = Duration::from_millis(500);
        loop {
            let left = limit.saturating_sub(down.elapsed());
            if left.is_zero() {
                return format!("engine unreachable for {}s", limit.as_secs());
            }
            tokio::time::sleep(pause.min(left)).await;
            pause = (pause * 2).min(REDIAL_MAX);
            if let Ok(c) = dial_with(data_dir, listen, Some(SESSION_REQUEST_TIMEOUT)).await {
                if no_such_session(&c, &session).await {
                    return format!("session {session} does not exist");
                }
                tx.send_replace(Arc::new(c));
                break;
            }
        }
    }
}

#[cfg(unix)]
fn ppid() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(not(unix))]
fn ppid() -> u32 {
    0
}

/// 176 A3 (b): resolves once `ppid` differs from its first value or is 1
/// (re-parented to init/launchd: the harness died without closing stdin).
async fn parent_gone(ppid: impl Fn() -> u32, every: Duration) -> String {
    let first = ppid();
    loop {
        let now = ppid();
        if now == 1 || now != first {
            return format!("parent process {first} gone");
        }
        tokio::time::sleep(every).await;
    }
}

/// Serve `handler` on `transport` until the peer closes it (stdin EOF) or
/// `gone` resolves — during the handshake too; the reason goes to stderr,
/// which the harness logs.
async fn serve_until_gone<H, T, E, A>(
    handler: H,
    transport: T,
    gone: impl Future<Output = String>,
) -> anyhow::Result<()>
where
    H: ServerHandler,
    T: rmcp::transport::IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::pin!(gone);
    let running = tokio::select! {
        r = handler.serve(transport) => r?,
        why = &mut gone => {
            eprintln!("t mcp: {why}; exiting");
            return Ok(());
        }
    };
    let ct = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        r = &mut waiting => {
            r?;
        }
        why = gone => {
            eprintln!("t mcp: {why}; exiting");
            ct.cancel();
            (&mut waiting).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::server::{RpcModule, Server};
    use jsonrpsee::ws_client::WsClientBuilder;
    use serde_json::json;

    /// A well-formed session id (the typed client parses it).
    const SID: &str = "0190d7b0-0000-7000-8000-000000000001";

    /// The nine tools over a real MCP connection against a fake engine:
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
                (SID, "parent", "hi")
            );
            json!({"id": "m1", "to": "session:x", "state": "accepted"})
        })
        .unwrap();
        // The registry's list: the six, plus one the shim must not offer.
        m.register_method("list_tools", |_, _, _| {
            let spec = |name: &str, desc: &str| {
                json!({
                    "name": name, "description": desc, "risk": "mutating", "direct": false,
                    "returns": "str",
                    "schema": {"type": "object", "required": ["path", "range"], "properties": {}},
                })
            };
            let mut specs: Vec<_> = ["read_file", "edit", "move_text", "copy_text", "clip"]
                .map(|n| spec(n, "x"))
                .into();
            specs.push(spec("indent", "Re-indent lines."));
            specs.push(spec("shell", "must not be offered"));
            json!(specs)
        })
        .unwrap();
        m.register_method("session_call_tool", |p, _, _| {
            let (session, name, args): (String, String, serde_json::Value) = p.parse().unwrap();
            assert_eq!(session, SID);
            assert_ne!(name, "shell");
            let out = |content: &str, is_error: bool| {
                json!({"content": content, "is_error": is_error, "structured": null, "files": []})
            };
            match name.as_str() {
                "indent" => {
                    assert_eq!(args["range"], "L1-L2@1a2b3c4d");
                    out("indented a.rs:1-2 by +1", false)
                }
                // 193 §7: array-valued `edits` reach the engine unchanged.
                "edit" => {
                    assert_eq!(args["edits"], json!([{"at": "a.rs#L1@1a2b3c4d", "text": "x"}]));
                    out("edited", false)
                }
                _ => out("clip: no such ref", true),
            }
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
                session: SID.into(),
                client: watch::channel(Arc::new(client)).1
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
        assert_eq!(
            names,
            [
                "clip",
                "copy_text",
                "edit",
                "indent",
                "list_skills",
                "load_skill",
                "move_text",
                "read_file",
                "send_message"
            ]
        );
        // The registry's own description and schema, not the shim's.
        let indent = mcp
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.name == "indent")
            .unwrap();
        assert_eq!(indent.description.as_deref(), Some("Re-indent lines."));
        assert_eq!(indent.input_schema["required"], json!(["path", "range"]));

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
        // 193 §7: a registry tool runs in the child's session; the output text comes back,
        // and an `is_error` output is the tool's error text.
        assert_eq!(
            text(
                call(
                    "indent",
                    json!({"path": "a.rs", "range": "L1-L2@1a2b3c4d", "by": 1})
                )
                .await
                .unwrap()
            ),
            "indented a.rs:1-2 by +1"
        );
        assert_eq!(
            text(
                call(
                    "edit",
                    json!({"edits": [{"at": "a.rs#L1@1a2b3c4d", "text": "x"}]})
                )
                .await
                .unwrap()
            ),
            "edited"
        );
        let r = call("clip", json!({"name": "x", "src": "nope"}))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(r.content[0].as_text().unwrap().text, "clip: no such ref");
        // A tool outside the allowlist is not offered and not forwarded.
        assert!(call("shell", json!({"command": "ls"})).await.is_err());
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

    /// A shim on an engine with no methods (the lifecycle tests call none).
    async fn idle_shim() -> (Shim, jsonrpsee::server::ServerHandle) {
        let server = Server::builder().build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let handle = server.start(RpcModule::new(()));
        let client = WsClientBuilder::default()
            .build(format!("ws://{addr}"))
            .await
            .unwrap();
        let shim = Shim {
            session: SID.into(),
            client: watch::channel(Arc::new(client)).1,
        };
        (shim, handle)
    }

    /// 193 §7: a failed `list_tools` still lists exactly the shim's own three tools.
    #[tokio::test]
    async fn failed_list_tools_lists_only_the_own_three() {
        let (shim, _engine) = idle_shim().await;
        let (a, b) = tokio::io::duplex(1 << 16);
        let (server, mcp) = tokio::join!(
            shim.serve(tokio::io::split(a)),
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
        mcp.cancel().await.unwrap();
        server.waiting().await.unwrap();
    }

    /// 176 A3 (a): the harness closing its end (stdin EOF) ends the bridge.
    #[tokio::test]
    async fn bridge_exits_on_stdin_eof() {
        let (shim, _engine) = idle_shim().await;
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, ()) = tokio::join!(
            serve_until_gone(shim, tokio::io::split(a), std::future::pending()),
            async {
                let mcp = ().serve(tokio::io::split(b)).await.unwrap();
                mcp.cancel().await.unwrap();
            },
        );
        served.unwrap();
    }

    /// 176 A3 (b): the parent re-parenting the bridge to init ends it, and
    /// the harness side sees its server close.
    #[tokio::test]
    async fn bridge_exits_when_its_parent_dies() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let (shim, _engine) = idle_shim().await;
        let parent = Arc::new(AtomicU32::new(4242));
        let p = parent.clone();
        let gone = parent_gone(move || p.load(Ordering::SeqCst), Duration::from_millis(10));
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, ()) = tokio::join!(serve_until_gone(shim, tokio::io::split(a), gone), async {
            let mcp = ().serve(tokio::io::split(b)).await.unwrap();
            parent.store(1, Ordering::SeqCst);
            tokio::time::timeout(Duration::from_secs(5), mcp.waiting())
                .await
                .expect("the bridge closed its end")
                .unwrap();
        },);
        served.unwrap();
    }

    /// 176 A3 (b): a changed parent pid counts as gone, not only pid 1.
    #[tokio::test]
    async fn parent_gone_on_any_change() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let ppid = || {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                7
            } else {
                8
            }
        };
        let why = tokio::time::timeout(
            Duration::from_secs(1),
            parent_gone(ppid, Duration::from_millis(1)),
        )
        .await
        .unwrap();
        assert_eq!(why, "parent process 7 gone");
    }

    /// A scratch data dir with the bearer token `connect` reads.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("176-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("token"), "t").unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fake engine on `addr` whose `session_owner` knows `s1` or not.
    async fn engine(addr: &str, known: bool) -> (String, jsonrpsee::server::ServerHandle) {
        let server = Server::builder().build(addr).await.unwrap();
        let addr = server.local_addr().unwrap().to_string();
        let mut m = RpcModule::new(());
        m.register_method("session_owner", move |_, _, _| {
            if known {
                Ok(json!({"owner": "me", "here": true}))
            } else {
                Err(jsonrpsee::types::ErrorObjectOwned::owned(
                    -32000,
                    "s1: no such session",
                    None::<()>,
                ))
            }
        })
        .unwrap();
        (addr, server.start(m))
    }

    /// 176 A3 (c)/(d): a dropped engine is redialed and the new connection
    /// handed to the shim; an engine that comes back without the session
    /// ends the bridge.
    #[tokio::test]
    async fn bridge_redials_then_exits_on_unknown_session() {
        let scratch = Scratch::new("redial");
        let (listen, first) = engine("127.0.0.1:0", true).await;
        let client = dial(&scratch.0, &listen).await.unwrap();
        assert!(!no_such_session(&client, "s1").await);
        let (tx, mut rx) = watch::channel(Arc::new(client));
        let (dir, at) = (scratch.0.clone(), listen.clone());
        let keep = tokio::spawn(async move {
            keep_engine(&dir, &at, "s1".into(), tx, Duration::from_secs(30)).await
        });
        first.stop().unwrap();
        first.stopped().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_, second) = engine(&listen, true).await;
        tokio::time::timeout(Duration::from_secs(10), rx.changed())
            .await
            .expect("redialed")
            .unwrap();
        second.stop().unwrap();
        second.stopped().await;
        let (_, _third) = engine(&listen, false).await;
        let why = tokio::time::timeout(Duration::from_secs(15), keep)
            .await
            .expect("the bridge gave up")
            .unwrap();
        assert_eq!(why, "session s1 does not exist");
    }

    /// 176 A3 (d): an engine that stays away past the limit ends the bridge.
    #[tokio::test]
    async fn bridge_exits_after_engine_down_limit() {
        let scratch = Scratch::new("down");
        let (listen, engine) = engine("127.0.0.1:0", true).await;
        let client = dial(&scratch.0, &listen).await.unwrap();
        let (tx, _rx) = watch::channel(Arc::new(client));
        engine.stop().unwrap();
        engine.stopped().await;
        let why = tokio::time::timeout(
            Duration::from_secs(10),
            keep_engine(&scratch.0, &listen, "s1".into(), tx, Duration::from_secs(1)),
        )
        .await
        .expect("the bridge gave up");
        assert_eq!(why, "engine unreachable for 1s");
    }
}
