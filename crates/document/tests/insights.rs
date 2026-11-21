//! 144 §4.3 (E1) against the compiled helper: the default `Dense summary` at the end of an ingest
//! (appended as `Principal::Helper`, counted, `Changed{Completed}` emitted, never a second one on a
//! same-text refresh, a failed call recorded as `insight_error`), `insight_run` with a builtin and a
//! skill, `insight_promote` into a note with the trailer and the membership. The model call and the
//! skill lookup are the installed hooks — mocked here the way wave 1 mocked `title()`. Skipped when the
//! helper binary is not built (`just documents-helper` or `THEUTH_DOCUMENTS_HELPER`).

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::{Duration, Instant};

use document::actor::DocumentActor;
use document::ingest::insights::{
    self, DEFAULT_PROMPT, Hooks, InsightCall, InsightReply, install, section_of,
};
use document::ingest::{INGEST_ACTOR, IngestActor, NotebooksConfig, SourceAddArgs};
use document::{
    DOCUMENT_ACTOR, DocId, DocumentsConfig, InsightInstruction, ItemKind, Markdown, Principal,
    SourceEvent, SourceStatus, Tick, helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CALLS: Mutex<Vec<InsightCall>> = Mutex::new(Vec::new());

/// The helper path `Helper::new` still takes (ignored: the helper runs in-process); never skips.
macro_rules! helper_or_skip {
    () => {{ helper::helper_path() }};
}

/// The mocked seams: the model echoes the instruction (and fails on a source that says `FAIL`); one
/// insight skill, `legal-review`.
fn mock() {
    install(Hooks {
        side_call: Arc::new(|call| {
            Box::pin(async move {
                let fail = call.user.contains("FAIL");
                let system = call.system.clone();
                CALLS.lock().push(call);
                if fail {
                    return Err("provider said no".to_owned());
                }
                Ok(InsightReply {
                    text: format!("Summary of: {system}\n\n- point"),
                    model: "mock/model".into(),
                })
            })
        }),
        skill: Arc::new(|name| match name {
            "legal-review" => Ok(("Legal review".into(), "Review the legal exposure".into())),
            other => Err(format!("skill `{other}` is not an insight skill")),
        }),
    });
}

/// Collects every `SourceEvent` the runtime publishes.
#[derive(kameo::Actor)]
struct Collector(Arc<Mutex<Vec<SourceEvent>>>);

impl Message<SourceEvent> for Collector {
    type Reply = ();

    async fn handle(&mut self, ev: SourceEvent, _: &mut Context<Self, Self::Reply>) {
        self.0.lock().push(ev);
    }
}

struct Rig {
    docs: ActorRef<DocumentActor>,
    store: store::Store,
    notebook: DocId,
    events: Arc<Mutex<Vec<SourceEvent>>>,
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
    let events = Arc::new(Mutex::new(Vec::new()));
    let collector = Collector::spawn(Collector(events.clone()));
    rt.subscribe::<Collector, SourceEvent>(&collector).await;
    IngestActor::start_with(&rt, store.clone(), docs.clone(), cfg).unwrap();
    let notebook = new_notebook(&docs, "DR").await;
    Rig {
        docs,
        store,
        notebook,
        events,
    }
}

/// A real `DocKind::Notebook` document + row (`notebooks_of` joins `docs`).
async fn new_notebook(docs: &ActorRef<DocumentActor>, title: &str) -> DocId {
    docs.ask(document::NotebookCreate {
        title: title.into(),
        description: String::new(),
        project: None,
        by: OWNER,
    })
    .await
    .unwrap()
    .doc
    .id
}

const OWNER: Principal = Principal::Owner { device: None };

async fn add_text(rig: &Rig, text: &str) -> DocId {
    document::ingest::source_add(
        &rig.docs,
        SourceAddArgs {
            notebook: rig.notebook.clone(),
            text: Some(text.into()),
            ..SourceAddArgs::default()
        },
        OWNER,
    )
    .await
    .unwrap()
    .doc
    .id
}

/// Until `pred` holds for the card (≤ 30 s).
async fn until(rig: &Rig, id: &DocId, what: &str, pred: impl Fn(&document::Source) -> bool) {
    let start = Instant::now();
    loop {
        let s = document::ingest::source_get(&rig.docs, id.clone())
            .await
            .unwrap();
        if pred(&s) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "waited for {what}: {s:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn markdown(docs: &ActorRef<DocumentActor>, id: &DocId, annotate: bool) -> String {
    docs.ask(Markdown {
        doc: id.clone(),
        blocks: None,
        annotate,
    })
    .await
    .unwrap()
}

fn completed_events(rig: &Rig, id: &DocId) -> usize {
    rig.events
        .lock()
        .iter()
        .filter(|e| match e {
            SourceEvent::Changed { source, status } => {
                *status == SourceStatus::Completed && source.as_str() == id.as_str()
            }
            SourceEvent::Progress { .. } => false,
        })
        .count()
}

#[tokio::test]
async fn default_insight_is_appended_once_counted_and_announced_and_a_failure_is_recorded() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    mock();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), &bin, NotebooksConfig::default()).await;

    let id = add_text(
        &rig,
        "# Failover notes\n\nThe primary flapped twice.\n\n## Fix\n\nAdd a health check.\n",
    )
    .await;
    until(&rig, &id, "completed", |s| {
        s.status == SourceStatus::Completed
    })
    .await;
    until(&rig, &id, "the default insight", |s| s.insights == 1).await;
    let md = markdown(&rig.docs, &id, false).await;
    assert!(md.contains("## Insights"), "{md}");
    assert!(md.contains("### Dense summary · mock/model · 20"), "{md}");
    assert!(
        md.contains(&format!("Summary of: {DEFAULT_PROMPT}")),
        "{md}"
    );
    let call = CALLS.lock().last().cloned().unwrap();
    assert_eq!(call.system, DEFAULT_PROMPT);
    assert!(call.user.starts_with("# Failover notes"), "{}", call.user);
    assert!(!call.user.contains("## Insights"));
    assert_eq!(call.notebooks, vec![rig.notebook.clone()]);
    assert_eq!((call.session, call.model), (None, None));
    // `Changed{Completed}` once for the ingest and once more for the insight.
    let start = Instant::now();
    while completed_events(&rig, &id) < 2 {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no second Changed event"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A refresh with the same text keeps the blocks and does not add a second Dense summary.
    let calls = CALLS.lock().len();
    document::ingest::source_refresh(&rig.docs, id.clone(), OWNER)
        .await
        .unwrap();
    until(&rig, &id, "refresh", |s| {
        s.status == SourceStatus::Completed && s.attempts == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(CALLS.lock().len(), calls, "no second model call");
    assert_eq!(
        markdown(&rig.docs, &id, false)
            .await
            .matches("### Dense summary")
            .count(),
        1
    );

    // The model fails: the ingest is still `completed`, the error is on the card's details.
    let bad = add_text(&rig, "# Broken\n\nFAIL me please.\n").await;
    until(&rig, &bad, "completed", |s| {
        s.status == SourceStatus::Completed
    })
    .await;
    until(&rig, &bad, "insight_error", |s| {
        s.meta["insight_error"].is_string()
    })
    .await;
    let s = document::ingest::source_get(&rig.docs, bad.clone())
        .await
        .unwrap();
    assert_eq!(s.insights, 0);
    assert_eq!(s.meta["insight_error"], "provider said no");
    assert_eq!(s.error, None);
    assert!(
        !markdown(&rig.docs, &bad, false)
            .await
            .contains("## Insights")
    );
}

#[tokio::test]
async fn insight_run_builtin_and_skill_then_promote_to_a_note() {
    let _serial = SERIAL.lock().await;
    let bin = helper_or_skip!();
    mock();
    let dir = tempfile::tempdir().unwrap();
    let cfg = NotebooksConfig {
        insight_default: false,
        insight_model: Some("cheap/model".into()),
        ..NotebooksConfig::default()
    };
    let rig = start(dir.path(), &bin, cfg).await;
    let id = add_text(&rig, "# Failover notes\n\nThe primary flapped twice.\n").await;
    until(&rig, &id, "completed", |s| {
        s.status == SourceStatus::Completed
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = document::ingest::source_get(&rig.docs, id.clone())
        .await
        .unwrap();
    assert_eq!(s.insights, 0, "insight_default off");

    // A builtin: `## Insights` is created, the heading block id comes back, the count moves.
    let toc = insights::insight_run(
        &rig.docs,
        id.clone(),
        InsightInstruction {
            builtin: Some("toc".into()),
            skill: None,
        },
        None,
    )
    .await
    .unwrap();
    let annotated = markdown(&rig.docs, &id, true).await;
    assert!(
        annotated.contains(&format!(
            "<!-- {} -->\n### Table of contents · mock/model · ",
            toc.block
        )),
        "{annotated}"
    );
    let (heading, body) = section_of(&annotated, &toc.block).unwrap();
    assert!(heading.starts_with("Table of contents · mock/model · 20"));
    // The helper round-trips list bullets as `*`.
    assert_eq!(
        body,
        "Summary of: Table of contents: the document's structure as a nested list with one line per section\n\n* point"
    );
    let call = CALLS.lock().last().cloned().unwrap();
    assert_eq!(call.model.as_deref(), Some("cheap/model"));
    assert!(call.system.starts_with("Table of contents:"));
    assert_eq!(
        document::ingest::source_get(&rig.docs, id.clone())
            .await
            .unwrap()
            .insights,
        1
    );

    // A skill: appended under the same `## Insights`.
    let legal = insights::insight_run(
        &rig.docs,
        id.clone(),
        InsightInstruction {
            builtin: None,
            skill: Some("legal-review".into()),
        },
        None,
    )
    .await
    .unwrap();
    let annotated = markdown(&rig.docs, &id, true).await;
    assert_eq!(annotated.matches("## Insights").count(), 1);
    assert!(annotated.contains(&format!(
        "<!-- {} -->\n### Legal review · mock/model · ",
        legal.block
    )));
    assert!(
        section_of(&annotated, &legal.block)
            .unwrap()
            .1
            .starts_with("Summary of: Review the legal exposure")
    );
    assert_eq!(
        CALLS.lock().last().unwrap().system,
        "Review the legal exposure"
    );

    // Refusals: an unknown builtin, a non-insight skill, both or neither.
    for (b, s) in [
        (Some("nope"), None),
        (None, Some("harness")),
        (None, None),
        (Some("toc"), Some("legal-review")),
    ] {
        assert!(
            insights::insight_run(
                &rig.docs,
                id.clone(),
                InsightInstruction {
                    builtin: b.map(str::to_owned),
                    skill: s.map(str::to_owned),
                },
                None,
            )
            .await
            .is_err(),
            "{b:?} {s:?}"
        );
    }

    // Promote the TOC section: a note titled "<insight> — <source>", the trailer, filed in the notebook.
    let note = insights::insight_promote(&rig.docs, id.clone(), toc.block.clone(), None, OWNER)
        .await
        .unwrap();
    let note_md = markdown(&rig.docs, &note, false).await;
    assert!(
        note_md.starts_with("# Table of contents — Failover notes"),
        "{note_md}"
    );
    assert!(note_md.contains("Summary of: Table of contents"));
    // The trailer survives the import (BlockNote splits the emphasis around the link, as Z recorded for
    // W3's provenance line): the href, the model and the date are on the last line.
    let last = note_md.trim_end().lines().last().unwrap();
    assert!(last.starts_with("*From"), "{note_md}");
    assert!(
        last.contains(&format!(
            "[Failover notes](theuth://sources/{id}#{})",
            toc.block
        )),
        "{note_md}"
    );
    assert!(
        last.contains("· mock/model · 20") && last.ends_with('*'),
        "{note_md}"
    );
    let items = rig.store.items(&rig.notebook).await.unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.kind == ItemKind::Note && i.item == note.to_string()),
        "{items:?}"
    );

    // An explicit notebook files it there (and only there); an unknown block is refused.
    let other = new_notebook(&rig.docs, "Other").await;
    let note2 = insights::insight_promote(
        &rig.docs,
        id.clone(),
        legal.block.clone(),
        Some(other.clone()),
        OWNER,
    )
    .await
    .unwrap();
    assert!(
        rig.store
            .items(&other)
            .await
            .unwrap()
            .iter()
            .any(|i| i.item == note2.to_string())
    );
    assert!(
        !rig.store
            .items(&rig.notebook)
            .await
            .unwrap()
            .iter()
            .any(|i| i.item == note2.to_string())
    );
    assert!(
        insights::insight_promote(&rig.docs, id.clone(), "nope".into(), None, OWNER)
            .await
            .is_err()
    );
}
