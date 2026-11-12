use super::*;
/// Everything the UI can receive: bus events plus replies to its own commands.
#[derive(Debug)]
pub enum UiEvent {
    /// Local/remote topic intake (no typed bus replay).
    Wire(crate::topics::TopicEvent),
    /// Typed owner reply erased only at the registry boundary.
    Component(crate::components::LocalReply),
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
    /// Reply to [`Command::ListSessions`].
    Sessions(Vec<SessionSummary>),
    /// Reply to [`Command::ListAgents`]: the session's subtree with live state.
    Agents(Vec<AgentInfo>),
    /// Reply to [`Command::Load`] with `settled_only: false`: shown as is.
    History(SessionId, Loaded),
    /// Reply to [`Command::Load`] with `settled_only: true` (48 §5): shown
    /// only when it is settled and not older than what is on screen.
    Refreshed(SessionId, Loaded),
    /// Reply to [`Command::EditContext`]: what was removed; the transcript reload follows.
    ContextRemoved(SessionId, ContextRemoval),
    /// Reply to [`Command::Fork`]: the new session, already loaded.
    Forked(SessionId, Loaded),
    /// Reply to [`Command::Send`]: the session the turn went to.
    SessionCreated(SessionId),
    /// Draft creation failed; deliver each queued owner's typed failure.
    DraftFailed(String),
    /// Reply to [`Command::PathComplete`]: the generation that asked, `None` when the call failed.
    PathComplete(u64, Option<agent::pathac::PathCompletion>),
    /// Reply to [`Command::SetAdvisor`] / [`Command::AdvisorStatus`] (125 §7.1).
    AdvisorStatus(AdvisorStatus),
    /// Reply to [`Command::SetApprovals`].
    Approvals(ApprovalsState),
    /// Reply to [`Command::ShowPrompt`].
    Prompt(String),
    /// Reply to [`Command::ListSkills`].
    Skills(Vec<SkillMeta>),
    /// Reply to [`Command::LoadSkill`].
    Skill(SkillBody),
    /// Reply to [`Command::AddSkillRepo`]: the skills the clone holds.
    SkillsAdded(Vec<SkillMeta>),
    /// Reply to [`Command::SyncSkills`].
    Synced(Vec<SyncResult>),
    /// Reply to [`Command::ListAcp`] (also sent after [`Command::RemoveAcp`]).
    Acp(Vec<EntryInfo>),
    /// Reply to [`Command::ListAcpRegistry`].
    AcpRegistry(Vec<RegistryAgent>),
    /// Reply to [`Command::AddAcp`].
    AcpAdded(EntryInfo),
    /// A command succeeded with nothing to show but a notice.
    Info(String),
    /// Reply to [`Command::GetCompactStyle`].
    CompactStyle(CompactStyle),
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

    /// Reply to [`Command::CreateSession`].
    SessionOpened(SessionOpen),
    /// Reply to [`Command::LeaveSession`]: the root left and how far its cleanup got.
    SessionLeft(SessionId, CloseResult),
}

/// Everything the UI can ask of the rest of the process. One task turns
/// these into `rpc::Api` calls and their replies into [`UiEvent`]s.
#[derive(Debug)]
pub enum Command {
    /// Generic owner effect; new panels do not add command cases.
    Component(crate::components::Effect),
    /// Ordered widget input addressed to its registered owner, without an RPC task.
    ComponentInput(crate::components::LocalReply),
    /// `send_input`.
    Send {
        /// `None` creates a session.
        session: Option<SessionId>,
        /// User text.
        text: String,
        /// Attached image files (17 §2).
        files: Vec<PathBuf>,
        /// `session: None` only (148 R1): the draft's pending model/effort; the
        /// session is opened with `create_session(draft)` first, never the default.
        draft: Option<CreateSessionOptions>,
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
    /// `session_take(id, "user")`: `/take` continues the open session on this engine (68 §10).
    TakeSession(SessionId),
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
    /// `endless_fold` (137G `/endless fold`).
    EndlessFold {
        /// Target.
        session: SessionId,
    },
    /// `endless_pause` (137G `/endless pause|resume`).
    EndlessPause {
        /// Target.
        session: SessionId,
        /// Pause (`true`) or resume.
        paused: bool,
    },
    /// `set_session_compactor_model` (137G `/endless model <id|clear>`).
    EndlessModel {
        /// Target.
        session: SessionId,
        /// The model id; `None` clears this chat's pick.
        model: Option<String>,
    },
    /// `compact_style`.
    GetCompactStyle,
    /// `set_compact_style`.
    SetCompactStyle(CompactStyle),
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
    /// `list_agents`: the subtree under a session.
    ListAgents(SessionId),
    /// `load_session`; `settled_only` (48 §5) reads through the idle actor
    /// and replies with [`UiEvent::Refreshed`] instead of [`UiEvent::History`].
    Load {
        /// Session.
        session: SessionId,
        /// Refuse a busy session's half-persisted transcript.
        settled_only: bool,
    },
    /// `remove_context`, then `load_session(settled_only: true)`.
    EditContext {
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
    /// `set_advisor_max_consults` (R1.2): the per-turn `consult` cap, 0-32.
    SetAdvisorMaxConsults(u8),
    /// `resume_child` on the open ACP session (75 §10).
    ResumeChild(SessionId),
    /// `set_approvals`.
    SetApprovals {
        /// `None` = global, persisted.
        session: Option<SessionId>,
        /// On or off.
        enabled: bool,
    },
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
        /// `session: None` only (148 R1): the draft's pending model/effort for the started session.
        draft: Option<CreateSessionOptions>,
    },
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
        /// Custom provider name.
        name: String,
        /// OpenAI-compatible base URL.
        base_url: String,
        /// API key.
        key: String,
    },
    /// `remove_provider`.
    RemoveProvider(String),
    /// `refresh_provider_models` (162 D8).
    RefreshProvider(String),
    /// `local_servers_probe` (177 E8).
    FindLocalServers,
    /// `set_model_limits` (177 E6).
    SetModelLimits {
        /// Provider name.
        provider: String,
        /// Model id within the provider.
        model: String,
        /// Context window; `None` = unknown.
        context: Option<u32>,
        /// Max output; `None` = keep the stored one, `Some(0)` = unknown (R2-8).
        output: Option<u32>,
    },
    /// `list_providers`.
    ListProviders,

    /// `set_session_project`: label or clear the root's project (51 §3).
    SetSessionProject {
        /// Any session of the tree.
        session: SessionId,
        /// `None` clears.
        project: Option<String>,
    },
    /// `create_session`: explicit creation, private or durable (51 §4.1).
    CreateSession(CreateSessionOptions),
    /// Open an unsent draft without resetting the composer or component owners.
    CreateDraft(CreateSessionOptions),
    /// `leave_session`: release the private root this view owns (51 §4.3).
    LeaveSession {
        /// The root.
        session: SessionId,
        /// Its lease secret.
        secret: String,
    },
    /// `cancel(session, "tree")`: the turn and every running descendant (133 §5).
    CancelTree(SessionId),
}

pub(crate) fn send(tx: &UnboundedSender<Command>, cmd: Command) {
    // The receiver lives as long as the UI; a closed channel means shutdown.
    let _ = tx.send(cmd);
}
