//! 144 §3.3/§3.5 (D14): the four document tables ride `replica::attach` — a notebook, its items and a
//! source row written on engine A show up on engine B's store within the capture→feed hop; a soft
//! removal on A and a revival on B merge as column writes (no row ever deleted). Two in-process
//! engines over a `tokio::io::duplex` transport, the harness `replica/tests/sync_loopback.rs` uses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use document::store::{NotebookRow, Store};
use document::{
    AgentStyle, Binding, DocId, DocMeta, ItemKind, NotebookItem, SourceOrigin, SourceStatus,
};
use parking_lot::Mutex;
use replica::{ByteStream, Engine, PeerEvent, PeerId, Transport};
use tokio::sync::{broadcast, mpsc};

type Inbound = (PeerId, Box<dyn ByteStream>);

#[derive(Default)]
struct Switch {
    inboxes: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>,
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
        self.switch
            .inboxes
            .lock()
            .keys()
            .filter(|p| **p != self.me)
            .cloned()
            .collect()
    }

    async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
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
    transport: Arc<Loop>,
    _dir: tempfile::TempDir,
}

async fn node(name: &str, switch: &Arc<Switch>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    engine
        .attach(store.pool(), document::store::TABLES)
        .await
        .unwrap();
    let transport = Loop::new(name, switch);
    engine.register_transport(transport.clone());
    Node {
        store,
        _engine: engine,
        transport,
        _dir: dir,
    }
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
