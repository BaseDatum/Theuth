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
use crate::endless::{EndlessStats, GuardTrips, MemoryMode, TailOverride, UnitFilter};

/// The session's endless columns (R1-D4 base, R1-D6 filter, 137G pause and models).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EndlessMeta {
    /// `sessions.memory`.
    pub mode: MemoryMode,
    /// `sessions.endless_filter`.
    pub filter: UnitFilter,
    /// `sessions.endless_base`: first unit the tree covers.
    pub base: u64,
    /// `sessions.endless_view_bytes`: the user's per-session view budget (R1b).
    pub view_bytes: Option<u32>,
    /// `sessions.endless_paused` (137G): `user` | `spend`; `None` = running.
    pub paused_by: Option<String>,
    /// `sessions.endless_spend_override` (137G G11): the UTC day the user resumed past the cap.
    pub spend_override: Option<String>,
    /// `sessions.endless_model` (137G): this chat's own compactor pick.
    pub model_override: Option<String>,
    /// `sessions.endless_agent_model` (137G): the desk agent's `memory_model`.
    pub agent_model: Option<String>,
    /// `sessions.endless_effort` (194): this chat's own compactor effort.
    pub effort_override: Option<String>,
    /// `sessions.endless_agent_effort` (194): the desk agent's `memory_effort`.
    pub agent_effort: Option<String>,
    /// `sessions.endless_tail_mode/low/high` (201 §4.2): the user's
    /// per-session tail override; `None` = inherit.
    pub tail: Option<TailOverride>,
}

/// 201 §4.3 (P2): one `endless_generations` row — a generation's frozen view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationRow {
    /// Generation number (1 = the session's first).
    pub n: u32,
    /// First unit the view does not cover (the tail's first unit).
    pub t_cut: u64,
    /// First `messages.seq` still in the tail.
    pub first_seq: i64,
    /// The rendered view blocks, `(text, mark)`.
    pub blocks: Vec<(String, bool)>,
    /// 201 §9 (P6a): the ledger snapshot frozen with this generation
    /// (`None`: the ledger was off at its trim).
    pub ledger: Option<String>,
    /// 201 R1-2: when the generation started (the cold rule after a
    /// restart); `None` on write = now.
    pub created_at: Option<DateTime<Utc>>,
}

/// One `endless_units` row (the unit without its text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitRow {
    /// Unit index (0-based, never renumbered).
    pub i: u64,
    /// The `messages.seq` of the row.
    pub seq: i64,
    /// Part index within the row.
    pub part: u16,
    /// Unit kind.
    pub kind: Kind,
    /// UTF-8 bytes of `kind: text`.
    pub size: u32,
    /// Rendered-turn counter when the unit committed (D6 frozen-context key).
    pub turn: u32,
}

/// An endless session at engine start (`resume_all`).
#[derive(Clone, Debug, FromRow)]
pub struct EndlessSessionRow {
    /// Session id as text.
    pub id: String,
    /// `sessions.model`.
    pub model: String,
    /// `sessions.effort`.
    pub effort: Option<String>,
    /// `sessions.desk` (the agent name for a desk).
    pub desk: Option<String>,
}

/// A `messages` row as the unit deriver needs it.
pub struct EndlessMessage {
    /// The `messages.seq` of the row.
    pub seq: i64,
    /// The decoded `ChatMessage`.
    pub message: ChatMessage,
    /// The whole row is excluded (07 §0): no units.
    pub excluded: bool,
    /// `messages.at`, when recorded.
    pub at: Option<DateTime<Utc>>,
    /// R3-9: the row carries typed vision-note metadata (`messages.vision`,
    /// 52 §6) — the only mark of a real `<vision_observation>` note.
    pub vision: bool,
}

/// Delete every endless row of `sid` (session deletion).
pub(crate) async fn delete_endless_in(
    conn: &mut SqliteConnection,
    sid: &str,
) -> anyhow::Result<()> {
    for sql in [
        "DELETE FROM endless_calls WHERE session_id = ?",
        "DELETE FROM endless_nodes WHERE session_id = ?",
        "DELETE FROM endless_units WHERE session_id = ?",
        // 201 R1-1: the frozen views and the state ledger hold the
        // conversation's substance; they go with the session.
        "DELETE FROM endless_generations WHERE session_id = ?",
        "DELETE FROM endless_ledger WHERE session_id = ?",
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

    /// The session's endless columns; defaults for an unknown session.
    pub async fn endless_meta(&self, id: SessionId) -> anyhow::Result<EndlessMeta> {
        type Row = (
            String,
            String,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<f64>,
            Option<f64>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT memory, endless_filter, endless_base, endless_view_bytes, endless_paused, \
             endless_spend_override, endless_model, endless_agent_model, endless_effort, \
             endless_agent_effort, endless_tail_mode, endless_tail_low, endless_tail_high \
             FROM sessions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(row
            .map(
                |(m, f, b, v, p, o, cm, am, ce, ae, tm, tl, th)| EndlessMeta {
                    mode: m.parse().unwrap_or_default(),
                    filter: f.parse().unwrap_or_default(),
                    base: u64::try_from(b).unwrap_or(0),
                    view_bytes: v.and_then(|v| u32::try_from(v).ok()),
                    paused_by: p.filter(|p| !p.is_empty()),
                    spend_override: o,
                    model_override: cm.filter(|m| !m.is_empty()),
                    agent_model: am.filter(|m| !m.is_empty()),
                    effort_override: ce.filter(|e| !e.is_empty()),
                    agent_effort: ae.filter(|e| !e.is_empty()),
                    tail: match (tm.and_then(|m| m.parse().ok()), tl, th) {
                        (Some(mode), Some(low), Some(high)) => {
                            Some(TailOverride { mode, low, high })
                        }
                        _ => None,
                    },
                },
            )
            .unwrap_or_default())
    }

    /// 137G `endless_pause`: who paused (`user` | `spend`), `None` = running;
    /// `spend_override` = the UTC day the user resumed past the day cap.
    pub async fn set_endless_paused(
        &self,
        id: SessionId,
        paused_by: Option<&str>,
        spend_override: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE sessions SET endless_paused = ?, \
             endless_spend_override = COALESCE(?, endless_spend_override) WHERE id = ?",
        )
        .bind(paused_by)
        .bind(spend_override)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// 137G: this chat's compactor model (`None` clears) or, with `agent`,
    /// the desk agent's `memory_model` — with its effort (194), one write.
    pub async fn set_endless_model(
        &self,
        id: SessionId,
        model: Option<&str>,
        effort: Option<&str>,
        agent: bool,
    ) -> anyhow::Result<()> {
        let sql = if agent {
            "UPDATE sessions SET endless_agent_model = ?, endless_agent_effort = ? WHERE id = ?"
        } else {
            "UPDATE sessions SET endless_model = ?, endless_effort = ? WHERE id = ?"
        };
        let mut tx = self.begin_write().await?;
        sqlx::query(sql)
            .bind(model)
            .bind(effort)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// 194: this chat's compactor effort (`None` clears) or, with `agent`,
    /// the desk agent's `memory_effort`.
    pub async fn set_endless_effort(
        &self,
        id: SessionId,
        effort: Option<&str>,
        agent: bool,
    ) -> anyhow::Result<()> {
        let sql = if agent {
            "UPDATE sessions SET endless_agent_effort = ? WHERE id = ?"
        } else {
            "UPDATE sessions SET endless_effort = ? WHERE id = ?"
        };
        let mut tx = self.begin_write().await?;
        sqlx::query(sql)
            .bind(effort)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// 137G G11: USD this session's compactor calls (`model_calls.kind =
    /// 'endless'`) cost at or after `since` (RFC 3339): the returned cost
    /// else the estimate, USD rows only.
    pub async fn endless_spend_since(&self, id: SessionId, since: &str) -> anyhow::Result<f64> {
        let nanos: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(CASE \
                WHEN returned_nanos IS NOT NULL THEN IIF(returned_currency = 'USD', returned_nanos, 0) \
                WHEN estimated_currency = 'USD' THEN estimated_nanos ELSE 0 END), 0) \
             FROM model_calls WHERE owner_id = ? AND kind = ? AND created_at >= ?",
        )
        .bind(id.to_string())
        .bind(crate::cost::CallKind::Endless.as_str())
        .bind(since)
        .fetch_one(&self.read)
        .await?;
        Ok(nanos as f64 / 1e9)
    }

    /// Fix the filter and base at switch-on (R1-D4, R1-D6).
    pub async fn set_endless_meta(
        &self,
        id: SessionId,
        filter: UnitFilter,
        base: u64,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET endless_filter = ?, endless_base = ? WHERE id = ?")
            .bind(filter.as_str())
            .bind(i64::try_from(base).unwrap_or(i64::MAX))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// R1b: the per-session view budget; `None` clears it.
    pub async fn set_endless_view_bytes(
        &self,
        id: SessionId,
        bytes: Option<u32>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE sessions SET endless_view_bytes = ? WHERE id = ?")
            .bind(bytes.map(i64::from))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// 201 §4.2: the per-session tail override (mode and both marks); `None` clears it.
    pub async fn set_endless_tail(
        &self,
        id: SessionId,
        tail: Option<TailOverride>,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE sessions SET endless_tail_mode = ?, endless_tail_low = ?, \
             endless_tail_high = ? WHERE id = ?",
        )
        .bind(tail.map(|t| t.mode.as_str()))
        .bind(tail.map(|t| t.low))
        .bind(tail.map(|t| t.high))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// 201 §4.3 (P2): the session's newest generation row, if any.
    pub async fn endless_generation(&self, id: SessionId) -> anyhow::Result<Option<GenerationRow>> {
        let mut conn = self.read.acquire().await?;
        let row: Option<(i64, i64, i64, String, Option<String>, String)> = sqlx::query_as(
            "SELECT n, t_cut, first_seq, blocks, ledger, created_at FROM endless_generations \
             WHERE session_id = ? ORDER BY n DESC LIMIT 1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *conn)
        .await?;
        let Some((n, t_cut, first_seq, blocks, ledger, created_at)) = row else {
            return Ok(None);
        };
        let blocks: Vec<(String, bool)> = serde_json::from_str(&blocks)?;
        Ok(Some(GenerationRow {
            n: n as u32,
            t_cut: t_cut as u64,
            first_seq,
            blocks,
            ledger,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .ok()
                .map(|t| t.with_timezone(&Utc)),
        }))
    }

    /// 201 §9 (P6a): the session's ledger body (JSON), `None` before one.
    pub async fn endless_ledger(&self, id: SessionId) -> anyhow::Result<Option<String>> {
        let mut conn = self.read.acquire().await?;
        Ok(
            sqlx::query_scalar("SELECT body FROM endless_ledger WHERE session_id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *conn)
                .await?,
        )
    }

    /// 201 §9 (P6a): store (or replace) the session's ledger body.
    pub async fn endless_ledger_put(&self, id: SessionId, body: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO endless_ledger (session_id, body, updated_at) VALUES (?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(body)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// 201 §4.3 (P2): store (or replace) one generation's frozen view.
    pub async fn endless_generation_put(
        &self,
        id: SessionId,
        g: &GenerationRow,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO endless_generations \
             (session_id, n, t_cut, first_seq, blocks, created_at, ledger) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(i64::from(g.n))
        .bind(g.t_cut as i64)
        .bind(g.first_seq)
        .bind(serde_json::to_string(&g.blocks)?)
        .bind(g.created_at.unwrap_or_else(Utc::now).to_rfc3339())
        .bind(g.ledger.as_deref())
        .execute(&mut *tx)
        .await?;
        // 201 R1-8: only the newest generation is ever read; older frozen
        // views (up to 256 KB each) go in the same transaction.
        sqlx::query("DELETE FROM endless_generations WHERE session_id = ? AND n < ?")
            .bind(id.to_string())
            .bind(i64::from(g.n))
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// 201 (P2): forget the session's generations (`/endless on` starts at 1 again).
    pub async fn endless_generations_clear(&self, id: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM endless_generations WHERE session_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
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

    /// Drop the unit cache (a first switch-on with a different filter re-derives it).
    pub async fn endless_units_clear(&self, id: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM endless_units WHERE session_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
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

    /// Every built node of the session. 201 P1: `quality` is the truth for
    /// the node's flags ([`Node::set_quality`]).
    pub async fn endless_nodes(&self, id: SessionId) -> anyhow::Result<Vec<Node>> {
        type Row = (
            i64,
            i64,
            String,
            i64,
            bool,
            Option<String>,
            i64,
            bool,
            String,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT l, i, text, size, free, model, tries, mechanical, quality FROM endless_nodes \
             WHERE session_id = ? ORDER BY l, i",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(l, i, text, size, free, model, tries, mechanical, quality)| {
                    let mut node = Node {
                        l: l as u8,
                        i: i as u64,
                        text,
                        size: size as u32,
                        free,
                        model,
                        tries: tries as i32,
                        mechanical,
                    };
                    node.set_quality(crate::endless::tree::Quality::parse(&quality));
                    node
                },
            )
            .collect())
    }

    /// Store one built node (the FTS triggers mirror level 0).
    pub async fn endless_node_put(&self, id: SessionId, node: &Node) -> anyhow::Result<()> {
        self.endless_nodes_put(id, std::slice::from_ref(node)).await
    }

    /// Store built nodes in one transaction. An upsert, not `INSERT OR
    /// REPLACE`: REPLACE's implicit delete skips the delete trigger and would
    /// leave the old line in the FTS index.
    pub async fn endless_nodes_put(&self, id: SessionId, nodes: &[Node]) -> anyhow::Result<()> {
        self.nodes_put(id, nodes, true).await
    }

    /// 137G R1-8: store nodes only where none is built yet (`endless_fold`'s
    /// excerpts never overwrite a model line that landed meanwhile).
    pub async fn endless_nodes_put_new(&self, id: SessionId, nodes: &[Node]) -> anyhow::Result<()> {
        self.nodes_put(id, nodes, false).await
    }

    async fn nodes_put(&self, id: SessionId, nodes: &[Node], replace: bool) -> anyhow::Result<()> {
        let sid = id.to_string();
        let now = Utc::now().to_rfc3339();
        let sql = if replace {
            "INSERT INTO endless_nodes (session_id, l, i, text, size, free, model, tries, mechanical, quality, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, l, i) DO UPDATE SET text = excluded.text, size = excluded.size, \
             free = excluded.free, model = excluded.model, tries = excluded.tries, \
             mechanical = excluded.mechanical, quality = excluded.quality, created_at = excluded.created_at"
        } else {
            "INSERT INTO endless_nodes (session_id, l, i, text, size, free, model, tries, mechanical, quality, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(session_id, l, i) DO NOTHING"
        };
        let mut tx = self.begin_write().await?;
        for node in nodes {
            sqlx::query(sql)
                .bind(&sid)
                .bind(i64::from(node.l))
                .bind(node.i as i64)
                .bind(&node.text)
                .bind(i64::from(node.size))
                .bind(node.free)
                .bind(&node.model)
                .bind(i64::from(node.tries))
                .bind(node.mechanical)
                .bind(node.quality().as_str())
                .bind(&now)
                .execute(&mut *tx)
                .await?;
        }
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
        let usages: Vec<String> = sqlx::query_scalar(
            "SELECT usage_json FROM endless_calls WHERE session_id = ? AND usage_json IS NOT NULL",
        )
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
        self.endless_fts(id, &q, limit).await
    }

    /// 201 §10 (P6b): [`Store::endless_recall`] matching ANY of `words`
    /// (the prefetch over a new input's rare words), best rank first.
    pub async fn endless_recall_any(
        &self,
        id: SessionId,
        words: &[String],
        limit: u32,
    ) -> anyhow::Result<Vec<(u64, String)>> {
        let q: Vec<String> = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        self.endless_fts(id, &q.join(" OR "), limit).await
    }

    async fn endless_fts(
        &self,
        id: SessionId,
        q: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<(u64, String)>> {
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
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
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

    /// 201 §8 (P4): the one `messages` row `seq` (an artefact unit's binary).
    pub async fn endless_message(
        &self,
        id: SessionId,
        seq: i64,
    ) -> anyhow::Result<Option<ChatMessage>> {
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM messages WHERE session_id = ?1 AND seq = ?2")
                .bind(id.to_string())
                .bind(seq)
                .fetch_optional(&self.read)
                .await?;
        Ok(match body {
            Some(b) => Some(serde_json::from_str(&b)?),
            None => None,
        })
    }

    /// `messages` rows from `seq` on (cut_at respected), for unit derivation.
    pub async fn endless_messages_from(
        &self,
        id: SessionId,
        seq: i64,
    ) -> anyhow::Result<Vec<EndlessMessage>> {
        type Row = (i64, String, Option<String>, Option<String>, bool);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT seq, body, excluded, at, vision IS NOT NULL FROM messages WHERE session_id = ?1 AND seq >= ?2 \
             AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?1), 9223372036854775807) \
             ORDER BY seq",
        )
        .bind(id.to_string())
        .bind(seq)
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(seq, body, excluded, at, vision)| {
                Ok(EndlessMessage {
                    seq,
                    message: serde_json::from_str(&body)?,
                    excluded: Mask::decode(excluded.as_deref())?.whole,
                    at: at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                    vision,
                })
            })
            .collect()
    }

    /// 201 R1-3b: the seqs of `messages` rows removed whole from the context
    /// (the pump replaces their units' lines).
    pub async fn endless_excluded_seqs(&self, id: SessionId) -> anyhow::Result<Vec<i64>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seq, excluded FROM messages WHERE session_id = ? AND excluded IS NOT NULL",
        )
        .bind(id.to_string())
        .fetch_all(&self.read)
        .await?;
        let mut out = Vec::new();
        for (seq, mask) in rows {
            if Mask::decode(Some(&mask))?.whole {
                out.push(seq);
            }
        }
        Ok(out)
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

    async fn fresh() -> (Store, crate::TestDir) {
        let scratch = crate::TestDir::new("endless");
        let store = Store::open(&scratch.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, scratch)
    }

    /// 137G R1-8: `endless_fold`'s insert-only put never overwrites a model
    /// line that landed first; the FTS mirror keeps that line.
    #[tokio::test]
    async fn fold_put_keeps_a_model_line_that_landed_first() {
        let (store, _dir) = fresh().await;
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let node = |text: &str, mechanical: bool| Node {
            l: 0,
            i: 0,
            text: text.into(),
            size: text.len() as u32,
            free: false,
            model: (!mechanical).then(|| "m".to_owned()),
            tries: i32::from(!mechanical),
            mechanical,
        };
        store
            .endless_node_put(id, &node("user: the model line about zebras", false))
            .await
            .unwrap();
        store
            .endless_nodes_put_new(id, &[node("user: excerpt …", true)])
            .await
            .unwrap();
        let nodes = store.endless_nodes(id).await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(!nodes[0].mechanical && nodes[0].text.contains("zebras"));
        assert_eq!(
            store.endless_recall(id, "zebras", 8).await.unwrap().len(),
            1
        );
    }

    /// Mode round-trips, units/nodes/calls persist, FTS follows level 0
    /// through insert / replace / delete, stats sum trips and cost.
    #[tokio::test]
    async fn endless_rows_round_trip_and_fts_follows_level0() {
        let (store, _dir) = fresh().await;
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        assert_eq!(store.session_memory(id).await.unwrap(), MemoryMode::Epochs);
        store
            .set_session_memory(id, MemoryMode::Endless)
            .await
            .unwrap();
        assert_eq!(store.session_memory(id).await.unwrap(), MemoryMode::Endless);
        store
            .set_endless_meta(id, UnitFilter::Meeting, 40)
            .await
            .unwrap();
        store
            .set_endless_view_bytes(id, Some(48_000))
            .await
            .unwrap();
        store
            .set_endless_paused(id, Some("spend"), Some("2026-11-05"))
            .await
            .unwrap();
        store
            .set_endless_paused(id, Some("user"), None)
            .await
            .unwrap();
        store
            .set_endless_model(id, Some("p/chat"), Some("high"), false)
            .await
            .unwrap();
        store
            .set_endless_model(id, Some("p/desk"), Some("low"), true)
            .await
            .unwrap();
        let tail = TailOverride {
            mode: crate::endless::TailMode::Scaled,
            low: 0.08,
            high: 0.3,
        };
        store.set_endless_tail(id, Some(tail)).await.unwrap();
        assert_eq!(
            store.endless_meta(id).await.unwrap(),
            EndlessMeta {
                mode: MemoryMode::Endless,
                filter: UnitFilter::Meeting,
                base: 40,
                view_bytes: Some(48_000),
                paused_by: Some("user".into()),
                spend_override: Some("2026-11-05".into()),
                model_override: Some("p/chat".into()),
                agent_model: Some("p/desk".into()),
                effort_override: Some("high".into()),
                agent_effort: Some("low".into()),
                tail: Some(tail),
            }
        );
        store
            .set_endless_model(id, None, None, false)
            .await
            .unwrap();
        let meta = store.endless_meta(id).await.unwrap();
        assert_eq!((meta.model_override, meta.effort_override), (None, None));
        // 201 §4.2: clearing the tail override reads back as inherit.
        store.set_endless_tail(id, None).await.unwrap();
        assert_eq!(store.endless_meta(id).await.unwrap().tail, None);
        assert_eq!(meta.agent_effort.as_deref(), Some("low"));
        store
            .set_endless_effort(id, Some("high"), false)
            .await
            .unwrap();
        assert_eq!(
            store
                .endless_meta(id)
                .await
                .unwrap()
                .effort_override
                .as_deref(),
            Some("high")
        );
        assert_eq!(store.endless_sessions().await.unwrap().len(), 1);
        assert_eq!(
            store.session(id).await.unwrap().unwrap().memory,
            MemoryMode::Endless
        );

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
            mechanical: false,
        };
        store
            .endless_node_put(id, &node(0, 0, "user: rename the config loader"))
            .await
            .unwrap();
        store
            .endless_node_put(id, &node(0, 1, "echo: nginx reload failed"))
            .await
            .unwrap();
        store
            .endless_node_put(id, &node(1, 0, "user: rename loader; echo: nginx failed"))
            .await
            .unwrap();
        let hits = store.endless_recall(id, "nginx", 8).await.unwrap();
        assert_eq!(
            hits,
            vec![(1, "echo: nginx reload failed".to_owned())],
            "level 1 is not indexed"
        );
        store
            .endless_node_put(id, &node(0, 1, "echo: apache reload failed"))
            .await
            .unwrap();
        assert!(
            store
                .endless_recall(id, "nginx", 8)
                .await
                .unwrap()
                .is_empty(),
            "replace re-indexes"
        );
        assert_eq!(
            store.endless_recall(id, "apache", 8).await.unwrap().len(),
            1
        );
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
        assert_eq!(
            (
                stats.input_tokens,
                stats.cache_read_tokens,
                stats.output_tokens
            ),
            (100, 80, 20)
        );

        store.endless_fts_sync().await.unwrap();
        assert_eq!(
            store.endless_recall(id, "apache", 8).await.unwrap().len(),
            1
        );
        store.delete_session(id).await.unwrap();
        assert!(store.endless_nodes(id).await.unwrap().is_empty());
        assert!(
            store
                .endless_recall(id, "apache", 8)
                .await
                .unwrap()
                .is_empty(),
            "delete trigger"
        );
    }

    async fn rows(store: &Store, table: &str, id: SessionId) -> i64 {
        let mut conn = store.read.acquire().await.unwrap();
        // Table names are this test's literals.
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?");
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(id.to_string())
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    }

    fn gen_row(n: u32) -> GenerationRow {
        GenerationRow {
            n,
            t_cut: u64::from(n) * 10,
            first_seq: i64::from(n) * 10,
            blocks: vec![(format!("view {n}"), true)],
            ledger: Some("{}".into()),
            created_at: None,
        }
    }

    /// 201 R1-8: a new generation row replaces the older ones of the session.
    #[tokio::test]
    async fn generation_put_keeps_only_the_newest() {
        let (store, _dir) = fresh().await;
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.endless_generation_put(id, &gen_row(1)).await.unwrap();
        store.endless_generation_put(id, &gen_row(2)).await.unwrap();
        assert_eq!(rows(&store, "endless_generations", id).await, 1);
        assert_eq!(store.endless_generation(id).await.unwrap().unwrap().n, 2);
    }

    /// 201 R1-1: deleting a session leaves no row of it in any endless table.
    #[tokio::test]
    async fn delete_session_removes_every_endless_row() {
        let (store, _dir) = fresh().await;
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store
            .endless_units_put(
                id,
                &[UnitRow {
                    i: 0,
                    seq: 1,
                    part: 0,
                    kind: Kind::User,
                    size: 10,
                    turn: 0,
                }],
            )
            .await
            .unwrap();
        store.endless_generation_put(id, &gen_row(1)).await.unwrap();
        store
            .endless_ledger_put(id, "{\"goals\":[]}")
            .await
            .unwrap();
        assert_eq!(rows(&store, "endless_generations", id).await, 1);
        assert_eq!(rows(&store, "endless_ledger", id).await, 1);
        store.delete_session(id).await.unwrap();
        for table in [
            "endless_units",
            "endless_nodes",
            "endless_calls",
            "endless_generations",
            "endless_ledger",
            "endless_lines_fts",
        ] {
            assert_eq!(rows(&store, table, id).await, 0, "{table}");
        }
    }
}
