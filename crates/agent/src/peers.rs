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

use std::collections::{BTreeMap, HashSet};
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
use crate::supervisor::{Deliver, DropWait, Resolve, SpawnChild};
use crate::{AGENT_ACTOR, AgentConfig, Agents, ListModels};

/// Arguments for [`SpawnSubagentTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnSubagentArgs {
    /// Complete, self-contained instructions; the subagent sees nothing of this conversation.
    pub task: String,
    /// Stable peer name (used to address it with send_message); generated when omitted.
    #[serde(default)]
    pub name: Option<String>,
    /// An external harness from ~/.theuth/acp/agents.yaml, a node (another machine running theuth), or `<harness>@<node>` (that node's own external harness); default: a theuth subagent.
    #[serde(default)]
    pub agent: Option<String>,
    /// Absolute working directory for an external harness; default: theuth's cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Model as `provider/model` (see list_models); default: the user's preferred subagent model (named in your system prompt when set), else the session model. On a node it is resolved there; an external harness switches to it when it offers a model option.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort: low|medium|high|off|default or a token budget; default: the configured one. On an external harness it is applied when the harness offers that effort value (e.g. omp: low|medium|high|xhigh|max; opencode: the model's variants), else the harness keeps its default.
    #[serde(default)]
    pub effort: Option<String>,
    /// Risk ceiling: read_only | mutating | destructive (default: no ceiling). The child and its descendants only see and may only run tools at or below this risk — enforced for theuth children here and on a node; an external harness only reads it as a first line of its brief.
    #[serde(default)]
    pub tools: Option<String>,
    /// Skills from ~/.theuth/skills (by name) written into an external harness's cwd for it to load, on top of the `always` ones; removed when it stops. Only for an external harness — a theuth subagent has `load_skill()`.
    #[serde(default)]
    pub skills: Vec<String>,
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

/// The tool description's node line (32 §7, 67 §8); empty with no nodes.
pub(crate) fn describe_nodes(nodes: &BTreeMap<String, String>) -> String {
    if nodes.is_empty() {
        return String::new();
    }
    let items: Vec<String> = nodes.iter().map(|(n, d)| format!("{n} — {d}")).collect();
    format!(
        "Nodes (theuth on other machines, same tools, their own cwd and credentials; `agent: \"<harness>@<node>\"` runs that node's own external harness, e.g. `omp@{}`): {}",
        nodes.keys().next().map_or("build-A", String::as_str),
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

/// `agent` as `[<harness>@]<engine>` (67 §8): `"build-A"` → `(None,
/// "build-A")`, a node or a local harness by that name; `"omp@build-A"` →
/// `(Some("omp"), "build-A")`, the node's own harness `omp`. Empty parts and
/// a second `@` are the error the model reads.
pub(crate) fn parse_agent(s: &str) -> Result<(Option<&str>, &str), String> {
    let bad = || format!("`agent`: `{s}` is not `<name>` or `<harness>@<node>`");
    match s.rsplit_once('@') {
        None if s.is_empty() => Err(bad()),
        None => Ok((None, s)),
        Some((h, e)) if h.is_empty() || e.is_empty() || h.contains('@') => Err(bad()),
        Some((h, e)) => Ok((Some(h), e)),
    }
}

/// The registered node `engine` names (67 §8): by name, else the one node
/// whose mesh id starts with it ([`acp::Dialer::engine_id`]; a 32 node has
/// none). Two id matches are no match.
pub(crate) fn resolve_node(nodes: &Nodes, engine: &str) -> Option<String> {
    let nodes = nodes.read().unwrap_or_else(|p| p.into_inner());
    if nodes.contains_key(engine) {
        return Some(engine.to_owned());
    }
    let mut hits = nodes
        .iter()
        .filter(|(_, (_, d))| d.engine_id().is_some_and(|id| id.starts_with(engine)))
        .map(|(n, _)| n.clone());
    match (hits.next(), hits.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// `agent` with its engine part replaced by the registered node name
/// ([`resolve_node`]); unchanged when it names no node (a local harness).
/// `None`: the engine is *this* one (its `mesh.name`, hostname by default, as
/// the mesh's peers and a task's `where` name it) and no node — a local child,
/// native or the named local harness (`omp@<this engine>` → `omp`).
pub(crate) fn canonical_agent(nodes: &Nodes, agent: &str) -> Option<String> {
    match parse_agent(agent) {
        Ok((harness, engine)) => match (harness, resolve_node(nodes, engine)) {
            (Some(h), Some(node)) => Some(format!("{h}@{node}")),
            (None, Some(node)) => Some(node),
            (h, None) if engine == mesh::self_name() => h.map(str::to_owned),
            (_, None) => Some(agent.to_owned()),
        },
        Err(_) => Some(agent.to_owned()),
    }
}

/// The supervisor, or the text the model reads when it is not running.
pub(crate) fn supervisor() -> Result<ActorRef<Agents>, &'static str> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent supervisor not running")
}

/// `tools` as a [`Risk`] ceiling: `None` (no cap) when absent or
/// `destructive`; an unknown value is the error the model reads.
/// `destructive` is `None` so the argument reads naturally.
fn cap_of(args: &SpawnSubagentArgs) -> Result<Option<Risk>, String> {
    let Some(tools) = &args.tools else {
        return Ok(None);
    };
    let risk: Risk = tools.parse().map_err(|e| format!("`tools`: {e}"))?;
    Ok((risk != Risk::Destructive).then_some(risk))
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
             For a theuth subagent, omit `model` to use the user's preferred subagent model; set `model`/`effort` only when the task clearly needs a different capability. \
             `tools: read_only` (or `mutating`) caps what the subagent and its descendants may run at that risk (`destructive` = no cap); an investigation that must not change anything gets `read_only`. \
             A child that will compile, run a large test suite or otherwise load the machine gets the resource rules you are under in its brief - lock or job cap, scope, target directory - because an external harness cannot discover peers (see Guidance: Share the machine).",
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
        let cap = match cap_of(&args) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let agent = args.agent.and_then(|a| canonical_agent(&self.nodes, &a));
        if let Some(agent) = &agent {
            let known = harnesses(&self.data_dir);
            let nodes = node_lines(&self.nodes);
            let (harness, engine) = match parse_agent(agent) {
                Ok(p) => p,
                Err(e) => return Ok(ToolOutput::err(e)),
            };
            // A remote harness is checked on its node; the node must be one of ours.
            let ok = match harness {
                Some(_) => nodes.contains_key(engine),
                None => known.contains_key(engine) || nodes.contains_key(engine),
            };
            if !ok {
                return Ok(ToolOutput::err(unknown(engine, &known, &nodes)));
            }
        }
        if agent.is_none() && !args.skills.is_empty() {
            return Ok(ToolOutput::err(
                "`skills` is for an external harness; a theuth subagent loads skills itself with load_skill",
            ));
        }
        Ok(
            match agents
                .ask(SpawnChild {
                    parent: cx.session,
                    name: args.name,
                    task: args.task,
                    agent,
                    cwd: args.cwd,
                    model: args.model,
                    effort: args.effort,
                    cap,
                    skills: args.skills,
                })
                .await
            {
                Ok((name, id, queued)) => ToolOutput::ok(format!(
                    "Spawned `{name}` (session:{id}). Its result will arrive as a <message from=\"session:{id}\"> when it finishes; message it with send_message to `{name}` or `session:{id}`.{}",
                    queued.map_or(String::new(), |q| format!("\n{q}"))
                )),
                Err(SendError::HandlerError(e)) => ToolOutput::err(e),
                Err(e) => ToolOutput::err(format!("agent supervisor: {e}")),
            },
        )
    }
}

/// Most recipients one `send_message` call fans out to (62 §0.8).
pub const SEND_MAX_RECIPIENTS: usize = 16;

/// `send_message.to` (62 §0.8): one address or a list of up to
/// [`SEND_MAX_RECIPIENTS`], each a canonical address from discover_peers
/// (`session:<uuid>` or `peer:<name>`), `parent`, or one of your subagents' names.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Recipients {
    /// One address.
    One(String),
    /// Several addresses (max 16); the same message reaches each, one receipt per recipient.
    Many(Vec<String>),
}

/// Arguments for [`SendMessageTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageArgs {
    /// One canonical address from discover_peers (`session:<uuid>` or `peer:<name>`), `parent`, or one of your subagents' names — or a list of up to 16 of them (one receipt per recipient).
    pub to: Recipients,
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

/// The recipient list as sent: `to` order, duplicates dropped, never empty
/// or over [`SEND_MAX_RECIPIENTS`].
fn recipients(to: Recipients) -> Result<Vec<String>, String> {
    let mut v = match to {
        Recipients::One(s) => vec![s],
        Recipients::Many(v) => v,
    };
    let mut seen = HashSet::new();
    v.retain(|a| seen.insert(a.clone()));
    if v.is_empty() {
        return Err("`to` names no recipient".into());
    }
    if v.len() > SEND_MAX_RECIPIENTS {
        return Err(format!(
            "`to` names {} recipients; at most {SEND_MAX_RECIPIENTS} per call",
            v.len()
        ));
    }
    Ok(v)
}

/// A refusal before anything was accepted, in receipt shape so the output
/// keeps one row per recipient. The nil id marks "never minted".
fn failed(to: String, error: String) -> PeerReceipt {
    PeerReceipt {
        id: Uuid::nil(),
        to,
        state: PeerMessageState::Failed,
        reply_to: None,
        reply: None,
        task_id: None,
        error: Some(error),
        answered_by: None,
    }
}

/// `<address> <state> id=<uuid>` plus task, error and the reply text.
fn receipt_line(r: &PeerReceipt) -> String {
    let mut s = format!("{} {} id={}", r.to, r.state.as_str(), r.id);
    if let Some(t) = &r.task_id {
        s.push_str(&format!(" task={t}"));
    }
    if let Some(e) = &r.error {
        s.push_str(&format!(" error={e}"));
    }
    if let Some(reply) = &r.reply {
        s.push('\n');
        s.push_str(reply);
    }
    s
}

/// One line per recipient; `structured = {"receipts": [...]}` always a list;
/// `is_error` only when every recipient failed.
pub(crate) fn receipts_output(receipts: &[PeerReceipt]) -> ToolOutput {
    let text = receipts
        .iter()
        .map(receipt_line)
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = if receipts.iter().all(|r| r.state == PeerMessageState::Failed) {
        ToolOutput::err(text)
    } else {
        ToolOutput::ok(text)
    };
    out.structured = Some(serde_json::json!({ "receipts": receipts }));
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
        format!(
            "Send one message to one or more peers. `to`: one canonical address from `discover_peers()` (python; `session:<uuid>`, \
             `peer:<name>`), `parent` if you are a subagent, or one of your subagents by name — or a list of up to \
             {SEND_MAX_RECIPIENTS} such addresses (every address must resolve or nothing is sent). Returns one receipt \
             per recipient (`structured.receipts`, always a list): `accepted` means committed to their intake, not read. \
             With await_reply=true you block until each answers with reply_to set to its message's id (or one shared \
             timeout passes; unanswered messages are not recalled). When answering a <message> with \
             reply_expected=\"true\", set reply_to to its id. `peer_message()` (python) shows a message's current state."
        )
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
        let to = match recipients(args.to) {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let resolved = match agents
            .ask(Resolve {
                from: cx.session,
                to: to.clone(),
            })
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        // A partial fan-out is the surprising outcome (62 §3.5): one bad
        // address among several means nothing is sent; the model resends
        // the good subset.
        if to.len() > 1 && resolved.iter().any(Result::is_err) {
            let rows: Vec<PeerReceipt> = to
                .into_iter()
                .zip(resolved)
                .map(|(a, r)| match r {
                    Ok(c) => failed(c, "not sent: another address failed".into()),
                    Err(e) => failed(a, e),
                })
                .collect();
            return Ok(receipts_output(&rows));
        }
        // Two spellings of one peer (`kid`, `session:<its id>`) send once.
        let mut canonical: Vec<String> = Vec::with_capacity(to.len());
        for r in resolved {
            match r {
                Ok(c) if !canonical.contains(&c) => canonical.push(c),
                Ok(_) => {}
                Err(e) => return Ok(ToolOutput::err(e)),
            }
        }
        let mut receipts = Vec::with_capacity(canonical.len());
        let mut waits = Vec::with_capacity(canonical.len());
        for c in canonical {
            match agents
                .ask(Deliver {
                    from: cx.session,
                    to: c.clone(),
                    text: args.message.clone(),
                    await_reply: args.await_reply,
                    reply_to: args.reply_to,
                    request_id: args.request_id.clone(),
                    admitted: admitted.token().clone(),
                })
                .await
            {
                Ok(d) => {
                    receipts.push(d.receipt);
                    waits.push(d.reply);
                }
                Err(SendError::HandlerError(e)) => {
                    receipts.push(failed(c, e));
                    waits.push(None);
                }
                Err(e) => {
                    receipts.push(failed(c, format!("agent supervisor: {e}")));
                    waits.push(None);
                }
            }
        }
        // One shared deadline for every awaited reply.
        let secs = self.cfg.subagents.reply_timeout_secs;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        // 109 W3 §4.4: the `waits(peer_reply)` row while this call blocks, so
        // a restart re-arms the waiter (`Agents::rebuild_waits`) and the
        // resumed call takes the reply that landed meanwhile.
        let awaited: Vec<Uuid> = receipts
            .iter()
            .zip(&waits)
            .filter(|(_, w)| w.is_some())
            .map(|(r, _)| r.id)
            .collect();
        let journal = tools::registry::journal();
        if let Some(first) = awaited.first() {
            journal
                .wait(
                    Some(tools::WaitRow {
                        session: cx.session,
                        tool_call_id: cx.call_id.clone(),
                        kind: tools::WaitKind::PeerReply,
                        target: first.to_string(),
                        deadline: Some(chrono::Utc::now() + Duration::from_secs(secs)),
                        payload: serde_json::json!({ "request_id": first, "request_ids": awaited }),
                    }),
                    cx.session,
                    &cx.call_id,
                )
                .await;
        }
        for (r, rx) in receipts.iter_mut().zip(waits) {
            let Some(rx) = rx else { continue };
            match tokio::time::timeout_at(deadline, rx).await {
                // Replied, or expired by the supervisor: disable, cancel, deletion.
                Ok(Ok(fin)) => *r = fin,
                Ok(Err(_)) => r.error = Some("wait dropped".into()),
                Err(_) => {
                    // Timeout: the waiter is removed, the accepted message stands.
                    let _ = agents.tell(DropWait(r.id)).await;
                    r.error = Some(format!("timed out after {secs}s; message not recalled"));
                }
            }
        }
        if !awaited.is_empty() {
            journal.wait(None, cx.session, &cx.call_id).await;
        }
        Ok(receipts_output(&receipts))
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
    /// Only peers working now: a turn in flight (running, approval, awaiting) or a running job/PTY.
    #[serde(default)]
    pub active_only: bool,
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
             a short name, kind, relationship, coarse state and capabilities, plus `activity` for local peers: \
             `state` (idle|running|approval|awaiting|error|cancelled|stored), `last_active` (the last input or stop, \
             not a heartbeat), `turn_secs`, `jobs`, `ptys`, `children`; remote peers have none. `engine` names the \
             engine a session lives on; a session owned by another engine of the mesh has state `remote` (its \
             activity is unknown here; send_message still reaches it). `title` is filled only \
             for your own tree (parent, children, siblings, ancestors, descendants); `tags` (short keywords the title \
             model assigned, e.g. `[rust, merge]`) come with every local peer, empty until it has a model-written title, \
             and say what a peer is working on so you can tell which are relevant to you. Rows come working-first, then \
             most recently active; `active_only` keeps only peers with a turn in flight or a running job/PTY (never \
             a `remote` one). Default {DISCOVER_DEFAULT_LIMIT} rows, max {DISCOVER_MAX_LIMIT}; pass `cursor` for the \
             next page. Listing wakes nobody and probes no network."
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
                active_only: args.active_only,
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
                    let mut s = format!(
                        "{} — {} ({:?}, {:?}, {:?}{}{})",
                        p.address,
                        p.name,
                        p.kind,
                        p.relationship,
                        p.state,
                        if p.reply { ", replies" } else { "" },
                        if p.stream { ", streams" } else { "" },
                    );
                    if let Some(a) = &p.activity {
                        s.push_str(&format!(" {:?}", a.state).to_lowercase());
                        if let Some(t) = a.turn_secs {
                            s.push_str(&format!(" turn={t}s"));
                        }
                        if a.jobs + a.ptys > 0 {
                            s.push_str(&format!(" jobs={} ptys={}", a.jobs, a.ptys));
                        }
                        if a.children > 0 {
                            s.push_str(&format!(" children={}", a.children));
                        }
                        if let Some(at) = a.last_active {
                            s.push_str(&format!(" last_active={}", at.to_rfc3339()));
                        }
                    }
                    if p.state == crate::communication::PeerState::Remote
                        && let Some(e) = &p.engine
                    {
                        s.push_str(&format!(" engine={e}"));
                    }
                    if let Some(t) = &p.title {
                        s.push_str(&format!(" title={t:?}"));
                    }
                    if !p.tags.is_empty() {
                        s.push_str(&format!(" [{}]", p.tags.join(", ")));
                    }
                    s
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

/// The models a parent may give a theuth subagent.
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

    /// A mesh engine for the resolution test: known by id, never dialled.
    #[derive(Debug)]
    struct MeshId(&'static str);

    #[async_trait::async_trait]
    impl acp::Dialer for MeshId {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            Err("not dialled".to_owned())
        }
        fn engine_id(&self) -> Option<&str> {
            Some(self.0)
        }
    }

    /// 67 §8: `agent: "<harness>@<engine>"` resolves the engine by node name
    /// first, then by a unique mesh id prefix; a 32 node has no id; the
    /// spawn stores the canonical `<harness>@<node name>`.
    #[test]
    fn engine_resolves_by_name_then_unique_id_prefix() {
        let nodes = Nodes::default();
        {
            let mut w = nodes.write().unwrap_or_else(|p| p.into_inner());
            let d = |d: Arc<dyn acp::Dialer>| (String::new(), d);
            w.insert("build-A".into(), d(Arc::new(MeshId("abcdef0123456789"))));
            w.insert("build-B".into(), d(Arc::new(MeshId("abcdff9876543210"))));
            w.insert("mac-mini".into(), d(Arc::new(NoDial)));
        }
        assert_eq!(resolve_node(&nodes, "build-A").as_deref(), Some("build-A"));
        assert_eq!(
            resolve_node(&nodes, "mac-mini").as_deref(),
            Some("mac-mini")
        );
        assert_eq!(resolve_node(&nodes, "abcdef01").as_deref(), Some("build-A"));
        assert_eq!(resolve_node(&nodes, "abcdff").as_deref(), Some("build-B"));
        assert_eq!(
            resolve_node(&nodes, "abcd"),
            None,
            "two matches are no match"
        );
        assert_eq!(resolve_node(&nodes, "mac"), None, "a 32 node has no id");
        assert_eq!(
            canonical_agent(&nodes, "omp@abcdef01").as_deref(),
            Some("omp@build-A")
        );
        assert_eq!(
            canonical_agent(&nodes, "abcdff98").as_deref(),
            Some("build-B")
        );
        assert_eq!(
            canonical_agent(&nodes, "omp").as_deref(),
            Some("omp"),
            "a local harness passes through"
        );
        assert_eq!(
            canonical_agent(&nodes, "a@b@c").as_deref(),
            Some("a@b@c"),
            "bad input is the spawn's error"
        );
        let me = platform::process::hostname();
        assert_eq!(
            canonical_agent(&nodes, me),
            None,
            "this engine → a local native child (hostname by default)"
        );
        assert_eq!(
            canonical_agent(&nodes, &format!("omp@{me}")).as_deref(),
            Some("omp")
        );
        // 67 2.3: with `mesh.name` set, the local engine goes by that name, not the hostname.
        mesh::set_self_name("build-me");
        assert_eq!(canonical_agent(&nodes, "build-me"), None);
        assert_eq!(
            canonical_agent(&nodes, "omp@build-me").as_deref(),
            Some("omp")
        );
        assert_eq!(
            canonical_agent(&nodes, &format!("omp@{me}")).as_deref(),
            Some(format!("omp@{me}").as_str()),
            "the hostname is a stranger now"
        );
        mesh::set_self_name("");
    }

    /// `tools` parses to a cap, `destructive` and absence are no cap, an
    /// unknown value is an error before any spawn; `agent` no longer refuses
    /// it (67 §8: enforced on a node, a brief line for an external harness).
    #[test]
    fn spawn_tools_arg_parses_to_a_cap_or_a_clear_error() {
        let args = |tools: Option<&str>, agent: Option<&str>| SpawnSubagentArgs {
            task: "t".into(),
            name: None,
            agent: agent.map(str::to_owned),
            cwd: None,
            model: None,
            effort: None,
            tools: tools.map(str::to_owned),
            skills: Vec::new(),
        };
        assert_eq!(cap_of(&args(None, None)), Ok(None));
        assert_eq!(
            cap_of(&args(Some("read_only"), None)),
            Ok(Some(Risk::ReadOnly))
        );
        assert_eq!(
            cap_of(&args(Some("mutating"), None)),
            Ok(Some(Risk::Mutating))
        );
        assert_eq!(cap_of(&args(Some("destructive"), None)), Ok(None));
        let e = cap_of(&args(Some("readonly"), None)).unwrap_err();
        assert!(
            e.contains("unknown risk `readonly`") && e.contains("read_only"),
            "{e}"
        );
        assert_eq!(
            cap_of(&args(Some("read_only"), Some("build-A"))),
            Ok(Some(Risk::ReadOnly))
        );
        assert_eq!(cap_of(&args(None, Some("codex"))), Ok(None));
    }

    #[test]
    fn spawn_description_lists_configured_harnesses() {
        let dir = std::env::temp_dir().join(format!("theuth-peers-{}", uuid::Uuid::now_v7()));
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
                model: None,
                skills: None,
                skills_dir: None,
                mcp: None,
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
        let dir = std::env::temp_dir().join(format!("theuth-peers-{}", uuid::Uuid::now_v7()));
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
                    "linux · theuth 0.4.1".into(),
                    Arc::new(NoDial) as Arc<dyn acp::Dialer>,
                ),
            );
        }
        let desc = SpawnSubagentTool {
            data_dir: dir.clone(),
            nodes: nodes.clone(),
        }
        .description();
        let line =
            "e.g. `omp@build-box`): build-box — linux · theuth 0.4.1; mac-mini — macos · offline";
        assert!(desc.contains(line), "{desc}");
        assert!(desc.find("External harnesses").unwrap() < desc.find("Nodes (").unwrap());

        let err = unknown("box", &harnesses(&dir), &node_lines(&nodes));
        assert!(err.starts_with("unknown agent `box`"), "{err}");
        assert!(err.ends_with("nodes: build-box, mac-mini"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 67 §8: `agent` is `<name>` or `<harness>@<node>`; nothing else.
    #[test]
    fn agent_parses_as_optional_harness_at_engine() {
        assert_eq!(parse_agent("omp@build-A"), Ok((Some("omp"), "build-A")));
        assert_eq!(parse_agent("build-A"), Ok((None, "build-A")));
        assert_eq!(parse_agent("omp"), Ok((None, "omp")));
        for bad in ["@x", "x@", "@", "", "a@b@c"] {
            let e = parse_agent(bad).unwrap_err();
            assert!(e.contains("<harness>@<node>"), "{bad}: {e}");
        }
    }

    /// 62 §3.5: `to` is one address or a list; the list is deduplicated in
    /// order and bounded to [`SEND_MAX_RECIPIENTS`], empty is refused.
    #[test]
    fn recipients_accept_one_or_many() {
        let one: SendMessageArgs =
            serde_json::from_value(serde_json::json!({ "to": "parent", "message": "m" })).unwrap();
        assert_eq!(recipients(one.to).unwrap(), vec!["parent"]);
        let many: SendMessageArgs =
            serde_json::from_value(serde_json::json!({ "to": ["x", "y", "x"], "message": "m" }))
                .unwrap();
        assert_eq!(recipients(many.to).unwrap(), vec!["x", "y"]);
        assert!(recipients(Recipients::Many(vec![])).is_err());
        let over: Vec<String> = (0..=SEND_MAX_RECIPIENTS).map(|i| format!("p{i}")).collect();
        let e = recipients(Recipients::Many(over)).unwrap_err();
        assert!(e.contains("at most 16"), "{e}");
        let full: Vec<String> = (0..SEND_MAX_RECIPIENTS).map(|i| format!("p{i}")).collect();
        assert_eq!(recipients(Recipients::Many(full)).unwrap().len(), 16);
        // The schema the model reads offers both shapes.
        let schema = serde_json::to_value(schemars::schema_for!(Recipients)).unwrap();
        assert_eq!(
            schema["anyOf"].as_array().map(Vec::len),
            Some(2),
            "{schema}"
        );
    }

    /// 62 §3.5: two recipients yield two receipt lines and a two-element
    /// `structured.receipts`; one unresolved address among two sends nothing
    /// and reports both rows as failed; `is_error` only when all failed.
    #[test]
    fn receipts_output_is_one_row_per_recipient() {
        let ok = |to: &str| PeerReceipt {
            id: Uuid::now_v7(),
            to: to.into(),
            state: PeerMessageState::Accepted,
            reply_to: None,
            reply: None,
            task_id: None,
            error: None,
            answered_by: None,
        };
        let (a, b) = (ok("session:a"), ok("session:b"));
        let out = receipts_output(&[a.clone(), b.clone()]);
        assert!(!out.is_error);
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], format!("session:a accepted id={}", a.id));
        assert_eq!(lines[1], format!("session:b accepted id={}", b.id));
        let s = out.structured.unwrap();
        assert_eq!(s["receipts"].as_array().map(Vec::len), Some(2));
        assert_eq!(s["receipts"][1]["to"], "session:b");
        // A single recipient is still a list.
        let s = receipts_output(std::slice::from_ref(&a))
            .structured
            .unwrap();
        assert_eq!(s["receipts"].as_array().map(Vec::len), Some(1));
        // One bad address among two: nothing sent, two error lines, is_error.
        let out = receipts_output(&[
            failed(
                "session:a".into(),
                "not sent: another address failed".into(),
            ),
            failed("bogus".into(), "unknown peer `bogus`".into()),
        ]);
        assert!(out.is_error);
        assert_eq!(out.content.lines().count(), 2);
        assert!(
            out.content.contains(
                "bogus failed id=00000000-0000-0000-0000-000000000000 error=unknown peer `bogus`"
            ),
            "{}",
            out.content
        );
        // One failure beside one acceptance is not an error.
        let out = receipts_output(&[a, failed("session:c".into(), "busy".into())]);
        assert!(!out.is_error);
        // A reply follows its line.
        let mut r = b;
        r.state = PeerMessageState::Replied;
        r.reply = Some("yes".into());
        assert_eq!(
            receipt_line(&r),
            format!("session:b replied id={}\nyes", r.id)
        );
    }
}
