//! 209 §10 P1: `web_sites` rides `replica::attach` — a site row the tile pushes on engine A is read,
//! opened and loaded on engine B (same mesh keyring and salt), and B's save merges back on A. Two
//! in-process engines over a `tokio::io::duplex` transport, the harness of
//! `crates/document/tests/replicate.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use replica::{ByteStream, Engine, PeerEvent, PeerId, Transport};
use tokio::sync::{broadcast, mpsc};
use webprofile::{Cookie, CookieChange, Profile, Push, merge, store};

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
    profile: Profile,
    _engine: Arc<Engine>,
    transport: Arc<Loop>,
    _dir: tempfile::TempDir,
}

/// One engine: its own `webprofile.sqlite` and replica engine; the vault is the mesh's shared
/// keyring (the mesh key and the salt replicate with the vault in a real mesh).
async fn node(name: &str, switch: &Arc<Switch>, vault: &Arc<secrets::Vault>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let profile = Profile::open(&dir.path().join(store::DB), vault.clone())
        .await
        .unwrap();
    engine.attach(profile.pool(), store::TABLES).await.unwrap();
    let transport = Loop::new(name, switch);
    engine.register_transport(transport.clone());
    Node {
        profile,
        _engine: engine,
        transport,
        _dir: dir,
    }
}

async fn until(what: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn host_sid(value: &str) -> Cookie {
    Cookie {
        name: "__Host-sid".into(),
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

#[tokio::test]
async fn a_site_row_replicates_between_two_engines() {
    let keyring = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        secrets::Vault::open(keyring.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch, &vault).await;
    let b = node("B", &switch, &vault).await;
    let _ = a
        .transport
        .events
        .send(PeerEvent::Connected(b.transport.me.clone()));
    let _ = b
        .transport
        .events
        .send(PeerEvent::Connected(a.transport.me.clone()));

    // 209 §5 steps 1-3: the iPad pushes its sign-in to engine A.
    a.profile
        .push(Push {
            writer: "tile:iPad".into(),
            epoch: 0,
            changes: vec![CookieChange::Set {
                cookie: host_sid("one"),
            }],
            storage: vec![],
        })
        .await
        .unwrap();

    // Step 4: engine B loads it, `host_only` and `HttpOnly` intact.
    until("B sees example.com", async || {
        !b.profile.load(None).await.unwrap().sites.is_empty()
    })
    .await;
    let loaded = b.profile.load(None).await.unwrap();
    let site = &loaded.sites[0];
    assert_eq!(site.jar.site, "example.com");
    assert_eq!(site.jar.cookies, vec![host_sid("one")]);
    assert_eq!(site.writer, "tile:iPad");
    assert_eq!(b.profile.sites().await.unwrap()[0].cookies, 1);

    // Step 6: the agent browser on B saves a refreshed cookie as a delta; A has it.
    let changes = merge::diff(&site.jar.cookies, &[host_sid("two")]);
    b.profile
        .push(Push {
            writer: "obscura:B".into(),
            epoch: loaded.epoch,
            changes,
            storage: vec![],
        })
        .await
        .unwrap();
    until("A sees the refresh", async || {
        a.profile.load(None).await.unwrap().sites[0].jar.cookies == vec![host_sid("two")]
    })
    .await;
    assert_eq!(
        a.profile.load(None).await.unwrap().sites[0].writer,
        "obscura:B"
    );

    // Sign out everywhere on A reaches B: B's rows of the old epoch are dead, its push is stale.
    a.profile.set_epoch(1).await.unwrap();
    until("B sees epoch 1", async || {
        b.profile.epoch().await.unwrap() == 1
    })
    .await;
    assert!(b.profile.load(None).await.unwrap().sites.is_empty());
    assert_eq!(
        b.profile
            .push(Push {
                writer: "obscura:B".into(),
                epoch: 0,
                changes: vec![CookieChange::Set {
                    cookie: host_sid("three"),
                }],
                storage: vec![],
            })
            .await,
        Err(webprofile::ProfileError::StaleEpoch { current: 1 })
    );
}
