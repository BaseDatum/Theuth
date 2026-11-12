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
    /// Reply to [`Command::ListRoster`] (67 §12).
    Roster(Vec<roster::AgentSummary>),
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
    /// Generic owner effect; new panels do not add command cases.
    Component(crate::components::Effect),
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
    /// `autoresearch.list` / `autoresearch.get` + last 10 experiments: `/autoresearch [<slug>]` (140 §13.1).
    AutoresearchStatus(Option<String>),
    /// `autoresearch.control`: `/autoresearch pause|resume|stop|stop_now <slug>`.
    AutoresearchControl(String, autoresearch::ControlOp),
    /// `autoresearch.steer`: `/autoresearch steer <slug> <text>`.
    AutoresearchSteer(String, String),
    /// `agents_list`: the roster (67 §12).
    ListRoster,
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
    /// `set_model`: the shown session only (148 R1); with no session yet the
    /// pick is the draft's (`Model::draft`), no command.
    SetModel {
        /// Session to switch now.
        session: SessionId,
        /// `provider/model` id.
        model: String,
    },
    /// `set_effort`: the shown session only (148 R1); with no session yet the
    /// pick is the draft's (`Model::draft`), no command.
    SetEffort {
        /// Session to switch from its next turn.
        session: SessionId,
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
    /// `set_advisor_max_consults` (R1.2): the per-turn `consult` cap, 0-32.
    SetAdvisorMaxConsults(u8),
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
    /// `set_mcp_hidden`, then `list_mcp`. Without a session yet, one is created
    /// first: the hide must exist before the first turn lists tools.
    SetHidden {
        /// Root session; `None` = create it.
        session: Option<SessionId>,
        /// Registered tool names.
        tools: Vec<String>,
        /// `true` hides.
        hidden: bool,
        /// `session: None` only (148 R1): the draft's pending model/effort for the created session.
        draft: Option<CreateSessionOptions>,
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
        /// `session: None` only (148 R1): the draft's pending model/effort for the started session.
        draft: Option<CreateSessionOptions>,
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
    /// `export_artifact(id, ~/Downloads/<name>, session)`.
    ExportArtifact {
        /// The artifact.
        id: String,
        /// Its file name.
        name: String,
        /// The owned private session it belongs to; `None` when durable.
        session: Option<SessionId>,
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
    /// `dictation_start(None, opts)` (153 §14.1: `Surface::Tui`, `AgentPrompt`, the slash commands,
    /// `press_ago_ms` from `pressed`); the reply is [`UiEvent::DictationStarting`] or [`UiEvent::DictationFailed`].
    DictationStart {
        /// When Ctrl+D was pressed (153 §8.7).
        pressed: std::time::Instant,
        /// One completion/help vocabulary, including component commands.
        commands: Vec<String>,
    },
    /// `dictation_stop(id)`: close the mic; `DictationFinished` follows on the bus.
    DictationStop(voice::DictationId),
    /// `dictation_correction(id, sent_text)` (153 §6.1): fire-and-forget at send.
    DictationCorrection {
        /// The dictation that contributed to the message.
        id: voice::DictationId,
        /// What went out.
        text: String,
    },
    /// `dictionary_suggestion_answer(id, answer)` (153 §6.3).
    SuggestionAnswer {
        /// The suggestion shown.
        id: voice::SuggestionId,
        /// `y` add · `n` not now · `N` never.
        answer: voice::SuggestionAnswer,
    },
    /// `/voice`: `voice_status` + `dictation_status` + `list_input_devices` → one `Info` (153 §8.1/8.7).
    VoiceStatus,
    /// `/voice mic <name|default>` → `set_input_devices` (153 §8.1).
    SetInputDevices(Vec<String>),
    /// `/voice shape <full|plain|off>` → `set_shape(Surface::Tui, preset)` (153 §4.5).
    SetShape(voice::Preset),
    /// `talk_start(session, "device", null)`: a live spoken conversation (76 §5.1);
    /// `session: None` = no session yet: one is created first (`SessionOpened`), as the desktop's
    /// Talk entry does, on the draft's pending model/effort when there is one (148 R1).
    /// The reply is [`UiEvent::TalkStarting`] or [`UiEvent::TalkFailed`].
    TalkStart {
        /// The shown session.
        session: Option<SessionId>,
        /// `session: None` only: the draft's options.
        draft: Option<CreateSessionOptions>,
    },
    /// `talk_stop(id)`; `TalkFinished` follows on the bus.
    TalkStop(talk::TalkId),
    /// `cancel(session, "tree")`: the turn and every running descendant (133 §5).
    CancelTree(SessionId),
}

pub(crate) fn send(tx: &UnboundedSender<Command>, cmd: Command) {
    // The receiver lives as long as the UI; a closed channel means shutdown.
    let _ = tx.send(cmd);
}
