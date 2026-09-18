//! Peer tools (design docs 01 §2.4, 50 §5): `spawn_subagent` returns
//! immediately; `send_message` routes a bounded body to a peer address and
//! returns a durable [`PeerReceipt`]; `discover_peers` is the permitted
//! directory; `peer_message` is the caller's own record of one exchange;
//! `list_models` shows what a parent may pick for a child.
//!
//! The registry gates `discover_peers`/`send_message`/`spawn_subagent` on
//! the agent-owned [`crate::communication::SharedPolicy`] (50 §2.2) and runs
//! their bodies under [`tools::communication::scope`]; `send_message` hands
//! the generation token it was admitted under to [`Deliver`], so an off→on
//! between approval and dispatch cannot revive it (50 §8).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::communication::{Disabled, refused};
use tools::{Risk, Tool, ToolCx, ToolOutput};
use uuid::Uuid;

use crate::communication::{
    DISCOVER_DEFAULT_LIMIT, DISCOVER_MAX_LIMIT, GetPeerMessage, ListPeers, PeerMessageState,
    PeerReceipt, PeerRecord,
};
use crate::supervisor::{Deliver, DropWait, SpawnChild};
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
    const DIRECT: bool = true;
    type Args = SpawnSubagentArgs;

    fn description(&self) -> String {
        let mut s = String::from(
            "Delegate a complete, self-contained task to a subagent running in its own session while you continue. \
             Returns immediately with its name and address; its final message arrives later as a <message from=\"session:<id>\"> input (when communication is on), so do not poll. \
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
                    "Spawned `{name}` (session:{id}). Its result will arrive as a <message from=\"session:{id}\"> when it finishes; message it with send_message to `{name}` or `session:{id}`."
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
    /// Canonical address from discover_peers (`session:<uuid>` or `peer:<name>`), `parent`, or one of your subagents' names.
    pub to: String,
    /// Body; at most 32 KiB.
    pub message: String,
    /// Block until the addressee answers with reply_to = this message's id (bounded by a timeout).
    #[serde(default)]
    pub await_reply: bool,
    /// Id of the `<message>` you are answering; the exact message id, never a name.
    #[serde(default)]
    pub reply_to: Option<Uuid>,
    /// Your idempotency key: repeating a call with the same request_id, addressee and body returns the original receipt instead of sending again.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// Symmetric peer messaging.
pub struct SendMessageTool {
    /// For `reply_timeout_secs`.
    pub cfg: Arc<AgentConfig>,
}

/// Receipt as text plus `structured`; `is_error` for a terminal failure.
fn receipt_output(lead: &str, r: &PeerReceipt) -> ToolOutput {
    let mut s = format!("{lead}id={} to={} state={}", r.id, r.to, r.state.as_str());
    if let Some(t) = &r.task_id {
        s.push_str(&format!(" task={t}"));
    }
    if let Some(e) = &r.error {
        s.push_str(&format!(" error={e}"));
    }
    let mut out = if matches!(r.state, PeerMessageState::Failed) {
        ToolOutput::err(s)
    } else {
        ToolOutput::ok(s)
    };
    out.structured = serde_json::to_value(r).ok();
    out
}

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    const NAME: &'static str = "send_message";
    const RISK: Risk = Risk::Mutating;
    const DIRECT: bool = true;
    const RETURNS: &'static str = "dict";
    type Args = SendMessageArgs;

    fn description(&self) -> String {
        "Send a message to a peer: a canonical address from discover_peers (`session:<uuid>`, `peer:<name>`), \
         `parent` if you are a subagent, or one of your subagents by name. Returns a receipt: `accepted` means \
         committed to their intake, not read. With await_reply=true you block until they answer with reply_to set \
         to this message's id (or the timeout passes; the message is not recalled). When answering a <message> \
         with reply_expected=\"true\", set reply_to to its id. peer_message shows a message's current state."
            .into()
    }

    async fn call(&self, args: SendMessageArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        // The admission the registry dispatched this body under (50 §8): no
        // admission means a bypassed gate, and a revoked one is refused here
        // instead of re-reading the switch.
        let Some(admitted) = tools::communication::current() else {
            return Ok(refused(Self::NAME, Disabled::Uninstalled));
        };
        if !admitted.live() {
            return Ok(refused(Self::NAME, Disabled::Revoked));
        }
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let delivered = match agents
            .ask(Deliver {
                from: cx.session,
                to: args.to,
                text: args.message,
                await_reply: args.await_reply,
                reply_to: args.reply_to,
                request_id: args.request_id,
                admitted: admitted.token().clone(),
            })
            .await
        {
            Ok(d) => d,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        let Some(rx) = delivered.reply else {
            return Ok(receipt_output("", &delivered.receipt));
        };
        let id = delivered.receipt.id;
        let secs = self.cfg.subagents.reply_timeout_secs;
        Ok(
            match tokio::time::timeout(Duration::from_secs(secs), rx).await {
                Ok(Ok(r)) => match (&r.state, &r.reply) {
                    (PeerMessageState::Replied, Some(text)) => {
                        let mut out = ToolOutput::ok(text.clone());
                        out.structured = serde_json::to_value(&r).ok();
                        out
                    }
                    // Expired by the supervisor: disable, cancel, deletion.
                    _ => receipt_output("no reply: ", &r),
                },
                Ok(Err(_)) => receipt_output("wait dropped: ", &delivered.receipt),
                Err(_) => {
                    // Timeout: the waiter is removed, the accepted message stands.
                    let _ = agents.tell(DropWait(id)).await;
                    let mut r = delivered.receipt;
                    r.error = Some(format!(
                        "no reply within {secs}s; the message was not recalled"
                    ));
                    receipt_output("timeout: ", &r)
                }
            },
        )
    }
}

/// Arguments for [`DiscoverPeersTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DiscoverPeersArgs {
    /// Substring filter on name or address.
    #[serde(default)]
    pub query: Option<String>,
    /// Cursor from a previous page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size, default 25, max 100.
    #[serde(default)]
    pub limit: Option<u16>,
}

/// The permitted peer directory (50 §5.1).
pub struct DiscoverPeersTool;

#[async_trait::async_trait]
impl Tool for DiscoverPeersTool {
    const NAME: &'static str = "discover_peers";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = DiscoverPeersArgs;

    fn description(&self) -> String {
        format!(
            "List the peers you may message: parent, children, siblings, other sessions of the same tree, \
             other owner roots and configured remote agents. Each row has the canonical address for send_message, \
             a short name, kind, relationship, coarse state and capabilities. Default {DISCOVER_DEFAULT_LIMIT} rows, \
             max {DISCOVER_MAX_LIMIT}; pass `cursor` for the next page. Listing wakes nobody and probes no network."
        )
    }

    async fn call(&self, args: DiscoverPeersArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let page = match agents
            .ask(ListPeers {
                session: cx.session,
                query: args.query,
                cursor: args.cursor,
                limit: args.limit,
            })
            .await
        {
            Ok(p) => p,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        let mut text = if page.peers.is_empty() {
            "no peers".to_owned()
        } else {
            page.peers
                .iter()
                .map(|p| {
                    format!(
                        "{} — {} ({:?}, {:?}, {:?}{}{})",
                        p.address,
                        p.name,
                        p.kind,
                        p.relationship,
                        p.state,
                        if p.reply { ", replies" } else { "" },
                        if p.stream { ", streams" } else { "" },
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        if let Some(c) = &page.cursor {
            text.push_str(&format!("\nmore: cursor={c}"));
        }
        let mut out = ToolOutput::ok(text);
        out.structured = serde_json::to_value(&page).ok();
        Ok(out)
    }
}

/// Arguments for [`PeerMessageTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeerMessageArgs {
    /// Message id from a send_message receipt or a received <message>.
    pub id: Uuid,
}

/// The caller's own record of one exchange (50 §5.2). Not gated: an agent
/// may learn that its accepted message was suppressed after a disable.
pub struct PeerMessageTool;

#[async_trait::async_trait]
impl Tool for PeerMessageTool {
    const NAME: &'static str = "peer_message";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = PeerMessageArgs;

    fn description(&self) -> String {
        "Inspect a peer message you sent or received: current state (accepted, consumed, replied, suppressed, \
         failed, unknown), reply text when replied, task handle, error. Only your own exchanges; anything else is unknown."
            .into()
    }

    async fn call(&self, args: PeerMessageArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(ToolOutput::err(out)),
        };
        let rec: PeerRecord = match agents
            .ask(GetPeerMessage {
                session: cx.session,
                id: args.id,
            })
            .await
        {
            Ok(r) => r,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        let r = &rec.receipt;
        let mut text = format!(
            "id={} from={} to={} state={} at={}",
            r.id,
            rec.from,
            r.to,
            r.state.as_str(),
            rec.created_at.to_rfc3339()
        );
        if let Some(rt) = r.reply_to {
            text.push_str(&format!(" reply_to={rt}"));
        }
        if let Some(t) = &r.task_id {
            text.push_str(&format!(" task={t}"));
        }
        if let Some(e) = &r.error {
            text.push_str(&format!(" error={e}"));
        }
        text.push_str(&format!("\n{}", rec.text));
        if let Some(reply) = &r.reply {
            text.push_str(&format!("\n--- reply ---\n{reply}"));
        }
        let mut out = ToolOutput::ok(text);
        out.structured = serde_json::to_value(&rec).ok();
        Ok(out)
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
