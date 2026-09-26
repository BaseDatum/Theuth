//! The task thread is a 59 document: comments, answers, notes and the
//! one-line event log are blocks appended at its end (67 §7.1).

use document::{Bind, Binding, BlockEdit, Create, DocId, Edit, Handle, Markdown, Principal};

/// Where thread text goes. One real implementation ([`DocThread`]); tests
/// keep blocks in memory.
#[async_trait::async_trait]
pub trait Thread: Send + Sync {
    /// New document bound to task `task`, titled `title` with `body`; its id, or `None` when
    /// documents are unavailable (the task still exists, without a thread).
    async fn create(&self, task: &str, title: &str, body: &str) -> Option<String>;
    /// Append `markdown` as blocks at the end.
    async fn append(&self, doc: &str, markdown: &str);
    /// The whole thread as markdown.
    async fn read(&self, doc: &str) -> Option<String>;
}

/// The document plugin, looked up per call so start order never matters.
pub struct DocThread;

#[async_trait::async_trait]
impl Thread for DocThread {
    async fn create(&self, task: &str, title: &str, body: &str) -> Option<String> {
        let h = Handle::lookup()?;
        match h
            .actor()
            .ask(Create {
                binding: Binding::TaskThread(task.to_owned()),
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

/// TASK-NOTES-HIDDEN: threads minted before this fix are `kind: note` and list as notes. Bind every
/// `tasks.thread` to its task; idempotent (an already-bound thread is a no-op), so it runs at each start.
pub async fn adopt_threads(pool: &sqlx::SqlitePool) {
    let Some(h) = Handle::lookup() else { return };
    let rows: Vec<(String, String)> = match sqlx::query_as("SELECT id, thread FROM tasks WHERE thread IS NOT NULL")
        .fetch_all(pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("task threads not adopted: {e}");
            return;
        }
    };
    for (task, doc) in rows {
        match h
            .actor()
            .ask(Bind {
                doc: DocId(doc.clone()),
                binding: Binding::TaskThread(task.clone()),
            })
            .await
        {
            Ok(true) => tracing::info!(task, doc, "task thread re-kinded from note"),
            Ok(false) => {}
            Err(e) => tracing::warn!(task, doc, "task thread not adopted: {e}"),
        }
    }
}
