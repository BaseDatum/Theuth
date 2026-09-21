//! The authority end to end against the compiled helper (design 59 §14 step 1
//! checks 2, 3, 6): create from a file, edit → export within 3 s, file edit →
//! diff import, restart → identical export. Skipped when the helper binary is
//! not built (`just documents-helper`).

use std::path::Path;
use std::time::{Duration, Instant};

use document::actor::DocumentActor;
use document::tools::{CreateTool, DeleteTool, Docs, InsertTool, ReadTool, ReplaceTool};
use document::{
    ApplyUpdate, Binding, BlockEdit, CreateNote, DOCUMENT_ACTOR, DeleteNote, DocEvent, DocKind,
    DocumentsConfig, Edit, EditError, Ensure, ImportMarkdown, ImportMode, ListDocs, OpenDoc, Outline,
    Principal, Rename, Subscribe, Tick, UndoLastAgentEdit, Who, helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use tools::{Tool, ToolCx};
use yrs::{Doc, Text, Transact};

/// Both tests register the one `DOCUMENT_ACTOR` name: one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
            "{} did not settle within {within:?}; now {:?}",
            path.display(),
            std::fs::read_to_string(path)
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
    let _serial = SERIAL.lock().await;
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

fn cx(session: uuid::Uuid) -> ToolCx {
    ToolCx {
        session: runtime::SessionId(session),
        call_id: String::new(),
        quiet: false,
        resources: std::sync::Arc::new(tools::SessionResources {
            root: std::env::temp_dir(),
            retention: tools::Retention::Durable,
            generation: 1,
        }),
    }
}

async fn next_event(sub: &mut tokio::sync::mpsc::Receiver<DocEvent>) -> DocEvent {
    tokio::time::timeout(Duration::from_secs(3), sub.recv())
        .await
        .expect("an event within 3 s")
        .expect("subscription open")
}

/// Design 59 §5 / §14 row 2: the agent edits through the tools, one update per call, stamped
/// `Agent{session}`, announced by `Authored`, revealed in the export, undone by the page menu.
#[tokio::test]
async fn agent_tools_edit_attribute_and_undo() {
    let _serial = SERIAL.lock().await;
    let _ = tracing_subscriber::fmt().with_env_filter("document=debug").with_test_writer().try_init();
    let Some(bin) = helper_bin() else {
        eprintln!("skipped: documents helper not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let derived = dir.path().join("knowledge/facts/b.md");
    std::fs::create_dir_all(derived.parent().unwrap()).unwrap();
    let md = "---\ntype: fact\ntitle: Bullets\n---\n## Heading\n\n* one\n* two\n* three\n";
    std::fs::write(&derived, md).unwrap();
    let actor = start(dir.path(), &bin).await;
    let session = uuid::Uuid::now_v7();
    let docs = Docs::new(actor.clone(), dir.path().to_path_buf());

    // `knowledge:<id>` resolves by creating the document from the file (§7.1).
    let read = ReadTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": "knowledge:facts/b" })).unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!read.is_error, "{}", read.content);
    let outline = read.content.clone();
    let lines: Vec<&str> = outline.lines().collect();
    assert!(lines[0].starts_with("[fm] type=fact title=\"Bullets\""), "{outline}");
    assert_eq!(lines.len(), 5, "{outline}");
    let id_of = |line: &str| line.split_whitespace().next().unwrap().to_owned();
    let two = id_of(lines[3]);
    assert!(lines[3].ends_with("• two"), "{outline}");
    let structured = read.structured.expect("structured");
    let blocks = structured["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 5);
    assert_eq!(blocks[3]["id"].as_str().unwrap(), two);
    assert_eq!(blocks[3]["type"], "bulletListItem");
    assert_eq!(structured["text"].as_str().unwrap(), outline);
    let id = actor
        .ask(document::Lookup {
            binding: Binding::Knowledge("facts/b".into()),
        })
        .await
        .unwrap()
        .expect("created on demand");
    let mut sub = actor
        .ask(Subscribe {
            doc: id.clone(),
            conn: Some(1),
        })
        .await
        .unwrap();
    // Wait out the on-demand creation's export so the pre-edit file is settled.
    let before = wait_for(&derived, |s| s.contains("* three"), Duration::from_secs(3)).await;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), sub.recv()).await {
        assert!(matches!(ev, DocEvent::Exported | DocEvent::Awareness { .. }), "{ev:?}");
    }

    // doc_replace: one update, Agent-stamped, Authored first, revealed ids = inserted ids.
    let r = ReplaceTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": id.as_str(), "blocks": [two], "markdown": "* X\n" }))
                .unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    let out = r.structured.unwrap();
    assert_eq!(out["removed"], serde_json::json!([two]));
    let x = out["inserted"][0].as_str().unwrap().to_owned();
    // Awareness (agent present), Authored, Update, then the agent's second awareness carries the blocks.
    let mut saw = Vec::new();
    for _ in 0..4 {
        saw.push(next_event(&mut sub).await);
    }
    let authored = saw
        .iter()
        .position(|e| matches!(e, DocEvent::Authored { .. }))
        .expect("Authored delivered");
    let update = saw
        .iter()
        .position(|e| matches!(e, DocEvent::Update { .. }))
        .expect("Update delivered");
    assert!(authored < update, "Authored precedes Update: {saw:?}");
    match &saw[authored] {
        DocEvent::Authored { blocks, by } => {
            assert_eq!(blocks, &vec![x.clone()]);
            assert_eq!(
                by,
                &Who::Agent {
                    session: session.to_string()
                }
            );
        }
        _ => unreachable!(),
    }
    match &saw[update] {
        DocEvent::Update { by, .. } => assert_eq!(
            by,
            &Who::Agent {
                session: session.to_string()
            }
        ),
        _ => unreachable!(),
    }
    assert!(saw.iter().any(|e| matches!(e, DocEvent::Awareness { .. })));
    let after = wait_for(&derived, |s| s.contains("* X"), Duration::from_secs(3)).await;
    assert_eq!(after, before.replace("* two", "* X"));
    // The log stamps the session (tag 2 + 16 bytes).
    let log = std::fs::read(dir.path().join(format!("documents/{id}/updates.log"))).unwrap();
    let mut tags = Vec::new();
    let mut at = 0;
    while at + 5 <= log.len() {
        let tag = log[at];
        at += 1;
        if tag == 2 {
            assert_eq!(&log[at..at + 16], session.as_bytes());
            at += 16;
        }
        let n = u32::from_be_bytes(log[at..at + 4].try_into().unwrap()) as usize;
        at += 4 + n;
        tags.push(tag);
    }
    assert!(tags.contains(&2), "{tags:?}");

    // doc_insert after X, doc_delete it again, doc_read shows each step.
    let r = InsertTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": id.as_str(), "markdown": "* Y\n", "after": x }))
                .unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    let y = r.structured.unwrap()["inserted"][0].as_str().unwrap().to_owned();
    let o = actor
        .ask(Outline {
            doc: id.clone(),
            depth: None,
        })
        .await
        .unwrap();
    let ol: Vec<&str> = o.outline.lines().collect();
    assert!(ol[3].ends_with("• X") && ol[4].ends_with("• Y") && ol[5].ends_with("• three"), "{}", o.outline);
    assert_eq!(id_of(ol[4]), y);
    let r = DeleteTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": id.as_str(), "blocks": [y] })).unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    let md_now = actor
        .ask(document::Markdown {
            doc: id.clone(),
            blocks: None,
            annotate: true,
        })
        .await
        .unwrap();
    assert!(md_now.contains(&format!("<!-- {x} -->\n* X\n")), "{md_now}");
    assert!(!md_now.contains("* Y"));

    // Errors the model can act on.
    let r = DeleteTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": id.as_str(), "blocks": ["nope"] })).unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(r.is_error);
    assert!(r.content.contains("no block `nope`") && r.content.contains("nearest ids"), "{}", r.content);
    let fm = blocks[0]["id"].as_str().unwrap();
    let e = actor
        .ask(Edit {
            doc: id.clone(),
            edits: vec![BlockEdit::Delete {
                blocks: vec![fm.to_owned()],
            }],
            by: Principal::Agent { session },
        })
        .await
        .unwrap_err();
    assert!(
        matches!(e, kameo::error::SendError::HandlerError(EditError::FrontmatterImmutable { .. })),
        "{e:?}"
    );
    assert!(e.to_string().contains("knowledge_put"), "{e}");

    // "Undo last agent edit" reverts the delete of Y (the last agent edit) as the owner.
    let u = actor
        .ask(UndoLastAgentEdit { doc: id.clone() })
        .await
        .unwrap();
    assert_eq!(u.inserted, vec![y.clone()]);
    let restored = wait_for(&derived, |s| s.contains("* Y"), Duration::from_secs(3)).await;
    assert_eq!(restored, before.replace("* two", "* X\n* Y"));
    let e = actor
        .ask(UndoLastAgentEdit { doc: id.clone() })
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("no agent edit to undo"), "{e}");
    // The undo's update is the owner's: no Authored, no reveal.
    let mut owner_update = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), sub.recv()).await {
        match ev {
            DocEvent::Update { by: Who::Owner, .. } => owner_update = true,
            DocEvent::Authored { by: Who::Owner, .. } => panic!("owner undo announced as authored"),
            _ => {}
        }
    }
    assert!(owner_update);

    // Byte-for-byte: undo a replace of X and the export equals the pre-edit file.
    let pre = std::fs::read_to_string(&derived).unwrap();
    let r = ReplaceTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "doc": id.as_str(), "blocks": [x], "markdown": "## Not a bullet\n\npara\n" }))
                .unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    wait_for(&derived, |s| s.contains("## Not a bullet"), Duration::from_secs(3)).await;
    actor
        .ask(UndoLastAgentEdit { doc: id.clone() })
        .await
        .unwrap();
    let back = wait_for(&derived, |s| !s.contains("Not a bullet"), Duration::from_secs(3)).await;
    assert_eq!(back, pre);

    // doc_create: a note with its derived file, listed by kind.
    let r = CreateTool(docs.clone())
        .call(
            serde_json::from_value(serde_json::json!({ "title": "Scratch", "markdown": "# Scratch\n\nhello\n" })).unwrap(),
            cx(session),
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    let out = r.structured.unwrap();
    let note = out["doc"].as_str().unwrap().to_owned();
    assert_eq!(out["url"], format!("theuth://notes/{note}"));
    let note_md = wait_for(
        &dir.path().join(format!("documents/{note}/note.md")),
        |s| s.contains("hello"),
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(note_md, "# Scratch\n\nhello\n");
    let notes = actor
        .ask(ListDocs {
            kind: Some(DocKind::Note),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id.as_str(), note);
    assert_eq!(notes[0].title, "Scratch");
    let all = actor.ask(ListDocs::default()).await.unwrap();
    assert_eq!(all.len(), 2);
    stop(actor).await;
}

/// Design 59 §7.3 / §14 row 3: notes from the UI side — created with a visible heading, listed newest
/// first, renamed in meta and in the first block, deleted into `.trash` with subscribers closed.
#[tokio::test]
async fn notes_create_list_rename_delete() {
    let _serial = SERIAL.lock().await;
    let _ = tracing_subscriber::fmt().with_env_filter("document=debug").with_test_writer().try_init();
    let Some(bin) = helper_bin() else {
        eprintln!("skipped: documents helper not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let actor = start(dir.path(), &bin).await;
    let owner = Principal::Owner { device: None };

    // No heading in the body → `# <title>` is inserted; an own heading is kept.
    let a = actor
        .ask(CreateNote { title: "First".into(), markdown: "para\n".into(), by: owner.clone() })
        .await
        .unwrap();
    assert_eq!((a.kind, a.title.as_str(), &a.binding), (DocKind::Note, "First", &Binding::Note));
    let a_md = dir.path().join(format!("documents/{}/note.md", a.id));
    assert_eq!(wait_for(&a_md, |s| s.contains("para"), Duration::from_secs(3)).await, "# First\n\npara\n");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let b = actor
        .ask(CreateNote { title: "Second".into(), markdown: "# Own heading\n\nbody\n".into(), by: owner.clone() })
        .await
        .unwrap();
    let b_md = dir.path().join(format!("documents/{}/note.md", b.id));
    assert_eq!(wait_for(&b_md, |s| s.contains("body"), Duration::from_secs(3)).await, "# Own heading\n\nbody\n");

    // Newest update first; an update to `a` moves it to the top.
    let ids = |v: Vec<document::DocSummary>| v.into_iter().map(|d| d.id).collect::<Vec<_>>();
    let list = actor.ask(ListDocs { kind: Some(DocKind::Note), limit: None }).await.unwrap();
    assert!(list.iter().all(|d| d.created_ms > 0 && d.updated_ms >= d.created_ms));
    assert_eq!(ids(list), vec![b.id.clone(), a.id.clone()]);
    tokio::time::sleep(Duration::from_millis(5)).await;
    actor
        .ask(ApplyUpdate { doc: a.id.clone(), update: scratch_update("x"), by: owner.clone() })
        .await
        .unwrap();
    let list = actor.ask(ListDocs { kind: Some(DocKind::Note), limit: None }).await.unwrap();
    assert_eq!(ids(list), vec![a.id.clone(), b.id.clone()]);

    // Rename: meta/index title and the heading block; `b`'s own heading is rewritten too (it is an h1).
    actor.ask(Rename { doc: a.id.clone(), title: "Renamed".into() }).await.unwrap();
    assert_eq!(wait_for(&a_md, |s| s.starts_with("# Renamed"), Duration::from_secs(3)).await, "# Renamed\n\npara\n");
    let list = actor.ask(ListDocs { kind: Some(DocKind::Note), limit: None }).await.unwrap();
    assert_eq!(list.iter().find(|d| d.id == a.id).unwrap().title, "Renamed");
    let e = actor.ask(Rename { doc: a.id.clone(), title: "  ".into() }).await.unwrap_err().to_string();
    assert!(e.contains("title must not be empty"), "{e}");

    // A knowledge document's title belongs to its frontmatter.
    std::fs::create_dir_all(dir.path().join("knowledge")).unwrap();
    let k = actor
        .ask(Ensure { binding: Binding::Knowledge("k".into()), markdown: MD.into() })
        .await
        .unwrap();
    let e = actor.ask(Rename { doc: k.clone(), title: "X".into() }).await.unwrap_err().to_string();
    assert!(e.contains("title_owned_by_binding"), "{e}");
    let e = actor.ask(DeleteNote { doc: k }).await.unwrap_err().to_string();
    assert!(e.contains("delete_via_owner"), "{e}");

    // Delete: subscribers see `Closed{deleted}`, the directory is in `.trash`, the list drops it.
    let mut sub = actor.ask(Subscribe { doc: b.id.clone(), conn: Some(1) }).await.unwrap();
    actor.ask(DeleteNote { doc: b.id.clone() }).await.unwrap();
    assert_eq!(next_event(&mut sub).await, DocEvent::Closed { reason: "deleted".into() });
    assert!(!b_md.exists());
    assert!(dir.path().join(format!("documents/.trash/{}/note.md", b.id)).is_file());
    let list = actor.ask(ListDocs { kind: Some(DocKind::Note), limit: None }).await.unwrap();
    assert_eq!(ids(list), vec![a.id.clone()]);
    // Loading the deleted note fails (its directory is gone from `documents/`).
    assert!(actor.ask(OpenDoc { doc: b.id.clone(), sv: None }).await.is_err());
    stop(actor).await;
}
