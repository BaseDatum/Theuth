//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::VecDeque;

use agent::{AgentEvent, HistoryEntry, SessionSummary, Usage};
use runtime::SessionId;
use tokio::sync::mpsc::UnboundedSender;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved, Decision};

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
}

/// One transcript row.
#[derive(Debug)]
pub enum Entry {
    /// What the user typed.
    User(String),
    /// Streamed assistant text.
    Assistant(String),
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
        /// Argument summary.
        summary: String,
        /// `(first line of output, is_error)`.
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
    /// Transcript of the shown session.
    pub entries: Vec<Entry>,
    /// The root has a turn in flight.
    pub streaming: bool,
    /// Usage of the last finished turn of the shown session.
    pub usage: Option<Usage>,
    /// Pending approvals for the root or any child, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// The user asked to leave.
    pub quit: bool,
}

impl Model {
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
        match ev {
            // Echo the turn's input unless the human just typed it here.
            AgentEvent::TurnStarted { text, .. } => {
                if !matches!(self.entries.last(), Some(Entry::User(t)) if *t == text) {
                    self.entries.push(Entry::User(text));
                }
            }
            AgentEvent::TextDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Assistant(s)) => s.push_str(&text),
                _ => self.entries.push(Entry::Assistant(text.to_string())),
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
                summary,
                ..
            } => self.entries.push(Entry::Tool {
                call_id,
                name,
                summary,
                result: None,
            }),
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                let first = output.lines().next().unwrap_or_default().to_owned();
                match self.entries.iter_mut().rev().find(
                    |e| matches!(e, Entry::Tool { call_id: c, result: None, .. } if *c == call_id),
                ) {
                    Some(Entry::Tool { result, .. }) => *result = Some((first, is_error)),
                    _ => self.entries.push(Entry::Info(format!(
                        "{} {first}",
                        if is_error { "✗" } else { "✓" }
                    ))),
                }
            }
            AgentEvent::TurnFinished { usage, .. } => self.usage = Some(usage),
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
                *self = Model::default();
                self.entries.push(Entry::Info("new session".into()));
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
        *self = Model::default();
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
    pub fn status(&self, model_name: &str) -> String {
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
        format!("{session}  {model_name}  {state}{usage}{focus}{agents}")
    }
}

impl From<HistoryEntry> for Entry {
    fn from(h: HistoryEntry) -> Self {
        match h {
            HistoryEntry::User { text } => Entry::User(text),
            HistoryEntry::Assistant { text } => Entry::Assistant(text),
            HistoryEntry::Tool {
                name,
                summary,
                output,
                is_error,
            } => Entry::Tool {
                call_id: String::new(),
                name,
                summary,
                result: Some((
                    output.lines().next().unwrap_or_default().to_owned(),
                    is_error,
                )),
            },
        }
    }
}

fn send(tx: &UnboundedSender<Command>, cmd: Command) {
    // The receiver lives as long as the UI; a closed channel means shutdown.
    let _ = tx.send(cmd);
}

fn short(id: SessionId) -> String {
    id.to_string()[..8].to_owned()
}
