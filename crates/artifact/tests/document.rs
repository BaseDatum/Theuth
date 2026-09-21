//! Design 59 §7.2 / §14 row 5 against the compiled documents helper: `show` a
//! markdown report twice → one document, two `artifact-v<N>` snapshots, two
//! version rows; a settled user edit → version 3 (`call_id = "edit:…"`,
//! `origin = None`) whose blob `Export` copies. Skipped when the helper binary
//! is not built (`just documents-helper`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use artifact::{ARTIFACT_ACTOR, ArtifactActor, Export, Kind, ShowArgs, State, Stores};
use document::actor::DocumentActor;
use document::{
    Binding, BlockEdit, DOCUMENT_ACTOR, DocumentsConfig, Edit, Lookup, Principal, Snapshots, Tick,
    helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use runtime::SessionId;

async fn start_documents(rt: &runtime::Runtime, bin: &std::path::Path) -> ActorRef<DocumentActor> {
    let store = store::Store::open(&rt.data_dir).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = helper::Helper::new(bin.to_path_buf(), tx, rt.shutdown.clone());
    let cfg = DocumentsConfig {
        export_debounce_ms: 100,
        ..DocumentsConfig::default()
    };
    let actor = DocumentActor::spawn(DocumentActor::new(store, h, cfg, rt.clone()));
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

async fn versions(state: &State, session: SessionId) -> Vec<artifact::Artifact> {
    let mut v = state.list(Some(session), false).await.unwrap();
    v.sort_by_key(|a| a.version);
    v
}

#[tokio::test]
async fn show_twice_then_edit_is_three_versions_of_one_document() {
    let bin = helper::helper_path();
    if !bin.is_file() {
        eprintln!("skipped: documents helper not built at {}", bin.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let docs = start_documents(&rt, &bin).await;
    let state = Arc::new(State::open(dir.path(), rt.clone(), Vec::new()).await.unwrap());
    let stores = Arc::new(Stores::new(state.clone()));
    let actor = ArtifactActor::spawn_with_mailbox(
        ArtifactActor::new(stores),
        runtime::subscriber_mailbox(),
    );
    actor.register(ARTIFACT_ACTOR).unwrap();
    rt.subscribe::<ArtifactActor, document::Exported>(&actor)
        .await;

    let session = SessionId::new();
    let show = |call: &str, text: &str| {
        let (call, text) = (call.to_owned(), text.to_owned());
        let state = state.clone();
        async move {
            state
                .show(
                    session,
                    call,
                    ShowArgs {
                        content: Some(text),
                        name: Some("report.md".into()),
                        ..ShowArgs::default()
                    },
                )
                .await
                .unwrap()
                .artifact
        }
    };
    let v1 = show("c1", "# Report\n\nfirst paragraph\n").await;
    assert_eq!((v1.kind, v1.version), (Kind::Markdown, 1));
    let binding = Binding::Artifact {
        session: session.to_string(),
        name: "report.md".into(),
    };
    let doc = docs
        .ask(Lookup {
            binding: binding.clone(),
        })
        .await
        .unwrap()
        .expect("the first show binds a document");

    let v2 = show("c2", "# Report\n\nfirst paragraph\n\nsecond paragraph\n").await;
    assert_eq!(v2.version, 2);
    assert_eq!(
        docs.ask(Lookup { binding }).await.unwrap(),
        Some(doc.clone()),
        "the second show imports into the same document"
    );
    let labels: Vec<String> = docs
        .ask(Snapshots { doc: doc.clone() })
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.label)
        .collect();
    assert!(labels.contains(&"artifact-v1".to_owned()), "{labels:?}");
    assert!(labels.contains(&"artifact-v2".to_owned()), "{labels:?}");
    assert_eq!(versions(&state, session).await.len(), 2);
    // A File import never exports: no edit version sneaks in behind the shows.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(versions(&state, session).await.len(), 2, "a show is not an edit");

    // The user types a paragraph (an owner update) → export → version 3 from the artifact store.
    docs.ask(Edit {
        doc: doc.clone(),
        edits: vec![BlockEdit::Insert {
            markdown: "third paragraph, typed".into(),
            after: None,
            before: None,
            at: Some("end".into()),
        }],
        by: Principal::Owner { device: None },
    })
    .await
    .unwrap();
    let t0 = Instant::now();
    let v3 = loop {
        let v = versions(&state, session).await;
        if let Some(a) = v.iter().find(|a| a.version == 3) {
            break a.clone();
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "no third version within 10 s: {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(v3.call_id.starts_with("edit:"), "{}", v3.call_id);
    assert_eq!(v3.origin, None);
    assert_eq!(v3.kind, Kind::Markdown);
    assert_eq!(v3.title, "Report");
    assert_ne!(v3.id, v2.id, "a new content-addressed blob");
    let dest = dir.path().join("out").join("report.md");
    let exported = actor
        .ask(Export {
            id: v3.id.clone(),
            dest: dest.clone(),
        })
        .await
        .unwrap();
    let text = std::fs::read_to_string(exported).unwrap();
    assert!(text.contains("third paragraph, typed"), "{text}");
    assert!(text.contains("second paragraph"), "{text}");
    let t0 = Instant::now();
    loop {
        let labels: Vec<String> = docs
            .ask(Snapshots { doc: doc.clone() })
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.label)
            .collect();
        if labels.contains(&"artifact-v3".to_owned()) {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "{labels:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Exporting an unchanged document again records nothing.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(versions(&state, session).await.len(), 3);

    docs.stop_gracefully().await.unwrap();
    docs.wait_for_shutdown().await;
}
