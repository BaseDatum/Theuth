//! Owner: 171F E2 — the note a chat reply was opened as (171F B9: "Open as note" reopens it).
//! Whether the doc still exists is the UI's check.

use crate::actor::Cells;
use crate::error::CellsError;
use crate::types::{DocId, SessionId};

impl Cells {
    /// `cell_reply_note_get`.
    pub async fn reply_note_get(
        &self,
        session: SessionId,
        entry: &str,
    ) -> Result<Option<DocId>, CellsError> {
        let doc = self
            .store()?
            .reply_note(&session.to_string(), entry)
            .await?;
        Ok(doc.map(DocId))
    }

    /// `cell_reply_note_set`.
    pub async fn reply_note_set(
        &self,
        session: SessionId,
        entry: &str,
        doc: &DocId,
    ) -> Result<(), CellsError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.store()?
            .set_reply_note(&session.to_string(), entry, &doc.0, now)
            .await
    }
}
