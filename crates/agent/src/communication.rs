//! Design doc 50: agent communication contracts. Wire DTOs and owner
//! commands/events (§§5, 10), the bounds of §8, and [`Policy`]: the pure
//! availability predicate (§2.1) with the generation barrier of §8. Nothing
//! here touches sqlite, actors, or the model loop; `Agents` owns one
//! [`Policy`], serializes every call through its mailbox, and mirrors
//! persisted topology into it before routing.
//!
//! Availability is not approval (§0.5): a `Generation` says a route exists,
//! `Risk → PolicyAction` still decides whether a tool call runs.

use std::collections::{HashMap, HashSet};
use std::fmt;

use chrono::{DateTime, Utc};
use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::continuation::InputTicket;

/// Network (A2A) request/subscription state and its `Agents` handlers (§6);
/// storage only through agent commands, never a raw DB handle.
pub mod network;

/// Peer messages across engines (69 P): the `peer_deliver` wire on
/// `theuth/rpc/1` and the engine-name lookup for `discover_peers`.
pub mod mesh;

/// `tell` (runner → `Agents`): a peer exchange was linked into `session`'s
/// history at `seq` inside `turn` (§7 consumption transaction). Sent after
/// the runner's `InputConsumed` for the same ticket, which is what binds the
/// turn to an A2A request scope (network ownership is by ticket, not here).
#[derive(Clone, Copy, Debug)]
pub struct PeerConsumed {
    /// Recipient.
    pub session: SessionId,
    /// Exchange id.
    pub id: Uuid,
    /// The turn that consumed it.
    pub turn: TurnId,
    /// History row it became.
    pub seq: i64,
}

/// `tell` (runner → `Agents`): an accepted exchange never reached the model
/// (revoked generation, suppressed, cancelled scope, closed mailbox). The
/// ticket is released and the sender's receipt reflects the outcome.
#[derive(Clone, Debug)]
pub struct PeerIntakeFailed {
    /// Recipient.
    pub session: SessionId,
    /// Exchange id.
    pub id: Uuid,
    /// The admission the runner was handed.
    pub ticket: InputTicket,
    /// Why; persisted as the exchange error/suppression reason.
    pub reason: String,
}

/// `tell` (runner → `Agents`): the final result of a turn that consumed at
/// least one peer exchange, extracted by that turn's own rows (§7), sent
/// after persistence and before `AgentEvent::TurnFinished`.
#[derive(Clone, Debug)]
pub struct PeerTurnFinished {
    /// The session whose turn ended.
    pub session: SessionId,
    /// The turn.
    pub turn: TurnId,
    /// The request scope the turn's peer input belonged to, when any.
    pub scope: Option<Uuid>,
    /// Assistant text persisted in `turn`.
    pub text: String,
    /// The turn's stop reason.
    pub stop_reason: String,
}

/// Max UTF-8 bytes of one peer message body before wrapping (§8). Oversized
/// requests are rejected, never truncated.
pub const PEER_TEXT_MAX: usize = 32 * 1024;
/// `discover_peers` / [`ListPeers`] default page size (§5.1).
pub const DISCOVER_DEFAULT_LIMIT: u16 = 25;
/// `discover_peers` / [`ListPeers`] maximum page size (§5.1).
pub const DISCOVER_MAX_LIMIT: u16 = 100;
/// Accepted, unconsumed peer inputs per recipient (§8); beyond it: busy.
pub const PENDING_PER_RECIPIENT: usize = 64;
/// Accepted, unconsumed peer inputs per harness (§8).
pub const PENDING_PER_HARNESS: usize = 1024;
/// Pending reply waits per session (§8): an orchestrator awaiting its children.
pub const WAITS_PER_SESSION: usize = 32;
/// Pending reply waits per harness (§8): ~10 orchestrators × 20 children.
pub const WAITS_PER_HARNESS: usize = 1024;
/// Peer-triggered continuations per initiating request (§8). Every consumed
/// child result counts, so an autonomous orchestrator needs room for all of
/// its children's results and their follow-ups.
pub const CHAIN_MAX: u32 = 256;
/// Max chars of an owner-chosen root alias and of a remote `peer:` name.
pub const ALIAS_MAX: usize = 64;

/// Root communication scope (§2.3): who may discover a tree from outside it.
/// Children inherit their root's scope dynamically.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerScope {
    /// Owner-console root: may exchange with other `owner` roots.
    Owner,
    /// Reachable only inside its own tree. The default and the migration
    /// value for roots of unknown provenance.
    #[default]
    Isolated,
}

/// Outcome of one peer exchange (§5.2). `Accepted` means committed to the
/// recipient's intake, not "read".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerMessageState {
    /// Durably queued for the recipient.
    Accepted,
    /// 183 F7: a contact's export set to Ask first holds it until its owner presses Allow;
    /// `error` explains. Not terminal: it moves to `consumed`/`replied` once allowed, or
    /// `failed` on Block, revoke or expiry.
    Held,
    /// Linked into the recipient's history; a model has or will see it.
    Consumed,
    /// A correlated reply exists.
    Replied,
    /// Communication was disabled before consumption; terminal, never revived.
    Suppressed,
    /// Acceptance or execution failed; `error` says why.
    Failed,
    /// Remote outcome unresolved (bytes may have left); not "undelivered".
    Unknown,
}

impl PeerScope {
    /// The serde word: `owner` | `isolated`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Isolated => "isolated",
        }
    }
}

impl std::str::FromStr for PeerScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Self::Owner),
            "isolated" => Ok(Self::Isolated),
            other => Err(format!("unknown peer scope {other:?}")),
        }
    }
}

impl PeerMessageState {
    /// The serde word: `accepted` | `held` | `consumed` | `replied` | `suppressed` | `failed` |
    /// `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Held => "held",
            Self::Consumed => "consumed",
            Self::Replied => "replied",
            Self::Suppressed => "suppressed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    /// No further transition is possible.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Replied | Self::Suppressed | Self::Failed)
    }
}

impl std::str::FromStr for PeerMessageState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accepted" => Ok(Self::Accepted),
            "held" => Ok(Self::Held),
            "consumed" => Ok(Self::Consumed),
            "replied" => Ok(Self::Replied),
            "suppressed" => Ok(Self::Suppressed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown peer message state {other:?}")),
        }
    }
}

/// What `send_message` / [`AnswerPeerRequest`] return (§5.2).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerReceipt {
    /// Harness-minted message id; opaque, never a process-local counter.
    pub id: Uuid,
    /// Canonical destination address (`session:<uuid>` | `peer:<name>`).
    pub to: String,
    /// Current outcome.
    pub state: PeerMessageState,
    /// Local message id this message answers.
    pub reply_to: Option<Uuid>,
    /// Bounded reply text when `state == Replied` and the caller awaited it.
    pub reply: Option<String>,
    /// Remote A2A task handle when known; opaque bounded string.
    pub task_id: Option<String>,
    /// Failure / suppression / unknown reason.
    pub error: Option<String>,
    /// Who produced `reply` (§5.3), durable provenance, never inferred from
    /// text: the replying peer's endpoint key (`session:<uuid>` |
    /// `peer:<binding-uuid>`), or `owner:<session-uuid>` when the human
    /// answered on behalf of that session, either for the recipient (the
    /// reply travels to the sender as a peer message from the recipient) or
    /// for the waiting sender (its own wait is resolved; no peer identity is
    /// forged and the recipient's copy stays as it was).
    #[serde(default)]
    pub answered_by: Option<String>,
}

/// One participant's durable record of a sent or received message (§5.2
/// `peer_message`). Never another session's history.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerRecord {
    /// Identity, destination and outcome.
    pub receipt: PeerReceipt,
    /// Authenticated sender address.
    pub from: String,
    /// The session whose view this record is.
    pub session: SessionId,
    /// Bounded, redacted body.
    pub text: String,
    /// Acceptance time.
    pub created_at: DateTime<Utc>,
    /// Owning A2A request scope, when the exchange belongs to one.
    pub request_scope: Option<Uuid>,
}

/// Backend behind a peer address (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerKind {
    /// A theuth session.
    Native,
    /// An installed ACP harness child.
    Acp,
    /// A paired node child.
    Node,
    /// A configured remote A2A peer.
    A2a,
    /// The theuth session on another engine that opened this one over the
    /// node conn (67 §8); reached only as `parent`.
    Parent,
}

/// The candidate's place relative to the asking session (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerRelationship {
    /// Direct parent.
    Parent,
    /// Direct child.
    Child,
    /// Same parent.
    Sibling,
    /// Farther up the same tree.
    Ancestor,
    /// Farther down the same tree.
    Descendant,
    /// Another enabled `owner` root, or a member of its tree.
    Root,
    /// Configured network peer.
    Remote,
}

/// Coarse liveness (§5.1): stored/idle is not offline, and a card is not a heartbeat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    /// Loaded, no turn in flight.
    Idle,
    /// A turn is in flight.
    Running,
    /// Blocked in an awaited send.
    Awaiting,
    /// Persisted but not loaded; listing did not wake it.
    Stored,
    /// Remote endpoint known to be unreachable, or a child that is gone.
    Offline,
    /// Owned by another engine of the mesh (69 P): stored there, its
    /// liveness unknown here; `send_message` reaches it through its owner.
    Remote,
}

/// Fine-grained liveness (62 §3.4): the [`crate::AgentState`] names, plus
/// `Stored` (no actor now) and `Offline` (a node known to be down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerLiveState {
    /// Loaded, no turn in flight.
    Idle,
    /// A turn is in flight.
    Running,
    /// An approval prompt is unanswered.
    Approval,
    /// Blocked in an awaited send.
    Awaiting,
    /// The last turn ended with an error.
    Error,
    /// The last turn was cancelled.
    Cancelled,
    /// Persisted but not loaded; listing did not wake it.
    Stored,
    /// A node known to be unreachable.
    Offline,
}

impl PeerLiveState {
    /// A turn is in flight or blocked mid-turn: the peer is working.
    pub fn working(self) -> bool {
        matches!(self, Self::Running | Self::Approval | Self::Awaiting)
    }
}

/// Liveness detail for one directory row (62 §0.7): state, times and counts,
/// never content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerActivity {
    /// Fine-grained state.
    pub state: PeerLiveState,
    /// Last input or stop, not a heartbeat.
    pub last_active: Option<DateTime<Utc>>,
    /// Age of the current turn in seconds; `None` when no turn is running.
    pub turn_secs: Option<u64>,
    /// Running background jobs (non-PTY).
    pub jobs: u16,
    /// Running PTY jobs.
    pub ptys: u16,
    /// Descendants with a live actor.
    pub children: u16,
    /// 155 §5.5: seconds since the child's open stall episode began; `None`
    /// when it is not stalled (or not an ACP child).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub stalled_secs: Option<u64>,
}

impl PeerActivity {
    /// Working now: a turn in flight or a running job/PTY (62 §3.4 `active_only`).
    pub fn active(&self) -> bool {
        self.state.working() || self.jobs + self.ptys > 0
    }
}

/// One row of the permitted directory (§5.1). No title, brief, cwd, model,
/// tools or transcript of another root; `name` is the same-tree brief name
/// or the owner's explicit alias.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerInfo {
    /// Canonical address to pass to `send_message`.
    pub address: String,
    /// Short display name (escaped data, never trusted).
    pub name: String,
    /// Backend.
    pub kind: PeerKind,
    /// Relative position.
    pub relationship: PeerRelationship,
    /// Coarse liveness.
    pub state: PeerState,
    /// Accepts messages.
    pub send: bool,
    /// Can answer with `reply_to`.
    pub reply: bool,
    /// Streams task updates (A2A only).
    pub stream: bool,
    /// Liveness detail (62 §0.7); `None` for a remote peer (a card is not a heartbeat).
    #[serde(default)]
    pub activity: Option<PeerActivity>,
    /// Session title, only for the caller's own tree (parent, child, sibling, ancestor, descendant).
    #[serde(default)]
    pub title: Option<String>,
    /// The title model's keywords for the session (07 §15 as-built): every
    /// local peer, empty until its first model-written title or for a
    /// remote peer.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The engine owning the session (69 P): this one's display name for a
    /// local peer, the owner's for a `remote` one; `None` for a peer that is
    /// no session (`peer:<name>`, `parent` over a conn).
    #[serde(default)]
    pub engine: Option<String>,
    /// 183 §6.6: whether the owning engine is connected now (remote rows
    /// only); `None` for a local peer or an older engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reachable: Option<bool>,
}

/// Reply to [`ListPeers`] / `discover_peers`: one page after permission filtering.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerPage {
    /// Rows in stable order.
    pub peers: Vec<PeerInfo>,
    /// Opaque cursor for the next page; `None` at the end.
    pub cursor: Option<String>,
}

/// Effective communication state for the global default or one session
/// (§2.1, §10). Authoritative: frontends render it, they do not guess it.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CommunicationState {
    /// `agent.communication`, the harness-wide ceiling.
    pub global: bool,
    /// Where `global` came from: `default` | `config` | `env` | `command`, and 183 §6.1's
    /// `mesh` (absent key, on because this engine has a paired peer).
    pub source: String,
    /// The session asked about; `None` = the global default alone.
    pub session: Option<SessionId>,
    /// This session itself opted out (memory-only, gone on restart).
    pub opted_out: bool,
    /// Nearest self-or-ancestor with an opt-out, if any.
    pub inherited_from: Option<SessionId>,
    /// `global && inherited_from.is_none()`; a route still needs scope/grant.
    pub effective: bool,
    /// The root's scope; [`PeerScope::Isolated`] for a global query.
    pub scope: PeerScope,
    /// Owner alias of the root, if set.
    pub alias: Option<String>,
    /// `a2a.enabled` as configured.
    pub network_enabled: bool,
    /// The A2A listener is up (implies `global && network_enabled`).
    pub listening: bool,
    /// Monotonic; bumps on every policy mutation.
    pub revision: u64,
}

/// `ask` → [`CommunicationState`]: read the switches without changing anything.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetCommunication {
    /// Session to read; `None` reads the global default alone.
    pub session: Option<SessionId>,
}

/// `ask` → `Result<CommunicationState, String>`: `/communication on|off`
/// (session) or `on|off always` (`session: None`, persisted to
/// `agent.communication` before the live switch moves; a failed write keeps
/// the old state and is the `Err`). Suppression and the revocation barrier
/// complete before the reply (§8).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetCommunication {
    /// Session to switch; `None` switches the global default.
    pub session: Option<SessionId>,
    /// `true` removes the opt-out / enables the harness.
    pub enabled: bool,
}

/// `ask` → `Result<CommunicationState, String>`: owner-only root scope and
/// alias (§2.3). `Err` for a non-root or unknown session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetPeerScope {
    /// The root to change.
    pub root: SessionId,
    /// New scope.
    pub scope: PeerScope,
    /// New alias; `None` keeps the current one, `Some("")` clears it.
    pub alias: Option<String>,
}

/// `ask` → `Result<PeerPage, String>`: the directory `session` may address
/// (§5.1), the same filter the model's `discover_peers` sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListPeers {
    /// Whose view.
    pub session: SessionId,
    /// Substring filter on `name`/`address`; never matches another root's title.
    pub query: Option<String>,
    /// Cursor from a previous page.
    pub cursor: Option<String>,
    /// Page size; `None` = [`DISCOVER_DEFAULT_LIMIT`], capped at [`DISCOVER_MAX_LIMIT`].
    pub limit: Option<u16>,
    /// Keep only rows whose `activity` is working (62 §3.4); 183 F6: contacts and their
    /// export rows always stay.
    #[serde(default)]
    pub active_only: bool,
    /// 183 F6: keep only rows of this kind; `contact` lists every contact row uncapped.
    #[serde(default)]
    pub kind: Option<DirectoryKind>,
}

/// 183 F6: the `discover_peers` `kind` filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryKind {
    /// theuth sessions.
    Native,
    /// External harness children.
    Acp,
    /// Paired node children.
    Node,
    /// Remote agents and contacts (with their shared chats and agents).
    A2a,
    /// The session on another engine that opened this one.
    Parent,
    /// Contacts and every chat or agent they share with you, none left out.
    Contact,
}

impl DirectoryKind {
    /// Whether `p` is a row of this kind.
    pub fn admits(self, p: &PeerInfo) -> bool {
        match self {
            Self::Native => p.kind == PeerKind::Native,
            Self::Acp => p.kind == PeerKind::Acp,
            Self::Node => p.kind == PeerKind::Node,
            Self::A2a => p.kind == PeerKind::A2a,
            Self::Parent => p.kind == PeerKind::Parent,
            Self::Contact => is_contact_row(p),
        }
    }
}

/// 183 F6: a contact's row or one of its export rows (a local session's title model may tag
/// a session `contact`, so the kind is checked too).
pub fn is_contact_row(p: &PeerInfo) -> bool {
    p.kind == PeerKind::A2a && p.tags.first().is_some_and(|t| t == "contact")
}

/// `ask` → `Result<PeerRecord, String>`: `session`'s own record of message
/// `id` (§5.2 `peer_message`). `Err("unknown message")` when the session is
/// neither its sender nor its recipient.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetPeerMessage {
    /// Whose view.
    pub session: SessionId,
    /// Message id.
    pub id: Uuid,
}

/// `ask` → `Result<PeerMessagePage, String>`: `session`'s own sent and
/// received records, newest first (§5.2, §10 owner API; no model tool). The
/// durable snapshot a UI reloads after reconnect; events only invalidate.
/// `pending_only` restricts to requests a human may resolve now: received
/// requests whose sender awaits `session`'s answer, and `session`'s own
/// sent requests it is blocked on. Both are exact ids [`AnswerPeerRequest`]
/// accepts; [`PeerMessagePage`] tells them apart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListPeerMessages {
    /// Whose view; only its own exchanges, never another session's.
    pub session: SessionId,
    /// Cursor from a previous page (the last returned id).
    pub cursor: Option<String>,
    /// Page size; `None` = [`DISCOVER_DEFAULT_LIMIT`], capped at [`DISCOVER_MAX_LIMIT`].
    pub limit: Option<u16>,
    /// Only requests a human may resolve now (either side).
    #[serde(default)]
    pub pending_only: bool,
}

/// Reply to [`ListPeerMessages`]. Pending sets derive from live waiters,
/// never abandoned oneshots; after an engine restart the waiters of open
/// turns are rebuilt from the `waits` rows (109 §4.4, `Agents::rebuild_waits`),
/// so those requests stay pending, and a request's durable outcome is its `state`.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerMessagePage {
    /// Records, newest first.
    pub messages: Vec<PeerRecord>,
    /// Received requests whose sender is blocked awaiting `session`'s
    /// answer: "answer on behalf of this session"; the answer reaches the
    /// sender as a peer message from `session`, recorded
    /// `answered_by = owner:<session>`.
    pub awaiting_answer: Vec<Uuid>,
    /// `session`'s own sent requests it is blocked on: "resolve this wait";
    /// the owner's text becomes the tool's return value, recorded
    /// `answered_by = owner:<session>`, and the recipient is not impersonated.
    pub awaiting_reply: Vec<Uuid>,
    /// Opaque cursor for the next page; `None` at the end.
    pub cursor: Option<String>,
}

/// `ask` → `Result<PeerReceipt, String>`: the human resolves pending request
/// `reply_to` on behalf of `session` (§5.3). Exact id; never picks an
/// arbitrary waiter. Two sides, told apart by the durable record:
/// - `session` *received* `reply_to`: the answer is sent to its sender as a
///   peer message from `session` (completing the sender's wait when live);
/// - `session` *sent* `reply_to` and awaits it: its own wait resolves with
///   the text, as the original human-answer behaviour for a blocked sender.
///
/// Either way the original is marked `replied` with
/// `answered_by = owner:<session>`. `Err` when `session` is neither side, or
/// nothing is pending.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerPeerRequest {
    /// The session the human acts for: the request's recipient or its
    /// blocked sender.
    pub session: SessionId,
    /// The request being resolved.
    pub reply_to: Uuid,
    /// Answer text; bounded by [`PEER_TEXT_MAX`].
    pub text: String,
}

/// One peer message on the wire between engines (69 P): what the sender's
/// supervisor forwards to the owner of the addressed session as
/// `peer_deliver(session, envelope)` over `theuth/rpc/1`. `id` is the
/// sender's minted message id, kept by the owner's intake so `reply_to`
/// names the same message on both engines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerEnvelope {
    /// The message id, minted on the sender's engine.
    pub id: Uuid,
    /// Authenticated sender session (a row every replica holds).
    pub from: SessionId,
    /// Redacted, bounded body.
    pub text: String,
    /// The message this one answers.
    pub reply_to: Option<Uuid>,
    /// Sender's idempotency key.
    pub request_id: Option<String>,
    /// 183 §6.5: the sender's chain count, so the 256 limit holds across
    /// engines; 0 from an older sender.
    #[serde(default)]
    pub chain: u32,
}

/// `ask` → `Result<PeerReceipt, String>`: the owner's intake of a peer
/// message another engine forwarded (69 P). Runs the ordinary [`crate::supervisor::Deliver`]
/// path — this engine's communication policy, redaction, size bound, reply
/// matching — under the envelope's id; never takes the session. `Err` is the
/// refusal the sender records as a `failed` receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerDeliver {
    /// The addressed session; must be owned here.
    pub session: SessionId,
    /// The message.
    pub envelope: PeerEnvelope,
    /// 183 §6.5: the engine that forwarded it (the RPC's `Forwarded(peer)`), checked
    /// against the owner of `envelope.from` and `mesh.messaging_off`; `None` = local.
    #[serde(default)]
    pub forwarder: Option<String>,
}

/// 183 §14.6 (D9, W-40): who a contact's message into an exported chat comes from, as the
/// engine that holds the contact vouches for it. The chat's owner may lower `cap`, never
/// raise it above `mutating`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactOrigin {
    /// The contact's name on the holding engine.
    pub contact: String,
    /// The contact's machine, from its card.
    pub machine: String,
    /// The export it addressed.
    pub export: String,
    /// `read_only` | `mutating`: the turn cap.
    pub cap: tools::Risk,
    /// Engine id that holds the contact and the A2A task.
    pub holder: String,
    /// 183 Z3-R2: the export's allowlist — the only tools that exist in this contact's
    /// session (inbox) or turn (exported chat), besides the reply. Absent (an older holder)
    /// = none.
    #[serde(default)]
    pub tools: Vec<String>,
}

/// 183 §14.6: one contact message for an exported chat owned by another engine of the
/// mesh, the `contact_deliver` params (W-40). There is no `from` session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactEnvelope {
    /// The message id, minted on the holding engine.
    pub id: Uuid,
    /// The exported chat.
    pub to: SessionId,
    /// Redacted, bounded body.
    pub text: String,
    /// Who sent it.
    pub origin: ContactOrigin,
    /// The message this one answers.
    #[serde(default)]
    pub reply_to: Option<Uuid>,
    /// The holder's idempotency key (the A2A `messageId`).
    #[serde(default)]
    pub request_id: Option<String>,
}

/// `ask` → `Result<PeerReceipt, String>`: the chat owner's intake of `contact_deliver`
/// (W-40): this engine's policy, then one capped turn (`tools::SetTurnCap`) opened by the
/// `<contact_message>` row (§11.6).
#[derive(Clone, Debug)]
pub struct ContactDeliver {
    /// The message.
    pub envelope: ContactEnvelope,
    /// The forwarding engine (the RPC's `Forwarded(peer)`); `None` = the holder is this engine.
    pub forwarder: Option<String>,
}

/// Event: communication settings changed (§10). `session: None` = the global
/// switch; reload every state and directory snapshot. `Some(root or session)`
/// = that subtree's opt-out or scope changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct CommunicationChanged {
    /// Affected subtree; `None` for global.
    pub session: Option<SessionId>,
    /// The policy revision after the change.
    pub revision: u64,
}

/// Event: a participant's receipt changed state (§10). No body; reload the
/// durable record when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct PeerMessageChanged {
    /// Participant whose view changed.
    pub session: SessionId,
    /// Message id.
    pub id: Uuid,
    /// New state.
    pub state: PeerMessageState,
}

/// Typed provenance of a transcript row that a peer exchange produced (§9):
/// carried on `HistoryEntry::User { peer }` for replay and inside
/// [`PeerDelivered`] live. Rendered outside the body; never parsed from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PeerOrigin {
    /// Exchange id (`peer_message { id }` shows the full record).
    pub id: Uuid,
    /// Authenticated sender endpoint (`session:<uuid>` | `peer:<binding-uuid>`).
    pub from: String,
    /// The local message this one answered, if any.
    pub reply_to: Option<Uuid>,
    /// Owning A2A request scope, when any.
    pub scope: Option<Uuid>,
}

/// 172: which channel delivered a user row (UIs show it as a chip; the model input is unchanged).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChannelOrigin {
    /// slack | signal | telegram
    pub via: ChannelVia,
    /// dm | group — a Slack channel and a Telegram group, supergroup or forum topic are `group`; Signal is DM-only today.
    pub chat: ChannelChat,
    /// The sender's label when it is not the owner (an allowed user), as the channel already names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub from: Option<String>,
    /// The channel or group title when the channel knows it (Slack channel name without `#`, Telegram group title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub title: Option<String>,
}

/// 172: the channel kind of a [`ChannelOrigin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ChannelVia {
    /// Slack (DM or channel).
    Slack,
    /// Signal (DM only today).
    Signal,
    /// Telegram (private chat, group, supergroup or forum topic).
    Telegram,
}

/// 172: a direct conversation or a shared one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ChannelChat {
    /// A direct message / private chat.
    Dm,
    /// A Slack channel; a Telegram group, supergroup or forum topic.
    Group,
}

/// Event: a peer exchange became part of `session`'s transcript (§10):
/// emitted after the consumption transaction linked it to row `seq` in
/// `turn`, never for a merely accepted or suppressed input. Structured
/// successor of `AgentEvent::MessageDelivered` for peer input; that legacy
/// event still fires until every surface renders this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct PeerDelivered {
    /// Receiving session.
    pub session: SessionId,
    /// Provenance.
    pub origin: PeerOrigin,
    /// Bounded, redacted body as consumed.
    pub text: String,
    /// History row it became (the replay key for `HistoryEntry::User { peer }`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: i64,
    /// The turn that consumed it.
    pub turn: TurnId,
}

/// Bus message: a session opened by a remote parent (67 §8) sent
/// `send_message{to: "parent"}`. The ACP relay that owns the session's conn
/// carries it out as a `_theuth/peer_message` notification; nothing else
/// consumes it. Minted here, never stored: the parent's exchange is the
/// durable record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerUplink {
    /// Sending session (the remote child, a root on this engine).
    pub session: SessionId,
    /// This engine's id for the message; the parent's idempotency key.
    pub id: Uuid,
    /// The parent-side message it answers, when it is a reply.
    pub reply_to: Option<Uuid>,
    /// Redacted, bounded body.
    pub text: String,
}

/// A parsed `to` argument (§5.1). Only canonical forms carry identity;
/// [`Self::Relative`] must be resolved against the authenticated caller.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PeerAddress {
    /// `session:<uuid>`; unknown ids fail, never create.
    Session(SessionId),
    /// `peer:<configured-name>`; an owner alias, identity is the binding.
    Remote(String),
    /// `parent` or a direct child name.
    Relative(String),
}

impl PeerAddress {
    /// Parse; `None` for a malformed canonical form (bad UUID, empty or
    /// oversized `peer:` name, empty string).
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(id) = s.strip_prefix("session:") {
            return id.parse().ok().map(Self::Session);
        }
        if let Some(name) = s.strip_prefix("peer:") {
            return (!name.is_empty() && name.chars().count() <= ALIAS_MAX)
                .then(|| Self::Remote(name.to_owned()));
        }
        (!s.is_empty()).then(|| Self::Relative(s.to_owned()))
    }

    /// The local session an endpoint key names (`session:<uuid>`); `None`
    /// for a remote binding key or anything else.
    pub fn local(key: &str) -> Option<SessionId> {
        match Self::parse(key)? {
            Self::Session(s) => Some(s),
            _ => None,
        }
    }
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(id) => write!(f, "session:{id}"),
            Self::Remote(name) => write!(f, "peer:{name}"),
            Self::Relative(name) => f.write_str(name),
        }
    }
}

/// Why a session is not effectively enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blocker {
    /// `agent.communication` is off: a ceiling, not an overridable default.
    Global,
    /// This session or the named ancestor opted out.
    Session(SessionId),
}

/// A refused policy question. Every variant is a concrete reason; none is a
/// fabricated success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    /// Not registered through [`Policy::record_root`] / [`Policy::record_child`].
    UnknownSession(SessionId),
    /// `session`'s recorded ancestor `ancestor` is not registered: the
    /// supervisor must mirror persisted topology before routing (§2.1).
    UnknownAncestor {
        /// The session asked about.
        session: SessionId,
        /// The missing link.
        ancestor: SessionId,
    },
    /// [`Policy::set_scope`] on a session that has a parent.
    NotRoot(SessionId),
    /// `a != b` is required.
    SelfRoute(SessionId),
    /// `session` is off because of `blocker`.
    Disabled {
        /// The endpoint that is off.
        session: SessionId,
        /// Why.
        blocker: Blocker,
    },
    /// Different roots and not both `owner`.
    Scope {
        /// Sender's root.
        from: SessionId,
        /// Recipient's root.
        to: SessionId,
    },
    /// `a2a.enabled` is false.
    NetworkDisabled,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSession(id) => write!(f, "unknown session {id}"),
            Self::UnknownAncestor { session, ancestor } => {
                write!(f, "session {session}: ancestor {ancestor} is not loaded")
            }
            Self::NotRoot(id) => write!(f, "session {id} is not a root"),
            Self::SelfRoute(id) => write!(f, "session {id} cannot message itself"),
            Self::Disabled {
                session,
                blocker: Blocker::Global,
            } => write!(
                f,
                "communication for {session} is disabled globally; use `/communication on always` to enable the harness"
            ),
            Self::Disabled {
                session,
                blocker: Blocker::Session(by),
            } if by == session => write!(f, "communication is off for session {session}"),
            Self::Disabled {
                session,
                blocker: Blocker::Session(by),
            } => write!(
                f,
                "communication for {session} is off: ancestor {by} opted out"
            ),
            Self::Scope { from, to } => write!(
                f,
                "no route between root {from} and root {to}: both must be in owner scope"
            ),
            Self::NetworkDisabled => f.write_str("network communication (a2a.enabled) is off"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// 183 §11.4: a cross-engine refusal, naming the engine rather than a session. `Display` is
/// the text after `denied: send_message: communication_disabled: ` (`Off`, `Messaging`) or
/// after `failed: ` (`NotOwner`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshRefusal {
    /// The owner engine's ceiling (`agent.communication`) is off.
    Off {
        /// Mesh name of the engine that is off.
        engine: String,
    },
    /// The per-machine switch (§6.2): `on` lists the other engine in `mesh.messaging_off`.
    Messaging {
        /// Mesh name of the sending engine.
        from: String,
        /// Mesh name of the receiving engine.
        to: String,
        /// Mesh name of the engine whose switch is off.
        on: String,
    },
    /// A forwarded `peer_deliver` from an engine that does not own `envelope.from` (§6.5).
    NotOwner {
        /// Mesh name of the forwarding engine.
        engine: String,
        /// The claimed sender.
        from: SessionId,
    },
}

impl fmt::Display for MeshRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off { engine } => write!(f, "communication is off on {engine}"),
            Self::Messaging { from, to, on } => {
                write!(f, "agent messaging between {from} and {to} is off on {on}")
            }
            Self::NotOwner { engine, from } => {
                write!(f, "peer_deliver: {engine} does not own session:{from}")
            }
        }
    }
}

impl std::error::Error for MeshRefusal {}

/// 183 §11.4, §7.10: what a contact (or the engine holding it) reads when its message is not
/// delivered. `Display` is the text after `waiting: ` (`NotAllowed`) or `failed: ` (the rest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactRefusal {
    /// First contact not answered yet (W-27): the task waits in `TASK_STATE_AUTH_REQUIRED`.
    NotAllowed {
        /// Contact name.
        contact: String,
        /// Export name.
        export: String,
    },
    /// Blocked or removed.
    NotAContact {
        /// Contact name.
        contact: String,
    },
    /// W-41: the export answers nothing for now; `reason` is `Export.paused`.
    Paused {
        /// `chat archived`.
        reason: String,
    },
    /// §13 row 55: the exported chat's `sessions.communication` is `0`.
    NoAgentMessages,
    /// W-15: the chat's owner is not connected. Z3-R2: a contact is not told which machine.
    OwnerOffline,
    /// §13 row 52: the owner answered `-32601` to `contact_deliver`.
    OwnerOld,
}

impl fmt::Display for ContactRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { contact, export } => {
                write!(f, "{contact} has not allowed messages to {export} yet")
            }
            Self::NotAContact { contact } => write!(f, "peer:{contact}: not a contact"),
            Self::Paused { reason } => write!(f, "export paused: {reason}"),
            Self::NoAgentMessages => {
                f.write_str("export unavailable: this chat does not take agent messages")
            }
            Self::OwnerOffline => f.write_str("the chat's machine is offline"),
            Self::OwnerOld => f.write_str("the chat's machine needs an update"),
        }
    }
}

impl ContactRefusal {
    /// 183 F7: the (machine, export) of a holder's `waiting: <machine> has not allowed
    /// messages to <export> yet` status (the [`Self::NotAllowed`] text), wherever it sits in
    /// `text`; `None` for any other status.
    pub fn waiting(text: &str) -> Option<(String, String)> {
        let rest = &text[text.find("waiting: ")? + "waiting: ".len()..];
        let (machine, rest) = rest.split_once(" has not allowed messages to ")?;
        let (export, _) = rest.split_once(" yet")?;
        (!machine.is_empty() && !export.is_empty()).then(|| (machine.to_owned(), export.to_owned()))
    }
}

/// 183 F7: what a sender's tools show for a message a contact's owner holds. The contact
/// name and the export come from the sender's own facts (its contact list, the address it
/// wrote); `machine` is the holder's text, so it is cut to 64 characters with control
/// characters dropped (the export too, when the address named none), and the whole line to
/// 512.
pub fn held_line(contact: &str, machine: &str, export: &str) -> String {
    let clean =
        |s: &str, n: usize| -> String { s.chars().filter(|c| !c.is_control()).take(n).collect() };
    let line = format!(
        "held: {contact} ({}) has not allowed messages to {} yet; it will be delivered when \
         they press Allow — do not resend",
        clean(machine, 64),
        clean(export, 64),
    );
    line.chars().take(512).collect()
}

impl std::error::Error for ContactRefusal {}

/// 183 §6.5: why the owner's `peer_deliver` intake refused. `NotOwner` is typed so the RPC
/// layer answers it `UNAUTHORIZED` without matching its text; `Display` is the plain text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerDeliverError {
    /// The forwarder does not own `envelope.from`.
    NotOwner(MeshRefusal),
    /// Any other refusal, as text.
    Other(String),
}

impl fmt::Display for PeerDeliverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOwner(r) => fmt::Display::fmt(r, f),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for PeerDeliverError {}

impl From<String> for PeerDeliverError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

/// An admitted route's live generation (§8). Cloned into the exchange or
/// network operation it admits; recheck [`Self::live`] at send admission
/// after approval, at intake consumption, before each network dispatch /
/// protected stream emission, and at reply/result routing. Memory-only, so a
/// restart has no valid old generation.
#[derive(Clone, Debug)]
pub struct Generation {
    revision: u64,
    tokens: [CancellationToken; 2],
}

impl Generation {
    /// Policy revision at admission.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Both endpoints still enabled under the same scope/global state.
    pub fn live(&self) -> bool {
        self.tokens.iter().all(|t| !t.is_cancelled())
    }

    /// Resolves once either endpoint's generation is invalidated.
    pub async fn revoked(&self) {
        tokio::select! {
            () = self.tokens[0].cancelled() => {}
            () = self.tokens[1].cancelled() => {}
        }
    }
}

#[derive(Clone, Debug)]
struct Root {
    scope: PeerScope,
    alias: Option<String>,
}

/// `s` is a strict descendant of `ancestor` in the recorded topology.
fn within(parents: &HashMap<SessionId, SessionId>, mut s: SessionId, ancestor: SessionId) -> bool {
    while let Some(&p) = parents.get(&s) {
        if p == ancestor {
            return true;
        }
        s = p;
    }
    false
}

/// The availability predicate of §2.1 plus the revocation barrier of §8.
/// `Agents` owns the one instance behind [`SharedPolicy`] and performs every
/// mutation and admission from its mailbox, so a setter completes its
/// barrier before it replies; the tool registry only reads through
/// [`tools::CommunicationAccess`].
///
/// Supervisor integration (next wave):
/// - startup: [`Self::new`] from `agent.communication` (source `default` |
///   `config` | `env`), every persisted root via [`Self::record_root`] and
///   every persisted parent edge via [`Self::record_child`] root-first (so
///   synchronous registry views never miss lazy ancestry), then
///   `registry.ask(tools::InstallCommunicationAccess(Arc::new(shared)))`;
///   [`Self::set_network`] from the A2A plugin's status;
/// - session creation: [`Self::record_root`] / [`Self::record_child`];
///   [`Self::record_fork`] for `ForkSession`; [`Self::forget`] on deletion;
/// - `/communication`: [`Self::state`], [`Self::set_session`],
///   [`Self::set_global`] after the config write succeeded; then suppress
///   accepted-unconsumed input, expire waiters with `communication_disabled`,
///   publish [`CommunicationChanged`], reply;
/// - `/peers` scope: [`Self::set_scope`];
/// - tool exposure: [`Self::enabled`]; send admission after approval:
///   [`Self::route`] / [`Self::remote`]; intake consumption and reply/result
///   routing: the admitted [`Generation::live`] plus a fresh [`Self::route`];
/// - directory filtering: [`Self::route`] per candidate, [`Self::root_of`]
///   for the relationship.
#[derive(Debug)]
pub struct Policy {
    global: bool,
    source: String,
    network_enabled: bool,
    listening: bool,
    revision: u64,
    roots: HashMap<SessionId, Root>,
    /// child → parent, persisted topology mirrored in.
    parents: HashMap<SessionId, SessionId>,
    opted_out: HashSet<SessionId>,
    /// 125 §4.2: advisor sessions — never a route end in either direction.
    hidden: HashSet<SessionId>,
    /// Barrier root: cancelled and replaced by every global change.
    epoch: CancellationToken,
    /// Per-session tokens, children of their parent's token (root: of `epoch`).
    generations: HashMap<SessionId, CancellationToken>,
}

impl Policy {
    /// From `agent.communication` and where it came from (`default` |
    /// `config` | `env`). Network state starts off until [`Self::set_network`].
    pub fn new(global: bool, source: impl Into<String>) -> Self {
        Self {
            global,
            source: source.into(),
            network_enabled: false,
            listening: false,
            revision: 0,
            roots: HashMap::new(),
            parents: HashMap::new(),
            opted_out: HashSet::new(),
            hidden: HashSet::new(),
            epoch: CancellationToken::new(),
            generations: HashMap::new(),
        }
    }

    /// Current revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// `agent.communication` as it stands.
    pub fn global(&self) -> bool {
        self.global
    }

    // --- topology -----------------------------------------------------------

    /// 125 §4.2: mark `id` an advisor session — [`Self::route`] refuses it
    /// as either end, whatever its lineage says.
    pub fn record_hidden(&mut self, id: SessionId) {
        self.hidden.insert(id);
    }

    /// A trusted creator declares a root (§2.3). Re-recording an existing root
    /// keeps its scope and alias; a root with a recorded parent is an error.
    pub fn record_root(&mut self, id: SessionId, scope: PeerScope) -> Result<(), PolicyError> {
        if self.parents.contains_key(&id) {
            return Err(PolicyError::NotRoot(id));
        }
        self.roots.entry(id).or_insert(Root { scope, alias: None });
        Ok(())
    }

    /// Mirror a persisted parent link. `parent` must already be known so
    /// ancestry never dangles; a session cannot be its own ancestor.
    pub fn record_child(&mut self, child: SessionId, parent: SessionId) -> Result<(), PolicyError> {
        if child == parent {
            return Err(PolicyError::SelfRoute(child));
        }
        if !self.known(parent) {
            return Err(PolicyError::UnknownSession(parent));
        }
        // Reject a cycle: `parent`'s chain must not pass through `child`.
        let mut cur = parent;
        while let Some(&p) = self.parents.get(&cur) {
            if p == child {
                return Err(PolicyError::UnknownAncestor {
                    session: child,
                    ancestor: parent,
                });
            }
            cur = p;
        }
        self.roots.remove(&child);
        self.parents.insert(child, parent);
        Ok(())
    }

    /// A fork is a new root (§2.1): it inherits the source root's scope and
    /// alias-less identity, and starts opted out when its source is not
    /// effectively enabled, so dropping ancestry cannot publish a private
    /// conversation. `Err` when the source is unknown.
    pub fn record_fork(&mut self, source: SessionId, fork: SessionId) -> Result<(), PolicyError> {
        let root = self.root_of(source)?;
        let scope = self.roots[&root].scope;
        let off = self.enabled(source).is_err();
        self.roots.insert(fork, Root { scope, alias: None });
        self.parents.remove(&fork);
        if off {
            self.opted_out.insert(fork);
        }
        Ok(())
    }

    /// Drop a deleted session and its whole subtree: invalidates their
    /// generations, opt-outs, scope and links.
    pub fn forget(&mut self, id: SessionId) {
        let gone: Vec<SessionId> = self
            .roots
            .keys()
            .chain(self.parents.keys())
            .copied()
            .filter(|&s| s == id || within(&self.parents, s, id))
            .collect();
        if let Some(t) = self.generations.get(&id) {
            t.cancel();
        }
        for s in gone {
            self.roots.remove(&s);
            self.parents.remove(&s);
            self.opted_out.remove(&s);
            self.generations.remove(&s);
        }
        self.revision += 1;
    }

    fn known(&self, id: SessionId) -> bool {
        self.roots.contains_key(&id) || self.parents.contains_key(&id)
    }

    /// The root of `s`'s tree, walking persisted links; errors when `s` or
    /// any link is not recorded.
    pub fn root_of(&self, s: SessionId) -> Result<SessionId, PolicyError> {
        if !self.known(s) {
            return Err(PolicyError::UnknownSession(s));
        }
        let mut cur = s;
        loop {
            match self.parents.get(&cur) {
                Some(&p) if self.known(p) => cur = p,
                Some(&p) => {
                    return Err(PolicyError::UnknownAncestor {
                        session: s,
                        ancestor: p,
                    });
                }
                None => return Ok(cur),
            }
        }
    }

    /// Nearest self-or-ancestor opt-out; walks the full chain so unknown
    /// ancestry is an error, not "enabled".
    fn blocker_of(&self, s: SessionId) -> Result<Option<SessionId>, PolicyError> {
        if !self.known(s) {
            return Err(PolicyError::UnknownSession(s));
        }
        let mut cur = s;
        let mut found = None;
        loop {
            if found.is_none() && self.opted_out.contains(&cur) {
                found = Some(cur);
            }
            match self.parents.get(&cur) {
                Some(&p) if self.known(p) => cur = p,
                Some(&p) => {
                    return Err(PolicyError::UnknownAncestor {
                        session: s,
                        ancestor: p,
                    });
                }
                None => return Ok(found),
            }
        }
    }

    // --- predicates ---------------------------------------------------------

    /// `local_enabled(s)` (§2.1): the global ceiling and no self/ancestor
    /// opt-out. Use for tool exposure; a route needs [`Self::route`].
    pub fn enabled(&self, s: SessionId) -> Result<(), PolicyError> {
        let blocker = self.blocker_of(s)?;
        if !self.global {
            return Err(PolicyError::Disabled {
                session: s,
                blocker: Blocker::Global,
            });
        }
        match blocker {
            Some(by) => Err(PolicyError::Disabled {
                session: s,
                blocker: Blocker::Session(by),
            }),
            None => Ok(()),
        }
    }

    /// `local_route(a, b)` (§2.1): both enabled, `a != b`, same tree or both
    /// roots `owner`. Returns the live [`Generation`] to carry with the exchange.
    pub fn route(&mut self, from: SessionId, to: SessionId) -> Result<Generation, PolicyError> {
        if from == to {
            return Err(PolicyError::SelfRoute(from));
        }
        // 125 §4.2: an advisor is never a peer; it is unknown to every route.
        if let Some(&hidden) = [from, to].iter().find(|id| self.hidden.contains(id)) {
            return Err(PolicyError::UnknownSession(hidden));
        }
        self.enabled(from)?;
        self.enabled(to)?;
        let (rf, rt) = (self.root_of(from)?, self.root_of(to)?);
        if rf != rt
            && (self.roots[&rf].scope != PeerScope::Owner
                || self.roots[&rt].scope != PeerScope::Owner)
        {
            return Err(PolicyError::Scope { from: rf, to: rt });
        }
        Ok(Generation {
            revision: self.revision,
            tokens: [self.token(from), self.token(to)],
        })
    }

    /// The local half of `remote_route(s, p)` (§2.1): `s` enabled and
    /// `a2a.enabled`. Grant and authentication belong to the A2A plugin.
    pub fn remote(&mut self, s: SessionId) -> Result<Generation, PolicyError> {
        if !self.network_enabled {
            self.enabled(s)?;
            return Err(PolicyError::NetworkDisabled);
        }
        self.contact(s)
    }

    /// 183 W-26: [`Self::remote`] for a contact (a pinned endpoint on `theuth/a2a/1`):
    /// `agent.communication` and the session's opt-out gate it, `a2a.enabled` does not (it
    /// gates only the HTTP listener).
    pub fn contact(&mut self, s: SessionId) -> Result<Generation, PolicyError> {
        self.enabled(s)?;
        let t = self.token(s);
        Ok(Generation {
            revision: self.revision,
            tokens: [t.clone(), t],
        })
    }

    /// Single-endpoint generation for work owned by `s` (an accepted intake,
    /// an exported task scope) once `s` is enabled.
    pub fn generation(&mut self, s: SessionId) -> Result<Generation, PolicyError> {
        self.enabled(s)?;
        let t = self.token(s);
        Ok(Generation {
            revision: self.revision,
            tokens: [t.clone(), t],
        })
    }

    /// The session's token, a child of its parent's (root: of the epoch), so
    /// cancelling an ancestor cancels the subtree. Only called for enabled,
    /// hence fully known, sessions.
    fn token(&mut self, s: SessionId) -> CancellationToken {
        if let Some(t) = self.generations.get(&s) {
            return t.clone();
        }
        let parent = match self.parents.get(&s).copied() {
            Some(p) => self.token(p),
            None => self.epoch.clone(),
        };
        let t = parent.child_token();
        self.generations.insert(s, t.clone());
        t
    }

    /// Cancel `s`'s subtree generations and drop their cached tokens so the
    /// next admission mints fresh ones.
    fn invalidate(&mut self, s: SessionId) {
        if let Some(t) = self.generations.remove(&s) {
            t.cancel();
        }
        let parents = &self.parents;
        self.generations.retain(|&k, _| !within(parents, k, s));
    }

    // --- settings -----------------------------------------------------------

    /// Authoritative state for the global default or one session (§10).
    pub fn state(&self, session: Option<SessionId>) -> Result<CommunicationState, PolicyError> {
        let (opted_out, inherited_from, scope, alias) = match session {
            None => (false, None, PeerScope::Isolated, None),
            Some(s) => {
                let blocker = self.blocker_of(s)?;
                let root = &self.roots[&self.root_of(s)?];
                (
                    self.opted_out.contains(&s),
                    blocker,
                    root.scope,
                    root.alias.clone(),
                )
            }
        };
        Ok(CommunicationState {
            global: self.global,
            source: self.source.clone(),
            session,
            opted_out,
            inherited_from,
            effective: self.global && inherited_from.is_none(),
            scope,
            alias,
            network_enabled: self.network_enabled,
            listening: self.listening && self.network_enabled && self.global,
            revision: self.revision,
        })
    }

    /// `/communication on|off always`, after the config write succeeded.
    /// Every outstanding generation is invalidated in both directions; the
    /// caller then suppresses accepted-unconsumed input and expires waiters
    /// with `communication_disabled` before replying. Returns the new revision.
    pub fn set_global(&mut self, enabled: bool, source: impl Into<String>) -> u64 {
        self.global = enabled;
        self.source = source.into();
        self.epoch.cancel();
        self.epoch = CancellationToken::new();
        self.generations.clear();
        self.revision += 1;
        self.revision
    }

    /// `/communication on|off` for one session. Memory only: the supervisor
    /// persists it as `sessions.communication` (183 §4.5) and reapplies it at
    /// actor spawn. Off installs the
    /// subtree barrier; on removes the opt-out and grants nothing
    /// retroactively (suppressed input stays suppressed). Returns the new
    /// revision; `Err` for an unknown session.
    pub fn set_session(&mut self, s: SessionId, enabled: bool) -> Result<u64, PolicyError> {
        if !self.known(s) {
            return Err(PolicyError::UnknownSession(s));
        }
        if enabled {
            self.opted_out.remove(&s);
        } else {
            self.opted_out.insert(s);
            self.invalidate(s);
        }
        self.revision += 1;
        Ok(self.revision)
    }

    /// Owner-only root scope/alias (§2.3). `alias: None` keeps, `Some("")`
    /// clears; trimmed and cut at [`ALIAS_MAX`] chars. A scope change installs
    /// the subtree barrier. `Err` for a non-root or unknown session.
    pub fn set_scope(
        &mut self,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> Result<u64, PolicyError> {
        if self.parents.contains_key(&root) {
            return Err(PolicyError::NotRoot(root));
        }
        let r = self
            .roots
            .get_mut(&root)
            .ok_or(PolicyError::UnknownSession(root))?;
        let changed = r.scope != scope;
        r.scope = scope;
        if let Some(a) = alias {
            let a: String = a.trim().chars().take(ALIAS_MAX).collect();
            r.alias = (!a.is_empty()).then_some(a);
        }
        if changed {
            self.invalidate(root);
        }
        self.revision += 1;
        Ok(self.revision)
    }

    /// Report the A2A plugin's status (`a2a.enabled`, listener up). Turning
    /// the network off invalidates every generation, so open remote
    /// operations recheck and fail; local routes are unaffected by the
    /// listener alone. Returns the new revision, `None` when the report
    /// matches what is already recorded: the plugin re-reports after every
    /// [`CommunicationChanged`], so an unchanged report must neither bump
    /// nor notify, or the two feed each other forever.
    pub fn set_network(&mut self, enabled: bool, listening: bool) -> Option<u64> {
        if self.network_enabled == enabled && self.listening == listening {
            return None;
        }
        if self.network_enabled && !enabled {
            self.epoch.cancel();
            self.epoch = CancellationToken::new();
            self.generations.clear();
        }
        self.network_enabled = enabled;
        self.listening = listening;
        self.revision += 1;
        Some(self.revision)
    }
}

/// The one [`Policy`] shared between `Agents` (mutations, routing) and the
/// tool registry's synchronous [`tools::CommunicationAccess`] view. No
/// shadow copy: the registry reads the same generations `Agents` invalidates.
/// Install with `registry.ask(tools::InstallCommunicationAccess(Arc::new(shared.clone())))`.
#[derive(Clone, Debug, kameo::Reply)]
pub struct SharedPolicy(pub std::sync::Arc<parking_lot::RwLock<Policy>>);

impl SharedPolicy {
    /// Wrap a fresh policy.
    pub fn new(policy: Policy) -> Self {
        Self(std::sync::Arc::new(parking_lot::RwLock::new(policy)))
    }
}

impl tools::CommunicationAccess for SharedPolicy {
    fn enabled(&self, session: SessionId) -> bool {
        self.0.read().enabled(session).is_ok()
    }

    /// `None` for an unknown or disabled session; otherwise the session's
    /// live token (minted under a short write lock on first use).
    fn generation(&self, session: SessionId) -> Option<CancellationToken> {
        let mut p = self.0.write();
        p.enabled(session).ok()?;
        Some(p.token(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(global: bool) -> (Policy, SessionId, SessionId, SessionId) {
        let mut p = Policy::new(global, "config");
        let (root, child, grandchild) = (SessionId::new(), SessionId::new(), SessionId::new());
        p.record_root(root, PeerScope::Owner).unwrap();
        p.record_child(child, root).unwrap();
        p.record_child(grandchild, child).unwrap();
        (p, root, child, grandchild)
    }

    #[test]
    fn truth_table_global_ancestor_self() {
        for bits in 0..8u8 {
            let (global, anc, own) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            let (mut p, root, _child, gc) = tree(global);
            if anc {
                p.set_session(root, false).unwrap();
            }
            if own {
                p.set_session(gc, false).unwrap();
            }
            let ok = p.enabled(gc).is_ok();
            assert_eq!(
                ok,
                global && !anc && !own,
                "global={global} anc={anc} self={own}"
            );
            let st = p.state(Some(gc)).unwrap();
            assert_eq!(st.effective, ok);
            assert_eq!(st.opted_out, own);
            // Nearest blocker wins: self before ancestor.
            let expect = if own {
                Some(gc)
            } else if anc {
                Some(root)
            } else {
                None
            };
            assert_eq!(st.inherited_from, expect);
            if !global {
                assert!(matches!(
                    p.enabled(gc),
                    Err(PolicyError::Disabled {
                        blocker: Blocker::Global,
                        ..
                    })
                ));
            }
        }
    }

    #[test]
    fn session_on_cannot_defeat_global_off() {
        let (mut p, _root, child, _) = tree(false);
        p.set_session(child, true).unwrap();
        let err = p.enabled(child).unwrap_err();
        assert!(err.to_string().contains("on always"));
        assert!(!p.state(Some(child)).unwrap().effective);
    }

    #[test]
    fn unknown_session_and_dangling_ancestor_are_errors() {
        let mut p = Policy::new(true, "config");
        let ghost = SessionId::new();
        assert_eq!(p.enabled(ghost), Err(PolicyError::UnknownSession(ghost)));
        assert_eq!(
            p.state(Some(ghost)),
            Err(PolicyError::UnknownSession(ghost))
        );
        assert_eq!(
            p.set_session(ghost, false),
            Err(PolicyError::UnknownSession(ghost))
        );
        // A child cannot be recorded under an unknown parent.
        let child = SessionId::new();
        assert_eq!(
            p.record_child(child, ghost),
            Err(PolicyError::UnknownSession(ghost))
        );
        // Deleting the root leaves no dangling child (whole subtree forgotten).
        let (mut p, root, child, gc) = tree(true);
        p.forget(root);
        assert_eq!(p.enabled(child), Err(PolicyError::UnknownSession(child)));
        assert_eq!(p.enabled(gc), Err(PolicyError::UnknownSession(gc)));
    }

    #[test]
    fn record_child_rejects_self_and_cycles() {
        let (mut p, root, child, gc) = tree(true);
        assert_eq!(
            p.record_child(root, root),
            Err(PolicyError::SelfRoute(root))
        );
        assert!(matches!(
            p.record_child(root, gc),
            Err(PolicyError::UnknownAncestor { .. })
        ));
        assert_eq!(p.root_of(child), Ok(root));
    }

    #[test]
    fn route_same_tree_both_directions_and_self() {
        let (mut p, root, child, gc) = tree(true);
        assert!(p.route(gc, root).is_ok());
        assert!(p.route(root, gc).is_ok());
        assert!(p.route(child, gc).is_ok());
        assert_eq!(
            p.route(child, child).unwrap_err(),
            PolicyError::SelfRoute(child)
        );
        // Recipient off blocks both directions.
        p.set_session(gc, false).unwrap();
        assert!(
            matches!(p.route(root, gc), Err(PolicyError::Disabled { session, .. }) if session == gc)
        );
        assert!(
            matches!(p.route(gc, root), Err(PolicyError::Disabled { session, .. }) if session == gc)
        );
        // Sibling of gc under child still reaches root.
        assert!(p.route(child, root).is_ok());
    }

    #[test]
    fn cross_root_needs_both_owner() {
        let mut p = Policy::new(true, "config");
        let (a, b, bc) = (SessionId::new(), SessionId::new(), SessionId::new());
        p.record_root(a, PeerScope::Owner).unwrap();
        p.record_root(b, PeerScope::Isolated).unwrap();
        p.record_child(bc, b).unwrap();
        assert_eq!(
            p.route(a, bc).unwrap_err(),
            PolicyError::Scope { from: a, to: b }
        );
        assert_eq!(
            p.route(bc, a).unwrap_err(),
            PolicyError::Scope { from: b, to: a }
        );
        p.set_scope(b, PeerScope::Owner, Some("  lab  ".into()))
            .unwrap();
        assert!(p.route(a, bc).is_ok());
        assert_eq!(p.state(Some(bc)).unwrap().alias.as_deref(), Some("lab"));
        // Isolating one side again closes the route; alias `None` keeps.
        p.set_scope(a, PeerScope::Isolated, None).unwrap();
        assert_eq!(
            p.route(a, bc).unwrap_err(),
            PolicyError::Scope { from: a, to: b }
        );
        assert_eq!(p.state(Some(bc)).unwrap().alias.as_deref(), Some("lab"));
        p.set_scope(b, PeerScope::Owner, Some("".into())).unwrap();
        assert_eq!(p.state(Some(bc)).unwrap().alias, None);
        // Scope is owner-only on roots.
        assert_eq!(
            p.set_scope(bc, PeerScope::Owner, None),
            Err(PolicyError::NotRoot(bc))
        );
    }

    #[test]
    fn default_scope_is_isolated_and_default_config_is_off() {
        assert_eq!(PeerScope::default(), PeerScope::Isolated);
        // 183 §6.1: absent, so the mesh default decides.
        assert_eq!(crate::AgentConfig::default().communication, None);
        let p = Policy::new(false, "default");
        let st = p.state(None).unwrap();
        assert!(!st.effective && !st.global && st.source == "default");
        assert_eq!(st.scope, PeerScope::Isolated);
    }

    #[test]
    fn opt_out_invalidates_subtree_only() {
        let (mut p, root, child, gc) = tree(true);
        let other = SessionId::new();
        p.record_child(other, root).unwrap();
        let g_gc = p.route(gc, root).unwrap();
        let g_other = p.route(other, root).unwrap();
        p.set_session(child, false).unwrap();
        assert!(!g_gc.live(), "descendant of the opted-out session");
        assert!(g_other.live(), "unrelated sibling tree");
        // Re-enable: old generation stays dead, a new one is live.
        p.set_session(child, true).unwrap();
        assert!(!g_gc.live());
        assert!(p.route(gc, root).unwrap().live());
    }

    #[test]
    fn opt_out_of_recipient_kills_admitted_route() {
        let (mut p, root, _child, gc) = tree(true);
        let g = p.route(root, gc).unwrap();
        p.set_session(gc, false).unwrap();
        assert!(!g.live());
        // An admission for the sender alone is untouched.
        let (mut p, root, _child, gc) = tree(true);
        let g = p.generation(root).unwrap();
        p.set_session(gc, false).unwrap();
        assert!(g.live());
    }

    #[test]
    fn global_change_invalidates_everything_and_bumps_revision() {
        let (mut p, root, _child, gc) = tree(true);
        let rev = p.revision();
        let g = p.route(gc, root).unwrap();
        assert_eq!(g.revision(), rev);
        let next = p.set_global(false, "command");
        assert_eq!(next, rev + 1);
        assert!(!g.live());
        assert!(matches!(
            p.route(gc, root),
            Err(PolicyError::Disabled { .. })
        ));
        // Turning it back on mints a fresh live generation; the old stays dead.
        p.set_global(true, "command");
        assert!(!g.live());
        let g2 = p.route(gc, root).unwrap();
        assert!(g2.live());
        assert_eq!(p.state(None).unwrap().source, "command");
    }

    #[test]
    fn scope_change_installs_barrier() {
        let mut p = Policy::new(true, "config");
        let (a, b) = (SessionId::new(), SessionId::new());
        p.record_root(a, PeerScope::Owner).unwrap();
        p.record_root(b, PeerScope::Owner).unwrap();
        let g = p.route(a, b).unwrap();
        // Alias-only change: no barrier.
        p.set_scope(a, PeerScope::Owner, Some("x".into())).unwrap();
        assert!(g.live());
        p.set_scope(b, PeerScope::Isolated, None).unwrap();
        assert!(!g.live());
    }

    #[test]
    fn network_gating() {
        let (mut p, root, _, _) = tree(true);
        assert_eq!(p.remote(root).unwrap_err(), PolicyError::NetworkDisabled);
        p.set_network(true, true);
        let g = p.remote(root).unwrap();
        assert!(p.state(None).unwrap().listening);
        p.set_network(false, true);
        assert!(!g.live());
        assert!(!p.state(None).unwrap().listening);
        // Listening cannot be reported while global is off.
        p.set_global(false, "command");
        p.set_network(true, true);
        assert!(!p.state(None).unwrap().listening);
        assert!(matches!(p.remote(root), Err(PolicyError::Disabled { .. })));
    }

    #[test]
    fn repeated_network_status_is_a_no_op() {
        let (mut p, root, _, gc) = tree(true);
        assert_eq!(p.set_network(true, true), Some(p.revision()));
        let rev = p.revision();
        let g = p.remote(root).unwrap();
        let local = p.route(gc, root).unwrap();
        for _ in 0..3 {
            assert_eq!(p.set_network(true, true), None);
        }
        assert_eq!(p.revision(), rev);
        assert!(g.live());
        assert!(local.live());
        // Listener alone changes state once, not generations.
        assert_eq!(p.set_network(true, false), Some(rev + 1));
        assert_eq!(p.set_network(true, false), None);
        assert!(g.live());
        // Global off is reflected at once, before the plugin re-reports.
        p.set_network(true, true);
        p.set_global(false, "command");
        assert!(!p.state(None).unwrap().listening);
    }

    #[test]
    fn fork_inherits_scope_and_opts_out_when_source_is_off() {
        let (mut p, root, child, gc) = tree(true);
        // Enabled source → fork is an enabled owner root.
        let f1 = SessionId::new();
        p.record_fork(gc, f1).unwrap();
        assert_eq!(p.root_of(f1), Ok(f1));
        assert_eq!(p.state(Some(f1)).unwrap().scope, PeerScope::Owner);
        assert!(p.enabled(f1).is_ok());
        // Ancestor-disabled source → fork starts opted out, on its own.
        p.set_session(child, false).unwrap();
        let f2 = SessionId::new();
        p.record_fork(gc, f2).unwrap();
        assert_eq!(p.state(Some(f2)).unwrap().inherited_from, Some(f2));
        assert!(p.enabled(f2).is_err());
        // Globally-off source → fork opted out too; a private fork stays private after `on always`.
        p.set_global(false, "command");
        let f3 = SessionId::new();
        p.record_fork(root, f3).unwrap();
        p.set_global(true, "command");
        assert!(p.enabled(f3).is_err());
        assert!(p.enabled(root).is_ok());
        let ghost = SessionId::new();
        assert_eq!(
            p.record_fork(ghost, SessionId::new()),
            Err(PolicyError::UnknownSession(ghost))
        );
    }

    #[test]
    fn shared_policy_access_view() {
        use tools::CommunicationAccess as _;
        let (p, root, child, gc) = tree(true);
        let shared = SharedPolicy::new(p);
        assert!(shared.enabled(gc));
        let tok = shared.generation(gc).expect("enabled session has a token");
        assert!(!tok.is_cancelled());
        // Unknown: no fabricated token.
        assert!(shared.generation(SessionId::new()).is_none());
        // Ancestor opt-out through the owner's handle revokes the registry's token.
        shared.0.write().set_session(child, false).unwrap();
        assert!(tok.is_cancelled());
        assert!(!shared.enabled(gc));
        assert!(shared.generation(gc).is_none());
        assert!(shared.generation(root).is_some_and(|t| !t.is_cancelled()));
        // Re-enable: a new, live token; the old one stays dead.
        shared.0.write().set_session(child, true).unwrap();
        let fresh = shared.generation(gc).unwrap();
        assert!(!fresh.is_cancelled() && tok.is_cancelled());
    }

    #[test]
    fn forget_cancels_and_removes_subtree() {
        let (mut p, root, child, gc) = tree(true);
        let g = p.route(gc, root).unwrap();
        p.forget(child);
        assert!(!g.live());
        assert_eq!(p.enabled(gc), Err(PolicyError::UnknownSession(gc)));
        assert!(p.enabled(root).is_ok());
    }

    #[tokio::test]
    async fn revoked_resolves_on_invalidation() {
        let (mut p, root, _child, gc) = tree(true);
        let g = p.route(gc, root).unwrap();
        p.set_session(gc, false).unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(50), g.revoked())
            .await
            .expect("revoked() resolves once cancelled");
    }

    #[test]
    fn peer_address_forms() {
        let id = SessionId::new();
        assert_eq!(
            PeerAddress::parse(&format!("session:{id}")),
            Some(PeerAddress::Session(id))
        );
        assert_eq!(PeerAddress::parse("session:nope"), None);
        assert_eq!(
            PeerAddress::parse("peer:lab"),
            Some(PeerAddress::Remote("lab".into()))
        );
        assert_eq!(PeerAddress::parse("peer:"), None);
        assert_eq!(
            PeerAddress::parse(&format!("peer:{}", "x".repeat(ALIAS_MAX + 1))),
            None
        );
        assert_eq!(
            PeerAddress::parse("parent"),
            Some(PeerAddress::Relative("parent".into()))
        );
        assert_eq!(PeerAddress::parse(""), None);
        assert_eq!(
            PeerAddress::Session(id).to_string(),
            format!("session:{id}")
        );
    }

    #[test]
    fn wire_snake_case() {
        assert_eq!(
            serde_json::to_string(&PeerScope::Isolated).unwrap(),
            "\"isolated\""
        );
        assert_eq!(
            serde_json::to_string(&PeerMessageState::Suppressed).unwrap(),
            "\"suppressed\""
        );
        let ev: CommunicationChanged = serde_json::from_value(serde_json::json!({
            "type": "CommunicationChanged", "session": null, "revision": 3
        }))
        .unwrap();
        assert_eq!(ev.revision, 3);
    }
}
