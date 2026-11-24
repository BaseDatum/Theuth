//! M2 over real iroh connections to an in-process edge (fake zone, CA and CAA; no network): reserved names,
//! online transfer, several engines under one name, Stop all with `published_ms`, revoke, rekey with its chain
//! and conflict rules, and the P0-6 races (§5.7, §10 M2 exit) whose edge side can be driven here.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, RelayMode, SecretKey};
use serde::Serialize;
use share_edge::Running;
use share_edge::config::Config;
use share_proto::version::client_agent;
use share_proto::{
    CertChain, CsrQuery, Delegation, ErrorBody, ErrorCode, Hello, MeshStatus, NameAvailability,
    NameReleased, NameRequest, NamesStatus, NamesStatusRequest, PublicKey, Rekey, RekeyAccepted,
    RekeyConflict, RevokeRequest, RouteEntry, RoutesPut, StopAll, TransferAccepted, TransferRequest,
    Welcome, names, paths, rekey_conflict,
};

const D: &str = "tun.zip";

struct Rig {
    run: Running,
    _dir: tempfile::TempDir,
}

/// One engine: its key, its endpoint (kept alive) and its connection to the edge.
struct Eng {
    key: SecretKey,
    _ep: Endpoint,
    conn: Connection,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn err(body: &[u8]) -> ErrorBody {
    serde_json::from_slice(body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(body)))
}

fn host(label: Option<&str>, name: &str) -> String {
    names::host(name, label, D)
}

fn routes(hosts: &[String]) -> RoutesPut {
    RoutesPut {
        hosts: hosts.to_vec(),
        ..Default::default()
    }
}

fn keys<const N: usize>() -> [SecretKey; N] {
    std::array::from_fn(|_| SecretKey::generate())
}

fn csr(name: &str) -> Vec<u8> {
    let key = rcgen::KeyPair::generate().unwrap();
    let sans = vec![format!("{name}.{D}"), format!("*.{name}.{D}")];
    let params = rcgen::CertificateParams::new(sans).unwrap();
    params.serialize_request(&key).unwrap().der().to_vec()
}

impl Rig {
    async fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!(
            "domain: {D}\ndata_dir: {d}\nlisten: \"127.0.0.1:0\"\niroh: {{listen: \"127.0.0.1:0\"}}\n\
             admin: {{socket: {d}/admin.sock}}\nacme: {{propagation: 0, auth_gap: 0}}\n",
            d = dir.path().display()
        );
        let (cfg, secrets) = Config::from_yaml(&yaml, &|_| None).unwrap();
        let run = share_edge::start_with(cfg, secrets, common::fakes().backends)
            .await
            .unwrap();
        Rig { run, _dir: dir }
    }

    fn edge_id(&self) -> PublicKey {
        self.run.edge.id
    }

    /// A connection for `key`, no `hello` yet.
    async fn dial(&self, key: &SecretKey) -> Eng {
        let ep = Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let conn = ep
            .connect(self.run.endpoint_addr(), share_proto::ALPN)
            .await
            .unwrap();
        Eng {
            key: key.clone(),
            _ep: ep,
            conn,
        }
    }

    /// Dial and `hello` with a delegation of `mesh`; must succeed.
    async fn attach(&self, key: &SecretKey, mesh: &SecretKey) -> (Eng, Welcome) {
        let e = self.dial(key).await;
        let (status, body) = e.hello(self, mesh).await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        (e, serde_json::from_slice(&body).unwrap())
    }

    fn signed_rekey(&self, old: &SecretKey, new: &SecretKey, revoke: Vec<PublicKey>) -> Vec<u8> {
        let r = Rekey::new(D, self.edge_id(), old.public(), new.public(), revoke, now_ms());
        serde_json::to_vec(&r.sign(old, new).unwrap()).unwrap()
    }

    fn route_of(&self, host: &str) -> Option<PublicKey> {
        self.run.edge.routes.get(host).map(|r| r.tunnel.engine)
    }
}

impl Eng {
    async fn call(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> (u16, Vec<u8>) {
        let (send, recv) = self.conn.open_bi().await.expect("open_bi");
        let io = TokioIo::new(tokio::io::join(recv, send));
        let (mut sender, c) = hyper::client::conn::http1::handshake(io)
            .await
            .expect("handshake");
        tokio::spawn(c);
        let req = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "edge")
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .expect("request");
        let resp = sender.send_request(req).await.expect("send");
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, bytes.to_vec())
    }

    async fn send<T: Serialize>(&self, method: &str, path: &str, body: &T) -> (u16, Vec<u8>) {
        self.call(method, path, Some(serde_json::to_vec(body).unwrap()))
            .await
    }

    async fn hello(&self, rig: &Rig, mesh: &SecretKey) -> (u16, Vec<u8>) {
        let d = Delegation::new(D, rig.edge_id(), mesh.public(), self.key.public(), now_ms(), now_ms());
        let hello = Hello {
            v: 1,
            delegation: d.sign(mesh).unwrap(),
            invite: None,
            client: client_agent("2.25.5"),
            terms_rev: None,
        };
        self.send("POST", paths::HELLO, &hello).await
    }

    async fn mesh(&self) -> MeshStatus {
        let (status, body) = self.call("GET", paths::MESH, None).await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    /// The edge closed this connection within 5 s.
    async fn closed(&self) -> bool {
        tokio::time::timeout(Duration::from_secs(5), self.conn.closed())
            .await
            .is_ok()
    }

    /// `POST /v1/names/status` with a delegation of `mesh` for `engine` (normally this connection's key).
    async fn names_status(&self, rig: &Rig, mesh: &SecretKey, engine: PublicKey) -> (u16, Vec<u8>) {
        let d = Delegation::new(D, rig.edge_id(), mesh.public(), engine, now_ms(), now_ms());
        let req = NamesStatusRequest {
            v: 1,
            client: client_agent("2.25.5"),
            delegation: d.sign(mesh).unwrap(),
        };
        self.send("POST", paths::NAMES_STATUS, &req).await
    }
}

/// CERT-STATE: `POST /v1/names/status` answers the mesh's names on a connection without `hello` and attaches
/// nothing (no engine row, tunnel, session); a delegation for another key, a revoked engine and a banned mesh are
/// refused as `hello` refuses them.
#[tokio::test(flavor = "multi_thread")]
async fn names_status_reads_the_names_without_attaching() {
    use share_proto::admin::{AdminCommand, AdminRequest, BanKind};
    let rig = Rig::new().await;
    let [m, ka, kb, gone] = keys();
    let (a, _) = rig.attach(&ka, &m).await;
    assert_eq!(a.call("PUT", &paths::name("sky"), None).await.0, 200);
    let engines = a.mesh().await.engines.len();

    let b = rig.dial(&kb).await;
    let (status, body) = b.names_status(&rig, &m, kb.public()).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let s: NamesStatus = serde_json::from_slice(&body).unwrap();
    assert_eq!(s.mesh, m.public());
    assert_eq!(s.names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["sky"]);
    assert_eq!(a.mesh().await.engines.len(), engines, "no engine recorded");
    assert_eq!(rig.run.edge.mesh_tunnels(&m.public()).len(), 1, "no tunnel attached");
    assert!(rig.run.edge.tunnels.get(&kb.public()).is_none());
    let (_, body) = b.call("GET", paths::MESH, None).await;
    assert_eq!(err(&body).code, ErrorCode::HelloRequired, "the connection has no session");

    let (status, body) = b.names_status(&rig, &m, gone.public()).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::BadDelegation), "a delegation for another key");

    let req = RevokeRequest { engines: vec![gone.public()] };
    assert_eq!(a.send("POST", paths::MESH_REVOKE, &req).await.0, 200);
    let (_, body) = rig.dial(&gone).await.names_status(&rig, &m, gone.public()).await;
    assert_eq!(err(&body).code, ErrorCode::Revoked);

    let ban = AdminRequest {
        id: 1,
        command: AdminCommand::Ban { kind: BanKind::Mesh, value: m.public().to_string(), reason: "test".into() },
        reference: None,
    };
    assert!(share_edge::admin::handle(&rig.run.edge, ban).await.ok);
    let (_, body) = rig.dial(&kb).await.names_status(&rig, &m, kb.public()).await;
    assert_eq!(err(&body).code, ErrorCode::Banned);
}

/// §5.2: one reserved name per mesh at once, a random one only while none is held, first claim wins, release
/// with cooldown for others but not for the previous owner, and the online transfer (D20) without cooldown.
#[tokio::test(flavor = "multi_thread")]
async fn reserved_names_cooldown_and_online_transfer() {
    let rig = Rig::new().await;
    let [ma, mb, ka, kb] = keys();
    let (a, _) = rig.attach(&ka, &ma).await;
    let (b, _) = rig.attach(&kb, &mb).await;

    assert_eq!(a.call("PUT", &paths::name("sky"), None).await.0, 200);
    let (status, body) = a.call("PUT", &paths::name("fog"), None).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Quota));
    let random = NameRequest {
        name: names::random_name([1, 2, 3, 4, 5, 6]),
    };
    let (status, body) = a.send("POST", paths::NAMES, &random).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Quota), "random while a reserved one is held");

    let (status, body) = b.call("PUT", &paths::name("sky"), None).await;
    assert_eq!((status, err(&body).code), (409, ErrorCode::NameTaken));
    let (_, body) = b.call("GET", &paths::name("sky"), None).await;
    let av: NameAvailability = serde_json::from_slice(&body).unwrap();
    assert_eq!((av.available, av.reason.as_deref()), (false, Some("taken")));

    // `a` serves the name, then offers it to `b`; only the holder may offer, never to itself.
    let h = host(None, "sky");
    assert_eq!(a.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&h))).await.0, 204);
    let offer = TransferRequest { to_mesh: mb.public() };
    let to_a = TransferRequest { to_mesh: ma.public() };
    let (status, body) = b.send("POST", &paths::name_transfer("sky"), &to_a).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::NotOwner));
    let self_offer = TransferRequest { to_mesh: ma.public() };
    assert_eq!(a.send("POST", &paths::name_transfer("sky"), &self_offer).await.0, 400);
    let (status, body) = a.send("POST", &paths::name_transfer("sky"), &offer).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let until = serde_json::from_slice::<TransferAccepted>(&body).unwrap().transfer_until_ms;
    let week = share_proto::limits::TRANSFER_TTL_MS;
    assert!(until > now_ms() + week - 60_000 && until <= now_ms() + week);
    let (_, body) = b.call("GET", &paths::name("sky"), None).await;
    assert!(serde_json::from_slice::<NameAvailability>(&body).unwrap().available);
    // The pending offer shows in the giver's welcome.
    let (_, body) = a.hello(&rig, &ma).await;
    let w: Welcome = serde_json::from_slice(&body).unwrap();
    assert_eq!(w.names[0].transfer_to, Some(mb.public()));
    assert_eq!(w.names[0].transfer_until_ms, Some(until));

    // The receiver claims with the plain PUT: no cooldown, `a`'s route goes, `a` no longer holds it.
    assert_eq!(b.call("PUT", &paths::name("sky"), None).await.0, 200);
    assert_eq!(rig.route_of(&h), None);
    let (status, body) = a.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&h))).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::NotOwner));
    assert_eq!(b.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&h))).await.0, 204);
    assert_eq!(rig.route_of(&h), Some(kb.public()));
    // `a` holds nothing now: its quota is free again.
    assert_eq!(a.call("PUT", &paths::name("fog"), None).await.0, 200);

    // Release: cooldown for everyone else, the previous owner may come back.
    let (status, body) = b.call("DELETE", &paths::name("sky"), None).await;
    assert_eq!(status, 200);
    let rel: NameReleased = serde_json::from_slice(&body).unwrap();
    assert_eq!(rig.route_of(&h), None);
    assert_eq!(a.call("DELETE", &paths::name("fog"), None).await.0, 200);
    let (status, body) = a.call("PUT", &paths::name("sky"), None).await;
    let e = err(&body);
    assert_eq!((status, e.code, e.reclaim_after_ms), (409, ErrorCode::Cooldown, Some(rel.reclaim_after_ms)));
    let (_, body) = a.call("GET", &paths::name("sky"), None).await;
    let av: NameAvailability = serde_json::from_slice(&body).unwrap();
    assert_eq!((av.reason.as_deref(), av.reclaim_after_ms), (Some("cooldown"), Some(rel.reclaim_after_ms)));
    assert_eq!(b.call("PUT", &paths::name("sky"), None).await.0, 200);
}

/// The three-engine family test, edge side (§10 M2 exit): publish from two engines under one name (each
/// sub-label to the engine that published it, `route_conflict` naming the holder), a freed host taken by
/// another engine, Stop all from the third, `published_ms` against `stop_all_ms`, then a removal whose vault
/// rotation fails (revoke only: the removed engine's tunnel closes at once and stays out, the names stay), and
/// the later rekey from a hub that publishes nothing (its sharing switch off).
#[tokio::test(flavor = "multi_thread")]
async fn three_engine_family() {
    let rig = Rig::new().await;
    let [m, m2, k1, k2, k3] = keys();
    let (e1, _) = rig.attach(&k1, &m).await;
    let (e2, _) = rig.attach(&k2, &m).await;
    let (e3, _) = rig.attach(&k3, &m).await;
    assert_eq!(e1.call("PUT", &paths::name("sky"), None).await.0, 200);

    let (bare, app, docs) = (host(None, "sky"), host(Some("app"), "sky"), host(Some("docs"), "sky"));
    assert_eq!(e1.send("PUT", paths::ROUTES, &routes(&[bare.clone(), app.clone()])).await.0, 204);
    assert_eq!(e2.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&docs))).await.0, 204);
    let (status, body) = e2.send("PUT", paths::ROUTES, &routes(&[docs.clone(), app.clone()])).await;
    let e = err(&body);
    assert_eq!((status, e.code), (409, ErrorCode::RouteConflict));
    assert_eq!(e.conflicts, vec![RouteEntry { host: app.clone(), engine: k1.public() }]);
    assert_eq!(rig.route_of(&docs), Some(k2.public()), "nothing applied on a conflict");

    let ms = e3.mesh().await;
    assert_eq!(ms.mesh, m.public());
    let mut want = vec![
        RouteEntry { host: app.clone(), engine: k1.public() },
        RouteEntry { host: docs.clone(), engine: k2.public() },
        RouteEntry { host: bare.clone(), engine: k1.public() },
    ];
    want.sort_by(|a, b| a.host.cmp(&b.host));
    assert_eq!(ms.routes, want);
    assert_eq!(ms.engines.iter().filter(|e| e.attached).count(), 3);

    // e2 stops its share; the freed host goes to e1.
    assert_eq!(e2.send("PUT", paths::ROUTES, &routes(&[])).await.0, 204);
    assert_eq!(rig.route_of(&docs), None);
    let all = [bare.clone(), app.clone(), docs.clone()];
    assert_eq!(e1.send("PUT", paths::ROUTES, &routes(&all)).await.0, 204);

    // Stop all from the third: every tunnel of the mesh closes, every route goes.
    let (status, body) = e3.call("POST", paths::MESH_STOP, None).await;
    assert_eq!(status, 200);
    let stop = serde_json::from_slice::<StopAll>(&body).unwrap().stop_all_ms;
    for e in [&e1, &e2, &e3] {
        assert!(e.closed().await);
    }
    assert!(all.iter().all(|h| rig.route_of(h).is_none()));
    drop((e1, e2, e3));

    // A share published at or before the Stop all is refused by name, nothing applied; a newer one goes live.
    let (e1, w) = rig.attach(&k1, &m).await;
    assert_eq!(w.stop_all_ms, Some(stop));
    let mut put = routes(&[bare.clone(), app.clone()]);
    put.published_ms = [(bare.clone(), stop), (app.clone(), stop + 1)].into();
    let (status, body) = e1.send("PUT", paths::ROUTES, &put).await;
    let e = err(&body);
    assert_eq!((status, e.code), (409, ErrorCode::StoppedAll));
    assert_eq!((e.stop_all_ms, e.hosts), (Some(stop), vec![bare.clone()]));
    assert_eq!(rig.route_of(&app), None);
    put.published_ms.insert(bare.clone(), stop + 2);
    assert_eq!(e1.send("PUT", paths::ROUTES, &put).await.0, 204);

    // Removal of e2 whose vault rotation fails: the hub revokes now; no rekey follows yet.
    let (e2, _) = rig.attach(&k2, &m).await;
    assert_eq!(e2.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&docs))).await.0, 204);
    let (hub, _) = rig.attach(&k3, &m).await;
    let req = RevokeRequest { engines: vec![k2.public()] };
    assert_eq!(hub.send("POST", paths::MESH_REVOKE, &req).await.0, 200);
    assert!(e2.closed().await, "the removed engine's tunnel closes at once");
    assert_eq!(rig.route_of(&docs), None);
    assert_eq!(rig.route_of(&bare), Some(k1.public()), "other engines keep serving");
    drop(e2);
    let e2 = rig.dial(&k2).await;
    let (status, body) = e2.hello(&rig, &m).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Revoked));
    let ms = hub.mesh().await;
    assert_eq!(ms.revoked, vec![k2.public()]);
    assert!(ms.rekeys.is_empty());

    // The vault finally rotates: the hub (no shares of its own) rekeys m → m2, revoking e2.
    let (status, body) = hub
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&m, &m2, vec![k2.public()])))
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(serde_json::from_slice::<RekeyAccepted>(&body).unwrap().mesh, m2.public());
    assert!(e1.closed().await && hub.closed().await, "kill {{mesh_rekeyed}} closes every tunnel of m");
    assert_eq!(rig.route_of(&bare), None);
    drop((e1, hub));

    let e1 = rig.dial(&k1).await;
    let (status, body) = e1.hello(&rig, &m).await;
    let e = err(&body);
    assert_eq!((status, e.code, e.to), (409, ErrorCode::MeshRekeyed, Some(m2.public())));
    let (status, body) = e1.hello(&rig, &m2).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let w: Welcome = serde_json::from_slice(&body).unwrap();
    assert_eq!(w.names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["sky"]);
    assert_eq!(w.revoked, vec![k2.public()]);
    assert_eq!(w.stop_all_ms, Some(stop), "Stop all moves with the mesh");
    let e2 = rig.dial(&k2).await;
    let (status, body) = e2.hello(&rig, &m2).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Revoked));
    let ms = e1.mesh().await;
    assert_eq!(ms.mesh, m2.public());
    assert_eq!(ms.revoked, vec![k2.public()]);
    assert_eq!(ms.rekeys.len(), 1);
    assert_eq!((ms.rekeys[0].old, ms.rekeys[0].new), (m.public(), m2.public()));
    let mut put = routes(std::slice::from_ref(&bare));
    put.published_ms.insert(bare.clone(), now_ms());
    assert_eq!(e1.send("PUT", paths::ROUTES, &put).await.0, 204);
    assert_eq!(rig.route_of(&bare), Some(k1.public()));
}

/// P0-6: two machines of one mesh publish a first name in one sync gap. Two random allocations: one wins, the
/// other is `quota`. Two reserved claims of different names: one wins; the loser then claims the winner's
/// (idempotent). Two different CSRs for it: one is stored, the other is `csr_mismatch` naming the held SPKI;
/// the heal (`rotate`, §6.2 step 6) then gets the settled key's certificate.
#[tokio::test(flavor = "multi_thread")]
async fn two_machines_publish_a_first_name_in_one_sync_gap() {
    let rig = Rig::new().await;
    let [m, k1, k2] = keys();
    let (e1, _) = rig.attach(&k1, &m).await;
    let (e2, _) = rig.attach(&k2, &m).await;

    let r1 = NameRequest { name: names::random_name([1, 1, 1, 1, 1, 1]) };
    let r2 = NameRequest { name: names::random_name([2, 2, 2, 2, 2, 2]) };
    let (a, b) = tokio::join!(e1.send("POST", paths::NAMES, &r1), e2.send("POST", paths::NAMES, &r2));
    let mut got = [a.0, b.0];
    got.sort_unstable();
    assert_eq!(got, [201, 403], "{} {}", String::from_utf8_lossy(&a.1), String::from_utf8_lossy(&b.1));
    let loser = if a.0 == 403 { &a.1 } else { &b.1 };
    assert_eq!(err(loser).code, ErrorCode::Quota);

    let (sky, fog) = (paths::name("sky"), paths::name("fog"));
    let (a, b) = tokio::join!(e1.call("PUT", &sky, None), e2.call("PUT", &fog, None));
    let mut got = [a.0, b.0];
    got.sort_unstable();
    assert_eq!(got, [200, 403]);
    let name = if a.0 == 200 { "sky" } else { "fog" };
    assert_eq!(e1.call("PUT", &paths::name(name), None).await.0, 200);
    assert_eq!(e2.call("PUT", &paths::name(name), None).await.0, 200);

    let (c1, c2) = (csr(name), csr(name));
    let path = paths::name_csr(name, false);
    let (a, b) = tokio::join!(e1.call("POST", &path, Some(c1.clone())), e2.call("POST", &path, Some(c2.clone())));
    let (won, lost, by, body) = match (a.0, b.0) {
        (200 | 202, 409) => (&c1, &c2, &e2, &b.1),
        (409, 200 | 202) => (&c2, &c1, &e1, &a.1),
        other => panic!("{other:?} {} {}", String::from_utf8_lossy(&a.1), String::from_utf8_lossy(&b.1)),
    };
    let e = err(body);
    assert_eq!(e.code, ErrorCode::CsrMismatch);
    let spki = |der: &[u8]| share_proto::check_csr(der, name, D).unwrap().spki;
    assert_eq!(e.spki, Some(spki(won)));

    // Heal: the lease holder rotates to the settled key (here the loser's). While the first order is
    // `ordering` a rotation is `csr_mismatch` too; retry until accepted.
    let rotate = paths::name_csr_query(name, &CsrQuery { rotate: true, compromised: false });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (status, body) = by.call("POST", &rotate, Some(lost.clone())).await;
        if status == 202 || status == 200 {
            break;
        }
        assert_eq!(err(&body).code, ErrorCode::CsrMismatch);
        assert!(tokio::time::Instant::now() < deadline, "rotation never accepted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    loop {
        let (status, body) = e1.call("GET", &paths::name_cert(name), None).await;
        if status == 200 {
            let chain: CertChain = serde_json::from_slice(&body).unwrap();
            if share_edge::acme::leaf_info(&chain.chain_pem).unwrap().spki == spki(lost) {
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "healed certificate never issued");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// P0-6 rekey races: a thief's revoke under the old key does not survive the rekey (the revoke list is
/// replaced); two holders after a partition heal: the first valid rekey wins, the second gets `409 {to}`, the
/// holder whose stash has `to` chains it to the settled key (sent without a `hello`), a stale retry is
/// `Confirming`; a forged signature, an unknown mesh and a `new` already in use are refused.
#[tokio::test(flavor = "multi_thread")]
async fn rekey_races_first_valid_wins_and_the_chain_heals() {
    let rig = Rig::new().await;
    let [k, a, b, owner, thief, h1, h2] = keys();
    let (o, _) = rig.attach(&owner, &k).await;
    let (t, _) = rig.attach(&thief, &k).await;
    assert_eq!(o.call("PUT", &paths::name("sky"), None).await.0, 200);

    // The thief, still delegated under k, revokes the owner's engine.
    let req = RevokeRequest { engines: vec![owner.public()] };
    assert_eq!(t.send("POST", paths::MESH_REVOKE, &req).await.0, 200);
    assert!(o.closed().await);

    // Partition: holder 1 settled `a` and rekeys k → a (revoking the thief).
    let (hub1, _) = rig.attach(&h1, &k).await;
    let (status, body) = hub1
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&k, &a, vec![thief.public()])))
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(t.closed().await);

    // Heal: the vault settled on `b`. Holder 2 (stash: k) sends k → b without a hello.
    let hub2 = rig.dial(&h2).await;
    let (status, body) = hub2
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&k, &b, vec![thief.public()])))
        .await;
    let e = err(&body);
    assert_eq!((status, e.code, e.to), (409, ErrorCode::MeshRekeyed, Some(a.public())));
    assert_eq!(rekey_conflict(&a.public(), &b.public(), &[k.public()]), RekeyConflict::Contested);
    // Holder 1's stash holds `a`: the heal rule chains a → b.
    let from = match rekey_conflict(&a.public(), &b.public(), &[k.public(), a.public()]) {
        RekeyConflict::Chain { from } => from,
        other => panic!("{other:?}"),
    };
    let chained = Rekey::new(D, rig.edge_id(), k.public(), b.public(), vec![thief.public()], now_ms()).chain(from);
    let hub1 = rig.dial(&h1).await;
    let body = serde_json::to_vec(&chained.sign(&a, &b).unwrap()).unwrap();
    let (status, body) = hub1.call("POST", paths::MESH_REKEY, Some(body)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(serde_json::from_slice::<RekeyAccepted>(&body).unwrap().mesh, b.public());
    // Holder 2's retry: `to` is now its settled key.
    let (status, body) = hub2
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&k, &b, vec![thief.public()])))
        .await;
    assert_eq!((status, err(&body).to), (409, Some(b.public())));
    assert_eq!(rekey_conflict(&b.public(), &b.public(), &[k.public()]), RekeyConflict::Confirming);

    // The owner is back (the thief's revoke did not survive), the thief is out, the names moved.
    let (o, w) = rig.attach(&owner, &b).await;
    assert_eq!(w.names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["sky"]);
    assert_eq!(w.revoked, vec![thief.public()]);
    let t = rig.dial(&thief).await;
    let (status, body) = t.hello(&rig, &b).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Revoked));
    let ms = o.mesh().await;
    assert_eq!(ms.mesh, b.public());
    let hist: Vec<(PublicKey, PublicKey)> = ms.rekeys.iter().map(|r| (r.old, r.new)).collect();
    assert_eq!(hist, vec![(k.public(), a.public()), (a.public(), b.public())]);
    let (status, body) = t.hello(&rig, &k).await;
    assert_eq!((status, err(&body).to), (409, Some(b.public())), "the end of the chain");

    // Refusals: a forged new-key signature; an unknown old mesh; a `new` that already holds names.
    let mut forged = Rekey::new(D, rig.edge_id(), b.public(), a.public(), vec![], now_ms())
        .sign(&b, &a)
        .unwrap();
    forged.sig_new[0] ^= 1;
    let (status, body) = o.send("POST", paths::MESH_REKEY, &forged).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::BadSignature));
    let [stranger, fresh] = keys();
    let (status, _) = o
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&stranger, &fresh, vec![])))
        .await;
    assert_eq!(status, 404);
    let [other, other_engine] = keys();
    let (x, _) = rig.attach(&other_engine, &other).await;
    let (status, body) = x
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&other, &b, vec![])))
        .await;
    assert_eq!((status, err(&body).code), (400, ErrorCode::Invalid));
}

/// FIX4-EDGE #2 and #4: a route another mesh left on a name this mesh holds is evicted, not a conflict; a banned
/// name loses its routes, cannot be routed or ordered again, and is not in `welcome.names`; a ban needs a label.
#[tokio::test(flavor = "multi_thread")]
async fn a_banned_name_is_not_held_and_a_stale_route_is_evicted() {
    use share_proto::admin::{AdminCommand, AdminRequest, BanKind};
    let rig = Rig::new().await;
    let [ma, mb, ka, kb] = keys();
    let (a, _) = rig.attach(&ka, &ma).await;
    let (_b, _) = rig.attach(&kb, &mb).await;
    assert_eq!(a.call("PUT", &paths::name("sky"), None).await.0, 200);
    let h = host(None, "sky");
    let tb = rig.run.edge.tunnels.get(&kb.public()).unwrap().clone();
    rig.run.edge.routes.insert(
        h.clone(),
        share_edge::state::Route { name: "sky".into(), mesh: mb.public(), tunnel: tb, stats: Default::default() },
    );
    assert_eq!(a.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&h))).await.0, 204);
    assert_eq!(rig.route_of(&h), Some(ka.public()), "stale route of another mesh evicted");

    let ban = |value: &str| AdminRequest {
        id: 1,
        command: AdminCommand::Ban { kind: BanKind::Name, value: value.into(), reason: "test".into() },
        reference: None,
    };
    assert!(!share_edge::admin::handle(&rig.run.edge, ban("Not A Label")).await.ok);
    assert!(share_edge::admin::handle(&rig.run.edge, ban("sky")).await.ok);
    assert_eq!(rig.route_of(&h), None, "ban drops the live route");
    let (status, body) = a.send("PUT", paths::ROUTES, &routes(std::slice::from_ref(&h))).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::NotOwner));
    let (status, body) = a.call("POST", &paths::name_csr("sky", false), Some(csr("sky"))).await;
    assert_eq!(err(&body).code, ErrorCode::Banned, "{status}");
    let (_a2, w) = rig.attach(&ka, &ma).await;
    assert!(w.names.is_empty(), "a banned name is not in welcome.names");
}

/// FIX5-EDGE R1: a machine revoked before it ever attached is in `RevokeResult.revoked` and its `hello` with the
/// delegation it already holds is refused `revoked`.
#[tokio::test(flavor = "multi_thread")]
async fn an_engine_revoked_before_its_first_hello_cannot_attach() {
    let rig = Rig::new().await;
    let [m, ka, lost] = keys();
    let (a, _) = rig.attach(&ka, &m).await;
    let req = RevokeRequest { engines: vec![lost.public()] };
    let (status, body) = a.send("POST", paths::MESH_REVOKE, &req).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let r: share_proto::RevokeResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(r.revoked, vec![lost.public()]);
    let (_, body) = rig.dial(&lost).await.hello(&rig, &m).await;
    assert_eq!(err(&body).code, ErrorCode::Revoked);
}

/// FIX6-EDGE N1: a revocation outlives the delegation window while the mesh has not been rekeyed (the removed
/// machine still holds the share key and can sign itself a fresh delegation): housekeeping 30 h later keeps the
/// row and `hello` is still refused. After a rekey the row is pruned.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_is_pruned_only_after_a_rekey() {
    let rig = Rig::new().await;
    let [m, m2, ka, gone] = keys();
    let edge = &rig.run.edge;
    let (a, _) = rig.attach(&ka, &m).await;
    let req = RevokeRequest { engines: vec![gone.public()] };
    assert_eq!(a.send("POST", paths::MESH_REVOKE, &req).await.0, 200);
    let later = now_ms() + 30 * 3_600_000;
    share_edge::queue::housekeeping(edge, later).await.unwrap();
    assert!(edge.store.revoked(&m.public().to_string(), &gone.public().to_string()).await.unwrap().is_some());
    let (status, body) = rig.dial(&gone).await.hello(&rig, &m).await;
    assert_eq!((status, err(&body).code), (403, ErrorCode::Revoked));

    let (status, body) = a
        .call("POST", paths::MESH_REKEY, Some(rig.signed_rekey(&m, &m2, vec![gone.public()])))
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    share_edge::queue::housekeeping(edge, later).await.unwrap();
    for mesh in [&m, &m2] {
        let row = edge.store.revoked(&mesh.public().to_string(), &gone.public().to_string()).await.unwrap();
        assert!(row.is_none(), "pruned after the rekey");
    }
    let (status, body) = rig.dial(&gone).await.hello(&rig, &m).await;
    assert_eq!((status, err(&body).code), (409, ErrorCode::MeshRekeyed), "the old key is dead");
}

/// FIX5-EDGE R3: the eviction re-reads the owner. A route of another mesh on a name this mesh no longer holds
/// (released since its `held` read) stays; on a name it holds, it is evicted.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_route_is_evicted_only_while_the_name_is_held() {
    let rig = Rig::new().await;
    let [ma, mb, ka, kb] = keys();
    let (a, _) = rig.attach(&ka, &ma).await;
    let (_b, _) = rig.attach(&kb, &mb).await;
    assert_eq!(a.call("PUT", &paths::name("sky"), None).await.0, 200);
    let h = host(None, "sky");
    let edge = &rig.run.edge;
    let tb = edge.tunnels.get(&kb.public()).unwrap().clone();
    let route = || share_edge::state::Route {
        name: "sky".into(),
        mesh: mb.public(),
        tunnel: tb.clone(),
        stats: Default::default(),
    };
    let wanted = vec![(h.clone(), "sky".to_string())];
    let (ma_s, mb_s) = (ma.public().to_string(), mb.public().to_string());

    // Held by `ma`: `mb`'s route is evicted.
    edge.routes.insert(h.clone(), route());
    let gone = share_edge::control::evict_stale_routes(edge, D, &ma.public(), &wanted).await.unwrap();
    assert_eq!(gone, vec![h.clone()]);
    assert_eq!(rig.route_of(&h), None);

    // `ma` no longer holds `sky`: `mb`'s route survives.
    edge.store.release(D, "sky", &ma_s, now_ms(), 0).await.unwrap();
    edge.routes.insert(h.clone(), route());
    let gone = share_edge::control::evict_stale_routes(edge, D, &ma.public(), &wanted).await.unwrap();
    assert!(gone.is_empty());
    assert_eq!(rig.route_of(&h), Some(kb.public()));
    // Held by `mb` now: still nothing for `ma` to evict.
    let at = i64::try_from(now_ms() + 1_000).unwrap();
    let n = share_edge::store::NewName { domain: D, name: "sky", kind: "reserved", owner: &mb_s, now_ms: at, net: "n" };
    assert_eq!(edge.store.claim(n, 3).await.unwrap(), share_edge::store::Claimed::Granted);
    let gone = share_edge::control::evict_stale_routes(edge, D, &ma.public(), &wanted).await.unwrap();
    assert!(gone.is_empty());
    assert_eq!(rig.route_of(&h), Some(kb.public()));
}
