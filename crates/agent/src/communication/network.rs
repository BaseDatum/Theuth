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

mod full;
pub use full::{
    A2aReturn, FullWire, InboxPersonasChanged, META, RETURN_ROUTE, THREAD_TTL, is_receipt,
    receipt_metadata,
};

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
    /// 183 §7.6: set when the caller is a contact on `theuth/a2a/1`: its name, machine and
    /// the export.
    /// `None` for an HTTP A2A peer (bearer), which keeps today's admission.
    pub contact: Option<super::ContactOrigin>,
    /// 183 §7.6 (W-27): the contact's first-contact answer for this export is `ask`: a new
    /// task waits in `TASK_STATE_AUTH_REQUIRED` until the owner answers the Attention item.
    pub ask_first: bool,
}

/// 191 (D3, S4): the A2A JSON-RPC error of a contact path aimed at a machine that does not own
/// the session: `{"code": -32030, "message": "not owner"}`. It collides with no code of
/// theuth (-32000..-32014, -32020) or of A2A (-32001..-32009). The contact's client tries its
/// next roster member on it, each once.
pub const NOT_OWNER_CODE: i32 = -32030;
/// [`NOT_OWNER_CODE`]'s message.
pub const NOT_OWNER_TEXT: &str = "not owner";
/// 191 (D3, P3): the `@type` of the detail a [`NOT_OWNER_CODE`] error carries when the owner is
/// another machine of the mesh: `{"@type": OWNER_DETAIL, "owner": <engine id>}`. The receiving
/// machine forwards to that owner (`a2a_forward`) and never passes the detail to the contact.
pub const OWNER_DETAIL: &str = "theuth.io/a2a/owner/v1";

/// 191 (D3): a new A2A task id that names the machine admitting it, `<first 16 hex of its
/// engine id>-<uuidv7>`, so `GetTask` and `CancelTask` reach it from any machine of the mesh. A
/// lone engine (no mesh id) mints a bare UUIDv7, as before 191.
pub fn task_id(engine: &str) -> String {
    match engine.get(..16) {
        Some(prefix) => format!("{prefix}-{}", new_task_id()),
        None => new_task_id(),
    }
}

/// `ask` → `Result<(), A2AError>` (191 D3, S4): the a2a router's strict session resolve for a
/// contact's request, before any task exists: `Err` ([`NOT_OWNER_CODE`]) unless the session's
/// row exists here and this engine owns it.
#[derive(Clone, Copy, Debug)]
pub struct ContactOwned(pub SessionId);

impl Message<ContactOwned> for Agents {
    type Reply = Result<(), A2AError>;

    async fn handle(&mut self, m: ContactOwned, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.contact_owned(m.0).await
    }
}

/// Tests (`test-support`): `session`'s row now names `owner` (another machine took it), as the
/// session log would apply it, so a dependent crate's test reaches a non-owner.
#[cfg(feature = "test-support")]
#[derive(Clone, Debug)]
pub struct OwnedElsewhereForTest(pub SessionId, pub String);

#[cfg(feature = "test-support")]
impl Message<OwnedElsewhereForTest> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: OwnedElsewhereForTest, _: &mut Context<Self, ()>) {
        self.store.owned_elsewhere(m.0, &m.1).await;
    }
}

/// `ask` → `Result<SessionId, A2AError>` (183 §7.6): the capped inbox of (contact
/// `binding`, `origin.export`), an agent export: created on the first send (an isolated
/// root titled `<contact> · <export>`, recorded in `a2a_contexts`), reused after, and its
/// limits installed on every resolve so they hold after a restart.
#[derive(Clone, Debug)]
pub struct ContactInbox {
    /// The contact's live binding.
    pub binding: Uuid,
    /// The exported agent (`<agent>@<engine>` or `theuth`).
    pub agent: String,
    /// Who asks and the export.
    pub origin: super::ContactOrigin,
    /// 191 (D4): the export's replicated id, for the session's `contact_inbox` marker.
    pub export_id: Option<String>,
    /// 191 (D4): the export's home (`a2a_exports.home`), the only machine that opens a new
    /// inbox.
    pub home: InboxHome,
    /// 191 (S4): a send (`SendMessage`, the streaming send); else a read (a task's get, list,
    /// cancel or subscribe), which never opens an inbox.
    pub send: bool,
}

/// 191 (D4): an agent export's home as the a2a plugin staged it from `a2a_exports.home`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboxHome {
    /// This machine.
    Here,
    /// Another live machine of the mesh: its engine id.
    At(String),
    /// It left the mesh (or the row names none): no new inbox.
    Left,
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
    /// 186 (D5): a full contact share's message: its metadata and references.
    pub full: Option<FullWire>,
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

/// 191 (D2): a contact record's `peer_bindings.endpoint` is this prefix plus its record id,
/// constant for the record, so an endpoint-set change never revokes its binding.
pub const CONTACT_PREFIX: &str = "contact:";

/// 191 (D2): the binding endpoint of the contact record `id`.
pub fn contact_record_endpoint(id: &str) -> String {
    format!("{CONTACT_PREFIX}{id}")
}

/// Whether a binding endpoint is a contact's (a 191 record, or a 183 pinned endpoint).
pub fn is_contact_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with(CONTACT_PREFIX) || contact_eid(endpoint).is_some()
}

/// 191 (D2, Sky 17:59 option b): contacts are admitted only once the mesh's bounded latch is
/// done and the a2a plugin's first contact reconcile has finished on the post-latch tables
/// (revokes, inbox caps, closes). Starts false; the a2a plugin sets it. Until then every
/// contact inbox starts capped and no capped inbox is lifted, whatever its local row says.
pub static CONTACTS_READY: std::sync::LazyLock<Arc<std::sync::atomic::AtomicBool>> =
    std::sync::LazyLock::new(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));

/// 191: what a send to an unknown peer reads while [`CONTACTS_READY`] is false.
pub const CONTACTS_WAITING: &str = "waiting for your other machines";

/// 183 §7.7 (F1): what one export a contact offers us is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OfferKind {
    /// One of the contact's chats: our messages arrive there as a mesh peer's.
    Chat,
    /// One of the contact's agents: its inbox for us.
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
    /// 186: the mode the contact granted us this export in.
    pub mode: super::ShareMode,
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
    /// 191 (D2): a contact record's replicated binding id, taken instead of minted (a
    /// revoked one fails the commit: the reconcile quarantines it first).
    pub binding: Option<Uuid>,
    /// Granted local roots for outbound use.
    pub roots: Vec<SessionId>,
    /// The validated SDK client; `None` for an inbound-only peer.
    pub sender: Option<Arc<dyn PeerSender>>,
    /// Route generation, cancelled by the plugin on the next replacement.
    pub generation: CancellationToken,
    /// 186 (D1): our exports shared with this contact in full (its answers may come back
    /// over the return route of each one's conversations).
    pub full: Vec<String>,
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
    /// 191 (D2): bindings of contact records quarantined for replication lag or a convergence
    /// in flight: no route, their inboxes capped, but never revoked (revocation is for good).
    pub hold: Vec<Uuid>,
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
    /// 186: our exports shared with this contact in full.
    full: Vec<String>,
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
    /// 186 (D8): the owner's Stop ended it (the contact reads so, not a bare "canceled").
    owner_stop: bool,
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
    /// 186 (D5): an answer over the contact's return route: acknowledged at once.
    ret: bool,
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
    /// 183 §7.6: inbox sessions resolved this run.
    inboxes: HashSet<SessionId>,
    /// 183 §7.6: request scope → the contact behind it (its first-contact hold); gone with the
    /// scope.
    contact_scopes: HashMap<Uuid, super::ContactOrigin>,
    /// 183 W-27, F11: tasks waiting for a first-contact answer, by (contact, export), oldest
    /// first, with the admission each waits under. A held task waits aside: it is neither the
    /// session's active task nor the head of its queue.
    held: HashMap<(String, String), Vec<HeldTask>>,
    /// 183 F1: each contact's latest offers (alias → exports it grants us), from its card.
    offers: BTreeMap<String, Vec<ContactOffer>>,
    /// 183 F1: when `discover_peers` last asked for a refresh (unix ms; 0 = never).
    offers_asked_ms: std::sync::atomic::AtomicU64,
    /// 183 §7.6, W-25: inbox session → the exported agent's persona, recorded on every
    /// resolve (so after a restart too, before the contact's turn runs).
    inbox_personas: HashMap<SessionId, crate::supervisor::InboxPersona>,
    /// 183 Z3-R2: inbox session → the limits installed (190: only ever the fail-closed
    /// ones), re-installed at every actor start (fail-closed when absent).
    inbox_limits: HashMap<SessionId, (tools::Risk, Vec<String>)>,
    /// 183 Z3-R2: the paired-machine count `mesh_default` last saw (re-run when contacts change).
    last_paired: Option<usize>,
    /// 186 (D4): a full contact's message id → its task, completed when the message is
    /// consumed.
    full_inbound: HashMap<Uuid, TaskRef>,
}

/// 186 (D8): what a contact reads when the owner stopped the turn its message opened.
pub const STOPPED_BY_OWNER: &str = "stopped by the owner";

/// 183 F5: a held first message fails after this long without the owner's answer.
pub const HOLD_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 183 F5: what a held task fails with when [`HOLD_TTL`] passes.
pub const HOLD_EXPIRED: &str = "the owner did not answer";

/// 183 F4: what a held task fails with when its export was taken away or redefined (another
/// chat or agent, other tools, another cap) while it waited.
pub const HOLD_CHANGED: &str = "the shared chat or agent changed while this waited; send it again";

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
                full: Vec::new(),
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
        let inbox = self.inboxes.contains(&root);
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

    /// 183 F8: a record shows a remote peer by its alias (`peer:sky`), not by the binding
    /// id the store keys it with (`peer:01a1…`); unknown or revoked bindings stay as stored.
    pub(crate) fn name_peers(&self, r: &mut PeerRecord) {
        let name = |key: &str| {
            let id = key.strip_prefix("peer:")?.parse::<Uuid>().ok()?;
            Some(format!("peer:{}", self.peers.get(&id)?.name))
        };
        if let Some(n) = name(&r.from) {
            r.from = n;
        }
        if let Some(n) = name(&r.receipt.to) {
            r.receipt.to = n;
        }
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
    /// opening a private turn. Each row goes when its producer settles it. `true` when a
    /// held first message was dropped (183 review 1: the caller announces it), or a contact's
    /// queued message (186 review 1: its waiting row goes).
    #[must_use]
    fn retire(&mut self, scope: Uuid) -> bool {
        let contact = self.contact_scopes.remove(&scope).is_some();
        let mut dropped = false;
        for tasks in self.held.values_mut() {
            if let Some(i) = tasks.iter().position(|h| h.task.request_scope == scope) {
                tasks.remove(i).watch.abort();
                dropped = true;
            }
        }
        self.held.retain(|_, v| !v.is_empty());
        if let Some(q) = self.queued.remove(&scope) {
            q.watch.abort();
            self.feeds.remove(&q.task);
            dropped |= contact;
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
        dropped
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
        // 191 (D3, S4): a contact's message into a session that is not strictly this engine's
        // (an unknown row, an empty owner, another machine's) is `NOT_OWNER` before anything is
        // accepted here: the contact's client tries its next member.
        if p.contact.is_some() {
            self.contact_owned(p.session).await?;
        }
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
                })
                // 186 (D9): and, in full, its messages not yet consumed.
                + self
                    .network
                    .full_inbound
                    .values()
                    .filter(|r| r.principal == key && r.export == p.export)
                    .count();
            if waiting >= CONTACT_QUEUED {
                return Err(A2AError::unsupported_operation(format!(
                    "busy: {waiting} tasks already wait here; retry later"
                )));
            }
        }
        // 186 (D5): a contact writes in the conversation its engine minted (190: every share
        // is full).
        let full = p.contact.is_some();
        // New task in an owned (or freshly minted) context.
        let context_id = match message.context_id.clone() {
            Some(ctx) => {
                let known = self
                    .store
                    .a2a_context(scope, &ctx)
                    .await
                    .map_err(internal)?;
                if known.is_none() {
                    if !full || ctx.is_empty() || ctx.len() > EXTERNAL_ID_MAX {
                        return Err(A2AError::invalid_params("unknown contextId"));
                    }
                    self.store
                        .create_a2a_context(scope, &ctx, p.session)
                        .await
                        .map_err(internal)?;
                }
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
        // 186 (D5): in full, `metadata.theuth.re` names the message of ours this one answers.
        let re = full
            .then(|| full::FullWire::of(req.message.metadata.as_ref()).re)
            .flatten();
        let reply_to = self.answered(re.as_deref(), &recipient, &key).await?;
        let minted = message.context_id.is_none();
        let id = task_id(self.store.engine());
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
            reply_to,
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
            // 186: the row's provenance (190: every contact's message is `full`).
            if let Err(e) = self
                .store
                .set_exchange_contact(receipt.id, super::ShareMode::Full, None)
                .await
            {
                tracing::warn!(exchange = %receipt.id, "a2a: contact mode not recorded: {e:#}");
            }
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
        // 186 (D4): a contact's message takes no slot: it goes in as a mesh peer's at once.
        // `deliver_full`'s owner predicate decides where it lands (191 D3).
        if self
            .network
            .contact_scopes
            .contains_key(&task.request_scope)
        {
            return self.deliver_full(task, generation).await;
        }
        match self.store.active_a2a_task(task.session).await {
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
                owner_stop: false,
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
            .transition_with(&task, snap, A2aTaskStatus::Working, None, None, None, false)
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
        self.transition_with(task, snap, status, message, artifact, error, false)
            .await
    }

    /// [`Self::transition`] over a caller-edited snapshot. Order on the feed:
    /// artifact, then status; a terminal status retires the scope. 183 F11: `held` parks
    /// the task in `AUTH_REQUIRED` aside (it holds no scope; the store's one-active-task rule
    /// skips it); any later transition takes it out.
    #[allow(clippy::too_many_arguments)]
    async fn transition_with(
        &mut self,
        task: &A2aTask,
        mut snap: Task,
        status: A2aTaskStatus,
        message: Option<&str>,
        artifact: Option<&str>,
        error: Option<&str>,
        held: bool,
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
            // 186 (D4): a full contact's receipt rides the event too (followers fold it).
            metadata: snap.metadata.clone(),
        }));
        let scope = A2aScope {
            principal: &task.principal,
            export: &task.export,
        };
        let updated = if held {
            self.store
                .hold_a2a_task(
                    scope,
                    &task.id,
                    task.revision,
                    &serde_json::to_value(&snap)?,
                    status,
                )
                .await?
        } else {
            self.store
                .update_a2a_task(
                    scope,
                    &task.id,
                    task.revision,
                    status,
                    &serde_json::to_value(&snap)?,
                    error,
                )
                .await?
        };
        for ev in events {
            self.network.emit(&task.id, ev);
        }
        // 183 review 1: a hold that ends here (a cancel, a revoke) leaves no stale strip,
        // row or Attention item.
        if status.is_terminal() && self.network.retire(task.request_scope) {
            self.rt.publish(ContactsHeldChanged).await;
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
            // 186 review 1: a contact's waiting row goes as its message starts.
            if admitted.is_some()
                && self
                    .network
                    .contact_scopes
                    .contains_key(&next.request_scope)
            {
                self.rt.publish(ContactsHeldChanged).await;
            }
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
            let why = if !task.cancel_requested && sc.owner_stop {
                STOPPED_BY_OWNER
            } else {
                "canceled"
            };
            (A2aTaskStatus::Canceled, Some(why.into()), None, None)
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
        let Some(scope) = m.scope.filter(|s| self.network.scopes.contains_key(s)) else {
            return;
        };
        if self.network.scopes[&scope].session == m.session {
            let text = self.redact(&m.text).into_owned();
            let sc = self.network.scopes.get_mut(&scope).expect("checked above");
            match m.stop_reason.as_str() {
                // 186 (D8): the owner's Stop; a talk cut-in (`interrupted`) or the runtime cap
                // is no Stop of the owner's.
                "cancelled" => {
                    sc.interrupted = true;
                    sc.owner_stop = true;
                }
                "timed_out" | "interrupted" => sc.interrupted = true,
                "error" => sc.result = Some(Err("turn stopped by error".into())),
                _ => sc.result = Some(Ok(text)),
            }
        }
        self.network_settle_work(scope, &format!("turn:{}", m.turn))
            .await;
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
        // 183 review 1: a first message held for the owner never started either; canceled at
        // once (its retire drops the hold, so a later Allow finds nothing to run).
        let held = self
            .network
            .held
            .values()
            .flatten()
            .any(|h| h.task.request_scope == cur.request_scope);
        if cur.status == A2aTaskStatus::Submitted || held {
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
                full: Vec::new(),
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
        let bind: Vec<(&str, &str, Option<Uuid>)> = m
            .routes
            .iter()
            .map(|r| (r.alias.as_str(), r.endpoint.as_str(), r.binding))
            .collect();
        let revoke: Vec<&str> = m.revoke.iter().map(String::as_str).collect();
        // 191: a held binding steps out of its alias first, so the record that keeps the
        // alias (a rename, a merge in flight) never revokes it.
        self.store
            .hold_peer_bindings(&m.hold)
            .await
            .map_err(|e| e.to_string())?;
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
                // 186 review 2: blocked, unpaired or re-pinned: its inboxes stop running in
                // full and are capped now.
                if p.contact {
                    self.cap_inboxes_of(id).await;
                }
            }
        }
        // 191: a held binding stays live, unrouted, its inboxes capped.
        let mut keep_live = keep.clone();
        keep_live.extend(m.hold.iter().copied());
        self.drop_unrouted(&keep_live).await;
        for id in &m.hold {
            self.cap_inboxes_of(*id).await;
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
                    contact: is_contact_endpoint(&route.endpoint),
                    generation: route.generation,
                    full: route.full,
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

/// `ask` → `Result<Records, String>` (191 D2): the replicated contact records, read in one
/// snapshot, for the a2a reconcile.
#[derive(Clone, Copy, Debug)]
pub struct ContactRecords;

impl Message<ContactRecords> for Agents {
    type Reply = Result<crate::store::contacts::Records, String>;

    async fn handle(
        &mut self,
        _: ContactRecords,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .contact_records()
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<(), String>` (191 D2): write contact record columns in one transaction.
#[derive(Clone, Debug)]
pub struct WriteContactRecords(pub Vec<crate::store::contacts::RecordWrite>);

impl Message<WriteContactRecords> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: WriteContactRecords,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .write_contact_records(&m.0)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<u64, String>` (191 D2 import): mark this machine's local inboxes in their
/// session rows; `exports` maps export names to their replicated ids.
#[derive(Clone, Debug)]
pub struct BackfillContactInboxes(pub BTreeMap<String, String>);

impl Message<BackfillContactInboxes> for Agents {
    type Reply = Result<u64, String>;

    async fn handle(
        &mut self,
        m: BackfillContactInboxes,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .backfill_contact_inboxes(&m.0)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<(), String>` (191 D2): this machine's last direct addresses of `eid`.
#[derive(Clone, Debug)]
pub struct SetContactAddrs {
    /// The contact's endpoint id.
    pub eid: String,
    /// Socket addresses as text.
    pub addrs: Vec<String>,
}

impl Message<SetContactAddrs> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetContactAddrs,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .set_contact_addrs(&m.eid, &m.addrs)
            .await
            .map_err(|e| format!("{e:#}"))
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
        // 191 (D7): never admitted before `contacts_ready` (the gate refuses first).
        if !self.contacts_ready.load(Ordering::Relaxed) {
            return Err(A2AError::unsupported_operation(format!(
                "export unavailable: {CONTACTS_WAITING}"
            )));
        }
        let key = format!("peer:{}", m.binding);
        let scope = A2aScope {
            principal: &key,
            export: &m.origin.export,
        };
        // 191 (S4, P2 review SHOULD 1): the strict owner check, before any side effect (an
        // inbox opened, its row marked, its limits installed): an inbox here whose session is
        // not strictly this engine's (moved, unknown, unreadable) is `not owner` to a send (191
        // P3: naming its owner when that is another machine, which the receiving machine
        // forwards to) and TASK_NOT_FOUND to a read, as an unknown task is; a read never opens
        // or adopts an inbox. P3 (D4): the resolve is [`Agents::inbox_of`] (the marker, then
        // the local row, then the marker's session, then the home).
        let read_refused = || A2AError::task_not_found("task");
        let inbox = match self.inbox_of(&m, scope).await {
            Ok(Inbox::Here(id)) => Inbox::Here(id),
            Ok(_) | Err(_) if !m.send => return Err(read_refused()),
            Ok(other) => other,
            Err(e) => return Err(e),
        };
        // W-25: the exported agent's persona, charter, model and ceiling (`<agent>@<engine>`
        // names the agent of this engine).
        let bare = m.agent.split('@').next().unwrap_or(&m.agent);
        let live = self.live_persona(bare).await;
        // 186 review 2: a share (190: every share is full) runs under the agent's own ceiling,
        // read live: with no persona for an agent (the roster not joined yet, its file gone or
        // invalid) it does not run. theuth's own inbox, before the roster joins, keeps its
        // stored ceiling.
        if match live {
            full::Live::Known(_) => false,
            full::Live::Unknown => bare != "theuth",
            full::Live::Missing => true,
        } {
            // 186 review 3: the contact hears its export's name, never the roster agent's.
            tracing::warn!(
                agent = bare,
                export = %m.origin.export,
                "a2a: full share refused: the agent has no persona (roster not joined, file missing or invalid)"
            );
            return Err(A2AError::unsupported_operation(format!(
                "export unavailable: {} is not available",
                m.origin.export
            )));
        }
        let persona = match &live {
            full::Live::Known(p) => Some(p.clone()),
            _ => None,
        };
        let id = match inbox {
            Inbox::Here(id) => id,
            // 191 (D4): a marked inbox owned here with no local row (it moved here): the
            // local row is the whole re-admission; the inbox runs full from here on.
            Inbox::Adopt(id) => {
                if persona.is_none() {
                    tracing::warn!(session = %id, agent = bare, "191: inbox not adopted: the agent has no live persona");
                    return Err(A2AError::unsupported_operation(format!(
                        "export unavailable: {} is not available",
                        m.origin.export
                    )));
                }
                self.store
                    .create_a2a_context(scope, &new_context_id(), id)
                    .await
                    .map_err(internal)?;
                tracing::info!(session = %id, contact = %m.origin.contact, export = %m.origin.export, "191: contact inbox adopted by its marker");
                id
            }
            Inbox::Mint => {
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
                self.network.inboxes.insert(id);
                id
            }
        };
        self.network.inboxes.insert(id);
        // 191 (D4): the marker travels with the session row (set on each admission, so an
        // inbox opened before 191 is marked too; a no-op once set).
        if let Some(export) = &m.export_id
            && let Err(e) = self
                .store
                .set_contact_inbox(id, &format!("{key}|{export}"))
                .await
        {
            tracing::warn!(session = %id, "a2a: inbox marker: {e:#}");
        }
        // Z3-R2: every later actor start of this session re-installs its limits (an inbox
        // opened by an older build is marked on its first resolve); Z3-R3: and a resumed turn
        // speaks as the agent again, though the roster's source joins after the boot resume.
        // 186 (D6): the inbox runs under the agent's own policy: its row keeps the agent and
        // the ceiling of its real persona, so an actor start re-reads the live one and takes
        // the stricter; it is never capped as a contact's.
        let real = persona.as_ref().map(|p| p.tools);
        self.store
            .mark_a2a_inbox(
                id,
                persona.as_ref().and_then(|p| p.identity.as_deref()),
                bare,
                true,
                real,
            )
            .await
            .map_err(internal)?;
        match persona {
            Some(p) => self.network.inbox_personas.insert(id, p),
            None => self.network.inbox_personas.remove(&id),
        };
        // A live persona's ceiling as is; theuth before the roster joins: its stored one.
        let run = match real {
            Some(c) => Some(c),
            None => self
                .store
                .a2a_inbox_full(id)
                .await
                .map_err(internal)?
                .and_then(|r| r.ceiling),
        };
        self.run_full_inbox(id, run)
            .await
            .map_err(|e| internal(anyhow::anyhow!(e)))?;
        Ok(id)
    }
}

/// 191 (D4): where a contact's inbox for an agent export is, on this machine's view.
enum Inbox {
    /// Its local row names a session this machine owns.
    Here(SessionId),
    /// No local row; the one session carrying its marker is owned here: adopt it.
    Adopt(SessionId),
    /// None anywhere, and this machine is the export's home: open one.
    Mint,
}

impl Agents {
    /// 191 (D4): [`Inbox`] for `m`, or the refusal. Two or more sessions carrying the marker
    /// are refused (`export unavailable`), never chosen between, and capped here. Then the
    /// strict owner check of the local row (P2 review, before any side effect): owned here is
    /// the inbox; owned elsewhere is `NOT_OWNER` naming its owner (the inbox moved; the
    /// receiving machine forwards there); an empty owner (a mesh engine's unknown) is neither
    /// here nor elsewhere: refused. Then the marker (a name-valued marker never matches: the
    /// lookup is by export id): owned here is adopted, elsewhere is `NOT_OWNER` naming the
    /// owner. With none, only the export's home opens an inbox; any other machine answers
    /// `NOT_OWNER` naming the home.
    async fn inbox_of(&mut self, m: &ContactInbox, scope: A2aScope<'_>) -> Result<Inbox, A2AError> {
        let me = self.store.engine().to_owned();
        let marker = m
            .export_id
            .as_ref()
            .map(|x| format!("{}|{x}", scope.principal));
        let marked = match &marker {
            Some(k) => self.store.marked_inboxes(k).await.map_err(internal)?,
            None => Vec::new(),
        };
        if marked.len() > 1 {
            tracing::warn!(sessions = ?marked.iter().map(|(s, _)| s.to_string()).collect::<Vec<_>>(), export = %m.origin.export, "191: two sessions carry one inbox marker; refused, never chosen between");
            // A read refuses too, but writes nothing (no unmark, no cap): only a send does.
            for (s, _) in marked.iter().filter(|(_, o)| m.send && *o == me) {
                if let Err(e) = self.store.unmark_a2a_inbox_full(*s).await {
                    tracing::error!(session = %s, "a2a: inbox mode not reset: {e:#}");
                }
                if let Err(e) = self
                    .install_contact_limits(*s, INBOX_FAIL_CLOSED, Vec::new())
                    .await
                {
                    tracing::error!(session = %s, "{e}");
                }
            }
            return Err(A2AError::unsupported_operation(format!(
                "export unavailable: {} has more than one inbox; delete the extra one",
                m.origin.export
            )));
        }
        let rows = self
            .store
            .inbox_rows(scope.principal, scope.export)
            .await
            .map_err(internal)?;
        if let Some((s, owner, _)) = rows.into_iter().next() {
            return if owner == me {
                Ok(Inbox::Here(s))
            } else if owner.is_empty() {
                tracing::info!(session = %s, "191: contact inbox refused: its owner is unknown here");
                Err(full::not_owner())
            } else {
                Err(full::not_owner_at(&owner))
            };
        }
        if let Some((s, owner)) = marked.first() {
            return if *owner == me {
                Ok(Inbox::Adopt(*s))
            } else if owner.is_empty() {
                Err(full::not_owner())
            } else {
                Err(full::not_owner_at(owner))
            };
        }
        match &m.home {
            InboxHome::Here => Ok(Inbox::Mint),
            InboxHome::At(h) => Err(full::not_owner_at(h)),
            InboxHome::Left => Err(A2AError::unsupported_operation(format!(
                "export unavailable: {}: its machine left the mesh",
                m.origin.export
            ))),
        }
    }
}

/// 183 Z3-R2: the default-deny limits of a contact inbox that cannot prove its agent's
/// ceiling or a live binding (an actor start before the contact's next admission, a child
/// result waking it): read-only, and nothing but the reply.
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
        self.network.inboxes.insert(id);
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

    /// 183 §7.6, Z3-R2, 186 (D6): at every actor start of a session, before anything of it
    /// runs: a contact's inbox runs under its agent's ceiling when its row is marked full,
    /// its binding is live and the ceiling is known; otherwise it is capped fail-closed. A
    /// store error is treated as an inbox (fail closed); limits not installed refuse the
    /// start.
    pub(crate) async fn reinstall_contact_limits(&mut self, id: SessionId) -> Result<(), String> {
        match self.store.a2a_inbox_identity(id).await {
            Ok(Some(_)) => {}
            // 191 (D4): an inbox marked in the session row but with no local context here (a
            // Write take from an owner that could not release it) runs capped until the
            // owner re-admits it.
            Ok(None) => match self.store.contact_inbox(id).await {
                Ok(None) => return Ok(()),
                Ok(Some(_)) => {
                    return self
                        .install_contact_limits(id, INBOX_FAIL_CLOSED, Vec::new())
                        .await;
                }
                Err(e) => {
                    tracing::error!(session = %id, "a2a: inbox marker lookup failed, capping it: {e:#}");
                    return self
                        .install_contact_limits(id, INBOX_FAIL_CLOSED, Vec::new())
                        .await;
                }
            },
            Err(e) => tracing::error!(session = %id, "a2a: inbox lookup failed, capping it: {e:#}"),
        }
        // 191 (D7): before `contacts_ready` a local live binding proves nothing (a removal made
        // elsewhere may not have synced): every inbox starts capped, its row left marked full
        // so `refresh_full_inboxes` lifts it after the flip if it is still due.
        if !self.contacts_ready.load(Ordering::Relaxed) {
            tracing::info!(session = %id, "191: contact inbox starts capped (contacts not ready)");
            return self
                .install_contact_limits(id, INBOX_FAIL_CLOSED, Vec::new())
                .await;
        }
        // 186 (D6), review 2: an inbox shared in full keeps the agent's own policy: the
        // stricter of its row's ceiling and the roster's live one (the persona is not in memory
        // at an actor start). Its contact unpaired or blocked (binding revoked), its agent
        // gone, or no ceiling known: capped as a contact's. Unsure caps it.
        match self.store.a2a_inbox_full(id).await {
            Ok(Some(row)) => {
                let paired = match row
                    .principal
                    .strip_prefix("peer:")
                    .and_then(|b| b.parse::<Uuid>().ok())
                {
                    Some(b) => matches!(self.store.peer_binding_by_id(b).await, Ok(Some(_))),
                    None => false,
                };
                if paired {
                    let live = match row.agent.as_deref() {
                        Some(agent) => self.live_persona(agent).await,
                        None => full::Live::Unknown,
                    };
                    return self
                        .run_full_inbox(id, full::full_ceiling(&live, row.ceiling))
                        .await;
                }
                if let Err(e) = self.store.unmark_a2a_inbox_full(id).await {
                    tracing::error!(session = %id, "a2a: inbox mode not reset: {e:#}");
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(session = %id, "a2a: inbox mode lookup failed, capping it: {e:#}")
            }
        }
        let (cap, tools) = self
            .network
            .inbox_limits
            .get(&id)
            .cloned()
            .unwrap_or((INBOX_FAIL_CLOSED, Vec::new()));
        self.install_contact_limits(id, cap, tools).await
    }

    /// 186 (D6): a contact's inbox that may run uncapped: no contact limits (allowlist,
    /// contact cap), only the exported agent's own `tools:` ceiling, as for its owner's
    /// sessions.
    pub(crate) async fn clear_contact_limits(
        &mut self,
        id: SessionId,
        ceiling: Option<tools::Risk>,
    ) -> Result<(), String> {
        match ceiling {
            Some(c) => self.caps.insert(id, c),
            None => self.caps.remove(&id),
        };
        self.network.inbox_limits.remove(&id);
        self.network.inboxes.insert(id);
        let Some(reg) =
            kameo::actor::ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
                .map_err(|e| format!("a2a: inbox limits: tool registry: {e}"))?
        else {
            return Ok(());
        };
        reg.ask(tools::SetAllowOnly {
            session: id,
            tools: None,
            turn: false,
        })
        .await
        .map_err(|e| format!("a2a: inbox allowlist: {e}"))?;
        reg.ask(tools::SetCap {
            session: id,
            cap: ceiling,
        })
        .await
        .map_err(|e| format!("a2a: inbox cap: {e}"))
    }
}

/// `ask` → `()` (183 §7.6, 186 review 2): `peers.yaml` reloaded: contact `contact`'s inboxes
/// whose export it no longer holds stop running uncapped at once, a running turn included.
#[derive(Clone, Debug)]
pub struct ContactLimits {
    /// The contact's alias.
    pub contact: String,
    /// The agent exports granted to it (190: every one in full); their inboxes keep running
    /// under the agent's ceiling, every other inbox of this contact is capped.
    pub full: Vec<String>,
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
        // 186 review 2: an inbox whose export is no longer granted (unshared, deleted) stops
        // running uncapped now and is capped as a contact's inbox that proves no ceiling.
        match self.store.a2a_inboxes_of(&key).await {
            Ok(rows) => {
                for (export, inbox, full) in rows {
                    if m.full.contains(&export) {
                        continue;
                    }
                    if full && let Err(e) = self.store.unmark_a2a_inbox_full(inbox).await {
                        tracing::error!(session = %inbox, "a2a: inbox mode not reset: {e:#}");
                    }
                    if let Err(e) = self
                        .install_contact_limits(inbox, INBOX_FAIL_CLOSED, Vec::new())
                        .await
                    {
                        tracing::error!(session = %inbox, "{e}");
                    }
                }
            }
            Err(e) => tracing::error!(contact = %m.contact, "a2a: inboxes: {e:#}"),
        }
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
        let expired = held_wait(&route, ttl).await;
        crate::continuation::notify(HeldEnded { scope, expired }).await;
    })
    .abort_handle()
}

/// [`held_watch`]'s wait: `true` when `ttl` passed first, `false` when `route` ended.
async fn held_wait(route: &CancellationToken, ttl: Duration) -> bool {
    tokio::select! {
        () = route.cancelled() => false,
        () = tokio::time::sleep(ttl) => true,
    }
}

/// `ask` → `()` (183 F4): `contact`'s grant changed, or its cap (`route` is its epoch
/// now). Its held tasks for an export whose definition is unchanged (`exports`) wait on
/// under `route`; every other one fails at once ([`HOLD_CHANGED`]). Asked right after the
/// commit, before the old epoch is cancelled, so editing a contact (or sharing another export
/// with it) never drops a waiting first message, and never lets one run under authority that
/// is gone.
#[derive(Clone, Debug)]
pub struct RebindHeld {
    /// The contact's alias.
    pub contact: String,
    /// The exports it holds in both epochs with an unchanged definition and cap.
    pub exports: Vec<String>,
    /// Its new grant epoch.
    pub route: CancellationToken,
}

impl Message<RebindHeld> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: RebindHeld, _: &mut Context<Self, Self::Reply>) {
        self.rebind_held(m).await;
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
        let preview = preview_of(&task);
        let held = match self
            .transition_with(
                &task,
                snapshot(&task),
                A2aTaskStatus::AuthRequired,
                Some(&why),
                None,
                None,
                true,
            )
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
    async fn rebind_held(&mut self, m: RebindHeld) {
        let now = Utc::now().timestamp_millis().max(0) as u64;
        let changed: Vec<(String, String)> = self
            .network
            .held
            .keys()
            .filter(|(c, e)| *c == m.contact && !m.exports.contains(e))
            .cloned()
            .collect();
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
        // Taken away or redefined (another chat, other tools, another cap): admitted under
        // authority that is gone, so it fails now rather than run on a later Allow.
        for key in &changed {
            for h in self.network.held.remove(key).unwrap_or_default() {
                h.watch.abort();
                if let Err(e) = self.fail_task(&h.task, HOLD_CHANGED).await {
                    tracing::warn!(task = %h.task.id, "a2a: fail held task: {e:#}");
                }
            }
        }
        if !changed.is_empty() {
            self.rt.publish(ContactsHeldChanged).await;
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

/// `ask` → `bool` (183 review 1): `peer:<name>` (or `<name>/<export>`) is a live contact.
#[derive(Clone, Debug)]
pub struct IsContact(pub String);

impl Message<IsContact> for Agents {
    type Reply = bool;

    async fn handle(&mut self, m: IsContact, _: &mut Context<Self, Self::Reply>) -> bool {
        self.network.is_contact(&m.0)
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

/// 183 F11: the first line of a task's message, cut to 200 bytes.
fn preview_of(task: &A2aTask) -> String {
    snapshot(task)
        .history
        .as_deref()
        .and_then(|h| h.first())
        .and_then(|m| parts_text(&m.parts))
        .map(|t| cut(t.lines().next().unwrap_or_default().trim(), 200).to_owned())
        .unwrap_or_default()
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
        let ready = self.contacts_ready.load(Ordering::Relaxed);
        let (binding, sender, route, contact) = {
            let (binding, peer) = self.network.peer_by_name(alias).ok_or_else(|| {
                // 191: before the first contact reconcile no route is committed.
                if ready {
                    format!("unknown peer `{alias}`")
                } else {
                    format!("peer `{alias}`: {CONTACTS_WAITING}")
                }
            })?;
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
        let mut orig = None;
        if let Some(r) = reply_to {
            let o = self
                .store
                .exchange_record(r)
                .await
                .map_err(|e| format!("{e:#}"))?
                .ok_or_else(|| "unknown message".to_owned())?;
            if o.from != recipient_key || o.receipt.to != sender_key {
                return Err("reply_to does not name a message this peer sent you".into());
            }
            orig = Some(o);
        }
        // 186 (D5): a contact shared in full: on the conversation, never a task question.
        let full = match contact {
            true => self.full_shape(from, alias, binding, orig.as_ref()).await?,
            false => None,
        };
        if let Some(o) = orig.as_ref().filter(|_| full.is_none()) {
            match o.request_scope.filter(|s| {
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
                None => remote_task = o.receipt.task_id.clone(),
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
                // Z3-R2: a contact's inbox reaches no contact (no relay through it). 186 (D5):
                // it may answer, in full, the contact whose message it holds.
                let answer = full.as_ref().is_some_and(|f| f.ret);
                let granted = (contact && (answer || !self.network.inboxes.contains(&root)))
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
            // 186: an answer names its message while that one is still open; a later
            // answer to the same message travels on the conversation alone.
            reply_to: match &full {
                Some(_) => orig
                    .as_ref()
                    .filter(|o| full::answerable(o.receipt.state))
                    .map(|o| o.receipt.id),
                None => reply_to,
            },
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
        if let Some(f) = &full
            && let Err(e) = self
                .store
                .set_exchange_contact(receipt.id, super::ShareMode::Full, Some(&f.context))
                .await
        {
            tracing::warn!(exchange = %receipt.id, "a2a: conversation not recorded: {e:#}");
        }
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
                ret: full.as_ref().is_some_and(|f| f.ret),
            },
        );
        let export = match &full {
            Some(f) => Some(f.export.clone()),
            // `peer_by_name` matched only a contact for `<name>/<export>`.
            None => alias
                .split_once('/')
                .filter(|_| contact)
                .map(|(_, e)| e.to_owned()),
        };
        let outbound = OutboundPeerMessage {
            binding,
            request_id: request_id.map_or_else(|| receipt.id.to_string(), str::to_owned),
            record,
            task_id: remote_task,
            context_id: full.as_ref().map(|f| f.context.clone()),
            generation,
            export,
            full: full.map(|f| f.wire),
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
        let (ret, alias) = (o.ret, o.alias.clone());
        let r = match outcome {
            Err(RemoteSendError::Refused(why)) => {
                self.network.outbound.remove(&exchange);
                let why = self.redact(&why).into_owned();
                // 186 (D10): an older contact has no return route.
                let why = match ret {
                    true if why.contains("grants no export") => format!(
                        "update needed: {alias}'s theuth cannot take answers in full; update it"
                    ),
                    true if why.contains("TASK_NOT_FOUND") => format!(
                        "the conversation with {alias} ended (no longer shared in full, or quiet for 7 days)"
                    ),
                    _ => why,
                };
                self.store.fail_exchange(exchange, &why).await
            }
            Err(RemoteSendError::Unknown(why)) => {
                self.network.outbound.remove(&exchange);
                let why = self.redact(&why).into_owned();
                self.store.mark_exchange_unknown(exchange, None, &why).await
            }
            // 186 (D4, D5): received in full — read, not answered.
            Ok(SendMessageResponse::Message(m)) if ret || full::is_receipt(m.metadata.as_ref()) => {
                self.full_read(exchange).await;
                return;
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
            // 186 (D4): a full contact consumed it; its answers come on their own.
            TaskState::Completed if full::task_is_receipt(t) => self.full_read(exchange).await,
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
                if contact
                    && let Some((machine, remote_export)) = super::ContactRefusal::waiting(&text)
                {
                    // 183 review 1: the name and export from our own facts; the holder's
                    // export only when the address named none and it is an export name.
                    let (name, written) = match alias.split_once('/') {
                        Some((n, e)) => (n, Some(e)),
                        None => (alias.as_str(), None),
                    };
                    let export = written.map(str::to_owned).unwrap_or_else(|| {
                        let ok = remote_export.len() <= 64
                            && remote_export.bytes().all(|b| {
                                b.is_ascii_lowercase()
                                    || b.is_ascii_digit()
                                    || b == b'-'
                                    || b == b'_'
                            });
                        if ok {
                            remote_export
                        } else {
                            "its first shared chat".to_owned()
                        }
                    });
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
                full: Vec::new(),
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

    fn bob(export: &str) -> super::super::ContactOrigin {
        super::super::ContactOrigin {
            contact: "bob".into(),
            machine: "Bob's MacBook".into(),
            export: export.into(),
            mode: super::super::ShareMode::Cautious,
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
    /// aside: the session's running task keeps its slot, and (190: every share is full) an
    /// allowed message goes in at once, never behind the held one; Allow once delivers the
    /// held one as any admitted message; Block fails a waiting one as not a contact.
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
            contact: Some(bob("research")),
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
        // F11: an allowed message goes in at once, though one still waits for the owner.
        let allowed = a.admit_send(&p(false), &send("m-b")).await.unwrap().task;
        assert_eq!(allowed.status, A2aTaskStatus::Working, "no slot taken");
        assert_eq!(
            a.store
                .active_a2a_task(session)
                .await
                .unwrap()
                .map(|t| t.id),
            Some(busy.id),
            "the running task keeps its slot"
        );
        let still = a.store.a2a_task(s, &first.id).await.unwrap().unwrap();
        assert_eq!(still.status, A2aTaskStatus::AuthRequired, "it did not run");

        // Allow once: it is delivered as any admitted message.
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
        assert_eq!(
            now.status,
            A2aTaskStatus::Working,
            "delivered, as any admitted message"
        );

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

    /// 183 W-41, §13 row 55, 191: a contact's message into an exported chat that is archived,
    /// takes no agent messages, or is gone is refused at admission (`admit_principal`, which
    /// every chat owned on this machine relies on, whichever machine received the message).
    #[tokio::test]
    async fn a_contacts_message_into_an_archived_silenced_or_deleted_chat_is_refused() {
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
            contact: Some(bob("research")),
            ask_first: false,
        };
        let refused = |r: Result<Admitted, A2AError>| {
            let e = r.err().expect("refused");
            assert_eq!(e.code, a2a_types::error_code::UNSUPPORTED_OPERATION);
            e.message
        };
        a.store.set_archived(session, true).await.unwrap();
        assert_eq!(
            refused(a.admit_send(&p, &send("m-archived")).await),
            super::super::ContactRefusal::Paused {
                reason: PAUSED_ARCHIVED.into()
            }
            .to_string()
        );
        a.store.set_archived(session, false).await.unwrap();
        a.store
            .set_session_communication(session, Some(false))
            .await
            .unwrap();
        assert_eq!(
            refused(a.admit_send(&p, &send("m-silenced")).await),
            super::super::ContactRefusal::NoAgentMessages.to_string()
        );
        a.store
            .set_session_communication(session, None)
            .await
            .unwrap();
        assert!(
            a.admit_send(&p, &send("m-open")).await.is_ok(),
            "admitted once neither holds"
        );
        a.store.delete_session(session).await.unwrap();
        assert_eq!(
            refused(a.admit_send(&p, &send("m-deleted")).await),
            "export unavailable: export session missing"
        );
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
            contact: Some(bob("research")),
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

    /// 183 F5 (review 1): the hold's own timer, on a paused clock: nothing before
    /// `HOLD_TTL`, expiry at it; the grant epoch ending first wins and is not an expiry.
    #[tokio::test(start_paused = true)]
    async fn the_hold_timer_fires_at_the_ttl_and_not_before() {
        let route = CancellationToken::new();
        let timer = tokio::spawn({
            let route = route.clone();
            async move { held_wait(&route, HOLD_TTL).await }
        });
        tokio::time::advance(HOLD_TTL - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(
            !timer.is_finished(),
            "still waiting a second before the TTL"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(timer.await.unwrap(), "expired at the TTL");

        let revoked = tokio::spawn({
            let route = route.clone();
            async move { held_wait(&route, HOLD_TTL).await }
        });
        tokio::time::advance(Duration::from_secs(60)).await;
        route.cancel();
        assert!(
            !revoked.await.unwrap(),
            "the route ended first: not an expiry"
        );
    }

    /// 183 review 1: the contact's own `tasks/cancel` on a held first message cancels it at
    /// once (no wait for work that never started), the surfaces hear the hold ended (review
    /// 2: `retire` → `ContactsHeldChanged`), and a later Allow finds nothing to run.
    #[tokio::test]
    async fn a_contact_cancel_of_a_held_message_ends_it_at_once() {
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
            contact: Some(bob("research")),
            ask_first: true,
        };
        let held = a.admit_send(&p, &send("m-a")).await.unwrap().task;
        assert_eq!(held.status, A2aTaskStatus::AuthRequired);
        #[derive(kameo::Actor)]
        struct HeldTap(tokio::sync::mpsc::UnboundedSender<()>);
        impl Message<ContactsHeldChanged> for HeldTap {
            type Reply = ();
            async fn handle(&mut self, _: ContactsHeldChanged, _: &mut Context<Self, ()>) {
                let _ = self.0.send(());
            }
        }
        let (tx, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let tap = <HeldTap as kameo::actor::Spawn>::spawn_with_mailbox(
            HeldTap(tx),
            runtime::subscriber_mailbox(),
        );
        a.rt.subscribe::<HeldTap, ContactsHeldChanged>(&tap).await;
        let done = a
            .cancel_task(&p, &held.id)
            .await
            .unwrap()
            .expect("settled at once");
        assert_eq!(done.status, A2aTaskStatus::Canceled);
        assert!(a.network.held.is_empty(), "no longer waiting anywhere");
        tokio::time::timeout(Duration::from_secs(10), heard.recv())
            .await
            .expect("ContactsHeldChanged published")
            .unwrap();
        let n = a
            .release_held(ContactRelease {
                contact: "bob".into(),
                export: Some("research".into()),
                allow: true,
            })
            .await;
        assert_eq!(n, 0, "Allow finds nothing to run");
    }

    /// 183 review 1: the held line takes the holder's machine text cut to 64 characters
    /// without control characters, and is at most 512 characters.
    #[test]
    fn held_lines_are_bounded() {
        let machine = format!("33\n3\u{1b}[31m{}", "x".repeat(4096));
        let line = held_line("sky", &machine, "hi-there");
        assert!(line.chars().count() <= 512);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.starts_with("held: sky (333[31mxxx"), "{line}");
        let between = line.split_once('(').unwrap().1.split_once(')').unwrap().0;
        assert_eq!(between.chars().count(), 64);
        assert!(line.contains("has not allowed messages to hi-there yet"));
    }

    /// 183 F1 (review 1): `discover_peers` asks for a refresh of the contacts' offers at most
    /// once per `OFFERS_TTL`, and never without a contact.
    #[tokio::test]
    async fn offers_are_due_once_per_ttl_with_a_contact() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        assert!(!a.network.offers_due(), "no contact, nothing to refresh");
        a.network.insert_contact_for_test("sky");
        assert!(a.network.offers_due(), "the first listing asks");
        assert!(
            !a.network.offers_due(),
            "within the TTL it does not ask again"
        );
        a.network.offers_asked_ms.store(
            (Utc::now().timestamp_millis() as u64)
                .saturating_sub(OFFERS_TTL.as_millis() as u64 + 1),
            Ordering::Relaxed,
        );
        assert!(a.network.offers_due(), "after the TTL it asks again");
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
        let (session, _, _) = exported_task(&mut a, &principal).await;
        let p = A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research")),
            ask_first: false,
        };
        for i in 0..70 {
            let task = a
                .admit_send(&p, &send(&format!("m-{i}")))
                .await
                .unwrap_or_else(|e| panic!("message {i}: {e:?}"))
                .task;
            // Each taken in before the next one (no turn runs here: its receipt completes it).
            a.full_consumed(task.dedup_exchange.unwrap()).await.unwrap();
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
            contact: Some(bob("research")),
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
    /// contact session of its own, so it would carry the owner's tools back to the contact);
    /// an owner root still spawns.
    #[tokio::test]
    async fn a_contact_session_spawns_nothing() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let origin = bob("research");
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin,
                export_id: None,
                home: InboxHome::Here,
                send: true,
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
                full: Vec::new(),
            },
        );
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin: bob("research"),
                export_id: None,
                home: InboxHome::Here,
                send: true,
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
                full: Vec::new(),
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

    /// 190 (D9): to an export an older contact's card still tags `cautious`, a send keeps the
    /// 183 shape (no conversation, no `theuth` metadata), and the turn's final text the
    /// contact's engine completes the task with comes back as the reply; one tagged `full`
    /// goes full.
    #[tokio::test]
    async fn a_cautious_tagged_card_gets_the_183_shape_and_its_final_text_reply() {
        struct Record(tokio::sync::mpsc::UnboundedSender<OutboundPeerMessage>);
        #[async_trait]
        impl PeerSender for Record {
            async fn send(&self, o: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError> {
                let _ = self.0.send(o);
                Err(RemoteSendError::Unknown("recorded".into()))
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("sky", &contact_endpoint("sky-endpoint"))
            .await
            .unwrap();
        let (tx, mut out) = tokio::sync::mpsc::unbounded_channel();
        a.network.peers.insert(
            binding.id,
            RemotePeer {
                name: "sky".into(),
                roots: Vec::new(),
                sender: Some(Arc::new(Record(tx))),
                generation: CancellationToken::new(),
                contact: true,
                full: Vec::new(),
            },
        );
        let offer = |name: &str, mode| ContactOffer {
            name: name.into(),
            title: name.into(),
            kind: OfferKind::Chat,
            state: OfferState::Allowed,
            mode,
        };
        a.network.set_offers(BTreeMap::from([(
            "sky".into(),
            vec![
                offer("old", super::super::ShareMode::Cautious),
                offer("new", super::super::ShareMode::Full),
            ],
        )]));
        let owner = a
            .new_root(None, super::super::PeerScope::Owner, None, None)
            .await
            .unwrap();
        let sent = a
            .network_send(owner, "sky/old", "hello", None, None, false)
            .await
            .unwrap();
        let wire = tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("it leaves")
            .unwrap();
        assert!(
            wire.full.is_none() && wire.context_id.is_none(),
            "the 183 shape"
        );
        let t = Task {
            id: "t-1".into(),
            context_id: "c-1".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: Some(vec![text_artifact("the turn's answer")]),
            history: None,
            metadata: None,
        };
        a.apply_remote_task(sent.id, &t).await;
        let r = a.store.exchange_receipt(sent.id).await.unwrap().unwrap();
        assert_eq!(r.state, PeerMessageState::Replied);
        assert_eq!(r.reply.as_deref(), Some("the turn's answer"));

        a.network_send(owner, "sky/new", "hello", None, None, false)
            .await
            .unwrap();
        let wire = tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("it leaves")
            .unwrap();
        assert!(wire.full.is_some() && wire.context_id.is_some(), "full");
    }

    /// 183 F4: a grant change keeps a held first message whose export is unchanged: it moves
    /// to the new grant epoch, so Allow once runs it after the old epoch ended; one whose
    /// export was taken away or redefined fails at once (`HOLD_CHANGED`), before any Allow.
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
            contact: Some(bob(export)),
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
        })
        .await;
        let other = A2aScope {
            principal: &principal,
            export: "other",
        };
        let now = a.store.a2a_task(other, &dropped.id).await.unwrap().unwrap();
        assert_eq!(now.status, A2aTaskStatus::Failed, "failed at the rebind");
        assert_eq!(now.error.as_deref(), Some(HOLD_CHANGED));
        assert!(!a.network.held.contains_key(&("bob".into(), "other".into())));
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
        let _ = a.network.retire(busy.request_scope);
        a.release_held(ContactRelease {
            contact: "bob".into(),
            export: Some("research".into()),
            allow: true,
        })
        .await;
        let kept = a.store.a2a_task(s, &kept.id).await.unwrap().unwrap();
        assert_ne!(kept.status, A2aTaskStatus::AuthRequired);
        assert_ne!(
            kept.status,
            A2aTaskStatus::Failed,
            "the unchanged export runs under the new epoch: {:?}",
            kept.error
        );
    }

    /// 183 Z3-R2 (M1): an inbox's limits come back at every actor start, before anything of
    /// it runs: after a restart (or a child result waking it) the engine has no admission
    /// yet, so the inbox is read-only with nothing but the reply until the contact writes;
    /// an ordinary chat gets nothing. 190: theuth's inbox with no ceiling known (no roster
    /// yet, none stored) runs capped from its first admission too.
    #[tokio::test]
    async fn an_inbox_is_capped_and_default_deny_at_every_actor_start() {
        let scratch = crate::TestDir::new("z3r2");
        let dir = scratch.0.clone();
        let mut a = crate::supervisor::tests::agents_on(dir.clone(), None, true).await;
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: Uuid::now_v7(),
                agent: "theuth".into(),
                origin: bob("research"),
                export_id: None,
                home: InboxHome::Here,
                send: true,
            })
            .await
            .unwrap();
        let chat = a
            .new_root(None, super::super::PeerScope::Owner, None, None)
            .await
            .unwrap();
        assert_eq!(
            a.network.inbox_limits.get(&inbox),
            Some(&(tools::Risk::ReadOnly, Vec::new()))
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
                origin: bob("research"),
                export_id: None,
                home: InboxHome::Here,
                send: true,
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

    /// 183 §7.6: an agent export's inbox is one isolated root per (contact, export), titled
    /// for both, reused on every resolve.
    #[tokio::test]
    async fn a_contact_inbox_is_one_root_reused_per_contact_and_export() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = Uuid::now_v7();
        let ask = |export: &str| ContactInbox {
            binding,
            agent: "theuth".into(),
            origin: bob(export),
            export_id: None,
            home: InboxHome::Here,
            send: true,
        };
        let first = a.handle_inbox(ask("research")).await.unwrap();
        let row = a.store.session(first).await.unwrap().unwrap();
        assert_eq!(row.title, "bob · research");
        assert_eq!(
            a.communication.0.read().state(Some(first)).unwrap().scope,
            super::super::PeerScope::Isolated,
            "an inbox reaches no other root"
        );
        let again = a.handle_inbox(ask("research")).await.unwrap();
        assert_eq!(again, first, "the same contact and export reuse the inbox");
        let other = a.handle_inbox(ask("ops")).await.unwrap();
        assert_ne!(other, first, "another export, another inbox");
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
            full: Vec::new(),
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
    /// the system prompt (not theuth's), the session starts on its model, and its `tools:`
    /// ceiling holds (190: not the contact's cap). Another agent's inbox is its own.
    #[tokio::test]
    async fn an_agent_export_inbox_runs_the_exported_agents_persona_model_and_ceiling() {
        struct Roster;
        #[async_trait::async_trait]
        impl crate::IdentitySource for Roster {
            async fn identity(&self, _: &[SessionId]) -> Option<String> {
                None
            }
            async fn inbox(&self, agent: &str) -> Option<crate::InboxPersona> {
                match agent {
                    "research" => Some(crate::InboxPersona {
                        identity: Some(
                            "You are Research (`research`). Your charter:\nfind things".into(),
                        ),
                        model: Some("fake/research-model".into()),
                        tools: Some(tools::Risk::ReadOnly),
                    }),
                    "theuth" => Some(crate::InboxPersona {
                        identity: None,
                        model: None,
                        tools: None,
                    }),
                    _ => None,
                }
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(std::sync::Arc::new(Roster));
        let binding = Uuid::now_v7();
        let inbox = a
            .handle_inbox(ContactInbox {
                binding,
                agent: "research@sky-mac".into(),
                origin: bob("research"),
                export_id: None,
                home: InboxHome::Here,
                send: true,
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
            "the agent's ceiling"
        );

        let plain = a
            .handle_inbox(ContactInbox {
                binding,
                agent: "theuth".into(),
                origin: bob("ops"),
                export_id: None,
                home: InboxHome::Here,
                send: true,
            })
            .await
            .unwrap();
        assert_eq!(a.identity_of(plain).await, None, "theuth's own persona");
        assert_eq!(a.caps.get(&plain), None, "theuth has no ceiling");
    }

    fn full_bob(export: &str) -> super::super::ContactOrigin {
        super::super::ContactOrigin {
            mode: super::super::ShareMode::Full,
            ..bob(export)
        }
    }

    /// A full contact's message: `contextId` minted by its sender, `theuth` metadata.
    fn full_send(id: &str, context: &str, wire: &FullWire) -> SendMessageRequest {
        let mut r = send(id);
        r.message.context_id = Some(context.into());
        r.message.metadata = Some(wire.metadata());
        r
    }

    /// 186 (D4): in full, a contact's message takes no slot (the chat keeps serving a cautious
    /// task), is delivered unscoped as a mesh peer's, records its conversation (the sender's
    /// `contextId`), and its task completes with `theuth.receipt` once consumed — no reply.
    /// D9: a chain at `CHAIN_MAX` is refused like a mesh peer's.
    #[tokio::test]
    async fn a_full_contact_message_is_a_peer_message_and_its_task_a_receipt() {
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
            contact: Some(full_bob("research")),
            ask_first: false,
        };
        let wire = FullWire {
            chain: 3,
            ..FullWire::default()
        };
        let task = a
            .admit_send(&p, &full_send("m-full", "bob-ctx", &wire))
            .await
            .unwrap()
            .task;
        assert_eq!(task.status, A2aTaskStatus::Working, "{:?}", task.error);
        assert_eq!(task.context_id, "bob-ctx", "the sender's conversation");
        assert_eq!(
            a.store
                .active_a2a_task(session)
                .await
                .unwrap()
                .map(|t| t.id),
            Some(busy.id),
            "the cautious task keeps the slot"
        );
        let exchange = task.dedup_exchange.unwrap();
        let record = a.store.exchange_record(exchange).await.unwrap().unwrap();
        assert_eq!(record.contact_mode, Some(super::super::ShareMode::Full));
        assert_eq!(
            a.store.exchange_thread(exchange).await.unwrap(),
            Some(("bob-ctx".into(), "m-full".into()))
        );
        let thread = a
            .store
            .contact_thread(binding.id, "bob-ctx")
            .await
            .unwrap()
            .unwrap();
        assert!(!thread.outbound && thread.session == session && thread.export == "research");
        assert_eq!(record.receipt.task_id.as_deref(), Some(task.id.as_str()));

        let done = a.full_consumed(exchange).await.unwrap();
        assert_eq!(done.status, A2aTaskStatus::Completed);
        let snap = snapshot(&done);
        assert!(full::task_is_receipt(&snap), "a receipt");
        assert!(snap.artifacts.is_none(), "no reply in it");

        // D9: a sender at the chain limit is refused.
        let spent = FullWire {
            chain: super::super::CHAIN_MAX,
            ..FullWire::default()
        };
        let refused = a
            .admit_send(&p, &full_send("m-chain", "bob-ctx", &spent))
            .await
            .unwrap()
            .task;
        assert_eq!(refused.status, A2aTaskStatus::Failed);
        assert!(
            refused
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("chain limit"),
            "{:?}",
            refused.error
        );
    }

    /// 186 (D8): a contact's task whose turn the owner stopped reads "stopped by the owner",
    /// not a bare "canceled".
    #[tokio::test]
    async fn an_owner_stop_tells_the_contact_so() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (session, task, _) = exported_task(&mut a, "peer:bob").await;
        let mut stop = finished(session, TurnId::new(), Some(task.request_scope), "");
        stop.stop_reason = "cancelled".into();
        a.network_turn_finished(&stop).await;
        let now = current(&a.store, "peer:bob").await;
        assert_eq!(now.status, A2aTaskStatus::Canceled);
        let why = snapshot(&now).status.message.unwrap();
        assert_eq!(why.text(), Some(STOPPED_BY_OWNER));
    }

    /// 190 (D1, D3, D5): a contact's message to a chat is delivered full whatever its
    /// origin's mode says (`bob` carries `cautious`): no slot, a `full` row and no
    /// `<contact_message>` turn, its task a receipt once consumed, in a context its sender
    /// named. The owner then cancels the turn that took it (an interject): a later
    /// `send_message(reply_to)` still goes back to the contact over the return route.
    #[tokio::test]
    async fn a_contact_message_is_full_and_answerable_after_its_turn_is_cancelled() {
        struct Record(tokio::sync::mpsc::UnboundedSender<OutboundPeerMessage>);
        #[async_trait]
        impl PeerSender for Record {
            async fn send(&self, o: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError> {
                let _ = self.0.send(o);
                Err(RemoteSendError::Unknown("recorded".into()))
            }
        }
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.communication.0.write().set_network(true, true);
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, busy, _) = exported_task(&mut a, &principal).await;
        let (tx, mut out) = tokio::sync::mpsc::unbounded_channel();
        a.network.peers.insert(
            binding.id,
            RemotePeer {
                name: "bob".into(),
                roots: Vec::new(),
                sender: Some(Arc::new(Record(tx))),
                generation: CancellationToken::new(),
                contact: true,
                full: vec!["research".into()],
            },
        );
        let p = A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research")),
            ask_first: false,
        };
        let task = a
            .admit_send(&p, &full_send("m-1", "bob-ctx", &FullWire::default()))
            .await
            .unwrap()
            .task;
        assert_eq!(task.status, A2aTaskStatus::Working, "{:?}", task.error);
        assert_eq!(task.context_id, "bob-ctx", "the sender's conversation");
        assert_eq!(
            a.store
                .active_a2a_task(session)
                .await
                .unwrap()
                .map(|t| t.id),
            Some(busy.id),
            "it takes no slot"
        );
        let exchange = task.dedup_exchange.unwrap();
        let record = a.store.exchange_record(exchange).await.unwrap().unwrap();
        assert_eq!(record.contact_mode, Some(super::super::ShareMode::Full));
        assert!(
            a.network.full_inbound.contains_key(&exchange),
            "delivered full (a peer message), not a capped <contact_message> turn"
        );

        let mut stop = finished(session, TurnId::new(), None, "");
        stop.stop_reason = "cancelled".into();
        a.network_turn_finished(&stop).await;
        let done = a.full_consumed(exchange).await.unwrap();
        assert_eq!(done.status, A2aTaskStatus::Completed);
        assert!(full::task_is_receipt(&snapshot(&done)), "a receipt");

        a.network_send(session, "bob", "the answer", None, Some(exchange), false)
            .await
            .unwrap();
        let sent = tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("the answer leaves")
            .unwrap();
        assert_eq!(sent.export.as_deref(), Some(RETURN_ROUTE));
        assert_eq!(sent.context_id.as_deref(), Some("bob-ctx"));
        assert_eq!(sent.full.and_then(|w| w.re).as_deref(), Some("m-1"));
        assert_eq!(sent.record.text, "the answer");
    }

    /// 190 review 1: a contact's message whose chat row cannot be read (a store error) is
    /// refused; it is never taken for a local chat and delivered uncapped.
    #[tokio::test]
    async fn a_contact_message_to_an_unreadable_chat_is_refused_not_delivered_full() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, _, _) = exported_task(&mut a, &principal).await;
        let p = |ask_first| A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research")),
            ask_first,
        };
        // Held first, so it reaches delivery only after the row broke.
        let held = a.admit_send(&p(true), &send("m-1")).await.unwrap().task;
        assert_eq!(held.status, A2aTaskStatus::AuthRequired);
        let generation = a.admit_principal(&p(false)).await.unwrap();
        let h = a
            .network
            .held
            .remove(&("bob".into(), "research".into()))
            .unwrap()
            .remove(0);
        h.watch.abort();
        a.store.break_session_for_test(session).await;
        assert!(
            a.store.session(session).await.is_err(),
            "the row no longer reads"
        );
        let task = a.start_or_queue(h.task, generation, h.route).await;
        assert_eq!(task.status, A2aTaskStatus::Failed);
        // 191 (D3): `deliver_full`'s strict owner predicate refuses an unreadable row.
        assert_eq!(task.error.as_deref(), Some(NOT_OWNER_TEXT));
        assert!(a.network.full_inbound.is_empty(), "never delivered full");
    }

    /// 186 (D5): the return route admits a contact's answer only in a conversation this
    /// engine opened, while it lives and the share is full; else `TASK_NOT_FOUND` (unknown,
    /// older than 7 days, downgraded, unpaired). An admitted answer is a `full` row in the
    /// originating session that names the message it answers.
    #[tokio::test]
    async fn the_return_route_admits_answers_only_on_a_live_full_conversation() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob/proj", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let (session, _, _) = exported_task(&mut a, "peer:lab").await;
        a.network.peers.insert(
            binding.id,
            RemotePeer {
                name: "bob".into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: true,
                full: Vec::new(),
            },
        );
        let offer = |mode| ContactOffer {
            name: "proj".into(),
            title: "proj".into(),
            kind: OfferKind::Chat,
            state: OfferState::Allowed,
            mode,
        };
        a.network.set_offers(BTreeMap::from([(
            "bob".into(),
            vec![offer(super::super::ShareMode::Full)],
        )]));
        // The message we sent, to answer.
        let (from, to) = (format!("session:{session}"), format!("peer:{}", binding.id));
        let sent = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(session),
                recipient_session: None,
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "ping",
            })
            .await
            .unwrap();
        let answer = |ctx: &str, id: &str| A2aReturn {
            binding: binding.id,
            route: CancellationToken::new(),
            request: full_send(
                id,
                ctx,
                &FullWire {
                    re: Some(sent.id.to_string()),
                    ..FullWire::default()
                },
            ),
        };
        let not_found = |r: Result<SendMessageResponse, A2AError>| {
            r.err()
                .is_some_and(|e| e.code == a2a_types::error_code::TASK_NOT_FOUND)
        };
        assert!(
            not_found(a.full_return(answer("ctx-x", "r-0")).await),
            "unknown"
        );

        a.store
            .open_contact_thread(binding.id, "ctx-1", session, "proj", true)
            .await
            .unwrap();
        match a.full_return(answer("ctx-1", "r-1")).await {
            Ok(SendMessageResponse::Message(m)) => assert!(is_receipt(m.metadata.as_ref())),
            other => panic!("a receipt, got {other:?}"),
        }
        let page = a
            .store
            .peer_records(session, None, 50)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.from == to)
            .expect("the answer is a row of the originating session");
        assert_eq!(page.receipt.reply_to, Some(sent.id));
        assert_eq!(page.contact_mode, Some(super::super::ShareMode::Full));

        // 186 review 1: another contact on the same conversation id; a conversation the
        // contact opened (inbound) on the return route; `re` naming another contact's message.
        let carol = a
            .store
            .bind_peer("carol", &contact_endpoint("carol-endpoint"))
            .await
            .unwrap();
        a.network.peers.insert(
            carol.id,
            RemotePeer {
                name: "carol".into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: true,
                full: Vec::new(),
            },
        );
        a.network.set_offers(BTreeMap::from([
            ("bob".into(), vec![offer(super::super::ShareMode::Full)]),
            ("carol".into(), vec![offer(super::super::ShareMode::Full)]),
        ]));
        let as_carol = A2aReturn {
            binding: carol.id,
            ..answer("ctx-1", "c-1")
        };
        assert!(
            not_found(a.full_return(as_carol).await),
            "another contact's id"
        );
        a.store
            .open_contact_thread(binding.id, "ctx-in", session, "proj", false)
            .await
            .unwrap();
        assert!(
            not_found(a.full_return(answer("ctx-in", "r-in")).await),
            "a conversation the contact opened takes no answers here"
        );
        let to_carol = format!("peer:{}", carol.id);
        let theirs = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to_carol,
                sender_session: Some(session),
                recipient_session: None,
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "for carol",
            })
            .await
            .unwrap();
        let mut stray = answer("ctx-1", "r-stray");
        stray.request.message.metadata = Some(
            FullWire {
                re: Some(theirs.id.to_string()),
                ..FullWire::default()
            }
            .metadata(),
        );
        a.full_return(stray).await.unwrap();
        let row = a
            .store
            .peer_records(session, None, 50)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.text == "do research" && r.from == to && r.receipt.id != page.receipt.id)
            .expect("the stray answer is a row");
        assert_eq!(row.receipt.reply_to, None, "it answers nothing of bob's");

        let old = Utc::now().timestamp_millis() - THREAD_TTL.as_millis() as i64 - 1;
        a.store
            .age_contact_thread_for_test(binding.id, "ctx-1", old)
            .await;
        assert!(
            not_found(a.full_return(answer("ctx-1", "r-2")).await),
            "lapsed"
        );
        a.store
            .open_contact_thread(binding.id, "ctx-1", session, "proj", true)
            .await
            .unwrap();
        a.network.set_offers(BTreeMap::from([(
            "bob".into(),
            vec![offer(super::super::ShareMode::Cautious)],
        )]));
        assert!(
            not_found(a.full_return(answer("ctx-1", "r-3")).await),
            "downgraded"
        );
        a.network.set_offers(BTreeMap::from([(
            "bob".into(),
            vec![offer(super::super::ShareMode::Full)],
        )]));
        // 186 review 1, 2 (D9): at most 8 of a contact's answers wait to be taken in; its
        // messages on its own conversations do not count, and a retry is its dedup.
        for (i, ctx) in (0..9).map(|i| (i, if i < 8 { "ctx-1" } else { "ctx-in" })) {
            let r = a
                .store
                .accept_exchange(&AcceptExchange {
                    sender: &to,
                    recipient: &from,
                    sender_session: None,
                    recipient_session: Some(session),
                    request_id: Some(&format!("w-{i}")),
                    reply_to: None,
                    request_scope: None,
                    generation: 1,
                    body: "waiting",
                })
                .await
                .unwrap();
            a.store
                .set_exchange_contact(r.id, super::super::ShareMode::Full, Some(ctx))
                .await
                .unwrap();
        }
        assert!(a.store.answers_waiting(binding.id, session).await.unwrap() >= 8);
        let busy = a.full_return(answer("ctx-1", "r-busy")).await.unwrap_err();
        assert!(busy.message.starts_with("busy:"), "{}", busy.message);
        if let Err(e) = a.full_return(answer("ctx-1", "r-1")).await {
            panic!("a retry of an accepted answer is never busy: {}", e.message);
        }

        // 186 review 2: an answer on a conversation that was collected reads so.
        let lost = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &to,
                recipient: &from,
                sender_session: None,
                recipient_session: Some(session),
                request_id: Some("lost"),
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "from a lapsed conversation",
            })
            .await
            .unwrap();
        a.store
            .set_exchange_contact(lost.id, super::super::ShareMode::Full, Some("ctx-gone"))
            .await
            .unwrap();
        let record = a.store.exchange_record(lost.id).await.unwrap().unwrap();
        let ended = a
            .full_shape(session, "bob", binding.id, Some(&record))
            .await
            .err()
            .expect("no thread: refused");
        assert!(ended.contains("conversation with `bob` ended"), "{ended}");
        a.network.peers.clear();
        assert!(
            not_found(a.full_return(answer("ctx-1", "r-4")).await),
            "unpaired"
        );
    }

    /// 191 (D3, S4): a contact path serves only a session whose row exists here and whose
    /// owner is this engine. On the return route another engine's session, an empty owner and
    /// an unknown row are each `NOT_OWNER` (the contact's client tries its next member) with
    /// nothing accepted; this engine's own is served.
    #[tokio::test]
    async fn the_return_route_serves_only_a_session_strictly_owned_here() {
        let scratch = crate::TestDir::new("agents");
        let mut a =
            crate::supervisor::tests::agents_on(scratch.0.clone(), Some("me-engine"), true).await;
        let binding = a
            .store
            .bind_peer("bob/proj", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let (session, _, _) = exported_task(&mut a, "peer:lab").await;
        a.network.peers.insert(
            binding.id,
            RemotePeer {
                name: "bob".into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: true,
                full: Vec::new(),
            },
        );
        a.network.set_offers(BTreeMap::from([(
            "bob".into(),
            vec![ContactOffer {
                name: "proj".into(),
                title: "proj".into(),
                kind: OfferKind::Chat,
                state: OfferState::Allowed,
                mode: super::super::ShareMode::Full,
            }],
        )]));
        for (ctx, s) in [("ctx-1", session), ("ctx-unknown", SessionId::new())] {
            a.store
                .open_contact_thread(binding.id, ctx, s, "proj", true)
                .await
                .unwrap();
        }
        let answer = |ctx: &str, id: &str| A2aReturn {
            binding: binding.id,
            route: CancellationToken::new(),
            request: full_send(id, ctx, &FullWire::default()),
        };
        let peer = format!("peer:{}", binding.id);
        for (owner, ctx, id) in [
            (Some("other-engine"), "ctx-1", "r-other"),
            (Some(""), "ctx-1", "r-empty"),
            (None, "ctx-unknown", "r-unknown"),
        ] {
            if let Some(owner) = owner {
                a.store.owned_elsewhere(session, owner).await;
            }
            let r = a.full_return(answer(ctx, id)).await;
            assert_eq!(r.err().map(|e| e.code), Some(NOT_OWNER_CODE), "{id}");
            assert_eq!(
                a.store.exchange_by_request_id(&peer, id).await.unwrap(),
                None,
                "{id}: nothing accepted here"
            );
        }
        a.store.owned_elsewhere(session, "me-engine").await;
        match a.full_return(answer("ctx-1", "r-mine")).await {
            Ok(SendMessageResponse::Message(m)) => assert!(is_receipt(m.metadata.as_ref())),
            other => panic!("served, got {other:?}"),
        }
        // An unreadable row proves nothing: refused too.
        a.store.break_session_for_test(session).await;
        let r = a.full_return(answer("ctx-1", "r-unreadable")).await;
        assert_eq!(r.err().map(|e| e.code), Some(NOT_OWNER_CODE));
    }

    /// 191 (D3, S4): a contact's message into a chat whose owner is empty (not this engine's)
    /// is `NOT_OWNER` at admission, before anything is
    /// accepted; one held before the owner went empty is never delivered full. A drained inbox
    /// still in the insert-only `inboxes` set never short-circuits the check.
    #[tokio::test]
    async fn a_contact_message_into_a_session_not_strictly_ours_is_never_delivered() {
        let scratch = crate::TestDir::new("agents");
        let mut a =
            crate::supervisor::tests::agents_on(scratch.0.clone(), Some("me-engine"), true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, _, _) = exported_task(&mut a, &principal).await;
        let p = |ask_first| A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research")),
            ask_first,
        };
        // Two held first, so they reach delivery only after the owner changed.
        for id in ["m-1", "m-2"] {
            let held = a.admit_send(&p(true), &send(id)).await.unwrap().task;
            assert_eq!(held.status, A2aTaskStatus::AuthRequired);
        }
        let generation = a.admit_principal(&p(false)).await.unwrap();
        let mut held = a
            .network
            .held
            .remove(&("bob".into(), "research".into()))
            .unwrap();
        let not_owner = |t: &A2aTask| {
            t.status == A2aTaskStatus::Failed && t.error.as_deref() == Some(NOT_OWNER_TEXT)
        };
        // An empty owner on a mesh engine.
        a.store.owned_elsewhere(session, "").await;
        let refused = a.admit_send(&p(false), &send("m-3")).await.err();
        assert_eq!(refused.map(|e| e.code), Some(NOT_OWNER_CODE));
        let h = held.remove(0);
        h.watch.abort();
        let task = a.start_or_queue(h.task, generation.clone(), h.route).await;
        assert!(not_owner(&task), "{task:?}");
        // A drained inbox: still in `inboxes`, now another engine's.
        a.store.owned_elsewhere(session, "other-engine").await;
        a.network.inboxes.insert(session);
        let refused = a.admit_send(&p(false), &send("m-4")).await.err();
        assert_eq!(refused.map(|e| e.code), Some(NOT_OWNER_CODE));
        let h = held.remove(0);
        h.watch.abort();
        // Its task row cannot even be failed here (the store refuses writes to a session
        // another engine owns): it is never started nor delivered.
        let task = a.start_or_queue(h.task, generation, h.route).await;
        assert_eq!(task.status, A2aTaskStatus::AuthRequired, "{task:?}");
        assert!(a.network.full_inbound.is_empty(), "never delivered full");
    }

    /// 191 (D3, S4, P2 review MUST 1): an unreadable session row proves no owner. A contact's
    /// message is refused at admission (fixed text, never the store's), `full_intake` refuses
    /// on its own, and a task held before the row broke is never delivered (`deliver_full`),
    /// even for a session in the insert-only `inboxes` set.
    #[tokio::test]
    async fn an_unreadable_owner_refuses_every_contact_path() {
        let scratch = crate::TestDir::new("agents");
        let mut a =
            crate::supervisor::tests::agents_on(scratch.0.clone(), Some("me-engine"), true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = format!("peer:{}", binding.id);
        let (session, _, _) = exported_task(&mut a, &principal).await;
        let p = |ask_first| A2aPrincipal {
            binding: binding.id,
            export: "research".into(),
            session,
            route: CancellationToken::new(),
            contact: Some(bob("research")),
            ask_first,
        };
        let held = a.admit_send(&p(true), &send("m-1")).await.unwrap().task;
        assert_eq!(held.status, A2aTaskStatus::AuthRequired);
        let generation = a.admit_principal(&p(false)).await.unwrap();
        let h = a
            .network
            .held
            .remove(&("bob".into(), "research".into()))
            .unwrap()
            .remove(0);
        h.watch.abort();
        let record = a
            .store
            .exchange_record(h.task.dedup_exchange.unwrap())
            .await
            .unwrap()
            .unwrap();
        a.store.break_session_for_test(session).await;
        let refused = a.admit_send(&p(false), &send("m-2")).await.err().unwrap();
        assert_eq!(
            (refused.code, refused.message.as_str()),
            (a2a_types::error_code::INTERNAL_ERROR, "internal error")
        );
        assert_eq!(
            a.full_intake(record, "peer:bob".into(), generation.clone(), 0)
                .await,
            Err(NOT_OWNER_TEXT.to_owned())
        );
        a.network.inboxes.insert(session);
        let task = a.start_or_queue(h.task, generation, h.route).await;
        assert!(
            task.status == A2aTaskStatus::AuthRequired
                || (task.status == A2aTaskStatus::Failed
                    && task.error.as_deref() == Some(NOT_OWNER_TEXT)),
            "{task:?}"
        );
        assert!(a.network.full_inbound.is_empty(), "never delivered full");
    }
}

#[cfg(test)]
mod full_inbox_tests {
    use super::*;
    use std::sync::Arc;

    /// The roster's `research` agent: `None` = no such agent (deleted, invalid file); else its
    /// `tools:` ceiling.
    #[derive(Clone, Default)]
    struct Roster(Arc<parking_lot::Mutex<Option<Option<tools::Risk>>>>);

    impl Roster {
        fn research(tools: Option<tools::Risk>) -> Self {
            let r = Self::default();
            r.set(Some(tools));
            r
        }
        fn set(&self, v: Option<Option<tools::Risk>>) {
            *self.0.lock() = v;
        }
    }

    #[async_trait::async_trait]
    impl crate::IdentitySource for Roster {
        async fn identity(&self, _: &[SessionId]) -> Option<String> {
            None
        }
        async fn inbox(&self, agent: &str) -> Option<crate::InboxPersona> {
            let tools = (*self.0.lock())?;
            (agent == "research").then_some(crate::InboxPersona {
                identity: None,
                model: None,
                tools,
            })
        }
    }

    fn full_bob() -> crate::communication::ContactOrigin {
        crate::communication::ContactOrigin {
            contact: "bob".into(),
            machine: "b".into(),
            export: "research".into(),
            mode: crate::communication::ShareMode::Full,
        }
    }

    async fn admit(a: &mut Agents, binding: Uuid) -> Result<SessionId, A2AError> {
        a.handle_inbox(ContactInbox {
            binding,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: None,
            home: InboxHome::Here,
            send: true,
        })
        .await
    }

    /// A restart: nothing of the inbox in memory.
    fn forget(a: &mut Agents, inbox: SessionId) {
        a.network.inbox_personas.clear();
        a.network.inbox_limits.clear();
        a.caps.remove(&inbox);
    }

    fn capped(a: &Agents, inbox: SessionId) -> bool {
        a.caps.get(&inbox) == Some(&INBOX_FAIL_CLOSED)
            && a.network.inbox_limits.get(&inbox) == Some(&(INBOX_FAIL_CLOSED, Vec::new()))
    }

    /// `binding` in the reload's routes (a live contact).
    fn route(a: &mut Agents, binding: Uuid) {
        a.network.peers.insert(
            binding,
            RemotePeer {
                name: "bob".into(),
                roots: Vec::new(),
                sender: None,
                generation: CancellationToken::new(),
                contact: true,
                full: vec!["research".into()],
            },
        );
    }

    /// 190 (D1, D6): an agent export runs full under the agent's own ceiling whatever the
    /// contact's origin says (an older holder's `cautious`, its cap and allowlist): no contact
    /// limits, the row marked full, so an actor start keeps the ceiling.
    #[tokio::test]
    async fn an_agent_export_runs_full_whatever_the_origin_says() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let inbox = a
            .handle_inbox(ContactInbox {
                binding: binding.id,
                agent: "research@sky-mac".into(),
                origin: crate::communication::ContactOrigin {
                    mode: crate::communication::ShareMode::Cautious,
                    ..full_bob()
                },
                export_id: None,
                home: InboxHome::Here,
                send: true,
            })
            .await
            .unwrap();
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "the agent's ceiling, not the contact's read_only"
        );
        assert!(!a.network.is_inbox(inbox), "no allowlist");
        assert!(a.store.a2a_inbox_full(inbox).await.unwrap().is_some());
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert_eq!(a.caps.get(&inbox), Some(&tools::Risk::Mutating));
    }

    /// 16 §5 (2026-11-11), Sky's ruling (20:28): a full inbox runs with the agent's own tools,
    /// so its calls get the local-network grant like the owner's (the fetcher's setting decides
    /// the rest), also after an actor start; once its share is no longer granted it is capped,
    /// a restricted state, and the public web only.
    #[test]
    fn a_full_inbox_keeps_the_lan_a_capped_one_does_not() {
        struct Probe;
        #[async_trait::async_trait]
        impl tools::Tool for Probe {
            const NAME: &'static str = "read_url";
            const RISK: tools::Risk = tools::Risk::ReadOnly;
            type Args = serde_json::Value;
            fn description(&self) -> String {
                String::new()
            }
            async fn call(
                &self,
                _: Self::Args,
                _: tools::ToolCx,
            ) -> anyhow::Result<tools::ToolOutput> {
                Ok(tools::ToolOutput::ok(tools::egress::lan().to_string()))
            }
        }
        crate::with_test_registry(async {
            let (scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
            let reg = tools::registry::start(&runtime::Runtime::new(
                figment::Figment::new(),
                scratch.0.clone(),
            ))
            .await
            .unwrap();
            reg.ask(tools::RegisterTool(Arc::new(Probe))).await.unwrap();
            let lan = |session| {
                let reg = reg.clone();
                async move {
                    reg.ask(tools::Invoke(tools::ToolCall {
                        id: "c".into(),
                        name: "read_url".into(),
                        args: serde_json::json!({}),
                        session,
                        quiet: true,
                    }))
                    .await
                    .unwrap()
                    .content
                }
            };
            a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
            let binding = a
                .store
                .bind_peer("bob", &contact_endpoint("bob-endpoint"))
                .await
                .unwrap();
            route(&mut a, binding.id);
            let inbox = admit(&mut a, binding.id).await.unwrap();
            assert!(!a.network.is_inbox(inbox), "full: no contact limits");
            assert_eq!(lan(inbox).await, "true", "a full inbox");
            forget(&mut a, inbox);
            a.reinstall_contact_limits(inbox).await.unwrap();
            assert_eq!(lan(inbox).await, "true", "after an actor start");
            // No longer granted: capped fail-closed, a restricted state (the public web only).
            a.apply_contact_limits(ContactLimits {
                contact: "bob".into(),
                full: Vec::new(),
            })
            .await;
            assert!(reg.ask(tools::ContactScoped(inbox)).await.unwrap());
        });
    }

    /// 186 review 1, 2 (D6): an agent inbox shared in full runs under the agent's own ceiling,
    /// read live: kept across a restart before the roster joins (the stored one), tightened
    /// on a running inbox when the roster reads a stricter one, capped while the agent is gone
    /// and back when it returns; once the share leaves full, the next actor start caps it.
    #[tokio::test]
    async fn a_full_agent_inbox_follows_its_agents_live_ceiling() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let roster = Roster::research(Some(tools::Risk::Mutating));
        a.identity = Some(Arc::new(roster.clone()));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let inbox = admit(&mut a, binding.id).await.unwrap();
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "the agent's ceiling, not the contact's read_only"
        );
        assert!(!a.network.is_inbox(inbox), "no contact limits");

        // A restart, the roster not joined yet: the stored ceiling.
        forget(&mut a, inbox);
        a.identity = None;
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "from the row"
        );
        assert!(!a.network.is_inbox(inbox));

        // The roster joins with a stricter `tools:`: the running inbox takes it at once.
        roster.set(Some(Some(tools::Risk::ReadOnly)));
        a.identity = Some(Arc::new(roster.clone()));
        a.refresh_full_inboxes().await;
        assert_eq!(a.caps.get(&inbox), Some(&tools::Risk::ReadOnly));
        // An actor start takes the stricter of the stored and the live one.
        roster.set(Some(Some(tools::Risk::Mutating)));
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::ReadOnly),
            "the stricter"
        );

        // The agent deleted: capped; an actor start too. Re-created: its ceiling again.
        roster.set(None);
        a.refresh_full_inboxes().await;
        assert!(capped(&a, inbox), "agent gone");
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox), "agent gone, at an actor start");
        roster.set(Some(Some(tools::Risk::Mutating)));
        a.refresh_full_inboxes().await;
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "re-created"
        );
        assert!(!a.network.is_inbox(inbox));

        // No longer granted (the reload's limits), then a restart: capped fail-closed.
        a.apply_contact_limits(ContactLimits {
            contact: "bob".into(),
            full: Vec::new(),
        })
        .await;
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::ReadOnly),
            "capped at once"
        );
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox), "default-deny again");
    }

    /// 191 (S4, P2 review SHOULD 1): at a machine whose inbox's session is no longer its own
    /// (moved), the router's resolve answers a read (`GetTask`, `CancelTask`: `send` unset)
    /// TASK_NOT_FOUND and a send `not owner`, before any side effect: no inbox opened, the row
    /// not marked (its stored ceiling kept though the live one is higher), no limits installed
    /// (no cap lifted). A read from a contact with no inbox here opens none.
    #[tokio::test]
    async fn a_non_owner_inbox_resolve_changes_nothing() {
        use a2a_types::error_code::TASK_NOT_FOUND;
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let roster = Roster::research(Some(tools::Risk::ReadOnly));
        a.identity = Some(Arc::new(roster.clone()));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let inbox = admit(&mut a, binding.id).await.unwrap();
        forget(&mut a, inbox);
        a.store.owned_elsewhere(inbox, "other-engine").await;
        roster.set(Some(Some(tools::Risk::Mutating)));
        let ask = |binding, send| ContactInbox {
            binding,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: Some("x-research".into()),
            home: InboxHome::Here,
            send,
        };
        let key = format!("peer:{}", binding.id);
        let scope = A2aScope {
            principal: &key,
            export: "research",
        };
        let sessions = a.store.list_sessions().await.unwrap().len();
        for (what, send, code) in [
            ("GetTask", false, TASK_NOT_FOUND),
            ("CancelTask", false, TASK_NOT_FOUND),
            ("SendMessage", true, NOT_OWNER_CODE),
        ] {
            let r = a.handle_inbox(ask(binding.id, send)).await;
            assert_eq!(r.err().map(|e| e.code), Some(code), "{what}");
            assert_eq!(a.store.a2a_context_count_for_test(scope).await, 1, "{what}");
            assert_eq!(a.store.list_sessions().await.unwrap().len(), sessions);
            assert_eq!(
                a.store
                    .a2a_inbox_full(inbox)
                    .await
                    .unwrap()
                    .map(|r| r.ceiling),
                Some(Some(Some(tools::Risk::ReadOnly))),
                "{what}: not marked"
            );
            assert!(
                !a.caps.contains_key(&inbox) && !a.network.inbox_personas.contains_key(&inbox),
                "{what}: no limits installed"
            );
        }
        // A read from a contact with no inbox here: none opened.
        let carol = a
            .store
            .bind_peer("carol", &contact_endpoint("carol-endpoint"))
            .await
            .unwrap();
        route(&mut a, carol.id);
        let r = a.handle_inbox(ask(carol.id, false)).await;
        assert_eq!(r.err().map(|e| e.code), Some(TASK_NOT_FOUND));
        let key = format!("peer:{}", carol.id);
        let scope = A2aScope {
            principal: &key,
            export: "research",
        };
        assert_eq!(a.store.a2a_context_count_for_test(scope).await, 0);
        assert_eq!(a.store.list_sessions().await.unwrap().len(), sessions);
        // An unreadable row proves nothing either.
        a.store.break_session_for_test(inbox).await;
        let r = a.handle_inbox(ask(binding.id, true)).await;
        assert_eq!(r.err().map(|e| e.code), Some(NOT_OWNER_CODE));
        assert!(!a.caps.contains_key(&inbox));
    }

    /// 186 review 2: with no live persona (the roster not joined, the agent missing) a full
    /// share of an agent does not run: refused, no inbox made; a NULL stored ceiling is unknown.
    #[tokio::test]
    async fn a_full_share_without_the_agents_persona_is_refused() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let refused = admit(&mut a, binding.id).await.unwrap_err();
        assert!(
            refused.message.contains("not available"),
            "{}",
            refused.message
        );
        a.identity = Some(Arc::new(Roster::default()));
        let refused = admit(&mut a, binding.id).await.unwrap_err();
        assert!(
            refused.message.contains("not available"),
            "{}",
            refused.message
        );
        assert!(
            a.store
                .inbox_rows(&format!("peer:{}", binding.id), "research")
                .await
                .unwrap()
                .is_empty(),
            "no inbox made"
        );
        assert_eq!(
            full::full_ceiling(&full::Live::Unknown, None),
            None,
            "unknown: capped"
        );
    }

    /// 186 review 2: an inbox stops running in full when its export is unshared or deleted
    /// (the reload's limits no longer list it in full) and when its contact is blocked or
    /// unpaired (its binding falls out); each is capped at once and at its next actor start.
    #[tokio::test]
    async fn unshare_delete_block_and_unpair_end_a_full_inbox() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(None)));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let full_row = |a: &Agents, inbox| {
            let store = a.store.clone();
            async move { store.a2a_inbox_full(inbox).await.unwrap().is_some() }
        };
        // Unshared or its agent export deleted: the contact's limits list it nowhere.
        let inbox = admit(&mut a, binding.id).await.unwrap();
        assert!(full_row(&a, inbox).await);
        a.apply_contact_limits(ContactLimits {
            contact: "bob".into(),
            full: Vec::new(),
        })
        .await;
        assert!(!full_row(&a, inbox).await, "unshared or deleted: not full");
        assert!(capped(&a, inbox));
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox));

        // Blocked or unpaired: its binding falls out of the reload.
        assert_eq!(admit(&mut a, binding.id).await.unwrap(), inbox);
        assert!(full_row(&a, inbox).await);
        a.cap_inboxes_of(binding.id).await;
        assert!(!full_row(&a, inbox).await, "blocked or unpaired: not full");
        assert!(capped(&a, inbox));
        // And a row left full by an older build: the revoked binding caps it at actor start.
        assert_eq!(admit(&mut a, binding.id).await.unwrap(), inbox);
        a.store.revoke_peer_binding("bob").await.unwrap();
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox));
        assert!(!full_row(&a, inbox).await);
    }

    /// 186 review 3: a contact removed or blocked in the config while the engine was down:
    /// its stored binding is live and its inbox starts full, until the first reload after the
    /// boot (no route of it, none in memory) revokes the binding and caps the inbox; a roster
    /// re-read before that reload leaves the unrouted inbox alone.
    #[tokio::test]
    async fn a_contact_removed_while_down_loses_its_full_inbox_at_the_first_reload() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let roster = Roster::research(Some(tools::Risk::Mutating));
        a.identity = Some(Arc::new(roster.clone()));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let inbox = admit(&mut a, binding.id).await.unwrap();
        // The boot: nothing in memory, the binding row still live.
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "starts full"
        );
        // A roster re-read (the agent gone, which would cap a routed inbox) skips it.
        roster.set(None);
        a.refresh_full_inboxes().await;
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "unrouted: skipped"
        );
        roster.set(Some(Some(tools::Risk::Mutating)));
        // The first reload: bob is in no route (removed or blocked).
        a.drop_unrouted(&HashSet::new()).await;
        assert!(
            a.store
                .peer_binding_by_id(binding.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(capped(&a, inbox));
        assert!(a.store.a2a_inbox_full(inbox).await.unwrap().is_none());
        forget(&mut a, inbox);
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox), "and at the next actor start");
    }

    /// 191 (D7, M2): before `contacts_ready` a full inbox whose binding is live here starts
    /// capped at an actor start (a removal made elsewhere may not have synced), is admitted
    /// nothing and lifted by nothing; its row stays marked full, so the refresh right after
    /// the flip lifts the cap that is still due.
    #[tokio::test]
    async fn an_inbox_starting_before_contacts_ready_runs_capped_until_the_flip() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let inbox = admit(&mut a, binding.id).await.unwrap();
        // A restart, the latch not passed yet.
        forget(&mut a, inbox);
        a.contacts_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        a.reinstall_contact_limits(inbox).await.unwrap();
        assert!(capped(&a, inbox), "capped at the actor start");
        assert!(
            a.store.a2a_inbox_full(inbox).await.unwrap().is_some(),
            "the row stays marked full"
        );
        a.refresh_full_inboxes().await;
        assert!(capped(&a, inbox), "nothing is lifted before the flip");
        let refused = admit(&mut a, binding.id).await.unwrap_err();
        assert!(refused.to_string().contains(CONTACTS_WAITING), "{refused}");
        assert!(capped(&a, inbox));
        a.contacts_ready.store(true, Ordering::SeqCst);
        a.refresh_full_inboxes().await;
        assert_eq!(
            a.caps.get(&inbox),
            Some(&tools::Risk::Mutating),
            "lifted after the flip"
        );
    }

    /// 191 (D4): an inbox taken by a Write (its session row marked, no local context here)
    /// runs capped: read-only, nothing but the reply. An unmarked session gets no limits.
    #[tokio::test]
    async fn an_inbox_taken_by_a_write_runs_capped_from_its_marker() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (taken, plain) = (SessionId::new(), SessionId::new());
        for s in [taken, plain] {
            a.store
                .create_session(s, "m", None, None, None)
                .await
                .unwrap();
        }
        assert!(
            a.store
                .set_contact_inbox(taken, "peer:b1|x1")
                .await
                .unwrap()
        );
        a.reinstall_contact_limits(taken).await.unwrap();
        assert!(capped(&a, taken));
        a.reinstall_contact_limits(plain).await.unwrap();
        assert!(!a.network.is_inbox(plain) && !a.caps.contains_key(&plain));
    }

    /// The owner a `not owner` names (its [`OWNER_DETAIL`]).
    fn named(e: &A2AError) -> Option<String> {
        e.details.iter().flatten().find_map(|d| {
            (d.type_url == OWNER_DETAIL)
                .then(|| d.value.get("owner")?.as_str().map(str::to_owned))
                .flatten()
        })
    }

    /// 191 (D4): an agent export's inbox. With none anywhere only the home opens one (another
    /// machine answers `not owner` naming the home; a home that left opens none). A marked
    /// inbox owned here with no local row (it moved here) runs capped until a contact message
    /// adopts it, then full, at once and at the next actor start. Owned elsewhere, it is `not
    /// owner` naming its owner. Two sessions carrying one marker are refused, never chosen
    /// between, and both run capped.
    #[tokio::test]
    async fn an_inbox_is_adopted_by_its_marker_and_never_chosen_between() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let ask = |home: InboxHome| ContactInbox {
            binding: binding.id,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: Some("x1".into()),
            home,
            send: true,
        };
        let marker = format!("peer:{}|x1", binding.id);
        let e = a
            .handle_inbox(ask(InboxHome::At("home-engine".into())))
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, named(&e).as_deref()),
            (NOT_OWNER_CODE, Some("home-engine")),
            "not the home: forwarded there"
        );
        let e = a.handle_inbox(ask(InboxHome::Left)).await.unwrap_err();
        assert!(e.message.contains("left the mesh"), "{}", e.message);
        assert!(a.store.marked_inboxes(&marker).await.unwrap().is_empty());

        // A marked inbox that moved here: no local row, capped from its marker.
        let moved = SessionId::new();
        a.store
            .create_session(moved, "m", None, None, None)
            .await
            .unwrap();
        a.store.set_contact_inbox(moved, &marker).await.unwrap();
        a.reinstall_contact_limits(moved).await.unwrap();
        assert!(capped(&a, moved), "capped before the adoption");
        assert_eq!(
            a.handle_inbox(ask(InboxHome::At("home-engine".into())))
                .await
                .unwrap(),
            moved,
            "adopted, though this machine is not the home"
        );
        assert_eq!(a.caps.get(&moved), Some(&tools::Risk::Mutating), "full");
        forget(&mut a, moved);
        a.reinstall_contact_limits(moved).await.unwrap();
        assert_eq!(
            a.caps.get(&moved),
            Some(&tools::Risk::Mutating),
            "full at the next actor start"
        );

        // It moves on: its local row here names a session another machine owns.
        a.store.owned_elsewhere(moved, "n2-engine").await;
        let e = a.handle_inbox(ask(InboxHome::Here)).await.unwrap_err();
        assert_eq!(
            (e.code, named(&e).as_deref()),
            (NOT_OWNER_CODE, Some("n2-engine"))
        );
        a.store.owned_elsewhere(moved, "").await;

        // A second session carrying the marker: refused, both capped.
        let extra = SessionId::new();
        a.store
            .create_session(extra, "m", None, None, None)
            .await
            .unwrap();
        a.store.set_contact_inbox(extra, &marker).await.unwrap();
        let e = a.handle_inbox(ask(InboxHome::Here)).await.unwrap_err();
        assert!(e.message.contains("more than one inbox"), "{}", e.message);
        assert!(named(&e).is_none());
        assert!(capped(&a, moved) && capped(&a, extra), "both capped");
        assert!(a.store.a2a_inbox_full(moved).await.unwrap().is_none());
    }

    /// 191 (D4): a name-valued marker is never adopted (the home opens a new, marked inbox and
    /// the named one stays capped). After the P2 review's owner check, a local row whose
    /// session another machine owns refuses before anything happens here, marked or not (an
    /// inbox that moved before it was marked included): `not owner` naming that machine.
    #[tokio::test]
    async fn a_name_valued_or_unmarked_moved_inbox_is_never_reached_again() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let ask = |export_id: Option<&str>| ContactInbox {
            binding: binding.id,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: export_id.map(str::to_owned),
            home: InboxHome::Here,
            send: true,
        };
        let marker = format!("peer:{}|x1", binding.id);
        // A name-valued marker (an export no longer known by name) on a session owned here.
        let named_one = SessionId::new();
        a.store
            .create_session(named_one, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .set_contact_inbox(named_one, &format!("peer:{}|research", binding.id))
            .await
            .unwrap();
        // Not the home: nothing to mint, so only an adoption could serve it; there is none.
        let sessions = a.store.list_sessions().await.unwrap().len();
        let e = a
            .handle_inbox(ContactInbox {
                home: InboxHome::At("home-engine".into()),
                ..ask(Some("x1"))
            })
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, named(&e).as_deref()),
            (NOT_OWNER_CODE, Some("home-engine")),
            "a name-valued marker is never adopted"
        );
        assert!(
            a.store
                .a2a_inbox_identity(named_one)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(a.store.list_sessions().await.unwrap().len(), sessions);
        let fresh = a.handle_inbox(ask(Some("x1"))).await.unwrap();
        assert_ne!(fresh, named_one, "the home opens a new one");
        assert_eq!(
            a.store.contact_inbox(fresh).await.unwrap().as_deref(),
            Some(marker.as_str())
        );
        a.reinstall_contact_limits(named_one).await.unwrap();
        assert!(capped(&a, named_one), "and it still caps");
        // Before 191: an unmarked inbox (no export id at its admission) that moved away.
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let ask = |export_id: Option<&str>| ContactInbox {
            binding: binding.id,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: export_id.map(str::to_owned),
            home: InboxHome::Here,
            send: true,
        };
        let old = a.handle_inbox(ask(None)).await.unwrap();
        a.store.owned_elsewhere(old, "n2-engine").await;
        let sessions = a.store.list_sessions().await.unwrap().len();
        let e = a.handle_inbox(ask(Some("x1"))).await.unwrap_err();
        assert_eq!(
            (e.code, named(&e).as_deref()),
            (NOT_OWNER_CODE, Some("n2-engine"))
        );
        assert_eq!(
            a.store.list_sessions().await.unwrap().len(),
            sessions,
            "none opened"
        );
    }

    /// 191 (D4): a marked inbox that moved here is adopted only under the agent's live persona:
    /// `theuth` before the roster joins (no persona) is refused, nothing is adopted and it
    /// stays capped.
    #[tokio::test]
    async fn a_moved_inbox_is_never_adopted_without_a_live_persona() {
        let (_scratch, mut a) = crate::supervisor::tests::agents_with(true).await;
        assert!(a.identity.is_none(), "the roster has not joined");
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        route(&mut a, binding.id);
        let moved = SessionId::new();
        a.store
            .create_session(moved, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .set_contact_inbox(moved, &format!("peer:{}|x1", binding.id))
            .await
            .unwrap();
        a.reinstall_contact_limits(moved).await.unwrap();
        let e = a
            .handle_inbox(ContactInbox {
                binding: binding.id,
                agent: "theuth@sky-mac".into(),
                origin: full_bob(),
                export_id: Some("x1".into()),
                home: InboxHome::At("home-engine".into()),
                send: true,
            })
            .await
            .unwrap_err();
        assert!(e.message.contains("is not available"), "{}", e.message);
        assert!(a.store.a2a_inbox_identity(moved).await.unwrap().is_none());
        assert!(capped(&a, moved), "still capped");
    }

    /// 191 (D4, S4): on a mesh engine an empty owner is unknown, never this engine: a local
    /// inbox row, or the one session carrying the marker, whose owner reads `''` is a plain
    /// `not owner` (naming no one), and nothing is opened or adopted. (A lone engine reads its
    /// own rows as `''`, so this needs a mesh engine.)
    #[tokio::test]
    async fn an_empty_owner_on_a_mesh_engine_is_never_here() {
        let setup = || async {
            let scratch = crate::TestDir::new("agents");
            let mut a =
                crate::supervisor::tests::agents_on(scratch.0.clone(), Some("me-engine"), true)
                    .await;
            assert_eq!(a.store.engine(), "me-engine");
            a.identity = Some(Arc::new(Roster::research(Some(tools::Risk::Mutating))));
            let binding = a
                .store
                .bind_peer("bob", &contact_endpoint("bob-endpoint"))
                .await
                .unwrap();
            route(&mut a, binding.id);
            (scratch, a, binding.id)
        };
        let ask = |binding: Uuid, export_id: Option<&str>| ContactInbox {
            binding,
            agent: "research@sky-mac".into(),
            origin: full_bob(),
            export_id: export_id.map(str::to_owned),
            home: InboxHome::Here,
            send: true,
        };
        // The local row.
        let (_scratch, mut a, binding) = setup().await;
        let opened = a.handle_inbox(ask(binding, None)).await.unwrap();
        a.store.owned_elsewhere(opened, "").await;
        let sessions = a.store.list_sessions().await.unwrap().len();
        let e = a.handle_inbox(ask(binding, None)).await.unwrap_err();
        assert_eq!((e.code, named(&e)), (NOT_OWNER_CODE, None), "local row");
        assert_eq!(a.store.list_sessions().await.unwrap().len(), sessions);
        // The marker, with no local row.
        let (_scratch, mut a, binding) = setup().await;
        let moved = SessionId::new();
        a.store
            .create_session(moved, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .set_contact_inbox(moved, &format!("peer:{binding}|x1"))
            .await
            .unwrap();
        a.store.owned_elsewhere(moved, "").await;
        let sessions = a.store.list_sessions().await.unwrap().len();
        let e = a.handle_inbox(ask(binding, Some("x1"))).await.unwrap_err();
        assert_eq!((e.code, named(&e)), (NOT_OWNER_CODE, None), "marker");
        assert!(a.store.a2a_inbox_identity(moved).await.unwrap().is_none());
        assert_eq!(a.store.list_sessions().await.unwrap().len(), sessions);
    }
}
