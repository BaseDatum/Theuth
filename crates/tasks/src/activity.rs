//! Activity log (70 HUD): one append-only `activity` table in `tasks.sqlite`,
//! engine-local for now.
//!
//! Two feeds, no edits to the ledger: every task change already lands in
//! `task_events`, so the [`Activity`] actor tails that table (1 s poll,
//! cursor in `activity_cursor`) and maps each row to an [`ActivityKind`];
//! other plugins publish [`Record`] on the bus (roster: hires, pauses,
//! budget thresholds) and [`ScheduleEvent::Fired`] is subscribed directly.
//! Every appended row is published as [`ActivityAppended`] for the UI.

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use schedule::ScheduleEvent;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

use crate::store::Store;
use crate::types::TaskEvent;

/// Registry name of the [`Activity`] actor.
pub const ACTIVITY_ACTOR: &str = "activity";

/// Tail cadence.
const TAIL_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// Rows taken from `task_events` per tail pass.
const TAIL_BATCH: i64 = 500;
/// `activity_list` default and ceiling.
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// A task was created (`created` event).
    TaskCreated,
    /// A state change other than done.
    TaskMoved,
    /// `owner` changed.
    TaskReassigned,
    /// A session took the task (or the sweep orphaned it).
    TaskCheckedOut,
    /// A comment (not a progress note).
    TaskCommented,
    /// A desk was nudged about a task (field `nudge`).
    TaskNudged,
    /// `state` → done.
    TaskDone,
    /// A task with a parent was created.
    ChildSpawned,
    /// A task with a parent reached done.
    ChildReturned,
    /// A proposal was approved.
    AgentHired,
    /// `paused:` flipped either way.
    AgentPaused,
    /// `agent_propose` wrote a file.
    AgentProposed,
    /// An agent file was deleted.
    AgentRemoved,
    /// A schedule fired.
    RoutineFired,
    /// A `decision:` blocker was added.
    EscalationRaised,
    /// A `decision:` blocker was cleared.
    EscalationAnswered,
    /// 80 % or the cap crossed.
    BudgetThreshold,
    /// 130 E4: the user answered (or dismissed) an `ask_user` card.
    AskAnswered,
    /// 130 E4: the user allowed / denied a tool approval.
    ApprovalResolved,
}

impl ActivityKind {
    /// The serde name, also the `activity.kind` column value.
    pub fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
    }
}

/// One row of the feed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ActivityEntry {
    /// Row id, ascending.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ts_ms: u64,
    /// What.
    pub kind: ActivityKind,
    /// `agent:<name>` | `user` | `user@<client>` (130 E4: `desktop | tui | ios | companion`) | `system`.
    pub actor: String,
    /// Task / agent / session / routine id.
    pub subject: String,
    /// One line.
    pub summary: String,
    /// Spend attached to the entry, when known (USD).
    pub cost: Option<f64>,
}

/// `activity_list` filter.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields = nullable))]
#[serde(default)]
pub struct ActivityFilter {
    /// Only entries after this (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub since_ms: Option<u64>,
    /// Only these kinds.
    pub kinds: Option<Vec<ActivityKind>>,
    /// Newest first; default 50, max 500.
    pub limit: Option<u32>,
}

/// Bus event: a row was appended (the UI's live feed).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ActivityAppended {
    /// The row.
    pub entry: ActivityEntry,
}

impl wire::WireEvent for ActivityAppended {
    const TOPIC: &'static str = "tasks.activity";
}

/// Bus message any plugin publishes to append a row (see [`record`]).
#[derive(Clone, Debug)]
pub struct Record {
    /// What.
    pub kind: ActivityKind,
    /// Who.
    pub actor: String,
    /// Task / agent / session / routine id.
    pub subject: String,
    /// One line.
    pub summary: String,
    /// Spend, when known.
    pub cost: Option<f64>,
}

/// Publish one [`Record`] — the one-liner other plugins call.
pub async fn record(rt: &Runtime, kind: ActivityKind, actor: &str, subject: &str, summary: String) {
    rt.publish(Record {
        kind,
        actor: actor.to_owned(),
        subject: subject.to_owned(),
        summary,
        cost: None,
    })
    .await;
}

/// `ask` → `Result<Vec<ActivityEntry>, String>`, newest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListActivity(pub ActivityFilter);

/// Internal `tell`: one tail pass over `task_events`.
#[derive(Clone, Copy, Debug)]
struct Tail;

/// One `task_events` row with what the summary needs from its task.
struct Tailed {
    id: i64,
    task: String,
    title: String,
    parent: Option<String>,
    event: TaskEvent,
}

/// Map one task event to a feed row. `None` = not feed-worthy (progress
/// notes, placement, reviewer, title edits …).
pub fn from_task_event(
    title: &str,
    parent: Option<&str>,
    e: &TaskEvent,
) -> Option<(ActivityKind, String)> {
    let after = e.after.as_deref().unwrap_or("");
    let before = e.before.as_deref().unwrap_or("");
    let q = |s: &str| format!("\u{201c}{s}\u{201d}");
    Some(match e.field.as_str() {
        "created" if parent.is_some() => (
            ActivityKind::ChildSpawned,
            format!("spawned child {} ({after})", q(title)),
        ),
        "created" => (
            ActivityKind::TaskCreated,
            format!("created {} ({after})", q(title)),
        ),
        "state" if after == "done" && parent.is_some() => (
            ActivityKind::ChildReturned,
            format!("child {} returned", q(title)),
        ),
        "state" if after == "done" => (ActivityKind::TaskDone, format!("{} done", q(title))),
        "state" => (
            ActivityKind::TaskMoved,
            format!("{} {before} \u{2192} {after}", q(title)),
        ),
        "owner" => (
            ActivityKind::TaskReassigned,
            format!("{} reassigned {before} \u{2192} {after}", q(title)),
        ),
        "checkout" => (
            ActivityKind::TaskCheckedOut,
            format!("checked out {} ({after})", q(title)),
        ),
        "comment" => (
            ActivityKind::TaskCommented,
            format!("on {}: {after}", q(title)),
        ),
        "nudge" | "nudged" => (
            ActivityKind::TaskNudged,
            format!("nudged {}: {after}", q(title)),
        ),
        "blockers" => {
            let decisions = |s: &str| {
                s.split(", ")
                    .filter(|b| b.starts_with("decision:"))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let (b, a) = (decisions(before), decisions(after));
            if let Some(d) = a.iter().find(|d| !b.contains(d)) {
                (
                    ActivityKind::EscalationRaised,
                    format!("escalated {} ({d})", q(title)),
                )
            } else {
                let d = b.iter().find(|d| !a.contains(d))?;
                (
                    ActivityKind::EscalationAnswered,
                    format!("decided {} ({d})", q(title)),
                )
            }
        }
        _ => return None,
    })
}

/// The [`ACTIVITY_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Activity {
    pool: SqlitePool,
    rt: Runtime,
}

impl Activity {
    /// Spawn, register, subscribe, start the tail tick.
    pub async fn start(rt: &Runtime, store: &Store) -> anyhow::Result<ActorRef<Self>> {
        let actor = Self::spawn_with_mailbox(
            Self {
                pool: store.pool().clone(),
                rt: rt.clone(),
            },
            runtime::subscriber_mailbox(),
        );
        actor.register(ACTIVITY_ACTOR)?;
        rt.subscribe::<Self, Record>(&actor).await;
        rt.subscribe::<Self, ScheduleEvent>(&actor).await;
        let me = actor.clone();
        let rt2 = rt.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = rt2.shutdown.cancelled() => break,
                    () = tokio::time::sleep(TAIL_EVERY) => {}
                }
                if me.tell(Tail).await.is_err() {
                    break;
                }
            }
        });
        Ok(actor)
    }

    /// Append one row and publish it.
    async fn append(
        &self,
        ts_ms: u64,
        kind: ActivityKind,
        actor: &str,
        subject: &str,
        summary: &str,
        cost: Option<f64>,
    ) {
        match append(&self.pool, ts_ms, kind, actor, subject, summary, cost).await {
            Ok(entry) => self.rt.publish(ActivityAppended { entry }).await,
            Err(e) => tracing::warn!("activity not recorded: {e:#}"),
        }
    }
}

async fn append(
    pool: &SqlitePool,
    ts_ms: u64,
    kind: ActivityKind,
    actor: &str,
    subject: &str,
    summary: &str,
    cost: Option<f64>,
) -> anyhow::Result<ActivityEntry> {
    let id = sqlx::query("INSERT INTO activity (ts_ms, kind, actor, subject, summary, cost) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(ts_ms as i64)
        .bind(kind.as_str())
        .bind(actor)
        .bind(subject)
        .bind(summary)
        .bind(cost)
        .execute(pool)
        .await?
        .last_insert_rowid();
    Ok(ActivityEntry {
        id,
        ts_ms,
        kind,
        actor: actor.to_owned(),
        subject: subject.to_owned(),
        summary: summary.to_owned(),
        cost,
    })
}

/// New `task_events` rows past the cursor, with their task's title and parent.
async fn tail_rows(pool: &SqlitePool) -> anyhow::Result<Vec<Tailed>> {
    let last: Option<(i64,)> =
        sqlx::query_as("SELECT last_event FROM activity_cursor WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    // A store that predates the feed starts at its head: no backfill of history.
    let last = match last {
        Some((n,)) => n,
        None => {
            let (head,): (i64,) = sqlx::query_as("SELECT COALESCE(MAX(id), 0) FROM task_events")
                .fetch_one(pool)
                .await?;
            sqlx::query("INSERT INTO activity_cursor (id, last_event) VALUES (1, ?)")
                .bind(head)
                .execute(pool)
                .await?;
            head
        }
    };
    type R = (
        i64,
        String,
        i64,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<R> = sqlx::query_as(
        "SELECT e.id, e.task, e.at_ms, e.actor, e.session, e.field, e.before, e.after, t.title, t.parent \
         FROM task_events e LEFT JOIN tasks t ON t.id = e.task WHERE e.id > ? ORDER BY e.id LIMIT ?",
    )
    .bind(last)
    .bind(TAIL_BATCH)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, task, at_ms, actor, session, field, before, after, title, parent)| Tailed {
                id,
                title: title.unwrap_or_else(|| task.clone()),
                task,
                parent,
                event: TaskEvent {
                    at_ms: at_ms as u64,
                    actor,
                    session,
                    field,
                    before,
                    after,
                },
            },
        )
        .collect())
}

/// One tail pass: map, append, advance the cursor. Returns the appended rows.
pub async fn tail_once(pool: &SqlitePool) -> anyhow::Result<Vec<ActivityEntry>> {
    let rows = tail_rows(pool).await?;
    let Some(last) = rows.last().map(|r| r.id) else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for r in rows {
        if let Some((kind, summary)) = from_task_event(&r.title, r.parent.as_deref(), &r.event) {
            out.push(
                append(
                    pool,
                    r.event.at_ms,
                    kind,
                    &r.event.actor,
                    &r.task,
                    &summary,
                    None,
                )
                .await?,
            );
        }
    }
    sqlx::query("UPDATE activity_cursor SET last_event = ? WHERE id = 1")
        .bind(last)
        .execute(pool)
        .await?;
    Ok(out)
}

/// `activity_list`: newest first.
pub async fn list(pool: &SqlitePool, f: &ActivityFilter) -> anyhow::Result<Vec<ActivityEntry>> {
    let limit = f.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let kinds: Vec<String> = f.kinds.iter().flatten().map(|k| k.as_str()).collect();
    let kind_clause = if kinds.is_empty() {
        String::new()
    } else {
        format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(","))
    };
    let mut q = sqlx::query_as::<_, (i64, i64, String, String, String, String, Option<f64>)>(sqlx::AssertSqlSafe(format!(
        "SELECT id, ts_ms, kind, actor, subject, summary, cost FROM activity WHERE ts_ms > ?{kind_clause} ORDER BY id DESC LIMIT ?"
    )))
    .bind(f.since_ms.map_or(-1, |s| s as i64));
    for k in &kinds {
        q = q.bind(k);
    }
    let rows = q.bind(i64::from(limit)).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, ts_ms, kind, actor, subject, summary, cost)| {
            Some(ActivityEntry {
                id,
                ts_ms: ts_ms as u64,
                kind: ActivityKind::parse(&kind)?,
                actor,
                subject,
                summary,
                cost,
            })
        })
        .collect())
}

impl Message<Tail> for Activity {
    type Reply = ();

    async fn handle(&mut self, _: Tail, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match tail_once(&self.pool).await {
            Ok(entries) => {
                for entry in entries {
                    self.rt.publish(ActivityAppended { entry }).await;
                }
            }
            Err(e) => tracing::warn!("activity tail: {e:#}"),
        }
    }
}

impl Message<Record> for Activity {
    type Reply = ();

    async fn handle(&mut self, m: Record, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.append(now_ms(), m.kind, &m.actor, &m.subject, &m.summary, m.cost)
            .await;
    }
}

impl Message<ScheduleEvent> for Activity {
    type Reply = ();

    async fn handle(
        &mut self,
        m: ScheduleEvent,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let ScheduleEvent::Fired { name, session, at } = m {
            let ts = u64::try_from(at.timestamp_millis()).unwrap_or_else(|_| now_ms());
            self.append(
                ts,
                ActivityKind::RoutineFired,
                "system",
                &name,
                &format!("routine {name} fired (session {session})"),
                None,
            )
            .await;
        }
    }
}

impl Message<ListActivity> for Activity {
    type Reply = Result<Vec<ActivityEntry>, String>;

    async fn handle(&mut self, m: ListActivity, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        list(&self.pool, &m.0).await.map_err(|e| e.to_string())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The activity actor; `None` before the tasks plugin started.
pub fn actor() -> Option<ActorRef<Activity>> {
    ActorRef::<Activity>::lookup(ACTIVITY_ACTOR).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskId;

    fn ev(field: &str, before: Option<&str>, after: Option<&str>) -> TaskEvent {
        TaskEvent {
            at_ms: 1,
            actor: "agent:ops".into(),
            session: None,
            field: field.into(),
            before: before.map(Into::into),
            after: after.map(Into::into),
        }
    }

    #[test]
    fn task_events_map_to_kinds() {
        let k =
            |f, b, a, parent| from_task_event("fix deploy", parent, &ev(f, b, a)).map(|(k, _)| k);
        assert_eq!(
            k("created", None, Some("owner agent:ops"), None),
            Some(ActivityKind::TaskCreated)
        );
        assert_eq!(
            k("created", None, Some("owner agent:ops"), Some("p")),
            Some(ActivityKind::ChildSpawned)
        );
        assert_eq!(
            k("state", Some("todo"), Some("doing"), None),
            Some(ActivityKind::TaskMoved)
        );
        assert_eq!(
            k("state", Some("review"), Some("done"), None),
            Some(ActivityKind::TaskDone)
        );
        assert_eq!(
            k("state", Some("review"), Some("done"), Some("p")),
            Some(ActivityKind::ChildReturned)
        );
        assert_eq!(
            k("owner", Some("agent:ops"), Some("agent:dev"), None),
            Some(ActivityKind::TaskReassigned)
        );
        assert_eq!(
            k("checkout", None, Some("s1 on mac (epoch 1)"), None),
            Some(ActivityKind::TaskCheckedOut)
        );
        assert_eq!(
            k("comment", None, Some("looks good"), None),
            Some(ActivityKind::TaskCommented)
        );
        assert_eq!(
            k("blockers", Some(""), Some("decision:abc"), None),
            Some(ActivityKind::EscalationRaised)
        );
        assert_eq!(
            k("blockers", Some("x, decision:abc"), Some("x"), None),
            Some(ActivityKind::EscalationAnswered)
        );
        assert_eq!(k("blockers", Some("x"), Some("x, y"), None), None);
        assert_eq!(k("note", None, Some("progress"), None), None);
        assert_eq!(k("reviewer", None, Some("user"), None), None);
        let (_, s) = from_task_event(
            "fix deploy",
            None,
            &ev("state", Some("todo"), Some("doing")),
        )
        .unwrap();
        assert_eq!(s, "\u{201c}fix deploy\u{201d} todo \u{2192} doing");
    }

    #[tokio::test]
    async fn tail_appends_once_and_list_filters() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.sqlite")).await.unwrap();
        let pool = store.pool();
        // A pre-existing event is history, not feed.
        let id = TaskId::new();
        store
            .add_event(&id, &ev("created", None, Some("owner user")))
            .await
            .unwrap();
        assert!(tail_once(pool).await.unwrap().is_empty());
        // New events land, titled from the task row when it exists, else by id.
        store
            .add_event(&id, &ev("state", Some("todo"), Some("doing")))
            .await
            .unwrap();
        store
            .add_event(&id, &ev("note", None, Some("progress")))
            .await
            .unwrap();
        store
            .add_event(&id, &ev("state", Some("review"), Some("done")))
            .await
            .unwrap();
        let got = tail_once(pool).await.unwrap();
        assert_eq!(
            got.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [ActivityKind::TaskMoved, ActivityKind::TaskDone]
        );
        assert_eq!(got[0].subject, id.0);
        assert!(
            got[0].summary.contains(&id.0),
            "no task row: the id stands in for the title"
        );
        assert!(tail_once(pool).await.unwrap().is_empty(), "cursor advanced");
        append(
            pool,
            5,
            ActivityKind::AgentPaused,
            "user",
            "ops",
            "paused ops",
            None,
        )
        .await
        .unwrap();
        let all = list(pool, &ActivityFilter::default()).await.unwrap();
        assert_eq!(
            all.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [
                ActivityKind::AgentPaused,
                ActivityKind::TaskDone,
                ActivityKind::TaskMoved
            ]
        );
        let only = list(
            pool,
            &ActivityFilter {
                kinds: Some(vec![ActivityKind::TaskDone]),
                limit: Some(10),
                since_ms: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(only.len(), 1);
        let since = list(
            pool,
            &ActivityFilter {
                since_ms: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            since.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [ActivityKind::AgentPaused]
        );
        let one = list(
            pool,
            &ActivityFilter {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(one.len(), 1);
    }
}
