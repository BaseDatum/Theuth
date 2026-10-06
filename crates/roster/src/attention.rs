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

use std::collections::HashMap;

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
}

/// One inbox item. `id` is stable across derivations (dismiss / snooze key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionItem {
    /// `approval:<id>` | `decision:<task>:<decision>` | `hire:<agent>` | `blocked:<task>` | `review:<task>` | `budget:<agent>` | `node:<id>` | `handoff:<id>` | `secret:<id>` | `ask:<id>` | `show:<call_id>`.
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
}

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

/// The inbox from its inputs: pure, newest first, hidden items removed.
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
        out.push(AttentionItem {
            agent: desk(&session),
            session: Some(session),
            approval: Some(a.id),
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
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(session.clone()),
            ask: Some(a.clone()),
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
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(s.session.clone()),
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
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        session: session.clone(),
                        decision: Some((*d).clone()),
                        packet: packet.map(|p| p.key.clone()),
                        ..item(
                            format!("decision:{}:{d}", t.id),
                            AttentionKind::Decision,
                            format!(
                                "{} asks for a decision on \u{201c}{}\u{201d}",
                                packet.map_or_else(|| t.owner.to_string(), |p| p.by.clone()),
                                t.title
                            ),
                            Some(packet.map_or_else(
                                || "the question is in the task thread".to_owned(),
                                |p| {
                                    tasks::packet::head(p.question.as_deref().unwrap_or(&p.summary))
                                },
                            )),
                            t.updated_ms,
                        )
                    });
                } else if t.blockers.iter().any(|b| !b.starts_with("decision:")) {
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        session: session.clone(),
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
                out.push(AttentionItem {
                    task: Some(t.id.clone()),
                    agent: owner_agent.clone(),
                    session: session.clone(),
                    packet: packet.map(|_| key.clone()),
                    ..item(
                        key,
                        AttentionKind::Review,
                        format!("\u{201c}{}\u{201d} awaits your review", t.title),
                        Some(packet.map_or_else(
                            || format!("by {}", t.owner),
                            |p| tasks::packet::head(&p.summary),
                        )),
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
            out.push(AttentionItem {
                task: hire_task.map(|t| t.id.clone()),
                agent: Some(name.clone()),
                session,
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
                    now_ms,
                )
            });
        }
    }

    for p in inputs.peers.iter().filter(|p| !p.reachable) {
        out.push(item(
            format!("node:{}", p.id.0),
            AttentionKind::Node,
            format!("{} is unreachable", p.name),
            Some(format!("last seen {}", stamp(p.last_seen_ms))),
            p.last_seen_ms,
        ));
    }

    let engine_name = |id: &str| {
        inputs
            .engines
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    };

    for h in &inputs.handoffs {
        out.push(AttentionItem {
            agent: h.agent.clone(),
            session: Some(h.session.clone()),
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
        out.push(AttentionItem {
            agent: agent.clone(),
            session: Some(r.session.clone()),
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

    out.retain(|i| {
        !states
            .get(&i.id)
            .copied()
            .unwrap_or_default()
            .hidden(now_ms, i.at_ms)
    });
    out.sort_by_key(|i| std::cmp::Reverse(i.at_ms));
    out
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

/// The [`ATTENTION_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Attention {
    pool: SqlitePool,
    pending: HashMap<ApprovalId, (ApprovalRequested, u64)>,
}

impl Attention {
    /// Spawn, register, subscribe to approvals and shows.
    pub async fn start(rt: &Runtime, store: &Store) -> anyhow::Result<ActorRef<Self>> {
        let actor = Self::spawn_with_mailbox(
            Self {
                pool: store.pool().clone(),
                pending: HashMap::new(),
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(ATTENTION_ACTOR)?;
        rt.subscribe::<Self, ApprovalRequested>(&actor).await;
        rt.subscribe::<Self, ApprovalResolved>(&actor).await;
        rt.subscribe::<Self, ArtifactShown>(&actor).await;
        Ok(actor)
    }

    async fn gather(&self) -> Inputs {
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
        let shows = shows(&self.pool).await.unwrap_or_else(|e| {
            tracing::warn!("attention: shows: {e:#}");
            vec![]
        });
        // 104 A3: session → desk agent, for desks and every session under one.
        let mut desks: HashMap<String, String> = agents
            .iter()
            .filter_map(|a| Some((a.desk.clone()?, a.file.name.as_str().to_owned())))
            .collect();
        if let Ok(Some(ag)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(sessions) = ag.ask(agent::ListSessions).await
        {
            desks.extend(desk_map(&sessions));
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
        }
    }
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
    }
}

impl Message<ArtifactShown> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: ArtifactShown, _: &mut Context<Self, Self::Reply>) {
        if let Err(e) = record_show(&self.pool, &m, now_ms()).await {
            tracing::warn!("attention: record show {}: {e:#}", m.call_id);
        }
    }
}

impl Message<ListAttention> for Attention {
    type Reply = Result<Vec<AttentionItem>, String>;

    async fn handle(
        &mut self,
        _: ListAttention,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let inputs = self.gather().await;
        let states = states(&self.pool).await.map_err(|e| e.to_string())?;
        Ok(derive(&inputs, &states, now_ms()))
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
            .map_err(|e| e.to_string())
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
            .map_err(|e| e.to_string())
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
                body: "x".into(),
            },
            status,
            desk: None,
            open_tasks: 0,
            spent_month: spent,
            error: None,
        }
    }

    #[test]
    fn derives_every_kind_and_hides_dismissed_or_snoozed() {
        let ops = tasks::Owner::Agent("ops".into());
        let inputs = Inputs {
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
            }],
            ..Default::default()
        };
        let items = derive(&inputs, &HashMap::new(), 100);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [
                AttentionKind::Budget,
                AttentionKind::Budget,
                AttentionKind::Decision,
                AttentionKind::Blocked,
                AttentionKind::Review,
                AttentionKind::Hire,
                AttentionKind::Node
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
        let items = derive(&bare, &HashMap::new(), 100);
        assert_eq!(items[0].packet, None);
        assert_eq!(
            items[0].detail.as_deref(),
            Some("the question is in the task thread")
        );
        assert_eq!(items[1].packet, None);
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
        let items = derive(&with, &HashMap::new(), 100);
        assert_eq!(items[0].kind, AttentionKind::Decision);
        assert_eq!(items[0].packet.as_deref(), Some("decision:d1"));
        assert_eq!(items[0].detail.as_deref(), Some("Cap retries at 3?"));
        assert_eq!(items[1].kind, AttentionKind::Review);
        assert_eq!(items[1].packet.as_deref(), Some(review_key.as_str()));
        let detail = items[1].detail.as_deref().unwrap();
        assert!(detail.starts_with("Summary line sss"), "{detail}");
        assert_eq!(detail.chars().count(), 140);
        assert!(detail.ends_with('…'));
        assert_eq!(items.len(), 2, "a stray packet makes no item");
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
        assert!(
            items[0].title.starts_with("agent:lead asks"),
            "{}",
            items[0].title
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
                })
                .collect(),
            created_ms,
            answered_ms: None,
            outcome: None,
            hlc: None,
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
                "secret:0199aaaa-0000-7000-8000-000000000001",
                "handoff:h2",
                "handoff:h1",
                "secret:0199aaaa-0000-7000-8000-000000000002",
            ],
            "newest first"
        );
        let h1 = &items[2];
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
            items[1].title, "0199aaaa needs you: log in to GitHub",
            "session short id"
        );
        let form = &items[0];
        assert_eq!(form.kind, AttentionKind::Secret);
        assert_eq!(form.title, "s-secret asks for a secret: GitHub login");
        assert_eq!(form.detail.as_deref(), Some("e9"), "unknown engine: the id");
        assert_eq!(items[3].title, "s-secret asks for a secret: TOKEN");

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
            ["handoff:h2", "secret:0199aaaa-0000-7000-8000-000000000002"]
        );
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
            "asks are now, the show is older"
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
        assert_eq!(sessions, [Some("work-1"), Some("work-2"), None]);
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
}
