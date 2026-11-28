//! Agent tool for interacting with owner-created node terminals (in-memory
//! `engine_shell` instances, outside any session or job).
//!
//! Attaching a terminal (`[attached terminal <engine>:<shell> "<name>" · <node>]`)
//! is only a nudge: the model can list open shells on this engine or a named
//! mesh node and type into any matching shell via the existing `shell_input`
//! path, without creating a session or a job and without calling
//! `engine_terminal`.

use std::sync::Arc;

use replica::{PeerId, Transport};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::shell_registry::{self, ShellRegistry};
use tools::{Key, Risk, Tool, ToolCx, ToolOutput};

use crate::rpc_engine::{list_shells_in, send_shell_input_in};
use crate::rpc_forward as forward;

/// Registered tool name.
pub const NAME: &str = "node_shell";

/// Action performed by [`NodeShellTool`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeShellAction {
    /// List open owner-created shell ids on `engine` (default: this engine).
    List,
    /// Type `text` and/or `keys` into an open shell via `shell_input`.
    Input,
}

/// Arguments for [`NodeShellTool`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NodeShellArgs {
    /// `list` to list open shells on an engine, or `input` to type into an open shell.
    pub action: NodeShellAction,
    /// Mesh engine id or node name; omit (or leave empty) for this engine.
    #[serde(default)]
    pub engine: Option<String>,
    /// Open shell id (required for `input`, e.g. `"main"`, or `"<engine>:<shell>"`
    /// from an `[attached terminal <engine>:<shell> …]` marker).
    #[serde(default)]
    pub shell: Option<String>,
    /// Text to type verbatim into the shell (for `input`). Include `\r`/`\n`,
    /// set `enter: true`, or pass `keys: ["enter"]` to submit a command.
    #[serde(default)]
    pub text: Option<String>,
    /// Keys encoded for the shell's terminal modes, sent after `text` (for `input`).
    #[serde(default)]
    pub keys: Option<Vec<Key>>,
    /// When `true`, append `enter` after `text` (default `false`; `text` is sent
    /// verbatim like `shell_input`).
    #[serde(default)]
    pub enter: Option<bool>,
}

/// Agent tool to list and type into open owner-created node terminals.
pub struct NodeShellTool;

#[async_trait::async_trait]
impl Tool for NodeShellTool {
    const NAME: &'static str = NAME;
    const RISK: Risk = Risk::Mutating;
    type Args = NodeShellArgs;

    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "List or type into open owner-created node terminals (in-memory `engine_shell` instances on \
         this engine or a named mesh node). \
         If the owner mentions a terminal they created on a node, list shells (`action=\"list\"`, \
         optional `engine`) and use the matching one (`action=\"input\"`, with `shell`, optional \
         `engine`, and `text`/`keys`). \
         Attaching a terminal (`[attached terminal <engine>:<shell> \"<name>\" · <node>]`) is only \
         a nudge — you can find and type into any open shell by listing shells without the owner \
         attaching it. Do not create a session or a job. Do not call `engine_terminal`."
            .into()
    }

    async fn call(&self, args: NodeShellArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let transport = forward::transport().await;
        Ok(Self::execute_with(args, shell_registry::global(), transport).await)
    }
}

impl NodeShellTool {
    /// Execute against an explicit `registry` and `transport` (production uses
    /// [`shell_registry::global`] and [`forward::transport`]; tests inject
    /// isolated registries and loopback transports).
    pub(crate) async fn execute_with(
        args: NodeShellArgs,
        registry: &ShellRegistry,
        transport: Option<Arc<dyn Transport>>,
    ) -> ToolOutput {
        match args.action {
            NodeShellAction::List => {
                let peer = resolve_target(args.engine.as_deref()).await;
                let label = target_label(args.engine.as_deref(), peer.as_ref());
                match list_shells_in(registry, transport, peer.as_ref()).await {
                    Ok(shells) => {
                        let content = if shells.is_empty() {
                            format!("no open shells on {label}")
                        } else {
                            format!("open shells on {label}: {}", shells.join(", "))
                        };
                        let mut out = ToolOutput::ok(content);
                        out.structured = Some(serde_json::json!({
                            "engine": peer.as_ref().map(|p| p.0.as_str()),
                            "shells": shells,
                        }));
                        out
                    }
                    Err(e) => ToolOutput::err(e.message().to_owned()),
                }
            }
            NodeShellAction::Input => {
                let (engine, shell) = match parse_shell_target(
                    args.engine.as_deref(),
                    args.shell.as_deref().unwrap_or(""),
                ) {
                    Ok(t) => t,
                    Err(msg) => return ToolOutput::err(msg),
                };
                let text = args.text.as_deref().filter(|t| !t.is_empty());
                let mut keys = args.keys.unwrap_or_default();
                if args.enter == Some(true) {
                    keys.push(Key::Enter);
                }
                if text.is_none() && keys.is_empty() {
                    return ToolOutput::err("input requires `text` or `keys`");
                }
                let keys_slice = (!keys.is_empty()).then_some(keys.as_slice());
                let peer = resolve_target(engine.as_deref()).await;
                let label = target_label(engine.as_deref(), peer.as_ref());
                match send_shell_input_in(registry, transport, peer.as_ref(), &shell, text, keys_slice)
                    .await
                {
                    Ok(()) => ToolOutput::ok(format!("sent input to shell `{shell}` on {label}")),
                    Err(e) => ToolOutput::err(e.message().to_owned()),
                }
            }
        }
    }
}

/// Split optional `engine` and `shell` (accepting `<engine>:<shell>` when
/// `engine` is unset, as emitted in `[attached terminal <engine>:<shell> …]`).
fn parse_shell_target(
    engine: Option<&str>,
    shell: &str,
) -> Result<(Option<String>, String), String> {
    let engine = engine.map(str::trim).filter(|e| !e.is_empty());
    let shell = shell.trim();
    if shell.is_empty() {
        return Err("input requires a non-empty `shell` id".into());
    }
    if engine.is_none()
        && let Some((eng, sh)) = shell.split_once(':')
    {
        let eng = eng.trim();
        let sh = sh.trim();
        if eng.is_empty() || sh.is_empty() {
            return Err(format!("invalid `<engine>:<shell>` target `{shell}`"));
        }
        return Ok((Some(eng.to_owned()), sh.to_owned()));
    }
    Ok((engine.map(str::to_owned), shell.to_owned()))
}

/// Resolve an optional engine id or mesh display name to a remote [`PeerId`],
/// or `None` when it refers to this engine.
async fn resolve_target(engine: Option<&str>) -> Option<PeerId> {
    let engine = engine.map(str::trim).filter(|e| !e.is_empty())?;
    if mesh::self_id().is_some_and(|id| id == engine) || engine == mesh::self_name() {
        return None;
    }
    if let Ok(Some(agents)) = kameo::actor::ActorRef::<crate::Agents>::lookup(crate::AGENT_ACTOR)
        && let Ok(store) = agents.ask(crate::DurableStore).await
        && store.engine() == engine
    {
        return None;
    }
    let (self_name, peers) = crate::communication::mesh::engine_names().await;
    if !self_name.is_empty() && self_name == engine {
        return None;
    }
    if peers.contains_key(engine) {
        return Some(PeerId(engine.to_owned()));
    }
    let mut by_name = peers
        .iter()
        .filter(|(_, name)| name.as_str() == engine)
        .map(|(id, _)| id.clone());
    if let (Some(id), None) = (by_name.next(), by_name.next()) {
        return Some(PeerId(id));
    }
    let mut by_prefix = peers
        .keys()
        .filter(|id| id.starts_with(engine))
        .cloned();
    if let (Some(id), None) = (by_prefix.next(), by_prefix.next()) {
        return Some(PeerId(id));
    }
    Some(PeerId(engine.to_owned()))
}

fn target_label(requested: Option<&str>, peer: Option<&PeerId>) -> String {
    match (
        requested.map(str::trim).filter(|e| !e.is_empty()),
        peer,
    ) {
        (_, None) => "this engine".into(),
        (Some(name), Some(_)) => format!("engine `{name}`"),
        (None, Some(peer)) => format!("engine `{}`", peer.0),
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use jsonrpsee::Methods;
    use runtime::Runtime;
    use tools::PtyOut;

    use super::*;

    #[test]
    fn shells_parse_target_splits_attached_terminal_id() {
        assert_eq!(
            parse_shell_target(None, "xeus:main").unwrap(),
            (Some("xeus".into()), "main".into())
        );
        assert_eq!(
            parse_shell_target(Some("xeus"), "main").unwrap(),
            (Some("xeus".into()), "main".into())
        );
        assert_eq!(
            parse_shell_target(None, "main").unwrap(),
            (None, "main".into())
        );
        assert!(parse_shell_target(None, "").is_err());
        assert!(parse_shell_target(None, ":main").is_err());
        assert!(parse_shell_target(None, "xeus:").is_err());
    }

    #[tokio::test]
    async fn shells_tool_lists_and_types_via_shell_input_without_creating_session() {
        // Verify tool guidance covers owner-created terminals and forbids
        // creating sessions/jobs or calling engine_terminal.
        let desc = NodeShellTool.description();
        assert!(desc.contains("list shells"), "{desc}");
        assert!(desc.contains("Do not create a session or a job"), "{desc}");
        assert!(desc.contains("engine_terminal"), "{desc}");
        assert!(!NodeShellTool.promotable());

        // No Agents actor, no Jobs actor, no Store: proves neither a session
        // nor a job can be created by listing or typing into a shell.
        let dir = tempfile::tempdir().unwrap();
        let reg = ShellRegistry::new();
        let sh = Path::new("/bin/sh");
        let id = reg
            .open_or_reuse("main", (sh, &[]), dir.path(), Some(80), Some(24))
            .expect("open shell");
        let mut at = reg.attach(&id, None).expect("attach");

        let listed = NodeShellTool::execute_with(
            NodeShellArgs {
                action: NodeShellAction::List,
                engine: None,
                shell: None,
                text: None,
                keys: None,
                enter: None,
            },
            &reg,
            None,
        )
        .await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("main"), "{}", listed.content);
        assert_eq!(
            listed.structured.as_ref().and_then(|v| v.get("shells")),
            Some(&serde_json::json!(["main"]))
        );

        let typed = NodeShellTool::execute_with(
            NodeShellArgs {
                action: NodeShellAction::Input,
                engine: None,
                shell: Some("main".into()),
                text: Some("printf 'SHELL_INPUT_TOOL_OK\\n'".into()),
                keys: None,
                enter: Some(true),
            },
            &reg,
            None,
        )
        .await;
        assert!(!typed.is_error, "{}", typed.content);

        let mut buf = String::from_utf8_lossy(&at.replay).into_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !buf.contains("SHELL_INPUT_TOOL_OK") {
            let chunk = tokio::time::timeout_at(deadline, at.rx.recv())
                .await
                .expect("timed out waiting for shell output")
                .expect("broadcast open");
            if let PtyOut::Data(bytes) = chunk {
                buf.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(buf.contains("SHELL_INPUT_TOOL_OK"), "output was: {buf}");
        reg.close(&id);
    }

    #[tokio::test]
    async fn shells_tool_forwards_list_and_shell_input_to_named_remote_engine() {
        let dir = tempfile::tempdir().unwrap();
        let local_reg = ShellRegistry::new();
        let remote_reg = Arc::new(ShellRegistry::new());
        let sh = Path::new("/bin/sh");
        remote_reg
            .open_or_reuse("build", (sh, &[]), dir.path(), Some(80), Some(24))
            .expect("open remote shell");
        let mut remote_at = remote_reg.attach("build", None).expect("attach remote");

        // Serve shells_list and shell_input on the remote engine over theuth/rpc/1.
        let mut module = jsonrpsee::RpcModule::new(Arc::clone(&remote_reg));
        module
            .register_method("shells_list", |_, ctx, _| -> jsonrpsee::core::RpcResult<Vec<String>> {
                Ok(ctx.list())
            })
            .unwrap();
        module
            .register_method("shell_input", |params, ctx, _| -> jsonrpsee::core::RpcResult<()> {
                let (shell, text, keys): (String, Option<String>, Option<Vec<Key>>) =
                    params.parse()?;
                ctx.input(&shell, text.as_deref(), keys.as_deref().unwrap_or(&[]))
                    .map_err(rpc_core::failed)
            })
            .unwrap();
        let methods: Methods = module.into();

        let nodes = crate::communication::mesh::loopback::nodes(&["local", "remote"]);
        let local_t: Arc<dyn Transport> = nodes[0].clone();
        let remote_t: Arc<dyn Transport> = nodes[1].clone();
        let rt = Runtime::new(Default::default(), dir.path().to_path_buf());
        let serve_task = tokio::spawn(forward::serve(rt.clone(), remote_t, methods));

        let listed = NodeShellTool::execute_with(
            NodeShellArgs {
                action: NodeShellAction::List,
                engine: Some("remote".into()),
                shell: None,
                text: None,
                keys: None,
                enter: None,
            },
            &local_reg,
            Some(Arc::clone(&local_t)),
        )
        .await;
        assert!(!listed.is_error, "{}", listed.content);
        assert_eq!(
            listed.structured.as_ref().and_then(|v| v.get("shells")),
            Some(&serde_json::json!(["build"]))
        );

        // Type via `<engine>:<shell>` form (as emitted by `[attached terminal ...]`).
        let typed = NodeShellTool::execute_with(
            NodeShellArgs {
                action: NodeShellAction::Input,
                engine: None,
                shell: Some("remote:build".into()),
                text: Some("printf 'REMOTE_SHELL_OK\\n'".into()),
                keys: None,
                enter: Some(true),
            },
            &local_reg,
            Some(local_t),
        )
        .await;
        assert!(!typed.is_error, "{}", typed.content);

        let mut buf = String::from_utf8_lossy(&remote_at.replay).into_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !buf.contains("REMOTE_SHELL_OK") {
            let chunk = tokio::time::timeout_at(deadline, remote_at.rx.recv())
                .await
                .expect("timed out waiting for remote shell output")
                .expect("broadcast open");
            if let PtyOut::Data(bytes) = chunk {
                buf.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(buf.contains("REMOTE_SHELL_OK"), "output was: {buf}");

        rt.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), serve_task).await;
        remote_reg.close("build");
    }
}
