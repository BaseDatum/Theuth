//! sqlite session store (sqlx): `sessions` + `messages`; resume by replay.
//! Only the agent writes it; UIs read through the supervisor's commands.

use std::path::Path;

use genai::chat::ChatMessage;
use runtime::SessionId;

use crate::SessionSummary;

/// Handle to `<data_dir>/sessions.sqlite`. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    // filled in by the store implementation
}

impl Store {
    /// Open (creating) the database in WAL mode and run migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let _ = path;
        todo!("store implementation")
    }

    /// Insert a session row. `title` starts empty.
    pub async fn create_session(&self, id: SessionId, model: &str, parent: Option<SessionId>) -> anyhow::Result<()> {
        let _ = (id, model, parent);
        todo!("store implementation")
    }

    /// Set the display title (first user message, truncated by the caller).
    pub async fn set_title(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        let _ = (id, title);
        todo!("store implementation")
    }

    /// Append one message; `seq` is assigned as max+1 within the session.
    pub async fn append(&self, id: SessionId, message: &ChatMessage) -> anyhow::Result<()> {
        let _ = (id, message);
        todo!("store implementation")
    }

    /// All messages in `seq` order.
    pub async fn messages(&self, id: SessionId) -> anyhow::Result<Vec<ChatMessage>> {
        let _ = id;
        todo!("store implementation")
    }

    /// One session, if it exists.
    pub async fn session(&self, id: SessionId) -> anyhow::Result<Option<SessionSummary>> {
        let _ = id;
        todo!("store implementation")
    }

    /// Every session, newest first.
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        todo!("store implementation")
    }
}
