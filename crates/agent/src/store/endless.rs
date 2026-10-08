//! 137: `sessions.memory`, the unit cache (`endless_units`), the tree
//! (`endless_nodes` + the FTS5 mirror `endless_lines_fts`) and the compactor
//! ledger (`endless_calls`). Row shapes live here; the pure logic in
//! `crate::endless`.

use chrono::{DateTime, Utc};
use genai::chat::ChatMessage;
use runtime::SessionId;
use sqlx::{FromRow, SqliteConnection};

use super::Store;
use crate::context::Mask;
use crate::endless::compactor::CallRow;
use crate::endless::tree::{Kind, Node};
use crate::endless::{EndlessStats, GuardTrips, MemoryMode};

/// One `endless_units` row (the unit without its text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitRow {
    pub i: u64,
    pub seq: i64,
    pub part: u16,
    pub kind: Kind,
    pub size: u32,
    /// Rendered-turn counter when the unit committed (D6 frozen-context key).
    pub turn: u32,
}

/// An endless session at engine start (`resume_all`).
#[derive(Clone, Debug, FromRow)]
pub struct EndlessSessionRow {
    pub id: String,
    pub model: String,
    pub effort: Option<String>,
    pub desk: Option<String>,
}

/// A `messages` row as the unit deriver needs it.
pub struct EndlessMessage {
    pub seq: i64,
    pub message: ChatMessage,
    /// The whole row is excluded (07 §0): no units.
    pub excluded: bool,
    pub at: Option<DateTime<Utc>>,
}

/// Delete every endless row of `sid` (session deletion).
pub(crate) async fn delete_endless_in(conn: &mut SqliteConnection, sid: &str) -> anyhow::Result<()> {
    for sql in [
        "DELETE FROM endless_calls WHERE session_id = ?",
        "DELETE FROM endless_nodes WHERE session_id = ?",
        "DELETE FROM endless_units WHERE session_id = ?",
    ] {
        sqlx::query(sql).bind(sid).execute(&mut *conn).await?;
    }
    Ok(())
}

/// A user query as an FTS5 MATCH expression: every word quoted, so `-`, `:`
/// and the like are literal; empty → `None`.
pub fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

impl Store {
    /// `sessions.memory` (D1); `epochs` for an unknown session.
    pub async fn session_memory(&self, id: SessionId) -> anyhow::Result<MemoryMode> {
        let s: Option<String> = sqlx::query_scalar("SELECT memory FROM sessions WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.read)
            .await?;
        Ok(s.and_then(|s| s.parse().ok()).unwrap_or_default())
    }

    /// Set `sessions.memory` (8.6, create_session).
    pub async fn set_session_memory(&self, id: SessionId, mode: MemoryMode) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET memory = ? WHERE id = ?")
            .bind(mode.as_str())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Every `memory = 'endless'` session this engine owns.
    pub async fn endless_sessions(&self) -> anyhow::Result<Vec<EndlessSessionRow>> {
        Ok(sqlx::query_as(
            "SELECT id, model, effort, desk FROM sessions \
             WHERE memory = 'endless' AND (owner = ? OR owner = '') ORDER BY updated_at DESC",
        )
        .bind(&*self.me)
        .fetch_all(&self.read)
        .await?)
    }

    /// The unit cache, by `i`.
    pub async fn endless_units(&self, id: SessionId) -> anyhow::Result<Vec<UnitRow>> {
        let rows: Vec<(i64, i64, i64, String, i64, i64)> = sqlx::query_as(
            "SELECT i, seq, part, kind, size, turn FROM endless_units WHERE session_id = ? ORDER BY i",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(i, seq, part, kind, size, turn)| {
                Ok(UnitRow {
                    i: i as u64,
                    seq,
                    part: part as u16,
                    kind: kind.parse().map_err(anyhow::Error::msg)?,
                    size: size as u32,
                    turn: turn as u32,
                })
            })
            .collect()
    }

    /// Append unit rows (idempotent on `i`).
    pub async fn endless_units_put(&self, id: SessionId, rows: &[UnitRow]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let sid = id.to_string();
        let mut tx = self.begin_write().await?;
        for r in rows {
            sqlx::query(
                "INSERT OR REPLACE INTO endless_units (session_id, i, seq, part, kind, size, turn) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&sid)
            .bind(r.i as i64)
            .bind(r.seq)
            .bind(i64::from(r.part))
            .bind(r.kind.as_str())
            .bind(i64::from(r.size))
            .bind(i64::from(r.turn))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Every built node of the session.
    pub async fn endless_nodes(&self, id: SessionId) -> anyhow::Result<Vec<Node>> {
        let rows: Vec<(i64, i64, String, i64, bool, Option<String>, i64)> = sqlx::query_as(
            "SELECT l, i, text, size, free, model, tries FROM endless_nodes WHERE session_id = ? ORDER BY l, i",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(l, i, text, size, free, model, tries)| Node {
                l: l as u8,
                i: i as u64,
                text,
                size: size as u32,
                free,
                model,
                tries: tries as i32,
            })
            .collect())
    }

    /// Store one built node (the FTS triggers mirror level 0). An upsert, not
    /// `INSERT OR REPLACE`: REPLACE's implicit delete skips the delete trigger
    /// and would leave the old line in the FTS index.
    pub async fn endless_node_put(&self, id: SessionId, node: &Node) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO endless_nodes (session_id, l, i, text, size, free, model, tries, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, l, i) DO UPDATE SET text = excluded.text, size = excluded.size, \
             free = excluded.free, model = excluded.model, tries = excluded.tries, created_at = excluded.created_at",
        )
        .bind(id.to_string())
        .bind(i64::from(node.l))
        .bind(node.i as i64)
        .bind(&node.text)
        .bind(i64::from(node.size))
        .bind(node.free)
        .bind(&node.model)
        .bind(i64::from(node.tries))
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// Record the attempts of one node build.
    pub async fn endless_calls_put(
        &self,
        id: SessionId,
        l: u8,
        i: u64,
        calls: &[CallRow],
    ) -> anyhow::Result<()> {
        if calls.is_empty() {
            return Ok(());
        }
        let sid = id.to_string();
        let now = Utc::now().to_rfc3339();
        let mut tx = self.begin_write().await?;
        for c in calls {
            sqlx::query(
                "INSERT INTO endless_calls (call_id, session_id, l, i, try, bytes, guard, usage_json, \
                 usd_estimate, latency_ms, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(&sid)
            .bind(i64::from(l))
            .bind(i as i64)
            .bind(i64::from(c.try_no))
            .bind(c.bytes.map(i64::from))
            .bind(c.guard)
            .bind(c.usage.as_ref().and_then(|u| serde_json::to_string(u).ok()))
            .bind(c.usd_estimate)
            .bind(c.latency_ms as i64)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Per-session compactor totals (§6).
    pub async fn endless_stats(&self, id: SessionId) -> anyhow::Result<EndlessStats> {
        let sid = id.to_string();
        let (calls, usd): (i64, Option<f64>) = sqlx::query_as(
            "SELECT COUNT(*), SUM(usd_estimate) FROM endless_calls WHERE session_id = ?",
        )
        .bind(&sid)
        .fetch_one(&self.read)
        .await?;
        let trips: Vec<(Option<String>, i64)> = sqlx::query_as(
            "SELECT guard, COUNT(*) FROM endless_calls WHERE session_id = ? AND guard IS NOT NULL GROUP BY guard",
        )
        .bind(&sid)
        .fetch_all(&self.read)
        .await?;
        let mut guard_trips = GuardTrips::default();
        for (g, n) in trips {
            let n = n as u32;
            match g.as_deref() {
                Some("copy") => guard_trips.copy = n,
                Some("grounding") => guard_trips.grounding = n,
                Some("status") => guard_trips.status = n,
                Some("format") => guard_trips.format = n,
                Some("empty") => guard_trips.empty = n,
                Some("size") => guard_trips.size = n,
                _ => {}
            }
        }
        let usages: Vec<String> =
            sqlx::query_scalar("SELECT usage_json FROM endless_calls WHERE session_id = ? AND usage_json IS NOT NULL")
                .bind(&sid)
                .fetch_all(&self.read)
                .await?;
        let mut stats = EndlessStats {
            calls: calls as u32,
            guard_trips,
            usd_estimate: usd.unwrap_or(0.0),
            ..Default::default()
        };
        for u in usages {
            let Ok(u) = serde_json::from_str::<genai::chat::Usage>(&u) else {
                continue;
            };
            let n = |v: Option<i32>| v.map_or(0, |v| u64::try_from(v).unwrap_or(0));
            let d = u.prompt_tokens_details.as_ref();
            stats.input_tokens += n(u.prompt_tokens);
            stats.output_tokens += n(u.completion_tokens);
            stats.cache_read_tokens += n(d.and_then(|d| d.cached_tokens));
            stats.cache_write_tokens += n(d.and_then(|d| d.cache_creation_tokens));
        }
        Ok(stats)
    }

    /// FTS5 over the level-0 lines: `(i, text)` best rank first.
    pub async fn endless_recall(
        &self,
        id: SessionId,
        query: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<(u64, String)>> {
        let Some(q) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT i, text FROM endless_lines_fts WHERE session_id = ? AND endless_lines_fts MATCH ? \
             ORDER BY rank, i DESC LIMIT ?",
        )
        .bind(id.to_string())
        .bind(q)
        .bind(i64::from(limit))
        .fetch_all(&self.read)
        .await?;
        Ok(rows.into_iter().map(|(i, t)| (i as u64, t)).collect())
    }

    /// Raw fallback (§9): unit indexes whose message row contains `needle`
    /// (case-insensitive LIKE on `messages.body`), newest first.
    pub async fn endless_units_like(
        &self,
        id: SessionId,
        needle: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<u64>> {
        let pat = format!(
            "%{}%",
            needle.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        );
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT u.i FROM endless_units u JOIN messages m ON m.session_id = u.session_id AND m.seq = u.seq \
             WHERE u.session_id = ? AND m.body LIKE ? ESCAPE '\\' ORDER BY u.i DESC LIMIT ?",
        )
        .bind(id.to_string())
        .bind(pat)
        .bind(i64::from(limit))
        .fetch_all(&self.read)
        .await?;
        Ok(rows.into_iter().map(|i| i as u64).collect())
    }

    /// `messages` rows from `seq` on (cut_at respected), for unit derivation.
    pub async fn endless_messages_from(
        &self,
        id: SessionId,
        seq: i64,
    ) -> anyhow::Result<Vec<EndlessMessage>> {
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT seq, body, excluded, at FROM messages WHERE session_id = ?1 AND seq >= ?2 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
             ORDER BY seq",
        )
        .bind(id.to_string())
        .bind(seq)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(seq, body, excluded, at)| {
                Ok(EndlessMessage {
                    seq,
                    message: serde_json::from_str(&body)?,
                    excluded: Mask::decode(excluded.as_deref())?.whole,
                    at: at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                })
            })
            .collect()
    }

    /// Rebuild the FTS mirror when it drifted from level 0 (a replica applied
    /// changesets without triggers).
    pub async fn endless_fts_sync(&self) -> anyhow::Result<()> {
        let (fts, l0): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM endless_lines_fts), (SELECT COUNT(*) FROM endless_nodes WHERE l = 0)",
        )
        .fetch_one(&self.read)
        .await?;
        if fts == l0 {
            return Ok(());
        }
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM endless_lines_fts")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO endless_lines_fts (text, session_id, i) SELECT text, session_id, i FROM endless_nodes WHERE l = 0",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endless::compactor::CallRow;

    async fn fresh() -> Store {
        let dir = std::env::temp_dir().join(format!("theuth-endless-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap()
    }

    /// Mode round-trips, units/nodes/calls persist, FTS follows level 0
    /// through insert / replace / delete, stats sum trips and cost.
    #[tokio::test]
    async fn endless_rows_round_trip_and_fts_follows_level0() {
        let store = fresh().await;
        let id = SessionId::new();
        store.create_session(id, "m", None, None, None).await.unwrap();
        assert_eq!(store.session_memory(id).await.unwrap(), MemoryMode::Epochs);
        store.set_session_memory(id, MemoryMode::Endless).await.unwrap();
        assert_eq!(store.session_memory(id).await.unwrap(), MemoryMode::Endless);
        assert_eq!(store.endless_sessions().await.unwrap().len(), 1);
        assert_eq!(store.session(id).await.unwrap().unwrap().memory, MemoryMode::Endless);

        store
            .endless_units_put(
                id,
                &[UnitRow {
                    i: 0,
                    seq: 0,
                    part: 0,
                    kind: Kind::User,
                    size: 10,
                    turn: 1,
                }],
            )
            .await
            .unwrap();
        assert_eq!(store.endless_units(id).await.unwrap()[0].kind, Kind::User);

        let node = |l: u8, i: u64, text: &str| Node {
            l,
            i,
            text: text.into(),
            size: text.len() as u32,
            free: false,
            model: Some("m".into()),
            tries: 1,
        };
        store.endless_node_put(id, &node(0, 0, "user: rename the config loader")).await.unwrap();
        store.endless_node_put(id, &node(0, 1, "echo: nginx reload failed")).await.unwrap();
        store.endless_node_put(id, &node(1, 0, "user: rename loader; echo: nginx failed")).await.unwrap();
        let hits = store.endless_recall(id, "nginx", 8).await.unwrap();
        assert_eq!(hits, vec![(1, "echo: nginx reload failed".to_owned())], "level 1 is not indexed");
        store.endless_node_put(id, &node(0, 1, "echo: apache reload failed")).await.unwrap();
        assert!(store.endless_recall(id, "nginx", 8).await.unwrap().is_empty(), "replace re-indexes");
        assert_eq!(store.endless_recall(id, "apache", 8).await.unwrap().len(), 1);
        assert!(store.endless_recall(id, "", 8).await.unwrap().is_empty());
        assert_eq!(store.endless_nodes(id).await.unwrap().len(), 3);

        store
            .endless_calls_put(
                id,
                0,
                0,
                &[
                    CallRow {
                        try_no: 0,
                        bytes: Some(600),
                        guard: Some("size"),
                        usage: serde_json::from_str(r#"{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":80}}"#).ok(),
                        usd_estimate: Some(0.001),
                        latency_ms: 10,
                    },
                    CallRow {
                        try_no: 1,
                        bytes: Some(400),
                        guard: None,
                        usage: None,
                        usd_estimate: Some(0.002),
                        latency_ms: 10,
                    },
                ],
            )
            .await
            .unwrap();
        let stats = store.endless_stats(id).await.unwrap();
        assert_eq!((stats.calls, stats.guard_trips.size), (2, 1));
        assert!((stats.usd_estimate - 0.003).abs() < 1e-9);
        assert_eq!((stats.input_tokens, stats.cache_read_tokens, stats.output_tokens), (100, 80, 20));

        store.endless_fts_sync().await.unwrap();
        assert_eq!(store.endless_recall(id, "apache", 8).await.unwrap().len(), 1);
        store.delete_session(id).await.unwrap();
        assert!(store.endless_nodes(id).await.unwrap().is_empty());
        assert!(store.endless_recall(id, "apache", 8).await.unwrap().is_empty(), "delete trigger");
    }
}
