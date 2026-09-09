//! MCP client: every `*.json` in `<data_dir>/mcp/` holds one or more servers in
//! whatever layout its README used; each remote tool registers under its own
//! name (no prefix). The directory is watched and reconciled on change;
//! `<data_dir>/mcp/config.yaml` gates tools per server (design doc 01 §1.4).
//! `<data_dir>/mcp/repos/` holds git clones of server sources; an entry's
//! `cwd` resolves against the mcp dir (design doc 04 §3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use notify::Watcher;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::{ListTools, RegisterTool, ToolRegistry, UnregisterTool};
use crate::{
    DynTool, ErasedTool, MCP_ACTOR, PolicyAction, Risk, ToolCx, ToolOutput, ToolSpec, git,
};

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
    },
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
        return Some(ServerSpec {
            transport: Transport::Http {
                url: url.to_owned(),
                headers: string_map(v.get("headers")),
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

/// `<data_dir>/mcp/config.yaml`: per-server and per-tool approval actions.
/// Machine-written by [`SetGate`] (comments are not preserved); `BTreeMap`
/// keeps the output stable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Gates {
    servers: BTreeMap<String, ServerGate>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ServerGate {
    #[serde(skip_serializing_if = "Option::is_none")]
    gate: Option<PolicyAction>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tools: BTreeMap<String, PolicyAction>,
}

impl Gates {
    fn for_server(&self, server: &str) -> PolicyAction {
        self.servers
            .get(server)
            .and_then(|g| g.gate)
            .unwrap_or(PolicyAction::Allow)
    }

    /// tool line → server line → allow.
    fn for_tool(&self, server: &str, tool: &str) -> PolicyAction {
        self.servers
            .get(server)
            .and_then(|g| g.tools.get(tool).copied().or(g.gate))
            .unwrap_or(PolicyAction::Allow)
    }

    /// The tool's own line, ignoring inheritance.
    fn own(&self, server: &str, tool: &str) -> Option<PolicyAction> {
        self.servers.get(server)?.tools.get(tool).copied()
    }

    /// Set or clear (`None`) a server or tool line; an emptied server entry is dropped.
    fn set(&mut self, server: &str, tool: Option<&str>, action: Option<PolicyAction>) {
        let g = self.servers.entry(server.to_owned()).or_default();
        match (tool, action) {
            (None, a) => g.gate = a,
            (Some(t), Some(a)) => {
                g.tools.insert(t.to_owned(), a);
            }
            (Some(t), None) => {
                g.tools.remove(t);
            }
        }
        if g.gate.is_none() && g.tools.is_empty() {
            self.servers.remove(server);
        }
    }
}

/// Missing file = all allow; unreadable/unparsable = warn + all allow.
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
            tracing::warn!(path = %path.display(), error = %e, "mcp gates invalid");
            Gates::default()
        }
    }
}

type Service = Arc<RunningService<RoleClient, ()>>;

/// A remote tool. Holds the connection alive through `service`.
struct McpTool {
    spec: ToolSpec,
    remote_name: String,
    service: Service,
    gate: PolicyAction,
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
            })
        })
    }

    fn gate(&self) -> Option<PolicyAction> {
        Some(self.gate)
    }
}

/// A connected server. Dropping `service` ends the child process / session.
struct Running {
    spec: ServerSpec,
    service: Service,
    /// Remote tool list, kept so a gates change can re-register without a round trip.
    remote: Vec<rmcp::model::Tool>,
    /// Names currently registered from this server.
    tools: Vec<String>,
}

#[derive(Default)]
struct State {
    running: HashMap<String, Running>,
    gates: Gates,
    /// Native tool names; always win a clash.
    natives: HashSet<String>,
    /// Last connect / list_tools failure per desired server; cleared on success
    /// or when the server leaves the files.
    errors: HashMap<String, String>,
}

/// The MCP actor. Look up with `ActorRef::<Mcp>::lookup(MCP_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Mcp {
    state: State,
    dir: PathBuf,
    registry: ActorRef<ToolRegistry>,
}

/// `tell` from the watcher: re-read `<data_dir>/mcp` and apply.
struct Reconcile;

/// `ask` → `Vec<McpServer>`: every server the files want, sorted by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ListServers;

/// `ask` → `Result<(), String>`: set (or clear with `action: None`) a server
/// or tool gate, rewrite `config.yaml`, reconcile now.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SetGate {
    /// Server name (file stem or entry key).
    pub server: String,
    /// `None` = the server line.
    pub tool: Option<String>,
    /// `None` removes the line.
    pub action: Option<PolicyAction>,
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

/// One server as the files want it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpServer {
    /// Server name.
    pub name: String,
    /// False when deny-gated or connect failed; then `tools` is empty.
    pub connected: bool,
    /// Server-level gate (default allow).
    pub gate: PolicyAction,
    /// Remote tools, sorted by name.
    pub tools: Vec<McpToolInfo>,
    /// Name of the clone under `mcp/repos/` that holds the entry's `cwd`, if any.
    pub repo: Option<String>,
}

/// One remote tool of a connected server.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpToolInfo {
    /// Remote tool name (also the registered name).
    pub name: String,
    /// Remote description.
    pub description: String,
    /// The tool's own override, if any.
    pub gate: Option<PolicyAction>,
    /// What the gate does after inheritance.
    pub effective: PolicyAction,
    /// False when denied or hidden by a name clash.
    pub registered: bool,
}

impl Message<Reconcile> for Mcp {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, _: &mut Context<Self, Self::Reply>) {
        reconcile(&mut self.state, &self.dir, &self.registry).await;
    }
}

impl Mcp {
    /// One [`McpServer`] from the current state; `spec` is the file's entry
    /// when the server is not running.
    fn server(&self, name: String, spec: Option<&ServerSpec>) -> McpServer {
        let state = &self.state;
        let running = state.running.get(&name);
        let mut tools: Vec<McpToolInfo> = running
            .map(|r| {
                r.remote
                    .iter()
                    .map(|t| {
                        let tool = t.name.to_string();
                        McpToolInfo {
                            gate: state.gates.own(&name, &tool),
                            effective: state.gates.for_tool(&name, &tool),
                            registered: r.tools.contains(&tool),
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
            repo: running
                .map(|r| &r.spec)
                .or(spec)
                .and_then(|s| repo_of(&self.dir, s)),
            name,
        }
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
        _: ListServers,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<McpServer> {
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
                self.server(name, spec)
            })
            .collect()
    }
}

impl Message<SetGate> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetGate,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.server)?;
        // Edit a copy and let `reconcile` load it from the file: it diffs the
        // file against `state.gates` to decide whether to re-register.
        let mut gates = self.state.gates.clone();
        gates.set(&m.server, m.tool.as_deref(), m.action);
        let text = serde_yaml::to_string(&gates).map_err(|e| e.to_string())?;
        // Write-then-rename so the watcher never reads a half file.
        let path = self.dir.join("config.yaml");
        let tmp = self.dir.join("config.yaml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        reconcile(&mut self.state, &self.dir, &self.registry).await;
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
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.name)?;
        restart(&mut self.state, &self.dir, &self.registry, &[m.name]).await;
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
        if entry(&m.entry, &self.dir).is_none() {
            return Err("entry is disabled or malformed".into());
        }
        let path = self.dir.join(format!("{}.json", m.name));
        if path.exists() || self.known(&m.name).is_ok() {
            return Err(format!("mcp server `{}` already exists", m.name));
        }
        write_json(&path, &m.entry)?;
        reconcile(&mut self.state, &self.dir, &self.registry).await;
        if self.state.running.contains_key(&m.name) {
            return Ok(self.server(m.name, None));
        }
        let err = self
            .state
            .errors
            .remove(&m.name)
            .unwrap_or_else(|| format!("`{}` is denied by mcp/config.yaml", m.name));
        // Nothing half-added: the caller fixes the entry and calls again.
        let _ = std::fs::remove_file(&path);
        Err(err)
    }
}

impl Message<RemoveServer> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveServer,
        _: &mut Context<Self, Self::Reply>,
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
        reconcile(&mut self.state, &self.dir, &self.registry).await;
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
        _: &mut Context<Self, Self::Reply>,
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
            restart(&mut self.state, &self.dir, &self.registry, &stale).await;
        }
        results
    }
}

/// Drop `names` (ending their processes) and unregister their tools, then
/// reconcile: every dropped server the files still want reconnects.
async fn restart(
    state: &mut State,
    dir: &Path,
    registry: &ActorRef<ToolRegistry>,
    names: &[String],
) {
    for name in names {
        tracing::info!(server = %name, "mcp server restarting");
        if let Some(r) = state.running.remove(name) {
            for tool in r.tools {
                let _ = registry.ask(UnregisterTool(tool)).await;
            }
        }
    }
    reconcile(state, dir, registry).await;
}

async fn connect(name: &str, spec: &ServerSpec) -> anyhow::Result<Service> {
    let service = match &spec.transport {
        Transport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
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
        Transport::Http { url, headers } => {
            let mut custom = HashMap::new();
            for (k, v) in headers {
                custom.insert(k.parse()?, v.parse()?);
            }
            let cfg =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(custom);
            ().serve(StreamableHttpClientTransport::from_config(cfg))
                .await?
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

/// Bring `state.running` in line with the files in `dir`.
async fn reconcile(state: &mut State, dir: &Path, registry: &ActorRef<ToolRegistry>) {
    let want = desired(dir);
    let gates = load_gates(&dir.join("config.yaml"));
    let gates_changed = gates != state.gates;
    state.gates = gates;

    let mut changed = gates_changed;
    let stale: Vec<String> = state
        .running
        .iter()
        .filter(|(name, r)| {
            want.get(*name) != Some(&r.spec) || state.gates.for_server(name) == PolicyAction::Deny
        })
        .map(|(name, _)| name.clone())
        .collect();
    let mut orphans: Vec<String> = Vec::new();
    for name in stale {
        changed = true;
        tracing::info!(server = %name, "mcp server dropped");
        // Removing drops `service`, which ends the child / session.
        if let Some(r) = state.running.remove(&name) {
            orphans.extend(r.tools);
        }
    }
    state.errors.retain(|name, _| want.contains_key(name));
    for (name, spec) in want {
        if state.running.contains_key(&name) || state.gates.for_server(&name) == PolicyAction::Deny
        {
            continue;
        }
        let attempt = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let service = connect(&name, &spec).await?;
            let remote = service.list_all_tools().await?;
            anyhow::Ok((service, remote))
        })
        .await
        .map_err(|_| anyhow::anyhow!("no response within {CONNECT_TIMEOUT:?}"))
        .and_then(|r| r);
        match attempt {
            Ok((service, remote)) => {
                tracing::info!(server = %name, count = remote.len(), "mcp server connected");
                changed = true;
                state.errors.remove(&name);
                state.running.insert(
                    name,
                    Running {
                        spec,
                        service,
                        remote,
                        tools: Vec::new(),
                    },
                );
            }
            Err(e) => {
                tracing::warn!(server = %name, error = %e, "mcp connect failed");
                state
                    .errors
                    .insert(name.clone(), format!("{name}: connect failed: {e:#}"));
            }
        }
    }
    if !changed {
        return;
    }

    // Recompute every registration in sorted server order (first name wins),
    // then diff against what is registered so nothing flickers.
    let mut taken: HashMap<String, String> = HashMap::new();
    let mut regs: Vec<(String, McpTool)> = Vec::new();
    let mut names: Vec<&String> = state.running.keys().collect();
    names.sort();
    for server in names {
        let r = &state.running[server];
        for t in &r.remote {
            let name = t.name.to_string();
            let gate = state.gates.for_tool(server, &name);
            if gate == PolicyAction::Deny {
                continue;
            }
            let owner = if state.natives.contains(&name) {
                Some("native")
            } else {
                taken.get(&name).map(String::as_str)
            };
            if let Some(owner) = owner {
                tracing::warn!("mcp tool {name} from {server} hidden: clashes with {owner}");
                continue;
            }
            taken.insert(name.clone(), server.clone());
            regs.push((
                server.clone(),
                McpTool {
                    spec: ToolSpec {
                        name: name.clone(),
                        description: t.description.as_deref().unwrap_or_default().to_owned(),
                        schema: Value::Object((*t.input_schema).clone()),
                        risk: Risk::Mutating,
                    },
                    remote_name: name,
                    service: Arc::clone(&r.service),
                    gate,
                },
            ));
        }
    }
    orphans.extend(state.running.values_mut().flat_map(|r| r.tools.drain(..)));
    for name in orphans {
        if !taken.contains_key(&name) {
            let _ = registry.ask(UnregisterTool(name)).await;
        }
    }
    for (server, tool) in regs {
        let name = tool.spec.name.clone();
        let _ = registry.ask(RegisterTool(Arc::new(tool))).await;
        if let Some(r) = state.running.get_mut(&server) {
            r.tools.push(name);
        }
    }
}

/// Register the MCP source tools (natives, so they win clashes), spawn the
/// [`Mcp`] actor, run the initial reconcile of `<data_dir>/mcp`, then watch
/// the directory. Returns after the first reconcile so the tools exist when
/// the plugin reports started.
pub async fn start(rt: &Runtime, registry: ActorRef<ToolRegistry>) -> anyhow::Result<()> {
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
        .ask(ListTools)
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
            registry,
        },
        runtime::subscriber_mailbox(),
    );
    mcp.register(MCP_ACTOR)?;
    mcp.ask(Reconcile).await?;

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
    fn remote_url_with_headers() {
        let json = serde_json::json!({"mcpServers": {"api": {
            "type": "sse", "url": "https://x.example/mcp", "headers": {"Authorization": "Bearer t"}
        }}});
        assert_eq!(
            parse_file("r", &json),
            vec![(
                "api".into(),
                ServerSpec {
                    transport: Transport::Http {
                        url: "https://x.example/mcp".into(),
                        headers: [("Authorization".to_string(), "Bearer t".to_string())].into(),
                    }
                }
            )]
        );
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
            "servers:\n  e:\n    gate: ask\n    tools:\n      echo: allow\n      slow: deny\n  f:\n    gate: deny\n",
        )
        .unwrap();
        assert_eq!(g.for_tool("e", "echo"), PolicyAction::Allow);
        assert_eq!(g.for_tool("e", "slow"), PolicyAction::Deny);
        assert_eq!(g.for_tool("e", "other"), PolicyAction::Ask);
        assert_eq!(g.for_server("f"), PolicyAction::Deny);
        assert_eq!(g.for_tool("unknown", "x"), PolicyAction::Allow);
        assert_eq!(serde_yaml::from_str::<Option<Gates>>("").unwrap(), None);
    }

    #[test]
    fn gates_set_roundtrip_drops_empty() {
        let mut g = Gates::default();
        g.set("e", Some("echo"), Some(PolicyAction::Deny));
        g.set("e", None, Some(PolicyAction::Ask));
        g.set("f", None, Some(PolicyAction::Deny));
        let back: Gates = serde_yaml::from_str(&serde_yaml::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.own("e", "echo"), Some(PolicyAction::Deny));
        assert_eq!(back.for_tool("e", "other"), PolicyAction::Ask);

        g.set("e", None, None);
        assert_eq!(
            g.for_tool("e", "echo"),
            PolicyAction::Deny,
            "tool override survives"
        );
        g.set("e", Some("echo"), None);
        g.set("f", None, None);
        assert!(g.servers.is_empty(), "emptied entries dropped: {g:?}");
        assert_eq!(serde_yaml::to_string(&g).unwrap(), "servers: {}\n");
    }
}
