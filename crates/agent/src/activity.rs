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
use std::time::Instant;

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
        // ponytail: skeleton (155 C); W1 fills.
        self.delivered = Some(now);
        self.episode = None;
    }

    /// A `session/update` of this session arrived.
    pub fn update(&mut self, now: Instant, what: ChildUpdate<'_>) {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = what;
        self.first_update.get_or_insert(now);
        self.last_update = Some(now);
        self.episode = None;
    }

    /// The turn closed (`PromptDone`, cancel, harness closed): nothing to watch.
    pub fn turn_ended(&mut self, now: Instant) {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = now;
        self.delivered = None;
        self.first_update = None;
        self.last_update = None;
        self.tools_in_progress = 0;
        self.episode = None;
    }

    /// When the next stall check is due; `None` when nothing can fire (no
    /// turn, a tool call in progress, an episode already open, rule off).
    #[must_use]
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = (now, self.cfg);
        None
    }

    /// At or after the deadline: the wake to send, once per episode.
    pub fn fire(&mut self, now: Instant) -> Option<Stall> {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = now;
        None
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
    pub reports: acp::Reports,
}

/// The §5.1 fold: blocked > stalled > done > idle > running; `not_running`
/// is terminal and wins; without `reports.turn_end` idle/done stay running.
#[must_use]
pub fn peer_state_of(f: &PeerFacts) -> PeerStateWord {
    // ponytail: skeleton (155 C); W1 fills the table.
    let _ = f;
    PeerStateWord::Running
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
