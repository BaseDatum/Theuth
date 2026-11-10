//! Network (A2A) request state on `Agents` (design doc 50 §4, §6, §8): the
//! typed commands the `a2a` plugin's `RequestHandler` and client translate
//! into, the exported-task lifecycle over existing native sessions, and the
//! registered remote senders the `send_message` tool reaches `peer:<name>`
//! through. No second actor, store or ledger: [`NetworkState`] maps existing
//! identities (input tickets, turns, operations) to tasks explicitly — never
//! "whatever the session is doing" — the store is the only durable truth, and
//! its work ledger decides when a scope owns no more work.
//!
//! Every inbound operation is authorized against the trusted
//! [`A2aPrincipal`] (binding → export → session) and the live communication
//! policy *before* any lookup; unknown and foreign tasks are one answer
//! (`TASK_NOT_FOUND`). Nothing here holds the mailbox across a wait: blocking
//! sends and cancels reply through a delegated sender, streams are pulled by
//! the plugin, remote sends run in spawned tasks that report back by message.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use a2a_types::{
    A2AError, Artifact, CancelTaskRequest, GetTaskRequest, ListTasksRequest, ListTasksResponse,
    Message as A2aMessage, Part, Role, SendMessageRequest, SendMessageResponse, StreamResponse,
    SubscribeToTaskRequest, Task, TaskArtifactUpdateEvent, TaskState, TaskStatus,
    TaskStatusUpdateEvent, new_artifact_id, new_context_id, new_task_id,
};
use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt, stream};
use kameo::message::{Context, Message};
use runtime::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tools::{Operation, OperationId};
use uuid::Uuid;

use super::{
    CommunicationChanged, Generation, PEER_TEXT_MAX, PeerInfo, PeerKind, PeerMessageState,
    PeerReceipt, PeerRecord, PeerRelationship, PeerState, PeerTurnFinished, SharedPolicy,
    held_line,
};
use crate::continuation::{InputConsumed, InputTicket};
use crate::store::communication::{
    A2aRequest, A2aScope, A2aTask, A2aTaskStatus, AcceptExchange, CommError, EXTERNAL_ID_MAX,
    PeerBinding, TASK_PAGE_MAX, TaskQuery,
};
use crate::supervisor::Agents;

/// SSE subscribers per task (§8).
pub const SUBSCRIBERS_PER_TASK: usize = 4;
/// SSE subscribers across the harness (§8).
pub const SUBSCRIBERS_MAX: usize = 128;
/// Buffered events per subscriber before it is disconnected (§8: 64 KiB;
/// one status/artifact event is at most one bounded text).
pub const SUBSCRIBER_BUFFER: usize = 64;
/// Cumulative text artifact bytes per task (§8); excess is cut and named.
pub const ARTIFACT_BYTES_MAX: usize = 1 << 20;
/// `ListTasks` default page when the request names none (SDK default).
const DEFAULT_PAGE: u16 = 50;
/// How long a cancel waits for owned work to stop before reporting the
/// task as still settling.
const CANCEL_WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Commands (plugin → Agents)
// ---------------------------------------------------------------------------

/// Trusted caller identity, constructed by the plugin from its configured
/// binding after HTTP authentication (§6.1). Never deserialized from a
/// request; no header, `tenant` or metadata can select another one.
#[derive(Clone, Debug)]
pub struct A2aPrincipal {
    /// Live `peer_bindings.id` of the authenticated credential.
    pub binding: Uuid,
    /// Export name the request addressed.
    pub export: String,
    /// The exported native session.
    pub session: SessionId,
    /// The peer's grant epoch the HTTP layer admitted under (credentials,
    /// exports and roots as configured then); cancelled when any of them
    /// changes, which ends every task and stream admitted under it.
    pub route: CancellationToken,
    /// 183 §7.6: set when the caller is a contact on `theuth/a2a/1`: its name, machine, the
    /// export and its cap (the inbox's `SetCap`, or the exported chat's `SetTurnCap`).
    /// `None` for an HTTP A2A peer (bearer), which keeps today's admission.
    pub contact: Option<super::ContactOrigin>,
    /// 183 §7.6 (W-27): the contact's first-contact answer for this export is `ask`: a new
    /// task waits in `TASK_STATE_AUTH_REQUIRED` until the owner answers the Attention item.
    pub ask_first: bool,
}

/// `ask` → `Result<SessionId, A2AError>` (183 §7.6): the capped inbox of (contact
/// `binding`, `origin.export`), an agent export: created on the first request (an isolated
/// root titled `<contact> · <export>`, recorded in `a2a_contexts`), reused after, and its cap
/// (`origin.cap`) installed on every resolve so it holds after a restart and a lowered cap
/// applies at once.
#[derive(Clone, Debug)]
pub struct ContactInbox {
    /// The contact's live binding.
    pub binding: Uuid,
    /// The exported agent (`<agent>@<engine>` or `theuth`).
    pub agent: String,
    /// Who asks, the export and the cap.
    pub origin: super::ContactOrigin,
}

impl A2aPrincipal {
    /// Authenticated endpoint key of this caller.
    fn key(&self) -> String {
        format!("peer:{}", self.binding)
    }
}

/// `ask` → `Result<SendMessageResponse, A2AError>`: `SendMessage`. Always a
/// `Task`; blocking (`returnImmediately` unset/false) waits off the mailbox
/// for a settled state.
#[derive(Clone, Debug)]
pub struct A2aSend {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request as decoded by the SDK.
    pub request: SendMessageRequest,
}

/// `ask` → `Result<TaskStream, A2AError>`: `SendStreamingMessage`; the
/// `Task` snapshot first, then ordered status/artifact updates until terminal.
#[derive(Clone, Debug)]
pub struct A2aStream {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request as decoded by the SDK.
    pub request: SendMessageRequest,
}

/// `ask` → `Result<Task, A2AError>`: `GetTask` with `historyLength`.
#[derive(Clone, Debug)]
pub struct A2aGet {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request.
    pub request: GetTaskRequest,
}

/// `ask` → `Result<ListTasksResponse, A2AError>`: caller-scoped page.
#[derive(Clone, Debug)]
pub struct A2aList {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request.
    pub request: ListTasksRequest,
}

/// `ask` → `Result<Task, A2AError>`: cancel one task's queued input and owned
/// work; the terminal race is decided by the store.
#[derive(Clone, Debug)]
pub struct A2aCancel {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request.
    pub request: CancelTaskRequest,
}

/// `ask` → `Result<TaskStream, A2AError>`: `SubscribeToTask`; snapshot first,
/// `UNSUPPORTED_OPERATION` on a terminal task.
#[derive(Clone, Debug)]
pub struct A2aSubscribe {
    /// Caller.
    pub principal: A2aPrincipal,
    /// The request.
    pub request: SubscribeToTaskRequest,
}

/// What the plugin's SSE route pulls: SDK stream events or a protocol error
/// that ends the stream (revocation, subscriber overflow).
pub type TaskStream = Pin<Box<dyn Stream<Item = Result<StreamResponse, A2AError>> + Send>>;

/// `ask` → `()`: the plugin reports `a2a.enabled` / listener state (§2.1);
/// turning the network off installs the revocation barrier.
#[derive(Clone, Copy, Debug)]
pub struct NetworkStatus {
    /// `a2a.enabled` as configured and loaded.
    pub enabled: bool,
    /// The listener is accepting connections.
    pub listening: bool,
}

/// How an outbound send failed (§7): before or after bytes may have left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteSendError {
    /// Nothing reached the peer (card, credential, validation, connect):
    /// the exchange is `failed`.
    Refused(String),
    /// Bytes may have left (timeout, reset, undecodable response): the
    /// exchange is `unknown`; no blind resend.
    Unknown(String),
}

/// The SDK response to an outbound `SendMessage`.
pub type RemoteReply = SendMessageResponse;

/// One outbound peer message for a registered sender.
#[derive(Clone, Debug)]
pub struct OutboundPeerMessage {
    /// The live binding the route was registered under; echo it in
    /// [`RemoteTaskUpdate::binding`].
    pub binding: Uuid,
    /// The sender session's durable record (redacted body, ids).
    pub record: PeerRecord,
    /// Idempotency key to present as the A2A `messageId`.
    pub request_id: String,
    /// Remote task to send on (a reply to its `INPUT_REQUIRED` question).
    pub task_id: Option<String>,
    /// Remote context, when known.
    pub context_id: Option<String>,
    /// Admitted generation; recheck [`Generation::live`] before dispatch.
    pub generation: Generation,
    /// 183 §7.7: the contact's export `peer:<contact>/<export>` named; `None` = the
    /// first export the contact granted (`peer:<contact>`), or an HTTP peer.
    pub export: Option<String>,
}

/// A registered remote route: the plugin's SDK client behind one binding.
#[async_trait]
pub trait PeerSender: Send + Sync {
    /// Send one message; `Ok` is the peer's typed response.
    async fn send(&self, outbound: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError>;
}

/// 183 §7.3: a contact's `peer_bindings.endpoint` and card interface URL are this prefix
/// plus its iroh endpoint id; an HTTP peer's endpoint is its card origin.
pub const IROH_PREFIX: &str = "iroh:";

/// 183 §7.3: the binding endpoint of the contact pinned to `eid`.
pub fn contact_endpoint(eid: &str) -> String {
    format!("{IROH_PREFIX}{eid}")
}

/// 183 §7.3: the endpoint id of a contact binding endpoint; `None` for an HTTP peer.
pub fn contact_eid(endpoint: &str) -> Option<&str> {
    endpoint.strip_prefix(IROH_PREFIX)
}

/// 183 §7.7 (F1): what one export a contact offers us is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OfferKind {
    /// One of the contact's chats: one capped turn per message.
    Chat,
    /// One of the contact's agents: a capped inbox.
    Agent,
}

/// 183 §7.7 (F1): whether a message to an offered export goes through now, as the sharer
/// reported it on its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OfferState {
    /// Delivered at once.
    Allowed,
    /// The first message waits until the owner presses Allow, then goes through.
    AskFirst,
    /// Answers nothing right now (chat archived, or its messaging off).
    Unavailable,
}

impl OfferState {
    /// The card tag and the `discover_peers` tag: `allowed` | `ask_first` | `unavailable`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::AskFirst => "ask_first",
            Self::Unavailable => "unavailable",
        }
    }
}

impl OfferKind {
    /// The card tag and the `discover_peers` tag: `chat` | `agent`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Agent => "agent",
        }
    }
}

/// 183 §7.7 (F1): one export a contact grants this engine, from the card it serves us.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContactOffer {
    /// The export name: `peer:<contact>/<name>`.
    pub name: String,
    /// Its description, else its name.
    pub title: String,
    /// Chat or agent.
    pub kind: OfferKind,
    /// Whether a message goes through now.
    pub state: OfferState,
}

/// `tell` (183 F1): every contact's latest offers, by alias, replacing the last set.
#[derive(Clone, Debug, Default)]
pub struct ContactOffers(pub BTreeMap<String, Vec<ContactOffer>>);

/// Bus event (183 F1): `discover_peers` ran and the contacts' offers were last refreshed
/// more than [`OFFERS_TTL`] ago; the a2a plugin refetches the stale cards.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactOffersStale;

/// 183 F1: how long a contact's offers count as fresh for `discover_peers`.
pub const OFFERS_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Bus event (183 F3, F11): a first-contact hold began or ended (held, released, failed,
/// revoked, expired); the a2a plugin re-derives Attention and tells the UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactsHeldChanged;

/// `ask` → `()`: register (or replace) the route for `binding` (§4). `roots`
/// are the exact local roots whose enabled trees may use it; cancelling
/// `generation` revokes the route and every in-flight result for it.
#[derive(Clone)]
pub struct RegisterPeerSender {
    /// Live binding id.
    pub binding: Uuid,
    /// Owner alias: the `peer:<name>` address.
    pub name: String,
    /// Bounded untrusted description for discovery.
    pub description: String,
    /// Granted local roots.
    pub roots: Vec<SessionId>,
    /// The SDK client adapter.
    pub sender: Arc<dyn PeerSender>,
    /// Route generation.
    pub generation: CancellationToken,
}

/// `ask` → `()`: forget a route; late results for it are refused.
#[derive(Clone, Copy, Debug)]
pub struct UnregisterPeerSender {
    /// The binding.
    pub binding: Uuid,
}

/// A typed update about a remote task the plugin follows (§6.3 client side).
// The `Task` variant is the payload this exists for; boxing it would only
// move the size to the plugin's side of the same message.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum RemoteUpdate {
    /// A task snapshot (poll or stream); stale duplicates are idempotent.
    Task(Task),
    /// A message-only response.
    Message(A2aMessage),
    /// The plugin refused the remote result (oversized, non-text): `failed`.
    Failed(String),
    /// The plugin stopped following without learning the outcome (route
    /// revoked mid-follow, follow ceiling, task vanished): `unknown`.
    Unknown(String),
}

/// `ask` → `Result<(), String>`: the plugin reports a remote task's progress
/// for the outbound exchange it was created by.
#[derive(Clone, Debug)]
pub struct RemoteTaskUpdate {
    /// The route that produced it.
    pub binding: Uuid,
    /// `OutboundPeerMessage::record.receipt.id`.
    pub exchange: Uuid,
    /// The update.
    pub update: RemoteUpdate,
}

/// `ask` → `Result<PeerBinding, String>`: the immutable identity behind an
/// alias at an endpoint (§5.1); a changed endpoint mints a new one.
#[derive(Clone, Debug)]
pub struct BindPeer {
    /// Owner alias.
    pub alias: String,
    /// Validated card/endpoint origin.
    pub endpoint: String,
}

/// `ask` → `Result<bool, String>`: revoke the live binding of `alias`.
#[derive(Clone, Debug)]
pub struct RevokePeerBinding {
    /// Owner alias.
    pub alias: String,
}

/// One validated remote peer for [`CommitPeerConfig`].
#[derive(Clone)]
pub struct PeerRoute {
    /// Owner alias: the `peer:<alias>` address.
    pub alias: String,
    /// Validated card/endpoint origin; a change mints a new binding.
    pub endpoint: String,
    /// Granted local roots for outbound use.
    pub roots: Vec<SessionId>,
    /// The validated SDK client; `None` for an inbound-only peer.
    pub sender: Option<Arc<dyn PeerSender>>,
    /// Route generation, cancelled by the plugin on the next replacement.
    pub generation: CancellationToken,
}

/// `ask` → `Result<Vec<PeerBinding>, String>` (same order as `routes`): one
/// reload's whole peer configuration, installed only after every card and
/// client validated (§6.1). Bindings change in one store transaction
/// (`revoke` first, then idempotent binds); on success the registered route
/// table becomes exactly `routes`, and every binding that fell out is
/// cancelled with its in-flight outbound marked `unknown`. On failure
/// nothing changes.
#[derive(Clone)]
pub struct CommitPeerConfig {
    /// The complete new peer set.
    pub routes: Vec<PeerRoute>,
    /// Aliases removed from configuration.
    pub revoke: Vec<String>,
}

/// 183 W-41: how [`ValidateExport`] starts its answer for a session that no longer exists (a
/// deleted exported chat, whose export the plugin then drops).
pub const UNKNOWN_SESSION: &str = "unknown session";

/// `ask` → `Result<Option<String>, String>` (183 W-41, §13 row 55): why the exported chat
/// `session` answers nothing right now: `chat archived`, `agent messaging off`, or `None`.
/// `Err` starts with [`UNKNOWN_SESSION`] for a deleted chat.
#[derive(Clone, Copy, Debug)]
pub struct ChatPaused {
    /// The exported chat.
    pub session: SessionId,
}

/// 183 W-41: [`ChatPaused`]'s reason for an archived chat.
pub const PAUSED_ARCHIVED: &str = "chat archived";
/// 183 §13 row 55: [`ChatPaused`]'s reason for a chat with `sessions.communication = 0`.
pub const PAUSED_MESSAGING: &str = "agent messaging off";

/// `ask` → `Result<(), String>`: an export names an existing native session
/// (§6.3: ACP proxies are not exportable), which the policy then knows (Z3-R4).
#[derive(Clone, Copy, Debug)]
pub struct ValidateExport {
    /// The configured session.
    pub session: SessionId,
}

/// `ask` → [`SharedPolicy`]: the plugin's synchronous view of enablement.
#[derive(Clone, Copy, Debug, Default)]
pub struct PeerPolicy;

/// `tell` (internal): an exported task hit the request deadline.
struct ScopeDeadline(Uuid);

/// `tell` (internal): the grant epoch an exported task was admitted under
/// was cancelled (credential, export or root change; binding gone).
struct ScopeRevoked(Uuid);

/// `tell` (internal): a spawned remote send returned.
struct OutboundSettled {
    exchange: Uuid,
    outcome: Result<RemoteReply, RemoteSendError>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct RemotePeer {
    name: String,
    roots: Vec<SessionId>,
    /// `None` for an inbound-only peer: a principal, never a destination.
    sender: Option<Arc<dyn PeerSender>>,
    generation: CancellationToken,
    /// 183 W-26: a contact (`iroh:` endpoint): every root may reach it, and it needs
    /// `agent.communication`, not `a2a.enabled`.
    contact: bool,
}

/// One started exported task: what its native work is bound to.
struct Scope {
    task: String,
    session: SessionId,
    principal: String,
    export: String,
    /// Final text of the latest bound turn (`Err` = the turn stopped by error).
    result: Option<Result<String, String>>,
    /// The latest bound turn was cancelled by the owner or the runtime cap.
    interrupted: bool,
    /// Pending outbound question to the initiating peer (`INPUT_REQUIRED`).
    question: Option<Uuid>,
    /// A local approval is waiting (`AUTH_REQUIRED`).
    approval: bool,
    /// Cancel requested (caller, deadline, revoke): owned work is stopping
    /// and no new input of this scope is admitted.
    cancelled: bool,
    /// Deadline / grant-epoch watcher of the started task.
    watch: Option<AbortHandle>,
}

/// A `submitted` task waiting for its session's scope: the admission it
/// was accepted under, kept so it starts under exactly that grant epoch and
/// fails as soon as that epoch ends, never revived by a later one.
struct QueuedTask {
    task: String,
    principal: String,
    export: String,
    route: CancellationToken,
    watch: AbortHandle,
}

struct Feed {
    tx: broadcast::Sender<StreamResponse>,
    count: Arc<AtomicUsize>,
}

/// Decrements the per-task and global subscriber counts when the stream drops.
struct SubscriberGuard {
    task: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.task.fetch_sub(1, Ordering::Relaxed);
        self.global.fetch_sub(1, Ordering::Relaxed);
    }
}

/// An outbound exchange in flight or followed on a remote task.
struct Outbound {
    binding: Uuid,
    session: SessionId,
    generation: Generation,
    /// Grant epoch of the route the send was admitted under.
    route: CancellationToken,
    task_id: Option<String>,
    /// 183 F8: the address as the sender wrote it (`sky`, `sky/hi-there`): what its receipt
    /// and the replies show instead of the binding id.
    alias: String,
    /// 183 F7: the route is a contact's.
    contact: bool,
}

/// Derived memory only (§7): routes, started scopes, subscribers and
/// outbound correlation. Rebuilt empty on restart, when every interrupted
/// task is already `failed` in the store.
#[derive(Default)]
pub struct NetworkState {
    peers: HashMap<Uuid, RemotePeer>,
    scopes: HashMap<Uuid, Scope>,
    /// request scope → accepted-not-started task and its admission.
    queued: HashMap<Uuid, QueuedTask>,
    /// session → its one active scope (the store allows one active task per
    /// session). Never used to attribute work: that is what `turns`,
    /// `pending` and `ops` are for (§6.3 binding rule).
    active: HashMap<SessionId, Uuid>,
    /// session → the scope whose input opened its running turn. Absent
    /// during a private turn or while idle.
    turns: HashMap<SessionId, (TurnId, Uuid)>,
    /// Admitted-but-unconsumed inputs of a scope by ticket id: each is bound
    /// as work `ticket:<id>` until it enters a turn or is dropped.
    pending: HashMap<u64, Uuid>,
    /// Operations admitted by a scope's turn, bound as work `op:<id>`.
    ops: HashMap<OperationId, Uuid>,
    feeds: HashMap<String, Feed>,
    subscribers: Arc<AtomicUsize>,
    /// Exchanges already handed to intake: a dedup hit re-enqueues nothing.
    enqueued: HashSet<Uuid>,
    outbound: HashMap<Uuid, Outbound>,
    /// 183 §7.6: inbox sessions resolved this run → their first contact row still owes the
    /// `<contact_inbox/>` notice (§11.5).
    inboxes: HashMap<SessionId, bool>,
    /// 183 §7.6: request scope → the contact behind it (its turn cap, its wrapper, its
    /// first-contact hold); gone with the scope.
    contact_scopes: HashMap<Uuid, super::ContactOrigin>,
    /// 183 W-27, F11: tasks waiting for a first-contact answer, by (contact, export), oldest
    /// first, with the admission each waits under. A held task waits aside: it is neither the
    /// session's active task nor the head of its queue.
    held: HashMap<(String, String), Vec<HeldTask>>,
    /// 183 F1: each contact's latest offers (alias → exports it grants us), from its card.
    offers: BTreeMap<String, Vec<ContactOffer>>,
    /// 183 F1: when `discover_peers` last asked for a refresh (unix ms; 0 = never).
    offers_asked_ms: std::sync::atomic::AtomicU64,
    /// 183 W-40: contact message id → the request scope of the A2A task on this (holding)
    /// engine whose exported chat lives on another engine; its answer comes back as a
    /// `contact_deliver` with `reply_to`.
    contact_waits: HashMap<Uuid, Uuid>,
    /// 183 W-40: on a chat's owner, the contact turns it runs for another engine: the
    /// pseudo-scope its input opened the turn under (the message id) → where the answer goes.
    contact_replies: HashMap<Uuid, super::ContactEnvelope>,
    /// 183 §7.6, W-25: inbox session → the exported agent's persona, recorded on every
    /// resolve (so after a restart too, before the contact's turn runs).
    inbox_personas: HashMap<SessionId, crate::supervisor::InboxPersona>,
    /// 183 Z3-R2: inbox session → the cap and allowlist its last admission installed,
    /// re-installed at every actor start (fail-closed when absent).
    inbox_limits: HashMap<SessionId, (tools::Risk, Vec<String>)>,
    /// 183 Z3-R2: the paired-machine count `mesh_default` last saw (re-run when contacts change).
    last_paired: Option<usize>,
}

/// 183 F5: a held first message fails after this long without the owner's answer.
pub const HOLD_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 183 F5: what a held task fails with when [`HOLD_TTL`] passes.
pub const HOLD_EXPIRED: &str = "the owner did not answer";

/// 183 W-27: one task held for a first-contact answer.
struct HeldTask {
    task: A2aTask,
    route: CancellationToken,
    /// Fails the task when its grant epoch ends or [`HOLD_TTL`] passes while it waits.
    watch: AbortHandle,
    since_ms: u64,
    /// 183 F11: the first line of its message.
    preview: String,
}

/// `ask` → `Result<usize, String>` (183 W-27): the owner answered the first-contact item of
/// `contact` (for `export`, or every export when `None`): `allow` starts the waiting tasks
/// (re-admitted under today's policy), otherwise they fail as not a contact. Answers how many.
#[derive(Clone, Debug)]
pub struct ContactRelease {
    /// The contact's alias.
    pub contact: String,
    /// The export, or all of them (Remove, Block).
    pub export: Option<String>,
    /// Let them through.
    pub allow: bool,
}

/// `ask` → `Vec<HeldContact>` (183 W-27): the (contact, export) pairs with a task waiting
/// for the owner's first-contact answer.
#[derive(Clone, Copy, Debug)]
pub struct ContactsHeld;

/// One waiting (contact, export), for its Attention item.
#[derive(Clone, Debug, PartialEq, Eq, kameo::Reply)]
pub struct HeldContact {
    /// The contact's alias.
    pub contact: String,
    /// The export it wants to reach.
    pub export: String,
    /// The session the first task would run in.
    pub session: SessionId,
    /// When the oldest waiting task arrived (ms since epoch).
    pub since_ms: u64,
    /// How many tasks wait.
    pub waiting: usize,
    /// 183 F11: the first line of the oldest waiting message.
    pub preview: String,
}

impl NetworkState {
    /// 183 §7.6: the exported agent's persona when `session` is a contact's inbox.
    pub(crate) fn inbox_persona(
        &self,
        session: SessionId,
    ) -> Option<&crate::supervisor::InboxPersona> {
        self.inbox_personas.get(&session)
    }

    /// 183 Z3-R3: `session` is a contact's inbox (its limits are installed).
    pub(crate) fn is_inbox(&self, session: SessionId) -> bool {
        self.inbox_limits.contains_key(&session)
    }

    /// 183 Z3-R2: the live contact routes (they count for `agent.communication`'s default).
    pub(crate) fn contact_count(&self) -> usize {
        self.peers
            .values()
            .filter(|p| p.contact && !p.generation.is_cancelled())
            .count()
    }

    pub(crate) fn last_paired(&self) -> Option<usize> {
        self.last_paired
    }

    pub(crate) fn set_last_paired(&mut self, n: usize) {
        self.last_paired = Some(n);
    }

    /// Tests: a live contact route named `name` (no sender).
    #[cfg(test)]
    pub(crate) fn insert_contact_for_test(&mut self, name: &str) {
        self.peers.insert(
            Uuid::now_v7(),
            RemotePeer {
                name: name.into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: true,
            },
        );
    }

    /// Tests: every route gone.
    #[cfg(test)]
    pub(crate) fn clear_peers_for_test(&mut self) {
        self.peers.clear();
    }

    /// Discovery rows (§5.1) for the routes granted to `root`: alias, kind and
    /// capabilities only; a card is not a heartbeat, so the state is `Idle`. 183 §7.7: a
    /// contact is a row for every root, tagged `contact` — except a contact's inbox, which
    /// sees no contact (Z3-R2, §7.5: a contact never learns our contact list).
    pub(crate) fn remote_peers(&self, root: SessionId) -> Vec<PeerInfo> {
        let inbox = self.inboxes.contains_key(&root);
        let row =
            |address: String, name: String, title: Option<String>, tags: Vec<String>| PeerInfo {
                address,
                name,
                kind: PeerKind::A2a,
                relationship: PeerRelationship::Remote,
                state: PeerState::Idle,
                send: true,
                reply: true,
                stream: false,
                activity: None,
                title,
                tags,
                engine: None,
                reachable: None,
            };
        let mut peers: Vec<&RemotePeer> = self
            .peers
            .values()
            .filter(|p| {
                !p.generation.is_cancelled()
                    && p.sender.is_some()
                    && ((p.contact && !inbox) || p.roots.contains(&root))
            })
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        let mut rows = Vec::with_capacity(peers.len());
        for p in peers {
            if !p.contact {
                rows.push(row(
                    format!("peer:{}", p.name),
                    p.name.clone(),
                    None,
                    Vec::new(),
                ));
                continue;
            }
            rows.push(row(
                format!("peer:{}", p.name),
                p.name.clone(),
                None,
                vec!["contact".to_owned()],
            ));
            // 183 F1: one row per chat or agent the contact shares with us, from its card.
            for o in self.offers.get(&p.name).into_iter().flatten() {
                rows.push(row(
                    format!("peer:{}/{}", p.name, o.name),
                    format!("{}/{}", p.name, o.name),
                    Some(o.title.clone()),
                    vec![
                        "contact".to_owned(),
                        o.kind.as_str().to_owned(),
                        o.state.as_str().to_owned(),
                    ],
                ));
            }
        }
        rows
    }

    /// 183 F1: replace every contact's offers.
    pub(crate) fn set_offers(&mut self, offers: BTreeMap<String, Vec<ContactOffer>>) {
        self.offers = offers;
    }

    /// 183 F1: `discover_peers` ran; `true` (once per [`OFFERS_TTL`]) when this engine has a
    /// contact and their offers should be refetched now.
    pub(crate) fn offers_due(&self) -> bool {
        let now = Utc::now().timestamp_millis().max(0) as u64;
        let last = self.offers_asked_ms.load(Ordering::Relaxed);
        let contacts = self
            .peers
            .values()
            .any(|p| p.contact && !p.generation.is_cancelled());
        if !contacts || now.saturating_sub(last) < OFFERS_TTL.as_millis() as u64 {
            return false;
        }
        self.offers_asked_ms.store(now, Ordering::Relaxed);
        true
    }

    /// 183 W-26: `peer:<name>` (or the binding of `peer:<binding>`) is a contact.
    pub(crate) fn is_contact(&self, name_or_key: &str) -> bool {
        let binding = name_or_key
            .strip_prefix("peer:")
            .and_then(|b| b.parse::<Uuid>().ok());
        match binding {
            Some(b) => self.peers.get(&b).is_some_and(|p| p.contact),
            None => self
                .peer_by_name(name_or_key)
                .is_some_and(|(_, p)| p.contact),
        }
    }

    /// The live route `name` names. 183 §7.7: a contact's `<name>/<export>` names the
    /// contact's route (the export rides on the request, [`OutboundPeerMessage::export`]).
    fn peer_by_name(&self, name: &str) -> Option<(Uuid, &RemotePeer)> {
        let contact = name
            .split_once('/')
            .filter(|(_, e)| {
                !e.is_empty()
                    && e.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                    })
            })
            .map(|(c, _)| c);
        self.peers
            .iter()
            .find(|(_, p)| {
                !p.generation.is_cancelled()
                    && (p.name == name || (p.contact && contact == Some(p.name.as_str())))
            })
            .map(|(id, p)| (*id, p))
    }

    /// Tests: the contact message queued for `session` on its owner (W-40).
    #[cfg(test)]
    pub(crate) fn contact_replies_for_test(
        &self,
        session: SessionId,
    ) -> Option<super::ContactEnvelope> {
        self.contact_replies
            .values()
            .find(|e| e.to == session)
            .cloned()
    }

    /// Tests: a restart lost every contact envelope (Z3-R3).
    #[cfg(test)]
    pub(crate) fn forget_contact_replies_for_test(&mut self) {
        self.contact_replies.clear();
    }

    fn feed(&mut self, task: &str) -> &Feed {
        self.feeds.entry(task.to_owned()).or_insert_with(|| Feed {
            tx: broadcast::channel(SUBSCRIBER_BUFFER).0,
            count: Arc::default(),
        })
    }

    /// A bounded subscription: `UNSUPPORTED_OPERATION` at the per-task or
    /// harness cap (§8).
    fn subscribe(
        &mut self,
        task: &str,
    ) -> Result<(broadcast::Receiver<StreamResponse>, SubscriberGuard), A2AError> {
        let global = Arc::clone(&self.subscribers);
        let feed = self.feed(task);
        if feed.count.load(Ordering::Relaxed) >= SUBSCRIBERS_PER_TASK
            || global.load(Ordering::Relaxed) >= SUBSCRIBERS_MAX
        {
            return Err(A2AError::unsupported_operation(
                "subscriber limit reached; retry later",
            ));
        }
        feed.count.fetch_add(1, Ordering::Relaxed);
        global.fetch_add(1, Ordering::Relaxed);
        Ok((
            feed.tx.subscribe(),
            SubscriberGuard {
                task: Arc::clone(&feed.count),
                global,
            },
        ))
    }

    fn emit(&mut self, task: &str, ev: StreamResponse) {
        if let Some(f) = self.feeds.get(task) {
            let _ = f.tx.send(ev);
        }
    }

    /// Forget a settled scope; dropping the feed ends its streams after the
    /// buffered terminal event. `ops` keeps its rows: a job the scope's turn
    /// started stays *its* work when it reports after the task ended, so
    /// [`Agents::network_input_queued`] refuses the result instead of it
    /// opening a private turn. Each row goes when its producer settles it.
    fn retire(&mut self, scope: Uuid) {
        self.contact_scopes.remove(&scope);
        for tasks in self.held.values_mut() {
            if let Some(i) = tasks.iter().position(|h| h.task.request_scope == scope) {
                tasks.remove(i).watch.abort();
            }
        }
        self.held.retain(|_, v| !v.is_empty());
        if let Some(q) = self.queued.remove(&scope) {
            q.watch.abort();
            self.feeds.remove(&q.task);
        }
        if let Some(sc) = self.scopes.remove(&scope) {
            if let Some(t) = sc.watch {
                t.abort();
            }
            if self.active.get(&sc.session) == Some(&scope) {
                self.active.remove(&sc.session);
            }
            if self
                .turns
                .get(&sc.session)
                .is_some_and(|(_, s)| *s == scope)
            {
                self.turns.remove(&sc.session);
            }
            self.pending.retain(|_, s| *s != scope);
            self.feeds.remove(&sc.task);
        }
    }
}

// ---------------------------------------------------------------------------
// SDK projection helpers
// ---------------------------------------------------------------------------

fn state_of(s: A2aTaskStatus) -> TaskState {
    match s {
        A2aTaskStatus::Submitted => TaskState::Submitted,
        A2aTaskStatus::Working => TaskState::Working,
        A2aTaskStatus::InputRequired => TaskState::InputRequired,
        A2aTaskStatus::AuthRequired => TaskState::AuthRequired,
        A2aTaskStatus::Completed => TaskState::Completed,
        A2aTaskStatus::Failed => TaskState::Failed,
        A2aTaskStatus::Canceled => TaskState::Canceled,
        A2aTaskStatus::Rejected => TaskState::Rejected,
    }
}

fn status_of(s: &TaskState) -> Option<A2aTaskStatus> {
    Some(match s {
        TaskState::Unspecified => return None,
        TaskState::Submitted => A2aTaskStatus::Submitted,
        TaskState::Working => A2aTaskStatus::Working,
        TaskState::InputRequired => A2aTaskStatus::InputRequired,
        TaskState::AuthRequired => A2aTaskStatus::AuthRequired,
        TaskState::Completed => A2aTaskStatus::Completed,
        TaskState::Failed => A2aTaskStatus::Failed,
        TaskState::Canceled => A2aTaskStatus::Canceled,
        TaskState::Rejected => A2aTaskStatus::Rejected,
    })
}

/// Settled for a blocking caller: terminal or waiting on the caller.
fn settled(s: A2aTaskStatus) -> bool {
    s.is_terminal()
        || matches!(
            s,
            A2aTaskStatus::InputRequired | A2aTaskStatus::AuthRequired
        )
}

fn agent_message(text: &str, task_id: &str, context_id: &str) -> A2aMessage {
    let mut m = A2aMessage::new(Role::Agent, vec![Part::text(text)]);
    m.task_id = Some(task_id.to_owned());
    m.context_id = Some(context_id.to_owned());
    m
}

fn text_artifact(text: &str) -> Artifact {
    Artifact {
        artifact_id: new_artifact_id(),
        name: Some("result".into()),
        description: None,
        parts: vec![Part::text(text)],
        metadata: None,
        extensions: None,
    }
}

/// The SDK snapshot with the store's status made authoritative (after
/// `fail_interrupted` the stored JSON lags; §7).
fn snapshot(task: &A2aTask) -> Task {
    let mut t: Task = serde_json::from_value(task.snapshot.clone()).unwrap_or_else(|_| Task {
        id: task.id.clone(),
        context_id: task.context_id.clone(),
        status: TaskStatus {
            state: TaskState::Unspecified,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    });
    let want = state_of(task.status);
    if t.status.state != want {
        t.status = TaskStatus {
            state: want,
            message: task
                .error
                .as_deref()
                .map(|e| agent_message(e, &task.id, &task.context_id)),
            timestamp: Some(task.updated_at),
        };
    }
    t
}

/// `a2a_server::task_store::apply_history_length` is `pub(crate)`; same rule.
fn trim_history(task: &mut Task, history_length: Option<i32>) {
    let Some(n) = history_length else { return };
    if n <= 0 {
        task.history = None;
        return;
    }
    if let Some(h) = task.history.as_mut() {
        let keep = n as usize;
        if h.len() > keep {
            h.drain(..h.len() - keep);
        }
    }
}

fn project(task: &A2aTask, history_length: Option<i32>, artifacts: bool) -> Task {
    let mut t = snapshot(task);
    trim_history(&mut t, history_length);
    if !artifacts {
        t.artifacts = None;
    }
    t
}

/// Cut `text` to at most `max` bytes on a char boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Validate one inbound message (§6.2 text profile, §8 bounds) and return
/// its redacted text. ProtoJSON leaves absent fields as empty defaults, so
/// every identity field is checked for presence, not just shape.
fn inbound_text(m: &A2aMessage, redact: impl Fn(&str) -> String) -> Result<String, A2AError> {
    if m.message_id.is_empty() || m.message_id.len() > EXTERNAL_ID_MAX {
        return Err(A2AError::invalid_params(
            "message.messageId is required and bounded",
        ));
    }
    if m.role != Role::User {
        return Err(A2AError::invalid_params("message.role must be ROLE_USER"));
    }
    for id in [m.task_id.as_deref(), m.context_id.as_deref()]
        .into_iter()
        .flatten()
    {
        if id.is_empty() || id.len() > EXTERNAL_ID_MAX {
            return Err(A2AError::invalid_params(
                "taskId/contextId must be nonempty and bounded",
            ));
        }
    }
    if m.parts.is_empty() {
        return Err(A2AError::invalid_params("message.parts is required"));
    }
    let mut text = String::new();
    for p in &m.parts {
        let Some(t) = p.as_text() else {
            return Err(A2AError::content_type_not_supported());
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(t);
    }
    if text.len() > PEER_TEXT_MAX {
        return Err(A2AError::invalid_params(format!(
            "message text exceeds {PEER_TEXT_MAX} bytes"
        )));
    }
    Ok(redact(&text))
}

/// Text of a remote message: every text part joined, `None` when there is none.
fn parts_text(parts: &[Part]) -> Option<String> {
    let texts: Vec<&str> = parts.iter().filter_map(Part::as_text).collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// Text of a remote task's artifacts, falling back to its status message.
fn task_text(t: &Task) -> String {
    let arts: Vec<String> = t
        .artifacts
        .iter()
        .flatten()
        .filter_map(|a| parts_text(&a.parts))
        .collect();
    if !arts.is_empty() {
        return arts.join("\n");
    }
    t.status
        .message
        .as_ref()
        .and_then(|m| parts_text(&m.parts))
        .unwrap_or_default()
}

fn comm_error(e: &anyhow::Error) -> Option<&CommError> {
    e.downcast_ref::<CommError>()
}

fn internal(e: anyhow::Error) -> A2AError {
    tracing::error!("a2a: {e:#}");
    A2AError::internal("internal error")
}

/// A caller-scoped task handle for off-mailbox waits.
#[derive(Clone)]
struct TaskRef {
    principal: String,
    export: String,
    id: String,
}

impl TaskRef {
    fn of(task: &A2aTask) -> Self {
        Self {
            principal: task.principal.clone(),
            export: task.export.clone(),
            id: task.id.clone(),
        }
    }

    async fn load(&self, store: &crate::store::Store) -> Result<A2aTask, A2AError> {
        let scope = A2aScope {
            principal: &self.principal,
            export: &self.export,
        };
        store
            .a2a_task(scope, &self.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| A2AError::task_not_found(&self.id))
    }
}

/// The blocking wait (§6.2 `returnImmediately: false`): the current store
/// row decides, the feed only wakes the loop. `terminal_only` (cancel) does
/// not stop at `input_required`. Revocation ends the wait with whatever the
/// store says after the barrier ran.
async fn wait_settled(
    store: crate::store::Store,
    task: TaskRef,
    mut rx: broadcast::Receiver<StreamResponse>,
    generation: Generation,
    history_length: Option<i32>,
    terminal_only: bool,
) -> Result<Task, A2AError> {
    let mut revoked = false;
    loop {
        if revoked {
            // Let the barrier's failure land, then report the row.
            let _ = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        }
        let row = task.load(&store).await?;
        let done = if terminal_only {
            row.status.is_terminal()
        } else {
            settled(row.status)
        };
        if done || revoked {
            return Ok(project(&row, history_length, true));
        }
        tokio::select! {
            r = rx.recv() => match r {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                // The feed is dropped when the scope retires: reload the terminal row.
                Err(RecvError::Closed) => revoked = true,
            },
            () = generation.revoked() => revoked = true,
        }
    }
}

/// Snapshot first, then live events until terminal, revocation or overflow.
/// The guard travels in the state so the counts drop exactly when the
/// consumer drops the stream.
fn follow(
    first: Task,
    rx: broadcast::Receiver<StreamResponse>,
    generation: Generation,
    route: CancellationToken,
    guard: SubscriberGuard,
) -> TaskStream {
    let done = first.status.state.is_terminal();
    let head = stream::once(async move { Ok(StreamResponse::Task(first)) });
    let tail = stream::unfold((done, rx, guard), move |(done, mut rx, guard)| {
        let generation = generation.clone();
        let route = route.clone();
        async move {
            if done {
                return None;
            }
            let item = tokio::select! {
                r = rx.recv() => match r {
                    Ok(ev) => {
                        let done = match &ev {
                            StreamResponse::Task(t) => t.status.state.is_terminal(),
                            StreamResponse::StatusUpdate(u) => u.status.state.is_terminal(),
                            _ => false,
                        };
                        (Ok(ev), done)
                    }
                    Err(RecvError::Lagged(_)) => (
                        Err(A2AError::internal("subscriber fell behind; resubscribe for the current snapshot")),
                        true,
                    ),
                    Err(RecvError::Closed) => return None,
                },
                () = generation.revoked() => {
                    (Err(A2AError::unsupported_operation("communication disabled")), true)
                }
                () = route.cancelled() => {
                    (Err(A2AError::unsupported_operation("peer route revoked")), true)
                }
            };
            Some((item.0, (item.1, rx, guard)))
        }
    });
    Box::pin(head.chain(tail))
}

/// What an admitted send hands to the unary and streaming paths.
pub(crate) struct Admitted {
    pub(crate) task: A2aTask,
    generation: Generation,
    route: CancellationToken,
}

// ---------------------------------------------------------------------------
// Inbound lifecycle
// ---------------------------------------------------------------------------

impl Agents {
    /// Authorize a principal for one operation: live binding, native export
    /// session, enabled route with the network on (§6.1, §8 item 4).
    async fn admit_principal(&mut self, p: &A2aPrincipal) -> Result<Generation, A2AError> {
        let unavailable =
            |why: String| A2AError::unsupported_operation(format!("export unavailable: {why}"));
        match self.store.peer_binding_by_id(p.binding).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(unavailable("peer binding revoked".into())),
            Err(e) => return Err(internal(e)),
        }
        if p.route.is_cancelled()
            || self
                .network
                .peers
                .get(&p.binding)
                .is_some_and(|r| r.generation.is_cancelled())
        {
            return Err(unavailable("peer route revoked".into()));
        }
        match self.store.session(p.session).await {
            Ok(Some(s)) if s.agent.is_none() => {}
            Ok(Some(_)) => return Err(unavailable("export is not a native session".into())),
            Ok(None) => return Err(unavailable("export session missing".into())),
            Err(e) => return Err(internal(e)),
        }
        if p.contact.is_none() {
            return self
                .communication
                .0
                .write()
                .remote(p.session)
                .map_err(|e| unavailable(e.to_string()));
        }
        // 183 W-41, §13 row 55: an archived chat, or one that takes no agent messages, is
        // paused for contacts (an inbox is a root of its own and pauses the same way).
        match self.chat_paused(p.session).await {
            Ok(None) => {}
            Ok(Some(reason)) if reason == PAUSED_MESSAGING => {
                return Err(A2AError::unsupported_operation(
                    super::ContactRefusal::NoAgentMessages.to_string(),
                ));
            }
            Ok(Some(reason)) => {
                return Err(A2AError::unsupported_operation(
                    super::ContactRefusal::Paused { reason }.to_string(),
                ));
            }
            Err(e) => return Err(unavailable(e)),
        }
        // W-26: a contact needs `agent.communication`, not `a2a.enabled`.
        self.communication
            .0
            .write()
            .contact(p.session)
            .map_err(|e| unavailable(e.to_string()))
    }

    /// Validate, persist and start (or queue / resume) one `SendMessage`.
    pub(crate) async fn admit_send(
        &mut self,
        p: &A2aPrincipal,
        req: &SendMessageRequest,
    ) -> Result<Admitted, A2AError> {
        let generation = self.admit_principal(p).await?;
        if req
            .configuration
            .as_ref()
            .is_some_and(|c| c.task_push_notification_config.is_some())
        {
            return Err(A2AError::push_notification_not_supported());
        }
        let text = inbound_text(&req.message, |t| self.redact(t).into_owned())?;
        let key = p.key();
        let scope = A2aScope {
            principal: &key,
            export: &p.export,
        };
        let recipient = format!("session:{}", p.session);
        let mut message = req.message.clone();
        let generation_i64 = generation.revision() as i64;

        // Resume: an owned, settled-but-open task whose context agrees.
        if let Some(task_id) = message.task_id.clone() {
            let cur = self
                .store
                .a2a_task(scope, &task_id)
                .await
                .map_err(internal)?
                .ok_or_else(|| A2AError::task_not_found(&task_id))?;
            if message
                .context_id
                .as_ref()
                .is_some_and(|c| *c != cur.context_id)
            {
                return Err(A2AError::invalid_params(
                    "contextId does not match the referenced task",
                ));
            }
            if cur.status.is_terminal() {
                return Err(A2AError::unsupported_operation(format!(
                    "task is terminal: {task_id}"
                )));
            }
            // 183 W-27, Z3-R2: a task held for the owner's first-contact answer runs only once
            // the owner releases it; a message on it never skips the wait.
            if cur.status == A2aTaskStatus::AuthRequired {
                return Err(A2AError::unsupported_operation(format!(
                    "waiting: the owner has not allowed this task yet: {task_id}"
                )));
            }
            if !settled(cur.status) {
                return Err(A2AError::unsupported_operation(format!(
                    "task is still running: {task_id}"
                )));
            }
            message.context_id = Some(cur.context_id.clone());
            let question = self
                .network
                .scopes
                .get(&cur.request_scope)
                .and_then(|s| s.question);
            let a = AcceptExchange {
                sender: &key,
                recipient: &recipient,
                sender_session: None,
                recipient_session: Some(p.session),
                request_id: Some(&message.message_id),
                reply_to: question,
                request_scope: None,
                generation: generation_i64,
                body: &text,
            };
            let (receipt, task) = self
                .store
                .accept_a2a_request(
                    scope,
                    &a,
                    A2aRequest::Resume {
                        task_id: &task_id,
                        context_id: message.context_id.as_deref(),
                    },
                )
                .await
                .map_err(|e| map_accept(&e, &task_id))?;
            let route = p.route.clone();
            if !self.network.enqueued.insert(receipt.id) {
                return Ok(Admitted {
                    task,
                    generation,
                    route,
                });
            }
            let task = self
                .resume_task(task, message, receipt, question, &generation, &route)
                .await?;
            return Ok(Admitted {
                task,
                generation,
                route,
            });
        }

        // 183 Z3-R2: a contact's queued work is bounded (each task costs a model turn): at
        // most CONTACT_QUEUED tasks waiting (queued or held for the owner) per (contact,
        // export). Z3-R3: no bound on contexts — a sender that never reuses one (ours) would
        // be locked out for good, and the waiting bound already limits the work.
        if p.contact.is_some() {
            let waiting = self
                .network
                .queued
                .values()
                .filter(|q| q.principal == key && q.export == p.export)
                .count()
                + p.contact.as_ref().map_or(0, |o| {
                    self.network
                        .held
                        .get(&(o.contact.clone(), o.export.clone()))
                        .map_or(0, Vec::len)
                });
            if waiting >= CONTACT_QUEUED {
                return Err(A2AError::unsupported_operation(format!(
                    "busy: {waiting} tasks already wait here; retry later"
                )));
            }
        }
        // New task in an owned (or freshly minted) context.
        let context_id = match message.context_id.clone() {
            Some(ctx) => {
                self.store
                    .a2a_context(scope, &ctx)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| A2AError::invalid_params("unknown contextId"))?;
                ctx
            }
            None => {
                let ctx = new_context_id();
                self.store
                    .create_a2a_context(scope, &ctx, p.session)
                    .await
                    .map_err(internal)?;
                ctx
            }
        };
        let minted = message.context_id.is_none();
        let id = new_task_id();
        message.context_id = Some(context_id.clone());
        message.task_id = Some(id.clone());
        let snap = Task {
            id: id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Some(Utc::now()),
            },
            artifacts: None,
            history: Some(vec![message]),
            metadata: None,
        };
        let a = AcceptExchange {
            sender: &key,
            recipient: &recipient,
            sender_session: None,
            recipient_session: Some(p.session),
            request_id: Some(&req.message.message_id),
            reply_to: None,
            request_scope: None,
            generation: generation_i64,
            body: &text,
        };
        let (receipt, task) = self
            .store
            .accept_a2a_request(
                scope,
                &a,
                A2aRequest::New {
                    task_id: &id,
                    context_id: &context_id,
                    snapshot: serde_json::to_value(&snap).map_err(|e| internal(e.into()))?,
                },
            )
            .await
            .map_err(|e| map_accept(&e, &id))?;
        // Z3-R4: a retry of an earlier message (same messageId) was answered from that one's
        // task: the context minted for it holds nothing, so it does not stay behind.
        if minted
            && task.context_id != context_id
            && let Err(e) = self.store.drop_unused_a2a_context(scope, &context_id).await
        {
            tracing::warn!(context = %context_id, "a2a: retry's context not dropped: {e:#}");
        }
        let route = p.route.clone();
        if !self.network.enqueued.insert(receipt.id) {
            // Durable dedup: the existing task, whatever it has become.
            return Ok(Admitted {
                task,
                generation,
                route,
            });
        }
        if let Some(origin) = &p.contact {
            self.network
                .contact_scopes
                .insert(task.request_scope, origin.clone());
            // W-27, F11: the first task for (contact, export) waits for the owner, aside: it
            // takes neither the session's active slot nor the head of its queue, so the chat
            // keeps working until the owner answers.
            if p.ask_first {
                let task = self.hold_task(task, origin, route.clone()).await;
                return Ok(Admitted {
                    task,
                    generation,
                    route,
                });
            }
        }
        let task = self
            .start_or_queue(task, generation.clone(), route.clone())
            .await;
        Ok(Admitted {
            task,
            generation,
            route,
        })
    }

    /// A fresh `submitted` task starts now when the session's scope is free,
    /// otherwise waits in accepted order (§6.3 one active scope per session)
    /// under the admission it was accepted with: it starts under that grant
    /// epoch or fails the moment the epoch ends, whichever comes first.
    async fn start_or_queue(
        &mut self,
        task: A2aTask,
        generation: Generation,
        route: CancellationToken,
    ) -> A2aTask {
        match self.active_task(task.session).await {
            Ok(None) => {
                let task = self.start_task(task, generation, route).await;
                if task.status.is_terminal() {
                    self.start_next(task.session).await;
                }
                task
            }
            Ok(Some(_)) => {
                let scope = task.request_scope;
                let watched = route.clone();
                let watch = tokio::spawn(async move {
                    watched.cancelled().await;
                    crate::continuation::notify(ScopeRevoked(scope)).await;
                })
                .abort_handle();
                self.network.queued.insert(
                    scope,
                    QueuedTask {
                        task: task.id.clone(),
                        principal: task.principal.clone(),
                        export: task.export.clone(),
                        route,
                        watch,
                    },
                );
                task
            }
            Err(e) => {
                tracing::error!("a2a: active task lookup: {e:#}");
                task
            }
        }
    }

    /// The session's active task (the store allows one): a held first-contact task waits
    /// aside (183 F11) and never counts, though it is stored `AUTH_REQUIRED`.
    async fn active_task(&self, session: SessionId) -> anyhow::Result<Option<A2aTask>> {
        let held: HashSet<Uuid> = self
            .network
            .held
            .values()
            .flatten()
            .map(|h| h.task.request_scope)
            .collect();
        if held.is_empty() {
            return self.store.active_a2a_task(session).await;
        }
        Ok(self
            .store
            .open_a2a_tasks(Some(session))
            .await?
            .into_iter()
            .find(|t| {
                matches!(
                    t.status,
                    A2aTaskStatus::Working
                        | A2aTaskStatus::InputRequired
                        | A2aTaskStatus::AuthRequired
                ) && !held.contains(&t.request_scope)
            }))
    }

    /// Working transition, scope registration, deadline / grant-epoch watch
    /// and intake of the task's initiating exchange. A refused intake fails
    /// the task visibly.
    async fn start_task(
        &mut self,
        task: A2aTask,
        generation: Generation,
        route: CancellationToken,
    ) -> A2aTask {
        let Some(exchange) = task.dedup_exchange else {
            return self
                .fail_task(&task, "task has no initiating exchange")
                .await
                .unwrap_or(task);
        };
        let record = match self.store.exchange_record(exchange).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return self
                    .fail_task(&task, "initiating exchange missing")
                    .await
                    .unwrap_or(task);
            }
            Err(e) => {
                tracing::error!("a2a: exchange record: {e:#}");
                return task;
            }
        };
        let task = match self
            .transition(&task, A2aTaskStatus::Working, None, None, None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(task = %task.id, "a2a: working transition: {e:#}");
                return task;
            }
        };
        self.register_scope(&task, route);
        // 183 W-40: a contact's message for an exported chat another engine of the mesh
        // owns goes to that owner; its answer comes back as a `contact_deliver` reply.
        if let Some(origin) = self
            .network
            .contact_scopes
            .get(&task.request_scope)
            .cloned()
            && !self.network.inboxes.contains_key(&task.session)
            && let Some(owner) = self.remote_owner(task.session).await
        {
            let envelope = super::ContactEnvelope {
                id: record.receipt.id,
                to: task.session,
                text: record.text,
                origin,
                reply_to: None,
                request_id: Some(task.id.clone()),
            };
            self.forward_contact(task.request_scope, owner, envelope, true)
                .await;
            return task;
        }
        if let Err(e) = self.enqueue_peer(record, generation, true, false).await {
            return self
                .fail_task(&task, &format!("intake refused: {e}"))
                .await
                .unwrap_or(task);
        }
        task
    }

    /// Bind the task's scope: one watcher task ends it at the request
    /// deadline or when the grant epoch it was admitted under is cancelled.
    fn register_scope(&mut self, task: &A2aTask, route: CancellationToken) {
        let scope = task.request_scope;
        let after = self.request_deadline();
        let watch = tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(after) => {
                    crate::continuation::notify(ScopeDeadline(scope)).await;
                }
                () = route.cancelled() => {
                    crate::continuation::notify(ScopeRevoked(scope)).await;
                }
            }
        })
        .abort_handle();
        self.network.scopes.insert(
            scope,
            Scope {
                task: task.id.clone(),
                session: task.session,
                principal: task.principal.clone(),
                export: task.export.clone(),
                result: None,
                interrupted: false,
                question: None,
                approval: false,
                cancelled: false,
                watch: Some(watch),
            },
        );
        self.network.active.insert(task.session, scope);
    }

    /// The caller answered an `INPUT_REQUIRED` / `AUTH_REQUIRED` task: the
    /// snapshot returns to `working` before the receipt leaves (§3.4 ordering),
    /// then the answer either completes the awaiting `send_message` call or
    /// becomes the scope's next isolated input.
    async fn resume_task(
        &mut self,
        task: A2aTask,
        message: A2aMessage,
        receipt: PeerReceipt,
        question: Option<Uuid>,
        generation: &Generation,
        route: &CancellationToken,
    ) -> Result<A2aTask, A2AError> {
        let mut snap = snapshot(&task);
        snap.history.get_or_insert_with(Vec::new).push(message);
        let task = self
            .transition_with(&task, snap, A2aTaskStatus::Working, None, None, None)
            .await
            .map_err(internal)?;
        if let Some(sc) = self.network.scopes.get_mut(&task.request_scope) {
            sc.question = None;
        } else {
            // The scope was lost (never started in this process); bind it now.
            self.register_scope(&task, route.clone());
        }
        let mut handed = false;
        if let Some(q) = question
            && let Ok(Some(orig)) = self.store.exchange_receipt(q).await
        {
            if orig.state == PeerMessageState::Replied && self.settle_wait(&orig) {
                handed = true;
                if let Err(e) = self
                    .store
                    .consume_exchange_direct(receipt.id, task.session, generation.revision() as i64)
                    .await
                {
                    tracing::warn!(exchange = %receipt.id, "a2a: direct consume: {e:#}");
                }
            }
            self.peer_changed(&orig).await;
        }
        if !handed {
            let record = self
                .store
                .exchange_record(receipt.id)
                .await
                .map_err(internal)?
                .ok_or_else(|| A2AError::internal("accepted exchange missing"))?;
            if let Err(e) = self
                .enqueue_peer(record, generation.clone(), true, false)
                .await
            {
                let t = self
                    .fail_task(&task, &format!("intake refused: {e}"))
                    .await
                    .map_err(internal)?;
                self.start_next(t.session).await;
                return Ok(t);
            }
        }
        Ok(task)
    }

    /// Apply a lifecycle step to the stored snapshot and fan it out.
    async fn transition(
        &mut self,
        task: &A2aTask,
        status: A2aTaskStatus,
        message: Option<&str>,
        artifact: Option<&str>,
        error: Option<&str>,
    ) -> anyhow::Result<A2aTask> {
        let snap = snapshot(task);
        self.transition_with(task, snap, status, message, artifact, error)
            .await
    }

    /// [`Self::transition`] over a caller-edited snapshot. Order on the feed:
    /// artifact, then status; a terminal status retires the scope.
    async fn transition_with(
        &mut self,
        task: &A2aTask,
        mut snap: Task,
        status: A2aTaskStatus,
        message: Option<&str>,
        artifact: Option<&str>,
        error: Option<&str>,
    ) -> anyhow::Result<A2aTask> {
        let mut events = Vec::with_capacity(2);
        if let Some(text) = artifact {
            let used: usize = snap
                .artifacts
                .iter()
                .flatten()
                .flat_map(|a| a.parts.iter().filter_map(Part::as_text))
                .map(str::len)
                .sum();
            let room = ARTIFACT_BYTES_MAX.saturating_sub(used);
            let mut art = text_artifact(cut(text, room));
            if text.len() > room {
                art.description = Some(format!(
                    "truncated: output exceeded the {ARTIFACT_BYTES_MAX}-byte artifact budget"
                ));
            }
            snap.artifacts
                .get_or_insert_with(Vec::new)
                .push(art.clone());
            events.push(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: task.id.clone(),
                context_id: task.context_id.clone(),
                artifact: art,
                append: None,
                last_chunk: Some(true),
                metadata: None,
            }));
        }
        let message =
            message.map(|t| agent_message(cut(t, PEER_TEXT_MAX), &task.id, &task.context_id));
        if let Some(m) = &message {
            snap.history.get_or_insert_with(Vec::new).push(m.clone());
        }
        snap.status = TaskStatus {
            state: state_of(status),
            message,
            timestamp: Some(Utc::now()),
        };
        events.push(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            status: snap.status.clone(),
            metadata: None,
        }));
        let scope = A2aScope {
            principal: &task.principal,
            export: &task.export,
        };
        let updated = self
            .store
            .update_a2a_task(
                scope,
                &task.id,
                task.revision,
                status,
                &serde_json::to_value(&snap)?,
                error,
            )
            .await?;
        for ev in events {
            self.network.emit(&task.id, ev);
        }
        if status.is_terminal() {
            self.network.retire(task.request_scope);
        }
        Ok(updated)
    }

    /// Terminal failure with a reason; the scope's queued intake fails with
    /// it. Does not advance the session's queue: callers that freed the
    /// scope call [`Self::start_next`], which itself fails tasks that cannot
    /// start (so this must not call back into it).
    async fn fail_task(&mut self, task: &A2aTask, why: &str) -> anyhow::Result<A2aTask> {
        let why = self.redact(why).into_owned();
        self.transition(task, A2aTaskStatus::Failed, Some(&why), None, Some(&why))
            .await
    }

    /// The session's scope is free: start the oldest queued task under the
    /// admission it was accepted with (never a newer grant epoch: a rotated
    /// credential or changed export does not revive queued work). A task
    /// that fails at start frees the scope again, so the loop moves on to the
    /// next one in accepted order; a task left non-terminal without a scope
    /// (store error at start) is not retried here.
    async fn start_next(&mut self, session: SessionId) {
        while !self.network.active.contains_key(&session) {
            let next = match self.store.next_a2a_task(session).await {
                Ok(Some(t)) => t,
                Ok(None) => return,
                Err(e) => {
                    tracing::error!(session = %session, "a2a: next task: {e:#}");
                    return;
                }
            };
            let binding = next
                .principal
                .strip_prefix("peer:")
                .and_then(|b| b.parse::<Uuid>().ok());
            // The admission is memory: a task accepted by an earlier process
            // has none and cannot start (its interrupted siblings already
            // failed on restart).
            let admitted = self.network.queued.remove(&next.request_scope);
            let (binding, admitted) = match (binding, admitted) {
                (Some(b), Some(q)) => (b, q),
                (None, _) => {
                    if self.fail_task(&next, "malformed principal").await.is_err() {
                        return;
                    }
                    continue;
                }
                (_, None) => {
                    if self
                        .fail_task(&next, "admission lost before start")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            };
            admitted.watch.abort();
            if admitted.route.is_cancelled() {
                // The epoch ended while it waited and its `ScopeRevoked` is
                // still in flight: never start under a dead grant.
                match self.fail_task(&next, "peer route revoked").await {
                    Ok(_) => continue,
                    Err(_) => return,
                }
            }
            let p = A2aPrincipal {
                binding,
                export: next.export.clone(),
                session,
                route: admitted.route.clone(),
                contact: self
                    .network
                    .contact_scopes
                    .get(&next.request_scope)
                    .cloned(),
                ask_first: false,
            };
            let started = match self.admit_principal(&p).await {
                Ok(generation) => self.start_task(next, generation, admitted.route).await,
                Err(e) => match self.fail_task(&next, &e.message).await {
                    Ok(_) => continue,
                    Err(_) => return,
                },
            };
            if !started.status.is_terminal() {
                return;
            }
        }
    }

    /// The stored task of a started or queued scope.
    async fn scope_task(&self, scope: Uuid) -> Option<A2aTask> {
        let (principal, export, id) = match self.network.scopes.get(&scope) {
            Some(sc) => (&sc.principal, &sc.export, &sc.task),
            None => {
                let q = self.network.queued.get(&scope)?;
                (&q.principal, &q.export, &q.task)
            }
        };
        let s = A2aScope { principal, export };
        match self.store.a2a_task(s, id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(task = %id, "a2a: load task: {e:#}");
                None
            }
        }
    }

    /// Settle the session's active task once its scope owns no more work.
    /// Only the scope's own events call this (its turns, queued inputs,
    /// operations and children), never the session's unrelated private
    /// activity (§6.3: never on `TurnFinished` alone); the store's work
    /// ledger refuses a premature `completed`. Idempotent.
    async fn try_settle(&mut self, session: SessionId) {
        let Some(&scope) = self.network.active.get(&session) else {
            return;
        };
        let Some(task) = self.scope_task(scope).await else {
            return;
        };
        let sc = &self.network.scopes[&scope];
        let (status, message, artifact, error): (
            A2aTaskStatus,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = if task.cancel_requested || sc.interrupted {
            (A2aTaskStatus::Canceled, Some("canceled".into()), None, None)
        } else if sc.question.is_some() || sc.approval {
            return;
        } else {
            match &sc.result {
                None => return,
                Some(Err(e)) => (
                    A2aTaskStatus::Failed,
                    Some(e.clone()),
                    None,
                    Some(e.clone()),
                ),
                Some(Ok(text)) => (A2aTaskStatus::Completed, None, Some(text.clone()), None),
            }
        };
        match self
            .transition(
                &task,
                status,
                message.as_deref(),
                artifact.as_deref(),
                error.as_deref(),
            )
            .await
        {
            Ok(_) => self.start_next(session).await,
            Err(e) if comm_error(&e) == Some(&CommError::WorkPending) => {
                tracing::debug!(task = %task.id, "a2a: settlement waits for owned work");
            }
            Err(e) => tracing::error!(task = %task.id, "a2a: settle: {e:#}"),
        }
    }

    /// End a started task with `why`: its owned work is asked to stop, the
    /// task fails visibly, and the session's queue moves on.
    async fn abort_scope(&mut self, scope: Uuid, why: &str) {
        let Some(task) = self.scope_task(scope).await else {
            return;
        };
        if self.network.scopes.contains_key(&scope) {
            self.stop_scope(&task).await;
        }
        if let Ok(t) = self.fail_task(&task, why).await {
            self.start_next(t.session).await;
        }
    }

    /// Stop a started task's owned work (§8) and admit nothing more of its
    /// scope: every cancel path marks the scope here, so a result landing
    /// after the cancel is refused by [`Self::network_input_queued`].
    async fn stop_scope(&mut self, task: &A2aTask) {
        if let Some(sc) = self.network.scopes.get_mut(&task.request_scope) {
            sc.cancelled = true;
        }
        if let Err(e) = self
            .cancel_peer_scope(task.session, task.request_scope)
            .await
        {
            tracing::debug!(task = %task.id, "a2a: scope cancel: {e}");
        }
    }

    // ---- hooks (supervisor → network) ------------------------------------
    //
    // Ownership is explicit end to end: an input of a scope is bound as work
    // when admitted (`ticket:<id>`), the turn it opens when consumed
    // (`turn:<id>`), operations admitted from that turn (`op:<id>`) and
    // children spawned from it (`child:<id>`, supervisor). An input, turn or
    // operation with no scope is private and binds nothing — a human
    // message queued during an exported task never becomes its result.

    /// The ledger shows `session` idle with no owned activity.
    pub(crate) async fn network_session_idle(&mut self, session: SessionId) {
        self.try_settle(session).await;
    }

    /// The scope whose input opened `session`'s running turn, if any: what a
    /// child spawned or an operation admitted from that turn belongs to.
    pub(crate) fn network_turn_scope(&self, session: SessionId) -> Option<Uuid> {
        self.network.turns.get(&session).map(|(_, s)| *s)
    }

    /// Bind native work `key` to the scope's task (idempotent).
    pub(crate) async fn network_bind_work(&mut self, scope: Uuid, key: &str) {
        let Some(sc) = self.network.scopes.get(&scope) else {
            return;
        };
        let task = sc.task.clone();
        if let Err(e) = self.store.bind_a2a_work(&task, key).await {
            tracing::warn!(task = %task, "a2a: bind {key}: {e:#}");
        }
    }

    /// Work `key` of the scope's task ended; the task may settle now.
    pub(crate) async fn network_settle_work(&mut self, scope: Uuid, key: &str) {
        let Some(sc) = self.network.scopes.get(&scope) else {
            return;
        };
        let (task, session) = (sc.task.clone(), sc.session);
        if let Err(e) = self.store.settle_a2a_work(&task, key).await {
            tracing::warn!(task = %task, "a2a: settle {key}: {e:#}");
        }
        self.try_settle(session).await;
    }

    /// An input of `scope` is about to be admitted (a task message, the
    /// result of owned work, an owned child's assignment). `true`: it is the
    /// scope's work until it enters a turn or is dropped. `false`: the scope
    /// is cancelled, interrupted (its bound turn was stopped by the owner or
    /// the runtime cap, so the task settles `canceled`) or gone — the caller
    /// must not route the input (a late job result never starts a turn for a
    /// cancelled or settled task) and releases its ticket itself.
    #[must_use]
    pub(crate) async fn network_input_queued(&mut self, scope: Uuid, ticket: InputTicket) -> bool {
        if !self
            .network
            .scopes
            .get(&scope)
            .is_some_and(|sc| !sc.cancelled && !sc.interrupted)
        {
            return false;
        }
        self.network.pending.insert(ticket.id, scope);
        self.network_bind_work(scope, &format!("ticket:{}", ticket.id))
            .await;
        true
    }

    /// A ticket entered `turn`. For a scope's input the turn becomes the
    /// scope's work — bound before the queued input settles, so no zero-work
    /// gap opens — and the session's running turn is the scope's (§6.3).
    pub(crate) async fn network_input_consumed(&mut self, m: &InputConsumed) {
        let Some(scope) = self.network.pending.remove(&m.id) else {
            return;
        };
        self.network.turns.insert(m.session, (m.turn, scope));
        self.network_bind_work(scope, &format!("turn:{}", m.turn))
            .await;
        self.network_settle_work(scope, &format!("ticket:{}", m.id))
            .await;
    }

    /// An admitted input never entered a turn (intake refused, dropped by a
    /// cancel): its work is over.
    pub(crate) async fn network_input_dropped(&mut self, ticket: InputTicket) {
        let Some(scope) = self.network.pending.remove(&ticket.id) else {
            return;
        };
        self.network_settle_work(scope, &format!("ticket:{}", ticket.id))
            .await;
    }

    /// An operation was admitted; one admitted from a scope's turn is the
    /// scope's work (§6.3 binding rule). A private turn binds nothing.
    pub(crate) async fn network_operation_admitted(&mut self, op: &Operation) {
        let Some(scope) = self.network_turn_scope(op.owner) else {
            return;
        };
        self.network.ops.insert(op.id, scope);
        self.network_bind_work(scope, &format!("op:{}", op.id))
            .await;
    }

    /// The scope a still-bound operation belongs to: what its result reports to.
    pub(crate) fn network_operation_scope(&self, op: OperationId) -> Option<Uuid> {
        self.network.ops.get(&op).copied()
    }

    /// The operation's producer settled it. `unknown` names work that cannot
    /// be proven stopped (47 §4.2 rule 6): the task fails with it rather
    /// than completing over it.
    pub(crate) async fn network_operation_settled(
        &mut self,
        op: OperationId,
        unknown: Option<&str>,
    ) {
        let Some(scope) = self.network.ops.remove(&op) else {
            return;
        };
        match unknown {
            None => self.network_settle_work(scope, &format!("op:{op}")).await,
            Some(why) => {
                self.abort_scope(scope, &format!("background work outcome unknown: {why}"))
                    .await;
            }
        }
    }

    /// A turn ended with its persisted text. A turn a scope's input opened is
    /// the scope's work; when it ran in the exported session itself its text
    /// is the result candidate until a later bound turn replaces it (an owned
    /// child's turn is work, never the result). A private turn (`scope:
    /// None`) is not attributed, whatever the session is serving.
    pub(crate) async fn network_turn_finished(&mut self, m: &PeerTurnFinished) {
        if self
            .network
            .turns
            .get(&m.session)
            .is_some_and(|(t, _)| *t == m.turn)
        {
            self.network.turns.remove(&m.session);
        }
        // 183 W-40: a turn this owner ran for another engine's contact: its answer goes back.
        if let Some(scope) = m.scope {
            let env = match self.network.contact_replies.remove(&scope) {
                Some(env) => Some(env),
                None if !self.network.scopes.contains_key(&scope) => {
                    self.resumed_contact_envelope(m.session, scope).await
                }
                None => None,
            };
            if let Some(env) = env {
                self.contact_turn_finished(env, m).await;
                return;
            }
        }
        let Some(scope) = m.scope.filter(|s| self.network.scopes.contains_key(s)) else {
            return;
        };
        if self.network.scopes[&scope].session == m.session {
            let text = self.redact(&m.text).into_owned();
            let sc = self.network.scopes.get_mut(&scope).expect("checked above");
            match m.stop_reason.as_str() {
                "cancelled" | "timed_out" | "interrupted" => sc.interrupted = true,
                "error" => sc.result = Some(Err("turn stopped by error".into())),
                _ => sc.result = Some(Ok(text)),
            }
        }
        self.network_settle_work(scope, &format!("turn:{}", m.turn))
            .await;
    }

    /// Z3-R3: the contact message that opened `session`'s resumed turn, rebuilt from its
    /// exchange (`contact:<holder>:<contact>`) after a restart lost the envelope, so the
    /// answer still reaches the holder (which only needs the message id it waits on).
    async fn resumed_contact_envelope(
        &self,
        session: SessionId,
        id: Uuid,
    ) -> Option<super::ContactEnvelope> {
        let record = match self.store.peer_record(session, id).await {
            Ok(r) => r?,
            Err(e) => {
                tracing::warn!(%session, "resumed contact turn: {e:#}");
                return None;
            }
        };
        let (holder, contact) = record.from.strip_prefix("contact:")?.split_once(':')?;
        Some(super::ContactEnvelope {
            id,
            to: session,
            text: String::new(),
            origin: super::ContactOrigin {
                contact: contact.to_owned(),
                machine: String::new(),
                export: String::new(),
                cap: tools::Risk::ReadOnly,
                holder: holder.to_owned(),
                tools: Vec::new(),
            },
            reply_to: None,
            request_id: None,
        })
    }

    /// A local approval is pending / resolved in `session` (§6.3 `AUTH_REQUIRED`).
    /// A pending one belongs to the running turn's scope, so a private turn's
    /// approval never surfaces on an exported task.
    pub(crate) async fn network_approval(&mut self, session: SessionId, pending: bool) {
        let scope = if pending {
            self.network_turn_scope(session)
        } else {
            self.network.active.get(&session).copied()
        };
        let Some(scope) = scope else {
            return;
        };
        if self.network.scopes[&scope].approval == pending {
            return;
        }
        let Some(task) = self.scope_task(scope).await else {
            return;
        };
        let (status, msg) = if pending {
            (
                A2aTaskStatus::AuthRequired,
                Some("the owner must approve a local action before this request continues"),
            )
        } else {
            (A2aTaskStatus::Working, None)
        };
        match self.transition(&task, status, msg, None, None).await {
            Ok(_) => {
                if let Some(sc) = self.network.scopes.get_mut(&scope) {
                    sc.approval = pending;
                }
            }
            Err(e) => tracing::warn!(task = %task.id, "a2a: approval state: {e:#}"),
        }
    }

    /// Communication was disabled for `subtree` (`None` = everywhere) or the
    /// network went off (§2.2, §8): open tasks of affected sessions fail,
    /// their owned work is asked to stop, streams end at the barrier.
    pub(crate) async fn network_revoked(&mut self, subtree: Option<SessionId>, reason: &str) {
        let open = match self.store.open_a2a_tasks(None).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("a2a: open tasks: {e:#}");
                return;
            }
        };
        for task in open {
            if let Some(root) = subtree {
                let inside = match self.store.lineage(task.session).await {
                    Ok(l) => l.chain.contains(&root),
                    Err(_) => true,
                };
                if !inside {
                    continue;
                }
            }
            if task.status.is_active() {
                self.stop_scope(&task).await;
            }
            let why = format!("communication_disabled: {reason}");
            if let Err(e) = self
                .transition(&task, A2aTaskStatus::Failed, Some(&why), None, Some(&why))
                .await
            {
                tracing::warn!(task = %task.id, "a2a: fail on revoke: {e:#}");
            }
        }
        // Outbound sends whose admission died: bytes may have left, so the
        // outcome is unknown, never "undelivered" (§2.2).
        self.abandon_outbound(
            |o| !o.generation.live(),
            "communication_disabled while in flight",
        )
        .await;
    }

    /// Drop the outbound entries matching `gone` and record their exchanges
    /// as `unknown` (any known remote task handle is kept).
    async fn abandon_outbound(&mut self, gone: impl Fn(&Outbound) -> bool, why: &str) {
        let ids: Vec<Uuid> = self
            .network
            .outbound
            .iter()
            .filter(|(_, o)| gone(o))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.network.outbound.remove(&id);
            match self.store.mark_exchange_unknown(id, None, why).await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::warn!(exchange = %id, "a2a: abandon outbound: {e:#}"),
            }
        }
    }
}

/// Store refusals of the acceptance transaction as protocol errors.
fn map_accept(e: &anyhow::Error, task_id: &str) -> A2AError {
    match comm_error(e) {
        Some(CommError::NotFound) => A2AError::task_not_found(task_id),
        Some(CommError::Terminal) => {
            A2AError::unsupported_operation(format!("task is terminal: {task_id}"))
        }
        Some(CommError::Conflict) => {
            A2AError::invalid_params("messageId was already used with different content")
        }
        Some(CommError::Busy) => {
            A2AError::unsupported_operation("recipient intake is full; retry later")
        }
        Some(CommError::TooLarge) => {
            A2AError::invalid_params(format!("message text exceeds {PEER_TEXT_MAX} bytes"))
        }
        Some(CommError::Invalid(why)) => A2AError::invalid_params(why.clone()),
        _ => {
            tracing::error!("a2a: accept: {e:#}");
            A2AError::internal("internal error")
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

impl Message<A2aSend> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<SendMessageResponse, A2AError>>;

    async fn handle(&mut self, m: A2aSend, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let admitted = match self.admit_send(&m.principal, &m.request).await {
            Ok(a) => a,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        let cfg = m.request.configuration.as_ref();
        let history_length = cfg.and_then(|c| c.history_length);
        if cfg.and_then(|c| c.return_immediately) == Some(true) || settled(admitted.task.status) {
            tx.send(Ok(SendMessageResponse::Task(project(
                &admitted.task,
                history_length,
                true,
            ))));
            return reply;
        }
        let rx = self.network.feed(&admitted.task.id).tx.subscribe();
        let store = self.store.clone();
        let task = TaskRef::of(&admitted.task);
        tokio::spawn(async move {
            let r = wait_settled(store, task, rx, admitted.generation, history_length, false).await;
            tx.send(r.map(SendMessageResponse::Task));
        });
        reply
    }
}

impl Message<A2aStream> for Agents {
    type Reply = Result<TaskStream, A2AError>;

    async fn handle(&mut self, m: A2aStream, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let admitted = self.admit_send(&m.principal, &m.request).await?;
        let (rx, guard) = self.network.subscribe(&admitted.task.id)?;
        let history_length = m
            .request
            .configuration
            .as_ref()
            .and_then(|c| c.history_length);
        Ok(follow(
            project(&admitted.task, history_length, true),
            rx,
            admitted.generation,
            admitted.route,
            guard,
        ))
    }
}

impl Message<A2aGet> for Agents {
    type Reply = Result<Task, A2AError>;

    async fn handle(&mut self, m: A2aGet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.admit_principal(&m.principal).await?;
        let key = m.principal.key();
        let scope = A2aScope {
            principal: &key,
            export: &m.principal.export,
        };
        if m.request.id.is_empty() || m.request.id.len() > EXTERNAL_ID_MAX {
            return Err(A2AError::invalid_params("id is required and bounded"));
        }
        let task = self
            .store
            .a2a_task(scope, &m.request.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| A2AError::task_not_found(&m.request.id))?;
        Ok(project(&task, m.request.history_length, true))
    }
}

impl Message<A2aList> for Agents {
    type Reply = Result<ListTasksResponse, A2AError>;

    async fn handle(&mut self, m: A2aList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.admit_principal(&m.principal).await?;
        let key = m.principal.key();
        let scope = A2aScope {
            principal: &key,
            export: &m.principal.export,
        };
        let r = &m.request;
        let limit = match r.page_size {
            None | Some(0) => DEFAULT_PAGE,
            Some(n) if n < 0 => return Err(A2AError::invalid_params("pageSize must be positive")),
            Some(n) => u16::try_from(n).unwrap_or(TASK_PAGE_MAX).min(TASK_PAGE_MAX),
        };
        let page = self
            .store
            .list_a2a_tasks(
                scope,
                TaskQuery {
                    context: r.context_id.as_deref().filter(|c| !c.is_empty()),
                    status: r.status.as_ref().and_then(status_of),
                    updated_after: r.status_timestamp_after,
                    after: r.page_token.as_deref().filter(|t| !t.is_empty()),
                    limit,
                },
            )
            .await
            .map_err(|e| match comm_error(&e) {
                Some(CommError::Invalid(why)) => A2AError::invalid_params(why.clone()),
                _ => internal(e),
            })?;
        let include_artifacts = r.include_artifacts == Some(true);
        Ok(ListTasksResponse {
            tasks: page
                .tasks
                .iter()
                .map(|t| project(t, r.history_length, include_artifacts))
                .collect(),
            next_page_token: page.next.unwrap_or_default(),
            page_size: i32::from(limit),
            total_size: i32::try_from(page.total).unwrap_or(i32::MAX),
        })
    }
}

impl Message<A2aCancel> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<Task, A2AError>>;

    /// Queued: canceled at once. Active: the scope's turn and queued input
    /// are asked to stop, then the reply waits (off the mailbox, bounded) for
    /// the ledger-driven terminal transition; owned work that cannot stop
    /// leaves the task `working` with the request recorded, never a claimed
    /// `CANCELED` (§8).
    async fn handle(&mut self, m: A2aCancel, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let r = match self.cancel_task(&m.principal, &m.request.id).await {
            Ok(r) => r,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        match r {
            Ok(task) => {
                tx.send(Ok(project(&task, None, true)));
            }
            Err((task, rx, generation)) => {
                let store = self.store.clone();
                let task = TaskRef::of(&task);
                tokio::spawn(async move {
                    let waited = tokio::time::timeout(
                        CANCEL_WAIT,
                        wait_settled(store.clone(), task.clone(), rx, generation, None, true),
                    )
                    .await;
                    // Owned work that would not stop: the honest current row.
                    let r = match waited {
                        Ok(r) => r,
                        Err(_) => task.load(&store).await.map(|t| project(&t, None, true)),
                    };
                    tx.send(r);
                });
            }
        }
        reply
    }
}

impl Agents {
    /// `Ok(Ok(task))` settled now; `Ok(Err(..))` the terminal transition is
    /// pending on owned work (wait on the feed).
    #[allow(clippy::type_complexity)]
    async fn cancel_task(
        &mut self,
        p: &A2aPrincipal,
        id: &str,
    ) -> Result<Result<A2aTask, (A2aTask, broadcast::Receiver<StreamResponse>, Generation)>, A2AError>
    {
        let generation = self.admit_principal(p).await?;
        if id.is_empty() || id.len() > EXTERNAL_ID_MAX {
            return Err(A2AError::invalid_params("id is required and bounded"));
        }
        let key = p.key();
        let scope = A2aScope {
            principal: &key,
            export: &p.export,
        };
        let cur = self
            .store
            .a2a_task(scope, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| A2AError::task_not_found(id))?;
        if cur.status.is_terminal() {
            return Err(A2AError::task_not_cancelable(id));
        }
        let cur = self
            .store
            .request_a2a_cancel(scope, id)
            .await
            .map_err(|e| match comm_error(&e) {
                Some(CommError::NotFound) => A2AError::task_not_found(id),
                Some(CommError::Terminal) => A2AError::task_not_cancelable(id),
                _ => internal(e),
            })?;
        if cur.status == A2aTaskStatus::Submitted {
            // Never started: nothing runs, nothing to wait for.
            let t = self
                .transition(
                    &cur,
                    A2aTaskStatus::Canceled,
                    Some("canceled before start"),
                    None,
                    None,
                )
                .await
                .map_err(internal)?;
            return Ok(Ok(t));
        }
        self.stop_scope(&cur).await;
        let rx = self.network.feed(&cur.id).tx.subscribe();
        // The turn's return reaches the ledger asynchronously; settle from the
        // idle hook, or now if nothing was running.
        self.try_settle(cur.session).await;
        match self.store.a2a_task(scope, id).await.map_err(internal)? {
            Some(t) if t.status.is_terminal() => Ok(Ok(t)),
            Some(t) => Ok(Err((t, rx, generation))),
            None => Err(A2AError::task_not_found(id)),
        }
    }
}

impl Message<A2aSubscribe> for Agents {
    type Reply = Result<TaskStream, A2AError>;

    async fn handle(&mut self, m: A2aSubscribe, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let generation = self.admit_principal(&m.principal).await?;
        if m.request.id.is_empty() || m.request.id.len() > EXTERNAL_ID_MAX {
            return Err(A2AError::invalid_params("id is required and bounded"));
        }
        let key = m.principal.key();
        let scope = A2aScope {
            principal: &key,
            export: &m.principal.export,
        };
        let task = self
            .store
            .a2a_task(scope, &m.request.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| A2AError::task_not_found(&m.request.id))?;
        if task.status.is_terminal() {
            return Err(A2AError::unsupported_operation(format!(
                "task is terminal: {}",
                task.id
            )));
        }
        let (rx, guard) = self.network.subscribe(&task.id)?;
        Ok(follow(
            project(&task, None, true),
            rx,
            generation,
            m.principal.route.clone(),
            guard,
        ))
    }
}

impl Message<NetworkStatus> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: NetworkStatus, _: &mut Context<Self, Self::Reply>) {
        let was = self
            .communication
            .0
            .read()
            .state(None)
            .map(|s| s.network_enabled);
        // Every generation-cancelling mutation runs under the intake barrier.
        // An unchanged report is silent: the plugin re-reports after every
        // global `CommunicationChanged`, so notifying here would loop.
        let Some(revision) = self
            .revoking(|p| p.set_network(m.enabled, m.listening))
            .await
        else {
            return;
        };
        if was == Ok(true) && !m.enabled {
            self.network_revoked(None, "network disabled").await;
        }
        self.rt
            .publish(CommunicationChanged {
                session: None,
                revision,
            })
            .await;
    }
}

impl Message<RegisterPeerSender> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: RegisterPeerSender, _: &mut Context<Self, Self::Reply>) {
        self.network.peers.insert(
            m.binding,
            RemotePeer {
                name: m.name,
                roots: m.roots,
                sender: Some(m.sender),
                generation: m.generation,
                contact: false,
            },
        );
    }
}

impl Message<UnregisterPeerSender> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterPeerSender, _: &mut Context<Self, Self::Reply>) {
        if let Some(p) = self.network.peers.remove(&m.binding) {
            p.generation.cancel();
        }
        self.abandon_outbound(|o| o.binding == m.binding, "peer route unregistered")
            .await;
    }
}

impl Message<CommitPeerConfig> for Agents {
    type Reply = Result<Vec<PeerBinding>, String>;

    async fn handle(
        &mut self,
        m: CommitPeerConfig,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut seen = HashSet::with_capacity(m.routes.len());
        for r in &m.routes {
            if !seen.insert(r.alias.as_str()) {
                return Err(format!("duplicate peer alias `{}`", r.alias));
            }
            if m.revoke.contains(&r.alias) {
                return Err(format!("peer `{}` is both bound and revoked", r.alias));
            }
        }
        let bind: Vec<(&str, &str)> = m
            .routes
            .iter()
            .map(|r| (r.alias.as_str(), r.endpoint.as_str()))
            .collect();
        let revoke: Vec<&str> = m.revoke.iter().map(String::as_str).collect();
        let bindings = self
            .store
            .commit_peer_bindings(&bind, &revoke)
            .await
            .map_err(|e| e.to_string())?;
        // The store committed: replace the route table. Bindings that fell out
        // (revoked aliases, rotated endpoints, dropped peers) are cancelled;
        // an unchanged binding keeps its id and simply takes the new sender.
        let keep: HashSet<Uuid> = bindings.iter().map(|b| b.id).collect();
        let gone: Vec<Uuid> = self
            .network
            .peers
            .keys()
            .filter(|id| !keep.contains(*id))
            .copied()
            .collect();
        for id in gone {
            if let Some(p) = self.network.peers.remove(&id) {
                p.generation.cancel();
            }
        }
        for (route, binding) in m.routes.into_iter().zip(&bindings) {
            // Inbound-only peers register too: their token ends open inbound
            // streams when the binding falls out of a later reload.
            self.network.peers.insert(
                binding.id,
                RemotePeer {
                    name: route.alias,
                    roots: route.roots,
                    sender: route.sender,
                    contact: contact_eid(&route.endpoint).is_some(),
                    generation: route.generation,
                },
            );
        }
        self.abandon_outbound(
            |o| !keep.contains(&o.binding),
            "peer route removed by configuration reload",
        )
        .await;
        // Z3-R2: contacts count for `agent.communication`'s default; re-evaluate it.
        self.mesh_default(None).await;
        Ok(bindings)
    }
}

impl Message<BindPeer> for Agents {
    type Reply = Result<PeerBinding, String>;

    async fn handle(&mut self, m: BindPeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .bind_peer(&m.alias, &m.endpoint)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<RevokePeerBinding> for Agents {
    type Reply = Result<bool, String>;

    async fn handle(
        &mut self,
        m: RevokePeerBinding,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let revoked = self
            .store
            .revoke_peer_binding(&m.alias)
            .await
            .map_err(|e| e.to_string())?;
        if revoked {
            // The binding's registered route (if any) dies with it.
            let gone: Vec<Uuid> = self
                .network
                .peers
                .iter()
                .filter(|(_, p)| p.name == m.alias)
                .map(|(id, _)| *id)
                .collect();
            for id in gone {
                if let Some(p) = self.network.peers.remove(&id) {
                    p.generation.cancel();
                }
            }
        }
        Ok(revoked)
    }
}

impl Message<ValidateExport> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ValidateExport,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.validate_export(m.session).await
    }
}

impl Agents {
    /// [`ValidateExport`].
    pub(crate) async fn validate_export(&mut self, session: SessionId) -> Result<(), String> {
        match self.store.session(session).await {
            // 183 Z3-R4: and the policy learns it now. `Live::open` asks the policy whether the
            // exported chat is enabled; a chat only another engine has run (created there
            // seconds ago) was unknown here until something else mirrored it, so a contact's
            // first message after a fresh grant was refused as "grants no export".
            Ok(Some(s)) if s.agent.is_none() => {
                self.mirror(session).await;
                Ok(())
            }
            Ok(Some(_)) => Err(format!(
                "session {session} runs an external harness; only native sessions are exportable"
            )),
            Ok(None) => Err(format!("{UNKNOWN_SESSION} {session}")),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

impl Message<ChatPaused> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(&mut self, m: ChatPaused, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.chat_paused(m.session).await
    }
}

impl Agents {
    /// [`ChatPaused`]: archived first, then the chat's own opt-out (`sessions.communication
    /// = 0`, replicated with the row, so it holds on whichever engine owns the chat).
    pub(crate) async fn chat_paused(
        &mut self,
        session: SessionId,
    ) -> Result<Option<String>, String> {
        match self.store.session(session).await {
            Ok(Some(s)) if s.archived => return Ok(Some(PAUSED_ARCHIVED.to_owned())),
            Ok(Some(_)) => {}
            Ok(None) => return Err(format!("{UNKNOWN_SESSION} {session}")),
            Err(e) => return Err(format!("{e:#}")),
        }
        self.mirror(session).await;
        let opted_out = self
            .communication
            .0
            .read()
            .state(Some(session))
            .is_ok_and(|s| s.opted_out);
        Ok(opted_out.then(|| PAUSED_MESSAGING.to_owned()))
    }
}

impl Message<ContactInbox> for Agents {
    type Reply = Result<SessionId, A2AError>;

    async fn handle(&mut self, m: ContactInbox, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.handle_inbox(m).await
    }
}

impl Agents {
    /// [`ContactInbox`].
    pub(crate) async fn handle_inbox(&mut self, m: ContactInbox) -> Result<SessionId, A2AError> {
        let key = format!("peer:{}", m.binding);
        let scope = A2aScope {
            principal: &key,
            export: &m.origin.export,
        };
        // W-25: the exported agent's persona, charter, model and ceiling (`<agent>@<engine>`
        // names the agent of this engine).
        let bare = m.agent.split('@').next().unwrap_or(&m.agent);
        let persona = match &self.identity {
            Some(source) => source.inbox(bare).await,
            None => None,
        };
        let id = match self.store.a2a_inbox(scope).await.map_err(internal)? {
            Some(id) => id,
            None => {
                // §7.6: isolated, so the contact-driven model reaches no other root.
                let model = persona.as_ref().and_then(|p| p.model.as_deref());
                let id = match self
                    .new_root(None, super::PeerScope::Isolated, model, None)
                    .await
                {
                    Ok(id) => id,
                    Err(e) if model.is_some() => {
                        tracing::warn!(agent = %m.agent, "a2a: inbox on the agent's model failed ({e}); the default stands in");
                        self.new_root(None, super::PeerScope::Isolated, None, None)
                            .await
                            .map_err(A2AError::internal)?
                    }
                    Err(e) => return Err(A2AError::internal(e)),
                };
                let title = format!("{} · {}", m.origin.contact, m.origin.export);
                match self.store.set_title(id, &title).await {
                    // The first contact turn must not retitle it from the wrapper.
                    Ok(()) => self.mark_titled(id).await,
                    Err(e) => tracing::warn!(session = %id, "a2a: inbox title: {e:#}"),
                }
                self.store
                    .create_a2a_context(scope, &new_context_id(), id)
                    .await
                    .map_err(internal)?;
                tracing::info!(session = %id, contact = %m.origin.contact, export = %m.origin.export, agent = %m.agent, "a2a: contact inbox opened");
                self.network.inboxes.insert(id, true);
                id
            }
        };
        self.network.inboxes.entry(id).or_insert(false);
        // Z3-R2: every later actor start of this session re-installs its limits (an inbox
        // opened by an older build is marked on its first resolve); Z3-R3: and a resumed turn
        // speaks as the agent again, though the roster's source joins after the boot resume.
        self.store
            .mark_a2a_inbox(id, persona.as_ref().and_then(|p| p.identity.as_deref()))
            .await
            .map_err(internal)?;
        // §7.6: on every resolve, so the cap holds after a restart and a change applies now;
        // the agent's own `tools:` ceiling holds there too (the stricter one wins).
        let cap = persona
            .as_ref()
            .and_then(|p| p.tools)
            .map_or(m.origin.cap, |t| t.min(m.origin.cap));
        match persona {
            Some(p) => self.network.inbox_personas.insert(id, p),
            None => self.network.inbox_personas.remove(&id),
        };
        // Z3-R2: and default-deny: only the export's allowlist exists in the inbox. Z3-R3: not
        // installed = not admitted.
        self.install_contact_limits(id, cap, m.origin.tools.clone())
            .await
            .map_err(|e| internal(anyhow::anyhow!(e)))?;
        Ok(id)
    }
}

/// 183 Z3-R2: the default-deny limits of a contact inbox no admission has given yet (an
/// actor start after a restart, a child result waking it): read-only, and nothing but the
/// reply. The contact's next admission installs its real cap and allowlist.
const INBOX_FAIL_CLOSED: tools::Risk = tools::Risk::ReadOnly;

/// 183 Z3-R2: tasks one contact may have waiting on one export (queued behind a running
/// one, or held for the owner's first-contact answer).
const CONTACT_QUEUED: usize = 8;

impl Agents {
    /// 183 §7.6, Z3-R2: a contact's inbox is default-deny: `cap` and the allowlist `tools`
    /// (the only tools that exist in it and its subagents), asked so both hold before the
    /// next tool call. Re-installed on every admission and at every actor start. Z3-R3: an
    /// error is the caller's to refuse on (no registry in the process: no tool runs at all).
    pub(crate) async fn install_contact_limits(
        &mut self,
        id: SessionId,
        cap: tools::Risk,
        tools: Vec<String>,
    ) -> Result<(), String> {
        self.caps.insert(id, cap);
        self.network.inbox_limits.insert(id, (cap, tools.clone()));
        self.network.inboxes.entry(id).or_insert(false);
        let Some(reg) =
            kameo::actor::ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
                .map_err(|e| format!("a2a: inbox limits: tool registry: {e}"))?
        else {
            return Ok(());
        };
        reg.ask(tools::SetAllowOnly {
            session: id,
            tools: Some(tools),
            turn: false,
        })
        .await
        .map_err(|e| format!("a2a: inbox allowlist: {e}"))?;
        reg.ask(tools::SetCap {
            session: id,
            cap: Some(cap),
        })
        .await
        .map_err(|e| format!("a2a: inbox cap: {e}"))
    }

    /// 183 §7.6, Z3-R2: at every actor start of a session, before anything of it runs: a
    /// contact's inbox gets its limits back (the last admission's, else fail-closed).
    /// A store error is treated as an inbox (fail closed); limits not installed refuse the
    /// start.
    pub(crate) async fn reinstall_contact_limits(&mut self, id: SessionId) -> Result<(), String> {
        match self.store.a2a_inbox_identity(id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(()),
            Err(e) => tracing::error!(session = %id, "a2a: inbox lookup failed, capping it: {e:#}"),
        }
        let (cap, tools) = self
            .network
            .inbox_limits
            .get(&id)
            .cloned()
            .unwrap_or((INBOX_FAIL_CLOSED, Vec::new()));
        self.install_contact_limits(id, cap, tools).await
    }
}

/// `ask` → `()` (183 §7.6, Z3-R2): contact `contact`'s cap or an export's allowlist changed
/// (`peers.yaml` reloaded): its live inboxes take the new limits at once, a running turn
/// included. `exports` = (agent export, its allowlist) for every agent export granted.
#[derive(Clone, Debug)]
pub struct ContactLimits {
    /// The contact's alias.
    pub contact: String,
    /// Its cap.
    pub cap: tools::Risk,
    /// Agent export → its allowlist.
    pub exports: Vec<(String, Vec<String>)>,
}

impl Message<ContactLimits> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ContactLimits, _: &mut Context<Self, Self::Reply>) {
        self.apply_contact_limits(m).await;
    }
}

impl Agents {
    /// [`ContactLimits`].
    pub(crate) async fn apply_contact_limits(&mut self, m: ContactLimits) {
        let Ok(Some(binding)) = self.store.peer_binding(&m.contact).await else {
            return;
        };
        let key = format!("peer:{}", binding.id);
        for (export, tools) in m.exports {
            let scope = A2aScope {
                principal: &key,
                export: &export,
            };
            let Ok(Some(inbox)) = self.store.a2a_inbox(scope).await else {
                continue;
            };
            let cap = self
                .network
                .inbox_persona(inbox)
                .and_then(|p| p.tools)
                .map_or(m.cap, |t| t.min(m.cap));
            if let Err(e) = self.install_contact_limits(inbox, cap, tools).await {
                tracing::error!(session = %inbox, "{e}");
            }
        }
    }
}

impl Agents {
    /// 183 §11.5/§11.6: how `record` reaches its session when a contact sent it: as the
    /// `<contact_message>` turn of an exported chat (owned here for this engine's own A2A
    /// task, or for another engine's, W-40), or as an inbox's peer message.
    pub(crate) fn network_contact_frame(
        &mut self,
        record: &PeerRecord,
    ) -> Option<crate::loop_::ContactFrame> {
        if let Some(env) = self.network.contact_replies.get(&record.receipt.id) {
            return Some(crate::loop_::ContactFrame::Chat {
                origin: env.origin.clone(),
                scope: Some(record.receipt.id),
            });
        }
        let origin = self
            .network
            .contact_scopes
            .get(&record.request_scope?)?
            .clone();
        Some(match self.network.inboxes.get_mut(&record.session) {
            Some(first) => crate::loop_::ContactFrame::Inbox {
                origin,
                first: std::mem::take(first),
            },
            None => crate::loop_::ContactFrame::Chat {
                origin,
                scope: None,
            },
        })
    }
}

/// `tell` (internal, 183 W-27, F5): a held task's wait ended without the owner's answer:
/// its grant epoch ended (`expired: false`) or [`HOLD_TTL`] passed (`expired: true`).
struct HeldEnded {
    scope: Uuid,
    expired: bool,
}

/// 183 W-27, F5: the watch of one held task: [`HeldEnded`] when `route` (its grant epoch)
/// ends or `ttl` passes, whichever comes first.
fn held_watch(scope: Uuid, route: CancellationToken, ttl: Duration) -> AbortHandle {
    tokio::spawn(async move {
        let expired = tokio::select! {
            () = route.cancelled() => false,
            () = tokio::time::sleep(ttl) => true,
        };
        crate::continuation::notify(HeldEnded { scope, expired }).await;
    })
    .abort_handle()
}

/// `ask` → `()` (183 F4): `contact`'s grant changed and its grant epoch is about to end
/// (`route` is the new one). Its held tasks for an export it still grants (`exports`) wait
/// on under `route`; the others fail when the old epoch ends. Asked before the old epoch is
/// cancelled, so editing a contact (or sharing another export with it) never drops a
/// waiting first message.
#[derive(Clone, Debug)]
pub struct RebindHeld {
    /// The contact's alias.
    pub contact: String,
    /// The exports it is granted now.
    pub exports: Vec<String>,
    /// Its new grant epoch.
    pub route: CancellationToken,
}

impl Message<RebindHeld> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: RebindHeld, _: &mut Context<Self, Self::Reply>) {
        self.rebind_held(m);
    }
}

impl Agents {
    /// W-27: park a contact's first task for (contact, export) in `AUTH_REQUIRED` until the
    /// owner answers; the contact reads "waiting: … has not allowed messages to … yet". F11:
    /// it waits aside (the chat keeps working); F5: it fails after [`HOLD_TTL`].
    async fn hold_task(
        &mut self,
        task: A2aTask,
        origin: &super::ContactOrigin,
        route: CancellationToken,
    ) -> A2aTask {
        let why = format!(
            "waiting: {}",
            super::ContactRefusal::NotAllowed {
                contact: mesh::self_name(),
                export: origin.export.clone(),
            }
        );
        let preview = snapshot(&task)
            .history
            .as_deref()
            .and_then(|h| h.first())
            .and_then(|m| parts_text(&m.parts))
            .map(|t| cut(t.lines().next().unwrap_or_default().trim(), 200).to_owned())
            .unwrap_or_default();
        let held = match self
            .transition(&task, A2aTaskStatus::AuthRequired, Some(&why), None, None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(task = %task.id, "a2a: hold for first contact: {e:#}");
                let why = format!("could not wait for the owner's answer: {e}");
                return self.fail_task(&task, &why).await.unwrap_or(task);
            }
        };
        let watch = held_watch(held.request_scope, route.clone(), HOLD_TTL);
        tracing::info!(task = %held.id, contact = %origin.contact, export = %origin.export, "a2a: first contact waits for the owner");
        self.network
            .held
            .entry((origin.contact.clone(), origin.export.clone()))
            .or_default()
            .push(HeldTask {
                task: held.clone(),
                route,
                watch,
                since_ms: Utc::now().timestamp_millis().max(0) as u64,
                preview,
            });
        self.rt.publish(ContactsHeldChanged).await;
        held
    }

    /// [`RebindHeld`].
    fn rebind_held(&mut self, m: RebindHeld) {
        let now = Utc::now().timestamp_millis().max(0) as u64;
        for ((c, e), tasks) in self.network.held.iter_mut() {
            if *c != m.contact || !m.exports.contains(e) {
                continue;
            }
            for h in tasks.iter_mut() {
                h.watch.abort();
                let waited = Duration::from_millis(now.saturating_sub(h.since_ms));
                h.watch = held_watch(
                    h.task.request_scope,
                    m.route.clone(),
                    HOLD_TTL.saturating_sub(waited),
                );
                h.route = m.route.clone();
            }
        }
    }

    /// [`ContactRelease`].
    async fn release_held(&mut self, m: ContactRelease) -> usize {
        let keys: Vec<(String, String)> = self
            .network
            .held
            .keys()
            .filter(|(c, e)| *c == m.contact && m.export.as_ref().is_none_or(|x| x == e))
            .cloned()
            .collect();
        let mut n = 0;
        for key in keys {
            for h in self.network.held.remove(&key).unwrap_or_default() {
                h.watch.abort();
                n += 1;
                if m.allow {
                    self.start_held(h).await;
                } else {
                    let why = super::ContactRefusal::NotAContact {
                        contact: m.contact.clone(),
                    }
                    .to_string();
                    if let Err(e) = self.fail_task(&h.task, &why).await {
                        tracing::warn!(task = %h.task.id, "a2a: fail held task: {e:#}");
                    }
                }
            }
        }
        if n > 0 {
            self.rt.publish(ContactsHeldChanged).await;
        }
        n
    }

    /// A released task starts as a fresh admission would: under today's policy and the
    /// grant it was accepted with.
    async fn start_held(&mut self, h: HeldTask) {
        let binding = h
            .task
            .principal
            .strip_prefix("peer:")
            .and_then(|b| b.parse::<Uuid>().ok());
        let p = binding.map(|binding| A2aPrincipal {
            binding,
            export: h.task.export.clone(),
            session: h.task.session,
            route: h.route.clone(),
            contact: self
                .network
                .contact_scopes
                .get(&h.task.request_scope)
                .cloned(),
            ask_first: false,
        });
        let admitted = match &p {
            Some(p) => self.admit_principal(p).await.map_err(|e| e.message),
            None => Err("malformed principal".into()),
        };
        match admitted {
            Ok(generation) => {
                // Back to `submitted`: it now waits only for the session's scope, if at all.
                let task = match self
                    .transition(&h.task, A2aTaskStatus::Submitted, None, None, None)
                    .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(task = %h.task.id, "a2a: release held task: {e:#}");
                        h.task
                    }
                };
                self.start_or_queue(task, generation, h.route).await;
            }
            Err(why) => {
                if let Err(e) = self.fail_task(&h.task, &why).await {
                    tracing::warn!(task = %h.task.id, "a2a: fail held task: {e:#}");
                }
            }
        }
    }
}

impl Message<ContactRelease> for Agents {
    type Reply = Result<usize, String>;

    async fn handle(
        &mut self,
        m: ContactRelease,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.release_held(m).await)
    }
}

impl Message<ContactOffers> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ContactOffers, _: &mut Context<Self, Self::Reply>) {
        self.network.set_offers(m.0);
    }
}

impl Message<ContactsHeld> for Agents {
    type Reply = Vec<HeldContact>;

    async fn handle(&mut self, _: ContactsHeld, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut out: Vec<HeldContact> = self
            .network
            .held
            .iter()
            .filter_map(|((contact, export), tasks)| {
                let first = tasks.first()?;
                Some(HeldContact {
                    contact: contact.clone(),
                    export: export.clone(),
                    session: first.task.session,
                    since_ms: first.since_ms,
                    waiting: tasks.len(),
                    preview: first.preview.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.contact, &a.export).cmp(&(&b.contact, &b.export)));
        out
    }
}

impl Agents {
    /// [`HeldEnded`]: the held task fails (`HOLD_EXPIRED` after [`HOLD_TTL`], else the
    /// revoked route); it never held the session's slot, so nothing else moves.
    async fn held_ended(&mut self, scope: Uuid, expired: bool) {
        let mut gone = None;
        for tasks in self.network.held.values_mut() {
            if let Some(i) = tasks.iter().position(|h| h.task.request_scope == scope) {
                gone = Some(tasks.remove(i));
                break;
            }
        }
        self.network.held.retain(|_, v| !v.is_empty());
        let Some(h) = gone else { return };
        h.watch.abort();
        let why = if expired {
            tracing::info!(task = %h.task.id, "a2a: first contact expired unanswered");
            HOLD_EXPIRED
        } else {
            "peer route revoked"
        };
        if let Err(e) = self.fail_task(&h.task, why).await {
            tracing::warn!(task = %h.task.id, "a2a: fail held task: {e:#}");
        }
        self.rt.publish(ContactsHeldChanged).await;
    }
}

impl Message<HeldEnded> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: HeldEnded, _: &mut Context<Self, Self::Reply>) {
        self.held_ended(m.scope, m.expired).await;
    }
}

/// 183 W-40: how [`super::ContactDeliver`] refuses a message for a chat this engine no
/// longer owns; the RPC answers it `-32013 NotOwner` (the holder retries once).
pub const CONTACT_NOT_OWNER: &str = "contact_deliver: not the owner of";

/// `tell` (internal, 183 W-40): a forwarded contact message came back from its owner.
struct ContactForwarded {
    scope: Uuid,
    owner: String,
    envelope: super::ContactEnvelope,
    retry: bool,
    outcome: Result<PeerReceipt, super::mesh::ContactFailure>,
}

impl Message<super::ContactDeliver> for Agents {
    type Reply = Result<PeerReceipt, String>;

    async fn handle(
        &mut self,
        m: super::ContactDeliver,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.contact_deliver(m.envelope, m.forwarder).await
    }
}

impl Message<ContactForwarded> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ContactForwarded, _: &mut Context<Self, Self::Reply>) {
        let why = match m.outcome {
            Ok(_) => return,
            // §13 row 53: the chat moved mid-message: once more, on its new owner.
            Err(super::mesh::ContactFailure::NotOwner) if m.retry => {
                match self.remote_owner(m.envelope.to).await {
                    Some(owner) if owner != m.owner => {
                        self.forward_contact(m.scope, owner, m.envelope, false)
                            .await;
                        return;
                    }
                    _ => "the chat moved; its new owner is not known here".to_owned(),
                }
            }
            Err(super::mesh::ContactFailure::NotOwner) => {
                "the chat moved; its new owner is not known here".to_owned()
            }
            // Z3-R2: what a contact reads never names a machine of this mesh; the log does.
            Err(super::mesh::ContactFailure::Offline) => {
                let owner = engine_name(&m.owner).await;
                tracing::info!(%owner, "a2a: exported chat's owner offline");
                super::ContactRefusal::OwnerOffline.to_string()
            }
            Err(super::mesh::ContactFailure::Old) => {
                let owner = engine_name(&m.owner).await;
                tracing::info!(%owner, "a2a: exported chat's owner needs an update");
                super::ContactRefusal::OwnerOld.to_string()
            }
            Err(super::mesh::ContactFailure::Refused(why)) => {
                tracing::info!(%why, "a2a: exported chat's owner refused a contact message");
                "the chat is not taking messages right now".to_owned()
            }
        };
        self.network.contact_waits.remove(&m.envelope.id);
        self.abort_scope(m.scope, &why).await;
    }
}

/// A machine as a contact refusal names it: its mesh name, else a short id.
async fn engine_name(id: &str) -> String {
    super::mesh::engine_name(&super::mesh::engine_names().await.1, id)
}

impl Agents {
    /// W-40 on the holder: send `envelope` to `owner` off the mailbox; the answer arrives
    /// as a `contact_deliver` with `reply_to` (or the failure as [`ContactForwarded`]).
    async fn forward_contact(
        &mut self,
        scope: Uuid,
        owner: String,
        envelope: super::ContactEnvelope,
        retry: bool,
    ) {
        self.network.contact_waits.insert(envelope.id, scope);
        let transport = self.transport().await;
        tokio::spawn(async move {
            let outcome = match &transport {
                Some(t) => {
                    super::mesh::contact_forward(
                        t.as_ref(),
                        &replica::PeerId(owner.clone()),
                        &envelope,
                    )
                    .await
                }
                None => Err(super::mesh::ContactFailure::Offline),
            };
            crate::continuation::notify(ContactForwarded {
                scope,
                owner,
                envelope,
                retry,
                outcome,
            })
            .await;
        });
    }

    /// `contact_deliver` (W-40): on the chat's owner, one contact message queued for its
    /// own capped turn (W-39); on the holder, the owner's answer to one it forwarded, which
    /// completes the contact's A2A task. `forwarder`: the engine that sent the call.
    pub(crate) async fn contact_deliver(
        &mut self,
        env: super::ContactEnvelope,
        forwarder: Option<String>,
    ) -> Result<PeerReceipt, String> {
        let row = self
            .store
            .session(env.to)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "export unavailable: export session missing".to_owned())?;
        // The holder side: the owner's answer to a message this engine forwarded.
        if let Some(reply_to) = env.reply_to {
            let owner = if row.here {
                mesh::self_id().unwrap_or_default()
            } else {
                row.owner.clone()
            };
            if forwarder.as_deref() != Some(owner.as_str()) {
                return Err("contact_deliver: only the chat's owner answers for it".into());
            }
            let Some(scope) = self.network.contact_waits.remove(&reply_to) else {
                return Err("contact_deliver: no contact message waits for this answer".into());
            };
            let text = self.redact(&env.text).into_owned();
            if let Some(sc) = self.network.scopes.get_mut(&scope) {
                sc.result = Some(Ok(text));
                let session = sc.session;
                self.try_settle(session).await;
            }
            return Ok(PeerReceipt {
                id: env.id,
                to: format!("session:{}", env.to),
                state: PeerMessageState::Consumed,
                reply_to: env.reply_to,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            });
        }
        // The owner side.
        if !row.here {
            return Err(format!("{CONTACT_NOT_OWNER} session:{}", env.to));
        }
        if let Some(fwd) = forwarder.as_deref() {
            // The family forwarder vouches for its contact: it must be the holder.
            if env.origin.holder != fwd {
                return Err("contact_deliver: the forwarder does not hold this contact".into());
            }
            if mesh::messaging_off(fwd) {
                let me = mesh::self_name();
                return Err(format!(
                    "send_message: {}: {}",
                    tools::communication::DISABLED,
                    super::MeshRefusal::Messaging {
                        from: engine_name(fwd).await,
                        to: me.clone(),
                        on: me,
                    }
                ));
            }
        }
        match self.chat_paused(env.to).await? {
            None => {}
            Some(reason) if reason == PAUSED_MESSAGING => {
                return Err(super::ContactRefusal::NoAgentMessages.to_string());
            }
            Some(reason) => return Err(super::ContactRefusal::Paused { reason }.to_string()),
        }
        let generation = self.communication.0.write().contact(env.to).map_err(|_| {
            format!(
                "send_message: {}: {}",
                tools::communication::DISABLED,
                super::MeshRefusal::Off {
                    engine: mesh::self_name()
                }
            )
        })?;
        let body = self.redact(&env.text).into_owned();
        let sender = format!("contact:{}:{}", env.origin.holder, env.origin.contact);
        let recipient = format!("session:{}", env.to);
        let accepted = self
            .store
            .accept_exchange_as(
                env.id,
                &AcceptExchange {
                    sender: &sender,
                    recipient: &recipient,
                    sender_session: None,
                    recipient_session: Some(env.to),
                    request_id: env.request_id.as_deref(),
                    reply_to: None,
                    request_scope: None,
                    generation: generation.revision() as i64,
                    body: &body,
                },
            )
            .await
            .map_err(|e| format!("{e:#}"))?;
        if accepted.state != PeerMessageState::Accepted
            || !self.network.enqueued.insert(accepted.id)
        {
            return Ok(accepted);
        }
        // The owner may lower the cap, never raise it above `mutating`.
        let mut env = env;
        env.origin.cap = env.origin.cap.min(tools::Risk::Mutating);
        self.network.contact_replies.insert(env.id, env.clone());
        let record = self
            .store
            .peer_record(env.to, accepted.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "accepted contact message missing".to_owned())?;
        if let Err(why) = self.enqueue_peer(record, generation, true, false).await {
            self.network.contact_replies.remove(&env.id);
            if let Err(e) = self.store.fail_exchange(accepted.id, &why).await {
                tracing::warn!(id = %accepted.id, "contact_deliver: fail exchange: {e:#}");
            }
            return Err(why);
        }
        tracing::info!(session = %env.to, contact = %env.origin.contact, holder = %env.origin.holder, "contact message queued for its own turn");
        Ok(accepted)
    }

    /// W-40 on the owner: the turn a forwarded contact message opened ended; its answer
    /// (only that turn's) goes back to the holder.
    async fn contact_turn_finished(&mut self, env: super::ContactEnvelope, m: &PeerTurnFinished) {
        let text = match m.stop_reason.as_str() {
            "cancelled" | "timed_out" | "interrupted" => {
                format!("the turn in this chat was stopped ({})", m.stop_reason)
            }
            "error" => "the turn in this chat stopped with an error".to_owned(),
            _ => self.redact(&m.text).into_owned(),
        };
        let reply = super::ContactEnvelope {
            id: Uuid::now_v7(),
            to: env.to,
            text,
            origin: env.origin.clone(),
            reply_to: Some(env.id),
            request_id: None,
        };
        let holder = env.origin.holder.clone();
        if mesh::self_id().as_deref() == Some(holder.as_str()) {
            if let Err(e) = self.contact_deliver(reply, Some(holder)).await {
                tracing::warn!(session = %env.to, "contact answer: {e}");
            }
            return;
        }
        let transport = self.transport().await;
        tokio::spawn(async move {
            let outcome = match &transport {
                Some(t) => {
                    super::mesh::contact_forward(t.as_ref(), &replica::PeerId(holder), &reply).await
                }
                None => Err(super::mesh::ContactFailure::Offline),
            };
            if let Err(e) = outcome {
                tracing::warn!(session = %reply.to, "contact answer not delivered: {e:?}");
            }
        });
    }
}

impl Message<PeerPolicy> for Agents {
    type Reply = SharedPolicy;

    async fn handle(&mut self, _: PeerPolicy, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.communication.clone()
    }
}

impl Message<ScopeDeadline> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ScopeDeadline, _: &mut Context<Self, Self::Reply>) {
        self.abort_scope(m.0, "request deadline exceeded").await;
    }
}

impl Message<ScopeRevoked> for Agents {
    type Reply = ();

    /// The grant epoch the task was admitted under ended (credentials,
    /// exports or roots changed; binding gone): the task fails, its work is
    /// asked to stop, and its streams end at the same token.
    async fn handle(&mut self, m: ScopeRevoked, _: &mut Context<Self, Self::Reply>) {
        self.abort_scope(m.0, "peer route revoked").await;
    }
}

// ---------------------------------------------------------------------------
// Outbound (send tool → registered route)
// ---------------------------------------------------------------------------

impl Agents {
    /// 183 W-26: the policy half of an outbound remote send: a contact needs
    /// `agent.communication`, an HTTP peer also `a2a.enabled`.
    pub(crate) fn remote_generation(
        &self,
        from: SessionId,
        contact: bool,
    ) -> Result<Generation, super::PolicyError> {
        let mut p = self.communication.0.write();
        if contact {
            p.contact(from)
        } else {
            p.remote(from)
        }
    }

    /// `send_message` to `peer:<alias>` (§5.2, §6.3). Three shapes by
    /// `reply_to`: a question on an exported task the caller serves (travels
    /// on that task's channel, → `INPUT_REQUIRED`; needs only the live
    /// binding that opened the task, so an inbound-only peer can be asked),
    /// an answer to a remote question (sent on the remote task), or a fresh
    /// message. The two outbound shapes need a route: a registered sender
    /// whose grants include the caller's root; policy `remote` for all. The
    /// receipt is `accepted` once persisted; outcomes arrive later.
    pub(crate) async fn network_send(
        &mut self,
        from: SessionId,
        alias: &str,
        body: &str,
        request_id: Option<&str>,
        reply_to: Option<Uuid>,
        await_reply: bool,
    ) -> Result<PeerReceipt, String> {
        let (binding, sender, route, contact) = {
            let (binding, peer) = self
                .network
                .peer_by_name(alias)
                .ok_or_else(|| format!("unknown peer `{alias}`"))?;
            (
                binding,
                peer.sender.clone(),
                peer.generation.clone(),
                peer.contact,
            )
        };
        let generation = self
            .remote_generation(from, contact)
            .map_err(|e| e.to_string())?;
        let body = self.redact(body).into_owned();
        let sender_key = format!("session:{from}");
        let recipient_key = format!("peer:{binding}");

        // A question on an exported task this session is serving.
        let mut remote_task: Option<String> = None;
        let mut question_scope: Option<Uuid> = None;
        if let Some(r) = reply_to {
            let orig = self
                .store
                .exchange_record(r)
                .await
                .map_err(|e| format!("{e:#}"))?
                .ok_or_else(|| "unknown message".to_owned())?;
            if orig.from != recipient_key || orig.receipt.to != sender_key {
                return Err("reply_to does not name a message this peer sent you".into());
            }
            match orig.request_scope.filter(|s| {
                self.network
                    .scopes
                    .get(s)
                    .is_some_and(|sc| sc.session == from)
            }) {
                Some(scope) => {
                    if self.network.scopes[&scope].question.is_some() {
                        return Err(
                            "a question to this peer is already pending on this task".into()
                        );
                    }
                    question_scope = Some(scope);
                }
                None => remote_task = orig.receipt.task_id.clone(),
            }
        }
        let sender = match question_scope {
            // Not an outbound send: the task's own channel carries it.
            Some(_) => None,
            None => {
                let sender = sender.ok_or_else(|| {
                    format!("peer `{alias}` is inbound-only; it accepts no messages")
                })?;
                let root = self
                    .communication
                    .0
                    .read()
                    .root_of(from)
                    .map_err(|e| e.to_string())?;
                // Z3-R2: a contact's inbox reaches no contact (no relay through it).
                let granted = (contact && !self.network.inboxes.contains_key(&root))
                    || self
                        .network
                        .peers
                        .get(&binding)
                        .is_some_and(|p| p.roots.contains(&root));
                if !granted {
                    return Err(format!(
                        "peer `{alias}` is not granted to this session's root"
                    ));
                }
                Some(sender)
            }
        };
        let a = AcceptExchange {
            sender: &sender_key,
            recipient: &recipient_key,
            sender_session: Some(from),
            recipient_session: None,
            request_id,
            reply_to,
            request_scope: question_scope,
            generation: generation.revision() as i64,
            body: &body,
        };
        let mut receipt =
            self.store
                .accept_exchange(&a)
                .await
                .map_err(|e| match comm_error(&e) {
                    Some(CommError::NotFound) => {
                        "reply_to does not name a live message from this peer".to_owned()
                    }
                    Some(CommError::Terminal) => "that message was already answered".to_owned(),
                    Some(CommError::Conflict) => {
                        "request_id was already used with different content".to_owned()
                    }
                    Some(CommError::TooLarge) => format!("message exceeds {PEER_TEXT_MAX} bytes"),
                    _ => format!("{e:#}"),
                })?;
        // 183 F8: the receipt names the peer as the sender wrote it, not by its binding id.
        receipt.to = format!("peer:{alias}");
        if receipt.state != PeerMessageState::Accepted {
            // Idempotent retry of a settled send: the existing outcome.
            return Ok(receipt);
        }

        if let Some(scope) = question_scope {
            // Not dispatched over HTTP: the exported task carries it. Awaited
            // → `INPUT_REQUIRED`; otherwise a progress status that finishes
            // nothing (§6.3).
            let Some(task) = self.scope_task(scope).await else {
                return Err("the exported task is gone".into());
            };
            let status = if await_reply {
                A2aTaskStatus::InputRequired
            } else {
                A2aTaskStatus::Working
            };
            self.transition(&task, status, Some(&body), None, None)
                .await
                .map_err(|e| format!("{e:#}"))?;
            if let Some(sc) = self.network.scopes.get_mut(&scope).filter(|_| await_reply) {
                sc.question = Some(receipt.id);
            }
            return Ok(receipt);
        }

        let sender = sender.expect("outbound shapes resolved a sender above");
        let record = self
            .store
            .exchange_record(receipt.id)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| "accepted exchange missing".to_owned())?;
        if !generation.live() || route.is_cancelled() {
            let mut r = self
                .store
                .fail_exchange(receipt.id, "communication_disabled")
                .await
                .map_err(|e| format!("{e:#}"))?;
            r.to = receipt.to;
            return Ok(r);
        }
        self.network.outbound.insert(
            receipt.id,
            Outbound {
                binding,
                session: from,
                generation: generation.clone(),
                route,
                task_id: remote_task.clone(),
                alias: alias.to_owned(),
                contact,
            },
        );
        let outbound = OutboundPeerMessage {
            binding,
            request_id: request_id.map_or_else(|| receipt.id.to_string(), str::to_owned),
            record,
            task_id: remote_task,
            context_id: None,
            generation,
            // `peer_by_name` matched only a contact for `<name>/<export>`.
            export: alias
                .split_once('/')
                .filter(|_| contact)
                .map(|(_, e)| e.to_owned()),
        };
        let exchange = receipt.id;
        tokio::spawn(async move {
            let outcome = sender.send(outbound).await;
            crate::continuation::notify(OutboundSettled { exchange, outcome }).await;
        });
        Ok(receipt)
    }

    /// The remote's response (or its absence) for an outbound exchange.
    async fn remote_outcome(
        &mut self,
        exchange: Uuid,
        outcome: Result<RemoteReply, RemoteSendError>,
    ) {
        let Some(o) = self.network.outbound.get(&exchange) else {
            return;
        };
        if (!o.generation.live() || o.route.is_cancelled())
            && !matches!(outcome, Err(RemoteSendError::Refused(_)))
        {
            // Stale admission: the route or its grant epoch was revoked while
            // the send was out; whatever the peer answered is not routed
            // under it.
            self.network.outbound.remove(&exchange);
            if let Ok(r) = self
                .store
                .mark_exchange_unknown(exchange, None, "communication_disabled while in flight")
                .await
            {
                self.peer_changed(&r).await;
            }
            return;
        }
        let r = match outcome {
            Err(RemoteSendError::Refused(why)) => {
                self.network.outbound.remove(&exchange);
                let why = self.redact(&why).into_owned();
                self.store.fail_exchange(exchange, &why).await
            }
            Err(RemoteSendError::Unknown(why)) => {
                self.network.outbound.remove(&exchange);
                let why = self.redact(&why).into_owned();
                self.store.mark_exchange_unknown(exchange, None, &why).await
            }
            Ok(SendMessageResponse::Message(m)) => {
                let text = self
                    .redact(&parts_text(&m.parts).unwrap_or_default())
                    .into_owned();
                self.import_reply(exchange, &text).await;
                return;
            }
            Ok(SendMessageResponse::Task(t)) => {
                match self.store.bind_exchange_task(exchange, &t.id).await {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => tracing::warn!(exchange = %exchange, "a2a: bind remote task: {e:#}"),
                }
                if let Some(o) = self.network.outbound.get_mut(&exchange) {
                    o.task_id = Some(t.id.clone());
                }
                self.apply_remote_task(exchange, &t).await;
                return;
            }
        };
        match r {
            Ok(receipt) => self.peer_changed(&receipt).await,
            Err(e) => tracing::warn!(exchange = %exchange, "a2a: outbound outcome: {e:#}"),
        }
    }

    /// Project a remote task snapshot onto the outbound exchange (§6.3).
    async fn apply_remote_task(&mut self, exchange: Uuid, t: &Task) {
        let Some(o) = self.network.outbound.get(&exchange) else {
            return;
        };
        if o.task_id.as_deref().is_some_and(|id| id != t.id) {
            tracing::warn!(exchange = %exchange, "a2a: update names another task; ignored");
            return;
        }
        let (alias, contact) = (o.alias.clone(), o.contact);
        let text = self.redact(&task_text(t)).into_owned();
        match t.status.state {
            TaskState::Completed => self.import_reply(exchange, &text).await,
            TaskState::Failed | TaskState::Canceled | TaskState::Rejected => {
                self.network.outbound.remove(&exchange);
                let why = format!("remote task {:?}: {}", t.status.state, cut(&text, 512));
                match self.store.fail_exchange(exchange, &why).await {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => tracing::warn!(exchange = %exchange, "a2a: remote failure: {e:#}"),
                }
            }
            TaskState::InputRequired => {
                let key = t.status.message.as_ref().map_or_else(
                    || format!("q:{exchange}:input"),
                    |m| format!("q:{}", m.message_id),
                );
                self.import_question(exchange, &t.id, &key, &text, true)
                    .await;
            }
            TaskState::AuthRequired => {
                // 183 F7: a contact's first-contact hold is a state of this message, not a
                // question to answer: `held`, with the line the sender's tools show.
                if contact && let Some((machine, export)) = super::ContactRefusal::waiting(&text) {
                    let name = alias.split('/').next().unwrap_or_default();
                    let to = format!("peer:{alias}");
                    let why = held_line(name, &machine, &export);
                    match self.store.hold_exchange(exchange, &why).await {
                        Ok(mut r) => {
                            self.peer_changed(&r).await;
                            r.to = to;
                            // An awaiting `send_message` returns now, saying so.
                            self.settle_wait(&r);
                        }
                        Err(e) => tracing::warn!(exchange = %exchange, "a2a: hold exchange: {e:#}"),
                    }
                    return;
                }
                let key = t.status.message.as_ref().map_or_else(
                    || format!("q:{exchange}:auth"),
                    |m| format!("q:{}", m.message_id),
                );
                let text = format!(
                    "The remote agent requires the owner to authenticate before continuing; \
                     no automatic login or approval is performed. {text}"
                );
                self.import_question(exchange, &t.id, &key, &text, false)
                    .await;
            }
            TaskState::Submitted | TaskState::Working => {
                // 183 F7: the owner allowed it; it is being delivered now.
                match self.store.unhold_exchange(exchange).await {
                    Ok(Some(r)) => self.peer_changed(&r).await,
                    Ok(None) => {}
                    Err(e) => tracing::warn!(exchange = %exchange, "a2a: unhold exchange: {e:#}"),
                }
            }
            TaskState::Unspecified => {}
        }
    }

    /// The remote's final answer to `exchange`: persisted as a reply, then
    /// handed to the awaiting `send_message` call or delivered once as input.
    async fn import_reply(&mut self, exchange: Uuid, text: &str) {
        let Some(o) = self.network.outbound.remove(&exchange) else {
            // Already settled (a Completed snapshot after the first).
            return;
        };
        let (peer, session_key) = (
            format!("peer:{}", o.binding),
            format!("session:{}", o.session),
        );
        let request_id = format!("reply:{exchange}");
        let a = AcceptExchange {
            sender: &peer,
            recipient: &session_key,
            sender_session: None,
            recipient_session: Some(o.session),
            request_id: Some(&request_id),
            reply_to: Some(exchange),
            request_scope: None,
            generation: o.generation.revision() as i64,
            body: cut(text, PEER_TEXT_MAX),
        };
        let reply = match self.store.accept_exchange(&a).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(exchange = %exchange, "a2a: import reply: {e:#}");
                return;
            }
        };
        let Ok(Some(mut orig)) = self.store.exchange_receipt(exchange).await else {
            return;
        };
        self.peer_changed(&orig).await;
        if reply.state != PeerMessageState::Accepted {
            return;
        }
        // 183 F8: the awaiting tool and the reply show the address the sender wrote.
        orig.to = format!("peer:{}", o.alias);
        if self.settle_wait(&orig) {
            if let Err(e) = self
                .store
                .consume_exchange_direct(reply.id, o.session, o.generation.revision() as i64)
                .await
            {
                tracing::warn!(exchange = %reply.id, "a2a: direct consume: {e:#}");
            }
            return;
        }
        if !o.generation.live() {
            let _ = self
                .store
                .suppress_exchange(reply.id, "communication_disabled")
                .await;
            return;
        }
        match self.store.exchange_record(reply.id).await {
            Ok(Some(mut record)) => {
                record.from = format!("peer:{}", o.alias);
                if let Err(e) = self.enqueue_peer(record, o.generation, false, false).await {
                    let _ = self.store.fail_exchange(reply.id, &e).await;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(exchange = %reply.id, "a2a: reply record: {e:#}"),
        }
    }

    /// A remote `INPUT_REQUIRED` / `AUTH_REQUIRED`: a durable question to the
    /// initiating session, bound to the remote task so `reply_to` answers on
    /// it. Idempotent by the remote status message id.
    async fn import_question(
        &mut self,
        exchange: Uuid,
        remote_task: &str,
        request_id: &str,
        text: &str,
        reply_expected: bool,
    ) {
        let Some(o) = self.network.outbound.get(&exchange) else {
            return;
        };
        let (binding, session, generation) = (o.binding, o.session, o.generation.clone());
        let shown = format!("peer:{}", o.alias);
        let (peer, session_key) = (format!("peer:{binding}"), format!("session:{session}"));
        let a = AcceptExchange {
            sender: &peer,
            recipient: &session_key,
            sender_session: None,
            recipient_session: Some(session),
            request_id: Some(request_id),
            reply_to: None,
            request_scope: None,
            generation: generation.revision() as i64,
            body: cut(text, PEER_TEXT_MAX),
        };
        let q = match self.store.accept_exchange(&a).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(exchange = %exchange, "a2a: import question: {e:#}");
                return;
            }
        };
        if !self.network.enqueued.insert(q.id) {
            return;
        }
        if let Err(e) = self.store.bind_exchange_task(q.id, remote_task).await {
            tracing::warn!(exchange = %q.id, "a2a: bind question task: {e:#}");
        }
        if !generation.live() {
            let _ = self
                .store
                .suppress_exchange(q.id, "communication_disabled")
                .await;
            return;
        }
        match self.store.exchange_record(q.id).await {
            Ok(Some(mut record)) => {
                // 183 F8: the session reads the address it wrote, not the binding id.
                record.from = shown;
                if let Err(e) = self
                    .enqueue_peer(record, generation, false, reply_expected)
                    .await
                {
                    let _ = self.store.fail_exchange(q.id, &e).await;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(exchange = %q.id, "a2a: question record: {e:#}"),
        }
    }
}

impl Message<OutboundSettled> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: OutboundSettled, _: &mut Context<Self, Self::Reply>) {
        self.remote_outcome(m.exchange, m.outcome).await;
    }
}

impl Message<RemoteTaskUpdate> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoteTaskUpdate,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self.network.outbound.get(&m.exchange) {
            Some(o) if o.binding == m.binding => {}
            _ => return Err("unknown outbound exchange for this binding".into()),
        }
        match m.update {
            RemoteUpdate::Task(t) => self.apply_remote_task(m.exchange, &t).await,
            RemoteUpdate::Message(msg) => {
                let text = self
                    .redact(&parts_text(&msg.parts).unwrap_or_default())
                    .into_owned();
                self.import_reply(m.exchange, &text).await;
            }
            RemoteUpdate::Failed(why) => {
                self.network.outbound.remove(&m.exchange);
                let why = self.redact(&why).into_owned();
                match self.store.fail_exchange(m.exchange, &why).await {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => return Err(format!("{e:#}")),
                }
            }
            RemoteUpdate::Unknown(why) => {
                let o = self.network.outbound.remove(&m.exchange);
                let why = self.redact(&why).into_owned();
                let task_id = o.and_then(|o| o.task_id);
                match self
                    .store
                    .mark_exchange_unknown(m.exchange, task_id.as_deref(), &why)
                    .await
                {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => return Err(format!("{e:#}")),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::Kind;

    fn user(text: &str) -> A2aMessage {
        let mut m = A2aMessage::new(Role::User, vec![Part::text(text)]);
        m.message_id = "m-1".into();
        m
    }

    fn task_row(status: A2aTaskStatus, snap: &Task, error: Option<&str>) -> A2aTask {
        A2aTask {
            seq: 1,
            id: snap.id.clone(),
            context_id: snap.context_id.clone(),
            principal: "peer:x".into(),
            export: "research".into(),
            session: SessionId::new(),
            request_scope: Uuid::now_v7(),
            status,
            revision: 3,
            cancel_requested: false,
            snapshot: serde_json::to_value(snap).unwrap(),
            error: error.map(str::to_owned),
            dedup_exchange: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn inbound_validation_rejects_protojson_defaults_and_non_text() {
        let id = |t: Result<String, A2AError>| t.unwrap_err().code;
        let mut empty_id = user("hi");
        empty_id.message_id.clear();
        assert_eq!(
            id(inbound_text(&empty_id, str::to_owned)),
            a2a_types::error_code::INVALID_PARAMS
        );
        let mut no_role = user("hi");
        no_role.role = Role::Unspecified;
        assert_eq!(
            id(inbound_text(&no_role, str::to_owned)),
            a2a_types::error_code::INVALID_PARAMS
        );
        let mut no_parts = user("hi");
        no_parts.parts.clear();
        assert_eq!(
            id(inbound_text(&no_parts, str::to_owned)),
            a2a_types::error_code::INVALID_PARAMS
        );
        let mut empty_task = user("hi");
        empty_task.task_id = Some(String::new());
        assert_eq!(
            id(inbound_text(&empty_task, str::to_owned)),
            a2a_types::error_code::INVALID_PARAMS
        );
        let mut binary = user("hi");
        binary.parts.push(Part::raw(vec![1, 2, 3]));
        assert_eq!(
            id(inbound_text(&binary, str::to_owned)),
            a2a_types::error_code::CONTENT_TYPE_NOT_SUPPORTED
        );
        let mut big = user("x");
        big.parts = vec![Part::text("x".repeat(PEER_TEXT_MAX + 1))];
        assert_eq!(
            id(inbound_text(&big, str::to_owned)),
            a2a_types::error_code::INVALID_PARAMS
        );
        // Redaction runs on the joined text; parts join with newlines.
        let mut two = user("a");
        two.parts.push(Part::text("secret"));
        assert_eq!(
            inbound_text(&two, |t| t.replace("secret", "[redacted]")).unwrap(),
            "a\n[redacted]"
        );
    }

    #[test]
    fn snapshot_projection_follows_store_status_and_history_length() {
        let snap = Task {
            id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            artifacts: Some(vec![text_artifact("partial")]),
            history: Some(vec![user("one"), user("two"), user("three")]),
            metadata: None,
        };
        // After `fail_interrupted` the JSON says working; the row says failed.
        let row = task_row(A2aTaskStatus::Failed, &snap, Some("interrupted_by_restart"));
        let t = project(&row, Some(1), false);
        assert_eq!(t.status.state, TaskState::Failed);
        assert_eq!(
            t.status.message.unwrap().text(),
            Some("interrupted_by_restart")
        );
        assert_eq!(t.history.as_ref().map(Vec::len), Some(1));
        assert!(
            t.artifacts.is_none(),
            "ListTasks omits artifacts unless asked"
        );
        // historyLength 0 drops history; unset keeps it whole and matching status keeps its message.
        let row = task_row(A2aTaskStatus::Working, &snap, None);
        assert!(project(&row, Some(0), true).history.is_none());
        let full = project(&row, None, true);
        assert_eq!(full.history.as_ref().map(Vec::len), Some(3));
        assert_eq!(full.artifacts.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn remote_task_text_prefers_artifacts_then_status_message() {
        let mut t = Task {
            id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::InputRequired,
                message: Some(agent_message("which one?", "t", "c")),
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };
        assert_eq!(task_text(&t), "which one?");
        t.artifacts = Some(vec![text_artifact("A"), text_artifact("B")]);
        assert_eq!(task_text(&t), "A\nB");
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(status_of(&TaskState::Unspecified), None);
        assert_eq!(
            status_of(&TaskState::Canceled),
            Some(A2aTaskStatus::Canceled)
        );
    }

    /// A generation both of whose endpoints stay enabled.
    fn live() -> Generation {
        Generation {
            revision: 1,
            tokens: [CancellationToken::new(), CancellationToken::new()],
        }
    }

    #[tokio::test]
    async fn follow_streams_snapshot_first_then_stops_at_terminal_or_revocation() {
        let mut state = NetworkState::default();
        let (rx, guard) = state.subscribe("t").unwrap();
        let first = Task {
            id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };
        let route = CancellationToken::new();
        let mut s = follow(first.clone(), rx, live(), route.clone(), guard);
        assert!(matches!(s.next().await, Some(Ok(StreamResponse::Task(t))) if t.id == "t"));
        state.emit(
            "t",
            StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id: "t".into(),
                context_id: "c".into(),
                status: TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: None,
                },
                metadata: None,
            }),
        );
        assert!(matches!(
            s.next().await,
            Some(Ok(StreamResponse::StatusUpdate(_)))
        ));
        assert!(s.next().await.is_none(), "terminal event ends the stream");
        drop(s);
        assert_eq!(
            state.subscribers.load(Ordering::Relaxed),
            0,
            "guard released"
        );

        // Route revocation ends a live stream with a protocol error.
        let (rx, guard) = state.subscribe("t").unwrap();
        let mut s = follow(first, rx, live(), route.clone(), guard);
        let _ = s.next().await;
        route.cancel();
        assert!(
            matches!(s.next().await, Some(Err(e)) if e.code == a2a_types::error_code::UNSUPPORTED_OPERATION)
        );
        assert!(s.next().await.is_none());
    }

    #[test]
    fn subscriber_caps_are_enforced_per_task_and_globally() {
        let mut state = NetworkState::default();
        let mut held = Vec::new();
        for _ in 0..SUBSCRIBERS_PER_TASK {
            held.push(state.subscribe("t").unwrap());
        }
        assert_eq!(
            state.subscribe("t").err().unwrap().code,
            a2a_types::error_code::UNSUPPORTED_OPERATION
        );
        // Another task still has room until the harness cap.
        assert!(state.subscribe("u").is_ok());
        drop(held);
        assert_eq!(state.feeds["t"].count.load(Ordering::Relaxed), 0);
    }

    /// A started exported task in a fresh native session (a stored row the
    /// policy knows as its own root): the rows a `SendMessage` would create,
    /// then `working` with its scope registered. Returns the exported
    /// session, the task and its initiating exchange.
    async fn exported_task(a: &mut Agents, principal: &str) -> (SessionId, A2aTask, PeerReceipt) {
        let session = SessionId::new();
        a.store
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        a.communication
            .0
            .write()
            .record_root(session, super::super::PeerScope::Owner)
            .unwrap();
        let scope = A2aScope {
            principal,
            export: "research",
        };
        a.store
            .create_a2a_context(scope, "ctx-1", session)
            .await
            .unwrap();
        let to = format!("session:{session}");
        let accept = AcceptExchange {
            sender: principal,
            recipient: &to,
            sender_session: None,
            recipient_session: Some(session),
            request_id: Some("msg-1"),
            reply_to: None,
            request_scope: None,
            generation: 1,
            body: "do research",
        };
        let (receipt, task) = a
            .store
            .accept_a2a_request(
                scope,
                &accept,
                A2aRequest::New {
                    task_id: "task-1",
                    context_id: "ctx-1",
                    snapshot: serde_json::json!({ "status": { "state": "submitted" } }),
                },
            )
            .await
            .unwrap();
        let task = a
            .transition(&task, A2aTaskStatus::Working, None, None, None)
            .await
            .unwrap();
        a.register_scope(&task, CancellationToken::new());
        (session, task, receipt)
    }

    fn ticket(id: u64) -> InputTicket {
        InputTicket { id, epoch: 0 }
    }

    fn finished(
        session: SessionId,
        turn: TurnId,
        scope: Option<Uuid>,
        text: &str,
    ) -> PeerTurnFinished {
        PeerTurnFinished {
            session,
            turn,
            scope,
            text: text.into(),
            stop_reason: "end_turn".into(),
        }
    }

    fn artifact_text(t: &A2aTask) -> String {
        let snap: Task = serde_json::from_value(t.snapshot.clone()).unwrap();
        snap.artifacts
            .iter()
            .flatten()
            .flat_map(|a| a.parts.iter().filter_map(Part::as_text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The stored row of `task-1` as `principal` sees it.
    async fn current(store: &crate::store::Store, principal: &str) -> A2aTask {
        let scope = A2aScope {
            principal,
            export: "research",
        };
        store.a2a_task(scope, "task-1").await.unwrap().unwrap()
    }

    /// §6.3 ownership is explicit, never "the session's active task": a
    /// private human turn that runs while an exported task waits on its own
    /// work is not attributed — its text never becomes the task's result or
    /// artifact, its operations bind nothing, its approval does not surface
    /// — while the task's own continuation (a job its turn started, reported
    /// into its scope) keeps the task open and finally completes it with the
    /// continuation turn's text.
    #[tokio::test]
    async fn private_turns_never_fill_or_settle_an_exported_task() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (session, task, _) = exported_task(&mut a, "peer:lab").await;
        let rs = task.request_scope;
        let status = |a: &Agents| {
            let store = a.store.clone();
            async move { current(&store, "peer:lab").await }
        };

        // The task's input is work from admission; consuming it binds its turn.
        assert!(a.network_input_queued(rs, ticket(1)).await);
        let t1 = TurnId::new();
        a.network_input_consumed(&InputConsumed {
            session,
            id: 1,
            turn: t1,
        })
        .await;
        assert_eq!(a.network_turn_scope(session), Some(rs));
        // A background job started from that turn is the scope's too.
        let job = Operation {
            id: OperationId(1),
            owner: session,
            kind: Kind::Command("job"),
        };
        a.network_operation_admitted(&job).await;
        assert_eq!(a.network_operation_scope(job.id), Some(rs));
        a.network_turn_finished(&finished(session, t1, Some(rs), "job started; will report"))
            .await;
        assert_eq!(a.network_turn_scope(session), None);
        assert_eq!(
            status(&a).await.status,
            A2aTaskStatus::Working,
            "owned job still runs"
        );

        // The owner types privately meanwhile: a turn of no scope. Nothing of
        // it is the task's, even though the task is active in this session.
        let private = TurnId::new();
        let private_op = Operation {
            id: OperationId(2),
            owner: session,
            kind: Kind::Command("private"),
        };
        a.network_operation_admitted(&private_op).await;
        assert_eq!(a.network_operation_scope(private_op.id), None);
        a.network_approval(session, true).await;
        assert!(!a.network.scopes[&rs].approval);
        a.network_turn_finished(&finished(
            session,
            private,
            None,
            "the owner's private notes",
        ))
        .await;
        assert_eq!(
            a.network.scopes[&rs].result.as_ref().unwrap().as_deref(),
            Ok("job started; will report"),
            "a private turn never replaces the result candidate"
        );
        a.network_operation_settled(private_op.id, None).await;
        let cur = status(&a).await;
        assert_eq!(
            cur.status,
            A2aTaskStatus::Working,
            "private work settles nothing"
        );

        // The job reports into the task's scope: queued result is work until
        // consumed, so settling the operation completes nothing yet.
        assert!(a.network_input_queued(rs, ticket(2)).await);
        a.network_operation_settled(job.id, None).await;
        assert_eq!(status(&a).await.status, A2aTaskStatus::Working);
        let t2 = TurnId::new();
        a.network_input_consumed(&InputConsumed {
            session,
            id: 2,
            turn: t2,
        })
        .await;
        assert_eq!(a.network_turn_scope(session), Some(rs));
        a.network_turn_finished(&finished(session, t2, Some(rs), "final answer"))
            .await;
        let done = status(&a).await;
        assert_eq!(done.status, A2aTaskStatus::Completed);
        assert_eq!(artifact_text(&done), "final answer");
        assert!(!done.snapshot.to_string().contains("private notes"));
        assert!(!a.network.scopes.contains_key(&rs), "scope retired");
    }

    /// An owned child's turn is the task's work but never its result: only a
    /// turn of the exported session itself sets the candidate.
    #[tokio::test]
    async fn owned_child_turns_are_work_not_results() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (session, task, _) = exported_task(&mut a, "peer:lab").await;
        let rs = task.request_scope;
        let child = SessionId::new();
        a.store
            .create_session(child, "m", Some(session), Some("kid"), None)
            .await
            .unwrap();
        a.network_bind_work(rs, &format!("child:{child}")).await;
        assert!(a.network_input_queued(rs, ticket(1)).await);
        let ct = TurnId::new();
        a.network_input_consumed(&InputConsumed {
            session: child,
            id: 1,
            turn: ct,
        })
        .await;
        assert_eq!(
            a.network_turn_scope(child),
            Some(rs),
            "grandchildren bind to the task"
        );
        a.network_turn_finished(&finished(child, ct, Some(rs), "child's findings"))
            .await;
        assert!(a.network.scopes[&rs].result.is_none());
        assert_eq!(a.network_turn_scope(child), None);
        assert_eq!(
            current(&a.store, "peer:lab").await.status,
            A2aTaskStatus::Working
        );
    }

    /// §8: a job the task's turn started reports late — after the runtime
    /// cap interrupted the turn, after the owner cancelled the task, or
    /// after the task ended — and each time its result is refused (no
    /// pending input, so no turn), while settling the operation still lets
    /// the task reach its terminal state. The op→scope binding outlives the
    /// scope: a result landing after the task ended is never unscoped.
    #[tokio::test]
    async fn late_results_of_a_cancelled_or_ended_task_start_no_turn() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (session, task, _) = exported_task(&mut a, "peer:lab").await;
        let rs = task.request_scope;
        let status = |a: &Agents| {
            let store = a.store.clone();
            async move { current(&store, "peer:lab").await.status }
        };
        let job = |n: u64| Operation {
            id: OperationId(n),
            owner: session,
            kind: Kind::Command("job"),
        };

        // The task's turn starts three detached jobs, then the cap stops it.
        assert!(a.network_input_queued(rs, ticket(1)).await);
        let t1 = TurnId::new();
        a.network_input_consumed(&InputConsumed {
            session,
            id: 1,
            turn: t1,
        })
        .await;
        for n in 1..=3 {
            a.network_operation_admitted(&job(n)).await;
        }
        let mut timed_out = finished(session, t1, Some(rs), "");
        timed_out.stop_reason = "timed_out".into();
        a.network_turn_finished(&timed_out).await;
        assert_eq!(
            status(&a).await,
            A2aTaskStatus::Working,
            "owned jobs still run"
        );

        // Interrupted: the first job's result is refused, its settlement counts.
        assert!(!a.network_input_queued(rs, ticket(2)).await);
        a.network_operation_settled(OperationId(1), None).await;
        assert_eq!(status(&a).await, A2aTaskStatus::Working);

        // The owner cancels (`CancelTask`): the second job's result is refused.
        let scope = A2aScope {
            principal: "peer:lab",
            export: "research",
        };
        a.store.request_a2a_cancel(scope, "task-1").await.unwrap();
        a.stop_scope(&task).await;
        assert!(a.network.scopes[&rs].cancelled);
        assert!(!a.network_input_queued(rs, ticket(3)).await);
        a.network_operation_settled(OperationId(2), None).await;
        assert_eq!(
            status(&a).await,
            A2aTaskStatus::Working,
            "one job still owned"
        );

        // The task ends (`failed` is not held by owned work): the last job's
        // result still names the scope and is refused; its settlement is inert.
        a.network_revoked(None, "off").await;
        assert_eq!(status(&a).await, A2aTaskStatus::Failed);
        assert!(!a.network.scopes.contains_key(&rs), "scope retired");
        assert_eq!(a.network_operation_scope(OperationId(3)), Some(rs));
        assert!(!a.network_input_queued(rs, ticket(4)).await);
        a.network_operation_settled(OperationId(3), None).await;
        assert!(a.network.ops.is_empty());
        assert_eq!(status(&a).await, A2aTaskStatus::Failed);
    }

    /// Accept a second `submitted` task in the exported session (its scope busy).
    async fn accepted_task(a: &mut Agents, session: SessionId, principal: &str, n: u8) -> A2aTask {
        let scope = A2aScope {
            principal,
            export: "research",
        };
        let to = format!("session:{session}");
        let (request_id, task_id) = (format!("msg-{n}"), format!("task-{n}"));
        let accept = AcceptExchange {
            sender: principal,
            recipient: &to,
            sender_session: None,
            recipient_session: Some(session),
            request_id: Some(&request_id),
            reply_to: None,
            request_scope: None,
            generation: 1,
            body: "and this",
        };
        a.store
            .accept_a2a_request(
                scope,
                &accept,
                A2aRequest::New {
                    task_id: &task_id,
                    context_id: "ctx-1",
                    snapshot: serde_json::json!({ "status": { "state": "submitted" } }),
                },
            )
            .await
            .unwrap()
            .1
    }

    /// §8: a task queued behind the session's active one keeps the grant
    /// epoch it was admitted under. When that epoch ends while it waits
    /// (credential rotated, export or binding remapped) the task fails —
    /// revoked while queued, or at dequeue when the revocation is still in
    /// flight — and is never started under whatever epoch the binding has
    /// by then; a task with no remembered admission cannot start at all.
    #[tokio::test]
    async fn queued_tasks_keep_their_admitted_epoch_and_never_start_under_a_newer_one() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let principal = format!("peer:{}", Uuid::now_v7());
        let (session, active, _) = exported_task(&mut a, &principal).await;
        let task = |a: &Agents, id: &'static str| {
            let (store, principal) = (a.store.clone(), principal.clone());
            async move {
                let scope = A2aScope {
                    principal: &principal,
                    export: "research",
                };
                store.a2a_task(scope, id).await.unwrap().unwrap()
            }
        };
        let status = |a: &Agents, id: &'static str| {
            let t = task(a, id);
            async move { t.await.status }
        };

        // Three tasks wait behind the active one, each under its own epoch.
        let mut routes = Vec::new();
        for n in 2..=4 {
            let t = accepted_task(&mut a, session, &principal, n).await;
            let route = CancellationToken::new();
            let queued = a.start_or_queue(t.clone(), live(), route.clone()).await;
            assert_eq!(queued.status, A2aTaskStatus::Submitted);
            assert!(a.network.queued.contains_key(&t.request_scope));
            routes.push((t, route));
        }

        // Revoked while queued: fails at once, the active task is untouched.
        let (b, route_b) = &routes[0];
        route_b.cancel();
        a.abort_scope(b.request_scope, "peer route revoked").await;
        assert_eq!(status(&a, "task-2").await, A2aTaskStatus::Failed);
        assert!(!a.network.queued.contains_key(&b.request_scope));
        assert_eq!(status(&a, "task-1").await, A2aTaskStatus::Working);

        // Remapped while queued (its epoch ended, the binding may have a new
        // one): at dequeue it fails, never starts. A task whose admission
        // was lost (accepted by an earlier process) cannot start either. The
        // active task completes, which advances the queue.
        let (c, route_c) = &routes[1];
        route_c.cancel();
        let (d, _) = &routes[2];
        a.network
            .queued
            .remove(&d.request_scope)
            .unwrap()
            .watch
            .abort();
        assert!(
            a.network_input_queued(active.request_scope, ticket(1))
                .await
        );
        let t1 = TurnId::new();
        a.network_input_consumed(&InputConsumed {
            session,
            id: 1,
            turn: t1,
        })
        .await;
        a.network_turn_finished(&finished(session, t1, Some(active.request_scope), "done"))
            .await;
        assert_eq!(status(&a, "task-1").await, A2aTaskStatus::Completed);
        let c_row = task(&a, "task-3").await;
        assert_eq!(c_row.status, A2aTaskStatus::Failed);
        assert_eq!(c_row.error.as_deref(), Some("peer route revoked"));
        let d_row = task(&a, "task-4").await;
        assert_eq!(d_row.status, A2aTaskStatus::Failed);
        assert_eq!(d_row.error.as_deref(), Some("admission lost before start"));
        assert!(a.network.queued.is_empty());
        assert!(!a.network.active.contains_key(&session), "nothing started");
        assert!(!a.network.scopes.contains_key(&c.request_scope));
        assert!(!a.network.scopes.contains_key(&d.request_scope));
    }

    /// §6.3: a question on the exported task travels on the task's own
    /// channel, so the serving session needs only the live binding that
    /// opened the task — an inbound-only peer (no sender, no root grant) can
    /// be asked and the task goes `INPUT_REQUIRED`. The same peer still
    /// accepts no fresh outbound message, and another session cannot use
    /// the task's exchange as its ticket out.
    #[tokio::test]
    async fn inbound_only_peer_can_be_asked_on_its_own_task() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = Uuid::now_v7();
        let principal = format!("peer:{binding}");
        let (session, task, receipt) = exported_task(&mut a, &principal).await;
        a.communication.0.write().set_network(true, true);
        a.network.peers.insert(
            binding,
            RemotePeer {
                name: "lab".into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: false,
            },
        );

        let fresh = a
            .network_send(session, "lab", "hello?", None, None, false)
            .await
            .unwrap_err();
        assert!(fresh.contains("inbound-only"), "{fresh}");

        let other = SessionId::new();
        a.store
            .create_session(other, "m", None, None, None)
            .await
            .unwrap();
        a.communication
            .0
            .write()
            .record_root(other, super::super::PeerScope::Owner)
            .unwrap();
        let foreign = a
            .network_send(other, "lab", "mine now", None, Some(receipt.id), true)
            .await
            .unwrap_err();
        assert!(
            foreign.contains("does not name a message this peer sent you"),
            "{foreign}"
        );

        let q = a
            .network_send(session, "lab", "which one?", None, Some(receipt.id), true)
            .await
            .unwrap();
        assert_eq!(q.state, PeerMessageState::Accepted);
        assert_eq!(a.network.scopes[&task.request_scope].question, Some(q.id));
        let cur = current(&a.store, &principal).await;
        assert_eq!(cur.status, A2aTaskStatus::InputRequired);
        assert!(a.network.outbound.is_empty(), "nothing left over HTTP");
    }

    fn bob(export: &str, cap: tools::Risk) -> super::super::ContactOrigin {
        super::super::ContactOrigin {
            contact: "bob".into(),
            machine: "Bob's MacBook".into(),
            export: export.into(),
            cap,
            holder: "me".into(),
            tools: vec!["search_web".into(), "read_url".into()],
        }
    }

    fn send(id: &str) -> SendMessageRequest {
        let mut m = user("do research");
        m.message_id = id.into();
        SendMessageRequest {
            message: m,
            configuration: None,
            metadata: None,
            tenant: None,
        }
    }

    /// 183 W-27, F11: a contact's first task waits in `AUTH_REQUIRED` at once, saying so, and
    /// aside: the session's running task keeps its slot, an allowed message queues behind that
    /// one only and starts when it ends, never behind the held one; Allow once queues the held
    /// one normally; Block fails a waiting one as not a contact.
    #[tokio::test]
    async fn first_contact_holds_until_the_owner_answers() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, busy, _) = exported_task(&mut a, &principal).await;
        let route = CancellationToken::new();
        let p = |ask_first: bool| A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: route.clone(),
            contact: Some(bob("research", tools::Risk::ReadOnly)),
            ask_first,
        };
        let s = A2aScope {
            principal: &principal,
            export: "research",
        };
        let first = a.admit_send(&p(true), &send("m-a")).await.unwrap().task;
        assert_eq!(
            first.status,
            A2aTaskStatus::AuthRequired,
            "held at once, though the session is busy"
        );
        let why = snapshot(&first).status.message.unwrap();
        let why = why.text().unwrap();
        assert!(
            why.starts_with("waiting: ")
                && why.ends_with("has not allowed messages to research yet"),
            "{why}"
        );
        assert_eq!(a.network.held[&("bob".into(), "research".into())].len(), 1);
        // Z3-R2 (M3): a message naming the held task cannot skip the wait.
        let mut skip = send("m-skip");
        skip.message.task_id = Some(first.id.clone());
        let refused = a.admit_send(&p(true), &skip).await.err().expect("refused");
        assert!(
            refused.message.contains("not allowed this task yet"),
            "{refused:?}"
        );
        // F11: an allowed message waits for the running task only.
        let allowed = a.admit_send(&p(false), &send("m-b")).await.unwrap().task;
        assert_eq!(
            allowed.status,
            A2aTaskStatus::Submitted,
            "behind the busy one"
        );
        let busy = a.store.a2a_task(s, &busy.id).await.unwrap().unwrap();
        a.transition(&busy, A2aTaskStatus::Completed, None, Some("done"), None)
            .await
            .unwrap();
        a.network.retire(busy.request_scope);
        a.start_next(session).await;
        let allowed = a.store.a2a_task(s, &allowed.id).await.unwrap().unwrap();
        assert_ne!(
            allowed.status,
            A2aTaskStatus::Submitted,
            "the allowed message starts though one still waits for the owner"
        );
        let still = a.store.a2a_task(s, &first.id).await.unwrap().unwrap();
        assert_eq!(still.status, A2aTaskStatus::AuthRequired, "it did not run");

        // Allow once: it is queued as any admitted message (behind the running one).
        let n = a
            .release_held(ContactRelease {
                contact: "bob".into(),
                export: Some("research".into()),
                allow: true,
            })
            .await;
        assert_eq!(n, 1);
        assert!(a.network.held.is_empty());
        let now = a.store.a2a_task(s, &first.id).await.unwrap().unwrap();
        assert_ne!(now.status, A2aTaskStatus::AuthRequired);
        if allowed.status.is_active() {
            assert_eq!(
                now.status,
                A2aTaskStatus::Submitted,
                "behind the running one"
            );
        }

        // Block: a first-contact task still waiting fails as not a contact.
        let waiting = a.admit_send(&p(true), &send("m-c")).await.unwrap().task;
        assert_eq!(waiting.status, A2aTaskStatus::AuthRequired);
        let n = a
            .release_held(ContactRelease {
                contact: "bob".into(),
                export: None,
                allow: false,
            })
            .await;
        assert_eq!(n, 1);
        let now = a.store.a2a_task(s, &waiting.id).await.unwrap().unwrap();
        assert_eq!(now.status, A2aTaskStatus::Failed);
        assert_eq!(now.error.as_deref(), Some("peer:bob: not a contact"));
    }

    /// 183 F5: a held first message the owner never answers fails "the owner did not
    /// answer" (and no longer shows as waiting); the session's own work is untouched.
    #[tokio::test]
    async fn a_held_first_message_expires_unanswered() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, busy, _) = exported_task(&mut a, &principal).await;
        let p = A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research", tools::Risk::ReadOnly)),
            ask_first: true,
        };
        let s = A2aScope {
            principal: &principal,
            export: "research",
        };
        let held = a.admit_send(&p, &send("m-a")).await.unwrap().task;
        assert_eq!(held.status, A2aTaskStatus::AuthRequired);
        // What the `HOLD_TTL` timer delivers.
        a.held_ended(held.request_scope, true).await;
        let now = a.store.a2a_task(s, &held.id).await.unwrap().unwrap();
        assert_eq!(now.status, A2aTaskStatus::Failed);
        assert_eq!(now.error.as_deref(), Some(HOLD_EXPIRED));
        assert!(a.network.held.is_empty(), "nothing waits any more");
        let busy = a.store.a2a_task(s, &busy.id).await.unwrap().unwrap();
        assert_eq!(
            busy.status,
            A2aTaskStatus::Working,
            "the session's task runs on"
        );
        // A second end (the revoke racing the timer) changes nothing.
        a.held_ended(held.request_scope, false).await;
        let again = a.store.a2a_task(s, &held.id).await.unwrap().unwrap();
        assert_eq!(again.error.as_deref(), Some(HOLD_EXPIRED));
    }

    /// Z3-R3: a contact that never reuses a context (our own sender passes none) is never
    /// locked out: 70 sequential messages, each in a fresh context, are all admitted.
    #[tokio::test]
    async fn a_contact_never_runs_out_of_contexts() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, busy, _) = exported_task(&mut a, &principal).await;
        let p = A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research", tools::Risk::ReadOnly)),
            ask_first: false,
        };
        let s = A2aScope {
            principal: &principal,
            export: "research",
        };
        let mut done = a.store.a2a_task(s, &busy.id).await.unwrap().unwrap();
        for i in 0..70 {
            // Each answered before the next one (its turn is not run here: fail it).
            a.transition(&done, A2aTaskStatus::Failed, None, None, Some("ended"))
                .await
                .unwrap();
            a.start_next(session).await;
            let task = a
                .admit_send(&p, &send(&format!("m-{i}")))
                .await
                .unwrap_or_else(|e| panic!("message {i}: {e:?}"))
                .task;
            done = a.store.a2a_task(s, &task.id).await.unwrap().unwrap();
        }
    }

    /// Z3-R4: retries of one messageId (each with no contextId) are answered from the first
    /// task and leave no context behind: 5 sends, 1 task, 1 new context.
    #[tokio::test]
    async fn retries_of_one_message_leave_one_context() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, _, _) = exported_task(&mut a, &principal).await;
        let p = A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research", tools::Risk::ReadOnly)),
            ask_first: false,
        };
        let s = A2aScope {
            principal: &principal,
            export: "research",
        };
        let before = a.store.a2a_context_count_for_test(s).await;
        let mut ids = Vec::new();
        for _ in 0..5 {
            ids.push(a.admit_send(&p, &send("m-same")).await.unwrap().task.id);
        }
        ids.dedup();
        assert_eq!(ids.len(), 1, "one task");
        assert_eq!(a.store.a2a_context_count_for_test(s).await, before + 1);
    }

    /// Z3-R4: a chat another engine made (its row replicated here, never run here) is
    /// enabled in this engine's policy as soon as an export of it is validated — the put's
    /// reload — so a contact's message right after the grant is not refused as unknown.
    #[tokio::test]
    async fn an_exported_chat_from_another_engine_is_open_as_soon_as_it_is_exported() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let chat = SessionId::new();
        a.store
            .create_session(chat, "m", None, None, None)
            .await
            .unwrap();
        assert!(
            a.communication.0.read().enabled(chat).is_err(),
            "a replicated row the policy has not seen"
        );
        a.validate_export(chat).await.unwrap();
        assert!(a.communication.0.read().enabled(chat).is_ok());
    }

    /// Z3-R4: an inbox, and any session under it, spawns no subagent at all (a child is no
    /// contact session of its own, so it would carry the owner's tools back to the contact),
    /// whatever the export's allowlist says; an owner root still spawns.
    #[tokio::test]
    async fn a_contact_session_spawns_nothing() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let mut origin = bob("research", tools::Risk::Mutating);
        origin.tools.push("spawn_subagent".into());
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin,
            })
            .await
            .unwrap();
        let (below, _) = a
            .session(None, Some((inbox, "below")), None, (None, None))
            .await
            .unwrap();
        let (owner, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let agents = <Agents as kameo::actor::Spawn>::spawn(a);
        let spawn = |parent| crate::supervisor::SpawnChild {
            parent,
            name: Some("x".into()),
            task: "read the owner's notes".into(),
            agent: None,
            cwd: None,
            model: None,
            effort: None,
            cap: None,
            skills: Vec::new(),
            call_id: String::new(),
        };
        for parent in [inbox, below] {
            let e = agents.ask(spawn(parent)).await.unwrap_err();
            assert!(e.to_string().contains("not available to a contact"), "{e}");
        }
        assert!(agents.ask(spawn(owner)).await.is_ok());
    }

    /// Z4 (security re-check): with the live registry, a contact's turn in an owner's chat
    /// (a turn-scoped `SetAllowOnly` on it) refuses `SpawnChild { parent: chat }`, even with
    /// an empty allowlist; the turn's end lets the owner spawn again.
    #[test]
    fn a_contact_turn_in_a_chat_spawns_nothing_at_the_registry() {
        crate::with_test_registry(async {
            let reg_dir = crate::TestDir::new("spawn-reg");
            let reg = tools::registry::start(&runtime::Runtime::new(
                figment::Figment::new(),
                reg_dir.0.clone(),
            ))
            .await
            .unwrap();
            let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
            let (chat, _) = a.session(None, None, None, (None, None)).await.unwrap();
            let agents = <Agents as kameo::actor::Spawn>::spawn(a);
            let spawn = || crate::supervisor::SpawnChild {
                parent: chat,
                name: Some("x".into()),
                task: "read the owner's notes".into(),
                agent: None,
                cwd: None,
                model: None,
                effort: None,
                cap: None,
                skills: Vec::new(),
                call_id: String::new(),
            };
            let limits = |tools| tools::SetAllowOnly {
                session: chat,
                tools,
                turn: true,
            };
            reg.ask(limits(Some(Vec::new()))).await.unwrap();
            let e = agents.ask(spawn()).await.unwrap_err();
            assert!(e.to_string().contains("not available to a contact"), "{e}");
            reg.ask(limits(None)).await.unwrap();
            assert!(agents.ask(spawn()).await.is_ok());
        });
    }

    /// 183 Z3-R2 (M2): a contact's inbox lists no contact and reaches none (no relay of
    /// the owner's agent through it); an owner root still lists the contact.
    #[tokio::test]
    async fn an_inbox_neither_sees_nor_reaches_another_contact() {
        struct Never;
        #[async_trait]
        impl PeerSender for Never {
            async fn send(&self, _: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError> {
                Err(RemoteSendError::Refused("never".into()))
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.network.peers.insert(
            Uuid::now_v7(),
            RemotePeer {
                name: "carol".into(),
                roots: Vec::new(),
                sender: Some(Arc::new(Never)),
                generation: CancellationToken::new(),
                contact: true,
            },
        );
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin: bob("research", tools::Risk::ReadOnly),
            })
            .await
            .unwrap();
        assert!(a.network.remote_peers(inbox).is_empty(), "no contact rows");
        let sent = a
            .network_send(inbox, "carol", "relay this", None, None, false)
            .await;
        assert!(sent.is_err(), "no contact send from an inbox: {sent:?}");
        let owner = a
            .new_root(None, super::super::PeerScope::Owner, None, None)
            .await
            .unwrap();
        assert_eq!(a.network.remote_peers(owner).len(), 1);
    }

    /// 183 F7, F8: a contact's first-contact hold makes the sender's receipt `held` with the
    /// line its tools show (no peer message that reads like a login failure); the receipt
    /// names the peer as written (`peer:sky/hi-there`), not by its binding id; once the
    /// owner allows it the message is delivered (`consumed`, no explanation) and the reply
    /// still answers it.
    #[tokio::test]
    async fn a_held_send_reads_held_and_names_the_contact() {
        struct Silent;
        #[async_trait]
        impl PeerSender for Silent {
            async fn send(&self, _: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError> {
                Err(RemoteSendError::Unknown("not in this test".into()))
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("sky", &contact_endpoint("sky-endpoint"))
            .await
            .unwrap();
        a.network.peers.insert(
            binding.id,
            RemotePeer {
                name: "sky".into(),
                roots: Vec::new(),
                sender: Some(Arc::new(Silent)),
                generation: CancellationToken::new(),
                contact: true,
            },
        );
        let owner = a
            .new_root(None, super::super::PeerScope::Owner, None, None)
            .await
            .unwrap();
        let sent = a
            .network_send(owner, "sky/hi-there", "hello", None, None, false)
            .await
            .unwrap();
        assert_eq!(sent.to, "peer:sky/hi-there", "F8: not the binding id");
        let mut t = Task {
            id: "t-1".into(),
            context_id: "c-1".into(),
            status: TaskStatus {
                state: TaskState::AuthRequired,
                message: Some(agent_message(
                    "waiting: 333 has not allowed messages to hi-there yet",
                    "t-1",
                    "c-1",
                )),
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };
        a.remote_outcome(sent.id, Ok(SendMessageResponse::Task(t.clone())))
            .await;
        let r = a.store.exchange_receipt(sent.id).await.unwrap().unwrap();
        assert_eq!(r.state, PeerMessageState::Held);
        assert_eq!(
            r.error.as_deref(),
            Some(
                "held: sky (333) has not allowed messages to hi-there yet; it will be \
                 delivered when they press Allow — do not resend"
            )
        );
        assert!(
            a.store
                .pending_exchanges(owner, 10)
                .await
                .unwrap()
                .is_empty(),
            "no peer message reached the sender's session"
        );
        // Allowed: it runs on the owner's side, then answers.
        t.status = TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: None,
        };
        a.apply_remote_task(sent.id, &t).await;
        let r = a.store.exchange_receipt(sent.id).await.unwrap().unwrap();
        assert_eq!(r.state, PeerMessageState::Consumed);
        assert_eq!(r.error, None);
        t.status.state = TaskState::Completed;
        t.artifacts = Some(vec![text_artifact("hi back")]);
        a.apply_remote_task(sent.id, &t).await;
        let r = a.store.exchange_receipt(sent.id).await.unwrap().unwrap();
        assert_eq!(r.state, PeerMessageState::Replied);
        assert_eq!(r.reply.as_deref(), Some("hi back"));
    }

    /// 183 F4: a grant change keeps a held first message whose export the contact still
    /// has: it moves to the new grant epoch, so Allow once runs it after the old epoch ended;
    /// one whose export was taken away keeps the old epoch and fails at Allow.
    #[tokio::test]
    async fn a_grant_change_keeps_held_first_messages_of_exports_still_granted() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, busy, _) = exported_task(&mut a, &principal).await;
        let old = CancellationToken::new();
        let p = |export: &str| A2aPrincipal {
            binding: binding.id,
            export: export.into(),
            session,
            route: old.clone(),
            contact: Some(bob(export, tools::Risk::ReadOnly)),
            ask_first: true,
        };
        let kept = a
            .admit_send(&p("research"), &send("m-a"))
            .await
            .unwrap()
            .task;
        let dropped = a.admit_send(&p("other"), &send("m-b")).await.unwrap().task;
        assert_eq!(kept.status, A2aTaskStatus::AuthRequired);
        assert_eq!(dropped.status, A2aTaskStatus::AuthRequired);
        let new = CancellationToken::new();
        a.rebind_held(RebindHeld {
            contact: "bob".into(),
            exports: vec!["research".into()],
            route: new.clone(),
        });
        old.cancel();
        // The session's own task ends, so a released task can start at once.
        let s = A2aScope {
            principal: &principal,
            export: "research",
        };
        let busy = a.store.a2a_task(s, &busy.id).await.unwrap().unwrap();
        a.transition(&busy, A2aTaskStatus::Completed, None, Some("done"), None)
            .await
            .unwrap();
        a.network.retire(busy.request_scope);
        for export in ["research", "other"] {
            a.release_held(ContactRelease {
                contact: "bob".into(),
                export: Some(export.into()),
                allow: true,
            })
            .await;
        }
        let kept = a.store.a2a_task(s, &kept.id).await.unwrap().unwrap();
        assert_ne!(kept.status, A2aTaskStatus::AuthRequired);
        assert_ne!(
            kept.error.as_deref(),
            Some("peer route revoked"),
            "the export still granted runs under the new epoch"
        );
        let other = A2aScope {
            principal: &principal,
            export: "other",
        };
        let dropped = a.store.a2a_task(other, &dropped.id).await.unwrap().unwrap();
        assert_eq!(dropped.status, A2aTaskStatus::Failed);
        assert_eq!(dropped.error.as_deref(), Some("peer route revoked"));
    }

    /// 183 Z3-R2: a lowered cap or a changed allowlist reaches the contact's live inbox at
    /// once (the reload pushes `ContactLimits`), not at its next request.
    #[tokio::test]
    async fn a_cap_or_allowlist_change_reaches_a_live_inbox_at_once() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: binding.id,
                agent: "theuth".into(),
                origin: bob("research", tools::Risk::Mutating),
            })
            .await
            .unwrap();
        a.apply_contact_limits(ContactLimits {
            contact: "bob".into(),
            cap: tools::Risk::ReadOnly,
            exports: vec![("research".into(), vec!["search_web".into()])],
        })
        .await;
        assert_eq!(
            a.network.inbox_limits.get(&inbox),
            Some(&(tools::Risk::ReadOnly, vec!["search_web".to_owned()]))
        );
        assert_eq!(a.caps.get(&inbox), Some(&tools::Risk::ReadOnly));
    }

    /// 183 Z3-R2 (M1): an inbox's limits come back at every actor start, before anything of
    /// it runs: after a restart (or a child result waking it) the engine has no admission
    /// yet, so the inbox is read-only with nothing but the reply until the contact writes;
    /// an ordinary chat gets nothing.
    #[tokio::test]
    async fn an_inbox_is_capped_and_default_deny_at_every_actor_start() {
        let scratch = crate::TestDir::new("z3r2");
        let dir = scratch.0.clone();
        let mut a = crate::supervisor::tests::agents_on(dir.clone(), None, true).await;
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin: bob("research", tools::Risk::Mutating),
            })
            .await
            .unwrap();
        let chat = a
            .new_root(None, super::super::PeerScope::Owner, None, None)
            .await
            .unwrap();
        assert_eq!(
            a.network.inbox_limits.get(&inbox),
            Some(&(
                tools::Risk::Mutating,
                vec!["search_web".to_owned(), "read_url".to_owned()]
            ))
        );
        a.store.close().await;
        drop(a);

        // The restart: a fresh engine on the same store, the inbox's actor starts again.
        let mut b = crate::supervisor::tests::agents_on(dir, None, true).await;
        assert!(!b.caps.contains_key(&inbox));
        b.session(Some(inbox), None, None, (None, None))
            .await
            .unwrap();
        assert_eq!(
            b.caps.get(&inbox),
            Some(&tools::Risk::ReadOnly),
            "fail-closed cap"
        );
        assert_eq!(
            b.network.inbox_limits.get(&inbox),
            Some(&(tools::Risk::ReadOnly, Vec::new())),
            "nothing but the reply until the contact's next admission"
        );
        b.session(Some(chat), None, None, (None, None))
            .await
            .unwrap();
        assert!(
            !b.caps.contains_key(&chat),
            "an ordinary chat is not a contact session"
        );
        assert!(!b.network.inbox_limits.contains_key(&chat));
    }

    /// Z3-R3: after a restart an inbox's actor start brings the exported agent's persona back
    /// (a resumed turn runs as that agent), under its ceiling.
    #[tokio::test]
    async fn a_restarted_inbox_runs_the_exported_agents_persona_again() {
        struct Roster;
        #[async_trait::async_trait]
        impl crate::IdentitySource for Roster {
            async fn identity(&self, _: &[SessionId]) -> Option<String> {
                None
            }
            async fn inbox(&self, agent: &str) -> Option<crate::InboxPersona> {
                (agent == "research").then(|| crate::InboxPersona {
                    identity: Some("You are Research (`research`).".into()),
                    model: None,
                    tools: Some(tools::Risk::ReadOnly),
                })
            }
        }
        let scratch = crate::TestDir::new("z3r3");
        let dir = scratch.0.clone();
        let mut a = crate::supervisor::tests::agents_on(dir.clone(), None, true).await;
        a.identity = Some(std::sync::Arc::new(Roster));
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "research@c".into(),
                origin: bob("research", tools::Risk::Mutating),
            })
            .await
            .unwrap();
        a.store.close().await;
        drop(a);

        // The boot resume starts the inbox's actor before the roster's source joins (here it
        // never does): the persona of its last admission still speaks.
        let mut b = crate::supervisor::tests::agents_on(dir, None, true).await;
        b.session(Some(inbox), None, None, (None, None))
            .await
            .unwrap();
        assert_eq!(b.caps.get(&inbox), Some(&tools::Risk::ReadOnly));
        assert_eq!(
            b.identity_of(inbox).await.as_deref(),
            Some("You are Research (`research`).")
        );
    }

    /// 183 §7.6: an agent export's inbox is one root per (contact, export), titled for both,
    /// capped at the contact's cap on every resolve (a lowered cap applies at once); its
    /// first contact row carries `<contact_inbox/>`, later ones do not.
    #[tokio::test]
    async fn a_contact_inbox_is_one_capped_root_reused_per_contact_and_export() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = Uuid::now_v7();
        let ask = |cap| ContactInbox {
            binding,
            agent: "theuth".into(),
            origin: bob("research", cap),
        };
        let first = a.handle_inbox(ask(tools::Risk::Mutating)).await.unwrap();
        assert_eq!(a.caps.get(&first), Some(&tools::Risk::Mutating));
        let row = a.store.session(first).await.unwrap().unwrap();
        assert_eq!(row.title, "bob · research");
        assert_eq!(
            a.communication.0.read().state(Some(first)).unwrap().scope,
            super::super::PeerScope::Isolated,
            "an inbox reaches no other root"
        );
        let again = a.handle_inbox(ask(tools::Risk::ReadOnly)).await.unwrap();
        assert_eq!(again, first, "the same contact and export reuse the inbox");
        assert_eq!(
            a.caps.get(&first),
            Some(&tools::Risk::ReadOnly),
            "lowered at once"
        );
        let other = a
            .handle_inbox(ContactInbox {
                binding,
                agent: "theuth".into(),
                origin: bob("ops", tools::Risk::ReadOnly),
            })
            .await
            .unwrap();
        assert_ne!(other, first, "another export, another inbox");

        let scope = Uuid::now_v7();
        a.network
            .contact_scopes
            .insert(scope, bob("research", tools::Risk::ReadOnly));
        let record = |id| PeerRecord {
            receipt: PeerReceipt {
                id,
                to: format!("session:{first}"),
                state: PeerMessageState::Accepted,
                reply_to: None,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            },
            from: format!("peer:{binding}"),
            session: first,
            text: "hi".into(),
            created_at: Utc::now(),
            request_scope: Some(scope),
        };
        let mut frame = |r| match a.network_contact_frame(&r) {
            Some(crate::loop_::ContactFrame::Inbox { first, .. }) => first,
            other => panic!("an inbox frame, got {other:?}"),
        };
        assert!(
            frame(record(Uuid::now_v7())),
            "the first row carries the notice"
        );
        assert!(!frame(record(Uuid::now_v7())), "later rows do not");
    }

    /// 183 §7.7: `peer:<contact>/<export>` names the contact's route (the export rides on
    /// the request); an HTTP peer takes no export, and a malformed export names nothing.
    #[test]
    fn a_contact_export_address_names_the_contacts_route() {
        let mut n = NetworkState::default();
        let (bob, lab) = (Uuid::now_v7(), Uuid::now_v7());
        let peer = |name: &str, contact| RemotePeer {
            name: name.into(),
            roots: Vec::new(),
            sender: None,
            generation: CancellationToken::new(),
            contact,
        };
        n.peers.insert(bob, peer("bob", true));
        n.peers.insert(lab, peer("lab", false));
        let by = |a: &str| n.peer_by_name(a).map(|(id, _)| id);
        assert_eq!(by("bob"), Some(bob));
        assert_eq!(by("bob/research"), Some(bob));
        assert_eq!(by("lab"), Some(lab));
        assert_eq!(
            by("lab/research"),
            None,
            "an HTTP peer has no exports to name"
        );
        assert_eq!(by("bob/"), None);
        assert_eq!(by("bob/../x"), None);
        assert_eq!(by("bob/Research"), None);
    }

    /// 183 §7.6, W-25: an exported agent's inbox runs that agent: its identity block leads
    /// the system prompt (not theuth's), the session starts on its model, and the stricter
    /// of its `tools:` ceiling and the contact's cap holds. Another agent's inbox is its own.
    #[tokio::test]
    async fn an_agent_export_inbox_runs_the_exported_agents_persona_model_and_ceiling() {
        struct Roster;
        #[async_trait::async_trait]
        impl crate::IdentitySource for Roster {
            async fn identity(&self, _: &[SessionId]) -> Option<String> {
                None
            }
            async fn inbox(&self, agent: &str) -> Option<crate::InboxPersona> {
                (agent == "research").then(|| crate::InboxPersona {
                    identity: Some(
                        "You are Research (`research`). Your charter:\nfind things".into(),
                    ),
                    model: Some("fake/research-model".into()),
                    tools: Some(tools::Risk::ReadOnly),
                })
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(std::sync::Arc::new(Roster));
        let binding = Uuid::now_v7();
        let inbox = a
            .handle_inbox(ContactInbox {
                binding,
                agent: "research@sky-mac".into(),
                origin: bob("research", tools::Risk::Mutating),
            })
            .await
            .unwrap();
        assert_eq!(
            a.identity_of(inbox).await.as_deref(),
            Some("You are Research (`research`). Your charter:\nfind things")
        );
        let row = a.store.session(inbox).await.unwrap().unwrap();
        assert_eq!(row.model, "fake/research-model");
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::ReadOnly),
            "the agent's ceiling is stricter than the contact's mutating cap"
        );

        let plain = a
            .handle_inbox(ContactInbox {
                binding,
                agent: "theuth".into(),
                origin: bob("ops", tools::Risk::Mutating),
            })
            .await
            .unwrap();
        assert_eq!(a.identity_of(plain).await, None, "theuth's own persona");
        assert_eq!(a.caps.get(&plain), Some(&tools::Risk::Mutating));
    }
}
