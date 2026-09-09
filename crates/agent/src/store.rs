//! sqlite session store (sqlx): `sessions` + `messages`; resume by replay.
//! Only the agent writes it; UIs read through the supervisor's commands.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use genai::chat::ChatMessage;
use runtime::SessionId;
use sqlx::FromRow;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::SessionSummary;

/// Handle to `<data_dir>/sessions.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// Raw `sessions` row; ids and timestamps are TEXT and parsed on the way out.
#[derive(FromRow)]
struct SessionRow {
    id: String,
    created_at: String,
    title: String,
    model: String,
    parent: Option<String>,
}

impl TryFrom<SessionRow> for SessionSummary {
    type Error = anyhow::Error;

    fn try_from(r: SessionRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: r.id.parse().context("bad session id")?,
            created_at: r.created_at.parse().context("bad created_at")?,
            title: r.title,
            model: r.model,
            parent: r
                .parent
                .as_deref()
                .map(str::parse)
                .transpose()
                .context("bad parent id")?,
        })
    }
}

impl Store {
    /// Open (creating) the database in WAL mode and run migrations.
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

    /// Insert a session row. `title` starts empty.
    pub async fn create_session(
        &self,
        id: SessionId,
        model: &str,
        parent: Option<SessionId>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO sessions (id, created_at, title, model, parent) VALUES (?, ?, '', ?, ?)",
        )
        .bind(id.to_string())
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(model)
        .bind(parent.map(|p| p.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set the display title (first user message, truncated by the caller).
    pub async fn set_title(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Append one message; `seq` is assigned as max+1 within the session.
    pub async fn append(&self, id: SessionId, message: &ChatMessage) -> anyhow::Result<()> {
        let body = serde_json::to_string(message)?;
        sqlx::query(
            "INSERT INTO messages (session_id, seq, body) \
             SELECT ?1, COALESCE(MAX(seq), -1) + 1, ?2 FROM messages WHERE session_id = ?1",
        )
        .bind(id.to_string())
        .bind(body)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All messages in `seq` order.
    pub async fn messages(&self, id: SessionId) -> anyhow::Result<Vec<ChatMessage>> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT seq, body FROM messages WHERE session_id = ? ORDER BY seq")
                .bind(id.to_string())
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(seq, body)| match serde_json::from_str(&body) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(session = %id, seq, error = %e, "skipping undecodable message row");
                    None
                }
            })
            .collect())
    }

    /// One session, if it exists.
    pub async fn session(&self, id: SessionId) -> anyhow::Result<Option<SessionSummary>> {
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT id, created_at, title, model, parent FROM sessions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(SessionSummary::try_from).transpose()
    }

    /// Every session, newest first.
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let rows: Vec<SessionRow> =
            sqlx::query_as("SELECT id, created_at, title, model, parent FROM sessions ORDER BY created_at DESC, id DESC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(SessionSummary::try_from).collect()
    }
}

#[cfg(test)]
mod tests {
    use genai::chat::{ChatRole, MessageContent};

    use super::*;

    #[tokio::test]
    async fn resume_round_trip() {
        let dir = std::env::temp_dir().join(format!("wth-store-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite")).await.unwrap();

        let id = SessionId::new();
        store.create_session(id, "m", None).await.unwrap();
        for m in [
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant(MessageContent::from_text("hello")),
        ] {
            store.append(id, &m).await.unwrap();
        }

        let got = store.messages(id).await.unwrap();
        let flat: Vec<(ChatRole, String)> = got
            .iter()
            .map(|m| (m.role.clone(), m.content.joined_texts().unwrap_or_default()))
            .collect();
        assert_eq!(
            flat,
            [
                (ChatRole::System, "sys".to_string()),
                (ChatRole::User, "hi".to_string()),
                (ChatRole::Assistant, "hello".to_string()),
            ]
        );

        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].parent, None);
        assert_eq!(list[0].title, "");

        let child = SessionId::new();
        store.create_session(child, "m", Some(id)).await.unwrap();
        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), [child, id]);
        assert_eq!(list[0].parent, Some(id));

        store.set_title(id, "first words").await.unwrap();
        assert_eq!(
            store.session(id).await.unwrap().unwrap().title,
            "first words"
        );
        assert!(store.session(SessionId::new()).await.unwrap().is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
