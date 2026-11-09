//! The task thread is a 59 document: comments, answers, notes and the
//! one-line event log are blocks appended at its end (67 §7.1).

use document::{
    Bind, Binding, BlockEdit, Create, DocHeadings, DocId, Edit, Handle, Headings, Homing, Markdown,
    MoveHome, Principal,
};

/// Where thread text goes. One real implementation ([`DocThread`]); tests
/// keep blocks in memory.
#[async_trait::async_trait]
pub trait Thread: Send + Sync {
    /// New document bound to task `task`, titled `title` with `body`, homed on the task's `home`
    /// (184 §5.1; `None` = hub-served); its id, or `None` when documents are unavailable (the task
    /// still exists, without a thread).
    async fn create(
        &self,
        task: &str,
        title: &str,
        body: &str,
        home: Option<&str>,
    ) -> Option<String>;
    /// 184 §4.8: the thread follows its task to `home` (after `move_home` saved the task).
    async fn set_home(&self, doc: &str, home: &str);
    /// Append `markdown` as blocks at the end.
    async fn append(&self, doc: &str, markdown: &str);
    /// The whole thread as markdown.
    async fn read(&self, doc: &str) -> Option<String>;
    /// The kind and top-level `(level, text)` headings of document `doc` — what a packet's
    /// mini-board is checked against (101 R2); `Ok(None)` when no such document exists,
    /// `Err` when documents are unavailable.
    async fn headings(&self, doc: &str) -> Result<Option<DocHeadings>, String>;
}

/// The document plugin, looked up per call so start order never matters.
pub struct DocThread;

#[async_trait::async_trait]
impl Thread for DocThread {
    async fn create(
        &self,
        task: &str,
        title: &str,
        body: &str,
        home: Option<&str>,
    ) -> Option<String> {
        let h = Handle::lookup()?;
        match h
            .actor()
            .ask(Create {
                binding: Binding::TaskThread(task.to_owned()),
                title: title.to_owned(),
                markdown: body.to_owned(),
                by: Principal::Owner { device: None },
                home: home.map_or(Homing::Hub, |e| {
                    Homing::Engine(mesh::EngineId(e.to_owned()))
                }),
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

    async fn set_home(&self, doc: &str, home: &str) {
        let Some(h) = Handle::lookup() else { return };
        if let Err(e) = h
            .actor()
            .ask(MoveHome {
                doc: DocId(doc.to_owned()),
                to: mesh::EngineId(home.to_owned()),
                checks: false,
            })
            .await
        {
            tracing::warn!(doc, "task thread did not follow its task: {e}");
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

    async fn headings(&self, doc: &str) -> Result<Option<DocHeadings>, String> {
        let Some(h) = Handle::lookup() else {
            return Err("documents are not running; the board cannot be read".into());
        };
        let Some(id) = DocId::parse(doc) else {
            return Ok(None);
        };
        h.actor().ask(Headings(id)).await.map_err(|e| e.to_string())
    }
}

/// TASK-NOTES-HIDDEN: threads minted before this fix are `kind: note` and list as notes. Bind every
/// `tasks.thread` to its task; idempotent (an already-bound thread is a no-op), so it runs at each start.
pub async fn adopt_threads(pool: &sqlx::SqlitePool) {
    let Some(h) = Handle::lookup() else { return };
    let rows: Vec<(String, String)> =
        match sqlx::query_as("SELECT id, thread FROM tasks WHERE thread IS NOT NULL")
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

#[cfg(test)]
mod tests {
    use document::{AgentStyle, DocKind, DocMeta, DocumentActor, DocumentsConfig};
    use kameo::actor::Spawn;

    use super::*;

    fn meta(kind: DocKind) -> DocMeta {
        let id = DocId::new();
        DocMeta {
            derived: Binding::Note.derived(&id),
            id,
            kind,
            title: "t".into(),
            binding: Binding::Note,
            fragment: document::FRAGMENT.into(),
            members: vec![],
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        }
    }

    /// 101 R2, the actor hop: `DocThread::headings` reaches the registered document actor,
    /// which answers the kind from its store and the headings from the helper's outline
    /// (a `sh` helper with one canned tree); an unknown or malformed id is `None`.
    #[tokio::test]
    async fn headings_reach_the_document_actor() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("helper.sh");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
if [ "$1" = --version ]; then echo "theuth-documents {}"; exit 0; fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"op":"outline"'*) printf '{{"id":%s,"ok":true,"result":{{"outline":"","blocks":[{{"id":"t","type":"heading","text":"[h1] T","children":[]}},{{"id":"a","type":"heading","text":"[h2] What was done","children":[]}},{{"id":"p","type":"paragraph","text":"x","children":[]}}]}}}}\n' "$id" ;;
    *) printf '{{"id":%s,"ok":true,"result":{{}}}}\n' "$id" ;;
  esac
done
"#,
                document::protocol_version()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let store = document::store::Store::open(dir.path()).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = document::helper::Helper::new(script, tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            DocumentsConfig::default(),
            rt,
        ));
        actor.register(document::DOCUMENT_ACTOR).unwrap();
        let board = meta(DocKind::Board);
        let note = meta(DocKind::Note);
        store.create(&board).await.unwrap();
        store.create(&note).await.unwrap();

        assert_eq!(
            DocThread.headings(board.id.as_str()).await,
            Ok(Some(DocHeadings {
                kind: DocKind::Board,
                headings: vec![(1, "T".into()), (2, "What was done".into())],
            }))
        );
        assert_eq!(
            DocThread
                .headings(note.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .kind,
            DocKind::Note
        );
        assert_eq!(DocThread.headings(DocId::new().as_str()).await, Ok(None));
        assert_eq!(DocThread.headings("nope").await, Ok(None));
    }
}
