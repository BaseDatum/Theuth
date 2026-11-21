//! The ingest job end to end against the compiled helper (144 §4.2): pre-flight refusals on the
//! card, `new → queued → running → completed | failed`, dedupe by origin key, refresh freshness,
//! the routes that need no network (text, PDF, meeting) and the ones that fail cleanly without
//! their service (audio without the voice plugin, session without a registry). Skipped when the
//! helper binary is not built (`just documents-helper`).

use std::path::Path;
use std::time::{Duration, Instant};

use document::actor::DocumentActor;
use document::ingest::{
    self, Active, INGEST_ACTOR, IngestActor, NotebooksConfig, SourceAddArgs, YoutubeRouteCfg,
};
use document::{
    DOCUMENT_ACTOR, DocId, DocKind, DocumentsConfig, ItemKind, Markdown, Principal, SourceOrigin,
    SourceStatus, Tick, helper, store,
};
use kameo::actor::{ActorRef, Spawn};

/// Every test registers the one `DOCUMENT_ACTOR` / `INGEST_ACTOR` pair: one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ingest");

/// The helper path `Helper::new` still takes (ignored: the helper runs in-process); never skips.
macro_rules! helper_or_skip {
    () => {{ helper::helper_path() }};
}

struct Rig {
    docs: ActorRef<DocumentActor>,
    ingest: ActorRef<IngestActor>,
    store: store::Store,
    notebook: DocId,
}

async fn start(data_dir: &Path, bin: &Path, cfg: NotebooksConfig) -> Rig {
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.to_path_buf());
    let vault = secrets::Vault::open(data_dir, secrets::KeyStore::File)
        .await
        .unwrap();
    let store = store::Store::open(data_dir)
        .await
        .unwrap()
        .with_vault(std::sync::Arc::new(vault));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = helper::Helper::new(bin.to_path_buf(), tx, rt.shutdown.clone());
    let dcfg = DocumentsConfig {
        export_debounce_ms: 100,
        ..DocumentsConfig::default()
    };
    let docs = DocumentActor::spawn(DocumentActor::new(store.clone(), h, dcfg, rt.clone()));
    {
        let mut reg = kameo::registry::ACTOR_REGISTRY.lock().unwrap();
        reg.remove(DOCUMENT_ACTOR);
        reg.remove(INGEST_ACTOR);
    }
    docs.register(DOCUMENT_ACTOR).unwrap();
    let a = docs.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = a.tell(ev).await;
        }
    });
    let a = docs.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(50));
        loop {
            t.tick().await;
            if a.tell(Tick).await.is_err() {
                break;
            }
        }
    });
    let ingest = IngestActor::start_with(&rt, store.clone(), docs.clone(), cfg).unwrap();
    // A notebook row (the container is C's; the ingest only needs the row to exist).
    let notebook = DocId::new();
    store
        .notebook_upsert(&store::NotebookRow {
            id: notebook.clone(),
            project: None,
            archived_ms: None,
        })
        .await
        .unwrap();
    Rig {
        docs,
        ingest,
        store,
        notebook,
    }
}

fn args(notebook: &DocId) -> SourceAddArgs {
    SourceAddArgs {
        notebook: notebook.clone(),
        ..SourceAddArgs::default()
    }
}

const OWNER: Principal = Principal::Owner { device: None };

async fn settled(docs: &ActorRef<DocumentActor>, id: &DocId) -> document::Source {
    let start = Instant::now();
    loop {
        let s = ingest::source_get(docs, id.clone()).await.unwrap();
        if matches!(s.status, SourceStatus::Completed | SourceStatus::Failed) {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "ingest did not settle: {s:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The title follows the `# ` heading at export (after the debounce): wait for it.
async fn titled(docs: &ActorRef<DocumentActor>, id: &DocId, title: &str) {
    let start = Instant::now();
    loop {
        let s = ingest::source_get(docs, id.clone()).await.unwrap();
        if s.doc.title == title {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "title stayed {:?}, wanted {title:?}",
            s.doc.title
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn markdown(docs: &ActorRef<DocumentActor>, id: &DocId) -> String {
    docs.ask(Markdown {
        doc: id.clone(),
        blocks: None,
        annotate: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn pasted_text_runs_to_completed_and_refresh_tracks_freshness() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), &bin, NotebooksConfig::default()).await;

    let text = "# Failover notes\n\nThe primary flapped twice.\n\n## Fix\n\nAdd a health check.\n";
    let t = Instant::now();
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            text: Some(text.into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert!(
        matches!(
            s.status,
            SourceStatus::Queued | SourceStatus::Running | SourceStatus::Completed
        ),
        "{:?}",
        s.status
    );
    assert_eq!(s.doc.kind, DocKind::Source);
    assert_eq!(s.doc.title, "Failover notes");
    assert!(matches!(&s.origin, SourceOrigin::Text { title, .. } if title == "Failover notes"));
    let done = settled(&rig.docs, &s.doc.id).await;
    eprintln!("text ingest settled in {:?}", t.elapsed());
    assert_eq!(done.status, SourceStatus::Completed, "{:?}", done.error);
    assert_eq!(done.attempts, 1);
    assert_eq!(done.words, Some(13));
    assert!(
        done.fetched_ms.is_some() && done.content_sha.is_some() && done.changed_since.is_none()
    );
    let md = markdown(&rig.docs, &s.doc.id).await;
    assert!(md.contains("Add a health check."), "{md}");
    assert!(rig.ingest.ask(Active).await.unwrap().is_empty());

    // Filed in the notebook; listed for it and for everyone.
    let items = rig.store.items(&rig.notebook).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (
            items[0].kind,
            items[0].item.as_str(),
            items[0].added_by.as_str()
        ),
        (ItemKind::Source, s.doc.id.as_str(), "owner")
    );
    assert_eq!(
        ingest::source_list(&rig.docs, Some(rig.notebook.clone()))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ingest::source_list(&rig.docs, None).await.unwrap()[0]
            .doc
            .id,
        s.doc.id
    );

    // Refresh with the same bytes: a second attempt, same sha, nothing "changed since".
    let r = ingest::source_refresh(&rig.docs, s.doc.id.clone(), OWNER)
        .await
        .unwrap();
    assert!(matches!(
        r.status,
        SourceStatus::Queued | SourceStatus::Running | SourceStatus::Completed
    ));
    let again = settled(&rig.docs, &s.doc.id).await;
    assert_eq!((again.status, again.attempts), (SourceStatus::Completed, 2));
    assert_eq!(again.content_sha, done.content_sha);
    assert!(again.changed_since.is_none());

    // The origin changes (the blob is rewritten): Diff import, `changed_since` = the previous fetch.
    let blob = rig
        .store
        .source_row(&s.doc.id)
        .await
        .unwrap()
        .unwrap()
        .blob
        .unwrap();
    std::fs::write(
        &blob,
        "# Failover notes\n\nThe primary flapped three times.\n",
    )
    .unwrap();
    ingest::source_refresh(&rig.docs, s.doc.id.clone(), OWNER)
        .await
        .unwrap();
    let changed = settled(&rig.docs, &s.doc.id).await;
    assert_eq!(
        (changed.status, changed.attempts),
        (SourceStatus::Completed, 3)
    );
    assert_ne!(changed.content_sha, done.content_sha);
    assert_eq!(changed.changed_since, again.fetched_ms);
    assert!(markdown(&rig.docs, &s.doc.id).await.contains("three times"));

    // Refresh of an unknown source is an error, not a row.
    assert!(
        ingest::source_refresh(&rig.docs, DocId::new(), OWNER)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn preflight_refusals_land_on_the_card_without_a_job() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let cfg = NotebooksConfig {
        max_source_bytes: 2_000,
        youtube_route: YoutubeRouteCfg::Audio,
        youtube_ytdlp: Some("/nonexistent/yt-dlp".into()),
        ..NotebooksConfig::default()
    };
    let rig = start(dir.path(), &bin, cfg).await;

    let odd = dir.path().join("thing.xyz");
    std::fs::write(&odd, b"??").unwrap();
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            path: Some(odd.to_string_lossy().into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(s.status, SourceStatus::Failed);
    assert_eq!(s.error.as_deref(), Some("415 unsupported: xyz"));
    assert!(
        matches!(&s.origin, SourceOrigin::File { ext, name, .. } if ext == "xyz" && name == "thing.xyz")
    );

    let big = dir.path().join("big.md");
    std::fs::write(&big, "x".repeat(5_000)).unwrap();
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            path: Some(big.to_string_lossy().into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(s.status, SourceStatus::Failed);
    assert!(
        s.error.as_deref().unwrap().starts_with("too large: "),
        "{:?}",
        s.error
    );

    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            url: Some("http://127.0.0.1:8080/secret".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(s.status, SourceStatus::Failed);
    assert!(
        s.error
            .as_deref()
            .unwrap()
            .starts_with("refused: private address"),
        "{:?}",
        s.error
    );
    assert!(
        matches!(&s.origin, SourceOrigin::Url { canonical: Some(c), .. } if c.contains("127.0.0.1"))
    );

    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            url: Some("https://youtu.be/dQw4w9WgXcQ".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(s.status, SourceStatus::Failed);
    assert!(
        s.error
            .as_deref()
            .unwrap()
            .starts_with("no transcript route"),
        "{:?}",
        s.error
    );
    assert!(matches!(&s.origin, SourceOrigin::Youtube { video, .. } if video == "dQw4w9WgXcQ"));

    // Four cards, no job ran, all filed.
    assert!(rig.ingest.ask(Active).await.unwrap().is_empty());
    assert_eq!(rig.store.items(&rig.notebook).await.unwrap().len(), 4);
    let all = ingest::source_list(&rig.docs, None).await.unwrap();
    assert_eq!(all.len(), 4);
    assert!(
        all.iter()
            .all(|s| s.status == SourceStatus::Failed && s.attempts == 0)
    );

    // Bad arguments are errors, not cards.
    assert!(
        ingest::source_add(&rig.docs, args(&rig.notebook), OWNER)
            .await
            .is_err()
    );
    assert!(
        ingest::source_add(
            &rig.docs,
            SourceAddArgs {
                text: Some("x".into()),
                ..args(&DocId::new())
            },
            OWNER
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("no such notebook")
    );
}

#[tokio::test]
async fn pdf_and_meeting_routes_complete_on_fixtures() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), &bin, NotebooksConfig::default()).await;

    let t = Instant::now();
    let pdf = format!("{FIXTURES}/two-pages.pdf");
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            path: Some(pdf),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    let done = settled(&rig.docs, &s.doc.id).await;
    eprintln!("pdf ingest settled in {:?}", t.elapsed());
    assert_eq!(done.status, SourceStatus::Completed, "{:?}", done.error);
    assert_eq!(done.pages, Some(2));
    assert_eq!(done.bytes, Some(859));
    assert_eq!(done.doc.title, "two-pages.pdf");
    let md = markdown(&rig.docs, &s.doc.id).await;
    assert!(
        md.contains("Hello notebooks") && md.contains("Second page here"),
        "{md}"
    );
    // The original was copied into the attachment store.
    let blob = rig
        .store
        .source_row(&s.doc.id)
        .await
        .unwrap()
        .unwrap()
        .blob
        .unwrap();
    assert!(
        blob.starts_with(dir.path().join("attachments").to_str().unwrap()),
        "{blob}"
    );
    assert!(Path::new(&blob).is_file());

    let mdir = dir
        .path()
        .join("meetings/0199a000-0000-7000-8000-000000000001");
    std::fs::create_dir_all(&mdir).unwrap();
    for f in ["manifest.json", "transcript.json"] {
        std::fs::copy(format!("{FIXTURES}/{f}"), mdir.join(f)).unwrap();
    }
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            meeting: Some("0199a000-0000-7000-8000-000000000001".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(s.doc.title, "Meeting 0199a000-0000-7000-8000-000000000001");
    let done = settled(&rig.docs, &s.doc.id).await;
    assert_eq!(done.status, SourceStatus::Completed, "{:?}", done.error);
    titled(&rig.docs, &s.doc.id, "Failover sync").await;
    assert_eq!(done.duration_ms, Some(1_800_000));
    assert_eq!(done.meta["segments"].as_array().unwrap().len(), 3);
    let md = markdown(&rig.docs, &s.doc.id).await;
    assert!(md.contains("## [00:04] Ana"), "{md}");
    assert!(
        ingest::source_add(
            &rig.docs,
            SourceAddArgs {
                meeting: Some("nope".into()),
                ..args(&rig.notebook)
            },
            OWNER
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn same_origin_is_one_document_in_two_notebooks_and_a_caller_title_sticks() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), &bin, NotebooksConfig::default()).await;
    let other = DocId::new();
    rig.store
        .notebook_upsert(&store::NotebookRow {
            id: other.clone(),
            project: None,
            archived_ms: None,
        })
        .await
        .unwrap();

    let a = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            text: Some("same paste".into()),
            title: Some("My paste".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    let done = settled(&rig.docs, &a.doc.id).await;
    assert_eq!(
        (done.status, done.doc.title.as_str()),
        (SourceStatus::Completed, "My paste")
    );
    let b = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            text: Some("same paste".into()),
            ..args(&other)
        },
        Principal::Agent {
            session: uuid::Uuid::nil(),
        },
    )
    .await
    .unwrap();
    assert_eq!(b.doc.id, a.doc.id, "one document per origin key");
    assert_eq!(
        b.status,
        SourceStatus::Completed,
        "a finished source is reused, not re-run"
    );
    assert_eq!(
        rig.store.items(&other).await.unwrap()[0].added_by,
        format!("agent:{}", uuid::Uuid::nil())
    );
    assert_eq!(
        ingest::source_list(&rig.docs, Some(other))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(ingest::source_list(&rig.docs, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn routes_without_their_service_fail_transiently_and_can_be_refreshed() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), &bin, NotebooksConfig::default()).await;

    let wav = format!("{FIXTURES}/tone-3s.wav");
    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            path: Some(wav),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert!(matches!(&s.origin, SourceOrigin::Audio { ext, .. } if ext == "wav"));
    let done = settled(&rig.docs, &s.doc.id).await;
    assert_eq!(done.status, SourceStatus::Failed);
    let e = done.error.unwrap();
    assert!(
        e.contains("speech-to-text is not running") && e.ends_with("(transient; refresh to retry)"),
        "{e}"
    );
    assert_eq!(done.attempts, 1);

    let s = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            session: Some("0199-some-session".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    let done = settled(&rig.docs, &s.doc.id).await;
    assert_eq!(done.status, SourceStatus::Failed);
    assert!(
        done.error.as_deref().unwrap().contains("no tool registry"),
        "{:?}",
        done.error
    );

    // A failed card added again runs again.
    let again = ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            session: Some("0199-some-session".into()),
            ..args(&rig.notebook)
        },
        OWNER,
    )
    .await
    .unwrap();
    assert_eq!(again.doc.id, s.doc.id);
    let done = settled(&rig.docs, &s.doc.id).await;
    assert_eq!(done.attempts, 2);
}
