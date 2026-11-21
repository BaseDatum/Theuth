//! MCP client: every `*.json` in `<data_dir>/mcp/` holds one or more servers in
//! whatever layout its README used; each remote tool registers under its own
//! name (no prefix). The directory is watched and reconciled on change;
//! `<data_dir>/mcp/config.yaml` overrides a tool's [`Risk`] per server or per
//! tool, or turns it `off` (design docs 01 §1.4, 12 §5). A tool's default risk
//! comes from its `ToolAnnotations`. `<data_dir>/mcp/repos/` holds git clones
//! of server sources; an entry's `cwd` resolves against the mcp dir (design
//! doc 04 §3). `<data_dir>/mcp/homes.yaml` names each server's home engine:
//! the directory replicates to every mesh engine, but a server runs only at
//! home unless it runs everywhere; a failed start backs off (design doc 187).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::future::BoxFuture;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use notify::Watcher;
use rmcp::model::{CallToolRequestParams, ContentBlock, ProtocolVersion};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, ClientServiceExt, RoleClient, RunningService,
    ServiceExt,
};
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
    DynTool, ErasedTool, Gate, MCP_ACTOR, McpEntry, Policy, PolicyAction, Risk, ToolCx, ToolOutput,
    ToolSource, ToolSpec, git, images, secrets,
};

mod oauth;
mod tools;

/// A server that has not finished its lifecycle (`server/discover`, or the
/// legacy `initialize` after rmcp's 10 s discover probe) + `tools/list` by then
/// is dropped and recorded as a connect error; the next reconcile retries it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 187: the first retry after a failed start; it doubles per failure up to [`RETRY_CAP`].
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_CAP: Duration = Duration::from_secs(30 * 60);
/// 187: the theuth-owned sidecar naming each server's home (replicates with the directory).
const HOMES: &str = "homes.yaml";

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

    /// The entry as an app may inspect it (46 §3.2): scrubbed per
    /// [`McpEntry`], display only. `repo` is the clone under `<dir>/repos/`
    /// holding `cwd`, when any.
    fn entry(&self, dir: &Path) -> McpEntry {
        match &self.transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => McpEntry::Stdio {
                command: secrets::redact(command).into_owned(),
                args: scrub_argv(args),
                cwd: cwd.clone(),
                env: env.keys().cloned().collect(),
                repo: repo_of(dir, self),
            },
            Transport::Http { url, .. } => McpEntry::Http {
                url: scrub_url(url),
            },
        }
    }

    /// [`ErasedTool::origin`] for every tool of `server`: sha256 hex of the
    /// *raw* entry (unexpanded command/args/env/cwd, or URL with query and
    /// headers), so a changed endpoint parameter, argv or credential is a
    /// different identity while nothing of the entry leaves this hash.
    fn origin(&self, server: &str) -> String {
        use sha2::{Digest as _, Sha256};
        let raw = match &self.transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => serde_json::json!({
                "server": server, "command": command, "args": args, "env": env, "cwd": cwd,
            }),
            Transport::Http { url, headers, .. } => serde_json::json!({
                "server": server, "url": url, "headers": headers,
            }),
        };
        format!("mcp:{}", hex::encode(Sha256::digest(raw.to_string())))
    }
}

/// Flag names whose following value (or `=value`) is a credential.
fn credential_flag(flag: &str) -> bool {
    let name = flag.trim_start_matches('-').to_ascii_lowercase();
    name == "p"
        || [
            "token",
            "secret",
            "pass",
            "key",
            "auth",
            "credential",
            "bearer",
        ]
        .iter()
        .any(|w| name.contains(w))
}

/// Argv for display and hashing: vault values → placeholders; the value of
/// a credential-looking flag → `<redacted>`, whether `--flag value` or
/// `--flag=value`. `${VAR}` references stay: they are names, not values.
/// ponytail: name heuristic for literal secrets no vault knows; the vault is
/// the real answer (37), this only keeps an obvious `--token abc` off screen.
fn scrub_argv(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for a in args {
        if mask_next {
            out.push("<redacted>".to_owned());
            mask_next = false;
            continue;
        }
        if a.starts_with('-') {
            match a.split_once('=') {
                Some((flag, _)) if credential_flag(flag) => {
                    out.push(format!("{flag}=<redacted>"));
                    continue;
                }
                None if credential_flag(a) => mask_next = true,
                _ => {}
            }
        }
        out.push(secrets::redact(a).into_owned());
    }
    out
}

/// Scheme, host, port and path only: no userinfo, query or fragment, where
/// tokens travel. Unparsable stays as the vault-redacted text.
fn scrub_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => secrets::redact(url).into_owned(),
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

/// 187: `<data_dir>/mcp/homes.yaml`, keyed by server name so every file shape
/// works and third-party files stay untouched. Machine-written; replicates
/// with the directory (one row, last writer wins: a lost claim is made again
/// by the next reconcile of the engine it belongs to).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Homes {
    servers: BTreeMap<String, Home>,
}

/// One server's line in [`Homes`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Home {
    /// Mesh engine id of the home; `None` until an engine claims it.
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<String>,
    /// The home's name when it was stamped; display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Run on every engine, not only at home.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    everywhere: bool,
    /// Engines it is turned off on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    off: Vec<String>,
}

impl Homes {
    fn engine(&self, server: &str) -> Option<&str> {
        self.servers.get(server)?.engine.as_deref()
    }
}

/// Missing = no homes. Unreadable or unparsable is an error: the caller keeps
/// what it had and writes nothing, so a typo never drops homes mesh-wide.
fn load_homes(dir: &Path) -> Result<Homes, String> {
    let path = dir.join(HOMES);
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_yaml::from_str::<Option<Homes>>(&t)
            .map(Option::unwrap_or_default)
            .map_err(|e| format!("{} is invalid: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Homes::default()),
        Err(e) => Err(format!("{} is unreadable: {e}", path.display())),
    }
}

/// Dotfile temp then rename: the watcher and the mirror never read a half file.
fn write_homes(dir: &Path, homes: &Homes) -> Result<(), String> {
    let text = serde_yaml::to_string(homes).map_err(|e| e.to_string())?;
    let path = dir.join(HOMES);
    let tmp = dir.join(format!(".{HOMES}.tmp"));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// 187: this engine as the mesh names it — the id a [`Home`] holds, and the
/// name shown for it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Machine {
    /// Mesh engine id.
    id: String,
    /// Display name (`mesh.name`, else the hostname).
    name: String,
}

static MACHINE: parking_lot::RwLock<Option<Machine>> = parking_lot::RwLock::new(None);

/// 187: name this engine for MCP homes. `t` calls it before the plugins start
/// (from the stored mesh identity), the mesh plugin at start and on a rename.
/// Never called (no mesh): every server runs here and none is claimed.
pub fn set_machine(id: &str, name: &str) {
    let m = Machine {
        id: id.to_owned(),
        name: name.to_owned(),
    };
    *MACHINE.write() = Some(m.clone());
    if let Ok(Some(mcp)) = ActorRef::<Mcp>::lookup(MCP_ACTOR)
        && let Ok(rt) = tokio::runtime::Handle::try_current()
    {
        rt.spawn(async move {
            let _ = mcp.tell(SetMachine(m)).await;
        });
    }
}

fn machine() -> Option<Machine> {
    MACHINE.read().clone()
}

/// This engine's paired peers (mesh ids, `mesh` `Net::paired_ids`); `None` = unknown (no endpoint yet).
type Paired = fn() -> Option<Vec<String>>;

static PAIRED: std::sync::OnceLock<Paired> = std::sync::OnceLock::new();

/// 187: the mesh plugin hands over its paired set at start, so a server whose
/// home is not paired here runs on one engine still in the mesh.
pub fn set_paired_check(f: Paired) {
    let _ = PAIRED.set(f);
}

fn paired() -> Option<Vec<String>> {
    PAIRED.get().and_then(|f| f())
}

/// 187: the mesh plugin calls this when the paired set changed: homes on an
/// engine that left (or came back) are placed again.
pub fn peers_changed() {
    if let Ok(Some(mcp)) = ActorRef::<Mcp>::lookup(MCP_ACTOR)
        && let Ok(rt) = tokio::runtime::Handle::try_current()
    {
        rt.spawn(async move {
            let _ = mcp.tell(Reconcile).await;
        });
    }
}

/// Where a wanted server stands on this engine, before any connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Here,
    /// Its home is not paired here (it left, or a peer row not applied yet)
    /// and this engine is the lowest id among itself and its peers: it runs
    /// here, written nowhere, so a re-pair gives it back.
    Adopted,
    /// Its home is another engine (or nobody claimed it yet) and it does not run everywhere.
    Elsewhere,
    /// Turned off on this engine.
    OffHere,
}

impl Place {
    fn runs(self) -> bool {
        matches!(self, Place::Here | Place::Adopted)
    }
}

/// No machine (no mesh) = here: a lone engine runs everything. `peers` is
/// this engine's paired set (`None` = unknown: a home is then never adopted).
fn place(home: Option<&Home>, me: Option<&Machine>, peers: Option<&[String]>) -> Place {
    let Some(me) = me else {
        return Place::Here;
    };
    match home {
        Some(h) if h.off.contains(&me.id) => Place::OffHere,
        Some(h) if h.everywhere || h.engine.as_deref() == Some(me.id.as_str()) => Place::Here,
        Some(Home {
            engine: Some(e), ..
        }) if peers.is_some_and(|p| !p.contains(e) && p.iter().all(|p| me.id < *p)) => {
            Place::Adopted
        }
        _ => Place::Elsewhere,
    }
}

/// 187: may this engine claim a server without a home? `writer` is its
/// file's replica row: `None` = no row, so the file appeared here (a peer's
/// file is a row before it is a file); `Some(None)` = a row whose writer is
/// unknown, never claimed; else the row's last writer decides.
fn written_here(writer: Option<&Option<replica::PeerId>>, mine: Option<&replica::PeerId>) -> bool {
    match writer {
        None => true,
        Some(w) => w.is_some() && w.as_ref() == mine,
    }
}

/// What `mcp: <name> home is <…>` says.
fn home_text(home: Option<&Home>, me: Option<&Machine>) -> String {
    let Some(me) = me else {
        return "this machine (no mesh)".to_owned();
    };
    let mut text = match home.and_then(|h| Some((h.engine.as_deref()?, h.name.as_deref()))) {
        Some((id, _)) if id == me.id => me.name.clone(),
        Some((id, name)) => name.map_or_else(|| format!("engine {id}"), str::to_owned),
        // Another engine wrote it, or the claim round could not read the replica.
        None => "not claimed yet".to_owned(),
    };
    if home.is_some_and(|h| h.everywhere) {
        text.push_str(", runs everywhere");
    }
    text
}

/// 187: a start that failed because the command (or its `cwd`) is not on this
/// machine: the reason, else `None`.
fn missing_here(spec: &ServerSpec, e: &anyhow::Error, machine: &str) -> Option<String> {
    let Transport::Stdio { command, cwd, .. } = &spec.transport else {
        return None;
    };
    let enoent = e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    });
    enoent.then(|| match cwd {
        Some(c) if !c.is_dir() => format!("{} does not exist on {machine}", c.display()),
        _ => format!("{} is not installed on {machine}", secrets::redact(command)),
    })
}

/// Wait after the `failures`-th failure in a row: 30 s, doubling, capped at 30 min.
fn backoff(failures: u32) -> Duration {
    RETRY_FIRST
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(RETRY_CAP)
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
    source: ToolSource,
    /// [`ServerSpec::origin`] of the owning server; private to the identity hash.
    origin: String,
    /// `<data_dir>/mcp/images`; image blocks land in `<images>/<session>/`.
    images: PathBuf,
}

impl ErasedTool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn source(&self) -> ToolSource {
        self.source.clone()
    }

    fn origin(&self) -> String {
        self.origin.clone()
    }

    fn call(&self, args: Value, cx: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let arguments = match args {
                Value::Object(m) => m,
                Value::Null => Default::default(),
                other => anyhow::bail!("arguments must be a JSON object, got {other}"),
            };
            let params =
                CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);
            let result = self.service.call_tool(params).await?;
            Ok(output(result, &self.images.join(cx.session.to_string())))
        })
    }
}

/// Longest base64 text that can decode to [`images::MAX_IMAGE_BYTES`].
const MAX_IMAGE_BASE64: usize = (images::MAX_IMAGE_BYTES as usize).div_ceil(3) * 4;

/// Decode one image block (52 §7.1): size is bounded on the encoded text
/// before anything is allocated; content is validated by [`images::save`].
/// The error never quotes the payload.
fn image_block(data: &str, mime: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    if data.len() > MAX_IMAGE_BASE64 {
        anyhow::bail!("exceeds {} MB", images::MAX_IMAGE_BYTES >> 20);
    }
    let bytes = BASE64
        .decode(data)
        .map_err(|_| anyhow::anyhow!("data is not base64"))?;
    images::save(&bytes, mime, dir)
}

/// Replace every string in `v` that is exactly one of `payloads` (an image
/// block's base64 the server repeated in `structuredContent`) with the
/// matching file path. Exact known strings only; nothing is scanned for.
fn replace_payloads(v: &mut Value, payloads: &[(&str, &Path)]) {
    match v {
        Value::String(s) => {
            if let Some((_, p)) = payloads.iter().find(|(d, _)| *d == s.as_str()) {
                *s = p.display().to_string();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|v| replace_payloads(v, payloads)),
        Value::Object(o) => o.values_mut().for_each(|v| replace_payloads(v, payloads)),
        _ => {}
    }
}

/// The model-facing form of a `tools/call` result (52 §7.1). Text blocks and
/// text resources pass through; image blocks become validated files under
/// `dir`, in source order, each leaving a path line where the block was;
/// audio and blob resources leave a placeholder instead of their base64;
/// anything else keeps the JSON dump. A malformed image block makes the
/// whole result an error naming the block, with no payload text. Base64 the
/// server repeated verbatim in `structuredContent` is replaced by the path.
fn output(result: rmcp::model::CallToolResult, dir: &Path) -> ToolOutput {
    let mut files = Vec::new();
    let mut lines = Vec::with_capacity(result.content.len());
    for (i, block) in result.content.iter().enumerate() {
        match block {
            ContentBlock::Text(t) => lines.push(t.text.clone()),
            ContentBlock::Image(img) => match image_block(&img.data, &img.mime_type, dir) {
                Ok(path) => {
                    lines.push(format!("image saved to {}", path.display()));
                    files.push(path);
                }
                Err(e) => {
                    return ToolOutput::err(format!(
                        "content block {i}: invalid {} image: {e}",
                        img.mime_type
                    ));
                }
            },
            ContentBlock::Audio(a) => lines.push(format!("[audio {} omitted]", a.mime_type)),
            ContentBlock::Resource(r) => match &r.resource {
                rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
                    lines.push(text.clone());
                }
                rmcp::model::ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                    lines.push(format!(
                        "[resource {uri} ({}) omitted]",
                        mime_type.as_deref().unwrap_or("application/octet-stream")
                    ));
                }
                _ => lines.push(serde_json::to_string(block).unwrap_or_default()),
            },
            other => lines.push(serde_json::to_string(other).unwrap_or_default()),
        }
    }
    let mut structured = result.structured_content;
    if let Some(s) = structured.as_mut().filter(|_| !files.is_empty()) {
        let payloads: Vec<(&str, &Path)> = result
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Image(img) => Some(img.data.as_str()),
                _ => None,
            })
            .zip(files.iter().map(PathBuf::as_path))
            .collect();
        replace_payloads(s, &payloads);
    }
    ToolOutput {
        content: lines.join("\n"),
        is_error: result.is_error.unwrap_or(false),
        structured,
        files,
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
    /// `homes.yaml` as of the last reconcile (plus this engine's claims).
    homes: Homes,
    /// Native tool names; always win a clash.
    natives: HashSet<String>,
    /// Last connect / list_tools failure per desired server; cleared on success
    /// or when the server leaves the files.
    errors: HashMap<String, String>,
    /// Servers that answered 401 on connect: the `WWW-Authenticate` challenge,
    /// consumed by [`McpLogin`]. Cleared with `errors`.
    login: HashMap<String, String>,
    /// 187: failed starts waiting out their backoff; dropped when the entry
    /// changes or the server stops being wanted here.
    retry: HashMap<String, Retry>,
    /// 187: the state each server was last logged in, so a line is written per change, not per attempt.
    said: HashMap<String, String>,
    /// 187: the home each server was last logged with (`mcp: <name> home is <machine>`).
    told: HashMap<String, String>,
    /// 187: why `homes.yaml` or the replica could not be read, logged once
    /// per distinct problem; `None` while both read fine.
    unreadable: Option<String>,
    /// 187: when the one pending reconcile for `unreadable` fires.
    unreadable_retry: Option<tokio::time::Instant>,
    /// 187: servers running here while their home is not paired (logged once each).
    adopted: HashSet<String>,
    /// [`StartServer`] callers waiting for a pending connect to settle.
    waiters: HashMap<String, Vec<ReplySender<Result<(), String>>>>,
}

/// 187: a failed start and when the next one may run.
struct Retry {
    spec: ServerSpec,
    /// Failures in a row.
    failures: u32,
    due: tokio::time::Instant,
    /// `Some(reason)`: the command (or `cwd`) is not on this machine.
    unavailable: Option<String>,
}

/// Event: a reconcile snapshot, published every time a server changes state
/// (also mid-reconcile as each connect settles). UIs show it as a status
/// line; `connecting` empty means the reconcile is over.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct McpProgress {
    /// Servers still being connected, sorted.
    pub connecting: Vec<String>,
    /// Connected servers, sorted.
    pub connected: Vec<String>,
    /// Servers whose last connect failed, sorted.
    pub failed: Vec<String>,
    /// 187: why `mcp/homes.yaml` (or the replica) could not be read; homes stay
    /// as they were and nothing is written to it until it reads again.
    pub homes_problem: Option<String>,
}

impl wire::WireEvent for McpProgress {
    const TOPIC: &'static str = "tools.mcp-progress";
    const VERSION: u32 = 1;
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
    /// 187: this engine ([`set_machine`]); `None` without a mesh.
    machine: Option<Machine>,
    /// 187: this engine's replica origin, read once `replica.sqlite` exists.
    origin: Option<replica::PeerId>,
    /// 187: `mesh::is_paired` ([`set_paired_check`]); `None` = unknown.
    paired: Paired,
}

/// `tell` from [`set_machine`]: this engine's id or name, then reconcile.
struct SetMachine(Machine);

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

/// `ask` → `Result<(), String>`: (re)start one server this engine runs and wait
/// for it: a failed start waiting out its backoff is tried now, a pending one
/// awaited (bounded by [`CONNECT_TIMEOUT`]). `Ok` once it runs; Err with the
/// connect error, or why it does not run here (`off` in `mcp/config.yaml`,
/// homed elsewhere). Writes nothing: turning a server on or off for every chat
/// is [`SetGate`] (`off` stops it).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StartServer {
    /// Server name.
    pub name: String,
}

/// `ask` → `Result<McpServer, String>`: connect a new entry before writing
/// `<name>.json`, or reconnect an existing name using its saved entry without
/// rewriting its file or home. Connect + tools/list is bounded by 20 seconds.
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
    /// 187: mesh engine id of the home; `None` while no engine claimed it, or without a mesh.
    pub home: Option<String>,
    /// The home's name when it was stamped (a UI prefers the mesh's current name).
    pub home_name: Option<String>,
    /// The home is this engine (always, without a mesh).
    pub home_here: bool,
    /// Runs on every engine, not only at home.
    pub everywhere: bool,
    /// What the server does on this engine.
    pub state: McpState,
    /// Why, for `unavailable_here` and `error`.
    pub reason: Option<String>,
}

/// 187: a server's state on the engine asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum McpState {
    /// Connected; its tools are registered here.
    Running,
    /// A start is in flight.
    Connecting,
    /// Not started here: its home is another engine.
    Elsewhere,
    /// Turned off on this engine ([`McpHomeAction::OffHere`]).
    OffHere,
    /// `off` in `mcp/config.yaml`, on every engine.
    Off,
    /// Its command (or `cwd`) is not on this machine; retried with backoff.
    UnavailableHere,
    /// The last start failed; retried with backoff.
    Error,
}

/// 187: what [`SetMcpHome`] does to a server's line in `mcp/homes.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum McpHomeAction {
    /// Start it on every engine.
    RunEverywhere,
    /// Start it at home only.
    OnlyHome,
    /// Make this engine its home.
    MoveHere,
    /// Never start it on this engine.
    OffHere,
    /// Undo [`McpHomeAction::OffHere`].
    OnHere,
}

/// `ask` → `Result<(), String>`: apply `action` to `server`'s home line,
/// rewrite `mcp/homes.yaml` (it replicates), reconcile now. Err for a name
/// no file wants, or on an engine without a mesh.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SetMcpHome {
    /// Server name.
    pub server: String,
    /// What to change.
    pub action: McpHomeAction,
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
        // A whole-server hide covers every tool, registered now or later.
        let server_hidden = hidden.contains(&crate::registry::mcp_server_key(&name));
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
                            hidden: server_hidden
                                || r.tools.get(&tool).is_some_and(|n| hidden.contains(n)),
                            registered: r.tools.get(&tool).cloned(),
                            description: t.description.as_deref().unwrap_or_default().to_owned(),
                            name: tool,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let home = state.homes.servers.get(&name);
        let me = self.machine.as_ref();
        let retry = state.retry.get(&name);
        let (run, reason) = if state.gates.server_off(&name) {
            (McpState::Off, None)
        } else if running.is_some() {
            (McpState::Running, None)
        } else {
            match place(home, me, (self.paired)().as_deref()) {
                Place::Elsewhere => (McpState::Elsewhere, None),
                Place::OffHere => (McpState::OffHere, None),
                Place::Here | Place::Adopted if state.pending.contains(&name) => {
                    (McpState::Connecting, None)
                }
                Place::Here | Place::Adopted => match (
                    retry.and_then(|r| r.unavailable.clone()),
                    state.errors.get(&name),
                ) {
                    (Some(why), _) => (
                        McpState::UnavailableHere,
                        Some(secrets::redact(&why).into_owned()),
                    ),
                    (None, Some(e)) => (McpState::Error, Some(secrets::redact(e).into_owned())),
                    (None, None) => (McpState::Connecting, None),
                },
            }
        };
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
            home: home.and_then(|h| h.engine.clone()),
            home_name: home.and_then(|h| h.name.clone()),
            home_here: me
                .is_none_or(|m| home.and_then(|h| h.engine.as_deref()) == Some(m.id.as_str())),
            everywhere: home.is_some_and(|h| h.everywhere),
            state: run,
            reason,
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

    /// Answer the [`StartServer`] callers of every server no longer connecting:
    /// `Ok` when it runs, else why not (its last error, or its state).
    fn wake(&mut self) {
        let done: Vec<String> = self
            .state
            .waiters
            .keys()
            .filter(|n| !self.state.pending.contains(*n))
            .cloned()
            .collect();
        for name in done {
            let outcome = self.started(&name);
            for tx in self.state.waiters.remove(&name).unwrap_or_default() {
                tx.send(outcome.clone());
            }
        }
    }

    /// `Ok` when `name` runs here; else why not.
    fn started(&self, name: &str) -> Result<(), String> {
        if self.state.running.contains_key(name) {
            return Ok(());
        }
        let want = desired(&self.dir);
        let s = self.server(name.to_owned(), want.get(name), &HashSet::new());
        Err(s.reason.unwrap_or_else(|| match s.state {
            McpState::Off => format!("{name} is off"),
            McpState::Elsewhere => format!("{name} runs on another engine"),
            other => format!("{name} did not start ({other:?})"),
        }))
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

impl Message<StartServer> for Mcp {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(
        &mut self,
        m: StartServer,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let outcome = match self.known(&m.name) {
            Err(e) => Err(e),
            Ok(()) => {
                // A failed start waiting out its backoff is tried again now.
                self.state.retry.remove(&m.name);
                self.reconcile(ctx.actor_ref()).await;
                if self.state.pending.contains(&m.name) {
                    if let Some(tx) = tx {
                        self.state.waiters.entry(m.name).or_default().push(tx);
                    }
                    return reply;
                }
                self.started(&m.name)
            }
        };
        if let Some(tx) = tx {
            tx.send(outcome);
        }
        reply
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
        let mut req = AuthorizationRequest::new(redirect).with_client_name("theuth");
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
        ctx: &mut Context<Self, Self::Reply>,
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
        let existing = desired(&self.dir).remove(&m.name);
        let adding = existing.is_none();
        let spec = existing.unwrap_or(spec);
        let path = self.dir.join(format!("{}.json", m.name));
        if adding && path.exists() {
            return Err(format!(
                "{} already exists but has no enabled server `{}`; edit it first",
                path.display(),
                m.name
            ));
        }
        if self.state.gates.server_off(&m.name) {
            return Err(format!("`{}` is off in mcp/config.yaml", m.name));
        }
        // 187: an unreadable homes.yaml is the caller's error, before anything is written.
        let homes = match &self.machine {
            Some(_) => Some(load_homes(&self.dir)?),
            None => None,
        };
        if !adding
            && homes.as_ref().is_some_and(|h| {
                !place(
                    h.servers.get(&m.name),
                    self.machine.as_ref(),
                    (self.paired)().as_deref(),
                )
                .runs()
            })
        {
            return Err(format!(
                "`{}` does not run here; change its MCP home before reconnecting",
                m.name
            ));
        }
        // Connect before writing: nothing half-added, the caller fixes the
        // entry and calls again. This one wait is the caller's own request.
        let result = attempt(&m.name, &spec, &self.auth_dir)
            .await
            .map_err(|e| format!("{}: connect failed: {e:#}", m.name))?;
        if adding {
            // Added here, so homed here — before the entry, so no reconcile sees it homeless.
            if let (Some(me), Some(mut homes)) = (&self.machine, homes) {
                homes.servers.insert(
                    m.name.clone(),
                    Home {
                        engine: Some(me.id.clone()),
                        name: Some(me.name.clone()),
                        ..Home::default()
                    },
                );
                write_homes(&self.dir, &homes)?;
                self.state.homes = homes;
            }
            write_json(&path, &m.entry)?;
        }
        // Keep a healthy connection until its replacement is ready, then remove
        // every old registration, including tools the new server no longer offers.
        let orphans = self
            .state
            .running
            .remove(&m.name)
            .map(|r| r.tools.into_values().collect())
            .unwrap_or_default();
        self.state.pending.remove(&m.name);
        self.settle(m.name.clone(), spec, Ok(result), ctx.actor_ref());
        register(&mut self.state, &self.dir, &self.registry, orphans).await;
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
        let mut homes = load_homes(&self.dir)?;
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
        if homes.servers.remove(&m.name).is_some() {
            write_homes(&self.dir, &homes)?;
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
    /// Bring the state in line with the files in `dir`: claim homeless servers
    /// this engine wrote, drop servers whose entry changed, left or no longer
    /// runs here, and start a connect task for every server wanted here that
    /// is not running, connecting or waiting out a backoff. Tasks report back
    /// as [`Settled`], so the actor never waits on a server. Publishes an
    /// [`McpProgress`] when anything moved.
    async fn reconcile(&mut self, me: &ActorRef<Mcp>) {
        let want = wanted(&self.dir);
        let gates = load_gates(&self.dir.join("config.yaml"));
        let mut changed = gates != self.state.gates;
        self.state.gates = gates;
        // An unreadable `homes.yaml` keeps the last good homes and writes nothing.
        let read = match load_homes(&self.dir) {
            Ok(homes) => {
                changed |= homes != self.state.homes;
                self.state.homes = homes;
                self.claim(&want).await
            }
            Err(e) => Err(e),
        };
        changed |= self.note_unreadable(read.err(), me);
        self.tell_homes(&want);

        let machine = self.machine.as_ref();
        let peers = (self.paired)();
        let state = &mut self.state;
        let places: Vec<(String, ServerSpec, Place)> = want
            .into_iter()
            .map(|(name, (_, spec))| {
                let p = place(state.homes.servers.get(&name), machine, peers.as_deref());
                (name, spec, p)
            })
            .collect();
        let adopted: HashSet<String> = places
            .iter()
            .filter(|(_, _, p)| *p == Place::Adopted)
            .map(|(name, _, _)| name.clone())
            .collect();
        for name in adopted.difference(&state.adopted) {
            let home = state.homes.servers.get(name);
            let who = home
                .and_then(|h| h.name.clone().or_else(|| h.engine.clone()))
                .unwrap_or_default();
            tracing::info!("mcp: {name} runs here while its home {who} is not paired");
        }
        state.adopted = adopted;
        let here: BTreeMap<String, ServerSpec> = places
            .into_iter()
            .filter(|(_, _, p)| p.runs())
            .map(|(name, spec, _)| (name, spec))
            .collect();
        let stale: Vec<String> = state
            .running
            .iter()
            .filter(|(name, r)| here.get(*name) != Some(&r.spec) || state.gates.server_off(name))
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
        state.errors.retain(|name, _| here.contains_key(name));
        state.login.retain(|name, _| here.contains_key(name));
        // A changed entry retries at once; so does a server that comes back here.
        state
            .retry
            .retain(|name, r| here.get(name) == Some(&r.spec));
        state.said.retain(|name, _| here.contains_key(name));
        let now = tokio::time::Instant::now();
        for (name, spec) in here {
            if state.running.contains_key(&name)
                || state.pending.contains(&name)
                || state.gates.server_off(&name)
                || state.retry.get(&name).is_some_and(|r| r.due > now)
            {
                continue;
            }
            changed = true;
            state.pending.insert(name.clone());
            let me = me.clone();
            let auth_dir = self.auth_dir.clone();
            let task = tokio::spawn({
                let (name, spec) = (name.clone(), spec.clone());
                async move { attempt(&name, &spec, &auth_dir).await }
            });
            tokio::spawn(async move {
                // A panicking connect is a failed one: `pending` never sticks.
                let result = task
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("connect task failed: {e}")));
                let _ = me.tell(Settled { name, spec, result }).await;
            });
        }
        if changed {
            register(&mut self.state, &self.dir, &self.registry, orphans).await;
            self.rt.publish(progress(&self.state)).await;
        }
        self.wake();
    }

    /// 187: home every wanted server `homes.yaml` has no home for and whose
    /// file this engine wrote ([`written_here`]); drop this engine's own lines
    /// of servers no file defines at all (disabled or malformed entries keep
    /// theirs; nothing is dropped while a file fails to parse). Writes
    /// `homes.yaml` when anything changed. A peer's file waits for that peer's
    /// claim; a home that is not paired is adopted at runtime only
    /// ([`Place::Adopted`]), never written. No machine: nothing to claim. Err
    /// (nothing written) when the replica cannot be read: an unread row is
    /// never "no row".
    async fn claim(&mut self, want: &BTreeMap<String, (String, ServerSpec)>) -> Result<(), String> {
        let Some(me) = self.machine.clone() else {
            return Ok(());
        };
        let homes = &self.state.homes;
        let homeless: Vec<(&String, &String)> = want
            .iter()
            .filter(|(name, _)| homes.engine(name).is_none())
            .map(|(name, (file, _))| (name, file))
            .collect();
        let mine: Vec<&String> = homes
            .servers
            .iter()
            .filter(|(name, h)| {
                h.engine.as_deref() == Some(me.id.as_str()) && !want.contains_key(*name)
            })
            .map(|(name, _)| name)
            .collect();
        let gone: Vec<&String> = match (mine.is_empty(), defined(&self.dir)) {
            (false, Some(defined)) => mine.into_iter().filter(|n| !defined.contains(*n)).collect(),
            _ => Vec::new(),
        };
        if homeless.is_empty() && gone.is_empty() {
            return Ok(());
        }
        let mut next = homes.clone();
        if !homeless.is_empty() {
            if self.origin.is_none() {
                self.origin = replica::files::stored_origin(&self.dir.with_file_name(replica::DB))
                    .await
                    .map_err(|e| format!("replica.sqlite unreadable: {e:#}"))?;
            }
            let writers = replica::files::writers(&self.dir.with_file_name("mcp.sqlite"), "mcp")
                .await
                .map_err(|e| format!("mcp.sqlite unreadable: {e:#}"))?;
            for (name, file) in &homeless {
                if written_here(writers.get(*file), self.origin.as_ref()) {
                    let h = next.servers.entry((*name).clone()).or_default();
                    h.engine = Some(me.id.clone());
                    h.name = Some(me.name.clone());
                }
            }
        }
        for name in gone {
            next.servers.remove(name);
        }
        if next != self.state.homes {
            write_homes(&self.dir, &next)?;
            self.state.homes = next;
        }
        Ok(())
    }

    /// 187: one line when `homes.yaml` or the replica becomes unreadable (or a
    /// different problem), one when it reads again; while it fails ONE
    /// reconcile is pending, [`RETRY_FIRST`] out (a locked file at boot clears
    /// by itself). True when the problem changed (the UI shows it).
    fn note_unreadable(&mut self, problem: Option<String>, me: &ActorRef<Mcp>) -> bool {
        let changed = self.state.unreadable != problem;
        if changed {
            match &problem {
                Some(e) => {
                    tracing::warn!("mcp homes: {e}; homes kept as they were, nothing claimed")
                }
                None => tracing::info!("mcp homes: readable again"),
            }
            self.state.unreadable.clone_from(&problem);
        }
        let now = tokio::time::Instant::now();
        if problem.is_some() && self.state.unreadable_retry.is_none_or(|at| at <= now) {
            self.state.unreadable_retry = Some(now + RETRY_FIRST);
            let me = me.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RETRY_FIRST).await;
                let _ = me.tell(Reconcile).await;
            });
        }
        changed
    }

    /// One `mcp: <name> home is <machine>` line per server and change.
    fn tell_homes(&mut self, want: &BTreeMap<String, (String, ServerSpec)>) {
        let state = &mut self.state;
        state.told.retain(|name, _| want.contains_key(name));
        for name in want.keys() {
            let text = home_text(state.homes.servers.get(name), self.machine.as_ref());
            if state.told.get(name) != Some(&text) {
                tracing::info!("mcp: {name} home is {text}");
                state.told.insert(name.clone(), text);
            }
        }
    }

    /// Record a settled attempt for a server still wanted here. A failure
    /// backs off ([`backoff`]) and wakes a reconcile when due; a line is
    /// logged only when the server's state changes, never per attempt.
    fn settle(&mut self, name: String, spec: ServerSpec, result: Attempt, me: &ActorRef<Mcp>) {
        let state = &mut self.state;
        let e = match result {
            Ok((service, remote)) => {
                // rmcp keeps which lifecycle won private, so the version alone is
                // logged; one past `NO_INITIALIZE` can only come from discover.
                let protocol = service
                    .peer_info()
                    .map(|i| i.protocol_version.as_str().to_owned())
                    .unwrap_or_default();
                tracing::info!(server = %name, count = remote.len(), %protocol, "mcp server connected");
                state.errors.remove(&name);
                state.login.remove(&name);
                state.retry.remove(&name);
                state.said.insert(name.clone(), "running".to_owned());
                state.running.insert(
                    name,
                    Running {
                        spec,
                        service,
                        remote,
                        tools: BTreeMap::new(),
                    },
                );
                return;
            }
            Err(e) => e,
        };
        let failures = state
            .retry
            .remove(&name)
            .filter(|r| r.spec == spec)
            .map_or(1, |r| r.failures + 1);
        let delay = backoff(failures);
        let host = self.machine.as_ref().map_or_else(
            || platform::process::hostname().to_owned(),
            |m| m.name.clone(),
        );
        let unavailable = missing_here(&spec, &e, &host);
        let challenge = e
            .downcast_ref::<ClientInitializeError>()
            .and_then(ClientInitializeError::auth_challenge);
        let kind = if let Some(why) = &unavailable {
            format!("unavailable: {why}")
        } else if challenge.is_some() {
            "login".to_owned()
        } else {
            "error".to_owned()
        };
        let new = state.said.get(&name) != Some(&kind);
        if let Some(why) = &unavailable {
            // Not an error any more: the reason is the state.
            state.errors.remove(&name);
            state.login.remove(&name);
            if new {
                tracing::warn!(server = %name, "mcp server not available here: {why}; retrying from {RETRY_FIRST:?}, backing off to {RETRY_CAP:?}");
            } else {
                tracing::debug!(server = %name, failures, "mcp server still not available here: {why}");
            }
        } else if let Some(challenge) = challenge {
            if new {
                tracing::info!(server = %name, "mcp server needs login");
            }
            state.login.insert(name.clone(), challenge.to_owned());
            state.errors.insert(
                name.clone(),
                format!("{name}: login required (/login {name})"),
            );
        } else {
            if new {
                tracing::warn!(server = %name, error = %e, "mcp connect failed; retrying from {RETRY_FIRST:?}, backing off to {RETRY_CAP:?}");
            } else {
                tracing::debug!(server = %name, error = %e, failures, "mcp connect failed again");
            }
            state
                .errors
                .insert(name.clone(), format!("{name}: connect failed: {e:#}"));
        }
        state.said.insert(name.clone(), kind);
        state.retry.insert(
            name,
            Retry {
                spec,
                failures,
                due: tokio::time::Instant::now() + delay,
                unavailable,
            },
        );
        let me = me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = me.tell(Reconcile).await;
        });
    }

    /// Drop `names` (ending their processes) and unregister their tools, then
    /// reconcile: every dropped server the files still want reconnects, at
    /// once even if it was backing off.
    async fn restart(&mut self, me: &ActorRef<Mcp>, names: &[String]) {
        for name in names {
            tracing::info!(server = %name, "mcp server restarting");
            self.state.retry.remove(name);
            if let Some(r) = self.state.running.remove(name) {
                for tool in r.tools.into_values() {
                    let _ = self.registry.ask(UnregisterTool(tool)).await;
                }
            }
        }
        self.reconcile(me).await;
    }
}

impl Message<SetMachine> for Mcp {
    type Reply = ();

    async fn handle(&mut self, m: SetMachine, ctx: &mut Context<Self, Self::Reply>) {
        if self.machine.as_ref() != Some(&m.0) {
            self.machine = Some(m.0);
            self.reconcile(ctx.actor_ref()).await;
        }
    }
}

impl Message<SetMcpHome> for Mcp {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetMcpHome,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.known(&m.server)?;
        let Some(me) = self.machine.clone() else {
            return Err("this engine is in no mesh: every server runs here".into());
        };
        // ponytail: one replicated row, last writer wins — two engines changing
        // homes within a sync window can lose one change; set it again.
        let mut homes = load_homes(&self.dir)?;
        let h = homes.servers.entry(m.server.clone()).or_default();
        match m.action {
            McpHomeAction::RunEverywhere => h.everywhere = true,
            McpHomeAction::OnlyHome => h.everywhere = false,
            McpHomeAction::MoveHere => {
                h.engine = Some(me.id.clone());
                h.name = Some(me.name);
                h.off.retain(|e| *e != me.id);
            }
            McpHomeAction::OffHere if !h.off.contains(&me.id) => h.off.push(me.id),
            McpHomeAction::OffHere => {}
            McpHomeAction::OnHere => h.off.retain(|e| *e != me.id),
        }
        write_homes(&self.dir, &homes)?;
        tracing::info!(server = %m.server, action = ?m.action, "mcp home changed");
        self.reconcile(ctx.actor_ref()).await;
        Ok(())
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
        // An explicit add/reconnect may already have superseded this attempt.
        if !self.state.pending.remove(&m.name) {
            self.wake();
            return;
        }
        let wanted = desired(&self.dir).get(&m.name) == Some(&m.spec)
            && !self.state.gates.server_off(&m.name)
            && place(
                self.state.homes.servers.get(&m.name),
                self.machine.as_ref(),
                (self.paired)().as_deref(),
            )
            .runs();
        if !wanted {
            // The entry changed, left or moved away while connecting: `result`
            // is dropped (ending the process) and reconcile starts over if needed.
            self.reconcile(ctx.actor_ref()).await;
            return;
        }
        let connected = m.result.is_ok();
        self.settle(m.name, m.spec, m.result, ctx.actor_ref());
        if connected {
            register(&mut self.state, &self.dir, &self.registry, Vec::new()).await;
        }
        self.rt.publish(progress(&self.state)).await;
        self.wake();
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

/// Expand `{{secret:NAME}}` placeholders (37 §3.4; an unknown name is an
/// error), then `$VAR`, `${VAR:-x}`, `$(cmd)` and backticks with
/// [`platform::shell`] (the login shell, else `/bin/sh`), so a file can say
/// `"GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)"` or `"${GRAFANA_TOKEN}"`,
/// or `"Authorization": "Bearer {{secret:grafana}}"` and store nothing.
/// The string is fed as an unquoted heredoc: quotes stay literal, `\$` is a
/// literal dollar, and the shell's own parser handles nesting. A `$`-free
/// string is returned as is; a failed substitution is a warning and empty
/// (POSIX), the server's own auth error names the consequence.
async fn expand(raw: &str) -> anyhow::Result<String> {
    let mut s = raw.to_owned();
    if s.contains("{{secret:") {
        let mut v = serde_json::Value::String(s);
        crate::secrets::expand(&mut v).map_err(anyhow::Error::msg)?;
        s = match v {
            serde_json::Value::String(expanded) => expanded,
            other => other.to_string(),
        };
    }
    if !s.contains('$') && !s.contains('`') {
        return Ok(s);
    }
    // ponytail: `ShellKind::Brush` (Windows) is 58 §11 step 3 — brush runs the
    // heredoc in-process there; until then the string is kept as written.
    let Some(mut cmd) =
        platform::shell().command(&format!("cat <<__theuth_mcp__\n{s}\n__theuth_mcp__"), false)
    else {
        tracing::warn!(value = %raw, "mcp expansion: no login shell on this platform; value kept as written");
        return Ok(s);
    };
    let out = cmd.output().await?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !err.trim().is_empty() {
        // `raw`, not `s`: the expanded string may hold a secret value.
        tracing::warn!(value = %raw, status = %out.status, "mcp expansion: {}", err.trim());
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

/// HTTP servers run the newest protocol both sides speak (175): `server/discover`
/// offering every version rmcp knows, newest first, and for a server that
/// reports itself legacy the `initialize` handshake at the newest version that
/// still has one (never [`ProtocolVersion::LATEST`], which a legacy server
/// downgrades and then rejects the session for).
fn http_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: ProtocolVersion::KNOWN_VERSIONS
            .iter()
            .rev()
            .cloned()
            .collect(),
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    }
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
            platform::process::hide_console(cmd.as_std_mut());
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
                ().serve_with_lifecycle(
                    StreamableHttpClientTransport::from_config(cfg),
                    http_lifecycle(),
                )
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
                ().serve_with_lifecycle(
                    StreamableHttpClientTransport::with_client(AuthClient::new(http, manager), cfg),
                    http_lifecycle(),
                )
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
    wanted(dir)
        .into_iter()
        .map(|(name, (_, spec))| (name, spec))
        .collect()
}

/// [`desired`] with the name of the file each server comes from.
fn wanted(dir: &Path) -> BTreeMap<String, (String, ServerSpec)> {
    let mut out = BTreeMap::new();
    let mut owner: HashMap<String, PathBuf> = HashMap::new();
    for path in json_files(dir) {
        let Some(json) = read_json(&path) else {
            continue;
        };
        let file = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_owned();
        for (name, spec) in parse_file(stem(&path), &json, dir) {
            if let Some(first) = owner.get(&name) {
                tracing::warn!(
                    server = %name, kept = %first.display(), ignored = %path.display(),
                    "duplicate mcp server name"
                );
                continue;
            }
            owner.insert(name.clone(), path.clone());
            out.insert(name, (file.clone(), spec));
        }
    }
    out
}

/// 187: every server name any file defines, disabled or malformed entries
/// too (the shapes of [`parse_file`]); `None` when a file does not parse.
fn defined(dir: &Path) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for path in json_files(dir) {
        let json = read_json(&path)?;
        if is_entry(&json) {
            out.insert(stem(&path).to_owned());
            continue;
        }
        for (k, v) in json.as_object().into_iter().flatten() {
            if is_entry(v) {
                out.insert(k.clone());
            } else if let Some(wrapper) = v.as_object() {
                out.extend(
                    wrapper
                        .iter()
                        .filter(|(_, v2)| is_entry(v2))
                        .map(|(k2, _)| k2.clone()),
                );
            }
        }
    }
    Some(out)
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
        homes_problem: state.unreadable.clone(),
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
async fn register(
    state: &mut State,
    dir: &Path,
    registry: &ActorRef<ToolRegistry>,
    mut orphans: Vec<String>,
) {
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
                    source: ToolSource::Mcp {
                        server: server.clone(),
                        entry: r.spec.entry(dir),
                    },
                    origin: r.spec.origin(server),
                    images: dir.join("images"),
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
    // 67 §5.1: `mcp/*.json` and `mcp/config.yaml` replicate; `repos/` does not (a directory).
    tokio::spawn(::replica::files::mirror_at(
        rt.data_dir.join("mcp.sqlite"),
        dir.clone(),
        "mcp",
        std::time::Duration::from_secs(120),
        None,
    ));
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
            machine: machine(),
            origin: None,
            paired,
        },
        runtime::subscriber_mailbox(),
    );
    mcp.register(MCP_ACTOR)?;
    mcp.tell(Reconcile).await?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // 187: only server configs, `config.yaml` and `homes.yaml` matter; an
        // audit append or an image write is no reason to reconcile.
        let config = |p: &PathBuf| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                !n.starts_with('.') && (n.ends_with(".json") || n == "config.yaml" || n == HOMES)
            })
        };
        if res.is_ok_and(|e| e.paths.iter().any(config)) {
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

    fn call_result(v: Value) -> rmcp::model::CallToolResult {
        serde_json::from_value(v).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        BASE64.encode(bytes)
    }

    #[test]
    fn output_turns_image_blocks_into_files_in_source_order() {
        use image::ImageFormat;
        let guard = crate::TestDir::new("mcp-img");
        let dir = guard.0.clone();
        let (png, jpg) = (
            b64(&images::tests::encoded(ImageFormat::Png, 2, 2, 1)),
            b64(&images::tests::encoded(ImageFormat::Jpeg, 2, 2, 1)),
        );
        let out = output(
            call_result(serde_json::json!({
                "content": [
                    { "type": "text", "text": "before" },
                    { "type": "image", "data": png, "mimeType": "image/png" },
                    { "type": "audio", "data": "AAAA", "mimeType": "audio/wav" },
                    { "type": "resource", "resource": { "uri": "file:///r.txt", "text": "embedded" } },
                    { "type": "resource", "resource": { "uri": "file:///r.bin", "blob": "AAAA" } },
                    { "type": "image", "data": jpg, "mimeType": "image/jpeg" },
                    { "type": "text", "text": "after" },
                ],
                "structuredContent": { "shot": png, "nested": [ { "again": png } ], "other": "kept" },
            })),
            &dir,
        );
        assert!(!out.is_error);
        assert_eq!(out.files.len(), 2);
        assert_eq!(out.files[0].extension().unwrap(), "png");
        assert_eq!(out.files[1].extension().unwrap(), "jpg");
        assert!(out.files.iter().all(|f| f.starts_with(&dir) && f.is_file()));
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "before");
        assert_eq!(
            lines[1],
            format!("image saved to {}", out.files[0].display())
        );
        assert_eq!(lines[2], "[audio audio/wav omitted]");
        assert_eq!(lines[3], "embedded");
        assert_eq!(
            lines[4],
            "[resource file:///r.bin (application/octet-stream) omitted]"
        );
        assert_eq!(
            lines[5],
            format!("image saved to {}", out.files[1].display())
        );
        assert_eq!(lines[6], "after");
        // No base64 anywhere the model reads; exact repeats in structured
        // content became the path, everything else stayed.
        let structured = out.structured.unwrap();
        let dump = serde_json::to_string(&structured).unwrap();
        assert!(!out.content.contains(&png) && !out.content.contains(&jpg));
        assert!(!dump.contains(&png));
        let path = out.files[0].display().to_string();
        assert_eq!(structured["shot"], path);
        assert_eq!(structured["nested"][0]["again"], path);
        assert_eq!(structured["other"], "kept");
    }

    #[test]
    fn output_rejects_malformed_images_without_echoing_them() {
        use image::ImageFormat;
        // Uncreated on purpose: the test asserts a rejected image leaves no dir.
        let guard = crate::TestDir::path("mcp-bad");
        let dir = guard.0.clone();
        let png = images::tests::encoded(ImageFormat::Png, 2, 2, 1);
        let check = |data: String, mime: &str, needle: &str| {
            let out = output(
                call_result(serde_json::json!({
                    "content": [
                        { "type": "text", "text": "ok" },
                        { "type": "image", "data": data, "mimeType": mime },
                    ],
                })),
                &dir,
            );
            assert!(out.is_error, "{mime}: {}", out.content);
            assert!(
                out.content.starts_with("content block 1: invalid"),
                "{}",
                out.content
            );
            assert!(out.content.contains(needle), "{}", out.content);
            assert!(
                !out.content.contains(&data[..8]),
                "payload leaked: {}",
                out.content
            );
            assert!(out.files.is_empty());
        };
        check(
            b64(&png),
            "image/jpeg",
            "content is image/png but was labeled image/jpeg",
        );
        check(
            b64(b"not an image at all"),
            "image/png",
            "not a recognized image",
        );
        check(b64(&png[..png.len() / 2]), "image/png", "invalid image/png");
        check("%%%not base64%%%".into(), "image/png", "not base64");
        check(b64(&png), "image/bmp", "not a supported image type");
        // Oversize is refused on the encoded length, before decoding.
        check(
            "A".repeat(MAX_IMAGE_BASE64 + 4),
            "image/png",
            "exceeds 5 MB",
        );
        assert!(!dir.exists());
    }

    #[test]
    fn output_keeps_plain_results_as_before() {
        let out = output(
            call_result(serde_json::json!({
                "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }],
                "structuredContent": { "n": 1 },
                "isError": true,
            })),
            Path::new("/nonexistent"),
        );
        assert_eq!(out.content, "a\nb");
        assert!(out.is_error);
        assert_eq!(out.structured, Some(serde_json::json!({ "n": 1 })));
        assert!(out.files.is_empty());
    }

    #[test]
    fn origin_separates_backends_the_scrubbed_entry_shows_alike() {
        let dir = Path::new(DIR);
        let http = |url: &str| ServerSpec {
            transport: Transport::Http {
                url: url.into(),
                headers: BTreeMap::new(),
                oauth: None,
            },
        };
        let (a, b) = (
            http("https://api.example/mcp?region=eu&token=s3cret"),
            http("https://api.example/mcp?region=us&token=s3cret"),
        );
        assert_ne!(a.origin("srv"), b.origin("srv"));
        assert_ne!(a.origin("srv"), a.origin("other"));
        assert_eq!(a.entry(dir), b.entry(dir));
        assert_eq!(
            a.entry(dir),
            McpEntry::Http {
                url: "https://api.example/mcp".into()
            }
        );
        let shown = serde_json::to_string(&a.entry(dir)).unwrap() + &a.origin("srv");
        assert!(!shown.contains("s3cret") && !shown.contains("region"));

        let (c, d) = (
            stdio(
                "srv",
                &["--keyspace", "prod", "--token", "abc"],
                &[("K", "v1")],
            ),
            stdio(
                "srv",
                &["--keyspace", "stage", "--token", "abc"],
                &[("K", "v1")],
            ),
        );
        assert_ne!(c.origin("srv"), d.origin("srv"));
        // A changed env value or credential is a new identity too, conservatively.
        assert_ne!(
            c.origin("srv"),
            stdio(
                "srv",
                &["--keyspace", "prod", "--token", "abc"],
                &[("K", "v2")]
            )
            .origin("srv")
        );
        assert_ne!(
            c.origin("srv"),
            stdio(
                "srv",
                &["--keyspace", "prod", "--token", "xyz"],
                &[("K", "v1")]
            )
            .origin("srv")
        );
        // The entry shows argv and env *names* but never the credential;
        // which non-credential values the name heuristic also masks is
        // incidental and not pinned here.
        let shown = serde_json::to_string(&c.entry(dir)).unwrap() + &c.origin("srv");
        assert!(!shown.contains("abc"), "token leaked: {shown}");
        let McpEntry::Stdio { args, env, .. } = c.entry(dir) else {
            panic!("stdio entry");
        };
        assert_eq!(args.len(), 4);
        assert_eq!(
            [args[0].as_str(), args[2].as_str(), args[3].as_str()],
            ["--keyspace", "--token", "<redacted>"]
        );
        assert_eq!(env, ["K"]);
        assert_eq!(
            scrub_argv(&[
                "--api-key=abc".into(),
                "-p".into(),
                "pw".into(),
                "--v".into()
            ]),
            ["--api-key=<redacted>", "-p", "<redacted>", "--v"]
        );
    }

    #[tokio::test]
    async fn expand_uses_the_shell_and_keeps_quotes_literal() {
        // SAFETY: test-only unique key, set before any thread reads it.
        unsafe { std::env::set_var("THEUTH_MCP_T", "tok") };
        assert_eq!(
            expand("Bearer ${THEUTH_MCP_T}-$(printf hi)-\"q\" \\$lit")
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
        let guard = crate::TestDir::new("mcp");
        let dir = guard.0.clone();
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

    #[test]
    fn http_lifecycle_prefers_newest_and_falls_back_to_newest_initialize() {
        let ClientLifecycleMode::Auto {
            preferred_versions: v,
            legacy_version,
        } = http_lifecycle()
        else {
            panic!("HTTP lifecycle is not Auto");
        };
        assert_eq!(v[0], ProtocolVersion::LATEST);
        assert!(v.windows(2).all(|w| w[0].as_str() > w[1].as_str()), "{v:?}");
        assert_eq!(v.len(), ProtocolVersion::KNOWN_VERSIONS.len());
        assert!(
            ProtocolVersion::KNOWN_VERSIONS
                .iter()
                .all(|k| v.contains(k))
        );
        assert_eq!(
            legacy_version,
            Some(ProtocolVersion::LATEST_WITH_INITIALIZE)
        );
    }

    /// One `echo` tool. `legacy` = a pre-2026-07-28 server: no 2026 version,
    /// and `server/discover` is an unknown method (-32601). `asked` = the
    /// version of the first `initialize` it saw.
    #[derive(Clone, Default)]
    struct TestServer {
        legacy: bool,
        asked: Arc<std::sync::OnceLock<ProtocolVersion>>,
    }

    impl rmcp::ServerHandler for TestServer {
        fn get_info(&self) -> rmcp::model::ServerConfig {
            rmcp::model::ServerConfig::new(
                rmcp::model::ServerCapabilities::builder()
                    .enable_tools()
                    .build(),
            )
        }

        fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
            std::borrow::Cow::Borrowed(if self.legacy {
                ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25)
            } else {
                ProtocolVersion::KNOWN_VERSIONS
            })
        }

        async fn initialize(
            &self,
            request: rmcp::model::InitializeRequestParams,
            context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
        ) -> Result<rmcp::model::InitializeResult, rmcp::ErrorData> {
            let _ = self.asked.set(request.protocol_version.clone());
            context.peer.set_peer_info(request.clone());
            self.negotiate_initialize(&request)
        }

        async fn discover(
            &self,
            _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
        ) -> Result<rmcp::model::DiscoverResult, rmcp::ErrorData> {
            if self.legacy {
                return Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::DiscoverRequestMethod,
                >());
            }
            Ok(rmcp::model::DiscoverResult::from_server_info(
                self.supported_protocol_versions().into_owned(),
                self.get_info(),
            ))
        }

        async fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
            let schema = serde_json::json!({"type": "object"});
            Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                rmcp::model::Tool::new("echo", "echo", schema.as_object().unwrap().clone()),
            ]))
        }

        async fn call_tool(
            &self,
            req: CallToolRequestParams,
            _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
        ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
            Ok(
                rmcp::model::CallToolResult::success(vec![ContentBlock::text(format!(
                    "echo:{}",
                    req.name
                ))])
                .into(),
            )
        }
    }

    /// `app` on `http://127.0.0.1:<port>/mcp` for the rest of the test.
    async fn serve_http(app: axum::Router) -> ServerSpec {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        ServerSpec {
            transport: Transport::Http {
                url,
                headers: BTreeMap::new(),
                oauth: None,
            },
        }
    }

    async fn serve_mcp(server: TestServer) -> ServerSpec {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
        let svc = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        serve_http(axum::Router::new().route_service("/mcp", svc)).await
    }

    /// Connect, list and call `echo`; the negotiated version.
    async fn connect_list_call(spec: &ServerSpec, auth_dir: &Path) -> ProtocolVersion {
        let service = connect("t", spec, auth_dir).await.unwrap();
        let tools = service.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        let result = service
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .unwrap();
        assert_eq!(output(result, auth_dir).content, "echo:echo");
        service.peer_info().unwrap().protocol_version.clone()
    }

    #[tokio::test]
    async fn http_connect_runs_discover_at_the_latest_version() {
        let dir = crate::TestDir::new("mcp-modern");
        let server = TestServer::default();
        let mut spec = serve_mcp(server.clone()).await;
        // The hand-auth path; the OAuth path is the legacy case below.
        let Transport::Http { headers, .. } = &mut spec.transport else {
            unreachable!()
        };
        headers.insert("Authorization".into(), "Bearer t".into());
        assert_eq!(
            connect_list_call(&spec, &dir.0).await,
            ProtocolVersion::V_2026_07_28
        );
        assert_eq!(server.asked.get(), None, "sent initialize");
    }

    #[tokio::test]
    async fn http_connect_falls_back_to_initialize_for_a_legacy_server() {
        let dir = crate::TestDir::new("mcp-legacy");
        let server = TestServer {
            legacy: true,
            ..TestServer::default()
        };
        let spec = serve_mcp(server.clone()).await;
        let started = std::time::Instant::now();
        assert_eq!(
            connect_list_call(&spec, &dir.0).await,
            ProtocolVersion::V_2025_11_25
        );
        // D3: the fallback asks for the newest version with a handshake, not LATEST.
        assert_eq!(server.asked.get(), Some(&ProtocolVersion::V_2025_11_25));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "fallback waited on the discover timeout: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn http_401_on_the_discover_probe_is_login_required() {
        let dir = crate::TestDir::new("mcp-401");
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::any(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", r#"Bearer realm="t""#)],
                )
            }),
        );
        let spec = serve_http(app).await;
        let Transport::Http { url, .. } = &spec.transport else {
            unreachable!()
        };
        let mcp = actor(&dir.0, None);
        let entry = serde_json::json!({ "url": url });
        std::fs::write(dir.0.join("mcp/s.json"), entry.to_string()).unwrap();
        mcp.ask(Reconcile).await.unwrap();
        settled(&mcp, "s").await;
        let s = listed(&mcp, "s").await;
        assert_eq!(
            (s.state, s.reason.as_deref(), s.auth),
            (
                McpState::Error,
                Some("s: login required (/login s)"),
                McpAuth::Required
            )
        );
        assert_eq!(
            mcp.ask(Failures("s")).await.unwrap(),
            Some(1),
            "backs off too"
        );
    }

    /// T3 (175), live: `mbx test -p tools github_mcp_live -- --ignored --nocapture`.
    /// Needs `gh auth login`; prints the protocol, the tool count and the login only.
    #[tokio::test]
    #[ignore = "live: api.githubcopilot.com with `gh auth token`"]
    async fn github_mcp_live_negotiates_the_latest_version() {
        let dir = crate::TestDir::new("mcp-live");
        let spec = ServerSpec {
            transport: Transport::Http {
                url: "https://api.githubcopilot.com/mcp/".into(),
                headers: BTreeMap::from([(
                    "Authorization".into(),
                    "Bearer $(gh auth token)".into(),
                )]),
                oauth: None,
            },
        };
        // `{e:#}`, never `{e:?}`: no chance of a header in the panic.
        let service = connect("github", &spec, &dir.0)
            .await
            .unwrap_or_else(|e| panic!("connect: {e:#}"));
        let protocol = service.peer_info().unwrap().protocol_version.clone();
        let tools = service
            .list_all_tools()
            .await
            .unwrap_or_else(|e| panic!("tools/list: {e:#}"));
        let me = service
            .call_tool(CallToolRequestParams::new("get_me"))
            .await
            .unwrap_or_else(|e| panic!("get_me: {e:#}"));
        let me = output(me, &dir.0);
        assert!(!me.is_error, "get_me failed");
        let login = serde_json::from_str::<Value>(&me.content)
            .ok()
            .and_then(|v| v["login"].as_str().map(str::to_owned))
            .expect("get_me has a login");
        println!(
            "protocol={} tools={} login={login}",
            protocol.as_str(),
            tools.len()
        );
        assert_eq!(protocol, ProtocolVersion::V_2026_07_28);
        assert!(!tools.is_empty());
    }

    /// T4 (175), live: a public no-auth legacy server (gitmcp.io answers
    /// `server/discover` with 400 `Mcp-Session-Id header is required`).
    /// `mbx test -p tools legacy_mcp_live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live: gitmcp.io"]
    async fn legacy_mcp_live_falls_back_fast() {
        let dir = crate::TestDir::new("mcp-live-legacy");
        let spec = ServerSpec {
            transport: Transport::Http {
                url: "https://gitmcp.io/docs".into(),
                headers: BTreeMap::new(),
                oauth: None,
            },
        };
        let started = std::time::Instant::now();
        let service = connect("gitmcp", &spec, &dir.0)
            .await
            .unwrap_or_else(|e| panic!("connect: {e:#}"));
        let connected = started.elapsed();
        let protocol = service.peer_info().unwrap().protocol_version.clone();
        let tools = service.list_all_tools().await.unwrap();
        println!(
            "protocol={} tools={} connect={connected:?}",
            protocol.as_str(),
            tools.len()
        );
        assert!(protocol.has_initialize(), "{protocol:?}");
        assert!(!tools.is_empty());
        assert!(connected < Duration::from_secs(5), "{connected:?}");
    }

    // ---- 187: homes and the calm failure mode ----

    const MISSING: &str = "/nonexistent/mcphome-test-server";

    #[tokio::test]
    async fn mcp_live_add_has_a_bounded_connect_wait() {
        let root = crate::TestDir::new("mcp-live-timeout");
        let mcp = actor(&root.0, None);
        let started = std::time::Instant::now();
        let error = mcp
            .ask(AddServer {
                name: "slow".into(),
                entry: serde_json::json!({
                    "command": "python3",
                    "args": [concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mcp_live.py"), "60"]
                }),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("connect failed") && error.contains("no response within 20s"),
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "connect wait was not bounded"
        );
        assert!(
            !root.0.join("mcp/slow.json").exists(),
            "a timed-out add must not leave a half-added server"
        );
    }

    #[tokio::test]
    async fn mcp_live_reconnect_preserves_the_saved_entry_and_its_siblings() {
        let root = crate::TestDir::new("mcp-live-wrapper");
        let mcp = actor(&root.0, None);
        let spec = serve_mcp(TestServer::default()).await;
        let Transport::Http { url, .. } = spec.transport else {
            unreachable!()
        };
        let saved = serde_json::json!({"mcpServers": {
            "live": {"url": url},
            "sibling": {"command": MISSING, "disabled": true}
        }});
        let path = root.0.join("mcp/bundle.json");
        write_json(&path, &saved).unwrap();
        mcp.ask(Reconcile).await.unwrap();
        settled(&mcp, "live").await;
        let reply = mcp
            .ask(AddServer {
                name: "live".into(),
                entry: serde_json::json!({"command": MISSING}),
            })
            .await;
        assert!(
            reply.is_ok(),
            "reconnect must reuse the saved entry: {reply:?}"
        );
        assert!(reply.unwrap().connected);
        assert_eq!(read_json(&path).unwrap(), saved);
        assert!(
            !root.0.join("mcp/live.json").exists(),
            "reconnect must not create a duplicate entry"
        );
    }

    /// The popover's switch: `set_mcp_gate(server, null, null)` + `mcp_start` starts an
    /// `off` server for every chat and waits for its tools; a chat's `mcp:<server>` hide
    /// still hides it from that chat (and its subagent) alone; `off` stops it and its
    /// tools leave every session. `mcp_start` writes nothing and refuses an `off` server.
    #[tokio::test]
    async fn mcp_live_switch_starts_and_stops_a_server_for_every_chat() {
        use crate::registry::{SetHidden, SetParent};
        let root = crate::TestDir::new("mcp-live-switch");
        std::fs::create_dir_all(root.0.join("mcp")).unwrap();
        let registry = crate::registry::tests::registry(Vec::new(), None);
        let mcp = Mcp::spawn(Mcp {
            state: State::default(),
            dir: root.0.join("mcp"),
            auth_dir: root.0.join("oauth"),
            registry: registry.clone(),
            policy: Policy::default(),
            rt: Runtime::new(figment::Figment::new(), root.0.clone()),
            machine: None,
            origin: None,
            paired: || None,
        });
        let entry = serde_json::json!({"command": "python3", "args": [
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mcp_live.py")
        ]})
        .to_string();
        std::fs::write(root.0.join("mcp/live.json"), &entry).unwrap();
        std::fs::write(
            root.0.join("mcp/config.yaml"),
            "servers:\n  live:\n    risk: off\n",
        )
        .unwrap();
        mcp.ask(Reconcile).await.unwrap();
        assert_eq!(listed(&mcp, "live").await.state, McpState::Off);
        let start = || {
            mcp.ask(StartServer {
                name: "live".into(),
            })
        };
        let off = start().await.unwrap_err().to_string();
        assert!(off.contains("live is off"), "{off}");

        let (chat, sub, other) = (
            runtime::SessionId::new(),
            runtime::SessionId::new(),
            runtime::SessionId::new(),
        );
        registry
            .ask(SetParent {
                session: sub,
                parent: chat,
            })
            .await
            .unwrap();
        let sees = |s| {
            let registry = registry.clone();
            async move {
                registry
                    .ask(ListTools(Some(s)))
                    .await
                    .unwrap()
                    .iter()
                    .any(|t| t.name == "live_echo")
            }
        };
        assert!(!sees(chat).await && !sees(other).await);

        // Switch on: clear the `off` line, then wait for the connect.
        let gate = |gate| SetGate {
            server: "live".into(),
            tool: None,
            gate,
        };
        mcp.ask(gate(None)).await.unwrap();
        start().await.unwrap();
        let s = listed(&mcp, "live").await;
        assert!(s.connected && s.state == McpState::Running, "{s:?}");
        for s in [chat, sub, other] {
            assert!(sees(s).await, "registered for every session");
        }
        start().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.0.join("mcp/live.json")).unwrap(),
            entry
        );

        // The per-chat checkbox still hides it in that chat (and its subagent) only.
        let hide = |hidden| SetHidden {
            session: chat,
            tools: vec![crate::registry::mcp_server_key("live")],
            hidden,
        };
        registry.ask(hide(true)).await.unwrap();
        assert!(!sees(chat).await && !sees(sub).await && sees(other).await);
        registry.ask(hide(false)).await.unwrap();
        assert!(sees(chat).await);

        // Switch off: the process stops and its tools leave every session.
        mcp.ask(gate(Some(Gate::Off))).await.unwrap();
        let s = listed(&mcp, "live").await;
        assert!(!s.connected && s.state == McpState::Off, "{s:?}");
        for s in [chat, sub, other] {
            assert!(!sees(s).await, "unregistered everywhere");
        }
        assert!(start().await.is_err());
    }

    fn engine(id: &str) -> Machine {
        Machine {
            id: id.into(),
            name: format!("box-{id}"),
        }
    }

    /// An `Mcp` actor over `<root>/mcp`, not started: tests drive `Reconcile`.
    fn actor(root: &Path, machine: Option<Machine>) -> ActorRef<Mcp> {
        actor_paired(root, machine, || None)
    }

    fn actor_paired(root: &Path, machine: Option<Machine>, paired: Paired) -> ActorRef<Mcp> {
        std::fs::create_dir_all(root.join("mcp")).unwrap();
        Mcp::spawn(Mcp {
            state: State::default(),
            dir: root.join("mcp"),
            auth_dir: root.join("oauth"),
            registry: crate::registry::tests::registry(Vec::new(), None),
            policy: Policy::default(),
            rt: Runtime::new(figment::Figment::new(), root.to_owned()),
            machine,
            origin: None,
            paired,
        })
    }

    /// `<root>/mcp/<name>.json`: a stdio server whose command is on no machine.
    fn missing(root: &Path, name: &str, args: &[&str]) {
        let entry = serde_json::json!({"command": MISSING, "args": args});
        std::fs::write(root.join(format!("mcp/{name}.json")), entry.to_string()).unwrap();
    }

    fn homes(root: &Path, yaml: &str) {
        std::fs::write(root.join("mcp").join(HOMES), yaml).unwrap();
    }

    async fn listed(mcp: &ActorRef<Mcp>, name: &str) -> McpServer {
        mcp.ask(ListServers::default())
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap()
    }

    /// Failed starts in a row (= start attempts while failing); `None` = not backing off.
    struct Failures(&'static str);

    impl Message<Failures> for Mcp {
        type Reply = Option<u32>;

        async fn handle(&mut self, m: Failures, _: &mut Context<Self, Self::Reply>) -> Option<u32> {
            self.state.retry.get(m.0).map(|r| r.failures)
        }
    }

    /// Until the connect task of a just-reconciled server settled.
    async fn settled(mcp: &ActorRef<Mcp>, name: &'static str) {
        for _ in 0..200 {
            if mcp.ask(Failures(name)).await.unwrap().is_some()
                || listed(mcp, name).await.state != McpState::Connecting
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{name} never settled");
    }

    /// Log lines written on this thread (a `#[tokio::test]` runtime's only one).
    #[derive(Clone, Default)]
    struct Lines(Arc<parking_lot::Mutex<Vec<u8>>>);

    impl std::io::Write for Lines {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Lines {
        type Writer = Lines;
        fn make_writer(&'a self) -> Lines {
            self.clone()
        }
    }

    impl Lines {
        fn count(&self, needle: &str) -> usize {
            String::from_utf8_lossy(&self.0.lock())
                .lines()
                .filter(|l| l.contains(needle))
                .count()
        }
    }

    fn capture() -> (Lines, tracing::subscriber::DefaultGuard) {
        let lines = Lines::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        (lines.clone(), tracing::subscriber::set_default(sub))
    }

    #[test]
    fn placement_backoff_and_who_may_claim() {
        let (a, b) = (engine("a"), engine("b"));
        let home = |engine: Option<&str>, everywhere: bool, off: &[&str]| Home {
            engine: engine.map(str::to_owned),
            name: None,
            everywhere,
            off: off.iter().map(|s| s.to_string()).collect(),
        };
        // `b` paired with `a` and `c`.
        let peers = ["a".to_owned(), "c".to_owned()];
        let p = Some(&peers[..]);
        let at = |h: Option<&Home>, me: Option<&Machine>| place(h, me, p);
        assert_eq!(
            at(Some(&home(Some("a"), false, &[])), Some(&a)),
            Place::Here
        );
        assert_eq!(
            at(Some(&home(Some("a"), false, &[])), Some(&b)),
            Place::Elsewhere
        );
        assert_eq!(at(Some(&home(Some("a"), true, &[])), Some(&b)), Place::Here);
        assert_eq!(
            at(Some(&home(Some("a"), true, &["b"])), Some(&b)),
            Place::OffHere
        );
        assert_eq!(at(Some(&home(None, true, &[])), Some(&b)), Place::Here);
        assert_eq!(at(None, Some(&a)), Place::Elsewhere, "unclaimed waits");
        assert_eq!(
            place(Some(&home(Some("b"), false, &[])), None, None),
            Place::Here,
            "no mesh"
        );
        // A home not paired here: only the lowest id among this engine and its peers runs it.
        let left = home(Some("x"), false, &[]);
        assert_eq!(
            at(Some(&left), Some(&b)),
            Place::Elsewhere,
            "a is lower than b"
        );
        let (b_alone, a_peers) = (Vec::<String>::new(), ["b".to_owned()]);
        assert_eq!(place(Some(&left), Some(&b), Some(&b_alone)), Place::Adopted);
        assert_eq!(place(Some(&left), Some(&a), Some(&a_peers)), Place::Adopted);
        assert_eq!(
            place(Some(&left), Some(&a), None),
            Place::Elsewhere,
            "paired set unknown"
        );
        assert!(Place::Adopted.runs() && Place::Here.runs() && !Place::Elsewhere.runs());

        let secs: Vec<u64> = (1..=10).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [30, 60, 120, 240, 480, 960, 1800, 1800, 1800, 1800]);
        assert_eq!(backoff(u32::MAX), RETRY_CAP);

        let (mine, theirs) = (replica::PeerId("ra".into()), replica::PeerId("rb".into()));
        assert!(
            written_here(None, Some(&mine)),
            "no row: the file appeared here"
        );
        let (row_mine, row_theirs) = (Some(mine.clone()), Some(theirs));
        assert!(written_here(Some(&row_mine), Some(&mine)));
        assert!(!written_here(Some(&row_theirs), Some(&mine)));
        assert!(!written_here(Some(&row_theirs), None));
        assert!(
            !written_here(Some(&None), Some(&mine)),
            "a row with no stamp is never ours"
        );
    }

    #[tokio::test]
    async fn add_stamps_this_engine_as_home_and_other_engines_list_it_unstarted() {
        let a_root = crate::TestDir::new("mcp-home-add-a");
        let a = actor(&a_root.0, Some(engine("a")));
        let spec = serve_mcp(TestServer::default()).await;
        let Transport::Http { url, .. } = &spec.transport else {
            unreachable!()
        };
        let entry = serde_json::json!({"url": url, "headers": {"Authorization": "Bearer t"}});
        let added = a
            .ask(AddServer {
                name: "remote".into(),
                entry: entry.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            (
                added.state,
                added.home.as_deref(),
                added.home_name.as_deref(),
                added.home_here,
                added.everywhere
            ),
            (McpState::Running, Some("a"), Some("box-a"), true, false)
        );
        assert_eq!(
            load_homes(&a_root.0.join("mcp")).unwrap().engine("remote"),
            Some("a")
        );
        // The entry itself stays as given: third-party readers see their own shape.
        let file: Value =
            serde_json::from_slice(&std::fs::read(a_root.0.join("mcp/remote.json")).unwrap())
                .unwrap();
        assert_eq!(file, entry);

        // Engine b holds the replicated files: listed with its home, not started, no tools.
        let b_root = crate::TestDir::new("mcp-home-add-b");
        let b = actor(&b_root.0, Some(engine("b")));
        for f in ["remote.json", HOMES] {
            std::fs::copy(a_root.0.join("mcp").join(f), b_root.0.join("mcp").join(f)).unwrap();
        }
        b.ask(Reconcile).await.unwrap();
        let s = listed(&b, "remote").await;
        assert_eq!(
            (s.state, s.home_name.as_deref(), s.home_here, s.tools.len()),
            (McpState::Elsewhere, Some("box-a"), false, 0)
        );
        assert_eq!(
            b.ask(Failures("remote")).await.unwrap(),
            None,
            "never started"
        );
    }

    #[tokio::test]
    async fn only_the_home_starts_a_server_unless_it_runs_everywhere() {
        let root = crate::TestDir::new("mcp-home-place");
        let mcp = actor(&root.0, Some(engine("a")));
        missing(&root.0, "gone", &[]);
        homes(
            &root.0,
            "servers:\n  gone:\n    engine: b\n    name: box-b\n",
        );
        mcp.ask(Reconcile).await.unwrap();
        let s = listed(&mcp, "gone").await;
        assert_eq!(
            (s.state, s.home_name.as_deref(), s.home_here),
            (McpState::Elsewhere, Some("box-b"), false)
        );
        assert_eq!(
            mcp.ask(Failures("gone")).await.unwrap(),
            None,
            "never started"
        );

        let set = async |action| {
            mcp.ask(SetMcpHome {
                server: "gone".into(),
                action,
            })
            .await
            .unwrap();
            settled(&mcp, "gone").await;
            listed(&mcp, "gone").await
        };
        let s = set(McpHomeAction::RunEverywhere).await;
        assert_eq!((s.state, s.everywhere), (McpState::UnavailableHere, true));
        assert_eq!(
            s.reason.as_deref(),
            Some(format!("{MISSING} is not installed on box-a").as_str())
        );
        assert_eq!(mcp.ask(Failures("gone")).await.unwrap(), Some(1));
        assert_eq!(set(McpHomeAction::OffHere).await.state, McpState::OffHere);
        assert_eq!(mcp.ask(Failures("gone")).await.unwrap(), None);
        assert_eq!(
            set(McpHomeAction::OnHere).await.state,
            McpState::UnavailableHere
        );
        assert_eq!(
            set(McpHomeAction::OnlyHome).await.state,
            McpState::Elsewhere
        );
        let s = set(McpHomeAction::MoveHere).await;
        assert_eq!(
            (s.state, s.home.as_deref(), s.home_here, s.everywhere),
            (McpState::UnavailableHere, Some("a"), true, false)
        );
        assert_eq!(
            load_homes(&root.0.join("mcp")).unwrap().servers["gone"],
            Home {
                engine: Some("a".into()),
                name: Some("box-a".into()),
                everywhere: false,
                off: Vec::new(),
            }
        );

        // No mesh: a lone engine runs everything and has no homes to set.
        let lone_root = crate::TestDir::new("mcp-home-lone");
        let lone = actor(&lone_root.0, None);
        missing(&lone_root.0, "gone", &[]);
        homes(&lone_root.0, "servers:\n  gone:\n    engine: b\n");
        lone.ask(Reconcile).await.unwrap();
        settled(&lone, "gone").await;
        assert_eq!(listed(&lone, "gone").await.state, McpState::UnavailableHere);
        assert!(
            lone.ask(SetMcpHome {
                server: "gone".into(),
                action: McpHomeAction::MoveHere
            })
            .await
            .is_err()
        );
    }

    /// Hand-written and pre-187 files: the engine that last wrote the file's
    /// replica row claims it (no row = it appeared here); a peer's file waits
    /// for that peer's claim. One `home is` line per server and change.
    #[tokio::test]
    async fn homeless_servers_are_claimed_by_their_last_writer() {
        use sqlx::ConnectOptions as _;
        let (lines, _log) = capture();
        let root = crate::TestDir::new("mcp-home-claim");
        let engine_a = replica::Engine::open(&root.0.join(replica::DB))
            .await
            .unwrap();
        let mut rows = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.0.join("mcp.sqlite"))
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE mcp (path TEXT PRIMARY KEY, bytes BLOB, mtime_ms INTEGER,
             deleted INTEGER NOT NULL DEFAULT 0, hlc TEXT)",
        )
        .execute(&mut rows)
        .await
        .unwrap();
        let row =
            async |rows: &mut sqlx::SqliteConnection, path: &str, origin: &replica::PeerId| {
                let hlc = replica::Hlc {
                    wall_ms: 1,
                    counter: 0,
                    origin: origin.clone(),
                };
                sqlx::query("INSERT OR REPLACE INTO mcp (path, hlc) VALUES (?, ?)")
                    .bind(path)
                    .bind(hlc.to_text())
                    .execute(rows)
                    .await
                    .unwrap();
            };
        let peer = replica::PeerId("replica-of-b".into());
        row(&mut rows, "mine.json", engine_a.origin()).await;
        row(&mut rows, "theirs.json", &peer).await;
        let mcp = actor(&root.0, Some(engine("a")));
        for name in ["local", "mine", "theirs"] {
            missing(&root.0, name, &[]);
        }
        mcp.ask(Reconcile).await.unwrap();
        mcp.ask(Reconcile).await.unwrap();
        let homes_now = load_homes(&root.0.join("mcp")).unwrap();
        assert_eq!(homes_now.engine("local"), Some("a"), "no row: written here");
        assert_eq!(homes_now.engine("mine"), Some("a"), "our row");
        assert_eq!(homes_now.engine("theirs"), None, "b claims its own");
        let s = listed(&mcp, "theirs").await;
        assert_eq!((s.state, s.home), (McpState::Elsewhere, None));
        assert_eq!(lines.count("mcp: local home is box-a"), 1);
        assert_eq!(lines.count("mcp: mine home is box-a"), 1);
        assert_eq!(lines.count("mcp: theirs home is not claimed yet"), 1);

        // b's claim replicates in; a later edit of `mine` on b does not move its home.
        let mut text = std::fs::read_to_string(root.0.join("mcp").join(HOMES)).unwrap();
        text.push_str("  theirs:\n    engine: b\n    name: box-b\n");
        homes(&root.0, &text);
        row(&mut rows, "mine.json", &peer).await;
        mcp.ask(Reconcile).await.unwrap();
        let homes_now = load_homes(&root.0.join("mcp")).unwrap();
        assert_eq!(homes_now.engine("theirs"), Some("b"));
        assert_eq!(homes_now.engine("mine"), Some("a"), "the first claim holds");
        assert_eq!(lines.count("mcp: theirs home is box-b"), 1);
        assert_eq!(lines.count("home is"), 4, "one line per server and change");
    }

    /// The xeus hot loop: a command missing here is `unavailable_here` after
    /// one start; reconciles (mirror writes, editors) start nothing; retries
    /// back off 30 s → 30 min; a changed entry retries at once; one log line.
    #[tokio::test(start_paused = true)]
    async fn missing_command_backs_off_without_a_hot_loop() {
        let (lines, _log) = capture();
        let root = crate::TestDir::new("mcp-home-enoent");
        let mcp = actor(&root.0, Some(engine("a")));
        missing(&root.0, "gone", &[]);
        homes(&root.0, "servers:\n  gone:\n    engine: a\n");
        let fails = async || mcp.ask(Failures("gone")).await.unwrap();
        let wait = |secs: u64| tokio::time::sleep(Duration::from_secs(secs));
        mcp.ask(Reconcile).await.unwrap();
        wait(1).await;
        assert_eq!(fails().await, Some(1));
        let s = listed(&mcp, "gone").await;
        assert_eq!(s.state, McpState::UnavailableHere);
        assert_eq!(
            s.reason,
            Some(format!("{MISSING} is not installed on box-a"))
        );
        for _ in 0..50 {
            mcp.tell(Reconcile).await.unwrap();
        }

        // One second past each due time; the next is due `delay` after the last.
        for (n, delay) in [30, 60, 120, 240, 480, 960, 1800, 1800, 1800]
            .into_iter()
            .enumerate()
        {
            wait(delay - 2).await;
            assert_eq!(
                fails().await,
                Some(n as u32 + 1),
                "early retry before {delay}s"
            );
            wait(2).await;
            assert_eq!(fails().await, Some(n as u32 + 2), "no retry after {delay}s");
        }

        // A changed entry starts at once, the count starts over.
        missing(&root.0, "gone", &["--v2"]);
        mcp.ask(Reconcile).await.unwrap();
        wait(1).await;
        assert_eq!(fails().await, Some(1));
        assert_eq!(
            lines.count("mcp server not available here"),
            1,
            "one line per state change, not per attempt"
        );
    }

    /// A hand-edit typo in `homes.yaml`: the last good homes stay in force,
    /// nothing is claimed or written over it, one line is logged, and the
    /// RPCs that would write it say why.
    #[tokio::test]
    async fn a_broken_homes_file_is_kept_and_never_written_over() {
        let (lines, _log) = capture();
        let root = crate::TestDir::new("mcp-home-broken");
        let mcp = actor(&root.0, Some(engine("a")));
        missing(&root.0, "gone", &[]);
        homes(
            &root.0,
            "servers:\n  gone:\n    engine: b\n    everywhere: true\n",
        );
        mcp.ask(Reconcile).await.unwrap();
        settled(&mcp, "gone").await;
        assert_eq!(listed(&mcp, "gone").await.state, McpState::UnavailableHere);

        let broken = "servers: [this is: not\n";
        homes(&root.0, broken);
        missing(&root.0, "fresh", &[]);
        for _ in 0..3 {
            mcp.ask(Reconcile).await.unwrap();
        }
        let on_disk = std::fs::read_to_string(root.0.join("mcp").join(HOMES)).unwrap();
        assert_eq!(on_disk, broken, "nothing claimed over it");
        let s = listed(&mcp, "gone").await;
        assert!(s.everywhere, "the last good homes stay in force");
        assert_eq!(listed(&mcp, "fresh").await.state, McpState::Elsewhere);
        let set = mcp
            .ask(SetMcpHome {
                server: "gone".into(),
                action: McpHomeAction::OnlyHome,
            })
            .await;
        assert!(format!("{set:?}").contains("is invalid"), "{set:?}");
        let add = mcp
            .ask(AddServer {
                name: "other".into(),
                entry: serde_json::json!({"command": MISSING}),
            })
            .await;
        assert!(format!("{add:?}").contains("is invalid"), "{add:?}");
        let rm = mcp
            .ask(RemoveServer {
                name: "fresh".into(),
            })
            .await;
        assert!(format!("{rm:?}").contains("is invalid"), "{rm:?}");
        assert!(root.0.join("mcp/fresh.json").exists(), "nothing removed");
        assert_eq!(
            std::fs::read_to_string(root.0.join("mcp").join(HOMES)).unwrap(),
            broken
        );
        assert_eq!(lines.count("homes kept as they were"), 1);
        let problem = mcp.ask(Progress).await.unwrap().homes_problem;
        assert!(
            problem.is_some_and(|p| p.contains("homes.yaml is invalid")),
            "shown over RPC"
        );

        // Fixed: read again, and the homeless server is claimed.
        homes(
            &root.0,
            "servers:\n  gone:\n    engine: b\n    everywhere: true\n",
        );
        mcp.ask(Reconcile).await.unwrap();
        assert_eq!(lines.count("mcp homes: readable again"), 1);
        assert_eq!(
            load_homes(&root.0.join("mcp")).unwrap().engine("fresh"),
            Some("a")
        );
    }

    /// An unreadable replica is never "no row": nothing is claimed until it reads.
    #[tokio::test]
    async fn an_unreadable_replica_claims_nothing() {
        let (lines, _log) = capture();
        let root = crate::TestDir::new("mcp-home-locked");
        let mcp = actor(&root.0, Some(engine("a")));
        std::fs::write(root.0.join("mcp.sqlite"), b"not a database").unwrap();
        missing(&root.0, "theirs", &[]);
        mcp.ask(Reconcile).await.unwrap();
        mcp.ask(Reconcile).await.unwrap();
        assert!(!root.0.join("mcp").join(HOMES).exists(), "nothing claimed");
        assert_eq!(listed(&mcp, "theirs").await.state, McpState::Elsewhere);
        assert_eq!(lines.count("mcp.sqlite unreadable"), 1);
        std::fs::remove_file(root.0.join("mcp.sqlite")).unwrap();
        mcp.ask(Reconcile).await.unwrap();
        assert_eq!(
            load_homes(&root.0.join("mcp")).unwrap().engine("theirs"),
            Some("a")
        );
    }

    static B_PAIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

    /// Engine `a`'s paired set: `b` while [`B_PAIRED`], else nobody.
    fn a_peers() -> Option<Vec<String>> {
        Some(if B_PAIRED.load(std::sync::atomic::Ordering::SeqCst) {
            vec!["b".to_owned()]
        } else {
            Vec::new()
        })
    }

    /// Sky's pair: mikrotik is homed on b. b unpaired: a runs it, writes
    /// nothing, says so once. b paired again: the home is b's as before.
    #[tokio::test]
    async fn an_unpaired_home_runs_here_unwritten_and_a_re_pair_restores_it() {
        let (lines, _log) = capture();
        let root = crate::TestDir::new("mcp-home-adopt");
        let mcp = actor_paired(&root.0, Some(engine("a")), a_peers);
        missing(&root.0, "mikrotik", &[]);
        let yaml = "servers:\n  mikrotik:\n    engine: b\n    name: box-b\n";
        homes(&root.0, yaml);
        let on_disk = || std::fs::read_to_string(root.0.join("mcp").join(HOMES)).unwrap();
        mcp.ask(Reconcile).await.unwrap();
        assert_eq!(listed(&mcp, "mikrotik").await.state, McpState::Elsewhere);

        B_PAIRED.store(false, std::sync::atomic::Ordering::SeqCst);
        mcp.ask(Reconcile).await.unwrap();
        settled(&mcp, "mikrotik").await;
        mcp.ask(Reconcile).await.unwrap();
        let s = listed(&mcp, "mikrotik").await;
        assert_eq!(
            (s.state, s.home.as_deref(), s.home_here),
            (McpState::UnavailableHere, Some("b"), false)
        );
        assert_eq!(on_disk(), yaml, "adopted at runtime only");
        assert_eq!(
            lines.count("mcp: mikrotik runs here while its home box-b is not paired"),
            1
        );

        B_PAIRED.store(true, std::sync::atomic::Ordering::SeqCst);
        mcp.ask(Reconcile).await.unwrap();
        let s = listed(&mcp, "mikrotik").await;
        assert_eq!(
            (s.state, s.home.as_deref()),
            (McpState::Elsewhere, Some("b"))
        );
        assert_eq!(
            mcp.ask(Failures("mikrotik")).await.unwrap(),
            None,
            "stopped here"
        );
        assert_eq!(on_disk(), yaml);
    }

    /// This engine's own lines of servers no file defines are dropped; a
    /// disabled entry keeps its line (and flags), a peer's line stays, and
    /// nothing is dropped while a file fails to parse.
    #[tokio::test]
    async fn own_lines_of_undefined_servers_are_dropped() {
        let root = crate::TestDir::new("mcp-home-prune");
        let mcp = actor(&root.0, Some(engine("a")));
        let entry =
            serde_json::json!({"mcpServers": {"paused": {"command": MISSING, "disabled": true}}});
        std::fs::write(root.0.join("mcp/paused.json"), entry.to_string()).unwrap();
        std::fs::write(root.0.join("mcp/broken.json"), "{ not json").unwrap();
        homes(
            &root.0,
            "servers:\n  stale:\n    engine: a\n  paused:\n    engine: a\n    everywhere: true\n  peers:\n    engine: b\n",
        );
        mcp.ask(Reconcile).await.unwrap();
        let h = load_homes(&root.0.join("mcp")).unwrap();
        assert_eq!(
            h.engine("stale"),
            Some("a"),
            "a file failed to parse: nothing dropped"
        );

        std::fs::remove_file(root.0.join("mcp/broken.json")).unwrap();
        mcp.ask(Reconcile).await.unwrap();
        let h = load_homes(&root.0.join("mcp")).unwrap();
        assert_eq!(
            (h.engine("stale"), h.engine("paused"), h.engine("peers")),
            (None, Some("a"), Some("b"))
        );
        assert!(
            h.servers["paused"].everywhere,
            "a disabled entry keeps its flags"
        );
    }
}
