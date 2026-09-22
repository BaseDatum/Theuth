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

use std::collections::{HashMap, HashSet};
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
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tools::{Operation, OperationId};
use uuid::Uuid;

use super::{
    CommunicationChanged, Generation, PEER_TEXT_MAX, PeerInfo, PeerKind, PeerMessageState,
    PeerReceipt, PeerRecord, PeerRelationship, PeerState, PeerTurnFinished, SharedPolicy,
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
}

/// A registered remote route: the plugin's SDK client behind one binding.
#[async_trait]
pub trait PeerSender: Send + Sync {
    /// Send one message; `Ok` is the peer's typed response.
    async fn send(&self, outbound: OutboundPeerMessage) -> Result<RemoteReply, RemoteSendError>;
}

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

/// `ask` → `Result<(), String>`: an export names an existing native session
/// (§6.3: ACP proxies are not exportable).
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
}

impl NetworkState {
    /// Discovery rows (§5.1) for the routes granted to `root`: alias, kind and
    /// capabilities only; a card is not a heartbeat, so the state is `Idle`.
    pub(crate) fn remote_peers(&self, root: SessionId) -> Vec<PeerInfo> {
        let mut rows: Vec<PeerInfo> = self
            .peers
            .values()
            .filter(|p| {
                !p.generation.is_cancelled() && p.sender.is_some() && p.roots.contains(&root)
            })
            .map(|p| PeerInfo {
                address: format!("peer:{}", p.name),
                name: p.name.clone(),
                kind: PeerKind::A2a,
                relationship: PeerRelationship::Remote,
                state: PeerState::Idle,
                send: true,
                reply: true,
                stream: false,
                activity: None,
                title: None,
                tags: Vec::new(),
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        rows
    }

    fn peer_by_name(&self, name: &str) -> Option<(Uuid, &RemotePeer)> {
        self.peers
            .iter()
            .find(|(_, p)| p.name == name && !p.generation.is_cancelled())
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
    /// opening a private turn. Each row goes when its producer settles it.
    fn retire(&mut self, scope: Uuid) {
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
struct Admitted {
    task: A2aTask,
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
        self.communication
            .0
            .write()
            .remote(p.session)
            .map_err(|e| unavailable(e.to_string()))
    }

    /// Validate, persist and start (or queue / resume) one `SendMessage`.
    async fn admit_send(
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
        let route = p.route.clone();
        if !self.network.enqueued.insert(receipt.id) {
            // Durable dedup: the existing task, whatever it has become.
            return Ok(Admitted {
                task,
                generation,
                route,
            });
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
        if let Some(q) = question {
            if let Ok(Some(orig)) = self.store.exchange_receipt(q).await {
                if orig.state == PeerMessageState::Replied && self.settle_wait(&orig) {
                    handed = true;
                    if let Err(e) = self
                        .store
                        .consume_exchange_direct(
                            receipt.id,
                            task.session,
                            generation.revision() as i64,
                        )
                        .await
                    {
                        tracing::warn!(exchange = %receipt.id, "a2a: direct consume: {e:#}");
                    }
                }
                self.peer_changed(&orig).await;
            }
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
        let Some(scope) = m.scope.filter(|s| self.network.scopes.contains_key(s)) else {
            return;
        };
        if self.network.scopes[&scope].session == m.session {
            let text = self.redact(&m.text).into_owned();
            let sc = self.network.scopes.get_mut(&scope).expect("checked above");
            match m.stop_reason.as_str() {
                "cancelled" | "timed_out" => sc.interrupted = true,
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
            if m.revoke.iter().any(|a| *a == r.alias) {
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
                    generation: route.generation,
                },
            );
        }
        self.abandon_outbound(
            |o| !keep.contains(&o.binding),
            "peer route removed by configuration reload",
        )
        .await;
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
        match self.store.session(m.session).await {
            Ok(Some(s)) if s.agent.is_none() => Ok(()),
            Ok(Some(_)) => Err(format!(
                "session {} runs an external harness; only native sessions are exportable",
                m.session
            )),
            Ok(None) => Err(format!("unknown session {}", m.session)),
            Err(e) => Err(format!("{e:#}")),
        }
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
        let (binding, sender, route) = {
            let (binding, peer) = self
                .network
                .peer_by_name(alias)
                .ok_or_else(|| format!("unknown peer `{alias}`"))?;
            (binding, peer.sender.clone(), peer.generation.clone())
        };
        let generation = self
            .communication
            .0
            .write()
            .remote(from)
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
                let granted = self
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
        let receipt = self
            .store
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
            let r = self
                .store
                .fail_exchange(receipt.id, "communication_disabled")
                .await
                .map_err(|e| format!("{e:#}"))?;
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
            },
        );
        let outbound = OutboundPeerMessage {
            binding,
            request_id: request_id.map_or_else(|| receipt.id.to_string(), str::to_owned),
            record,
            task_id: remote_task,
            context_id: None,
            generation,
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
            TaskState::Submitted | TaskState::Working | TaskState::Unspecified => {}
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
        let Ok(Some(orig)) = self.store.exchange_receipt(exchange).await else {
            return;
        };
        self.peer_changed(&orig).await;
        if reply.state != PeerMessageState::Accepted {
            return;
        }
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
            Ok(Some(record)) => {
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
            Ok(Some(record)) => {
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
        a.store.create_session(session, "m", None, None, None)
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
        let mut a = crate::supervisor::tests::agents_with(true).await;
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
        let mut a = crate::supervisor::tests::agents_with(true).await;
        let (session, task, _) = exported_task(&mut a, "peer:lab").await;
        let rs = task.request_scope;
        let child = SessionId::new();
        a.store.create_session(child, "m", Some(session), Some("kid"), None)
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
        let mut a = crate::supervisor::tests::agents_with(true).await;
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
        let mut a = crate::supervisor::tests::agents_with(true).await;
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
        let mut a = crate::supervisor::tests::agents_with(true).await;
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
            },
        );

        let fresh = a
            .network_send(session, "lab", "hello?", None, None, false)
            .await
            .unwrap_err();
        assert!(fresh.contains("inbound-only"), "{fresh}");

        let other = SessionId::new();
        a.store.create_session(other, "m", None, None, None)
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
}
