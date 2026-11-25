//! Attention inbox (70 HUD, paperclip's attention service): things only the
//! user can decide, *derived* on every `attention_list` from what already
//! exists — permission prompts (subscribed [`ApprovalRequested`]), pending
//! `ask_user` questions ([`tools::PendingAsks`]), artifacts an agent showed
//! (subscribed [`ArtifactShown`], recorded in `attention_shows`), tasks
//! blocked on a `decision:` or a person, tasks in review with the user as
//! reviewer, proposed agents (the hire approval), budgets at 80 % / paused,
//! unreachable mesh peers, unresolved desktop hand-offs and unanswered secret
//! requests (79 §6.6, replicated rows). Only dismiss / snooze per item id is
//! stored (`attention_state`, engine-local); a dismissal hides the version the
//! user saw (`dismissed_ms >= at_ms`), a newer version surfaces again (119).
//! Badge count = the returned list's length.

use std::collections::{HashMap, HashSet};

use platform::time::now_ms;

use artifact::ArtifactShown;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved, AskRequested};

use crate::store::Store;
use crate::types::{AgentStatus, AgentSummary, Budget};

pub use crate::handoff::Handoff;

/// Registry name of the [`Attention`] actor.
pub const ATTENTION_ACTOR: &str = "attention";

/// Budget share that lands an agent in the inbox (67 §6.4 warning line).
pub const BUDGET_WARN: f64 = 0.8;

/// 207 E3: the detail of a `blocked:<task>` item whose only blockers are
/// `left:` ones — a parked decision the user left to the agent. Visible, low
/// (base class Notice), no alert; dismiss hides it, Unblock still works.
pub const LEFT_DECISION: &str = "Decision left to the agent; it will act when it next runs";

/// What kind of decision an item wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A tool call waits for allow / deny.
    Approval,
    /// A task is blocked on a `decision:<id>` — answer it.
    Decision,
    /// A proposed agent waits for hire / decline.
    Hire,
    /// A task is blocked on a person or another task.
    Blocked,
    /// A task in review with the user as reviewer — done or back.
    Review,
    /// An agent at ≥ 80 % of its month budget, or paused at the cap.
    Budget,
    /// A paired engine is unreachable.
    Node,
    /// An agent in a VM needs a human at its desktop (79 §6.6): open it, then Done / Cancel.
    Handoff,
    /// A session asked for a secret (79 §6.6): the masked card, one or several fields.
    Secret,
    /// A session asked the user a question (`ask_user`, 104 A1): answer or dismiss.
    Ask,
    /// An agent showed the user an artifact (104 A1): see it.
    Show,
    /// 109 §8.3: a session's interrupted turn needs a human — a replay
    /// skipped a mutating call (effect unknown), an ACP child got a hand-off,
    /// or `durability.resume: ask` waits for the Resume button.
    Resume,
    /// 126 §10: the double answered a card for the owner ("double answered
    /// Allow for `git push` · undo 14 s"), or the daily L3 digest.
    // 126 W1 produces items; W4 renders.
    Double,
    /// 130 E3: a root or desk session's turn ended cleanly — the receipt, with
    /// `summary` and the agent's `## Next` suggestions.
    Finished,
    /// 182 D9: a task's home engine is offline (or unknown) with tasks waiting
    /// on it (`node:<engine id>`, raised by the lease holder) — move them here,
    /// or keep waiting; nothing runs elsewhere on its own. S1: a task homed
    /// here whose agent lives elsewhere (`misplaced:<task>`).
    Home,
    /// 183 §7.6 (W-27): a contact's first task for an export waits — Allow once · Always ·
    /// Block (`contact_first_answer`). Id `contact_first:<contact>:<export>` (both names are
    /// `[a-z0-9_-]`); the title names the contact, its machine and the export.
    // 183 P3: E — derive these items from the waiting tasks.
    ContactFirst,
    /// A `show_web` call opened a live web page beside a chat (205): see it. Id
    /// `web_view:<tile id>` (the tile's `sha256(session ‖ origin)` prefix, 205 D4), so the same
    /// origin in the same chat is one item; it leaves when the tile closes.
    WebView,
}

/// One inbox item. `id` is stable across derivations (dismiss / snooze key).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionItem {
    /// `approval:<id>` · `ask:<id>` · `decision:<task>:<decision>` · `hire:<agent>` · `blocked:<task>` · `review:<task>` · `budget:<agent>` · `node:<engine>` (kind node or home) · `misplaced:<task>` · `handoff:<id>` · `secret:<id>` · `show:<call_id>` · `web_view:<tile id>` · `resume:<session>:<turn>` · `resume:boot:<ms>` · `finished:<session>:<turn>` · `double:<question>` · `contact_first:<contact>:<export>` · `contact_roster:<record>` · `autoresearch:{finished,paused,reverted,pair,launch}:…`.
    pub id: String,
    /// What.
    pub kind: AttentionKind,
    /// One line.
    pub title: String,
    /// Why / what it waits on.
    pub detail: Option<String>,
    /// ms since epoch: when the item arose (task update, approval time, …).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// The task, for task-born items.
    pub task: Option<tasks::TaskId>,
    /// The agent: hire / budget items, the task owner, else the desk the session belongs to (104 A3).
    pub agent: Option<String>,
    /// The session that produced the item, for every kind that has one (104 A3).
    pub session: Option<String>,
    /// The approval id, for approvals.
    pub approval: Option<ApprovalId>,
    /// The `decision:<id>` blocker, for decisions.
    pub decision: Option<String>,
    /// The packet key (`decision:<id>` | `review:<task>`) when one exists (101): `packet_get` reads it.
    pub packet: Option<String>,
    /// The pending question(s), for asks (104 A3): the screen renders at once.
    #[serde(default)]
    pub ask: Option<AskRequested>,
    /// The turn the item belongs to, when the source carries one (104 A3). No source does yet.
    #[serde(default)]
    pub turn: Option<u32>,
    /// 130 E2: ms since epoch the user has been waited on — the approval's
    /// arrival, the ask's arrival, the blocker's / review's `task_events`
    /// time when known, else `at_ms`. `None` for kinds that wait on nobody.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub waiting_since_ms: Option<u64>,
    /// 130 E3: the first line of the turn's final text, ≤ 160 chars (Finished items).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub summary: Option<String>,
    /// 130 E3: the agent's `## Next` list, ≤ 2 items of ≤ 80 chars (Finished items).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional))]
    pub next_steps: Vec<String>,
    /// 149 E6: the conversation the item sits in, for session-born kinds
    /// (ask, approval, finished, resume, blocked-with-session); `None` when
    /// the session is unknown or has no messages.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub context: Option<ItemContext>,
    /// 132 D4: the task's priority — task-born items and session items whose
    /// session is a task checkout; `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub priority: Option<tasks::Priority>,
    /// 132 D4: [`tasks::rank::rank`] of that task (`project_order` step + priority
    /// weight); `Priority::Medium.weight()` (5) for items without one. Higher first.
    #[serde(default)]
    pub rank: u32,
    /// 155 §7.1: the engine's alert decision for this item — set after
    /// `roster.alert_delay_ms` for kinds that alert; rpc blanks it per
    /// connection for the focused session and seen finishes (§7.3, §7.5).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub alert: Option<crate::alert::Alert>,
    /// 207 D3: the engine's score — the sum of `reasons`; the list is ordered by it (§3.4).
    #[serde(default)]
    pub score: u32,
    /// 207 D3: the score's contributors, points desc (ties in `AttentionSignal` order); `base` always present.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<AttentionReason>>", optional))]
    pub reasons: Vec<AttentionReason>,
    /// 207 R5: approvals — `ApprovalRequested.risk`; `None` for other kinds.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub risk: Option<tools::Risk>,
    /// 207 R5: approvals — the tool name (`ApprovalRequested.tool`); `None` for other kinds.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub tool: Option<String>,
    /// 207 R5: ms since epoch the request resolves itself (approval/ask/hand-off/secret timeout, a double's grace end).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub deadline_ms: Option<u64>,
    /// 207 R4: ms since epoch the item leaves the list on its own (`finished`: stop + `stopped_ttl_mins`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub expires_ms: Option<u64>,
    /// 207 D5: sessions stopped behind this item (transitive `awaiting` on its session).
    #[serde(default)]
    pub blocks: u32,
    /// 207 R3: the item's session is running / awaiting / in approval right now.
    #[serde(default)]
    pub session_live: bool,
    /// 207 R5: the session's effective project (its root's), else the task's; `None` when neither.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub project: Option<String>,
    /// 207 R5: the root of the item's session tree (itself for a root).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub root_session: Option<String>,
    /// 207 R5: the item's session's parent; `None` for a root.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent_session: Option<String>,
    /// 207 R5: display path, root first: the root's title, then each child's peer name; empty when the session is unknown.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional))]
    pub path: Vec<String>,
    /// 207 R4: `finished` items — how the turn stopped.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stop: Option<AttentionStop>,
}

impl AttentionItem {
    /// An item with every optional field at its default: `waiting_since_ms =
    /// Some(at_ms)`, `rank = Priority::Medium.weight()`, the rest empty. Sources
    /// build theirs as `AttentionItem { field: …, ..AttentionItem::new(…) }`.
    #[must_use]
    pub fn new(
        id: String,
        kind: AttentionKind,
        title: String,
        detail: Option<String>,
        at_ms: u64,
    ) -> Self {
        Self {
            id,
            kind,
            title,
            detail,
            at_ms,
            task: None,
            agent: None,
            session: None,
            approval: None,
            decision: None,
            packet: None,
            ask: None,
            turn: None,
            // 130 E2: every kind waits since it arose unless it knows better (asks) or waits on nobody (finished).
            waiting_since_ms: Some(at_ms),
            summary: None,
            next_steps: Vec::new(),
            context: None,
            priority: None,
            rank: tasks::Priority::Medium.weight(),
            alert: None,
            score: 0,
            reasons: Vec::new(),
            risk: None,
            tool: None,
            deadline_ms: None,
            expires_ms: None,
            blocks: 0,
            session_live: false,
            project: None,
            root_session: None,
            parent_session: None,
            path: Vec::new(),
            stop: None,
        }
    }
}

/// 207 D3: one contributor to an item's score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AttentionSignal {
    /// The kind's base (207 §3.1).
    Base,
    /// The item's session is live right now.
    Live,
    /// Sessions stopped behind the item.
    Blocks,
    /// The request resolves itself soon.
    Deadline,
    /// The approval's risk class.
    Risk,
    /// The task's priority and project rank.
    Task,
    /// Minutes waited.
    Age,
}

/// 207 D3: one line of an item's score breakdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionReason {
    /// Which signal.
    pub signal: AttentionSignal,
    /// Its points (> 0, except `base` which is always listed).
    pub points: u32,
    /// Short text for the card ("blocks 3 agents", "denies itself in 1:42"); a snapshot at derive time.
    pub text: String,
}

/// 207 D5: how a session's turn stopped (`finished` items).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    /// Any provider stop reason that is not one of the three below (`end_turn`, `stop`, `max_tokens`, …).
    Finished,
    /// `stop_reason == "error"`.
    Failed,
    /// `stop_reason == "cancelled"` (Stop, a parent's cancel, `max_runtime`).
    Cancelled,
    /// `stop_reason == "timed_out"` (the peer-work deadline).
    TimedOut,
}

impl StopOutcome {
    /// 207 §4: the outcome of a `TurnFinished.stop_reason` (not `interrupted` / `suspended`).
    #[must_use]
    pub fn of(stop_reason: &str) -> Self {
        match stop_reason {
            "error" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            _ => Self::Finished,
        }
    }

    /// The `attention_finished.outcome` column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }

    /// The inverse of [`Self::as_str`]; anything else reads as `Finished`.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            _ => Self::Finished,
        }
    }
}

/// 207 D5: the stop behind a `finished` item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionStop {
    /// Outcome class.
    pub outcome: StopOutcome,
    /// The raw `TurnFinished.stop_reason`.
    pub reason: String,
    /// The turn's last `AgentEvent::Error` message, cut to `SUMMARY_MAX` chars; `None` when there was none.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub error: Option<String>,
}

/// 207 §5.3: what a bulk call acts on — exactly one of `ids` (non-empty) or `filter`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionTarget {
    /// Item ids, in the caller's order.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ids: Vec<String>,
    /// Every listed item matching this, in engine order.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub filter: Option<AttentionFilter>,
}

/// 207 §5.3: a filter over the current list. Every part that is set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionFilter {
    /// Any of these kinds; empty = every kind.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub kinds: Vec<AttentionKind>,
    /// Any of these projects; `""` matches items without a project; empty = every project.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub projects: Vec<String>,
    /// Only items with `at_ms <= before_ms` (the newest the caller had seen); `None` = no bound.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub before_ms: Option<u64>,
}

impl AttentionFilter {
    /// Whether `item` matches every part that is set.
    #[must_use]
    pub fn matches(&self, item: &AttentionItem) -> bool {
        (self.kinds.is_empty() || self.kinds.contains(&item.kind))
            && (self.projects.is_empty()
                || self
                    .projects
                    .iter()
                    .any(|p| item.project.as_deref().unwrap_or("") == p.as_str()))
            && self.before_ms.is_none_or(|b| item.at_ms <= b)
    }
}

impl AttentionTarget {
    /// 207 §5.1: the ids this target names in `items` (the current list, engine
    /// order): `ids` as given, or every item matching `filter`. `Err` unless exactly one is set.
    pub fn resolve(&self, items: &[AttentionItem]) -> Result<Vec<String>, String> {
        match (self.ids.is_empty(), &self.filter) {
            (false, None) => Ok(self.ids.clone()),
            (true, Some(f)) => Ok(items
                .iter()
                .filter(|i| f.matches(i))
                .map(|i| i.id.clone())
                .collect()),
            (false, Some(_)) => Err("attention target: give ids or a filter, not both".into()),
            (true, None) => Err("attention target: give ids or a filter".into()),
        }
    }
}

/// 207 §5.2: what a bulk call did to one id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AttentionResolution {
    /// Hidden (119, versioned): review, hire, budget, node, home, blocked, show, finished, double, resume, contact_first, autoresearch:*.
    Hidden,
    /// Snoozed until the given time.
    Snoozed,
    /// A live tool call was told to decide itself (ask, live decision).
    LeftToAgent,
    /// Recorded for the agent's next wake; nothing woken (parked decision).
    LeftForLater,
    /// An approval the user left to the agent: the call was not run.
    NotRun,
    /// A secret request or hand-off resolved "none is coming, proceed without".
    ProceedWithout,
    /// Not in the list any more, or already resolved elsewhere.
    Gone,
    /// The resolver failed; nothing was written. See `error`.
    Failed,
}

/// 207 §5.3: one id's result of a bulk call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionOutcome {
    /// The item id.
    pub id: String,
    /// What happened.
    pub resolution: AttentionResolution,
    /// The resolver's message when `resolution` is `failed`.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub error: Option<String>,
}

impl AttentionOutcome {
    fn of(id: &str, resolution: AttentionResolution) -> Self {
        Self {
            id: id.to_owned(),
            resolution,
            error: None,
        }
    }

    fn failed(id: &str, error: impl std::fmt::Display) -> Self {
        Self {
            id: id.to_owned(),
            resolution: AttentionResolution::Failed,
            error: Some(error.to_string()),
        }
    }
}

/// 149 E6: the two bubbles behind an item — the user's last input to the
/// session and the agent's last assistant text before the item arose, each
/// ≤ [`CONTEXT_MAX`] chars (sentence-clipped, fences dropped); `None` when unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ItemContext {
    /// What the user last asked the session.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub request: Option<String>,
    /// The agent's last reply (the text after that input).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub last_reply: Option<String>,
}

/// 149 E6: cap of each [`ItemContext`] text in chars.
pub const CONTEXT_MAX: usize = 280;

/// 130 E1 / 207 §5.5: bus event the [`Attention`] actor publishes after a
/// derive whose result differs from the last published one (ids, order,
/// content, alerts); `count` is the new badge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct AttentionChanged {
    /// The new list's length (the badge).
    pub count: u32,
    /// Ids present now and not before, sorted.
    pub ids_added: Vec<String>,
    /// Ids present before and not now, sorted.
    pub ids_removed: Vec<String>,
    /// ms since epoch of the derive.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
    /// 207 §7: every item added or changed (content other than `score`/`reasons`/`alert`) since the previous
    /// event, with `alert` set to `None` (alerts are per connection; see `alerts_changed`).
    #[serde(default)]
    pub items: Vec<AttentionItem>,
    /// 207 §7: every listed id in engine order after this derive.
    #[serde(default)]
    pub order: Vec<String>,
    /// 207 §7: 1-based per engine run, +1 per published event; 0 = an engine before 207.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// 207 §7: ms since epoch the `Attention` actor started (a new value = a new run; `seq` restarts).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub epoch_ms: u64,
    /// 207 §7: some item's `alert` appeared, changed or went away: refetch `attention_list` (masked per connection).
    #[serde(default)]
    pub alerts_changed: bool,
}

impl wire::WireEvent for AttentionChanged {
    const TOPIC: &'static str = "roster.attention";
}

/// 130 E3 / 207 §4: one `attention_finished` row — a root / desk session's
/// last stopped turn; the item id is `finished:<session>:<turn>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// Owning session (one row per session: the newest turn replaces the last).
    pub session: String,
    /// The turn that stopped.
    pub turn: String,
    /// ms since epoch: when it stopped.
    pub at_ms: u64,
    /// First line of the final text (≤ 160 chars), else the error (failed / timed out), else the session title.
    pub summary: Option<String>,
    /// The `## Next` list, ≤ 2 × 80 chars.
    pub next_steps: Vec<String>,
    /// 207 §4: how the turn stopped.
    pub outcome: StopOutcome,
    /// 207 §4: the raw `TurnFinished.stop_reason`.
    pub stop_reason: String,
    /// 207 §4: the turn's last `AgentEvent::Error` message (≤ [`SUMMARY_MAX`] chars).
    pub error: Option<String>,
}

/// 130 E3: `summary` cap in chars.
pub const SUMMARY_MAX: usize = 160;
/// 130 E3: cap of each `next_steps` item in chars.
pub const NEXT_STEP_MAX: usize = 80;
/// 130 E3: how many `next_steps` an item carries at most.
pub const NEXT_STEPS_MAX: usize = 2;

/// One `attention_shows` row: an artifact an agent showed since this build (104 A1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The `show` call; the item id is `show:<call_id>`.
    pub call_id: String,
    /// Owning session.
    pub session: String,
    /// Card title.
    pub title: String,
    /// File name.
    pub name: String,
    /// `artifact::Kind::as_str`.
    pub kind: String,
    /// Mime type.
    pub mime: String,
    /// Blob size.
    pub bytes: u64,
    /// Per `(session, name)`, 1-based.
    pub version: u32,
    /// ms since epoch: when it was shown.
    pub at_ms: u64,
}

/// 109 §8.3: one resume that needs a human (an `Inputs` row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeNeed {
    /// Owning session.
    pub session: String,
    /// The open turn.
    pub turn: String,
    /// When the turn was interrupted (ms since epoch).
    pub at_ms: u64,
    /// What went wrong, from the `ResumeReport`; `None` under `resume: ask`,
    /// where the turn simply waits for the button; `"resume?"` for a takeover.
    pub detail: Option<String>,
    /// 139 §6.3 (D5): the engine the session was taken from — a system
    /// takeover found this open turn and left it for the button.
    pub taken_from: Option<String>,
}

/// `ask` → `Result<Vec<Handoff>, String>`: unresolved hand-offs, newest first (`handoffs_list`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListHandoffs;

/// `ask` → `Result<(), String>`: Done / Cancel on a hand-off (`handoff_resolve`).
/// `Err` when `outcome` is not `done` | `cancelled`, or the row is unknown / already resolved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolveHandoff {
    /// The row.
    pub id: String,
    /// `done` | `cancelled`.
    pub outcome: String,
}

/// `ask` → `Result<Vec<AttentionItem>, String>`: undismissed, unsnoozed, in engine order (207 §3.4).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAttention;

/// 126: another plugin's inbox items, pulled on every `ListAttention` (the
/// double's "answers Allow in 14 s — take over" rows). Items are shaped by the
/// source; dismiss / snooze apply to their ids like any other.
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    /// The source's current items.
    async fn items(&self) -> Vec<AttentionItem>;
}

/// `tell`: add a [`Source`] the inbox pulls from.
pub struct RegisterSource(pub std::sync::Arc<dyn Source>);

/// What `derive` needs; the actor gathers it, tests build it by hand.
#[derive(Default)]
pub struct Inputs {
    /// Open tasks (todo, doing, review, blocked).
    pub tasks: Vec<tasks::Task>,
    /// Their packets (101): `decision:<id>` and `review:<task>` rows.
    pub packets: Vec<tasks::packet::Packet>,
    /// The roster.
    pub agents: Vec<AgentSummary>,
    /// Pending permission prompts with when they arrived.
    pub approvals: Vec<(ApprovalRequested, u64)>,
    /// Mesh peers.
    pub peers: Vec<mesh::Peer>,
    /// Unresolved `handoffs` rows (79 §6.6).
    pub handoffs: Vec<Handoff>,
    /// Unanswered `secret_requests` rows (79 §6.6).
    pub secrets: Vec<secrets::SecretRequest>,
    /// Engine id → display name, this engine included (the `detail` of a hand-off).
    pub engines: HashMap<String, String>,
    /// Pending `ask_user` questions from every session (104 A1).
    pub asks: Vec<AskRequested>,
    /// Undismissed `attention_shows` rows (104 A1).
    pub shows: Vec<Shown>,
    /// 205: open `show_web` tiles (`artifact::webview::open_tiles`), one `web_view:<id>` item each.
    pub web_views: Vec<artifact::webview::OpenTile>,
    /// Session id → desk agent name, for a desk session and every session under it (104 A3).
    pub desks: HashMap<String, String>,
    /// Task id → newest `task_events.session`, for open tasks without a checkout (104 A3).
    pub task_sessions: HashMap<String, String>,
    /// 109 §8.3: resumes that need a human.
    pub resumes: Vec<ResumeNeed>,
    /// 146 D4: the latest boot pass counts; one `resume:boot:<at_ms>` item.
    pub boot: Option<agent::durability::BootPass>,
    /// Items from registered [`Source`]s (126), already shaped.
    pub external: Vec<AttentionItem>,
    /// 130 E2: ask id → ms since epoch the `AskRequested` arrived (the actor
    /// records it when a pending ask is first seen).
    pub ask_since: HashMap<tools::AskId, u64>,
    /// 130 E3: undismissed `attention_finished` rows.
    pub finished: Vec<Finished>,
    /// 132 D4: `tasks.project_order` from config, for [`tasks::rank::rank`].
    pub project_order: Vec<String>,
    /// 149 E6: session id → its last exchange, for the sessions behind
    /// session-born items (the actor reads the agent store; tests build it).
    pub session_context: HashMap<String, ItemContext>,
    /// 182 D9: engines offline past `tasks.home_offline_ask_secs` with tasks
    /// waiting on them — gathered only while this engine holds the lease.
    pub homes_offline: Vec<tasks::HomeOffline>,
    /// 182 S1: open tasks homed here whose agent this engine lacks.
    pub misplaced: Vec<Misplaced>,
    /// 207: session id → where it sits (from `ListSessions`).
    pub lineage: HashMap<String, Lineage>,
    /// 207: sessions live now (`ListActivity` state running/awaiting/approval, or `SessionSummary.running`).
    pub live: HashSet<String>,
    /// 207 D5: session id → sessions stopped behind it.
    pub blocks: HashMap<String, u32>,
    /// 207: item id → deadline ms (approval, ask, hand-off, secret).
    pub deadlines: HashMap<String, u64>,
    /// 207: weights and timers.
    pub config: crate::config::AttentionConfig,
}

/// 207: where a session sits in its tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lineage {
    /// The effective project (the root's).
    pub project: Option<String>,
    /// The root session's id (itself for a root).
    pub root: String,
    /// The parent session's id; `None` for a root.
    pub parent: Option<String>,
    /// Display path, root first.
    pub path: Vec<String>,
}

/// 207: lineage of every listed session; `desks` = session → desk agent name (for a desk root's title).
/// Path segments: the root's trimmed title, else `<desk> desk` for a desk root, else the first 8
/// chars of its id; every child's peer name, else its title, else 8 chars. Bounded walk (32 hops).
pub fn lineage_map(
    sessions: &[agent::SessionSummary],
    desks: &HashMap<String, String>,
) -> HashMap<String, Lineage> {
    let by_id: HashMap<runtime::SessionId, &agent::SessionSummary> =
        sessions.iter().map(|s| (s.id, s)).collect();
    let short = |s: &agent::SessionSummary| s.id.to_string().chars().take(8).collect::<String>();
    let nonempty = |t: &str| Some(t.trim().to_owned()).filter(|t| !t.is_empty());
    sessions
        .iter()
        .map(|s| {
            let mut chain: Vec<&agent::SessionSummary> = vec![s];
            let mut cur = s;
            // ponytail: bounded walk; the tree is shallow and a parent cycle is a store bug.
            for _ in 0..32 {
                let Some(&p) = cur.parent.and_then(|p| by_id.get(&p)) else {
                    break;
                };
                chain.push(p);
                cur = p;
            }
            chain.reverse();
            let root = chain[0];
            let id = root.id.to_string();
            let mut path = vec![
                nonempty(&root.title)
                    .or_else(|| {
                        root.desk
                            .clone()
                            .or_else(|| desks.get(&id).cloned())
                            .map(|d| format!("{d} desk"))
                    })
                    .unwrap_or_else(|| short(root)),
            ];
            path.extend(chain[1..].iter().map(|&c| {
                c.name
                    .as_deref()
                    .and_then(nonempty)
                    .or_else(|| nonempty(&c.title))
                    .unwrap_or_else(|| short(c))
            }));
            (
                s.id.to_string(),
                Lineage {
                    project: s.project.clone(),
                    root: id,
                    parent: s.parent.map(|p| p.to_string()),
                    path,
                },
            )
        })
        .collect()
}

/// 207 D5: session → count of distinct sessions transitively awaiting it.
/// An `Awaiting { from }` row waits on its parent (`"parent"`), a child or a
/// sibling of that peer name, or `session:<uuid>`; anything else is unresolved.
pub fn blocks_map(rows: &[agent::AgentInfo]) -> HashMap<String, u32> {
    let mut edges: HashMap<runtime::SessionId, Vec<runtime::SessionId>> = HashMap::new();
    for t in rows {
        let agent::AgentState::Awaiting { from } = &t.state else {
            continue;
        };
        let target = if from == "parent" {
            (t.parent != t.id).then_some(t.parent)
        } else if let Some(id) = from.strip_prefix("session:") {
            id.parse::<runtime::SessionId>().ok()
        } else {
            rows.iter()
                .find(|r| r.parent == t.id && r.id != t.id && r.name == *from)
                .or_else(|| {
                    rows.iter().find(|r| {
                        t.parent != t.id && r.parent == t.parent && r.id != t.id && r.name == *from
                    })
                })
                .map(|r| r.id)
        };
        if let Some(target) = target {
            edges.entry(t.id).or_default().push(target);
        }
    }
    let mut reached: HashMap<runtime::SessionId, HashSet<runtime::SessionId>> = HashMap::new();
    for &start in edges.keys() {
        let mut seen: HashSet<runtime::SessionId> = HashSet::new();
        let mut stack = edges[&start].clone();
        while let Some(s) = stack.pop() {
            if s == start || !seen.insert(s) {
                continue;
            }
            reached.entry(s).or_default().insert(start);
            if let Some(next) = edges.get(&s) {
                stack.extend(next.iter().copied());
            }
        }
    }
    reached
        .into_iter()
        .map(|(s, by)| (s.to_string(), by.len() as u32))
        .collect()
}

/// 207: the live set — rows running, awaiting or in approval, and every session the store marks running.
pub fn live_set(rows: &[agent::AgentInfo], sessions: &[agent::SessionSummary]) -> HashSet<String> {
    rows.iter()
        .filter(|r| {
            matches!(
                r.state,
                agent::AgentState::Running
                    | agent::AgentState::Awaiting { .. }
                    | agent::AgentState::Approval { .. }
            )
        })
        .map(|r| r.id.to_string())
        .chain(
            sessions
                .iter()
                .filter(|s| s.running)
                .map(|s| s.id.to_string()),
        )
        .collect()
}

/// 182 S1: a task homed on this engine whose owner agent lives elsewhere.
#[derive(Clone, Debug)]
pub struct Misplaced {
    /// The task.
    pub task: tasks::Task,
    /// Its owner agent's name.
    pub agent: String,
    /// This engine's display name.
    pub here: String,
    /// Display names of the engines whose roster has the agent.
    pub there: Vec<String>,
}

/// Dismissed / snoozed state per item id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// When it was dismissed; hides the item while its `at_ms` is not newer
    /// (119: a re-submitted review / updated task is a new version and surfaces).
    pub dismissed_ms: Option<u64>,
    /// Hidden until then.
    pub snoozed_until_ms: Option<u64>,
}

impl State {
    fn hidden(self, now_ms: u64, at_ms: u64) -> bool {
        self.dismissed_ms.is_some_and(|d| d >= at_ms)
            || self.snoozed_until_ms.is_some_and(|u| u > now_ms)
    }
}

/// The inbox from its inputs: pure, hidden and expired items removed, every
/// item scored and the list ordered by score (207 §3.4).
pub fn derive(inputs: &Inputs, states: &HashMap<String, State>, now_ms: u64) -> Vec<AttentionItem> {
    let mut out = Vec::new();
    let item = AttentionItem::new;
    // 132 D4: a task's (priority, rank); session items borrow their checkout task's.
    let ranked = |t: &tasks::Task| {
        (
            Some(t.priority),
            tasks::rank::rank(t.priority, t.project.as_deref(), &inputs.project_order),
        )
    };
    let checkouts: HashMap<&str, &tasks::Task> = inputs
        .tasks
        .iter()
        .filter_map(|t| t.checkout.as_ref().map(|c| (c.session.as_str(), t)))
        .collect();
    let by_session = |session: &str| {
        checkouts
            .get(session)
            .map_or((None, tasks::Priority::Medium.weight()), |t| ranked(t))
    };
    // 104 A3: the desk a session belongs to (itself or an ancestor), when the roster knows it.
    let desk = |session: &str| inputs.desks.get(session).cloned();
    // 79 §6.6: who = the agent's title, else the session's place in its tree (207), else its short id.
    let who = |agent: Option<&str>, session: &str| -> String {
        agent
            .and_then(|n| inputs.agents.iter().find(|a| a.file.name.as_str() == n))
            .map(|a| a.file.title.clone())
            .or_else(|| agent.map(str::to_owned))
            .or_else(|| {
                inputs
                    .lineage
                    .get(session)
                    .and_then(|l| l.path.last().cloned())
            })
            .unwrap_or_else(|| session.chars().take(8).collect())
    };

    for (a, at_ms) in &inputs.approvals {
        let args = match &a.args {
            serde_json::Value::Object(m) => {
                m.get("command").and_then(|v| v.as_str()).map(str::to_owned)
            }
            _ => None,
        };
        let session = a.session.to_string();
        let agent = desk(&session);
        let (priority, rank) = by_session(&session);
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(session.clone()),
            approval: Some(a.id),
            priority,
            rank,
            risk: Some(a.risk),
            tool: Some(a.tool.clone()),
            ..item(
                format!("approval:{}", a.id.0),
                AttentionKind::Approval,
                format!(
                    "{} wants to run {}",
                    who(agent.as_deref(), &session),
                    a.tool
                ),
                args,
                *at_ms,
            )
        });
    }

    for a in &inputs.asks {
        // 105: an escalation to the user raises BOTH a `decision:` blocker (the Decision
        // item below) and, for the channels, an ask card on the calling session whose
        // question id is that same `decision:<id>` key — one inbox item, not two.
        if a.questions.iter().any(|q| q.id.starts_with("decision:")) {
            continue;
        }
        let session = a.session.to_string();
        let agent = desk(&session);
        let first = a.questions.first();
        let detail = if a.questions.len() > 1 {
            format!("{} questions", a.questions.len())
        } else {
            tasks::packet::head(
                &first
                    .map(|q| {
                        q.options
                            .iter()
                            .map(|c| c.label.as_str())
                            .collect::<Vec<_>>()
                            .join(" \u{b7} ")
                    })
                    .unwrap_or_default(),
            )
        };
        let (priority, rank) = by_session(&session);
        // 207 / 119: the first-seen time, stable across derives, so a dismissal sticks.
        let since = inputs.ask_since.get(&a.id).copied().unwrap_or(now_ms);
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(session.clone()),
            ask: Some(a.clone()),
            priority,
            rank,
            ..item(
                format!("ask:{}", a.id.0),
                AttentionKind::Ask,
                format!(
                    "{} asks: {}",
                    who(agent.as_deref(), &session),
                    tasks::packet::head(first.map_or("", |q| q.question.as_str()))
                ),
                Some(detail).filter(|d| !d.is_empty()),
                since,
            )
        });
    }

    for s in &inputs.shows {
        let agent = desk(&s.session);
        let (priority, rank) = by_session(&s.session);
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(s.session.clone()),
            priority,
            rank,
            ..item(
                format!("show:{}", s.call_id),
                AttentionKind::Show,
                format!(
                    "{} showed you {}",
                    who(agent.as_deref(), &s.session),
                    s.title
                ),
                Some(format!(
                    "{} \u{b7} {} \u{b7} v{}",
                    s.kind,
                    platform::fmt::bytes(s.bytes),
                    s.version
                )),
                s.at_ms,
            )
        });
    }

    // 205: a live page an agent opened beside a chat; informational, waits on nobody.
    for w in &inputs.web_views {
        let session = w.session.to_string();
        let agent = desk(&session);
        let (priority, rank) = by_session(&session);
        let host = Some(w.host.clone()).filter(|h| !h.is_empty());
        let title = Some(w.title.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .or_else(|| host.clone())
            .unwrap_or_else(|| "web page".to_owned());
        out.push(AttentionItem {
            agent,
            session: Some(session),
            waiting_since_ms: None,
            priority,
            rank,
            ..item(
                format!("web_view:{}", w.id),
                AttentionKind::WebView,
                title,
                Some(w.url.clone()).filter(|u| !u.is_empty()),
                w.opened_ms,
            )
        });
    }

    let packets: HashMap<&str, &tasks::packet::Packet> =
        inputs.packets.iter().map(|p| (p.key.as_str(), p)).collect();
    // Hire tasks owned by the user are the proposal's approval: one item per agent, not two.
    let hire_prefix = "Hire agent ";
    for t in &inputs.tasks {
        let owner_agent = match &t.owner {
            tasks::Owner::Agent(a) => Some(a.clone()),
            _ => None,
        };
        // 104 A3: the working session, else the newest task event that named one.
        let session = t
            .checkout
            .as_ref()
            .map(|c| c.session.clone())
            .or_else(|| inputs.task_sessions.get(&t.id.0).cloned());
        let (priority, rank) = ranked(t);
        match t.state {
            tasks::TaskState::Blocked => {
                // 106 F-2: the newest decision addressed to the user (`to` =
                // `user`, or unset on legacy / packet-less rows); a decision
                // addressed to an agent is not the user's and makes no item.
                let decision = t
                    .blockers
                    .iter()
                    .rev()
                    .filter(|b| b.starts_with("decision:"))
                    .find(|b| {
                        packets
                            .get(b.as_str())
                            .is_none_or(|p| p.to.as_deref().is_none_or(|to| to == "user"))
                    });
                if let Some(d) = decision {
                    let packet = packets.get(d.as_str());
                    // 132 D1: the row leads with the question, sentence-clipped; the byline is the detail.
                    let question = packet
                        .and_then(|p| p.question.as_deref())
                        .map(str::trim)
                        .filter(|q| !q.is_empty());
                    let (title, detail) = match (packet, question) {
                        (Some(p), Some(q)) => (
                            tasks::packet::head_sentence(q, tasks::packet::ROW_BUDGET),
                            format!("{} \u{b7} {}", p.by, t.title),
                        ),
                        _ => (
                            format!(
                                "{} asks for a decision on \u{201c}{}\u{201d}",
                                packet.map_or_else(|| t.owner.to_string(), |p| p.by.clone()),
                                t.title
                            ),
                            packet.map_or_else(
                                || "the question is in the task thread".to_owned(),
                                |p| tasks::packet::head(&p.summary),
                            ),
                        ),
                    };
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        session: session.clone(),
                        decision: Some((*d).clone()),
                        packet: packet.map(|p| p.key.clone()),
                        priority,
                        rank,
                        ..item(
                            format!("decision:{}:{d}", t.id),
                            AttentionKind::Decision,
                            title,
                            Some(detail),
                            t.updated_ms,
                        )
                    });
                } else if t
                    .blockers
                    .iter()
                    .any(|b| !b.starts_with("decision:") && !b.starts_with(tasks::LEFT_PREFIX))
                {
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        session: session.clone(),
                        priority,
                        rank,
                        ..item(
                            format!("blocked:{}", t.id),
                            AttentionKind::Blocked,
                            format!("\u{201c}{}\u{201d} is blocked", t.title),
                            Some(format!("on {}", t.blockers.join(", "))),
                            t.updated_ms,
                        )
                    });
                } else if !t.blockers.is_empty()
                    && t.blockers.iter().all(|b| b.starts_with(tasks::LEFT_PREFIX))
                {
                    // 207 E3: a parked decision left to the agent wakes no one;
                    // it stays visible as a low notice until the agent runs.
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        session: session.clone(),
                        priority,
                        rank,
                        ..item(
                            format!("blocked:{}", t.id),
                            AttentionKind::Blocked,
                            format!("\u{201c}{}\u{201d}: decision left to the agent", t.title),
                            Some(LEFT_DECISION.to_owned()),
                            t.updated_ms,
                        )
                    });
                }
            }
            tasks::TaskState::Review if t.reviewer == Some(tasks::Owner::User) => {
                let key = tasks::packet::review_key(&t.id);
                let packet = packets.get(key.as_str());
                // 132 D1: the row leads with the outcome (the summary's first sentence).
                let (title, detail) = match packet.filter(|p| !p.summary.trim().is_empty()) {
                    Some(p) => (
                        tasks::packet::head_sentence(&p.summary, tasks::packet::ROW_BUDGET),
                        format!("\u{201c}{}\u{201d} \u{b7} by {}", t.title, p.by),
                    ),
                    None => (
                        format!("\u{201c}{}\u{201d} awaits your review", t.title),
                        format!("by {}", t.owner),
                    ),
                };
                out.push(AttentionItem {
                    task: Some(t.id.clone()),
                    agent: owner_agent.clone(),
                    session: session.clone(),
                    packet: packet.map(|_| key.clone()),
                    priority,
                    rank,
                    ..item(
                        key,
                        AttentionKind::Review,
                        title,
                        Some(detail),
                        // 119: a re-submitted packet is a new version even when the task row did not change.
                        packet.map_or(t.updated_ms, |p| p.at_ms.max(t.updated_ms)),
                    )
                });
            }
            _ => {}
        }
    }

    for a in &inputs.agents {
        let name = a.file.name.as_str().to_owned();
        if a.status == AgentStatus::Proposed || a.file.proposed_by.is_some() {
            let hire_task = inputs.tasks.iter().find(|t| {
                t.title.starts_with(hire_prefix)
                    && t.title[hire_prefix.len()..].starts_with(&name)
                    && !matches!(
                        t.state,
                        tasks::TaskState::Done | tasks::TaskState::Cancelled
                    )
            });
            // 104 A3: the proposing agent's open desk is the nearest thing to the proposing session.
            let session = a
                .file
                .proposed_by
                .as_deref()
                .and_then(|p| p.strip_prefix("agent:"))
                .and_then(|n| inputs.agents.iter().find(|x| x.file.name.as_str() == n))
                .and_then(|x| x.desk.clone());
            let (priority, rank) =
                hire_task.map_or((None, tasks::Priority::Medium.weight()), ranked);
            out.push(AttentionItem {
                task: hire_task.map(|t| t.id.clone()),
                agent: Some(name.clone()),
                session,
                priority,
                rank,
                ..item(
                    format!("hire:{name}"),
                    AttentionKind::Hire,
                    format!(
                        "{} proposes hiring {} ({})",
                        a.file
                            .proposed_by
                            .clone()
                            .unwrap_or_else(|| "someone".into()),
                        name,
                        a.file.title
                    ),
                    Some(format!(
                        "reports to {} \u{b7} budget {}",
                        a.file.reports_to, a.file.budget
                    )),
                    hire_task.map_or(now_ms, |t| t.created_ms),
                )
            });
            continue;
        }
        if let Budget::Monthly { amount, currency } = &a.file.budget
            && *amount > 0.0
            && a.spent_month / amount >= BUDGET_WARN
        {
            let pct = (a.spent_month / amount * 100.0).round();
            let detail = if a.file.paused {
                "paused at the cap; un-pause to override for the month"
            } else {
                "at 80 %: the cap pauses it"
            };
            // 132 D8: the crossing time is when the roster filed the budget task for it.
            let crossed = inputs
                .tasks
                .iter()
                .filter(|t| {
                    t.title
                        .strip_prefix("Budget: ")
                        .or_else(|| t.title.strip_prefix("Budget override: "))
                        .is_some_and(|rest| rest.starts_with(&format!("{name} ")))
                })
                .map(|t| t.created_ms)
                .max()
                .unwrap_or(now_ms);
            out.push(AttentionItem {
                agent: Some(name.clone()),
                ..item(
                    format!("budget:{name}"),
                    AttentionKind::Budget,
                    format!(
                        "{name} spent {:.2} of {amount:.2} {currency} ({pct:.0} %)",
                        a.spent_month
                    ),
                    Some(detail.into()),
                    crossed,
                )
            });
        }
    }

    // 182 D9: one item per unreachable engine, id `node:<id>` — kind `home`
    // when tasks wait on it (the lease holder's view, past
    // `home_offline_ask_secs`): move them here, or keep waiting; nothing moves
    // on its own. Kind `node` otherwise. S3: an unknown home gets the `home` item.
    for p in inputs.peers.iter().filter(|p| !p.reachable) {
        if inputs.homes_offline.iter().any(|h| h.engine == p.id.0) {
            continue;
        }
        out.push(item(
            format!("node:{}", p.id.0),
            AttentionKind::Node,
            format!("{} is unreachable", p.name),
            Some(format!("last seen {}", stamp(p.last_seen_ms))),
            p.last_seen_ms,
        ));
    }
    for h in &inputs.homes_offline {
        let stuck: Vec<String> = h
            .tasks
            .iter()
            .filter_map(|t| match &t.owner {
                tasks::Owner::Agent(a)
                    if a != crate::AgentName::DEFAULT
                        && !inputs.agents.iter().any(|s| s.file.name.as_str() == a) =>
                {
                    Some(format!("{} (no {a} agent here)", t.title))
                }
                _ => None,
            })
            .collect();
        let n = h.tasks.len();
        let mut waiting = h
            .tasks
            .iter()
            .take(5)
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if n > 5 {
            waiting.push_str(&format!(" and {} more", n - 5));
        }
        let advice = if stuck.is_empty() {
            "Nothing runs elsewhere while it is off. Move them here, or keep waiting.".to_owned()
        } else {
            format!("Not movable here: {}.", stuck.join("; "))
        };
        out.push(item(
            format!("node:{}", h.engine),
            AttentionKind::Home,
            format!(
                "{} is offline — {n} {} waiting",
                h.name,
                if n == 1 { "task is" } else { "tasks are" },
            ),
            Some(format!("Waiting: {waiting}. {advice}")),
            h.since_ms,
        ));
    }
    // 182 S1: a task homed here whose agent lives on another engine waits for a move.
    for m in &inputs.misplaced {
        let title = match m.there.as_slice() {
            [] => format!("{} isn't on {}, or on any machine", m.agent, m.here),
            there => format!(
                "{} isn't on {}; it's on {}",
                m.agent,
                m.here,
                there.join(", ")
            ),
        };
        out.push(AttentionItem {
            task: Some(m.task.id.clone()),
            agent: Some(m.agent.clone()),
            ..item(
                format!("misplaced:{}", m.task.id),
                AttentionKind::Home,
                title,
                Some("Move it there with Move to \u{25b8}".to_owned()),
                m.task.updated_ms,
            )
        });
    }

    let engine_name = |id: &str| {
        inputs
            .engines
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    };

    for h in &inputs.handoffs {
        let (priority, rank) = by_session(&h.session);
        out.push(AttentionItem {
            agent: h.agent.clone(),
            session: Some(h.session.clone()),
            priority,
            rank,
            ..item(
                format!("handoff:{}", h.id),
                AttentionKind::Handoff,
                format!(
                    "{} needs you: {}",
                    who(h.agent.as_deref(), &h.session),
                    h.reason
                ),
                Some(engine_name(&h.engine)),
                h.created_ms,
            )
        });
    }

    for r in &inputs.secrets {
        let what = r.title.clone().unwrap_or_else(|| {
            r.fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let agent = desk(&r.session);
        let (priority, rank) = by_session(&r.session);
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(r.session.clone()),
            priority,
            rank,
            ..item(
                format!("secret:{}", r.id.0),
                AttentionKind::Secret,
                format!(
                    "{} asks for a secret: {what}",
                    who(agent.as_deref(), &r.session)
                ),
                Some(engine_name(&r.engine)),
                r.created_ms,
            )
        });
    }

    for r in &inputs.resumes {
        let agent = desk(&r.session);
        let name = who(agent.as_deref(), &r.session);
        let title = match (&r.taken_from, &r.detail) {
            (Some(from), _) => format!("{name} was taken from {from}"),
            (None, Some(_)) => format!("{name} resumed after a restart with gaps"),
            (None, None) => format!("{name} was interrupted by a restart — resume?"),
        };
        let (priority, rank) = by_session(&r.session);
        out.push(AttentionItem {
            agent,
            session: Some(r.session.clone()),
            priority,
            rank,
            ..item(
                format!("resume:{}:{}", r.session, r.turn),
                AttentionKind::Resume,
                title,
                r.detail.clone(),
                r.at_ms,
            )
        });
    }
    // 146 D4: one restart-recovery notice per boot, live while respawns run;
    // a boot that resumed nothing and lost nothing says nothing.
    if let Some(b) = inputs
        .boot
        .as_ref()
        .filter(|b| b.resuming + b.respawned + b.waiting > 0 || !b.lost.is_empty())
    {
        let title = boot_title(b);
        let detail = (!b.lost.is_empty()).then(|| {
            "lost children were told to their parents (<child_lost>); resume one with send_message or spawn a fresh child with a RESUME brief".to_owned()
        });
        out.push(AttentionItem {
            // The first lost child: the HUD's Dismiss needs a session (kept when lost > 0).
            session: b.lost.first().map(|(s, _)| s.to_string()),
            ..item(
                format!("resume:boot:{}", b.at_ms),
                AttentionKind::Resume,
                title,
                detail,
                b.at_ms,
            )
        });
    }
    // 207 §8: a Source that left `priority` empty borrows its session's checkout task.
    out.extend(inputs.external.iter().cloned().map(|mut i| {
        if i.priority.is_none()
            && let Some(s) = i.session.as_deref()
        {
            (i.priority, i.rank) = by_session(s);
        }
        i
    }));

    // 130 E3 / 207 §4: one receipt per root / desk session for every stop; waits on nobody.
    let ttl_ms = inputs.config.stopped_ttl_mins.saturating_mul(60_000);
    for f in &inputs.finished {
        let agent = desk(&f.session);
        let name = who(agent.as_deref(), &f.session);
        let (priority, rank) = by_session(&f.session);
        let title = match f.outcome {
            StopOutcome::Finished => format!("{name} finished"),
            StopOutcome::Failed => format!("{name} failed"),
            StopOutcome::Cancelled => format!("{name} was cancelled"),
            StopOutcome::TimedOut => format!("{name} timed out"),
        };
        let detail = f.summary.clone().or_else(|| {
            (f.outcome == StopOutcome::Failed)
                .then(|| f.error.clone())
                .flatten()
        });
        out.push(AttentionItem {
            agent,
            session: Some(f.session.clone()),
            waiting_since_ms: None,
            summary: f.summary.clone(),
            next_steps: f.next_steps.clone(),
            priority,
            rank,
            expires_ms: Some(f.at_ms.saturating_add(ttl_ms)),
            stop: Some(AttentionStop {
                outcome: f.outcome,
                reason: f.stop_reason.clone(),
                error: f.error.clone(),
            }),
            ..item(
                format!("finished:{}:{}", f.session, f.turn),
                AttentionKind::Finished,
                title,
                detail,
                f.at_ms,
            )
        });
    }

    // 149 E6: the exchange behind every session-born item, by session.
    for i in out.iter_mut().filter(|i| context_kind(i.kind)) {
        i.context = i
            .session
            .as_deref()
            .and_then(|s| inputs.session_context.get(s))
            .filter(|c| **c != ItemContext::default())
            .cloned();
    }

    // 207 R3/R5/D5: where each item sits, whether its session is live, what waits on it, its deadline.
    let task_project: HashMap<&str, Option<&str>> = inputs
        .tasks
        .iter()
        .chain(inputs.misplaced.iter().map(|m| &m.task))
        .map(|t| (t.id.0.as_str(), t.project.as_deref()))
        .collect();
    for i in &mut out {
        if let Some(d) = inputs.deadlines.get(&i.id) {
            i.deadline_ms = Some(*d);
        }
        let Some(s) = i.session.as_deref() else {
            i.project = i
                .task
                .as_ref()
                .and_then(|t| task_project.get(t.0.as_str()).copied().flatten())
                .map(str::to_owned);
            continue;
        };
        match inputs.lineage.get(s) {
            Some(l) => {
                i.project = l.project.clone();
                i.root_session = Some(l.root.clone());
                i.parent_session = l.parent.clone();
                i.path = l.path.clone();
            }
            None => {
                i.project = i
                    .task
                    .as_ref()
                    .and_then(|t| task_project.get(t.0.as_str()).copied().flatten())
                    .map(str::to_owned);
            }
        }
        // Task-born items are live only through the task's own checkout session.
        i.session_live = inputs.live.contains(s)
            && i.task
                .as_ref()
                .is_none_or(|t| checkouts.get(s).is_some_and(|c| c.id == *t));
        i.blocks = inputs.blocks.get(s).copied().unwrap_or(0);
    }

    out.retain(|i| {
        !states
            .get(&i.id)
            .copied()
            .unwrap_or_default()
            .hidden(now_ms, i.at_ms)
            && i.expires_ms.is_none_or(|e| e > now_ms)
    });
    for i in &mut out {
        (i.score, i.reasons) = crate::score::score(i, &inputs.config.weights, now_ms);
    }
    crate::score::order(&mut out);
    out
}

/// 149 E6: the kinds that carry an [`ItemContext`] — the ones born of a session's turn.
fn context_kind(kind: AttentionKind) -> bool {
    matches!(
        kind,
        AttentionKind::Ask
            | AttentionKind::Approval
            | AttentionKind::Finished
            | AttentionKind::Resume
            | AttentionKind::Blocked
    )
}

/// 149 E6: one bubble's text — fenced code blocks dropped, whitespace
/// collapsed to one line, cut to [`CONTEXT_MAX`] on a sentence end (else a
/// word) as [`tasks::packet::head_sentence`]; `None` when nothing is left.
pub fn clip_context(text: &str) -> Option<String> {
    let mut in_fence = false;
    let kept = text
        .lines()
        .filter(|l| {
            if l.trim_start().starts_with("```") {
                in_fence = !in_fence;
                return false;
            }
            !in_fence
        })
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    Some(tasks::packet::head_sentence(&kept, CONTEXT_MAX)).filter(|s| !s.is_empty())
}

/// 130 E3: `(summary, next_steps)` from a turn's final assistant text — a
/// trailing `## Next` section's first [`NEXT_STEPS_MAX`] list items (`- ` or
/// `1.`), each cut to [`NEXT_STEP_MAX`] chars; the summary is the first
/// non-empty line of what precedes it, cut to [`SUMMARY_MAX`], else `fallback`.
pub fn parse_finished(text: &str, fallback: Option<&str>) -> (Option<String>, Vec<String>) {
    let cut = |s: &str, max: usize| -> String {
        let s = s.trim();
        if s.chars().count() <= max {
            s.to_owned()
        } else {
            let mut out: String = s.chars().take(max - 1).collect();
            out.push('\u{2026}');
            out
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let (body, next) = match lines
        .iter()
        .rposition(|l| l.trim().eq_ignore_ascii_case("## next"))
    {
        Some(i) => (&lines[..i], &lines[i + 1..]),
        None => (&lines[..], &lines[..0]),
    };
    let next_steps: Vec<String> = next
        .iter()
        .map(|l| l.trim())
        .filter_map(|l| {
            l.strip_prefix("- ")
                .or_else(|| l.strip_prefix("* "))
                .or_else(|| {
                    let digits = l.trim_start_matches(|c: char| c.is_ascii_digit());
                    (digits.len() < l.len())
                        .then(|| digits.strip_prefix(". "))
                        .flatten()
                })
        })
        .filter(|l| !l.trim().is_empty())
        .take(NEXT_STEPS_MAX)
        .map(|l| cut(l, NEXT_STEP_MAX))
        .collect();
    let summary = body
        .iter()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map(|l| cut(l, SUMMARY_MAX))
        .or_else(|| fallback.map(|f| cut(f, SUMMARY_MAX)))
        .filter(|s| !s.is_empty());
    (summary, next_steps)
}

fn stamp(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map_or_else(
        || "never".into(),
        |d| d.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

/// 146 D4: "Restarted 14:12 · 3 sessions resumed · 2 subagents back · 1 restarted fresh · 1 waiting · 3 lost (a, b, c)".
pub fn boot_title(b: &agent::durability::BootPass) -> String {
    let plural = |n: u32, one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            many.to_owned()
        }
    };
    let when = chrono::DateTime::from_timestamp_millis(b.at_ms as i64).map_or_else(
        || "?".into(),
        |d| d.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    );
    let mut parts = vec![format!("Restarted {when}")];
    if b.resuming > 0 {
        parts.push(format!(
            "{} {} resumed",
            b.resuming,
            plural(b.resuming, "session", "sessions")
        ));
    }
    if b.respawned > b.fresh {
        let back = b.respawned - b.fresh;
        parts.push(format!(
            "{} {} back",
            back,
            plural(back, "subagent", "subagents")
        ));
    }
    if b.fresh > 0 {
        parts.push(format!("{} restarted fresh", b.fresh));
    }
    if b.waiting > 0 {
        parts.push(format!("{} waiting", b.waiting));
    }
    if !b.lost.is_empty() {
        let names: Vec<&str> = b.lost.iter().map(|(_, n)| n.as_str()).collect();
        parts.push(format!("{} lost ({})", b.lost.len(), names.join(", ")));
    }
    parts.join(" · ")
}

/// 146 D4: `tell` the actor sends itself 60 s after a clean pass settled.
struct ClearBoot(u64);

/// 146 D4: how long a clean boot notice stays after the last respawn settled.
const BOOT_CLEAR: std::time::Duration = std::time::Duration::from_secs(60);

/// The [`ATTENTION_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Attention {
    pool: SqlitePool,
    pending: HashMap<ApprovalId, (ApprovalRequested, u64)>,
    /// 109 §8.3: `ResumeReport`s that need a human, by session; a dismissed
    /// one is hidden by `attention_state`, a new turn replaces it.
    reports: HashMap<String, ResumeNeed>,
    /// `durability.resume: ask`: every open turn is an item with a Resume button.
    resume_ask: bool,
    /// 126: other plugins' item sources, pulled on every list.
    sources: Vec<std::sync::Arc<dyn Source>>,
    /// 130 E1: the bus, for `AttentionChanged`.
    rt: Runtime,
    /// 207 §5.5: the list as last published (alerts included); the next derive diffs against it.
    last: Vec<AttentionItem>,
    /// 207 §5.5: events published by this run; the next one carries `seq + 1`.
    seq: u64,
    /// 207 §5.5: ms since epoch this actor started (the run's identity on the wire).
    epoch_ms: u64,
    /// 207 §4: the last `AgentEvent::Error` message of each running turn.
    errors: HashMap<runtime::SessionId, String>,
    /// 207 §3.5: item id → deadline ms, cached per item and retained to listed ids.
    deadlines: HashMap<String, u64>,
    /// 130 E2: when each pending ask was first seen.
    ask_since: HashMap<tools::AskId, (runtime::SessionId, u64)>,
    /// 130 E3: assistant text of each running turn since its last tool call —
    /// the "final text" once `TurnFinished` arrives. One entry per running session.
    texts: HashMap<runtime::SessionId, String>,
    /// 146 D4: the latest boot pass; `None` once cleared.
    boot: Option<agent::durability::BootPass>,
    /// 155 §7.4: item id → episode start, for `alert` ids and the delay.
    episodes: crate::alert::Episodes,
    /// 155 §7.4: the ms the armed [`AlertDue`] fires at; `None` when none is armed.
    alert_due: Option<u64>,
}

/// `tell` (internal, 155 §7.4): the earliest `alert_delay_ms` passed —
/// recheck so the now-visible alert publishes `AttentionChanged`.
#[derive(Clone, Copy, Debug)]
pub struct AlertDue;

impl Attention {
    /// Spawn, register, subscribe to approvals, shows and resume reports.
    pub async fn start(rt: &Runtime, store: &Store) -> anyhow::Result<ActorRef<Self>> {
        let resume_ask = rt.config.contains("durability")
            && rt
                .section::<agent::durability::DurabilityConfig>("durability")
                .is_ok_and(|d| d.resume == agent::durability::ResumeMode::Ask);
        let actor = Self::spawn_with_mailbox(
            Self {
                pool: store.pool().clone(),
                pending: HashMap::new(),
                reports: HashMap::new(),
                resume_ask,
                sources: Vec::new(),
                rt: rt.clone(),
                last: Vec::new(),
                seq: 0,
                epoch_ms: now_ms(),
                errors: HashMap::new(),
                deadlines: HashMap::new(),
                ask_since: HashMap::new(),
                texts: HashMap::new(),
                boot: None,
                episodes: crate::alert::Episodes::default(),
                alert_due: None,
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(ATTENTION_ACTOR)?;
        rt.subscribe::<Self, ApprovalRequested>(&actor).await;
        rt.subscribe::<Self, ApprovalResolved>(&actor).await;
        rt.subscribe::<Self, ArtifactShown>(&actor).await;
        rt.subscribe::<Self, agent::AgentEvent>(&actor).await;
        rt.subscribe::<Self, agent::durability::BootPass>(&actor)
            .await;
        rt.subscribe::<Self, AskRequested>(&actor).await;
        rt.subscribe::<Self, tools::AskResolved>(&actor).await;
        rt.subscribe::<Self, tasks::activity::ActivityAppended>(&actor)
            .await;
        // 146 D4: a pass that settled inside the agent plugin's start was
        // published before this subscription; take it as if it arrived now.
        if let Ok(Some(ag)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(Some(pass)) = ag.ask(agent::supervisor::LastBootPass).await
        {
            let _ = actor.tell(pass).await;
        }
        // 207 §4/§5.5: the coarse tick — expire stopped rows, re-score, publish only on change.
        let every = rt
            .section::<crate::config::RosterConfig>("roster")
            .unwrap_or_default()
            .attention
            .rescore_secs
            .max(1);
        let weak = actor.downgrade();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(every));
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(me) = weak.upgrade() else {
                    break;
                };
                if me.tell(RescoreTick).await.is_err() {
                    break;
                }
            }
        });
        // 205: a `show_web` tile opened, closed or changed its page — re-derive its `web_view` item.
        // Coalesced: one recheck per burst (a page load reports several times).
        let weak = actor.downgrade();
        let mut tiles = artifact::webview::watch_tiles();
        tokio::spawn(async move {
            while tiles.changed().await.is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                tiles.borrow_and_update();
                let Some(me) = weak.upgrade() else {
                    break;
                };
                if me.tell(WebViewsChanged).await.is_err() {
                    break;
                }
            }
        });
        Ok(actor)
    }

    async fn gather(&mut self) -> Inputs {
        let tasks = match tasks::actor() {
            Some(t) => t
                .ask(tasks::ListTasks(tasks::TaskFilter {
                    state: Some(vec![
                        tasks::TaskState::Todo,
                        tasks::TaskState::Doing,
                        tasks::TaskState::Review,
                        tasks::TaskState::Blocked,
                    ]),
                    ..Default::default()
                }))
                .await
                .ok()
                .unwrap_or_default(),
            None => vec![],
        };
        let agents = match crate::actor() {
            Some(r) => r.ask(crate::ListAgents).await.ok().unwrap_or_default(),
            None => vec![],
        };
        let (peers, engines) = match mesh::actor() {
            Some(m) => match m.ask(mesh::Status).await {
                Ok(s) => {
                    let mut engines: HashMap<String, String> = s
                        .peers
                        .iter()
                        .map(|p| (p.id.0.clone(), p.name.clone()))
                        .collect();
                    engines.insert(s.self_id.0, s.self_name);
                    (s.peers, engines)
                }
                Err(_) => (vec![], HashMap::new()),
            },
            None => (vec![], HashMap::new()),
        };
        // 182 S1: open tasks homed here whose agent this engine lacks.
        let mut misplaced = Vec::new();
        if let Some(me) = mesh::self_id() {
            let name = |e: &str| {
                engines
                    .get(e)
                    .cloned()
                    .unwrap_or_else(|| e.get(..8).unwrap_or(e).to_owned())
            };
            for t in tasks
                .iter()
                .filter(|t| t.home.as_deref() == Some(me.as_str()))
            {
                let tasks::Owner::Agent(a) = &t.owner else {
                    continue;
                };
                if a == crate::AgentName::DEFAULT
                    || agents.iter().any(|s| s.file.name.as_str() == a)
                {
                    continue;
                }
                let rows: Vec<(String,)> = sqlx::query_as(
                    "SELECT engine FROM engine_agents WHERE name = ? ORDER BY engine",
                )
                .bind(a)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
                misplaced.push(Misplaced {
                    task: t.clone(),
                    agent: a.clone(),
                    here: name(&me),
                    there: rows
                        .iter()
                        .filter(|(e,)| *e != me && crate::known_engine(e))
                        .map(|(e,)| name(e))
                        .collect(),
                });
            }
        }
        let handoffs = crate::handoff::list(&self.pool, true)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("attention: handoffs: {e:#}");
                vec![]
            });
        let secrets = match secrets::actor() {
            Some(s) => s
                .ask(secrets::ListRequests {
                    unanswered_only: true,
                })
                .await
                .ok()
                .unwrap_or_default(),
            None => vec![],
        };
        let packets = match tasks::actor() {
            Some(t) if !tasks.is_empty() => t
                .ask(tasks::ListPackets(
                    tasks.iter().map(|t| t.id.clone()).collect(),
                ))
                .await
                .ok()
                .unwrap_or_default(),
            _ => vec![],
        };
        // 104 A3: a session for task-born items without a checkout: the newest event that named one.
        let mut task_sessions = HashMap::new();
        if let Some(t) = tasks::actor() {
            for task in tasks.iter().filter(|t| {
                t.checkout.is_none()
                    && match t.state {
                        tasks::TaskState::Blocked => !t.blockers.is_empty(),
                        tasks::TaskState::Review => t.reviewer == Some(tasks::Owner::User),
                        _ => false,
                    }
            }) {
                if let Ok(events) = t.ask(tasks::TaskEvents(task.id.clone())).await
                    && let Some(s) = events.iter().rev().find_map(|e| e.session.clone())
                {
                    task_sessions.insert(task.id.0.clone(), s);
                }
            }
        }
        let asks = match ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR) {
            Ok(Some(a)) => a.ask(tools::PendingAsks).await.ok().unwrap_or_default(),
            _ => vec![],
        };
        // 130 E2: a pending ask is "waiting since" the first time this actor saw it.
        let now = now_ms();
        self.ask_since
            .retain(|id, _| asks.iter().any(|a| a.id == *id));
        for a in &asks {
            self.ask_since.entry(a.id).or_insert((a.session, now));
        }
        let finished = finished(&self.pool).await.unwrap_or_else(|e| {
            tracing::warn!("attention: finished: {e:#}");
            vec![]
        });
        let shows = shows(&self.pool).await.unwrap_or_else(|e| {
            tracing::warn!("attention: shows: {e:#}");
            vec![]
        });
        // 104 A3: session → desk agent, for desks and every session under one.
        let mut desks: HashMap<String, String> = agents
            .iter()
            .filter_map(|a| Some((a.desk.clone()?, a.file.name.as_str().to_owned())))
            .collect();
        let mut resumes: Vec<ResumeNeed> = self.reports.values().cloned().collect();
        let mut session_context = HashMap::new();
        let mut sessions: Vec<agent::SessionSummary> = Vec::new();
        let mut activity: Vec<agent::AgentInfo> = Vec::new();
        let mut store: Option<agent::store::Store> = None;
        if let Ok(Some(ag)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            if let Ok(list) = ag.ask(agent::ListSessions).await {
                desks.extend(desk_map(&list));
                sessions = list;
            }
            // 207 R3 / D5: the live set and the awaiting edges.
            if let Ok(rows) = ag.ask(agent::ListActivity).await {
                activity = rows;
            }
            store = ag.ask(agent::DurableStore).await.ok();
            // 109 §4.6: under `ask` every open turn waits for the button;
            // 139 §5.1 item 13: only the turns this engine owns (its button
            // is `resume_turn`, which must run on the owner).
            if self.resume_ask
                && let Ok(open) = ag
                    .ask(agent::supervisor::OpenTurns(agent::store::Scope::Here))
                    .await
            {
                resumes.extend(open.into_iter().map(|(s, t, at)| ResumeNeed {
                    session: s.to_string(),
                    turn: t.to_string(),
                    at_ms: at.timestamp_millis().max(0) as u64,
                    detail: None,
                    taken_from: None,
                }));
            }
            // 149 E6: the exchange behind every session-born item (ask, approval,
            // finished, resume, blocked-with-session), one store read per session.
            let mut wanted: HashSet<String> = HashSet::new();
            wanted.extend(asks.iter().map(|a| a.session.to_string()));
            wanted.extend(self.pending.values().map(|(a, _)| a.session.to_string()));
            wanted.extend(finished.iter().map(|f| f.session.clone()));
            wanted.extend(resumes.iter().map(|r| r.session.clone()));
            wanted.extend(
                tasks
                    .iter()
                    .filter(|t| matches!(t.state, tasks::TaskState::Blocked))
                    .filter_map(|t| {
                        t.checkout
                            .as_ref()
                            .map(|c| c.session.clone())
                            .or_else(|| task_sessions.get(&t.id.0).cloned())
                    }),
            );
            if !wanted.is_empty()
                && let Some(store) = store.as_ref()
            {
                for s in wanted {
                    let Ok(id) = s.parse::<runtime::SessionId>() else {
                        continue;
                    };
                    match store.last_exchange(id).await {
                        Ok((request, reply)) => {
                            session_context.insert(
                                s,
                                ItemContext {
                                    request: request.as_deref().and_then(clip_context),
                                    last_reply: clip_context(&reply),
                                },
                            );
                        }
                        Err(e) => tracing::warn!("attention: last_exchange {s}: {e:#}"),
                    }
                }
            }
        }
        // 207 §3.5: each request's deadline — the journal row's, else arrival + the
        // configured timeout; cached per item id, retained to the pending ids.
        let tools_cfg = self
            .rt
            .section::<tools::ToolsConfig>("tools")
            .unwrap_or_default();
        let secret_ms = self
            .rt
            .section::<secrets::SecretsConfig>("secrets")
            .unwrap_or_default()
            .request_timeout_secs
            .saturating_mul(1000);
        let handoff_ms = crate::handoff::timeout(&self.rt).as_millis() as u64;
        // (item id, journal row id, fallback deadline ms)
        let mut wanted_deadlines: Vec<(String, Option<String>, u64)> = Vec::new();
        for (a, at) in self.pending.values() {
            wanted_deadlines.push((
                format!("approval:{}", a.id.0),
                Some(a.id.0.to_string()),
                at.saturating_add(tools_cfg.approval_timeout_secs.saturating_mul(1000)),
            ));
        }
        for a in &asks {
            let at = self.ask_since.get(&a.id).map_or(now, |(_, at)| *at);
            wanted_deadlines.push((
                format!("ask:{}", a.id.0),
                Some(a.id.0.to_string()),
                at.saturating_add(tools_cfg.ask_timeout_secs.saturating_mul(1000)),
            ));
        }
        for h in &handoffs {
            wanted_deadlines.push((
                format!("handoff:{}", h.id),
                Some(h.id.clone()),
                h.created_ms.saturating_add(handoff_ms),
            ));
        }
        for r in &secrets {
            wanted_deadlines.push((
                format!("secret:{}", r.id.0),
                None,
                r.created_ms.saturating_add(secret_ms),
            ));
        }
        self.deadlines
            .retain(|id, _| wanted_deadlines.iter().any(|(w, _, _)| w == id));
        for (id, row, fallback) in wanted_deadlines {
            if self.deadlines.contains_key(&id) {
                continue;
            }
            let mut deadline = fallback;
            if let (Some(row), Some(store)) = (row, store.as_ref())
                && let Ok(Some(q)) = store.question_get(&row).await
            {
                deadline = q.deadline.timestamp_millis().max(0) as u64;
            }
            self.deadlines.insert(id, deadline);
        }
        let config = self
            .rt
            .section::<crate::config::RosterConfig>("roster")
            .unwrap_or_default()
            .attention;
        let lineage = lineage_map(&sessions, &desks);
        Inputs {
            tasks,
            packets,
            agents,
            approvals: self.pending.values().cloned().collect(),
            peers,
            handoffs,
            secrets,
            engines,
            asks,
            shows,
            web_views: artifact::webview::open_tiles(),
            desks,
            task_sessions,
            resumes,
            boot: self.boot.clone(),
            external: {
                let mut v = Vec::new();
                for s in &self.sources {
                    v.extend(s.items().await);
                }
                v
            },
            ask_since: self
                .ask_since
                .iter()
                .map(|(id, (_, at))| (*id, *at))
                .collect(),
            finished,
            project_order: self
                .rt
                .section::<tasks::TasksConfig>("tasks")
                .map(|c| c.project_order)
                .unwrap_or_default(),
            session_context,
            // 182 D9: one item mesh-wide — the lease holder raises it.
            homes_offline: match tasks::actor() {
                Some(t) if mesh::is_hub() => t.ask(tasks::HomesOffline).await.unwrap_or_default(),
                _ => vec![],
            },
            misplaced,
            lineage,
            live: live_set(&activity, &sessions),
            blocks: blocks_map(&activity),
            deadlines: self.deadlines.clone(),
            config,
        }
    }

    /// 207 §5.5: derive now and publish what changed since the last published list.
    /// 155 §7.4: then decorate every item's `alert` through the episode map
    /// and arm one [`AlertDue`] at the earliest delay still open.
    async fn recheck(&mut self) -> Result<Vec<AttentionItem>, String> {
        let inputs = self.gather().await;
        let states = states(&self.pool).await.map_err(|e| e.to_string())?;
        let now = now_ms();
        let mut items = derive(&inputs, &states, now);
        let delay = self
            .rt
            .section::<crate::config::RosterConfig>("roster")
            .unwrap_or_default()
            .alert_delay_ms;
        self.episodes.retain(items.iter().map(|i| i.id.as_str()));
        for i in &mut items {
            i.alert = self.episodes.alert(i, now, delay);
        }
        if let Some(due) = self.episodes.next_due(now, delay)
            && self.alert_due.is_none_or(|armed| due < armed)
        {
            self.alert_due = Some(due);
            if let Ok(Some(me)) = ActorRef::<Self>::lookup(ATTENTION_ACTOR) {
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(due - now)).await;
                    let _ = me.tell(AlertDue).await;
                });
            }
        }
        if let Some(ev) = changed(&self.last, &items, now, self.seq + 1, self.epoch_ms) {
            self.seq += 1;
            self.last = items.clone();
            self.rt.publish(ev).await;
        }
        Ok(items)
    }

    /// `recheck` for the event handlers: the list is nobody's, an error is a log line.
    async fn recheck_quietly(&mut self) {
        if let Err(e) = self.recheck().await {
            tracing::warn!("attention: recheck: {e}");
        }
    }

    /// 130 E3 / 207 §4: a root / desk turn stopped with nothing pending → one
    /// `attention_finished` row for the session (the previous turn's is replaced).
    async fn finish_turn(
        &mut self,
        session: runtime::SessionId,
        turn: runtime::TurnId,
        stop_reason: String,
    ) {
        let text = self.texts.remove(&session).unwrap_or_default();
        let error = self
            .errors
            .remove(&session)
            .map(|e| platform::text::ellipsize(e.trim(), SUMMARY_MAX).into_owned());
        let Some(outcome) = stop_card(&stop_reason) else {
            return;
        };
        if self.pending.values().any(|(a, _)| a.session == session)
            || self.ask_since.values().any(|(s, _)| *s == session)
        {
            return;
        }
        let Ok(Some(ag)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else {
            return;
        };
        let Ok(chain) = ag.ask(agent::Ancestors { session }).await else {
            return;
        };
        if !reports_finished(&chain) {
            return;
        }
        let fallback = match outcome {
            StopOutcome::Failed | StopOutcome::TimedOut => {
                error.as_deref().or(chain[0].title.as_deref())
            }
            _ => chain[0].title.as_deref(),
        };
        let (summary, next_steps) = parse_finished(&text, fallback);
        let row = Finished {
            session: session.to_string(),
            turn: turn.to_string(),
            at_ms: now_ms(),
            summary,
            next_steps,
            outcome,
            stop_reason,
            error,
        };
        if let Err(e) = record_finished(&self.pool, &row).await {
            tracing::warn!("attention: record finished {session}: {e:#}");
        }
        self.recheck_quietly().await;
    }
}

/// 207 §4: the outcome a turn's stop earns a card with; `None` for the resume
/// path's own stops (`interrupted`, `suspended`).
pub fn stop_card(stop_reason: &str) -> Option<StopOutcome> {
    (!matches!(stop_reason, "interrupted" | "suspended")).then(|| StopOutcome::of(stop_reason))
}

/// Whether a session's clean turn end earns a `finished:` row: roots only (a subagent has
/// ancestors; the parent reports for it) and never an autoresearch study session — its
/// title is the schedule name `autoresearch-<slug>` (140 §13 Revision 2), and the study's own
/// attention item says when it is done.
fn reports_finished(chain: &[agent::AncestorInfo]) -> bool {
    chain.len() == 1
        && !chain[0]
            .title
            .as_deref()
            .is_some_and(|t| t.starts_with("autoresearch-"))
}

/// 207 §5.5: the `AttentionChanged` a derive raises against the last published
/// list (`prev`); `None` when no id, order, content (other than `score`,
/// `reasons`, `alert`) or alert changed. `seq` is the number the event carries.
pub fn changed(
    prev: &[AttentionItem],
    items: &[AttentionItem],
    at_ms: u64,
    seq: u64,
    epoch_ms: u64,
) -> Option<AttentionChanged> {
    let content = |i: &AttentionItem| AttentionItem {
        score: 0,
        reasons: Vec::new(),
        alert: None,
        ..i.clone()
    };
    let before: HashMap<&str, &AttentionItem> = prev.iter().map(|i| (i.id.as_str(), i)).collect();
    let now: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut ids_added = Vec::new();
    let mut out = Vec::new();
    let mut alerts_changed = false;
    for i in items {
        match before.get(i.id.as_str()) {
            None => {
                ids_added.push(i.id.clone());
                alerts_changed |= i.alert.is_some();
                out.push(AttentionItem {
                    alert: None,
                    ..i.clone()
                });
            }
            Some(p) => {
                alerts_changed |= p.alert != i.alert;
                if content(p) != content(i) {
                    out.push(AttentionItem {
                        alert: None,
                        ..i.clone()
                    });
                }
            }
        }
    }
    let mut ids_removed: Vec<String> = Vec::new();
    for p in prev.iter().filter(|p| !now.contains(p.id.as_str())) {
        alerts_changed |= p.alert.is_some();
        ids_removed.push(p.id.clone());
    }
    let order_changed =
        prev.len() != items.len() || prev.iter().zip(items).any(|(a, b)| a.id != b.id);
    if ids_added.is_empty()
        && ids_removed.is_empty()
        && out.is_empty()
        && !order_changed
        && !alerts_changed
    {
        return None;
    }
    ids_added.sort();
    ids_removed.sort();
    Some(AttentionChanged {
        count: items.len() as u32,
        ids_added,
        ids_removed,
        at_ms,
        items: out,
        order: items.iter().map(|i| i.id.clone()).collect(),
        seq,
        epoch_ms,
        alerts_changed,
    })
}

/// 130 E3: upsert the session's receipt (one row per session; the newest turn wins).
pub async fn record_finished(pool: &SqlitePool, f: &Finished) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO attention_finished (session, turn, at_ms, summary, next_steps, outcome, stop_reason, error) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(session) DO UPDATE SET turn = excluded.turn, at_ms = excluded.at_ms, \
         summary = excluded.summary, next_steps = excluded.next_steps, outcome = excluded.outcome, \
         stop_reason = excluded.stop_reason, error = excluded.error",
    )
    .bind(&f.session)
    .bind(&f.turn)
    .bind(f.at_ms as i64)
    .bind(&f.summary)
    .bind(serde_json::to_string(&f.next_steps)?)
    .bind(f.outcome.as_str())
    .bind(&f.stop_reason)
    .bind(&f.error)
    .execute(pool)
    .await?;
    Ok(())
}

/// 207 §4: drop a session's receipt (a new turn started); `true` when a row went.
pub async fn clear_finished(pool: &SqlitePool, session: &str) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM attention_finished WHERE session = ?")
        .bind(session)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// 207 §4: drop receipts older than `ttl_ms` at `now_ms`; `true` when a row went.
pub async fn prune_finished(pool: &SqlitePool, now_ms: u64, ttl_ms: u64) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM attention_finished WHERE at_ms <= ?")
        .bind(now_ms.saturating_sub(ttl_ms) as i64)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// One `attention_finished` row: session, turn, at_ms, summary, next_steps, outcome, stop_reason, error.
type FinishedRow = (
    String,
    String,
    i64,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
);

/// 130 E3: undismissed `attention_finished` rows, newest first (snooze is `derive`'s job).
pub async fn finished(pool: &SqlitePool) -> anyhow::Result<Vec<Finished>> {
    let rows: Vec<FinishedRow> = sqlx::query_as(
        "SELECT f.session, f.turn, f.at_ms, f.summary, f.next_steps, f.outcome, f.stop_reason, f.error \
         FROM attention_finished f \
         LEFT JOIN attention_state st ON st.id = 'finished:' || f.session || ':' || f.turn \
         WHERE st.dismissed_ms IS NULL ORDER BY f.at_ms DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(session, turn, at_ms, summary, next_steps, outcome, stop_reason, error)| Finished {
                session,
                turn,
                at_ms: at_ms as u64,
                summary,
                next_steps: serde_json::from_str(&next_steps).unwrap_or_default(),
                outcome: StopOutcome::parse(&outcome),
                stop_reason,
                error,
            },
        )
        .collect())
}

/// Session id → desk agent name for every session that is a desk or sits under one.
fn desk_map(sessions: &[agent::SessionSummary]) -> HashMap<String, String> {
    let by_id: HashMap<_, _> = sessions.iter().map(|s| (s.id, s)).collect();
    sessions
        .iter()
        .filter_map(|s| {
            let mut cur = s;
            // ponytail: bounded walk; the tree is shallow and a parent cycle is a store bug.
            for _ in 0..32 {
                if let Some(d) = &cur.desk {
                    return Some((s.id.to_string(), d.clone()));
                }
                cur = by_id.get(&cur.parent?)?;
            }
            None
        })
        .collect()
}

/// Record a show (`ArtifactShown`): the same `call_id` shown again replaces the row.
pub async fn record_show(pool: &SqlitePool, ev: &ArtifactShown, now_ms: u64) -> anyhow::Result<()> {
    let a = &ev.artifact;
    sqlx::query(
        "INSERT INTO attention_shows (call_id, session, title, name, kind, mime, bytes, version, at_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(call_id) DO UPDATE SET session = excluded.session, title = excluded.title, \
         name = excluded.name, kind = excluded.kind, mime = excluded.mime, bytes = excluded.bytes, \
         version = excluded.version, at_ms = excluded.at_ms",
    )
    .bind(&ev.call_id)
    .bind(ev.session.to_string())
    .bind(&a.title)
    .bind(&a.name)
    .bind(a.kind.as_str())
    .bind(&a.mime)
    .bind(a.bytes as i64)
    .bind(i64::from(a.version))
    .bind(now_ms as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// One `attention_shows` row: call_id, session, title, name, kind, mime, bytes, version, at_ms.
type ShowRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
);

/// Undismissed `attention_shows` rows, newest first (snooze is `derive`'s job).
pub async fn shows(pool: &SqlitePool) -> anyhow::Result<Vec<Shown>> {
    let rows: Vec<ShowRow> = sqlx::query_as(
        "SELECT s.call_id, s.session, s.title, s.name, s.kind, s.mime, s.bytes, s.version, s.at_ms \
         FROM attention_shows s \
         LEFT JOIN attention_state st ON st.id = 'show:' || s.call_id \
         WHERE st.dismissed_ms IS NULL ORDER BY s.at_ms DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(call_id, session, title, name, kind, mime, bytes, version, at_ms)| Shown {
                call_id,
                session,
                title,
                name,
                kind,
                mime,
                bytes: bytes as u64,
                version: version as u32,
                at_ms: at_ms as u64,
            },
        )
        .collect())
}

impl Message<ListHandoffs> for Attention {
    type Reply = Result<Vec<Handoff>, String>;

    async fn handle(&mut self, _: ListHandoffs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        crate::handoff::list(&self.pool, true)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<ResolveHandoff> for Attention {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: ResolveHandoff,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !matches!(m.outcome.as_str(), "done" | "cancelled") {
            return Err(format!("outcome: {:?} is not done | cancelled", m.outcome));
        }
        match crate::handoff::resolve(&self.pool, &m.id, &m.outcome, now_ms()).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("hand-off {} is unknown or already resolved", m.id)),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Every stored dismiss / snooze.
pub async fn states(pool: &SqlitePool) -> anyhow::Result<HashMap<String, State>> {
    let rows: Vec<(String, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT id, dismissed_ms, snoozed_until_ms FROM attention_state")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, d, s)| {
            (
                id,
                State {
                    dismissed_ms: d.map(|v| v as u64),
                    snoozed_until_ms: s.map(|v| v as u64),
                },
            )
        })
        .collect())
}

/// Snooze (`until_ms = Some`) or dismiss (`None`) one id.
pub async fn set_state(
    pool: &SqlitePool,
    id: &str,
    until_ms: Option<u64>,
    now_ms: u64,
) -> anyhow::Result<()> {
    match until_ms {
        Some(u) => sqlx::query(
            "INSERT INTO attention_state (id, snoozed_until_ms) VALUES (?, ?) \
                 ON CONFLICT(id) DO UPDATE SET snoozed_until_ms = excluded.snoozed_until_ms",
        )
        .bind(id)
        .bind(u as i64),
        None => sqlx::query(
            "INSERT INTO attention_state (id, dismissed_ms) VALUES (?, ?) \
                 ON CONFLICT(id) DO UPDATE SET dismissed_ms = excluded.dismissed_ms",
        )
        .bind(id)
        .bind(now_ms as i64),
    }
    .execute(pool)
    .await?;
    Ok(())
}

impl Message<ApprovalRequested> for Attention {
    type Reply = ();

    async fn handle(
        &mut self,
        m: ApprovalRequested,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.pending.insert(m.id, (m, now_ms()));
        self.recheck_quietly().await;
    }
}

/// 109 §8.3: the `ResumeNeed` a report raises, `None` for a clean resume
/// (nothing skipped, no child handed off): the banner is enough.
pub fn resume_need(ev: &agent::AgentEvent, now_ms: u64) -> Option<ResumeNeed> {
    let agent::AgentEvent::ResumeReport {
        session,
        turn,
        interrupted,
        children_handed_off,
        ..
    } = ev
    else {
        return None;
    };
    let mut parts = Vec::new();
    if !interrupted.is_empty() {
        let names: Vec<&str> = interrupted.iter().map(|(_, name)| name.as_str()).collect();
        parts.push(format!(
            "{} tool call{} skipped: {} — effect unknown",
            interrupted.len(),
            if interrupted.len() == 1 { "" } else { "s" },
            names.join(", ")
        ));
    }
    if *children_handed_off > 0 {
        parts.push(format!(
            "{children_handed_off} external child{} handed off",
            if *children_handed_off == 1 { "" } else { "ren" }
        ));
    }
    (!parts.is_empty()).then(|| ResumeNeed {
        session: session.to_string(),
        turn: turn.to_string(),
        at_ms: now_ms,
        detail: Some(parts.join("; ")),
        taken_from: None,
    })
}

/// 139 Q9 / D5: what an agent event does to the `resume:` map. A
/// `ResumeReport` with gaps and a `ResumePending` (system takeover) raise
/// the session's item; `TurnResumed` (the button was pressed, or the owner
/// resumed) and `TurnFinished` clear it. `true` when the map changed.
fn resume_transition(
    reports: &mut HashMap<String, ResumeNeed>,
    ev: &agent::AgentEvent,
    now_ms: u64,
) -> bool {
    if let Some(need) = resume_need(ev, now_ms) {
        reports.insert(need.session.clone(), need);
        return true;
    }
    match ev {
        agent::AgentEvent::ResumePending {
            session,
            turn,
            from,
        } => {
            reports.insert(
                session.to_string(),
                ResumeNeed {
                    session: session.to_string(),
                    turn: turn.to_string(),
                    at_ms: now_ms,
                    detail: Some("resume?".into()),
                    taken_from: Some(from.clone()),
                },
            );
            true
        }
        agent::AgentEvent::TurnResumed { session, .. }
        | agent::AgentEvent::TurnFinished { session, .. } => {
            reports.remove(&session.to_string()).is_some()
        }
        _ => false,
    }
}

impl Message<agent::AgentEvent> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if resume_transition(&mut self.reports, &m, now_ms()) {
            self.recheck_quietly().await;
            if !matches!(m, agent::AgentEvent::TurnFinished { .. }) {
                return;
            }
        }
        // 130 E3: the final text is what streamed after the last tool call.
        match m {
            agent::AgentEvent::TurnStarted { session, .. } => {
                self.texts.insert(session, String::new());
                self.errors.remove(&session);
                // 207 §4: the session is live again — its stopped card goes.
                match clear_finished(&self.pool, &session.to_string()).await {
                    Ok(true) => self.recheck_quietly().await,
                    Ok(false) => {}
                    Err(e) => tracing::warn!("attention: clear finished {session}: {e:#}"),
                }
            }
            agent::AgentEvent::ToolCallStarted { session, .. } => {
                self.texts.insert(session, String::new());
            }
            agent::AgentEvent::TextDelta { session, text, .. } => {
                // ponytail: unbounded per running turn; a model's final answer is tens of KB at most.
                self.texts.entry(session).or_default().push_str(&text);
            }
            agent::AgentEvent::Error {
                session, message, ..
            } => {
                self.errors.insert(session, message);
            }
            agent::AgentEvent::TurnFinished {
                session,
                turn,
                stop_reason,
                ..
            } => {
                // 207 §4: every stop but the resume path's own gets a card.
                self.finish_turn(session, turn, stop_reason).await;
            }
            _ => {}
        }
    }
}

impl Message<ApprovalResolved> for Attention {
    type Reply = ();

    async fn handle(
        &mut self,
        m: ApprovalResolved,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.pending.remove(&m.id);
        self.recheck_quietly().await;
    }
}

/// 146 D4: the boot pass's counts, re-sent as respawns settle. A count change
/// moves the item's title, which `changed` publishes (207 §5.5). A settled
/// pass that lost nothing clears itself 60 s later; one with losses stays
/// until dismissed.
impl Message<agent::durability::BootPass> for Attention {
    type Reply = ();

    async fn handle(
        &mut self,
        b: agent::durability::BootPass,
        ctx: &mut Context<Self, Self::Reply>,
    ) {
        let settled = b.waiting == 0;
        let clean = b.lost.is_empty();
        let at_ms = b.at_ms;
        self.boot = Some(b);
        self.recheck_quietly().await;
        if settled && clean {
            let me = ctx.actor_ref().clone();
            tokio::spawn(async move {
                tokio::time::sleep(BOOT_CLEAR).await;
                let _ = me.tell(ClearBoot(at_ms)).await;
            });
        }
    }
}

impl Message<ClearBoot> for Attention {
    type Reply = ();

    async fn handle(&mut self, ClearBoot(at_ms): ClearBoot, _: &mut Context<Self, Self::Reply>) {
        if self.boot.as_ref().is_some_and(|b| b.at_ms == at_ms) {
            self.boot = None;
            self.recheck_quietly().await;
        }
    }
}

impl Message<ArtifactShown> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: ArtifactShown, _: &mut Context<Self, Self::Reply>) {
        if let Err(e) = record_show(&self.pool, &m, now_ms()).await {
            tracing::warn!("attention: record show {}: {e:#}", m.call_id);
        }
        self.recheck_quietly().await;
    }
}

impl Message<AskRequested> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: AskRequested, _: &mut Context<Self, Self::Reply>) {
        self.ask_since.insert(m.id, (m.session, now_ms()));
        self.recheck_quietly().await;
    }
}

impl Message<tools::AskResolved> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: tools::AskResolved, _: &mut Context<Self, Self::Reply>) {
        self.ask_since.remove(&m.id);
        self.recheck_quietly().await;
    }
}

impl Message<tasks::activity::ActivityAppended> for Attention {
    type Reply = ();

    async fn handle(
        &mut self,
        _: tasks::activity::ActivityAppended,
        _: &mut Context<Self, Self::Reply>,
    ) {
        // Task and agent changes reach the inbox through the feed (130 §5.3 E1).
        self.recheck_quietly().await;
    }
}

impl Message<ListAttention> for Attention {
    type Reply = Result<Vec<AttentionItem>, String>;

    async fn handle(
        &mut self,
        _: ListAttention,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.recheck().await
    }
}

impl Message<AlertDue> for Attention {
    type Reply = ();

    /// 155 §7.4: a delay window closed; derive again so the alert shows — the
    /// new `alert` sets `alerts_changed` on the published event (207 §5.5).
    async fn handle(&mut self, _: AlertDue, _: &mut Context<Self, Self::Reply>) {
        self.alert_due = None;
        self.recheck_quietly().await;
    }
}

/// `ask` → `Result<Vec<AttentionOutcome>, String>`: 207 §5 per kind; one recheck after the batch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DismissMany {
    /// Which items.
    pub target: AttentionTarget,
    /// Who (`rpc_core::Client::actor`).
    pub actor: String,
}

/// `ask` → `Result<Vec<AttentionOutcome>, String>`: hide every target id until `until_ms` (every kind; outcome `snoozed`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnoozeMany {
    /// Which items.
    pub target: AttentionTarget,
    /// ms since epoch.
    pub until_ms: u64,
}

/// `tell` (internal): the coarse tick (207 §4, §5.5) — prune stopped rows past
/// `stopped_ttl_mins`, re-derive, publish only when something changed.
#[derive(Clone, Copy, Debug)]
pub struct RescoreTick;

/// `tell` (internal): `show_web` tiles changed (205): re-derive, publish only when something changed.
#[derive(Clone, Copy, Debug)]
pub struct WebViewsChanged;

impl Message<WebViewsChanged> for Attention {
    type Reply = ();

    async fn handle(&mut self, _: WebViewsChanged, _: &mut Context<Self, Self::Reply>) {
        self.recheck_quietly().await;
    }
}

impl Attention {
    /// 207 §5.1: the current list (no publish) and the ids `target` names in it.
    async fn targets(
        &mut self,
        target: &AttentionTarget,
    ) -> Result<(HashMap<String, AttentionItem>, Vec<String>), String> {
        let inputs = self.gather().await;
        let states = states(&self.pool).await.map_err(|e| e.to_string())?;
        let items = derive(&inputs, &states, now_ms());
        let ids = target.resolve(&items)?;
        Ok((items.into_iter().map(|i| (i.id.clone(), i)).collect(), ids))
    }

    /// 207 §5.2: leave one item to its agent, per kind. Never wakes a session.
    async fn dismiss_one(&mut self, item: &AttentionItem, actor: &str) -> AttentionOutcome {
        use AttentionResolution as R;
        let id = item.id.as_str();
        let now = now_ms();
        match item.kind {
            AttentionKind::Approval if id.starts_with("approval:") => {
                let Some(approval) = item.approval else {
                    return AttentionOutcome::of(id, R::Gone);
                };
                let Ok(Some(a)) = ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR) else {
                    return AttentionOutcome::failed(id, "the approval actor is not running");
                };
                match a.ask(tools::approval::Dismiss { id: approval }).await {
                    Ok(true) => {
                        self.pending.remove(&approval);
                        tasks::activity::record(
                            &self.rt,
                            tasks::activity::ActivityKind::ApprovalResolved,
                            actor,
                            item.session.as_deref().unwrap_or(id),
                            format!("left approval {} to the agent (not run)", approval.0),
                        )
                        .await;
                        AttentionOutcome::of(id, R::NotRun)
                    }
                    Ok(false) => AttentionOutcome::of(id, R::Gone),
                    Err(e) => AttentionOutcome::failed(id, e),
                }
            }
            AttentionKind::Ask if id.starts_with("ask:") => {
                let Some(ask) = item.ask.as_ref().map(|a| a.id) else {
                    return AttentionOutcome::of(id, R::Gone);
                };
                let Ok(Some(a)) = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR) else {
                    return AttentionOutcome::failed(id, "the ask actor is not running");
                };
                match a.ask(tools::LeaveAsk { id: ask }).await {
                    Ok(Some(_)) => {
                        self.ask_since.remove(&ask);
                        AttentionOutcome::of(id, R::LeftToAgent)
                    }
                    Ok(None) => AttentionOutcome::of(id, R::Gone),
                    Err(e) => AttentionOutcome::failed(id, e),
                }
            }
            AttentionKind::Decision if id.starts_with("decision:") => {
                let Some(decision) = item.decision.clone() else {
                    return AttentionOutcome::of(id, R::Gone);
                };
                let Some(t) = tasks::actor() else {
                    return AttentionOutcome::failed(id, "the tasks actor is not running");
                };
                let m = tasks::LeaveDecision {
                    decision,
                    actor: actor.to_owned(),
                };
                match t.ask(m).await {
                    Ok(l) if l.live => AttentionOutcome::of(id, R::LeftToAgent),
                    Ok(_) => AttentionOutcome::of(id, R::LeftForLater),
                    Err(kameo::error::SendError::HandlerError(e))
                        if e.starts_with("no task is blocked") =>
                    {
                        AttentionOutcome::of(id, R::Gone)
                    }
                    Err(e) => AttentionOutcome::failed(id, e),
                }
            }
            AttentionKind::Secret if id.starts_with("secret:") => {
                let Some(rid) = id
                    .strip_prefix("secret:")
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                else {
                    return AttentionOutcome::of(id, R::Gone);
                };
                let Some(s) = secrets::actor() else {
                    return AttentionOutcome::failed(id, "the secrets actor is not running");
                };
                match s
                    .ask(secrets::Waive {
                        id: secrets::RequestId(rid),
                    })
                    .await
                {
                    Ok(true) => AttentionOutcome::of(id, R::ProceedWithout),
                    Ok(false) => AttentionOutcome::of(id, R::Gone),
                    Err(e) => AttentionOutcome::failed(id, e),
                }
            }
            AttentionKind::Handoff if id.starts_with("handoff:") => {
                let hid = &id["handoff:".len()..];
                match crate::handoff::resolve(&self.pool, hid, crate::handoff::PROCEED_WITHOUT, now)
                    .await
                {
                    Ok(true) => AttentionOutcome::of(id, R::ProceedWithout),
                    Ok(false) => AttentionOutcome::of(id, R::Gone),
                    Err(e) => AttentionOutcome::failed(id, format!("{e:#}")),
                }
            }
            // 119: everything else (Source items with their own prefixes too) is hidden, versioned.
            _ => match set_state(&self.pool, id, None, now).await {
                Ok(()) => AttentionOutcome::of(id, R::Hidden),
                Err(e) => AttentionOutcome::failed(id, format!("{e:#}")),
            },
        }
    }
}

impl Message<DismissMany> for Attention {
    type Reply = Result<Vec<AttentionOutcome>, String>;

    async fn handle(&mut self, m: DismissMany, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (items, ids) = self.targets(&m.target).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            out.push(match items.get(id) {
                Some(item) => self.dismiss_one(item, &m.actor).await,
                None => AttentionOutcome::of(id, AttentionResolution::Gone),
            });
        }
        self.recheck_quietly().await;
        Ok(out)
    }
}

impl Message<SnoozeMany> for Attention {
    type Reply = Result<Vec<AttentionOutcome>, String>;

    async fn handle(&mut self, m: SnoozeMany, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (items, ids) = self.targets(&m.target).await?;
        let now = now_ms();
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            out.push(if items.contains_key(id) {
                match set_state(&self.pool, id, Some(m.until_ms), now).await {
                    Ok(()) => AttentionOutcome::of(id, AttentionResolution::Snoozed),
                    Err(e) => AttentionOutcome::failed(id, format!("{e:#}")),
                }
            } else {
                AttentionOutcome::of(id, AttentionResolution::Gone)
            });
        }
        self.recheck_quietly().await;
        Ok(out)
    }
}

impl Message<RescoreTick> for Attention {
    type Reply = ();

    async fn handle(&mut self, _: RescoreTick, _: &mut Context<Self, Self::Reply>) {
        let ttl_ms = self
            .rt
            .section::<crate::config::RosterConfig>("roster")
            .unwrap_or_default()
            .attention
            .stopped_ttl_mins
            .saturating_mul(60_000);
        if let Err(e) = prune_finished(&self.pool, now_ms(), ttl_ms).await {
            tracing::warn!("attention: prune finished: {e:#}");
        }
        self.recheck_quietly().await;
    }
}

impl Message<RegisterSource> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: RegisterSource, _: &mut Context<Self, ()>) {
        self.sources.push(m.0);
    }
}

/// The attention actor; `None` before the roster plugin started.
pub fn actor() -> Option<ActorRef<Attention>> {
    ActorRef::<Attention>::lookup(ATTENTION_ACTOR)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentFile, AgentName};

    fn task(
        title: &str,
        state: tasks::TaskState,
        owner: tasks::Owner,
        blockers: &[&str],
        reviewer: Option<tasks::Owner>,
        updated_ms: u64,
    ) -> tasks::Task {
        tasks::Task {
            id: tasks::TaskId::new(),
            title: title.into(),
            brief: String::new(),
            done_when: None,
            attachments: vec![],
            priority: tasks::Priority::Medium,
            home: None,
            owner,
            state,
            reviewer,
            blockers: blockers.iter().map(|s| (*s).to_owned()).collect(),
            blocks: vec![],
            parent: None,
            project: None,
            where_: None,
            harness: None,
            external: vec![],
            thread: None,
            checkout: None,
            schedule: None,
            created_by: "user".into(),
            created_ms: updated_ms,
            updated_ms,
            no_progress_count: 0,
            resolved_blockers: vec![],
            next_wake_after_ms: None,
        }
    }

    fn agent(
        name: &str,
        status: AgentStatus,
        budget: Budget,
        spent: f64,
        proposed_by: Option<&str>,
        paused: bool,
    ) -> AgentSummary {
        AgentSummary {
            isolated_memory: false,
            file: AgentFile {
                name: AgentName::new(name).unwrap(),
                title: name.to_uppercase(),
                reports_to: "theuth".into(),
                model: None,
                effort: None,
                tools: None,
                mcp: None,
                budget,
                capabilities: None,
                paused,
                proposed_by: proposed_by.map(Into::into),
                runs_in: Default::default(),
                memory: None,
                isolated_memory: false,
                view_bytes: None,
                tail_mode: None,
                tail_low_bytes: None,
                tail_high_bytes: None,
                tail_low_share: None,
                tail_high_share: None,
                profile: None,
                memory_model: None,
                memory_effort: None,
                body: "x".into(),
            },
            status,
            desk: None,
            open_tasks: 0,
            spent_month: spent,
            error: None,
            waiting_on: Vec::new(),
            held_wakes: 0,
            suspended: false,
        }
    }

    #[test]
    fn derives_every_kind_and_hides_dismissed_or_snoozed() {
        let ops = tasks::Owner::Agent("ops".into());
        let mut inputs = Inputs {
            tasks: vec![
                task(
                    "deploy",
                    tasks::TaskState::Blocked,
                    ops.clone(),
                    &["decision:d1"],
                    None,
                    50,
                ),
                task(
                    "migrate",
                    tasks::TaskState::Blocked,
                    ops.clone(),
                    &["person:sky"],
                    None,
                    40,
                ),
                task(
                    "review me",
                    tasks::TaskState::Review,
                    ops.clone(),
                    &[],
                    Some(tasks::Owner::User),
                    30,
                ),
                task(
                    "not mine",
                    tasks::TaskState::Review,
                    ops.clone(),
                    &[],
                    Some(ops.clone()),
                    30,
                ),
                task(
                    "Hire agent amy (AMY)",
                    tasks::TaskState::Todo,
                    tasks::Owner::User,
                    &[],
                    None,
                    20,
                ),
                task("plain", tasks::TaskState::Doing, ops.clone(), &[], None, 10),
            ],
            agents: vec![
                agent(
                    "amy",
                    AgentStatus::Proposed,
                    Budget::None,
                    0.0,
                    Some("agent:ops"),
                    true,
                ),
                agent(
                    "ops",
                    AgentStatus::Idle,
                    "40 USD/month".parse().unwrap(),
                    33.0,
                    None,
                    false,
                ),
                agent(
                    "dev",
                    AgentStatus::Idle,
                    "40 USD/month".parse().unwrap(),
                    10.0,
                    None,
                    false,
                ),
                agent(
                    "cap",
                    AgentStatus::Paused,
                    "10 USD/month".parse().unwrap(),
                    10.0,
                    None,
                    true,
                ),
            ],
            approvals: vec![],
            peers: vec![mesh::Peer {
                id: mesh::EngineId("e2".into()),
                name: "build-b".into(),
                addrs: vec![],
                reachable: false,
                last_seen_ms: 5,
                hub_pref: mesh::HubPref::Eligible,
                path: mesh::PeerPath::Offline,
                rtt_ms: None,
            }],
            ..Default::default()
        };
        // 149 E6: the blocked task's newest event named a session whose exchange is known.
        inputs
            .task_sessions
            .insert(inputs.tasks[1].id.0.clone(), "s-blocked".into());
        inputs.session_context.insert(
            "s-blocked".into(),
            ItemContext {
                request: Some("Migrate the users table.".into()),
                last_reply: Some("Blocked on Sky's sign-off.".into()),
            },
        );
        let items = derive(&inputs, &HashMap::new(), 100);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        // 207 §3: score order — a decision stops an agent (600 + medium 50), a node is a mesh
        // notice (450), hire / review / blocked are yours (300 + 50, then the longest-waiting
        // first), a paused budget is yours (300), a budget at 80 % a notice (120).
        assert_eq!(
            kinds,
            [
                AttentionKind::Decision,
                AttentionKind::Node,
                AttentionKind::Hire,
                AttentionKind::Review,
                AttentionKind::Blocked,
                AttentionKind::Budget,
                AttentionKind::Budget
            ]
        );
        let hire = items
            .iter()
            .find(|i| i.kind == AttentionKind::Hire)
            .unwrap();
        assert_eq!(hire.id, "hire:amy");
        assert!(hire.task.is_some(), "the open hire task is linked");
        assert!(
            !items
                .iter()
                .any(|i| i.title.starts_with("\u{201c}Hire agent")),
            "the hire task is not a second item"
        );
        let decision = items
            .iter()
            .find(|i| i.kind == AttentionKind::Decision)
            .unwrap();
        assert_eq!(decision.decision.as_deref(), Some("decision:d1"));
        assert_eq!(
            decision.context, None,
            "a decision is task-born: no bubbles"
        );
        let blocked = items
            .iter()
            .find(|i| i.kind == AttentionKind::Blocked)
            .unwrap();
        assert_eq!(blocked.session.as_deref(), Some("s-blocked"));
        assert_eq!(
            blocked.context.as_ref().and_then(|c| c.request.as_deref()),
            Some("Migrate the users table.")
        );
        assert_eq!(
            items
                .iter()
                .filter(|i| i.kind == AttentionKind::Budget)
                .map(|i| i.agent.clone().unwrap())
                .collect::<Vec<_>>(),
            ["cap", "ops"]
        );

        let mut states = HashMap::new();
        states.insert(
            hire.id.clone(),
            State {
                dismissed_ms: Some(20),
                snoozed_until_ms: None,
            },
        );
        states.insert(
            decision.id.clone(),
            State {
                dismissed_ms: None,
                snoozed_until_ms: Some(200),
            },
        );
        states.insert(
            "budget:ops".into(),
            State {
                dismissed_ms: None,
                snoozed_until_ms: Some(90),
            },
        );
        let left = derive(&inputs, &states, 100);
        assert_eq!(
            left.len(),
            5,
            "dismissed and live-snoozed gone, expired snooze back"
        );
        assert!(left.iter().any(|i| i.id == "budget:ops"));
    }

    fn packet(
        key: &str,
        task: &tasks::TaskId,
        kind: tasks::packet::PacketKind,
    ) -> tasks::packet::Packet {
        tasks::packet::Packet {
            key: key.into(),
            task: task.clone(),
            kind,
            by: "agent:ops".into(),
            at_ms: 1,
            question: (kind == tasks::packet::PacketKind::Decision)
                .then(|| "Cap retries at 3?\n\nmore".to_owned()),
            summary: format!("# Summary line {}\n\nbody", "s".repeat(150)),
            board: None,
            options: vec![],
            recommended: None,
            pointers: vec![],
            checks: vec![],
            recommendation: None,
            items: vec![],
            to: None,
            if_wrong: None,
            left_to_agent_ms: None,
        }
    }

    /// 101: a decision / review item carries its packet key and the first line
    /// of the question / summary as `detail`; without a packet today's strings stay.
    #[test]
    fn decision_and_review_items_carry_their_packet() {
        let ops = tasks::Owner::Agent("ops".into());
        let d = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["decision:d1"],
            None,
            50,
        );
        let r = task(
            "migrate",
            tasks::TaskState::Review,
            ops.clone(),
            &[],
            Some(tasks::Owner::User),
            40,
        );
        let bare = Inputs {
            tasks: vec![d.clone(), r.clone()],
            ..Default::default()
        };
        // 207 §3 order: the decision (600 + 50) before the review (300 + 50); legacy strings stay.
        let items = derive(&bare, &HashMap::new(), 100);
        assert_eq!(items[0].packet, None);
        assert_eq!(
            items[0].title,
            "agent:ops asks for a decision on \u{201c}deploy\u{201d}"
        );
        assert_eq!(
            items[0].detail.as_deref(),
            Some("the question is in the task thread")
        );
        assert_eq!(items[1].packet, None);
        assert_eq!(items[1].title, "\u{201c}migrate\u{201d} awaits your review");
        assert_eq!(items[1].detail.as_deref(), Some("by agent:ops"));

        let review_key = format!("review:{}", r.id);
        let with = Inputs {
            tasks: vec![d.clone(), r.clone()],
            packets: vec![
                packet("decision:d1", &d.id, tasks::packet::PacketKind::Decision),
                packet(&review_key, &r.id, tasks::packet::PacketKind::Review),
                packet("review:other", &d.id, tasks::packet::PacketKind::Review),
            ],
            ..Default::default()
        };
        // 132 D1: the decision row leads with the question, the byline is the detail;
        // the review row leads with the outcome (summary clipped to ROW_BUDGET), the task and author follow.
        let items = derive(&with, &HashMap::new(), 100);
        assert_eq!(items[0].kind, AttentionKind::Decision);
        assert_eq!(items[0].packet.as_deref(), Some("decision:d1"));
        assert_eq!(items[0].title, "Cap retries at 3?");
        assert_eq!(items[0].detail.as_deref(), Some("agent:ops \u{b7} deploy"));
        assert_eq!(items[1].kind, AttentionKind::Review);
        assert_eq!(items[1].packet.as_deref(), Some(review_key.as_str()));
        let title = items[1].title.as_str();
        assert!(title.starts_with("Summary line sss"), "{title}");
        assert_eq!(title.chars().count(), tasks::packet::ROW_BUDGET);
        assert!(title.ends_with('…'));
        assert_eq!(
            items[1].detail.as_deref(),
            Some("\u{201c}migrate\u{201d} \u{b7} by agent:ops")
        );
        assert_eq!(items.len(), 2, "a stray packet makes no item");
        assert!(
            items
                .iter()
                .all(|i| i.priority == Some(tasks::Priority::Medium) && i.rank == 5)
        );
    }

    /// 119: a dismissal hides the version the user saw. A review re-submitted
    /// after the dismissal (packet `at_ms` > `dismissed_ms`, task row unchanged)
    /// is back; dismissed at or after its `at_ms` it stays hidden; a snooze is
    /// time-based regardless of `at_ms`.
    #[test]
    fn dismissal_hides_only_the_dismissed_version() {
        let ops = tasks::Owner::Agent("ops".into());
        let r = task(
            "migrate",
            tasks::TaskState::Review,
            ops.clone(),
            &[],
            Some(tasks::Owner::User),
            40,
        );
        let key = format!("review:{}", r.id);
        let mut p = packet(&key, &r.id, tasks::packet::PacketKind::Review);
        p.at_ms = 70;
        let inputs = Inputs {
            tasks: vec![r.clone()],
            packets: vec![p],
            ..Default::default()
        };
        let dismissed = |ms| {
            HashMap::from([(
                key.clone(),
                State {
                    dismissed_ms: Some(ms),
                    snoozed_until_ms: None,
                },
            )])
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items[0].at_ms, 70, "the packet time wins over updated_ms");
        assert_eq!(
            derive(&inputs, &dismissed(60), 100).len(),
            1,
            "re-submitted after the dismissal: visible"
        );
        assert!(
            derive(&inputs, &dismissed(70), 100).is_empty(),
            "dismissed at at_ms: hidden"
        );
        assert!(
            derive(&inputs, &dismissed(80), 100).is_empty(),
            "dismissed later: hidden"
        );

        let snoozed = |until| {
            HashMap::from([(
                key.clone(),
                State {
                    dismissed_ms: None,
                    snoozed_until_ms: Some(until),
                },
            )])
        };
        assert!(
            derive(&inputs, &snoozed(200), 100).is_empty(),
            "snoozed into the future: hidden"
        );
        assert_eq!(
            derive(&inputs, &snoozed(90), 100).len(),
            1,
            "snooze expired: visible"
        );

        // Without a packet the task row is the version: updated_ms 40 vs dismissed 50.
        let bare = Inputs {
            tasks: vec![r],
            ..Default::default()
        };
        assert!(derive(&bare, &dismissed(50), 100).is_empty());
    }

    /// 119: a blocked item dismissed, then the task updated → visible again;
    /// a `decision:` item's `at_ms` does not move on its own, so it stays hidden.
    #[test]
    fn blocked_item_resurfaces_on_update_and_decision_stays_dismissed() {
        let ops = tasks::Owner::Agent("ops".into());
        let mut b = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["someone"],
            None,
            40,
        );
        let d = task(
            "cap",
            tasks::TaskState::Blocked,
            ops,
            &["decision:d1"],
            None,
            40,
        );
        let states = HashMap::from([
            (
                format!("blocked:{}", b.id),
                State {
                    dismissed_ms: Some(50),
                    snoozed_until_ms: None,
                },
            ),
            (
                format!("decision:{}:decision:d1", d.id),
                State {
                    dismissed_ms: Some(50),
                    snoozed_until_ms: None,
                },
            ),
        ]);
        let inputs = Inputs {
            tasks: vec![b.clone(), d.clone()],
            ..Default::default()
        };
        assert!(
            derive(&inputs, &states, 100).is_empty(),
            "both dismissed after their at_ms"
        );
        b.updated_ms = 60;
        let inputs = Inputs {
            tasks: vec![b, d],
            ..Default::default()
        };
        let items = derive(&inputs, &states, 100);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Blocked);
    }

    /// 182 D9: one `node:<id>` item per unreachable engine — kind `home` with a
    /// short title when tasks wait on it (the first five named in the detail),
    /// kind `node` otherwise.
    #[test]
    fn offline_home_is_one_node_item_with_a_short_title() {
        let peer = |id: &str, name: &str| mesh::Peer {
            id: mesh::EngineId(id.into()),
            name: name.into(),
            addrs: vec![],
            reachable: false,
            last_seen_ms: 5,
            hub_pref: mesh::HubPref::Eligible,
            path: mesh::PeerPath::Offline,
            rtt_ms: None,
        };
        let waiting: Vec<tasks::Task> = (1..=7)
            .map(|i| {
                task(
                    &format!("t{i}"),
                    tasks::TaskState::Todo,
                    tasks::Owner::Agent("theuth".into()),
                    &[],
                    None,
                    1,
                )
            })
            .collect();
        let inputs = Inputs {
            peers: vec![peer("A", "a"), peer("C", "c")],
            homes_offline: vec![tasks::HomeOffline {
                engine: "A".into(),
                name: "a".into(),
                since_ms: 2,
                tasks: waiting,
            }],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let mut mesh: Vec<(&str, AttentionKind)> = items
            .iter()
            .filter(|i| i.id.starts_with("node:"))
            .map(|i| (i.id.as_str(), i.kind))
            .collect();
        mesh.sort_by_key(|(id, _)| *id);
        assert_eq!(
            mesh,
            [
                ("node:A", AttentionKind::Home),
                ("node:C", AttentionKind::Node)
            ]
        );
        let a = items.iter().find(|i| i.id == "node:A").unwrap();
        assert_eq!(a.title, "a is offline — 7 tasks are waiting");
        assert!(
            a.detail
                .as_deref()
                .unwrap()
                .starts_with("Waiting: t1, t2, t3, t4, t5 and 2 more."),
            "{:?}",
            a.detail
        );
    }

    /// 106 F-2: a forward chain lists the newest decision addressed to the
    /// user; an agent-addressed decision alone makes no item (a non-decision
    /// blocker still makes the Blocked item); a legacy packet without `to`
    /// still counts.
    #[test]
    fn decision_item_is_the_newest_one_addressed_to_the_user() {
        let ops = tasks::Owner::Agent("ops".into());
        let addressed = |key: &str, task: &tasks::TaskId, by: &str, to: &str| {
            let mut p = packet(key, task, tasks::packet::PacketKind::Decision);
            p.by = by.into();
            p.to = Some(to.into());
            p
        };
        // forward chain: d1 ops → lead, d2 lead → user
        let chain = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["decision:d1", "decision:d2"],
            None,
            50,
        );
        let inputs = Inputs {
            tasks: vec![chain.clone()],
            packets: vec![
                addressed("decision:d1", &chain.id, "agent:ops", "agent:lead"),
                addressed("decision:d2", &chain.id, "agent:lead", "user"),
            ],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Decision);
        assert_eq!(items[0].id, format!("decision:{}:decision:d2", chain.id));
        assert_eq!(items[0].decision.as_deref(), Some("decision:d2"));
        assert_eq!(items[0].packet.as_deref(), Some("decision:d2"));
        assert_eq!(items[0].title, "Cap retries at 3?");
        assert!(
            items[0]
                .detail
                .as_deref()
                .unwrap()
                .starts_with("agent:lead \u{b7} "),
            "{:?}",
            items[0].detail
        );

        // only an agent-audience decision: nothing for the user
        let agents_only = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["decision:d1"],
            None,
            50,
        );
        let inputs = Inputs {
            tasks: vec![agents_only.clone()],
            packets: vec![addressed(
                "decision:d1",
                &agents_only.id,
                "agent:ops",
                "agent:lead",
            )],
            ..Default::default()
        };
        assert!(derive(&inputs, &HashMap::new(), 100).is_empty());

        // … unless something else blocks it too: the Blocked item as before
        let mixed = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["no continuation", "decision:d1"],
            None,
            50,
        );
        let inputs = Inputs {
            tasks: vec![mixed.clone()],
            packets: vec![addressed(
                "decision:d1",
                &mixed.id,
                "agent:ops",
                "agent:lead",
            )],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Blocked);

        // legacy packet (no `to`) after an agent-addressed one: still an item
        let legacy = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops,
            &["decision:d0", "decision:d1"],
            None,
            50,
        );
        let inputs = Inputs {
            tasks: vec![legacy.clone()],
            packets: vec![
                packet(
                    "decision:d0",
                    &legacy.id,
                    tasks::packet::PacketKind::Decision,
                ),
                addressed("decision:d1", &legacy.id, "agent:ops", "agent:lead"),
            ],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].decision.as_deref(), Some("decision:d0"));
    }

    fn handoff(id: &str, agent: Option<&str>, created_ms: u64, resolved: bool) -> Handoff {
        Handoff {
            id: id.into(),
            engine: "vm1".into(),
            session: "0199aaaa-bbbb-7000-8000-000000000000".into(),
            agent: agent.map(Into::into),
            reason: "log in to GitHub".into(),
            url: None,
            created_ms,
            resolved_ms: resolved.then_some(created_ms + 1),
            outcome: resolved.then(|| "done".to_owned()),
            hlc: None,
        }
    }

    fn secret(
        id: &str,
        title: Option<&str>,
        names: &[&str],
        created_ms: u64,
    ) -> secrets::SecretRequest {
        secrets::SecretRequest {
            id: secrets::RequestId(uuid::Uuid::parse_str(id).unwrap()),
            engine: "e9".into(),
            session: "s-secret".into(),
            title: title.map(Into::into),
            fields: names
                .iter()
                .map(|n| secrets::SecretRequestField {
                    name: (*n).to_owned(),
                    label: (*n).to_owned(),
                    masked: true,
                    current: None,
                    note: None,
                })
                .collect(),
            created_ms,
            answered_ms: None,
            outcome: None,
            hlc: None,
            expires: None,
        }
    }

    #[test]
    fn handoffs_and_secret_requests_become_items() {
        // The gatherer only passes unresolved / unanswered rows; `derive` trusts that.
        let inputs = Inputs {
            agents: vec![agent(
                "ops",
                AgentStatus::Idle,
                Budget::None,
                0.0,
                None,
                false,
            )],
            handoffs: vec![
                handoff("h1", Some("ops"), 50, false),
                handoff("h2", None, 60, false),
            ],
            secrets: vec![
                secret(
                    "0199aaaa-0000-7000-8000-000000000001",
                    Some("GitHub login"),
                    &["gh/user", "gh/pass"],
                    70,
                ),
                secret("0199aaaa-0000-7000-8000-000000000002", None, &["TOKEN"], 40),
            ],
            engines: HashMap::from([("vm1".to_owned(), "lab".to_owned())]),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "secret:0199aaaa-0000-7000-8000-000000000002",
                "handoff:h1",
                "handoff:h2",
                "secret:0199aaaa-0000-7000-8000-000000000001",
            ],
            "longest waiting first (132 D4)"
        );
        let h1 = &items[1];
        assert_eq!(h1.kind, AttentionKind::Handoff);
        assert_eq!(h1.title, "OPS needs you: log in to GitHub", "agent title");
        assert_eq!(h1.detail.as_deref(), Some("lab"), "engine's peer name");
        assert_eq!(h1.agent.as_deref(), Some("ops"));
        assert_eq!(
            h1.session.as_deref(),
            Some("0199aaaa-bbbb-7000-8000-000000000000")
        );
        assert_eq!(h1.at_ms, 50);
        assert_eq!(
            items[2].title, "0199aaaa needs you: log in to GitHub",
            "session short id"
        );
        let form = &items[3];
        assert_eq!(form.kind, AttentionKind::Secret);
        assert_eq!(form.title, "s-secret asks for a secret: GitHub login");
        assert_eq!(form.detail.as_deref(), Some("e9"), "unknown engine: the id");
        assert_eq!(items[0].title, "s-secret asks for a secret: TOKEN");

        let states = HashMap::from([
            (
                "handoff:h1".to_owned(),
                State {
                    dismissed_ms: Some(50),
                    snoozed_until_ms: None,
                },
            ),
            (
                "secret:0199aaaa-0000-7000-8000-000000000001".to_owned(),
                State {
                    dismissed_ms: None,
                    snoozed_until_ms: Some(200),
                },
            ),
        ]);
        let left: Vec<_> = derive(&inputs, &states, 100)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(
            left,
            ["secret:0199aaaa-0000-7000-8000-000000000002", "handoff:h2"]
        );
    }

    /// 109 §8.3: a report with a skipped mutating call raises one item, a
    /// clean report none; an `ask`-mode open turn is an item with no detail;
    /// Dismiss hides it like every other kind.
    #[test]
    fn resume_items_lifecycle() {
        let session = runtime::SessionId::new();
        let turn = runtime::TurnId::new();
        let clean = agent::AgentEvent::ResumeReport {
            session,
            turn,
            replayed: vec![("c1".into(), "read_file".into())],
            interrupted: vec![],
            lost_jobs: vec![],
            children_restored: 1,
            children_handed_off: 0,
        };
        assert_eq!(
            resume_need(&clean, 5),
            None,
            "a clean resume raises no item"
        );
        let gaps = agent::AgentEvent::ResumeReport {
            session,
            turn,
            replayed: vec![],
            interrupted: vec![
                ("c2".into(), "git push".into()),
                ("c3".into(), "shell".into()),
            ],
            lost_jobs: vec![],
            children_restored: 0,
            children_handed_off: 1,
        };
        let need = resume_need(&gaps, 70).unwrap();
        assert_eq!(
            need.detail.as_deref(),
            Some(
                "2 tool calls skipped: git push, shell — effect unknown; 1 external child handed off"
            )
        );

        let waiting = ResumeNeed {
            session: "0199aaaa-bbbb-7000-8000-000000000000".into(),
            turn: "t2".into(),
            at_ms: 90,
            detail: None,
            taken_from: None,
        };
        let inputs = Inputs {
            resumes: vec![need.clone(), waiting.clone()],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].kind, AttentionKind::Resume);
        assert_eq!(items[1].id, format!("resume:{}:t2", waiting.session));
        assert_eq!(
            items[1].title,
            "0199aaaa was interrupted by a restart — resume?"
        );
        assert_eq!(items[1].detail, None);
        assert_eq!(items[1].session.as_deref(), Some(waiting.session.as_str()));
        assert_eq!(items[0].id, format!("resume:{session}:{turn}"));
        assert!(
            items[0]
                .title
                .ends_with("resumed after a restart with gaps")
        );
        assert_eq!(items[0].detail, need.detail);

        // 119: a dismissal hides the version the user saw (`dismissed_ms >= at_ms`).
        let states = HashMap::from([(
            items[0].id.clone(),
            State {
                dismissed_ms: Some(items[0].at_ms),
                snoozed_until_ms: None,
            },
        )]);
        let left: Vec<_> = derive(&inputs, &states, 100)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(left, [items[1].id.clone()]);
    }

    /// 146 D4: one `resume:boot:<ts>` item with live counts and the lost
    /// names; its session is the first lost child (the Dismiss button's
    /// gate); a pass that resumed and lost nothing raises no item; dismissal
    /// sticks across re-derives because `at_ms` is the boot time.
    #[test]
    fn boot_pass_item() {
        use agent::durability::BootPass;
        let (w1, w2) = (runtime::SessionId::new(), runtime::SessionId::new());
        let pass = BootPass {
            at_ms: 1_000,
            resuming: 3,
            respawned: 2,
            fresh: 1,
            waiting: 2,
            lost: vec![(w1, "omp-125R1-W1".into()), (w2, "omp-125R1-W2".into())],
        };
        let inputs = Inputs {
            boot: Some(pass.clone()),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 5_000);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "resume:boot:1000");
        assert_eq!(items[0].kind, AttentionKind::Resume);
        assert!(
            items[0].title.ends_with(
                " · 3 sessions resumed · 1 subagent back · 1 restarted fresh · 2 waiting · 2 lost (omp-125R1-W1, omp-125R1-W2)"
            ),
            "{}",
            items[0].title
        );
        assert!(items[0].title.starts_with("Restarted "));
        assert_eq!(items[0].session.as_deref(), Some(w1.to_string().as_str()));
        assert_eq!(items[0].at_ms, 1_000);
        let dismissed = HashMap::from([(
            "resume:boot:1000".to_owned(),
            State {
                dismissed_ms: Some(2_000),
                snoozed_until_ms: None,
            },
        )]);
        assert!(derive(&inputs, &dismissed, 5_000).is_empty());
        let quiet = Inputs {
            boot: Some(BootPass {
                at_ms: 1_000,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(derive(&quiet, &HashMap::new(), 5_000).is_empty());
    }

    /// 139 D5 / Q9: a system takeover's `ResumePending` raises one `resume:`
    /// item with the takeover title and a Resume button; `TurnResumed` (the
    /// press, or the owner resuming) clears it, and so does `TurnFinished`
    /// for a report with gaps — the map no longer grows forever.
    #[test]
    fn takeover_pending_item_raised_and_cleared() {
        let session = runtime::SessionId::new();
        let turn = runtime::TurnId::new();
        let mut reports = HashMap::new();
        let pending = agent::AgentEvent::ResumePending {
            session,
            turn,
            from: "a".into(),
        };
        assert!(resume_transition(&mut reports, &pending, 70));
        let inputs = Inputs {
            resumes: reports.values().cloned().collect(),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, AttentionKind::Resume);
        assert_eq!(items[0].id, format!("resume:{session}:{turn}"));
        assert!(
            items[0].title.ends_with("was taken from a"),
            "{}",
            items[0].title
        );
        assert_eq!(items[0].detail.as_deref(), Some("resume?"));

        let resumed = agent::AgentEvent::TurnResumed {
            session,
            turn,
            kind: agent::durability::ResumeKind::Interrupted,
            interrupted_at: chrono::Utc::now(),
            resumed_at: chrono::Utc::now(),
            partial_dropped: false,
            cause: agent::durability::ResumeCause::Takeover,
            from: Some("a".into()),
        };
        assert!(resume_transition(&mut reports, &resumed, 80));
        assert!(reports.is_empty());
        assert!(
            !resume_transition(&mut reports, &resumed, 81),
            "nothing left to clear"
        );

        let gaps = agent::AgentEvent::ResumeReport {
            session,
            turn,
            replayed: vec![],
            interrupted: vec![("c2".into(), "git push".into())],
            lost_jobs: vec![],
            children_restored: 0,
            children_handed_off: 0,
        };
        assert!(resume_transition(&mut reports, &gaps, 90));
        let finished = agent::AgentEvent::TurnFinished {
            session,
            turn,
            stop_reason: "end_turn".into(),
            usage: agent::Usage::default(),
            context: 0,
        };
        assert!(resume_transition(&mut reports, &finished, 95));
        assert!(reports.is_empty());
    }

    #[tokio::test]
    async fn state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        set_state(store.pool(), "a", Some(500), 1).await.unwrap();
        set_state(store.pool(), "b", None, 2).await.unwrap();
        set_state(store.pool(), "a", Some(600), 3).await.unwrap();
        let s = states(store.pool()).await.unwrap();
        assert_eq!(
            s["a"],
            State {
                dismissed_ms: None,
                snoozed_until_ms: Some(600)
            }
        );
        assert_eq!(
            s["b"],
            State {
                dismissed_ms: Some(2),
                snoozed_until_ms: None
            }
        );
    }

    fn ask(session: runtime::SessionId, questions: &[(&str, &[&str])]) -> AskRequested {
        AskRequested {
            id: tools::AskId(uuid::Uuid::now_v7()),
            session,
            questions: questions
                .iter()
                .map(|(q, opts)| tools::Question {
                    id: "q".into(),
                    question: (*q).to_owned(),
                    options: opts
                        .iter()
                        .map(|l| tools::Choice {
                            label: (*l).to_owned(),
                            description: None,
                        })
                        .collect(),
                    recommended: None,
                })
                .collect(),
        }
    }

    fn shown(call_id: &str, session: &str, at_ms: u64) -> Shown {
        Shown {
            call_id: call_id.into(),
            session: session.into(),
            title: "Report".into(),
            name: "report.pdf".into(),
            kind: "pdf".into(),
            mime: "application/pdf".into(),
            bytes: 1_234_567,
            version: 2,
            at_ms,
        }
    }

    /// 105: the ask card an escalation raises for the channels carries the
    /// `decision:<id>` key as its question id; the Decision item already
    /// represents it, so the ask is not a second inbox row.
    #[test]
    fn an_escalation_ask_card_is_not_a_second_item() {
        let desk = runtime::SessionId::new();
        let mut card = ask(desk, &[("Which path?", &["A", "B"])]);
        card.questions[0].id = "decision:0199-abc".into();
        let plain = ask(desk, &[("Ship?", &["Yes"])]);
        let inputs = Inputs {
            asks: vec![card.clone(), plain.clone()],
            ..Default::default()
        };
        let ids: Vec<_> = derive(&inputs, &HashMap::new(), 100)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, [format!("ask:{}", plain.id.0)]);
    }

    #[test]
    fn asks_and_shows_become_items_with_their_session_and_desk() {
        let desk = runtime::SessionId::new();
        let other = runtime::SessionId::new();
        let one = ask(desk, &[("Deploy now?", &["Yes", "Later"])]);
        let many = ask(other, &[("A?", &[]), ("B?", &["x"]), ("C?", &[])]);
        let inputs = Inputs {
            agents: vec![agent(
                "ops",
                AgentStatus::Idle,
                Budget::None,
                0.0,
                None,
                false,
            )],
            asks: vec![one.clone(), many.clone()],
            shows: vec![shown("c1", &desk.to_string(), 50)],
            desks: HashMap::from([(desk.to_string(), "ops".to_owned())]),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [AttentionKind::Ask, AttentionKind::Ask, AttentionKind::Show],
            "asks stop an agent (600), a show is shown (120)"
        );
        let a = items
            .iter()
            .find(|i| i.id == format!("ask:{}", one.id.0))
            .unwrap();
        assert_eq!(a.title, "OPS asks: Deploy now?", "desk agent's title");
        assert_eq!(a.detail.as_deref(), Some("Yes \u{b7} Later"));
        assert_eq!(a.session.as_deref(), Some(desk.to_string().as_str()));
        assert_eq!(a.agent.as_deref(), Some("ops"));
        assert_eq!(a.ask.as_ref(), Some(&one), "the questions ride along");
        assert_eq!(a.at_ms, 100);
        let m = items
            .iter()
            .find(|i| i.id == format!("ask:{}", many.id.0))
            .unwrap();
        assert_eq!(m.detail.as_deref(), Some("3 questions"));
        assert_eq!(m.agent, None, "not under a desk");
        assert!(
            m.title.starts_with(&other.to_string()[..8]),
            "session short id: {}",
            m.title
        );
        let s = &items[2];
        assert_eq!(s.id, "show:c1");
        assert_eq!(s.title, "OPS showed you Report");
        assert_eq!(s.detail.as_deref(), Some("pdf \u{b7} 1.2 MB \u{b7} v2"));
        assert_eq!(s.session.as_deref(), Some(desk.to_string().as_str()));
        assert_eq!(s.agent.as_deref(), Some("ops"));
        assert_eq!((s.at_ms, s.ask.as_ref(), s.turn), (50, None, None));

        let states = HashMap::from([
            (
                "show:c1".to_owned(),
                State {
                    dismissed_ms: Some(50),
                    snoozed_until_ms: None,
                },
            ),
            (
                a.id.clone(),
                State {
                    dismissed_ms: None,
                    snoozed_until_ms: Some(200),
                },
            ),
        ]);
        let left: Vec<_> = derive(&inputs, &states, 100)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(
            left,
            std::slice::from_ref(&m.id),
            "dismissed show and snoozed ask hidden"
        );
    }

    #[test]
    fn task_items_carry_the_working_session() {
        let ops = tasks::Owner::Agent("ops".into());
        let mut checked_out = task(
            "deploy",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["decision:d1"],
            None,
            50,
        );
        checked_out.checkout = Some(tasks::Checkout {
            session: "work-1".into(),
            engine: "e1".into(),
            epoch: 1,
            progress_at_ms: 50,
            orphaned: false,
            taken: false,
        });
        let from_events = task(
            "review me",
            tasks::TaskState::Review,
            ops.clone(),
            &[],
            Some(tasks::Owner::User),
            40,
        );
        let nowhere = task(
            "migrate",
            tasks::TaskState::Blocked,
            ops,
            &["person:sky"],
            None,
            30,
        );
        let inputs = Inputs {
            tasks: vec![checked_out, from_events.clone(), nowhere],
            task_sessions: HashMap::from([(from_events.id.0.clone(), "work-2".to_owned())]),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let sessions: Vec<_> = items.iter().map(|i| i.session.as_deref()).collect();
        // 207 §3: the decision (600) first, then the yours-to-do items longest-waiting first.
        assert_eq!(sessions, [Some("work-1"), None, Some("work-2")]);
    }

    #[test]
    fn desk_map_walks_to_the_desk_root() {
        fn summary(
            id: runtime::SessionId,
            parent: Option<runtime::SessionId>,
            desk: Option<&str>,
        ) -> agent::SessionSummary {
            serde_json::from_value(serde_json::json!({
                "id": id, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                "title": "t", "model": "m", "parent": parent, "name": null, "effort": null,
                "agent": null, "cwd": null, "desk": desk,
            }))
            .unwrap()
        }
        let (desk, child, grandchild, loose) = (
            runtime::SessionId::new(),
            runtime::SessionId::new(),
            runtime::SessionId::new(),
            runtime::SessionId::new(),
        );
        let map = desk_map(&[
            summary(desk, None, Some("ops")),
            summary(child, Some(desk), None),
            summary(grandchild, Some(child), None),
            summary(loose, None, None),
        ]);
        assert_eq!(map.len(), 3);
        assert_eq!(map[&grandchild.to_string()], "ops");
        assert!(!map.contains_key(&loose.to_string()));
    }

    #[tokio::test]
    async fn shows_roundtrip_latest_version_wins_and_dismiss_hides() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        let session = runtime::SessionId::new();
        let ev = |version: u32| ArtifactShown {
            session,
            call_id: "c1".into(),
            artifact: artifact::Artifact {
                id: "abc".into(),
                session,
                call_id: "c1".into(),
                name: "report.pdf".into(),
                title: "Report".into(),
                kind: artifact::Kind::Pdf,
                mime: "application/pdf".into(),
                bytes: 10,
                version,
                path: "/x/abc.pdf".into(),
                source: None,
                preview: None,
                pages: None,
                origin: None,
                created_at: 0,
            },
        };
        record_show(store.pool(), &ev(1), 5).await.unwrap();
        record_show(store.pool(), &ev(2), 6).await.unwrap();
        let rows = shows(store.pool()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].version, rows[0].at_ms, rows[0].kind.as_str()),
            (2, 6, "pdf")
        );
        assert_eq!(rows[0].session, session.to_string());
        set_state(store.pool(), "show:c1", None, 7).await.unwrap();
        assert!(
            shows(store.pool()).await.unwrap().is_empty(),
            "dismissed rows stay out"
        );
    }

    /// 207 §5.5: ids, order, content (not score / reasons / alert) and alerts
    /// each announce; a score-only drift is silent; `seq` / `epoch_ms` ride along.
    #[test]
    fn attention_changed_tracks_ids_order_content_and_alerts() {
        let desk = runtime::SessionId::new();
        let one = ask(desk, &[("Ship?", &["Yes"])]);
        let two = ask(desk, &[("Deploy?", &["Yes"])]);
        let (id_one, id_two) = (format!("ask:{}", one.id.0), format!("ask:{}", two.id.0));
        let mut inputs = Inputs {
            asks: vec![one.clone()],
            ask_since: HashMap::from([(one.id, 10), (two.id, 20)]),
            ..Default::default()
        };
        let first = derive(&inputs, &HashMap::new(), 100);
        let ev = changed(&[], &first, 100, 1, 7).expect("first derive announces");
        assert_eq!(
            (ev.count, ev.ids_added.len(), ev.ids_removed.len()),
            (1, 1, 0)
        );
        assert_eq!((ev.seq, ev.epoch_ms, ev.alerts_changed), (1, 7, false));
        assert_eq!(ev.order, std::slice::from_ref(&id_one));
        assert_eq!(ev.items.len(), 1);

        // The ask's `at_ms` is its first-seen time: stable across derives; only the age drifts.
        let later = derive(&inputs, &HashMap::new(), 100 + 120_000);
        assert_eq!(later[0].at_ms, 10);
        assert_ne!(later[0].score, first[0].score);
        assert_eq!(
            changed(&first, &later, 2, 2, 7),
            None,
            "score-only: nothing"
        );

        let mut retitled = first.clone();
        retitled[0].title = "other".into();
        let ev = changed(&first, &retitled, 3, 2, 7).expect("content announces");
        assert!(ev.ids_added.is_empty() && ev.ids_removed.is_empty());
        assert_eq!(ev.items.len(), 1);
        assert!(!ev.alerts_changed);

        let mut alerted = first.clone();
        alerted[0].alert = Some(crate::alert::Alert {
            id: format!("{id_one}@10"),
            class: crate::alert::AlertClass::NeedsYou,
            why: "a session asked you a question".into(),
            at_ms: 10,
        });
        let ev = changed(&first, &alerted, 4, 2, 7).expect("an alert announces");
        assert!(ev.alerts_changed);
        assert!(ev.items.is_empty(), "an alert alone is no content change");
        let ev = changed(&alerted, &first, 4, 3, 7).expect("an alert going away announces");
        assert!(ev.alerts_changed);

        inputs.asks.push(two.clone());
        let more = derive(&inputs, &HashMap::new(), 102);
        let ev = changed(&first, &more, 102, 2, 7).unwrap();
        assert_eq!(ev.ids_added, std::slice::from_ref(&id_two));
        assert!(ev.ids_removed.is_empty());
        assert_eq!(ev.count, 2);
        assert_eq!(
            ev.order,
            [id_one.clone(), id_two.clone()],
            "longest waiting first"
        );
        assert_eq!(
            ev.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            [id_two.as_str()],
            "only the new item rides along"
        );

        let swapped: Vec<AttentionItem> = more.iter().rev().cloned().collect();
        let ev = changed(&more, &swapped, 103, 3, 7).expect("order announces");
        assert!(ev.items.is_empty() && ev.ids_added.is_empty() && ev.ids_removed.is_empty());
        assert_eq!(ev.order, [id_two.clone(), id_one.clone()]);

        let states = HashMap::from([(
            id_one.clone(),
            State {
                dismissed_ms: Some(200),
                snoozed_until_ms: None,
            },
        )]);
        let ev = changed(&more, &derive(&inputs, &states, 103), 103, 4, 7).unwrap();
        assert_eq!(ev.ids_removed, [id_one]);
        assert!(ev.ids_added.is_empty());
        assert_eq!(ev.count, 1);
    }

    /// 130 E2: an approval waits since it arrived, an ask since it was first seen.
    #[test]
    fn waiting_since_for_approvals_and_asks() {
        let s = runtime::SessionId::new();
        let a = ApprovalRequested {
            id: ApprovalId(uuid::Uuid::now_v7()),
            session: s,
            tool: "bash".into(),
            args: serde_json::json!({ "command": "rm -rf /tmp/x" }),
            risk: tools::Risk::Destructive,
            human_only: false,
            always_ask: false,
        };
        let q = ask(s, &[("Ship?", &["Yes"])]);
        let inputs = Inputs {
            approvals: vec![(a.clone(), 40)],
            asks: vec![q.clone()],
            ask_since: HashMap::from([(q.id, 70)]),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let by = |id: String| items.iter().find(|i| i.id == id).unwrap().waiting_since_ms;
        assert_eq!(by(format!("approval:{}", a.id.0)), Some(40));
        assert_eq!(by(format!("ask:{}", q.id.0)), Some(70));
        // 207 §3.5: an ask's `at_ms` is its first sighting, stable across derives.
        let at = |now: u64| {
            derive(&inputs, &HashMap::new(), now)
                .into_iter()
                .find(|i| i.id == format!("ask:{}", q.id.0))
                .unwrap()
                .at_ms
        };
        assert_eq!((at(100), at(5_000)), (70, 70));
    }

    /// 149 E6: every session-born kind carries the exchange of its session;
    /// a show on the same session does not; an empty store leaves `None`.
    #[test]
    fn context_lands_on_session_born_kinds_only() {
        let s = runtime::SessionId::new();
        let a = ApprovalRequested {
            id: ApprovalId(uuid::Uuid::now_v7()),
            session: s,
            tool: "bash".into(),
            args: serde_json::json!({ "command": "ls" }),
            risk: tools::Risk::Destructive,
            human_only: false,
            always_ask: false,
        };
        let mut inputs = Inputs {
            approvals: vec![(a, 40)],
            asks: vec![ask(s, &[("Ship?", &["Yes"])])],
            finished: vec![Finished {
                session: s.to_string(),
                turn: "t1".into(),
                at_ms: 50,
                summary: None,
                next_steps: vec![],
                outcome: StopOutcome::Finished,
                stop_reason: "end_turn".into(),
                error: None,
            }],
            resumes: vec![ResumeNeed {
                session: s.to_string(),
                turn: "t2".into(),
                at_ms: 60,
                detail: None,
                taken_from: None,
            }],
            shows: vec![Shown {
                call_id: "c1".into(),
                session: s.to_string(),
                title: "chart".into(),
                name: "c.png".into(),
                kind: "image".into(),
                mime: "image/png".into(),
                bytes: 1,
                version: 1,
                at_ms: 70,
            }],
            ..Default::default()
        };
        let empty = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(empty.len(), 5);
        assert!(
            empty.iter().all(|i| i.context.is_none()),
            "empty store: no bubbles"
        );

        let ctx = ItemContext {
            request: Some("Ship it?".into()),
            last_reply: Some("Running the checks.".into()),
        };
        inputs.session_context.insert(s.to_string(), ctx.clone());
        let items = derive(&inputs, &HashMap::new(), 100);
        for kind in [
            AttentionKind::Ask,
            AttentionKind::Approval,
            AttentionKind::Finished,
            AttentionKind::Resume,
        ] {
            let it = items.iter().find(|i| i.kind == kind).unwrap();
            assert_eq!(it.context.as_ref(), Some(&ctx), "{kind:?}");
        }
        let show = items
            .iter()
            .find(|i| i.kind == AttentionKind::Show)
            .unwrap();
        assert_eq!(show.context, None);

        // A session whose exchange is empty on both sides yields `None`, not an empty object.
        inputs
            .session_context
            .insert(s.to_string(), ItemContext::default());
        let blank = derive(&inputs, &HashMap::new(), 100);
        assert!(blank.iter().all(|i| i.context.is_none()));
    }

    /// 149 E6: fences dropped, lines joined, sentence-clipped at [`CONTEXT_MAX`].
    #[test]
    fn clip_context_drops_fences_and_clips_on_a_sentence() {
        assert_eq!(
            clip_context("Here you go:\n```sh\nrm -rf x\n```\nDone. Next?").as_deref(),
            Some("Here you go: Done. Next?")
        );
        assert_eq!(clip_context("```\nonly code\n```\n  "), None);
        assert_eq!(clip_context(""), None);
        let long = format!(
            "{} A second sentence that runs past the cap.",
            "word ".repeat(55)
        );
        let clipped = clip_context(&long).unwrap();
        assert!(
            clipped.chars().count() <= CONTEXT_MAX,
            "{}",
            clipped.chars().count()
        );
        assert!(
            clipped.ends_with('…') || clipped.ends_with('.'),
            "{clipped}"
        );
    }

    /// 130 E3: the `## Next` parser — two items, none, over-long.
    #[test]
    fn next_section_parses_two_items_none_and_long() {
        let (summary, next) = parse_finished(
            "Rotated the token and redeployed.\n\nDetails follow.\n\n## Next\n- Check the Grafana alerts\n2. Close the incident\n- a third one\n",
            Some("Title"),
        );
        assert_eq!(
            summary.as_deref(),
            Some("Rotated the token and redeployed.")
        );
        assert_eq!(next, ["Check the Grafana alerts", "Close the incident"]);

        let (summary, next) = parse_finished("", Some("Rotate the Grafana token"));
        assert_eq!(
            summary.as_deref(),
            Some("Rotate the Grafana token"),
            "title fallback"
        );
        assert!(next.is_empty());
        assert_eq!(parse_finished("  \n", None), (None, vec![]));

        let long = "x".repeat(300);
        let (summary, next) = parse_finished(&format!("{long}\n## Next\n- {long}"), None);
        assert_eq!(summary.as_ref().unwrap().chars().count(), SUMMARY_MAX);
        assert!(summary.unwrap().ends_with('\u{2026}'));
        assert_eq!(next[0].chars().count(), NEXT_STEP_MAX);
    }

    /// 140 §13 Revision 2: a study session (`autoresearch-<slug>`) and any subagent end
    /// their turns without a `finished:` row; an ordinary root still gets one.
    #[test]
    fn study_sessions_and_subagents_report_no_finished_row() {
        let link = |title: Option<&str>, name: Option<&str>| agent::AncestorInfo {
            id: runtime::SessionId::new(),
            title: title.map(str::to_owned),
            name: name.map(str::to_owned),
        };
        assert!(reports_finished(&[link(Some("Gateway limits"), None)]));
        assert!(reports_finished(&[link(None, None)]));
        assert!(!reports_finished(&[link(
            Some("autoresearch-sort-a3"),
            None
        )]));
        assert!(!reports_finished(&[
            link(Some("Gateway limits"), None),
            link(None, Some("brisk-otter"))
        ]));
    }

    /// 130 E3: one Finished item per session; the next turn's row replaces it; a dismissed one stays out.
    #[tokio::test]
    async fn finished_rows_replace_per_session_and_dismiss_hides() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        let s = runtime::SessionId::new().to_string();
        let row = |turn: &str, at_ms: u64| Finished {
            session: s.clone(),
            turn: turn.into(),
            at_ms,
            summary: Some("done".into()),
            next_steps: vec!["Reply ok".into()],
            outcome: StopOutcome::Finished,
            stop_reason: "end_turn".into(),
            error: None,
        };
        record_finished(store.pool(), &row("t1", 5)).await.unwrap();
        record_finished(store.pool(), &row("t2", 6)).await.unwrap();
        let rows = finished(store.pool()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].turn.as_str(), rows[0].at_ms), ("t2", 6));
        assert_eq!(rows[0].next_steps, ["Reply ok"]);
        let inputs = Inputs {
            finished: rows,
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.kind, AttentionKind::Finished);
        assert_eq!(it.id, format!("finished:{s}:t2"));
        assert_eq!(it.summary.as_deref(), Some("done"));
        assert_eq!(it.next_steps, ["Reply ok"]);
        assert_eq!(it.waiting_since_ms, None, "a receipt waits on nobody");
        set_state(store.pool(), &format!("finished:{s}:t2"), None, 7)
            .await
            .unwrap();
        assert!(finished(store.pool()).await.unwrap().is_empty());
    }

    /// 207 §4: every stop reason but the resume path's own earns a card, classed by outcome.
    #[test]
    fn stop_reasons_map_to_cards() {
        assert_eq!(stop_card("end_turn"), Some(StopOutcome::Finished));
        assert_eq!(stop_card("stop"), Some(StopOutcome::Finished));
        assert_eq!(stop_card("max_tokens"), Some(StopOutcome::Finished));
        assert_eq!(stop_card("error"), Some(StopOutcome::Failed));
        assert_eq!(stop_card("cancelled"), Some(StopOutcome::Cancelled));
        assert_eq!(stop_card("timed_out"), Some(StopOutcome::TimedOut));
        assert_eq!(stop_card("interrupted"), None);
        assert_eq!(stop_card("suspended"), None);
        for o in [
            StopOutcome::Finished,
            StopOutcome::Failed,
            StopOutcome::Cancelled,
            StopOutcome::TimedOut,
        ] {
            assert_eq!(StopOutcome::parse(o.as_str()), o);
        }
    }

    /// 207 §4: a row per stop with its outcome, reason and error; the title and
    /// detail follow the outcome; `expires_ms` = stop + ttl, and an expired row
    /// is gone from the list; a new turn deletes the row; the tick prunes by ttl.
    #[tokio::test]
    async fn stopped_rows_roundtrip_expire_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        let pool = store.pool();
        let row = |session: &str,
                   at_ms: u64,
                   outcome,
                   reason: &str,
                   summary: Option<&str>,
                   error: Option<&str>| Finished {
            session: session.into(),
            turn: "t1".into(),
            at_ms,
            summary: summary.map(str::to_owned),
            next_steps: vec![],
            outcome,
            stop_reason: reason.into(),
            error: error.map(str::to_owned),
        };
        record_finished(
            pool,
            &row(
                "s-fail",
                1_000,
                StopOutcome::Failed,
                "error",
                None,
                Some("rate limited"),
            ),
        )
        .await
        .unwrap();
        record_finished(
            pool,
            &row(
                "s-cancel",
                2_000,
                StopOutcome::Cancelled,
                "cancelled",
                Some("half"),
                None,
            ),
        )
        .await
        .unwrap();
        record_finished(
            pool,
            &row(
                "s-late",
                3_000,
                StopOutcome::TimedOut,
                "timed_out",
                Some("drafted"),
                Some("deadline"),
            ),
        )
        .await
        .unwrap();
        let rows = finished(pool).await.unwrap();
        assert_eq!(rows.len(), 3);
        let fail = rows.iter().find(|r| r.session == "s-fail").unwrap();
        assert_eq!(
            (
                fail.outcome,
                fail.stop_reason.as_str(),
                fail.error.as_deref()
            ),
            (StopOutcome::Failed, "error", Some("rate limited"))
        );

        let ttl_ms = 45 * 60_000;
        let inputs = Inputs {
            finished: rows.clone(),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 5_000);
        let by = |s: &str| {
            items
                .iter()
                .find(|i| i.session.as_deref() == Some(s))
                .unwrap()
        };
        let f = by("s-fail");
        assert_eq!(f.title, "s-fail failed");
        assert_eq!(
            f.detail.as_deref(),
            Some("rate limited"),
            "no summary: the error"
        );
        assert_eq!(f.expires_ms, Some(1_000 + ttl_ms));
        assert_eq!(
            f.stop,
            Some(AttentionStop {
                outcome: StopOutcome::Failed,
                reason: "error".into(),
                error: Some("rate limited".into()),
            })
        );
        assert_eq!(f.reasons[0].text, "failed");
        assert_eq!(by("s-cancel").title, "s-cancel was cancelled");
        assert_eq!(by("s-cancel").detail.as_deref(), Some("half"));
        assert_eq!(by("s-late").title, "s-late timed out");
        assert_eq!(
            by("s-late").score,
            150,
            "failed and timed out share the higher base"
        );

        // Past the ttl the item leaves the list before the tick deletes the row.
        let late = derive(&inputs, &HashMap::new(), 2_000 + ttl_ms);
        assert_eq!(
            late.iter()
                .map(|i| i.session.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["s-late"]
        );

        assert!(clear_finished(pool, "s-cancel").await.unwrap());
        assert!(!clear_finished(pool, "s-cancel").await.unwrap());
        assert!(prune_finished(pool, 1_000 + ttl_ms, ttl_ms).await.unwrap());
        let left: Vec<String> = finished(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.session)
            .collect();
        assert_eq!(left, ["s-late"]);
        assert!(!prune_finished(pool, 2_999 + ttl_ms, ttl_ms).await.unwrap());
    }

    // ---- 207 ATTENTION-FLAT-LIST ----------------------------------------------------

    /// 207 §3.4: score desc → waiting_since asc → at_ms desc; `rank` is still filled
    /// (132) but sorts nothing; a session item borrows its checkout task's priority.
    #[test]
    fn order_is_score_then_waiting_then_newest() {
        let ops = tasks::Owner::Agent("ops".into());
        let mut urgent = task(
            "urgent one",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["person:a"],
            None,
            90,
        );
        urgent.priority = tasks::Priority::Urgent;
        let mut ranked = task(
            "ranked",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["person:b"],
            None,
            95,
        );
        ranked.priority = tasks::Priority::Low;
        ranked.project = Some("sre".into());
        let old = task(
            "old medium",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["person:c"],
            None,
            10,
        );
        let new = task(
            "new medium",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["person:d"],
            None,
            80,
        );
        let mut checked = task(
            "checked out",
            tasks::TaskState::Doing,
            ops.clone(),
            &[],
            None,
            5,
        );
        checked.priority = tasks::Priority::High;
        checked.checkout = Some(tasks::Checkout {
            session: "work-9".into(),
            engine: "e1".into(),
            epoch: 1,
            progress_at_ms: 5,
            orphaned: false,
            taken: false,
        });
        let finished = Finished {
            session: "root-1".into(),
            turn: "t1".into(),
            at_ms: 99,
            summary: Some("done".into()),
            next_steps: vec![],
            outcome: StopOutcome::Finished,
            stop_reason: "end_turn".into(),
            error: None,
        };
        let inputs = Inputs {
            tasks: vec![
                urgent.clone(),
                ranked.clone(),
                old.clone(),
                new.clone(),
                checked,
            ],
            handoffs: vec![handoff("h-checked", Some("ops"), 50, false)],
            shows: vec![shown("c1", "work-9", 60)],
            finished: vec![finished],
            project_order: vec!["sre".into()],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "handoff:h-checked",                       // 600: stops an agent
                format!("blocked:{}", urgent.id).as_str(), // 300 + 90
                format!("blocked:{}", old.id).as_str(),    // 300 + 50, waiting since 10
                format!("blocked:{}", new.id).as_str(),    // 300 + 50, waiting since 80
                format!("blocked:{}", ranked.id).as_str(), // 300 + 2 × 10 + 10
                "show:c1",            // 120 + 70 borrowed from the checkout task
                "finished:root-1:t1", // 80
            ],
            "{items:#?}"
        );
        let scores: Vec<u32> = items.iter().map(|i| i.score).collect();
        assert_eq!(scores, [600, 390, 350, 350, 330, 190, 80]);
        assert_eq!(
            (items[4].priority, items[4].rank),
            (Some(tasks::Priority::Low), 102)
        );
        assert_eq!(
            (items[5].priority, items[5].rank),
            (Some(tasks::Priority::High), 7)
        );
        assert_eq!((items[0].priority, items[0].rank), (None, 5));
        // Equal score, wait and time: the id decides, so every client shows one order.
        let twins = Inputs {
            handoffs: vec![
                handoff("h-b", None, 50, false),
                handoff("h-a", None, 50, false),
            ],
            ..Default::default()
        };
        let ids: Vec<String> = derive(&twins, &HashMap::new(), 100)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, ["handoff:h-a", "handoff:h-b"]);
    }

    fn summary(
        id: runtime::SessionId,
        parent: Option<runtime::SessionId>,
        title: &str,
        name: Option<&str>,
        desk: Option<&str>,
        project: Option<&str>,
    ) -> agent::SessionSummary {
        serde_json::from_value(serde_json::json!({
            "id": id, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "title": title, "model": "m", "parent": parent, "name": name, "effort": null,
            "agent": null, "cwd": null, "desk": desk, "project": project,
        }))
        .unwrap()
    }

    /// 207 §3.5: root, desk root, child, grandchild, unknown parent.
    #[test]
    fn lineage_map_paths_roots_and_projects() {
        let ids: Vec<runtime::SessionId> = (0..6).map(|_| runtime::SessionId::new()).collect();
        let (root, child, grandchild, desk, orphan, missing) =
            (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
        let map = lineage_map(
            &[
                summary(
                    root,
                    None,
                    " Ship 2.5 release ",
                    None,
                    None,
                    Some("release-2.5"),
                ),
                summary(
                    child,
                    Some(root),
                    "child title",
                    Some("builder"),
                    None,
                    Some("release-2.5"),
                ),
                summary(
                    grandchild,
                    Some(child),
                    "lint it",
                    None,
                    None,
                    Some("release-2.5"),
                ),
                summary(desk, None, "", None, Some("ops"), Some("theuth")),
                summary(orphan, Some(missing), "", None, None, None),
            ],
            &HashMap::new(),
        );
        let l = |s: runtime::SessionId| map[&s.to_string()].clone();
        assert_eq!(
            l(root),
            Lineage {
                project: Some("release-2.5".into()),
                root: root.to_string(),
                parent: None,
                path: vec!["Ship 2.5 release".into()],
            }
        );
        assert_eq!(
            l(grandchild),
            Lineage {
                project: Some("release-2.5".into()),
                root: root.to_string(),
                parent: Some(child.to_string()),
                path: vec![
                    "Ship 2.5 release".into(),
                    "builder".into(),
                    "lint it".into()
                ],
            }
        );
        assert_eq!(l(desk).path, ["ops desk"]);
        assert_eq!(l(desk).project.as_deref(), Some("theuth"));
        let o = l(orphan);
        assert_eq!(
            o.root,
            orphan.to_string(),
            "an unknown parent ends the walk"
        );
        assert_eq!(o.parent, Some(missing.to_string()));
        assert_eq!(o.path, [orphan.to_string()[..8].to_owned()]);
    }

    fn row(
        id: runtime::SessionId,
        parent: runtime::SessionId,
        name: &str,
        state: agent::AgentState,
    ) -> agent::AgentInfo {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "parent": parent, "depth": 1, "agent": null, "model": "m",
            "state": state, "started_at": null, "finished_at": null, "last": "",
        }))
        .unwrap()
    }

    /// 207 D5: parent chain, child name, sibling name, `session:` id, a cycle, unresolved names.
    #[test]
    fn blocks_map_counts_transitive_waiters() {
        use agent::AgentState as A;
        let await_ = |from: &str| A::Awaiting { from: from.into() };
        let ids: Vec<runtime::SessionId> = (0..8).map(|_| runtime::SessionId::new()).collect();
        let (root, builder, linter, tester, x, y, other, lone) = (
            ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7],
        );
        let rows = [
            // the root waits on its child `builder` (child name)
            row(root, root, "", await_("builder")),
            // builder runs; its child linter waits on it (parent)
            row(builder, root, "builder", A::Running),
            row(linter, builder, "linter", await_("parent")),
            // tester waits on its sibling `builder`
            row(tester, root, "tester", await_("builder")),
            // a cycle through `session:` ids
            row(x, x, "", await_(&format!("session:{y}"))),
            row(y, y, "", await_(&format!("session:{x}"))),
            // unresolved: nobody is called `ghost`; a root's `parent` is itself
            row(other, other, "", await_("ghost")),
            row(lone, lone, "", await_("parent")),
        ];
        let m = blocks_map(&rows);
        assert_eq!(
            m.get(&builder.to_string()),
            Some(&3),
            "root, linter, tester"
        );
        assert_eq!(
            m.get(&x.to_string()),
            Some(&1),
            "a cycle counts the other side once"
        );
        assert_eq!(m.get(&y.to_string()), Some(&1));
        assert_eq!(m.get(&root.to_string()), None);
        assert_eq!(m.get(&other.to_string()), None);
        assert_eq!(m.get(&lone.to_string()), None);
        // A chain: c waits on b, b waits on a → a blocks 2.
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let chain = [
            row(a, a, "", A::Running),
            row(b, b, "", await_(&format!("session:{a}"))),
            row(c, c, "", await_(&format!("session:{b}"))),
        ];
        let m = blocks_map(&chain);
        assert_eq!((m[&a.to_string()], m[&b.to_string()]), (2, 1));
    }

    /// 207: running / awaiting / approval rows and store-running sessions are live.
    #[test]
    fn live_set_from_activity_and_sessions() {
        use agent::AgentState as A;
        let ids: Vec<runtime::SessionId> = (0..6).map(|_| runtime::SessionId::new()).collect();
        let rows = [
            row(ids[0], ids[0], "", A::Running),
            row(
                ids[1],
                ids[0],
                "a",
                A::Awaiting {
                    from: "parent".into(),
                },
            ),
            row(
                ids[2],
                ids[0],
                "b",
                A::Approval {
                    tool: "shell".into(),
                },
            ),
            row(ids[3], ids[0], "c", A::Idle),
            row(ids[4], ids[0], "d", A::Error),
        ];
        let mut remote = summary(ids[5], None, "t", None, None, None);
        remote.running = true;
        let live = live_set(&rows, &[remote]);
        let want: HashSet<String> = [0, 1, 2, 5].iter().map(|i| ids[*i].to_string()).collect();
        assert_eq!(live, want);
    }

    /// 207 §3.5: derive puts lineage, liveness, blocks, deadlines, risk and tool on
    /// items; task-born items are live only through their checkout session.
    #[test]
    fn derive_fills_the_207_fields() {
        let s = runtime::SessionId::new();
        let a = ApprovalRequested {
            id: ApprovalId(uuid::Uuid::now_v7()),
            session: s,
            tool: "shell".into(),
            args: serde_json::json!({ "command": "kubectl delete deployment x" }),
            risk: tools::Risk::Destructive,
            human_only: false,
            always_ask: false,
        };
        let approval_id = format!("approval:{}", a.id.0);
        let ops = tasks::Owner::Agent("ops".into());
        let mut blocked = task(
            "b",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["person:x"],
            None,
            40,
        );
        blocked.project = Some("tasks-proj".into());
        let inputs = Inputs {
            approvals: vec![(a.clone(), 40)],
            tasks: vec![blocked.clone()],
            // The blocked task's session is known from its events, not a checkout.
            task_sessions: HashMap::from([(blocked.id.0.clone(), s.to_string())]),
            lineage: HashMap::from([(
                s.to_string(),
                Lineage {
                    project: Some("release-2.5".into()),
                    root: "root-1".into(),
                    parent: Some("root-1".into()),
                    path: vec!["Ship 2.5 release".into(), "builder".into()],
                },
            )]),
            live: HashSet::from([s.to_string()]),
            blocks: HashMap::from([(s.to_string(), 2)]),
            deadlines: HashMap::from([(approval_id.clone(), 40 + 300_000)]),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let ap = items.iter().find(|i| i.id == approval_id).unwrap();
        assert_eq!(
            ap.title, "builder wants to run shell",
            "who falls back to the path"
        );
        assert_eq!(
            (ap.risk, ap.tool.as_deref()),
            (Some(tools::Risk::Destructive), Some("shell"))
        );
        assert_eq!(ap.deadline_ms, Some(300_040));
        assert_eq!(ap.project.as_deref(), Some("release-2.5"));
        assert_eq!(ap.root_session.as_deref(), Some("root-1"));
        assert_eq!(ap.parent_session.as_deref(), Some("root-1"));
        assert_eq!(ap.path, ["Ship 2.5 release", "builder"]);
        assert!(ap.session_live);
        assert_eq!(ap.blocks, 2);
        assert_eq!(ap.score, 600 + 300 + 100 + 80);
        let b = items
            .iter()
            .find(|i| i.kind == AttentionKind::Blocked)
            .unwrap();
        assert!(
            !b.session_live,
            "a task-born item is live only through its checkout"
        );
        assert_eq!(
            b.project.as_deref(),
            Some("release-2.5"),
            "the session's project wins"
        );
        // Without a known session the task's project stands in.
        let bare = Inputs {
            tasks: vec![blocked],
            ..Default::default()
        };
        let items = derive(&bare, &HashMap::new(), 100);
        assert_eq!(items[0].project.as_deref(), Some("tasks-proj"));
        assert!(items[0].path.is_empty());
    }

    /// 207 §5.2 / D7, E3: a task blocked only by decisions the user left to
    /// the agent stays visible as a low notice (base class Notice, no alert);
    /// with any other blocker it is the ordinary blocked item.
    #[test]
    fn left_blocker_is_a_quiet_notice() {
        use crate::score::{BaseClass, base_class};
        let ops = tasks::Owner::Agent("ops".into());
        let left = task(
            "ship it",
            tasks::TaskState::Blocked,
            ops.clone(),
            &["left:decision:d1"],
            None,
            40,
        );
        let both = task(
            "u",
            tasks::TaskState::Blocked,
            ops,
            &["left:decision:d2", "person:x"],
            None,
            40,
        );
        let inputs = Inputs {
            tasks: vec![left.clone(), both.clone()],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let quiet = items
            .iter()
            .find(|i| i.id == format!("blocked:{}", left.id))
            .expect("the left decision stays visible");
        assert_eq!(quiet.kind, AttentionKind::Blocked);
        assert_eq!(
            quiet.title,
            "\u{201c}ship it\u{201d}: decision left to the agent"
        );
        assert_eq!(quiet.detail.as_deref(), Some(LEFT_DECISION));
        assert_eq!(base_class(quiet), BaseClass::Notice);
        assert_eq!(crate::alert::class_of(quiet), None);
        let loud = items
            .iter()
            .find(|i| i.id == format!("blocked:{}", both.id))
            .expect("ordinary blocked item");
        assert_eq!(base_class(loud), BaseClass::Yours);
        assert!(crate::alert::class_of(loud).is_some());
    }

    /// 207 §8: a Source item without a priority borrows its session's checkout task.
    #[test]
    fn source_items_borrow_the_checkout_priority() {
        let ops = tasks::Owner::Agent("ops".into());
        let mut t = task("t", tasks::TaskState::Doing, ops, &[], None, 5);
        t.priority = tasks::Priority::Urgent;
        t.checkout = Some(tasks::Checkout {
            session: "work-1".into(),
            engine: "e1".into(),
            epoch: 1,
            progress_at_ms: 5,
            orphaned: false,
            taken: false,
        });
        let ext = AttentionItem {
            session: Some("work-1".into()),
            ..AttentionItem::new(
                "double:q".into(),
                AttentionKind::Double,
                "d".into(),
                None,
                50,
            )
        };
        let inputs = Inputs {
            tasks: vec![t],
            external: vec![ext],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let d = items.iter().find(|i| i.id == "double:q").unwrap();
        assert_eq!((d.priority, d.rank), (Some(tasks::Priority::Urgent), 9));
    }

    /// 207 §5.1: ids xor filter; a filter matches kinds any-of, projects any-of
    /// (`""` = no project) and `at_ms <= before_ms`, in list order.
    #[test]
    fn target_validation_and_filter_matching() {
        let it = |id: &str, kind, project: Option<&str>, at_ms| AttentionItem {
            project: project.map(str::to_owned),
            ..AttentionItem::new(id.into(), kind, "t".into(), None, at_ms)
        };
        let items = [
            it("show:a", AttentionKind::Show, Some("p1"), 10),
            it("show:b", AttentionKind::Show, None, 20),
            it("finished:c:t", AttentionKind::Finished, Some("p2"), 30),
            it("show:d", AttentionKind::Show, Some("p2"), 40),
        ];
        let ids = AttentionTarget {
            ids: vec!["show:d".into(), "gone:x".into()],
            filter: None,
        };
        assert_eq!(
            ids.resolve(&items).unwrap(),
            ["show:d", "gone:x"],
            "caller's order, unknown kept"
        );
        let both = AttentionTarget {
            ids: vec!["show:a".into()],
            filter: Some(AttentionFilter::default()),
        };
        assert_eq!(
            both.resolve(&items).unwrap_err(),
            "attention target: give ids or a filter, not both"
        );
        assert_eq!(
            AttentionTarget::default().resolve(&items).unwrap_err(),
            "attention target: give ids or a filter"
        );
        let filter = |f: AttentionFilter| {
            AttentionTarget {
                ids: vec![],
                filter: Some(f),
            }
            .resolve(&items)
            .unwrap()
        };
        assert_eq!(
            filter(AttentionFilter::default()).len(),
            4,
            "an empty filter matches all"
        );
        assert_eq!(
            filter(AttentionFilter {
                kinds: vec![AttentionKind::Show],
                projects: vec!["".into(), "p2".into()],
                before_ms: None,
            }),
            ["show:b", "show:d"]
        );
        assert_eq!(
            filter(AttentionFilter {
                before_ms: Some(30),
                ..Default::default()
            }),
            ["show:a", "show:b", "finished:c:t"]
        );
        // The wire form: both parts optional.
        let t: AttentionTarget =
            serde_json::from_value(serde_json::json!({ "filter": { "kinds": ["show"] } })).unwrap();
        assert_eq!(t.ids, Vec::<String>::new());
        assert_eq!(t.filter.unwrap().kinds, [AttentionKind::Show]);
    }

    /// D8: a budget item's time is the crossing — the budget task the roster
    /// filed for that agent — not `now`; without one `now` stays the fallback.
    #[test]
    fn budget_item_carries_the_crossing_time() {
        let inputs = Inputs {
            tasks: vec![
                task(
                    "Budget: ops at 80 % (33.00 of 40.00 USD)",
                    tasks::TaskState::Todo,
                    tasks::Owner::User,
                    &[],
                    None,
                    42,
                ),
                task(
                    "Budget override: other spent 10.00 of 10.00 USD in 2026-10",
                    tasks::TaskState::Todo,
                    tasks::Owner::User,
                    &[],
                    None,
                    7,
                ),
            ],
            agents: vec![
                agent(
                    "ops",
                    AgentStatus::Idle,
                    "40 USD/month".parse().unwrap(),
                    33.0,
                    None,
                    false,
                ),
                agent(
                    "dev",
                    AgentStatus::Idle,
                    "40 USD/month".parse().unwrap(),
                    39.0,
                    None,
                    false,
                ),
            ],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let at = |id: &str| {
            items
                .iter()
                .find(|i| i.id == id)
                .map(|i| (i.at_ms, i.waiting_since_ms))
        };
        assert_eq!(at("budget:ops"), Some((42, Some(42))));
        assert_eq!(
            at("budget:dev"),
            Some((100, Some(100))),
            "no filed task → now"
        );
    }

    /// 205: one `web_view:<tile>` item per open tile: the page's title (else its host), the address as
    /// detail, shown-class base without the live bonus, below an ask; a dismissal sticks until the
    /// next `show_web` re-targets the tile.
    #[test]
    fn web_view_items_are_quiet_and_keyed_by_tile() {
        let s = runtime::SessionId::new();
        let tile = |id: &str, url: &str, host: &str, title: &str, at: u64| artifact::webview::OpenTile {
            session: s,
            id: id.into(),
            url: url.into(),
            title: title.into(),
            host: host.into(),
            opened_ms: at,
        };
        let mut inputs = Inputs {
            web_views: vec![
                tile("00000000000000aa", "http://localhost:3000/orders", "localhost:3000", "Orders", 10),
                tile("00000000000000bb", "https://docs.rs/", "docs.rs", "", 20),
            ],
            live: [s.to_string()].into_iter().collect(),
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 60_010);
        let a = items.iter().find(|i| i.id == "web_view:00000000000000aa").unwrap();
        assert_eq!(a.kind, AttentionKind::WebView);
        assert_eq!(a.title, "Orders");
        assert_eq!(a.detail.as_deref(), Some("http://localhost:3000/orders"));
        assert_eq!(a.session.as_deref(), Some(s.to_string().as_str()));
        assert_eq!(a.waiting_since_ms, None);
        assert!(a.session_live);
        // 120 shown + 1 min of age; no +300 for the live session.
        assert_eq!(a.score, 121, "{:?}", a.reasons);
        assert!(a.reasons.iter().all(|r| r.signal != AttentionSignal::Live));
        assert_eq!(a.reasons[0].text, "web page");
        let b = items.iter().find(|i| i.id == "web_view:00000000000000bb").unwrap();
        assert_eq!(b.title, "docs.rs", "an untitled page is named by its host");

        let states = HashMap::from([(
            "web_view:00000000000000aa".to_owned(),
            State { dismissed_ms: Some(50), snoozed_until_ms: None },
        )]);
        assert!(derive(&inputs, &states, 60_010).iter().all(|i| i.id != "web_view:00000000000000aa"));
        // A later `show_web` for the same origin re-targets the tile: the item is back.
        inputs.web_views[0].opened_ms = 70;
        assert!(derive(&inputs, &states, 60_010).iter().any(|i| i.id == "web_view:00000000000000aa"));
        // The tile closed: no item.
        inputs.web_views.clear();
        assert!(derive(&inputs, &HashMap::new(), 60_010).is_empty());
    }
}
