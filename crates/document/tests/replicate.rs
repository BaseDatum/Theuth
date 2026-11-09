//! 144 §3.3/§3.5 (D14), 171 §13.2: the document tables ride `replica::attach` — a notebook, its items and a
//! source row written on engine A show up on engine B's store within the capture→feed hop; a soft
//! removal on A and a revival on B merge as column writes (no row ever deleted). Two in-process
//! engines over a `tokio::io::duplex` transport, the harness `replica/tests/sync_loopback.rs` uses.
//! 184 §6.4: `cell_latest` rows from two engines never mix, and a foreign row is announced on the bus.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use document::store::{NotebookRow, Store};
use document::{
    AgentStyle, Binding, CellLatest, CellLatestChanged, DocId, DocMeta, ItemKind, NotebookItem,
    SourceOrigin, SourceStatus,
};
use kameo::actor::Spawn as _;
use parking_lot::Mutex;
use replica::{ByteStream, Engine, PeerEvent, PeerId, Transport};
use tokio::sync::{broadcast, mpsc};

type Inbound = (PeerId, Box<dyn ByteStream>);

#[derive(Default)]
struct Switch {
    inboxes: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>,
    /// 184: every link cut (a partition) while set.
    parted: AtomicBool,
}

struct Loop {
    me: PeerId,
    switch: Arc<Switch>,
    inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
    events: broadcast::Sender<PeerEvent>,
}

impl Loop {
    fn new(name: &str, switch: &Arc<Switch>) -> Arc<Self> {
        let me = PeerId(name.to_owned());
        let (tx, rx) = mpsc::unbounded_channel();
        switch.inboxes.lock().insert(me.clone(), tx);
        Arc::new(Self {
            me,
            switch: switch.clone(),
            inbox: tokio::sync::Mutex::new(rx),
            events: broadcast::channel(64).0,
        })
    }
}

#[async_trait::async_trait]
impl Transport for Loop {
    fn peers(&self) -> Vec<PeerId> {
        if self.switch.parted.load(Ordering::SeqCst) {
            return Vec::new();
        }
        self.switch
            .inboxes
            .lock()
            .keys()
            .filter(|p| **p != self.me)
            .cloned()
            .collect()
    }

    async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::ensure!(!self.switch.parted.load(Ordering::SeqCst), "parted");
        let (mine, theirs) = tokio::io::duplex(1 << 20);
        let inboxes = self.switch.inboxes.lock();
        let to = inboxes
            .get(peer)
            .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
        to.send((self.me.clone(), Box::new(theirs)))
            .map_err(|_| anyhow::anyhow!("peer gone"))?;
        Ok(Box::new(mine))
    }

    async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
        self.inbox
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("switch closed"))
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }
}

struct Node {
    store: Store,
    _engine: Arc<Engine>,
    handle: replica::ReplicaHandle,
    transport: Arc<Loop>,
    _dir: tempfile::TempDir,
}

async fn node(name: &str, switch: &Arc<Switch>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    let handle = engine
        .attach(store.pool(), document::store::TABLES)
        .await
        .unwrap();
    let transport = Loop::new(name, switch);
    engine.register_transport(transport.clone());
    Node {
        store,
        _engine: engine,
        handle,
        transport,
        _dir: dir,
    }
}

/// Connect (`up`) or cut every link between `a` and `b`, and tell both sides.
fn link(switch: &Switch, a: &Node, b: &Node, up: bool) {
    switch.parted.store(!up, Ordering::SeqCst);
    let ev = |p: &PeerId| {
        if up {
            PeerEvent::Connected(p.clone())
        } else {
            PeerEvent::Disconnected(p.clone())
        }
    };
    let _ = a.transport.events.send(ev(&b.transport.me));
    let _ = b.transport.events.send(ev(&a.transport.me));
}

/// The `CellLatestChanged` documents `n`'s bus carried (`document::cells::watch_latest`).
#[derive(kameo::Actor)]
struct Heard(Arc<Mutex<Vec<DocId>>>);

impl kameo::message::Message<CellLatestChanged> for Heard {
    type Reply = ();
    async fn handle(
        &mut self,
        m: CellLatestChanged,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) {
        self.0.lock().push(m.doc);
    }
}

async fn heard(n: &Node) -> Arc<Mutex<Vec<DocId>>> {
    let rt = runtime::Runtime::new(figment::Figment::new(), n._dir.path().to_path_buf());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Heard::spawn_with_mailbox(Heard(seen.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Heard, CellLatestChanged>(&sink).await;
    document::cells::watch_latest(n.handle.clone(), rt);
    seen
}

fn meta(binding: Binding, title: &str) -> DocMeta {
    let id = DocId::for_binding(&binding);
    DocMeta {
        derived: binding.derived(&id),
        id,
        kind: binding.kind(),
        title: title.into(),
        binding,
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

/// Poll until `f` holds or fail after 10 s (capture polls at 200 ms).
async fn until(what: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn notebook_tables_replicate_between_two_engines() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    let _ = a
        .transport
        .events
        .send(PeerEvent::Connected(b.transport.me.clone()));
    let _ = b
        .transport
        .events
        .send(PeerEvent::Connected(a.transport.me.clone()));

    // A: a notebook document + row, a source document + row, one item each.
    let nb = meta(Binding::Notebook, "Runbooks");
    let src = meta(Binding::Source("url:http://x/".into()), "X");
    a.store.create(&nb).await.unwrap();
    a.store.create(&src).await.unwrap();
    a.store
        .notebook_upsert(&NotebookRow {
            id: nb.id.clone(),
            project: Some("ops".into()),
            archived_ms: None,
        })
        .await
        .unwrap();
    a.store
        .source_upsert(&document::store::SourceRow {
            id: src.id.clone(),
            origin: SourceOrigin::Url {
                url: "http://x/".into(),
                canonical: None,
            },
            status: SourceStatus::Completed,
            error: None,
            attempts: 1,
            blob: None,
            fetched_ms: Some(5),
            content_sha: Some("abc".into()),
            changed_since: None,
            bytes: Some(10),
            words: Some(2),
            duration_ms: None,
            pages: None,
            meta: serde_json::json!({ "title": "X" }),
            insights: 0,
        })
        .await
        .unwrap();
    let item = NotebookItem {
        notebook: nb.id.clone(),
        kind: ItemKind::Source,
        item: src.id.to_string(),
        added_ms: 10,
        added_by: "owner".into(),
        removed_ms: None,
    };
    a.store.item_add(&item).await.unwrap();

    // B sees the notebook, its item and the source row.
    until("notebook on B", async || {
        b.store.notebook_rows(false).await.unwrap().len() == 1
    })
    .await;
    until("item on B", async || {
        b.store.items(&nb.id).await.unwrap().len() == 1
    })
    .await;
    until("source row on B", async || {
        b.store.source_row(&src.id).await.unwrap().is_some()
    })
    .await;
    let rows = b.store.notebook_rows(false).await.unwrap();
    assert_eq!(rows[0].project.as_deref(), Some("ops"));
    assert_eq!(b.store.items(&nb.id).await.unwrap(), vec![item.clone()]);
    let row = b.store.source_row(&src.id).await.unwrap().unwrap();
    assert_eq!(
        (row.status, row.content_sha.as_deref()),
        (SourceStatus::Completed, Some("abc"))
    );
    assert_eq!(
        b.store
            .notebooks_of(ItemKind::Source, src.id.as_str(), true)
            .await
            .unwrap(),
        vec![nb.id.clone()]
    );
    assert_eq!(
        b.store.summary(&nb.id).await.unwrap().map(|s| s.title),
        Some("Runbooks".into()),
        "the docs row rides along"
    );

    // A removes (a column write), B sees it; B re-adds, A sees the revival with the first add kept.
    assert!(
        a.store
            .item_remove(&nb.id, ItemKind::Source, src.id.as_str(), 20)
            .await
            .unwrap()
    );
    until("removal on B", async || {
        b.store.items(&nb.id).await.unwrap().is_empty()
    })
    .await;
    let mut again = item.clone();
    again.added_ms = 99;
    again.added_by = "member:bbbbbbbb".into();
    b.store.item_add(&again).await.unwrap();
    until("revival on A", async || {
        a.store.items(&nb.id).await.unwrap().len() == 1
    })
    .await;
    assert_eq!(a.store.items(&nb.id).await.unwrap(), vec![item]);
}

/// 171 §13.2: a note cell's latest row upserted on one engine reaches the other engine, a newer run
/// replaces it there, and deleting the note removes it on both. 184 §6.4 (W-p): the receiving
/// engine's bus says `CellLatestChanged{doc}`; the writer's own batch says nothing.
#[tokio::test]
async fn cell_latest_replicates_between_two_engines() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    let (heard_a, heard_b) = (heard(&a).await, heard(&b).await);
    link(&switch, &a, &b, true);

    let note = meta(Binding::Note, "Runbook");
    a.store.create(&note).await.unwrap();
    let mut row = latest(&note, "r_1", "A");
    a.store.put_cell_latest(&row).await.unwrap();
    until("row on B", async || {
        b.store.cell_latest(&note.id).await.unwrap() == vec![row.clone()]
    })
    .await;
    until("B hears it", async || heard_b.lock().contains(&note.id)).await;
    assert!(heard_a.lock().is_empty(), "A's own write is not news on A");

    row.run = "r_2".into();
    row.exit = Some(1);
    row.text = "error".into();
    a.store.put_cell_latest(&row).await.unwrap();
    until("newer run on B", async || {
        b.store.cell_latest(&note.id).await.unwrap() == vec![row.clone()]
    })
    .await;

    a.store.delete(&note.id).await.unwrap();
    assert!(a.store.cell_latest(&note.id).await.unwrap().is_empty());
    until("delete on B", async || {
        b.store.cell_latest(&note.id).await.unwrap().is_empty()
    })
    .await;
}

/// One `cell_latest` row of `note`'s `preview` cell: run `run` on engine `engine`.
fn latest(note: &DocMeta, run: &str, engine: &str) -> CellLatest {
    CellLatest {
        id: format!("{}:preview", note.id),
        doc: note.id.clone(),
        cell: "preview".into(),
        run: run.into(),
        state: "finished".into(),
        exit: Some(0),
        secs: Some(33.0),
        expect: Some("✓".into()),
        summary: None,
        code_sha: "abc".into(),
        ended_ms: 5,
        text: "3 to update".into(),
        engine: engine.into(),
    }
}

/// 184 §6.4 (W-h), §13 #17: two engines upsert one cell's row while cut off from each other, the
/// second write later. After the heal both hold that later run whole — its run, exit and text —
/// never the earlier run's text under the later run's id (the per-column merge of the old columns).
#[tokio::test]
async fn two_engines_runs_of_one_cell_never_mix() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    link(&switch, &a, &b, true);
    let note = meta(Binding::Note, "Runbook");
    a.store.create(&note).await.unwrap();
    let base = latest(&note, "r_1", "A");
    a.store.put_cell_latest(&base).await.unwrap();
    until("base on B", async || {
        b.store.cell_latest(&note.id).await.unwrap() == vec![base.clone()]
    })
    .await;

    link(&switch, &a, &b, false);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A's run changes the text; B's, later, changes the exit and keeps the old text.
    let on_a = CellLatest {
        run: "r_a".into(),
        text: "from A".into(),
        ended_ms: 6,
        ..base.clone()
    };
    a.store.put_cell_latest(&on_a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await; // past a capture poll: B's HLC is later
    let on_b = CellLatest {
        run: "r_b".into(),
        exit: Some(1),
        ended_ms: 7,
        engine: "B".into(),
        ..base.clone()
    };
    b.store.put_cell_latest(&on_b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;

    link(&switch, &a, &b, true);
    for (name, n) in [("A", &a), ("B", &b)] {
        until(&format!("B's run whole on {name}"), async || {
            n.store.cell_latest(&note.id).await.unwrap() == vec![on_b.clone()]
        })
        .await;
    }
}
