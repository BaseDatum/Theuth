//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use ::search::SearchCategory;
use acp::EntryInfo;
use acp::registry::RegistryAgent;
use agent::durability::ResumeKind;
use agent::todo::{Status as TodoStatus, TodoSnapshot, TodoSource};
use agent::{
    AdvisorDefaults, AdvisorSeverity, AdvisorState, AdvisorStatus, Advisory, AgentEvent, AgentInfo,
    AgentState, AwsProfiles, CommunicationChanged, CommunicationState, CompactStyle,
    ContextAttachment, ContextEntry, ContextItem, ContextKind, ContextRemoval, ContextSnapshot,
    ContextTarget, HistoryEntry, Loaded, ModelInfo, PeerDelivered, PeerInfo, PeerMessageChanged,
    PeerMessagePage, PeerOrigin, PeerPage, PeerReceipt, PeerRecord, PeerScope, ProviderInfo,
    QueuedInput, SessionKind, SessionSummary, SkillBody, SkillMeta, Usage, VisionNote, VisionStage,
    VisionStatus,
};
use agent::{
    BroadcastReceipt, BroadcastRequest, BroadcastTargets, CleanupStatus, CloseResult, CostBucket,
    CostScope, Coverage, CreateSessionOptions, MemoryMode, Money, Retention, SessionCost,
    SessionOpen,
};
use iocraft::{KeyCode, KeyModifiers};
use knowledge::{Change, ChangeStatus, HarvestResult, History, Meta, Revision};
use models::Catalog;
use runtime::{InputId, SessionId, TurnId};
use tokio::sync::mpsc::UnboundedSender;
use tools::git::SyncResult;
use tools::{
    Answer, ApprovalId, ApprovalRequested, ApprovalResolved, ApprovalsState, AskId, AskRequested,
    AskResolved, Decision, Gate, JobEvent, JobView, McpAuth, McpProgress, McpServer, McpToolInfo,
    PolicyAction, RepoInfo,
};
use tools::{JobId, Key, PtyPage, PtyScreenReply, PtyViewport, TerminalSummary};
use uuid::Uuid;
use double::{DoubleAnswered, DoublePredicted};

use crate::broadcast::{Broadcast, BroadcastStage};
use crate::clip::Image;
use crate::dictation::{Dictating, Phase};
use crate::pathac::PathState;
use crate::search;
use crate::talk::Talking;

/// Everything the UI can receive: bus events plus replies to its own commands.
#[derive(Debug)]
pub enum UiEvent {
    /// Agent progress.
    Agent(AgentEvent),
    /// Where the calls go now (63 §0.3): `Some("attached · pid N · host")`
    /// while attached to another process's engine, `None` once this process
    /// is the engine. The shown session is re-selected on the new end.
    Engine(Option<String>),
    /// A tool call waits on a human.
    Approval(ApprovalRequested),
    /// An approval was answered (by anyone).
    Resolved(ApprovalResolved),
    /// The model asks the user questions (`ask_user`).
    Ask(AskRequested),
    /// An ask was answered or dismissed (by anyone).
    AskResolved(AskResolved),
    /// The double predicted a card's answer (126 §10); keyed by `question_id`.
    DoublePredicted(DoublePredicted),
    /// The double answered a card at L2/L3 (126 §10).
    DoubleAnswered(DoubleAnswered),
    /// Reply to [`Command::ListSessions`].
    Sessions(Vec<SessionSummary>),
    /// Reply to [`Command::Search`], tagged with the view generation that asked (49 §6.1).
    Search(u64, search::Reply),
    /// A concept was written or removed (bus event): open search re-asks.
    KnowledgeChanged(knowledge::KnowledgeChanged),
    /// Reply to [`Command::ListAgents`]: the session's subtree with live state.
    Agents(Vec<AgentInfo>),
    /// Reply to [`Command::ListActivity`]: every live or root session (62 §0.6).
    Activity(Vec<AgentInfo>),
    /// Reply to [`Command::Broadcast`]: the receipt (dry run or send), or the refusal.
    Broadcasted(Result<BroadcastReceipt, String>),
    /// Reply to [`Command::Load`] with `settled_only: false`: shown as is.
    History(SessionId, Loaded),
    /// Reply to [`Command::Load`] with `settled_only: true` (48 §5): shown
    /// only when it is settled and not older than what is on screen.
    Refreshed(SessionId, Loaded),
    /// Reply to [`Command::ContextItems`]: the `/shake` inventory.
    ContextItems {
        /// Session the inventory describes.
        session: SessionId,
        /// The `/shake` argument that asked for it.
        category: Option<ShakeCategory>,
        /// The inventory.
        snapshot: ContextSnapshot,
    },
    /// Reply to [`Command::RemoveContext`]: what was removed; the transcript reload follows.
    ContextRemoved(SessionId, ContextRemoval),
    /// Reply to [`Command::GetTodos`]: the session's latest committed plan
    /// (47 §5.1); `None` when the read failed, which marks the plan stale.
    Todos(SessionId, Option<TodoSnapshot>),
    /// Reply to [`Command::Fork`]: the new session, already loaded.
    Forked(SessionId, Loaded),
    /// Reply to [`Command::Send`]: the session the turn went to.
    SessionCreated(SessionId),
    /// Reply to [`Command::ListModels`].
    Models(Vec<ModelInfo>),
    /// Reply to [`Command::PathComplete`]: the generation that asked, `None` when the call failed.
    PathComplete(u64, Option<rpc::pathac::PathCompletion>),
    /// Reply to [`Command::SetModel`].
    ModelSet(ModelInfo),
    /// Reply to [`Command::SetEffort`]: the value now in force.
    EffortSet(Option<String>),
    /// Reply to [`Command::SetAdvisor`] / [`Command::AdvisorStatus`] (125 §7.1).
    AdvisorStatus(AdvisorStatus),
    /// Reply to [`Command::SetModelFavorite`]: the engine's favorite list now (127 §3).
    FavoriteModels(Vec<String>),
    /// Reply to [`Command::HarnessOptions`] (75 §8): the harness's option list.
    HarnessOptions(Vec<acp::ConfigOption>),
    /// Reply to [`Command::SetHarnessOption`]: the option key asked for, the value, and the result.
    HarnessOptionSet(String, String, acp::SetOptionResult),
    /// Reply to [`Command::ListMcp`] (also sent after every [`Command::SetGate`] / [`Command::SetHidden`]).
    Mcp(Vec<McpServer>),
    /// Reply to [`Command::SetApprovals`].
    Approvals(ApprovalsState),
    /// MCP reconcile snapshot (bus event).
    McpProgress(McpProgress),
    /// Reply to [`Command::ShowPrompt`].
    Prompt(String),
    /// Reply to [`Command::ListSkills`].
    Skills(Vec<SkillMeta>),
    /// Reply to [`Command::LoadSkill`].
    Skill(SkillBody),
    /// Reply to [`Command::AddSkillRepo`]: the skills the clone holds.
    SkillsAdded(Vec<SkillMeta>),
    /// Reply to [`Command::SyncSkills`] / [`Command::SyncMcp`].
    Synced(Vec<SyncResult>),
    /// Reply to [`Command::AddMcp`].
    McpAdded(McpServer),
    /// Reply to [`Command::AddMcpRepo`].
    RepoAdded(RepoInfo),
    /// Reply to [`Command::ListAcp`] (also sent after [`Command::RemoveAcp`]).
    Acp(Vec<EntryInfo>),
    /// Reply to [`Command::ListAcpRegistry`].
    AcpRegistry(Vec<RegistryAgent>),
    /// Reply to [`Command::AddAcp`].
    AcpAdded(EntryInfo),
    /// Reply to [`Command::KnowledgeList`] / [`Command::KnowledgeSearch`]: metas: best first for a search, by id for a list.
    Knowledge(Vec<Meta>),
    /// Reply to [`Command::KnowledgeHistory`]: the request's scope and page
    /// size; `history` holds up to `limit + 1` changes (the extra one means
    /// an older page exists).
    KnowledgeHistory {
        /// Concept id, `None` for the whole bundle.
        id: Option<String>,
        /// Rows asked for.
        limit: usize,
        /// The reply.
        history: History,
    },
    /// Reply to [`Command::KnowledgeRevision`]: the request's scope and the commit.
    KnowledgeRevision {
        /// Concept id filter, `None` for every concept the commit touched.
        id: Option<String>,
        /// The reply.
        revision: Revision,
        /// `/knowledge show`: a transcript card, not the history menu.
        card: bool,
    },
    /// Reply to [`Command::KnowledgeRestore`]: the restoring commit.
    KnowledgeRestored(Change),
    /// Reply to [`Command::Harvest`].
    Harvested(HarvestResult),
    /// A command succeeded with nothing to show but a notice.
    Info(String),
    /// Reply to [`Command::GetCompactStyle`].
    CompactStyle(CompactStyle),
    /// Reply to [`Command::GetTitleModel`] / [`Command::SetTitleModel`].
    TitleModel(Option<ModelInfo>),
    /// Reply to [`Command::GetVisionModel`] / [`Command::SetVisionModel`]:
    /// the harness-wide vision fallback (52 §3), `None` = off.
    VisionModel(Option<ModelInfo>),
    /// Reply to [`Command::ListAwsProfiles`] (`pick`) / [`Command::SetAwsProfile`].
    AwsProfiles {
        /// Discovered profiles and the harness-wide selection.
        info: AwsProfiles,
        /// Open the picker when there is a choice.
        pick: bool,
    },
    /// A command failed (actor missing, ask error).
    Error(String),
    /// Reply to [`Command::ListLogins`]: providers and OAuth MCP servers.
    Logins(Vec<LoginRow>),
    /// A provider login started by [`Command::Login`] ended (bus event).
    LoginFinished(agent::LoginFinished),
    /// Reply to [`Command::ListProviders`].
    Providers(Vec<ProviderInfo>),
    /// Reply to [`Command::AddProvider`].
    ProviderAdded(ProviderInfo),
    /// Reply to [`Command::Withdraw`]: the steer left the queue; its text goes back to the editor.
    Withdrawn {
        /// The withdrawn steer.
        id: InputId,
        /// Its text.
        text: String,
    },
    /// A `Send`/`Steer` the engine refused or never got: the text goes back to the editor.
    Unsent {
        /// The steer's id; `None` for a `Send`.
        id: Option<InputId>,
        /// Its text.
        text: String,
    },
    /// Reply to [`Command::Undo`]: first line of the turn now hidden.
    Undone(String),
    /// Reply to [`Command::Redo`]: first line of the turn restored.
    Redone(String),
    /// Reply to [`Command::ListJobs`].
    Jobs(Vec<JobView>),
    /// A background job started, wrote output or ended (bus event).
    Job(JobEvent),
    /// Reply to [`Command::ListSchedules`].
    Schedules(Vec<schedule::ScheduleInfo>),
    /// Reply to [`Command::ScheduleHistory`]: the schedule's name, its info
    /// when the file still exists, and its runs, newest first.
    ScheduleHistory(
        String,
        Option<schedule::ScheduleInfo>,
        Vec<schedule::RunInfo>,
    ),
    /// Reply to [`Command::ListTasks`]: the project asked for and its tasks (67 §12).
    Tasks(Option<String>, Vec<tasks::Task>),
    /// Reply to [`Command::ListRoster`] (67 §12).
    Roster(Vec<roster::AgentSummary>),
    /// Reply to [`Command::ComputerStatus`].
    Computer(computer::PermissionStatus),
    /// Reply to [`Command::LinkStatus`] and the link commands that change it.
    Link(link::LinkStatus),
    /// Reply to [`Command::LinkPair`]: the string the QR card encodes.
    Pairing(link::Pairing),
    /// Link plugin traffic (bus event).
    LinkEvent(link::LinkEvent),
    /// Reply to [`Command::MeshDevicesList`] (and the commands that change it): the mesh's replicated registry (69 §3).
    MeshDevices(Vec<link::DeviceInfo>),
    /// Reply to [`Command::MeshDevicesAdd`]: the 4-part string a device scans and its pending id (69 decision 4).
    MeshDevicePairing(link::PairingPayload),
    /// Reply to [`Command::SandboxList`]: every sandbox record (79 §8).
    Sandboxes(Vec<sandbox::SandboxInfo>),
    /// Reply to [`Command::SandboxDesktop`]: the noVNC URL for a browser (the password stays out of the transcript).
    SandboxDesktop(String, sandbox::types::DesktopAccess),
    /// The self-updater staged a newer version (39 §4.1).
    UpdateReady(update::UpdateReady),
    /// Reply to [`Command::ListNodes`] and the node commands that change the list.
    Nodes(Vec<nodes::Node>),
    /// Reply to [`Command::InviteNode`]: the node's name and the string its QR card encodes.
    NodeInvite(String, link::Pairing),
    /// Nodes plugin traffic (bus event).
    NodeEvent(nodes::NodeEvent),
    /// A `show` call produced a file (bus event, design doc 35).
    ArtifactShown(artifact::ArtifactShown),
    /// Live URLs of a `crawl` / `map_site` / `scrape_url` call (bus event, design doc 38).
    ScrapeProgress(web::progress::ScrapeProgress),
    /// Reply to [`Command::ListArtifacts`]: the session subtree's artifacts, newest first;
    /// `pick` opens the `/artifacts` (`/apps` when `apps`) menu, else the rows
    /// only join their `show` / `app` cards.
    Artifacts {
        /// The rows.
        items: Vec<artifact::Artifact>,
        /// Open the picker.
        pick: bool,
        /// Only [`artifact::Kind::App`] records (`/apps`, 46 §8.2).
        apps: bool,
        /// `list_sessions` when picking: the owners whose project the rows
        /// group under (51 §3); empty otherwise.
        sessions: Vec<SessionSummary>,
    },
    /// Reply to [`Command::ListSecrets`]: names, notes, versions. Never values.
    Secrets(secrets::SecretsList),
    /// Reply to [`Command::SetSecret`] / [`Command::RestoreSecret`]: the new version.
    SecretSet(secrets::SecretInfo),
    /// Secrets plugin traffic (bus event): a change, a `secret_request`, its resolution.
    SecretsEvent(secrets::SecretsEvent),
    /// Reply to [`Command::GetCommunication`], [`Command::SetCommunication`]
    /// and [`Command::SetPeerScope`]: the authoritative switches (50 §10);
    /// `state.session` says whose, so a reply for a session left behind is dropped.
    Communication(CommunicationState),
    /// Reply to [`Command::ListPeers`]: the permitted directory of `session`,
    /// or why there is none (communication off is an `Err`, not an empty page).
    Peers(SessionId, Result<PeerPage, String>),
    /// Reply to [`Command::ListPeerRequests`]: the requests a human may
    /// resolve for `session` right now, by exact id.
    PeerRequests(SessionId, PeerMessagePage),
    /// Reply to [`Command::AnswerPeerRequest`].
    PeerAnswered(PeerReceipt),
    /// Reply to [`Command::A2aStatus`] / [`Command::ReloadA2a`].
    A2a(a2a::A2aStatus),
    /// Communication switches or a root's scope changed (bus event): reload the state.
    CommunicationChanged(CommunicationChanged),
    /// A peer receipt changed state (bus event): reload the pending requests.
    PeerMessageChanged(PeerMessageChanged),
    /// A peer exchange entered a transcript (bus event): typed provenance, never parsed from text.
    PeerDelivered(PeerDelivered),
    /// Network configuration or listener state changed (bus event).
    A2aChanged(a2a::A2aChanged),
    /// Reply to [`Command::SessionCost`]: the owner summary of `session` (51 §6.5).
    Cost(SessionId, SessionCost),
    /// Reply to [`Command::ListTerminals`].
    Terminals(Vec<TerminalSummary>),
    /// Reply to [`Command::PtyScreen`], echoing the request's identity so a
    /// reply for a terminal or viewport left behind is dropped (51 §7.4).
    PtyScreen {
        /// Owning session asked.
        session: SessionId,
        /// Job asked.
        job: JobId,
        /// Viewport asked.
        viewport: PtyViewport,
        /// [`TerminalView::epoch`] when asked.
        epoch: u64,
        /// The page, or why there is none (`terminal busy` is retried).
        reply: Result<PtyScreenReply, String>,
    },
    /// Reply to [`Command::CreateSession`].
    SessionOpened(SessionOpen),
    /// Reply to [`Command::LeaveSession`]: the root left and how far its cleanup got.
    SessionLeft(SessionId, CloseResult),
    /// Voice plugin bus event; the `Dictation*` variants (54 §6) and `SpeechFinished` for the talk subtitle (76 §5.2).
    Voice(voice::VoiceEvent),
    /// Talk plugin bus event (76 §5.2): the `/talk` view.
    Talk(talk::TalkEvent),
    /// Reply to [`Command::DictationStart`]: the daemon opened a dictation for us.
    DictationStarting(voice::DictationId),
    /// [`Command::DictationStart`] was refused (already dictating, voice disabled, model missing).
    DictationFailed(String),
    /// Reply to [`Command::TalkStart`]: the daemon opened a talk for us.
    TalkStarting(talk::TalkId),
    /// [`Command::TalkStart`] was refused (already talking, talk disabled, model missing).
    TalkFailed(String),
}

/// Everything the UI can ask of the rest of the process. One task turns
/// these into `rpc::Api` calls and their replies into [`UiEvent`]s.
#[derive(Debug)]
pub enum Command {
    /// `send_input`.
    Send {
        /// `None` creates a session.
        session: Option<SessionId>,
        /// User text.
        text: String,
        /// Attached image files (17 §2).
        files: Vec<PathBuf>,
    },
    /// `steer`: queued until the current tool batch ends. `interject`: `cancel`
    /// first (one task, so the order holds), and the queued text starts the next turn now.
    Steer {
        /// Target.
        session: SessionId,
        /// Client-minted id, echoed by the `Input*` events.
        id: InputId,
        /// User text.
        text: String,
        /// Attached image files (17 §2).
        files: Vec<PathBuf>,
        /// Ctrl/Cmd+Enter: abort the in-flight turn before queueing.
        interject: bool,
    },
    /// `withdraw`: take a steer back out of the queue.
    Withdraw {
        /// Target.
        session: SessionId,
        /// The steer.
        id: InputId,
    },
    /// `undo`: hide the last turn.
    Undo(SessionId),
    /// `redo`: restore the last hidden turn.
    Redo(SessionId),
    /// `regenerate`: replay the turn opened at `turn`, dropping its old tail (29 §3).
    Regenerate {
        /// Target.
        session: SessionId,
        /// Seq of the user row that opens the turn.
        turn: i64,
    },
    /// `edit_turn`: replace the turn opened at `turn` with `text`.
    EditTurn {
        /// Target.
        session: SessionId,
        /// Seq of the user row that opens the turn.
        turn: i64,
        /// New user text.
        text: String,
    },
    /// `fork_session`: a new session with the conversation through `through` (all when `None`).
    Fork {
        /// Source.
        session: SessionId,
        /// Seq of the last turn to keep.
        through: Option<i64>,
    },
    /// `cancel`.
    Cancel(SessionId),
    /// `list_jobs` for the shown session.
    ListJobs(SessionId),
    /// `cancel_job`.
    CancelJob {
        /// Owning session.
        session: SessionId,
        /// Job id, e.g. `j3`.
        id: String,
    },
    /// `list_schedules`.
    ListSchedules,
    /// `fire_schedule`: run a schedule now.
    FireSchedule(String),
    /// `schedule_history`: the schedule's last runs.
    ScheduleHistory(String),
    /// `enable_schedule`: `/schedules pause|resume <name>` (33 §3.5).
    EnableSchedule(String, bool),
    /// `test_schedule`: `/schedules test <name>` — the 40 §1.12 verdict.
    TestSchedule(String),
    /// `tasks_list` for one project (`None` = every project) (67 §12).
    ListTasks(Option<String>),
    /// `agents_list`: the roster (67 §12).
    ListRoster,
    /// `session_take(id, "user")`: `/take` continues the open session on this engine (68 §10).
    TakeSession(SessionId),
    /// `computer_permissions`.
    ComputerStatus,
    /// `compact`.
    Compact {
        /// Target.
        session: SessionId,
        /// One-shot style; `None` = configured default.
        style: Option<CompactStyle>,
    },
    /// `set_session_memory` (137 §8.6 `/endless on|off`), or `session_memory` when `mode` is `None`.
    Memory {
        /// Target.
        session: SessionId,
        /// The mode to switch to; `None` = show the memory state.
        mode: Option<MemoryMode>,
    },
    /// `compact_style`.
    GetCompactStyle,
    /// `set_compact_style`.
    SetCompactStyle(CompactStyle),
    /// `title_model`.
    GetTitleModel,
    /// `set_title_model`; `None` clears it.
    SetTitleModel(Option<String>),
    /// `vision_model`.
    GetVisionModel,
    /// `set_vision_model`; `None` turns the fallback off.
    SetVisionModel(Option<String>),
    /// `list_aws_profiles`.
    ListAwsProfiles,
    /// `set_aws_profile`; `None` restores the AWS default credential chain.
    SetAwsProfile(Option<String>),
    /// `list_sessions`.
    ListSessions,
    /// One `/search` request (49 §6): `search`, `search_refresh`, a
    /// destination fetch or the page around an old message.
    Search(search::Request),
    /// `list_agents`: the subtree under a session.
    ListAgents(SessionId),
    /// `list_activity`: the broadcast picker's rows (62 §0.6).
    ListActivity,
    /// `broadcast` (62 §0.1); `dry_run` previews.
    Broadcast(BroadcastRequest),
    /// `load_session`; `settled_only` (48 §5) reads through the idle actor
    /// and replies with [`UiEvent::Refreshed`] instead of [`UiEvent::History`].
    Load {
        /// Session.
        session: SessionId,
        /// Refuse a busy session's half-persisted transcript.
        settled_only: bool,
    },
    /// `context_items`: the `/shake` inventory (48 §3.2).
    ContextItems {
        /// Session.
        session: SessionId,
        /// The `/shake` argument, echoed in the reply.
        category: Option<ShakeCategory>,
    },
    /// `get_todos`: the latest committed plan, readable mid-turn (47 §5.1).
    GetTodos(SessionId),
    /// `remove_context`, then `load_session(settled_only: true)`.
    RemoveContext {
        /// Session.
        session: SessionId,
        /// The inventory revision the review was made from.
        expected_revision: String,
        /// Exact targets, companions included.
        targets: Vec<ContextTarget>,
    },
    /// `resolve_approval`.
    Resolve {
        /// Request.
        id: ApprovalId,
        /// Answer.
        decision: Decision,
    },
    /// `answer_ask`; `answers: None` dismisses the set and cancels the turn.
    AnswerAsk {
        /// Request.
        id: AskId,
        /// One per question, in order.
        answers: Option<Vec<Answer>>,
    },
    /// `list_models`.
    ListModels,
    /// `path_complete` (PATH-AC): the token under `cursor` in `input`, listed by `session`'s engine.
    PathComplete {
        /// The listing session (`None`: this engine, its cwd).
        session: Option<SessionId>,
        /// The editor's text.
        input: String,
        /// Byte offset of the caret in `input`.
        cursor: usize,
        /// Echoed in the reply so a superseded request is inert.
        generation: u64,
    },
    /// `set_model`.
    SetModel {
        /// Session to switch now (`None`: only the default).
        session: Option<SessionId>,
        /// `provider/model` id.
        model: String,
    },
    /// `set_effort`.
    SetEffort {
        /// Session to switch from its next turn (`None`: only the default).
        session: Option<SessionId>,
        /// Effort keyword or token budget; `None` clears it (provider default).
        effort: Option<String>,
    },
    /// `set_advisor` (125 §7.1): `model: None` turns the advisor off.
    SetAdvisor {
        /// The advised session.
        session: SessionId,
        /// `provider/model` of the advisor; `None` = off.
        model: Option<String>,
        /// Its effort.
        effort: Option<String>,
    },
    /// `advisor_status` (125 §7.3).
    AdvisorStatus(SessionId),
    /// `set_model_favorite` (127 §3): star (`true`) or unstar `model` on the engine.
    SetModelFavorite {
        /// `provider/model` id.
        model: String,
        /// `true` stars, `false` unstars.
        favorite: bool,
    },
    /// `harness_options` on an ACP session (75 §8).
    HarnessOptions(SessionId),
    /// `set_harness_option` on an ACP session (75 §8): `config_id` is an
    /// option id or the alias `model` / `effort`.
    SetHarnessOption {
        /// The ACP session.
        session: SessionId,
        /// Option id or alias.
        config_id: String,
        /// One of the option's listed values.
        value: String,
    },
    /// `resume_child` on the open ACP session (75 §10).
    ResumeChild(SessionId),
    /// `list_mcp` for the root session (its in-memory hides).
    ListMcp(Option<SessionId>),
    /// `mcp_status`: the snapshot a late-attaching UI missed.
    McpStatus,
    /// `set_mcp_gate`, then `list_mcp`.
    SetGate {
        /// Server name.
        server: String,
        /// Tool name; `None` sets the server line.
        tool: Option<String>,
        /// New line; `None` clears it (back to annotations).
        gate: Option<Gate>,
        /// Root session, for the re-list.
        session: Option<SessionId>,
    },
    /// `set_mcp_hidden`, then `list_mcp`. Without a session yet, `new_session`
    /// first: the hide must exist before the first turn lists tools.
    SetHidden {
        /// Root session; `None` = create it.
        session: Option<SessionId>,
        /// Registered tool names.
        tools: Vec<String>,
        /// `true` hides.
        hidden: bool,
    },
    /// `set_approvals`.
    SetApprovals {
        /// `None` = global, persisted.
        session: Option<SessionId>,
        /// On or off.
        enabled: bool,
    },
    /// `/double …` (126 §10): one `double_*` call, answered with an `Info` line.
    Double(DoubleCmd),
    /// `get_prompt`.
    ShowPrompt(Option<SessionId>),
    /// `list_skills`.
    ListSkills,
    /// `load_skill`.
    LoadSkill(String),
    /// `add_skill_repo`.
    AddSkillRepo(String),
    /// `remove_skill_repo`, falling back to `remove_skill`.
    RemoveSkill(String),
    /// `sync_skills`.
    SyncSkills,
    /// `use_skill`: hand a skill to the model (04 §2.4).
    UseSkill {
        /// Shown session, or `None` to start one.
        session: Option<SessionId>,
        /// Skill name.
        name: String,
        /// The user's words after the name.
        args: String,
    },
    /// `knowledge_list` (first 50 by id).
    KnowledgeList,
    /// `knowledge_search`.
    KnowledgeSearch(String),
    /// `knowledge_history` for `limit + 1` rows; `id: None` for the whole bundle.
    KnowledgeHistory {
        /// Concept id.
        id: Option<String>,
        /// Rows to show; one more is fetched to learn whether older ones exist.
        limit: usize,
    },
    /// `knowledge_revision`: one commit against its first parent.
    KnowledgeRevision {
        /// Abbreviated commit.
        commit: String,
        /// Concept id filter.
        id: Option<String>,
        /// `/knowledge show`: print a card rather than fill the history menu.
        card: bool,
    },
    /// `knowledge_restore`.
    KnowledgeRestore {
        /// Concept id.
        id: String,
        /// Abbreviated commit; default the one before the latest change.
        commit: Option<String>,
    },
    /// `knowledge_harvest` for the shown session.
    Harvest(SessionId),
    /// `add_mcp_server`.
    AddMcp {
        /// Server name (file stem).
        name: String,
        /// One server object (`command`/`args`/`env`/`cwd` or `url`/`headers`).
        entry: serde_json::Value,
    },
    /// `add_mcp_repo`.
    AddMcpRepo(String),
    /// `sync_mcp`.
    SyncMcp,
    /// `list_logins` plus OAuth servers from `list_mcp`.
    ListLogins,
    /// `list_acp`.
    ListAcp {
        /// Run an `initialize` probe per entry (slow: spawns each harness).
        probe: bool,
    },
    /// `add_acp` from the registry, then re-list.
    AddAcp(String),
    /// `remove_acp`, then `list_acp`.
    RemoveAcp(String),
    /// `list_acp_registry`.
    ListAcpRegistry,
    /// `login` (provider name) or `mcp_login` (server name).
    Login(String),
    /// `logout` (provider name) or `mcp_logout` (server name).
    Logout(String),
    /// `add_provider`.
    AddProvider {
        /// Gateway name.
        name: String,
        /// OpenAI-compatible base URL.
        base_url: String,
        /// API key.
        key: String,
    },
    /// `remove_provider`.
    RemoveProvider(String),
    /// `list_providers`.
    ListProviders,
    /// `link_status`.
    LinkStatus,
    /// `link_enable(enabled, None)`, then `link_status`.
    LinkEnable(bool),
    /// `link_enable(<current>, Some(url))`, then `link_status`.
    LinkRelay(String),
    /// `link_pair`.
    LinkPair,
    /// `link_cancel_pair`.
    LinkCancelPair,
    /// `link_rename` (engine display name), then `link_status`.
    LinkRename(String),
    /// `link_rename_device(id, name)`, then `link_status`.
    LinkRenameDevice(String, String),
    /// `link_revoke_device`, then `link_status`.
    LinkRevoke(String),
    /// `devices_list` (69 §3).
    MeshDevicesList,
    /// `sandboxes_list` (79 §8).
    SandboxList,
    /// `sandbox_desktop_access(name)`: the URL to open in a browser.
    SandboxDesktop(String),
    /// `device_pair_start(name)`: the mesh pairing QR.
    MeshDevicesAdd(Option<String>),
    /// `device_revoke(id)`, then `devices_list`.
    MeshDevicesRevoke(String),
    /// `device_rename(id, name)`, then `devices_list`.
    MeshDevicesRename(String, String),
    /// `list_nodes`.
    ListNodes,
    /// `add_node(key, name)`, then `list_nodes`.
    AddNode(String, Option<String>),
    /// `remove_node(<resolved id>)`, then `list_nodes`. Holds a name or id prefix.
    RemoveNode(String),
    /// `rename_node(<resolved id>, name)`, then `list_nodes`. Holds a name or id prefix.
    RenameNode(String, String),
    /// `invite_node(<resolved id>)`. Holds a name or id prefix.
    InviteNode(String),
    /// `list_artifacts(session, descendants: true)`; `pick` opens the `/artifacts`
    /// menu, `apps` keeps only [`artifact::Kind::App`] records (`/apps`).
    ListArtifacts {
        /// Session.
        session: SessionId,
        /// Open the picker on reply.
        pick: bool,
        /// Filter to apps.
        apps: bool,
    },
    /// `export_artifact(id, ~/Downloads/<name>)`.
    ExportArtifact {
        /// The artifact.
        id: String,
        /// Its file name.
        name: String,
    },
    /// Open a local file with the OS default handler ([`platform::open::open_path`], detached).
    OpenPath(PathBuf),
    /// `list_secrets`.
    ListSecrets,
    /// `set_secret(name, value, note)`. The value transits once and is never shown again.
    SetSecret {
        /// Name.
        name: String,
        /// The value (source `user`).
        value: String,
        /// `None` keeps the old note.
        note: Option<String>,
    },
    /// `restore_secret(name, back)`: version `-back` becomes current, as a new version.
    RestoreSecret {
        /// Name.
        name: String,
        /// Versions back, 1..=4.
        back: u8,
    },
    /// `delete_secret`: the name and every version.
    DeleteSecret(String),
    /// `answer_secret`: `None` declines a `secret_request`.
    AnswerSecret {
        /// The request.
        id: secrets::RequestId,
        /// The typed value.
        value: Option<String>,
    },
    /// `get_communication`; `None` reads the global default alone.
    GetCommunication(Option<SessionId>),
    /// `set_communication`, then `get_communication(view)`: the reply is the
    /// shown session's state, whichever switch moved (a global flip changes
    /// what this session may do; a session flip under global-off changes nothing).
    SetCommunication {
        /// `None` = the persisted global switch (`always`).
        session: Option<SessionId>,
        /// On or off.
        enabled: bool,
        /// The shown session, whose state the reply carries.
        view: Option<SessionId>,
    },
    /// `set_peer_scope`: owner-only root scope and alias (50 §2.3).
    SetPeerScope {
        /// The root.
        root: SessionId,
        /// New scope.
        scope: PeerScope,
        /// New alias; `None` keeps it.
        alias: Option<String>,
    },
    /// `list_peers(session)`: the directory `session` may address.
    ListPeers(SessionId),
    /// `list_peer_messages(session, pending_only: true)`: requests a human may resolve.
    ListPeerRequests(SessionId),
    /// `answer_peer_request`: resolve one exact pending request for `session`.
    AnswerPeerRequest {
        /// The request's recipient, or its blocked sender.
        session: SessionId,
        /// The request.
        reply_to: Uuid,
        /// The answer.
        text: String,
    },
    /// `a2a_status`.
    A2aStatus,
    /// `reload_a2a`: validate and swap the peer/export configuration.
    ReloadA2a,
    /// `set_session_project`: label or clear the root's project (51 §3).
    SetSessionProject {
        /// Any session of the tree.
        session: SessionId,
        /// `None` clears.
        project: Option<String>,
    },
    /// `session_cost` for the shown session.
    SessionCost(SessionId),
    /// `list_terminals(session, descendants: true)`.
    ListTerminals(SessionId),
    /// `pty_screen`: one page of a retained terminal (51 §7.4).
    PtyScreen {
        /// Owning session.
        session: SessionId,
        /// Job.
        job: JobId,
        /// Rows asked for.
        viewport: PtyViewport,
        /// Revision already shown; `Unchanged` comes back when it still holds.
        if_revision: Option<u64>,
        /// [`TerminalView::epoch`] at the time of asking, echoed by the reply.
        epoch: u64,
    },
    /// `pty_input`: the human types into a PTY job (55 §3).
    PtyInput {
        /// Owning session.
        session: SessionId,
        /// Job.
        job: JobId,
        /// Typed text.
        text: Option<String>,
        /// Keys after `text`.
        keys: Vec<Key>,
    },
    /// `create_session`: explicit creation, private or durable (51 §4.1).
    CreateSession(CreateSessionOptions),
    /// `leave_session`: release the private root this view owns (51 §4.3).
    LeaveSession {
        /// The root.
        session: SessionId,
        /// Its lease secret.
        secret: String,
    },
    /// `dictation_prewarm`: load the streaming STT model once at startup (54 §0.5).
    DictationPrewarm,
    /// `dictation_start`: open the mic; the reply is [`UiEvent::DictationStarting`] or [`UiEvent::DictationFailed`].
    DictationStart,
    /// `dictation_stop(id)`: close the mic; `DictationFinished` follows on the bus.
    DictationStop(voice::DictationId),
    /// `talk_start(session, "device", null)`: a live spoken conversation (76 §5.1);
    /// `None` = no session yet: one is created first (`SessionOpened`), as the desktop's
    /// Talk entry does. The reply is [`UiEvent::TalkStarting`] or [`UiEvent::TalkFailed`].
    TalkStart(Option<SessionId>),
    /// `talk_stop(id)`; `TalkFinished` follows on the bus.
    TalkStop(talk::TalkId),
    /// `cancel(session, "tree")`: the turn and every running descendant (133 §5).
    CancelTree(SessionId),
}

/// One row of the `/login` picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRow {
    /// Provider namespace or MCP server name.
    pub name: String,
    /// An MCP server (else a provider).
    pub mcp: bool,
    /// `logged in (acct)` / `login required` / `not logged in`.
    pub status: String,
}

/// Rows of live output a `shell` card shows for its background job.
pub const JOB_LINES: usize = 10;

/// The tail of a background job's output as it arrives (`JobEvent::Output`,
/// coalesced upstream to one chunk per half second per job): the last
/// [`JOB_LINES`] lines, frozen with how it ended. A PTY job (22 §5) instead
/// shows the last non-blank rows of its latest `JobEvent::Screen`. Bounded,
/// so any number of jobs costs the same; the log file is the whole story.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobTail {
    /// Job id (`j3`).
    pub id: String,
    /// Complete lines, oldest first, at most [`JOB_LINES`].
    lines: VecDeque<String>,
    /// Bytes after the last newline (a progress bar's current state).
    partial: String,
    /// `exited 0 · 47s` and the like once finished.
    pub exit: Option<String>,
}

impl JobTail {
    /// An empty tail for job `id`.
    pub fn new(id: String) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// Append raw output. Escape sequences are dropped, `\r` rewinds the
    /// current line (progress bars), tabs become spaces.
    pub fn push(&mut self, chunk: &str) {
        let mut chars = chunk.chars();
        while let Some(c) = chars.next() {
            match c {
                '\n' => {
                    if self.lines.len() == JOB_LINES {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(std::mem::take(&mut self.partial));
                }
                '\r' => self.partial.clear(),
                '\t' => self.partial.push_str("    "),
                '\x1b' => match chars.next() {
                    // CSI: parameters, then one final byte in `@`..`~`.
                    Some('[') => {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    // OSC: to BEL or ST.
                    Some(']') => {
                        let mut prev = '\0';
                        for c in chars.by_ref() {
                            if c == '\x07' || (prev == '\x1b' && c == '\\') {
                                break;
                            }
                            prev = c;
                        }
                    }
                    _ => {}
                },
                c if c.is_control() => {}
                c => self.partial.push(c),
            }
        }
    }

    /// Replace the tail with the last [`JOB_LINES`] non-blank rows of a PTY
    /// job's screen (22 §5). The screen is a whole, not a stream: nothing
    /// carried over from before.
    pub fn set_screen(&mut self, rows: &[String]) {
        self.partial.clear();
        self.lines.clear();
        for r in rows
            .iter()
            .rev()
            .filter(|r| !r.trim().is_empty())
            .take(JOB_LINES)
        {
            self.lines.push_front(r.clone());
        }
    }

    /// The rows to draw, oldest first: complete lines and the partial one.
    pub fn rows(&self) -> impl Iterator<Item = &str> {
        let extra = usize::from(!self.partial.is_empty());
        self.lines
            .iter()
            .map(String::as_str)
            .skip((self.lines.len() + extra).saturating_sub(JOB_LINES))
            .chain((!self.partial.is_empty()).then_some(self.partial.as_str()))
    }

    /// Header row: `j3 · running` until it ends.
    pub fn header(&self) -> String {
        match &self.exit {
            Some(e) => format!("{} · {e}", self.id),
            None => format!("{} · running", self.id),
        }
    }
}

/// One transcript row.
#[derive(Debug)]
pub enum Entry {
    /// What the user typed.
    User {
        /// The text.
        text: String,
        /// Seq of this row when it opens a turn (the `/edit`, `/regen`,
        /// `/fork` handle, 29 §6.1); `None` for steers and notes.
        turn: Option<i64>,
    },
    /// Streamed assistant text: rendered blocks + raw tail.
    Assistant(Box<markdown::Text>),
    /// Collapsible card: a peer message, a skill body, or the system prompt.
    /// One line until toggled with Ctrl+O.
    Card {
        /// Glyph and colour family.
        kind: CardKind,
        /// Peer name, skill name, or `prompt`.
        title: String,
        /// Body rendered as markdown.
        text: Box<markdown::Text>,
        /// Show the body.
        expanded: bool,
    },
    /// Streamed reasoning; collapsed once `done`.
    Reasoning {
        /// Text so far.
        text: String,
        /// No more deltas will come.
        done: bool,
    },
    /// The model is still emitting a tool call's arguments.
    Pending {
        /// Provider call id (matches `ToolCallStarted`).
        call_id: String,
        /// Tool name.
        name: String,
        /// `ToolCallProgress` so far: what a `show` call is doing and the
        /// argument bytes streamed (design doc 35 §6.1).
        progress: Option<Progress>,
    },
    /// A tool call and, once finished, its result.
    Tool {
        /// Provider call id (matches `ToolCallFinished`).
        call_id: String,
        /// Tool name.
        name: String,
        /// Arguments as [`describe`] lines.
        args: Vec<String>,
        /// `(output, is_error)` once finished.
        result: Option<(String, bool)>,
        /// Full output shown (Ctrl+O); built on first expand.
        expanded: Option<Box<markdown::Text>>,
        /// Live output of the background job a `shell` call started
        /// (design doc 14); `None` for other tools and jobs that finished
        /// within the grace window (their result carries the output).
        job: Option<JobTail>,
        /// The file a `show` call produced (`ArtifactShown`, design doc 35);
        /// the card renders as an artifact card once set.
        artifact: Option<Box<artifact::Artifact>>,
        /// Spinner row of a running `crawl` / `map_site` / `scrape_url`
        /// (`ScrapeProgress`, design doc 38); cleared when the result lands.
        scrape: Option<String>,
    },
    /// A compaction boundary (design doc 07 §8.4): a divider, expandable with
    /// Ctrl+O to the summary when loaded from history (the live event has none).
    Compacted {
        /// Epoch the boundary opened.
        epoch: u32,
        /// How the head was built.
        style: CompactStyle,
        /// Summary the model works from; `None` for the live event.
        summary: Option<Box<markdown::Text>>,
        /// Show the summary.
        expanded: bool,
    },
    /// 109 §10.3: a plain divider in the `Compacted` shape (`── resumed after
    /// restart ──`); nothing to expand.
    Divider(String),
    /// 125 §7.4: one Advisory card — every note the advisor raised at one
    /// step boundary. Three notes until Ctrl+O (`ADVISORY_COLLAPSED`).
    Advisory {
        /// The typed metadata of the row.
        advisory: Advisory,
        /// Show every note.
        expanded: bool,
    },
    /// Informational line (session lists, notices).
    Info(String),
    /// Error line.
    Error(String),
}

/// What an [`Entry::Card`] holds; picks its glyph and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// Text a peer injected (`<subagent_result>` / `<message>`).
    Peer,
    /// A skill body (`/skills`) or a knowledge card (`/knowledge`).
    Skill,
    /// The assembled system prompt (`/prompt`).
    Prompt,
    /// A `<job_result>` the harness injected when a background job ended.
    Job,
    /// A `<schedule>` fire the harness injected (34 §1.5).
    Schedule,
    /// A `/link pair` QR (31 §6.5).
    Link,
    /// A `/nodes` `i` invite QR: a phone that scans it pairs with that node (32 §6.7).
    Node,
    /// An artifact shown inline from `/artifacts` (`s`), or the `show` card header (35 §6.1).
    Artifact,
    /// A message the user removed from the model's context (48 §2.2): the
    /// original stays in the archive, shown only when expanded.
    Removed,
    /// A vision consultation note (52 §6): the main model's question and
    /// the fallback's answer, shown only when expanded.
    Vision,
}

impl CardKind {
    /// Header glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            CardKind::Peer => "⟵",
            CardKind::Skill => "◇",
            CardKind::Prompt => "§",
            CardKind::Job => "▸",
            CardKind::Schedule => "◷",
            CardKind::Link | CardKind::Node => "⌁",
            CardKind::Artifact => "▣",
            CardKind::Removed => "⊘",
            CardKind::Vision => "◉",
        }
    }
}

/// A collapsed [`Entry::Card`] over `text`.
fn card(kind: CardKind, title: String, text: &str) -> Entry {
    Entry::Card {
        kind,
        title,
        text: Box::new(markdown::Text::complete(text)),
        expanded: false,
    }
}

/// Notes an [`Entry::Advisory`] shows before Ctrl+O (125 §7.4).
pub const ADVISORY_COLLAPSED: usize = 3;
/// The advisor glyph (U+25CE; outside `tests/no_emoji.rs`' ranges, `◉` is Vision).
pub const ADVISOR_GLYPH: &str = "◎";

/// `nit` / `concern` / `blocker`: the badge word.
pub fn severity_name(s: AdvisorSeverity) -> &'static str {
    match s {
        AdvisorSeverity::Nit => "nit",
        AdvisorSeverity::Concern => "concern",
        AdvisorSeverity::Blocker => "blocker",
    }
}

/// The card's severity: the worst of its notes (`Nit` for an empty list).
pub fn advisory_severity(a: &Advisory) -> AdvisorSeverity {
    a.notes
        .iter()
        .map(|n| n.severity)
        .max()
        .unwrap_or(AdvisorSeverity::Nit)
}

/// `◎ advisor · 2 notes`: the card title.
pub fn advisory_title(a: &Advisory) -> String {
    let n = a.notes.len();
    format!(
        "{ADVISOR_GLYPH} advisor · {n} note{}",
        if n == 1 { "" } else { "s" }
    )
}

/// `[concern] note text`: one line per note (the headless printer; the
/// renderer colours the badge itself).
pub fn advisory_lines(a: &Advisory) -> Vec<String> {
    a.notes
        .iter()
        .map(|n| format!("[{}] {}", severity_name(n.severity), n.note))
        .collect()
}

/// `advisor → anthropic/claude-opus-4-1` / `advisor → off`: the one info
/// line on `AdvisorChanged` (125 §7.5, the `model → …` shape).
pub fn advisor_changed_line(s: &AdvisorStatus) -> String {
    match (&s.state, &s.model) {
        (AdvisorState::Off, _) | (_, None) => "advisor → off".to_owned(),
        (_, Some(m)) => format!("advisor → {m}"),
    }
}

/// `off` / `idle` / `reviewing` / `paused` / `halted`.
pub fn advisor_state_name(s: AdvisorState) -> &'static str {
    match s {
        AdvisorState::Off => "off",
        AdvisorState::Idle => "idle",
        AdvisorState::Reviewing => "reviewing",
        AdvisorState::Paused => "paused",
        AdvisorState::Halted => "halted",
    }
}

/// `/advisor status` (125 §7.3): the four lines, `—` for unknowns; one
/// line when off.
pub fn advisor_status_lines(s: &AdvisorStatus) -> Vec<String> {
    if s.state == AdvisorState::Off {
        return vec!["advisor: off  ·  /advisor to pick a model".to_owned()];
    }
    let dash = || "—".to_owned();
    // Why paused / halted rides on the state line.
    let detail = s
        .detail
        .as_deref()
        .map_or_else(String::new, |d| format!("  ·  {d}"));
    let mut out = vec![
        format!(
            "advisor: {}  ·  {}  ·  {}{detail}",
            advisor_state_name(s.state),
            s.model.clone().unwrap_or_else(dash),
            s.effort.clone().unwrap_or_else(dash)
        ),
        format!(
            "notes: {} nit · {} concern · {} blocker  ·  step {}  ·  backlog {}",
            s.notes.nit, s.notes.concern, s.notes.blocker, s.step, s.backlog
        ),
    ];
    let context = if s.context_limit > 0 {
        format!(
            "{} / {} ({}%)",
            k(s.context_used),
            k(s.context_limit),
            u64::from(s.context_used) * 100 / u64::from(s.context_limit)
        )
    } else {
        k(s.context_used)
    };
    out.push(format!(
        "context: {context}  ·  in {} · out {}  ·  cost {}",
        k(u32::try_from(s.tokens_in).unwrap_or(u32::MAX)),
        k(u32::try_from(s.tokens_out).unwrap_or(u32::MAX)),
        s.cost.as_ref().map_or_else(dash, cost_short)
    ));
    out.push(match &s.last_note {
        Some(n) => format!("last: [{}] {}", severity_name(n.severity), n.note),
        None => "last: —".to_owned(),
    });
    out
}

/// Rows a string of `chars` occupies at `width` columns (at least one).
fn rows(chars: usize, width: usize) -> usize {
    chars.div_ceil(width.max(1)).max(1)
}

/// Rows a rendered markdown line takes once the terminal wraps it.
fn line_rows(line: &markdown::Line, width: usize) -> usize {
    rows(line.iter().map(|s| s.text.chars().count()).sum(), width)
}

/// Rows a plain text takes once wrapped: one per line, more for long ones.
fn text_rows(text: &str, width: usize) -> usize {
    text.lines().map(|l| rows(l.chars().count(), width)).sum()
}

impl Entry {
    /// Rows this entry draws at `width` columns (borders, headers and
    /// wrapping included; an estimate — the terminal's wrap is the truth).
    fn height(&self, width: usize) -> usize {
        // Bordered boxes have two columns of frame and padding each side.
        let inner = width.saturating_sub(4);
        let md = |t: &markdown::Text, width: usize| {
            let v = t.view();
            v.lines().map(|l| line_rows(l, width)).sum::<usize>() + text_rows(v.plain, width)
        };
        match self {
            Entry::User { text, .. } => match crate::attach::spoken(text) {
                Some((_, _, body)) => text_rows(&crate::attach::display(body), inner) + 5,
                None => text_rows(&crate::attach::display(text), inner) + 4,
            },
            // Rendered fences (diagrams, surfaces) never exceed the construction
            // width, so they count one row per line like everything else.
            Entry::Assistant(t) => md(t, width),
            Entry::Card { text, expanded, .. } => 4 + if *expanded { md(text, inner) } else { 0 },
            // 125 §7.4: border + title + margin, one row per note shown, the `… +N more` row.
            Entry::Advisory { advisory, expanded } => {
                let n = advisory.notes.len();
                if *expanded {
                    4 + n
                } else {
                    4 + n.min(ADVISORY_COLLAPSED) + usize::from(n > ADVISORY_COLLAPSED)
                }
            }
            Entry::Reasoning { done: true, .. } => 1,
            Entry::Reasoning { text, .. } | Entry::Info(text) | Entry::Error(text) => {
                text_rows(text, width)
            }
            Entry::Pending { .. } => 2,
            // Artifact card: header, title (when it differs), path row; the
            // arguments are not drawn (35 §6.1).
            Entry::Tool {
                artifact: Some(a),
                expanded,
                ..
            } => 6 + usize::from(a.title != a.name) + expanded.as_ref().map_or(0, |t| md(t, inner)),
            Entry::Tool {
                args,
                expanded,
                job,
                ..
            } => {
                5 + args.len()
                    + expanded.as_ref().map_or(0, |t| md(t, inner))
                    + job.as_ref().map_or(0, |j| 1 + j.rows().count())
            }
            Entry::Compacted {
                summary, expanded, ..
            } => {
                4 + match summary {
                    Some(t) if *expanded => md(t, inner),
                    _ => 0,
                }
            }
            Entry::Divider(_) => 3,
        }
    }
}

/// A held paste: large text, or an attached image (17 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paste {
    /// Text too large for the input box.
    Text(String),
    /// An image file to send with the input.
    Image(Image),
    /// A reference picked with `/attach` (66 §10): its marker line leads the sent text.
    Ref(agent::attach::AttachRef),
}

/// What the input box shows for a held paste: `[⎘ paste 1: 69 lines]`
/// (`chars` for a single long line), `[⎘ image 2: 1440×900]` (file name
/// when the size is unknown; `file` for a PDF), `[⎘ attach 3: <title>]`.
/// `i` is the index into `Model::pastes`.
pub fn paste_token(i: usize, p: &Paste) -> String {
    match p {
        Paste::Text(text) => {
            let lines = text.lines().count();
            let size = if lines > 1 {
                format!("{lines} lines")
            } else {
                format!("{} chars", text.chars().count())
            };
            format!("[⎘ paste {}: {size}]", i + 1)
        }
        Paste::Image(img) if img.w > 0 => format!("[⎘ image {}: {}×{}]", i + 1, img.w, img.h),
        Paste::Image(img) => {
            let name = img.path.file_name().unwrap_or_default().to_string_lossy();
            let kind = if img
                .path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            {
                "file"
            } else {
                "image"
            };
            format!("[⎘ {kind} {}: {name}]", i + 1)
        }
        Paste::Ref(r) => format!("[⎘ attach {}: {}]", i + 1, r.title),
    }
}

/// Split a harness-injected input into `(kind, title, body)` per wrapper.
/// Inputs queued during a turn arrive as one text: `<subagent_result
/// name="…">` / `<message from="…">` / `<job_result id="…" command="…"
/// exit="…" secs="…">` / `<job_idle id="…" quiet="…">` (61) blocks joined by
/// newlines. Harness notes (`<context_summary>`, `<todo>`,
/// `<context_pressure>`, `<runtime>`, `<knowledge>`; design docs 07 §8.7,
/// 10 §4) are wrappers too but yield no card. `None` for human text
/// (or anything not entirely made of wrappers).
pub fn injected(text: &str) -> Option<Vec<(CardKind, String, String)>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let (open, after) = rest.strip_prefix('<')?.split_once('>')?;
        let name = open.split(' ').next()?;
        let (key, close) = match name {
            "subagent_result" => (Some("name=\""), "</subagent_result>"),
            "message" => (Some("from=\""), "</message>"),
            "job_result" => (Some("id=\""), "</job_result>"),
            "job_idle" => (Some("id=\""), "</job_idle>"),
            "reminder" => (Some("id=\""), "</reminder>"),
            "schedule" => (Some("name=\""), "</schedule>"),
            "context_summary" => (None, "</context_summary>"),
            "todo" => (None, "</todo>"),
            "context_pressure" => (None, "</context_pressure>"),
            "runtime" => (None, "</runtime>"),
            "knowledge" => (None, "</knowledge>"),
            "nudge" => (None, "</nudge>"),
            "journal" => (None, "</journal>"),
            // 125 §7.4: an advisory steer's row is carded by `AdvisoryAdded`, never echoed.
            "advisory" => (None, "</advisory>"),
            _ => return None,
        };
        let (body, tail) = after.split_once(close)?;
        if let Some(key) = key {
            let from = attr(open, key)?;
            let (kind, title) = if name == "job_result" {
                let command = unescape(attr(open, "command=\"").unwrap_or_default());
                let exit = attr(open, "exit=\"").unwrap_or_default();
                let secs = attr(open, "secs=\"").unwrap_or_default();
                // 22 §5: a PTY job's card says so.
                let pty = if attr(open, "pty=\"") == Some("true") {
                    " · pty"
                } else {
                    ""
                };
                (
                    CardKind::Job,
                    format!("{from} · {command} · exit {exit} · {secs}s{pty}"),
                )
            } else if name == "job_idle" {
                // 61 §2.3: `j4 idle 63s · omp1` (name only when the job has one).
                let quiet = attr(open, "quiet=\"").unwrap_or_default();
                let name = attr(open, "name=\"")
                    .map(|n| format!(" · {}", unescape(n)))
                    .unwrap_or_default();
                (CardKind::Job, format!("{from} idle {quiet}s{name}"))
            } else if name == "reminder" {
                // 14 as-built: `r3 reminder · after 10m` (` · late` when re-armed past due).
                let after = attr(open, "after=\"")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map_or_else(String::new, |s| format!(" · after {}", human_secs(s)));
                let late = if attr(open, "late=\"") == Some("true") {
                    " · late"
                } else {
                    ""
                };
                (CardKind::Job, format!("{from} reminder{after}{late}"))
            } else if name == "schedule" {
                let at = attr(open, "at=\"").unwrap_or_default();
                let at = chrono::DateTime::parse_from_rfc3339(at).map_or_else(
                    |_| at.to_owned(),
                    |t| t.with_timezone(&chrono::Local).format("%H:%M").to_string(),
                );
                (CardKind::Schedule, format!("{from} · {at}"))
            } else {
                (CardKind::Peer, from.to_owned())
            };
            let body = body.strip_prefix('\n').unwrap_or(body).trim_end();
            out.push((kind, title, body.to_owned()));
        }
        rest = tail.trim_start();
    }
    Some(out)
}

/// `45s`, `10m`, `1h30m`.
fn human_secs(s: u64) -> String {
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, _) => format!("{m}m"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h{m}m"),
    }
}

/// Undo the wrappers' XML attribute escape.
fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Value of `key` (`name="`) in an open tag, up to the closing quote.
fn attr<'a>(open: &'a str, key: &str) -> Option<&'a str> {
    open.split_once(key).and_then(|(_, v)| v.split('"').next())
}

/// Slash commands: name, argument hint (`<x>` required, `[x]` optional, `""` none), description.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("/model", "[id]", "switch model (picker without id)"),
    (
        "/title",
        "[model|off]",
        "model that names sessions in the history list (off = the session's model)",
    ),
    (
        "/vision",
        "[model|off]",
        "model that inspects images when the main model cannot (off = images are omitted)",
    ),
    (
        "/effort",
        "[level|clear]",
        "reasoning effort (picker without arg)",
    ),
    (
        "/advisor",
        "[off|on|status|<provider/model>]",
        "a second model that reviews this session (picker without argument)",
    ),
    (
        "/aws",
        "[profile|clear]",
        "AWS profile for Bedrock, harness-wide (picker without arg)",
    ),
    (
        "/mcp",
        "[add <url|json>|sync]",
        "MCP servers: on/off (this session or always), risk lines",
    ),
    (
        "/acp",
        "[add <registry-id>|remove <name>]",
        "External harnesses (ACP) and the registry",
    ),
    (
        "/skills",
        "[add <url>|rm <name>|sync]",
        "skills: picker (⏎ hands one to the model, s shows it); `/<skill> [words]` does the same",
    ),
    (
        "/knowledge",
        "[query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]",
        "knowledge base: list, search, history, show a commit, restore, harvest",
    ),
    (
        "/login",
        "[name]",
        "log in to a provider or MCP server (picker without name)",
    ),
    ("/logout", "<name>", "forget a login"),
    ("/prompt", "", "show the assembled system prompt"),
    (
        "/agents",
        "",
        "subagents of this session (picker: ⏎ opens one)",
    ),
    ("/todos", "", "show or hide the plan card (F2)"),
    ("/dictate", "", "toggle dictation (Ctrl+D)"),
    (
        "/talk",
        "",
        "talk with the session aloud; Esc cancels the reply or leaves the view, Ctrl+T stops",
    ),
    (
        "/jobs",
        "[kill <id>]",
        "list this session's background jobs",
    ),
    (
        "/terminals",
        "",
        "PTY jobs of this session's tree (picker: ⏎ opens a read-only observer)",
    ),
    (
        "/cost",
        "",
        "model cost of this session's tree: returned, estimated, coverage, per child",
    ),
    (
        "/project",
        "<label|off>",
        "label this session's root for grouping in /sessions and /artifacts (off clears)",
    ),
    (
        "/schedules",
        "[run <name>|test <name>|history <name>|pause <name>|resume <name>]",
        "scheduled prompts: list, fire one now, show its runs, pause or resume a loop",
    ),
    (
        "/tasks",
        "",
        "this project's tasks: id, state, owner, title (67)",
    ),
    (
        "/roster",
        "",
        "the agent roster: name, status, open tasks (67)",
    ),
    (
        "/computer",
        "",
        "computer control: permissions and displays",
    ),
    ("/parent", "", "open the parent session (from a subagent)"),
    (
        "/sessions",
        "",
        "switch session (picker; owner chip on sessions owned elsewhere)",
    ),
    ("/take", "", "continue the open session on this node (68)"),
    (
        "/broadcast",
        "[running|roots] [text]",
        "send one message to many sessions",
    ),
    (
        "/search",
        "[query]",
        "search sessions, artifacts, knowledge and history (live; fzf syntax)",
    ),
    (
        "/attach",
        "[query]",
        "attach a note, concept, artifact, chat or meeting to the next message (66)",
    ),
    (
        "/resume",
        "[id]",
        "open a session; on a stopped ACP child, restart its harness in the same context (75 §10)",
    ),
    (
        "/new",
        "[private]",
        "start a new session (private: memory-only tree, closed when you leave it)",
    ),
    (
        "/clear",
        "",
        "drop the conversation, keep the session and its settings",
    ),
    (
        "/approvals",
        "[on|off] [always]",
        "tool approval prompts; `always` persists to config.yaml",
    ),
    (
        "/communication",
        "[on|off] [always]",
        "agent-to-agent communication: status with its exact blocker; `always` persists the global switch",
    ),
    (
        "/peers",
        "[scope owner|isolated [alias]]",
        "peers this session may address and requests awaiting a human (picker: ⏎ opens, i inspects, a answers)",
    ),
    (
        "/a2a",
        "[reload]",
        "network (A2A) status: listener, configured peers and exports; reload swaps peers.yaml",
    ),
    (
        "/double",
        "[status|off|pause [off]|train [a|b] [venue]|rollback <version>|cap <class> <rung> [opt-in]]",
        "the owner's double (126): its numbers, learning on/off, a training run, caps",
    ),
    (
        "/cancel",
        "",
        "abort the in-flight turn (Esc; Esc x3 within 1 s stops its subagents too)",
    ),
    (
        "/compact",
        "[summary|cut|snap|style …]",
        "fold context; style sets the default",
    ),
    (
        "/endless",
        "[on|off]",
        "endless memory: switch it, or show the view state",
    ),
    (
        "/shake",
        "[images|documents|messages|tools|summary]",
        "remove images, messages or the summary from the model's context",
    ),
    ("/undo", "", "drop the last turn (redo restores)"),
    ("/redo", "", "restore the last undone turn"),
    (
        "/edit",
        "[n]",
        "edit turn n's message and regenerate from it (picker without n)",
    ),
    (
        "/regen",
        "[n]",
        "regenerate the reply to turn n (picker without n)",
    ),
    (
        "/fork",
        "[n]",
        "new session with the conversation through turn n (picker without n)",
    ),
    ("/quit", "", "exit"),
    (
        "/provider",
        "[add|rm <name>]",
        "providers: built-in opencode (Zen) / openrouter keyed via /secrets add; add prompts for an OpenAI-compatible gateway's name, url, key",
    ),
    (
        "/secrets",
        "[add [name]|rm <name>|restore <name> [n]]",
        "encrypted secrets the agent uses by name",
    ),
    (
        "/link",
        "[on|off|pair|rm <n>|name <n> <name>|relay <url>]",
        "pair and manage phones",
    ),
    (
        "/mesh",
        "devices [list|add [name]|rm <id>|name <id> <name>]",
        "devices paired with the mesh (any node serves them)",
    ),
    (
        "/nodes",
        "[add <key>|rm <n>|name <n> <name>|invite <n>]",
        "machines this harness can run sessions on",
    ),
    (
        "/sandbox",
        "[desktop <name>]",
        "sandbox VMs on this engine (design doc 79); `desktop` prints the noVNC URL",
    ),
    (
        "/artifacts",
        "",
        "files the agent showed (picker: ⏎ opens, s shows inline, e exports to ~/Downloads)",
    ),
    (
        "/apps",
        "",
        "interactive apps the agent built, run on desktop/web (picker: ⏎ opens, s shows source, e exports to ~/Downloads)",
    ),
    ("/exit", "", "same as /quit"),
];

/// Commands whose name starts with the typed word (`/mo` → `/model`);
/// `/apps` only while the app plugin runs.
pub fn slash_matches(
    word: &str,
    app_enabled: bool,
) -> Vec<&'static (&'static str, &'static str, &'static str)> {
    COMMANDS
        .iter()
        .filter(|c| c.0.starts_with(word) && (app_enabled || c.0 != "/apps"))
        .collect()
}

/// One `/double` subcommand (126 §10).
#[derive(Clone, Debug, PartialEq)]
pub enum DoubleCmd {
    /// `double_status`.
    Status,
    /// `enabled: false` in config.yaml via `double_settings_set`; takes effect at restart.
    Off,
    /// `double_pause(on)`.
    Pause(bool),
    /// `double_train(tier, venue)`.
    Train {
        /// `a` | `b`.
        tier: double::Tier,
        /// `auto` | `local` | `node:<name>`.
        venue: double::Venue,
    },
    /// `double_rollback(version)`.
    Rollback(String),
    /// `double_set_cap(class, rung, mutating_opt_in)`.
    Cap {
        /// The §3 key.
        class: String,
        /// 0..=3.
        rung: u8,
        /// `opt-in` was given.
        opt_in: bool,
    },
}

impl DoubleCmd {
    /// The usage line.
    pub const USAGE: &'static str = "usage: /double [status|off|pause [off]|train [a|b] [auto|local|node:<name>]|rollback <version>|cap <class> <rung> [opt-in]]";

    /// Parse the text after `/double`; `None` is a usage error. A class key may
    /// contain spaces (`approval:shell:mutating:git push:root`), so `cap` reads
    /// its trailing `<rung> [opt-in]` first and the rest is the key.
    pub fn parse(arg: &str) -> Option<Self> {
        let mut w = arg.split_whitespace();
        let head = w.next().unwrap_or("status");
        if head == "cap" {
            let mut rest: Vec<&str> = w.collect();
            let opt_in = rest.last() == Some(&"opt-in");
            if opt_in {
                rest.pop();
            }
            let rung: u8 = rest.pop()?.parse().ok().filter(|r| *r <= 3)?;
            if rest.is_empty() {
                return None;
            }
            return Some(Self::Cap {
                class: rest.join(" "),
                rung,
                opt_in,
            });
        }
        Some(match (head, w.next(), w.next(), w.next()) {
            ("status", None, ..) => Self::Status,
            ("off", None, ..) => Self::Off,
            ("pause", None, ..) => Self::Pause(true),
            ("pause", Some("off"), None, _) => Self::Pause(false),
            ("train", tier, venue, None) => Self::Train {
                tier: match tier.unwrap_or("a") {
                    "a" => double::Tier::A,
                    "b" => double::Tier::B,
                    _ => return None,
                },
                venue: match double::Venue::parse(venue.unwrap_or("auto"))? {
                    double::Venue::Off => return None,
                    v => v,
                },
            },
            ("rollback", Some(v), None, _) => Self::Rollback(v.to_owned()),
            _ => return None,
        })
    }
}

/// The chip text for one card's prediction (126 §10): `double: Allow 91 % · L1`, or at L2
/// the countdown `double: Allow 91 % · L2 · answers in 18 s (answer to take over)`.
/// `none` is the double saying it is unsure.
pub fn double_chip(p: &DoublePredicted, now_ms: u64) -> String {
    let mut s = format!("double: {} {:.0} % · L{}", p.choice, p.p * 100.0, p.rung);
    if let Some(left) = double::chip::grace_left_s(p, now_ms) {
        s.push_str(&format!(" · answers in {left} s (answer to take over)"));
    }
    s
}

/// Tool arguments as display lines. `shell` shows the command itself
/// (`$ ` on the first line); anything else is `key: value` per argument,
/// with long or multi-line strings collapsed to their size.
pub fn describe(name: &str, args: &serde_json::Value) -> Vec<String> {
    if name == "shell"
        && let Some(cmd) = args.get("command").and_then(|c| c.as_str())
    {
        return cmd
            .trim()
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{}{l}", if i == 0 { "$ " } else { "  " }))
            .collect();
    }
    let Some(obj) = args.as_object() else {
        return if args.is_null() {
            Vec::new()
        } else {
            vec![args.to_string()]
        };
    };
    obj.iter()
        .map(|(k, v)| match v {
            serde_json::Value::String(s) if !s.contains('\n') && s.chars().count() <= 120 => {
                format!("{k}: {s}")
            }
            serde_json::Value::String(s) => {
                format!("{k}: ({} lines, {} bytes)", s.lines().count(), s.len())
            }
            other => {
                let s = other.to_string();
                match s.char_indices().nth(120) {
                    Some((cut, _)) => format!("{k}: {}…", &s[..cut]),
                    None => format!("{k}: {s}"),
                }
            }
        })
        .collect()
}

/// Full tool output for an expanded card in a code fence so it renders
/// verbatim; JSON is pretty-printed, also when a trailer line follows it
/// (the shell tool appends `exit code: n`).
pub fn output_markdown(output: &str) -> String {
    let output = output.trim_end();
    let pretty = |s: &str| {
        serde_json::from_str::<serde_json::Value>(s.trim())
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
    };
    if let Some(json) = pretty(output) {
        return format!("```json\n{json}\n```");
    }
    if let Some((head, tail)) = output.rsplit_once('\n')
        && let Some(json) = pretty(head)
    {
        return format!("```json\n{json}\n```\n{tail}");
    }
    format!("```\n{output}\n```")
}

/// First output line plus a `(+n lines)` tail for the rest.
pub fn result_line(output: &str) -> String {
    let mut lines = output.lines();
    let first = lines.next().unwrap_or_default();
    match lines.count() {
        0 => first.to_owned(),
        n => format!("{first}  (+{n} lines)"),
    }
}

/// Visible slice of a list `max` rows tall that keeps `cursor` in view.
pub fn window(len: usize, cursor: usize, max: usize) -> Range<usize> {
    let start = (cursor + 1)
        .saturating_sub(max)
        .min(len.saturating_sub(max));
    start..len.min(start + max)
}

/// Where a `/model` / `/effort` pick goes (125 §7.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PickTarget {
    /// `set_model` / `set_effort`: the shown session (and the default).
    #[default]
    Session,
    /// `set_advisor`: the shown session's advisor.
    Advisor,
}

/// Why a `set_advisor` / `advisor_status` call was made (125 §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdvisorReply {
    /// `/advisor status`: print the lines of §7.3.
    Status,
    /// A model pick: chain into the advisor's effort picker when it has levels.
    Pick,
}

/// An open menu; modal except `Slash`, which rides on the input box.
#[derive(Debug)]
pub enum Menu {
    /// Command list filtered by the input's first word.
    Slash {
        /// Highlighted match.
        cursor: usize,
    },
    /// `/model` picker (124 §3): providers level, a provider's models, or word search.
    Models {
        /// Everything `ListModels` returned, in catalog order (provider, id).
        items: Vec<ModelInfo>,
        /// Search text typed inline; non-empty means search mode over every model.
        filter: String,
        /// Highlighted row of `models_rows()`.
        cursor: usize,
        /// The provider whose models are listed; `None` is the providers level.
        provider: Option<String>,
        /// Whose model a pick sets (125 §7.2): the session's or its advisor's.
        target: PickTarget,
    },
    /// `/effort` picker.
    Efforts {
        /// The model's effort levels, then `clear`.
        items: Vec<String>,
        /// Highlighted row.
        cursor: usize,
        /// Whose effort a pick sets.
        target: PickTarget,
    },
    /// `/aws` picker.
    AwsProfiles {
        /// The harness's profiles and selection; row 0 is the default chain.
        info: AwsProfiles,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/sessions` picker.
    Sessions {
        /// Project headings, then roots with their children under them (51 §3).
        items: Vec<Grouped<(SessionSummary, usize)>>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/search` (49 §6): the live menu over [`Model::search`].
    Search,
    /// `/attach` (66 §10): the search menu picking a reference for the input box.
    Attach,
    /// `/agents` picker.
    Agents {
        /// The current session's subtree: running first, then awaiting /
        /// approval, then the rest newest-finished first.
        items: Vec<AgentRow>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/mcp` switches and risk lines.
    Mcp {
        /// Latest `ListServers` reply.
        servers: Vec<McpServer>,
        /// Highlighted row of [`Model::mcp_rows`].
        cursor: usize,
        /// Servers whose tools are listed.
        expanded: BTreeSet<String>,
        /// ⏎ on an `on` row: waiting for the scope (`s` session / `a` always).
        pending: Option<Target>,
    },
    /// `/skills` picker.
    Skills {
        /// Every skill, as indexed.
        items: Vec<SkillMeta>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/login` picker.
    Logins {
        /// Providers then OAuth MCP servers.
        items: Vec<LoginRow>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/acp`: configured harnesses, then the registry.
    Acp {
        /// Latest `ListAcp` reply.
        entries: Vec<EntryInfo>,
        /// Latest `ListAcpRegistry` reply; empty until it arrives.
        registry: Vec<RegistryAgent>,
        /// Highlighted row: entries first, then registry rows.
        cursor: usize,
    },
    /// `/edit`, `/regen`, `/fork` picker over the transcript's turns (29 §6.2).
    Turns {
        /// `(entry index, turn seq, first line of the opener)`, oldest first.
        items: Vec<(usize, i64, String)>,
        /// Highlighted row; starts on the newest.
        cursor: usize,
        /// What ⏎ does with the highlighted turn.
        action: TurnAction,
    },
    /// `/link`: paired devices over [`Model::link`] (31 §6.5).
    Link {
        /// Highlighted device.
        cursor: usize,
    },
    /// `/nodes`: paired nodes over [`Model::nodes`] (32 §6.7).
    Nodes {
        /// Highlighted node.
        cursor: usize,
    },
    /// `/artifacts` picker (35 §6.2); `/apps` when `apps` (46 §8.2).
    Artifacts {
        /// Project and session headings, then the rows by name and version (51 §3).
        items: Vec<Grouped<artifact::Artifact>>,
        /// Highlighted row.
        cursor: usize,
        /// Rows are apps: the title says so and where they run.
        apps: bool,
    },
    /// `/secrets`: the vault's names over [`Model::secrets`] (37 §5).
    Secrets {
        /// Highlighted secret.
        cursor: usize,
        /// History of the highlighted secret is open: the highlighted
        /// version row (0 = current). `None` shows the names.
        history: Option<usize>,
    },
    /// `/knowledge [query]` picker (10 §6).
    Knowledge {
        /// The list or search reply.
        items: Vec<Meta>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/knowledge history [id]`: commits newest first, then one commit's
    /// diff against its parent. Opened when asked, filled by the reply that
    /// matches `id` and `limit`; later or stale replies are dropped.
    KnowledgeHistory {
        /// Concept id, `None` for the whole bundle.
        id: Option<String>,
        /// Shown commits, at most `limit`.
        changes: Vec<Change>,
        /// Page size asked for; `⏎` on the `older` row asks for 20 more.
        limit: usize,
        /// Commits exist past `changes`: the `older` row shows.
        more: bool,
        /// Highlighted commit (or the `older` row at `changes.len()`).
        cursor: usize,
        /// A page request is in flight.
        loading: bool,
        /// Commit whose revision is being fetched; cleared by moving.
        pending: Option<String>,
        /// The last failed request's message, shown in place of the rows.
        error: Option<String>,
        /// Open diff of the highlighted commit: first shown row, first shown column.
        revision: Option<(Revision, usize, usize)>,
    },
    /// `/shake` picker and review (48 §2.4).
    Shake(Box<Shake>),
    /// `/broadcast` picker, composer and review (62 §5.1).
    Broadcast(Box<Broadcast>),
    /// `/peers` (50 §5.1, §5.3): the directory `session` may address, then
    /// the requests a human may resolve for it by exact id. Opened at once,
    /// filled by the replies that name `session`; a switch drops the rest.
    Peers {
        /// Whose view.
        session: SessionId,
        /// `list_peers`: `None` until it arrives; `Err` is the blocker (communication off).
        peers: Option<Result<PeerPage, String>>,
        /// `list_peer_messages(pending_only)`; `None` until it arrives.
        requests: Option<PeerMessagePage>,
        /// Highlighted row of [`Model::peers_rows`].
        cursor: usize,
    },
    /// `/cost`: the shown session's owner summary (51 §6.5, §8.3).
    Cost {
        /// The summary at open (or the latest refresh).
        cost: SessionCost,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/terminals` picker (51 §7.4).
    Terminals {
        /// PTY jobs of the shown session's tree.
        items: Vec<TerminalSummary>,
        /// Highlighted row.
        cursor: usize,
    },
    /// The read-only terminal observer (51 §7.4); drawn by `terminal_box`, never `menu`.
    Terminal(Box<TerminalView>),
}

/// The private root this view owns (51 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Private {
    /// The root.
    pub root: SessionId,
    /// Lease secret presented on leave.
    pub secret: String,
    /// Tree members seen so far (the root, its subtree as listed or spawned):
    /// navigating among them is not a leave.
    pub members: std::collections::HashSet<SessionId>,
}

impl Private {
    /// `id` is in the owned tree as far as this view has seen.
    pub fn member(&self, id: SessionId) -> bool {
        id == self.root || self.members.contains(&id)
    }
}

/// A picker row that may be a heading: the cursor skips headings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grouped<T> {
    /// Non-selectable label.
    Heading(String),
    /// A selectable row.
    Item(T),
}

impl<T> Grouped<T> {
    /// The item, if this is one.
    pub fn item(&self) -> Option<&T> {
        match self {
            Grouped::Item(t) => Some(t),
            Grouped::Heading(_) => None,
        }
    }
}

/// Cursor after moving `delta` selectable rows from `cursor` (headings skipped, ends clamp).
pub fn grouped_step<T>(rows: &[Grouped<T>], cursor: usize, delta: isize) -> usize {
    let mut at = cursor.min(rows.len().saturating_sub(1));
    for _ in 0..delta.unsigned_abs() {
        let next = if delta < 0 {
            (0..at).rev().find(|&i| rows[i].item().is_some())
        } else {
            (at + 1..rows.len()).find(|&i| rows[i].item().is_some())
        };
        match next {
            Some(i) => at = i,
            None => break,
        }
    }
    at
}

/// First selectable row (`0` when there is none).
pub fn grouped_first<T>(rows: &[Grouped<T>]) -> usize {
    rows.iter().position(|r| r.item().is_some()).unwrap_or(0)
}

/// Heading for a project label; labels are display data, control characters dropped.
fn project_heading(project: Option<&str>) -> String {
    match project {
        Some(p) => p.chars().filter(|c| !c.is_control()).collect(),
        None => "Ungrouped".to_owned(),
    }
}

/// Projects in first-seen order over `roots` (the pinned/newest list order), `None` last (51 §3).
fn project_order<'a>(roots: impl Iterator<Item = Option<&'a str>>) -> Vec<Option<&'a str>> {
    let mut order: Vec<Option<&str>> = Vec::new();
    let mut ungrouped = false;
    for p in roots {
        match p {
            None => ungrouped = true,
            Some(p) if !order.contains(&Some(p)) => order.push(Some(p)),
            Some(_) => {}
        }
    }
    if ungrouped {
        order.push(None);
    }
    order
}

/// `/sessions` rows: a heading per project in first-seen order of the roots
/// (Ungrouped last), each root followed by its descendants, depth-first, in
/// list order (51 §3). A roster desk (`desk` set) is not a chat: it and its
/// tree (reached only through it) are left out.
pub fn session_rows(mut list: Vec<SessionSummary>) -> Vec<Grouped<(SessionSummary, usize)>> {
    list.retain(|s| s.desk.is_none());
    let mut children: HashMap<SessionId, Vec<usize>> = HashMap::new();
    for (i, s) in list.iter().enumerate() {
        if let Some(p) = s.parent {
            children.entry(p).or_default().push(i);
        }
    }
    let roots: Vec<usize> = (0..list.len())
        .filter(|&i| list[i].parent.is_none())
        .collect();
    let order = project_order(roots.iter().map(|&i| list[i].project.as_deref()));
    fn walk(
        list: &[SessionSummary],
        children: &HashMap<SessionId, Vec<usize>>,
        i: usize,
        depth: usize,
        out: &mut Vec<Grouped<(SessionSummary, usize)>>,
    ) {
        out.push(Grouped::Item((list[i].clone(), depth)));
        for &c in children.get(&list[i].id).map_or(&[][..], Vec::as_slice) {
            walk(list, children, c, depth + 1, out);
        }
    }
    let mut rows = Vec::with_capacity(list.len() + order.len());
    for project in order {
        rows.push(Grouped::Heading(project_heading(project)));
        for &r in roots
            .iter()
            .filter(|&&r| list[r].project.as_deref() == project)
        {
            walk(&list, &children, r, 0, &mut rows);
        }
    }
    rows
}

/// What `/artifacts` knows about an owner: its effective project and a label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// Effective project of the owner's root.
    pub project: Option<String>,
    /// Heading text: `short-id  title`, a peer name, or `Unavailable session`.
    pub label: String,
}

/// `/artifacts` rows grouped effective project → session → name/version
/// (51 §3). `owner` resolves an artifact's session; `None` is a missing
/// durable owner, shown as `Unavailable session` under Ungrouped.
pub fn artifact_rows(
    items: Vec<artifact::Artifact>,
    owner: impl Fn(SessionId) -> Option<Owner>,
) -> Vec<Grouped<artifact::Artifact>> {
    // Sessions in first-seen order (the list is newest first), each with its rows.
    let mut sessions: Vec<(SessionId, Owner, Vec<artifact::Artifact>)> = Vec::new();
    for a in items {
        match sessions.iter_mut().find(|(s, ..)| *s == a.session) {
            Some((_, _, rows)) => rows.push(a),
            None => {
                let o = owner(a.session).unwrap_or(Owner {
                    project: None,
                    label: "Unavailable session".to_owned(),
                });
                sessions.push((a.session, o, vec![a]));
            }
        }
    }
    for (_, _, rows) in &mut sessions {
        rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    }
    let order = project_order(sessions.iter().map(|(_, o, _)| o.project.as_deref()));
    let mut out = Vec::new();
    for project in order {
        out.push(Grouped::Heading(project_heading(project)));
        for (_, o, rows) in sessions
            .iter()
            .filter(|(_, o, _)| o.project.as_deref() == project)
        {
            out.push(Grouped::Heading(format!("  {}", o.label)));
            out.extend(rows.iter().cloned().map(Grouped::Item));
        }
    }
    out
}

/// Poll period of the open terminal observer (51 §7.3: at most one ask per half second).
pub const TERMINAL_POLL_MS: u64 = 500;
/// Poll period while the human is typing into the live screen (55 §3).
pub const TERMINAL_INTERACTIVE_POLL_MS: u64 = 60;
/// Rows a history page asks for.
const TERMINAL_PAGE: u16 = 40;

/// The `/terminals` observer over one PTY job (51 §7.4): what is asked
/// for, what was last painted, and the request bookkeeping that keeps one
/// ask in flight and drops replies for a selection or viewport left behind.
#[derive(Debug)]
pub struct TerminalView {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub job: JobId,
    /// `name · command` for the title.
    pub label: String,
    /// Rows wanted; `Live` follows the screen.
    pub viewport: PtyViewport,
    /// Bumped by [`Model`] whenever the selected terminal changes; a reply
    /// with another epoch is dropped even if session and job match.
    pub epoch: u64,
    /// Last page painted.
    pub page: Option<PtyPage>,
    /// Revision of `page` (or of the last `Unchanged`).
    pub revision: Option<u64>,
    /// An ask is out; nothing else is sent until it answers.
    pub inflight: bool,
    /// Something changed while an ask was out: re-ask as soon as it answers.
    pub dirty: bool,
    /// Unix ms of the next scheduled poll.
    pub next_poll: u64,
    /// Leading columns dropped (horizontal pan).
    pub col: usize,
    /// The owner said the terminal is gone.
    pub unavailable: bool,
    /// Daemon epoch of the first page; another one means the daemon restarted.
    pub daemon_epoch: Option<Option<String>>,
    /// A later page named another daemon epoch.
    pub restarted: bool,
    /// The last failed ask, shown in the status row.
    pub error: Option<String>,
    /// Keys go to the process (55 §3); Ctrl-] returns to viewing.
    pub interactive: bool,
}

impl TerminalView {
    /// A fresh observer on `t`, following the live screen.
    pub fn new(t: &TerminalSummary, epoch: u64) -> Self {
        let label = match &t.name {
            Some(n) => format!("{n} · {}", t.command),
            None => t.command.clone(),
        };
        Self {
            session: t.session,
            job: t.id.clone(),
            label,
            viewport: PtyViewport::Live,
            epoch,
            page: None,
            revision: None,
            inflight: false,
            dirty: false,
            next_poll: 0,
            col: 0,
            unavailable: false,
            daemon_epoch: None,
            restarted: false,
            error: None,
            interactive: false,
        }
    }

    /// Change the viewport: what was painted no longer matches, so the next
    /// ask carries no `if_revision`.
    fn set_viewport(&mut self, v: PtyViewport) {
        if v != self.viewport {
            self.viewport = v;
            self.revision = None;
            self.dirty = true;
            self.next_poll = 0;
        }
    }

    /// Scroll `rows` up (negative) or down (positive) through history; past
    /// the bottom the view is live again.
    fn scroll(&mut self, rows: i64) {
        let retained = self.page.as_ref().map_or(0, |p| u64::from(p.retained));
        let (offset, height) = match self.viewport {
            PtyViewport::Live => (0i64, TERMINAL_PAGE),
            PtyViewport::History { offset, height } => (i64::from(offset), height),
        };
        // Up means an older page: a larger offset above the live screen.
        let offset = (offset - rows).clamp(0, i64::try_from(retained).unwrap_or(i64::MAX));
        let v = if offset == 0 {
            PtyViewport::Live
        } else {
            PtyViewport::History {
                offset: u32::try_from(offset).unwrap_or(u32::MAX),
                height,
            }
        };
        self.set_viewport(v);
    }

    /// Fold a reply in. Applies only when it answers the current selection
    /// and viewport and is not older than what is shown; an `Unchanged` for
    /// another viewport re-asks. Returns whether the screen changed.
    pub fn reply(
        &mut self,
        session: SessionId,
        job: &JobId,
        viewport: PtyViewport,
        epoch: u64,
        reply: Result<PtyScreenReply, String>,
    ) -> bool {
        if session != self.session || *job != self.job || epoch != self.epoch {
            return false;
        }
        self.inflight = false;
        if viewport != self.viewport {
            // Answers the viewport before a scroll; the scroll already re-asked.
            self.dirty = true;
            self.next_poll = 0;
            return false;
        }
        match reply {
            Err(e) => {
                self.error = Some(e);
                false
            }
            Ok(PtyScreenReply::Unavailable) => {
                self.error = None;
                self.unavailable = true;
                true
            }
            Ok(PtyScreenReply::Unchanged {
                viewport: v,
                revision,
            }) => {
                self.error = None;
                if v != self.viewport {
                    self.revision = None;
                    self.dirty = true;
                    self.next_poll = 0;
                } else if self.revision.is_none_or(|r| r <= revision) {
                    self.revision = Some(revision);
                }
                false
            }
            Ok(PtyScreenReply::Updated(page)) => {
                self.error = None;
                if page.session != self.session
                    || page.id != self.job
                    || self.revision.is_some_and(|r| page.revision < r)
                {
                    return false;
                }
                match &self.daemon_epoch {
                    None => self.daemon_epoch = Some(page.epoch.clone()),
                    Some(e) if *e != page.epoch => self.restarted = true,
                    Some(_) => {}
                }
                self.revision = Some(page.revision);
                self.unavailable = false;
                if page.exit.is_some() {
                    self.interactive = false;
                }
                self.page = Some(page);
                true
            }
        }
    }

    /// The next ask when one is due at `now` and none is out: marks it in
    /// flight and schedules the following poll.
    pub fn poll(&mut self, now: u64) -> Option<Command> {
        if self.inflight || self.unavailable || (!self.dirty && now < self.next_poll) {
            return None;
        }
        self.inflight = true;
        self.dirty = false;
        let period = if self.interactive && self.viewport == PtyViewport::Live {
            TERMINAL_INTERACTIVE_POLL_MS
        } else {
            TERMINAL_POLL_MS
        };
        self.next_poll = now + period;
        Some(Command::PtyScreen {
            session: self.session,
            job: self.job.clone(),
            viewport: self.viewport,
            if_revision: self.revision,
            epoch: self.epoch,
        })
    }

    /// When the event loop must next wake for this view (unix ms); `None` while nothing is due.
    pub fn wake(&self) -> Option<u64> {
        (!self.inflight && !self.unavailable).then_some(if self.dirty { 0 } else { self.next_poll })
    }

    /// Title row: label, owner, state, viewport and revision.
    pub fn title(&self) -> String {
        let state = match &self.page {
            Some(p) => match &p.exit {
                Some(e) => format!("exit {e}"),
                None => "running".to_owned(),
            },
            None => "loading…".to_owned(),
        };
        let where_ = match self.viewport {
            PtyViewport::Live => "live".to_owned(),
            PtyViewport::History { offset, .. } => format!("history −{offset}"),
        };
        let rev = self
            .revision
            .map_or_else(String::new, |r| format!(" · rev {r}"));
        let keys = if self.interactive {
            "typing · Ctrl-] to stop"
        } else if self.page.as_ref().is_some_and(|p| p.exit.is_none()) {
            "↑↓ PgUp PgDn history  ←→ pan  r refresh  i type  esc close"
        } else {
            "↑↓ PgUp PgDn history  ←→ pan  r refresh  esc close"
        };
        format!(
            "terminal · {} · {} · {state} · {where_}{rev}   {keys}",
            short(self.session),
            self.job.0,
        )
    }

    /// Status row under the page, if anything needs saying.
    pub fn status(&self) -> Option<String> {
        if self.unavailable {
            return Some("No longer available.".to_owned());
        }
        let mut notes = Vec::new();
        if self.restarted {
            notes.push("Daemon restarted.".to_owned());
        }
        if let Some(p) = &self.page {
            if !p.redaction_complete {
                notes.push("redaction incomplete at retained boundary".to_owned());
            }
            if p.truncated {
                notes.push("top rows cut to fit the reply".to_owned());
            }
        }
        if let Some(e) = &self.error {
            notes.push(format!("last ask failed: {e}"));
        }
        (!notes.is_empty()).then(|| notes.join(" · "))
    }
}

/// The `/shake` argument: which kind of context item the picker lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeCategory {
    /// Image occurrences: user images, tool-response screenshots, snap-head PNGs.
    Images,
    /// Native PDF parts.
    Documents,
    /// Whole messages; nothing preselected.
    Messages,
    /// Tool exchanges with their protocol groups.
    Tools,
    /// The current epoch head.
    Summary,
}

impl ShakeCategory {
    /// The literal `/shake` argument; `None` for anything else (the caller prints usage).
    pub fn parse(arg: &str) -> Option<Self> {
        Some(match arg {
            "images" => Self::Images,
            "documents" => Self::Documents,
            "messages" => Self::Messages,
            "tools" => Self::Tools,
            "summary" => Self::Summary,
            _ => return None,
        })
    }

    /// The inventory kind this category lists.
    fn kind(self) -> ContextKind {
        match self {
            Self::Images => ContextKind::Image,
            Self::Documents => ContextKind::Document,
            Self::Messages => ContextKind::Message,
            Self::Tools => ContextKind::Tool,
            Self::Summary => ContextKind::Summary,
        }
    }

    /// Plural noun for the zero-matches notice.
    fn noun(self) -> &'static str {
        match self {
            Self::Images => "images",
            Self::Documents => "documents",
            Self::Messages => "messages",
            Self::Tools => "tool exchanges",
            Self::Summary => "summary",
        }
    }
}

/// Which `/shake` screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeStage {
    /// Ticking inventory rows.
    Pick,
    /// The exact removal set, awaiting ⏎.
    Review,
}

/// `/shake` state (48 §2.4): the server's whole active inventory, what the
/// picker lists and what is ticked. Targets come from the inventory only;
/// the transcript on screen is never a deletion handle.
#[derive(Debug)]
pub struct Shake {
    /// Session the inventory came from.
    pub session: SessionId,
    /// Inventory revision; the removal is refused if the session moved on.
    pub revision: String,
    /// The inventory, in server order.
    pub items: Vec<ContextItem>,
    /// Indices into `items` the picker lists: the category's kind, or all.
    pub shown: Vec<usize>,
    /// Ticked indices into `items`.
    pub selected: BTreeSet<usize>,
    /// Highlighted row of `shown` (pick) or `review` (review).
    pub cursor: usize,
    /// Which screen is up.
    pub stage: ShakeStage,
    /// The removal set, built when review opens.
    pub review: Vec<ReviewRow>,
}

/// One line of the `/shake` review: an exact target and why it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRow {
    /// What `remove_context` gets.
    pub target: ContextTarget,
    /// Inventory kind (`Message` for a companion the inventory does not list).
    pub kind: ContextKind,
    /// The inventory label.
    pub label: String,
    /// Not ticked: a protocol companion of a ticked message (48 §4.2).
    pub companion: bool,
}

/// Lowercase kind for picker rows and the print-mode inventory.
pub fn kind_name(k: ContextKind) -> &'static str {
    match k {
        ContextKind::Message => "message",
        ContextKind::Image => "image",
        ContextKind::Document => "document",
        ContextKind::Tool => "tool",
        ContextKind::Summary => "summary",
    }
}

/// The message or head a binary target sits in.
fn owner_target(t: ContextTarget) -> Option<ContextTarget> {
    match t {
        ContextTarget::Binary { owner, .. } => Some(match owner {
            agent::ContextOwner::Message { seq } => ContextTarget::Message { seq },
            agent::ContextOwner::Head { epoch } => ContextTarget::Head { epoch },
        }),
        _ => None,
    }
}

/// The exact removal set (48 §3.2): the ticked items plus their
/// `remove_with` companions, followed to closure, each target once, in
/// inventory order. A whole message or head subsumes its attachments, so a
/// ticked image inside a ticked message is not listed twice.
pub fn expand(items: &[ContextItem], selected: &BTreeSet<usize>) -> Vec<ReviewRow> {
    let picked: BTreeSet<ContextTarget> = selected
        .iter()
        .filter_map(|&i| items.get(i))
        .map(|it| it.target)
        .collect();
    let mut companions = BTreeSet::new();
    let mut queue: Vec<ContextTarget> = picked.iter().copied().collect();
    while let Some(t) = queue.pop() {
        for &c in items
            .iter()
            .filter(|it| it.target == t)
            .flat_map(|it| &it.remove_with)
        {
            if !picked.contains(&c) && companions.insert(c) {
                queue.push(c);
            }
        }
    }
    let subsumed = |t: ContextTarget| {
        owner_target(t).is_some_and(|o| picked.contains(&o) || companions.contains(&o))
    };
    let mut rows: Vec<ReviewRow> = items
        .iter()
        .filter(|it| {
            (picked.contains(&it.target) || companions.contains(&it.target)) && !subsumed(it.target)
        })
        .map(|it| ReviewRow {
            target: it.target,
            kind: it.kind,
            label: it.label.clone(),
            companion: !picked.contains(&it.target),
        })
        .collect();
    // A companion the inventory does not list (the server names a row it
    // hid): still part of the set, labelled by its coordinate.
    for &c in &companions {
        if !subsumed(c) && !rows.iter().any(|r| r.target == c) {
            rows.push(ReviewRow {
                target: c,
                kind: ContextKind::Message,
                label: match c {
                    ContextTarget::Message { seq } => format!("message #{seq}"),
                    ContextTarget::Head { epoch } => format!("summary (epoch {epoch})"),
                    ContextTarget::Binary { part, .. } => format!("attachment part {part}"),
                },
                companion: true,
            });
        }
    }
    rows
}

impl Shake {
    /// Tick or untick the highlighted picker row.
    fn toggle(&mut self) {
        if let Some(&i) = self.shown.get(self.cursor)
            && !self.selected.remove(&i)
        {
            self.selected.insert(i);
        }
    }

    /// Build the removal set and show it.
    fn open_review(&mut self) {
        self.review = expand(&self.items, &self.selected);
        self.stage = ShakeStage::Review;
        self.cursor = 0;
    }

    /// `(messages, attachments, heads, companions)` of the review.
    fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for r in &self.review {
            match r.target {
                ContextTarget::Message { .. } => c.0 += 1,
                ContextTarget::Binary { .. } => c.1 += 1,
                ContextTarget::Head { .. } => c.2 += 1,
            }
            c.3 += usize::from(r.companion);
        }
        c
    }

    /// Picker rows: `[x] image     shot.png · +2 messages` (the companions a
    /// tick pulls in); review rows: the set with companions and summary
    /// warnings spelled out.
    pub fn rows(&self) -> Vec<String> {
        match self.stage {
            ShakeStage::Pick => self
                .shown
                .iter()
                .map(|&i| {
                    let it = &self.items[i];
                    let more = match it.remove_with.len() {
                        0 => String::new(),
                        n => format!(" · +{n} message(s)"),
                    };
                    format!(
                        "[{}] {:<8} {}{more}",
                        if self.selected.contains(&i) { "x" } else { " " },
                        kind_name(it.kind),
                        it.label
                    )
                })
                .collect(),
            ShakeStage::Review => self
                .review
                .iter()
                .map(|r| {
                    let note = match r.target {
                        ContextTarget::Head { .. } => " · summarized earlier context will be lost",
                        ContextTarget::Binary {
                            owner: agent::ContextOwner::Head { .. },
                            ..
                        } => {
                            " · summary image: removing this loses part of the earlier conversation"
                        }
                        _ if r.companion => " · protocol companion",
                        _ => "",
                    };
                    format!("{:<8} {}{note}", kind_name(r.kind), r.label)
                })
                .collect(),
        }
    }

    /// Menu title: keys and, in review, the counts and what removal means.
    pub fn title(&self) -> String {
        match self.stage {
            ShakeStage::Pick => format!(
                "shake  ↑↓ move  space tick  ⏎ review  esc close   {} ticked",
                self.selected.len()
            ),
            ShakeStage::Review => {
                let (m, a, h, c) = self.counts();
                format!(
                    "remove from context: {m} message(s), {a} attachment(s), {h} head(s), {c} companion(s)  ⏎ remove  esc back   originals stay in history; later replies stay; /undo does not restore"
                )
            }
        }
    }
}

/// A `/link pair` or `/nodes` invite card that is still counting down.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PairingCard {
    /// The pairing string (rendered as the QR and on one line).
    key: String,
    /// Unix ms.
    expires_at_ms: u64,
    /// Whole seconds the card last showed; redraw when it changes.
    left: u64,
}

impl PairingCard {
    /// A card over `p`, showing its whole seconds left as of now.
    fn new(p: link::Pairing) -> Self {
        Self {
            left: p.expires_at_ms.saturating_sub(now_ms()) / 1000,
            key: p.key,
            expires_at_ms: p.expires_at_ms,
        }
    }

    /// Seconds until expiry, floored at zero.
    fn remaining(&self) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms()) / 1000
    }

    /// Card body: the QR in a fence, the key, the countdown.
    fn body(&self) -> String {
        let left = self.left;
        format!(
            "```\n{}\n```\n{}\nexpires in {}:{:02}",
            link::qr(&self.key),
            self.key,
            left / 60,
            left % 60
        )
    }
}

/// Redraw the live countdown card in `slot` when its countdown moved a
/// second; at zero it reads `pairing expired` and the slot empties.
fn tick_card(entries: &mut [Entry], slot: &mut Option<PairingCard>, kind: CardKind) {
    let Some(p) = slot else { return };
    let left = p.remaining();
    if left == p.left {
        return;
    }
    p.left = left;
    let body = if left == 0 {
        "pairing expired".to_owned()
    } else {
        p.body()
    };
    if left == 0 {
        *slot = None;
    }
    set_card(entries, kind, &body);
}

/// Replace the body of the newest card of `kind`, if any.
fn set_card(entries: &mut [Entry], kind: CardKind, body: &str) {
    if let Some(Entry::Card { text, .. }) = entries
        .iter_mut()
        .rev()
        .find(|e| matches!(e, Entry::Card { kind: k, .. } if *k == kind))
    {
        **text = markdown::Text::complete(body);
    }
}

/// Unix ms now.
pub(crate) fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// A wire context revision (decimal string, 48 §3.1) as a number; `0` when
/// missing, so an unloaded transcript is older than any loaded one.
fn revision(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

/// What a [`Menu::Turns`] pick does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TurnAction {
    /// Put the opener's text in the editor; ⏎ replaces the turn with it.
    Edit,
    /// Regenerate the turn's reply.
    Regen,
    /// New session with the conversation through the turn.
    Fork,
}

/// Multi-step input (`/provider add`, `/link` rename / revoke, `/nodes` add /
/// rename / remove, `/secrets` add / replace / delete). Rides on the input
/// box (not a modal menu).
#[derive(Debug)]
pub enum Prompt {
    /// Asking for the gateway name.
    ProviderName,
    /// Asking for the base URL.
    ProviderUrl {
        /// Name already collected.
        name: String,
    },
    /// Asking for the API key.
    ProviderKey {
        /// Name already collected.
        name: String,
        /// Normalized URL already collected.
        url: String,
    },
    /// `/link` `r`: asking for a paired device's new name.
    LinkDeviceName {
        /// `Device.id`.
        id: String,
        /// Its current name (the hint).
        name: String,
    },
    /// `/link` `n`: asking for the harness display name.
    LinkName,
    /// `/link` `x`: `y` revokes the device, anything else keeps it.
    LinkRevoke {
        /// `Device.id`.
        id: String,
        /// Its name (the hint).
        name: String,
    },
    /// `/nodes` `a`: asking for a node's pasted pairing key.
    NodeKey,
    /// `/nodes` `r`: asking for a node's new local alias.
    NodeName {
        /// `Node.id`.
        id: String,
        /// Its current name (the hint).
        name: String,
    },
    /// `/nodes` `x`: `y` removes the node, anything else keeps it.
    NodeRemove {
        /// `Node.id`.
        id: String,
        /// Its name (the hint).
        name: String,
    },
    /// `/secrets add`: asking for the name.
    SecretName,
    /// `/secrets add NAME` / `r`: asking for the value (masked).
    SecretValue {
        /// Name already collected.
        name: String,
        /// `Some(v)` when the name exists: the value adds `v + 1`.
        current: Option<u32>,
    },
    /// `/secrets add`: asking for the note; an empty line keeps the old one (or none).
    SecretNote {
        /// Name already collected.
        name: String,
        /// Value already collected. Never written to the transcript.
        value: String,
    },
    /// `/secrets` `d`: typing the name again deletes it, anything else keeps it.
    SecretDelete {
        /// The name.
        name: String,
    },
    /// `/peers` `a`: typing the answer to one exact pending request (50 §5.3).
    PeerAnswer {
        /// The session the human acts for.
        session: SessionId,
        /// The request.
        reply_to: Uuid,
        /// Its authenticated sender (the hint).
        from: String,
        /// `session` sent the request and is blocked on it: the answer
        /// resolves its own wait instead of travelling to a peer.
        resolve: bool,
    },
}

impl Prompt {
    /// Line shown above the input.
    pub fn hint(&self) -> String {
        match self {
            Prompt::ProviderName => "provider add  name  (esc cancel)".into(),
            Prompt::ProviderUrl { name } => {
                format!("provider add  {name}  base url  (esc cancel)")
            }
            Prompt::ProviderKey { name, .. } => {
                format!("provider add  {name}  api key  (esc cancel)")
            }
            Prompt::LinkDeviceName { name, .. } => {
                format!("link  rename {name}  new name  (esc cancel)")
            }
            Prompt::LinkName => "link  node name  (esc cancel)".into(),
            Prompt::LinkRevoke { name, .. } => format!("link  revoke {name}?  y/n"),
            Prompt::NodeKey => "nodes  add  paste the node's key  (esc cancel)".into(),
            Prompt::NodeName { name, .. } => {
                format!("nodes  rename {name}  new name  (esc cancel)")
            }
            Prompt::NodeRemove { name, .. } => format!("nodes  remove {name}?  y/n"),
            Prompt::SecretName => "secrets add  name  (esc cancel)".into(),
            Prompt::SecretValue {
                name,
                current: None,
            } => format!("secrets add  {name}  value  (esc cancel)"),
            Prompt::SecretValue {
                name,
                current: Some(v),
            } => format!(
                "secrets add  {name}  value  (replaces v{v} → v{}; esc cancel)",
                v + 1
            ),
            Prompt::SecretNote { name, .. } => {
                format!("secrets add  {name}  note  (⏎ none; esc cancel)")
            }
            Prompt::SecretDelete { name } => {
                format!("secrets  delete {name}?  type the name to confirm  (esc cancel)")
            }
            Prompt::PeerAnswer {
                reply_to,
                from,
                resolve,
                ..
            } => format!(
                "peers  {} {} · {}  answer  (esc cancel)",
                if *resolve {
                    "resolve own wait on"
                } else {
                    "answer for this session to"
                },
                short_uuid(*reply_to),
                from
            ),
        }
    }

    /// Notice when Esc drops the flow.
    fn cancelled(&self) -> &'static str {
        match self {
            Prompt::ProviderName | Prompt::ProviderUrl { .. } | Prompt::ProviderKey { .. } => {
                "provider add cancelled"
            }
            Prompt::LinkDeviceName { .. } | Prompt::LinkName | Prompt::LinkRevoke { .. } => {
                "link: cancelled"
            }
            Prompt::NodeKey | Prompt::NodeName { .. } | Prompt::NodeRemove { .. } => {
                "nodes: cancelled"
            }
            Prompt::SecretName
            | Prompt::SecretValue { .. }
            | Prompt::SecretNote { .. }
            | Prompt::SecretDelete { .. } => "secrets: cancelled",
            Prompt::PeerAnswer { .. } => "peers: answer cancelled",
        }
    }

    /// The input box hides what is typed: a secret's value, a provider's key.
    pub fn masked(&self) -> bool {
        matches!(
            self,
            Prompt::ProviderKey { .. } | Prompt::SecretValue { .. }
        )
    }
}

/// A pending `ask_user` being answered one question at a time.
#[derive(Debug)]
pub struct Ask {
    /// The request.
    pub req: AskRequested,
    /// Index of the question being shown.
    pub current: usize,
    /// Row under the cursor: an option, or `options.len()` for "type your own".
    pub cursor: usize,
    /// Answers so far, one per finished question.
    pub answers: Vec<Answer>,
    /// The input box is taking a freeform answer.
    pub typing: bool,
}

impl Ask {
    fn new(req: AskRequested) -> Self {
        let cursor = req
            .questions
            .first()
            .and_then(|q| q.recommended)
            .unwrap_or(0);
        Self {
            req,
            current: 0,
            cursor,
            answers: Vec::new(),
            typing: false,
        }
    }

    /// The question being shown.
    pub fn question(&self) -> &tools::Question {
        &self.req.questions[self.current]
    }
}

/// A pending `secret_request` (37 §5): the masked box in the approval box's place.
#[derive(Debug)]
pub struct SecretRequest {
    /// Answer with this id.
    pub id: secrets::RequestId,
    /// Name it will be stored under.
    pub name: String,
    /// What it is for.
    pub note: String,
    /// `Some(v)` when the name exists: the answer adds `v + 1`.
    pub current: Option<u32>,
}

/// One line of the `/mcp` menu.
#[derive(Debug)]
pub enum Row<'a> {
    /// A server (its line applies to all its tools).
    Server(&'a McpServer),
    /// A tool of an expanded server.
    Tool(&'a McpServer, &'a McpToolInfo),
}

/// What a `/mcp` switch applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Server name.
    pub server: String,
    /// Tool name; `None` = the whole server.
    pub tool: Option<String>,
}

/// Whether a `/mcp` row is on, and if not, which switch turned it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Visible to the model.
    On,
    /// Hidden for this session (`set_mcp_hidden`); other sessions still see it.
    OffSession,
    /// `off` in `mcp/config.yaml`: every session, every restart.
    OffAlways,
}

impl Switch {
    /// Column text.
    pub fn label(self) -> &'static str {
        match self {
            Switch::On => "on",
            Switch::OffSession => "off · this session",
            Switch::OffAlways => "off · always",
        }
    }
}

impl Row<'_> {
    /// The row's switch. A server is `OffSession` when every tool it has
    /// registered is hidden.
    pub fn switch(&self) -> Switch {
        match self {
            Row::Server(s) if s.gate == Some(Gate::Off) => Switch::OffAlways,
            Row::Server(s) => {
                let mut live = s.tools.iter().filter(|t| t.registered.is_some());
                if live.clone().next().is_some() && live.all(|t| t.hidden) {
                    Switch::OffSession
                } else {
                    Switch::On
                }
            }
            Row::Tool(_, t) if t.gate == Some(Gate::Off) => Switch::OffAlways,
            Row::Tool(_, t) if t.hidden => Switch::OffSession,
            _ => Switch::On,
        }
    }

    /// Registered names a session switch covers (a clashing tool registers as
    /// `<server>__<name>`, so this only ever touches this server's own slots).
    fn tool_names(&self) -> Vec<String> {
        match self {
            Row::Server(s) => s
                .tools
                .iter()
                .filter_map(|t| t.registered.clone())
                .collect(),
            Row::Tool(_, t) => t.registered.clone().into_iter().collect(),
        }
    }

    fn target(&self) -> Target {
        match self {
            Row::Server(s) => Target {
                server: s.name.clone(),
                tool: None,
            },
            Row::Tool(s, t) => Target {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
            },
        }
    }

    /// The row's own risk line.
    fn gate(&self) -> Option<Gate> {
        match self {
            Row::Server(s) => s.gate,
            Row::Tool(_, t) => t.gate,
        }
    }
}

/// One line of the `/acp` menu.
#[derive(Debug)]
pub enum AcpRow<'a> {
    /// A configured harness (`agents.yaml`).
    Entry(&'a EntryInfo),
    /// Blank line between the two halves; the cursor skips it.
    Sep,
    /// A registry agent, addable with Enter.
    Registry(&'a RegistryAgent),
}

/// What the key handler should do after a menu saw a key.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// Nothing more.
    Consumed,
    /// Replace the input text.
    Input(String),
}

/// How full the context window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Under 75 %.
    Normal,
    /// 75–90 %: context filling up.
    Warn,
    /// 90 % and above: start a new session or summarise.
    Danger,
}

/// `12.3k`, `200k`, `1.0M`.
pub fn k(n: u32) -> String {
    let k = f64::from(n) / 1000.0;
    if n < 1000 {
        n.to_string()
    } else if k < 100.0 {
        format!("{k:.1}k")
    } else if k < 1000.0 {
        format!("{k:.0}k")
    } else {
        format!("{:.1}M", k / 1000.0)
    }
}

/// Lowercase yaml name of a policy action.
pub fn action_name(a: PolicyAction) -> &'static str {
    match a {
        PolicyAction::Allow => "allow",
        PolicyAction::Ask => "ask",
        PolicyAction::Deny => "deny",
    }
}

/// Lowercase yaml name of a `mcp/config.yaml` line.
pub fn gate_name(g: Gate) -> &'static str {
    match g {
        Gate::ReadOnly => "read_only",
        Gate::Mutating => "mutating",
        Gate::Destructive => "destructive",
        Gate::Off => "off",
    }
}

/// Space in `/mcp`: annotations → read_only → mutating → destructive → annotations (12 §5).
/// `off` is not a risk; ⏎ switches it.
fn next_gate(g: Option<Gate>) -> Option<Gate> {
    match g {
        None => Some(Gate::ReadOnly),
        Some(Gate::ReadOnly) => Some(Gate::Mutating),
        Some(Gate::Mutating) => Some(Gate::Destructive),
        Some(Gate::Destructive | Gate::Off) => None,
    }
}

/// What `Model::window` hands the renderer; see there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First entry built.
    pub from: usize,
    /// One past the last entry built.
    pub to: usize,
    /// Pinned: rendered lines of `from` to drop.
    pub skip: usize,
    /// Anchored: rows of `from` above the fold.
    pub top: usize,
}

/// Three Escs inside this window stop the running subtree as well as the root (133 §5).
pub const ESC_TAPS_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// Body rows the open todo card shows at most (47 §6.3).
pub const TODO_ROWS: usize = 6;
/// Columns of a todo row's status column: `[in progress] `.
const TODO_LABEL: usize = 14;

/// What the fixed todo card draws (47 §6.3): the trigger row, and while
/// open the body rows in view with an overflow indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoCard {
    /// `> Todos · 2/5 done · 1 blocked   Now: …  [F2]`; `v` while open.
    pub header: String,
    /// Wrapped body rows in view (`Model::todo_scroll` applied); empty collapsed.
    /// The status colours an item's rows; `None` is a blocker's reason.
    pub rows: Vec<(Option<TodoStatus>, String)>,
    /// `↕ 3–8 / 14 · ↑↓ PgUp PgDn` when the body does not fit; `None` otherwise.
    pub overflow: Option<String>,
    /// The card has the keys.
    pub focused: bool,
}

impl TodoCard {
    /// Terminal rows the card takes: border, trigger, body, overflow row.
    pub fn height(&self) -> usize {
        3 + self.rows.len() + usize::from(self.overflow.is_some())
    }
}

/// The bracketed label of a todo row.
pub fn status_label(s: TodoStatus) -> &'static str {
    match s {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in progress",
        TodoStatus::Done => "done",
        TodoStatus::Dropped => "dropped",
        TodoStatus::Blocked => "blocked",
    }
}

/// Greedy word wrap at `width` chars (`0` = one line); a word longer than
/// the width is split. Never empty: an empty text is one empty row.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let n = word.chars().count();
        if width > 0 && len > 0 && len + 1 + n > width {
            lines.push(std::mem::take(&mut cur));
            len = 0;
        } else if len > 0 {
            cur.push(' ');
            len += 1;
        }
        for c in word.chars() {
            if width > 0 && len >= width {
                lines.push(std::mem::take(&mut cur));
                len = 0;
            }
            cur.push(c);
            len += 1;
        }
    }
    lines.push(cur);
    lines
}

/// UI state.
#[derive(Debug, Default)]
pub struct Model {
    /// Current session (the one the human started, resumed or opened).
    pub session: Option<SessionId>,
    /// Peer name when the current session is a subagent (`Loaded.name`).
    pub session_name: Option<String>,
    /// Parent when the current session is a subagent (`Loaded.parent`): where `/parent` goes.
    pub session_parent: Option<SessionId>,
    /// The current session's subtree, seeded by `list_agents` on open and
    /// kept live by every event of a session in it (design doc 19 §3.3).
    pub agents: HashMap<SessionId, AgentRow>,
    /// `/agents` was typed; the next `UiEvent::Agents` opens the picker
    /// (seeding replies on open only fill the map).
    agents_picker_pending: bool,
    /// Multi-step `/provider add`; `None` when the input is a normal line.
    pub prompt: Option<Prompt>,
    /// A `Send { session: None }` is in flight; adopt the next `TurnStarted`.
    awaiting_session: bool,
    /// A routing failure of that send (`Error { turn: None }`) that arrived
    /// before `SessionCreated` named the session: shown once it does, if the
    /// ids agree; another session's error is dropped.
    send_error: Option<(SessionId, String)>,
    /// `/mcp` was typed; the next `UiEvent::Mcp` opens the menu. Refreshes
    /// after `SetGate` only update an open menu, never reopen a closed one.
    awaiting_mcp: bool,
    /// `/acp` was typed; the next `UiEvent::Acp` opens the menu (same rule as `awaiting_mcp`).
    awaiting_acp: bool,
    /// Transcript of the shown session.
    pub entries: Vec<Entry>,
    /// The current session has a turn in flight.
    pub streaming: bool,
    /// Animation frame; advances while something is in flight.
    pub frame: usize,
    /// Usage of the last finished turn of the shown session.
    pub usage: Option<Usage>,
    /// Model of the shown session (`TurnStarted.model`; `default_model` before that).
    /// For an ACP child this is the harness name (`omp`).
    pub model_id: String,
    /// The engine's default for new sessions (`Info.default_model`, then
    /// `DefaultModelChanged`, 82 §3): what the header shows after `/new`.
    pub default_model: String,
    /// The engine's starred models (`Info.favorite_models`, then
    /// `FavoriteModelsChanged` / the `set_model_favorite` reply, 127 §3), in
    /// starring order; `/model` lists them first.
    pub favorite_models: Vec<String>,
    /// The LLM an ACP child's harness reports (06 §8.2); the header shows
    /// `model_id · harness_model`. `None` for native sessions.
    pub harness_model: Option<String>,
    /// The ACP harness's manifest of loaded skills (`skill:<name>`) and
    /// commands (75 §3); empty for native sessions.
    pub harness_commands: Vec<agent::HarnessCommand>,
    /// Context window the harness reported (75 §5); the header prefers it
    /// over the catalog limit. `None` for native sessions.
    pub context_size: Option<u32>,
    /// `Loaded.agent`: the shown session runs on an external harness (75 §8);
    /// `/model` and `/effort` then pick from the harness's own options.
    pub agent: Option<String>,
    /// Reasoning effort of the shown session (`default_effort` before the first turn);
    /// `None` = provider default.
    pub effort: Option<String>,
    /// The engine's default for new sessions (`Info.default_effort`, then
    /// `DefaultEffortChanged`, 83 §3): what the header shows after `/new`.
    pub default_effort: Option<String>,
    /// A `SetEffort` is in flight; its `EffortChanged` echo is not announced twice.
    awaiting_effort: bool,
    /// Effort levels per model for the `/effort` picker; `None` in the line printer.
    pub catalog: Option<Arc<Catalog>>,
    /// Tokens the shown session's window held after its last turn.
    pub context: u32,
    /// `context` is the engine's estimate (after a compaction or a load), shown
    /// as `≈` until the provider's next usage replaces it (70 COMPACT-SIZE).
    pub context_estimated: bool,
    /// Open menu.
    pub menu: Option<Menu>,
    /// Path autocomplete under the editor (PATH-AC).
    pub path: PathState,
    /// Pending approvals for the current session or its subtree, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// Pending `ask_user` calls for the current session or its subtree, oldest first; the front is shown.
    pub asks: VecDeque<Ask>,
    /// The double's prediction per pending card (`question_id` as text), from `DoublePredicted`;
    /// dropped when the card resolves.
    pub double: HashMap<String, DoublePredicted>,
    /// `tools.approvals` (global); false = never prompts.
    pub approvals_global: bool,
    /// The root session opted out of prompts (`/approvals off`).
    pub approvals_off: bool,
    /// `Info.app_enabled`: the app plugin runs, so `/apps` is offered. Off,
    /// app records stay readable under `/artifacts` (46 §9).
    pub app_enabled: bool,
    /// The harness-wide vision fallback (52 §3): a text-only shown model
    /// says at attach time whether this model will inspect the image.
    /// Asked once at startup; `/vision` replies keep it current.
    pub vision_model: Option<ModelInfo>,
    /// `/vision` was typed: the next reply prints its line (the startup
    /// fetch is silent).
    awaiting_vision: bool,
    /// Latest MCP reconcile snapshot; `None` before the first.
    pub mcp: Option<McpProgress>,
    /// A reconcile settled while the `/mcp` menu is open: the event loop
    /// re-lists so the rows show the servers that just (dis)connected.
    pub refresh_mcp: bool,
    /// Large pastes and attached images held out of the input box; the box
    /// shows one [`paste_token`] each, expanded back on submit.
    pub pastes: Vec<Paste>,
    /// Text of the last input submitted here, pastes expanded: the next
    /// `TurnStarted` echoing it is not shown again (its card is already up,
    /// with paste tokens).
    sent: Option<String>,
    /// Steers of the shown session waiting for the current tool batch to
    /// end, oldest first; drawn below the transcript until `InputInjected`.
    pub queued: Vec<QueuedInput>,
    /// Ids of the shown session's running background jobs (`JobEvent`).
    pub jobs: BTreeSet<String>,
    /// Turn the last `InputInjected` joined: its `TurnStarted` is not echoed
    /// (the card moved into `entries` on injection).
    last_injected_turn: Option<TurnId>,
    /// Text the editor takes on the next render: a withdrawn steer, or the
    /// opener `/edit` puts back.
    pub recall: Option<String>,
    /// Opener seq of the shown session's current or last turn (`TurnStarted.turn_seq`).
    pub turn_seq: Option<i64>,
    /// `/edit` in progress: `(entry index, turn seq)` of the message in the
    /// editor; the next plain line replaces that turn (29 §6.2).
    pub editing: Option<(usize, i64)>,
    /// Notice shown after the transcript reloads that an undo/redo triggered.
    notice: Option<String>,
    /// The shown session's summariser call is in flight (`Compacting`).
    pub compacting: bool,
    /// 109 §10.3: the status-line note after a restart (`resumed · 2
    /// replayed · 1 skipped`); cleared by the next `TurnFinished`.
    pub resume_note: Option<String>,
    /// The shown session's harvest is in flight (`Harvesting` … `Harvested`).
    /// Detached from the turn (10 §5.2 as built): survives `TurnFinished`,
    /// shown only while nothing else is.
    pub harvesting: bool,
    /// The shown session's vision consultation step in flight
    /// (`VisionProgress`, 52 §6): the status-line text until it settles.
    pub vision: Option<String>,
    /// The user asked to leave.
    pub quit: bool,
    /// Skill index (`list_skills`), fetched at start and by `/skills`: names
    /// resolve `/<skill> [args]`.
    pub skills: Vec<SkillMeta>,
    /// `/skills` was typed: the next index reply opens the picker.
    skills_picker_pending: bool,
    /// Skills whose body is in the shown session's frame (`SkillsChanged`,
    /// `Loaded.skills`): the status-line badge.
    pub loaded_skills: Vec<String>,
    /// First visible transcript row while scrolled up: `(entry, rows into
    /// it)`. `None` follows the tail. Anchored to an entry rather than to a
    /// row count from the bottom so what you are reading stays put while a
    /// reply streams in below.
    pub anchor: Option<(usize, usize)>,
    /// Terminal columns, set by the fullscreen UI before events apply; `0`
    /// (tests, line printer) = unbounded. Assistant text renders at this
    /// width when constructed; resize does not re-render (design doc 30 §7).
    pub width: usize,
    /// Latest `link_status` (badge and `/link` menu); `None` until fetched.
    pub link: Option<link::LinkStatus>,
    /// Latest [`update::UpdateReady`]; header tag and one transcript line (39 §4.1).
    pub update: Option<update::UpdateReady>,
    /// `/link` was typed (or a flow that reopens it ran); the next
    /// `UiEvent::Link` opens the menu (same rule as `awaiting_mcp`).
    awaiting_link: bool,
    /// A link event arrived that the local copy cannot fold in: the event
    /// loop re-asks `link_status`.
    pub refresh_link: bool,
    /// The live `/link pair` card, redrawn each second by [`Model::tick`].
    pairing: Option<PairingCard>,
    /// Latest `list_nodes` (badge and `/nodes` menu); `None` until fetched.
    pub nodes: Option<Vec<nodes::Node>>,
    /// `/nodes` was typed (or a flow that reopens it ran); the next
    /// `UiEvent::Nodes` opens the menu (same rule as `awaiting_link`).
    awaiting_nodes: bool,
    /// A node event arrived: the event loop re-asks `list_nodes`.
    pub refresh_nodes: bool,
    /// A loaded transcript holds `show` cards: the event loop asks
    /// `list_artifacts` to join their artifacts back (35 §0.3).
    pub refresh_artifacts: bool,
    /// The live `/nodes` invite card, redrawn each second by [`Model::tick`].
    invite: Option<PairingCard>,
    /// Latest `list_secrets` (the `/secrets` menu, the `replaces vN` hint); `None` until fetched.
    pub secrets: Option<secrets::SecretsList>,
    /// `/secrets` was typed; the next `UiEvent::Secrets` opens the menu (same rule as `awaiting_link`).
    awaiting_secrets: bool,
    /// The vault changed while a list is held: the event loop re-asks `list_secrets`.
    pub refresh_secrets: bool,
    /// Pending `secret_request`s for the current session or its subtree, oldest first; the front is shown.
    pub secret_requests: VecDeque<SecretRequest>,
    /// Context revision of the shown transcript (`Loaded.context_revision`,
    /// 48 §3.1); empty until loaded. A `ContextChanged` with a newer one
    /// sets `refresh_context`; an older settled reload is dropped.
    pub context_revision: String,
    /// The shown session's context takes `/shake` (`Loaded.context_editable`).
    pub context_editable: bool,
    /// Why it does not, when it does not (`Loaded.context_reason`).
    pub context_reason: Option<String>,
    /// The transcript is behind a context edit or rewind and a settled
    /// reload was refused (the session was busy): retry at the next
    /// `ContextChanged { reason: "settled" }`.
    pub context_stale: bool,
    /// A context edit or rewind landed (here or in another client): the
    /// event loop re-asks `load_session(settled_only: true)`.
    pub refresh_context: bool,
    /// Status-line suffix while attached to another process's engine (63 §0.3).
    pub attached: Option<String>,
    /// The engine behind the calls changed: the event loop reloads the shown
    /// session and its subtree from the new one (63 §2.3).
    pub reselect: bool,
    /// Latest plan of the shown session (`Loaded.todos`, `TodoChanged`,
    /// `get_todos`; 47 §5.1). `None` until a load carried one.
    pub todos: Option<TodoSnapshot>,
    /// The plan on screen may be behind: the last load carried no snapshot.
    /// Labelled as such; never drawn as an empty plan (47 §5.4).
    pub todos_stale: bool,
    /// Disclosure choice per session, kept across switches within this
    /// client and across incoming snapshots (47 §6.1). Absent = collapsed.
    todo_open: HashMap<SessionId, bool>,
    /// The todo card has the keys (F2): ↑/↓/PgUp/PgDn scroll it, esc
    /// collapses, tab hands the editor back. Modals still win; see [`Model::todo_focused`].
    pub todo_focus: bool,
    /// First wrapped body row shown while expanded.
    pub todo_scroll: usize,
    /// Latest `get_communication` of the shown session (the global default
    /// without one): the header tag and the `/communication` lines (50 §10).
    /// `None` until fetched; never guessed.
    pub communication: Option<CommunicationState>,
    /// `/communication` (or a switch) was typed: the next state reply prints its lines.
    awaiting_communication: bool,
    /// The switches, a scope or the network changed: the event loop re-asks
    /// `get_communication` for the shown session.
    pub refresh_communication: bool,
    /// The `/peers` menu is open and its inputs changed: the event loop
    /// re-asks `list_peers` and `list_peer_messages`.
    pub refresh_peers: bool,
    /// Bodies `PeerDelivered` already carded: the legacy `MessageDelivered`
    /// for the same input (50 §10) is not shown twice.
    peer_shown: VecDeque<String>,
    /// `/search` state (49 §6.1): query, chips, request identity, groups,
    /// cursor, previews. Kept across session switches so chips survive and
    /// **Back to results** restores the last view; `menu == Menu::Search`
    /// says whether it is up.
    pub search: Box<search::State>,
    /// The **Earlier messages** inspection (49 §7): the transcript shows a
    /// page ending at an old match, isolated from the session's live events.
    pub inspect: Option<search::Inspect>,
    /// Search requests a reply produced where no sender was at hand (the
    /// refresh reply's re-ask); flushed by the next tick or key.
    search_outbox: Vec<search::Request>,
    /// `/cost` was typed; the next `UiEvent::Cost` opens the menu (same rule as `awaiting_mcp`).
    awaiting_cost: bool,
    /// Latest `session_cost` of the shown session (the header tag, `/cost`,
    /// the `/agents` child totals; 51 §8.3). `None` until fetched; never summed here.
    pub cost: Option<SessionCost>,
    /// The ledger of the shown session (or an ancestor) changed: the event
    /// loop re-asks `session_cost`.
    pub refresh_cost: bool,
    /// The shown session's advisor (125 §7.5): the status segment, the
    /// header tag, the picker's `●`. `state: Off` until a reply or event says otherwise.
    pub advisor: AdvisorStatus,
    /// `Info.advisor` (125 §6.5): the last remembered pick, served by `/advisor on`;
    /// follows every `AdvisorChanged` with a model.
    pub advisor_default: AdvisorDefaults,
    /// Where the next `UiEvent::Models` picker sends its pick.
    pick_target: PickTarget,
    /// What the next `UiEvent::AdvisorStatus` reply is for; `None` installs it silently.
    advisor_reply: Option<AdvisorReply>,
    /// Seqs of the Advisory rows `AdvisoryAdded` carded: a `TurnStarted`
    /// opened by one is not echoed as `◆ you`.
    advisory_seqs: BTreeSet<i64>,
    /// A transcript was loaded: the event loop re-asks `advisor_status`.
    pub refresh_advisor: bool,
    /// Effective project of the shown session's root (`Loaded.project`).
    pub project: Option<String>,
    /// A `/sessions` or `/artifacts` menu is open and session metadata
    /// changed: the event loop re-lists it.
    pub refresh_grouping: bool,
    /// The private root this view owns (51 §4): its id, lease secret and
    /// the tree members seen so far. Leaving the tree releases it.
    pub private: Option<Private>,
    /// The retention warning was shown once this run.
    private_warned: bool,
    /// Bumped whenever a terminal observer opens: replies carry the epoch
    /// they were asked under and older ones are dropped.
    terminal_epoch: u64,
    /// The dictation in progress (54 §6); the editor is read-only while `Some`.
    pub dictation: Option<Dictating>,
    /// `before`/`after` snapshot taken at Ctrl+D, waiting for `dictation_start`'s reply.
    dictation_pending: Option<(String, String)>,
    /// Streaming STT model state (`DictationModel` events); Ctrl+D needs `Ready`.
    pub dictation_model: voice::ModelState,
    /// `(value, caret)` the editor must take this frame: a dictation stopped or was discarded.
    pub pending_input: Option<(String, usize)>,
    /// The talk this view started (76 §0.12); `Some` whether or not the view is open.
    pub talk: Option<Talking>,
    /// The `/talk` pane covers the transcript; Esc closes it, the talk keeps running.
    pub talk_view: bool,
    /// `/talk` sent, waiting for `talk_start`'s reply.
    talk_pending: bool,
    /// A `/attach` pick this frame: the token the editor prepends to its text.
    pub attach_token: Option<String>,
    /// When Esc last cancelled (133 §5): three within [`ESC_TAPS_WINDOW`] stop the subtree too.
    esc_taps: Vec<std::time::Instant>,
}

impl Model {
    /// Fresh state showing `default_model` in the header.
    pub fn new(default_model: String, approvals: bool) -> Self {
        Self {
            model_id: default_model.clone(),
            default_model,
            approvals_global: approvals,
            ..Self::default()
        }
    }

    /// `Self::new` on the engine's default model and effort, keeping catalog and skill index: `/new` and `/resume`.
    fn reset(&mut self) {
        *self = Self {
            effort: self.default_effort.clone(),
            default_effort: self.default_effort.take(),
            catalog: self.catalog.take(),
            skills: std::mem::take(&mut self.skills),
            width: self.width,
            secrets: self.secrets.take(),
            link: self.link.take(),
            nodes: self.nodes.take(),
            update: self.update.take(),
            todo_open: std::mem::take(&mut self.todo_open),
            // Per session: the switch view belongs to whatever is shown next
            // (the global default after `/new`).
            refresh_communication: true,
            app_enabled: self.app_enabled,
            advisor_default: std::mem::take(&mut self.advisor_default),
            vision_model: self.vision_model.take(),
            search: std::mem::take(&mut self.search),
            private: self.private.take(),
            private_warned: self.private_warned,
            terminal_epoch: self.terminal_epoch,
            dictation: self.dictation.take(),
            dictation_pending: self.dictation_pending.take(),
            dictation_model: self.dictation_model.clone(),
            attached: self.attached.take(),
            ..Self::new(
                std::mem::take(&mut self.default_model),
                self.approvals_global,
            )
        };
    }

    /// Show a loaded transcript: `UiEvent::History` and `UiEvent::Forked`.
    fn load(&mut self, loaded: Loaded) {
        let opts = self.assistant_options();
        self.entries = loaded
            .history
            .into_iter()
            .flat_map(|h| from_history(h, &opts))
            .collect();
        self.refresh_artifacts = self
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Tool { name, .. } if name == SHOW || name == APP));
        self.anchor = None;
        // An index into the old transcript is meaningless now.
        self.editing = None;
        // The reload an undo/redo triggered shows its own notice
        // instead of the generic depth line.
        match self.notice.take() {
            Some(n) => self.entries.push(Entry::Info(n)),
            None if loaded.undone > 0 => self.entries.push(Entry::Info(format!(
                "{} turn(s) undone — /redo restores",
                loaded.undone
            ))),
            None => {}
        }
        self.queued = loaded.queued;
        self.model_id = loaded.model;
        self.harness_model = loaded.harness_model;
        self.harness_commands = loaded.harness_commands;
        self.context_size = loaded.context_size;
        self.agent = loaded.agent;
        self.effort = loaded.effort;
        self.streaming = loaded.running;
        self.session_name = loaded.name;
        self.session_parent = loaded.parent;
        self.loaded_skills = loaded.skills;
        self.context_revision = loaded.context_revision;
        self.context_editable = loaded.context_editable;
        self.context_reason = loaded.context_reason;
        self.context_stale = false;
        self.project = loaded.project;
        // The header's cost tag is the owner's summary, re-read on every load (51 §6.2).
        self.refresh_cost = true;
        // The link survives a restart (125 §3): re-read it with the transcript.
        self.refresh_advisor = true;
        if let Some(n) = loaded.context_estimate {
            self.context = n;
            self.context_estimated = true;
        }
        self.loaded_todos(loaded.todos);
        // The load carries the switches too; a `get_communication` reply
        // already in flight is reconciled by revision.
        if let Some(c) = loaded.communication {
            self.set_communication(c);
        }
    }

    /// `Loaded.todos` (47 §5.4): a snapshot applies by version; none means the
    /// read failed — the plan on screen stays, labelled stale, never emptied.
    fn loaded_todos(&mut self, todos: Option<TodoSnapshot>) {
        match todos {
            Some(s) => self.set_todos(s),
            None => self.todos_stale = true,
        }
    }

    /// Install a snapshot of the shown session: only a higher version
    /// replaces; an equal one is idempotent and, being authoritative, clears
    /// the stale mark; an older one (a slow `load_session` racing a
    /// `TodoChanged`) is dropped (47 §5.4). A newer *empty* plan is an
    /// explicit clear: the card hides whatever the disclosure choice was, and
    /// gives the editor back if it had the keys (47 §6.1).
    pub fn set_todos(&mut self, s: TodoSnapshot) {
        match &self.todos {
            Some(cur) if s.version < cur.version => return,
            Some(cur) if s.version == cur.version => {}
            _ => {
                if s.items.is_empty() {
                    if let Some(id) = self.session {
                        self.todo_open.insert(id, false);
                    }
                    self.todo_focus = false;
                }
                self.todos = Some(s);
            }
        }
        self.todos_stale = false;
    }

    /// A settled reload (`UiEvent::Refreshed`, 48 §5): shown only when the
    /// actor read it idle, nothing started streaming here meanwhile, and its
    /// revision is not behind the one on screen. Otherwise the transcript
    /// stays as it is, marked stale until the next settled notification;
    /// a pending notice (our own removal) lands anyway.
    fn refresh(&mut self, mut loaded: Loaded) {
        let (theirs, ours) = (
            revision(&loaded.context_revision),
            revision(&self.context_revision),
        );
        // Our own removal reloads once through `RemoveContext` and once more
        // when its `ContextChanged` arrives first: the second is the same picture.
        if loaded.settled
            && theirs == ours
            && ours != 0
            && !self.context_stale
            && self.notice.is_none()
        {
            self.loaded_todos(loaded.todos.take());
            return;
        }
        if !loaded.settled || self.streaming || theirs < ours {
            self.context_stale = true;
            self.context_editable = false;
            if let Some(n) = self.notice.take() {
                self.entries.push(Entry::Info(n));
            }
            if !loaded.settled
                && let Some(reason) = loaded.context_reason
            {
                self.entries.push(Entry::Info(format!(
                    "{reason} — transcript refreshes when it settles"
                )));
            }
            self.loaded_todos(loaded.todos.take());
            return;
        }
        // A picker over the old inventory would be refused as stale anyway.
        if matches!(self.menu, Some(Menu::Shake(_))) {
            self.menu = None;
            self.entries.push(Entry::Info(
                "context changed — /shake again to review".into(),
            ));
        }
        self.load(loaded);
    }

    /// Markdown options for assistant text: the terminal width less the
    /// card padding, `surface::fence` for `mermaid`/`json-render` fences.
    fn assistant_options(&self) -> markdown::Options {
        markdown::Options {
            width: if self.width == 0 {
                0
            } else {
                self.width.saturating_sub(2).max(20)
            },
            fence: Some(FENCE.clone()),
        }
    }

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The ask currently shown, if any.
    pub fn ask(&self) -> Option<&Ask> {
        self.asks.front()
    }

    /// The double's prediction for the shown approval (first) or ask, when it has one.
    fn shown_prediction(&self) -> Option<&DoublePredicted> {
        let id = match (self.approval(), self.ask()) {
            (Some(a), _) => a.id.0,
            (None, Some(a)) => a.req.id.0,
            (None, None) => return None,
        };
        self.double.get(&id.to_string())
    }

    /// The double's chip for the shown approval or ask, when it has predicted it.
    pub fn double_chip(&self) -> Option<String> {
        self.shown_prediction().map(|p| double_chip(p, now_ms()))
    }

    /// The shown card is in an L2 grace: frames are needed for the countdown.
    fn double_counting_down(&self) -> bool {
        self.shown_prediction().is_some_and(|p| p.will_act_at.is_some())
    }

    /// The `secret_request` currently shown, if any.
    pub fn secret_request(&self) -> Option<&SecretRequest> {
        self.secret_requests.front()
    }

    /// The input box takes keys: no approval prompt, no modal menu (the
    /// slash popup rides on the box) and the todo card not focused (F2). A
    /// shown `secret_request` types into the box (masked).
    pub fn input_focused(&self) -> bool {
        if self.approval().is_some() || self.dictation.is_some() || self.talk_view {
            return false;
        }
        if self.secret_request().is_some() {
            return true;
        }
        if let Some(a) = self.ask() {
            return a.typing;
        }
        !self.todo_focus && !self.modal_menu() && self.inspect.is_none()
    }

    /// A menu that takes the keys is open: every menu but the slash popup.
    pub fn modal_menu(&self) -> bool {
        !matches!(self.menu, None | Some(Menu::Slash { .. }))
    }

    /// The `/search` menu is up (49 §6).
    pub fn search_open(&self) -> bool {
        matches!(self.menu, Some(Menu::Search | Menu::Attach))
    }

    /// The `/attach` menu is up (66 §10): Enter attaches instead of opening.
    pub fn attach_open(&self) -> bool {
        matches!(self.menu, Some(Menu::Attach))
    }

    /// The todo card has the keys: F2 focused it and nothing modal is up.
    /// Approvals, secret entry, `ask_user` and menus keep their own keys;
    /// their answers are never consumed here (47 §6.3).
    pub fn todo_focused(&self) -> bool {
        self.todo_focus
            && self.approval().is_none()
            && self.secret_request().is_none()
            && self.ask().is_none()
            && !self.modal_menu()
    }

    /// The shown session's disclosure choice; collapsed until F2 (47 §6.1).
    pub fn todo_expanded(&self) -> bool {
        self.session
            .is_some_and(|s| self.todo_open.get(&s).copied().unwrap_or(false))
    }

    /// F2 and `/todos`: expand and focus a collapsed card, collapse an open
    /// one. The editor's draft is untouched. Opening re-reads `get_todos`
    /// so a plan written while this client was elsewhere shows (47 §6.3).
    pub fn toggle_todos(&mut self, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info("no session".into()));
            return;
        };
        let open = !self.todo_expanded();
        self.todo_open.insert(session, open);
        self.todo_focus = open;
        if open {
            send(tx, Command::GetTodos(session));
        }
    }

    /// A key while the card is focused ([`Model::todo_focused`]) at a
    /// `width`×`height` terminal: ↑/↓ and PgUp/PgDn scroll the body, esc
    /// collapses and hands the editor back (no turn is cancelled), tab
    /// hands it back with the card left open, ⏎ does nothing (the draft is
    /// never submitted from here). `true` when consumed; any other key
    /// hands the editor back and is *not* consumed, so typing lands in the
    /// draft instead of vanishing.
    pub fn todo_key(&mut self, code: KeyCode, width: usize, height: usize) -> bool {
        let total = self.todo_lines(width).len();
        let rows = total.min(self.todo_body_rows(height));
        let page = rows.max(1);
        // A resize may have left the scroll past the end; step from what is drawn.
        let at = self.todo_scroll.min(total - rows);
        self.todo_scroll = match code {
            KeyCode::Up => at.saturating_sub(1),
            KeyCode::Down => at + 1,
            KeyCode::PageUp => at.saturating_sub(page),
            KeyCode::PageDown => at + page,
            KeyCode::Esc => {
                if let Some(s) = self.session {
                    self.todo_open.insert(s, false);
                }
                self.todo_focus = false;
                at
            }
            KeyCode::Tab => {
                self.todo_focus = false;
                at
            }
            KeyCode::Enter => at,
            _ => {
                self.todo_focus = false;
                return false;
            }
        }
        .min(total - rows);
        true
    }

    /// The card is drawn: the plan has items, is stale or unavailable, or
    /// the user opened it. Only an authoritative empty plan hides it (47 §6.1).
    pub fn todo_shown(&self) -> bool {
        self.todo_expanded()
            || self.todos_stale
            || self.todos.as_ref().is_some_and(|t| !t.items.is_empty())
    }

    /// Body rows the open card may use: [`TODO_ROWS`], fewer on a short
    /// terminal so the trigger row, editor and some transcript stay; `0`
    /// leaves the trigger alone (47 §6.3).
    pub fn todo_body_rows(&self, height: usize) -> usize {
        TODO_ROWS.min(height.saturating_sub(14))
    }

    /// The open body at `width` columns (`0` = unbounded): `[status] text`
    /// per item, wrapped under the text column, a blocker's reason on its
    /// own rows. Nothing is cut; scrolling reaches every row (47 §6.1).
    pub fn todo_lines(&self, width: usize) -> Vec<(Option<TodoStatus>, String)> {
        let Some(t) = &self.todos else {
            return Vec::new();
        };
        // Border 2 + padding 2 + one spare column (see `menu`), then the label column.
        let text_width = match width {
            0 => 0,
            w => w.saturating_sub(5 + TODO_LABEL).max(10),
        };
        let pad = " ".repeat(TODO_LABEL);
        let mut out = Vec::new();
        for item in &t.items {
            let label = format!(
                "{:<TODO_LABEL$}",
                format!("[{}]", status_label(item.status))
            );
            for (i, line) in wrap(&item.text, text_width).into_iter().enumerate() {
                let lead = if i == 0 { &label } else { &pad };
                out.push((Some(item.status), format!("{lead}{line}")));
            }
            if let Some(reason) = &item.reason {
                for line in wrap(reason, text_width) {
                    out.push((None, format!("{pad}{line}")));
                }
            }
        }
        out
    }

    /// What the fixed todo card draws this frame; `None` while hidden.
    pub fn todo_card(&self, width: usize, height: usize) -> Option<TodoCard> {
        if !self.todo_shown() {
            return None;
        }
        let expanded = self.todo_expanded();
        let focused = self.todo_focused();
        let (glyph, hint) = if expanded { ("v", "") } else { (">", "  [F2]") };
        let external = match &self.todos {
            Some(t) if t.source == TodoSource::Acp => " (external plan)",
            _ => "",
        };
        let mut header = format!("{glyph} Todos{external} · {}", self.todo_summary());
        if !expanded {
            if let Some(now) = self.todo_now() {
                // Fit the row: border, padding and a spare column (see `menu`).
                let room = match width {
                    0 => usize::MAX,
                    w => w
                        .saturating_sub(5 + header.chars().count() + hint.len() + 3)
                        .max(8),
                };
                let now: String = if now.chars().count() > room {
                    now.chars().take(room - 1).chain(['…']).collect()
                } else {
                    now
                };
                header.push_str("   ");
                header.push_str(&now);
            }
            header.push_str(hint);
            return Some(TodoCard {
                header,
                rows: Vec::new(),
                overflow: None,
                focused,
            });
        }
        let lines = self.todo_lines(width);
        let total = lines.len();
        let rows = total.min(self.todo_body_rows(height));
        let from = self.todo_scroll.min(total - rows);
        let overflow = (total > rows).then(|| {
            if rows == 0 {
                format!("{total} rows need a taller terminal")
            } else {
                format!("↕ {}–{} / {total} · ↑↓ PgUp PgDn", from + 1, from + rows)
            }
        });
        Some(TodoCard {
            header,
            rows: lines.into_iter().skip(from).take(rows).collect(),
            overflow,
            focused,
        })
    }

    /// `2/5 done · 1 blocked`; a settled list with drops reads `3 done, 1
    /// dropped`, never `100%`; `none` for an empty plan, `unavailable`
    /// without one; ` · stale` while the shown plan may be behind (47 §6.1).
    fn todo_summary(&self) -> String {
        let Some(t) = &self.todos else {
            return "unavailable".into();
        };
        let count = |s: TodoStatus| t.items.iter().filter(|i| i.status == s).count();
        let (done, dropped, blocked) = (
            count(TodoStatus::Done),
            count(TodoStatus::Dropped),
            count(TodoStatus::Blocked),
        );
        let open = t.items.iter().any(|i| i.status.is_actionable());
        let mut s = match (t.items.len(), open, blocked, dropped) {
            (0, ..) => "none".to_owned(),
            (_, false, 0, 1..) => format!("{done} done, {dropped} dropped"),
            (n, ..) => format!("{done}/{n} done"),
        };
        if dropped > 0 && open {
            s.push_str(&format!(" · {dropped} dropped"));
        }
        if blocked > 0 {
            s.push_str(&format!(" · {blocked} blocked"));
        }
        if self.todos_stale {
            s.push_str(" · stale");
        }
        s
    }

    /// The collapsed row's task: `Now: <in progress>`, else the first
    /// pending item, else `Waiting: N blocked` for a blocked-only list;
    /// nothing when settled (47 §6.1).
    fn todo_now(&self) -> Option<String> {
        let t = self.todos.as_ref()?;
        let pick = |s: TodoStatus| t.items.iter().find(|i| i.status == s);
        if let Some(i) = pick(TodoStatus::InProgress).or_else(|| pick(TodoStatus::Pending)) {
            return Some(format!("Now: {}", i.text));
        }
        match t
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Blocked)
            .count()
        {
            0 => None,
            n => Some(format!("Waiting: {n} blocked")),
        }
    }

    /// The slice of the transcript worth building this frame. Every frame
    /// lays out and paints every child it is given, so building the whole
    /// history made each frame O(session); the screen shows `view` rows.
    ///
    /// Pinned (`anchor == None`): `[from, len)` drawn bottom-aligned, with
    /// `skip` leading rendered lines of `from` dropped when that one entry
    /// (a long reply) is taller than the window. Anchored: `[from, to)` drawn
    /// from the top with the first `top` rows of `from` above the fold.
    pub fn window(&self, view: usize, width: usize) -> Window {
        let budget = view * 2;
        if let Some((i, r)) = self.anchor.filter(|(i, _)| *i < self.entries.len()) {
            let mut total = self.entries[i].height(width).saturating_sub(r);
            let mut to = i + 1;
            while total < budget && to < self.entries.len() {
                total += self.entries[to].height(width);
                to += 1;
            }
            return Window {
                from: i,
                to,
                skip: 0,
                top: r,
            };
        }
        let (from, skip) = self.tail(budget, width);
        Window {
            from,
            to: self.entries.len(),
            skip,
            top: 0,
        }
    }

    /// The pinned window's start: the first entry whose tail reaches `lines`
    /// rows at `width`, and how many rendered markdown lines of it to skip
    /// when that entry alone is taller than the window.
    fn tail(&self, lines: usize, width: usize) -> (usize, usize) {
        let mut total = 0;
        for (i, e) in self.entries.iter().enumerate().rev() {
            let h = e.height(width);
            if total + h < lines {
                total += h;
                continue;
            }
            // Only a rendered reply is skippable line by line; leave a full
            // budget of its own rows, the plain tail always shows.
            let Entry::Assistant(t) = e else {
                return (i, 0);
            };
            let v = t.view();
            let mut budget = (lines - total).saturating_sub(text_rows(v.plain, width));
            let rendered: Vec<&markdown::Line> = v.lines().collect();
            let mut skip = rendered.len();
            for l in rendered.iter().rev() {
                if budget == 0 {
                    break;
                }
                budget = budget.saturating_sub(line_rows(l, width));
                skip -= 1;
            }
            return (i, skip);
        }
        (0, 0)
    }

    /// Move the transcript by `delta` rows (negative = up) in a `view`-row
    /// window at `width`. Heights are the same estimates `window` draws by.
    /// Reaching the bottom re-pins; the top stops at the first entry.
    pub fn scroll(&mut self, delta: isize, view: usize, width: usize) {
        let h = |i: usize| self.entries[i].height(width);
        let len = self.entries.len();
        let (mut i, mut r) = match self.anchor.filter(|(i, _)| *i < len) {
            Some(a) => a,
            None => {
                // The pinned window's first row: `view` rows up from the end.
                let mut left = view;
                let mut i = len;
                loop {
                    if i == 0 {
                        return; // shorter than the window: nothing to scroll
                    }
                    i -= 1;
                    let hi = h(i);
                    if hi > left {
                        break (i, hi - left);
                    }
                    left -= hi;
                }
            }
        };
        if delta < 0 {
            let mut d = delta.unsigned_abs();
            loop {
                if r >= d {
                    r -= d;
                    break;
                }
                d -= r;
                if i == 0 {
                    r = 0;
                    break;
                }
                i -= 1;
                r = h(i);
            }
        } else {
            r += delta.unsigned_abs();
            while r >= h(i) {
                r -= h(i);
                i += 1;
                if i == len {
                    self.anchor = None;
                    return;
                }
            }
        }
        // Less than a window left below the anchor: follow the tail again.
        let mut below = h(i) - r;
        let mut j = i + 1;
        while below <= view && j < len {
            below += h(j);
            j += 1;
        }
        self.anchor = (below > view).then_some((i, r));
    }

    /// Take a paste: small ones come back verbatim for the input box; large
    /// ones are held in `pastes` and a [`paste_token`] comes back instead.
    /// Terminals paste newlines as `\r`; normalise to `\n`.
    pub fn paste(&mut self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.lines().count() <= 5 && text.chars().count() <= 400 {
            return text;
        }
        let i = self.pastes.len();
        self.pastes.push(Paste::Text(text));
        paste_token(i, &self.pastes[i])
    }

    /// Hold an image for the next submit; returns its [`paste_token`]. A
    /// text-only shown model gets a notice so the user can `/model` before
    /// Enter (17 §4.4).
    pub fn attach(&mut self, img: Image) -> String {
        let text_only = self
            .catalog
            .as_ref()
            .and_then(|c| c.closest(&self.model_id))
            .is_some_and(|m| !m.image);
        if text_only {
            let tail = match &self.vision_model {
                Some(v) => format!(
                    "{} will inspect the image for it and answer its question (/vision to change)",
                    v.id
                ),
                None => "the image will be sent as omitted — /model to switch or /vision <model> to set a fallback".to_owned(),
            };
            self.entries.push(Entry::Info(format!(
                "{} is text-only; {tail}",
                self.model_id
            )));
        }
        let p = Paste::Image(img);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }

    /// `line` with every paste token the user left in place replaced by its
    /// text, every image token removed (with one adjacent space) and the
    /// images' paths returned in token order, every attach token removed and
    /// its marker line put first (66 §10); `pastes` is emptied either way. A
    /// token the user deleted drops its paste.
    fn expand_pastes(&mut self, line: &str) -> (String, Vec<PathBuf>) {
        let mut text = line.to_owned();
        let mut files = Vec::new();
        let mut markers = String::new();
        for (i, p) in std::mem::take(&mut self.pastes).into_iter().enumerate() {
            let token = paste_token(i, &p);
            match p {
                Paste::Text(t) => text = text.replace(&token, &t),
                Paste::Image(_) | Paste::Ref(_) => {
                    let Some(at) = text.find(&token) else {
                        continue;
                    };
                    let end = at + token.len();
                    let range = if text[end..].starts_with(' ') {
                        at..end + 1
                    } else if text[..at].ends_with(' ') {
                        at - 1..end
                    } else {
                        at..end
                    };
                    text.replace_range(range, "");
                    match p {
                        Paste::Image(img) => files.push(img.path),
                        Paste::Ref(r) => {
                            markers.push_str(&agent::attach::marker(&r));
                            markers.push('\n');
                        }
                        Paste::Text(_) => {}
                    }
                }
            }
        }
        if !markers.is_empty() {
            markers.push_str(text.trim_start());
            text = markers;
        }
        (text, files)
    }

    /// Add an entry that did not come from the model's stream (a peer message,
    /// a spawn notice). It goes above a still-streaming tail so the text keeps
    /// accumulating in one `markdown::Text`: pushing it last would split the
    /// response mid-block and render both halves as broken markdown.
    fn interject(&mut self, e: Entry) {
        let streaming = match self.entries.last() {
            Some(Entry::Assistant(t)) => !t.is_done(),
            Some(Entry::Reasoning { done, .. }) => !done,
            _ => false,
        };
        let at = self.entries.len() - usize::from(streaming);
        self.entries.insert(at, e);
    }

    /// Expand or collapse the most recent card or finished tool call (Ctrl+O).
    /// An artifact card expands to its file rendered by kind (35 §6.1).
    pub fn toggle_card(&mut self) {
        let opts = self.assistant_options();
        let Some(e) = self.entries.iter_mut().rev().find(|e| {
            matches!(
                e,
                Entry::Card { .. }
                    | Entry::Tool {
                        result: Some(_),
                        ..
                    }
                    | Entry::Compacted {
                        summary: Some(_),
                        ..
                    }
            ) || matches!(e, Entry::Advisory { advisory, .. } if advisory.notes.len() > ADVISORY_COLLAPSED)
        }) else {
            return;
        };
        match e {
            Entry::Card { expanded, .. }
            | Entry::Compacted { expanded, .. }
            | Entry::Advisory { expanded, .. } => {
                *expanded = !*expanded;
            }
            Entry::Tool {
                result: Some((_, false)),
                artifact: Some(a),
                expanded,
                ..
            } => {
                *expanded = match expanded.take() {
                    Some(_) => None,
                    None => Some(Box::new(artifact_body(a, &opts))),
                }
            }
            Entry::Tool {
                result: Some((output, _)),
                expanded,
                ..
            } => {
                *expanded = match expanded.take() {
                    Some(_) => None,
                    None => Some(Box::new(markdown::Text::complete(&output_markdown(output)))),
                }
            }
            _ => {}
        }
    }

    /// The path `o` opens: the newest artifact card of the current exchange
    /// (nothing later than the user's last message), if any.
    pub fn artifact_path(&self) -> Option<&std::path::Path> {
        self.entries
            .iter()
            .rev()
            .take_while(|e| !matches!(e, Entry::User { .. }))
            .find_map(|e| match e {
                Entry::Tool {
                    artifact: Some(a), ..
                } => Some(a.path.as_path()),
                _ => None,
            })
    }

    /// Join an artifact to its `show` card by `call_id` (35 §0.3); a card
    /// not on this transcript (another session's, or scrolled out of
    /// history) is left alone.
    fn attach_artifact(&mut self, a: artifact::Artifact) {
        if let Some(Entry::Tool { artifact, .. }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { call_id, .. } if *call_id == a.call_id))
        {
            *artifact = Some(Box::new(a));
        }
    }

    /// The live block of job `id`, if a card on screen or off has one.
    fn job_mut(&mut self, id: &str) -> Option<&mut JobTail> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool { job: Some(j), .. } if j.id == id => Some(j),
            _ => None,
        })
    }

    /// Some assistant text is still typing out.
    pub fn revealing(&self) -> bool {
        self.entries
            .iter()
            .rev()
            .any(|e| matches!(e, Entry::Assistant(t) if t.unrevealed() > 0))
    }

    /// Frames are needed: text is typing out, a turn is in flight, MCP
    /// servers are connecting, a compaction runs, or a subagent is live (the
    /// board's spinner and elapsed column).
    pub fn animating(&self) -> bool {
        self.revealing()
            || self.streaming
            || self.mcp_connecting()
            || self.compacting
            || self.harvesting
            || self.vision.is_some()
            || self.advisor.state == AdvisorState::Reviewing
            || self.agents.values().any(AgentRow::live)
            || self.pairing.is_some()
            || self.double_counting_down()
            || self.invite.is_some()
            || self.dictation.is_some()
            || self.talk_view
    }

    /// The board (design doc 19 §3.3): the subtree's live rows plus errors
    /// and cancels the picker has not shown yet, in picker order. Empty =
    /// hidden.
    pub fn board(&self) -> Vec<&AgentRow> {
        let mut rows: Vec<&AgentRow> = self.agents.values().filter(|r| r.on_board()).collect();
        rows.sort_by_key(|r| rank(r));
        rows
    }

    fn mcp_connecting(&self) -> bool {
        self.mcp.as_ref().is_some_and(|p| !p.connecting.is_empty())
    }

    /// Bottom-right status: MCP servers connecting (with a spinner and the
    /// first names) or, once settled, `mcp 11/12`; empty with no servers.
    pub fn mcp_status(&self) -> String {
        let Some(p) = &self.mcp else {
            return String::new();
        };
        let total = p.connecting.len() + p.connected.len() + p.failed.len();
        if total == 0 {
            return String::new();
        }
        let up = format!("mcp {}/{total}", p.connected.len());
        if p.connecting.is_empty() {
            return match p.failed.len() {
                0 => up,
                n => format!("{up}  ·  {n} failed"),
            };
        }
        let names = p
            .connecting
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = match p.connecting.len() {
            n if n > 3 => format!(" (+{})", n - 3),
            _ => String::new(),
        };
        format!("{} {up}  ·  connecting {names}{more}", self.spinner())
    }

    /// What the shown session is doing right now, for the status line and
    /// the trailing activity row; `None` when idle. A background harvest is
    /// named only when the session is otherwise idle.
    pub fn activity(&self) -> Option<String> {
        if self.compacting {
            return Some("compacting…".into());
        }
        if let Some(v) = &self.vision {
            return Some(v.clone());
        }
        if !self.streaming {
            return self.harvesting.then(|| "harvesting…".into());
        }
        // Parallel calls: a running card need not be the last entry.
        let busy = self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool {
                name,
                result: None,
                scrape,
                ..
            } => Some(scrape.clone().unwrap_or_else(|| format!("running {name}…"))),
            Entry::Pending {
                name,
                progress: Some(p),
                ..
            } => Some(progress_line(name, p)),
            Entry::Pending { name, .. } => Some(format!("preparing {name}…")),
            _ => None,
        });
        if busy.is_some() {
            return busy;
        }
        Some(match self.entries.last() {
            Some(Entry::Assistant(t)) if !t.is_done() => "streaming…".into(),
            Some(Entry::Reasoning { done: false, .. }) => "reasoning…".into(),
            _ => "thinking…".into(),
        })
    }

    /// The transcript's trailing activity row: only while waiting on the
    /// model with nothing else showing progress in place.
    pub fn waiting(&self) -> bool {
        self.activity().is_some_and(|a| a == "thinking…")
    }

    /// Current spinner glyph.
    pub fn spinner(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(self.frame / 5) % FRAMES.len()]
    }

    /// One animation frame: advance the spinner and reveal a slice of the
    /// typewriter backlog proportional to its size, so the cursor trails the
    /// provider by a fixed number of frames (~a quarter second at 60 Hz)
    /// however fast it streams, and settles smoothly when it stops.
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        for e in self.entries.iter_mut().rev() {
            if let Entry::Assistant(t) = e {
                let backlog = t.unrevealed();
                if backlog == 0 {
                    continue;
                }
                t.reveal((backlog / 16).max(1));
            }
        }
        tick_card(&mut self.entries, &mut self.pairing, CardKind::Link);
        tick_card(&mut self.entries, &mut self.invite, CardKind::Node);
    }

    /// Replace the body of the newest `/link pair` card, if any.
    fn set_link_card(&mut self, body: &str) {
        set_card(&mut self.entries, CardKind::Link, body);
    }

    /// Peer name of a subagent in the current session's subtree; `None`
    /// for the session itself or strangers.
    pub fn peer_name(&self, id: SessionId) -> Option<&str> {
        self.agents.get(&id).map(|a| a.info.name.as_str())
    }

    /// A pending approval was answered (here or elsewhere): its row runs again.
    fn approval_done(&mut self, session: SessionId) {
        if let Some(row) = self.agents.get_mut(&session)
            && matches!(row.info.state, AgentState::Approval { .. })
        {
            row.info.state = AgentState::Running;
            row.tool = None;
        }
    }

    /// Fold one event into the state.
    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Agent(ev) => self.apply_agent(ev),
            UiEvent::Approval(req) => {
                if let Some(row) = self.agents.get_mut(&req.session) {
                    row.info.state = AgentState::Approval {
                        tool: req.tool.clone(),
                    };
                    row.tool = Some(format!(
                        "{} {}",
                        req.tool,
                        describe(&req.tool, &req.args).join(" ")
                    ));
                }
                if self.session == Some(req.session) || self.agents.contains_key(&req.session) {
                    self.approvals.push_back(req);
                }
            }
            UiEvent::Resolved(r) => {
                if let Some(req) = self.approvals.iter().find(|a| a.id == r.id) {
                    let session = req.session;
                    self.approval_done(session);
                }
                self.approvals.retain(|a| a.id != r.id);
                self.double.remove(&r.id.0.to_string());
            }
            UiEvent::Ask(req) => {
                if !req.questions.is_empty()
                    && (self.session == Some(req.session) || self.agents.contains_key(&req.session))
                {
                    self.asks.push_back(Ask::new(req));
                }
            }
            UiEvent::AskResolved(r) => {
                self.asks.retain(|a| a.req.id != r.id);
                self.double.remove(&r.id.0.to_string());
            }
            UiEvent::DoublePredicted(p) => {
                self.double.insert(p.question_id.clone(), p);
            }
            UiEvent::DoubleAnswered(a) => {
                self.double.remove(&a.question_id);
                if a.session.is_none_or(|s| {
                    self.session == Some(s) || self.agents.contains_key(&s)
                }) {
                    self.entries.push(Entry::Info(double::chip::answered(&a)));
                }
            }
            UiEvent::Sessions(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("no sessions".into()));
                } else {
                    let items = session_rows(list);
                    // A refresh of an open menu keeps the highlight where it was.
                    let cursor = match &self.menu {
                        Some(Menu::Sessions { cursor, .. }) => *cursor,
                        _ => items
                            .iter()
                            .position(|r| r.item().is_some_and(|(s, _)| Some(s.id) == self.session))
                            .unwrap_or_else(|| grouped_first(&items)),
                    };
                    self.menu = Some(Menu::Sessions { items, cursor });
                }
            }
            UiEvent::Search(generation, reply) => self.search_reply(generation, reply),
            UiEvent::KnowledgeChanged(_) => {
                self.search_invalidate(SearchCategory::Knowledge);
                self.search_invalidate(SearchCategory::KnowledgeHistory);
            }
            UiEvent::Agents(list) => {
                // Reseeding keeps what events taught about rows still there.
                let mut old = std::mem::take(&mut self.agents);
                self.agents = list
                    .into_iter()
                    // 125 §7.5: the hidden advisor child is never a board row.
                    .filter(|info| info.kind != SessionKind::Advisor)
                    .map(|info| {
                        let row = match old.remove(&info.id) {
                            Some(mut row) => {
                                row.info = info;
                                row
                            }
                            None => AgentRow::new(info),
                        };
                        (row.info.id, row)
                    })
                    .collect();
                // The owned tree's roster: navigating within it is not a leave (51 §4.3).
                if let Some(p) = &mut self.private
                    && self.session.is_some_and(|s| p.member(s))
                {
                    p.members.extend(self.agents.keys().copied());
                }
                self.approvals
                    .retain(|a| Some(a.session) == self.session || self.agents.contains_key(&a.session));
                if !std::mem::take(&mut self.agents_picker_pending) {
                    return;
                }
                // The picker shows errors and cancels; the board lets go of them.
                for row in self.agents.values_mut() {
                    row.fresh = false;
                }
                if self.agents.is_empty() {
                    self.entries.push(Entry::Info("no subagents".into()));
                } else {
                    self.menu = Some(Menu::Agents {
                        items: sort_agents(self.agents.values().cloned().collect()),
                        cursor: 0,
                    });
                }
            }
            UiEvent::History(id, loaded) => {
                if self.session == Some(id) {
                    self.load(loaded);
                }
            }
            UiEvent::Refreshed(id, loaded) => {
                // An inspected page is not the live transcript: no reload stitches over it.
                if self.session == Some(id) && self.inspect.is_none() {
                    self.refresh(loaded);
                }
            }
            UiEvent::ContextItems {
                session,
                category,
                snapshot,
            } => self.open_shake(session, category, snapshot),
            UiEvent::Activity(list) => {
                self.menu = Some(Menu::Broadcast(Box::new(Broadcast::pick(list))));
            }
            UiEvent::Broadcasted(r) => self.broadcasted(r),
            // A failed read leaves the plan on screen, labelled stale (47 §5.4).
            UiEvent::Todos(id, snapshot) => {
                if self.session == Some(id) {
                    self.loaded_todos(snapshot);
                }
            }
            // The transcript reload that follows pushes the notice (`Refreshed`).
            UiEvent::ContextRemoved(id, r) => {
                if self.session == Some(id) {
                    self.notice = Some(removal_line(&r));
                }
            }
            UiEvent::Forked(id, loaded) => {
                self.reset();
                self.session = Some(id);
                self.notice = Some("forked — /rename to tell them apart".into());
                self.load(loaded);
            }
            UiEvent::SessionCreated(id) => {
                self.awaiting_session = false;
                if self.session.is_none() {
                    self.session = Some(id);
                    // The global default was shown; now this session's own view.
                    self.refresh_communication = true;
                    self.refresh_cost = true;
                }
                if let Some((s, message)) = self.send_error.take()
                    && s == id
                {
                    self.entries.push(Entry::Error(message));
                }
            }
            UiEvent::Models(items) => {
                let target = std::mem::take(&mut self.pick_target);
                if items.is_empty() {
                    self.entries
                        .push(Entry::Error("no models: no provider is configured".into()));
                } else {
                    // One provider only: skip the providers level (124 §3.5).
                    let provider = match providers(&items).as_slice() {
                        [(p, _)] => Some(p.clone()),
                        _ => None,
                    };
                    self.menu = Some(Menu::Models {
                        items,
                        filter: String::new(),
                        cursor: 0,
                        provider,
                        target,
                    });
                    self.models_home();
                }
            }
            UiEvent::PathComplete(generation, reply) => self.path.reply(generation, reply),
            // Chained picker: a model with effort levels asks for one right away (Esc skips).
            UiEvent::ModelSet(info) => {
                self.model_id = info.id;
                if !info.efforts.is_empty() {
                    self.open_efforts(info.efforts, PickTarget::Session);
                }
            }
            UiEvent::FavoriteModels(models) => self.set_favorites(models),
            UiEvent::EffortSet(effort) => {
                self.awaiting_effort = false;
                self.entries.push(Entry::Info(match &effort {
                    Some(e) => format!("effort: {e}"),
                    None => "effort cleared".to_owned(),
                }));
                self.effort = effort;
            }
            // The reply of `/advisor …` or a load's refresh (125 §7.1): installed
            // silently; `status` prints its lines; a pick chains into the effort picker.
            UiEvent::AdvisorStatus(status) => {
                if status.session != self.session.unwrap_or(status.session) {
                    return;
                }
                let why = self.advisor_reply.take();
                self.set_advisor_status(status);
                match why {
                    Some(AdvisorReply::Status) => {
                        for line in advisor_status_lines(&self.advisor) {
                            self.entries.push(Entry::Info(line));
                        }
                    }
                    Some(AdvisorReply::Pick) => {
                        let levels = self
                            .advisor
                            .model
                            .as_deref()
                            .zip(self.catalog.as_ref())
                            .and_then(|(id, c)| c.closest(id))
                            .map(|m| m.efforts())
                            .unwrap_or_default();
                        if !levels.is_empty() {
                            self.open_efforts(levels, PickTarget::Advisor);
                        }
                    }
                    None => {}
                }
            }
            UiEvent::HarnessOptions(options) => {
                for line in harness_options_lines(&options) {
                    self.entries.push(Entry::Info(line));
                }
            }
            UiEvent::HarnessOptionSet(key, value, r) => {
                self.entries
                    .push(Entry::Info(harness_option_set_line(&key, &value, &r)));
            }
            // The cursor is clamped where it is used (`mcp_key`, renderer).
            UiEvent::Mcp(list) => match &mut self.menu {
                Some(Menu::Mcp { servers, .. }) => *servers = list,
                _ if self.awaiting_mcp => {
                    self.awaiting_mcp = false;
                    self.menu = Some(Menu::Mcp {
                        servers: list,
                        cursor: 0,
                        expanded: BTreeSet::new(),
                        pending: None,
                    });
                }
                _ => {}
            },
            UiEvent::Acp(list) => match &mut self.menu {
                Some(Menu::Acp { entries, .. }) => *entries = list,
                _ if self.awaiting_acp => {
                    self.awaiting_acp = false;
                    self.menu = Some(Menu::Acp {
                        entries: list,
                        registry: Vec::new(),
                        cursor: 0,
                    });
                }
                _ => {}
            },
            UiEvent::AcpRegistry(list) => {
                if let Some(Menu::Acp { registry, .. }) = &mut self.menu {
                    *registry = list;
                }
            }
            UiEvent::AcpAdded(e) => self.entries.push(Entry::Info(format!(
                "acp: added `{}` ({} {})",
                e.name,
                e.command,
                e.args.join(" ")
            ))),
            UiEvent::Approvals(s) => {
                self.approvals_global = s.global;
                self.entries.push(Entry::Info(format!(
                    "approvals: {} (this session), {} (default)",
                    on_off(s.session && !self.approvals_off),
                    on_off(s.global)
                )));
            }
            UiEvent::Communication(s) => {
                // A reply for the session shown when it was asked, not now.
                if s.session != self.session {
                    return;
                }
                let print = std::mem::take(&mut self.awaiting_communication);
                if self.set_communication(s) && print {
                    let lines = communication_lines(self.communication.as_ref().expect("just set"));
                    self.entries.extend(lines.into_iter().map(Entry::Info));
                }
            }
            UiEvent::Peers(session, page) => {
                if let Some(Menu::Peers {
                    session: s, peers, ..
                }) = &mut self.menu
                    && *s == session
                {
                    *peers = Some(page);
                }
            }
            UiEvent::PeerRequests(session, page) => {
                if let Some(Menu::Peers {
                    session: s,
                    requests,
                    ..
                }) = &mut self.menu
                    && *s == session
                {
                    *requests = Some(page);
                }
            }
            UiEvent::PeerAnswered(r) => self.entries.push(Entry::Info(format!(
                "peers: {} answered → {}{}{}",
                short_uuid(r.reply_to.unwrap_or(r.id)),
                r.state.as_str(),
                r.answered_by
                    .as_deref()
                    .map_or_else(String::new, |by| format!(" · by {by}")),
                r.error
                    .as_deref()
                    .map_or_else(String::new, |e| format!(" · {e}")),
            ))),
            UiEvent::A2a(status) => self
                .entries
                .extend(a2a_lines(&status).into_iter().map(Entry::Info)),
            // Any subtree may be an ancestor of ours: the state is one cheap
            // call, so every change reloads it (50 §10). The open picker's
            // directory follows the switches too.
            UiEvent::CommunicationChanged(_) | UiEvent::A2aChanged(_) => {
                self.refresh_communication = true;
                self.refresh_peers |= matches!(self.menu, Some(Menu::Peers { .. }));
            }
            UiEvent::PeerMessageChanged(ev) => {
                self.refresh_peers |=
                    matches!(&self.menu, Some(Menu::Peers { session, .. }) if *session == ev.session);
            }
            UiEvent::PeerDelivered(ev) => {
                if self.session == Some(ev.session) {
                    self.remember_peer(&ev.text);
                    self.interject(peer_card(&ev.origin, ev.text));
                }
            }
            UiEvent::McpProgress(p) => {
                // Any change while the menu is open (a login settled, a server
                // (dis)connected) re-lists; waiting for `connecting` to drain
                // left a finished login unshown while a slow server reconnected.
                if self.mcp.as_ref() != Some(&p) && matches!(self.menu, Some(Menu::Mcp { .. })) {
                    self.refresh_mcp = true;
                }
                self.mcp = Some(p);
            }
            // Verbatim: the point is to see exactly what the model sees.
            UiEvent::Prompt(text) => self.entries.push(card(
                CardKind::Prompt,
                "prompt".into(),
                &format!("```\n{text}\n```"),
            )),
            UiEvent::Skills(items) => {
                self.skills = items.clone();
                if !std::mem::take(&mut self.skills_picker_pending) {
                    return;
                }
                if items.is_empty() {
                    self.entries.push(Entry::Info("no skills".into()));
                } else {
                    self.menu = Some(Menu::Skills { items, cursor: 0 });
                }
            }
            UiEvent::Logins(items) => {
                if items.is_empty() {
                    self.entries
                        .push(Entry::Info("nothing to log in to".into()));
                } else {
                    self.menu = Some(Menu::Logins { items, cursor: 0 });
                }
            }
            UiEvent::LoginFinished(f) => self.entries.push(match f.error {
                None => Entry::Info(format!("login: {} ok", f.provider)),
                Some(e) => Entry::Error(format!("login: {} failed: {e}", f.provider)),
            }),
            UiEvent::Skill(s) => {
                let text = if s.files.is_empty() {
                    s.body
                } else {
                    format!("{}\n\n---\nfiles: {}", s.body, s.files.join(", "))
                };
                self.entries.push(card(CardKind::Skill, s.name, &text));
            }
            UiEvent::SkillsAdded(items) => self.entries.push(Entry::Info(if items.is_empty() {
                "cloned; no skills found".into()
            } else {
                format!(
                    "added {} skills: {}",
                    items.len(),
                    items
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })),
            UiEvent::Synced(results) => {
                if results.is_empty() {
                    self.entries.push(Entry::Info("no repos".into()));
                }
                for r in results {
                    self.entries
                        .push(Entry::Info(format!("{}  {}", r.name, r.status)));
                }
            }
            UiEvent::McpAdded(s) => self.entries.push(Entry::Info(format!(
                "{}  {} tools  {}",
                s.name,
                s.tools.len(),
                if s.connected {
                    "connected"
                } else {
                    "not connected"
                }
            ))),
            UiEvent::RepoAdded(r) => self.entries.push(Entry::Info(format!(
                "cloned {} → {}  (build it, then `/mcp add {{...\"cwd\": ...}}`)",
                r.name, r.path
            ))),
            // Always the picker: `H` reaches the bundle's history (and so
            // deleted concepts) even when nothing is left to list.
            UiEvent::Knowledge(items) => self.menu = Some(Menu::Knowledge { items, cursor: 0 }),
            // Fills the menu opened by the request it answers; anything else
            // (closed with esc, another scope or page since) is dropped.
            UiEvent::KnowledgeHistory {
                id,
                limit,
                history,
            } => {
                if let Some(Menu::KnowledgeHistory {
                    id: open,
                    changes: shown,
                    limit: page,
                    more,
                    loading,
                    error,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && *page == limit
                {
                    let mut changes = history.changes;
                    *more = changes.len() > limit;
                    changes.truncate(limit);
                    *shown = changes;
                    *loading = false;
                    *error = None;
                }
            }
            UiEvent::KnowledgeRevision {
                id,
                revision,
                card: true,
            } => {
                let title = format!(
                    "knowledge {}  {}{}",
                    revision.change.commit,
                    revision.change.message,
                    id.map_or(String::new(), |i| format!("  ({i})"))
                );
                let text = revision_rows(&revision).join("\n");
                self.entries.push(card(
                    CardKind::Skill,
                    title,
                    &format!("```diff\n{text}\n```"),
                ));
            }
            // Only the commit still awaited in the open scope.
            UiEvent::KnowledgeRevision {
                id,
                revision,
                card: false,
            } => {
                if let Some(Menu::KnowledgeHistory {
                    id: open,
                    pending,
                    revision: shown,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && pending.as_deref() == Some(revision.change.commit.as_str())
                {
                    *pending = None;
                    *shown = Some((revision, 0, 0));
                }
            }
            UiEvent::KnowledgeRestored(c) => self
                .entries
                .push(Entry::Info(format!("{}  {}", c.commit, c.message))),
            UiEvent::Harvested(r) => self.entries.push(Entry::Info(format!(
                "knowledge +{} ~{}",
                r.added, r.updated
            ))),
            UiEvent::Info(t) => self.entries.push(Entry::Info(t)),
            // The note rides the reload (like undo's), so it lands after the
            // transcript the new engine serves — a pushed entry would go with
            // the old one.
            UiEvent::Engine(attached) => {
                let note = match &attached {
                    Some(label) => label.clone(),
                    None => format!("node process started here (pid {})", std::process::id()),
                };
                self.attached = attached;
                match self.session {
                    Some(_) => {
                        self.notice = Some(note);
                        self.reselect = true;
                    }
                    None => self.entries.push(Entry::Info(note)),
                }
            }
            UiEvent::Jobs(jobs) => self.entries.push(Entry::Info(tools::jobs::table(&jobs))),
            UiEvent::Schedules(rows) => self.entries.push(Entry::Info(schedules_table(&rows))),
            UiEvent::Tasks(project, rows) => self
                .entries
                .push(Entry::Info(tasks_table(project.as_deref(), &rows))),
            UiEvent::Roster(rows) => self.entries.push(Entry::Info(agents_table(&rows))),
            UiEvent::ScheduleHistory(name, info, rows) => self
                .entries
                .push(Entry::Info(schedule_history_table(&name, info.as_ref(), &rows))),
            UiEvent::Computer(status) => self.entries.push(Entry::Info(computer_line(&status))),
            UiEvent::ArtifactShown(ev) => {
                self.search_invalidate(SearchCategory::Artifacts);
                if self.session == Some(ev.session) {
                    self.attach_artifact(ev.artifact);
                }
            }
            UiEvent::ScrapeProgress(ev) if self.session == Some(ev.session) => {
                if let Some(Entry::Tool { name, scrape, .. }) = self.entries.iter_mut().rev().find(
                    |e| matches!(e, Entry::Tool { call_id, result: None, .. } if *call_id == ev.call_id),
                ) {
                    *scrape = Some(scrape_line(name, &ev));
                }
            }
            UiEvent::ScrapeProgress(_) => {}
            UiEvent::Artifacts {
                items,
                pick,
                apps,
                sessions,
            } => {
                if !pick {
                    for a in items {
                        self.attach_artifact(a);
                    }
                } else if items.is_empty() {
                    self.entries.push(Entry::Info(
                        if apps { "no apps" } else { "no artifacts" }.into(),
                    ));
                } else {
                    let items = artifact_rows(items, |id| self.owner(&sessions, id));
                    let cursor = match &self.menu {
                        Some(Menu::Artifacts { cursor, .. }) => *cursor,
                        _ => grouped_first(&items),
                    };
                    self.menu = Some(Menu::Artifacts {
                        items,
                        cursor,
                        apps,
                    });
                }
            }
            UiEvent::Job(ev) => match ev {
                JobEvent::Started {
                    session,
                    id,
                    call_id,
                    ..
                } if self.session == Some(session) => {
                    if let Some(Entry::Tool { job, .. }) = self.entries.iter_mut().rev().find(
                        |e| matches!(e, Entry::Tool { call_id: c, .. } if *c == call_id),
                    ) {
                        *job = Some(JobTail::new(id.0.clone()));
                    }
                    self.jobs.insert(id.0);
                }
                JobEvent::Output { session, id, chunk } if self.session == Some(session) => {
                    if let Some(j) = self.job_mut(&id.0) {
                        j.push(&chunk);
                    }
                }
                JobEvent::Screen {
                    session, id, rows, ..
                } if self.session == Some(session) => {
                    if let Some(j) = self.job_mut(&id.0) {
                        j.set_screen(&rows);
                    }
                }
                JobEvent::Finished {
                    session,
                    id,
                    exit,
                    secs,
                    inline,
                    ..
                } if self.session == Some(session) => {
                    match self.entries.iter_mut().rev().find(
                        |e| matches!(e, Entry::Tool { job: Some(j), .. } if j.id == id.0),
                    ) {
                        // Within grace: the result carries the output.
                        Some(Entry::Tool { job, .. }) if inline => *job = None,
                        Some(Entry::Tool { job: Some(j), .. }) => {
                            j.exit = Some(format!("{exit} · {secs}s"));
                        }
                        _ => {}
                    }
                    self.jobs.remove(&id.0);
                }
                _ => {}
            },
            UiEvent::CompactStyle(s) => self.entries.push(Entry::Info(format!(
                "compact style: {s} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
            ))),
            UiEvent::TitleModel(m) => self.entries.push(Entry::Info(title_model_line(m.as_ref()))),
            // The startup fetch only primes the attach notice; `/vision` prints.
            UiEvent::VisionModel(m) => {
                if std::mem::take(&mut self.awaiting_vision) {
                    self.entries
                        .push(Entry::Info(vision_model_line(m.as_ref())));
                }
                self.vision_model = m;
            }
            // Status always; the picker when there is a choice to make or a
            // stale selection to clear (row 0 restores the default chain).
            UiEvent::AwsProfiles { info, pick } => {
                self.entries.push(Entry::Info(aws_profile_line(&info)));
                if pick && (info.profiles.len() > 1 || info.selected.is_some()) {
                    self.menu = Some(Menu::AwsProfiles { info, cursor: 0 });
                }
            }
            UiEvent::Providers(list) => {
                // 85 §0.9: the built-ins come first; a gateway list may be empty.
                for p in &list {
                    self.entries.push(Entry::Info(if p.builtin {
                        format!(
                            "{}  built-in  {} models  {}",
                            p.name,
                            p.models,
                            if p.configured {
                                "key set".to_owned()
                            } else {
                                format!("no key — /secrets add {}", p.name)
                            }
                        )
                    } else {
                        format!("{}  {}  {} models", p.name, p.base_url, p.models)
                    }));
                }
                if list.iter().all(|p| p.builtin) {
                    self.entries.push(Entry::Info(
                        "no custom providers; /provider add to create one".into(),
                    ));
                }
            }
            UiEvent::ProviderAdded(p) => self.entries.push(Entry::Info(format!(
                "provider {}  {}  {} models — /model {}/<id>",
                p.name, p.base_url, p.models, p.name
            ))),
            UiEvent::Withdrawn { id, text } => {
                self.queued.retain(|q| q.id != id);
                self.recall = Some(text);
            }
            UiEvent::Unsent { id, text } => {
                match id {
                    Some(id) => self.queued.retain(|q| q.id != id),
                    // The optimistic row from `submit_with`.
                    None => {
                        if let Some(i) = self
                            .entries
                            .iter()
                            .rposition(|e| matches!(e, Entry::User { turn: None, .. }))
                        {
                            self.entries.remove(i);
                        }
                        self.sent = None;
                        self.awaiting_session = false;
                    }
                }
                self.recall = Some(text);
            }
            // The transcript reload that follows pushes the notice (`History`).
            UiEvent::Undone(t) => self.notice = Some(format!("undone: {t} — /redo restores")),
            UiEvent::Redone(t) => self.notice = Some(format!("restored: {t}")),
            UiEvent::Link(status) => {
                let open = matches!(self.menu, Some(Menu::Link { .. }));
                if !open && !self.awaiting_link {
                    self.entries.push(Entry::Info(link_line(&status)));
                } else if !open {
                    self.menu = Some(Menu::Link { cursor: 0 });
                }
                self.awaiting_link = false;
                self.link = Some(status);
            }
            UiEvent::Pairing(p) => {
                // One pending pairing per harness: a new key kills the old card.
                if self.pairing.take().is_some() {
                    self.set_link_card("pairing expired");
                }
                let card = PairingCard::new(p);
                self.entries.push(Entry::Card {
                    kind: CardKind::Link,
                    title: "link pairing".into(),
                    text: Box::new(markdown::Text::complete(&card.body())),
                    expanded: true,
                });
                self.pairing = Some(card);
            }
            UiEvent::LinkEvent(ev) => self.apply_link(ev),
            UiEvent::UpdateReady(ev) => {
                // Once per version: the actor may re-emit the same fact.
                if self.update.as_ref() != Some(&ev) {
                    let text = if ev.applied {
                        format!(
                            "theuth {} is installed; it runs the next time you start theuth.",
                            ev.version
                        )
                    } else {
                        format!(
                            "theuth {} is available; `t --update` installs it (the binary's directory is not writable).",
                            ev.version
                        )
                    };
                    self.update = Some(ev);
                    self.interject(Entry::Info(text));
                }
            }
            UiEvent::Nodes(list) => {
                // Never a transcript line: every node event refetches, and a
                // presence flip is not worth a row. The verbs report themselves.
                if self.awaiting_nodes && !matches!(self.menu, Some(Menu::Nodes { .. })) {
                    self.menu = Some(Menu::Nodes { cursor: 0 });
                }
                self.awaiting_nodes = false;
                self.nodes = Some(list);
            }
            UiEvent::NodeInvite(name, p) => {
                // ponytail: one countdown at a time; an older invite card
                // freezes at its last second. Per-card slots if two are common.
                let card = PairingCard::new(p);
                self.entries.push(Entry::Card {
                    kind: CardKind::Node,
                    title: format!("node invite · {name}"),
                    text: Box::new(markdown::Text::complete(&card.body())),
                    expanded: true,
                });
                self.invite = Some(card);
            }
            UiEvent::MeshDevices(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("mesh devices: none paired".into()));
                }
                let now = now_ms();
                for d in list {
                    self.entries.push(Entry::Info(mesh_device_line(&d, now)));
                }
            }
            UiEvent::Sandboxes(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("sandboxes: none".into()));
                }
                for i in list {
                    self.entries.push(Entry::Info(sandbox_line(&i)));
                }
            }
            UiEvent::SandboxDesktop(name, d) => {
                self.entries.push(Entry::Info(format!(
                    "{name} desktop: {} (password in the vault as sandbox/{name}/vnc; `t sandbox desktop {name}` prints it)",
                    d.http_url
                )));
            }
            UiEvent::MeshDevicePairing(p) => {
                // The QR any engine may show; the hub accepts (69 decision 4). Shares the
                // node-invite countdown slot: one ticking invite card at a time.
                let card = PairingCard::new(link::Pairing {
                    key: p.string,
                    expires_at_ms: p.expires_ms,
                });
                self.entries.push(Entry::Card {
                    kind: CardKind::Node,
                    title: format!("mesh device pairing · pending {}", p.pending_id),
                    text: Box::new(markdown::Text::complete(&card.body())),
                    expanded: true,
                });
                self.invite = Some(card);
            }
            // Added, removed, online, revoked: all reshape the rows; re-ask.
            UiEvent::NodeEvent(_) => self.refresh_nodes = true,
            UiEvent::Secrets(list) => {
                if self.awaiting_secrets && !matches!(self.menu, Some(Menu::Secrets { .. })) {
                    self.menu = Some(Menu::Secrets {
                        cursor: 0,
                        history: None,
                    });
                }
                self.awaiting_secrets = false;
                self.secrets = Some(list);
            }
            UiEvent::SecretSet(info) => self
                .entries
                .push(Entry::Info(format!("{}: v{} set", info.name, info.current))),
            UiEvent::SecretsEvent(ev) => self.apply_secrets(ev),
            UiEvent::Error(e) => {
                // A failed `/compact` answers here, not with an agent event.
                self.compacting = false;
                self.awaiting_effort = false;
                self.awaiting_vision = false;
                self.awaiting_link = false;
                self.awaiting_secrets = false;
                self.awaiting_nodes = false;
                self.awaiting_cost = false;
                self.notice = None;
                // A knowledge request that was in flight failed: the menu
                // shows it where the rows would be (`r` retries).
                if let Some(Menu::KnowledgeHistory {
                    loading,
                    pending,
                    error,
                    ..
                }) = &mut self.menu
                    && (*loading || pending.is_some())
                {
                    *loading = false;
                    *pending = None;
                    *error = Some(e);
                    return;
                }
                self.entries.push(Entry::Error(e));
            }
            // The owner's summary, never a local sum (51 §6.2): a reply for
            // a session left behind is dropped.
            UiEvent::Cost(id, cost) => {
                if self.session != Some(id) {
                    return;
                }
                if let Some(Menu::Cost { cost: shown, .. }) = &mut self.menu {
                    *shown = cost.clone();
                } else if std::mem::take(&mut self.awaiting_cost) {
                    self.menu = Some(Menu::Cost {
                        cost: cost.clone(),
                        cursor: 0,
                    });
                }
                self.cost = Some(cost);
            }
            UiEvent::Terminals(items) => {
                if items.is_empty() {
                    self.entries
                        .push(Entry::Info("no terminals in this session's tree".into()));
                } else {
                    self.menu = Some(Menu::Terminals { items, cursor: 0 });
                }
            }
            UiEvent::PtyScreen {
                session,
                job,
                viewport,
                epoch,
                reply,
            } => {
                if let Some(Menu::Terminal(v)) = &mut self.menu {
                    v.reply(session, &job, viewport, epoch, reply);
                }
            }
            UiEvent::SessionOpened(open) => {
                self.reset();
                self.session = Some(open.session);
                self.refresh_communication = true;
                self.refresh_cost = true;
                if open.retention == Retention::Private
                    && let Some(secret) = open.lease_secret
                {
                    self.private = Some(Private {
                        root: open.root,
                        secret,
                        members: std::collections::HashSet::new(),
                    });
                    if !std::mem::replace(&mut self.private_warned, true) {
                        self.entries.push(Entry::Info(PRIVATE_WARNING.into()));
                    }
                    self.entries.push(Entry::Info(format!(
                        "private session {} — leaving it (/new, /sessions, /resume, /quit) closes the tree",
                        short(open.root)
                    )));
                } else {
                    self.entries.push(Entry::Info("new session".into()));
                }
            }
            UiEvent::SessionLeft(_, result) => {
                self.entries.push(Entry::Info(
                    match result.cleanup {
                        CleanupStatus::Complete => "private session closed",
                        CleanupStatus::Pending => "private session closed (cleanup pending)",
                        CleanupStatus::Incomplete => "private session closed (cleanup incomplete)",
                    }
                    .into(),
                ));
            }
            UiEvent::Voice(ev) => self.apply_voice(ev),
            UiEvent::Talk(ev) => self.apply_talk(ev),
            UiEvent::DictationStarting(id) => {
                // `/new` or a menu may have run between the key and the reply; an
                // empty anchor still tracks the dictation the daemon opened for us.
                let (before, after) = self.dictation_pending.take().unwrap_or_default();
                self.dictation = Some(Dictating::new(id, before, after));
            }
            UiEvent::DictationFailed(m) => {
                self.dictation_pending = None;
                self.dictation = None;
                self.entries.push(Entry::Error(format!("dictation: {m}")));
            }
            UiEvent::TalkStarting(id) => {
                self.talk_pending = false;
                // `SessionOpened` came first when `/talk` had to create the session.
                let Some(session) = self.session else { return };
                self.talk = Some(Talking::new(id, session));
                self.talk_view = true;
            }
            UiEvent::TalkFailed(m) => {
                self.talk_pending = false;
                self.entries.push(Entry::Error(format!("talk: {m}")));
            }
        }
    }

    /// Fold a voice bus event in (54 §4.4): the model state, and the five
    /// dictation events for the dictation this view started. Other ids
    /// (another frontend's dictation) and TTS events are ignored.
    fn apply_voice(&mut self, ev: voice::VoiceEvent) {
        use voice::VoiceEvent::*;
        match ev {
            DictationModel { state } => self.dictation_model = state,
            DictationStarted { id } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id)
                    && d.phase == Phase::Starting
                {
                    d.phase = Phase::Dictating;
                }
            }
            DictationLevel { id, levels } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id) {
                    d.push_levels(&levels);
                }
            }
            DictationText {
                id,
                utterance,
                text,
                is_final,
            } => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.id == id) {
                    d.set_utt(utterance as usize, text, is_final);
                }
            }
            DictationFinished { id, reason } => {
                let Some(d) = self.dictation.take_if(|d| d.id == id) else {
                    return;
                };
                // Esc already restored `before + after`.
                if !d.discard {
                    self.pending_input = Some(d.splice());
                }
                if let voice::FinishReason::Error { message } = reason {
                    self.entries
                        .push(Entry::Error(format!("dictation: {message}")));
                }
            }
            SpeechFinished { id, .. } => {
                if let Some(t) = &mut self.talk {
                    t.speech_finished(id);
                }
            }
            VoiceModel { .. } | SpeechStarted { .. } | SpeechAudio { .. } => {}
        }
    }

    /// Fold a talk bus event in (76 §5.2) for the talk this view started;
    /// `TalkFinished` closes the view and reports an error reason.
    fn apply_talk(&mut self, ev: talk::TalkEvent) {
        let Some(t) = &mut self.talk else { return };
        let Some(reason) = t.apply(ev, self.frame) else {
            return;
        };
        self.talk = None;
        self.talk_view = false;
        if let talk::EndReason::Error { message } = reason {
            self.entries.push(Entry::Error(format!("talk: {message}")));
        }
    }

    /// `/talk`: no talk → start one on the current session (created first
    /// when there is none) and open the view; one running with the view
    /// closed → re-enter; view open → stop and leave. Ctrl+T (`stop: true`)
    /// stops a running talk from anywhere. Waiting for the start reply: no-op.
    pub fn toggle_talk(&mut self, stop: bool) -> Option<Command> {
        if let Some(t) = &self.talk {
            if self.talk_view || stop {
                self.talk_view = false;
                return Some(Command::TalkStop(t.id));
            }
            self.talk_view = true;
            return None;
        }
        // Waiting for the start reply, or for the session a `Send` is creating
        // (`TalkStart(None)` would create a second one): no-op.
        if self.talk_pending || (self.session.is_none() && self.awaiting_session) {
            return None;
        }
        self.talk_pending = true;
        Some(Command::TalkStart(self.session))
    }

    /// Esc in the talk view (76 §5.2): while the agent is thinking, speaking
    /// or held, it is the agent's cancel — the talk stays on and returns to
    /// `Listening`; otherwise it leaves the view, the talk keeps running.
    pub fn talk_escape(&mut self) -> Option<Command> {
        match &self.talk {
            Some(t)
                if matches!(
                    t.state,
                    talk::State::Thinking | talk::State::Speaking | talk::State::Holding
                ) =>
            {
                Some(Command::Cancel(t.session))
            }
            _ => {
                self.talk_view = false;
                None
            }
        }
    }

    /// Ctrl+D / `/dictate` (54 §5.2). Idle with the model `Ready`: anchor the
    /// caret at `before | after` and start. Idle otherwise: say why, do
    /// nothing. Dictating: stop (keep the text). Starting or stopping: no-op.
    pub fn toggle_dictation(&mut self, before: String, after: String) -> Option<Command> {
        if let Some(d) = &mut self.dictation {
            if d.phase != Phase::Dictating {
                return None;
            }
            d.phase = Phase::Stopping;
            return Some(Command::DictationStop(d.id));
        }
        if self.dictation_pending.is_some() {
            return None;
        }
        let reason = match &self.dictation_model {
            voice::ModelState::Ready => {
                self.dictation_pending = Some((before, after));
                return Some(Command::DictationStart);
            }
            voice::ModelState::Loading => "dictation: model loading…".to_owned(),
            voice::ModelState::Missing => {
                "dictation: model missing (~/.theuth/models/nemotron-streaming-en or voice.dictation_model_dir)"
                    .to_owned()
            }
            voice::ModelState::Failed { message } => format!("dictation: {message}"),
        };
        self.entries.push(Entry::Info(reason));
        None
    }

    /// Esc while dictating: the editor takes `before + after` back now, the
    /// daemon is told to stop, and the later `Finished` splices nothing.
    pub fn discard_dictation(&mut self) -> Option<Command> {
        let d = self.dictation.as_mut()?;
        d.discard = true;
        d.phase = Phase::Stopping;
        self.pending_input = Some((format!("{}{}", d.before, d.after), d.before.len()));
        Some(Command::DictationStop(d.id))
    }

    /// Fold a secrets bus event in: a change re-lists a held list, a request
    /// from this session's tree queues its masked box, a resolution (answered
    /// elsewhere, declined, timed out) drops it.
    fn apply_secrets(&mut self, ev: secrets::SecretsEvent) {
        use secrets::SecretsEvent::*;
        match ev {
            Changed => self.refresh_secrets = self.secrets.is_some(),
            Requested {
                id,
                session,
                name,
                note,
                current,
                ..
            } => {
                if self.session == Some(session) || self.agents.contains_key(&session) {
                    self.secret_requests.push_back(SecretRequest {
                        id,
                        name,
                        note,
                        current,
                    });
                }
            }
            Resolved { id, timed_out, .. } => {
                if let Some(i) = self.secret_requests.iter().position(|r| r.id == id) {
                    let r = self.secret_requests.remove(i).expect("position");
                    if timed_out {
                        self.entries
                            .push(Entry::Info(format!("{}: request timed out", r.name)));
                    }
                }
            }
        }
    }

    /// Answer the shown `secret_request` (`None` declines) and dismiss it
    /// locally; its `Resolved` is then a no-op. The value is never shown.
    pub fn answer_secret(&mut self, value: Option<String>, tx: &UnboundedSender<Command>) {
        let Some(r) = self.secret_requests.pop_front() else {
            return;
        };
        self.entries.push(Entry::Info(format!(
            "{}: {}",
            r.name,
            if value.is_some() {
                "stored"
            } else {
                "declined"
            }
        )));
        send(tx, Command::AnswerSecret { id: r.id, value });
    }

    /// Fold a link bus event into the local status and the pairing card.
    /// What the copy cannot absorb (a device it has not seen) re-asks.
    fn apply_link(&mut self, ev: link::LinkEvent) {
        use link::LinkEvent::*;
        match ev {
            LinkRelay { connected } => match &mut self.link {
                Some(s) => s.connected = connected,
                None => self.refresh_link = true,
            },
            LinkPairing { key: None, .. } => {
                if self.pairing.take().is_some() {
                    self.set_link_card("pairing expired");
                }
                if let Some(s) = &mut self.link {
                    s.pairing = None;
                }
            }
            LinkPairing {
                key: Some(key),
                expires_at_ms,
            } => {
                if let Some(s) = &mut self.link {
                    s.pairing = Some(link::Pairing {
                        key,
                        expires_at_ms: expires_at_ms.unwrap_or(0),
                    });
                }
            }
            LinkPaired { device } => {
                if self.pairing.take().is_some() {
                    self.set_link_card(&format!("paired: {}", device.name));
                }
                match &mut self.link {
                    Some(s) => {
                        s.pairing = None;
                        s.devices.retain(|d| d.id != device.id);
                        s.devices.push(device);
                    }
                    None => self.refresh_link = true,
                }
            }
            LinkDevice { id, online } => {
                match self
                    .link
                    .as_mut()
                    .and_then(|s| s.devices.iter_mut().find(|d| d.id == id))
                {
                    Some(d) => {
                        d.online = online;
                        if !online {
                            d.last_seen_ms = now_ms();
                        }
                    }
                    None => self.refresh_link = true,
                }
            }
            LinkRevoked { id } => {
                if let Some(s) = &mut self.link {
                    s.devices.retain(|d| d.id != id);
                }
            }
            // 69: the mesh identity's uplink and its replicated registry; the
            // status snapshot carries both, so re-ask.
            MeshRelay { .. } | DevicePaired { .. } | DevicesChanged => self.refresh_link = true,
        }
    }

    fn apply_agent(&mut self, ev: AgentEvent) {
        if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
            self.session = ev.session();
            self.awaiting_session = false;
        }
        // The send was refused before any turn (a rejected attachment, no
        // actor): its error names a session we do not know yet, so it would
        // fall through the ownership check below. Held for `SessionCreated`.
        if self.awaiting_session
            && self.session.is_none()
            && let AgentEvent::Error {
                session,
                turn: None,
                message,
            } = &ev
        {
            self.send_error = Some((*session, message.clone()));
        }
        // The plan is session chrome, not a transcript row (47 §6.1): it
        // never finishes a streaming tail below. Children's plans show when
        // opened; a hidden session refreshes from `Loaded.todos` then.
        let ev = match ev {
            AgentEvent::TodoChanged { session, snapshot } => {
                if self.session == Some(session) {
                    self.set_todos(snapshot);
                }
                return;
            }
            // 82 §3: global; the draft header (no session open) follows it.
            AgentEvent::DefaultModelChanged { model } => {
                if self.session.is_none() {
                    self.model_id = model.clone();
                }
                self.default_model = model;
                return;
            }
            // 127 §3: global; an open `/model` picker re-renders from it.
            AgentEvent::FavoriteModelsChanged { models } => {
                self.set_favorites(models);
                return;
            }
            // 83 §3: global; the draft header (no session open) follows it.
            AgentEvent::DefaultEffortChanged { effort } => {
                if self.session.is_none() {
                    self.effort = effort.clone();
                }
                self.default_effort = effort;
                return;
            }
            ev => ev,
        };
        // Roster bookkeeping for the subtree, independent of what is shown.
        match &ev {
            // 125 §7.5: the advisor is session chrome (status segment, header
            // tag), never a transcript row — it must not finish a streaming
            // tail below. `set_advisor_status` prints the one change line.
            AgentEvent::AdvisorChanged { session, status } => {
                if self.session == Some(*session) {
                    self.set_advisor_status(status.clone());
                }
                return;
            }
            // 125 §7.4: the card goes above a streaming tail; a steer's row
            // may also open a turn, whose `TurnStarted` must not echo it.
            AgentEvent::AdvisoryAdded {
                session,
                seq,
                advisory,
            } => {
                if self.session == Some(*session) && self.advisory_seqs.insert(*seq) {
                    self.interject(Entry::Advisory {
                        advisory: advisory.clone(),
                        expanded: false,
                    });
                }
                return;
            }
            // 125 §7.5: the hidden advisor child never reaches the board (the
            // event carries no `kind`; the link's id is the guard).
            AgentEvent::SubagentSpawned { child, .. } if self.advisor.advisor == Some(*child) => {
                return;
            }
            AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
                cap,
            } => {
                let depth = if self.session == Some(*parent) {
                    Some(1)
                } else {
                    self.agents.get(parent).map(|p| p.info.depth + 1)
                };
                if let Some(depth) = depth {
                    self.agents.insert(
                        *child,
                        AgentRow::new(AgentInfo {
                            id: *child,
                            name: name.clone(),
                            parent: *parent,
                            depth,
                            agent: None,
                            model: String::new(),
                            state: AgentState::Running,
                            started_at: Some(chrono::Utc::now()),
                            finished_at: None,
                            last: String::new(),
                            project: None,
                            retention: agent::Retention::Durable,
                            activity: Default::default(),
                            cap: *cap,
                            kind: Default::default(),
                        }),
                    );
                }
                if let Some(p) = &mut self.private
                    && p.member(*parent)
                {
                    p.members.insert(*child);
                }
                if self.session == Some(*parent) {
                    self.interject(Entry::Info(format!("↳ spawned {name}")));
                }
                return;
            }
            AgentEvent::MessageDelivered {
                session,
                from,
                text,
            } => {
                if self.session == Some(*session) {
                    let kind = if from.starts_with("job ") {
                        CardKind::Job
                    } else {
                        CardKind::Peer
                    };
                    // The typed `PeerDelivered` already carded this body (50 §10).
                    if kind == CardKind::Peer
                        && let Some(at) = self.peer_shown.iter().position(|t| t == text)
                    {
                        self.peer_shown.remove(at);
                        return;
                    }
                    self.interject(card(kind, from.clone(), text));
                }
                return;
            }
            // Ownership moved (68 §4.2): the picker's chip follows; the open
            // session gets one line. Nothing streams, so finish nothing below.
            AgentEvent::SessionOwnerChanged {
                session,
                owner,
                epoch,
                here,
            } => {
                if let Some(Menu::Sessions { items, .. }) = &mut self.menu {
                    for row in items.iter_mut() {
                        if let Grouped::Item((s, _)) = row
                            && s.id == *session
                        {
                            s.owner = owner.clone();
                            s.epoch = *epoch;
                            s.here = *here;
                        }
                    }
                }
                if self.session == Some(*session) {
                    self.entries.push(Entry::Info(if *here {
                        format!("session continues here (epoch {epoch})")
                    } else {
                        format!(
                            "session now owned by {} (epoch {epoch})",
                            engine_chip(owner)
                        )
                    }));
                }
                return;
            }
            // Queue bookkeeping mid-stream must not finish the streaming tail below.
            AgentEvent::InputQueued {
                session, id, text, ..
            } => {
                if self.session == Some(*session) && !self.queued.iter().any(|q| q.id == *id) {
                    self.queued.push(QueuedInput {
                        id: *id,
                        text: text.clone(),
                    });
                }
                return;
            }
            AgentEvent::InputWithdrawn { session, id } => {
                if self.session == Some(*session) {
                    self.queued.retain(|q| q.id != *id);
                }
                return;
            }
            // The sidebar of another frontend; nothing on screen changes and
            // a mid-stream arrival must not finish the streaming tail below.
            AgentEvent::TitleChanged { .. } | AgentEvent::SessionDeleted { .. } => {
                self.search_invalidate(SearchCategory::Sessions);
                return;
            }
            // Another client (or our own removal, whose reload is already on
            // its way) changed what the model sees. An edit or rewind reloads
            // the transcript once settled; the settled handback after a turn
            // only clears a stale mark, the live stream already showed it.
            AgentEvent::ContextChanged {
                session,
                revision: rev,
                reason,
            } => {
                if self.session == Some(*session) {
                    let newer = revision(rev) > revision(&self.context_revision);
                    self.refresh_context |= (newer && reason != "settled") || self.context_stale;
                }
                return;
            }
            // Root metadata moved (51 §3): an open grouped picker re-lists;
            // the shown tree's label follows. Session chrome, never a transcript row.
            AgentEvent::SessionMetadataChanged { session, project } => {
                if self.session == Some(*session) || self.session_parent == Some(*session) {
                    self.project = project.clone();
                }
                self.refresh_grouping |= matches!(
                    self.menu,
                    Some(Menu::Sessions { .. } | Menu::Artifacts { .. })
                );
                return;
            }
            // The ledger of the shown session or an ancestor moved: re-read
            // the owner summary rather than sum deltas (51 §6.2).
            AgentEvent::ModelUsageChanged { session, .. } => {
                self.refresh_cost |= self.session == Some(*session)
                    || self.session_parent == Some(*session)
                    || self.private.as_ref().is_some_and(|p| p.root == *session);
                return;
            }
            // The owned private tree is closing (a leave from here already
            // dropped `private`; this is the harness's own close): let go of
            // everything it held.
            AgentEvent::SessionClosing { session, .. } => {
                if self.private.as_ref().is_some_and(|p| p.root == *session) {
                    let shown = self
                        .session
                        .is_some_and(|s| self.private.as_ref().is_some_and(|p| p.member(s)));
                    self.private = None;
                    if shown {
                        self.reset();
                        self.entries
                            .push(Entry::Info("private session closed by the harness".into()));
                    }
                }
                return;
            }
            AgentEvent::TurnStarted { session, .. } if self.session == Some(*session) => {
                self.streaming = true;
            }
            AgentEvent::TurnFinished { session, .. } => {
                self.search_invalidate(SearchCategory::Sessions);
                if self.session == Some(*session) {
                    self.streaming = false;
                } else {
                    self.track(&ev);
                }
            }
            _ => self.track(&ev),
        }
        // A subagent's stream is not rendered here; open it to read it. An
        // inspected page (49 §7) is not the live tail: arriving events must
        // not stitch across the missing middle or move the match offscreen.
        if ev.session().is_none_or(|s| self.session != Some(s)) || self.inspect.is_some() {
            return;
        }
        if !matches!(ev, AgentEvent::ReasoningDelta { .. })
            && let Some(Entry::Reasoning { done, .. }) = self.entries.last_mut()
        {
            *done = true;
        }
        if !matches!(ev, AgentEvent::TextDelta { .. })
            && let Some(Entry::Assistant(t)) = self.entries.last_mut()
        {
            t.finish();
        }
        match ev {
            // Echo the turn's input unless the human just typed it here, it was
            // a steer whose card moved up on `InputInjected`, or the harness
            // injected it (`MessageDelivered` already shows those). The card
            // that opens the turn carries `turn_seq`: the `/edit` handle.
            AgentEvent::TurnStarted {
                turn,
                turn_seq,
                text,
                model,
                ..
            } => {
                self.model_id = model;
                self.turn_seq = Some(turn_seq);
                // 125 §7.4: a steer's row is already its card (by seq, or by
                // the `<advisory>` wrapper when the card has not landed yet).
                if self.advisory_seqs.contains(&turn_seq) || injected(&text).is_some() {
                    return;
                }
                let shown = self.last_injected_turn.take() == Some(turn)
                    || self.sent.take().as_deref() == Some(&*text);
                if !shown {
                    self.entries.extend(user_entries(text, Some(turn_seq)));
                } else if let Some(Entry::User { turn, .. }) = self
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Entry::User { turn: None, .. }))
                {
                    *turn = Some(turn_seq);
                }
            }
            // The steer enters the context now: its card moves from the queue
            // to the transcript, after everything the model said before it.
            AgentEvent::InputInjected { id, turn, .. } => {
                if let Some(at) = self.queued.iter().position(|q| q.id == id) {
                    self.entries
                        .extend(user_entries(self.queued.remove(at).text, None));
                    self.last_injected_turn = Some(turn);
                }
            }
            // `InputQueued` is handled above, before the streaming-tail bookkeeping.
            AgentEvent::InputQueued { .. } => {}
            AgentEvent::TextDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) if !t.is_done() => t.push(&text),
                _ => {
                    let mut t = markdown::Text::with(self.assistant_options());
                    t.push(&text);
                    self.entries.push(Entry::Assistant(Box::new(t)));
                }
            },
            AgentEvent::ReasoningDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Reasoning {
                    text: s,
                    done: false,
                }) => s.push_str(&text),
                _ => self.entries.push(Entry::Reasoning {
                    text: text.to_string(),
                    done: false,
                }),
            },
            AgentEvent::ToolCallPending { call_id, name, .. } => {
                self.entries.push(Entry::Pending {
                    call_id,
                    name,
                    progress: None,
                });
            }
            AgentEvent::ToolCallProgress { call_id, args, .. } => {
                if let Some(Entry::Pending { progress, .. }) = self
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, Entry::Pending { call_id: c, .. } if *c == call_id))
                {
                    *progress = Some(Progress {
                        show: surface::partial_show(&args),
                        bytes: args.len(),
                    });
                }
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => {
                // A `.` in the id marks a call made from inside a `python` cell
                // (design doc 08 §4.1); it lands right after its parent card.
                let inner = call_id.contains('.');
                let tool = Entry::Tool {
                    call_id: call_id.clone(),
                    args: describe(&name, &args),
                    name: if inner { format!("↳ {name}") } else { name },
                    result: None,
                    expanded: None,
                    job: None,
                    artifact: None,
                    scrape: None,
                };
                match self
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, Entry::Pending { call_id: c, .. } if *c == call_id))
                {
                    Some(slot) => *slot = tool,
                    None => self.entries.push(tool),
                }
            }
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                match self.entries.iter_mut().rev().find(
                    |e| matches!(e, Entry::Tool { call_id: c, result: None, .. } if *c == call_id),
                ) {
                    Some(Entry::Tool { result, scrape, .. }) => {
                        *result = Some((output, is_error));
                        *scrape = None;
                    }
                    _ => self.entries.push(Entry::Info(format!(
                        "{} {}",
                        if is_error { "✗" } else { "✓" },
                        result_line(&output)
                    ))),
                }
            }
            AgentEvent::TurnFinished { usage, context, .. } => {
                // A call the model abandoned mid-stream never starts.
                self.entries.retain(|e| !matches!(e, Entry::Pending { .. }));
                self.usage = Some(usage);
                self.context = context;
                self.context_estimated = false;
                self.compacting = false;
                self.vision = None;
                self.resume_note = None;
            }
            // The gauge moves per round, with the cost (51 §6.4).
            AgentEvent::ContextUsed { context, size, .. } => {
                self.context = context;
                self.context_estimated = false;
                if let Some(s) = size {
                    self.context_size = Some(s);
                }
            }
            AgentEvent::ModelChanged { model, .. } => {
                self.entries.push(Entry::Info(format!("model → {model}")));
                self.model_id = model;
            }
            // The harness's own model (06 §8.2): announced only when it
            // switches away from one already shown, not at connect.
            AgentEvent::HarnessModelChanged { model, .. } => {
                if self.harness_model.as_ref().is_some_and(|m| *m != model) {
                    self.entries
                        .push(Entry::Info(format!("harness model → {model}")));
                }
                self.harness_model = Some(model);
            }
            // The harness's manifest (75 §3): the first report is silent, a
            // change in size afterwards is one line.
            AgentEvent::HarnessCommandsChanged { commands, .. } => {
                if !self.harness_commands.is_empty()
                    && self.harness_commands.len() != commands.len()
                {
                    self.entries.push(Entry::Info(format!(
                        "harness commands → {}",
                        commands.len()
                    )));
                }
                self.harness_commands = commands;
            }
            // Our own `SetEffort` is announced by its reply; anything else
            // (a `/model` switch dropping a level the new model lacks) here.
            AgentEvent::EffortChanged { effort, .. } => {
                if !self.awaiting_effort && self.effort != effort {
                    self.entries.push(Entry::Info(match &effort {
                        Some(e) => format!("effort → {e}"),
                        None => format!("effort reset: not offered by {}", self.model_id),
                    }));
                }
                self.effort = effort;
            }
            AgentEvent::Compacting { .. } => self.compacting = true,
            AgentEvent::Compacted {
                epoch,
                style,
                after,
                ..
            } => {
                self.compacting = false;
                self.context = after;
                self.context_estimated = true;
                if style == CompactStyle::Clear {
                    // A clean slate on screen too; `/resume` still shows everything.
                    // Compaction only runs between turns, so nothing is streaming.
                    self.entries.clear();
                    self.anchor = None;
                    self.entries
                        .push(Entry::Info(format!("context cleared (epoch {epoch})")));
                } else {
                    self.interject(Entry::Compacted {
                        epoch,
                        style,
                        summary: None,
                        expanded: false,
                    });
                }
            }
            AgentEvent::Error { message, .. } => {
                self.compacting = false;
                self.vision = None;
                self.entries.push(Entry::Error(message));
            }
            // 91: the thinking block is already closed by the generic rule above
            // (any non-delta event); the TUI shows no thinking time.
            AgentEvent::ReasoningDone { .. }
            | AgentEvent::SubagentSpawned { .. }
            | AgentEvent::MessageDelivered { .. }
            | AgentEvent::InputWithdrawn { .. }
            | AgentEvent::TitleChanged { .. }
            | AgentEvent::SessionDeleted { .. }
            | AgentEvent::ContextChanged { .. }
            | AgentEvent::TodoChanged { .. }
            | AgentEvent::DefaultModelChanged { .. }
            | AgentEvent::DefaultEffortChanged { .. }
            | AgentEvent::FavoriteModelsChanged { .. } => {}
            AgentEvent::Harvesting { .. } => self.harvesting = true,
            AgentEvent::Harvested { added, updated, .. } => {
                self.harvesting = false;
                self.interject(Entry::Info(format!("knowledge +{added} ~{updated}")))
            }
            AgentEvent::SkillsChanged { skills, .. } => self.loaded_skills = skills,
            // The consultation's steps in the status line; the settled note
            // is one collapsed card naming the sources (52 §6). Its body text
            // arrives with the reloaded transcript, not on the wire.
            AgentEvent::VisionProgress {
                stage,
                model,
                sources,
                ..
            } => {
                let line = vision_line(stage, &model, sources.len());
                if matches!(stage, VisionStage::Questioning | VisionStage::Inspecting) {
                    self.vision = Some(line);
                } else {
                    self.vision = None;
                    let body = sources
                        .iter()
                        .map(|s| format!("{} · {}", s.label, s.reference()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.interject(card(CardKind::Vision, line, &body));
                }
            }
            AgentEvent::SessionMetadataChanged { .. }
            | AgentEvent::ModelUsageChanged { .. }
            | AgentEvent::SessionOwnerChanged { .. }
            | AgentEvent::SessionClosing { .. }
            | AgentEvent::PartialDropped { .. } => {}
            // 125: handled before the streaming-tail bookkeeping above.
            AgentEvent::AdvisorChanged { .. } | AgentEvent::AdvisoryAdded { .. } => {}
            // 109 §10.3: the divider and the status-line note.
            AgentEvent::TurnResumed {
                kind,
                interrupted_at,
                ..
            } => {
                let at = interrupted_at.with_timezone(&chrono::Local).format("%H:%M");
                let (label, note) = match kind {
                    ResumeKind::Interrupted => (
                        "resumed after restart",
                        format!("interrupted by a restart at {at} — resumed"),
                    ),
                    ResumeKind::Suspended => (
                        "resumed after update",
                        format!("paused for an update at {at} — resumed"),
                    ),
                };
                self.interject(Entry::Divider(label.to_owned()));
                self.resume_note = Some(note);
            }
            AgentEvent::ResumeReport {
                replayed,
                interrupted,
                lost_jobs,
                ..
            } => {
                let mut note = format!("resumed · {} replayed", replayed.len());
                if !interrupted.is_empty() {
                    let names: Vec<&str> = interrupted.iter().map(|(_, n)| n.as_str()).collect();
                    note.push_str(&format!(
                        " · {} skipped: {} — effect unknown",
                        interrupted.len(),
                        names.join(", ")
                    ));
                }
                if !lost_jobs.is_empty() {
                    note.push_str(&format!(" · {} job(s) lost", lost_jobs.len()));
                }
                if !interrupted.is_empty() || !lost_jobs.is_empty() {
                    self.interject(Entry::Info(note.clone()));
                }
                self.resume_note = Some(note);
            }
            AgentEvent::TurnSuspended { reason, .. } => {
                self.interject(Entry::Divider(format!("suspended for {reason}")));
            }
        }
    }

    /// Fold an event of a subtree session into its row (design doc 19 §3.3).
    /// A direct child's error or cancel also interjects one line here: they
    /// must not be silent, and the board has everything else.
    fn track(&mut self, ev: &AgentEvent) {
        let Some(row) = ev.session().and_then(|s| self.agents.get_mut(&s)) else {
            return;
        };
        let direct = self.session == Some(row.info.parent);
        let mut note = None;
        match ev {
            AgentEvent::TurnStarted { .. } => {
                row.info.state = AgentState::Running;
                row.info.started_at = Some(chrono::Utc::now());
                row.tool = None;
                row.fresh = false;
            }
            AgentEvent::ToolCallPending { name, .. } => {
                row.tool = Some(format!("preparing {name}"));
            }
            AgentEvent::ToolCallStarted { name, args, .. } => {
                if name == "send_message" && args["await_reply"] == true {
                    row.info.state = AgentState::Awaiting {
                        from: args["to"].as_str().unwrap_or("?").to_owned(),
                    };
                    row.question = args["message"]
                        .as_str()
                        .and_then(|m| m.lines().next())
                        .map(str::to_owned);
                }
                row.tool = Some(match first_arg(args) {
                    Some(arg) => format!("running {name} {arg}"),
                    None => format!("running {name}"),
                });
            }
            AgentEvent::ToolCallFinished { .. } => {
                row.tool = None;
                if matches!(row.info.state, AgentState::Awaiting { .. }) {
                    row.info.state = AgentState::Running;
                }
            }
            AgentEvent::TurnFinished { stop_reason, .. } => {
                row.info.state = match stop_reason.as_str() {
                    "error" => AgentState::Error,
                    "cancelled" => AgentState::Cancelled,
                    _ => AgentState::Idle,
                };
                row.fresh = row.info.state != AgentState::Idle;
                row.info.started_at = None;
                row.info.finished_at = Some(chrono::Utc::now());
                row.tool = None;
                if direct && stop_reason == "cancelled" {
                    note = Some(Entry::Info(format!("↳ {} cancelled", row.info.name)));
                }
            }
            AgentEvent::Error { message, .. } => {
                row.error = Some(message.clone());
                if direct {
                    note = Some(Entry::Error(format!(
                        "↳ {} error: {message}",
                        row.info.name
                    )));
                }
            }
            _ => {}
        }
        if let Some(e) = note {
            self.interject(e);
        }
    }

    /// Handle a submitted input line: slash command or message.
    pub fn submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        self.submit_with(line, false, tx);
    }

    /// Ctrl/Cmd+Enter: like `submit`, but a mid-turn message interjects —
    /// the turn is cancelled and the message starts the next one now. Slash
    /// commands, prompt answers and idle sessions behave as `submit`.
    pub fn submit_now(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        self.submit_with(line, true, tx);
    }

    fn submit_with(&mut self, line: &str, interject: bool, tx: &UnboundedSender<Command>) {
        let line = line.trim();
        // An empty note is an answer (keep / none); every other step waits for text.
        if line.is_empty() && !matches!(self.prompt, Some(Prompt::SecretNote { .. })) {
            return;
        }
        // Anything you send is what you want to see land.
        self.anchor = None;
        if self.prompt.is_some() {
            self.prompt_submit(line, tx);
            return;
        }
        // `/edit` in progress: a plain line replaces the turn being edited
        // (slash commands still run and leave the edit pending).
        if let Some((idx, turn)) = self.editing.filter(|_| !line.starts_with('/')) {
            self.editing = None;
            let Some(session) = self.session else { return };
            let (text, _) = self.expand_pastes(line);
            self.entries.truncate(idx);
            self.entries.push(Entry::User {
                text: line.to_owned(),
                turn: None,
            });
            self.sent = Some(text.clone());
            send(
                tx,
                Command::EditTurn {
                    session,
                    turn,
                    text,
                },
            );
            return;
        }
        let (cmd, arg) = line
            .split_once(' ')
            .map_or((line, ""), |(c, a)| (c, a.trim()));
        match cmd {
            "/quit" | "/exit" => {
                // Leaving the process leaves the owned private tree (51 §4.3).
                self.leave_private(tx);
                self.quit = true;
            }
            "/sessions" => send(tx, Command::ListSessions),
            "/take" => match self.session {
                Some(s) => send(tx, Command::TakeSession(s)),
                None => self.entries.push(Entry::Error("no session open".into())),
            },
            "/search" => self.open_search(arg, tx),
            "/attach" => self.open_attach(arg, tx),
            // 75 §10: on an ACP child, `/resume` alone restarts its harness.
            "/resume" if arg.is_empty() && self.agent.is_some() => {
                if let Some(s) = self.session {
                    send(tx, Command::ResumeChild(s));
                }
            }
            "/resume" => match arg.parse::<SessionId>() {
                Ok(id) => self.resume(id, tx),
                Err(_) => self
                    .entries
                    .push(Entry::Error(format!("not a session id: {arg:?}"))),
            },
            "/new" if arg == "private" => self.new_private(tx),
            "/new" => {
                self.leave_private(tx);
                self.reset();
                self.entries.push(Entry::Info("new session".into()));
            }
            // 75 §8: an ACP session picks from its harness's options.
            "/model" | "/effort" if self.agent.is_some() && arg.is_empty() => {
                if let Some(s) = self.session {
                    send(tx, Command::HarnessOptions(s));
                }
            }
            "/model" | "/effort" if self.agent.is_some() => {
                if let Some(s) = self.session {
                    send(
                        tx,
                        Command::SetHarnessOption {
                            session: s,
                            config_id: cmd[1..].to_owned(),
                            value: arg.to_owned(),
                        },
                    );
                }
            }
            "/model" if arg.is_empty() => send(tx, Command::ListModels),
            "/model" => send(
                tx,
                Command::SetModel {
                    session: self.session,
                    model: arg.to_owned(),
                },
            ),
            "/title" if arg.is_empty() => send(tx, Command::GetTitleModel),
            "/title" => send(
                tx,
                Command::SetTitleModel((arg != "off" && arg != "clear").then(|| arg.to_owned())),
            ),
            "/vision" => {
                self.awaiting_vision = true;
                send(
                    tx,
                    if arg.is_empty() {
                        Command::GetVisionModel
                    } else {
                        Command::SetVisionModel(
                            (arg != "off" && arg != "clear").then(|| arg.to_owned()),
                        )
                    },
                );
            }
            "/effort" if arg.is_empty() => {
                let items = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.closest(&self.model_id))
                    .map(|m| m.efforts())
                    .unwrap_or_default();
                self.open_efforts(items, PickTarget::Session);
            }
            "/effort" => self.set_effort((arg != "clear").then(|| arg.to_owned()), tx),
            "/aws" if arg.is_empty() => send(tx, Command::ListAwsProfiles),
            "/aws" => send(
                tx,
                Command::SetAwsProfile((arg != "clear").then(|| arg.to_owned())),
            ),
            "/approvals" => self.approvals_cmd(arg, tx),
            "/advisor" => self.advisor_cmd(arg, tx),
            "/communication" => self.communication_cmd(arg, tx),
            "/peers" => self.peers_cmd(arg, tx),
            "/a2a" if arg.is_empty() => send(tx, Command::A2aStatus),
            "/a2a" if arg == "reload" => send(tx, Command::ReloadA2a),
            "/a2a" => self
                .entries
                .push(Entry::Error("usage: /a2a [reload]".into())),
            "/double" => match DoubleCmd::parse(arg) {
                Some(cmd) => send(tx, Command::Double(cmd)),
                None => self.entries.push(Entry::Error(DoubleCmd::USAGE.into())),
            },
            "/mcp" if arg.is_empty() => {
                self.awaiting_mcp = true;
                send(tx, Command::ListMcp(self.session));
            }
            "/mcp" => self.mcp_cmd(arg, tx),
            "/acp" if arg.is_empty() => {
                self.awaiting_acp = true;
                send(tx, Command::ListAcp { probe: false });
                send(tx, Command::ListAcpRegistry);
            }
            "/acp" => match arg.split_once(' ').map(|(s, r)| (s, r.trim())) {
                Some(("add", id)) if !id.is_empty() => send(tx, Command::AddAcp(id.to_owned())),
                Some(("remove", name)) if !name.is_empty() => {
                    send(tx, Command::RemoveAcp(name.to_owned()));
                }
                _ => self.entries.push(Entry::Error(
                    "usage: /acp [add <registry-id> | remove <name>]".into(),
                )),
            },
            "/skills" => self.skills_cmd(arg, tx),
            "/prompt" => send(tx, Command::ShowPrompt(self.session)),
            "/knowledge" => self.knowledge_cmd(arg, tx),
            "/login" if arg.is_empty() => send(tx, Command::ListLogins),
            "/login" => send(tx, Command::Login(arg.to_owned())),
            "/logout" if arg.is_empty() => self
                .entries
                .push(Entry::Error("usage: /logout <name>".into())),
            "/logout" => send(tx, Command::Logout(arg.to_owned())),
            "/todos" => self.toggle_todos(tx),
            // The line was taken out of the editor to get here: the region lands in an empty box.
            "/dictate" => {
                if let Some(cmd) = self.toggle_dictation(String::new(), String::new()) {
                    send(tx, cmd);
                }
            }
            "/talk" => {
                if let Some(cmd) = self.toggle_talk(false) {
                    send(tx, cmd);
                }
            }
            "/broadcast" => self.broadcast_cmd(arg, tx),
            "/agents" => match self.session {
                Some(session) => {
                    self.agents_picker_pending = true;
                    send(tx, Command::ListAgents(session));
                }
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/parent" => match self.session_parent {
                Some(parent) => self.resume(parent, tx),
                None => self
                    .entries
                    .push(Entry::Info("this is the top-level session".into())),
            },
            "/jobs" => match (self.session, arg.split_once(' ')) {
                (None, _) => self.entries.push(Entry::Info("no session".into())),
                (Some(session), None) if arg.is_empty() => send(tx, Command::ListJobs(session)),
                (Some(session), Some(("kill", id))) if !id.trim().is_empty() => send(
                    tx,
                    Command::CancelJob {
                        session,
                        id: id.trim().to_owned(),
                    },
                ),
                _ => self
                    .entries
                    .push(Entry::Error("usage: /jobs [kill <id>]".into())),
            },
            "/project" => match (self.session, arg) {
                (None, _) => self.entries.push(Entry::Info("no session".into())),
                (Some(_), "") => self
                    .entries
                    .push(Entry::Error("usage: /project <label|off>".into())),
                (Some(session), label) => send(
                    tx,
                    Command::SetSessionProject {
                        session,
                        project: (label != "off" && label != "clear").then(|| label.to_owned()),
                    },
                ),
            },
            "/cost" => match self.session {
                Some(session) => {
                    self.awaiting_cost = true;
                    send(tx, Command::SessionCost(session));
                }
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/terminals" => match self.session {
                Some(session) => send(tx, Command::ListTerminals(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/apps" if !self.app_enabled => self.entries.push(Entry::Info(
                "apps are off (app.enabled); /artifacts lists their sources".into(),
            )),
            "/artifacts" | "/apps" => match self.session {
                Some(session) => send(
                    tx,
                    Command::ListArtifacts {
                        session,
                        pick: true,
                        apps: cmd == "/apps",
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/schedules" => match arg.split_once(' ') {
                None if arg.is_empty() => send(tx, Command::ListSchedules),
                Some(("run", name)) if !name.trim().is_empty() => {
                    send(tx, Command::FireSchedule(name.trim().to_owned()))
                }
                Some(("history", name)) if !name.trim().is_empty() => {
                    send(tx, Command::ScheduleHistory(name.trim().to_owned()))
                }
                Some(("pause", name)) if !name.trim().is_empty() => {
                    send(tx, Command::EnableSchedule(name.trim().to_owned(), false))
                }
                Some(("resume", name)) if !name.trim().is_empty() => {
                    send(tx, Command::EnableSchedule(name.trim().to_owned(), true))
                }
                Some(("test", name)) if !name.trim().is_empty() => {
                    send(tx, Command::TestSchedule(name.trim().to_owned()))
                }
                _ => self.entries.push(Entry::Info(SCHEDULES_USAGE.into())),
            },
            "/tasks" => send(tx, Command::ListTasks(self.project.clone())),
            "/roster" => send(tx, Command::ListRoster),
            "/computer" => send(tx, Command::ComputerStatus),
            "/compact" => self.compact_cmd(arg, tx),
            "/endless" => self.endless_cmd(arg, tx),
            "/shake" => self.shake_cmd(arg, tx),
            "/clear" => match self.session {
                Some(session) => send(
                    tx,
                    Command::Compact {
                        session,
                        style: Some(CompactStyle::Clear),
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/cancel" => {
                if let Some(session) = self.session {
                    send(tx, Command::Cancel(session));
                }
            }
            "/undo" | "/redo" => match self.session {
                Some(session) if cmd == "/undo" => send(tx, Command::Undo(session)),
                Some(session) => send(tx, Command::Redo(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/edit" => self.turn_cmd(TurnAction::Edit, cmd, arg, tx),
            "/regen" => self.turn_cmd(TurnAction::Regen, cmd, arg, tx),
            "/fork" => self.turn_cmd(TurnAction::Fork, cmd, arg, tx),
            "/provider" => self.provider_cmd(arg, tx),
            "/secrets" => self.secrets_cmd(arg, tx),
            "/mesh" => self.mesh_cmd(arg, tx),
            "/sandbox" => self.sandbox_cmd(arg, tx),
            "/link" => self.link_cmd(arg, tx),
            "/nodes" => self.nodes_cmd(arg, tx),
            // `/<skill> [args]` hands the skill to the model (04 §2.4); any other `/word` — a path, a typo —
            // is message text and takes the arm below.
            c if c.starts_with('/') && self.skill_word(&c[1..]) => {
                self.use_skill(c[1..].to_owned(), arg.to_owned(), tx);
            }
            _ => {
                let (text, files) = self.expand_pastes(line);
                match self.session {
                    // Mid-turn, or mid-`/compact` (a mini-turn without `TurnStarted`;
                    // 07 §8.6): the card waits below the transcript until
                    // `InputInjected` says where in the conversation it landed.
                    Some(session) if self.streaming || self.compacting => {
                        let id = InputId::new();
                        self.queued.push(QueuedInput {
                            id,
                            text: text.clone(),
                        });
                        send(
                            tx,
                            Command::Steer {
                                session,
                                id,
                                text,
                                files,
                                interject,
                            },
                        );
                    }
                    session => {
                        self.entries.push(Entry::User {
                            text: line.to_owned(),
                            turn: None,
                        });
                        self.sent = Some(text.clone());
                        self.awaiting_session = session.is_none();
                        send(
                            tx,
                            Command::Send {
                                session,
                                text,
                                files,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Does `/name` hand over a skill: a known one; before the index has
    /// arrived, any plain word (the agent names an unknown skill) — but not
    /// one holding another `/` or a `.` (`/etc/hosts`, `/x.txt`), nor a bare `/`.
    fn skill_word(&self, name: &str) -> bool {
        if self.skills.is_empty() {
            !name.is_empty() && !name.contains(['/', '.'])
        } else {
            self.skills.iter().any(|s| s.name == name)
        }
    }

    /// Switch to `id`: ask for its history and its subtree (design doc 19
    /// §3.5). `/resume`, `/sessions`, `/agents` and `/parent` all land here.
    /// Pending approvals ride along (an approval `o`pened from stays up); the
    /// subtree reply prunes the ones that do not belong to the new view.
    pub fn resume(&mut self, id: SessionId, tx: &UnboundedSender<Command>) {
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
        // Root→child→parent moves stay inside the owned tree; anything else releases it (51 §4.3).
        if self.private.as_ref().is_some_and(|p| !p.member(id)) {
            self.leave_private(tx);
        }
        self.reset();
        self.approvals = approvals;
        self.asks = asks;
        self.session = Some(id);
        send(
            tx,
            Command::Load {
                session: id,
                settled_only: false,
            },
        );
        send(tx, Command::ListAgents(id));
    }

    /// Esc: cancel the current session's in-flight turn or `/compact`, if any
    /// (the root only); the third Esc within [`ESC_TAPS_WINDOW`] stops its
    /// running subagents too (133 §5).
    pub fn cancel(&mut self, tx: &UnboundedSender<Command>) {
        self.cancel_at(std::time::Instant::now(), tx);
    }

    /// [`Self::cancel`] with the clock injected.
    pub fn cancel_at(&mut self, now: std::time::Instant, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session.filter(|_| self.streaming || self.compacting) else {
            return;
        };
        self.esc_taps
            .retain(|t| now.duration_since(*t) < ESC_TAPS_WINDOW);
        self.esc_taps.push(now);
        if self.esc_taps.len() < 3 {
            send(tx, Command::Cancel(session));
            return;
        }
        self.esc_taps.clear();
        let n = self
            .agents
            .values()
            .filter(|r| r.info.id != session && r.live())
            .count();
        self.entries.push(Entry::Info(format!(
            "stopped {n} agent{}",
            if n == 1 { "" } else { "s" }
        )));
        send(tx, Command::CancelTree(session));
    }

    /// Alt+Up: take the newest queued steer back into the editor. Only into
    /// an empty editor, so nothing typed is overwritten; the text arrives
    /// with `UiEvent::Withdrawn` once the agent confirms it was still queued.
    pub fn withdraw(&mut self, editor_empty: bool, tx: &UnboundedSender<Command>) {
        let Some(q) = self.queued.last() else { return };
        if !editor_empty {
            self.entries.push(Entry::Info("editor not empty".into()));
            return;
        }
        if let Some(session) = self.session {
            send(tx, Command::Withdraw { session, id: q.id });
        }
    }

    /// The transcript's turns, oldest first: `(entry index, turn seq, first
    /// line of the opener)`. The 1-based position is the `n` of `/edit n`.
    fn turns(&self) -> Vec<(usize, i64, String)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entry::User {
                    text,
                    turn: Some(t),
                } => Some((
                    i,
                    *t,
                    crate::attach::spoken(text)
                        .map_or(text.as_str(), |(_, _, body)| body)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// `[n]` of `/edit`, `/regen`, `/fork`: `None` when absent (open the
    /// picker), the turn's `(entry index, seq)` when it names one.
    fn turn_arg(&self, cmd: &str, arg: &str) -> Result<Option<(usize, i64)>, String> {
        if arg.is_empty() {
            return Ok(None);
        }
        let n: usize = arg.parse().map_err(|_| format!("usage: {cmd} [n]"))?;
        self.turns()
            .get(n.wrapping_sub(1))
            .map(|&(idx, seq, _)| Some((idx, seq)))
            .ok_or_else(|| format!("no turn #{n}"))
    }

    /// `/edit`, `/regen`, `/fork`: act on turn `arg`, or open the picker on the newest.
    fn turn_cmd(
        &mut self,
        action: TurnAction,
        cmd: &str,
        arg: &str,
        tx: &UnboundedSender<Command>,
    ) {
        if self.session.is_none() {
            self.entries.push(Entry::Info("no session".into()));
            return;
        }
        if self.turn_blocked(action) {
            return;
        }
        match self.turn_arg(cmd, arg) {
            Err(e) => self.entries.push(Entry::Error(e)),
            Ok(Some((idx, turn))) => self.turn_action(action, idx, turn, tx),
            Ok(None) => {
                let items = self.turns();
                if items.is_empty() {
                    self.entries.push(Entry::Error("no turns yet".into()));
                } else {
                    self.menu = Some(Menu::Turns {
                        cursor: items.len() - 1,
                        items,
                        action,
                    });
                }
            }
        }
    }

    /// Edit and regenerate are refused while a turn runs (the server's
    /// `"turn in flight"`); fork copies stored rows and is not. Pushes the
    /// error and says whether `action` must wait.
    fn turn_blocked(&mut self, action: TurnAction) -> bool {
        let blocked = self.streaming && action != TurnAction::Fork;
        if blocked {
            self.entries
                .push(Entry::Error("turn in flight — /cancel first".into()));
        }
        blocked
    }

    /// Apply `action` to the turn at `entries[idx]` (29 §6.2). Regenerate
    /// drops the turn from the transcript now; the new one streams back in.
    /// Edit only puts the opener in the editor: `submit` does the rest.
    fn turn_action(
        &mut self,
        action: TurnAction,
        idx: usize,
        turn: i64,
        tx: &UnboundedSender<Command>,
    ) {
        let Some(session) = self.session else { return };
        // Checked again: a turn may have started while the picker was open.
        if self.turn_blocked(action) {
            return;
        }
        match action {
            TurnAction::Regen => {
                self.entries.truncate(idx);
                send(tx, Command::Regenerate { session, turn });
            }
            TurnAction::Edit => {
                // The line that ran `/edit` is gone; held pastes are what is left.
                if !self.pastes.is_empty() {
                    self.entries.push(Entry::Info("editor not empty".into()));
                    return;
                }
                let Some(Entry::User { text, .. }) = self.entries.get(idx) else {
                    return;
                };
                self.recall = Some(text.clone());
                self.editing = Some((idx, turn));
            }
            TurnAction::Fork => send(
                tx,
                Command::Fork {
                    session,
                    through: Some(turn),
                },
            ),
        }
    }

    /// Esc while `/edit` is pending: forget the edit and empty the editor.
    /// Returns whether one was pending.
    pub fn cancel_edit(&mut self) -> bool {
        if self.editing.take().is_none() {
            return false;
        }
        self.recall = Some(String::new());
        true
    }

    /// Answer the shown approval and dismiss it locally; `ApprovalResolved`
    /// (or a `Resolve` failure) is then a no-op for this id.
    pub fn decide(&mut self, decision: Decision, tx: &UnboundedSender<Command>) {
        if let Some(req) = self.approvals.pop_front() {
            self.approval_done(req.session);
            send(
                tx,
                Command::Resolve {
                    id: req.id,
                    decision,
                },
            );
        }
    }

    /// A key while an ask is shown. Returns text for the input box when it
    /// must change (entering or leaving freeform typing, sending an answer).
    pub fn ask_key(
        &mut self,
        code: KeyCode,
        input: &str,
        tx: &UnboundedSender<Command>,
    ) -> Option<String> {
        let a = self.asks.front_mut()?;
        if a.typing {
            return match code {
                KeyCode::Enter => {
                    let text = input.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let text = text.to_owned();
                    self.ask_answer(Answer::Text { text }, tx);
                    Some(String::new())
                }
                KeyCode::Esc => {
                    a.typing = false;
                    Some(String::new())
                }
                _ => None,
            };
        }
        let n = a.question().options.len();
        match code {
            KeyCode::Up => a.cursor = a.cursor.saturating_sub(1),
            KeyCode::Down => a.cursor = (a.cursor + 1).min(n),
            KeyCode::Enter if a.cursor < n => {
                let index = a.cursor;
                self.ask_answer(Answer::Choice { index }, tx);
            }
            KeyCode::Enter => a.typing = true,
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let d = usize::from(c as u8 - b'0');
                if (1..=n).contains(&d) {
                    self.ask_answer(Answer::Choice { index: d - 1 }, tx);
                }
            }
            KeyCode::Char('s') | KeyCode::Tab => self.ask_answer(Answer::Skipped, tx),
            KeyCode::Esc => {
                if let Some(a) = self.asks.pop_front() {
                    send(
                        tx,
                        Command::AnswerAsk {
                            id: a.req.id,
                            answers: None,
                        },
                    );
                }
            }
            _ => {}
        }
        None
    }

    /// Record the shown question's answer; the last one sends the set.
    fn ask_answer(&mut self, answer: Answer, tx: &UnboundedSender<Command>) {
        let Some(a) = self.asks.front_mut() else {
            return;
        };
        a.answers.push(answer);
        a.typing = false;
        a.current += 1;
        if a.current < a.req.questions.len() {
            a.cursor = a.question().recommended.unwrap_or(0);
            return;
        }
        if let Some(a) = self.asks.pop_front() {
            send(
                tx,
                Command::AnswerAsk {
                    id: a.req.id,
                    answers: Some(a.answers),
                },
            );
        }
    }

    /// One-line status: session, streaming state, last usage, idle
    /// subagents (while the board is hidden; it has the live ones), queue.
    pub fn status(&self) -> String {
        let session = self.session.map_or_else(|| "no session".to_owned(), short);
        let state = self
            .activity()
            .map_or_else(|| "idle".to_owned(), |a| format!("{} {a}", self.spinner()));
        let usage = self.usage.map_or_else(String::new, |u| {
            format!("  {}↑ {}↓", u.prompt, u.completion)
        });
        let agents = match self.agents.len() {
            0 => String::new(),
            _ if self.agents.values().any(AgentRow::on_board) => String::new(),
            n => format!("  agents: {n} idle"),
        };
        let queued = match self.queued.len() {
            0 => String::new(),
            n => format!("  ·  {n} queued"),
        };
        let jobs = match self.jobs.len() {
            0 => String::new(),
            n => format!("  ·  {n} job(s)"),
        };
        let harness_skills: Vec<&str> = self
            .harness_commands
            .iter()
            .filter_map(|c| c.name.strip_prefix("skill:"))
            .collect();
        let skills = match (self.harness_commands.len(), self.loaded_skills.len()) {
            (0, 0) => String::new(),
            (0, _) => format!("  ·  skills: {}", self.loaded_skills.join(", ")),
            (n, _) if harness_skills.is_empty() => format!("  ·  harness commands: {n}"),
            _ => format!("  ·  harness skills: {}", harness_skills.join(", ")),
        };
        let scrolled = if self.anchor.is_some() {
            "  ·  ↑ history · PgDn follows"
        } else {
            ""
        };
        let editing = self.editing.map_or_else(String::new, |(idx, _)| {
            let n = self.entries[..idx.min(self.entries.len())]
                .iter()
                .filter(|e| matches!(e, Entry::User { turn: Some(_), .. }))
                .count()
                + 1;
            format!("  ·  editing #{n} — ⏎ sends, esc cancels")
        });
        let todos = if self.todo_focused() {
            "  ·  todos: ↑↓ scroll · esc closes · tab editor"
        } else {
            ""
        };
        let talk = match &self.talk {
            Some(t) if !self.talk_view => {
                let state = format!("{:?}", t.state).to_lowercase();
                format!("  ·  ● talk: {state} (/talk)")
            }
            _ => String::new(),
        };
        let attached = self
            .attached
            .as_deref()
            .map_or_else(String::new, |a| format!("  ·  {a}"));
        let resumed = self
            .resume_note
            .as_deref()
            .map_or_else(String::new, |n| format!("  ·  {n}"));
        format!(
            "{session}  {state}{usage}{agents}{queued}{jobs}{skills}{editing}{scrolled}{todos}{talk}{attached}{resumed}"
        )
    }

    /// Line above the input: `model  ·  high  ·  ctx 12.3k / 200k (6%)  ·  cache 94%`
    /// and its colour level. In a subagent the model is headed by its peer
    /// name: `↳ grep-logs  child · model  ·  …`. An ACP child shows the
    /// harness and its reported model: `omp · anthropic/claude-opus-4-8`
    /// (06 §8.2). The effort segment is omitted while unset; `cache` is the
    /// prefix-cache hit ratio of the last finished turn (design doc 07 §7),
    /// omitted until one has reported usage.
    pub fn header(&self, catalog: &Catalog) -> (String, Level) {
        let used = format!(
            "{}{}",
            if self.context_estimated { "≈" } else { "" },
            k(self.context)
        );
        let cache = match self.usage {
            Some(u) if u.prompt > 0 => {
                format!(
                    "  ·  cache {}%",
                    u64::from(u.cached) * 100 / u64::from(u.prompt)
                )
            }
            _ => String::new(),
        };
        let effort = self
            .effort
            .as_deref()
            .map_or_else(String::new, |e| format!("  ·  {e}"));
        let model = match &self.harness_model {
            Some(m) => format!("{} · {m}", self.model_id),
            None => self.model_id.clone(),
        };
        let model = match &self.session_name {
            Some(name) => format!("↳ {name}  child · {model}"),
            None => model,
        };
        let limit = self
            .context_size
            .or_else(|| catalog.closest(&self.model_id).map(|m| m.limit.context));
        match limit {
            Some(limit) if limit > 0 => {
                let pct = u64::from(self.context) * 100 / u64::from(limit);
                let level = match pct {
                    0..75 => Level::Normal,
                    75..90 => Level::Warn,
                    _ => Level::Danger,
                };
                (
                    format!(
                        "{model}{effort}  ·  ctx {used} / {} ({pct}%){cache}{}{}{}{}{}{}{}",
                        k(limit),
                        self.cost_tag(),
                        self.advisor_tag(),
                        self.approvals_tag(),
                        self.communication_tag(),
                        self.link_tag(),
                        self.nodes_tag(),
                        self.update_tag()
                    ),
                    level,
                )
            }
            _ => (
                format!(
                    "{model}{effort}  ·  ctx {used}{cache}{}{}{}{}{}{}{}",
                    self.cost_tag(),
                    self.advisor_tag(),
                    self.approvals_tag(),
                    self.communication_tag(),
                    self.link_tag(),
                    self.nodes_tag(),
                    self.update_tag()
                ),
                Level::Normal,
            ),
        }
    }

    /// `  ·  advisor $0.012` (125 §7.5): the advisor session's spend from the
    /// last `AdvisorStatus`; empty while off or before it was priced.
    fn advisor_tag(&self) -> String {
        match (&self.advisor.state, &self.advisor.cost) {
            (AdvisorState::Off, _) | (_, None) => String::new(),
            (_, Some(c)) => format!("  ·  advisor {}", cost_short(c)),
        }
    }

    /// The status line's advisor segment (125 §7.5): `  ·  ◎ advisor: idle` /
    /// `⠋ reviewing…` / `paused` / `halted` with the state that colours it;
    /// `None` while off.
    pub fn advisor_segment(&self) -> Option<(String, AdvisorState)> {
        let state = self.advisor.state;
        let word = match state {
            AdvisorState::Off => return None,
            AdvisorState::Reviewing => format!("{} reviewing…", self.spinner()),
            s => advisor_state_name(s).to_owned(),
        };
        Some((format!("  ·  {ADVISOR_GLYPH} advisor: {word}"), state))
    }

    fn approvals_tag(&self) -> &'static str {
        if !self.approvals_global || self.approvals_off {
            "  ·  approvals off"
        } else {
            ""
        }
    }

    /// `  ·  communication off` while the authoritative state says the
    /// shown session (or the global default) is not effectively enabled;
    /// `  ·  communication on` when it is; empty until fetched. Never
    /// inferred from a local toggle (50 §0.8).
    fn communication_tag(&self) -> &'static str {
        match &self.communication {
            None => "",
            Some(c) if c.effective => "  ·  communication on",
            Some(_) => "  ·  communication off",
        }
    }

    /// `  ·  link ●N` (N = devices online) while the uplink is registered
    /// and a device is paired; empty otherwise.
    fn link_tag(&self) -> String {
        match &self.link {
            Some(s) if s.connected && !s.devices.is_empty() => {
                format!(
                    "  ·  link ●{}",
                    s.devices.iter().filter(|d| d.online).count()
                )
            }
            _ => String::new(),
        }
    }

    /// `  ·  nodes ●N` (N = nodes online) while a node is paired; empty otherwise.
    fn nodes_tag(&self) -> String {
        match self.nodes.as_deref() {
            Some([]) | None => String::new(),
            Some(list) => format!("  ·  nodes ●{}", list.iter().filter(|n| n.online).count()),
        }
    }

    /// `  ·  0.2.0 ready` once a newer binary is installed, `  ·  0.2.0
    /// available` when only staged; empty otherwise (39 §4.1).
    fn update_tag(&self) -> String {
        match &self.update {
            Some(u) if u.applied => format!("  ·  {} ready", u.version),
            Some(u) => format!("  ·  {} available", u.version),
            None => String::new(),
        }
    }

    /// Open the `/effort` picker over `items` plus a `clear` row; no levels → a notice.
    /// `target` says whose effort the pick sets (125 §7.2).
    fn open_efforts(&mut self, mut items: Vec<String>, target: PickTarget) {
        if items.is_empty() {
            let model = match target {
                PickTarget::Session => self.model_id.as_str(),
                PickTarget::Advisor => self.advisor.model.as_deref().unwrap_or("advisor"),
            };
            self.entries.push(Entry::Info(format!(
                "{model} has no reasoning effort levels"
            )));
            return;
        }
        items.push("clear".into());
        self.menu = Some(Menu::Efforts {
            items,
            cursor: 0,
            target,
        });
    }

    /// `/advisor [off|on|status|<provider/model>]` (125 §7.1): bare opens the
    /// `/model` picker targeted at the advisor; `on` re-uses the remembered
    /// model; anything else is a `provider/model` id. Not on an ACP child:
    /// the advisor reads theuth's transcript rows, which a harness does not produce.
    fn advisor_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        if self.agent.is_some() {
            self.entries.push(Entry::Error(
                "/advisor: not available for an external harness session".into(),
            ));
            return;
        }
        if arg.is_empty() {
            self.pick_target = PickTarget::Advisor;
            send(tx, Command::ListModels);
            return;
        }
        let Some(session) = self.session else {
            self.entries
                .push(Entry::Error("no session yet: send a message first".into()));
            return;
        };
        let (model, effort) = match arg {
            "status" => {
                self.advisor_reply = Some(AdvisorReply::Status);
                send(tx, Command::AdvisorStatus(session));
                return;
            }
            "off" => (None, None),
            "on" => match self.advisor_default.model.clone() {
                Some(m) => (Some(m), self.advisor_default.effort.clone()),
                None => {
                    self.entries.push(Entry::Error(
                        "/advisor on: no remembered advisor model — pick one with /advisor".into(),
                    ));
                    return;
                }
            },
            id => (Some(id.to_owned()), None),
        };
        self.set_advisor(session, model, effort, tx);
    }

    /// `SetAdvisor` for `session`; a pick (`Some` model) chains into the
    /// advisor's effort picker when the reply names a model with levels.
    fn set_advisor(
        &mut self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
        tx: &UnboundedSender<Command>,
    ) {
        self.advisor_reply = (model.is_some() && effort.is_none()).then_some(AdvisorReply::Pick);
        send(
            tx,
            Command::SetAdvisor {
                session,
                model,
                effort,
            },
        );
    }

    /// Install the shown session's advisor status (a reply or `AdvisorChanged`,
    /// whichever lands first): one `advisor → …` line when the model or the
    /// off state changed (125 §7.5); a model remembers itself as the `/advisor on` default.
    fn set_advisor_status(&mut self, status: AdvisorStatus) {
        let off = |s: &AdvisorStatus| s.state == AdvisorState::Off || s.model.is_none();
        let changed = status.model != self.advisor.model || off(&status) != off(&self.advisor);
        if status.model.is_some() {
            self.advisor_default.model.clone_from(&status.model);
            self.advisor_default.effort.clone_from(&status.effort);
        }
        self.advisor = status;
        if changed {
            self.interject(Entry::Info(advisor_changed_line(&self.advisor)));
        }
    }

    /// `SetEffort` for the root session (and the default); the reply announces it.
    fn set_effort(&mut self, effort: Option<String>, tx: &UnboundedSender<Command>) {
        self.awaiting_effort = true;
        send(
            tx,
            Command::SetEffort {
                session: self.session,
                effort,
            },
        );
    }

    /// `/approvals [on|off] [always]`: bare shows the state; `always` switches
    /// the persisted default, otherwise the root session.
    fn approvals_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let mut words = arg.split_whitespace();
        let enabled = match words.next() {
            None => {
                self.entries.push(Entry::Info(format!(
                    "approvals: {} (this session), {} (default)",
                    on_off(!self.approvals_off),
                    on_off(self.approvals_global)
                )));
                return;
            }
            Some("on") => true,
            Some("off") => false,
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/approvals: expected on|off, got {w:?}"
                )));
                return;
            }
        };
        let session = match words.next() {
            Some("always") => None,
            None => match self.session {
                Some(id) => Some(id),
                None => {
                    self.entries.push(Entry::Error(
                        "no session yet: send a message first, or use `/approvals off always`"
                            .into(),
                    ));
                    return;
                }
            },
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/approvals: expected `always`, got {w:?}"
                )));
                return;
            }
        };
        if session.is_some() {
            self.approvals_off = !enabled;
        }
        send(tx, Command::SetApprovals { session, enabled });
    }

    /// Install an authoritative state for the shown session; an older
    /// revision (a slow reply racing a change) is dropped. `true` when installed.
    fn set_communication(&mut self, s: CommunicationState) -> bool {
        if self
            .communication
            .as_ref()
            .is_some_and(|c| c.session == s.session && c.revision > s.revision)
        {
            return false;
        }
        self.communication = Some(s);
        true
    }

    /// Remember a body `PeerDelivered` carded, bounded.
    fn remember_peer(&mut self, text: &str) {
        if self.peer_shown.len() == 8 {
            self.peer_shown.pop_front();
        }
        self.peer_shown.push_back(text.to_owned());
    }

    /// `/communication [on|off] [always]` (50 §2.1): bare asks for the state
    /// and prints it; `always` moves the persisted global switch, otherwise
    /// this session's memory-only opt-out. Nothing is assumed locally: the
    /// reply says what is effective and why.
    fn communication_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let mut words = arg.split_whitespace();
        let enabled = match words.next() {
            None => {
                self.awaiting_communication = true;
                send(tx, Command::GetCommunication(self.session));
                return;
            }
            Some("on") => true,
            Some("off") => false,
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/communication: expected on|off, got {w:?}"
                )));
                return;
            }
        };
        let session = match words.next() {
            Some("always") => None,
            None => match self.session {
                Some(s) => Some(s),
                None => {
                    self.entries.push(Entry::Error(
                        "no session yet: send a message first, or use `/communication on always`"
                            .into(),
                    ));
                    return;
                }
            },
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/communication: expected `always`, got {w:?}"
                )));
                return;
            }
        };
        self.awaiting_communication = true;
        send(
            tx,
            Command::SetCommunication {
                session,
                enabled,
                view: self.session,
            },
        );
    }

    /// `/peers` opens the picker; `/peers scope owner|isolated [alias]` sets
    /// the shown root's scope (50 §2.3): owner-only, roots only, so a child
    /// is told where to go instead of guessing its root.
    fn peers_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info(
                "no session — `/communication` shows the global default".into(),
            ));
            return;
        };
        let mut words = arg.split_whitespace();
        match words.next() {
            None => {
                self.menu = Some(Menu::Peers {
                    session,
                    peers: None,
                    requests: None,
                    cursor: 0,
                });
                send(tx, Command::ListPeers(session));
                send(tx, Command::ListPeerRequests(session));
            }
            Some("scope") => {
                let scope = match words.next().map(str::parse::<PeerScope>) {
                    Some(Ok(s)) => s,
                    _ => {
                        self.entries.push(Entry::Error(
                            "usage: /peers scope owner|isolated [alias]".into(),
                        ));
                        return;
                    }
                };
                if self.session_parent.is_some() {
                    self.entries.push(Entry::Error(
                        "scope belongs to the root session: `/parent` up to it, then `/peers scope …`"
                            .into(),
                    ));
                    return;
                }
                let alias: String = words.collect::<Vec<_>>().join(" ");
                self.awaiting_communication = true;
                send(
                    tx,
                    Command::SetPeerScope {
                        root: session,
                        scope,
                        alias: (!alias.is_empty()).then_some(alias),
                    },
                );
            }
            Some(_) => self.entries.push(Entry::Error(
                "usage: /peers [scope owner|isolated [alias]]".into(),
            )),
        }
    }

    /// `/peers` menu title: the effective state and its blocker, then the keys.
    pub fn peers_title(&self) -> String {
        let state = match &self.communication {
            None => "communication: …".to_owned(),
            Some(c) => communication_lines(c).remove(0),
        };
        format!("peers · {state}   ⏎ open  i inspect  a answer  r refresh  esc")
    }

    /// `/peers` rows in cursor order: the directory (or its one-line
    /// blocker), a separator, then pending requests by exact id.
    pub fn peers_rows(&self) -> Vec<PeerRow<'_>> {
        let Some(Menu::Peers {
            peers, requests, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows: Vec<PeerRow<'_>> = match peers {
            None => vec![PeerRow::Note("loading peers…".into())],
            Some(Err(e)) => vec![PeerRow::Note(format!("no peers · {e}"))],
            Some(Ok(p)) if p.peers.is_empty() => {
                vec![PeerRow::Note("no peers this session may address".into())]
            }
            Some(Ok(p)) => p.peers.iter().map(PeerRow::Peer).collect(),
        };
        rows.push(PeerRow::Sep);
        match requests {
            None => rows.push(PeerRow::Note("loading pending requests…".into())),
            Some(page) => {
                let pending: Vec<PeerRow<'_>> = page
                    .messages
                    .iter()
                    .filter_map(|r| {
                        let id = r.receipt.id;
                        if page.awaiting_answer.contains(&id) {
                            Some(PeerRow::Request(r, false))
                        } else if page.awaiting_reply.contains(&id) {
                            Some(PeerRow::Request(r, true))
                        } else {
                            None
                        }
                    })
                    .collect();
                if pending.is_empty() {
                    rows.push(PeerRow::Note(
                        "no requests await a human (a restart clears live waits)".into(),
                    ));
                } else {
                    rows.extend(pending);
                }
            }
        }
        rows
    }

    /// `/peers` keys: ⏎ opens a native peer's session, `i`/⏎ on a request
    /// cards its full record, `a` answers it (exact id), `r` refetches.
    fn peers_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Open(SessionId),
            Inspect(Entry),
            Answer(Prompt),
            Refresh,
            Note(&'static str),
            Close,
            Nothing,
        }
        let Some(Menu::Peers {
            session, cursor, ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let session = *session;
        let rows = self.peers_rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        // Up/Down step over the separator row.
        let step = |to: usize, dir: isize| match rows.get(to) {
            Some(PeerRow::Sep) => to.saturating_add_signed(dir).min(last),
            _ => to,
        };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('r'), _) => Act::Refresh,
            (KeyCode::Enter, Some(PeerRow::Peer(p))) => {
                match agent::communication::PeerAddress::local(&p.address) {
                    Some(id) => Act::Open(id),
                    None => Act::Note("remote peer: no transcript here (/a2a shows its status)"),
                }
            }
            (KeyCode::Enter | KeyCode::Char('i'), Some(PeerRow::Request(r, resolve))) => {
                Act::Inspect(request_card(r, *resolve))
            }
            (KeyCode::Char('a'), Some(PeerRow::Request(r, resolve))) => {
                Act::Answer(Prompt::PeerAnswer {
                    session,
                    reply_to: r.receipt.id,
                    from: if *resolve {
                        r.receipt.to.clone()
                    } else {
                        r.from.clone()
                    },
                    resolve: *resolve,
                })
            }
            _ => Act::Nothing,
        };
        let Some(Menu::Peers { cursor: cur, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Open(id) => {
                self.menu = None;
                self.resume(id, tx);
            }
            Act::Inspect(card) => {
                self.menu = None;
                self.entries.push(card);
            }
            Act::Answer(p) => {
                self.menu = None;
                self.prompt = Some(p);
            }
            Act::Refresh => {
                send(tx, Command::ListPeers(session));
                send(tx, Command::ListPeerRequests(session));
            }
            Act::Note(n) => self.entries.push(Entry::Info(n.into())),
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
        MenuKey::Consumed
    }

    /// `/skills add <url>` | `rm <name>` | `sync`.
    fn skills_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.skills_picker_pending = true;
                send(tx, Command::ListSkills);
            }
            ("sync", _) => send(tx, Command::SyncSkills),
            ("add", url) if !url.is_empty() => send(tx, Command::AddSkillRepo(url.to_owned())),
            ("rm", name) if !name.is_empty() => send(tx, Command::RemoveSkill(name.to_owned())),
            _ => self.entries.push(Entry::Error(
                "usage: /skills [add <url> | rm <name> | sync]".into(),
            )),
        }
    }

    /// `/knowledge [query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]`.
    fn knowledge_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Command::KnowledgeList),
            ("history", id) => {
                self.open_history((!id.is_empty()).then(|| id.to_owned()), HISTORY_PAGE, tx);
            }
            ("show", "") => self
                .entries
                .push(Entry::Error("usage: /knowledge show <commit> [id]".into())),
            ("show", rest) => {
                let (commit, id) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(c, i)| (c, Some(i.trim().to_owned())));
                send(
                    tx,
                    Command::KnowledgeRevision {
                        commit: commit.to_owned(),
                        id,
                        card: true,
                    },
                );
            }
            ("restore", "") => self.entries.push(Entry::Error(
                "usage: /knowledge restore <id> [commit]".into(),
            )),
            ("restore", rest) => {
                let (id, commit) = rest
                    .split_once(' ')
                    .map_or((rest, None), |(i, c)| (i, Some(c.trim().to_owned())));
                send(
                    tx,
                    Command::KnowledgeRestore {
                        id: id.to_owned(),
                        commit,
                    },
                );
            }
            ("harvest", _) => match self.session {
                Some(session) => send(tx, Command::Harvest(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            _ => send(tx, Command::KnowledgeSearch(arg.to_owned())),
        }
    }

    /// `/mcp add <url|json>` | `sync`.
    fn mcp_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("sync", _) => send(tx, Command::SyncMcp),
            ("add", src) if git_url(src) => send(tx, Command::AddMcpRepo(src.to_owned())),
            ("add", src) if !src.is_empty() => match mcp_entry(src) {
                Ok((name, entry)) => send(tx, Command::AddMcp { name, entry }),
                Err(e) => self.entries.push(Entry::Error(format!("/mcp add: {e}"))),
            },
            _ => self
                .entries
                .push(Entry::Error("usage: /mcp [add <url|json> | sync]".into())),
        }
    }

    /// `/provider` list | `add` [name [url [key]]] | `rm <name>`.
    fn provider_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Command::ListProviders),
            ("add", "") => {
                self.prompt = Some(Prompt::ProviderName);
                self.entries.push(Entry::Info("provider add: name".into()));
            }
            ("add", rest) => {
                let mut parts = rest.splitn(3, char::is_whitespace);
                let name = parts.next().unwrap_or_default();
                let url = parts.next().unwrap_or("").trim();
                let key = parts.next().unwrap_or("").trim();
                if name.is_empty() {
                    self.entries.push(Entry::Error(
                        "usage: /provider add [name [url [key]]]".into(),
                    ));
                    return;
                }
                if url.is_empty() {
                    self.prompt = Some(Prompt::ProviderUrl {
                        name: name.to_owned(),
                    });
                    self.entries
                        .push(Entry::Info(format!("provider add {name}: base url")));
                    return;
                }
                if key.is_empty() {
                    self.prompt = Some(Prompt::ProviderKey {
                        name: name.to_owned(),
                        url: url.to_owned(),
                    });
                    self.entries
                        .push(Entry::Info(format!("provider add {name}: api key")));
                    return;
                }
                send(
                    tx,
                    Command::AddProvider {
                        name: name.to_owned(),
                        base_url: url.to_owned(),
                        key: key.to_owned(),
                    },
                );
            }
            ("rm", name) if !name.is_empty() => send(tx, Command::RemoveProvider(name.to_owned())),
            _ => self.entries.push(Entry::Error(
                "usage: /provider [add [name [url [key]]] | rm <name>]".into(),
            )),
        }
    }

    /// `/secrets` menu | `add [name]` | `rm <name>` | `restore <name> [n]` (37 §5).
    fn secrets_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.awaiting_secrets = true;
                send(tx, Command::ListSecrets);
            }
            ("add", "") => self.prompt = Some(Prompt::SecretName),
            ("add", name) if secrets::vault::valid_name(name) => {
                self.prompt = Some(Prompt::SecretValue {
                    name: name.to_owned(),
                    current: self.secret_current(name),
                });
            }
            ("add", name) => self.entries.push(Entry::Error(bad_secret_name(name))),
            ("rm", name) if !name.is_empty() => send(tx, Command::DeleteSecret(name.to_owned())),
            ("restore", rest) if !rest.is_empty() => {
                let (name, back) = rest
                    .split_once(char::is_whitespace)
                    .map_or((rest, "1"), |(n, b)| (n, b.trim()));
                match back.parse::<u8>() {
                    Ok(back) if (1..secrets::KEEP as u8).contains(&back) => send(
                        tx,
                        Command::RestoreSecret {
                            name: name.to_owned(),
                            back,
                        },
                    ),
                    _ => self.entries.push(Entry::Error(format!(
                        "/secrets restore: n is 1..={}",
                        secrets::KEEP - 1
                    ))),
                }
            }
            _ => self.entries.push(Entry::Error(
                "usage: /secrets [add [name] | rm <name> | restore <name> [n]]".into(),
            )),
        }
    }

    /// Current version of `name` per the last `list_secrets`; `None` when
    /// unknown (or never listed).
    fn secret_current(&self, name: &str) -> Option<u32> {
        self.secrets
            .as_ref()?
            .secrets
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.current)
    }

    /// `/secrets` menu title: the key store, then the keys.
    pub fn secrets_title(&self) -> String {
        let key = self.secrets.as_ref().map_or("?", |l| l.key_store.as_str());
        match &self.menu {
            Some(Menu::Secrets {
                cursor,
                history: Some(_),
            }) => {
                let name = self
                    .secrets
                    .as_ref()
                    .and_then(|l| l.secrets.get(*cursor))
                    .map_or("", |s| s.name.as_str());
                format!("secrets · {name} · history   ⏎ restore  h/esc back")
            }
            _ => format!("secrets · key: {key}   a add  r replace  h history  d delete  esc"),
        }
    }

    /// `/secrets` menu rows: `NAME · vN (k kept) · note · updated`, or the
    /// highlighted secret's versions `vN · created · source [· current]`
    /// while its history is open. Never a value.
    pub fn secrets_rows(&self) -> Vec<String> {
        let list = self
            .secrets
            .as_ref()
            .map(|l| l.secrets.as_slice())
            .unwrap_or_default();
        if let Some(Menu::Secrets {
            cursor,
            history: Some(_),
        }) = &self.menu
            && let Some(s) = list.get(*cursor)
        {
            return s
                .versions
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    format!(
                        "v{} · {} · {}{}",
                        v.v,
                        when(&v.created),
                        v.source,
                        if i == 0 { " · current" } else { "" }
                    )
                })
                .collect();
        }
        if list.is_empty() {
            return vec!["no secrets · a adds one".into()];
        }
        list.iter()
            .map(|s| {
                format!(
                    "{:<20} · v{} ({} kept) · {} · {}",
                    s.name.chars().take(20).collect::<String>(),
                    s.current,
                    s.versions.len(),
                    s.note,
                    when(&s.updated)
                )
            })
            .collect()
    }

    /// `/secrets`: `a` adds, `r` replaces the highlighted value, `h` opens its
    /// history (⏎ on an older version restores it), `d` deletes after a
    /// typed confirm. Esc leaves the history, then the menu.
    fn secrets_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Secrets { cursor, history }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        // Field access, not a method: `cursor`/`history` hold `self.menu`.
        let list = self
            .secrets
            .as_ref()
            .map(|l| l.secrets.as_slice())
            .unwrap_or_default();
        let info = list.get(*cursor);
        let name = info.map(|s| s.name.clone());
        let current = info.map(|s| s.current);
        let versions = info.map_or(0, |s| s.versions.len());
        match (code, *history) {
            (KeyCode::Up, Some(h)) => *history = Some(h.saturating_sub(1)),
            (KeyCode::Down, Some(h)) => *history = Some((h + 1).min(versions.saturating_sub(1))),
            (KeyCode::Esc | KeyCode::Char('h'), Some(_)) => *history = None,
            (KeyCode::Enter, Some(h)) if h > 0 => {
                if let (Some(name), Ok(back)) = (name, u8::try_from(h)) {
                    send(tx, Command::RestoreSecret { name, back });
                }
            }
            (KeyCode::Up, None) => *cursor = cursor.saturating_sub(1),
            (KeyCode::Down, None) => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            (KeyCode::Esc, None) => self.menu = None,
            (KeyCode::Char('h'), None) if info.is_some() => *history = Some(0),
            (KeyCode::Char('a'), _) => {
                self.menu = None;
                self.prompt = Some(Prompt::SecretName);
            }
            (KeyCode::Char('r'), _) => {
                if let Some(name) = name {
                    self.menu = None;
                    self.prompt = Some(Prompt::SecretValue { name, current });
                }
            }
            (KeyCode::Char('d'), _) => {
                if let Some(name) = name {
                    self.menu = None;
                    self.prompt = Some(Prompt::SecretDelete { name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/knowledge`: `⏎` / `h` opens the highlighted concept's history, `H`
    /// the whole bundle's (deleted concepts included).
    fn knowledge_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Knowledge { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter | KeyCode::Char('h') => {
                if let Some(id) = items.get(*cursor).map(|m| m.id.clone()) {
                    self.open_history(Some(id), HISTORY_PAGE, tx);
                }
            }
            KeyCode::Char('H') => self.open_history(None, HISTORY_PAGE, tx),
            _ => {}
        }
        MenuKey::Consumed
    }

    /// Ask for `limit` commits of `id`'s history and show the menu waiting
    /// for them; the same scope keeps its rows and highlight while the next
    /// page loads. Only the reply for this `id` / `limit` fills it.
    fn open_history(&mut self, id: Option<String>, limit: usize, tx: &UnboundedSender<Command>) {
        match &mut self.menu {
            Some(Menu::KnowledgeHistory {
                id: open,
                limit: page,
                loading,
                pending,
                error,
                revision,
                ..
            }) if *open == id => {
                *page = limit;
                *loading = true;
                *pending = None;
                *error = None;
                *revision = None;
            }
            _ => {
                self.menu = Some(Menu::KnowledgeHistory {
                    id: id.clone(),
                    changes: Vec::new(),
                    limit,
                    more: false,
                    cursor: 0,
                    loading: true,
                    pending: None,
                    error: None,
                    revision: None,
                });
            }
        }
        send(tx, Command::KnowledgeHistory { id, limit });
    }

    /// `/knowledge history`: `⏎` on a commit fetches its diff (`↑↓` / page
    /// keys scroll rows, `←→` columns; esc / `h` / `⌫` go back), `⏎` on the
    /// `older` row asks for the next page, `r` re-asks for the current one.
    /// Moving drops a pending fetch. Esc on the list closes.
    fn knowledge_history_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::KnowledgeHistory {
            id,
            changes,
            limit,
            more,
            cursor,
            pending,
            revision,
            ..
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        if let Some((rev, row, col)) = revision {
            // One header row per file plus its diff lines; `revision_rows` shape.
            let last = rev
                .files
                .iter()
                .map(|f| 1 + f.diff.lines().count())
                .sum::<usize>()
                .saturating_sub(1);
            match code {
                KeyCode::Up => *row = row.saturating_sub(1),
                KeyCode::Down => *row = (*row + 1).min(last),
                KeyCode::PageUp => *row = row.saturating_sub(HISTORY_PAGE),
                KeyCode::PageDown => *row = (*row + HISTORY_PAGE).min(last),
                KeyCode::Left => *col = col.saturating_sub(DIFF_STEP),
                KeyCode::Right => *col += DIFF_STEP,
                KeyCode::Esc | KeyCode::Char('h') | KeyCode::Backspace => *revision = None,
                _ => {}
            }
            return MenuKey::Consumed;
        }
        let rows = changes.len() + usize::from(*more);
        match code {
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                *pending = None;
            }
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(rows.saturating_sub(1));
                *pending = None;
            }
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('r') => {
                let (id, limit) = (id.clone(), *limit);
                self.open_history(id, limit, tx);
            }
            KeyCode::Enter => match changes.get(*cursor) {
                Some(c) => {
                    *pending = Some(c.commit.clone());
                    send(
                        tx,
                        Command::KnowledgeRevision {
                            commit: c.commit.clone(),
                            id: id.clone(),
                            card: false,
                        },
                    );
                }
                None if *more => {
                    let (id, limit) = (id.clone(), *limit + HISTORY_PAGE);
                    self.open_history(id, limit, tx);
                }
                None => {}
            },
            _ => {}
        }
        MenuKey::Consumed
    }

    /// The `/knowledge history` menu: `(title, rows, cursor)`. The commit
    /// list (plus an `older` row when a page remains; `loading…`, the error
    /// or `no history` in its place), or the highlighted commit's diff with
    /// the cursor as its first shown row and `col` leading chars dropped.
    pub fn knowledge_history_menu(&self) -> (String, Vec<String>, usize) {
        let Some(Menu::KnowledgeHistory {
            id,
            changes,
            more,
            cursor,
            loading,
            pending,
            error,
            revision,
            ..
        }) = &self.menu
        else {
            return (String::new(), Vec::new(), 0);
        };
        let scope = id.as_deref().unwrap_or("all concepts");
        if let Some((rev, row, col)) = revision {
            let parent = rev
                .parent
                .as_deref()
                .map_or("none (root)", |p| p.get(..7).unwrap_or(p));
            let title = format!(
                "knowledge · {scope} · {} · {} · {} · parent {parent} · {} file{}   ↑↓←→ scroll  esc back",
                rev.change.commit,
                rev.change.time,
                rev.change.message,
                rev.files.len(),
                if rev.files.len() == 1 { "" } else { "s" },
            );
            let rows = revision_rows(rev)
                .into_iter()
                .map(|r| {
                    if *col == 0 {
                        r
                    } else {
                        r.chars().skip(*col).collect()
                    }
                })
                .collect();
            return (title, rows, *row);
        }
        let state = match (*loading, pending.as_deref(), error.as_deref()) {
            (_, _, Some(e)) => format!("   error: {e}"),
            (true, _, None) => "   loading…".to_owned(),
            (false, Some(c), None) => format!("   loading {c}…"),
            (false, None, None) => String::new(),
        };
        let title = format!("knowledge history · {scope}   ⏎ diff  r refresh  esc close{state}");
        let mut rows: Vec<String> = changes
            .iter()
            .map(|c| format!("{}  {}  {}", c.commit, c.time, c.message))
            .collect();
        if *more {
            rows.push(format!("… older  (⏎ loads {HISTORY_PAGE} more)"));
        }
        if rows.is_empty() {
            rows.push(match (*loading, error) {
                (_, Some(_)) => "request failed · r retries".into(),
                (true, None) => "loading…".into(),
                (false, None) => "no history · r refreshes".into(),
            });
        }
        (title, rows, *cursor)
    }

    /// `/shake [images|documents|messages|tools|summary]` (48 §2.1): ask the
    /// server for the whole active inventory; the reply opens the picker.
    /// An unknown word is usage, never chat input or a skill.
    fn shake_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let category = match arg {
            "" => None,
            a => match ShakeCategory::parse(a) {
                Some(c) => Some(c),
                None => {
                    self.entries.push(Entry::Error(SHAKE_USAGE.into()));
                    return;
                }
            },
        };
        match self.session {
            Some(session) => send(tx, Command::ContextItems { session, category }),
            None => self.entries.push(Entry::Info("no session".into())),
        }
    }

    /// Open the `/shake` picker over `snapshot`: every item for the bare
    /// command and `messages` with nothing ticked; a category's items ticked
    /// and straight into review otherwise. A refusal (busy, external
    /// harness) and an empty match are one notice each; nothing is written.
    fn open_shake(
        &mut self,
        session: SessionId,
        category: Option<ShakeCategory>,
        snapshot: ContextSnapshot,
    ) {
        if self.session != Some(session) {
            return;
        }
        self.context_revision = snapshot.revision.clone();
        self.context_editable = snapshot.editable;
        if !snapshot.editable {
            self.context_reason = snapshot.reason.clone();
            self.entries.push(Entry::Info(format!(
                "shake: {}",
                snapshot
                    .reason
                    .as_deref()
                    .unwrap_or("context is not editable")
            )));
            return;
        }
        let shown: Vec<usize> = (0..snapshot.items.len())
            .filter(|&i| category.is_none_or(|c| snapshot.items[i].kind == c.kind()))
            .collect();
        if shown.is_empty() {
            self.entries.push(Entry::Info(match category {
                Some(c) => format!("no {} in current context", c.noun()),
                None => "nothing in current context".into(),
            }));
            return;
        }
        let selected: BTreeSet<usize> = match category {
            None | Some(ShakeCategory::Messages) => BTreeSet::new(),
            Some(_) => shown.iter().copied().collect(),
        };
        let mut shake = Shake {
            session,
            revision: snapshot.revision,
            items: snapshot.items,
            shown,
            selected,
            cursor: 0,
            stage: ShakeStage::Pick,
            review: Vec::new(),
        };
        if !shake.selected.is_empty() {
            shake.open_review();
        }
        self.menu = Some(Menu::Shake(Box::new(shake)));
    }

    /// `/shake` keys: ↑↓ move, space ticks a picker row, ⏎ reviews then
    /// removes, esc backs out of review then closes. Nothing is sent
    /// before the review's ⏎.
    fn shake_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Shake(s)) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let len = match s.stage {
            ShakeStage::Pick => s.shown.len(),
            ShakeStage::Review => s.review.len(),
        };
        match (code, s.stage) {
            (KeyCode::Up, _) => s.cursor = s.cursor.saturating_sub(1),
            (KeyCode::Down, _) => s.cursor = (s.cursor + 1).min(len.saturating_sub(1)),
            (KeyCode::Char(' '), ShakeStage::Pick) => s.toggle(),
            (KeyCode::Enter, ShakeStage::Pick) => {
                if !s.selected.is_empty() {
                    s.open_review();
                }
            }
            (KeyCode::Esc, ShakeStage::Review) => {
                s.stage = ShakeStage::Pick;
                s.cursor = 0;
            }
            (KeyCode::Esc, ShakeStage::Pick) => self.menu = None,
            (KeyCode::Enter, ShakeStage::Review) => {
                let cmd = Command::RemoveContext {
                    session: s.session,
                    expected_revision: s.revision.clone(),
                    targets: s.review.iter().map(|r| r.target).collect(),
                };
                self.menu = None;
                send(tx, cmd);
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/broadcast [running|roots] [text]` (62 §5.1): bare opens the picker
    /// over `list_activity()`; with text the dry run goes straight to review
    /// (`running` when no selector is named).
    fn broadcast_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        if arg.is_empty() {
            send(tx, Command::ListActivity);
            return;
        }
        let (word, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(w, r)| (w, r.trim()));
        let (targets, text) = match word {
            "running" => (BroadcastTargets::Running, rest),
            "roots" => (BroadcastTargets::Roots, rest),
            _ => (BroadcastTargets::Running, arg),
        };
        if text.is_empty() {
            self.entries.push(Entry::Error(
                "usage: /broadcast [running|roots] [text]".into(),
            ));
            return;
        }
        let b = Broadcast::review(targets, text.to_owned());
        send(tx, Command::Broadcast(b.request(true)));
        self.menu = Some(Menu::Broadcast(Box::new(b)));
    }

    /// A `broadcast` reply: a dry run fills the review; a send closes the
    /// menu and prints the receipt; a refusal stays on the menu's error line.
    fn broadcasted(&mut self, r: Result<BroadcastReceipt, String>) {
        let Some(Menu::Broadcast(b)) = &mut self.menu else {
            return;
        };
        match (r, b.stage, b.preview.is_some()) {
            (Err(e), _, _) => b.error = Some(e),
            (Ok(receipt), BroadcastStage::Review, true) => {
                self.menu = None;
                self.entries.extend(
                    crate::broadcast::result_lines(&receipt)
                        .into_iter()
                        .map(Entry::Info),
                );
            }
            (Ok(receipt), _, _) => {
                b.preview = Some(receipt);
                b.error = None;
                b.stage = BroadcastStage::Review;
                b.cursor = 0;
            }
        }
    }

    /// `/broadcast` keys: Pick ticks (`space`, `r` running roots, `a` all
    /// roots, `n` none, `d` delivery) and ⏎ composes; Compose edits the line
    /// and ⏎ asks for the dry run; Review's ⏎ sends. Esc backs one stage.
    fn broadcast_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Broadcast(b)) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let len = match b.stage {
            BroadcastStage::Pick => b.rows.len(),
            BroadcastStage::Compose => 0,
            BroadcastStage::Review => b.preview.as_ref().map_or(0, |r| r.rows.len() + 2),
        };
        match (code, b.stage) {
            (KeyCode::Up, _) => b.cursor = b.cursor.saturating_sub(1),
            (KeyCode::Down, _) => b.cursor = (b.cursor + 1).min(len.saturating_sub(1)),
            (KeyCode::Char(' '), BroadcastStage::Pick) => b.toggle(),
            (KeyCode::Char('r'), BroadcastStage::Pick) => {
                b.tick_roots(|a| crate::broadcast::running(&a.activity));
            }
            (KeyCode::Char('a'), BroadcastStage::Pick) => b.tick_roots(|_| true),
            (KeyCode::Char('n'), BroadcastStage::Pick) => b.selected.clear(),
            (KeyCode::Char('d'), BroadcastStage::Pick) => b.toggle_delivery(),
            (KeyCode::Enter, BroadcastStage::Pick) => {
                if !b.selected.is_empty() {
                    b.stage = BroadcastStage::Compose;
                    b.error = None;
                }
            }
            (KeyCode::Esc, BroadcastStage::Pick) => self.menu = None,
            (KeyCode::Char(c), BroadcastStage::Compose) => b.text.push(c),
            (KeyCode::Backspace, BroadcastStage::Compose) => {
                b.text.pop();
            }
            (KeyCode::Enter, BroadcastStage::Compose) => {
                if !b.text.trim().is_empty() {
                    b.select_ticked();
                    b.error = None;
                    send(tx, Command::Broadcast(b.request(true)));
                }
            }
            (KeyCode::Esc, BroadcastStage::Compose) => {
                b.stage = BroadcastStage::Pick;
                b.cursor = 0;
            }
            (KeyCode::Enter, BroadcastStage::Review) => {
                if b.preview.is_some() {
                    b.error = None;
                    send(tx, Command::Broadcast(b.request(false)));
                }
            }
            (KeyCode::Esc, BroadcastStage::Review) => {
                if b.picked() {
                    b.stage = BroadcastStage::Compose;
                    b.preview = None;
                    b.cursor = 0;
                } else {
                    self.menu = None;
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/compact [summary|cut|snap|style …]`.
    fn compact_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => match self.session {
                Some(session) => send(
                    tx,
                    Command::Compact {
                        session,
                        style: None,
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            (token, _) if token.parse::<CompactStyle>().is_ok() => {
                let style = token.parse().expect("guard");
                match self.session {
                    Some(session) => send(
                        tx,
                        Command::Compact {
                            session,
                            style: Some(style),
                        },
                    ),
                    None => self.entries.push(Entry::Info("no session".into())),
                }
            }
            ("style", "") => send(tx, Command::GetCompactStyle),
            ("style", rest) if rest.parse::<CompactStyle>().is_ok() => {
                let style = rest.parse().expect("guard");
                send(tx, Command::SetCompactStyle(style));
            }
            _ => self.entries.push(Entry::Error(
                "usage: /compact [summary|cut|snap|style …]".into(),
            )),
        }
    }

    /// `/endless [on|off]` (137 §8.6): switch the session's memory mode, or show it.
    fn endless_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info("no session".into()));
            return;
        };
        let mode = match arg.trim() {
            "" | "status" => None,
            "on" => Some(MemoryMode::Endless),
            "off" => Some(MemoryMode::Epochs),
            _ => {
                self.entries
                    .push(Entry::Error("usage: /endless [on|off]".into()));
                return;
            }
        };
        send(tx, Command::Memory { session, mode });
    }

    /// Advance a prompt flow one field. An API key or a secret's value is
    /// never written to the transcript.
    fn prompt_submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        match self.prompt.take() {
            Some(Prompt::ProviderName) => {
                self.prompt = Some(Prompt::ProviderUrl {
                    name: line.to_owned(),
                });
                self.entries.push(Entry::Info(format!("name: {line}")));
            }
            Some(Prompt::ProviderUrl { name }) => {
                self.prompt = Some(Prompt::ProviderKey {
                    name: name.clone(),
                    url: line.to_owned(),
                });
                self.entries.push(Entry::Info(format!("url: {line}")));
            }
            Some(Prompt::ProviderKey { name, url }) => {
                self.entries.push(Entry::Info("api key: set".into()));
                send(
                    tx,
                    Command::AddProvider {
                        name,
                        base_url: url,
                        key: line.to_owned(),
                    },
                );
            }
            Some(Prompt::LinkDeviceName { id, .. }) => {
                self.awaiting_link = true;
                send(tx, Command::LinkRenameDevice(id, line.to_owned()));
            }
            Some(Prompt::LinkName) => {
                self.awaiting_link = true;
                send(tx, Command::LinkRename(line.to_owned()));
            }
            Some(Prompt::LinkRevoke { id, name }) => {
                if line.eq_ignore_ascii_case("y") || line.eq_ignore_ascii_case("yes") {
                    self.awaiting_link = true;
                    send(tx, Command::LinkRevoke(id));
                } else {
                    self.entries.push(Entry::Info(format!("link: kept {name}")));
                }
            }
            Some(Prompt::NodeKey) => {
                self.awaiting_nodes = true;
                send(tx, Command::AddNode(line.to_owned(), None));
            }
            Some(Prompt::NodeName { id, .. }) => {
                self.awaiting_nodes = true;
                send(tx, Command::RenameNode(id, line.to_owned()));
            }
            Some(Prompt::NodeRemove { id, name }) => {
                if line.eq_ignore_ascii_case("y") || line.eq_ignore_ascii_case("yes") {
                    self.awaiting_nodes = true;
                    send(tx, Command::RemoveNode(id));
                } else {
                    self.entries
                        .push(Entry::Info(format!("nodes: kept {name}")));
                }
            }
            Some(Prompt::SecretName) => {
                if secrets::vault::valid_name(line) {
                    self.prompt = Some(Prompt::SecretValue {
                        name: line.to_owned(),
                        current: self.secret_current(line),
                    });
                } else {
                    self.prompt = Some(Prompt::SecretName);
                    self.entries.push(Entry::Error(bad_secret_name(line)));
                }
            }
            Some(Prompt::SecretValue { name, .. }) => {
                self.prompt = Some(Prompt::SecretNote {
                    name,
                    value: line.to_owned(),
                });
            }
            Some(Prompt::SecretNote { name, value }) => send(
                tx,
                Command::SetSecret {
                    name,
                    value,
                    note: (!line.is_empty()).then(|| line.to_owned()),
                },
            ),
            Some(Prompt::SecretDelete { name }) => {
                if line == name {
                    send(tx, Command::DeleteSecret(name));
                } else {
                    self.entries
                        .push(Entry::Info(format!("secrets: kept {name}")));
                }
            }
            Some(Prompt::PeerAnswer {
                session, reply_to, ..
            }) => send(
                tx,
                Command::AnswerPeerRequest {
                    session,
                    reply_to,
                    text: line.to_owned(),
                },
            ),
            None => {}
        }
    }

    /// Drop an in-progress prompt flow.
    pub fn cancel_prompt(&mut self) {
        if let Some(p) = self.prompt.take() {
            self.entries.push(Entry::Info(p.cancelled().into()));
        }
    }

    /// Open or close the slash popup to match what the input box holds.
    pub fn sync_slash(&mut self, input: &str) {
        if self.prompt.is_some() || self.ask().is_some() || self.secret_request().is_some() {
            return;
        }
        match (&self.menu, input.starts_with('/')) {
            (None, true) => self.menu = Some(Menu::Slash { cursor: 0 }),
            (Some(Menu::Slash { .. }), false) => self.menu = None,
            _ => {}
        }
    }

    /// `/model` picker rows (124 §3, 127 §3): the favorites present in the
    /// catalog, the current model unless it is one of them, then one row per
    /// provider; a provider's models by name; or, with a filter, the current
    /// model then the ranked results over every model (the current one
    /// removed so it shows once).
    pub fn models_rows(&self) -> Vec<ModelRow<'_>> {
        let Some(Menu::Models {
            items,
            filter,
            provider,
            target,
            ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let picked = self.pick_current();
        let current = items.iter().find(|m| Some(m.id.as_str()) == picked);
        let mut rows = Vec::new();
        // 125 §7.2: the advisor's picker leads with `Off` while browsing (either
        // level — a single provider skips the providers one), marked while there is none.
        if filter.is_empty() && *target == PickTarget::Advisor {
            rows.push(ModelRow::Off {
                current: picked.is_none(),
            });
        }
        if !filter.is_empty() {
            rows.extend(current.map(ModelRow::Current));
            rows.extend(
                rank_models(items, filter)
                    .into_iter()
                    .filter(|m| Some(m.id.as_str()) != picked)
                    .map(ModelRow::Model),
            );
        } else if let Some(p) = provider {
            rows.extend(provider_models(items, p).into_iter().map(ModelRow::Model));
        } else {
            rows.extend(
                self.favorite_models
                    .iter()
                    .filter_map(|f| items.iter().find(|m| m.id == *f))
                    .map(ModelRow::Favorite),
            );
            if !self.favorite_models.contains(&self.model_id) {
                rows.extend(current.map(ModelRow::Current));
            }
            let now = current.map(|c| c.provider.as_str());
            rows.extend(providers(items).into_iter().map(|(id, count)| {
                let current = now == Some(id.as_str());
                ModelRow::Provider { id, count, current }
            }));
        }
        rows
    }

    /// The model the open `/model` picker marks `●`: the session's, or the
    /// advisor's (`None` while off) when the picker targets it (125 §7.2).
    pub fn pick_current(&self) -> Option<&str> {
        match &self.menu {
            Some(Menu::Models {
                target: PickTarget::Advisor,
                ..
            }) => self.advisor.model.as_deref(),
            _ => Some(self.model_id.as_str()),
        }
    }

    /// Apply the engine's favorite list (127 §3); an open `/model` picker's
    /// cursor is clamped to the rows it now has.
    fn set_favorites(&mut self, models: Vec<String>) {
        self.favorite_models = models;
        let len = self.models_rows().len();
        if let Some(Menu::Models { cursor, .. }) = &mut self.menu {
            *cursor = (*cursor).min(len.saturating_sub(1));
        }
    }

    /// Put the `/model` cursor where a level starts: the first row after the
    /// Off and Current rows (first favorite, first provider, or best search hit)
    /// when there is one.
    fn models_home(&mut self) {
        let rows = self.models_rows();
        let lead = rows
            .iter()
            .take_while(|r| matches!(r, ModelRow::Off { .. } | ModelRow::Current(_)))
            .count();
        let home = if rows.len() > lead { lead } else { 0 };
        if let Some(Menu::Models { cursor, .. }) = &mut self.menu {
            *cursor = home;
        }
    }

    /// `/mcp` menu rows: every server, then the tools of expanded ones.
    pub fn mcp_rows(&self) -> Vec<Row<'_>> {
        let Some(Menu::Mcp {
            servers, expanded, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for s in servers {
            rows.push(Row::Server(s));
            if expanded.contains(&s.name) {
                rows.extend(s.tools.iter().map(|t| Row::Tool(s, t)));
            }
        }
        rows
    }

    /// Route a key to the open menu (no-op without one). Returns what the
    /// input box should do.
    pub fn menu_key(
        &mut self,
        code: KeyCode,
        input: &str,
        tx: &UnboundedSender<Command>,
    ) -> MenuKey {
        match &self.menu {
            None => MenuKey::Consumed,
            Some(Menu::Slash { .. }) => self.slash_key(code, input, tx),
            Some(Menu::Models { .. }) => self.models_key(code, tx),
            Some(Menu::Efforts { .. }) => self.efforts_key(code, tx),
            Some(Menu::AwsProfiles { .. }) => self.aws_key(code, tx),
            Some(Menu::Sessions { .. }) => self.sessions_key(code, tx),
            Some(Menu::Search | Menu::Attach) => MenuKey::Consumed,
            Some(Menu::Agents { .. }) => self.agents_key(code, tx),
            Some(Menu::Mcp { .. }) => self.mcp_key(code, tx),
            Some(Menu::Acp { .. }) => self.acp_key(code, tx),
            Some(Menu::Skills { .. }) => self.skills_key(code, tx),
            Some(Menu::Logins { .. }) => self.logins_key(code, tx),
            Some(Menu::Turns { .. }) => self.turns_key(code, tx),
            Some(Menu::Link { .. }) => self.link_key(code, tx),
            Some(Menu::Nodes { .. }) => self.nodes_key(code, tx),
            Some(Menu::Secrets { .. }) => self.secrets_key(code, tx),
            Some(Menu::Artifacts { .. }) => self.artifacts_key(code, tx),
            Some(Menu::Shake(_)) => self.shake_key(code, tx),
            Some(Menu::Broadcast(_)) => self.broadcast_key(code, tx),
            Some(Menu::Peers { .. }) => self.peers_key(code, tx),
            Some(Menu::Knowledge { .. }) => self.knowledge_key(code, tx),
            Some(Menu::KnowledgeHistory { .. }) => self.knowledge_history_key(code, tx),
            Some(Menu::Cost { .. }) => self.cost_key(code),
            Some(Menu::Terminals { .. }) => self.terminals_key(code, tx),
            Some(Menu::Terminal(_)) => self.terminal_key(code, tx),
        }
    }

    fn slash_key(&mut self, code: KeyCode, input: &str, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Slash { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let (word, arg) = input
            .split_once(' ')
            .map_or((input, ""), |(w, a)| (w, a.trim()));
        let matches = slash_matches(word, self.app_enabled);
        *cursor = (*cursor).min(matches.len().saturating_sub(1));
        let picked = matches.get(*cursor).copied();
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(matches.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Tab => {
                if let Some((name, hint, _)) = picked {
                    let tail = if arg.is_empty() && hint.is_empty() {
                        ""
                    } else {
                        " "
                    };
                    return MenuKey::Input(format!("{name}{tail}{arg}"));
                }
            }
            KeyCode::Enter => {
                let line = match picked {
                    Some((name, hint, _)) if hint.starts_with('<') && arg.is_empty() => {
                        return MenuKey::Input(format!("{name} "));
                    }
                    Some((name, _, _)) => format!("{name} {arg}"),
                    None => input.to_owned(),
                };
                self.menu = None;
                self.submit(&line, tx);
                return MenuKey::Input(String::new());
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/model` (124 §3, 127 §3): ↑↓ move; typing searches every model; ⏎/→
    /// on a provider opens it; ←, or ⌫ on an empty filter, returns to the
    /// providers level (no-op with one provider); ⏎ on a model sets it; ⇥ on
    /// a model stars or unstars it (the menu stays open); Esc closes.
    fn models_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Models {
            items,
            cursor,
            target,
            ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let target = *target;
        let rows = self.models_rows();
        let len = rows.len();
        let at = (*cursor).min(len.saturating_sub(1));
        // `(true, id)` opens provider `id`; `(false, Some(id))` picks model `id`;
        // `(false, None)` is the advisor's `Off` row.
        let picked = match rows.get(at) {
            Some(ModelRow::Current(m) | ModelRow::Favorite(m) | ModelRow::Model(m)) => {
                Some((false, Some(m.id.clone())))
            }
            Some(ModelRow::Provider { id, .. }) => Some((true, Some(id.clone()))),
            Some(ModelRow::Off { .. }) => Some((false, None)),
            None => None,
        };
        let many = providers(items).len() > 1;
        let Some(Menu::Models {
            filter,
            cursor,
            provider,
            ..
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        *cursor = at;
        let mut home = false;
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(len.saturating_sub(1)),
            KeyCode::Char(c) => {
                filter.push(c);
                home = true;
            }
            KeyCode::Backspace => {
                if filter.pop().is_some() {
                    home = true;
                } else if many && provider.is_some() {
                    *provider = None;
                    home = true;
                }
            }
            KeyCode::Left => {
                if filter.is_empty() && many && provider.is_some() {
                    *provider = None;
                    home = true;
                }
            }
            KeyCode::Right => {
                if let Some((true, Some(p))) = picked {
                    *provider = Some(p);
                    home = true;
                }
            }
            KeyCode::Enter => match (picked, target) {
                (Some((true, Some(p))), _) => {
                    *provider = Some(p);
                    home = true;
                }
                (Some((false, Some(model))), PickTarget::Session) => {
                    send(
                        tx,
                        Command::SetModel {
                            session: self.session,
                            model,
                        },
                    );
                    self.menu = None;
                }
                // 125 §7.2: a model or `Off` goes to `set_advisor`.
                (Some((false, model)), PickTarget::Advisor) => {
                    self.menu = None;
                    match self.session {
                        Some(s) => self.set_advisor(s, model, None, tx),
                        None => self
                            .entries
                            .push(Entry::Error("no session yet: send a message first".into())),
                    }
                }
                _ => {}
            },
            KeyCode::Esc => self.menu = None,
            KeyCode::Tab => {
                if let Some((false, Some(model))) = picked {
                    let favorite = !self.favorite_models.contains(&model);
                    send(tx, Command::SetModelFavorite { model, favorite });
                }
            }
            _ => {}
        }
        if home {
            self.models_home();
        }
        MenuKey::Consumed
    }

    /// `/effort`: Enter sets the highlighted level (`clear` unsets) from the next
    /// turn; targeted at the advisor (125 §7.2) it re-sends its model with the effort.
    fn efforts_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Efforts {
            items,
            cursor,
            target,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).cloned();
                let target = *target;
                self.menu = None;
                let Some(v) = picked else {
                    return MenuKey::Consumed;
                };
                let effort = (v != "clear").then_some(v);
                match (target, self.session, self.advisor.model.clone()) {
                    (PickTarget::Session, ..) => self.set_effort(effort, tx),
                    (PickTarget::Advisor, Some(session), model @ Some(_)) => {
                        self.advisor_reply = None;
                        send(
                            tx,
                            Command::SetAdvisor {
                                session,
                                model,
                                effort,
                            },
                        );
                    }
                    // The advisor went off while the picker was open: nothing to set.
                    (PickTarget::Advisor, ..) => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/aws`: Enter selects the highlighted profile for the whole harness;
    /// row 0 restores the AWS default credential chain.
    fn aws_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::AwsProfiles { info, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(info.profiles.len()),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = cursor.checked_sub(1).map(|i| info.profiles.get(i).cloned());
                self.menu = None;
                match picked {
                    None => send(tx, Command::SetAwsProfile(None)),
                    Some(Some(p)) => send(tx, Command::SetAwsProfile(Some(p))),
                    Some(None) => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/aws` picker rows: the default chain, then every SDK-discovered
    /// profile; `●` marks the selection in force.
    pub fn aws_rows(&self) -> Vec<String> {
        let Some(Menu::AwsProfiles { info, .. }) = &self.menu else {
            return Vec::new();
        };
        let mark = |on: bool| if on { "●" } else { " " };
        std::iter::once(format!(
            "{} default credential chain ({})",
            mark(info.selected.is_none()),
            info.default_profile
        ))
        .chain(
            info.profiles
                .iter()
                .map(|p| format!("{} {p}", mark(info.selected.as_deref() == Some(p.as_str())))),
        )
        .collect()
    }

    /// `/sessions`: Enter switches to the highlighted session. The one left
    /// behind keeps running in the agent; only what is shown changes.
    fn sessions_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Sessions { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = grouped_step(items, *cursor, -1),
            KeyCode::Down => *cursor = grouped_step(items, *cursor, 1),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).and_then(|r| r.item()).map(|(s, _)| s.id);
                self.menu = None;
                match picked {
                    Some(id) if Some(id) == self.session => {}
                    Some(id) => self.resume(id, tx),
                    None => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/edit`, `/regen`, `/fork` picker: Enter applies the action to the
    /// highlighted turn.
    fn turns_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Turns {
            items,
            cursor,
            action,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let action = *action;
                let picked = items.get(*cursor).map(|&(idx, turn, _)| (idx, turn));
                self.menu = None;
                if let Some((idx, turn)) = picked {
                    self.turn_action(action, idx, turn, tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/search [query]` (49 §6): open the live menu, a fresh view with the
    /// remembered chips; a prefilled query runs at once. Any preview or
    /// inspection under way is left.
    pub fn open_search(&mut self, query: &str, tx: &UnboundedSender<Command>) {
        self.inspect = None;
        self.menu = Some(Menu::Search);
        for r in self.search.open(query) {
            send(tx, Command::Search(r));
        }
    }

    /// `/attach [query]` (66 §10): the search menu in pick mode — Enter on a
    /// row holds it as a [`Paste::Ref`] and puts its token in the input box.
    pub fn open_attach(&mut self, query: &str, tx: &UnboundedSender<Command>) {
        self.inspect = None;
        self.menu = Some(Menu::Attach);
        for r in self.search.open(query) {
            send(tx, Command::Search(r));
        }
    }

    /// Hold `r` for the next submit; returns its [`paste_token`]. Attaching
    /// the same kind+id twice is a no-op that returns the existing token.
    pub fn attach_ref(&mut self, r: agent::attach::AttachRef) -> String {
        if let Some((i, p)) = self
            .pastes
            .iter()
            .enumerate()
            .find(|(_, p)| matches!(p, Paste::Ref(x) if x.kind == r.kind && x.id == r.id))
        {
            return paste_token(i, p);
        }
        let p = Paste::Ref(r);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }

    /// The search editor changed (49 §6.1): the debounce starts.
    pub fn search_edit(&mut self, text: String) {
        if self.search_open() {
            self.search.edit(text, now_ms());
        }
    }

    /// When the event loop must next wake for search (unix ms); `None` idle.
    pub fn search_wake(&self) -> Option<u64> {
        self.search_open().then(|| self.search.wake()).flatten()
    }

    /// Send whatever search request is due at `now`, and anything a reply
    /// queued meanwhile.
    pub fn search_tick(&mut self, now: u64, tx: &UnboundedSender<Command>) {
        self.search_flush(tx);
        if !self.search_open() {
            return;
        }
        for r in self.search.tick(now) {
            send(tx, Command::Search(r));
        }
    }

    /// A source changed: an open search re-asks, a closed one on restore.
    fn search_invalidate(&mut self, category: SearchCategory) {
        let open = self.search_open();
        self.search.invalidate(category, open, now_ms());
    }

    /// A tagged search reply; stale generations are inert inside.
    fn search_reply(&mut self, generation: u64, reply: search::Reply) {
        if !self.search_open() {
            return;
        }
        match self.search.reply(generation, reply, now_ms()) {
            search::Effect::None => {}
            search::Effect::Send(reqs) => {
                // Only the refresh reply sends from here; it needs the sender,
                // which `apply` lacks. Queue through the deadline instead.
                self.search_queue(reqs);
            }
            search::Effect::Artifact(load) => {
                let opts = self.assistant_options();
                let body = artifact_text(
                    &load.artifact,
                    load.bytes.as_deref().map_err(String::as_str),
                    &opts,
                );
                self.search.previews.push(search::Preview::Artifact {
                    artifact: Box::new(load.artifact),
                    body: Box::new(body),
                    matched: load.matched,
                    panel: false,
                    scroll: 0,
                });
            }
            search::Effect::Inspect {
                session,
                seq,
                loaded,
            } => self.inspect_page(session, seq, loaded),
        }
    }

    /// Requests produced while no sender was at hand: held until the next
    /// [`Model::search_tick`] / key.
    fn search_queue(&mut self, reqs: Vec<search::Request>) {
        self.search_outbox.extend(reqs);
    }

    /// Flush [`Model::search_queue`]'d requests.
    pub fn search_flush(&mut self, tx: &UnboundedSender<Command>) {
        for r in std::mem::take(&mut self.search_outbox) {
            send(tx, Command::Search(r));
        }
    }

    /// Keys while the `/search` menu is up (49 §6.1). `area` is the rows the
    /// result list gets and `width` the box's inner columns, for paging and
    /// preview clamping. Query-focus editing keys are the search editor's
    /// (its own `TextInput`); this sees the rest.
    pub fn search_key(
        &mut self,
        code: KeyCode,
        width: usize,
        area: usize,
        tx: &UnboundedSender<Command>,
    ) {
        use crate::search::{Act, FOOTER, Focus};
        let compact = area < 4;
        // A preview owns the keys while up; Esc pops it.
        if let Some(act) = self.search.preview_key(code, width, area) {
            match code {
                KeyCode::Char('o') => {
                    if let Some(a) = self.search.preview_artifact() {
                        send(tx, Command::OpenPath(a.path.clone()));
                    }
                }
                KeyCode::Char('e') => {
                    if let Some(a) = self.search.preview_artifact() {
                        send(
                            tx,
                            Command::ExportArtifact {
                                id: a.id.clone(),
                                name: a.name.clone(),
                            },
                        );
                    }
                }
                _ => {}
            }
            self.search_act(act, tx);
            return;
        }
        let focus = self.search.focus;
        let act = match (code, focus) {
            (KeyCode::Esc, _) => {
                self.menu = None;
                self.search.close();
                return;
            }
            (KeyCode::Tab, Focus::Query) => {
                self.search.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Filters(_)) => {
                self.search.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Tab, Focus::Results) => {
                self.search.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Footer(_)) => {
                self.search.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Query) => {
                self.search.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Filters(_)) => {
                self.search.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Results) => {
                self.search.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Footer(_)) => {
                self.search.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Up, _) => {
                self.search.move_cursor(-1);
                Act::None
            }
            (KeyCode::Down, _) => {
                self.search.move_cursor(1);
                Act::None
            }
            (KeyCode::PageUp, _) => {
                self.search.move_cursor(-(area.max(1) as isize));
                Act::None
            }
            (KeyCode::PageDown, _) => {
                self.search.move_cursor(area.max(1) as isize);
                Act::None
            }
            (KeyCode::Left, Focus::Filters(i)) => {
                self.search.focus = Focus::Filters(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Filters(i)) => {
                self.search.focus = Focus::Filters((i + 1).min(crate::search::N));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Filters(i)) => {
                Act::Send(self.search.toggle(i))
            }
            (KeyCode::Left, Focus::Footer(i)) => {
                self.search.focus = Focus::Footer(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Footer(i)) => {
                self.search.focus = Focus::Footer((i + 1).min(FOOTER.len() - 1));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(0)) => {
                Act::Send(self.search.refresh())
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(_)) => {
                self.search.previews.push(search::Preview::Help);
                Act::None
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) if self.attach_open() => {
                match self.search.pick() {
                    Some(r) => {
                        self.attach_token = Some(self.attach_ref(r));
                        self.menu = None;
                        self.search.close();
                        Act::None
                    }
                    // A `show more` row or nothing selectable: the ordinary key.
                    None => self.search.enter(),
                }
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) => self.search.enter(),
            _ => Act::None,
        };
        self.search.ensure_visible(area, compact);
        self.search_act(act, tx);
    }

    /// Carry out what a search key asked.
    fn search_act(&mut self, act: search::Act, tx: &UnboundedSender<Command>) {
        self.search_flush(tx);
        match act {
            search::Act::None => {}
            search::Act::Send(reqs) => {
                for r in reqs {
                    send(tx, Command::Search(r));
                }
            }
            // A title hit or a recent row: an ordinary session switch (49 §7).
            // The current session needs no reload; the menu just closes.
            search::Act::Resume(id) => {
                self.menu = None;
                self.search.close();
                if Some(id) != self.session || self.inspect.is_some() {
                    self.resume(id, tx);
                }
            }
        }
    }

    /// Show the page ending at `seq` as the **Earlier messages** inspection
    /// (49 §7): the session's rows up to the match, the match revealed at the
    /// top (a removed row as its archive card), live events kept out until
    /// **Jump to latest** loads the normal tail. Nothing here enters the
    /// model's context.
    fn inspect_page(&mut self, session: SessionId, seq: i64, loaded: Loaded) {
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
        self.reset();
        self.approvals = approvals;
        self.asks = asks;
        self.session = Some(session);
        let opts = self.assistant_options();
        let mut target = None;
        let mut entries = Vec::new();
        for h in loaded.history {
            let here = h
                .context()
                .is_some_and(|c| c.target == ContextTarget::Message { seq });
            if here {
                target = Some(entries.len());
                entries.push(Entry::Info(format!("▸ matched message · seq {seq}")));
            }
            entries.extend(from_history(h, &opts));
        }
        if target.is_none() {
            entries.insert(
                0,
                Entry::Info(format!(
                    "message {seq} is not on this page (removed or hidden); showing the messages around it"
                )),
            );
        }
        self.entries = entries;
        self.anchor = Some((target.unwrap_or(0), 0));
        self.model_id = loaded.model;
        self.harness_model = loaded.harness_model;
        self.harness_commands = loaded.harness_commands;
        self.context_size = loaded.context_size;
        self.agent = loaded.agent;
        self.effort = loaded.effort;
        self.session_name = loaded.name;
        self.session_parent = loaded.parent;
        self.streaming = loaded.running;
        self.inspect = Some(search::Inspect {
            session,
            seq,
            found: target.is_some(),
        });
    }

    /// **Jump to latest** from the inspection: the normal tail, live again.
    pub fn jump_latest(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(i) = self.inspect.take() {
            self.resume(i.session, tx);
        }
    }

    /// **Back to results** from the inspection: the normal tail loads behind
    /// the reopened menu, which shows the view as left.
    pub fn back_to_results(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(i) = self.inspect.take() {
            self.resume(i.session, tx);
        }
        self.menu = Some(Menu::Search);
        for r in self.search.restore() {
            send(tx, Command::Search(r));
        }
    }

    /// The inspection's hint row (49 §7).
    pub fn inspect_line(&self) -> Option<String> {
        let i = self.inspect.as_ref()?;
        Some(format!(
            "earlier messages · {} · message {}{}   ↑↓ scroll  ⏎/j jump to latest  esc/b back to results",
            short(i.session),
            i.seq,
            if i.found { "" } else { " (not on this page)" }
        ))
    }

    /// `/agents`: Enter opens the highlighted subagent — a session switch,
    /// the same path as `/sessions`; the one left keeps running.
    fn agents_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Agents { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).map(|a| a.info.id);
                self.menu = None;
                if let Some(id) = picked {
                    self.resume(id, tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/artifacts` and `/apps`: Enter opens the highlighted file with the OS,
    /// `s` shows it inline as a card (an app's read-only source), `e` exports
    /// it to `~/Downloads/<name>` (35 §6.2, 46 §8.2).
    fn artifacts_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Artifacts { items, cursor, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = grouped_step(items, *cursor, -1),
            KeyCode::Down => *cursor = grouped_step(items, *cursor, 1),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(a) = items.get(*cursor).and_then(Grouped::item) {
                    send(tx, Command::OpenPath(a.path.clone()));
                }
                self.menu = None;
            }
            KeyCode::Char('e') => {
                if let Some(a) = items.get(*cursor).and_then(Grouped::item) {
                    send(
                        tx,
                        Command::ExportArtifact {
                            id: a.id.clone(),
                            name: a.name.clone(),
                        },
                    );
                }
                self.menu = None;
            }
            KeyCode::Char('s') => {
                let picked = items.get(*cursor).and_then(Grouped::item).cloned();
                self.menu = None;
                if let Some(a) = picked {
                    let text = artifact_body(&a, &self.assistant_options());
                    self.entries.push(Entry::Card {
                        kind: CardKind::Artifact,
                        title: a.name,
                        text: Box::new(text),
                        expanded: true,
                    });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/skills`: Enter hands the highlighted skill to the model (04 §2.4);
    /// `s` shows its body as a card to review first.
    fn skills_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Skills { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('s') => {
                if let Some(s) = items.get(*cursor) {
                    send(tx, Command::LoadSkill(s.name.clone()));
                }
                self.menu = None;
            }
            KeyCode::Enter => {
                let name = items.get(*cursor).map(|s| s.name.clone());
                self.menu = None;
                if let Some(name) = name {
                    self.use_skill(name, String::new(), tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// Hand a skill to the model: the agent composes the input and delivers
    /// it like typed text (turn when idle, steer when running); the cards
    /// arrive with `TurnStarted` / `InputInjected`.
    fn use_skill(&mut self, name: String, args: String, tx: &UnboundedSender<Command>) {
        self.awaiting_session = self.session.is_none();
        send(
            tx,
            Command::UseSkill {
                session: self.session,
                name,
                args,
            },
        );
    }

    /// `/login`: Enter logs the highlighted row in.
    fn logins_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Logins { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(r) = items.get(*cursor) {
                    send(tx, Command::Login(r.name.clone()));
                }
                self.menu = None;
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    fn mcp_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Expand(String, bool),
            Send(Command),
            Pending(Option<Target>),
            Close,
            Nothing,
        }
        let Some(Menu::Mcp {
            cursor, pending, ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let session = self.session;
        let rows = self.mcp_rows();
        let cursor = (*cursor).min(rows.len().saturating_sub(1));
        let row = rows.get(cursor);
        let set_gate = |row: &Row<'_>, gate: Option<Gate>| {
            let t = row.target();
            Act::Send(Command::SetGate {
                server: t.server,
                tool: t.tool,
                gate,
                session,
            })
        };
        let set_hidden = |row: &Row<'_>, hidden: bool| {
            Act::Send(Command::SetHidden {
                session,
                tools: row.tool_names(),
                hidden,
            })
        };
        // The scope prompt after ⏎ on an `on` row: the target is the row the
        // cursor was on, re-found by name (a refresh may have moved it).
        let act = if let Some(target) = pending {
            let row = rows.iter().find(|r| r.target() == *target);
            match (code, row) {
                (KeyCode::Esc, _) => Act::Pending(None),
                (KeyCode::Char('s') | KeyCode::Enter, Some(r)) => set_hidden(r, true),
                (KeyCode::Char('a'), Some(r)) => set_gate(r, Some(Gate::Off)),
                (_, None) => Act::Pending(None),
                _ => Act::Nothing,
            }
        } else {
            match (code, row) {
                (KeyCode::Up, _) => Act::Move(cursor.saturating_sub(1)),
                (KeyCode::Down, _) => Act::Move((cursor + 1).min(rows.len().saturating_sub(1))),
                (KeyCode::Esc, _) => Act::Close,
                (KeyCode::Char('s'), _) => Act::Send(Command::SyncMcp),
                (KeyCode::Right, Some(Row::Server(s))) => Act::Expand(s.name.clone(), true),
                (KeyCode::Left, Some(Row::Server(s) | Row::Tool(s, _))) => {
                    Act::Expand(s.name.clone(), false)
                }
                (KeyCode::Char('l'), Some(Row::Server(s))) => match s.auth {
                    McpAuth::Required => Act::Send(Command::Login(s.name.clone())),
                    McpAuth::LoggedIn => Act::Send(Command::Logout(s.name.clone())),
                    McpAuth::None => Act::Nothing,
                },
                // ⏎: turn on undoes whichever switch is off; turn off asks for the scope.
                (KeyCode::Enter, Some(r)) => match r.switch() {
                    Switch::OffAlways => set_gate(r, None),
                    Switch::OffSession => set_hidden(r, false),
                    Switch::On => Act::Pending(Some(r.target())),
                },
                (KeyCode::Char(' ') | KeyCode::Backspace, Some(r))
                    if r.switch() == Switch::OffAlways =>
                {
                    Act::Nothing
                }
                (KeyCode::Char(' '), Some(r)) => set_gate(r, next_gate(r.gate())),
                (KeyCode::Backspace, Some(r)) => set_gate(r, None),
                _ => Act::Nothing,
            }
        };
        let Some(Menu::Mcp {
            servers,
            cursor: cur,
            expanded,
            pending,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Expand(name, true) => {
                expanded.insert(name);
            }
            Act::Expand(name, false) => {
                expanded.remove(&name);
                // Land on the collapsed server's own row.
                *cur = servers.iter().position(|s| s.name == name).map_or(0, |i| {
                    servers[..i]
                        .iter()
                        .filter(|s| expanded.contains(&s.name))
                        .map(|s| s.tools.len())
                        .sum::<usize>()
                        + i
                });
            }
            Act::Send(cmd) => {
                *pending = None;
                send(tx, cmd);
            }
            Act::Pending(p) => *pending = p,
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
        MenuKey::Consumed
    }

    /// `/acp` menu rows in cursor order: entries, a separator when both
    /// halves are present, then the registry.
    pub fn acp_rows(&self) -> Vec<AcpRow<'_>> {
        let Some(Menu::Acp {
            entries, registry, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows: Vec<AcpRow<'_>> = entries.iter().map(AcpRow::Entry).collect();
        if !entries.is_empty() && !registry.is_empty() {
            rows.push(AcpRow::Sep);
        }
        rows.extend(registry.iter().map(AcpRow::Registry));
        rows
    }

    fn acp_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Send(Command),
            Probe,
            Close,
            Nothing,
        }
        let Some(Menu::Acp { cursor, .. }) = &self.menu else {
            return MenuKey::Consumed;
        };
        let rows = self.acp_rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        // Up/Down step over the separator row.
        let step = |to: usize, dir: isize| match rows.get(to) {
            Some(AcpRow::Sep) => to.saturating_add_signed(dir).min(last),
            _ => to,
        };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('p'), _) => Act::Probe,
            (KeyCode::Enter, Some(AcpRow::Registry(a))) => Act::Send(Command::AddAcp(a.id.clone())),
            (KeyCode::Char('d') | KeyCode::Backspace, Some(AcpRow::Entry(e))) => {
                Act::Send(Command::RemoveAcp(e.name.clone()))
            }
            _ => Act::Nothing,
        };
        let Some(Menu::Acp {
            entries,
            cursor: cur,
            ..
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Probe => {
                for e in entries.iter_mut() {
                    e.status = Some("probing…".into());
                }
                send(tx, Command::ListAcp { probe: true });
            }
            Act::Send(cmd) => send(tx, cmd),
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
        MenuKey::Consumed
    }

    /// `/link` menu | `on` | `off` | `pair` | `cancel` | `rm <id>` |
    /// `name <id> <name…>` | `relay <url>` (31 §6.5).
    fn link_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.awaiting_link = true;
                send(tx, Command::LinkStatus);
            }
            ("on", _) => {
                self.awaiting_link = true;
                send(tx, Command::LinkEnable(true));
            }
            ("off", _) => send(tx, Command::LinkEnable(false)),
            ("pair", _) => send(tx, Command::LinkPair),
            ("cancel", _) => send(tx, Command::LinkCancelPair),
            ("rm", id) if !id.is_empty() => send(tx, Command::LinkRevoke(id.to_owned())),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                send(
                    tx,
                    Command::LinkRenameDevice(id.to_owned(), name.trim().to_owned()),
                );
            }
            ("relay", url) if !url.is_empty() => send(tx, Command::LinkRelay(url.to_owned())),
            _ => self.entries.push(Entry::Error(
                "usage: /link [on|off|pair|cancel|rm <id>|name <id> <name>|relay <url>]".into(),
            )),
        }
    }

    /// `/sandbox` lists the sandbox VMs; `/sandbox desktop NAME` prints the noVNC URL (79 §8).
    fn sandbox_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("" | "ls" | "list", _) => send(tx, Command::SandboxList),
            ("desktop", name) if !name.is_empty() => {
                send(tx, Command::SandboxDesktop(name.to_owned()))
            }
            _ => self
                .entries
                .push(Entry::Error("usage: /sandbox [desktop <name>]".into())),
        }
    }

    /// `/mesh devices [list|add [name]|rm <id>|name <id> <name…>]` (69 §7), the CLI's `t mesh devices`.
    fn mesh_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        const USAGE: &str = "usage: /mesh devices [list|add [name]|rm <id>|name <id> <name>]";
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        if sub != "devices" {
            self.entries.push(Entry::Error(USAGE.into()));
            return;
        }
        let (verb, rest) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(s, r)| (s, r.trim()));
        match (verb, rest) {
            ("" | "list", _) => send(tx, Command::MeshDevicesList),
            ("add", name) => send(
                tx,
                Command::MeshDevicesAdd((!name.is_empty()).then(|| name.to_owned())),
            ),
            ("rm", id) if !id.is_empty() => send(tx, Command::MeshDevicesRevoke(id.to_owned())),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                send(
                    tx,
                    Command::MeshDevicesRename(id.to_owned(), name.trim().to_owned()),
                );
            }
            _ => self.entries.push(Entry::Error(USAGE.into())),
        }
    }

    /// `/link` menu title: the status line, then the keys.
    pub fn link_title(&self) -> String {
        let Some(s) = &self.link else {
            return "link".into();
        };
        let state = match (s.enabled, s.connected) {
            (false, _) => "off",
            (true, true) => "connected",
            (true, false) => "disconnected",
        };
        let id: String = s
            .harness_id
            .as_deref()
            .map_or_else(|| "no identity".into(), |id| id.chars().take(8).collect());
        format!(
            "link · {} · {state} · {} ({id}…)   p pair  r rename  x revoke  n node name  e {}  esc",
            relay_host(&s.relay),
            s.name,
            if s.enabled { "disable" } else { "enable" }
        )
    }

    /// `/link` menu rows: `● name  platform  paired <ago>  online|seen <ago>`
    /// per device, or one hint row when none is paired.
    pub fn link_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self
            .link
            .iter()
            .flat_map(|s| &s.devices)
            .map(|d| {
                let seen = match (d.online, d.last_seen_ms) {
                    (true, _) => "online".to_owned(),
                    (false, 0) => "never seen".to_owned(),
                    (false, ms) => format!("seen {}", since(ms)),
                };
                format!(
                    "{} {:<20} {:<8} paired {:<10} {seen}",
                    if d.online { "●" } else { "○" },
                    d.name.chars().take(20).collect::<String>(),
                    platform_name(&d.platform),
                    since(d.paired_at_ms),
                )
            })
            .collect();
        if rows.is_empty() {
            return vec!["no devices paired · p pairs one".into()];
        }
        rows
    }

    /// `/link`: `p` pairs (card in the transcript), `r`/`n`/`x` open a
    /// prompt, `e` flips `link.enabled` in place.
    fn link_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Link { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let Some(s) = &self.link else {
            self.menu = None;
            return MenuKey::Consumed;
        };
        let device = s
            .devices
            .get(*cursor)
            .map(|d| (d.id.clone(), d.name.clone()));
        let enabled = s.enabled;
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(s.devices.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('p') => {
                self.menu = None;
                send(tx, Command::LinkPair);
            }
            KeyCode::Char('e') => send(tx, Command::LinkEnable(!enabled)),
            KeyCode::Char('n') => {
                self.menu = None;
                self.prompt = Some(Prompt::LinkName);
            }
            KeyCode::Char('r') => {
                if let Some((id, name)) = device {
                    self.menu = None;
                    self.prompt = Some(Prompt::LinkDeviceName { id, name });
                }
            }
            KeyCode::Char('x') => {
                if let Some((id, name)) = device {
                    self.menu = None;
                    self.prompt = Some(Prompt::LinkRevoke { id, name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/nodes` menu | `add <key>` | `rm <n>` | `name <n> <name…>` |
    /// `invite <n>` (32 §6.7). `<n>` is a node name or id prefix, resolved
    /// by the command task against `list_nodes` ([`find_node`]).
    fn nodes_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        match parse_nodes_arg(arg) {
            Some(NodesArg::Menu) => {
                self.awaiting_nodes = true;
                send(tx, Command::ListNodes);
            }
            Some(NodesArg::Add(key)) => send(tx, Command::AddNode(key, None)),
            Some(NodesArg::Remove(n)) => send(tx, Command::RemoveNode(n)),
            Some(NodesArg::Rename(n, name)) => send(tx, Command::RenameNode(n, name)),
            Some(NodesArg::Invite(n)) => send(tx, Command::InviteNode(n)),
            None => self.entries.push(Entry::Error(
                "usage: /nodes [add <key>|rm <n>|name <n> <name>|invite <n>]".into(),
            )),
        }
    }

    /// `/nodes` menu title: the count, then the keys.
    pub fn nodes_title(&self) -> String {
        format!(
            "nodes · {} paired   a add (paste key)  i invite (QR for a phone)  r rename  x remove  esc",
            self.nodes.as_ref().map_or(0, Vec::len)
        )
    }

    /// `/nodes` menu rows: `● name  platform  theuth <version>  [<n> running]  paired <ago>`
    /// online, `○ name  platform  theuth <version>  seen <ago>|revoked` otherwise,
    /// or one hint row when none is paired.
    pub fn nodes_rows(&self) -> Vec<String> {
        let now = now_ms();
        let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
        let rows: Vec<String> = self
            .nodes
            .iter()
            .flatten()
            .map(|n| {
                let tail = match (n.revoked, n.online, n.children, n.last_seen_ms) {
                    (true, ..) => "revoked".to_owned(),
                    (false, true, 0, _) => format!("paired {}", since(n.paired_at_ms)),
                    (false, true, c, _) => {
                        format!("{c} running   paired {}", since(n.paired_at_ms))
                    }
                    (false, false, _, 0) => "never seen".to_owned(),
                    (false, false, _, ms) => format!("seen {}", since(ms)),
                };
                format!(
                    "{} {:<20} {:<8} theuth {:<8} {tail}",
                    if n.online { "●" } else { "○" },
                    n.name.chars().take(20).collect::<String>(),
                    platform_name(&n.platform),
                    n.version,
                )
            })
            .collect();
        if rows.is_empty() {
            return vec!["no nodes paired · a adds one from its key".into()];
        }
        rows
    }

    /// `/nodes`: `a` prompts for a key, `i` invites (card in the transcript),
    /// `r`/`x` open a prompt over the highlighted node.
    fn nodes_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Nodes { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let list = self.nodes.as_deref().unwrap_or_default();
        let node = list.get(*cursor).map(|n| (n.id.clone(), n.name.clone()));
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(list.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('a') => {
                self.menu = None;
                self.prompt = Some(Prompt::NodeKey);
            }
            KeyCode::Char('i') => {
                if let Some((id, _)) = node {
                    self.menu = None;
                    send(tx, Command::InviteNode(id));
                }
            }
            KeyCode::Char('r') => {
                if let Some((id, name)) = node {
                    self.menu = None;
                    self.prompt = Some(Prompt::NodeName { id, name });
                }
            }
            KeyCode::Char('x') => {
                if let Some((id, name)) = node {
                    self.menu = None;
                    self.prompt = Some(Prompt::NodeRemove { id, name });
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

/// A parsed `/nodes` argument.
#[derive(Debug, PartialEq, Eq)]
pub enum NodesArg {
    /// Bare `/nodes`: open the menu.
    Menu,
    /// `add <key>`.
    Add(String),
    /// `rm <n>`.
    Remove(String),
    /// `name <n> <name…>`.
    Rename(String, String),
    /// `invite <n>`.
    Invite(String),
}

/// `/nodes` verbs; `None` for anything else (the caller prints usage).
pub fn parse_nodes_arg(arg: &str) -> Option<NodesArg> {
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    Some(match (sub, rest) {
        ("", _) => NodesArg::Menu,
        ("add", key) if !key.is_empty() => NodesArg::Add(key.to_owned()),
        ("rm", n) if !n.is_empty() => NodesArg::Remove(n.to_owned()),
        ("invite", n) if !n.is_empty() => NodesArg::Invite(n.to_owned()),
        ("name", rest) => {
            let (n, name) = rest.split_once(char::is_whitespace)?;
            NodesArg::Rename(n.to_owned(), name.trim().to_owned())
        }
        _ => return None,
    })
}

/// The node `/nodes <verb> <n>` names: an exact name, else the one node
/// whose id starts with `n`.
pub fn find_node<'a>(list: &'a [nodes::Node], n: &str) -> Option<&'a nodes::Node> {
    list.iter().find(|x| x.name == n).or_else(|| {
        let mut hits = list.iter().filter(|x| x.id.starts_with(n));
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    })
}

/// Host part of a relay origin: `wss://relay.theuth.io/x` → `relay.theuth.io`.
fn relay_host(relay: &str) -> &str {
    let rest = relay.split_once("://").map_or(relay, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

/// `ios` → `iOS`, `macos` → `macOS`; anything else as sent.
fn platform_name(p: &str) -> &str {
    match p {
        "ios" => "iOS",
        "macos" => "macOS",
        other => other,
    }
}

/// One-line `link_status`: `link off` or `link on · <host> · connected · 2 devices (1 online)`.
pub fn link_line(s: &link::LinkStatus) -> String {
    if !s.enabled {
        return "link off".into();
    }
    format!(
        "link on · {} · {} · {} device{} ({} online)",
        relay_host(&s.relay),
        if s.connected {
            "connected"
        } else {
            "disconnected"
        },
        s.devices.len(),
        if s.devices.len() == 1 { "" } else { "s" },
        s.devices.iter().filter(|d| d.online).count()
    )
}

/// One `/sandbox` line: name, backend, state, engine, size.
pub fn sandbox_line(i: &sandbox::SandboxInfo) -> String {
    let state = serde_json::to_value(&i.state)
        .ok()
        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let mut s = format!(
        "{} {} {} engine={}",
        i.name,
        i.backend,
        state,
        i.engine.as_deref().unwrap_or("-")
    );
    if let Some(vm) = &i.vm {
        let r = vm.resources;
        s.push_str(&format!(
            " cpus={} memory={}M disk={}M shares={} desktop={}",
            r.cpus,
            r.memory_mib,
            r.root_disk_mib,
            vm.shares.len(),
            if vm.desktop.is_some() { "yes" } else { "no" }
        ));
    }
    s
}

/// One `/mesh devices` row: `<id>  <name>  <platform>  paired 2d ago · seen 3h ago | never seen | revoked`.
pub fn mesh_device_line(d: &link::DeviceInfo, now: u64) -> String {
    let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
    let state = if d.revoked {
        "revoked".to_owned()
    } else if d.last_seen_ms == 0 {
        "never seen".to_owned()
    } else {
        format!("seen {}", since(d.last_seen_ms))
    };
    format!(
        "{}  {}  {}  paired {} · {state}",
        d.id,
        d.name,
        d.platform,
        since(d.paired_ms)
    )
}

/// A `/model` picker row (124 §3).
#[derive(Debug)]
pub enum ModelRow<'a> {
    /// 125 §7.2: the advisor picker's lead row; `current` while no advisor is set.
    Off {
        /// Whether the advisor is off now.
        current: bool,
    },
    /// The session's current model: always first, never filtered out.
    Current(&'a ModelInfo),
    /// A provider on the providers level; `current` marks the current model's.
    Provider {
        /// Provider id as it appears in `provider/model`.
        id: String,
        /// Models listed under it.
        count: usize,
        /// Whether the current model belongs to it.
        current: bool,
    },
    /// A starred model on the providers level (127 §3), in favorites order.
    Favorite(&'a ModelInfo),
    /// A model on a provider's level or in the search results.
    Model(&'a ModelInfo),
}

/// Search terms (124 §2.1): lowercased words of `query`; none means browse mode.
fn search_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether `t` occurs in `s` at the start of a word (124 §2.2): at index 0 or
/// after a character outside `[a-z0-9]`.
fn boundary_match(s: &str, t: &str) -> bool {
    s.match_indices(t)
        .any(|(i, _)| !s[..i].ends_with(|c: char| c.is_ascii_alphanumeric()))
}

/// Tier of one search term for one model (124 §2.2 table), before gating.
fn term_tier(name: &str, id: &str, provider: &str, t: &str) -> u32 {
    let tail = id.split_once('/').map_or(id, |(_, tail)| tail);
    if name == t || tail == t {
        100
    } else if name.starts_with(t) {
        90
    } else if boundary_match(name, t) {
        80
    } else if provider == t {
        70
    } else if provider.starts_with(t) {
        65
    } else if boundary_match(tail, t) {
        60
    } else if name.contains(t) {
        40
    } else if id.contains(t) {
        30
    } else {
        0
    }
}

/// Word search over `items` (124 §2): every term must match; per term, plain
/// substring hits (40/30) only count when nothing matches at a word boundary;
/// score is the sum of tiers, ties keep catalog order. Browse mode (no terms)
/// returns everything.
pub fn rank_models<'a>(items: &'a [ModelInfo], query: &str) -> Vec<&'a ModelInfo> {
    let lower: Vec<(String, String, String)> = items
        .iter()
        .map(|m| {
            (
                m.name.to_lowercase(),
                m.id.to_lowercase(),
                m.provider.to_lowercase(),
            )
        })
        .collect();
    let mut scores = vec![Some(0u32); items.len()];
    for t in search_terms(query) {
        let tiers: Vec<u32> = lower
            .iter()
            .map(|(name, id, provider)| term_tier(name, id, provider, &t))
            .collect();
        let gate = tiers.iter().any(|&tier| tier >= 60);
        for (score, &tier) in scores.iter_mut().zip(&tiers) {
            match tier {
                0 => *score = None,
                1..60 if gate => *score = None,
                _ => {
                    if let Some(s) = score {
                        *s += tier;
                    }
                }
            }
        }
    }
    let mut hits: Vec<(u32, &ModelInfo)> = scores
        .iter()
        .zip(items)
        .filter_map(|(score, m)| score.map(|s| (s, m)))
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, m)| m).collect()
}

/// Distinct providers of `items` with their model counts, A→Z (124 §3.1).
pub fn providers(items: &[ModelInfo]) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for m in items {
        *counts.entry(m.provider.as_str()).or_insert(0) += 1;
    }
    counts.into_iter().map(|(p, n)| (p.to_owned(), n)).collect()
}

/// The models of `provider`, by name (case-insensitive) then id (124 §3.2).
pub fn provider_models<'a>(items: &'a [ModelInfo], provider: &str) -> Vec<&'a ModelInfo> {
    let mut out: Vec<&ModelInfo> = items.iter().filter(|m| m.provider == provider).collect();
    out.sort_by_cached_key(|m| (m.name.to_lowercase(), m.id.as_str()));
    out
}

/// `/mcp add` argument that means "clone a repo": a `.git` URL or a known git host.
pub fn git_url(s: &str) -> bool {
    s.ends_with(".git")
        || ["github.com/", "gitlab.com/", "codeberg.org/"]
            .iter()
            .any(|h| s.contains(h))
}

/// Parse one MCP server from `/mcp add <json>`: a server object with a
/// `name` key (stripped), `{ "<name>": {...} }`, or `{ "mcpServers": { "<name>": {...} } }`.
pub fn mcp_entry(text: &str) -> Result<(String, serde_json::Value), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(mut obj) = v else {
        return Err("expected a JSON object".into());
    };
    if let Some(serde_json::Value::Object(servers)) = obj.remove("mcpServers") {
        obj = servers;
    }
    if obj.contains_key("command") || obj.contains_key("url") {
        return match obj.remove("name") {
            Some(serde_json::Value::String(name)) => Ok((name, serde_json::Value::Object(obj))),
            _ => Err("server object needs a \"name\" field".into()),
        };
    }
    match (obj.len(), obj.into_iter().next()) {
        (1, Some((name, entry))) if entry.is_object() => Ok((name, entry)),
        _ => Err("expected exactly one server: {\"<name>\": {\"command\": ...}}".into()),
    }
}

/// Transcript entries for one user input: a skill injection (04 §2.4) is a
/// collapsed `◇ name` card plus the user's words; an injected turn holding
/// several peer messages becomes one card each; anything else is `you`,
/// stamped with `turn` when the input opened one.
fn user_entries(text: String, turn: Option<i64>) -> Vec<Entry> {
    // 74 §3: the model's `<time …/>` line is never a `you` row.
    let text = agent::clock::strip_owned(text);
    if let Some((name, body, rest)) = agent::skills::injected(&text) {
        let mut out = vec![card(CardKind::Skill, name.to_owned(), body)];
        if !rest.is_empty() {
            out.push(Entry::User {
                text: rest.to_owned(),
                turn,
            });
        }
        return out;
    }
    match injected(&text) {
        None => vec![Entry::User { text, turn }],
        Some(cards) => cards
            .into_iter()
            .map(|(kind, title, body)| card(kind, title, &body))
            .collect(),
    }
}

/// The fence hook every assistant entry renders through; one allocation for the process.
static FENCE: LazyLock<markdown::FenceHook> = LazyLock::new(|| Arc::new(surface::fence));

/// Transcript entries for one history record; assistant text renders with
/// `opts` so replayed history looks like it did live. A row the user removed
/// from context (48 §2.2) is a collapsed `⊘` card over the original — the
/// archive stays readable (Ctrl+O), nothing re-enters the model's frame.
/// Attachment occurrences are listed by name, removed ones marked.
fn from_history(h: HistoryEntry, opts: &markdown::Options) -> Vec<Entry> {
    let removed = |c: &Option<ContextEntry>| c.as_ref().is_some_and(|c| c.removed);
    match h {
        // A consultation note (52 §6) is harness-authored, never `◆ you`:
        // one collapsed card, the question and answer on Ctrl+O.
        HistoryEntry::User {
            text,
            context,
            vision: Some(note),
            ..
        } => {
            if removed(&context) {
                return vec![removed_card("vision", &text)];
            }
            vec![card(CardKind::Vision, note_title(&note), &text)]
        }
        HistoryEntry::User {
            text,
            turn,
            context,
            peer,
            vision: None,
        } => {
            if removed(&context) {
                return vec![removed_card("you", &text)];
            }
            // Typed provenance (50 §9): the card is titled by the
            // authenticated sender, the wrapper is only unwrapped for the
            // body. A wrapper without `peer` stays the untrusted card
            // `user_entries` always drew: no id, no controls.
            let mut out = match peer {
                Some(origin) => {
                    let body = injected(&text)
                        .and_then(|cards| cards.into_iter().next())
                        .map_or(text, |(_, _, body)| body);
                    vec![peer_card(&origin, body)]
                }
                None => user_entries(text, turn),
            };
            // One row each: `Entry::height` counts an `Info` as one line.
            out.extend(attachment_labels(&context).into_iter().map(Entry::Info));
            out
        }
        HistoryEntry::Assistant { text, context, .. } => {
            if removed(&context) {
                return vec![removed_card("assistant", &text)];
            }
            let mut t = markdown::Text::with(opts.clone());
            t.push(&text);
            t.finish();
            t.reveal_all();
            vec![Entry::Assistant(Box::new(t))]
        }
        // 70 THINKING-TRACES: the reload shows the same one-line collapsed block the stream left.
        HistoryEntry::Reasoning { text, .. } => vec![Entry::Reasoning { text, done: true }],
        HistoryEntry::Tool {
            call_id,
            name,
            args,
            result,
            files: _,
            context,
        } => {
            let mut args = describe(&name, &args);
            args.extend(attachment_labels(&context));
            // `.` marks a call from inside a `python` cell (08 §4.1), as live.
            let shown = if call_id.contains('.') {
                format!("↳ {name}")
            } else {
                name
            };
            if removed(&context) {
                if let Some((output, _)) = &result {
                    args.push(output.clone());
                }
                return vec![removed_card(&shown, &args.join("\n"))];
            }
            vec![Entry::Tool {
                args,
                name: shown,
                call_id,
                job: None,
                artifact: None,
                scrape: None,
                result,
                expanded: None,
            }]
        }
        HistoryEntry::Compacted {
            epoch,
            summary,
            style,
            context,
        } => {
            if removed(&context) {
                return vec![removed_card(&format!("summary (epoch {epoch})"), &summary)];
            }
            vec![Entry::Compacted {
                epoch,
                style,
                summary: Some(Box::new(markdown::Text::complete(&summary))),
                expanded: false,
            }]
        }
        // Never model content: shown, never reasoned over (48 §3.3 has nothing to remove).
        HistoryEntry::Error { text, .. } => vec![Entry::Error(text)],
        // 109 §8.2: the dropped partial, greyed as an info line under its divider.
        HistoryEntry::Interrupted { text, .. } => vec![
            Entry::Divider("interrupted — resuming".to_owned()),
            Entry::Info(text),
        ],
        // 125 §7.4: one card per Advisory row, three notes until Ctrl+O.
        HistoryEntry::Advisory { advisory, .. } => vec![Entry::Advisory {
            advisory,
            expanded: false,
        }],
    }
}

/// The collapsed marker of a row removed from context; `original` is the
/// archived text, shown on Ctrl+O only.
fn removed_card(what: &str, original: &str) -> Entry {
    card(
        CardKind::Removed,
        format!("{what} · removed from context"),
        original,
    )
}

/// One `VisionProgress` step as a line (52 §6): the status text while the
/// question or inspection is in flight, the card title once it settled
/// (every source answered on `Completed`, none on `Failed`). `pub` for
/// print mode.
pub fn vision_line(stage: VisionStage, model: &str, n: usize) -> String {
    match stage {
        VisionStage::Questioning => format!("asking {model} what to look for…"),
        VisionStage::Inspecting => format!("{model} inspecting {n} image(s)…"),
        VisionStage::Completed => vision_title(model, n, n),
        VisionStage::Failed => vision_title(model, 0, n),
    }
}

/// Header of a vision card: `vision: <fallback> inspected 2 image(s)`,
/// `… inspected 1 of 3 image(s)`, or `… failed on 3 image(s)` when nothing
/// was answered. Shared by [`vision_line`] and the reloaded note
/// ([`note_title`]).
fn vision_title(model: &str, answered: usize, total: usize) -> String {
    match answered {
        0 => format!("vision: {model} failed on {total} image(s)"),
        a if a == total => format!("vision: {model} inspected {total} image(s)"),
        a => format!("vision: {model} inspected {a} of {total} image(s)"),
    }
}

/// [`vision_title`] of a stored note, counting its answered observations.
pub fn note_title(note: &VisionNote) -> String {
    let answered = note
        .observations
        .iter()
        .filter(|o| o.status == VisionStatus::Answered)
        .count();
    vision_title(&note.vision_model, answered, note.observations.len())
}

/// One line per attachment occurrence of a history row: `⎘ shot.png
/// (image/png)`, `⊘ report.pdf (application/pdf) · removed from context`.
fn attachment_labels(c: &Option<ContextEntry>) -> Vec<String> {
    c.as_ref().map_or_else(Vec::new, |c| {
        c.attachments.iter().map(attachment_label).collect()
    })
}

/// See [`attachment_labels`].
pub fn attachment_label(a: &ContextAttachment) -> String {
    if a.removed {
        format!("⊘ {} ({}) · removed from context", a.name, a.mime)
    } else {
        format!("⎘ {} ({})", a.name, a.mime)
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// First eight hex digits of a message id: enough to tell rows apart in a
/// menu; the exact id travels in the command, never re-parsed from this.
fn short_uuid(id: Uuid) -> String {
    id.simple().to_string()[..8].to_owned()
}

/// The peer card of a consumed exchange (live `PeerDelivered` or replayed
/// `HistoryEntry::User { peer }`): titled by its authenticated origin.
fn peer_card(origin: &PeerOrigin, body: String) -> Entry {
    let mut title = format!("{} · {}", origin.from, short_uuid(origin.id));
    if let Some(r) = origin.reply_to {
        title.push_str(&format!(" · reply to {}", short_uuid(r)));
    }
    card(CardKind::Peer, title, &body)
}

/// The inspector card of one pending request (`/peers` `i`): every durable
/// field above the body, expanded so it reads at once.
fn request_card(r: &PeerRecord, resolve: bool) -> Entry {
    let rc = &r.receipt;
    let mut meta = vec![
        format!("id: {}", rc.id),
        format!("from: {}", r.from),
        format!("to: {}", rc.to),
        format!("state: {}", rc.state.as_str()),
        format!(
            "created: {}",
            r.created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
        ),
        if resolve {
            "pending: this session sent it and is blocked on the reply — `a` resolves its wait with your text".to_owned()
        } else {
            "pending: the sender awaits this session's answer — `a` answers on its behalf"
                .to_owned()
        },
    ];
    if let Some(rt) = rc.reply_to {
        meta.push(format!("reply_to: {rt}"));
    }
    if let Some(s) = r.request_scope {
        meta.push(format!("request scope: {s}"));
    }
    if let Some(e) = &rc.error {
        meta.push(format!("error: {e}"));
    }
    Entry::Card {
        kind: CardKind::Peer,
        title: format!("request {} · {}", short_uuid(rc.id), r.from),
        text: Box::new(markdown::Text::complete(&format!(
            "{}\n\n{}",
            meta.join("  \n"),
            r.text
        ))),
        expanded: true,
    }
}

/// `/communication` lines (50 §2.1): the effective state with its exact
/// blocker first, then the switches it derives from, then the network. The
/// global switch is a ceiling: a session `on` under global `off` says so.
pub fn communication_lines(s: &CommunicationState) -> Vec<String> {
    let first = match (s.session, s.effective) {
        (None, _) => format!(
            "communication: {} (global default · source {}){}",
            on_off(s.global),
            s.source,
            if s.global {
                ""
            } else {
                " — `/communication on always` enables the harness"
            }
        ),
        (Some(_), true) => "communication: on (this session) — routes still need scope or a grant".to_owned(),
        (Some(_), false) if !s.global => format!(
            "communication: off — disabled globally (source {}); a session `on` cannot override it, `/communication on always` enables the harness",
            s.source
        ),
        (Some(_), false) if s.opted_out => {
            "communication: off — this session opted out (memory-only: cleared on daemon restart); `/communication on` re-enables it".to_owned()
        }
        (Some(_), false) => format!(
            "communication: off — inherited: ancestor {} opted out (memory-only: cleared on daemon restart); `/communication on` there re-enables the tree",
            s.inherited_from.map_or_else(|| "?".to_owned(), short)
        ),
    };
    let mut out = vec![first];
    if s.session.is_some() {
        out.push(format!(
            "  global {} ({})  ·  this session {}  ·  root scope {}{}",
            on_off(s.global),
            s.source,
            if s.opted_out { "off" } else { "on" },
            s.scope.as_str(),
            s.alias
                .as_deref()
                .map_or_else(String::new, |a| format!(" · alias {a}"))
        ));
    }
    out.push(match (s.network_enabled, s.listening) {
        (false, _) => "  network (a2a): off — `a2a.enabled: false`".to_owned(),
        (true, true) => "  network (a2a): on · listening".to_owned(),
        (true, false) if !s.global => {
            "  network (a2a): enabled · listener stopped while communication is off globally"
                .to_owned()
        }
        (true, false) => "  network (a2a): enabled · not listening — `/a2a` shows why".to_owned(),
    });
    out
}

/// `/a2a` lines: the listener, then one row per configured peer and export,
/// errors where they belong.
pub fn a2a_lines(s: &a2a::A2aStatus) -> Vec<String> {
    let mut out = vec![match (s.enabled, s.listening) {
        (false, _) => "a2a: off — `a2a.enabled: false`; nothing listens, nothing dials".to_owned(),
        (true, true) => format!(
            "a2a: on · listening {}{}",
            s.address.as_deref().unwrap_or("?"),
            s.public_url
                .as_deref()
                .map_or_else(String::new, |u| format!(" · public {u}"))
        ),
        (true, false) => {
            "a2a: enabled · not listening (communication off globally, or see error)".to_owned()
        }
    }];
    if let Some(e) = &s.error {
        out.push(format!("  error: {e}"));
    }
    for p in &s.peers {
        out.push(format!(
            "  peer {:<16} {}{}  sessions {}  exports {}{}{}",
            p.name,
            if p.outbound { "→out " } else { "" },
            if p.inbound { "←in" } else { "" },
            p.sessions.len(),
            if p.exports.is_empty() {
                "–".to_owned()
            } else {
                p.exports.join(",")
            },
            p.card
                .as_deref()
                .map_or_else(String::new, |c| format!("  card {c}")),
            p.error
                .as_deref()
                .map_or_else(String::new, |e| format!("  error: {e}")),
        ));
    }
    for e in &s.exports {
        out.push(format!(
            "  export {:<14} session {}  {}{}",
            e.name,
            short(e.session),
            e.url,
            e.peer
                .as_deref()
                .map_or_else(String::new, |p| format!("  peer {p}"))
        ));
    }
    if s.enabled && s.peers.is_empty() && s.exports.is_empty() {
        out.push(
            "  no peers or exports configured (peers.yaml; `/a2a reload` applies edits)".to_owned(),
        );
    }
    out
}

/// One row of the `/peers` menu.
#[derive(Debug)]
pub enum PeerRow<'a> {
    /// A permitted peer; ⏎ opens a native one.
    Peer(&'a PeerInfo),
    /// Blank line between the directory and the requests; the cursor skips it.
    Sep,
    /// A request a human may resolve now: `true` when the shown session
    /// sent it and awaits the reply, `false` when it must answer.
    Request(&'a PeerRecord, bool),
    /// A hint or blocker in place of rows.
    Note(String),
}

impl PeerRow<'_> {
    /// The row's text.
    pub fn line(&self) -> String {
        match self {
            PeerRow::Peer(p) => format!(
                "{} {:<20} {:<12} {:<10} {:<8} {}{}{}",
                match p.state {
                    agent::communication::PeerState::Running
                    | agent::communication::PeerState::Awaiting => "●",
                    agent::communication::PeerState::Idle => "○",
                    _ => " ",
                },
                p.name.chars().take(20).collect::<String>(),
                format!("{:?}", p.relationship).to_lowercase(),
                format!("{:?}", p.state).to_lowercase(),
                format!("{:?}", p.kind).to_lowercase(),
                if p.send { "send " } else { "" },
                if p.reply { "reply " } else { "" },
                p.address
            ),
            PeerRow::Sep => String::new(),
            PeerRow::Request(r, resolve) => format!(
                "{} {}  {:<44}  {}",
                if *resolve { "↑" } else { "?" },
                short_uuid(r.receipt.id),
                if *resolve {
                    format!("to {} · awaiting their reply", r.receipt.to)
                } else {
                    format!("from {} · awaiting your answer", r.from)
                }
                .chars()
                .take(44)
                .collect::<String>(),
                r.text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(50)
                    .collect::<String>()
            ),
            PeerRow::Note(n) => n.clone(),
        }
    }
}

/// The tool whose card is an artifact card (35 §0.3).
const SHOW: &str = "show";
/// The tool whose card is an app card (46 §3.1): same join, source is the file.
const APP: &str = "app";
/// Where an app runs; the TUI only reads its source (46 §8.2).
pub const APP_WHERE: &str = "interactive on desktop/web";
/// Rows an inline artifact body shows at most (35 §6.1).
const INLINE_LINES: usize = 200;
/// Rows of a CSV artifact shown inline.
const CSV_ROWS: usize = 20;

/// Bytes as `512 B` / `3.2 KB` / `1.4 MB`.
pub fn bytes_label(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// A `Pending` call's streamed arguments so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// For `show`: the file and whether `content` is being written.
    pub show: Option<surface::PartialShow>,
    /// Cumulative argument bytes.
    pub bytes: usize,
}

/// The spinner row of a `Pending` call with progress: `writing report.pdf… 3.2 KB`
/// when `show` carries `content`, `showing report.pdf…` when it names a path,
/// `building <name>… 3.2 KB` for `app` (its `name` streams first),
/// `preparing <tool>… 3.2 KB` for every other tool.
pub fn progress_line(tool: &str, p: &Progress) -> String {
    let size = bytes_label(p.bytes as u64);
    match (tool, &p.show) {
        ("show", Some(s)) => match (&s.name, s.writing) {
            (Some(n), true) => format!("writing {n}… {size}"),
            (Some(n), false) => format!("showing {n}…"),
            (None, true) => format!("writing… {size}"),
            (None, false) => "preparing show…".to_owned(),
        },
        (APP, Some(s)) => match &s.name {
            Some(n) => format!("building {n}… {size}"),
            None => format!("building app… {size}"),
        },
        _ => format!("preparing {tool}… {size}"),
    }
}

/// The spinner row of a running scrape tool from its latest `ScrapeProgress`
/// (design doc 38): `crawling… 3/50 · example.com` (host of the first URL in
/// flight), `mapping… 120/500` for `map_site`, `reading example.com…` for
/// `scrape_url`, `searching…` then `reading 2/5 · example.com` for
/// `search_web` (16 §2); `running <tool>…` for anything else.
pub fn scrape_line(tool: &str, p: &web::progress::ScrapeProgress) -> String {
    let at = p.inflight.first().or(p.done.last()).map_or("", |u| host(u));
    let n = p.done.len();
    match tool.strip_prefix("↳ ").unwrap_or(tool) {
        "crawl" if at.is_empty() => format!("crawling… {n}/{}", p.limit),
        "crawl" => format!("crawling… {n}/{} · {at}", p.limit),
        "map_site" => format!("mapping… {n}/{}", p.limit),
        "scrape_url" => format!("reading {at}…"),
        "search_web" if at.is_empty() => "searching…".to_owned(),
        "search_web" => format!("reading {n}/{} · {at}", p.limit),
        _ => format!("running {tool}…"),
    }
}

/// `example.com` from `https://example.com/a/b`; the input when it has no scheme or path.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

/// The kind-specific size of an artifact card header (35 §6.1): pages for a
/// compiled PDF, bytes otherwise. Drawn every frame, so nothing here touches
/// the file; the line count shows in the expanded body.
pub fn artifact_size(a: &artifact::Artifact) -> String {
    match (a.kind, a.pages) {
        (artifact::Kind::Pdf, Some(p)) => format!("{p} pages · {}", bytes_label(a.bytes)),
        _ => bytes_label(a.bytes),
    }
}

/// One `/artifacts` row: `▣ name · kind · size · vN · 2m ago`.
pub fn artifact_row(a: &artifact::Artifact) -> String {
    let age = chrono::DateTime::from_timestamp(a.created_at, 0)
        .map(|t| chrono::Utc::now().signed_duration_since(t))
        .map_or_else(String::new, |d| {
            let secs = d.num_seconds().max(0);
            match secs {
                s if s < 60 => format!("{s}s ago"),
                s if s < 3600 => format!("{}m ago", s / 60),
                s if s < 86400 => format!("{}h ago", s / 3600),
                s => format!("{}d ago", s / 86400),
            }
        });
    format!(
        "▣ {} · {} · {} · v{} · {age}",
        a.name,
        a.kind.as_str(),
        bytes_label(a.bytes),
        a.version
    )
}

/// The first [`INLINE_LINES`] lines of `s` and how many were cut.
fn cap_lines(s: &str) -> (String, usize) {
    let total = s.lines().count();
    if total <= INLINE_LINES {
        return (s.trim_end().to_owned(), 0);
    }
    let kept: Vec<&str> = s.lines().take(INLINE_LINES).collect();
    (kept.join("\n"), total - INLINE_LINES)
}

/// `body` in a code fence tagged `lang`; the fence grows past any backtick run inside.
fn fenced(lang: &str, body: &str) -> String {
    let longest = body
        .lines()
        .map(|l| l.chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{body}\n{fence}")
}

/// A CSV cell as a markdown table cell: pipes escaped, newlines flattened.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// An artifact's inline body by kind (35 §3.2): markdown rendered with the
/// assistant's options so its own fences draw, text in a fence tagged with the
/// extension, CSV as a table of the first [`CSV_ROWS`] rows, an app's
/// `.openui` source read-only in a fence under where it runs (46 §8.2),
/// anything else one line. Text bodies cap at [`INLINE_LINES`].
fn artifact_body(a: &artifact::Artifact, opts: &markdown::Options) -> markdown::Text {
    let bytes = artifact_read_path(a).map(|p| std::fs::read(p).map_err(|e| e.to_string()));
    artifact_text(
        a,
        bytes
            .as_ref()
            .map_or(Ok(&[][..]), |b| b.as_deref().map_err(String::as_str)),
        opts,
    )
}

/// The file an expanded card reads (35 §6.1): the blob for text-like kinds,
/// the listing sidecar for an archive, nothing for pictures and downloads.
pub(crate) fn artifact_read_path(a: &artifact::Artifact) -> Option<std::path::PathBuf> {
    use artifact::Kind;
    match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::Csv | Kind::App | Kind::Mermaid => {
            Some(a.path.clone())
        }
        Kind::Archive => Some(artifact::archive::sidecar(&a.path)),
        Kind::Image | Kind::Svg | Kind::Pdf | Kind::File => None,
    }
}

/// Rows of an expanded archive card.
const ARCHIVE_ROWS: usize = 50;

/// [`artifact_body`] over bytes already read (the search preview reads them
/// off the UI thread); `Err` is why the file could not be read.
pub(crate) fn artifact_text(
    a: &artifact::Artifact,
    bytes: Result<&[u8], &str>,
    opts: &markdown::Options,
) -> markdown::Text {
    use artifact::Kind;
    let ext = a.path.extension().map_or("", |e| e.to_str().unwrap_or(""));
    let path = a.path.display().to_string();
    let unreadable = |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}");
    let md = match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::App => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => {
                let s = String::from_utf8_lossy(b);
                let (body, cut) = cap_lines(&s);
                let text = match a.kind {
                    Kind::Markdown => body,
                    Kind::App => format!("[app] {APP_WHERE} · source:\n\n{}", fenced(ext, &body)),
                    _ => fenced(ext, &body),
                };
                match cut {
                    0 => text,
                    n => format!("{text}\n\n… (+{n} lines, o opens)"),
                }
            }
        },
        Kind::Csv => match bytes
            .map_err(str::to_owned)
            .and_then(|b| artifact::kind::csv_table(b, ext == "tsv", CSV_ROWS))
        {
            Err(e) => unreadable(&e),
            Ok((header, rows, total)) => {
                let cols = header.len().max(1);
                let row = |cells: &[String]| {
                    let mut line = String::from("|");
                    for i in 0..cols {
                        line.push(' ');
                        line.push_str(&cells.get(i).map_or_else(String::new, |c| md_cell(c)));
                        line.push_str(" |");
                    }
                    line
                };
                let mut out = row(&header);
                out.push('\n');
                out.push_str(&format!("|{}", " --- |".repeat(cols)));
                for r in &rows {
                    out.push('\n');
                    out.push_str(&row(r));
                }
                if total > rows.len() {
                    out.push_str(&format!("\n\n… (+{} rows)", total - rows.len()));
                }
                out
            }
        },
        // A `mermaid` fence: the assistant's fence hook draws it through
        // `surface::mermaid_lines`, the plain code block when that declines.
        Kind::Mermaid => match bytes {
            Err(e) => unreadable(&e),
            Ok(b) => fenced("mermaid", String::from_utf8_lossy(b).trim_end()),
        },
        Kind::Archive => match bytes.map_err(str::to_owned).and_then(|b| {
            serde_json::from_slice::<artifact::archive::ArchiveListing>(b)
                .map_err(|e| e.to_string())
        }) {
            Err(_) => format!(
                "{} · no listing (archive unreadable) · {path}",
                bytes_label(a.bytes)
            ),
            Ok(l) => {
                let mut out = format!(
                    "{} files · {} unpacked · {} · {path}\n",
                    l.total_entries,
                    bytes_label(l.total_size),
                    bytes_label(a.bytes)
                );
                for e in l.entries.iter().take(ARCHIVE_ROWS) {
                    out.push_str(&format!(
                        "\n    {:>9}  {}",
                        if e.is_dir {
                            String::new()
                        } else {
                            bytes_label(e.size)
                        },
                        e.path
                    ));
                }
                if l.total_entries > ARCHIVE_ROWS as u64 {
                    out.push_str(&format!(
                        "\n\n… (+{} entries, o opens)",
                        l.total_entries - ARCHIVE_ROWS as u64
                    ));
                }
                out
            }
        },
        Kind::Image => format!("[image] {path}"),
        Kind::Svg => format!("[svg] {path}"),
        Kind::Pdf => format!("{} · {path}", artifact_size(a)),
        Kind::File => path.clone(),
    };
    let mut t = markdown::Text::with(opts.clone());
    t.push(&md);
    t.finish();
    t.reveal_all();
    t
}

/// One `/knowledge` row: `` `id` — title [type]: description `` (10 §6).
pub fn meta_line(m: &Meta) -> String {
    format!("`{}` — {} [{}]: {}", m.id, m.title, m.type_, m.description)
}

/// `/knowledge history` rows per page; `older` asks for this many more.
pub const HISTORY_PAGE: usize = 20;
/// Chars a diff scrolls sideways per `←` / `→`.
const DIFF_STEP: usize = 8;

/// A revision as diff rows: `+ id` / `~ id` / `- id` per concept file, each
/// followed by its unified diff. Empty commits (only generated files
/// changed) say so.
pub fn revision_rows(r: &Revision) -> Vec<String> {
    if r.files.is_empty() {
        return vec!["no concept files changed".into()];
    }
    let mut rows = Vec::new();
    for f in &r.files {
        let glyph = match f.status {
            ChangeStatus::Added => "+",
            ChangeStatus::Modified => "~",
            ChangeStatus::Deleted => "-",
        };
        rows.push(format!("{glyph} {}  ({})", f.id, status_name(f.status)));
        rows.extend(f.diff.lines().map(str::to_owned));
    }
    rows
}

/// `added` / `modified` / `deleted`.
fn status_name(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Added => "added",
        ChangeStatus::Modified => "modified",
        ChangeStatus::Deleted => "deleted",
    }
}

/// The `/title` reply: the model naming sessions, or the fallback.
pub fn title_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!("title model: {} ({})", m.id, m.name),
        None => "title model: off — each session names itself with its own model (/title <model> to set a cheap one)".into(),
    }
}

/// The `/vision` reply: the model that inspects images for a text-only main
/// model (52 §3), or off.
pub fn vision_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!(
            "vision fallback: {} ({}) — used only when the main model cannot view images; it receives the image and a question written by the main model, an extra call to that provider",
            m.id, m.name
        ),
        None => "vision fallback: off — a text-only main model gets images as omitted (/vision <model> to set one)".into(),
    }
}

/// The `/aws` reply: which AWS profile Bedrock calls use, harness-wide.
pub fn aws_profile_line(p: &AwsProfiles) -> String {
    let now = match &p.selected {
        Some(s) if p.profiles.contains(s) => s.clone(),
        Some(s) => {
            format!("{s} (not found by the AWS SDK; `/aws clear` restores the default chain)")
        }
        None => format!(
            "default credential chain ({}), unless a Bedrock API key is configured",
            p.default_profile
        ),
    };
    let found = if p.profiles.is_empty() {
        "none found".to_owned()
    } else {
        p.profiles.join(", ")
    };
    format!(
        "aws profile: {now} — applies to every session's Bedrock calls from the next request\n  profiles: {found}"
    )
}

/// The `/schedules` usage line, shown on a bad argument.
pub const SCHEDULES_USAGE: &str =
    "usage: /schedules [run <name>|test <name>|history <name>|pause <name>|resume <name>]";

/// The `/shake` usage line, shown on an unknown argument (48 §2.1).
pub const SHAKE_USAGE: &str = "usage: /shake [images|documents|messages|tools|summary]";

/// The notice after `remove_context`: what left the context and the new
/// estimate — an estimate, not provider usage (48 §3.2).
pub fn removal_line(r: &ContextRemoval) -> String {
    let mut parts = Vec::new();
    for (n, noun) in [
        (r.messages, "message(s)"),
        (r.images, "image(s)"),
        (r.documents, "document(s)"),
        (r.heads, "summary head(s)"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {noun}"));
        }
    }
    if parts.is_empty() {
        return "nothing removed: already outside the context".into();
    }
    format!(
        "removed from context: {} · ~{} tokens now · originals stay in history",
        parts.join(", "),
        k(r.estimated_tokens)
    )
}

/// `/model` / `/effort` on an ACP session (75 §8): one line per harness
/// option — `model: anthropic/claude-opus-4-8  (has: a, b, c)` — the current
/// value first, then its values verbatim; the `thought_level` option is
/// labelled `effort` so `/effort <value>` reads back.
pub fn harness_options_lines(options: &[acp::ConfigOption]) -> Vec<String> {
    if options.is_empty() {
        return vec!["harness offers no options".into()];
    }
    options
        .iter()
        .map(|o| {
            let label = match o.category.as_deref() {
                Some("thought_level") => "effort",
                _ => o.id.as_str(),
            };
            let values: Vec<&str> = o.options.iter().map(|v| v.value.as_str()).collect();
            format!("{label}: {}  (has: {})", o.current, values.join(", "))
        })
        .collect()
}

/// The announcement after `set_harness_option` (75 §8): `model → x`, or
/// `model → x after this turn` when the change waits for the turn in flight.
pub fn harness_option_set_line(key: &str, value: &str, r: &acp::SetOptionResult) -> String {
    if r.pending {
        format!("{key} → {value} after this turn")
    } else {
        format!("{key} → {value}")
    }
}

/// `/schedules`: one row per file — `name · when · next · runs · last · result · state`,
/// plus `last check` (`10:15:03 quiet 1200 ms`, 40 §6) when any row is a watcher that was checked.
/// `runs` is `—` for a plain schedule, `N` or `N/max` for a loop (33 §3.2);
/// `state` is `running`, `off`, `error: …` (then `next` is `—`), or empty.
pub fn schedules_table(rows: &[schedule::ScheduleInfo]) -> String {
    if rows.is_empty() {
        return "no schedules".into();
    }
    let stamp = |t: Option<chrono::DateTime<chrono::Local>>| {
        t.map_or("—".to_owned(), |t| t.format("%a %d %b %H:%M").to_string())
    };
    let checks = rows.iter().any(|s| s.last_check.is_some());
    let mut header = "name · when · next · runs · last · result · state".to_owned();
    if checks {
        header.push_str(" · last check");
    }
    let mut out = vec![header];
    out.extend(rows.iter().map(|s| {
        let state = match (&s.error, s.running, s.enabled) {
            (Some(e), ..) => format!("error: {e}"),
            (None, true, _) => "running".to_owned(),
            (None, false, false) => "off".to_owned(),
            (None, false, true) => String::new(),
        };
        let next = if s.error.is_some() { None } else { s.next };
        let runs = match s.max_runs {
            Some(max) => format!("{}/{max}", s.runs),
            None if s.until.is_some() => s.runs.to_string(),
            None => "—".to_owned(),
        };
        let mut row = format!(
            "{} · {} · {} · {runs} · {} · {} · {state}",
            s.name,
            s.when,
            stamp(next),
            stamp(s.last),
            s.last_result.as_deref().unwrap_or("—"),
        );
        if checks {
            row.push_str(&s.last_check.as_ref().map_or_else(
                || " · —".to_owned(),
                |c| {
                    format!(
                        " · {} {} {} ms",
                        c.at.format("%H:%M:%S"),
                        c.outcome,
                        c.elapsed_ms
                    )
                },
            ));
        }
        row
    }));
    out.join("\n")
}

/// `/tasks`: `id · state · owner · title`, oldest first as the engine lists them (67 §12).
/// The heading names the project, or `all projects` when the session has none.
pub fn tasks_table(project: Option<&str>, rows: &[tasks::Task]) -> String {
    let scope = project.unwrap_or("all projects");
    if rows.is_empty() {
        return format!("no tasks ({scope})");
    }
    let mut out = vec![
        format!("tasks · {scope}"),
        "id · state · owner · title".to_owned(),
    ];
    out.extend(rows.iter().map(|t| {
        let id = &t.id.0;
        let short = &id[id.len().saturating_sub(6)..];
        format!("{short} · {} · {} · {}", t.state.as_str(), t.owner, t.title)
    }));
    out.join("\n")
}

/// `/roster`: `name · status · open` per roster agent (67 §12; `/agents` is the subagent picker).
pub fn agents_table(rows: &[roster::AgentSummary]) -> String {
    if rows.is_empty() {
        return "no agents".into();
    }
    let mut out = vec!["name · status · open".to_owned()];
    out.extend(rows.iter().map(|a| {
        let status = serde_json::to_value(a.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        format!("{} · {status} · {}", a.file.name.as_str(), a.open_tasks)
    }));
    out.join("\n")
}

/// Owner chip for a mesh `EngineId` (a 64-hex key): its first 8 chars; `?` when empty.
pub fn engine_chip(owner: &str) -> String {
    if owner.is_empty() {
        return "?".into();
    }
    owner.chars().take(8).collect()
}

/// `/sessions` row: mark · id · stamp · model · owner chip · title (68 §10).
/// The chip is the engine's short id on a session owned elsewhere, blank here.
pub fn session_row(s: &SessionSummary, depth: usize, width: u16, current: bool) -> String {
    // Row prefix (mark, id, stamp, model, chip, gaps, box) is ~62 columns; the title gets the rest.
    let room = usize::from(width).saturating_sub(62 + 2 * depth).max(16);
    let title: String = s.title.chars().take(room).collect();
    let chip = if s.here {
        String::new()
    } else {
        engine_chip(&s.owner)
    };
    format!(
        "{} {}{}  {}  {:<20} {chip:<8}  {title}",
        if current { "●" } else { " " },
        "  ".repeat(depth),
        short(s.id),
        s.updated_at
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M"),
        s.model.rsplit('/').next().unwrap_or(&s.model),
    )
}

/// The owner-offline sentence (68 §5), `/take` as the action. `last_seen` is
/// already text (`3m ago`, `never`).
pub fn owner_offline_line(owner: &str, last_seen: &str) -> String {
    let chip = engine_chip(owner);
    format!(
        "owned by {chip}, last seen {last_seen}. continue here now? \
         (turns not yet shipped from {chip} stay there) — /take"
    )
}

/// `OwnerUnreachable{owner, last_seen}` out of a failed call's `data`: the
/// serde external form (`{"OwnerUnreachable": {…}}`) or a flat object tagged
/// `kind`/`type`. Returns the owner and `last_seen` as `ago` text.
pub fn owner_unreachable(data: Option<&str>) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(data?).ok()?;
    let tagged = ["kind", "type"]
        .iter()
        .any(|k| v.get(k).and_then(|t| t.as_str()) == Some("OwnerUnreachable"));
    let o = v.get("OwnerUnreachable").or(tagged.then_some(&v))?;
    let owner = o.get("owner")?.as_str()?.to_owned();
    let last_seen = match o.get("last_seen") {
        Some(serde_json::Value::String(t)) => chrono::DateTime::parse_from_rfc3339(t).map_or_else(
            |_| t.clone(),
            |t| ago(chrono::Utc::now() - t.with_timezone(&chrono::Utc)),
        ),
        Some(serde_json::Value::Number(ms)) => ms
            .as_i64()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map_or_else(|| ms.to_string(), |t| ago(chrono::Utc::now() - t)),
        _ => "never".to_owned(),
    };
    Some((owner, last_seen))
}

/// `/schedules history <name>`: `fired · took · result · session · report` rows,
/// newest first; `took` is `…` while the run is still going, `report` is the
/// run's first line cut at 60 chars. A loop with `until` or `approvals: false`
/// gets a line naming those above the header (33 §6).
pub fn schedule_history_table(
    name: &str,
    info: Option<&schedule::ScheduleInfo>,
    rows: &[schedule::RunInfo],
) -> String {
    if rows.is_empty() {
        return format!("no runs of {name}");
    }
    let mut out = Vec::with_capacity(rows.len() + 2);
    if let Some(info) = info {
        let mut loop_line: Vec<String> = Vec::new();
        if let Some(until) = &info.until {
            loop_line.push(format!("until: {until}"));
        }
        if !info.approvals {
            loop_line.push("approvals off".to_owned());
        }
        if !loop_line.is_empty() {
            out.push(format!("{name}: {}", loop_line.join(" · ")));
        }
    }
    out.push(format!("{name}: fired · took · result · session · report"));
    out.extend(rows.iter().map(|r| {
        let took = r
            .finished_at
            .map_or("…".to_owned(), |f| elapsed(f - r.fired_at));
        let session = r.session.map_or("—".to_owned(), |s| {
            s.to_string().chars().take(8).collect()
        });
        let report = r
            .report
            .as_deref()
            .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
            .map_or("—".to_owned(), |l| {
                if l.chars().count() > 60 {
                    let mut cut: String = l.chars().take(59).collect();
                    cut.push('…');
                    cut
                } else {
                    l.to_owned()
                }
            });
        format!(
            "{} · {took} · {} · {session} · {report}",
            r.fired_at.format("%a %d %b %H:%M"),
            r.result.as_deref().unwrap_or("—"),
        )
    }));
    out.join("\n")
}

/// The `/computer` reply: permission ticks and the attached displays on one
/// line, then the status note on a second line when there is one.
pub fn computer_line(s: &computer::PermissionStatus) -> String {
    let tick = |ok: bool| if ok { "✓" } else { "✗" };
    let displays: Vec<String> = s
        .displays
        .iter()
        .map(|d| {
            format!(
                "{}{} {}×{} @{}×",
                d.index,
                if d.main { " main" } else { "" },
                d.width,
                d.height,
                d.scale
            )
        })
        .collect();
    let mut line = format!(
        "accessibility {} · screen recording {} · {} displays: {}",
        tick(s.accessibility),
        tick(s.screen_recording),
        s.displays.len(),
        displays.join(", ")
    );
    if !s.note.is_empty() {
        line.push('\n');
        line.push_str(&s.note);
    }
    line
}

pub(crate) fn send(tx: &UnboundedSender<Command>, cmd: Command) {
    // The receiver lives as long as the UI; a closed channel means shutdown.
    let _ = tx.send(cmd);
}

/// Last 8 chars of a session id, for display (the first 8 of a UUIDv7 are
/// the timestamp and collide across sessions created close together).
pub fn short(id: SessionId) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_owned()
}

/// One row of the `/agents` picker and the board: the supervisor's snapshot
/// plus what the TUI learned from this session's events since (design doc
/// 19 §3.3).
#[derive(Clone, Debug)]
pub struct AgentRow {
    /// The snapshot (`list_agents`) as kept live by `TurnStarted` / `TurnFinished` and friends.
    pub info: AgentInfo,
    /// `preparing <tool>` / `running <tool> <arg>` while a call is in flight; the
    /// tool and its args while an approval is pending. `None` = streaming text.
    pub tool: Option<String>,
    /// First line of the question while `Awaiting`.
    pub question: Option<String>,
    /// Last `Error` message; the detail of an `Error` row.
    pub error: Option<String>,
    /// The turn ended in error / cancelled while this TUI watched and the
    /// picker has not been opened since: the row stays on the board.
    pub fresh: bool,
}

impl AgentRow {
    /// A row with nothing learned yet.
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            tool: None,
            question: None,
            error: None,
            fresh: false,
        }
    }

    /// The agent is in a turn: running, asking, or waiting on an approval.
    pub fn live(&self) -> bool {
        matches!(
            self.info.state,
            AgentState::Running | AgentState::Awaiting { .. } | AgentState::Approval { .. }
        )
    }

    /// Shown on the board: live, or an unacknowledged error / cancel.
    pub fn on_board(&self) -> bool {
        self.live() || self.fresh
    }
}

/// Picker / board order (design doc 19 §3.1): running first in spawn order,
/// then awaiting / approval, then the rest newest-finished first. The id
/// (UUIDv7, time-ordered) breaks ties so a `HashMap`-fed board never jitters.
fn rank(a: &AgentRow) -> (u8, i64, u128) {
    let ms = |t: Option<chrono::DateTime<chrono::Utc>>| t.map_or(0, |t| t.timestamp_millis());
    let id = a.info.id.0.as_u128();
    match a.info.state {
        AgentState::Running => (0, ms(a.info.started_at), id),
        AgentState::Awaiting { .. } | AgentState::Approval { .. } => (1, ms(a.info.started_at), id),
        _ => (2, -ms(a.info.finished_at), id),
    }
}

/// `/agents` picker order; see [`rank`].
pub fn sort_agents(mut list: Vec<AgentRow>) -> Vec<AgentRow> {
    list.sort_by_key(rank);
    list
}

/// One `/agents` picker or board row: `glyph name state detail age [jN pN ·age]`
/// (design doc 19 §3.1, 62 §5.1). `spinner` is the running glyph; depth ≥ 2
/// indents the name under its parent; `width` (terminal columns) bounds the
/// detail column. Age is elapsed while the agent is live, `ago` since it
/// finished otherwise; the trailing counts and `last_active` age come from
/// the snapshot's `activity` and appear only when known.
pub fn agent_row(a: &AgentRow, spinner: &str, width: u16) -> String {
    let glyph = crate::broadcast::glyph(&a.info.state, spinner);
    let name = match a.info.depth {
        0 | 1 => a.info.name.clone(),
        d => format!("{}↳ {}", " ".repeat(2 * usize::from(d - 1)), a.info.name),
    };
    let now = chrono::Utc::now();
    let age = match (a.info.started_at, a.info.finished_at) {
        (Some(t), _) if a.live() => elapsed(now - t),
        (_, Some(t)) => ago(now - t),
        _ => String::new(),
    };
    // Glyph, name, state, gaps, age and the box take ~44 columns; the detail gets the rest.
    let room = usize::from(width).saturating_sub(44).max(16);
    let detail = match &a.info.state {
        AgentState::Running => a.tool.clone().unwrap_or_else(|| "streaming".to_owned()),
        AgentState::Awaiting { from } => match &a.question {
            Some(q) => format!("{from}: \"{q}\""),
            None => from.clone(),
        },
        AgentState::Approval { tool } => a.tool.clone().unwrap_or_else(|| tool.clone()),
        AgentState::Error => a.error.clone().unwrap_or_else(|| a.info.last.clone()),
        AgentState::Cancelled | AgentState::Idle => a.info.last.clone(),
    };
    let detail: String = detail.chars().take(room).collect();
    let active = match crate::broadcast::age(a.info.activity.last_active) {
        s if s.is_empty() => s,
        s => format!(" ·{s}"),
    };
    format!(
        "{glyph} {name:<14} {:<9} {detail:<room$}  {age}{}{active}",
        a.info.state.word(),
        crate::broadcast::counts(&a.info.activity),
    )
}

/// The first string argument of a tool call, cut to 40 chars: the `running
/// read_file crates/…` detail of a row.
fn first_arg(args: &serde_json::Value) -> Option<String> {
    args.as_object()?
        .values()
        .find_map(|v| v.as_str())
        .map(|s| {
            s.lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect()
        })
}

/// Running time: `48s`, `1m32s`, `2h05m`.
fn elapsed(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, _) => format!("{h}h{m:02}m"),
    }
}

/// An RFC 3339 stamp as local `YYYY-MM-DD HH:MM`; anything else verbatim.
pub(crate) fn when(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339).map_or_else(
        |_| rfc3339.to_owned(),
        |t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// Error line for a name the vault would refuse (37 §3.1).
fn bad_secret_name(name: &str) -> String {
    format!("secrets: {name:?} is not a name; use [A-Za-z0-9_.-], at most 64")
}

/// Time since a turn ended, coarse: `48s ago`, `3m ago`, `2h ago`, `3d ago`.
fn ago(d: chrono::Duration) -> String {
    format!("{} ago", crate::broadcast::since(d))
}

/// Shown once per run before the first private session (51 §1).
pub const PRIVATE_WARNING: &str = "private session: its records live in memory only and are gone when you leave or the harness restarts; \
files its tools write are real files under a temporary root, deleted on close; \
provider retention, swap, crash dumps and effects outside the tree are not undone";

/// Nanos as units with three decimals, rounded half up: `0.123`. Integer
/// arithmetic only (51 §6.2: nanos never go through a float).
pub fn money_units(nanos: u64) -> String {
    let millis = (nanos / 1_000_000) + u64::from(nanos % 1_000_000 >= 500_000);
    format!("{}.{:03}", millis / 1000, millis % 1000)
}

/// The display symbol for an ISO 4217 code, from `iso_currency`'s table, or `None`
/// to print the code. The table gives 18 currencies a bare `$` (and several a `£`,
/// `¥`, `kr`), so a symbol is used only when no other currency shares it, or for
/// the three CLDR English also shows bare (USD `$`, GBP `£`, JPY `¥`) — the same
/// result the desktop gets from `Intl` for these, minus its `CA$`-style prefixes.
fn currency_symbol(code: &str) -> Option<String> {
    use iso_currency::{Currency, IntoEnumIterator};
    let c = Currency::from_code(code)?;
    let sym = c.symbol().symbol;
    if sym.is_empty() || sym == "¤" || sym == code {
        return None;
    }
    let shared = Currency::iter()
        .filter(|o| o.symbol().symbol == sym)
        .count()
        > 1;
    (!shared || matches!(code, "USD" | "GBP" | "JPY")).then_some(sym)
}

/// `$0.123 · €0.010 · CAD 0.500`: one entry per currency, as sorted by the owner.
fn money_list(list: &[Money], prefix: &str) -> String {
    list.iter()
        .map(|m| match currency_symbol(&m.currency) {
            Some(s) => format!("{prefix}{s}{}", money_units(m.nanos)),
            None => format!("{prefix}{} {}", m.currency, money_units(m.nanos)),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The display rules of 51 §6.2 over one bucket: `Returned cost —` when
/// nothing was dispatched; `Returned cost $0.123`; partial coverage adds
/// `· N unavailable`; calls the provider did not price but the catalog or
/// adapter could add a second line `Estimated ~$0.023`. Returned and
/// estimated never share a line.
pub fn cost_lines(b: &CostBucket) -> Vec<String> {
    let mut first = String::from("Returned cost ");
    if b.dispatched == 0 || b.returned.is_empty() {
        first.push('—');
    } else {
        first.push_str(&money_list(&b.returned, ""));
    }
    if b.unavailable > 0 {
        first.push_str(&format!(" · {} unavailable", b.unavailable));
    }
    let mut lines = vec![first];
    if !b.estimated_for_unavailable.is_empty() {
        lines.push(format!(
            "Estimated {}",
            money_list(&b.estimated_for_unavailable, "~")
        ));
    }
    lines
}

/// One bucket for a header tag or child row: returned money, else the
/// estimate as `~$0.123` (the tilde keeps its provenance visible), else `—`.
pub fn cost_short(b: &CostBucket) -> String {
    if !b.returned.is_empty() {
        money_list(&b.returned, "")
    } else if !b.estimated_for_unavailable.is_empty() {
        money_list(&b.estimated_for_unavailable, "~")
    } else {
        "—".into()
    }
}

/// One `/terminals` row: `● abcd1234  j3  name · command  running  rev 12`.
pub fn terminal_row(t: &TerminalSummary) -> String {
    let label = match &t.name {
        Some(n) => format!("{n} · {}", t.command),
        None => t.command.clone(),
    };
    let label: String = label.chars().take(48).collect();
    format!(
        "{} {}  {:<4} {:<48} {:<9} rev {}",
        if t.state == "running" { "●" } else { "○" },
        short(t.session),
        t.id.0,
        label,
        t.state,
        t.revision
    )
}

impl Model {
    /// This view's stable id for the private lease (51 §4.1): one per process.
    fn view_id() -> String {
        format!("tui-{}", std::process::id())
    }

    /// `/new private`: release any owned tree first, then ask for a fresh
    /// private root labelled with the shown project (51 §4.1).
    fn new_private(&mut self, tx: &UnboundedSender<Command>) {
        self.leave_private(tx);
        let project = self.project.clone();
        self.reset();
        self.entries
            .push(Entry::Info("opening a private session…".into()));
        send(
            tx,
            Command::CreateSession(CreateSessionOptions {
                retention: Retention::Private,
                project,
                view: Some(Self::view_id()),
                cwd: std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string()),
                ..Default::default()
            }),
        );
    }

    /// Release the owned private tree, if any (51 §4.3). The caller resets
    /// what was shown; the reply says how far cleanup got.
    fn leave_private(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(p) = self.private.take() {
            send(
                tx,
                Command::LeaveSession {
                    session: p.root,
                    secret: p.secret,
                },
            );
        }
    }

    /// What `/artifacts` knows about `id`: a listed durable session, the
    /// shown session or a roster member (the owned private tree included).
    /// `None` is a missing durable owner (51 §3).
    fn owner(&self, sessions: &[SessionSummary], id: SessionId) -> Option<Owner> {
        if let Some(s) = sessions.iter().find(|s| s.id == id) {
            let title: String = s.title.chars().take(40).collect();
            return Some(Owner {
                project: s.project.clone(),
                label: format!("{}  {title}", short(id)),
            });
        }
        if self.session == Some(id) {
            return Some(Owner {
                project: self.project.clone(),
                label: format!(
                    "{}  {}",
                    short(id),
                    self.session_name.as_deref().unwrap_or("this session")
                ),
            });
        }
        self.agents.get(&id).map(|a| Owner {
            project: a.info.project.clone().or_else(|| self.project.clone()),
            label: format!("{}  {}", short(id), a.info.name),
        })
    }

    /// `  ·  cost $0.123` (or `~$0.123` when only an estimate exists) for
    /// the shown session's inclusive total; empty until the owner summary arrived (51 §8.3).
    fn cost_tag(&self) -> String {
        match &self.cost {
            Some(c) => format!("  ·  cost {}", cost_short(&c.total)),
            None => String::new(),
        }
    }

    /// A direct child's inclusive total for its `/agents` row: `  ·  $0.123`
    /// (`~$` for an estimate, `—` when nothing was priced); empty for deeper rows and before the summary arrived.
    pub fn child_cost_tag(&self, id: SessionId) -> String {
        let Some(child) = self
            .cost
            .as_ref()
            .and_then(|c| c.children.iter().find(|ch| ch.session == id))
        else {
            return String::new();
        };
        format!("  ·  {}", cost_short(&child.total))
    }

    /// `/cost` rows (51 §8.3): direct, descendants and total buckets, the
    /// estimate line, unresolved attempts, history coverage, remote-meter
    /// gaps, scope, then one row per immediate child.
    pub fn cost_rows(&self) -> Vec<String> {
        let Some(Menu::Cost { cost: c, .. }) = &self.menu else {
            return Vec::new();
        };
        let bucket = |name: &str, b: &CostBucket| {
            let mut lines = cost_lines(b);
            lines[0] = format!("{name:<12}{}", lines[0]);
            for l in &mut lines[1..] {
                *l = format!("{:<12}{l}", "");
            }
            lines
        };
        let mut rows = bucket("direct", &c.direct);
        rows.extend(bucket("descendants", &c.descendants));
        rows.extend(bucket("total", &c.total));
        rows.push(format!(
            "{:<12}{} calls · {} dispatched · {} unavailable · {} unresolved",
            "attempts", c.total.calls, c.total.dispatched, c.total.unavailable, c.total.unresolved
        ));
        rows.push(format!(
            "{:<12}in {} · out {} · cache read {} · cache write {}{}",
            "tokens",
            k(u32::try_from(c.total.tokens.input).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.output).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.cache_read).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.cache_write).unwrap_or(u32::MAX)),
            match c.total.tokens.unknown_calls {
                0 => String::new(),
                n => format!(" · {n} calls without token counts"),
            }
        ));
        // 75 §7 / 06 §8.4: ACP children's tokens per harness model; a
        // `lower_bound` group (opencode 1.x, #1860) says so.
        for r in &c.remote_tokens {
            rows.push(format!(
                "{:<12}{} · {} turn(s) · in {} · out {} · cache read {} · cache write {}{}",
                "",
                r.model.as_deref().unwrap_or(&r.harness),
                r.turns,
                k(u32::try_from(r.tokens.input).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.output).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.cache_read).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.cache_write).unwrap_or(u32::MAX)),
                if r.lower_bound {
                    " · last request only"
                } else {
                    ""
                }
            ));
        }
        rows.push(format!(
            "{:<12}{}",
            "history",
            match c.history_coverage {
                Coverage::Complete => "complete",
                Coverage::Unknown =>
                    "historical cost unknown (ledger starts after this session did)",
                Coverage::Ambiguous => "ambiguous",
            }
        ));
        if c.remote_meter_gaps > 0 {
            rows.push(format!(
                "{:<12}{} remote meter(s) with incomplete coverage",
                "remote", c.remote_meter_gaps
            ));
        }
        if c.delta_suspected {
            rows.push(format!(
                "{:<12}harness may report per-turn cost — totals may be low",
                "remote"
            ));
        }
        rows.push(format!(
            "{:<12}{}",
            "scope",
            match c.scope {
                CostScope::Native => "native",
                CostScope::OpaqueRemoteInclusive => "includes hidden remote descendants",
            }
        ));
        for ch in &c.children {
            rows.push(format!(
                "  ↳ {} · {}{}{}",
                ch.name.as_deref().unwrap_or("(unnamed)"),
                cost_short(&ch.total),
                if ch.deleted { " · deleted" } else { "" },
                match ch.scope {
                    CostScope::Native => "",
                    CostScope::OpaqueRemoteInclusive => " · remote inclusive",
                }
            ));
        }
        rows
    }

    /// `/cost`: a read-only list; esc closes.
    fn cost_key(&mut self, code: KeyCode) -> MenuKey {
        let rows = self.cost_rows().len();
        let Some(Menu::Cost { cursor, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(rows.saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/terminals`: ⏎ opens the observer on the highlighted job, `r` re-lists.
    fn terminals_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Terminals { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('r') => {
                if let Some(session) = self.session {
                    send(tx, Command::ListTerminals(session));
                }
            }
            KeyCode::Enter => {
                if let Some(t) = items.get(*cursor) {
                    self.terminal_epoch += 1;
                    let mut view = TerminalView::new(t, self.terminal_epoch);
                    if let Some(cmd) = view.poll(now_ms()) {
                        send(tx, cmd);
                    }
                    self.menu = Some(Menu::Terminal(Box::new(view)));
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// The observer's keys (51 §7.4): history scrolling, horizontal pan,
    /// `r` refresh, `i`/⏎ to type into the process (55 §3), esc back to the picker.
    fn terminal_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Terminal(v)) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => v.scroll(-1),
            KeyCode::Down => v.scroll(1),
            KeyCode::PageUp => v.scroll(-i64::from(TERMINAL_PAGE)),
            KeyCode::PageDown => v.scroll(i64::from(TERMINAL_PAGE)),
            KeyCode::Left => v.col = v.col.saturating_sub(DIFF_STEP),
            KeyCode::Right => {
                let cols = v.page.as_ref().map_or(120, |p| usize::from(p.cols));
                v.col = (v.col + DIFF_STEP).min(cols.saturating_sub(DIFF_STEP));
            }
            KeyCode::Char('r') => {
                v.revision = None;
                v.dirty = true;
                v.next_poll = 0;
            }
            // 55 §3: hand the keyboard to the program while it runs.
            KeyCode::Char('i') | KeyCode::Enter
                if v.page.as_ref().is_some_and(|p| p.exit.is_none()) =>
            {
                v.interactive = true;
                v.set_viewport(PtyViewport::Live);
            }
            // Closes the overlay only; the owned tree is untouched (51 §4.3, §7.4).
            KeyCode::Esc => {
                self.menu = None;
                return MenuKey::Consumed;
            }
            _ => {}
        }
        if let Some(cmd) = v.poll(now_ms()) {
            send(tx, cmd);
        }
        MenuKey::Consumed
    }

    /// When the event loop must next wake for the observer (unix ms); `None` while none is open.
    pub fn terminal_wake(&self) -> Option<u64> {
        match &self.menu {
            Some(Menu::Terminal(v)) => v.wake(),
            _ => None,
        }
    }

    /// Send the observer's due poll, if any (51 §7.3: one in flight, one per half second).
    pub fn terminal_tick(&mut self, now: u64, tx: &UnboundedSender<Command>) {
        if let Some(Menu::Terminal(v)) = &mut self.menu
            && let Some(cmd) = v.poll(now)
        {
            send(tx, cmd);
        }
    }

    /// The open observer is forwarding keys to the process (55 §3).
    pub fn terminal_interactive(&self) -> bool {
        matches!(&self.menu, Some(Menu::Terminal(v)) if v.interactive)
    }

    /// A key while interactive: Ctrl-] detaches, everything else is typed
    /// into the process as structured input (`pty.rs` owns the byte table).
    pub fn terminal_type(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        tx: &UnboundedSender<Command>,
    ) {
        let Some(Menu::Terminal(v)) = &mut self.menu else {
            return;
        };
        if code == KeyCode::Char(']') && modifiers.contains(KeyModifiers::CONTROL) {
            v.interactive = false;
            return;
        }
        if let Some((text, keys)) = key_input(code, modifiers) {
            send(
                tx,
                Command::PtyInput {
                    session: v.session,
                    job: v.job.clone(),
                    text,
                    keys,
                },
            );
            v.dirty = true;
            if let Some(cmd) = v.poll(now_ms()) {
                send(tx, cmd);
            }
        }
    }

    /// Pasted text while interactive goes in one piece (bracketed when the
    /// program asked for it).
    pub fn terminal_paste(&mut self, text: String, tx: &UnboundedSender<Command>) {
        if let Some(Menu::Terminal(v)) = &mut self.menu {
            send(
                tx,
                Command::PtyInput {
                    session: v.session,
                    job: v.job.clone(),
                    text: None,
                    keys: vec![Key::Paste(text)],
                },
            );
            v.dirty = true;
        }
    }
}

/// One terminal key as `pty_input` wants it; `None` for keys with no
/// terminal meaning (bare modifiers, media keys).
fn key_input(code: KeyCode, modifiers: KeyModifiers) -> Option<(Option<String>, Vec<Key>)> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    let key = match code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c),
        KeyCode::Char(c) if alt => Key::Alt(c),
        KeyCode::Char(c) => return Some((Some(c.to_string()), Vec::new())),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::ShiftTab,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some((None, vec![key]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn slash_tab_completes_and_enter_asks_for_required_arg() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.sync_slash("/mo");
        assert_eq!(
            m.menu_key(KeyCode::Tab, "/mo", &tx),
            MenuKey::Input("/model ".into())
        );
        assert_eq!(
            m.menu_key(KeyCode::Enter, "/logo", &tx),
            MenuKey::Input("/logout ".into())
        );
        assert!(rx.try_recv().is_err(), "nothing submitted yet");
        // `/q` has one match; Enter runs it and clears the input.
        assert_eq!(
            m.menu_key(KeyCode::Enter, "/q", &tx),
            MenuKey::Input(String::new())
        );
        assert!(m.quit && m.menu.is_none());
        assert_eq!(slash_matches("/", true).len(), COMMANDS.len());
        assert_eq!(slash_matches("/", false).len(), COMMANDS.len() - 1);
        assert!(slash_matches("/zzz", true).is_empty());
    }

    /// 54 §5.2: Ctrl+D needs `Ready`, anchors the caret, tracks only our
    /// id, stops on the second press and splices on `Finished`; Esc restores
    /// `before + after` and the later `Finished` splices nothing.
    #[test]
    fn dictation_lifecycle() {
        use voice::{DictationId, FinishReason, ModelState, VoiceEvent};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        // Not ready: a reason, no command.
        assert!(m.toggle_dictation("a".into(), "b".into()).is_none());
        assert!(matches!(m.entries.last(), Some(Entry::Info(s)) if s.contains("missing")));
        m.apply(UiEvent::Voice(VoiceEvent::DictationModel {
            state: ModelState::Ready,
        }));
        assert!(matches!(
            m.toggle_dictation("hello".into(), "world".into()),
            Some(Command::DictationStart)
        ));
        // A second press while the reply is pending does nothing.
        assert!(m.toggle_dictation(String::new(), String::new()).is_none());
        let id = DictationId(Uuid::now_v7());
        let other = DictationId(Uuid::now_v7());
        m.apply(UiEvent::DictationStarting(id));
        assert!(!m.input_focused() && m.animating());
        assert_eq!(m.dictation.as_ref().map(|d| d.phase), Some(Phase::Starting));
        m.apply(UiEvent::Voice(VoiceEvent::DictationStarted { id: other }));
        assert_eq!(m.dictation.as_ref().map(|d| d.phase), Some(Phase::Starting));
        m.apply(UiEvent::Voice(VoiceEvent::DictationStarted { id }));
        assert_eq!(
            m.dictation.as_ref().map(|d| d.phase),
            Some(Phase::Dictating)
        );
        m.apply(UiEvent::Voice(VoiceEvent::DictationText {
            id: other,
            utterance: 0,
            text: "not ours".into(),
            is_final: true,
        }));
        m.apply(UiEvent::Voice(VoiceEvent::DictationText {
            id,
            utterance: 0,
            text: "this is".into(),
            is_final: false,
        }));
        m.apply(UiEvent::Voice(VoiceEvent::DictationText {
            id,
            utterance: 0,
            text: "this is a test.".into(),
            is_final: true,
        }));
        m.apply(UiEvent::Voice(VoiceEvent::DictationLevel {
            id,
            levels: vec![0.5; 8],
        }));
        assert_eq!(m.dictation.as_ref().unwrap().levels.len(), 8);
        assert!(matches!(
            m.toggle_dictation(String::new(), String::new()),
            Some(Command::DictationStop(i)) if i == id
        ));
        assert_eq!(m.dictation.as_ref().map(|d| d.phase), Some(Phase::Stopping));
        m.apply(UiEvent::Voice(VoiceEvent::DictationFinished {
            id: other,
            reason: FinishReason::Done,
        }));
        assert!(m.dictation.is_some() && m.pending_input.is_none());
        m.apply(UiEvent::Voice(VoiceEvent::DictationFinished {
            id,
            reason: FinishReason::Done,
        }));
        assert!(m.dictation.is_none() && m.input_focused());
        assert_eq!(
            m.pending_input.take(),
            Some((
                "hello this is a test. world".into(),
                "hello this is a test.".len()
            ))
        );

        // Esc: restored at once; `Finished` afterwards changes nothing.
        m.toggle_dictation("hel".into(), "lo".into());
        m.apply(UiEvent::DictationStarting(id));
        m.apply(UiEvent::Voice(VoiceEvent::DictationText {
            id,
            utterance: 0,
            text: "there".into(),
            is_final: false,
        }));
        assert!(matches!(m.discard_dictation(), Some(Command::DictationStop(i)) if i == id));
        assert_eq!(m.pending_input.take(), Some(("hello".into(), 3)));
        m.apply(UiEvent::Voice(VoiceEvent::DictationFinished {
            id,
            reason: FinishReason::Error {
                message: "mic".into(),
            },
        }));
        assert!(m.dictation.is_none() && m.pending_input.is_none());
        assert!(matches!(m.entries.last(), Some(Entry::Error(s)) if s == "dictation: mic"));

        // A refused start clears the anchor; `/dictate` routes to the same toggle.
        m.toggle_dictation("x".into(), String::new());
        m.apply(UiEvent::DictationFailed("already dictating".into()));
        assert!(m.dictation_pending.is_none());
        m.submit("/dictate", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::DictationStart)));
    }

    /// 76 §0.12: `/talk` with no session asks for one (`TalkStart(None)`),
    /// a second press waits, `TalkStarting` opens the view and unfocuses the
    /// editor, Esc leaves the view with the talk on (status shows it), `/talk`
    /// re-enters, Ctrl+T stops; a stranger's `TalkFinished` is ignored, ours
    /// closes the view and an error reason is reported.
    #[test]
    fn talk_lifecycle() {
        use talk::{EndReason, State, TalkEvent, TalkId};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/talk", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::TalkStart(None))));
        assert!(m.toggle_talk(false).is_none());
        let root = SessionId::new();
        m.apply(UiEvent::SessionOpened(SessionOpen {
            session: root,
            root,
            retention: Retention::Durable,
            lease_secret: None,
        }));
        let id = TalkId::new();
        m.apply(UiEvent::TalkStarting(id));
        assert!(m.talk_view && !m.input_focused() && m.animating());
        assert_eq!(m.talk.as_ref().map(|t| t.session), Some(root));
        m.apply(UiEvent::Talk(TalkEvent::TalkState {
            id,
            state: State::Speaking,
        }));
        assert!(!m.status().contains("talk:"));
        // Esc while speaking cancels the turn and keeps the pane; in Listening it leaves.
        assert!(matches!(m.talk_escape(), Some(Command::Cancel(s)) if s == root) && m.talk_view);
        m.apply(UiEvent::Talk(TalkEvent::TalkState {
            id,
            state: State::Listening,
        }));
        assert!(m.talk_escape().is_none() && !m.talk_view);
        m.apply(UiEvent::Talk(TalkEvent::TalkState {
            id,
            state: State::Speaking,
        }));
        assert!(m.status().contains("● talk: speaking (/talk)"));
        assert!(m.toggle_talk(false).is_none() && m.talk_view);
        assert!(matches!(m.toggle_talk(true), Some(Command::TalkStop(i)) if i == id));
        assert!(!m.talk_view && m.talk.is_some());
        m.apply(UiEvent::Talk(TalkEvent::TalkFinished {
            id: TalkId::new(),
            reason: EndReason::Stopped,
        }));
        assert!(m.talk.is_some());
        m.apply(UiEvent::Talk(TalkEvent::TalkFinished {
            id,
            reason: EndReason::Error {
                message: "mic".into(),
            },
        }));
        assert!(m.talk.is_none() && !m.talk_view);
        assert!(matches!(m.entries.last(), Some(Entry::Error(s)) if s == "talk: mic"));
        m.apply(UiEvent::TalkFailed("talk is disabled".into()));
        assert!(!m.talk_pending);
        assert!(matches!(m.toggle_talk(false), Some(Command::TalkStart(Some(s))) if s == root));
    }

    /// An inventory: a user message with two images, an assistant call
    /// row paired with its tool response (each the other's companion, the
    /// response holding a screenshot), a PDF part, and the epoch head.
    fn shake_inventory() -> Vec<ContextItem> {
        use agent::ContextOwner as O;
        let msg = |seq| ContextTarget::Message { seq };
        let bin = |seq, part| ContextTarget::Binary {
            owner: O::Message { seq },
            part,
            image: None,
        };
        let item = |target, kind, label: &str, remove_with: Vec<ContextTarget>| ContextItem {
            target,
            kind,
            label: label.into(),
            remove_with,
        };
        vec![
            item(msg(1), ContextKind::Message, "you: look at these", vec![]),
            item(bin(1, 1), ContextKind::Image, "a.png", vec![]),
            item(bin(1, 2), ContextKind::Image, "b.png", vec![]),
            item(msg(2), ContextKind::Tool, "shell: ls", vec![msg(3)]),
            item(msg(3), ContextKind::Tool, "shell result", vec![msg(2)]),
            item(
                ContextTarget::Binary {
                    owner: O::Message { seq: 3 },
                    part: 0,
                    image: Some(0),
                },
                ContextKind::Image,
                "screen.png",
                vec![],
            ),
            item(bin(4, 0), ContextKind::Document, "report.pdf", vec![]),
            item(
                ContextTarget::Head { epoch: 2 },
                ContextKind::Summary,
                "summary (epoch 2)",
                vec![],
            ),
        ]
    }

    fn snapshot(items: Vec<ContextItem>) -> ContextSnapshot {
        ContextSnapshot {
            revision: "7".into(),
            editable: true,
            items,
            ..ContextSnapshot::default()
        }
    }

    #[test]
    fn expand_follows_companions_and_subsumes_attachments() {
        let items = shake_inventory();
        // Ticking the call row pulls in its response; the response's
        // screenshot is not listed twice (the whole message subsumes it).
        let rows = expand(&items, &BTreeSet::from([3, 5]));
        let targets: Vec<_> = rows.iter().map(|r| (r.target, r.companion)).collect();
        assert_eq!(
            targets,
            vec![
                (ContextTarget::Message { seq: 2 }, false),
                (ContextTarget::Message { seq: 3 }, true),
            ]
        );
        // A ticked image inside a ticked message is one row: the message.
        let rows = expand(&items, &BTreeSet::from([0, 1]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, ContextTarget::Message { seq: 1 });
        // Images alone: exact occurrences, siblings untouched.
        let rows = expand(&items, &BTreeSet::from([1]));
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0].target,
            ContextTarget::Binary { part: 1, .. }
        ));
    }

    #[test]
    fn shake_categories_preselect_and_review_once() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let session = SessionId::new();
        m.session = Some(session);
        // Unknown argument: usage, no request, no chat input.
        m.submit("/shake everything", &tx);
        assert!(matches!(m.entries.last(), Some(Entry::Error(t)) if t == SHAKE_USAGE));
        assert!(rx.try_recv().is_err());
        m.submit("/shake images", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::ContextItems {
                category: Some(ShakeCategory::Images),
                ..
            })
        ));
        m.apply(UiEvent::ContextItems {
            session,
            category: Some(ShakeCategory::Images),
            snapshot: snapshot(shake_inventory()),
        });
        // Every image ticked (the tool screenshot too, the PDF not), straight into review.
        let Some(Menu::Shake(s)) = &m.menu else {
            panic!("picker open");
        };
        assert_eq!(s.stage, ShakeStage::Review);
        assert_eq!(s.review.len(), 3);
        assert!(s.review.iter().all(|r| r.kind == ContextKind::Image));
        assert!(!m.input_focused(), "the picker is modal");
        // Esc backs into the picker (images only listed); a second Esc closes it.
        m.menu_key(KeyCode::Esc, "", &tx);
        let Some(Menu::Shake(s)) = &m.menu else {
            panic!("picker open");
        };
        assert_eq!(s.stage, ShakeStage::Pick);
        assert_eq!(s.shown.len(), 3);
        // Space unticks the highlighted row; ⏎ reviews the rest, ⏎ removes exactly those.
        m.menu_key(KeyCode::Char(' '), "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(rx.try_recv().is_err(), "review is not a removal");
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(m.menu.is_none());
        let Ok(Command::RemoveContext {
            expected_revision,
            targets,
            ..
        }) = rx.try_recv()
        else {
            panic!("removal sent");
        };
        assert_eq!(expected_revision, "7");
        assert_eq!(targets.len(), 2);
        assert!(!targets.contains(&ContextTarget::Binary {
            owner: agent::ContextOwner::Message { seq: 1 },
            part: 1,
            image: None,
        }));
    }

    #[test]
    fn shake_bare_and_messages_tick_nothing_and_zero_matches_write_nothing() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let session = SessionId::new();
        m.session = Some(session);
        m.apply(UiEvent::ContextItems {
            session,
            category: None,
            snapshot: snapshot(shake_inventory()),
        });
        let Some(Menu::Shake(s)) = &m.menu else {
            panic!("picker open");
        };
        assert_eq!(
            (s.stage, s.shown.len(), s.selected.len()),
            (ShakeStage::Pick, 8, 0)
        );
        // ⏎ with nothing ticked stays put; esc closes without a request.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(&m.menu, Some(Menu::Shake(s)) if s.stage == ShakeStage::Pick));
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(m.menu.is_none() && rx.try_recv().is_err());
        m.apply(UiEvent::ContextItems {
            session,
            category: Some(ShakeCategory::Messages),
            snapshot: snapshot(shake_inventory()),
        });
        let Some(Menu::Shake(s)) = &m.menu else {
            panic!("picker open");
        };
        assert!(s.selected.is_empty());
        assert!(
            s.shown
                .iter()
                .all(|&i| s.items[i].kind == ContextKind::Message)
        );
        m.menu = None;
        // No documents in an image-only context: a notice, no picker.
        let images: Vec<_> = shake_inventory()
            .into_iter()
            .filter(|i| i.kind == ContextKind::Image)
            .collect();
        m.apply(UiEvent::ContextItems {
            session,
            category: Some(ShakeCategory::Documents),
            snapshot: snapshot(images),
        });
        assert!(m.menu.is_none());
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t == "no documents in current context")
        );
        // A busy session: the reason, no picker.
        m.apply(UiEvent::ContextItems {
            session,
            category: None,
            snapshot: ContextSnapshot {
                editable: false,
                reason: Some("Session is busy; wait or stop it, then retry.".into()),
                ..snapshot(shake_inventory())
            },
        });
        assert!(m.menu.is_none());
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t.contains("busy")));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn settled_refresh_is_guarded_and_context_changed_sets_the_flag() {
        let (tx, mut rx) = unbounded_channel::<Command>();
        let mut m = Model::default();
        let session = SessionId::new();
        m.session = Some(session);
        m.apply(UiEvent::History(
            session,
            Loaded {
                context_revision: "5".into(),
                context_editable: true,
                ..Loaded::default()
            },
        ));
        // A settled handback with no newer edit changes nothing on screen.
        m.apply(UiEvent::Agent(AgentEvent::ContextChanged {
            session,
            revision: "6".into(),
            reason: "settled".into(),
        }));
        assert!(!m.refresh_context);
        m.apply(UiEvent::Agent(AgentEvent::ContextChanged {
            session,
            revision: "6".into(),
            reason: "context_edit".into(),
        }));
        assert!(m.refresh_context);
        m.refresh_context = false;
        // A busy reply is refused: transcript kept, marked stale, the
        // pending removal notice still shown.
        m.entries.push(Entry::Info("kept".into()));
        m.apply(UiEvent::ContextRemoved(
            session,
            ContextRemoval {
                revision: "6".into(),
                images: 1,
                estimated_tokens: 1200,
                ..ContextRemoval::default()
            },
        ));
        m.apply(UiEvent::Refreshed(
            session,
            Loaded {
                context_revision: "6".into(),
                settled: false,
                context_reason: Some("Session is busy".into()),
                ..Loaded::default()
            },
        ));
        assert!(m.context_stale && !m.context_editable);
        assert_eq!(m.context_revision, "5");
        assert!(matches!(&m.entries[0], Entry::Info(t) if t == "kept"));
        assert!(m.entries.iter().any(
            |e| matches!(e, Entry::Info(t) if t.starts_with("removed from context: 1 image(s)"))
        ));
        // The next settled notification retries while stale.
        m.apply(UiEvent::Agent(AgentEvent::ContextChanged {
            session,
            revision: "6".into(),
            reason: "settled".into(),
        }));
        assert!(m.refresh_context);
        // An older settled snapshot never replaces a newer transcript.
        m.context_revision = "9".into();
        m.apply(UiEvent::Refreshed(
            session,
            Loaded {
                context_revision: "6".into(),
                settled: true,
                ..Loaded::default()
            },
        ));
        assert_eq!(m.context_revision, "9");
        // A newer settled one installs and clears the stale mark.
        m.apply(UiEvent::Refreshed(
            session,
            Loaded {
                context_revision: "10".into(),
                settled: true,
                context_editable: true,
                context_estimate: Some(4200),
                ..Loaded::default()
            },
        ));
        assert!(!m.context_stale && m.context_editable);
        assert_eq!((m.context_revision.as_str(), m.context), ("10", 4200));
        assert!(rx.try_recv().is_err());
        drop(tx);
    }

    #[test]
    fn removed_rows_become_archive_cards_and_attachments_are_labelled() {
        let opts = markdown::Options::default();
        let entry = |removed, attachments| {
            Some(ContextEntry {
                target: ContextTarget::Message { seq: 1 },
                active: true,
                removed,
                remove_with: vec![],
                attachments,
                vision: None,
                at: None,
            })
        };
        let shot = |removed| ContextAttachment {
            target: ContextTarget::Binary {
                owner: agent::ContextOwner::Message { seq: 1 },
                part: 1,
                image: None,
            },
            name: "shot.png".into(),
            mime: "image/png".into(),
            path: None,
            removed,
        };
        let out = from_history(
            HistoryEntry::User {
                text: "secret plan".into(),
                turn: Some(1),
                context: entry(true, vec![shot(false)]),
                peer: None,
                vision: None,
            },
            &opts,
        );
        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0],
            Entry::Card { kind: CardKind::Removed, expanded: false, title, .. } if title.starts_with("you · removed from context")
        ));
        let out = from_history(
            HistoryEntry::User {
                text: "look".into(),
                turn: Some(1),
                context: entry(false, vec![shot(true)]),
                peer: None,
                vision: None,
            },
            &opts,
        );
        assert!(matches!(&out[0], Entry::User { turn: Some(1), .. }));
        assert!(
            matches!(&out[1], Entry::Info(t) if t == "⊘ shot.png (image/png) · removed from context")
        );
        let out = from_history(
            HistoryEntry::Tool {
                call_id: "c1".into(),
                name: "screenshot".into(),
                args: serde_json::json!({}),
                result: Some(("ok".into(), false)),
                files: vec![],
                context: entry(false, vec![shot(false)]),
            },
            &opts,
        );
        assert!(
            matches!(&out[0], Entry::Tool { args, .. } if args.last().is_some_and(|l| l == "⎘ shot.png (image/png)"))
        );
    }

    /// A session search hit for tests.
    fn search_session_hit(session: SessionId, seq: Option<i64>) -> agent::search::SearchHit {
        use ::search::MatchRange;
        use agent::search::{HitKind, HitRole};
        agent::search::SearchHit {
            session,
            title: "deploy checklist".into(),
            kind: if seq.is_some() {
                HitKind::Message
            } else {
                HitKind::Title
            },
            seq,
            role: seq.map(|_| HitRole::Assistant),
            snippet: "deploy checklist".into(),
            ranges: vec![MatchRange { start: 0, end: 6 }],
            score: 1,
            matches: 1,
            created_at: String::new(),
            updated_at: String::new(),
            context_removed: false,
            archived: false,
        }
    }

    /// The reply to a `Command::Search` results request pulled off `rx`.
    fn search_answer(cmd: Command, hits: Vec<agent::search::SearchHit>) -> UiEvent {
        let Command::Search(search::Request { generation, kind }) = cmd else {
            panic!("expected a search request");
        };
        let search::RequestKind::Results {
            categories, limit, ..
        } = kind
        else {
            panic!("expected a results request");
        };
        UiEvent::Search(
            generation,
            search::Reply::Results {
                categories,
                limit,
                result: Ok(rpc::search::SearchResponse {
                    groups: vec![rpc::search::SearchGroup::Sessions {
                        page: ::search::SearchPage::ready(hits, 10),
                    }],
                }),
            },
        )
    }

    #[test]
    fn search_opens_live_menu_and_enter_on_a_title_hit_switches_session() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        // Blank `/search` opens the menu and asks only for recent sessions.
        m.submit("/search", &tx);
        assert!(m.search_open() && !m.input_focused());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Search(search::Request {
                kind: search::RequestKind::Recent,
                ..
            }))
        ));
        m.search_key(KeyCode::Esc, 80, 10, &tx);
        assert!(m.menu.is_none());
        // A prefilled query runs at once, all six chips at limit 10.
        m.submit("/search deploy", &tx);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            &cmd,
            Command::Search(search::Request { kind: search::RequestKind::Results { query, categories, limit: 10 }, .. })
                if query == "deploy" && categories.len() == search::N
        ));
        let hit = SessionId::new();
        m.apply(search_answer(cmd, vec![search_session_hit(hit, None)]));
        assert!(matches!(
            m.search.rows.get(m.search.cursor),
            Some(search::Row::Hit(0, 0))
        ));
        // Enter on a title hit is a session switch, the `/sessions` path.
        m.search_key(KeyCode::Enter, 80, 10, &tx);
        assert!(m.menu.is_none());
        assert_eq!(m.session, Some(hit));
        assert!(matches!(rx.try_recv(), Ok(Command::Load { session, .. }) if session == hit));
        // Chips survive the reset a session switch performs.
        m.search.selected = [true, false, false, false, false, false];
        m.resume(SessionId::new(), &tx);
        assert_eq!(m.search.selected, [true, false, false, false, false, false]);
    }

    /// 66 §10: `/attach q` opens the search menu in pick mode; Enter on a hit
    /// holds a `Paste::Ref`, hands its token to the editor, and the send puts
    /// the marker line first with the token gone. The same pick twice is one ref.
    #[test]
    fn attach_picks_a_hit_and_sends_its_marker_first() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/attach deploy", &tx);
        assert!(m.search_open() && m.attach_open());
        let cmd = rx.try_recv().unwrap();
        let hit = SessionId::new();
        m.apply(search_answer(cmd, vec![search_session_hit(hit, None)]));
        m.search_key(KeyCode::Enter, 80, 10, &tx);
        assert!(m.menu.is_none(), "the menu closes on a pick");
        assert_eq!(m.session, None, "a pick is not a session switch");
        let token = m.attach_token.take().expect("a token for the editor");
        assert_eq!(token, "[⎘ attach 1: deploy checklist]");
        assert_eq!(m.pastes.len(), 1);
        // Attaching the same thing again is a no-op returning the same token.
        let again = m.attach_ref(crate::attach::from_hit(&search::Hit::Session(
            search_session_hit(hit, None),
        )));
        assert_eq!((again, m.pastes.len()), (token.clone(), 1));

        m.submit(&format!("{token} pick up where this left off"), &tx);
        let Ok(Command::Send { text, files, .. }) = rx.try_recv() else {
            panic!("no send");
        };
        assert!(files.is_empty());
        let parsed = agent::attach::parse(&text);
        assert_eq!(parsed.refs.len(), 1);
        assert_eq!(parsed.refs[0].kind, agent::attach::Kind::Session);
        assert_eq!(parsed.refs[0].id, hit.to_string());
        assert_eq!(parsed.refs[0].title, "deploy checklist");
        assert_eq!(parsed.body, "pick up where this left off");
        assert!(m.pastes.is_empty());
        // The transcript card shows the reference as a `⎘` line, not the marker.
        let shown = crate::attach::display(&text);
        assert!(
            shown.starts_with("⎘ deploy checklist (session)\n"),
            "{shown}"
        );
    }

    #[test]
    fn message_hit_opens_an_isolated_earlier_messages_page() {
        use agent::{ContextEntry, ContextTarget, HistoryEntry};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let live = SessionId::new();
        m.session = Some(live);
        m.submit("/search deploy", &tx);
        let cmd = rx.try_recv().unwrap();
        m.apply(search_answer(cmd, vec![search_session_hit(live, Some(7))]));
        m.search_key(KeyCode::Enter, 80, 10, &tx);
        // The current session too: a page ending at the match, not a resume.
        let Ok(Command::Search(search::Request {
            generation,
            kind: search::RequestKind::Page { session, seq: 7 },
        })) = rx.try_recv()
        else {
            panic!("expected a page request");
        };
        assert_eq!(session, live);
        let row = |seq: i64, text: &str| HistoryEntry::Assistant {
            text: text.into(),
            turn: None,
            context: Some(ContextEntry {
                target: ContextTarget::Message { seq },
                active: true,
                removed: false,
                remove_with: vec![],
                attachments: vec![],
                vision: None,
                at: None,
            }),
        };
        let loaded = Loaded {
            history: vec![
                row(5, "older"),
                row(7, "the deploy checklist"),
                row(8, "after"),
            ],
            ..Loaded::default()
        };
        m.apply(UiEvent::Search(
            generation,
            search::Reply::Page {
                session: live,
                seq: 7,
                result: Ok(loaded),
            },
        ));
        assert!(m.menu.is_none());
        let inspect = m.inspect.clone().expect("inspection view");
        assert!(inspect.found);
        let marker = m
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Info(t) if t.contains("seq 7")))
            .expect("match marker");
        assert_eq!(
            m.anchor,
            Some((marker, 0)),
            "the match is revealed at the top"
        );
        assert!(!m.input_focused(), "no composer over an incomplete context");
        // Live events for the session do not stitch into the page.
        let before = m.entries.len();
        m.apply(UiEvent::Agent(AgentEvent::TextDelta {
            session: live,
            turn: TurnId::new(),
            text: "streamed".into(),
        }));
        assert_eq!(m.entries.len(), before);
        // Jump to latest loads the normal tail.
        m.jump_latest(&tx);
        assert!(m.inspect.is_none());
        assert!(
            matches!(rx.try_recv(), Ok(Command::Load { session, settled_only: false }) if session == live)
        );
    }

    #[test]
    fn back_to_results_restores_the_view_and_a_stale_reply_after_escape_is_dropped() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/search deploy", &tx);
        let cmd = rx.try_recv().unwrap();
        let Command::Search(search::Request { generation, .. }) = &cmd else {
            panic!()
        };
        let generation = *generation;
        m.search_key(KeyCode::Esc, 80, 10, &tx);
        assert!(m.menu.is_none());
        m.apply(search_answer(
            cmd,
            vec![search_session_hit(SessionId::new(), None)],
        ));
        assert!(
            m.search.groups.is_empty(),
            "an answer to a closed view never lands"
        );
        assert!(m.entries.iter().all(|e| !matches!(e, Entry::Error(_))));
        // An error for that generation is inert too, not a transcript line.
        m.apply(UiEvent::Search(
            generation,
            search::Reply::Results {
                categories: vec![SearchCategory::Sessions],
                limit: 10,
                result: Err("late".into()),
            },
        ));
        assert!(m.entries.iter().all(|e| !matches!(e, Entry::Error(_))));
        // Back to results reopens the menu with the query kept.
        m.back_to_results(&tx);
        assert!(m.search_open());
        assert_eq!(m.search.query, "deploy");
    }

    /// `/broadcast`: the picker pre-ticks running roots and groups children
    /// under them; ⏎ composes, ⏎ asks for the dry run, ⏎ on the review sends
    /// the same request for real; the receipt closes the menu and prints.
    #[test]
    fn broadcast_picker_composes_reviews_and_sends() {
        use agent::{BroadcastOutcome, BroadcastRow, SessionActivity};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let info = |name: &str, parent: Option<SessionId>, state: AgentState, ptys: u16| {
            let id = SessionId::new();
            AgentInfo {
                id,
                name: name.into(),
                parent: parent.unwrap_or(id),
                depth: u8::from(parent.is_some()),
                agent: None,
                model: "m".into(),
                state: state.clone(),
                started_at: None,
                finished_at: None,
                last: String::new(),
                project: None,
                retention: agent::Retention::Durable,
                activity: SessionActivity {
                    state,
                    ptys,
                    live: true,
                    ..SessionActivity::default()
                },
                cap: None,
                kind: Default::default(),
            }
        };
        let busy = info("busy", None, AgentState::Running, 0);
        let idle = info("idle", None, AgentState::Idle, 0);
        let pty = info("pty", None, AgentState::Idle, 1);
        let child = info("kid", Some(busy.id), AgentState::Running, 0);
        // Children arrive out of order; the picker regroups them under their root.
        m.submit("/broadcast", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListActivity)));
        m.apply(UiEvent::Activity(vec![
            child.clone(),
            busy.clone(),
            idle.clone(),
            pty.clone(),
        ]));
        let Some(Menu::Broadcast(b)) = &m.menu else {
            panic!("picker open");
        };
        let names: Vec<&str> = b.rows.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["busy", "kid", "idle", "pty"]);
        assert_eq!(b.selected, [busy.id, pty.id].into_iter().collect());
        assert!(b.rows("●")[1].contains("↳ kid"));
        // `a` ticks every root, `n` none, `r` the running ones again; `d` flips delivery.
        m.menu_key(KeyCode::Char('a'), "", &tx);
        m.menu_key(KeyCode::Char('n'), "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Char('r'), "", &tx);
        m.menu_key(KeyCode::Char('d'), "", &tx);
        let Some(Menu::Broadcast(b)) = &m.menu else {
            panic!("still picking");
        };
        assert_eq!(b.stage, BroadcastStage::Pick, "⏎ with nothing ticked stays");
        assert_eq!(b.selected.len(), 2);
        assert_eq!(b.delivery, agent::Delivery::NextTurn);
        m.menu_key(KeyCode::Enter, "", &tx);
        for c in "hi all".chars() {
            m.menu_key(KeyCode::Char(c), "", &tx);
        }
        m.menu_key(KeyCode::Enter, "", &tx);
        let Ok(Command::Broadcast(req)) = rx.try_recv() else {
            panic!("dry run sent");
        };
        assert!(req.dry_run && req.text == "hi all");
        assert_eq!(
            req.targets,
            BroadcastTargets::Sessions {
                ids: vec![busy.id, pty.id]
            }
        );
        let row = |a: &AgentInfo, outcome| BroadcastRow {
            session: a.id,
            title: a.name.clone(),
            parent: None,
            before: a.activity.clone(),
            outcome,
        };
        let receipt = |outcomes: [BroadcastOutcome; 2]| BroadcastReceipt {
            id: Uuid::new_v4(),
            at: chrono::Utc::now(),
            rows: {
                let [x, y] = outcomes;
                vec![row(&busy, x), row(&pty, y)]
            },
        };
        let skip = || BroadcastOutcome::Skipped {
            reason: "dry run".into(),
        };
        m.apply(UiEvent::Broadcasted(Ok(receipt([skip(), skip()]))));
        let Some(Menu::Broadcast(b)) = &m.menu else {
            panic!("review open");
        };
        assert_eq!(b.stage, BroadcastStage::Review);
        let rows = b.rows("●");
        assert!(rows[0].starts_with("[broadcast … · 2 sessions]"));
        assert!(rows[2].ends_with("will queue (next turn)"), "{}", rows[2]);
        // A refusal stays on the menu; ⏎ then sends for real.
        m.menu_key(KeyCode::Enter, "", &tx);
        let Ok(Command::Broadcast(req)) = rx.try_recv() else {
            panic!("send");
        };
        assert!(!req.dry_run && req.delivery == agent::Delivery::NextTurn);
        m.apply(UiEvent::Broadcasted(Err("too many targets".into())));
        assert!(
            matches!(&m.menu, Some(Menu::Broadcast(b)) if b.error.as_deref() == Some("too many targets"))
        );
        m.apply(UiEvent::Broadcasted(Ok(receipt([
            BroadcastOutcome::Started,
            BroadcastOutcome::Queued {
                input_id: InputId::new(),
                delivery: agent::Delivery::NextTurn,
            },
        ]))));
        assert!(m.menu.is_none());
        let last = m.entries.iter().rev().find_map(|e| match e {
            Entry::Info(t) => Some(t.as_str()),
            _ => None,
        });
        assert_eq!(last, Some("sent to 2 (1 started, 1 queued, 0 skipped)"));
    }

    #[test]
    fn agents_picker_sorts_opens_and_parent_climbs() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let root = SessionId::new();
        m.session = Some(root);
        let info = |name: &str, state: AgentState, finished: Option<i64>| AgentInfo {
            id: SessionId::new(),
            name: name.into(),
            parent: root,
            depth: 1,
            agent: None,
            model: "m".into(),
            state,
            started_at: None,
            finished_at: finished.map(|s| chrono::Utc::now() - chrono::Duration::seconds(s)),
            last: String::new(),
            project: None,
            retention: agent::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
        };
        let list = vec![
            info("old", AgentState::Idle, Some(600)),
            info("new", AgentState::Error, Some(5)),
            info(
                "ask",
                AgentState::Awaiting {
                    from: "parent".into(),
                },
                None,
            ),
            info("run", AgentState::Running, None),
        ];
        // A seeding reply (open/resume) only fills the map.
        m.apply(UiEvent::Agents(list.clone()));
        assert!(m.menu.is_none() && m.agents.len() == 4);
        // `/agents` opens the picker: running, awaiting, then newest-finished first.
        m.submit("/agents", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListAgents(s)) if s == root));
        m.apply(UiEvent::Agents(list));
        let Some(Menu::Agents { items, .. }) = &m.menu else {
            panic!("picker open");
        };
        let names: Vec<&str> = items.iter().map(|a| a.info.name.as_str()).collect();
        assert_eq!(names, ["run", "ask", "new", "old"]);
        let run = items[0].info.id;
        assert!(agent_row(&items[2], "⠹", 80).starts_with("✗ new            error"));
        // Enter is a session switch: load + reseed.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert_eq!(m.session, Some(run));
        assert!(matches!(rx.try_recv(), Ok(Command::Load { session, .. }) if session == run));
        assert!(matches!(rx.try_recv(), Ok(Command::ListAgents(s)) if s == run));
        // `/parent` goes where `Loaded.parent` says; at the top it says so.
        m.apply(UiEvent::History(
            run,
            Loaded {
                name: Some("run".into()),
                parent: Some(root),
                ..Loaded::default()
            },
        ));
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        assert!(m.header(&catalog).0.starts_with("↳ run  child · "));
        m.submit("/parent", &tx);
        assert_eq!(m.session, Some(root));
        assert!(matches!(rx.try_recv(), Ok(Command::Load { session, .. }) if session == root));
        while rx.try_recv().is_ok() {}
        m.submit("/parent", &tx);
        assert!(rx.try_recv().is_err());
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t == "this is the top-level session")
        );
    }

    #[test]
    fn board_tracks_child_events_and_interjects() {
        let mut m = Model::default();
        let root = SessionId::new();
        m.session = Some(root);
        let (a, b) = (SessionId::new(), SessionId::new());
        for (child, parent, name) in [(a, root, "a"), (b, a, "b")] {
            m.apply_agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name: name.into(),
                cap: None,
            });
        }
        assert!(m.animating() && m.board().len() == 2 && !m.status().contains("agents:"));
        let turn = TurnId::new();
        let call = |name: &str, args: serde_json::Value| AgentEvent::ToolCallStarted {
            session: a,
            turn,
            call_id: "c1".into(),
            name: name.into(),
            args,
        };
        m.apply_agent(call(
            "read_file",
            serde_json::json!({ "path": "crates/agent/src/lib.rs" }),
        ));
        assert!(
            agent_row(&m.agents[&a], "⠹", 80).contains("running   running read_file crates/agent")
        );
        // A blocking `send_message` is the `awaiting` state, question attached.
        m.apply_agent(call(
            "send_message",
            serde_json::json!({ "to": "parent", "message": "which colour?\nteal or blue", "await_reply": true }),
        ));
        assert!(
            matches!(&m.agents[&a].info.state, AgentState::Awaiting { from } if from == "parent")
        );
        assert!(agent_row(&m.agents[&a], "⠹", 80).contains("awaiting  parent: \"which colour?\""));
        m.apply_agent(AgentEvent::ToolCallFinished {
            session: a,
            turn,
            call_id: "c1".into(),
            output: "teal".into(),
            is_error: false,
            files: Vec::new(),
        });
        assert_eq!(m.agents[&a].info.state, AgentState::Running);
        assert!(agent_row(&m.agents[&a], "⠹", 80).contains("streaming"));
        // A direct child's error interjects; a grandchild's only marks its row.
        for (s, msg) in [(a, "429 rate limited"), (b, "deep")] {
            m.apply_agent(AgentEvent::Error {
                session: s,
                turn: Some(turn),
                message: msg.into(),
            });
        }
        let errors: Vec<&str> = m
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Error(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(errors, ["↳ a error: 429 rate limited"]);
        assert_eq!(m.agents[&b].error.as_deref(), Some("deep"));
        // Cancelled: interject, stays on the board until the picker shows it.
        m.apply_agent(AgentEvent::TurnFinished {
            session: a,
            turn,
            stop_reason: "cancelled".into(),
            usage: Usage::default(),
            context: 0,
        });
        assert_eq!(m.agents[&a].info.state, AgentState::Cancelled);
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "↳ a cancelled"));
        assert!(m.board().iter().any(|r| r.info.id == a));
        m.agents_picker_pending = true;
        m.apply(UiEvent::Agents(
            m.agents.values().map(|r| r.info.clone()).collect(),
        ));
        assert!(matches!(m.menu, Some(Menu::Agents { .. })));
        assert!(m.board().iter().all(|r| r.info.id != a) && m.agents[&b].error.is_some());
    }

    #[test]
    fn mesh_devices_command_mirrors_the_cli() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/mesh devices", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::MeshDevicesList)));
        m.submit("/mesh devices add", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::MeshDevicesAdd(None))));
        m.submit("/mesh devices add work phone", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::MeshDevicesAdd(Some(n))) if n == "work phone"));
        m.submit("/mesh devices rm abcd1234", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::MeshDevicesRevoke(id)) if id == "abcd1234"));
        m.submit("/mesh devices name abcd1234 Sky's phone", &tx);
        assert!(
            matches!(rx.try_recv(), Ok(Command::MeshDevicesRename(id, n)) if id == "abcd1234" && n == "Sky's phone")
        );
        // Anything but `devices` is usage, sent nowhere.
        m.submit("/mesh status", &tx);
        assert!(rx.try_recv().is_err());
        assert!(
            matches!(m.entries.last(), Some(Entry::Error(e)) if e.starts_with("usage: /mesh devices"))
        );

        let now = now_ms();
        let row = link::DeviceInfo {
            id: "abcd1234".into(),
            name: "phone".into(),
            platform: "ios".into(),
            paired_ms: now - 86_400_000,
            last_seen_ms: 0,
            revoked: false,
        };
        assert_eq!(
            mesh_device_line(&row, now),
            "abcd1234  phone  ios  paired 1d ago · never seen"
        );
        let revoked = link::DeviceInfo {
            revoked: true,
            last_seen_ms: now - 3_600_000,
            ..row.clone()
        };
        assert!(mesh_device_line(&revoked, now).ends_with("· revoked"));
        m.apply(UiEvent::MeshDevices(vec![]));
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(s)) if s == "mesh devices: none paired")
        );
        m.apply(UiEvent::MeshDevicePairing(link::PairingPayload {
            string: "https://relay.example/link#1.AAAA.d29yaw.QWJDLV8xMjM".into(),
            expires_ms: now + 300_000,
            pending_id: "AbC-_123".into(),
        }));
        assert!(m.invite.is_some());
        assert!(
            matches!(m.entries.last(), Some(Entry::Card { kind: CardKind::Node, title, .. }) if title.ends_with("pending AbC-_123"))
        );
    }

    #[test]
    fn link_menu_pairs_and_card_follows_events() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let device = link::Device {
            id: "abcd1234".into(),
            name: "phone".into(),
            platform: "ios".into(),
            paired_at_ms: 1,
            last_seen_ms: 0,
            online: false,
        };
        let status = link::LinkStatus {
            enabled: true,
            relay: "wss://relay.example:8787".into(),
            name: "Studio".into(),
            harness_id: Some("HsAbCdEf0123".into()),
            connected: true,
            devices: vec![device.clone()],
            pairing: None,
            mesh: None,
        };
        // Bare `/link` asks, the reply opens the modal; `/link off` alone only reports.
        m.submit("/link", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::LinkStatus)));
        m.apply(UiEvent::Link(status.clone()));
        assert!(matches!(m.menu, Some(Menu::Link { .. })) && !m.input_focused());
        assert!(
            m.link_title()
                .starts_with("link · relay.example:8787 · connected · Studio (HsAbCdEf…)")
        );
        assert!(m.link_rows()[0].starts_with("○ phone"));
        // `x` confirms through a prompt; `n` keeps.
        m.menu_key(KeyCode::Char('x'), "", &tx);
        assert!(matches!(m.prompt, Some(Prompt::LinkRevoke { .. })) && m.menu.is_none());
        m.submit("n", &tx);
        assert!(rx.try_recv().is_err() && m.prompt.is_none());
        // `p` pairs: the card counts down, then reads `paired: <name>`.
        m.menu = Some(Menu::Link { cursor: 0 });
        m.menu_key(KeyCode::Char('p'), "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::LinkPair)));
        m.apply(UiEvent::Pairing(link::Pairing {
            key: "https://relay.example:8787/link#1.AAAA.QQ".into(),
            expires_at_ms: now_ms() + 300_000,
        }));
        assert!(m.animating());
        let body = |m: &Model| match m.entries.last() {
            Some(Entry::Card {
                kind: CardKind::Link,
                text,
                ..
            }) => {
                text.view().plain.to_owned()
                    + &text
                        .view()
                        .lines()
                        .flat_map(|l| l.iter().map(|s| s.text.clone()))
                        .collect::<String>()
            }
            _ => panic!("no pairing card"),
        };
        assert!(body(&m).contains("expires in 4:5") || body(&m).contains("expires in 5:00"));
        m.apply(UiEvent::LinkEvent(link::LinkEvent::LinkPaired {
            device: link::Device {
                id: "zzzz9999".into(),
                name: "Sky's iPhone".into(),
                ..device
            },
        }));
        assert!(!m.animating());
        assert!(body(&m).contains("paired: Sky's iPhone"));
        assert_eq!(m.link.as_ref().map(|s| s.devices.len()), Some(2));
        // `/link off` with the menu closed: a one-line notice, no menu.
        m.menu = None;
        m.submit("/link off", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::LinkEnable(false))));
        m.apply(UiEvent::Link(link::LinkStatus {
            enabled: false,
            ..status
        }));
        assert!(m.menu.is_none());
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "link off"));
    }

    #[test]
    fn update_ready_tags_header_and_lines_once() {
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let mut m = Model::new("m".into(), true);
        let ev = update::UpdateReady {
            version: "0.2.0".into(),
            applied: true,
        };
        m.apply(UiEvent::UpdateReady(ev.clone()));
        m.apply(UiEvent::UpdateReady(ev));
        assert!(m.header(&catalog).0.ends_with("  ·  0.2.0 ready"));
        let lines = m
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Info(t) if t.contains("is installed")))
            .count();
        assert_eq!(lines, 1);
        assert!(m.menu.is_none());
    }

    #[test]
    fn nodes_arg_parser_maps_verbs_and_rejects_unknown() {
        assert_eq!(parse_nodes_arg(""), Some(NodesArg::Menu));
        assert_eq!(
            parse_nodes_arg("add https://r/link#1.AAAA.QQ"),
            Some(NodesArg::Add("https://r/link#1.AAAA.QQ".into()))
        );
        assert_eq!(
            parse_nodes_arg("rm build-box"),
            Some(NodesArg::Remove("build-box".into()))
        );
        assert_eq!(
            parse_nodes_arg("name abcd  Build Box "),
            Some(NodesArg::Rename("abcd".into(), "Build Box".into()))
        );
        assert_eq!(
            parse_nodes_arg("invite mac"),
            Some(NodesArg::Invite("mac".into()))
        );
        for bad in ["add", "rm", "name", "name only", "invite", "frob x"] {
            assert_eq!(parse_nodes_arg(bad), None, "{bad}");
        }
    }

    #[test]
    fn find_node_prefers_exact_name_then_unique_id_prefix() {
        let node = |id: &str, name: &str| nodes::Node {
            id: id.into(),
            name: name.into(),
            relay: "wss://r".into(),
            platform: "linux".into(),
            version: "0.4.1".into(),
            paired_at_ms: 1,
            last_seen_ms: 0,
            online: false,
            revoked: false,
            children: 0,
        };
        let list = [
            node("abc1", "build-box"),
            node("abd2", "abc1"),
            node("zzz3", "mac"),
        ];
        assert_eq!(
            find_node(&list, "abc1").map(|n| &n.id),
            Some(&"abd2".to_owned())
        );
        assert_eq!(
            find_node(&list, "zz").map(|n| &n.name),
            Some(&"mac".to_owned())
        );
        assert!(find_node(&list, "ab").is_none(), "ambiguous prefix");
        assert!(find_node(&list, "nope").is_none());
    }

    #[test]
    fn nodes_menu_rows_keys_and_invite_card() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let node = nodes::Node {
            id: "abcd1234".into(),
            name: "build-box".into(),
            relay: "wss://relay.example".into(),
            platform: "linux".into(),
            version: "0.4.1".into(),
            paired_at_ms: now_ms() - 3 * 86_400_000,
            last_seen_ms: now_ms(),
            online: true,
            revoked: false,
            children: 2,
        };
        let offline = nodes::Node {
            id: "zzzz9999".into(),
            name: "mac-mini".into(),
            online: false,
            children: 0,
            last_seen_ms: now_ms() - 7_200_000,
            ..node.clone()
        };
        // Bare `/nodes` asks; the reply opens the modal.
        m.submit("/nodes", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListNodes)));
        m.apply(UiEvent::Nodes(vec![node.clone(), offline]));
        assert!(matches!(m.menu, Some(Menu::Nodes { .. })) && !m.input_focused());
        assert!(m.nodes_title().starts_with("nodes · 2 paired"));
        let rows = m.nodes_rows();
        assert!(
            rows[0].starts_with("● build-box") && rows[0].contains("2 running   paired 3d ago")
        );
        assert!(rows[1].starts_with("○ mac-mini") && rows[1].ends_with("seen 2h ago"));
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        assert!(m.header(&catalog).0.ends_with("  ·  nodes ●1"));
        // `x` confirms through a prompt; `n` keeps.
        m.menu_key(KeyCode::Char('x'), "", &tx);
        assert!(matches!(m.prompt, Some(Prompt::NodeRemove { .. })) && m.menu.is_none());
        m.submit("n", &tx);
        assert!(rx.try_recv().is_err() && m.prompt.is_none());
        // `a` prompts for the key; the reply reopens the menu.
        m.menu = Some(Menu::Nodes { cursor: 0 });
        m.menu_key(KeyCode::Char('a'), "", &tx);
        assert!(matches!(m.prompt, Some(Prompt::NodeKey)));
        m.submit(" https://relay.example/link#1.AAAA.QQ ", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::AddNode(k, None)) if k == "https://relay.example/link#1.AAAA.QQ"
        ));
        m.apply(UiEvent::Nodes(vec![node.clone()]));
        assert!(matches!(m.menu, Some(Menu::Nodes { .. })));
        // `i` invites: a node card counting down, untouched by link pairing events.
        m.menu_key(KeyCode::Char('i'), "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::InviteNode(id)) if id == "abcd1234"));
        m.apply(UiEvent::NodeInvite(
            "build-box".into(),
            link::Pairing {
                key: "https://relay.example/link#1.BBBB.QQ".into(),
                expires_at_ms: now_ms() + 300_000,
            },
        ));
        assert!(m.animating());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Card { kind: CardKind::Node, title, .. }) if title == "node invite · build-box"
        ));
        m.apply(UiEvent::LinkEvent(link::LinkEvent::LinkPairing {
            key: None,
            expires_at_ms: None,
        }));
        assert!(
            m.animating(),
            "a link pairing ending leaves the node invite live"
        );
        // Any node event re-asks the list; verbs resolve later, in the command task.
        m.apply(UiEvent::NodeEvent(nodes::NodeEvent::NodeRemoved {
            id: "zzzz9999".into(),
        }));
        assert!(std::mem::take(&mut m.refresh_nodes));
        m.menu = None;
        m.submit("/nodes rm mac-mini", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::RemoveNode(n)) if n == "mac-mini"));
        m.submit("/nodes frob", &tx);
        assert!(
            matches!(m.entries.last(), Some(Entry::Error(e)) if e.starts_with("usage: /nodes"))
        );
    }

    #[test]
    fn provider_add_wizard_then_one_shot() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/provider add", &tx);
        assert!(matches!(m.prompt, Some(Prompt::ProviderName)));
        assert!(rx.try_recv().is_err());
        m.submit("rwx", &tx);
        assert!(matches!(m.prompt, Some(Prompt::ProviderUrl { .. })));
        m.submit("https://api.ai.rwx.dev/v1", &tx);
        assert!(matches!(m.prompt, Some(Prompt::ProviderKey { .. })));
        m.submit("sk-secret", &tx);
        assert!(m.prompt.is_none());
        let Ok(Command::AddProvider {
            name,
            base_url,
            key,
        }) = rx.try_recv()
        else {
            panic!("wizard did not send AddProvider");
        };
        assert_eq!(
            (name.as_str(), base_url.as_str(), key.as_str()),
            ("rwx", "https://api.ai.rwx.dev/v1", "sk-secret")
        );
        let leaked = m.entries.iter().any(|e| match e {
            Entry::Info(t) | Entry::Error(t) | Entry::User { text: t, .. } => {
                t.contains("sk-secret")
            }
            _ => false,
        });
        assert!(!leaked, "api key must not land in the transcript");
        m.submit("/provider add rwx https://x/v1 sk-one", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::AddProvider { key, .. }) if key == "sk-one"
        ));
        m.submit("/provider", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListProviders)));
        m.submit("/provider rm rwx", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::RemoveProvider(n)) if n == "rwx"));
        m.submit("/provider add", &tx);
        m.cancel_prompt();
        assert!(m.prompt.is_none());
    }

    /// A vault listing with one two-version secret.
    fn secrets_list() -> secrets::SecretsList {
        let version = |v: u32, source: &str| secrets::VersionInfo {
            v,
            created: format!("2026-09-1{v}T10:02:11Z"),
            source: source.into(),
        };
        secrets::SecretsList {
            key_store: "keychain".into(),
            secrets: vec![secrets::SecretInfo {
                name: "github".into(),
                note: "PAT, repo scope".into(),
                source: "local".into(),
                current: 2,
                versions: vec![version(2, "user"), version(1, "import")],
                updated: "2026-09-12T10:02:11Z".into(),
            }],
        }
    }

    /// No transcript line holds `value`.
    fn leaked(m: &Model, value: &str) -> bool {
        m.entries.iter().any(|e| match e {
            Entry::Info(t) | Entry::Error(t) | Entry::User { text: t, .. } => t.contains(value),
            _ => false,
        })
    }

    #[test]
    fn secrets_add_and_request_never_show_the_value() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        // `/secrets add`: name → masked value → note; the note may be empty.
        m.submit("/secrets add", &tx);
        assert!(matches!(m.prompt, Some(Prompt::SecretName)));
        assert!(!m.prompt.as_ref().unwrap().masked());
        m.submit("bad name", &tx);
        assert!(matches!(m.prompt, Some(Prompt::SecretName)));
        m.submit("github", &tx);
        assert!(matches!(
            m.prompt,
            Some(Prompt::SecretValue { current: None, .. })
        ));
        assert!(m.prompt.as_ref().unwrap().masked());
        m.submit("ghp_topsecret", &tx);
        assert!(matches!(m.prompt, Some(Prompt::SecretNote { .. })));
        m.submit("", &tx);
        assert!(m.prompt.is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetSecret { name, value, note: None }) if name == "github" && value == "ghp_topsecret"
        ));
        let mut list = secrets_list();
        list.secrets[0].current = 1;
        list.secrets[0].versions.truncate(1);
        m.apply(UiEvent::SecretSet(list.secrets[0].clone()));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "github: v1 set"));
        // Once listed, adding to a known name says which version it replaces.
        m.apply(UiEvent::Secrets(list));
        m.submit("/secrets add github", &tx);
        assert!(
            matches!(
                &m.prompt,
                Some(Prompt::SecretValue {
                    current: Some(1),
                    ..
                })
            ) && m
                .prompt
                .as_ref()
                .unwrap()
                .hint()
                .contains("replaces v1 → v2")
        );
        m.submit("ghp_second", &tx);
        m.submit("rotated", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetSecret { note: Some(n), .. }) if n == "rotated"
        ));
        // `secret_request`: ours opens the masked box; a stranger's does not.
        let session = SessionId::new();
        m.session = Some(session);
        let id: secrets::RequestId =
            serde_json::from_str("\"6f1a2b3c-4d5e-4f60-8a9b-0c1d2e3f4a5b\"").unwrap();
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Requested {
            id,
            session: SessionId::new(),
            name: "other".into(),
            note: String::new(),
            current: None,
            title: None,
            fields: vec![],
        }));
        assert!(m.secret_request().is_none());
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Requested {
            id,
            session,
            name: "github".into(),
            note: "PAT".into(),
            current: Some(2),
            title: None,
            fields: vec![],
        }));
        assert!(m.secret_request().is_some() && m.input_focused());
        m.answer_secret(Some("ghp_requested".into()), &tx);
        assert!(m.secret_request().is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::AnswerSecret { id: i, value: Some(v) }) if i == id && v == "ghp_requested"
        ));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "github: stored"));
        // Resolved elsewhere (or timed out): the box goes away.
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Requested {
            id,
            session,
            name: "github".into(),
            note: "PAT".into(),
            current: None,
            title: None,
            fields: vec![],
        }));
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Resolved {
            id,
            stored: None,
            timed_out: true,
        }));
        assert!(m.secret_request().is_none());
        for v in ["ghp_topsecret", "ghp_second", "ghp_requested"] {
            assert!(!leaked(&m, v), "{v} must not land in the transcript");
        }
    }

    #[test]
    fn secrets_menu_history_restore_delete() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/secrets", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListSecrets)));
        m.apply(UiEvent::Secrets(secrets_list()));
        assert!(matches!(m.menu, Some(Menu::Secrets { .. })) && !m.input_focused());
        assert!(m.secrets_title().starts_with("secrets · key: keychain"));
        assert!(
            m.secrets_rows()[0].starts_with("github")
                && m.secrets_rows()[0].contains("v2 (2 kept)")
        );
        // `h` opens the history; ⏎ on the current row is a no-op, on v1 restores it.
        m.menu_key(KeyCode::Char('h'), "", &tx);
        let rows = m.secrets_rows();
        assert!(
            rows[0].starts_with("v2") && rows[0].ends_with("current") && rows[1].starts_with("v1")
        );
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(rx.try_recv().is_err());
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::RestoreSecret { name, back: 1 }) if name == "github"
        ));
        // A change re-lists while the menu is open; Esc leaves the history first.
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Changed));
        assert!(std::mem::take(&mut m.refresh_secrets));
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(matches!(m.menu, Some(Menu::Secrets { history: None, .. })));
        // `r` replaces the highlighted value, knowing its version.
        m.menu_key(KeyCode::Char('r'), "", &tx);
        assert!(
            matches!(
                m.prompt,
                Some(Prompt::SecretValue {
                    current: Some(2),
                    ..
                })
            ) && m.menu.is_none()
        );
        m.cancel_prompt();
        // `d` deletes only when the name is typed back.
        m.menu = Some(Menu::Secrets {
            cursor: 0,
            history: None,
        });
        m.menu_key(KeyCode::Char('d'), "", &tx);
        assert!(matches!(m.prompt, Some(Prompt::SecretDelete { .. })));
        m.submit("nope", &tx);
        assert!(rx.try_recv().is_err() && m.prompt.is_none());
        m.menu = Some(Menu::Secrets {
            cursor: 0,
            history: None,
        });
        m.menu_key(KeyCode::Char('d'), "", &tx);
        m.submit("github", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::DeleteSecret(n)) if n == "github"));
        // One-shot forms.
        m.submit("/secrets rm github", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::DeleteSecret(n)) if n == "github"));
        m.submit("/secrets restore github", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::RestoreSecret { back: 1, .. })
        ));
        m.submit("/secrets restore github 3", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::RestoreSecret { back: 3, .. })
        ));
        m.submit("/secrets restore github 9", &tx);
        assert!(rx.try_recv().is_err() && matches!(m.entries.last(), Some(Entry::Error(_))));
    }

    #[test]
    fn mcp_enter_switches_space_cycles_risk_only() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let sid = SessionId::new();
        m.session = Some(sid);
        let fresh = |gate: Option<Gate>, tool_gate: Option<Gate>, hidden: bool| McpServer {
            name: "fs".into(),
            connected: gate != Some(Gate::Off),
            repo: None,
            auth: McpAuth::None,
            gate,
            tools: vec![McpToolInfo {
                name: "read".into(),
                description: String::new(),
                gate: tool_gate,
                risk: tools::Risk::Mutating,
                action: PolicyAction::Ask,
                registered: Some("read".into()),
                hidden,
            }],
        };
        // A refresh with no menu open and no `/mcp` typed must not open one.
        m.apply(UiEvent::Mcp(vec![fresh(None, None, false)]));
        assert!(m.menu.is_none());
        m.submit("/mcp", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListMcp(Some(s))) if s == sid));
        m.apply(UiEvent::Mcp(vec![fresh(
            Some(Gate::Destructive),
            None,
            false,
        )]));

        // Space never reaches `off`: destructive wraps to annotations.
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { server, tool: None, gate: None, .. }) if server == "fs"
        ));

        // ⏎ on an `on` server asks for the scope; `s` hides its tools for the session.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(&m.menu, Some(Menu::Mcp { pending: Some(t), .. }) if t.server == "fs"));
        assert!(
            rx.try_recv().is_err(),
            "nothing sent until the scope is chosen"
        );
        m.menu_key(KeyCode::Char('s'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetHidden { session, tools, hidden: true }) if session == Some(sid) && tools == ["read"]
        ));
        assert!(matches!(&m.menu, Some(Menu::Mcp { pending: None, .. })));

        // Every tool hidden = server `off · this session`; ⏎ turns it back on.
        m.apply(UiEvent::Mcp(vec![fresh(None, None, true)]));
        assert_eq!(m.mcp_rows()[0].switch(), Switch::OffSession);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetHidden { hidden: false, .. })
        ));

        // `a` writes `off` to config.yaml; ⏎ on it clears the line; space is inert.
        m.apply(UiEvent::Mcp(vec![fresh(None, None, false)]));
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Char('a'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate {
                tool: None,
                gate: Some(Gate::Off),
                ..
            })
        ));
        m.apply(UiEvent::Mcp(vec![fresh(Some(Gate::Off), None, false)]));
        assert_eq!(m.mcp_rows()[0].switch(), Switch::OffAlways);
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(
            rx.try_recv().is_err(),
            "risk cycling on an off server does nothing"
        );
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate {
                tool: None,
                gate: None,
                ..
            })
        ));

        // Esc in the scope prompt cancels the prompt, not the menu.
        m.apply(UiEvent::Mcp(vec![fresh(None, None, false)]));
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(matches!(&m.menu, Some(Menu::Mcp { pending: None, .. })));
        assert!(rx.try_recv().is_err());

        // Tool rows: space cycles the tool line, ⌫ clears it, ⏎ + a turns it off.
        m.menu_key(KeyCode::Right, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        assert_eq!(m.mcp_rows().len(), 2);
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { tool: Some(t), gate: Some(Gate::ReadOnly), .. }) if t == "read"
        ));
        m.menu_key(KeyCode::Backspace, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate {
                tool: Some(_),
                gate: None,
                ..
            })
        ));
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Char('a'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { tool: Some(t), gate: Some(Gate::Off), .. }) if t == "read"
        ));
        m.menu_key(KeyCode::Left, "", &tx);
        assert!(matches!(m.menu, Some(Menu::Mcp { cursor: 0, .. })));
        assert_eq!(m.mcp_rows().len(), 1);

        // Without a session the prompt still shows; `s` asks for one to be created.
        m.session = None;
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            &m.menu,
            Some(Menu::Mcp {
                pending: Some(_),
                ..
            })
        ));
        m.menu_key(KeyCode::Char('s'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetHidden {
                session: None,
                hidden: true,
                ..
            })
        ));
    }

    #[test]
    fn mcp_add_routes_urls_and_json_shapes() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/mcp add https://github.com/o/r", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::AddMcpRepo(u)) if u.ends_with("/o/r")));
        for json in [
            r#"{"name":"fs","command":"npx","args":["-y","x"]}"#,
            r#"{"fs":{"command":"npx","args":["-y","x"]}}"#,
            r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","x"]}}}"#,
        ] {
            m.submit(&format!("/mcp add {json}"), &tx);
            let Ok(Command::AddMcp { name, entry }) = rx.try_recv() else {
                panic!("{json}");
            };
            assert_eq!(name, "fs");
            assert_eq!(entry["command"], "npx");
            assert!(entry.get("name").is_none());
        }
        m.submit(r#"/mcp add {"command":"npx"}"#, &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(m.entries.last(), Some(Entry::Error(_))));
        m.submit("/mcp sync", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SyncMcp)));
    }

    #[test]
    fn acp_menu_lists_probes_adds_and_removes() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let entry = EntryInfo {
            name: "codex".into(),
            command: "codex-acp".into(),
            args: vec![],
            description: None,
            status: None,
        };
        let reg = RegistryAgent {
            id: "codex-acp".into(),
            name: "Codex".into(),
            version: "1.0".into(),
            description: String::new(),
            distribution: serde_json::Value::Null,
        };
        // A refresh with no menu open and no `/acp` typed must not open one.
        m.apply(UiEvent::Acp(vec![]));
        assert!(m.menu.is_none());
        m.submit("/acp", &tx);
        assert!(m.awaiting_acp);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::ListAcp { probe: false })
        ));
        assert!(matches!(rx.try_recv(), Ok(Command::ListAcpRegistry)));
        m.apply(UiEvent::Acp(vec![entry]));
        m.apply(UiEvent::AcpRegistry(vec![reg]));
        assert_eq!(m.acp_rows().len(), 3, "entry, separator, registry row");
        m.menu_key(KeyCode::Char('p'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::ListAcp { probe: true })
        ));
        assert!(matches!(
            &m.menu,
            Some(Menu::Acp { entries, .. }) if entries[0].status.as_deref() == Some("probing…")
        ));
        // Down skips the separator and lands on the registry row.
        m.menu_key(KeyCode::Down, "", &tx);
        assert!(matches!(m.menu, Some(Menu::Acp { cursor: 2, .. })));
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::AddAcp(id)) if id == "codex-acp"));
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Char('d'), "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::RemoveAcp(n)) if n == "codex"));
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(m.menu.is_none());
        m.submit("/acp add codex-acp", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::AddAcp(id)) if id == "codex-acp"));
        m.submit("/acp remove codex", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::RemoveAcp(n)) if n == "codex"));
        m.submit("/acp bogus", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(m.entries.last(), Some(Entry::Error(_))));
    }

    #[test]
    fn injected_splits_a_batched_turn_and_leaves_human_text_alone() {
        let text = "<message from=\"pinger\" id=\"m1\" reply_expected=\"false\">\nPONG\n</message>\n<subagent_result name=\"pinger\" session=\"x\" stop=\"end_turn\">\nSent.\n</subagent_result>";
        let peer = |from: &str, body: &str| (CardKind::Peer, from.to_owned(), body.to_owned());
        assert_eq!(
            injected(text),
            Some(vec![peer("pinger", "PONG"), peer("pinger", "Sent.")])
        );
        // Harness notes are hidden without producing a card; a peer message
        // batched with one still gets its card.
        assert_eq!(injected("<todo>- [ ] x\n- [x] y</todo>"), Some(Vec::new()));
        assert_eq!(
            injected("<knowledge>\nConcepts that may be relevant:\n- `facts/x`\n</knowledge>"),
            Some(Vec::new())
        );
        assert_eq!(
            injected(
                "<context_pressure>80% full</context_pressure>\n<message from=\"p\">\nhi\n</message>"
            ),
            Some(vec![peer("p", "hi")])
        );
        assert_eq!(injected("<b>\nnot a wrapper\n</b>"), None);
        assert_eq!(injected("hello"), None);
        assert_eq!(injected("<todo>unterminated"), None);
    }

    /// 61 §2.3: a quiet PTY job cards as `j4 idle 63s · name`; body is the screen.
    #[test]
    fn injected_parses_job_idle() {
        assert_eq!(
            injected(
                "<job_idle id=\"j4\" command=\"omp\" name=\"omp&lt;1&gt;\" quiet=\"63\" secs=\"912\" nudge=\"1\" user_typed=\"12\">\n>>> \ncursor: row 1 col 5\n</job_idle>"
            ),
            Some(vec![(
                CardKind::Job,
                "j4 idle 63s · omp<1>".to_owned(),
                ">>> \ncursor: row 1 col 5".to_owned(),
            )])
        );
        assert_eq!(
            injected(
                "<job_idle id=\"j1\" command=\"python3 -q\" quiet=\"120\" secs=\"200\" nudge=\"2\">\n</job_idle>"
            ),
            Some(vec![(
                CardKind::Job,
                "j1 idle 120s".to_owned(),
                String::new()
            )])
        );
    }

    /// 14 as-built: a fired reminder cards as `r3 reminder · after 10m` (` · late` when re-armed past due).
    #[test]
    fn injected_parses_reminder() {
        assert_eq!(
            injected(
                "<reminder id=\"r3\" after=\"600\" set_at=\"2026-09-23T10:00:00+02:00\" late=\"false\">\ncheck the deploy\n</reminder>"
            ),
            Some(vec![(
                CardKind::Job,
                "r3 reminder · after 10m".to_owned(),
                "check the deploy".to_owned()
            )])
        );
        assert_eq!(
            injected(
                "<reminder id=\"r1\" after=\"5400\" set_at=\"x\" late=\"true\">\n\n</reminder>"
            ),
            Some(vec![(
                CardKind::Job,
                "r1 reminder · after 1h30m · late".to_owned(),
                String::new()
            )])
        );
    }

    #[test]
    fn injected_parses_job_result() {
        let text = "<job_result id=\"j3\" command=\"echo &quot;a&quot; &amp;&amp; ls &gt; x\" exit=\"0\" secs=\"47\">\nok\n</job_result>";
        assert_eq!(
            injected(text),
            Some(vec![(
                CardKind::Job,
                "j3 · echo \"a\" && ls > x · exit 0 · 47s".to_owned(),
                "ok".to_owned(),
            )])
        );
        assert_eq!(
            injected(
                "<job_result id=\"j1\" command=\"sleep 9\" exit=\"killed\" secs=\"2\">\n</job_result>"
            ),
            Some(vec![(
                CardKind::Job,
                "j1 · sleep 9 · exit killed · 2s".to_owned(),
                String::new()
            )])
        );
        // 22 §5: a PTY job's card says so.
        assert_eq!(
            injected(
                "<job_result id=\"j4\" command=\"python3 -q\" exit=\"0\" secs=\"41\" pty=\"true\">\n>>> \n</job_result>"
            ),
            Some(vec![(
                CardKind::Job,
                "j4 · python3 -q · exit 0 · 41s · pty".to_owned(),
                ">>>".to_owned()
            )])
        );
        // 34 §1.5: a schedule fire cards as `name · HH:MM` (local); a bad `at` shows raw.
        let at = "2026-09-16T09:00:00-07:00";
        let hhmm = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();
        assert_eq!(
            injected(&format!(
                "<schedule name=\"morning-brief\" at=\"{at}\" quiet=\"false\">\nBrief me.\n</schedule>"
            )),
            Some(vec![(
                CardKind::Schedule,
                format!("morning-brief · {hhmm}"),
                "Brief me.".to_owned()
            )])
        );
        assert_eq!(
            injected("<schedule name=\"x\" at=\"soon\" quiet=\"true\">\nhi\n</schedule>"),
            Some(vec![(
                CardKind::Schedule,
                "x · soon".to_owned(),
                "hi".to_owned()
            )])
        );
    }

    #[test]
    fn screen_event_replaces_tail() {
        let mut m = Model::default();
        let s = SessionId::new();
        m.session = Some(s);
        m.apply_agent(AgentEvent::ToolCallStarted {
            session: s,
            turn: TurnId::new(),
            call_id: "c1".into(),
            name: "start".into(),
            args: serde_json::json!({ "command": "python3 -q" }),
        });
        m.apply(UiEvent::Job(JobEvent::Started {
            session: s,
            id: tools::JobId("j1".into()),
            command: "python3 -q".into(),
            call_id: "c1".into(),
            log: "/tmp/j.log".into(),
            name: None,
            pty: false,
        }));
        let screen = |rows: &[&str]| {
            UiEvent::Job(JobEvent::Screen {
                session: s,
                id: tools::JobId("j1".into()),
                rows: rows.iter().map(|r| (*r).to_owned()).collect(),
                cursor: (0, 0),
            })
        };
        let rows = |m: &Model| -> Vec<String> {
            m.entries
                .iter()
                .find_map(|e| match e {
                    Entry::Tool { job: Some(j), .. } => Some(j.rows().map(str::to_owned).collect()),
                    _ => None,
                })
                .unwrap()
        };
        m.apply(screen(&["Python 3.12", ">>> ", "", "   "]));
        assert_eq!(rows(&m), ["Python 3.12", ">>> "], "blank rows dropped");
        // The next screen replaces, never appends; capped at JOB_LINES from the end.
        let many: Vec<String> = (0..24).map(|i| format!("row {i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        m.apply(screen(&many));
        let got = rows(&m);
        assert_eq!(got.len(), JOB_LINES);
        assert_eq!(got[0], "row 14");
        assert_eq!(got[JOB_LINES - 1], "row 23");
        // Another session's screen is not ours.
        m.apply(UiEvent::Job(JobEvent::Screen {
            session: SessionId::new(),
            id: tools::JobId("j1".into()),
            rows: vec!["nope".into()],
            cursor: (0, 0),
        }));
        assert_eq!(rows(&m)[0], "row 14");
    }

    #[test]
    fn computer_slash_submits_status_and_renders() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/computer", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ComputerStatus)));

        let status = computer::PermissionStatus {
            accessibility: true,
            screen_recording: false,
            displays: vec![
                computer::DisplayInfo {
                    index: 1,
                    name: "Built-in".into(),
                    main: true,
                    width: 1512,
                    height: 982,
                    scale: 2.0,
                },
                computer::DisplayInfo {
                    index: 2,
                    name: "Ext".into(),
                    main: false,
                    width: 2560,
                    height: 1440,
                    scale: 1.0,
                },
            ],
            note: "grant screen recording".into(),
        };
        assert_eq!(
            computer_line(&status),
            "accessibility ✓ · screen recording ✗ · 2 displays: 1 main 1512×982 @2×, 2 2560×1440 @1×\ngrant screen recording"
        );
        m.apply(UiEvent::Computer(status));
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.starts_with("accessibility ✓"))
        );
    }

    #[test]
    fn tasks_and_agents_slash_use_the_session_project_and_render_tables() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/tasks", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListTasks(None))));
        m.project = Some("sre".into());
        m.submit("/tasks", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListTasks(Some(p))) if p == "sre"));
        m.submit("/roster", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListRoster)));

        assert_eq!(tasks_table(None, &[]), "no tasks (all projects)");
        let task = tasks::Task {
            id: tasks::TaskId("0192a000-0000-7000-8000-00000000abcdef".into()),
            title: "Fix the deploy".into(),
            brief: String::new(),
            owner: tasks::Owner::Agent("ops".into()),
            state: tasks::TaskState::Doing,
            reviewer: None,
            blockers: vec![],
            blocks: vec![],
            parent: None,
            project: Some("sre".into()),
            where_: None,
            harness: None,
            external: vec![],
            thread: None,
            checkout: None,
            schedule: None,
            created_by: "user".into(),
            created_ms: 0,
            updated_ms: 0,
            no_progress_count: 0,
            next_wake_after_ms: None,
            done_when: None,
            attachments: vec![],
            priority: tasks::Priority::Medium,
        };
        assert_eq!(
            tasks_table(Some("sre"), &[task]),
            "tasks · sre\nid · state · owner · title\nabcdef · doing · agent:ops · Fix the deploy"
        );

        assert_eq!(agents_table(&[]), "no agents");
        let file = roster::file::parse("ops", "---\ntitle: Ops\n---\nYou are Ops.").unwrap();
        let agent = roster::AgentSummary {
            file,
            status: roster::AgentStatus::Working,
            desk: None,
            open_tasks: 2,
            spent_month: 0.0,
            error: None,
        };
        assert_eq!(
            agents_table(&[agent]),
            "name · status · open\nops · working · 2"
        );
        m.apply(UiEvent::Roster(vec![]));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "no agents"));
    }

    /// 68 §10: `/take` needs an open session; the picker row carries the
    /// owner chip only when the session lives elsewhere and follows
    /// `SessionOwnerChanged`; the owner-offline refusal is one sentence
    /// ending in `/take`.
    #[test]
    fn take_slash_owner_chip_and_offline_sentence() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/take", &tx);
        assert!(matches!(m.entries.last(), Some(Entry::Error(t)) if t == "no session open"));
        let id = SessionId::new();
        m.session = Some(id);
        m.submit("/take", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::TakeSession(s)) if s == id));

        let mut s = summary(id, None, None);
        s.title = "Fix the deploy".into();
        assert!(
            session_row(&s, 0, 120, true)
                .ends_with(&format!("{:<20} {:<8}  Fix the deploy", "m", ""))
        );
        s.owner = "abcdef0123456789".repeat(4);
        s.here = false;
        let row = session_row(&s, 0, 120, false);
        assert!(row.contains(" abcdef01  Fix the deploy"), "{row}");
        assert_eq!(engine_chip(""), "?");

        m.apply(UiEvent::Sessions(vec![s.clone()]));
        m.apply(UiEvent::Agent(AgentEvent::SessionOwnerChanged {
            session: id,
            owner: "me".into(),
            epoch: 2,
            here: true,
        }));
        match &m.menu {
            Some(Menu::Sessions { items, .. }) => {
                let Grouped::Item((row, _)) = &items[1] else {
                    panic!("{items:?}")
                };
                assert!(row.here && row.epoch == 2 && row.owner == "me");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t == "session continues here (epoch 2)")
        );

        assert_eq!(
            owner_offline_line(&s.owner, "3m ago"),
            "owned by abcdef01, last seen 3m ago. continue here now? (turns not yet shipped from abcdef01 stay there) — /take"
        );
        let (owner, seen) = owner_unreachable(Some(
            r#"{"OwnerUnreachable":{"owner":"abcdef0123456789","last_seen":null}}"#,
        ))
        .unwrap();
        assert_eq!(
            (owner.as_str(), seen.as_str()),
            ("abcdef0123456789", "never")
        );
        let (_, seen) = owner_unreachable(Some(
            r#"{"kind":"OwnerUnreachable","owner":"x","last_seen":"2020-01-01T00:00:00Z"}"#,
        ))
        .unwrap();
        assert!(seen.ends_with("d ago"), "{seen}");
        assert!(owner_unreachable(Some(r#"{"other":1}"#)).is_none());
        assert!(owner_unreachable(None).is_none());
    }

    #[test]
    fn schedules_slash_args_and_loop_columns() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/schedules pause port", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::EnableSchedule(name, false)) if name == "port"
        ));
        m.submit("/schedules resume port", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::EnableSchedule(name, true)) if name == "port"
        ));
        m.submit("/schedules test port", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::TestSchedule(name)) if name == "port"
        ));
        for bad in ["/schedules pause", "/schedules stop x"] {
            m.submit(bad, &tx);
            assert!(rx.try_recv().is_err(), "{bad}");
            assert!(
                matches!(m.entries.last(), Some(Entry::Info(t)) if t == SCHEDULES_USAGE),
                "{bad}"
            );
        }

        let info =
            |name: &str, until: Option<&str>, max_runs, runs, approvals| schedule::ScheduleInfo {
                name: name.into(),
                when: "every 0s".into(),
                describe: "every 0s".into(),
                enabled: true,
                quiet: false,
                next: None,
                last: None,
                last_result: None,
                session: None,
                running: false,
                error: None,
                until: until.map(Into::into),
                max_runs,
                runs,
                approvals,
                cwd: None,
                agent: None,
                project: None,
                when_sensor: None,
                sensor: None,
                fire: None,
                for_: None,
                cooldown: None,
                mode: None,
                memory: None,
                expires: None,
                timeout: None,
                model: None,
                run_timeout: None,
                created: None,
                last_check: None,
            };
        let rows = [
            info("plain", None, None, 0, true),
            info("capped", None, Some(20), 3, true),
            info("checked", Some("cargo test"), None, 7, false),
        ];
        let table = schedules_table(&rows);
        let mut lines = table.lines();
        assert_eq!(
            lines.next(),
            Some("name · when · next · runs · last · result · state")
        );
        assert!(lines.next().unwrap().contains(" · — · — · — · — · "));
        assert!(lines.next().unwrap().contains(" · 3/20 · "));
        assert!(lines.next().unwrap().contains(" · 7 · "));

        // 40 §6: a checked watcher adds the `last check` column; other rows get `—`.
        let mut watcher = info("watch", None, Some(3), 0, true);
        watcher.when = "every 60s when fs_exists.py".into();
        watcher.last_check = Some(schedule::LastCheck {
            at: chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 9, 28, 10, 15, 3).unwrap(),
            outcome: "quiet".into(),
            elapsed_ms: 1200,
            hash: None,
        });
        let table = schedules_table(&[rows[0].clone(), watcher]);
        let mut lines = table.lines();
        assert!(lines.next().unwrap().ends_with(" · state · last check"));
        assert!(lines.next().unwrap().ends_with(" · —"));
        let w = lines.next().unwrap();
        assert!(w.contains(" · every 60s when fs_exists.py · "), "{w}");
        assert!(w.ends_with(" · 10:15:03 quiet 1200 ms"), "{w}");

        let fired = chrono::Local::now();
        let run = schedule::RunInfo {
            session: None,
            fired_at: fired,
            finished_at: Some(fired),
            result: Some("end_turn".into()),
            report: Some(format!("\n  {}\nsecond line", "x".repeat(70))),
            envelope: None,
            kind: schedule::RunKind::Run,
        };
        let table = schedule_history_table("checked", Some(&rows[2]), &[run]);
        let mut lines = table.lines();
        assert_eq!(
            lines.next(),
            Some("checked: until: cargo test · approvals off")
        );
        assert_eq!(
            lines.next(),
            Some("checked: fired · took · result · session · report")
        );
        let row = lines.next().unwrap();
        assert!(row.ends_with(&format!(" · {}…", "x".repeat(59))), "{row}");
    }

    /// 133 §5: Esc cancels the root; the third Esc within a second cancels the
    /// tree and says how many subagents went with it. Spaced taps never do.
    #[test]
    fn triple_esc_cancels_the_tree() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let root = SessionId::new();
        m.session = Some(root);
        m.streaming = true;
        let info = |state: AgentState| AgentInfo {
            id: SessionId::new(),
            name: "w".into(),
            parent: root,
            depth: 1,
            agent: None,
            model: "m".into(),
            state,
            started_at: None,
            finished_at: None,
            last: String::new(),
            project: None,
            retention: agent::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
        };
        for state in [AgentState::Running, AgentState::Running, AgentState::Idle] {
            let i = info(state);
            m.agents.insert(i.id, AgentRow::new(i));
        }
        let t0 = std::time::Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        m.cancel_at(ms(0), &tx);
        m.cancel_at(ms(300), &tx);
        m.cancel_at(ms(600), &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Cancel(s)) if s == root));
        assert!(matches!(rx.try_recv(), Ok(Command::Cancel(s)) if s == root));
        assert!(matches!(rx.try_recv(), Ok(Command::CancelTree(s)) if s == root));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "stopped 2 agents"));
        // The taps were spent: three more are needed, and spaced ones never add up.
        m.cancel_at(ms(700), &tx);
        m.cancel_at(ms(1800), &tx);
        m.cancel_at(ms(2900), &tx);
        for _ in 0..3 {
            assert!(matches!(rx.try_recv(), Ok(Command::Cancel(_))));
        }
        assert!(rx.try_recv().is_err());
        // Nothing in flight: Esc is not a cancel at all.
        m.streaming = false;
        m.cancel_at(ms(3000), &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn jobs_slash_args_and_status_count() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/jobs", &tx);
        assert!(rx.try_recv().is_err(), "no session");
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "no session"));

        let s = SessionId::new();
        m.session = Some(s);
        m.submit("/jobs", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListJobs(session)) if session == s));
        m.submit("/jobs kill j3", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::CancelJob { session, id }) if session == s && id == "j3"
        ));
        for bad in ["/jobs foo", "/jobs kill"] {
            m.submit(bad, &tx);
            assert!(rx.try_recv().is_err(), "{bad}");
            assert!(
                matches!(m.entries.last(), Some(Entry::Error(t)) if t == "usage: /jobs [kill <id>]"),
                "{bad}"
            );
        }

        let started = |session, id: &str| JobEvent::Started {
            session,
            id: tools::JobId(id.into()),
            command: "sleep 9".into(),
            call_id: String::new(),
            log: "/tmp/j.log".into(),
            name: None,
            pty: false,
        };
        m.apply(UiEvent::Job(started(s, "j1")));
        m.apply(UiEvent::Job(started(s, "j2")));
        // Another session's job does not count.
        m.apply(UiEvent::Job(started(SessionId::new(), "j1")));
        assert!(m.status().ends_with("2 job(s)"), "{}", m.status());
        m.apply(UiEvent::Job(JobEvent::Finished {
            session: s,
            id: tools::JobId("j1".into()),
            command: "sleep 9".into(),
            exit: tools::JobExit::Killed,
            secs: 2,
            output: String::new(),
            inline: false,
            pty: false,
        }));
        assert!(m.status().ends_with("1 job(s)"), "{}", m.status());
        m.apply_agent(AgentEvent::MessageDelivered {
            session: s,
            from: "job j1".into(),
            text: "done".into(),
        });
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Card { kind: CardKind::Job, title, .. }) if title == "job j1"
        ));
    }

    #[test]
    fn shell_card_shows_the_job_tail_until_its_result_lands() {
        let mut m = Model::default();
        let s = SessionId::new();
        m.session = Some(s);
        let turn = TurnId::new();
        for c in ["c1", "c2"] {
            m.apply_agent(AgentEvent::ToolCallStarted {
                session: s,
                turn,
                call_id: c.into(),
                name: "shell".into(),
                args: serde_json::json!({ "command": "make" }),
            });
        }
        let job = |m: &Model, c: &str| {
            m.entries
                .iter()
                .find_map(|e| match e {
                    Entry::Tool { call_id, job, .. } if call_id == c => Some(job.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let started = |id: &str, call: &str| {
            UiEvent::Job(JobEvent::Started {
                session: s,
                id: tools::JobId(id.into()),
                command: "make".into(),
                call_id: call.into(),
                log: "/tmp/j.log".into(),
                name: None,
                pty: false,
            })
        };
        m.apply(started("j1", "c1"));
        m.apply(started("j2", "c2"));
        // Identical commands: the call id, not the command, picks the card.
        assert_eq!(job(&m, "c1").unwrap().id, "j1");
        assert_eq!(job(&m, "c2").unwrap().id, "j2");

        // Output: colours dropped, `\r` rewinds, only the last 10 lines kept.
        let chunk = (0..12).fold(String::new(), |mut acc, i| {
            acc.push_str(&format!("\x1b[32mline {i}\x1b[0m\n"));
            acc
        });
        m.apply(UiEvent::Job(JobEvent::Output {
            session: s,
            id: tools::JobId("j2".into()),
            chunk: format!("{chunk}10%\r55%"),
        }));
        let j2 = job(&m, "c2").unwrap();
        let rows: Vec<&str> = j2.rows().collect();
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], "line 3");
        assert_eq!(
            rows[9], "55%",
            "partial line shows, oldest complete line goes"
        );
        assert_eq!(j2.header(), "j2 · running");
        assert!(job(&m, "c1").unwrap().rows().next().is_none());
        // The card grows by the header and the rows, never past 11.
        let base = Entry::Tool {
            call_id: String::new(),
            name: "shell".into(),
            args: vec!["make".into()],
            result: None,
            expanded: None,
            job: None,
            artifact: None,
            scrape: None,
        }
        .height(80);
        let tool = m
            .entries
            .iter()
            .find(|e| matches!(e, Entry::Tool { call_id, .. } if call_id == "c2"))
            .unwrap();
        assert_eq!(tool.height(80), base + 1 + 10);

        // Finished within grace: the result carries the output, the block goes.
        let finished = |id: &str, inline: bool| {
            UiEvent::Job(JobEvent::Finished {
                session: s,
                id: tools::JobId(id.into()),
                command: "make".into(),
                exit: tools::JobExit::Exited(0),
                secs: 47,
                output: String::new(),
                inline,
                pty: false,
            })
        };
        m.apply(finished("j1", true));
        assert_eq!(job(&m, "c1"), None);
        // Backgrounded: the block freezes with how it ended.
        m.apply(finished("j2", false));
        assert_eq!(job(&m, "c2").unwrap().header(), "j2 · exited 0 · 47s");
        assert_eq!(job(&m, "c2").unwrap().rows().count(), 10);
    }

    #[test]
    fn knowledge_command_grammar() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/knowledge", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::KnowledgeList)));
        m.submit("/knowledge history facts/x", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeHistory { id: Some(id), limit: HISTORY_PAGE }) if id == "facts/x"
        ));
        m.submit("/knowledge history", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeHistory { id: None, .. })
        ));
        m.submit("/knowledge show a1b2c3d facts/x", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeRevision { commit, id: Some(id), card: true }) if commit == "a1b2c3d" && id == "facts/x"
        ));
        m.submit("/knowledge restore facts/x a1b2c3d", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeRestore { id, commit: Some(c) }) if id == "facts/x" && c == "a1b2c3d"
        ));
        m.submit("/knowledge restore", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(m.entries.last(), Some(Entry::Error(_))));
        m.submit("/knowledge harvest", &tx);
        assert!(rx.try_recv().is_err(), "no session shown");
        m.submit("/knowledge rust toolchain", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::KnowledgeSearch(q)) if q == "rust toolchain"));
    }

    /// `/knowledge` → `h` opens the history menu loading; only the matching
    /// reply fills it; the `older` row pages by 20 and lands on the first
    /// new commit; ⏎ fetches one commit's diff, a reply for another commit
    /// or after esc is dropped, `←→` scroll columns, esc steps back then
    /// closes.
    #[test]
    fn knowledge_history_menu_pages_and_shows_one_commit() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.apply(UiEvent::Knowledge(vec![Meta {
            id: "facts/x".into(),
            type_: "fact".into(),
            title: "x".into(),
            description: String::new(),
            tags: Vec::new(),
            scope: None,
            source: None,
            timestamp: None,
            owner: "user".into(),
            visibility: Default::default(),
            ..Default::default()
        }]));
        assert!(matches!(m.menu, Some(Menu::Knowledge { .. })) && !m.input_focused());
        m.menu_key(KeyCode::Char('h'), "", &tx);
        let Ok(Command::KnowledgeHistory { id, limit }) = rx.try_recv() else {
            panic!("history asked");
        };
        assert_eq!((id.as_deref(), limit), (Some("facts/x"), HISTORY_PAGE));
        assert!(matches!(
            m.menu,
            Some(Menu::KnowledgeHistory { loading: true, .. })
        ));
        assert_eq!(m.knowledge_history_menu().1, ["loading…"]);
        let change = |c: &str| Change {
            commit: c.into(),
            time: "2026-09-18 10:00".into(),
            message: format!("edit {c}"),
        };
        let history = |n: usize| History {
            changes: (0..n).map(|i| change(&format!("c{i:06}"))).collect(),
            diff: None,
        };
        // Another scope's reply is dropped; the matching one fills the menu.
        m.apply(UiEvent::KnowledgeHistory {
            id: None,
            limit,
            history: history(3),
        });
        assert!(matches!(
            m.menu,
            Some(Menu::KnowledgeHistory { loading: true, .. })
        ));
        // A full page plus one: the `older` row shows and asks for 20 more.
        m.apply(UiEvent::KnowledgeHistory {
            id: id.clone(),
            limit,
            history: history(HISTORY_PAGE + 1),
        });
        assert!(matches!(
            m.menu,
            Some(Menu::KnowledgeHistory {
                more: true,
                loading: false,
                ..
            })
        ));
        let (_, rows, _) = m.knowledge_history_menu();
        assert_eq!(rows.len(), HISTORY_PAGE + 1);
        assert!(rows[HISTORY_PAGE].starts_with("… older"));
        for _ in 0..HISTORY_PAGE + 5 {
            m.menu_key(KeyCode::Down, "", &tx);
        }
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeHistory { limit, .. }) if limit == 2 * HISTORY_PAGE
        ));
        // The old page stays up while the next loads; a stale first-page reply is dropped.
        assert_eq!(m.knowledge_history_menu().1.len(), HISTORY_PAGE + 1);
        m.apply(UiEvent::KnowledgeHistory {
            id: id.clone(),
            limit: HISTORY_PAGE,
            history: history(2),
        });
        m.apply(UiEvent::KnowledgeHistory {
            id: id.clone(),
            limit: 2 * HISTORY_PAGE,
            history: history(30),
        });
        let (_, rows, cursor) = m.knowledge_history_menu();
        assert_eq!(
            (rows.len(), cursor),
            (30, HISTORY_PAGE),
            "no older row; highlight on the first new commit"
        );
        // ⏎ on a commit fetches its revision in the menu's scope.
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeRevision { commit, id: Some(i), card: false }) if commit == "c000019" && i == "facts/x"
        ));
        assert!(m.knowledge_history_menu().0.contains("loading c000019"));
        let revision = |c: &str| Revision {
            change: change(c),
            parent: None,
            files: vec![knowledge::ChangedFile {
                id: "facts/x".into(),
                status: ChangeStatus::Deleted,
                diff: "-a\n-bcdefghijklmnop".into(),
            }],
        };
        // A reply for a commit no longer awaited is dropped.
        m.apply(UiEvent::KnowledgeRevision {
            id: id.clone(),
            revision: revision("c000003"),
            card: false,
        });
        assert!(matches!(
            m.menu,
            Some(Menu::KnowledgeHistory {
                revision: None,
                pending: Some(_),
                ..
            })
        ));
        // A failed fetch shows in the menu, not the transcript; `r` re-asks.
        m.apply(UiEvent::Error("boom".into()));
        assert!(m.entries.is_empty());
        assert!(m.knowledge_history_menu().0.contains("error: boom"));
        m.menu_key(KeyCode::Char('r'), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeHistory { limit, .. }) if limit == 2 * HISTORY_PAGE
        ));
        m.apply(UiEvent::KnowledgeHistory {
            id: id.clone(),
            limit: 2 * HISTORY_PAGE,
            history: history(30),
        });
        assert!(!m.knowledge_history_menu().0.contains("error"));
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::KnowledgeRevision { .. })
        ));
        m.apply(UiEvent::KnowledgeRevision {
            id: id.clone(),
            revision: revision("c000019"),
            card: false,
        });
        let (title, rows, scroll) = m.knowledge_history_menu();
        assert!(title.contains("parent none (root)"), "{title}");
        assert_eq!(rows, ["- facts/x  (deleted)", "-a", "-bcdefghijklmnop"]);
        assert_eq!(scroll, 0);
        // ↓ scrolls rows and clamps; → drops leading chars, ← restores them.
        for _ in 0..3 {
            m.menu_key(KeyCode::Down, "", &tx);
        }
        assert_eq!(m.knowledge_history_menu().2, 2);
        m.menu_key(KeyCode::Right, "", &tx);
        assert_eq!(m.knowledge_history_menu().1[2], "ijklmnop");
        m.menu_key(KeyCode::Left, "", &tx);
        assert_eq!(m.knowledge_history_menu().1[1], "-a");
        // esc goes back to the list, then closes; a reply after that is dropped.
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(matches!(
            m.menu,
            Some(Menu::KnowledgeHistory {
                revision: None,
                cursor: 19,
                ..
            })
        ));
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(m.menu.is_none());
        m.apply(UiEvent::KnowledgeRevision {
            id: id.clone(),
            revision: revision("c000019"),
            card: false,
        });
        assert!(m.menu.is_none() && m.entries.is_empty());
        // `/knowledge show` is the one path to a transcript card.
        m.apply(UiEvent::KnowledgeRevision {
            id: None,
            revision: revision("c000019"),
            card: true,
        });
        assert!(matches!(m.entries.last(), Some(Entry::Card { .. })));
    }

    #[test]
    fn compact_slash_args() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/compact", &tx);
        assert!(rx.try_recv().is_err(), "no session");
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "no session"));

        let s = SessionId::new();
        m.session = Some(s);
        m.submit("/compact cut", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Compact {
                session,
                style: Some(CompactStyle::Cut)
            }) if session == s
        ));
        m.submit("/compact foo", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Error(t)) if t == "usage: /compact [summary|cut|snap|style …]"
        ));
        m.submit("/compact style", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::GetCompactStyle)));
        m.submit("/compact style cut", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetCompactStyle(CompactStyle::Cut))
        ));
    }

    #[test]
    fn peer_message_lands_above_streaming_text() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        let turn = runtime::TurnId::new();
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn,
            text: "| a | b |\n|---|---|\n".into(),
        });
        m.apply_agent(AgentEvent::MessageDelivered {
            session: s,
            from: "pinger".into(),
            text: "PONG".into(),
        });
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn,
            text: "| 1 | 2 |\n".into(),
        });
        assert!(matches!(m.entries[0], Entry::Card { .. }));
        assert_eq!(m.entries.len(), 2);
        let Entry::Assistant(t) = &m.entries[1] else {
            panic!("text not kept as one entry");
        };
        assert_eq!(t.raw(), "| a | b |\n|---|---|\n| 1 | 2 |\n");
    }

    /// Rendered lines of the last (assistant) entry once finished and revealed.
    fn assistant_lines(m: &mut Model) -> Vec<String> {
        let Some(Entry::Assistant(t)) = m.entries.last_mut() else {
            panic!("assistant entry");
        };
        t.finish();
        t.reveal_all();
        t.view()
            .lines()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn assistant_json_render_fence_becomes_surface() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.width = 80;
        let spec = r#"{"root":"c","elements":{
  "c":{"type":"Card","props":{"title":"Rollout"},"children":["l"]},
  "l":{"type":"Checklist","props":{"items":[{"text":"schema","done":true},{"text":"traffic","done":false}]}}}}"#;
        let md = format!("Status:\n\n```json-render\n{spec}\n```\n\nDone.\n");
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn: TurnId::new(),
            text: md.as_str().into(),
        });
        let lines = assistant_lines(&mut m);
        assert!(
            lines.iter().any(|l| l.starts_with("╭─ Rollout")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("[x] schema")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("\"type\"")), "{lines:?}");
        assert_eq!(lines.first().map(String::as_str), Some("Status:"));
        assert_eq!(lines.last().map(String::as_str), Some("Done."));
        // Replayed history renders the same way.
        let live = lines;
        let replayed = from_history(
            HistoryEntry::Assistant {
                text: md,
                turn: None,
                context: None,
            },
            &m.assistant_options(),
        );
        m.entries = replayed;
        assert_eq!(assistant_lines(&mut m), live);
    }

    #[test]
    fn assistant_other_fences_keep_code_block() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.width = 80;
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn: TurnId::new(),
            text: "```rust\nlet x = 1;\n```\n".into(),
        });
        let lines = assistant_lines(&mut m);
        assert_eq!(lines[0], "╭ rust", "{lines:?}");
        assert!(lines.iter().any(|l| l == "let x = 1;"), "{lines:?}");
    }

    #[test]
    fn header_numbers() {
        assert_eq!(k(999), "999");
        assert_eq!(k(12_345), "12.3k");
        assert_eq!(k(200_000), "200k");
        assert_eq!(k(1_000_000), "1.0M");
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let mut m = Model::new("anthropic/claude-sonnet-4-5".into(), true);
        let limit = catalog
            .get(&m.model_id)
            .expect("bundled model")
            .1
            .limit
            .context;
        m.context = limit * 9 / 10;
        let (text, level) = m.header(&catalog);
        assert_eq!(level, Level::Danger);
        assert!(text.ends_with("(90%)"), "{text}");
        m.usage = Some(Usage {
            prompt: 1000,
            completion: 10,
            cached: 940,
        });
        let (text, _) = m.header(&catalog);
        assert!(text.ends_with("(90%)  ·  cache 94%"), "{text}");
        m.model_id = "nobody/nothing".into();
        m.context = 1500;
        assert_eq!(
            m.header(&catalog),
            (
                "nobody/nothing  ·  ctx 1.5k  ·  cache 94%".to_owned(),
                Level::Normal
            )
        );
        m.usage = Some(Usage::default());
        assert_eq!(m.header(&catalog).0, "nobody/nothing  ·  ctx 1.5k");
        assert_eq!(window(50, 20, 10), 11..21);
        assert_eq!(window(5, 0, 10), 0..5);
    }

    #[test]
    fn effort_sets_clears_and_picks() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/effort high", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetEffort { session: None, effort: Some(e) }) if e == "high"
        ));
        m.submit("/effort clear", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetEffort { effort: None, .. })
        ));
        // Picker: levels then `clear`; Enter on `clear` unsets.
        m.open_efforts(vec!["low".into(), "high".into()], PickTarget::Session);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(m.menu.is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetEffort { effort: None, .. })
        ));
        // The reply lands in the header; the echo event is not announced twice.
        m.apply(UiEvent::EffortSet(Some("high".into())));
        assert_eq!(m.effort.as_deref(), Some("high"));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "effort: high"));
        // No levels: a notice, no menu.
        m.open_efforts(Vec::new(), PickTarget::Session);
        assert!(m.menu.is_none());
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.ends_with("no reasoning effort levels"))
        );
    }

    /// 83 §3: `DefaultEffortChanged` moves the draft header but not an open
    /// session's; `/new` (`reset`) shows the default, not the last session's effort.
    #[test]
    fn default_effort_follows_the_engine() {
        let mut m = Model::default();
        m.apply(UiEvent::Agent(AgentEvent::DefaultEffortChanged {
            effort: Some("high".into()),
        }));
        assert_eq!(m.effort.as_deref(), Some("high"));
        m.session = Some(SessionId::new());
        m.effort = Some("low".into());
        m.apply(UiEvent::Agent(AgentEvent::DefaultEffortChanged {
            effort: None,
        }));
        assert_eq!(m.effort.as_deref(), Some("low"));
        assert_eq!(m.default_effort, None);
        m.default_effort = Some("medium".into());
        m.reset();
        assert_eq!(m.effort.as_deref(), Some("medium"));
        assert_eq!(m.default_effort.as_deref(), Some("medium"));
    }

    /// 75 §8: on an ACP session `/model` lists the harness's options,
    /// `/model <v>` and `/effort <v>` set them by alias, and a queued change
    /// is announced as landing after the turn.
    #[test]
    fn acp_session_model_and_effort_go_to_the_harness() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let id = SessionId::new();
        m.session = Some(id);
        m.agent = Some("omp".into());
        m.submit("/model", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::HarnessOptions(s)) if s == id));
        m.submit("/effort xhigh", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetHarnessOption { session, config_id, value })
                if session == id && config_id == "effort" && value == "xhigh"
        ));
        m.submit("/model openai/gpt-5.4", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetHarnessOption { config_id, value, .. })
                if config_id == "model" && value == "openai/gpt-5.4"
        ));
        let options: Vec<acp::ConfigOption> = serde_json::from_value(serde_json::json!([
            {"id":"model","name":"Model","category":"model","current":"a/x",
             "options":[{"value":"a/x","name":"X","description":null},{"value":"b/y","name":"Y","description":null}]},
            {"id":"thinking","name":"Thinking","category":"thought_level","current":"medium",
             "options":[{"value":"low","name":"Low","description":null},{"value":"medium","name":"Medium","description":null}]}
        ]))
        .unwrap();
        assert_eq!(
            harness_options_lines(&options),
            [
                "model: a/x  (has: a/x, b/y)",
                "effort: medium  (has: low, medium)"
            ]
        );
        m.apply(UiEvent::HarnessOptionSet(
            "model".into(),
            "b/y".into(),
            acp::SetOptionResult {
                options,
                pending: true,
            },
        ));
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t == "model → b/y after this turn")
        );
    }

    #[test]
    fn aws_picks_sets_and_clears_the_harness_profile() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let info = |profiles: &[&str], selected: Option<&str>| AwsProfiles {
            profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
            selected: selected.map(str::to_owned),
            default_profile: "default".into(),
        };
        // Bare `/aws` asks; two profiles open the picker: default chain first, then names.
        m.submit("/aws", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListAwsProfiles)));
        m.apply(UiEvent::AwsProfiles {
            info: info(&["dev", "prod"], Some("prod")),
            pick: true,
        });
        assert!(matches!(m.menu, Some(Menu::AwsProfiles { .. })) && !m.input_focused());
        let rows = m.aws_rows();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].starts_with("  default") && rows[2].starts_with("● prod"));
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(m.menu.is_none());
        assert!(matches!(rx.try_recv(), Ok(Command::SetAwsProfile(Some(p))) if p == "dev"));
        // Row 0 clears; so does `/aws clear`; `/aws name` sets directly.
        m.apply(UiEvent::AwsProfiles {
            info: info(&["dev", "prod"], Some("dev")),
            pick: true,
        });
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SetAwsProfile(None))));
        m.submit("/aws clear", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SetAwsProfile(None))));
        m.submit("/aws prod", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SetAwsProfile(Some(p))) if p == "prod"));
        // A set reply is status only; one profile and nothing selected needs no picker.
        m.apply(UiEvent::AwsProfiles {
            info: info(&["dev"], None),
            pick: false,
        });
        assert!(m.menu.is_none());
        m.apply(UiEvent::AwsProfiles {
            info: info(&["dev"], None),
            pick: true,
        });
        assert!(m.menu.is_none());
        // A stale explicit selection still opens the picker so row 0 can clear it.
        m.apply(UiEvent::AwsProfiles {
            info: info(&[], Some("gone")),
            pick: true,
        });
        assert!(matches!(m.menu, Some(Menu::AwsProfiles { .. })));
    }

    #[test]
    fn header_shows_effort_when_set() {
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let mut m = Model::new("nobody/nothing".into(), true);
        assert_eq!(m.header(&catalog).0, "nobody/nothing  ·  ctx 0");
        m.effort = Some("high".into());
        assert_eq!(m.header(&catalog).0, "nobody/nothing  ·  high  ·  ctx 0");
        m.model_id = "anthropic/claude-sonnet-4-5".into();
        assert!(m.header(&catalog).0.contains("  ·  high  ·  ctx "));
    }

    /// An ACP child (06 §8.2): `Loaded.harness_model` and `HarnessModelChanged`
    /// put the harness's LLM beside the harness name; the first report is
    /// silent, a switch away from a shown model is announced;
    /// `TurnStarted.model` (the harness name) does not disturb it.
    #[test]
    fn header_shows_harness_and_its_model_for_an_acp_child() {
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let mut m = Model::new("nobody/nothing".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.apply(UiEvent::History(
            s,
            Loaded {
                model: "omp".into(),
                agent: Some("omp".into()),
                ..Loaded::default()
            },
        ));
        assert_eq!(m.header(&catalog).0, "omp  ·  ctx 0");
        let infos = m.entries.len();
        m.apply_agent(AgentEvent::HarnessModelChanged {
            session: s,
            model: "anthropic/claude-opus-4-8".into(),
        });
        assert_eq!(
            m.header(&catalog).0,
            "omp · anthropic/claude-opus-4-8  ·  ctx 0"
        );
        assert_eq!(
            m.entries.len(),
            infos,
            "first report is silent: {:?}",
            m.entries
        );
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn: TurnId::new(),
            turn_seq: 1,
            text: "go".into(),
            model: "omp".into(),
        });
        assert!(
            m.header(&catalog)
                .0
                .starts_with("omp · anthropic/claude-opus-4-8")
        );
        m.apply_agent(AgentEvent::HarnessModelChanged {
            session: s,
            model: "openai/gpt-5.4".into(),
        });
        assert!(
            m.header(&catalog).0.starts_with("omp · openai/gpt-5.4"),
            "{}",
            m.header(&catalog).0
        );
        assert!(
            m.entries
                .iter()
                .any(|e| matches!(e, Entry::Info(t) if t == "harness model → openai/gpt-5.4")),
            "{:?}",
            m.entries
        );

        // A reload carries the stored value.
        m.apply(UiEvent::History(
            s,
            Loaded {
                model: "omp".into(),
                agent: Some("omp".into()),
                harness_model: Some("openai/gpt-5.4".into()),
                ..Loaded::default()
            },
        ));
        assert_eq!(m.header(&catalog).0, "omp · openai/gpt-5.4  ·  ctx 0");
    }

    /// Context bands (75 §5): the header prefers the harness's window over
    /// the catalog and turns at 75 / 90 for every session.
    #[test]
    fn header_context_bands_prefer_the_harness_window() {
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let mut m = Model::new("nobody/nothing".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.apply(UiEvent::History(
            s,
            Loaded {
                model: "omp".into(),
                agent: Some("omp".into()),
                context_size: Some(200_000),
                ..Loaded::default()
            },
        ));
        m.apply_agent(AgentEvent::ContextUsed {
            session: s,
            turn: TurnId::new(),
            context: 150_000,
            size: Some(200_000),
        });
        let (text, level) = m.header(&catalog);
        assert!(text.contains("ctx 150k / 200k (75%)"), "{text}");
        assert_eq!(level, Level::Warn);
        m.apply_agent(AgentEvent::ContextUsed {
            session: s,
            turn: TurnId::new(),
            context: 190_000,
            size: None,
        });
        let (text, level) = m.header(&catalog);
        assert!(text.contains("ctx 190k / 200k (95%)"), "{text}");
        assert_eq!(level, Level::Danger);

        let mut m = Model::new("anthropic/claude-sonnet-4-5".into(), true);
        let limit = catalog
            .get(&m.model_id)
            .expect("bundled model")
            .1
            .limit
            .context;
        m.context = limit * 74 / 100;
        assert_eq!(m.header(&catalog).1, Level::Normal);
        m.context = limit * 3 / 4;
        assert_eq!(m.header(&catalog).1, Level::Warn);
    }

    /// The harness manifest (75 §3): `skill:` entries are the status badge,
    /// the first report is silent, a change in count is one line.
    #[test]
    fn status_shows_harness_skills_and_announces_manifest_changes() {
        let cmd = |name: &str| agent::HarnessCommand {
            name: name.into(),
            description: String::new(),
            hint: None,
        };
        let mut m = Model::new("nobody/nothing".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.loaded_skills = vec!["native".into()];
        assert!(m.status().ends_with("skills: native"), "{}", m.status());
        let infos = m.entries.len();
        m.apply_agent(AgentEvent::HarnessCommandsChanged {
            session: s,
            commands: vec![
                cmd("skill:ponytail"),
                cmd("skill:caveman"),
                cmd("/ponytail"),
            ],
        });
        assert!(
            m.status().ends_with("harness skills: ponytail, caveman"),
            "{}",
            m.status()
        );
        assert_eq!(
            m.entries.len(),
            infos,
            "first report is silent: {:?}",
            m.entries
        );
        m.apply_agent(AgentEvent::HarnessCommandsChanged {
            session: s,
            commands: vec![cmd("/ponytail"), cmd("init")],
        });
        assert!(
            m.status().ends_with("harness commands: 2"),
            "{}",
            m.status()
        );
        assert_eq!(
            m.entries
                .iter()
                .filter(|e| matches!(e, Entry::Info(t) if t == "harness commands → 2"))
                .count(),
            1,
            "{:?}",
            m.entries
        );
    }

    #[test]
    fn large_paste_becomes_a_token_that_submit_expands() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        assert_eq!(m.paste("one\r\ntwo\r"), "one\ntwo\n");
        let big: String = (0..69).map(|i| format!("line {i}\r\n")).collect();
        let token = m.paste(&big);
        assert_eq!(token, "[⎘ paste 1: 69 lines]");
        assert_eq!(m.paste(&"x".repeat(500)), "[⎘ paste 2: 500 chars]");
        m.submit(&format!("look: {token} and [⎘ paste 2: 500 chars]"), &tx);
        let Some(Command::Send { text, .. }) = rx.try_recv().ok() else {
            panic!("no send")
        };
        assert!(text.starts_with("look: line 0\nline 1\n"));
        assert!(text.ends_with(&"x".repeat(500)));
        assert!(m.pastes.is_empty(), "held pastes are dropped on submit");
        assert!(matches!(&m.entries[0], Entry::User { text: t, .. } if t.contains(&token)));
        // The agent echoes the expanded text; the token card already shows it.
        let session = SessionId::new();
        m.apply_agent(AgentEvent::TurnStarted {
            session,
            turn: runtime::TurnId::new(),
            turn_seq: 0,
            text: text.clone(),
            model: "m".into(),
        });
        assert_eq!(
            m.entries.len(),
            1,
            "expanded echo must not add a second card"
        );
        m.apply_agent(AgentEvent::TurnStarted {
            session,
            turn: runtime::TurnId::new(),
            turn_seq: 2,
            text: "from another frontend".into(),
            model: "m".into(),
        });
        assert_eq!(m.entries.len(), 2, "foreign input is still echoed");
    }

    #[test]
    fn attached_images_leave_the_text_and_travel_as_files() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let shot = Image {
            path: PathBuf::from("/tmp/a.png"),
            w: 1440,
            h: 900,
        };
        let dropped = Image {
            path: PathBuf::from("/tmp/my shot.jpg"),
            w: 0,
            h: 0,
        };
        let t1 = m.attach(shot.clone());
        let t2 = m.attach(dropped.clone());
        assert_eq!(t1, "[⎘ image 1: 1440×900]");
        assert_eq!(t2, "[⎘ image 2: my shot.jpg]");
        m.submit(&format!("describe {t1} {t2}"), &tx);
        let Ok(Command::Send { text, files, .. }) = rx.try_recv() else {
            panic!("no send");
        };
        assert_eq!(text, "describe");
        assert_eq!(files, vec![shot.path.clone(), dropped.path.clone()]);
        assert!(m.pastes.is_empty());
        assert!(matches!(&m.entries[0], Entry::User { text: t, .. } if t.contains(&t1)));
        // A token the user deleted drops its image.
        m.attach(shot);
        let t2 = m.attach(dropped);
        m.submit(&format!("{t2} only this"), &tx);
        let Ok(Command::Send { text, files, .. }) = rx.try_recv() else {
            panic!("no send");
        };
        assert_eq!(text, "only this");
        assert_eq!(files, vec![PathBuf::from("/tmp/my shot.jpg")]);
    }

    #[test]
    fn vision_sets_clears_and_shows_the_harness_fallback() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let info = |id: &str| ModelInfo {
            id: id.into(),
            name: "Haiku".into(),
            provider: "anthropic".into(),
            context: 200_000,
            reasoning: false,
            image: true,
            efforts: vec![],
            budget_min: None,
            budget_max: None,
        };
        // The startup fetch primes state silently.
        m.apply(UiEvent::VisionModel(Some(info(
            "anthropic/claude-haiku-4-5",
        ))));
        assert_eq!(
            m.vision_model.as_ref().map(|v| v.id.as_str()),
            Some("anthropic/claude-haiku-4-5")
        );
        assert!(m.entries.is_empty());
        // Bare `/vision` asks and prints the canonical reply; never a prompt.
        m.submit("/vision", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::GetVisionModel)));
        m.apply(UiEvent::VisionModel(Some(info(
            "anthropic/claude-haiku-4-5",
        ))));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Info(t)) if t.starts_with("vision fallback: anthropic/claude-haiku-4-5 (Haiku)")
        ));
        // Set sends the id; `off` and `clear` send None.
        m.submit("/vision openai/gpt-5", &tx);
        assert!(
            matches!(rx.try_recv(), Ok(Command::SetVisionModel(Some(id))) if id == "openai/gpt-5")
        );
        m.submit("/vision off", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SetVisionModel(None))));
        m.submit("/vision clear", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::SetVisionModel(None))));
        assert!(rx.try_recv().is_err(), "no Send slipped through");
        // A rejected selection is an error line; the old selection stands,
        // and the flag it left behind does not make a later silent reply print.
        m.apply(UiEvent::Error("not image-capable".into()));
        assert!(matches!(m.entries.last(), Some(Entry::Error(e)) if e == "not image-capable"));
        assert!(m.vision_model.is_some());
        let n = m.entries.len();
        m.apply(UiEvent::VisionModel(None));
        assert!(m.vision_model.is_none() && m.entries.len() == n);
        // Off reply after `/vision off` prints the off line.
        m.submit("/vision off", &tx);
        m.apply(UiEvent::VisionModel(None));
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.starts_with("vision fallback: off"))
        );
        // Harness-wide: `/new` keeps it.
        m.apply(UiEvent::VisionModel(Some(info(
            "anthropic/claude-haiku-4-5",
        ))));
        m.submit("/new", &tx);
        assert!(m.vision_model.is_some());
    }

    #[test]
    fn attach_notice_names_the_vision_fallback_for_a_text_only_model() {
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        let (provider, model) = catalog
            .providers()
            .flat_map(|p| p.models.values().map(move |m| (&p.id, m)))
            .find(|(_, m)| !m.image && m.limit.context > 0)
            .map(|(p, m)| (p.clone(), m.id.clone()))
            .expect("a text-only model in the bundled catalog");
        let mut m = Model::new(format!("{provider}/{model}"), true);
        m.catalog = Some(Arc::new(catalog));
        let shot = Image {
            path: PathBuf::from("/tmp/a.png"),
            w: 1,
            h: 1,
        };
        m.attach(shot.clone());
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.contains("text-only") && t.contains("omitted") && t.contains("/vision <model>")),
            "{:?}",
            m.entries.last()
        );
        m.apply(UiEvent::VisionModel(Some(ModelInfo {
            id: "anthropic/claude-haiku-4-5".into(),
            name: "Haiku".into(),
            provider: "anthropic".into(),
            context: 200_000,
            reasoning: false,
            image: true,
            efforts: vec![],
            budget_min: None,
            budget_max: None,
        })));
        m.attach(shot);
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.contains("anthropic/claude-haiku-4-5 will inspect the image")),
            "{:?}",
            m.entries.last()
        );
    }

    #[test]
    fn tail_reaches_the_requested_height() {
        let mut m = Model::default();
        for _ in 0..10 {
            m.entries.push(Entry::Info("x".into()));
            m.entries.push(Entry::User {
                text: "a\nb".into(),
                turn: None,
            });
        }
        assert_eq!(m.tail(1, 80), (19, 0));
        assert_eq!(m.tail(7, 80), (18, 0), "Info 1 + User 6 rows");
        assert_eq!(m.tail(1000, 80), (0, 0), "short histories draw whole");
        // A long paragraph wraps: at 20 columns it fills the window alone.
        m.entries.push(Entry::User {
            text: "x".repeat(200),
            turn: None,
        });
        assert_eq!(m.tail(8, 20), (20, 0));
        // A reply taller than the window draws only its last lines: 40
        // one-row paragraphs (blank-separated → 79 rendered lines) plus a
        // 2-row plain tail into a 10-row window keeps 8 rendered rows.
        let mut t = markdown::Text::new();
        for i in 0..40 {
            t.push(&format!("paragraph {i}\n\n"));
        }
        t.push("tail one\ntail two");
        t.reveal_all();
        m.entries.push(Entry::Assistant(Box::new(t)));
        let (from, skip) = m.tail(10, 80);
        assert_eq!(from, 21);
        assert_eq!(skip, 79 - 8);
    }

    #[test]
    fn scroll_anchors_to_an_entry_and_repins_at_the_bottom() {
        let mut m = Model::default();
        // 20 user cards of 6 rows each (2 text rows + border and title).
        for i in 0..20 {
            m.entries.push(Entry::User {
                text: format!("{i}\n{i}"),
                turn: None,
            });
        }
        // 120 rows of history, a 10-row view. Short histories cannot scroll.
        let mut short = Model::default();
        short.entries.push(Entry::Info("x".into()));
        short.scroll(-5, 10, 80);
        assert_eq!(short.anchor, None);

        // Pinned window's first row is 10 rows up: entry 18, 2 rows in.
        m.scroll(-1, 10, 80);
        assert_eq!(m.anchor, Some((18, 1)));
        // A page up crosses entries.
        m.scroll(-10, 10, 80);
        assert_eq!(m.anchor, Some((16, 3)));
        // The top stops at the first entry.
        m.scroll(-1000, 10, 80);
        assert_eq!(m.anchor, Some((0, 0)));
        let w = m.window(10, 80);
        assert_eq!((w.from, w.top, w.skip), (0, 0, 0));
        assert_eq!(w.to, 4, "two views' worth of 6-row entries");
        // Scrolling down until less than a view remains below re-pins.
        m.scroll(109, 10, 80);
        assert_eq!(m.anchor, Some((18, 1)));
        m.scroll(1, 10, 80);
        assert_eq!(m.anchor, None);
        // An anchor at an entry's first row with exactly one more view below
        // it is not the bottom yet (the entry itself is still above).
        m.scroll(-1000, 12, 80);
        m.scroll(102, 12, 80);
        assert_eq!(m.anchor, Some((17, 0)));
        // Growth below an anchor leaves it where it was.
        m.scroll(-30, 10, 80);
        let before = m.anchor;
        m.entries.push(Entry::User {
            text: "new".into(),
            turn: None,
        });
        assert_eq!(m.anchor, before);
        // Reloading the transcript follows the tail again.
        m.entries.clear();
        assert_eq!(m.window(10, 80).to, 0);
    }

    #[test]
    fn steer_waits_in_the_queue_until_injected() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.streaming = true;
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn: TurnId::new(),
            text: "working".into(),
        });
        m.submit("also do X", &tx);
        let Ok(Command::Steer { id, text, .. }) = rx.try_recv() else {
            panic!("mid-turn input must steer");
        };
        assert_eq!(text, "also do X");
        assert_eq!(m.queued.len(), 1);
        assert!(
            !m.entries.iter().any(|e| matches!(e, Entry::User { .. })),
            "a queued steer has no transcript card yet"
        );
        assert!(m.status().contains("1 queued"));
        // Another frontend's steer shows up too; ours is not duplicated.
        let other = InputId::new();
        for id in [other, id] {
            m.apply_agent(AgentEvent::InputQueued {
                session: s,
                id,
                text: "x".into(),
                defer: false,
            });
        }
        assert_eq!(m.queued.len(), 2);
        assert!(
            matches!(m.entries.last(), Some(Entry::Assistant(t)) if !t.is_done()),
            "queue bookkeeping must not cut the streaming tail"
        );
        let turn = TurnId::new();
        m.apply_agent(AgentEvent::InputInjected {
            session: s,
            id,
            turn,
        });
        assert_eq!(m.queued.len(), 1, "injected steer left the queue");
        assert!(
            matches!(m.entries.last(), Some(Entry::User { text: t, turn: None }) if t == "also do X"),
            "the card lands after the streamed text"
        );
        let before = m.entries.len();
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn,
            turn_seq: 7,
            text: "also do X".into(),
            model: "m".into(),
        });
        assert_eq!(
            m.entries.len(),
            before,
            "the turn echo is not a second card"
        );
        assert!(
            matches!(m.entries.last(), Some(Entry::User { turn: Some(7), .. })),
            "the steer that opened the turn takes its seq"
        );
        m.apply_agent(AgentEvent::InputWithdrawn {
            session: s,
            id: other,
        });
        assert!(m.queued.is_empty());
    }

    #[test]
    fn interject_flags_the_steer_and_sends_idle_input_plainly() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.streaming = true;
        m.submit_now("stop, do Y instead", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Steer {
                interject: true,
                ..
            })
        ));
        m.submit("also do X", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Steer {
                interject: false,
                ..
            })
        ));
        // Idle: a plain send.
        m.streaming = false;
        m.submit_now("hello", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Send { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn skills_inject_by_slash_and_picker_and_badge_tracks_the_frame() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        let meta = |name: &str| SkillMeta {
            name: name.into(),
            description: format!("{name} skill"),
            path: format!("/skills/{name}.md"),
            always: false,
            repo: None,
        };
        // The index arrives at start without opening the picker; names now resolve.
        m.apply(UiEvent::Skills(vec![meta("caveman"), meta("ponytail")]));
        assert!(m.menu.is_none());
        m.submit("/caveman lite", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::UseSkill { session: Some(id), name, args }) if id == s && name == "caveman" && args == "lite"
        ));
        // An unknown word is message text, not an error — as is a path even before the index arrives.
        m.submit("/nonesuch", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Send { text, .. }) if text == "/nonesuch"));
        assert!(matches!(m.entries.last(), Some(Entry::User { text, .. }) if text == "/nonesuch"));
        m.submit("/Users/x/y.txt is broken, look at it", &tx);
        assert!(
            matches!(rx.try_recv(), Ok(Command::Send { text, .. }) if text == "/Users/x/y.txt is broken, look at it")
        );
        let mut fresh = Model::new("m".into(), true);
        fresh.session = Some(s);
        fresh.submit("/etc/hosts has a bad entry", &tx);
        assert!(
            matches!(rx.try_recv(), Ok(Command::Send { text, .. }) if text == "/etc/hosts has a bad entry")
        );
        fresh.submit("/", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Send { text, .. }) if text == "/"));
        fresh.submit("/maybe lite", &tx);
        assert!(
            matches!(rx.try_recv(), Ok(Command::UseSkill { name, args, .. }) if name == "maybe" && args == "lite")
        );

        // `/skills` opens the picker; Enter uses, `s` shows.
        m.submit("/skills", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListSkills)));
        m.apply(UiEvent::Skills(vec![meta("caveman"), meta("ponytail")]));
        assert!(matches!(m.menu, Some(Menu::Skills { .. })));
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Char('s'), "", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::LoadSkill(n)) if n == "ponytail"));
        assert!(m.menu.is_none());
        m.submit("/skills", &tx);
        rx.try_recv().unwrap();
        m.apply(UiEvent::Skills(vec![meta("caveman")]));
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::UseSkill { name, args, .. }) if name == "caveman" && args.is_empty()
        ));

        // The injected turn renders as a skill card plus the user's words.
        let text = agent::skills::inject_text("caveman", "# Caveman\nGrunt.", "lite");
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn: TurnId::new(),
            turn_seq: 1,
            text,
            model: "m".into(),
        });
        let n = m.entries.len();
        assert!(
            matches!(&m.entries[n - 2], Entry::Card { kind: CardKind::Skill, title, .. } if title == "caveman")
        );
        assert!(matches!(&m.entries[n - 1], Entry::User { text: t, turn: Some(1) } if t == "lite"));

        // The badge follows the agent's set: grows on load, empties on compaction.
        m.apply_agent(AgentEvent::SkillsChanged {
            session: s,
            skills: vec!["caveman".into(), "ponytail".into()],
        });
        assert!(m.status().ends_with("skills: caveman, ponytail"));
        m.apply_agent(AgentEvent::SkillsChanged {
            session: s,
            skills: Vec::new(),
        });
        assert!(!m.status().contains("skills:"));
    }

    #[test]
    fn alt_up_withdraws_only_into_an_empty_editor() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.streaming = true;
        m.submit("first", &tx);
        m.submit("second", &tx);
        while rx.try_recv().is_ok() {}
        m.withdraw(false, &tx);
        assert!(
            rx.try_recv().is_err(),
            "a non-empty editor is never overwritten"
        );
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "editor not empty"));
        m.withdraw(true, &tx);
        let newest = m.queued[1].id;
        assert!(matches!(rx.try_recv(), Ok(Command::Withdraw { id, .. }) if id == newest));
        m.apply(UiEvent::Withdrawn {
            id: newest,
            text: "second".into(),
        });
        assert_eq!(m.queued.len(), 1);
        assert_eq!(m.recall.take().as_deref(), Some("second"));
    }

    #[test]
    fn unsent_text_leaves_the_transcript_and_returns_to_the_editor() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        m.submit("hello", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Send { .. })));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::User { turn: None, .. })
        ));
        assert!(m.awaiting_session);
        m.apply(UiEvent::Unsent {
            id: None,
            text: "hello".into(),
        });
        assert!(!m.entries.iter().any(|e| matches!(e, Entry::User { .. })));
        assert!(m.sent.is_none() && !m.awaiting_session);
        assert_eq!(m.recall.take().as_deref(), Some("hello"));
        // A refused steer leaves the queue the same way.
        m.session = Some(SessionId::new());
        m.streaming = true;
        m.submit("later", &tx);
        let id = m.queued[0].id;
        m.apply(UiEvent::Unsent {
            id: Some(id),
            text: "later".into(),
        });
        assert!(m.queued.is_empty());
        assert_eq!(m.recall.take().as_deref(), Some("later"));
    }

    #[test]
    fn ask_picks_types_skips_and_dismisses() {
        use tools::{AskId, Choice, Question};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        let q = |id: &str, n: usize, rec: Option<usize>| Question {
            id: id.into(),
            question: format!("{id}?"),
            options: (0..n)
                .map(|i| Choice {
                    label: format!("o{i}"),
                    description: None,
                })
                .collect(),
            recommended: rec,
        };
        let req = AskRequested {
            id: AskId(Default::default()),
            session: s,
            questions: vec![q("a", 3, Some(2)), q("b", 2, None), q("c", 0, None)],
        };
        // A stranger's ask is ignored; ours opens on the recommended row.
        m.apply(UiEvent::Ask(AskRequested {
            session: SessionId::new(),
            ..req.clone()
        }));
        assert!(m.ask().is_none());
        m.apply(UiEvent::Ask(req.clone()));
        assert_eq!(m.ask().unwrap().cursor, 2);
        assert!(!m.input_focused());
        // Enter picks the cursor row; then `s` skips; then the freeform-only
        // question goes straight to typing and Enter sends the text.
        assert_eq!(m.ask_key(KeyCode::Enter, "", &tx), None);
        assert_eq!(m.ask().unwrap().current, 1);
        m.ask_key(KeyCode::Char('s'), "", &tx);
        assert_eq!(m.ask().unwrap().current, 2);
        m.ask_key(KeyCode::Enter, "", &tx);
        assert!(m.ask().unwrap().typing && m.input_focused());
        assert_eq!(
            m.ask_key(KeyCode::Enter, "  ", &tx),
            None,
            "blank text stays"
        );
        assert_eq!(
            m.ask_key(KeyCode::Enter, " mine ", &tx),
            Some(String::new())
        );
        assert!(m.ask().is_none());
        match rx.try_recv() {
            Ok(Command::AnswerAsk { id, answers }) => {
                assert_eq!(id, req.id);
                assert_eq!(
                    answers,
                    Some(vec![
                        Answer::Choice { index: 2 },
                        Answer::Skipped,
                        Answer::Text {
                            text: "mine".into()
                        },
                    ])
                );
            }
            other => panic!("{other:?}"),
        }
        // Digits pick directly; Esc dismisses the whole set.
        m.apply(UiEvent::Ask(req.clone()));
        m.ask_key(KeyCode::Char('1'), "", &tx);
        assert_eq!(m.ask().unwrap().current, 1);
        m.ask_key(KeyCode::Esc, "", &tx);
        assert!(m.ask().is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::AnswerAsk { answers: None, .. })
        ));
        // Resolved elsewhere: the box goes away.
        m.apply(UiEvent::Ask(req.clone()));
        m.apply(UiEvent::AskResolved(AskResolved {
            id: req.id,
            dismissed: false,
            timed_out: false,
        }));
        assert!(m.ask().is_none());
    }

    /// A model showing session `s` with two finished turns: seqs 1 and 3.
    fn two_turns(s: SessionId) -> Model {
        let mut m = Model::new("m".into(), true);
        m.session = Some(s);
        for (seq, q, a) in [
            (1, "first question\nmore", "reply one"),
            (3, "second", "reply two"),
        ] {
            m.entries.push(Entry::User {
                text: q.into(),
                turn: Some(seq),
            });
            m.entries
                .push(Entry::Assistant(Box::new(markdown::Text::complete(a))));
        }
        m
    }

    #[test]
    fn turn_started_stamps_the_own_send_with_its_seq() {
        let (tx, _rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        m.submit("hello", &tx);
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn: TurnId::new(),
            turn_seq: 5,
            text: "hello".into(),
            model: "m".into(),
        });
        assert_eq!(m.entries.len(), 1, "own send is not echoed twice");
        assert!(matches!(&m.entries[0], Entry::User { text, turn: Some(5) } if text == "hello"));
        assert_eq!(m.turn_seq, Some(5));
    }

    /// An access failure (expired SSO) mid-turn stays readable: the turn's
    /// end clears the busy state, and the authoritative reload carries the
    /// same row as `HistoryEntry::Error` — one error line, not two, not none.
    #[test]
    fn provider_error_outlives_turn_end_and_reload() {
        let (tx, _rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        let turn = TurnId::new();
        m.session = Some(s);
        m.submit("hello", &tx);
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn,
            turn_seq: 1,
            text: "hello".into(),
            model: "m".into(),
        });
        assert!(m.streaming);
        let message = "provider: SSO session expired; run `aws sso login`";
        m.apply_agent(AgentEvent::Error {
            session: s,
            turn: Some(turn),
            message: message.into(),
        });
        m.apply_agent(AgentEvent::TurnFinished {
            session: s,
            turn,
            stop_reason: "error".into(),
            usage: Usage::default(),
            context: 0,
        });
        assert!(!m.streaming, "the failed turn is over");
        let errors = |m: &Model| {
            m.entries
                .iter()
                .filter(|e| matches!(e, Entry::Error(t) if t == message))
                .count()
        };
        assert_eq!(errors(&m), 1);
        // Reopen: the stored row renders as the same error line.
        m.apply(UiEvent::History(
            s,
            Loaded {
                history: vec![
                    HistoryEntry::User {
                        text: "hello".into(),
                        turn: Some(1),
                        context: None,
                        peer: None,
                        vision: None,
                    },
                    HistoryEntry::Error {
                        text: message.into(),
                        context: None,
                    },
                ],
                ..Loaded::default()
            },
        ));
        assert_eq!(errors(&m), 1);
        assert!(!m.streaming);
        assert!(matches!(m.entries.last(), Some(Entry::Error(_))));
        // A long provider message is measured wrapped, not as one row.
        assert_eq!(Entry::Error("x".repeat(50)).height(20), 3);
    }

    /// 109 §10.3: `TurnResumed` interjects the divider and sets the status
    /// note; `ResumeReport` fills the counts (a skipped call also gets a
    /// transcript line); `TurnFinished` clears the note; a reload renders the
    /// stored partial under its own divider.
    #[test]
    fn resume_events_draw_divider_and_status_note() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        let turn = TurnId::new();
        m.session = Some(s);
        let at = chrono::Utc::now();
        m.apply_agent(AgentEvent::TurnResumed {
            session: s,
            turn,
            kind: ResumeKind::Interrupted,
            interrupted_at: at,
            resumed_at: at,
            partial_dropped: true,
        });
        assert!(
            matches!(m.entries.last(), Some(Entry::Divider(l)) if l == "resumed after restart"),
            "{:?}",
            m.entries
        );
        let hhmm = at.with_timezone(&chrono::Local).format("%H:%M").to_string();
        assert!(
            m.status()
                .contains(&format!("interrupted by a restart at {hhmm} — resumed"))
        );
        m.apply_agent(AgentEvent::ResumeReport {
            session: s,
            turn,
            replayed: vec![
                ("c1".into(), "read_file".into()),
                ("c2".into(), "grep".into()),
            ],
            interrupted: vec![("c3".into(), "git push".into())],
            lost_jobs: vec![],
            children_restored: 0,
            children_handed_off: 0,
        });
        let note = "resumed · 2 replayed · 1 skipped: git push — effect unknown";
        assert!(m.status().contains(note), "{}", m.status());
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == note));
        m.apply_agent(AgentEvent::TurnFinished {
            session: s,
            turn,
            stop_reason: "end_turn".into(),
            usage: Usage::default(),
            context: 0,
        });
        assert!(!m.status().contains("resumed"), "{}", m.status());

        m.apply(UiEvent::History(
            s,
            Loaded {
                history: vec![HistoryEntry::Interrupted {
                    text: "half a reply".into(),
                    at,
                    context: None,
                }],
                ..Loaded::default()
            },
        ));
        let n = m.entries.len();
        assert!(
            matches!(&m.entries[n - 2], Entry::Divider(l) if l == "interrupted — resuming"),
            "{:?}",
            m.entries
        );
        assert!(matches!(&m.entries[n - 1], Entry::Info(t) if t == "half a reply"));
    }

    /// One consultation (52 §6): `Questioning` and `Inspecting` name the
    /// model in the status line, the terminal step clears it and leaves
    /// exactly one collapsed vision card, never a `◆ you` row.
    #[test]
    fn vision_progress_drives_the_status_and_settles_into_one_card() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        let turn = TurnId::new();
        m.session = Some(s);
        m.streaming = true;
        let source = agent::VisionSource {
            target: ContextTarget::Binary {
                owner: agent::ContextOwner::Message { seq: 4 },
                part: 1,
                image: None,
            },
            fingerprint: "abcd".into(),
            label: "shot.png".into(),
        };
        let step = |stage, model: &str, seq| AgentEvent::VisionProgress {
            session: s,
            turn: Some(turn),
            stage,
            model: model.into(),
            sources: vec![source.clone()],
            seq,
        };
        m.apply_agent(step(VisionStage::Questioning, "text/main", None));
        assert_eq!(
            m.activity().as_deref(),
            Some("asking text/main what to look for…")
        );
        assert!(m.animating());
        m.apply_agent(step(VisionStage::Inspecting, "prov/eyes", None));
        assert_eq!(
            m.activity().as_deref(),
            Some("prov/eyes inspecting 1 image(s)…")
        );
        m.apply_agent(step(VisionStage::Completed, "prov/eyes", Some(5)));
        assert!(m.vision.is_none());
        assert_eq!(m.activity().as_deref(), Some("thinking…"));
        let cards: Vec<_> = m
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Entry::Card {
                        kind: CardKind::Vision,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(cards.len(), 1);
        assert!(matches!(
            cards[0],
            Entry::Card { title, expanded: false, .. }
                if title == "vision: prov/eyes inspected 1 image(s)"
        ));
        assert!(!m.entries.iter().any(|e| matches!(e, Entry::User { .. })));
        // A failure names the count it missed and also settles the status.
        m.apply_agent(step(VisionStage::Inspecting, "prov/eyes", None));
        m.apply_agent(step(VisionStage::Failed, "prov/eyes", Some(7)));
        assert!(m.vision.is_none());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Card { kind: CardKind::Vision, title, .. })
                if title == "vision: prov/eyes failed on 1 image(s)"
        ));
    }

    /// A reloaded note row (`HistoryEntry::User { vision }`) is the vision
    /// card with the note body behind Ctrl+O, not the human's `◆ you` row;
    /// removing it from context keeps the `⊘` archive card convention.
    #[test]
    fn vision_note_history_renders_as_a_card_not_a_user_row() {
        let opts = markdown::Options::default();
        let source = |seq| agent::VisionSource {
            target: ContextTarget::Binary {
                owner: agent::ContextOwner::Message { seq },
                part: 1,
                image: None,
            },
            fingerprint: format!("f{seq}"),
            label: "shot.png".into(),
        };
        let note = |statuses: &[VisionStatus]| VisionNote {
            version: agent::vision::NOTE_VERSION,
            question_model: "text/main".into(),
            vision_model: "prov/eyes".into(),
            observations: statuses
                .iter()
                .enumerate()
                .map(|(i, &status)| agent::VisionObservation {
                    source: source(i as i64),
                    status,
                })
                .collect(),
        };
        let body = "<vision_observation>\nQ: what is highlighted?\nA: $420\n</vision_observation>";
        let row = |note: VisionNote, removed| HistoryEntry::User {
            text: body.into(),
            turn: None,
            context: Some(ContextEntry {
                target: ContextTarget::Message { seq: 9 },
                active: true,
                removed,
                remove_with: vec![],
                attachments: vec![],
                vision: Some(note.clone()),
                at: None,
            }),
            peer: None,
            vision: Some(note),
        };
        let out = from_history(
            row(note(&[VisionStatus::Answered, VisionStatus::Failed]), false),
            &opts,
        );
        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0],
            Entry::Card { kind: CardKind::Vision, expanded: false, title, .. }
                if title == "vision: prov/eyes inspected 1 of 2 image(s)"
        ));
        let out = from_history(row(note(&[VisionStatus::Answered]), true), &opts);
        assert!(matches!(
            &out[0],
            Entry::Card { kind: CardKind::Removed, title, .. }
                if title == "vision · removed from context"
        ));
    }

    /// The first send is refused before any turn (`Error { turn: None }`)
    /// and the event outruns the `SessionCreated` reply: the error shows once
    /// the reply names that session; another session's does not attach.
    #[test]
    fn send_rejection_before_session_reply_is_shown_for_that_session_only() {
        let (tx, _rx) = unbounded_channel();
        let mut m = Model::new("m".into(), true);
        let (ours, theirs) = (SessionId::new(), SessionId::new());
        m.submit("hello", &tx);
        assert!(m.awaiting_session && m.session.is_none());
        m.apply_agent(AgentEvent::Error {
            session: theirs,
            turn: None,
            message: "elsewhere".into(),
        });
        m.apply_agent(AgentEvent::Error {
            session: ours,
            turn: None,
            message: "attachment rejected".into(),
        });
        assert!(m.entries.iter().all(|e| !matches!(e, Entry::Error(_))));
        m.apply(UiEvent::SessionCreated(ours));
        assert_eq!(m.session, Some(ours));
        assert!(!m.awaiting_session && !m.streaming);
        let errors: Vec<&str> = m
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Error(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(errors, ["attachment rejected"]);
        // Once the session is known, its errors land directly; strangers' do not.
        m.apply_agent(AgentEvent::Error {
            session: theirs,
            turn: None,
            message: "still elsewhere".into(),
        });
        assert_eq!(
            m.entries
                .iter()
                .filter(|e| matches!(e, Entry::Error(_)))
                .count(),
            1
        );
    }

    #[test]
    fn regen_truncates_at_the_turn_and_refuses_mid_turn() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = two_turns(s);
        m.submit("/regen 1", &tx);
        assert!(
            m.entries.is_empty(),
            "turn 1 and everything after it is dropped"
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Regenerate { session, turn: 1 }) if session == s
        ));

        let mut m = two_turns(s);
        m.submit("/regen 2", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Regenerate { turn: 3, .. })
        ));
        assert!(
            m.entries
                .iter()
                .any(|e| matches!(e, Entry::User { text, .. } if text == "first question\nmore"))
        );

        let mut m = two_turns(s);
        m.streaming = true;
        m.submit("/regen", &tx);
        assert!(rx.try_recv().is_err(), "nothing is sent while a turn runs");
        assert!(m.menu.is_none());
        assert!(
            matches!(m.entries.last(), Some(Entry::Error(t)) if t.starts_with("turn in flight"))
        );

        let mut m = two_turns(s);
        m.submit("/regen 9", &tx);
        assert!(matches!(m.entries.last(), Some(Entry::Error(t)) if t == "no turn #9"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn edit_recalls_the_opener_then_replaces_the_turn() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = two_turns(s);
        m.submit("/edit 2", &tx);
        assert_eq!(m.editing, Some((2, 3)));
        assert_eq!(m.recall.take().as_deref(), Some("second"));
        assert!(m.status().contains("editing #2"));
        assert!(rx.try_recv().is_err(), "edit sends nothing until ⏎");
        // A slash command in between runs and leaves the edit pending.
        m.submit("/sessions", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListSessions)));
        assert_eq!(m.editing, Some((2, 3)));

        m.submit("new text", &tx);
        assert!(m.editing.is_none());
        assert_eq!(m.entries.len(), 3, "the edited turn's tail is gone");
        assert!(
            matches!(m.entries.last(), Some(Entry::User { text, turn: None }) if text == "new text")
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::EditTurn { session, turn: 3, text }) if session == s && text == "new text"
        ));
        assert!(!m.status().contains("editing"));

        m.submit("/edit 1", &tx);
        assert!(m.editing.is_some());
        assert!(m.cancel_edit());
        assert!(m.editing.is_none());
        assert_eq!(m.recall.take().as_deref(), Some(""));
        assert!(!m.cancel_edit(), "nothing left to cancel");
    }

    #[test]
    fn fork_picker_opens_on_the_newest_turn() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = two_turns(s);
        m.submit("/fork", &tx);
        assert!(matches!(
            &m.menu,
            Some(Menu::Turns { items, cursor: 1, action: TurnAction::Fork })
                if items.len() == 2 && items[0].2 == "first question" && items[1] == (2, 3, "second".to_owned())
        ));
        assert!(!m.input_focused(), "the picker is modal");
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        assert_eq!(m.menu_key(KeyCode::Enter, "", &tx), MenuKey::Consumed);
        assert!(m.menu.is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Fork { session, through: Some(3) }) if session == s
        ));
        assert_eq!(m.entries.len(), 4, "the source transcript is untouched");

        // The reply switches to the copy.
        let id = SessionId::new();
        m.apply(UiEvent::Forked(
            id,
            Loaded {
                history: vec![HistoryEntry::User {
                    text: "first question\nmore".into(),
                    turn: Some(1),
                    context: None,
                    peer: None,
                    vision: None,
                }],
                ..Loaded::default()
            },
        ));
        assert_eq!(m.session, Some(id));
        assert!(matches!(&m.entries[0], Entry::User { turn: Some(1), .. }));
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t.starts_with("forked")));

        let mut m = Model::new("m".into(), true);
        m.session = Some(s);
        m.submit("/fork", &tx);
        assert!(matches!(m.entries.last(), Some(Entry::Error(t)) if t == "no turns yet"));
    }

    #[test]
    fn show_card_joins_its_artifact_and_expands_the_markdown() {
        let s = SessionId::new();
        let turn = TurnId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        // Progress before the call starts names the file from the partial JSON.
        m.apply_agent(AgentEvent::ToolCallPending {
            session: s,
            turn,
            call_id: "c1".into(),
            name: "show".into(),
        });
        m.apply_agent(AgentEvent::ToolCallProgress {
            session: s,
            turn,
            call_id: "c1".into(),
            args: r##"{"name": "note.md", "content": "# Hel"##.into(),
        });
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Pending { name, progress: Some(Progress { show: Some(s), bytes: 37 }), .. })
                if name == "show" && s.name.as_deref() == Some("note.md") && s.writing
        ));
        let Some(Entry::Pending {
            name,
            progress: Some(p),
            ..
        }) = m.entries.last()
        else {
            unreachable!()
        };
        assert_eq!(progress_line(name, p), "writing note.md… 37 B");
        let shown = Progress {
            show: Some(surface::PartialShow {
                name: Some("x.pdf".into()),
                writing: false,
            }),
            bytes: 40,
        };
        assert_eq!(progress_line("show", &shown), "showing x.pdf…");
        assert_eq!(progress_line("shell", &shown), "preparing shell… 40 B");
        m.apply_agent(AgentEvent::ToolCallStarted {
            session: s,
            turn,
            call_id: "c1".into(),
            name: "show".into(),
            args: serde_json::json!({"name": "note.md", "content": "# Hello\n\nbody"}),
        });
        let path =
            std::env::temp_dir().join(format!("theuth-tui-artifact-{}.md", std::process::id()));
        std::fs::write(&path, "# Hello\n\nbody\n").unwrap();
        let artifact = artifact::Artifact {
            id: "abc".into(),
            session: s,
            call_id: "c1".into(),
            name: "note.md".into(),
            title: "Hello".into(),
            kind: artifact::Kind::Markdown,
            mime: "text/markdown".into(),
            bytes: 14,
            version: 1,
            path: path.clone(),
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at: 0,
        };
        // Another session's artifact is not ours.
        m.apply(UiEvent::ArtifactShown(artifact::ArtifactShown {
            session: SessionId::new(),
            call_id: "c1".into(),
            artifact: artifact.clone(),
        }));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool { artifact: None, .. })
        ));
        m.apply(UiEvent::ArtifactShown(artifact::ArtifactShown {
            session: s,
            call_id: "c1".into(),
            artifact,
        }));
        m.apply_agent(AgentEvent::ToolCallFinished {
            session: s,
            turn,
            call_id: "c1".into(),
            output: "shown note.md (v1 · 3 lines · 14 B)".into(),
            is_error: false,
            files: Vec::new(),
        });
        let card = m.entries.last().unwrap();
        assert!(matches!(card, Entry::Tool { artifact: Some(a), .. } if a.name == "note.md"));
        assert_eq!(m.artifact_path(), Some(path.as_path()));
        let collapsed = card.height(80);
        m.toggle_card();
        let card = m.entries.last().unwrap();
        assert!(card.height(80) > collapsed);
        let Entry::Tool {
            expanded: Some(t), ..
        } = card
        else {
            panic!("expanded");
        };
        assert!(
            t.view()
                .lines()
                .any(|l| l.iter().any(|s| s.text.contains("Hello")))
        );
        m.toggle_card();
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool { expanded: None, .. })
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn app_card_reads_its_source_and_apps_picker_lists_only_apps() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let turn = TurnId::new();
        let mut m = Model {
            session: Some(s),
            app_enabled: true,
            ..Model::default()
        };
        // The streamed `name` names the app being built; no pretend render.
        m.apply_agent(AgentEvent::ToolCallPending {
            session: s,
            turn,
            call_id: "a1".into(),
            name: "app".into(),
        });
        m.apply_agent(AgentEvent::ToolCallProgress {
            session: s,
            turn,
            call_id: "a1".into(),
            args: r#"{"name": "panel", "source": "root = Card("#.into(),
        });
        let Some(Entry::Pending {
            name,
            progress: Some(p),
            ..
        }) = m.entries.last()
        else {
            unreachable!()
        };
        assert_eq!(progress_line(name, p), "building panel… 41 B");
        m.apply_agent(AgentEvent::ToolCallStarted {
            session: s,
            turn,
            call_id: "a1".into(),
            name: "app".into(),
            args: serde_json::json!({"name": "panel", "source": "root = Card(TextContent(\"hi\"))"}),
        });
        let path =
            std::env::temp_dir().join(format!("theuth-tui-app-{}.openui", std::process::id()));
        std::fs::write(&path, "root = Card(TextContent(\"hi\"))\n").unwrap();
        let app = artifact::Artifact {
            id: "app1".into(),
            session: s,
            call_id: "a1".into(),
            name: "panel.openui".into(),
            title: "Panel".into(),
            kind: artifact::Kind::App,
            mime: "text/plain".into(),
            bytes: 31,
            version: 1,
            path: path.clone(),
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at: 0,
        };
        // The app's `ArtifactShown` joins the `app` card by call_id like `show`.
        m.apply(UiEvent::ArtifactShown(artifact::ArtifactShown {
            session: s,
            call_id: "a1".into(),
            artifact: app.clone(),
        }));
        m.apply_agent(AgentEvent::ToolCallFinished {
            session: s,
            turn,
            call_id: "a1".into(),
            output: "published panel v1 (a1) · 1 statement".into(),
            is_error: false,
            files: Vec::new(),
        });
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool { artifact: Some(a), .. }) if a.kind == artifact::Kind::App
        ));
        // `o` opens the source file; Ctrl+O reads it, saying where the app runs.
        assert_eq!(m.artifact_path(), Some(path.as_path()));
        m.toggle_card();
        let Some(Entry::Tool {
            expanded: Some(t), ..
        }) = m.entries.last()
        else {
            panic!("expanded");
        };
        let has = |needle: &str| {
            t.view()
                .lines()
                .any(|l| l.iter().any(|s| s.text.contains(needle)))
        };
        assert!(has("desktop/web") && has("TextContent"));

        // A loaded transcript with an `app` card asks to re-join artifacts like `show`.
        m.apply(UiEvent::History(
            s,
            Loaded {
                history: vec![HistoryEntry::Tool {
                    call_id: "a1".into(),
                    name: "app".into(),
                    args: serde_json::json!({"name": "panel"}),
                    result: Some(("published".into(), false)),
                    files: vec![],
                    context: None,
                }],
                ..Loaded::default()
            },
        ));
        assert!(m.refresh_artifacts);
        m.session = Some(s);

        // With the plugin off, `/apps` is not offered and asks nothing; the
        // source stays readable through `/artifacts` (the card above).
        m.app_enabled = false;
        assert!(slash_matches("/apps", false).is_empty());
        m.submit("/apps", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t.starts_with("apps are off")));
        m.app_enabled = true;

        // `/apps` asks for the filtered list; the picker names it and `s` shows the source.
        m.submit("/apps", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::ListArtifacts { session, pick: true, apps: true }) if session == s
        ));
        m.apply(UiEvent::Artifacts {
            items: vec![app],
            pick: true,
            apps: true,
            sessions: Vec::new(),
        });
        assert!(matches!(m.menu, Some(Menu::Artifacts { apps: true, .. })));
        m.menu_key(KeyCode::Char('s'), "", &tx);
        assert!(m.menu.is_none());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Card { kind: CardKind::Artifact, title, expanded: true, .. }) if title == "panel.openui"
        ));
        // `/artifacts` stays every kind; an empty `/apps` says so.
        m.submit("/artifacts", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::ListArtifacts {
                pick: true,
                apps: false,
                ..
            })
        ));
        m.apply(UiEvent::Artifacts {
            items: Vec::new(),
            pick: true,
            apps: true,
            sessions: Vec::new(),
        });
        assert!(matches!(m.entries.last(), Some(Entry::Info(t)) if t == "no apps"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn scrape_progress_rides_the_running_card_and_clears_on_finish() {
        let s = SessionId::new();
        let turn = TurnId::new();
        let mut m = Model {
            session: Some(s),
            streaming: true,
            ..Model::default()
        };
        m.apply_agent(AgentEvent::ToolCallStarted {
            session: s,
            turn,
            call_id: "c1".into(),
            name: "crawl".into(),
            args: serde_json::json!({"url": "https://example.com"}),
        });
        let ev = |session| web::progress::ScrapeProgress {
            session,
            call_id: "c1".into(),
            inflight: vec!["https://example.com/docs/a".into()],
            done: vec![
                "https://example.com/".into(),
                "https://example.com/docs".into(),
            ],
            limit: 50,
        };
        // Another session's crawl is not ours.
        m.apply(UiEvent::ScrapeProgress(ev(SessionId::new())));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool { scrape: None, .. })
        ));
        m.apply(UiEvent::ScrapeProgress(ev(s)));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool { scrape: Some(row), .. }) if row == "crawling… 2/50 · example.com"
        ));
        assert_eq!(
            m.activity().as_deref(),
            Some("crawling… 2/50 · example.com")
        );
        assert_eq!(scrape_line("↳ map_site", &ev(s)), "mapping… 2/50");
        assert_eq!(scrape_line("scrape_url", &ev(s)), "reading example.com…");
        assert_eq!(
            scrape_line("search_web", &ev(s)),
            "reading 2/50 · example.com"
        );
        let idle = web::progress::ScrapeProgress {
            inflight: vec![],
            done: vec![],
            ..ev(s)
        };
        assert_eq!(scrape_line("search_web", &idle), "searching…");
        m.apply_agent(AgentEvent::ToolCallFinished {
            session: s,
            turn,
            call_id: "c1".into(),
            output: "# https://example.com/".into(),
            is_error: false,
            files: Vec::new(),
        });
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Tool {
                result: Some(_),
                scrape: None,
                ..
            })
        ));
    }

    /// A plan of `n` items: the first done, the second in progress, one
    /// blocked with a reason, the rest pending.
    fn plan(version: u64, n: usize) -> TodoSnapshot {
        use agent::todo::Todo;
        TodoSnapshot {
            version,
            source: TodoSource::Native,
            items: (0..n)
                .map(|i| Todo {
                    text: format!("task {i} with enough words to wrap on a narrow terminal"),
                    status: match i {
                        0 => TodoStatus::Done,
                        1 => TodoStatus::InProgress,
                        2 => TodoStatus::Blocked,
                        _ => TodoStatus::Pending,
                    },
                    reason: (i == 2).then(|| "waiting for the account owner".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn todos_follow_versions_survive_stale_loads_and_session_switches() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let (a, b) = (SessionId::new(), SessionId::new());
        m.session = Some(a);
        // Nothing loaded: no card. A load carries the plan.
        assert!(m.todo_card(80, 40).is_none());
        m.apply(UiEvent::History(
            a,
            Loaded {
                todos: Some(plan(2, 3)),
                ..Loaded::default()
            },
        ));
        let header = m.todo_card(80, 40).unwrap().header;
        assert!(
            header.starts_with("> Todos · 1/3 done · 1 blocked"),
            "{header}"
        );
        assert!(
            header.contains("Now: task 1") && header.ends_with("[F2]"),
            "{header}"
        );
        // Older and foreign snapshots are dropped; a newer one replaces.
        m.apply_agent(AgentEvent::TodoChanged {
            session: a,
            snapshot: plan(1, 9),
        });
        m.apply_agent(AgentEvent::TodoChanged {
            session: b,
            snapshot: plan(9, 9),
        });
        assert_eq!(m.todos.as_ref().unwrap().version, 2);
        m.apply_agent(AgentEvent::TodoChanged {
            session: a,
            snapshot: plan(3, 4),
        });
        assert_eq!(m.todos.as_ref().unwrap().items.len(), 4);
        // A refused settled reload without a snapshot marks the plan stale,
        // never empty; the same version back from `get_todos` clears it.
        m.apply(UiEvent::Refreshed(
            a,
            Loaded {
                settled: false,
                ..Loaded::default()
            },
        ));
        assert!(m.todos_stale && m.todos.as_ref().unwrap().items.len() == 4);
        assert!(m.todo_card(80, 40).unwrap().header.contains("stale"));
        m.apply(UiEvent::Todos(a, Some(plan(3, 4))));
        assert!(!m.todos_stale);
        // A failed `get_todos` marks the plan stale; it never looks current.
        m.apply(UiEvent::Todos(a, None));
        assert!(m.todos_stale && m.todos.as_ref().unwrap().items.len() == 4);
        m.apply(UiEvent::Todos(a, Some(plan(3, 4))));
        assert!(!m.todos_stale);
        // An explicit clear hides the card even while it is open and focused,
        // and the editor gets the keys back. A later F2 opens it again.
        m.toggle_todos(&tx);
        rx.try_recv().unwrap();
        assert!(m.todo_expanded() && m.todo_focused());
        m.apply_agent(AgentEvent::TodoChanged {
            session: a,
            snapshot: plan(4, 0),
        });
        assert!(m.todo_card(80, 40).is_none() && !m.todo_focus && m.input_focused());
        m.toggle_todos(&tx);
        assert!(matches!(rx.try_recv(), Ok(Command::GetTodos(s)) if s == a));
        assert!(
            m.todo_card(80, 40)
                .unwrap()
                .header
                .starts_with("v Todos · none")
        );
        // The same empty version back from that read is not a second clear.
        m.apply(UiEvent::Todos(a, Some(plan(4, 0))));
        assert!(m.todo_expanded() && m.todo_focused());
        // The choice is per session: B opens collapsed, A is still open on return.
        m.resume(b, &tx);
        assert!(m.todos.is_none() && !m.todo_expanded() && !m.todo_focus);
        m.resume(a, &tx);
        assert!(m.todo_expanded());
    }

    #[test]
    fn todo_card_scrolls_every_row_and_yields_to_modals() {
        use tools::{AskId, AskRequested, Question};
        let (tx, _rx) = unbounded_channel();
        let mut m = Model::default();
        let s = SessionId::new();
        m.session = Some(s);
        m.set_todos(plan(1, 8));
        // F2: open and focused; the editor loses the keys, the draft is not touched.
        m.toggle_todos(&tx);
        assert!(m.todo_focused() && !m.input_focused());
        let (w, h) = (40, 40);
        let total = m.todo_lines(w).len();
        let card = m.todo_card(w, h).unwrap();
        assert!(total > TODO_ROWS, "narrow terminal wraps: {total} rows");
        assert_eq!(card.rows.len(), TODO_ROWS);
        assert_eq!(card.height(), 3 + TODO_ROWS + 1);
        assert!(card.overflow.as_deref().unwrap().starts_with("↕ 1–6"));
        // Down one, then PageDown to the end: the last row is reachable and
        // the window never runs past it.
        m.todo_key(KeyCode::Down, w, h);
        assert_eq!(m.todo_scroll, 1);
        for _ in 0..10 {
            m.todo_key(KeyCode::PageDown, w, h);
        }
        assert_eq!(m.todo_scroll, total - TODO_ROWS);
        let last = m.todo_card(w, h).unwrap().rows.last().unwrap().1.clone();
        assert_eq!(last, m.todo_lines(w).last().unwrap().1);
        m.todo_key(KeyCode::PageUp, w, h);
        m.todo_key(KeyCode::Up, w, h);
        assert_eq!(m.todo_scroll, total - 2 * TODO_ROWS - 1);
        // A short terminal keeps the trigger and shrinks the body.
        assert_eq!(m.todo_card(w, 17).unwrap().rows.len(), 3);
        assert_eq!(m.todo_card(w, 10).unwrap().height(), 4);
        // Widening the terminal leaves fewer rows than the scroll position:
        // the card clamps to the end, and ↑ steps from that drawn end.
        for _ in 0..10 {
            m.todo_key(KeyCode::PageDown, w, h);
        }
        let wide = m.todo_lines(200).len();
        assert!(wide < m.todo_scroll, "{wide} rows at 200 columns");
        let card = m.todo_card(200, h).unwrap();
        assert_eq!(
            card.rows.last().unwrap().1,
            m.todo_lines(200).last().unwrap().1
        );
        m.todo_key(KeyCode::Up, 200, h);
        assert_eq!(m.todo_scroll, wide - TODO_ROWS - 1);
        // ⏎ is swallowed (the draft is not submitted); a typed char is not:
        // it hands the editor back so it lands in the draft.
        assert!(m.todo_key(KeyCode::Enter, w, h) && m.todo_focused());
        assert!(!m.todo_key(KeyCode::Char('x'), w, h));
        assert!(!m.todo_focus && m.todo_expanded() && m.input_focused());
        // Tab: editor back, card stays open. F2 again closes it.
        m.todo_focus = true;
        m.todo_key(KeyCode::Tab, w, h);
        assert!(!m.todo_focus && m.todo_expanded() && m.input_focused());
        m.toggle_todos(&tx);
        assert!(!m.todo_expanded() && m.todo_card(w, h).unwrap().rows.is_empty());
        // Esc while focused collapses and frees the editor.
        m.toggle_todos(&tx);
        m.todo_key(KeyCode::Esc, w, h);
        assert!(!m.todo_expanded() && !m.todo_focus);
        // A modal takes precedence: the focused card yields its keys.
        m.toggle_todos(&tx);
        m.apply(UiEvent::Ask(AskRequested {
            id: AskId(Default::default()),
            session: s,
            questions: vec![Question {
                id: "q".into(),
                question: "q?".into(),
                options: Vec::new(),
                recommended: None,
            }],
        }));
        assert!(!m.todo_focused());
        m.asks.clear();
        assert!(m.todo_focused());
        m.menu = Some(Menu::Slash { cursor: 0 });
        assert!(m.todo_focused(), "the slash popup is not modal");
    }

    #[test]
    fn todo_summary_names_drops_and_blocked_waits() {
        use agent::todo::Todo;
        let mut m = Model {
            session: Some(SessionId::new()),
            ..Model::default()
        };
        let item = |status, reason: Option<&str>| Todo {
            text: "t".into(),
            status,
            reason: reason.map(str::to_owned),
        };
        m.set_todos(TodoSnapshot {
            version: 1,
            source: TodoSource::Acp,
            items: vec![
                item(TodoStatus::Done, None),
                item(TodoStatus::Done, None),
                item(TodoStatus::Dropped, None),
            ],
        });
        assert_eq!(
            m.todo_card(0, 40).unwrap().header,
            "> Todos (external plan) · 2 done, 1 dropped  [F2]"
        );
        m.set_todos(TodoSnapshot {
            version: 2,
            source: TodoSource::Native,
            items: vec![
                item(TodoStatus::Done, None),
                item(TodoStatus::Blocked, Some("owner")),
            ],
        });
        assert_eq!(
            m.todo_card(0, 40).unwrap().header,
            "> Todos · 1/2 done · 1 blocked   Waiting: 1 blocked  [F2]"
        );
        let lines = m.todo_lines(0);
        assert_eq!(lines[1].1, "[blocked]     t");
        assert_eq!(lines[2], (None, format!("{}owner", " ".repeat(14))));
    }

    /// 50 §2.1: global-off is a ceiling the status names; a session reply
    /// for another session is dropped; `/peers` `a` answers the exact id
    /// the durable page marked pending, on the side the page says.
    #[test]
    fn communication_precedence_and_exact_answer() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let state = |session, global, opted_out, effective| CommunicationState {
            global,
            source: "config".into(),
            session,
            opted_out,
            inherited_from: opted_out.then_some(s),
            effective,
            scope: PeerScope::Isolated,
            alias: None,
            network_enabled: false,
            listening: false,
            revision: 1,
        };
        let off = communication_lines(&state(Some(s), false, false, false));
        assert!(off[0].contains("disabled globally") && off[0].contains("on always"));
        let opted = communication_lines(&state(Some(s), true, true, false));
        assert!(opted[0].contains("opted out") && opted[0].contains("restart"));
        assert!(
            communication_lines(&state(None, false, false, false))[0]
                .starts_with("communication: off (global default")
        );

        let mut m = Model::new("m".into(), true);
        m.session = Some(s);
        m.submit("/communication on", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetCommunication { session: Some(x), enabled: true, view: Some(v) }) if x == s && v == s
        ));
        // A reply about a different session never lands here.
        m.apply(UiEvent::Communication(state(
            Some(SessionId::new()),
            true,
            false,
            true,
        )));
        assert!(m.communication.is_none() && m.entries.is_empty());
        m.apply(UiEvent::Communication(state(Some(s), false, false, false)));
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        assert!(m.header(&catalog).0.contains("communication off"));
        assert!(
            matches!(m.entries.first(), Some(Entry::Info(l)) if l.contains("disabled globally"))
        );

        m.submit("/peers", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListPeers(x)) if x == s));
        assert!(matches!(rx.try_recv(), Ok(Command::ListPeerRequests(x)) if x == s));
        m.apply(UiEvent::Peers(s, Err("communication disabled".into())));
        let id = Uuid::new_v4();
        let record = PeerRecord {
            receipt: PeerReceipt {
                id,
                to: format!("session:{s}"),
                state: agent::PeerMessageState::Consumed,
                reply_to: None,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            },
            from: "session:00000000-0000-0000-0000-000000000001".into(),
            session: s,
            text: "which port?".into(),
            created_at: chrono::Utc::now(),
            request_scope: None,
        };
        m.apply(UiEvent::PeerRequests(
            s,
            PeerMessagePage {
                messages: vec![record],
                awaiting_answer: vec![id],
                awaiting_reply: Vec::new(),
                cursor: None,
            },
        ));
        let rows = m.peers_rows();
        assert!(matches!(&rows[0], PeerRow::Note(n) if n.contains("communication disabled")));
        assert!(matches!(&rows[2], PeerRow::Request(r, false) if r.receipt.id == id));
        // Down skips the separator onto the request; `a` prompts for its answer.
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Char('a'), "", &tx);
        assert!(
            matches!(&m.prompt, Some(Prompt::PeerAnswer { reply_to, resolve: false, .. }) if *reply_to == id)
        );
        m.submit("8080", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::AnswerPeerRequest { session, reply_to, text }) if session == s && reply_to == id && text == "8080"
        ));
        // A pending change for this session refreshes an open picker only.
        assert!(!m.refresh_peers);
        m.menu = Some(Menu::Peers {
            session: s,
            peers: None,
            requests: None,
            cursor: 0,
        });
        m.apply(UiEvent::PeerMessageChanged(PeerMessageChanged {
            session: s,
            id,
            state: agent::PeerMessageState::Replied,
        }));
        assert!(m.refresh_peers);
    }

    fn summary(id: SessionId, parent: Option<SessionId>, project: Option<&str>) -> SessionSummary {
        SessionSummary {
            id,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            title: String::new(),
            tags: Vec::new(),
            model: "m".into(),
            parent,
            name: parent.map(|_| "child".to_owned()),
            effort: None,
            agent: None,
            cwd: None,
            harness_model: None,
            context_size: None,
            desk: None,
            pinned: false,
            archived: false,
            project: project.map(str::to_owned),
            retention: Retention::Durable,
            running: false,
            owner: String::new(),
            epoch: 1,
            here: true,
            kind: Default::default(),
            memory: Default::default(),
        }
    }

    /// 51 §3: headings in first-seen order of the roots, Ungrouped last,
    /// children under their roots; the cursor never rests on a heading.
    #[test]
    fn sessions_group_by_project_with_children_under_roots() {
        let ids: Vec<SessionId> = (0..5).map(|_| SessionId::new()).collect();
        let list = vec![
            summary(ids[0], None, Some("B")),
            summary(ids[1], None, Some("A")),
            summary(ids[2], None, None),
            summary(ids[3], None, Some("B")),
            // A child listed first still lands under its root.
            summary(ids[4], Some(ids[1]), None),
        ];
        let rows = session_rows(list);
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Grouped::Heading(h) => format!("# {h}"),
                Grouped::Item((s, d)) => {
                    format!("{d}:{}", ids.iter().position(|i| *i == s.id).unwrap())
                }
            })
            .collect();
        assert_eq!(
            shape,
            [
                "# B",
                "0:0",
                "0:3",
                "# A",
                "0:1",
                "1:4",
                "# Ungrouped",
                "0:2"
            ]
        );
        assert_eq!(grouped_first(&rows), 1);
        assert_eq!(grouped_step(&rows, 2, 1), 4, "down skips the A heading");
        assert_eq!(grouped_step(&rows, 4, -1), 2, "up skips it too");
        assert_eq!(grouped_step(&rows, 1, -1), 1, "the top clamps");
        assert_eq!(grouped_step(&rows, 7, 1), 7, "the bottom clamps");
    }

    /// 51 §6.2 display strings over one bucket.
    #[test]
    fn cost_lines_follow_the_display_rules() {
        let usd = |nanos| Money {
            currency: "USD".into(),
            nanos,
        };
        let mut b = CostBucket::default();
        assert_eq!(cost_lines(&b), ["Returned cost —"]);
        b.calls = 3;
        b.dispatched = 3;
        b.returned = vec![usd(123_000_000)];
        assert_eq!(cost_lines(&b), ["Returned cost $0.123"]);
        b.returned = vec![usd(100_000_000)];
        b.unavailable = 2;
        b.estimated_for_unavailable = vec![usd(23_400_000)];
        assert_eq!(
            cost_lines(&b),
            ["Returned cost $0.100 · 2 unavailable", "Estimated ~$0.023"]
        );
        // Header tag and child rows: returned wins, then the estimate keeps its tilde, then a dash.
        assert_eq!(cost_short(&b), "$0.100");
        b.returned.clear();
        assert_eq!(cost_short(&b), "~$0.023");
        assert_eq!(
            cost_lines(&b),
            ["Returned cost — · 2 unavailable", "Estimated ~$0.023"]
        );
        b.estimated_for_unavailable.clear();
        assert_eq!(cost_short(&b), "—");
        b.unavailable = 0;
        b.returned = vec![
            Money {
                currency: "EUR".into(),
                nanos: 1_999_500_000,
            },
            usd(5_000_000),
        ];
        assert_eq!(cost_lines(&b), ["Returned cost €2.000 · $0.005"]);
        // Nanos never touch a float: the u64 ceiling formats exactly.
        assert_eq!(money_units(u64::MAX), "18446744073.710");
    }

    fn page(session: SessionId, job: &str, revision: u64, viewport: PtyViewport) -> PtyPage {
        PtyPage {
            session,
            id: JobId(job.into()),
            revision,
            epoch: None,
            viewport,
            origin: 0,
            retained: 100,
            rows: 40,
            cols: 120,
            alternate: false,
            cursor: None,
            wrapped: Vec::new(),
            lines: Vec::new(),
            truncated: false,
            redaction_complete: true,
            exit: None,
        }
    }

    /// 51 §7.4: a reply paints only when it answers the current selection,
    /// epoch and viewport and is not older than what is shown; an
    /// `Unchanged` for another viewport re-asks.
    #[test]
    fn terminal_replies_are_guarded_by_epoch_viewport_and_revision() {
        let s = SessionId::new();
        let t = TerminalSummary {
            session: s,
            id: JobId("j1".into()),
            name: None,
            command: "top".into(),
            state: "running".into(),
            rows: 40,
            cols: 120,
            revision: 3,
            epoch: None,
        };
        let mut v = TerminalView::new(&t, 7);
        assert!(matches!(
            v.poll(1000),
            Some(Command::PtyScreen {
                epoch: 7,
                if_revision: None,
                ..
            })
        ));
        assert!(v.poll(1000).is_none(), "one ask in flight");
        // A reply asked under the previous selection is dropped, and does not free the slot.
        let live = PtyViewport::Live;
        assert!(!v.reply(
            s,
            &JobId("j1".into()),
            live,
            6,
            Ok(PtyScreenReply::Updated(page(s, "j1", 9, live)))
        ));
        assert!(v.page.is_none() && v.inflight);
        // The current one paints.
        assert!(v.reply(
            s,
            &JobId("j1".into()),
            live,
            7,
            Ok(PtyScreenReply::Updated(page(s, "j1", 4, live)))
        ));
        assert_eq!(v.revision, Some(4));
        assert!(!v.inflight);
        // Scrolling into history: the next ask carries no `if_revision` and goes out at once.
        v.scroll(-10);
        let hist = PtyViewport::History {
            offset: 10,
            height: TERMINAL_PAGE,
        };
        assert_eq!(v.viewport, hist);
        assert!(
            matches!(v.poll(1100), Some(Command::PtyScreen { viewport, if_revision: None, .. }) if viewport == hist)
        );
        // `Unchanged` echoing the old viewport is not an answer: re-ask.
        assert!(!v.reply(
            s,
            &JobId("j1".into()),
            hist,
            7,
            Ok(PtyScreenReply::Unchanged {
                viewport: live,
                revision: 4
            })
        ));
        assert!(v.dirty && v.revision.is_none());
        assert_eq!(v.wake(), Some(0));
        assert!(v.poll(1200).is_some());
        // A higher revision from the job selected before does not paint the new selection.
        let mut next = TerminalView::new(
            &TerminalSummary {
                id: JobId("j2".into()),
                ..t.clone()
            },
            8,
        );
        next.poll(2000);
        assert!(!next.reply(
            s,
            &JobId("j1".into()),
            live,
            8,
            Ok(PtyScreenReply::Updated(page(s, "j1", 99, live)))
        ));
        assert!(next.page.is_none());
        assert!(next.reply(
            s,
            &JobId("j2".into()),
            live,
            8,
            Ok(PtyScreenReply::Updated(page(s, "j2", 1, live)))
        ));
        // An older page than the one shown is dropped; gone is gone.
        assert!(!next.reply(
            s,
            &JobId("j2".into()),
            live,
            8,
            Ok(PtyScreenReply::Updated(page(s, "j2", 0, live)))
        ));
        assert!(next.reply(
            s,
            &JobId("j2".into()),
            live,
            8,
            Ok(PtyScreenReply::Unavailable)
        ));
        assert_eq!(next.status().as_deref(), Some("No longer available."));
        assert!(next.wake().is_none(), "no more polls for a gone terminal");
    }

    /// 55 §3: `i` hands keys to the process (Ctrl-C and Esc included), the
    /// poll tightens, Ctrl-] hands them back, and an exited job cannot be typed into.
    #[test]
    fn terminal_interactive_forwards_keys_and_detaches() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let s = SessionId::new();
        let t = TerminalSummary {
            session: s,
            id: JobId("j1".into()),
            name: None,
            command: "python3".into(),
            state: "running".into(),
            rows: 40,
            cols: 120,
            revision: 1,
            epoch: None,
        };
        let live = PtyViewport::Live;
        let mut v = TerminalView::new(&t, 1);
        v.poll(0);
        v.reply(
            s,
            &JobId("j1".into()),
            live,
            1,
            Ok(PtyScreenReply::Updated(page(s, "j1", 1, live))),
        );
        m.menu = Some(Menu::Terminal(Box::new(v)));
        assert!(!m.terminal_interactive());
        // Esc in view mode closes; `i` first.
        m.menu_key(KeyCode::Char('i'), "", &tx);
        assert!(m.terminal_interactive());
        while rx.try_recv().is_ok() {}
        m.terminal_type(KeyCode::Char('c'), KeyModifiers::CONTROL, &tx);
        m.terminal_type(KeyCode::Esc, KeyModifiers::NONE, &tx);
        m.terminal_type(KeyCode::Char('x'), KeyModifiers::NONE, &tx);
        m.terminal_type(KeyCode::Char('w'), KeyModifiers::SHIFT, &tx);
        let mut sent = Vec::new();
        while let Ok(c) = rx.try_recv() {
            if let Command::PtyInput {
                text, keys, job, ..
            } = c
            {
                assert_eq!(job, JobId("j1".into()));
                sent.push((text, keys));
            }
        }
        assert_eq!(
            sent,
            vec![
                (None, vec![Key::Ctrl('c')]),
                (None, vec![Key::Escape]),
                (Some("x".into()), vec![]),
                (Some("w".into()), vec![]),
            ]
        );
        let Some(Menu::Terminal(v)) = &mut m.menu else {
            panic!()
        };
        assert!(v.interactive && v.inflight);
        v.inflight = false;
        v.poll(5000);
        assert_eq!(
            v.next_poll,
            5000 + TERMINAL_INTERACTIVE_POLL_MS,
            "tight poll while typing"
        );
        // Ctrl-] detaches and sends nothing.
        m.terminal_type(KeyCode::Char(']'), KeyModifiers::CONTROL, &tx);
        assert!(!m.terminal_interactive());
        assert!(rx.try_recv().is_err());
        // The job exits: `i` no longer enters, and an open typing mode drops.
        let Some(Menu::Terminal(v)) = &mut m.menu else {
            panic!()
        };
        v.interactive = true;
        v.inflight = true;
        let mut exited = page(s, "j1", 2, live);
        exited.exit = Some(tools::JobExit::Exited(0));
        v.reply(
            s,
            &JobId("j1".into()),
            live,
            1,
            Ok(PtyScreenReply::Updated(exited)),
        );
        assert!(!v.interactive);
        m.menu_key(KeyCode::Char('i'), "", &tx);
        assert!(!m.terminal_interactive());
        // Esc in view mode closes the overlay.
        m.menu_key(KeyCode::Esc, "", &tx);
        assert!(m.menu.is_none());
    }

    /// 51 §4.3: leaving the owned tree for another root releases it once;
    /// moving within the tree does not.
    #[test]
    fn private_tree_is_left_only_when_navigating_outside_it() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/new private", &tx);
        let Ok(Command::CreateSession(opts)) = rx.try_recv() else {
            panic!("expected create_session");
        };
        assert_eq!(opts.retention, Retention::Private);
        assert!(opts.view.as_deref().is_some_and(|v| v.starts_with("tui-")));
        let root = SessionId::new();
        m.apply(UiEvent::SessionOpened(SessionOpen {
            session: root,
            root,
            retention: Retention::Private,
            lease_secret: Some("s3cret".into()),
        }));
        assert_eq!(m.session, Some(root));
        assert!(
            m.entries
                .iter()
                .any(|e| matches!(e, Entry::Info(t) if t == PRIVATE_WARNING))
        );
        let child = SessionId::new();
        m.apply(UiEvent::Agent(AgentEvent::SubagentSpawned {
            parent: root,
            child,
            name: "worker".into(),
            cap: None,
        }));
        while rx.try_recv().is_ok() {}
        // Root → child → parent: no leave.
        m.resume(child, &tx);
        m.resume(root, &tx);
        assert!(m.private.is_some());
        while let Ok(c) = rx.try_recv() {
            assert!(!matches!(c, Command::LeaveSession { .. }));
        }
        // Another root: leave first, exactly once, with the secret.
        m.resume(SessionId::new(), &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::LeaveSession { session, secret }) if session == root && secret == "s3cret"
        ));
        assert!(m.private.is_none());
        m.apply(UiEvent::SessionLeft(
            root,
            CloseResult {
                revoked: true,
                cleanup: CleanupStatus::Pending,
            },
        ));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Info(t)) if t == "private session closed (cleanup pending)"
        ));
        // The warning shows once per run (the open resets the transcript).
        m.submit("/new private", &tx);
        m.apply(UiEvent::SessionOpened(SessionOpen {
            session: root,
            root,
            retention: Retention::Private,
            lease_secret: Some("next".into()),
        }));
        assert!(m.private.is_some());
        assert!(
            !m.entries
                .iter()
                .any(|e| matches!(e, Entry::Info(t) if t == PRIVATE_WARNING))
        );
    }

    /// The 124 §2.3 fixture catalog, in catalog order.
    fn model_catalog() -> Vec<ModelInfo> {
        [
            (
                "amazon-bedrock",
                "amazon-bedrock/amazon.nova-pro-v1:0",
                "Nova Pro",
            ),
            (
                "amazon-bedrock",
                "amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0",
                "Claude Sonnet 4.5",
            ),
            ("anthropic", "anthropic/claude-opus-4-1", "Claude Opus 4.1"),
            (
                "anthropic",
                "anthropic/claude-sonnet-4-5",
                "Claude Sonnet 4.5",
            ),
            ("cursor", "cursor/gpt-5.4", "GPT-5.4"),
            (
                "google",
                "google/gemini-2.5-flash-thinking",
                "Gemini 2.5 Flash Thinking",
            ),
            ("google", "google/gemini-2.5-pro", "Gemini 2.5 Pro"),
            ("openai", "openai/gpt-5.4", "GPT-5.4"),
            ("openai", "openai/gpt-5.4-mini", "GPT-5.4 mini"),
            ("openai", "openai/o3-pro", "o3-pro"),
        ]
        .into_iter()
        .map(|(provider, id, name)| ModelInfo {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context: 200_000,
            reasoning: false,
            image: false,
            efforts: vec![],
            budget_min: None,
            budget_max: None,
        })
        .collect()
    }

    fn ids(rows: &[&ModelInfo]) -> Vec<String> {
        rows.iter().map(|m| m.id.clone()).collect()
    }

    /// Design 124 §2.3: every row of the vector table against the fixture catalog.
    #[test]
    fn rank_models_matches_the_design_vectors() {
        let items = model_catalog();
        const NOVA: &str = "amazon-bedrock/amazon.nova-pro-v1:0";
        const BEDROCK_SONNET: &str = "amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0";
        const OPUS: &str = "anthropic/claude-opus-4-1";
        const SONNET: &str = "anthropic/claude-sonnet-4-5";
        const CURSOR_GPT: &str = "cursor/gpt-5.4";
        const GEMINI_PRO: &str = "google/gemini-2.5-pro";
        const GPT: &str = "openai/gpt-5.4";
        const GPT_MINI: &str = "openai/gpt-5.4-mini";
        const O3: &str = "openai/o3-pro";
        let vectors: [(&str, &[&str]); 13] = [
            ("gpt", &[CURSOR_GPT, GPT, GPT_MINI]),
            ("pro", &[NOVA, GEMINI_PRO, O3]),
            ("anthropic sonnet", &[SONNET, BEDROCK_SONNET]),
            ("sonnet 4.5", &[BEDROCK_SONNET, SONNET]),
            ("claude", &[BEDROCK_SONNET, OPUS, SONNET]),
            ("mini", &[GPT_MINI]),
            ("aude", &[BEDROCK_SONNET, OPUS, SONNET]),
            ("GPT-5.4", &[CURSOR_GPT, GPT, GPT_MINI]),
            ("anth", &[OPUS, SONNET, BEDROCK_SONNET]),
            ("openai mini", &[GPT_MINI]),
            ("nova pro", &[NOVA]),
            ("v1:0", &[NOVA, BEDROCK_SONNET]),
            ("xyz", &[]),
        ];
        for (query, want) in vectors {
            assert_eq!(ids(&rank_models(&items, query)), want, "query {query:?}");
        }
        // Browse mode: no terms, no scoring, the catalog as is.
        assert!(search_terms("  ").is_empty());
        assert_eq!(
            ids(&rank_models(&items, "  ")),
            ids(&items.iter().collect::<Vec<_>>())
        );
    }

    #[test]
    fn providers_count_a_to_z_and_provider_models_sort_by_name_then_id() {
        let mut items = model_catalog();
        items.push(ModelInfo {
            id: "openai/aaa".into(),
            name: "gpt-5.4".into(),
            ..items[7].clone()
        });
        assert_eq!(
            providers(&items),
            vec![
                ("amazon-bedrock".to_owned(), 2),
                ("anthropic".to_owned(), 2),
                ("cursor".to_owned(), 1),
                ("google".to_owned(), 2),
                ("openai".to_owned(), 4),
            ]
        );
        // Case-insensitive name order; equal names fall back to id.
        assert_eq!(
            ids(&provider_models(&items, "openai")),
            [
                "openai/aaa",
                "openai/gpt-5.4",
                "openai/gpt-5.4-mini",
                "openai/o3-pro"
            ]
        );
    }

    fn model_row_ids(m: &Model) -> Vec<String> {
        m.models_rows()
            .iter()
            .map(|r| match r {
                ModelRow::Off { current } => format!("{}off", if *current { "●" } else { "" }),
                ModelRow::Current(i) => format!("●{}", i.id),
                ModelRow::Provider { id, count, current } => {
                    format!("{}{id}:{count}", if *current { "●" } else { "" })
                }
                ModelRow::Favorite(i) => format!("★{}", i.id),
                ModelRow::Model(i) => i.id.clone(),
            })
            .collect()
    }

    fn models_cursor(m: &Model) -> usize {
        match &m.menu {
            Some(Menu::Models { cursor, .. }) => *cursor,
            other => panic!("no models menu: {other:?}"),
        }
    }

    #[test]
    fn models_picker_opens_on_providers_and_navigates_levels() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model {
            model_id: "openai/gpt-5.4".into(),
            ..Model::default()
        };
        m.apply(UiEvent::Models(model_catalog()));
        // Providers level: the current model first, its provider marked, cursor on the first provider.
        assert_eq!(
            model_row_ids(&m),
            [
                "●openai/gpt-5.4",
                "amazon-bedrock:2",
                "anthropic:2",
                "cursor:1",
                "google:2",
                "●openai:3"
            ]
        );
        assert_eq!(models_cursor(&m), 1);
        // Up stops at the Current row; Down clamps at the last provider.
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Up, "", &tx);
        assert_eq!(models_cursor(&m), 0);
        for _ in 0..9 {
            m.menu_key(KeyCode::Down, "", &tx);
        }
        assert_eq!(models_cursor(&m), 5);
        // Enter on a provider descends: its models by name, the current one still a Model row.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert_eq!(
            model_row_ids(&m),
            ["openai/gpt-5.4", "openai/gpt-5.4-mini", "openai/o3-pro"]
        );
        assert_eq!(models_cursor(&m), 0);
        assert!(rx.try_recv().is_err());
        // Left ascends to the providers level with the cursor home again.
        m.menu_key(KeyCode::Left, "", &tx);
        assert_eq!(model_row_ids(&m)[1], "amazon-bedrock:2");
        assert_eq!(models_cursor(&m), 1);
        // Left on the providers level is a no-op; Right descends like Enter.
        m.menu_key(KeyCode::Left, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Right, "", &tx);
        assert_eq!(
            model_row_ids(&m),
            ["anthropic/claude-opus-4-1", "anthropic/claude-sonnet-4-5"]
        );
        // Backspace on an empty filter ascends.
        m.menu_key(KeyCode::Backspace, "", &tx);
        assert_eq!(model_row_ids(&m)[0], "●openai/gpt-5.4");
        // Right on the Current row does nothing; Enter on a model row sets it and closes.
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Right, "", &tx);
        assert_eq!(models_cursor(&m), 0);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModel { session: None, model }) if model == "anthropic/claude-sonnet-4-5"
        ));
        assert!(m.menu.is_none());
    }

    #[test]
    fn models_picker_search_covers_all_models_and_returns_to_the_level() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model {
            model_id: "openai/o3-pro".into(),
            ..Model::default()
        };
        m.apply(UiEvent::Models(model_catalog()));
        // Descend into google, then type: search runs over every model, the
        // Current row stays on top even though it does not match, and the
        // cursor lands on the best match.
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert_eq!(
            model_row_ids(&m),
            ["google/gemini-2.5-flash-thinking", "google/gemini-2.5-pro"]
        );
        for c in "gpt".chars() {
            m.menu_key(KeyCode::Char(c), "", &tx);
        }
        assert_eq!(
            model_row_ids(&m),
            [
                "●openai/o3-pro",
                "cursor/gpt-5.4",
                "openai/gpt-5.4",
                "openai/gpt-5.4-mini"
            ]
        );
        assert_eq!(models_cursor(&m), 1);
        // A query matching only the current model lists it once, as the Current
        // row, and the Current row stays when nothing matches at all.
        for _ in 0..3 {
            m.menu_key(KeyCode::Backspace, "", &tx);
        }
        for c in "o3".chars() {
            m.menu_key(KeyCode::Char(c), "", &tx);
        }
        assert_eq!(model_row_ids(&m), ["●openai/o3-pro"]);
        assert_eq!(models_cursor(&m), 0);
        m.menu_key(KeyCode::Char('x'), "", &tx);
        assert_eq!(model_row_ids(&m), ["●openai/o3-pro"]);
        // Clearing the filter returns to the remembered provider level.
        for _ in 0..3 {
            m.menu_key(KeyCode::Backspace, "", &tx);
        }
        assert_eq!(
            model_row_ids(&m),
            ["google/gemini-2.5-flash-thinking", "google/gemini-2.5-pro"]
        );
        assert_eq!(models_cursor(&m), 0);
        // Enter in search mode picks the best match.
        for c in "GPT-5.4".chars() {
            m.menu_key(KeyCode::Char(c), "", &tx);
        }
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModel { session: None, model }) if model == "cursor/gpt-5.4"
        ));
        assert!(m.menu.is_none());
    }

    #[test]
    fn single_provider_catalog_opens_on_its_models_level_and_stays_there() {
        let (tx, _rx) = unbounded_channel();
        let mut m = Model {
            model_id: "openai/gpt-5.4".into(),
            ..Model::default()
        };
        let items: Vec<ModelInfo> = model_catalog()
            .into_iter()
            .filter(|i| i.provider == "openai")
            .collect();
        m.apply(UiEvent::Models(items));
        let level = ["openai/gpt-5.4", "openai/gpt-5.4-mini", "openai/o3-pro"];
        assert_eq!(model_row_ids(&m), level);
        assert_eq!(models_cursor(&m), 0);
        m.menu_key(KeyCode::Left, "", &tx);
        m.menu_key(KeyCode::Backspace, "", &tx);
        assert_eq!(model_row_ids(&m), level);
        assert!(matches!(
            &m.menu,
            Some(Menu::Models { provider: Some(p), .. }) if p == "openai"
        ));
        // Search still works and clearing it comes back to the models level.
        // `o`: o3-pro by name (90) beats the provider-prefix hits (65); the current model shows once.
        m.menu_key(KeyCode::Char('o'), "", &tx);
        assert_eq!(
            model_row_ids(&m),
            ["●openai/gpt-5.4", "openai/o3-pro", "openai/gpt-5.4-mini"]
        );
        assert_eq!(models_cursor(&m), 1);
        m.menu_key(KeyCode::Backspace, "", &tx);
        assert_eq!(model_row_ids(&m), level);
    }

    #[test]
    fn double_cmd_parses_the_table() {
        use double::{Tier, Venue};
        assert_eq!(DoubleCmd::parse(""), Some(DoubleCmd::Status));
        assert_eq!(DoubleCmd::parse("off"), Some(DoubleCmd::Off));
        assert_eq!(DoubleCmd::parse("pause"), Some(DoubleCmd::Pause(true)));
        assert_eq!(DoubleCmd::parse("pause off"), Some(DoubleCmd::Pause(false)));
        assert_eq!(
            DoubleCmd::parse("train"),
            Some(DoubleCmd::Train { tier: Tier::A, venue: Venue::Auto })
        );
        assert_eq!(
            DoubleCmd::parse("train b node:gpu"),
            Some(DoubleCmd::Train { tier: Tier::B, venue: Venue::Node("gpu".into()) })
        );
        assert_eq!(
            DoubleCmd::parse("cap approval:shell:mutating:git push:root 2 opt-in"),
            Some(DoubleCmd::Cap {
                class: "approval:shell:mutating:git push:root".into(),
                rung: 2,
                opt_in: true
            })
        );
        assert_eq!(DoubleCmd::parse("rollback v3"), Some(DoubleCmd::Rollback("v3".into())));
        for bad in ["train c", "train a off", "cap k 4", "cap k", "cap 2", "pause maybe", "rollback", "x"] {
            assert_eq!(DoubleCmd::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn double_chip_follows_the_shown_card_and_counts_down() {
        let mut m = Model::new("m".into(), true);
        let s = SessionId::new();
        m.session = Some(s);
        let id = ApprovalId(Uuid::new_v4());
        let pred = |will_act_at: Option<String>| double::DoublePredicted {
            question_id: id.0.to_string(),
            session: Some(s),
            kind: double::CardKind::Approval,
            class: "approval:shell:mutating:git push:root".into(),
            choice: "Allow".into(),
            index: None,
            p: 0.912,
            probs: vec![],
            rung: 1,
            will_act_at,
            version: "baseline".into(),
            explain: Default::default(),
        };
        // The prediction arrives before the card: kept, shown once the card is.
        m.apply(UiEvent::DoublePredicted(pred(None)));
        assert_eq!(m.double_chip(), None);
        assert!(!m.animating());
        m.apply(UiEvent::Approval(ApprovalRequested {
            id,
            session: s,
            tool: "shell".into(),
            args: serde_json::json!({"command": "git push"}),
            risk: tools::Risk::Mutating,
        }));
        assert_eq!(m.double_chip().as_deref(), Some("double: Allow 91 % · L1"));
        // L2: the countdown is derived from `will_act_at` and keeps frames coming.
        let p = pred(Some("2030-01-01T00:00:30Z".into()));
        let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:12Z").unwrap();
        assert_eq!(
            double_chip(&p, u64::try_from(t0.timestamp_millis()).unwrap()),
            "double: Allow 91 % · L1 · answers in 18 s (answer to take over)"
        );
        m.apply(UiEvent::DoublePredicted(p));
        assert!(m.animating());
        // Resolution drops it; a later answer line names the choice.
        m.apply(UiEvent::Resolved(ApprovalResolved { id, decision: Decision::Allow, timed_out: false }));
        assert_eq!(m.double_chip(), None);
        assert!(m.double.is_empty());
        m.apply(UiEvent::DoubleAnswered(double::DoubleAnswered {
            question_id: id.0.to_string(),
            session: Some(s),
            class: "approval:shell:mutating:git push:root".into(),
            choice: "Allow".into(),
            version: "baseline".into(),
            undo_until: None,
        }));
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Info(t)) if t == "double answered Allow (approval:shell:mutating:git push:root)"
        ));
    }
    /// 127 §3: favorites lead the providers level in starring order (only
    /// those in the catalog), the Current row shows only when the current
    /// model is not one of them, the cursor starts on the first favorite,
    /// and Tab stars/unstars the highlighted model without closing.
    #[test]
    fn models_picker_lists_favorites_first_and_tab_toggles_them() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model {
            model_id: "openai/gpt-5.4".into(),
            favorite_models: vec![
                "google/gemini-2.5-pro".into(),
                "gone/provider-unkeyed".into(),
                "anthropic/claude-opus-4-1".into(),
            ],
            ..Model::default()
        };
        m.apply(UiEvent::Models(model_catalog()));
        assert_eq!(
            model_row_ids(&m),
            [
                "★google/gemini-2.5-pro",
                "★anthropic/claude-opus-4-1",
                "●openai/gpt-5.4",
                "amazon-bedrock:2",
                "anthropic:2",
                "cursor:1",
                "google:2",
                "●openai:3"
            ]
        );
        assert_eq!(models_cursor(&m), 0);
        // Tab on a favorite asks to unstar it; the menu stays open.
        m.menu_key(KeyCode::Tab, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModelFavorite { model, favorite: false }) if model == "google/gemini-2.5-pro"
        ));
        assert!(m.menu.is_some());
        // Tab on the Current row asks to star it.
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Tab, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModelFavorite { model, favorite: true }) if model == "openai/gpt-5.4"
        ));
        // Tab on a provider row does nothing.
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Tab, "", &tx);
        assert!(rx.try_recv().is_err());
        // The engine answers: the current model is now a favorite and shows once, as a favorite.
        m.apply(UiEvent::Agent(AgentEvent::FavoriteModelsChanged {
            models: vec!["anthropic/claude-opus-4-1".into(), "openai/gpt-5.4".into()],
        }));
        assert_eq!(
            model_row_ids(&m),
            [
                "★anthropic/claude-opus-4-1",
                "★openai/gpt-5.4",
                "amazon-bedrock:2",
                "anthropic:2",
                "cursor:1",
                "google:2",
                "●openai:3"
            ]
        );
        // Enter on a favorite sets it like any model row.
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModel { session: None, model }) if model == "anthropic/claude-opus-4-1"
        ));
        assert!(m.menu.is_none());
        // The command reply applies the list too, and a shrinking list clamps an open cursor.
        m.apply(UiEvent::Models(model_catalog()));
        for _ in 0..9 {
            m.menu_key(KeyCode::Down, "", &tx);
        }
        assert_eq!(models_cursor(&m), 6);
        m.apply(UiEvent::FavoriteModels(vec![]));
        assert!(m.favorite_models.is_empty());
        assert_eq!(model_row_ids(&m)[0], "●openai/gpt-5.4");
        assert_eq!(models_cursor(&m), 5);
        // Tab on a provider's models level and in search results toggles the highlighted model.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert_eq!(model_row_ids(&m)[0], "openai/gpt-5.4");
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Tab, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModelFavorite { model, favorite: true }) if model == "openai/gpt-5.4-mini"
        ));
        m.menu_key(KeyCode::Char('n'), "", &tx);
        m.menu_key(KeyCode::Char('o'), "", &tx);
        m.menu_key(KeyCode::Char('v'), "", &tx);
        m.menu_key(KeyCode::Char('a'), "", &tx);
        m.menu_key(KeyCode::Tab, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetModelFavorite { model, favorite: true })
                if model == "amazon-bedrock/amazon.nova-pro-v1:0"
        ));
        assert!(m.menu.is_some());
    }

    fn note(severity: AdvisorSeverity, text: &str) -> agent::AdvisoryNote {
        agent::AdvisoryNote {
            severity,
            note: text.into(),
        }
    }

    fn advisory(advisor: SessionId, notes: Vec<agent::AdvisoryNote>) -> Advisory {
        Advisory {
            advisor,
            step: 14,
            turn: None,
            at_ms: 0,
            notes,
        }
    }

    fn advisor_status(session: SessionId, state: AdvisorState, model: &str) -> AdvisorStatus {
        AdvisorStatus {
            session,
            advisor: Some(SessionId::new()),
            state,
            model: Some(model.into()),
            effort: Some("high".into()),
            ..AdvisorStatus::default()
        }
    }

    fn info_lines(m: &Model) -> Vec<&str> {
        m.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Info(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 125 §7.2: bare `/advisor` opens the `/model` picker targeted at the
    /// advisor — `Off` leads (marked while there is none), the current advisor
    /// is the `●` row instead of the session's model, ⏎ on `Off` sends `None`.
    #[test]
    fn advisor_cmd_bare_opens_picker_with_off_row() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            model_id: "openai/gpt-5.4".into(),
            ..Model::default()
        };
        m.submit("/advisor", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListModels)));
        m.apply(UiEvent::Models(model_catalog()));
        assert_eq!(model_row_ids(&m)[..2], ["●off", "amazon-bedrock:2"]);
        assert_eq!(models_cursor(&m), 1, "home skips the Off row");
        assert!(!model_row_ids(&m).iter().any(|r| r == "●openai/gpt-5.4"));
        m.menu_key(KeyCode::Up, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetAdvisor { session, model: None, effort: None }) if session == s
        ));
        assert!(m.menu.is_none());
        // With an advisor set: `Off` unmarked, the advisor's model is the Current row,
        // ⏎ on a model goes to `set_advisor`, and `/model` has no Off row at all.
        m.advisor = advisor_status(s, AdvisorState::Idle, "anthropic/claude-opus-4-1");
        m.submit("/advisor", &tx);
        rx.try_recv().unwrap();
        m.apply(UiEvent::Models(model_catalog()));
        assert_eq!(
            model_row_ids(&m)[..4],
            [
                "off",
                "●anthropic/claude-opus-4-1",
                "amazon-bedrock:2",
                "●anthropic:2"
            ]
        );
        assert_eq!(models_cursor(&m), 2);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetAdvisor { session, model: Some(model), effort: None })
                if session == s && model == "anthropic/claude-sonnet-4-5"
        ));
        m.submit("/model", &tx);
        rx.try_recv().unwrap();
        m.apply(UiEvent::Models(model_catalog()));
        assert_eq!(model_row_ids(&m)[0], "●openai/gpt-5.4");
    }

    /// 125 §7.1: `/advisor off` is `set_advisor(session, None)`; a direct id is a pick;
    /// an ACP child refuses.
    #[test]
    fn advisor_cmd_off_sends_none() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        m.submit("/advisor off", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetAdvisor { session, model: None, effort: None }) if session == s
        ));
        m.submit("/advisor openai/o3-pro", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetAdvisor { model: Some(model), effort: None, .. }) if model == "openai/o3-pro"
        ));
        m.agent = Some("omp".into());
        m.submit("/advisor off", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Error(t)) if t == "/advisor: not available for an external harness session"
        ));
    }

    /// 125 §7.1: `/advisor on` re-uses `Info.advisor` (or the last status
    /// that named a model); with nothing remembered it says so.
    #[test]
    fn advisor_cmd_on_uses_remembered_model_or_errors() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        m.submit("/advisor on", &tx);
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Error(t)) if t == "/advisor on: no remembered advisor model — pick one with /advisor"
        ));
        m.advisor_default.model = Some("anthropic/claude-opus-4-1".into());
        m.advisor_default.effort = Some("high".into());
        m.submit("/advisor on", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetAdvisor { session, model: Some(model), effort: Some(effort) })
                if session == s && model == "anthropic/claude-opus-4-1" && effort == "high"
        ));
        // A status naming a model becomes the remembered pick; going off keeps it.
        m.apply(UiEvent::AdvisorStatus(advisor_status(
            s,
            AdvisorState::Idle,
            "openai/o3-pro",
        )));
        m.apply(UiEvent::AdvisorStatus(AdvisorStatus {
            session: s,
            ..AdvisorStatus::default()
        }));
        assert_eq!(m.advisor_default.model.as_deref(), Some("openai/o3-pro"));
        assert_eq!(info_lines(&m), ["advisor → openai/o3-pro", "advisor → off"]);
    }

    /// 125 §7.3: the exact four lines; one line when off.
    #[test]
    fn advisor_status_prints_four_lines() {
        let (tx, mut rx) = unbounded_channel();
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        m.submit("/advisor status", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::AdvisorStatus(id)) if id == s));
        m.apply(UiEvent::AdvisorStatus(AdvisorStatus {
            session: s,
            ..AdvisorStatus::default()
        }));
        assert_eq!(
            info_lines(&m),
            ["advisor: off  ·  /advisor to pick a model"]
        );
        m.entries.clear();
        m.submit("/advisor status", &tx);
        rx.try_recv().unwrap();
        let mut status = advisor_status(s, AdvisorState::Reviewing, "anthropic/claude-opus-4-1");
        status.notes = agent::AdvisorNoteCounts {
            nit: 2,
            concern: 1,
            blocker: 0,
        };
        status.step = 14;
        status.context_used = 38_200;
        status.context_limit = 200_000;
        status.tokens_in = 120_400;
        status.tokens_out = 3_100;
        status.cost = Some(CostBucket {
            returned: vec![Money {
                currency: "USD".into(),
                nanos: 412_000_000,
            }],
            ..CostBucket::default()
        });
        status.last_note = Some(note(
            AdvisorSeverity::Concern,
            "cite the loop_.rs line before claiming drain_steer runs after every batch",
        ));
        m.apply(UiEvent::AdvisorStatus(status));
        assert_eq!(
            info_lines(&m),
            [
                "advisor → anthropic/claude-opus-4-1",
                "advisor: reviewing  ·  anthropic/claude-opus-4-1  ·  high",
                "notes: 2 nit · 1 concern · 0 blocker  ·  step 14  ·  backlog 0",
                // `k()` drops the decimal from 100k up (`120k`, not the design's illustrative `120.4k`).
                "context: 38.2k / 200k (19%)  ·  in 120k · out 3.1k  ·  cost $0.412",
                "last: [concern] cite the loop_.rs line before claiming drain_steer runs after every batch",
            ]
        );
    }

    /// 125 §7.4: the live card lands above a streaming tail (the reply keeps
    /// one `markdown::Text`), once per seq; the steer's `TurnStarted` is not a `◆ you`.
    #[test]
    fn advisory_added_interjects_card_above_streaming_tail() {
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        let turn = TurnId::new();
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn,
            text: "first ".into(),
        });
        let added = AgentEvent::AdvisoryAdded {
            session: s,
            seq: 7,
            advisory: advisory(
                SessionId::new(),
                vec![note(AdvisorSeverity::Concern, "check the lock order")],
            ),
        };
        m.apply_agent(added.clone());
        m.apply_agent(added);
        m.apply_agent(AgentEvent::TextDelta {
            session: s,
            turn,
            text: "second".into(),
        });
        assert_eq!(m.entries.len(), 2, "one card, one assistant entry");
        assert!(
            matches!(&m.entries[0], Entry::Advisory { advisory, expanded: false } if advisory.notes.len() == 1)
        );
        assert!(matches!(&m.entries[1], Entry::Assistant(t) if !t.is_done()));
        // The blocker that opened a turn: its row is the card already.
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn: TurnId::new(),
            turn_seq: 7,
            text: "<advisory severity=\"blocker\" guidance=\"weigh\">stop</advisory>".into(),
            model: "m".into(),
        });
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn: TurnId::new(),
            turn_seq: 9,
            text: "<advisory severity=\"concern\" guidance=\"weigh\">not carded yet</advisory>"
                .into(),
            model: "m".into(),
        });
        assert!(!m.entries.iter().any(|e| matches!(e, Entry::User { .. })));
    }

    /// 125 §7.4: three notes collapsed, `… +N more notes`; Ctrl+O expands the
    /// newest expandable card and skips one with nothing hidden.
    #[test]
    fn advisory_card_collapses_to_three_notes() {
        let mut m = Model::default();
        let five = (0..5)
            .map(|i| note(AdvisorSeverity::Nit, &format!("note {i}")))
            .collect();
        m.entries.push(Entry::Advisory {
            advisory: advisory(SessionId::new(), five),
            expanded: false,
        });
        assert_eq!(m.entries[0].height(80), 4 + 3 + 1);
        m.toggle_card();
        assert!(matches!(
            &m.entries[0],
            Entry::Advisory { expanded: true, .. }
        ));
        assert_eq!(m.entries[0].height(80), 4 + 5);
        m.toggle_card();
        assert!(matches!(
            &m.entries[0],
            Entry::Advisory {
                expanded: false,
                ..
            }
        ));
        m.entries.push(Entry::Advisory {
            advisory: advisory(SessionId::new(), vec![note(AdvisorSeverity::Blocker, "x")]),
            expanded: false,
        });
        assert_eq!(m.entries[1].height(80), 4 + 1);
        m.toggle_card();
        assert!(
            matches!(
                &m.entries[1],
                Entry::Advisory {
                    expanded: false,
                    ..
                }
            ),
            "nothing to expand"
        );
        assert!(
            matches!(&m.entries[0], Entry::Advisory { expanded: true, .. }),
            "the newest card with hidden notes toggles"
        );
    }

    /// 125 §7.4: a history `advisory` row is a card, never a `◆ you`; the
    /// wrapper alone (an older engine's `User` row) yields no bubble either.
    #[test]
    fn history_advisory_row_renders_card_not_user_bubble() {
        let opts = markdown::Options::default();
        let entries = from_history(
            HistoryEntry::Advisory {
                advisory: advisory(
                    SessionId::new(),
                    vec![
                        note(AdvisorSeverity::Nit, "a"),
                        note(AdvisorSeverity::Blocker, "b"),
                    ],
                ),
                context: None,
            },
            &opts,
        );
        assert_eq!(entries.len(), 1);
        let Entry::Advisory {
            advisory,
            expanded: false,
        } = &entries[0]
        else {
            panic!("not a card: {entries:?}");
        };
        assert_eq!(advisory_severity(advisory), AdvisorSeverity::Blocker);
        assert_eq!(advisory_title(advisory), "◎ advisor · 2 notes");
        assert!(
            user_entries(
                "<advisory severity=\"nit\" guidance=\"weigh\">a</advisory>".into(),
                Some(3)
            )
            .is_empty()
        );
    }

    /// 125 §7.5: `AdvisorChanged` drives the header tag, the status segment,
    /// the spinner and one `advisor → …` line per model change.
    #[test]
    fn advisor_changed_updates_header_cost_and_status() {
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        let catalog = Catalog::load(
            std::env::temp_dir()
                .join("theuth-tui-no-user-file")
                .as_path(),
        );
        assert!(m.advisor_segment().is_none() && !m.animating());
        let mut status = advisor_status(s, AdvisorState::Reviewing, "anthropic/claude-opus-4-1");
        status.cost = Some(CostBucket {
            returned: vec![Money {
                currency: "USD".into(),
                nanos: 12_000_000,
            }],
            ..CostBucket::default()
        });
        m.apply_agent(AgentEvent::AdvisorChanged {
            session: s,
            status: status.clone(),
        });
        assert!(m.header(&catalog).0.contains("  ·  advisor $0.012"));
        let (segment, state) = m.advisor_segment().unwrap();
        assert!(segment.starts_with("  ·  ◎ advisor: ") && segment.ends_with(" reviewing…"));
        assert_eq!(state, AdvisorState::Reviewing);
        assert!(m.animating());
        // Counters move without a new line; idle stops the spinner.
        status.state = AdvisorState::Idle;
        status.step = 3;
        m.apply_agent(AgentEvent::AdvisorChanged {
            session: s,
            status: status.clone(),
        });
        assert_eq!(
            m.advisor_segment().map(|(t, _)| t).as_deref(),
            Some("  ·  ◎ advisor: idle")
        );
        assert!(!m.animating());
        // Another session's advisor is not ours.
        m.apply_agent(AgentEvent::AdvisorChanged {
            session: SessionId::new(),
            status: advisor_status(SessionId::new(), AdvisorState::Halted, "x/y"),
        });
        assert_eq!(m.advisor.state, AdvisorState::Idle);
        m.apply_agent(AgentEvent::AdvisorChanged {
            session: s,
            status: AdvisorStatus {
                session: s,
                ..AdvisorStatus::default()
            },
        });
        assert!(m.advisor_segment().is_none());
        assert!(!m.header(&catalog).0.contains("advisor"));
        assert_eq!(
            info_lines(&m),
            ["advisor → anthropic/claude-opus-4-1", "advisor → off"]
        );
    }

    /// 125 §7.5: the hidden advisor child is never a board row — by kind in
    /// `list_agents`, by the link's id on `SubagentSpawned` — and spawns no line.
    #[test]
    fn spawned_advisor_never_reaches_board() {
        let s = SessionId::new();
        let mut m = Model {
            session: Some(s),
            ..Model::default()
        };
        let advisor = SessionId::new();
        let mut status = advisor_status(s, AdvisorState::Idle, "openai/o3-pro");
        status.advisor = Some(advisor);
        m.apply_agent(AgentEvent::AdvisorChanged { session: s, status });
        m.apply_agent(AgentEvent::SubagentSpawned {
            parent: s,
            child: advisor,
            name: "advisor".into(),
            cap: None,
        });
        assert!(m.agents.is_empty() && m.board().is_empty());
        assert!(!info_lines(&m).iter().any(|l| l.starts_with("↳ spawned")));
        let info = |id: SessionId, kind: SessionKind| AgentInfo {
            id,
            name: "x".into(),
            parent: s,
            depth: 1,
            agent: None,
            model: "m".into(),
            state: AgentState::Running,
            started_at: None,
            finished_at: None,
            last: String::new(),
            project: None,
            retention: agent::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind,
        };
        let worker = SessionId::new();
        m.apply(UiEvent::Agents(vec![
            info(advisor, SessionKind::Advisor),
            info(worker, SessionKind::Chat),
        ]));
        assert_eq!(m.agents.keys().copied().collect::<Vec<_>>(), [worker]);
    }
}
