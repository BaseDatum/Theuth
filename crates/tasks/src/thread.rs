//! The task thread is a 59 document: comments, answers, notes and the
//! one-line event log are blocks appended at its end (67 §7.1).

use document::{BlockEdit, CreateNote, Edit, Handle, Markdown, Principal};

/// Where thread text goes. One real implementation ([`DocThread`]); tests
/// keep blocks in memory.
#[async_trait::async_trait]
pub trait Thread: Send + Sync {
    /// New document titled `title` with `body`; its id, or `None` when
    /// documents are unavailable (the task still exists, without a thread).
    async fn create(&self, title: &str, body: &str) -> Option<String>;
    /// Append `markdown` as blocks at the end.
    async fn append(&self, doc: &str, markdown: &str);
    /// The whole thread as markdown.
    async fn read(&self, doc: &str) -> Option<String>;
}

/// The document plugin, looked up per call so start order never matters.
pub struct DocThread;

#[async_trait::async_trait]
impl Thread for DocThread {
    async fn create(&self, title: &str, body: &str) -> Option<String> {
        let h = Handle::lookup()?;
        match h
            .actor()
            .ask(CreateNote {
                title: title.to_owned(),
                markdown: body.to_owned(),
                by: Principal::Owner { device: None },
            })
            .await
        {
            Ok(s) => Some(s.id.0),
            Err(e) => {
                tracing::warn!("task thread not created: {e}");
                None
            }
        }
    }

    async fn append(&self, doc: &str, markdown: &str) {
        let Some(h) = Handle::lookup() else { return };
        if let Err(e) = h
            .actor()
            .ask(Edit {
                doc: document::DocId(doc.to_owned()),
                edits: vec![BlockEdit::Insert {
                    markdown: markdown.to_owned(),
                    after: None,
                    before: None,
                    at: Some("end".to_owned()),
                }],
                by: Principal::Owner { device: None },
            })
            .await
        {
            tracing::warn!(doc, "task thread not appended: {e}");
        }
    }

    async fn read(&self, doc: &str) -> Option<String> {
        let h = Handle::lookup()?;
        h.actor()
            .ask(Markdown {
                doc: document::DocId(doc.to_owned()),
                blocks: None,
                annotate: false,
            })
            .await
            .ok()
    }
}
