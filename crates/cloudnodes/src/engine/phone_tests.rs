//! The phone-only create path (224 §16.12): no mesh ticket, the one-time link claim's lifecycle,
//! resume from the provider after a restart. No mesh at all (as on iOS), recorded HTTP only.

use super::phone::claim_row;
use super::*;
use crate::creds::tests::MemStore;
use crate::providers::http::testing::FakeTransport;
use crate::providers::{Created, Say};

/// A provider that records its calls and the ticket it got; `exists` = the server is already
/// there (a create the app's freeze cut off).
#[derive(Default)]
struct Prov {
    calls: Mutex<Vec<String>>,
    tickets: Mutex<Vec<String>>,
    exists: Mutex<bool>,
    fail_creates: Mutex<u32>,
}

#[async_trait]
impl Provider for Prov {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Digitalocean
    }
    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        self.calls.lock().push(format!("create {}", req.name));
        {
            let mut f = self.fail_creates.lock();
            if *f > 0 {
                *f -= 1;
                return Err(CloudError("connection reset".into()));
            }
        }
        self.tickets.lock().push(req.ticket.clone());
        say(Stage::Server, "droplet");
        say(Stage::Storage, "volume attached");
        *self.exists.lock() = true;
        Ok(Created {
            server_id: Some("42".into()),
            ip: Some("203.0.113.7".into()),
            size_slug: "s-1vcpu-2gb".into(),
            ..Created::default()
        })
    }
    async fn status(&self, _: &str) -> Result<Option<ProviderStatus>> {
        self.calls.lock().push("status".into());
        Ok(self.exists.lock().then(|| ProviderStatus {
            server_state: Some("active".into()),
            ip: Some("203.0.113.7".into()),
            size_slug: Some("s-1vcpu-2gb".into()),
            ..ProviderStatus::default()
        }))
    }
    async fn list(&self) -> Result<Vec<Listed>> {
        Ok(Vec::new())
    }
    async fn restart(&self, _: &str) -> Result<()> {
        Ok(())
    }
    async fn resize(&self, _: &str, gb: u8) -> Result<String> {
        Ok(format!("s-{gb}"))
    }
    async fn destroy(&self, name: &str, _: bool) -> Result<Vec<String>> {
        self.calls.lock().push(format!("destroy {name}"));
        Ok(vec![])
    }
}

/// The iOS engine's mesh: not running. A remote ticket must never be asked for.
struct NoMesh;

#[async_trait]
impl MeshView for NoMesh {
    async fn remote_ticket(&self, _: u64) -> Result<String> {
        panic!("a phone-only create must not mint a mesh ticket");
    }
    async fn peers(&self) -> Result<Vec<MeshPeer>> {
        Err(CloudError("the mesh is not running on this engine".into()))
    }
    async fn remove(&self, _: &str) -> Result<()> {
        Err(CloudError("the mesh is not running on this engine".into()))
    }
    async fn self_id(&self) -> Result<String> {
        Err(CloudError("the mesh is not running on this engine".into()))
    }
    async fn call(
        &self,
        _: &str,
        _: &str,
        _: Value,
        _: Duration,
    ) -> std::result::Result<Option<Value>, crate::mesh_view::CallError> {
        Ok(None)
    }
    async fn desktop(&self, _: &str) -> Result<DesktopOpen> {
        Err(CloudError("no mesh".into()))
    }
}

struct Conn(Arc<Prov>);

#[async_trait]
impl Connect for Conn {
    fn open(&self, _: ProviderKind, _: Option<&StoredCred>, _: Option<&K8sOptions>) -> Result<Arc<dyn Provider>> {
        Ok(self.0.clone())
    }
    async fn check(&self, _: ProviderKind, _: &StoredCred) -> Result<()> {
        Ok(())
    }
    async fn sizes(&self, kind: ProviderKind, _: &StoredCred, _: Option<&str>) -> Result<Vec<SizeOption>> {
        Ok(providers::sizes(kind))
    }
}

struct Rig {
    cloud: Arc<Cloud>,
    prov: Arc<Prov>,
    http: Arc<FakeTransport>,
    store: Arc<MemStore>,
    events: Arc<Mutex<Vec<CloudEvent>>>,
    dir: tempfile::TempDir,
}

fn pace() -> Pacing {
    Pacing {
        poll: Duration::from_millis(5),
        reachable_wait: Duration::from_millis(5),
        grace_ms: 0,
        oauth_every: Duration::ZERO,
        oauth_tries: 1,
        sizes_ttl: Duration::from_secs(3600),
        vault_wait: Duration::from_millis(5),
    }
}

/// A fresh service on `dir` (a second one on the same dir = the engine after a restart).
async fn rig_on(dir: tempfile::TempDir, prov: Arc<Prov>, store: Arc<MemStore>) -> Rig {
    let http = FakeTransport::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let cloud = Arc::new(Cloud::new(
        Records::new(dir.path().join("cloudnodes")),
        Arc::new(NoMesh),
        Creds {
            store: store.clone(),
            http: http.clone(),
            relay: "https://relay.test".into(),
        },
        Arc::new(Conn(prov.clone())),
        Arc::new(move |e| ev.lock().push(e)),
        pace(),
        None,
    ));
    cloud.set_own_mesh(true);
    Rig {
        cloud,
        prov,
        http,
        store,
        events,
        dir,
    }
}

async fn rig() -> Rig {
    let r = rig_on(
        tempfile::tempdir().unwrap(),
        Arc::new(Prov::default()),
        Arc::new(MemStore::default()),
    )
    .await;
    r.cloud
        .set_token(ProviderKind::Digitalocean, "dop_v1_x".into())
        .await
        .unwrap();
    r
}

fn spec(name: &str) -> NodeSpec {
    NodeSpec {
        provider: ProviderKind::Digitalocean,
        name: name.into(),
        region: "lon1".into(),
        memory_gb: 2,
        volume_gb: None,
        k8s: None,
        desktop: None,
    }
}

async fn settle(cloud: &Cloud, name: &str) -> Record {
    for _ in 0..1000 {
        let r = cloud.records.get(name).unwrap().unwrap();
        if r.state != NodeState::Creating {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{name} never settled");
}

const ENGINE: &str = "CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk"; // b64url([9; 32])

/// What the node parks on the relay, with the MAC of `claim` (or a forged one).
fn park(r: &Rig, claim: &link::claim::Claim, mac: Option<&str>) {
    let mac = mac.map_or_else(|| claim.mac(ENGINE, "lab"), str::to_owned);
    r.http.on(
        "GET",
        &format!("/claim/{}", claim.id()),
        200,
        json!({"engine": ENGINE, "name": "lab", "mac": mac}),
    );
}

/// Every place the PSK must never be: record files, events, the provider's ticket aside.
fn assert_psk_nowhere(r: &Rig, psk: &[u8; 32]) {
    use base64::Engine as _;
    let forms = [
        psk.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(psk),
        base64::engine::general_purpose::STANDARD.encode(psk),
    ];
    let mut haystack = String::new();
    for e in std::fs::read_dir(r.dir.path().join("cloudnodes")).unwrap().flatten() {
        haystack.push_str(&std::fs::read_to_string(e.path()).unwrap());
    }
    haystack.push_str(&format!("{:?}", r.events.lock()));
    haystack.push_str(&serde_json::to_string(&*r.events.lock()).unwrap());
    for f in &forms {
        assert!(!haystack.contains(f.as_str()), "the claim's PSK leaked");
    }
}

#[tokio::test]
async fn phone_only_create_uses_a_link_claim_and_pairs_once() {
    let r = rig().await;
    let pw = Some(Zeroizing::new("correct horse battery".to_owned()));
    assert!(
        r.cloud.create(spec("lab"), pw).await.is_err(),
        "the master password never goes through a phone-only create"
    );
    let node = r.cloud.create(spec("lab"), None).await.unwrap();
    let link = node.link.clone().expect("a phone-only node carries its link state");
    assert!(!link.paired && link.engine.is_none());
    let claim = r.cloud.load_claim("lab").unwrap();
    assert_eq!(link.claim_id, claim.id());
    assert!(claim.expires_ms <= now_ms() + phone::CLAIM_TTL_MS);
    assert_eq!(claim.relay.as_deref(), Some("wss://relay.test"), "the node dials the phone's relay");

    // The provider gets the claim, not a mesh ticket.
    for _ in 0..200 {
        if !r.prov.tickets.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let ticket = r.prov.tickets.lock()[0].clone();
    assert!(ticket.starts_with("L1.") && !ticket.starts_with("1."), "{}", &ticket[..3]);
    assert_eq!(link::claim::Claim::parse(&ticket).unwrap().psk(), claim.psk());
    assert!(crate::providers::userdata::render("lab", "/dev/x", &ticket, catalog::MANIFEST, true).is_ok());

    // Still waiting: the relay has nothing yet.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(r.cloud.records.get("lab").unwrap().unwrap().state, NodeState::Creating);
    assert!(r.cloud.link_pairing("lab").await.is_err(), "not online yet");

    park(&r, &claim, None);
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.state, NodeState::Ready, "{:?}", rec.error);
    let l = rec.link.clone().unwrap();
    assert_eq!((l.engine.as_deref(), l.engine_name.as_deref()), (Some(ENGINE), Some("lab")));
    assert_psk_nowhere(&r, claim.psk());

    let s = r.cloud.link_pairing("lab").await.unwrap();
    let p = link::pairing::parse(&s).unwrap();
    assert_eq!(p.secret, *claim.psk());
    assert_eq!(p.relay_host, "relay.test");
    assert!(p.secure);
    assert_eq!(p.pending, None, "the node's engine identity, not a mesh pending row");

    let node = r.cloud.link_paired("lab").await.unwrap();
    assert!(node.link.unwrap().paired);
    assert!(r.store.0.lock().get(&claim_row("lab")).is_none(), "the PSK is gone here");
    assert!(r.cloud.link_pairing("lab").await.is_err(), "single use");
    assert!(r.http.lines().iter().all(|l| !l.contains("oauth")));
}

#[tokio::test]
async fn a_forged_relay_answer_is_ignored_and_the_claim_expires() {
    let r = rig().await;
    r.cloud.create(spec("lab"), None).await.unwrap();
    let claim = r.cloud.load_claim("lab").unwrap();
    park(&r, &claim, Some(&link::claim::Claim::from_parts([1; 32], 0, None).mac(ENGINE, "lab")));
    // Shorten the claim: rewrite the vault row with an expiry just ahead.
    let soon = now_ms() + 80;
    r.store.0.lock().insert(
        claim_row("lab"),
        (
            9,
            json!({"psk": base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, claim.psk()), "expires_ms": soon, "relay": null}).to_string(),
        ),
    );
    // The job read the claim already; a resumed one sees the short expiry.
    for h in r.cloud.jobs.lock().drain() {
        h.1.abort();
    }
    r.cloud.resume();
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.state, NodeState::Failed);
    assert!(rec.error.unwrap().contains("30-minute link claim expired"));
    assert!(rec.link.unwrap().engine.is_none(), "the forged answer was not taken");
    assert!(r.store.0.lock().get(&claim_row("lab")).is_none(), "a failed job forgets the PSK");
}

/// iOS killed the app after the record and the claim were written, before the provider call
/// returned: the relaunched engine asks the provider, finds the server, does not create again.
#[tokio::test]
async fn resume_after_restart_finds_the_server_at_the_provider() {
    let r = rig().await;
    let link = r.cloud.mint_claim("lab").await.unwrap();
    r.cloud
        .records
        .put(&Record {
            spec: spec("lab"),
            created: None,
            state: NodeState::Creating,
            stage: Some(Stage::Server),
            error: None,
            peer_id: None,
            peers_before: vec![],
            ticket_expires_ms: None,
            created_ms: now_ms(),
            vault: None,
            link: Some(link),
        })
        .unwrap();
    *r.prov.exists.lock() = true;
    let claim = r.cloud.load_claim("lab").unwrap();
    // The relaunch: a new service over the same records and vault.
    let Rig { prov, store, dir, .. } = r;
    let r = rig_on(dir, prov, store).await;
    park(&r, &claim, None);
    r.cloud.resume();
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.state, NodeState::Ready, "{:?}", rec.error);
    assert!(rec.created.is_some());
    assert!(
        !r.prov.calls.lock().iter().any(|c| c.starts_with("create")),
        "{:?}",
        r.prov.calls.lock()
    );
}

/// A create call the freeze cut off (an error) is retried after asking the provider again.
#[tokio::test]
async fn a_cut_off_create_is_retried_once_the_provider_says_nothing_exists() {
    let r = rig().await;
    *r.prov.fail_creates.lock() = 1;
    r.cloud.create(spec("lab"), None).await.unwrap();
    let claim = r.cloud.load_claim("lab").unwrap();
    park(&r, &claim, None);
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.state, NodeState::Ready, "{:?}", rec.error);
    let calls = r.prov.calls.lock().clone();
    assert_eq!(calls.iter().filter(|c| c.starts_with("create")).count(), 2, "{calls:?}");
}

#[tokio::test]
async fn destroy_forgets_the_claim() {
    let r = rig().await;
    r.cloud.create(spec("lab"), None).await.unwrap();
    assert!(r.store.0.lock().get(&claim_row("lab")).is_some());
    r.cloud.destroy("lab", false).await.unwrap();
    assert!(r.store.0.lock().get(&claim_row("lab")).is_none());
}
