//! In-process edge: an engine attaches over iroh with a signed delegation, claims a random name and puts a
//! route; a TCP visitor's ClientHello then arrives on a bi-stream of the engine's connection behind a PROXY v2
//! preamble, byte for byte.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, RelayMode, SecretKey};
use share_edge::config::Config;
use share_proto::proxy::{TLV_AUTHORITY, TLV_CONN_ID, conn_id_from};
use share_proto::version::client_agent;
use share_proto::{
    Delegation, Hello, Info, NameGranted, NameRequest, RoutesPut, Welcome, names, paths,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

async fn call(
    conn: &Connection,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
) -> (u16, Vec<u8>) {
    let (send, recv) = conn.open_bi().await.expect("open_bi");
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

fn client_hello(sni: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("name");
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), name).expect("client");
    let mut out = Vec::new();
    while conn.wants_write() {
        conn.write_tls(&mut out).expect("write_tls");
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn visitor_hello_reaches_the_engine_behind_proxy_v2() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "domain: tun.zip\ndata_dir: {d}\nlisten: \"127.0.0.1:0\"\niroh: {{listen: \"127.0.0.1:0\"}}\n\
         terms_rev: \"r1\"\nadmin: {{socket: {d}/admin.sock}}\n",
        d = dir.path().display()
    );
    let (cfg, secrets) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let edge = share_edge::start_with(cfg, secrets, common::fakes().backends)
        .await
        .unwrap();

    let engine_key = SecretKey::generate();
    let mesh_key = SecretKey::generate();
    let ep = Endpoint::builder(presets::Minimal)
        .secret_key(engine_key.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let conn = ep
        .connect(edge.endpoint_addr(), share_proto::ALPN)
        .await
        .unwrap();

    // Anything but info before hello is refused.
    let (status, _) = call(
        &conn,
        "PUT",
        paths::ROUTES,
        Some(b"{\"hosts\":[]}".to_vec()),
    )
    .await;
    assert_eq!(status, 403);

    let (status, body) = call(&conn, "GET", paths::INFO, None).await;
    assert_eq!(status, 200);
    let info: Info = serde_json::from_slice(&body).unwrap();
    assert_eq!(info.domain, "tun.zip");

    let d = Delegation::new(
        info.domain.clone(),
        info.edge_id,
        mesh_key.public(),
        engine_key.public(),
        now_ms(),
        info.now_ms,
    );
    let hello = Hello {
        v: 1,
        delegation: d.sign(&mesh_key).unwrap(),
        invite: None,
        client: client_agent("2.25.5"),
        terms_rev: Some(info.terms_rev.clone()),
    };
    let (status, body) = call(
        &conn,
        "POST",
        paths::HELLO,
        Some(serde_json::to_vec(&hello).unwrap()),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let welcome: Welcome = serde_json::from_slice(&body).unwrap();
    assert_eq!(welcome.mesh, mesh_key.public());
    assert!(welcome.terms_ok);

    let name = names::random_name([1, 2, 3, 4, 5, 6]);
    let req = NameRequest { name: name.clone() };
    let (status, body) = call(
        &conn,
        "POST",
        paths::NAMES,
        Some(serde_json::to_vec(&req).unwrap()),
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let granted: NameGranted = serde_json::from_slice(&body).unwrap();
    assert_eq!(granted.name, name);

    // A host under a name the mesh does not hold is refused, and nothing is applied.
    let bad = RoutesPut {
        hosts: vec![format!("x.{name}.tun.zip"), "x.other1.tun.zip".into()],
        ..Default::default()
    };
    let (status, _) = call(
        &conn,
        "PUT",
        paths::ROUTES,
        Some(serde_json::to_vec(&bad).unwrap()),
    )
    .await;
    assert_eq!(status, 403);

    let host = format!("x.{name}.tun.zip");
    let routes = RoutesPut {
        hosts: vec![host.clone()],
        ..Default::default()
    };
    let (status, body) = call(
        &conn,
        "PUT",
        paths::ROUTES,
        Some(serde_json::to_vec(&routes).unwrap()),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));

    // An unknown host gets a reset, no bytes.
    let mut stray = tokio::net::TcpStream::connect(edge.tcp_addr).await.unwrap();
    stray
        .write_all(&client_hello(&format!("y.{name}x.tun.zip")))
        .await
        .unwrap();
    let mut one = [0u8; 1];
    let r = tokio::time::timeout(Duration::from_secs(5), stray.read(&mut one))
        .await
        .unwrap();
    assert!(matches!(r, Ok(0) | Err(_)), "{r:?}");

    // The visitor.
    let raw = client_hello(&host);
    let engine_conn = conn.clone();
    let want = raw.len();
    let engine_side = tokio::spawn(async move {
        let (_send, mut recv) = engine_conn.accept_bi().await.unwrap();
        let mut fixed = [0u8; 16];
        recv.read_exact(&mut fixed).await.unwrap();
        let len = u16::from_be_bytes([fixed[14], fixed[15]]) as usize;
        let mut rest = vec![0u8; len];
        recv.read_exact(&mut rest).await.unwrap();
        let mut preamble = fixed.to_vec();
        preamble.extend_from_slice(&rest);
        let mut hello = vec![0u8; want];
        recv.read_exact(&mut hello).await.unwrap();
        (preamble, hello)
    });
    let mut tcp = tokio::net::TcpStream::connect(edge.tcp_addr).await.unwrap();
    tcp.write_all(&raw).await.unwrap();
    let (preamble, got) = tokio::time::timeout(Duration::from_secs(10), engine_side)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, raw, "the exact ClientHello bytes follow the preamble");
    let header = ppp::v2::Header::try_from(preamble.as_slice()).unwrap();
    match header.addresses {
        ppp::v2::Addresses::IPv4(a) => {
            let local = tcp.local_addr().unwrap();
            assert_eq!(SocketAddr::from((a.source_address, a.source_port)), local);
        }
        other => panic!("addresses {other:?}"),
    }
    let tlvs: Vec<_> = header.tlvs().map(Result::unwrap).collect();
    assert!(
        tlvs.iter()
            .any(|t| t.kind == TLV_AUTHORITY && &*t.value == host.as_bytes())
    );
    assert!(
        tlvs.iter()
            .any(|t| t.kind == TLV_CONN_ID && conn_id_from(&t.value).is_some())
    );

    edge.shutdown().await;
}

fn hello_body(v: u32, client: &str, info: &Info, mesh: &SecretKey, engine: &SecretKey) -> Vec<u8> {
    let d = Delegation::new(
        info.domain.clone(),
        info.edge_id,
        mesh.public(),
        engine.public(),
        now_ms(),
        info.now_ms,
    );
    serde_json::to_vec(&Hello {
        v,
        delegation: d.sign(mesh).unwrap(),
        invite: None,
        client: client.into(),
        terms_rev: None,
    })
    .unwrap()
}

/// FIX-EDGE: `/v1/info` rate limit, protocol and client version gates, routes refused after a Stop all until a
/// new `hello`, and the admin socket's mode and line cap.
#[tokio::test(flavor = "multi_thread")]
async fn control_gates() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "domain: tun.zip\ndata_dir: {d}\nlisten: \"127.0.0.1:0\"\niroh: {{listen: \"127.0.0.1:0\"}}\n\
         min_client: \"2.0.0\"\nlimits: {{info_per_ip_min: 2}}\nadmin: {{socket: {d}/admin.sock}}\n",
        d = dir.path().display()
    );
    let (cfg, secrets) = Config::from_yaml(&yaml, &|_| None).unwrap();
    let edge = share_edge::start_with(cfg, secrets, common::fakes().backends)
        .await
        .unwrap();
    let engine_key = SecretKey::generate();
    let mesh_key = SecretKey::generate();
    let ep = Endpoint::builder(presets::Minimal)
        .secret_key(engine_key.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let conn = ep
        .connect(edge.endpoint_addr(), share_proto::ALPN)
        .await
        .unwrap();

    // m9: two per minute from this network, then 429 rate_limited.
    let (status, body) = call(&conn, "GET", paths::INFO, None).await;
    assert_eq!(status, 200);
    let info: Info = serde_json::from_slice(&body).unwrap();
    assert_eq!(call(&conn, "GET", paths::INFO, None).await.0, 200);
    let (status, body) = call(&conn, "GET", paths::INFO, None).await;
    assert_eq!(status, 429);
    assert!(String::from_utf8_lossy(&body).contains("rate_limited"));

    // Another protocol major: 426 protocol_mismatch naming the served majors; a client below min_client: 426
    // client_too_old.
    for (v, client, code) in [
        (2, client_agent("2.25.5"), "protocol_mismatch"),
        (1, client_agent("1.9.9"), "client_too_old"),
    ] {
        let body = hello_body(v, &client, &info, &mesh_key, &engine_key);
        let (status, body) = call(&conn, "POST", paths::HELLO, Some(body)).await;
        assert_eq!(status, 426, "v{v} {client}");
        let e: share_proto::ErrorBody = serde_json::from_slice(&body).unwrap();
        assert_eq!(e.code.as_str(), code);
        if v == 2 {
            assert_eq!(e.supported, share_proto::SUPPORTED_MAJORS.to_vec());
        }
    }

    let good = || hello_body(1, &client_agent("2.25.5"), &info, &mesh_key, &engine_key);
    let (status, body) = call(&conn, "POST", paths::HELLO, Some(good())).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let name = names::random_name([9, 8, 7, 6, 5, 4]);
    let req = serde_json::to_vec(&NameRequest { name: name.clone() }).unwrap();
    assert_eq!(call(&conn, "POST", paths::NAMES, Some(req)).await.0, 201);
    let routes = serde_json::to_vec(&RoutesPut {
        hosts: vec![format!("x.{name}.tun.zip")],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(call(&conn, "PUT", paths::ROUTES, Some(routes.clone())).await.0, 204);

    // M7: a Stop all after this session's hello (here written straight to the store, so the tunnel is not
    // killed): routes are refused until a new hello.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let mesh = mesh_key.public().to_string();
    edge.edge.store.stop_all(&mesh, now_ms()).await.unwrap();
    let (status, body) = call(&conn, "PUT", paths::ROUTES, Some(routes.clone())).await;
    assert_eq!(status, 403);
    assert!(String::from_utf8_lossy(&body).contains("hello_required"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(call(&conn, "POST", paths::HELLO, Some(good())).await.0, 200);
    assert_eq!(call(&conn, "PUT", paths::ROUTES, Some(routes)).await.0, 204);

    // m2: 0600 from the start (bound in a 0700 staging directory, nothing left behind).
    let sock = dir.path().join("admin.sock");
    let mode = std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let stray: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".admin-bind-"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");

    // m1: a request line over the cap gets `too_large` and the connection closes.
    let mut admin = tokio::net::UnixStream::connect(&sock).await.unwrap();
    let long = vec![b'x'; share_edge::admin::MAX_LINE + 1];
    admin.write_all(&long).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), admin.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&out).contains("too_large"), "{}", String::from_utf8_lossy(&out));

    edge.shutdown().await;
}
