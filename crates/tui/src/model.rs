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

/// UI state.
#[derive(Debug, Default)]
pub struct Model {
    /// Session being viewed; events for other sessions are ignored.
    pub session: Option<SessionId>,
    /// A `Send { session: None }` is in flight; adopt the next `TurnStarted`.
    awaiting_session: bool,
    /// Transcript.
    pub entries: Vec<Entry>,
    /// A turn is in flight.
    pub streaming: bool,
    /// Usage of the last finished turn.
    pub usage: Option<Usage>,
    /// Pending approvals for this session, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// The user asked to leave.
    pub quit: bool,
}

impl Model {
    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// Fold one event into the state.
    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Agent(ev) => self.apply_agent(ev),
            UiEvent::Approval(req) => {
                if self.session == Some(req.session) {
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
            UiEvent::History(id, history) => {
                if self.session == Some(id) {
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
        if self.session != Some(ev.session()) {
            if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
                self.session = Some(ev.session());
                self.awaiting_session = false;
            } else {
                return;
            }
        }
        if !matches!(ev, AgentEvent::ReasoningDelta { .. })
            && let Some(Entry::Reasoning { done, .. }) = self.entries.last_mut()
        {
            *done = true;
        }
        match ev {
            AgentEvent::TurnStarted { .. } => self.streaming = true,
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
            AgentEvent::TurnFinished { usage, .. } => {
                self.streaming = false;
                self.usage = Some(usage);
            }
            AgentEvent::Error { message, .. } => self.entries.push(Entry::Error(message)),
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
            "/cancel" => {
                if let Some(session) = self.session {
                    send(tx, Command::Cancel(session));
                }
            }
            c if c.starts_with('/') => self
                .entries
                .push(Entry::Error(format!("unknown command {c}"))),
            _ => {
                self.entries.push(Entry::User(line.to_owned()));
                let text = line.to_owned();
                match self.session {
                    Some(session) if self.streaming => send(tx, Command::Steer { session, text }),
                    session => {
                        self.awaiting_session = session.is_none();
                        send(tx, Command::Send { session, text });
                    }
                }
            }
        }
    }

    /// Switch to `id` and ask for its history.
    pub fn resume(&mut self, id: SessionId, tx: &UnboundedSender<Command>) {
        *self = Model::default();
        self.session = Some(id);
        send(tx, Command::Load(id));
    }

    /// Cancel the in-flight turn, if any.
    pub fn cancel(&self, tx: &UnboundedSender<Command>) {
        if let (Some(session), true) = (self.session, self.streaming) {
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

    /// One-line status: session, streaming state, last usage.
    pub fn status(&self, model_name: &str) -> String {
        let session = self.session.map_or_else(
            || "no session".to_owned(),
            |s| s.to_string()[..8].to_owned(),
        );
        let state = if self.streaming {
            "streaming…"
        } else {
            "idle"
        };
        let usage = self.usage.map_or_else(String::new, |u| {
            format!("  {}↑ {}↓", u.prompt, u.completion)
        });
        format!("{session}  {model_name}  {state}{usage}")
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
