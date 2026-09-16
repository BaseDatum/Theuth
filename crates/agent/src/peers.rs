//! Peer tools (design doc 01 §2.4): `spawn_subagent` returns immediately;
//! `send_message` routes text between a session and its parent/children;
//! `list_models` shows what a parent may pick for a child.
//! All are `Risk::ReadOnly`: nothing outside the harness changes.
//!
//! ponytail: the registry is global, so both tools are visible to every
//! session and depth/peer availability is enforced at call time; per-session
//! tool visibility is the upgrade path.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::supervisor::{Deliver, SpawnChild};
use crate::{AGENT_ACTOR, AgentConfig, Agents, ListModels};

/// Arguments for [`SpawnSubagentTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnSubagentArgs {
    /// Complete, self-contained instructions; the subagent sees nothing of this conversation.
    pub task: String,
    /// Stable peer name (used to address it with send_message); generated when omitted.
    #[serde(default)]
    pub name: Option<String>,
    /// An external harness from ~/.wth/acp/agents.yaml or a node (another machine running wth); default: a wth subagent.
    #[serde(default)]
    pub agent: Option<String>,
    /// Absolute working directory for an external harness; default: wth's cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Model for a wth subagent as `provider/model` (see list_models); default: the configured subagent model. Not for external harnesses.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort for a wth subagent: low|medium|high|off|default or a token budget; default: the configured one.
    #[serde(default)]
    pub effort: Option<String>,
}

/// Registered nodes (32 §6.5): name → (`describe` line, dialer). Written by
/// `Agents` on `RegisterNode`/`UnregisterNode`, read live by the tool
/// description, so the model sees a node the moment it is registered.
pub(crate) type Nodes = Arc<RwLock<BTreeMap<String, (String, Arc<dyn acp::Dialer>)>>>;

/// Non-blocking delegation to a child session.
pub struct SpawnSubagentTool {
    /// For `acp/agents.yaml`: the harness names the description offers.
    pub data_dir: PathBuf,
    /// The nodes the description offers.
    pub nodes: Nodes,
}

/// The configured harnesses, empty (and logged) when the file is unreadable.
fn harnesses(data_dir: &std::path::Path) -> BTreeMap<String, acp::Entry> {
    acp::load(data_dir).unwrap_or_else(|e| {
        tracing::warn!("acp/agents.yaml: {e:#}");
        Default::default()
    })
}

/// A snapshot of the registered nodes' `describe` lines, by name.
pub(crate) fn node_lines(nodes: &Nodes) -> BTreeMap<String, String> {
    nodes
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|(n, (d, _))| (n.clone(), d.clone()))
        .collect()
}

/// The tool description's node line (32 §7); empty with no nodes.
pub(crate) fn describe_nodes(nodes: &BTreeMap<String, String>) -> String {
    if nodes.is_empty() {
        return String::new();
    }
    let items: Vec<String> = nodes.iter().map(|(n, d)| format!("{n} — {d}")).collect();
    format!(
        "Nodes (wth on other machines, same tools, their own cwd and credentials): {}",
        items.join("; ")
    )
}

/// [`acp::unknown`] plus the registered node names, when there are any.
pub(crate) fn unknown(
    name: &str,
    entries: &BTreeMap<String, acp::Entry>,
    nodes: &BTreeMap<String, String>,
) -> String {
    let mut s = acp::unknown(name, entries);
    if !nodes.is_empty() {
        let names: Vec<&str> = nodes.keys().map(String::as_str).collect();
        s.push_str("; nodes: ");
        s.push_str(&names.join(", "));
    }
    s
}

/// The supervisor, or the text the model reads when it is not running.
pub(crate) fn supervisor() -> Result<ActorRef<Agents>, &'static str> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent supervisor not running")
}

#[async_trait::async_trait]
impl Tool for SpawnSubagentTool {
    const NAME: &'static str = "spawn_subagent";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SpawnSubagentArgs;

    fn description(&self) -> String {
        let mut s = String::from(
            "Delegate a complete, self-contained task to a subagent running in its own session while you continue. \
             Returns immediately with its name; its final message arrives later as a <subagent_result> input, so do not poll. \
             Several calls in one message run concurrently. Message it with send_message. \
             For a wth subagent you choose `model` and `effort`; pick a little more capability than the task seems to need.",
        );
        let line = acp::describe(&harnesses(&self.data_dir));
        if !line.is_empty() {
            s.push(' ');
            s.push_str(&line);
        }
        let line = describe_nodes(&node_lines(&self.nodes));
        if !line.is_empty() {
            s.push(' ');
            s.push_str(&line);
        }
        s
    }

    async fn call(&self, args: SpawnSubagentArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        if let Some(agent) = &args.agent {
            let known = harnesses(&self.data_dir);
            let nodes = node_lines(&self.nodes);
            if !known.contains_key(agent) && !nodes.contains_key(agent) {
                return Ok(ToolOutput::err(unknown(agent, &known, &nodes)));
            }
        }
        Ok(
            match agents
                .ask(SpawnChild {
                    parent: cx.session,
                    name: args.name,
                    task: args.task,
                    agent: args.agent,
                    cwd: args.cwd,
                    model: args.model,
                    effort: args.effort,
                })
                .await
            {
                Ok((name, id)) => ToolOutput::ok(format!(
                    "Spawned `{name}` (session {id}). Its result will arrive as a <subagent_result> when it finishes; message it with send_message."
                )),
                Err(SendError::HandlerError(e)) => ToolOutput::err(e),
                Err(e) => ToolOutput::err(format!("agent supervisor: {e}")),
            },
        )
    }
}

/// Arguments for [`SendMessageTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageArgs {
    /// `parent` (when you are a subagent) or the name of one of your subagents.
    pub to: String,
    /// Body.
    pub message: String,
    /// Block until the addressee's next message to you (bounded by a timeout).
    #[serde(default)]
    pub await_reply: bool,
    /// Id of the `<message>` you are answering.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Symmetric peer messaging.
pub struct SendMessageTool {
    /// For `reply_timeout_secs`.
    pub cfg: Arc<AgentConfig>,
}

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    const NAME: &'static str = "send_message";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SendMessageArgs;

    fn description(&self) -> String {
        "Send a message to a peer: `parent` if you are a subagent, or one of your subagents by name. \
         The text is injected into their conversation. With await_reply=true you block until they message you back \
         (or the timeout passes). When answering a <message> that has reply_expected=\"true\", set reply_to to its id."
            .into()
    }

    async fn call(&self, args: SendMessageArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let to = args.to;
        let rx = match agents
            .ask(Deliver {
                from: cx.session,
                to: to.clone(),
                text: args.message,
                await_reply: args.await_reply,
                reply_to: args.reply_to,
            })
            .await
        {
            Ok(rx) => rx,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        let Some(rx) = rx else {
            return Ok(ToolOutput::ok("delivered"));
        };
        let secs = self.cfg.subagents.reply_timeout_secs;
        Ok(
            match tokio::time::timeout(Duration::from_secs(secs), rx).await {
                Ok(Ok(reply)) => ToolOutput::ok(reply),
                _ => ToolOutput::ok(format!("no reply from {to} within {secs}s")),
            },
        )
    }
}

/// Arguments for [`ListModelsTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListModelsArgs {}

/// The models a parent may give a wth subagent.
pub struct ListModelsTool;

#[async_trait::async_trait]
impl Tool for ListModelsTool {
    const NAME: &'static str = "list_models";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListModelsArgs;

    fn description(&self) -> String {
        "List the models available to this harness (providers with credentials), one per line: \
         `provider/model · context · reasoning efforts · vision`. Use an id as spawn_subagent's `model`."
            .into()
    }

    async fn call(&self, _: ListModelsArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let models = agents
            .ask(ListModels)
            .await
            .map_err(|e| anyhow::anyhow!("agent supervisor: {e}"))?;
        if models.is_empty() {
            return Ok(ToolOutput::ok("no providers have credentials"));
        }
        let lines: Vec<String> = models
            .iter()
            .map(|m| {
                let mut line = m.id.clone();
                if m.context > 0 {
                    line.push_str(&format!(" · {}k", m.context / 1000));
                }
                if m.reasoning {
                    line.push_str(" · reasoning");
                    if !m.efforts.is_empty() {
                        line.push(' ');
                        line.push_str(&m.efforts.join("|"));
                    }
                }
                if m.image {
                    line.push_str(" · vision");
                }
                line
            })
            .collect();
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialer for the description test: never dialled.
    #[derive(Debug)]
    struct NoDial;

    #[async_trait::async_trait]
    impl acp::Dialer for NoDial {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            Err("not dialled".to_owned())
        }
    }

    #[test]
    fn spawn_description_lists_configured_harnesses() {
        let dir = std::env::temp_dir().join(format!("wth-peers-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = SpawnSubagentTool {
            data_dir: dir.clone(),
            nodes: Nodes::default(),
        }
        .description();
        assert!(plain.contains("none configured"));
        assert!(!plain.contains("Nodes ("));

        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "codex".to_string(),
            acp::Entry {
                command: "codex-acp".into(),
                args: Vec::new(),
                env: Default::default(),
                description: Some("ChatGPT's coding agent".into()),
            },
        );
        acp::save(&dir, &entries).unwrap();
        let with = SpawnSubagentTool {
            data_dir: dir.clone(),
            nodes: Nodes::default(),
        }
        .description();
        assert!(with.contains("External harnesses: codex"));
        assert!(with.contains("ChatGPT's coding agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 32 §7: a registered node is offered after the harness line, sorted by
    /// name, and an unknown name's error lists it.
    #[test]
    fn spawn_description_and_unknown_list_nodes() {
        let dir = std::env::temp_dir().join(format!("wth-peers-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let nodes = Nodes::default();
        {
            let mut w = nodes.write().unwrap_or_else(|p| p.into_inner());
            w.insert(
                "mac-mini".into(),
                (
                    "macos · offline".into(),
                    Arc::new(NoDial) as Arc<dyn acp::Dialer>,
                ),
            );
            w.insert(
                "build-box".into(),
                (
                    "linux · wth 0.4.1".into(),
                    Arc::new(NoDial) as Arc<dyn acp::Dialer>,
                ),
            );
        }
        let desc = SpawnSubagentTool {
            data_dir: dir.clone(),
            nodes: nodes.clone(),
        }
        .description();
        let line = "Nodes (wth on other machines, same tools, their own cwd and credentials): \
                    build-box — linux · wth 0.4.1; mac-mini — macos · offline";
        assert!(desc.contains(line), "{desc}");
        assert!(desc.find("External harnesses").unwrap() < desc.find("Nodes (").unwrap());

        let err = unknown("box", &harnesses(&dir), &node_lines(&nodes));
        assert!(err.starts_with("unknown agent `box`"), "{err}");
        assert!(err.ends_with("nodes: build-box, mac-mini"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
