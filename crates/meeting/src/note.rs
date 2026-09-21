//! The meeting's note (design 59 §7.4): one `MeetingNote` document per meeting,
//! created at `meeting_start`, shown beside the live transcript, edited by the
//! summary turn through the `doc_*` tools — the user's blocks are moved, never
//! rewritten — and exported as `meetings/<id>/notes.md` until the summary is in
//! it, then as `summary.md` so `GetSummary`, harvest and `meeting_summary` read
//! what they always did. Every call goes through `document::Handle` lazily:
//! documents off → `None`/`Err`, the meeting goes on as before 59.

use document::{DocId, Handle, Principal};

use crate::MeetingId;

/// Snapshot label before the summary turn: "the agent rearranged my notes badly" is one restore.
pub const BEFORE_SUMMARY: &str = "meeting-before-summary";
/// Snapshot label after the summary turn.
pub const SUMMARY: &str = "meeting-summary";

/// The summary turn's contract for a meeting with a note (§7.4). Starts with
/// [`crate::summary::SUMMARY_HEAD`] so the actor matches the turn.
pub fn summary_form(doc: &DocId) -> String {
    format!(
        "{} The meeting has ended and the transcript was reviewed. The meeting's note is document \
         `{doc}`. First `doc_read` its outline. Regroup the user's blocks under headings where that \
         helps — `doc_move` them, never rewrite or delete a block the user wrote, keep their order \
         inside a heading. Then append, with `doc_insert(at=\"end\")`: `# <title>` (at most 8 words) \
         only if the note has no heading yet, **Summary** (at most 120 words), **Decisions** (with \
         who), **Action items** (owner — item — due if said), **Open questions**, **Numbers and names \
         said aloud**. Use `meeting_read` (all of it, in pieces) and `meeting_speakers` for the \
         transcript. Reply with the one-line title only.",
        crate::summary::SUMMARY_HEAD
    )
}

/// `meetings/<id>/summary.md` relative to the data dir: the note's derived path once summarized.
pub fn summary_path(meeting: MeetingId) -> String {
    format!("{}/{meeting}/{}", crate::DIR, crate::store::SUMMARY)
}

/// Create the meeting's note, headed `# <title>`; `None` when documents are off or refused.
pub async fn create(meeting: MeetingId, title: &str) -> Option<DocId> {
    let docs = Handle::lookup()?;
    match docs
        .actor()
        .ask(document::Create {
            binding: document::Binding::MeetingNote(meeting.to_string()),
            title: title.to_owned(),
            markdown: String::new(),
            by: Principal::Owner { device: None },
        })
        .await
    {
        Ok(s) => Some(s.id),
        Err(e) => {
            tracing::warn!(%meeting, "meeting note not created: {e}");
            None
        }
    }
}

fn docs() -> Result<Handle, String> {
    Handle::lookup().ok_or_else(|| "documents are not running".to_owned())
}

/// A labelled snapshot of the note.
pub async fn snapshot(doc: &DocId, label: &str) -> Result<(), String> {
    docs()?
        .actor()
        .ask(document::Snapshot {
            doc: doc.clone(),
            label: label.to_owned(),
        })
        .await
        .map_err(|e| e.to_string())
}

/// Import the newest [`BEFORE_SUMMARY`] snapshot back (a diff, §4.6); `Ok(false)` when there is none.
pub async fn restore_before_summary(doc: &DocId) -> Result<bool, String> {
    let docs = docs()?;
    let snaps = docs
        .actor()
        .ask(document::Snapshots { doc: doc.clone() })
        .await
        .map_err(|e| e.to_string())?;
    let Some(s) = snaps.into_iter().find(|s| s.label == BEFORE_SUMMARY) else {
        return Ok(false);
    };
    docs.actor()
        .ask(document::Restore {
            doc: doc.clone(),
            snapshot: s.name,
        })
        .await
        .map(|_| true)
        .map_err(|e| e.to_string())
}

/// The note's markdown as the helper renders it now.
pub async fn markdown(doc: &DocId) -> Result<String, String> {
    docs()?
        .actor()
        .ask(document::Markdown {
            doc: doc.clone(),
            blocks: None,
            annotate: false,
        })
        .await
        .map_err(|e| e.to_string())
}

/// Move the note's derived file to `summary.md` (written before this returns).
pub async fn set_summary_path(doc: &DocId, meeting: MeetingId) -> Result<(), String> {
    docs()?
        .actor()
        .ask(document::SetDerived {
            doc: doc.clone(),
            derived: summary_path(meeting),
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_names_the_document_and_starts_with_the_head() {
        let id = DocId::from_uuid(uuid::Uuid::nil());
        let f = summary_form(&id);
        assert!(f.starts_with(crate::summary::SUMMARY_HEAD));
        assert!(f.contains(&format!("`{id}`")));
        assert!(f.contains("doc_move") && f.contains("doc_insert(at=\"end\")"));
    }
}
