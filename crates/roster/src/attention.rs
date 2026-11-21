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
}

/// One inbox item. `id` is stable across derivations (dismiss / snooze key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionItem {
    /// `approval:<id>` | `decision:<task>:<decision>` | `hire:<agent>` | `blocked:<task>` | `review:<task>` | `budget:<agent>` | `node:<id>` | `misplaced:<task>` | `handoff:<id>` | `secret:<id>` | `ask:<id>` | `show:<call_id>` | `resume:<session>:<turn>`.
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

/// 130 E1: bus event the [`Attention`] actor publishes after a derive whose
/// result differs from the previous one by id set; `count` is the new badge.
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
}

impl wire::WireEvent for AttentionChanged {
    const TOPIC: &'static str = "roster.attention";
}

/// 130 E3: one `attention_finished` row — a root / desk session's last clean
/// turn; the item id is `finished:<session>:<turn>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// Owning session (one row per session: the newest turn replaces the last).
    pub session: String,
    /// The turn that finished.
    pub turn: String,
    /// ms since epoch: when it finished.
    pub at_ms: u64,
    /// First line of the final text (≤ 160 chars), else the session title.
    pub summary: Option<String>,
    /// The `## Next` list, ≤ 2 × 80 chars.
    pub next_steps: Vec<String>,
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

/// `ask` → `Result<Vec<AttentionItem>, String>`: undismissed, unsnoozed, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAttention;

/// `ask` → `Result<(), String>`: hide `id` until `until_ms`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnoozeAttention {
    /// Item id.
    pub id: String,
    /// ms since epoch.
    pub until_ms: u64,
}

/// `ask` → `Result<(), String>`: hide `id` for good.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DismissAttention(pub String);

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

/// The inbox from its inputs: pure, hidden items removed, ordered rank desc →
/// waiting_since asc (None last) → at_ms desc (132 D4).
pub fn derive(inputs: &Inputs, states: &HashMap<String, State>, now_ms: u64) -> Vec<AttentionItem> {
    let mut out = Vec::new();
    let item = |id: String, kind, title: String, detail: Option<String>, at_ms| AttentionItem {
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
    };
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
    // 79 §6.6: who = the agent's title, else the session's short id.
    let who = |agent: Option<&str>, session: &str| -> String {
        agent
            .and_then(|n| inputs.agents.iter().find(|a| a.file.name.as_str() == n))
            .map(|a| a.file.title.clone())
            .or_else(|| agent.map(str::to_owned))
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
        let (priority, rank) = by_session(&session);
        out.push(AttentionItem {
            agent: desk(&session),
            session: Some(session),
            approval: Some(a.id),
            priority,
            rank,
            ..item(
                format!("approval:{}", a.id.0),
                AttentionKind::Approval,
                format!("{} wants to run {}", a.session, a.tool),
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
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(session.clone()),
            ask: Some(a.clone()),
            waiting_since_ms: Some(inputs.ask_since.get(&a.id).copied().unwrap_or(now_ms)),
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
                now_ms,
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
                    human_bytes(s.bytes),
                    s.version
                )),
                s.at_ms,
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
                } else if t.blockers.iter().any(|b| !b.starts_with("decision:")) {
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
    out.extend(inputs.external.iter().cloned());

    // 130 E3: one receipt per root / desk session; waits on nobody.
    for f in &inputs.finished {
        let agent = desk(&f.session);
        let name = who(agent.as_deref(), &f.session);
        let (priority, rank) = by_session(&f.session);
        out.push(AttentionItem {
            agent,
            session: Some(f.session.clone()),
            waiting_since_ms: None,
            summary: f.summary.clone(),
            next_steps: f.next_steps.clone(),
            priority,
            rank,
            ..item(
                format!("finished:{}:{}", f.session, f.turn),
                AttentionKind::Finished,
                format!("{name} finished"),
                f.summary.clone(),
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

    out.retain(|i| {
        !states
            .get(&i.id)
            .copied()
            .unwrap_or_default()
            .hidden(now_ms, i.at_ms)
    });
    // 132 D4: rank desc → waiting_since asc (None last) → at_ms desc.
    out.sort_by(|a, b| {
        b.rank
            .cmp(&a.rank)
            .then_with(|| match (a.waiting_since_ms, b.waiting_since_ms) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| b.at_ms.cmp(&a.at_ms))
    });
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

/// `12 B` · `3.4 KB` · `1.2 MB` · `2.0 GB`.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
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
    /// 130 E1: the id set of the last derive; `announce` diffs against it.
    last_ids: HashSet<String>,
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
                last_ids: HashSet::new(),
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
        if let Ok(Some(ag)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            if let Ok(sessions) = ag.ask(agent::ListSessions).await {
                desks.extend(desk_map(&sessions));
            }
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
                && let Ok(store) = ag.ask(agent::DurableStore).await
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
        }
    }

    /// 130 E1: derive now and announce the difference from the last derive.
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
        if let Some(ev) = changed(&self.last_ids, &items, now) {
            self.last_ids = items.iter().map(|i| i.id.clone()).collect();
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

    /// 130 E3: a root / desk turn ended cleanly with nothing pending → one
    /// `attention_finished` row for the session (the previous turn's is replaced).
    async fn finish_turn(&mut self, session: runtime::SessionId, turn: runtime::TurnId) {
        let text = self.texts.remove(&session).unwrap_or_default();
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
        let (summary, next_steps) = parse_finished(&text, chain[0].title.as_deref());
        let row = Finished {
            session: session.to_string(),
            turn: turn.to_string(),
            at_ms: now_ms(),
            summary,
            next_steps,
        };
        if let Err(e) = record_finished(&self.pool, &row).await {
            tracing::warn!("attention: record finished {session}: {e:#}");
        }
        self.recheck_quietly().await;
    }
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

/// 130 E1: the `AttentionChanged` a derive raises against the previous id set; `None` when equal.
pub fn changed(
    prev: &HashSet<String>,
    items: &[AttentionItem],
    at_ms: u64,
) -> Option<AttentionChanged> {
    let now: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut ids_added: Vec<String> = now
        .iter()
        .filter(|id| !prev.contains(**id))
        .map(|id| (*id).to_owned())
        .collect();
    let mut ids_removed: Vec<String> = prev
        .iter()
        .filter(|id| !now.contains(id.as_str()))
        .cloned()
        .collect();
    if ids_added.is_empty() && ids_removed.is_empty() {
        return None;
    }
    ids_added.sort();
    ids_removed.sort();
    Some(AttentionChanged {
        count: items.len() as u32,
        ids_added,
        ids_removed,
        at_ms,
    })
}

/// 130 E3: upsert the session's receipt (one row per session; the newest turn wins).
pub async fn record_finished(pool: &SqlitePool, f: &Finished) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO attention_finished (session, turn, at_ms, summary, next_steps) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(session) DO UPDATE SET turn = excluded.turn, at_ms = excluded.at_ms, \
         summary = excluded.summary, next_steps = excluded.next_steps",
    )
    .bind(&f.session)
    .bind(&f.turn)
    .bind(f.at_ms as i64)
    .bind(&f.summary)
    .bind(serde_json::to_string(&f.next_steps)?)
    .execute(pool)
    .await?;
    Ok(())
}

/// 130 E3: undismissed `attention_finished` rows, newest first (snooze is `derive`'s job).
pub async fn finished(pool: &SqlitePool) -> anyhow::Result<Vec<Finished>> {
    let rows: Vec<(String, String, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT f.session, f.turn, f.at_ms, f.summary, f.next_steps FROM attention_finished f \
         LEFT JOIN attention_state st ON st.id = 'finished:' || f.session || ':' || f.turn \
         WHERE st.dismissed_ms IS NULL ORDER BY f.at_ms DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(session, turn, at_ms, summary, next_steps)| Finished {
            session,
            turn,
            at_ms: at_ms as u64,
            summary,
            next_steps: serde_json::from_str(&next_steps).unwrap_or_default(),
        })
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
            agent::AgentEvent::TurnStarted { session, .. }
            | agent::AgentEvent::ToolCallStarted { session, .. } => {
                self.texts.insert(session, String::new());
            }
            agent::AgentEvent::TextDelta { session, text, .. } => {
                // ponytail: unbounded per running turn; a model's final answer is tens of KB at most.
                self.texts.entry(session).or_default().push_str(&text);
            }
            agent::AgentEvent::TurnFinished {
                session,
                turn,
                stop_reason,
                ..
            } => {
                if stop_reason == "end_turn" {
                    self.finish_turn(session, turn).await;
                } else {
                    self.texts.remove(&session);
                }
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

/// 146 D4: the boot pass's counts, re-sent as respawns settle. The item id
/// is stable (`resume:boot:<at_ms>`), so a count change alone does not move
/// the id set: the HUD is nudged with an empty-diff `AttentionChanged`. A
/// settled pass that lost nothing clears itself 60 s later; one with losses
/// stays until dismissed.
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
        let before = self.last_ids.clone();
        if let Ok(items) = self.recheck().await
            && before == self.last_ids
        {
            self.rt
                .publish(AttentionChanged {
                    count: items.len() as u32,
                    ids_added: Vec::new(),
                    ids_removed: Vec::new(),
                    at_ms: now_ms(),
                })
                .await;
        }
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

    /// 155 §7.4: a delay window closed; derive again so the alert shows, and
    /// nudge clients with an empty-diff `AttentionChanged` when the id set
    /// alone would not have (the item is the same, its `alert` is new).
    async fn handle(&mut self, _: AlertDue, _: &mut Context<Self, Self::Reply>) {
        self.alert_due = None;
        let before = self.last_ids.clone();
        if let Ok(items) = self.recheck().await
            && before == self.last_ids
        {
            self.rt
                .publish(AttentionChanged {
                    count: items.len() as u32,
                    ids_added: Vec::new(),
                    ids_removed: Vec::new(),
                    at_ms: now_ms(),
                })
                .await;
        }
    }
}

impl Message<SnoozeAttention> for Attention {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SnoozeAttention,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        set_state(&self.pool, &m.id, Some(m.until_ms), now_ms())
            .await
            .map_err(|e| e.to_string())?;
        self.recheck_quietly().await;
        Ok(())
    }
}

impl Message<DismissAttention> for Attention {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: DismissAttention,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        set_state(&self.pool, &m.0, None, now_ms())
            .await
            .map_err(|e| e.to_string())?;
        self.recheck_quietly().await;
        Ok(())
    }
}

impl Message<RegisterSource> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: RegisterSource, _: &mut Context<Self, ()>) {
        self.sources.push(m.0);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
        // 132 D4: equal rank → the longest-waiting first; budgets without a filed task wait since `now`.
        assert_eq!(
            kinds,
            [
                AttentionKind::Node,
                AttentionKind::Hire,
                AttentionKind::Review,
                AttentionKind::Blocked,
                AttentionKind::Decision,
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
            ["ops", "cap"]
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
        // 132 D4 order: the review (waiting since 40) before the decision (50); legacy strings stay.
        let items = derive(&bare, &HashMap::new(), 100);
        assert_eq!(items[1].packet, None);
        assert_eq!(
            items[1].title,
            "agent:ops asks for a decision on \u{201c}deploy\u{201d}"
        );
        assert_eq!(
            items[1].detail.as_deref(),
            Some("the question is in the task thread")
        );
        assert_eq!(items[0].packet, None);
        assert_eq!(items[0].title, "\u{201c}migrate\u{201d} awaits your review");
        assert_eq!(items[0].detail.as_deref(), Some("by agent:ops"));

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
        assert_eq!(items[1].kind, AttentionKind::Decision);
        assert_eq!(items[1].packet.as_deref(), Some("decision:d1"));
        assert_eq!(items[1].title, "Cap retries at 3?");
        assert_eq!(items[1].detail.as_deref(), Some("agent:ops \u{b7} deploy"));
        assert_eq!(items[0].kind, AttentionKind::Review);
        assert_eq!(items[0].packet.as_deref(), Some(review_key.as_str()));
        let title = items[0].title.as_str();
        assert!(title.starts_with("Summary line sss"), "{title}");
        assert_eq!(title.chars().count(), tasks::packet::ROW_BUDGET);
        assert!(title.ends_with('…'));
        assert_eq!(
            items[0].detail.as_deref(),
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
            [AttentionKind::Show, AttentionKind::Ask, AttentionKind::Ask],
            "the show has waited since 50, the asks since now"
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
        let s = &items[0];
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
        assert_eq!(sessions, [None, Some("work-2"), Some("work-1")]);
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

    /// 130 E1: equal id sets are silent; an added id, a dismissed id each announce once.
    #[test]
    fn attention_changed_diffs_id_sets() {
        let desk = runtime::SessionId::new();
        let one = ask(desk, &[("Ship?", &["Yes"])]);
        let two = ask(desk, &[("Deploy?", &["Yes"])]);
        let mut inputs = Inputs {
            asks: vec![one.clone()],
            ..Default::default()
        };
        let first = derive(&inputs, &HashMap::new(), 100);
        let ev = changed(&HashSet::new(), &first, 100).expect("first derive announces");
        assert_eq!(
            (ev.count, ev.ids_added.len(), ev.ids_removed.len()),
            (1, 1, 0)
        );
        let prev: HashSet<String> = first.iter().map(|i| i.id.clone()).collect();
        assert_eq!(
            changed(&prev, &derive(&inputs, &HashMap::new(), 101), 101),
            None,
            "same ids: nothing"
        );
        inputs.asks.push(two.clone());
        let more = derive(&inputs, &HashMap::new(), 102);
        let ev = changed(&prev, &more, 102).unwrap();
        assert_eq!(ev.ids_added, [format!("ask:{}", two.id.0)]);
        assert!(ev.ids_removed.is_empty());
        assert_eq!(ev.count, 2);
        let prev: HashSet<String> = more.iter().map(|i| i.id.clone()).collect();
        let states = HashMap::from([(
            format!("ask:{}", one.id.0),
            State {
                dismissed_ms: Some(200),
                snoozed_until_ms: None,
            },
        )]);
        let ev = changed(&prev, &derive(&inputs, &states, 103), 103).unwrap();
        assert_eq!(ev.ids_removed, [format!("ask:{}", one.id.0)]);
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

    // ---- 132 ATTENTION-ROWS-AND-ORDER ---------------------------------------------

    /// D4: rank desc (project order step, then priority) → waiting_since asc (None
    /// last) → at_ms desc; a session item borrows its checkout task's priority/rank.
    #[test]
    fn order_is_rank_then_waiting_then_newest() {
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
                format!("blocked:{}", ranked.id).as_str(), // 102: project step beats priority
                format!("blocked:{}", urgent.id).as_str(), // 9
                "show:c1",                                 // 7: borrowed from the checkout task
                format!("blocked:{}", old.id).as_str(),    // 5, waiting since 10
                "handoff:h-checked", // 5, waiting since 50 (no checkout on its session)
                format!("blocked:{}", new.id).as_str(), // 5, waiting since 80
                "finished:root-1:t1", // 5, waits on nobody → last
            ],
            "{items:#?}"
        );
        assert_eq!(
            (items[0].priority, items[0].rank),
            (Some(tasks::Priority::Low), 102)
        );
        assert_eq!(
            (items[1].priority, items[1].rank),
            (Some(tasks::Priority::Urgent), 9)
        );
        assert_eq!(
            (items[2].priority, items[2].rank),
            (Some(tasks::Priority::High), 7)
        );
        assert_eq!((items[4].priority, items[4].rank), (None, 5));
        assert_eq!((items[6].priority, items[6].rank), (None, 5));
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
}
