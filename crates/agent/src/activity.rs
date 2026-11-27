//! Design doc 155 ACTIVITY (§3, §5): the per-child activity monitor and the
//! state words an orchestrator can wait on.
//!
//! [`ChildActivity`] lives in the `AcpChild` actor and is pure — every method
//! takes `now` — so the stall rules (D3) test with a fake clock. The actor
//! arms one timer at [`ChildActivity::deadline`] and reports a stall episode
//! to the supervisor as [`ChildStalled`]. [`peer_state_of`] folds what the
//! supervisor knows about a child into one [`PeerStateWord`]; [`Monitor`] is
//! the supervisor-side state (`Agents.activity`): open stall episodes and
//! registered `wait_peer`s (§5.2).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `agent.activity` (155 §3.5): the D3 stall thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityConfig {
    /// No `session/update` within this many seconds of a delivered prompt is
    /// `no_first_update`; `0` = never.
    pub first_update_secs: u64,
    /// No update for this many seconds mid-turn with no tool call in
    /// progress is `silent`; `0` = never.
    pub silent_secs: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            first_update_secs: 60,
            silent_secs: 180,
        }
    }
}

/// Which D3 rule fired (155 §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum StallPhase {
    /// Nothing came back within `first_update_secs` of the prompt.
    NoFirstUpdate,
    /// Mid-turn, no update for `silent_secs`, no tool call in progress.
    Silent,
}

impl StallPhase {
    /// The wire / attribute spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFirstUpdate => "no_first_update",
            Self::Silent => "silent",
        }
    }
}

/// One `session/update` as the monitor sees it (155 §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildUpdate<'a> {
    /// `agent_message_chunk`.
    Message,
    /// `agent_thought_chunk`.
    Thinking,
    /// `tool_call` with a non-terminal status; the title.
    ToolStarted(&'a str),
    /// `tool_call_update` with `completed` | `failed` (or a `tool_call` born terminal).
    ToolFinished,
    /// Plan, usage, mode, config, commands: bookkeeping that still counts as life.
    Other,
}

/// One wake (155 §4.1): the attributes of `<child_stalled>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    /// Which rule fired.
    pub phase: StallPhase,
    /// Seconds since delivery (`no_first_update`) or the last update (`silent`).
    pub quiet_secs: u64,
    /// What the child last did, a few words (tool title, `message`, `thinking`); empty when nothing yet.
    pub last: String,
}

/// The per-child monitor (155 §3). Pure: nothing here reads the clock.
#[derive(Debug)]
pub struct ChildActivity {
    cfg: ActivityConfig,
    delivered: Option<Instant>,
    first_update: Option<Instant>,
    last_update: Option<Instant>,
    tools_in_progress: u32,
    last: String,
    /// An open episode: the phase and when it fired.
    episode: Option<(StallPhase, Instant)>,
}

impl ChildActivity {
    /// A monitor with no turn.
    #[must_use]
    pub fn new(cfg: ActivityConfig) -> Self {
        Self {
            cfg,
            delivered: None,
            first_update: None,
            last_update: None,
            tools_in_progress: 0,
            last: String::new(),
            episode: None,
        }
    }

    /// The prompt was handed to the harness (`session/prompt` sent): the turn's
    /// clock starts; a previous episode ends.
    pub fn delivered(&mut self, now: Instant) {
        self.delivered = Some(now);
        self.episode = None;
    }

    /// A `session/update` of this session arrived.
    pub fn update(&mut self, now: Instant, what: ChildUpdate<'_>) {
        self.first_update.get_or_insert(now);
        self.last_update = Some(now);
        self.episode = None;
        match what {
            ChildUpdate::Message => self.last = "message".into(),
            ChildUpdate::Thinking => self.last = "thinking".into(),
            ChildUpdate::ToolStarted(title) => {
                self.tools_in_progress += 1;
                self.last = title.to_owned();
            }
            ChildUpdate::ToolFinished => {
                self.tools_in_progress = self.tools_in_progress.saturating_sub(1);
            }
            ChildUpdate::Other => {}
        }
    }

    /// The turn closed (`PromptDone`, cancel, harness closed): nothing to watch.
    pub fn turn_ended(&mut self, _now: Instant) {
        self.delivered = None;
        self.first_update = None;
        self.last_update = None;
        self.tools_in_progress = 0;
        self.episode = None;
    }

    /// The rule that can fire next: `(phase, since, deadline)`; the earliest
    /// when both apply. `None` with an open episode, a tool call in
    /// progress (for `silent`), or the rule off (§3.2).
    fn due(&self) -> Option<(StallPhase, Instant, Instant)> {
        if self.episode.is_some() {
            return None;
        }
        let first = match (
            self.delivered,
            self.first_update,
            self.cfg.first_update_secs,
        ) {
            (Some(d), None, secs) if secs > 0 => {
                Some((StallPhase::NoFirstUpdate, d, d + Duration::from_secs(secs)))
            }
            _ => None,
        };
        let silent = match (
            self.last_update,
            self.tools_in_progress,
            self.cfg.silent_secs,
        ) {
            (Some(l), 0, secs) if secs > 0 => {
                Some((StallPhase::Silent, l, l + Duration::from_secs(secs)))
            }
            _ => None,
        };
        match (first, silent) {
            (Some(a), Some(b)) => Some(if a.2 <= b.2 { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// When the next stall check is due; `None` when nothing can fire (no
    /// turn, a tool call in progress, an episode already open, rule off).
    #[must_use]
    pub fn deadline(&self, _now: Instant) -> Option<Instant> {
        self.due().map(|(_, _, at)| at)
    }

    /// At or after the deadline: the wake to send, once per episode.
    pub fn fire(&mut self, now: Instant) -> Option<Stall> {
        let (phase, since, at) = self.due()?;
        if now < at {
            return None;
        }
        self.episode = Some((phase, now));
        Some(Stall {
            phase,
            quiet_secs: now.duration_since(since).as_secs(),
            last: self.last.clone(),
        })
    }

    /// Tool calls started and not yet finished.
    #[must_use]
    pub fn tools_in_progress(&self) -> u32 {
        self.tools_in_progress
    }

    /// What the child last did, a few words.
    #[must_use]
    pub fn last(&self) -> &str {
        &self.last
    }

    /// Whether a stall episode is open.
    #[must_use]
    pub fn stalled(&self) -> Option<StallPhase> {
        self.episode.map(|(p, _)| p)
    }
}

/// The 155 §5.1 words. `until` accepts the [`Self::waitable`] four; the
/// rest are what a wait ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PeerStateWord {
    /// A turn is in flight and nothing below applies.
    Running,
    /// An unanswered permission / ask keyed to the child, or `RequiresAction`.
    Blocked,
    /// Turn ended, connection up: it can be prompted again.
    Idle,
    /// Idle, result delivered, nothing queued.
    Done,
    /// The activity monitor has an open episode (§3.2).
    Stalled,
    /// The pinned child is gone or replaced (terminal).
    NotRunning,
    /// The wait's deadline passed (terminal).
    Timeout,
}

impl PeerStateWord {
    /// The wire / attribute spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Stalled => "stalled",
            Self::NotRunning => "not_running",
            Self::Timeout => "timeout",
        }
    }

    /// A word `wait_peer.until` may name.
    #[must_use]
    pub const fn waitable(self) -> bool {
        matches!(
            self,
            Self::Blocked | Self::Idle | Self::Done | Self::Stalled
        )
    }
}

/// Harness capability flags (mirrors `acp::Reports` on desktop).
#[cfg(not(target_os = "ios"))]
pub use acp::Reports;

/// Harness capability flags (iOS stand-in for `acp::Reports` — native sessions always report full lifecycle).
#[cfg(target_os = "ios")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reports {
    /// Harness surfaces permission requests.
    pub permission: bool,
    /// Harness signals turn completion.
    pub turn_end: bool,
    /// Harness supports session/load.
    pub session_load: bool,
}

/// What the supervisor knows about one child (155 §5.1 table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerFacts {
    /// A live actor exists for the session.
    pub live: bool,
    /// The harness connection ended for good (`AcpChild.dead`).
    pub dead: bool,
    /// `Agents.running` has it (a turn in flight, continuation included).
    pub running: bool,
    /// An unanswered approval keyed to it.
    pub approval: bool,
    /// An unanswered `ask_user` of it.
    pub ask: bool,
    /// `observed.state == RequiresAction`.
    pub requires_action: bool,
    /// Seconds since its open stall episode began.
    pub stalled_secs: Option<u64>,
    /// No `waits(child_result)` row targets it.
    pub result_delivered: bool,
    /// The ledger shows admitted input for it.
    pub queued: bool,
    /// Its harness's capability entry (§8).
    pub reports: Reports,
}

/// The §5.1 fold: blocked > stalled > done > idle > running; `not_running`
/// is terminal and wins; without `reports.turn_end` idle/done stay running.
#[must_use]
pub fn peer_state_of(f: &PeerFacts) -> PeerStateWord {
    if !f.live || f.dead {
        return PeerStateWord::NotRunning;
    }
    if f.approval || f.ask || f.requires_action {
        return PeerStateWord::Blocked;
    }
    if f.stalled_secs.is_some() {
        return PeerStateWord::Stalled;
    }
    if f.running || !f.reports.turn_end {
        return PeerStateWord::Running;
    }
    if f.result_delivered && !f.queued {
        PeerStateWord::Done
    } else {
        PeerStateWord::Idle
    }
}

/// One registered `wait_peer` (155 §5.2); the `waits(peer_state)` row's
/// in-memory twin.
#[derive(Clone, Debug)]
pub struct PeerWait {
    /// Who waits.
    pub caller: SessionId,
    /// The registering call (the row key with `caller`).
    pub call_id: String,
    /// The pinned child.
    pub child: SessionId,
    /// The child's name at registration, for the wake.
    pub name: String,
    /// Words that settle it.
    pub until: Vec<PeerStateWord>,
    /// `timeout_secs` from registration, if any.
    pub deadline: Option<Instant>,
    /// When it was registered (re-arm keeps the original).
    pub since: Instant,
}

/// Supervisor-side state: `Agents.activity` (155 §4.2, §5.2).
#[derive(Debug, Default)]
pub struct Monitor {
    /// Open stall episodes by child: phase and when it began.
    pub stalls: HashMap<SessionId, (StallPhase, Instant)>,
    /// Registered waits by `(caller, child)`; one per pair.
    pub waits: HashMap<(SessionId, SessionId), PeerWait>,
}

impl Monitor {
    /// Seconds since `child`'s open stall episode began; `None` when it is
    /// not stalled (§5.5).
    #[must_use]
    pub fn stalled_secs(&self, child: SessionId, now: Instant) -> Option<u64> {
        self.stalls
            .get(&child)
            .map(|(_, since)| now.saturating_duration_since(*since).as_secs())
    }
}

/// `tell` from the child actor (155 §4.2): `Some` opens an episode and wakes
/// the parent; `None` ends it.
#[derive(Clone, Debug)]
pub struct ChildStalled {
    /// The child.
    pub child: SessionId,
    /// The wake, or the all-clear.
    pub stall: Option<Stall>,
}

/// `ask` → `Result<PeerWaitRegistered, String>` (155 §5.2): register a
/// `wait_peer`, or answer at once when the condition already holds.
#[derive(Clone, Debug)]
pub struct WaitPeer {
    /// The caller.
    pub session: SessionId,
    /// The caller's tool call (the `waits` row key).
    pub call_id: String,
    /// One of the caller's children: a name or `session:<uuid>`.
    pub to: String,
    /// Non-empty subset of the waitable words.
    pub until: Vec<PeerStateWord>,
    /// Optional deadline in seconds; `Some(0)` is refused.
    pub timeout_secs: Option<u64>,
}

/// What `wait_peer` returns at once (155 §5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerWaitRegistered {
    /// The pinned child.
    pub child: SessionId,
    /// Its name.
    pub name: String,
    /// Its state now.
    pub state: PeerStateWord,
    /// `state ∈ until`: nothing registered, no wake follows.
    pub holds: bool,
    /// Seconds since `state` became true (0 when unknown).
    pub since_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    fn monitor() -> (ChildActivity, Instant) {
        (
            ChildActivity::new(ActivityConfig::default()),
            Instant::now(),
        )
    }

    /// §3.2 `no_first_update`: the deadline is `delivered + 60 s`; 59 s is
    /// not a stall, 60 s is, with the seconds since delivery.
    #[test]
    fn no_first_update_fires_at_the_threshold() {
        let (mut a, t0) = monitor();
        assert_eq!(a.deadline(t0), None, "no turn, nothing due");
        a.delivered(t0);
        assert_eq!(a.deadline(t0), Some(t0 + S(60)));
        assert_eq!(a.fire(t0 + S(59)), None);
        let stall = a.fire(t0 + S(60)).expect("fires at 60 s");
        assert_eq!(stall.phase, StallPhase::NoFirstUpdate);
        assert_eq!(stall.quiet_secs, 60);
        assert_eq!(stall.last, "");
        assert_eq!(a.stalled(), Some(StallPhase::NoFirstUpdate));
        assert_eq!(a.deadline(t0 + S(60)), None, "episode open: nothing due");
    }

    /// §3.2 `silent`: 180 s after the last update, only with no tool call in
    /// progress; `ToolStarted` suspends the deadline, `ToolFinished` resumes
    /// it from the finish; `last` names the tool.
    #[test]
    fn silent_fires_only_without_a_tool_call_in_progress() {
        let (mut a, t0) = monitor();
        a.delivered(t0);
        a.update(t0 + S(5), ChildUpdate::Message);
        assert_eq!(a.deadline(t0), Some(t0 + S(185)));
        a.update(t0 + S(10), ChildUpdate::ToolStarted("shell: cargo check"));
        assert_eq!(a.tools_in_progress(), 1);
        assert_eq!(
            a.deadline(t0),
            None,
            "a tool call in progress is never a stall"
        );
        assert_eq!(a.fire(t0 + S(1000)), None);
        a.update(t0 + S(1000), ChildUpdate::ToolFinished);
        assert_eq!(a.tools_in_progress(), 0);
        assert_eq!(
            a.deadline(t0),
            Some(t0 + S(1180)),
            "resumes from the finish"
        );
        assert_eq!(a.fire(t0 + S(1179)), None);
        let stall = a.fire(t0 + S(1180)).expect("silent at 180 s");
        assert_eq!(stall.phase, StallPhase::Silent);
        assert_eq!(stall.quiet_secs, 180);
        assert_eq!(stall.last, "shell: cargo check");
    }

    /// §3.2: one `Some` per episode; the next update re-arms and the next
    /// episode fires again; the turn end clears everything.
    #[test]
    fn one_wake_per_episode_rearmed_by_update() {
        let (mut a, t0) = monitor();
        a.delivered(t0);
        assert!(a.fire(t0 + S(60)).is_some());
        assert_eq!(a.fire(t0 + S(120)), None, "same episode");
        a.update(t0 + S(130), ChildUpdate::Thinking);
        assert_eq!(a.stalled(), None);
        assert_eq!(a.deadline(t0), Some(t0 + S(310)));
        assert!(a.fire(t0 + S(310)).is_some(), "next episode");
        assert_eq!(a.fire(t0 + S(400)), None);
        a.turn_ended(t0 + S(400));
        assert_eq!(a.stalled(), None);
        assert_eq!(a.deadline(t0), None);
        assert_eq!(a.last(), "thinking", "what it did last survives the turn");
    }

    /// §3.2: `first_update_secs = 0` disables that rule; `silent_secs = 0`
    /// disables the other.
    #[test]
    fn zero_disables_a_rule() {
        let t0 = Instant::now();
        let mut a = ChildActivity::new(ActivityConfig {
            first_update_secs: 0,
            silent_secs: 180,
        });
        a.delivered(t0);
        assert_eq!(a.deadline(t0), None);
        assert_eq!(a.fire(t0 + S(3600)), None);
        a.update(t0 + S(1), ChildUpdate::Message);
        assert_eq!(a.deadline(t0), Some(t0 + S(181)));
        let mut b = ChildActivity::new(ActivityConfig {
            first_update_secs: 60,
            silent_secs: 0,
        });
        b.delivered(t0);
        b.update(t0 + S(1), ChildUpdate::Message);
        assert_eq!(b.deadline(t0), None);
        assert_eq!(b.fire(t0 + S(3600)), None);
    }

    /// §3.1: a continuation turn has no `delivered`, so only `silent` applies.
    #[test]
    fn continuation_turn_only_fires_silent() {
        let (mut a, t0) = monitor();
        a.update(t0, ChildUpdate::Message);
        assert_eq!(a.deadline(t0), Some(t0 + S(180)));
        assert_eq!(a.fire(t0 + S(60)), None);
        let stall = a.fire(t0 + S(180)).expect("silent");
        assert_eq!(stall.phase, StallPhase::Silent);
        assert_eq!(stall.last, "message");
    }

    fn facts() -> PeerFacts {
        PeerFacts {
            live: true,
            dead: false,
            running: false,
            approval: false,
            ask: false,
            requires_action: false,
            stalled_secs: None,
            result_delivered: true,
            queued: false,
            reports: Reports {
                permission: true,
                turn_end: true,
                session_load: true,
            },
        }
    }

    /// §5.1 precedence: blocked > stalled > done > idle > running;
    /// `turn_end = false` turns idle/done into running; `dead` or `!live`
    /// is `not_running` over everything.
    #[test]
    fn peer_state_precedence() {
        use PeerStateWord::*;
        let f = facts();
        assert_eq!(peer_state_of(&f), Done);
        assert_eq!(
            peer_state_of(&PeerFacts {
                queued: true,
                ..f.clone()
            }),
            Idle
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                result_delivered: false,
                ..f.clone()
            }),
            Idle
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                running: true,
                ..f.clone()
            }),
            Running
        );
        let stalled = PeerFacts {
            stalled_secs: Some(184),
            running: true,
            ..f.clone()
        };
        assert_eq!(
            peer_state_of(&stalled),
            Stalled,
            "stalled beats running/done"
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                approval: true,
                ..stalled.clone()
            }),
            Blocked,
            "blocked beats stalled"
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                ask: true,
                ..f.clone()
            }),
            Blocked
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                requires_action: true,
                ..f.clone()
            }),
            Blocked
        );
        let no_turn_end = PeerFacts {
            reports: Reports::default(),
            ..f.clone()
        };
        assert_eq!(
            peer_state_of(&no_turn_end),
            Running,
            "no turn_end: never idle/done"
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                stalled_secs: Some(1),
                ..no_turn_end
            }),
            Stalled,
            "stalls are time facts (§3.6)"
        );
        assert_eq!(
            peer_state_of(&PeerFacts {
                dead: true,
                approval: true,
                ..f.clone()
            }),
            NotRunning
        );
        assert_eq!(peer_state_of(&PeerFacts { live: false, ..f }), NotRunning);
    }
}
