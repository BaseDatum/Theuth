//! 208: the Attention graph — one aggregate of every active and recently active session tree and the peer messages
//! between them, served as one snapshot (`agent_graph`) and one coalesced delta topic (`agent.graph`). Lease-driven:
//! dormant (only the `last_event_ms` clock) until a client asks; see `docs/design/208-attention-graph.md` §5.

use std::collections::{HashMap, HashSet};

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::Store;
use crate::supervisor::observed_state;
use crate::{
    ActivityOf, AgentEvent, AgentInfo, AgentState, Agents, ListActivity, ListSessions,
    ObservedMeta, ObservedState, PeerDelivered, PeerMessageChanged, PeerMessageState,
    SessionActivity, SessionKind, SessionSummary,
};

/// 208 D9: what a graph node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeKind {
    /// A root chat or routine run (no parent, no desk).
    Root,
    /// A hired agent's desk session.
    Desk,
    /// A subagent: native, or an ACP harness child (`agent` set).
    Child,
    /// A watch-only child a harness runs inside itself (141 D2).
    Observed,
    /// A boundary endpoint outside this engine's sessions: a mesh peer or a contact (`peer:<name>`).
    Peer,
}

/// 208 D7: what a session is parked on besides its `AgentState`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphWait {
    /// A pending question to the user: ask, hand-off or secret (`waits.kind = 'question'`).
    User,
    /// A shell job or pty (`waits.kind` `job` | `pty_wait`).
    Job,
    /// Its children's results (`waits.kind = 'child_result'`).
    Children,
}

/// 208 D7: how the session's last turn ended (207 §4's mapping of `stop_reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphOutcome {
    /// Any provider stop reason that is not one of the three below.
    Finished,
    /// `error`.
    Failed,
    /// `cancelled`.
    Cancelled,
    /// `timed_out`.
    TimedOut,
}

/// 208 §2.1: one session (or boundary endpoint) on the graph. `PartialEq` only: [`ObservedMeta`] carries a
/// `serde_json::Value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphNode {
    /// Session id; a boundary node is `peer:<name>`.
    pub id: String,
    /// What the node is.
    pub kind: GraphNodeKind,
    /// The parent session; `None` for roots, desks and boundary nodes.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent: Option<String>,
    /// The root of the node's tree (itself for a root, desk or boundary node).
    pub root: String,
    /// Root/desk: the title (empty → `<desk> desk`, else the short id); child: the peer name; boundary: the name.
    pub title: String,
    /// The peer name of a child; `None` for roots, desks and boundary nodes.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub name: Option<String>,
    /// The harness (`agents.yaml` key) of an ACP session; `None` for theuth sessions.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub agent: Option<String>,
    /// The session's model id ("" for boundary nodes).
    pub model: String,
    /// The root's project.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub project: Option<String>,
    /// The desk's roster agent.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub desk: Option<String>,
    /// Live facts (`ActivityOf`); remote-owned: `Running` when `SessionSummary.running`, else `Idle`; boundary: idle default.
    pub activity: SessionActivity,
    /// What it waits on besides its state, sorted, deduplicated.
    pub waits: Vec<GraphWait>,
    /// The last turn's outcome while the session is not running; `None` before any turn, while running, and for
    /// `interrupted` / `suspended` / `harness_continued`.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub outcome: Option<GraphOutcome>,
    /// Watch-only children: the harness's view (141).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub observed: Option<ObservedMeta>,
    /// The owning engine (`SessionSummary.owner`; "" for boundary nodes).
    pub owner: String,
    /// Owned by another engine: state is last-known (208 §6).
    pub remote: bool,
    /// ms since epoch the session was created (boundary: its first exchange).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_ms: u64,
    /// ms since epoch of the session's last event of any kind (208 §2.3); boundary: its newest exchange.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_event_ms: u64,
}

/// 208 D8: the peer messages between one unordered pair of nodes, `a < b` (byte order of the ids).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphEdge {
    /// `"<a>|<b>"`.
    pub id: String,
    /// The smaller node id.
    pub a: String,
    /// The larger node id.
    pub b: String,
    /// Exchanges sent a → b in the window.
    pub ab: u32,
    /// Exchanges sent b → a in the window.
    pub ba: u32,
    /// Of those, a child's final result to its parent (request id `result:*`).
    pub results: u32,
    /// Exchanges still in flight (`accepted` or `held`).
    pub open: u32,
    /// ms since epoch of the newest exchange change.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_ms: u64,
    /// Sender of the newest exchange (`a` or `b`).
    pub last_from: String,
    /// The newest exchange's id (for the popover).
    pub last_id: String,
}

/// 208 §2.2: what happened to one exchange (a live edge event).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GraphPulseKind {
    /// Accepted into the recipient's intake.
    Sent,
    /// Consumed by the recipient's turn.
    Delivered,
    /// Answered.
    Replied,
    /// A child's final result reached its parent.
    Result,
    /// Suppressed or failed.
    Failed,
}

/// 208 §2.2: one exchange's state change, for the UI to animate along `edge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphPulse {
    /// The edge (`GraphEdge.id`).
    pub edge: String,
    /// Sender node id.
    pub from: String,
    /// Recipient node id.
    pub to: String,
    /// What happened.
    pub kind: GraphPulseKind,
    /// The exchange id.
    pub id: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
}

/// 208 D10: idle trees left out of a snapshot over the node cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphOlder {
    /// Trees left out.
    pub roots: u32,
    /// Nodes in them.
    pub nodes: u32,
    /// The newest `last_event_ms` among them.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub newest_ms: u64,
}

/// 208 §4: where a client is in the delta stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GraphCursor {
    /// The activation (`AgentGraph.epoch_ms`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// The last applied `seq`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
}

/// 208 §4: the `agent_graph` reply. `PartialEq` only (carries [`GraphNode`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentGraph {
    /// ms since epoch this activation started; a new value = a new stream (`seq` restarts).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// The last published delta's `seq` (0 = none yet); the next delta is `seq + 1`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// ms since epoch of this reply.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// The window served (requested, defaulted and clamped).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub window_ms: u64,
    /// The caller's cursor was current: every list is empty, keep your state.
    pub unchanged: bool,
    /// Every node in the window, parents before children.
    pub nodes: Vec<GraphNode>,
    /// Every message edge whose two ends are in `nodes`.
    pub edges: Vec<GraphEdge>,
    /// Trees left out over the node cap.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub older: Option<GraphOlder>,
}

/// 208 §4: coalesced changes since the previous event (at most one per `FLUSH_MS`). `PartialEq` only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct AgentGraphChanged {
    /// The activation this belongs to.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// 1-based per activation, +1 per event.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// Nodes added or changed (whole nodes), parents before children.
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    /// Node ids gone (session deleted); their edges go too.
    #[serde(default)]
    pub removed: Vec<String>,
    /// Edges added or changed (whole aggregates).
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Edge ids gone.
    #[serde(default)]
    pub edges_removed: Vec<String>,
    /// Exchange state changes since the previous event, oldest first, at most `MAX_PULSES`.
    #[serde(default)]
    pub pulses: Vec<GraphPulse>,
    /// The current over-cap stub (unchanged when equal to the last one sent).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub older: Option<GraphOlder>,
    /// Too much changed: every list is empty; call `agent_graph` again.
    #[serde(default)]
    pub resync: bool,
}

impl wire::WireEvent for AgentGraphChanged {
    const TOPIC: &'static str = "agent.graph";

    fn policy(json: &serde_json::Value) -> wire::EventPolicy {
        wire::EventPolicy {
            always_wanted: true,
            ..wire::EventPolicy::legacy_fields(json)
        }
    }
}

/// Registry name of the [`Graph`] actor.
pub const GRAPH_ACTOR: &str = "graph";
/// A lease lasts this long after the last `agent_graph` call.
pub const LEASE_SECS: u64 = 300;
/// `window_ms` when the caller passes null.
pub const DEFAULT_WINDOW_MS: u64 = 24 * 3_600_000;
/// The largest window served.
pub const MAX_WINDOW_MS: u64 = 7 * 24 * 3_600_000;
/// Nodes per snapshot; idle trees over it fold into `older`.
pub const MAX_NODES: usize = 1_500;
/// Edges per snapshot.
pub const MAX_EDGES: usize = 3_000;
/// Newest exchange rows read for a seed.
pub const HISTORY_ROWS: i64 = 5_000;
/// `include` ids honoured per call.
pub const MAX_INCLUDE: usize = 200;
/// Minimum spacing of two deltas.
pub const FLUSH_MS: u64 = 250;
/// Upserts per delta; more → `resync`.
pub const MAX_UPSERTS: usize = 500;
/// Pulses per delta (the newest are kept).
pub const MAX_PULSES: usize = 100;
/// The `session_last_event` write period.
pub const PERSIST_SECS: u64 = 30;
/// A delta-class event re-publishes its node at most this often.
pub const QUIET_BUMP_MS: u64 = 30_000;

/// Prune period.
const PRUNE_SECS: u64 = 60;
/// Events buffered while a seed runs; beyond it the seed's reply is still served and later events re-mark.
const MAX_BUFFERED: usize = 10_000;

/// One `peer_exchanges` row as the graph reads it (`Store::exchanges_since`).
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct ExchangeEdgeRow {
    /// Exchange id.
    pub id: String,
    /// Sender address.
    pub sender: String,
    /// Recipient address.
    pub recipient: String,
    /// The sender's local session, when local.
    pub sender_session: Option<String>,
    /// The recipient's local session, when local.
    pub recipient_session: Option<String>,
    /// Idempotency key; `result:*` for a child's final result.
    pub request_id: String,
    /// `PeerMessageState` word.
    pub state: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

fn ms(t: chrono::DateTime<chrono::Utc>) -> u64 {
    u64::try_from(t.timestamp_millis()).unwrap_or(0)
}

fn rfc_ms(s: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| ms(t.with_timezone(&chrono::Utc)))
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    ms(chrono::Utc::now())
}

/// 208 D9: the node kind of a session row; advisor and ask sessions are not on the graph.
pub(crate) fn node_kind(s: &SessionSummary) -> Option<GraphNodeKind> {
    match s.kind {
        SessionKind::Advisor | SessionKind::Ask => None,
        SessionKind::Observed => Some(GraphNodeKind::Observed),
        SessionKind::Chat if s.desk.is_some() => Some(GraphNodeKind::Desk),
        SessionKind::Chat if s.parent.is_some() => Some(GraphNodeKind::Child),
        SessionKind::Chat => Some(GraphNodeKind::Root),
    }
}

/// 207 §4: a turn's stop reason as an outcome.
pub(crate) fn outcome_of(stop_reason: &str) -> Option<GraphOutcome> {
    match stop_reason {
        "error" => Some(GraphOutcome::Failed),
        "cancelled" => Some(GraphOutcome::Cancelled),
        "timed_out" => Some(GraphOutcome::TimedOut),
        "interrupted" | "suspended" | "harness_continued" => None,
        _ => Some(GraphOutcome::Finished),
    }
}

/// A `waits.kind` as a graph wait.
pub(crate) fn wait_of(kind: &str) -> Option<GraphWait> {
    match kind {
        "question" => Some(GraphWait::User),
        "job" | "pty_wait" => Some(GraphWait::Job),
        "child_result" => Some(GraphWait::Children),
        _ => None,
    }
}

/// A peer address as a node id: `session:<uuid>` → the uuid, `peer:<name>` and `peer:<c>/session:<u>` → `peer:<name>`.
pub(crate) fn endpoint(addr: &str) -> Option<String> {
    if let Some(id) = addr.strip_prefix("session:") {
        return id.parse::<Uuid>().ok().map(|u| u.to_string());
    }
    let rest = addr.strip_prefix("peer:")?;
    let name = rest.split_once('/').map_or(rest, |(n, _)| n);
    (!name.is_empty()).then(|| format!("peer:{name}"))
}

/// The unordered pair `(id, lo, hi)`, byte order.
pub(crate) fn pair(a: &str, b: &str) -> (String, String, String) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (format!("{lo}|{hi}"), lo.to_owned(), hi.to_owned())
}

fn short_id(id: SessionId) -> String {
    id.to_string().chars().take(8).collect()
}

/// 208 §2.1: root/desk → the title (empty → `<desk> desk`, else the short id); child → the peer name.
pub(crate) fn title_of(s: &SessionSummary) -> String {
    let kind = node_kind(s);
    if matches!(kind, Some(GraphNodeKind::Child | GraphNodeKind::Observed))
        && let Some(n) = s.name.as_deref().filter(|n| !n.is_empty())
    {
        return n.to_owned();
    }
    if !s.title.is_empty() {
        return s.title.clone();
    }
    match &s.desk {
        Some(d) => format!("{d} desk"),
        None => short_id(s.id),
    }
}

/// A session's clock: its `last_event_ms`, else `max(updated_at, finished_at)`.
fn last_of(s: &SessionSummary, last: &HashMap<SessionId, u64>) -> u64 {
    last.get(&s.id)
        .copied()
        .unwrap_or_else(|| ms(s.updated_at).max(s.finished_at.map_or(0, ms)))
}

/// Eligible rows (not advisor/ask/private, ancestors eligible), their root and depth.
fn forest(sessions: &[SessionSummary]) -> HashMap<SessionId, (SessionId, usize)> {
    let by_id: HashMap<SessionId, &SessionSummary> = sessions.iter().map(|s| (s.id, s)).collect();
    let ok =
        |s: &SessionSummary| node_kind(s).is_some() && s.retention != tools::Retention::Private;
    let mut out = HashMap::new();
    for s in sessions {
        let mut cur = s;
        let mut depth = 0usize;
        let mut good = ok(s);
        while good && let Some(p) = cur.parent.and_then(|p| by_id.get(&p)) {
            if !ok(p) || depth > 64 {
                good = false;
                break;
            }
            cur = p;
            depth += 1;
        }
        if good {
            out.insert(s.id, (cur.id, depth));
        }
    }
    out
}

/// 208 §5: the sessions to serve, parents before children, and the trees folded over `max_nodes`.
pub(crate) fn select_window(
    sessions: &[SessionSummary],
    last: &HashMap<SessionId, u64>,
    live: &HashSet<SessionId>,
    include: &[SessionId],
    now_ms: u64,
    window_ms: u64,
    max_nodes: usize,
) -> (Vec<SessionId>, Option<GraphOlder>) {
    let tree = forest(sessions);
    let by_id: HashMap<SessionId, &SessionSummary> = sessions.iter().map(|s| (s.id, s)).collect();
    let since = now_ms.saturating_sub(window_ms);
    let included: HashSet<SessionId> = include.iter().copied().collect();
    let mut chosen: HashSet<SessionId> = HashSet::new();
    for s in sessions {
        if !tree.contains_key(&s.id) {
            continue;
        }
        if live.contains(&s.id) || included.contains(&s.id) || last_of(s, last) >= since {
            let mut cur = Some(s.id);
            while let Some(id) = cur {
                if !chosen.insert(id) {
                    break;
                }
                cur = by_id
                    .get(&id)
                    .and_then(|r| r.parent)
                    .filter(|p| tree.contains_key(p));
            }
        }
    }
    // Per root: (nodes, pinned, newest).
    let mut trees: HashMap<SessionId, (u32, bool, u64)> = HashMap::new();
    for id in &chosen {
        let (root, _) = tree[id];
        let t = trees.entry(root).or_default();
        t.0 += 1;
        t.1 |= live.contains(id) || included.contains(id);
        t.2 = t.2.max(last_of(by_id[id], last));
    }
    let mut older = None;
    if chosen.len() > max_nodes {
        let mut idle: Vec<(SessionId, u32, u64)> = trees
            .iter()
            .filter(|(_, t)| !t.1)
            .map(|(r, t)| (*r, t.0, t.2))
            .collect();
        idle.sort_by_key(|(r, _, newest)| (*newest, r.0));
        let mut total = chosen.len();
        let mut folded = GraphOlder {
            roots: 0,
            nodes: 0,
            newest_ms: 0,
        };
        let mut gone = HashSet::new();
        for (root, n, newest) in idle {
            if total <= max_nodes {
                break;
            }
            total -= n as usize;
            gone.insert(root);
            folded.roots += 1;
            folded.nodes += n;
            folded.newest_ms = folded.newest_ms.max(newest);
        }
        chosen.retain(|id| !gone.contains(&tree[id].0));
        older = (folded.roots > 0).then_some(folded);
    }
    let mut out: Vec<SessionId> = chosen.into_iter().collect();
    out.sort_by_key(|id| (tree[id].1, by_id[id].created_at, id.0));
    (out, older)
}

/// An exchange row's two ends as node ids.
fn row_ends(r: &ExchangeEdgeRow) -> Option<(String, String)> {
    let from = endpoint(&r.sender).or_else(|| r.sender_session.clone())?;
    let to = endpoint(&r.recipient).or_else(|| r.recipient_session.clone())?;
    (from != to).then_some((from, to))
}

/// 208 D8: pair aggregates over `rows` (newest first), at most `max_edges` pairs (the newest), and the boundary
/// (`peer:`) ids among their ends, sorted.
pub(crate) fn aggregate(
    rows: &[ExchangeEdgeRow],
    max_edges: usize,
) -> (Vec<GraphEdge>, Vec<String>) {
    let mut order: Vec<String> = Vec::new();
    let mut edges: HashMap<String, GraphEdge> = HashMap::new();
    for r in rows {
        let Some((from, to)) = row_ends(r) else {
            continue;
        };
        let (id, a, b) = pair(&from, &to);
        if !edges.contains_key(&id) {
            if order.len() >= max_edges {
                continue;
            }
            order.push(id.clone());
            edges.insert(
                id.clone(),
                GraphEdge {
                    id: id.clone(),
                    a: a.clone(),
                    b,
                    ab: 0,
                    ba: 0,
                    results: 0,
                    open: 0,
                    last_ms: 0,
                    last_from: String::new(),
                    last_id: String::new(),
                },
            );
        }
        let e = edges.get_mut(&id).expect("inserted");
        if from == a {
            e.ab += 1;
        } else {
            e.ba += 1;
        }
        if r.request_id.starts_with("result:") {
            e.results += 1;
        }
        if matches!(r.state.as_str(), "accepted" | "held") {
            e.open += 1;
        }
        let at = rfc_ms(&r.updated_at);
        if e.last_id.is_empty() || at > e.last_ms {
            e.last_ms = at;
            e.last_from = from;
            e.last_id = r.id.clone();
        }
    }
    let mut peers: Vec<String> = order
        .iter()
        .flat_map(|id| [edges[id].a.clone(), edges[id].b.clone()])
        .filter(|n| n.starts_with("peer:"))
        .collect();
    peers.sort();
    peers.dedup();
    (
        order
            .into_iter()
            .map(|id| edges.remove(&id).expect("kept"))
            .collect(),
        peers,
    )
}

fn observed_outcome(o: &ObservedMeta) -> Option<GraphOutcome> {
    match o.state {
        ObservedState::Completed => Some(GraphOutcome::Finished),
        ObservedState::Failed => Some(GraphOutcome::Failed),
        ObservedState::Cancelled => Some(GraphOutcome::Cancelled),
        _ => None,
    }
}

fn waits_of(kinds: Option<&Vec<String>>) -> Vec<GraphWait> {
    let mut w: Vec<GraphWait> = kinds
        .into_iter()
        .flatten()
        .filter_map(|k| wait_of(k))
        .collect();
    w.sort();
    w.dedup();
    w
}

fn remote_activity(running: bool) -> SessionActivity {
    SessionActivity {
        state: if running {
            AgentState::Running
        } else {
            AgentState::Idle
        },
        ..SessionActivity::default()
    }
}

fn is_live(n: &GraphNode) -> bool {
    n.activity.live
        || !n.waits.is_empty()
        || matches!(
            n.activity.state,
            AgentState::Running | AgentState::Awaiting { .. } | AgentState::Approval { .. }
        )
}

/// A node from its row. `root` is the tree's root id.
fn node_of(
    s: &SessionSummary,
    root: SessionId,
    last_ms: u64,
    activity: Option<&SessionActivity>,
    waits: Option<&Vec<String>>,
    stop: Option<&String>,
) -> Option<GraphNode> {
    let kind = node_kind(s)?;
    let remote = !s.here;
    let mut activity = match activity {
        Some(a) if !remote => a.clone(),
        _ => remote_activity(s.running),
    };
    let outcome = match &s.observed {
        Some(o) => {
            activity.state = observed_state(o, &activity.state);
            observed_outcome(o)
        }
        None if s.running => None,
        None => stop.and_then(|r| outcome_of(r)),
    };
    Some(GraphNode {
        id: s.id.to_string(),
        kind,
        parent: s.parent.map(|p| p.to_string()),
        root: root.to_string(),
        title: title_of(s),
        name: matches!(kind, GraphNodeKind::Child | GraphNodeKind::Observed)
            .then(|| s.name.clone())
            .flatten(),
        agent: s.agent.clone(),
        model: s.model.clone(),
        project: s.project.clone(),
        desk: s.desk.clone(),
        activity,
        waits: waits_of(waits),
        outcome,
        observed: s.observed.clone(),
        owner: s.owner.clone(),
        remote,
        started_ms: ms(s.created_at),
        last_event_ms: last_ms,
    })
}

fn peer_node(id: &str, at: u64) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        kind: GraphNodeKind::Peer,
        parent: None,
        root: id.to_owned(),
        title: id.trim_start_matches("peer:").to_owned(),
        name: None,
        agent: None,
        model: String::new(),
        project: None,
        desk: None,
        activity: SessionActivity::default(),
        waits: Vec::new(),
        outcome: None,
        observed: None,
        owner: String::new(),
        remote: true,
        started_ms: at,
        last_event_ms: at,
    }
}

/// What a seed read.
pub(crate) struct Seed {
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) last: HashMap<SessionId, u64>,
    pub(crate) activity: HashMap<SessionId, SessionActivity>,
    pub(crate) waits: HashMap<SessionId, Vec<String>>,
    pub(crate) stops: HashMap<SessionId, String>,
    pub(crate) rows: Vec<ExchangeEdgeRow>,
    pub(crate) chosen: Vec<SessionId>,
    pub(crate) older: Option<GraphOlder>,
}

/// Ids of a flush in flight.
#[derive(Debug, Default)]
pub(crate) struct FlushBatch {
    pub(crate) nodes: Vec<String>,
    pub(crate) edges: Vec<String>,
}

impl FlushBatch {
    /// The local sessions whose live facts to read.
    pub(crate) fn sessions(&self, model: &Model) -> Vec<SessionId> {
        self.nodes
            .iter()
            .filter_map(|id| model.nodes.get(id))
            .filter(|n| n.kind != GraphNodeKind::Peer && !n.remote)
            .filter_map(|n| n.id.parse().ok())
            .collect()
    }
}

/// The active aggregate (one lease).
#[derive(Debug)]
pub(crate) struct Model {
    pub(crate) epoch_ms: u64,
    pub(crate) seq: u64,
    pub(crate) window_ms: u64,
    pub(crate) nodes: HashMap<String, GraphNode>,
    /// Every pair seen; published only while both ends are nodes.
    pub(crate) edges: HashMap<String, GraphEdge>,
    include: HashSet<String>,
    older: Option<GraphOlder>,
    published: HashMap<String, GraphNode>,
    published_edges: HashMap<String, GraphEdge>,
    published_older: Option<GraphOlder>,
    dirty: HashSet<String>,
    dirty_edges: HashSet<String>,
    removed: Vec<String>,
    edges_removed: Vec<String>,
    pulses: Vec<GraphPulse>,
    seen: HashSet<(Uuid, PeerMessageState)>,
    closed: HashSet<Uuid>,
    /// When the scheduled flush fires; `None` = none scheduled.
    pub(crate) flush_at: Option<u64>,
    last_flush: u64,
}

impl Model {
    /// An activation at `now_ms` from a seed's reads.
    pub(crate) fn build(seed: Seed, now_ms: u64, window_ms: u64, include: &[SessionId]) -> Self {
        let tree = forest(&seed.sessions);
        let by_id: HashMap<SessionId, &SessionSummary> =
            seed.sessions.iter().map(|s| (s.id, s)).collect();
        let mut chosen: Vec<SessionId> = seed.chosen.clone();
        let mut have: HashSet<SessionId> = chosen.iter().copied().collect();
        let (edges, peers) = aggregate(&seed.rows, MAX_EDGES);
        // An exchange whose session end is outside the window brings it (and its ancestors) in.
        for e in &edges {
            for end in [&e.a, &e.b] {
                let Ok(id) = end.parse::<SessionId>() else {
                    continue;
                };
                let mut cur = Some(id);
                while let Some(c) = cur.filter(|c| tree.contains_key(c)) {
                    if !have.insert(c) {
                        break;
                    }
                    chosen.push(c);
                    cur = by_id[&c].parent;
                }
            }
        }
        chosen.sort_by_key(|id| (tree[id].1, by_id[id].created_at, id.0));
        let mut nodes = HashMap::new();
        for id in &chosen {
            let s = by_id[id];
            if let Some(n) = node_of(
                s,
                tree[id].0,
                last_of(s, &seed.last),
                seed.activity.get(id),
                seed.waits.get(id),
                seed.stops.get(id),
            ) {
                nodes.insert(n.id.clone(), n);
            }
        }
        let edges: HashMap<String, GraphEdge> =
            edges.into_iter().map(|e| (e.id.clone(), e)).collect();
        for p in peers {
            let (first, newest) = seed
                .rows
                .iter()
                .filter(|r| row_ends(r).is_some_and(|(f, t)| f == p || t == p))
                .fold((u64::MAX, 0), |(lo, hi), r| {
                    (lo.min(rfc_ms(&r.created_at)), hi.max(rfc_ms(&r.updated_at)))
                });
            let mut n = peer_node(&p, newest);
            n.started_ms = first.min(newest);
            nodes.insert(p, n);
        }
        let mut m = Self::empty(now_ms, window_ms);
        m.nodes = nodes;
        m.edges = edges;
        m.older = seed.older;
        m.include = include.iter().map(|s| s.to_string()).collect();
        m.published = m.nodes.clone();
        m.published_edges = m.edges.clone();
        m.published_older = m.older.clone();
        m
    }

    pub(crate) fn empty(now_ms: u64, window_ms: u64) -> Self {
        Self {
            epoch_ms: now_ms,
            seq: 0,
            window_ms,
            nodes: HashMap::new(),
            edges: HashMap::new(),
            include: HashSet::new(),
            older: None,
            published: HashMap::new(),
            published_edges: HashMap::new(),
            published_older: None,
            dirty: HashSet::new(),
            dirty_edges: HashSet::new(),
            removed: Vec::new(),
            edges_removed: Vec::new(),
            pulses: Vec::new(),
            seen: HashSet::new(),
            closed: HashSet::new(),
            flush_at: None,
            last_flush: 0,
        }
    }

    /// Schedule a flush if none is: `Some(at)` = the caller arms a timer.
    fn schedule(&mut self, now: u64) -> Option<u64> {
        if self.flush_at.is_some() {
            return None;
        }
        let at = now.max(self.last_flush.saturating_add(FLUSH_MS));
        self.flush_at = Some(at);
        Some(at)
    }

    /// Mark node `id` changed (ignored when not in the model).
    pub(crate) fn mark(&mut self, id: &str, now: u64) -> Option<u64> {
        if !self.nodes.contains_key(id) {
            return None;
        }
        self.dirty.insert(id.to_owned());
        self.schedule(now)
    }

    /// A delta-class event: mark only when the node's published clock is `QUIET_BUMP_MS` old.
    pub(crate) fn bump(&mut self, id: &str, now: u64) -> Option<u64> {
        let seen = self
            .published
            .get(id)
            .or_else(|| self.nodes.get(id))
            .map(|n| n.last_event_ms)?;
        (now.saturating_sub(seen) >= QUIET_BUMP_MS)
            .then(|| self.mark(id, now))
            .flatten()
    }

    /// Delete a node and its edges.
    pub(crate) fn remove(&mut self, id: &str, now: u64) -> Option<u64> {
        self.nodes.remove(id)?;
        self.dirty.remove(id);
        self.include.remove(id);
        if self.published.remove(id).is_some() {
            self.removed.push(id.to_owned());
        }
        let gone: Vec<String> = self
            .edges
            .values()
            .filter(|e| e.a == id || e.b == id)
            .map(|e| e.id.clone())
            .collect();
        for e in gone {
            self.edges.remove(&e);
            self.dirty_edges.remove(&e);
            if self.published_edges.remove(&e).is_some() {
                self.edges_removed.push(e);
            }
        }
        self.schedule(now)
    }

    /// Insert or patch a child announced by `SubagentSpawned` / `ObservedChildChanged`; `false` when the parent is
    /// not in the model.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn child(
        &mut self,
        parent: SessionId,
        child: SessionId,
        name: &str,
        agent: Option<&String>,
        model: Option<&String>,
        observed: Option<&ObservedMeta>,
        now: u64,
    ) -> bool {
        let Some(p) = self.nodes.get(&parent.to_string()).cloned() else {
            return false;
        };
        let id = child.to_string();
        let n = self.nodes.entry(id.clone()).or_insert_with(|| GraphNode {
            id: id.clone(),
            kind: GraphNodeKind::Child,
            parent: Some(p.id.clone()),
            root: p.root.clone(),
            title: name.to_owned(),
            name: Some(name.to_owned()),
            agent: None,
            model: String::new(),
            project: p.project.clone(),
            desk: None,
            activity: SessionActivity::default(),
            waits: Vec::new(),
            outcome: None,
            observed: None,
            owner: p.owner.clone(),
            remote: p.remote,
            started_ms: now,
            last_event_ms: now,
        });
        n.title = name.to_owned();
        n.name = Some(name.to_owned());
        if let Some(a) = agent {
            n.agent = Some(a.clone());
        }
        if let Some(m) = model {
            n.model = m.clone();
        }
        if let Some(o) = observed {
            n.kind = GraphNodeKind::Observed;
            n.activity.state = observed_state(o, &n.activity.state);
            n.outcome = observed_outcome(o);
            n.observed = Some(o.clone());
        }
        n.last_event_ms = n.last_event_ms.max(now);
        self.mark(&id, now);
        self.mark(&p.id, now);
        true
    }

    /// Add sessions read for an unknown id (it and its ancestors); private and advisor rows never arrive here
    /// (the durable store does not hold private trees; `node_kind` drops advisors).
    pub(crate) fn adopt(
        &mut self,
        rows: &[SessionSummary],
        stops: &HashMap<SessionId, String>,
        last: &HashMap<SessionId, u64>,
        now: u64,
    ) -> Option<u64> {
        let tree = forest(rows);
        let mut sorted: Vec<&SessionSummary> =
            rows.iter().filter(|s| tree.contains_key(&s.id)).collect();
        sorted.sort_by_key(|s| tree[&s.id].1);
        let mut at = None;
        for s in sorted {
            let id = s.id.to_string();
            if self.nodes.contains_key(&id) {
                continue;
            }
            // The root: the topmost ancestor among `rows`, or the model's root of the nearest known ancestor.
            let root = s
                .parent
                .and_then(|p| self.nodes.get(&p.to_string()))
                .and_then(|p| p.root.parse().ok())
                .unwrap_or(tree[&s.id].0);
            if let Some(n) = node_of(s, root, last_of(s, last), None, None, stops.get(&s.id)) {
                self.nodes.insert(id.clone(), n);
                at = at.or(self.mark(&id, now));
            }
        }
        at
    }

    /// Record an exchange's state change.
    pub(crate) fn peer(&mut self, ev: &PeerMessageChanged, now: u64) -> Option<u64> {
        if !self.seen.insert((ev.id, ev.state)) {
            return None;
        }
        let (Some(from), Some(to)) = (endpoint(&ev.from), endpoint(&ev.to)) else {
            return None;
        };
        if from == to {
            return None;
        }
        let (edge, a, b) = pair(&from, &to);
        let kind = match ev.state {
            PeerMessageState::Accepted if ev.result => Some(GraphPulseKind::Result),
            PeerMessageState::Accepted => Some(GraphPulseKind::Sent),
            PeerMessageState::Consumed => Some(GraphPulseKind::Delivered),
            PeerMessageState::Replied => Some(GraphPulseKind::Replied),
            PeerMessageState::Suppressed | PeerMessageState::Failed => Some(GraphPulseKind::Failed),
            PeerMessageState::Held | PeerMessageState::Unknown => None,
        };
        let e = self.edges.entry(edge.clone()).or_insert_with(|| GraphEdge {
            id: edge.clone(),
            a: a.clone(),
            b,
            ab: 0,
            ba: 0,
            results: 0,
            open: 0,
            last_ms: 0,
            last_from: String::new(),
            last_id: String::new(),
        });
        match ev.state {
            PeerMessageState::Accepted => {
                if from == a {
                    e.ab += 1;
                } else {
                    e.ba += 1;
                }
                if ev.result {
                    e.results += 1;
                }
                e.open += 1;
            }
            PeerMessageState::Consumed
            | PeerMessageState::Replied
            | PeerMessageState::Suppressed
            | PeerMessageState::Failed => {
                if self.closed.insert(ev.id) {
                    e.open = e.open.saturating_sub(1);
                }
            }
            PeerMessageState::Held | PeerMessageState::Unknown => {}
        }
        e.last_ms = now;
        e.last_from = from.clone();
        e.last_id = ev.id.to_string();
        for end in [&from, &to] {
            if end.starts_with("peer:") {
                let n = self
                    .nodes
                    .entry(end.clone())
                    .or_insert_with(|| peer_node(end, now));
                n.last_event_ms = now;
                self.dirty.insert(end.clone());
            }
        }
        if let Some(kind) = kind {
            self.pulses.push(GraphPulse {
                edge: edge.clone(),
                from,
                to,
                kind,
                id: ev.id.to_string(),
                at_ms: now,
            });
        }
        self.dirty_edges.insert(edge);
        self.schedule(now)
    }

    /// Take the dirty ids for a flush; later marks schedule the next one `FLUSH_MS` after `now`.
    pub(crate) fn begin_flush(&mut self, now: u64) -> FlushBatch {
        self.flush_at = None;
        self.last_flush = now;
        FlushBatch {
            nodes: self.dirty.drain().collect(),
            edges: self.dirty_edges.drain().collect(),
        }
    }

    fn depth(&self, id: &str) -> usize {
        let mut d = 0;
        let mut cur = self.nodes.get(id).and_then(|n| n.parent.clone());
        while let Some(p) = cur {
            d += 1;
            if d > 64 {
                break;
            }
            cur = self.nodes.get(&p).and_then(|n| n.parent.clone());
        }
        d
    }

    fn ordered(&self, ids: impl IntoIterator<Item = String>) -> Vec<GraphNode> {
        let mut v: Vec<(usize, u64, String)> = ids
            .into_iter()
            .filter_map(|id| {
                self.nodes
                    .get(&id)
                    .map(|n| (self.depth(&id), n.started_ms, id))
            })
            .collect();
        v.sort();
        v.into_iter()
            .map(|(.., id)| self.nodes[&id].clone())
            .collect()
    }

    fn drawn(&self, e: &GraphEdge) -> bool {
        self.nodes.contains_key(&e.a) && self.nodes.contains_key(&e.b)
    }

    /// Apply a flush's reads and build the delta; `None` when nothing visible changed.
    pub(crate) fn finish_flush(
        &mut self,
        batch: FlushBatch,
        activity: &HashMap<SessionId, SessionActivity>,
        waits: &HashMap<SessionId, Vec<String>>,
        last: &HashMap<SessionId, u64>,
        now: u64,
    ) -> Option<AgentGraphChanged> {
        for id in &batch.nodes {
            let Some(n) = self.nodes.get_mut(id) else {
                continue;
            };
            let Ok(sid) = id.parse::<SessionId>() else {
                continue;
            };
            if let Some(t) = last.get(&sid) {
                n.last_event_ms = n.last_event_ms.max(*t);
            }
            if n.remote {
                continue;
            }
            if let Some(a) = activity.get(&sid) {
                let mut a = a.clone();
                if let Some(o) = &n.observed {
                    a.state = observed_state(o, &a.state);
                }
                if matches!(a.state, AgentState::Running) && n.observed.is_none() {
                    n.outcome = None;
                }
                n.activity = a;
            }
            n.waits = waits_of(waits.get(&sid));
        }
        let changed: Vec<String> = batch
            .nodes
            .into_iter()
            .filter(|id| {
                self.nodes
                    .get(id)
                    .is_some_and(|n| self.published.get(id) != Some(n))
            })
            .collect();
        let edges: Vec<GraphEdge> = batch
            .edges
            .iter()
            .filter_map(|id| self.edges.get(id))
            .filter(|e| self.drawn(e) && self.published_edges.get(&e.id) != Some(*e))
            .cloned()
            .collect();
        let pulses = std::mem::take(&mut self.pulses);
        let pulses: Vec<GraphPulse> = pulses
            .into_iter()
            .filter(|p| self.edges.get(&p.edge).is_some_and(|e| self.drawn(e)))
            .collect();
        let skip = pulses.len().saturating_sub(MAX_PULSES);
        let pulses: Vec<GraphPulse> = pulses.into_iter().skip(skip).collect();
        let removed = std::mem::take(&mut self.removed);
        let edges_removed = std::mem::take(&mut self.edges_removed);
        let older_changed = self.older != self.published_older;
        if changed.is_empty()
            && edges.is_empty()
            && pulses.is_empty()
            && removed.is_empty()
            && edges_removed.is_empty()
            && !older_changed
        {
            return None;
        }
        self.seq += 1;
        let mut ev = AgentGraphChanged {
            epoch_ms: self.epoch_ms,
            seq: self.seq,
            at_ms: now,
            nodes: Vec::new(),
            removed: Vec::new(),
            edges: Vec::new(),
            edges_removed: Vec::new(),
            pulses: Vec::new(),
            older: self.older.clone(),
            resync: false,
        };
        self.published_older = self.older.clone();
        if changed.len() + edges.len() > MAX_UPSERTS {
            // Too much: clients re-snapshot; what they will read is the model as it is now.
            ev.resync = true;
            ev.older = None;
            self.published = self.nodes.clone();
            self.published_edges = self
                .edges
                .iter()
                .filter(|(_, e)| self.drawn(e))
                .map(|(k, e)| (k.clone(), e.clone()))
                .collect();
            return Some(ev);
        }
        for id in &changed {
            self.published.insert(id.clone(), self.nodes[id].clone());
        }
        for e in &edges {
            self.published_edges.insert(e.id.clone(), e.clone());
        }
        ev.nodes = self.ordered(changed);
        ev.edges = edges;
        ev.removed = removed;
        ev.edges_removed = edges_removed;
        ev.pulses = pulses;
        Some(ev)
    }

    /// The `agent_graph` reply.
    pub(crate) fn reply(&self, cursor: Option<GraphCursor>, now: u64) -> AgentGraph {
        let current = cursor.is_some_and(|c| c.epoch_ms == self.epoch_ms && c.seq == self.seq);
        let (nodes, edges) = if current {
            (Vec::new(), Vec::new())
        } else {
            let mut edges: Vec<GraphEdge> = self
                .edges
                .values()
                .filter(|e| self.drawn(e))
                .cloned()
                .collect();
            edges.sort_by(|x, y| x.id.cmp(&y.id));
            (self.ordered(self.nodes.keys().cloned()), edges)
        };
        AgentGraph {
            epoch_ms: self.epoch_ms,
            seq: self.seq,
            at_ms: now,
            window_ms: self.window_ms,
            unchanged: current,
            nodes,
            edges,
            older: if current { None } else { self.older.clone() },
        }
    }

    /// Add `include` ids; returns the ones the model lacks (to read).
    pub(crate) fn include(&mut self, ids: &[SessionId]) -> Vec<SessionId> {
        let mut missing = Vec::new();
        for id in ids {
            let s = id.to_string();
            if !self.nodes.contains_key(&s) {
                missing.push(*id);
            }
            self.include.insert(s);
        }
        missing
    }

    /// 208 §4.3 (6): drop idle nodes that left the window (no descendant, not included), and their edges. Silent.
    pub(crate) fn prune(&mut self, now: u64) {
        let bound = now.saturating_sub(self.window_ms).saturating_sub(60_000);
        loop {
            let parents: HashSet<String> = self
                .nodes
                .values()
                .filter_map(|n| n.parent.clone())
                .collect();
            let gone: Vec<String> = self
                .nodes
                .values()
                .filter(|n| {
                    !is_live(n)
                        && !self.include.contains(&n.id)
                        && !parents.contains(&n.id)
                        && n.last_event_ms < bound
                        && !self
                            .edges
                            .values()
                            .any(|e| (e.a == n.id || e.b == n.id) && e.last_ms >= bound)
                })
                .map(|n| n.id.clone())
                .collect();
            if gone.is_empty() {
                break;
            }
            for id in gone {
                self.nodes.remove(&id);
                self.published.remove(&id);
                self.dirty.remove(&id);
            }
        }
        let nodes = &self.nodes;
        self.edges
            .retain(|_, e| nodes.contains_key(&e.a) && nodes.contains_key(&e.b));
        let edges = &self.edges;
        self.published_edges.retain(|k, _| edges.contains_key(k));
    }
}

/// `lease_until_ms` passed: the model goes, silently.
pub(crate) fn lease_expired(lease_until_ms: u64, now: u64) -> bool {
    now > lease_until_ms
}

/// The sessions an event moves the clock of.
fn event_sessions(ev: &AgentEvent) -> Vec<SessionId> {
    match ev {
        AgentEvent::SubagentSpawned { parent, child, .. }
        | AgentEvent::ObservedChildChanged { parent, child, .. } => vec![*child, *parent],
        other => other.session().into_iter().collect(),
    }
}

/// `ask` → `anyhow::Result<AgentGraph>`: the `agent_graph` RPC.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Requested window (null = default).
    pub window_ms: Option<u64>,
    /// Sessions to show regardless of the window.
    pub include: Vec<SessionId>,
    /// The caller's place in the delta stream.
    pub cursor: Option<GraphCursor>,
}

type Waiter = (Snapshot, Option<ReplySender<anyhow::Result<AgentGraph>>>);

/// An input buffered while a seed runs.
enum Buffered {
    Agent(Box<AgentEvent>),
    Peer(PeerMessageChanged),
}

struct Seeding {
    waiters: Vec<Waiter>,
    buffered: Vec<Buffered>,
    /// The epoch of the model this seed replaces (0 = none): the new one must be later.
    prev_epoch: u64,
}

/// 208 §4: the graph actor. Dormant: only the clock; leased: the model and the delta stream.
#[derive(kameo::Actor)]
pub struct Graph {
    rt: Runtime,
    store: Store,
    agents: ActorRef<Agents>,
    last_event: HashMap<SessionId, u64>,
    unsaved: HashSet<SessionId>,
    /// Approval / ask id → session, to place their resolutions (which carry no session).
    prompts: HashMap<String, SessionId>,
    model: Option<Model>,
    lease_until_ms: u64,
    seeding: Option<Seeding>,
    flushing: bool,
    /// The model epoch the flush in flight began on.
    flush_epoch: u64,
    again: bool,
}

impl Graph {
    /// The actor over `store` and `agents`, not yet subscribed or registered.
    pub fn new(rt: Runtime, store: Store, agents: ActorRef<Agents>) -> Self {
        Self {
            rt,
            store,
            agents,
            last_event: HashMap::new(),
            unsaved: HashSet::new(),
            prompts: HashMap::new(),
            model: None,
            lease_until_ms: 0,
            seeding: None,
            flushing: false,
            flush_epoch: 0,
            again: false,
        }
    }

    /// Spawn, subscribe, register as [`GRAPH_ACTOR`] and arm the persist and prune ticks.
    pub async fn start(
        rt: &Runtime,
        store: Store,
        agents: ActorRef<Agents>,
    ) -> anyhow::Result<ActorRef<Self>> {
        let actor = Self::spawn_with_mailbox(
            Self::new(rt.clone(), store, agents),
            runtime::subscriber_mailbox(),
        );
        actor.register(GRAPH_ACTOR)?;
        Self::subscribe(rt, &actor).await;
        let weak = actor.downgrade();
        tokio::spawn(async move {
            let mut persist = tokio::time::interval(std::time::Duration::from_secs(PERSIST_SECS));
            let mut prune = tokio::time::interval(std::time::Duration::from_secs(PRUNE_SECS));
            persist.tick().await;
            prune.tick().await;
            loop {
                let msg = tokio::select! {
                    _ = persist.tick() => Tick::Persist,
                    _ = prune.tick() => Tick::Prune,
                };
                let Some(me) = weak.upgrade() else {
                    break;
                };
                if me.tell(msg).await.is_err() {
                    break;
                }
            }
        });
        Ok(actor)
    }

    /// The bus subscriptions (also used by tests).
    pub(crate) async fn subscribe(rt: &Runtime, actor: &ActorRef<Self>) {
        rt.subscribe::<Self, AgentEvent>(actor).await;
        rt.subscribe::<Self, PeerMessageChanged>(actor).await;
        rt.subscribe::<Self, PeerDelivered>(actor).await;
        rt.subscribe::<Self, tools::ApprovalRequested>(actor).await;
        rt.subscribe::<Self, tools::ApprovalResolved>(actor).await;
        rt.subscribe::<Self, tools::AskRequested>(actor).await;
        rt.subscribe::<Self, tools::AskResolved>(actor).await;
        rt.subscribe::<Self, tools::jobs::JobEvent>(actor).await;
    }

    fn touch(&mut self, s: SessionId, now: u64) {
        self.last_event.insert(s, now);
        self.unsaved.insert(s);
    }

    fn arm(&self, at: Option<u64>, me: &ActorRef<Self>, now: u64) {
        if let Some(at) = at {
            let me = me.clone();
            let wait = at.saturating_sub(now);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                let _ = me.tell(FlushTick).await;
            });
        }
    }

    /// Mark `s` (delta-class → `bump`) in the model; schedule as needed.
    fn mark(&mut self, s: SessionId, quiet: bool, me: &ActorRef<Self>, now: u64) {
        let at = match self.model.as_mut() {
            Some(m) if quiet => m.bump(&s.to_string(), now),
            Some(m) => m.mark(&s.to_string(), now),
            None => None,
        };
        self.arm(at, me, now);
    }

    fn adopt_later(&self, ids: Vec<SessionId>, me: &ActorRef<Self>) {
        if ids.is_empty() {
            return;
        }
        let (store, me) = (self.store.clone(), me.clone());
        tokio::spawn(async move {
            let mut rows: Vec<SessionSummary> = Vec::new();
            for id in ids {
                let mut cur = Some(id);
                let mut hops = 0;
                while let Some(c) = cur.filter(|c| !rows.iter().any(|r| r.id == *c)) {
                    match store.session(c).await {
                        Ok(Some(s)) => {
                            cur = s.parent;
                            rows.push(s);
                        }
                        _ => break,
                    }
                    hops += 1;
                    if hops > 64 {
                        break;
                    }
                }
            }
            if rows.is_empty() {
                return;
            }
            let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
            let stops = store.last_stops(&ids).await.unwrap_or_default();
            let _ = me.tell(Adopt { rows, stops }).await;
        });
    }

    fn on_agent(&mut self, ev: AgentEvent, me: &ActorRef<Self>, now: u64) {
        let sessions = event_sessions(&ev);
        for s in &sessions {
            self.touch(*s, now);
        }
        if let AgentEvent::SessionDeleted { session } = &ev {
            self.last_event.remove(session);
            self.unsaved.remove(session);
            let (store, id) = (self.store.clone(), *session);
            tokio::spawn(async move {
                if let Err(e) = store.delete_last_event(id).await {
                    tracing::warn!(%id, "graph: delete_last_event: {e:#}");
                }
            });
        }
        if let Some(s) = self.seeding.as_mut() {
            if s.buffered.len() < MAX_BUFFERED {
                s.buffered.push(Buffered::Agent(Box::new(ev)));
            }
            return;
        }
        self.apply_agent(ev, me, now);
    }

    fn apply_agent(&mut self, ev: AgentEvent, me: &ActorRef<Self>, now: u64) {
        let Some(m) = self.model.as_mut() else {
            return;
        };
        let mut adopt = Vec::new();
        // Structure edits mark nodes directly; whichever mark scheduled the flush, the timer is armed once below.
        let idle = m.flush_at.is_none();
        let mut quiet = false;
        match &ev {
            AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
                agent,
                model,
                ..
            } => {
                if !m.child(
                    *parent,
                    *child,
                    name,
                    agent.as_ref(),
                    model.as_ref(),
                    None,
                    now,
                ) {
                    adopt.push(*child);
                }
            }
            AgentEvent::ObservedChildChanged {
                parent,
                child,
                name,
                model,
                observed,
            } => {
                if !m.child(
                    *parent,
                    *child,
                    name,
                    None,
                    model.as_ref(),
                    Some(observed),
                    now,
                ) {
                    adopt.push(*child);
                }
            }
            AgentEvent::TurnStarted { session, .. } => {
                match m.nodes.get_mut(&session.to_string()) {
                    Some(n) => {
                        n.outcome = None;
                        if n.remote {
                            n.activity.state = AgentState::Running;
                        }
                    }
                    None => adopt.push(*session),
                }
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                if let Some(n) = m.nodes.get_mut(&session.to_string()) {
                    n.outcome = outcome_of(stop_reason);
                    if n.remote {
                        n.activity.state = AgentState::Idle;
                    }
                }
            }
            AgentEvent::SessionDeleted { session } => {
                m.remove(&session.to_string(), now);
            }
            AgentEvent::TitleChanged { session, title, .. } => {
                if let Some(n) = m.nodes.get_mut(&session.to_string())
                    && matches!(n.kind, GraphNodeKind::Root | GraphNodeKind::Desk)
                {
                    n.title = title.clone();
                }
            }
            AgentEvent::ModelChanged { session, model } => {
                if let Some(n) = m.nodes.get_mut(&session.to_string()) {
                    n.model = model.clone();
                }
            }
            AgentEvent::SessionOwnerChanged {
                session,
                owner,
                here,
                ..
            } => {
                if let Some(n) = m.nodes.get_mut(&session.to_string()) {
                    n.owner = owner.clone();
                    n.remote = !here;
                }
            }
            AgentEvent::TextDelta { .. }
            | AgentEvent::ReasoningDelta { .. }
            | AgentEvent::ToolCallProgress { .. }
            | AgentEvent::VisionProgress { .. } => quiet = true,
            _ => {}
        }
        for s in event_sessions(&ev) {
            let id = s.to_string();
            if quiet {
                m.bump(&id, now);
            } else {
                m.mark(&id, now);
            }
        }
        let at = m.flush_at.filter(|_| idle);
        self.arm(at, me, now);
        self.adopt_later(adopt, me);
    }

    fn on_peer(&mut self, ev: PeerMessageChanged, me: &ActorRef<Self>, now: u64) {
        for end in [&ev.from, &ev.to] {
            if let Some(s) = endpoint(end).and_then(|e| e.parse().ok()) {
                self.touch(s, now);
            }
        }
        self.touch(ev.session, now);
        if let Some(s) = self.seeding.as_mut() {
            if s.buffered.len() < MAX_BUFFERED {
                s.buffered.push(Buffered::Peer(ev));
            }
            return;
        }
        let at = self.model.as_mut().and_then(|m| m.peer(&ev, now));
        self.arm(at, me, now);
        self.mark(ev.session, false, me, now);
    }

    fn on_session(&mut self, s: SessionId, quiet: bool, me: &ActorRef<Self>, now: u64) {
        self.touch(s, now);
        if self.seeding.is_none() {
            self.mark(s, quiet, me, now);
        }
    }

    /// Answer a `Snapshot` against the model (the seed has run).
    fn answer(&mut self, m: Snapshot, me: &ActorRef<Self>, now: u64) -> anyhow::Result<AgentGraph> {
        let model = self
            .model
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("graph: no model"))?;
        // `window_of(&m) <= model.window_ms` here: a wider ask re-seeds (`Message<Snapshot>`).
        let include: Vec<SessionId> = m.include.into_iter().take(MAX_INCLUDE).collect();
        let missing = model.include(&include);
        let reply = model.reply(m.cursor, now);
        self.adopt_later(missing, me);
        Ok(reply)
    }

    async fn seed(
        store: Store,
        agents: ActorRef<Agents>,
        mem: HashMap<SessionId, u64>,
        include: Vec<SessionId>,
        now: u64,
        window: u64,
    ) -> anyhow::Result<Seed> {
        let sessions = agents
            .ask(ListSessions)
            .await
            .map_err(|e| anyhow::anyhow!("list sessions: {e}"))?;
        let rows: Vec<AgentInfo> = agents
            .ask(ListActivity)
            .await
            .map_err(|e| anyhow::anyhow!("list activity: {e}"))?;
        let private: HashSet<SessionId> = rows
            .iter()
            .filter(|r| r.retention == tools::Retention::Private)
            .map(|r| r.id)
            .collect();
        let live: HashSet<SessionId> = rows
            .iter()
            .filter(|r| {
                r.activity.live
                    || r.activity.jobs + r.activity.ptys > 0
                    || matches!(
                        r.state,
                        AgentState::Running
                            | AgentState::Awaiting { .. }
                            | AgentState::Approval { .. }
                    )
            })
            .map(|r| r.id)
            .collect();
        let sessions: Vec<SessionSummary> = sessions
            .into_iter()
            .filter(|s| !private.contains(&s.id))
            .collect();
        let ids: Vec<SessionId> = sessions.iter().map(|s| s.id).collect();
        let mut last = store.last_events(&ids).await?;
        for (id, t) in mem {
            let e = last.entry(id).or_insert(t);
            *e = (*e).max(t);
        }
        let (chosen, older) =
            select_window(&sessions, &last, &live, &include, now, window, MAX_NODES);
        let since = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            i64::try_from(now.saturating_sub(window)).unwrap_or(0),
        )
        .unwrap_or_default();
        let ex = store.exchanges_since(since, HISTORY_ROWS).await?;
        // Sessions an exchange brings in need their live facts too.
        let mut want: HashSet<SessionId> = chosen.iter().copied().collect();
        for r in &ex {
            for s in [&r.sender_session, &r.recipient_session]
                .into_iter()
                .flatten()
            {
                if let Ok(id) = s.parse() {
                    want.insert(id);
                }
            }
        }
        let want: Vec<SessionId> = want.into_iter().collect();
        let activity = agents
            .ask(ActivityOf(want.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("activity: {e}"))?;
        let waits = store.graph_waits(&want).await?;
        let stops = store.last_stops(&want).await?;
        Ok(Seed {
            sessions,
            last,
            activity,
            waits,
            stops,
            rows: ex,
            chosen,
            older,
        })
    }
}

/// Internal ticks.
enum Tick {
    Persist,
    Prune,
}

/// Internal: a scheduled flush is due.
struct FlushTick;

/// Internal: a flush's reads.
struct Flushed {
    batch: FlushBatch,
    activity: HashMap<SessionId, SessionActivity>,
    waits: HashMap<SessionId, Vec<String>>,
}

/// Internal: a seed's reads.
struct Seeded {
    seed: anyhow::Result<Seed>,
    now: u64,
    window: u64,
    include: Vec<SessionId>,
}

/// Internal: rows read for sessions the model lacked.
struct Adopt {
    rows: Vec<SessionSummary>,
    stops: HashMap<SessionId, String>,
}

impl Message<Snapshot> for Graph {
    type Reply = DelegatedReply<anyhow::Result<AgentGraph>>;

    async fn handle(&mut self, m: Snapshot, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let now = now_ms();
        self.lease_until_ms = now + LEASE_SECS * 1000;
        if let Some(s) = self.seeding.as_mut() {
            s.waiters.push((m, tx));
            return reply;
        }
        let window = window_of(&m);
        let mut include: Vec<SessionId> = m.include.iter().copied().take(MAX_INCLUDE).collect();
        let mut prev_epoch = 0;
        if let Some(model) = self.model.as_ref() {
            if window <= model.window_ms {
                let me = ctx.actor_ref().clone();
                let r = self.answer(m, &me, now);
                if let Some(tx) = tx {
                    tx.send(r);
                }
                return reply;
            }
            // A wider window than the seed read: older trees are only in the store, so seed again (a new epoch;
            // every client re-snapshots on its next delta or renewal).
            prev_epoch = model.epoch_ms;
            for s in model.include.iter().filter_map(|s| s.parse().ok()) {
                if include.len() < MAX_INCLUDE && !include.contains(&s) {
                    include.push(s);
                }
            }
            self.model = None;
        }
        let seeding = Seeding {
            waiters: vec![(m, tx)],
            buffered: Vec::new(),
            prev_epoch,
        };
        self.start_seed(seeding, include, window, now, ctx.actor_ref().clone());
        reply
    }
}

/// The window a `Snapshot` asks for (null = default; clamped).
fn window_of(m: &Snapshot) -> u64 {
    m.window_ms.unwrap_or(DEFAULT_WINDOW_MS).min(MAX_WINDOW_MS)
}

impl Graph {
    /// Run a seed over `window` in the background; its reads arrive as [`Seeded`].
    fn start_seed(
        &mut self,
        seeding: Seeding,
        include: Vec<SessionId>,
        window: u64,
        now: u64,
        me: ActorRef<Self>,
    ) {
        self.seeding = Some(seeding);
        let (store, agents, mem) = (
            self.store.clone(),
            self.agents.clone(),
            self.last_event.clone(),
        );
        tokio::spawn(async move {
            let seed = Self::seed(store, agents, mem, include.clone(), now, window).await;
            let _ = me
                .tell(Seeded {
                    seed,
                    now,
                    window,
                    include,
                })
                .await;
        });
    }
}

impl Message<Seeded> for Graph {
    type Reply = ();

    async fn handle(&mut self, m: Seeded, ctx: &mut Context<Self, ()>) {
        let Some(seeding) = self.seeding.take() else {
            return;
        };
        let me = ctx.actor_ref().clone();
        // A caller asked a wider window while this seed ran: seed again for it before answering anyone.
        let wider = seeding.waiters.iter().map(|(s, _)| window_of(s)).max();
        if m.seed.is_ok()
            && let Some(w) = wider.filter(|w| *w > m.window)
        {
            self.start_seed(seeding, m.include, w, now_ms(), me);
            return;
        }
        match m.seed {
            Ok(seed) => {
                let mut model = Model::build(seed, m.now, m.window, &m.include);
                model.epoch_ms = model.epoch_ms.max(seeding.prev_epoch + 1);
                self.model = Some(model);
                let now = now_ms();
                for b in seeding.buffered {
                    match b {
                        Buffered::Agent(ev) => self.apply_agent(*ev, &me, now),
                        Buffered::Peer(ev) => {
                            let at = self.model.as_mut().and_then(|md| md.peer(&ev, now));
                            self.arm(at, &me, now);
                            self.mark(ev.session, false, &me, now);
                        }
                    }
                }
                for (snap, tx) in seeding.waiters {
                    let r = self.answer(snap, &me, now);
                    if let Some(tx) = tx {
                        tx.send(r);
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::error!("graph: seed: {msg}");
                for (_, tx) in seeding.waiters {
                    if let Some(tx) = tx {
                        tx.send(Err(anyhow::anyhow!("graph seed: {msg}")));
                    }
                }
            }
        }
    }
}

impl Message<FlushTick> for Graph {
    type Reply = ();

    async fn handle(&mut self, _: FlushTick, ctx: &mut Context<Self, ()>) {
        if self.flushing {
            self.again = true;
            return;
        }
        let now = now_ms();
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let batch = model.begin_flush(now);
        let ids = batch.sessions(model);
        self.flushing = true;
        self.flush_epoch = model.epoch_ms;
        let (store, agents, me) = (
            self.store.clone(),
            self.agents.clone(),
            ctx.actor_ref().clone(),
        );
        tokio::spawn(async move {
            let (activity, waits) = if ids.is_empty() {
                (HashMap::new(), HashMap::new())
            } else {
                let activity = agents
                    .ask(ActivityOf(ids.clone()))
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("graph: activity: {e}");
                        HashMap::new()
                    });
                let waits = store.graph_waits(&ids).await.unwrap_or_else(|e| {
                    tracing::warn!("graph: waits: {e:#}");
                    HashMap::new()
                });
                (activity, waits)
            };
            let _ = me
                .tell(Flushed {
                    batch,
                    activity,
                    waits,
                })
                .await;
        });
    }
}

impl Message<Flushed> for Graph {
    type Reply = ();

    async fn handle(&mut self, m: Flushed, ctx: &mut Context<Self, ()>) {
        self.flushing = false;
        let now = now_ms();
        let Some(model) = self.model.as_mut() else {
            return;
        };
        // A flush begun on a model a wider-window re-seed replaced: its batch names the old model's state.
        let ev = if model.epoch_ms == self.flush_epoch {
            model.finish_flush(m.batch, &m.activity, &m.waits, &self.last_event, now)
        } else {
            None
        };
        let again = std::mem::take(&mut self.again).then(|| {
            model.flush_at = None;
            model.schedule(now)
        });
        if let Some(ev) = ev {
            self.rt.publish(ev).await;
        }
        let me = ctx.actor_ref().clone();
        self.arm(again.flatten(), &me, now);
    }
}

impl Message<Adopt> for Graph {
    type Reply = ();

    async fn handle(&mut self, m: Adopt, ctx: &mut Context<Self, ()>) {
        let now = now_ms();
        let at = self
            .model
            .as_mut()
            .and_then(|model| model.adopt(&m.rows, &m.stops, &self.last_event, now));
        let me = ctx.actor_ref().clone();
        self.arm(at, &me, now);
    }
}

impl Message<Tick> for Graph {
    type Reply = ();

    async fn handle(&mut self, t: Tick, _: &mut Context<Self, ()>) {
        let now = now_ms();
        match t {
            Tick::Persist => {
                if self.unsaved.is_empty() {
                    return;
                }
                let rows: Vec<(SessionId, u64)> = self
                    .unsaved
                    .drain()
                    .filter_map(|s| self.last_event.get(&s).map(|t| (s, *t)))
                    .collect();
                let store = self.store.clone();
                tokio::spawn(async move {
                    if let Err(e) = store.upsert_last_events(&rows).await {
                        tracing::warn!("graph: upsert_last_events: {e:#}");
                    }
                });
            }
            Tick::Prune => {
                if lease_expired(self.lease_until_ms, now) {
                    self.model = None;
                } else if let Some(m) = self.model.as_mut() {
                    m.prune(now);
                }
            }
        }
    }
}

impl Message<AgentEvent> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, ctx: &mut Context<Self, ()>) {
        let me = ctx.actor_ref().clone();
        self.on_agent(ev, &me, now_ms());
    }
}

impl Message<PeerMessageChanged> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: PeerMessageChanged, ctx: &mut Context<Self, ()>) {
        let me = ctx.actor_ref().clone();
        self.on_peer(ev, &me, now_ms());
    }
}

impl Message<PeerDelivered> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: PeerDelivered, ctx: &mut Context<Self, ()>) {
        let me = ctx.actor_ref().clone();
        self.on_session(ev.session, false, &me, now_ms());
    }
}

impl Message<tools::ApprovalRequested> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: tools::ApprovalRequested, ctx: &mut Context<Self, ()>) {
        self.prompts.insert(format!("a:{}", ev.id.0), ev.session);
        let me = ctx.actor_ref().clone();
        self.on_session(ev.session, false, &me, now_ms());
    }
}

impl Message<tools::ApprovalResolved> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: tools::ApprovalResolved, ctx: &mut Context<Self, ()>) {
        if let Some(s) = self.prompts.remove(&format!("a:{}", ev.id.0)) {
            let me = ctx.actor_ref().clone();
            self.on_session(s, false, &me, now_ms());
        }
    }
}

impl Message<tools::AskRequested> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: tools::AskRequested, ctx: &mut Context<Self, ()>) {
        self.prompts.insert(format!("q:{}", ev.id.0), ev.session);
        let me = ctx.actor_ref().clone();
        self.on_session(ev.session, false, &me, now_ms());
    }
}

impl Message<tools::AskResolved> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: tools::AskResolved, ctx: &mut Context<Self, ()>) {
        if let Some(s) = self.prompts.remove(&format!("q:{}", ev.id.0)) {
            let me = ctx.actor_ref().clone();
            self.on_session(s, false, &me, now_ms());
        }
    }
}

impl Message<tools::jobs::JobEvent> for Graph {
    type Reply = ();

    async fn handle(&mut self, ev: tools::jobs::JobEvent, ctx: &mut Context<Self, ()>) {
        use tools::jobs::JobEvent as J;
        let (s, quiet) = match &ev {
            J::Output { session, .. } | J::Screen { session, .. } => (*session, true),
            J::Started { session, .. } | J::Idle { session, .. } | J::Finished { session, .. } => {
                (*session, false)
            }
        };
        let me = ctx.actor_ref().clone();
        self.on_session(s, quiet, &me, now_ms());
    }
}

#[cfg(test)]
mod tests;
