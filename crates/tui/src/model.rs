//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use acp::EntryInfo;
use acp::registry::RegistryAgent;
use agent::search::SearchHit;
use agent::todo::{Status as TodoStatus, TodoSnapshot, TodoSource};
use agent::{
    AgentEvent, AgentInfo, AgentState, AwsProfiles, CompactStyle, ContextAttachment, ContextEntry,
    ContextItem, ContextKind, ContextRemoval, ContextSnapshot, ContextTarget, HistoryEntry, Loaded,
    ModelInfo, ProviderInfo, QueuedInput, SessionSummary, SkillBody, SkillMeta, Usage,
};
use iocraft::KeyCode;
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

use crate::clip::Image;

/// Everything the UI can receive: bus events plus replies to its own commands.
#[derive(Debug)]
pub enum UiEvent {
    /// Agent progress.
    Agent(AgentEvent),
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
    /// Reply to [`Command::Search`]: the query echoed with its hits.
    Search(String, Vec<SearchHit>),
    /// Reply to [`Command::ListAgents`]: the session's subtree with live state.
    Agents(Vec<AgentInfo>),
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
    /// Reply to [`Command::SetModel`].
    ModelSet(ModelInfo),
    /// Reply to [`Command::SetEffort`]: the value now in force.
    EffortSet(Option<String>),
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
    /// Reply to [`Command::ComputerStatus`].
    Computer(computer::PermissionStatus),
    /// Reply to [`Command::LinkStatus`] and the link commands that change it.
    Link(link::LinkStatus),
    /// Reply to [`Command::LinkPair`]: the string the QR card encodes.
    Pairing(link::Pairing),
    /// Link plugin traffic (bus event).
    LinkEvent(link::LinkEvent),
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
    },
    /// Reply to [`Command::ListSecrets`]: names, notes, versions. Never values.
    Secrets(secrets::SecretsList),
    /// Reply to [`Command::SetSecret`] / [`Command::RestoreSecret`]: the new version.
    SecretSet(secrets::SecretInfo),
    /// Secrets plugin traffic (bus event): a change, a `secret_request`, its resolution.
    SecretsEvent(secrets::SecretsEvent),
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
    /// `steer`: queued until the current tool batch ends.
    Steer {
        /// Target.
        session: SessionId,
        /// Client-minted id, echoed by the `Input*` events.
        id: InputId,
        /// User text.
        text: String,
        /// Attached image files (17 §2).
        files: Vec<PathBuf>,
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
    /// `computer_permissions`.
    ComputerStatus,
    /// `compact`.
    Compact {
        /// Target.
        session: SessionId,
        /// One-shot style; `None` = configured default.
        style: Option<CompactStyle>,
    },
    /// `compact_style`.
    GetCompactStyle,
    /// `set_compact_style`.
    SetCompactStyle(CompactStyle),
    /// `title_model`.
    GetTitleModel,
    /// `set_title_model`; `None` clears it.
    SetTitleModel(Option<String>),
    /// `list_aws_profiles`.
    ListAwsProfiles,
    /// `set_aws_profile`; `None` restores the AWS default credential chain.
    SetAwsProfile(Option<String>),
    /// `list_sessions`.
    ListSessions,
    /// `search_sessions`: fuzzy match over titles and message text.
    Search(String),
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
    /// `link_rename` (harness display name), then `link_status`.
    LinkRename(String),
    /// `link_rename_device(id, name)`, then `link_status`.
    LinkRenameDevice(String, String),
    /// `link_revoke_device`, then `link_status`.
    LinkRevoke(String),
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
    /// Open a local file with the OS default handler (`open` / `xdg-open`, detached).
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
}

impl CardKind {
    /// Header glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            CardKind::Peer => "⟵",
            CardKind::Skill => "◇",
            CardKind::Prompt => "§",
            CardKind::Job => "⚙",
            CardKind::Schedule => "◷",
            CardKind::Link | CardKind::Node => "⌁",
            CardKind::Artifact => "▣",
            CardKind::Removed => "⊘",
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
            Entry::User { text, .. } => text_rows(text, inner) + 4,
            // Rendered fences (diagrams, surfaces) never exceed the construction
            // width, so they count one row per line like everything else.
            Entry::Assistant(t) => md(t, width),
            Entry::Card { text, expanded, .. } => 4 + if *expanded { md(text, inner) } else { 0 },
            Entry::Reasoning { done: true, .. } | Entry::Info(_) | Entry::Error(_) => 1,
            Entry::Reasoning { text, .. } => text_rows(text, width),
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
}

/// What the input box shows for a held paste: `[⎘ paste 1: 69 lines]`
/// (`chars` for a single long line), `[⎘ image 2: 1440×900]` (file name
/// when the size is unknown; `file` for a PDF). `i` is the index into `Model::pastes`.
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
    }
}

/// Split a harness-injected input into `(kind, title, body)` per wrapper.
/// Inputs queued during a turn arrive as one text: `<subagent_result
/// name="…">` / `<message from="…">` / `<job_result id="…" command="…"
/// exit="…" secs="…">` blocks joined by newlines. Harness notes
/// (`<context_summary>`, `<todo>`, `<context_pressure>`, `<runtime>`,
/// `<knowledge>`; design docs 07 §8.7, 10 §4) are wrappers too but yield no
/// card. `None` for human text
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
            "schedule" => (Some("name=\""), "</schedule>"),
            "context_summary" => (None, "</context_summary>"),
            "todo" => (None, "</todo>"),
            "context_pressure" => (None, "</context_pressure>"),
            "runtime" => (None, "</runtime>"),
            "knowledge" => (None, "</knowledge>"),
            _ => return None,
        };
        let (body, tail) = after.split_once(close)?;
        if let Some(key) = key {
            let from = attr(open, key)?;
            let (kind, title) = if name == "job_result" {
                let command = attr(open, "command=\"")
                    .unwrap_or_default()
                    .replace("&quot;", "\"")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&");
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
        "/effort",
        "[level|clear]",
        "reasoning effort (picker without arg)",
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
    (
        "/jobs",
        "[kill <id>]",
        "list this session's background jobs",
    ),
    (
        "/schedules",
        "[run <name>|history <name>|pause <name>|resume <name>]",
        "scheduled prompts: list, fire one now, show its runs, pause or resume a loop",
    ),
    (
        "/computer",
        "",
        "computer control: permissions and displays",
    ),
    ("/parent", "", "open the parent session (from a subagent)"),
    ("/sessions", "", "switch session (picker)"),
    (
        "/search",
        "<query>",
        "find a session by title or message text (fuzzy)",
    ),
    ("/resume", "<id>", "open a session"),
    ("/new", "", "start a new session"),
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
    ("/cancel", "", "abort the in-flight turn"),
    (
        "/compact",
        "[summary|cut|snap|style …]",
        "fold context; style sets the default",
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
        "OpenAI-compatible providers (add prompts for name, url, key)",
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
        "/nodes",
        "[add <key>|rm <n>|name <n> <name>|invite <n>]",
        "machines this harness can run sessions on",
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

/// An open menu; modal except `Slash`, which rides on the input box.
#[derive(Debug)]
pub enum Menu {
    /// Command list filtered by the input's first word.
    Slash {
        /// Highlighted match.
        cursor: usize,
    },
    /// `/model` picker.
    Models {
        /// Everything `ListModels` returned.
        items: Vec<ModelInfo>,
        /// Substring typed inline (matched on id and name).
        filter: String,
        /// Highlighted filtered row.
        cursor: usize,
    },
    /// `/effort` picker.
    Efforts {
        /// The model's effort levels, then `clear`.
        items: Vec<String>,
        /// Highlighted row.
        cursor: usize,
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
        /// Top-level sessions, newest first.
        items: Vec<SessionSummary>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/search` picker.
    Search {
        /// What was searched for.
        query: String,
        /// Best score first, one per session.
        hits: Vec<SearchHit>,
        /// Highlighted row.
        cursor: usize,
    },
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
        /// The session subtree's artifacts, newest first.
        items: Vec<artifact::Artifact>,
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
fn now_ms() -> u64 {
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
            Prompt::LinkName => "link  harness name  (esc cancel)".into(),
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
    /// Under 70 %.
    Normal,
    /// 70–90 %.
    Warn,
    /// 90 % and above.
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
    /// Model of the shown session (`TurnStarted.model`; the config default before that).
    pub model_id: String,
    /// Reasoning effort of the shown session; `None` = provider default.
    pub effort: Option<String>,
    /// A `SetEffort` is in flight; its `EffortChanged` echo is not announced twice.
    awaiting_effort: bool,
    /// Effort levels per model for the `/effort` picker; `None` in the line printer.
    pub catalog: Option<Arc<Catalog>>,
    /// Tokens the shown session's window held after its last turn.
    pub context: u32,
    /// Open menu.
    pub menu: Option<Menu>,
    /// Pending approvals for the current session or its subtree, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// Pending `ask_user` calls for the current session or its subtree, oldest first; the front is shown.
    pub asks: VecDeque<Ask>,
    /// `tools.approvals` (global); false = never prompts.
    pub approvals_global: bool,
    /// The root session opted out of prompts (`/approvals off`).
    pub approvals_off: bool,
    /// `Info.app_enabled`: the app plugin runs, so `/apps` is offered. Off,
    /// app records stay readable under `/artifacts` (46 §9).
    pub app_enabled: bool,
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
    /// The shown session's harvest call is in flight (`Harvesting`).
    pub harvesting: bool,
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
}

impl Model {
    /// Fresh state showing `default_model` in the header.
    pub fn new(default_model: String, approvals: bool) -> Self {
        Self {
            model_id: default_model,
            approvals_global: approvals,
            ..Self::default()
        }
    }

    /// `Self::new` keeping the current model, effort, catalog and skill index: `/new` and `/resume`.
    fn reset(&mut self) {
        *self = Self {
            effort: self.effort.take(),
            catalog: self.catalog.take(),
            skills: std::mem::take(&mut self.skills),
            width: self.width,
            secrets: self.secrets.take(),
            link: self.link.take(),
            nodes: self.nodes.take(),
            update: self.update.take(),
            todo_open: std::mem::take(&mut self.todo_open),
            app_enabled: self.app_enabled,
            ..Self::new(std::mem::take(&mut self.model_id), self.approvals_global)
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
        self.effort = loaded.effort;
        self.streaming = loaded.running;
        self.session_name = loaded.name;
        self.session_parent = loaded.parent;
        self.loaded_skills = loaded.skills;
        self.context_revision = loaded.context_revision;
        self.context_editable = loaded.context_editable;
        self.context_reason = loaded.context_reason;
        self.context_stale = false;
        if let Some(n) = loaded.context_estimate {
            self.context = n;
        }
        self.loaded_todos(loaded.todos);
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

    /// The `secret_request` currently shown, if any.
    pub fn secret_request(&self) -> Option<&SecretRequest> {
        self.secret_requests.front()
    }

    /// The input box takes keys: no approval prompt, no modal menu (the
    /// slash popup rides on the box) and the todo card not focused (F2). A
    /// shown `secret_request` types into the box (masked).
    pub fn input_focused(&self) -> bool {
        if self.approval().is_some() {
            return false;
        }
        if self.secret_request().is_some() {
            return true;
        }
        if let Some(a) = self.ask() {
            return a.typing;
        }
        !self.todo_focus && !self.modal_menu()
    }

    /// A menu that takes the keys is open: every menu but the slash popup.
    pub fn modal_menu(&self) -> bool {
        !matches!(self.menu, None | Some(Menu::Slash { .. }))
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
            self.entries.push(Entry::Info(format!(
                "{} is text-only; the image will be sent as omitted — /model to switch",
                self.model_id
            )));
        }
        let p = Paste::Image(img);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }

    /// `line` with every paste token the user left in place replaced by its
    /// text and every image token removed (with one adjacent space), the
    /// images' paths in token order; `pastes` is emptied either way. A token
    /// the user deleted drops its paste.
    fn expand_pastes(&mut self, line: &str) -> (String, Vec<PathBuf>) {
        let mut text = line.to_owned();
        let mut files = Vec::new();
        for (i, p) in std::mem::take(&mut self.pastes).into_iter().enumerate() {
            let token = paste_token(i, &p);
            match p {
                Paste::Text(t) => text = text.replace(&token, &t),
                Paste::Image(img) => {
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
                    files.push(img.path);
                }
            }
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
            )
        }) else {
            return;
        };
        match e {
            Entry::Card { expanded, .. } | Entry::Compacted { expanded, .. } => {
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
            || self.agents.values().any(AgentRow::live)
            || self.pairing.is_some()
            || self.invite.is_some()
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
    /// the trailing activity row; `None` when idle.
    pub fn activity(&self) -> Option<String> {
        if self.compacting {
            return Some("compacting…".into());
        }
        if self.harvesting {
            return Some("harvesting…".into());
        }
        if !self.streaming {
            return None;
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
            }
            UiEvent::Ask(req) => {
                if !req.questions.is_empty()
                    && (self.session == Some(req.session) || self.agents.contains_key(&req.session))
                {
                    self.asks.push_back(Ask::new(req));
                }
            }
            UiEvent::AskResolved(r) => self.asks.retain(|a| a.req.id != r.id),
            UiEvent::Sessions(list) => {
                let items: Vec<SessionSummary> =
                    list.into_iter().filter(|s| s.parent.is_none()).collect();
                if items.is_empty() {
                    self.entries.push(Entry::Info("no sessions".into()));
                } else {
                    let cursor = items
                        .iter()
                        .position(|s| Some(s.id) == self.session)
                        .unwrap_or(0);
                    self.menu = Some(Menu::Sessions { items, cursor });
                }
            }
            UiEvent::Search(query, hits) => {
                self.menu = Some(Menu::Search {
                    query,
                    hits,
                    cursor: 0,
                });
            }
            UiEvent::Agents(list) => {
                // Reseeding keeps what events taught about rows still there.
                let mut old = std::mem::take(&mut self.agents);
                self.agents = list
                    .into_iter()
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
                if self.session == Some(id) {
                    self.refresh(loaded);
                }
            }
            UiEvent::ContextItems {
                session,
                category,
                snapshot,
            } => self.open_shake(session, category, snapshot),
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
                }
            }
            UiEvent::Models(items) => {
                if items.is_empty() {
                    self.entries
                        .push(Entry::Error("no models: no provider is configured".into()));
                } else {
                    self.menu = Some(Menu::Models {
                        items,
                        filter: String::new(),
                        cursor: 0,
                    });
                }
            }
            // Chained picker: a model with effort levels asks for one right away (Esc skips).
            UiEvent::ModelSet(info) => {
                self.model_id = info.id;
                if !info.efforts.is_empty() {
                    self.open_efforts(info.efforts);
                }
            }
            UiEvent::EffortSet(effort) => {
                self.awaiting_effort = false;
                self.entries.push(Entry::Info(match &effort {
                    Some(e) => format!("effort: {e}"),
                    None => "effort cleared".to_owned(),
                }));
                self.effort = effort;
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
            UiEvent::Jobs(jobs) => self.entries.push(Entry::Info(tools::jobs::table(&jobs))),
            UiEvent::Schedules(rows) => self.entries.push(Entry::Info(schedules_table(&rows))),
            UiEvent::ScheduleHistory(name, info, rows) => self
                .entries
                .push(Entry::Info(schedule_history_table(&name, info.as_ref(), &rows))),
            UiEvent::Computer(status) => self.entries.push(Entry::Info(computer_line(&status))),
            UiEvent::ArtifactShown(ev) => {
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
            UiEvent::Artifacts { items, pick, apps } => {
                if !pick {
                    for a in items {
                        self.attach_artifact(a);
                    }
                } else if items.is_empty() {
                    self.entries.push(Entry::Info(
                        if apps { "no apps" } else { "no artifacts" }.into(),
                    ));
                } else {
                    self.menu = Some(Menu::Artifacts {
                        items,
                        cursor: 0,
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
            // Status always; the picker when there is a choice to make or a
            // stale selection to clear (row 0 restores the default chain).
            UiEvent::AwsProfiles { info, pick } => {
                self.entries.push(Entry::Info(aws_profile_line(&info)));
                if pick && (info.profiles.len() > 1 || info.selected.is_some()) {
                    self.menu = Some(Menu::AwsProfiles { info, cursor: 0 });
                }
            }
            UiEvent::Providers(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info(
                        "no custom providers; /provider add to create one".into(),
                    ));
                } else {
                    for p in list {
                        self.entries.push(Entry::Info(format!(
                            "{}  {}  {} models",
                            p.name, p.base_url, p.models
                        )));
                    }
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
                            "wth {} is installed; it runs the next time you start wth.",
                            ev.version
                        )
                    } else {
                        format!(
                            "wth {} is available; `wth --update` installs it (the binary's directory is not writable).",
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
                self.awaiting_link = false;
                self.awaiting_secrets = false;
                self.awaiting_nodes = false;
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
        }
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
        }
    }

    fn apply_agent(&mut self, ev: AgentEvent) {
        if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
            self.session = ev.session();
            self.awaiting_session = false;
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
            ev => ev,
        };
        // Roster bookkeeping for the subtree, independent of what is shown.
        match &ev {
            AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
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
                        }),
                    );
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
                    self.interject(card(kind, from.clone(), text));
                }
                return;
            }
            // Queue bookkeeping mid-stream must not finish the streaming tail below.
            AgentEvent::InputQueued { session, id, text } => {
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
            AgentEvent::TitleChanged { .. } | AgentEvent::SessionDeleted { .. } => return,
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
            AgentEvent::TurnStarted { session, .. } if self.session == Some(*session) => {
                self.streaming = true;
            }
            AgentEvent::TurnFinished { session, .. } if self.session == Some(*session) => {
                self.streaming = false;
            }
            _ => self.track(&ev),
        }
        // A subagent's stream is not rendered here; open it to read it.
        if ev.session().is_none_or(|s| self.session != Some(s)) {
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
                if injected(&text).is_some() {
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
                self.compacting = false;
                self.harvesting = false;
            }
            AgentEvent::ModelChanged { model, .. } => {
                self.entries.push(Entry::Info(format!("model → {model}")));
                self.model_id = model;
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
                self.harvesting = false;
                self.entries.push(Entry::Error(message));
            }
            AgentEvent::SubagentSpawned { .. }
            | AgentEvent::MessageDelivered { .. }
            | AgentEvent::InputWithdrawn { .. }
            | AgentEvent::TitleChanged { .. }
            | AgentEvent::SessionDeleted { .. }
            | AgentEvent::ContextChanged { .. }
            | AgentEvent::TodoChanged { .. } => {}
            AgentEvent::Harvesting { .. } => self.harvesting = true,
            AgentEvent::Harvested { added, updated, .. } => {
                self.harvesting = false;
                self.interject(Entry::Info(format!("knowledge +{added} ~{updated}")))
            }
            AgentEvent::SkillsChanged { skills, .. } => self.loaded_skills = skills,
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
            "/quit" | "/exit" => self.quit = true,
            "/sessions" => send(tx, Command::ListSessions),
            "/search" if arg.is_empty() => self
                .entries
                .push(Entry::Error("usage: /search <query>".into())),
            "/search" => send(tx, Command::Search(arg.to_owned())),
            "/resume" => match arg.parse::<SessionId>() {
                Ok(id) => self.resume(id, tx),
                Err(_) => self
                    .entries
                    .push(Entry::Error(format!("not a session id: {arg:?}"))),
            },
            "/new" => {
                self.reset();
                self.entries.push(Entry::Info("new session".into()));
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
            "/effort" if arg.is_empty() => {
                let items = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.closest(&self.model_id))
                    .map(|m| m.efforts())
                    .unwrap_or_default();
                self.open_efforts(items);
            }
            "/effort" => self.set_effort((arg != "clear").then(|| arg.to_owned()), tx),
            "/aws" if arg.is_empty() => send(tx, Command::ListAwsProfiles),
            "/aws" => send(
                tx,
                Command::SetAwsProfile((arg != "clear").then(|| arg.to_owned())),
            ),
            "/approvals" => self.approvals_cmd(arg, tx),
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
                _ => self.entries.push(Entry::Info(SCHEDULES_USAGE.into())),
            },
            "/computer" => send(tx, Command::ComputerStatus),
            "/compact" => self.compact_cmd(arg, tx),
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
            "/link" => self.link_cmd(arg, tx),
            "/nodes" => self.nodes_cmd(arg, tx),
            c if c.starts_with('/') => {
                // `/<skill> [args]` hands the skill to the model (04 §2.4). Before
                // the index has arrived the agent decides; it names an unknown skill.
                let name = &c[1..];
                if self.skills.is_empty() || self.skills.iter().any(|s| s.name == name) {
                    self.use_skill(name.to_owned(), arg.to_owned(), tx);
                } else {
                    self.entries
                        .push(Entry::Error(format!("unknown command {c}")));
                }
            }
            _ => {
                let (text, files) = self.expand_pastes(line);
                match self.session {
                    // Mid-turn: the card waits below the transcript until
                    // `InputInjected` says where in the conversation it landed.
                    Some(session) if self.streaming => {
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

    /// Switch to `id`: ask for its history and its subtree (design doc 19
    /// §3.5). `/resume`, `/sessions`, `/agents` and `/parent` all land here.
    /// Pending approvals ride along (an approval `o`pened from stays up); the
    /// subtree reply prunes the ones that do not belong to the new view.
    pub fn resume(&mut self, id: SessionId, tx: &UnboundedSender<Command>) {
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
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

    /// Cancel the current session's in-flight turn, if any.
    pub fn cancel(&self, tx: &UnboundedSender<Command>) {
        if let (Some(session), true) = (self.session, self.streaming) {
            send(tx, Command::Cancel(session));
        }
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
                    text.lines()
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
        let skills = match self.loaded_skills.len() {
            0 => String::new(),
            _ => format!("  ·  skills: {}", self.loaded_skills.join(", ")),
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
        format!("{session}  {state}{usage}{agents}{queued}{jobs}{skills}{editing}{scrolled}{todos}")
    }

    /// Line above the input: `model  ·  high  ·  ctx 12.3k / 200k (6%)  ·  cache 94%`
    /// and its colour level. In a subagent the model is headed by its peer
    /// name: `↳ grep-logs  child · model  ·  …`. The effort segment is
    /// omitted while unset; `cache` is the prefix-cache hit ratio of the last
    /// finished turn (design doc 07 §7), omitted until one has reported usage.
    pub fn header(&self, catalog: &Catalog) -> (String, Level) {
        let used = k(self.context);
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
        let model = match &self.session_name {
            Some(name) => format!("↳ {name}  child · {}", self.model_id),
            None => self.model_id.clone(),
        };
        match catalog.closest(&self.model_id).map(|m| m.limit.context) {
            Some(limit) if limit > 0 => {
                let pct = u64::from(self.context) * 100 / u64::from(limit);
                let level = match pct {
                    0..70 => Level::Normal,
                    70..90 => Level::Warn,
                    _ => Level::Danger,
                };
                (
                    format!(
                        "{model}{effort}  ·  ctx {used} / {} ({pct}%){cache}{}{}{}{}",
                        k(limit),
                        self.approvals_tag(),
                        self.link_tag(),
                        self.nodes_tag(),
                        self.update_tag()
                    ),
                    level,
                )
            }
            _ => (
                format!(
                    "{model}{effort}  ·  ctx {used}{cache}{}{}{}{}",
                    self.approvals_tag(),
                    self.link_tag(),
                    self.nodes_tag(),
                    self.update_tag()
                ),
                Level::Normal,
            ),
        }
    }

    fn approvals_tag(&self) -> &'static str {
        if !self.approvals_global || self.approvals_off {
            "  ·  approvals off"
        } else {
            ""
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
    fn open_efforts(&mut self, mut items: Vec<String>) {
        if items.is_empty() {
            self.entries.push(Entry::Info(format!(
                "{} has no reasoning effort levels",
                self.model_id
            )));
            return;
        }
        items.push("clear".into());
        self.menu = Some(Menu::Efforts { items, cursor: 0 });
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

    /// `/model` picker rows after the inline filter.
    pub fn models_filtered(&self) -> Vec<&ModelInfo> {
        match &self.menu {
            Some(Menu::Models { items, filter, .. }) => filter_models(items, filter),
            _ => Vec::new(),
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
            Some(Menu::Search { .. }) => self.search_key(code, tx),
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
            Some(Menu::Knowledge { .. }) => self.knowledge_key(code, tx),
            Some(Menu::KnowledgeHistory { .. }) => self.knowledge_history_key(code, tx),
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

    fn models_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Models {
            items,
            filter,
            cursor,
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        let filtered = filter_models(items, filter);
        *cursor = (*cursor).min(filtered.len().saturating_sub(1));
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(filtered.len().saturating_sub(1)),
            KeyCode::Char(c) => {
                filter.push(c);
                *cursor = 0;
            }
            KeyCode::Backspace => {
                filter.pop();
                *cursor = 0;
            }
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(m) = filtered.get(*cursor) {
                    let model = m.id.clone();
                    send(
                        tx,
                        Command::SetModel {
                            session: self.session,
                            model,
                        },
                    );
                    self.menu = None;
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/effort`: Enter sets the highlighted level (`clear` unsets) from the next turn.
    fn efforts_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Efforts { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).cloned();
                self.menu = None;
                if let Some(v) = picked {
                    self.set_effort((v != "clear").then_some(v), tx);
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
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).map(|s| s.id);
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

    /// `/search`: Enter switches to the highlighted hit's session, the same
    /// path as `/sessions`.
    fn search_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Search { hits, cursor, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(hits.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = hits.get(*cursor).map(|h| h.session);
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
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(a) = items.get(*cursor) {
                    send(tx, Command::OpenPath(a.path.clone()));
                }
                self.menu = None;
            }
            KeyCode::Char('e') => {
                if let Some(a) = items.get(*cursor) {
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
                let picked = items.get(*cursor).cloned();
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
            "link · {} · {state} · {} ({id}…)   p pair  r rename  x revoke  n harness name  e {}  esc",
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

    /// `/nodes` menu rows: `● name  platform  wth <version>  [<n> running]  paired <ago>`
    /// online, `○ name  platform  wth <version>  seen <ago>|revoked` otherwise,
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
                    "{} {:<20} {:<8} wth {:<8} {tail}",
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

/// Host part of a relay origin: `wss://relay.wth.rwx.dev/x` → `relay.wth.rwx.dev`.
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

fn filter_models<'a>(items: &'a [ModelInfo], filter: &str) -> Vec<&'a ModelInfo> {
    let f = filter.to_lowercase();
    items
        .iter()
        .filter(|m| m.id.to_lowercase().contains(&f) || m.name.to_lowercase().contains(&f))
        .collect()
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
        HistoryEntry::User {
            text,
            turn,
            context,
        } => {
            if removed(&context) {
                return vec![removed_card("you", &text)];
            }
            let mut out = user_entries(text, turn);
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
    use artifact::Kind;
    let ext = a.path.extension().map_or("", |e| e.to_str().unwrap_or(""));
    let path = a.path.display().to_string();
    let unreadable = |e: &dyn std::fmt::Display| format!("cannot read {path}: {e}");
    let md = match a.kind {
        Kind::Markdown | Kind::Text | Kind::Html | Kind::App => {
            match std::fs::read_to_string(&a.path) {
                Err(e) => unreadable(&e),
                Ok(s) => {
                    let (body, cut) = cap_lines(&s);
                    let text = match a.kind {
                        Kind::Markdown => body,
                        Kind::App => {
                            format!("[app] {APP_WHERE} · source:\n\n{}", fenced(ext, &body))
                        }
                        _ => fenced(ext, &body),
                    };
                    match cut {
                        0 => text,
                        n => format!("{text}\n\n… (+{n} lines, o opens)"),
                    }
                }
            }
        }
        Kind::Csv => match std::fs::read(&a.path)
            .map_err(|e| e.to_string())
            .and_then(|b| artifact::kind::csv_table(&b, ext == "tsv", CSV_ROWS))
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
    "usage: /schedules [run <name>|history <name>|pause <name>|resume <name>]";

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

/// `/schedules`: one row per file — `name · when · next · runs · last · result · state`.
/// `runs` is `—` for a plain schedule, `N` or `N/max` for a loop (33 §3.2);
/// `state` is `running`, `off`, `error: …` (then `next` is `—`), or empty.
pub fn schedules_table(rows: &[schedule::ScheduleInfo]) -> String {
    if rows.is_empty() {
        return "no schedules".into();
    }
    let stamp = |t: Option<chrono::DateTime<chrono::Local>>| {
        t.map_or("—".to_owned(), |t| t.format("%a %d %b %H:%M").to_string())
    };
    let mut out = vec!["name · when · next · runs · last · result · state".to_owned()];
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
        format!(
            "{} · {} · {} · {runs} · {} · {} · {state}",
            s.name,
            s.when,
            stamp(next),
            stamp(s.last),
            s.last_result.as_deref().unwrap_or("—"),
        )
    }));
    out.join("\n")
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

fn send(tx: &UnboundedSender<Command>, cmd: Command) {
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

/// One `/agents` picker or board row: `glyph name state detail age` (design
/// doc 19 §3.1). `spinner` is the running glyph; depth ≥ 2 indents the name
/// under its parent; `width` (terminal columns) bounds the detail column.
/// Age is elapsed while the agent is live, `ago` since it finished otherwise.
pub fn agent_row(a: &AgentRow, spinner: &str, width: u16) -> String {
    let glyph = match a.info.state {
        AgentState::Running => spinner,
        AgentState::Awaiting { .. } => "⏳",
        AgentState::Approval { .. } => "⚠",
        AgentState::Error => "✗",
        AgentState::Cancelled => "∅",
        AgentState::Idle => "·",
    };
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
    format!(
        "{glyph} {name:<14} {:<9} {detail:<room$}  {age}",
        a.info.state.word()
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
fn when(rfc3339: &str) -> String {
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
    let s = d.num_seconds().max(0);
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
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
            m.menu_key(KeyCode::Enter, "/res", &tx),
            MenuKey::Input("/resume ".into())
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

    #[test]
    fn search_opens_picker_and_enter_switches_session() {
        use agent::search::{HitKind, MatchRange};
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        m.submit("/search", &tx);
        assert!(matches!(m.entries.last(), Some(Entry::Error(t)) if t == "usage: /search <query>"));
        assert!(rx.try_recv().is_err());
        m.submit("/search deploy", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::Search(q)) if q == "deploy"));
        let hit = SessionId::new();
        m.apply(UiEvent::Search(
            "deploy".into(),
            vec![SearchHit {
                session: hit,
                title: "deploy checklist".into(),
                kind: HitKind::Title,
                seq: None,
                role: None,
                snippet: "deploy checklist".into(),
                ranges: vec![MatchRange { start: 0, end: 6 }],
                score: 1,
                matches: 0,
                updated_at: String::new(),
                context_removed: false,
            }],
        ));
        assert!(
            matches!(&m.menu, Some(Menu::Search { query, hits, cursor: 0 }) if query == "deploy" && hits.len() == 1)
        );
        assert!(!m.input_focused());
        // Enter is a session switch, the `/sessions` path.
        m.menu_key(KeyCode::Enter, "", &tx);
        assert!(m.menu.is_none());
        assert_eq!(m.session, Some(hit));
        assert!(matches!(rx.try_recv(), Ok(Command::Load { session, .. }) if session == hit));
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
        let catalog = Catalog::load(std::env::temp_dir().join("wth-tui-no-user-file").as_path());
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
        let catalog = Catalog::load(std::env::temp_dir().join("wth-tui-no-user-file").as_path());
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
        let catalog = Catalog::load(std::env::temp_dir().join("wth-tui-no-user-file").as_path());
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
        }));
        assert!(m.secret_request().is_none());
        m.apply(UiEvent::SecretsEvent(secrets::SecretsEvent::Requested {
            id,
            session,
            name: "github".into(),
            note: "PAT".into(),
            current: Some(2),
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

        let fired = chrono::Local::now();
        let run = schedule::RunInfo {
            session: None,
            fired_at: fired,
            finished_at: Some(fired),
            result: Some("end_turn".into()),
            report: Some(format!("\n  {}\nsecond line", "x".repeat(70))),
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
        let catalog = Catalog::load(std::env::temp_dir().join("wth-tui-no-user-file").as_path());
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
        m.open_efforts(vec!["low".into(), "high".into()]);
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
        m.open_efforts(Vec::new());
        assert!(m.menu.is_none());
        assert!(
            matches!(m.entries.last(), Some(Entry::Info(t)) if t.ends_with("no reasoning effort levels"))
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
        let catalog = Catalog::load(std::env::temp_dir().join("wth-tui-no-user-file").as_path());
        let mut m = Model::new("nobody/nothing".into(), true);
        assert_eq!(m.header(&catalog).0, "nobody/nothing  ·  ctx 0");
        m.effort = Some("high".into());
        assert_eq!(m.header(&catalog).0, "nobody/nothing  ·  high  ·  ctx 0");
        m.model_id = "anthropic/claude-sonnet-4-5".into();
        assert!(m.header(&catalog).0.contains("  ·  high  ·  ctx "));
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
        m.submit("/nonesuch", &tx);
        assert!(
            matches!(m.entries.last(), Some(Entry::Error(t)) if t == "unknown command /nonesuch")
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
        let path = std::env::temp_dir().join(format!("wth-tui-artifact-{}.md", std::process::id()));
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
        let path = std::env::temp_dir().join(format!("wth-tui-app-{}.openui", std::process::id()));
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
}
