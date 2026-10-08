//! 139 Revision 2: the engine-local harness outbox (`harness_outbox`) and
//! the receiver's seen set (`harness_seen`). Neither table is in
//! `capture::ATTACHED`: both are about sessions this engine does not own,
//! which the fence would refuse. The sender is `Agents::inject`; the
//! shipper (`sessionsync::sync`) sends rows over `Msg::Harness`, deletes
//! them on ack, and wakes on [`Store::harness_watch`].

use sqlx::FromRow;

use super::Store;

/// One queued harness-made input for a session owned elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct HarnessRow {
    /// Sender-minted UUID; the receiver dedups on it.
    pub id: String,
    /// The session, as text.
    pub session: String,
    /// The `from` label `inject` carries (`job j3`, `reminder r1`, `session:<child>`).
    pub kind: String,
    /// The engine that minted the row.
    pub origin: String,
    /// The bare body (`AgentEvent::MessageDelivered.text`).
    pub text: String,
    /// The body as the model reads it (`<job_result>`, `<reminder>`, …).
    pub wrapped: String,
    /// The sender's request scope (50 §8), if the work had one.
    pub scope: Option<String>,
    /// Unix ms when enqueued; the TTL counts from here.
    pub at: i64,
    /// Sends so far.
    pub attempts: i64,
    /// What the last attempt's ack said, when it refused.
    pub last_error: Option<String>,
}

impl Store {
    /// Queue `row` and wake the shippers.
    pub async fn harness_enqueue(&self, row: &HarnessRow) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO harness_outbox (id, session, kind, origin, text, wrapped, scope, at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.session)
        .bind(&row.kind)
        .bind(&row.origin)
        .bind(&row.text)
        .bind(&row.wrapped)
        .bind(&row.scope)
        .bind(row.at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.harness_tx.send_modify(|n| *n += 1);
        Ok(())
    }

    /// Every queued row, oldest first.
    pub async fn harness_outbox(&self) -> anyhow::Result<Vec<HarnessRow>> {
        Ok(sqlx::query_as(
            "SELECT id, session, kind, origin, text, wrapped, scope, at, attempts, last_error \
             FROM harness_outbox ORDER BY at, id",
        )
        .fetch_all(&self.read)
        .await?)
    }

    /// One more send of `id`; the previous refusal, if any, is cleared until the ack.
    pub async fn harness_sent(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE harness_outbox SET attempts = attempts + 1, last_error = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The receiver refused `id` (`Msg::HarnessAck { error }`): keep the row, note why.
    pub async fn harness_refused(&self, id: &str, error: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE harness_outbox SET last_error = ? WHERE id = ?")
            .bind(error)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Acked, delivered here, or expired: the row goes.
    pub async fn harness_delete(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM harness_outbox WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Receiver side: remember `id`; `false` when it was already seen.
    pub async fn harness_mark_seen(&self, id: &str, at: i64) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("INSERT OR IGNORE INTO harness_seen (id, at) VALUES (?, ?)")
            .bind(id)
            .bind(at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n == 1)
    }

    /// The handoff failed after marking: forget `id` so a resend is delivered.
    pub async fn harness_unsee(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM harness_seen WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Forget seen ids older than `before` (unix ms): the sender's TTL bounds a resend.
    pub async fn harness_prune_seen(&self, before: i64) -> anyhow::Result<u64> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("DELETE FROM harness_seen WHERE at < ?")
            .bind(before)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n)
    }

    /// Wakes after every [`Store::harness_enqueue`].
    pub fn harness_watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.harness_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use runtime::SessionId;

    use super::*;

    fn row(id: &str) -> HarnessRow {
        HarnessRow {
            id: id.into(),
            session: SessionId::new().to_string(),
            kind: "job j1".into(),
            origin: "a".into(),
            text: "done".into(),
            wrapped: "<job_result>done</job_result>".into(),
            scope: None,
            at: 5,
            attempts: 0,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn outbox_rows_round_trip_count_attempts_and_wake_the_watch() {
        let store = Store::open_memory().await.unwrap();
        let mut watch = store.harness_watch();
        store.harness_enqueue(&row("x")).await.unwrap();
        assert!(watch.has_changed().unwrap());
        watch.mark_unchanged();
        store.harness_sent("x").await.unwrap();
        store.harness_refused("x", "unknown session").await.unwrap();
        let rows = store.harness_outbox().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].attempts, rows[0].last_error.as_deref()),
            (1, Some("unknown session"))
        );
        store.harness_sent("x").await.unwrap();
        let rows = store.harness_outbox().await.unwrap();
        assert_eq!((rows[0].attempts, rows[0].last_error.as_deref()), (2, None));
        assert_eq!(rows[0].kind, "job j1");
        store.harness_delete("x").await.unwrap();
        assert!(store.harness_outbox().await.unwrap().is_empty());
        assert!(
            !watch.has_changed().unwrap(),
            "only an enqueue wakes the shippers"
        );
    }

    #[tokio::test]
    async fn seen_ids_dedup_until_pruned() {
        let store = Store::open_memory().await.unwrap();
        assert!(store.harness_mark_seen("x", 10).await.unwrap());
        assert!(!store.harness_mark_seen("x", 11).await.unwrap());
        store.harness_unsee("x").await.unwrap();
        assert!(store.harness_mark_seen("x", 12).await.unwrap());
        assert_eq!(store.harness_prune_seen(12).await.unwrap(), 0);
        assert_eq!(store.harness_prune_seen(13).await.unwrap(), 1);
        assert!(store.harness_mark_seen("x", 14).await.unwrap());
    }
}
