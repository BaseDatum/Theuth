use super::*;
use crate::creds::tests::MemStore;
use crate::providers::http::testing::FakeTransport;
use crate::providers::{Created, Say};
use crate::types::MeshPath;

/// This engine's id in the fake mesh.
const ME: &str = "me";

/// The node's sealed vault as `vault_*` calls over the fake mesh see it.
#[derive(Default)]
struct FakeVault {
    /// `None`: the node's release has no `vault_status` (method not found).
    methods: Option<Vec<(Kind, String)>>,
    /// The node enrols [`ME`] as its unlock device after this many `vault_status` reads.
    enrol_after: Option<u32>,
    /// `vault_add_password` answers an error.
    refuse_password: bool,
}

impl FakeVault {
    /// A fresh sealed-store vault (no method yet) whose node enrols this engine on the second look.
    fn sealed() -> Self {
        FakeVault {
            methods: Some(Vec::new()),
            enrol_after: Some(1),
            refuse_password: false,
        }
    }
}

fn vault_json(methods: &[(Kind, String)]) -> Value {
    let methods: Vec<Value> = methods
        .iter()
        .map(|(k, id)| json!({"kind": k.as_str(), "id": id, "label": id, "created_ms": 1}))
        .collect();
    json!({
        "engine": "node-id", "name": "lab", "state": "unsealed", "unsealed_by": "memory",
        "password_set": methods.iter().any(|m| m["kind"] == "password"),
        "methods": methods, "warning": null, "asking": [], "error": null,
    })
}

#[derive(Default)]
struct FakeProvider {
    calls: Mutex<Vec<String>>,
    listed: Mutex<Vec<Listed>>,
}

#[async_trait]
impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Digitalocean
    }
    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        assert!(
            req.ticket.starts_with("1."),
            "the minted ticket reaches the provider"
        );
        assert!(req.desktop, "the desktop is on by default");
        self.calls.lock().push(format!("create {}", req.name));
        say(Stage::Server, "firewall, droplet");
        say(Stage::Storage, "volume attached");
        Ok(Created {
            server_id: Some("42".into()),
            volume_id: Some("vol".into()),
            extra: Some("fw".into()),
            ip: Some("203.0.113.7".into()),
            size_slug: "s-1vcpu-2gb".into(),
        })
    }
    async fn status(&self, _: &str) -> Result<Option<ProviderStatus>> {
        Ok(None)
    }
    async fn list(&self) -> Result<Vec<Listed>> {
        Ok(self.listed.lock().clone())
    }
    async fn restart(&self, name: &str) -> Result<()> {
        self.calls.lock().push(format!("restart {name}"));
        Ok(())
    }
    async fn resize(&self, name: &str, gb: u8) -> Result<String> {
        self.calls.lock().push(format!("resize {name} {gb}"));
        Ok(format!("s-2vcpu-{gb}gb"))
    }
    async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>> {
        self.calls
            .lock()
            .push(format!("destroy {name} keep={keep_volume}"));
        Ok(vec![format!("Deleted droplet {name}")])
    }
}

/// A mesh whose node shows up after `join_after` reads of the peer set.
#[derive(Default)]
struct FakeMesh {
    peers: Mutex<Vec<MeshPeer>>,
    join_after: Mutex<Option<(u32, MeshPeer)>>,
    tickets: Mutex<Vec<u64>>,
    removed: Mutex<Vec<String>>,
    calls: Mutex<Vec<(String, String, Value)>>,
    vault: Mutex<FakeVault>,
    /// Optional override for `engine_shell`: `Some(None)` = offline, `Some(Some(false))` = old node (`NoMethod`).
    shell_override: Mutex<Option<Option<bool>>>,
}

#[async_trait]
impl MeshView for FakeMesh {
    async fn remote_ticket(&self, ttl_ms: u64) -> Result<String> {
        self.tickets.lock().push(ttl_ms);
        Ok("1.TICKET".into())
    }
    async fn peers(&self) -> Result<Vec<MeshPeer>> {
        let mut j = self.join_after.lock();
        if let Some((n, p)) = j.as_mut() {
            if *n == 0 {
                self.peers.lock().push(p.clone());
                *j = None;
            } else {
                *n -= 1;
            }
        }
        Ok(self.peers.lock().clone())
    }
    async fn remove(&self, id: &str) -> Result<()> {
        self.removed.lock().push(id.into());
        self.peers.lock().retain(|p| p.id != id);
        Ok(())
    }
    async fn self_id(&self) -> Result<String> {
        Ok(ME.into())
    }
    async fn call(
        &self,
        id: &str,
        method: &str,
        params: Value,
        _: Duration,
    ) -> std::result::Result<Option<Value>, CallError> {
        self.calls.lock().push((id.into(), method.into(), params));
        if method.starts_with("vault_") {
            let mut guard = self.vault.lock();
            let v = &mut *guard;
            let Some(methods) = v.methods.as_mut() else {
                return Err(CallError::NoMethod(method.into()));
            };
            match method {
                "vault_status" => match v.enrol_after.as_mut() {
                    Some(0) => {
                        methods.push((Kind::Device, ME.into()));
                        v.enrol_after = None;
                    }
                    Some(n) => *n -= 1,
                    None => {}
                },
                "vault_add_password" if v.refuse_password => {
                    return Err(CallError::Failed(CloudError(
                        "the vault is sealed on lab".into(),
                    )));
                }
                "vault_add_password" => methods.push((Kind::Password, "password".into())),
                _ => {}
            }
            return Ok(Some(vault_json(methods)));
        }
        if method == "engine_shell" {
            if let Some(mode) = *self.shell_override.lock() {
                return match mode {
                    None => Ok(None),
                    Some(false) => Err(CallError::NoMethod("engine_shell".into())),
                    Some(true) => Ok(Some(json!({"shell": "main", "engine": id}))),
                };
            }
            return Ok(Some(json!({"shell": "main", "engine": id})));
        }
        Ok(Some(match method {
            "info" => json!({"version": "2.32.0"}),
            "engine_logs" => json!({"lines": ["a", "b"], "path": "/data/logs/t.log"}),
            _ => Value::Null,
        }))
    }
    async fn desktop(&self, id: &str) -> Result<DesktopOpen> {
        Ok(DesktopOpen {
            ws_url: "ws://127.0.0.1:50001/websockify".into(),
            password: "pw".into(),
            geometry: Some("1280x800".into()),
            engine: id.into(),
        })
    }
}

struct FakeConnect(Arc<FakeProvider>);

#[async_trait]
impl Connect for FakeConnect {
    fn open(
        &self,
        _: ProviderKind,
        cred: Option<&StoredCred>,
        _: Option<&K8sOptions>,
    ) -> Result<Arc<dyn Provider>> {
        assert!(cred.is_some());
        Ok(self.0.clone())
    }
    async fn check(&self, _: ProviderKind, cred: &StoredCred) -> Result<()> {
        if cred.access_token == "bad" {
            return Err(CloudError("401 Unauthorized".into()));
        }
        Ok(())
    }
    async fn sizes(
        &self,
        kind: ProviderKind,
        _: &StoredCred,
        _: Option<&str>,
    ) -> Result<Vec<SizeOption>> {
        // As a live answer would price them (test data, not a price list).
        Ok(providers::sizes(kind)
            .into_iter()
            .map(|mut s| {
                s.monthly_usd = Some(f64::from(s.memory_gb) * 6.0);
                s
            })
            .collect())
    }
}

struct Rig {
    cloud: Arc<Cloud>,
    provider: Arc<FakeProvider>,
    mesh: Arc<FakeMesh>,
    events: Arc<Mutex<Vec<CloudEvent>>>,
    _dir: tempfile::TempDir,
}

fn peer(id: &str, name: &str, reachable: bool) -> MeshPeer {
    MeshPeer {
        id: id.into(),
        name: name.into(),
        reachable,
        path: if reachable {
            MeshPath::Direct
        } else {
            MeshPath::Offline
        },
    }
}

async fn rig(pace: Pacing) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mesh = Arc::new(FakeMesh::default());
    mesh.peers.lock().push(peer("laptop", "laptop", true));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let creds = Creds {
        store: Arc::new(MemStore::default()),
        http: FakeTransport::new(),
        relay: "https://relay.test".into(),
    };
    let cloud = Arc::new(Cloud::new(
        Records::new(dir.path().join("cloudnodes")),
        mesh.clone(),
        creds,
        Arc::new(FakeConnect(provider.clone())),
        Arc::new(move |e| ev.lock().push(e)),
        pace,
        None,
    ));
    cloud
        .set_token(ProviderKind::Digitalocean, " dop_v1_x \n".into())
        .await
        .unwrap();
    Rig {
        cloud,
        provider,
        mesh,
        events,
        _dir: dir,
    }
}

fn fast() -> Pacing {
    Pacing {
        poll: Duration::from_millis(5),
        reachable_wait: Duration::from_millis(50),
        grace_ms: 0,
        oauth_every: Duration::ZERO,
        oauth_tries: 3,
        sizes_ttl: Duration::from_secs(3600),
        vault_wait: Duration::from_millis(50),
    }
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
    for _ in 0..400 {
        let r = cloud.records.get(name).unwrap().unwrap();
        if !matches!(r.state, NodeState::Creating | NodeState::Busy) {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{name} never settled");
}

#[tokio::test]
async fn create_runs_the_stages_in_order_and_ends_ready() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((3, peer("node-id", "lab", true)));
    *r.mesh.vault.lock() = FakeVault::sealed();
    let node = r.cloud.create(spec("lab"), pw()).await.unwrap();
    assert_eq!(node.vault, Some(VaultProtection::Pending));
    assert_eq!(
        (node.state, node.stage),
        (NodeState::Creating, Some(Stage::Server))
    );
    assert_eq!(node.monthly_usd, None, "events carry no price");
    let q = r.cloud.quote(&spec("lab")).await.unwrap();
    assert_eq!(q.total_usd, Some(13.0), "2 GB at the live $12 + 10 GB disk");
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.state, NodeState::Ready, "{:?}", rec.error);
    assert_eq!(rec.peer_id.as_deref(), Some("node-id"));
    assert_eq!(rec.peers_before, ["laptop"]);
    assert_eq!(
        rec.created.as_ref().unwrap().server_id.as_deref(),
        Some("42")
    );
    assert!(rec.ticket_expires_ms.is_some());
    assert_eq!(*r.mesh.tickets.lock(), [TICKET_TTL_MS]);
    assert_eq!(*r.provider.calls.lock(), ["create lab"]);

    // The stages the UI saw, in order, each once.
    let mut stages: Vec<Stage> = Vec::new();
    let mut pcts: Vec<u8> = Vec::new();
    for e in r.events.lock().iter() {
        let s = match e {
            CloudEvent::CloudNodeChanged { node } if node.name == "lab" => node.stage,
            CloudEvent::CloudNodeProgress { stage, pct, .. } => {
                pcts.extend(*pct);
                *stage
            }
            _ => None,
        };
        if let Some(s) = s
            && stages.last() != Some(&s)
        {
            stages.push(s);
        }
    }
    assert_eq!(
        stages,
        [
            Stage::Server,
            Stage::Storage,
            Stage::Install,
            Stage::Join,
            Stage::Vault,
            Stage::Ready
        ]
    );
    assert!(
        pcts.windows(2).all(|w| w[0] <= w[1]),
        "progress never goes back: {pcts:?}"
    );
    assert_eq!(pcts.last(), Some(&100));
    // The ticket is never in an event.
    let all = format!("{:?}", r.events.lock());
    assert!(!all.contains("1.TICKET"));
    assert_eq!(rec.vault, Some(VaultProtection::Protected));
    let lines = progress_lines(&r, "lab");
    assert!(
        lines.contains(&"Protecting the vault".to_owned()),
        "{lines:?}"
    );

    // A second node of the same name is refused; so is a machine name already in the mesh.
    assert!(r.cloud.create(spec("lab"), None).await.is_err());
    r.mesh.peers.lock().push(peer("other", "desk", true));
    assert!(
        r.cloud
            .create(spec("desk"), None)
            .await
            .unwrap_err()
            .0
            .contains("already in the mesh")
    );
}

#[tokio::test]
async fn a_node_that_never_joins_fails_when_its_ticket_expires() {
    let r = rig(fast()).await;
    let node = r.cloud.create(spec("late"), None).await.unwrap();
    assert_eq!(node.state, NodeState::Creating);
    // Expire the ticket under the running job.
    for _ in 0..200 {
        if r.cloud.records.get("late").unwrap().unwrap().stage == Some(Stage::Install) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    r.cloud
        .update("late", |rec| rec.ticket_expires_ms = Some(1))
        .unwrap();
    let rec = settle(&r.cloud, "late").await;
    assert_eq!(rec.state, NodeState::Failed);
    assert!(rec.error.unwrap().contains("pairing ticket expired"));
}

#[tokio::test]
async fn destroy_deletes_and_unpairs_or_keeps_the_volume_and_the_pairing() {
    let r = rig(fast()).await;
    // `create` reads the peer set once before the job: the node appears on the read after.
    *r.mesh.join_after.lock() = Some((1, peer("n1", "keep", true)));
    r.cloud.create(spec("keep"), None).await.unwrap();
    assert_eq!(settle(&r.cloud, "keep").await.state, NodeState::Ready);
    let lines = r.cloud.destroy("keep", true).await.unwrap();
    assert_eq!(lines, ["Deleted droplet keep"]);
    let rec = r.cloud.records.get("keep").unwrap().unwrap();
    assert_eq!(rec.state, NodeState::VolumeKept);
    assert_eq!(
        rec.peer_id.as_deref(),
        Some("n1"),
        "a kept disk keeps its identity"
    );
    assert!(r.mesh.removed.lock().is_empty());

    // Re-created on the kept disk: the old identity coming back counts as joined.
    r.mesh
        .peers
        .lock()
        .iter_mut()
        .for_each(|p| p.reachable = p.id != "n1");
    let rec = {
        r.cloud.create(spec("keep"), None).await.unwrap();
        r.mesh
            .peers
            .lock()
            .iter_mut()
            .for_each(|p| p.reachable = true);
        settle(&r.cloud, "keep").await
    };
    assert_eq!(
        (rec.state, rec.peer_id.as_deref()),
        (NodeState::Ready, Some("n1"))
    );

    let lines = r.cloud.destroy("keep", false).await.unwrap();
    assert_eq!(
        lines,
        ["Deleted droplet keep", "Removed keep from the mesh"]
    );
    assert!(r.cloud.records.get("keep").unwrap().is_none());
    assert_eq!(*r.mesh.removed.lock(), ["n1"]);
    assert!(
        r.events
            .lock()
            .iter()
            .any(|e| matches!(e, CloudEvent::CloudNodeRemoved { name } if name == "keep"))
    );
    assert_eq!(
        *r.provider.calls.lock(),
        [
            "create keep",
            "destroy keep keep=true",
            "create keep",
            "destroy keep keep=false"
        ]
    );
}

#[tokio::test]
async fn list_merges_records_orphans_kept_volumes_and_the_mesh() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((1, peer("n1", "mine", true)));
    r.cloud.create(spec("mine"), None).await.unwrap();
    settle(&r.cloud, "mine").await;
    r.mesh.peers.lock().push(peer("n2", "stray", false));
    let listed = |name: &str, volume_only: bool| Listed {
        name: name.into(),
        server_state: (!volume_only).then(|| "active".into()),
        region: Some("lon1".into()),
        ip: (!volume_only).then(|| "198.51.100.1".into()),
        memory_gb: (!volume_only).then_some(4),
        volume_gb: Some(10),
        volume_only,
    };
    *r.provider.listed.lock() = vec![
        listed("mine", false),
        listed("stray", false),
        listed("old", true),
    ];
    let list = r.cloud.list().await;
    assert!(list.errors.is_empty());
    let names: Vec<(&str, NodeState)> = list
        .nodes
        .iter()
        .map(|n| (n.name.as_str(), n.state.clone()))
        .collect();
    assert_eq!(
        names,
        [
            ("mine", NodeState::Ready),
            ("stray", NodeState::Orphan),
            ("old", NodeState::VolumeKept)
        ]
    );
    let mine = &list.nodes[0];
    assert_eq!(mine.provider_state.as_deref(), Some("active"));
    assert_eq!(
        (mine.reachable, mine.path),
        (Some(true), Some(MeshPath::Direct))
    );
    assert_eq!(
        mine.version.as_deref(),
        Some("2.32.0"),
        "info answered over the mesh"
    );
    assert_eq!(mine.region_label.as_deref(), Some("London"));
    let stray = &list.nodes[1];
    assert_eq!(
        (stray.peer_id.as_deref(), stray.reachable),
        (Some("n2"), Some(false))
    );
    assert_eq!(stray.version, None, "an unreachable node is not asked");
    assert_eq!(stray.monthly_usd, Some(25.0), "4 GB DO $24 + $1 disk");

    // An orphan can be destroyed by name.
    r.cloud.destroy("stray", false).await.unwrap();
    assert_eq!(*r.mesh.removed.lock(), ["n2"]);

    // Terminal and logs ride the mesh to the node's engine with no engine parameter.
    let t = r.cloud.terminal("mine").await.unwrap();
    assert_eq!((t.shell.as_str(), t.engine.as_str()), ("main", "n1"));
    let logs = r.cloud.logs("mine", Some(50)).await.unwrap();
    assert_eq!(logs.lines, ["a", "b"]);
    let calls = r.mesh.calls.lock().clone();
    assert!(calls.contains(&(
        "n1".into(),
        "engine_shell".into(),
        json!([null, null, null, null])
    )));
    assert!(!calls.iter().any(|(_, m, _)| m == "engine_terminal"));
    assert!(calls.contains(&("n1".into(), "engine_logs".into(), json!([null, 50]))));

    // The desktop: on by default; refused for a node created without one.
    assert!(list.nodes[0].desktop);
    assert_eq!(r.cloud.desktop("mine").await.unwrap().engine, "n1");
    r.cloud
        .update("mine", |rec| rec.spec.desktop = Some(false))
        .unwrap();
    assert_eq!(
        r.cloud.desktop("mine").await.unwrap_err().0,
        "desktop is off for mine"
    );
}

#[tokio::test]
async fn terminal_old_node_and_offline_errors_do_not_fall_back_to_engine_terminal() {
    let r = rig(fast()).await;
    r.mesh.peers.lock().push(peer("n1", "legacy", true));

    // Old node without engine_shell returns a clear "update this node" error
    // and never falls back to engine_terminal.
    *r.mesh.shell_override.lock() = Some(Some(false));
    let err = r.cloud.terminal("legacy").await.unwrap_err().0;
    assert_eq!(
        err,
        "legacy is running an older version of theuth that does not support standalone terminals — update this node to open a terminal."
    );
    let calls = r.mesh.calls.lock().clone();
    assert!(calls.iter().any(|(_, m, _)| m == "engine_shell"));
    assert!(
        !calls.iter().any(|(_, m, _)| m == "engine_terminal"),
        "must never fall back to engine_terminal"
    );

    // Offline node returns a clear not-connected error.
    *r.mesh.shell_override.lock() = Some(None);
    let err = r.cloud.terminal("legacy").await.unwrap_err().0;
    assert_eq!(err, "legacy is not connected");
}

#[tokio::test]
async fn restart_and_resize_go_busy_then_back() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((1, peer("n1", "box", true)));
    r.cloud.create(spec("box"), None).await.unwrap();
    settle(&r.cloud, "box").await;
    let n = r.cloud.resize("box", 8).await.unwrap();
    assert_eq!(n.state, NodeState::Busy);
    let rec = settle(&r.cloud, "box").await;
    assert_eq!((rec.state, rec.spec.memory_gb), (NodeState::Ready, 8));
    assert_eq!(rec.created.unwrap().size_slug, "s-2vcpu-8gb");
    r.cloud.update_now("box").await.unwrap();
    settle(&r.cloud, "box").await;
    assert!(r.cloud.resize("box", 3).await.is_err());
    assert_eq!(
        *r.provider.calls.lock(),
        ["create box", "resize box 8", "restart box"]
    );
}

#[tokio::test]
async fn a_bad_pasted_token_is_not_stored() {
    let r = rig(fast()).await;
    let e = r
        .cloud
        .set_token(ProviderKind::Fly, "bad".into())
        .await
        .unwrap_err();
    assert!(e.0.contains("401"));
    assert!(!r.cloud.connected(ProviderKind::Fly));
    assert!(r.cloud.connected(ProviderKind::Digitalocean));
    let info = r.cloud.providers();
    assert_eq!(info.len(), 4);
    assert!(info[0].connected && info[0].oauth);
    assert_eq!(info[0].source, Some(CredentialSource::Pasted));
    assert!(!info[3].available, "Lightsail is coming soon");
    assert!(
        r.cloud
            .set_token(ProviderKind::Kubernetes, "x".into())
            .await
            .is_err()
    );
    r.cloud.forget(ProviderKind::Digitalocean).await.unwrap();
    assert!(!r.cloud.connected(ProviderKind::Digitalocean));
    let stored = format!("{:?}", r.events.lock());
    assert!(!stored.contains("dop_v1_x"), "tokens never reach an event");
}

/// A recorded `GET /v2/sizes` answer (trimmed to the fields read, one size per family).
fn do_sizes_body() -> Value {
    json!({
        "sizes": [
            {"slug": "s-1vcpu-1gb", "memory": 1024, "vcpus": 1, "disk": 25, "price_monthly": 6.0,
             "price_hourly": 0.00893, "regions": ["lon1", "nyc3"], "available": true},
            {"slug": "s-1vcpu-2gb", "memory": 2048, "vcpus": 1, "disk": 50, "price_monthly": 12.0,
             "price_hourly": 0.01786, "regions": ["lon1", "nyc3"], "available": true},
            {"slug": "s-2vcpu-4gb", "memory": 4096, "vcpus": 2, "disk": 80, "price_monthly": 24.0,
             "price_hourly": 0.03571, "regions": ["lon1", "nyc3"], "available": true},
            {"slug": "s-4vcpu-8gb", "memory": 8192, "vcpus": 4, "disk": 160, "price_monthly": 48.0,
             "price_hourly": 0.07143, "regions": ["nyc3"], "available": true}
        ],
        "links": {},
        "meta": {"total": 4}
    })
}

async fn live_cloud(http: Arc<FakeTransport>) -> (Arc<Cloud>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let creds = Creds {
        store: Arc::new(MemStore::default()),
        http: http.clone(),
        relay: "https://relay.test".into(),
    };
    creds
        .save(
            ProviderKind::Digitalocean,
            &StoredCred {
                access_token: "dop_v1_x".into(),
                refresh_token: None,
                expires_at_ms: None,
                source: CredentialSource::Pasted,
                org: None,
            },
        )
        .await
        .unwrap();
    let cloud = Arc::new(Cloud::new(
        Records::new(dir.path().join("cloudnodes")),
        Arc::new(FakeMesh::default()),
        creds,
        Arc::new(LiveConnect { http }),
        Arc::new(|_: CloudEvent| {}),
        fast(),
        None,
    ));
    (cloud, dir)
}

#[tokio::test]
async fn sizes_are_priced_live_and_cached_for_an_hour() {
    let http = FakeTransport::new();
    http.on("GET", "/v2/sizes", 200, do_sizes_body());
    let (cloud, _dir) = live_cloud(http.clone()).await;
    let list = cloud
        .sizes(ProviderKind::Digitalocean, Some("lon1".into()))
        .await;
    assert!(list.priced && list.error.is_none(), "{list:?}");
    let priced: Vec<(&str, Option<f64>)> = list
        .sizes
        .iter()
        .map(|s| (s.slug.as_str(), s.monthly_usd))
        .collect();
    assert_eq!(
        priced,
        [("s-1vcpu-2gb", Some(12.0)), ("s-2vcpu-4gb", Some(24.0))],
        "8 GB is not in lon1"
    );
    assert_eq!(http.requests().len(), 1);
    assert_eq!(
        http.requests()[0].bearer.as_deref().map(String::as_str),
        Some("dop_v1_x")
    );
    // Within the hour: no second request, for sizes and for a quote in the same region.
    let again = cloud
        .sizes(ProviderKind::Digitalocean, Some("lon1".into()))
        .await;
    assert_eq!(again, list);
    let q = cloud.quote(&spec("q")).await.unwrap();
    assert_eq!((q.server_usd, q.total_usd), (Some(12.0), Some(13.0)));
    assert_eq!(http.requests().len(), 1, "served from the cache");
    // Another region is its own entry.
    cloud
        .sizes(ProviderKind::Digitalocean, Some("nyc3".into()))
        .await;
    assert_eq!(http.requests().len(), 2);
    // Fly and Kubernetes: the unpriced skeleton, no request.
    let fly = cloud.sizes(ProviderKind::Fly, None).await;
    assert!(
        !fly.priced && fly.error.is_none() && fly.sizes.iter().all(|s| s.monthly_usd.is_none())
    );
    assert_eq!(http.requests().len(), 2);
}

#[tokio::test]
async fn a_failed_price_fetch_is_not_cached_and_falls_back_to_the_skeleton() {
    let http = FakeTransport::new();
    http.once(
        "GET",
        "/v2/sizes",
        500,
        json!({"id": "server_error", "message": "boom"}),
    )
    .on("GET", "/v2/sizes", 200, do_sizes_body());
    let (cloud, _dir) = live_cloud(http.clone()).await;
    let first = cloud.sizes(ProviderKind::Digitalocean, None).await;
    assert!(!first.priced && first.error.is_some());
    assert_eq!(first.sizes.len(), 3);
    assert!(first.sizes.iter().all(|s| s.monthly_usd.is_none()));
    // Not cached: the next call asks again and gets prices.
    let second = cloud.sizes(ProviderKind::Digitalocean, None).await;
    assert!(second.priced);
    assert_eq!(http.requests().len(), 2);
    // No credential: unpriced, and the quote has no total.
    cloud.forget(ProviderKind::Digitalocean).await.unwrap();
    let mut s = spec("q");
    s.region = "ams3".into();
    let q = cloud.quote(&s).await.unwrap();
    assert_eq!(q.total_usd, None);
    let none = cloud
        .sizes(ProviderKind::Digitalocean, Some("ams3".into()))
        .await;
    assert!(!none.priced && none.error.unwrap().contains("not connected"));
}

// ---- the vault stage (design doc 225) ----

const PW: &str = "correct horse battery staple";

fn pw() -> Option<Zeroizing<String>> {
    Some(Zeroizing::new(PW.into()))
}

fn progress_lines(r: &Rig, name: &str) -> Vec<String> {
    r.events
        .lock()
        .iter()
        .filter_map(|e| match e {
            CloudEvent::CloudNodeProgress { name: n, line, .. } if n == name => Some(line.clone()),
            _ => None,
        })
        .collect()
}

/// The `vault_*` calls over the mesh, in order.
fn vault_calls(r: &Rig) -> Vec<(String, Value)> {
    r.mesh
        .calls
        .lock()
        .iter()
        .filter(|(_, m, _)| m.starts_with("vault_"))
        .map(|(_, m, p)| (m.clone(), p.clone()))
        .collect()
}

/// Every serialized form the password could leak into: the events, the records on disk, and
/// the `cloud_list` / `cloud_status` answers.
async fn assert_no_password(r: &Rig, name: &str) {
    let events = serde_json::to_string(&*r.events.lock()).unwrap();
    assert!(!events.contains(PW), "an event carries the password");
    for e in std::fs::read_dir(r.cloud.records.dir()).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        assert!(!text.contains(PW), "a record carries the password");
    }
    let list = serde_json::to_string(&r.cloud.list().await).unwrap();
    assert!(!list.contains(PW), "cloud_list carries the password");
    let status = serde_json::to_string(&r.cloud.status(name).await.unwrap()).unwrap();
    assert!(!status.contains(PW), "cloud_status carries the password");
}

#[tokio::test]
async fn the_password_goes_to_the_node_once_after_enrolment_and_nowhere_else() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((1, peer("node-id", "lab", true)));
    *r.mesh.vault.lock() = FakeVault::sealed();
    let node = r.cloud.create(spec("lab"), pw()).await.unwrap();
    assert!(
        !serde_json::to_string(&node).unwrap().contains(PW),
        "the create answer"
    );
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(
        (rec.state, rec.vault),
        (NodeState::Ready, Some(VaultProtection::Protected))
    );
    let calls = vault_calls(&r);
    let sent: Vec<&Value> = calls
        .iter()
        .filter(|(m, _)| m == "vault_add_password")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(sent, [&json!([null, PW])], "once, to the node's own vault");
    assert!(
        r.mesh
            .calls
            .lock()
            .iter()
            .all(|(id, m, _)| !m.starts_with("vault_") || id == "node-id"),
        "every vault call goes to the node"
    );
    // The node had not enrolled this engine at the first look: asked once on its behalf, the
    // password only after the enrolment, and a read-back last.
    let at = |m: &str| calls.iter().position(|(x, _)| x == m).unwrap();
    assert_eq!(calls[at("vault_add_device")].1, json!([null, ME]));
    assert!(at("vault_add_device") < at("vault_add_password"));
    assert_eq!(calls.last().unwrap().0, "vault_status");
    assert_no_password(&r, "lab").await;
}

#[tokio::test]
async fn a_node_without_the_sealed_vault_skips_the_stage_and_is_still_created() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((1, peer("node-id", "lab", true)));
    // `FakeVault::default()`: the node's release answers method-not-found to `vault_status`.
    r.cloud.create(spec("lab"), pw()).await.unwrap();
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(
        (rec.state, rec.error.as_deref(), rec.vault),
        (NodeState::Ready, None, Some(VaultProtection::Unsupported))
    );
    let methods: Vec<String> = vault_calls(&r).into_iter().map(|(m, _)| m).collect();
    assert_eq!(methods, ["vault_status"], "nothing more is tried");
    assert!(
        progress_lines(&r, "lab")
            .iter()
            .any(|l| l == "Vault protection arrives with the next theuth release on this node")
    );
    assert_no_password(&r, "lab").await;
}

#[tokio::test]
async fn a_refused_password_leaves_the_node_ready_and_asking_for_one() {
    let r = rig(fast()).await;
    *r.mesh.join_after.lock() = Some((1, peer("node-id", "lab", true)));
    *r.mesh.vault.lock() = FakeVault {
        refuse_password: true,
        ..FakeVault::sealed()
    };
    r.cloud.create(spec("lab"), pw()).await.unwrap();
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(
        (rec.state, rec.error.as_deref(), rec.vault),
        (NodeState::Ready, None, Some(VaultProtection::NeedsPassword))
    );
    assert!(
        progress_lines(&r, "lab")
            .iter()
            .any(|l| l == "Vault: device only — set a master password")
    );
    assert_no_password(&r, "lab").await;

    // The user sets one from the row (`vault_add_password` on the node): the next list sees it.
    r.mesh
        .vault
        .lock()
        .methods
        .as_mut()
        .unwrap()
        .push((Kind::Password, "password".into()));
    let list = r.cloud.list().await;
    assert_eq!(list.nodes[0].vault, Some(VaultProtection::Protected));
    assert_eq!(
        r.cloud.records.get("lab").unwrap().unwrap().vault,
        Some(VaultProtection::Protected)
    );
}

#[tokio::test]
async fn an_empty_field_is_device_only_and_a_short_password_is_refused_up_front() {
    let r = rig(fast()).await;
    let short = r
        .cloud
        .create(spec("lab"), Some(Zeroizing::new("short".into())))
        .await
        .unwrap_err();
    assert!(short.0.contains("12 characters"), "{}", short.0);
    assert!(
        r.cloud.records.get("lab").unwrap().is_none(),
        "nothing was made"
    );

    *r.mesh.join_after.lock() = Some((1, peer("node-id", "lab", true)));
    *r.mesh.vault.lock() = FakeVault::sealed();
    let node = r
        .cloud
        .create(spec("lab"), Some(Zeroizing::new(String::new())))
        .await
        .unwrap();
    assert_eq!(node.vault, None);
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(rec.vault, Some(VaultProtection::DeviceOnly));
    assert!(
        vault_calls(&r)
            .iter()
            .all(|(m, _)| m != "vault_add_password")
    );
}

#[tokio::test]
async fn a_restart_loses_the_password_and_the_node_asks_for_one() {
    let r = rig(fast()).await;
    *r.mesh.vault.lock() = FakeVault::sealed();
    // A create interrupted after the server, as the restarted engine finds it: `Pending`, no job.
    let rec = Record {
        spec: spec("lab"),
        created: Some(Created {
            server_id: Some("42".into()),
            volume_id: None,
            extra: None,
            ip: None,
            size_slug: "s-1vcpu-2gb".into(),
        }),
        state: NodeState::Creating,
        stage: Some(Stage::Install),
        error: None,
        peer_id: None,
        peers_before: vec!["laptop".into()],
        ticket_expires_ms: Some(now_ms() + TICKET_TTL_MS),
        created_ms: now_ms(),
        vault: Some(VaultProtection::Pending),
        link: None,
    };
    r.cloud.records.put(&rec).unwrap();
    // One that was past the job when the engine stopped.
    r.cloud
        .records
        .put(&Record {
            spec: spec("old"),
            state: NodeState::Ready,
            stage: Some(Stage::Ready),
            peer_id: Some("old-id".into()),
            ..rec.clone()
        })
        .unwrap();
    r.mesh.peers.lock().push(peer("node-id", "lab", true));
    r.cloud.resume();
    let rec = settle(&r.cloud, "lab").await;
    assert_eq!(
        (rec.state, rec.vault),
        (NodeState::Ready, Some(VaultProtection::NeedsPassword))
    );
    assert!(
        vault_calls(&r)
            .iter()
            .all(|(m, _)| m != "vault_add_password")
    );
    assert_eq!(
        r.cloud.records.get("old").unwrap().unwrap().vault,
        Some(VaultProtection::NeedsPassword)
    );
}

#[test]
fn classify_names_what_the_vault_has() {
    let status = |m: &[(Kind, &str)]| -> VaultStatus {
        let m: Vec<(Kind, String)> = m.iter().map(|(k, id)| (*k, (*id).to_owned())).collect();
        serde_json::from_value(vault_json(&m)).unwrap()
    };
    let both = status(&[(Kind::Device, ME), (Kind::Password, "password")]);
    assert_eq!(
        classify(Some(&both), Some(ME), true),
        VaultProtection::Protected
    );
    let pw_only = status(&[(Kind::Password, "password")]);
    assert_eq!(
        classify(Some(&pw_only), Some(ME), true),
        VaultProtection::PasswordOnly
    );
    let device = status(&[(Kind::Device, ME)]);
    assert_eq!(
        classify(Some(&device), Some(ME), false),
        VaultProtection::DeviceOnly
    );
    assert_eq!(
        classify(Some(&device), Some(ME), true),
        VaultProtection::NeedsPassword
    );
    // Another engine's device method is not this one's; an unprotected key file asks for a password.
    let other = status(&[(Kind::Device, "phone"), (Kind::File, "file")]);
    assert_eq!(
        classify(Some(&other), Some(ME), false),
        VaultProtection::NeedsPassword
    );
    assert_eq!(
        classify(None, Some(ME), false),
        VaultProtection::NeedsPassword
    );
}
