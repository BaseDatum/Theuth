//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, VecDeque};
use std::ops::Range;

use agent::{AgentEvent, HistoryEntry, ModelInfo, SessionSummary, Usage};
use iocraft::KeyCode;
use models::Catalog;
use runtime::SessionId;
use tokio::sync::mpsc::UnboundedSender;
use tools::{
    ApprovalId, ApprovalRequested, ApprovalResolved, ApprovalsState, Decision, McpServer,
    McpToolInfo, PolicyAction,
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
    History(SessionId, Vec<HistoryEntry>),
    /// Reply to [`Command::Send`]: the session the turn went to.
    SessionCreated(SessionId),
    /// Reply to [`Command::ListModels`].
    Models(Vec<ModelInfo>),
    /// Reply to [`Command::SetModel`].
    ModelSet(ModelInfo),
    /// Reply to [`Command::ListMcp`] (also sent after every [`Command::SetGate`]).
    Mcp(Vec<McpServer>),
    /// Reply to [`Command::SetApprovals`].
    Approvals(ApprovalsState),
    /// A command failed (actor missing, ask error).
    Error(String),
}

/// Everything the UI can ask of the rest of the process. One task owns all
/// actor traffic and turns replies into [`UiEvent`]s.
#[derive(Debug)]
pub enum Command {
    /// `SendInput`.
    Send {
        /// `None` creates a session.
        session: Option<SessionId>,
        /// User text.
        text: String,
    },
    /// `Steer`.
    Steer {
        /// Target.
        session: SessionId,
        /// User text.
        text: String,
    },
    /// `Cancel`.
    Cancel(SessionId),
    /// `ListSessions`.
    ListSessions,
    /// `ListChildren` of the root session.
    ListChildren(SessionId),
    /// `LoadSession`.
    Load(SessionId),
    /// `Resolve` on the approval actor.
    Resolve {
        /// Request.
        id: ApprovalId,
        /// Answer.
        decision: Decision,
    },
    /// `ListModels` on the agent actor.
    ListModels,
    /// `SetModel` on the agent actor.
    SetModel {
        /// Session to switch now (`None`: only the default).
        session: Option<SessionId>,
        /// `provider/model` id.
        model: String,
    },
    /// `ListServers` on the MCP actor.
    ListMcp,
    /// `SetGate` on the MCP actor, followed by a fresh `ListServers`.
    SetGate {
        /// Server name.
        server: String,
        /// Tool name; `None` sets the server gate.
        tool: Option<String>,
        /// New action; `None` clears a tool override.
        action: Option<PolicyAction>,
    },
    /// `SetApprovals` on the registry.
    SetApprovals {
        /// `None` = global, persisted.
        session: Option<SessionId>,
        /// On or off.
        enabled: bool,
    },
}

/// One transcript row.
#[derive(Debug)]
pub enum Entry {
    /// What the user typed.
    User(String),
    /// Streamed assistant text: rendered blocks + raw tail.
    Assistant(Box<markdown::Text>),
    /// Streamed reasoning; collapsed once `done`.
    Reasoning {
        /// Text so far.
        text: String,
        /// No more deltas will come.
        done: bool,
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
    },
    /// Informational line (session lists, notices).
    Info(String),
    /// Error line.
    Error(String),
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
    ("/mcp", "", "MCP servers and tool gates"),
    ("/agents", "", "list this session's subagents"),
    ("/focus", "<name>", "show a subagent's transcript"),
    ("/sessions", "", "list sessions"),
    ("/resume", "<id>", "open a session"),
    ("/new", "", "start a new session"),
    (
        "/approvals",
        "[on|off] [always]",
        "tool approval prompts; `always` persists to config.yaml",
    ),
    ("/cancel", "", "abort the in-flight turn"),
    ("/quit", "", "exit"),
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
    /// `/mcp` gates.
    Mcp {
        /// Latest `ListServers` reply.
        servers: Vec<McpServer>,
        /// Highlighted row of [`Model::mcp_rows`].
        cursor: usize,
        /// Servers whose tools are listed.
        expanded: BTreeSet<String>,
    },
}

/// One line of the `/mcp` menu.
#[derive(Debug)]
pub enum Row<'a> {
    /// A server (gate applies to all its tools).
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

/// Lowercase yaml name of a gate action.
pub fn gate_name(a: PolicyAction) -> &'static str {
    match a {
        PolicyAction::Allow => "allow",
        PolicyAction::Ask => "ask",
        PolicyAction::Deny => "deny",
    }
}

fn next_gate(a: PolicyAction) -> PolicyAction {
    match a {
        PolicyAction::Allow => PolicyAction::Ask,
        PolicyAction::Ask => PolicyAction::Deny,
        PolicyAction::Deny => PolicyAction::Allow,
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
    /// A `Send { session: None }` is in flight; adopt the next `TurnStarted`.
    awaiting_session: bool,
    /// `/mcp` was typed; the next `UiEvent::Mcp` opens the menu. Refreshes
    /// after `SetGate` only update an open menu, never reopen a closed one.
    awaiting_mcp: bool,
    /// Transcript of the shown session.
    pub entries: Vec<Entry>,
    /// The root has a turn in flight.
    pub streaming: bool,
    /// Usage of the last finished turn of the shown session.
    pub usage: Option<Usage>,
    /// Model of the shown session (`TurnStarted.model`; the config default before that).
    pub model_id: String,
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

    /// `Self::new` keeping the current model: `/new` and `/resume`.
    fn reset(&mut self) {
        *self = Self::new(std::mem::take(&mut self.model_id), self.approvals_global);
    }

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The session whose transcript is shown: the focused child, else the root.
    pub fn shown(&self) -> Option<SessionId> {
        self.focus.or(self.session)
    }

    /// Whether the shown session has a turn in flight.
    fn shown_running(&self) -> bool {
        match self.focus {
            Some(id) => self.child(id).is_some_and(|c| c.running),
            None => self.streaming,
        }
    }

    /// Some assistant text is still typing out.
    pub fn revealing(&self) -> bool {
        self.entries
            .iter()
            .rev()
            .any(|e| matches!(e, Entry::Assistant(t) if t.unrevealed() > 0))
    }

    /// One typewriter frame. Reveals a slice of the backlog proportional to
    /// its size, so the cursor trails the provider by a fixed number of
    /// frames (~a quarter second at 60 Hz) however fast it streams, and
    /// settles smoothly when it stops.
    pub fn tick(&mut self) {
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
                if list.is_empty() {
                    self.entries.push(Entry::Info("no sessions".into()));
                }
                for s in list {
                    self.entries.push(Entry::Info(format!(
                        "{}  {}  {}  {}",
                        s.id,
                        s.created_at.format("%Y-%m-%d %H:%M"),
                        s.model,
                        s.title
                    )));
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
            UiEvent::History(id, history) => {
                if self.shown() == Some(id) {
                    self.entries = history.into_iter().map(Entry::from).collect();
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
            UiEvent::ModelSet(info) => self.model_id = info.id,
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
            UiEvent::Error(e) => self.entries.push(Entry::Error(e)),
        }
    }

    fn apply_agent(&mut self, ev: AgentEvent) {
        if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
            self.session = Some(ev.session());
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
                    self.entries.push(Entry::Info(format!("↳ spawned {name}")));
                }
                return;
            }
            AgentEvent::MessageDelivered {
                session,
                from,
                text,
            } => {
                if self.shown() == Some(*session) {
                    let first = text.lines().next().unwrap_or_default();
                    self.entries
                        .push(Entry::Info(format!("⟵ {from}: {first}…")));
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
        if self.shown() != Some(ev.session()) {
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
            // Echo the turn's input unless the human just typed it here.
            AgentEvent::TurnStarted { text, model, .. } => {
                self.model_id = model;
                if !matches!(self.entries.last(), Some(Entry::User(t)) if *t == text) {
                    self.entries.push(Entry::User(text));
                }
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
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => self.entries.push(Entry::Tool {
                call_id,
                args: describe(&name, &args),
                name,
                result: None,
            }),
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
                self.usage = Some(usage);
                self.context = context;
            }
            AgentEvent::ModelChanged { model, .. } => {
                self.entries.push(Entry::Info(format!("model → {model}")));
                self.model_id = model;
            }
            AgentEvent::Error { message, .. } => self.entries.push(Entry::Error(message)),
            AgentEvent::SubagentSpawned { .. } | AgentEvent::MessageDelivered { .. } => {}
        }
    }

    /// Handle a submitted input line: slash command or message.
    pub fn submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let (cmd, arg) = line
            .split_once(' ')
            .map_or((line, ""), |(c, a)| (c, a.trim()));
        match cmd {
            "/quit" => self.quit = true,
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
            "/approvals" => self.approvals_cmd(arg, tx),
            "/mcp" => {
                self.awaiting_mcp = true;
                send(tx, Command::ListMcp);
            }
            "/agents" => match self.session {
                Some(root) => send(tx, Command::ListChildren(root)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/focus" => self.focus(arg, tx),
            "/cancel" => {
                if let Some(session) = self.shown() {
                    send(tx, Command::Cancel(session));
                }
            }
            c if c.starts_with('/') => self
                .entries
                .push(Entry::Error(format!("unknown command {c}"))),
            _ => {
                self.entries.push(Entry::User(line.to_owned()));
                let text = line.to_owned();
                match self.shown() {
                    Some(session) if self.shown_running() => {
                        send(tx, Command::Steer { session, text });
                    }
                    session => {
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
        self.usage = None;
        // ponytail: the header keeps the previous session's model until this
        // one's next TurnStarted; carry model per session if that misleads.
        self.context = 0;
        send(tx, Command::Load(target));
    }

    /// Cancel the shown session's in-flight turn, if any.
    pub fn cancel(&self, tx: &UnboundedSender<Command>) {
        if let (Some(session), true) = (self.shown(), self.shown_running()) {
            send(tx, Command::Cancel(session));
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

    /// One-line status: session, streaming state, last usage, focus, roster.
    pub fn status(&self) -> String {
        let session = self.shown().map_or_else(|| "no session".to_owned(), short);
        let state = if self.shown_running() {
            "streaming…"
        } else {
            "idle"
        };
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
        format!("{session}  {state}{usage}{focus}{agents}")
    }

    /// Line above the input: `model  ·  ctx 12.3k / 200k (6%)` and its colour level.
    pub fn header(&self, catalog: &Catalog) -> (String, Level) {
        let used = k(self.context);
        match catalog.get(&self.model_id).map(|(_, m)| m.limit.context) {
            Some(limit) if limit > 0 => {
                let pct = u64::from(self.context) * 100 / u64::from(limit);
                let level = match pct {
                    0..70 => Level::Normal,
                    70..90 => Level::Warn,
                    _ => Level::Danger,
                };
                (
                    format!(
                        "{}  ·  ctx {used} / {} ({pct}%){}",
                        self.model_id,
                        k(limit),
                        self.approvals_tag()
                    ),
                    level,
                )
            }
            _ => (
                format!("{}  ·  ctx {used}{}", self.model_id, self.approvals_tag()),
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

    /// Open or close the slash popup to match what the input box holds.
    pub fn sync_slash(&mut self, input: &str) {
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
            Some(Menu::Mcp { .. }) => self.mcp_key(code, tx),
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

    fn mcp_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Expand(String, bool),
            Gate(Command),
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
            (KeyCode::Right | KeyCode::Enter, Some(Row::Server(s))) => {
                Act::Expand(s.name.clone(), true)
            }
            (KeyCode::Left, Some(Row::Server(s) | Row::Tool(s, _))) => {
                Act::Expand(s.name.clone(), false)
            }
            (KeyCode::Char(' '), Some(Row::Server(s))) => Act::Gate(Command::SetGate {
                server: s.name.clone(),
                tool: None,
                action: Some(next_gate(s.gate)),
            }),
            (KeyCode::Char(' '), Some(Row::Tool(s, t))) => Act::Gate(Command::SetGate {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
                action: Some(next_gate(t.gate.unwrap_or(t.effective))),
            }),
            (KeyCode::Backspace, Some(Row::Tool(s, t))) => Act::Gate(Command::SetGate {
                server: s.name.clone(),
                tool: Some(t.name.clone()),
                action: None,
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
            Act::Gate(cmd) => send(tx, cmd),
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

impl From<HistoryEntry> for Entry {
    fn from(h: HistoryEntry) -> Self {
        match h {
            HistoryEntry::User { text } => Entry::User(text),
            HistoryEntry::Assistant { text } => {
                Entry::Assistant(Box::new(markdown::Text::complete(&text)))
            }
            HistoryEntry::Tool {
                name,
                args,
                output,
                is_error,
            } => Entry::Tool {
                call_id: String::new(),
                args: describe(&name, &args),
                name,
                result: Some((output, is_error)),
            },
        }
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn send(tx: &UnboundedSender<Command>, cmd: Command) {
    // The receiver lives as long as the UI; a closed channel means shutdown.
    let _ = tx.send(cmd);
}

fn short(id: SessionId) -> String {
    id.to_string()[..8].to_owned()
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
    fn mcp_space_cycles_server_and_tool_gates() {
        let (tx, mut rx) = unbounded_channel();
        let mut m = Model::default();
        let fresh = |gate: PolicyAction| McpServer {
            name: "fs".into(),
            connected: true,
            gate,
            tools: vec![McpToolInfo {
                name: "read".into(),
                description: String::new(),
                gate: None,
                effective: gate,
                registered: true,
            }],
        };
        // A refresh with no menu open and no `/mcp` typed must not open one.
        m.apply(UiEvent::Mcp(vec![fresh(PolicyAction::Allow)]));
        assert!(m.menu.is_none());
        m.submit("/mcp", &tx);
        assert!(matches!(rx.try_recv(), Ok(Command::ListMcp)));
        m.apply(UiEvent::Mcp(vec![fresh(PolicyAction::Ask)]));
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { server, tool: None, action: Some(PolicyAction::Deny) }) if server == "fs"
        ));
        m.menu_key(KeyCode::Right, "", &tx);
        m.menu_key(KeyCode::Down, "", &tx);
        assert_eq!(m.mcp_rows().len(), 2);
        m.menu_key(KeyCode::Char(' '), "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate { tool: Some(t), action: Some(PolicyAction::Deny), .. }) if t == "read"
        ));
        m.menu_key(KeyCode::Backspace, "", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SetGate {
                tool: Some(_),
                action: None,
                ..
            })
        ));
        m.menu_key(KeyCode::Left, "", &tx);
        assert!(matches!(m.menu, Some(Menu::Mcp { cursor: 0, .. })));
        assert_eq!(m.mcp_rows().len(), 1);
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
        m.model_id = "nobody/nothing".into();
        m.context = 1500;
        assert_eq!(
            m.header(&catalog),
            ("nobody/nothing  ·  ctx 1.5k".to_owned(), Level::Normal)
        );
        assert_eq!(window(50, 20, 10), 11..21);
        assert_eq!(window(5, 0, 10), 0..5);
    }
}
