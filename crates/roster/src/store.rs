//! `<data_dir>/roster.sqlite`: `agents`, `episodes`, `proposals`, `budget`,
//! `budget_override` (67 §6).

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::types::{AgentFile, Episode};

/// Handle to `<data_dir>/roster.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

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

    /// Mirror one parsed file into `agents` (67 §5.1: a hand edit is a record write).
    pub async fn upsert_agent(&self, f: &AgentFile, mtime_ms: u64) -> anyhow::Result<()> {
        let mut fm = serde_json::to_value(f)?;
        if let Some(o) = fm.as_object_mut() {
            o.remove("body");
        }
        sqlx::query(
            "INSERT INTO agents (name, front_matter_json, body, mtime_ms) VALUES (?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET front_matter_json = excluded.front_matter_json, \
             body = excluded.body, mtime_ms = excluded.mtime_ms",
        )
        .bind(f.name.as_str())
        .bind(fm.to_string())
        .bind(&f.body)
        .bind(mtime_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The file is gone: drop its row.
    pub async fn delete_agent(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM agents WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Names with a row (the mirror as of the last reconcile).
    pub async fn agent_names(&self) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM agents ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }

    /// A desk opened (67 §6.2).
    pub async fn open_episode(&self, agent: &str, session: &str, now_ms: u64) -> anyhow::Result<Episode> {
        let ep = Episode {
            id: uuid::Uuid::now_v7().to_string(),
            agent: agent.to_owned(),
            session: session.to_owned(),
            opened_ms: now_ms,
            closed_ms: None,
            reason: None,
        };
        sqlx::query("INSERT INTO episodes (id, agent, session, opened_ms) VALUES (?, ?, ?, ?)")
            .bind(&ep.id)
            .bind(&ep.agent)
            .bind(&ep.session)
            .bind(ep.opened_ms as i64)
            .execute(&self.pool)
            .await?;
        Ok(ep)
    }

    /// A desk closed: `reason` is `idle | context | thread_end | lease | shutdown | …`.
    pub async fn close_episode(&self, id: &str, now_ms: u64, reason: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE episodes SET closed_ms = ?, reason = ? WHERE id = ?")
            .bind(now_ms as i64)
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Episodes still open (the desks of the previous run, at startup).
    pub async fn open_episodes(&self) -> anyhow::Result<Vec<Episode>> {
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT id, agent, session, opened_ms FROM episodes WHERE closed_ms IS NULL ORDER BY opened_ms",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, agent, session, opened_ms)| Episode {
                id,
                agent,
                session,
                opened_ms: opened_ms as u64,
                closed_ms: None,
                reason: None,
            })
            .collect())
    }

    /// A pending hire (67 §6.5).
    pub async fn insert_proposal(
        &self,
        name: &str,
        proposed_by: &str,
        approval_id: &str,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO proposals (name, proposed_by, approval_id, created_ms) VALUES (?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET proposed_by = excluded.proposed_by, \
             approval_id = excluded.approval_id, created_ms = excluded.created_ms",
        )
        .bind(name)
        .bind(proposed_by)
        .bind(approval_id)
        .bind(now_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Approved or withdrawn: the row's approval id when a row was there.
    pub async fn delete_proposal(&self, name: &str) -> anyhow::Result<Option<String>> {
        let r: Option<(String,)> = sqlx::query_as("DELETE FROM proposals WHERE name = ? RETURNING approval_id")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(|(id,)| id))
    }

    /// 67 §6.4: `delta` more spent by `agent` on this `engine` in `month`.
    pub async fn add_spent(
        &self,
        agent: &str,
        month: &str,
        engine: &str,
        delta: f64,
        currency: &str,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO budget (id, agent, month, engine, spent, currency, updated_ms) VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET spent = spent + excluded.spent, currency = excluded.currency, \
             updated_ms = excluded.updated_ms",
        )
        .bind(format!("{agent}|{month}|{engine}"))
        .bind(agent)
        .bind(month)
        .bind(engine)
        .bind(delta)
        .bind(currency)
        .bind(now_ms as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The month's spend across every engine.
    pub async fn spent_month(&self, agent: &str, month: &str) -> anyhow::Result<f64> {
        let (sum,): (f64,) = sqlx::query_as("SELECT COALESCE(SUM(spent), 0.0) FROM budget WHERE agent = ? AND month = ?")
            .bind(agent)
            .bind(month)
            .fetch_one(&self.pool)
            .await?;
        Ok(sum)
    }

    /// The manager un-paused an over-cap agent: no re-pause this month.
    pub async fn set_override(&self, agent: &str, month: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("INSERT OR IGNORE INTO budget_override (id, agent, month, created_ms) VALUES (?, ?, ?, ?)")
            .bind(format!("{agent}|{month}"))
            .bind(agent)
            .bind(month)
            .bind(now_ms as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether the month's cap was overridden for `agent`.
    pub async fn has_override(&self, agent: &str, month: &str) -> anyhow::Result<bool> {
        let r: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM budget_override WHERE id = ?")
            .bind(format!("{agent}|{month}"))
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.is_some())
    }
}
