//! MCP client: every `*.json` in `<data_dir>/mcp/` holds one or more servers in
//! whatever layout its README used; each remote tool registers under its own
//! name (no prefix). The directory is watched and reconciled on change;
//! `<data_dir>/mcp/config.yaml` overrides a tool's [`Risk`] per server or per
//! tool, or turns it `off` (design docs 01 §1.4, 12 §5). A tool's default risk
//! comes from its `ToolAnnotations`. `<data_dir>/mcp/repos/` holds git clones
//! of server sources; an entry's `cwd` resolves against the mcp dir (design
//! doc 04 §3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use notify::Watcher;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{ClientInitializeError, RoleClient, RunningService, ServiceExt};
use rmcp::transport::auth::{
    AuthClient, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::{HiddenTools, ListTools, RegisterTool, ToolRegistry, UnregisterTool};
use crate::{
    DynTool, ErasedTool, Gate, MCP_ACTOR, Policy, PolicyAction, Risk, ToolCx, ToolOutput, ToolSpec,
    git,
};

mod oauth;
mod tools;

/// A server that has not answered `initialize` + `tools/list` by then is dropped
/// and recorded as a connect error; the next reconcile retries it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How to reach a server; the normalised form of one file entry.
#[derive(Clone, Debug, PartialEq)]
enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        /// Absolute; relative `cwd` in the file was joined onto the mcp dir.
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
        /// Pre-registered OAuth client, for servers offering neither DCR nor CIMD.
        oauth: Option<OauthClient>,
    },
}

/// The entry's optional `oauth` object (design doc 05 §3.2). Every field is
/// optional: `client_id` (+ `client_secret`) for servers offering neither DCR
/// nor CIMD, `scopes`, and `redirect_uri` for servers that only accept a
/// registered loopback URL (`http://localhost:8765/callback`).
#[derive(Clone, Debug, Default, PartialEq)]
struct OauthClient {
    client_id: Option<String>,
    client_secret: Option<String>,
    scopes: Vec<String>,
    redirect_uri: Option<String>,
}

impl OauthClient {
    /// Loopback port to listen on and the redirect URI to register: the
    /// configured one, else an ephemeral `http://127.0.0.1:<port>/callback`.
    fn redirect(&self) -> Result<(u16, Option<String>), String> {
        let Some(uri) = &self.redirect_uri else {
            return Ok((0, None));
        };
        let parsed = reqwest::Url::parse(uri).map_err(|e| format!("oauth.redirect_uri: {e}"))?;
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| format!("oauth.redirect_uri `{uri}` has no port"))?;
        Ok((port, Some(uri.clone())))
    }
}

/// One desired server. Equality is what "changed" means for reconcile.
#[derive(Clone, Debug, PartialEq)]
struct ServerSpec {
    transport: Transport,
}

impl ServerSpec {
    fn cwd(&self) -> Option<&Path> {
        match &self.transport {
            Transport::Stdio { cwd, .. } => cwd.as_deref(),
            Transport::Http { .. } => None,
        }
    }
}

fn is_entry(v: &Value) -> bool {
    v.get("command").is_some() || v.get("url").is_some()
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Normalise one entry; `None` for disabled or malformed. `dir` anchors a
/// relative `cwd` (`Path::join` keeps an absolute one as is).
fn entry(v: &Value, dir: &Path) -> Option<ServerSpec> {
    if v.get("enabled") == Some(&Value::Bool(false))
        || v.get("disabled") == Some(&Value::Bool(true))
    {
        return None;
    }
    if let Some(url) = v.get("url").and_then(Value::as_str) {
        // `type` (http/sse/remote/streamable-http) is not consulted: everything is streamable HTTP.
        let str_of = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
        let oauth = v
            .get("oauth")
            .filter(|o| o.is_object())
            .map(|o| OauthClient {
                client_id: str_of(o, "client_id"),
                client_secret: str_of(o, "client_secret"),
                scopes: strings(o.get("scopes")),
                redirect_uri: str_of(o, "redirect_uri"),
            });
        return Some(ServerSpec {
            transport: Transport::Http {
                url: url.to_owned(),
                headers: string_map(v.get("headers")),
                oauth,
            },
        });
    }
    let env = string_map(v.get("env").or_else(|| v.get("environment")));
    let (command, args) = match v.get("command")? {
        Value::String(c) => (c.clone(), strings(v.get("args"))),
        Value::Array(a) => {
            let mut parts = a.iter().filter_map(Value::as_str).map(str::to_owned);
            let exe = parts.next()?;
            let mut args: Vec<String> = parts.collect();
            args.extend(strings(v.get("args")));
            (exe, args)
        }
        other => {
            tracing::warn!(command = %other, "mcp entry: `command` must be a string or array");
            return None;
        }
    };
    let cwd = v.get("cwd").and_then(Value::as_str).map(|c| dir.join(c));
    Some(ServerSpec {
        transport: Transport::Stdio {
            command,
            args,
            env,
            cwd,
        },
    })
}

/// Structural detection: an entry is any object with `command` or `url`. Root
/// entry → one server named `stem`; else top-level entries and one wrapper
/// level (`mcpServers`, `mcp`, `servers`, …).
fn parse_file(stem: &str, json: &Value, dir: &Path) -> Vec<(String, ServerSpec)> {
    if is_entry(json) {
        return entry(json, dir)
            .map(|s| vec![(stem.to_owned(), s)])
            .unwrap_or_default();
    }
    let Some(root) = json.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, v) in root {
        if is_entry(v) {
            out.extend(entry(v, dir).map(|s| (k.clone(), s)));
        } else if let Some(wrapper) = v.as_object() {
            for (k2, v2) in wrapper {
                if is_entry(v2) {
                    out.extend(entry(v2, dir).map(|s| (k2.clone(), s)));
                }
            }
        }
    }
    out
}

/// What [`remove_entry`] decided for the file.
#[derive(Debug, PartialEq)]
enum FileEdit {
    /// The file held only `name` as a bare entry: delete it.
    Delete,
    /// `"disabled": true` was set on `name`'s entry: rewrite the file.
    Rewrite,
}

/// Drop server `name` from a file's JSON, mirroring [`parse_file`]'s shapes;
/// `None` when the file does not hold it.
fn remove_entry(json: &mut Value, stem: &str, name: &str) -> Option<FileEdit> {
    if is_entry(json) {
        return (stem == name).then_some(FileEdit::Delete);
    }
    let root = json.as_object_mut()?;
    let target = if root.get(name).is_some_and(is_entry) {
        root.get_mut(name)
    } else {
        root.values_mut()
            .filter_map(Value::as_object_mut)
            .find_map(|w| w.get_mut(name).filter(|v| is_entry(v)))
    };
    target?
        .as_object_mut()?
        .insert("disabled".to_owned(), Value::Bool(true));
    Some(FileEdit::Rewrite)
}

/// `<data_dir>/mcp/config.yaml`: per-server and per-tool [`Gate`] lines, a
/// [`Risk`] override or `off` (12 §5). Machine-written by [`SetGate`]
/// (comments are not preserved); `BTreeMap` keeps the output stable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Gates {
    servers: BTreeMap<String, ServerGate>,
}

/// `deny_unknown_fields` so a pre-12 §5 `gate: allow` line fails loudly
/// instead of being ignored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerGate {
    /// Every tool of the server; `None` = each tool's annotations.
    #[serde(skip_serializing_if = "Option::is_none")]
    risk: Option<Gate>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tools: BTreeMap<String, Gate>,
}

impl Gates {
    /// The server line; `None` = annotations.
    fn for_server(&self, server: &str) -> Option<Gate> {
        self.servers.get(server).and_then(|g| g.risk)
    }

    /// `Some(Off)` on the server line: not connected.
    fn server_off(&self, server: &str) -> bool {
        self.for_server(server) == Some(Gate::Off)
    }

    /// tool line → server line → `None` (annotations).
    fn for_tool(&self, server: &str, tool: &str) -> Option<Gate> {
        self.servers
            .get(server)
            .and_then(|g| g.tools.get(tool).copied().or(g.risk))
    }

    /// The tool's own line, ignoring inheritance.
    fn own(&self, server: &str, tool: &str) -> Option<Gate> {
        self.servers.get(server)?.tools.get(tool).copied()
    }

    /// Set or clear (`None`) a server or tool line; an emptied server entry is dropped.
    fn set(&mut self, server: &str, tool: Option<&str>, gate: Option<Gate>) {
        let g = self.servers.entry(server.to_owned()).or_default();
        match (tool, gate) {
            (None, a) => g.risk = a,
            (Some(t), Some(a)) => {
                g.tools.insert(t.to_owned(), a);
            }
            (Some(t), None) => {
                g.tools.remove(t);
            }
        }
        if g.risk.is_none() && g.tools.is_empty() {
            self.servers.remove(server);
        }
    }
}

/// Missing file = annotations; unreadable/unparsable = warn + annotations.
fn load_gates(path: &Path) -> Gates {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Gates::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "mcp gates unreadable");
            return Gates::default();
        }
    };
    match serde_yaml::from_str::<Option<Gates>>(&text) {
        Ok(g) => g.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(), error = %e,
                "mcp gates invalid (values are read_only | mutating | destructive | off), all default"
            );
            Gates::default()
        }
    }
}

/// Risk from the remote tool's annotations: `read_only_hint` → read-only,
/// `destructive_hint` → destructive, else mutating. rmcp's
/// `is_destructive()` reads an *unset* hint as destructive; we do not (12 §5):
/// most servers set nothing, and `tools.policy.mutating = ask` already
/// prompts. A server that means destructive says so.
fn annotated(t: &rmcp::model::Tool) -> Risk {
    let a = t.annotations.as_ref();
    if a.and_then(|a| a.read_only_hint) == Some(true) {
        Risk::ReadOnly
    } else if a.and_then(|a| a.destructive_hint) == Some(true) {
        Risk::Destructive
    } else {
        Risk::Mutating
    }
}

type Service = Arc<RunningService<RoleClient, ()>>;

/// A remote tool. Holds the connection alive through `service`. Its
/// `spec.risk` is the only thing the registry gates on (12 §5).
struct McpTool {
    spec: ToolSpec,
    remote_name: String,
    service: Service,
}

impl ErasedTool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn call(&self, args: Value, _: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let arguments = match args {
                Value::Object(m) => m,
                Value::Null => Default::default(),
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
                structured: result.structured_content,
                files: Vec::new(),
            })
        })
    }
}

/// A connected server. Dropping `service` ends the child process / session.
struct Running {
    spec: ServerSpec,
    service: Service,
    /// Remote tool list, kept so a gates change can re-register without a round trip.
    remote: Vec<rmcp::model::Tool>,
    /// Remote name → registered name, for what is currently registered from this server.
    tools: BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    running: HashMap<String, Running>,
    /// Servers with a connect task in flight.
    pending: HashSet<String>,
    gates: Gates,
    /// Native tool names; always win a clash.
    natives: HashSet<String>,
    /// Last connect / list_tools failure per desired server; cleared on success
    /// or when the server leaves the files.
    errors: HashMap<String, String>,
    /// Servers that answered 401 on connect: the `WWW-Authenticate` challenge,
    /// consumed by [`McpLogin`]. Cleared with `errors`.
    login: HashMap<String, String>,
}

/// Event: a reconcile snapshot, published every time a server changes state
/// (also mid-reconcile as each connect settles). UIs show it as a status
/// line; `connecting` empty means the reconcile is over.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct McpProgress {
    /// Servers still being connected, sorted.
    pub connecting: Vec<String>,
    /// Connected servers, sorted.
    pub connected: Vec<String>,
    /// Servers whose last connect failed, sorted.
    pub failed: Vec<String>,
}

/// The MCP actor. Look up with `ActorRef::<Mcp>::lookup(MCP_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Mcp {
    state: State,
    dir: PathBuf,
    /// `<data_dir>/oauth/mcp`; `<name>.json` holds a server's rmcp credentials.
    auth_dir: PathBuf,
    registry: ActorRef<ToolRegistry>,
    /// `tools.policy`, read at start alongside the registry's copy; only used
    /// to show what a tool's risk resolves to ([`McpToolInfo::action`]).
    policy: Policy,
    rt: Runtime,
}

/// `tell` from the watcher: re-read `<data_dir>/mcp` and apply.
struct Reconcile;

/// `ask` → `Vec<McpServer>`: every server the files want, sorted by name.
/// With a session, [`McpToolInfo::hidden`] reflects its [`crate::SetHidden`] state.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ListServers {
    /// Session whose in-memory hides to report; `None` reports none.
    pub session: Option<runtime::SessionId>,
}

/// `ask` → `Result<(), String>`: set (or clear with `gate: None`) a server
/// or tool line, rewrite `config.yaml`, reconcile now.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SetGate {
    /// Server name (file stem or entry key).
    pub server: String,
    /// `None` = the server line.
    pub tool: Option<String>,
    /// `None` removes the line (back to annotations).
    pub gate: Option<Gate>,
}

/// `ask` → `Result<(), String>`: drop one server's connection and tools, then
/// reconnect and re-register even though its entry is unchanged (a repo pull
/// rebuilt the binary). Err for a name no file wants.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Restart {
    /// Server name.
    pub name: String,
}

/// `ask` → `Result<McpServer, String>`: write `entry` as `<name>.json`
/// (bare-entry form), reconcile, and report the connected server. On connect
/// failure the file is removed again and the error returned.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AddServer {
    /// File stem and server name, `[a-z0-9][a-z0-9._-]*`.
    pub name: String,
    /// One server object: `command`/`args`/`env`/`cwd` or `url`/`headers`.
    pub entry: Value,
}

/// `ask` → `Result<(), String>`: delete `<name>.json` when it is a bare entry,
/// otherwise set `"disabled": true` on the entry in its file; reconcile.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RemoveServer {
    /// Server name.
    pub name: String,
}

/// `ask` → `Result<RepoInfo, String>`: `git clone --depth 1` into
/// `mcp/repos/<name>`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AddRepo {
    /// Anything `git clone` accepts.
    pub url: String,
    /// Directory name; defaults to the URL's last segment minus `.git`.
    pub name: Option<String>,
}

/// A cloned server source.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RepoInfo {
    /// Directory name under `mcp/repos/`.
    pub name: String,
    /// Absolute path of the clone.
    pub path: String,
    /// First 60 lines of its `README.md` / `README`; empty when none.
    pub readme: String,
}

/// `ask` → `Vec<SyncResult>`: `git pull --ff-only` every repo under
/// `mcp/repos/`; servers whose `cwd` is inside an updated repo are restarted.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SyncMcp;

/// `ask` → `Result<String, String>`: start an OAuth login for a remote server
/// and return the URL to open (the browser is also opened). On success the
/// server is restarted and connects with the saved tokens (visible as
/// [`McpProgress`]); on failure `failed` gains the server with a
/// `login failed` error.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpLogin {
    /// Server name.
    pub server: String,
}

/// `ask` → `Result<(), String>`: delete the server's tokens and restart it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpLogout {
    /// Server name.
    pub server: String,
}

/// Where a server stands with OAuth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum McpAuth {
    /// Stdio, hand-set header, or a remote that never asked.
    None,
    /// The server answered 401; `/login <name>`.
    Required,
    /// A token file exists.
    LoggedIn,
}

/// One server as the files want it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpServer {
    /// Server name.
    pub name: String,
    /// False when `off` or connect failed; then `tools` is empty.
    pub connected: bool,
    /// Server line of `mcp/config.yaml`; `None` = each tool's annotations.
    pub gate: Option<Gate>,
    /// Remote tools, sorted by name.
    pub tools: Vec<McpToolInfo>,
    /// Name of the clone under `mcp/repos/` that holds the entry's `cwd`, if any.
    pub repo: Option<String>,
    /// OAuth standing (design doc 05 §3.1).
    pub auth: McpAuth,
}

/// One remote tool of a connected server.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpToolInfo {
    /// Remote tool name (also the registered name).
    pub name: String,
    /// Remote description.
    pub description: String,
    /// The tool's own line, if any.
    pub gate: Option<Gate>,
    /// Effective risk: tool line → server line → annotations.
    pub risk: Risk,
    /// What `tools.policy` does for `risk`; for display only.
    pub action: PolicyAction,
    /// Name in the registry: the remote name, or `<server>__<name>` when
    /// another server or a native offers the same name. `None` when `off`.
    pub registered: Option<String>,
    /// Hidden for the session asked about ([`crate::SetHidden`]).
    pub hidden: bool,
}

impl Message<Reconcile> for Mcp {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, ctx: &mut Context<Self, Self::Reply>) {
        self.reconcile(ctx.actor_ref()).await;
    }
}

impl Mcp {
    /// One [`McpServer`] from the current state; `spec` is the file's entry
    /// when the server is not running; `hidden` is the session's hide set.
    fn server(
        &self,
        name: String,
        spec: Option<&ServerSpec>,
        hidden: &HashSet<String>,
    ) -> McpServer {
        let state = &self.state;
        let running = state.running.get(&name);
        let mut tools: Vec<McpToolInfo> = running
            .map(|r| {
                r.remote
                    .iter()
                    .map(|t| {
                        let tool = t.name.to_string();
                        let risk = risk_of(&state.gates, &name, t);
                        McpToolInfo {
                            gate: state.gates.own(&name, &tool),
                            risk,
                            action: self.policy.action(risk),
                            hidden: r.tools.get(&tool).is_some_and(|n| hidden.contains(n)),
                            registered: r.tools.get(&tool).cloned(),
                            description: t.description.as_deref().unwrap_or_default().to_owned(),
                            name: tool,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        McpServer {
            connected: running.is_some(),
            gate: state.gates.for_server(&name),
            tools,
            auth: if state.login.contains_key(&name) {
                McpAuth::Required
            } else if self.auth_path(&name).exists() {
                McpAuth::LoggedIn
            } else {
                McpAuth::None
            },
            repo: running
                .map(|r| &r.spec)
                .or(spec)
                .and_then(|s| repo_of(&self.dir, s)),
            name,
        }
    }

    fn auth_path(&self, name: &str) -> PathBuf {
        self.auth_dir.join(format!("{name}.json"))
    }

    /// Err unless some file wants `name` (or it is still running).
    fn known(&self, name: &str) -> Result<(), String> {
        if self.state.running.contains_key(name) || desired(&self.dir).contains_key(name) {
            Ok(())
        } else {
            Err(format!("unknown mcp server `{name}`"))
        }
    }
}

/// The clone under `<dir>/repos/` containing the entry's `cwd`.
fn repo_of(dir: &Path, spec: &ServerSpec) -> Option<String> {
    git::owner(&dir.join("repos"), spec.cwd()?)
}

impl Message<ListServers> for Mcp {
    type Reply = Vec<McpServer>;

    async fn handle(
        &mut self,
        m: ListServers,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<McpServer> {
        let hidden = match m.session {
            Some(s) => self.registry.ask(HiddenTools(s)).await.unwrap_or_default(),
            None => HashSet::new(),
        };
        let want = desired(&self.dir);
        let names: BTreeSet<String> = want
            .keys()
            .chain(self.state.running.keys())
            .cloned()
            .collect();
        names
            .into_iter()
            .map(|name| {
                let spec = want.get(&name);
                self.server(name, spec, &hidden)
            })
            .collect()
    }
}

impl Message<SetGate> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetGate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.server)?;
        // Edit a copy and let `reconcile` load it from the file: it diffs the
        // file against `state.gates` to decide whether to re-register.
        let mut gates = self.state.gates.clone();
        gates.set(&m.server, m.tool.as_deref(), m.gate);
        let text = serde_yaml::to_string(&gates).map_err(|e| e.to_string())?;
        // Write-then-rename so the watcher never reads a half file.
        let path = self.dir.join("config.yaml");
        let tmp = self.dir.join("config.yaml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        self.reconcile(ctx.actor_ref()).await;
        Ok(())
    }
}

/// Pretty JSON, write-then-rename so the watcher never reads a half file.
fn write_json(path: &Path, json: &Value) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(json).map_err(|e| e.to_string())?,
    )
    .and_then(|()| std::fs::rename(&tmp, path))
    .map_err(|e| format!("write {}: {e}", path.display()))
}

impl Message<Restart> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: Restart,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.name)?;
        self.restart(ctx.actor_ref(), &[m.name]).await;
        Ok(())
    }
}

impl Message<McpLogin> for Mcp {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        m: McpLogin,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<String, String> {
        let name = m.server;
        let Some(spec) = desired(&self.dir).remove(&name) else {
            return Err(format!("unknown mcp server `{name}`"));
        };
        let Transport::Http { url, oauth, .. } = spec.transport else {
            return Err(format!("{name} is a stdio server; nothing to log in to"));
        };
        let oauth = oauth.unwrap_or_default();
        let (want_port, redirect) = oauth.redirect()?;
        let (port, back) = ::oauth::callback(want_port)
            .await
            .map_err(|e| format!("callback listener on 127.0.0.1:{want_port}: {e}"))?;
        let mut manager = AuthorizationManager::new(url.as_str())
            .await
            .map_err(|e| e.to_string())?;
        manager.set_credential_store(oauth::FileStore::new(self.auth_path(&name)));
        // Resolve before configuring the client (design doc 05 §6).
        let resolved = manager
            .resolve_metadata_from_challenge(self.state.login.get(&name).map(String::as_str))
            .await
            .map_err(|e| e.to_string())?;
        manager.set_metadata(resolved.metadata);
        let redirect = redirect.unwrap_or_else(|| format!("http://127.0.0.1:{port}/callback"));
        let mut req = AuthorizationRequest::new(redirect).with_client_name("wth");
        if let Some(id) = oauth.client_id {
            req = req.with_preregistered_client(id);
            if let Some(s) = oauth.client_secret {
                req = req.with_client_secret(s);
            }
        }
        if !oauth.scopes.is_empty() {
            req = req.with_scopes(oauth.scopes);
        }
        let session = AuthorizationSession::new(manager, req)
            .await
            .map_err(|(_, e)| e.to_string())?;
        let url = session.get_authorization_url().to_owned();
        ::oauth::open_browser(&url);
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let done = match tokio::time::timeout(::oauth::LOGIN_TIMEOUT, back).await {
                // rmcp exchanges the code and saves through `FileStore`.
                Ok(Ok(cb)) => session
                    .handle_callback_url(&cb)
                    .await
                    .map(drop)
                    .map_err(|e| e.to_string()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("timed out waiting for the browser".into()),
            };
            match done {
                Ok(()) => {
                    let _ = me.tell(Restart { name }).await;
                }
                Err(error) => {
                    let _ = me.tell(LoginFailed { name, error }).await;
                }
            }
        });
        Ok(url)
    }
}

/// `tell` from a login task: the browser round trip or the code exchange failed.
struct LoginFailed {
    name: String,
    error: String,
}

impl Message<LoginFailed> for Mcp {
    type Reply = ();

    async fn handle(&mut self, m: LoginFailed, _: &mut Context<Self, Self::Reply>) {
        tracing::warn!(server = %m.name, error = %m.error, "mcp login failed");
        self.state.errors.insert(
            m.name.clone(),
            format!("{}: login failed: {}", m.name, m.error),
        );
        self.rt.publish(progress(&self.state)).await;
    }
}

impl Message<McpLogout> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: McpLogout,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.server)?;
        if let Err(e) = std::fs::remove_file(self.auth_path(&m.server))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("remove tokens: {e}"));
        }
        self.restart(ctx.actor_ref(), &[m.server]).await;
        Ok(())
    }
}

impl Message<AddServer> for Mcp {
    type Reply = Result<McpServer, String>;

    async fn handle(
        &mut self,
        m: AddServer,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<McpServer, String> {
        if !git::valid_name(&m.name) {
            return Err(format!(
                "invalid server name `{}`: use [a-z0-9][a-z0-9._-]*",
                m.name
            ));
        }
        if !is_entry(&m.entry) {
            return Err("entry must be an object with `command` or `url`".into());
        }
        let Some(spec) = entry(&m.entry, &self.dir) else {
            return Err("entry is disabled or malformed".into());
        };
        let path = self.dir.join(format!("{}.json", m.name));
        if path.exists() || self.known(&m.name).is_ok() {
            return Err(format!("mcp server `{}` already exists", m.name));
        }
        if self.state.gates.server_off(&m.name) {
            return Err(format!("`{}` is off in mcp/config.yaml", m.name));
        }
        // Connect before writing: nothing half-added, the caller fixes the
        // entry and calls again. This one wait is the caller's own request.
        let result = attempt(&m.name, &spec, &self.auth_dir)
            .await
            .map_err(|e| format!("{}: connect failed: {e:#}", m.name))?;
        write_json(&path, &m.entry)?;
        settle(&mut self.state, m.name.clone(), spec, Ok(result));
        register(&mut self.state, &self.registry, Vec::new()).await;
        self.rt.publish(progress(&self.state)).await;
        // The watcher's reconcile then finds it running with the same spec.
        Ok(self.server(m.name, None, &HashSet::new()))
    }
}

impl Message<RemoveServer> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveServer,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        // Same order as `desired`, so the file that owns the name is the one edited.
        let hit = json_files(&self.dir).into_iter().find_map(|path| {
            let mut json = read_json(&path)?;
            let edit = remove_entry(&mut json, stem(&path), &m.name)?;
            Some((path, json, edit))
        });
        let Some((path, json, edit)) = hit else {
            return Err(format!("unknown mcp server `{}`", m.name));
        };
        match edit {
            FileEdit::Delete => std::fs::remove_file(&path)
                .map_err(|e| format!("remove {}: {e}", path.display()))?,
            FileEdit::Rewrite => write_json(&path, &json)?,
        }
        self.reconcile(ctx.actor_ref()).await;
        Ok(())
    }
}

/// First 60 lines of `README.md` / `README` (any case) in `dir`; empty if none.
fn readme_head(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| matches!(n.to_ascii_lowercase().as_str(), "readme.md" | "readme"))
        })
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().take(60).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

impl Message<AddRepo> for Mcp {
    type Reply = Result<RepoInfo, String>;

    async fn handle(
        &mut self,
        m: AddRepo,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<RepoInfo, String> {
        let name = m.name.unwrap_or_else(|| git::repo_name(&m.url));
        if !git::valid_name(&name) {
            return Err(format!(
                "invalid repo name `{name}`: use [a-z0-9][a-z0-9._-]*"
            ));
        }
        let repos = self.dir.join("repos");
        let path = repos.join(&name);
        if path.exists() {
            return Err(format!("{} already exists", path.display()));
        }
        std::fs::create_dir_all(&repos).map_err(|e| format!("mkdir {}: {e}", repos.display()))?;
        // ponytail: the clone runs inside the actor, so `/mcp` waits for it;
        // move it to a task if a slow network ever makes that felt.
        git::clone(&m.url, &path)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(RepoInfo {
            name,
            readme: readme_head(&path),
            path: path.display().to_string(),
        })
    }
}

impl Message<SyncMcp> for Mcp {
    type Reply = Vec<git::SyncResult>;

    async fn handle(
        &mut self,
        _: SyncMcp,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Vec<git::SyncResult> {
        let repos = self.dir.join("repos");
        let results = git::sync_all(&repos).await;
        let updated: Vec<PathBuf> = results
            .iter()
            .filter(|r| r.status == "updated")
            .map(|r| repos.join(&r.name))
            .collect();
        let stale: Vec<String> = self
            .state
            .running
            .iter()
            .filter(|(_, r)| {
                r.spec
                    .cwd()
                    .is_some_and(|c| updated.iter().any(|u| c.starts_with(u)))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if !stale.is_empty() {
            self.restart(ctx.actor_ref(), &stale).await;
        }
        results
    }
}

impl Mcp {
    async fn reconcile(&mut self, me: &ActorRef<Mcp>) {
        reconcile(
            &mut self.state,
            &self.dir,
            &self.auth_dir,
            &self.registry,
            &self.rt,
            me,
        )
        .await;
    }

    /// Drop `names` (ending their processes) and unregister their tools, then
    /// reconcile: every dropped server the files still want reconnects.
    async fn restart(&mut self, me: &ActorRef<Mcp>, names: &[String]) {
        for name in names {
            tracing::info!(server = %name, "mcp server restarting");
            if let Some(r) = self.state.running.remove(name) {
                for tool in r.tools.into_values() {
                    let _ = self.registry.ask(UnregisterTool(tool)).await;
                }
            }
        }
        self.reconcile(me).await;
    }
}

/// `tell` from a connect task: one server's attempt finished.
struct Settled {
    name: String,
    spec: ServerSpec,
    result: Attempt,
}

type Attempt = anyhow::Result<(Service, Vec<rmcp::model::Tool>)>;

impl Message<Settled> for Mcp {
    type Reply = ();

    async fn handle(&mut self, m: Settled, ctx: &mut Context<Self, Self::Reply>) {
        self.state.pending.remove(&m.name);
        let wanted = desired(&self.dir).get(&m.name) == Some(&m.spec)
            && !self.state.gates.server_off(&m.name);
        if !wanted {
            // The entry changed or left while connecting: `result` is dropped
            // (ending the process) and reconcile starts over if needed.
            self.reconcile(ctx.actor_ref()).await;
            return;
        }
        let connected = m.result.is_ok();
        settle(&mut self.state, m.name, m.spec, m.result);
        if connected {
            register(&mut self.state, &self.registry, Vec::new()).await;
        }
        self.rt.publish(progress(&self.state)).await;
    }
}

/// `ask` → [`McpProgress`]: the current snapshot, for a UI that attached
/// after the first ones were published.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Progress;

impl Message<Progress> for Mcp {
    type Reply = McpProgress;

    async fn handle(&mut self, _: Progress, _: &mut Context<Self, Self::Reply>) -> McpProgress {
        progress(&self.state)
    }
}

/// Expand `$VAR`, `${VAR:-x}`, `$(cmd)` and backticks in one entry string with
/// the user's shell (`$SHELL`, else `/bin/sh`), so a file can say
/// `"GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)"` or `"${GRAFANA_TOKEN}"`.
/// The string is fed as an unquoted heredoc: quotes stay literal, `\$` is a
/// literal dollar, and the shell's own parser handles nesting. A `$`-free
/// string is returned as is; a failed substitution is a warning and empty
/// (POSIX), the server's own auth error names the consequence.
async fn expand(s: &str) -> anyhow::Result<String> {
    if !s.contains('$') && !s.contains('`') {
        return Ok(s.to_owned());
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let out = tokio::process::Command::new(shell)
        .arg("-c")
        .arg(format!("cat <<__wth_mcp__\n{s}\n__wth_mcp__"))
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !err.trim().is_empty() {
        tracing::warn!(value = %s, status = %out.status, "mcp expansion: {}", err.trim());
    }
    let mut text = String::from_utf8(out.stdout)?;
    text.pop(); // the heredoc's own trailing newline
    Ok(text)
}

async fn expand_map(m: &BTreeMap<String, String>) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (k, v) in m {
        out.insert(k.clone(), expand(v).await?);
    }
    Ok(out)
}

/// `auth_dir` holds the server's OAuth tokens, `<auth_dir>/<name>.json`.
/// Strings from the file go through [`expand`] here, at connect, so a
/// `$(gh auth token)` runs once per connection and not on every reconcile.
async fn connect(name: &str, spec: &ServerSpec, auth_dir: &Path) -> anyhow::Result<Service> {
    let service = match &spec.transport {
        Transport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let mut cmd = tokio::process::Command::new(expand(command).await?);
            for a in args {
                cmd.arg(expand(a).await?);
            }
            cmd.envs(expand_map(env).await?);
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
            // The child must not touch our terminal: an inherited tty on stderr
            // leaks its banners into the TUI and node resets termios when it dies.
            let (transport, stderr) = TokioChildProcess::builder(cmd)
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            if let Some(stderr) = stderr {
                let server = name.to_owned();
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::info!(server = %server, "mcp stderr: {line}");
                    }
                });
            }
            ().serve(transport).await?
        }
        Transport::Http { url, headers, .. } => {
            let url = expand(url).await?;
            let headers = &expand_map(headers).await?;
            let mut custom = HashMap::new();
            for (k, v) in headers {
                custom.insert(k.parse()?, v.parse()?);
            }
            let cfg =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(custom);
            if headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("authorization"))
            {
                // The user does auth by hand; leave them alone.
                ().serve(StreamableHttpClientTransport::from_config(cfg))
                    .await?
            } else {
                let http = reqwest::Client::new();
                let mut manager = AuthorizationManager::new(url.as_str()).await?;
                manager.with_client(http.clone())?;
                manager.set_credential_store(oauth::FileStore::new(
                    auth_dir.join(format!("{name}.json")),
                ));
                // Discovery only when tokens exist; without them the request
                // goes out plain and a 401 lands in `settle` as `login required`.
                manager.initialize_from_store().await?;
                ().serve(StreamableHttpClientTransport::with_client(
                    AuthClient::new(http, manager),
                    cfg,
                ))
                .await?
            }
        }
    };
    Ok(Arc::new(service))
}

/// The `*.json` files directly in `dir`, sorted by path. Non-recursive on
/// purpose: `repos/` (git clones, full of package manifests) is never scanned,
/// and `config.yaml` / `*.json.tmp` fall out of the extension test.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

/// Parse one file; unreadable or invalid → warn + `None`.
fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|t| Ok(serde_json::from_str(&t)?))
        .map_err(|e| tracing::warn!(file = %path.display(), error = %e, "mcp file skipped"))
        .ok()
}

fn stem(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
}

/// Every server the files want; first server name wins.
fn desired(dir: &Path) -> BTreeMap<String, ServerSpec> {
    let mut out = BTreeMap::new();
    let mut owner: HashMap<String, PathBuf> = HashMap::new();
    for path in json_files(dir) {
        let Some(json) = read_json(&path) else {
            continue;
        };
        for (name, spec) in parse_file(stem(&path), &json, dir) {
            if let Some(first) = owner.get(&name) {
                tracing::warn!(
                    server = %name, kept = %first.display(), ignored = %path.display(),
                    "duplicate mcp server name"
                );
                continue;
            }
            owner.insert(name.clone(), path.clone());
            out.insert(name, spec);
        }
    }
    out
}

/// Progress snapshot of `state`.
fn progress(state: &State) -> McpProgress {
    let sorted = |it: Vec<String>| {
        let mut v = it;
        v.sort();
        v
    };
    McpProgress {
        connecting: sorted(state.pending.iter().cloned().collect()),
        connected: sorted(state.running.keys().cloned().collect()),
        failed: sorted(state.errors.keys().cloned().collect()),
    }
}

/// Connect and list tools, bounded by [`CONNECT_TIMEOUT`].
async fn attempt(name: &str, spec: &ServerSpec, auth_dir: &Path) -> Attempt {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let service = connect(name, spec, auth_dir).await?;
        let remote = service.list_all_tools().await?;
        anyhow::Ok((service, remote))
    })
    .await
    .map_err(|_| anyhow::anyhow!("no response within {CONNECT_TIMEOUT:?}"))
    .and_then(|r| r)
}

/// Record a settled attempt for a server the files still want.
fn settle(state: &mut State, name: String, spec: ServerSpec, result: Attempt) {
    match result {
        Ok((service, remote)) => {
            tracing::info!(server = %name, count = remote.len(), "mcp server connected");
            state.errors.remove(&name);
            state.login.remove(&name);
            state.running.insert(
                name,
                Running {
                    spec,
                    service,
                    remote,
                    tools: BTreeMap::new(),
                },
            );
        }
        Err(e) => {
            let challenge = e
                .downcast_ref::<ClientInitializeError>()
                .and_then(ClientInitializeError::auth_challenge);
            if let Some(challenge) = challenge {
                tracing::info!(server = %name, "mcp server needs login");
                state.login.insert(name.clone(), challenge.to_owned());
                state.errors.insert(
                    name.clone(),
                    format!("{name}: login required (/login {name})"),
                );
            } else {
                tracing::warn!(server = %name, error = %e, "mcp connect failed");
                state
                    .errors
                    .insert(name.clone(), format!("{name}: connect failed: {e:#}"));
            }
        }
    }
}

/// Bring `state` in line with the files in `dir`: drop servers whose entry
/// changed or left, and start a connect task for every wanted server not
/// running or already connecting. Tasks report back as [`Settled`], so the
/// actor never waits on a server. Publishes an [`McpProgress`] when anything
/// moved.
async fn reconcile(
    state: &mut State,
    dir: &Path,
    auth_dir: &Path,
    registry: &ActorRef<ToolRegistry>,
    rt: &Runtime,
    me: &ActorRef<Mcp>,
) {
    let want = desired(dir);
    let gates = load_gates(&dir.join("config.yaml"));
    let gates_changed = gates != state.gates;
    state.gates = gates;

    let mut changed = gates_changed;
    let stale: Vec<String> = state
        .running
        .iter()
        .filter(|(name, r)| want.get(*name) != Some(&r.spec) || state.gates.server_off(name))
        .map(|(name, _)| name.clone())
        .collect();
    let mut orphans: Vec<String> = Vec::new();
    for name in stale {
        changed = true;
        tracing::info!(server = %name, "mcp server dropped");
        // Removing drops `service`, which ends the child / session.
        if let Some(r) = state.running.remove(&name) {
            orphans.extend(r.tools.into_values());
        }
    }
    state.errors.retain(|name, _| want.contains_key(name));
    state.login.retain(|name, _| want.contains_key(name));
    for (name, spec) in want {
        if state.running.contains_key(&name)
            || state.pending.contains(&name)
            || state.gates.server_off(&name)
        {
            continue;
        }
        changed = true;
        state.pending.insert(name.clone());
        let me = me.clone();
        let auth_dir = auth_dir.to_owned();
        tokio::spawn(async move {
            let result = attempt(&name, &spec, &auth_dir).await;
            let _ = me.tell(Settled { name, spec, result }).await;
        });
    }
    if changed {
        register(state, registry, orphans).await;
        rt.publish(progress(state)).await;
    }
}

/// Effective risk of remote tool `t` on `server`: its gate line (tool → server)
/// when that names a risk, else its annotations.
fn risk_of(gates: &Gates, server: &str, t: &rmcp::model::Tool) -> Risk {
    gates
        .for_tool(server, &t.name)
        .and_then(Gate::risk)
        .unwrap_or_else(|| annotated(t))
}

/// Recompute every registration, then diff against what is registered so
/// nothing flickers. A tool name offered by more than one running server, or
/// by a native, registers as `<server>__<name>` for every server offering
/// it, so each server keeps its own slot (01 §1.4). `orphans` are names
/// registered by servers that have since been dropped.
async fn register(state: &mut State, registry: &ActorRef<ToolRegistry>, mut orphans: Vec<String>) {
    let offered = |server: &str, t: &rmcp::model::Tool| {
        state.gates.for_tool(server, &t.name) != Some(Gate::Off)
    };
    let mut count: HashMap<&str, usize> = HashMap::new();
    for (server, r) in &state.running {
        for t in r.remote.iter().filter(|t| offered(server, t)) {
            *count.entry(&t.name).or_default() += 1;
        }
    }
    let mut taken: HashMap<String, String> = HashMap::new();
    let mut regs: Vec<(String, McpTool)> = Vec::new();
    let mut names: Vec<&String> = state.running.keys().collect();
    names.sort();
    for server in names {
        let r = &state.running[server];
        for t in r.remote.iter().filter(|t| offered(server, t)) {
            let remote = t.name.to_string();
            // ponytail: the prefix follows the *running* set, so a session hide on
            // `a__x` is lost when `b` goes off and `x` re-registers bare; key hides
            // by (server, remote name) if that bites.
            let name = if state.natives.contains(&remote) || count[remote.as_str()] > 1 {
                format!("{server}__{remote}")
            } else {
                remote.clone()
            };
            if let Some(owner) = taken.get(&name) {
                tracing::warn!("mcp tool {name} from {server} hidden: clashes with {owner}");
                continue;
            }
            taken.insert(name.clone(), server.clone());
            regs.push((
                server.clone(),
                McpTool {
                    spec: ToolSpec {
                        name,
                        description: t.description.as_deref().unwrap_or_default().to_owned(),
                        schema: Value::Object((*t.input_schema).clone()),
                        risk: risk_of(&state.gates, server, t),
                        direct: false,
                        returns: "Any".to_owned(),
                    },
                    remote_name: remote,
                    service: Arc::clone(&r.service),
                },
            ));
        }
    }
    orphans.extend(
        state
            .running
            .values_mut()
            .flat_map(|r| std::mem::take(&mut r.tools).into_values()),
    );
    for name in orphans {
        if !taken.contains_key(&name) {
            let _ = registry.ask(UnregisterTool(name)).await;
        }
    }
    for (server, tool) in regs {
        let (remote, name) = (tool.remote_name.clone(), tool.spec.name.clone());
        let _ = registry.ask(RegisterTool(Arc::new(tool))).await;
        if let Some(r) = state.running.get_mut(&server) {
            r.tools.insert(remote, name);
        }
    }
}

/// Register the MCP source tools (natives, so they win clashes), spawn the
/// [`Mcp`] actor, kick off the first reconcile of `<data_dir>/mcp` without
/// waiting for it (servers connect in the background and report through
/// [`McpProgress`]), then watch the directory. `policy` is `tools.policy`,
/// for [`McpToolInfo::action`].
pub async fn start(
    rt: &Runtime,
    registry: ActorRef<ToolRegistry>,
    policy: Policy,
) -> anyhow::Result<()> {
    let dir = rt.data_dir.join("mcp");
    std::fs::create_dir_all(&dir)?;
    let sources: [DynTool; 4] = [
        Arc::new(tools::AddMcpServer),
        Arc::new(tools::RemoveMcpServer),
        Arc::new(tools::AddMcpRepo),
        Arc::new(tools::SyncMcpRepos),
    ];
    for tool in sources {
        registry.ask(RegisterTool(tool)).await?;
    }
    let natives = registry
        .ask(ListTools(None))
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    let mcp = Mcp::spawn_with_mailbox(
        Mcp {
            state: State {
                natives,
                ..State::default()
            },
            dir: dir.clone(),
            auth_dir: ::oauth::Store::new(&rt.data_dir).mcp_dir(),
            registry,
            policy,
            rt: rt.clone(),
        },
        runtime::subscriber_mailbox(),
    );
    mcp.register(MCP_ACTOR)?;
    mcp.tell(Reconcile).await?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
    tokio::spawn(async move {
        let _watcher = watcher;
        while rx.recv().await.is_some() {
            // ponytail: fixed 300ms debounce; editors save in bursts. Tune if a
            // reconnect storm ever shows up.
            tokio::time::sleep(Duration::from_millis(300)).await;
            while rx.try_recv().is_ok() {}
            if mcp.tell(Reconcile).await.is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/data/mcp";

    fn parse_file(stem: &str, json: &Value) -> Vec<(String, ServerSpec)> {
        super::parse_file(stem, json, Path::new(DIR))
    }

    fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> ServerSpec {
        ServerSpec {
            transport: Transport::Stdio {
                command: command.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                cwd: None,
            },
        }
    }

    #[tokio::test]
    async fn expand_uses_the_shell_and_keeps_quotes_literal() {
        // SAFETY: test-only unique key, set before any thread reads it.
        unsafe { std::env::set_var("WTH_MCP_T", "tok") };
        assert_eq!(
            expand("Bearer ${WTH_MCP_T}-$(printf hi)-\"q\" \\$lit")
                .await
                .unwrap(),
            "Bearer tok-hi-\"q\" $lit"
        );
        assert_eq!(expand("plain \"text\" ").await.unwrap(), "plain \"text\" ");
        assert_eq!(expand("$(false)x").await.unwrap(), "x");
    }

    #[test]
    fn cwd_relative_joins_mcp_dir_absolute_kept() {
        let cwd = |c: &str| {
            parse_file("s", &serde_json::json!({"command": "node", "cwd": c}))
                .remove(0)
                .1
                .cwd()
                .map(Path::to_path_buf)
        };
        assert_eq!(
            cwd("repos/my-server"),
            Some(PathBuf::from("/data/mcp/repos/my-server"))
        );
        assert_eq!(cwd("/opt/srv"), Some(PathBuf::from("/opt/srv")));
        assert_eq!(stdio("x", &[], &[]).cwd(), None);
    }

    #[test]
    fn repo_detected_from_cwd_inside_a_clone() {
        let dir = std::env::temp_dir().join(format!("wth-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("repos/srv/.git")).unwrap();
        std::fs::create_dir_all(dir.join("repos/plain")).unwrap();
        let spec = |cwd: &str| {
            super::parse_file(
                "s",
                &serde_json::json!({"command": "node", "cwd": cwd}),
                &dir,
            )
            .remove(0)
            .1
        };
        assert_eq!(
            repo_of(&dir, &spec("repos/srv/dist")).as_deref(),
            Some("srv")
        );
        assert_eq!(repo_of(&dir, &spec("repos/plain")), None, "no .git");
        assert_eq!(repo_of(&dir, &spec("/elsewhere")), None);
        assert_eq!(repo_of(&dir, &stdio("x", &[], &[])), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_entry_disables_in_multi_server_file_deletes_bare_file() {
        let original = serde_json::json!({"mcpServers": {
            "a": {"command": "x", "args": ["1"]},
            "b": {"command": "y", "env": {"K": "v"}},
            "c": {"url": "https://c.example/mcp"}
        }});
        let mut json = original.clone();
        assert_eq!(
            remove_entry(&mut json, "file", "b"),
            Some(FileEdit::Rewrite)
        );
        assert_eq!(json["mcpServers"]["b"]["disabled"], Value::Bool(true));
        assert_eq!(json["mcpServers"]["b"]["command"], "y");
        assert_eq!(json["mcpServers"]["a"], original["mcpServers"]["a"]);
        assert_eq!(json["mcpServers"]["c"], original["mcpServers"]["c"]);
        // Reserialised, the loader now skips b and still sees a and c.
        let back: Value =
            serde_json::from_slice(&serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let names: Vec<String> = parse_file("file", &back)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(remove_entry(&mut json, "file", "zzz"), None);

        // Top-level entries (no wrapper) work too.
        let mut flat = serde_json::json!({"a": {"command": "x"}, "b": {"command": "y"}});
        assert_eq!(remove_entry(&mut flat, "f", "a"), Some(FileEdit::Rewrite));
        assert_eq!(flat["a"]["disabled"], Value::Bool(true));
        assert!(flat["b"].get("disabled").is_none());

        let mut bare = serde_json::json!({"command": "npx", "args": ["-y", "x"]});
        assert_eq!(
            remove_entry(&mut bare, "everything", "everything"),
            Some(FileEdit::Delete)
        );
        assert_eq!(remove_entry(&mut bare, "everything", "other"), None);
    }

    #[test]
    fn claude_desktop_mcp_servers() {
        let json = serde_json::json!({"mcpServers": {
            "everything": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-everything"]},
            "fs": {"command": "uvx", "args": ["mcp-fs"], "env": {"ROOT": "/tmp"}}
        }});
        assert_eq!(
            parse_file("a", &json),
            vec![
                (
                    "everything".into(),
                    stdio(
                        "npx",
                        &["-y", "@modelcontextprotocol/server-everything"],
                        &[]
                    )
                ),
                ("fs".into(), stdio("uvx", &["mcp-fs"], &[("ROOT", "/tmp")])),
            ]
        );
    }

    #[test]
    fn opencode_array_command_and_environment() {
        let json = serde_json::json!({"mcp": {"everything": {
            "type": "local",
            "command": ["npx", "-y", "@modelcontextprotocol/server-everything"],
            "environment": {"A": "1"}
        }}});
        assert_eq!(
            parse_file("o", &json),
            vec![(
                "everything".into(),
                stdio(
                    "npx",
                    &["-y", "@modelcontextprotocol/server-everything"],
                    &[("A", "1")]
                )
            )]
        );
    }

    #[test]
    fn vscode_servers_with_type_stdio() {
        let json = serde_json::json!({"servers": {"everything": {
            "type": "stdio", "command": "npx", "args": ["-y", "x"]
        }}});
        assert_eq!(
            parse_file("v", &json),
            vec![("everything".into(), stdio("npx", &["-y", "x"], &[]))]
        );
    }

    #[test]
    fn bare_entry_named_by_stem() {
        let json = serde_json::json!({"command": "npx", "args": ["-y", "x"]});
        assert_eq!(
            parse_file("everything", &json),
            vec![("everything".into(), stdio("npx", &["-y", "x"], &[]))]
        );
    }

    #[test]
    fn remote_url_with_headers_and_oauth() {
        let json = serde_json::json!({"mcpServers": {
            "api": {"type": "sse", "url": "https://x.example/mcp", "headers": {"Authorization": "Bearer t"}},
            "slack": {"url": "https://s.example/mcp", "oauth": {"client_id": "id", "scopes": ["a"], "redirect_uri": "http://localhost:8765/cb"}},
            "bad": {"url": "https://b.example/mcp", "oauth": {"client_secret": "no id"}}
        }});
        let http = |url: &str, headers: &[(&str, &str)], oauth| ServerSpec {
            transport: Transport::Http {
                url: url.into(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                oauth,
            },
        };
        // Key order depends on whether `serde_json/preserve_order` is unified in.
        let mut got = parse_file("r", &json);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                (
                    "api".into(),
                    http(
                        "https://x.example/mcp",
                        &[("Authorization", "Bearer t")],
                        None
                    )
                ),
                (
                    "bad".into(),
                    http(
                        "https://b.example/mcp",
                        &[],
                        Some(OauthClient {
                            client_secret: Some("no id".into()),
                            ..OauthClient::default()
                        })
                    )
                ),
                (
                    "slack".into(),
                    http(
                        "https://s.example/mcp",
                        &[],
                        Some(OauthClient {
                            client_id: Some("id".into()),
                            client_secret: None,
                            scopes: vec!["a".into()],
                            redirect_uri: Some("http://localhost:8765/cb".into()),
                        })
                    )
                ),
            ]
        );
    }

    #[test]
    fn oauth_redirect_port_comes_from_the_uri() {
        let c = |uri: &str| OauthClient {
            redirect_uri: Some(uri.into()),
            ..OauthClient::default()
        };
        assert_eq!(OauthClient::default().redirect(), Ok((0, None)));
        assert_eq!(
            c("http://localhost:8765/callback").redirect(),
            Ok((8765, Some("http://localhost:8765/callback".into())))
        );
        assert_eq!(c("http://127.0.0.1/cb").redirect().map(|(p, _)| p), Ok(80));
        assert!(c("not a url").redirect().is_err());
    }

    #[test]
    fn disabled_entries_skipped() {
        let json = serde_json::json!({"mcpServers": {
            "a": {"command": "x", "enabled": false},
            "b": {"command": "y", "disabled": true},
            "c": {"command": "z"}
        }});
        assert_eq!(
            parse_file("d", &json),
            vec![("c".into(), stdio("z", &[], &[]))]
        );
    }

    #[test]
    fn gates_resolution() {
        let g: Gates = serde_yaml::from_str(
            "servers:\n  e:\n    risk: read_only\n    tools:\n      echo: destructive\n      slow: off\n  f:\n    risk: off\n",
        )
        .unwrap();
        assert_eq!(g.for_tool("e", "echo"), Some(Gate::Destructive));
        assert_eq!(g.for_tool("e", "slow"), Some(Gate::Off));
        assert_eq!(g.for_tool("e", "other"), Some(Gate::ReadOnly));
        assert!(g.server_off("f"));
        assert_eq!(g.for_tool("unknown", "x"), None);
        assert_eq!(serde_yaml::from_str::<Option<Gates>>("").unwrap(), None);
        // Old allow|ask|deny files fail to parse (12 §5: no migration).
        assert!(serde_yaml::from_str::<Gates>("servers:\n  e:\n    gate: allow\n").is_err());
        assert!(serde_yaml::from_str::<Gates>("servers:\n  e:\n    risk: allow\n").is_err());
    }

    #[test]
    fn risk_from_annotations_then_gates() {
        use rmcp::model::{Tool, ToolAnnotations};
        let tool = |a: Option<ToolAnnotations>| {
            let mut t = Tool::new("t", "", serde_json::Map::new());
            t.annotations = a;
            t
        };
        let g = Gates::default();
        assert_eq!(risk_of(&g, "s", &tool(None)), Risk::Mutating);
        assert_eq!(
            risk_of(&g, "s", &tool(Some(ToolAnnotations::new().read_only(true)))),
            Risk::ReadOnly
        );
        assert_eq!(
            risk_of(
                &g,
                "s",
                &tool(Some(ToolAnnotations::new().destructive(true)))
            ),
            Risk::Destructive
        );
        // Unset destructive_hint is *not* destructive here (12 §5).
        assert_eq!(
            risk_of(&g, "s", &tool(Some(ToolAnnotations::new()))),
            Risk::Mutating
        );
        let mut g = Gates::default();
        g.set("s", None, Some(Gate::ReadOnly));
        assert_eq!(
            risk_of(
                &g,
                "s",
                &tool(Some(ToolAnnotations::new().destructive(true)))
            ),
            Risk::ReadOnly,
            "server line beats annotations"
        );
        g.set("s", Some("t"), Some(Gate::Off));
        assert_eq!(
            risk_of(&g, "s", &tool(None)),
            Risk::Mutating,
            "off falls back to annotations for display"
        );
    }

    #[test]
    fn gates_set_roundtrip_drops_empty() {
        let mut g = Gates::default();
        g.set("e", Some("echo"), Some(Gate::Off));
        g.set("e", None, Some(Gate::ReadOnly));
        g.set("f", None, Some(Gate::Off));
        let back: Gates = serde_yaml::from_str(&serde_yaml::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.own("e", "echo"), Some(Gate::Off));
        assert_eq!(back.for_tool("e", "other"), Some(Gate::ReadOnly));

        g.set("e", None, None);
        assert_eq!(
            g.for_tool("e", "echo"),
            Some(Gate::Off),
            "tool override survives"
        );
        g.set("e", Some("echo"), None);
        g.set("f", None, None);
        assert!(g.servers.is_empty(), "emptied entries dropped: {g:?}");
        assert_eq!(serde_yaml::to_string(&g).unwrap(), "servers: {}\n");
    }
}
