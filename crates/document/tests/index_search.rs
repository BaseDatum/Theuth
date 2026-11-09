//! The block index end to end (144 §7) against the compiled helper and the tiny random-weight
//! embedding checkpoint the test writes: notes index at block level, hybrid `nb_search` finds the
//! block (lexical ∪ vector, fused), the `notes` category pages title/body hits and skips deleted
//! notes, no model → FTS5-only with `nb_index_state.model = NULL`, a model change rebuilds the
//! vectors with FTS5 untouched, `DocumentChanged` re-indexes the changed blocks. Skipped loudly when
//! the helper binary is not built (`just documents-helper` or `THEUTH_DOCUMENTS_HELPER`).

use std::path::Path;
use std::time::Duration;

use decision::candle::embed::write_tiny_checkpoint;
use decision::embed::{EMBED_ACTOR, EmbedActor};
use document::actor::DocumentActor;
use document::index::{self, NbSearchArgs};
use document::search::SearchNotes;
use document::{
    CreateNote, DOCUMENT_ACTOR, DeleteNote, DocId, DocKind, DocumentChanged, DocumentsConfig,
    ImportMarkdown, ImportMode, Principal, Tick, helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use search::PageState;

/// Every test registers the one `DOCUMENT_ACTOR` / `EMBED_ACTOR` names: one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! helper_or_skip {
    () => {{
        let p = helper::helper_path();
        if !p.is_file() {
            eprintln!(
                "SKIPPED: no documents helper at {} — run `just documents-helper` or set THEUTH_DOCUMENTS_HELPER",
                p.display()
            );
            return;
        }
        p
    }};
}

/// The document actor over `data_dir` with `notebooks.embedding_model = model`.
async fn start(data_dir: &Path, bin: &Path, model: &str) -> ActorRef<DocumentActor> {
    let figment = figment::Figment::new().merge(figment::providers::Serialized::defaults(
        serde_json::json!({ "notebooks": { "embedding_model": model, "chunk_tokens": 12, "chunk_overlap": 0.25 } }),
    ));
    let rt = runtime::Runtime::new(figment, data_dir.to_path_buf());
    let vault = secrets::Vault::open(data_dir, secrets::KeyStore::File)
        .await
        .unwrap();
    let store = store::Store::open(data_dir)
        .await
        .unwrap()
        .with_vault(std::sync::Arc::new(vault));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = helper::Helper::new(bin.to_path_buf(), tx, rt.shutdown.clone());
    let cfg = DocumentsConfig {
        export_debounce_ms: 100,
        ..DocumentsConfig::default()
    };
    let actor = DocumentActor::spawn(DocumentActor::new(store, h, cfg, rt));
    kameo::registry::ACTOR_REGISTRY
        .lock()
        .unwrap()
        .remove(DOCUMENT_ACTOR);
    actor.register(DOCUMENT_ACTOR).unwrap();
    let a = actor.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = a.tell(ev).await;
        }
    });
    let a = actor.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(50));
        loop {
            t.tick().await;
            if a.tell(Tick).await.is_err() {
                break;
            }
        }
    });
    actor
}

/// The embed actor over `<data_dir>/models/<name>` (written tiny), registered as `embed`.
fn start_embed(data_dir: &Path, name: &str) -> ActorRef<EmbedActor> {
    let dir = decision::embed::model_dir(data_dir, name).unwrap();
    write_tiny_checkpoint(&dir).unwrap();
    let actor = EmbedActor::spawn(EmbedActor::new(dir, decision::candle::Gpu::None));
    kameo::registry::ACTOR_REGISTRY
        .lock()
        .unwrap()
        .remove(EMBED_ACTOR);
    actor.register(EMBED_ACTOR).unwrap();
    actor
}

fn unregister_embed() {
    kameo::registry::ACTOR_REGISTRY
        .lock()
        .unwrap()
        .remove(EMBED_ACTOR);
}

async fn stop(actor: ActorRef<DocumentActor>) {
    actor.stop_gracefully().await.unwrap();
    actor.wait_for_shutdown().await;
}

async fn note(actor: &ActorRef<DocumentActor>, title: &str, md: &str) -> DocId {
    actor
        .ask(CreateNote {
            title: title.into(),
            markdown: md.into(),
            by: Principal::Owner { device: None },
            home: document::Homing::Here,
        })
        .await
        .unwrap()
        .id
}

async fn search(actor: &ActorRef<DocumentActor>, q: &str) -> Vec<index::BlockHit> {
    index::nb_search(
        actor,
        NbSearchArgs {
            query: q.into(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// `nb_index_state` and `nb_vec` as the file holds them.
async fn state(data_dir: &Path) -> (Vec<(String, Option<String>)>, i64) {
    let db = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=ro",
        data_dir.join("documents/index.sqlite").display()
    ))
    .await
    .unwrap();
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT doc, model FROM nb_index_state ORDER BY doc")
            .fetch_all(&db)
            .await
            .unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM nb_vec")
        .fetch_one(&db)
        .await
        .unwrap();
    db.close().await;
    (rows, n)
}

const RUNBOOK: &str = "## Failover\n\nPromote the standby node when the primary heartbeat times out.\n\n\
Retry the leader election after the quorum is back.\n\n## Backup\n\nRestore from the backup replica.\n";

/// Round trip with the model: a note's blocks land in `nb_blocks`, chunks in `nb_vec` with the
/// model recorded; `nb_search` returns the block (id, heading, snippet, href) — the query equal
/// to a block's text ranks it first (lexical ∪ vector fused above lexical-only siblings); a
/// notebook scope that holds nothing finds nothing; the state records the model.
#[tokio::test]
async fn note_blocks_index_and_hybrid_search_hits_the_block() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let _embed = start_embed(dir.path(), "tiny");
    let actor = start(dir.path(), &bin, "tiny").await;
    let runbook = note(&actor, "Runbook", RUNBOOK).await;
    let other = note(&actor, "Groceries", "Promote the standby cheese.\n").await;
    index::reindex(&actor).await.unwrap();

    let hits = search(
        &actor,
        "Promote the standby node when the primary heartbeat times out.",
    )
    .await;
    assert!(!hits.is_empty());
    let top = &hits[0];
    assert_eq!(top.doc, runbook, "{hits:#?}");
    assert_eq!(top.kind, DocKind::Note);
    assert_eq!(top.title, "Runbook");
    assert_eq!(top.heading.as_deref(), Some("Failover"));
    assert!(!top.block.is_empty());
    assert_eq!(top.r#match.field, "text");
    assert!(top.r#match.snippet.contains("standby"), "{top:?}");
    assert!(!top.r#match.ranges.is_empty());
    let prefix = format!("theuth://notes/{}#{}?s=", runbook, top.block);
    assert!(
        top.href.starts_with(&prefix) && top.href.len() == prefix.len() + 8,
        "{}",
        top.href
    );
    assert!(
        hits.iter().any(|h| h.doc == other),
        "lexical-only sibling still listed"
    );
    assert!(hits.iter().all(|h| h.score > 0.0));

    // Scope: a notebook holding nothing → nothing; docs scope → that document only.
    let scoped = index::nb_search(
        &actor,
        NbSearchArgs {
            query: "standby".into(),
            notebooks: Some(vec![DocId::new()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(scoped.is_empty());
    let scoped = index::nb_search(
        &actor,
        NbSearchArgs {
            query: "standby".into(),
            docs: Some(vec![other.clone()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(scoped.iter().all(|h| h.doc == other) && !scoped.is_empty());

    let (rows, vectors) = state(dir.path()).await;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|(_, m)| m.as_deref() == Some("tiny")),
        "{rows:?}"
    );
    assert!(vectors >= 3, "chunks of two notes: {vectors}");
    stop(actor).await;
    unregister_embed();
}

/// The `notes` category: a title hit, a body hit at its block, a deleted note excluded, paging.
#[tokio::test]
async fn notes_category_pages_title_and_block_hits_and_skips_deleted() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    unregister_embed();
    let dir = tempfile::tempdir().unwrap();
    let actor = start(dir.path(), &bin, "absent").await;
    let groceries = note(&actor, "Groceries", "milk\n\neggs\n").await;
    let plan = note(&actor, "Plan", "buy groceries on the way home\n").await;
    let gone = note(&actor, "Groceries archive", "old groceries\n").await;
    index::reindex(&actor).await.unwrap();
    actor.ask(DeleteNote { doc: gone.clone() }).await.unwrap();
    index::on_document_changed(&DocumentChanged {
        doc: gone.clone(),
        kind: DocKind::Note,
        title: "Groceries archive".into(),
        updated_ms: 0,
        deleted: true,
    });
    // The hook runs off the mailbox: wait for the rows to go.
    for _ in 0..100 {
        if state(dir.path()).await.0.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let page = actor
        .ask(SearchNotes {
            query: "groceries".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(page.state, PageState::Ready, "{page:?}");
    let ids: Vec<&DocId> = page.hits.iter().map(|h| &h.doc.id).collect();
    assert!(
        ids.contains(&&groceries) && ids.contains(&&plan),
        "{page:?}"
    );
    assert!(!ids.contains(&&gone), "deleted note dropped: {page:?}");
    let title = page
        .hits
        .iter()
        .find(|h| h.doc.id == groceries && h.block.is_empty())
        .expect("title hit");
    assert_eq!(
        (title.r#match.field.as_str(), title.block.as_str()),
        ("title", "")
    );
    assert_eq!(title.href, format!("theuth://notes/{groceries}"));
    let body = page.hits.iter().find(|h| h.doc.id == plan).unwrap();
    assert_eq!(body.r#match.field, "text");
    assert!(!body.block.is_empty());
    assert!(body.r#match.snippet.contains("groceries"));
    assert!(!page.has_more);

    let page = actor
        .ask(SearchNotes {
            query: "groceries".into(),
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(page.hits.len(), 1);
    assert!(page.has_more);
    let blank = actor
        .ask(SearchNotes {
            query: "  ".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(blank.state, PageState::Ready);
    assert!(blank.hits.is_empty());

    // No model: FTS5-only, `nb_index_state.model` says so, nothing crashed.
    let (rows, vectors) = state(dir.path()).await;
    assert!(rows.iter().all(|(_, m)| m.is_none()), "{rows:?}");
    assert_eq!(vectors, 0);
    stop(actor).await;
}

/// §7.5: FTS5-only rows gain vectors once a model is installed; a different model re-embeds
/// every document (state names it) with the FTS5 rows untouched; `DocumentChanged` after an edit
/// re-indexes the changed blocks so the new text is found and the old is not.
#[tokio::test]
async fn model_change_rebuilds_vectors_and_edits_reindex_blocks() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    unregister_embed();
    let dir = tempfile::tempdir().unwrap();
    let actor = start(dir.path(), &bin, "tiny").await;
    let runbook = note(&actor, "Runbook", RUNBOOK).await;
    index::reindex(&actor).await.unwrap();
    let (rows, vectors) = state(dir.path()).await;
    assert_eq!((rows[0].1.as_deref(), vectors), (None, 0), "no model yet");
    assert_eq!(
        search(&actor, "heartbeat").await[0].doc,
        runbook,
        "lexical works"
    );

    let embed = start_embed(dir.path(), "tiny");
    index::reindex(&actor).await.unwrap();
    let (rows, vectors) = state(dir.path()).await;
    assert_eq!(rows[0].1.as_deref(), Some("tiny"));
    assert!(vectors > 0);
    embed.stop_gracefully().await.unwrap();
    embed.wait_for_shutdown().await;
    stop(actor).await;

    // A different checkpoint name at start → every row stale → re-embedded under the new name.
    let _embed = start_embed(dir.path(), "tiny2");
    let actor = start(dir.path(), &bin, "tiny2").await;
    index::reindex(&actor).await.unwrap();
    let (rows, vectors2) = state(dir.path()).await;
    assert_eq!(rows[0].1.as_deref(), Some("tiny2"));
    assert_eq!(vectors2, vectors, "same chunks, new model");
    assert_eq!(
        search(&actor, "heartbeat").await[0].doc,
        runbook,
        "FTS5 untouched"
    );

    // Edit → DocumentChanged → the changed block re-indexes.
    actor
        .ask(ImportMarkdown {
            doc: runbook.clone(),
            markdown: RUNBOOK.replace("heartbeat", "latency"),
            by: Principal::Owner { device: None },
            mode: ImportMode::Diff,
        })
        .await
        .unwrap();
    index::on_document_changed(&DocumentChanged {
        doc: runbook.clone(),
        kind: DocKind::Note,
        title: "Runbook".into(),
        updated_ms: 0,
        deleted: false,
    });
    let mut found = false;
    for _ in 0..100 {
        let hits = search(&actor, "latency").await;
        if hits
            .iter()
            .any(|h| h.doc == runbook && h.r#match.snippet.contains("latency"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(found, "the edited block is found");
    assert!(
        search(&actor, "heartbeat")
            .await
            .iter()
            .all(|h| !h.r#match.snippet.contains("heartbeat")),
        "the old text is gone"
    );
    stop(actor).await;
    unregister_embed();
}
