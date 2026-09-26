//! Attention inbox (70 HUD, paperclip's attention service): things only the
//! user can decide, *derived* on every `attention_list` from what already
//! exists — permission prompts (subscribed [`ApprovalRequested`]), tasks
//! blocked on a `decision:` or a person, tasks in review with the user as
//! reviewer, proposed agents (the hire approval), budgets at 80 % / paused,
//! unreachable mesh peers. Only dismiss / snooze per item id is stored
//! (`attention_state`, engine-local). Badge count = the returned list's length.

use std::collections::HashMap;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved};

use crate::store::Store;
use crate::types::{AgentStatus, AgentSummary, Budget};

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
}

/// One inbox item. `id` is stable across derivations (dismiss / snooze key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttentionItem {
    /// `approval:<id>` | `decision:<task>:<decision>` | `hire:<agent>` | `blocked:<task>` | `review:<task>` | `budget:<agent>` | `node:<id>`.
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
    /// The roster.
    pub agents: Vec<AgentSummary>,
    /// Pending permission prompts with when they arrived.
    pub approvals: Vec<(ApprovalRequested, u64)>,
    /// Mesh peers.
    pub peers: Vec<mesh::Peer>,
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
    };

    for (a, at_ms) in &inputs.approvals {
        let args = match &a.args {
            serde_json::Value::Object(m) => m.get("command").and_then(|v| v.as_str()).map(str::to_owned),
            _ => None,
        };
        out.push(AttentionItem {
            task: None,
            agent: None,
            session: Some(a.session.to_string()),
            approval: Some(a.id),
            decision: None,
            ..item(format!("approval:{}", a.id.0), AttentionKind::Approval, format!("{} wants to run {}", a.session, a.tool), args, *at_ms)
        });
    }

    // Hire tasks owned by the user are the proposal's approval: one item per agent, not two.
    let hire_prefix = "Hire agent ";
    for t in &inputs.tasks {
        let owner_agent = match &t.owner {
            tasks::Owner::Agent(a) => Some(a.clone()),
            _ => None,
        };
        match t.state {
            tasks::TaskState::Blocked => {
                let decisions: Vec<&String> = t.blockers.iter().filter(|b| b.starts_with("decision:")).collect();
                if let Some(d) = decisions.first() {
                    out.push(AttentionItem {
                        task: Some(t.id.clone()),
                        agent: owner_agent.clone(),
                        decision: Some((*d).clone()),
                        ..item(
                            format!("decision:{}:{d}", t.id),
                            AttentionKind::Decision,
                            format!("{} asks for a decision on \u{201c}{}\u{201d}", t.owner, t.title),
                            Some("the question is in the task thread".into()),
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
                out.push(AttentionItem {
                    task: Some(t.id.clone()),
                    agent: owner_agent.clone(),
                    ..item(
                        format!("review:{}", t.id),
                        AttentionKind::Review,
                        format!("\u{201c}{}\u{201d} awaits your review", t.title),
                        Some(format!("by {}", t.owner)),
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
            let hire_task = inputs
                .tasks
                .iter()
                .find(|t| t.title.starts_with(hire_prefix) && t.title[hire_prefix.len()..].starts_with(&name) && !matches!(t.state, tasks::TaskState::Done | tasks::TaskState::Cancelled));
            out.push(AttentionItem {
                task: hire_task.map(|t| t.id.clone()),
                agent: Some(name.clone()),
                ..item(
                    format!("hire:{name}"),
                    AttentionKind::Hire,
                    format!("{} proposes hiring {} ({})", a.file.proposed_by.clone().unwrap_or_else(|| "someone".into()), name, a.file.title),
                    Some(format!("reports to {} \u{b7} budget {}", a.file.reports_to, a.file.budget)),
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
            let detail = if a.file.paused { "paused at the cap; un-pause to override for the month" } else { "at 80 %: the cap pauses it" };
            out.push(AttentionItem {
                agent: Some(name.clone()),
                ..item(
                    format!("budget:{name}"),
                    AttentionKind::Budget,
                    format!("{name} spent {:.2} of {amount:.2} {currency} ({pct:.0} %)", a.spent_month),
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

    out.retain(|i| !states.get(&i.id).copied().unwrap_or_default().hidden(now_ms));
    out.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    out
}

fn stamp(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map_or_else(|| "never".into(), |d| d.format("%Y-%m-%d %H:%M UTC").to_string())
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
                    state: Some(vec![tasks::TaskState::Todo, tasks::TaskState::Doing, tasks::TaskState::Review, tasks::TaskState::Blocked]),
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
        let peers = match mesh::actor() {
            Some(m) => m.ask(mesh::Status).await.ok().map(|s| s.peers).unwrap_or_default(),
            None => vec![],
        };
        Inputs {
            tasks,
            agents,
            approvals: self.pending.values().cloned().collect(),
            peers,
        }
    }
}

/// Every stored dismiss / snooze.
pub async fn states(pool: &SqlitePool) -> anyhow::Result<HashMap<String, State>> {
    let rows: Vec<(String, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT id, dismissed_ms, snoozed_until_ms FROM attention_state").fetch_all(pool).await?;
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
pub async fn set_state(pool: &SqlitePool, id: &str, until_ms: Option<u64>, now_ms: u64) -> anyhow::Result<()> {
    match until_ms {
        Some(u) => {
            sqlx::query(
                "INSERT INTO attention_state (id, snoozed_until_ms) VALUES (?, ?) \
                 ON CONFLICT(id) DO UPDATE SET snoozed_until_ms = excluded.snoozed_until_ms",
            )
            .bind(id)
            .bind(u as i64)
        }
        None => {
            sqlx::query(
                "INSERT INTO attention_state (id, dismissed_ms) VALUES (?, ?) \
                 ON CONFLICT(id) DO UPDATE SET dismissed_ms = excluded.dismissed_ms",
            )
            .bind(id)
            .bind(now_ms as i64)
        }
    }
    .execute(pool)
    .await?;
    Ok(())
}

impl Message<ApprovalRequested> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: ApprovalRequested, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pending.insert(m.id, (m, now_ms()));
    }
}

impl Message<ApprovalResolved> for Attention {
    type Reply = ();

    async fn handle(&mut self, m: ApprovalResolved, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pending.remove(&m.id);
    }
}

impl Message<ListAttention> for Attention {
    type Reply = Result<Vec<AttentionItem>, String>;

    async fn handle(&mut self, _: ListAttention, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let inputs = self.gather().await;
        let states = states(&self.pool).await.map_err(|e| e.to_string())?;
        Ok(derive(&inputs, &states, now_ms()))
    }
}

impl Message<SnoozeAttention> for Attention {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SnoozeAttention, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        set_state(&self.pool, &m.id, Some(m.until_ms), now_ms()).await.map_err(|e| e.to_string())
    }
}

impl Message<DismissAttention> for Attention {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: DismissAttention, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        set_state(&self.pool, &m.0, None, now_ms()).await.map_err(|e| e.to_string())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The attention actor; `None` before the roster plugin started.
pub fn actor() -> Option<ActorRef<Attention>> {
    ActorRef::<Attention>::lookup(ATTENTION_ACTOR).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentFile, AgentName};

    fn task(title: &str, state: tasks::TaskState, owner: tasks::Owner, blockers: &[&str], reviewer: Option<tasks::Owner>, updated_ms: u64) -> tasks::Task {
        tasks::Task {
            id: tasks::TaskId::new(),
            title: title.into(),
            brief: String::new(),
            done_when: None,
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

    fn agent(name: &str, status: AgentStatus, budget: Budget, spent: f64, proposed_by: Option<&str>, paused: bool) -> AgentSummary {
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
                task("deploy", tasks::TaskState::Blocked, ops.clone(), &["decision:d1"], None, 50),
                task("migrate", tasks::TaskState::Blocked, ops.clone(), &["person:sky"], None, 40),
                task("review me", tasks::TaskState::Review, ops.clone(), &[], Some(tasks::Owner::User), 30),
                task("not mine", tasks::TaskState::Review, ops.clone(), &[], Some(ops.clone()), 30),
                task("Hire agent amy (AMY)", tasks::TaskState::Todo, tasks::Owner::User, &[], None, 20),
                task("plain", tasks::TaskState::Doing, ops.clone(), &[], None, 10),
            ],
            agents: vec![
                agent("amy", AgentStatus::Proposed, Budget::None, 0.0, Some("agent:ops"), true),
                agent("ops", AgentStatus::Idle, "40 USD/month".parse().unwrap(), 33.0, None, false),
                agent("dev", AgentStatus::Idle, "40 USD/month".parse().unwrap(), 10.0, None, false),
                agent("cap", AgentStatus::Paused, "10 USD/month".parse().unwrap(), 10.0, None, true),
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
        let hire = items.iter().find(|i| i.kind == AttentionKind::Hire).unwrap();
        assert_eq!(hire.id, "hire:amy");
        assert!(hire.task.is_some(), "the open hire task is linked");
        assert!(!items.iter().any(|i| i.title.starts_with("\u{201c}Hire agent")), "the hire task is not a second item");
        let decision = items.iter().find(|i| i.kind == AttentionKind::Decision).unwrap();
        assert_eq!(decision.decision.as_deref(), Some("decision:d1"));
        assert_eq!(items.iter().filter(|i| i.kind == AttentionKind::Budget).map(|i| i.agent.clone().unwrap()).collect::<Vec<_>>(), ["ops", "cap"]);

        let mut states = HashMap::new();
        states.insert(hire.id.clone(), State { dismissed_ms: Some(1), snoozed_until_ms: None });
        states.insert(decision.id.clone(), State { dismissed_ms: None, snoozed_until_ms: Some(200) });
        states.insert("budget:ops".into(), State { dismissed_ms: None, snoozed_until_ms: Some(90) });
        let left = derive(&inputs, &states, 100);
        assert_eq!(left.len(), 5, "dismissed and live-snoozed gone, expired snooze back");
        assert!(left.iter().any(|i| i.id == "budget:ops"));
    }

    #[tokio::test]
    async fn state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        set_state(store.pool(), "a", Some(500), 1).await.unwrap();
        set_state(store.pool(), "b", None, 2).await.unwrap();
        set_state(store.pool(), "a", Some(600), 3).await.unwrap();
        let s = states(store.pool()).await.unwrap();
        assert_eq!(s["a"], State { dismissed_ms: None, snoozed_until_ms: Some(600) });
        assert_eq!(s["b"], State { dismissed_ms: Some(2), snoozed_until_ms: None });
    }
}
