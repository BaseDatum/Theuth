//! `<data_dir>/tasks.sqlite`: `tasks`, `wakes`, `task_events`, `write_budget`, `packets` (67 §7, 101).
//! Plain row access; the rules live in [`crate::ledger`].

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::packet::Packet;
use crate::types::{
    Checkout, Owner, Priority, Task, TaskEvent, TaskFilter, TaskId, TaskState, Wake, WakeReason,
};

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

/// Outcome of [`Store::budget_write`] (99 WRITE-CAP-LINE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteCharge {
    /// The write may proceed (the task is within the session's budget).
    pub allowed: bool,
    /// Distinct foreign tasks the session has charged within the window.
    pub used: u32,
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
    resolved_blockers_json: String,
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
    attachments_json: String,
    priority: String,
    home: Option<String>,
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
        "resumed" => WakeReason::Resumed,
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
            resolved_blockers: serde_json::from_str(&r.resolved_blockers_json).unwrap_or_default(),
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
            attachments: serde_json::from_str(&r.attachments_json).unwrap_or_default(),
            priority: Priority::parse(&r.priority).unwrap_or_default(),
            home: r.home,
        }
    }
}

const COLS: &str = "id, title, brief, owner, state, reviewer, blockers_json, parent, project, where_, \
                    harness, external_json, thread, checkout_json, schedule_name, schedule_fire_ms, \
                    created_by, created_ms, updated_ms, no_progress_count, next_wake_after_ms, done_when, \
                    attachments_json, priority, home, resolved_blockers_json";

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
        platform::startup::migrate::run("tasks", &sqlx::migrate!("./migrations"), &pool)
            .await
            .context("run migrations")?;
        Ok(Self { pool })
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert `t`, or update the row to `t` (`hlc` stamped now). 182 M2b: an
    /// UPSERT, not a replace — the capture logs only the columns that changed,
    /// so an unrelated write never re-announces a `home` or `checkout` this
    /// engine merely holds (possibly stale) under a fresh clock.
    pub async fn put(&self, t: &Task) -> anyhow::Result<()> {
        static SET: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            COLS.split(',')
                .map(str::trim)
                .filter(|c| *c != "id")
                .chain(["hlc"])
                .map(|c| format!("{c} = excluded.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        });
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO tasks ({COLS}, hlc) VALUES \
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27) \
             ON CONFLICT(id) DO UPDATE SET {}",
            *SET
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
        .bind(serde_json::to_string(&t.attachments)?)
        .bind(t.priority.as_str())
        .bind(&t.home)
        .bind(serde_json::to_string(&t.resolved_blockers)?)
        .bind(hlc(t.updated_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One task.
    pub async fn get(&self, id: &TaskId) -> anyhow::Result<Option<Task>> {
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM tasks WHERE id = ?"
        )))
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
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM tasks WHERE parent = ? ORDER BY created_ms"
        )))
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
        type R = (
            i64,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        );
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
    pub async fn last_event(
        &self,
        task: &TaskId,
        field: &str,
    ) -> anyhow::Result<Option<TaskEvent>> {
        Ok(self
            .events(task)
            .await?
            .into_iter()
            .rev()
            .find(|e| e.field == field))
    }

    /// Raise a wake: a new row, or `count + 1` on the row with the same key
    /// (67 §7.3 coalescing). A row already claimed by a desk is re-queued so
    /// the new raise is delivered again. Returns the row as it now is.
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
             ON CONFLICT(key) DO UPDATE SET count = count + 1, claimed_by = NULL, hlc = excluded.hlc",
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
        type R = (
            String,
            String,
            Option<String>,
            String,
            String,
            i64,
            Option<String>,
            i64,
        );
        let rows: Vec<R> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, agent, task, reason, key, count, claimed_by, created_ms FROM wakes \
             WHERE {cond} ORDER BY created_ms, id"
        )))
        .bind(arg)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, agent, task, reason, key, count, claimed_by, created_ms)| Wake {
                    id,
                    agent,
                    task: task.map(TaskId),
                    reason: reason_of(&reason),
                    key,
                    count: count as u32,
                    claimed_by,
                    created_ms: created_ms as u64,
                },
            )
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

    /// Delete one wake (its desk turn ended, or it was found stale).
    pub async fn finish_wake(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM wakes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// STALE-WAKE-DESKS: delete every wake on `task` whose reason has no
    /// purpose in `state` ([`WakeReason::applies_to`]); a closed task loses
    /// them all. Returns how many went.
    pub async fn drop_stale_wakes(&self, task: &TaskId, state: TaskState) -> anyhow::Result<u64> {
        let stale: Vec<&str> = WakeReason::ALL
            .iter()
            .filter(|r| !r.applies_to(state))
            .map(|r| r.as_str())
            .collect();
        if stale.is_empty() {
            return Ok(0);
        }
        let marks = vec!["?"; stale.len()].join(", ");
        let mut q = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM wakes WHERE task = ? AND reason IN ({marks})"
        )))
        .bind(&task.0);
        for r in stale {
            q = q.bind(r);
        }
        Ok(q.execute(&self.pool).await?.rows_affected())
    }

    /// Un-claim every wake claimed by `by` (the desk turn that took them is
    /// gone: an episode closed, a desk reopened, a delivery never returned).
    /// They are dispatched again and validated then.
    pub async fn release_claimed_by(&self, by: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE wakes SET claimed_by = NULL, hlc = ? WHERE claimed_by = ?")
            .bind(hlc(now_ms))
            .bind(by)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 182 D10: delete the claimed wakes on `task` — it moved, so they were
    /// delivered to the old home's desks; the move's `reassigned` wake is the
    /// new home's, and a claim left behind would hold its agent busy.
    /// Unclaimed wakes stay for the new home to deliver.
    pub async fn drop_claimed_wakes(&self, task: &TaskId) -> anyhow::Result<u64> {
        Ok(
            sqlx::query("DELETE FROM wakes WHERE task = ? AND claimed_by IS NOT NULL")
                .bind(&task.0)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Charge a write by `session` to a task outside its line against a budget
    /// of `cap` distinct tasks within `window_ms` (99 WRITE-CAP-LINE; `0` =
    /// the session's lifetime). A repeat write to a task already charged is
    /// free and does not refresh `at_ms`. Rows older than the window are
    /// pruned first; `used` is the session's count after the attempt.
    pub async fn budget_write(
        &self,
        session: &str,
        task: &TaskId,
        cap: u32,
        now_ms: u64,
        window_ms: u64,
    ) -> anyhow::Result<WriteCharge> {
        if window_ms > 0 {
            sqlx::query("DELETE FROM write_budget WHERE session = ?1 AND at_ms < ?2")
                .bind(session)
                .bind(now_ms.saturating_sub(window_ms) as i64)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO write_budget (session, task, at_ms) SELECT ?1, ?2, ?3 \
             WHERE (SELECT COUNT(*) FROM write_budget WHERE session = ?1) < ?4",
        )
        .bind(session)
        .bind(&task.0)
        .bind(now_ms as i64)
        .bind(cap)
        .execute(&self.pool)
        .await?;
        let (n,): (u32,) =
            sqlx::query_as("SELECT COUNT(*) FROM write_budget WHERE session = ? AND task = ?")
                .bind(session)
                .bind(&task.0)
                .fetch_one(&self.pool)
                .await?;
        let (used,): (u32,) = sqlx::query_as("SELECT COUNT(*) FROM write_budget WHERE session = ?")
            .bind(session)
            .fetch_one(&self.pool)
            .await?;
        Ok(WriteCharge {
            allowed: n > 0,
            used,
        })
    }

    /// How many distinct foreign tasks `session` has charged within
    /// `window_ms` (`0` = ever). Reads only.
    pub async fn budget_used(
        &self,
        session: &str,
        now_ms: u64,
        window_ms: u64,
    ) -> anyhow::Result<u32> {
        let cutoff = if window_ms > 0 {
            now_ms.saturating_sub(window_ms) as i64
        } else {
            i64::MIN
        };
        let (n,): (u32,) =
            sqlx::query_as("SELECT COUNT(*) FROM write_budget WHERE session = ?1 AND at_ms >= ?2")
                .bind(session)
                .bind(cutoff)
                .fetch_one(&self.pool)
                .await?;
        Ok(n)
    }

    /// Mark chat message `(sender, timestamp)` as answered (67 §4.7).
    /// `Ok(true)` = first to mark it; `Ok(false)` = some engine already had.
    pub async fn mark_answered(
        &self,
        sender: &str,
        timestamp: u64,
        now_ms: u64,
    ) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "INSERT OR IGNORE INTO answered (id, sender, timestamp, hlc) VALUES (?, ?, ?, ?)",
        )
        .bind(format!("{sender}:{timestamp}"))
        .bind(sender)
        .bind(timestamp as i64)
        .bind(hlc(now_ms))
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// Insert or replace one packet (101 §3; `hlc` stamped from `at_ms`).
    pub async fn put_packet(&self, p: &Packet) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO packets (key, task, kind, json, at_ms, hlc) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.key)
        .bind(&p.task.0)
        .bind(p.kind.as_str())
        .bind(serde_json::to_string(p)?)
        .bind(p.at_ms as i64)
        .bind(hlc(p.at_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One packet by key.
    pub async fn packet(&self, key: &str) -> anyhow::Result<Option<Packet>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT json FROM packets WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|(j,)| serde_json::from_str(&j).map_err(Into::into))
            .transpose()
    }

    /// Every packet of `tasks`, newest first. Rows that no longer parse are skipped.
    pub async fn packets_for(&self, tasks: &[TaskId]) -> anyhow::Result<Vec<Packet>> {
        let ids = serde_json::to_string(&tasks.iter().map(|t| t.0.as_str()).collect::<Vec<_>>())?;
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT json FROM packets WHERE task IN (SELECT value FROM json_each(?)) \
             ORDER BY at_ms DESC",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(j,)| serde_json::from_str(&j).ok())
            .collect())
    }

    /// 182 D7: ids of tasks with a local `created` event (task_events never
    /// replicates: the task was created on this engine) at or after `since_ms`.
    pub async fn created_here(&self, since_ms: u64) -> anyhow::Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT task FROM task_events WHERE field = 'created' AND at_ms >= ?",
        )
        .bind(since_ms as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    /// 182 D7: `engine` ran its home backfill already.
    pub async fn backfilled(&self, engine: &str) -> anyhow::Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT at_ms FROM home_backfill WHERE engine = ?")
                .bind(engine)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    /// 182 D7: record that `engine` ran its home backfill.
    pub async fn mark_backfilled(&self, engine: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("INSERT OR IGNORE INTO home_backfill (engine, at_ms) VALUES (?, ?)")
            .bind(engine)
            .bind(now_ms as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
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

    /// 99 WRITE-CAP-LINE: migration 0006 gives `write_budget` an `at_ms`, and
    /// `budget_used` counts a row inside the window but not outside it.
    #[tokio::test]
    async fn write_budget_at_ms_is_windowed() {
        let dir = tempfile::tempdir().unwrap();
        let s = super::Store::open(&dir.path().join("tasks.sqlite"))
            .await
            .unwrap();
        sqlx::query("INSERT INTO write_budget (session, task, at_ms) VALUES ('s', 't', 1000)")
            .execute(&s.pool)
            .await
            .unwrap();
        assert_eq!(s.budget_used("s", 1500, 1000).await.unwrap(), 1);
        assert_eq!(s.budget_used("s", 2001, 1000).await.unwrap(), 0);
        assert_eq!(
            s.budget_used("s", 2001, 0).await.unwrap(),
            1,
            "0 = lifetime"
        );
        let c = s
            .budget_write("s", &super::TaskId("u".into()), 1, 2001, 1000)
            .await
            .unwrap();
        assert_eq!(
            c,
            super::WriteCharge {
                allowed: true,
                used: 1
            },
            "old row pruned"
        );
    }
}
