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
use std::time::{Duration, Instant};

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
    InstallCommunicationAccess, JOBS_ACTOR, JobEvent, JobExit, JobId, JobKind, Jobs, ListPrompts,
    ListTools, Notice, REGISTRY_ACTOR, RearmReminder, RegisterActivity, RegisterPrompt,
    RegisterTool, ReminderEvent, RestoreApprovals, RestoreHidden, Risk, SetCap, SetParent,
    SetPromoteAfter, Settle, ToolRegistry,
};
use uuid::Uuid;

use crate::communication::{
    AnswerPeerRequest, ChannelOrigin, CommunicationChanged, CommunicationState,
    DISCOVER_DEFAULT_LIMIT, DISCOVER_MAX_LIMIT, Generation, GetCommunication, GetPeerMessage,
    ListPeerMessages, ListPeers, MeshRefusal, PEER_TEXT_MAX, PeerActivity, PeerAddress,
    PeerConsumed, PeerDeliver, PeerDeliverError, PeerDelivered, PeerEnvelope, PeerInfo,
    PeerIntakeFailed, PeerKind, PeerLiveState, PeerMessageChanged, PeerMessagePage,
    PeerMessageState, PeerOrigin, PeerPage, PeerReceipt, PeerRecord, PeerRelationship, PeerScope,
    PeerState, PeerTurnFinished, PeerUplink, Policy, PolicyError, SetCommunication, SetPeerScope,
    SharedPolicy, WAITS_PER_HARNESS, WAITS_PER_SESSION, mesh as peer_mesh, network,
};

use crate::CostService;
use crate::acp_child::{AcpChild, CancelObserved};
use crate::context::{
    ContextBreakdown, ContextEntry, ContextHistory, ContextOwner, ContextPiece, ContextRemoval,
    ContextSnapshot, ContextTarget, save_blob,
};
use crate::continuation::{
    DecideYield, InputConsumed, InputDropped, InputOrigin, InputTicket, Ledger, SessionUnknown,
    TurnEntered, TurnReturned, YieldDecision,
};
use crate::endless::{
    Attached, Endless, EndlessConfig, MemoryMode, SessionMemory, SetMode, UnitFilter, ViewLine,
};
use crate::lifecycle::{
    Authorize, Authorized, CleanupStatus, CloseResult, CreateSession, LeaveSession, PrivateOwner,
    PrivateRoot, PrivateTrees, ResumePrivate, SessionOpen,
};
use crate::loop_::{
    CancelPeerScope, CancelTurn, CatalogChanged, CompactNow, CompactionOverride,
    ContextBreakdownNow, ContextHistoryNow, ContextItemsNow, GetToolset, HarvestNow, IsRunning,
    PEER_INTAKE, PeerInput, QueuedInputs, QuietInput, RedoTurn, RemoveContextNow, ResetRuntimeNow,
    RestartTurn, ResumedTurn, Session, SessionState, SetBudgetNow, SetCompactionOverride,
    SetMemoryNow, SetPendingEffort, SetTalking, SetTitlePin, SetToolsetOverride, SuppressPeerInputs,
    SwitchModel, TrimTurn, UndoTurn, UserInput, WithdrawInput, prompt_blocks, text_of,
};
use crate::peers::{
    DiscoverPeersTool, ListModelsTool, Nodes, PeerMessageTool, SendMessageTool, SpawnSubagentTool,
    WaitPeerTool, node_lines,
};
use crate::private::Scratch;
use crate::skills::{
    self, AddSkillRepoTool, AddSkillTool, LoadSkillTool, RemoveSkillRepoTool, RemoveSkillTool,
    SyncSkillsTool,
};
use crate::store::communication::{AcceptExchange, CommError};
use crate::store::{self, Epoch, Store};
use crate::user::{MessageUserTool, USER_ACTOR, UserChannels};
use crate::{
    AGENT_ACTOR, AddAcp, AddProvider, AddSkill, AddSkillRepo, AgentConfig, AgentEvent, AgentInfo,
    AgentState, AncestorInfo, Ancestors, ArchiveSession, BroadcastOutcome, BroadcastReceipt,
    BroadcastRequest, BroadcastRow, BroadcastTargets, Cancel, CancelScope, Compact, CompactStyle,
    ContextItems, DeleteSession, Delivery, DeskSession, EditTurn, EscalateModel, ForkSession,
    GetCompactStyle, GetDefaultEffort, GetDefaultModel, GetEscalateModel, GetMaxSalvages,
    GetMaxToolRounds, GetStreamIdle, GetSubagentModel, GetSubagentTimeout, GetTitleModel, GetTodos,
    GetVisionModel, Harvest, HistoryEntry, ListAcp, ListAgents, ListLogins, ListModels,
    ListProviders, ListQueued, ListRegistry, ListSessions, ListSkills, LoadSession, LoadSkill,
    Loaded, Login, LoginFinished, LoginInfo, Logout, MarkDesk, ModelInfo, NewRootSession,
    NewSession, PinSession, PinTitle, ProviderInfo, QueuedInput, ReadSession, Redo,
    RefreshProviderModels, RefreshedModels, Regenerate, RemoveAcp, RemoveContext, RemoveProvider,
    RemoveSkill, RemoveSkillRepo, RenameSession, ResetRuntime, ResolveModel, SearchRefresh,
    SearchSessionPage, SendInput, SessionActivity, SessionContextBreakdown, SessionModel,
    SessionRead, SessionSummary, SetCompactStyle, SetCompaction, SetDeskModel, SetEffort,
    SetEscalateModel, SetMaxSalvages, SetMaxToolRounds, SetModel, SetStreamIdle, SetSubagentModel,
    SetSubagentTimeout, SetTalk, SetTitleModel, SetToolset, SetVisionModel, ShowPrompt, SideCall,
    SideCallable, SideReply, SkillBody, SkillMeta, Steer, SyncSkills, TALK_COALESCE_MS,
    TALK_PARK_CAP_MS, TalkLive, TrimAssistant, Undo, UseSkill, UserRenameSession, Withdraw,
    effort_of,
};
use crate::{
    AdvisorConfig, DurableStore, GetPolishModel, ModelRole, Polish, PolishReply, Rewrite,
    SessionCostQuery, SessionKind, SetPolishModel, SpendReportQuery, SubagentsConfig,
    UnloadSession, cost::CallKind, polish,
};
use crate::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
use crate::{
    EndlessConfigChanged, EndlessFold, EndlessPause, EndlessRecall, EndlessRetry, EndlessZoom,
    GetEndlessSettings, RenderMemoryView, SessionMemoryQuery, SetAgentCompactorModel,
    SetEndlessSettings, SetSessionAgentTail, SetSessionCompactorModel, SetSessionMemory,
    SetSessionTail, SetSessionViewBytes,
};
use crate::{GetFavoriteModels, SetModelFavorite};
use crate::{GetSubagentEffort, SetDeskEffort, SetSubagentEffort, ValidateEffort};
use crate::{ListProjectStyles, SetProjectStyle, SetSessionProject, project};
use crate::{
    LocalServer, ProbeLocalServers, ProviderModel, ProviderModels, SetModelLimits, SummaryTest,
    TestSummaryCall,
};
use crate::{SessionEffort, SetSessionCompactorEffort};
use crate::{attach, clock, frame, repl, search, spill};
use tools::{
    CloseTree, CloseTreeAll, OpenTree, ParticipantsReady, Retention, SessionResources, TreeOpened,
};

// 109: handler bodies in their own files so the waves do not collide.
mod acp_resume; // 109 W4
mod activity; // 155 W1
pub(crate) mod advisor; // 125 W1
pub mod child_waits; // 139 W3
mod drain;
pub mod pause; // 179
mod resume;
pub mod transfer; // 139 W2
mod view; // 176 E1
pub(crate) use view::observed_state; // 208: the graph's node state
mod waits;
pub use acp_resume::LastBootPass; // 146 D4
pub use drain::{Drain, DrainReport, OpenTurns, drain_engine, draining}; // 109 W5
pub use pause::{
    HeldDesks, PauseReport, PauseTree, ReleaseTree, ResumeReport, ResumeTree, TreeSuspended,
}; // 179
pub use resume::{OpenTurn, ResumeTurn, TakeoverPending};
pub use transfer::{DrainSession, SessionDrainReport}; // 139 C
// 109 W3: what a resumed human-wait / peer-wait call asks the supervisor for.
pub(crate) use waits::{ResumeQuestion, ResumeWait, remaining};

/// The supervisor actor. Look up with `ActorRef::<Agents>::lookup(AGENT_ACTOR)`.
#[derive(kameo::Actor)]
pub struct Agents {
    pub(crate) rt: Runtime,
    cfg: Arc<AgentConfig>,
    /// `cfg.model` as last set by [`SetModel`]; new sessions start on it.
    default_model: String,
    /// `cfg.effort` as last set by [`SetEffort`]; new sessions start on it.
    default_effort: Option<String>,
    /// `cfg.favorite_models` as last set by [`SetModelFavorite`] (127 §1), in starring order.
    favorite_models: Vec<String>,
    /// Live default compaction style; cloned into each session so [`SetCompactStyle`] is in-process.
    compact_style: Arc<RwLock<CompactStyle>>,
    /// Live title model (07 §15); `None` = each session's own model. Shared with every session.
    title_model: Arc<RwLock<Option<Resolved>>>,
    /// Live polish model (153 §9.2, `voice.polish_model`); `None` = the title model, then
    /// each session's own model. Shared with every session once A5 wires `ModelRole::Polish`.
    polish_model: Arc<RwLock<Option<Resolved>>>,
    /// Live `voice.polish_enabled` (153 §9, 204); `false` = `Polish` never calls a model.
    polish_enabled: bool,
    /// Live subagent default model (`agent.subagents.model`); `None` = `agent.model`.
    /// Children spawned without `model` use it; the system prompt names it. Shared with every session.
    subagent_model: Arc<RwLock<Option<Resolved>>>,
    /// 194 row 3: live subagent default effort (`agent.subagents.effort`); `None` =
    /// `agent.effort`. Children spawned without `effort` use it; the system prompt names
    /// it. Shared with every session.
    subagent_effort: Arc<RwLock<Option<String>>>,
    /// 148 E2: desk session → its agent's configured model ([`SetDeskModel`]);
    /// `None` = the agent has none. A native child of a desk starts on it
    /// before `subagent_model` and `default_model`.
    desk_models: HashMap<SessionId, Option<String>>,
    /// 194: desk session → its agent's configured effort ([`SetDeskModel`]); a native
    /// child of the desk spawned without `effort` starts on it before `subagent_effort`.
    desk_efforts: HashMap<SessionId, String>,
    /// Live vision fallback model (52 §3.1); `None` = no fallback. Shared with
    /// every native session once the loop consults it; snapshot, never hold
    /// the lock across an await.
    pub(crate) vision_model: Arc<RwLock<Option<Resolved>>>,
    /// Live tier-2 model of the decision oracle (57 §0.9); `None` = the title
    /// model, then each session's own model. Shared with every session.
    escalate_model: Arc<RwLock<Option<Resolved>>>,
    /// Live `agent.knowledge_harvester_model`: the knowledge harvest's default when a
    /// session has no own pick; `None` = each session's own model. Shared with every session.
    knowledge_harvester_model: Arc<RwLock<Option<Resolved>>>,
    /// 194: the live title/vision/escalate/polish efforts (`agent.title_effort`,
    /// `agent.vision_effort`, `agent.escalate_effort`, `voice.polish_effort`). Shared with every session.
    role_efforts: Arc<parking_lot::RwLock<crate::loop_::RoleEfforts>>,
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
    /// 160: the in-memory cost cube over the durable store.
    costs: CostService,
    client: Client,
    auth: Auth,
    sessions: HashMap<SessionId, Live>,
    /// Sessions with a turn in flight (from `TurnStarted`/`TurnFinished`).
    running: HashSet<SessionId>,
    /// 146: the boot pass's ACP respawn queue and counts, until settled.
    boot: Option<acp_resume::Boot>,
    /// 146 D4: the last `BootPass` published, for a late `LastBootPass` ask.
    last_boot: Option<crate::durability::BootPass>,
    /// 139 D16: sessions mid-release-drain → the drain's reason; `route`
    /// refuses their inputs (`transfer::draining_error`) until the demote
    /// lands or the hold expires (`transfer::ClearDraining`).
    pub(crate) draining_sessions: HashMap<SessionId, &'static str>,
    /// 139 §6.3 (D5): sessions a system takeover left `ResumePending` →
    /// the engine taken from; the user's `resume_turn` (boot-rule cause,
    /// no `suspended` row) resumes them as `Takeover{from}` and runs the
    /// §7 jobs / child-waits pass instead of reading as a `crash`.
    pending_takeovers: HashMap<SessionId, String>,
    /// 179: paused agents' desk trees, by desk → the inputs that arrived
    /// for any session of the tree meanwhile, in arrival order (`route`,
    /// `enqueue_peer`); replayed by `pause::ResumeTree`.
    paused: HashMap<SessionId, Vec<(SessionId, Held)>>,
    /// 179: desks whose pause is still draining → its drain deadline and the
    /// un-pauses that came meanwhile; answered once the drained actors are
    /// stopped, or `pause::UNPAUSE_GRACE` past the deadline.
    pausing: HashMap<SessionId, pause::Pausing>,
    /// 179: ACP children a pause stopped mid-turn whose `cancelled` end has
    /// not been seen yet (`pause::cut_by_pause`); cleared by the child's next
    /// turn start, or by the un-pause when nothing re-prompts it.
    acp_cut: HashSet<SessionId>,
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
    /// 183 §6.3: receipts whose network leg is in flight, with the retry waiters of the same
    /// `request_id` attached to it; they are answered with the leg's settled outcome.
    forwarding: HashMap<Uuid, Vec<kameo::reply::ReplySender<Result<Delivered, String>>>>,
    /// Child-work permits per orchestrator tree, keyed by the tree's root
    /// session (01 §2.6): created on demand, never shrunk. A session's own
    /// loop takes peer-opened turns and Python cells from its tree's
    /// `native` pool (50 §8).
    pools: HashMap<SessionId, TreePools>,
    /// Every running child turn: `started_at` always; permit + runtime timer
    /// only for the spawn turn started by [`SpawnChild`].
    child_turns: HashMap<SessionId, ChildTurn>,
    /// Children whose spawn turn waits for a pool permit (01 §2.6): the
    /// brief and every input that arrived for them meanwhile, in memory only.
    queued: HashMap<SessionId, QueuedSpawn>,
    /// Child results a parent's intake refused (R2.7), by parent: offered
    /// once more when that parent's next turn ends. In memory only.
    undelivered: HashMap<SessionId, Vec<Undelivered>>,
    /// Children whose current turn was cancelled by the runtime cap.
    timed_out: HashSet<SessionId>,
    /// 178 F2: arms handed out so far ([`ChildTurn::arm`]).
    arms: u64,
    /// 178 W6: the last `AgentEvent::Error` of a child turn, for the
    /// `[child turn ended: error — …]` prefix of its result.
    child_errors: HashMap<(SessionId, TurnId), String>,
    /// Children spawned with a `tools` ceiling ([`SpawnChild::cap`]), for
    /// the board row; the registry enforces it ([`SetCap`]).
    pub(crate) caps: HashMap<SessionId, Risk>,
    /// The `model` a harness child's spawn asked for, its roster model until
    /// the harness reports `harness_model` (this process only, like `caps`).
    pub(crate) requested_models: HashMap<SessionId, String>,
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
    /// Input ids routed by this process (idempotent retries): id -> (session, when).
    // ponytail: process-local, lost on restart; persist beside the row if retries must survive engine restarts.
    seen: HashMap<InputId, (SessionId, Instant)>,
    /// 145 §0.3: sessions with a live talk — its spoken window and what it parked.
    talk: HashMap<SessionId, Talk>,
    /// L1 (171 inv. 3): user-cell kernel jobs, fed by [`CellJobWatch`].
    cell_jobs: Arc<CellJobs>,
    /// Unanswered approvals per session: `(id, tool)` (design doc 19 §2.1).
    approvals: HashMap<SessionId, (ApprovalId, String, u64)>,
    /// 176 A1: ACP children this process parked idle → when; taken by the
    /// resume (`acp_session`) for its log line.
    parked: HashMap<SessionId, Instant>,
    /// 176 A1: ACP children with a park probe in flight → the inputs
    /// `route` / `enqueue_peer` held for them until [`ParkRound`].
    parking: HashMap<SessionId, Vec<Held>>,
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
    pub(crate) ledger: Ledger,
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
    /// 183 §6.5: `(message id, chain)` of the forwarded message [`Self::peer_deliver`] is
    /// taking in, for `enqueue_peer`'s chain bound; set and cleared around that one intake.
    inbound_chain: Option<(Uuid, u32)>,
    /// Who a session speaks as when it is not theuth (70 DESK-PROTOCOL): the
    /// roster installs it with [`InstallIdentitySource`]. `None`: every
    /// session is theuth's.
    pub(crate) identity: Option<Arc<dyn IdentitySource>>,
    /// 191 (D2): the flag the a2a plugin sets once the mesh latch and its first contact
    /// reconcile are done ([`crate::communication::network::CONTACTS_READY`]; a test's own).
    /// Read, never asked for: the agent never waits on the a2a actor.
    pub(crate) contacts_ready: Arc<std::sync::atomic::AtomicBool>,
    /// 125 §4.2: ids of advisor sessions seen by this process — never a
    /// peer, told nothing about peers, speaking as the advisor.
    pub(crate) advisors: HashSet<SessionId>,
    /// 125 §6.5: the live `advisor:` section (the remembered pick and the
    /// three tuning keys), rewritten by the pickers and the setters.
    pub(crate) advisor_cfg: AdvisorConfig,
    /// 137: the endless memory service (one per engine); every native
    /// session attaches to it and the loop, tools and RPC call through it.
    pub(crate) endless: Endless,
    /// 155 §4.2 / §5.2: open stall episodes of ACP children and registered
    /// `wait_peer`s.
    pub(crate) activity: crate::activity::Monitor,
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
    /// 130 E2: ms since epoch the wait began (`SessionActivity.waiting_since_ms`).
    since_ms: u64,
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
pub(crate) enum LiveActor {
    Native(ActorRef<Session>),
    Acp(ActorRef<AcpChild>),
}

/// The refusal for every `Session`-only command on an ACP child.
const NOT_NATIVE: &str = "not available for an external harness";
/// 141 §5.4: the refusal for anything that would drive or message a
/// watch-only child (`Agents::session()` is the one choke point).
pub const OBSERVED: &str = "watch-only: the harness runs this subagent; open or message its parent";
/// 141 §5.4: the peer-address refusal (`send_message`, peer intake).
pub const OBSERVED_PEER: &str = "watch-only harness subagent — message its parent";

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

    async fn tell_cancel(&self, keep: Option<String>) {
        match self {
            Self::Native(a) => {
                let _ = a.tell(CancelTurn { keep }).await;
            }
            Self::Acp(a) => {
                let _ = a.tell(CancelTurn { keep }).await;
            }
        }
    }

    /// 139 D19: cancel the turn, then stop once the cancel is processed —
    /// never `kill()`, which drops the queued cancel with the mailbox.
    async fn stop(&self) {
        match self {
            Self::Native(a) => transfer::stop_actor(a).await,
            Self::Acp(a) => transfer::stop_actor(a).await,
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

    /// The open turn the actor relaunched through `Resume`, while it runs;
    /// `None` for an ordinary turn. An ACP child's prompts are never
    /// relaunched this way (146 D9: a respawn re-prompts).
    async fn resumed_turn(&self) -> Option<TurnId> {
        match self {
            Self::Native(a) => a.ask(ResumedTurn).await.unwrap_or_default(),
            Self::Acp(_) => None,
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
    /// The pool permit the spawn turn holds; `None` for a follow-up turn
    /// (human steer, peer message) and for a human-promoted spawn turn.
    permit: Option<SpawnPermit>,
    /// `max_runtime_secs` timer (178 W1: every child turn has one); `None`
    /// only when no supervisor ref was at hand to arm it.
    timer: Option<AbortHandle>,
    /// 178 F2: which arm `timer` is; a [`ChildTimedOut`] of another is stale.
    arm: u64,
    /// When the turn started (the picker's elapsed column).
    started_at: DateTime<Utc>,
}

/// One orchestrator tree's two child-work pools (01 §2.6).
struct TreePools {
    /// `subagents.max_concurrent`: children this engine's loop executes,
    /// plus the tree's peer-opened bounded turns (50 §8).
    native: Arc<Semaphore>,
    /// `subagents.max_concurrent_external`: ACP harness and mesh-node children.
    external: Arc<Semaphore>,
}

impl TreePools {
    fn new(cfg: &SubagentsConfig) -> Self {
        Self {
            native: Arc::new(Semaphore::new(cfg.max_concurrent.max(1))),
            external: Arc::new(Semaphore::new(cfg.max_concurrent_external.max(1))),
        }
    }

    fn get(&self, pool: Pool) -> &Arc<Semaphore> {
        match pool {
            Pool::Native => &self.native,
            Pool::External => &self.external,
        }
    }
}

/// A spawn turn's permit: from `pool` of the tree rooted at `root`.
struct SpawnPermit {
    pool: Pool,
    root: SessionId,
    /// Held, never read: dropping it frees the slot.
    _permit: OwnedSemaphorePermit,
}

/// Which concurrency pool a child's spawn turn draws from (01 §2.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pool {
    /// Executed by this engine's loop.
    Native,
    /// Executed elsewhere: an ACP harness or a mesh node.
    External,
}

impl Pool {
    fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::External => "external",
        }
    }
}

/// A spawned child whose first turn waits for a pool permit (01 §2.6).
struct QueuedSpawn {
    /// The waiting `acquire_owned`; aborted when the child is dropped or promoted.
    abort: AbortHandle,
    /// The brief's ticket, admitted by [`SpawnChild`].
    ticket: InputTicket,
    /// The brief.
    text: String,
    pool: Pool,
    /// The orchestrator tree whose pool it waits for.
    root: SessionId,
    since: Instant,
    /// Inputs addressed to the child while it waited, in arrival order;
    /// routed right after the brief so it is always the child's first input.
    held: Vec<Held>,
}

/// An input held for a queued child (01 §2.6), with its original flags.
enum Held {
    /// A [`Agents::route`] input (a harness result, an injected text).
    Routed {
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        channel: Option<ChannelOrigin>,
        ticket: InputTicket,
        scope: Option<Uuid>,
        defer: bool,
    },
    /// A peer exchange admitted by [`Agents::enqueue_peer`].
    Peer(Box<PeerInput>),
}

/// 145 §0.3: a session's live talk and the harness inputs parked behind its
/// spoken window. One timer at a time: the coalesce window while `open`,
/// the cap while closed; `epoch` outdates a timer that fires after a release.
struct Talk {
    live: TalkLive,
    parked: Vec<ParkedInput>,
    timer: Option<AbortHandle>,
    epoch: u64,
    /// The supervisor, for the timer's [`ReleaseParked`].
    me: ActorRef<Agents>,
}

impl Talk {
    /// (Re)start the one timer: [`ReleaseParked`] for `session` after `delay` ms.
    fn arm(&mut self, session: SessionId, delay: u64) {
        self.disarm();
        let (me, epoch) = (self.me.clone(), self.epoch);
        self.timer = Some(
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let _ = me.tell(ReleaseParked { session, epoch }).await;
            })
            .abort_handle(),
        );
    }

    fn disarm(&mut self) {
        if let Some(h) = self.timer.take() {
            h.abort();
        }
    }

    /// Take what was parked; a timer in flight is stale from here.
    fn take(&mut self) -> Vec<ParkedInput> {
        self.disarm();
        self.epoch += 1;
        std::mem::take(&mut self.parked)
    }
}

/// One admitted `Automatic` input waiting for the spoken window (145 §0.3).
struct ParkedInput {
    text: String,
    files: Vec<PathBuf>,
    ticket: InputTicket,
    scope: Option<Uuid>,
}

/// `tell` from a [`Talk`] timer: route what `session` parked, unless a
/// release already happened (`epoch` moved on).
struct ReleaseParked {
    session: SessionId,
    epoch: u64,
}

/// A child's result its parent could not take yet (R2.7).
struct Undelivered {
    child: SessionId,
    turn: TurnId,
    /// Already capped and redacted, as `hand_off` prepared it.
    text: String,
    scope: Option<Uuid>,
}

/// The request scope a spawned child works for; `cancelled` once
/// [`Agents::cancel_peer_scope`] stopped it.
struct OwnedChild {
    scope: Uuid,
    cancelled: bool,
}

/// `ask` → `Result<(name, child, queued), String>`: create a child session and
/// start it on `task`. `queued` is the notice for the parent when every slot
/// of the child's pool is busy (01 §2.6): the brief runs when one frees, and
/// messages sent meanwhile are delivered after it. Returns before the child
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
    /// theuth skills (by name) materialised into an external harness's cwd
    /// on top of the `always` ones (design doc 75 §4); empty for the rest.
    pub skills: Vec<String>,
    /// 139 §7.4: the `spawn_subagent` call's id — the `waits(child_result)`
    /// row's key; empty when no tool call spawned it (tests, internal).
    pub call_id: String,
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

    /// 183 §7.6, W-25: the exported agent `agent` (its bare roster name) as a contact's
    /// inbox runs it. `None`: no such agent (the inbox keeps theuth's persona).
    async fn inbox(&self, _agent: &str) -> Option<InboxPersona> {
        None
    }
}

/// 183 §7.6: what a contact's inbox takes from the exported agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxPersona {
    /// The identity block (persona and charter; never the desk's journal or manual).
    /// `None`: theuth's own persona.
    pub identity: Option<String>,
    /// The agent's configured model; `None` = the default.
    pub model: Option<String>,
    /// The agent file's `tools:` ceiling; the inbox runs under the stricter of it and
    /// the contact's cap.
    pub tools: Option<Risk>,
}

/// `ask` → `()`: install the [`IdentitySource`]; installing again replaces it.
pub struct InstallIdentitySource(pub Arc<dyn IdentitySource>);

impl Message<InstallIdentitySource> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: InstallIdentitySource, _: &mut Context<Self, Self::Reply>) {
        self.identity = Some(m.0);
        // 186 review 2: the roster joined: full inboxes take their agents' live ceilings.
        self.refresh_full_inboxes().await;
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
    /// 183 Z3-R3: `session`, or an ancestor, is a contact's inbox.
    pub(crate) fn under_inbox(&self, session: SessionId) -> bool {
        let mut cur = session;
        for _ in 0..64 {
            if self.network.is_inbox(cur) {
                return true;
            }
            match self.sessions.get(&cur).and_then(|l| l.parent.as_ref()) {
                Some((parent, _)) => cur = *parent,
                None => return false,
            }
        }
        false
    }

    /// [`IdentitySource::identity`] over `session`'s lineage; `None` without a
    /// source or for a session of theuth's. 125 §5.1: an advisor session
    /// speaks as the advisor, whatever the roster says.
    pub(crate) async fn identity_of(&self, session: SessionId) -> Option<String> {
        if self.advisors.contains(&session) {
            return Some(crate::advisor::identity::identity(
                &self.rt.data_dir,
                self.advisor_cfg.max_notes_per_update,
            ));
        }
        // 183 §7.6: a contact's inbox speaks as the exported agent — also after a restart,
        // before the contact writes again and before the roster's source joins (Z3-R3: the
        // persona of its last admission is kept with the inbox).
        if let Some(persona) = self.network.inbox_persona(session) {
            return persona.identity.clone();
        }
        if let Ok(Some(identity)) = self.store.a2a_inbox_identity(session).await {
            return identity;
        }
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

/// `ask` → `HashMap<SessionId, SessionActivity>`: the live facts of exactly
/// these sessions (62 §0.5), cold ones included. Internal: the tasks
/// sweep's liveness probe (WAKE-LIVE-CHILD D4).
#[derive(Clone, Debug)]
pub struct ActivityOf(pub Vec<SessionId>);

/// `tell` (internal): a queued child's permit was acquired; run its first
/// turn from the [`QueuedSpawn`] entry (gone = dropped meanwhile: the permit
/// is released untouched).
struct StartChildTurn {
    child: SessionId,
    permit: SpawnPermit,
}

/// `tell` (internal): a child turn hit `max_runtime_secs`; `.1` is the arm
/// ([`ChildTurn::arm`]) whose timer fired (178 F2).
struct ChildTimedOut(SessionId, u64);

const ADJECTIVES: [&str; 8] = [
    "quick", "calm", "bold", "keen", "brisk", "neat", "wise", "swift",
];
const NOUNS: [&str; 8] = [
    "otter", "heron", "fox", "lynx", "finch", "hare", "wren", "elk",
];

/// `Agents::session`'s `acp` argument: `(harness, cwd, cap, skills)` of a new
/// child on an external harness (06 §3.2, 75 §4).
type AcpSpawn = (String, Option<PathBuf>, Option<Risk>, Vec<String>);

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
    /// needed. `acp` = `(harness, cwd, cap, skills)` runs a new child on an external
    /// harness (design doc 06 §3.2, 75 §4); a row whose `agent` column is set resumes on one.
    /// `pick` = `(model, effort)` chosen by the spawning parent for a new child
    /// (01 §2.4); `None` entries fall back to the subagent config.
    pub(crate) async fn session(
        &mut self,
        id: Option<SessionId>,
        parent: Option<(SessionId, &str)>,
        acp: Option<AcpSpawn>,
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
        // 125 §4.1/§4.2: the hidden advisor child is never a peer; the
        // advised session's loop hooks switch on when a link exists.
        let kind = row.as_ref().map(|r| r.kind).unwrap_or_default();
        // 141 §5.4: a watch-only child is the harness's; nothing here drives it.
        if kind == SessionKind::Observed || row.as_ref().is_some_and(|r| r.observed.is_some()) {
            anyhow::bail!("{OBSERVED}");
        }
        let advised = kind == SessionKind::Chat && store.advisor_link(id).await?.is_some();
        if kind == SessionKind::Advisor {
            self.advisors.insert(id);
            self.communication.0.write().record_hidden(id);
        }
        let harness = acp
            .as_ref()
            .map(|a| a.0.clone())
            .or_else(|| row.as_ref().and_then(|r| r.agent.clone()));
        if let Some(name) = harness {
            if private_root.is_some() {
                anyhow::bail!("remote agents are unavailable in a private session");
            }
            let (cwd, cap, skills) = acp.map_or((None, None, Vec::new()), |a| (a.1, a.2, a.3));
            let actor = self
                .acp_session(id, row, parent, name, cwd, pick, cap, skills)
                .await?;
            return Ok((id, actor));
        }
        // 136 R-22: the cap set before a restart still meters the resumed turn.
        let budget = store.budget(id).await?;
        let desk = row.as_ref().and_then(|r| r.desk.clone());
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
                // 109 §4.5: the persisted `last_stop` / `finished_at`, so a
                // cold session reports its real end after a restart.
                if let Ok(Some(stop)) = store.last_stop(id).await {
                    self.last_stop.entry(id).or_insert(stop);
                }
                (
                    self.resolve_model(&row.model),
                    row.effort,
                    state,
                    row.parent.zip(row.name),
                    resumed,
                )
            }
            None => {
                let sub_effort = self
                    .subagent_effort
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let preferred = self
                    .subagent_model
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .map(|r| r.id.clone());
                let desk_model = parent
                    .and_then(|(p, _)| self.desk_models.get(&p))
                    .and_then(|m| m.as_deref());
                let desk_effort = parent.and_then(|(p, _)| self.desk_efforts.get(&p));
                let (model, effort) = match parent {
                    // 148 E2: an explicit pick, then the desk agent's model
                    // when the parent is a desk, then the subagent default.
                    // 194: the effort follows the same chain.
                    Some(_) => (
                        pick.0
                            .or(desk_model)
                            .or(preferred.as_deref())
                            .unwrap_or(&self.default_model),
                        pick.1
                            .map(str::to_owned)
                            .or_else(|| desk_effort.cloned())
                            .or(sub_effort)
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
                // 148 R1 F6: an inherited default effort (not a pick) that the
                // chosen model does not take would 400 the first call: drop it,
                // as `set_model` does for an in-session switch.
                let effort = effort.filter(|e| {
                    pick.1.is_some()
                        || model
                            .0
                            .model
                            .as_ref()
                            .is_none_or(|m| m.validate_effort(e).is_ok())
                });
                store
                    .create_session(
                        id,
                        &model.0.id,
                        parent.map(|p| p.0),
                        parent.map(|p| p.1),
                        None,
                    )
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
        // 50 §8: a peer-opened bounded turn draws from the recipient's own
        // orchestrator tree (01 §2.6) — resolved here for a new child and
        // for a session loaded lazily after a restart alike.
        let root = match &parent {
            Some((p, _)) => store.root_of(*p).await?,
            None => id,
        };
        let permits = self.tree_pools(root).native.clone();
        // 137: register with the endless service — the pump starts when
        // `sessions.memory` is `endless`; a private tree hands over its own
        // store. A native child of a parent running the endless frame gets
        // the R1-D1 note (its opener carries the view, `start_child_turn`).
        let parent_view = parent
            .as_ref()
            .is_some_and(|(p, _)| self.endless.active(*p));
        let attached = Attached {
            model: model.0.clone(),
            effort: effort.as_deref().and_then(effort_of),
            agent: desk.unwrap_or_else(|| "theuth".to_owned()),
            store: private_root.map(|_| store.clone()),
        };
        if let Err(e) = self.endless.attach(id, attached).await {
            tracing::warn!(session = %id, "endless attach: {e:#}");
        }
        // 183 §7.6, Z3-R2: a contact's inbox is capped and default-deny before its actor
        // exists (after a restart, or a child result waking it), failing closed: limits not
        // installed, no actor.
        self.reinstall_contact_limits(id)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        // Unbounded (R2.6): `route` awaits `tell_input` inside a supervisor
        // handler; a full bounded mailbox here would stall the engine.
        let session = Session::new(
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
            permits,
            resources,
        )
        .with_kind(kind)
        .with_advisor(advised)
        .with_budget(budget)
        .with_polish_model(self.polish_model.clone())
        .with_knowledge_harvester_model(self.knowledge_harvester_model.clone())
        .with_role_efforts(self.role_efforts.clone())
        .with_subagent_effort(self.subagent_effort.clone())
        .with_endless(self.endless.clone())
        .with_parent_view(parent_view);
        let actor = Session::spawn_with_mailbox(session, runtime::subscriber_mailbox());
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
    pub(crate) async fn mirror(&mut self, id: SessionId) {
        let root = self.communication.0.read().root_of(id);
        match root {
            Err(_) => {
                let lineage = self.store.lineage(id).await;
                self.apply_lineage(id, lineage);
            }
            Ok(root) => self.refresh_root(root).await,
        }
        self.sync_opt_out(id).await;
    }

    /// 183 §6.6: a known root owned by another engine changed its scope or alias there (a
    /// replicated row): the live policy follows, so a scope flipped on B reaches A's routes.
    /// A root of ours is the policy's own (`SetPeerScope` writes both); one whose row agrees
    /// is left alone.
    async fn refresh_root(&mut self, root: SessionId) {
        if self.remote_owner(root).await.is_none() {
            return;
        }
        let Ok(Some((scope, alias))) = self.store.root_scope(root).await else {
            return;
        };
        let changed = {
            let p = self.communication.0.read();
            match p.state(Some(root)) {
                Ok(s) if s.scope == scope && s.alias == alias => return,
                Ok(s) => s.scope != scope,
                Err(_) => return,
            }
        };
        let _ = self
            .revoking(|p| p.set_scope(root, scope, Some(alias.unwrap_or_default())))
            .await;
        if changed {
            self.expire_waits("scope changed", |w| !w.generation.live());
            // Boxed: the revoke path can reach `mirror` again (an intake spawning an actor).
            Box::pin(self.network_revoked(Some(root), "scope changed")).await;
        }
        self.communication_changed(Some(root)).await;
    }

    /// 183 R1 (S1): the row's agent-messaging opt-out (`sessions.communication
    /// = 0`) onto the live policy, both ways. The row is the truth after a
    /// restart or a move, and an opt-in written on another engine must not
    /// leave a stale opt-out here (A→B→A). The setter writes the row first.
    async fn sync_opt_out(&self, id: SessionId) {
        let Ok(Some((_, communication))) = self.store.session_settings(id).await else {
            return;
        };
        let out = communication == Some(false);
        let mut p = self.communication.0.write();
        if p.state(Some(id)).is_ok_and(|s| s.opted_out != out) {
            let _ = p.set_session(id, !out);
        }
    }

    /// [`Self::mirror`] given its lineage read, which 176 E1 may have done
    /// off the mailbox: a no-op when `id` became known meanwhile, and a root
    /// already known keeps its live scope (a `SetPeerScope` since the read
    /// is never undone).
    fn apply_lineage(&self, id: SessionId, lineage: anyhow::Result<store::communication::Lineage>) {
        let lineage = match lineage {
            Ok(l) => l,
            // No row: nothing to mirror; every route for `id` fails closed.
            Err(e) if e.downcast_ref::<CommError>() == Some(&CommError::NotFound) => return,
            Err(e) => {
                tracing::error!(session = %id, "lineage: {e:#}");
                return;
            }
        };
        let mut p = self.communication.0.write();
        if p.root_of(id).is_ok() {
            return;
        }
        let root = lineage.root();
        if p.root_of(root).is_err() {
            if let Err(e) = p.record_root(root, lineage.scope) {
                tracing::error!(session = %root, "record_root: {e}");
                return;
            }
            if lineage.alias.is_some() {
                let _ = p.set_scope(root, lineage.scope, lineage.alias.clone());
            }
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
    /// §8); a local harness picks its own. A new child on a local harness
    /// gets the `always` skills plus `skills` written into its cwd first
    /// (75 §4); the actor removes them when it stops.
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
        skills: Vec<String>,
    ) -> anyhow::Result<LiveActor> {
        let entries = acp::load(&self.rt.data_dir)?;
        let (harness, engine) = crate::peers::parse_agent(&name).map_err(anyhow::Error::msg)?;
        let node = self
            .nodes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(engine)
            .map(|(_, d)| d.clone());
        let (transport, fallback_model, meta, model, effort, harness_skills, mcp, steer) =
            match (harness, entries.get(engine), node) {
                // A local harness: `model` and `effort` through its options
                // after `session/new` (67 §8, 75 §8).
                (None, Some(entry), _) => (
                    acp::Transport::Process(entry.config()),
                    entry.model.clone(),
                    None,
                    pick.0.map(str::to_owned),
                    pick.1.map(str::to_owned),
                    Some((
                        entry.skills.unwrap_or(true),
                        entry
                            .skills_dir
                            .clone()
                            .unwrap_or_else(|| acp::DEFAULT_SKILLS_DIR.into()),
                    )),
                    entry.mcp_enabled(),
                    entry.steer(),
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
                    (
                        acp::Transport::Dial(dialer),
                        None,
                        Some(meta),
                        None,
                        None,
                        None,
                        false,
                        acp::Steer::default(),
                    )
                }
                _ => anyhow::bail!(
                    "{}",
                    crate::peers::unknown(engine, &entries, &node_lines(&self.nodes))
                ),
            };
        let cwd_path = cwd
            .clone()
            .or_else(|| {
                row.as_ref()
                    .and_then(|r| r.cwd.as_deref().map(PathBuf::from))
            })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        // 75 §4: skills into the cwd before the row exists, so a bad cwd or
        // name is a clean spawn error. A resume (75 §10) writes the row's
        // stored names back — the stopped actor removed them; a directory
        // still there is left alone, and a name no longer in the index is
        // dropped with a warning and a note to the child, not a failure.
        let mut stored_names = Vec::new();
        let mut notes = Vec::new();
        let names: &[String] = match &row {
            Some(_) => {
                let index = crate::skills::index(&crate::skills::dir(&self.rt.data_dir));
                for name in self.store.acp_skills(id).await? {
                    if index.iter().any(|s| s.name == name) {
                        stored_names.push(name);
                    } else {
                        tracing::warn!(session = %id, skill = %name, "stored skill no longer exists, not materialised on resume");
                        notes.push(format!(
                            "skill {name} no longer exists and was not materialised"
                        ));
                    }
                }
                &stored_names
            }
            None => &skills,
        };
        let materialised = match (&row, harness_skills) {
            (_, Some((true, dir))) => crate::skills::materialise(
                &crate::skills::dir(&self.rt.data_dir),
                &cwd_path,
                &dir,
                names,
                &id.to_string(),
            )
            .map_err(anyhow::Error::msg)?,
            (None, Some((false, _))) if !skills.is_empty() => {
                anyhow::bail!("`{engine}` has `skills: false` in agents.yaml")
            }
            (None, None) if !skills.is_empty() => {
                anyhow::bail!("`skills` cannot be written into a node's cwd")
            }
            _ => Default::default(),
        };
        let mcp_servers = if mcp {
            vec![mcp_shim(&self.rt.data_dir, id)?]
        } else {
            Vec::new()
        };
        // An existing row belonged to an earlier process (see `Agents::ledger`).
        let resumed = row.is_some();
        let (parent, resume) = match row {
            Some(row) => (row.parent.zip(row.name), self.store.acp_session(id).await?),
            None => {
                self.store
                    .create_session(id, &name, parent.map(|p| p.0), parent.map(|p| p.1), None)
                    .await?;
                // 06 §8.4: the resolved cwd is stored, so a resume passes
                // the session's own directory (opencode 2 requires it).
                self.store
                    .set_acp(id, &name, Some(&cwd_path), &skills)
                    .await?;
                (parent.map(|(p, n)| (p, n.to_string())), None)
            }
        };
        self.inherit_approvals(id, parent.as_ref().map(|p| p.0))
            .await;
        // 193 B1: the child's tools (its MCP shim's `session_call_tool`) resolve relative paths
        // where the harness runs, keeping the parent's write_root. The registry's `SetParent` is
        // a `tell` and `inherit_approvals` may already have seated the child from its row (an
        // entry `seat::inherit` would not touch), so copy the write_root here, then the cwd.
        if let Some((p, _)) = &parent
            && tools::seat::seat_of(id).write_root.is_none()
        {
            tools::seat::set_write_root(id, tools::seat::seat_of(*p).write_root);
        }
        tools::seat::set_cwd(id, Some(cwd_path.clone()));
        self.mirror(id).await;
        // 176 A2: a child parked idle is back. Like `relaunch_acp_child`,
        // a cancelled last turn is not to be picked up from the replay.
        if let Some(at) = self.parked.remove(&id) {
            tracing::info!(
                "acp: resumed parked child session={id} after={}s",
                at.elapsed().as_secs()
            );
            if self
                .last_stop
                .get(&id)
                .is_some_and(|(r, _)| r == "cancelled")
            {
                notes.push(crate::acp_child::CANCELLED_NOTE.to_owned());
            }
        }
        // Unbounded (R2.6): the supervisor `tell`s into this mailbox from
        // its own handlers and must never block on a chatty harness.
        let child = AcpChild::new(
            id,
            name,
            transport,
            cwd_path,
            resume,
            fallback_model,
            meta,
            model,
            effort,
            self.request_deadline(),
            self.store.clone(),
            self.rt.clone(),
            materialised,
            mcp_servers,
        )
        .with_activity(self.cfg.activity)
        .with_steer(steer);
        let actor = LiveActor::Acp(AcpChild::spawn_with_mailbox(
            child,
            runtime::subscriber_mailbox(),
        ));
        if let LiveActor::Acp(a) = &actor {
            for n in notes {
                let _ = a.tell(crate::acp_child::Note(n)).await;
            }
        }
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

    /// Subagents inherit the parent's approval switch and hidden tools. 183
    /// §4.5 (D4): the row's approvals pin, hidden tools and agent-messaging
    /// opt-out are applied here, so a restart, resume or take keeps them
    /// before the first tool call.
    async fn inherit_approvals(&mut self, id: SessionId, parent: Option<SessionId>) {
        let reg = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten();
        if let Some(parent) = parent
            && let Some(reg) = &reg
        {
            let _ = reg
                .tell(SetParent {
                    session: id,
                    parent,
                })
                .await;
        }
        let (pin, _) = match self.store.session_settings(id).await {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                tracing::error!(session = %id, "session settings: {e:#}");
                return;
            }
        };
        // `None` too: a pin this registry held from before a move away is stale.
        if let Some(reg) = &reg {
            let _ = reg.ask(RestoreApprovals { session: id, pin }).await;
            // Empty too, for the same reason; a list that does not parse
            // leaves the registry as it is.
            match self.store.session_hidden(id).await {
                Ok(tools) => {
                    let _ = reg.ask(RestoreHidden { session: id, tools }).await;
                }
                Err(e) => tracing::error!(session = %id, "hidden tools: {e:#}"),
            }
        }
        // 183 R1 (S1): the opt-out both ways, as every reader applies it.
        self.mirror(id).await;
        // 183 R2: a restart keeps a chat's chosen folder (tools and the
        // prompt's "Working directory"), as its create and a take seat it;
        // a seat set already (a schedule's `cwd:`, a worktree) wins.
        if tools::seat::seat_of(id).cwd.is_none()
            && let Ok(Some(row)) = self.store.session(id).await
            && let Some(cwd) = row.cwd.map(PathBuf::from).filter(|c| c.is_dir())
        {
            tools::seat::set_cwd(id, Some(cwd));
        }
    }

    /// Route `text` (plus image `files`) into `sid` (created when unknown);
    /// errors surface as `AgentEvent::Error` and as the `Err`. A rejected
    /// attachment drops the whole input (17 §3.1). The input's ticket is
    /// admitted here, before the actor `tell`, so a yield decision handled
    /// after this sees it (47 §4.3). `delivery` says how it lands on a busy
    /// session (62 §0.3). `channel` is the 172 origin of a channel's input.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn input(
        &mut self,
        sid: SessionId,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        channel: Option<ChannelOrigin>,
        origin: InputOrigin,
        delivery: Delivery,
    ) -> Result<(), String> {
        // 179 D6: a person writing to a paused agent's session is told so
        // (no hold that a restart could lose, no silent wait).
        if origin == InputOrigin::External
            && let Some(root) = self.paused_root(sid).await
        {
            let agent = self.agent_of_desk(root).await;
            return Err(format!("{agent} is paused — un-pause it to send"));
        }
        let ticket = self.ledger.admit(sid, origin, id);
        let defer = matches!(delivery, Delivery::NextTurn);
        // 145 §0.3: a harness input for an idle talk session waits for the
        // spoken window; a human input takes what was parked along as steers.
        if origin == InputOrigin::Automatic && self.parks(sid) {
            self.park(sid, text, files, ticket, None).await;
            return Ok(());
        }
        // A human reaches a queued child (01 §2.6): its brief runs now,
        // permit-free, then this text, then whatever was held for it.
        let held = self.promote(sid).await;
        let r = self
            .route(sid, id, text, files, channel, ticket, None, defer)
            .await;
        self.replay(sid, held).await;
        if origin == InputOrigin::External {
            self.release_parked(sid).await;
        }
        r
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
        channel: Option<ChannelOrigin>,
        ticket: InputTicket,
        scope: Option<Uuid>,
        defer: bool,
    ) -> Result<(), String> {
        // The child's brief waits for a permit: nothing reaches its session
        // before the brief (01 §2.6); 176 A1: an ACP child mid park probe
        // may stop before reading it. Replayed by `replay`, flags intact.
        if let Some(held) = self
            .queued
            .get_mut(&sid)
            .map(|q| &mut q.held)
            .or_else(|| self.parking.get_mut(&sid))
        {
            held.push(Held::Routed {
                id,
                text,
                files,
                channel,
                ticket,
                scope,
                defer,
            });
            return Ok(());
        }
        // 179 D6: a paused agent's tree starts no turn; the input waits,
        // flags intact, for `ResumeTree` (the caller's receipt stands).
        if let Some(root) = self.paused_root(sid).await {
            self.hold(
                root,
                sid,
                Held::Routed {
                    id,
                    text,
                    files,
                    channel,
                    ticket,
                    scope,
                    defer,
                },
            );
            return Ok(());
        }
        // 139 D16: a session mid-release-drain takes nothing — the fence is
        // about to move. Refused before any row is written (no error row
        // in the transcript); the RPC layer maps the prefix to
        // `SESSION_DRAINING` (-32012).
        if let Some(reason) = self.draining_sessions.get(&sid).copied() {
            self.ledger.release(sid, ticket);
            self.network_input_dropped(ticket).await;
            return Err(transfer::draining_error(reason));
        }
        let routed = match check_files(&files) {
            Ok(()) => self.session(Some(sid), None, None, (None, None)).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        let sent = match routed {
            Ok((_, actor)) => {
                // 146 D9: the harness owes work from this hand-off on
                // (journaled before the tell: a crash in between reads as
                // an owed prompt, which the respawn re-prompts).
                if let LiveActor::Acp(_) = &actor {
                    self.acp_journal_open(sid, "input", &text, None).await;
                }
                actor
                    .tell_input(UserInput {
                        id,
                        text,
                        files,
                        channel,
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
    /// An `id` seen within [`SEEN_TTL`] is a retry: the remembered session is
    /// returned and nothing is routed. A routing failure is `Err` so the
    /// caller keeps the text.
    async fn human_input(
        &mut self,
        session: Option<SessionId>,
        id: Option<InputId>,
        text: String,
        files: Vec<PathBuf>,
        channel: Option<ChannelOrigin>,
        delivery: Delivery,
    ) -> Result<SessionId, String> {
        let sid = session.unwrap_or_default();
        if let Some(id) = id {
            self.seen.retain(|_, (_, t)| t.elapsed() < SEEN_TTL);
            if let Some((sid, _)) = self.seen.get(&id) {
                return Ok(*sid);
            }
        }
        // A failure was published as `AgentEvent::Error` on `sid` already; it
        // is not remembered, so the retry routes.
        self.input(
            sid,
            id,
            text,
            files,
            channel,
            InputOrigin::External,
            delivery,
        )
        .await?;
        if let Some(id) = id {
            self.seen.insert(id, (sid, Instant::now()));
        }
        Ok(sid)
    }

    /// A session-scoped failure outside any turn: recorded behind the last
    /// visible message (history replaces live rows on reload), then published.
    /// A closed private id has no row anywhere and gets none (51 §4.4).
    async fn error(&self, session: SessionId, message: String) {
        if !self.closing.contains(&session)
            && let Err(e) = self
                .store_for(session)
                .record_error(session, &message)
                .await
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
    /// caller's `Err` string. `pub(crate)` for `notebook_context::WakeSession` (144 §4.3).
    pub(crate) async fn actor(&mut self, session: SessionId) -> Result<LiveActor, String> {
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
    /// the scope's work from admission (50 §6.3). 139 Revision 2: a session
    /// owned by another engine never gets an actor here on account of a
    /// harness message — the body is queued in the engine-local
    /// `harness_outbox` and the shipper sends it to the owner over
    /// `Msg::Harness` (re-resolving the owner at every attempt); the owner
    /// runs this same function ([`HarnessDeliver`]).
    async fn inject(
        &mut self,
        to: SessionId,
        from: &str,
        text: String,
        wrapped: String,
        scope: Option<Uuid>,
    ) {
        self.inject_as(to, from, text, wrapped, scope, true).await;
    }

    /// [`Self::inject`] with 171 §9.3's wake rule: `wake: false` parks the result in the
    /// session actor until its next turn opens for any reason ([`QuietInput`]), holding no
    /// ledger ticket meanwhile; a session owned elsewhere gets the [`QUIET_KIND`] marker on
    /// its `harness_outbox` row so the owner parks it too. An ACP runner has no quiet
    /// input: the result wakes it as before.
    async fn inject_as(
        &mut self,
        to: SessionId,
        from: &str,
        text: String,
        mut wrapped: String,
        scope: Option<Uuid>,
        wake: bool,
    ) {
        if let Some(owner) = self.remote_owner(to).await {
            let me = self.store.engine();
            // 184 §6.3: a cell result ran here whatever the session did.
            let note = if from.starts_with("cell ") {
                format!("(ran on {me})")
            } else {
                format!("(produced on {me} after the session moved to {owner})")
            };
            // Inside the closing tag, so the model and the UI read one element.
            let cut = wrapped.rfind("\n</").unwrap_or(wrapped.len());
            let wrapped = format!("{}\n{note}{}", &wrapped[..cut], &wrapped[cut..]);
            let row = store::harness::HarnessRow {
                id: Uuid::new_v4().to_string(),
                session: to.to_string(),
                kind: if wake {
                    from.to_owned()
                } else {
                    format!("{QUIET_KIND}{from}")
                },
                origin: me.to_owned(),
                text: format!("{text}\n{note}"),
                wrapped,
                scope: scope.map(|s| s.to_string()),
                at: chrono::Utc::now().timestamp_millis(),
                attempts: 0,
                last_error: None,
            };
            match self.store.harness_enqueue(&row).await {
                Ok(()) => {
                    tracing::info!(session = %to, %owner, id = %row.id, "139: {from}: queued for the owner")
                }
                Err(e) => {
                    tracing::warn!(session = %to, %owner, "139: {from}: not queued for the owner: {e:#}")
                }
            }
            return;
        }
        if !wake && scope.is_none() {
            match self.quiet(to, wrapped).await {
                Ok(()) => {
                    self.rt
                        .publish(AgentEvent::MessageDelivered {
                            session: to,
                            from: from.to_string(),
                            text,
                        })
                        .await;
                    return;
                }
                Err(w) => wrapped = w,
            }
        }
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
        // 145 §0.3: an idle talk session hears results when its window opens.
        if self.parks(to) {
            self.park(to, wrapped, Vec::new(), ticket, scope).await;
        } else {
            // A failure is already an `AgentEvent::Error` on `to`.
            let _ = self
                .route(to, None, wrapped, Vec::new(), None, ticket, scope, false)
                .await;
        }
        self.rt
            .publish(AgentEvent::MessageDelivered {
                session: to,
                from: from.to_string(),
                text,
            })
            .await;
    }

    /// Hand `wrapped` to `sid`'s native session actor as a [`QuietInput`]; `Err` gives it
    /// back when no native actor can take it (a queued child, a draining session, a paused
    /// agent's tree — 179: `route` holds it —, an ACP runner, a closed mailbox) and the
    /// caller routes it as a waking input instead.
    async fn quiet(&mut self, sid: SessionId, wrapped: String) -> Result<(), String> {
        if self.queued.contains_key(&sid)
            || self.draining_sessions.contains_key(&sid)
            || self.paused_root(sid).await.is_some()
        {
            return Err(wrapped);
        }
        let Ok((_, LiveActor::Native(actor))) =
            self.session(Some(sid), None, None, (None, None)).await
        else {
            return Err(wrapped);
        };
        actor
            .tell(QuietInput(wrapped))
            .await
            .map_err(|e| e.msg().map_or_else(String::new, |QuietInput(w)| w))
    }

    /// 145 §0.3: an `Automatic` input for `sid` waits for the spoken window —
    /// a live talk and no running turn (the ledger's idle, the supervisor's
    /// view of the busy test `defer` uses). A session without a talk, or a
    /// busy one, routes as before.
    fn parks(&self, sid: SessionId) -> bool {
        self.talk.contains_key(&sid) && self.ledger.is_idle(sid)
    }

    /// Park an admitted input ([`Self::parks`] held). Open window: items
    /// landing within [`TALK_COALESCE_MS`] of the latest join one turn.
    /// Closed: parked until it opens, an `External` input, `live: None` or
    /// the [`TALK_PARK_CAP_MS`] cap, counted from the first item; each
    /// parked item publishes [`AgentEvent::InputsParked`].
    async fn park(
        &mut self,
        sid: SessionId,
        text: String,
        files: Vec<PathBuf>,
        ticket: InputTicket,
        scope: Option<Uuid>,
    ) {
        let Some(t) = self.talk.get_mut(&sid) else {
            return;
        };
        t.parked.push(ParkedInput {
            text,
            files,
            ticket,
            scope,
        });
        let count = u32::try_from(t.parked.len()).unwrap_or(u32::MAX);
        let open = t.live.open;
        if open {
            t.arm(sid, TALK_COALESCE_MS);
        } else if t.timer.is_none() {
            t.arm(sid, TALK_PARK_CAP_MS);
        }
        if !open {
            self.rt
                .publish(AgentEvent::InputsParked {
                    session: sid,
                    count,
                })
                .await;
        }
    }

    /// Route everything `sid` parked as one input per request scope (texts
    /// joined by a blank line, files concatenated), on the first item's
    /// ticket; the other tickets are released — their input rides the merged
    /// one. Disarms the timer. No-op without a talk or parked items.
    async fn release_parked(&mut self, sid: SessionId) {
        let Some(t) = self.talk.get_mut(&sid) else {
            return;
        };
        let parked = t.take();
        self.route_parked(sid, parked).await;
    }

    async fn route_parked(&mut self, sid: SessionId, parked: Vec<ParkedInput>) {
        // Scopes stay apart (50 §6.3), in first-appearance order.
        let mut groups: Vec<(Option<Uuid>, Vec<ParkedInput>)> = Vec::new();
        for p in parked {
            match groups.iter_mut().find(|(s, _)| *s == p.scope) {
                Some((_, g)) => g.push(p),
                None => groups.push((p.scope, vec![p])),
            }
        }
        for (scope, group) in groups {
            let mut items = group.into_iter();
            let Some(first) = items.next() else { continue };
            let (mut text, mut files) = (first.text, first.files);
            for p in items {
                text.push_str("\n\n");
                text.push_str(&p.text);
                files.extend(p.files);
                self.ledger.release(sid, p.ticket);
                self.network_input_dropped(p.ticket).await;
            }
            // A failure is already an `AgentEvent::Error` on `sid`.
            let _ = self
                .route(sid, None, text, files, None, first.ticket, scope, false)
                .await;
        }
    }

    /// Resolve `to` relative to `from` (50 §5.1): `session:<uuid>` must be a
    /// stored row (never created), `parent` and a direct child's name are
    /// conveniences, `peer:<name>` is a configured remote. Unknown and
    /// malformed are one answer, so a guessed UUID learns nothing.
    async fn resolve(&mut self, from: SessionId, to: &str) -> Result<Target, String> {
        // 200 (Contract): `peer:<contact>/session:<x>` with no UUID says so.
        let unknown = || {
            crate::communication::network::access::bad_session_id(to)
                .unwrap_or_else(|| format!("unknown peer `{to}`"))
        };
        match PeerAddress::parse(to).ok_or_else(unknown)? {
            PeerAddress::Remote(name) => Ok(Target::Remote(name)),
            PeerAddress::Session(id) => {
                match self.store.session(id).await.map_err(|e| e.to_string())? {
                    // 125 §4.2: the hidden advisor child is never a peer.
                    Some(row) if row.kind == SessionKind::Advisor => {
                        Err(format!("`{to}` is not a peer"))
                    }
                    // 141 §5.4: nor is a watch-only harness subagent.
                    Some(row) if row.kind == SessionKind::Observed || row.observed.is_some() => {
                        Err(format!("`{to}`: {OBSERVED_PEER}"))
                    }
                    Some(_) => {
                        self.mirror(id).await;
                        Ok(Target::Local(id))
                    }
                    None => Err(unknown()),
                }
            }
            PeerAddress::Relative(name)
                if name == "parent" && self.remote_parent.contains_key(&from) =>
            {
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
            PeerAddress::Relative(name) => {
                if let Some(child) = self
                    .store
                    .child_by_name(from, &name)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    return Ok(Target::Local(child));
                }
                // 183 §6.6: else an owner-scope root by its alias (`ops@xeus`, a desk on
                // any engine of the mesh); the route still checks both scopes. Only a root
                // its `@engine` owner wrote counts: any member can mint an alias.
                let (_, suffix) = name.rsplit_once('@').ok_or_else(unknown)?;
                let (me, names) = peer_mesh::engine_names().await;
                let root = self
                    .store
                    .root_by_alias(&name)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|(_, owner)| {
                        let here = owner.is_empty()
                            || mesh::self_id().is_some_and(|s| s == owner.as_str());
                        let owner_name = if here {
                            me.clone()
                        } else {
                            peer_mesh::engine_name(&names, owner)
                        };
                        suffix == owner || suffix == owner_name
                    })
                    .map(|(id, _)| id)
                    .ok_or_else(unknown)?;
                self.mirror(root).await;
                Ok(Target::Local(root))
            }
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
    pub(crate) fn wait(
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
                since_ms: Utc::now().timestamp_millis().max(0) as u64,
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

    /// Publish [`PeerMessageChanged`] to each local participant of `receipt`, with the exchange's ends and whether
    /// it is a child's final result (208 D6). The sender read runs in a spawned task, never on this actor (176);
    /// the changes of one exchange still publish in call order (each task waits for its predecessor's). A failed
    /// read publishes to the recipient with `from` empty: the event stays a refetch trigger, and the graph ignores
    /// an exchange without both ends rather than guessing one.
    pub(crate) async fn peer_changed(&self, receipt: &PeerReceipt) {
        type Chain = HashMap<Uuid, (u64, tokio::sync::oneshot::Receiver<()>)>;
        static ORDER: std::sync::LazyLock<parking_lot::Mutex<Chain>> =
            std::sync::LazyLock::new(Default::default);
        static TICKET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = receipt.id;
        let ticket = TICKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (done, rx) = tokio::sync::oneshot::channel();
        let prev = ORDER.lock().insert(id, (ticket, rx)).map(|(_, r)| r);
        let (store, rt, state, to) = (
            self.store.clone(),
            self.rt.clone(),
            receipt.state,
            receipt.to.clone(),
        );
        tokio::spawn(async move {
            if let Some(prev) = prev {
                // A dropped predecessor (panic) resolves too.
                let _ = prev.await;
            }
            let mut parties: Vec<SessionId> = PeerAddress::local(&to).into_iter().collect();
            let (mut from, mut result) = (String::new(), false);
            match store.exchange_sender(id).await {
                Ok(Some((sender, request_id))) => {
                    if let Some(s) = PeerAddress::local(&sender).filter(|s| !parties.contains(s)) {
                        parties.push(s);
                    }
                    result = request_id.starts_with("result:");
                    from = sender;
                }
                Ok(None) => {}
                Err(e) => tracing::error!(%id, "exchange_sender: {e:#}"),
            }
            for session in parties {
                rt.publish(PeerMessageChanged {
                    session,
                    id,
                    state,
                    from: from.clone(),
                    to: to.clone(),
                    result,
                })
                .await;
            }
            let _ = done.send(());
            let mut order = ORDER.lock();
            if order.get(&id).is_some_and(|(t, _)| *t == ticket) {
                order.remove(&id);
            }
        });
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

    /// 186 (D9): `session`'s peer-chain count, what a full contact message carries.
    pub(crate) fn chain_of(&self, session: SessionId) -> u32 {
        self.ledger.chain(session)
    }

    /// 186: `id` is handed to a runner now (`false`: already queued in this process).
    pub(crate) fn claim_enqueued(&mut self, id: Uuid) -> bool {
        self.enqueued.insert(id)
    }

    /// 186: the claim of [`Self::claim_enqueued`] failed before any runner took it.
    pub(crate) fn unclaim_enqueued(&mut self, id: Uuid) {
        self.enqueued.remove(&id);
    }

    /// 186 (D9): [`Self::enqueue_peer`] (unscoped) of a message from another engine that
    /// carries its sender's chain count, bounded as a mesh peer's (183 §6.5).
    pub(crate) async fn enqueue_peer_chained(
        &mut self,
        record: PeerRecord,
        generation: Generation,
        chain: u32,
    ) -> Result<(), String> {
        self.inbound_chain = Some((record.receipt.id, chain));
        let r = self.enqueue_peer(record, generation, false, false).await;
        self.inbound_chain = None;
        r
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
        if row.kind == SessionKind::Advisor {
            return Err(format!("`session:{to}` is not a peer"));
        }
        if row.kind == SessionKind::Observed || row.observed.is_some() {
            return Err(format!("`session:{to}`: {OBSERVED_PEER}"));
        }
        if isolated && row.agent.is_some() {
            return Err("an external harness cannot own an A2A task scope".into());
        }
        // The chain bound follows the initiating request across sessions
        // (50 §8): a local sender's chain is inherited; a sender on another
        // engine sends its count with the envelope (183 §6.5).
        let sender = PeerAddress::local(&record.from);
        let carried = self
            .inbound_chain
            .filter(|(id, _)| *id == record.receipt.id)
            .map_or(0, |(_, n)| n);
        if self.ledger.chain_exhausted_for(sender, to, carried) {
            return Err(format!(
                "chain limit: {} peer-triggered continuations since the last external input",
                crate::communication::CHAIN_MAX
            ));
        }
        // 179 D6: no actor for a paused agent's tree — the accepted exchange
        // waits for `ResumeTree`.
        let actor = match self.paused_root(to).await {
            Some(root) => Err(root),
            None => Ok(self.actor(to).await?),
        };
        let scope = record.request_scope;
        let ticket = self.ledger.admit_chained(to, None, sender, carried);
        if let Some(scope) = scope
            && !self.network_input_queued(scope, ticket).await
        {
            // The scope's work from admission until it enters a turn (50
            // §6.3); a cancelled scope takes no more input.
            self.ledger.release(to, ticket);
            return Err("canceled: the request scope is closed".into());
        }
        let input = PeerInput {
            record,
            generation,
            ticket,
            request_scope: scope,
            isolated,
            reply_expected,
        };
        // The recipient's brief waits for a permit (01 §2.6): held, so the
        // brief stays its first input and no bounded turn races the permit.
        if let Some(q) = self.queued.get_mut(&to) {
            q.held.push(Held::Peer(Box::new(input)));
            return Ok(());
        }
        let actor = match actor {
            Ok(actor) => actor,
            Err(root) => {
                self.hold(root, to, Held::Peer(Box::new(input)));
                return Ok(());
            }
        };
        // 146 D9: the harness owes work from this hand-off on.
        if let LiveActor::Acp(_) = &actor {
            self.acp_journal_open(to, "peer", &input.record.text, Some(&input.record.from))
                .await;
        }
        // 176 A1: a child mid park probe may stop before reading it.
        if let Some(held) = self.parking.get_mut(&to) {
            held.push(Held::Peer(Box::new(input)));
            return Ok(());
        }
        self.offer_peer(actor, to, input);
        Ok(())
    }

    /// The `ask` half of [`Self::enqueue_peer`], off this mailbox; a refusal
    /// comes back as [`PeerIntakeFailed`].
    fn offer_peer(&self, actor: LiveActor, to: SessionId, input: PeerInput) {
        let id = input.record.receipt.id;
        let ticket = input.ticket;
        let me = ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten();
        tokio::spawn(async move {
            let Err(reason) = actor.ask_peer(input).await else {
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
            self.drop_queued(child).await;
            match self.live(child) {
                // Its first turn runs under the inherited scope: the same
                // exact signal, in flight or still queued.
                Some(LiveActor::Native(a)) => {
                    let _ = a.tell(CancelPeerScope { scope }).await;
                }
                Some(acp @ LiveActor::Acp(_)) => acp.tell_cancel(None).await,
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
        // 139 §7.4 (W3): the parent's `child_result` wait is consumed here.
        child_waits::child_result_delivered(self, parent, child).await;
        let request = self.bound.remove(&(child, turn));
        let answered = self.delivered.remove(&(child, turn));
        let timed_out = self.timed_out.remove(&child);
        let error = self.child_errors.remove(&(child, turn));
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
        let prefix = if timed_out {
            Some(format!(
                "[timed out after {}s, stop={}]",
                self.cfg.subagents.max_runtime_secs,
                attr(stop_reason)
            ))
        } else {
            failure_prefix(stop_reason, error.as_deref(), &text)
        };
        if let Some(prefix) = prefix {
            text = format!("{prefix}\n{text}");
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
        let scope = owned.map(|o| o.scope);
        let cancelled = owned.is_some_and(|o| o.cancelled);
        if let Err(reason) = self
            .deliver_result(
                child,
                turn,
                parent,
                &text,
                scope,
                cancelled,
                parent_answered,
                false,
            )
            .await
        {
            // R2.7: the parent hears about it, and the result is offered once
            // more when the parent's current turn ends (it may free the quota).
            self.result_refused(
                parent,
                &reason,
                true,
                Undelivered {
                    child,
                    turn,
                    text,
                    scope,
                },
            )
            .await;
        }
    }

    /// Hand `text` — `child`'s result for `turn` — to `parent` through the
    /// durable intake (50 §7): one `result:<turn>` exchange (`:retry` on the
    /// second offer), consumed directly when the parent already took it as
    /// a reply, failed when the owning scope was cancelled. `Err(reason)`
    /// when the intake refused it (quota, chain, no runner); an off route or
    /// a duplicate is `Ok` — nothing is owed.
    #[allow(clippy::too_many_arguments)]
    async fn deliver_result(
        &mut self,
        child: SessionId,
        turn: TurnId,
        parent: SessionId,
        text: &str,
        scope: Option<Uuid>,
        cancelled: bool,
        parent_answered: bool,
        retry: bool,
    ) -> Result<(), String> {
        let generation = self.communication.0.write().route(child, parent);
        let Ok(generation) = generation else {
            tracing::info!(session = %child, %parent, "child result not delivered: communication off");
            return Ok(());
        };
        let (from_key, to_key) = (format!("session:{child}"), format!("session:{parent}"));
        let request_id = if retry {
            format!("result:{turn}:retry")
        } else {
            format!("result:{turn}")
        };
        let receipt = self
            .store
            .accept_exchange(&AcceptExchange {
                sender: &from_key,
                recipient: &to_key,
                sender_session: Some(child),
                recipient_session: Some(parent),
                request_id: Some(&request_id),
                reply_to: None,
                request_scope: scope,
                generation: generation.revision() as i64,
                body: text,
            })
            .await
            .map_err(comm_err)?;
        if receipt.state != PeerMessageState::Accepted || !self.enqueued.insert(receipt.id) {
            // Durable dedup: this turn's result was already accounted for.
            return Ok(());
        }
        self.peer_changed(&receipt).await;
        if cancelled {
            // The scope was cancelled while the child ran: its result is
            // recorded, never a continuation of the cancelled request.
            self.enqueued.remove(&receipt.id);
            match self.store.fail_exchange(receipt.id, "cancelled").await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::error!(id = %receipt.id, "fail_exchange: {e:#}"),
            }
            return Ok(());
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
            return Ok(());
        }
        // 139 §8 "Children" (69 P): the parent moved to another engine — its
        // owner runs the intake. `enqueue_peer` would spawn a local actor for
        // a session this engine no longer owns, whose intake refuses the row.
        if let Some(owner) = self.remote_owner(parent).await {
            let m = Deliver {
                from: child,
                to: to_key,
                text: text.to_owned(),
                await_reply: false,
                reply_to: None,
                request_id: Some(request_id),
                admitted: CancellationToken::new(),
            };
            self.forward_peer(owner, parent, &m, text.to_owned(), receipt, generation)
                .await?;
            return Ok(());
        }
        let record = match self.store.peer_record(parent, receipt.id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                tracing::error!(id = %receipt.id, "accepted child result has no record");
                return Ok(());
            }
            Err(e) => {
                tracing::error!(id = %receipt.id, "peer_record: {e:#}");
                return Ok(());
            }
        };
        if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
            self.enqueued.remove(&receipt.id);
            match self.store.fail_exchange(receipt.id, &reason).await {
                Ok(r) => self.peer_changed(&r).await,
                Err(e) => tracing::error!(id = %receipt.id, "fail_exchange: {e:#}"),
            }
            return Err(reason);
        }
        Ok(())
    }

    /// A child's result the parent's intake refused (R2.7): an error on the
    /// parent naming the child and the reason; with `retry`, the result is
    /// kept for one more offer at the parent's next `TurnFinished`.
    async fn result_refused(
        &mut self,
        parent: SessionId,
        reason: &str,
        retry: bool,
        result: Undelivered,
    ) {
        let name = self
            .sessions
            .get(&result.child)
            .and_then(|l| l.parent.as_ref())
            .map_or_else(|| format!("session:{}", result.child), |(_, n)| n.clone());
        tracing::error!(session = %result.child, %parent, retry, "child result not delivered: {reason}");
        let again = if retry {
            "; offered once more when this turn ends"
        } else {
            ""
        };
        self.error(
            parent,
            format!(
                "result of {name} (session:{}) not delivered: {reason}; read it with session_read{again}",
                result.child
            ),
        )
        .await;
        if retry {
            self.undelivered.entry(parent).or_default().push(result);
        }
    }

    /// `AgentEvent::TurnFinished` for `session`: the child-turn bookkeeping,
    /// the hand-off of a child's result, and the one retry of results this
    /// session refused earlier (R2.7).
    async fn turn_finished(&mut self, session: SessionId, turn: TurnId, stop_reason: &str) {
        self.running.remove(&session);
        if let Some(timer) = self.child_turns.remove(&session).and_then(|ct| ct.timer) {
            timer.abort();
        }
        // 178 F1: an ACP prompt's run clock stops with it (a native turn
        // stops its own at the end of `drive`).
        if matches!(
            self.sessions.get(&session).map(|l| &l.actor),
            Some(LiveActor::Acp(_))
        ) {
            crate::durability::run_clock::stop_session(session);
        }
        self.last_stop
            .insert(session, (stop_reason.to_owned(), Utc::now()));
        self.search.invalidate();
        self.child_finished(session, turn, stop_reason).await;
        for u in self.undelivered.remove(&session).unwrap_or_default() {
            if let Err(reason) = self
                .deliver_result(
                    u.child, u.turn, session, &u.text, u.scope, false, false, true,
                )
                .await
            {
                self.result_refused(session, &reason, false, u).await;
            }
        }
        self.idle_check(session).await;
        // 155 §5.3: idle/done (and running's end) are state-relevant.
        self.check_peer_waits(session).await;
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

/// The theuth MCP shim for an ACP child's `session/new.mcpServers` (75 §9):
/// this binary as `--data-dir <dir> mcp --session <id>`, a stdio MCP server
/// proxying `load_skill` / `list_skills` / `send_message` and (193 §7) the
/// registry's `read_file` / `edit` / `move_text` / `copy_text` / `indent` /
/// `clip` to this engine.
fn mcp_shim(
    data_dir: &Path,
    session: SessionId,
) -> anyhow::Result<agent_client_protocol::schema::v1::McpServer> {
    use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot locate own executable: {e}"))?;
    Ok(McpServer::Stdio(McpServerStdio::new("theuth", exe).args(
        vec![
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().into_owned(),
            "mcp".to_owned(),
            "--session".to_owned(),
            session.to_string(),
        ],
    )))
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

    async fn handle(&mut self, ev: AgentEvent, ctx: &mut Context<Self, Self::Reply>) {
        self.costs.observe(&ev); // 160: ledger / root dimension may have moved
        // 179: the cancel a pause's stop lands on an ACP child is no turn end
        // (it has no suspension of its own): no journal close, no result for
        // the parent — the un-pause re-prompts the open journal turn.
        if let AgentEvent::TurnFinished {
            session,
            stop_reason,
            ..
        } = &ev
            && stop_reason == "cancelled"
            && self.cut_by_pause(*session).await
        {
            tracing::info!(%session, "179: turn cut by the pause; resumes on un-pause");
            self.running.remove(session);
            return;
        }
        // The re-prompted child's start: any cancel of the stopped actor came
        // before it on the bus, so a later `cancelled` is a real turn end.
        if let AgentEvent::TurnStarted { session, .. } | AgentEvent::TurnResumed { session, .. } =
            &ev
        {
            self.acp_cut.remove(session);
        }
        self.acp_journal_event(&ev).await; // 146 D9
        if self.boot.is_some() {
            self.boot_child_event(&ev).await; // 146 D2
        }
        match ev {
            // 109 LIVE: a turn resumed after a restart (`Turn::resume`)
            // publishes `TurnResumed`, never `TurnStarted`; for liveness it is
            // the start of a turn all the same.
            AgentEvent::TurnStarted { session, .. } => {
                self.turn_began(session, None, ctx.actor_ref().clone())
                    .await;
            }
            AgentEvent::TurnResumed {
                session,
                turn,
                interrupted_at,
                ..
            } => {
                self.turn_began(
                    session,
                    Some((turn, interrupted_at)),
                    ctx.actor_ref().clone(),
                )
                .await;
            }
            AgentEvent::TurnFinished {
                session,
                turn,
                stop_reason,
                ..
            } => self.turn_finished(session, turn, &stop_reason).await,
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
            // 178 W6: what a failed child turn hands its parent says why.
            AgentEvent::Error {
                session,
                turn: Some(turn),
                message,
            } if self
                .sessions
                .get(&session)
                .is_some_and(|l| l.parent.is_some()) =>
            {
                self.child_errors.insert((session, turn), message);
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
        self.approvals.insert(
            session,
            (ev.id, ev.tool, Utc::now().timestamp_millis().max(0) as u64),
        );
        self.network_approval(session, true).await;
        self.check_peer_waits(session).await; // 155 §5.3: `blocked`
    }
}

impl Message<ApprovalResolved> for Agents {
    type Reply = ();

    async fn handle(&mut self, ev: ApprovalResolved, _: &mut Context<Self, Self::Reply>) {
        let sessions: Vec<SessionId> = self
            .approvals
            .iter()
            .filter(|(_, (id, ..))| *id == ev.id)
            .map(|(s, _)| *s)
            .collect();
        self.approvals.retain(|_, (id, ..)| *id != ev.id);
        for s in sessions {
            self.network_approval(s, false).await;
            self.check_peer_waits(s).await; // 155 §5.3: `blocked` ended
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
        self.rearm_reminders_for(None).await;
        self.rebuild_peer_waits().await; // 155 §5.4
        // 146 D5: one `open_turns(Here)` for the whole pass.
        let open = self.open_here().await;
        // 179 D8: paused agents' trees were held at construction (from the
        // agent files), before the reminders above re-armed.
        // 109 §4.6 step 10, in this order: no `ResumeTurn` before the reaper.
        let reaped = crate::durability::jobs::reap_jobs_in(&self.store, &open).await;
        if reaped != crate::durability::jobs::ReapReport::default() {
            tracing::info!("109: reaper: {reaped:?}");
        }
        self.rebuild_waits(&open).await;
        // 146 D9: one list, one pass — an ACP child's open turn (it owed a
        // prompt when the engine died) is the respawn's, a native one the
        // resume's.
        let (acp_open, native) = self.split_acp_turns(&open).await;
        let (acp_open, native) = self.unpaused_turns(acp_open, native).await;
        self.resume_scan(&native).await;
        let resuming = match self.cfg.durability.resume {
            crate::durability::ResumeMode::Auto => native.len() as u32,
            _ => 0,
        };
        self.respawn_acp_children(resuming, acp_open).await; // 109 W4 §7.3, 146 D1–D4, D9
        // 141 §7.8: watch-only children of a harness that did not come back.
        self.mark_observed_lost().await;
        // 109 §7.2: the spared child results, after the open turns are queued.
        self.deliver_spared_results(&open).await;
    }
}

impl Agents {
    /// Restore stored reminders after a restart (14 as-built 2026-09-23) —
    /// every row when `session` is `None` — or arm one taken session's
    /// ([`RearmReminders`], 139 Revision 2): rows of sessions that no longer
    /// exist go; rows of sessions owned elsewhere stay unarmed, they fire on
    /// the owner (139 Q3); the rest are re-armed by the jobs actor, which
    /// fires overdue ones at once as `late`. A `tell`: the actor admits them
    /// off its mailbox by asking back into us.
    pub(crate) async fn rearm_reminders_for(&mut self, session: Option<SessionId>) {
        let rows = match self.store.reminders().await {
            Ok(rows) => rows,
            Err(e) => return tracing::error!("stored reminders unreadable: {e:#}"),
        };
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| session.is_none_or(|s| s == r.session))
            .collect();
        if rows.is_empty() {
            return;
        }
        let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) else {
            return tracing::error!("jobs actor not running: stored reminders not re-armed");
        };
        for r in rows {
            match self.store.session(r.session).await {
                Ok(Some(row)) if row.here => {}
                Ok(Some(_)) => continue,
                _ => {
                    let _ = self.store.delete_reminder(r.session, &r.id).await;
                    continue;
                }
            }
            // 179 D6: a paused agent's reminders wait for its un-pause
            // (`ResumeTree` re-arms them); the row stays.
            if self.paused_root(r.session).await.is_some() {
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

/// `tell` from `sessionsync::owner::Take::take` once `set_owner` landed:
/// the session's reminders now fire here (139 Revision 2 D4). The old
/// owner's timers go with its demote (`tools::CancelReminders`).
pub struct RearmReminders {
    /// The taken session.
    pub session: SessionId,
}

impl Message<RearmReminders> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: RearmReminders, _: &mut Context<Self, Self::Reply>) {
        self.rearm_reminders_for(Some(m.session)).await;
    }
}

/// `tell` from the shipper for a received `Msg::Harness` (139 Revision 2):
/// the body goes through `inject` like a local one — and on to the next
/// owner if the session moved again meanwhile. The sender's request scope
/// is its ledger's, not ours: delivered unscoped.
#[derive(Debug, serde::Deserialize)]
pub struct HarnessDeliver {
    /// The session.
    pub session: SessionId,
    /// The `from` label (`job j3`, `reminder r1`, `session:<child>`).
    pub kind: String,
    /// The bare body.
    pub text: String,
    /// The body as the model reads it.
    pub wrapped: String,
    /// 171 §9.3 (W-m): `false` = quiet — parked until the session's next turn opens
    /// for any reason, which it joins in arrival order; it never starts one itself.
    /// A running turn takes it at its next round either way.
    #[serde(default = "wake_default")]
    pub wake: bool,
}

fn wake_default() -> bool {
    true
}

/// 171 §9.3: the `kind` prefix that carries `wake: false` through `harness_outbox` and
/// `Msg::Harness` (139), whose rows and frames have no wake field.
pub const QUIET_KIND: &str = "quiet ";

impl Message<HarnessDeliver> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: HarnessDeliver, _: &mut Context<Self, Self::Reply>) {
        let (kind, wake) = match m.kind.strip_prefix(QUIET_KIND) {
            Some(kind) => (kind, false),
            None => (m.kind.as_str(), m.wake),
        };
        self.inject_as(m.session, kind, m.text, m.wrapped, None, wake)
            .await;
    }
}

#[cfg(test)]
mod harness_deliver_tests {
    use super::HarnessDeliver;

    #[test]
    fn wake_defaults_to_true() {
        let m: HarnessDeliver = serde_json::from_str(
            r#"{"session":"00000000-0000-0000-0000-000000000000","kind":"job j3","text":"t","wrapped":"w"}"#,
        )
        .unwrap();
        assert!(m.wake);
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
            // The fired row is already deleted with its `Settle::Reminder`.
            ReminderEvent::Fired { .. } => Ok(()),
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
            // 171 HANDOFF-Z-1: an owner with no row (a note cell kernel's UUIDv5 session)
            // is its own root; left unresolved it would block every session's activity.
            Ok(None) => self.ledger.register(session, None, false),
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

/// L1 (171 §12.1 inv. 3): the reserved `start` name prefix of a user-cell kernel job.
pub const CELL_JOB_PREFIX: &str = "cell:";

/// Per session, the job ids whose `JobEvent::Started` carried a [`CELL_JOB_PREFIX`] name:
/// user-cell kernels, whose end or silence is never model input (`<job_result>`,
/// `<job_idle>`) — the cells actor delivers `<cell_result>` instead.
#[derive(Default)]
pub(crate) struct CellJobs(parking_lot::Mutex<HashMap<SessionId, HashSet<JobId>>>);

impl CellJobs {
    /// Remember a cell job announced by `ev`.
    pub(crate) fn observe(&self, ev: &JobEvent) {
        if let JobEvent::Started {
            session,
            id,
            name: Some(name),
            ..
        } = ev
            && name.starts_with(CELL_JOB_PREFIX)
        {
            self.0
                .lock()
                .entry(*session)
                .or_default()
                .insert(id.clone());
        }
    }

    /// Whether `id` of `session` is a cell job (still running).
    fn is_cell(&self, session: SessionId, id: &JobId) -> bool {
        self.0
            .lock()
            .get(&session)
            .is_some_and(|ids| ids.contains(id))
    }

    /// [`Self::is_cell`] for a job that ended; forgets it.
    fn finished(&self, session: SessionId, id: &JobId) -> bool {
        let mut map = self.0.lock();
        let Some(ids) = map.get_mut(&session) else {
            return false;
        };
        let hit = ids.remove(id);
        if ids.is_empty() {
            map.remove(&session);
        }
        hit
    }
}

/// The bus subscriber that feeds [`CellJobs`], so the supervisor itself stays off the
/// busy `JobEvent` stream (finished jobs reach it through `Settle`).
#[derive(kameo::Actor)]
struct CellJobWatch(Arc<CellJobs>);

impl Message<JobEvent> for CellJobWatch {
    type Reply = ();

    async fn handle(&mut self, ev: JobEvent, _: &mut Context<Self, Self::Reply>) {
        self.0.observe(&ev);
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
                // L1 (171 inv. 3): a user-cell kernel ending is the cells actor's
                // `<cell_result>`, never a `<job_result>`.
                let cell = matches!(&result, JobEvent::Finished { session, id, .. }
                    if self.cell_jobs.finished(*session, id));
                if !cell
                    && let JobEvent::Finished {
                        session,
                        id,
                        command,
                        exit,
                        secs,
                        output,
                        inline: false,
                        kind,
                        ..
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
                    let wrapped = job_result(
                        &id,
                        &command,
                        &exit,
                        secs,
                        &text,
                        kind,
                        chrono::Local::now(),
                    );
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
                        chrono::Local::now(),
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
                    why,
                },
        } = m
        else {
            return;
        };
        if self.ledger.op_owner(op).is_none() {
            tracing::info!(%op, "idle notice for a settled job dropped");
            return;
        }
        // L1 (171 inv. 3): a user-cell kernel waiting on its user is no `<job_idle>`.
        if name
            .as_deref()
            .is_some_and(|n| n.starts_with(CELL_JOB_PREFIX))
            || self.cell_jobs.is_cell(session, &id)
        {
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
            &why,
            &text,
            chrono::Local::now(),
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
/// attribute-escaped; `exit` is the code, `timeout` or `killed`; `secs` the
/// job's wall time; `at` the local delivery clock (74 §4). PTY jobs
/// (22 §2.2) carry ` pty="true"` and `text` is the final screen. A promoted
/// tool call (145 §0.4) names `tool="…"` instead of `command=`.
pub(crate) fn job_result(
    id: &JobId,
    command: &str,
    exit: &JobExit,
    secs: u64,
    text: &str,
    kind: JobKind,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let cmd: String = command.chars().take(120).collect();
    let exit = match exit {
        JobExit::Exited(code) => code.to_string(),
        JobExit::Timeout => "timeout".into(),
        JobExit::Killed => "killed".into(),
        JobExit::Lost => "lost".into(),
    };
    let (what, pty) = match kind {
        JobKind::Shell => ("command", ""),
        JobKind::Pty => ("command", " pty=\"true\""),
        JobKind::Promoted => ("tool", ""),
    };
    format!(
        "<job_result id=\"{id}\" {what}=\"{}\" exit=\"{exit}\" secs=\"{secs}\"{pty} at=\"{}\">\n{text}\n</job_result>",
        attr(&cmd),
        clock::hhmm(now)
    )
}

/// The `<job_idle>` wrapper (61 §2.2): `command` cut and escaped as
/// [`job_result`]; `name` and `user_typed` only when present; `why` (155
/// §6.5) only when non-empty; `at` the local clock (74 §4); `text` is the
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
    why: &str,
    text: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let cmd: String = command.chars().take(120).collect();
    let name = name.map_or_else(String::new, |n| format!(" name=\"{}\"", attr(n)));
    let typed = user_typed.map_or_else(String::new, |t| format!(" user_typed=\"{t}\""));
    let why = if why.is_empty() {
        String::new()
    } else {
        format!(" why=\"{}\"", attr(why))
    };
    format!(
        "<job_idle id=\"{id}\" command=\"{}\"{name} quiet=\"{quiet}\" secs=\"{secs}\" nudge=\"{nudge}\"{why}{typed} at=\"{}\">\n{text}\n</job_idle>",
        attr(&cmd),
        clock::hhmm(now)
    )
}

/// The `<child_stalled>` wrapper (155 §4.1): the parent's wake for a child
/// whose activity monitor fired; `last` only when non-empty; `body` is the
/// one sentence of guidance.
pub(crate) fn child_stalled(
    name: &str,
    session: SessionId,
    phase: crate::activity::StallPhase,
    quiet_secs: u64,
    last: &str,
    body: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let last = if last.is_empty() {
        String::new()
    } else {
        format!(" last=\"{}\"", attr(last))
    };
    format!(
        "<child_stalled child=\"{}\" session=\"{session}\" phase=\"{}\" quiet_secs=\"{quiet_secs}\"{last} at=\"{}\">\n{body}\n</child_stalled>",
        attr(name),
        phase.as_str(),
        clock::hhmm(now)
    )
}

/// The `<peer_state>` wrapper (155 §5.3): a `wait_peer` settled — the word
/// it reached (or `not_running` / `timeout`) and the one-line reason.
pub(crate) fn peer_state(
    name: &str,
    session: SessionId,
    state: crate::activity::PeerStateWord,
    since_secs: u64,
    body: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    format!(
        "<peer_state child=\"{}\" session=\"{session}\" state=\"{}\" since_secs=\"{since_secs}\" at=\"{}\">\n{body}\n</peer_state>",
        attr(name),
        state.as_str(),
        clock::hhmm(now)
    )
}

/// The `<reminder>` wrapper (14 as-built 2026-09-23): `after` is the
/// requested delay in seconds, `set_at` RFC 3339 local, `late` true when it
/// was re-armed past due after a restart, `at` the local clock when it fired
/// (74 §4); `text` is the model's message.
pub(crate) fn reminder(
    id: &str,
    after: u64,
    set_at: &str,
    late: bool,
    text: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    format!(
        "<reminder id=\"{}\" after=\"{after}\" set_at=\"{}\" late=\"{late}\" at=\"{}\">\n{text}\n</reminder>",
        attr(id),
        attr(set_at),
        clock::hhmm(now)
    )
}

/// XML attribute escape.
fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 178 W6: the line that leads a child result whose turn did not end well —
/// an `error` (with the turn's last error, first line, ≤ 200 chars), a
/// `cancelled` (the runtime cap has its own prefix) or no reply at all.
/// `None` for an ordinary result.
fn failure_prefix(stop_reason: &str, error: Option<&str>, text: &str) -> Option<String> {
    match stop_reason {
        "error" => {
            let why: String = error
                .and_then(|e| e.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("unknown error")
                .trim()
                .chars()
                .take(200)
                .collect();
            Some(format!("[child turn ended: error — {why}]"))
        }
        "cancelled" => Some("[child turn cancelled]".into()),
        _ if text.trim().is_empty() => Some("[child turn ended with no reply]".into()),
        _ => None,
    }
}

impl Message<SendInput> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: SendInput, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.human_input(m.session, m.id, m.text, m.files, m.channel, m.delivery)
            .await
    }
}

impl Message<crate::LateAnswer> for Agents {
    type Reply = Result<(), String>;

    /// 132 D6: the journal's `questions` row names the session and carries the
    /// `AskRequested` payload; the answers become one user message on that
    /// session, routed like a chat prompt. An unknown or payload-less row is a no-op.
    async fn handle(
        &mut self,
        m: crate::LateAnswer,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let row = self
            .store
            .question_get(&m.id.0.to_string())
            .await
            .map_err(|e| format!("store: {e:#}"))?;
        let Some(row) = row else { return Ok(()) };
        let Ok(asked) = serde_json::from_value::<tools::AskRequested>(row.payload) else {
            return Ok(());
        };
        let text = tools::late_answer_text(&asked.questions, &m.answers);
        if text.is_empty() {
            return Ok(());
        }
        self.human_input(
            Some(row.session),
            None,
            text,
            Vec::new(),
            None,
            Delivery::default(),
        )
        .await
        .map(|_| ())
    }
}

impl Message<UseSkill> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: UseSkill, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let text = skills::use_text(&self.skills_dir(), &m.names, &m.args)?;
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
                None,
                InputOrigin::External,
                Delivery::Steer,
            )
            .await;
        Ok(sid)
    }
}

impl Message<Steer> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Steer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.human_input(
            Some(m.session),
            Some(m.id),
            m.text,
            m.files,
            m.channel,
            m.delivery,
        )
        .await
        .map(|_| ())
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

impl Message<SetTalk> for Agents {
    type Reply = ();

    /// 145 §0.3–0.4. `open: true` and `live: None` release what was parked;
    /// `open: false` with items held for coalescing re-arms the cap. The
    /// registry learns the session's promotion delay (`None` switches
    /// promotion off).
    async fn handle(&mut self, m: SetTalk, ctx: &mut Context<Self, Self::Reply>) {
        let sid = m.session;
        let after = match m.live {
            Some(live) => {
                let me = ctx.actor_ref().clone();
                let t = self.talk.entry(sid).or_insert_with(|| Talk {
                    live,
                    parked: Vec::new(),
                    timer: None,
                    epoch: 0,
                    me,
                });
                let was_open = t.live.open;
                t.live = live;
                if live.open {
                    self.release_parked(sid).await;
                } else if was_open && !t.parked.is_empty() {
                    t.arm(sid, TALK_PARK_CAP_MS);
                }
                live.promote_after_ms.map(Duration::from_millis)
            }
            None => {
                if let Some(mut t) = self.talk.remove(&sid) {
                    let parked = t.take();
                    self.route_parked(sid, parked).await;
                }
                None
            }
        };
        // No "What's next?" suggestions on spoken turns while the talk lasts.
        let talking = self.talk.contains_key(&sid);
        if let Some(Ok(native)) = self.live(sid).map(LiveActor::native) {
            let _ = native.tell(SetTalking(talking)).await;
        }
        if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
            let _ = reg
                .tell(SetPromoteAfter {
                    session: sid,
                    after,
                })
                .await;
        }
    }
}

impl Message<ReleaseParked> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ReleaseParked, _: &mut Context<Self, Self::Reply>) {
        if self
            .talk
            .get(&m.session)
            .is_some_and(|t| t.epoch == m.epoch)
        {
            self.release_parked(m.session).await;
        }
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

impl Message<TrimAssistant> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: TrimAssistant,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.search.invalidate();
        // A context edit: candidates admitted against the full reply are stale.
        self.ledger.bump(m.session);
        self.actor(m.session)
            .await?
            .native()?
            .ask(TrimTurn {
                turn: m.turn,
                keep: m.keep,
            })
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<ForkSession> for Agents {
    type Reply = Result<SessionId, String>;

    async fn handle(
        &mut self,
        m: ForkSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
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
        let (scope, opted_out) = {
            let mut p = self.communication.0.write();
            match p.record_fork(m.session, id) {
                Ok(()) => (
                    p.root_of(id)
                        .ok()
                        .and_then(|r| p.state(Some(r)).ok())
                        .map(|s| s.scope),
                    p.state(Some(id)).is_ok_and(|s| s.opted_out),
                ),
                Err(e) => {
                    tracing::error!(session = %id, "record_fork: {e}");
                    (None, false)
                }
            }
        };
        // 183 R1 (S2): the fork's opt-out on its row, as the setter writes
        // it, so a restart (or the row sync in `mirror`) keeps it off.
        if opted_out && let Err(e) = self.store.set_session_communication(id, Some(false)).await {
            tracing::error!(session = %id, "set_session_communication: {e:#}");
        }
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
        self.cancel(m).await;
    }
}

impl Agents {
    /// [`Cancel`].
    pub(crate) async fn cancel(&mut self, mut m: Cancel) {
        // `Tree`: every running descendant, not one level (design doc 19 §2.3).
        // `Root` (the default): the target alone; its children keep working and
        // their later `TurnFinished` reaches the parent as when it is idle (133 §2).
        let tree = match m.scope {
            CancelScope::Root => Vec::new(),
            CancelScope::Tree => self
                .store_for(m.session)
                .descendants(m.session)
                .await
                .unwrap_or_default(),
        };
        // 141 §7.7: a watch-only child has no actor; its stop goes to the
        // harness that runs it. The target itself is always forwarded (the
        // actor answers "cannot stop" when the harness offers no per-child
        // cancel); tree descendants only when the harness said it can.
        let mut observed: Vec<SessionSummary> = tree
            .iter()
            .filter(|(_, s)| s.observed.as_ref().is_some_and(|o| o.can_cancel))
            .map(|(_, s)| s.clone())
            .collect();
        if m.scope == CancelScope::Root
            && self.live(m.session).is_none()
            && let Ok(Some(row)) = self.store_for(m.session).session(m.session).await
            && row.observed.is_some()
        {
            observed.push(row);
        }
        let ids: Vec<SessionId> = std::iter::once(m.session)
            .chain(tree.into_iter().map(|(_, s)| s.id))
            .collect();
        for &id in &ids {
            // Stop control (47 §4.3): candidates admitted before this are stale.
            self.ledger.bump(id);
            // A queued spawn turn is this stop's work too (01 §2.6): it never starts.
            self.drop_queued(id).await;
            if let Some(live) = self
                .sessions
                .get(&id)
                .filter(|_| id == m.session || self.running.contains(&id))
            {
                // 76 §7: only the target keeps its heard prefix; descendants drop theirs.
                let keep = if id == m.session { m.keep.take() } else { None };
                live.actor.tell_cancel(keep).await;
            }
        }
        for row in observed {
            self.cancel_observed(row).await;
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
                head: m.head,
            }))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<crate::SetBudget> for Agents {
    type Reply = Result<(), String>;

    /// 136 §2.2: a session's money cap. `since` is stamped here (the cap is
    /// per night, R-1) and the cap is persisted on the session row so a
    /// durability-resumed turn is still metered (R-22).
    async fn handle(
        &mut self,
        m: crate::SetBudget,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let budget = m.nanos.map(|nanos| crate::Budget {
            nanos,
            currency: m.currency,
            since: chrono::Utc::now().to_rfc3339(),
        });
        let actor = self.actor(m.session).await?.native()?.clone();
        self.store
            .set_budget(m.session, budget.as_ref())
            .await
            .map_err(|e| format!("{e:#}"))?;
        actor
            .tell(SetBudgetNow(budget))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<crate::LedgerSince> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<crate::Ledger, String>>;

    /// 136 §2.2: the day ledger — the durable store's read-only aggregates
    /// (`Store::ledger_since`; private trees live in their own stores and
    /// are never in it) plus knowledge `Stats` for the ids the window's
    /// `<knowledge>` notes showed. Knowledge off → `hints` empty. The scan
    /// takes seconds on a large store, so it runs off the mailbox (R-21).
    async fn handle(
        &mut self,
        m: crate::LedgerSince,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let store = self.store.clone();
        let vault = self.auth.vault.clone();
        tokio::spawn(async move {
            let redact = move |text: &str| -> String {
                vault
                    .as_deref()
                    .map_or_else(|| text.to_owned(), |v| v.redact(text).into_owned())
            };
            let (mut ledger, hinted) = match store.ledger_since(&m, &redact).await {
                Ok(r) => r,
                Err(e) => {
                    tx.send(Err(format!("ledger: {e:#}")));
                    return;
                }
            };
            if !hinted.is_empty()
                && let Ok(Some(k)) =
                    ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
            {
                match k.ask(knowledge::Stats { ids: hinted }).await {
                    Ok(uses) => {
                        ledger.hints = uses
                            .into_iter()
                            .map(|u| crate::LedgerHintUse {
                                id: u.id,
                                shown: u.shown,
                                opened: u.opened,
                            })
                            .collect();
                    }
                    Err(e) => tracing::warn!("ledger: knowledge stats: {e}"),
                }
            }
            tx.send(Ok(ledger));
        });
        reply
    }
}

impl Message<ResetRuntime> for Agents {
    type Reply = Result<repl::ResetReport, String>;

    async fn handle(
        &mut self,
        m: ResetRuntime,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<repl::ResetReport, String> {
        self.actor(m.session)
            .await?
            .native()?
            .ask(ResetRuntimeNow)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<SetToolset> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetToolset,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.actor(m.session)
            .await?
            .native()?
            .tell(SetToolsetOverride(m.toolset))
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<PinTitle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: PinTitle,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.actor(m.session)
            .await?
            .native()?
            .tell(SetTitlePin(m.pinned))
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

impl Message<crate::SetStatusEvery> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: crate::SetStatusEvery,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        Arc::make_mut(&mut self.cfg).status_every = m.count;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) =
            runtime::write_config_value(&path, "agent", "status_every", &m.count.to_string())
        {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<crate::SetHintLimit> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: crate::SetHintLimit,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        Arc::make_mut(&mut self.cfg).knowledge.hint_limit = m.count;
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_nested_config_value(
            &path,
            "agent",
            "knowledge",
            "hint_limit",
            &m.count.to_string(),
        ) {
            tracing::warn!("config.yaml: {e}");
        }
        Ok(())
    }
}

impl Message<crate::SetLoopFlag> for Agents {
    type Reply = ();

    /// 212 §15: config.yaml is the caller's; sessions started from now on copy this.
    async fn handle(&mut self, m: crate::SetLoopFlag, _: &mut Context<Self, Self::Reply>) {
        let cfg = Arc::make_mut(&mut self.cfg);
        match m.flag {
            crate::LoopFlag::StatusTool => cfg.status_tool = m.on,
            crate::LoopFlag::AutoTitle => cfg.auto_title = m.on,
            crate::LoopFlag::Suggestions => cfg.suggestions = m.on,
            crate::LoopFlag::KnowledgePeek => cfg.knowledge_peek = m.on,
        }
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
        // 194: every write first (the model, then a dropped effort), then publish.
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        runtime::write_config_value(&path, "agent", "title_model", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        let cleared = self.follow_role_model(resolved.as_ref(), TITLE_EFFORT)?;
        *self.title_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        // 137F E1: the compactor chain's second link, live.
        self.endless
            .set_title_model(info.as_ref().map(|i| i.id.clone()));
        if cleared {
            self.clear_role_effort(TITLE_EFFORT);
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

/// 194: `agent.title_effort`'s config key and live slot.
const TITLE_EFFORT: RoleEffortKey = ("agent", "title_effort", |r| &mut r.title);
/// 194: `voice.polish_effort`.
const POLISH_EFFORT: RoleEffortKey = ("voice", "polish_effort", |r| &mut r.polish);
/// 194: `agent.vision_effort`.
const VISION_EFFORT: RoleEffortKey = ("agent", "vision_effort", |r| &mut r.vision);
/// 194: `agent.escalate_effort`.
const ESCALATE_EFFORT: RoleEffortKey = ("agent", "escalate_effort", |r| &mut r.escalate);
/// `agent.knowledge_harvester_effort`.
const KNOWLEDGE_HARVESTER_EFFORT: RoleEffortKey = ("agent", "knowledge_harvester_effort", |r| {
    &mut r.knowledge_harvester
});

/// A role effort's `(section, key)` in config.yaml and its field of the live slot.
type RoleEffortKey = (
    &'static str,
    &'static str,
    fn(&mut crate::loop_::RoleEfforts) -> &mut Option<String>,
);

/// The model in `slot`, cloned out (never held across a call).
fn resolved_effort_model(slot: &RwLock<Option<Resolved>>) -> Option<Resolved> {
    slot.read().unwrap_or_else(|p| p.into_inner()).clone()
}

impl Agents {
    /// 194 D4b/D4g: refuse an effort `model` (the setting's effective model)
    /// does not take — `validate_effort`'s message; with no catalog entry, an
    /// unknown word — then persist `key` and swap the live slot. A refusal or a
    /// failed write changes nothing. Replies the stored value.
    fn set_role_effort(
        &mut self,
        model: Option<Resolved>,
        (section, key, field): RoleEffortKey,
        effort: Option<String>,
    ) -> Result<Option<String>, String> {
        if let Some(e) = &effort {
            match model.and_then(|r| r.model) {
                Some(m) => m.validate_effort(e)?,
                None if effort_of(e).is_none() => return Err(format!("unknown effort {e}")),
                None => {}
            }
        }
        // Quoted so a bare budget (`"8000"`) stays a YAML string; `~` clears.
        let value = effort
            .as_ref()
            .map_or_else(|| "~".to_owned(), |e| format!("\"{e}\""));
        let path = self.rt.data_dir.join("config.yaml");
        runtime::write_config_value(&path, section, key, &value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        *field(&mut self.role_efforts.write()) = effort.clone();
        Ok(effort)
    }

    /// 194 D4c/D4d, before the role's model becomes `model`: `None` (its
    /// fallback row) clears the stored effort; a model that does not take it
    /// drops it. Persists `~` and replies whether the live slot must clear —
    /// the caller publishes that ([`Agents::clear_role_effort`]) with its
    /// model, once every write landed; a failed write changes nothing live.
    fn follow_role_model(
        &self,
        model: Option<&Resolved>,
        (section, key, field): RoleEffortKey,
    ) -> Result<bool, String> {
        let mut efforts = self.role_efforts.read().clone();
        let Some(e) = field(&mut efforts).take() else {
            return Ok(false);
        };
        let keeps = model.is_some_and(|r| {
            r.model
                .as_ref()
                .is_none_or(|m| m.validate_effort(&e).is_ok())
        });
        if keeps {
            return Ok(false);
        }
        let path = self.rt.data_dir.join("config.yaml");
        runtime::write_config_value(&path, section, key, "~")
            .map_err(|e| format!("config.yaml: {e}"))?;
        Ok(true)
    }

    /// Publish a role effort [`Agents::follow_role_model`] cleared.
    fn clear_role_effort(&self, (_, _, field): RoleEffortKey) {
        *field(&mut self.role_efforts.write()) = None;
    }

    /// The live value of one role effort.
    fn role_effort(&self, (_, _, field): RoleEffortKey) -> Option<String> {
        let mut efforts = self.role_efforts.read().clone();
        field(&mut efforts).take()
    }
}

impl Message<crate::SetTitleEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: crate::SetTitleEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        let model = resolved_effort_model(&self.title_model);
        self.set_role_effort(model, TITLE_EFFORT, m.effort)
    }
}

impl Message<crate::GetTitleEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: crate::GetTitleEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.role_effort(TITLE_EFFORT)
    }
}

impl Message<SetPolishModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: SetPolishModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let resolved = match &m.model {
            Some(id) => Some(self.catalog.resolve(id, &*self.configured)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        // 194: every write first (the model, then a dropped effort), then publish.
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        runtime::write_config_value(&path, "voice", "polish_model", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        let cleared = self.follow_role_model(resolved.as_ref(), POLISH_EFFORT)?;
        *self.polish_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        if cleared {
            self.clear_role_effort(POLISH_EFFORT);
        }
        Ok(info)
    }
}

impl Message<GetPolishModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: GetPolishModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.polish_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Message<crate::SetPolishEnabled> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: crate::SetPolishEnabled,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let path = self.rt.data_dir.join("config.yaml");
        let value = if m.enabled { "true" } else { "false" };
        runtime::write_config_value(&path, "voice", "polish_enabled", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        self.polish_enabled = m.enabled;
        Ok(())
    }
}

impl Message<crate::GetPolishEnabled> for Agents {
    type Reply = bool;

    async fn handle(&mut self, _: crate::GetPolishEnabled, _: &mut Context<Self, bool>) -> bool {
        self.polish_enabled
    }
}

impl Message<crate::SetPolishEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: crate::SetPolishEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        // Its effective model: polish → title (the session's own when neither).
        let model = resolved_effort_model(&self.polish_model)
            .or_else(|| resolved_effort_model(&self.title_model));
        self.set_role_effort(model, POLISH_EFFORT, m.effort)
    }
}

impl Message<crate::GetPolishEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: crate::GetPolishEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.role_effort(POLISH_EFFORT)
    }
}

/// 153 §9: one `CallKind::Polish` side call on `ModelRole::Polish`, charged to
/// `session` (theuth's desk when `None`), run detached; the reply fails closed
/// to `refused` so the caller keeps the shaped text.
impl Message<Polish> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<PolishReply, String>>;

    async fn handle(&mut self, m: Polish, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        // 204: the global switch is off → never a model call; the caller keeps its text.
        if !self.polish_enabled {
            if let Some(tx) = tx {
                tx.send(Ok(refused("polish off")));
            }
            return reply;
        }
        // §9.1: the agent reads raw text fine; never a model call.
        let target = if m.destination == voice::Destination::AgentPrompt {
            Err("agent prompt".to_owned())
        } else {
            self.polish_target(m.session).await
        };
        let cfg = self.voice_config();
        let system = polish::polish_system(&cfg, &m.destination);
        let user = polish::polish_user(&m.text, &m.destination);
        tokio::spawn(async move {
            let out = match target {
                Ok((session, actor)) => {
                    let call = SideCall {
                        session,
                        role: ModelRole::Polish,
                        system,
                        user,
                        budget: polish::POLISH_BUDGET,
                        kind: CallKind::Polish,
                    };
                    polish_call(actor, call, |t| polish::check_polish(&m.text, t)).await
                }
                Err(why) => refused(why),
            };
            if let Some(tx) = tx {
                tx.send(Ok(out));
            }
        });
        reply
    }
}

/// 153 §13.6: the spoken instruction is the user message, the selection goes
/// into the system prompt through `voice.rewrite_template`; charged to theuth's desk.
impl Message<Rewrite> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<PolishReply, String>>;

    async fn handle(&mut self, m: Rewrite, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let target = self.polish_target(None).await;
        let cfg = self.voice_config();
        let system = polish::rewrite_system(&cfg, &m.selection, &m.app, m.thread.as_deref());
        tokio::spawn(async move {
            let out = match target {
                Ok((session, actor)) => {
                    let call = SideCall {
                        session,
                        role: ModelRole::Polish,
                        system,
                        user: m.instruction,
                        budget: polish::POLISH_BUDGET,
                        kind: CallKind::Polish,
                    };
                    polish_call(actor, call, |t| polish::check_rewrite(&m.selection, t)).await
                }
                Err(why) => refused(why),
            };
            if let Some(tx) = tx {
                tx.send(Ok(out));
            }
        });
        reply
    }
}

impl Agents {
    /// The session a polish call is charged to and runs on: `session`, else
    /// theuth's desk (W-h), resumed from the store when idle. `Err` = why
    /// polish is refused (no desk, external harness, closed, …).
    async fn polish_target(
        &mut self,
        session: Option<SessionId>,
    ) -> Result<(SessionId, ActorRef<Session>), String> {
        let id = match session {
            Some(id) => id,
            None => self
                .store
                .desk_session("theuth")
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "no session".to_owned())?,
        };
        let (id, live) = self
            .session(Some(id), None, None, (None, None))
            .await
            .map_err(|e| e.to_string())?;
        let actor = live.native()?.clone();
        Ok((id, actor))
    }

    /// The live `voice.` section (polish profiles, rewrite template); defaults when unreadable.
    fn voice_config(&self) -> voice::VoiceConfig {
        self.rt
            .section::<voice::VoiceConfig>("voice")
            .unwrap_or_default()
    }
}

fn refused(why: impl Into<String>) -> PolishReply {
    PolishReply {
        refused: Some(why.into()),
        ..PolishReply::default()
    }
}

/// Ask `actor` the call and shape the answer: `timed out` → `refused: timeout`
/// (§9.5), a provider error → `refused: <error>`, a reply that fails `check` →
/// `refused: <why>`; `ms` and `model` are set whenever the model answered.
async fn polish_call(
    actor: ActorRef<Session>,
    call: SideCall,
    check: impl FnOnce(&str) -> Result<String, &'static str>,
) -> PolishReply {
    let out = match actor.ask(call).await {
        Ok(r) => r,
        Err(SendError::HandlerError(e)) if e == "timed out" => return refused("timeout"),
        Err(SendError::HandlerError(e)) => return refused(e),
        Err(e) => return refused(e.to_string()),
    };
    let ms = u32::try_from(out.latency.as_millis()).unwrap_or(u32::MAX);
    let model = Some(out.model);
    match check(&out.text) {
        Ok(text) => PolishReply {
            text: Some(text),
            model,
            ms,
            refused: None,
        },
        Err(why) => PolishReply {
            text: None,
            model,
            ms,
            refused: Some(why.to_owned()),
        },
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
        let cleared = self.follow_role_model(resolved.as_ref(), ESCALATE_EFFORT)?;
        *self
            .escalate_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = resolved;
        if cleared {
            self.clear_role_effort(ESCALATE_EFFORT);
        }
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

impl Message<crate::SetEscalateEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: crate::SetEscalateEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        // Its effective model: escalate → title (the session's own when neither).
        let model = resolved_effort_model(&self.escalate_model)
            .or_else(|| resolved_effort_model(&self.title_model));
        self.set_role_effort(model, ESCALATE_EFFORT, m.effort)
    }
}

impl Message<crate::GetEscalateEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: crate::GetEscalateEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.role_effort(ESCALATE_EFFORT)
    }
}

impl Message<crate::SetKnowledgeHarvesterModel> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: crate::SetKnowledgeHarvesterModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let resolved = match &m.model {
            Some(id) => Some(self.catalog.resolve(id, &*self.configured)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        // 194: every write first (the model, then a dropped effort), then publish.
        let path = self.rt.data_dir.join("config.yaml");
        let value = info.as_ref().map(|i| i.id.as_str()).unwrap_or_default();
        runtime::write_config_value(&path, "agent", "knowledge_harvester_model", value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        let cleared = self.follow_role_model(resolved.as_ref(), KNOWLEDGE_HARVESTER_EFFORT)?;
        *self
            .knowledge_harvester_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = resolved;
        if cleared {
            self.clear_role_effort(KNOWLEDGE_HARVESTER_EFFORT);
        }
        Ok(info)
    }
}

impl Message<crate::GetKnowledgeHarvesterModel> for Agents {
    type Reply = Option<ModelInfo>;

    async fn handle(
        &mut self,
        _: crate::GetKnowledgeHarvesterModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<ModelInfo> {
        self.knowledge_harvester_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ModelInfo::from)
    }
}

impl Message<crate::SetKnowledgeHarvesterEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: crate::SetKnowledgeHarvesterEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        let model = resolved_effort_model(&self.knowledge_harvester_model);
        self.set_role_effort(model, KNOWLEDGE_HARVESTER_EFFORT, m.effort)
    }
}

impl Message<crate::GetKnowledgeHarvesterEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: crate::GetKnowledgeHarvesterEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.role_effort(KNOWLEDGE_HARVESTER_EFFORT)
    }
}

impl Agents {
    /// A session's own knowledge harvester, validated: the id resolves (stored
    /// as the catalog id) and takes the effort; no model = the default, no effort.
    fn validated_knowledge_harvester(
        &self,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<crate::SessionKnowledgeHarvester, String> {
        let trimmed = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let Some(id) = trimmed(model) else {
            return Ok(crate::SessionKnowledgeHarvester::default());
        };
        let resolved = self.catalog.resolve(&id, &*self.configured)?;
        let effort = trimmed(effort);
        if let Some(e) = &effort {
            match &resolved.model {
                Some(m) => m.validate_effort(e)?,
                None if effort_of(e).is_none() => return Err(format!("unknown effort {e}")),
                None => {}
            }
        }
        Ok(crate::SessionKnowledgeHarvester {
            model: Some(resolved.id),
            effort,
        })
    }
}

impl Message<crate::SetSessionKnowledgeHarvester> for Agents {
    type Reply = Result<crate::SessionKnowledgeHarvester, String>;

    async fn handle(
        &mut self,
        m: crate::SetSessionKnowledgeHarvester,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        let pick = self.validated_knowledge_harvester(m.model, m.effort)?;
        self.store_for(m.session)
            .set_session_knowledge_harvester(m.session, &pick)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.rt
            .publish(AgentEvent::KnowledgeHarvesterChanged {
                session: m.session,
                harvester: pick.clone(),
            })
            .await;
        Ok(pick)
    }
}

impl Message<crate::SessionKnowledgeHarvesterOf> for Agents {
    type Reply = Result<crate::SessionKnowledgeHarvester, String>;

    async fn handle(
        &mut self,
        m: crate::SessionKnowledgeHarvesterOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.store_for(m.session)
            .session_knowledge_harvester(m.session)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// 57 §2 `SideCall` from another plugin: forwarded to the session's actor,
/// which resolves the role and runs the call detached; the supervisor never
/// waits on the provider. An external (ACP) session has no model of ours.
impl Message<SideCall> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<SideReply, String>>;

    async fn handle(&mut self, m: SideCall, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let actor = self.side_target(m.session);
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

impl Agents {
    /// Resolve `id` through the catalog and refuse a provider without
    /// credentials — the check behind [`SetSubagentModel`] and [`ResolveModel`]
    /// (148 R4). An unknown id under a keyed provider passes through.
    fn resolve_keyed(&self, id: &str) -> Result<Resolved, String> {
        let r = self.catalog.resolve(id, &*self.configured)?;
        if let Some(p) = &r.provider
            && !(self.configured)(Catalog::adapter(p))
        {
            return Err(format!("provider `{p}` has no credentials"));
        }
        Ok(r)
    }
}

impl Message<ResolveModel> for Agents {
    type Reply = Result<ModelInfo, String>;

    async fn handle(
        &mut self,
        m: ResolveModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ModelInfo, String> {
        self.resolve_keyed(&m.model).map(|r| ModelInfo::from(&r))
    }
}

impl Agents {
    /// 194 D4b: `effort` against the catalog entry `model` resolves to
    /// (`None`, or an unknown id: only that the word is an effort).
    fn check_effort(&self, model: Option<&str>, effort: &str) -> Result<(), String> {
        let known = model.and_then(|m| self.catalog.resolve(m, &*self.configured).ok());
        crate::endless::pump::check_effort(known.as_ref().and_then(|r| r.model.as_ref()), effort)
    }
}

/// 194 D4b: [`crate::lifecycle::model_meta`] — [`ResolveModel`] without the credentials check;
/// `None` without a catalog row (what `SetEffort` then accepts: any known word).
impl Message<crate::lifecycle::ModelMeta> for Agents {
    type Reply = Result<Option<ModelInfo>, String>;

    async fn handle(
        &mut self,
        m: crate::lifecycle::ModelMeta,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<ModelInfo>, String> {
        let r = self.catalog.resolve(&m.model, &*self.configured)?;
        Ok(r.model.is_some().then(|| ModelInfo::from(&r)))
    }
}

impl Message<SideCallable> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SideCallable, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.side_target(m.session).map(drop)
    }
}

impl Agents {
    /// The native session actor a [`SideCall`] for `session` runs on, or the
    /// text it fails with (an ACP session has no model of ours).
    fn side_target(&self, session: SessionId) -> Result<ActorRef<Session>, String> {
        match self.sessions.get(&session).map(|l| &l.actor) {
            Some(LiveActor::Native(a)) => Ok(a.clone()),
            Some(LiveActor::Acp(_)) => Err("session runs on an external harness".to_owned()),
            None => Err(format!("no live session {session}")),
        }
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
            Some(id) => Some(self.resolve_keyed(id)?),
            None => None,
        };
        let info = resolved.as_ref().map(ModelInfo::from);
        // 194 D4c/d: "Same as new sessions" clears the subagent effort too; a
        // model that does not take it drops it. R1: first, so a failed write
        // changes nothing.
        if let Some(e) = self.subagent_effort()
            && info
                .as_ref()
                .is_none_or(|i| self.check_effort(Some(&i.id), &e).is_err())
        {
            self.store_subagent_effort(None)?;
        }
        *self
            .subagent_model
            .write()
            .unwrap_or_else(|p| p.into_inner()) = resolved;
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

impl Message<SetSubagentEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: SetSubagentEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        // 194 D4b: checked against the effective subagent model.
        if let Some(e) = &m.effort {
            let model = self
                .subagent_model_id()
                .unwrap_or_else(|| self.default_model.clone());
            self.check_effort(Some(&model), e)?;
        }
        self.store_subagent_effort(m.effort.clone())?;
        Ok(m.effort)
    }
}

impl Message<GetSubagentEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: GetSubagentEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.subagent_effort()
    }
}

impl Message<ValidateEffort> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ValidateEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.check_effort(m.model.as_deref(), &m.effort)
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
    /// The subagent default model's id for the system prompt, if set.
    fn subagent_model_id(&self) -> Option<String> {
        self.subagent_model
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|r| r.id.clone())
    }

    /// The live subagent default effort, if set.
    fn subagent_effort(&self) -> Option<String> {
        self.subagent_effort
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 194 row 3: persist `agent.subagents.effort` (quoted so a bare budget stays a
    /// string; `~` clears), then swap the live value. R1: a failed write is the
    /// caller's `Err` and leaves the live value alone.
    fn store_subagent_effort(&self, effort: Option<String>) -> Result<(), String> {
        let value = effort
            .as_deref()
            .map_or_else(|| "~".to_owned(), |e| format!("\"{e}\""));
        let path = self.rt.data_dir.join("config.yaml");
        runtime::write_nested_config_value(&path, "agent", "subagents", "effort", &value)
            .map_err(|e| format!("config.yaml: {e}"))?;
        *self
            .subagent_effort
            .write()
            .unwrap_or_else(|p| p.into_inner()) = effort;
        Ok(())
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
                        "provider `{provider}` is not configured; add a key, log in, or add a custom provider first"
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
        let cleared = self.follow_role_model(resolved.as_ref(), VISION_EFFORT)?;
        *self.vision_model.write().unwrap_or_else(|p| p.into_inner()) = resolved;
        // read-file-images §3: a (re)chosen fallback gets a fresh chance everywhere.
        crate::vision::fallback_changed();
        self.vision_error = None;
        if cleared {
            self.clear_role_effort(VISION_EFFORT);
        }
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

impl Message<crate::SetVisionEffort> for Agents {
    type Reply = Result<Option<String>, String>;

    async fn handle(
        &mut self,
        m: crate::SetVisionEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Option<String>, String> {
        let model = resolved_effort_model(&self.vision_model);
        self.set_role_effort(model, VISION_EFFORT, m.effort)
    }
}

impl Message<crate::GetVisionEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: crate::GetVisionEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.role_effort(VISION_EFFORT)
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
    type Reply = Result<(String, SessionId, Option<String>), String>;

    async fn handle(&mut self, m: SpawnChild, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 183 Z3-R4: a contact session spawns nothing, native or external, whatever its
        // allowlist says: a child is not a contact session itself (a turn's allowlist is the
        // turn's own session's), so it would carry the owner's tools back to the contact. The
        // supervisor's own record answers for inboxes (a registry that cannot answer does not
        // open the door); the registry for a contact turn in the spawner's session.
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR).unwrap_or_default();
        let turn_contact = match &registry {
            Some(reg) => reg
                .ask(::tools::ContactScoped(m.parent))
                .await
                .unwrap_or(false),
            None => false,
        };
        if turn_contact || self.under_inbox(m.parent) {
            return Err("subagents are not available to a contact".into());
        }
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
        // itself; an external harness (here or on a node) takes `model` and
        // `effort` through its config options (75 §8: an effort it does not
        // list fails the spawn naming the list) and reads the ceiling only
        // as the first line of its brief.
        let m = SpawnChild {
            agent: m
                .agent
                .and_then(|a| crate::peers::canonical_agent(&self.nodes, &a)),
            ..m
        };
        let (harness, engine) = match m.agent.as_deref() {
            Some(a) => crate::peers::parse_agent(a)?,
            None => (None, ""),
        };
        let node = m.agent.is_some() && self.nodes.read().is_ok_and(|n| n.contains_key(engine));
        let external = harness.is_some() || (m.agent.is_some() && !node);
        // 183 Z3-R2 (M4): the spawner's ceiling right now — its own, an ancestor's — becomes a
        // native child's own cap (it never outlives it uncapped).
        let ceiling = match &registry {
            Some(reg) => reg
                .ask(::tools::EffectiveCap(m.parent))
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let m = SpawnChild {
            cap: child_cap(m.cap, ceiling, external),
            ..m
        };
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
        // The harness's own value list is the truth for an external harness.
        if let Some(e) = &m.effort
            && !external
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
        let agent_name = m.agent.clone();
        self.session(
            Some(id),
            Some((m.parent, &name)),
            m.agent.map(|a| (a, m.cwd, m.cap, m.skills)),
            (m.model.as_deref(), m.effort.as_deref()),
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
        if let (Some(_), Some(model)) = (&agent_name, &m.model) {
            self.requested_models.insert(id, model.clone());
        }
        // 139 §7.4 (W3): the parent's `waits(child_result)` row.
        child_waits::child_spawned(self, m.parent, id, &m.call_id, agent_name.as_deref()).await;
        // Next to `inherit_approvals` in spirit: the ceiling is on the
        // registry before the child's first `ListTools`.
        if let Some(cap) = m.cap {
            self.install_cap(id, cap).await;
        }
        // The roster's model (and harness) at once, so the parent's row needs no reload.
        let model = match self.store_for(m.parent).session(id).await {
            Ok(Some(row)) => Some(view::effective_model(&row, self.requested_models.get(&id)))
                .filter(|m| !m.is_empty()),
            _ => None,
        };
        self.rt
            .publish(AgentEvent::SubagentSpawned {
                parent: m.parent,
                child: id,
                name: name.clone(),
                cap: m.cap,
                agent: agent_name.clone(),
                model,
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
        // Not enforceable on an external harness: the ceiling is the first
        // line of its brief instead (67 §8).
        let text = match m.cap.filter(|_| external) {
            Some(cap) => format!("Limits: tools {cap}\n{}", m.task),
            None => m.task,
        };
        // 01 §2.6: the pool of the spawning tree — external for children
        // another harness or engine executes, native for ours.
        let pool = if external || node {
            Pool::External
        } else {
            Pool::Native
        };
        let root = self
            .store_for(m.parent)
            .root_of(m.parent)
            .await
            .unwrap_or(m.parent);
        let permits = self.tree_pools(root).get(pool).clone();
        let me = ctx.actor_ref().clone();
        if let Ok(permit) = permits.clone().try_acquire_owned() {
            let permit = SpawnPermit {
                pool,
                root,
                _permit: permit,
            };
            self.start_child_turn(id, Some(permit), text, ticket, Some(me))
                .await;
            return Ok((name, id, None));
        }
        // Every slot is busy: the brief waits, and the parent is told.
        let notice = self.queued_notice(pool, root, &permits);
        let abort = tokio::spawn(async move {
            let Ok(permit) = permits.acquire_owned().await else {
                return;
            };
            let _ = me
                .tell(StartChildTurn {
                    child: id,
                    permit: SpawnPermit {
                        pool,
                        root,
                        _permit: permit,
                    },
                })
                .await;
        })
        .abort_handle();
        self.queued.insert(
            id,
            QueuedSpawn {
                abort,
                ticket,
                text,
                pool,
                root,
                since: Instant::now(),
                held: Vec::new(),
            },
        );
        Ok((name, id, Some(notice)))
    }
}

impl Message<StartChildTurn> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: StartChildTurn, ctx: &mut Context<Self, Self::Reply>) {
        let child = m.child;
        // Dropped meanwhile (stopped, closed, deleted; its ticket and work
        // settled then): the permit goes back untouched.
        let Some(q) = self.queued.remove(&child) else {
            return;
        };
        let waited = q.since.elapsed().as_secs();
        if waited > 300 {
            tracing::warn!(child = %child, pool = q.pool.name(), root = %short_id(q.root), waited, "queued child started");
        } else {
            tracing::info!(child = %child, pool = q.pool.name(), root = %short_id(q.root), waited, "queued child started");
        }
        let me = ctx.actor_ref().clone();
        self.start_child_turn(child, Some(m.permit), q.text, q.ticket, Some(me))
            .await;
        self.replay(child, q.held).await;
    }
}

/// The first eight hex digits of a session id, as log lines name a tree.
fn short_id(id: SessionId) -> String {
    id.to_string()[..8].to_owned()
}

impl Agents {
    /// The two child-work pools of the tree rooted at `root` (01 §2.6),
    /// created at the configured sizes on first use.
    fn tree_pools(&mut self, root: SessionId) -> &TreePools {
        let cfg = &self.cfg.subagents;
        self.pools
            .entry(root)
            .or_insert_with(|| TreePools::new(cfg))
    }

    /// The `spawn_subagent` line telling the parent its child waits (01 §2.6).
    fn queued_notice(&self, pool: Pool, root: SessionId, permits: &Semaphore) -> String {
        let cap = match pool {
            Pool::Native => self.cfg.subagents.max_concurrent,
            Pool::External => self.cfg.subagents.max_concurrent_external,
        }
        .max(1);
        let held = cap.saturating_sub(permits.available_permits());
        let mut running: Vec<(DateTime<Utc>, &str)> = self
            .child_turns
            .iter()
            .filter(|(_, ct)| {
                ct.permit
                    .as_ref()
                    .is_some_and(|p| p.pool == pool && p.root == root)
            })
            .filter_map(|(id, ct)| {
                let (_, name) = self.sessions.get(id)?.parent.as_ref()?;
                Some((ct.started_at, name.as_str()))
            })
            .collect();
        running.sort();
        let running = running
            .iter()
            .take(5)
            .map(|(at, name)| format!("{name} since {}", at.format("%H:%M")))
            .collect::<Vec<_>>()
            .join(", ");
        let running = if running.is_empty() {
            String::new()
        } else {
            format!(" (running: {running})")
        };
        tracing::info!(
            pool = pool.name(),
            root = %short_id(root),
            held = %format!("{held}/{cap}"),
            "child queued"
        );
        format!(
            "Queued: all {cap} {} child slots of this orchestrator are busy{running}; it starts when one frees. Messages you send it are delivered after its brief.",
            pool.name()
        )
    }

    /// Route a child's brief as its first turn under `permit` (`None`: a
    /// human promoted it, 01 §2.6) and start the `max_runtime_secs` timer.
    /// A child closed meanwhile, or whose owning scope was cancelled
    /// ([`Self::claim_child_turn`]), gets nothing: its ticket is released.
    async fn start_child_turn(
        &mut self,
        child: SessionId,
        permit: Option<SpawnPermit>,
        text: String,
        ticket: InputTicket,
        me: Option<ActorRef<Agents>>,
    ) {
        if self.closing.contains(&child) {
            self.ledger.release(child, ticket);
            self.network_input_dropped(ticket).await;
            return;
        }
        let Ok(scope) = self.claim_child_turn(child, ticket).await else {
            return;
        };
        // 137 R1-D1: a native child of an endless parent opens with the
        // parent's view before its brief (its prompt says so, R1-D1 note).
        if let Some(Live {
            actor: LiveActor::Native(actor),
            parent: Some((parent, _)),
        }) = self.sessions.get(&child)
            && self.endless.active(*parent)
        {
            let (actor, parent) = (actor.clone(), *parent);
            let blocks = crate::endless_ext::render_snapshot(&self.endless, parent).await;
            if !blocks.is_empty() {
                let _ = actor
                    .tell(crate::loop_::OpenerContext(
                        blocks.into_iter().map(|b| b.text).collect(),
                    ))
                    .await;
            }
        }
        let _ = self
            .route(child, None, text, Vec::new(), None, ticket, scope, false)
            .await;
        // 179: a brief a pause held starts no turn yet — the permit is kept,
        // the cap's timer is armed when the turn really starts (`turn_began`),
        // so the paused time never counts.
        let me = match self.paused_root(child).await {
            Some(_) => None,
            None => me.or_else(|| ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten()),
        };
        self.arm_child_turn(child, permit, chrono::Duration::zero(), me);
    }

    /// A turn of `session` began (`TurnStarted`) or was resumed after a
    /// restart (`TurnResumed`: `resumed` = its turn and `interrupted_at`).
    /// 178 W1: every child turn is capped — native and ACP, spawn, follow-up
    /// or resumed — with what is left of `max_runtime_secs` after the run
    /// time the journal shows before the restart (downtime never counts).
    async fn turn_began(
        &mut self,
        session: SessionId,
        resumed: Option<(TurnId, DateTime<Utc>)>,
        me: ActorRef<Agents>,
    ) {
        self.running.insert(session);
        let live = self.sessions.get(&session);
        let child = live.is_some_and(|l| l.parent.is_some());
        let acp = matches!(live.map(|l| &l.actor), Some(LiveActor::Acp(_)));
        // 178 F1: an ACP prompt's run clock lives here (its journal turn is
        // the supervisor's); the drain reads it for the `suspended` row.
        let acp_ran = if acp {
            self.acp_run_clock(session).await
        } else {
            0
        };
        if child
            && self
                .child_turns
                .get(&session)
                .is_none_or(|ct| ct.timer.is_none())
        {
            let ran_ms = match resumed {
                Some((turn, at)) => self
                    .store_for(session)
                    .turn_run_ms(session, turn, Some(at))
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(session = %session, "178: child run time unknown: {e:#}");
                        0
                    }),
                None => acp_ran,
            };
            let ran = crate::durability::ran_delta(ran_ms);
            self.arm_child_turn(session, None, ran, Some(me));
        }
        // 155 §5.3: a turn starting ends `idle`/`done`.
        self.check_peer_waits(session).await;
    }

    /// 178 F1: start the run clock of the ACP session's open journal turn
    /// (146 D9: one per prompt, kept open across a restart and respawned on)
    /// from the run time its journal holds; returns that run time (ms).
    async fn acp_run_clock(&self, session: SessionId) -> u64 {
        let store = self.store_for(session);
        let turn = match store.open_turn(session).await {
            Ok(Some((turn, _))) => turn,
            Ok(None) => return 0,
            Err(e) => {
                tracing::warn!(session = %session, "178: ACP open turn unknown: {e:#}");
                return 0;
            }
        };
        let ran = store
            .turn_run_ms(session, turn, None)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(session = %session, "178: ACP run time unknown: {e:#}");
                0
            });
        crate::durability::run_clock::start(session, turn, ran);
        ran
    }

    /// The one arming point of a child turn's `max_runtime_secs` timer
    /// (178 W1): a turn that already has a timer keeps it; otherwise the
    /// timer gets [`crate::durability::remaining_run`] after `ran`, and
    /// `started_at` is `now - ran` so status shows the turn's true age.
    fn arm_child_turn(
        &mut self,
        child: SessionId,
        permit: Option<SpawnPermit>,
        ran: chrono::Duration,
        me: Option<ActorRef<Agents>>,
    ) {
        let cap = Duration::from_secs(self.cfg.subagents.max_runtime_secs);
        let budget = crate::durability::remaining_run(cap, ran);
        self.arms += 1;
        let arm = self.arms;
        let ct = self.child_turns.entry(child).or_insert_with(|| ChildTurn {
            permit: None,
            timer: None,
            arm: 0,
            started_at: Utc::now().checked_sub_signed(ran).unwrap_or_else(Utc::now),
        });
        if permit.is_some() {
            ct.permit = permit;
        }
        if ct.timer.is_none() && me.is_some() {
            ct.arm = arm;
            // 179: an entry kept timer-less while a pause held the brief
            // counts from now, not from the spawn.
            ct.started_at = Utc::now().checked_sub_signed(ran).unwrap_or_else(Utc::now);
            ct.timer = me.map(|me| runtime_timer(child, arm, me, budget));
        }
        if !ran.is_zero() {
            tracing::info!(session = %child, ran_secs = ran.num_seconds(), budget_secs = budget.as_secs(), "178: resumed child turn capped with its remaining run time");
        }
    }
}

/// The `max_runtime_secs` timer of one child turn (arm `arm`):
/// [`ChildTimedOut`] after `after` unless aborted when the turn ends.
fn runtime_timer(child: SessionId, arm: u64, me: ActorRef<Agents>, after: Duration) -> AbortHandle {
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        let _ = me.tell(ChildTimedOut(child, arm)).await;
    })
    .abort_handle()
}

impl Agents {
    /// A human input reaches a queued child (01 §2.6): the permit wait is
    /// abandoned and the brief runs now, permit-free, as a human-opened
    /// turn. Returns the inputs held for the child, for [`Self::replay`]
    /// after the human's text.
    async fn promote(&mut self, child: SessionId) -> Vec<Held> {
        let Some(q) = self.queued.remove(&child) else {
            return Vec::new();
        };
        q.abort.abort();
        tracing::info!(child = %child, pool = q.pool.name(), root = %short_id(q.root), waited = q.since.elapsed().as_secs(), "queued child promoted by human input");
        self.start_child_turn(child, None, q.text, q.ticket, None)
            .await;
        q.held
    }

    /// Route the inputs held for a once-queued child, in arrival order and
    /// with their original flags, after its brief.
    async fn replay(&mut self, child: SessionId, held: Vec<Held>) {
        for h in held {
            match h {
                Held::Routed {
                    id,
                    text,
                    files,
                    channel,
                    ticket,
                    scope,
                    defer,
                } => {
                    let _ = self
                        .route(child, id, text, files, channel, ticket, scope, defer)
                        .await;
                }
                Held::Peer(input) => match self.actor(child).await {
                    Ok(actor) => self.offer_peer(actor, child, *input),
                    Err(reason) => {
                        self.peer_intake_failed(PeerIntakeFailed {
                            session: child,
                            id: input.record.receipt.id,
                            ticket: input.ticket,
                            reason,
                        })
                        .await;
                    }
                },
            }
        }
    }

    /// A queued child is stopped, closed, deleted or archived — or its
    /// parent is — before its permit came (01 §2.6): the wait is aborted,
    /// the brief's ticket released, its owning scope's `child:<id>` work
    /// settled as [`Self::claim_child_turn`] does, and every held input
    /// dropped. The child hears why as an `AgentEvent::Error`.
    async fn drop_queued(&mut self, child: SessionId) {
        let Some(q) = self.queued.remove(&child) else {
            return;
        };
        q.abort.abort();
        tracing::info!(child = %child, pool = q.pool.name(), root = %short_id(q.root), waited = q.since.elapsed().as_secs(), "queued child dropped");
        self.ledger.release(child, q.ticket);
        if let Some(OwnedChild { scope, .. }) = self.owned_children.remove(&child) {
            self.network_settle_work(scope, &format!("child:{child}"))
                .await;
        }
        const WHY: &str = "spawn cancelled before it started";
        // 178 W8: nothing will come from it — the parent's wait goes, said once.
        child_waits::child_gone(
            self,
            &[child],
            "was dropped: its spawn was cancelled before it started",
            &[],
        )
        .await;
        for h in q.held {
            match h {
                Held::Routed { ticket, .. } => {
                    self.ledger.release(child, ticket);
                    self.network_input_dropped(ticket).await;
                }
                Held::Peer(input) => {
                    self.peer_intake_failed(PeerIntakeFailed {
                        session: child,
                        id: input.record.receipt.id,
                        ticket: input.ticket,
                        reason: WHY.into(),
                    })
                    .await;
                }
            }
        }
        self.error(child, WHY.into()).await;
    }

    /// [`PeerIntakeFailed`]'s body: the runner (or the supervisor, for an
    /// input never offered) did not take the exchange.
    async fn peer_intake_failed(&mut self, m: PeerIntakeFailed) {
        tracing::info!(session = %m.session, id = %m.id, "peer intake failed: {}", m.reason);
        self.enqueued.remove(&m.id);
        self.ledger.release(m.session, m.ticket);
        self.network_input_dropped(m.ticket).await;
        self.full_intake_failed(m.id, &m.reason).await;
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

impl Message<ChildTimedOut> for Agents {
    type Reply = ();

    /// 178 F2: only the timer of the child's current arm cancels it; a fire
    /// that lost the race with its turn's end (a stale arm) does nothing.
    async fn handle(
        &mut self,
        ChildTimedOut(child, arm): ChildTimedOut,
        _: &mut Context<Self, Self::Reply>,
    ) {
        if self
            .child_turns
            .get(&child)
            .is_some_and(|ct| ct.timer.is_some() && ct.arm == arm)
            && let Some(live) = self.sessions.get(&child)
        {
            self.timed_out.insert(child);
            self.ledger.bump(child);
            live.actor.tell_cancel(None).await;
        }
    }
}

/// 183 §11.4: a machine as a refusal names it — its mesh name, else a short id.
async fn engine_label(id: &str) -> String {
    peer_mesh::engine_name(&peer_mesh::engine_names().await.1, id)
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

/// 183 §6.3: what [`Agents::deliver_step`] leaves to do: nothing, or the network leg to the
/// recipient's owner on another engine.
enum Step {
    Done(Delivered),
    Forward(Delivered, ForwardJob),
    /// A retry of a `request_id` whose network leg is still in flight.
    InFlight(Uuid),
}

/// The network leg of a forwarded peer message (69 P): one `peer_deliver` over
/// `theuth/rpc/1` and, on failure, the receipt's error text. Runs off the mailbox (183 §6.3).
struct ForwardJob {
    transport: Option<Arc<dyn replica::Transport>>,
    owner: String,
    to: SessionId,
    envelope: PeerEnvelope,
}

impl ForwardJob {
    /// `Err(receipt error)` when the owner's intake did not commit.
    async fn run(self) -> Result<(), String> {
        let outcome = match &self.transport {
            Some(t) => {
                let owner = replica::PeerId(self.owner.clone());
                peer_mesh::forward(t.as_ref(), &owner, self.to, &self.envelope)
                    .await
                    .map(drop)
            }
            None => Err(peer_mesh::Failure::Unreachable {
                owner: self.owner.clone(),
                why: "mesh not running".into(),
            }),
        };
        outcome.map_err(|f| {
            tracing::info!(session = %self.to, id = %self.envelope.id, "peer message not forwarded: {f:?}");
            f.receipt_error()
        })
    }
}

/// `tell` (internal, 183 §6.3): a [`ForwardJob`] came back to the mailbox; settle the receipt
/// and answer the [`Deliver`] that started it.
struct ForwardOutcome {
    delivered: Delivered,
    result: Result<(), String>,
    reply: Option<kameo::reply::ReplySender<Result<Delivered, String>>>,
}

impl Message<Deliver> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<Delivered, String>>;

    /// 183 §6.3 (the 176 E1 shape): resolve, policy, accept and the waiter in the mailbox;
    /// the network leg to another engine's owner off it, settled by [`ForwardOutcome`]. Two
    /// engines messaging each other at once no longer hold each other's mailbox for 30 s.
    async fn handle(&mut self, m: Deliver, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (delegated, reply) = ctx.reply_sender();
        match self.deliver_step(m, None).await {
            Ok(Step::Forward(delivered, job)) => {
                self.forwarding.insert(delivered.receipt.id, Vec::new());
                let me = ctx.actor_ref().clone();
                tokio::spawn(async move {
                    let result = job.run().await;
                    let _ = me
                        .tell(ForwardOutcome {
                            delivered,
                            result,
                            reply,
                        })
                        .await;
                });
            }
            Ok(Step::InFlight(id)) => {
                if let Some(r) = reply {
                    self.forwarding.entry(id).or_default().push(r);
                }
            }
            Ok(Step::Done(d)) => {
                if let Some(r) = reply {
                    r.send(Ok(d));
                }
            }
            Err(e) => {
                if let Some(r) = reply {
                    r.send(Err(e));
                }
            }
        }
        delegated
    }
}

impl Message<ForwardOutcome> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ForwardOutcome, _: &mut Context<Self, Self::Reply>) {
        let waiters = self.forwarding.remove(&m.delivered.receipt.id);
        let d = self.forward_settled(m.delivered, m.result).await;
        for r in waiters.into_iter().flatten() {
            r.send(Ok(Delivered {
                receipt: d.receipt.clone(),
                reply: None,
            }));
        }
        if let Some(r) = m.reply {
            r.send(Ok(d));
        }
    }
}

impl Message<PeerDeliver> for Agents {
    type Reply = Result<PeerReceipt, PeerDeliverError>;

    async fn handle(&mut self, m: PeerDeliver, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.peer_deliver(m).await
    }
}

impl Agents {
    /// The owner's intake of a forwarded peer message (69 P): the ordinary
    /// [`Self::deliver`] path from the envelope's sender to `session`, under
    /// the envelope's id and this engine's policy. Refused when the row is
    /// not ours (the sender's replica was stale) — never a take.
    pub(crate) async fn peer_deliver(
        &mut self,
        m: PeerDeliver,
    ) -> Result<PeerReceipt, PeerDeliverError> {
        let row = self
            .store
            .session(m.session)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("unknown peer `session:{}`", m.session))?;
        if !row.here {
            return Err(
                format!("session:{} is owned by {}, not here", m.session, row.owner).into(),
            );
        }
        // 183 §6.5, §6.2, §11.4: a forwarded message must come from the engine that owns its
        // sender, from a machine this one has not switched off, past this engine's ceiling.
        if let Some(fwd) = m.forwarder.as_deref() {
            let owner = self
                .store
                .session(m.envelope.from)
                .await
                .map_err(|e| e.to_string())?
                .map(|r| r.owner);
            let refusal = if owner.as_deref() != Some(fwd) {
                Some(MeshRefusal::NotOwner {
                    engine: engine_label(fwd).await,
                    from: m.envelope.from,
                })
            } else if mesh::messaging_off(fwd) {
                let me = mesh::self_name();
                Some(MeshRefusal::Messaging {
                    from: engine_label(fwd).await,
                    to: me.clone(),
                    on: me,
                })
            } else if !self
                .communication
                .0
                .read()
                .state(None)
                .is_ok_and(|s| s.global)
            {
                Some(MeshRefusal::Off {
                    engine: mesh::self_name(),
                })
            } else {
                None
            };
            match refusal {
                Some(r @ MeshRefusal::NotOwner { .. }) => {
                    return Err(PeerDeliverError::NotOwner(r));
                }
                Some(r) => {
                    return Err(
                        format!("send_message: {}: {r}", tools::communication::DISABLED).into(),
                    );
                }
                None => {}
            }
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
            chain,
        } = m.envelope;
        // 183 §6.5: `enqueue_peer` bounds the chain from the sender's count.
        self.inbound_chain = Some((id, chain));
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
            .await;
        self.inbound_chain = None;
        Ok(d?.receipt)
    }

    /// `(owner engine id)` when `session`'s replicated row is owned by
    /// another engine (69 P); `None` for ours, unowned or unknown — today's
    /// local path.
    pub(crate) async fn remote_owner(&self, session: SessionId) -> Option<String> {
        self.store
            .session(session)
            .await
            .ok()
            .flatten()
            .filter(|row| !row.here && !row.owner.is_empty())
            .map(|row| row.owner)
    }

    /// The mesh transport, fetched once from the replica actor (tests inject it).
    pub(crate) async fn transport(&mut self) -> Option<Arc<dyn replica::Transport>> {
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
    /// ponytail: inline (in the mailbox) for the child-result paths; `send_message`'s own
    /// `Deliver` runs the leg off the mailbox (183 §6.3), and these could too given an actor ref.
    async fn forward_peer(
        &mut self,
        owner: String,
        to: SessionId,
        m: &Deliver,
        body: String,
        receipt: PeerReceipt,
        generation: Generation,
    ) -> Result<Delivered, String> {
        let (d, job) = self
            .forward_prepare(owner, to, m, body, receipt, generation)
            .await?;
        let result = job.run().await;
        Ok(self.forward_settled(d, result).await)
    }

    /// 183 §6.3, in the mailbox: the envelope (with the sender's chain, §6.5) and, when
    /// `await_reply`, the waiter registered before the network leg so a fast reply finds it.
    /// 183 §6.2: refused (the exchange `failed`) when this engine switched the owner's
    /// machine off.
    async fn forward_prepare(
        &mut self,
        owner: String,
        to: SessionId,
        m: &Deliver,
        body: String,
        receipt: PeerReceipt,
        generation: Generation,
    ) -> Result<(Delivered, ForwardJob), String> {
        // Nothing here will consume it.
        self.enqueued.remove(&receipt.id);
        let refused = if mesh::messaging_off(&owner) {
            let me = mesh::self_name();
            Some(format!(
                "denied: send_message: {}: {}",
                tools::communication::DISABLED,
                MeshRefusal::Messaging {
                    from: me.clone(),
                    to: engine_label(&owner).await,
                    on: me,
                }
            ))
        } else {
            None
        };
        let reply = match refused {
            Some(why) => Err(why),
            None => m
                .await_reply
                .then(|| self.wait(m.from, receipt.to.clone(), receipt.id, generation))
                .transpose(),
        };
        let reply = match reply {
            Ok(r) => r,
            Err(why) => {
                if let Ok(failed) = self.store.fail_exchange(receipt.id, &why).await {
                    self.peer_changed(&failed).await;
                }
                return Err(why);
            }
        };
        let envelope = PeerEnvelope {
            id: receipt.id,
            from: m.from,
            text: body,
            reply_to: m.reply_to,
            request_id: m.request_id.clone(),
            chain: self.ledger.chain(m.from),
        };
        let job = ForwardJob {
            transport: self.transport().await,
            owner,
            to,
            envelope,
        };
        Ok((Delivered { receipt, reply }, job))
    }

    /// 183 §6.3, back in the mailbox: a forward that did not commit fails the receipt and
    /// drops its waiter. One that timed out after the owner already answered keeps the reply.
    async fn forward_settled(&mut self, d: Delivered, result: Result<(), String>) -> Delivered {
        let id = d.receipt.id;
        match result {
            Ok(()) => d,
            Err(_) if d.reply.is_some() && !self.waits.contains_key(&id) => d,
            Err(error) => {
                if self.waits.remove(&id).is_some() {
                    self.bound.retain(|_, b| *b != id);
                }
                let failed = self
                    .store
                    .fail_exchange(id, &error)
                    .await
                    .unwrap_or(d.receipt);
                self.peer_changed(&failed).await;
                Delivered {
                    receipt: failed,
                    reply: None,
                }
            }
        }
    }

    /// [`Deliver`]'s body, the forward leg included (tests, [`Self::peer_deliver`]).
    /// `id`: the message id when another engine minted it; minted by the store otherwise.
    pub(crate) async fn deliver(
        &mut self,
        m: Deliver,
        id: Option<Uuid>,
    ) -> Result<Delivered, String> {
        match self.deliver_step(m, id).await? {
            Step::Done(d) => Ok(d),
            Step::InFlight(id) => Ok(Delivered {
                receipt: self
                    .store
                    .exchange_receipt(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "unknown message".to_string())?,
                reply: None,
            }),
            Step::Forward(d, job) => {
                let result = job.run().await;
                Ok(self.forward_settled(d, result).await)
            }
        }
    }

    /// [`Self::deliver`] up to the network leg, which it returns undone (183 §6.3).
    async fn deliver_step(&mut self, m: Deliver, id: Option<Uuid>) -> Result<Step, String> {
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
            Target::Uplink => return self.uplink(&m, body).await.map(Step::Done),
            Target::Remote(alias) => {
                let contact = self.network.is_contact(&alias);
                let generation = self
                    .remote_generation(m.from, contact)
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
                return Ok(Step::Done(Delivered { receipt, reply }));
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
        if self.forwarding.contains_key(&receipt.id) {
            return Ok(Step::InFlight(receipt.id));
        }
        if receipt.state != PeerMessageState::Accepted || !self.enqueued.insert(receipt.id) {
            // A retried request_id: the existing outcome, nothing sent again.
            return Ok(Step::Done(Delivered {
                receipt,
                reply: None,
            }));
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
                return Ok(Step::Done(Delivered {
                    receipt: settled,
                    reply,
                }));
            }
        }
        // 69 P: the recipient lives on another engine — its owner runs the
        // intake; nothing here is woken.
        if let Some(owner) = self.remote_owner(to).await {
            let (d, job) = self
                .forward_prepare(owner, to, &m, body, receipt, generation)
                .await?;
            return Ok(Step::Forward(d, job));
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
        Ok(Step::Done(Delivered { receipt, reply }))
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
                        origin: r.origin(),
                        text: r.text,
                        seq: m.seq,
                        turn: m.turn,
                        files: r.files,
                    })
                    .await;
            }
            Ok(None) => {
                tracing::error!(session = %m.session, id = %m.id, "consumed exchange has no record")
            }
            Err(e) => tracing::error!(id = %m.id, "peer_record: {e:#}"),
        }
        // 186 (D4): a full contact's task completes on consumption.
        self.full_consumed(m.id).await;
    }
}

impl Message<PeerIntakeFailed> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PeerIntakeFailed, _: &mut Context<Self, Self::Reply>) {
        self.peer_intake_failed(m).await;
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

impl Message<mesh::MeshEvent> for Agents {
    type Reply = ();

    /// 183 §6.1: a peer connected or went (pairing and unpairing end in one), or the lease
    /// moved (the first event after the mesh starts): re-read the paired set.
    async fn handle(&mut self, ev: mesh::MeshEvent, _: &mut Context<Self, Self::Reply>) {
        if matches!(ev, mesh::MeshEvent::Peer(_) | mesh::MeshEvent::Lease(_)) {
            self.mesh_default(mesh::paired_count()).await;
        }
    }
}

impl Agents {
    /// 183 §6.1 (D3, D6): with `agent.communication` absent (source `default` or `mesh`) the
    /// ceiling is on exactly while this engine has a paired machine (`paired`, `None` = no
    /// mesh yet) or, Z3-R2, a contact (a lone engine with contacts can message them). An
    /// explicit value (`config`, `env`, `command`) is never touched.
    pub(crate) async fn mesh_default(&mut self, paired: Option<usize>) {
        let paired = paired.or(self.network.last_paired());
        let Some(n) = paired else { return };
        self.network.set_last_paired(n);
        let Ok(s) = self.communication.0.read().state(None) else {
            return;
        };
        let contacts = self.network.contact_count();
        let on = n > 0 || contacts > 0;
        if !matches!(s.source.as_str(), "default" | "mesh") || s.global == on {
            return;
        }
        tracing::info!(
            "agent.communication unset: {} ({n} paired machine(s), {contacts} contact(s))",
            if on {
                "on in this mesh"
            } else {
                "off, no paired machine or contact"
            }
        );
        self.revoking(|p| p.set_global(on, if on { "mesh" } else { "default" }))
            .await;
        // As `SetCommunication`: settle what the invalidated generations carried.
        self.revoke(None).await;
        self.communication_changed(None).await;
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
                // 183 §4.5 (D4): durable first; the policy only has an opt-out,
                // so "on" is stored as NULL (inherit).
                self.store
                    .set_session_communication(s, (!m.enabled).then_some(false))
                    .await
                    .map_err(comm_err)?;
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

// 183 P3: E — contacts list as `peer:` rows, their exports as tags (§7.7).
impl Message<ListPeers> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<PeerPage, String>>;

    /// 176 E1: the session list, lineages, mesh names and job counts are
    /// read off the mailbox ([`PeerRead`]); the policy walk comes back to it
    /// ([`PeersRead`]), where every policy mutation happens. 200 (D7): the
    /// full-access listings and the paging run off it again ([`PeerPlan::finish`]).
    async fn handle(&mut self, m: ListPeers, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (store, policy) = (self.store.clone(), self.communication.clone());
        let me = ctx.actor_ref().clone();
        ctx.spawn(async move {
            let read = PeerRead::fetch(&store, &policy, m.session).await?;
            let plan = me.ask(PeersRead { m, read }).await.map_err(|e| match e {
                SendError::HandlerError(e) => e,
                e => e.to_string(),
            })?;
            plan.finish().await
        })
    }
}

/// `ask` (internal, 176 E1): [`ListPeers`] once its [`PeerRead`] is in.
struct PeersRead {
    m: ListPeers,
    read: PeerRead,
}

impl Message<PeersRead> for Agents {
    type Reply = Result<PeerPlan, String>;

    async fn handle(
        &mut self,
        PeersRead { m, read }: PeersRead,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let rows = self.directory_from(m.session, read)?;
        // 183 F1: contacts' offers older than the TTL are refetched (the next listing shows
        // them); this one answers from what is cached.
        if self.network.offers_due() {
            self.rt
                .publish(crate::communication::network::ContactOffersStale)
                .await;
        }
        // 200 (Contract): the contacts whose cards grant us full access; none for an inbox
        // root (`directory_from` checked the session's own communication).
        let root = self
            .communication
            .0
            .read()
            .root_of(m.session)
            .map_err(|e| e.to_string())?;
        let granting = self.network.granting(root);
        Ok(PeerPlan { m, rows, granting })
    }
}

/// [`ListPeers`] after the policy walk: the directory rows and the contacts to list.
struct PeerPlan {
    m: ListPeers,
    rows: Vec<PeerInfo>,
    granting: Vec<(String, Arc<dyn crate::communication::network::PeerSender>)>,
}

/// 200: the `peer:<contact>` row's title while the contact gives us full access.
const FULL_TITLE: &str = "full access";

impl PeerPlan {
    /// The page: 200's walk when the query names one granting contact (or the cursor is
    /// `full:`), else the directory with each granting contact's preview (D8, Contract).
    async fn finish(self) -> Result<PeerPage, String> {
        use crate::communication::network::access;
        use crate::communication::{DirectoryKind, is_contact_row};
        let PeerPlan {
            m,
            mut rows,
            granting,
        } = self;
        let limit = m
            .limit
            .filter(|l| *l > 0)
            .unwrap_or(DISCOVER_DEFAULT_LIMIT)
            .min(DISCOVER_MAX_LIMIT);
        let query = m
            .query
            .as_deref()
            .map(str::to_lowercase)
            .filter(|q| !q.is_empty());
        let names: Vec<&str> = granting.iter().map(|(n, _)| n.as_str()).collect();
        let walks = matches!(m.kind, None | Some(DirectoryKind::Contact));
        let named = query
            .as_deref()
            .filter(|_| walks)
            .and_then(|q| access::names_contact(q, &names));
        let full_cursor = m
            .cursor
            .as_deref()
            .filter(|c| c.starts_with(access::CURSOR_PREFIX));
        // The walk: page 1 when the query names the contact, later pages by its cursor (the
        // query must still name that contact; an inbox has no granting contact to name).
        if full_cursor.is_some() || named.is_some() {
            let after = match full_cursor {
                Some(c) => Some(
                    access::parse_full_cursor(c)
                        .filter(|(c, _)| named.as_ref().is_some_and(|(n, _)| n == c))
                        .ok_or("unknown cursor")?
                        .1,
                ),
                None => None,
            };
            let (contact, rest) = named.ok_or("unknown cursor")?;
            let sender = &granting
                .iter()
                .find(|(n, _)| *n == contact)
                .ok_or("unknown cursor")?
                .1;
            let req = access::ListRequest {
                after,
                limit,
                query: rest.map(str::to_owned),
                kind: m.kind,
                active_only: m.active_only,
            };
            let (listing, refused) = match sender.list_sessions(req).await {
                Ok(l) => (l, None),
                Err(e) => (access::ContactListing::default(), Some(e)),
            };
            let mut peers = Vec::new();
            if after.is_none() {
                // Page 1's bounded prefix: the contact's row and its export rows.
                let head = format!("peer:{contact}");
                let prefix = format!("{head}/");
                peers.extend(rows.into_iter().filter(|p| {
                    is_contact_row(p) && (p.address == head || p.address.starts_with(&prefix))
                }));
                if let Some(p) = peers.iter_mut().find(|p| p.address == head) {
                    p.title = Some(full_title(refused.as_deref(), &listing.partial, None));
                }
            }
            peers.extend(
                listing
                    .rows
                    .iter()
                    .map(|r| access::session_row(&contact, r)),
            );
            return Ok(PeerPage {
                peers,
                cursor: listing.cursor.map(|c| access::full_cursor(&contact, c)),
                partial: partial_of(&contact, &listing),
            });
        }
        // Default: every granting contact's preview, all asked at once.
        let previews = match m.kind {
            None | Some(DirectoryKind::A2a | DirectoryKind::Contact) => {
                let req = access::ListRequest {
                    after: None,
                    limit: CONTACT_ROWS as u16,
                    query: query.clone(),
                    kind: m.kind,
                    active_only: m.active_only,
                };
                futures::future::join_all(granting.iter().map(|(n, s)| {
                    let req = req.clone();
                    async move { (n.clone(), s.list_sessions(req).await) }
                }))
                .await
            }
            Some(_) => Vec::new(),
        };
        let mut partial = Vec::new();
        let mut titles = HashMap::new();
        for (contact, answer) in previews {
            let (listing, refused) = match answer {
                Ok(l) => (l, None),
                Err(e) => (access::ContactListing::default(), Some(e)),
            };
            partial.extend(partial_of(&contact, &listing));
            // Under the contact's row and its export rows (`arrange` keeps that order).
            let head = format!("peer:{contact}");
            let prefix = format!("{head}/");
            let at = rows
                .iter()
                .rposition(|p| {
                    is_contact_row(p) && (p.address == head || p.address.starts_with(&prefix))
                })
                .map_or(rows.len(), |i| i + 1);
            rows.splice(
                at..at,
                listing
                    .rows
                    .iter()
                    .map(|r| access::session_row(&contact, r)),
            );
            titles.insert(contact, (refused, listing));
        }
        let hidden = arrange(
            &mut rows,
            m.active_only,
            m.kind == Some(DirectoryKind::Contact),
        );
        for p in rows.iter_mut().filter(|p| is_contact_row(p)) {
            if let Some((refused, listing)) = titles.get(&p.name) {
                let more = hidden.contains_key(&p.name) || listing.cursor.is_some();
                let walk = more.then_some(p.name.as_str());
                p.title = Some(full_title(refused.as_deref(), &listing.partial, walk));
            }
        }
        if let Some(k) = m.kind {
            rows.retain(|p| k.admits(p));
        }
        // A listed row was filtered by its member (`query` included).
        if let Some(q) = &query {
            rows.retain(|p| access::is_session_row(p) || peer_matches(p, q));
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
        let end = (start + usize::from(limit)).min(rows.len());
        let cursor = (end < rows.len()).then(|| rows[end - 1].address.clone());
        Ok(PeerPage {
            peers: rows.drain(start..end).collect(),
            cursor,
            partial,
        })
    }
}

/// Contract: a granting contact's `peer:<contact>` title: `full access`, with the walk's call
/// for `more` (the contact whose preview is not its whole list), then the machines that did
/// not answer. A refused listing says why (`… has not given you full access`).
fn full_title(refused: Option<&str>, partial: &[String], more: Option<&str>) -> String {
    if let Some(why) = refused {
        return why.to_owned();
    }
    let mut t = FULL_TITLE.to_owned();
    if let Some(contact) = more {
        t.push_str(&format!(
            " · more: discover_peers(kind=\"contact\", query=\"{contact}\")"
        ));
    }
    if !partial.is_empty() {
        t.push_str(&format!(" · did not answer: {}", partial.join(", ")));
    }
    t
}

/// `PeerPage.partial` of one contact's listing: `<contact>/<machine>`.
fn partial_of(
    contact: &str,
    listing: &crate::communication::network::access::ContactListing,
) -> Vec<String> {
    listing
        .partial
        .iter()
        .map(|m| format!("{contact}/{m}"))
        .collect()
}

/// `discover_peers(query)`: `q` (lowercase) in the name or the address, and (183 §6.6) in a
/// tag or the engine (`xeus`).
fn peer_matches(p: &PeerInfo, q: &str) -> bool {
    p.name.to_lowercase().contains(q)
        || p.address.contains(q)
        || p.tags.iter().any(|t| t.to_lowercase().contains(q))
        || p.engine
            .as_deref()
            .is_some_and(|e| e.to_lowercase().contains(q))
}

/// [`Agents::directory_from`]'s I/O, read off the mailbox: the durable rows
/// in creation order, the lineage of `session` and of every row the policy
/// does not know yet, the mesh engine names and every row's job counts.
struct PeerRead {
    rows: Vec<SessionSummary>,
    lineages: Vec<(SessionId, anyhow::Result<store::communication::Lineage>)>,
    engines: (String, HashMap<String, String>),
    counts: HashMap<SessionId, (u16, u16)>,
}

impl PeerRead {
    async fn fetch(
        store: &Store,
        policy: &SharedPolicy,
        session: SessionId,
    ) -> Result<Self, String> {
        let mut rows = store.list_sessions().await.map_err(|e| e.to_string())?;
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.0.cmp(&b.id.0)));
        let mut lineages = Vec::new();
        for id in std::iter::once(session).chain(rows.iter().map(|r| r.id)) {
            let known = policy.0.read().root_of(id).is_ok();
            if !known {
                lineages.push((id, store.lineage(id).await));
            }
        }
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        Ok(Self {
            engines: peer_mesh::engine_names().await,
            counts: view::job_counts(&ids).await,
            rows,
            lineages,
        })
    }
}

impl Agents {
    /// [`PeerRead::fetch`] then [`Self::directory_from`] in one go.
    #[cfg(test)]
    async fn directory(&mut self, session: SessionId) -> Result<Vec<PeerInfo>, String> {
        let read = PeerRead::fetch(&self.store, &self.communication, session).await?;
        self.directory_from(session, read)
    }

    /// The full permitted directory for `session` (50 §5.1) in stable order:
    /// same-tree sessions (parent, children, siblings, ancestors,
    /// descendants), then other `owner` roots and their trees when this
    /// root is `owner`, then configured remotes. Each local candidate passes
    /// [`Policy::route`]; nothing dormant is woken and no network is probed.
    /// Every row is mirrored first, so `route` never fails on a dangling link.
    fn directory_from(
        &mut self,
        session: SessionId,
        read: PeerRead,
    ) -> Result<Vec<PeerInfo>, String> {
        let PeerRead {
            rows,
            lineages,
            engines,
            counts,
        } = read;
        for (id, lineage) in lineages {
            self.apply_lineage(id, lineage);
        }
        let root = {
            let p = self.communication.0.read();
            p.enabled(session).map_err(|e| e.to_string())?;
            p.root_of(session).map_err(|e| e.to_string())?
        };
        let me_parent = rows.iter().find(|r| r.id == session).and_then(|r| r.parent);
        // 69 P: engine display names, asked once; rows owned elsewhere are
        // `remote` (no activity here — their owner would know), ours carry
        // this engine's name for symmetry.
        let (me_name, names) = engines;
        // 183 §6.6: which owners are connected now, for `reachable` on remote rows.
        let connected = mesh::connected_ids().unwrap_or_default();
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
                let kind = self.row_kind(r);
                let remote = !r.here && !r.owner.is_empty();
                let state = if remote {
                    PeerState::Remote
                } else {
                    self.row_state(r.id)
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
                    // 141 D10: a watch-only child is listed, never messaged.
                    send: r.observed.is_none(),
                    // An external harness cannot call theuth peer tools (50
                    // §6.4); a node child can, back over its conn (67 §8).
                    reply: kind != PeerKind::Acp && r.observed.is_none(),
                    stream: false,
                    activity: None,
                    title,
                    tags: r.tags.clone(),
                    engine: Some(if remote {
                        peer_mesh::engine_name(&names, &r.owner)
                    } else {
                        me_name.clone()
                    }),
                    // 183 §6.6: whether the owner is connected now.
                    reachable: remote.then(|| connected.contains(&r.owner)),
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
        let wanted: HashSet<SessionId> = same
            .iter()
            .filter(|(_, info)| info.state != PeerState::Remote)
            .map(|(id, _)| *id)
            .collect();
        let mut activity = self
            .view()
            .activity_of_rows(rows.iter().filter(|r| wanted.contains(&r.id)), &counts);
        let now = Instant::now();
        let mut out: Vec<PeerInfo> = same
            .into_iter()
            .map(|(id, mut info)| {
                info.activity = activity
                    .remove(&id)
                    .map(|a| peer_activity(a, self.activity.stalled_secs(id, now)));
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

    /// A row's backend as a directory shows it.
    fn row_kind(&self, r: &SessionSummary) -> PeerKind {
        match &r.agent {
            None => PeerKind::Native,
            // `"<node>"` is a theuth there and answers back over its
            // conn (67 §8); `"<harness>@<node>"` is that harness, which
            // has no peer tools, like a local one.
            Some(a)
                if crate::peers::parse_agent(a).is_ok_and(|(h, e)| {
                    h.is_none() && self.nodes.read().is_ok_and(|n| n.contains_key(e))
                }) =>
            {
                PeerKind::Node
            }
            Some(_) => PeerKind::Acp,
        }
    }

    /// The coarse liveness of a session this engine owns.
    fn row_state(&self, id: SessionId) -> PeerState {
        if self.approvals.contains_key(&id) || self.running.contains(&id) {
            PeerState::Running
        } else if self.waits.values().any(|w| w.session == id) {
            PeerState::Awaiting
        } else if self.live(id).is_some() {
            PeerState::Idle
        } else {
            PeerState::Stored
        }
    }

    /// 200 (D5, the live half) of `row`, a session this engine owns whose lineage is
    /// mirrored: `Policy::reachable_from_owner` and no contact's inbox on it or its root;
    /// `sendable` when ordinary unscoped peer intake takes it (not observed, its tree not
    /// paused). The row half is the caller's.
    pub(crate) fn full_access_live(
        &self,
        row: &SessionSummary,
    ) -> crate::communication::network::access::Eligibility {
        let (visible, root) = {
            let p = self.communication.0.read();
            let root = p.root_of(row.id).ok();
            (p.reachable_from_owner(row.id), root)
        };
        let inbox = |s: SessionId| self.network.contact_inbox(s);
        let visible = visible && !inbox(row.id) && root.is_some_and(|r| !inbox(r));
        let paused =
            self.paused.contains_key(&row.id) || root.is_some_and(|r| self.paused.contains_key(&r));
        let observed = row.kind == SessionKind::Observed || row.observed.is_some();
        crate::communication::network::access::Eligibility {
            visible,
            sendable: visible && !observed && !paused,
        }
    }
}

impl Message<crate::communication::network::access::FullAccessList> for Agents {
    type Reply = kameo::reply::DelegatedReply<
        Result<Vec<crate::communication::network::access::ListedSession>, String>,
    >;

    /// 200 (D7): the rows, their row half, owners, lineages and job counts are read off the
    /// mailbox; the live half and the activity come back to it ([`FullAccessRead`]).
    async fn handle(
        &mut self,
        m: crate::communication::network::access::FullAccessList,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (store, policy) = (self.store.clone(), self.communication.clone());
        let me = ctx.actor_ref().clone();
        ctx.spawn(async move {
            let read = FullAccessRead::fetch(&store, &policy).await?;
            me.ask(FullAccessListed { m, read })
                .await
                .map_err(|e| match e {
                    SendError::HandlerError(e) => e,
                    e => e.to_string(),
                })
        })
    }
}

/// 200 (D7): the I/O of one member listing, off the mailbox: this engine's own rows that
/// pass the row half, the lineages the policy lacks, their job counts.
struct FullAccessRead {
    rows: Vec<SessionSummary>,
    lineages: Vec<(SessionId, anyhow::Result<store::communication::Lineage>)>,
    counts: HashMap<SessionId, (u16, u16)>,
}

impl FullAccessRead {
    async fn fetch(store: &Store, policy: &SharedPolicy) -> Result<Self, String> {
        use crate::communication::network::access::row_eligibility;
        use crate::store::contacts::Owner;
        let mut rows = Vec::new();
        for r in store.list_sessions().await.map_err(|e| e.to_string())? {
            if matches!(store.owner_strict(r.id).await, Ok(Owner::Here))
                && row_eligibility(store, r.id).await
            {
                rows.push(r);
            }
        }
        let mut lineages = Vec::new();
        for r in &rows {
            if policy.0.read().root_of(r.id).is_err() {
                lineages.push((r.id, store.lineage(r.id).await));
            }
        }
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        Ok(Self {
            counts: view::job_counts(&ids).await,
            rows,
            lineages,
        })
    }
}

/// `ask` (internal): [`FullAccessList`] once its [`FullAccessRead`] is in.
struct FullAccessListed {
    m: crate::communication::network::access::FullAccessList,
    read: FullAccessRead,
}

impl Message<FullAccessListed> for Agents {
    type Reply = Result<Vec<crate::communication::network::access::ListedSession>, String>;

    async fn handle(
        &mut self,
        FullAccessListed { m, read }: FullAccessListed,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        use crate::communication::network::access::{self, ListedSession};
        let FullAccessRead {
            rows,
            lineages,
            counts,
        } = read;
        for (id, lineage) in lineages {
            self.apply_lineage(id, lineage);
        }
        let mut activity = self.view().activity_of_rows(rows.iter(), &counts);
        let now = Instant::now();
        let listed = rows
            .iter()
            .filter_map(|r| {
                let e = self.full_access_live(r);
                if !e.visible {
                    return None;
                }
                let kind = self.row_kind(r);
                Some(ListedSession {
                    session: r.id,
                    title: Some(r.title.clone()).filter(|t| !t.is_empty()),
                    kind,
                    state: self.row_state(r.id),
                    send: e.sendable,
                    reply: kind != PeerKind::Acp && r.observed.is_none(),
                    activity: activity
                        .remove(&r.id)
                        .map(|a| peer_activity(a, self.activity.stalled_secs(r.id, now))),
                })
            })
            .filter(|s| access::member_admits(&m.req, s, &m.machine))
            .collect();
        Ok(access::member_page(listed, m.req.after, m.req.limit))
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
        reachable: None,
    }
}

/// [`SessionActivity`] as a directory row shows it (62 §3.4): liveness
/// detail without `Awaiting { from }` / `Approval { tool }` content. `Stored`
/// when no actor is loaded; `Offline` is never derived here (no node-down
/// notion reaches the supervisor).
fn peer_activity(a: SessionActivity, stalled_secs: Option<u64>) -> PeerActivity {
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
        stalled_secs,
    }
}

/// 183 review 1: a contact's export rows listed by default; `kind: contact` lists them all.
const CONTACT_ROWS: usize = 8;

/// `discover_peers` order and `active_only` filter (62 §3.4). 183 F6 (review 1): first the
/// asking session's own tree (parent, children, siblings, ancestors, descendants), then
/// contacts with their export rows (at most [`CONTACT_ROWS`] each unless `all_contact_rows`;
/// the contact's row then says how to list the rest), then everything else (other roots,
/// stored sessions, remote agents); inside each, working rows first, then most recently
/// active, ties in [`Agents::directory`]'s order. `active_only` keeps contacts. Contact rows
/// keep their order (200: a contact's listed sessions stay under its row); answers each
/// contact's hidden row count.
fn arrange(
    rows: &mut Vec<PeerInfo>,
    active_only: bool,
    all_contact_rows: bool,
) -> HashMap<String, usize> {
    use crate::communication::is_contact_row;
    if active_only {
        rows.retain(|p| is_contact_row(p) || p.activity.as_ref().is_some_and(PeerActivity::active));
    }
    let group = |p: &PeerInfo| {
        if is_contact_row(p) {
            1
        } else if matches!(
            p.relationship,
            PeerRelationship::Parent
                | PeerRelationship::Child
                | PeerRelationship::Sibling
                | PeerRelationship::Ancestor
                | PeerRelationship::Descendant
        ) {
            0
        } else {
            2
        }
    };
    rows.sort_by_key(|p| {
        let a = p.activity.as_ref().filter(|_| !is_contact_row(p));
        (
            group(p),
            std::cmp::Reverse((
                a.is_some_and(PeerActivity::active),
                a.and_then(|a| a.last_active),
            )),
        )
    });
    let mut hidden: HashMap<String, usize> = HashMap::new();
    if all_contact_rows {
        return hidden;
    }
    let mut listed: HashMap<String, usize> = HashMap::new();
    rows.retain(|p| {
        let Some((contact, _)) = p.name.split_once('/').filter(|_| is_contact_row(p)) else {
            return true;
        };
        let n = listed.entry(contact.to_owned()).or_default();
        *n += 1;
        if *n <= CONTACT_ROWS {
            return true;
        }
        *hidden.entry(contact.to_owned()).or_default() += 1;
        false
    });
    for p in rows.iter_mut().filter(|p| is_contact_row(p)) {
        if let Some(n) = hidden.get(&p.name) {
            p.title = Some(format!(
                "+{n} more: discover_peers(kind=\"contact\", query=\"{}\")",
                p.name
            ));
        }
    }
    hidden
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
        let mut r = self
            .store
            .peer_record(m.session, m.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "unknown message".to_string())?;
        self.network.name_peers(&mut r);
        Ok(r)
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
            messages.sort_by_key(|m| std::cmp::Reverse(m.created_at));
            for r in &mut messages {
                self.network.name_peers(r);
            }
            return Ok(PeerMessagePage {
                awaiting_answer: received,
                awaiting_reply: sent,
                messages,
                cursor: None,
            });
        }
        let mut messages = self
            .store
            .peer_records(m.session, after, limit)
            .await
            .map_err(|e| e.to_string())?;
        for r in &mut messages {
            self.network.name_peers(r);
        }
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
    pub(crate) async fn new_root(
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
            // 183 §12.5: a chat started in a folder works there, as a take
            // seats it there (§5.5); a path missing here keeps the default.
            if cwd.is_dir() {
                tools::seat::set_cwd(id, Some(cwd.to_path_buf()));
            }
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
    pub(crate) async fn install_cap(&mut self, id: SessionId, cap: Risk) {
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

    /// A title set by the harness (a contact's inbox, 183 §7.6) stands: the session's first
    /// turn does not seed one from its input, as after a rename.
    pub(crate) async fn mark_titled(&self, id: SessionId) {
        if let Some(Live {
            actor: LiveActor::Native(a),
            ..
        }) = self.sessions.get(&id)
        {
            let _ = a.tell(crate::loop_::Titled).await;
        }
    }
}

/// 183 Z3-R2 (M4): the cap a child spawned now gets as its own: for a native child the
/// stricter of what the spawner asked for and the spawner's ceiling at that moment (its own,
/// an ancestor's), so it never outlives it uncapped. An external harness keeps the cap it
/// was asked with (an owner desk's ceiling rides in its brief; a contact session spawns
/// nothing at all, Z3-R4).
fn child_cap(requested: Option<Risk>, ceiling: Option<Risk>, external: bool) -> Option<Risk> {
    if external {
        return requested;
    }
    match (requested, ceiling) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
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
        // 125 §4.2: the advisor is told nothing about peers and has no `send_message`.
        if self.advisors.contains(&session) {
            return String::new();
        }
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
    /// 141 §7.7: stop watch-only `row` through the live harness actor that
    /// runs it — its parent, or the nearest ACP ancestor when nested. No
    /// live harness: nothing to tell; the row is already lost or terminal.
    async fn cancel_observed(&self, row: SessionSummary) {
        let mut parent = row.parent;
        let mut hops = 0;
        while let Some(p) = parent
            && hops < 8
        {
            if let Some(LiveActor::Acp(a)) = self.live(p) {
                let _ = a.tell(CancelObserved { child: row.id }).await;
                return;
            }
            parent = self
                .store_for(p)
                .session(p)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.parent);
            hops += 1;
        }
        tracing::info!(session = %row.id, "cancel: watch-only child has no live harness");
    }

    /// Engine start (141 §7.8, D11): non-terminal watch-only children whose
    /// harness is not live become `unknown` / "lost contact"; each is
    /// published so an open UI updates.
    async fn mark_observed_lost(&self) {
        let live: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, l)| matches!(l.actor, LiveActor::Acp(_)) && l.actor.is_alive())
            .map(|(id, _)| *id)
            .collect();
        let lost = match self.store.mark_observed_lost(&live).await {
            Ok(lost) => lost,
            Err(e) => {
                tracing::warn!("mark_observed_lost: {e:#}");
                return;
            }
        };
        for id in lost {
            let Ok(Some(row)) = self.store.session(id).await else {
                continue;
            };
            let (Some(parent), Some(observed)) = (row.parent, row.observed) else {
                continue;
            };
            self.rt
                .publish(AgentEvent::ObservedChildChanged {
                    parent,
                    child: id,
                    name: row.name.unwrap_or_default(),
                    model: row.harness_model,
                    observed,
                })
                .await;
        }
    }
}

impl Message<ListActivity> for Agents {
    type Reply = kameo::reply::DelegatedReply<Vec<AgentInfo>>;

    /// 176 E1: the session list, job counts and last replies are read off
    /// the mailbox, over the state as of this handler.
    async fn handle(
        &mut self,
        _: ListActivity,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let view = self.view();
        ctx.spawn(async move { view.activity_rows().await })
    }
}

impl Message<ActivityOf> for Agents {
    type Reply = kameo::reply::DelegatedReply<HashMap<SessionId, SessionActivity>>;

    async fn handle(&mut self, m: ActivityOf, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let view = self.view();
        ctx.spawn(async move { view.activity_for(&m.0).await })
    }
}

impl Agents {
    /// 62 §3.3 step 1: the rows `targets` names, in selector order, `before`
    /// filled, every resolved row `Skipped{"dry run"}` and an unknown id
    /// `Failed{"unknown session"}`. `Running`/`Roots` never name a child.
    async fn resolve_targets(&self, targets: &BroadcastTargets) -> Vec<BroadcastRow> {
        let view = self.view();
        let rows = view.all_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        let mut activity = view.activity_of_rows(&rows, &view::job_counts(&ids).await);
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
            let store_row = self
                .store_for(r.session)
                .session(r.session)
                .await
                .ok()
                .flatten();
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
                    None,
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
    type Reply = kameo::reply::DelegatedReply<Vec<AgentInfo>>;

    /// 176 E1: read off the mailbox like [`ListActivity`].
    async fn handle(&mut self, m: ListAgents, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let view = self.view();
        ctx.spawn(async move { view.agents(m.session).await })
    }
}

impl Message<Roster> for Agents {
    type Reply = kameo::reply::DelegatedReply<String>;

    async fn handle(&mut self, m: Roster, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let view = self.view();
        ctx.spawn(async move {
            let agents = view.agents(m.session).await;
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
        })
    }
}

impl Message<ShowPrompt> for Agents {
    type Reply = String;

    async fn handle(&mut self, m: ShowPrompt, _: &mut Context<Self, Self::Reply>) -> String {
        let peers = m.session.map(|s| self.peer_context(s)).unwrap_or_default();
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
        let subagent_effort = self.subagent_effort();
        let identity = match m.session {
            Some(s) => self.identity_of(s).await,
            None => None,
        };
        // 125 §5.4: the paragraph shows while a link exists.
        let mut advised = false;
        if let Some(s) = m.session {
            advised = self
                .store_for(s)
                .advisor_link(s)
                .await
                .ok()
                .flatten()
                .is_some();
        }
        let mut prompt = frame::baseline(
            &self.rt.data_dir,
            identity.as_deref(),
            &peers,
            &blocks,
            subagent_model.as_deref(),
            subagent_effort.as_deref(),
            attended,
            advised,
            // 125 R1.6: the root's `consult` paragraph while the cap allows the tool.
            advised && self.advisor_cfg.max_consults > 0,
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
        self.unload_session(m.session).await
    }
}

impl Agents {
    /// As `DeleteSession` stops an actor (68 §4.2): cancel the turn, stop
    /// gracefully (139 D19: the cancel is processed, never dropped), forget
    /// the live entry; rows and peer state stay. A demoted turn's
    /// `finish_turn("cancelled")` is refused `NotOwner` by the fence, which
    /// is correct — the new owner's rows stand.
    pub(super) async fn unload_session(&mut self, session: SessionId) -> bool {
        self.drop_queued(session).await;
        let Some(live) = self.sessions.remove(&session) else {
            return false;
        };
        live.actor.stop().await;
        self.running.remove(&session);
        // 178 F5: its turn is gone with the actor; a kept entry would leave
        // the next turn (here or after a move back) without a timer.
        if let Some(timer) = self.child_turns.remove(&session).and_then(|ct| ct.timer) {
            timer.abort();
        }
        crate::durability::run_clock::stop_session(session);
        self.endless.detach(session);
        true
    }
}

impl Message<ListSessions> for Agents {
    type Reply = kameo::reply::DelegatedReply<Vec<SessionSummary>>;

    /// The durable session list with each row's live `running` flag; 176
    /// E1: read off the mailbox, `running` as of this handler.
    async fn handle(
        &mut self,
        _: ListSessions,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (store, running) = (self.store.clone(), self.running.clone());
        ctx.spawn(async move {
            let mut rows = store.list_sessions().await.unwrap_or_else(|e| {
                tracing::error!("list sessions: {e:#}");
                Vec::new()
            });
            for r in &mut rows {
                r.running = running.contains(&r.id);
            }
            let connected = mesh::connected_ids();
            if let Err(e) = store
                .mark_remote_running(&mut rows, connected.as_deref())
                .await
            {
                tracing::warn!("list sessions: remote running: {e:#}");
            }
            rows
        })
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
    type Reply = kameo::reply::DelegatedReply<Result<SessionRead, String>>;

    /// 176 E1: the transcript is read and rendered off the mailbox.
    async fn handle(
        &mut self,
        m: ReadSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // 51 §4.2 / 66 §9: a private tree's sessions are readable only from
        // inside that tree; everything durable is readable from anywhere.
        let target_root = self.private_member.get(&m.session);
        if target_root.is_some() && target_root != self.private_member.get(&m.caller) {
            return ctx.reply(Err(format!(
                "session {} is private to another tree",
                m.session
            )));
        }
        let store = self.store_for(m.session).clone();
        let retention = self.retention_of(m.session);
        ctx.spawn(async move { read_session(&store, m.session, retention).await })
    }
}

/// [`ReadSession`]'s reads once the caller may see `session`.
async fn read_session(
    store: &Store,
    session: SessionId,
    retention: Retention,
) -> Result<SessionRead, String> {
    let mut summary = store
        .session(session)
        .await
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("session {session} not found"))?;
    // 125 §4.2: an advisor's transcript is readable by nobody else.
    if summary.kind == SessionKind::Advisor {
        return Err(format!("session {session} is not a peer"));
    }
    summary.retention = retention;
    let h = store
        .context_history(session)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let advisories = store.advisories(session).await.unwrap_or_default();
    let moved = store.moved(session).await.unwrap_or_default();
    let mut out = render_with(&h.messages, &h.entries, &h.thinking, &advisories, &moved);
    // Peer provenance (50 §9), as `load_session` marks it.
    if let Ok(peers) = store.peer_origins(session).await {
        mark_peers(&mut out, &peers);
    }
    // 172: channel provenance, as `load_session` marks it.
    if let Ok(channels) = store.channels(session).await {
        mark_channels(&mut out, &channels);
    }
    Ok(SessionRead {
        summary,
        history: out.into_iter().map(|(_, e)| e).collect(),
    })
}

impl Message<LoadSession> for Agents {
    type Reply = kameo::reply::DelegatedReply<Loaded>;

    /// 176 E1: the store reads and the session actor's asks run off the
    /// mailbox; only the lineage mirror and the communication state come
    /// back to it ([`Mirrored`]).
    async fn handle(
        &mut self,
        m: LoadSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let session = m.session;
        let store = self.store_for(session).clone();
        let durable = self.store.clone();
        let live = self.live(session).cloned();
        let root = self.resources_for(session).root.clone();
        let retention = self.retention_of(session);
        let known = self.communication.0.read().root_of(session).is_ok();
        let endless = self.endless.clone();
        let me = ctx.actor_ref().clone();
        ctx.spawn(async move {
            let mut loaded = load_session(m, &store, &durable, live, &root, retention).await;
            // The store's estimate prices the epochs frame: the whole raw
            // history of an endless session that never compacted.
            if loaded.context_estimate.is_some()
                && let Some(view) = endless_view_tokens(&endless, session).await
            {
                loaded.context_estimate = Some(view);
            }
            let lineage = if known {
                None
            } else {
                Some(durable.lineage(session).await)
            };
            loaded.communication = me.ask(Mirrored { session, lineage }).await.ok().flatten();
            loaded
        })
    }
}

/// 137 §8.1 (D4): what an active endless session's next turn opens with
/// besides system prompt and tools — its view blocks, no head, no carried
/// rows — priced as [`crate::loop_`]'s `system_tokens` prices them (bytes/4).
/// `None` when the session is not active in endless mode or the render
/// fails: its turns then run the epochs frame, which the store prices.
async fn endless_view_tokens(endless: &Endless, session: SessionId) -> Option<u32> {
    if !endless.active(session) {
        return None;
    }
    let blocks = endless.render_snapshot(session).await.ok()?;
    let bytes: usize = blocks.iter().map(|b| b.text.len()).sum();
    Some(u32::try_from(bytes / 4).unwrap_or(u32::MAX))
}

/// `ask` (internal, 176 E1) → `Option<CommunicationState>`: apply a lineage
/// read off the mailbox (`None`: it was already known) and read `session`'s
/// switches, as [`Agents::mirror`] then `GetCommunication` would.
struct Mirrored {
    session: SessionId,
    lineage: Option<anyhow::Result<store::communication::Lineage>>,
}

impl Message<Mirrored> for Agents {
    type Reply = Option<CommunicationState>;

    async fn handle(&mut self, m: Mirrored, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(lineage) = m.lineage {
            self.apply_lineage(m.session, lineage);
        }
        self.communication.0.read().state(Some(m.session)).ok()
    }
}

/// [`LoadSession`]'s reads over the handler's snapshot; `communication` is
/// left `None` for the mailbox to fill.
async fn load_session(
    m: LoadSession,
    store: &Store,
    durable: &Store,
    live: Option<LiveActor>,
    root: &Path,
    retention: Retention,
) -> Loaded {
    // Raw rows and their context metadata in one consistent read (48 §5):
    // through the session actor when it is live (it owns admission and
    // fills editable/reason), from the store when no actor exists (idle
    // by construction) or the session runs on an external harness.
    let row = store.session(m.session).await.unwrap_or_else(|e| {
        tracing::error!(session = %m.session, "load session: {e:#}");
        None
    });
    let external = row.as_ref().is_some_and(|r| r.agent.is_some());
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
    let (running, resumed, queued) = match &live {
        Some(actor) => (
            actor.is_running().await,
            actor.resumed_turn().await,
            match actor.native() {
                Ok(a) => a.ask(QueuedInputs).await.unwrap_or_default(),
                Err(_) => Vec::new(),
            },
        ),
        None => (false, None, Vec::new()),
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
                thinking,
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
            // 125 §5.4: Advisory rows render as typed cards (`role: "advisory"`).
            let advisories = store.advisories(m.session).await.unwrap_or_default();
            let moved = store.moved(m.session).await.unwrap_or_default();
            for (seq, entry) in render_with(&messages, &entries, &thinking, &advisories, &moved) {
                while let Some(e) = dividers.next_if(|e| e.at_seq < seq) {
                    out.push((e.at_seq, compacted(e, heads.get(&e.n).cloned())));
                }
                out.push((seq, entry));
            }
            out.extend(dividers.map(|e| (e.at_seq, compacted(e, heads.get(&e.n).cloned()))));
            // 172: the channel that delivered each user row (`messages.channel`).
            match store.channels(m.session).await {
                Ok(channels) => mark_channels(&mut out, &channels),
                Err(e) => tracing::error!(session = %m.session, "channels: {e:#}"),
            }
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
            // Typed provenance for replay (50 §9): this engine's consumption
            // records, else the replicated `messages.peer`, by history row.
            match store.peer_origins(m.session).await {
                Ok(peers) => mark_peers(&mut out, &peers),
                Err(e) => tracing::error!(session = %m.session, "peer_origins: {e:#}"),
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
            materialize(&mut history, &messages, &epochs, root);
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
    let undone = store.hidden_turns(m.session).await.unwrap_or_else(|e| {
        tracing::error!(session = %m.session, "hidden_turns: {e:#}");
        0
    });
    let (model, effort, agent, harness_model, context_size, name, parent, project, kind, observed) =
        row.map(|r| {
            (
                r.model,
                r.effort,
                r.agent,
                r.harness_model,
                r.context_size,
                r.name,
                r.parent,
                r.project,
                r.kind,
                r.observed,
            )
        })
        .unwrap_or_default();
    // 75 §3: the harness's manifest, only where a harness exists.
    let harness_commands = if external {
        store.harness_commands(m.session).await.unwrap_or_else(|e| {
            tracing::error!(session = %m.session, "harness_commands: {e:#}");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let (context_revision, context_editable, context_reason, context_estimate, settled) = context;
    // 47 §5.1: the committed plan, independent of paging and of a running
    // turn; a failed read is `None` (unavailable), never an empty plan.
    let todos = store
        .todos(m.session)
        .await
        .map_err(|e| tracing::error!(session = %m.session, "todos: {e:#}"))
        .ok();
    // 109 W5 §10.3: the open turn's state and its partial, after everything
    // stored. `resumed` tells a relaunched open turn from one that is
    // simply running (no banner for the latter).
    let (resume, partial) = Agents::resume_state(durable, m.session, running, resumed).await;
    let mut history = history;
    history.extend(partial);
    Loaded {
        history,
        running,
        model,
        effort,
        queued,
        undone,
        agent,
        harness_model,
        harness_commands,
        context_size,
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
        communication: None,
        project,
        retention,
        resume,
        kind,
        observed,
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

impl Message<SessionContextBreakdown> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<ContextBreakdown, String>>;

    /// 176 E1: the store reads and the session actor's ask run off the mailbox.
    async fn handle(
        &mut self,
        m: SessionContextBreakdown,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let store = self.store_for(m.session).clone();
        let live = self.live(m.session).and_then(|a| a.native().ok()).cloned();
        let endless = self.endless.clone();
        ctx.spawn(async move {
            store
                .session(m.session)
                .await
                .map_err(|e| format!("{e:#}"))?
                .ok_or_else(|| "unknown session".to_owned())?;
            if let Some(a) = live
                && let Ok(Some(b)) = a.ask(ContextBreakdownNow).await
            {
                return Ok(b);
            }
            // An active endless session's frame is its view (the live actor
            // answered above once an endless turn ran), counted with the
            // system share as the loop counts it.
            if let Some(view) = endless_view_tokens(&endless, m.session).await {
                return Ok(ContextBreakdown::build(
                    view,
                    None,
                    std::iter::empty::<&ChatMessage>(),
                    None,
                    None,
                ));
            }
            // Busy or not running: the stored frame, without the live turn's
            // system prompt, calibration and provider bill.
            let (_, head, tail) = store.frame(m.session).await.map_err(|e| format!("{e:#}"))?;
            Ok(ContextBreakdown::build(
                0,
                head.as_ref(),
                tail.iter().map(|(_, m)| m),
                None,
                None,
            ))
        })
    }
}

impl Message<SessionMemoryQuery> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137 §11.2: a store read for any session this engine knows — `mode:
    /// epochs` and an empty view when the session is not endless.
    async fn handle(
        &mut self,
        m: SessionMemoryQuery,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .memory(m.session)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<EndlessRetry> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137F: any session this engine knows, like [`SessionMemoryQuery`].
    async fn handle(&mut self, m: EndlessRetry, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .retry(m.session)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<EndlessFold> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<SessionMemory, String>>;

    /// 137G: any session this engine knows, like [`EndlessRetry`]. 201 (P5):
    /// a chat on the endless frame takes a trim now through its actor
    /// ([`CompactNow`]: at once when idle, at the next turn boundary when a
    /// turn runs); a meeting-filtered tree keeps the 137G fold.
    async fn handle(
        &mut self,
        m: EndlessFold,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if let Err(e) = self.stored(m.session).await {
            tx.send(Err(e));
            return reply;
        }
        let endless = self.endless.clone();
        if endless.active(m.session) && !endless.meeting(m.session) {
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
            // Off the mailbox, as [`Compact`]: the actor may ask us back.
            tokio::spawn(async move {
                let r = match actor
                    .ask(CompactNow {
                        style: None,
                        head: None,
                    })
                    .await
                {
                    Ok(_) => endless
                        .memory(m.session)
                        .await
                        .map_err(|e| format!("{e:#}")),
                    Err(e) => Err(e.to_string()),
                };
                tx.send(r);
            });
            return reply;
        }
        tx.send(endless.fold(m.session).await.map_err(|e| format!("{e:#}")));
        reply
    }
}

impl Message<EndlessPause> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137G: any session this engine knows, like [`EndlessRetry`].
    async fn handle(&mut self, m: EndlessPause, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .pause(m.session, m.paused)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<SetSessionCompactorModel> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137G: any session this engine knows, like [`EndlessRetry`].
    async fn handle(
        &mut self,
        m: SetSessionCompactorModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .set_model(m.session, m.model)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<SetSessionCompactorEffort> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 194: any session this engine knows, like [`SetSessionCompactorModel`].
    async fn handle(
        &mut self,
        m: SetSessionCompactorEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .set_effort(m.session, m.effort)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<SetAgentCompactorModel> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137G: the roster's desk default, like [`SetSessionCompactorModel`].
    async fn handle(
        &mut self,
        m: SetAgentCompactorModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .set_agent_model(m.session, m.model, m.effort)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<EndlessZoom> for Agents {
    type Reply = Result<Vec<ViewLine>, String>;

    async fn handle(&mut self, m: EndlessZoom, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless.zoom(m.session, m.id, m.n).await
    }
}

impl Message<EndlessRecall> for Agents {
    type Reply = Result<Vec<ViewLine>, String>;

    async fn handle(
        &mut self,
        m: EndlessRecall,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        self.endless
            .recall(m.session, m.query.trim(), m.limit.unwrap_or(8).max(1))
            .await
    }
}

impl Message<SetSessionMemory> for Agents {
    type Reply = kameo::reply::DelegatedReply<anyhow::Result<SetMode>>;

    /// 137 §8.6 / R1-D6: the switch runs on the session's actor as a
    /// mini-turn ([`SetMemoryNow`]) so `off` can install the view head the way
    /// a compaction does; the actor refuses with `busy` while a turn runs.
    async fn handle(
        &mut self,
        m: SetSessionMemory,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if self.running.contains(&m.session) {
            tx.send(Err(anyhow::anyhow!("busy")));
            return reply;
        }
        let actor = match self
            .actor(m.session)
            .await
            .and_then(|a| a.native().cloned())
        {
            Ok(actor) => actor,
            Err(e) => {
                tx.send(Err(anyhow::anyhow!(e)));
                return reply;
            }
        };
        // `off` rewrites the context (a new epoch): old candidates are stale.
        self.ledger.bump(m.session);
        tokio::spawn(async move {
            tx.send(
                actor
                    .ask(SetMemoryNow {
                        mode: m.mode,
                        filter: m.filter,
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}")),
            );
        });
        reply
    }
}

impl Message<RenderMemoryView> for Agents {
    type Reply = anyhow::Result<Vec<ViewLine>>;

    /// 137 R1-D6: the view folded to `bytes` for a meeting head, as lines
    /// (W1's `render_with_budget` directly); empty for a non-endless session.
    /// Any session this engine knows.
    async fn handle(
        &mut self,
        m: RenderMemoryView,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        crate::endless_ext::render_with_budget(&self.endless, m.session, m.bytes).await
    }
}

impl Message<SetSessionViewBytes> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 137 R1-D2b: idle or running — it only takes effect at the next turn boundary.
    async fn handle(
        &mut self,
        m: SetSessionViewBytes,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        crate::endless_ext::set_view_bytes(&self.endless, m.session, m.bytes).await
    }
}

impl Message<SetSessionAgentTail> for Agents {
    type Reply = ();

    /// 201 §4.2: the desk agent's tail keys, read at the session's next trim.
    async fn handle(
        &mut self,
        m: SetSessionAgentTail,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.endless.set_agent_tail(m.session, m.tail);
    }
}

impl Message<SetSessionTail> for Agents {
    type Reply = Result<SessionMemory, String>;

    /// 201 §4.2: idle or running — phase P2 reads it at the next trim.
    async fn handle(
        &mut self,
        m: SetSessionTail,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stored(m.session).await?;
        crate::endless_ext::set_tail(&self.endless, m.session, m.tail).await
    }
}

impl Message<GetEndlessSettings> for Agents {
    type Reply = Result<EndlessConfig, String>;

    /// The live section (`Endless::config`); `Result` because a bare config struct is no kameo `Reply`.
    async fn handle(
        &mut self,
        _: GetEndlessSettings,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.endless.config())
    }
}

impl Message<SetEndlessSettings> for Agents {
    type Reply = Result<EndlessConfig, String>;

    /// 137 §11.3: the Settings → Memory writer. Ranges as §11.1 documents
    /// them; one `write_nested_config_value` per key (the writer is a scalar
    /// writer); the live config is replaced so new sessions, every pump,
    /// the roster and the meeting plugin see it.
    async fn handle(
        &mut self,
        m: SetEndlessSettings,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut c = m.config;
        // 201 §12.1: the new keys' ranges and the two high-above-low pairs.
        c.validate_201()?;
        if !(16_000..=256_000).contains(&c.view_bytes) {
            return Err(format!(
                "view_bytes {} out of range 16000..=256000",
                c.view_bytes
            ));
        }
        if !(0.0..=0.5).contains(&c.slack) {
            return Err(format!("slack {} out of range 0.0..=0.5", c.slack));
        }
        if !(128..=4_096).contains(&c.node_bytes) {
            return Err(format!(
                "node_bytes {} out of range 128..=4096",
                c.node_bytes
            ));
        }
        if c.jobs == 0 || c.tries == 0 {
            return Err("jobs and tries must be at least 1".into());
        }
        if let Some(id) = c.model.as_deref()
            && let Err(e) = self.catalog.resolve(id, &*self.configured)
        {
            return Err(format!("model: {e}"));
        }
        // 194 D4b–d: the effort follows `model` — cleared with it, dropped
        // when a changed model does not take it, refused otherwise.
        c.effort = c
            .effort
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty());
        if c.model.is_none() {
            c.effort = None;
        } else if let Some(e) = c.effort.clone()
            && let Err(why) = self.check_effort(c.model.as_deref(), &e)
        {
            if c.model != self.endless.config().model {
                c.effort = None;
            } else {
                return Err(format!("effort: {why}"));
            }
        }
        let path = self.rt.data_dir.join("config.yaml");
        if !(16_000..=256_000).contains(&c.meeting_view_bytes) {
            return Err(format!(
                "meeting_view_bytes {} out of range 16000..=256000",
                c.meeting_view_bytes
            ));
        }
        // 137F E6/E8/E9.
        for (key, v, lo, hi) in [
            ("attempt_secs", c.attempt_secs, 10, 3_600),
            ("local_attempt_secs", c.local_attempt_secs, 30, 7_200),
            ("local_ctx_bytes", c.local_ctx_bytes, 4_000, 256_000),
            ("local_jobs", u32::from(c.local_jobs), 1, 16),
            // 137G G2/G3/G6.
            ("raw_bytes", c.raw_bytes, 4_000, 128_000),
            ("compactor_ctx_bytes", c.compactor_ctx_bytes, 8_000, 256_000),
            ("batch_units", u32::from(c.batch_units), 1, 64),
        ] {
            if !(lo..=hi).contains(&v) {
                return Err(format!("{key} {v} out of range {lo}..={hi}"));
            }
        }
        // 137G G11.
        if !(0.001..=10.0).contains(&c.max_usd_per_call) {
            return Err(format!(
                "max_usd_per_call {} out of range 0.001..=10",
                c.max_usd_per_call
            ));
        }
        if !(0.0..=1_000.0).contains(&c.max_usd_per_session_day) {
            return Err(format!(
                "max_usd_per_session_day {} out of range 0..=1000",
                c.max_usd_per_session_day
            ));
        }
        let keys: [(&str, String); 38] = [
            ("default_chat", c.default_chat.to_string()),
            ("default_desk", c.default_desk.to_string()),
            ("default_meeting", c.default_meeting.to_string()),
            ("view_bytes", c.view_bytes.to_string()),
            ("meeting_view_bytes", c.meeting_view_bytes.to_string()),
            ("backfill_max_units", c.backfill_max_units.to_string()),
            ("node_bytes", c.node_bytes.to_string()),
            ("slack", c.slack.to_string()),
            ("settle_secs", c.settle_secs.to_string()),
            ("jobs", c.jobs.to_string()),
            ("tries", c.tries.to_string()),
            ("retry_secs", c.retry_secs.to_string()),
            ("cap_chars", c.cap_chars.to_string()),
            ("attempt_secs", c.attempt_secs.to_string()),
            ("local_attempt_secs", c.local_attempt_secs.to_string()),
            ("local_ctx_bytes", c.local_ctx_bytes.to_string()),
            ("local_jobs", c.local_jobs.to_string()),
            ("raw_bytes", c.raw_bytes.to_string()),
            ("compactor_ctx_bytes", c.compactor_ctx_bytes.to_string()),
            ("batch_units", c.batch_units.to_string()),
            ("max_usd_per_call", c.max_usd_per_call.to_string()),
            (
                "max_usd_per_session_day",
                c.max_usd_per_session_day.to_string(),
            ),
            // 201 §12.1.
            ("tail_mode", c.tail_mode.as_str().to_owned()),
            ("tail_low_bytes", c.tail_low_bytes.to_string()),
            ("tail_high_bytes", c.tail_high_bytes.to_string()),
            ("tail_low_share", c.tail_low_share.to_string()),
            ("tail_high_share", c.tail_high_share.to_string()),
            ("cold_after_secs", c.cold_after_secs.to_string()),
            ("profile", c.profile.as_str().to_owned()),
            ("merge_at", c.merge_at.to_string()),
            ("ledger", c.ledger.to_string()),
            ("ledger_bytes", c.ledger_bytes.to_string()),
            ("ledger_suggestions", c.ledger_suggestions.to_string()),
            ("prefetch_bytes", c.prefetch_bytes.to_string()),
            ("age_by_use", c.age_by_use.to_string()),
            ("describe_images", c.describe_images.as_str().to_owned()),
            (
                "model",
                c.model.clone().unwrap_or_else(|| "null".to_owned()),
            ),
            // Quoted so a bare budget (`"8000"`) stays a YAML string.
            (
                "effort",
                c.effort
                    .as_ref()
                    .map_or_else(|| "null".to_owned(), |e| format!("\"{e}\"")),
            ),
        ];
        for (key, value) in &keys {
            runtime::write_nested_config_value(&path, "agent", "endless", key, value)
                .map_err(|e| format!("config.yaml: {e}"))?;
        }
        Arc::make_mut(&mut self.cfg).endless = c.clone();
        // Live (closer): the service's shared config, then the roster and the
        // meeting plugin over the bus.
        self.endless.set_config(c.clone());
        self.rt.publish(EndlessConfigChanged(c.clone())).await;
        tracing::info!(default_chat = c.default_chat, default_desk = c.default_desk, default_meeting = c.default_meeting, view_bytes = c.view_bytes, model = ?c.model, effort = ?c.effort, "endless settings written");
        Ok(c)
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
        self.rename(m.session, &m.title, false).await
    }
}

impl Message<UserRenameSession> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: UserRenameSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        self.rename(m.session, &m.title, true).await
    }
}

impl Agents {
    /// [`RenameSession`]; `by_user` (a human's rename, [`UserRenameSession`]) also marks
    /// the session `user_titled` (212 §11), stored and live.
    async fn rename(
        &mut self,
        session: SessionId,
        title: &str,
        by_user: bool,
    ) -> Result<(), String> {
        let title: String = title.trim().chars().take(200).collect();
        if title.is_empty() {
            return Err("empty title".into());
        }
        let store = self.store_for(session);
        store
            .set_title(session, &title)
            .await
            .map_err(|e| e.to_string())?;
        // 212 §11: the title model never overwrites a user's name, nor does `set_status`.
        if by_user {
            store
                .set_user_titled(session)
                .await
                .map_err(|e| e.to_string())?;
        }
        // A rename changes the title only; the event repeats the stored tags.
        let tags = store
            .session(session)
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
        }) = self.sessions.get(&session)
        {
            let _ = a.tell(crate::loop_::Titled).await;
            if by_user {
                let _ = a.tell(crate::loop_::UserTitled).await;
            }
        }
        self.rt
            .publish(AgentEvent::TitleChanged {
                session,
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
        // 148 R1 F5: a put-away desk is no desk; its model entry goes with it.
        if m.agent.is_none() {
            self.desk_models.remove(&m.session);
            self.desk_efforts.remove(&m.session);
        }
        if let Some(name) = &m.agent {
            #[derive(serde::Deserialize, Default)]
            struct Memory {
                #[serde(default)]
                isolated_memory: bool,
            }
            let isolated = if name == "theuth" {
                false
            } else {
                let text = std::fs::read_to_string(
                    self.rt.data_dir.join("agents").join(format!("{name}.md")),
                )
                .map_err(|e| format!("isolated memory unavailable here: {e}"))?;
                pause::front_matter::<Memory>(&text)?
                    .0
                    .unwrap_or_default()
                    .isolated_memory
            };
            self.store_for(m.session)
                .set_memory_identity(m.session, name, isolated)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        self.store_for(m.session)
            .set_desk(m.session, m.agent.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<SessionModel> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        m: SessionModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.store_for(m.0)
            .session(m.0)
            .await
            .ok()
            .flatten()
            .map(|row| row.model)
    }
}

impl Message<SessionEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        m: SessionEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.store_for(m.0)
            .session(m.0)
            .await
            .ok()
            .flatten()
            .and_then(|row| row.effort)
    }
}

impl Message<SetDeskModel> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: SetDeskModel, _: &mut Context<Self, Self::Reply>) {
        // 194: the desk's effective model decides whether it takes the effort.
        let model = m
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        self.desk_models.insert(m.session, m.model);
        let effort = self.desk_effort(m.session, &model, m.effort);
        let had = match &effort {
            Some(e) => self.desk_efforts.insert(m.session, e.clone()),
            None => self.desk_efforts.remove(&m.session),
        };
        // A desk whose agent never configured an effort keeps its own (`/effort`);
        // a cleared one goes back to the default for new sessions.
        if effort.is_none() && had.is_none() {
            return;
        }
        self.put_desk_effort(m.session, &model, effort).await;
    }
}

impl Message<SetDeskEffort> for Agents {
    type Reply = ();

    /// 194 R1: unlike [`SetDeskModel`], always puts the row on the setting.
    async fn handle(&mut self, m: SetDeskEffort, _: &mut Context<Self, Self::Reply>) {
        let model = m.model.unwrap_or_else(|| self.default_model.clone());
        let effort = self.desk_effort(m.session, &model, m.effort);
        self.put_desk_effort(m.session, &model, effort).await;
    }
}

impl Agents {
    /// 194: the agent's configured effort if the desk's effective `model` takes it
    /// (dropped with a warning otherwise).
    fn desk_effort(&self, id: SessionId, model: &str, effort: Option<String>) -> Option<String> {
        effort.filter(|e| {
            self.check_effort(Some(model), e)
                .map_err(|err| tracing::warn!(session = %id, "desk effort dropped: {err}"))
                .is_ok()
        })
    }

    /// 194: put the desk row on `effort`, `None` meaning the default for new sessions
    /// when `model` takes it — live through [`Self::apply_effort`] when its actor runs,
    /// else on the row its actor will load. No-op when it already is.
    async fn put_desk_effort(&self, id: SessionId, model: &str, effort: Option<String>) {
        let effort = effort.or_else(|| {
            self.default_effort
                .clone()
                .filter(|e| self.check_effort(Some(model), e).is_ok())
        });
        let Ok(Some(row)) = self.store_for(id).session(id).await else {
            return;
        };
        if row.effort == effort || row.agent.is_some() {
            return;
        }
        match self.live(id).and_then(|a| a.native().ok()).cloned() {
            Some(actor) => self.apply_effort(id, &actor, effort).await,
            None => {
                if let Err(e) = self.store_for(id).set_effort(id, effort.as_deref()).await {
                    tracing::warn!(session = %id, "desk effort: {e:#}");
                }
            }
        }
    }
}

impl Message<DeskSession> for Agents {
    type Reply = Option<SessionId>;

    async fn handle(
        &mut self,
        m: DeskSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<SessionId> {
        self.store.desk_session(&m.0).await.unwrap_or_else(|e| {
            tracing::error!(agent = %m.0, "desk session lookup: {e:#}");
            None
        })
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
        // An archived child's queued spawn turn never starts (01 §2.6).
        if m.archived {
            self.drop_queued(m.session).await;
        }
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
        // 178 W8: before the rows go — a parent outside the tree is told once.
        child_waits::child_gone(self, &ids, "was deleted", &ids).await;
        for id in &ids {
            self.drop_queued(*id).await;
            self.ledger.forget(*id);
            self.bound.retain(|(s, _), _| s != id);
            self.delivered.retain(|(s, _), _| s != id);
            self.turn_text.retain(|(s, _), _| s != id);
            self.child_errors.retain(|(s, _), _| s != id);
            self.owned_children.remove(id);
            self.caps.remove(id);
            self.requested_models.remove(id);
            self.remote_parent.remove(id);
            self.desk_models.remove(id);
            self.desk_efforts.remove(id);
            self.parked.remove(id);
            self.parking.remove(id);
            if let Some(live) = self.sessions.remove(id) {
                live.actor.kill().await;
            }
            self.endless.detach(*id);
            // R3-1: a durable session's zoom scratch (a private one's goes
            // with its tree's scratch root).
            crate::loop_::endless_tools::forget(&self.rt.data_dir.join("zoom"), *id);
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
        let hide_google =
            google_shadowed(&self.auth.gateways.read().unwrap_or_else(|e| e.into_inner()));
        let mut out: Vec<ModelInfo> = self
            .catalog
            .providers()
            .filter(|p| (self.configured)(Catalog::adapter(&p.id)))
            .filter(|p| !(hide_google && p.id == "google"))
            .flat_map(|p| {
                let local = self.provider_local(&p.id);
                p.models
                    .values()
                    .filter(|m| !go_free(&p.id, &m.id))
                    .map(move |m| ModelInfo {
                        local,
                        ..ModelInfo::new(&p.id, m)
                    })
            })
            .collect();
        // Cursor is not in models.dev: ask the backend what the account may use.
        if self.auth.login("cursor") {
            match self.auth.oauth.access(&oauth::CURSOR).await {
                Ok(t) => match crate::backend::cursor_models(&t.access_token).await {
                    Ok(models) => out.extend(
                        models
                            .into_iter()
                            .map(|m| cursor_model_info(&self.catalog, m)),
                    ),
                    Err(e) => tracing::warn!("cursor models: {e}"),
                },
                Err(e) => tracing::warn!("{e}"),
            }
        }
        out.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        out
    }
}

/// OpenCode Go's free-tier models (`*-free`) answer 403 to every client but
/// OpenCode itself, key or not: they are left out of the picker and the count.
fn go_free(provider: &str, id: &str) -> bool {
    provider == "opencode-go" && id.ends_with("-free")
}

/// The picker lists Gemini once: a custom provider on Google's Gemini API host
/// (now spoken natively, `backend::gateway_kind`) serves the built-in
/// `google/*` models, so the user's configured provider stays and the
/// built-in `google` block is hidden (still resolvable when typed) — unless
/// that provider is itself named `google` (then the block is its own).
fn google_shadowed(gateways: &HashMap<String, crate::providers::Gateway>) -> bool {
    !gateways.contains_key("google")
        && gateways
            .values()
            .any(|g| crate::backend::gemini_api_host(&g.base_url))
}

/// A Cursor listing row, its limits borrowed from the closest catalog
/// sibling. 194: only a family whose wire takes a `reasoning` parameter
/// (`cursor::takes_reasoning`) offers the sibling's efforts — the rest carry
/// their thinking in the id, so the picker shows no chips it cannot honour.
fn cursor_model_info(catalog: &Catalog, m: cursor::Model) -> ModelInfo {
    let id = format!("cursor/{}", m.id);
    let near = catalog.closest(&id);
    let tunable = near.filter(|_| cursor::takes_reasoning(&m.id));
    ModelInfo {
        context: near.map_or(0, |n| n.limit.context),
        reasoning: near.is_some_and(|n| n.reasoning),
        // Cursor's backend is text-only whatever the sibling says
        // (`backend::supports_images`).
        image: false,
        efforts: tunable.map(|n| n.efforts()).unwrap_or_default(),
        budget_min: tunable.and_then(|n| n.budget()).map(|b| b.0),
        budget_max: tunable.and_then(|n| n.budget()).and_then(|b| b.1),
        id,
        name: m.name,
        provider: "cursor".into(),
        local: false,
    }
}

impl Message<SetModel> for Agents {
    type Reply = Result<ModelInfo, String>;

    async fn handle(
        &mut self,
        m: SetModel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ModelInfo, String> {
        self.set_model(m).await
    }
}

impl Message<crate::GetInfoDefaults> for Agents {
    type Reply = Result<crate::InfoDefaults, String>;

    /// 176 E3: the fields the per-field `Get*` handlers answer, read alike.
    async fn handle(
        &mut self,
        _: crate::GetInfoDefaults,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(crate::InfoDefaults {
            default_model: self.default_model.clone(),
            default_effort: self.default_effort.clone(),
            favorite_models: self.favorite_models.clone(),
            compact_style: *self.compact_style.read().unwrap_or_else(|p| p.into_inner()),
            subagent_timeout_secs: self.cfg.subagents.max_runtime_secs,
            max_tool_rounds: self.cfg.max_tool_rounds,
            stream_idle_secs: self.cfg.stream_idle_secs,
            max_salvages: self.cfg.max_salvages,
            communication: self
                .communication
                .0
                .read()
                .state(None)
                .map_err(|e| e.to_string())?,
            advisor: self.advisor_cfg.defaults(),
            status_every: self.cfg.status_every,
            hint_limit: self.cfg.knowledge.hint_limit,
        })
    }
}

impl Message<GetDefaultModel> for Agents {
    type Reply = String;

    async fn handle(&mut self, _: GetDefaultModel, _: &mut Context<Self, Self::Reply>) -> String {
        self.default_model.clone()
    }
}

impl Message<GetFavoriteModels> for Agents {
    type Reply = Vec<String>;

    async fn handle(
        &mut self,
        _: GetFavoriteModels,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<String> {
        self.favorite_models.clone()
    }
}

impl Message<SetModelFavorite> for Agents {
    type Reply = Result<Vec<String>, String>;

    async fn handle(
        &mut self,
        m: SetModelFavorite,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Vec<String>, String> {
        self.set_model_favorite(m).await
    }
}

impl Agents {
    /// 127 §1: star or unstar `m.model` (resolved like [`Self::set_model`]);
    /// when the list changes it is written to `agent.favorite_models` as one
    /// flow-sequence line and announced as `FavoriteModelsChanged`.
    async fn set_model_favorite(&mut self, m: SetModelFavorite) -> Result<Vec<String>, String> {
        let id = self.catalog.resolve(&m.model, &*self.configured)?.id;
        let at = self.favorite_models.iter().position(|f| *f == id);
        match (at, m.favorite) {
            (None, true) => self.favorite_models.push(id),
            (Some(i), false) => {
                self.favorite_models.remove(i);
            }
            _ => return Ok(self.favorite_models.clone()),
        }
        // JSON array syntax is valid YAML and survives ids with `:` / `.`.
        let value = format!(
            "[{}]",
            self.favorite_models
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "agent", "favorite_models", &value) {
            tracing::warn!("config.yaml: {e}");
        }
        self.rt
            .publish(AgentEvent::FavoriteModelsChanged {
                models: self.favorite_models.clone(),
            })
            .await;
        Ok(self.favorite_models.clone())
    }
}

impl Agents {
    /// 148 R1: with a session, switch that session only — whatever `manual`
    /// says, the default for new sessions is untouched. With no session, a
    /// manual pick sets the default (`agent.model` in config.yaml, announced as
    /// `DefaultModelChanged`): the Settings path. A programmatic call with no
    /// session is a no-op beyond resolving the id.
    async fn set_model(&mut self, m: SetModel) -> Result<ModelInfo, String> {
        let resolved = self.catalog.resolve(&m.model, &*self.configured)?;
        if let Some(id) = m.session {
            let Some(row) = self
                .store_for(id)
                .session(id)
                .await
                .map_err(|e| e.to_string())?
            else {
                return Err(format!("unknown session {id}"));
            };
            if row.agent.is_some() {
                return Err(NOT_NATIVE_OPTION.into());
            }
            let actor = self.actor(id).await?.native()?.clone();
            let _ = actor.tell(SwitchModel(resolved.clone())).await;
            // An effort the new model does not take would 400 its next call: drop it.
            let drop_effort = row.effort.as_ref().is_some_and(|e| {
                resolved
                    .model
                    .as_ref()
                    .is_some_and(|m| m.validate_effort(e).is_err())
            });
            // 137F E1 / R1-6: the compactor chain's last link, live — the final
            // model and effort at once (the drop below then changes nothing).
            let effort = if drop_effort {
                None
            } else {
                row.effort.as_deref().and_then(effort_of)
            };
            self.endless
                .set_session_model(id, Some(resolved.clone()), effort);
            if let Err(e) = self.store_for(id).set_model(id, &resolved.id).await {
                tracing::warn!(session = %id, "set_model: {e:#}");
            }
            self.rt
                .publish(AgentEvent::ModelChanged {
                    session: id,
                    model: resolved.id.clone(),
                })
                .await;
            if drop_effort {
                self.apply_effort(id, &actor, None).await;
            }
        }
        if m.session.is_none() && m.manual {
            self.default_model = resolved.id.clone();
            // ponytail: writes <data_dir>/config.yaml; a `--config <other>` file is not written back.
            let path = self.rt.data_dir.join("config.yaml");
            if let Err(e) = runtime::write_config_value(&path, "agent", "model", &resolved.id) {
                tracing::warn!("config.yaml: {e}");
            }
            self.rt
                .publish(AgentEvent::DefaultModelChanged {
                    model: resolved.id.clone(),
                })
                .await;
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
        self.endless
            .set_session_model(id, None, effort.as_deref().and_then(effort_of));
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
        self.set_effort(m).await
    }
}

impl Message<GetDefaultEffort> for Agents {
    type Reply = Option<String>;

    async fn handle(
        &mut self,
        _: GetDefaultEffort,
        _: &mut Context<Self, Self::Reply>,
    ) -> Option<String> {
        self.default_effort.clone()
    }
}

impl Agents {
    /// 148 R1: with a session, switch that session only — whatever `manual`
    /// says. With no session, a manual change sets the default
    /// (`agent.effort` in config.yaml, announced as `DefaultEffortChanged`):
    /// the Settings path. A programmatic change with no session is a no-op
    /// beyond validating the value.
    async fn set_effort(&mut self, m: SetEffort) -> Result<(), String> {
        let target = match m.session {
            Some(id) => {
                let row = self
                    .store_for(id)
                    .session(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown session {id}"))?;
                if row.agent.is_some() {
                    return Err(NOT_NATIVE_OPTION.into());
                }
                row.model
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
        if m.session.is_none() && m.manual {
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
            self.rt
                .publish(AgentEvent::DefaultEffortChanged { effort: m.effort })
                .await;
        }
        Ok(())
    }
}

/// `set_model` / `set_effort` on an ACP child (75 §8): the harness's own
/// option list, not the catalog, is what it can switch to.
const NOT_NATIVE_OPTION: &str =
    "an external harness picks from its own options; use harness_options / set_harness_option";

/// `ask` → `Result<Vec<acp::ConfigOption>, String>` (75 §8): the config
/// options of `session`'s live harness process; `Err` when the session is
/// not an ACP child or its process is gone.
#[derive(Clone, Copy, Debug)]
pub struct HarnessOptions {
    /// An ACP child.
    pub session: SessionId,
}

/// `ask` → `Result<acp::SetOptionResult, String>` (75 §8): set one config
/// option on `session`'s live harness — `config_id` is the option's id or the
/// alias `model` / `effort`, `value` one of its listed values. Mid-turn the
/// change is queued for the turn's end (`pending: true`). A missing option
/// or an unlisted value is refused with the list.
#[derive(Clone, Debug)]
pub struct SetHarnessOption {
    /// An ACP child.
    pub session: SessionId,
    /// Option id or alias.
    pub config_id: String,
    /// One of `ConfigOption.options[].value`.
    pub value: String,
}

impl Agents {
    /// The live ACP actor of `session`; the refusal otherwise. Never resumes
    /// a stopped child: a gone process has no options to show or set.
    async fn harness_actor(&self, session: SessionId) -> Result<ActorRef<AcpChild>, String> {
        let row = self.stored(session).await?;
        if row.agent.is_none() {
            return Err("not an external harness; use set_model / set_effort".into());
        }
        match self.live(session) {
            Some(LiveActor::Acp(a)) => Ok(a.clone()),
            _ => Err(format!(
                "{}: harness process not running",
                row.agent.unwrap_or_default()
            )),
        }
    }
}

impl Message<HarnessOptions> for Agents {
    type Reply = Result<Vec<acp::ConfigOption>, String>;

    async fn handle(
        &mut self,
        m: HarnessOptions,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.harness_actor(m.session)
            .await?
            .ask(crate::acp_child::HarnessOptions)
            .await
            // `SendError::HandlerError` displays as the inner `String`.
            .map_err(|e| e.to_string())
    }
}

impl Message<SetHarnessOption> for Agents {
    type Reply = Result<acp::SetOptionResult, String>;

    async fn handle(
        &mut self,
        m: SetHarnessOption,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.harness_actor(m.session)
            .await?
            .ask(crate::acp_child::SetOption {
                config_id: m.config_id,
                value: m.value,
            })
            .await
            .map_err(|e| e.to_string())
    }
}

/// `ask` → `Result<ChildResumed, String>` (75 §10, 146 D8): put a stopped
/// ACP child back in its harness context — a new harness process opened with
/// `session/load` on the row's stored ACP session id, its skills written
/// back into the cwd — through the same [`Agents::relaunch_acp_child`] the
/// boot pass uses. A child that was mid-prompt when it died (its last row a
/// prompt / dangling `ToolCall`, `Store::midprompt`) gets the respawn prompt
/// and continues its work (`Prompted`); a cancelled or idle one is only
/// reconnected (`Idle`; the 75 §10 cancelled note when its last turn was
/// cancelled). A live actor for the session is stopped first (its process
/// group dies with the connection: the 06 §8.3 orphan), so the next
/// `send_message` goes to the new process. Refused for a native session, a
/// remote node's session (its ACP session belongs to another engine) and a
/// row without a stored harness session id. Explicit user intent: the
/// parent's state is not a gate here (unlike the boot pass).
#[derive(Clone, Copy, Debug)]
pub struct ResumeChild {
    /// An ACP child.
    pub session: SessionId,
}

/// What [`ResumeChild`] did (146 D8): the toast's two sentences.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ChildResumed {
    /// The harness is back and was prompted to continue its interrupted task.
    Prompted,
    /// The harness is back in its context; it was idle, nothing to continue.
    Idle,
}

impl Message<ResumeChild> for Agents {
    type Reply = Result<ChildResumed, String>;

    async fn handle(&mut self, m: ResumeChild, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let row = self.stored(m.session).await?;
        if row.observed.is_some() {
            return Err(OBSERVED.into());
        }
        let Some(agent) = row.agent.as_deref() else {
            return Err("not an external harness; only an ACP child can be resumed".into());
        };
        let (_, engine) = crate::peers::parse_agent(agent)?;
        if !acp::load(&self.rt.data_dir)
            .map_err(|e| e.to_string())?
            .contains_key(engine)
        {
            return Err(format!(
                "`{agent}` is not a local harness in agents.yaml; a node's session cannot be resumed here"
            ));
        }
        if self
            .store
            .acp_session(m.session)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(format!(
                "{agent}: no harness session id stored; nothing to resume"
            ));
        }
        // The replayed turn ended `cancelled` — the last one this process
        // saw, or the one the shutdown below is about to cut — so the first
        // prompt tells the harness not to pick it up (75 §10).
        let mut cancelled = self
            .last_stop
            .get(&m.session)
            .is_some_and(|(r, _)| r == "cancelled");
        if let Some(Live {
            actor: LiveActor::Acp(a),
            ..
        }) = self.sessions.remove(&m.session)
            && a.is_alive()
        {
            cancelled |= a.ask(IsRunning).await.unwrap_or(false);
            let _ = a.tell(crate::acp_child::Shutdown).await;
            a.wait_for_shutdown().await;
        }
        // 146 D8: a child killed mid-prompt (an engine restart, a crashed
        // harness) continues its task; the button must not "do nothing".
        let midprompt = !cancelled
            && self
                .store
                .midprompt(m.session)
                .await
                .map_err(|e| e.to_string())?;
        let first = if cancelled {
            acp_resume::Continue::Cancelled
        } else if midprompt {
            acp_resume::Continue::Prompt
        } else {
            acp_resume::Continue::Idle
        };
        self.relaunch_acp_child(m.session, first).await?;
        tracing::info!(session = %m.session, ?first, "146: resume_child");
        Ok(if midprompt {
            ChildResumed::Prompted
        } else {
            ChildResumed::Idle
        })
    }
}

/// 176 A1: how often idle ACP children are looked for.
const PARK_EVERY: Duration = Duration::from_secs(60);
/// 176 A1: longest one child's park probe may take; a busy child mailbox
/// is skipped this round rather than holding this one.
const PARK_PROBE: Duration = Duration::from_secs(2);

/// `tell` (176 A1): park every external ACP child idle at least `.0` — its
/// harness process stopped, its row kept; the next input resumes it
/// through `session()` → `acp_session` (`session/load`), the path a
/// restart's lazy resume takes. The handler only snapshots; the Jobs ask
/// and the probes run on a task ([`park_round`]) that reports
/// [`ParkRound`]. Until then the candidates are in `Agents.parking`, where
/// `route` and `enqueue_peer` hold their inputs, so nothing is told into a
/// mailbox that is about to stop.
#[derive(Clone, Copy, Debug)]
pub struct ParkIdle(pub Duration);

impl Message<ParkIdle> for Agents {
    type Reply = ();

    async fn handle(&mut self, ParkIdle(after): ParkIdle, ctx: &mut Context<Self, ()>) {
        let children: Vec<(SessionId, ActorRef<AcpChild>, bool)> = self
            .sessions
            .iter()
            .filter_map(|(id, l)| match &l.actor {
                LiveActor::Acp(a) if a.is_alive() && !self.parking.contains_key(id) => {
                    let pending = self.running.contains(id)
                        || self.approvals.contains_key(id)
                        || self.waits.values().any(|w| w.session == *id)
                        || self.child_turns.contains_key(id)
                        || self.queued.contains_key(id);
                    Some((*id, a.clone(), pending))
                }
                _ => None,
            })
            .collect();
        if children.is_empty() {
            return;
        }
        for (id, ..) in &children {
            self.parking.insert(*id, Vec::new());
        }
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let round = park_round(children, after).await;
            let _ = me.tell(ParkRound(round)).await;
        });
    }
}

/// 176 A1, off the mailbox: the Jobs counts, then every child probed at
/// once, each bounded by [`PARK_PROBE`]. `(session, actor, parked)` per
/// candidate; a Jobs actor that does not answer parks none this round.
async fn park_round(
    children: Vec<(SessionId, ActorRef<AcpChild>, bool)>,
    after: Duration,
) -> Vec<(SessionId, kameo::actor::ActorId, bool)> {
    let ids: Vec<SessionId> = children.iter().map(|(id, ..)| *id).collect();
    // No Jobs actor, or a stopped one, runs no jobs (`activity_for` reads it
    // the same way).
    let counts = match ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR) {
        Ok(Some(jobs)) => {
            match tokio::time::timeout(PARK_PROBE, jobs.ask(tools::JobCounts(ids))).await {
                Ok(r) => Some(r.unwrap_or_default()),
                Err(_) => None,
            }
        }
        _ => Some(HashMap::new()),
    };
    futures::future::join_all(children.into_iter().map(|(id, actor, pending)| {
        let jobs = counts.as_ref().map(|c| {
            let (jobs, ptys) = c.get(&id).copied().unwrap_or_default();
            u32::from(jobs) + u32::from(ptys)
        });
        async move {
            let parked = match jobs {
                Some(jobs) => {
                    let probe = crate::acp_child::ParkIfIdle {
                        after,
                        pending,
                        jobs,
                        until: Instant::now() + PARK_PROBE,
                    };
                    matches!(
                        tokio::time::timeout(PARK_PROBE, actor.ask(probe)).await,
                        Ok(Ok(true))
                    )
                }
                None => false,
            };
            (id, actor.id(), parked)
        }
    }))
    .await
}

/// `tell` (internal, 176 A1): a park round's outcome. A parked child leaves
/// `sessions` (when the entry is still that actor) for `parked`; either way
/// the inputs held meanwhile are replayed — into the live child, or through
/// `session()` into its resume.
struct ParkRound(Vec<(SessionId, kameo::actor::ActorId, bool)>);

impl Message<ParkRound> for Agents {
    type Reply = ();

    async fn handle(&mut self, ParkRound(round): ParkRound, _: &mut Context<Self, ()>) {
        for (id, actor, parked) in round {
            // Gone from `parking`: the session was deleted meanwhile.
            let Some(held) = self.parking.remove(&id) else {
                continue;
            };
            let same = self
                .sessions
                .get(&id)
                .is_some_and(|l| matches!(&l.actor, LiveActor::Acp(a) if a.id() == actor));
            if parked && same {
                self.sessions.remove(&id);
                self.parked.insert(id, Instant::now());
            }
            self.replay(id, held).await;
        }
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

/// genai's OpenRouter base URL, shown on its Connections row and listed by Refresh.
const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/";

impl Agents {
    /// The Connections row of built-in or custom provider `name` (85 §0.9, 162 D6);
    /// `None` for a name that is neither.
    fn provider_info(&self, name: &str) -> Option<ProviderInfo> {
        let models = self
            .catalog
            .providers()
            .find(|p| p.id == name)
            .map_or(0, |p| {
                p.models.keys().filter(|id| !go_free(name, id)).count() as u32
            });
        let (base_url, builtin, configured, has_key, gw) = match name {
            "opencode" => {
                let k = self.auth.opencode_key().is_some();
                (crate::backend::ZEN_ENDPOINT.to_owned(), true, k, k, None)
            }
            "opencode-go" => {
                let k = self.auth.opencode_go_key().is_some();
                (crate::backend::GO_BASE.to_owned(), true, k, k, None)
            }
            "openrouter" => {
                let k = self.auth.openrouter_key().is_some();
                (OPENROUTER_ENDPOINT.to_owned(), true, k, k, None)
            }
            _ => {
                let g = self.auth.gateway(name)?;
                (g.base_url.clone(), false, true, !g.key.is_empty(), Some(g))
            }
        };
        Some(ProviderInfo {
            name: name.to_owned(),
            base_url,
            models,
            builtin,
            configured,
            // Go lists its models publicly, keyed or not.
            refreshable: configured || name == "opencode-go",
            local: gw.as_ref().is_some_and(|g| g.local()),
            runtime: gw.and_then(|g| g.runtime),
            has_key,
        })
    }

    /// 177: `provider` is a local model server for the tag (a local gateway by
    /// the broad host rule, or a block the listing marked local).
    fn provider_local(&self, provider: &str) -> bool {
        self.auth.gateway(provider).is_some_and(|g| g.local()) || self.provider_on_host(provider)
    }

    /// 177 R2-5: `provider` runs on this machine (a preset or loopback
    /// gateway, or a block marked local) — its unknown windows are assumed.
    fn provider_on_host(&self, provider: &str) -> bool {
        self.auth
            .gateway(provider)
            .is_some_and(|g| g.on_this_machine())
            || self
                .catalog
                .providers()
                .any(|p| p.id == provider && p.local)
    }

    /// Swap in a reloaded catalog everywhere it is read.
    async fn reload_catalog(&mut self) {
        self.catalog = Arc::new(load_catalog(&self.rt.data_dir, &self.auth));
        self.costs.set_catalog(self.catalog.clone());
        self.endless.set_catalog(self.catalog.clone());
        // 177 E6: the role models (title, polish, subagent, vision, escalate)
        // take their new entries; running chats take theirs at their next call.
        for slot in [
            &self.title_model,
            &self.polish_model,
            &self.subagent_model,
            &self.vision_model,
            &self.escalate_model,
            &self.knowledge_harvester_model,
        ] {
            let mut slot = slot.write().unwrap_or_else(|e| e.into_inner());
            if let Some(r) = slot.as_mut()
                && let Some((_, m)) = self.catalog.get(&r.id)
            {
                r.model = Some(m.clone());
            }
        }
        for live in self.sessions.values() {
            if let LiveActor::Native(a) = &live.actor
                && live.actor.is_alive()
            {
                let _ = a.tell(CatalogChanged(self.catalog.clone())).await;
            }
        }
    }

    /// 177 R2-7: the longest a provider's model listing may take (it runs on
    /// the Agents mailbox).
    const LISTING_TIMEOUT: Duration = Duration::from_secs(10);

    /// List gateway `name`'s models (`GET {base}models`, keyless when it has no
    /// key) with the context windows the server reports (177 E5), and merge
    /// them into `models.json` (162 D4).
    async fn list_gateway(
        &self,
        name: &str,
        g: &crate::providers::Gateway,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let auth = if g.key.is_empty() {
            AuthData::None
        } else {
            AuthData::Key(g.key.clone())
        };
        // R2-7: a black-holed host must not hold the Agents mailbox.
        let ids = tokio::time::timeout(
            Self::LISTING_TIMEOUT,
            self.client.all_model_names(
                AdapterKind::OpenAI,
                (Endpoint::from_owned(g.base_url.clone()), auth),
            ),
        )
        .await
        .map_err(|_| {
            format!(
                "listing {name}'s models: timed out after {}s",
                Self::LISTING_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("listing {name}'s models: {e}"))?;
        let contexts = crate::local::server_contexts(&g.base_url, &g.key, g.local(), &ids).await;
        crate::providers::write_models(
            &self.rt.data_dir,
            name,
            &ids,
            crate::providers::Listing::Custom {
                local: g.on_this_machine(),
                contexts: &contexts,
            },
        )
    }

    /// 162 D3: ask `name`'s model-list endpoint again, merge the ids into
    /// `models.json` (D4), and reload the catalog and costs' price table.
    async fn refresh_provider_models(&mut self, name: &str) -> Result<RefreshedModels, String> {
        let (kind, endpoint, key) = match name {
            "opencode" => (
                AdapterKind::OpenAI,
                crate::backend::ZEN_ENDPOINT.to_owned(),
                self.auth.opencode_key(),
            ),
            // Go's listing is public; no key is sent.
            "opencode-go" => (
                AdapterKind::OpenAI,
                format!("{}v1/", crate::backend::GO_BASE),
                None,
            ),
            "openrouter" => (
                AdapterKind::OpenRouter,
                OPENROUTER_ENDPOINT.to_owned(),
                self.auth.openrouter_key(),
            ),
            _ => {
                let (added, removed) = match self.auth.gateway(name) {
                    Some(g) if g.key.is_empty() && !g.local() => {
                        return Err(format!("{name} has no key; set it first"));
                    }
                    Some(g) => self.list_gateway(name, &g).await?,
                    None if matches!(name, "ollama" | "omlx") => {
                        return Err(format!(
                            "{name} is built in; add it under Local servers to list its models"
                        ));
                    }
                    None if self.catalog.providers().any(|p| p.id == name)
                        || oauth::provider(name).is_some()
                        || AdapterKind::from_lower_str(name).is_some() =>
                    {
                        return Err(format!("{name}'s models come from the built-in catalog"));
                    }
                    None => return Err(format!("no provider {name}")),
                };
                self.reload_catalog().await;
                let provider = self
                    .provider_info(name)
                    .ok_or_else(|| format!("no provider {name}"))?;
                return Ok(RefreshedModels {
                    provider,
                    added,
                    removed,
                });
            }
        };
        let auth = match key.filter(|k| !k.is_empty()) {
            Some(k) => AuthData::Key(k),
            None if name == "opencode-go" => AuthData::None,
            None => return Err(format!("{name} has no key; set it first")),
        };
        let ids = tokio::time::timeout(
            Self::LISTING_TIMEOUT,
            self.client
                .all_model_names(kind, (Endpoint::from_owned(endpoint), auth)),
        )
        .await
        .map_err(|_| {
            format!(
                "listing {name}'s models: timed out after {}s",
                Self::LISTING_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("listing {name}'s models: {e}"))?;
        let catalog = self.catalog.clone();
        let (added, removed) = crate::providers::write_models(
            &self.rt.data_dir,
            name,
            &ids,
            crate::providers::Listing::Builtin(&catalog),
        )?;
        self.reload_catalog().await;
        let provider = self
            .provider_info(name)
            .ok_or_else(|| format!("no provider {name}"))?;
        Ok(RefreshedModels {
            provider,
            added,
            removed,
        })
    }
}

impl Message<ListProviders> for Agents {
    type Reply = Vec<ProviderInfo>;

    async fn handle(
        &mut self,
        _: ListProviders,
        _: &mut Context<Self, Self::Reply>,
    ) -> Vec<ProviderInfo> {
        let mut custom: Vec<String> = self
            .auth
            .gateways
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        custom.sort();
        // 85 §0.9: the built-ins first, keyed by a secret rather than a providers.yaml row.
        ["opencode", "opencode-go", "openrouter"]
            .map(str::to_owned)
            .into_iter()
            .chain(custom)
            .filter_map(|name| self.provider_info(&name))
            .collect()
    }
}

impl Message<AddProvider> for Agents {
    type Reply = Result<ProviderInfo, String>;

    async fn handle(
        &mut self,
        m: AddProvider,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<ProviderInfo, String> {
        let base_url = crate::providers::normalize_url(&m.base_url)?;
        let mut g = crate::providers::Gateway {
            base_url,
            key: m.key.as_deref().unwrap_or_default().trim().to_owned(),
            runtime: m.runtime,
            protocol: None,
        };
        crate::providers::validate_name(&m.name, g.local())?;
        if g.key.is_empty() && !g.local() {
            return Err("api key is empty".into());
        }
        let secrets = crate::providers::secrets_actor()?;
        g.protocol = crate::providers::save(
            &self.rt.data_dir,
            &secrets,
            &m.name,
            &g.base_url,
            &g.key,
            g.runtime,
        )
        .await?;
        self.auth
            .gateways
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(m.name.clone(), g.clone());
        // 162 D4: the same writer as Refresh; a failed listing keeps the existing list.
        if let Err(e) = self.list_gateway(&m.name, &g).await {
            tracing::warn!(provider = %m.name, "{e}; keeping the existing list");
        }
        self.reload_catalog().await;
        self.provider_info(&m.name)
            .ok_or_else(|| format!("no provider {}", m.name))
    }
}

impl Message<RefreshProviderModels> for Agents {
    type Reply = Result<RefreshedModels, String>;

    async fn handle(
        &mut self,
        m: RefreshProviderModels,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<RefreshedModels, String> {
        self.refresh_provider_models(&m.name).await
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
        self.reload_catalog().await;
        Ok(())
    }
}

impl Message<ProbeLocalServers> for Agents {
    type Reply = kameo::reply::DelegatedReply<Vec<LocalServer>>;

    async fn handle(
        &mut self,
        _: ProbeLocalServers,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let providers: Vec<(String, String)> = self
            .auth
            .gateways
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(n, g)| (n.clone(), g.base_url.clone()))
            .collect();
        tokio::spawn(async move {
            tx.send(crate::local::probe(&crate::local::probe_ports(), &providers).await);
        });
        reply
    }
}

impl Agents {
    /// 177: `provider`'s models with their stored limits — a custom provider's
    /// from the user `models.json` (what its listing and the user wrote), a
    /// catalog provider's from the catalog.
    fn provider_models(&self, provider: &str) -> Result<Vec<ProviderModel>, String> {
        let local = self.provider_on_host(provider);
        let custom = self.auth.gateway(provider).is_some();
        let models: Vec<models::Model> = if custom {
            crate::providers::read_models(&self.rt.data_dir)?
                .remove(provider)
                .map(|p| p.models.into_values().collect())
                .unwrap_or_default()
        } else {
            self.catalog
                .providers()
                .find(|p| p.id == provider)
                .ok_or_else(|| format!("no provider {provider}"))?
                .models
                .values()
                .cloned()
                .collect()
        };
        Ok(models
            .iter()
            .map(|m| provider_model(m, local, custom))
            .collect())
    }
}

/// 177: one [`ProviderModel`] row. A stored source wins; an entry without one
/// but with limits is a hand edit (custom) or the bundled catalog (`server`).
pub(crate) fn provider_model(m: &models::Model, local: bool, custom: bool) -> ProviderModel {
    let known = m.limit.context > 0 || m.limit.output > 0;
    let source = match m.limit.source {
        models::LimitSource::Unknown if known && custom => models::LimitSource::User,
        models::LimitSource::Unknown if known => models::LimitSource::Server,
        s => s,
    };
    ProviderModel {
        id: m.id.clone(),
        name: m.name.clone(),
        context: m.limit.context,
        output: m.limit.output,
        source,
        assumed_context: (local && m.limit.context == 0).then_some(crate::local::ASSUMED_CONTEXT),
    }
}

impl Message<ProviderModels> for Agents {
    type Reply = Result<Vec<ProviderModel>, String>;

    async fn handle(
        &mut self,
        m: ProviderModels,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.provider_models(&m.provider)
    }
}

impl Message<SetModelLimits> for Agents {
    type Reply = Result<ProviderModel, String>;

    async fn handle(
        &mut self,
        m: SetModelLimits,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let stored = crate::providers::set_limits(
            &self.rt.data_dir,
            &self.catalog,
            &m.provider,
            &m.model,
            m.context,
            m.output,
        )?;
        // E6: the same reload as Refresh (the endless compactors re-resolve).
        self.reload_catalog().await;
        Ok(provider_model(
            &stored,
            self.provider_on_host(&m.provider),
            self.auth.gateway(&m.provider).is_some(),
        ))
    }
}

impl Message<TestSummaryCall> for Agents {
    type Reply = kameo::reply::DelegatedReply<SummaryTest>;

    async fn handle(
        &mut self,
        m: TestSummaryCall,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let endless = self.endless.clone();
        tokio::spawn(async move {
            tx.send(endless.test_summary(&m.model).await);
        });
        reply
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
        reports: e.reports(),
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
            local: crate::endless::compactor::local_name(provider),
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
            local: r
                .provider
                .as_deref()
                .is_some_and(crate::endless::compactor::local_name),
        }
    }
}

/// 172: put each user row's stored channel origin (`Store::channels`) on its
/// [`HistoryEntry::User`] entry.
pub(crate) fn mark_channels(
    out: &mut [(i64, HistoryEntry)],
    channels: &BTreeMap<i64, ChannelOrigin>,
) {
    for (seq, entry) in out {
        if let HistoryEntry::User { channel, .. } = entry
            && let Some(c) = channels.get(seq)
        {
            *channel = Some(c.clone());
        }
    }
}

/// 50 §9: put each user row's peer provenance (`Store::peer_origins`) on its
/// [`HistoryEntry::User`] entry.
fn mark_peers(out: &mut [(i64, HistoryEntry)], peers: &BTreeMap<i64, PeerOrigin>) {
    for (seq, entry) in out {
        if let HistoryEntry::User { peer, .. } = entry
            && let Some(o) = peers.get(seq)
        {
            *peer = Some(o.clone());
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
/// call still running has no stored response and so no target. `thinking`
/// is the per-row thinking time (91) the `Reasoning` entry carries.
/// `advisories` is the session's Advisory metadata (125 §5.4,
/// `Store::advisories`): such a user row becomes [`HistoryEntry::Advisory`]
/// and, like a vision note, never opens a turn.
pub(crate) fn render_with(
    messages: &[(i64, ChatMessage)],
    entries: &BTreeMap<i64, ContextEntry>,
    thinking: &BTreeMap<i64, u64>,
    advisories: &BTreeMap<i64, crate::Advisory>,
    moved: &BTreeMap<i64, crate::SessionMoved>,
) -> Vec<(i64, HistoryEntry)> {
    let mut out = Vec::new();
    // ponytail: linear scan; one round's calls, never more than a handful.
    // (call row seq, call part index, call)
    let mut pending: Vec<(i64, u32, &genai::chat::ToolCall)> = Vec::new();
    let mut current_turn: Option<i64> = None;
    // 52 §6: a typed vision note neither opens a turn nor hides the next
    // real user message's handle; the store decides from the metadata.
    let openers = store::turn_openers(messages, |s| {
        entries.get(&s).is_some_and(|e| e.vision.is_some())
            || advisories.contains_key(&s)
            || moved.contains_key(&s)
    });
    for (seq, m) in messages {
        let opens = openers.contains(seq);
        if opens {
            current_turn = Some(*seq);
        }
        let context = entries.get(seq);
        match m.role {
            ChatRole::User if advisories.contains_key(seq) => out.push((
                *seq,
                HistoryEntry::Advisory {
                    advisory: advisories[seq].clone(),
                    context: context.cloned(),
                },
            )),
            ChatRole::User if moved.contains_key(seq) => out.push((
                *seq,
                HistoryEntry::Moved {
                    moved: moved[seq].clone(),
                    context: context.cloned(),
                },
            )),
            ChatRole::User => out.push((
                *seq,
                HistoryEntry::User {
                    // 74 §3, §5: the stamp is the model's; the row's `at` is the human's clock.
                    text: clock::strip_owned(text_of(&m.content)),
                    turn: opens.then_some(*seq),
                    context: context.cloned(),
                    peer: None,
                    // 52 §6: the consultation card renders from the typed metadata.
                    vision: context.and_then(|c| c.vision.clone()),
                    // 172: set from `Store::channels` by `mark_channels`.
                    channel: None,
                },
            )),
            ChatRole::Assistant => {
                // 70 THINKING-TRACES: the thinking blocks precede the reply they led to.
                let reasoning = m.content.reasoning_contents().concat();
                if !reasoning.trim().is_empty() {
                    out.push((
                        *seq,
                        HistoryEntry::Reasoning {
                            text: reasoning,
                            turn: current_turn,
                            context: context.cloned(),
                            thinking_ms: thinking.get(seq).copied(),
                        },
                    ));
                }
                let text = text_of(&m.content);
                if !text.is_empty() {
                    out.push((
                        *seq,
                        HistoryEntry::Assistant {
                            text,
                            turn: current_turn,
                            // 197 §5.1: sources ride on the cards, fences here.
                            context: context.map(|c| ContextEntry {
                                sources: Vec::new(),
                                ..c.clone()
                            }),
                        },
                    ));
                }
                pending.extend(
                    m.content.iter().enumerate().filter_map(|(i, p)| {
                        Some((*seq, u32::try_from(i).ok()?, p.as_tool_call()?))
                    }),
                );
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
                        .position(|(_, _, c)| c.call_id == r.call_id)
                        .map(|i| pending.remove(i));
                    let (name, args) = call.map_or(("?", &serde_json::Value::Null), |(_, _, c)| {
                        (c.fn_name.as_str(), &c.fn_arguments)
                    });
                    // 197 §5/§8: a `show`/`app`/`page_preview` card's source
                    // piece lives on its call row, which renders no entry of
                    // its own; the card carries it.
                    let sources: Vec<ContextPiece> = call
                        .and_then(|(s, at, _)| Some((entries.get(&s)?, at)))
                        .map(|(e, at)| {
                            e.sources
                                .iter()
                                .filter(|p| matches!(p.target, ContextTarget::Source { part, .. } if part == at))
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
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
                        sources,
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
    out.extend(pending.into_iter().map(|(seq, _, c)| {
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

/// [`render_with`] without Advisory metadata (test fixtures).
#[cfg(test)]
pub(crate) fn render(
    messages: &[(i64, ChatMessage)],
    entries: &BTreeMap<i64, ContextEntry>,
    thinking: &BTreeMap<i64, u64>,
) -> Vec<(i64, HistoryEntry)> {
    render_with(
        messages,
        entries,
        thinking,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
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
    /// `agent.openrouter` (85 §0.6), applied by the backend to OpenRouter requests.
    pub(crate) openrouter: Arc<crate::OpenRouterConfig>,
}

impl Auth {
    /// No vault, no logins, no gateways; `dir` holds the (empty) OAuth store.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn stub(dir: &Path) -> Self {
        Self {
            vault: None,
            oauth: oauth::Store::new(dir),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            aws_profile: Arc::new(parking_lot::RwLock::new(None)),
            openrouter: Arc::default(),
        }
    }

    fn aws_profile(&self) -> Option<String> {
        self.aws_profile.read().clone()
    }

    /// The current vault value for genai namespace `ns`, read live.
    pub(crate) fn key(&self, ns: &str) -> Option<String> {
        self.vault.as_ref()?.get(ns, 0).map(|k| k.to_string())
    }

    /// 85 §0.1: the Zen key — vault `opencode`, else `OPENCODE_API_KEY`. Read by
    /// the backend's protocol target, never by genai's resolver.
    pub(crate) fn opencode_key(&self) -> Option<String> {
        self.key("opencode").or_else(|| env_key("OPENCODE_API_KEY"))
    }

    /// The OpenCode Go key — vault `opencode-go` (its Settings row), else the
    /// genai-namespace name `opencode_go` keys were stored under before, else
    /// `OPENCODE_GO_API_KEY`. Read by the backend's protocol target and limits.
    pub(crate) fn opencode_go_key(&self) -> Option<String> {
        self.key("opencode-go")
            .or_else(|| self.key("opencode_go"))
            .or_else(|| env_key("OPENCODE_GO_API_KEY"))
    }

    /// 85 §0.6: the OpenRouter key — vault `openrouter` (preferred) or genai's
    /// `open_router`, else `OPENROUTER_API_KEY` or `OPEN_ROUTER_API_KEY`.
    pub(crate) fn openrouter_key(&self) -> Option<String> {
        self.key("openrouter")
            .or_else(|| self.key("open_router"))
            .or_else(|| env_key("OPENROUTER_API_KEY"))
            .or_else(|| env_key("OPEN_ROUTER_API_KEY"))
    }

    /// A login is in force for genai namespace `ns`: no key in the vault and
    /// tokens on disk.
    pub(crate) fn login(&self, ns: &str) -> bool {
        self.key(ns).is_none() && oauth::provider(ns).is_some() && self.oauth.load(ns).is_some()
    }

    /// The custom provider (OpenAI-compatible gateway) for user-facing provider id `ns`.
    /// Its key is read from the vault on every call (162 D5); the copy taken at start
    /// or add is used only when the vault has none.
    pub(crate) fn gateway(&self, ns: &str) -> Option<crate::providers::Gateway> {
        let mut g = self.gateways.read().ok()?.get(ns).cloned()?;
        if let Some(key) = self.key(ns).filter(|k| !k.is_empty()) {
            g.key = key;
        }
        Some(g)
    }
}

/// A non-empty environment variable.
fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
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

/// Common non-routing configuration for built-in and complete protocol targets.
fn client_builder() -> genai::ClientBuilder {
    Client::builder()
}

/// Complete provider targets need no resolvers. Reuse their connection pool and
/// return initialization errors rather than panicking on the request path.
pub(crate) fn protocol_client() -> Result<&'static Client, String> {
    static CLIENT: std::sync::LazyLock<Result<Client, String>> =
        std::sync::LazyLock::new(|| client_builder().build().map_err(|e| e.to_string()));
    CLIENT.as_ref().map_err(Clone::clone)
}

/// genai client whose keys come from the secrets vault (read per request, so
/// a new key is live), falling back to each adapter's env var. A login in
/// force (design doc 05 §2.3) rewrites the target instead: OpenAI → the
/// ChatGPT Codex backend; Anthropic → the same Messages API with the Claude
/// Code bearer and betas.
pub(crate) fn client(auth: Auth) -> anyhow::Result<Client> {
    let keys = auth.clone();
    Ok(client_builder()
        .with_auth_resolver_fn(move |m: ModelIden| {
            // SigV4's AuthData is a profile, not a vault key. Resolve it per request below.
            if m.adapter_kind == AdapterKind::BedrockSigv4 {
                return Ok(Some(AuthData::None));
            }
            // 85 §0.6: OpenRouter accepts the `openrouter` name and env too.
            if m.adapter_kind == AdapterKind::OpenRouter {
                return Ok(keys.openrouter_key().map(AuthData::Key));
            }
            // No vault key: `None` hands genai the adapter's own default
            // (its env var, or anonymous for local servers such as omlx and
            // ollama). Forcing `from_env` here made a keyless local model
            // fail before any request left the process.
            Ok(keys.key(m.adapter_kind.as_lower_str()).map(AuthData::Key))
        })
        // Login rewrites apply only to genai-resolved built-in ids.
        // Theuth-built protocol targets never pass through this resolver.
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
                            let cc = crate::claude_code::global().current();
                            target.auth = AuthData::RequestOverride {
                                url: "https://api.anthropic.com/v1/messages?beta=true".into(),
                                headers: Headers::from(vec![
                                    ("Authorization", format!("Bearer {}", t.access_token)),
                                    ("anthropic-version", "2023-06-01".to_owned()),
                                    ("User-Agent", cc.user_agent()),
                                    ("anthropic-beta", cc.betas),
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

/// The merged catalog with every local custom provider marked (177 E7:
/// assumed windows, no cloud siblings).
fn load_catalog(data_dir: &Path, auth: &Auth) -> Catalog {
    let gateways = auth.gateways.read().unwrap_or_else(|e| e.into_inner());
    Catalog::load_with(data_dir, &|p| {
        gateways.get(p).is_some_and(|g| g.on_this_machine())
    })
}

/// The auth-resolver rule as a predicate over genai namespaces: a key in
/// the vault, a login on disk, the adapter's default env var set, or an
/// adapter needing no key (`ollama`). `cursor` is not a genai adapter: only
/// a login configures it. Bedrock has no key env, so SigV4 counts as
/// configured only when [`bedrock_configured`] finds credentials to use.
/// `opencode` (Zen), `opencode_go` (Go) and `open_router` are keyed by name or env (85 §2).
pub(crate) fn configured(auth: Auth) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |ns: &str| {
        match ns {
            "bedrock_sigv4" => return bedrock_configured(&auth),
            "opencode" => return auth.opencode_key().is_some(),
            "opencode_go" => return auth.opencode_go_key().is_some(),
            "open_router" => return auth.openrouter_key().is_some(),
            _ => {}
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

/// 136 §2.1: knowledge's "may this session consolidate" answerer, backed by
/// `sessions` ([`Store::may_consolidate`]).
struct StoreConsolidator(Store);

#[async_trait::async_trait]
impl knowledge::Consolidator for StoreConsolidator {
    async fn may_consolidate(&self, session: SessionId) -> bool {
        self.0.may_consolidate(session).await.unwrap_or(false)
    }
}

/// Open the store, spawn and register the supervisor.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let mut cfg = agent_config(rt)?;
    // Design doc 30 §12: its own top-level section, carried on `cfg` so the loop sees it.
    if rt.config.contains("surface") {
        cfg.surface = rt.section("surface")?;
    }
    // 109 §10.1: its own top-level section, carried on `cfg` like `surface`.
    if rt.config.contains("durability") {
        cfg.durability = rt.section("durability")?;
    }
    // 125 §6.5: the top-level `advisor:` section (the `surface:` pattern).
    if rt.config.contains("advisor") {
        cfg.advisor = rt.section("advisor")?;
    }
    // 176 A1: the top-level `acp:` section (the `surface:` pattern).
    if rt.config.contains("acp") {
        cfg.acp = rt.section("acp")?;
    }
    // 212 §15: `knowledge.peek`, carried on `cfg` so each session snapshots it.
    cfg.knowledge_peek = rt
        .section::<knowledge::KnowledgeConfig>("knowledge")
        .map_or(true, |k| k.peek);
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
            if rt2
                .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
                .await
                .is_none()
            {
                return;
            }
            match store.replicate_records().await {
                Ok((n, handle)) => {
                    tracing::info!(
                        "accounting: projects/cost_owners replicated ({n} row(s) announced)"
                    );
                    crate::rpc_suffixes::watch(rt2, handle);
                }
                Err(e) => tracing::info!("accounting: replication unavailable ({e}); local only"),
            }
        });
    }
    // 136 §2.1: knowledge's consolidation gate — "is this session an unattended
    // root (the dreaming routine) or the owner's own root (no parent, no desk)?"
    // answered from `sessions`. Knowledge starts before us; wait for its actor.
    {
        let rt2 = rt.clone();
        let store = store.clone();
        tokio::spawn(async move {
            if let Some(k) = rt2
                .lookup_when::<knowledge::KnowledgeActor>(knowledge::KNOWLEDGE_ACTOR)
                .await
            {
                let _ = k
                    .ask(knowledge::RegisterConsolidator(Arc::new(
                        StoreConsolidator(store),
                    )))
                    .await;
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
        openrouter: Arc::new(cfg.openrouter.clone()),
    };
    auth.oauth.migrate_files().await;
    crate::limits::LimitsHub::global().start(rt, auth.clone()); // 160:LIM pollers
    crate::claude_code::start(&rt.data_dir, auth.clone(), rt.shutdown.clone());
    // A harness-wide ceiling on Python workers (08 §9), decoupled from the
    // per-tree child caps (01 §2.6); lazy, so idle trees cost nothing.
    // The worker is this binary (08 §9).
    let pool = match std::env::current_exe() {
        Ok(exe) => repl::open_pool(&exe, repl::REPL_POOL_MAX).await,
        Err(e) => {
            tracing::warn!("cannot locate own executable ({e}); python runtime disabled");
            None
        }
    };
    spill::sweep(
        &rt.data_dir.join("spill"),
        Duration::from_secs(7 * 24 * 3600),
    );
    let catalog = Arc::new(load_catalog(&rt.data_dir, &auth));
    let configured = configured(auth.clone());
    // Unresolvable → warn and fall back to the session model rather than fail every title.
    let title_model = cfg.title_model.as_deref().and_then(|id| {
        catalog
            .resolve(id, &*configured)
            .map_err(|e| tracing::warn!("agent.title_model: {e}"))
            .ok()
    });
    // 153 §9.2: the polish slot lives under `voice.`, read the same way; unresolvable → warn, chain falls through.
    let polish_model = rt
        .section::<voice::VoiceConfig>("voice")
        .ok()
        .and_then(|v| v.polish_model)
        .and_then(|id| {
            catalog
                .resolve(&id, &*configured)
                .map_err(|e| tracing::warn!("voice.polish_model: {e}"))
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
    // Same leniency as the title model: an unresolvable id harvests with the session's model.
    let knowledge_harvester_model = cfg.knowledge_harvester_model.as_deref().and_then(|id| {
        catalog
            .resolve(id, &*configured)
            .map_err(|e| {
                tracing::warn!(
                    "agent.knowledge_harvester_model: {e}; harvests use the session's model"
                )
            })
            .ok()
    });
    // 194: the side-call roles' own efforts; one its model does not take is dropped at call time.
    let role_efforts = crate::loop_::RoleEfforts {
        title: cfg.title_effort.clone(),
        vision: cfg.vision_effort.clone(),
        escalate: cfg.escalate_effort.clone(),
        polish: rt
            .section::<voice::VoiceConfig>("voice")
            .ok()
            .and_then(|v| v.polish_effort),
        knowledge_harvester: cfg.knowledge_harvester_effort.clone(),
    };
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
    // 183 §6.1 (D3, D6): absent → on once this engine has a paired machine (source `mesh`),
    // re-evaluated on mesh events ([`Agents::mesh_default`]); an explicit value wins.
    let (enabled, source) = match cfg.communication {
        Some(on) => (on, source),
        None => {
            let paired = match mesh::paired_count() {
                Some(n) => n > 0,
                None => mesh::has_stored_peers(&rt.data_dir).await,
            };
            (paired, if paired { "mesh" } else { source })
        }
    };
    let communication = SharedPolicy::new(Policy::new(enabled, source));
    load_topology(&store, &communication).await;
    // 179 D8: paused agents' trees are held from the start — before the
    // reminders re-arm and spared results go — and what was sent to them
    // stays `accepted` for the un-pause (the restart accounting spares it).
    let boot_paused = pause::boot_accounting(&store, &rt.data_dir).await;
    // 199: this engine's file service, after the restart accounting (its first pass fails
    // the files of the exchanges that failed there); it registers the offers the a2a file
    // route serves through.
    #[cfg(unix)]
    crate::communication::files::service::FileService::new(
        store.clone(),
        rt.data_dir.clone(),
        Some(rt.clone()),
        Arc::new(crate::communication::files::notices::FrontDoor),
    )
    .start(rt.shutdown.clone());
    // 125 §4.14: the Advisors actor loads every link before `resume_scan`
    // (which the activity registration triggers later).
    let advisor_store = store.clone();
    let vault = auth.vault.clone();
    let redact: Arc<dyn Fn(&str) -> String + Send + Sync> = Arc::new(move |text: &str| {
        vault
            .as_deref()
            .map_or_else(|| text.to_owned(), |v| v.redact(text).into_owned())
    });
    // 139 D15 (§5.3): finished-turn journal retention runs where the rows
    // are owned — hourly, `Scope::Here`; the deletes replicate as log rows.
    // ponytail: no config key for the interval (sessionsync's orphan sweep shape).
    {
        let store = store.clone();
        let keep_days = cfg.durability.journal_keep_days;
        let shutdown = rt.shutdown.clone();
        tokio::spawn(async move {
            loop {
                match store.journal_prune(store::Scope::Here, keep_days).await {
                    Ok(n) if n > 0 => {
                        tracing::info!(
                            "109: pruned {n} finished turn(s) older than {keep_days} days"
                        )
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("109: journal prune: {e:#}"),
                }
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(3600)) => {}
                }
            }
        });
    }
    let client = client(auth.clone())?;
    // 137G R1-1: the bytes/token the preflight learned before this restart.
    crate::tokens::init(&rt.data_dir);
    // 137: one endless service per engine; its pumps restart from the rows
    // once the actor is up (`resume_all` below).
    let endless = Endless::new(
        store.clone(),
        rt.clone(),
        client.clone(),
        auth.clone(),
        catalog.clone(),
        cfg.endless.clone(),
        // R1-D3: the compactor chain's middle link (`agent.endless.model` → title model → session).
        cfg.title_model.clone(),
    );
    // 160: the cost cube builds in the background; RPCs answer from memory
    // through `CostService::engine()`, never through this mailbox.
    let costs = CostService::new(
        store.clone(),
        catalog.clone(),
        platform::process::hostname().to_owned(),
    );
    {
        let rt2 = rt.clone();
        costs.spawn(rt.shutdown.clone(), move |ev| {
            let rt = rt2.clone();
            async move { rt.publish(ev).await }
        });
        costs.install();
    }
    let cell_jobs = Arc::new(CellJobs::default());
    let idle_stop = Duration::from_secs(cfg.acp.idle_stop_secs);
    let agents = Agents::spawn_with_mailbox(
        Agents {
            rt: rt.clone(),
            cfg: cfg.clone(),
            default_model: cfg.model.clone(),
            default_effort: cfg.effort.clone(),
            favorite_models: cfg.favorite_models.clone(),
            compact_style: Arc::new(RwLock::new(cfg.compact_style)),
            title_model: Arc::new(RwLock::new(title_model)),
            polish_model: Arc::new(RwLock::new(polish_model)),
            // 204: `voice.polish_enabled`, default on; unreadable section → on.
            polish_enabled: rt
                .section::<voice::VoiceConfig>("voice")
                .map_or(true, |v| v.polish_enabled),
            subagent_model: Arc::new(RwLock::new(subagent_model)),
            subagent_effort: Arc::new(RwLock::new(cfg.subagents.effort.clone())),
            desk_models: HashMap::new(),
            desk_efforts: HashMap::new(),
            vision_model: Arc::new(RwLock::new(vision_model)),
            escalate_model: Arc::new(RwLock::new(escalate_model)),
            knowledge_harvester_model: Arc::new(RwLock::new(knowledge_harvester_model)),
            role_efforts: Arc::new(parking_lot::RwLock::new(role_efforts)),
            vision_error,
            catalog,
            pool,
            configured,
            store,
            costs,
            client,
            auth,
            endless: endless.clone(),
            sessions: HashMap::new(),
            running: HashSet::new(),
            boot: None,
            last_boot: None,
            draining_sessions: HashMap::new(),
            pending_takeovers: HashMap::new(),
            paused: boot_paused
                .iter()
                .map(|(_, desk)| (*desk, Vec::new()))
                .collect(),
            pausing: HashMap::new(),
            acp_cut: HashSet::new(),
            communication: communication.clone(),
            network: network::NetworkState::default(),
            waits: HashMap::new(),
            bound: HashMap::new(),
            turn_text: HashMap::new(),
            delivered: HashMap::new(),
            enqueued: HashSet::new(),
            forwarding: HashMap::new(),
            pools: HashMap::new(),
            child_turns: HashMap::new(),
            queued: HashMap::new(),
            undelivered: HashMap::new(),
            timed_out: HashSet::new(),
            arms: 0,
            child_errors: HashMap::new(),
            caps: HashMap::new(),
            requested_models: HashMap::new(),
            remote_parent: HashMap::new(),
            owned_children: HashMap::new(),
            last_stop: HashMap::new(),
            last_input: HashMap::new(),
            seen: HashMap::new(),
            talk: HashMap::new(),
            cell_jobs: cell_jobs.clone(),
            approvals: HashMap::new(),
            parked: HashMap::new(),
            parking: HashMap::new(),
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
            inbound_chain: None,
            identity: None,
            contacts_ready: crate::communication::network::CONTACTS_READY.clone(),
            advisors: HashSet::new(),
            advisor_cfg: cfg.advisor.clone(),
            activity: crate::activity::Monitor::default(),
        },
        runtime::subscriber_mailbox(),
    );
    agents.register(AGENT_ACTOR)?;
    notice_authority(rt, &agents);
    // 176 A1: idle ACP children are parked, checked once a minute.
    if !idle_stop.is_zero() {
        let agents = agents.clone();
        let shutdown = rt.shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(PARK_EVERY) => {}
                }
                if agents.tell(ParkIdle(idle_stop)).await.is_err() {
                    return;
                }
            }
        });
    }
    // 137 §8.7: pumps for every endless session with unbuilt nodes.
    match endless.resume_all().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(pumps = n, "endless: resumed"),
        Err(e) => tracing::warn!("endless resume_all: {e:#}"),
    }
    crate::advisor::Advisors::start(
        advisor_store,
        rt.clone(),
        cfg.advisor.clone(),
        cfg.compact_at,
        redact,
    )
    .await?;
    UserChannels::spawn(UserChannels::default()).register(USER_ACTOR)?;
    rt.subscribe::<Agents, AgentEvent>(&agents).await;
    // 183 §6.1: the absent-`agent.communication` default follows the paired set.
    rt.subscribe::<Agents, mesh::MeshEvent>(&agents).await;
    let watch =
        CellJobWatch::spawn_with_mailbox(CellJobWatch(cell_jobs), runtime::subscriber_mailbox());
    rt.subscribe::<CellJobWatch, JobEvent>(&watch).await;
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
                // 109: journal seam — installed before anything can be resumed.
                if let Ok(store) = agents.ask(DurableStore).await {
                    let _ = reg
                        .ask(tools::InstallJournal(Arc::new(
                            crate::durability::journal::StoreJournal(Arc::new(store)),
                        )))
                        .await;
                }
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
                     secrets.vault (encrypted; provider keys and every other secret — `secret_list()` shows the names, never values), \
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
        tokio::spawn(replica::files::mirror_at(
            data_dir.join("skills.sqlite"),
            dir.clone(),
            "skills",
            std::time::Duration::from_secs(120),
            None,
        ));
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
        // 155 §5.2: the state wait, beside the directory and the record.
        let _ = reg.ask(RegisterTool(Arc::new(WaitPeerTool))).await;
        let _ = reg.ask(RegisterTool(Arc::new(MessageUserTool))).await;
        let _ = reg.ask(RegisterTool(Arc::new(ListModelsTool))).await;
        let _ = reg
            .ask(RegisterTool(Arc::new(crate::session_read::SessionReadTool)))
            .await;
        // 144 §13.2 (E2): ask across notebooks and the overviews.
        let _ = reg
            .ask(RegisterTool(Arc::new(crate::ask::NotebookAskTool)))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(
                crate::overviews::NotebookOverviewRunTool,
            )))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(
                crate::overviews::NotebookOverviewsTool,
            )))
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
    // 125 §4.2: advisor children are absent from `list_sessions`; the policy
    // learns them as hidden so no route ever reaches one.
    let hidden: Vec<SessionId> = match store.advisor_links().await {
        Ok(links) => links.into_iter().map(|l| l.advisor).collect(),
        Err(e) => {
            tracing::warn!("advisor links not loaded: {e:#}");
            Vec::new()
        }
    };
    let mut p = communication.0.write();
    for id in hidden {
        p.record_hidden(id);
    }
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

    async fn handle(
        &mut self,
        _: ListProjectStyles,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .list_project_styles()
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<SetProjectStyle> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetProjectStyle,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
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
/// How long a routed input id stays a recognised retry (`Agents::seen`).
const SEEN_TTL: Duration = Duration::from_secs(600);

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
    pub(crate) fn store_for(&self, id: SessionId) -> &Store {
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
            self.drop_queued(id).await;
            self.ledger.forget(id);
            self.running.remove(&id);
            self.approvals.remove(&id);
            self.last_stop.remove(&id);
            self.last_input.remove(&id);
            self.owned_children.remove(&id);
            self.caps.remove(&id);
            self.requested_models.remove(&id);
            self.remote_parent.remove(&id);
            self.bound.retain(|(s, _), _| *s != id);
            self.delivered.retain(|(s, _), _| *s != id);
            self.turn_text.retain(|(s, _), _| *s != id);
            if let Some(timer) = self.child_turns.remove(&id).and_then(|ct| ct.timer) {
                timer.abort();
            }
            if let Some(live) = self.sessions.remove(&id) {
                live.actor.tell_cancel(None).await;
                live.actor.kill().await;
            }
            self.endless.detach(id);
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

    async fn handle(
        &mut self,
        m: CreateSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let project = project::normalize_project(m.options.project.as_deref())?;
        if let Some((model, effort)) = &m.options.advisor {
            if m.options.retention != Retention::Durable || m.options.harness.is_some() {
                return Err("an advisor needs a durable native chat".into());
            }
            let resolved = self.catalog.resolve(model, &*self.configured)?;
            if let (Some(model), Some(effort)) = (resolved.model, effort) {
                model.validate_effort(effort)?;
            }
        }
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
                    Some((harness.clone(), cwd, None, Vec::new())),
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
        // 137 D1: the memory mode — explicit, else `agent.endless.default_chat`.
        let memory = m
            .options
            .memory
            .unwrap_or(if self.endless.config().default_chat {
                MemoryMode::Endless
            } else {
                MemoryMode::Epochs
            });
        if m.options.retention == Retention::Durable {
            let id = self
                .create_durable(
                    m.options.cwd.as_deref().map(Path::new),
                    project.as_deref(),
                    model.as_deref(),
                    effort,
                    m.options.cap,
                    m.options
                        .parent_session
                        .clone()
                        .map(|session| RemoteParent {
                            session,
                            engine: m.options.parent_engine.clone(),
                        }),
                )
                .await?;
            // 144 §8: the notebook binding is root metadata like `project`.
            if let Some(nb) = m.options.notebook.as_deref().filter(|n| !n.is_empty()) {
                self.store
                    .set_notebook(id, Some(nb))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            // The actor is attached by now: `set_mode` sets the column and
            // starts the (empty) tree, so the first turn is already endless.
            if memory == MemoryMode::Endless {
                self.endless
                    .set_mode(id, memory, 0, UnitFilter::All)
                    .await
                    .map_err(|e| format!("fractal context: {e:#}"))?;
                tracing::info!(session = %id, "endless: created in endless mode");
            }
            if let Some((model, effort)) = m.options.advisor {
                self.advisor_on(id, model, effort)
                    .await
                    .map_err(|e| format!("advisor: {e}"))?;
            }
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
            match reg.ask(ParticipantsReady).await {
                Ok(()) => {}
                Err(SendError::HandlerError(missing)) => {
                    return Err(format!("private mode unavailable: {missing}"));
                }
                // Test-only race guard: `registry()` found ANOTHER test's live
                // registry (one global name per test binary) and it stopped
                // before this ask — the same "no registry" rule `registry()`
                // applies under cfg(test) when none is found.
                Err(_) if cfg!(test) => {}
                Err(e) => return Err(format!("private mode unavailable: {e}")),
            }
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
        // Before the actor attaches (below): it reads the mode from this store.
        if memory == MemoryMode::Endless {
            store
                .set_session_memory(root, memory)
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

    async fn handle(
        &mut self,
        m: ResumePrivate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let root = *self
            .private_member
            .get(&m.session)
            .ok_or("no such private session")?;
        let p = self
            .private
            .get_mut(&root)
            .ok_or("no such private session")?;
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

    async fn handle(
        &mut self,
        m: LeaveSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
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
        let still_held = self.private.get(&m.root).is_some_and(|p| {
            !p.closing && p.owner.peer == m.peer && p.owner.generation == m.generation
        });
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
            self.rt
                .publish(AgentEvent::SessionDeleted { session })
                .await;
        }
    }
}

impl Message<SessionCostQuery> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<Option<crate::cost::SessionCost>, String>>;

    /// 176 E1: read off the mailbox.
    async fn handle(
        &mut self,
        m: SessionCostQuery,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let store = self.store_for(m.0).clone();
        ctx.spawn(async move { store.session_cost(m.0).await.map_err(|e| format!("{e:#}")) })
    }
}

impl Message<SpendReportQuery> for Agents {
    type Reply = kameo::reply::DelegatedReply<Result<crate::cost::SpendReport, String>>;

    /// 176 E1: read off the mailbox.
    async fn handle(
        &mut self,
        m: SpendReportQuery,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // The durable store only: private trees (51 §4.5) never reach the HUD's ledger.
        // 160 H5: plus the cube's placed reported money.
        let (costs, now) = (
            self.costs.clone(),
            Utc::now().timestamp_millis().max(0) as u64,
        );
        ctx.spawn(async move {
            costs
                .spend_report(now, m.0)
                .await
                .map_err(|e| format!("{e:#}"))
        })
    }
}

#[cfg(test)]
impl Agents {
    /// The credential bundle sessions stream with (tests point a gateway at a fake provider).
    pub(crate) fn test_auth(&self) -> &Auth {
        &self.auth
    }

    /// Which providers count as keyed (194 R1: the advisor rig's default-model setter).
    pub(crate) fn set_test_keyed(
        &mut self,
        configured: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) {
        self.configured = Arc::new(configured);
    }
}

/// An `Agents` value (no actor, no tool registry) over `rt.data_dir`: the
/// store at `sessions.sqlite`, the catalog from `models.json`, one native and
/// one external child slot per tree. Session actors it spawns are real; with
/// no registry their turns end as `error` before any model is called. The
/// in-crate tests' rig, shared with other crates' tests under `test-support`.
#[cfg(any(test, feature = "test-support"))]
pub(crate) async fn bare_agents(rt: Runtime, engine: Option<&str>, communication: bool) -> Agents {
    let dir = rt.data_dir.clone();
    std::fs::create_dir_all(&dir).unwrap();
    // One native and one external child slot per tree: a second spawn queues.
    let mut cfg = AgentConfig::default();
    cfg.subagents.max_concurrent = 1;
    cfg.subagents.max_concurrent_external = 1;
    let cfg = Arc::new(cfg);
    let engine = engine.map(|e| mesh::EngineId(e.to_owned()));
    let store = Store::open(&dir.join("sessions.sqlite"), engine)
        .await
        .unwrap();
    let client = Client::new().unwrap();
    let auth = Auth::stub(&dir);
    let catalog = Arc::new(Catalog::load(&dir));
    let endless = Endless::new(
        store.clone(),
        rt.clone(),
        client.clone(),
        auth.clone(),
        catalog.clone(),
        cfg.endless.clone(),
        cfg.title_model.clone(),
    );
    // 179 D8 / 182 M2a, as `start` builds it: held from the agent files, and
    // desk trees with an open turn while the mesh has peers.
    let mut boot_paused = pause::paused_desks(&store, &dir).await;
    let sync = pause::sync_held(&store, &dir, &boot_paused).await;
    boot_paused.extend(sync);
    Agents {
        rt,
        default_model: cfg.model.clone(),
        default_effort: None,
        favorite_models: Vec::new(),
        cfg,
        compact_style: Arc::default(),
        title_model: Arc::default(),
        polish_model: Arc::default(),
        polish_enabled: true,
        subagent_model: Arc::default(),
        subagent_effort: Arc::default(),
        vision_model: Arc::default(),
        escalate_model: Arc::default(),
        knowledge_harvester_model: Arc::default(),
        role_efforts: Arc::default(),
        vision_error: None,
        catalog: catalog.clone(),
        pool: None,
        configured: Arc::new(|_: &str| false),
        costs: CostService::new(store.clone(), catalog.clone(), "test".to_owned()),
        store,
        client,
        auth,
        endless,
        sessions: HashMap::new(),
        desk_models: HashMap::new(),
        desk_efforts: HashMap::new(),
        running: HashSet::new(),
        boot: None,
        last_boot: None,
        draining_sessions: HashMap::new(),
        pending_takeovers: HashMap::new(),
        paused: boot_paused
            .into_iter()
            .map(|(_, desk)| (desk, Vec::new()))
            .collect(),
        pausing: HashMap::new(),
        acp_cut: HashSet::new(),
        communication: SharedPolicy::new(Policy::new(communication, "config")),
        network: network::NetworkState::default(),
        waits: HashMap::new(),
        bound: HashMap::new(),
        turn_text: HashMap::new(),
        delivered: HashMap::new(),
        enqueued: HashSet::new(),
        forwarding: HashMap::new(),
        pools: HashMap::new(),
        child_turns: HashMap::new(),
        queued: HashMap::new(),
        undelivered: HashMap::new(),
        timed_out: HashSet::new(),
        arms: 0,
        child_errors: HashMap::new(),
        caps: HashMap::new(),
        requested_models: HashMap::new(),
        remote_parent: HashMap::new(),
        owned_children: HashMap::new(),
        last_stop: HashMap::new(),
        last_input: HashMap::new(),
        seen: HashMap::new(),
        talk: HashMap::new(),
        cell_jobs: Arc::default(),
        approvals: HashMap::new(),
        parked: HashMap::new(),
        parking: HashMap::new(),
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
        inbound_chain: None,
        identity: None,
        // A test rig is past the latch unless it says otherwise (its own flag, never the
        // process-wide one).
        contacts_ready: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        advisors: HashSet::new(),
        advisor_cfg: AdvisorConfig::default(),
        activity: crate::activity::Monitor::default(),
    }
}

/// SEC-200-1: `rt`'s engine judges a contact's file notice at consumption through this
/// supervisor (`notices::consumption_live`); every spawn of an engine's [`Agents`] does this.
pub(crate) fn notice_authority(rt: &Runtime, agents: &ActorRef<Agents>) {
    crate::communication::files::notices::set_authority(
        rt,
        Arc::new(crate::communication::files::notices::Supervisor(
            agents.downgrade(),
        )),
    );
}

/// 148 R1 (`test-support`): [`bare_agents`] with `configured` deciding which
/// providers count as keyed, spawned and registered as [`AGENT_ACTOR`] so a
/// dependent crate's tests (the roster's desks) can talk to a live engine
/// that never calls a model. `Err` when an actor already holds the name.
#[cfg(feature = "test-support")]
pub async fn spawn_test_agents(
    rt: Runtime,
    configured: impl Fn(&str) -> bool + Send + Sync + 'static,
) -> Result<ActorRef<Agents>, String> {
    let mut a = bare_agents(rt.clone(), None, false).await;
    a.configured = Arc::new(configured);
    let agents = Agents::spawn(a);
    notice_authority(&rt, &agents);
    agents.register(AGENT_ACTOR).map_err(|e| e.to_string())?;
    Ok(agents)
}

/// 183 (`test-support`): one engine's live [`Agents`] with agent communication on and no
/// keyed provider, spawned but not registered, so a test can run two engines in one process
/// (the a2a contact tests). 191: `ready` is that engine's own `contacts_ready` (its a2a
/// context holds the same one), never the process-wide flag another engine sets. 199:
/// `engine` is its store's engine id (`sessions.owner`, a file offer's holder), its iroh
/// endpoint as in a binary; `None` is `""`.
#[cfg(feature = "test-support")]
pub async fn spawn_engine_agents(
    rt: Runtime,
    ready: Arc<std::sync::atomic::AtomicBool>,
    engine: Option<&str>,
) -> ActorRef<Agents> {
    let mut a = bare_agents(rt.clone(), engine, true).await;
    a.configured = Arc::new(|_| false);
    a.contacts_ready = ready;
    let agents = Agents::spawn(a);
    notice_authority(&rt, &agents);
    agents
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::CreateSessionOptions;
    use crate::ObservedState;
    use crate::context::ContextAttachment;

    /// The picker lists Gemini once: a gateway on Google's Gemini API host
    /// hides the built-in `google` block, unless the gateway is `google` itself.
    #[test]
    fn google_gateway_hides_builtin_google_once() {
        let gw = |url: &str| crate::providers::Gateway {
            base_url: url.into(),
            ..Default::default()
        };
        let google = "https://generativelanguage.googleapis.com/v1beta/openai/";
        let set = |rows: &[(&str, &str)]| -> HashMap<_, _> {
            rows.iter().map(|(n, u)| (n.to_string(), gw(u))).collect()
        };
        assert!(google_shadowed(&set(&[
            ("gemini", google),
            ("rwx", "https://api.ai.rwx.dev/v1/")
        ])));
        assert!(!google_shadowed(&set(&[(
            "rwx",
            "https://api.ai.rwx.dev/v1/"
        )])));
        assert!(!google_shadowed(&set(&[("google", google)])));
        // Vertex is not the Gemini API's `google/*` catalog.
        assert!(!google_shadowed(&set(&[(
            "vx",
            "https://us-central1-aiplatform.googleapis.com/v1/"
        )])));
    }

    #[tokio::test]
    async fn bedrock_profile_changes_reach_existing_clients_without_vault_keys() {
        let auth = Auth {
            vault: None,
            oauth: oauth::Store::new(Path::new("/unused")),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            aws_profile: Arc::new(parking_lot::RwLock::new(Some("work".into()))),
            openrouter: Arc::default(),
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

    /// 85 §2: `opencode` is configured by its vault key; OpenRouter by either
    /// vault name, and the genai resolver hands the `openrouter` key to the
    /// `open_router` adapter. Env fallbacks are not exercised (process-global).
    #[tokio::test]
    async fn builtin_keys_configure_opencode_and_openrouter_by_either_name() {
        let scratch = crate::test_dir("builtin");
        let dir = scratch.path().to_path_buf();
        let vault = Arc::new(
            secrets::Vault::open(&dir, secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let auth = Auth {
            vault: Some(vault.clone()),
            ..Auth::stub(&dir)
        };
        let is = configured(auth.clone());
        let env_zen = std::env::var_os("OPENCODE_API_KEY").is_some();
        let env_or = ["OPENROUTER_API_KEY", "OPEN_ROUTER_API_KEY"]
            .iter()
            .any(|k| std::env::var_os(k).is_some());
        assert_eq!(is("opencode"), env_zen);
        assert_eq!(is("open_router"), env_or);

        vault.set("opencode", "zk", None, "test").await.unwrap();
        assert!(is("opencode") && auth.opencode_key().as_deref() == Some("zk"));

        vault
            .set("open_router", "legacy", None, "test")
            .await
            .unwrap();
        assert!(is("open_router") && auth.openrouter_key().as_deref() == Some("legacy"));
        vault.set("openrouter", "ork", None, "test").await.unwrap();
        assert_eq!(auth.openrouter_key().as_deref(), Some("ork"));

        // OpenCode Go: its Settings name first, the genai-namespace name still read.
        let env_go = std::env::var_os("OPENCODE_GO_API_KEY").is_some();
        assert_eq!(is("opencode_go"), env_go);
        vault.set("opencode_go", "old", None, "test").await.unwrap();
        assert!(is("opencode_go") && auth.opencode_go_key().as_deref() == Some("old"));
        vault.set("opencode-go", "gk", None, "test").await.unwrap();
        assert_eq!(auth.opencode_go_key().as_deref(), Some("gk"));

        let client = client(auth).unwrap();
        let target = client
            .resolve_service_target("open_router::anthropic/claude-opus-5.5")
            .await
            .unwrap();
        assert_eq!(target.model.adapter_kind, AdapterKind::OpenRouter);
        assert_eq!(target.auth.single_key_value().unwrap(), "ork");
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
        let got = render(&messages, &BTreeMap::new(), &BTreeMap::new());
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
                at: None,
                fences: Vec::new(),
                sources: Vec::new(),
            },
        );
        let turns: Vec<Option<i64>> = render(&messages, &entries, &BTreeMap::new())
            .into_iter()
            .filter_map(|(_, e)| match e {
                HistoryEntry::User { turn, .. } => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns, [Some(0), None, Some(3)]);
        // Untyped: only prose, so the quoted note text is an ordinary turn.
        let turns: Vec<Option<i64>> = render(&messages, &BTreeMap::new(), &BTreeMap::new())
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
        let got = render(&messages, &BTreeMap::new(), &BTreeMap::new());
        assert!(matches!(
            &got[0],
            (1, HistoryEntry::Tool { result: Some((out, false)), files, .. })
                if out == "saved\n[image: shot-1.png]" && files == &[PathBuf::from("/data/browser/s/shot-1.png")]
        ));
    }

    /// 197 §5.1: each show/app/page_preview card carries its own call's
    /// source piece, whether the call sits in a calls-only row or beside text;
    /// the reply keeps its fences and no sources; a pending card has no context.
    #[test]
    fn render_puts_each_call_source_on_its_card() {
        use genai::chat::{ContentPart, MessageContent, ToolCall, ToolResponse};
        let call = |id: &str, name: &str, args: serde_json::Value| {
            ContentPart::ToolCall(ToolCall {
                thought_signatures: None,
                call_id: id.into(),
                fn_name: name.into(),
                fn_arguments: args,
            })
        };
        let assistant = |parts| ChatMessage::assistant(MessageContent::from_parts(parts));
        let messages = vec![
            (0, ChatMessage::user("go")),
            (
                1,
                assistant(vec![call(
                    "s1",
                    "show",
                    serde_json::json!({"name": "mock.html", "content": "<p>x</p>"}),
                )]),
            ),
            (2, ChatMessage::from(vec![ToolResponse::new("s1", "shown")])),
            (
                3,
                assistant(vec![
                    ContentPart::Text("Here:\n```mermaid\ngraph TD\n```\n".into()),
                    call(
                        "a1",
                        "app",
                        serde_json::json!({"name": "counter", "source": "root = Stack([])"}),
                    ),
                    call("x1", "shell", serde_json::json!({"command": "ls"})),
                ]),
            ),
            (
                4,
                ChatMessage::from(vec![
                    ToolResponse::new("a1", "ok"),
                    ToolResponse::new("x1", "ok"),
                ]),
            ),
            (
                5,
                assistant(vec![call(
                    "s9",
                    "show",
                    serde_json::json!({"name": "b.md", "content": "b"}),
                )]),
            ),
        ];
        let entries: BTreeMap<i64, ContextEntry> = messages
            .iter()
            .map(|(seq, m)| {
                let owner = ContextOwner::Message { seq: *seq };
                (
                    *seq,
                    crate::context::entry(
                        owner,
                        m,
                        &crate::context::Mask::default(),
                        true,
                        Vec::new(),
                    ),
                )
            })
            .collect();
        let got = render(&messages, &entries, &BTreeMap::new());
        let card = |id: &str| {
            got.iter()
                .find_map(|(_, e)| match e {
                    HistoryEntry::Tool {
                        call_id, context, ..
                    } if call_id == id => Some(context.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let source = |seq, part| ContextTarget::Source {
            owner: ContextOwner::Message { seq },
            part,
        };
        let targets = |c: Option<ContextEntry>| -> Vec<ContextTarget> {
            c.unwrap().sources.iter().map(|p| p.target).collect()
        };
        assert_eq!(targets(card("s1")), [source(1, 0)], "calls-only row");
        assert_eq!(targets(card("a1")), [source(3, 1)], "text-plus-calls row");
        assert!(targets(card("x1")).is_empty(), "a shell card has no source");
        assert!(card("s9").is_none(), "a pending card keeps no context");
        let reply = got
            .iter()
            .find_map(|(_, e)| match e {
                HistoryEntry::Assistant { context, .. } => context.clone(),
                _ => None,
            })
            .unwrap();
        assert!(reply.sources.is_empty() && reply.fences.len() == 1);
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
        // 197: the calls-only row renders no entry; c2's source piece rides on its card.
        let piece = ContextPiece {
            target: ContextTarget::Source {
                owner: ContextOwner::Message { seq: 0 },
                part: 1,
            },
            label: "mock.html".into(),
            removed: true,
        };
        entries.insert(
            0,
            ContextEntry {
                target: ContextTarget::Message { seq: 0 },
                active: true,
                removed: false,
                remove_with: vec![ContextTarget::Message { seq: 1 }],
                attachments: Vec::new(),
                vision: None,
                at: None,
                fences: Vec::new(),
                sources: vec![piece.clone()],
            },
        );
        entries.insert(
            1,
            ContextEntry {
                target: ContextTarget::Message { seq: 1 },
                active: true,
                removed: false,
                remove_with: vec![ContextTarget::Message { seq: 0 }],
                attachments: vec![attachment(0, "a.png"), attachment(1, "b.png")],
                vision: None,
                at: None,
                fences: Vec::new(),
                sources: Vec::new(),
            },
        );
        let got = render(&messages, &entries, &BTreeMap::new());
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
            assert_eq!(c.sources, if i == 1 { vec![piece.clone()] } else { vec![] });
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
                at: None,
                fences: Vec::new(),
                sources: Vec::new(),
            })
        };
        let mut history = vec![
            HistoryEntry::User {
                channel: None,
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
        let scratch = crate::test_dir("preview");
        let dir = scratch.path().to_path_buf();
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
            channel: None,
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
            channel: None,
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
            channel: None,
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

    /// A fixed local wall clock for wrapper `at` attributes.
    fn at() -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone as _;
        chrono::Local
            .with_ymd_and_hms(2026, 9, 28, 14, 3, 0)
            .unwrap()
    }

    #[test]
    fn job_result_wrapper_escapes_and_names_exit() {
        let id = JobId("j3".into());
        let w = job_result(
            &id,
            "echo \"a\" <b>",
            &JobExit::Exited(0),
            7,
            "out",
            JobKind::Shell,
            at(),
        );
        assert_eq!(
            w,
            "<job_result id=\"j3\" command=\"echo &quot;a&quot; &lt;b&gt;\" exit=\"0\" secs=\"7\" at=\"14:03\">\nout\n</job_result>"
        );
        assert!(!w.contains("pty"));
        let w = job_result(
            &id,
            "python3 -q",
            &JobExit::Exited(0),
            41,
            ">>> ",
            JobKind::Pty,
            at(),
        );
        assert_eq!(
            w,
            "<job_result id=\"j3\" command=\"python3 -q\" exit=\"0\" secs=\"41\" pty=\"true\" at=\"14:03\">\n>>> \n</job_result>"
        );
        let w = job_result(
            &id,
            "sleep 9",
            &JobExit::Timeout,
            1800,
            "",
            JobKind::Shell,
            at(),
        );
        assert!(w.contains(" exit=\"timeout\" "));
        let w = job_result(&id, "x", &JobExit::Killed, 1, "", JobKind::Shell, at());
        assert!(w.contains(" exit=\"killed\" "));
        let long = "a".repeat(200);
        let w = job_result(&id, &long, &JobExit::Exited(1), 0, "", JobKind::Shell, at());
        assert!(w.contains(&format!(" command=\"{}\" ", "a".repeat(120))));
        assert!(!w.contains(&"a".repeat(121)));
        // 145 T1: a promoted call names its tool, never a command.
        let w = job_result(
            &id,
            "search_web",
            &JobExit::Exited(0),
            12,
            "hits",
            JobKind::Promoted,
            at(),
        );
        assert_eq!(
            w,
            "<job_result id=\"j3\" tool=\"search_web\" exit=\"0\" secs=\"12\" at=\"14:03\">\nhits\n</job_result>"
        );
        assert!(!w.contains("command="));
    }

    #[test]
    fn reminder_wrapper_escapes_attributes_and_names_late() {
        let w = reminder(
            "r3",
            600,
            "2026-09-23T10:00:00+02:00",
            false,
            "check \"a\" <b>",
            at(),
        );
        assert_eq!(
            w,
            "<reminder id=\"r3\" after=\"600\" set_at=\"2026-09-23T10:00:00+02:00\" late=\"false\" at=\"14:03\">\ncheck \"a\" <b>\n</reminder>"
        );
        let w = reminder("r<1>", 5, "\"t\"", true, "", at());
        assert_eq!(
            w,
            "<reminder id=\"r&lt;1&gt;\" after=\"5\" set_at=\"&quot;t&quot;\" late=\"true\" at=\"14:03\">\n\n</reminder>"
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
            "",
            ">>> \ncursor: row 1 col 5",
            at(),
        );
        assert_eq!(
            w,
            "<job_idle id=\"j4\" command=\"echo &quot;a&quot; &lt;b&gt;\" quiet=\"63\" secs=\"912\" nudge=\"1\" at=\"14:03\">\n>>> \ncursor: row 1 col 5\n</job_idle>"
        );
        let w = job_idle(
            &id,
            "omp",
            Some("omp<1>"),
            120,
            30,
            2,
            Some(12),
            "no \"reaction\" to input",
            "",
            at(),
        );
        assert_eq!(
            w,
            "<job_idle id=\"j4\" command=\"omp\" name=\"omp&lt;1&gt;\" quiet=\"120\" secs=\"30\" nudge=\"2\" why=\"no &quot;reaction&quot; to input\" user_typed=\"12\" at=\"14:03\">\n\n</job_idle>"
        );
        let long = "a".repeat(200);
        let w = job_idle(&id, &long, None, 0, 0, 1, None, "", "", at());
        assert!(w.contains(&format!(" command=\"{}\" ", "a".repeat(120))));
        assert!(!w.contains(&"a".repeat(121)));
    }

    /// 155 §4.1: every attribute, `last` escaped and omitted when empty;
    /// `<peer_state>` likewise.
    #[test]
    fn child_stalled_and_peer_state_wrappers() {
        let child: SessionId = "01999999-0000-7000-8000-000000000001".parse().unwrap();
        let w = child_stalled(
            "w<2>",
            child,
            crate::activity::StallPhase::Silent,
            184,
            "shell: echo \"a\" <b>",
            "w2 has sent no session update for 184 s.",
            at(),
        );
        assert_eq!(
            w,
            format!(
                "<child_stalled child=\"w&lt;2&gt;\" session=\"{child}\" phase=\"silent\" quiet_secs=\"184\" last=\"shell: echo &quot;a&quot; &lt;b&gt;\" at=\"14:03\">\nw2 has sent no session update for 184 s.\n</child_stalled>"
            )
        );
        let w = child_stalled(
            "w1",
            child,
            crate::activity::StallPhase::NoFirstUpdate,
            60,
            "",
            "body",
            at(),
        );
        assert!(w.starts_with(&format!(
            "<child_stalled child=\"w1\" session=\"{child}\" phase=\"no_first_update\" quiet_secs=\"60\" at=\"14:03\">\n"
        )));
        assert!(!w.contains("last="));
        let w = peer_state(
            "w1",
            child,
            crate::activity::PeerStateWord::NotRunning,
            3,
            "w1 is gone.",
            at(),
        );
        assert_eq!(
            w,
            format!(
                "<peer_state child=\"w1\" session=\"{child}\" state=\"not_running\" since_secs=\"3\" at=\"14:03\">\nw1 is gone.\n</peer_state>"
            )
        );
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
        let scratch = crate::test_dir("attach");
        let dir = scratch.path().to_path_buf();
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
    }

    /// An `Agents` value (no actor, no tool registry) over a temp store: the
    /// supervisor's own sequencing runs inline and its ledger is readable.
    /// Session actors it spawns are real; with no registry their turns end
    /// as `error` before any model is called.
    async fn agents() -> (crate::TestDir, Agents) {
        agents_with(false).await
    }

    /// [`agents`] with `agent.communication` preset. Shared with the network
    /// module's tests, which drive the same value through its hooks.
    pub(crate) async fn agents_with(communication: bool) -> (crate::TestDir, Agents) {
        let scratch = crate::test_dir("agents");
        let a = agents_on(scratch.path().to_path_buf(), None, communication).await;
        (scratch, a)
    }

    /// [`agents_with`] over `dir`'s store opened as mesh engine `engine`
    /// (69 P: two of these, each holding the other's rows, are two engines).
    pub(crate) async fn agents_on(
        dir: PathBuf,
        engine: Option<&str>,
        communication: bool,
    ) -> Agents {
        std::fs::create_dir_all(&dir).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir);
        bare_agents(rt, engine, communication).await
    }

    /// 0065: a contact's message consumed on A and replicated to B. B holds no
    /// exchange record, yet its `load_session` and `session_read` mark the row
    /// with A's typed provenance (contact_mode full).
    #[tokio::test]
    async fn a_replica_marks_a_consumed_contact_row_with_the_owner_provenance() {
        use crate::communication::ShareMode;
        use crate::store::communication::AcceptExchange;
        use std::ffi::{c_int, c_void};
        unsafe extern "C" fn replace(
            _: *mut c_void,
            _: c_int,
            _: *mut libsqlite3_sys::sqlite3_changeset_iter,
        ) -> c_int {
            libsqlite3_sys::SQLITE_CHANGESET_REPLACE
        }
        let scratch = crate::test_dir("0065");
        let a = Store::open(
            &scratch.path().join("a.sqlite"),
            Some(mesh::EngineId("A".into())),
        )
        .await
        .unwrap();
        let chat = SessionId::new();
        a.create_session(chat, "m", None, None, None).await.unwrap();
        let to = format!("session:{chat}");
        let receipt = a
            .accept_exchange(&AcceptExchange {
                sender: "peer:sky/ops",
                recipient: &to,
                sender_session: None,
                recipient_session: Some(chat),
                request_id: Some("r1"),
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "**hello**",
            })
            .await
            .unwrap();
        a.set_exchange_contact(receipt.id, ShareMode::Full, None)
            .await
            .unwrap();
        a.consume_exchange(receipt.id, chat, 1, ChatMessage::user("**hello**"))
            .await
            .unwrap()
            .unwrap();
        let want = a
            .peer_record(chat, receipt.id)
            .await
            .unwrap()
            .unwrap()
            .origin();
        assert_eq!(want.contact_mode, Some(ShareMode::Full));

        let b = agents_on(scratch.path().join("b"), Some("B"), false).await;
        for row in a.log_after(0).await.unwrap() {
            let mut tx = b.store.begin_apply().await.unwrap();
            let rc = tx
                .with_raw(|db| unsafe {
                    libsqlite3_sys::sqlite3changeset_apply(
                        db.as_ptr(),
                        c_int::try_from(row.changeset.len()).unwrap(),
                        row.changeset.as_ptr().cast::<c_void>().cast_mut(),
                        None,
                        Some(replace),
                        std::ptr::null_mut(),
                    )
                })
                .await
                .unwrap();
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK, "apply of seq {}", row.seq);
            tx.commit().await.unwrap();
        }
        assert!(b.store.consumed_exchanges(chat).await.unwrap().is_empty());
        let agents = Agents::spawn(b);
        let peers = |h: &[HistoryEntry]| -> Vec<PeerOrigin> {
            h.iter()
                .filter_map(|e| match e {
                    HistoryEntry::User { peer, .. } => peer.clone(),
                    _ => None,
                })
                .collect()
        };
        let loaded = agents
            .ask(LoadSession {
                session: chat,
                before: None,
                limit: None,
                settled_only: false,
            })
            .await
            .unwrap();
        assert_eq!(peers(&loaded.history), std::slice::from_ref(&want));
        let read = agents
            .ask(ReadSession {
                session: chat,
                caller: chat,
            })
            .await
            .unwrap();
        assert_eq!(peers(&read.history), [want]);
        a.close().await;
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

    /// 183 Z3-R2 (M4): a native child gets the stricter of its requested cap and the
    /// spawner's ceiling as its own. Z3-R3: an owner desk with a `read_only` ceiling still
    /// spawns an external worker with the cap it asked for (a contact session spawns nothing:
    /// `a_contact_session_spawns_nothing`).
    #[test]
    fn a_native_child_takes_the_spawners_ceiling_an_external_one_its_request() {
        use Risk::{Destructive, Mutating, ReadOnly};
        assert_eq!(child_cap(None, Some(ReadOnly), false), Some(ReadOnly));
        assert_eq!(
            child_cap(Some(Destructive), Some(ReadOnly), false),
            Some(ReadOnly)
        );
        assert_eq!(
            child_cap(Some(ReadOnly), Some(Mutating), false),
            Some(ReadOnly)
        );
        assert_eq!(child_cap(None, Some(ReadOnly), true), None);
        assert_eq!(
            child_cap(Some(Mutating), Some(ReadOnly), true),
            Some(Mutating)
        );
        assert_eq!(child_cap(None, None, true), None);
    }

    /// 52 §3.1: the fallback must be an exact, image-capable catalog entry —
    /// fuzzy siblings, pass-through ids, text-only entries, and ambiguous
    /// bare ids are rejected; a unique bare id canonicalises.
    #[test]
    fn vision_candidate_requires_exact_image_capable_entry() {
        let scratch = crate::test_dir("vision");
        let dir = scratch.path().to_path_buf();
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
    }

    /// 52 §3.1: the setter validates, writes `config.yaml`, then swaps the live
    /// value; an unconfigured provider or a failed write changes nothing, and
    /// `None` clears both. A startup diagnostic is cleared by a successful set.
    #[tokio::test]
    async fn set_vision_model_persists_before_swapping() {
        let (_scratch, mut a) = agents().await;
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
    }

    /// 57 §0.9: the setter refuses an id that is not a catalog entry (or is
    /// ambiguous) and changes nothing; a unique bare id canonicalises and is
    /// written before the live value swaps; the getter reports the effective
    /// model — the escalation model, else the title model, else `None` (the
    /// session's model) — and `None` clears the slot.
    #[tokio::test]
    async fn set_escalate_model_validates_and_reports_effective() {
        let (_scratch, mut a) = agents().await;
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

    /// 153 §9.2: `set_polish_model` validates against the catalog, writes
    /// `voice.polish_model` to config.yaml and swaps the live slot; `None` clears both.
    #[tokio::test]
    async fn set_polish_model_persists() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis" || ns == "othervis");
        let agents = Agents::spawn(a);

        // Validated like `title_model`: an ambiguous bare id is refused and nothing is written.
        let e = agents
            .ask(SetPolishModel {
                model: Some("eyes".into()),
            })
            .await
            .unwrap_err();
        assert!(e.to_string().contains("ambiguous"), "{e:?}");
        assert!(!dir.join("config.yaml").exists());
        assert!(agents.ask(GetPolishModel).await.unwrap().is_none());

        let info = agents
            .ask(SetPolishModel {
                model: Some("blind".into()),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.id, "testvis/blind");
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("voice:\n"), "{yaml}");
        assert!(yaml.contains("  polish_model: testvis/blind\n"), "{yaml}");
        assert_eq!(
            agents
                .ask(GetPolishModel)
                .await
                .unwrap()
                .map(|m| m.id)
                .as_deref(),
            Some("testvis/blind")
        );
        // Re-read as the `voice.` section the boot path deserialises.
        let all: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let v: voice::VoiceConfig = serde_yaml::from_value(all["voice"].clone()).unwrap();
        assert_eq!(v.polish_model.as_deref(), Some("testvis/blind"));

        assert!(
            agents
                .ask(SetPolishModel { model: None })
                .await
                .unwrap()
                .is_none()
        );
        assert!(agents.ask(GetPolishModel).await.unwrap().is_none());
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(!yaml.contains("testvis/blind"), "{yaml}");
    }

    /// 194: `testfx/think` (takes off|low|high) and `testfx/plain` (no
    /// effort) as `a`'s catalog, `testfx` configured.
    fn role_effort_models(a: &mut Agents) {
        let dir = a.rt.data_dir.clone();
        let model = |id: &str, efforts: Option<&[&str]>| {
            let mut m = serde_json::json!({
                "id": id, "name": id, "image": true,
                "limit": { "context": 100_000, "output": 4_096 }
            });
            if let Some(v) = efforts {
                m["reasoning"] = true.into();
                m["reasoning_options"] = serde_json::json!([{"type": "effort", "values": v}]);
            }
            m
        };
        let user = serde_json::json!({ "testfx": { "id": "testfx", "name": "FX", "models": {
            "think": model("think", Some(&["low", "high"])),
            "plain": model("plain", None),
        }}});
        std::fs::write(dir.join("models.json"), user.to_string()).unwrap();
        a.catalog = Arc::new(Catalog::load(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testfx");
    }

    /// 194 rows 7–10, per setting: the effort persists beside its model key
    /// (D4g) and swaps the slot every session reads per call (D4f); a value the
    /// setting's model does not take is refused with `validate_effort`'s message
    /// (D4b), an unknown word when no model resolves; a model change that does
    /// not take the stored effort drops it (D4c); the `none` row clears it (D4d).
    #[tokio::test]
    async fn role_efforts_follow_their_models() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        role_effort_models(&mut a);
        let live = a.role_efforts.clone();
        let agents = Agents::spawn(a);
        let stored = |section: &str, key: &str| {
            let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
            let all: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            all[section][key].as_str().map(str::to_owned)
        };
        macro_rules! role {
            ($set_model:ident, $set:ident, $get:ident, $section:literal, $key:literal, $field:ident) => {{
                let set_model = |m: Option<&str>| $set_model {
                    model: m.map(str::to_owned),
                };
                let set = |e: Option<&str>| crate::$set {
                    effort: e.map(str::to_owned),
                };
                // No model resolves: only a known word is taken.
                let e = agents
                    .ask(set(Some("bogus")))
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(e.contains("unknown effort bogus"), "{}: {e}", $key);
                agents.ask(set_model(Some("testfx/think"))).await.unwrap();
                let e = agents.ask(set(Some("max"))).await.unwrap_err().to_string();
                assert!(e.contains("think accepts off|low|high"), "{}: {e}", $key);
                let got = agents.ask(set(Some("high"))).await.unwrap();
                assert_eq!(got.as_deref(), Some("high"), "{}", $key);
                assert_eq!(
                    agents.ask(crate::$get).await.unwrap().as_deref(),
                    Some("high")
                );
                assert_eq!(stored($section, $key).as_deref(), Some("high"), "{}", $key);
                assert_eq!(live.read().$field.as_deref(), Some("high"), "{}", $key);
                // A model that takes no effort drops it, persisted.
                agents.ask(set_model(Some("testfx/plain"))).await.unwrap();
                assert_eq!(agents.ask(crate::$get).await.unwrap(), None, "{}", $key);
                assert_eq!(stored($section, $key), None, "{}", $key);
                // The none row clears it too.
                agents.ask(set_model(Some("testfx/think"))).await.unwrap();
                agents.ask(set(Some("low"))).await.unwrap();
                agents.ask(set_model(None)).await.unwrap();
                assert_eq!(agents.ask(crate::$get).await.unwrap(), None, "{}", $key);
                assert_eq!(live.read().$field, None, "{}", $key);
                assert_eq!(stored($section, $key), None, "{}", $key);
            }};
        }
        role!(
            SetTitleModel,
            SetTitleEffort,
            GetTitleEffort,
            "agent",
            "title_effort",
            title
        );
        role!(
            SetVisionModel,
            SetVisionEffort,
            GetVisionEffort,
            "agent",
            "vision_effort",
            vision
        );
        role!(
            SetEscalateModel,
            SetEscalateEffort,
            GetEscalateEffort,
            "agent",
            "escalate_effort",
            escalate
        );
        role!(
            SetPolishModel,
            SetPolishEffort,
            GetPolishEffort,
            "voice",
            "polish_effort",
            polish
        );
        use crate::SetKnowledgeHarvesterModel;
        role!(
            SetKnowledgeHarvesterModel,
            SetKnowledgeHarvesterEffort,
            GetKnowledgeHarvesterEffort,
            "agent",
            "knowledge_harvester_effort",
            knowledge_harvester
        );
    }

    /// The knowledge harvester default: `agent.knowledge_harvester_model`
    /// persists and swaps the slot every session reads; a session's own pick
    /// is validated (catalog id, its effort), stored in the session store, read
    /// back, and reset to the default (dropping its effort) with `None`.
    #[tokio::test]
    async fn knowledge_harvester_default_and_session_pick() {
        use crate::{
            GetKnowledgeHarvesterModel, SessionKnowledgeHarvester, SessionKnowledgeHarvesterOf,
            SetKnowledgeHarvesterModel, SetSessionKnowledgeHarvester,
        };
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        role_effort_models(&mut a);
        let slot = a.knowledge_harvester_model.clone();
        let store = a.store.clone();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(Arc::clone(&seen)), runtime::subscriber_mailbox());
        a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let agents = Agents::spawn(a);

        // The default: unset, then set (bare id canonicalises), persisted and live.
        assert!(
            agents
                .ask(GetKnowledgeHarvesterModel)
                .await
                .unwrap()
                .is_none()
        );
        let info = agents
            .ask(SetKnowledgeHarvesterModel {
                model: Some("plain".into()),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.id, "testfx/plain");
        assert_eq!(
            slot.read().unwrap().as_ref().map(|r| r.id.as_str()),
            Some("testfx/plain")
        );
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(
            yaml.contains("  knowledge_harvester_model: testfx/plain\n"),
            "{yaml}"
        );

        // The session follows the default until it picks its own.
        let of = || SessionKnowledgeHarvesterOf { session: id };
        assert_eq!(
            agents.ask(of()).await.unwrap(),
            SessionKnowledgeHarvester::default()
        );
        let set = |m: Option<&str>, e: Option<&str>| SetSessionKnowledgeHarvester {
            session: id,
            model: m.map(str::to_owned),
            effort: e.map(str::to_owned),
        };
        let e = agents
            .ask(set(Some("testfx/think"), Some("max")))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("think accepts off|low|high"), "{e}");
        assert_eq!(
            agents.ask(of()).await.unwrap(),
            SessionKnowledgeHarvester::default()
        );

        let own = SessionKnowledgeHarvester {
            model: Some("testfx/think".into()),
            effort: Some("low".into()),
        };
        assert_eq!(
            agents.ask(set(Some("think"), Some("low"))).await.unwrap(),
            own
        );
        assert_eq!(agents.ask(of()).await.unwrap(), own);
        assert_eq!(store.session_knowledge_harvester(id).await.unwrap(), own);
        // The default changing leaves the session's own pick alone.
        agents
            .ask(SetKnowledgeHarvesterModel { model: None })
            .await
            .unwrap();
        assert_eq!(agents.ask(of()).await.unwrap(), own);

        // Reset: back to the default; the effort goes with the model.
        let reset = agents.ask(set(None, Some("low"))).await.unwrap();
        assert_eq!(reset, SessionKnowledgeHarvester::default());
        assert_eq!(
            store.session_knowledge_harvester(id).await.unwrap(),
            SessionKnowledgeHarvester::default()
        );
        // Each stored pick went out on the bus, in order; the refused one did not.
        // The bus is a `tell`; a round trip through the sink orders us after it.
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: id,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let changes: Vec<_> = seen
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::KnowledgeHarvesterChanged { session, harvester } if *session == id => {
                    Some(harvester.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(changes, [own.clone(), SessionKnowledgeHarvester::default()]);

        let e = agents
            .ask(SessionKnowledgeHarvesterOf {
                session: SessionId::new(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown session"), "{e}");
    }

    /// 194 R1: a role's model change persists every key before it publishes.
    /// With config.yaml unwritable, an incompatible model and the none row
    /// both reply `Err` and leave the live model, the live effort and the
    /// file as they were.
    #[tokio::test]
    async fn role_model_change_persists_before_it_publishes() {
        let (_scratch, mut a) = agents().await;
        let config = a.rt.data_dir.join("config.yaml");
        role_effort_models(&mut a);
        let live = a.role_efforts.clone();
        let slots = [
            a.title_model.clone(),
            a.vision_model.clone(),
            a.escalate_model.clone(),
            a.polish_model.clone(),
        ];
        let agents = Agents::spawn(a);
        let read_only = |on: bool| {
            let mut p = std::fs::metadata(&config).unwrap().permissions();
            p.set_readonly(on);
            std::fs::set_permissions(&config, p).unwrap();
        };
        macro_rules! role {
            ($set_model:ident, $set:ident, $slot:expr, $field:ident) => {{
                let set_model = |m: Option<&str>| $set_model {
                    model: m.map(str::to_owned),
                };
                agents.ask(set_model(Some("testfx/think"))).await.unwrap();
                agents
                    .ask(crate::$set {
                        effort: Some("high".into()),
                    })
                    .await
                    .unwrap();
                let before = std::fs::read_to_string(&config).unwrap();
                read_only(true);
                for to in [Some("testfx/plain"), None] {
                    let e = agents.ask(set_model(to)).await.unwrap_err().to_string();
                    assert!(e.contains("config.yaml: "), "{to:?}: {e}");
                    assert_eq!(live.read().$field.as_deref(), Some("high"), "{to:?}");
                    let model = $slot.read().unwrap().as_ref().map(|r| r.id.clone());
                    assert_eq!(model.as_deref(), Some("testfx/think"), "{to:?}");
                    assert_eq!(std::fs::read_to_string(&config).unwrap(), before, "{to:?}");
                }
                read_only(false);
            }};
        }
        role!(SetTitleModel, SetTitleEffort, slots[0], title);
        role!(SetVisionModel, SetVisionEffort, slots[1], vision);
        role!(SetEscalateModel, SetEscalateEffort, slots[2], escalate);
        role!(SetPolishModel, SetPolishEffort, slots[3], polish);
    }

    /// 194 D4e/D4f through the production handoff: an effort set after a
    /// session exists rides that session's next `SideCall` — supervisor →
    /// session actor → its `SideCall` handler → the wire — with no restart.
    #[tokio::test]
    async fn a_set_role_effort_reaches_an_existing_sessions_next_side_call() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let (_scratch, mut a) = agents().await;
        role_effort_models(&mut a);
        let server = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n\
                   data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                   data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;
        a.auth.gateways.write().unwrap().insert(
            "testfx".into(),
            crate::providers::Gateway {
                base_url: format!("{}/v1/", server.uri()),
                ..Default::default()
            },
        );
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let agents = Agents::spawn(a);
        agents
            .ask(SetTitleModel {
                model: Some("testfx/think".into()),
            })
            .await
            .unwrap();
        let call = || SideCall {
            session: root,
            role: crate::ModelRole::Title,
            system: "s".into(),
            user: "q".into(),
            budget: Duration::from_secs(5),
            kind: crate::cost::CallKind::Title,
        };
        agents.ask(call()).await.unwrap();
        agents
            .ask(crate::SetTitleEffort {
                effort: Some("high".into()),
            })
            .await
            .unwrap();
        agents.ask(call()).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body = |i: usize| -> serde_json::Value { reqs[i].body_json().unwrap() };
        assert_eq!(reqs.len(), 2);
        assert!(body(0).get("reasoning_effort").is_none(), "{}", body(0));
        assert_eq!(body(1)["reasoning_effort"], "high", "{}", body(1));
    }

    /// 194 R1: a Cursor row offers its sibling's efforts only where the wire
    /// takes a `reasoning` parameter (the OpenAI family); a Claude row — its
    /// thinking in the id — offers none, so the picker shows no chips for it.
    #[test]
    fn cursor_rows_offer_only_efforts_the_wire_takes() {
        let cat = Catalog::load(Path::new("/nonexistent"));
        let row = |id: &str| {
            cursor_model_info(
                &cat,
                cursor::Model {
                    id: id.into(),
                    name: id.into(),
                    max_mode: false,
                },
            )
        };
        let sibling = |id: &str| cat.closest(&format!("cursor/{id}")).unwrap().efforts();
        let gpt = row("gpt-5.4-mini-low");
        assert!(!gpt.efforts.is_empty(), "{gpt:?}");
        assert_eq!(gpt.efforts, sibling("gpt-5.4-mini-low"));
        let claude = row("claude-4.6-sonnet-medium-thinking");
        assert!(!sibling("claude-4.6-sonnet-medium-thinking").is_empty());
        assert!(claude.reasoning && claude.context > 0, "{claude:?}");
        assert!(
            claude.efforts.is_empty() && claude.budget_min.is_none(),
            "{claude:?}"
        );
    }

    /// 194: configs written before the role efforts load with them unset; the
    /// keys read back beside their models.
    #[test]
    fn role_effort_keys_load_old_and_new_configs() {
        let old: AgentConfig =
            serde_yaml::from_str("title_model: a/t\nvision_model: a/v\n").unwrap();
        assert_eq!(
            (old.title_effort, old.vision_effort, old.escalate_effort),
            (None, None, None)
        );
        let new: AgentConfig = serde_yaml::from_str(
            "title_effort: low\nvision_effort: \"8000\"\nescalate_effort: high\n",
        )
        .unwrap();
        assert_eq!(new.title_effort.as_deref(), Some("low"));
        assert_eq!(new.vision_effort.as_deref(), Some("8000"));
        assert_eq!(new.escalate_effort.as_deref(), Some("high"));
        let v: voice::VoiceConfig = serde_yaml::from_str("polish_model: a/p\n").unwrap();
        assert_eq!(v.polish_effort, None);
        let v: voice::VoiceConfig = serde_yaml::from_str("polish_effort: low\n").unwrap();
        assert_eq!(v.polish_effort.as_deref(), Some("low"));
    }

    /// 153 §9.1: an agent prompt is never polished — no model call, no session
    /// lookup, `refused: "agent prompt"`; a human destination with no session
    /// and no desk fails closed with `refused: "no session"`.
    #[tokio::test]
    async fn polish_refuses_agent_prompt() {
        let (_scratch, a) = agents().await;
        let agents = Agents::spawn(a);
        let r = agents
            .ask(Polish {
                text: "hello there".into(),
                destination: voice::Destination::AgentPrompt,
                session: None,
            })
            .await
            .unwrap();
        assert_eq!(
            r,
            PolishReply {
                refused: Some("agent prompt".into()),
                ..PolishReply::default()
            }
        );
        let r = agents
            .ask(Polish {
                text: "hello there".into(),
                destination: voice::Destination::Document,
                session: None,
            })
            .await
            .unwrap();
        assert_eq!(r.refused.as_deref(), Some("no session"));
        assert!(r.text.is_none());
    }

    /// 204: with `voice.polish_enabled: false` a human destination is refused `polish off`
    /// before any target lookup or model call (no `model`, `ms` 0); the switch persists to
    /// config.yaml and turning it back on restores the chain. Old configs default to on.
    #[tokio::test]
    async fn polish_switch_off_never_calls_a_model() {
        let v: voice::VoiceConfig = serde_yaml::from_str("polish_model: a/p\n").unwrap();
        assert!(v.polish_enabled);
        assert!(voice::VoiceConfig::default().polish_enabled);
        let (_scratch, a) = agents().await;
        let dir = a.rt.data_dir.clone();
        let agents = Agents::spawn(a);
        assert!(agents.ask(crate::GetPolishEnabled).await.unwrap());
        agents
            .ask(crate::SetPolishEnabled { enabled: false })
            .await
            .unwrap();
        assert!(!agents.ask(crate::GetPolishEnabled).await.unwrap());
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        let back: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            back["voice"]["polish_enabled"],
            serde_yaml::Value::Bool(false)
        );
        let polish = || Polish {
            text: "hello there".into(),
            destination: voice::Destination::Document,
            session: None,
        };
        let r = agents.ask(polish()).await.unwrap();
        assert_eq!(
            r,
            PolishReply {
                refused: Some("polish off".into()),
                ..PolishReply::default()
            }
        );
        agents
            .ask(crate::SetPolishEnabled { enabled: true })
            .await
            .unwrap();
        let r = agents.ask(polish()).await.unwrap();
        assert_eq!(r.refused.as_deref(), Some("no session"));
    }

    /// 137F R1-17: a session's model pick and `SetTitleModel` reach the
    /// endless compactor of an attached session, no re-attach.
    #[tokio::test]
    async fn model_and_title_picks_reach_the_compactor() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.set_model(SetModel {
            session: Some(root),
            model: "testvis/eyes".into(),
            manual: true,
        })
        .await
        .unwrap();
        assert_eq!(a.endless.memory(root).await.unwrap().model, "testvis/eyes");
        let agents = Agents::spawn(a);
        agents
            .ask(SetTitleModel {
                model: Some("testvis/blind".into()),
            })
            .await
            .unwrap();
        let m = agents
            .ask(SessionMemoryQuery { session: root })
            .await
            .unwrap();
        assert_eq!(
            m.model, "testvis/blind",
            "the title model outranks the session's"
        );
        agents.ask(SetTitleModel { model: None }).await.unwrap();
        let m = agents
            .ask(SessionMemoryQuery { session: root })
            .await
            .unwrap();
        assert_eq!(m.model, "testvis/eyes");
    }

    /// 148 R1: a pick with a session switches that session alone, manual or
    /// not — `default_model`, `agent.model` in config.yaml and
    /// `DefaultModelChanged` stay put. Only a manual pick with no session (the
    /// Settings path) sets the default; a programmatic one with no session
    /// resolves and nothing else. A new session without a model of its own
    /// starts on the default; one opened with `CreateSessionOptions.model`
    /// (a meeting, a draft pick) leaves it alone.
    #[tokio::test]
    async fn only_a_settings_pick_becomes_the_default() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let yaml = dir.join("config.yaml");
        let at_start = a.default_model.clone();
        let programmatic = |session, model: &str| SetModel {
            session,
            model: model.into(),
            manual: false,
        };

        // No session, not manual: resolves and nothing else.
        let info = a.set_model(programmatic(None, "blind")).await.unwrap();
        assert_eq!(info.id, "testvis/blind");
        assert_eq!(a.default_model, at_start);
        assert!(!yaml.exists());

        // A schedule's `model:` on its own session.
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.set_model(programmatic(Some(root), "testvis/blind"))
            .await
            .unwrap();
        let row = a.store.session(root).await.unwrap().unwrap();
        assert_eq!(row.model, "testvis/blind");
        assert_eq!(a.default_model, at_start);
        assert!(!yaml.exists());

        // (a) The in-chat picker: the session moves, the default does not.
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(Arc::clone(&seen)), runtime::subscriber_mailbox());
        a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
        a.set_model(SetModel {
            session: Some(root),
            model: "testvis/eyes".into(),
            manual: true,
        })
        .await
        .unwrap();
        let row = a.store.session(root).await.unwrap().unwrap();
        assert_eq!(row.model, "testvis/eyes");
        assert_eq!(a.default_model, at_start);
        assert!(!yaml.exists());
        // The bus is a `tell`; a round trip through the sink orders us after it.
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: root,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        assert!(
            !seen
                .lock()
                .iter()
                .any(|e| matches!(e, AgentEvent::DefaultModelChanged { .. })),
            "{:?}",
            seen.lock()
        );

        // (b) Settings: no session, manual — the default, config.yaml, the event.
        a.set_model(SetModel {
            session: None,
            model: "testvis/eyes".into(),
            manual: true,
        })
        .await
        .unwrap();
        assert_eq!(a.default_model, "testvis/eyes");
        let written = std::fs::read_to_string(&yaml).unwrap();
        assert!(written.contains("  model: testvis/eyes\n"), "{written}");
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: root,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        assert!(
            seen.lock().iter().any(|e| matches!(
                e,
                AgentEvent::DefaultModelChanged { model } if model == "testvis/eyes"
            )),
            "{:?}",
            seen.lock()
        );

        // New sessions: the default, unless the create names a model (a meeting);
        // that one never writes the default back.
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let plain = agents
            .ask(CreateSession {
                options: CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap();
        let row = store.session(plain.session).await.unwrap().unwrap();
        assert_eq!(row.model, "testvis/eyes");
        let meeting = agents
            .ask(CreateSession {
                options: CreateSessionOptions {
                    model: Some("testvis/blind".into()),
                    ..Default::default()
                },
                peer: None,
            })
            .await
            .unwrap();
        let row = store.session(meeting.session).await.unwrap().unwrap();
        assert_eq!(row.model, "testvis/blind");
        assert_eq!(agents.ask(GetDefaultModel).await.unwrap(), "testvis/eyes");
        let written = std::fs::read_to_string(&yaml).unwrap();
        assert!(written.contains("  model: testvis/eyes\n"), "{written}");
    }

    /// 148 R1: an effort pick with a session changes that session's row
    /// alone, manual or not; only a manual change with no session (Settings)
    /// sets `default_effort`, `agent.effort` in config.yaml and announces
    /// `DefaultEffortChanged`. A new session starts on the default.
    #[tokio::test]
    async fn only_a_settings_effort_becomes_the_default() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        let yaml = dir.join("config.yaml");
        assert_eq!(a.default_effort, None);
        let programmatic = |session, effort: &str| SetEffort {
            session,
            effort: Some(effort.into()),
            manual: false,
        };

        // No session, not manual: validates and nothing else.
        a.set_effort(programmatic(None, "low")).await.unwrap();
        assert_eq!(a.default_effort, None);
        assert!(!yaml.exists());

        // A bot's or schedule's effort on its own session.
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(Arc::clone(&seen)), runtime::subscriber_mailbox());
        a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
        a.set_effort(programmatic(Some(root), "low")).await.unwrap();
        let row = a.store.session(root).await.unwrap().unwrap();
        assert_eq!(row.effort.as_deref(), Some("low"));
        assert_eq!(a.default_effort, None);
        assert!(!yaml.exists());

        // (a) The in-chat picker: the session moves, the default does not.
        a.set_effort(SetEffort {
            session: Some(root),
            effort: Some("medium".into()),
            manual: true,
        })
        .await
        .unwrap();
        let row = a.store.session(root).await.unwrap().unwrap();
        assert_eq!(row.effort.as_deref(), Some("medium"));
        assert_eq!(a.default_effort, None);
        assert!(!yaml.exists());

        // (b) Settings: no session, manual — the default, config.yaml, the event.
        a.set_effort(SetEffort {
            session: None,
            effort: Some("high".into()),
            manual: true,
        })
        .await
        .unwrap();
        let row = a.store.session(root).await.unwrap().unwrap();
        assert_eq!(row.effort.as_deref(), Some("medium"));
        assert_eq!(a.default_effort.as_deref(), Some("high"));
        let written = std::fs::read_to_string(&yaml).unwrap();
        assert!(written.contains("  effort: \"high\"\n"), "{written}");
        // The bus is a `tell`; a round trip through the sink orders us after it.
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: root,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let defaults: Vec<_> = seen
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::DefaultEffortChanged { effort } => Some(effort.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(defaults, vec![Some("high".to_owned())], "{:?}", seen.lock());

        // New sessions start on the default.
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let plain = agents
            .ask(CreateSession {
                options: CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap();
        let row = store.session(plain.session).await.unwrap().unwrap();
        assert_eq!(row.effort.as_deref(), Some("high"));
        assert_eq!(
            agents.ask(GetDefaultEffort).await.unwrap(),
            Some("high".to_owned())
        );
    }

    /// 148 E2: a native child of a desk starts on the desk agent's configured
    /// model ([`SetDeskModel`]) unless the spawn names one; a desk without a
    /// model, and any non-desk parent, fall through to `agent.subagents.model`
    /// and then the default.
    #[tokio::test]
    async fn a_desk_child_starts_on_the_desk_agents_model() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let default = a.default_model.clone();
        async fn child_of(a: &mut Agents, parent: SessionId, pick: Option<&str>) -> String {
            let child = SessionId::new();
            a.session(Some(child), Some((parent, "kid")), None, (pick, None))
                .await
                .unwrap();
            a.store.session(child).await.unwrap().unwrap().model
        }

        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.desk_models.insert(desk, Some("testvis/eyes".into()));
        assert_eq!(child_of(&mut a, desk, None).await, "testvis/eyes");
        // An explicit pick wins.
        assert_eq!(
            child_of(&mut a, desk, Some("testvis/blind")).await,
            "testvis/blind"
        );

        // A desk without a model: the subagent default, then the default.
        let (bare, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.desk_models.insert(bare, None);
        assert_eq!(child_of(&mut a, bare, None).await, default);
        let blind = a.catalog.resolve("testvis/blind", &*a.configured).unwrap();
        *a.subagent_model.write().unwrap_or_else(|p| p.into_inner()) = Some(blind);
        assert_eq!(child_of(&mut a, bare, None).await, "testvis/blind");
        // A non-desk parent never sees a desk model.
        let (plain, _) = a.session(None, None, None, (None, None)).await.unwrap();
        assert_eq!(child_of(&mut a, plain, None).await, "testvis/blind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 194: `testvis/think` takes `off|low|high`, `testvis/blind` no effort.
    fn effort_catalog(dir: &Path) -> Catalog {
        let model = |id: &str, efforts: bool| {
            let mut m = serde_json::json!({
                "id": id, "name": id,
                "limit": { "context": 100_000, "output": 4_096 }
            });
            if efforts {
                m["reasoning_options"] =
                    serde_json::json!([{ "type": "effort", "values": ["low", "high"] }]);
            }
            m
        };
        let user = serde_json::json!({
            "testvis": {
                "id": "testvis", "name": "Test",
                "models": { "think": model("think", true), "blind": model("blind", false) }
            }
        });
        std::fs::write(dir.join("models.json"), user.to_string()).unwrap();
        Catalog::load(dir)
    }

    /// `ask` → the effort stored on a new native child of the parent (test-only spawn
    /// through the live actor, as `spawn_subagent` without `model`/`effort`).
    #[derive(Clone, Copy, Debug)]
    struct SpawnKid(SessionId);

    impl Message<SpawnKid> for Agents {
        type Reply = Option<String>;

        async fn handle(
            &mut self,
            m: SpawnKid,
            _: &mut Context<Self, Self::Reply>,
        ) -> Option<String> {
            let kid = SessionId::new();
            self.session(Some(kid), Some((m.0, "kid")), None, (None, None))
                .await
                .unwrap();
            self.store.session(kid).await.unwrap().unwrap().effort
        }
    }

    /// The effort stored on `id`'s row.
    async fn row_effort_of(store: &Store, id: SessionId) -> Option<String> {
        store.session(id).await.unwrap().unwrap().effort
    }

    /// A new root through the live actor.
    async fn new_root(agents: &ActorRef<Agents>) -> SessionId {
        agents
            .ask(CreateSession {
                options: CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap()
            .session
    }

    /// 194 row 3: `SetSubagentEffort` is checked against the effective subagent model
    /// (D4b), persisted to `agent.subagents.effort` (D4g), and live for the next spawn
    /// and the system prompt (D4e/f/h); a subagent model that does not take it drops it
    /// (D4c) and "Same as new sessions" clears it (D4d).
    #[tokio::test]
    async fn subagent_effort_is_live_validated_and_persisted() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(effort_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        a.default_model = "testvis/think".into();
        let store = a.store.clone();
        let yaml = dir.join("config.yaml");
        let agents = Agents::spawn(a);
        let set = |e: &str| SetSubagentEffort {
            effort: Some(e.into()),
        };
        let written = || {
            agent_section(&std::fs::read_to_string(&yaml).unwrap())
                .subagents
                .effort
        };
        let model = |m: Option<&str>| SetSubagentModel {
            model: m.map(Into::into),
        };

        // D4b: no subagent model, so the default for new sessions decides.
        let err = agents.ask(set("bogus")).await.unwrap_err().to_string();
        assert_eq!(err, "think accepts off|low|high");
        assert!(agents.ask(set("max")).await.is_err());
        assert!(!yaml.exists(), "a refused effort writes nothing");
        assert_eq!(
            agents.ask(set("low")).await.unwrap().as_deref(),
            Some("low")
        );
        assert_eq!(
            agents.ask(GetSubagentEffort).await.unwrap().as_deref(),
            Some("low")
        );
        assert_eq!(written().as_deref(), Some("low"));

        // D4e/f: the next spawn without an effort starts on the live value.
        let root = new_root(&agents).await;
        assert_eq!(
            row_effort_of(&store, root).await,
            None,
            "a root keeps agent.effort"
        );
        assert_eq!(
            agents.ask(SpawnKid(root)).await.unwrap().as_deref(),
            Some("low")
        );
        agents.ask(set("high")).await.unwrap();
        assert_eq!(
            agents.ask(SpawnKid(root)).await.unwrap().as_deref(),
            Some("high")
        );
        // D4h: the system prompt names it.
        let prompt = agents.ask(ShowPrompt { session: None }).await.unwrap();
        assert!(
            prompt.contains("\nSubagent default effort: high ("),
            "{prompt}"
        );

        // D4c: a subagent model without efforts drops it, then decides D4b.
        agents.ask(model(Some("testvis/blind"))).await.unwrap();
        assert_eq!(agents.ask(GetSubagentEffort).await.unwrap(), None);
        assert_eq!(written(), None);
        let err = agents.ask(set("low")).await.unwrap_err().to_string();
        assert!(err.contains("no reasoning effort"), "{err}");

        // A model that takes it keeps it; D4d: "Same as new sessions" clears it.
        agents.ask(model(Some("testvis/think"))).await.unwrap();
        agents.ask(set("high")).await.unwrap();
        agents.ask(model(Some("testvis/think"))).await.unwrap();
        assert_eq!(
            agents.ask(GetSubagentEffort).await.unwrap().as_deref(),
            Some("high")
        );
        let prompt = agents.ask(ShowPrompt { session: None }).await.unwrap();
        assert!(
            prompt.contains("\nSubagent default model: testvis/think, effort high ("),
            "{prompt}"
        );
        agents.ask(model(None)).await.unwrap();
        assert_eq!(agents.ask(GetSubagentEffort).await.unwrap(), None);
        assert_eq!(written(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 194 rows 4–5: `SetDeskModel`'s effort puts the desk row on it and is the first
    /// fallback of the desk's native children (before the subagent effort); one the
    /// desk's model does not take is dropped; clearing it puts the desk back on
    /// `agent.effort`; a desk whose agent never set one keeps its own `/effort`.
    #[tokio::test]
    async fn a_desk_and_its_children_start_on_the_agents_effort() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(effort_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        a.default_model = "testvis/think".into();
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        agents
            .ask(SetSubagentEffort {
                effort: Some("low".into()),
            })
            .await
            .unwrap();
        let desk = new_root(&agents).await;
        let set = |model: Option<&str>, effort: Option<&str>| SetDeskModel {
            session: desk,
            model: model.map(Into::into),
            effort: effort.map(Into::into),
        };

        // An agent without an effort leaves the desk's own pick alone.
        agents
            .ask(SetEffort {
                session: Some(desk),
                effort: Some("low".into()),
                manual: true,
            })
            .await
            .unwrap();
        agents.ask(set(None, None)).await.unwrap();
        assert_eq!(row_effort_of(&store, desk).await.as_deref(), Some("low"));

        // The agent's effort: on the desk now, and its children's before the subagent one.
        agents.ask(set(None, Some("high"))).await.unwrap();
        assert_eq!(row_effort_of(&store, desk).await.as_deref(), Some("high"));
        assert_eq!(
            agents.ask(SpawnKid(desk)).await.unwrap().as_deref(),
            Some("high")
        );

        // Cleared: the desk is back on agent.effort (unset), children on the subagent one.
        agents.ask(set(None, None)).await.unwrap();
        assert_eq!(row_effort_of(&store, desk).await, None);
        assert_eq!(
            agents.ask(SpawnKid(desk)).await.unwrap().as_deref(),
            Some("low")
        );

        // A desk model without efforts drops a configured one (and F6 the children's).
        agents.ask(set(None, Some("high"))).await.unwrap();
        agents
            .ask(set(Some("testvis/blind"), Some("high")))
            .await
            .unwrap();
        assert_eq!(row_effort_of(&store, desk).await, None);
        assert_eq!(agents.ask(SpawnKid(desk)).await.unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 194 row 6: `SetAdvisorDefaults.model` resolves and persists `advisor.model`,
    /// updates the live defaults `Info.advisor` carries, and owns its effort: D4b
    /// against it (the word alone with no model), D4c drop, D4d clear. R1: under
    /// the registry lock, as the setter tells the global `Advisors` (an advisor rig's).
    #[test]
    fn the_advisor_default_model_is_persisted_live_and_owns_its_effort() {
        crate::with_test_registry(async {
            use crate::{GetAdvisorDefaults, SetAdvisorDefaults};
            let (_scratch, mut a) = agents().await;
            let dir = a.rt.data_dir.clone();
            a.catalog = Arc::new(effort_catalog(&dir));
            a.configured = Arc::new(|ns: &str| ns == "testvis");
            let yaml = dir.join("config.yaml");
            let agents = Agents::spawn(a);
            let model = |m: Option<&str>| SetAdvisorDefaults {
                model: Some(m.map(Into::into)),
                ..SetAdvisorDefaults::default()
            };
            let effort = |e: &str| SetAdvisorDefaults {
                effort: Some(Some(e.into())),
                ..SetAdvisorDefaults::default()
            };
            let written = |key: &str| {
                let v: serde_yaml::Value =
                    serde_yaml::from_str(&std::fs::read_to_string(&yaml).unwrap()).unwrap();
                v["advisor"][key].clone()
            };

            // No model: only the word is checked.
            assert!(agents.ask(effort("bogus")).await.is_err());
            agents.ask(effort("high")).await.unwrap();
            // Resolved (a bare id canonicalises), persisted and live; `think` takes `high`.
            let d = agents.ask(model(Some("think"))).await.unwrap();
            assert_eq!(
                (d.model.as_deref(), d.effort.as_deref()),
                (Some("testvis/think"), Some("high"))
            );
            assert_eq!(agents.ask(GetAdvisorDefaults).await.unwrap(), d);
            assert_eq!(written("model").as_str(), Some("testvis/think"));
            assert!(
                agents.ask(model(Some("nope/none"))).await.is_err(),
                "un-keyed"
            );
            // D4b against the model.
            let err = agents.ask(effort("max")).await.unwrap_err().to_string();
            assert_eq!(err, "think accepts off|low|high");
            // D4c: a model without efforts drops it.
            let d = agents.ask(model(Some("testvis/blind"))).await.unwrap();
            assert_eq!(
                (d.model.as_deref(), d.effort),
                (Some("testvis/blind"), None)
            );
            assert!(written("effort").is_null());
            // D4d: `None` clears both.
            agents.ask(model(Some("testvis/think"))).await.unwrap();
            agents.ask(effort("low")).await.unwrap();
            let d = agents.ask(model(None)).await.unwrap();
            assert_eq!((d.model, d.effort), (None, None));
            assert!(written("model").is_null() && written("effort").is_null());
            assert_eq!(agents.ask(GetAdvisorDefaults).await.unwrap().model, None);
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }

    /// 194 R1, persist before publish: with config.yaml unwritable, the subagent
    /// effort setter, a subagent model change that would drop it, and the advisor
    /// default model (dropping its effort) and effort each return `Err` and leave
    /// both the live and the on-disk value as they were. Under the registry lock
    /// (the advisor setter tells the global `Advisors`).
    #[test]
    fn a_failed_config_write_changes_no_effort_setting() {
        crate::with_test_registry(async {
            use crate::{GetAdvisorDefaults, SetAdvisorDefaults};
            use std::os::unix::fs::PermissionsExt;
            let (_scratch, mut a) = agents().await;
            let dir = a.rt.data_dir.clone();
            a.catalog = Arc::new(effort_catalog(&dir));
            a.configured = Arc::new(|ns: &str| ns == "testvis");
            a.default_model = "testvis/think".into();
            let yaml = dir.join("config.yaml");
            let agents = Agents::spawn(a);
            let advisor = |model: Option<&str>, effort: Option<&str>| SetAdvisorDefaults {
                model: model.map(|m| Some(m.into())),
                effort: effort.map(|e| Some(e.into())),
                ..SetAdvisorDefaults::default()
            };
            let effort = |e: &str| SetSubagentEffort {
                effort: Some(e.into()),
            };
            agents.ask(effort("low")).await.unwrap();
            agents
                .ask(advisor(Some("testvis/think"), None))
                .await
                .unwrap();
            agents.ask(advisor(None, Some("low"))).await.unwrap();
            let before = std::fs::read_to_string(&yaml).unwrap();
            let mode =
                |m| std::fs::set_permissions(&yaml, std::fs::Permissions::from_mode(m)).unwrap();
            mode(0o444);

            let err = agents.ask(effort("high")).await.unwrap_err().to_string();
            assert!(err.contains("config.yaml"), "{err}");
            // `blind` takes no effort: the drop is refused before the model moves.
            let blind = SetSubagentModel {
                model: Some("testvis/blind".into()),
            };
            assert!(agents.ask(blind).await.is_err());
            assert!(agents.ask(SetSubagentModel { model: None }).await.is_err());
            assert!(
                agents
                    .ask(advisor(Some("testvis/blind"), None))
                    .await
                    .is_err()
            );
            assert!(agents.ask(advisor(None, Some("high"))).await.is_err());
            mode(0o644);

            assert_eq!(
                agents.ask(GetSubagentEffort).await.unwrap().as_deref(),
                Some("low")
            );
            assert!(agents.ask(GetSubagentModel).await.unwrap().is_none());
            let d = agents.ask(GetAdvisorDefaults).await.unwrap();
            assert_eq!(
                (d.model.as_deref(), d.effort.as_deref()),
                (Some("testvis/think"), Some("low"))
            );
            assert_eq!(std::fs::read_to_string(&yaml).unwrap(), before);
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }

    /// `ask` → the desk sessions with a model entry (test-only inspection).
    #[derive(Clone, Copy, Debug)]
    struct DeskModelKeys;

    impl Message<DeskModelKeys> for Agents {
        type Reply = Vec<SessionId>;

        async fn handle(
            &mut self,
            _: DeskModelKeys,
            _: &mut Context<Self, Self::Reply>,
        ) -> Vec<SessionId> {
            self.desk_models.keys().copied().collect()
        }
    }

    /// 148 R1 F5: a desk's model entry leaves with the desk — on a put-away
    /// (`MarkDesk { agent: None }`) and on `DeleteSession`.
    #[tokio::test]
    async fn desk_models_leave_with_the_desk() {
        let (_scratch, a) = agents().await;
        let dir = a.rt.data_dir.clone();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::write(dir.join("agents/ops.md"), "Ops.").unwrap();
        let agents = Agents::spawn(a);
        let open = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                let s = agents
                    .ask(CreateSession {
                        options: CreateSessionOptions::default(),
                        peer: None,
                    })
                    .await
                    .unwrap()
                    .session;
                agents
                    .ask(MarkDesk {
                        session: s,
                        agent: Some("ops".into()),
                    })
                    .await
                    .unwrap();
                agents
                    .ask(SetDeskModel {
                        session: s,
                        model: Some("x/y".into()),
                        effort: None,
                    })
                    .await
                    .unwrap();
                s
            }
        };
        let put_away = open(&agents).await;
        agents
            .ask(MarkDesk {
                session: put_away,
                agent: None,
            })
            .await
            .unwrap();
        let deleted = open(&agents).await;
        agents
            .ask(DeleteSession { session: deleted })
            .await
            .unwrap();
        let kept = open(&agents).await;
        assert_eq!(agents.ask(DeskModelKeys).await.unwrap(), vec![kept]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 148 R4: `ResolveModel` refuses a provider without credentials with
    /// `SetSubagentModel`'s words, accepts a keyed one (canonical id), lets an
    /// unknown id under a keyed provider pass through, and moves nothing.
    #[tokio::test]
    async fn resolve_model_checks_credentials_without_side_effects() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let default = a.default_model.clone();
        assert_eq!(
            a.resolve_keyed("nope/none").unwrap_err(),
            "provider `nope` has no credentials"
        );
        assert_eq!(
            a.resolve_keyed("othervis/eyes").unwrap_err(),
            "provider `othervis` has no credentials"
        );
        assert_eq!(a.resolve_keyed("testvis/eyes").unwrap().id, "testvis/eyes");
        assert_eq!(
            a.resolve_keyed("testvis/unlisted").unwrap().id,
            "testvis/unlisted"
        );
        assert_eq!(a.default_model, default);
        assert!(!dir.join("config.yaml").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 148 R1 F6: a new session with its own model inherits the default
    /// effort only when that model takes it; a model without reasoning
    /// options drops it (its own default applies), an unknown pass-through
    /// id keeps it, and an explicit effort pick is never second-guessed here.
    #[tokio::test]
    async fn an_inherited_default_effort_the_model_rejects_is_dropped() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        a.default_effort = Some("high".into());
        async fn row_effort(a: &Agents, id: SessionId) -> Option<String> {
            a.store.session(id).await.unwrap().unwrap().effort
        }
        // `blind` takes no effort: the inherited default is dropped.
        let (blind, _) = a
            .session(None, None, None, (Some("testvis/blind"), None))
            .await
            .unwrap();
        assert_eq!(row_effort(&a, blind).await, None);
        // A pass-through id cannot be checked: the default is kept.
        let (raw, _) = a
            .session(None, None, None, (Some("nowhere/model"), None))
            .await
            .unwrap();
        assert_eq!(row_effort(&a, raw).await.as_deref(), Some("high"));
        // An explicit pick is the caller's (validated upstream).
        let (picked, _) = a
            .session(None, None, None, (Some("testvis/blind"), Some("low")))
            .await
            .unwrap();
        assert_eq!(row_effort(&a, picked).await.as_deref(), Some("low"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The `agent:` section of a written config.yaml, as the engine would load it.
    fn agent_section(yaml: &str) -> AgentConfig {
        let file: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        serde_yaml::from_value(file["agent"].clone()).unwrap()
    }

    /// 127 §1: starring resolves through the catalog, keeps starring order,
    /// never duplicates, is a no-op in the requested state, persists one
    /// flow-sequence line that `AgentConfig` reads back, and announces the
    /// new list; an id the resolver rejects (like `set_model`: an ambiguous
    /// bare id — `provider/model` passes through) leaves the file alone.
    #[tokio::test]
    async fn favorites_persist_as_one_flow_sequence_line() {
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        a.catalog = Arc::new(vision_catalog(&dir));
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        let yaml = dir.join("config.yaml");
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Sink::spawn_with_mailbox(Sink(Arc::clone(&seen)), runtime::subscriber_mailbox());
        a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let star = |model: &str, favorite| SetModelFavorite {
            model: model.into(),
            favorite,
        };

        // A bare id canonicalises; order is starring order; twice is once.
        assert_eq!(
            a.set_model_favorite(star("eyes", true)).await.unwrap(),
            ["testvis/eyes"]
        );
        assert_eq!(
            a.set_model_favorite(star("testvis/blind", true))
                .await
                .unwrap(),
            ["testvis/eyes", "testvis/blind"]
        );
        assert_eq!(
            a.set_model_favorite(star("testvis/eyes", true))
                .await
                .unwrap(),
            ["testvis/eyes", "testvis/blind"]
        );
        let written = std::fs::read_to_string(&yaml).unwrap();
        assert!(
            written.contains("  favorite_models: [\"testvis/eyes\", \"testvis/blind\"]\n"),
            "{written}"
        );
        let cfg = agent_section(&written);
        assert_eq!(cfg.favorite_models, ["testvis/eyes", "testvis/blind"]);

        // Rejected like `set_model` (the resolver's only error: a bare id in
        // two configured providers); nothing written or announced.
        a.configured = Arc::new(|_: &str| true);
        let err = a.set_model_favorite(star("eyes", true)).await.unwrap_err();
        assert!(err.starts_with("ambiguous model `eyes`"), "{err}");
        a.configured = Arc::new(|ns: &str| ns == "testvis");
        assert_eq!(std::fs::read_to_string(&yaml).unwrap(), written);

        // Unstar to empty: `[]`, and the config still reads back.
        assert_eq!(
            a.set_model_favorite(star("eyes", false)).await.unwrap(),
            ["testvis/blind"]
        );
        assert_eq!(
            a.set_model_favorite(star("blind", false)).await.unwrap(),
            Vec::<String>::new()
        );
        // Already unstarred: a no-op reply, no third write.
        assert_eq!(
            a.set_model_favorite(star("blind", false)).await.unwrap(),
            Vec::<String>::new()
        );
        let written = std::fs::read_to_string(&yaml).unwrap();
        assert!(written.contains("  favorite_models: []\n"), "{written}");
        assert!(agent_section(&written).favorite_models.is_empty());
        assert!(a.favorite_models.is_empty());

        // Every change was announced with the whole list; no-ops were not.
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: SessionId::new(),
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let lists: Vec<Vec<String>> = seen
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::FavoriteModelsChanged { models } => Some(models.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            lists,
            [
                vec!["testvis/eyes".to_owned()],
                vec!["testvis/eyes".to_owned(), "testvis/blind".to_owned()],
                vec!["testvis/blind".to_owned()],
                vec![],
            ]
        );
    }

    /// `ModelInfo.image` is effective input support (52 §3.2): a Cursor id
    /// with an image-capable catalog sibling reports `false`.
    #[test]
    fn model_info_image_is_effective_support() {
        let scratch = crate::test_dir("vision");
        let dir = scratch.path().to_path_buf();
        let catalog = vision_catalog(&dir);
        let (p, m) = catalog.get("testvis/eyes").unwrap();
        assert!(ModelInfo::new(&p.id, m).image);
        assert!(!ModelInfo::new("cursor", m).image);
        let mut r = catalog.resolve("testvis/eyes", &|_| true).unwrap();
        assert!(ModelInfo::from(&r).image);
        r.genai = "cursor::eyes".into();
        assert!(!ModelInfo::from(&r).image);
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
        let (_scratch, mut a) = agents_with(true).await;
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
        let (_scratch, mut a) = agents_with(true).await;
        let (me, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let (other, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let rev = a.store.context_revision(other).await.unwrap();
        assert!(
            a.store
                .set_title_at_revision(
                    other,
                    "Merge design 62",
                    &["rust".into(), "merge".into()],
                    &rev
                )
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
        let (_scratch, mut a) = agents_with(true).await;
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
        let (_scratch, mut a) = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.store.set_title(root, "deploy checklist").await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let rows = a.directory(child).await.unwrap();
        let parent = rows
            .iter()
            .find(|r| r.relationship == PeerRelationship::Parent)
            .unwrap();
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
        assert_eq!(
            cold_row.activity.as_ref().unwrap().state,
            PeerLiveState::Stored
        );
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
            reachable: None,
        };
        let act = |state: PeerLiveState, ptys: u16, secs_ago: i64| PeerActivity {
            state,
            last_active: Some(Utc::now() - chrono::Duration::seconds(secs_ago)),
            turn_secs: None,
            jobs: 0,
            ptys,
            children: 0,
            stalled_secs: None,
        };
        let mut rows = vec![
            row("peer:lab", None),
            row("idle-old", Some(act(PeerLiveState::Idle, 0, 600))),
            row("pty", Some(act(PeerLiveState::Idle, 1, 300))),
            row("idle-new", Some(act(PeerLiveState::Idle, 0, 10))),
            row("running", Some(act(PeerLiveState::Running, 0, 900))),
        ];
        arrange(&mut rows, false, false);
        let order: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(
            order,
            ["pty", "running", "idle-new", "idle-old", "peer:lab"]
        );
        arrange(&mut rows, true, false);
        let order: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(order, ["pty", "running"]);

        // 183 F6 (review 1): the own tree first, then contacts with at most 8 export rows
        // each (the contact's row names the call that lists the rest), then the rest;
        // active_only keeps contacts; `kind: contact` lists every row.
        let contact = |address: &str| PeerInfo {
            kind: PeerKind::A2a,
            relationship: PeerRelationship::Remote,
            tags: vec!["contact".into()],
            name: address.trim_start_matches("peer:").into(),
            ..row(address, None)
        };
        let child = PeerInfo {
            relationship: PeerRelationship::Child,
            ..row("kid", Some(act(PeerLiveState::Idle, 0, 5000)))
        };
        let mut all = vec![
            row("stored", Some(act(PeerLiveState::Idle, 0, 1))),
            contact("peer:sky"),
        ];
        all.extend((0..10).map(|i| contact(&format!("peer:sky/e{i}"))));
        all.push(child);
        let mut rows = all.clone();
        arrange(&mut rows, false, false);
        let order: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        let mut want = vec!["kid", "peer:sky"];
        let shared: Vec<String> = (0..8).map(|i| format!("peer:sky/e{i}")).collect();
        want.extend(shared.iter().map(String::as_str));
        want.push("stored");
        assert_eq!(order, want);
        assert_eq!(
            rows[1].title.as_deref(),
            Some("+2 more: discover_peers(kind=\"contact\", query=\"sky\")")
        );
        let mut rows = all.clone();
        arrange(&mut rows, true, false);
        assert_eq!(
            rows.len(),
            9,
            "active_only keeps the contact and its 8 rows"
        );
        let mut rows = all;
        arrange(&mut rows, false, true);
        assert_eq!(
            rows.iter()
                .filter(|r| r.address.starts_with("peer:sky/"))
                .count(),
            10
        );
    }

    /// 200 (D5): the kind matrix row by row through `full_access_eligibility`, and an
    /// opted-out grandparent hiding its grandchild on both halves.
    #[tokio::test]
    async fn full_access_eligibility_follows_the_kind_matrix() {
        use crate::communication::PeerScope;
        use crate::communication::network::access::{Eligibility, row_eligibility};
        let (_scratch, mut a) = agents_with(true).await;
        async fn row(a: &Agents, parent: Option<SessionId>, kind: SessionKind) -> SessionId {
            let id = SessionId::new();
            a.store
                .create_session_kind(id, "m", parent, None, None, kind)
                .await
                .unwrap();
            id
        }
        let owner = |a: &Agents, id: SessionId| {
            let store = a.store.clone();
            async move {
                store
                    .set_root_scope(id, PeerScope::Owner, None)
                    .await
                    .unwrap()
            }
        };
        let root = row(&a, None, SessionKind::Chat).await;
        owner(&a, root).await;
        let child = row(&a, Some(root), SessionKind::Chat).await;
        let ask = row(&a, None, SessionKind::Ask).await;
        owner(&a, ask).await;
        let advisor = row(&a, Some(root), SessionKind::Advisor).await;
        let observed = row(&a, Some(root), SessionKind::Observed).await;
        let acp = row(&a, Some(root), SessionKind::Chat).await;
        a.store.set_acp(acp, "claude", None, &[]).await.unwrap();
        let isolated = row(&a, None, SessionKind::Chat).await;
        let isolated_child = row(&a, Some(isolated), SessionKind::Chat).await;
        let inbox = row(&a, None, SessionKind::Chat).await;
        owner(&a, inbox).await;
        a.store
            .set_contact_inbox(inbox, "peer:x|export-1")
            .await
            .unwrap();
        let in_inbox = row(&a, Some(inbox), SessionKind::Chat).await;
        let yes = Eligibility {
            visible: true,
            sendable: true,
        };
        let listed = Eligibility {
            visible: true,
            sendable: false,
        };
        let no = Eligibility::default();
        for (s, want, what) in [
            (
                SessionId::new(),
                no,
                "no durable row (a private chat, a random id)",
            ),
            (root, yes, "an Owner root"),
            (child, yes, "its native child"),
            (ask, yes, "an ask root"),
            (advisor, no, "an advisor"),
            (observed, listed, "an observed child"),
            (acp, yes, "an ACP session: unscoped intake takes it"),
            (isolated, no, "an Isolated root"),
            (isolated_child, no, "its tree"),
            (inbox, no, "another contact's inbox"),
            (in_inbox, no, "a session under it"),
        ] {
            assert_eq!(a.full_access_eligibility(s).await, want, "{what}");
        }
        // A paused tree: listed, takes nothing; resumed, as before.
        a.paused.insert(root, Vec::new());
        assert_eq!(a.full_access_eligibility(child).await, listed, "paused");
        a.paused.remove(&root);
        // The global ceiling.
        a.communication.0.write().set_global(false, "test");
        assert_eq!(a.full_access_eligibility(child).await, no, "ceiling off");
        a.communication.0.write().set_global(true, "test");
        // Another machine's row.
        a.store.owned_elsewhere(child, "another-engine").await;
        assert_eq!(
            a.full_access_eligibility(child).await,
            no,
            "owned elsewhere"
        );

        // An opted-out grandparent: the row half reads it from the replicated rows, the live
        // half from the policy; each alone hides the grandchild.
        let top = row(&a, None, SessionKind::Chat).await;
        owner(&a, top).await;
        let mid = row(&a, Some(top), SessionKind::Chat).await;
        let leaf = row(&a, Some(mid), SessionKind::Chat).await;
        assert_eq!(a.full_access_eligibility(leaf).await, yes);
        a.store
            .set_session_communication(top, Some(false))
            .await
            .unwrap();
        assert!(!row_eligibility(&a.store, leaf).await, "row half");
        a.store.set_session_communication(top, None).await.unwrap();
        assert!(row_eligibility(&a.store, leaf).await);
        a.communication.0.write().set_session(top, false).unwrap();
        let r = a.store.session(leaf).await.unwrap().unwrap();
        assert_eq!(a.full_access_live(&r), no, "live half");
    }

    /// A contact of three machines for [`PeerPlan::finish`]: each answers its own ids (as
    /// sort keys) below `after`, up to `limit`; a machine in `down` answers nothing.
    struct Members {
        rows: Vec<(&'static str, Vec<u128>)>,
        down: parking_lot::Mutex<Vec<&'static str>>,
    }

    #[async_trait::async_trait]
    impl crate::communication::network::PeerSender for Members {
        async fn send(
            &self,
            _: crate::communication::network::OutboundPeerMessage,
        ) -> Result<
            crate::communication::network::RemoteReply,
            crate::communication::network::RemoteSendError,
        > {
            Err(crate::communication::network::RemoteSendError::Refused(
                "no".into(),
            ))
        }

        async fn list_sessions(
            &self,
            req: crate::communication::network::access::ListRequest,
        ) -> Result<crate::communication::network::access::ContactListing, String> {
            use crate::communication::network::access::{
                ContactListing, ListedRow, ListedSession, member_page,
            };
            let down = self.down.lock().clone();
            let limit = usize::from(req.limit);
            let (mut rows, mut partial, mut full) = (Vec::new(), Vec::new(), false);
            for (m, ids) in &self.rows {
                if down.contains(m) {
                    partial.push((*m).to_owned());
                    continue;
                }
                let mine = ids
                    .iter()
                    .map(|n| ListedSession {
                        session: SessionId(Uuid::from_u128(*n)),
                        title: Some(format!("s{n}")),
                        kind: PeerKind::Native,
                        state: PeerState::Idle,
                        send: true,
                        reply: true,
                        activity: None,
                    })
                    .collect();
                let page = member_page(mine, req.after, req.limit);
                full |= page.len() >= limit;
                rows.extend(page.into_iter().map(|s| ListedRow {
                    machine: (*m).to_owned(),
                    session: s,
                }));
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.session.session.0));
            rows.dedup_by_key(|r| r.session.session);
            let trimmed = rows.len() > limit;
            rows.truncate(limit);
            let cursor = (full || trimmed)
                .then(|| rows.last().map(|r| r.session.session))
                .flatten();
            Ok(ContactListing {
                rows,
                partial,
                cursor,
            })
        }
    }

    /// 200 (Contract): `discover_peers` for a contact that gives us full access. The walk
    /// pages by the global cursor, a machine that timed out on a later page is in `partial`
    /// and answers on the repeat; the default preview is 8 rows under the contact's row
    /// with the walk's call, no cursor; page 1's prefix (64 export rows) never trims the
    /// session page; a `full:` cursor no granting contact names (an inbox has none) is
    /// unknown.
    #[tokio::test]
    async fn the_full_access_walk_and_preview() {
        use crate::communication::DirectoryKind;
        use crate::communication::network::PeerSender;
        use crate::communication::network::access::full_cursor;
        let contact_row = |address: String, title: Option<String>| PeerInfo {
            name: address.trim_start_matches("peer:").into(),
            address,
            kind: PeerKind::A2a,
            relationship: PeerRelationship::Remote,
            state: PeerState::Idle,
            send: true,
            reply: true,
            stream: false,
            activity: None,
            title,
            tags: vec!["contact".into()],
            engine: None,
            reachable: None,
        };
        let members = Arc::new(Members {
            rows: vec![("A", vec![100, 80]), ("B", vec![90, 70])],
            down: parking_lot::Mutex::new(Vec::new()),
        });
        let plan = |cursor: Option<String>, query: &str, limit: u16, exports: usize| PeerPlan {
            m: ListPeers {
                session: SessionId::new(),
                query: Some(query.into()),
                cursor,
                limit: Some(limit),
                active_only: false,
                kind: Some(DirectoryKind::Contact),
            },
            rows: std::iter::once(contact_row("peer:maya".into(), None))
                .chain((0..exports).map(|i| contact_row(format!("peer:maya/e{i}"), None)))
                .collect(),
            granting: vec![("maya".into(), members.clone() as Arc<dyn PeerSender>)],
        };
        let id = |n: u128| SessionId(Uuid::from_u128(n));
        let sessions = |p: &PeerPage| -> Vec<String> {
            p.peers
                .iter()
                .filter(|r| r.address.contains("/session:"))
                .map(|r| r.address.clone())
                .collect()
        };
        let at = |n: u128| format!("peer:maya/session:{}", id(n));
        let p1 = plan(None, "maya", 2, 0).finish().await.unwrap();
        assert_eq!(sessions(&p1), [at(100), at(90)]);
        assert_eq!(p1.peers[0].title.as_deref(), Some("full access"));
        assert_eq!(p1.cursor, Some(full_cursor("maya", id(90))));
        assert!(p1.partial.is_empty());
        members.down.lock().push("B");
        let p2 = plan(p1.cursor.clone(), "maya", 2, 0)
            .finish()
            .await
            .unwrap();
        assert_eq!(sessions(&p2), [at(80)], "no prefix on a later page");
        assert_eq!(p2.peers.len(), 1);
        assert_eq!(p2.cursor, None);
        assert_eq!(p2.partial, ["maya/B"]);
        members.down.lock().clear();
        let again = plan(p1.cursor.clone(), "peer:maya", 2, 0)
            .finish()
            .await
            .unwrap();
        assert_eq!(sessions(&again), [at(80), at(70)]);
        assert_eq!(again.cursor, None);
        assert!(again.partial.is_empty());
        // Page 1's prefix is bounded apart from the session page.
        let wide = plan(None, "maya", 3, 64).finish().await.unwrap();
        assert_eq!(wide.peers.len(), 1 + 64 + 3);
        assert_eq!(sessions(&wide).len(), 3);
        // A cursor the query does not name, and one no granting contact has.
        for (cursor, query) in [
            (full_cursor("maya", id(90)), "sky"),
            (full_cursor("sky", id(90)), "maya"),
        ] {
            let mut p = plan(Some(cursor), query, 2, 0);
            p.granting.retain(|(n, _)| n == "maya");
            assert_eq!(p.finish().await.unwrap_err(), "unknown cursor");
        }
        let mut inbox = plan(Some(full_cursor("maya", id(90))), "maya", 2, 0);
        inbox.granting.clear();
        assert_eq!(inbox.finish().await.unwrap_err(), "unknown cursor");

        // The default listing: the preview of 8 under the contact's row, the walk named.
        let many = Arc::new(Members {
            rows: vec![("A", (1..=12).collect())],
            down: parking_lot::Mutex::new(Vec::new()),
        });
        let mut p = plan(None, "", 25, 0);
        p.m.query = None;
        p.m.kind = None;
        p.granting = vec![("maya".into(), many as Arc<dyn PeerSender>)];
        let page = p.finish().await.unwrap();
        assert_eq!(page.peers[0].address, "peer:maya");
        assert_eq!(
            page.peers[0].title.as_deref(),
            Some("full access · more: discover_peers(kind=\"contact\", query=\"maya\")")
        );
        assert_eq!(sessions(&page).len(), 8);
        assert_eq!(sessions(&page)[0], at(12), "newest first");
        assert_eq!(page.cursor, None, "no cursor for a preview");
    }

    /// 200 (D4 (3)): a live private chat answers a full-access contact exactly as an unknown
    /// id (no durable row: the row half finds nothing), with the same error.
    #[tokio::test]
    async fn a_private_chat_answers_like_an_unknown_id() {
        use crate::communication::network::access::FullAccessRow;
        let (_scratch, a) = agents_with(true).await;
        let agents = Agents::spawn(a);
        let private = agents
            .ask(CreateSession {
                options: CreateSessionOptions {
                    retention: tools::Retention::Private,
                    view: Some("v1".into()),
                    ..Default::default()
                },
                peer: Some((7, CancellationToken::new())),
            })
            .await
            .unwrap()
            .session;
        let refused = |r: Result<(), SendError<FullAccessRow, a2a_types::A2AError>>| match r {
            Err(SendError::HandlerError(e)) => (e.code, e.message),
            other => panic!("expected a refusal, got {other:?}"),
        };
        let unknown = refused(agents.ask(FullAccessRow(SessionId::new())).await);
        assert_eq!(unknown.1, crate::communication::network::NOT_OWNER_TEXT);
        assert_eq!(refused(agents.ask(FullAccessRow(private)).await), unknown);
    }

    /// 200 (Interplay with 199): a file notice from a contact we gave full access passes D17's
    /// re-check for a visible chat it was never shared; while the tree is paused it is
    /// `accepted` and held (no turn, no row: status unknown), and a resend stays one
    /// pending notice; `ResumeTree` consumes it: one turn, one committed row, and a later
    /// resend adds nothing. Without full access the same notice is an ended share.
    #[tokio::test]
    async fn a_full_access_notice_for_a_paused_tree_waits_for_the_resume() {
        use crate::communication::PeerScope;
        use crate::communication::files::NoticeAdmission;
        use crate::communication::files::notices::{AdmitNotice, claim};
        use crate::store::contacts::{Cell, RecordTable, RecordWrite};
        let d = crate::test_dir("notice-full");
        let mut a = agents_on(d.path().to_path_buf(), Some("A"), true).await;
        let root = SessionId::new();
        a.store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .set_root_scope(root, PeerScope::Owner, None)
            .await
            .unwrap();
        let binding = Uuid::now_v7();
        a.store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Contacts,
                "rec",
                vec![("binding", Cell::Text(format!("peer:{binding}")))],
            )])
            .await
            .unwrap();
        const TEXT: &str = "<message_file from=\"peer:maya\" message=\"m\" name=\"a.pdf\" type=\"application/pdf\" state=\"failed\" reason=\"r\" holder=\"h\" file=\"f\"/>\nFile notice from theuth, not from peer:maya: no reply to the contact is needed.";
        let notice = |id| AdmitNotice {
            session: root,
            id,
            contact: "rec".into(),
            text: TEXT.into(),
        };
        // No full access: the share ended.
        let ended = Uuid::new_v4();
        assert_eq!(
            a.admit_notice(notice(ended)).await,
            Ok(NoticeAdmission::Accepted)
        );
        assert_eq!(a.store.errors(root).await.unwrap().len(), 1, "ended");
        // Full access (the route and the record, as a staged grant has both), the tree
        // paused: held for the resume.
        a.network.insert_full_access_for_test(binding, "maya");
        a.store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Contacts,
                "rec",
                vec![("full_access", Cell::Int(1))],
            )])
            .await
            .unwrap();
        a.paused.insert(root, Vec::new());
        let id = Uuid::new_v4();
        for _ in 0..2 {
            assert_eq!(
                a.admit_notice(notice(id)).await,
                Ok(NoticeAdmission::Accepted)
            );
        }
        assert_eq!(a.paused[&root].len(), 1, "held once, for the resume");
        assert!(claim(root, TEXT).is_some_and(|p| p.id == id), "pending");
        assert!(!a.store.notice_committed(root, id).await.unwrap(), "no row");
        assert_eq!(a.store.errors(root).await.unwrap().len(), 1, "no new row");
        assert!(a.live(root).is_none(), "no actor, no turn");

        // `ResumeTree`: the held notice is consumed like any automatic input, then committed
        // once: one turn on its text, one row; a resend after that adds nothing.
        let store = a.store.clone();
        let mut rx = tap(&a.rt).await;
        let rt = a.rt.clone();
        let agents = Agents::spawn(a);
        notice_authority(&rt, &agents);
        let report = agents.ask(ResumeTree { desk: root }).await.unwrap();
        assert_eq!(report.replayed, 1, "the one held notice");
        let mut seen = Vec::new();
        let started = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match rx.recv().await.expect("bus open") {
                    AgentEvent::TurnStarted {
                        session: s, text, ..
                    } if s == root => return text,
                    e => seen.push(format!("{e:?}").chars().take(300).collect::<String>()),
                }
            }
        })
        .await;
        assert_eq!(started.as_deref(), Ok(TEXT), "events: {seen:#?}");
        let committed = tokio::time::timeout(Duration::from_secs(60), async {
            while !store.notice_committed(root, id).await.unwrap() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(committed.is_ok(), "committed after the resume");
        assert_eq!(
            agents.ask(notice(id)).await.unwrap(),
            NoticeAdmission::Accepted,
            "a resend after the commit"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(e) = rx.try_recv() {
            assert!(
                !matches!(&e, AgentEvent::TurnStarted { session, .. } if *session == root),
                "one turn only: {e:?}"
            );
        }
        crate::communication::files::notices::clear(id);
    }

    /// SEC-200-1 (design-200 security review of `fb124d15f`): 199 D17's one predicate, at
    /// admission and at consumption (paused and resumed included). The inbound
    /// `session:<uuid>` thread a full-access send leaves never authorises on its own: a
    /// notice first arriving after Off, or after the chat became ineligible, is the ended
    /// row; one held for a paused tree across Off and the resume is given to no agent. An
    /// export share and a conversation this chat opened still authorise, the latter only
    /// while its return rule holds (the settled cards).
    #[tokio::test]
    async fn a_full_access_notice_needs_the_grant_and_visibility_now() {
        use crate::communication::PeerScope;
        use crate::communication::files::NoticeAdmission;
        use crate::communication::files::notices::{AdmitNotice, claim, clear};
        use crate::communication::network::SettledCards;
        use crate::store::contacts::{Cell, RecordTable, RecordWrite};
        let d = crate::test_dir("notice-sec2001");
        let mut a = agents_on(d.path().to_path_buf(), Some("A"), true).await;
        let root = SessionId::new();
        a.store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .set_root_scope(root, PeerScope::Owner, None)
            .await
            .unwrap();
        let binding = Uuid::now_v7();
        let record =
            |cols: Vec<(&'static str, Cell)>| RecordWrite::new(RecordTable::Contacts, "rec", cols);
        a.store
            .write_contact_records(&[record(vec![
                ("binding", Cell::Text(format!("peer:{binding}"))),
                ("alias", Cell::Text("maya".into())),
                ("full_access", Cell::Int(1)),
            ])])
            .await
            .unwrap();
        a.network.insert_full_access_for_test(binding, "maya");
        // B's full-access message into the chat left its inbound conversation.
        a.store
            .open_contact_thread(binding, "in", root, &format!("session:{root}"), false)
            .await
            .unwrap();
        const TEXT: &str = "<message_file from=\"peer:maya\" message=\"m\" name=\"a.pdf\" type=\"application/pdf\" state=\"received\" holder=\"h\" file=\"f\"/>\nFile notice from theuth, not from peer:maya: no reply to the contact is needed.";
        let notice = |id| AdmitNotice {
            session: root,
            id,
            contact: "rec".into(),
            text: TEXT.into(),
        };
        let ended = |errors: &[(i64, String)]| {
            errors
                .iter()
                .filter(|(_, t)| t.ends_with("has ended, so this was not given to the agent."))
                .count()
        };
        let off = |a: &mut Agents| {
            a.network.insert_route_for_test(binding, "maya", false);
        };
        assert_eq!(a.notice_live(root, "rec", Uuid::new_v4()).await, Ok(true));

        // Off before the notice first arrives: the inbound thread does not carry it.
        off(&mut a);
        a.store
            .write_contact_records(&[record(vec![("full_access", Cell::Int(0))])])
            .await
            .unwrap();
        let after_off = Uuid::new_v4();
        assert_eq!(
            a.admit_notice(notice(after_off)).await,
            Ok(NoticeAdmission::Accepted)
        );
        assert!(claim(root, TEXT).is_none(), "nothing pending");
        assert_eq!(ended(&a.store.errors(root).await.unwrap()), 1);
        // The route says on while the records say off (a reconcile in flight): still off.
        a.network.insert_full_access_for_test(binding, "maya");
        assert_eq!(a.notice_live(root, "rec", Uuid::new_v4()).await, Ok(false));

        // On again, the chat made ineligible (its root `Isolated`): ended too.
        a.store
            .write_contact_records(&[record(vec![("full_access", Cell::Int(1))])])
            .await
            .unwrap();
        a.store
            .set_root_scope(root, PeerScope::Isolated, None)
            .await
            .unwrap();
        let ineligible = Uuid::new_v4();
        assert_eq!(
            a.admit_notice(notice(ineligible)).await,
            Ok(NoticeAdmission::Accepted)
        );
        assert!(claim(root, TEXT).is_none(), "nothing pending");
        assert_eq!(ended(&a.store.errors(root).await.unwrap()), 2);
        a.store
            .set_root_scope(root, PeerScope::Owner, None)
            .await
            .unwrap();

        // An export share, and a conversation this chat opened, authorise without full
        // access; the conversation only while the settled cards keep it open.
        off(&mut a);
        a.store
            .write_contact_records(&[
                RecordWrite::new(
                    RecordTable::Exports,
                    "x1",
                    vec![
                        ("name", Cell::Text("ops".into())),
                        ("session", Cell::Text(root.to_string())),
                    ],
                ),
                RecordWrite::new(
                    RecordTable::Grants,
                    "rec|x1",
                    vec![
                        ("contact", Cell::Text("rec".into())),
                        ("export", Cell::Text("x1".into())),
                        ("granted", Cell::Int(1)),
                    ],
                ),
            ])
            .await
            .unwrap();
        assert_eq!(a.notice_live(root, "rec", Uuid::new_v4()).await, Ok(true));
        a.store
            .write_contact_records(&[RecordWrite::new(
                RecordTable::Grants,
                "rec|x1",
                vec![("granted", Cell::Int(0))],
            )])
            .await
            .unwrap();
        assert_eq!(a.notice_live(root, "rec", Uuid::new_v4()).await, Ok(false));
        let theirs = SessionId::new();
        for (context, export) in [
            ("out", "lab".to_owned()),
            ("fa", format!("session:{theirs}")),
        ] {
            a.store
                .open_contact_thread(binding, context, root, &export, true)
                .await
                .unwrap();
        }
        assert_eq!(
            a.notice_live(root, "rec", Uuid::new_v4()).await,
            Ok(true),
            "my own conversation, cards not settled"
        );
        let settle = |a: &mut Agents, full: &[&str], full_access| {
            a.network.settle_for_test(
                "maya",
                SettledCards {
                    full: full.iter().map(|e| (*e).to_owned()).collect(),
                    full_access,
                },
            );
        };
        settle(&mut a, &["lab"], false);
        assert_eq!(
            a.notice_live(root, "rec", Uuid::new_v4()).await,
            Ok(true),
            "the export conversation still open"
        );
        settle(&mut a, &[], true);
        assert_eq!(
            a.notice_live(root, "rec", Uuid::new_v4()).await,
            Ok(true),
            "the session conversation open while they grant us"
        );
        settle(&mut a, &[], false);
        assert_eq!(
            a.notice_live(root, "rec", Uuid::new_v4()).await,
            Ok(false),
            "both closed by the settled cards"
        );
        a.store
            .exec_for_test("DELETE FROM contact_threads WHERE outbound = 1")
            .await;

        // Held for a paused tree while on, then Off, then the resume: its turn takes it and
        // the predicate (asked again) ends it: no row of it, nothing given to the agent.
        a.network.insert_full_access_for_test(binding, "maya");
        a.store
            .write_contact_records(&[record(vec![("full_access", Cell::Int(1))])])
            .await
            .unwrap();
        a.paused.insert(root, Vec::new());
        let held = Uuid::new_v4();
        assert_eq!(
            a.admit_notice(notice(held)).await,
            Ok(NoticeAdmission::Accepted)
        );
        assert_eq!(a.paused[&root].len(), 1, "held for the resume");
        off(&mut a);
        a.store
            .write_contact_records(&[record(vec![("full_access", Cell::Int(0))])])
            .await
            .unwrap();
        let store = a.store.clone();
        let rt = a.rt.clone();
        let agents = Agents::spawn(a);
        notice_authority(&rt, &agents);
        let report = agents.ask(ResumeTree { desk: root }).await.unwrap();
        assert_eq!(report.replayed, 1, "the one held notice");
        let recorded = tokio::time::timeout(Duration::from_secs(60), async {
            while ended(&store.errors(root).await.unwrap()) < 3 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(recorded.is_ok(), "the ended row after the resume");
        assert!(claim(root, TEXT).is_none(), "settled");
        assert!(
            store
                .messages(root)
                .await
                .unwrap()
                .iter()
                .all(|(_, m)| !crate::loop_::text_of(&m.content).contains("<message_file")),
            "nothing given to the agent"
        );
        for id in [after_off, ineligible, held] {
            clear(id);
        }
    }

    /// 183 §4.5 (D4): the row's messaging opt-out is applied when the actor
    /// spawns (restart, resume, take); an unset row changes nothing.
    #[tokio::test]
    async fn actor_spawn_loads_the_rows_communication_opt_out() {
        let (_scratch, mut a) = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let opted_out = |a: &Agents| {
            a.communication
                .0
                .read()
                .state(Some(root))
                .unwrap()
                .opted_out
        };
        a.inherit_approvals(root, None).await;
        assert!(!opted_out(&a));
        a.store
            .set_session_communication(root, Some(false))
            .await
            .unwrap();
        a.inherit_approvals(root, None).await;
        assert!(opted_out(&a));
    }

    /// 50 §5.3, §8: a wait resolves only by its exact id; a disable expires
    /// it with `communication_disabled` (never a fake reply), suppresses the
    /// accepted row durably, and the same wait cannot be resolved again.
    #[tokio::test]
    async fn waits_are_exact_and_expire_on_disable() {
        let (_scratch, mut a) = agents_with(true).await;
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
        let (_scratch, mut a) = agents_with(true).await;
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
        let (_scratch, mut a) = agents_with(true).await;
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
        let (_scratch, mut a) = agents_with(true).await;
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
        let scratch = crate::test_dir("cfg");
        let dir = scratch.path().to_path_buf();
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
    }

    /// Ticket id named by a `Ledger::activity` pending-input reason.
    fn pending_ticket(why: &str) -> u64 {
        let mut words = why.split(' ');
        assert_eq!(words.next(), Some("input"), "{why}");
        words.next().unwrap().parse().unwrap()
    }

    /// 190 (D5): the owner's input (interject, flush) cancels a turn that took a contact's
    /// message exactly like the owner's own turn: no refusal, whatever the journal says.
    #[tokio::test]
    async fn an_interject_cancels_a_contact_turn_like_any_turn() {
        let (_scratch, mut a) = agents_with(true).await;
        let (p, t) = in_turn(&mut a).await;
        let cancel = |reason| Cancel {
            session: p,
            keep: None,
            scope: CancelScope::Root,
            reason,
        };
        let before = a.ledger.epoch(p);
        a.cancel(cancel(crate::CancelReason::Input)).await;
        assert!(a.ledger.epoch(p) > before, "the owner's own turn");
        for source in ["peer", "contact_full", "contact", "contact_inbox"] {
            a.store
                .input_accepted(p, t, Some("contact"), source)
                .await
                .unwrap();
            let before = a.ledger.epoch(p);
            a.cancel(cancel(crate::CancelReason::Input)).await;
            assert!(a.ledger.epoch(p) > before, "{source}: cancelled");
        }
    }

    /// A certified root in turn `t` with its opener consumed.
    pub(crate) async fn in_turn(a: &mut Agents) -> (SessionId, TurnId) {
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
        let (_scratch, mut a) = agents_with(true).await;
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
                kind: JobKind::Shell,
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
        let (_scratch, mut a) = agents_with(true).await;
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
                why: String::new(),
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
        let (_scratch, mut a) = agents().await;
        let (p, _) = in_turn(&mut a).await;
        let r = a
            .input(
                p,
                None,
                "x".into(),
                vec![PathBuf::from("/nonexistent/theuth.png")],
                None,
                InputOrigin::External,
                Delivery::Steer,
            )
            .await;
        assert!(r.is_err(), "the caller learns the drop too (62 §3.3)");
        assert_eq!(a.ledger.activity(p), None);
    }

    /// A retried `SendInput` id routes once and answers with the same session;
    /// a fresh id routes again.
    #[tokio::test]
    async fn retried_input_id_routes_once() {
        let (_scratch, mut a) = agents().await;
        let (p, _) = in_turn(&mut a).await;
        let id = InputId::new();
        let sid = a
            .human_input(Some(p), Some(id), "x".into(), vec![], None, Delivery::Steer)
            .await;
        assert_eq!(sid.unwrap(), p);
        a.last_input.clear();
        let sid = a
            .human_input(Some(p), Some(id), "x".into(), vec![], None, Delivery::Steer)
            .await;
        assert_eq!(sid.unwrap(), p);
        assert!(a.last_input.is_empty(), "the retry routed nothing");
        let fresh = Some(InputId::new());
        a.human_input(Some(p), fresh, "x".into(), vec![], None, Delivery::Steer)
            .await
            .unwrap();
        assert!(a.last_input.contains_key(&p), "a fresh id routes");
    }

    /// 62 §3.2: a root whose model is idle but that owns a running PTY is
    /// `Idle, ptys: 1, live: true` — the job, not the model, marks it as
    /// working. (`JobCounts` is fed as a synthetic reply: no `Jobs` actor is
    /// registered in this harness.)
    #[tokio::test]
    async fn activity_counts_a_pty_on_an_idle_root() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let counts = HashMap::from([(root, (0u16, 1u16))]);
        let act = a.view().assemble_activity(&[root], &counts).await;
        let act = &act[&root];
        assert_eq!(act.state, AgentState::Idle);
        assert_eq!((act.jobs, act.ptys, act.children), (0, 1, 0));
        assert!(act.live);
        assert!(
            act.last_active.is_some(),
            "a cold row falls back to `updated_at`"
        );
        assert!(act.turn_started_at.is_none());
    }

    /// 130 E2: `waiting_since_ms` is the approval's arrival while the state is
    /// `approval`, gone once it resolves, and on the wire as a plain number.
    #[tokio::test]
    async fn activity_waiting_since_follows_the_pending_approval() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.approvals.insert(
            root,
            (ApprovalId(uuid::Uuid::now_v7()), "bash".into(), 4_200),
        );
        let act = a.view().assemble_activity(&[root], &HashMap::new()).await;
        assert_eq!(
            act[&root].state,
            AgentState::Approval {
                tool: "bash".into()
            }
        );
        assert_eq!(act[&root].waiting_since_ms, Some(4_200));
        let v = serde_json::to_value(&act[&root]).unwrap();
        assert_eq!(v["waiting_since_ms"], 4_200);
        a.approvals.clear();
        let act = a.view().assemble_activity(&[root], &HashMap::new()).await;
        assert_eq!(
            act[&root].waiting_since_ms, None,
            "cleared when the state leaves"
        );
        let pre130: SessionActivity =
            serde_json::from_value(serde_json::json!({ "state": { "state": "idle" }, "last_active": null, "turn_started_at": null, "jobs": 0, "ptys": 0, "children": 0, "asks": null, "live": false })).unwrap();
        assert_eq!(
            pre130.waiting_since_ms, None,
            "an older engine's row still parses"
        );
    }

    /// WAKE-LIVE-CHILD D4: `ActivityOf` answers exactly the asked ids — a cold
    /// child included (its `last_active` from the store) — and nothing else.
    #[tokio::test]
    async fn activity_of_returns_rows_for_the_asked_ids() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        a.sessions.remove(&child);
        a.running.insert(root);
        let rows = a.view().activity_for(&[root, child]).await;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[&root].state, AgentState::Running);
        assert!(rows[&root].live && !rows[&child].live);
        assert!(
            rows[&child].last_active.is_some(),
            "cold: the store's updated_at"
        );
        assert!(a.view().activity_for(&[]).await.is_empty());
    }

    /// 62 §3.2: `ListActivity` lists every root, cold or not, and a child only
    /// while it has an actor or a job; a live child counts toward its root.
    #[tokio::test]
    async fn list_activity_keeps_cold_roots_and_drops_cold_children() {
        let (_scratch, mut a) = agents().await;
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

        let rows = a.view().activity_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        assert!(ids.contains(&root) && ids.contains(&cold_root));
        assert!(ids.contains(&live_child) && !ids.contains(&cold_child));
        let by_id: HashMap<SessionId, &AgentInfo> = rows.iter().map(|r| (r.id, r)).collect();
        assert_eq!((by_id[&root].depth, by_id[&root].parent), (0, root));
        assert_eq!(
            (by_id[&live_child].depth, by_id[&live_child].parent),
            (1, root)
        );
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
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        a.running.insert(child);
        let rows = Agents::spawn(a).ask(ListSessions).await.unwrap();
        let running = |id| rows.iter().find(|r| r.id == id).unwrap().running;
        assert!(running(child) && !running(root));
    }

    /// 176 E5: a read never holds the mailbox for its I/O. Every
    /// `session`/`list_sessions`/`descendants` read is stalled 1 s
    /// (`Store::set_read_delay`); while `ListActivity`, `ListAgents` or
    /// `LoadSession` is in flight, `GetCompactStyle` answers in < 100 ms and
    /// before it. Shown to fail without E1: with those three handlers made
    /// to await their reads inline again (the pre-176 shape), it failed at
    /// its first case — "ListActivity: GetCompactStyle took 953.556083ms".
    #[tokio::test]
    async fn trivial_ask_answers_while_a_read_is_in_flight() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.session(
            Some(SessionId::new()),
            Some((root, "kid")),
            None,
            (None, None),
        )
        .await
        .unwrap();
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        store.set_read_delay(Duration::from_secs(1));
        for which in ["ListActivity", "ListAgents", "LoadSession"] {
            let reader = {
                let agents = agents.clone();
                tokio::spawn(async move {
                    match which {
                        "ListActivity" => {
                            assert!(!agents.ask(ListActivity).await.unwrap().is_empty())
                        }
                        "ListAgents" => {
                            let rows = agents.ask(ListAgents { session: root }).await.unwrap();
                            assert_eq!(rows.len(), 1);
                        }
                        _ => {
                            let load = LoadSession {
                                session: root,
                                before: None,
                                limit: None,
                                settled_only: false,
                            };
                            agents.ask(load).await.unwrap();
                        }
                    }
                    Instant::now()
                })
            };
            // The read is in the mailbox (and, inline, would be mid-stall).
            tokio::time::sleep(Duration::from_millis(50)).await;
            let asked = Instant::now();
            agents.ask(GetCompactStyle).await.unwrap();
            let answered = Instant::now();
            let read_done = reader.await.unwrap();
            assert!(
                answered - asked < Duration::from_millis(100),
                "{which}: GetCompactStyle took {:?}",
                answered - asked
            );
            assert!(answered < read_done, "{which}: answered after the read");
            assert!(
                read_done - asked >= Duration::from_millis(900),
                "{which}: the read was not stalled"
            );
        }
    }

    /// 62 §0.2, §3.3: `Running` names roots only — a running child is never
    /// swept in; a dry run fills `before` and routes nothing.
    #[tokio::test]
    async fn broadcast_running_selects_roots_and_dry_run_sends_nothing() {
        let (_scratch, mut a) = agents().await;
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
        let (_scratch, mut a) = agents().await;
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
        let (_scratch, a) = agents().await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
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

    /// 212 §11: a harness rename (`RenameSession`) leaves the title guard alone; only a
    /// user's (`UserRenameSession`, the `rename_session` RPC) sets `user_titled`.
    #[tokio::test]
    async fn only_a_user_rename_guards_the_title() {
        let (_scratch, a) = agents().await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        agents
            .ask(RenameSession {
                session: id,
                title: "Harness title".into(),
            })
            .await
            .unwrap();
        assert!(!store.title_guard(id).await.unwrap().user_titled);
        agents
            .ask(UserRenameSession {
                session: id,
                title: " Mine ".into(),
            })
            .await
            .unwrap();
        assert!(store.title_guard(id).await.unwrap().user_titled);
        assert_eq!(store.session(id).await.unwrap().unwrap().title, "Mine");
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

    /// 137 §11.3: the Settings → Memory writer persists every key, updates the
    /// live config and refuses out-of-range values.
    #[tokio::test]
    async fn endless_settings_round_trip_and_validate() {
        let (_scratch, a) = agents_with(false).await;
        let dir = a.rt.data_dir.clone();
        let agents = Agents::spawn(a);
        let before = agents.ask(GetEndlessSettings).await.unwrap();
        assert_eq!(before.view_bytes, EndlessConfig::default().view_bytes);
        let mut want = before.clone();
        want.default_chat = true;
        want.view_bytes = 96_000;
        want.slack = 0.25;
        let got = agents
            .ask(SetEndlessSettings {
                config: want.clone(),
            })
            .await
            .unwrap();
        assert!(got.default_chat && got.view_bytes == 96_000);
        let live = agents.ask(GetEndlessSettings).await.unwrap();
        assert!(live.default_chat && live.view_bytes == 96_000 && live.slack == 0.25);
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        for needle in [
            "endless:",
            "default_chat: true",
            "view_bytes: 96000",
            "slack: 0.25",
            "meeting_view_bytes:",
            "backfill_max_units:",
            "model: null",
        ] {
            assert!(yaml.contains(needle), "{needle} in {yaml}");
        }
        // A new chat now starts endless by default.
        let open = agents
            .ask(CreateSession {
                options: crate::CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap();
        assert_eq!(
            agents
                .ask(SessionMemoryQuery {
                    session: open.session,
                })
                .await
                .unwrap()
                .mode,
            MemoryMode::Endless
        );
        let mut bad = want.clone();
        bad.view_bytes = 1_000;
        let e = agents
            .ask(SetEndlessSettings { config: bad })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("view_bytes"), "{e}");
    }

    /// 194 row 11: `agent.endless.effort` is written beside `agent.endless.model`
    /// (quoted), refused when the model does not take it (D4b), dropped when a
    /// new model does not (D4c), cleared with the model (D4d), and in force at
    /// once (`Endless::config`, D4f); `ValidateEffort` is the same check.
    #[tokio::test]
    async fn endless_settings_effort_follows_its_model() {
        let (_scratch, mut a) = agents_with(false).await;
        let dir = a.rt.data_dir.clone();
        let model = |id: &str, efforts: &[&str]| {
            serde_json::json!({
                "id": id, "name": id, "limit": { "context": 100_000, "output": 4_096 },
                "reasoning": !efforts.is_empty(),
                "reasoning_options": if efforts.is_empty() {
                    serde_json::json!([])
                } else {
                    serde_json::json!([{ "type": "effort", "values": efforts }])
                }
            })
        };
        let user = serde_json::json!({ "testfx": { "id": "testfx", "name": "FX", "models": {
            "think": model("think", &["low", "high"]),
            "flat": model("flat", &[]),
        }}});
        std::fs::write(dir.join("models.json"), user.to_string()).unwrap();
        a.catalog = Arc::new(Catalog::load(&dir));
        let endless = a.endless.clone();
        let agents = Agents::spawn(a);
        let base = agents.ask(GetEndlessSettings).await.unwrap();
        assert_eq!(base.effort, None);
        let set = |model: Option<&str>, effort: Option<&str>| SetEndlessSettings {
            config: EndlessConfig {
                model: model.map(str::to_owned),
                effort: effort.map(str::to_owned),
                ..base.clone()
            },
        };
        let got = agents
            .ask(set(Some("testfx/think"), Some("high")))
            .await
            .unwrap();
        assert_eq!(got.effort.as_deref(), Some("high"));
        assert_eq!(endless.config().effort.as_deref(), Some("high"));
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("effort: \"high\""), "{yaml}");
        // D4b: the same model, a level it does not take.
        let e = agents
            .ask(set(Some("testfx/think"), Some("max")))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("effort: think accepts off|low|high"), "{e}");
        assert_eq!(endless.config().effort.as_deref(), Some("high"));
        // D4c: a new model that takes no effort drops it, persisted.
        let got = agents
            .ask(set(Some("testfx/flat"), Some("high")))
            .await
            .unwrap();
        assert_eq!(
            (got.model.as_deref(), got.effort),
            (Some("testfx/flat"), None)
        );
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(yaml.contains("effort: null"), "{yaml}");
        // D4d: no model, no effort.
        let got = agents.ask(set(None, Some("low"))).await.unwrap();
        assert_eq!(got.effort, None);
        // The shared check, as meeting and dreaming ask it.
        assert!(
            agents
                .ask(ValidateEffort {
                    model: Some("testfx/think".into()),
                    effort: "low".into(),
                })
                .await
                .is_ok()
        );
        let e = agents
            .ask(ValidateEffort {
                model: None,
                effort: "loud".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown effort loud"), "{e}");
    }

    /// 183 §12.5 (Z live proof): a chat created in a folder that exists here
    /// works there — the tools' cwd and the prompt's "Working directory" —
    /// not in the engine's own cwd; a folder missing here keeps the default.
    #[tokio::test]
    async fn a_chat_created_in_a_folder_is_seated_there() {
        let (scratch, a) = agents_with(false).await;
        let agents = Agents::spawn(a);
        let create = |cwd: &Path| CreateSession {
            options: crate::CreateSessionOptions {
                cwd: Some(cwd.to_string_lossy().into_owned()),
                ..Default::default()
            },
            peer: None,
        };
        let here = scratch.path().join("proj");
        std::fs::create_dir_all(&here).unwrap();
        let s = agents.ask(create(&here)).await.unwrap().session;
        assert_eq!(tools::seat::cwd(s).unwrap(), here);
        let gone = agents
            .ask(create(&scratch.path().join("missing")))
            .await
            .unwrap()
            .session;
        assert_eq!(
            tools::seat::cwd(gone).unwrap(),
            std::env::current_dir().unwrap()
        );
    }

    /// The 1760 % desk: an endless session that never compacted priced its
    /// whole raw history as the frame. `load_session` and the breakdown now
    /// report the view the next turn opens with, not the transcript.
    #[tokio::test]
    async fn an_endless_session_reports_its_view_not_its_raw_history() {
        let (_scratch, a) = agents_with(false).await;
        let store = a.store.clone();
        let endless = a.endless.clone();
        let agents = Agents::spawn(a);
        let session = agents
            .ask(CreateSession {
                options: crate::CreateSessionOptions {
                    memory: Some(MemoryMode::Endless),
                    ..Default::default()
                },
                peer: None,
            })
            .await
            .unwrap()
            .session;
        agents
            .ask(EndlessPause {
                session,
                paused: true,
            })
            .await
            .unwrap();
        let mut raw = 0u32;
        for k in 0..60 {
            let m = ChatMessage::user(format!("row {k}: {}", "word ".repeat(4_000)));
            raw += crate::compact::est(&m);
            store.append(session, &m).await.unwrap();
        }
        // Mechanical lines for every unit, as the service's fold makes them
        // (no model; 201 P5: `/compact` in a chat is a trim now).
        endless.fold(session).await.unwrap();
        let view = endless_view_tokens(&endless, session).await.unwrap();
        assert!(view > 0 && view < raw / 2, "view {view} vs raw {raw}");
        let loaded = agents
            .ask(LoadSession {
                session,
                before: None,
                limit: None,
                settled_only: false,
            })
            .await
            .unwrap();
        assert_eq!(loaded.context_estimate, Some(view));
        let b = agents
            .ask(SessionContextBreakdown { session })
            .await
            .unwrap();
        assert_eq!(b.total_estimate, view);
    }

    /// 137G G9 → 201 P5: `/compact` in an endless chat is "take a trim now";
    /// with no tail to cut it answers one plain line, the epoch stays, and
    /// no summariser or compactor runs (settling is paused).
    #[tokio::test]
    async fn compact_in_an_endless_session_trims_without_a_model_call() {
        let (_scratch, a) = agents_with(false).await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let session = agents
            .ask(CreateSession {
                options: crate::CreateSessionOptions {
                    memory: Some(MemoryMode::Endless),
                    ..Default::default()
                },
                peer: None,
            })
            .await
            .unwrap()
            .session;
        let m = agents
            .ask(EndlessPause {
                session,
                paused: true,
            })
            .await
            .unwrap();
        assert_eq!(m.backlog.and_then(|b| b.paused_by).as_deref(), Some("user"));
        for k in 0..3 {
            store
                .append(
                    session,
                    // 201 P3: talk units (a long user message needs no model).
                    &ChatMessage::assistant(format!("long message {k}: {}", "x ".repeat(400))),
                )
                .await
                .unwrap();
        }
        let err = agents
            .ask(Compact {
                session,
                style: None,
                head: None,
            })
            .await
            .expect_err("nothing to cut")
            .to_string();
        assert!(err.contains("Nothing to cut"), "{err}");
        let (epoch, _, _) = store.frame(session).await.unwrap();
        assert_eq!(epoch, 0, "no epoch: nothing was summarised");
        let nodes = store.endless_nodes(session).await.unwrap();
        assert!(
            nodes.iter().all(|n| n.model.is_none()),
            "no model built a line"
        );
    }

    /// 137F: `endless_retry` answers like `session_memory` (refused for an
    /// unknown session, an epochs session unchanged); the liveness keys are
    /// range-checked and persisted.
    #[tokio::test]
    async fn endless_retry_and_liveness_settings() {
        let (_scratch, a) = agents_with(false).await;
        let dir = a.rt.data_dir.clone();
        let agents = Agents::spawn(a);
        let e = agents
            .ask(EndlessRetry {
                session: SessionId::new(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown session"), "{e}");
        let open = |memory| {
            agents.ask(CreateSession {
                options: crate::CreateSessionOptions {
                    memory,
                    ..Default::default()
                },
                peer: None,
            })
        };
        let plain = open(None).await.unwrap().session;
        let m = agents.ask(EndlessRetry { session: plain }).await.unwrap();
        assert_eq!(m.mode, MemoryMode::Epochs);
        assert!(m.view.is_empty() && m.stalled.is_none());
        let endless = open(Some(MemoryMode::Endless)).await.unwrap().session;
        let m = agents.ask(EndlessRetry { session: endless }).await.unwrap();
        assert_eq!(m.mode, MemoryMode::Endless);
        assert!(m.stalled.is_none());

        let mut c = agents.ask(GetEndlessSettings).await.unwrap();
        assert_eq!(
            (
                c.attempt_secs,
                c.local_attempt_secs,
                c.local_ctx_bytes,
                c.local_jobs
            ),
            (120, 600, 24_000, 1)
        );
        c.local_attempt_secs = 900;
        c.local_jobs = 2;
        let got = agents
            .ask(SetEndlessSettings { config: c.clone() })
            .await
            .unwrap();
        assert_eq!((got.local_attempt_secs, got.local_jobs), (900, 2));
        let yaml = std::fs::read_to_string(dir.join("config.yaml")).unwrap();
        for needle in [
            "attempt_secs: 120",
            "local_attempt_secs: 900",
            "local_ctx_bytes: 24000",
            "local_jobs: 2",
        ] {
            assert!(yaml.contains(needle), "{needle} in {yaml}");
        }
        for (key, bad) in [
            (
                "attempt_secs",
                EndlessConfig {
                    attempt_secs: 5,
                    ..c.clone()
                },
            ),
            (
                "local_attempt_secs",
                EndlessConfig {
                    local_attempt_secs: 8_000,
                    ..c.clone()
                },
            ),
            (
                "local_ctx_bytes",
                EndlessConfig {
                    local_ctx_bytes: 100,
                    ..c.clone()
                },
            ),
            (
                "local_jobs",
                EndlessConfig {
                    local_jobs: 0,
                    ..c.clone()
                },
            ),
        ] {
            let e = agents
                .ask(SetEndlessSettings { config: bad })
                .await
                .unwrap_err()
                .to_string();
            assert!(e.contains(key), "{key}: {e}");
        }
    }

    /// 137 §11.2 / §14: `session_memory` of an epochs session is `mode:
    /// epochs` with an empty view; `create_session.options.memory` lands in
    /// the column; `/endless` flips an idle session both ways; `view_bytes`
    /// is range-checked. The tree itself is the service's (W1).
    #[tokio::test]
    async fn memory_mode_is_created_queried_and_switched() {
        let (_scratch, a) = agents_with(false).await;
        let agents = Agents::spawn(a);
        let open = |memory| {
            agents.ask(CreateSession {
                options: crate::CreateSessionOptions {
                    memory,
                    ..Default::default()
                },
                peer: None,
            })
        };
        let plain = open(None).await.unwrap().session;
        let m = agents
            .ask(SessionMemoryQuery { session: plain })
            .await
            .unwrap();
        assert_eq!(m.mode, MemoryMode::Epochs);
        assert!(m.view.is_empty() && m.backfill.is_none());
        assert_eq!(m.stats, crate::EndlessStats::default());

        let endless = open(Some(MemoryMode::Endless)).await.unwrap().session;
        let m = agents
            .ask(SessionMemoryQuery { session: endless })
            .await
            .unwrap();
        assert_eq!(m.mode, MemoryMode::Endless);
        // Idle: `off` then `on` again, each a store write the query reflects.
        let m = agents
            .ask(SetSessionMemory {
                session: endless,
                mode: MemoryMode::Epochs,
                filter: UnitFilter::All,
            })
            .await
            .unwrap();
        assert_eq!(m.memory.mode, MemoryMode::Epochs);
        // R1-D6: a meeting-filtered switch is the same store write.
        let m = agents
            .ask(SetSessionMemory {
                session: plain,
                mode: MemoryMode::Endless,
                filter: UnitFilter::Meeting,
            })
            .await
            .unwrap();
        assert_eq!(m.memory.mode, MemoryMode::Endless);
        // RenderMemoryView: lines within the budget (an empty tree renders none).
        assert!(
            agents
                .ask(RenderMemoryView {
                    session: plain,
                    bytes: 32_000,
                })
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            agents
                .ask(SessionMemoryQuery { session: plain })
                .await
                .unwrap()
                .mode,
            MemoryMode::Endless
        );
        // R1-D2b: the view budget is range-checked.
        let e = agents
            .ask(SetSessionViewBytes {
                session: plain,
                bytes: 1_000,
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("out of range"), "{e}");
        assert_eq!(
            agents
                .ask(SetSessionViewBytes {
                    session: plain,
                    bytes: 128_000,
                })
                .await
                .unwrap()
                .mode,
            MemoryMode::Endless
        );
        // An unknown session is refused, not created.
        assert!(
            agents
                .ask(SessionMemoryQuery {
                    session: SessionId::new(),
                })
                .await
                .is_err()
        );
    }

    /// 67 §8: a root opened for a remote parent carries its `tools` ceiling
    /// and effort like a local subagent; a plain create carries neither.
    #[tokio::test]
    async fn remote_root_gets_cap_and_effort() {
        let (_scratch, mut a) = agents_with(true).await;
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
        assert_eq!(
            (p.address.as_str(), p.kind, p.reply),
            ("parent", PeerKind::Parent, true)
        );
        assert!(
            p.name.contains("parent-1") && p.name.contains("Ab3dEf9h"),
            "{}",
            p.name
        );
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
        assert_eq!(
            (d.receipt.to.as_str(), d.receipt.state),
            ("parent", PeerMessageState::Accepted)
        );
        assert!(d.reply.is_none());
        assert!(
            a.uplink(&deliver(true), "done".into())
                .await
                .unwrap_err()
                .contains("await_reply")
        );

        let plain = a
            .create_durable(None, None, None, None, None, None)
            .await
            .unwrap();
        assert!(!a.caps.contains_key(&plain));
        let row = a.store.session(plain).await.unwrap().unwrap();
        assert_eq!(row.effort, a.default_effort);
        assert!(a.resolve_all(plain, &["parent".to_owned()]).await[0].is_err());
        assert!(
            a.directory(plain)
                .await
                .unwrap()
                .iter()
                .all(|p| p.address != "parent")
        );
    }

    /// 51 §4.1–§4.2: a private root needs a view and an owning peer; once
    /// open it authorizes only that peer, is absent from the durable list
    /// and present in `PrivateTrees`, and its state reads through its own store.
    #[tokio::test]
    async fn private_root_is_owned_and_unlisted() {
        let (_scratch, a) = agents().await;
        let agents = Agents::spawn(a);
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
        assert!(
            !listed.iter().any(|s| s.id == open.root),
            "private root listed"
        );
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
        let (_scratch, a) = agents().await;
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
        assert_eq!(
            agents.ask(PrivateTrees).await.unwrap(),
            vec![(root, root, 7)]
        );
        assert_eq!(refused(leave(7).await), "session closed");
        // Input to the closed id is refused (the caller learns it), not turned
        // into a durable row.
        let e = refused(
            agents
                .ask(SendInput {
                    channel: None,
                    session: Some(root),
                    text: "hello".into(),
                    files: Vec::new(),
                    delivery: Delivery::default(),
                    id: None,
                })
                .await,
        );
        assert!(e.contains("session closed"), "{e}");
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
        let (_scratch, a) = agents().await;
        let agents = Agents::spawn(a);
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
        assert_eq!(
            agents.ask(PrivateTrees).await.unwrap(),
            vec![(root, root, 9)]
        );

        // The old connection dies well past the (200 ms) grace: the root stays.
        old_closed.cancel();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            agents.ask(PrivateTrees).await.unwrap(),
            vec![(root, root, 9)]
        );
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
        _scratch: crate::TestDir,
    }

    async fn seed(dir: &Path, as_engine: &str, id: SessionId) {
        let s = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId(as_engine.into())),
        )
        .await
        .unwrap();
        s.create_session(id, "m", None, None, None).await.unwrap();
        s.set_root_scope(id, PeerScope::Owner, None).await.unwrap();
        s.close().await;
    }

    async fn two_engines() -> TwoEngines {
        two_engines_as("a", "b").await
    }

    /// [`two_engines`] under other engine ids (a test that sets the process-wide
    /// `mesh.messaging_off` must not touch the others' `a` and `b`).
    async fn two_engines_as(na: &str, nb: &str) -> TwoEngines {
        let scratch = crate::test_dir("69p");
        let (dir_a, dir_b) = (scratch.path().join("a"), scratch.path().join("b"));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let (a_root, b_root) = (SessionId::new(), SessionId::new());
        for dir in [&dir_a, &dir_b] {
            seed(dir, na, a_root).await;
            seed(dir, nb, b_root).await;
        }
        let net = peer_mesh::loopback::nodes(&[na, nb]);
        let mut a = agents_on(dir_a, Some(na), true).await;
        let mut b = agents_on(dir_b, Some(nb), true).await;
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
            _scratch: scratch,
        }
    }

    /// The owner side of `theuth/rpc/1` for one engine, as `rpc::forward::serve`
    /// and `api::peer_deliver` do it: one request line per stream, dispatched
    /// into the supervisor's [`Agents::peer_deliver`], one response line.
    fn serve_peer_deliver(
        t: Arc<peer_mesh::loopback::Loop>,
        agents: Arc<tokio::sync::Mutex<Agents>>,
    ) {
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
                    let session: SessionId =
                        serde_json::from_value(req["params"][0].clone()).unwrap();
                    let envelope: PeerEnvelope =
                        serde_json::from_value(req["params"][1].clone()).unwrap();
                    let rp = match agents
                        .lock()
                        .await
                        .peer_deliver(PeerDeliver {
                            session,
                            envelope,
                            forwarder: None,
                        })
                        .await
                    {
                        Ok(r) => serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": r}),
                        Err(e) => {
                            serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": e.to_string()}})
                        }
                    };
                    write.write_all(format!("{rp}\n").as_bytes()).await.unwrap();
                    write.flush().await.unwrap();
                });
            }
        });
    }

    fn deliver(
        from: SessionId,
        to: SessionId,
        text: &str,
        await_reply: bool,
        reply_to: Option<Uuid>,
    ) -> Deliver {
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
            _scratch,
            a,
            b,
            a_root,
            b_root,
            net,
        } = two_engines().await;
        let (a, b) = (
            Arc::new(tokio::sync::Mutex::new(a)),
            Arc::new(tokio::sync::Mutex::new(b)),
        );
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
            assert!(
                !a.enqueued.contains(&id),
                "nothing here consumes a forwarded message"
            );
            assert!(
                !a.sessions.contains_key(&b_root),
                "the remote row was not woken here"
            );
            assert!(
                a.waits.contains_key(&id),
                "the wait is registered as for a local recipient"
            );
            assert_eq!(
                a.store.exchange_receipt(id).await.unwrap().unwrap().state,
                PeerMessageState::Accepted
            );
        }
        // The owner's intake: same id, reversed view, its own row.
        let theirs = b
            .lock()
            .await
            .store
            .peer_record(b_root, id)
            .await
            .unwrap()
            .unwrap();
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
        assert_eq!(
            (r.receipt.state, r.receipt.reply_to),
            (PeerMessageState::Accepted, Some(id))
        );
        let fin = d.reply.unwrap().await.unwrap();
        assert_eq!((fin.id, fin.state), (id, PeerMessageState::Replied));
        assert_eq!(fin.reply.as_deref(), Some("42"));
        assert_eq!(
            fin.answered_by.as_deref(),
            Some(format!("session:{b_root}").as_str())
        );
        assert!(a.lock().await.waits.is_empty());
    }

    /// No owner on the wire: the receipt is `failed` with 68 F's error data
    /// shape (the words the desktop card shows), durably; no wait is kept.
    #[tokio::test]
    async fn an_unreachable_owner_is_a_failed_receipt_in_the_front_door_shape() {
        let TwoEngines {
            _scratch,
            mut a,
            a_root,
            b_root,
            ..
        } = two_engines().await;
        // A switch on which `b` is not connected.
        a.transport = Some(peer_mesh::loopback::nodes(&["a"]).remove(0));
        let d = a
            .deliver(deliver(a_root, b_root, "hello?", true, None), None)
            .await
            .unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Failed);
        assert_eq!(
            d.receipt.error.as_deref(),
            Some(r#"{"OwnerUnreachable":{"owner":"b","last_seen":null}}"#)
        );
        assert!(d.reply.is_none());
        assert!(a.waits.is_empty());
        assert_eq!(
            a.store
                .exchange_receipt(d.receipt.id)
                .await
                .unwrap()
                .unwrap()
                .state,
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
                    chain: 0,
                },
                forwarder: None,
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("owned by b"), "{e}");
    }

    /// `discover_peers`: a row owned elsewhere is `remote` with its owner's
    /// engine name and no activity; ours carry this engine's name.
    /// `active_only` drops remote rows (their activity is unknown here).
    #[tokio::test]
    async fn directory_marks_rows_owned_elsewhere_as_remote() {
        let TwoEngines {
            _scratch,
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
        assert_eq!(
            (r.state, r.relationship),
            (PeerState::Remote, PeerRelationship::Root)
        );
        assert_eq!(
            r.engine.as_deref(),
            Some("b"),
            "no mesh actor: the id itself (short)"
        );
        assert!(r.activity.is_none());
        assert_eq!(
            r.reachable,
            Some(false),
            "183 §6.6: a remote row says whether its owner is connected"
        );
        arrange(&mut rows, true, false);
        assert!(
            rows.is_empty(),
            "remote activity is unknown: active_only keeps none"
        );
        // Seen from b, a's root is the remote one.
        let rows = b.directory(b_root).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, format!("session:{a_root}"));
        assert_eq!(
            (rows[0].state, rows[0].engine.as_deref()),
            (PeerState::Remote, Some("a"))
        );
        assert_eq!(a.directory(a_root).await.unwrap()[0].reachable, Some(false));
    }

    /// 183 §6.6: `discover_peers(query)` also matches a row's tags and its engine.
    #[test]
    fn a_discover_query_matches_tags_and_engine() {
        let row = PeerInfo {
            address: "session:0190".into(),
            name: "deadbeef".into(),
            kind: PeerKind::Native,
            relationship: PeerRelationship::Root,
            state: PeerState::Remote,
            send: true,
            reply: true,
            stream: false,
            activity: None,
            title: None,
            tags: vec!["Release-Notes".into()],
            engine: Some("Xeus".into()),
            reachable: Some(true),
        };
        for q in ["dead", "0190", "release", "xeus"] {
            assert!(peer_matches(&row, q), "{q}");
        }
        assert!(!peer_matches(&row, "cloud"));
    }

    fn envelope_from(from: SessionId) -> PeerEnvelope {
        PeerEnvelope {
            id: Uuid::now_v7(),
            from,
            text: "hi".into(),
            reply_to: None,
            request_id: None,
            chain: 0,
        }
    }

    /// 183 §6.1 (D3, D6): with `agent.communication` absent the ceiling is on while this
    /// engine has a paired machine (source `mesh`) and off without one; an explicit value is
    /// never moved by the paired set.
    #[tokio::test]
    async fn absent_communication_is_on_with_a_paired_machine_and_explicit_false_wins() {
        let scratch = crate::test_dir("183md");
        let mut a = agents_on(scratch.path().to_path_buf(), None, false).await;
        a.communication.0.write().set_global(false, "default");
        let state = |a: &Agents| {
            let s = a.communication.0.read().state(None).unwrap();
            (s.global, s.source)
        };
        a.mesh_default(None).await;
        assert_eq!(
            state(&a),
            (false, "default".into()),
            "no mesh yet: unchanged"
        );
        a.mesh_default(Some(1)).await;
        assert_eq!(state(&a), (true, "mesh".into()));
        a.mesh_default(Some(0)).await;
        assert_eq!(
            state(&a),
            (false, "default".into()),
            "the last peer went: off"
        );
        for source in ["config", "env", "command"] {
            a.communication.0.write().set_global(false, source);
            a.mesh_default(Some(2)).await;
            assert_eq!(state(&a), (false, source.into()), "explicit false wins");
        }
    }

    /// 183 Z3-R2: with `agent.communication` absent, a lone engine (no paired machine) with
    /// a contact has messaging on; it goes off with its last contact.
    #[tokio::test]
    async fn absent_communication_counts_contacts_on_a_lone_engine() {
        let scratch = crate::test_dir("183mdc");
        let mut a = agents_on(scratch.path().to_path_buf(), None, false).await;
        a.communication.0.write().set_global(false, "default");
        let on = |a: &Agents| a.communication.0.read().state(None).unwrap().global;
        a.mesh_default(Some(0)).await;
        assert!(!on(&a), "no machine, no contact: off");
        a.network.insert_contact_for_test("bob");
        a.mesh_default(None).await;
        assert!(on(&a), "a contact turns the default on");
        a.network.clear_peers_for_test();
        a.mesh_default(None).await;
        assert!(!on(&a), "the last contact went: off");
    }

    /// 183 §6.2 (W-14), §11.4: `mesh.messaging_off` refuses both ways, naming the machines:
    /// the sender before anything is forwarded, the receiver at its intake.
    #[tokio::test]
    async fn the_per_machine_switch_refuses_both_ways_and_names_the_machine() {
        let TwoEngines {
            _scratch,
            mut a,
            b,
            a_root,
            b_root,
            net,
        } = two_engines_as("deny-a", "deny-b").await;
        let b = Arc::new(tokio::sync::Mutex::new(b));
        serve_peer_deliver(net[1].clone(), b.clone());
        // One process holds both "engines": a lists b, b lists a.
        mesh::set_messaging_off(vec!["deny-b".into(), "deny-a".into()]);
        // This engine's own name is the process-wide mesh name another test may set meanwhile.
        let disabled = tools::communication::DISABLED;
        let e = a
            .deliver(deliver(a_root, b_root, "hi", false, None), None)
            .await
            .unwrap_err();
        assert!(
            e.starts_with(&format!(
                "denied: send_message: {disabled}: agent messaging between "
            )) && e.contains(" and deny-b is off on "),
            "{e}"
        );
        let e = b
            .lock()
            .await
            .peer_deliver(PeerDeliver {
                session: b_root,
                envelope: envelope_from(a_root),
                forwarder: Some("deny-a".into()),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.starts_with(&format!(
                "send_message: {disabled}: agent messaging between deny-a and "
            )) && e.contains(" is off on "),
            "{e}"
        );
        mesh::set_messaging_off(Vec::new());
    }

    /// 183 §6.5: a forwarded `peer_deliver` is taken only from the engine that owns its
    /// `from`; any other paired engine is refused, naming it.
    #[tokio::test]
    async fn a_forwarder_that_does_not_own_the_sender_is_refused() {
        let TwoEngines {
            _scratch,
            mut b,
            a_root,
            b_root,
            ..
        } = two_engines().await;
        let env = envelope_from(a_root);
        let e = b
            .peer_deliver(PeerDeliver {
                session: b_root,
                envelope: env.clone(),
                forwarder: Some("c".into()),
            })
            .await
            .unwrap_err();
        // Typed, so the RPC layer answers `UNAUTHORIZED` without matching the text.
        assert_eq!(
            e,
            PeerDeliverError::NotOwner(MeshRefusal::NotOwner {
                engine: "c".into(),
                from: a_root,
            })
        );
        assert_eq!(
            e.to_string(),
            format!("peer_deliver: c does not own session:{a_root}")
        );
        let ok = b
            .peer_deliver(PeerDeliver {
                session: b_root,
                envelope: env,
                forwarder: Some("a".into()),
            })
            .await
            .unwrap();
        assert_eq!(ok.state, PeerMessageState::Accepted);
    }

    /// 183 §6.5: the chain count crosses the forward: a sender at the bound is refused by the
    /// owner's intake, which used to count a remote sender as 0.
    #[tokio::test]
    async fn the_chain_is_inherited_across_a_forward() {
        let TwoEngines {
            _scratch,
            mut a,
            b,
            a_root,
            b_root,
            net,
        } = two_engines().await;
        serve_peer_deliver(net[1].clone(), Arc::new(tokio::sync::Mutex::new(b)));
        a.ledger.register(a_root, None, false);
        for _ in 0..crate::communication::CHAIN_MAX {
            let t = a.ledger.admit(a_root, InputOrigin::Automatic, None);
            a.ledger.consumed(InputConsumed {
                session: a_root,
                id: t.id,
                turn: TurnId::new(),
            });
        }
        let d = a
            .deliver(deliver(a_root, b_root, "again", false, None), None)
            .await
            .unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Failed);
        assert!(
            d.receipt
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("chain limit")),
            "{:?}",
            d.receipt.error
        );
    }

    /// 183 §6.4 (W-15): an offline owner still fails at once; a retry with the same
    /// `request_id` once it is back forwards the same message again.
    #[tokio::test]
    async fn a_same_request_id_retry_re_forwards_an_unreachable_message() {
        let TwoEngines {
            _scratch,
            mut a,
            b,
            a_root,
            b_root,
            net,
        } = two_engines().await;
        let online = a.transport.take();
        a.transport = Some(peer_mesh::loopback::nodes(&["a"]).remove(0));
        let mut m = deliver(a_root, b_root, "are you there?", false, None);
        m.request_id = Some("r-183".into());
        let first = a.deliver(m.clone(), None).await.unwrap();
        assert_eq!(first.receipt.state, PeerMessageState::Failed);
        assert!(
            first
                .receipt
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(peer_mesh::UNREACHABLE_PREFIX))
        );
        let b = Arc::new(tokio::sync::Mutex::new(b));
        serve_peer_deliver(net[1].clone(), b.clone());
        a.transport = online;
        let again = a.deliver(m, None).await.unwrap();
        assert_eq!(
            (again.receipt.id, again.receipt.state),
            (first.receipt.id, PeerMessageState::Accepted)
        );
        let theirs = b
            .lock()
            .await
            .store
            .peer_record(b_root, first.receipt.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(theirs.text, "are you there?");
    }

    /// The owner side of `theuth/rpc/1` through the supervisor actor's mailbox, as
    /// `api::peer_deliver` asks it.
    fn serve_through_mailbox(t: Arc<peer_mesh::loopback::Loop>, agents: ActorRef<Agents>) {
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
                    let deliver = PeerDeliver {
                        session: serde_json::from_value(req["params"][0].clone()).unwrap(),
                        envelope: serde_json::from_value(req["params"][1].clone()).unwrap(),
                        forwarder: None,
                    };
                    let rp = match agents.ask(deliver).await {
                        Ok(r) => serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": r}),
                        Err(e) => {
                            serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": e.to_string()}})
                        }
                    };
                    write.write_all(format!("{rp}\n").as_bytes()).await.unwrap();
                    write.flush().await.unwrap();
                });
            }
        });
    }

    /// 183 §6.3, §13 row 25: two engines send to each other at once while B's owner side is
    /// blocked. A's forward to B waits on the network, not in A's mailbox: A's other work
    /// answers within 100 ms and B's message to A is taken in.
    #[tokio::test]
    async fn a_blocked_owner_does_not_stall_the_senders_mailbox() {
        let TwoEngines {
            _scratch,
            a,
            mut b,
            a_root,
            b_root,
            net,
        } = two_engines().await;
        // B never answers a forward: every stream to it is held open.
        let held = net[1].clone();
        tokio::spawn(async move {
            use replica::Transport;
            let mut keep = Vec::new();
            while let Ok((_, s)) = held.accept(replica::ALPN_RPC).await {
                keep.push(s);
            }
        });
        let a_ref = Agents::spawn(a);
        serve_through_mailbox(net[0].clone(), a_ref.clone());
        let to_b = {
            let a = a_ref.clone();
            tokio::spawn(async move { a.ask(deliver(a_root, b_root, "to b", true, None)).await })
        };
        // A's Deliver is in its mailbox first.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = Instant::now();
        a_ref.ask(GetCommunication { session: None }).await.unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "A's mailbox answered in {:?}",
            start.elapsed()
        );
        let back = tokio::time::timeout(
            Duration::from_secs(5),
            b.deliver(deliver(b_root, a_root, "to a", false, None), None),
        )
        .await
        .expect("B's send to A is not held behind A's forward")
        .unwrap();
        assert_eq!(back.receipt.state, PeerMessageState::Accepted);
        assert!(!to_b.is_finished(), "A's own forward still waits on B");
        to_b.abort();
    }

    /// 183 §6.3 (S5): a retry with the same `request_id` while the first network leg is in
    /// flight attaches to it — no second forward — and learns how that leg settled.
    #[tokio::test]
    async fn a_same_request_id_retry_during_the_forward_learns_its_outcome() {
        let TwoEngines {
            _scratch,
            a,
            a_root,
            b_root,
            net,
            ..
        } = two_engines().await;
        // B holds every stream open until released, then drops them: the leg fails.
        let release = Arc::new(tokio::sync::Notify::new());
        let (held, drop_them) = (net[1].clone(), release.clone());
        tokio::spawn(async move {
            use replica::Transport;
            let mut keep = Vec::new();
            loop {
                tokio::select! {
                    Ok((_, s)) = held.accept(replica::ALPN_RPC) => keep.push(s),
                    () = drop_them.notified() => break,
                }
            }
        });
        let a_ref = Agents::spawn(a);
        let mut m = deliver(a_root, b_root, "to b", false, None);
        m.request_id = Some("r-s5".into());
        let first = {
            let (a, m) = (a_ref.clone(), m.clone());
            tokio::spawn(async move { a.ask(m).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        let retry = {
            let a = a_ref.clone();
            tokio::spawn(async move { a.ask(m).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !first.is_finished() && !retry.is_finished(),
            "both wait on the leg"
        );
        release.notify_one();
        let wait = Duration::from_secs(5);
        let first = tokio::time::timeout(wait, first)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let retry = tokio::time::timeout(wait, retry)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first.receipt.state, PeerMessageState::Failed);
        assert_eq!(
            (retry.receipt.id, retry.receipt.state),
            (first.receipt.id, PeerMessageState::Failed)
        );
    }

    /// 183 §6.6: a desk's `<agent>@<engine>` alias resolves from another engine, and a scope
    /// or alias the owner changed (a replicated row) reaches this engine's policy.
    #[tokio::test]
    async fn desk_aliases_resolve_and_replicated_scope_changes_reach_the_policy() {
        let TwoEngines {
            _scratch,
            mut a,
            a_root,
            b_root,
            ..
        } = two_engines().await;
        // Any member writes an alias: a newer root engine `c` minted as `ops@b` is not b's desk.
        // (Written and closed first: two open handles on one file keep separate log counters.)
        let as_c = Store::open(
            &_scratch.path().join("a").join("sessions.sqlite"),
            Some(mesh::EngineId("c".into())),
        )
        .await
        .unwrap();
        let c_root = SessionId::new();
        as_c.create_session(c_root, "m", None, None, None)
            .await
            .unwrap();
        as_c.set_root_scope(c_root, PeerScope::Owner, Some("ops@b"))
            .await
            .unwrap();
        as_c.close().await;
        // B named its desk; the row replicated here (written as `b`, which owns it).
        let as_b = Store::open(
            &_scratch.path().join("a").join("sessions.sqlite"),
            Some(mesh::EngineId("b".into())),
        )
        .await
        .unwrap();
        as_b.set_root_scope(b_root, PeerScope::Owner, Some("ops@b"))
            .await
            .unwrap();
        assert!(matches!(
            a.resolve(a_root, "ops@b").await,
            Ok(Target::Local(id)) if id == b_root
        ));
        assert_eq!(
            a.communication
                .0
                .read()
                .state(Some(b_root))
                .unwrap()
                .alias
                .as_deref(),
            Some("ops@b")
        );
        assert!(a.resolve(a_root, "nobody@b").await.is_err());
        // B flips its desk to isolated: once mirrored, routes from A are gone.
        as_b.set_root_scope(b_root, PeerScope::Isolated, Some("ops@b"))
            .await
            .unwrap();
        as_b.close().await;
        a.mirror(b_root).await;
        assert_eq!(
            a.communication.0.read().state(Some(b_root)).unwrap().scope,
            PeerScope::Isolated
        );
        assert!(a.communication.0.write().route(a_root, b_root).is_err());
        assert!(
            a.resolve(a_root, "ops@b").await.is_err(),
            "owner scope only"
        );
    }

    /// 136 R-22: a cap set through `SetBudget` is persisted, stamped `since`,
    /// and reloaded onto the actor a restart rebuilds from the store — so a
    /// durability-resumed turn is still metered before its first call.
    #[tokio::test]
    async fn budget_survives_a_rebuild_from_the_store() {
        let (_scratch, mut a) = agents().await;
        let store = a.store.clone();
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let agents = Agents::spawn(a);
        agents
            .ask(crate::SetBudget {
                session: root,
                nanos: Some(1_000_000_000),
                currency: "USD".into(),
            })
            .await
            .unwrap();
        let saved = store.budget(root).await.unwrap().expect("persisted");
        assert_eq!(
            (saved.nanos, saved.currency.as_str()),
            (1_000_000_000, "USD")
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&saved.since).is_ok(),
            "since is stamped server-side: {}",
            saved.since
        );
        // The process "restarts": the live actor is gone, the row remains.
        assert!(agents.ask(UnloadSession { session: root }).await.unwrap());
        // Any route rebuilds the actor lazily; the cap rides along.
        agents
            .ask(PinTitle {
                session: root,
                pinned: false,
            })
            .await
            .unwrap();
        let actor = agents
            .ask(With(move |a: &mut Agents| {
                a.sessions[&root].actor.native().unwrap().clone()
            }))
            .await
            .unwrap();
        assert_eq!(
            actor.ask(crate::loop_::BudgetOf).await.unwrap(),
            Some(saved),
            "the rebuilt actor is metered before its first call"
        );
    }

    /// A mesh of one: every row is ours, so `Deliver` takes today's local
    /// path — the exchange is enqueued here, no transport is consulted — and
    /// the directory carries this engine's name on every row.
    #[tokio::test]
    async fn a_lone_engine_delivers_locally_as_before() {
        let (_scratch, mut a) = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = SessionId::new();
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        assert_eq!(a.remote_owner(child).await, None);
        let d = a
            .deliver(deliver(root, child, "hi", false, None), None)
            .await
            .unwrap();
        assert_eq!(d.receipt.state, PeerMessageState::Accepted);
        assert!(
            a.enqueued.contains(&d.receipt.id),
            "queued for the local recipient"
        );
        assert!(a.transport.is_none(), "no mesh was consulted");
        let rows = a.directory(root).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_ne!(rows[0].state, PeerState::Remote);
        assert_eq!(
            rows[0].engine.as_deref(),
            Some(platform::process::hostname())
        );
    }

    /// A dialer that never connects: the actor it backs stays alive with
    /// `dead` set, the shape of a child whose harness is gone.
    #[derive(Debug)]
    struct Offline;

    #[async_trait::async_trait]
    impl acp::Dialer for Offline {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            Err("offline".to_owned())
        }
    }

    /// 75 §10: `resume_child` is refused with a clear error for a native
    /// session, for an ACP row whose harness is not in `agents.yaml` (a
    /// node's session), for one with no stored harness session id, and (141
    /// §5.4) for a watch-only row with the watch-only text.
    #[tokio::test]
    async fn resume_child_refuses_native_node_and_unstored() {
        let (_scratch, a) = agents().await;
        let store = a.store.clone();
        let agents = Agents::spawn(a);
        let native = SessionId::new();
        store
            .create_session(native, "m", None, None, None)
            .await
            .unwrap();
        assert_eq!(
            refused(agents.ask(ResumeChild { session: native }).await),
            "not an external harness; only an ACP child can be resumed"
        );
        let remote = SessionId::new();
        store
            .create_session(remote, "omp@box", None, None, None)
            .await
            .unwrap();
        store.set_acp(remote, "omp@box", None, &[]).await.unwrap();
        assert!(
            refused(agents.ask(ResumeChild { session: remote }).await)
                .contains("cannot be resumed here")
        );
        let local = SessionId::new();
        store
            .create_session(local, "omp", None, None, None)
            .await
            .unwrap();
        store.set_acp(local, "omp", None, &[]).await.unwrap();
        assert!(
            refused(agents.ask(ResumeChild { session: local }).await)
                .contains("cannot be resumed here"),
            "no agents.yaml entry"
        );
        let mut up = crate::store::ObservedUpsert::new(local, crate::ObservedSource::OmpTask, "A");
        up.state = Some(ObservedState::Running);
        let (watched, _) = store.upsert_observed(up).await.unwrap();
        assert_eq!(
            refused(agents.ask(ResumeChild { session: watched }).await),
            OBSERVED
        );
    }

    /// 75 §10: a resumed child gets exactly one actor — the live one (a
    /// harness that stayed up after its cancel, 06 §8.3) is stopped before
    /// the respawn from the row, and a row without a stored ACP session id
    /// is refused before anything is stopped. A stored skill name that no
    /// longer exists does not block the resume (75 §4).
    #[tokio::test]
    async fn resume_child_replaces_the_live_actor() {
        let (_scratch, mut a) = agents().await;
        let store = a.store.clone();
        let dir = a.rt.data_dir.clone();
        acp::save(
            &dir,
            &serde_yaml::from_str("omp:\n  command: /nonexistent/theuth-test-harness\n").unwrap(),
        )
        .unwrap();
        let child = SessionId::new();
        store
            .create_session(child, "omp", None, None, None)
            .await
            .unwrap();
        store
            .set_acp(child, "omp", Some(&dir), &["nope".into()])
            .await
            .unwrap();
        let old = AcpChild::spawn(AcpChild::new(
            child,
            "omp".into(),
            acp::Transport::Dial(Arc::new(Offline)),
            dir.clone(),
            None,
            None,
            None,
            None,
            None,
            Duration::from_secs(60),
            store.clone(),
            a.rt.clone(),
            Default::default(),
            Vec::new(),
        ));
        a.sessions.insert(
            child,
            Live {
                actor: LiveActor::Acp(old.clone()),
                parent: None,
            },
        );
        let agents = Agents::spawn(a);

        assert_eq!(
            refused(agents.ask(ResumeChild { session: child }).await),
            "omp: no harness session id stored; nothing to resume"
        );
        assert!(old.is_alive(), "refused before the live actor is touched");

        store.set_acp_session(child, "s1").await.unwrap();
        // The stored skill `nope` is unknown here: dropped with a warning
        // and a note to the child; the respawn proceeds.
        agents.ask(ResumeChild { session: child }).await.unwrap();
        assert!(!old.is_alive(), "the previous actor is stopped");
        assert!(
            agents.ask(UnloadSession { session: child }).await.unwrap(),
            "exactly one new live entry for the session"
        );
    }

    /// 146 D8: `resume_child` on a child that died mid-prompt (last row a
    /// prompt / dangling `ToolCall`) takes the boot respawn's path — the new
    /// actor is told `Respawn`, so its first turn is the continue prompt (here
    /// the harness binary is missing, so that turn is the `fail` turn whose
    /// `TurnStarted` text is the respawn marker) — and answers `Prompted`; an
    /// idle child (last row assistant text) is only reconnected (`Idle`, no
    /// turn); a child whose last turn was cancelled is `Idle` too, whatever
    /// its last row.
    #[tokio::test]
    async fn resume_child_prompts_a_midprompt_child() {
        let (_scratch, mut a) = agents().await;
        let store = a.store.clone();
        let dir = a.rt.data_dir.clone();
        acp::save(
            &dir,
            &serde_yaml::from_str("omp:\n  command: /nonexistent/theuth-test-harness\n").unwrap(),
        )
        .unwrap();
        let (mid, idle, cancelled) = (SessionId::new(), SessionId::new(), SessionId::new());
        for id in [mid, idle, cancelled] {
            store
                .create_session(id, "omp", None, None, None)
                .await
                .unwrap();
            store.set_acp(id, "omp", Some(&dir), &[]).await.unwrap();
            store.set_acp_session(id, "s1").await.unwrap();
            store
                .append(id, &genai::chat::ChatMessage::user("go"))
                .await
                .unwrap();
        }
        store
            .append(idle, &genai::chat::ChatMessage::assistant("done"))
            .await
            .unwrap();
        a.last_stop
            .insert(cancelled, ("cancelled".into(), chrono::Utc::now()));
        let mut events = tap(&a.rt).await;
        let agents = Agents::spawn(a);

        assert_eq!(
            agents.ask(ResumeChild { session: mid }).await.unwrap(),
            ChildResumed::Prompted
        );
        let started = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(AgentEvent::TurnStarted { session, text, .. }) = events.recv().await
                    && session == mid
                {
                    break text;
                }
            }
        })
        .await
        .expect("the respawn prompt opens a turn on the child");
        assert_eq!(started, "<runtime>respawn after a theuth restart</runtime>");

        assert_eq!(
            agents.ask(ResumeChild { session: idle }).await.unwrap(),
            ChildResumed::Idle
        );
        assert_eq!(
            agents
                .ask(ResumeChild { session: cancelled })
                .await
                .unwrap(),
            ChildResumed::Idle
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        while let Ok(e) = events.try_recv() {
            assert!(
                !matches!(e, AgentEvent::TurnStarted { session, .. } if session == idle || session == cancelled),
                "an idle or cancelled child is not prompted: {e:?}"
            );
        }
    }

    // ----- spawn permit queue (01 §2.6, spawnstall 2026-10-07) -----
    //
    // The `Agents` actor is deliberately NOT bus-subscribed in these tests:
    // a native child's turn ends as `error` at once (no registry), so the
    // test ends a spawn turn itself by telling `TurnFinished`, exactly as
    // the bus would. Real time, no sleeps: every wait is on a channel.

    /// `ask`: run `f` on the actor's state — test-only inspection.
    struct With<F>(F);

    impl<F, R> Message<With<F>> for Agents
    where
        F: FnOnce(&mut Agents) -> R + Send + 'static,
        R: kameo::reply::Reply + Send + 'static,
    {
        type Reply = R;

        async fn handle(&mut self, m: With<F>, _: &mut Context<Self, Self::Reply>) -> R {
            (m.0)(self)
        }
    }

    /// Every `AgentEvent` on the bus, as a stream.
    #[derive(kameo::Actor)]
    struct Tap(tokio::sync::mpsc::UnboundedSender<AgentEvent>);

    impl Message<AgentEvent> for Tap {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    /// A bus tap on `rt`.
    async fn tap(rt: &Runtime) -> tokio::sync::mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let tap = Tap::spawn_with_mailbox(Tap(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Tap, AgentEvent>(&tap).await;
        rx
    }

    /// The text of the next `TurnStarted` on `session`, within 60 s (the
    /// failing test model is not instant under load; a logic error still fails).
    async fn next_turn(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        session: SessionId,
    ) -> String {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match rx.recv().await.expect("bus open") {
                    AgentEvent::TurnStarted {
                        session: s, text, ..
                    } if s == session => {
                        return text;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("a turn starts")
    }

    /// The next event on `session` matching `pick`, within 60 s (see `next_turn`).
    async fn next_event(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        pick: impl Fn(&AgentEvent) -> bool,
    ) -> AgentEvent {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let e = rx.recv().await.expect("bus open");
                if pick(&e) {
                    return e;
                }
            }
        })
        .await
        .expect("the event arrives")
    }

    /// A spawn of a child of `parent` named `name`, briefed `task`.
    fn spawn(parent: SessionId, name: &str, task: &str) -> SpawnChild {
        SpawnChild {
            parent,
            name: Some(name.into()),
            task: task.into(),
            agent: None,
            cwd: None,
            model: None,
            effort: None,
            cap: None,
            skills: Vec::new(),
            call_id: String::new(),
        }
    }

    /// `session`'s turn ended, as the bus reports it.
    fn finished(session: SessionId) -> AgentEvent {
        AgentEvent::TurnFinished {
            session,
            turn: TurnId::new(),
            stop_reason: "end_turn".into(),
            usage: crate::Usage::default(),
            context: 0,
        }
    }

    /// A root session and the actor over `a`.
    async fn rooted(mut a: Agents) -> (ActorRef<Agents>, SessionId) {
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        (Agents::spawn(a), p)
    }

    /// Native permits free in `root`'s tree, and whether `child` is queued.
    async fn pool_state(
        agents: &ActorRef<Agents>,
        root: SessionId,
        child: SessionId,
    ) -> (usize, bool) {
        agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.pools[&root].native.available_permits(),
                    a.queued.contains_key(&child),
                )
            }))
            .await
            .unwrap()
    }

    /// T1: with one native slot, the first spawn starts and the second is
    /// queued — the parent's reply says so and names the running child —
    /// with nothing routed into the queued child and no permit left.
    #[tokio::test]
    async fn second_spawn_queues_and_tells_the_parent() {
        let (_scratch, a) = agents().await;
        let (agents, p) = rooted(a).await;
        let (_, x, queued) = agents.ask(spawn(p, "x", "first")).await.unwrap();
        assert!(queued.is_none(), "a free slot: started");
        let (_, y, queued) = agents.ask(spawn(p, "y", "second")).await.unwrap();
        let notice = queued.expect("no slot: queued");
        assert!(
            notice.starts_with(
                "Queued: all 1 native child slots of this orchestrator are busy (running: x since "
            ),
            "{notice}"
        );
        assert!(
            notice.ends_with("Messages you send it are delivered after its brief."),
            "{notice}"
        );
        let (routed_x, routed_y) = agents
            .ask(With(move |a: &mut Agents| {
                (a.last_input.contains_key(&x), a.last_input.contains_key(&y))
            }))
            .await
            .unwrap();
        assert!(routed_x && !routed_y, "the brief waits with the permit");
        assert_eq!(pool_state(&agents, p, y).await, (0, true));
    }

    /// T2: a peer message to a queued child is held, not routed; when the
    /// running child's turn ends the queued child's first input is its
    /// brief and the held message comes second, under exactly one permit,
    /// which the supervisor lets go of when its turn ends. The semaphore
    /// count is not read after that: the held message is a peer input, and
    /// when y's actor has already opened it as a bounded turn its 50 §8 gate
    /// takes the permit the instant it frees (a race the test must not pin).
    #[tokio::test]
    async fn held_message_follows_the_brief() {
        let (_scratch, a) = agents_with(true).await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        let (_, x, _) = agents.ask(spawn(p, "x", "first")).await.unwrap();
        assert_eq!(next_turn(&mut rx, x).await, "first");
        let (_, y, queued) = agents.ask(spawn(p, "y", "second")).await.unwrap();
        assert!(queued.is_some());
        agents
            .ask(Deliver {
                from: p,
                to: format!("session:{y}"),
                text: "hello y".into(),
                await_reply: false,
                reply_to: None,
                request_id: None,
                admitted: CancellationToken::new(),
            })
            .await
            .unwrap();
        let (held, routed) = agents
            .ask(With(move |a: &mut Agents| {
                (a.queued[&y].held.len(), a.last_input.contains_key(&y))
            }))
            .await
            .unwrap();
        assert_eq!((held, routed), (1, false), "held, nothing routed");

        agents.tell(finished(x)).await.unwrap();
        assert_eq!(next_turn(&mut rx, y).await, "second", "the brief is first");
        let second = next_turn(&mut rx, y).await;
        assert!(
            second.contains("hello y"),
            "the held message follows: {second}"
        );
        assert_eq!(
            pool_state(&agents, p, y).await,
            (0, false),
            "y holds the one permit"
        );
        agents.tell(finished(y)).await.unwrap();
        let released = agents
            .ask(With(move |a: &mut Agents| {
                a.child_turns.get(&y).is_none_or(|ct| ct.permit.is_none())
            }))
            .await
            .unwrap();
        assert!(released, "the spawn permit goes when y's turn ends");
    }

    /// 183 F11: a contact's first message waiting for the owner never blocks the chat it
    /// asks for: the owner's own turn runs, another session's peer message is delivered, an
    /// allowed message into the same chat is admitted and started (on v2.19.0 it queued
    /// behind the held one), and Allow once then queues the held one as any admitted message.
    #[tokio::test]
    async fn a_waiting_first_message_leaves_the_chat_working() {
        use crate::communication::network::{A2aPrincipal, ContactsHeld, contact_endpoint};
        let (_scratch, mut a) = agents_with(true).await;
        let mut rx = tap(&a.rt).await;
        // Two owner roots: they may message each other (as two chats of the owner do).
        let owner = crate::communication::PeerScope::Owner;
        let chat = a.new_root(None, owner, None, None).await.unwrap();
        let other = a.new_root(None, owner, None, None).await.unwrap();
        let binding = a
            .store
            .bind_peer("bob", &contact_endpoint("bob-endpoint"))
            .await
            .unwrap();
        let principal = A2aPrincipal {
            binding: binding.id,
            export: "hi-there".into(),
            session: chat,
            route: CancellationToken::new(),
            contact: Some(crate::communication::ContactOrigin {
                contact: "bob".into(),
                machine: "b".into(),
                export: "hi-there".into(),
                mode: crate::communication::ShareMode::Cautious,
            }),
            ask_first: true,
        };
        let mut m = a2a_types::Message::new(
            a2a_types::Role::User,
            vec![a2a_types::Part::text("hello owner")],
        );
        m.message_id = "m-1".into();
        let req = a2a_types::SendMessageRequest {
            message: m,
            configuration: None,
            metadata: None,
            tenant: None,
        };
        let held = a.admit_send(&principal, &req).await.unwrap().task;
        assert_eq!(
            held.status,
            crate::store::communication::A2aTaskStatus::AuthRequired
        );
        let agents = Agents::spawn(a);
        agents
            .ask(SendInput {
                session: Some(chat),
                text: "the owner's own turn".into(),
                files: Vec::new(),
                delivery: Default::default(),
                id: None,
                channel: None,
            })
            .await
            .unwrap();
        assert!(
            next_turn(&mut rx, chat)
                .await
                .contains("the owner's own turn")
        );
        agents.tell(finished(chat)).await.unwrap();
        agents
            .ask(Deliver {
                from: other,
                to: format!("session:{chat}"),
                text: "a peer's message".into(),
                await_reply: false,
                reply_to: None,
                request_id: None,
                admitted: CancellationToken::new(),
            })
            .await
            .unwrap();
        assert!(next_turn(&mut rx, chat).await.contains("a peer's message"));
        let waiting = agents.ask(ContactsHeld).await.unwrap();
        assert_eq!(
            waiting.len(),
            1,
            "the first message still waits for the owner"
        );
        assert_eq!(waiting[0].preview, "hello owner");
        agents.tell(finished(chat)).await.unwrap();

        let mut m = a2a_types::Message::new(
            a2a_types::Role::User,
            vec![a2a_types::Part::text("an allowed message")],
        );
        m.message_id = "m-2".into();
        let allowed = agents
            .ask(crate::communication::network::A2aSend {
                principal: A2aPrincipal {
                    ask_first: false,
                    ..principal
                },
                request: a2a_types::SendMessageRequest {
                    message: m,
                    configuration: Some(a2a_types::SendMessageConfiguration {
                        accepted_output_modes: Some(vec!["text/plain".into()]),
                        task_push_notification_config: None,
                        history_length: Some(0),
                        return_immediately: Some(true),
                    }),
                    metadata: None,
                    tenant: None,
                },
            })
            .await
            .unwrap();
        let a2a_types::SendMessageResponse::Task(allowed) = allowed else {
            panic!("a task")
        };
        assert_ne!(
            allowed.status.state,
            a2a_types::TaskState::Submitted,
            "admitted and started though one waits for the owner"
        );
        let released = agents
            .ask(crate::communication::network::ContactRelease {
                contact: "bob".into(),
                export: Some("hi-there".into()),
                allow: true,
            })
            .await
            .unwrap();
        assert_eq!(released, 1);
        assert!(agents.ask(ContactsHeld).await.unwrap().is_empty());
    }

    /// T3: a queued child stopped by the user never gets its brief: the
    /// wait is aborted, its ticket released, the error published; when the
    /// slot frees it stays free.
    #[tokio::test]
    async fn stopped_queued_child_never_starts() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        let (_, x, _) = agents.ask(spawn(p, "x", "first")).await.unwrap();
        let (_, y, _) = agents.ask(spawn(p, "y", "second")).await.unwrap();
        agents
            .ask(Cancel {
                session: y,
                keep: None,
                scope: CancelScope::Root,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::Error { session, .. } if *session == y),
        )
        .await;
        assert!(
            matches!(e, AgentEvent::Error { message, .. } if message == "spawn cancelled before it started")
        );
        let (queued, counted) = agents
            .ask(With(move |a: &mut Agents| {
                (a.queued.contains_key(&y), a.ledger.activity(y).is_some())
            }))
            .await
            .unwrap();
        assert!(!queued && !counted, "dropped and its ticket released");
        agents.tell(finished(x)).await.unwrap();
        // Anything y would have started is ordered before this round trip.
        let (free, routed_y) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.pools[&p].native.available_permits(),
                    a.last_input.contains_key(&y),
                )
            }))
            .await
            .unwrap();
        assert_eq!((free, routed_y), (1, false), "the stale brief never ran");
    }

    /// 133 §2: a `Root` cancel of the parent leaves its running child alone
    /// (ledger untouched, still running, actor alive); `Tree` stops both.
    #[tokio::test]
    async fn root_cancel_spares_running_children_tree_cancel_does_not() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        let (_, x, _) = agents.ask(spawn(p, "x", "first")).await.unwrap();
        assert_eq!(next_turn(&mut rx, x).await, "first");
        // No model here: the child's turn ends at once; wait for that, then mark it running by hand.
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == x),
        )
        .await;
        let (ep, ex) = agents
            .ask(With(move |a: &mut Agents| {
                a.running.insert(x);
                (a.ledger.epoch(p), a.ledger.epoch(x))
            }))
            .await
            .unwrap();
        agents
            .ask(Cancel {
                session: p,
                keep: None,
                scope: CancelScope::Root,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        let (ep2, ex2, running, alive) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.ledger.epoch(p),
                    a.ledger.epoch(x),
                    a.running.contains(&x),
                    a.sessions[&x].actor.is_alive(),
                )
            }))
            .await
            .unwrap();
        assert!(ep2 > ep, "the parent is cancelled");
        assert_eq!(ex2, ex, "the child's ledger is untouched");
        assert!(running && alive, "the child keeps working");
        agents
            .ask(Cancel {
                session: p,
                keep: None,
                scope: CancelScope::Tree,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        let (ep3, ex3) = agents
            .ask(With(move |a: &mut Agents| {
                (a.ledger.epoch(p), a.ledger.epoch(x))
            }))
            .await
            .unwrap();
        assert!(ep3 > ep2 && ex3 > ex2, "tree: both cancelled");
    }

    /// T5: a human input into a queued child promotes it: the brief runs at
    /// once, then the human text, with no permit taken and the running
    /// sibling untouched.
    #[tokio::test]
    async fn human_input_promotes_a_queued_child() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        let (_, _x, _) = agents.ask(spawn(p, "x", "first")).await.unwrap();
        let (_, y, queued) = agents.ask(spawn(p, "y", "second")).await.unwrap();
        assert!(queued.is_some());
        agents
            .ask(SendInput {
                channel: None,
                session: Some(y),
                text: "hurry".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        assert_eq!(next_turn(&mut rx, y).await, "second", "the brief first");
        assert_eq!(next_turn(&mut rx, y).await, "hurry", "then the human");
        let (free, queued, permit_free) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.pools[&p].native.available_permits(),
                    a.queued.contains_key(&y),
                    a.child_turns[&y].permit.is_none(),
                )
            }))
            .await
            .unwrap();
        assert_eq!(
            (free, queued, permit_free),
            (0, false, true),
            "promoted permit-free"
        );
    }

    /// 145: a finished job's result for `p` — the `Settle` path `inject`s as
    /// a `<job_result>` harness input (`Automatic`).
    async fn job_done(agents: &ActorRef<Agents>, p: SessionId, id: &str, out: &str) {
        let op = tools::Activity::untracked(p, tools::Kind::Command("t")).id();
        agents
            .ask(Settle::FinishedWithResult {
                op,
                result: JobEvent::Finished {
                    session: p,
                    id: JobId(id.into()),
                    command: "echo".into(),
                    exit: JobExit::Exited(0),
                    secs: 1,
                    output: out.into(),
                    inline: false,
                    pty: false,
                    kind: JobKind::Shell,
                },
            })
            .await
            .unwrap();
    }

    async fn set_talk(agents: &ActorRef<Agents>, p: SessionId, open: bool) {
        agents
            .ask(SetTalk {
                session: p,
                live: Some(TalkLive {
                    open,
                    promote_after_ms: None,
                }),
            })
            .await
            .unwrap();
    }

    /// `true` when no `TurnStarted` for `session` arrives within `ms`.
    async fn no_turn_within(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        session: SessionId,
        ms: u64,
    ) -> bool {
        tokio::time::timeout(Duration::from_millis(ms), async {
            loop {
                if let AgentEvent::TurnStarted { session: s, .. } = rx.recv().await.expect("bus")
                    && s == session
                {
                    return;
                }
            }
        })
        .await
        .is_err()
    }

    /// Parked count on `p`.
    async fn parked(agents: &ActorRef<Agents>, p: SessionId) -> usize {
        agents
            .ask(With(move |a: &mut Agents| {
                a.talk.get(&p).map_or(0, |t| t.parked.len())
            }))
            .await
            .unwrap()
    }

    /// 145 A1: a closed window parks harness inputs for an idle talk session
    /// (`InputsParked` counts them, nothing is routed); opening it starts one
    /// turn whose text holds both envelopes in arrival order.
    #[tokio::test]
    async fn closed_window_parks_and_opening_coalesces() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        set_talk(&agents, p, false).await;
        job_done(&agents, p, "j1", "one").await;
        job_done(&agents, p, "j2", "two").await;
        let mut seen = Vec::new();
        for _ in 0..2 {
            if let AgentEvent::InputsParked { count, .. } =
                next_event(&mut rx, |e| matches!(e, AgentEvent::InputsParked { .. })).await
            {
                seen.push(count);
            }
        }
        assert_eq!(seen, [1, 2]);
        assert_eq!(parked(&agents, p).await, 2);
        assert!(no_turn_within(&mut rx, p, 300).await, "nothing routed");
        set_talk(&agents, p, true).await;
        let text = next_turn(&mut rx, p).await;
        let (one, two) = (
            text.find("id=\"j1\"").unwrap(),
            text.find("id=\"j2\"").unwrap(),
        );
        assert!(one < two, "{text}");
        assert!(text.contains("</job_result>\n\n<job_result"), "{text}");
        assert_eq!(parked(&agents, p).await, 0);
    }

    /// 145 A2: on an open window two harness inputs 50 ms apart share one
    /// turn; 500 ms apart they get one each.
    #[tokio::test]
    async fn open_window_coalesces_within_the_window_only() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        set_talk(&agents, p, true).await;
        job_done(&agents, p, "j1", "one").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        job_done(&agents, p, "j2", "two").await;
        let text = next_turn(&mut rx, p).await;
        assert!(
            text.contains("id=\"j1\"") && text.contains("id=\"j2\""),
            "{text}"
        );
        // The test model fails at once, but under load not instantly: the
        // second pair starts from an idle actor, so a queued pair cannot be
        // mistaken for a coalesced one.
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == p),
        )
        .await;
        job_done(&agents, p, "j3", "three").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        job_done(&agents, p, "j4", "four").await;
        let third = next_turn(&mut rx, p).await;
        assert!(
            third.contains("id=\"j3\"") && !third.contains("id=\"j4\""),
            "{third}"
        );
        let fourth = next_turn(&mut rx, p).await;
        assert!(fourth.contains("id=\"j4\""), "{fourth}");
    }

    /// 145 A3: the cap releases a parked input with the window still closed.
    #[tokio::test]
    async fn cap_releases_parked_input_while_closed() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        set_talk(&agents, p, false).await;
        job_done(&agents, p, "j1", "one").await;
        assert_eq!(parked(&agents, p).await, 1);
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(TALK_PARK_CAP_MS + 1)).await;
        tokio::time::resume();
        let text = next_turn(&mut rx, p).await;
        assert!(text.contains("id=\"j1\""), "{text}");
        assert_eq!(parked(&agents, p).await, 0);
    }

    /// 145 A4: a human input on a session with parked items starts its own
    /// turn first; the parked items follow it as steers (here: the next turn,
    /// since the test model fails at once).
    #[tokio::test]
    async fn human_input_releases_parked_items_as_steers() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        set_talk(&agents, p, false).await;
        job_done(&agents, p, "j1", "one").await;
        agents
            .ask(SendInput {
                channel: None,
                session: Some(p),
                text: "hurry".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        assert_eq!(next_turn(&mut rx, p).await, "hurry", "the human first");
        assert_eq!(parked(&agents, p).await, 0);
        let text = next_turn(&mut rx, p).await;
        assert!(text.contains("id=\"j1\""), "{text}");
    }

    /// 145 A6: the talk ending releases parked items and forgets the session.
    #[tokio::test]
    async fn talk_ending_releases_parked_items() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        set_talk(&agents, p, false).await;
        job_done(&agents, p, "j1", "one").await;
        assert_eq!(parked(&agents, p).await, 1);
        agents
            .ask(SetTalk {
                session: p,
                live: None,
            })
            .await
            .unwrap();
        let text = next_turn(&mut rx, p).await;
        assert!(text.contains("id=\"j1\""), "{text}");
        let gone = agents
            .ask(With(move |a: &mut Agents| !a.talk.contains_key(&p)))
            .await
            .unwrap();
        assert!(gone);
        // Without a talk the next result routes at once, as before.
        job_done(&agents, p, "j2", "two").await;
        assert!(next_turn(&mut rx, p).await.contains("id=\"j2\""));
    }

    /// A `<cell_result>` for `p` as the cells actor delivers it (171 L3); `kind` may carry
    /// [`QUIET_KIND`] the way a 139 `harness_outbox` row does.
    async fn cell_done(agents: &ActorRef<Agents>, p: SessionId, kind: &str, run: &str, wake: bool) {
        agents
            .ask(HarnessDeliver {
                session: p,
                kind: format!("{kind}{run}"),
                text: run.into(),
                wrapped: format!("<cell_result run=\"{run}\" exit=\"0\">\nok\n</cell_result>"),
                wake,
            })
            .await
            .unwrap();
    }

    async fn human(agents: &ActorRef<Agents>, p: SessionId, text: &str) {
        agents
            .ask(SendInput {
                channel: None,
                session: Some(p),
                text: text.into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
    }

    /// 171 §9.3: a quiet result starts no turn; the next turn — the human's — carries it,
    /// ahead of the human's text. Parked items live in memory only (the cells actor
    /// re-parks undelivered runs at engine start).
    #[tokio::test]
    async fn parked_cell_result_joins_the_next_turn_and_starts_none() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        cell_done(&agents, p, "cell ", "r_1", false).await;
        assert!(no_turn_within(&mut rx, p, 300).await, "parked, no turn");
        human(&agents, p, "what now?").await;
        let text = next_turn(&mut rx, p).await;
        let (cell, asked) = (
            text.find("run=\"r_1\"").unwrap(),
            text.find("what now?").unwrap(),
        );
        assert!(cell < asked, "{text}");
        assert!(no_turn_within(&mut rx, p, 300).await, "one turn only");
    }

    /// 171 §9.3: a waking result on an idle session starts exactly one turn.
    #[tokio::test]
    async fn waking_cell_result_starts_one_turn() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        cell_done(&agents, p, "cell ", "r_1", true).await;
        assert!(next_turn(&mut rx, p).await.contains("run=\"r_1\""));
        assert!(no_turn_within(&mut rx, p, 300).await, "one turn only");
    }

    /// 171 §9.3: two parked results (one through the 139 outbox marker) and a waking one
    /// share the turn the waking one opens, in arrival order.
    #[tokio::test]
    async fn parked_and_waking_results_keep_arrival_order() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        cell_done(&agents, p, "cell ", "r_1", false).await;
        cell_done(&agents, p, &format!("{QUIET_KIND}cell "), "r_2", true).await;
        assert!(no_turn_within(&mut rx, p, 300).await, "both parked");
        cell_done(&agents, p, "cell ", "r_3", true).await;
        let text = next_turn(&mut rx, p).await;
        let at = |run: &str| text.find(&format!("run=\"{run}\"")).unwrap();
        assert!(at("r_1") < at("r_2") && at("r_2") < at("r_3"), "{text}");
        assert!(no_turn_within(&mut rx, p, 300).await, "one turn only");
    }

    /// 171F B22: Ask theuth on a parked run (the same run again, with a wake) opens one turn
    /// that carries the run once.
    #[tokio::test]
    async fn asking_about_a_parked_run_delivers_it_once() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        cell_done(&agents, p, "cell ", "r_x", false).await;
        assert!(no_turn_within(&mut rx, p, 300).await, "parked");
        cell_done(&agents, p, "cell ", "r_x", true).await;
        let text = next_turn(&mut rx, p).await;
        assert_eq!(
            text.matches("<cell_result run=\"r_x\"").count(),
            1,
            "{text}"
        );
        assert!(no_turn_within(&mut rx, p, 300).await, "one turn only");
    }

    /// 171F4 T2-5: five parked runs of one cell, then one of another, reach the next turn as
    /// two results — the first is the cell's newest with `runs="5"` and the earlier exits that
    /// differ from its own.
    #[tokio::test]
    async fn a_cells_repeats_fold_into_its_newest_result() {
        let (_scratch, a) = agents().await;
        let mut rx = tap(&a.rt).await;
        let (agents, p) = rooted(a).await;
        let runs = [("r_1", "1", "1"), ("r_2", "1", "0"), ("r_3", "1", "1")];
        let more = [("r_4", "1", "0"), ("r_5", "1", "0"), ("r_o", "2", "0")];
        for (run, cell, exit) in runs.into_iter().chain(more) {
            agents
                .ask(HarnessDeliver {
                    session: p,
                    kind: format!("cell {cell}"),
                    text: run.into(),
                    wrapped: format!(
                        "<cell_result run=\"{run}\" cell=\"{cell}\" message=\"4\" exit=\"{exit}\">\nok\n</cell_result>"
                    ),
                    wake: false,
                })
                .await
                .unwrap();
        }
        assert!(no_turn_within(&mut rx, p, 300).await, "all parked");
        human(&agents, p, "what now?").await;
        let text = next_turn(&mut rx, p).await;
        assert_eq!(text.matches("<cell_result ").count(), 2, "{text}");
        assert!(
            text.contains("<cell_result run=\"r_5\" cell=\"1\" message=\"4\" exit=\"0\" runs=\"5\" earlier=\"2 failed\">"),
            "{text}"
        );
        assert!(text.find("run=\"r_5\"").unwrap() < text.find("run=\"r_o\"").unwrap());
        assert!(
            text.contains("run=\"r_o\" cell=\"2\" message=\"4\" exit=\"0\">"),
            "{text}"
        );
    }

    /// 171 HANDOFF-Z-1: work admitted under an owner with no `sessions` row (a note cell
    /// kernel's session id) blocks only that owner, never an unrelated session.
    #[tokio::test]
    async fn rowless_owner_blocks_no_other_session() {
        let (_scratch, mut a) = agents().await;
        a.ledger.certify();
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let agents = Agents::spawn(a);
        let ghost = SessionId::new();
        let kind = tools::Kind::Pty(JobId("k1".into()));
        let op = tools::Operation {
            id: tools::Activity::untracked(ghost, kind.clone()).id(),
            owner: ghost,
            kind,
        };
        agents.ask(Admit(op)).await.unwrap();
        let (other, own) = agents
            .ask(With(move |a: &mut Agents| {
                (a.ledger.activity(p), a.ledger.activity(ghost))
            }))
            .await
            .unwrap();
        assert_eq!(other, None, "an unrelated session is not blocked");
        assert!(own.is_some(), "the owner's own work still counts");
    }

    /// L1 (171 inv. 3): a job announced with a `cell:` name, or idling under one, becomes
    /// neither `<job_result>` nor `<job_idle>`; an agent job beside it still does.
    #[tokio::test]
    async fn cell_jobs_are_never_job_result_or_job_idle() {
        let (_scratch, mut a) = agents_with(true).await;
        let (p, _t) = in_turn(&mut a).await;
        let op = |id: &str| tools::Operation {
            id: tools::Activity::untracked(p, tools::Kind::Pty(JobId(id.into()))).id(),
            owner: p,
            kind: tools::Kind::Pty(JobId(id.into())),
        };
        let idle = |op: &tools::Operation, id: &str, name: Option<&str>| Notice::JobIdle {
            op: op.id,
            event: JobEvent::Idle {
                session: p,
                id: JobId(id.into()),
                command: "bash".into(),
                name: name.map(str::to_owned),
                quiet_secs: 61,
                secs: 90,
                nudge: 1,
                user_typed_secs: None,
                output: "$ ".into(),
                why: String::new(),
            },
        };
        let finished = |op: &tools::Operation, id: &str| Settle::FinishedWithResult {
            op: op.id,
            result: JobEvent::Finished {
                session: p,
                id: JobId(id.into()),
                command: "bash".into(),
                exit: JobExit::Exited(0),
                secs: 1,
                output: "bye".into(),
                inline: false,
                pty: true,
                kind: JobKind::Pty,
            },
        };
        a.cell_jobs.observe(&JobEvent::Started {
            session: p,
            id: JobId("k1".into()),
            command: "bash".into(),
            call_id: String::new(),
            log: PathBuf::new(),
            name: Some("cell:bash 5.2 (Homebrew)".into()),
            pty: true,
            kind: JobKind::Pty,
        });
        let (k1, k2, j1) = (op("k1"), op("k2"), op("j1"));
        for o in [&k1, &k2, &j1] {
            a.ledger.admit_op(o);
        }
        a.notice(idle(&k1, "k1", None)).await;
        a.notice(idle(&k2, "k2", Some("cell:zsh"))).await;
        a.settle(finished(&k1, "k1")).await;
        assert_eq!(a.ledger.pending_count(p), 0, "no input for cell jobs");
        assert!(
            !a.cell_jobs.is_cell(p, &JobId("k1".into())),
            "forgotten once finished"
        );
        a.notice(idle(&j1, "j1", None)).await;
        a.settle(finished(&j1, "j1")).await;
        assert_eq!(a.ledger.pending_count(p), 2, "an agent job still reports");
    }

    /// A fake node `box` on `a`: the harness side of its one connection.
    fn node(a: &Agents, name: &str) -> crate::acp_child::tests::Fake {
        let (fake, dialer) = crate::acp_child::tests::Fake::new();
        a.nodes
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.into(), ("fake".into(), dialer));
        fake
    }

    /// A spawn of a child of `parent` on node `node`.
    fn spawn_on(parent: SessionId, node: &str, name: &str, task: &str) -> SpawnChild {
        SpawnChild {
            agent: Some(node.into()),
            ..spawn(parent, name, task)
        }
    }

    /// T4: the pools are separate — with the one native slot taken, a node
    /// child starts at once from the external pool; with the one external
    /// slot taken, a second node child queues while a native spawn still
    /// starts.
    #[tokio::test]
    async fn native_and_external_pools_are_separate() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let (agents, p) = rooted(a).await;
        let (_, _x, _) = agents.ask(spawn(p, "x", "native")).await.unwrap();
        let (_, _, queued) = agents
            .ask(spawn_on(p, "box", "e1", "remote one"))
            .await
            .unwrap();
        assert!(queued.is_none(), "the native pool being full is irrelevant");
        fake.handshake(Vec::new(), None).await;
        let (_, text) = fake.prompt().await;
        assert_eq!(text, "remote one", "the brief reached the harness");
        let (_, e2, queued) = agents
            .ask(spawn_on(p, "box", "e2", "remote two"))
            .await
            .unwrap();
        let notice = queued.expect("the one external slot is held");
        assert!(notice.starts_with("Queued: all 1 external child slots of this orchestrator are busy (running: e1 since "), "{notice}");
        let (ext_free, nat_free, e2_queued) = agents
            .ask(With(move |a: &mut Agents| {
                let pools = &a.pools[&p];
                (
                    pools.external.available_permits(),
                    pools.native.available_permits(),
                    a.queued.contains_key(&e2),
                )
            }))
            .await
            .unwrap();
        assert_eq!((ext_free, nat_free, e2_queued), (0, 0, true));
        // The native pool frees: a native spawn starts although external is full.
        agents.tell(finished(_x)).await.unwrap();
        let (_, _, queued) = agents.ask(spawn(p, "z", "native two")).await.unwrap();
        assert!(
            queued.is_none(),
            "native spawns never wait on the external pool"
        );
    }

    /// T6 (D6): a node harness that drops mid-prompt ends the spawn turn as
    /// `error`, and the supervisor — bus-subscribed here, as in production —
    /// releases the external permit with the turn.
    #[tokio::test]
    async fn harness_exit_releases_the_external_permit() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        // Subscribed first: the bus tells subscribers in registration order,
        // so the supervisor has handled an event before the tap sees it.
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, queued) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        assert!(queued.is_none());
        fake.handshake(Vec::new(), None).await;
        let (_, text) = fake.prompt().await;
        assert_eq!(text, "go");
        let held = agents
            .ask(With(move |a: &mut Agents| {
                a.pools[&p].external.available_permits()
            }))
            .await
            .unwrap();
        assert_eq!(held, 0, "the spawn turn holds the external permit");
        // The harness goes away with the prompt in flight.
        drop(fake);
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        assert!(
            matches!(e, AgentEvent::TurnFinished { stop_reason, .. } if stop_reason == "error")
        );
        // The bus delivered the same event to the supervisor before this ask.
        let (free, turn) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.pools[&p].external.available_permits(),
                    a.child_turns.contains_key(&e1),
                )
            }))
            .await
            .unwrap();
        assert_eq!((free, turn), (1, false), "permit released with the turn");
    }

    /// 141 §5.4: a watch-only row is never driven — `session()` (the one
    /// choke point behind send_input/steer/undo/compact/resume) refuses it
    /// with [`OBSERVED`]; it is not a peer (`resolve`, `enqueue_peer`); the
    /// directory lists it `send: false`; the roster state follows the
    /// harness's state; a repeat announce is the same row.
    #[tokio::test]
    async fn observed_rows_are_refused_as_targets_and_listed_watch_only() {
        let (_scratch, mut a) = agents_with(true).await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let acp = SessionId::new();
        a.store
            .create_session(acp, "omp", Some(root), Some("omp"), None)
            .await
            .unwrap();
        let mut up =
            crate::store::ObservedUpsert::new(acp, crate::ObservedSource::OmpTask, "ReadA");
        up.name = Some("ReadA");
        up.state = Some(ObservedState::Running);
        let (child, created) = a.store.upsert_observed(up).await.unwrap();
        assert!(created);
        let err = match a.session(Some(child), None, None, (None, None)).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("watch-only rows are never driven"),
        };
        assert_eq!(err, OBSERVED);
        assert!(!a.sessions.contains_key(&child), "no actor was spawned");
        let err = match a.resolve(root, &format!("session:{child}")).await {
            Err(e) => e,
            Ok(_) => panic!("not a peer"),
        };
        assert!(err.ends_with(OBSERVED_PEER), "{err}");
        let rows = a.directory(root).await.unwrap();
        let row = rows
            .iter()
            .find(|r| r.address == format!("session:{child}"))
            .expect("listed");
        assert!(!row.send && !row.reply, "{row:?}");
        let infos = a.view().agents(root).await;
        let info = infos.iter().find(|i| i.id == child).expect("in the tree");
        assert_eq!(info.state, AgentState::Running, "{info:?}");
        assert_eq!(info.kind, SessionKind::Observed);
        assert_eq!(
            info.observed.as_ref().map(|o| o.external.as_str()),
            Some("ReadA")
        );
        assert_eq!(info.parent, acp);
        // Engine restart with the harness gone: lost contact, published.
        let rt = a.rt.clone();
        let mut rx = tap(&rt).await;
        a.mark_observed_lost().await;
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::ObservedChildChanged { child: c, .. } if *c == child),
        )
        .await;
        let AgentEvent::ObservedChildChanged { observed, .. } = e else {
            unreachable!()
        };
        assert_eq!(observed.state, ObservedState::Unknown);
        assert_eq!(observed.stop.as_deref(), Some("lost contact"));
        let infos = a.view().agents(root).await;
        let info = infos.iter().find(|i| i.id == child).unwrap();
        assert_eq!(
            info.state,
            AgentState::Idle,
            "unknown shows idle, labelled by the UI"
        );
    }

    /// 141 §7.7: `Cancel` on a watch-only child (no actor of its own) is
    /// forwarded to the harness actor that runs it; with
    /// `capabilities.cancel` that is a `session/cancel` on the child's own
    /// id. The parent's turn is untouched.
    #[tokio::test]
    async fn cancel_of_an_observed_child_is_forwarded_to_its_harness() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, _) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        fake.handshake(Vec::new(), None).await;
        let (id, _) = fake.prompt().await;
        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        let actor = agents
            .ask(With(move |a: &mut Agents| match &a.sessions[&e1].actor {
                LiveActor::Acp(a) => a.clone(),
                LiveActor::Native(_) => unreachable!(),
            }))
            .await
            .unwrap();
        let mut ann = acp::ChildAnnounce::new(acp::ChildSource::Rfd, "sess_child_1");
        ann.title = Some("Reader".into());
        ann.can_cancel = Some(true);
        ann.state = Some(acp::ChildState::Running);
        actor.tell(acp::Update::Subagent(ann)).await.unwrap();
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::ObservedChildChanged { parent, .. } if *parent == e1),
        )
        .await;
        let AgentEvent::ObservedChildChanged { child, .. } = e else {
            unreachable!()
        };
        agents
            .tell(Cancel {
                session: child,
                scope: CancelScope::Root,
                keep: None,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        let m = fake.next().await;
        assert_eq!(m["method"], "session/cancel", "{m}");
        assert_eq!(m["params"]["sessionId"], "sess_child_1", "{m}");
        // `Tree` on the harness session reaches the cancellable child too.
        agents
            .tell(Cancel {
                session: e1,
                scope: CancelScope::Tree,
                keep: None,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        let m = fake.next().await;
        assert_eq!(m["method"], "session/cancel", "{m}");
        assert_eq!(m["params"]["sessionId"], "sess_child_1", "{m}");
    }

    /// T7: pools are per orchestrator tree — a queued child in tree A does
    /// not delay a spawn in tree B, and a session in tree B draws its
    /// bounded turns' permits from B's native pool, never A's.
    #[tokio::test]
    async fn pools_are_per_orchestrator_tree() {
        let (_scratch, mut a) = agents().await;
        let (p1, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let (p2, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let agents = Agents::spawn(a);
        let (_, _a1, _) = agents.ask(spawn(p1, "a1", "one")).await.unwrap();
        let (_, a2, queued) = agents.ask(spawn(p1, "a2", "two")).await.unwrap();
        assert!(queued.is_some(), "tree A is full");
        let (_, b1, queued) = agents.ask(spawn(p2, "b1", "three")).await.unwrap();
        assert!(queued.is_none(), "tree B has its own slot");
        let (a_free, a2_queued, b_free, b1_permits) = agents
            .ask(With(move |a: &mut Agents| {
                let b1_actor = a.sessions[&b1].actor.native().unwrap().clone();
                (
                    a.pools[&p1].native.available_permits(),
                    a.queued.contains_key(&a2),
                    a.pools[&p2].native.available_permits(),
                    (
                        Arc::as_ptr(&a.pools[&p1].native) as usize,
                        Arc::as_ptr(&a.pools[&p2].native) as usize,
                        b1_actor,
                    ),
                )
            }))
            .await
            .unwrap();
        assert_eq!((a_free, a2_queued, b_free), (0, true, 0));
        let (pool_a, pool_b, b1_actor) = b1_permits;
        let draws_from = b1_actor.ask(crate::loop_::PermitsOf).await.unwrap();
        assert_eq!(draws_from, pool_b, "b1's bounded turns take B's permit");
        assert_ne!(draws_from, pool_a, "never A's");
    }

    // ----- R2: ACP robustness -----

    /// R2.4: a follow-up turn on an ACP child (a human message after its
    /// spawn turn ended) holds no permit but is timed like the spawn turn,
    /// so a prompt that never returns cannot keep the child running forever.
    #[tokio::test]
    async fn follow_up_acp_turns_get_the_runtime_timer() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, _) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        fake.handshake(Vec::new(), None).await;
        let (id, _) = fake.prompt().await;
        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        agents
            .ask(SendInput {
                channel: None,
                session: Some(e1),
                text: "more".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        let (_, text) = fake.prompt().await;
        assert_eq!(text, "more");
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnStarted { session, .. } if *session == e1),
        )
        .await;
        let (timed, permit_free) = agents
            .ask(With(move |a: &mut Agents| {
                let ct = &a.child_turns[&e1];
                (ct.timer.is_some(), ct.permit.is_none())
            }))
            .await
            .unwrap();
        assert!(timed && permit_free, "timed, permit-free follow-up turn");
    }

    /// 193 B1: an ACP child's seat cwd is its own spawn cwd, its write_root the parent's.
    /// (A relative path resolving in that cwd: tools'
    /// `text::tests::an_acp_childs_relative_path_resolves_in_its_own_cwd`.)
    #[tokio::test]
    async fn acp_child_seat_is_its_spawn_cwd_with_the_parents_write_root() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let (da, db) = (crate::test_dir("seat-a"), crate::test_dir("seat-b"));
        let (agents, p) = rooted(a).await;
        tools::seat::set_cwd(p, Some(da.path().to_path_buf()));
        tools::seat::set_write_root(p, Some(da.path().to_path_buf()));
        let (_, c, _) = agents
            .ask(SpawnChild {
                cwd: Some(db.path().to_path_buf()),
                ..spawn_on(p, "box", "seated", "go")
            })
            .await
            .unwrap();
        fake.handshake(Vec::new(), None).await;
        // `ask` returned: `acp_session` (which seats the child) ran before the spawn answered.
        let seat = tools::seat::seat_of(c);
        assert_eq!(seat.cwd, Some(db.path().to_path_buf()));
        assert_eq!(seat.write_root, Some(da.path().to_path_buf()));
    }

    /// 109 LIVE: a child whose open turn is resumed after a restart
    /// publishes `TurnResumed`, never `TurnStarted`; the roster still shows it
    /// `Running` (with a start time) until its `TurnFinished`.
    #[tokio::test]
    async fn resumed_child_turn_counts_as_running() {
        let (_scratch, mut a) = agents().await;
        let rt = a.rt.clone();
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let state = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                let rows = agents.ask(ListAgents { session: root }).await.unwrap();
                let row = rows.into_iter().find(|r| r.id == child).unwrap();
                (row.state, row.started_at.is_some())
            }
        };
        assert_eq!(state(&agents).await, (AgentState::Idle, false));

        let now = Utc::now();
        rt.publish(AgentEvent::TurnResumed {
            session: child,
            turn: TurnId::new(),
            kind: crate::durability::ResumeKind::Interrupted,
            interrupted_at: now,
            resumed_at: now,
            partial_dropped: false,
            cause: crate::durability::ResumeCause::Crash,
            from: None,
        })
        .await;
        next_event(&mut rx, |e| matches!(e, AgentEvent::TurnResumed { .. })).await;
        assert_eq!(state(&agents).await, (AgentState::Running, true));

        rt.publish(finished(child)).await;
        next_event(&mut rx, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
        assert_eq!(state(&agents).await, (AgentState::Idle, false));
    }

    // ----- 178: every child turn is capped; failed children say so -----

    use crate::store::Scope;
    use tools::WaitRow;

    /// `child`'s `ChildTurn`: (present, timer armed, permit held, age in s).
    async fn child_turn_of(agents: &ActorRef<Agents>, child: SessionId) -> (bool, bool, bool, i64) {
        agents
            .ask(With(move |a: &mut Agents| {
                match a.child_turns.get(&child) {
                    Some(ct) => (
                        true,
                        ct.timer.is_some(),
                        ct.permit.is_some(),
                        (Utc::now() - ct.started_at).num_seconds(),
                    ),
                    None => (false, false, false, 0),
                }
            }))
            .await
            .unwrap()
    }

    fn started(session: SessionId, turn: TurnId) -> AgentEvent {
        AgentEvent::TurnStarted {
            session,
            turn,
            text: "go".into(),
            model: "m".into(),
            turn_seq: 1,
            channel: None,
        }
    }

    fn resumed(session: SessionId, turn: TurnId, interrupted_at: DateTime<Utc>) -> AgentEvent {
        AgentEvent::TurnResumed {
            session,
            turn,
            kind: crate::durability::ResumeKind::Interrupted,
            interrupted_at,
            resumed_at: Utc::now(),
            partial_dropped: false,
            cause: crate::durability::ResumeCause::Crash,
            from: None,
        }
    }

    /// A journaled turn `turn` of `session`: two steps, at `first` and `last`.
    async fn journal_turn(
        store: &Store,
        session: SessionId,
        turn: TurnId,
        first: DateTime<Utc>,
        last: DateTime<Utc>,
    ) {
        use crate::durability::StepKind;
        let p = serde_json::json!({"source": "user"});
        store
            .step(session, turn, StepKind::InputAccepted, None, &p)
            .await
            .unwrap();
        store
            .step(
                session,
                turn,
                StepKind::ModelCall,
                Some("c1"),
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        store.set_step_at(session, turn, 1, first).await.unwrap();
        store.set_step_at(session, turn, 2, last).await.unwrap();
    }

    /// 178 W1: a native child turn is capped by the supervisor's timer; at
    /// the cap the turn is cancelled, its result reaches the parent with the
    /// `[timed out …]` prefix and the parent's `child_result` row is cleared.
    #[tokio::test]
    async fn native_spawn_turn_is_capped_and_timeout_is_prefixed() {
        let (_scratch, mut a) = agents_with(true).await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 1;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (p, _) = in_turn(&mut a).await;
        let child = kid(&mut a, p, "x").await;
        child_waits::child_spawned(&mut a, p, child, "call-x", None).await;
        assert_eq!(store.child_waits(Scope::Here).await.unwrap().len(), 1);
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let ct = TurnId::new();
        agents.tell(started(child, ct)).await.unwrap();
        let (_, timed, permit, _) = child_turn_of(&agents, child).await;
        assert!(timed && !permit, "timer armed");
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let hit = agents
            .ask(With(move |a: &mut Agents| a.timed_out.contains(&child)))
            .await
            .unwrap();
        assert!(hit, "the cap fired");
        // The cancel lands: the turn ends `cancelled` with what it had.
        agents
            .ask(With(move |a: &mut Agents| {
                child_turn(a, child, ct, true);
                a.turn_text.insert((child, ct), "partial".into());
            }))
            .await
            .unwrap();
        agents
            .tell(AgentEvent::TurnFinished {
                session: child,
                turn: ct,
                stop_reason: "cancelled".into(),
                usage: crate::Usage::default(),
                context: 0,
            })
            .await
            .unwrap();
        assert!(!child_turn_of(&agents, child).await.0);
        let got = store.peer_records(p, None, 1).await.unwrap().remove(0).text;
        assert_eq!(got, "[timed out after 1s, stop=cancelled]\npartial");
        assert!(store.child_waits(Scope::Here).await.unwrap().is_empty());
    }

    /// 178 W1: a native child's follow-up turn (no permit) is timed too, and
    /// each new turn gets a fresh timer.
    #[tokio::test]
    async fn native_follow_up_turn_gets_runtime_timer() {
        let (_scratch, mut a) = agents().await;
        let rt = a.rt.clone();
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        agents.tell(started(child, TurnId::new())).await.unwrap();
        let (present, timed, permit, _) = child_turn_of(&agents, child).await;
        assert!(present && timed && !permit, "timed, permit-free");
        agents.tell(finished(child)).await.unwrap();
        assert!(!child_turn_of(&agents, child).await.0, "gone with the turn");
        agents.tell(started(child, TurnId::new())).await.unwrap();
        assert!(
            child_turn_of(&agents, child).await.1,
            "the next turn is timed"
        );
        // A depth-0 session's turn is never a child turn.
        agents.tell(started(root, TurnId::new())).await.unwrap();
        assert!(!child_turn_of(&agents, root).await.0);
    }

    /// 178 W1: a native child turn resumed after a restart is capped with
    /// what is left of its cap — here none (it ran 3 s of a 1 s cap), so the
    /// floor applies (`RESUME_GRACE`, never above the cap) and it fires; its
    /// age counts the pre-restart run. A resumed depth-0 turn gets no timer.
    #[tokio::test]
    async fn resumed_native_child_gets_a_timer_with_remaining_time() {
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 1;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let (t, now) = (TurnId::new(), Utc::now());
        journal_turn(&store, child, t, now - chrono::Duration::seconds(3), now).await;
        agents.tell(resumed(child, t, now)).await.unwrap();
        let (present, timed, _, age) = child_turn_of(&agents, child).await;
        assert!(present && timed);
        assert!((3..=5).contains(&age), "age {age}");
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let hit = agents
            .ask(With(move |a: &mut Agents| a.timed_out.contains(&child)))
            .await
            .unwrap();
        assert!(hit, "over its cap: cancelled after the floor");
        let rt0 = TurnId::new();
        journal_turn(&store, root, rt0, now - chrono::Duration::seconds(3), now).await;
        agents.tell(resumed(root, rt0, now)).await.unwrap();
        assert!(
            !child_turn_of(&agents, root).await.0,
            "depth 0 is unbounded"
        );
        assert_eq!(
            crate::durability::remaining_run(
                Duration::from_secs(90),
                chrono::Duration::seconds(40)
            ),
            crate::durability::RESUME_GRACE,
            "50 s left of 90 → the 60 s floor"
        );
        assert_eq!(
            crate::durability::remaining_run(
                Duration::from_secs(28_800),
                chrono::Duration::seconds(800)
            ),
            Duration::from_secs(28_000)
        );
    }

    /// 178 W1: the pre-restart run is the journal's first step to the
    /// interruption — the downtime after it (a laptop asleep) never counts.
    #[tokio::test]
    async fn timer_does_not_count_downtime() {
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 600;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let (t, now) = (TurnId::new(), Utc::now());
        let stopped = now - chrono::Duration::seconds(990);
        journal_turn(
            &store,
            child,
            t,
            now - chrono::Duration::seconds(1000),
            stopped,
        )
        .await;
        agents.tell(resumed(child, t, stopped)).await.unwrap();
        let (_, timed, _, age) = child_turn_of(&agents, child).await;
        assert!(timed);
        assert!((10..=12).contains(&age), "10 s ran, not 1000: {age}");
    }

    /// 178 W1/F1 (146): an ACP child whose journal turn stayed open across
    /// two restarts — opened 50 min ago, drained at 15 s of run time, resumed,
    /// drained again at 40 s — is timed on its next `TurnStarted` from the
    /// drains' cumulative `ran_ms` (40 s), never its wall-clock age.
    #[tokio::test]
    async fn twice_resumed_acp_child_counts_run_time_once() {
        use crate::durability::StepKind;
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 600;
        let mut fake = node(&a, "box");
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, _) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        fake.handshake(Vec::new(), None).await;
        let (id, _) = fake.prompt().await;
        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        for _ in 0..800 {
            if store.open_turn(e1).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // The prompt's journal turn, opened again as a restart leaves it (or
        // still open when the close lags under load: one `input_accepted` either way).
        let payload = serde_json::json!({"source": "input", "text": "go"});
        let jt = match store.acp_turn_open(e1, &payload).await.unwrap() {
            Some(t) => t,
            None => store.open_turn(e1).await.unwrap().expect("open").0,
        };
        for ms in [15_000, 40_000] {
            store
                .step(
                    e1,
                    jt,
                    StepKind::Suspended,
                    None,
                    &serde_json::json!({"reason": "shutdown", "ran_ms": ms}),
                )
                .await
                .unwrap();
        }
        let now = Utc::now();
        for (n, ago) in [(1, 3000), (2, 2985), (3, 10)] {
            store
                .set_step_at(e1, jt, n, now - chrono::Duration::seconds(ago))
                .await
                .unwrap();
        }
        agents
            .ask(SendInput {
                channel: None,
                session: Some(e1),
                text: "more".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        let (_, text) = fake.prompt().await;
        assert_eq!(text, "more");
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnStarted { session, .. } if *session == e1),
        )
        .await;
        let (_, timed, _, age) = child_turn_of(&agents, e1).await;
        assert!(timed);
        assert!(
            (40..=42).contains(&age),
            "40 s ran before the restart: {age}"
        );
    }

    /// 178 F1: a native child turn drained at 30 s of run time, resumed,
    /// run 20 s more and drained again (`ran_ms` 50 s), then resumed after
    /// hours down: its age and budget count 50 s — each downtime never, the
    /// first run once.
    #[tokio::test]
    async fn twice_resumed_turn_counts_run_time_once() {
        use crate::durability::StepKind;
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 600;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let (t, now) = (TurnId::new(), Utc::now());
        let at = |secs| now - chrono::Duration::seconds(secs);
        let rows: [(StepKind, serde_json::Value, i64); 5] = [
            (
                StepKind::InputAccepted,
                serde_json::json!({"source": "user"}),
                9000,
            ),
            (StepKind::ModelCall, serde_json::json!({}), 8999),
            (
                StepKind::Suspended,
                serde_json::json!({"reason": "shutdown", "ran_ms": 30_000}),
                8970,
            ),
            (StepKind::ModelCall, serde_json::json!({}), 5000),
            (
                StepKind::Suspended,
                serde_json::json!({"reason": "shutdown", "ran_ms": 50_000}),
                4980,
            ),
        ];
        for (n, (kind, payload, ago)) in rows.into_iter().enumerate() {
            store.step(child, t, kind, None, &payload).await.unwrap();
            store
                .set_step_at(child, t, n as i64 + 1, at(ago))
                .await
                .unwrap();
        }
        agents.tell(resumed(child, t, at(4980))).await.unwrap();
        let (_, timed, _, age) = child_turn_of(&agents, child).await;
        assert!(timed);
        assert!((50..=52).contains(&age), "50 s ran, not 9000: {age}");
        let plan = store.resume_plan(child, t).await.unwrap();
        assert_eq!(plan.ran_ms, 50_000, "the plan's bound counts the same");
    }

    /// 178: a child turn whose `suspended` row carries a corrupt `ran_ms`
    /// (`u64::MAX`: only a damaged row) resumes timed on its journal span
    /// (20 s) and the actor lives on; past the store's guard the duration
    /// falls back to zero, never a panic.
    #[tokio::test]
    async fn resume_with_corrupt_ran_ms_does_not_panic() {
        use crate::durability::StepKind;
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).subagents.max_runtime_secs = 600;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let (t, now) = (TurnId::new(), Utc::now());
        let at = |secs| now - chrono::Duration::seconds(secs);
        journal_turn(&store, child, t, at(1000), at(990)).await;
        let p = serde_json::json!({"reason": "shutdown", "ran_ms": u64::MAX});
        store
            .step(child, t, StepKind::Suspended, None, &p)
            .await
            .unwrap();
        store.set_step_at(child, t, 3, at(980)).await.unwrap();
        agents.tell(resumed(child, t, at(980))).await.unwrap();
        let (_, timed, _, age) = child_turn_of(&agents, child).await;
        assert!(timed);
        assert!((20..=22).contains(&age), "the 20 s span: {age}");
        assert_eq!(store.resume_plan(child, t).await.unwrap().ran_ms, 20_000);
        assert!(crate::durability::ran_delta(u64::MAX).is_zero());
        assert!(crate::durability::ran_delta(i64::MAX as u64).is_zero());
    }

    /// 178 F2: a timer that fired for an earlier arm (its turn ended as it
    /// fired) never cancels the child's next turn; the current arm's does.
    #[tokio::test]
    async fn stale_runtime_fire_does_not_cancel_the_next_turn() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        let agents = Agents::spawn(a);
        let arm = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                agents
                    .ask(With(move |a: &mut Agents| a.child_turns[&child].arm))
                    .await
                    .unwrap()
            }
        };
        let hit = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                agents
                    .ask(With(move |a: &mut Agents| a.timed_out.contains(&child)))
                    .await
                    .unwrap()
            }
        };
        agents.tell(started(child, TurnId::new())).await.unwrap();
        let first = arm(&agents).await;
        agents.tell(finished(child)).await.unwrap();
        agents.tell(started(child, TurnId::new())).await.unwrap();
        let second = arm(&agents).await;
        assert_ne!(first, second);
        agents.tell(ChildTimedOut(child, first)).await.unwrap();
        assert!(!hit(&agents).await, "a stale fire is ignored");
        agents.tell(ChildTimedOut(child, second)).await.unwrap();
        assert!(hit(&agents).await, "the current arm's fire cancels");
    }

    /// 178 F5: unloading a child (a release, a transfer) drops its turn's
    /// timer entry with the actor, so the child's next turn — after it is
    /// loaded again — is armed instead of finding a stale entry.
    #[tokio::test]
    async fn next_turn_after_unloaded_child_is_capped() {
        let (_scratch, mut a) = agents().await;
        let (root, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, root, "kid").await;
        // Its turn was timed when the session was unloaded (a release).
        a.child_turns.insert(
            child,
            ChildTurn {
                permit: None,
                timer: None,
                arm: 1,
                started_at: Utc::now(),
            },
        );
        assert!(a.unload_session(child).await);
        assert!(!a.child_turns.contains_key(&child), "gone with the actor");
        // Loaded again (the lazy path a later input takes), its next turn is timed.
        a.session(Some(child), Some((root, "kid")), None, (None, None))
            .await
            .unwrap();
        let agents = Agents::spawn(a);
        agents.tell(started(child, TurnId::new())).await.unwrap();
        let (present, timed, _, _) = child_turn_of(&agents, child).await;
        assert!(present && timed, "the next turn is capped");
    }

    /// 179 D1: a pause suspends the desk's turn and its child's as a
    /// graceful quit does — `suspended{paused, ran_ms}` first, the actors
    /// stopped — and holds the tree for the un-pause.
    #[tokio::test]
    async fn pause_suspends_the_desk_tree() {
        let (_scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).durability.drain_secs = 1;
        let store = a.store.clone();
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        let now = Utc::now();
        let turns = [(desk, TurnId::new(), 4_000), (child, TurnId::new(), 9_000)];
        for (s, t, ran) in turns {
            // Started a minute ago (a `ran_ms` beyond the journal's span reads as corrupt, 178).
            let ago = now - chrono::Duration::seconds(60);
            journal_turn(&store, s, t, ago, ago).await;
            crate::durability::run_clock::start(s, t, ran);
        }
        let agents = Agents::spawn(a);
        let report = agents.ask(PauseTree { desk }).await.unwrap();
        assert_eq!(
            report.suspended,
            turns.iter().map(|(s, t, _)| (*s, *t)).collect::<Vec<_>>(),
            "desk first"
        );
        for (s, t, ran) in turns {
            let rows = store.suspended_rows(s, t).await.unwrap();
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].0, pause::PAUSED);
            assert!(rows[0].1.is_some_and(|ms| ms >= ran), "ran_ms: {rows:?}");
            let plan = store.resume_plan(s, t).await.unwrap();
            assert_eq!(plan.kind, crate::durability::ResumeKind::Suspended);
            assert!(plan.ran_ms >= ran, "the resume continues the run clock");
        }
        let (live, held) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.live(desk).is_some() || a.live(child).is_some(),
                    a.paused.contains_key(&desk),
                )
            }))
            .await
            .unwrap();
        assert!(!live, "no actor left running");
        assert!(held, "the tree is held");
    }

    /// 179 D8: at boot the open turns of a tree whose agent file says
    /// `paused: true` are held — held from the file at construction, not
    /// resumed by the scan, not by a Resume press, not offered as a Resume
    /// item — while another agent's resume.
    #[tokio::test]
    async fn resume_scan_skips_a_paused_agents_sessions() {
        let scratch = crate::test_dir("179-scan");
        let dir = scratch.path().to_path_buf();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::write(
            dir.join("agents").join("nap.md"),
            "---\npaused: true\n---\nNap.\n",
        )
        .unwrap();
        std::fs::write(dir.join("agents").join("ops.md"), "Ops.\n").unwrap();
        let seed = agents_on(dir.clone(), None, false).await;
        let store = seed.store.clone();
        let mut open = Vec::new();
        for agent in ["nap", "ops"] {
            let (desk, child) = (SessionId::new(), SessionId::new());
            store
                .create_session(desk, "m", None, None, None)
                .await
                .unwrap();
            store.set_desk(desk, Some(agent)).await.unwrap();
            store
                .create_session(child, "m", Some(desk), Some("kid"), None)
                .await
                .unwrap();
            for s in [desk, child] {
                let t = TurnId::new();
                journal_turn(&store, s, t, Utc::now(), Utc::now()).await;
                open.push((agent, s, t));
            }
        }
        drop(seed);
        store.close().await;
        // The next boot over the same data dir.
        let mut a = agents_on(dir, None, false).await;
        Arc::make_mut(&mut a.cfg).durability.resume = crate::durability::ResumeMode::Auto;
        let rows = a.open_here().await;
        let (_, native) = a.unpaused_turns(Vec::new(), rows).await;
        a.resume_scan(&native).await;
        for (agent, s, _) in &open {
            assert_eq!(a.sessions.contains_key(s), *agent == "ops", "{agent} {s}");
        }
        let (_, nap, t) = open[0];
        a.resume_turn(nap, t, None, None, None).await;
        assert!(!a.sessions.contains_key(&nap), "a Resume press waits too");
        let agents = Agents::spawn(a);
        let items = agents.ask(OpenTurns(Scope::Here)).await.unwrap();
        assert!(
            items
                .iter()
                .all(|(s, _, _)| open.iter().any(|(ag, o, _)| o == s && *ag == "ops")),
            "no Resume item for a paused agent: {items:?}"
        );
    }

    /// 182 M2a: with paired mesh peers a desk tree's open turn is held at
    /// boot — not resumed by the scan — until the roster lifts the hold after
    /// the first sync; without peers it resumes as before.
    #[tokio::test]
    async fn boot_resume_waits_for_first_sync() {
        for peers in [true, false] {
            let scratch = crate::test_dir("182-sync-hold");
            let dir = scratch.path().to_path_buf();
            std::fs::create_dir_all(dir.join("agents")).unwrap();
            std::fs::write(dir.join("agents").join("ops.md"), "Ops.\n").unwrap();
            if peers {
                let pool = sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_with(
                        sqlx::sqlite::SqliteConnectOptions::new()
                            .filename(dir.join(mesh::DB))
                            .create_if_missing(true),
                    )
                    .await
                    .unwrap();
                sqlx::query("CREATE TABLE peers (id TEXT PRIMARY KEY)")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO peers (id) VALUES ('peer-b')")
                    .execute(&pool)
                    .await
                    .unwrap();
                pool.close().await;
            }
            let seed = agents_on(dir.clone(), None, false).await;
            let store = seed.store.clone();
            let desk = SessionId::new();
            store
                .create_session(desk, "m", None, None, None)
                .await
                .unwrap();
            store.set_desk(desk, Some("ops")).await.unwrap();
            let t = TurnId::new();
            journal_turn(&store, desk, t, Utc::now(), Utc::now()).await;
            drop(seed);
            store.close().await;
            let mut a = agents_on(dir, None, false).await;
            Arc::make_mut(&mut a.cfg).durability.resume = crate::durability::ResumeMode::Auto;
            assert_eq!(
                a.paused.contains_key(&desk),
                peers,
                "held at boot iff the mesh has peers"
            );
            let rows = a.open_here().await;
            let (_, native) = a.unpaused_turns(Vec::new(), rows).await;
            a.resume_scan(&native).await;
            assert_eq!(
                a.sessions.contains_key(&desk),
                !peers,
                "peers={peers}: resumed by the scan"
            );
            if peers {
                let agents = Agents::spawn(a);
                let report = agents.ask(ResumeTree { desk }).await.unwrap();
                assert_eq!(
                    report.resumed,
                    vec![(desk, t)],
                    "the roster's lift after the first sync resumes it"
                );
            }
        }
    }

    /// 179 D2: un-pausing resumes every open turn of the tree, desk first,
    /// through the normal resume path; each plan carries the run time the
    /// pause measured (178: a child's cap continues, it does not restart).
    #[tokio::test]
    async fn unpause_resumes_the_tree_with_remaining_caps() {
        use crate::durability::StepKind;
        let (_scratch, mut a) = agents().await;
        let store = a.store.clone();
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        let now = Utc::now();
        let turns = [(desk, TurnId::new()), (child, TurnId::new())];
        for (s, t) in turns {
            journal_turn(
                &store,
                s,
                t,
                now - chrono::Duration::seconds(3600),
                now - chrono::Duration::seconds(3570),
            )
            .await;
            let p = serde_json::json!({"reason": "paused", "ran_ms": 30_000});
            store
                .step(s, t, StepKind::Suspended, None, &p)
                .await
                .unwrap();
            store
                .set_step_at(s, t, 3, now - chrono::Duration::seconds(3540))
                .await
                .unwrap();
            // The plan the resume runs on: the pause's run time, never the hour paused.
            assert_eq!(store.resume_plan(s, t).await.unwrap().ran_ms, 30_000);
        }
        // As the pause left it: no actors, the tree held.
        a.unload_session(desk).await;
        a.unload_session(child).await;
        a.paused.insert(desk, Vec::new());
        let mut rx = tap(&a.rt).await;
        let agents = Agents::spawn(a);
        let report = agents.ask(ResumeTree { desk }).await.unwrap();
        assert_eq!(report.resumed, turns.to_vec(), "desk first");
        // Both turns run again from their journal (in whatever order they publish).
        let mut seen = std::collections::HashSet::new();
        while seen.len() < turns.len() {
            if let AgentEvent::TurnResumed { session, turn, .. } = next_event(&mut rx, |e| {
                matches!(e, AgentEvent::TurnResumed { session, turn, .. } if turns.contains(&(*session, *turn)))
            })
            .await
            {
                seen.insert((session, turn));
            }
        }
        let (loaded, held) = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.sessions.contains_key(&desk) && a.sessions.contains_key(&child),
                    a.paused.contains_key(&desk),
                )
            }))
            .await
            .unwrap();
        assert!(loaded && !held, "resumed, hold lifted");
    }

    /// 179 D6: a harness input to a paused tree (a child's result, a
    /// reminder, a notice) starts no turn and gets no actor; it is held in
    /// order and routed once the tree resumes.
    #[tokio::test]
    async fn input_to_a_suspended_session_is_held_until_resume() {
        let (_scratch, mut a) = agents().await;
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        a.unload_session(desk).await;
        a.unload_session(child).await;
        a.paused.insert(desk, Vec::new());
        for (to, text) in [(desk, "notice"), (child, "result")] {
            a.inject(
                to,
                "session:x",
                text.into(),
                format!("<message from=\"session:x\">{text}</message>"),
                None,
            )
            .await;
        }
        assert!(
            !a.sessions.contains_key(&desk) && !a.sessions.contains_key(&child),
            "no actor, no turn"
        );
        let held: Vec<SessionId> = a.paused[&desk].iter().map(|(s, _)| *s).collect();
        assert_eq!(held, [desk, child], "arrival order");
        let agents = Agents::spawn(a);
        let report = agents.ask(ResumeTree { desk }).await.unwrap();
        assert_eq!(report.replayed, 2);
        let loaded = agents
            .ask(With(move |a: &mut Agents| {
                (
                    a.sessions.contains_key(&desk),
                    a.sessions.contains_key(&child),
                    a.paused.is_empty(),
                )
            }))
            .await
            .unwrap();
        assert_eq!(loaded, (true, true, true), "routed after the resume");
    }

    /// 179 D6: a person's text to a paused agent's session is refused with
    /// words the UI shows — never held where a restart would lose it.
    #[tokio::test]
    async fn human_input_to_a_paused_agent_is_refused() {
        let (_scratch, mut a) = agents().await;
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.store.set_desk(desk, Some("nap")).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        a.unload_session(desk).await;
        a.unload_session(child).await;
        a.paused.insert(desk, Vec::new());
        for s in [desk, child] {
            let e = a
                .human_input(
                    Some(s),
                    None,
                    "hello".into(),
                    Vec::new(),
                    None,
                    Delivery::Steer,
                )
                .await
                .unwrap_err();
            assert_eq!(e, "nap is paused — un-pause it to send");
        }
        assert!(a.paused[&desk].is_empty(), "nothing held");
        assert!(!a.sessions.contains_key(&desk), "no actor");
        a.paused.remove(&desk);
        assert!(
            a.human_input(
                Some(desk),
                None,
                "hello".into(),
                Vec::new(),
                None,
                Delivery::Steer
            )
            .await
            .is_ok(),
            "un-paused: accepted"
        );
    }

    /// An accepted exchange `from` → `to` (`request_id`), as the intake writes it.
    async fn accepted(a: &Agents, from: SessionId, to: SessionId, request_id: &str) -> Uuid {
        let generation = a.communication.0.write().route(from, to).unwrap();
        a.store
            .accept_exchange(&crate::store::communication::AcceptExchange {
                sender: &format!("session:{from}"),
                recipient: &format!("session:{to}"),
                sender_session: Some(from),
                recipient_session: Some(to),
                request_id: Some(request_id),
                reply_to: None,
                request_scope: None,
                generation: generation.revision() as i64,
                body: "hello desk",
            })
            .await
            .unwrap()
            .id
    }

    /// A desk `nap` (paused on disk) with a child, over `dir`'s store.
    async fn paused_desk(dir: &Path) -> (Agents, SessionId, SessionId) {
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::write(
            dir.join("agents").join("nap.md"),
            "---\npaused: true\n---\nNap.\n",
        )
        .unwrap();
        let mut a = agents_on(dir.to_path_buf(), None, true).await;
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        a.store.set_desk(desk, Some("nap")).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        (a, desk, child)
    }

    /// 179 D6/D8: a peer's message to a paused tree is not failed by the
    /// restart accounting; after the restart it is delivered at the un-pause.
    #[tokio::test]
    async fn peer_message_to_a_paused_tree_survives_a_restart() {
        let scratch = crate::test_dir("179-peer");
        let dir = scratch.path().to_path_buf();
        let (a, desk, child) = paused_desk(&dir).await;
        // The admission counter moves (any policy change); a restart resets it.
        a.communication.0.write().set_global(true, "command");
        let id = accepted(&a, child, desk, "msg-1").await;
        let store = a.store.clone();
        drop(a);
        // Boot: the accounting `start` runs, then the supervisor.
        let held = pause::boot_accounting(&store, &dir).await;
        assert_eq!(held, vec![("nap".to_owned(), desk)]);
        let pending = store.pending_exchanges(desk, 10).await.unwrap();
        assert_eq!(pending.len(), 1, "spared: {pending:?}");
        store.close().await;
        let b = agents_on(dir, None, true).await;
        assert!(b.paused.contains_key(&desk), "held from the file");
        let bstore = b.store.clone();
        let agents = Agents::spawn(b);
        let report = agents.ask(ResumeTree { desk }).await.unwrap();
        assert_eq!(report.replayed, 1);
        let (handed, live) = agents
            .ask(With(move |a: &mut Agents| {
                (a.enqueued.contains(&id), a.sessions.contains_key(&desk))
            }))
            .await
            .unwrap();
        assert!(handed && live, "delivered to the resumed desk");
        // Admitted again under the policy as it is now — never suppressed
        // as revoked by the generation from before the restart.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let state = bstore.exchange_receipt(id).await.unwrap().unwrap().state;
        assert_ne!(state, PeerMessageState::Suppressed);
    }

    /// Every `ReminderEvent` published on the runtime.
    #[derive(kameo::Actor)]
    struct ReminderTap(tokio::sync::mpsc::UnboundedSender<tools::ReminderEvent>);

    impl Message<tools::ReminderEvent> for ReminderTap {
        type Reply = ();
        async fn handle(&mut self, e: tools::ReminderEvent, _: &mut Context<Self, ()>) {
            let _ = self.0.send(e);
        }
    }

    /// A jobs actor on `rt` (evicting a dead one a previous body left registered).
    fn jobs_on(rt: &Runtime) -> ActorRef<Jobs> {
        if ActorRef::<Jobs>::lookup(JOBS_ACTOR)
            .ok()
            .flatten()
            .is_some_and(|j| !j.is_alive())
        {
            kameo::registry::ACTOR_REGISTRY
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(JOBS_ACTOR);
        }
        tools::jobs::start(rt, Duration::ZERO, Duration::from_secs(30)).unwrap()
    }

    /// `session`'s armed reminder ids.
    async fn armed_of(jobs: &ActorRef<Jobs>, session: SessionId) -> Vec<String> {
        jobs.ask(tools::jobs::LiveCounts)
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.session == session)
            .flat_map(|r| r.reminders)
            .collect()
    }

    /// 179 MUST-FIX 2: a pause stops the tree's reminder timers (the rows
    /// stay) and the un-pause arms them again.
    #[test]
    fn unpause_rearms_the_trees_reminders() {
        crate::with_test_registry(async {
            let (_scratch, mut a) = agents().await;
            let jobs = jobs_on(&a.rt);
            let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
            let far = Utc::now().timestamp() + 600;
            a.store
                .insert_reminder(&crate::store::ReminderRow {
                    id: "r1".into(),
                    session: desk,
                    message: "x".into(),
                    set_at: far - 600,
                    fires_at: far,
                    seconds: 600,
                })
                .await
                .unwrap();
            a.rearm_reminders_for(Some(desk)).await;
            let wait_for = |want: usize| {
                let jobs = jobs.clone();
                async move {
                    for _ in 0..100 {
                        if armed_of(&jobs, desk).await.len() == want {
                            return true;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    false
                }
            };
            assert!(wait_for(1).await, "armed before the pause");
            let agents = Agents::spawn(a);
            agents.ask(PauseTree { desk }).await.unwrap();
            assert!(wait_for(0).await, "the pause stops the timer");
            agents.ask(ResumeTree { desk }).await.unwrap();
            assert!(wait_for(1).await, "the un-pause arms it again");
            assert_eq!(armed_of(&jobs, desk).await, ["r1"]);
        });
    }

    /// 179 MUST-FIX 3: a paused desk with no open turn, an overdue reminder
    /// and a spared child result runs nothing at boot — the hold comes from
    /// the agent file before the reminders re-arm and the results go — and
    /// at the un-pause both run once.
    #[test]
    fn boot_holds_a_paused_desk_without_an_open_turn() {
        crate::with_test_registry(async {
            let scratch = crate::test_dir("179-boot");
            let dir = scratch.path().to_path_buf();
            let (a, desk, child) = paused_desk(&dir).await;
            let past = Utc::now().timestamp() - 60;
            a.store
                .insert_reminder(&crate::store::ReminderRow {
                    id: "r1".into(),
                    session: desk,
                    message: "standup".into(),
                    set_at: past - 600,
                    fires_at: past,
                    seconds: 600,
                })
                .await
                .unwrap();
            let result = accepted(&a, child, desk, &format!("result:{}", TurnId::new())).await;
            let store = a.store.clone();
            drop(a);
            pause::boot_accounting(&store, &dir).await;
            store.close().await;

            let mut b = agents_on(dir, None, true).await;
            let jobs = jobs_on(&b.rt);
            let (tx, mut fired) = tokio::sync::mpsc::unbounded_channel();
            let tap = ReminderTap::spawn(ReminderTap(tx));
            b.rt.subscribe::<ReminderTap, tools::ReminderEvent>(&tap)
                .await;
            // The boot pass's steps that used to wake it.
            b.rearm_reminders_for(None).await;
            let open = b.open_here().await;
            b.deliver_spared_results(&open).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(armed_of(&jobs, desk).await.is_empty(), "no reminder armed");
            assert!(fired.try_recv().is_err(), "no reminder fired");
            assert!(!b.sessions.contains_key(&desk), "no actor, no turn");
            assert!(!b.enqueued.contains(&result), "the result waits");
            assert_eq!(b.store.pending_exchanges(desk, 10).await.unwrap().len(), 1);

            let agents = Agents::spawn(b);
            let report = agents.ask(ResumeTree { desk }).await.unwrap();
            assert_eq!(report.replayed, 1, "the result, once");
            let e = tokio::time::timeout(Duration::from_secs(5), fired.recv())
                .await
                .expect("the overdue reminder fires")
                .unwrap();
            assert!(
                matches!(&e, tools::ReminderEvent::Fired { session, id } if *session == desk && id == "r1"),
                "{e:?}"
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(fired.try_recv().is_err(), "once");
            let handed = agents
                .ask(With(move |a: &mut Agents| a.enqueued.contains(&result)))
                .await
                .unwrap();
            assert!(handed, "the result is handed to the desk");
            tap.kill();
        });
    }

    /// 179 SHOULD-FIX 8: a child's brief held by a pause starts no cap; the
    /// cap runs from the turn's real start after the un-pause.
    #[tokio::test]
    async fn held_child_cap_excludes_paused_time() {
        let (_scratch, mut a) = agents().await;
        let (desk, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, desk, "kid").await;
        a.paused.insert(desk, Vec::new());
        let ticket = a.ledger.admit(child, InputOrigin::Automatic, None);
        a.start_child_turn(child, None, "brief".into(), ticket, None)
            .await;
        assert_eq!(a.paused[&desk].len(), 1, "the brief is held");
        assert!(
            a.child_turns.get(&child).is_none_or(|c| c.timer.is_none()),
            "no cap while held"
        );
        tokio::time::sleep(Duration::from_millis(2100)).await;
        let agents = Agents::spawn(a);
        // The brief's turn starts after the un-pause.
        agents.tell(started(child, TurnId::new())).await.unwrap();
        let (present, timed, _, age) = child_turn_of(&agents, child).await;
        assert!(present && timed, "capped from its real start");
        assert!(age < 2, "the paused time is not counted: {age}");
    }

    /// A local ACP harness whose first `$HANG` prompts never answer on their
    /// own (a turn mid-way when the pause lands); a later prompt answers
    /// `pong`. A cancel after the first prompt ends the pending one
    /// `cancelled`; the first prompt's (the pause's) is never answered.
    const HANGING_ACP: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | grep -o '"id":[^,}]*' | head -n1 | cut -d: -f2-)
  method=$(printf '%s' "$line" | grep -o '"method":"[^"]*"' | head -n1 | cut -d'"' -f4)
  [ -n "$method" ] && echo "$method" >> "$DIR/methods"
  prompts=$(grep -c '^session/prompt$' "$DIR/methods")
  case "$method" in
    initialize) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}' ;;
    session/new) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"fake-1"}}' ;;
    session/load) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{}}' ;;
    session/prompt)
      if [ "$prompts" -gt $HANG ]; then
        echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fake-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"pong"}}}}'
        echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}'
      else
        echo "$id" > "$DIR/pending"
      fi ;;
    session/cancel)
      if [ "$prompts" -gt 1 ]; then
        echo '{"jsonrpc":"2.0","id":'"$(cat "$DIR/pending")"',"result":{"stopReason":"cancelled"}}'
      fi ;;
  esac
done
"#;

    /// A root `p` with an ACP child `c` mid-turn on [`HANGING_ACP`] with
    /// `hang` hanging prompts; the pause drains for 2 s.
    async fn acp_mid_turn(
        hang: usize,
    ) -> (
        crate::TestDir,
        ActorRef<Agents>,
        Store,
        SessionId,
        SessionId,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) {
        let (scratch, mut a) = agents().await;
        Arc::make_mut(&mut a.cfg).durability.drain_secs = 2;
        let dir = scratch.path().to_path_buf();
        let script = dir.join("hang-acp.sh");
        std::fs::write(
            &script,
            HANGING_ACP
                .replace("$DIR", &dir.display().to_string())
                .replace("$HANG", &hang.to_string()),
        )
        .unwrap();
        let yaml = format!(
            "fake:\n  command: /bin/sh\n  args: [\"{}\"]\n  mcp: false\n  skills: false\n",
            script.display()
        );
        acp::save(&a.rt.data_dir, &serde_yaml::from_str(&yaml).unwrap()).unwrap();
        // Results reach the parent (a cancelled one says so).
        a.communication.0.write().set_global(true, "command");
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, c, _) = agents
            .ask(SpawnChild {
                agent: Some("fake".into()),
                cwd: Some(dir.clone()),
                ..spawn(p, "c", "ping")
            })
            .await
            .unwrap();
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnStarted { session, .. } if *session == c),
        )
        .await;
        let methods = dir.join("methods");
        for _ in 0..200 {
            if std::fs::read_to_string(&methods).is_ok_and(|m| m.contains("session/prompt")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        (scratch, agents, store, p, c, rx)
    }

    /// How many prompts the harness of [`acp_mid_turn`] has read.
    fn prompts(scratch: &crate::TestDir) -> usize {
        std::fs::read_to_string(scratch.path().join("methods"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l == "session/prompt")
            .count()
    }

    /// The roster rows of `p` as `(name, agent, model)`, by name.
    async fn model_rows(a: &Agents, p: SessionId) -> Vec<(String, Option<String>, String)> {
        let mut rows: Vec<_> = a
            .view()
            .agents(p)
            .await
            .into_iter()
            .map(|r| (r.name, r.agent, r.model))
            .collect();
        rows.sort();
        rows
    }

    /// Subagents card (2026-11-17): a roster row's `model` is the child's
    /// effective model, never the harness name its `model` column holds. An
    /// ACP child has none, then the model its spawn asked for, then
    /// `harness_model`; a native child has its session model; a reopened
    /// store (a restart: the ask is forgotten) still shows `harness_model`.
    #[tokio::test]
    async fn roster_rows_carry_the_effective_model() {
        let (scratch, mut a) = agents_with(false).await;
        let p = a
            .create_durable(None, None, None, None, None, None)
            .await
            .unwrap();
        let (acp, native) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(acp, "omp", Some(p), Some("worker"), None)
            .await
            .unwrap();
        a.store.set_acp(acp, "omp", None, &[]).await.unwrap();
        a.store
            .create_session(
                native,
                "anthropic/claude-native",
                Some(p),
                Some("scout"),
                None,
            )
            .await
            .unwrap();
        let scout = (
            "scout".to_owned(),
            None,
            "anthropic/claude-native".to_owned(),
        );
        let rows = |m: &str| {
            vec![
                scout.clone(),
                ("worker".to_owned(), Some("omp".to_owned()), m.to_owned()),
            ]
        };
        assert_eq!(model_rows(&a, p).await, rows(""));
        a.requested_models.insert(acp, "openai/gpt-asked".into());
        assert_eq!(model_rows(&a, p).await, rows("openai/gpt-asked"));
        a.store
            .set_harness_model(acp, "openai/gpt-6.1-sol")
            .await
            .unwrap();
        assert_eq!(model_rows(&a, p).await, rows("openai/gpt-6.1-sol"));
        drop(a);
        let a = agents_on(scratch.path().to_path_buf(), None, false).await;
        assert!(a.requested_models.is_empty());
        assert_eq!(model_rows(&a, p).await, rows("openai/gpt-6.1-sol"));
    }

    /// Subagents card (2026-11-17): `SubagentSpawned` carries the harness and
    /// the roster's model, so the parent's row needs no reload; the model
    /// the harness reports reaches the parent's subtree as
    /// `HarnessModelChanged` on the child, and the roster shows it. A
    /// native spawn carries its session model.
    #[tokio::test]
    async fn spawned_children_announce_their_model() {
        let (scratch, a) = agents().await;
        let dir = scratch.path().to_path_buf();
        let script = dir.join("model-acp.sh");
        let options = r#""configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"fake/m1","options":[{"value":"fake/m1","name":"M1"}]}]"#;
        std::fs::write(
            &script,
            HANGING_ACP
                .replace("$DIR", &dir.display().to_string())
                .replace("$HANG", "0")
                .replace(
                    r#""sessionId":"fake-1"}}"#,
                    &format!(r#""sessionId":"fake-1",{options}}}}}"#),
                ),
        )
        .unwrap();
        let yaml = format!(
            "fake:\n  command: /bin/sh\n  args: [\"{}\"]\n  mcp: false\n  skills: false\n",
            script.display()
        );
        acp::save(&a.rt.data_dir, &serde_yaml::from_str(&yaml).unwrap()).unwrap();
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, c, _) = agents
            .ask(SpawnChild {
                agent: Some("fake".into()),
                cwd: Some(dir.clone()),
                ..spawn(p, "c", "ping")
            })
            .await
            .unwrap();
        // The connect may report before or after the spawn is announced.
        let (mut spawned, mut reported) = (None, false);
        tokio::time::timeout(Duration::from_secs(60), async {
            while spawned.is_none() || !reported {
                match rx.recv().await.expect("bus open") {
                    AgentEvent::SubagentSpawned {
                        child,
                        agent,
                        model,
                        ..
                    } if child == c => spawned = Some((agent, model)),
                    AgentEvent::HarnessModelChanged { session, model } if session == c => {
                        assert_eq!(model, "fake/m1");
                        reported = true;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("spawn and harness model announced");
        let (agent, model) = spawned.unwrap();
        assert_eq!(agent.as_deref(), Some("fake"));
        assert!(
            matches!(model.as_deref(), None | Some("fake/m1")),
            "never the harness name: {model:?}"
        );
        let row = agents
            .ask(ListAgents { session: p })
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == c)
            .unwrap();
        assert_eq!(
            (row.agent.as_deref(), row.model.as_str()),
            (Some("fake"), "fake/m1")
        );

        let (_, n, _) = agents.ask(spawn(p, "n", "ping")).await.unwrap();
        let AgentEvent::SubagentSpawned { agent, model, .. } = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::SubagentSpawned { child, .. } if *child == n),
        )
        .await
        else {
            unreachable!()
        };
        let native = store.session(n).await.unwrap().unwrap().model;
        assert!(!native.is_empty());
        assert_eq!((agent, model), (None, Some(native)));
    }

    /// 179 SHOULD-FIX 7: an ACP child cut by the pause keeps its journal
    /// turn open (its cancel hands the paused parent nothing) and is
    /// re-prompted on the un-pause.
    #[tokio::test]
    async fn paused_acp_child_resumes_on_unpause() {
        let (scratch, agents, store, p, c, mut rx) = acp_mid_turn(1).await;
        let report = agents.ask(PauseTree { desk: p }).await.unwrap();
        assert!(report.suspended.iter().any(|(s, _)| *s == c), "{report:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            store.open_turn(c).await.unwrap().is_some(),
            "journal left open"
        );
        assert!(
            store.peer_records(p, None, 10).await.unwrap().is_empty(),
            "no `[child turn cancelled]` for the paused parent"
        );
        let report = agents.ask(ResumeTree { desk: p }).await.unwrap();
        assert!(report.resumed.iter().any(|(s, _)| *s == c), "{report:?}");
        next_event(&mut rx, |e| {
            matches!(e, AgentEvent::TurnFinished { session, stop_reason, .. }
                if *session == c && stop_reason == "end_turn")
        })
        .await;
        assert_eq!(prompts(&scratch), 2, "re-prompted once");
    }

    /// 179 MUST-FIX 1: an un-pause sent while the pause still drains waits
    /// for the stop, then resumes the suspended turn — never skipping the
    /// draining actor as live and leaving the turn unheld.
    #[tokio::test]
    async fn unpause_during_the_pause_drain_resumes_after_the_stop() {
        let _flag = drain::DRAIN_FLAG.lock().await;
        drain::reset_draining();
        let (scratch, agents, store, p, c, mut rx) = acp_mid_turn(1).await;
        let t0 = Instant::now();
        agents.tell(PauseTree { desk: p }).await.unwrap();
        let report = agents.ask(ResumeTree { desk: p }).await.unwrap();
        assert!(
            t0.elapsed() >= Duration::from_millis(1500),
            "answered after the drain: {:?}",
            t0.elapsed()
        );
        assert!(report.resumed.iter().any(|(s, _)| *s == c), "{report:?}");
        next_event(&mut rx, |e| {
            matches!(e, AgentEvent::TurnFinished { session, stop_reason, .. }
                if *session == c && stop_reason == "end_turn")
        })
        .await;
        assert_eq!(prompts(&scratch), 2);
        // The supervisor closes the ACP journal turn off the same event.
        assert!(journal_closes(&store, c).await, "the turn ended");
        let (live, held) = agents
            .ask(With(move |a: &mut Agents| {
                (a.live(c).is_some(), a.paused.contains_key(&p))
            }))
            .await
            .unwrap();
        assert!(live && !held, "the resumed actor is kept, the hold lifted");
    }

    /// Whether `session`'s open journal turn closes within 5 s.
    async fn journal_closes(store: &Store, session: SessionId) -> bool {
        for _ in 0..200 {
            if store.open_turn(session).await.unwrap().is_none() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    /// 179 PAUSE3: the cut marker of a child the pause stopped (whose own
    /// `cancelled` end never came) goes with its re-prompt; the next real
    /// cancel is a turn end — journal closed, parent told, cap released.
    #[tokio::test]
    async fn acp_cancel_after_unpause_is_not_swallowed() {
        let _flag = drain::DRAIN_FLAG.lock().await;
        drain::reset_draining();
        let (scratch, agents, store, p, c, mut rx) = acp_mid_turn(2).await;
        agents.ask(PauseTree { desk: p }).await.unwrap();
        let report = agents.ask(ResumeTree { desk: p }).await.unwrap();
        assert!(report.resumed.iter().any(|(s, _)| *s == c), "{report:?}");
        let AgentEvent::TurnStarted { turn, .. } = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnStarted { session, .. } if *session == c),
        )
        .await
        else {
            unreachable!()
        };
        // The child's `PeerTurnFinished` goes to the registered supervisor
        // (none in a unit test); its text is seeded as the native tests do.
        agents
            .ask(With(move |a: &mut Agents| {
                a.turn_text.insert((c, turn), "partial".into());
            }))
            .await
            .unwrap();
        for _ in 0..200 {
            if prompts(&scratch) == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(prompts(&scratch), 2, "re-prompted, hanging");
        agents
            .ask(Cancel {
                session: c,
                keep: None,
                scope: CancelScope::Root,
                reason: crate::CancelReason::Stop,
            })
            .await
            .unwrap();
        next_event(&mut rx, |e| {
            matches!(e, AgentEvent::TurnFinished { session, stop_reason, .. }
                if *session == c && stop_reason == "cancelled")
        })
        .await;
        assert!(journal_closes(&store, c).await, "the journal turn closed");
        // Recorded for the parent as a cancelled result (an idle parent with
        // no tool registry never consumes it).
        let mut told = Vec::new();
        for _ in 0..200 {
            told = store.peer_records(p, None, 10).await.unwrap();
            if !told.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            told.iter()
                .any(|r| r.from == format!("session:{c}")
                    && r.text.contains("[child turn cancelled]")),
            "the parent is told: {told:?}"
        );
        let (present, timed, permit, _) = child_turn_of(&agents, c).await;
        assert!(
            !present && !timed && !permit,
            "cap timer and permit released"
        );
    }

    /// 178 W6: a child turn that ended in an error, a cancel or with nothing
    /// said reaches its parent with a line saying so; a good one does not.
    #[tokio::test]
    async fn hand_off_prefixes_error_cancelled_and_empty_results() {
        let (_scratch, mut a) = agents_with(true).await;
        let (p, _) = in_turn(&mut a).await;
        let child = kid(&mut a, p, "kid").await;
        let cases = [
            (
                "error",
                Some("provider said 503\nretry later"),
                "half",
                "[child turn ended: error — provider said 503]\nhalf",
            ),
            (
                "error",
                None,
                "",
                "[child turn ended: error — unknown error]\n",
            ),
            ("cancelled", None, "part", "[child turn cancelled]\npart"),
            (
                "end_turn",
                None,
                "  ",
                "[child turn ended with no reply]\n  ",
            ),
            ("end_turn", None, "fine", "fine"),
        ];
        for (stop, error, text, want) in cases {
            let ct = TurnId::new();
            child_turn(&mut a, child, ct, true);
            a.turn_text.insert((child, ct), text.into());
            if let Some(e) = error {
                a.child_errors.insert((child, ct), e.into());
            }
            a.child_finished(child, ct, stop).await;
            let got = a
                .store
                .peer_records(p, None, 1)
                .await
                .unwrap()
                .remove(0)
                .text;
            assert_eq!(got, want, "{stop}");
        }
        assert!(a.child_errors.is_empty(), "consumed with the result");
    }

    /// 178 W8: a queued child archived before its permit came never returns
    /// a result — the parent's `child_result` row on it goes, and the parent
    /// is told once.
    #[tokio::test]
    async fn drop_queued_clears_the_child_result_wait() {
        let (_scratch, a) = agents_with(true).await;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let mut rx = tap(&rt).await;
        let (agents, p) = rooted(a).await;
        let (_, _x, _) = agents
            .ask(SpawnChild {
                call_id: "c-x".into(),
                ..spawn(p, "x", "first")
            })
            .await
            .unwrap();
        let (_, y, queued) = agents
            .ask(SpawnChild {
                call_id: "c-y".into(),
                ..spawn(p, "y", "second")
            })
            .await
            .unwrap();
        assert!(queued.is_some(), "y waits for x's slot");
        let on_y = |rows: Vec<WaitRow>| rows.iter().any(|w| w.target == y.to_string());
        assert!(on_y(store.child_waits(Scope::Here).await.unwrap()));
        agents
            .ask(ArchiveSession {
                session: y,
                archived: true,
            })
            .await
            .unwrap();
        assert!(!on_y(store.child_waits(Scope::Here).await.unwrap()));
        let e = next_event(&mut rx, |e| {
            matches!(e, AgentEvent::MessageDelivered { session, from, .. }
                if *session == p && *from == format!("session:{y}"))
        })
        .await;
        let AgentEvent::MessageDelivered { text, .. } = e else {
            unreachable!()
        };
        assert!(
            text.contains("before it started; no result will come"),
            "{text}"
        );
    }

    /// 178 W8: deleting a child clears its parent's `child_result` row and
    /// tells the parent once.
    #[tokio::test]
    async fn deleted_child_clears_the_parent_wait() {
        let (_scratch, mut a) = agents_with(true).await;
        let (rt, store) = (a.rt.clone(), a.store.clone());
        let (p, _) = a.session(None, None, None, (None, None)).await.unwrap();
        let child = kid(&mut a, p, "gone").await;
        child_waits::child_spawned(&mut a, p, child, "call-g", None).await;
        let mut rx = tap(&rt).await;
        let agents = Agents::spawn(a);
        agents.ask(DeleteSession { session: child }).await.unwrap();
        assert!(store.child_waits(Scope::Here).await.unwrap().is_empty());
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::MessageDelivered { session, .. } if *session == p),
        )
        .await;
        let AgentEvent::MessageDelivered { text, .. } = e else {
            unreachable!()
        };
        assert!(text.starts_with("`gone` (session:"), "{text}");
        assert!(text.contains("was deleted; no result will come"), "{text}");
    }

    /// 109 LIVE: a respawned ACP child counts as running from the moment its
    /// handoff prompt is on the wire. `Respawn` goes through the ordinary
    /// `start` → `open` path, whose `TurnStarted` the supervisor already
    /// reads — so the child is `Running`, timed like a follow-up turn, and
    /// `Idle` again when the prompt returns. (The boot trigger's own gate is
    /// `acp_resume::tests::respawn_decision_table`; it only accepts a local
    /// harness, which no test can dial, so the actor is told directly here.)
    #[tokio::test]
    async fn respawned_acp_child_counts_as_running() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, _) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        fake.handshake(Vec::new(), None).await;
        let (id, _) = fake.prompt().await;
        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        let state = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                let rows = agents.ask(ListAgents { session: p }).await.unwrap();
                let row = rows.into_iter().find(|r| r.id == e1).unwrap();
                (row.state, row.started_at.is_some())
            }
        };
        assert_eq!(state(&agents).await, (AgentState::Idle, false));

        let actor = agents
            .ask(With(move |a: &mut Agents| match &a.sessions[&e1].actor {
                LiveActor::Acp(a) => a.clone(),
                LiveActor::Native(_) => unreachable!(),
            }))
            .await
            .unwrap();
        actor.tell(crate::acp_child::Respawn(4000)).await.unwrap();
        let (id, text) = fake.prompt().await;
        assert!(text.starts_with("<resume_handoff>\n"), "{text}");
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnStarted { session, .. } if *session == e1),
        )
        .await;
        assert_eq!(state(&agents).await, (AgentState::Running, true));
        let timed = agents
            .ask(With(move |a: &mut Agents| {
                a.child_turns[&e1].timer.is_some()
            }))
            .await
            .unwrap();
        assert!(timed, "timed like any follow-up ACP turn");

        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        assert_eq!(state(&agents).await, (AgentState::Idle, false));
    }

    /// 176: a local harness — the `Fake` dials and runs no process — as a
    /// shell loop: one session, every prompt answered `pong`; each process
    /// appends its pid to `pids` and every method it reads to `methods`.
    const FAKE_ACP: &str = r#"echo $$ >> "$DIR/pids"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | grep -o '"id":[^,}]*' | head -n1 | cut -d: -f2-)
  method=$(printf '%s' "$line" | grep -o '"method":"[^"]*"' | head -n1 | cut -d'"' -f4)
  [ -n "$method" ] && echo "$method" >> "$DIR/methods"
  case "$method" in
    initialize) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}' ;;
    session/new) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"fake-1"}}' ;;
    session/load) echo '{"jsonrpc":"2.0","id":'"$id"',"result":{}}' ;;
    session/prompt)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fake-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"pong"}}}}'
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}' ;;
  esac
done
"#;

    /// 176 A1/A2: an idle local harness child past the threshold is parked
    /// — its process gone, the parent still sees it idle — and the next
    /// input resumes it with `session/load` and is answered.
    #[tokio::test]
    async fn idle_acp_child_parks_and_resumes() {
        let (scratch, a) = agents().await;
        let dir = scratch.path().to_path_buf();
        let script = dir.join("fake-acp.sh");
        std::fs::write(
            &script,
            FAKE_ACP.replace("$DIR", &dir.display().to_string()),
        )
        .unwrap();
        let yaml = format!(
            "fake:\n  command: /bin/sh\n  args: [\"{}\"]\n  mcp: false\n  skills: false\n",
            script.display()
        );
        acp::save(&a.rt.data_dir, &serde_yaml::from_str(&yaml).unwrap()).unwrap();
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let answered = |c: SessionId| {
            move |e: &AgentEvent| {
                matches!(e, AgentEvent::TurnFinished { session, stop_reason, .. }
                    if *session == c && stop_reason == "end_turn")
            }
        };
        let (_, c, _) = agents
            .ask(SpawnChild {
                agent: Some("fake".into()),
                cwd: Some(dir.clone()),
                ..spawn(p, "c", "ping")
            })
            .await
            .unwrap();
        next_event(&mut rx, answered(c)).await;
        let pids = || {
            std::fs::read_to_string(dir.join("pids"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let first = pids();
        assert_eq!(first.len(), 1, "{first:?}");

        // Swept until parked: a probe may time out under a loaded suite (the
        // live sweep simply retries a minute later).
        let parked = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                agents
                    .ask(With(move |a: &mut Agents| {
                        (a.live(c).is_some(), a.parked.contains_key(&c))
                    }))
                    .await
                    .unwrap()
            }
        };
        let t0 = Instant::now();
        loop {
            agents.ask(ParkIdle(Duration::ZERO)).await.unwrap();
            if parked(&agents).await == (false, true) {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(30), "never parked");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let alive = |pid: &str| {
            std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let t0 = Instant::now();
        while alive(&first[0]) {
            assert!(
                t0.elapsed() < Duration::from_secs(30),
                "harness still running"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let rows = agents.ask(ListAgents { session: p }).await.unwrap();
        let row = rows.into_iter().find(|r| r.id == c).unwrap();
        assert_eq!(row.state, AgentState::Idle, "parked reads idle, not failed");

        agents
            .ask(SendInput {
                channel: None,
                session: Some(c),
                text: "again".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        next_event(&mut rx, answered(c)).await;
        assert_eq!(pids().len(), 2, "a second harness process");
        let methods = std::fs::read_to_string(dir.join("methods")).unwrap();
        assert_eq!(
            methods.lines().collect::<Vec<_>>(),
            [
                "initialize",
                "session/new",
                "session/prompt",
                "initialize",
                "session/load",
                "session/prompt"
            ]
        );
        let parked = agents
            .ask(With(move |a: &mut Agents| a.parked.contains_key(&c)))
            .await
            .unwrap();
        assert!(!parked, "the resume took the park");

        // 176 R1: an input that arrives while a park round is in flight is
        // held and replayed — into the live child, or into the resume when
        // the child parked — never told into a stopping mailbox.
        agents.tell(ParkIdle(Duration::ZERO)).await.unwrap();
        agents
            .ask(SendInput {
                channel: None,
                session: Some(c),
                text: "during the round".into(),
                files: Vec::new(),
                delivery: Delivery::Steer,
                id: None,
            })
            .await
            .unwrap();
        next_event(&mut rx, answered(c)).await;
    }

    /// 176 R1: a park round never holds this mailbox. With the child's
    /// mailbox stuck on a `set_config_option` its harness has not answered,
    /// the round waits on the probe while `GetCompactStyle` answers at once.
    #[tokio::test]
    async fn park_round_runs_off_the_mailbox() {
        let (_scratch, a) = agents().await;
        let mut fake = node(&a, "box");
        let rt = a.rt.clone();
        let (agents, p) = rooted(a).await;
        rt.subscribe::<Agents, AgentEvent>(&agents).await;
        let mut rx = tap(&rt).await;
        let (_, e1, _) = agents.ask(spawn_on(p, "box", "e1", "go")).await.unwrap();
        let options = crate::acp_child::tests::omp_options;
        fake.handshake(options("anthropic/claude-opus-4-8"), None)
            .await;
        let (id, _) = fake.prompt().await;
        fake.end_turn(&id);
        next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::TurnFinished { session, .. } if *session == e1),
        )
        .await;
        let actor = agents
            .ask(With(move |a: &mut Agents| match &a.sessions[&e1].actor {
                LiveActor::Acp(a) => a.clone(),
                LiveActor::Native(_) => unreachable!(),
            }))
            .await
            .unwrap();
        let set = tokio::spawn(async move {
            actor
                .ask(crate::acp_child::SetOption {
                    config_id: "model".into(),
                    value: "openai/gpt-5.4".into(),
                })
                .await
                .is_ok()
        });
        let (option, ..) = fake.set_option().await;
        agents.tell(ParkIdle(Duration::ZERO)).await.unwrap();
        let in_flight = |agents: &ActorRef<Agents>| {
            let agents = agents.clone();
            async move {
                agents
                    .ask(With(move |a: &mut Agents| a.parking.contains_key(&e1)))
                    .await
                    .unwrap()
            }
        };
        // The child stays stuck for over a second; the supervisor answers
        // throughout.
        for _ in 0..3 {
            let t0 = Instant::now();
            agents.ask(GetCompactStyle).await.unwrap();
            let took = t0.elapsed();
            assert!(took < Duration::from_millis(100), "{took:?}");
            assert!(in_flight(&agents).await, "the round waits on the probe");
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        fake.respond(
            &option,
            agent_client_protocol::schema::v1::SetSessionConfigOptionResponse::new(options(
                "openai/gpt-5.4",
            )),
        );
        assert!(set.await.unwrap());
        let t0 = Instant::now();
        while in_flight(&agents).await {
            assert!(t0.elapsed() < Duration::from_secs(10), "round never ended");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let live = agents
            .ask(With(move |a: &mut Agents| a.live(e1).is_some()))
            .await
            .unwrap();
        assert!(live, "a dialled node child is never parked");
    }

    /// R2.7: a child result the parent's intake refuses (its pending quota
    /// is full) is an error on the parent naming the child and the reason,
    /// and is offered once more when the parent's next turn ends —
    /// delivered then, once the quota has room.
    #[tokio::test]
    async fn refused_child_result_is_visible_and_retried_once() {
        let (_scratch, mut a) = agents_with(true).await;
        let mut rx = tap(&a.rt).await;
        let (p, _) = in_turn(&mut a).await;
        let child = kid(&mut a, p, "worker").await;
        // Fill the parent's pending quota.
        let generation = a.communication.0.write().route(child, p).unwrap();
        let (from, to) = (format!("session:{child}"), format!("session:{p}"));
        let mut filler = Vec::new();
        for i in 0..crate::communication::PENDING_PER_RECIPIENT {
            let r = a
                .store
                .accept_exchange(&AcceptExchange {
                    sender: &from,
                    recipient: &to,
                    sender_session: Some(child),
                    recipient_session: Some(p),
                    request_id: Some(&format!("fill-{i}")),
                    reply_to: None,
                    request_scope: None,
                    generation: generation.revision() as i64,
                    body: "x",
                })
                .await
                .unwrap();
            filler.push(r.id);
        }
        let ct = TurnId::new();
        a.ledger.entered(TurnEntered {
            session: child,
            turn: Some(ct),
        });
        a.ledger.returned(TurnReturned {
            session: child,
            turn: Some(ct),
        });
        a.turn_text.insert((child, ct), "done".into());
        a.child_finished(child, ct, "end_turn").await;
        let e = next_event(
            &mut rx,
            |e| matches!(e, AgentEvent::Error { session, .. } if *session == p),
        )
        .await;
        let AgentEvent::Error { message, .. } = e else {
            unreachable!()
        };
        assert!(
            message.starts_with(&format!(
                "result of worker (session:{child}) not delivered: "
            )) && message.contains("read it with session_read; offered once more"),
            "{message}"
        );
        assert_eq!(a.undelivered[&p].len(), 1);
        assert_eq!(a.ledger.activity(p), None, "nothing reached the parent");
        // Room again before the parent's turn ends: the retry delivers.
        for id in filler {
            a.store.fail_exchange(id, "drained").await.unwrap();
        }
        a.turn_finished(p, TurnId::new(), "end_turn").await;
        assert!(a.undelivered.is_empty(), "offered exactly once more");
        assert!(
            a.ledger.activity(p).is_some(),
            "the result is pending input on the parent now"
        );
    }

    /// 162 D3/D5/D6: Refresh lists a custom provider with the key now in the
    /// vault (rotated after start, so not the cached copy), merges the ids, and
    /// reloads the catalog; a failed listing and the name errors change nothing.
    #[tokio::test]
    async fn refresh_provider_models_lists_with_the_live_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        let vault = Arc::new(
            secrets::Vault::open(&dir, secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        vault.set("rwx", "sk-new", None, "test").await.unwrap();
        a.auth.vault = Some(vault.clone());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-new"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [{ "id": "m2", "object": "model" }, { "id": "m1", "object": "model" }]
            })))
            .mount(&server)
            .await;
        let base_url = format!("{}/v1/", server.uri());
        a.auth.gateways.write().unwrap().insert(
            "rwx".into(),
            crate::providers::Gateway {
                base_url: base_url.clone(),
                key: "sk-old".into(),
                runtime: None,
                protocol: None,
            },
        );

        let r = a.refresh_provider_models("rwx").await.unwrap();
        assert_eq!(r.added, ["m1", "m2"]);
        assert!(r.removed.is_empty());
        let p = &r.provider;
        assert_eq!(
            (p.name.as_str(), p.base_url.as_str(), p.models),
            ("rwx", base_url.as_str(), 2)
        );
        assert!(!p.builtin && p.configured && p.refreshable);
        assert!(a.catalog.get("rwx/m1").is_some(), "catalog reloaded");

        let r = a.refresh_provider_models("rwx").await.unwrap();
        assert!(r.added.is_empty() && r.removed.is_empty());
        assert_eq!(r.provider.models, 2);

        // The server refuses this key: an error, and models.json stays as it was.
        vault.set("rwx", "sk-bad", None, "test").await.unwrap();
        let before = std::fs::read(dir.join("models.json")).unwrap();
        let e = a.refresh_provider_models("rwx").await.unwrap_err();
        assert!(e.starts_with("listing rwx's models: "), "{e}");
        assert_eq!(std::fs::read(dir.join("models.json")).unwrap(), before);
        assert_eq!(
            a.catalog.get("rwx/m2").map(|(_, m)| m.id.as_str()),
            Some("m2")
        );

        assert_eq!(
            a.refresh_provider_models("anthropic").await.unwrap_err(),
            "anthropic's models come from the built-in catalog"
        );
        assert_eq!(
            a.refresh_provider_models("nope").await.unwrap_err(),
            "no provider nope"
        );
        // A cloud provider with no key (a local one may be keyless, 177 E4).
        a.auth.gateways.write().unwrap().insert(
            "bare".into(),
            crate::providers::Gateway {
                base_url: "https://bare.example/v1/".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            a.refresh_provider_models("bare").await.unwrap_err(),
            "bare has no key; set it first"
        );
        vault
            .set("openrouter", "sk-or", None, "test")
            .await
            .unwrap();
        let or = a.provider_info("openrouter").unwrap();
        assert!(or.builtin && or.configured && or.refreshable);
    }

    /// OpenCode Go is a built-in Providers row: Go's base, the bundled models,
    /// configured by the `opencode-go` secret, refreshable without one (its
    /// listing is public), not removable, and not a name a gateway may take.
    #[tokio::test]
    async fn opencode_go_is_a_builtin_row_keyed_by_its_secret() {
        let (_scratch, mut a) = agents().await;
        let vault = Arc::new(
            secrets::Vault::open(&a.rt.data_dir, secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        a.auth.vault = Some(vault.clone());
        let env_go = std::env::var_os("OPENCODE_GO_API_KEY").is_some();
        let p = a.provider_info("opencode-go").unwrap();
        assert_eq!(
            (p.base_url.as_str(), p.builtin, p.configured, p.has_key),
            (crate::backend::GO_BASE, true, env_go, env_go)
        );
        assert!(p.refreshable && !p.local && p.runtime.is_none());
        // Free-tier `*-free` ids (403 outside OpenCode) are not counted.
        let bundled = a
            .catalog
            .providers()
            .find(|p| p.id == "opencode-go")
            .unwrap();
        let paid = bundled
            .models
            .keys()
            .filter(|id| !id.ends_with("-free"))
            .count();
        assert!(paid > 0 && paid < bundled.models.len());
        assert_eq!(p.models as usize, paid);
        vault.set("opencode-go", "gk", None, "test").await.unwrap();
        let p = a.provider_info("opencode-go").unwrap();
        assert!(p.configured && p.has_key && p.refreshable);
        assert!(crate::providers::validate_name("opencode-go", false).is_err());
    }

    /// 177 R2-7: a listing that never answers (a black-holed LAN host) gives
    /// up after `LISTING_TIMEOUT` instead of holding the Agents mailbox.
    #[tokio::test]
    async fn hanging_listing_times_out() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let (_scratch, a) = agents().await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
            .mount(&server)
            .await;
        let g = crate::providers::Gateway {
            base_url: format!("{}/v1/", server.uri()),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let e = a.list_gateway("hang", &g).await.unwrap_err();
        assert!(e.contains("timed out after 10s"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// 177 E4–E7: a keyless local provider named after its runtime is listed
    /// with no `Authorization` header (listing) and the window its server reports
    /// (source `server`); an unknown window reads as the assumed 16k; a user
    /// value (`set_model_limits`) reaches the catalog and survives a refresh;
    /// clearing it makes the window unknown again.
    #[tokio::test]
    async fn keyless_local_provider_limits_from_server_then_user() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
        struct Models(Arc<parking_lot::Mutex<Vec<bool>>>);
        impl Respond for Models {
            fn respond(&self, r: &Request) -> ResponseTemplate {
                self.0.lock().push(r.headers.contains_key("authorization"));
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [
                    {"id": "big", "max_model_len": 32768}, {"id": "small"}]}))
            }
        }
        let (_scratch, mut a) = agents().await;
        let dir = a.rt.data_dir.clone();
        let server = MockServer::start().await;
        let auths = Arc::new(parking_lot::Mutex::new(Vec::new()));
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(Models(auths.clone()))
            .mount(&server)
            .await;
        a.auth.gateways.write().unwrap().insert(
            "omlx".into(),
            crate::providers::Gateway {
                base_url: format!("{}/v1/", server.uri()),
                ..Default::default()
            },
        );
        let r = a.refresh_provider_models("omlx").await.unwrap();
        assert_eq!(r.added, ["big", "small"]);
        let p = &r.provider;
        assert!(p.local && !p.has_key && p.configured && p.runtime.is_none());
        assert!(!auths.lock().is_empty() && auths.lock().iter().all(|a| !a));
        let row = |a: &Agents, i: usize| a.provider_models("omlx").unwrap()[i].clone();
        assert_eq!(
            row(&a, 0),
            ProviderModel {
                id: "big".into(),
                name: "big".into(),
                context: 32768,
                output: 0,
                source: models::LimitSource::Server,
                assumed_context: None,
            }
        );
        let small = row(&a, 1);
        assert_eq!(
            (small.context, small.source, small.assumed_context),
            (0, models::LimitSource::Unknown, Some(16_384))
        );
        let resolved = |a: &Agents| a.catalog.resolve("omlx/small", &|_| true).unwrap();
        // R1-1: unknown for turns, assumed for memory budgets only.
        assert_eq!(
            crate::tokens::window(&resolved(&a)),
            crate::tokens::UNKNOWN_WINDOW
        );
        assert_eq!(crate::tokens::memory_window(&resolved(&a)), 16_384);

        crate::providers::set_limits(&dir, &a.catalog, "omlx", "small", Some(8192), Some(1024))
            .unwrap();
        a.reload_catalog().await;
        assert_eq!(crate::tokens::window(&resolved(&a)), 8192);
        a.refresh_provider_models("omlx").await.unwrap();
        let small = row(&a, 1);
        assert_eq!(
            (small.context, small.output, small.source),
            (8192, 1024, models::LimitSource::User)
        );
        assert_eq!(row(&a, 0).source, models::LimitSource::Server);

        crate::providers::set_limits(&dir, &a.catalog, "omlx", "small", None, None).unwrap();
        a.reload_catalog().await;
        let small = row(&a, 1);
        assert_eq!(
            (small.context, small.source, small.assumed_context),
            (0, models::LimitSource::Unknown, Some(16_384))
        );
        assert!(
            crate::providers::set_limits(&dir, &a.catalog, "omlx", "nope", Some(1), None)
                .unwrap_err()
                .contains("no model omlx/nope")
        );
    }

    /// 162 D7: the one line every frontend prints after a refresh.
    #[test]
    fn refreshed_models_line() {
        let mut r = RefreshedModels {
            provider: ProviderInfo {
                name: "rwx".into(),
                base_url: "https://x/v1/".into(),
                models: 14,
                builtin: false,
                configured: true,
                refreshable: true,
                local: false,
                runtime: None,
                has_key: true,
            },
            added: vec!["a".into(), "b".into()],
            removed: vec!["c".into()],
        };
        assert_eq!(r.line(), "rwx: 14 models (+2, -1)");
        r.added.clear();
        r.removed.clear();
        assert_eq!(r.line(), "rwx: 14 models, no change");
    }
}
