//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use agent::{
    AgentEvent, CompactStyle, HistoryEntry, Loaded, ModelInfo, ProviderInfo, QueuedInput,
    SessionSummary, SkillBody, SkillMeta, Usage,
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
    /// Reply to [`Command::ListChildren`]: the focused root's subagents.
    Children(Vec<SessionSummary>),
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
    /// Reply to [`Command::ListMcp`] (also sent after every [`Command::SetGate`]).
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
    },
    /// `steer`: queued until the current tool batch ends.
    Steer {
        /// Target.
        session: SessionId,
        /// Client-minted id, echoed by the `Input*` events.
        id: InputId,
        /// User text.
        text: String,
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
    /// `list_sessions`.
    ListSessions,
    /// `list_children` of the root session.
    ListChildren(SessionId),
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
    /// `list_mcp`.
    ListMcp,
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

impl Entry {
    /// Rows this entry draws before wrapping (borders and headers included).
    fn height(&self) -> usize {
        match self {
            Entry::User(t) => t.lines().count() + 4,
            Entry::Assistant(t) => {
                let v = t.view();
                v.lines().count() + v.plain.lines().count()
            }
            Entry::Card { text, expanded, .. } => {
                4 + if *expanded {
                    text.view().lines().count()
                } else {
                    0
                }
            }
            Entry::Reasoning { done: true, .. } | Entry::Info(_) | Entry::Error(_) => 1,
            Entry::Reasoning { text, .. } => text.lines().count(),
            Entry::Pending { .. } => 2,
            Entry::Tool { args, expanded, .. } => {
                5 + args.len() + expanded.as_ref().map_or(0, |t| t.view().lines().count())
            }
            Entry::Compacted {
                summary, expanded, ..
            } => {
                4 + match summary {
                    Some(t) if *expanded => t.view().lines().count(),
                    _ => 0,
                }
            }
        }
    }
}

/// What the input box shows for a held paste: `[⎘ paste 1: 69 lines]`
/// (`chars` for a single long line). `i` is the index into `Model::pastes`.
pub fn paste_token(i: usize, text: &str) -> String {
    let lines = text.lines().count();
    let size = if lines > 1 {
        format!("{lines} lines")
    } else {
        format!("{} chars", text.chars().count())
    };
    format!("[⎘ paste {}: {size}]", i + 1)
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

/// A subagent of the root session, known from `SubagentSpawned` or `/agents`.
#[derive(Debug)]
pub struct Child {
    /// Peer name.
    pub name: String,
    /// Its session.
    pub id: SessionId,
    /// A turn is in flight.
    pub running: bool,
}

/// Slash commands: name, argument hint (`<x>` required, `[x]` optional, `""` none), description.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("/model", "[id]", "switch model (picker without id)"),
    (
        "/effort",
        "[level|clear]",
        "reasoning effort (picker without arg)",
    ),
    (
        "/mcp",
        "[add <url|json>|sync]",
        "MCP servers and tool gates",
    ),
    (
        "/skills",
        "[add <url>|rm <name>|sync]",
        "skills (picker without args)",
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
    ("/agents", "", "list this session's subagents"),
    (
        "/jobs",
        "[kill <id>]",
        "list this session's background jobs",
    ),
    ("/focus", "<name>", "show a subagent's transcript"),
    ("/sessions", "", "switch session (picker)"),
    ("/resume", "<id>", "open a session"),
    ("/new", "", "start a new session"),
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
    /// `/mcp` gates.
    Mcp {
        /// Latest `ListServers` reply.
        servers: Vec<McpServer>,
        /// Highlighted row of [`Model::mcp_rows`].
        cursor: usize,
        /// Servers whose tools are listed.
        expanded: BTreeSet<String>,
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

/// Space in `/mcp`: annotations → read_only → mutating → destructive → off → annotations (12 §5).
fn next_gate(g: Option<Gate>) -> Option<Gate> {
    match g {
        None => Some(Gate::ReadOnly),
        Some(Gate::ReadOnly) => Some(Gate::Mutating),
        Some(Gate::Mutating) => Some(Gate::Destructive),
        Some(Gate::Destructive) => Some(Gate::Off),
        Some(Gate::Off) => None,
    }
}

/// UI state.
#[derive(Debug, Default)]
pub struct Model {
    /// Root session (the one the human started or resumed).
    pub session: Option<SessionId>,
    /// Child whose transcript is shown instead of the root's (`/focus`).
    pub focus: Option<SessionId>,
    /// Subagents under the root, in spawn order.
    pub children: Vec<Child>,
    /// Multi-step `/provider add`; `None` when the input is a normal line.
    pub prompt: Option<Prompt>,
    /// A `Send { session: None }` is in flight; adopt the next `TurnStarted`.
    awaiting_session: bool,
    /// `/mcp` was typed; the next `UiEvent::Mcp` opens the menu. Refreshes
    /// after `SetGate` only update an open menu, never reopen a closed one.
    awaiting_mcp: bool,
    /// Transcript of the shown session.
    pub entries: Vec<Entry>,
    /// The root has a turn in flight.
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
    /// Pending approvals for the root or any child, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// `tools.approvals` (global); false = never prompts.
    pub approvals_global: bool,
    /// The root session opted out of prompts (`/approvals off`).
    pub approvals_off: bool,
    /// Latest MCP reconcile snapshot; `None` before the first.
    pub mcp: Option<McpProgress>,
    /// Large pastes held out of the input box; the box shows one
    /// [`paste_token`] each, expanded back on submit.
    pub pastes: Vec<String>,
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

    /// `Self::new` keeping the current model, effort and catalog: `/new` and `/resume`.
    fn reset(&mut self) {
        *self = Self {
            effort: self.effort.take(),
            catalog: self.catalog.take(),
            ..Self::new(std::mem::take(&mut self.model_id), self.approvals_global)
        };
    }

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The session whose transcript is shown: the focused child, else the root.
    pub fn shown(&self) -> Option<SessionId> {
        self.focus.or(self.session)
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
                        | Menu::Mcp { .. }
                        | Menu::Skills { .. }
                        | Menu::Logins { .. }
                )
            )
    }

    /// Index of the first entry worth drawing: the tail whose height (before
    /// wrapping, so a lower bound) reaches `lines`. Every frame lays out and
    /// paints every child it is given, so the whole history made each frame
    /// O(session); the screen can only ever show the tail anyway.
    // ponytail: scroll-back stops at this window; render on demand from the
    // scroll offset if anyone wants to page further up.
    pub fn tail(&self, lines: usize) -> usize {
        let mut total = 0;
        for (i, e) in self.entries.iter().enumerate().rev() {
            total += e.height();
            if total >= lines {
                return i;
            }
        }
        0
    }

    /// Take a paste: small ones come back verbatim for the input box; large
    /// ones are held in `pastes` and a [`paste_token`] comes back instead.
    /// Terminals paste newlines as `\r`; normalise to `\n`.
    pub fn paste(&mut self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.lines().count() <= 5 && text.chars().count() <= 400 {
            return text;
        }
        let token = paste_token(self.pastes.len(), &text);
        self.pastes.push(text);
        token
    }

    /// `line` with every paste token the user left in place replaced by its
    /// text; `pastes` is emptied either way.
    fn expand_pastes(&mut self, line: &str) -> String {
        let mut text = line.to_owned();
        for (i, p) in std::mem::take(&mut self.pastes).iter().enumerate() {
            text = text.replace(&paste_token(i, p), p);
        }
        text
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

    /// Whether the shown session has a turn in flight.
    fn shown_running(&self) -> bool {
        match self.focus {
            Some(id) => self.child(id).is_some_and(|c| c.running),
            None => self.streaming,
        }
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

    /// Some assistant text is still typing out.
    pub fn revealing(&self) -> bool {
        self.entries
            .iter()
            .rev()
            .any(|e| matches!(e, Entry::Assistant(t) if t.unrevealed() > 0))
    }

    /// Frames are needed: text is typing out, a turn is in flight, MCP
    /// servers are connecting, or a compaction runs.
    pub fn animating(&self) -> bool {
        self.revealing()
            || self.shown_running()
            || self.mcp_connecting()
            || self.compacting
            || self.harvesting
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
        if !self.shown_running() {
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

    fn child(&self, id: SessionId) -> Option<&Child> {
        self.children.iter().find(|c| c.id == id)
    }

    /// Peer name of a child session; `None` for the root or strangers.
    pub fn peer_name(&self, id: SessionId) -> Option<&str> {
        self.child(id).map(|c| c.name.as_str())
    }

    /// Fold one event into the state.
    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Agent(ev) => self.apply_agent(ev),
            UiEvent::Approval(req) => {
                if self.session == Some(req.session) || self.child(req.session).is_some() {
                    self.approvals.push_back(req);
                }
            }
            UiEvent::Resolved(r) => self.approvals.retain(|a| a.id != r.id),
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
            UiEvent::Children(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("no subagents".into()));
                }
                for s in list {
                    let name = s.name.unwrap_or_default();
                    if self.child(s.id).is_none() {
                        self.children.push(Child {
                            name: name.clone(),
                            id: s.id,
                            running: false,
                        });
                    }
                    let status = if self.child(s.id).is_some_and(|c| c.running) {
                        "running"
                    } else {
                        "idle"
                    };
                    self.entries
                        .push(Entry::Info(format!("{name}  {status}  {}", short(s.id))));
                }
            }
            UiEvent::History(id, loaded) => {
                if self.shown() == Some(id) {
                    self.entries = loaded.history.into_iter().flat_map(from_history).collect();
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
                    if self.session == Some(id) {
                        self.streaming = loaded.running;
                    } else if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                        c.running = loaded.running;
                    }
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
                    });
                }
                _ => {}
            },
            UiEvent::Approvals(s) => {
                self.approvals_global = s.global;
                self.entries.push(Entry::Info(format!(
                    "approvals: {} (this session), {} (default)",
                    on_off(s.session && !self.approvals_off),
                    on_off(s.global)
                )));
            }
            UiEvent::McpProgress(p) => self.mcp = Some(p),
            // Verbatim: the point is to see exactly what the model sees.
            UiEvent::Prompt(text) => self.entries.push(card(
                CardKind::Prompt,
                "prompt".into(),
                &format!("```\n{text}\n```"),
            )),
            UiEvent::Skills(items) => {
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
                JobEvent::Started { session, id, .. } if self.shown() == Some(session) => {
                    self.jobs.insert(id.0);
                }
                JobEvent::Finished { session, id, .. } if self.shown() == Some(session) => {
                    self.jobs.remove(&id.0);
                }
                _ => {}
            },
            UiEvent::CompactStyle(s) => self.entries.push(Entry::Info(format!(
                "compact style: {s} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
            ))),
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
        // Roster bookkeeping, independent of what is shown.
        match &ev {
            AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
            } => {
                if self.session == Some(*parent) || self.child(*parent).is_some() {
                    self.children.push(Child {
                        name: name.clone(),
                        id: *child,
                        running: false,
                    });
                }
                if self.shown() == Some(*parent) {
                    self.interject(Entry::Info(format!("↳ spawned {name}")));
                }
                return;
            }
            AgentEvent::MessageDelivered {
                session,
                from,
                text,
            } => {
                if self.shown() == Some(*session) {
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
                if self.shown() == Some(*session) && !self.queued.iter().any(|q| q.id == *id) {
                    self.queued.push(QueuedInput {
                        id: *id,
                        text: text.clone(),
                    });
                }
                return;
            }
            AgentEvent::InputWithdrawn { session, id } => {
                if self.shown() == Some(*session) {
                    self.queued.retain(|q| q.id != *id);
                }
                return;
            }
            AgentEvent::TurnStarted { session, .. } | AgentEvent::TurnFinished { session, .. } => {
                let running = matches!(ev, AgentEvent::TurnStarted { .. });
                if self.session == Some(*session) {
                    self.streaming = running;
                } else if let Some(c) = self.children.iter_mut().find(|c| c.id == *session) {
                    c.running = running;
                }
            }
            _ => {}
        }
        if ev.session().is_none_or(|s| self.shown() != Some(s)) {
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
                    self.entries.push(Entry::User(text));
                }
            }
            // The steer enters the context now: its card moves from the queue
            // to the transcript, after everything the model said before it.
            AgentEvent::InputInjected { id, turn, .. } => {
                if let Some(at) = self.queued.iter().position(|q| q.id == id) {
                    self.entries.push(Entry::User(self.queued.remove(at).text));
                }
                self.last_injected_turn = Some(turn);
            }
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
                self.interject(Entry::Compacted {
                    epoch,
                    style,
                    summary: None,
                    expanded: false,
                });
            }
            AgentEvent::Error { message, .. } => {
                self.compacting = false;
                self.harvesting = false;
                self.entries.push(Entry::Error(message));
            }
            AgentEvent::SubagentSpawned { .. }
            | AgentEvent::MessageDelivered { .. }
            | AgentEvent::InputQueued { .. }
            | AgentEvent::InputWithdrawn { .. } => {}
            AgentEvent::Harvesting { .. } => self.harvesting = true,
            AgentEvent::Harvested { added, updated, .. } => {
                self.harvesting = false;
                self.interject(Entry::Info(format!("knowledge +{added} ~{updated}")))
            }
        }
    }

    /// Handle a submitted input line: slash command or message.
    pub fn submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
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
                send(tx, Command::ListMcp);
            }
            "/mcp" => self.mcp_cmd(arg, tx),
            "/skills" => self.skills_cmd(arg, tx),
            "/prompt" => send(tx, Command::ShowPrompt(self.shown())),
            "/knowledge" => self.knowledge_cmd(arg, tx),
            "/login" if arg.is_empty() => send(tx, Command::ListLogins),
            "/login" => send(tx, Command::Login(arg.to_owned())),
            "/logout" if arg.is_empty() => self
                .entries
                .push(Entry::Error("usage: /logout <name>".into())),
            "/logout" => send(tx, Command::Logout(arg.to_owned())),
            "/agents" => match self.session {
                Some(root) => send(tx, Command::ListChildren(root)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/jobs" => match (self.shown(), arg.split_once(' ')) {
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
            "/focus" => self.focus(arg, tx),
            "/compact" => self.compact_cmd(arg, tx),
            "/cancel" => {
                if let Some(session) = self.shown() {
                    send(tx, Command::Cancel(session));
                }
            }
            "/undo" | "/redo" => match self.shown() {
                Some(session) if cmd == "/undo" => send(tx, Command::Undo(session)),
                Some(session) => send(tx, Command::Redo(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/provider" => self.provider_cmd(arg, tx),
            c if c.starts_with('/') => self
                .entries
                .push(Entry::Error(format!("unknown command {c}"))),
            _ => {
                let text = self.expand_pastes(line);
                match self.shown() {
                    // Mid-turn: the card waits below the transcript until
                    // `InputInjected` says where in the conversation it landed.
                    Some(session) if self.shown_running() => {
                        let id = InputId::new();
                        self.queued.push(QueuedInput {
                            id,
                            text: text.clone(),
                        });
                        send(tx, Command::Steer { session, id, text });
                    }
                    session => {
                        self.entries.push(Entry::User(line.to_owned()));
                        self.sent = Some(text.clone());
                        self.awaiting_session = session.is_none();
                        send(tx, Command::Send { session, text });
                    }
                }
            }
        }
    }

    /// Switch to `id` and ask for its history.
    // ponytail: children spawned before a resume are unknown until `/agents`
    // runs (their approvals are hidden until then); fetch ListChildren here
    // silently if that bites.
    pub fn resume(&mut self, id: SessionId, tx: &UnboundedSender<Command>) {
        self.reset();
        self.session = Some(id);
        send(tx, Command::Load(id));
    }

    /// `/focus <name>` shows that child's transcript; `/focus` (or Esc)
    /// returns to the root. Either way the transcript is reloaded.
    pub fn focus(&mut self, name: &str, tx: &UnboundedSender<Command>) {
        let Some(root) = self.session else { return };
        let target = if name.is_empty() {
            root
        } else {
            match self.children.iter().find(|c| c.name == name) {
                Some(c) => c.id,
                None => {
                    self.entries
                        .push(Entry::Error(format!("no subagent named {name:?}")));
                    return;
                }
            }
        };
        self.focus = (target != root).then_some(target);
        self.entries.clear();
        self.jobs.clear();
        self.usage = None;
        // `Loaded` carries the target's model and effort; the header flips on its reply.
        self.context = 0;
        send(tx, Command::Load(target));
    }

    /// Cancel the shown session's in-flight turn, if any.
    pub fn cancel(&self, tx: &UnboundedSender<Command>) {
        if let (Some(session), true) = (self.shown(), self.shown_running()) {
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
        if let Some(session) = self.shown() {
            send(tx, Command::Withdraw { session, id: q.id });
        }
    }

    /// Answer the shown approval and dismiss it locally; `ApprovalResolved`
    /// (or a `Resolve` failure) is then a no-op for this id.
    pub fn decide(&mut self, decision: Decision, tx: &UnboundedSender<Command>) {
        if let Some(req) = self.approvals.pop_front() {
            send(
                tx,
                Command::Resolve {
                    id: req.id,
                    decision,
                },
            );
        }
    }

    /// One-line status: session, streaming state, last usage, focus, roster, queue.
    pub fn status(&self) -> String {
        let session = self.shown().map_or_else(|| "no session".to_owned(), short);
        let state = self
            .activity()
            .map_or_else(|| "idle".to_owned(), |a| format!("{} {a}", self.spinner()));
        let usage = self.usage.map_or_else(String::new, |u| {
            format!("  {}↑ {}↓", u.prompt, u.completion)
        });
        let focus = self
            .focus
            .and_then(|id| self.peer_name(id))
            .map_or_else(String::new, |n| format!("  ↳ {n}"));
        let running = self.children.iter().filter(|c| c.running).count();
        let agents = if running > 0 {
            format!("  agents: {running} running")
        } else {
            String::new()
        };
        let queued = match self.queued.len() {
            0 => String::new(),
            n => format!("  ·  {n} queued"),
        };
        let jobs = match self.jobs.len() {
            0 => String::new(),
            n => format!("  ·  {n} job(s)"),
        };
        format!("{session}  {state}{usage}{focus}{agents}{queued}{jobs}")
    }

    /// Line above the input: `model  ·  high  ·  ctx 12.3k / 200k (6%)  ·  cache 94%`
    /// and its colour level. The effort segment is omitted while unset;
    /// `cache` is the prefix-cache hit ratio of the last finished turn (design
    /// doc 07 §7), omitted until one has reported usage.
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
                        "{}{effort}  ·  ctx {used} / {} ({pct}%){cache}{}",
                        self.model_id,
                        k(limit),
                        self.approvals_tag()
                    ),
                    level,
                )
            }
            _ => (
                format!(
                    "{}{effort}  ·  ctx {used}{cache}{}",
                    self.model_id,
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
            ("", _) => send(tx, Command::ListSkills),
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
            ("harvest", _) => match self.shown() {
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
            ("", _) => match self.shown() {
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
                match self.shown() {
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
            Some(Menu::Mcp { .. }) => self.mcp_key(code, tx),
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

    /// `/skills`: Enter shows the highlighted skill's body as a card.
    fn skills_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Skills { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                if let Some(s) = items.get(*cursor) {
                    send(tx, Command::LoadSkill(s.name.clone()));
                }
                self.menu = None;
            }
            _ => {}
        }
        MenuKey::Consumed
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
            Close,
            Nothing,
        }
        let Some(Menu::Mcp { cursor, .. }) = &self.menu else {
            return MenuKey::Consumed;
        };
        let rows = self.mcp_rows();
        let cursor = (*cursor).min(rows.len().saturating_sub(1));
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(cursor.saturating_sub(1)),
            (KeyCode::Down, _) => Act::Move((cursor + 1).min(rows.len().saturating_sub(1))),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('s'), _) => Act::Send(Command::SyncMcp),
            (KeyCode::Right | KeyCode::Enter, Some(Row::Server(s))) => {
                Act::Expand(s.name.clone(), true)
            }
            (KeyCode::Left, Some(Row::Server(s) | Row::Tool(s, _))) => {
                Act::Expand(s.name.clone(), false)
            }
            (KeyCode::Char('l'), Some(Row::Server(s))) => match s.auth {
                McpAuth::Required => Act::Send(Command::Login(s.name.clone())),
                McpAuth::LoggedIn => Act::Send(Command::Logout(s.name.clone())),
                McpAuth::None => Act::Nothing,
            },
            (KeyCode::Char(' '), Some(Row::Server(s))) => Act::Send(Command::SetGate {
                server: s.name.clone(),
                tool: None,
                gate: next_gate(s.gate),
            }),
            (KeyCode::Char(' '), Some(Row::Tool(s, t))) => Act::Send(Command::SetGate {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
                gate: next_gate(t.gate),
            }),
            (KeyCode::Backspace, Some(Row::Tool(s, t))) => Act::Send(Command::SetGate {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
                gate: None,
            }),
            _ => Act::Nothing,
        };
        let Some(Menu::Mcp {
            servers,
            cursor: cur,
            expanded,
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

/// Transcript entries for one history record: an injected user turn holding
/// several peer messages becomes one card each.
fn from_history(h: HistoryEntry) -> Vec<Entry> {
    match h {
        HistoryEntry::User { text } => match injected(&text) {
            None => vec![Entry::User(text)],
            Some(cards) => cards
                .into_iter()
                .map(|(kind, title, body)| card(kind, title, &body))
                .collect(),
        },
        HistoryEntry::Assistant { text } => {
            vec![Entry::Assistant(Box::new(markdown::Text::complete(&text)))]
        }
        HistoryEntry::Tool {
            name,
            args,
            output,
            is_error,
        } => vec![Entry::Tool {
            call_id: String::new(),
            args: describe(&name, &args),
            name,
            result: Some((output, is_error)),
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
            m.menu_key(KeyCode::Enter, "/foc", &tx),
            MenuKey::Input("/focus ".into())
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
    fn mcp_space_cycles_server_and_tool_gates() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let fresh = |gate: Option<Gate>| McpServer {
            name: "fs".into(),
            connected: true,
            repo: None,
            auth: McpAuth::None,
            gate,
            tools: vec![McpToolInfo {
                name: "read".into(),
                description: String::new(),
                gate: None,
                risk: tools::Risk::Mutating,
                action: PolicyAction::Ask,
                registered: true,
            }],
        };
        // A refresh with no menu open and no `/mcp` typed must not open one.
        m.apply(UiEvent::Mcp(vec![fresh(None)]));
        assert!(m.menu.is_none());
        m.submit("/mcp", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListMcp)));
        m.apply(UiEvent::Mcp(vec![fresh(Some(Gate::Destructive))]));
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { server, tool: None, gate: Some(Gate::Off) }) if server == "fs"
        ));
        m.apply(UiEvent::Mcp(vec![fresh(Some(Gate::Off))]));
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(Command::SetGate {
                    tool: None,
                    gate: None,
                    ..
                })
            ),
            "off cycles back to annotations"
        );
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
        m.menu_key(KeyCode::Left, "", &tx);
        assert!(matches!(m.menu, Some(Menu::Mcp { cursor: 0, .. })));
        assert_eq!(m.mcp_rows().len(), 1);
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
    fn tail_reaches_the_requested_height() {
        let mut m = Model::default();
        for _ in 0..10 {
            m.entries.push(Entry::Info("x".into()));
            m.entries.push(Entry::User("a\nb".into()));
        }
        assert_eq!(m.tail(1), 19);
        assert_eq!(m.tail(7), 18, "Info 1 + User 6 rows");
        assert_eq!(m.tail(1000), 0, "short histories draw whole");
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
