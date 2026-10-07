//! The board message of a parent thread (21 §9.2): one row per descendant,
//! driven by the child events the worker already routes, rendered as
//! markdown and `chat.update`d in place. Pure: no I/O, no clocks of its own.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use agent::{AgentEvent, AgentInfo, AgentState};
use runtime::SessionId;

/// Rows shown before `… n more`.
pub const MAX_ROWS: usize = 8;
/// Minimum spacing between two edits of the board message.
pub const BOARD_MIN_GAP: Duration = Duration::from_secs(2);
/// Longest detail (tool + first arg) kept on a row, in chars.
const DETAIL_MAX: usize = 60;

/// Live state of one descendant (19 §2.1 words).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Never ran, or the last turn ended normally. Not listed.
    Idle,
    /// A turn is in flight.
    Running,
    /// Blocked on a peer's reply.
    Awaiting,
    /// An approval is unanswered.
    Approval,
    /// The last turn ended with an error.
    Error,
    /// The last turn was cancelled.
    Cancelled,
}

impl State {
    fn glyph(self) -> &'static str {
        match self {
            Self::Idle => "·",
            Self::Running => "⠹",
            Self::Awaiting => "⏳",
            Self::Approval => "⚠",
            Self::Error | Self::Cancelled => "✗",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Awaiting => "awaiting",
            Self::Approval => "approval",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    /// Render order: running, then blocked, then finished badly.
    fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Awaiting | Self::Approval => 1,
            Self::Error | Self::Cancelled => 2,
            Self::Idle => 3,
        }
    }
}

/// One descendant of the thread's session.
#[derive(Clone, Debug)]
pub struct Row {
    /// The child's session.
    pub session: SessionId,
    /// Peer name.
    pub name: String,
    /// 1 = direct child; indents two spaces per extra level.
    pub depth: u8,
    /// Live state.
    pub state: State,
    /// What it is doing: `streaming`, `<tool> <first arg>`, an error line, …
    pub detail: String,
    /// When `state` last changed; the age column.
    pub since: Instant,
    /// Permalink of the child's thread, once known.
    pub link: Option<String>,
}

impl Row {
    /// An idle row with no detail.
    #[must_use]
    pub fn new(
        session: SessionId,
        name: impl Into<String>,
        depth: u8,
        link: Option<String>,
    ) -> Self {
        Self {
            session,
            name: name.into(),
            depth,
            state: State::Idle,
            detail: String::new(),
            since: Instant::now(),
            link,
        }
    }

    /// Apply a child event; `true` when the row changed. `TextDelta` and
    /// everything else not listed is ignored: the last tool detail stays
    /// until the tool or the turn finishes.
    pub fn apply(&mut self, ev: &AgentEvent) -> bool {
        match ev {
            AgentEvent::TurnStarted { .. } | AgentEvent::ToolCallFinished { .. } => {
                self.set(State::Running, "streaming".to_owned())
            }
            AgentEvent::ToolCallStarted { name, args, .. } => {
                let arg = first_arg(name, args);
                let detail = if arg.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {}", cut(arg, DETAIL_MAX))
                };
                self.set(State::Running, detail)
            }
            AgentEvent::TurnFinished { stop_reason, .. } => {
                let state = match stop_reason.as_str() {
                    "cancelled" => State::Cancelled,
                    "error" => State::Error,
                    _ => State::Idle,
                };
                self.set(state, String::new())
            }
            AgentEvent::Error { message, .. } => {
                let line = message.lines().next().unwrap_or_default();
                self.set(State::Error, cut(line, DETAIL_MAX))
            }
            _ => false,
        }
    }

    /// `Some(tool)`: an approval for `tool` is pending; `None`: it was
    /// answered and the child runs again. `true` when the row changed.
    pub fn approval(&mut self, tool: Option<&str>) -> bool {
        match tool {
            Some(tool) => self.set(State::Approval, format!("{tool} …")),
            None => {
                let detail = self.detail.trim_end_matches(" …").to_owned();
                self.set(State::Running, detail)
            }
        }
    }

    /// Rebuild from `list_agents` after a restart: state and detail from the
    /// live `AgentState` and `last`, age from `started_at` (running) or
    /// `finished_at` (otherwise).
    pub fn seed(&mut self, info: &AgentInfo) {
        let (state, detail) = match &info.state {
            AgentState::Idle => (State::Idle, String::new()),
            AgentState::Running if info.last.is_empty() => (State::Running, "streaming".to_owned()),
            AgentState::Running => (State::Running, cut(&info.last, DETAIL_MAX)),
            AgentState::Awaiting { from } => (
                State::Awaiting,
                cut(&format!("{from}: {}", info.last), DETAIL_MAX),
            ),
            AgentState::Approval { tool } => (State::Approval, format!("{tool} …")),
            AgentState::Error => (State::Error, cut(&info.last, DETAIL_MAX)),
            AgentState::Cancelled => (State::Cancelled, String::new()),
        };
        let now = Instant::now();
        let at = if state == State::Running {
            info.started_at
        } else {
            info.finished_at
        };
        self.name.clone_from(&info.name);
        self.depth = info.depth;
        self.state = state;
        self.detail = detail;
        self.since = at
            .and_then(|t| (chrono::Utc::now() - t).to_std().ok())
            .and_then(|ago| now.checked_sub(ago))
            .unwrap_or(now);
    }

    fn set(&mut self, state: State, detail: String) -> bool {
        let changed = self.state != state || self.detail != detail;
        if self.state != state {
            self.since = Instant::now();
        }
        self.state = state;
        self.detail = detail;
        changed
    }

    fn line(&self, out: &mut String, now: Instant) {
        let indent = 2 * usize::from(self.depth.saturating_sub(1));
        let age = fmt_age(now.saturating_duration_since(self.since));
        let _ = write!(
            out,
            "{:indent$}{} {:<14} {:<9} {:<40} {:>6}",
            "",
            self.state.glyph(),
            self.name,
            self.state.word(),
            self.detail,
            age
        );
        if let Some(link) = &self.link {
            let _ = write!(out, " · <{link}|thread>");
        }
    }
}

/// `true` when some row is not idle.
#[must_use]
pub fn any_live(rows: &[Row]) -> bool {
    rows.iter().any(|r| r.state != State::Idle)
}

/// The board text: `**agents**` then the live rows (running, then
/// awaiting/approval, then error/cancelled), at most [`MAX_ROWS`] plus
/// `… n more`; `**agents** · all idle` when nothing is live.
#[must_use]
pub fn render(rows: &[Row], now: Instant) -> String {
    let mut live: Vec<&Row> = rows.iter().filter(|r| r.state != State::Idle).collect();
    if live.is_empty() {
        return "**agents** · all idle".to_owned();
    }
    live.sort_by_key(|r| r.state.rank());
    let mut out = String::from("**agents**");
    for row in live.iter().take(MAX_ROWS) {
        out.push('\n');
        row.line(&mut out, now);
    }
    if live.len() > MAX_ROWS {
        let _ = write!(out, "\n… {} more", live.len() - MAX_ROWS);
    }
    out
}

/// `48s`, `1m32s`, `12m`, `1h05m`; also the age in a 110 §6 `↳ name · finished · <age>` title.
pub(crate) fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=599 => format!("{}m{:02}s", s / 60, s % 60),
        600..=3599 => format!("{}m", s / 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// The one argument worth a board column: `shell`'s command, else the first
/// string argument; first line only.
fn first_arg<'a>(name: &str, args: &'a serde_json::Value) -> &'a str {
    let s = if name == "shell" {
        args.get("command").and_then(|c| c.as_str())
    } else {
        args.as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str()))
    };
    s.and_then(|s| s.lines().next()).unwrap_or_default().trim()
}

/// `s` cut to `max` chars, the last one an ellipsis when cut.
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let end = s.char_indices().nth(max - 1).map_or(s.len(), |(i, _)| i);
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use runtime::TurnId;
    use serde_json::json;

    use super::*;

    fn row(name: &str) -> Row {
        Row::new(SessionId::new(), name, 1, None)
    }

    fn started(session: SessionId) -> AgentEvent {
        AgentEvent::TurnStarted {
            session,
            turn: TurnId::new(),
            text: "go".into(),
            model: "m".into(),
            turn_seq: 0,
        }
    }

    fn finished(session: SessionId, stop_reason: &str) -> AgentEvent {
        AgentEvent::TurnFinished {
            session,
            turn: TurnId::new(),
            stop_reason: stop_reason.into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    fn tool(session: SessionId, name: &str, args: serde_json::Value) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            session,
            turn: TurnId::new(),
            call_id: "c1".into(),
            name: name.into(),
            args,
        }
    }

    fn at(rows: &mut [Row], state: State) {
        for r in rows {
            r.state = state;
        }
    }

    #[test]
    fn turn_lifecycle() {
        let mut r = row("a");
        assert!(r.apply(&started(r.session)));
        assert_eq!((r.state, r.detail.as_str()), (State::Running, "streaming"));
        assert!(!r.apply(&started(r.session)), "no change twice");

        assert!(r.apply(&tool(
            r.session,
            "shell",
            json!({"command": "cargo test -p agent\n&& echo ok"})
        )));
        assert_eq!(r.detail, "shell cargo test -p agent");
        assert!(r.apply(&tool(
            r.session,
            "read_file",
            json!({"path": "src/lib.rs", "limit": 10})
        )));
        assert_eq!(r.detail, "read_file src/lib.rs");
        let long = "x".repeat(100);
        r.apply(&tool(r.session, "grep", json!({"pattern": long})));
        assert_eq!(r.detail.chars().count(), "grep ".len() + DETAIL_MAX);
        assert!(r.detail.ends_with('…'));

        assert!(!r.apply(&AgentEvent::TextDelta {
            session: r.session,
            turn: TurnId::new(),
            text: "hi".into()
        }));
        assert!(
            r.detail.starts_with("grep "),
            "text deltas keep the tool detail"
        );

        assert!(r.apply(&finished(r.session, "cancelled")));
        assert_eq!((r.state, r.detail.as_str()), (State::Cancelled, ""));
        r.apply(&finished(r.session, "error"));
        assert_eq!(r.state, State::Error);
        r.apply(&finished(r.session, "end_turn"));
        assert_eq!(r.state, State::Idle);

        r.apply(&AgentEvent::Error {
            session: r.session,
            turn: None,
            message: "boom\nstack".into(),
        });
        assert_eq!((r.state, r.detail.as_str()), (State::Error, "boom"));
    }

    #[test]
    fn approval_round_trip() {
        let mut r = row("a");
        r.apply(&started(r.session));
        assert!(r.approval(Some("shell")));
        assert_eq!((r.state, r.detail.as_str()), (State::Approval, "shell …"));
        assert!(r.approval(None));
        assert_eq!((r.state, r.detail.as_str()), (State::Running, "shell"));
    }

    #[test]
    fn render_orders_hides_idle_and_caps() {
        let now = Instant::now();
        let mut rows: Vec<Row> = (0..12).map(|i| row(&format!("r{i}"))).collect();
        assert_eq!(render(&rows, now), "**agents** · all idle");
        assert!(!any_live(&rows));

        at(&mut rows[0..1], State::Error);
        at(&mut rows[1..2], State::Approval);
        at(&mut rows[2..3], State::Running);
        at(&mut rows[3..4], State::Idle);
        at(&mut rows[4..12], State::Cancelled);
        rows[2].link = Some("https://x/p".into());
        rows[2].since = now - Duration::from_secs(92);
        assert!(any_live(&rows));

        let text = render(&rows, now);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "**agents**");
        assert!(lines[1].starts_with("⠹ r2"), "{text}");
        assert!(lines[1].contains(" 1m32s · <https://x/p|thread>"), "{text}");
        assert!(lines[2].starts_with("⚠ r1"), "{text}");
        assert!(lines[3].starts_with("✗ r0"), "{text}");
        assert_eq!(lines.len(), 1 + MAX_ROWS + 1);
        assert_eq!(lines[MAX_ROWS + 1], "… 3 more");
        assert!(!text.contains("r3"), "idle rows are not listed");
    }

    #[test]
    fn grandchild_indents() {
        let mut r = Row::new(SessionId::new(), "kid", 2, None);
        r.state = State::Running;
        assert!(
            render(&[r], Instant::now())
                .lines()
                .nth(1)
                .unwrap()
                .starts_with("  ⠹ kid")
        );
    }

    #[test]
    fn seed_from_list_agents() {
        let mut r = row("x");
        let info = AgentInfo {
            id: r.session,
            name: "scout".into(),
            parent: SessionId::new(),
            depth: 2,
            agent: None,
            model: "m".into(),
            state: AgentState::Running,
            started_at: Some(chrono::Utc::now() - chrono::Duration::seconds(75)),
            finished_at: None,
            last: "reading the loop".into(),
            project: None,
            retention: agent::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
        };
        r.seed(&info);
        assert_eq!(
            (r.state, r.detail.as_str(), r.depth, r.name.as_str()),
            (State::Running, "reading the loop", 2, "scout")
        );
        let age = Instant::now().duration_since(r.since).as_secs();
        assert!((74..=76).contains(&age), "{age}");

        let info = AgentInfo {
            state: AgentState::Approval {
                tool: "shell".into(),
            },
            ..info
        };
        r.seed(&info);
        assert_eq!((r.state, r.detail.as_str()), (State::Approval, "shell …"));
    }

    #[test]
    fn ages() {
        assert_eq!(fmt_age(Duration::from_secs(48)), "48s");
        assert_eq!(fmt_age(Duration::from_secs(92)), "1m32s");
        assert_eq!(fmt_age(Duration::from_secs(12 * 60 + 5)), "12m");
        assert_eq!(fmt_age(Duration::from_secs(3900)), "1h05m");
    }
}
