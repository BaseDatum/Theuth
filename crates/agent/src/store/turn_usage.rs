//! Per-turn token ledger for ACP children (75 §7): `remote_turns`, one row per
//! finished turn whose `session/prompt` response carried `usage`. Tokens are
//! stored as the harness sent them — omp and opencode ≥ 2 the turn's sum,
//! opencode 1.x the last request only (upstream #1860), flagged `lower_bound`
//! from the child's `initialize.agentInfo` (06 §8.4). `session_cost` folds
//! them into the owner's `TokenTotals` and answers the per-model breakdown.

use std::collections::BTreeMap;

use runtime::{SessionId, TurnId};
use sqlx::{FromRow, SqliteConnection};

use super::Store;
use super::accounting::{LINEAGE_MAX, bump_in};
use crate::cost::{RemoteTokens, TokenTotals};

/// Sums of one owner's rows for one `(harness, model)`.
#[derive(FromRow)]
pub(super) struct Sums {
    owner_id: String,
    harness: String,
    model: Option<String>,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    turns: i64,
    /// Any row of the group is a last-request figure.
    lower_bound: bool,
}

impl Sums {
    pub(super) fn owner(&self) -> &str {
        &self.owner_id
    }

    /// Add these sums into `into` (saturating; `unknown_calls` untouched).
    pub(super) fn add_to(&self, into: &mut TokenTotals) {
        let t = |n: i64| u64::try_from(n).unwrap_or(0);
        into.input = into.input.saturating_add(t(self.input));
        into.output = into.output.saturating_add(t(self.output));
        into.cache_read = into.cache_read.saturating_add(t(self.cache_read));
        into.cache_write = into.cache_write.saturating_add(t(self.cache_write));
    }
}

/// Per `(owner, harness, model)` sums over `root`'s accounting subtree.
pub(super) async fn sums(conn: &mut SqliteConnection, root: &str) -> anyhow::Result<Vec<Sums>> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE sub(id, depth) AS (SELECT ?1, 0 UNION ALL \
         SELECT c.id, sub.depth + 1 FROM cost_owners c JOIN sub ON c.parent_id = sub.id \
         WHERE sub.depth < ?2) \
         SELECT owner_id, harness, model, SUM(input_tokens) AS input, SUM(output_tokens) AS output, \
         SUM(cache_read_tokens) AS cache_read, SUM(cache_write_tokens) AS cache_write, COUNT(*) AS turns, \
         MAX(lower_bound) AS lower_bound \
         FROM remote_turns WHERE owner_id IN (SELECT id FROM sub) GROUP BY owner_id, harness, model",
    )
    .bind(root)
    .bind(LINEAGE_MAX)
    .fetch_all(conn)
    .await?)
}

/// The subtree-wide `(harness, model)` breakdown of `rows`, sorted by harness
/// then model.
pub(super) fn by_model(rows: &[Sums]) -> Vec<RemoteTokens> {
    let mut by_model: BTreeMap<(&str, Option<&str>), (u32, TokenTotals, bool)> = BTreeMap::new();
    for r in rows {
        let e = by_model
            .entry((&r.harness, r.model.as_deref()))
            .or_default();
        e.0 =
            e.0.saturating_add(u32::try_from(r.turns).unwrap_or(u32::MAX));
        r.add_to(&mut e.1);
        e.2 |= r.lower_bound;
    }
    by_model
        .into_iter()
        .map(
            |((harness, model), (turns, tokens, lower_bound))| RemoteTokens {
                harness: harness.to_owned(),
                model: model.map(str::to_owned),
                turns,
                tokens,
                lower_bound,
            },
        )
        .collect()
}

impl Store {
    /// Record one finished ACP turn's tokens under `owner` (an accounting
    /// owner); a repeat for the same turn overwrites. `lower_bound` marks a
    /// last-request figure (06 §8.4). Bumps the owner's and its ancestors'
    /// revisions and hands them back for `ModelUsageChanged`.
    pub async fn record_remote_turn(
        &self,
        owner: SessionId,
        turn: TurnId,
        harness: &str,
        model: Option<&str>,
        tokens: &TokenTotals,
        lower_bound: bool,
    ) -> anyhow::Result<Vec<(SessionId, u64)>> {
        let sid = owner.to_string();
        let n = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO remote_turns (owner_id, turn_id, harness, model, input_tokens, \
             output_tokens, cache_read_tokens, cache_write_tokens, at, lower_bound) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&sid)
        .bind(turn.to_string())
        .bind(harness)
        .bind(model)
        .bind(n(tokens.input))
        .bind(n(tokens.output))
        .bind(n(tokens.cache_read))
        .bind(n(tokens.cache_write))
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(lower_bound)
        .execute(&mut *tx)
        .await?;
        let bumped = bump_in(&mut tx, &sid).await?;
        tx.commit().await?;
        Ok(bumped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> Store {
        let dir = std::env::temp_dir().join(format!("theuth-turns-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap()
    }

    fn t(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenTotals {
        TokenTotals {
            input,
            output,
            cache_read,
            cache_write,
            unknown_calls: 0,
        }
    }

    /// 75 §7: rows sum into the child's and the parent's `TokenTotals`, the
    /// breakdown groups by model across a mid-session switch, the cache
    /// split survives, a group with one last-request row is a lower bound,
    /// and a root delete takes the rows with it.
    #[tokio::test]
    async fn remote_turns_sum_per_model_and_cascade() {
        let store = fresh().await;
        let (root, child) = (SessionId::new(), SessionId::new());
        store.ensure_owner(root, None).await.unwrap();
        store.ensure_owner(child, Some(root)).await.unwrap();
        let before = store.session_cost(root).await.unwrap().unwrap().revision;

        let bumped = store
            .record_remote_turn(
                child,
                TurnId::new(),
                "omp",
                Some("a/one"),
                &t(100, 10, 60, 5),
                false,
            )
            .await
            .unwrap();
        assert_eq!(bumped.len(), 2, "child and root revisions bumped");
        store
            .record_remote_turn(
                child,
                TurnId::new(),
                "omp",
                Some("a/one"),
                &t(1, 1, 0, 0),
                false,
            )
            .await
            .unwrap();
        store
            .record_remote_turn(
                child,
                TurnId::new(),
                "omp",
                Some("b/two"),
                &t(200, 20, 0, 0),
                true,
            )
            .await
            .unwrap();

        let rc = store.session_cost(root).await.unwrap().unwrap();
        assert!(rc.revision > before);
        assert_eq!(rc.direct.tokens, TokenTotals::default());
        assert_eq!(rc.total.tokens, t(301, 31, 60, 5));
        assert_eq!(rc.children[0].total.tokens, t(301, 31, 60, 5));
        let by: Vec<_> = rc
            .remote_tokens
            .iter()
            .map(|r| {
                (
                    r.harness.as_str(),
                    r.model.as_deref(),
                    r.turns,
                    r.tokens.clone(),
                    r.lower_bound,
                )
            })
            .collect();
        assert_eq!(
            by,
            [
                ("omp", Some("a/one"), 2, t(101, 11, 60, 5), false),
                ("omp", Some("b/two"), 1, t(200, 20, 0, 0), true),
            ]
        );
        let cc = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(cc.direct.tokens, t(301, 31, 60, 5));

        store.delete_root_ledger(root).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_turns")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(left, 0, "FK cascade removed the rows");
    }
}
