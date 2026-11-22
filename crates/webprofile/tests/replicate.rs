//! 209 §10 P1: `web_sites` rides `replica::attach`. Two in-process engines with their OWN vault
//! files sharing one mesh keyring (B adopted A's, as pairing does) over a `tokio::io::duplex`
//! transport, the harness of `crates/document/tests/replicate.rs`:
//!
//! - row ids come from the mesh key, so B finds A's rows by site (B1);
//! - one row per cookie: two engines writing different cookies of one site while apart both keep
//!   theirs, and a removal on one beats an older set on the other (B2);
//! - a peer's rows reach B's bus as `WebProfileChanged` (209 D14 on every engine);
//! - B's save merges back on A; sign out everywhere on A makes B's push stale.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::Spawn as _;
use parking_lot::Mutex;
use replica::{ByteStream, Engine, PeerEvent, PeerId, ReplicaHandle, Transport};
use tokio::sync::{broadcast, mpsc};
use webprofile::{
    Cookie, CookieChange, Profile, Push, WebProfileActor, WebProfileChanged, merge, store,
};

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

/// One engine: its own vault file, `webprofile.sqlite` and replica engine. Not on the network
/// until [`connect`].
struct Node {
    name: &'static str,
    vault: Arc<secrets::Vault>,
    profile: Profile,
    engine: Arc<Engine>,
    replica: ReplicaHandle,
    dir: tempfile::TempDir,
}

async fn node(name: &'static str, keyring_of: Option<&secrets::Vault>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        secrets::Vault::open(&dir.path().join("vault"), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    if let Some(owner) = keyring_of {
        vault.adopt_keyring(owner.keyring()).await.unwrap();
    }
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let profile = Profile::open(&dir.path().join(store::DB), vault.clone())
        .await
        .unwrap();
    let replica = engine.attach(profile.pool(), store::TABLES).await.unwrap();
    Node {
        name,
        vault,
        profile,
        engine,
        replica,
        dir,
    }
}

fn connect(switch: &Arc<Switch>, nodes: [&Node; 2]) {
    let loops = nodes.map(|n| {
        let t = Loop::new(n.name, switch);
        n.engine.register_transport(t.clone());
        t
    });
    let _ = loops[0]
        .events
        .send(PeerEvent::Connected(loops[1].me.clone()));
    let _ = loops[1]
        .events
        .send(PeerEvent::Connected(loops[0].me.clone()));
}

async fn until(what: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn cookie(name: &str, value: &str) -> Cookie {
    Cookie {
        name: name.into(),
        value: value.into(),
        domain: "app.example.com".into(),
        host_only: true,
        path: "/".into(),
        expires: None,
        secure: true,
        http_only: true,
        same_site: "Lax".into(),
        partitioned: false,
    }
}

fn push(writer: &str, epoch: u64, changes: Vec<CookieChange>) -> Push {
    Push {
        writer: writer.into(),
        epoch,
        changes,
        storage: vec![],
    }
}

/// Push on `n` and capture the write now, so its HLC is stamped at this moment.
async fn write(n: &Node, msg: Push) {
    n.profile.push(msg).await.unwrap();
    n.replica.flush().await.unwrap();
}

async fn names(p: &Profile) -> Vec<(String, String)> {
    let load = p.load(Some(&["example.com".to_owned()])).await.unwrap();
    let mut v: Vec<(String, String)> = load
        .sites
        .iter()
        .flat_map(|s| s.jar.cookies.iter())
        .map(|c| (c.name.clone(), c.value.clone()))
        .collect();
    v.sort();
    v
}

#[derive(kameo::Actor)]
struct Heard(Arc<Mutex<Vec<WebProfileChanged>>>);

impl kameo::message::Message<WebProfileChanged> for Heard {
    type Reply = ();
    async fn handle(
        &mut self,
        m: WebProfileChanged,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) {
        self.0.lock().push(m);
    }
}

#[tokio::test]
async fn per_cookie_rows_replicate_between_engines_with_their_own_vaults() {
    let a = node("A", None).await;
    let b = node("B", Some(&a.vault)).await;
    assert_eq!(*a.vault.mesh_key(), *b.vault.mesh_key());
    // B1: no per-engine secret: both engines name a site's rows the same.
    assert_eq!(
        a.profile.site_id("example.com"),
        b.profile.site_id("example.com")
    );

    // B's bus hears a peer's rows (209 D14): its actor over the same file, fed by the feed.
    let rt = runtime::Runtime::new(figment::Figment::new(), b.dir.path().to_path_buf());
    webprofile::declare_wire(&rt).await.unwrap();
    let b_store = Profile::open(&b.dir.path().join(store::DB), b.vault.clone())
        .await
        .unwrap();
    let b_actor = WebProfileActor::spawn(WebProfileActor::new(Some(rt.clone()), Ok(b_store)));
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = Heard::spawn_with_mailbox(Heard(heard.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Heard, WebProfileChanged>(&sink).await;
    tokio::spawn(webprofile::watch_replica(b.replica.clone(), b_actor));

    // Apart: A and B each set a different cookie of the same site; B sets `c`, then A (later)
    // removes `c`, which A never saw.
    let set = |c: Cookie| CookieChange::Set { cookie: c };
    write(&a, push("tile:iPad", 0, vec![set(cookie("a", "from-a"))])).await;
    write(&b, push("obscura:B", 0, vec![set(cookie("b", "from-b"))])).await;
    write(&b, push("obscura:B", 0, vec![set(cookie("c", "old"))])).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let remove_c = CookieChange::Remove {
        name: "c".into(),
        domain: "app.example.com".into(),
        path: "/".into(),
    };
    write(&a, push("tile:Mac", 0, vec![remove_c])).await;

    let switch = Arc::new(Switch::default());
    connect(&switch, [&a, &b]);
    let both = vec![
        ("a".to_owned(), "from-a".to_owned()),
        ("b".to_owned(), "from-b".to_owned()),
    ];
    until("A and B converge per cookie", async || {
        names(&a.profile).await == both && names(&b.profile).await == both
    })
    .await;
    until("B's bus hears example.com", async || {
        heard
            .lock()
            .iter()
            .any(|e| e.sites == ["example.com"] && e.epoch == 0)
    })
    .await;

    // 209 §5 step 6: B's agent browser saves a refreshed cookie as a delta; A has it, and A's
    // other cookie is untouched.
    let loaded = b.profile.load(None).await.unwrap();
    let site = &loaded.sites[0];
    let mut after = site.jar.cookies.clone();
    after.iter_mut().find(|c| c.name == "b").unwrap().value = "refreshed".into();
    let changes = merge::diff(&site.jar.cookies, &after);
    assert_eq!(changes.len(), 1);
    write(&b, push("obscura:B", loaded.epoch, changes)).await;
    until("A sees the refresh", async || {
        names(&a.profile).await
            == vec![
                ("a".to_owned(), "from-a".to_owned()),
                ("b".to_owned(), "refreshed".to_owned()),
            ]
    })
    .await;
    assert_eq!(
        a.profile.load(None).await.unwrap().sites[0].writer,
        "obscura:B"
    );

    // Sign out everywhere on A reaches B: B's rows are gone, its push is stale.
    a.profile.set_epoch(1).await.unwrap();
    a.replica.flush().await.unwrap();
    until("B sees epoch 1", async || {
        b.profile.epoch().await.unwrap() == 1
    })
    .await;
    until("B's rows are tombstones", async || {
        b.profile.load(None).await.unwrap().sites.is_empty()
    })
    .await;
    assert_eq!(
        b.profile
            .push(push("obscura:B", 0, vec![set(cookie("d", "three"))]))
            .await,
        Err(webprofile::ProfileError::StaleEpoch { current: 1 })
    );
}
