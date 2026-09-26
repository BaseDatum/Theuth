//! `Agents` supervisor: registered as [`crate::AGENT_ACTOR`]; owns one
//! session actor per live session, the sqlite store, the communication
//! [`Policy`] and peer routing (design docs 01 §2.3, 50): child results,
//! peer messages and finished jobs (14 §3, via `Settle::FinishedWithResult`)
//! are injected as [`UserInput`] / `PeerInput` into the addressee. It is also
//! the single continuation authority (47 §4): every input is admitted here
//! before routing, every runner reports its busy state, tools settle admitted
//! work here, and [`DecideYield`] is answered from that state alone.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use genai::adapter::AdapterKind;
use genai::chat::{BinarySource, ChatMessage, ChatRole, ContentPart};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, Headers, ModelIden, ServiceTarget};
use kameo::actor::{ActorRef, Spawn};
use kameo::error::SendError;
use kameo::message::{Context, Message};
use models::{Catalog, Resolved};
use runtime::{InputId, Runtime, SessionId, TurnId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tools::{
    ActivitySink, Admission, Admit, ApprovalId, ApprovalRequested, ApprovalResolved,
    InstallCommunicationAccess, JOBS_ACTOR, JobEvent, JobExit, JobId, Jobs, ListPrompts,
    ListTools, Notice, REGISTRY_ACTOR, RearmReminder, RegisterActivity, RegisterPrompt,
    RegisterTool, ReminderEvent, Risk, SetCap, SetParent, Settle, ToolRegistry,
};
use uuid::Uuid;

use crate::communication::{
    AnswerPeerRequest, CommunicationChanged, CommunicationState, DISCOVER_DEFAULT_LIMIT,
    DISCOVER_MAX_LIMIT, Generation, GetCommunication, GetPeerMessage, ListPeerMessages, ListPeers,
    PEER_TEXT_MAX, PeerActivity, PeerAddress, PeerConsumed, PeerDeliver, PeerDelivered,
    PeerEnvelope, PeerInfo, PeerIntakeFailed, PeerKind, PeerLiveState, PeerMessageChanged,
    PeerMessagePage, PeerMessageState, PeerOrigin, PeerPage, PeerReceipt, PeerRecord,
    PeerRelationship, PeerScope, PeerState, PeerTurnFinished, PeerUplink, Policy, PolicyError,
    SetCommunication, SetPeerScope, SharedPolicy, WAITS_PER_HARNESS, WAITS_PER_SESSION,
    mesh as peer_mesh, network,
};

use crate::acp_child::AcpChild;
use crate::context::{
    ContextEntry, ContextHistory, ContextOwner, ContextRemoval, ContextSnapshot, ContextTarget,
    save_blob,
};
use crate::continuation::{
    DecideYield, InputConsumed, InputDropped, InputOrigin, InputTicket, Ledger, SessionUnknown,
    TurnEntered, TurnReturned, YieldDecision,
};
use crate::loop_::{
    CancelPeerScope, CancelTurn, CompactNow, CompactionOverride, ContextHistoryNow,
    ContextItemsNow, GetToolset, HarvestNow, IsRunning, PEER_INTAKE, PeerInput, QueuedInputs,
    RedoTurn, RemoveContextNow, RestartTurn, Session, SessionState, SetCompactionOverride,
    SetPendingEffort, SetToolsetOverride, SuppressPeerInputs, SwitchModel, UndoTurn, UserInput,
    WithdrawInput, prompt_blocks, text_of,
};
use crate::peers::{
    DiscoverPeersTool, ListModelsTool, Nodes, PeerMessageTool, SendMessageTool, SpawnSubagentTool,
    node_lines,
};
use crate::skills::{
    self, AddSkillRepoTool, AddSkillTool, LoadSkillTool, RemoveSkillRepoTool, RemoveSkillTool,
    SyncSkillsTool,
};
use crate::store::communication::{AcceptExchange, CommError};
use crate::store::{self, Epoch, Store};
use crate::user::{MessageUserTool, USER_ACTOR, UserChannels};
use crate::{
    AGENT_ACTOR, AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentEvent, AgentInfo,
    ArchiveSession,
    AgentState, AncestorInfo, Ancestors, BroadcastOutcome, BroadcastReceipt, BroadcastRequest,
    BroadcastRow, BroadcastTargets, Cancel, Compact, CompactStyle, ContextItems, Delivery,
    SessionActivity,
    DeleteSession, EditTurn, ForkSession, GetCompactStyle, GetMaxSalvages, GetMaxToolRounds,
    GetStreamIdle, GetSubagentModel, GetSubagentTimeout, GetTitleModel, GetTodos, GetVisionModel, Harvest, GetEscalateModel, SetEscalateModel, EscalateModel, SideCall, SideReply,
    HistoryEntry, ListAcp, ListAgents,
    ListLogins, ListModels, ListProviders, ListQueued, ListRegistry, ListSessions, ListSkills,
    LoadSession, LoadSkill, Loaded, Login, LoginFinished, LoginInfo, Logout, ModelInfo,
    MarkDesk, NewRootSession, NewSession, PinSession, ProviderInfo, QueuedInput, ReadSession, Redo, Regenerate, RemoveAcp,
    RemoveContext, RemoveProvider, RemoveSkill, RemoveSkillRepo, RenameSession, SearchRefresh,
    SearchSessionPage, SendInput, SessionRead, SessionSummary, SetCompactStyle, SetCompaction, SetEffort,
    SetMaxSalvages,
    SetMaxToolRounds, SetModel, SetStreamIdle, SetSubagentModel, SetSubagentTimeout, SetTitleModel, SetToolset,
    SetVisionModel, ShowPrompt, SkillBody, SkillMeta,
    Steer, SyncSkills, Undo, UseSkill, Withdraw, effort_of,
};
use crate::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
use crate::{attach, frame, repl, search, spill};
use crate::{ListProjectStyles, SetProjectStyle, SetSessionProject, project};
use crate::{DurableStore, SessionCostQuery, UnloadSession};
use crate::lifecycle::{
    Authorize, Authorized, CleanupStatus, CloseResult, CreateSession, LeaveSession, PrivateOwner,
    PrivateRoot, PrivateTrees, ResumePrivate, SessionOpen,
};
use crate::private::Scratch;
use tools::{
    CloseTree, CloseTreeAll, OpenTree, ParticipantsReady, Retention, SessionResources, TreeOpened,
};

/// The supervisor actor. Look up with `ActorRef::<Agents>::lookup(AGENT_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Agents {
    pub(crate) rt: Runtime,
    cfg: Arc<AgentConfig>,
    /// `cfg.model` as last set by [`SetModel`]; new sessions start on it.
    default_model: String,
    /// `cfg.effort` as last set by [`SetEffort`]; new sessions start on it.
    default_effort: Option<String>,
    /// Live default compaction style; cloned into each session so [`SetCompactStyle`] is in-process.
    compact_style: Arc<RwLock<CompactStyle>>,
    /// Live title model (07 §15); `None` = each session's own model. Shared with every session.
    title_model: Arc<RwLock<Option<Resolved>>>,
    /// Live preferred subagent model (`agent.subagents.model`); `None` = `agent.model`.
    /// Children spawned without `model` use it; the system prompt names it. Shared with every session.
    subagent_model: Arc<RwLock<Option<Resolved>>>,
    /// Live vision fallback model (52 §3.1); `None` = no fallback. Shared with
    /// every native session once the loop consults it; snapshot, never hold
    /// the lock across an await.
    pub(crate) vision_model: Arc<RwLock<Option<Resolved>>>,
    /// Live tier-2 model of the decision oracle (57 §0.9); `None` = the title
    /// model, then each session's own model. Shared with every session.
    escalate_model: Arc<RwLock<Option<Resolved>>>,
    /// Why `agent.vision_model` from the config file was not selected at
    /// startup (invalid id or capability; 52 §3.1). The file is left as is
    /// and only the fallback is off; cleared by a successful [`SetVisionModel`].
    /// Retained here (logged at error level too) until the next wave's
    /// vision progress event carries it to the UI; no session exists at
    /// startup for `AgentEvent::Error`.
    pub vision_error: Option<String>,
    catalog: Arc<Catalog>,
    /// Python runtime workers; `None` when the `monty` binary was not found (design doc 08 §9).
    pool: Option<Arc<monty_pool::Pool>>,
    /// `configured(genai_namespace)`: whether genai can authenticate it.
    configured: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub(crate) store: Store,
    client: Client,
    auth: Auth,
    sessions: HashMap<SessionId, Live>,
    /// Sessions with a turn in flight (from `TurnStarted`/`TurnFinished`).
    running: HashSet<SessionId>,
    /// The communication policy (50 §2.1, §8), shared read-only with the
    /// tool registry; every mutation and admission happens in this mailbox.
    pub(crate) communication: SharedPolicy,
    /// A2A request/subscription state (50 §6); handlers live in `communication::network`.
    pub(crate) network: network::NetworkState,
    /// Exact reply waiters (50 §5.3): awaited message id → its waiter.
    /// Bounded by [`WAITS_PER_SESSION`] / [`WAITS_PER_HARNESS`]; removed on
    /// reply, timeout, cancel, disable and deletion.
    waits: HashMap<Uuid, Wait>,
    /// Turns bound to an awaited request (50 §5.3): `(recipient, turn)` →
    /// the message id that turn consumed while its sender was waiting. The
    /// turn's final text answers exactly that request.
    bound: HashMap<(SessionId, TurnId), Uuid>,
    /// Final text per finished turn, from [`PeerTurnFinished`] (arrives
    /// before `TurnFinished`); `child_finished` takes it by turn.
    turn_text: HashMap<(SessionId, TurnId), String>,
    /// Child results already answered explicitly, by `(child, turn)` → the
    /// session that asked (50 §7 dedup): the parent's own answered request
    /// blocks a second injection of the same result; a sibling's does not.
    delivered: HashMap<(SessionId, TurnId), SessionId>,
    /// Exchange ids handed to a runner in this process and not yet settled:
    /// a retried `request_id` must not queue the same row twice.
    enqueued: HashSet<Uuid>,
    /// Concurrent child turns.
    permits: Arc<Semaphore>,
    /// Every running child turn: `started_at` always; permit + runtime timer
    /// only for the spawn turn started by [`SpawnChild`].
    child_turns: HashMap<SessionId, ChildTurn>,
    /// Children whose current turn was cancelled by the runtime cap.
    timed_out: HashSet<SessionId>,
    /// Children spawned with a `tools` ceiling ([`SpawnChild::cap`]), for
    /// the board row; the registry enforces it ([`SetCap`]).
    caps: HashMap<SessionId, Risk>,
    /// Roots a theuth on another engine opened over the node conn (67 §8):
    /// the parent's session id and engine id from `session/new` `_meta`.
    /// `parent` for such a root is that session, reached by [`PeerUplink`].
    remote_parent: HashMap<SessionId, RemoteParent>,
    /// Children spawned by a turn an A2A request scope owns (50 §6.3): the
    /// child's first turn and its result are that scope's work (the durable
    /// `child:<id>` row) until [`Self::child_finished`] hands it off. A
    /// cancelled scope stops the child and drops the result instead.
    owned_children: HashMap<SessionId, OwnedChild>,
    /// Last `TurnFinished` per session this process saw: `(stop_reason, when)`.
    last_stop: HashMap<SessionId, (String, DateTime<Utc>)>,
    /// Last input routed into each session by this process (62 §0.5); with
    /// `last_stop` and the turn start it makes `SessionActivity.last_active`.
    last_input: HashMap<SessionId, DateTime<Utc>>,
    /// Unanswered approvals per session: `(id, tool)` (design doc 19 §2.1).
    approvals: HashMap<SessionId, (ApprovalId, String)>,
    /// Session search index (design doc 26), rebuilt after the rows change.
    search: Arc<search::Cache>,
    /// Registered nodes (32 §6.5); shared with the `spawn_subagent` tool's description.
    nodes: Nodes,
    /// Activity ledger and continuation state (47 §4); every transition is
    /// applied synchronously inside a handler, which is the linearization point.
    /// Sessions whose row already had history when this process first
    /// touched them are registered `resumed`: a previous owner's jobs,
    /// readers and children cannot be proven stopped (47 §4.5), and nothing
    /// short of producer reconciliation (not built) could prove it, so they
    /// never continue automatically in this process. Ordinary work, results
    /// and the todo card are unaffected; new sessions use fresh accounting.
    ledger: Ledger,
    /// Open private root trees (51 §4), by root; kept through teardown so
    /// `PrivateTrees` and a late `Authorize` see `closing`.
    private: HashMap<SessionId, PrivateRoot>,
    /// Member of an open private tree → its root; removed at close.
    private_member: HashMap<SessionId, SessionId>,
    /// Every id ever closed privately in this process: `route`/`session` on
    /// one refuses instead of creating a durable row (51 §4.4).
    closing: HashSet<SessionId>,
    /// Lifecycle generation (51 §8.2): bumped per private open and close.
    generation: u64,
    /// The durable capability (data dir, generation 0) every non-private
    /// session runs under.
    durable_resources: Arc<SessionResources>,
    /// Owner-gone grace before a private root closes itself (51 §4.3).
    private_grace: Duration,
    /// The mesh transport peer messages to sessions owned elsewhere travel
    /// on (69 P); fetched from the replica actor on first need, injected by
    /// tests. `None` until the mesh is up: every such message then fails
    /// `OwnerUnreachable`.
    transport: Option<Arc<dyn replica::Transport>>,
    /// Who a session speaks as when it is not theuth (70 DESK-PROTOCOL): the
    /// roster installs it with [`InstallIdentitySource`]. `None`: every
    /// session is theuth's.
    identity: Option<Arc<dyn IdentitySource>>,
}

/// One pending `send_message{await_reply}` (50 §5.3).
struct Wait {
    /// The session blocked in the tool call.
    session: SessionId,
    /// Endpoint key of the only peer whose reply may resolve it.
    to: String,
    /// Resolves with the final receipt: `replied` with the text, or the
    /// terminal state that expired it (`communication_disabled`, cancel, …).
    tx: oneshot::Sender<PeerReceipt>,
    /// The route's generation at admission; a revoked one expires the wait.
    generation: Generation,
}

/// A live session actor plus its peer identity.
struct Live {
    actor: LiveActor,
    /// `(parent, name)` for subagents.
    parent: Option<(SessionId, String)>,
}

/// What runs a session's turns: theuth's own loop, or an external ACP harness
/// (design doc 06 §3.2). Both take [`UserInput`], [`CancelTurn`] and [`IsRunning`].
#[derive(Clone)]
enum LiveActor {
    Native(ActorRef<Session>),
    Acp(ActorRef<AcpChild>),
}

/// The refusal for every `Session`-only command on an ACP child.
const NOT_NATIVE: &str = "not available for an external harness";

impl LiveActor {
    fn is_alive(&self) -> bool {
        match self {
            Self::Native(a) => a.is_alive(),
            Self::Acp(a) => a.is_alive(),
        }
    }

    /// `Err` when the actor's mailbox is closed: the input was dropped, never queued.
    async fn tell_input(&self, input: UserInput) -> anyhow::Result<()> {
        match self {
            Self::Native(a) => a.tell(input).await.map_err(|e| anyhow::anyhow!("{e}")),
            Self::Acp(a) => a.tell(input).await.map_err(|e| anyhow::anyhow!("{e}")),
        }
    }

    async fn tell_cancel(&self) {
        match self {
            Self::Native(a) => {
                let _ = a.tell(CancelTurn).await;
            }
            Self::Acp(a) => {
                let _ = a.tell(CancelTurn).await;
            }
        }
    }

    async fn kill(&self) {
        match self {
            Self::Native(a) => a.kill(),
            Self::Acp(a) => a.kill(),
        }
    }

    async fn is_running(&self) -> bool {
        match self {
            Self::Native(a) => a.ask(IsRunning).await.unwrap_or(false),
            Self::Acp(a) => a.ask(IsRunning).await.unwrap_or(false),
        }
    }

    /// The theuth loop behind this session; `Err` for an external harness.
    fn native(&self) -> Result<&ActorRef<Session>, String> {
        match self {
            Self::Native(a) => Ok(a),
            Self::Acp(_) => Err(NOT_NATIVE.into()),
        }
    }
}

/// One child turn in flight (design doc 19 §2.1).
struct ChildTurn {
    /// Concurrency permit; `None` for a follow-up turn (human steer, peer message).
    _permit: Option<OwnedSemaphorePermit>,
    /// `max_runtime_secs` timer; `None` for a follow-up turn.
    timer: Option<AbortHandle>,
    /// When the turn started (the picker's elapsed column).
    started_at: DateTime<Utc>,
}

/// The request scope a spawned child works for; `cancelled` once
/// [`Agents::cancel_peer_scope`] stopped it.
struct OwnedChild {
    scope: Uuid,
    cancelled: bool,
}

/// `ask` → `Result<(name, child), String>`: create a child session and start
/// it on `task` (queued behind the concurrency cap). Returns before the child
/// runs; its result reaches the parent as a peer message from `session:<child>`
/// through the durable intake (50 §7), when the child→parent route is on.
#[derive(Clone, Debug)]
pub struct SpawnChild {
    /// Owning session.
    pub parent: SessionId,
    /// Requested peer name; generated when `None`, uniquified within the parent.
    pub name: Option<String>,
    /// First input of the child.
    pub task: String,
    /// External harness (`agents.yaml` key) or registered node (32 §6.5); `None` runs a theuth subagent (design doc 06 §3.2).
    pub agent: Option<String>,
    /// Working directory for an external harness's `session/new`; default: theuth's cwd.
    pub cwd: Option<PathBuf>,
    /// Child model (`provider/model`); default `agent.subagents.model`, then `agent.model`.
    pub model: Option<String>,
    /// Child reasoning effort; default `agent.subagents.effort`, then `agent.effort`.
    pub effort: Option<String>,
    /// Risk ceiling for the child and its descendants (12 §5); `None` = no
    /// cap. Installed on the registry with [`SetCap`] and shown on the board.
    pub cap: Option<Risk>,
}

/// `ask` → `Result<Delivered, String>`: route a peer message (50 §5.2). The
/// exchange is persisted (`accepted`) before the reply; `Err` is a refused
/// admission (unknown address, disabled route, over quota, oversized,
/// revoked admission, conflicting retry) and nothing was sent.
#[derive(Clone, Debug)]
pub struct Deliver {
    /// Authenticated sender (`ToolCx::session`).
    pub from: SessionId,
    /// `session:<uuid>`, `peer:<name>`, `parent` or a direct child's name.
    pub to: String,
    /// Body; redacted here, at most [`PEER_TEXT_MAX`] bytes after redaction.
    pub text: String,
    /// Register an exact waiter for the answer.
    pub await_reply: bool,
    /// The local message this one answers.
    pub reply_to: Option<Uuid>,
    /// Sender's idempotency key; the minted id when `None`.
    pub request_id: Option<String>,
    /// The generation token the tool call was admitted under, from
    /// `tools::communication::current()`; a cancelled one is refused (50 §8).
    pub admitted: CancellationToken,
}

/// Reply to [`Deliver`].
#[derive(Debug, kameo::Reply)]
pub struct Delivered {
    /// The durable receipt at acceptance.
    pub receipt: PeerReceipt,
    /// With `await_reply`: resolves once with the final receipt.
    pub reply: Option<oneshot::Receiver<PeerReceipt>>,
}

/// `tell`: the awaiting tool call timed out; drop the waiter (the accepted
/// message stands, a late reply is delivered as ordinary input).
#[derive(Clone, Copy, Debug)]
pub struct DropWait(pub Uuid);

/// `ask` → `Vec<Result<String, String>>`: resolve each address relative to
/// `from` (62 §0.8) to the canonical address a [`PeerReceipt`] carries
/// (`session:<uuid>` | `peer:<name>`), or the same refusal [`Deliver`] would
/// give. Nothing is sent; `send_message` resolves its whole fan-out first.
#[derive(Clone, Debug)]
pub struct Resolve {
    /// Authenticated sender (`ToolCx::session`).
    pub from: SessionId,
    /// Addresses as the model wrote them, in order.
    pub to: Vec<String>,
}

/// `ask` → `String`: the child's identity line only (design doc 07 §10 — the
/// roster and static guidance live in [`frame::baseline`]). Empty for a root session.
#[derive(Clone, Copy, Debug)]
pub struct PeerContext {
    /// Session.
    pub session: SessionId,
}

/// The identity block that leads a session's system prompt in theuth's place
/// (70 DESK-PROTOCOL): a roster agent's desk speaks as that agent, its work
/// sessions as "working for" it. `lineage` is the session first, then its
/// parent, grandparent, … to the root, so a child is placed without waiting
/// for any spawn event. `None`: the session is theuth's.
#[async_trait::async_trait]
pub trait IdentitySource: Send + Sync {
    /// The block for the session `lineage[0]`, or `None` when it is theuth's.
    async fn identity(&self, lineage: &[SessionId]) -> Option<String>;
}

/// `ask` → `()`: install the [`IdentitySource`]; installing again replaces it.
pub struct InstallIdentitySource(pub Arc<dyn IdentitySource>);

impl Message<InstallIdentitySource> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: InstallIdentitySource, _: &mut Context<Self, Self::Reply>) {
        self.identity = Some(m.0);
    }
}

/// `ask` → `Option<String>`: the session's identity block ([`IdentitySource`]
/// over its lineage); `None` for a session of theuth's.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    /// Session.
    pub session: SessionId,
}

impl Message<Identity> for Agents {
    type Reply = Option<String>;

    async fn handle(&mut self, m: Identity, _: &mut Context<Self, Self::Reply>) -> Option<String> {
        self.identity_of(m.session).await
    }
}

impl Agents {
    /// [`IdentitySource::identity`] over `session`'s lineage; `None` without a
    /// source or for a session of theuth's.
    async fn identity_of(&self, session: SessionId) -> Option<String> {
        let source = self.identity.as_ref()?;
        let mut lineage = vec![session];
        let mut cur = session;
        while let Some((parent, _)) = self.sessions.get(&cur).and_then(|l| l.parent.as_ref()) {
            if lineage.contains(parent) {
                break;
            }
            lineage.push(*parent);
            cur = *parent;
        }
        source.identity(&lineage).await
    }
}

/// `ask` → `String`: `"Subagents: a (running), b (idle)"` or `""` — harness
/// truth for the compaction head (design doc 07 §8.4).
#[derive(Clone, Copy, Debug)]
pub struct Roster {
    /// Session whose children to list.
    pub session: SessionId,
}

/// `ask` → `()`. Registers (or replaces) a node the model may spawn on with
/// `spawn_subagent{agent: name}` (32 §6.5; the `RegisterUserSink` shape).
#[derive(Clone, Debug)]
pub struct RegisterNode {
    /// The name the model uses; one namespace with `agents.yaml`.
    pub name: String,
    /// The description shown after the name, e.g. `linux · theuth 0.4.1` or `macos · offline`.
    pub describe: String,
    /// Opens one connection per remote child.
    pub dialer: Arc<dyn acp::Dialer>,
}

/// `ask` → `()`. Forgets a node; a live child on it is unaffected.
#[derive(Clone, Debug)]
pub struct UnregisterNode {
    /// The registered name.
    pub name: String,
}

/// `ask` → `Result<BroadcastReceipt, String>`: one human message to many
/// sessions with a per-row receipt (62 §0.1, §3.3). `Err` only for the
/// target cap; every per-session failure is a row.
#[derive(Clone, Debug)]
pub struct Broadcast(pub BroadcastRequest);

/// `ask` → `Vec<AgentInfo>`: every session with a live actor or a running
/// job, plus every root; `depth` from its own root, 0 for roots (62 §3.2).
/// A root reports `parent == id`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListActivity;

/// `tell` (internal): a permit was acquired; run the child's first turn.
struct StartChildTurn {
    child: SessionId,
    permit: OwnedSemaphorePermit,
    text: String,
    /// Admitted by [`SpawnChild`] before the permit wait.
    ticket: InputTicket,
}

/// `tell` (internal): a child turn hit `max_runtime_secs`.
struct ChildTimedOut(SessionId);

const ADJECTIVES: [&str; 8] = [
    "quick", "calm", "bold", "keen", "brisk", "neat", "wise", "swift",
];
const NOUNS: [&str; 8] = [
    "otter", "heron", "fox", "lynx", "finch", "hare", "wren", "elk",
];

impl Agents {
    /// Resolve a config/row id; on failure keep it raw and carry the message
    /// for the session's next round to surface.
    fn resolve_model(&self, id: &str) -> (Resolved, Option<String>) {
        match self.catalog.resolve(id, &*self.configured) {
            Ok(r) => (r, None),
            Err(e) => (
                Resolved {
                    id: id.to_owned(),
                    genai: id.to_owned(),
                    provider: None,
                    model: None,
                },
                Some(e),
            ),
        }
    }

    /// Live actor for `id`, resuming from the store (or creating the row) when
    /// needed. `acp` = `(harness, cwd)` runs a new child on an external harness
    /// (design doc 06 §3.2); a row whose `agent` column is set resumes on one.
    /// `pick` = `(model, effort)` chosen by the spawning parent for a new child
    /// (01 §2.4); `None` entries fall back to the subagent config.
    async fn session(
        &mut self,
        id: Option<SessionId>,
        parent: Option<(SessionId, &str)>,
        acp: Option<(String, Option<PathBuf>, Option<Risk>)>,
        pick: (Option<&str>, Option<&str>),
    ) -> anyhow::Result<(SessionId, LiveActor)> {
        let id = id.unwrap_or_default();
        if let Some(live) = self.sessions.get(&id).filter(|l| l.actor.is_alive()) {
            return Ok((id, live.actor.clone()));
        }
        // 51 §4.4: a closed private id is never recreated as a durable row.
        if self.closing.contains(&id) {
            anyhow::bail!("session closed");
        }
        // A private member, or a new child of one, lives in its root's tree.
        let private_root = self
            .private_member
            .get(&id)
            .or_else(|| parent.and_then(|p| self.private_member.get(&p.0)))
            .copied();
        let store = private_root
            .and_then(|r| self.private.get(&r))
            .map_or_else(|| self.store.clone(), |p| (*p.store).clone());
        let resources = private_root
            .and_then(|r| self.private.get(&r))
            .map_or_else(|| self.durable_resources.clone(), |p| p.resources.clone());
        let row = store.session(id).await?;
        let harness = acp
            .as_ref()
            .map(|a| a.0.clone())
            .or_else(|| row.as_ref().and_then(|r| r.agent.clone()));
        if let Some(name) = harness {
            if private_root.is_some() {
                anyhow::bail!("remote agents are unavailable in a private session");
            }
            let (cwd, cap) = acp.map_or((None, None), |a| (a.1, a.2));
            let actor = self
                .acp_session(id, row, parent, name, cwd, pick, cap)
                .await?;
            return Ok((id, actor));
        }
        let (model, effort, state, parent, resumed) = match row {
            Some(row) => {
                let (epoch, head, history) = store.frame(id).await?;
                // A row with history belonged to an earlier process whose work
                // cannot be proven stopped (47 §4.5), see `Agents::ledger`.
                let resumed = epoch > 0 || !history.is_empty();
                let mut state = SessionState::resume(epoch, head, history);
                // 47 §5.2: the structured plan, not a parsed summary.
                let todos = store.todos(id).await?;
                state.todos = todos.items;
                state.todo_version = todos.version;
                (
                    self.resolve_model(&row.model),
                    row.effort,
                    state,
                    row.parent.zip(row.name),
                    resumed,
                )
            }
            None => {
                let sub = &self.cfg.subagents;
                let preferred = self
                    .subagent_model
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .map(|r| r.id.clone());
                let (model, effort) = match parent {
                    Some(_) => (
                        pick.0
                            .or(preferred.as_deref())
                            .unwrap_or(&self.default_model),
                        pick.1
                            .map(str::to_owned)
                            .or_else(|| sub.effort.clone())
                            .or_else(|| self.default_effort.clone()),
                    ),
                    None => (
                        pick.0.unwrap_or(self.default_model.as_str()),
                        pick.1
                            .map(str::to_owned)
                            .or_else(|| self.default_effort.clone()),
                    ),
                };
                let model = self.resolve_model(model);
                store
                    .create_session(id, &model.0.id, parent.map(|p| p.0), parent.map(|p| p.1), None)
                    .await?;
                if let Some(e) = &effort {
                    store.set_effort(id, Some(e)).await?;
                }
                (
                    model,
                    effort,
                    SessionState::resume(0, None, Vec::new()),
                    parent.map(|(p, n)| (p, n.to_string())),
                    false,
                )
            }
        };
        let depth = store.depth(id).await?;
        self.inherit_approvals(id, parent.as_ref().map(|p| p.0))
            .await;
        // A private tree is never mirrored into the peer topology (51 §4.2);
        // `mirror` reads the durable store, so for a member this is a no-op.
        self.mirror(id).await;
        let actor = Session::spawn(Session::new(
            id,
            depth,
            model,
            effort.as_deref().and_then(effort_of),
            state,
            self.cfg.clone(),
            self.compact_style.clone(),
            self.title_model.clone(),
            self.vision_model.clone(),
            self.escalate_model.clone(),
            self.subagent_model.clone(),
            store,
            self.client.clone(),
            self.auth.clone(),
            self.rt.clone(),
            self.catalog.clone(),
            self.pool.clone(),
            self.permits.clone(),
            resources,
        ));
        let actor = LiveActor::Native(actor);
        self.ledger
            .register(id, parent.as_ref().map(|p| p.0), resumed);
        self.sessions.insert(
            id,
            Live {
                actor: actor.clone(),
                parent,
            },
        );
        // A new child of a private tree joins it: the registry learns the
        // superset so its resources and every owner's routing cover it.
        if let Some(root) = private_root
            && !self.private_member.contains_key(&id)
        {
            self.private_member.insert(id, root);
            if let Some(p) = self.private.get_mut(&root) {
                p.members.push(id);
            }
            self.open_tree(root).await;
        }
        Ok((id, actor))
    }

    /// Make `id`'s persisted lineage known to the policy (50 §2.1: ancestry
    /// checks use stored topology, so a lazily resumed or freshly created
    /// row never routes with a dangling link). Idempotent; a row that cannot
    /// be read stays unknown and every route for it fails closed.
    async fn mirror(&mut self, id: SessionId) {
        if self.communication.0.read().root_of(id).is_ok() {
            return;
        }
        let lineage = match self.store.lineage(id).await {
            Ok(l) => l,
            // No row: nothing to mirror; every route for `id` fails closed.
            Err(e) if e.downcast_ref::<CommError>() == Some(&CommError::NotFound) => return,
            Err(e) => {
                tracing::error!(session = %id, "lineage: {e:#}");
                return;
            }
        };
        let mut p = self.communication.0.write();
        let root = lineage.root();
        if let Err(e) = p.record_root(root, lineage.scope) {
            tracing::error!(session = %root, "record_root: {e}");
            return;
        }
        if lineage.alias.is_some() {
            let _ = p.set_scope(root, lineage.scope, lineage.alias.clone());
        }
        // Root first, then each child of the one just recorded.
        for pair in lineage.chain.windows(2).rev() {
            if let Err(e) = p.record_child(pair[0], pair[1]) {
                tracing::error!(session = %pair[0], "record_child: {e}");
                return;
            }
        }
    }

    /// Spawn an [`AcpChild`] for `id` on harness `name` — an `agents.yaml`
    /// entry, else a registered node (32 §6.5) — a new row (model column = the
    /// harness name) or a resume of `row` with its stored ACP session id
    /// (design doc 06 §3.2, §3.6). For a node, `pick` (model, effort) and
    /// `cap` travel in `session/new` `_meta` with the parent's identity (67
    /// §8); a local harness picks its own.
    #[allow(clippy::too_many_arguments)]
    async fn acp_session(
        &mut self,
        id: SessionId,
        row: Option<SessionSummary>,
        parent: Option<(SessionId, &str)>,
        name: String,
        cwd: Option<PathBuf>,
        pick: (Option<&str>, Option<&str>),
        cap: Option<Risk>,
    ) -> anyhow::Result<LiveActor> {
        let entries = acp::load(&self.rt.data_dir)?;
        let (harness, engine) = crate::peers::parse_agent(&name).map_err(anyhow::Error::msg)?;
        let node = self
            .nodes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(engine)
            .map(|(_, d)| d.clone());
        let (transport, fallback_model, meta, model) = match (harness, entries.get(engine), node) {
            // A local harness: `model` through its model option after `session/new`.
            (None, Some(entry), _) => (
                acp::Transport::Process(entry.config()),
                entry.model.clone(),
                None,
                pick.0.map(str::to_owned),
            ),
            // A node: native there, or its own `harness`; either way the
            // instructions ride in `session/new` `_meta`.
            (_, _, Some(dialer)) => {
                let meta = acp::SessionMeta {
                    model: pick.0.map(str::to_owned),
                    effort: pick.1.map(str::to_owned),
                    tools: cap,
                    parent: parent.map(|p| p.0.to_string()),
                    parent_engine: dialer.caller_id(),
                    harness: harness.map(str::to_owned),
                };
                (acp::Transport::Dial(dialer), None, Some(meta), None)
            }
            _ => anyhow::bail!(
                "{}",
                crate::peers::unknown(engine, &entries, &node_lines(&self.nodes))
            ),
        };
        // An existing row belonged to an earlier process (see `Agents::ledger`).
        let resumed = row.is_some();
        let (parent, cwd, resume) = match row {
            Some(row) => (
                row.parent.zip(row.name),
                row.cwd.map(PathBuf::from),
                self.store.acp_session(id).await?,
            ),
            None => {
                self.store.create_session(id, &name, parent.map(|p| p.0), parent.map(|p| p.1), None)
                    .await?;
                self.store.set_acp(id, &name, cwd.as_deref()).await?;
                (parent.map(|(p, n)| (p, n.to_string())), cwd, None)
            }
        };
        let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.inherit_approvals(id, parent.as_ref().map(|p| p.0))
            .await;
        self.mirror(id).await;
        let actor = LiveActor::Acp(AcpChild::spawn(AcpChild::new(
            id,
            name,
            transport,
            cwd,
            resume,
            fallback_model,
            meta,
            model,
            self.request_deadline(),
            self.store.clone(),
            self.rt.clone(),
        )));
        self.ledger
            .register(id, parent.as_ref().map(|p| p.0), resumed);
        self.sessions.insert(
            id,
            Live {
                actor: actor.clone(),
                parent,
            },
        );
        Ok(actor)
    }

    /// Subagents inherit the parent's approval switch.
    async fn inherit_approvals(&self, id: SessionId, parent: Option<SessionId>) {
        if let Some(parent) = parent
            && let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
        {
            let _ = reg
                .tell(SetParent {
                    session: id,
                    parent,
                })
                .await;
        }
    }

    /// Route `text` (plus image `files`) into `sid` (created when unknown);
    /// errors surface as `AgentEvent::Error` and as the `Err`. A rejected
    /// attachment drops the whole input (17 §3.1). The input's ticket is
    /// admitted here, before the actor `tell`, so a yield decision handled
    /// after this sees it (47 §4.3). `delivery` says how it lands on a busy
    /// session (62 §0.3).
    async fn input(
        &mut self,
        sid: SessionId,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        origin: InputOrigin,
        delivery: Delivery,
    ) -> Result<(), String> {
        let ticket = self.ledger.admit(sid, origin, id);
        let defer = matches!(delivery, Delivery::NextTurn);
        self.route(sid, id, text, files, ticket, None, defer).await
    }

    /// [`Agents::input`] with a ticket admitted earlier (a queued child's first
    /// turn). `scope` is the A2A request scope the input belongs to (an owned
    /// child's assignment, the result of the task's own work); `None` is a
    /// private input (50 §6.3). `defer` holds a busy session's input until
    /// its turn ends (62 §0.3). A routing failure — a rejected attachment, no
    /// actor, or a `tell` into a closed mailbox — releases the ticket: no
    /// runner ever held it.
    #[allow(clippy::too_many_arguments)]
    async fn route(
        &mut self,
        sid: SessionId,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        ticket: InputTicket,
        scope: Option<Uuid>,
        defer: bool,
    ) -> Result<(), String> {
        let routed = match check_files(&files) {
            Ok(()) => self.session(Some(sid), None, None, (None, None)).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        let sent = match routed {
            Ok((_, actor)) => {
                actor
                    .tell_input(UserInput {
                        id,
                        text,
                        files,
                        ticket: Some(ticket),
                        scope,
                        defer,
                    })
                    .await
            }
            Err(e) => Err(e),
        };
        match sent {
            Ok(()) => {
                // Human and harness inputs alike (`inject` routes through here): 62 §0.5.
                self.last_input.insert(sid, Utc::now());
                Ok(())
            }
            Err(e) => {
                self.ledger.release(sid, ticket);
                self.network_input_dropped(ticket).await;
                tracing::error!(session = %sid, "session: {e:#}");
                let message = format!("{e:#}");
                self.error(sid, message.clone()).await;
                Err(message)
            }
        }
    }

    /// [`SendInput`] / [`Steer`] from a human: routed as a fresh external
    /// request (47 §4.5). A pending peer wait is never answered implicitly;
    /// the owner answers a request by id with [`AnswerPeerRequest`] (50 §5.3).
    async fn human_input(
        &mut self,
        session: Option<SessionId>,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        delivery: Delivery,
    ) -> SessionId {
        let sid = session.unwrap_or_default();
        // A failure was published as `AgentEvent::Error` on `sid` already.
        let _ = self
            .input(sid, id, text, files, InputOrigin::External, delivery)
            .await;
        sid
    }

    /// A session-scoped failure outside any turn: recorded behind the last
    /// visible message (history replaces live rows on reload), then published.
    /// A closed private id has no row anywhere and gets none (51 §4.4).
    async fn error(&self, session: SessionId, message: String) {
        if !self.closing.contains(&session)
            && let Err(e) = self.store_for(session).record_error(session, &message).await
        {
            tracing::warn!(session = %session, "record_error: {e:#}");
        }
        self.rt
            .publish(AgentEvent::Error {
                session,
                turn: None,
                message,
            })
            .await;
    }

    /// The live (or resumed) actor for `session`, with the lookup error as the
    /// caller's `Err` string.
    async fn actor(&mut self, session: SessionId) -> Result<LiveActor, String> {
        self.session(Some(session), None, None, (None, None))
            .await
            .map(|(_, actor)| actor)
            .map_err(|e| format!("{e:#}"))
    }

    /// The live actor for `session`, if any; a session with no actor has nothing queued.
    fn live(&self, session: SessionId) -> Option<&LiveActor> {
        self.sessions
            .get(&session)
            .filter(|l| l.actor.is_alive())
            .map(|l| &l.actor)
    }

    /// Wall deadline of one peer-triggered request scope (50 §8).
    pub(crate) fn request_deadline(&self) -> Duration {
        Duration::from_secs(self.cfg.subagents.max_runtime_secs)
    }

    /// Known vault values become placeholders (50 §9) before any peer
    /// persistence, event or transport; unknown literals cannot be detected.
    pub(crate) fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.auth
            .vault
            .as_deref()
            .map_or(Cow::Borrowed(text), |v| v.redact(text))
    }

    /// Inject `text` (already wrapped) into `to` and announce it. Harness-made,
    /// so `Automatic`: it never opens a new reminder budget (47 §4.5). A result
    /// of work an exported task started carries that task's `scope`, and is
    /// the scope's work from admission (50 §6.3).
    async fn inject(
        &mut self,
        to: SessionId,
        from: &str,
        text: String,
        wrapped: String,
        scope: Option<Uuid>,
    ) {
        let ticket = self.ledger.admit(to, InputOrigin::Automatic, None);
        if let Some(scope) = scope
            && !self.network_input_queued(scope, ticket).await
        {
            // The task was cancelled or settled meanwhile: a late result of
            // its work starts no turn (50 §8). Nothing is owed for it.
            self.ledger.release(to, ticket);
            tracing::info!(session = %to, "{from}: result dropped, its request scope is closed");
            return;
        }
        // A failure is already an `AgentEvent::Error` on `to`.
        let _ = self
            .route(to, None, wrapped, Vec::new(), ticket, scope, false)
            .await;
        self.rt
            .publish(AgentEvent::MessageDelivered {
                session: to,
                from: from.to_string(),
                text,
            })
            .await;
    }

    /// Resolve `to` relative to `from` (50 §5.1): `session:<uuid>` must be a
    /// stored row (never created), `parent` and a direct child's name are
    /// conveniences, `peer:<name>` is a configured remote. Unknown and
    /// malformed are one answer, so a guessed UUID learns nothing.
    async fn resolve(&mut self, from: SessionId, to: &str) -> Result<Target, String> {
        let unknown = || format!("unknown peer `{to}`");
        match PeerAddress::parse(to).ok_or_else(unknown)? {
            PeerAddress::Remote(name) => Ok(Target::Remote(name)),
            PeerAddress::Session(id) => {
                match self.store.session(id).await.map_err(|e| e.to_string())? {
                    Some(_) => {
                        self.mirror(id).await;
                        Ok(Target::Local(id))
                    }
                    None => Err(unknown()),
                }
            }
            PeerAddress::Relative(name) if name == "parent" && self.remote_parent.contains_key(&from) => {
                Ok(Target::Uplink)
            }
            PeerAddress::Relative(name) if name == "parent" => {
                let parent = match self.sessions.get(&from).and_then(|l| l.parent.as_ref()) {
                    Some((p, _)) => Some(*p),
                    None => self
                        .store
                        .session(from)
                        .await
                        .map_err(|e| e.to_string())?
                        .and_then(|r| r.parent),
                };
                parent.map(Target::Local).ok_or_else(unknown)
            }
            PeerAddress::Relative(name) => self
                .store
                .child_by_name(from, &name)
                .await
                .map_err(|e| e.to_string())?
                .map(Target::Local)
                .ok_or_else(unknown),
        }
    }

    /// [`Self::resolve`] per address, each to the canonical form
    /// (`session:<uuid>` | `peer:<name>`) or its refusal, in `to` order.
    async fn resolve_all(&mut self, from: SessionId, to: &[String]) -> Vec<Result<String, String>> {
        let mut out = Vec::with_capacity(to.len());
        for a in to {
            out.push(self.resolve(from, a).await.map(|t| match t {
                Target::Local(id) => format!("session:{id}"),
                Target::Remote(name) => format!("peer:{name}"),
                Target::Uplink => "parent".to_owned(),
            }));
        }
        out
    }

    /// A refused route as the sender reads it: its own disablement is named;
    /// another tree's existence, scope or opt-out is not disclosed (50 §2.3).
    fn route_error(&self, from: SessionId, to: SessionId, e: PolicyError) -> String {
        let same_tree = {
            let p = self.communication.0.read();
            p.root_of(from).ok() == p.root_of(to).ok()
        };
        match e {
            PolicyError::Disabled { session, .. } if session == from => {
                format!("send_message: {}: {e}", tools::communication::DISABLED)
            }
            PolicyError::Disabled { .. } if same_tree => {
                format!("send_message: {}: {e}", tools::communication::DISABLED)
            }
            PolicyError::SelfRoute(_) => "cannot message yourself".into(),
            PolicyError::NetworkDisabled => e.to_string(),
            _ => format!("unknown peer `session:{to}`"),
        }
    }

    /// Register an exact waiter for message `id` (50 §5.3), bounded per
    /// session and harness.
    fn wait(
        &mut self,
        session: SessionId,
        to: String,
        id: Uuid,
        generation: Generation,
    ) -> Result<oneshot::Receiver<PeerReceipt>, String> {
        if self.waits.len() >= WAITS_PER_HARNESS {
            return Err(format!("busy: {WAITS_PER_HARNESS} replies already awaited"));
        }
        if self.waits.values().filter(|w| w.session == session).count() >= WAITS_PER_SESSION {
            return Err(format!(
                "busy: this session already awaits {WAITS_PER_SESSION} replies"
            ));
        }
        let (tx, rx) = oneshot::channel();
        self.waits.insert(
            id,
            Wait {
                session,
                to,
                tx,
                generation,
            },
        );
        Ok(rx)
    }

    /// Hand the answer to the waiter of `original.id` (the awaited message,
    /// already `replied` in the store with the text). `false` when no live
    /// waiter exists: deliver the reply as ordinary input instead.
    pub(crate) fn settle_wait(&mut self, original: &PeerReceipt) -> bool {
        let Some(w) = self.waits.remove(&original.id) else {
            return false;
        };
        self.bound.retain(|_, id| *id != original.id);
        w.tx.send(original.clone()).is_ok()
    }

    /// Expire every wait `pred` selects with a terminal receipt naming
    /// `reason` (never a fake reply or a normal timeout).
    fn expire_waits(&mut self, reason: &str, pred: impl Fn(&Wait) -> bool) {
        let gone: Vec<Uuid> = self
            .waits
            .iter()
            .filter(|(_, w)| pred(w))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            let w = self.waits.remove(&id).expect("just listed");
            self.bound.retain(|_, b| *b != id);
            let _ = w.tx.send(PeerReceipt {
                id,
                to: w.to,
                state: PeerMessageState::Failed,
                reply_to: None,
                reply: None,
                task_id: None,
                error: Some(reason.to_owned()),
                answered_by: None,
            });
        }
    }

    /// Publish [`PeerMessageChanged`] to each local participant of `receipt`.
    pub(crate) async fn peer_changed(&self, receipt: &PeerReceipt) {
        let mut parties: Vec<SessionId> = PeerAddress::local(&receipt.to).into_iter().collect();
        match self.store.exchange_record(receipt.id).await {
            Ok(Some(rec)) => {
                if let Some(s) = PeerAddress::local(&rec.from).filter(|s| !parties.contains(s)) {
                    parties.push(s);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!(id = %receipt.id, "exchange_record: {e:#}"),
        }
        for session in parties {
            self.rt
                .publish(PeerMessageChanged {
                    session,
                    id: receipt.id,
                    state: receipt.state,
                })
                .await;
        }
    }

    /// Publish the policy change for `session` (`None` = global).
    async fn communication_changed(&self, session: Option<SessionId>) {
        let revision = self.communication.0.read().revision();
        self.rt
            .publish(CommunicationChanged { session, revision })
            .await;
    }

    /// A policy mutation that cancels generations, run behind the intake
    /// barrier (50 §8): runners hold [`PEER_INTAKE`] read across
    /// `generation.live()` + `consume_exchange`, so no consumption that saw a
    /// live generation can commit after the tokens moved, and the registry
    /// refuses new admissions from the shared tokens at once. The caller
    /// then suppresses accepted input, drops queued runner inputs and
    /// expires waits before it replies.
    pub(crate) async fn revoking<R>(&self, f: impl FnOnce(&mut Policy) -> R) -> R {
        let _barrier = PEER_INTAKE.write().await;
        f(&mut self.communication.0.write())
    }

    /// Schedule an accepted exchange into its local recipient (50 §7): the
    /// row exists and is `accepted`; the recipient must be a stored session
    /// (never created here), native when `isolated` (an A2A task scope needs
    /// native request ownership). The 47 ticket is admitted before the
    /// actor `ask`, which runs off this mailbox; a refused ask fails the
    /// exchange and releases the ticket through [`PeerIntakeFailed`].
    pub(crate) async fn enqueue_peer(
        &mut self,
        record: PeerRecord,
        generation: Generation,
        isolated: bool,
        reply_expected: bool,
    ) -> Result<(), String> {
        let to = PeerAddress::local(&record.receipt.to).ok_or("recipient is not local")?;
        if !generation.live() {
            return Err(format!(
                "{}: route revoked before intake",
                tools::communication::DISABLED
            ));
        }
        let row = self
            .store
            .session(to)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("unknown peer `session:{to}`"))?;
        if isolated && row.agent.is_some() {
            return Err("an external harness cannot own an A2A task scope".into());
        }
        // The chain bound follows the initiating request across sessions
        // (50 §8): a local sender's chain is inherited, a remote's is not.
        let sender = PeerAddress::local(&record.from);
        if self.ledger.chain_exhausted_for(sender, to) {
            return Err(format!(
                "chain limit: {} peer-triggered continuations since the last external input",
                crate::communication::CHAIN_MAX
            ));
        }
        let actor = self.actor(to).await?;
        let id = record.receipt.id;
        let scope = record.request_scope;
        let ticket = self.ledger.admit_chained(to, None, sender);
        if let Some(scope) = scope
            && !self.network_input_queued(scope, ticket).await
        {
            // The scope's work from admission until it enters a turn (50
            // §6.3); a cancelled scope takes no more input.
            self.ledger.release(to, ticket);
            return Err("canceled: the request scope is closed".into());
        }
        let me = ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten();
        tokio::spawn(async move {
            let Err(reason) = actor
                .ask_peer(PeerInput {
                    record,
                    generation,
                    ticket,
                    request_scope: scope,
                    isolated,
                    reply_expected,
                })
                .await
            else {
                return;
            };
            match me {
                Some(me) => {
                    let _ = me
                        .tell(PeerIntakeFailed {
                            session: to,
                            id,
                            ticket,
                            reason,
                        })
                        .await;
                }
                None => tracing::error!(session = %to, %id, "peer intake refused: {reason}"),
            }
        });
        Ok(())
    }

    /// Cancel one request scope in `session` (50 §8): queued input of that
    /// scope never starts, its in-flight turn is signalled, and so is every
    /// child the scope spawned (50 §6.3) — a queued first turn never starts,
    /// a running one stops, and its result is dropped by
    /// [`Self::child_finished`] whenever it lands. The cancelled turns'
    /// exact waits expire (50 §5.3). Unrelated turns, children and waits
    /// are untouched. Task rows transition through the store by their
    /// owner, only once the owned work has stopped.
    pub(crate) async fn cancel_peer_scope(
        &mut self,
        session: SessionId,
        scope: Uuid,
    ) -> Result<(), String> {
        let owner = match self.live(session) {
            Some(LiveActor::Native(a)) => a
                .tell(CancelPeerScope { scope })
                .await
                .map_err(|e| e.to_string()),
            Some(LiveActor::Acp(_)) => Err(NOT_NATIVE.into()),
            None => Ok(()),
        };
        let children: Vec<SessionId> = self
            .owned_children
            .iter()
            .filter(|(_, o)| o.scope == scope && !o.cancelled)
            .map(|(id, _)| *id)
            .collect();
        for &child in &children {
            self.owned_children
                .get_mut(&child)
                .expect("listed")
                .cancelled = true;
            // Stop control (47 §4.3): candidates admitted before this are stale.
            self.ledger.bump(child);
            match self.live(child) {
                // Its first turn runs under the inherited scope: the same
                // exact signal, in flight or still queued.
                Some(LiveActor::Native(a)) => {
                    let _ = a.tell(CancelPeerScope { scope }).await;
                }
                Some(acp @ LiveActor::Acp(_)) => acp.tell_cancel().await,
                None => {}
            }
        }
        // Only the scope's own turn awaits under `session`; a private turn
        // running there now keeps its waits.
        let owner_scoped = self.network_turn_scope(session) == Some(scope);
        self.expire_waits("cancelled", |w| {
            (owner_scoped && w.session == session) || children.contains(&w.session)
        });
        owner
    }

    /// A queued child's first turn got its permit ([`StartChildTurn`]): the
    /// scope it runs under, if a request scope owns the child (its input is
    /// registered as that scope's work, 50 §6.3). `Err` when the scope was
    /// cancelled while it waited: the ticket is released, the `child:<id>`
    /// work settled, and nothing is routed.
    async fn claim_child_turn(
        &mut self,
        child: SessionId,
        ticket: InputTicket,
    ) -> Result<Option<Uuid>, ()> {
        let Some(&OwnedChild { scope, cancelled }) = self.owned_children.get(&child) else {
            return Ok(None);
        };
        if cancelled || !self.network_input_queued(scope, ticket).await {
            self.owned_children.remove(&child);
            self.ledger.release(child, ticket);
            self.network_settle_work(scope, &format!("child:{child}"))
                .await;
            return Err(());
        }
        Ok(Some(scope))
    }

    /// Disable follow-through for `subtree` (`None` = everything) after the
    /// policy barrier moved (50 §2.2, §8): accepted-unconsumed input is
    /// suppressed durably, live runners drop their queued peer inputs,
    /// affected waits expire with `communication_disabled`, and the network
    /// module is told. Suppressed input never revives on re-enable.
    async fn revoke(&mut self, subtree: Option<SessionId>) {
        const REASON: &str = tools::communication::DISABLED;
        match self.store.suppress_exchanges(subtree, REASON).await {
            Ok(receipts) => {
                for r in &receipts {
                    self.peer_changed(r).await;
                }
            }
            Err(e) => tracing::error!("suppress_exchanges: {e:#}"),
        }
        // Live runners in the subtree (the ledger mirrors the same parent links).
        let affected: Vec<SessionId> = match subtree {
            None => self.sessions.keys().copied().collect(),
            Some(root) => self.ledger.subtree(root),
        };
        for s in affected {
            if let Some(LiveActor::Native(a)) = self.live(s) {
                let _ = a.tell(SuppressPeerInputs).await;
            }
        }
        self.expire_waits(REASON, |w| !w.generation.live());
        self.network_revoked(subtree, REASON).await;
    }

    /// A child's turn ended: hand its result on through the same durable
    /// intake as any peer message (50 §7), then release the child. The turn
    /// bound to an awaited request answers exactly that request first, to
    /// whichever session asked (`reply:<turn>`, 50 §5.3); the parent gets
    /// the result as uncorrelated input from `session:<child>`
    /// (`result:<turn>`) unless it already took it as that reply. Both keys
    /// are durable dedup keys, so a repeated finish for the same turn never
    /// injects twice, across restarts too. Off routes deliver nothing while
    /// the transcript stays inspectable. The text is this turn's own final
    /// result ([`PeerTurnFinished`]); a missing one is an error, never
    /// another turn's text. A child a request scope owns (50 §6.3) hands
    /// its result off as that scope's input — or, once the scope was
    /// cancelled, records it `failed` and wakes nobody — and its `child:<id>`
    /// work settles only here, after the child left the ledger, so the task
    /// never completes or cancels while the child could still act. The
    /// child stays counted in the ledger until this ran
    /// (`Ledger::handed_off`), and the parent's ticket is admitted inside
    /// `enqueue_peer` before the child is released (47 §3.2).
    async fn child_finished(&mut self, child: SessionId, turn: TurnId, stop_reason: &str) {
        // 06 §3.5 (2026-09-24): harness work after its prompt returned;
        // no result exists and nothing is owed to the parent.
        if stop_reason == crate::acp_child::HARNESS_CONTINUED {
            self.ledger.handed_off(child, turn);
            return;
        }
        let owned = self.owned_children.remove(&child);
        match self.sessions.get(&child).and_then(|l| l.parent.clone()) {
            Some((parent, _)) => {
                self.hand_off(child, turn, parent, stop_reason, owned.as_ref())
                    .await;
            }
            None => {
                self.turn_text.remove(&(child, turn));
            }
        }
        self.ledger.handed_off(child, turn);
        if let Some(o) = owned {
            self.network_settle_work(o.scope, &format!("child:{child}"))
                .await;
        }
    }

    /// The delivery half of [`Self::child_finished`].
    async fn hand_off(
        &mut self,
        child: SessionId,
        turn: TurnId,
        parent: SessionId,
        stop_reason: &str,
        owned: Option<&OwnedChild>,
    ) {
        let request = self.bound.remove(&(child, turn));
        let answered = self.delivered.remove(&(child, turn));
        let Some(mut text) = self.turn_text.remove(&(child, turn)) else {
            tracing::error!(session = %child, %turn, "child turn finished without its PeerTurnFinished result");
            self.error(
                parent,
                format!(
                    "result of subagent session:{child} (turn {turn}) is missing; inspect its transcript"
                ),
            )
            .await;
            return;
        };
        if self.timed_out.remove(&child) {
            text = format!(
                "[timed out after {}s, stop={}]\n{text}",
                self.cfg.subagents.max_runtime_secs,
                attr(stop_reason)
            );
        }
        // Bound like any tool result; the child's id is the spill file stem (36 §4.1).
        let text = spill::cap(
            parent,
            &child.to_string(),
            &text,
            self.cfg.max_tool_output_chars,
            &self.rt.data_dir.join("spill"),
        );
        let text = self.redact(&text).into_owned();
        // The exact reply goes to whoever is still waiting for it — the
        // parent, or a sibling that asked the child directly.
        let mut parent_answered = answered == Some(parent);
        if let Some(request) = request
            && let Some(requester) = self.answer_bound(child, turn, request, &text).await
        {
            parent_answered |= requester == parent;
        }
        let generation = self.communication.0.write().route(child, parent);
        let Ok(generation) = generation else {
            tracing::info!(session = %child, %parent, "child result not delivered: communication off");
            return;
        };
        let (from_key, to_key) = (format!("session:{child}"), format!("session:{parent}"));
        let request_id = format!("result:{turn}");
        let receipt = match self
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from_key,
                recipient: &to_key,
                sender_session: Some(child),
                recipient_session: Some(parent),
                request_id: Some(&request_id),
                reply_to: None,
                request_scope: owned.map(|o| o.scope),
                generation: generation.revision() as i64,
                body: &text,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(session = %child, "child result exchange: {}", comm_err(e));
                return;
            }
        };
        if receipt.state != PeerMessageState::Accepted || !self.enqueued.insert(receipt.id) {
            // Durable dedup: this turn's result was already accounted for.
            return;
        }
        self.peer_changed(&receipt).await;
        if owned.is_some_and(|o| o.cancelled) {
            // The scope was cancelled while the child ran: its result is
            // recorded, never a continuation of the cancelled request.
            self.enqueued.remove(&receipt.id);
            match self.store.fail_exchange(receipt.id, "cancelled").await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::error!(id = %receipt.id, "fail_exchange: {e:#}"),
            }
            return;
        }
        if parent_answered {
            // The parent already holds this result as the reply to its own
            // request (50 §7): the durable record exists, consumed without a
            // second turn.
            let revision = generation.revision() as i64;
            if let Ok(Some(r)) = self
                .store
                .consume_exchange_direct(receipt.id, parent, revision)
                .await
            {
                self.peer_changed(&r).await;
            }
            self.enqueued.remove(&receipt.id);
            return;
        }
        match self.store.peer_record(parent, receipt.id).await {
            Ok(Some(record)) => {
                if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
                    self.enqueued.remove(&receipt.id);
                    match self.store.fail_exchange(receipt.id, &reason).await {
                        Ok(r) => self.peer_changed(&r).await,
                        Err(e) => tracing::error!(id = %receipt.id, "fail_exchange: {e:#}"),
                    }
                    self.error(parent, format!("subagent result not delivered: {reason}"))
                        .await;
                }
            }
            Ok(None) => tracing::error!(id = %receipt.id, "accepted child result has no record"),
            Err(e) => tracing::error!(id = %receipt.id, "peer_record: {e:#}"),
        }
    }

    /// Answer the live waiter of `request` with `text` as `child`'s reply
    /// (`reply:<turn>`, 50 §5.3): the store verifies the reversed endpoints
    /// and marks the request `replied`; the waiter takes the receipt, or —
    /// when its tool call is already gone — the reply reaches the requester
    /// as ordinary input. `Some(requester)` when a reply was recorded;
    /// `None` with no live waiter or an off route (the result is then the
    /// parent's alone).
    async fn answer_bound(
        &mut self,
        child: SessionId,
        turn: TurnId,
        request: Uuid,
        text: &str,
    ) -> Option<SessionId> {
        let requester = self.waits.get(&request)?.session;
        let generation = match self.communication.0.write().route(child, requester) {
            Ok(g) => g,
            Err(e) => {
                tracing::info!(session = %child, %requester, "bound reply not delivered: {e}");
                return None;
            }
        };
        let (from_key, to_key) = (format!("session:{child}"), format!("session:{requester}"));
        let request_id = format!("reply:{turn}");
        let receipt = match self
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from_key,
                recipient: &to_key,
                sender_session: Some(child),
                recipient_session: Some(requester),
                request_id: Some(&request_id),
                reply_to: Some(request),
                // Inherited from the request by the store (50 §6.3).
                request_scope: None,
                generation: generation.revision() as i64,
                body: text,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(session = %child, %request, "bound reply exchange: {}", comm_err(e));
                return None;
            }
        };
        if receipt.state != PeerMessageState::Accepted || !self.enqueued.insert(receipt.id) {
            return Some(requester);
        }
        self.peer_changed(&receipt).await;
        let original = match self.store.exchange_receipt(request).await {
            Ok(Some(r)) => r,
            _ => return Some(requester),
        };
        let revision = generation.revision() as i64;
        if self.settle_wait(&original) {
            if let Ok(Some(r)) = self
                .store
                .consume_exchange_direct(receipt.id, requester, revision)
                .await
            {
                self.peer_changed(&r).await;
            }
            self.enqueued.remove(&receipt.id);
            self.peer_changed(&original).await;
            return Some(requester);
        }
        // The awaiting call timed out under us: ordinary correlated input.
        match self.store.peer_record(requester, receipt.id).await {
            Ok(Some(record)) => {
                if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
                    self.enqueued.remove(&receipt.id);
                    match self.store.fail_exchange(receipt.id, &reason).await {
                        Ok(r) => self.peer_changed(&r).await,
                        Err(e) => tracing::error!(id = %receipt.id, "fail_exchange: {e:#}"),
                    }
                }
            }
            Ok(None) => tracing::error!(id = %receipt.id, "accepted reply has no record"),
            Err(e) => tracing::error!(id = %receipt.id, "peer_record: {e:#}"),
        }
        Some(requester)
    }
}

/// Where a peer address points after [`Agents::resolve`].
enum Target {
    Local(SessionId),
    Remote(String),
    /// `parent` of a root a remote theuth opened (67 §8): out over its conn.
    Uplink,
}

/// Who opened a root over the node conn (`_meta.theuth.parent*`, 67 §8).
#[derive(Clone, Debug)]
struct RemoteParent {
    /// The parent session id on the calling engine.
    session: String,
    /// The calling engine's id as this one knows it (its device id).
    engine: Option<String>,
}

impl LiveActor {
    /// Offer a peer exchange to the runner; `Err` is the runner's refusal.
    async fn ask_peer(&self, input: PeerInput) -> Result<(), String> {
        let r = match self {
            Self::Native(a) => a.ask(input).await,
            Self::Acp(a) => a.ask(input).await,
        };
        match r {
            Ok(()) => Ok(()),
            Err(SendError::HandlerError(e)) => Err(e),
            Err(e) => Err(format!("runner unavailable: {e}")),
        }
    }
}

impl Message<AgentEvent> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) {
        match ev {
            AgentEvent::TurnStarted { session, .. } => {
                self.running.insert(session);
                // A follow-up turn (steer, peer message) has no permit or
                // timer but still needs a start time for the picker.
                if self
                    .sessions
                    .get(&session)
                    .is_some_and(|l| l.parent.is_some())
                {
                    self.child_turns
                        .entry(session)
                        .or_insert_with(|| ChildTurn {
                            _permit: None,
                            timer: None,
                            started_at: Utc::now(),
                        });
                }
            }
            AgentEvent::TurnFinished {
                session,
                turn,
                stop_reason,
                ..
            } => {
                self.running.remove(&session);
                if let Some(timer) = self.child_turns.remove(&session).and_then(|ct| ct.timer) {
                    timer.abort();
                }
                self.last_stop
                    .insert(session, (stop_reason.clone(), Utc::now()));
                self.search.invalidate();
                self.child_finished(session, turn, &stop_reason).await;
                self.idle_check(session).await;
            }
            // Nested `python` calls (08 as-built): the only durable trace of
            // an inner call, since none enters the model's messages. The
            // repl publishes with a bus `tell`, so this write never sits on
            // the interpreter's path. A closed private id has no row (51 §4.4).
            AgentEvent::ToolCallStarted {
                session,
                call_id,
                name,
                args,
                ..
            } if store::inner_call_id(&call_id).is_some() => {
                if !self.closing.contains(&session)
                    && let Err(e) = self
                        .store_for(session)
                        .inner_call_started(session, &call_id, &name, &args)
                        .await
                {
                    tracing::warn!(session = %session, "inner_call_started: {e:#}");
                }
            }
            AgentEvent::ToolCallFinished {
                session,
                call_id,
                output,
                is_error,
                files,
                ..
            } if store::inner_call_id(&call_id).is_some() => {
                if !self.closing.contains(&session)
                    && let Err(e) = self
                        .store_for(session)
                        .inner_call_finished(session, &call_id, &output, is_error, &files)
                        .await
                {
                    tracing::warn!(session = %session, "inner_call_finished: {e:#}");
                }
            }
            // `SessionDeleted` is only ever published by `DeleteSession`,
            // which bumps before replying; `TitleChanged` also comes from the
            // loop's title generation.
            AgentEvent::TitleChanged { .. }
            | AgentEvent::Compacted { .. }
            | AgentEvent::ContextChanged { .. } => self.search.invalidate(),
            _ => {}
        }
    }
}

/// Approvals (design doc 19 §2.1): an unanswered request shows the session
/// as `AgentState::Approval`.
impl Message<ApprovalRequested> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalRequested, _: &mut Context<Self, Self::Reply>) {
        let session = ev.session;
        self.approvals.insert(session, (ev.id, ev.tool));
        self.network_approval(session, true).await;
    }
}

impl Message<ApprovalResolved> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalResolved, _: &mut Context<Self, Self::Reply>) {
        let sessions: Vec<SessionId> = self
            .approvals
            .iter()
            .filter(|(_, (id, _))| *id == ev.id)
            .map(|(s, _)| *s)
            .collect();
        self.approvals.retain(|_, (id, _)| *id != ev.id);
        for s in sessions {
            self.network_approval(s, false).await;
        }
    }
}

/// `tell` (internal): the tool registry acknowledged [`RegisterActivity`];
/// from here every invocation/job/command is admitted before it runs, so a
/// zero-entry ledger is evidence (47 §4.2 rule 7). Never certified before.
struct ActivityRegistered;

impl Message<ActivityRegistered> for Agents {
    type Reply = ();

    async fn handle(&mut self, _: ActivityRegistered, _: &mut Context<Self, Self::Reply>) {
        self.ledger.certify();
        self.rearm_reminders().await;
    }
}

impl Agents {
    /// Restore stored reminders after a restart (14 as-built 2026-09-23):
    /// rows of sessions that no longer exist go; the rest are re-armed by
    /// the jobs actor, which fires overdue ones at once as `late`. A `tell`:
    /// the actor admits them off its mailbox by asking back into us.
    async fn rearm_reminders(&mut self) {
        let rows = match self.store.reminders().await {
            Ok(rows) => rows,
            Err(e) => return tracing::error!("stored reminders unreadable: {e:#}"),
        };
        if rows.is_empty() {
            return;
        }
        let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) else {
            return tracing::error!("jobs actor not running: stored reminders not re-armed");
        };
        for r in rows {
            if !matches!(self.store.session(r.session).await, Ok(Some(_))) {
                let _ = self.store.delete_reminder(r.session, &r.id).await;
                continue;
            }
            let _ = jobs
                .tell(RearmReminder {
                    session: r.session,
                    id: r.id,
                    message: r.message,
                    set_at: r.set_at,
                    fires_at: r.fires_at,
                    seconds: r.seconds,
                })
                .await;
        }
    }
}

/// Store mirror of the jobs actor's reminder state (14 as-built): rows are
/// inserted when set and deleted when cancelled; a fired one is deleted by
/// its `Settle::Reminder`.
impl Message<ReminderEvent> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ReminderEvent, _: &mut Context<Self, Self::Reply>) {
        let r = match m {
            ReminderEvent::Set {
                session,
                id,
                message,
                set_at,
                fires_at,
                seconds,
            } => {
                self.store_for(session)
                    .insert_reminder(&store::ReminderRow {
                        id,
                        session,
                        message,
                        set_at,
                        fires_at,
                        seconds,
                    })
                    .await
            }
            ReminderEvent::Cancelled { session, id } => {
                self.store_for(session).delete_reminder(session, &id).await
            }
        };
        if let Err(e) = r {
            tracing::warn!("reminder row not written: {e:#}");
        }
    }
}

impl Agents {
    /// Make `session`'s ancestry known to the ledger from its stored row when
    /// no actor has registered it (an RPC job start on a session with no
    /// actor, work adopted at registration). The store never asks back into
    /// this actor. A row that cannot be read leaves the owner unresolved,
    /// which blocks every decision until the work settles: fail closed, not
    /// a wildcard by design.
    async fn place(&mut self, session: SessionId) {
        if self.ledger.knows(session) {
            return;
        }
        match self.store_for(session).session(session).await {
            Ok(Some(row)) => self.ledger.register(session, row.parent, true),
            Ok(None) => tracing::warn!(session = %session, "activity for a session with no row"),
            Err(e) => tracing::error!(session = %session, "activity owner lookup: {e:#}"),
        }
    }
}

/// Activity admission (47 §4.1): the entry is recorded, then the reply.
/// Awaits only the store for an owner no actor registered (never a tools
/// actor, so no deadlock with the registry's barrier).
impl Message<Admit> for Agents {
    type Reply = Admission;

    async fn handle(&mut self, Admit(op): Admit, _: &mut Context<Self, Self::Reply>) -> Admission {
        self.place(op.owner).await;
        self.ledger.admit_op(&op);
        // Bind the work to the owner's active request scope before it runs (50 §6.3).
        self.network_operation_admitted(&op).await;
        Admission::Admitted
    }
}

impl Agents {
    /// Settlement (47 §4.1). A finished job's result (14 §3) becomes a
    /// `<job_result>` harness input here — the *only* model-delivery path; the
    /// public `JobEvent::Finished` stays for UIs. The owner's input ticket is
    /// admitted inside `inject` before the operation is retired, so execution
    /// turns into a pending delivery with no gap; an idle session starts a new
    /// turn, a running one queues it, and a session with no live actor is
    /// resumed by `route`. `Unknown` keeps the entry and surfaces the failure;
    /// only a later `Finished` for the same id (the owner reconciling) clears it.
    async fn settle(&mut self, m: Settle) {
        match m {
            Settle::Finished { op } => self.ledger.retire_op(op),
            Settle::FinishedWithResult { op, result } => {
                if let JobEvent::Finished {
                    session,
                    id,
                    command,
                    exit,
                    secs,
                    output,
                    inline: false,
                    pty,
                } = result
                {
                    // Same cap and dir as tool results in the loop (07 §6).
                    let text = spill::cap(
                        session,
                        &format!("job-{id}"),
                        &output,
                        self.cfg.max_tool_output_chars,
                        &self.rt.data_dir.join("spill"),
                    );
                    let wrapped = job_result(&id, &command, &exit, secs, &text, pty);
                    // A job an exported task's turn started reports to that
                    // task's scope, never to a private turn (50 §6.3).
                    let scope = self.network_operation_scope(op);
                    self.inject(session, &format!("job {id}"), text, wrapped, scope)
                        .await;
                }
                // An inline result was the `shell` tool's own reply (14 §3);
                // injecting it too would hand the model the same output twice.
                self.ledger.retire_op(op);
            }
            Settle::Reminder {
                op,
                id,
                message,
                set_at,
                seconds,
                late,
            } => {
                // The owner is the ledger's, as `notice`: a reminder for a
                // retired op (its session closed) is owed to nobody.
                if let Some(session) = self.ledger.op_owner(op) {
                    let wrapped = reminder(
                        &id,
                        seconds,
                        &tools::reminder::local_rfc3339(set_at),
                        late,
                        &message,
                    );
                    let scope = self.network_operation_scope(op);
                    self.inject(session, &format!("reminder {id}"), message, wrapped, scope)
                        .await;
                    if let Err(e) = self.store_for(session).delete_reminder(session, &id).await {
                        tracing::warn!(session = %session, "reminder {id} row not deleted: {e:#}");
                    }
                } else {
                    tracing::info!(%op, "reminder {id} for a retired op dropped");
                }
                self.ledger.retire_op(op);
            }
            Settle::Unknown { op, owner, failure } => {
                tracing::warn!(%op, %owner, "activity outcome unknown: {failure}");
                self.place(owner).await;
                self.ledger.unknown_op(op, owner, failure.clone());
                self.error(owner, format!("background work outcome unknown: {failure}"))
                    .await;
            }
        }
    }
}

impl Message<Settle> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Settle, _: &mut Context<Self, Self::Reply>) {
        let (op, owner, unknown) = match &m {
            Settle::Finished { op }
            | Settle::FinishedWithResult { op, .. }
            | Settle::Reminder { op, .. } => (*op, None, None),
            Settle::Unknown { op, owner, failure } => (*op, Some(*owner), Some(failure.clone())),
        };
        let owner = owner.or_else(|| self.ledger.op_owner(op));
        self.settle(m).await;
        self.network_operation_settled(op, unknown.as_deref()).await;
        if let Some(s) = owner {
            self.idle_check(s).await;
        }
    }
}

impl Agents {
    /// A quiet PTY job wakes its owner (61 §1.8): the same wrap-cap-inject
    /// path as a result, under the op's network scope, while the operation
    /// is still admitted. A late notice for a retired op is dropped: nothing
    /// is owed.
    async fn notice(&mut self, m: Notice) {
        let Notice::JobIdle {
            op,
            event:
                JobEvent::Idle {
                    session,
                    id,
                    command,
                    name,
                    quiet_secs,
                    secs,
                    nudge,
                    user_typed_secs,
                    output,
                },
        } = m
        else {
            return;
        };
        if self.ledger.op_owner(op).is_none() {
            tracing::info!(%op, "idle notice for a settled job dropped");
            return;
        }
        let text = spill::cap(
            session,
            &format!("job-{id}-idle"),
            &output,
            self.cfg.max_tool_output_chars,
            &self.rt.data_dir.join("spill"),
        );
        let wrapped = job_idle(
            &id,
            &command,
            name.as_deref(),
            quiet_secs,
            secs,
            nudge,
            user_typed_secs,
            &text,
        );
        let scope = self.network_operation_scope(op);
        self.inject(session, &format!("job {id}"), text, wrapped, scope)
            .await;
    }
}

impl Message<Notice> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Notice, _: &mut Context<Self, Self::Reply>) {
        self.notice(m).await;
    }
}

impl Agents {
    /// The network module settles exported tasks only from an idle session
    /// with no ledger activity, never from `TurnFinished` alone (50 §6.3).
    async fn idle_check(&mut self, session: SessionId) {
        if self.ledger.activity(session).is_none() {
            self.network_session_idle(session).await;
        }
    }
}

impl Message<TurnEntered> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: TurnEntered, _: &mut Context<Self, Self::Reply>) {
        self.ledger.entered(m);
    }
}

impl Message<InputConsumed> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: InputConsumed, _: &mut Context<Self, Self::Reply>) {
        self.ledger.consumed(m);
        // A scope's input opened / joined this turn: the turn is the scope's (50 §6.3).
        self.network_input_consumed(&m).await;
    }
}

impl Message<InputDropped> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: InputDropped, _: &mut Context<Self, Self::Reply>) {
        self.ledger.release(m.session, m.ticket);
        self.network_input_dropped(m.ticket).await;
        self.idle_check(m.session).await;
    }
}

impl Message<TurnReturned> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: TurnReturned, _: &mut Context<Self, Self::Reply>) {
        let session = m.session;
        self.ledger.returned(m);
        self.idle_check(session).await;
    }
}

impl Message<SessionUnknown> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: SessionUnknown, _: &mut Context<Self, Self::Reply>) {
        tracing::warn!(session = %m.session, "session activity unknown: {}", m.failure);
        let session = m.session;
        let message = format!(
            "work under this session cannot be proven stopped: {}",
            m.failure
        );
        self.ledger.session_unknown(m);
        self.error(session, message).await;
    }
}

/// The one decision (47 §4.3), entirely synchronous: supervisor-owned
/// evidence (approvals, peer waiters, event-projected running descendants)
/// plus the ledger, then the commit inside `Ledger::decide`. Never queries
/// an actor or the store.
impl Message<DecideYield> for Agents {
    type Reply = YieldDecision;

    async fn handle(
        &mut self,
        m: DecideYield,
        _: &mut Context<Self, Self::Reply>,
    ) -> YieldDecision {
        let blocked = self.ledger.subtree(m.session).into_iter().any(|d| {
            self.approvals.contains_key(&d)
                || self.waits.values().any(|w| w.session == d)
                || (d != m.session && self.running.contains(&d))
        });
        self.ledger.decide(&m, blocked)
    }
}

impl Message<GetTodos> for Agents {
    type Reply = Result<crate::todo::TodoSnapshot, String>;

    async fn handle(&mut self, m: GetTodos, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store_for(m.session)
            .todos(m.session)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// The `<job_result>` wrapper (14 §1.2): `command` cut to 120 chars and
/// attribute-escaped; `exit` is the code, `timeout` or `killed`. PTY jobs
/// (22 §2.2) carry ` pty="true"` and `text` is the final screen.
pub(crate) fn job_result(
    id: &JobId,
    command: &str,
    exit: &JobExit,
    secs: u64,
    text: &str,
    pty: bool,
) -> String {
    let cmd: String = command.chars().take(120).collect();
    let exit = match exit {
        JobExit::Exited(code) => code.to_string(),
        JobExit::Timeout => "timeout".into(),
        JobExit::Killed => "killed".into(),
    };
    let pty = if pty { " pty=\"true\"" } else { "" };
    format!(
        "<job_result id=\"{id}\" command=\"{}\" exit=\"{exit}\" secs=\"{secs}\"{pty}>\n{text}\n</job_result>",
        attr(&cmd)
    )
}

/// The `<job_idle>` wrapper (61 §2.2): `command` cut and escaped as
/// [`job_result`]; `name` and `user_typed` only when present; `text` is the
/// screen with its `cursor:` line.
#[allow(clippy::too_many_arguments)]
pub(crate) fn job_idle(
    id: &JobId,
    command: &str,
    name: Option<&str>,
    quiet: u64,
    secs: u64,
    nudge: u32,
    user_typed: Option<u64>,
    text: &str,
) -> String {
    let cmd: String = command.chars().take(120).collect();
    let name = name.map_or_else(String::new, |n| format!(" name=\"{}\"", attr(n)));
    let typed = user_typed.map_or_else(String::new, |t| format!(" user_typed=\"{t}\""));
    format!(
        "<job_idle id=\"{id}\" command=\"{}\"{name} quiet=\"{quiet}\" secs=\"{secs}\" nudge=\"{nudge}\"{typed}>\n{text}\n</job_idle>",
        attr(&cmd)
    )
}

/// The `<reminder>` wrapper (14 as-built 2026-09-23): `after` is the
/// requested delay in seconds, `set_at` RFC 3339 local, `late` true when it
/// was re-armed past due after a restart; `text` is the model's message.
pub(crate) fn reminder(id: &str, after: u64, set_at: &str, late: bool, text: &str) -> String {
    format!(
        "<reminder id=\"{}\" after=\"{after}\" set_at=\"{}\" late=\"{late}\">\n{text}\n</reminder>",
        attr(id),
        attr(set_at)
    )
}

/// XML attribute escape.
fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Message<SendInput> for Agents {
    type Reply = Result<SessionId, kameo::error::Infallible>;

    async fn handle(
        &mut self,
        m: SendInput,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SessionId, kameo::error::Infallible> {
        Ok(self
            .human_input(m.session, None, m.text, m.files, m.delivery)
            .await)
    }
}

impl Message<UseSkill> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: UseSkill, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let text = skills::load_text(&self.skills_dir(), &m.name)?;
        let text = skills::inject_text(&m.name, &text, &m.args);
        // Routed, never a waiter's answer: a skill body is not a reply. The
        // id makes the queue/inject lifecycle visible to UIs (they did not type it).
        // The user asked for it: a fresh request (47 §4.5).
        let sid = m.session.unwrap_or_default();
        // A routing failure is an `AgentEvent::Error` on `sid`, as before.
        let _ = self
            .input(
                sid,
                Some(InputId::new()),
                text,
                Vec::new(),
                InputOrigin::External,
                Delivery::Steer,
            )
            .await;
        Ok(sid)
    }
}

impl Message<Steer> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Steer, _: &mut Context<Self, Self::Reply>) {
        self.human_input(Some(m.session), Some(m.id), m.text, m.files, m.delivery)
            .await;
    }
}

impl Message<Withdraw> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Withdraw, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(actor) = self.live(m.session) else {
            return Err("not queued".into());
        };
        // `SendError::HandlerError` displays as the inner `String`.
        let r = actor
            .native()?
            .ask(WithdrawInput(m.id))
            .await
            .map_err(|e| e.to_string());
        if r.is_ok() {
            // The queued input left the loop with its ticket: nothing is owed.
            self.ledger.withdrawn(m.session, m.id);
        }
        r
    }
}

impl Message<ListQueued> for Agents {
    type Reply = Vec<QueuedInput>;

    async fn handle(&mut self, m: ListQueued, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match self.live(m.session).and_then(|a| a.native().ok()) {
            Some(actor) => actor.ask(QueuedInputs).await.unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl Message<Undo> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Undo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.search.invalidate();
        self.ledger.bump(m.session);
        self.actor(m.session)
            .await?
            .native()?
            .ask(UndoTurn)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<Redo> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Redo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.search.invalidate();
        self.ledger.bump(m.session);
        self.actor(m.session)
            .await?
            .native()?
            .ask(RedoTurn)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<Regenerate> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Regenerate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.search.invalidate();
        let actor = self.actor(m.session).await?.native()?.clone();
        // A rewind invalidates old candidates; the replayed opener is the
        // human's request again (fresh budget). The loop drops the ticket
        // with a refused restart, so release it on `Err`.
        self.ledger.bump(m.session);
        let ticket = self.ledger.admit(m.session, InputOrigin::External, None);
        let r = actor
            .ask(RestartTurn {
                turn: m.turn,
                input: None,
                ticket: Some(ticket),
            })
            .await
            .map_err(|e| e.to_string());
        if r.is_err() {
            self.ledger.release(m.session, ticket);
        }
        r
    }
}

impl Message<EditTurn> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: EditTurn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        check_files(&m.files)?;
        self.search.invalidate();
        let actor = self.actor(m.session).await?.native()?.clone();
        // As `Regenerate`: rewind, then the edited text is a fresh human request.
        self.ledger.bump(m.session);
        let ticket = self.ledger.admit(m.session, InputOrigin::External, None);
        let r = actor
            .ask(RestartTurn {
                turn: m.turn,
                input: Some((m.text, m.files)),
                ticket: Some(ticket),
            })
            .await
            .map_err(|e| e.to_string());
        if r.is_err() {
            self.ledger.release(m.session, ticket);
        }
        r
    }
}

impl Message<ForkSession> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: ForkSession, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(&root) = self.private_member.get(&m.session) {
            return self.fork_private(root, m, ctx.actor_ref().clone()).await;
        }
        let row = self
            .store
            .session(m.session)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no such session".to_string())?;
        if row.agent.is_some() {
            // The transcript is a translation; the harness owns the state (design doc 29 §3).
            return Err("cannot fork an external harness session".into());
        }
        let id = self
            .store
            .fork(m.session, m.through, m.reply)
            .await
            .map_err(|e| e.to_string())?;
        // A fork is a new root (50 §2.1): the source root's scope, no alias,
        // opted out when the source is effectively off; nothing peer-related
        // is copied (exchanges, tasks, waits).
        self.mirror(m.session).await;
        let scope = {
            let mut p = self.communication.0.write();
            match p.record_fork(m.session, id) {
                Ok(()) => p
                    .root_of(id)
                    .ok()
                    .and_then(|r| p.state(Some(r)).ok())
                    .map(|s| s.scope),
                Err(e) => {
                    tracing::error!(session = %id, "record_fork: {e}");
                    None
                }
            }
        };
        if let Some(scope) = scope.filter(|s| *s != PeerScope::Isolated)
            && let Err(e) = self.store.set_root_scope(id, scope, None).await
        {
            tracing::error!(session = %id, "set_root_scope: {e:#}");
        }
        self.search.invalidate();
        Ok(id)
    }
}

impl Message<Cancel> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: Cancel, _: &mut Context<Self, Self::Reply>) {
        // Every running descendant, not one level (design doc 19 §2.3).
        let tree = self
            .store_for(m.session)
            .descendants(m.session)
            .await
            .unwrap_or_default();
        let ids: Vec<SessionId> = std::iter::once(m.session)
            .chain(tree.into_iter().map(|(_, s)| s.id))
            .collect();
        for &id in &ids {
            // Stop control (47 §4.3): candidates admitted before this are stale.
            self.ledger.bump(id);
            if let Some(live) = self
                .sessions
                .get(&id)
                .filter(|_| id == m.session || self.running.contains(&id))
            {
                live.actor.tell_cancel().await;
            }
        }
        // The cancelled turns' awaiting tool calls are gone: their waits
        // expire explicitly (50 §5.3), never left for a late reply.
        self.expire_waits("cancelled", |w| ids.contains(&w.session));
        self.bound.retain(|(s, _), _| !ids.contains(s));
    }
}

impl Message<Compact> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<u32, String>>;

    async fn handle(&mut self, m: Compact, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let actor = match self
            .actor(m.session)
            .await
            .and_then(|a| a.native().cloned())
        {
            Ok(actor) => actor,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // Manual compaction rewrites the context: old candidates are stale.
        // Its busy window is reported by the loop (`TurnEntered { turn: None }`).
        self.ledger.bump(m.session);
        // Off the mailbox: `compact_now` asks us for the `Roster`, so awaiting
        // here would deadlock. `SendError::HandlerError` displays as the inner `String`.
        tokio::spawn(async move {
            tx.send(
                actor
                    .ask(CompactNow {
                        style: m.style,
                        head: m.head,
                    })
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

impl Message<SetCompactStyle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetCompactStyle,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if m.style == CompactStyle::Clear {
            return Err("clear is `/clear`, not a compaction default".into());
        }
        *self
            .compact_style
            .write()
            .unwrap_or_else(|p| p.into_inner()) = m.style;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "compact_style", &m.style.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<SetCompaction> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetCompaction,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.actor(m.session)
            .await?
            .native()?
            .tell(SetCompactionOverride(CompactionOverride {
                at_tokens: m.at_tokens,
                hint: m.hint,
            }))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<SetToolset> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetToolset, _: &mut Context<Self, Self::Reply>) -> Result<(), String> {
        self.actor(m.session)
            .await?
            .native()?
            .tell(SetToolsetOverride(m.toolset))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<GetCompactStyle> for Agents {
    type Reply = CompactStyle;

    async fn handle(
        &mut self,
        _: GetCompactStyle,
        _: &mut Context<Self, Self::Reply>,
    ) -> CompactStyle {
        *self.compact_style.read().unwrap_or_else(|p| p.into_inner())
    }
}

impl Message<SetSubagentTimeout> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetSubagentTimeout,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        Arc::make_mut(&mut self.cfg).subagents.max_runtime_secs = m.seconds;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_nested_config_value(
            &path,
            "agent",
            "subagents",
            "max_runtime_secs",
            &m.seconds.to_string(),
        ) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<GetSubagentTimeout> for Agents {
    type Reply = u64;

    async fn handle(&mut self, _: GetSubagentTimeout, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.cfg.subagents.max_runtime_secs
    }
}

impl Message<SetMaxToolRounds> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetMaxToolRounds,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if m.rounds == 0 {
            return Err("max_tool_rounds must be at least 1".into());
        }
        Arc::make_mut(&mut self.cfg).max_tool_rounds = m.rounds;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "max_tool_rounds", &m.rounds.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<GetMaxToolRounds> for Agents {
    type Reply = u32;

    async fn handle(&mut self, _: GetMaxToolRounds, _: &mut Context<Self, Self::Reply>) -> u32 {
        self.cfg.max_tool_rounds
    }
}

impl Message<SetStreamIdle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetStreamIdle,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        if m.seconds == 0 {
            return Err("stream_idle_secs must be at least 1".into());
        }
        Arc::make_mut(&mut self.cfg).stream_idle_secs = m.seconds;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "stream_idle_secs", &m.seconds.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<GetStreamIdle> for Agents {
    type Reply = u64;

    async fn handle(&mut self, _: GetStreamIdle, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.cfg.stream_idle_secs
    }
}

impl Message<SetMaxSalvages> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetMaxSalvages,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        Arc::make_mut(&mut self.cfg).max_salvages = m.count;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "max_salvages", &m.count.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<GetMaxSalvages> for Agents {
    type Reply = u32;

    async fn handle(&mut self, _: GetMaxSalvages, _: &mut Context<Self, Self::Reply>) -> u32 {
        self.cfg.max_salvages
    }
}

impl Message<SetTitleModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetTitleModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let resolved = match &m.model {
            Some(id) => Some(self.catalog.resolve(id, &*self.configured)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        *self.title_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        if let Err(e) = runtime::write_config_value(&path, "agent", "title_model", value) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(info)
    }
}

impl Message<GetTitleModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: GetTitleModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.title_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Agents {
    /// 57 §0.9: validate → write `config.yaml` → swap the live value, so a
    /// failed write leaves the selection as it was.
    fn set_escalate_model(&mut self, model: Option<&str>) -> Result<Option<ModelInfo>, String> {
        let resolved = match model {
            Some(id) => Some(escalate_candidate(&self.catalog, &*self.configured, id)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        runtime::write_config_value(&path, "agent", "escalate_model", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        *self
            .escalate_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = resolved;
        Ok(info)
    }

    /// The escalation slot and what it resolves to today (57 §0.9: unset →
    /// title model → each session's own model, which is `None` here).
    fn escalate_model(&self) -> EscalateModel {
        let slot = |s: &RwLock<Option<Resolved>>| {
            s.read()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(ModelInfo::from)
        };
        let model = slot(&self.escalate_model);
        let effective = model.clone().or_else(|| slot(&self.title_model));
        EscalateModel { model, effective }
    }
}

impl Message<SetEscalateModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetEscalateModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        self.set_escalate_model(m.model.as_deref())
    }
}

impl Message<GetEscalateModel> for Agents {
    type Reply = EscalateModel;

    async fn handle(
        &mut self,
        _: GetEscalateModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> EscalateModel {
        self.escalate_model()
    }
}

/// 57 §2 `SideCall` from another plugin: forwarded to the session's actor,
/// which resolves the role and runs the call detached; the supervisor never
/// waits on the provider. An external (ACP) session has no model of ours.
impl Message<SideCall> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<SideReply, String>>;

    async fn handle(&mut self, m: SideCall, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let actor = match self.sessions.get(&m.session).map(|l| &l.actor) {
            Some(LiveActor::Native(a)) => Ok(a.clone()),
            Some(LiveActor::Acp(_)) => Err("session runs on an external harness".to_owned()),
            None => Err(format!("no live session {}", m.session)),
        };
        tokio::spawn(async move {
            let out = match actor {
                Ok(a) => a.ask(m).await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if let Some(tx) = tx {
                tx.send(out);
            }
        });
        reply
    }
}

impl Message<SetSubagentModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetSubagentModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let resolved = match &m.model {
            Some(id) => {
                let r = self.catalog.resolve(id, &*self.configured)?;
                if let Some(p) = &r.provider
                    && !(self.configured)(Catalog::adapter(p))
                {
                    return Err(format!("provider `{p}` has no credentials"));
                }
                Some(r)
            }
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        *self.subagent_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        if let Err(e) =
            runtime::write_nested_config_value(&path, "agent", "subagents", "model", value)
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(info)
    }
}

impl Message<GetSubagentModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: GetSubagentModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.subagent_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Agents {
    /// The preferred subagent model's id for the system prompt, if set.
    fn subagent_model_id(&self) -> Option<String> {
        self.subagent_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|r| r.id.clone())
    }
}

impl Agents {
    /// [`SetVisionModel`] (52 §3.1): validate, persist, then swap — a rejected
    /// id or a failed config write leaves the live selection untouched.
    fn set_vision_model(&mut self, model: Option<&str>) -> Result<Option<ModelInfo>, String> {
        let resolved = match model {
            Some(id) => {
                let r = vision_candidate(&self.catalog, &*self.configured, id)?;
                let provider = r.provider.as_deref().unwrap_or_default();
                if !(self.configured)(Catalog::adapter(provider)) {
                    return Err(format!(
                        "provider `{provider}` is not configured; add a key, log in, or add a gateway first"
                    ));
                }
                Some(r)
            }
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        runtime::write_config_value(&path, "agent", "vision_model", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        *self.vision_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        self.vision_error = None;
        Ok(info)
    }
}

impl Message<SetVisionModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetVisionModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        self.set_vision_model(m.model.as_deref())
    }
}

impl Message<GetVisionModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: GetVisionModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.vision_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Message<Harvest> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<knowledge::HarvestResult, String>>;

    async fn handle(&mut self, m: Harvest, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let actor = match self
            .actor(m.session)
            .await
            .and_then(|a| a.native().cloned())
        {
            Ok(actor) => actor,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // Off the mailbox for the same reason as `Compact`: the harvest is a
        // model call and may ask us back.
        tokio::spawn(async move {
            tx.send(actor.ask(HarvestNow).await.map_err(|e| e.to_string()));
        });
        reply
    }
}

impl Message<SpawnChild> for Agents {
    type Reply = Result<(String, SessionId), String>;

    async fn handle(
        &mut self,
        m: SpawnChild,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(String, SessionId), String> {
        let sub = &self.cfg.subagents;
        // 51 §4.1: nothing of a private tree leaves the process.
        if m.agent.is_some() && self.private_member.contains_key(&m.parent) {
            return Err("remote agents are unavailable in a private session".into());
        }
        let depth = self
            .store_for(m.parent)
            .depth(m.parent)
            .await
            .map_err(|e| e.to_string())?;
        if depth >= sub.max_depth {
            return Err(format!("subagents are not available at depth {depth}"));
        }
        // 67 §8: `agent` is `[<harness>@]<engine>`; a mesh engine may be
        // named by its id prefix and is stored under its node name. A node
        // resolves `model` against its own catalog and enforces `tools`
        // itself; an external harness (here or on a node) takes `model`
        // through its model option, picks its own effort, and reads the
        // ceiling only as the first line of its brief.
        let m = SpawnChild {
            agent: m.agent.and_then(|a| crate::peers::canonical_agent(&self.nodes, &a)),
            ..m
        };
        let (harness, engine) = match m.agent.as_deref() {
            Some(a) => crate::peers::parse_agent(a)?,
            None => (None, ""),
        };
        let node = m.agent.is_some()
            && self.nodes.read().is_ok_and(|n| n.contains_key(engine));
        let external = harness.is_some() || (m.agent.is_some() && !node);
        if external && m.effort.is_some() {
            return Err("`effort` does not apply to an external harness; it picks its own".into());
        }
        if let Some(model) = &m.model
            && m.agent.is_none()
        {
            let r = self.catalog.resolve(model, &*self.configured)?;
            if let Some(p) = &r.provider
                && !(self.configured)(Catalog::adapter(p))
            {
                return Err(format!(
                    "provider `{p}` has no credentials; list_models shows what is available"
                ));
            }
        }
        if let Some(e) = &m.effort
            && e != "default"
            && effort_of(e).is_none()
        {
            return Err(format!(
                "unknown effort `{e}`; use low|medium|high|off|default or a token budget"
            ));
        }
        let id = SessionId::new();
        let base = m.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let b = id.0.as_bytes();
            format!(
                "{}-{}",
                ADJECTIVES[b[14] as usize % ADJECTIVES.len()],
                NOUNS[b[15] as usize % NOUNS.len()]
            )
        });
        let mut name = base.clone();
        let mut n = 1;
        while self
            .store_for(m.parent)
            .child_by_name(m.parent, &name)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            n += 1;
            name = format!("{base}-{n}");
        }
        self.session(
            Some(id),
            Some((m.parent, &name)),
            m.agent.map(|a| (a, m.cwd, m.cap)),
            (m.model.as_deref(), m.effort.as_deref()),
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
        // Next to `inherit_approvals` in spirit: the ceiling is on the
        // registry before the child's first `ListTools`.
        if let Some(cap) = m.cap {
            self.install_cap(id, cap).await;
        }
        self.rt
            .publish(AgentEvent::SubagentSpawned {
                parent: m.parent,
                child: id,
                name: name.clone(),
                cap: m.cap,
            })
            .await;
        // Spawned by a turn an A2A request scope owns (50 §6.3): the child
        // is that scope's work — explicitly, from the spawning turn, never
        // from whatever task happens to be active in the parent — until
        // `child_finished` settles the `child:<id>` row.
        if let Some(scope) = self.network_turn_scope(m.parent) {
            self.network_bind_work(scope, &format!("child:{id}")).await;
            self.owned_children.insert(
                id,
                OwnedChild {
                    scope,
                    cancelled: false,
                },
            );
        }
        // The child's initial assignment is admitted now, before the permit
        // wait: a child queued behind the cap already counts (47 §3.2), and
        // it opens the child's own reminder budget (`External`).
        let ticket = self.ledger.admit(id, InputOrigin::External, None);
        let me = ctx.actor_ref().clone();
        let permits = self.permits.clone();
        // Not enforceable on an external harness: the ceiling is the first
        // line of its brief instead (67 §8).
        let text = match m.cap.filter(|_| external) {
            Some(cap) => format!("Limits: tools {cap}\n{}", m.task),
            None => m.task,
        };
        tokio::spawn(async move {
            let Ok(permit) = permits.acquire_owned().await else {
                return;
            };
            let _ = me
                .tell(StartChildTurn {
                    child: id,
                    permit,
                    text,
                    ticket,
                })
                .await;
        });
        Ok((name, id))
    }
}

impl Message<StartChildTurn> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: StartChildTurn, ctx: &mut Context<Self, Self::Reply>) {
        let child = m.child;
        let Ok(scope) = self.claim_child_turn(child, m.ticket).await else {
            return;
        };
        let _ = self
            .route(child, None, m.text, Vec::new(), m.ticket, scope, false)
            .await;
        let me = ctx.actor_ref().clone();
        let after = Duration::from_secs(self.cfg.subagents.max_runtime_secs);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = me.tell(ChildTimedOut(child)).await;
        })
        .abort_handle();
        self.child_turns.insert(
            child,
            ChildTurn {
                _permit: Some(m.permit),
                timer: Some(timer),
                started_at: Utc::now(),
            },
        );
    }
}

impl Message<ChildTimedOut> for Agents {
    type Reply = ();

    async fn handle(
        &mut self,
        ChildTimedOut(child): ChildTimedOut,
        _: &mut Context<Self, Self::Reply>,
    ) {
        if self
            .child_turns
            .get(&child)
            .is_some_and(|ct| ct.timer.is_some())
            && let Some(live) = self.sessions.get(&child)
        {
            self.timed_out.insert(child);
            self.ledger.bump(child);
            live.actor.tell_cancel().await;
        }
    }
}

/// A store refusal as the sender reads it; unknown and unauthorized are one answer.
fn comm_err(e: anyhow::Error) -> String {
    match e.downcast_ref::<CommError>() {
        Some(CommError::NotFound) => "unknown message".into(),
        Some(CommError::Conflict) => {
            "request_id already used for a different message; pick a new request_id".into()
        }
        Some(CommError::Terminal) => "that message is already settled".into(),
        Some(CommError::Busy) => "busy: the recipient's intake is full; retry later".into(),
        Some(CommError::TooLarge) => format!("message exceeds {PEER_TEXT_MAX} bytes"),
        Some(CommError::Revoked) => format!("{}: route revoked", tools::communication::DISABLED),
        Some(other) => other.to_string(),
        None => format!("{e:#}"),
    }
}

impl Agents {
    /// `send_message{to: "parent"}` from a root a remote theuth opened (67
    /// §8): out over the node conn its parent holds, as a [`PeerUplink`] the
    /// ACP relay carries. The exchange is recorded on the parent (its
    /// `Deliver` from the child's row); here only the sender's own switch is
    /// checked, and a reply cannot be awaited — the parent's answer arrives
    /// as an ordinary prompt.
    async fn uplink(&mut self, m: &Deliver, body: String) -> Result<Delivered, String> {
        self.communication
            .0
            .read()
            .enabled(m.from)
            .map_err(|e| format!("send_message: {}: {e}", tools::communication::DISABLED))?;
        if m.await_reply {
            return Err(
                "await_reply is not available for a remote parent; its answer arrives as a <message>"
                    .into(),
            );
        }
        let id = Uuid::now_v7();
        self.rt
            .publish(PeerUplink {
                session: m.from,
                id,
                reply_to: m.reply_to,
                text: body,
            })
            .await;
        Ok(Delivered {
            receipt: PeerReceipt {
                id,
                to: "parent".to_owned(),
                state: PeerMessageState::Accepted,
                reply_to: m.reply_to,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            },
            reply: None,
        })
    }
}

impl Message<Deliver> for Agents {
    type Reply = Result<Delivered, String>;

    async fn handle(&mut self, m: Deliver, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.deliver(m, None).await
    }
}

impl Message<PeerDeliver> for Agents {
    type Reply = Result<PeerReceipt, String>;

    async fn handle(&mut self, m: PeerDeliver, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.peer_deliver(m).await
    }
}

impl Agents {
    /// The owner's intake of a forwarded peer message (69 P): the ordinary
    /// [`Self::deliver`] path from the envelope's sender to `session`, under
    /// the envelope's id and this engine's policy. Refused when the row is
    /// not ours (the sender's replica was stale) — never a take.
    pub(crate) async fn peer_deliver(&mut self, m: PeerDeliver) -> Result<PeerReceipt, String> {
        let row = self
            .store
            .session(m.session)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("unknown peer `session:{}`", m.session))?;
        if !row.here {
            return Err(format!("session:{} is owned by {}, not here", m.session, row.owner));
        }
        // The sender is a replicated row: its lineage must be known to the
        // policy before `route` (as `resolve` does for the recipient).
        self.mirror(m.envelope.from).await;
        let PeerEnvelope {
            id,
            from,
            text,
            reply_to,
            request_id,
        } = m.envelope;
        let d = self
            .deliver(
                Deliver {
                    from,
                    to: format!("session:{}", m.session),
                    text,
                    await_reply: false,
                    reply_to,
                    request_id,
                    admitted: CancellationToken::new(),
                },
                Some(id),
            )
            .await?;
        Ok(d.receipt)
    }

    /// `(owner engine id)` when `session`'s replicated row is owned by
    /// another engine (69 P); `None` for ours, unowned or unknown — today's
    /// local path.
    async fn remote_owner(&self, session: SessionId) -> Option<String> {
        let row = self.store.session(session).await.ok().flatten()?;
        (!row.here && !row.owner.is_empty()).then_some(row.owner)
    }

    /// The mesh transport, fetched once from the replica actor (tests inject it).
    async fn transport(&mut self) -> Option<Arc<dyn replica::Transport>> {
        if self.transport.is_none() {
            self.transport = match replica::actor() {
                Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
                None => None,
            };
        }
        self.transport.clone()
    }

    /// Hand an accepted exchange to the owner of `to` (69 P): one
    /// `peer_deliver` over `theuth/rpc/1`. The receipt stays `accepted`
    /// when the owner's intake committed (a waiter is registered as for a
    /// local recipient); otherwise it is `failed` with [`peer_mesh::Failure`]'s
    /// text — the sender reads a normal receipt either way.
    async fn forward_peer(
        &mut self,
        owner: String,
        to: SessionId,
        m: &Deliver,
        body: String,
        receipt: PeerReceipt,
        generation: Generation,
    ) -> Result<Delivered, String> {
        // Nothing here will consume it.
        self.enqueued.remove(&receipt.id);
        let envelope = PeerEnvelope {
            id: receipt.id,
            from: m.from,
            text: body,
            reply_to: m.reply_to,
            request_id: m.request_id.clone(),
        };
        let peer = replica::PeerId(owner.clone());
        let outcome = match self.transport().await {
            Some(t) => peer_mesh::forward(t.as_ref(), &peer, to, &envelope).await,
            None => Err(peer_mesh::Failure::Unreachable {
                owner,
                why: "mesh not running".into(),
            }),
        };
        match outcome {
            Ok(_) => {
                let reply = m
                    .await_reply
                    .then(|| self.wait(m.from, receipt.to.clone(), receipt.id, generation))
                    .transpose()?;
                Ok(Delivered { receipt, reply })
            }
            Err(f) => {
                tracing::info!(session = %to, id = %receipt.id, "peer message not forwarded: {f:?}");
                let error = f.receipt_error().await;
                let failed = self
                    .store
                    .fail_exchange(receipt.id, &error)
                    .await
                    .unwrap_or(receipt);
                self.peer_changed(&failed).await;
                Ok(Delivered {
                    receipt: failed,
                    reply: None,
                })
            }
        }
    }

    /// [`Deliver`]'s body. `id`: the message id when another engine minted
    /// it ([`Self::peer_deliver`]); minted by the store otherwise.
    pub(crate) async fn deliver(&mut self, m: Deliver, id: Option<Uuid>) -> Result<Delivered, String> {
        let revoked = || {
            format!(
                "send_message: {}: {}",
                tools::communication::DISABLED,
                tools::communication::Disabled::Revoked
            )
        };
        if m.admitted.is_cancelled() {
            return Err(revoked());
        }
        let body = self.redact(&m.text).into_owned();
        if body.len() > PEER_TEXT_MAX {
            return Err(format!("message exceeds {PEER_TEXT_MAX} bytes"));
        }
        let to = match self.resolve(m.from, &m.to).await? {
            Target::Uplink => return self.uplink(&m, body).await,
            Target::Remote(alias) => {
                let generation = self
                    .communication
                    .0
                    .write()
                    .remote(m.from)
                    .map_err(|e| e.to_string())?;
                if m.admitted.is_cancelled() {
                    return Err(revoked());
                }
                let receipt = self
                    .network_send(
                        m.from,
                        &alias,
                        &body,
                        m.request_id.as_deref(),
                        m.reply_to,
                        m.await_reply,
                    )
                    .await?;
                let reply = match (m.await_reply, receipt.state) {
                    (true, PeerMessageState::Accepted) => {
                        Some(self.wait(m.from, receipt.to.clone(), receipt.id, generation)?)
                    }
                    _ => None,
                };
                return Ok(Delivered { receipt, reply });
            }
            Target::Local(to) => to,
        };
        // The guard drops at the end of this statement; `route_error` reads.
        let routed = self.communication.0.write().route(m.from, to);
        let generation = routed.map_err(|e| self.route_error(m.from, to, e))?;
        // Admission after the route (50 §8 item 2): the tool's own token is
        // what an off→on between approval and dispatch cannot revive.
        if m.admitted.is_cancelled() {
            return Err(revoked());
        }
        if m.await_reply
            && (self.waits.len() >= WAITS_PER_HARNESS
                || self.waits.values().filter(|w| w.session == m.from).count() >= WAITS_PER_SESSION)
        {
            return Err("busy: too many replies awaited; let one resolve first".into());
        }
        // A reply inherits the request scope of what it answers (50 §6.3).
        let scope = match m.reply_to {
            Some(r) => {
                self.store
                    .peer_record(m.from, r)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "unknown message".to_string())?
                    .request_scope
            }
            None => None,
        };
        let (from_key, to_key) = (format!("session:{}", m.from), format!("session:{to}"));
        let accept = AcceptExchange {
            sender: &from_key,
            recipient: &to_key,
            sender_session: Some(m.from),
            recipient_session: Some(to),
            request_id: m.request_id.as_deref(),
            reply_to: m.reply_to,
            request_scope: scope,
            generation: generation.revision() as i64,
            body: &body,
        };
        let receipt = match id {
            Some(id) => self.store.accept_exchange_as(id, &accept).await,
            None => self.store.accept_exchange(&accept).await,
        }
        .map_err(comm_err)?;
        if receipt.state != PeerMessageState::Accepted || !self.enqueued.insert(receipt.id) {
            // A retried request_id: the existing outcome, nothing sent again.
            return Ok(Delivered {
                receipt,
                reply: None,
            });
        }
        self.peer_changed(&receipt).await;
        // An answer to a request its recipient awaits completes that tool
        // call (50 §5.3); the store verified the reversed endpoints.
        if let Some(request) = m.reply_to
            && self
                .waits
                .get(&request)
                .is_some_and(|w| w.session == to && w.to == from_key)
        {
            // A bound turn's explicit reply is the result of that request.
            if let Some(t) = self.ledger.turn_of(m.from)
                && self.bound.get(&(m.from, t)) == Some(&request)
            {
                self.delivered.insert((m.from, t), to);
            }
            let original = self
                .store
                .exchange_receipt(request)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "unknown message".to_string())?;
            if self.settle_wait(&original) {
                let settled = self
                    .store
                    .consume_exchange_direct(receipt.id, to, generation.revision() as i64)
                    .await
                    .map_err(comm_err)?
                    .unwrap_or_else(|| receipt.clone());
                self.enqueued.remove(&receipt.id);
                self.peer_changed(&settled).await;
                self.peer_changed(&original).await;
                let reply = m
                    .await_reply
                    .then(|| self.wait(m.from, to_key, settled.id, generation))
                    .transpose()?;
                return Ok(Delivered {
                    receipt: settled,
                    reply,
                });
            }
        }
        // 69 P: the recipient lives on another engine — its owner runs the
        // intake; nothing here is woken.
        if let Some(owner) = self.remote_owner(to).await {
            return self.forward_peer(owner, to, &m, body, receipt, generation).await;
        }
        let record = self
            .store
            .peer_record(to, receipt.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "unknown message".to_string())?;
        if let Err(reason) = self
            .enqueue_peer(record, generation.clone(), false, m.await_reply)
            .await
        {
            self.enqueued.remove(&receipt.id);
            let failed = self
                .store
                .fail_exchange(receipt.id, &reason)
                .await
                .unwrap_or(receipt);
            self.peer_changed(&failed).await;
            return Err(reason);
        }
        let reply = m
            .await_reply
            .then(|| self.wait(m.from, to_key, receipt.id, generation))
            .transpose()?;
        Ok(Delivered { receipt, reply })
    }
}

impl Message<DropWait> for Agents {
    type Reply = ();

    async fn handle(&mut self, DropWait(id): DropWait, _: &mut Context<Self, Self::Reply>) {
        self.waits.remove(&id);
        self.bound.retain(|_, b| *b != id);
    }
}

impl Message<Resolve> for Agents {
    type Reply = Vec<Result<String, String>>;

    async fn handle(&mut self, m: Resolve, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.resolve_all(m.from, &m.to).await
    }
}

impl Message<PeerConsumed> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PeerConsumed, _: &mut Context<Self, Self::Reply>) {
        self.enqueued.remove(&m.id);
        // The turn that consumed an awaited request is bound to it (50 §5.3).
        if self
            .waits
            .get(&m.id)
            .is_some_and(|w| w.to == format!("session:{}", m.session))
        {
            self.bound.insert((m.session, m.turn), m.id);
        }
        // Live typed provenance (50 §10), from the durable record only.
        match self.store.peer_record(m.session, m.id).await {
            Ok(Some(r)) => {
                self.peer_changed(&r.receipt).await;
                self.rt
                    .publish(PeerDelivered {
                        session: m.session,
                        origin: origin_of(&r),
                        text: r.text,
                        seq: m.seq,
                        turn: m.turn,
                    })
                    .await;
            }
            Ok(None) => {
                tracing::error!(session = %m.session, id = %m.id, "consumed exchange has no record")
            }
            Err(e) => tracing::error!(id = %m.id, "peer_record: {e:#}"),
        }
    }
}

/// The provenance a record carries into transcripts and events.
fn origin_of(r: &PeerRecord) -> PeerOrigin {
    PeerOrigin {
        id: r.receipt.id,
        from: r.from.clone(),
        reply_to: r.receipt.reply_to,
        scope: r.request_scope,
    }
}

impl Message<PeerIntakeFailed> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PeerIntakeFailed, _: &mut Context<Self, Self::Reply>) {
        tracing::info!(session = %m.session, id = %m.id, "peer intake failed: {}", m.reason);
        self.enqueued.remove(&m.id);
        self.ledger.release(m.session, m.ticket);
        self.network_input_dropped(m.ticket).await;
        match self.store.exchange_receipt(m.id).await {
            Ok(Some(r)) if r.state == PeerMessageState::Accepted => {
                // The runner did not settle the row itself (closed mailbox, refusal).
                match self.store.fail_exchange(m.id, &m.reason).await {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => tracing::error!(id = %m.id, "fail_exchange: {e:#}"),
                }
            }
            Ok(Some(r)) => self.peer_changed(&r).await,
            Ok(None) => {}
            Err(e) => tracing::error!(id = %m.id, "exchange_receipt: {e:#}"),
        }
        self.idle_check(m.session).await;
    }
}

impl Message<PeerTurnFinished> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PeerTurnFinished, _: &mut Context<Self, Self::Reply>) {
        // Arrives before `TurnFinished`: `child_finished` takes the text by turn.
        if self
            .sessions
            .get(&m.session)
            .is_some_and(|l| l.parent.is_some())
        {
            self.turn_text.insert((m.session, m.turn), m.text.clone());
        }
        self.network_turn_finished(&m).await;
    }
}

impl Message<GetCommunication> for Agents {
    type Reply = Result<CommunicationState, String>;

    async fn handle(
        &mut self,
        m: GetCommunication,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(s) = m.session {
            self.mirror(s).await;
        }
        self.communication
            .0
            .read()
            .state(m.session)
            .map_err(|e| e.to_string())
    }
}

impl Message<SetCommunication> for Agents {
    type Reply = Result<CommunicationState, String>;

    async fn handle(
        &mut self,
        m: SetCommunication,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match m.session {
            None => {
                // Persist first (50 §8): a failed write keeps the old state.
                write_config_atomic(
                    &self.rt.data_dir.join("config.yaml"),
                    "agent",
                    "communication",
                    if m.enabled { "true" } else { "false" },
                )
                .map_err(|e| format!("config.yaml: {e}"))?;
                self.revoking(|p| p.set_global(m.enabled, "command")).await;
                // `set_global` invalidated every generation in both
                // directions: settle what those generations carried
                // (accepted input, waits, exported work) before replying.
                self.revoke(None).await;
            }
            Some(s) => {
                self.mirror(s).await;
                self.revoking(|p| p.set_session(s, m.enabled))
                    .await
                    .map_err(|e| e.to_string())?;
                if !m.enabled {
                    self.revoke(Some(s)).await;
                }
            }
        }
        self.communication_changed(m.session).await;
        let state = self
            .communication
            .0
            .read()
            .state(m.session)
            .map_err(|e| e.to_string())?;
        Ok(state)
    }
}

impl Message<SetPeerScope> for Agents {
    type Reply = Result<CommunicationState, String>;

    async fn handle(&mut self, m: SetPeerScope, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.mirror(m.root).await;
        let (changed, alias) = {
            let p = self.communication.0.read();
            let s = p.state(Some(m.root)).map_err(|e| e.to_string())?;
            let alias = match &m.alias {
                None => s.alias,
                Some(a) => {
                    let a: String = a
                        .trim()
                        .chars()
                        .take(crate::communication::ALIAS_MAX)
                        .collect();
                    (!a.is_empty()).then_some(a)
                }
            };
            (s.scope != m.scope, alias)
        };
        // Durable first (the trusted record), then the live barrier.
        self.store
            .set_root_scope(m.root, m.scope, alias.as_deref())
            .await
            .map_err(comm_err)?;
        self.revoking(|p| p.set_scope(m.root, m.scope, Some(alias.unwrap_or_default())))
            .await
            .map_err(|e| e.to_string())?;
        if changed {
            // Cross-root routes admitted under the old scope are gone (50 §8).
            self.expire_waits("scope changed", |w| !w.generation.live());
            self.network_revoked(Some(m.root), "scope changed").await;
        }
        self.communication_changed(Some(m.root)).await;
        self.communication
            .0
            .read()
            .state(Some(m.root))
            .map_err(|e| e.to_string())
    }
}

impl Message<ListPeers> for Agents {
    type Reply = Result<PeerPage, String>;

    async fn handle(&mut self, m: ListPeers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.mirror(m.session).await;
        let limit = m
            .limit
            .filter(|l| *l > 0)
            .unwrap_or(DISCOVER_DEFAULT_LIMIT)
            .min(DISCOVER_MAX_LIMIT) as usize;
        let query = m
            .query
            .as_deref()
            .map(str::to_lowercase)
            .filter(|q| !q.is_empty());
        let mut rows = self.directory(m.session).await?;
        arrange(&mut rows, m.active_only);
        if let Some(q) = &query {
            rows.retain(|p| p.name.to_lowercase().contains(q) || p.address.contains(q.as_str()));
        }
        // The cursor is the last address of the previous page (stable order).
        let start = match &m.cursor {
            Some(c) => rows
                .iter()
                .position(|p| p.address == *c)
                .map(|i| i + 1)
                .ok_or_else(|| "unknown cursor".to_string())?,
            None => 0,
        };
        let end = (start + limit).min(rows.len());
        let cursor = (end < rows.len()).then(|| rows[end - 1].address.clone());
        Ok(PeerPage {
            peers: rows.drain(start..end).collect(),
            cursor,
        })
    }
}

impl Agents {
    /// The full permitted directory for `session` (50 §5.1) in stable order:
    /// same-tree sessions (parent, children, siblings, ancestors,
    /// descendants), then other `owner` roots and their trees when this
    /// root is `owner`, then configured remotes. Each local candidate passes
    /// [`Policy::route`]; nothing dormant is woken and no network is probed.
    async fn directory(&mut self, session: SessionId) -> Result<Vec<PeerInfo>, String> {
        let root = {
            let p = self.communication.0.read();
            p.enabled(session).map_err(|e| e.to_string())?;
            p.root_of(session).map_err(|e| e.to_string())?
        };
        let mut rows = self
            .store
            .list_sessions()
            .await
            .map_err(|e| e.to_string())?;
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.0.cmp(&b.id.0)));
        // Mirror every row so `route` never fails on a dangling link.
        for r in &rows {
            self.mirror(r.id).await;
        }
        let me_parent = rows.iter().find(|r| r.id == session).and_then(|r| r.parent);
        // 69 P: engine display names, asked once; rows owned elsewhere are
        // `remote` (no activity here — their owner would know), ours carry
        // this engine's name for symmetry.
        let (me_name, names) = peer_mesh::engine_names().await;
        // Rows keep their session id for the activity lookup, which runs once
        // the policy guard is released.
        let mut same = Vec::new();
        let mut other = Vec::new();
        {
            let mut p = self.communication.0.write();
            for r in &rows {
                if r.id == session || p.route(session, r.id).is_err() {
                    continue;
                }
                let Ok(their_root) = p.root_of(r.id) else {
                    continue;
                };
                let relationship = if their_root != root {
                    PeerRelationship::Root
                } else if Some(r.id) == me_parent {
                    PeerRelationship::Parent
                } else if r.parent == Some(session) {
                    PeerRelationship::Child
                } else if r.parent.is_some() && r.parent == me_parent {
                    PeerRelationship::Sibling
                } else if descends(&rows, r.id, session) {
                    PeerRelationship::Descendant
                } else if descends(&rows, session, r.id) {
                    PeerRelationship::Ancestor
                } else {
                    // Same tree, another branch: reachable like a descendant.
                    PeerRelationship::Descendant
                };
                let name = if their_root == root {
                    r.name.clone().unwrap_or_else(|| short(r.id))
                } else {
                    // Never another root's title (50 §5.1): the owner alias or a short id.
                    p.state(Some(their_root))
                        .ok()
                        .and_then(|s| s.alias)
                        .filter(|_| r.id == their_root)
                        .unwrap_or_else(|| short(r.id))
                };
                let kind = match &r.agent {
                    None => PeerKind::Native,
                    // `"<node>"` is a theuth there and answers back over its
                    // conn (67 §8); `"<harness>@<node>"` is that harness, which
                    // has no peer tools, like a local one.
                    Some(a) if crate::peers::parse_agent(a).is_ok_and(|(h, e)| {
                        h.is_none() && self.nodes.read().is_ok_and(|n| n.contains_key(e))
                    }) =>
                    {
                        PeerKind::Node
                    }
                    Some(_) => PeerKind::Acp,
                };
                let remote = !r.here && !r.owner.is_empty();
                let state = if remote {
                    PeerState::Remote
                } else if self.approvals.contains_key(&r.id) || self.running.contains(&r.id) {
                    PeerState::Running
                } else if self.waits.values().any(|w| w.session == r.id) {
                    PeerState::Awaiting
                } else if self.live(r.id).is_some() {
                    PeerState::Idle
                } else {
                    PeerState::Stored
                };
                // The title is the caller's own tree's business only (62 §0.7);
                // the tags cross roots so a model can tell related work apart
                // (50 §5.1 as-built).
                let title = (their_root == root).then(|| r.title.clone());
                let info = PeerInfo {
                    address: format!("session:{}", r.id),
                    name,
                    kind,
                    relationship,
                    state,
                    send: true,
                    // An external harness cannot call theuth peer tools (50
                    // §6.4); a node child can, back over its conn (67 §8).
                    reply: kind != PeerKind::Acp,
                    stream: false,
                    activity: None,
                    title,
                    tags: r.tags.clone(),
                    engine: Some(if remote {
                        peer_mesh::engine_name(&names, &r.owner)
                    } else {
                        me_name.clone()
                    }),
                };
                if their_root == root {
                    same.push((r.id, info));
                } else {
                    other.push((r.id, info));
                }
            }
        }
        same.extend(other);
        // Remote rows have no activity here: their owner's engine would know.
        let ids: Vec<SessionId> = same
            .iter()
            .filter(|(_, info)| info.state != PeerState::Remote)
            .map(|(id, _)| *id)
            .collect();
        let mut activity = self.activity_for(&ids).await;
        let mut out: Vec<PeerInfo> = same
            .into_iter()
            .map(|(id, mut info)| {
                info.activity = activity.remove(&id).map(peer_activity);
                info
            })
            .collect();
        // 67 §8: the session on another engine that opened this root, first —
        // it is the only peer `parent` names here, and nothing else lists it.
        if let Some(p) = self.remote_parent.get(&session) {
            out.insert(0, remote_parent_row(p));
        }
        out.extend(self.network.remote_peers(root));
        Ok(out)
    }
}

/// The `parent` row of a root a remote theuth opened (67 §8): addressed as
/// `parent`, answerable, never dormant or woken by listing.
fn remote_parent_row(p: &RemoteParent) -> PeerInfo {
    PeerInfo {
        address: "parent".to_owned(),
        name: match &p.engine {
            Some(e) => format!("parent (session {} on {e})", p.session),
            None => format!("parent (session {})", p.session),
        },
        kind: PeerKind::Parent,
        relationship: PeerRelationship::Parent,
        state: PeerState::Idle,
        send: true,
        reply: true,
        stream: false,
        activity: None,
        title: None,
        tags: Vec::new(),
        engine: p.engine.clone(),
    }
}

/// [`SessionActivity`] as a directory row shows it (62 §3.4): liveness
/// detail without `Awaiting { from }` / `Approval { tool }` content. `Stored`
/// when no actor is loaded; `Offline` is never derived here (no node-down
/// notion reaches the supervisor).
fn peer_activity(a: SessionActivity) -> PeerActivity {
    let state = if !a.live {
        PeerLiveState::Stored
    } else {
        match a.state {
            AgentState::Idle => PeerLiveState::Idle,
            AgentState::Running => PeerLiveState::Running,
            AgentState::Awaiting { .. } => PeerLiveState::Awaiting,
            AgentState::Approval { .. } => PeerLiveState::Approval,
            AgentState::Error => PeerLiveState::Error,
            AgentState::Cancelled => PeerLiveState::Cancelled,
        }
    };
    PeerActivity {
        state,
        last_active: a.last_active,
        turn_secs: a
            .turn_started_at
            .map(|t| (Utc::now() - t).num_seconds().max(0) as u64),
        jobs: a.jobs,
        ptys: a.ptys,
        children: a.children,
    }
}

/// `discover_peers` order and `active_only` filter (62 §3.4): working rows
/// first, then most recently active; ties keep [`Agents::directory`]'s order.
/// Remote rows have no activity and stay last.
fn arrange(rows: &mut Vec<PeerInfo>, active_only: bool) {
    if active_only {
        rows.retain(|p| p.activity.as_ref().is_some_and(PeerActivity::active));
    }
    rows.sort_by_key(|p| {
        let a = p.activity.as_ref();
        std::cmp::Reverse((
            a.is_some_and(PeerActivity::active),
            a.and_then(|a| a.last_active),
        ))
    });
}

/// `s` is a strict descendant of `ancestor` among `rows`.
fn descends(rows: &[SessionSummary], mut s: SessionId, ancestor: SessionId) -> bool {
    for _ in 0..rows.len() {
        match rows.iter().find(|r| r.id == s).and_then(|r| r.parent) {
            Some(p) if p == ancestor => return true,
            Some(p) => s = p,
            None => return false,
        }
    }
    false
}

/// Short session id for a nameless peer row: the first 8 hex chars.
fn short(id: SessionId) -> String {
    id.to_string().chars().take(8).collect()
}

impl Message<GetPeerMessage> for Agents {
    type Reply = Result<PeerRecord, String>;

    async fn handle(
        &mut self,
        m: GetPeerMessage,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .peer_record(m.session, m.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "unknown message".to_string())
    }
}

impl Agents {
    /// Live-waiter derived pending sets for `session` (50 §5.3):
    /// `(awaiting_answer, awaiting_reply)` = received requests whose sender
    /// is blocked on `session`, and `session`'s own sent requests it is
    /// blocked on. Never abandoned oneshots.
    fn pending_requests(&self, session: SessionId) -> (Vec<Uuid>, Vec<Uuid>) {
        let me = format!("session:{session}");
        let mut received = Vec::new();
        let mut sent = Vec::new();
        for (id, w) in &self.waits {
            if w.to == me {
                received.push(*id);
            }
            if w.session == session {
                sent.push(*id);
            }
        }
        (received, sent)
    }
}

impl Message<ListPeerMessages> for Agents {
    type Reply = Result<PeerMessagePage, String>;

    async fn handle(
        &mut self,
        m: ListPeerMessages,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let limit = m
            .limit
            .filter(|l| *l > 0)
            .unwrap_or(DISCOVER_DEFAULT_LIMIT)
            .min(DISCOVER_MAX_LIMIT);
        let after: Option<Uuid> = match &m.cursor {
            Some(c) => Some(c.parse().map_err(|_| "unknown cursor".to_string())?),
            None => None,
        };
        let (received, sent) = self.pending_requests(m.session);
        if m.pending_only {
            // Bounded by the wait caps: load each live request's own record.
            let mut messages = Vec::with_capacity(received.len() + sent.len());
            for id in received.iter().chain(&sent) {
                if let Some(r) = self
                    .store
                    .peer_record(m.session, *id)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    messages.push(r);
                }
            }
            messages.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            return Ok(PeerMessagePage {
                awaiting_answer: received,
                awaiting_reply: sent,
                messages,
                cursor: None,
            });
        }
        let messages = self
            .store
            .peer_records(m.session, after, limit)
            .await
            .map_err(|e| e.to_string())?;
        let cursor = (messages.len() == usize::from(limit))
            .then(|| messages.last().map(|r| r.receipt.id.to_string()))
            .flatten();
        let on_page = |set: &[Uuid]| -> Vec<Uuid> {
            messages
                .iter()
                .map(|r| r.receipt.id)
                .filter(|id| set.contains(id))
                .collect()
        };
        Ok(PeerMessagePage {
            awaiting_answer: on_page(&received),
            awaiting_reply: on_page(&sent),
            messages,
            cursor,
        })
    }
}

impl Message<AnswerPeerRequest> for Agents {
    type Reply = Result<PeerReceipt, String>;

    async fn handle(
        &mut self,
        m: AnswerPeerRequest,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // The durable record decides which side the human acts for.
        let request = self
            .store
            .peer_record(m.session, m.reply_to)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no such pending request for this session".to_string())?;
        let me = format!("session:{}", m.session);
        if request.from == me {
            // The blocked sender's own request: resolve its wait as the owner,
            // no peer identity forged, the recipient's copy untouched.
            if !self
                .waits
                .get(&m.reply_to)
                .is_some_and(|w| w.session == m.session)
            {
                return Err("this session is not awaiting a reply to that message".into());
            }
            let text = self.redact(&m.text).into_owned();
            if text.len() > PEER_TEXT_MAX {
                return Err(format!("answer exceeds {PEER_TEXT_MAX} bytes"));
            }
            let original = self
                .store
                .owner_answer(m.reply_to, m.session, &text)
                .await
                .map_err(comm_err)?;
            self.rt
                .publish(AgentEvent::MessageDelivered {
                    session: m.session,
                    from: "you".into(),
                    text,
                })
                .await;
            self.settle_wait(&original);
            self.peer_changed(&original).await;
            return Ok(original);
        }
        if request.receipt.to != me {
            return Err("no such pending request for this session".into());
        }
        let sender = PeerAddress::local(&request.from);
        let text = self.redact(&m.text).into_owned();
        if text.len() > PEER_TEXT_MAX {
            return Err(format!("answer exceeds {PEER_TEXT_MAX} bytes"));
        }
        let Some(sender) = sender else {
            // A remote requester (`peer:<binding>`): the answer travels on
            // its task channel, addressed by the binding's live alias.
            let binding: Uuid = request
                .from
                .strip_prefix("peer:")
                .and_then(|b| b.parse().ok())
                .ok_or_else(|| "unknown message".to_string())?;
            let alias = self
                .store
                .peer_binding_by_id(binding)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "the requesting peer's binding was revoked".to_string())?
                .alias;
            let receipt = self
                .network_send(m.session, &alias, &text, None, Some(m.reply_to), false)
                .await?;
            // Provenance: the human answered for this session.
            let _ = self.store.owner_answer(m.reply_to, m.session, &text).await;
            return Ok(receipt);
        };
        let routed = self.communication.0.write().route(m.session, sender);
        let generation = routed.map_err(|e| self.route_error(m.session, sender, e))?;
        let (from_key, to_key) = (format!("session:{}", m.session), request.from.clone());
        let reply = self
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from_key,
                recipient: &to_key,
                sender_session: Some(m.session),
                recipient_session: Some(sender),
                request_id: None,
                reply_to: Some(m.reply_to),
                request_scope: request.request_scope,
                generation: generation.revision() as i64,
                body: &text,
            })
            .await
            .map_err(comm_err)?;
        // Provenance: the reply travels as `session:<recipient>`, its author is the human.
        let original = self
            .store
            .owner_answer(m.reply_to, m.session, &text)
            .await
            .map_err(comm_err)?;
        self.rt
            .publish(AgentEvent::MessageDelivered {
                session: m.session,
                from: "you".into(),
                text: text.clone(),
            })
            .await;
        if self.settle_wait(&original) {
            let settled = self
                .store
                .consume_exchange_direct(reply.id, sender, generation.revision() as i64)
                .await
                .map_err(comm_err)?
                .unwrap_or(reply);
            self.peer_changed(&settled).await;
            self.peer_changed(&original).await;
            return Ok(settled);
        }
        // Nobody waits any more: the answer is delivered once as correlated input.
        self.peer_changed(&reply).await;
        let record = self
            .store
            .peer_record(sender, reply.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "unknown message".to_string())?;
        self.enqueued.insert(reply.id);
        if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
            self.enqueued.remove(&reply.id);
            let failed = self
                .store
                .fail_exchange(reply.id, &reason)
                .await
                .unwrap_or(reply);
            self.peer_changed(&failed).await;
            return Err(reason);
        }
        Ok(reply)
    }
}

impl Message<NewRootSession> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(
        &mut self,
        m: NewRootSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.new_root(m.cwd.as_deref(), m.scope, None, None).await
    }
}

impl Agents {
    /// A durable root: created, `cwd` recorded, scope declared (50 §2.1).
    /// `model`/`effort` are the root's own (`CreateSessionOptions`); `None` = the default.
    async fn new_root(
        &mut self,
        cwd: Option<&Path>,
        scope: PeerScope,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<SessionId, String> {
        let (id, _) = self
            .session(None, None, None, (model, effort))
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some(cwd) = cwd {
            self.store
                .set_cwd(id, cwd)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        if scope != PeerScope::Isolated {
            self.store
                .set_root_scope(id, scope, None)
                .await
                .map_err(comm_err)?;
            self.communication
                .0
                .write()
                .set_scope(id, scope, None)
                .map_err(|e| e.to_string())?;
            self.communication_changed(Some(id)).await;
        }
        Ok(id)
    }

    /// The durable half of [`CreateSession`]: a root with its own model and
    /// effort, its project label, and — for a remote parent's `tools` (67 §8)
    /// — a ceiling installed exactly as [`SpawnChild`] installs a subagent's.
    /// This root tops its own chain, so `cap_for` is simply `cap` for it and
    /// its descendants.
    async fn create_durable(
        &mut self,
        cwd: Option<&Path>,
        project: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        cap: Option<Risk>,
        remote: Option<RemoteParent>,
    ) -> Result<SessionId, String> {
        let id = self.new_root(cwd, PeerScope::Owner, model, effort).await?;
        if project.is_some() {
            self.store
                .set_project(id, project)
                .await
                .map_err(|e| e.to_string())?;
            self.search.invalidate();
        }
        if let Some(cap) = cap {
            self.install_cap(id, cap).await;
        }
        if let Some(remote) = remote {
            self.remote_parent.insert(id, remote);
        }
        Ok(id)
    }

    /// Remember `cap` for the board row and put it on the registry before the
    /// session's first `ListTools`; `cap_for` takes the minimum down the tree,
    /// so a capped session cannot spawn a looser one.
    async fn install_cap(&mut self, id: SessionId, cap: Risk) {
        self.caps.insert(id, cap);
        if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
            let _ = reg
                .tell(SetCap {
                    session: id,
                    cap: Some(cap),
                })
                .await;
        }
    }
}

/// [`runtime::write_config_value`] with an atomic replace: the edit lands
/// in a sibling temp file that is renamed over `path`, so a failed write
/// leaves the old file intact (50 §8).
fn write_config_atomic(path: &Path, section: &str, key: &str, value: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("yaml.{}.tmp", std::process::id()));
    match std::fs::copy(path, &tmp) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Err(e) = runtime::write_config_value(&tmp, section, key, value) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

impl Agents {
    /// Identity lines of a subagent (design doc 07 §10), or of a root a
    /// remote theuth opened (67 §8); `""` for any other root session.
    fn peer_context(&self, session: SessionId) -> String {
        if let Some(p) = self.remote_parent.get(&session) {
            let engine = p.engine.as_deref().unwrap_or("another engine");
            return format!(
                "## Peers\nYou were opened by session {} on engine {engine} (address it as `parent`; send_message to it, its messages reach you as <message> inputs).\nYour final message is delivered to it when your turn ends.",
                p.session
            );
        }
        match self.sessions.get(&session).and_then(|l| l.parent.as_ref()) {
            Some((parent, name)) => format!(
                "## Peers\nYou are `{name}`, a subagent of session {parent} (address it as `parent`).\nYour final message is delivered to the parent when your turn ends."
            ),
            None => String::new(),
        }
    }

    /// Whether the Claude Code identity line leads the system prompt for `model`
    /// (design doc 05 §2.3): Anthropic namespace with a login in force.
    fn claude_code(&self, model: &Resolved) -> bool {
        model
            .genai
            .split_once("::")
            .is_some_and(|(ns, _)| ns == "anthropic")
            && self.auth.login("anthropic")
    }

    fn skills_dir(&self) -> std::path::PathBuf {
        skills::dir(&self.rt.data_dir)
    }
}

impl Message<PeerContext> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: PeerContext, _: &mut Context<Self, Self::Reply>) -> String {
        self.peer_context(m.session)
    }
}

impl Agents {
    /// [`AgentInfo`] for every descendant of `session`, store rows in
    /// pre-order plus live state from this actor (design doc 19 §2.1).
    async fn agents(&self, session: SessionId) -> Vec<AgentInfo> {
        let store = self.store_for(session);
        let rows = match store.descendants(session).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(session = %session, "descendants: {e:#}");
                return Vec::new();
            }
        };
        let names: HashMap<SessionId, &str> = rows
            .iter()
            .filter_map(|(_, s)| Some((s.id, s.name.as_deref()?)))
            .collect();
        let ids: Vec<SessionId> = rows.iter().map(|(_, r)| r.id).collect();
        let mut activity = self.activity_for(&ids).await;
        let mut out = Vec::with_capacity(rows.len());
        for (depth, row) in &rows {
            let parent = row.parent.unwrap_or(session);
            // Names of cold siblings are known here; keep the two states equal.
            let state = self.state_of(row.id, Some(parent), &names);
            let mut activity = activity.remove(&row.id).unwrap_or_default();
            activity.state = state.clone();
            out.push(self.info_row(store, row, *depth, parent, state, activity).await);
        }
        out
    }

    /// [`SessionActivity`] for `ids` (62 §0.5): one `JobCounts` ask for all
    /// of them, the rest from this actor's maps; the store is read only for
    /// a cold session's `updated_at`.
    async fn activity_for(&self, ids: &[SessionId]) -> HashMap<SessionId, SessionActivity> {
        let counts = match ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR) {
            Ok(Some(jobs)) => jobs
                .ask(tools::JobCounts(ids.to_vec()))
                .await
                .unwrap_or_default(),
            _ => HashMap::new(),
        };
        self.assemble_activity(ids, &counts).await
    }

    /// [`Self::activity_for`] given the `JobCounts` reply.
    async fn assemble_activity(
        &self,
        ids: &[SessionId],
        counts: &HashMap<SessionId, (u16, u16)>,
    ) -> HashMap<SessionId, SessionActivity> {
        // Live descendants per ancestor: walk each live session's parent chain.
        let mut children: HashMap<SessionId, u16> = HashMap::new();
        for (id, live) in &self.sessions {
            if !live.actor.is_alive() {
                continue;
            }
            let mut cur = *id;
            while let Some((p, _)) = self.sessions.get(&cur).and_then(|l| l.parent.as_ref()) {
                *children.entry(*p).or_default() += 1;
                cur = *p;
            }
        }
        let names = HashMap::new();
        let mut out = HashMap::with_capacity(ids.len());
        for &id in ids {
            let parent = self.sessions.get(&id).and_then(|l| l.parent.as_ref().map(|(p, _)| *p));
            let turn_started_at = self.child_turns.get(&id).map(|ct| ct.started_at);
            let mut last_active = [
                self.last_input.get(&id).copied(),
                self.last_stop.get(&id).map(|(_, at)| *at),
                turn_started_at,
            ]
            .into_iter()
            .flatten()
            .max();
            if last_active.is_none() {
                last_active = self
                    .store_for(id)
                    .session(id)
                    .await
                    .ok()
                    .flatten()
                    .map(|s| s.updated_at);
            }
            let (jobs, ptys) = counts.get(&id).copied().unwrap_or_default();
            out.insert(
                id,
                SessionActivity {
                    state: self.state_of(id, parent, &names),
                    last_active,
                    turn_started_at,
                    jobs,
                    ptys,
                    children: children.get(&id).copied().unwrap_or_default(),
                    asks: None,
                    live: self.live(id).is_some(),
                },
            );
        }
        out
    }

    /// Every stored session row, durable store first then each open private
    /// tree's, in each store's `list_sessions` order.
    async fn all_rows(&self) -> Vec<SessionSummary> {
        let mut rows = self.store.list_sessions().await.unwrap_or_else(|e| {
            tracing::error!("list sessions: {e:#}");
            Vec::new()
        });
        for p in self.private.values().filter(|p| !p.closing) {
            rows.extend(p.store.list_sessions().await.unwrap_or_default());
        }
        rows
    }

    /// Live state of `id` as the roster shows it (design doc 19 §2.1).
    /// `names` maps sibling ids to their peer names for `Awaiting { from }`.
    fn state_of(
        &self,
        id: SessionId,
        parent: Option<SessionId>,
        names: &HashMap<SessionId, &str>,
    ) -> AgentState {
        if let Some((_, tool)) = self.approvals.get(&id) {
            AgentState::Approval { tool: tool.clone() }
        } else if let Some(w) = self.waits.values().find(|w| w.session == id) {
            // The awaited peer by its convenience name when it has one.
            let from = match PeerAddress::local(&w.to) {
                Some(p) if Some(p) == parent => "parent".to_owned(),
                Some(p) => names
                    .get(&p)
                    .map(|n| (*n).to_owned())
                    .or_else(|| Some(self.sessions.get(&p)?.parent.as_ref()?.1.clone()))
                    .unwrap_or_else(|| w.to.clone()),
                None => w.to.clone(),
            };
            AgentState::Awaiting { from }
        } else if self.running.contains(&id) {
            AgentState::Running
        } else {
            match self.last_stop.get(&id).map(|(reason, _)| reason.as_str()) {
                Some("error") => AgentState::Error,
                Some("cancelled") => AgentState::Cancelled,
                _ => AgentState::Idle,
            }
        }
    }

    /// One roster row from its store row plus live state.
    async fn info_row(
        &self,
        store: &Store,
        row: &SessionSummary,
        depth: u8,
        parent: SessionId,
        state: AgentState,
        activity: SessionActivity,
    ) -> AgentInfo {
        let id = row.id;
        // ponytail: one message replay per row; a `last_text` column if the picker gets slow.
        let last = store.last_turn_text(id).await.unwrap_or_else(|e| {
            tracing::error!(session = %id, "last_turn_text: {e:#}");
            String::new()
        });
        AgentInfo {
            id,
            name: row.name.clone().unwrap_or_default(),
            parent,
            depth,
            agent: row.agent.clone(),
            model: row.model.clone(),
            state,
            started_at: self.child_turns.get(&id).map(|ct| ct.started_at),
            finished_at: self.last_stop.get(&id).map(|(_, at)| *at),
            last: first_line(&last),
            project: row.project.clone(),
            retention: self.retention_of(id),
            activity,
            cap: self.caps.get(&id).copied(),
        }
    }

    /// [`ListActivity`]'s rows (62 §3.2): every root, plus every session with
    /// a live actor or a running job; `depth` from its own root.
    async fn activity_rows(&self) -> Vec<AgentInfo> {
        let rows = self.all_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        let mut activity = self.activity_for(&ids).await;
        // Pre-order like `agents()`: roots in roster order, then each one's
        // children oldest first. A cold child is skipped but its live
        // descendants are still walked (their depth counts it).
        let mut kids: HashMap<SessionId, Vec<&SessionSummary>> = HashMap::new();
        for r in &rows {
            if let Some(p) = r.parent {
                kids.entry(p).or_default().push(r);
            }
        }
        for v in kids.values_mut() {
            v.sort_by_key(|r| r.created_at);
        }
        let mut stack: Vec<(&SessionSummary, u8)> = rows
            .iter()
            .filter(|r| r.parent.is_none())
            .rev()
            .map(|r| (r, 0))
            .collect();
        let mut out = Vec::new();
        while let Some((row, depth)) = stack.pop() {
            if let Some(children) = kids.get(&row.id) {
                stack.extend(children.iter().rev().map(|c| (*c, depth.saturating_add(1))));
            }
            let Some(a) = activity.remove(&row.id) else {
                continue;
            };
            if depth > 0 && !a.live && a.jobs + a.ptys == 0 {
                continue;
            }
            let parent = row.parent.unwrap_or(row.id);
            let state = a.state.clone();
            out.push(
                self.info_row(self.store_for(row.id), row, depth, parent, state, a)
                    .await,
            );
        }
        out
    }
}

impl Message<ListActivity> for Agents {
    type Reply = Vec<AgentInfo>;

    async fn handle(&mut self, _: ListActivity, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.activity_rows().await
    }
}

impl Agents {
    /// 62 §3.3 step 1: the rows `targets` names, in selector order, `before`
    /// filled, every resolved row `Skipped{"dry run"}` and an unknown id
    /// `Failed{"unknown session"}`. `Running`/`Roots` never name a child.
    async fn resolve_targets(&self, targets: &BroadcastTargets) -> Vec<BroadcastRow> {
        let rows = self.all_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        let mut activity = self.activity_for(&ids).await;
        let by_id: HashMap<SessionId, &SessionSummary> = rows.iter().map(|r| (r.id, r)).collect();
        let row = |id: SessionId, before: SessionActivity| {
            let s = by_id[&id];
            BroadcastRow {
                session: id,
                title: s.title.clone(),
                parent: s.parent,
                before,
                outcome: BroadcastOutcome::Skipped {
                    reason: "dry run".into(),
                },
            }
        };
        match targets {
            BroadcastTargets::Running | BroadcastTargets::Roots => rows
                .iter()
                .filter(|r| r.parent.is_none())
                .filter_map(|r| {
                    let a = activity.remove(&r.id)?;
                    let keep = match targets {
                        BroadcastTargets::Roots => a.live,
                        _ => {
                            a.jobs + a.ptys > 0
                                || matches!(
                                    a.state,
                                    AgentState::Running
                                        | AgentState::Awaiting { .. }
                                        | AgentState::Approval { .. }
                                )
                        }
                    };
                    keep.then(|| row(r.id, a))
                })
                .collect(),
            BroadcastTargets::Sessions { ids } => {
                let mut seen = HashSet::with_capacity(ids.len());
                ids.iter()
                    .filter(|id| seen.insert(**id))
                    .map(|&id| match activity.remove(&id) {
                        Some(a) => row(id, a),
                        None => BroadcastRow {
                            session: id,
                            title: String::new(),
                            parent: None,
                            before: SessionActivity::default(),
                            outcome: BroadcastOutcome::Failed {
                                error: "unknown session".into(),
                            },
                        },
                    })
                    .collect()
            }
        }
    }

    /// [`Broadcast`] (62 §3.3 steps 1–5). `Err` only for the target cap; every
    /// per-session outcome is a row, and a row's failure never stops the loop.
    async fn broadcast(&mut self, req: BroadcastRequest) -> Result<BroadcastReceipt, String> {
        let mut rows = self.resolve_targets(&req.targets).await;
        let max = self.cfg.broadcast.max_targets;
        let n = rows
            .iter()
            .filter(|r| !matches!(r.outcome, BroadcastOutcome::Failed { .. }))
            .count();
        if n > max {
            return Err(format!("too many targets ({n} > {max})"));
        }
        let id = Uuid::now_v7();
        let at = Utc::now();
        if req.dry_run {
            return Ok(BroadcastReceipt { id, at, rows });
        }
        // Skips are decided before the count in the marker line: `n` is what
        // is actually sent.
        let nodes = node_lines(&self.nodes);
        for r in &mut rows {
            if !matches!(r.outcome, BroadcastOutcome::Skipped { .. }) {
                continue;
            }
            let store_row = self.store_for(r.session).session(r.session).await.ok().flatten();
            let reason = if store_row
                .as_ref()
                .and_then(|s| s.agent.as_deref())
                .is_some_and(|a| nodes.contains_key(a))
            {
                Some("remote node")
            } else if !r.before.live && r.parent.is_some() {
                Some("no actor and not a root")
            } else {
                None
            };
            if let Some(reason) = reason {
                r.outcome = BroadcastOutcome::Skipped {
                    reason: reason.into(),
                };
            } else {
                // Marks "send this one" for the loop below.
                r.outcome = BroadcastOutcome::Started;
            }
        }
        let n = rows
            .iter()
            .filter(|r| matches!(r.outcome, BroadcastOutcome::Started))
            .count();
        let short = &id.simple().to_string()[..4];
        let text = format!("[broadcast {short} · {n} sessions]\n{}", req.text);
        for r in &mut rows {
            if !matches!(r.outcome, BroadcastOutcome::Started) {
                continue;
            }
            let busy = self.running.contains(&r.session);
            let input_id = InputId::new();
            let sent = self
                .input(
                    r.session,
                    Some(input_id),
                    text.clone(),
                    req.files.clone(),
                    InputOrigin::External,
                    req.delivery,
                )
                .await;
            r.outcome = match sent {
                Err(error) => BroadcastOutcome::Failed { error },
                Ok(()) if busy => BroadcastOutcome::Queued {
                    input_id,
                    delivery: req.delivery,
                },
                Ok(()) => BroadcastOutcome::Started,
            };
        }
        Ok(BroadcastReceipt { id, at, rows })
    }
}

impl Message<Broadcast> for Agents {
    type Reply = Result<BroadcastReceipt, String>;

    async fn handle(&mut self, m: Broadcast, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.broadcast(m.0).await
    }
}

/// First non-empty line of `text`, trimmed, cut to 80 chars with `…`.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() > 80 {
        let mut s: String = line.chars().take(79).collect();
        s.push('…');
        s
    } else {
        line.to_owned()
    }
}

impl Message<ListAgents> for Agents {
    type Reply = Vec<AgentInfo>;

    async fn handle(&mut self, m: ListAgents, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.agents(m.session).await
    }
}

impl Message<Roster> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: Roster, _: &mut Context<Self, Self::Reply>) -> String {
        let agents = self.agents(m.session).await;
        if agents.is_empty() {
            return String::new();
        }
        let mut s = String::from("Subagents: ");
        for (i, a) in agents.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&a.name);
            s.push_str(" (");
            s.push_str(a.state.word());
            s.push(')');
        }
        s
    }
}

impl Message<ShowPrompt> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: ShowPrompt, _: &mut Context<Self, Self::Reply>) -> String {
        let peers = m.session.map(|s| self.peer_context(s)).unwrap_or_default();
        let (model, _) = self.resolve_model(&self.default_model);
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten();
        // A live session's toolset override (06); a session without an actor has none.
        let toolset = match m.session.and_then(|s| self.live(s)?.native().ok()) {
            Some(a) => a.ask(GetToolset).await.unwrap_or_default(),
            None => None,
        };
        // Same audience rule as `Turn::rebase`: a subagent or a scheduler-opened root is unattended.
        let mut attended = true;
        if let Some(s) = m.session {
            let store = self.store_for(s);
            attended = store.depth(s).await.unwrap_or(0) == 0
                && !store.unattended(s).await.unwrap_or(false);
        }
        let mut blocks = String::new();
        if let Some(reg) = &registry
            && let Ok(b) = reg.ask(ListPrompts).await
        {
            blocks = prompt_blocks(b, toolset.as_ref(), attended);
        }
        let subagent_model = self.subagent_model_id();
        let identity = match m.session {
            Some(s) => self.identity_of(s).await,
            None => None,
        };
        let mut prompt = frame::baseline(
            &self.rt.data_dir,
            identity.as_deref(),
            &peers,
            &blocks,
            self.claude_code(&model),
            subagent_model.as_deref(),
            attended,
        );
        if self.pool.is_some()
            && toolset.as_ref().is_none_or(|t| t.runtime)
            && let Some(reg) = registry
            && let Ok(specs) = reg.ask(ListTools(m.session)).await
        {
            prompt.push_str("\n\n");
            prompt.push_str(&repl::prompt_block(&specs, self.cfg.runtime_only));
        }
        prompt
    }
}

impl Message<ListSkills> for Agents {
    type Reply = Vec<SkillMeta>;

    async fn handle(
        &mut self,
        _: ListSkills,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<SkillMeta> {
        skills::index(&self.skills_dir())
    }
}

impl Message<LoadSkill> for Agents {
    type Reply = Result<SkillBody, String>;

    async fn handle(
        &mut self,
        m: LoadSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SkillBody, String> {
        skills::load(&self.skills_dir(), &m.name)
    }
}

impl Message<AddSkill> for Agents {
    type Reply = Result<SkillMeta, String>;

    async fn handle(
        &mut self,
        m: AddSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<SkillMeta, String> {
        skills::add(&self.skills_dir(), m)
    }
}

impl Message<RemoveSkill> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveSkill,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        skills::remove(&self.skills_dir(), &m.name)
    }
}

impl Message<AddSkillRepo> for Agents {
    type Reply = Result<Vec<SkillMeta>, String>;

    async fn handle(
        &mut self,
        m: AddSkillRepo,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Vec<SkillMeta>, String> {
        skills::add_repo(&self.skills_dir(), &m.url, m.name).await
    }
}

impl Message<RemoveSkillRepo> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveSkillRepo,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        skills::remove_repo(&self.skills_dir(), &m.name)
    }
}

impl Message<SyncSkills> for Agents {
    type Reply = Vec<tools::git::SyncResult>;

    async fn handle(
        &mut self,
        _: SyncSkills,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<tools::git::SyncResult> {
        tools::git::sync_all(&self.skills_dir()).await
    }
}

impl Message<DurableStore> for Agents {
    type Reply = Store;

    async fn handle(&mut self, _: DurableStore, _: &mut Context<Self, Self::Reply>) -> Store {
        self.store.clone()
    }
}

impl Message<UnloadSession> for Agents {
    type Reply = bool;

    async fn handle(&mut self, m: UnloadSession, _: &mut Context<Self, Self::Reply>) -> bool {
        // As `DeleteSession` stops an actor (68 §4.2): cancel the turn, kill,
        // forget the live entry; rows and peer state stay.
        let Some(live) = self.sessions.remove(&m.session) else {
            return false;
        };
        live.actor.tell_cancel().await;
        live.actor.kill().await;
        self.running.remove(&m.session);
        true
    }
}

impl Message<ListSessions> for Agents {
    type Reply = Vec<SessionSummary>;

    async fn handle(
        &mut self,
        _: ListSessions,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<SessionSummary> {
        self.list_sessions().await
    }
}

impl Agents {
    /// The durable session list with each row's live `running` flag.
    async fn list_sessions(&self) -> Vec<SessionSummary> {
        let mut rows = self.store.list_sessions().await.unwrap_or_else(|e| {
            tracing::error!("list sessions: {e:#}");
            Vec::new()
        });
        for r in &mut rows {
            r.running = self.running.contains(&r.id);
        }
        rows
    }
}

impl Message<Ancestors> for Agents {
    type Reply = Vec<AncestorInfo>;

    async fn handle(
        &mut self,
        m: Ancestors,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<AncestorInfo> {
        let mut chain = Vec::new();
        let mut id = m.session;
        loop {
            let parent = self.sessions.get(&id).and_then(|l| l.parent.clone());
            chain.push(AncestorInfo {
                id,
                title: None,
                name: parent.as_ref().map(|(_, n)| n.clone()),
            });
            match parent {
                Some((p, _)) if chain.len() < 32 => id = p,
                _ => break,
            }
        }
        // Titles live in sqlite only; one read for the root, the conversation the user knows.
        if let Some(root) = chain.last_mut() {
            root.title = self
                .store_for(root.id)
                .session(root.id)
                .await
                .ok()
                .flatten()
                .map(|s| s.title)
                .filter(|t| !t.trim().is_empty());
        }
        chain
    }
}

impl Message<ReadSession> for Agents {
    type Reply = Result<SessionRead, String>;

    async fn handle(&mut self, m: ReadSession, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 51 §4.2 / 66 §9: a private tree's sessions are readable only from
        // inside that tree; everything durable is readable from anywhere.
        let target_root = self.private_member.get(&m.session);
        if target_root.is_some() && target_root != self.private_member.get(&m.caller) {
            return Err(format!("session {} is private to another tree", m.session));
        }
        let store = self.store_for(m.session).clone();
        let mut summary = store
            .session(m.session)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("session {} not found", m.session))?;
        summary.retention = self.retention_of(m.session);
        let h = store.context_history(m.session).await.map_err(|e| format!("{e:#}"))?;
        let mut out = render(&h.messages, &h.entries);
        // Peer provenance (50 §9), as `load_session` marks it.
        if let Ok(rows) = store.consumed_exchanges(m.session).await {
            let peers: HashMap<i64, PeerOrigin> = rows.into_iter().map(|(seq, r)| (seq, origin_of(&r))).collect();
            for (seq, entry) in &mut out {
                if let HistoryEntry::User { peer, .. } = entry
                    && let Some(o) = peers.get(seq)
                {
                    *peer = Some(o.clone());
                }
            }
        }
        Ok(SessionRead {
            summary,
            history: out.into_iter().map(|(_, e)| e).collect(),
        })
    }
}

impl Message<LoadSession> for Agents {
    type Reply = Loaded;

    async fn handle(&mut self, m: LoadSession, _: &mut Context<Self, Self::Reply>) -> Loaded {
        // Raw rows and their context metadata in one consistent read (48 §5):
        // through the session actor when it is live (it owns admission and
        // fills editable/reason), from the store when no actor exists (idle
        // by construction) or the session runs on an external harness.
        let store = self.store_for(m.session).clone();
        let row = store.session(m.session).await.unwrap_or_else(|e| {
            tracing::error!(session = %m.session, "load session: {e:#}");
            None
        });
        let external = row.as_ref().is_some_and(|r| r.agent.is_some());
        let live = self.live(m.session).cloned();
        let native = live.as_ref().and_then(|a| a.native().ok());
        // `Err(reason)` = the actor refused a settled read (busy); the store
        // still answers so the reply is honest but not installable.
        let (read, refused) = match native {
            Some(a) => match a
                .ask(ContextHistoryNow {
                    settled_only: m.settled_only,
                })
                .await
            {
                Ok(h) => (Ok(h), None),
                Err(SendError::HandlerError(busy)) => {
                    (store.context_history(m.session).await, Some(busy))
                }
                Err(e) => (Err(anyhow::anyhow!("{e}")), None),
            },
            None => (store.context_history(m.session).await, None),
        };
        let (running, queued) = match &live {
            Some(actor) => (
                actor.is_running().await,
                match actor.native() {
                    Ok(a) => a.ask(QueuedInputs).await.unwrap_or_default(),
                    Err(_) => Vec::new(),
                },
            ),
            None => (false, Vec::new()),
        };
        // ponytail: every row is read and rendered per page; a seq-ranged
        // query is the upgrade if a measured session makes this slow.
        let (history, first_seq, more, skills, context) = match read {
            Ok(h) => {
                let ContextHistory {
                    revision,
                    messages,
                    epochs,
                    entries,
                    heads,
                    snapshot,
                } = h;
                // The frame is the latest epoch's tail (07 §8.4); its skills
                // are the badge (04 §2.4), read off the effective rows.
                let from = epochs.last().map_or(0, |e| e.tail_from);
                let at = messages.partition_point(|(seq, _)| *seq < from);
                let tail = &messages[at..];
                let removed = |seq: &i64| entries.get(seq).is_some_and(|e| e.removed);
                let skills = if tail.iter().any(|(seq, _)| removed(seq)) {
                    let kept: Vec<_> = tail.iter().filter(|(s, _)| !removed(s)).cloned().collect();
                    skills::in_frame(&kept)
                } else {
                    skills::in_frame(tail)
                }
                .into_iter()
                .collect();
                // A boundary sits right after the last entry completed at or
                // before `at_seq`, i.e. before the first entry past it.
                let mut dividers = epochs.iter().peekable();
                let mut out = Vec::new();
                for (seq, entry) in render(&messages, &entries) {
                    while let Some(e) = dividers.next_if(|e| e.at_seq < seq) {
                        out.push((e.at_seq, compacted(e, heads.get(&e.n).cloned())));
                    }
                    out.push((seq, entry));
                }
                out.extend(dividers.map(|e| (e.at_seq, compacted(e, heads.get(&e.n).cloned()))));
                // Durable failures sit behind their anchor's whole seq group
                // (tool siblings, a divider), so paging keeps them with it.
                match store.errors(m.session).await {
                    Ok(errors) if !errors.is_empty() => {
                        out.extend(errors.into_iter().map(|(at, text)| {
                            (
                                at,
                                HistoryEntry::Error {
                                    text,
                                    context: None,
                                },
                            )
                        }));
                        // Unfinished calls trail the transcript, regardless of
                        // their older call seq (they have no completion seq).
                        out.sort_by_key(|(seq, entry)| {
                            (
                                matches!(entry, HistoryEntry::Tool { result: None, .. }),
                                *seq,
                            )
                        });
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(session = %m.session, "errors: {e:#}"),
                }
                // Typed provenance for replay (50 §9): the exchange rows this
                // session consumed, keyed by the history row they became.
                match store.consumed_exchanges(m.session).await {
                    Ok(rows) => {
                        let peers: HashMap<i64, PeerOrigin> = rows
                            .into_iter()
                            .map(|(seq, r)| (seq, origin_of(&r)))
                            .collect();
                        for (seq, entry) in &mut out {
                            if let HistoryEntry::User { peer, .. } = entry
                                && let Some(o) = peers.get(seq)
                            {
                                *peer = Some(o.clone());
                            }
                        }
                    }
                    Err(e) => tracing::error!(session = %m.session, "consumed_exchanges: {e:#}"),
                }
                let (mut history, first_seq, more) = page(out, m.before, m.limit);
                match store.inner_calls(m.session).await {
                    Ok(mut inner) if !inner.is_empty() => {
                        attach_inner(&mut history, &mut inner, running);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(session = %m.session, "inner_calls: {e:#}"),
                }
                // Previews land under the tree's root (51 §4.5): the data
                // dir for durable, the scratch dir for private.
                let root = self.resources_for(m.session).root.clone();
                materialize(&mut history, &messages, &epochs, &root);
                let (editable, reason, settled) = match (&refused, native.is_some(), external) {
                    (Some(busy), _, _) => (false, Some(busy.clone()), false),
                    (None, _, true) => (false, Some(NOT_NATIVE.to_owned()), !running),
                    // The actor filled these from its own idle state.
                    (None, true, false) => (snapshot.editable, snapshot.reason, snapshot.editable),
                    // No actor: nothing runs; a removal resumes one.
                    (None, false, false) => (true, None, true),
                };
                let context = (
                    revision,
                    editable,
                    reason,
                    (!external).then_some(snapshot.estimated_tokens),
                    settled,
                );
                (history, first_seq, more, skills, context)
            }
            Err(e) => {
                tracing::error!(session = %m.session, "load session: {e:#}");
                let reason = Some(format!("load session: {e:#}"));
                (
                    Vec::new(),
                    None,
                    false,
                    Vec::new(),
                    (String::new(), false, reason, None, false),
                )
            }
        };
        let undone = store
            .hidden_turns(m.session)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(session = %m.session, "hidden_turns: {e:#}");
                0
            });
        let (model, effort, agent, harness_model, name, parent, project) = row
            .map(|r| {
                (
                    r.model,
                    r.effort,
                    r.agent,
                    r.harness_model,
                    r.name,
                    r.parent,
                    r.project,
                )
            })
            .unwrap_or_default();
        let (context_revision, context_editable, context_reason, context_estimate, settled) =
            context;
        // 47 §5.1: the committed plan, independent of paging and of a running
        // turn; a failed read is `None` (unavailable), never an empty plan.
        let todos = store
            .todos(m.session)
            .await
            .map_err(|e| tracing::error!(session = %m.session, "todos: {e:#}"))
            .ok();
        self.mirror(m.session).await;
        let communication = self.communication.0.read().state(Some(m.session)).ok();
        Loaded {
            history,
            running,
            model,
            effort,
            queued,
            undone,
            agent,
            harness_model,
            name,
            parent,
            skills,
            first_seq,
            more,
            context_revision,
            context_editable,
            context_reason,
            context_estimate,
            settled,
            todos,
            communication,
            project,
            retention: self.retention_of(m.session),
        }
    }
}

/// [`LoadSession`] paging over seq-tagged entries: keep `seq < before`, then
/// the last `limit`, widened backwards so a group of entries sharing one seq
/// (tool siblings from one response row, a divider on its message) is never
/// split — `before = first_seq` would otherwise skip its remainder (48 §5).
/// Returns `(history, first_seq, more)`.
fn page(
    mut entries: Vec<(i64, HistoryEntry)>,
    before: Option<i64>,
    limit: Option<u32>,
) -> (Vec<HistoryEntry>, Option<i64>, bool) {
    if let Some(before) = before {
        // Calls still running come last with their call's seq (see `render`);
        // they belong to the live tail, never to an older page.
        entries.retain(|(seq, e)| {
            *seq < before && !matches!(e, HistoryEntry::Tool { result: None, .. })
        });
    }
    let keep = limit.map_or(entries.len(), |l| l as usize);
    let mut cut = entries.len().saturating_sub(keep);
    while 0 < cut && cut < entries.len() && entries[cut - 1].0 == entries[cut].0 {
        cut -= 1;
    }
    let more = cut > 0;
    entries.drain(..cut);
    let first_seq = entries.first().map(|(seq, _)| *seq);
    (
        entries.into_iter().map(|(_, e)| e).collect(),
        first_seq,
        more,
    )
}

/// Result of an inner call the cell never awaited, once the turn is over.
const NOT_RUN: &str = "ERROR: not run: the cell ended before this call was awaited";

/// Nested cards (08 as-built): right after each `python` card, its stored
/// inner calls in `n` order as `Tool` entries with ids `<parent>.<n>`. Runs
/// on the paged history, so `before`/`limit` count stored rows only and a
/// parent is never split from its children; rows whose parent is hidden,
/// stripped or on another page stay unattached. A call without a result is
/// `result: None` only while its cell is still running (a live card the
/// `ToolCallFinished` will complete); otherwise it is the [`NOT_RUN`] error.
fn attach_inner(
    history: &mut Vec<HistoryEntry>,
    inner: &mut HashMap<String, Vec<store::InnerCall>>,
    running: bool,
) {
    let mut i = 0;
    while i < history.len() {
        i += 1;
        let HistoryEntry::Tool {
            call_id,
            name,
            result,
            ..
        } = &history[i - 1]
        else {
            continue;
        };
        if name != repl::NAME {
            continue;
        }
        let Some(children) = inner.remove(call_id.as_str()) else {
            continue;
        };
        let live = running && result.is_none();
        let parent = call_id.clone();
        let n = children.len();
        history.splice(
            i..i,
            children.into_iter().map(|c| HistoryEntry::Tool {
                call_id: format!("{parent}.{}", c.n),
                name: c.name,
                args: c.args,
                result: c
                    .result
                    .or_else(|| (!live).then(|| (NOT_RUN.to_owned(), true))),
                files: c.files,
                context: None,
            }),
        );
        i += n;
    }
}

/// Preview files for the binary occurrences on one page (48 §3.3): stored
/// base64 bytes become `<data_dir>/attachments/<sha256[..16]>.<ext>` via
/// [`save_blob`] (idempotent), served by `get_attachment`. Lazy per page,
/// never a sweep; URL sources are not fetched and a part whose MIME has no
/// extension here stays path-less — the chip is still named and removable.
/// Coordinates in the metadata are untouched: they address the stored rows.
fn materialize(
    history: &mut [HistoryEntry],
    messages: &[(i64, ChatMessage)],
    epochs: &[Epoch],
    data_dir: &Path,
) {
    for entry in history {
        let Some(ctx) = entry.context_mut() else {
            continue;
        };
        for a in ctx.attachments.iter_mut().filter(|a| a.path.is_none()) {
            let Some(b) = binary_at(&a.target, messages, epochs) else {
                continue;
            };
            let BinarySource::Base64(b64) = &b.source else {
                continue;
            };
            let Some(ext) = ext_of(&b.content_type) else {
                continue;
            };
            let bytes = match base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                b64.as_bytes(),
            ) {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(name = %a.name, "attachment preview: bad base64: {e}");
                    continue;
                }
            };
            match save_blob(&bytes, ext, data_dir) {
                Ok(p) => a.path = Some(p),
                Err(e) => tracing::warn!(name = %a.name, "attachment preview: {e}"),
            }
        }
    }
}

/// The stored binary a [`ContextTarget::Binary`] addresses: original part
/// index, then the original `ToolResponse.images` index when `image` is set.
fn binary_at<'a>(
    target: &ContextTarget,
    messages: &'a [(i64, ChatMessage)],
    epochs: &'a [Epoch],
) -> Option<&'a genai::chat::Binary> {
    let ContextTarget::Binary { owner, part, image } = target else {
        return None;
    };
    let content = match owner {
        ContextOwner::Message { seq } => {
            let i = messages.binary_search_by_key(seq, |(s, _)| *s).ok()?;
            &messages[i].1.content
        }
        ContextOwner::Head { epoch } => &epochs.iter().find(|e| e.n == *epoch)?.head.content,
    };
    match (content.parts().get(*part as usize)?, image) {
        (ContentPart::Binary(b), None) => Some(b),
        (ContentPart::ToolResponse(r), Some(i)) => r.images.get(*i as usize),
        _ => None,
    }
}

/// File extension for a stored MIME we preview (17 §1, 20 §2.5); the same
/// set `put_attachment` accepts for images and documents.
fn ext_of(mime: &str) -> Option<&'static str> {
    Some(match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        _ => return None,
    })
}

impl Agents {
    /// The stored row for `session`; `Err("unknown session")` — nothing is
    /// created and no harness is launched (48 §0.7).
    async fn stored(&self, session: SessionId) -> Result<SessionSummary, String> {
        self.store_for(session)
            .session(session)
            .await
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| "unknown session".to_owned())
    }
}

impl Message<ContextItems> for Agents {
    type Reply = Result<ContextSnapshot, String>;

    async fn handle(&mut self, m: ContextItems, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let row = self.stored(m.session).await?;
        let live = self.live(m.session).and_then(|a| a.native().ok());
        match (row.agent.is_some(), live) {
            // The theuth loop owns admission: busy shows as `editable: false` with its reason.
            (false, Some(a)) => a.ask(ContextItemsNow).await.map_err(|e| e.to_string()),
            (external, None) | (external @ true, Some(_)) => {
                let mut s = self
                    .store_for(m.session)
                    .context_items(m.session)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                // An external harness owns its real context (48 §0.7); a
                // native session with no actor is idle, and a removal resumes one.
                s.editable = !external;
                s.reason = external.then(|| NOT_NATIVE.to_owned());
                Ok(s)
            }
        }
    }
}

impl Message<RemoveContext> for Agents {
    type Reply = Result<ContextRemoval, String>;

    async fn handle(
        &mut self,
        m: RemoveContext,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if self.stored(m.session).await?.agent.is_some() {
            return Err(NOT_NATIVE.into());
        }
        // A context edit changes what the model sees: candidates before it are stale.
        self.ledger.bump(m.session);
        // The row exists and is native, so this resumes (never creates) the loop actor.
        self.actor(m.session)
            .await?
            .native()?
            .ask(RemoveContextNow {
                expected_revision: m.expected_revision,
                targets: m.targets,
            })
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<RenameSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RenameSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let title: String = m.title.trim().chars().take(200).collect();
        if title.is_empty() {
            return Err("empty title".into());
        }
        let store = self.store_for(m.session);
        store
            .set_title(m.session, &title)
            .await
            .map_err(|e| e.to_string())?;
        // A rename changes the title only; the event repeats the stored tags.
        let tags = store
            .session(m.session)
            .await
            .ok()
            .flatten()
            .map(|s| s.tags)
            .unwrap_or_default();
        // Before the reply: the caller's next search must not be served the
        // old title. The `TitleChanged` bump below arrives through the
        // mailbox after we have answered (and is still needed for titles the
        // loop generates); a second bump only costs a rebuild.
        self.search.invalidate();
        // A fresh session's first turn would otherwise overwrite this with the
        // input placeholder (`Session::start_turn`).
        if let Some(Live {
            actor: LiveActor::Native(a),
            ..
        }) = self.sessions.get(&m.session)
        {
            let _ = a.tell(crate::loop_::Titled).await;
        }
        self.rt
            .publish(AgentEvent::TitleChanged {
                session: m.session,
                title,
                tags,
            })
            .await;
        Ok(())
    }
}

impl Message<PinSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: PinSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.store_for(m.session)
            .set_pinned(m.session, m.pinned)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<MarkDesk> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: MarkDesk,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.store_for(m.session)
            .set_desk(m.session, m.agent.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<ArchiveSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ArchiveSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.store_for(m.session)
            .set_archived(m.session, m.archived)
            .await
            .map_err(|e| e.to_string())?;
        // Session hits carry the flag (49 §7): the next search rebuilds.
        self.search.invalidate();
        Ok(())
    }
}

impl Message<DeleteSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: DeleteSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        // A private member never takes the durable path (51 §4.4): the
        // whole tree closes, running turns included.
        if let Some(&root) = self.private_member.get(&m.session) {
            self.close_private(root, ctx.actor_ref().clone()).await;
            return Ok(());
        }
        if self.closing.contains(&m.session) {
            return Err("session closed".into());
        }
        let tree = self
            .store
            .descendants(m.session)
            .await
            .map_err(|e| e.to_string())?;
        let ids: Vec<SessionId> = std::iter::once(m.session)
            .chain(tree.into_iter().map(|(_, s)| s.id))
            .collect();
        if ids.iter().any(|id| self.running.contains(id)) {
            return Err("a turn is running; stop it first".into());
        }
        // Peers first (50 §7): exports and routes revoked, waits resolved,
        // accepted intake suppressed; then the rows (which drop the
        // exchanges/tasks) and the actors.
        self.mirror(m.session).await;
        self.revoking(|p| p.forget(m.session)).await;
        self.revoke(Some(m.session)).await;
        self.expire_waits("session deleted", |w| {
            ids.contains(&w.session) || PeerAddress::local(&w.to).is_some_and(|t| ids.contains(&t))
        });
        for id in &ids {
            self.ledger.forget(*id);
            self.bound.retain(|(s, _), _| s != id);
            self.delivered.retain(|(s, _), _| s != id);
            self.turn_text.retain(|(s, _), _| s != id);
            self.owned_children.remove(id);
            self.caps.remove(id);
            self.remote_parent.remove(id);
            if let Some(live) = self.sessions.remove(id) {
                live.actor.kill().await;
            }
        }
        self.communication_changed(Some(m.session)).await;
        let removed = self
            .store
            .delete_session(m.session)
            .await
            .map_err(|e| e.to_string())?;
        // Before the reply, as `RenameSession`: no search after a completed
        // delete may still list the rows.
        self.search.invalidate();
        for session in removed {
            self.rt
                .publish(AgentEvent::SessionDeleted { session })
                .await;
        }
        Ok(())
    }
}

impl Message<SearchSessionPage> for Agents {
    type Reply =
        kameo::reply::DelegatedReply<Result<::search::SearchPage<search::SearchHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchSessionPage,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // Off the mailbox: a cold cache answers `indexing` and builds in its
        // own task; matching runs on a blocking thread.
        let (cache, store) = (self.search.clone(), self.store.clone());
        tokio::spawn(async move {
            tx.send(cache.page(&store, m.query, m.limit).await);
        });
        reply
    }
}

impl Message<SearchRefresh> for Agents {
    type Reply = ();

    async fn handle(&mut self, _: SearchRefresh, _: &mut Context<Self, Self::Reply>) {
        self.search.invalidate();
    }
}

impl Message<ListAwsProfiles> for Agents {
    type Reply = Result<AwsProfiles, String>;

    async fn handle(
        &mut self,
        _: ListAwsProfiles,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        AwsProfiles::load(self.auth.aws_profile()).await
    }
}

impl Message<SetAwsProfile> for Agents {
    type Reply = Result<AwsProfiles, String>;

    async fn handle(
        &mut self,
        m: SetAwsProfile,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut profiles = AwsProfiles::load(self.auth.aws_profile()).await?;
        profiles.select(m.profile)?;
        // JSON strings are YAML scalars too: profile names must never become config syntax.
        let value = serde_json::to_string(&profiles.selected).map_err(|e| e.to_string())?;
        runtime::write_config_value(
            &self.rt.data_dir.join("config.yaml"),
            "agent",
            "aws_profile",
            &value,
        )
        .map_err(|e| format!("config.yaml: {e}"))?;
        *self.auth.aws_profile.write() = profiles.selected.clone();
        Ok(profiles)
    }
}

impl Message<ListModels> for Agents {
    type Reply = Vec<ModelInfo>;

    async fn handle(
        &mut self,
        _: ListModels,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ModelInfo> {
        let mut out: Vec<ModelInfo> = self
            .catalog
            .providers()
            .filter(|p| (self.configured)(Catalog::adapter(&p.id)))
            .flat_map(|p| p.models.values().map(|m| ModelInfo::new(&p.id, m)))
            .collect();
        // Cursor is not in models.dev: ask the backend what the account may use.
        if self.auth.login("cursor") {
            match self.auth.oauth.access(&oauth::CURSOR).await {
                Ok(t) => match crate::backend::cursor_models(&t.access_token).await {
                    Ok(models) => out.extend(models.into_iter().map(|m| {
                        let id = format!("cursor/{}", m.id);
                        let near = self.catalog.closest(&id);
                        ModelInfo {
                            context: near.map_or(0, |n| n.limit.context),
                            reasoning: near.is_some_and(|n| n.reasoning),
                            // Cursor's backend is text-only whatever the sibling says
                            // (`backend::supports_images`).
                            image: false,
                            efforts: near.map(|n| n.efforts()).unwrap_or_default(),
                            budget_min: near.and_then(|n| n.budget()).map(|b| b.0),
                            budget_max: near.and_then(|n| n.budget()).and_then(|b| b.1),
                            id,
                            name: m.name,
                            provider: "cursor".into(),
                        }
                    })),
                    Err(e) => tracing::warn!("cursor models: {e}"),
                },
                Err(e) => tracing::warn!("{e}"),
            }
        }
        out.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        out
    }
}

impl Message<SetModel> for Agents {
    type Reply = Result<ModelInfo, String>;

    async fn handle(
        &mut self,
        m: SetModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ModelInfo, String> {
        let resolved = self.catalog.resolve(&m.model, &*self.configured)?;
        if let Some(id) = m.session {
            let Some(row) = self.store_for(id).session(id).await.map_err(|e| e.to_string())? else {
                return Err(format!("unknown session {id}"));
            };
            let actor = self.actor(id).await?.native()?.clone();
            let _ = actor.tell(SwitchModel(resolved.clone())).await;
            if let Err(e) = self.store_for(id).set_model(id, &resolved.id).await {
                tracing::warn!(session = %id, "set_model: {e:#}");
            }
            self.rt
                .publish(AgentEvent::ModelChanged {
                    session: id,
                    model: resolved.id.clone(),
                })
                .await;
            // An effort the new model does not take would 400 its next call: drop it.
            if let Some(e) = &row.effort
                && resolved
                    .model
                    .as_ref()
                    .is_some_and(|m| m.validate_effort(e).is_err())
            {
                self.apply_effort(id, &actor, None).await;
            }
        }
        self.default_model = resolved.id.clone();
        // ponytail: writes <data_dir>/config.yaml; a `--config <other>` file is not written back.
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "agent", "model", &resolved.id) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(ModelInfo::from(&resolved))
    }
}

impl Agents {
    /// Switch `id`'s effort from its next turn, persist it, and announce it.
    async fn apply_effort(&self, id: SessionId, actor: &ActorRef<Session>, effort: Option<String>) {
        let _ = actor
            .tell(SetPendingEffort(effort.as_deref().and_then(effort_of)))
            .await;
        if let Err(e) = self.store_for(id).set_effort(id, effort.as_deref()).await {
            tracing::warn!(session = %id, "set_effort: {e:#}");
        }
        self.rt
            .publish(AgentEvent::EffortChanged {
                session: id,
                effort,
            })
            .await;
    }
}

impl Message<SetEffort> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let target = match m.session {
            Some(id) => {
                self.store_for(id)
                    .session(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown session {id}"))?
                    .model
            }
            None => self.default_model.clone(),
        };
        if let Some(e) = &m.effort {
            let known = self.catalog.resolve(&target, &*self.configured).ok();
            match known.and_then(|r| r.model) {
                Some(model) => model.validate_effort(e)?,
                None if effort_of(e).is_none() => return Err(format!("unknown effort {e}")),
                None => {}
            }
        }
        if let Some(id) = m.session {
            let actor = self.actor(id).await?.native()?.clone();
            self.apply_effort(id, &actor, m.effort.clone()).await;
        }
        self.default_effort = m.effort.clone();
        // Quoted so a bare budget (`"8000"`) stays a YAML string; `~` clears.
        let value = m
            .effort
            .as_ref()
            .map_or_else(|| "~".to_owned(), |e| format!("\"{e}\""));
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "agent", "effort", &value) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<ListLogins> for Agents {
    type Reply = Vec<LoginInfo>;

    async fn handle(
        &mut self,
        _: ListLogins,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<LoginInfo> {
        oauth::PROVIDERS
            .iter()
            .map(|p| {
                let t = self.auth.oauth.load(&p.ns);
                LoginInfo {
                    name: p.ns.to_string(),
                    logged_in: t.is_some(),
                    expires_at: t.as_ref().map(|t| t.expires_at),
                    account: t.and_then(|t| t.account_id),
                }
            })
            .collect()
    }
}

impl Message<Login> for Agents {
    type Reply = Result<String, String>;

    async fn handle(&mut self, m: Login, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = login_provider(&m.provider)?;
        let (url, finish) = self.auth.oauth.login(p).await?;
        oauth::open_browser(&url);
        let rt = self.rt.clone();
        tokio::spawn(async move {
            let error = finish.await.err();
            match &error {
                Some(e) => tracing::warn!("login {}: {e}", m.provider),
                None => tracing::info!("login {}: ok", m.provider),
            }
            rt.publish(LoginFinished {
                provider: m.provider,
                error,
            })
            .await;
        });
        Ok(url)
    }
}

impl Message<Logout> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Logout, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let p = login_provider(&m.provider)?;
        self.auth.oauth.remove(&p.ns).await
    }
}

impl Message<ListProviders> for Agents {
    type Reply = Vec<ProviderInfo>;

    async fn handle(
        &mut self,
        _: ListProviders,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ProviderInfo> {
        let gws = self.auth.gateways.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ProviderInfo> = gws
            .iter()
            .map(|(name, g)| ProviderInfo {
                models: self
                    .catalog
                    .providers()
                    .find(|p| p.id == *name)
                    .map_or(0, |p| p.models.len() as u32),
                name: name.clone(),
                base_url: g.base_url.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

impl Message<AddProvider> for Agents {
    type Reply = Result<ProviderInfo, String>;

    async fn handle(
        &mut self,
        m: AddProvider,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ProviderInfo, String> {
        crate::providers::validate_name(&m.name)?;
        let base_url = crate::providers::normalize_url(&m.base_url)?;
        let key = m.key.trim();
        if key.is_empty() {
            return Err("api key is empty".into());
        }
        let models = match self
            .client
            .all_model_names(
                AdapterKind::OpenAI,
                (
                    Endpoint::from_owned(base_url.clone()),
                    AuthData::Key(key.to_owned()),
                ),
            )
            .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(provider = %m.name, "listing models: {e}");
                Vec::new()
            }
        };
        let secrets = crate::providers::secrets_actor()?;
        crate::providers::save(
            &self.rt.data_dir,
            &secrets,
            &m.name,
            &base_url,
            key,
            &models,
        )
        .await?;
        self.auth
            .gateways
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                m.name.clone(),
                crate::providers::Gateway {
                    base_url: base_url.clone(),
                    key: key.to_owned(),
                },
            );
        self.catalog = Arc::new(Catalog::load(&self.rt.data_dir));
        Ok(ProviderInfo {
            name: m.name,
            base_url,
            models: models.len() as u32,
        })
    }
}

impl Message<RemoveProvider> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveProvider,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let secrets = crate::providers::secrets_actor()?;
        crate::providers::remove(&self.rt.data_dir, &secrets, &m.name).await?;
        self.auth
            .gateways
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&m.name);
        self.catalog = Arc::new(Catalog::load(&self.rt.data_dir));
        Ok(())
    }
}

impl Message<NewSession> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: NewSession, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (id, _) = self
            .session(None, None, None, (None, None))
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some(cwd) = &m.cwd {
            self.store
                .set_cwd(id, cwd)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        if m.unattended {
            self.store
                .set_unattended(id)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(id)
    }
}

/// One `/acp` row, unprobed.
fn acp_info(name: String, e: &acp::Entry) -> acp::EntryInfo {
    acp::EntryInfo {
        name,
        command: e.command.clone(),
        args: e.args.clone(),
        description: e.description.clone(),
        status: None,
    }
}

impl Message<ListAcp> for Agents {
    type Reply = kameo::reply::DelegatedReply<Vec<acp::EntryInfo>>;

    async fn handle(&mut self, m: ListAcp, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        // Probing spawns every harness (up to 10 s each): off the mailbox.
        tokio::spawn(async move {
            let entries = acp::load(&data_dir).unwrap_or_else(|e| {
                tracing::warn!("acp/agents.yaml: {e:#}");
                Default::default()
            });
            let logs = data_dir.join("logs");
            let logs = &logs;
            let probes = entries.iter().map(|(name, e)| async move {
                let mut info = acp_info(name.clone(), e);
                if m.probe {
                    let r = acp::probe(
                        acp::Transport::Process(e.config()),
                        logs.join(format!("acp-{name}.log")),
                    )
                    .await;
                    info.status = Some(r.status);
                }
                info
            });
            tx.send(futures::future::join_all(probes).await);
        });
        reply
    }
}

impl Message<AddAcp> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<acp::EntryInfo, String>>;

    async fn handle(&mut self, m: AddAcp, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        // Network + download: off the mailbox.
        tokio::spawn(async move {
            let result = async {
                let agents = acp::registry::fetch(&data_dir)
                    .await
                    .map_err(|e| format!("registry: {e:#}"))?;
                let agent = agents
                    .iter()
                    .find(|a| a.id == m.id)
                    .ok_or_else(|| format!("unknown registry id `{}`", m.id))?;
                let entry = acp::registry::install(&data_dir, agent)
                    .await
                    .map_err(|e| format!("install {}: {e:#}", m.id))?;
                let mut entries = acp::load(&data_dir).map_err(|e| format!("{e:#}"))?;
                entries.insert(m.id.clone(), entry.clone());
                acp::save(&data_dir, &entries).map_err(|e| format!("{e:#}"))?;
                Ok(acp_info(m.id, &entry))
            }
            .await;
            tx.send(result);
        });
        reply
    }
}

impl Message<RemoveAcp> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RemoveAcp, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let data_dir = &self.rt.data_dir;
        let mut entries = acp::load(data_dir).map_err(|e| format!("{e:#}"))?;
        if entries.remove(&m.name).is_none() {
            return Err(acp::unknown(&m.name, &entries));
        }
        acp::save(data_dir, &entries).map_err(|e| format!("{e:#}"))?;
        acp::registry::remove_bin(data_dir, &m.name).map_err(|e| format!("bin/{}: {e}", m.name))
    }
}

impl Message<RegisterNode> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: RegisterNode, _: &mut Context<Self, Self::Reply>) {
        self.nodes
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(m.name, (m.describe, m.dialer));
    }
}

impl Message<UnregisterNode> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterNode, _: &mut Context<Self, Self::Reply>) {
        self.nodes
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&m.name);
    }
}

impl Message<ListRegistry> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<Vec<acp::registry::RegistryAgent>, String>>;

    async fn handle(
        &mut self,
        _: ListRegistry,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let data_dir = self.rt.data_dir.clone();
        tokio::spawn(async move {
            tx.send(
                acp::registry::fetch(&data_dir)
                    .await
                    .map_err(|e| format!("registry: {e:#}")),
            );
        });
        reply
    }
}

/// [`oauth::provider`] with a user-facing error listing the choices.
fn login_provider(ns: &str) -> Result<&'static oauth::Provider, String> {
    oauth::provider(ns).ok_or_else(|| {
        let all: Vec<&str> = oauth::PROVIDERS.iter().map(|p| &*p.ns).collect();
        format!("unknown provider `{ns}`; providers: {}", all.join(", "))
    })
}

impl ModelInfo {
    fn new(provider: &str, m: &models::Model) -> Self {
        let budget = m.budget();
        Self {
            id: format!("{provider}/{}", m.id),
            name: m.name.clone(),
            provider: provider.to_owned(),
            context: m.limit.context,
            reasoning: m.reasoning,
            // Effective input support (52 §3.2), the `backend::supports_images` rule.
            image: m.image && Catalog::adapter(provider) != "cursor",
            efforts: m.efforts(),
            budget_min: budget.map(|b| b.0),
            budget_max: budget.and_then(|b| b.1),
        }
    }
}

impl From<&Resolved> for ModelInfo {
    fn from(r: &Resolved) -> Self {
        // `r.model` may be a name-matched neighbour: keep the id the user chose.
        let budget = r.model.as_ref().and_then(|m| m.budget());
        Self {
            id: r.id.clone(),
            name: r
                .model
                .as_ref()
                .map_or_else(|| r.id.clone(), |m| m.name.clone()),
            provider: r.provider.clone().unwrap_or_default(),
            context: r.model.as_ref().map_or(0, |m| m.limit.context),
            reasoning: r.model.as_ref().is_some_and(|m| m.reasoning),
            image: crate::backend::supports_images(r),
            efforts: r.model.as_ref().map(|m| m.efforts()).unwrap_or_default(),
            budget_min: budget.map(|b| b.0),
            budget_max: budget.and_then(|b| b.1),
        }
    }
}

/// Collapse stored messages into UI entries tagged with the seq of the
/// message that completed them; a tool call and its response become one
/// `Tool` entry (tagged with the response's seq). Calls without a response
/// yet — the round running when the session was loaded — come last, in
/// call order, with `result: None`. `User`/`Assistant` entries carry the seq
/// of the turn opener they belong to (design doc 29 §4.3). `entries` is the
/// per-row context metadata (48 §3.3): a text entry gets its own row's, a
/// tool card its response row's, narrowed to that response's images; a
/// call still running has no stored response and so no target.
pub(crate) fn render(
    messages: &[(i64, ChatMessage)],
    entries: &BTreeMap<i64, ContextEntry>,
) -> Vec<(i64, HistoryEntry)> {
    let mut out = Vec::new();
    // ponytail: linear scan; one round's calls, never more than a handful.
    let mut pending: Vec<(i64, &genai::chat::ToolCall)> = Vec::new();
    let mut current_turn: Option<i64> = None;
    // 52 §6: a typed vision note neither opens a turn nor hides the next
    // real user message's handle; the store decides from the metadata.
    let openers = store::turn_openers(messages, |s| {
        entries.get(&s).is_some_and(|e| e.vision.is_some())
    });
    for (seq, m) in messages {
        let opens = openers.contains(seq);
        if opens {
            current_turn = Some(*seq);
        }
        let context = entries.get(seq);
        match m.role {
            ChatRole::User => out.push((
                *seq,
                HistoryEntry::User {
                    text: text_of(&m.content),
                    turn: opens.then_some(*seq),
                    context: context.cloned(),
                    peer: None,
                    // 52 §6: the consultation card renders from the typed metadata.
                    vision: context.and_then(|c| c.vision.clone()),
                },
            )),
            ChatRole::Assistant => {
                let text = text_of(&m.content);
                if !text.is_empty() {
                    out.push((
                        *seq,
                        HistoryEntry::Assistant {
                            text,
                            turn: current_turn,
                            context: context.cloned(),
                        },
                    ));
                }
                pending.extend(m.content.tool_calls().into_iter().map(|c| (*seq, c)));
            }
            ChatRole::Tool => {
                let responses = m
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.as_tool_response().map(|r| (i, r)));
                for (part, r) in responses {
                    let call = pending
                        .iter()
                        .position(|(_, c)| c.call_id == r.call_id)
                        .map(|i| pending.remove(i).1);
                    let (name, args) = call.map_or(("?", &serde_json::Value::Null), |c| {
                        (c.fn_name.as_str(), &c.fn_arguments)
                    });
                    // 23 §5: same `[image: …]` lines the live `ToolCallFinished` showed;
                    // a part named by full path is a file the tool produced.
                    let mut text = r.content.clone();
                    let mut files = Vec::new();
                    for b in &r.images {
                        text.push('\n');
                        crate::loop_::binary_tag(&mut text, b);
                        if let Some(p) =
                            b.name.as_deref().map(Path::new).filter(|p| p.is_absolute())
                        {
                            files.push(p.to_path_buf());
                        }
                    }
                    let is_error = text.starts_with("ERROR: ");
                    // Same row target for every sibling card; only this response's images.
                    let context = context.map(|c| ContextEntry {
                        attachments: c
                            .attachments
                            .iter()
                            .filter(|a| {
                                matches!(a.target, ContextTarget::Binary { part: p, .. } if p as usize == part)
                            })
                            .cloned()
                            .collect(),
                        ..c.clone()
                    });
                    out.push((
                        *seq,
                        HistoryEntry::Tool {
                            call_id: r.call_id.clone(),
                            name: name.to_string(),
                            args: args.clone(),
                            result: Some((text, is_error)),
                            files,
                            context,
                        },
                    ));
                }
            }
            ChatRole::System => {}
        }
    }
    out.extend(pending.into_iter().map(|(seq, c)| {
        (
            seq,
            HistoryEntry::Tool {
                call_id: c.call_id.clone(),
                name: c.fn_name.clone(),
                args: c.fn_arguments.clone(),
                result: None,
                files: Vec::new(),
                context: None,
            },
        )
    }));
    out
}

/// The UI divider for one epoch: the head text without its
/// `<context_summary …>` wrapper (design doc 07 §8.4), carrying the head's
/// context membership (`ContextTarget::Head`, snap images as attachments).
fn compacted(e: &Epoch, context: Option<ContextEntry>) -> HistoryEntry {
    let text = text_of(&e.head.content);
    let style = crate::compact::style_of_head(&text);
    let summary = text
        .strip_prefix("<context_summary")
        .and_then(|s| s.split_once('>'))
        .and_then(|(_, s)| s.strip_suffix("</context_summary>"))
        .map_or(text.as_str(), str::trim)
        .to_owned();
    HistoryEntry::Compacted {
        epoch: e.n,
        summary,
        style,
        context,
    }
}

/// Key sources in precedence order (design doc 05 §2.2, 37 §0.7): the secrets
/// vault, then a `/login`, then the adapter's env var (left to genai).
#[derive(Clone)]
pub(crate) struct Auth {
    /// The vault (37 §3.7); `None` when it failed to open — keys then come
    /// from the environment and `/login` only.
    pub(crate) vault: Option<Arc<secrets::Vault>>,
    pub(crate) oauth: oauth::Store,
    /// Named OpenAI-compatible gateways; interior-mutable so `/provider add` is live.
    pub(crate) gateways: Arc<RwLock<HashMap<String, crate::providers::Gateway>>>,
    /// Shared by every inference client; never mutates the process's AWS environment.
    aws_profile: Arc<parking_lot::RwLock<Option<String>>>,
}

impl Auth {
    /// No vault, no logins, no gateways; `dir` holds the (empty) OAuth store.
    #[cfg(test)]
    pub(crate) fn stub(dir: &Path) -> Self {
        Self {
            vault: None,
            oauth: oauth::Store::new(dir),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            aws_profile: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    fn aws_profile(&self) -> Option<String> {
        self.aws_profile.read().clone()
    }

    /// The current vault value for genai namespace `ns`, read live.
    pub(crate) fn key(&self, ns: &str) -> Option<String> {
        self.vault.as_ref()?.get(ns, 0).map(|k| k.to_string())
    }

    /// A login is in force for genai namespace `ns`: no key in the vault and
    /// tokens on disk.
    pub(crate) fn login(&self, ns: &str) -> bool {
        self.key(ns).is_none() && oauth::provider(ns).is_some() && self.oauth.load(ns).is_some()
    }

    /// OpenAI-compatible gateway for user-facing provider id `ns`.
    pub(crate) fn gateway(&self, ns: &str) -> Option<crate::providers::Gateway> {
        self.gateways.read().ok()?.get(ns).cloned()
    }
}

/// Largest image a provider accepts (17 §1: Anthropic's 5 MB is the binding cap).
const MAX_ATTACHMENT: u64 = 5 * 1024 * 1024;
/// Largest PDF sent as a native document part (20 §2.5: Anthropic's 32 MB;
/// its 100-page cap is left to the provider's error).
const MAX_PDF: u64 = 32 * 1024 * 1024;

/// Rejects what a provider would (17 §3.1, 20 §2.5): missing/non-regular
/// file, a mime that is neither `image/*` nor `application/pdf`, over the cap.
fn check_files(files: &[PathBuf]) -> Result<(), String> {
    for f in files {
        let name = f.file_name().unwrap_or(f.as_os_str()).to_string_lossy();
        let meta = match std::fs::metadata(f) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Err(format!("attachment {name}: not a regular file")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("attachment {name}: not found"));
            }
            Err(e) => return Err(format!("attachment {name}: {e}")),
        };
        let mime = mime_guess::from_path(f).first_or_octet_stream();
        let cap = if mime.type_() == mime_guess::mime::IMAGE {
            MAX_ATTACHMENT
        } else if mime == mime_guess::mime::APPLICATION_PDF {
            MAX_PDF
        } else {
            return Err(format!("attachment {name}: {mime} is not an image or PDF"));
        };
        if meta.len() > cap {
            return Err(format!(
                "attachment {name}: {:.1} MB exceeds {} MB",
                meta.len() as f64 / (1024.0 * 1024.0),
                cap >> 20
            ));
        }
    }
    Ok(())
}

/// genai client whose keys come from the secrets vault (read per request, so
/// a new key is live), falling back to each adapter's env var. A login in
/// force (design doc 05 §2.3) rewrites the target instead: OpenAI → the
/// ChatGPT Codex backend; Anthropic → the same Messages API with the Claude
/// Code bearer and betas.
fn client(auth: Auth) -> anyhow::Result<Client> {
    let keys = auth.clone();
    Ok(Client::builder()
        .with_auth_resolver_fn(move |m: ModelIden| {
            // SigV4's AuthData is a profile, not a vault key. Resolve it per request below.
            if m.adapter_kind == AdapterKind::BedrockSigv4 {
                return Ok(Some(AuthData::None));
            }
            // No vault key: `None` hands genai the adapter's own default
            // (its env var, or anonymous for local servers such as omlx and
            // ollama). Forcing `from_env` here made a keyless local model
            // fail before any request left the process.
            Ok(keys.key(m.adapter_kind.as_lower_str()).map(AuthData::Key))
        })
        .with_service_target_resolver(ServiceTargetResolver::from_resolver_async_fn(
            move |mut target: ServiceTarget| -> Pin<
                Box<dyn Future<Output = genai::resolver::Result<ServiceTarget>> + Send>,
            > {
                let auth = auth.clone();
                Box::pin(async move {
                    match target.model.adapter_kind {
                        AdapterKind::BedrockSigv4 => {
                            target.auth = match auth.aws_profile() {
                                Some(profile) => AuthData::Key(profile),
                                None => match auth.key("bedrock_api").or_else(|| {
                                    ["BEDROCK_API_KEY", "AWS_BEARER_TOKEN_BEDROCK"]
                                        .into_iter()
                                        .find_map(|name| {
                                            std::env::var(name).ok().filter(|v| !v.is_empty())
                                        })
                                }) {
                                    Some(key) => {
                                        target.model.adapter_kind = AdapterKind::BedrockApi;
                                        AuthData::Key(key)
                                    }
                                    None => AuthData::None,
                                },
                            };
                        }
                        AdapterKind::OpenAI | AdapterKind::OpenAIResp if auth.login("openai") => {
                            let t = auth
                                .oauth
                                .access(&oauth::OPENAI)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.model.adapter_kind = AdapterKind::OpenAIResp;
                            target.endpoint =
                                Endpoint::from_static("https://chatgpt.com/backend-api/codex/");
                            target.auth = AuthData::RequestOverride {
                                url: "https://chatgpt.com/backend-api/codex/responses".into(),
                                headers: Headers::from(vec![
                                    ("Authorization", format!("Bearer {}", t.access_token)),
                                    ("chatgpt-account-id", t.account_id.unwrap_or_default()),
                                    ("OpenAI-Beta", "responses=v1".to_owned()),
                                    ("originator", "codex_cli_rs".to_owned()),
                                ]),
                            };
                        }
                        AdapterKind::Anthropic if auth.login("anthropic") => {
                            let t = auth
                                .oauth
                                .access(&oauth::ANTHROPIC)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.auth = AuthData::RequestOverride {
                                url: "https://api.anthropic.com/v1/messages?beta=true".into(),
                                headers: Headers::from(vec![
                                    ("Authorization", format!("Bearer {}", t.access_token)),
                                    ("anthropic-version", "2023-06-01".to_owned()),
                                    ("anthropic-beta", ANTHROPIC_BETAS.to_owned()),
                                    ("User-Agent", CLAUDE_CODE_UA.to_owned()),
                                    ("x-app", "cli".to_owned()),
                                    (
                                        "anthropic-dangerous-direct-browser-access",
                                        "true".to_owned(),
                                    ),
                                ]),
                            };
                        }
                        AdapterKind::Xai if auth.login("xai") => {
                            let t = auth
                                .oauth
                                .access(&oauth::XAI)
                                .await
                                .map_err(genai::resolver::Error::Custom)?;
                            target.auth = AuthData::Key(t.access_token);
                        }
                        _ => {}
                    }
                    Ok(target)
                })
            },
        ))
        .build()?)
}

/// Betas a subscription token is accepted with; `oauth-2025-04-20` is the
/// one that turns a 403 into a 200.
const ANTHROPIC_BETAS: &str =
    "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14";
/// The client the subscription token was minted for.
const CLAUDE_CODE_UA: &str = "claude-cli/2.1.257 (external, cli)";

/// The auth-resolver rule as a predicate over genai namespaces: a key in
/// the vault, a login on disk, the adapter's default env var set, or an
/// adapter needing no key (`ollama`). `cursor` is not a genai adapter: only
/// a login configures it. Bedrock has no key env, so SigV4 counts as
/// configured only when [`bedrock_configured`] finds credentials to use.
pub(crate) fn configured(auth: Auth) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |ns: &str| {
        if ns == "bedrock_sigv4" {
            return bedrock_configured(&auth);
        }
        auth.key(ns).is_some()
            || auth.gateway(ns).is_some()
            || auth.login(ns)
            || AdapterKind::from_lower_str(ns).is_some_and(|k| {
                k.default_key_env_name()
                    .is_none_or(|env| std::env::var_os(env).is_some_and(|v| !v.is_empty()))
            })
    })
}

/// What the Bedrock resolver ([`start`]'s `auth_resolver_fn`) can actually use:
/// a bearer token (vault `bedrock_api`, `BEDROCK_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK`),
/// the `/aws` profile, static or role credentials in the environment, or the
/// SDK's ambient profile present in its files. Never resolves credentials (02 §2.2).
// ponytail: IMDS/ECS metadata without env hints is not detected; add a cached async probe if EC2 users ask.
fn bedrock_configured(auth: &Auth) -> bool {
    let env = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    auth.key("bedrock_api").is_some()
        || env("BEDROCK_API_KEY")
        || env("AWS_BEARER_TOKEN_BEDROCK")
        || auth.aws_profile().is_some()
        || (env("AWS_ACCESS_KEY_ID") && env("AWS_SECRET_ACCESS_KEY"))
        || env("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
        || env("AWS_CONTAINER_CREDENTIALS_FULL_URI")
        || env("AWS_WEB_IDENTITY_TOKEN_FILE")
        || crate::aws::ambient_profile_exists()
}

/// The vision fallback's id/capability rule (52 §3.1), shared by
/// [`SetVisionModel`] and [`start`]: `id` must resolve (an ambiguous bare id
/// is the catalog's error) to an **exact** merged catalog entry — no fuzzy
/// sibling, no pass-through — that receives images through its backend
/// ([`crate::backend::supports_images`]: Cursor and text-only entries fail).
/// Whether the provider is configured is the setter's extra check: a saved
/// selection whose credentials disappeared stays selected and fails at use.
fn vision_candidate(
    catalog: &Catalog,
    configured: &dyn Fn(&str) -> bool,
    id: &str,
) -> Result<Resolved, String> {
    let resolved = catalog.resolve(id, configured)?;
    if catalog.get(&resolved.id).is_none() {
        return Err(format!(
            "`{id}` is not an exact catalog entry; a vision fallback must name one as `provider/model` (add it to models.json first)"
        ));
    }
    if !crate::backend::supports_images(&resolved) {
        return Err(format!("`{}` cannot receive images", resolved.id));
    }
    Ok(resolved)
}

/// 57 §0.9: the escalation model must be a catalog entry (text is enough —
/// it answers a rendered question); a unique bare id canonicalises, an unknown
/// or ambiguous one is refused so a typo never becomes a pass-through model.
fn escalate_candidate(
    catalog: &Catalog,
    configured: &dyn Fn(&str) -> bool,
    id: &str,
) -> Result<Resolved, String> {
    let resolved = catalog.resolve(id, configured)?;
    if catalog.get(&resolved.id).is_none() {
        return Err(format!(
            "`{id}` is not a catalog entry; name one as `provider/model` (or add it to models.json first)"
        ));
    }
    Ok(resolved)
}

/// The `agent:` section: absent = defaults; present but malformed (a
/// wrongly typed `vision_model`, say) is an error, not a silent reset of
/// every `agent.*` value to its default (52 §3.1).
fn agent_config(rt: &Runtime) -> anyhow::Result<AgentConfig> {
    if rt.config.contains("agent") {
        rt.section("agent")
            .map_err(|e| e.context("[agent] config section"))
    } else {
        Ok(AgentConfig::default())
    }
}

/// Open the store, spawn and register the supervisor.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let mut cfg = agent_config(rt)?;
    // Design doc 30 §12: its own top-level section, carried on `cfg` so the loop sees it.
    if rt.config.contains("surface") {
        cfg.surface = rt.section("surface")?;
    }
    let cfg = Arc::new(cfg);
    // 68 §2: `sessions.owner` is this engine's mesh id. The mesh plugin
    // starts after us, so read (or create) the identity file directly.
    let engine = match mesh::identity::Identity::load_or_create(&rt.data_dir.join(mesh::DIR)) {
        Ok(i) => Some(i.id()),
        Err(e) => {
            tracing::warn!("mesh identity unavailable; sessions stay owned by '': {e:#}");
            None
        }
    };
    let store = Store::open(&rt.data_dir.join("sessions.sqlite"), engine).await?;
    // 51 §6.4: calls a previous process left open can never settle.
    match store.unresolve_open_calls().await {
        Ok(n) if n > 0 => tracing::info!("accounting: {n} open call(s) marked unresolved"),
        Ok(_) => {}
        Err(e) => tracing::error!("accounting restart: {e:#}"),
    }
    // 68 decision 8: `projects`/`cost_owners` ride the record replica, which
    // starts after us.
    {
        let rt2 = rt.clone();
        let store = store.clone();
        tokio::spawn(async move {
            if rt2.lookup_when::<replica::Replica>(replica::REPLICA_ACTOR).await.is_none() {
                return;
            }
            match store.replicate_records().await {
                Ok(n) => tracing::info!("accounting: projects/cost_owners replicated ({n} row(s) announced)"),
                Err(e) => tracing::info!("accounting: replication unavailable ({e}); local only"),
            }
        });
    }
    // 51 §4.5: scratch dirs of dead processes, proven free by their lock.
    let swept = crate::private::sweep_stale();
    if swept > 0 {
        tracing::info!("private: {swept} stale scratch dir(s) removed");
    }
    // Plugins start in order and `secrets` precedes `agent`, so the actor is
    // registered by now or never (its vault failed to open).
    let vault = match ActorRef::<secrets::SecretsActor>::lookup(secrets::SECRETS_ACTOR) {
        Ok(Some(actor)) => actor.ask(secrets::Handle).await.ok(),
        _ => None,
    };
    if vault.is_none() {
        tracing::error!(
            "secrets vault unavailable; provider keys come from the environment and /login only"
        );
    }
    let gateways = crate::providers::load(&rt.data_dir, vault.as_deref());
    let auth = Auth {
        oauth: oauth::Store::new(&rt.data_dir)
            .with_grants(vault.clone().map(|v| v as Arc<dyn oauth::Grants>)),
        vault,
        gateways: Arc::new(RwLock::new(gateways)),
        aws_profile: Arc::new(parking_lot::RwLock::new(cfg.aws_profile.clone())),
    };
    auth.oauth.migrate_files().await;
    // One worker per concurrent child turn plus the root session (design doc 08 §9).
    // The worker is this binary (08 §9).
    let pool = match std::env::current_exe() {
        Ok(exe) => repl::open_pool(&exe, cfg.subagents.max_concurrent.max(1) + 1).await,
        Err(e) => {
            tracing::warn!("cannot locate own executable ({e}); python runtime disabled");
            None
        }
    };
    spill::sweep(
        &rt.data_dir.join("spill"),
        Duration::from_secs(7 * 24 * 3600),
    );
    let catalog = Arc::new(Catalog::load(&rt.data_dir));
    let configured = configured(auth.clone());
    // Unresolvable → warn and fall back to the session model rather than fail every title.
    let title_model = cfg.title_model.as_deref().and_then(|id| {
        catalog
            .resolve(id, &*configured)
            .map_err(|e| tracing::warn!("agent.title_model: {e}"))
            .ok()
    });
    let subagent_model = cfg.subagents.model.as_deref().and_then(|id| {
        catalog
            .resolve(id, &*configured)
            .map_err(|e| tracing::warn!("agent.subagents.model: {e}"))
            .ok()
    });
    // 52 §3.1: id/capability are validated like the setter does, but a valid
    // selection whose credentials went away stays selected and fails at use.
    // An invalid one disables only the fallback; the file is left alone and
    // the reason is kept for the API (`Agents::vision_error`).
    let (vision_model, vision_error) = match cfg.vision_model.as_deref() {
        None => (None, None),
        Some(id) => match vision_candidate(&catalog, &*configured, id) {
            Ok(r) => (Some(r), None),
            Err(e) => {
                let e = format!("agent.vision_model: {e}; vision fallback is off");
                tracing::error!("{e}");
                (None, Some(e))
            }
        },
    };
    // 57 §0.9: same leniency as the title model it inherits from.
    let escalate_model = cfg.escalate_model.as_deref().and_then(|id| {
        escalate_candidate(&catalog, &*configured, id)
            .map_err(|e| {
                tracing::warn!("agent.escalate_model: {e}; tier 2 inherits the title model")
            })
            .ok()
    });
    let nodes = Nodes::default();
    // 50 §2.1: the switch and where it came from; every persisted root and
    // parent link root-first, so the registry's synchronous view never sees
    // a dangling ancestor; then restart accounting (§7: nothing re-runs).
    let source = if std::env::var_os("THEUTH_AGENT__COMMUNICATION").is_some() {
        "env"
    } else if rt.config.contains("agent.communication") {
        "config"
    } else {
        "default"
    };
    let communication = SharedPolicy::new(Policy::new(cfg.communication, source));
    load_topology(&store, &communication).await;
    match store.fail_interrupted("interrupted_by_restart").await {
        Ok(i) if !i.exchanges.is_empty() || !i.tasks.is_empty() => tracing::info!(
            "communication: {} accepted exchange(s) and {} task(s) failed: interrupted_by_restart",
            i.exchanges.len(),
            i.tasks.len()
        ),
        Ok(_) => {}
        Err(e) => tracing::error!("communication restart accounting: {e:#}"),
    }
    let agents = Agents::spawn_with_mailbox(
        Agents {
            rt: rt.clone(),
            cfg: cfg.clone(),
            default_model: cfg.model.clone(),
            default_effort: cfg.effort.clone(),
            compact_style: Arc::new(RwLock::new(cfg.compact_style)),
            title_model: Arc::new(RwLock::new(title_model)),
            subagent_model: Arc::new(RwLock::new(subagent_model)),
            vision_model: Arc::new(RwLock::new(vision_model)),
            escalate_model: Arc::new(RwLock::new(escalate_model)),
            vision_error,
            catalog,
            pool,
            configured,
            store,
            client: client(auth.clone())?,
            auth,
            sessions: HashMap::new(),
            running: HashSet::new(),
            communication: communication.clone(),
            network: network::NetworkState::default(),
            waits: HashMap::new(),
            bound: HashMap::new(),
            turn_text: HashMap::new(),
            delivered: HashMap::new(),
            enqueued: HashSet::new(),
            permits: Arc::new(Semaphore::new(cfg.subagents.max_concurrent.max(1))),
            child_turns: HashMap::new(),
            timed_out: HashSet::new(),
            caps: HashMap::new(),
            remote_parent: HashMap::new(),
            owned_children: HashMap::new(),
            last_stop: HashMap::new(),
            last_input: HashMap::new(),
            approvals: HashMap::new(),
            search: Arc::default(),
            nodes: nodes.clone(),
            ledger: Ledger::default(),
            private: HashMap::new(),
            private_member: HashMap::new(),
            closing: HashSet::new(),
            generation: 0,
            durable_resources: SessionResources::durable_arc(&rt.data_dir),
            private_grace: PRIVATE_GRACE,
            transport: None,
            identity: None,
        },
        runtime::subscriber_mailbox(),
    );
    agents.register(AGENT_ACTOR)?;
    UserChannels::spawn(UserChannels::default()).register(USER_ACTOR)?;
    rt.subscribe::<Agents, AgentEvent>(&agents).await;
    // Finished jobs reach the model through `Settle::FinishedWithResult`
    // (47 §4.1), never a second time through the public `JobEvent`.
    rt.subscribe::<Agents, ApprovalRequested>(&agents).await;
    rt.subscribe::<Agents, ApprovalResolved>(&agents).await;
    // Reminder rows mirror the jobs actor (14 as-built): set → insert, cancel → delete.
    rt.subscribe::<Agents, ReminderEvent>(&agents).await;

    // First use of the registry (00 §4, 12 §2).
    let data_dir = rt.data_dir.clone();
    let rt = rt.clone();
    tokio::spawn(async move {
        let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await else {
            return;
        };
        // Availability source before any gated tool exists (50 §2.2): until
        // this lands the registry disables them fail-closed.
        if let Err(e) = reg
            .ask(InstallCommunicationAccess(Arc::new(communication)))
            .await
        {
            tracing::error!("communication access not installed; peer tools stay off: {e}");
        }
        // 47 §4.2 rule 7: coverage first. The registry's ack means every
        // invocation/job from here on is admitted through `Admit`, and any
        // already running was admitted before the reply (its barrier asks back
        // into `Agents`, which is why this runs off the mailbox). Only the
        // ack certifies; an unreachable registry leaves nudges off for good.
        match reg
            .ask(RegisterActivity(ActivitySink::new(agents.clone())))
            .await
        {
            Ok(()) => {
                let _ = agents.tell(ActivityRegistered).await;
            }
            // A handler `Err` (no `Jobs` actor / install failed: jobs not
            // covered) is lifted into `SendError::HandlerError`; with no
            // registry reply at all it is another `SendError`. Neither certifies.
            Err(e) => {
                tracing::error!("activity coverage incomplete; todo continuation stays off: {e}")
            }
        }
        let _ = reg
            .ask(RegisterPrompt {
                plugin: "agent",
                text: format!(
                    "This harness is `theuth`; its state is under {d}: config.yaml (every plugin's section: \
                     `agent.model` = default `provider/model`, `tools.policy`, `rpc`, `web`, `knowledge`, `surface`, `signal`, `secrets`), \
                     secrets.vault (encrypted; provider keys and every other secret — secret_list shows the names, never values), \
                     prompt.md (standing instructions, read every call), \
                     skills/ (markdown skills; this index), knowledge/, sessions.sqlite, logs/theuth.log.<date>. \
                     Load the `harness` skill before editing any of them; it has the full layout and every key. \
                     config.yaml is read once at startup (mcp/, skills/, prompt.md and the vault are live); \
                     the user switches the running model with /model.",
                    d = data_dir.display()
                ),
            })
            .await;
        let _ = reg
            .ask(RegisterPrompt {
                plugin: "attachments",
                text: attach::PROMPT.to_owned(),
            })
            .await;
        if cfg.surface.enabled {
            let _ = reg
                .ask(RegisterPrompt {
                    plugin: "surface",
                    text: surface::prompt_block(),
                })
                .await;
        }
        let dir = skills::dir(&data_dir);
        // 67 §5.1: flat skill files replicate; `skills/<repo>/` clones sync by `sync_skills`.
        tokio::spawn(replica::files::mirror_at(data_dir.join("skills.sqlite"), dir.clone(), "skills", std::time::Duration::from_secs(120)));
        let _ = reg
            .ask(RegisterTool(Arc::new(LoadSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(AddSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(RemoveSkillTool { dir: dir.clone() })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(AddSkillRepoTool {
                dir: dir.clone(),
            })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(RemoveSkillRepoTool {
                dir: dir.clone(),
            })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SyncSkillsTool { dir })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SpawnSubagentTool {
                data_dir,
                nodes,
            })))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SendMessageTool { cfg })))
            .await;
        let _ = reg.ask(RegisterTool(Arc::new(DiscoverPeersTool))).await;
        let _ = reg.ask(RegisterTool(Arc::new(PeerMessageTool))).await;
        let _ = reg.ask(RegisterTool(Arc::new(MessageUserTool))).await;
        let _ = reg.ask(RegisterTool(Arc::new(ListModelsTool))).await;
        let _ = reg
            .ask(RegisterTool(Arc::new(crate::session_read::SessionReadTool)))
            .await;
    });
    Ok(())
}

/// Mirror every persisted root (with its scope and alias) and parent link
/// into the policy, parents before children (50 §2.1). A link whose parent
/// row is missing is skipped and logged: that session routes nothing until
/// its topology is repaired.
async fn load_topology(store: &Store, communication: &SharedPolicy) {
    let (roots, rows) = match (store.roots().await, store.list_sessions().await) {
        (Ok(r), Ok(s)) => (r, s),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("communication topology not loaded; every route fails closed: {e:#}");
            return;
        }
    };
    let mut p = communication.0.write();
    for (id, scope, alias) in roots {
        if let Err(e) = p.record_root(id, scope) {
            tracing::error!(session = %id, "record_root: {e}");
        } else if alias.is_some() {
            let _ = p.set_scope(id, scope, alias);
        }
    }
    let mut pending: Vec<(SessionId, SessionId)> = rows
        .into_iter()
        .filter_map(|r| Some((r.id, r.parent?)))
        .collect();
    // Parents first: repeat until a pass records nothing new.
    loop {
        let before = pending.len();
        pending.retain(|&(child, parent)| p.record_child(child, parent).is_err());
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    for (child, parent) in pending {
        tracing::error!(session = %child, %parent, "parent link not mirrored: unknown or cyclic ancestry");
    }
}

impl Message<SetSessionProject> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetSessionProject,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let project = project::normalize_project(m.project.as_deref())?;
        let root = self
            .store_for(m.session)
            .set_project(m.session, project.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        // Committed above; the caller's next list must see it (RenameSession).
        self.search.invalidate();
        self.rt
            .publish(AgentEvent::SessionMetadataChanged {
                session: root,
                project,
            })
            .await;
        Ok(())
    }
}

impl Message<ListProjectStyles> for Agents {
    type Reply = Result<Vec<crate::ProjectStyle>, String>;

    async fn handle(&mut self, _: ListProjectStyles, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.list_project_styles().await.map_err(|e| e.to_string())
    }
}

impl Message<SetProjectStyle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetProjectStyle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // Same label rule as `SetSessionProject`, so a style row always names
        // a label a session can carry. Styles live in the durable store only,
        // like the session list the sidebar groups (`ListSessions`).
        let name = project::normalize_project(Some(&m.name))?.unwrap_or_default();
        self.store
            .set_project_style(&name, m.icon.as_deref(), m.color.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
}

/// How long a private root outlives its owner's connection before it closes
/// itself (51 §4.3): long enough for a reconnect + `resume_private`.
const PRIVATE_GRACE: Duration = Duration::from_secs(30);
/// Longest `Agents` waits for the owners' close acks before it gives up on
/// them; the registry already times out each participant at 30 s.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(60);

/// `tell` (internal): a private owner's connection has been gone for the
/// grace period; close the root if the same owner still holds it.
#[derive(Clone, Copy, Debug)]
struct PrivateExpired {
    root: SessionId,
    peer: u64,
    generation: u64,
}

/// `tell` (internal): the off-mailbox teardown of a private root finished.
#[derive(Clone, Debug)]
struct PrivateClosed {
    root: SessionId,
    ok: bool,
    detail: Option<String>,
}

/// Constant-time equality over the full length of both strings.
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

impl Agents {
    /// The store `id`'s tree lives in: a private member's in-memory store,
    /// else the durable one (51 §4.5).
    fn store_for(&self, id: SessionId) -> &Store {
        self.private_member
            .get(&id)
            .and_then(|root| self.private.get(root))
            .map_or(&self.store, |p| &p.store)
    }

    /// The file capability `id`'s tree runs under.
    fn resources_for(&self, id: SessionId) -> Arc<SessionResources> {
        self.private_member
            .get(&id)
            .and_then(|root| self.private.get(root))
            .map_or_else(|| self.durable_resources.clone(), |p| p.resources.clone())
    }

    fn retention_of(&self, id: SessionId) -> Retention {
        if self.private_member.contains_key(&id) {
            Retention::Private
        } else {
            Retention::Durable
        }
    }

    /// The tool registry, which owns the lifecycle barrier (51 §8.2).
    /// `Ok(None)` only under `cfg(test)`, where no registry runs and the
    /// barrier itself is the tools crate's to test; in a binary a missing
    /// registry makes private mode unavailable.
    fn registry() -> Result<Option<ActorRef<ToolRegistry>>, String> {
        // A name can outlive its actor (another test's registry, or a crashed
        // one); a dead ref would fail every ask with "actor not running".
        match ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
            Ok(Some(reg)) if reg.is_alive() => Ok(Some(reg)),
            _ if cfg!(test) => Ok(None),
            _ => Err("private mode unavailable: tool registry".into()),
        }
    }

    /// Tell the registry (and through it every owner) the current member
    /// set of a private tree; idempotent superset (51 §8.2).
    async fn open_tree(&self, root: SessionId) {
        let Some(p) = self.private.get(&root) else {
            return;
        };
        if let Ok(Some(reg)) = Self::registry() {
            let _ = reg
                .tell(OpenTree(TreeOpened {
                    root,
                    sessions: p.members.clone(),
                    resources: p.resources.clone(),
                }))
                .await;
        }
    }

    /// A fresh private tree around `store` (51 §4.1): scratch dir, next
    /// generation, owner record, registry announcement and the owner watcher.
    /// The root row must already exist in `store`.
    async fn open_private(
        &mut self,
        root: SessionId,
        store: Arc<Store>,
        owner: (String, u64, CancellationToken, String),
        me: ActorRef<Agents>,
    ) -> Result<(), String> {
        let (view, peer, peer_closed, secret) = owner;
        let scratch = Scratch::create().map_err(|e| format!("private scratch: {e}"))?;
        self.generation += 1;
        let generation = self.generation;
        let resources = Arc::new(SessionResources {
            root: scratch.root.clone(),
            retention: Retention::Private,
            generation,
        });
        self.private.insert(
            root,
            PrivateRoot {
                store,
                scratch: Some(scratch),
                owner: PrivateOwner {
                    view,
                    peer,
                    peer_closed,
                    generation,
                    secret,
                },
                resources,
                members: vec![root],
                cancel: CancellationToken::new(),
                closing: false,
            },
        );
        self.private_member.insert(root, root);
        self.open_tree(root).await;
        self.watch_owner(root, me);
        Ok(())
    }

    /// Watch the owner's connection: gone for [`Self::private_grace`] →
    /// [`PrivateExpired`]. The root's `cancel` token ends the watch (rebind,
    /// close), so a stale watcher never fires for a new owner.
    fn watch_owner(&self, root: SessionId, me: ActorRef<Agents>) {
        let Some(p) = self.private.get(&root) else {
            return;
        };
        let (peer, generation) = (p.owner.peer, p.owner.generation);
        let peer_closed = p.owner.peer_closed.clone();
        let watcher = p.cancel.clone();
        let grace = self.private_grace;
        tokio::spawn(async move {
            tokio::select! {
                _ = watcher.cancelled() => return,
                _ = peer_closed.cancelled() => {}
            }
            tokio::select! {
                _ = watcher.cancelled() => return,
                _ = tokio::time::sleep(grace) => {}
            }
            let _ = me
                .tell(PrivateExpired {
                    root,
                    peer,
                    generation,
                })
                .await;
        });
    }

    /// Fork a private member into a new private root for the same owner
    /// (51 §4.1): the copy is made inside the tree's in-memory store, which
    /// the new root then shares; the store closes with its last tree.
    async fn fork_private(
        &mut self,
        root: SessionId,
        m: ForkSession,
        me: ActorRef<Agents>,
    ) -> Result<SessionId, String> {
        let p = self.private.get(&root).ok_or("no such session")?;
        if p.closing {
            return Err("session closing".into());
        }
        let store = p.store.clone();
        let owner = (
            p.owner.view.clone(),
            p.owner.peer,
            p.owner.peer_closed.clone(),
            // Same holder, same lease: the owner already presents this secret.
            p.owner.secret.clone(),
        );
        let id = store
            .fork(m.session, m.through, m.reply)
            .await
            .map_err(|e| e.to_string())?;
        self.open_private(id, store, owner, me).await?;
        Ok(id)
    }

    /// Close a private tree (51 §4.4), linearised in this mailbox: every
    /// member is marked closed and stopped, the generation bumps and
    /// `SessionClosing` goes out; the owners' barrier, the store and the
    /// scratch dir are torn down off-mailbox, answered by [`PrivateClosed`].
    async fn close_private(&mut self, root: SessionId, me: ActorRef<Agents>) {
        let Some(p) = self.private.get_mut(&root) else {
            return;
        };
        if p.closing {
            return;
        }
        p.closing = true;
        p.cancel.cancel();
        self.generation += 1;
        let generation = self.generation;
        let members = p.members.clone();
        let store = p.store.clone();
        let scratch = p.scratch.take();
        for &id in &members {
            self.closing.insert(id);
            self.private_member.remove(&id);
            // Stop control (47 §4.3), then forget: nothing of this tree continues.
            self.ledger.bump(id);
            self.ledger.forget(id);
            self.running.remove(&id);
            self.approvals.remove(&id);
            self.last_stop.remove(&id);
            self.last_input.remove(&id);
            self.owned_children.remove(&id);
            self.caps.remove(&id);
            self.remote_parent.remove(&id);
            self.bound.retain(|(s, _), _| *s != id);
            self.delivered.retain(|(s, _), _| *s != id);
            self.turn_text.retain(|(s, _), _| *s != id);
            if let Some(timer) = self.child_turns.remove(&id).and_then(|ct| ct.timer) {
                timer.abort();
            }
            if let Some(live) = self.sessions.remove(&id) {
                live.actor.tell_cancel().await;
                live.actor.kill().await;
            }
        }
        self.expire_waits("session closed", |w| members.contains(&w.session));
        self.rt
            .publish(AgentEvent::SessionClosing {
                session: root,
                generation,
            })
            .await;
        let registry = Self::registry();
        tokio::spawn(async move {
            let (mut ok, mut detail) = (true, None);
            match registry {
                Ok(Some(reg)) => {
                    let cancel = CancellationToken::new();
                    let barrier = reg.ask(CloseTreeAll(CloseTree {
                        subtree: members.clone(),
                        generation,
                        cancel: cancel.clone(),
                    }));
                    match tokio::time::timeout(CLOSE_TIMEOUT, barrier).await {
                        Ok(Ok(acks)) => {
                            let failed: Vec<String> = acks
                                .iter()
                                .filter(|a| !a.ok)
                                .map(|a| match &a.detail {
                                    Some(d) => format!("{}: {d}", a.owner),
                                    None => a.owner.clone(),
                                })
                                .collect();
                            if !failed.is_empty() {
                                ok = false;
                                detail = Some(failed.join("; "));
                            }
                        }
                        Ok(Err(e)) => {
                            ok = false;
                            detail = Some(format!("close barrier: {e}"));
                        }
                        Err(_) => {
                            cancel.cancel();
                            ok = false;
                            detail = Some("close barrier: timeout".into());
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    ok = false;
                    detail = Some(e);
                }
            }
            // This tree's rows go even when a fork still shares the store;
            // the last tree out closes the pool.
            if let Err(e) = store.delete_session(root).await {
                tracing::warn!(session = %root, "private rows: {e:#}");
            }
            if let Ok(store) = Arc::try_unwrap(store) {
                store.close().await;
            }
            if let Some(scratch) = scratch
                && let Err(e) = scratch.remove()
            {
                ok = false;
                detail = Some(format!("scratch: {e}"));
            }
            let _ = me.tell(PrivateClosed { root, ok, detail }).await;
        });
    }
}

impl Message<CreateSession> for Agents {
    type Reply = Result<SessionOpen, String>;

    async fn handle(&mut self, m: CreateSession, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let project = project::normalize_project(m.options.project.as_deref())?;
        // 67 §8: `agent: "<harness>@<engine>"` landed here — the root is a
        // child of our own `acp/agents.yaml` entry, `model` goes to it through
        // its model option (unresolved: it is the harness's id, not ours) and
        // the durable path below is not taken.
        if let Some(harness) = &m.options.harness {
            if m.options.retention != Retention::Durable {
                return Err("an external harness cannot open a private session".into());
            }
            // Ours only: a node's harness never chains on to another node.
            let entries = acp::load(&self.rt.data_dir).map_err(|e| format!("{e:#}"))?;
            if !entries.contains_key(harness) {
                return Err(acp::unknown(harness, &entries));
            }
            let cwd = m.options.cwd.as_deref().map(PathBuf::from);
            let (id, _) = self
                .session(
                    None,
                    None,
                    Some((harness.clone(), cwd, None)),
                    (m.options.model.as_deref(), None),
                )
                .await
                .map_err(|e| format!("{e:#}"))?;
            if project.is_some() {
                self.store
                    .set_project(id, project.as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                self.search.invalidate();
            }
            return Ok(SessionOpen {
                session: id,
                root: id,
                retention: Retention::Durable,
                lease_secret: None,
            });
        }
        // Session-only model (`meetings/02-live.md` §0.2): resolved now so a typo
        // fails the create, not the first turn; the default and config.yaml stay.
        let model = match &m.options.model {
            Some(id) => Some(self.catalog.resolve(id, &*self.configured)?.id),
            None => None,
        };
        // Session-only effort, checked against that model like `SetEffort`.
        let effort = m.options.effort.as_deref().filter(|e| *e != "default");
        if let Some(e) = effort {
            let target = model.as_deref().unwrap_or(&self.default_model);
            let known = self.catalog.resolve(target, &*self.configured).ok();
            match known.and_then(|r| r.model) {
                Some(model) => model.validate_effort(e)?,
                None if effort_of(e).is_none() => return Err(format!("unknown effort {e}")),
                None => {}
            }
        }
        if m.options.retention == Retention::Durable {
            let id = self
                .create_durable(
                    m.options.cwd.as_deref().map(Path::new),
                    project.as_deref(),
                    model.as_deref(),
                    effort,
                    m.options.cap,
                    m.options.parent_session.clone().map(|session| RemoteParent {
                        session,
                        engine: m.options.parent_engine.clone(),
                    }),
                )
                .await?;
            return Ok(SessionOpen {
                session: id,
                root: id,
                retention: Retention::Durable,
                lease_secret: None,
            });
        }
        let view = m
            .options
            .view
            .filter(|v| !v.trim().is_empty())
            .ok_or("a private session needs a view")?;
        let (peer, peer_closed) = m.peer.ok_or("a private session needs an RPC peer")?;
        // 51 §8.2: every enabled owner must be able to clean up before any
        // private byte exists.
        if let Some(reg) = Self::registry()? {
            // A handler `Err` (missing owners) is lifted into `HandlerError`.
            reg.ask(ParticipantsReady).await.map_err(|e| match e {
                SendError::HandlerError(missing) => format!("private mode unavailable: {missing}"),
                e => format!("private mode unavailable: {e}"),
            })?;
        }
        let store = Store::open_memory().await.map_err(|e| format!("{e:#}"))?;
        let root = SessionId::new();
        let model = model.unwrap_or_else(|| self.resolve_model(&self.default_model).0.id);
        store
            .create_session(root, &model, None, None, project.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        if let Some(e) = &self.default_effort {
            store
                .set_effort(root, Some(e))
                .await
                .map_err(|e| e.to_string())?;
        }
        if let Some(cwd) = &m.options.cwd {
            store
                .set_cwd(root, Path::new(cwd))
                .await
                .map_err(|e| e.to_string())?;
        }
        let secret = PrivateOwner::mint_secret();
        self.open_private(
            root,
            Arc::new(store),
            (view, peer, peer_closed, secret.clone()),
            ctx.actor_ref().clone(),
        )
        .await?;
        // The loop actor over the private store, so the first input never
        // races the tree's registration above.
        if let Err(e) = self.session(Some(root), None, None, (None, None)).await {
            self.close_private(root, ctx.actor_ref().clone()).await;
            return Err(format!("{e:#}"));
        }
        Ok(SessionOpen {
            session: root,
            root,
            retention: Retention::Private,
            lease_secret: Some(secret),
        })
    }
}

impl Message<Authorize> for Agents {
    type Reply = Result<Authorized, String>;

    async fn handle(&mut self, m: Authorize, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(&root) = self.private_member.get(&m.session) {
            let p = self.private.get(&root).ok_or("no such session")?;
            if p.closing {
                return Err("session closing".into());
            }
            if m.peer != Some(p.owner.peer) {
                return Err("unauthorized".into());
            }
            return Ok(Authorized {
                root,
                retention: Retention::Private,
                generation: p.resources.generation,
                resources: p.resources.clone(),
                owner_peer: Some(p.owner.peer),
            });
        }
        if self.closing.contains(&m.session) {
            return Err("session closed".into());
        }
        match self.store.lineage(m.session).await {
            Ok(l) => Ok(Authorized {
                root: l.root(),
                retention: Retention::Durable,
                generation: 0,
                resources: self.durable_resources.clone(),
                owner_peer: None,
            }),
            Err(e) if e.downcast_ref::<CommError>() == Some(&CommError::NotFound) => {
                Err("no such session".into())
            }
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

impl Message<PrivateTrees> for Agents {
    type Reply = Vec<(SessionId, SessionId, u64)>;

    async fn handle(&mut self, _: PrivateTrees, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // A closing tree stays listed until `PrivateClosed` published its
        // `SessionDeleted`s: a subscriber seeded now must still filter them.
        self.private
            .iter()
            .flat_map(|(root, p)| p.members.iter().map(move |m| (*m, *root, p.owner.peer)))
            .collect()
    }
}

impl Message<ResumePrivate> for Agents {
    type Reply = Result<SessionOpen, String>;

    async fn handle(&mut self, m: ResumePrivate, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let root = *self
            .private_member
            .get(&m.session)
            .ok_or("no such private session")?;
        let p = self.private.get_mut(&root).ok_or("no such private session")?;
        if p.closing {
            return Err("session closing".into());
        }
        if p.owner.view != m.view || !secret_eq(&p.owner.secret, &m.lease_secret) {
            return Err("unauthorized".into());
        }
        // One step: new secret, new holder, old watcher gone, new watcher on.
        let secret = PrivateOwner::mint_secret();
        p.owner.secret = secret.clone();
        p.owner.peer = m.peer.0;
        p.owner.peer_closed = m.peer.1;
        p.cancel.cancel();
        p.cancel = CancellationToken::new();
        self.watch_owner(root, ctx.actor_ref().clone());
        Ok(SessionOpen {
            session: m.session,
            root,
            retention: Retention::Private,
            lease_secret: Some(secret),
        })
    }
}

impl Message<LeaveSession> for Agents {
    type Reply = Result<CloseResult, String>;

    async fn handle(&mut self, m: LeaveSession, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(&root) = self.private_member.get(&m.session) else {
            return Err(if self.closing.contains(&m.session) {
                "session closed".into()
            } else {
                "not a private session".into()
            });
        };
        let p = self.private.get(&root).ok_or("session closed")?;
        if p.closing {
            return Err("session closing".into());
        }
        let holder = m.peer == p.owner.peer
            || m.lease_secret
                .as_deref()
                .is_some_and(|s| secret_eq(s, &p.owner.secret));
        if !holder {
            return Err("unauthorized".into());
        }
        self.close_private(root, ctx.actor_ref().clone()).await;
        Ok(CloseResult {
            revoked: true,
            cleanup: CleanupStatus::Pending,
        })
    }
}

impl Message<PrivateExpired> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PrivateExpired, ctx: &mut Context<Self, Self::Reply>) {
        let still_held = self
            .private
            .get(&m.root)
            .is_some_and(|p| !p.closing && p.owner.peer == m.peer && p.owner.generation == m.generation);
        if still_held {
            tracing::info!(session = %m.root, "private owner gone; closing");
            self.close_private(m.root, ctx.actor_ref().clone()).await;
        }
    }
}

impl Message<PrivateClosed> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PrivateClosed, _: &mut Context<Self, Self::Reply>) {
        let Some(p) = self.private.remove(&m.root) else {
            return;
        };
        if !m.ok {
            tracing::warn!(
                session = %m.root,
                "private cleanup incomplete: {}",
                m.detail.as_deref().unwrap_or("unknown")
            );
        }
        // Children first, then the root: the order `DeleteSession` uses.
        for session in p.members.into_iter().rev() {
            self.rt.publish(AgentEvent::SessionDeleted { session }).await;
        }
    }
}

impl Message<SessionCostQuery> for Agents {
    type Reply = Result<Option<crate::cost::SessionCost>, String>;

    async fn handle(&mut self, m: SessionCostQuery, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store_for(m.0)
            .session_cost(m.0)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::CreateSessionOptions;
    use crate::context::ContextAttachment;

    #[tokio::test]
    async fn bedrock_profile_changes_reach_existing_clients_without_vault_keys() {
        let auth = Auth {
            vault: None,
            oauth: oauth::Store::new(Path::new("/unused")),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            aws_profile: Arc::new(parking_lot::RwLock::new(Some("work".into()))),
        };
        let client = client(auth.clone()).unwrap();
        let catalog = Catalog::load(Path::new("/unused"));
        let model = catalog
            .resolve("amazon-bedrock/amazon.nova-pro-v1:0", &|_| true)
            .unwrap();
        let target = client
            .resolve_service_target(model.genai.as_str())
            .await
            .unwrap();
        assert_eq!(target.model.adapter_kind, AdapterKind::BedrockSigv4);
        assert_eq!(target.auth.single_key_value().unwrap(), "work");

        *auth.aws_profile.write() = Some("personal".into());
        let target = client
            .resolve_service_target(model.genai.as_str())
            .await
            .unwrap();
        assert_eq!(target.auth.single_key_value().unwrap(), "personal");
    }

    /// A keyless local provider (`omlx/...`) with no vault key and no env var
    /// is anonymous, as genai's adapter defines it — not a `FromEnv` that fails
    /// before any request leaves (the smoke's 42 s "stall" with zero hits).
    #[tokio::test]
    async fn keyless_local_adapter_falls_back_to_its_own_default_auth() {
        let client = client(Auth::stub(Path::new("/unused"))).unwrap();
        let target = client.resolve_service_target("omlx::inert").await.unwrap();
        assert_eq!(target.model.adapter_kind, AdapterKind::Omlx);
        // With OMLX_API_KEY set in the environment the adapter itself picks
        // it; the resolver must never invent that requirement.
        if std::env::var_os("OMLX_API_KEY").is_none() {
            assert!(
                matches!(target.auth, AuthData::None),
                "resolver forced a key"
            );
        }
    }

    /// A session loaded mid-round: the call with a response is complete, the
    /// one still running is rendered with `result: None` (not dropped), so the
    /// TUI card survives a switch away and back and `ToolCallFinished` lands on it.
    #[test]
    fn render_keeps_in_flight_tool_calls() {
        use genai::chat::{ToolCall, ToolResponse};
        let call = |id: &str| ToolCall {
            thought_signatures: None,
            call_id: id.into(),
            fn_name: "shell".into(),
            fn_arguments: serde_json::json!({ "cmd": id }),
        };
        let messages = vec![
            (0, ChatMessage::user("go")),
            (1, ChatMessage::from(vec![call("c1"), call("c2")])),
            (2, ChatMessage::from(vec![ToolResponse::new("c1", "ok")])),
        ];
        let got = render(&messages, &BTreeMap::new());
        assert_eq!(got.len(), 3);
        assert!(matches!(
            &got[1],
            (2, HistoryEntry::Tool { call_id, result: Some((out, false)), .. })
                if call_id == "c1" && out == "ok"
        ));
        assert!(matches!(
            &got[2],
            (1, HistoryEntry::Tool { call_id, name, result: None, .. })
                if call_id == "c2" && name == "shell"
        ));
    }

    /// 52 §6: a typed vision note appended after an assistant reply (manual
    /// harvest) is no turn handle, and the real user message after it still
    /// opens one; a plain user message saying the same words is a turn.
    #[test]
    fn render_skips_vision_notes_as_turn_openers() {
        let messages = vec![
            (0, ChatMessage::user("look")),
            (1, ChatMessage::assistant("done")),
            (2, ChatMessage::user("Vision observation: a cat")),
            (3, ChatMessage::user("next")),
        ];
        let mut entries = BTreeMap::new();
        entries.insert(
            2,
            ContextEntry {
                target: ContextTarget::Message { seq: 2 },
                active: true,
                removed: false,
                remove_with: Vec::new(),
                attachments: Vec::new(),
                vision: Some(crate::vision::VisionNote {
                    version: 1,
                    question_model: "x/text".into(),
                    vision_model: "x/eyes".into(),
                    observations: Vec::new(),
                }),
            },
        );
        let turns: Vec<Option<i64>> = render(&messages, &entries)
            .into_iter()
            .filter_map(|(_, e)| match e {
                HistoryEntry::User { turn, .. } => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns, [Some(0), None, Some(3)]);
        // Untyped: only prose, so the quoted note text is an ordinary turn.
        let turns: Vec<Option<i64>> = render(&messages, &BTreeMap::new())
            .into_iter()
            .filter_map(|(_, e)| match e {
                HistoryEntry::User { turn, .. } => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns, [Some(0), Some(2), None]);
    }

    /// A tool response's binary parts carry the produced files' paths in `name`
    /// (`loop_::round`); the history entry lists them so a reloaded transcript
    /// shows the same thumbnails the live `ToolCallFinished` did.
    #[test]
    fn render_replays_tool_files() {
        use genai::chat::{Binary, BinarySource, ToolCall, ToolResponse};
        let call = ToolCall {
            thought_signatures: None,
            call_id: "c1".into(),
            fn_name: "browser_screenshot".into(),
            fn_arguments: serde_json::json!({}),
        };
        let shot = Binary {
            content_type: "image/png".into(),
            source: BinarySource::Base64("".into()),
            name: Some("/data/browser/s/shot-1.png".into()),
        };
        let messages = vec![
            (0, ChatMessage::from(vec![call])),
            (
                1,
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "saved").with_images(vec![shot]),
                ]),
            ),
        ];
        let got = render(&messages, &BTreeMap::new());
        assert!(matches!(
            &got[0],
            (1, HistoryEntry::Tool { result: Some((out, false)), files, .. })
                if out == "saved\n[image: shot-1.png]" && files == &[PathBuf::from("/data/browser/s/shot-1.png")]
        ));
    }

    /// Two responses in one stored Tool row become two cards that share the
    /// row's target (48 §3.3) but each lists only its own response's images,
    /// addressed by original part/image index; a call still running has none.
    #[test]
    fn render_narrows_tool_attachments_to_their_response() {
        use genai::chat::{Binary, BinarySource, ToolCall, ToolResponse};
        let call = |id: &str| ToolCall {
            thought_signatures: None,
            call_id: id.into(),
            fn_name: "shot".into(),
            fn_arguments: serde_json::json!({}),
        };
        let png = |n: &str| Binary {
            content_type: "image/png".into(),
            source: BinarySource::Base64("".into()),
            name: Some(n.into()),
        };
        let messages = vec![
            (
                0,
                ChatMessage::from(vec![call("c1"), call("c2"), call("c3")]),
            ),
            (
                1,
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "a").with_images(vec![png("a.png")]),
                    ToolResponse::new("c2", "b").with_images(vec![png("b.png")]),
                ]),
            ),
        ];
        let binary = |part, image| ContextTarget::Binary {
            owner: ContextOwner::Message { seq: 1 },
            part,
            image: Some(image),
        };
        let attachment = |part, name: &str| ContextAttachment {
            target: binary(part, 0),
            name: name.into(),
            mime: "image/png".into(),
            path: None,
            removed: false,
        };
        let mut entries = BTreeMap::new();
        entries.insert(
            1,
            ContextEntry {
                target: ContextTarget::Message { seq: 1 },
                active: true,
                removed: false,
                remove_with: vec![ContextTarget::Message { seq: 0 }],
                attachments: vec![attachment(0, "a.png"), attachment(1, "b.png")],
                vision: None,
            },
        );
        let got = render(&messages, &entries);
        let ctx = |i: usize| match &got[i].1 {
            HistoryEntry::Tool { context, .. } => context.as_ref(),
            _ => unreachable!(),
        };
        for (i, name) in [(0, "a.png"), (1, "b.png")] {
            let c = ctx(i).unwrap();
            assert_eq!(c.target, ContextTarget::Message { seq: 1 });
            assert_eq!(c.remove_with, [ContextTarget::Message { seq: 0 }]);
            assert_eq!(c.attachments.len(), 1);
            assert_eq!(c.attachments[0].name, name);
            assert_eq!(c.attachments[0].target, binary(i as u32, 0));
        }
        assert!(
            matches!(&got[2].1, HistoryEntry::Tool { call_id, result: None, .. } if call_id == "c3")
        );
        assert!(ctx(2).is_none());
    }

    /// Page-time previews follow the original coordinates into the stored
    /// arrays (a direct part, a nested tool image, a head part); URL sources
    /// and unknown MIMEs stay path-less.
    #[test]
    fn materialize_writes_previews_by_original_coordinates() {
        use genai::chat::{Binary, BinarySource, ContentPart, MessageContent, ToolResponse};
        let b64 = |s: &str| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, s);
        let png = |body: &str| Binary {
            content_type: "image/png".into(),
            source: BinarySource::Base64(b64(body).into()),
            name: None,
        };
        let messages = vec![
            (
                0,
                ChatMessage::user(MessageContent::from_parts(vec![
                    ContentPart::Text("hi".into()),
                    ContentPart::Binary(png("direct")),
                    ContentPart::Binary(Binary {
                        content_type: "image/png".into(),
                        source: BinarySource::Url("https://x/y.png".into()),
                        name: None,
                    }),
                ])),
            ),
            (
                1,
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "t")
                        .with_images(vec![png("nested-0"), png("nested-1")]),
                ]),
            ),
        ];
        let epochs = vec![Epoch {
            n: 1,
            at_seq: 1,
            tail_from: 2,
            head: ChatMessage::user(MessageContent::from_parts(vec![
                ContentPart::Text("summary".into()),
                ContentPart::Binary(png("head")),
            ])),
        }];
        let msg = |seq| ContextOwner::Message { seq };
        let att = |owner, part, image| ContextAttachment {
            target: ContextTarget::Binary { owner, part, image },
            name: "x".into(),
            mime: "image/png".into(),
            path: None,
            removed: false,
        };
        let entry = |target, attachments| {
            Some(ContextEntry {
                target,
                active: true,
                removed: false,
                remove_with: Vec::new(),
                attachments,
                vision: None,
            })
        };
        let mut history = vec![
            HistoryEntry::User {
                text: String::new(),
                turn: Some(0),
                context: entry(
                    ContextTarget::Message { seq: 0 },
                    vec![att(msg(0), 1, None), att(msg(0), 2, None)],
                ),
                peer: None,
                vision: None,
            },
            HistoryEntry::Tool {
                call_id: "c1".into(),
                name: "t".into(),
                args: serde_json::Value::Null,
                result: None,
                files: Vec::new(),
                context: entry(
                    ContextTarget::Message { seq: 1 },
                    vec![att(msg(1), 0, Some(1))],
                ),
            },
            HistoryEntry::Compacted {
                epoch: 1,
                summary: String::new(),
                style: CompactStyle::default(),
                context: entry(
                    ContextTarget::Head { epoch: 1 },
                    vec![att(ContextOwner::Head { epoch: 1 }, 1, None)],
                ),
            },
        ];
        let dir = std::env::temp_dir().join(format!("theuth-preview-{}", uuid::Uuid::new_v4()));
        materialize(&mut history, &messages, &epochs, &dir);
        let paths: Vec<Option<String>> = history
            .iter()
            .flat_map(|h| h.context().unwrap().attachments.iter())
            .map(|a| a.path.as_ref().map(|p| std::fs::read_to_string(p).unwrap()))
            .collect();
        assert_eq!(
            paths,
            [
                Some("direct".into()),
                None,
                Some("nested-1".into()),
                Some("head".into())
            ]
        );
        // Coordinates are metadata about the stored rows, not the preview file.
        assert_eq!(
            history[1].context().unwrap().attachments[0].target,
            ContextTarget::Binary {
                owner: msg(1),
                part: 0,
                image: Some(1)
            }
        );
    }

    /// Paging walks back from the tail: the newest `limit`, then everything
    /// older than the page's `first_seq`; a call still running is never
    /// repeated on an older page.
    #[test]
    fn page_walks_back_by_seq() {
        let user = |s: &str| HistoryEntry::User {
            text: s.into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
        };
        let tool = |id: &str, result| HistoryEntry::Tool {
            call_id: id.into(),
            name: "shell".into(),
            args: serde_json::Value::Null,
            result,
            files: Vec::new(),
            context: None,
        };
        let all = || {
            vec![
                (0, user("a")),
                (1, user("b")),
                (2, user("c")),
                (4, tool("c1", Some(("ok".into(), false)))),
                (3, tool("c2", None)),
            ]
        };
        let (h, first, more) = page(all(), None, Some(2));
        assert_eq!(h.len(), 2);
        assert!(more);
        assert_eq!(first, Some(4));
        assert!(matches!(&h[1], HistoryEntry::Tool { result: None, .. }));

        let (h, first, more) = page(all(), first, Some(100));
        assert_eq!(first, Some(0));
        assert!(!more);
        assert!(matches!(
            &h[..],
            [
                HistoryEntry::User { .. },
                HistoryEntry::User { .. },
                HistoryEntry::User { .. }
            ]
        ));

        let (h, first, more) = page(all(), None, None);
        assert_eq!((h.len(), first, more), (5, Some(0), false));
        let (h, first, more) = page(Vec::new(), None, Some(3));
        assert_eq!((h.len(), first, more), (0, None, false));
    }

    /// 08 as-built: a `python` card is followed by its stored inner calls in
    /// `n` order, whatever page it lands on; a row the cell never awaited is a
    /// live card only while the turn runs, else the `NOT_RUN` error; rows of a
    /// parent not on the page stay out.
    #[test]
    fn attach_inner_glues_children_to_their_python_card() {
        let user = |s: &str| HistoryEntry::User {
            text: s.into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
        };
        let python = |id: &str, result| HistoryEntry::Tool {
            call_id: id.into(),
            name: repl::NAME.into(),
            args: serde_json::json!({"code": "x"}),
            result,
            files: Vec::new(),
            context: None,
        };
        let child = |n: u32, name: &str, result: Option<(String, bool)>| store::InnerCall {
            n,
            name: name.into(),
            args: serde_json::json!({"n": n}),
            result,
            files: Vec::new(),
        };
        let inner = || {
            HashMap::from([
                (
                    "p1".to_owned(),
                    vec![
                        child(1, "shell", Some(("ok".into(), false))),
                        child(2, "read", Some(("ERROR: boom".into(), true))),
                    ],
                ),
                ("p2".to_owned(), vec![child(1, "grep", None)]),
            ])
        };
        let ids = |h: &[HistoryEntry]| -> Vec<String> {
            h.iter()
                .map(|e| match e {
                    HistoryEntry::Tool { call_id, .. } => call_id.clone(),
                    HistoryEntry::User { text, .. } => text.clone(),
                    _ => unreachable!(),
                })
                .collect()
        };
        let all = || {
            vec![
                (0, user("a")),
                (2, python("p1", Some(("done".into(), false)))),
                (3, user("b")),
                (4, python("p2", None)),
            ]
        };
        // Whole transcript, turn running: p2's unawaited call is a live card.
        let (mut h, _, _) = page(all(), None, None);
        attach_inner(&mut h, &mut inner(), true);
        assert_eq!(ids(&h), ["a", "p1", "p1.1", "p1.2", "b", "p2", "p2.1"]);
        assert!(matches!(
            &h[2],
            HistoryEntry::Tool { name, args, result: Some((out, false)), .. }
                if name == "shell" && args == &serde_json::json!({"n": 1}) && out == "ok"
        ));
        assert!(matches!(
            &h[3],
            HistoryEntry::Tool { name, result: Some((out, true)), .. }
                if name == "read" && out == "ERROR: boom"
        ));
        assert!(matches!(&h[6], HistoryEntry::Tool { result: None, .. }));
        // Turn over (crash, or loaded later): the same row is the not-run error.
        let (mut h, _, _) = page(all(), None, None);
        attach_inner(&mut h, &mut inner(), false);
        assert!(matches!(
            &h[6],
            HistoryEntry::Tool { result: Some((out, true)), .. } if out == NOT_RUN
        ));
        // A one-entry page holding p1 carries both children; p2's rows stay out.
        let (mut h, first, more) = page(all(), Some(3), Some(1));
        let mut left = inner();
        attach_inner(&mut h, &mut left, true);
        assert_eq!((first, more), (Some(2), true));
        assert_eq!(ids(&h), ["p1", "p1.1", "p1.2"]);
        assert!(left.contains_key("p2") && !left.contains_key("p1"));
        // The page before it has no python card and gains nothing.
        let (mut h, _, _) = page(all(), first, None);
        attach_inner(&mut h, &mut inner(), true);
        assert_eq!(ids(&h), ["a"]);
    }

    /// A limit that would land inside a group of entries sharing one seq
    /// (sibling tool cards of one response row, a divider on its message)
    /// widens the page to the whole group, so `before = first_seq` on the
    /// next page skips nothing (48 §5).
    #[test]
    fn page_never_splits_a_seq_group() {
        let user = |s: &str| HistoryEntry::User {
            text: s.into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
        };
        let tool = |id: &str| HistoryEntry::Tool {
            call_id: id.into(),
            name: "shell".into(),
            args: serde_json::Value::Null,
            result: Some(("ok".into(), false)),
            files: Vec::new(),
            context: None,
        };
        let all = || {
            vec![
                (0, user("a")),
                (2, tool("c1")),
                (2, tool("c2")),
                (2, tool("c3")),
                (3, user("b")),
            ]
        };
        let (h, first, more) = page(all(), None, Some(2));
        assert_eq!((h.len(), first, more), (4, Some(2), true));
        let (h, first, more) = page(all(), first, Some(2));
        assert_eq!((h.len(), first, more), (1, Some(0), false));
        assert!(matches!(&h[0], HistoryEntry::User { text, .. } if text == "a"));
        // A limit of zero stays empty rather than indexing past the end.
        let (h, first, more) = page(all(), None, Some(0));
        assert_eq!((h.len(), first, more), (0, None, true));
    }

    #[test]
    fn job_result_wrapper_escapes_and_names_exit() {
        let id = JobId("j3".into());
        let w = job_result(&id, "echo \"a\" <b>", &JobExit::Exited(0), 7, "out", false);
        assert_eq!(
            w,
            "<job_result id=\"j3\" command=\"echo &quot;a&quot; &lt;b&gt;\" exit=\"0\" secs=\"7\">\nout\n</job_result>"
        );
        assert!(!w.contains("pty"));
        let w = job_result(&id, "python3 -q", &JobExit::Exited(0), 41, ">>> ", true);
        assert_eq!(
            w,
            "<job_result id=\"j3\" command=\"python3 -q\" exit=\"0\" secs=\"41\" pty=\"true\">\n>>> \n</job_result>"
        );
        let w = job_result(&id, "sleep 9", &JobExit::Timeout, 1800, "", false);
        assert!(w.contains(" exit=\"timeout\" "));
        let w = job_result(&id, "x", &JobExit::Killed, 1, "", false);
        assert!(w.contains(" exit=\"killed\" "));
        let long = "a".repeat(200);
        let w = job_result(&id, &long, &JobExit::Exited(1), 0, "", false);
        assert!(w.contains(&format!(" command=\"{}\" ", "a".repeat(120))));
        assert!(!w.contains(&"a".repeat(121)));
    }

    #[test]
    fn reminder_wrapper_escapes_attributes_and_names_late() {
        let w = reminder("r3", 600, "2026-09-23T10:00:00+02:00", false, "check \"a\" <b>");
        assert_eq!(
            w,
            "<reminder id=\"r3\" after=\"600\" set_at=\"2026-09-23T10:00:00+02:00\" late=\"false\">\ncheck \"a\" <b>\n</reminder>"
        );
        let w = reminder("r<1>", 5, "\"t\"", true, "");
        assert_eq!(
            w,
            "<reminder id=\"r&lt;1&gt;\" after=\"5\" set_at=\"&quot;t&quot;\" late=\"true\">\n\n</reminder>"
        );
    }

    #[test]
    fn job_idle_wrapper_escapes_and_names_optionals() {
        let id = JobId("j4".into());
        let w = job_idle(
            &id,
            "echo \"a\" <b>",
            None,
            63,
            912,
            1,
            None,
            ">>> \ncursor: row 1 col 5",
        );
        assert_eq!(
            w,
            "<job_idle id=\"j4\" command=\"echo &quot;a&quot; &lt;b&gt;\" quiet=\"63\" secs=\"912\" nudge=\"1\">\n>>> \ncursor: row 1 col 5\n</job_idle>"
        );
        let w = job_idle(&id, "omp", Some("omp<1>"), 120, 30, 2, Some(12), "");
        assert_eq!(
            w,
            "<job_idle id=\"j4\" command=\"omp\" name=\"omp&lt;1&gt;\" quiet=\"120\" secs=\"30\" nudge=\"2\" user_typed=\"12\">\n\n</job_idle>"
        );
        let long = "a".repeat(200);
        let w = job_idle(&id, &long, None, 0, 0, 1, None, "");
        assert!(w.contains(&format!(" command=\"{}\" ", "a".repeat(120))));
        assert!(!w.contains(&"a".repeat(121)));
    }

    #[test]
    fn first_line_skips_blanks_and_cuts_by_char() {
        assert_eq!(first_line("\n  \n  hello world  \nmore"), "hello world");
        assert_eq!(first_line(""), "");
        let long = "é".repeat(100);
        let cut = first_line(&long);
        assert_eq!(cut.chars().count(), 80);
        assert!(cut.ends_with('…'));
        assert_eq!(first_line(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn check_files_rejects_what_a_provider_would() {
        let dir = std::env::temp_dir().join(format!("theuth-attach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"hi").unwrap();
        let pdf = dir.join("scan.pdf");
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        let big_pdf = dir.join("big.pdf");
        std::fs::File::create(&big_pdf)
            .unwrap()
            .set_len(MAX_PDF + 1)
            .unwrap();
        let big = dir.join("big.jpg");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_ATTACHMENT + 1)
            .unwrap();

        assert_eq!(check_files(&[]), Ok(()));
        assert_eq!(check_files(std::slice::from_ref(&png)), Ok(()));
        // A PDF rides as a native document part, with its own (larger) cap.
        assert_eq!(check_files(std::slice::from_ref(&pdf)), Ok(()));
        assert_eq!(
            check_files(&[big_pdf]).unwrap_err(),
            "attachment big.pdf: 32.0 MB exceeds 32 MB"
        );
        assert_eq!(
            check_files(&[png, txt]).unwrap_err(),
            "attachment notes.txt: text/plain is not an image or PDF"
        );
        assert_eq!(
            check_files(&[dir.join("gone.png")]).unwrap_err(),
            "attachment gone.png: not found"
        );
        assert_eq!(
            check_files(std::slice::from_ref(&dir)).unwrap_err(),
            format!(
                "attachment {}: not a regular file",
                dir.file_name().unwrap().to_string_lossy()
            )
        );
        assert_eq!(
            check_files(&[big]).unwrap_err(),
            "attachment big.jpg: 5.0 MB exceeds 5 MB"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An `Agents` value (no actor, no tool registry) over a temp store: the
    /// supervisor's own sequencing runs inline and its ledger is readable.
    /// Session actors it spawns are real; with no registry their turns end
    /// as `error` before any model is called.
    async fn agents() -> Agents {
        agents_with(false).await
    }

    /// [`agents`] with `agent.communication` preset. Shared with the network
    /// module's tests, which drive the same value through its hooks.
    pub(crate) async fn agents_with(communication: bool) -> Agents {
        let dir = std::env::temp_dir().join(format!("theuth-agents-{}", uuid::Uuid::now_v7()));
        agents_on(dir, None, communication).await
    }

    /// [`agents_with`] over `dir`'s store opened as mesh engine `engine`
    /// (69 P: two of these, each holding the other's rows, are two engines).
    pub(crate) async fn agents_on(dir: PathBuf, engine: Option<&str>, communication: bool) -> Agents {
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Arc::new(AgentConfig::default());
        let engine = engine.map(|e| mesh::EngineId(e.to_owned()));
        Agents {
            rt: Runtime::new(figment::Figment::new(), dir.clone()),
            default_model: cfg.model.clone(),
            default_effort: None,
            cfg,
            compact_style: Arc::default(),
            title_model: Arc::default(),
            subagent_model: Arc::default(),
            vision_model: Arc::default(),
            escalate_model: Arc::default(),
            vision_error: None,
            catalog: Arc::new(Catalog::load(&dir)),
            pool: None,
            configured: Arc::new(|_: &str| false),
            store: Store::open(&dir.join("sessions.sqlite"), engine).await.unwrap(),
            client: Client::new().unwrap(),
            auth: Auth::stub(&dir),
            sessions: HashMap::new(),
            running: HashSet::new(),
            communication: SharedPolicy::new(Policy::new(communication, "config")),
            network: network::NetworkState::default(),
            waits: HashMap::new(),
            bound: HashMap::new(),
            turn_text: HashMap::new(),
            delivered: HashMap::new(),
            enqueued: HashSet::new(),
            permits: Arc::new(Semaphore::new(1)),
            child_turns: HashMap::new(),
            timed_out: HashSet::new(),
            caps: HashMap::new(),
            remote_parent: HashMap::new(),
            owned_children: HashMap::new(),
            last_stop: HashMap::new(),
            last_input: HashMap::new(),
            approvals: HashMap::new(),
            search: Arc::default(),
            nodes: Nodes::default(),
            ledger: Ledger::default(),
            private: HashMap::new(),
            private_member: HashMap::new(),
            closing: HashSet::new(),
            generation: 0,
            durable_resources: SessionResources::durable_arc(&dir),
            // Short enough for a test to outwait it (`resume_rebinds_and_cancels_old_watcher`).
            private_grace: Duration::from_millis(200),
            transport: None,
            identity: None,
        }
    }

    /// A catalog with an image-capable `testvis/eyes`, a text-only
    /// `testvis/blind`, and a second provider also offering `eyes`.
    fn vision_catalog(dir: &Path) -> Catalog {
        let model = |id: &str, image: bool| {
            serde_json::json!({
                "id": id, "name": id,
                "limit": { "context": 100_000, "output": 4_096 },
                "image": image
            })
        };
        let user = serde_json::json!({
            "testvis": {
                "id": "testvis", "name": "Test Vision",
                "models": { "eyes": model("eyes", true), "blind": model("blind", false) }
            },
            "othervis": {
                "id": "othervis", "name": "Other",
                "models": { "eyes": model("eyes", true) }
            }
        });
        std::fs::write(dir.join("models.json"), user.to_string()).unwrap();
        Catalog::load(dir)
    }

    /// 52 §3.1: the fallback must be an exact, image-capable catalog entry —
    /// fuzzy siblings, pass-through ids, text-only entries, and ambiguous
    /// bare ids are rejected; a unique bare id canonicalises.
    #[test]
    fn vision_candidate_requires_exact_image_capable_entry() {
        let dir = std::env::temp_dir().join(format!("theuth-vision-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let catalog = vision_catalog(&dir);
        let only_test = |ns: &str| ns == "testvis";
        let both = |ns: &str| ns == "testvis" || ns == "othervis";

        assert_eq!(
            vision_candidate(&catalog, &only_test, "testvis/eyes")
                .unwrap()
                .id,
            "testvis/eyes"
        );
        assert_eq!(
            vision_candidate(&catalog, &only_test, "eyes").unwrap().id,
            "testvis/eyes"
        );
        // A provider that is not configured still passes the id/capability rule
        // (startup keeps such a selection); the setter adds the provider check.
        assert!(vision_candidate(&catalog, &|_| false, "testvis/eyes").is_ok());
        assert_eq!(
            vision_candidate(&catalog, &only_test, "testvis/blind").unwrap_err(),
            "`testvis/blind` cannot receive images"
        );
        // Fuzzy sibling (`closest` would find `eyes`) and pass-through ids are not exact.
        for id in ["testvis/eyes-fast", "nowhere/eyes", "no-such-model"] {
            let e = vision_candidate(&catalog, &only_test, id).unwrap_err();
            assert!(e.contains("not an exact catalog entry"), "{id}: {e}");
        }
        assert!(
            vision_candidate(&catalog, &both, "eyes")
                .unwrap_err()
                .starts_with("ambiguous model `eyes`")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 52 §3.1: the setter validates, writes `config.yaml`, then swaps the live
    /// value; an unconfigured provider or a failed write changes nothing, and
    /// `None` clears both. A startup diagnostic is cleared by a successful set.
    #[tokio::test]
    async fn set_vision_model_persists_before_swapping() {
        let mut a = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.vision_error = Some("stale".into());
        let live = |a: &Agents| {
            a.vision_model
                .read()
                .unwrap()
                .as_ref()
                .map(|r| r.id.clone())
        };

        // Valid entry, provider not configured: rejected, nothing written.
        let e = a.set_vision_model(Some("testvis/eyes")).unwrap_err();
        assert!(e.contains("`testvis` is not configured"), "{e}");
        assert_eq!(live(&a), None);
        assert!(!dir.join("config.yaml").exists());
        assert_eq!(a.vision_error.as_deref(), Some("stale"));

        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let info = a.set_vision_model(Some("eyes")).unwrap().unwrap();
        assert_eq!(info.id, "testvis/eyes");
        assert!(info.image);
        assert_eq!(live(&a).as_deref(), Some("testvis/eyes"));
        assert_eq!(a.vision_error, None);
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("  vision_model: testvis/eyes\n"), "{yaml}");

        // A write failure leaves the live value as it was.
        std::fs::remove_file(dir.join("config.yaml")).unwrap();
        std::fs::create_dir(dir.join("config.yaml")).unwrap();
        let e = a.set_vision_model(None).unwrap_err();
        assert!(e.starts_with("config.yaml: "), "{e}");
        assert_eq!(live(&a).as_deref(), Some("testvis/eyes"));
        std::fs::remove_dir(dir.join("config.yaml")).unwrap();

        assert!(a.set_vision_model(None).unwrap().is_none());
        assert_eq!(live(&a), None);
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("  vision_model: \n"), "{yaml}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 57 §0.9: the setter refuses an id that is not a catalog entry (or is
    /// ambiguous) and changes nothing; a unique bare id canonicalises and is
    /// written before the live value swaps; the getter reports the effective
    /// model — the escalation model, else the title model, else `None` (the
    /// session's model) — and `None` clears the slot.
    #[tokio::test]
    async fn set_escalate_model_validates_and_reports_effective() {
        let mut a = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis" || ns == "othervis");

        let e = a.set_escalate_model(Some("testvis/nothing")).unwrap_err();
        assert!(e.contains("not a catalog entry"), "{e}");
        let e = a.set_escalate_model(Some("eyes")).unwrap_err();
        assert!(e.contains("ambiguous"), "{e}");
        assert!(a.escalate_model().model.is_none());
        assert!(!dir.join("config.yaml").exists());

        // Unset: inherits the title model when there is one.
        assert!(a.escalate_model().effective.is_none());
        *a.title_model.write().unwrap() =
            Some(a.catalog.resolve("testvis/eyes", &|_| true).unwrap());
        let g = a.escalate_model();
        assert!(g.model.is_none());
        assert_eq!(g.effective.map(|m| m.id).as_deref(), Some("testvis/eyes"));

        // Text-only is enough; the bare unique id canonicalises.
        let info = a.set_escalate_model(Some("blind")).unwrap().unwrap();
        assert_eq!(info.id, "testvis/blind");
        assert!(!info.image);
        let g = a.escalate_model();
        assert_eq!(g.model.map(|m| m.id).as_deref(), Some("testvis/blind"));
        assert_eq!(g.effective.map(|m| m.id).as_deref(), Some("testvis/blind"));
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("  escalate_model: testvis/blind\n"), "{yaml}");

        // A failed write leaves the live value as it was.
        std::fs::remove_file(dir.join("config.yaml")).unwrap();
        std::fs::create_dir(dir.join("config.yaml")).unwrap();
        assert!(
            a.set_escalate_model(None)
                .unwrap_err()
                .starts_with("config.yaml: ")
        );
        assert_eq!(
            a.escalate_model().model.map(|m| m.id).as_deref(),
            Some("testvis/blind")
        );
        std::fs::remove_dir(dir.join("config.yaml")).unwrap();

        assert!(a.set_escalate_model(None).unwrap().is_none());
        let g = a.escalate_model();
        assert!(g.model.is_none());
        assert_eq!(g.effective.map(|m| m.id).as_deref(), Some("testvis/eyes"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `agent.escalate_model` reads and writes like its siblings; absent = unset.
    #[test]
    fn escalate_model_config_round_trip() {
        let cfg: AgentConfig =
            serde_yaml::from_str("escalate_model: cheap/e\ntitle_model: cheap/t\n").unwrap();
        assert_eq!(cfg.escalate_model.as_deref(), Some("cheap/e"));
        assert_eq!(cfg.title_model.as_deref(), Some("cheap/t"));
        let back: AgentConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.escalate_model, cfg.escalate_model);
        let cfg: AgentConfig = serde_yaml::from_str("model: x/y\n").unwrap();
        assert_eq!(cfg.escalate_model, None);
    }

    /// `ModelInfo.image` is effective input support (52 §3.2): a Cursor id
    /// with an image-capable catalog sibling reports `false`.
    #[test]
    fn model_info_image_is_effective_support() {
        let dir = std::env::temp_dir().join(format!("theuth-vision-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let catalog = vision_catalog(&dir);
        let (p, m) = catalog.get("testvis/eyes").unwrap();
        assert!(ModelInfo::new(&p.id, m).image);
        assert!(!ModelInfo::new("cursor", m).image);
        let mut r = catalog.resolve("testvis/eyes", &|_| true).unwrap();
        assert!(ModelInfo::from(&r).image);
        r.genai = "cursor::eyes".into();
        assert!(!ModelInfo::from(&r).image);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 52 §3.1: a wrongly typed `agent.vision_model` is a startup error that
    /// names the section, never a silent reset of `agent.model` and friends
    /// to defaults; an absent section still means defaults.
    #[tokio::test]
    async fn malformed_agent_section_is_reported_not_defaulted() {
        use figment::providers::{Format, Yaml};
        let dir = std::env::temp_dir();
        let rt = |yaml: &str| {
            Runtime::new(
                figment::Figment::new().merge(Yaml::string(yaml)),
                dir.clone(),
            )
        };
        let cfg = agent_config(&rt("agent:\n  model: x/y\n  vision_model: x/eyes\n")).unwrap();
        assert_eq!(cfg.model, "x/y");
        assert_eq!(cfg.vision_model.as_deref(), Some("x/eyes"));
        let e = agent_config(&rt("agent:\n  model: x/y\n  vision_model: [1, 2]\n"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("[agent] config section"), "{e}");
        assert!(
            agent_config(&rt("rpc:\n  listen: 127.0.0.1:1\n"))
                .unwrap()
                .vision_model
                .is_none()
        );
    }

    /// 50 §2.1: a session created here is mirrored into the policy with its
    /// persisted root/parent links, and a restart's `load_topology` rebuilds
    /// the same view root-first, so a lazily resumed child inherits its
    /// ancestor's opt-out instead of routing as an unknown root.
    #[tokio::test]
    async fn topology_is_mirrored_and_reloaded_root_first() {
        let mut a = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        assert_eq!(a.communication.0.read().root_of(child).unwrap(), root);
        assert!(a.communication.0.write().route(root, child).is_ok());

        // A fresh policy over the same store: what `start` would build.
        let fresh = SharedPolicy::new(Policy::new(true, "config"));
        load_topology(&a.store, &fresh).await;
        assert_eq!(fresh.0.read().root_of(child).unwrap(), root);
        fresh.0.write().set_session(root, false).unwrap();
        assert!(matches!(
            fresh.0.write().route(child, root),
            Err(PolicyError::Disabled { .. })
        ));
    }

    /// 50 §2.3, §5.1: another root's session is not addressable from an
    /// isolated tree and the refusal does not disclose that it exists; a
    /// guessed UUID with no row fails the same way and creates nothing.
    #[tokio::test]
    async fn cross_root_and_unknown_addresses_are_one_answer() {
        let mut a = agents_with(true).await;
        let (me, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let (other, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let rev = a.store.context_revision(other).await.unwrap();
        assert!(
            a.store
                .set_title_at_revision(other, "Merge design 62", &["rust".into(), "merge".into()], &rev)
                .await
                .unwrap()
        );
        let e = a.communication.0.write().route(me, other).unwrap_err();
        assert_eq!(
            a.route_error(me, other, e),
            format!("unknown peer `session:{other}`")
        );
        let ghost = SessionId::new();
        assert_eq!(
            a.resolve(me, &format!("session:{ghost}"))
                .await
                .err()
                .unwrap(),
            format!("unknown peer `session:{ghost}`")
        );
        assert!(a.store.session(ghost).await.unwrap().is_none());
        // Both roots `owner`: the route opens and the directory shows it as `Root`.
        for r in [me, other] {
            a.store
                .set_root_scope(r, PeerScope::Owner, None)
                .await
                .unwrap();
            a.communication
                .0
                .write()
                .set_scope(r, PeerScope::Owner, None)
                .unwrap();
        }
        let rows = a.directory(me).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, format!("session:{other}"));
        assert_eq!(rows[0].relationship, PeerRelationship::Root);
        assert_eq!(rows[0].state, PeerState::Idle);
        // 62 §0.7: liveness detail crosses roots, the title does not; the
        // tags do (50 §5.1 as-built), so a model can pick relevant roots.
        assert_eq!(rows[0].title, None);
        assert_eq!(rows[0].tags, ["rust", "merge"]);
        let act = rows[0].activity.as_ref().expect("activity");
        assert_eq!(act.state, PeerLiveState::Idle);
        assert_eq!((act.jobs, act.ptys, act.children), (0, 0, 0));
    }

    /// 62 §0.8: a fan-out resolves every address first; each answer is the
    /// canonical address a receipt carries (a child by name, `parent`, a
    /// remote alias) or the same refusal a single send would get, in order.
    #[tokio::test]
    async fn resolve_all_is_canonical_and_ordered() {
        let mut a = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let ghost = SessionId::new();
        let to = [
            "kid".to_owned(),
            format!("session:{ghost}"),
            "peer:lab".to_owned(),
        ];
        assert_eq!(
            a.resolve_all(root, &to).await,
            vec![
                Ok(format!("session:{child}")),
                Err(format!("unknown peer `session:{ghost}`")),
                Ok("peer:lab".to_owned()),
            ]
        );
        assert_eq!(
            a.resolve_all(child, &["parent".to_owned()]).await,
            vec![Ok(format!("session:{root}"))]
        );
    }

    /// 62 §0.7: within the caller's own tree a row carries the session title
    /// and its activity; a cold child reads `stored`.
    #[tokio::test]
    async fn same_tree_rows_carry_title_and_activity() {
        let mut a = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.store.set_title(root, "deploy checklist").await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let rows = a.directory(child).await.unwrap();
        let parent = rows.iter().find(|r| r.relationship == PeerRelationship::Parent).unwrap();
        assert_eq!(parent.title.as_deref(), Some("deploy checklist"));
        assert_eq!(parent.activity.as_ref().unwrap().state, PeerLiveState::Idle);

        // A stored-only sibling: no actor, so `stored` regardless of last stop.
        let cold = SessionId::new();
        a.store
            .create_session(cold, "m", Some(root), Some("cold"), None)
            .await
            .unwrap();
        let rows = a.directory(child).await.unwrap();
        let cold_row = rows
            .iter()
            .find(|r| r.address == format!("session:{cold}"))
            .unwrap();
        assert_eq!(cold_row.relationship, PeerRelationship::Sibling);
        assert_eq!(cold_row.state, PeerState::Stored);
        assert_eq!(cold_row.activity.as_ref().unwrap().state, PeerLiveState::Stored);
    }

    /// 62 §3.4: `active_only` drops an idle jobless row and keeps one with a
    /// running PTY; the default order is working first, then `last_active`
    /// desc, remotes (no activity) last.
    #[test]
    fn arrange_orders_working_first_and_filters_active_only() {
        let row = |address: &str, activity: Option<PeerActivity>| PeerInfo {
            address: address.into(),
            name: address.into(),
            kind: PeerKind::Native,
            relationship: PeerRelationship::Root,
            state: PeerState::Idle,
            send: true,
            reply: true,
            stream: false,
            activity,
            title: None,
            tags: Vec::new(),
            engine: None,
        };
        let act = |state: PeerLiveState, ptys: u16, secs_ago: i64| PeerActivity {
            state,
            last_active: Some(Utc::now() - chrono::Duration::seconds(secs_ago)),
            turn_secs: None,
            jobs: 0,
            ptys,
            children: 0,
        };
        let mut rows = vec![
            row("peer:lab", None),
            row("idle-old", Some(act(PeerLiveState::Idle, 0, 600))),
            row("pty", Some(act(PeerLiveState::Idle, 1, 300))),
            row("idle-new", Some(act(PeerLiveState::Idle, 0, 10))),
            row("running", Some(act(PeerLiveState::Running, 0, 900))),
        ];
        arrange(&mut rows, false);
        let order: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(order, ["pty", "running", "idle-new", "idle-old", "peer:lab"]);
        arrange(&mut rows, true);
        let order: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(order, ["pty", "running"]);
    }

    /// 50 §5.3, §8: a wait resolves only by its exact id; a disable expires
    /// it with `communication_disabled` (never a fake reply), suppresses the
    /// accepted row durably, and the same wait cannot be resolved again.
    #[tokio::test]
    async fn waits_are_exact_and_expire_on_disable() {
        let mut a = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let generation = a.communication.0.write().route(root, child).unwrap();
        let receipt = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &format!("session:{root}"),
                recipient: &format!("session:{child}"),
                sender_session: Some(root),
                recipient_session: Some(child),
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "question",
            })
            .await
            .unwrap();
        let rx = a
            .wait(root, format!("session:{child}"), receipt.id, generation)
            .unwrap();
        // A different id settles nothing.
        let stray = PeerReceipt {
            id: Uuid::now_v7(),
            ..receipt.clone()
        };
        assert!(!a.settle_wait(&stray));
        assert!(a.waits.contains_key(&receipt.id));

        a.communication.0.write().set_session(root, false).unwrap();
        a.revoke(Some(root)).await;
        let expired = rx.await.unwrap();
        assert_eq!(expired.state, PeerMessageState::Failed);
        assert_eq!(
            expired.error.as_deref(),
            Some(tools::communication::DISABLED)
        );
        assert!(a.waits.is_empty());
        let row = a.store.exchange_receipt(receipt.id).await.unwrap().unwrap();
        assert_eq!(row.state, PeerMessageState::Suppressed);
        assert!(!a.settle_wait(&receipt), "an expired wait is gone for good");
    }

    /// 50 §7: a child's result is one durable exchange keyed by
    /// `(session:<child>, "result:<turn>")`: a repeated finish for the same
    /// turn adds no second row and wakes nobody; a result already answered
    /// through its waiter is recorded consumed without a turn; a finish
    /// without this turn's own text is an error, never another turn's text;
    /// and a result whose child→parent route is off wakes nobody (the child
    /// is still released in the ledger).
    #[tokio::test]
    async fn child_results_are_durably_deduplicated_and_respect_off() {
        let mut a = agents_with(true).await;
        let (p, _) = in_turn(&mut a).await;
        let child = SessionId::new();
        a.session(Some(child), Some((p, "kid")), None, (None, None))
            .await
            .unwrap();
        let finish = |a: &mut Agents, ct: TurnId| {
            a.ledger.entered(TurnEntered {
                session: child,
                turn: Some(ct),
            });
            a.ledger.returned(TurnReturned {
                session: child,
                turn: Some(ct),
            });
        };
        // Answered explicitly during the bound turn: durable row, consumed, no turn.
        let ct = TurnId::new();
        finish(&mut a, ct);
        a.delivered.insert((child, ct), p);
        a.turn_text.insert((child, ct), "final".into());
        a.child_finished(child, ct, "end_turn").await;
        assert_eq!(a.ledger.activity(p), None, "nothing pending on the parent");
        let rows = a.store.pending_exchanges(p, 10).await.unwrap();
        assert!(rows.is_empty(), "consumed without intake: {rows:?}");
        // The same turn finishing again: the dedup key returns the existing
        // row and the parent is not woken.
        a.turn_text.insert((child, ct), "final again".into());
        a.child_finished(child, ct, "end_turn").await;
        assert_eq!(a.ledger.activity(p), None);
        assert!(a.store.pending_exchanges(p, 10).await.unwrap().is_empty());

        // No PeerTurnFinished text for this turn: an error, nothing injected.
        let ct2 = TurnId::new();
        finish(&mut a, ct2);
        a.child_finished(child, ct2, "end_turn").await;
        assert_eq!(a.ledger.activity(p), None);
        assert!(a.store.pending_exchanges(p, 10).await.unwrap().is_empty());

        // Off for the parent: the next result is inspectable but silent.
        a.communication.0.write().set_session(p, false).unwrap();
        let ct3 = TurnId::new();
        finish(&mut a, ct3);
        a.turn_text.insert((child, ct3), "done".into());
        a.child_finished(child, ct3, "end_turn").await;
        assert_eq!(a.ledger.activity(p), None);
        assert!(a.turn_text.is_empty());
    }

    /// A subagent of `p` named `name`, registered as a session.
    async fn kid(a: &mut Agents, p: SessionId, name: &str) -> SessionId {
        let id = SessionId::new();
        a.session(Some(id), Some((p, name)), None, (None, None))
            .await
            .unwrap();
        id
    }

    /// `child`'s turn `ct` entered and returned in the ledger.
    fn child_turn(a: &mut Agents, child: SessionId, ct: TurnId, returned: bool) {
        a.ledger.entered(TurnEntered {
            session: child,
            turn: Some(ct),
        });
        if returned {
            a.ledger.returned(TurnReturned {
                session: child,
                turn: Some(ct),
            });
        }
    }

    /// An accepted request `from → to` with a live waiter on it.
    async fn awaited(
        a: &mut Agents,
        from: SessionId,
        to: SessionId,
    ) -> (Uuid, oneshot::Receiver<PeerReceipt>) {
        let generation = a.communication.0.write().route(from, to).unwrap();
        let receipt = a
            .store
            .accept_exchange(&AcceptExchange {
                sender: &format!("session:{from}"),
                recipient: &format!("session:{to}"),
                sender_session: Some(from),
                recipient_session: Some(to),
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "question",
            })
            .await
            .unwrap();
        let rx = a
            .wait(from, format!("session:{to}"), receipt.id, generation)
            .unwrap();
        (receipt.id, rx)
    }

    /// 50 §6.3, §8: cancelling a request scope reaches the children it
    /// spawned — a running one is marked and its wait expires, and when its
    /// turn later ends the result is recorded `failed` instead of waking
    /// the parent — while a sibling the scope does not own keeps its wait
    /// and still delivers. A child still queued behind the cap never
    /// starts: its ticket is released, nothing is routed.
    #[tokio::test]
    async fn scope_cancel_stops_owned_children_and_drops_their_results() {
        let mut a = agents_with(true).await;
        let (p, _) = in_turn(&mut a).await;
        let scope = Uuid::now_v7();
        let owned = kid(&mut a, p, "owned").await;
        // Spawned by a private turn: nobody's scope owns it.
        let other = kid(&mut a, p, "other").await;
        a.owned_children.insert(
            owned,
            OwnedChild {
                scope,
                cancelled: false,
            },
        );
        let ct = TurnId::new();
        child_turn(&mut a, owned, ct, false);
        let (_, owned_rx) = awaited(&mut a, owned, other).await;
        let (_, other_rx) = awaited(&mut a, other, owned).await;

        a.cancel_peer_scope(p, scope).await.unwrap();
        assert!(a.owned_children[&owned].cancelled);
        assert!(
            !a.owned_children.contains_key(&other),
            "unrelated child untouched"
        );
        let expired = owned_rx.await.unwrap();
        assert_eq!(expired.state, PeerMessageState::Failed);
        assert_eq!(expired.error.as_deref(), Some("cancelled"));
        assert_eq!(a.waits.len(), 1, "the sibling's wait stands");
        drop(other_rx);

        // The cancelled child's turn ends: nothing reaches the parent.
        a.ledger.returned(TurnReturned {
            session: owned,
            turn: Some(ct),
        });
        a.turn_text.insert((owned, ct), "late result".into());
        a.child_finished(owned, ct, "cancelled").await;
        assert!(!a.owned_children.contains_key(&owned));
        assert_eq!(a.ledger.activity(p), None, "no continuation on the parent");
        assert!(a.store.pending_exchanges(p, 10).await.unwrap().is_empty());
        assert!(a.enqueued.is_empty());
        // The same finish again is a durable no-op.
        a.turn_text.insert((owned, ct), "late result".into());
        a.child_finished(owned, ct, "cancelled").await;
        assert_eq!(a.ledger.activity(p), None);

        // The unrelated child still delivers to the parent as before.
        let ot = TurnId::new();
        child_turn(&mut a, other, ot, true);
        a.turn_text.insert((other, ot), "done".into());
        a.child_finished(other, ot, "end_turn").await;
        assert!(
            a.ledger
                .activity(p)
                .is_some_and(|w| w.ends_with(&format!("pending on {p}"))),
            "{:?}",
            a.ledger.activity(p)
        );

        // Queued behind the cap when the scope is cancelled: never starts.
        let queued = kid(&mut a, p, "queued").await;
        a.owned_children.insert(
            queued,
            OwnedChild {
                scope,
                cancelled: false,
            },
        );
        let ticket = a.ledger.admit(queued, InputOrigin::External, None);
        assert!(a.ledger.activity(queued).is_some());
        a.cancel_peer_scope(p, scope).await.unwrap();
        assert!(a.claim_child_turn(queued, ticket).await.is_err());
        assert_eq!(a.ledger.activity(queued), None, "ticket released");
        assert!(!a.owned_children.contains_key(&queued));
        // An unowned child claims a plain, unscoped first turn.
        let plain = kid(&mut a, p, "plain").await;
        let ticket = a.ledger.admit(plain, InputOrigin::External, None);
        assert_eq!(a.claim_child_turn(plain, ticket).await, Ok(None));
    }

    /// 50 §5.3, §7: a child's turn bound to a sibling's awaited request
    /// answers exactly that sibling — the reply row carries the reversed
    /// endpoints the store verifies, the waiter resolves with the text —
    /// and the parent still receives the result as its own uncorrelated
    /// input; only the parent's own answered request suppresses that.
    #[tokio::test]
    async fn bound_reply_goes_to_the_requester_and_the_parent_keeps_its_result() {
        let mut a = agents_with(true).await;
        let (p, _) = in_turn(&mut a).await;
        let child = kid(&mut a, p, "kid").await;
        let sibling = kid(&mut a, p, "sib").await;
        let (request, rx) = awaited(&mut a, sibling, child).await;
        let ct = TurnId::new();
        child_turn(&mut a, child, ct, true);
        a.bound.insert((child, ct), request);
        a.turn_text.insert((child, ct), "the answer".into());
        a.child_finished(child, ct, "end_turn").await;

        let reply = rx.await.unwrap();
        assert_eq!(reply.id, request);
        assert_eq!(reply.state, PeerMessageState::Replied);
        assert_eq!(reply.reply.as_deref(), Some("the answer"));
        assert_eq!(
            reply.answered_by.as_deref(),
            Some(format!("session:{child}").as_str())
        );
        assert!(a.waits.is_empty() && a.bound.is_empty());
        assert!(
            a.store
                .pending_exchanges(sibling, 10)
                .await
                .unwrap()
                .is_empty(),
            "the waiter took the reply; no second intake"
        );
        let why = a.ledger.activity(p).unwrap();
        assert!(why.ends_with(&format!("pending on {p}")), "{why}");

        // Parent as requester: one exchange answers it, nothing else is injected.
        let (request, rx) = awaited(&mut a, p, child).await;
        let ct2 = TurnId::new();
        child_turn(&mut a, child, ct2, true);
        a.bound.insert((child, ct2), request);
        a.turn_text.insert((child, ct2), "for you".into());
        let before = a.ledger.pending_count(p);
        a.child_finished(child, ct2, "end_turn").await;
        assert_eq!(rx.await.unwrap().reply.as_deref(), Some("for you"));
        assert_eq!(a.ledger.pending_count(p), before, "no uncorrelated copy");
    }

    /// 50 §8: the global switch persists before it moves, atomically, and a
    /// write into an unwritable location changes nothing.
    #[test]
    fn global_switch_write_is_atomic_and_line_preserving() {
        let dir = std::env::temp_dir().join(format!("theuth-cfg-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "agent:\n  model: x # keep\n  communication: false\n").unwrap();
        write_config_atomic(&path, "agent", "communication", "true").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "agent:\n  model: x # keep\n  communication: true\n"
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().count() == 1,
            "no temp file left"
        );
        let bad = dir.join("missing").join("config.yaml");
        assert!(write_config_atomic(&bad, "agent", "communication", "true").is_err());
        assert!(!bad.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Ticket id named by a `Ledger::activity` pending-input reason.
    fn pending_ticket(why: &str) -> u64 {
        let mut words = why.split(' ');
        assert_eq!(words.next(), Some("input"), "{why}");
        words.next().unwrap().parse().unwrap()
    }

    /// A certified root in turn `t` with its opener consumed.
    async fn in_turn(a: &mut Agents) -> (SessionId, TurnId) {
        a.ledger.certify();
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let t = TurnId::new();
        let opener = a.ledger.admit(p, InputOrigin::External, None);
        a.ledger.entered(TurnEntered {
            session: p,
            turn: Some(t),
        });
        a.ledger.consumed(InputConsumed {
            session: p,
            id: opener.id,
            turn: t,
        });
        assert_eq!(a.ledger.activity(p), None);
        (p, t)
    }

    /// 47 §3.2: a child's result and a detached job's result each become the
    /// parent's pending input *before* the source stops counting, so no
    /// decision between the two sees zero work; an inline job result is
    /// retired without a second delivery.
    #[tokio::test]
    async fn results_are_pending_input_before_their_source_is_released() {
        let mut a = agents_with(true).await;
        let (p, t) = in_turn(&mut a).await;
        let child = SessionId::new();
        a.session(Some(child), Some((p, "kid")), None, (None, None))
            .await
            .unwrap();
        let ct = TurnId::new();
        a.ledger.entered(TurnEntered {
            session: child,
            turn: Some(ct),
        });
        a.ledger.returned(TurnReturned {
            session: child,
            turn: Some(ct),
        });
        assert!(a.ledger.activity(p).is_some_and(|w| w.contains("busy")));
        a.turn_text.insert((child, ct), "result".into());
        a.child_finished(child, ct, "end_turn").await;
        let why = a.ledger.activity(p).unwrap();
        assert!(why.ends_with(&format!("pending on {p}")), "{why}");
        a.ledger.consumed(InputConsumed {
            session: p,
            id: pending_ticket(&why),
            turn: t,
        });
        assert_eq!(
            a.ledger.activity(p),
            None,
            "child released with the handoff"
        );

        let finished = |op: &tools::Operation, inline: bool| Settle::FinishedWithResult {
            op: op.id,
            result: JobEvent::Finished {
                session: p,
                id: JobId("j".into()),
                command: "echo".into(),
                exit: JobExit::Exited(0),
                secs: 1,
                output: "out".into(),
                inline,
                pty: false,
            },
        };
        let op = |owner: SessionId| tools::Operation {
            id: tools::Activity::untracked(owner, tools::Kind::Command("t")).id(),
            owner,
            kind: tools::Kind::Command("t"),
        };
        let inline = op(p);
        a.ledger.admit_op(&inline);
        a.settle(finished(&inline, true)).await;
        assert_eq!(
            a.ledger.activity(p),
            None,
            "inline: retired, nothing injected"
        );
        let detached = op(p);
        a.ledger.admit_op(&detached);
        a.settle(finished(&detached, false)).await;
        let why = a.ledger.activity(p).unwrap();
        assert!(why.ends_with(&format!("pending on {p}")), "{why}");
        a.ledger.consumed(InputConsumed {
            session: p,
            id: pending_ticket(&why),
            turn: t,
        });
        assert_eq!(
            a.ledger.activity(p),
            None,
            "operation retired under the ticket"
        );
    }

    /// Every `AgentEvent` on a fixture's bus, in order.
    #[derive(kameo::Actor)]
    struct Sink(Arc<parking_lot::Mutex<Vec<AgentEvent>>>);

    impl Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            self.0.lock().push(e);
        }
    }

    /// 61 §5.9: a quiet-job notice for a live op becomes the owner's pending
    /// input (announced as `job j4` with the screen) without retiring the op —
    /// the job keeps running; one for a retired op injects nothing.
    #[tokio::test]
    async fn idle_notice_injects_only_for_a_live_op() {
        let mut a = agents_with(true).await;
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(Arc::clone(&seen)), runtime::subscriber_mailbox());
        a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let (p, t) = in_turn(&mut a).await;
        let op = tools::Operation {
            id: tools::Activity::untracked(p, tools::Kind::Pty(JobId("j4".into()))).id(),
            owner: p,
            kind: tools::Kind::Pty(JobId("j4".into())),
        };
        let notice = |op: &tools::Operation| Notice::JobIdle {
            op: op.id,
            event: JobEvent::Idle {
                session: p,
                id: JobId("j4".into()),
                command: "python3 -q".into(),
                name: None,
                quiet_secs: 61,
                secs: 90,
                nudge: 1,
                user_typed_secs: None,
                output: ">>>\ncursor: row 1 col 5".into(),
            },
        };

        a.notice(notice(&op)).await;
        assert_eq!(a.ledger.activity(p), None, "retired op: nothing injected");

        a.ledger.admit_op(&op);
        a.notice(notice(&op)).await;
        let why = a.ledger.activity(p).unwrap();
        assert!(why.ends_with(&format!("pending on {p}")), "{why}");
        a.ledger.consumed(InputConsumed {
            session: p,
            id: pending_ticket(&why),
            turn: t,
        });
        assert_eq!(
            a.ledger.op_owner(op.id),
            Some(p),
            "the job still runs: its op is not retired by a notice"
        );
        // The bus is a `tell`; a round trip through the sink orders us after it.
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: p,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let delivered: Vec<(String, String)> = seen
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::MessageDelivered {
                    session,
                    from,
                    text,
                } if *session == p => Some((from.clone(), text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            delivered,
            [("job j4".to_owned(), ">>>\ncursor: row 1 col 5".to_owned())],
            "one delivery, for the live op only"
        );
    }

    /// An input no runner ever held (rejected attachment) leaves nothing pending.
    #[tokio::test]
    async fn routing_failure_releases_the_ticket() {
        let mut a = agents().await;
        let (p, _) = in_turn(&mut a).await;
        let r = a.input(
            p,
            None,
            "x".into(),
            vec![PathBuf::from("/nonexistent/theuth.png")],
            InputOrigin::External,
            Delivery::Steer,
        )
        .await;
        assert!(r.is_err(), "the caller learns the drop too (62 §3.3)");
        assert_eq!(a.ledger.activity(p), None);
    }

    /// 62 §3.2: a root whose model is idle but that owns a running PTY is
    /// `Idle, ptys: 1, live: true` — the job, not the model, marks it as
    /// working. (`JobCounts` is fed as a synthetic reply: no `Jobs` actor is
    /// registered in this harness.)
    #[tokio::test]
    async fn activity_counts_a_pty_on_an_idle_root() {
        let mut a = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let counts = HashMap::from([(root, (0u16, 1u16))]);
        let act = a.assemble_activity(&[root], &counts).await;
        let act = &act[&root];
        assert_eq!(act.state, AgentState::Idle);
        assert_eq!((act.jobs, act.ptys, act.children), (0, 1, 0));
        assert!(act.live);
        assert!(act.last_active.is_some(), "a cold row falls back to `updated_at`");
        assert!(act.turn_started_at.is_none());
    }

    /// 62 §3.2: `ListActivity` lists every root, cold or not, and a child only
    /// while it has an actor or a job; a live child counts toward its root.
    #[tokio::test]
    async fn list_activity_keeps_cold_roots_and_drops_cold_children() {
        let mut a = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let (cold_root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let cold_child = SessionId::new();
        let live_child = SessionId::new();
        for (id, name) in [(cold_child, "cold"), (live_child, "live")] {
            a.session(Some(id), Some((root, name)), None, (None, None))
                .await
                .unwrap();
        }
        // Going cold: the actor is gone, the row stays.
        a.sessions.remove(&cold_root);
        a.sessions.remove(&cold_child);

        let rows = a.activity_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        assert!(ids.contains(&root) && ids.contains(&cold_root));
        assert!(ids.contains(&live_child) && !ids.contains(&cold_child));
        let by_id: HashMap<SessionId, &AgentInfo> = rows.iter().map(|r| (r.id, r)).collect();
        assert_eq!((by_id[&root].depth, by_id[&root].parent), (0, root));
        assert_eq!((by_id[&live_child].depth, by_id[&live_child].parent), (1, root));
        assert!(by_id[&root].activity.live && !by_id[&cold_root].activity.live);
        assert_eq!(by_id[&root].activity.children, 1);
        // Pre-order: a child right after its root.
        let at = |id| ids.iter().position(|x| *x == id).unwrap();
        assert_eq!(at(live_child), at(root) + 1);
    }

    /// `ListSessions` stamps each row with the live `running` set: a running
    /// child is `running` while its idle root is not.
    #[tokio::test]
    async fn list_sessions_stamps_running() {
        let mut a = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        a.running.insert(child);
        let rows = a.list_sessions().await;
        let running = |id| rows.iter().find(|r| r.id == id).unwrap().running;
        assert!(running(child) && !running(root));
    }

    /// 62 §0.2, §3.3: `Running` names roots only — a running child is never
    /// swept in; a dry run fills `before` and routes nothing.
    #[tokio::test]
    async fn broadcast_running_selects_roots_and_dry_run_sends_nothing() {
        let mut a = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        // An idle root: live, but not `Running`.
        a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        a.running.insert(root);
        a.running.insert(child);

        let receipt = a
            .broadcast(BroadcastRequest {
                targets: BroadcastTargets::Running,
                text: "stop touching Cargo.lock".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                dry_run: true,
            })
            .await
            .unwrap();
        let ids: Vec<SessionId> = receipt.rows.iter().map(|r| r.session).collect();
        assert_eq!(ids, [root], "running child and idle root excluded");
        let row = &receipt.rows[0];
        assert_eq!(row.before.state, AgentState::Running);
        assert!(row.before.live && row.parent.is_none());
        assert_eq!(
            row.outcome,
            BroadcastOutcome::Skipped {
                reason: "dry run".into()
            }
        );
        assert!(a.last_input.is_empty(), "a dry run routes no input");
    }

    /// 62 §3.3 step 4: a busy recipient queues (with the chosen delivery), an
    /// idle one starts, an unknown id is one failed row, and the loop never
    /// stops early. Above the cap nothing is sent at all.
    #[tokio::test]
    async fn broadcast_rows_queue_start_fail_and_cap() {
        let mut a = agents().await;
        let (busy, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let (idle, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let unknown = SessionId::new();
        a.running.insert(busy);

        let req = BroadcastRequest {
            targets: BroadcastTargets::Sessions {
                ids: vec![unknown, busy, idle, busy],
            },
            text: "hello all".into(),
            files: Vec::new(),
            delivery: Delivery::NextTurn,
            dry_run: false,
        };
        let receipt = a.broadcast(req.clone()).await.unwrap();
        assert_eq!(receipt.rows.len(), 3, "duplicates collapse, order kept");
        assert_eq!(
            receipt.rows[0].outcome,
            BroadcastOutcome::Failed {
                error: "unknown session".into()
            }
        );
        assert!(matches!(
            receipt.rows[1].outcome,
            BroadcastOutcome::Queued {
                delivery: Delivery::NextTurn,
                ..
            }
        ));
        assert_eq!(receipt.rows[2].outcome, BroadcastOutcome::Started);
        assert!(a.last_input.contains_key(&busy) && a.last_input.contains_key(&idle));

        let mut cfg = AgentConfig::default();
        cfg.broadcast.max_targets = 1;
        a.cfg = Arc::new(cfg);
        a.last_input.clear();
        let err = a.broadcast(req).await.unwrap_err();
        assert_eq!(err, "too many targets (2 > 1)");
        assert!(a.last_input.is_empty(), "refused before any send");
    }

    /// A rename or delete that has replied is visible to the very next search:
    /// it may still be `indexing`, but never `ready` from the previous rows.
    /// The actor is not subscribed to its own events here, so only the bump
    /// taken before the reply can make this pass.
    #[tokio::test]
    async fn mutation_reply_precedes_search_staleness() {
        use ::search::PageState;
        let a = agents().await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let id = SessionId::new();
        store.create_session(id, "m", None, None, None).await.unwrap();
        store.set_title(id, "deploy checklist").await.unwrap();

        let page = |q: &'static str| {
            agents.ask(SearchSessionPage {
                query: q.to_owned(),
                limit: None,
            })
        };
        let settled = |q: &'static str| async move {
            for _ in 0..200 {
                let p = page(q).await.unwrap();
                if p.state != PageState::Indexing {
                    return p;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("build never published");
        };
        assert_eq!(settled("deploy").await.hits.len(), 1, "warm index");

        agents
            .ask(RenameSession {
                session: id,
                title: "release notes".into(),
            })
            .await
            .unwrap();
        let next = page("deploy").await.unwrap();
        assert!(
            next.state == PageState::Indexing || next.hits.is_empty(),
            "stale title served after rename replied: {next:?}"
        );
        assert_eq!(settled("release").await.hits.len(), 1);
        assert_eq!(settled("deploy").await.hits.len(), 0);

        agents.ask(DeleteSession { session: id }).await.unwrap();
        let next = page("release").await.unwrap();
        assert!(
            next.state == PageState::Indexing || next.hits.is_empty(),
            "deleted session served after delete replied: {next:?}"
        );
        assert_eq!(settled("release").await.hits.len(), 0);
    }

    /// The handler's `Err(String)` of an `ask` (kameo lifts it into
    /// `SendError::HandlerError`); panics on `Ok` or a transport failure.
    fn refused<T: std::fmt::Debug, M>(r: Result<T, SendError<M, String>>) -> String {
        match r {
            Err(SendError::HandlerError(e)) => e,
            Ok(v) => panic!("expected a refusal, got {v:?}"),
            Err(e) => panic!("send failed: {e}"),
        }
    }

    /// `CreateSession` for a private root through a spawned supervisor. No
    /// tool registry runs in these tests, so the participant barrier is
    /// bypassed (`Agents::registry` → `Ok(None)` under `cfg(test)`); the
    /// barrier itself is covered by the tools crate.
    async fn open_private(
        agents: &ActorRef<Agents>,
        peer: u64,
        closed: CancellationToken,
    ) -> SessionOpen {
        agents
            .ask(CreateSession {
                options: CreateSessionOptions {
                    retention: Retention::Private,
                    project: Some("ops".into()),
                    view: Some("v1".into()),
                    ..Default::default()
                },
                peer: Some((peer, closed)),
            })
            .await
            .unwrap()
    }

    /// 67 §8: a root opened for a remote parent carries its `tools` ceiling
    /// and effort like a local subagent; a plain create carries neither.
    #[tokio::test]
    async fn remote_root_gets_cap_and_effort() {
        let mut a = agents_with(true).await;
        let remote = RemoteParent {
            session: "parent-1".into(),
            engine: Some("Ab3dEf9h".into()),
        };
        let capped = a
            .create_durable(
                None,
                None,
                None,
                Some("high"),
                Some(Risk::ReadOnly),
                Some(remote),
            )
            .await
            .unwrap();
        assert_eq!(a.caps.get(&capped), Some(&Risk::ReadOnly));
        let row = a.store.session(capped).await.unwrap().unwrap();
        assert_eq!(row.effort.as_deref(), Some("high"));
        assert!(row.parent.is_none(), "a remote child is a root here");
        // 67 §8 / 32 §13.1 amended: it is no longer a leaf — `parent` resolves
        // to the conn, and the directory lists that parent first.
        assert_eq!(
            a.resolve_all(capped, &["parent".to_owned()]).await,
            vec![Ok("parent".to_owned())]
        );
        let rows = a.directory(capped).await.unwrap();
        let p = &rows[0];
        assert_eq!((p.address.as_str(), p.kind, p.reply), ("parent", PeerKind::Parent, true));
        assert!(p.name.contains("parent-1") && p.name.contains("Ab3dEf9h"), "{}", p.name);
        assert!(a.peer_context(capped).contains("address it as `parent`"));
        // Its `send_message{to: "parent"}` is accepted as an uplink; a reply
        // cannot be awaited across the conn.
        let deliver = |await_reply| Deliver {
            from: capped,
            to: "parent".into(),
            text: "done".into(),
            await_reply,
            reply_to: None,
            request_id: None,
            admitted: CancellationToken::new(),
        };
        let d = a.uplink(&deliver(false), "done".into()).await.unwrap();
        assert_eq!((d.receipt.to.as_str(), d.receipt.state), ("parent", PeerMessageState::Accepted));
        assert!(d.reply.is_none());
        assert!(a.uplink(&deliver(true), "done".into()).await.unwrap_err().contains("await_reply"));

        let plain = a
            .create_durable(None, None, None, None, None, None)
            .await
            .unwrap();
        assert!(!a.caps.contains_key(&plain));
        let row = a.store.session(plain).await.unwrap().unwrap();
        assert_eq!(row.effort, a.default_effort);
        assert!(a.resolve_all(plain, &["parent".to_owned()]).await[0].is_err());
        assert!(a.directory(plain).await.unwrap().iter().all(|p| p.address != "parent"));
    }

    /// 51 §4.1–§4.2: a private root needs a view and an owning peer; once
    /// open it authorizes only that peer, is absent from the durable list
    /// and present in `PrivateTrees`, and its state reads through its own store.
    #[tokio::test]
    async fn private_root_is_owned_and_unlisted() {
        let agents = Agents::spawn(agents().await);
        let private = |view: Option<&str>| CreateSessionOptions {
            retention: Retention::Private,
            project: None,
            view: view.map(str::to_owned),
            ..Default::default()
        };
        let no_peer = agents
            .ask(CreateSession {
                options: private(Some("v1")),
                peer: None,
            })
            .await;
        assert_eq!(refused(no_peer), "a private session needs an RPC peer");
        let no_view = agents
            .ask(CreateSession {
                options: private(None),
                peer: Some((7, CancellationToken::new())),
            })
            .await;
        assert_eq!(refused(no_view), "a private session needs a view");

        let open = open_private(&agents, 7, CancellationToken::new()).await;
        assert_eq!(open.retention, Retention::Private);
        assert_eq!(open.session, open.root);
        assert_eq!(open.lease_secret.as_deref().map(str::len), Some(64));

        let authorize = |session, peer| agents.ask(Authorize { session, peer });
        let auth = authorize(open.root, Some(7)).await.unwrap();
        assert_eq!(
            (auth.root, auth.retention, auth.owner_peer),
            (open.root, Retention::Private, Some(7))
        );
        assert!(auth.resources.is_private());
        assert_eq!(auth.generation, 1);
        for peer in [Some(8), None] {
            assert_eq!(refused(authorize(open.root, peer).await), "unauthorized");
        }
        assert_eq!(
            refused(authorize(SessionId::new(), Some(7)).await),
            "no such session"
        );
        // A durable root: any peer, the durable capability, listed with its project.
        let durable = agents
            .ask(CreateSession {
                options: CreateSessionOptions {
                    project: Some("ops".into()),
                    ..CreateSessionOptions::default()
                },
                peer: None,
            })
            .await
            .unwrap();
        assert!(durable.lease_secret.is_none());
        let auth = authorize(durable.root, Some(99)).await.unwrap();
        assert_eq!(
            (auth.retention, auth.generation, auth.owner_peer),
            (Retention::Durable, 0, None)
        );

        let listed = agents.ask(ListSessions).await.unwrap();
        assert!(
            listed
                .iter()
                .any(|s| s.id == durable.root && s.project.as_deref() == Some("ops"))
        );
        assert!(!listed.iter().any(|s| s.id == open.root), "private root listed");
        assert_eq!(
            agents.ask(PrivateTrees).await.unwrap(),
            vec![(open.root, open.root, 7)]
        );
        // Session-scoped reads route to the tree's store: the private row
        // answers from the in-memory store, and the subtree listing is empty.
        agents.ask(GetTodos { session: open.root }).await.unwrap();
        assert!(
            agents
                .ask(ListAgents { session: open.root })
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 51 §4.4: only the holder may leave; a left tree is revoked at once,
    /// fails `Authorize` closed, and input to its id recreates nothing durable.
    #[tokio::test]
    async fn leave_revokes_and_never_recreates() {
        let a = agents().await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let open = open_private(&agents, 7, CancellationToken::new()).await;
        let root = open.root;
        let leave = |peer| {
            agents.ask(LeaveSession {
                session: root,
                lease_secret: None,
                peer,
            })
        };
        assert_eq!(refused(leave(8).await), "unauthorized");
        let left = leave(7).await.unwrap();
        assert!(left.revoked);
        assert_eq!(left.cleanup, CleanupStatus::Pending);

        let e = refused(
            agents
                .ask(Authorize {
                    session: root,
                    peer: Some(7),
                })
                .await,
        );
        assert!(e.starts_with("session clos"), "{e}");
        // Still listed while closing: a subscriber seeded now must filter its
        // closing/deleted events (RPC filter map is insert-only).
        assert_eq!(agents.ask(PrivateTrees).await.unwrap(), vec![(root, root, 7)]);
        assert_eq!(refused(leave(7).await), "session closed");
        // Input to the closed id is refused, not turned into a durable row.
        agents
            .ask(SendInput {
                session: Some(root),
                text: "hello".into(),
                files: Vec::new(),
                delivery: Delivery::default(),
            })
            .await
            .unwrap();
        assert!(store.session(root).await.unwrap().is_none());
        assert!(
            !agents
                .ask(ListSessions)
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == root)
        );
        // Teardown ran off-mailbox; the tree is gone for good.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(agents.ask(PrivateTrees).await.unwrap().is_empty());
    }

    /// 51 §4.3: `resume_private` needs the view and the current secret,
    /// rotates it and rebinds the owner. Rebinding cancels the old owner's
    /// watcher (its dead connection no longer closes the root); the new
    /// owner's does after the grace.
    #[tokio::test]
    async fn resume_rebinds_and_cancels_old_watcher() {
        let agents = Agents::spawn(agents().await);
        let old_closed = CancellationToken::new();
        let open = open_private(&agents, 7, old_closed.clone()).await;
        let (root, secret) = (open.root, open.lease_secret.unwrap());
        let resume = |view: &str, secret: &str, peer: u64, closed: CancellationToken| {
            agents.ask(ResumePrivate {
                session: root,
                view: view.to_owned(),
                lease_secret: secret.to_owned(),
                peer: (peer, closed),
            })
        };
        let authorize = |peer| {
            agents.ask(Authorize {
                session: root,
                peer: Some(peer),
            })
        };
        assert_eq!(
            refused(resume("v1", "nope", 9, CancellationToken::new()).await),
            "unauthorized"
        );
        assert_eq!(
            refused(resume("v2", &secret, 9, CancellationToken::new()).await),
            "unauthorized"
        );
        let new_closed = CancellationToken::new();
        let rebound = resume("v1", &secret, 9, new_closed.clone()).await.unwrap();
        let rotated = rebound.lease_secret.unwrap();
        assert_ne!(rotated, secret);
        assert_eq!(
            refused(resume("v1", &secret, 9, CancellationToken::new()).await),
            "unauthorized",
            "old secret still accepted"
        );
        authorize(9).await.unwrap();
        assert_eq!(refused(authorize(7).await), "unauthorized");
        assert_eq!(agents.ask(PrivateTrees).await.unwrap(), vec![(root, root, 9)]);

        // The old connection dies well past the (200 ms) grace: the root stays.
        old_closed.cancel();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(agents.ask(PrivateTrees).await.unwrap(), vec![(root, root, 9)]);
        // The holder's connection dies: the root closes itself after the grace.
        new_closed.cancel();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(agents.ask(PrivateTrees).await.unwrap().is_empty());
        assert!(refused(authorize(9).await).starts_with("session clos"));
    }

    // ---- 69 P: peer messages follow the owner --------------------------------

    /// Two engines `a` and `b` over one loopback switch, each holding both
    /// rows: `a_root` owned by `a`, `b_root` owned by `b`, both `owner`
    /// scope (the rows are seeded through a store opened as the owning
    /// engine, then re-opened as this one — the shape a replica holds).
    struct TwoEngines {
        a: Agents,
        b: Agents,
        a_root: SessionId,
        b_root: SessionId,
        net: Vec<Arc<peer_mesh::loopback::Loop>>,
    }

    async fn seed(dir: &Path, as_engine: &str, id: SessionId) {
        let s = Store::open(&dir.join("sessions.sqlite"), Some(mesh::EngineId(as_engine.into())))
            .await
            .unwrap();
        s.create_session(id, "m", None, None, None).await.unwrap();
        s.set_root_scope(id, PeerScope::Owner, None).await.unwrap();
        s.close().await;
    }

    async fn two_engines() -> TwoEngines {
        let base = std::env::temp_dir().join(format!("theuth-69p-{}", Uuid::now_v7()));
        let (dir_a, dir_b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let (a_root, b_root) = (SessionId::new(), SessionId::new());
        for dir in [&dir_a, &dir_b] {
            seed(dir, "a", a_root).await;
            seed(dir, "b", b_root).await;
        }
        let net = peer_mesh::loopback::nodes(&["a", "b"]);
        let mut a = agents_on(dir_a, Some("a"), true).await;
        let mut b = agents_on(dir_b, Some("b"), true).await;
        a.transport = Some(net[0].clone());
        b.transport = Some(net[1].clone());
        for x in [&mut a, &mut b] {
            x.mirror(a_root).await;
            x.mirror(b_root).await;
        }
        TwoEngines {
            a,
            b,
            a_root,
            b_root,
            net,
        }
    }

    /// The owner side of `theuth/rpc/1` for one engine, as `rpc::forward::serve`
    /// + `api::peer_deliver` do it: one request line per stream, dispatched
    /// into the supervisor's [`Agents::peer_deliver`], one response line.
    fn serve_peer_deliver(t: Arc<peer_mesh::loopback::Loop>, agents: Arc<tokio::sync::Mutex<Agents>>) {
        use replica::Transport;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        tokio::spawn(async move {
            while let Ok((_, stream)) = t.accept(replica::ALPN_RPC).await {
                let agents = agents.clone();
                tokio::spawn(async move {
                    let (read, mut write) = tokio::io::split(stream);
                    let mut line = String::new();
                    BufReader::new(read).read_line(&mut line).await.unwrap();
                    let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(req["method"], peer_mesh::METHOD);
                    let session: SessionId = serde_json::from_value(req["params"][0].clone()).unwrap();
                    let envelope: PeerEnvelope = serde_json::from_value(req["params"][1].clone()).unwrap();
                    let rp = match agents.lock().await.peer_deliver(PeerDeliver { session, envelope }).await {
                        Ok(r) => serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": r}),
                        Err(e) => serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": e}}),
                    };
                    write.write_all(format!("{rp}\n").as_bytes()).await.unwrap();
                    write.flush().await.unwrap();
                });
            }
        });
    }

    fn deliver(from: SessionId, to: SessionId, text: &str, await_reply: bool, reply_to: Option<Uuid>) -> Deliver {
        Deliver {
            from,
            to: format!("session:{to}"),
            text: text.into(),
            await_reply,
            reply_to,
            request_id: None,
            admitted: CancellationToken::new(),
        }
    }

    /// A `Deliver` to a row owned elsewhere is forwarded to the owner, whose
    /// intake records the exchange under the sender's id; the sender's own
    /// row stays `accepted` and nothing local is woken. The reply travels
    /// the other way through the second forwarder and settles the wait.
    #[tokio::test]
    async fn peer_messages_follow_the_owner_and_replies_round_trip() {
        let TwoEngines {
            a,
            b,
            a_root,
            b_root,
            net,
        } = two_engines().await;
        let (a, b) = (Arc::new(tokio::sync::Mutex::new(a)), Arc::new(tokio::sync::Mutex::new(b)));
        serve_peer_deliver(net[0].clone(), a.clone());
        serve_peer_deliver(net[1].clone(), b.clone());

        let d = a
            .lock()
            .await
            .deliver(deliver(a_root, b_root, "question", true, None), None)
            .await
            .unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Accepted);
        assert_eq!(d.receipt.to, format!("session:{b_root}"));
        let id = d.receipt.id;
        {
            let a = a.lock().await;
            assert!(!a.enqueued.contains(&id), "nothing here consumes a forwarded message");
            assert!(a.sessions.get(&b_root).is_none(), "the remote row was not woken here");
            assert!(a.waits.contains_key(&id), "the wait is registered as for a local recipient");
            assert_eq!(a.store.exchange_receipt(id).await.unwrap().unwrap().state, PeerMessageState::Accepted);
        }
        // The owner's intake: same id, reversed view, its own row.
        let theirs = b.lock().await.store.peer_record(b_root, id).await.unwrap().unwrap();
        assert_eq!(theirs.from, format!("session:{a_root}"));
        assert_eq!(theirs.text, "question");
        assert_ne!(theirs.receipt.state, PeerMessageState::Failed);

        // b answers: forwarded to a, whose intake settles the wait.
        let r = b
            .lock()
            .await
            .deliver(deliver(b_root, a_root, "42", false, Some(id)), None)
            .await
            .unwrap();
        assert_eq!((r.receipt.state, r.receipt.reply_to), (PeerMessageState::Accepted, Some(id)));
        let fin = d.reply.unwrap().await.unwrap();
        assert_eq!((fin.id, fin.state), (id, PeerMessageState::Replied));
        assert_eq!(fin.reply.as_deref(), Some("42"));
        assert_eq!(fin.answered_by.as_deref(), Some(format!("session:{b_root}").as_str()));
        assert!(a.lock().await.waits.is_empty());
    }

    /// No owner on the wire: the receipt is `failed` with 68 F's error data
    /// shape (the words the desktop card shows), durably; no wait is kept.
    #[tokio::test]
    async fn an_unreachable_owner_is_a_failed_receipt_in_the_front_door_shape() {
        let TwoEngines {
            mut a,
            a_root,
            b_root,
            ..
        } = two_engines().await;
        // A switch on which `b` is not connected.
        a.transport = Some(peer_mesh::loopback::nodes(&["a"]).remove(0));
        let d = a.deliver(deliver(a_root, b_root, "hello?", true, None), None).await.unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Failed);
        assert_eq!(
            d.receipt.error.as_deref(),
            Some(r#"{"OwnerUnreachable":{"owner":"b","last_seen":null}}"#)
        );
        assert!(d.reply.is_none());
        assert!(a.waits.is_empty());
        assert_eq!(
            a.store.exchange_receipt(d.receipt.id).await.unwrap().unwrap().state,
            PeerMessageState::Failed
        );
        // The owner's intake refuses the row it does not own (never a take).
        let e = a
            .peer_deliver(PeerDeliver {
                session: b_root,
                envelope: PeerEnvelope {
                    id: Uuid::now_v7(),
                    from: a_root,
                    text: "x".into(),
                    reply_to: None,
                    request_id: None,
                },
            })
            .await
            .unwrap_err();
        assert!(e.contains("owned by b"), "{e}");
    }

    /// `discover_peers`: a row owned elsewhere is `remote` with its owner's
    /// engine name and no activity; ours carry this engine's name.
    /// `active_only` drops remote rows (their activity is unknown here).
    #[tokio::test]
    async fn directory_marks_rows_owned_elsewhere_as_remote() {
        let TwoEngines {
            mut a,
            mut b,
            a_root,
            b_root,
            ..
        } = two_engines().await;
        let mut rows = a.directory(a_root).await.unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.address, format!("session:{b_root}"));
        assert_eq!((r.state, r.relationship), (PeerState::Remote, PeerRelationship::Root));
        assert_eq!(r.engine.as_deref(), Some("b"), "no mesh actor: the id itself (short)");
        assert!(r.activity.is_none());
        arrange(&mut rows, true);
        assert!(rows.is_empty(), "remote activity is unknown: active_only keeps none");
        // Seen from b, a's root is the remote one.
        let rows = b.directory(b_root).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, format!("session:{a_root}"));
        assert_eq!((rows[0].state, rows[0].engine.as_deref()), (PeerState::Remote, Some("a")));
    }

    /// A mesh of one: every row is ours, so `Deliver` takes today's local
    /// path — the exchange is enqueued here, no transport is consulted — and
    /// the directory carries this engine's name on every row.
    #[tokio::test]
    async fn a_lone_engine_delivers_locally_as_before() {
        let mut a = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        assert_eq!(a.remote_owner(child).await, None);
        let d = a.deliver(deliver(root, child, "hi", false, None), None).await.unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Accepted);
        assert!(a.enqueued.contains(&d.receipt.id), "queued for the local recipient");
        assert!(a.transport.is_none(), "no mesh was consulted");
        let rows = a.directory(root).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_ne!(rows[0].state, PeerState::Remote);
        assert_eq!(rows[0].engine.as_deref(), Some(platform::process::hostname()));
    }
}
