//! Agent-side MCP source tools (design doc 04 §3.2): the model writes files
//! through the [`Mcp`] actor and the actor reconciles. Gates stay the human's.

use std::collections::BTreeMap;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{AddRepo, AddServer, Mcp, RemoveServer, SyncMcp};
use crate::{MCP_ACTOR, Risk, Tool, ToolCx, ToolOutput};

/// The actor, or the text the model reads when it is not running.
fn mcp() -> Result<ActorRef<Mcp>, &'static str> {
    ActorRef::<Mcp>::lookup(MCP_ACTOR)
        .ok()
        .flatten()
        .ok_or("mcp actor not running")
}

/// Handler `Err(String)` and transport failures both become model-readable text.
fn flat<M, T>(r: Result<T, SendError<M, String>>) -> Result<T, String> {
    match r {
        Ok(t) => Ok(t),
        Err(SendError::HandlerError(e)) => Err(e),
        Err(e) => Err(format!("mcp actor: {e}")),
    }
}

/// Arguments for [`AddMcpServer`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddMcpServerArgs {
    /// Server name, `[a-z0-9][a-z0-9._-]*`; becomes `mcp/<name>.json`.
    pub name: String,
    /// Executable for a stdio server (e.g. `npx`, `node`, `./target/release/x`).
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for `command`.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Extra environment for the child.
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    /// Working directory; relative paths resolve against the `mcp/` dir, so a
    /// cloned repo is `repos/<name>`.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Streamable-HTTP endpoint for a remote server (instead of `command`).
    #[serde(default)]
    pub url: Option<String>,
    /// Headers for `url`.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
}

/// Writes one server file and connects it.
pub struct AddMcpServer;

#[async_trait::async_trait]
impl Tool for AddMcpServer {
    const NAME: &'static str = "add_mcp_server";
    // Makes the harness spawn a model-chosen executable on every start.
    const RISK: Risk = Risk::Destructive;
    type Args = AddMcpServerArgs;

    fn promotable(&self) -> bool {
        // Connecting already has a 20-second bound; a job stub would send the
        // caller into its next Python cell before the advertised tools exist.
        false
    }

    fn description(&self) -> String {
        "Register an MCP server: give `command` (+ `args`, `env`, `cwd`) for a local one or `url` (+ `headers`) for a remote one. \
         Writes mcp/<name>.json, connects (up to 20 seconds), and lists the tools now available. \
         An existing name reconnects its saved entry without rewriting it. \
         For a desk or its child, grants this server in that agent's mcp: list; other agents are untouched. \
         For a cloned repo (add_mcp_repo), build it with the shell tool first and pass cwd: \"repos/<name>\"."
            .into()
    }

    async fn call(&self, a: AddMcpServerArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let mcp = match mcp() {
            Ok(m) => m,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        if a.command.is_some() == a.url.is_some() {
            return Ok(ToolOutput::err("give exactly one of `command` or `url`"));
        }
        let mut entry = Map::new();
        let fields: [(&str, Option<Value>); 6] = [
            ("command", a.command.map(Value::from)),
            ("args", a.args.map(Value::from)),
            (
                "env",
                a.env.map(|m| serde_json::to_value(m).unwrap_or_default()),
            ),
            ("cwd", a.cwd.map(Value::from)),
            ("url", a.url.map(Value::from)),
            (
                "headers",
                a.headers
                    .map(|m| serde_json::to_value(m).unwrap_or_default()),
            ),
        ];
        for (k, v) in fields {
            if let Some(v) = v {
                entry.insert(k.to_owned(), v);
            }
        }
        let name = a.name;
        Ok(
            match flat(
                mcp.ask(AddServer {
                    name,
                    entry: Value::Object(entry),
                })
                .await,
            ) {
                Ok(s) => {
                    let registry = ActorRef::<crate::ToolRegistry>::lookup(crate::REGISTRY_ACTOR)?
                        .ok_or_else(|| anyhow::anyhow!("tool registry not running"))?;
                    if let Some(access) = registry.ask(crate::registry::GetMcpAccess).await?
                        && let Err(e) = access.grant(cx.session, &s.name).await
                    {
                        return Ok(ToolOutput::err(format!(
                            "`{}` connected, but agent MCP access could not be granted: {e}",
                            s.name
                        )));
                    }
                    let visible = registry.ask(crate::ListTools(Some(cx.session))).await?;
                    let tools: Vec<&str> = s
                        .tools
                        .iter()
                        .filter_map(|t| t.registered.as_deref())
                        .filter(|name| visible.iter().any(|s| s.name == *name))
                        .collect();
                    ToolOutput::ok(if tools.is_empty() {
                        format!(
                            "`{}` connected; no tools are enabled for this session.",
                            s.name
                        )
                    } else {
                        format!(
                            "`{}` connected; tools available now: {}",
                            s.name,
                            tools.join(", ")
                        )
                    })
                }
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// Arguments for [`RemoveMcpServer`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RemoveMcpServerArgs {
    /// Server name as listed by the harness.
    pub name: String,
}

/// Deletes or disables one server entry.
pub struct RemoveMcpServer;

#[async_trait::async_trait]
impl Tool for RemoveMcpServer {
    const NAME: &'static str = "remove_mcp_server";
    const RISK: Risk = Risk::Destructive;
    type Args = RemoveMcpServerArgs;

    fn description(&self) -> String {
        "Remove an MCP server: deletes mcp/<name>.json when it holds only that server, otherwise marks the entry disabled. \
         Its tools disappear immediately."
            .into()
    }

    async fn call(&self, a: RemoveMcpServerArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let mcp = match mcp() {
            Ok(m) => m,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        Ok(
            match flat(
                mcp.ask(RemoveServer {
                    name: a.name.clone(),
                })
                .await,
            ) {
                Ok(()) => ToolOutput::ok(format!("`{}` removed.", a.name)),
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// Arguments for [`AddMcpRepo`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddMcpRepoArgs {
    /// Anything `git clone` accepts.
    pub url: String,
    /// Directory name under mcp/repos/; defaults to the repo's basename.
    #[serde(default)]
    pub name: Option<String>,
}

/// Clones an MCP server's source.
pub struct AddMcpRepo;

#[async_trait::async_trait]
impl Tool for AddMcpRepo {
    const NAME: &'static str = "add_mcp_repo";
    const RISK: Risk = Risk::Mutating;
    type Args = AddMcpRepoArgs;

    fn description(&self) -> String {
        "Clone an MCP server repository into mcp/repos/<name> (shallow). Returns the path and the head of its README \
         so you can plan the build; then build it with the shell tool and register it with add_mcp_server (cwd: \"repos/<name>\")."
            .into()
    }

    async fn call(&self, a: AddMcpRepoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let mcp = match mcp() {
            Ok(m) => m,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        Ok(
            match flat(
                mcp.ask(AddRepo {
                    url: a.url,
                    name: a.name,
                })
                .await,
            ) {
                Ok(r) => {
                    let readme = if r.readme.is_empty() {
                        "(no README)".to_owned()
                    } else {
                        format!("README (first lines):\n{}", r.readme)
                    };
                    ToolOutput::ok(format!(
                        "Cloned `{name}` to {path}.\n\n{readme}\n\n\
                         Next: build it with the shell tool as the README says, then call add_mcp_server with \
                         cwd: \"repos/{name}\" and the command that starts it.",
                        name = r.name,
                        path = r.path
                    ))
                }
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// Arguments for [`SyncMcpRepos`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SyncMcpArgs {}

/// Pulls every cloned server repo and restarts what changed.
pub struct SyncMcpRepos;

#[async_trait::async_trait]
impl Tool for SyncMcpRepos {
    const NAME: &'static str = "sync_mcp";
    const RISK: Risk = Risk::Mutating;
    type Args = SyncMcpArgs;

    fn description(&self) -> String {
        "git pull --ff-only every repo under mcp/repos/ and restart the servers that run from an updated one. \
         Returns one line per repo: updated, unchanged, or the error."
            .into()
    }

    async fn call(&self, _: SyncMcpArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let mcp = match mcp() {
            Ok(m) => m,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let results = match mcp.ask(SyncMcp).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("mcp actor: {e}"))),
        };
        Ok(ToolOutput::ok(if results.is_empty() {
            "No repos under mcp/repos/.".to_owned()
        } else {
            results
                .iter()
                .map(|r| format!("{}: {}", r.name, r.status))
                .collect::<Vec<_>>()
                .join("\n")
        }))
    }
}
