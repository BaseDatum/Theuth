//! MCP client: every `*.json` in `<data_dir>/mcp/` holds one or more servers in
//! whatever layout its README used; each remote tool registers under its own
//! name (no prefix). The directory is watched and reconciled on change;
//! `<data_dir>/mcp/config.yaml` gates tools per server (design doc 01 §1.4).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kameo::actor::ActorRef;
use notify::Watcher;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use runtime::Runtime;
use serde::Deserialize;
use serde_json::Value;

use crate::registry::{ListTools, RegisterTool, ToolRegistry, UnregisterTool};
use crate::{ErasedTool, PolicyAction, Risk, ToolCx, ToolOutput, ToolSpec};

/// How to reach a server; the normalised form of one file entry.
#[derive(Clone, Debug, PartialEq)]
enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
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

/// Normalise one entry; `None` for disabled or malformed.
fn entry(v: &Value) -> Option<ServerSpec> {
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
    Some(ServerSpec {
        transport: Transport::Stdio { command, args, env },
    })
}

/// Structural detection: an entry is any object with `command` or `url`. Root
/// entry → one server named `stem`; else top-level entries and one wrapper
/// level (`mcpServers`, `mcp`, `servers`, …).
fn parse_file(stem: &str, json: &Value) -> Vec<(String, ServerSpec)> {
    if is_entry(json) {
        return entry(json)
            .map(|s| vec![(stem.to_owned(), s)])
            .unwrap_or_default();
    }
    let Some(root) = json.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, v) in root {
        if is_entry(v) {
            out.extend(entry(v).map(|s| (k.clone(), s)));
        } else if let Some(wrapper) = v.as_object() {
            for (k2, v2) in wrapper {
                if is_entry(v2) {
                    out.extend(entry(v2).map(|s| (k2.clone(), s)));
                }
            }
        }
    }
    out
}

/// `<data_dir>/mcp/config.yaml`: per-server and per-tool approval actions.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Gates {
    servers: HashMap<String, ServerGate>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct ServerGate {
    gate: Option<PolicyAction>,
    tools: HashMap<String, PolicyAction>,
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

    fn summary(&self, args: &Value) -> String {
        format!("{} {args}", self.spec.name)
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
}

async fn connect(spec: &ServerSpec) -> anyhow::Result<Service> {
    let service = match &spec.transport {
        Transport::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
            ().serve(TokioChildProcess::new(cmd)?).await?
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

/// Read every `*.json` in `dir`, sorted by path; first server name wins.
fn desired(dir: &Path) -> BTreeMap<String, ServerSpec> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    let mut out = BTreeMap::new();
    let mut owner: HashMap<String, PathBuf> = HashMap::new();
    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let json: Value = match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|t| Ok(serde_json::from_str(&t)?))
        {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "mcp file skipped");
                continue;
            }
        };
        for (name, spec) in parse_file(stem, &json) {
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
    for (name, spec) in want {
        if state.running.contains_key(&name) || state.gates.for_server(&name) == PolicyAction::Deny
        {
            continue;
        }
        match connect(&spec).await {
            Ok(service) => match service.list_all_tools().await {
                Ok(remote) => {
                    tracing::info!(server = %name, count = remote.len(), "mcp server connected");
                    changed = true;
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
                Err(e) => tracing::warn!(server = %name, error = %e, "mcp list_tools failed"),
            },
            Err(e) => tracing::warn!(server = %name, error = %e, "mcp connect failed"),
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

/// Initial reconcile of `<data_dir>/mcp`, then watch it for changes.
/// Returns after the first reconcile so the tools exist when the plugin
/// reports started.
pub async fn start(rt: &Runtime, registry: ActorRef<ToolRegistry>) -> anyhow::Result<()> {
    let dir = rt.data_dir.join("mcp");
    std::fs::create_dir_all(&dir)?;
    let natives = registry
        .ask(ListTools)
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    let mut state = State {
        natives,
        ..State::default()
    };
    reconcile(&mut state, &dir, &registry).await;

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
            reconcile(&mut state, &dir, &registry).await;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> ServerSpec {
        ServerSpec {
            transport: Transport::Stdio {
                command: command.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
        }
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
}
