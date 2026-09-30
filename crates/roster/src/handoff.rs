//! Desktop hand-off (79 §6.6, contract §2.7): `desktop_handoff` writes a
//! replicated `handoffs` row and polls it until a human pressed **Done** /
//! **Cancel** on any engine (`handoff_resolve`) or the wait ran out. The
//! attention inbox derives `handoff:<id>` items from the unresolved rows
//! ([`crate::attention`]); the rows live in `roster.sqlite` beside `agents`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::desk::Shared;

/// `sandbox.handoff_timeout_secs` when the section is absent (79 §4.7).
pub const DEFAULT_TIMEOUT_SECS: u64 = 1800;

/// How often the tool re-reads its row.
const POLL: Duration = Duration::from_secs(1);

/// One `handoffs` row (79 §6.6, contract §2.7): an agent in a VM asked a human
/// to take the desktop. Replicated; `resolved_ms` set by `handoff_resolve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Handoff {
    /// Row id (uuid).
    pub id: String,
    /// Engine the session runs on (a VM's engine id).
    pub engine: String,
    /// The asking session.
    pub session: String,
    /// Its agent, when it has one.
    pub agent: Option<String>,
    /// Why the human is needed (`log in to GitHub`).
    pub reason: String,
    /// A page to open first.
    pub url: Option<String>,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
    /// Set by `handoff_resolve`.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub resolved_ms: Option<u64>,
    /// `done` | `cancelled` | `timeout` once resolved.
    pub outcome: Option<String>,
    /// Replication clock.
    pub hlc: Option<String>,
}

type RowTuple = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
);

fn from_row(r: RowTuple) -> Handoff {
    Handoff {
        id: r.0,
        engine: r.1,
        session: r.2,
        agent: r.3,
        reason: r.4,
        url: r.5,
        created_ms: r.6 as u64,
        resolved_ms: r.7.map(|v| v as u64),
        outcome: r.8,
        hlc: r.9,
    }
}

/// Write a new row (`hlc` is stamped by the replica's trigger).
pub async fn insert(pool: &SqlitePool, h: &Handoff) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO handoffs (id, engine, session, agent, reason, url, created_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&h.id)
    .bind(&h.engine)
    .bind(&h.session)
    .bind(&h.agent)
    .bind(&h.reason)
    .bind(&h.url)
    .bind(h.created_ms as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// One row by id.
pub async fn get(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<Handoff>> {
    let r: Option<RowTuple> = sqlx::query_as(
        "SELECT id, engine, session, agent, reason, url, created_ms, resolved_ms, outcome, hlc \
         FROM handoffs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(r.map(from_row))
}

/// Every row (or only the unresolved ones), newest first.
pub async fn list(pool: &SqlitePool, unresolved_only: bool) -> anyhow::Result<Vec<Handoff>> {
    let sql = if unresolved_only {
        "SELECT id, engine, session, agent, reason, url, created_ms, resolved_ms, outcome, hlc \
         FROM handoffs WHERE resolved_ms IS NULL ORDER BY created_ms DESC"
    } else {
        "SELECT id, engine, session, agent, reason, url, created_ms, resolved_ms, outcome, hlc \
         FROM handoffs ORDER BY created_ms DESC"
    };
    let rows: Vec<RowTuple> = sqlx::query_as(sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(from_row).collect())
}

/// Set `resolved_ms` / `outcome` (`done` | `cancelled` | `timeout`) on an
/// unresolved row. `Ok(false)` when there is no such row or it was already
/// resolved (a second Done from another engine loses, LWW keeps the first).
pub async fn resolve(
    pool: &SqlitePool,
    id: &str,
    outcome: &str,
    now_ms: u64,
) -> anyhow::Result<bool> {
    let r = sqlx::query(
        "UPDATE handoffs SET resolved_ms = ?, outcome = ? WHERE id = ? AND resolved_ms IS NULL",
    )
    .bind(now_ms as i64)
    .bind(outcome)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// `desktop_handoff`: wait for a human at this desktop.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandoffArgs {
    /// Why a person is needed (`log in to GitHub`, `solve the CAPTCHA`); shown on the card.
    pub reason: String,
    /// A page the human should see first (you have usually navigated there already).
    #[serde(default)]
    pub url: Option<String>,
    /// How long to wait for Done; default `sandbox.handoff_timeout_secs` (1800).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// The `desktop_handoff` tool (79 §6.6): registered on every engine by the roster.
pub struct DesktopHandoffTool {
    /// `roster.sqlite`.
    pub pool: SqlitePool,
    /// Session → agent.
    pub shared: Arc<Shared>,
    /// Default wait.
    pub timeout: Duration,
}

#[async_trait::async_trait]
impl Tool for DesktopHandoffTool {
    const NAME: &'static str = "desktop_handoff";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = HandoffArgs;

    fn description(&self) -> String {
        "Hand the desktop to the user: passwords, passkeys, MFA, CAPTCHAs, payments, identity \
         checks — anything a person must do at the screen. Say in `reason` what you need; the \
         user gets an attention item that opens this desktop with Done / Cancel. Blocks until \
         they press one or `timeout_secs` passes; returns `{ outcome: done | cancelled | timeout, \
         elapsed_secs }`. After `done`, take a screenshot before continuing."
            .into()
    }

    async fn call(&self, a: HandoffArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if a.reason.trim().is_empty() {
            return Ok(ToolOutput::err("reason: say what the user has to do"));
        }
        let engine = match mesh::actor() {
            Some(m) => match m.ask(mesh::Status).await {
                Ok(s) => s.self_id.0,
                Err(_) => platform::process::hostname().to_owned(),
            },
            None => platform::process::hostname().to_owned(),
        };
        let row = Handoff {
            id: uuid::Uuid::now_v7().to_string(),
            engine,
            session: cx.session.to_string(),
            agent: self.shared.agent_of(cx.session),
            reason: a.reason,
            url: a.url,
            created_ms: now_ms(),
            resolved_ms: None,
            outcome: None,
            hlc: None,
        };
        insert(&self.pool, &row).await?;
        let timeout = a.timeout_secs.map_or(self.timeout, Duration::from_secs);
        let started = Instant::now();
        let outcome = loop {
            tokio::time::sleep(POLL).await;
            if let Some(h) = get(&self.pool, &row.id).await?
                && let Some(o) = h.outcome.filter(|_| h.resolved_ms.is_some())
            {
                break o;
            }
            if started.elapsed() >= timeout {
                resolve(&self.pool, &row.id, "timeout", now_ms()).await?;
                break "timeout".to_owned();
            }
        };
        let elapsed_secs = started.elapsed().as_secs();
        let text = format!("{outcome} after {elapsed_secs} s");
        let mut out = if outcome == "done" {
            ToolOutput::ok(text)
        } else {
            ToolOutput::err(text)
        };
        out.structured = Some(serde_json::json!({
            "outcome": outcome,
            "elapsed_secs": elapsed_secs,
        }));
        Ok(out)
    }
}

/// The `sandbox:` keys this crate reads, by name — no dependency on the sandbox crate.
#[derive(Deserialize)]
struct SandboxSection {
    #[serde(default = "default_timeout")]
    handoff_timeout_secs: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// `sandbox.handoff_timeout_secs`, or [`DEFAULT_TIMEOUT_SECS`] when the section is absent.
pub fn timeout(rt: &runtime::Runtime) -> Duration {
    let secs = rt
        .section::<SandboxSection>("sandbox")
        .map_or(DEFAULT_TIMEOUT_SECS, |s| s.handoff_timeout_secs);
    Duration::from_secs(secs)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn row(id: &str, created_ms: u64) -> Handoff {
        Handoff {
            id: id.into(),
            engine: "e1".into(),
            session: "s1".into(),
            agent: Some("ops".into()),
            reason: "log in to GitHub".into(),
            url: Some("https://github.com/login".into()),
            created_ms,
            resolved_ms: None,
            outcome: None,
            hlc: None,
        }
    }

    #[tokio::test]
    async fn insert_list_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.sqlite")).await.unwrap();
        let pool = store.pool();
        insert(pool, &row("a", 10)).await.unwrap();
        insert(pool, &row("b", 20)).await.unwrap();
        let ids: Vec<_> = list(pool, true)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(ids, ["b", "a"], "newest first");
        assert_eq!(get(pool, "a").await.unwrap().unwrap(), row("a", 10));

        assert!(resolve(pool, "a", "done", 30).await.unwrap());
        assert!(
            !resolve(pool, "a", "cancelled", 31).await.unwrap(),
            "already resolved: the first outcome sticks"
        );
        assert!(!resolve(pool, "zz", "done", 32).await.unwrap());
        let a = get(pool, "a").await.unwrap().unwrap();
        assert_eq!(
            (a.resolved_ms, a.outcome.as_deref()),
            (Some(30), Some("done"))
        );
        let open: Vec<_> = list(pool, true)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(open, ["b"]);
        assert_eq!(list(pool, false).await.unwrap().len(), 2);
    }
}
