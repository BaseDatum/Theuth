//! Owner: E2 — note scope (design 171 §5.1, §9.6, §13.2): kernel session ids, hub routing,
//! source resolution through `document::CodeBlockText`, origin delivery, notebook result lines,
//! `cell_latest`. E1's actor calls these at its `// 171:E2` lines; E2 never edits `actor.rs`.

use crate::error::{CellsError, Result};
use crate::types::{CellLatest, CellRun, DocId, NoteKey, SessionId};

/// Where a note cell runs (§5.1 W-d): the document's hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// This engine is the hub.
    Local,
    /// Forward to the hub engine (its link peer id).
    Hub(String),
}

/// The source the engine runs (§3.4): the live Y.Doc's block.
pub type NoteSource = document::CodeBlockTextReply;

/// §5.1 W-b: the deterministic session id that owns a note kernel's PTY job:
/// UUIDv5(UUIDv5(URL, "theuth:cells"), doc id). Never appears in a session list.
pub fn kernel_session(doc: &DocId) -> SessionId {
    let ns = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"theuth:cells");
    SessionId(uuid::Uuid::new_v5(&ns, doc.0.as_bytes()))
}

/// Plugin start: install the `document::RunAnnotations` impl so `doc_read` prints each
/// cell's latest run (§4.4, §9.6). No-op until E2.
pub fn install(cells: &crate::Cells) {
    let _ = cells;
}

/// Which engine runs `doc`'s cells.
pub async fn route(doc: &DocId) -> Result<Route, CellsError> {
    let _ = doc;
    Err(CellsError::NotBuilt("E2"))
}

/// Read a note cell's current text, language and last author (`CodeBlockText`).
pub async fn source(doc: &DocId, key: &NoteKey) -> Result<NoteSource, CellsError> {
    let _ = (doc, key);
    Err(CellsError::NotBuilt("E2"))
}

/// §9.6: the session whose agent last wrote the block; `None` = the owner wrote it.
pub async fn origin(doc: &DocId, block: &str) -> Option<SessionId> {
    let _ = (doc, block);
    None
}

/// §9.6: deliver a finished note run: the full `<cell_result in="note …">` to the origin
/// session and the body-less quiet line to every other bound notebook chat; then upsert
/// `cell_latest` (§13.2).
pub async fn deliver(run: &CellRun, cleaned: &str) -> Result<(), CellsError> {
    let _ = (run, cleaned);
    Ok(())
}

/// §13.2: the replicated latest runs of `doc` (any engine; membership is checked by the RPC).
pub async fn latest(doc: &DocId) -> Result<Vec<CellLatest>, CellsError> {
    let _ = doc;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_session_is_deterministic_per_doc() {
        let a = kernel_session(&DocId("doc-a".into()));
        assert_eq!(a, kernel_session(&DocId("doc-a".into())));
        assert_ne!(a, kernel_session(&DocId("doc-b".into())));
        assert_eq!(a.0.get_version_num(), 5);
    }
}
