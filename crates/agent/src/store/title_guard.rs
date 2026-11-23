//! A session's title guards (`session_title_guard`, 212 §11): who named the session.
//! Either flag stops the title model for that session. No row = both unset.

use runtime::SessionId;

use super::Store;

/// The guards of one session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TitleGuard {
    /// The agent has called `set_status` in this session.
    pub agent_titled: bool,
    /// The user renamed the session.
    pub user_titled: bool,
}

impl Store {
    /// `session`'s title guards; both `false` without a row.
    pub async fn title_guard(&self, session: SessionId) -> anyhow::Result<TitleGuard> {
        let row: Option<(bool, bool)> = sqlx::query_as(
            "SELECT agent_titled, user_titled FROM session_title_guard WHERE session = ?",
        )
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(
            row.map_or_else(TitleGuard::default, |(agent_titled, user_titled)| {
                TitleGuard {
                    agent_titled,
                    user_titled,
                }
            }),
        )
    }

    /// Mark `session` as retitled by its agent (`set_status`); idempotent.
    pub async fn set_agent_titled(&self, session: SessionId) -> anyhow::Result<()> {
        self.set_guard(
            session,
            "INSERT INTO session_title_guard (session, agent_titled) VALUES (?, 1) \
             ON CONFLICT(session) DO UPDATE SET agent_titled = 1",
        )
        .await
    }

    /// Mark `session` as renamed by the user; idempotent.
    pub async fn set_user_titled(&self, session: SessionId) -> anyhow::Result<()> {
        self.set_guard(
            session,
            "INSERT INTO session_title_guard (session, user_titled) VALUES (?, 1) \
             ON CONFLICT(session) DO UPDATE SET user_titled = 1",
        )
        .await
    }

    /// Run one constant guard upsert for `session`.
    async fn set_guard(&self, session: SessionId, sql: &'static str) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(sql)
            .bind(session.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both guards survive a reopen (an engine restart) and are per session.
    #[tokio::test]
    async fn title_guards_persist() {
        let scratch = crate::test_dir("title-guard");
        let path = scratch.path().join("sessions.sqlite");
        let store = Store::open(&path, None).await.unwrap();
        let (a, b) = (SessionId::new(), SessionId::new());
        for id in [a, b] {
            store
                .create_session(id, "m", None, None, None)
                .await
                .unwrap();
        }
        assert_eq!(store.title_guard(a).await.unwrap(), TitleGuard::default());
        store.set_agent_titled(a).await.unwrap();
        store.set_agent_titled(a).await.unwrap();
        store.set_user_titled(b).await.unwrap();
        drop(store);

        let store = Store::open(&path, None).await.unwrap();
        assert_eq!(
            store.title_guard(a).await.unwrap(),
            TitleGuard {
                agent_titled: true,
                user_titled: false
            }
        );
        store.set_user_titled(a).await.unwrap();
        assert_eq!(
            store.title_guard(a).await.unwrap(),
            TitleGuard {
                agent_titled: true,
                user_titled: true
            }
        );
        assert_eq!(
            store.title_guard(b).await.unwrap(),
            TitleGuard {
                agent_titled: false,
                user_titled: true
            }
        );
        // Deleting the session takes its guards along.
        store.delete_session(a).await.unwrap();
        assert_eq!(store.title_guard(a).await.unwrap(), TitleGuard::default());
    }
}
