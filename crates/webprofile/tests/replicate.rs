//! 209 §10 P1: `web_sites` rides `replica::attach`. Two in-process engines with their OWN vault
//! files sharing one mesh keyring (B adopted A's, as pairing does) over a `tokio::io::duplex`
//! transport, the harness of `crates/document/tests/replicate.rs`:
//!
//! - row ids come from the mesh key, so B finds A's rows by site (B1);
//! - one row per cookie: two engines writing different cookies of one site while apart both keep
//!   theirs, and a removal on one beats an older set on the other (B2);
//! - a peer's rows reach B's bus as `WebProfileChanged` (209 D14 on every engine);
//! - B's save merges back on A; sign out everywhere on A makes B's push stale;
//! - P2: a forget on A reaches B's bus naming the site, and a cookie B wrote before the forget,
//!   which A had not seen, stays dead on both;
//! - P2: a mesh key rotation across two engines — one adopting the new key late and writing
//!   under the old one meanwhile — loses no row, and the old key retires on both.

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
        basis_ms: None,
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

/// An actor over its own store on `n`'s file, fed by `n`'s replica feed and vault changes, as
/// the plugin wires it; `rt` publishes on a bus when given.
fn serve(n: &Node, rt: Option<runtime::Runtime>, store: Profile) -> webprofile::Handle {
    let actor = WebProfileActor::spawn(WebProfileActor::new(rt, Ok(store)));
    tokio::spawn(webprofile::watch_replica(n.replica.clone(), actor.clone()));
    tokio::spawn(webprofile::watch_keys(n.vault.clone(), actor.clone()));
    webprofile::Handle::from_actor(actor)
}

async fn store_of(n: &Node) -> Profile {
    Profile::open(&n.dir.path().join(store::DB), n.vault.clone())
        .await
        .unwrap()
}

/// Rows of `n`'s file that open only under a retained mesh key.
async fn under_old_key(n: &Node) -> usize {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, sealed FROM web_sites WHERE sealed IS NOT NULL")
            .fetch_all(n.profile.pool())
            .await
            .unwrap();
    rows.iter()
        .filter(|(id, sealed)| {
            use base64::Engine as _;
            let b = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(sealed)
                .unwrap();
            n.vault
                .reseal_with_mesh_key(format!("web_sites\n{id}").as_bytes(), &b)
                .is_some()
        })
        .count()
}

async fn pulled(wp: &webprofile::Handle) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = wp
        .pull(Some(vec!["example.com".into()]))
        .await
        .unwrap()
        .cookies
        .into_iter()
        .map(|c| (c.cookie.name, c.cookie.value))
        .collect();
    v.sort();
    v
}

/// 209 §4.4 `web_profile_forget` across engines: A forgets a site; B's bus hears it named (the
/// marker row names it, the tombstones alone could not), B's rows of it are gone, and a cookie B
/// wrote before the forget while apart — a row A never saw, so A has no tombstone for it — is
/// dead on both when it arrives.
#[tokio::test]
async fn a_forget_reaches_the_other_engine_and_an_earlier_write_stays_dead() {
    let a = node("A", None).await;
    let b = node("B", Some(&a.vault)).await;
    let set = |c: Cookie| CookieChange::Set { cookie: c };
    write(&a, push("tile:iPad", 0, vec![set(cookie("sid", "from-a"))])).await;
    write(&b, push("tile:Mac", 0, vec![set(cookie("late", "from-b"))])).await;
    tokio::time::sleep(Duration::from_millis(5)).await;

    let rt = runtime::Runtime::new(figment::Figment::new(), b.dir.path().to_path_buf());
    webprofile::declare_wire(&rt).await.unwrap();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = Heard::spawn_with_mailbox(Heard(heard.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Heard, WebProfileChanged>(&sink).await;
    let b_wp = serve(&b, Some(rt), store_of(&b).await);
    let a_wp = serve(&a, None, store_of(&a).await);
    // A client last pulled (from either engine) before the forget.
    let basis = store::now_ms();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        a_wp.forget("app.example.com").await.unwrap().site,
        "example.com"
    );
    a.replica.flush().await.unwrap();

    let late_id =
        store::Ids::of(&a.vault.mesh_key()).cookie("example.com", "late", "app.example.com", "/");
    connect(&Arc::new(Switch::default()), [&a, &b]);
    until("A received B's earlier row", async || {
        sqlx::query_scalar::<_, String>("SELECT id FROM web_sites WHERE id = ?")
            .bind(&late_id)
            .fetch_optional(a.profile.pool())
            .await
            .unwrap()
            .is_some()
    })
    .await;
    until("B's bus hears the forgotten site", async || {
        heard.lock().iter().any(|e| e.sites == ["example.com"])
    })
    .await;
    until("B's rows of the site are gone", async || {
        pulled(&b_wp).await.is_empty()
    })
    .await;
    assert_eq!(
        pulled(&a_wp).await,
        vec![],
        "B's earlier write is dead on A"
    );
    assert!(a_wp.sites().await.unwrap().is_empty());
    assert!(b_wp.sites().await.unwrap().is_empty());

    // 209 §4.1: the offline client comes back and pushes its old cookie with the basis of its
    // last pull, or with none (an old client): refused on the forgetting engine and on the
    // engine that only received the forget; B's pull tells it the site was forgotten.
    let full = b_wp.pull(None).await.unwrap();
    assert_eq!(full.forgotten.len(), 1, "{full:?}");
    assert_eq!(full.forgotten[0].site, "example.com");
    assert!(full.forgotten[0].forgotten_ms > basis);
    for (engine, wp) in [("A", &a_wp), ("B", &b_wp)] {
        for basis in [Some(basis), None] {
            let r = wp
                .push(Push {
                    basis_ms: basis,
                    ..push("tile:iPad", 0, vec![set(cookie("sid", "resurrected"))])
                })
                .await
                .unwrap();
            assert_eq!(r.refused, ["example.com"], "{engine} {basis:?}");
            assert!(r.sites.is_empty(), "{engine} {basis:?}");
        }
    }
    assert!(pulled(&a_wp).await.is_empty() && pulled(&b_wp).await.is_empty());
}

/// 209 §4.1: the epoch never goes down. A signs out everywhere twice while B, apart, signs out
/// once later (its epoch row wins last-writer-wins with the lower number): A keeps answering 2,
/// writes 2 again at its next maintenance, and B reaches 2.
#[tokio::test]
async fn the_epoch_never_goes_down_when_two_engines_clear_apart() {
    let a = node("A", None).await;
    let b = node("B", Some(&a.vault)).await;
    assert_eq!(a.profile.clear().await.unwrap(), 1);
    assert_eq!(a.profile.clear().await.unwrap(), 2);
    a.replica.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(b.profile.clear().await.unwrap(), 1);
    b.replica.flush().await.unwrap();

    connect(&Arc::new(Switch::default()), [&a, &b]);
    until("A received B's epoch row", async || {
        let sealed: Option<String> =
            sqlx::query_scalar("SELECT hlc FROM web_sites WHERE id = 'epoch'")
                .fetch_one(a.profile.pool())
                .await
                .unwrap();
        let b_hlc: Option<String> =
            sqlx::query_scalar("SELECT hlc FROM web_sites WHERE id = 'epoch'")
                .fetch_one(b.profile.pool())
                .await
                .unwrap();
        sealed.is_some() && sealed == b_hlc
    })
    .await;
    assert_eq!(a.profile.epoch().await.unwrap(), 2, "A never answers less");
    a.profile.maintain().await.unwrap();
    a.replica.flush().await.unwrap();
    until("B reaches 2", async || {
        b.profile.epoch().await.unwrap() == 2
    })
    .await;
    assert_eq!(a.profile.epoch().await.unwrap(), 2);
    // The next sign out everywhere goes on from the highest.
    assert_eq!(b.profile.clear().await.unwrap(), 3);
}

/// 209 §4.1 rotation across two engines: A mints a new mesh key and moves its rows at once; B,
/// still on the old key, writes a cookie; A moves B's row as it arrives (holding the old key
/// meanwhile) so A's old key can retire without losing it; B adopts the new key late, moves its
/// rows, and its old key retires too. Every cookie still opens on both.
#[tokio::test]
async fn a_key_rotation_across_two_engines_loses_no_row() {
    let a = node("A", None).await;
    let b = node("B", Some(&a.vault)).await;
    let a_wp = serve(&a, None, store_of(&a).await);
    let b_wp = serve(&b, None, store_of(&b).await);
    connect(&Arc::new(Switch::default()), [&a, &b]);
    let set = |c: Cookie| CookieChange::Set { cookie: c };
    a_wp.push(push("tile:iPad", 0, vec![set(cookie("a", "from-a"))]))
        .await
        .unwrap();
    a.replica.flush().await.unwrap();
    until("B has A's cookie", async || {
        pulled(&b_wp).await == [("a".to_owned(), "from-a".to_owned())]
    })
    .await;

    // A rotates (183 §6.9): its watcher moves the rows to the new key's ids at once.
    a.vault
        .rotate("A", vec!["A".into(), "B".into()], vec![], 0, false)
        .await
        .unwrap();
    assert_eq!(a.vault.held_keys().len(), 2);
    until("A moved its rows", async || under_old_key(&a).await == 0).await;

    // B has not adopted the new key: it writes under the old one; A moves the row on arrival.
    b_wp.push(push("obscura:B", 0, vec![set(cookie("lag", "from-b"))]))
        .await
        .unwrap();
    b.replica.flush().await.unwrap();
    let both = vec![
        ("a".to_owned(), "from-a".to_owned()),
        ("lag".to_owned(), "from-b".to_owned()),
    ];
    until("A has B's row under the new key", async || {
        under_old_key(&a).await == 0 && pulled(&a_wp).await == both
    })
    .await;
    // The vault's grace is over: the old key retires on A, and nothing is lost.
    let far = store::now_ms() + secrets::vault::RETIRE_GRACE_MS + 60_000;
    a.vault.maintain(far).await.unwrap();
    assert_eq!(a.vault.held_keys().len(), 1, "A's old key retired");
    assert_eq!(pulled(&a_wp).await, both);

    // B adopts the new key late: its watcher moves its rows; then its old key retires.
    let new_key = a.vault.keyring().into_iter().next().unwrap();
    assert!(b.vault.adopt_key(new_key).await.unwrap());
    until("B moved its rows", async || under_old_key(&b).await == 0).await;
    b.vault.maintain(far).await.unwrap();
    assert_eq!(b.vault.held_keys().len(), 1, "B's old key retired");
    assert_eq!(pulled(&b_wp).await, both);
    assert_eq!(
        a.profile.site_id("example.com"),
        b.profile.site_id("example.com"),
        "both name the site under the new key"
    );
    b.replica.flush().await.unwrap();
    until("A still has both after B's moves arrive", async || {
        pulled(&a_wp).await == both
    })
    .await;
}
