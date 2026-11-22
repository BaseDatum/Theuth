//! Prompt suffixes: the saved list (`prompt_suffixes`) and each chat's "every message" suffix
//! (`session_suffixes`) ride the record replica beside the 191 contact tables, so a suffix saved
//! on one machine of the mesh shows on every other, and a chat set to "every message" on one
//! device applies from any other. Removal is the `deleted_ms` column, never a row delete: a late
//! write from another machine lands on a row that stays deleted.

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use super::Store;

/// The replicated suffix tables, attached by [`Store::replicate_records`].
pub const SUFFIX_TABLES: [&str; 2] = ["prompt_suffixes", "session_suffixes"];

/// Longest suffix name and text a `put_suffix` accepts (bytes).
const NAME_MAX: usize = 200;
const TEXT_MAX: usize = 64 * 1024;

/// One saved prompt suffix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PromptSuffix {
    /// Client-chosen id (a uuid); stable across machines.
    pub id: String,
    /// Shown in the `+` menu and the chip.
    pub name: String,
    /// Appended to a message after a blank line.
    pub text: String,
}

#[derive(FromRow)]
struct Row {
    id: String,
    name: Option<String>,
    body: Option<String>,
}

impl Store {
    /// Every live suffix, oldest first. A row that arrived a column at a time shows with the
    /// columns it has.
    pub async fn suffixes(&self) -> anyhow::Result<Vec<PromptSuffix>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, name, body FROM prompt_suffixes WHERE deleted_ms IS NULL \
             ORDER BY created_ms, id",
        )
        .fetch_all(&self.read)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| PromptSuffix {
                id: r.id,
                name: r.name.unwrap_or_default(),
                text: r.body.unwrap_or_default(),
            })
            .collect())
    }

    /// Create or update `s` by id. `false` (and nothing written) when that id was deleted:
    /// a deleted suffix never comes back, not even from a late migration upload.
    pub async fn put_suffix(&self, s: &PromptSuffix, now_ms: i64) -> anyhow::Result<bool> {
        anyhow::ensure!(!s.id.trim().is_empty(), "suffix id is empty");
        anyhow::ensure!(s.name.len() <= NAME_MAX, "suffix name is too long");
        anyhow::ensure!(s.text.len() <= TEXT_MAX, "suffix text is too long");
        let mut tx = self.write.begin().await?;
        let deleted: Option<Option<i64>> =
            sqlx::query_scalar("SELECT deleted_ms FROM prompt_suffixes WHERE id = ?")
                .bind(&s.id)
                .fetch_optional(&mut *tx)
                .await?;
        match deleted {
            Some(Some(_)) => return Ok(false),
            Some(None) => {
                sqlx::query("UPDATE prompt_suffixes SET name = ?, body = ? WHERE id = ?")
                    .bind(&s.name)
                    .bind(&s.text)
                    .bind(&s.id)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO prompt_suffixes (id, name, body, created_ms) VALUES (?, ?, ?, ?)",
                )
                .bind(&s.id)
                .bind(&s.name)
                .bind(&s.text)
                .bind(now_ms)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Mark suffix `id` deleted (a tombstone column, kept so the deletion wins everywhere).
    /// Writes the tombstone even for an id this engine has not seen yet. Returns whether a live
    /// suffix was deleted.
    pub async fn delete_suffix(&self, id: &str, now_ms: i64) -> anyhow::Result<bool> {
        let mut tx = self.write.begin().await?;
        let live: Option<Option<i64>> =
            sqlx::query_scalar("SELECT deleted_ms FROM prompt_suffixes WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let deleted = match live {
            Some(Some(_)) => false,
            Some(None) => {
                sqlx::query("UPDATE prompt_suffixes SET deleted_ms = ? WHERE id = ?")
                    .bind(now_ms)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                true
            }
            None => {
                sqlx::query("INSERT INTO prompt_suffixes (id, deleted_ms) VALUES (?, ?)")
                    .bind(id)
                    .bind(now_ms)
                    .execute(&mut *tx)
                    .await?;
                false
            }
        };
        tx.commit().await?;
        Ok(deleted)
    }

    /// The live suffix `session` sends with every message, if any.
    pub async fn session_suffix(&self, session: SessionId) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT s.suffix FROM session_suffixes s \
             JOIN prompt_suffixes p ON p.id = s.suffix AND p.deleted_ms IS NULL \
             WHERE s.session = ?",
        )
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?)
    }

    /// Set (`Some`) or turn off (`None`) the suffix `session` sends with every message.
    pub async fn set_session_suffix(
        &self,
        session: SessionId,
        suffix: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO session_suffixes (session, suffix) VALUES (?, ?) \
             ON CONFLICT(session) DO UPDATE SET suffix = excluded.suffix",
        )
        .bind(session.to_string())
        .bind(suffix)
        .execute(&self.write)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suffix(id: &str, name: &str, text: &str) -> PromptSuffix {
        PromptSuffix {
            id: id.to_owned(),
            name: name.to_owned(),
            text: text.to_owned(),
        }
    }

    async fn open(name: &str) -> (Store, crate::TestDir) {
        let dir = crate::TestDir::new(name);
        let store = Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn put_list_delete_round_trip() {
        let (store, _dir) = open("suffix-rt").await;
        assert!(store.suffixes().await.unwrap().is_empty());
        assert!(
            store
                .put_suffix(&suffix("a", "Terse", "Be terse."), 1)
                .await
                .unwrap()
        );
        assert!(
            store
                .put_suffix(&suffix("b", "Cite", "Cite sources."), 2)
                .await
                .unwrap()
        );
        assert!(
            store
                .put_suffix(&suffix("a", "Terse!", "Be very terse."), 3)
                .await
                .unwrap()
        );
        assert_eq!(
            store.suffixes().await.unwrap(),
            [
                suffix("a", "Terse!", "Be very terse."),
                suffix("b", "Cite", "Cite sources.")
            ]
        );
        assert!(store.delete_suffix("a", 4).await.unwrap());
        assert!(!store.delete_suffix("a", 5).await.unwrap());
        assert_eq!(
            store.suffixes().await.unwrap(),
            [suffix("b", "Cite", "Cite sources.")]
        );
        assert!(
            !store
                .put_suffix(&suffix("a", "Back", "x"), 6)
                .await
                .unwrap(),
            "a deleted id is never recreated"
        );
        assert_eq!(store.suffixes().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_always_activation_is_stored_with_the_session() {
        let (store, _dir) = open("suffix-session").await;
        let session = SessionId::new();
        let other = SessionId::new();
        store
            .put_suffix(&suffix("a", "Terse", "Be terse."), 1)
            .await
            .unwrap();
        assert_eq!(store.session_suffix(session).await.unwrap(), None);
        store.set_session_suffix(session, Some("a")).await.unwrap();
        assert_eq!(
            store.session_suffix(session).await.unwrap().as_deref(),
            Some("a")
        );
        assert_eq!(store.session_suffix(other).await.unwrap(), None);
        store.set_session_suffix(session, None).await.unwrap();
        assert_eq!(store.session_suffix(session).await.unwrap(), None);
        store.set_session_suffix(session, Some("a")).await.unwrap();
        store.delete_suffix("a", 2).await.unwrap();
        assert_eq!(
            store.session_suffix(session).await.unwrap(),
            None,
            "a deleted suffix is off everywhere"
        );
    }

    /// Two engines on the real record replica: a suffix created on A shows on B, an edit
    /// follows, a deletion on A removes it on B, and a late edit on B does not bring it back.
    #[tokio::test]
    async fn suffixes_and_their_deletion_ride_the_record_replica() {
        let open = |name: &'static str| async move {
            let (store, dir) = open(name).await;
            let engine = replica::Engine::open(&dir.0.join("replica.sqlite"))
                .await
                .unwrap();
            let handle = engine.attach(&store.write, &SUFFIX_TABLES).await.unwrap();
            (store, engine, handle, dir)
        };
        let (a, ea, ha, _da) = open("suffix-rep-a").await;
        let (b, eb, hb, _db) = open("suffix-rep-b").await;
        let mut a_feed = ea.subscribe();
        let mut b_feed = eb.subscribe();
        let a_to_b = |feed: &mut tokio::sync::broadcast::Receiver<replica::ChangeBatch>| {
            let mut out = Vec::new();
            while let Ok(batch) = feed.try_recv() {
                out.push(batch);
            }
            out
        };

        a.put_suffix(&suffix("s1", "Terse", "Be terse."), 1)
            .await
            .unwrap();
        let session = SessionId::new();
        a.set_session_suffix(session, Some("s1")).await.unwrap();
        ha.flush().await.unwrap();
        for batch in a_to_b(&mut a_feed) {
            eb.apply(&batch).await.unwrap();
        }
        assert_eq!(
            b.suffixes().await.unwrap(),
            [suffix("s1", "Terse", "Be terse.")]
        );
        assert_eq!(
            b.session_suffix(session).await.unwrap().as_deref(),
            Some("s1")
        );

        a.put_suffix(&suffix("s1", "Terse", "Be very terse."), 2)
            .await
            .unwrap();
        ha.flush().await.unwrap();
        for batch in a_to_b(&mut a_feed) {
            eb.apply(&batch).await.unwrap();
        }
        assert_eq!(b.suffixes().await.unwrap()[0].text, "Be very terse.");

        a.delete_suffix("s1", 3).await.unwrap();
        ha.flush().await.unwrap();
        for batch in a_to_b(&mut a_feed) {
            eb.apply(&batch).await.unwrap();
        }
        assert!(b.suffixes().await.unwrap().is_empty(), "deleted on B too");
        assert_eq!(b.session_suffix(session).await.unwrap(), None);

        // B never saw the deletion's row as live again: a put there is refused, and a direct
        // column write that ships back to A leaves the row deleted.
        assert!(!b.put_suffix(&suffix("s1", "Again", "x"), 4).await.unwrap());
        sqlx::query("UPDATE prompt_suffixes SET name = 'late' WHERE id = 's1'")
            .execute(&b.write)
            .await
            .unwrap();
        hb.flush().await.unwrap();
        for batch in a_to_b(&mut b_feed) {
            ea.apply(&batch).await.unwrap();
        }
        assert!(
            a.suffixes().await.unwrap().is_empty(),
            "no resurrection on A"
        );
    }
}
