//! The §5.9 state machine against a fake edge: an in-process axum router served over duplex
//! streams, one HTTP/1.1 request per "bi-stream", SSE on `/v1/events`.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use parking_lot::Mutex;
use share_proto::{Admission, ErrorBody, SecretKey, WelcomeLimits};

use super::*;

struct Fake {
    edge: SecretKey,
    too_old: bool,
    dials: AtomicU32,
    hellos: AtomicU32,
    routes: Mutex<Vec<Vec<String>>>,
    sse: Mutex<Option<mpsc::Sender<Result<Bytes, Infallible>>>>,
}

const DOMAIN: &str = "tun.zip";

fn info(f: &Fake) -> Info {
    Info {
        v: 1,
        edge: "theuth-share-edge/0.1.0".into(),
        domain: DOMAIN.into(),
        edge_id: f.edge.public(),
        now_ms: platform::time::now_ms(),
        min_client: "0.0.1".into(),
        admission: Admission::Open,
        terms_url: "https://tun.zip/terms".into(),
        terms_rev: "1".into(),
        abuse_url: "https://tun.zip/abuse".into(),
        full: false,
    }
}

async fn h_info(State(f): State<Arc<Fake>>) -> axum::response::Response {
    if f.too_old {
        let b = ErrorBody::new(ErrorCode::ClientTooOld);
        return (http::StatusCode::UPGRADE_REQUIRED, Json(b)).into_response();
    }
    Json(info(&f)).into_response()
}

async fn h_hello(State(f): State<Arc<Fake>>, Json(h): Json<Hello>) -> axum::response::Response {
    let d = share_proto::Delegation::open(&h.delegation).unwrap();
    assert_eq!(d.edge, f.edge.public());
    f.hellos.fetch_add(1, Ordering::SeqCst);
    Json(Welcome {
        v: 1,
        now_ms: platform::time::now_ms(),
        min_client: "0.0.1".into(),
        mesh: d.mesh,
        names: vec![],
        routes: vec![],
        revoked: vec![],
        stop_all_ms: None,
        domain: DOMAIN.into(),
        limits: WelcomeLimits::default(),
        terms_url: String::new(),
        abuse_url: String::new(),
        terms_rev: "1".into(),
        terms_ok: true,
        full: false,
        retire: None,
        throttled: false,
    })
    .into_response()
}

async fn h_routes(State(f): State<Arc<Fake>>, Json(r): Json<RoutesPut>) -> http::StatusCode {
    f.routes.lock().push(r.hosts);
    http::StatusCode::NO_CONTENT
}

async fn h_events(State(f): State<Arc<Fake>>) -> axum::response::Response {
    let (tx, rx) = mpsc::channel(8);
    *f.sse.lock() = Some(tx);
    let stream =
        futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|x| (x, rx)) });
    (
        [(http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// An in-process edge connection: every bi-stream is one HTTP/1.1 connection to `app`.
pub(crate) struct FakeConn {
    app: Router,
    closed: tokio::sync::Notify,
}

impl FakeConn {
    /// A connection serving `app`.
    pub(crate) fn new(app: Router) -> Arc<Self> {
        Arc::new(Self {
            app,
            closed: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl EdgeConn for FakeConn {
    async fn open(&self) -> io::Result<Stream> {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let svc = hyper_util::service::TowerToHyperService::new(self.app.clone());
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(server), svc)
                .await;
        });
        Ok(Box::new(client))
    }

    async fn accept(&self) -> Option<Stream> {
        self.closed.notified().await;
        None
    }

    fn close(&self) {
        self.closed.notify_waiters();
    }
}

struct FakeDialer(Arc<Fake>);

#[async_trait::async_trait]
impl EdgeDialer for FakeDialer {
    async fn dial(&self, addr: EndpointAddr, alpns: &[&[u8]]) -> anyhow::Result<Arc<dyn EdgeConn>> {
        assert_eq!(addr.id, self.0.edge.public());
        assert_eq!(alpns, share_proto::ALPNS);
        self.0.dials.fetch_add(1, Ordering::SeqCst);
        let app = Router::new()
            .route(paths::INFO, get(h_info))
            .route(paths::HELLO, post(h_hello))
            .route(paths::ROUTES, put(h_routes))
            .route(paths::EVENTS, get(h_events))
            .with_state(self.0.clone());
        Ok(FakeConn::new(app))
    }
}

struct Att {
    states: watch::Sender<Vec<EdgeState>>,
}

#[async_trait::async_trait]
impl Attachment for Att {
    fn hosts(&self) -> Vec<String> {
        vec!["app.sky.tun.zip".into()]
    }
    async fn welcome(&self, _: &Control, _: &Info, _: &Welcome) {}
    async fn event(&self, _: &Control, _: Event) {}
    fn visitor(&self, _: Stream) {}
    fn lost(&self) {}
    async fn removed(&self) {}
    fn state(&self, s: &EdgeState, _: Option<i64>) {
        self.states.send_modify(|v| v.push(s.clone()));
    }
}

fn run(
    too_old: bool,
) -> (
    Arc<Fake>,
    watch::Receiver<Vec<EdgeState>>,
    watch::Sender<bool>,
) {
    let fake = Arc::new(Fake {
        edge: SecretKey::from_bytes(&[7; 32]),
        too_old,
        dials: AtomicU32::new(0),
        hellos: AtomicU32::new(0),
        routes: Mutex::new(vec![]),
        sse: Mutex::new(None),
    });
    let mesh = SecretKey::from_bytes(&[1; 32]);
    let states = watch::channel(Vec::new()).0;
    let rx = states.subscribe();
    let client = Client {
        dialer: Arc::new(FakeDialer(fake.clone())),
        delegation: Arc::new(SelfHolder {
            mesh_key: mesh,
            engine: SecretKey::from_bytes(&[2; 32]).public(),
        }),
        tickets: vec![ShareEdgeTicket {
            addr: EndpointAddr::new(fake.edge.public()),
            invite: None,
        }],
        terms_rev: watch::channel(None).1,
        attachment: Arc::new(Att { states }),
        timing: Timing {
            backoff: |_| Duration::from_millis(10),
            renew: Duration::from_secs(3600),
            clock_retry: Duration::from_millis(10),
            drain_redial: || Duration::from_millis(10),
        },
    };
    let need = watch::channel(false).0;
    tokio::spawn(client.run(need.subscribe()));
    (fake, rx, need)
}

async fn until(rx: &mut watch::Receiver<Vec<EdgeState>>, n: usize, s: EdgeState) {
    tokio::time::timeout(
        Duration::from_secs(5),
        rx.wait_for(|v| v.iter().filter(|x| **x == s).count() >= n),
    )
    .await
    .expect("state reached")
    .unwrap();
}

#[tokio::test]
async fn attaches_sends_routes_and_redials_after_drain() {
    let (fake, mut rx, need) = run(false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        fake.dials.load(Ordering::SeqCst),
        0,
        "nothing dials without a need"
    );
    need.send_replace(true);
    until(&mut rx, 1, EdgeState::Attached).await;
    assert_eq!(
        fake.routes.lock().clone(),
        vec![vec!["app.sky.tun.zip".to_owned()]]
    );
    assert_eq!(fake.hellos.load(Ordering::SeqCst), 1);
    let order: Vec<EdgeState> = rx.borrow().clone();
    assert_eq!(
        &order[..6],
        &[
            EdgeState::Idle,
            EdgeState::Dialing { attempt: 0 },
            EdgeState::Info,
            EdgeState::Delegating,
            EdgeState::Hello,
            EdgeState::Attached
        ]
    );

    let tx = fake.sse.lock().clone().unwrap();
    tx.send(Ok(Bytes::from("event: drain\ndata: {\"secs\":0}\n\n")))
        .await
        .unwrap();
    until(&mut rx, 1, EdgeState::Draining).await;
    until(&mut rx, 2, EdgeState::Attached).await;
    assert_eq!(fake.dials.load(Ordering::SeqCst), 2);

    need.send_replace(false);
    until(&mut rx, 2, EdgeState::Idle).await;
}

#[tokio::test]
async fn client_too_old_blocks() {
    let (_fake, mut rx, need) = run(true);
    need.send_replace(true);
    until(
        &mut rx,
        1,
        EdgeState::Blocked {
            reason: "client_too_old".into(),
        },
    )
    .await;
}

#[test]
fn sse_parser_splits_records_and_skips_unknown() {
    let mut p = SseParser::default();
    assert!(p.push(b"event: cert\ndata: {\"name\":").is_empty());
    let evs = p.push(
        b"\"sky\"}\n\nevent: future\ndata: {}\n\nevent: kill\ndata: {\"reason\":\"stop_all\"}\n\n",
    );
    assert_eq!(evs.len(), 2);
    assert!(matches!(&evs[0], Event::Cert(c) if c.name == "sky"));
    assert!(
        matches!(&evs[1], Event::Kill(k) if k.reason == KillReason::StopAll && k.host.is_none())
    );
}

#[test]
fn sse_parser_keeps_split_multibyte_and_caps_sizes() {
    let mut p = SseParser::default();
    let rec = "event: cert\ndata: {\"name\":\"é\"}\n\n".as_bytes();
    let cut = rec.iter().position(|&b| b == 0xC3).unwrap() + 1;
    assert!(p.push(&rec[..cut]).is_empty());
    let evs = p.push(&rec[cut..]);
    assert!(matches!(&evs[..], [Event::Cert(c)] if c.name == "é"));

    // An over-long line is dropped whole, across chunks, and the parser recovers.
    let long = vec![b'x'; SSE_MAX_LINE + 1];
    assert!(p.push(b"data: ").is_empty());
    assert!(p.push(&long).is_empty());
    assert!(p.push(&long).is_empty());
    let evs = p.push(b"tail\n\nevent: cert\ndata: {\"name\":\"ok\"}\n\n");
    assert!(matches!(&evs[..], [Event::Cert(c)] if c.name == "ok"));

    // An event whose data outgrows the cap is dropped, the next one parses.
    let chunk = format!("data: {}\n", "y".repeat(SSE_MAX_LINE - 16));
    let mut big = String::from("event: cert\n");
    while big.len() <= SSE_MAX_DATA {
        big.push_str(&chunk);
    }
    big.push_str("\nevent: cert\ndata: {\"name\":\"after\"}\n\n");
    let evs = p.push(big.as_bytes());
    assert!(matches!(&evs[..], [Event::Cert(c)] if c.name == "after"));
}

#[test]
fn protocol_versions_are_checked() {
    use crate::edge::{Incompatible, compat};
    let me = "theuth/2.26.0";
    assert_eq!(compat(share_proto::PROTOCOL_MAJOR, "2.0.0", me), Ok(()));
    assert_eq!(
        compat(share_proto::PROTOCOL_MAJOR + 1, "2.0.0", me),
        Err(Incompatible::Protocol {
            edge: share_proto::PROTOCOL_MAJOR + 1,
            ours: share_proto::PROTOCOL_MAJOR
        })
    );
    let e = compat(share_proto::PROTOCOL_MAJOR, "3.0.0", me).unwrap_err();
    assert!(e.to_string().starts_with("client_too_old"));
}
