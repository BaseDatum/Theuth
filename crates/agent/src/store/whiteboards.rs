//! 218 §5: the node-local `session_whiteboards` table (migration 0077). One row per
//! (root session, whiteboard). Not captured, so nothing here reaches the session log and
//! `SESSIONS_WIRE_SCHEMA` is unchanged; every engine re-derives its rows (marker, composer, tool).

use platform::time::now_ms_i64 as now_ms;
use runtime::SessionId;

use super::Store;

impl Store {
    /// Bind `session`'s root to `whiteboard` (`by`: `marker` | `composer` | `tool` | `rpc`).
    /// An existing binding gets a fresh `bound_ms` and `by`. `true` when the row is new.
    pub async fn bind_whiteboard(
        &self,
        session: SessionId,
        whiteboard: &str,
        by: &str,
    ) -> anyhow::Result<bool> {
        let root = self.root_of(session).await?;
        let mut tx = self.begin_write().await?;
        let existed: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM session_whiteboards WHERE session_root = ? AND whiteboard = ?",
        )
        .bind(root.to_string())
        .bind(whiteboard)
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO session_whiteboards (session_root, whiteboard, bound_ms, by) VALUES (?, ?, ?, ?) \
             ON CONFLICT(session_root, whiteboard) DO UPDATE SET bound_ms = excluded.bound_ms, by = excluded.by",
        )
        .bind(root.to_string())
        .bind(whiteboard)
        .bind(now_ms())
        .bind(by)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(existed.is_none())
    }

    /// Remove the binding of `session`'s root to `whiteboard`; idempotent. `true` when a row went.
    pub async fn unbind_whiteboard(&self, session: SessionId, whiteboard: &str) -> anyhow::Result<bool> {
        let root = self.root_of(session).await?;
        let mut tx = self.begin_write().await?;
        let done = sqlx::query(
            "DELETE FROM session_whiteboards WHERE session_root = ? AND whiteboard = ?",
        )
        .bind(root.to_string())
        .bind(whiteboard)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(done.rows_affected() > 0)
    }

    /// Remove every binding of `whiteboard` (it is gone or trashed). Returns the rows removed.
    pub async fn forget_whiteboard(&self, whiteboard: &str) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let done = sqlx::query("DELETE FROM session_whiteboards WHERE whiteboard = ?")
            .bind(whiteboard)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(done.rows_affected())
    }

    /// The whiteboards `session`'s root is bound to, `(whiteboard, bound_ms)`, newest binding first.
    pub async fn whiteboards_of(&self, session: SessionId) -> anyhow::Result<Vec<(String, i64)>> {
        let root = self.root_of(session).await?;
        Ok(sqlx::query_as(
            "SELECT whiteboard, bound_ms FROM session_whiteboards WHERE session_root = ? \
             ORDER BY bound_ms DESC, whiteboard",
        )
        .bind(root.to_string())
        .fetch_all(&self.read)
        .await?)
    }

    /// The root sessions bound to `whiteboard`, `(root, bound_ms)`, newest binding first.
    /// Rows whose root id does not parse are skipped.
    pub async fn sessions_of_whiteboard(
        &self,
        whiteboard: &str,
    ) -> anyhow::Result<Vec<(SessionId, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT session_root, bound_ms FROM session_whiteboards WHERE whiteboard = ? \
             ORDER BY bound_ms DESC, session_root",
        )
        .bind(whiteboard)
        .fetch_all(&self.read)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(s, ms)| s.parse().ok().map(|s| (s, ms)))
            .collect())
    }

    /// Drop every binding of the root `session` (it no longer exists).
    pub async fn forget_whiteboard_session(&self, root: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM session_whiteboards WHERE session_root = ?")
            .bind(root.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> (Store, crate::TestDir) {
        let scratch = crate::test_dir("store-whiteboards");
        let store = Store::open(&scratch.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, scratch)
    }

    /// A child's binding is its root's: bind through the child, read through either; unbind is idempotent.
    #[tokio::test]
    async fn bind_unbind_root_resolved() {
        let (store, _dir) = fresh().await;
        let (root, child, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        store.create_session(root, "m", None, None, None).await.unwrap();
        store.create_session(child, "m", Some(root), None, None).await.unwrap();
        store.create_session(other, "m", None, None, None).await.unwrap();
        let wb = "019a0000-0000-7000-8000-000000000001";
        assert!(store.bind_whiteboard(child, wb, "marker").await.unwrap(), "first bind is new");
        assert!(!store.bind_whiteboard(root, wb, "composer").await.unwrap(), "same root: an upsert");
        store.bind_whiteboard(other, wb, "tool").await.unwrap();
        assert_eq!(store.whiteboards_of(child).await.unwrap().len(), 1);
        assert_eq!(store.whiteboards_of(root).await.unwrap()[0].0, wb);
        let sessions: Vec<SessionId> = store
            .sessions_of_whiteboard(wb)
            .await
            .unwrap()
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&root) && sessions.contains(&other));
        assert!(!sessions.contains(&child), "rows hold the root");
        assert!(store.unbind_whiteboard(child, wb).await.unwrap());
        assert!(!store.unbind_whiteboard(child, wb).await.unwrap(), "idempotent");
        assert!(store.whiteboards_of(root).await.unwrap().is_empty());
        assert_eq!(store.forget_whiteboard(wb).await.unwrap(), 1);
        assert!(store.sessions_of_whiteboard(wb).await.unwrap().is_empty());
    }

    /// 218 §5: the table is node-local — not attached to the session log, the wire schema stays 70.
    #[tokio::test]
    async fn node_local_no_wire_bump() {
        assert_eq!(crate::store::SESSIONS_WIRE_SCHEMA, 70);
        assert!(
            !super::super::capture::ATTACHED
                .iter()
                .any(|(t, _)| *t == "session_whiteboards"),
            "session_whiteboards must stay node-local"
        );
        let (store, _dir) = fresh().await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_whiteboards")
            .fetch_one(&store.read)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
