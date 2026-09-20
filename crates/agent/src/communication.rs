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
pub const PENDING_PER_RECIPIENT: usize = 32;
/// Accepted, unconsumed peer inputs per harness (§8).
pub const PENDING_PER_HARNESS: usize = 256;
/// Pending reply waits per session (§8).
pub const WAITS_PER_SESSION: usize = 16;
/// Pending reply waits per harness (§8).
pub const WAITS_PER_HARNESS: usize = 128;
/// Peer-triggered continuations per initiating request (§8).
pub const CHAIN_MAX: u32 = 32;
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
    /// The serde word: `accepted` | `consumed` | `replied` | `suppressed` | `failed` | `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
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
    /// Where `global` came from: `default` | `config` | `env` | `command`.
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
/// never abandoned oneshots, so after a daemon restart both are empty and a
/// request's durable outcome is its `state`.
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
        self.enabled(s)?;
        if !self.network_enabled {
            return Err(PolicyError::NetworkDisabled);
        }
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

    /// `/communication on|off` for one session (memory-only). Off installs the
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
        assert!(!crate::AgentConfig::default().communication);
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
