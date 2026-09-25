//! `<data_dir>/tasks.sqlite`: `tasks`, `wakes`, `task_events`, `write_budget` (67 §7).
//! Plain row access; the rules live in [`crate::ledger`].

use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::types::{Checkout, Owner, Task, TaskEvent, TaskFilter, TaskId, TaskState, Wake, WakeReason};

/// Origin stamped into every `hlc`: the mesh engine id once known, else `local`.
static HLC_ORIGIN: OnceLock<String> = OnceLock::new();
/// Process-wide hybrid logical clock: `(wall_ms, counter)` of the last stamp.
static HLC: Mutex<(u64, u32)> = Mutex::new((0, 0));

/// Name the HLC origin (once; later calls are ignored).
pub fn set_hlc_origin(origin: &str) {
    let _ = HLC_ORIGIN.set(origin.to_owned());
}

/// Next HLC stamp as [`replica::Hlc::to_text`] (67 2.5: one format, so the
/// replica keeps the task's own stamp instead of re-stamping the row on capture).
pub fn hlc(now_ms: u64) -> String {
    let mut g = HLC.lock().unwrap_or_else(|e| e.into_inner());
    if now_ms > g.0 {
        *g = (now_ms, 0);
    } else {
        g.1 += 1;
    }
    replica::Hlc {
        wall_ms: g.0,
        counter: g.1,
        origin: replica::PeerId(HLC_ORIGIN.get().map_or("local", String::as_str).to_owned()),
    }
    .to_text()
}

/// Handle to `<data_dir>/tasks.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// One `tasks` row as sqlite hands it back.
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    title: String,
    brief: String,
    owner: String,
    state: String,
    reviewer: Option<String>,
    blockers_json: String,
    parent: Option<String>,
    project: Option<String>,
    where_: Option<String>,
    harness: Option<String>,
    external_json: String,
    thread: Option<String>,
    checkout_json: Option<String>,
    schedule_name: Option<String>,
    schedule_fire_ms: Option<i64>,
    created_by: String,
    created_ms: i64,
    updated_ms: i64,
    no_progress_count: i64,
    next_wake_after_ms: Option<i64>,
    done_when: Option<String>,
}

fn state_of(s: &str) -> TaskState {
    match s {
        "backlog" => TaskState::Backlog,
        "doing" => TaskState::Doing,
        "review" => TaskState::Review,
        "blocked" => TaskState::Blocked,
        "done" => TaskState::Done,
        "cancelled" => TaskState::Cancelled,
        _ => TaskState::Todo,
    }
}

fn reason_of(s: &str) -> WakeReason {
    match s {
        "comment" => WakeReason::Comment,
        "mention" => WakeReason::Mention,
        "schedule" => WakeReason::Schedule,
        "answer" => WakeReason::Answer,
        "review" => WakeReason::Review,
        "resume" => WakeReason::Resume,
        "manual" => WakeReason::Manual,
        "reassigned" => WakeReason::Reassigned,
        "blockers_resolved" => WakeReason::BlockersResolved,
        "children_completed" => WakeReason::ChildrenCompleted,
        _ => WakeReason::Assigned,
    }
}

impl From<Row> for Task {
    fn from(r: Row) -> Self {
        Self {
            id: TaskId(r.id),
            title: r.title,
            brief: r.brief,
            owner: r.owner.parse().unwrap_or(Owner::User),
            state: state_of(&r.state),
            reviewer: r.reviewer.and_then(|s| s.parse().ok()),
            blockers: serde_json::from_str(&r.blockers_json).unwrap_or_default(),
            blocks: Vec::new(),
            parent: r.parent.map(TaskId),
            project: r.project,
            where_: r.where_,
            harness: r.harness,
            external: serde_json::from_str(&r.external_json).unwrap_or_default(),
            thread: r.thread,
            checkout: r
                .checkout_json
                .and_then(|c| serde_json::from_str::<Checkout>(&c).ok()),
            schedule: r
                .schedule_name
                .zip(r.schedule_fire_ms)
                .map(|(n, f)| (n, f as u64)),
            created_by: r.created_by,
            created_ms: r.created_ms as u64,
            updated_ms: r.updated_ms as u64,
            no_progress_count: r.no_progress_count as u32,
            next_wake_after_ms: r.next_wake_after_ms.map(|v| v as u64),
            done_when: r.done_when,
        }
    }
}

const COLS: &str = "id, title, brief, owner, state, reviewer, blockers_json, parent, project, where_, \
                    harness, external_json, thread, checkout_json, schedule_name, schedule_fire_ms, \
                    created_by, created_ms, updated_ms, no_progress_count, next_wake_after_ms, done_when";

impl Store {
    /// Open or create the file and run the migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool })
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert or fully rewrite `t` (every column, `hlc` stamped now).
    pub async fn put(&self, t: &Task) -> anyhow::Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT OR REPLACE INTO tasks ({COLS}, hlc) VALUES \
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)"
        )))
        .bind(&t.id.0)
        .bind(&t.title)
        .bind(&t.brief)
        .bind(t.owner.to_string())
        .bind(t.state.as_str())
        .bind(t.reviewer.as_ref().map(ToString::to_string))
        .bind(serde_json::to_string(&t.blockers)?)
        .bind(t.parent.as_ref().map(|p| p.0.as_str()))
        .bind(&t.project)
        .bind(&t.where_)
        .bind(&t.harness)
        .bind(serde_json::to_string(&t.external)?)
        .bind(&t.thread)
        .bind(t.checkout.as_ref().map(serde_json::to_string).transpose()?)
        .bind(t.schedule.as_ref().map(|s| s.0.as_str()))
        .bind(t.schedule.as_ref().map(|s| s.1 as i64))
        .bind(&t.created_by)
        .bind(t.created_ms as i64)
        .bind(t.updated_ms as i64)
        .bind(t.no_progress_count as i64)
        .bind(t.next_wake_after_ms.map(|v| v as i64))
        .bind(&t.done_when)
        .bind(hlc(t.updated_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One task.
    pub async fn get(&self, id: &TaskId) -> anyhow::Result<Option<Task>> {
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM tasks WHERE id = ?")))
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(Task::from))
    }

    /// The task of a schedule fire, if one exists (67 §7.4 coalescing key).
    pub async fn by_schedule(&self, name: &str, fire_ms: u64) -> anyhow::Result<Option<Task>> {
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM tasks WHERE schedule_name = ? AND schedule_fire_ms = ?"
        )))
        .bind(name)
        .bind(fire_ms as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Task::from))
    }

    /// Tasks matching `filter`, project (NULL last) then oldest first.
    pub async fn list(&self, f: &TaskFilter) -> anyhow::Result<Vec<Task>> {
        let states = f
            .state
            .as_ref()
            .map(|s| serde_json::to_string(&s.iter().map(|s| s.as_str()).collect::<Vec<_>>()))
            .transpose()?;
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM tasks WHERE (?1 IS NULL OR owner = ?1) \
             AND (?2 IS NULL OR state IN (SELECT value FROM json_each(?2))) \
             AND (?3 IS NULL OR where_ = ?3) AND (?4 IS NULL OR project = ?4) \
             ORDER BY project IS NULL, project, created_ms"
        )))
        .bind(&f.owner)
        .bind(states)
        .bind(&f.engine)
        .bind(&f.project)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Task::from).collect())
    }

    /// Tasks whose blockers name `blocker`, any state (the inverse link), oldest first.
    // ponytail: LIKE over blockers_json; a blockers table if boards grow past thousands.
    pub async fn blocked_on(&self, blocker: &str) -> anyhow::Result<Vec<Task>> {
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM tasks WHERE blockers_json LIKE ? ORDER BY created_ms"
        )))
        .bind(format!("%{}%", serde_json::to_string(blocker)?))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(Task::from)
            .filter(|t| t.blockers.iter().any(|b| b == blocker))
            .collect())
    }

    /// Direct children of `parent`, any state, oldest first.
    pub async fn children(&self, parent: &TaskId) -> anyhow::Result<Vec<Task>> {
        let rows: Vec<Row> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM tasks WHERE parent = ? ORDER BY created_ms")))
                .bind(&parent.0)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(Task::from).collect())
    }

    /// Append one event.
    pub async fn add_event(&self, task: &TaskId, e: &TaskEvent) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO task_events (task, at_ms, actor, session, field, before, after) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task.0)
        .bind(e.at_ms as i64)
        .bind(&e.actor)
        .bind(&e.session)
        .bind(&e.field)
        .bind(&e.before)
        .bind(&e.after)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Events of `task`, oldest first.
    pub async fn events(&self, task: &TaskId) -> anyhow::Result<Vec<TaskEvent>> {
        type R = (i64, String, Option<String>, String, Option<String>, Option<String>);
        let rows: Vec<R> = sqlx::query_as(
            "SELECT at_ms, actor, session, field, before, after FROM task_events \
             WHERE task = ? ORDER BY id",
        )
        .bind(&task.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(at_ms, actor, session, field, before, after)| TaskEvent {
                at_ms: at_ms as u64,
                actor,
                session,
                field,
                before,
                after,
            })
            .collect())
    }

    /// The newest event of `task` on `field`.
    pub async fn last_event(&self, task: &TaskId, field: &str) -> anyhow::Result<Option<TaskEvent>> {
        Ok(self
            .events(task)
            .await?
            .into_iter()
            .rev()
            .find(|e| e.field == field))
    }

    /// Raise a wake: a new row, or `count + 1` on the row with the same key
    /// (67 §7.3 coalescing). Returns the row as it now is.
    pub async fn raise(
        &self,
        agent: &str,
        task: Option<&TaskId>,
        reason: WakeReason,
        now_ms: u64,
    ) -> anyhow::Result<Wake> {
        let key = Wake::key_of(agent, task, reason);
        sqlx::query(
            "INSERT INTO wakes (id, agent, task, reason, key, count, claimed_by, created_ms, hlc) \
             VALUES (?, ?, ?, ?, ?, 1, NULL, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET count = count + 1, hlc = excluded.hlc",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(agent)
        .bind(task.map(|t| t.0.as_str()))
        .bind(reason.as_str())
        .bind(&key)
        .bind(now_ms as i64)
        .bind(hlc(now_ms))
        .execute(&self.pool)
        .await?;
        let mut w = self.wakes_where("key = ?", &key).await?;
        w.pop().ok_or_else(|| anyhow::anyhow!("wake vanished"))
    }

    /// Wakes satisfying `cond` (one `?` bound to `arg`), oldest first.
    async fn wakes_where(&self, cond: &str, arg: &str) -> anyhow::Result<Vec<Wake>> {
        type R = (String, String, Option<String>, String, String, i64, Option<String>, i64);
        let rows: Vec<R> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, agent, task, reason, key, count, claimed_by, created_ms FROM wakes \
             WHERE {cond} ORDER BY created_ms, id"
        )))
        .bind(arg)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, agent, task, reason, key, count, claimed_by, created_ms)| Wake {
                id,
                agent,
                task: task.map(TaskId),
                reason: reason_of(&reason),
                key,
                count: count as u32,
                claimed_by,
                created_ms: created_ms as u64,
            })
            .collect())
    }

    /// Every wake, oldest first.
    pub async fn wakes(&self) -> anyhow::Result<Vec<Wake>> {
        // `1 = ?` with a TEXT bind is `1 = '1'`, which is false in sqlite (no column affinity).
        self.wakes_where("? = '1'", "1").await
    }

    /// Set (or clear) `claimed_by` of one wake.
    pub async fn claim(&self, id: &str, by: Option<&str>, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE wakes SET claimed_by = ?, hlc = ? WHERE id = ?")
            .bind(by)
            .bind(hlc(now_ms))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete the claimed wakes of `agent` about `task` (a wake completed).
    pub async fn finish_wakes(&self, agent: &str, task: &TaskId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM wakes WHERE agent = ? AND task = ? AND claimed_by IS NOT NULL")
            .bind(agent)
            .bind(&task.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete every wake claimed by `session` (its episode closed).
    pub async fn finish_claimed_by(&self, session: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM wakes WHERE claimed_by = ?")
            .bind(session)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Charge a write by `session` to a task it does not own against a budget of
    /// `cap` distinct tasks (67 §7.3). `Ok(true)` = allowed (a repeat write to a
    /// task already charged is free); `Ok(false)` = budget spent, nothing recorded.
    pub async fn budget_write(&self, session: &str, task: &TaskId, cap: u32) -> anyhow::Result<bool> {
        sqlx::query(
            "INSERT OR IGNORE INTO write_budget (session, task) SELECT ?1, ?2 \
             WHERE (SELECT COUNT(*) FROM write_budget WHERE session = ?1) < ?3",
        )
        .bind(session)
        .bind(&task.0)
        .bind(cap)
        .execute(&self.pool)
        .await?;
        let (n,): (u32,) = sqlx::query_as("SELECT COUNT(*) FROM write_budget WHERE session = ? AND task = ?")
            .bind(session)
            .bind(&task.0)
            .fetch_one(&self.pool)
            .await?;
        Ok(n > 0)
    }

    /// Mark chat message `(sender, timestamp)` as answered (67 §4.7).
    /// `Ok(true)` = first to mark it; `Ok(false)` = some engine already had.
    pub async fn mark_answered(&self, sender: &str, timestamp: u64, now_ms: u64) -> anyhow::Result<bool> {
        let r = sqlx::query("INSERT OR IGNORE INTO answered (id, sender, timestamp, hlc) VALUES (?, ?, ?, ?)")
            .bind(format!("{sender}:{timestamp}"))
            .bind(sender)
            .bind(timestamp as i64)
            .bind(hlc(now_ms))
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    /// 67 2.5: the stamp parses as a `replica::Hlc` (so the replica keeps it) and
    /// two stamps in one millisecond stay ordered by the counter.
    #[test]
    fn hlc_text_round_trips_through_replica() {
        let a = super::hlc(1_700_000_000_000);
        let b = super::hlc(1_700_000_000_000);
        let pa = replica::Hlc::parse(&a).expect("parseable");
        let pb = replica::Hlc::parse(&b).expect("parseable");
        assert_eq!(pa.to_text(), a);
        assert!(pa < pb, "{a} < {b}");
        assert_eq!(pa.origin, pb.origin);
    }
}
