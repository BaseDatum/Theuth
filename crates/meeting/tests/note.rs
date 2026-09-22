//! Design 59 §7.4 / §14 row 5 against the compiled documents helper, without
//! the meeting actor (it needs the agent and Core Audio): the note module end
//! to end — created at start with `notes.md`, the user's three blocks, the
//! before-summary snapshot, a scripted summary turn (`doc_move` + `doc_insert`
//! as the agent), the after-summary snapshot, `notes.md` → `summary.md`, and
//! `post_retry`'s restore. Skipped when the helper is not built.

use std::time::{Duration, Instant};

use document::actor::DocumentActor;
use document::{
    BlockEdit, DOCUMENT_ACTOR, DocumentsConfig, Edit, Outline, Principal, Snapshots, Tick, helper,
    store,
};
use kameo::actor::{ActorRef, Spawn};
use meeting::note;

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

/// `(id, text)` of every top-level block.
async fn blocks(docs: &ActorRef<DocumentActor>, doc: &document::DocId) -> Vec<(String, String)> {
    docs.ask(Outline {
        doc: doc.clone(),
        depth: Some(1),
    })
    .await
    .unwrap()
    .blocks
    .iter()
    .map(|b| {
        (
            b["id"].as_str().unwrap().to_owned(),
            b["text"].as_str().unwrap_or_default().to_owned(),
        )
    })
    .collect()
}

async fn wait_for(path: &std::path::Path, pred: impl Fn(&str) -> bool) -> String {
    let t0 = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(path)
            && pred(&s)
        {
            return s;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "{} did not settle: {:?}",
            path.display(),
            std::fs::read_to_string(path)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn note_lives_through_start_summary_and_retry() {
    let bin = helper::helper_path();
    if !bin.is_file() {
        eprintln!("skipped: documents helper not built at {}", bin.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let docs = start_documents(&rt, &bin).await;
    let meeting = meeting::MeetingId::new();
    let mdir = dir.path().join("meetings").join(meeting.to_string());
    std::fs::create_dir_all(&mdir).unwrap();

    // meeting_start: the note exists and exports `notes.md` headed with the title.
    let doc = match note::create(meeting, "Meeting 2026-09-21 14:00").await {
        note::Created::Now(Some(doc)) => doc,
        note::Created::Now(None) => panic!("documents are running"),
        note::Created::Late(task) => task.await.unwrap().expect("documents are running"),
    };
    let notes_md = mdir.join("notes.md");
    let text = wait_for(&notes_md, |s| s.contains("# Meeting 2026-09-21 14:00")).await;
    assert!(text.trim().starts_with("# Meeting 2026-09-21 14:00"), "{text}");

    // The user types three lines during the meeting (owner updates).
    docs.ask(Edit {
        doc: doc.clone(),
        edits: vec![BlockEdit::Insert {
            markdown: "budget is 40k\n\nAna owns the rollout\n\nask legal about the DPA\n".into(),
            after: None,
            before: None,
            at: Some("end".into()),
        }],
        by: Principal::Owner { device: None },
    })
    .await
    .unwrap();
    let before = blocks(&docs, &doc).await;
    assert_eq!(before.len(), 4, "{before:?}");
    let user: Vec<(String, String)> = before[1..].to_vec();
    wait_for(&notes_md, |s| s.contains("ask legal about the DPA")).await;

    // Summarize: snapshot, then the scripted turn — regroup by moving, append the sections.
    note::snapshot(&doc, note::BEFORE_SUMMARY).await.unwrap();
    let session = uuid::Uuid::new_v4();
    docs.ask(Edit {
        doc: doc.clone(),
        edits: vec![BlockEdit::Insert {
            markdown: "## Notes\n".into(),
            after: Some(before[0].0.clone()),
            before: None,
            at: None,
        }],
        by: Principal::Agent { session },
    })
    .await
    .unwrap();
    // Ana's line goes last among the user's blocks: moved, not rewritten.
    docs.ask(Edit {
        doc: doc.clone(),
        edits: vec![BlockEdit::Move {
            blocks: vec![user[1].0.clone()],
            after: Some(user[2].0.clone()),
            before: None,
            at: None,
        }],
        by: Principal::Agent { session },
    })
    .await
    .unwrap();
    docs.ask(Edit {
        doc: doc.clone(),
        edits: vec![BlockEdit::Insert {
            markdown: "## Summary\n\nA short meeting about the rollout budget.\n\n## Decisions\n\n- Ana owns the rollout\n".into(),
            after: None,
            before: None,
            at: Some("end".into()),
        }],
        by: Principal::Agent { session },
    })
    .await
    .unwrap();
    note::snapshot(&doc, note::SUMMARY).await.unwrap();
    let markdown = note::markdown(&doc).await.unwrap();
    note::set_summary_path(&doc, meeting).await.unwrap();

    // The user's blocks: same ids, same text, regrouped; the sections follow.
    let after = blocks(&docs, &doc).await;
    let after_user: Vec<&(String, String)> = after.iter().filter(|b| user.iter().any(|u| u.0 == b.0)).collect();
    assert_eq!(after_user.len(), 3, "{after:?}");
    for u in &user {
        assert!(after.contains(u), "user block {u:?} changed: {after:?}");
    }
    assert_eq!(
        after_user.iter().map(|b| b.1.as_str()).collect::<Vec<_>>(),
        ["budget is 40k", "ask legal about the DPA", "Ana owns the rollout"]
    );
    let texts: Vec<&str> = after.iter().map(|b| b.1.as_str()).collect();
    assert!(texts.contains(&"[h2] Summary") && texts.contains(&"[h2] Decisions"), "{texts:?}");
    // summary.md is the note's export; notes.md is gone; meeting_summary would read exactly the markdown.
    let summary_md = mdir.join("summary.md");
    assert_eq!(std::fs::read_to_string(&summary_md).unwrap(), markdown);
    assert!(!notes_md.exists(), "notes.md should be gone");
    assert!(markdown.contains("## Summary") && markdown.contains("budget is 40k"), "{markdown}");
    let labels: Vec<String> = docs
        .ask(Snapshots { doc: doc.clone() })
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.label)
        .collect();
    assert!(labels.contains(&note::BEFORE_SUMMARY.to_owned()), "{labels:?}");
    assert!(labels.contains(&note::SUMMARY.to_owned()), "{labels:?}");

    // post_retry: back to the before-summary text; the user's untouched blocks keep their ids.
    assert!(note::restore_before_summary(&doc).await.unwrap());
    let restored = blocks(&docs, &doc).await;
    assert_eq!(
        restored.iter().map(|b| b.1.as_str()).collect::<Vec<_>>(),
        before.iter().map(|b| b.1.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(restored[0], before[0], "the heading never moved");
    assert_eq!(restored[1], before[1], "the first user block never moved");
    wait_for(&summary_md, |s| !s.contains("## Summary") && s.contains("ask legal")).await;

    docs.stop_gracefully().await.unwrap();
    docs.wait_for_shutdown().await;
}
