//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use acp::EntryInfo;
use acp::registry::RegistryAgent;
use agent::{
    AgentEvent, AgentInfo, AgentState, CompactStyle, HistoryEntry, Loaded, ModelInfo, ProviderInfo,
    QueuedInput, SessionSummary, SkillBody, SkillMeta, Usage,
};
use iocraft::KeyCode;
use knowledge::{Change, HarvestResult, History, Meta};
use models::Catalog;
use runtime::{InputId, SessionId, TurnId};
use tokio::sync::mpsc::UnboundedSender;
use tools::git::SyncResult;
use tools::{
    ApprovalId, ApprovalRequested, ApprovalResolved, ApprovalsState, Decision, Gate, JobEvent,
    JobView, McpAuth, McpProgress, McpServer, McpToolInfo, PolicyAction, RepoInfo,
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
    /// Reply to [`Command::ListSessions`].
    Sessions(Vec<SessionSummary>),
    /// Reply to [`Command::ListAgents`]: the session's subtree with live state.
    Agents(Vec<AgentInfo>),
    /// Reply to [`Command::Load`].
    History(SessionId, Loaded),
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
    /// Reply to [`Command::KnowledgeHistory`].
    KnowledgeHistory(History),
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
    /// `list_sessions`.
    ListSessions,
    /// `list_agents`: the subtree under a session.
    ListAgents(SessionId),
    /// `load_session`.
    Load(SessionId),
    /// `resolve_approval`.
    Resolve {
        /// Request.
        id: ApprovalId,
        /// Answer.
        decision: Decision,
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
    /// `knowledge_history`; `None` for the whole bundle.
    KnowledgeHistory(Option<String>),
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
/// [`JOB_LINES`] lines, frozen with how it ended. Bounded, so any number of
/// jobs costs the same; the log file is the whole story.
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
    User(String),
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
}

impl CardKind {
    /// Header glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            CardKind::Peer => "⟵",
            CardKind::Skill => "◇",
            CardKind::Prompt => "§",
            CardKind::Job => "⚙",
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
            Entry::User(t) => text_rows(t, inner) + 4,
            Entry::Assistant(t) => md(t, width),
            Entry::Card { text, expanded, .. } => 4 + if *expanded { md(text, inner) } else { 0 },
            Entry::Reasoning { done: true, .. } | Entry::Info(_) | Entry::Error(_) => 1,
            Entry::Reasoning { text, .. } => text_rows(text, width),
            Entry::Pending { .. } => 2,
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
                (
                    CardKind::Job,
                    format!("{from} · {command} · exit {exit} · {secs}s"),
                )
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
        "[query | history [id] | restore <id> [commit] | harvest]",
        "knowledge base: list, search, history, restore, harvest",
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
    (
        "/jobs",
        "[kill <id>]",
        "list this session's background jobs",
    ),
    ("/parent", "", "open the parent session (from a subagent)"),
    ("/sessions", "", "switch session (picker)"),
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
    ("/undo", "", "drop the last turn (redo restores)"),
    ("/redo", "", "restore the last undone turn"),
    ("/quit", "", "exit"),
    (
        "/provider",
        "[add|rm <name>]",
        "OpenAI-compatible providers (add prompts for name, url, key)",
    ),
    ("/exit", "", "same as /quit"),
];

/// Commands whose name starts with the typed word (`/mo` → `/model`).
pub fn slash_matches(word: &str) -> Vec<&'static (&'static str, &'static str, &'static str)> {
    COMMANDS.iter().filter(|c| c.0.starts_with(word)).collect()
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
    /// `/sessions` picker.
    Sessions {
        /// Top-level sessions, newest first.
        items: Vec<SessionSummary>,
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
}

/// Multi-step `/provider add` input. Rides on the input box (not a modal menu).
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
        }
    }
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
    /// `tools.approvals` (global); false = never prompts.
    pub approvals_global: bool,
    /// The root session opted out of prompts (`/approvals off`).
    pub approvals_off: bool,
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
    /// Text the editor takes on the next render: a withdrawn steer.
    pub recall: Option<String>,
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
            ..Self::new(std::mem::take(&mut self.model_id), self.approvals_global)
        };
    }

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The input box takes keys: no approval prompt and no modal menu
    /// (the slash popup rides on the box).
    pub fn input_focused(&self) -> bool {
        self.approval().is_none()
            && !matches!(
                self.menu,
                Some(
                    Menu::Models { .. }
                        | Menu::Efforts { .. }
                        | Menu::Sessions { .. }
                        | Menu::Agents { .. }
                        | Menu::Mcp { .. }
                        | Menu::Acp { .. }
                        | Menu::Skills { .. }
                        | Menu::Logins { .. }
                )
            )
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
    pub fn toggle_card(&mut self) {
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
                name, result: None, ..
            } => Some(format!("running {name}…")),
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
                    self.entries = loaded.history.into_iter().flat_map(from_history).collect();
                    self.anchor = None;
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
                }
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
            UiEvent::Knowledge(metas) => {
                if metas.is_empty() {
                    self.entries.push(Entry::Info("no concepts".into()));
                } else {
                    let lines: Vec<String> = metas.iter().map(meta_line).collect();
                    self.entries.push(card(
                        CardKind::Skill,
                        format!("knowledge: {} concepts", metas.len()),
                        &lines.join("\n"),
                    ));
                }
            }
            UiEvent::KnowledgeHistory(h) => {
                if h.changes.is_empty() {
                    self.entries.push(Entry::Info("no history".into()));
                } else {
                    let mut text: String = h
                        .changes
                        .iter()
                        .map(|c| format!("{}  {}  {}\n", c.commit, c.time, c.message))
                        .collect();
                    if let Some(diff) = &h.diff {
                        text.push_str(&format!("\n```diff\n{diff}\n```"));
                    }
                    self.entries
                        .push(card(CardKind::Skill, "knowledge history".into(), &text));
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
            UiEvent::Error(e) => {
                // A failed `/compact` answers here, not with an agent event.
                self.compacting = false;
                self.awaiting_effort = false;
                self.notice = None;
                self.entries.push(Entry::Error(e));
            }
        }
    }

    fn apply_agent(&mut self, ev: AgentEvent) {
        if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
            self.session = ev.session();
            self.awaiting_session = false;
        }
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
            // injected it (`MessageDelivered` already shows those).
            AgentEvent::TurnStarted {
                turn, text, model, ..
            } => {
                self.model_id = model;
                let shown = injected(&text).is_some()
                    || self.last_injected_turn.take() == Some(turn)
                    || self.sent.take().as_deref() == Some(&*text);
                if !shown {
                    self.entries.extend(user_entries(text));
                }
            }
            // The steer enters the context now: its card moves from the queue
            // to the transcript, after everything the model said before it.
            AgentEvent::InputInjected { id, turn, .. } => {
                if let Some(at) = self.queued.iter().position(|q| q.id == id) {
                    self.entries
                        .extend(user_entries(self.queued.remove(at).text));
                    self.last_injected_turn = Some(turn);
                }
            }
            // `InputQueued` is handled above, before the streaming-tail bookkeeping.
            AgentEvent::InputQueued { .. } => {}
            AgentEvent::TextDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) if !t.is_done() => t.push(&text),
                _ => {
                    let mut t = markdown::Text::new();
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
                self.entries.push(Entry::Pending { call_id, name });
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
                    Some(Entry::Tool { result, .. }) => *result = Some((output, is_error)),
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
            AgentEvent::Compacted { epoch, style, .. } => {
                self.compacting = false;
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
            | AgentEvent::InputWithdrawn { .. } => {}
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
        if line.is_empty() {
            return;
        }
        // Anything you send is what you want to see land.
        self.anchor = None;
        if self.prompt.is_some() {
            self.prompt_submit(line, tx);
            return;
        }
        let (cmd, arg) = line
            .split_once(' ')
            .map_or((line, ""), |(c, a)| (c, a.trim()));
        match cmd {
            "/quit" | "/exit" => self.quit = true,
            "/sessions" => send(tx, Command::ListSessions),
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
            "/compact" => self.compact_cmd(arg, tx),
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
            "/provider" => self.provider_cmd(arg, tx),
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
                        self.entries.push(Entry::User(line.to_owned()));
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
        self.reset();
        self.approvals = approvals;
        self.session = Some(id);
        send(tx, Command::Load(id));
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
        format!("{session}  {state}{usage}{agents}{queued}{jobs}{skills}{scrolled}")
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
                        "{model}{effort}  ·  ctx {used} / {} ({pct}%){cache}{}",
                        k(limit),
                        self.approvals_tag()
                    ),
                    level,
                )
            }
            _ => (
                format!(
                    "{model}{effort}  ·  ctx {used}{cache}{}",
                    self.approvals_tag()
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

    /// `/knowledge [query | history [id] | restore <id> [commit] | harvest]`.
    fn knowledge_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => send(tx, Command::KnowledgeList),
            ("history", id) => send(
                tx,
                Command::KnowledgeHistory((!id.is_empty()).then(|| id.to_owned())),
            ),
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

    /// Advance `/provider add` one field. The API key is never written to the transcript.
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
            None => {}
        }
    }

    /// Drop an in-progress `/provider add`.
    pub fn cancel_prompt(&mut self) {
        if self.prompt.take().is_some() {
            self.entries
                .push(Entry::Info("provider add cancelled".into()));
        }
    }

    /// Open or close the slash popup to match what the input box holds.
    pub fn sync_slash(&mut self, input: &str) {
        if self.prompt.is_some() {
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
            Some(Menu::Sessions { .. }) => self.sessions_key(code, tx),
            Some(Menu::Agents { .. }) => self.agents_key(code, tx),
            Some(Menu::Mcp { .. }) => self.mcp_key(code, tx),
            Some(Menu::Acp { .. }) => self.acp_key(code, tx),
            Some(Menu::Skills { .. }) => self.skills_key(code, tx),
            Some(Menu::Logins { .. }) => self.logins_key(code, tx),
        }
    }

    fn slash_key(&mut self, code: KeyCode, input: &str, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Slash { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        let (word, arg) = input
            .split_once(' ')
            .map_or((input, ""), |(w, a)| (w, a.trim()));
        let matches = slash_matches(word);
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
/// several peer messages becomes one card each; anything else is `you`.
fn user_entries(text: String) -> Vec<Entry> {
    if let Some((name, body, rest)) = agent::skills::injected(&text) {
        let mut out = vec![card(CardKind::Skill, name.to_owned(), body)];
        if !rest.is_empty() {
            out.push(Entry::User(rest.to_owned()));
        }
        return out;
    }
    match injected(&text) {
        None => vec![Entry::User(text)],
        Some(cards) => cards
            .into_iter()
            .map(|(kind, title, body)| card(kind, title, &body))
            .collect(),
    }
}

/// Transcript entries for one history record.
fn from_history(h: HistoryEntry) -> Vec<Entry> {
    match h {
        HistoryEntry::User { text } => user_entries(text),
        HistoryEntry::Assistant { text } => {
            vec![Entry::Assistant(Box::new(markdown::Text::complete(&text)))]
        }
        HistoryEntry::Tool {
            call_id,
            name,
            args,
            result,
        } => vec![Entry::Tool {
            args: describe(&name, &args),
            // `.` marks a call from inside a `python` cell (08 §4.1), as live.
            name: if call_id.contains('.') {
                format!("↳ {name}")
            } else {
                name
            },
            call_id,
            job: None,
            result,
            expanded: None,
        }],
        HistoryEntry::Compacted {
            epoch,
            summary,
            style,
        } => vec![Entry::Compacted {
            epoch,
            style,
            summary: Some(Box::new(markdown::Text::complete(&summary))),
            expanded: false,
        }],
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// One `/knowledge` row: `` `id` — title [type]: description `` (10 §6).
pub fn meta_line(m: &Meta) -> String {
    format!("`{}` — {} [{}]: {}", m.id, m.title, m.type_, m.description)
}

/// The `/title` reply: the model naming sessions, or the fallback.
pub fn title_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!("title model: {} ({})", m.id, m.name),
        None => "title model: off — each session names itself with its own model (/title <model> to set a cheap one)".into(),
    }
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
        assert_eq!(slash_matches("/").len(), COMMANDS.len());
        assert!(slash_matches("/zzz").is_empty());
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
        assert!(matches!(rx.try_recv(), Ok(Command::Load(s)) if s == run));
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
        assert!(matches!(rx.try_recv(), Ok(Command::Load(s)) if s == root));
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
            Entry::Info(t) | Entry::Error(t) | Entry::User(t) => t.contains("sk-secret"),
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
        assert!(
            matches!(rx.try_recv(), Ok(Command::KnowledgeHistory(Some(id))) if id == "facts/x")
        );
        m.submit("/knowledge history", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::KnowledgeHistory(None))));
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
        assert!(matches!(&m.entries[0], Entry::User(t) if t.contains(&token)));
        // The agent echoes the expanded text; the token card already shows it.
        let session = SessionId::new();
        m.apply_agent(AgentEvent::TurnStarted {
            session,
            turn: runtime::TurnId::new(),
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
        assert!(matches!(&m.entries[0], Entry::User(t) if t.contains(&t1)));
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
            m.entries.push(Entry::User("a\nb".into()));
        }
        assert_eq!(m.tail(1, 80), (19, 0));
        assert_eq!(m.tail(7, 80), (18, 0), "Info 1 + User 6 rows");
        assert_eq!(m.tail(1000, 80), (0, 0), "short histories draw whole");
        // A long paragraph wraps: at 20 columns it fills the window alone.
        m.entries.push(Entry::User("x".repeat(200)));
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
            m.entries.push(Entry::User(format!("{i}\n{i}")));
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
        m.entries.push(Entry::User("new".into()));
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
            !m.entries.iter().any(|e| matches!(e, Entry::User(_))),
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
            matches!(m.entries.last(), Some(Entry::User(t)) if t == "also do X"),
            "the card lands after the streamed text"
        );
        let before = m.entries.len();
        m.apply_agent(AgentEvent::TurnStarted {
            session: s,
            turn,
            text: "also do X".into(),
            model: "m".into(),
        });
        assert_eq!(
            m.entries.len(),
            before,
            "the turn echo is not a second card"
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
            text,
            model: "m".into(),
        });
        let n = m.entries.len();
        assert!(
            matches!(&m.entries[n - 2], Entry::Card { kind: CardKind::Skill, title, .. } if title == "caveman")
        );
        assert!(matches!(&m.entries[n - 1], Entry::User(t) if t == "lite"));

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
}
