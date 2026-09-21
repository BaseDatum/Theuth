//! The authority end to end against the compiled helper (design 59 §14 step 1
//! checks 2, 3, 6): create from a file, edit → export within 3 s, file edit →
//! diff import, restart → identical export. Skipped when the helper binary is
//! not built (`just documents-helper`).

use std::path::Path;
use std::time::{Duration, Instant};

use document::actor::DocumentActor;
use document::{
    ApplyUpdate, Binding, DOCUMENT_ACTOR, DocEvent, DocKind, DocumentsConfig, Ensure,
    ImportMarkdown, ImportMode, OpenDoc, Principal, Subscribe, Tick, Who, helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use yrs::{Doc, Text, Transact};

const MD: &str = "---\ntype: fact\ntitle: A fact\ntags:\n  - x\n---\nhello there\n\nsecond block\n";

fn helper_bin() -> Option<std::path::PathBuf> {
    let p = helper::helper_path();
    p.is_file().then_some(p)
}

async fn start(data_dir: &Path, bin: &Path) -> ActorRef<DocumentActor> {
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.to_path_buf());
    let store = store::Store::open(data_dir).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = helper::Helper::new(bin.to_path_buf(), tx, rt.shutdown.clone());
    let cfg = DocumentsConfig {
        export_debounce_ms: 100,
        ..DocumentsConfig::default()
    };
    let actor = DocumentActor::spawn(DocumentActor::new(store, h, cfg, rt));
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

/// A Yjs update touching an unrelated root type: valid for the mirror, invisible to BlockNote.
fn scratch_update(s: &str) -> String {
    use base64::Engine as _;
    let doc = Doc::new();
    let t = doc.get_or_insert_text("scratch");
    let mut txn = doc.transact_mut();
    t.insert(&mut txn, 0, s);
    base64::engine::general_purpose::STANDARD.encode(txn.encode_update_v1())
}

async fn wait_for(path: &Path, pred: impl Fn(&str) -> bool, within: Duration) -> String {
    let start = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(path)
            && pred(&s)
        {
            return s;
        }
        assert!(
            start.elapsed() < within,
            "{} did not settle within {within:?}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn stop(actor: ActorRef<DocumentActor>) {
    actor.stop_gracefully().await.unwrap();
    actor.wait_for_shutdown().await;
}

#[tokio::test]
async fn knowledge_document_round_trip_export_import_restart() {
    let Some(bin) = helper_bin() else {
        eprintln!("skipped: documents helper not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let derived = dir.path().join("knowledge/facts/a.md");
    std::fs::create_dir_all(derived.parent().unwrap()).unwrap();
    std::fs::write(&derived, MD).unwrap();
    let actor = start(dir.path(), &bin).await;

    // Create on demand from the file (§7.1): no rewrite of the file.
    let id = actor
        .ask(Ensure {
            binding: Binding::Knowledge("facts/a".into()),
            markdown: MD.into(),
        })
        .await
        .unwrap();
    assert!(
        dir.path()
            .join(format!("documents/{id}/meta.json"))
            .is_file()
    );
    let again = actor
        .ask(Ensure {
            binding: Binding::Knowledge("facts/a".into()),
            markdown: MD.into(),
        })
        .await
        .unwrap();
    assert_eq!(again, id);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        std::fs::read_to_string(&derived).unwrap(),
        MD,
        "an import never rewrites the file"
    );

    let opened = actor
        .ask(OpenDoc {
            doc: id.clone(),
            sv: None,
        })
        .await
        .unwrap();
    assert_eq!(opened.meta.kind, DocKind::Knowledge);
    assert_eq!(opened.meta.title, "A fact");
    assert!(!opened.state.is_empty() && !opened.state[0].is_empty());
    assert_eq!(
        opened.meta.last_export.as_deref(),
        Some(store::sha256(MD.as_bytes()).as_str())
    );

    // An owner edit fans out to another connection and exports within the debounce (+ tick).
    let mut sub = actor
        .ask(Subscribe {
            doc: id.clone(),
            conn: Some(1),
        })
        .await
        .unwrap();
    let mut own = actor
        .ask(Subscribe {
            doc: id.clone(),
            conn: Some(2),
        })
        .await
        .unwrap();
    actor
        .ask(ApplyUpdate {
            doc: id.clone(),
            update: scratch_update("x"),
            by: Principal::Owner { device: Some(2) },
        })
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(1), sub.recv())
        .await
        .unwrap()
        .unwrap()
    {
        DocEvent::Update { by, .. } => assert_eq!(by, Who::Owner),
        other => panic!("{other:?}"),
    }
    let ev = tokio::time::timeout(Duration::from_secs(3), sub.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ev,
        DocEvent::Exported,
        "the sender's own update is not echoed, the export is"
    );
    let exported = std::fs::read_to_string(&derived).unwrap();
    assert!(
        exported.starts_with("---\ntype: fact\ntitle: A fact\ntags:\n  - x\n---\n"),
        "{exported}"
    );
    assert!(exported.contains("hello there") && exported.contains("second block"));
    // conn 2 (the author) saw only the export.
    assert_eq!(own.try_recv().unwrap(), DocEvent::Exported);

    // A foreign edit of the file: diff import, untouched block kept, `by: File`.
    let edited = exported.replace("hello there", "bye there");
    std::fs::write(&derived, &edited).unwrap();
    let r = actor
        .ask(ImportMarkdown {
            doc: id.clone(),
            markdown: edited.clone(),
            by: Principal::File,
            mode: ImportMode::Diff,
        })
        .await
        .unwrap();
    assert_eq!((r.changed, r.total, r.skipped), (1, 2, false));
    match tokio::time::timeout(Duration::from_secs(1), sub.recv())
        .await
        .unwrap()
        .unwrap()
    {
        DocEvent::Update { by, .. } => assert_eq!(by, Who::File),
        other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        std::fs::read_to_string(&derived).unwrap(),
        edited,
        "a file import does not re-export"
    );
    // The same text again is our own: skipped.
    let r = actor
        .ask(ImportMarkdown {
            doc: id.clone(),
            markdown: edited.clone(),
            by: Principal::File,
            mode: ImportMode::Diff,
        })
        .await
        .unwrap();
    assert!(r.skipped);

    // The next editor edit exports the merged document.
    actor
        .ask(ApplyUpdate {
            doc: id.clone(),
            update: scratch_update("y"),
            by: Principal::Owner { device: None },
        })
        .await
        .unwrap();
    let after = wait_for(
        &derived,
        |s| s.contains("bye there") && s.contains("second block"),
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(after, edited);

    // Restart the authority: snapshot + log reload to the same export.
    stop(actor).await;
    let actor = start(dir.path(), &bin).await;
    let reopened = actor
        .ask(OpenDoc {
            doc: id.clone(),
            sv: None,
        })
        .await
        .unwrap();
    assert_eq!(reopened.meta.title, "A fact");
    std::fs::remove_file(&derived).unwrap();
    actor
        .ask(ApplyUpdate {
            doc: id.clone(),
            update: scratch_update("z"),
            by: Principal::Owner { device: None },
        })
        .await
        .unwrap();
    let again = wait_for(&derived, |_| true, Duration::from_secs(3)).await;
    assert_eq!(again, after, "identical markdown before and after restart");

    // Refuse beyond the cap.
    let doc = Doc::new();
    let t = doc.get_or_insert_text("big");
    let mut txn = doc.transact_mut();
    t.insert(&mut txn, 0, &"a".repeat(9 * 1024 * 1024));
    let big = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(txn.encode_update_v1())
    };
    drop(txn);
    let err = actor
        .ask(ApplyUpdate {
            doc: id.clone(),
            update: big,
            by: Principal::Owner { device: None },
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("DocTooLarge"), "{err}");
    stop(actor).await;
}
