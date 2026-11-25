//! Whiteboards end to end against the document actor (218 contract §3, §4.1): create without the
//! helper, the `wb_*` tools, the outline, export, the `doc_*` refusal, marks, aliases, turns and
//! *Undo this turn*, the placeholder sweep, blobs and the bake.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use document::actor::DocumentActor;
use document::tools::{Docs, InsertTool, ReadTool};
use document::wb::engine::BakeNow;
use document::wb::model::{self, Maps};
use document::wb::tools::{
    WbCardTool, WbDimTool, WbFrameTool, WbLinkTool, WbPlaceTool, WbReadTool, WbUpdateTool,
    Whiteboards,
};
use document::wb::{
    BlobGet, BlobPut, CreateWhiteboard, TurnPhase, UndoTurn, WbOutline, WbTurn, WhiteboardCreated,
};
use document::{
    ApplyUpdate, DOCUMENT_ACTOR, DocId, DocumentsConfig, Homing, Markdown, OpenDoc, Outline,
    Principal, Tick, helper, store,
};
use kameo::actor::{ActorRef, Spawn};
use serde_json::{Value, json};
use tools::{Tool, ToolCx, ToolOutput};
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, Map, ReadTxn, StateVector, Transact, Update};

/// Every test registers the one `DOCUMENT_ACTOR` name: one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const OWNER: Principal = Principal::Owner { device: None };

/// A document actor over a fake helper that counts every request it gets.
async fn start(data_dir: &Path) -> (ActorRef<DocumentActor>, Arc<AtomicUsize>) {
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.to_path_buf());
    let store = store::Store::open(data_dir).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let h = helper::Helper::fake(tx, move |_line| {
        seen.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    });
    let cfg = DocumentsConfig {
        export_debounce_ms: 50,
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
    (actor, calls)
}

async fn stop(actor: ActorRef<DocumentActor>) {
    actor.stop_gracefully().await.unwrap();
    actor.wait_for_shutdown().await;
}

fn cx(session: uuid::Uuid) -> ToolCx {
    ToolCx {
        session: runtime::SessionId(session),
        call_id: String::new(),
        quiet: false,
        resources: Arc::new(tools::SessionResources {
            root: std::env::temp_dir(),
            retention: tools::Retention::Durable,
            generation: 1,
        }),
        cancel: tools::CancellationToken::new(),
        promote_after: None,
    }
}

async fn create(actor: &ActorRef<DocumentActor>, title: &str) -> WhiteboardCreated {
    actor
        .ask(CreateWhiteboard {
            title: Some(title.into()),
            by: OWNER,
            home: Homing::Here,
        })
        .await
        .unwrap()
}

async fn call<T: Tool>(t: &T, args: Value, s: uuid::Uuid) -> ToolOutput {
    let a: T::Args = serde_json::from_value(args).unwrap();
    t.call(a, cx(s)).await.unwrap()
}

fn ok(o: &ToolOutput) -> &Value {
    assert!(!o.is_error, "tool failed: {}", o.content);
    o.structured.as_ref().unwrap()
}

async fn outline(actor: &ActorRef<DocumentActor>, doc: &DocId) -> String {
    actor
        .ask(WbOutline {
            doc: doc.clone(),
            frame: None,
        })
        .await
        .unwrap()
        .text
}

/// The document's current state in a local `Doc` (as a client sees it).
async fn client(actor: &ActorRef<DocumentActor>, doc: &DocId) -> Doc {
    let open = actor
        .ask(OpenDoc {
            doc: doc.clone(),
            sv: None,
            by: OWNER,
        })
        .await
        .unwrap();
    let d = Doc::new();
    for chunk in open.state {
        let bytes = base64::engine::general_purpose::STANDARD.decode(chunk).unwrap();
        d.transact_mut()
            .apply_update(Update::decode_v1(&bytes).unwrap())
            .unwrap();
    }
    d
}

/// An owner edit: `f` on a client copy, sent as one `doc_update`.
async fn owner_edit(
    actor: &ActorRef<DocumentActor>,
    doc: &DocId,
    f: impl FnOnce(&Maps, &mut yrs::TransactionMut),
) {
    let d = client(actor, doc).await;
    let sv: StateVector = d.transact().state_vector();
    let maps = Maps::of(&d);
    {
        let mut txn = d.transact_mut();
        f(&maps, &mut txn);
    }
    let update = d.transact().encode_diff_v1(&sv);
    actor
        .ask(ApplyUpdate {
            doc: doc.clone(),
            update: base64::engine::general_purpose::STANDARD.encode(update),
            by: OWNER,
        })
        .await
        .unwrap();
}

fn id_of(snap: &model::Snapshot, alias: &str) -> String {
    snap.resolve(alias).unwrap()
}

async fn snapshot(actor: &ActorRef<DocumentActor>, doc: &DocId) -> model::Snapshot {
    let d = client(actor, doc).await;
    model::read(&d.transact())
}

#[tokio::test]
async fn create_place_fill_link_push() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Attention inbox").await;
    let w = Whiteboards::new(actor.clone());
    let s = uuid::Uuid::now_v7();
    let doc = wb.doc.to_string();

    // A sticky and a diagram, placed filled; two page placeholders in a phone frame.
    let r = call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "sticky", "title": "Requirements",
        "source": {"t": "sticky", "text": "- Triage with one thumb\n- Never lose a decision"}}), s).await;
    assert_eq!(ok(&r)["card"], "c1");
    assert_eq!(ok(&r)["placeholder"], false);
    // `whiteboard` is optional from here on: the session's current whiteboard.
    let r = call(&WbPlaceTool(w.clone()), json!({"kind": "diagram", "title": "Item lifecycle", "near": "c1", "side": "below",
        "source": {"t": "diagram", "lang": "mermaid", "code": "stateDiagram-v2\n  new --> seen"}}), s).await;
    assert_eq!(ok(&r)["card"], "c2");
    let a = call(&WbPlaceTool(w.clone()), json!({"kind": "page", "title": "Mock A", "device": "phone"}), s).await;
    assert_eq!(ok(&a)["placeholder"], true);
    assert_eq!((ok(&a)["w"].as_f64(), ok(&a)["h"].as_f64()), (Some(180.0), Some(390.0)));
    let f = call(&WbFrameTool(w.clone()), json!({"title": "Variants", "cards": ["c3"], "device": "phone"}), s).await;
    assert_eq!(ok(&f)["frame"], "f1");
    let b = call(&WbPlaceTool(w.clone()), json!({"kind": "page", "title": "Mock B", "frame": "f1"}), s).await;
    assert_eq!(ok(&b)["card"], "c4");
    assert_eq!(ok(&b)["frame"], "f1");
    let today = call(&WbPlaceTool(w.clone()), json!({"kind": "image", "title": "Today", "frame": "f1",
        "source": {"t": "image", "path": "/tmp/today.png"}}), s).await;
    assert_eq!(ok(&today)["card"], "c5");

    // Fill both placeholders, then push a version onto B.
    for (card, call_id) in [("c3", "toolu_A1"), ("c4", "toolu_B1")] {
        let u = call(&WbUpdateTool(w.clone()), json!({"card": card, "source": {"t": "page", "call_id": call_id}}), s).await;
        assert_eq!(ok(&u)["filled"], true);
        assert_eq!(ok(&u)["version"], 1);
    }
    let u = call(&WbUpdateTool(w.clone()), json!({"card": "c4", "source": {"t": "page", "call_id": "toolu_B2"},
        "note": "A's header"}), s).await;
    assert_eq!((ok(&u)["version"].as_u64(), ok(&u)["filled"].as_bool()), (Some(2), Some(false)));
    // A kind mismatch is refused with what to do.
    let bad = call(&WbUpdateTool(w.clone()), json!({"card": "c4", "source": {"t": "sticky", "text": "x"}}), s).await;
    assert!(bad.is_error && bad.content.starts_with("wb_update: "), "{}", bad.content);

    let l = call(&WbLinkTool(w.clone()), json!({"from": "c1", "to": "c4", "label": "drives"}), s).await;
    assert_eq!(ok(&l)["link"], "l1");
    let d = call(&WbDimTool(w.clone()), json!({"ids": ["c3"], "tag": "grouping rejected"}), s).await;
    assert_eq!(ok(&d)["dimmed"], json!(["c3"]));

    let text = outline(&actor, &wb.doc).await;
    for line in [
        "frame f1 \"Variants\" · phone 390×844",
        "c1 sticky \"Requirements\" · live · 2 lines · #",
        "c2 diagram \"Item lifecycle\" · live · #",
        "c3 page \"Mock A\" v1/1 · dimmed \"grouping rejected\"",
        "c4 page \"Mock B\" v2/2 · live",
        "c5 image \"Today\" v1/1 · live",
        "l1 c1 → c4 \"drives\"",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
    assert!(text.starts_with(&format!(
        "<whiteboard id=\"{}\" title=\"Attention inbox\" cards=\"5\" links=\"1\" marks=\"0\">",
        wb.doc
    )));
    // The three page/image cards share the frame's row.
    let row = text.lines().skip_while(|l| !l.starts_with("frame f1")).nth(1).unwrap();
    assert_eq!(row, "  row 1: c3 c4 c5");
    // No card overlaps another.
    let snap = snapshot(&actor, &wb.doc).await;
    for (i, a) in snap.cards.iter().enumerate() {
        for b in &snap.cards[i + 1..] {
            let hit = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
            assert!(!hit, "{} overlaps {}", a.title, b.title);
        }
    }
    // wb_card reads one card with its version history.
    let c = call(&WbCardTool(w.clone()), json!({"card": "c4", "version": 1}), s).await;
    assert_eq!(ok(&c)["versions"], 2);
    assert_eq!(ok(&c)["source"]["call_id"], "toolu_B1");
    assert_eq!(ok(&c)["read_with"], "artifact_read(\"toolu_B1\")");
    let r = call(&WbReadTool(w.clone()), json!({"frame": "f1"}), s).await;
    assert!(r.content.starts_with("<whiteboard_frame"), "{}", r.content);
    stop(actor).await;
}

#[tokio::test]
async fn create_never_calls_the_helper() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, calls) = start(dir.path()).await;
    let wb = create(&actor, "Quiet").await;
    let w = Whiteboards::new(actor.clone());
    let s = uuid::Uuid::now_v7();
    let r = call(&WbPlaceTool(w.clone()), json!({"whiteboard": wb.doc.to_string(), "kind": "sticky", "title": "A",
        "source": {"t": "sticky", "text": "one"}}), s).await;
    ok(&r);
    owner_edit(&actor, &wb.doc, |m, t| {
        m.set_meta(t, "bg", Any::from("grid"));
    })
    .await;
    outline(&actor, &wb.doc).await;
    actor.ask(document::actor::ExportNow(wb.doc.clone())).await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the helper was asked");
    let meta = store::Store::open(dir.path()).await.unwrap().read_meta(&wb.doc).unwrap();
    assert_eq!(meta.kind, document::DocKind::Whiteboard);
    assert!(meta.derived.ends_with("/whiteboard.md"));
    stop(actor).await;
}

#[tokio::test]
async fn export_writes_the_outline() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Spec").await;
    let w = Whiteboards::new(actor.clone());
    let s = uuid::Uuid::now_v7();
    ok(&call(&WbPlaceTool(w), json!({"whiteboard": wb.doc.to_string(), "kind": "sticky", "title": "Goals",
        "source": {"t": "sticky", "text": "ship"}}), s).await);
    let path = dir.path().join(format!("documents/{}/whiteboard.md", wb.doc));
    let start = Instant::now();
    loop {
        if let Ok(t) = std::fs::read_to_string(&path)
            && t.contains("c1 sticky \"Goals\"")
        {
            assert!(t.starts_with("# Spec\n\n"), "{t}");
            assert!(!t.contains("<whiteboard"));
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "no export: {:?}", std::fs::read_to_string(&path));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop(actor).await;
}

#[tokio::test]
async fn doc_tools_refuse_a_whiteboard() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, calls) = start(dir.path()).await;
    let wb = create(&actor, "Not a note").await;
    let refusal = document::wb::DOC_REFUSAL;
    let md = actor
        .ask(Markdown {
            doc: wb.doc.clone(),
            blocks: None,
            annotate: false,
        })
        .await;
    assert!(md.unwrap_err().to_string().contains(refusal));
    let ol = actor
        .ask(Outline {
            doc: wb.doc.clone(),
            depth: None,
        })
        .await;
    assert!(ol.unwrap_err().to_string().contains(refusal));
    let docs = Docs::new(actor.clone(), dir.path().to_path_buf());
    let s = uuid::Uuid::now_v7();
    let read = ReadTool(docs.clone())
        .call(serde_json::from_value(json!({"doc": wb.doc.to_string()})).unwrap(), cx(s))
        .await;
    let text = match read {
        Ok(o) => o.content,
        Err(e) => e.to_string(),
    };
    assert!(text.contains(refusal), "{text}");
    let ins = InsertTool(docs)
        .call(serde_json::from_value(json!({"doc": wb.doc.to_string(), "markdown": "hi"})).unwrap(), cx(s))
        .await;
    let text = match ins {
        Ok(o) => o.content,
        Err(e) => e.to_string(),
    };
    assert!(text.contains(refusal), "{text}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    stop(actor).await;
}

fn put_mark(m: &Maps, t: &mut yrs::TransactionMut, id: &str, card: &str, ver: u32, state: &str, sent_at: i64, strokes: &[&str]) {
    let mark = model::Mark {
        id: id.into(),
        strokes: strokes.iter().map(|s| (*s).to_owned()).collect(),
        card: Some(card.into()),
        ver: Some(ver),
        bb: [0.0, 0.0, 10.0, 10.0],
        state: serde_json::from_value(json!(state)).unwrap(),
        region: None,
        hw: None,
        sent_at,
        blob: false,
        baked: false,
    };
    m.put(t, "marks", id, model::mark_fields(&mark, "owner"));
}

fn put_stroke(m: &Maps, t: &mut yrs::TransactionMut, id: &str, card: &str) {
    let pts = model::pack::encode([0.0, 0.0], &[(0.0, 0.0, 0.5, 0), (5.0, 5.0, 0.5, 8)]);
    let v = Any::Map(Arc::new(
        [
            ("pts".to_owned(), Any::Buffer(pts.into())),
            ("n".to_owned(), Any::from(2.0)),
            ("o".to_owned(), model::plain(&[0.0, 0.0])),
            ("bb".to_owned(), model::plain(&[0.0, 0.0, 5.0, 5.0])),
            ("card".to_owned(), Any::from(card)),
            ("ver".to_owned(), Any::from(1.0)),
            ("at".to_owned(), Any::from(1.0)),
        ]
        .into_iter()
        .collect(),
    ));
    m.set_stroke(t, id, v);
}

#[tokio::test]
async fn update_answers_sent_marks() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Marks").await;
    let w = Whiteboards::new(actor.clone());
    let s = uuid::Uuid::now_v7();
    ok(&call(&WbPlaceTool(w.clone()), json!({"whiteboard": wb.doc.to_string(), "kind": "page", "title": "B",
        "source": {"t": "page", "call_id": "toolu_1"}}), s).await);
    let card = id_of(&snapshot(&actor, &wb.doc).await, "c1");
    let (m_sent, m_pinned) = (model::ulid(), model::ulid());
    owner_edit(&actor, &wb.doc, |m, t| {
        put_mark(m, t, &m_sent, &card, 1, "sent", 1, &[]);
        put_mark(m, t, &m_pinned, &card, 1, "pinned", 1, &[]);
    })
    .await;
    let text = outline(&actor, &wb.doc).await;
    assert!(text.contains("m1 on c1 v1 · sent"), "{text}");
    assert!(text.contains("m2 on c1 v1 · pinned"), "{text}");
    let u = call(&WbUpdateTool(w), json!({"card": "c1", "source": {"t": "page", "call_id": "toolu_2"}}), s).await;
    assert!(u.content.contains("marks answered: m1"), "{}", u.content);
    let snap = snapshot(&actor, &wb.doc).await;
    assert_eq!(snap.mark(&m_sent).unwrap().state, model::MarkState::Answered);
    assert_eq!(snap.mark(&m_pinned).unwrap().state, model::MarkState::Pinned);
    let text = outline(&actor, &wb.doc).await;
    assert!(!text.contains("m1 on"), "{text}");
    assert!(text.contains("m2 on c1 v1 · pinned"));
    stop(actor).await;
}

#[tokio::test]
async fn aliases_same_for_two_sessions() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Shared").await;
    let w = Whiteboards::new(actor.clone());
    let (x, y) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    let doc = wb.doc.to_string();
    // The owner adds a card the engine has not named yet.
    let owner_card = model::ulid();
    owner_edit(&actor, &wb.doc, |m, t| {
        let c = model::Card {
            id: owner_card.clone(),
            kind: model::CardKind::Sticky,
            x: 0.0,
            y: 0.0,
            w: 220.0,
            h: 150.0,
            z: 1,
            title: "Mine".into(),
            versions: vec![],
            viewport: None,
            state: model::CardState::Live,
            tag: None,
            frame: None,
            placeholder: None,
            snap: None,
            pin: false,
            by: "owner".into(),
        };
        m.put(t, "cards", &owner_card, model::card_fields(&c));
    })
    .await;
    let rx = call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "sticky", "title": "X",
        "source": {"t": "sticky", "text": "x"}}), x).await;
    let ry = call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "sticky", "title": "Y",
        "source": {"t": "sticky", "text": "y"}}), y).await;
    assert_eq!((ok(&rx)["card"].as_str(), ok(&ry)["card"].as_str()), (Some("c2"), Some("c3")));
    let a = actor.ask(WbOutline { doc: wb.doc.clone(), frame: None }).await.unwrap();
    let b = actor.ask(WbOutline { doc: wb.doc.clone(), frame: None }).await.unwrap();
    assert_eq!(a.aliases, b.aliases);
    assert_eq!(a.aliases[&owner_card], "c1");
    assert_eq!(a.text, b.text);
    stop(actor).await;
}

#[tokio::test]
async fn undo_turn_skips_what_the_owner_changed() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Undo").await;
    let w = Whiteboards::new(actor.clone());
    let s = uuid::Uuid::now_v7();
    let doc = wb.doc.to_string();
    let n = actor.ask(WbTurn { doc: wb.doc.clone(), session: s, phase: TurnPhase::Begin }).await.unwrap();
    assert_eq!(n, 1);
    for t in ["A", "B"] {
        ok(&call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "sticky", "title": t,
            "source": {"t": "sticky", "text": t}}), s).await);
    }
    ok(&call(&WbDimTool(w.clone()), json!({"ids": ["c1"], "tag": "no"}), s).await);
    let snap = snapshot(&actor, &wb.doc).await;
    let (a, b) = (id_of(&snap, "c1"), id_of(&snap, "c2"));
    // The owner moves B.
    owner_edit(&actor, &wb.doc, |m, t| {
        m.set(t, "cards", &b, "x", Some(Any::from(999.0)));
    })
    .await;
    let turn = actor
        .ask(document::wb::TurnOf { doc: wb.doc.clone(), session: s.to_string(), by: OWNER })
        .await
        .unwrap()
        .unwrap();
    assert_eq!((turn.turn, turn.writes, turn.cards.len()), (1, 3, 2));
    let r = actor
        .ask(UndoTurn { doc: wb.doc.clone(), session: s.to_string(), turn: None, by: OWNER })
        .await
        .unwrap();
    assert_eq!(r.turn, 1);
    assert_eq!(r.skipped.len(), 1, "{:?}", r.skipped);
    assert_eq!(r.skipped[0].id, b);
    assert_eq!(r.skipped[0].alias.as_deref(), Some("c2"));
    assert_eq!(r.skipped[0].why, "moved by you since");
    let snap = snapshot(&actor, &wb.doc).await;
    assert!(snap.card(&a).is_none(), "A (placed and dimmed this turn) is gone");
    assert_eq!(snap.card(&b).unwrap().x, 999.0);
    // Aliases stay reserved: the next card is c3.
    let r = call(&WbPlaceTool(w), json!({"whiteboard": doc, "kind": "sticky", "title": "C",
        "source": {"t": "sticky", "text": "c"}}), s).await;
    assert_eq!(ok(&r)["card"], "c3");
    // A second undo of the same turn is refused.
    let again = actor
        .ask(UndoTurn { doc: wb.doc.clone(), session: s.to_string(), turn: Some(1), by: OWNER })
        .await;
    assert!(again.is_err());
    stop(actor).await;
}

#[tokio::test]
async fn placeholder_swept_at_turn_begin_and_end() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Sweep").await;
    let w = Whiteboards::new(actor.clone());
    let (s, other) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    let doc = wb.doc.to_string();
    actor.ask(WbTurn { doc: wb.doc.clone(), session: s, phase: TurnPhase::Begin }).await.unwrap();
    ok(&call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "page", "title": "Never filled"}), s).await);
    ok(&call(&WbPlaceTool(w.clone()), json!({"whiteboard": doc, "kind": "page", "title": "Theirs"}), other).await);
    let text = outline(&actor, &wb.doc).await;
    assert!(text.contains("c1 page \"Never filled\" · placeholder"), "{text}");
    actor.ask(WbTurn { doc: wb.doc.clone(), session: s, phase: TurnPhase::End }).await.unwrap();
    let text = outline(&actor, &wb.doc).await;
    assert!(!text.contains("Never filled"), "{text}");
    assert!(text.contains("c2 page \"Theirs\" · placeholder"), "another session's placeholder stays");
    actor.ask(WbTurn { doc: wb.doc.clone(), session: other, phase: TurnPhase::Begin }).await.unwrap();
    let text = outline(&actor, &wb.doc).await;
    assert!(!text.contains("Theirs"), "{text}");
    stop(actor).await;
}

#[tokio::test]
async fn blob_put_get_and_key_validation() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Blobs").await;
    let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG fake");
    let card = model::ulid();
    let key = format!("snap/{card}/2");
    let put = |key: String, mime: &str, data: String| BlobPut {
        doc: wb.doc.clone(),
        key,
        mime: mime.into(),
        data,
        by: OWNER,
    };
    let r = actor.ask(put(key.clone(), "image/png", png.clone())).await.unwrap();
    assert_eq!((r.key.as_str(), r.bytes), (key.as_str(), 9));
    let got = actor
        .ask(BlobGet { doc: wb.doc.clone(), key: key.clone(), by: OWNER })
        .await
        .unwrap()
        .unwrap();
    assert_eq!((got.mime.as_str(), got.data.as_str()), ("image/png", png.as_str()));
    // Overwrite with another type replaces it.
    let jpg = base64::engine::general_purpose::STANDARD.encode(b"jpeg!");
    actor.ask(put(key.clone(), "image/jpeg", jpg.clone())).await.unwrap();
    let got = actor.ask(BlobGet { doc: wb.doc.clone(), key: key.clone(), by: OWNER }).await.unwrap().unwrap();
    assert_eq!((got.mime.as_str(), got.data.as_str()), ("image/jpeg", jpg.as_str()));
    // Absent → None.
    let none = actor
        .ask(BlobGet { doc: wb.doc.clone(), key: format!("mark/{}", model::ulid()), by: OWNER })
        .await
        .unwrap();
    assert!(none.is_none());
    for bad in [
        "snap/notaulid/1".to_owned(),
        format!("snap/{card}/12345"),
        format!("../{card}"),
        format!("mark/{card}/1"),
        format!("img/{}", card.to_lowercase()),
        "snap".to_owned(),
    ] {
        assert!(actor.ask(put(bad.clone(), "image/png", png.clone())).await.is_err(), "{bad} accepted");
    }
    assert!(actor.ask(put(format!("img/{card}"), "image/gif", png.clone())).await.is_err());
    assert!(actor.ask(put(format!("img/{card}"), "image/png", "%%%".into())).await.is_err());
    let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 3 * 1024 * 1024 + 1]);
    assert!(actor.ask(put(format!("img/{card}"), "image/png", big)).await.is_err());
    stop(actor).await;
}

#[tokio::test]
async fn bake_deletes_old_answered_strokes() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (actor, _) = start(dir.path()).await;
    let wb = create(&actor, "Bake").await;
    let card = model::ulid();
    let (old, recent, sent) = (model::ulid(), model::ulid(), model::ulid());
    let (s_old, s_recent, s_sent, s_free) = (model::ulid(), model::ulid(), model::ulid(), model::ulid());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    owner_edit(&actor, &wb.doc, |m, t| {
        for s in [&s_old, &s_recent, &s_sent, &s_free] {
            put_stroke(m, t, s, &card);
        }
        put_mark(m, t, &old, &card, 1, "answered", now - 25 * 3600 * 1000, &[&s_old]);
        put_mark(m, t, &recent, &card, 1, "answered", now - 3600 * 1000, &[&s_recent]);
        put_mark(m, t, &sent, &card, 1, "sent", now - 48 * 3600 * 1000, &[&s_sent]);
    })
    .await;
    let n = actor.ask(BakeNow(wb.doc.clone())).await.unwrap();
    assert_eq!(n, 1);
    let snap = snapshot(&actor, &wb.doc).await;
    let ink: Vec<&str> = snap.strokes.iter().map(|s| s.id.as_str()).collect();
    assert!(!ink.contains(&s_old.as_str()));
    for k in [&s_recent, &s_sent, &s_free] {
        assert!(ink.contains(&k.as_str()));
    }
    assert!(snap.mark(&old).unwrap().baked);
    assert!(!snap.mark(&recent).unwrap().baked);
    // Idempotent.
    assert_eq!(actor.ask(BakeNow(wb.doc.clone())).await.unwrap(), 0);
    stop(actor).await;
}

/// B3's fixture (`canvas/fixtures/example-v1.update.b64`, the §2.10 example written by yjs) reads
/// back through the Rust twin. Run by the orchestrator once both sides landed.
#[test]
#[ignore]
fn reads_the_yjs_fixture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../desktop/ui/src/components/whiteboard/canvas/fixtures/example-v1.update.b64");
    let b64 = std::fs::read_to_string(&path).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).unwrap();
    let d = Doc::new();
    d.transact_mut().apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
    let txn = d.transact();
    let snap = model::read(&txn);
    let meta = model::read_meta(&txn);
    assert_eq!((meta.v, meta.created, meta.bg.as_str()), (1, 1_795_570_000_000, "dots"));
    let c1 = snap.card("01KAR3ZC4M0000000000000001").unwrap();
    assert_eq!((c1.kind, c1.x, c1.y, c1.w, c1.h, c1.z), (model::CardKind::Page, 250.0, 80.0, 180.0, 390.0, 2));
    assert_eq!(c1.title, "Mock B — flat list");
    assert_eq!(c1.versions.len(), 2);
    assert_eq!(c1.versions[1].note.as_deref(), Some("A's header, filter chips"));
    assert_eq!(c1.versions[1].at, 1_795_570_400_000);
    assert_eq!(c1.viewport.as_ref().map(|v| (v.w, v.h, v.preset.as_str())), Some((390.0, 844.0, "phone")));
    assert_eq!(c1.frame.as_deref(), Some("01KAR3ZC4M0000000000000009"));
    assert_eq!(c1.snap.as_ref().map(|s| (s.v, s.at, s.theme.as_str())), Some((2, 1_795_570_405_000, "light")));
    let c2 = snap.card("01KAR3ZC4M0000000000000002").unwrap();
    assert_eq!(c2.kind, model::CardKind::Sticky);
    assert!(matches!(&c2.versions[0].source, model::Source::Sticky { text } if text.starts_with("- Triage")));
    let l = snap.link("01KAR3ZC4M0000000000000005").unwrap();
    assert_eq!((l.label.as_deref(), l.style, l.arrow), (Some("drives"), model::LinkStyle::Solid, model::Arrow::End));
    let f = snap.frame("01KAR3ZC4M0000000000000009").unwrap();
    assert_eq!((f.title.as_str(), f.x, f.y, f.w, f.h, f.preset.as_deref()), ("Variants", 36.0, 48.0, 610.0, 424.0, Some("phone")));
    let s = &snap.strokes[0];
    assert_eq!((s.id.as_str(), s.n, s.bytes, s.ver, s.at), ("01KAR3ZC4M0000000000000007", 3, 18, Some(1), 1_795_570_200_000));
    assert_eq!(s.card.as_deref(), Some("01KAR3ZC4M0000000000000001"));
    // The packed points decode to the golden vector.
    let ink = txn.get_map("ink").unwrap();
    let Some(yrs::Out::Any(Any::Map(stroke))) = ink.get(&txn, "01KAR3ZC4M0000000000000007") else {
        panic!("stroke is not a plain map");
    };
    let Some(Any::Buffer(pts)) = stroke.get("pts") else { panic!("pts is not bytes") };
    let hex: String = pts.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "0000000080000c000200ff02f0fff6ff3303");
    let m = snap.mark("01KAR3ZC4M0000000000000008").unwrap();
    assert_eq!((m.state, m.blob, m.ver), (model::MarkState::Answered, true, Some(1)));
    assert_eq!(m.region.as_ref().map(|r| r.label.as_str()), Some("tab bar, item 3"));
    assert_eq!(snap.aliases.get("01KAR3ZC4M0000000000000008").map(String::as_str), Some("m4"));
}
