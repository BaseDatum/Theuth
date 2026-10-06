//! Attention inbox (70 HUD, paperclip's attention service): things only the
//! user can decide, *derived* on every `attention_list` from what already
//! exists — permission prompts (subscribed [`ApprovalRequested`]), tasks
//! blocked on a `decision:` or a person, tasks in review with the user as
//! reviewer, proposed agents (the hire approval), budgets at 80 % / paused,
//! unreachable mesh peers, unresolved desktop hand-offs and unanswered secret
//! requests (79 §6.6, replicated rows). Only dismiss / snooze per item id is
//! stored (`attention_state`, engine-local). Badge count = the returned list's length.

use std::collections::HashMap;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved};

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
}

/// One inbox item. `id` is stable across derivations (dismiss / snooze key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionItem {
    /// `approval:<id>` | `decision:<task>:<decision>` | `hire:<agent>` | `blocked:<task>` | `review:<task>` | `budget:<agent>` | `node:<id>` | `handoff:<id>` | `secret:<id>`.
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
    /// The agent, for hire / budget items (and the task owner otherwise).
    pub agent: Option<String>,
    /// The session, for approvals.
    pub session: Option<String>,
    /// The approval id, for approvals.
    pub approval: Option<ApprovalId>,
    /// The `decision:<id>` blocker, for decisions.
    pub decision: Option<String>,
    /// The packet key (`decision:<id>` | `review:<task>`) when one exists (101): `packet_get` reads it.
    pub packet: Option<String>,
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
}

/// Dismissed / snoozed state per item id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// When it was dismissed; `Some` = hidden for good.
    pub dismissed_ms: Option<u64>,
    /// Hidden until then.
    pub snoozed_until_ms: Option<u64>,
}

impl State {
    fn hidden(self, now_ms: u64) -> bool {
        self.dismissed_ms.is_some() || self.snoozed_until_ms.is_some_and(|u| u > now_ms)
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
    };

    for (a, at_ms) in &inputs.approvals {
        let args = match &a.args {
            serde_json::Value::Object(m) => {
                m.get("command").and_then(|v| v.as_str()).map(str::to_owned)
            }
            _ => None,
        };
        out.push(AttentionItem {
            task: None,
            agent: None,
            session: Some(a.session.to_string()),
            approval: Some(a.id),
            decision: None,
            ..item(
                format!("approval:{}", a.id.0),
                AttentionKind::Approval,
                format!("{} wants to run {}", a.session, a.tool),
                args,
                *at_ms,
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
        match t.state {
            tasks::TaskState::Blocked => {
                let decisions: Vec<&String> = t
                    .blockers
                    .iter()
                    .filter(|b| b.starts_with("decision:"))
                    .collect();
                if let Some(d) = decisions.first() {
                    let packet = packets.get(d.as_str());
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        decision: Some((*d).clone()),
                        packet: packet.map(|p| p.key.clone()),
                        ..item(
                            format!("decision:{}:{d}", t.id),
                            AttentionKind::Decision,
                            format!(
                                "{} asks for a decision on \u{201c}{}\u{201d}",
                                t.owner, t.title
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
                } else if !t.blockers.is_empty() {
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
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
                    packet: packet.map(|_| key.clone()),
                    ..item(
                        key,
                        AttentionKind::Review,
                        format!("\u{201c}{}\u{201d} awaits your review", t.title),
                        Some(packet.map_or_else(
                            || format!("by {}", t.owner),
                            |p| tasks::packet::head(&p.summary),
                        )),
                        t.updated_ms,
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
            out.push(AttentionItem {
                task: hire_task.map(|t| t.id.clone()),
                agent: Some(name.clone()),
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

    // 79 §6.6: who = the agent's title, else the session's short id.
    let who = |agent: Option<&str>, session: &str| -> String {
        agent
            .and_then(|n| inputs.agents.iter().find(|a| a.file.name.as_str() == n))
            .map(|a| a.file.title.clone())
            .or_else(|| agent.map(str::to_owned))
            .unwrap_or_else(|| session.chars().take(8).collect())
    };
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
        out.push(AttentionItem {
            session: Some(r.session.clone()),
            ..item(
                format!("secret:{}", r.id.0),
                AttentionKind::Secret,
                format!("{} asks for a secret: {what}", who(None, &r.session)),
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
            .hidden(now_ms)
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

/// The [`ATTENTION_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Attention {
    pool: SqlitePool,
    pending: HashMap<ApprovalId, (ApprovalRequested, u64)>,
}

impl Attention {
    /// Spawn, register, subscribe to approvals.
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
        Inputs {
            tasks,
            packets,
            agents,
            approvals: self.pending.values().cloned().collect(),
            peers,
            handoffs,
            secrets,
            engines,
        }
    }
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
                dismissed_ms: Some(1),
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
                    dismissed_ms: Some(1),
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
}
