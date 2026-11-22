//! 209 §4.4, two engines in one process: a session owned by engine "a" attaches to its tile and
//! reads it; each tile call is relayed by a's event relay to the front door "b" (the session's
//! `Watch` stream), re-published on b's wire feed, claimed and answered there through b's real
//! `answer_web_view_call` by a client connection of b, forwarded to "a" over `theuth/rpc/1`, and
//! a's `browser_*` tool reads the answer. A second client connection of b cannot answer a call
//! the first one claimed. Own binary: it registers the agent, replica and sessions actors on b,
//! which the process-wide kameo registry allows once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::store::Store;
use artifact::webview::{WebViewCallAnswer, WebViewState};
use futures::StreamExt as _;
use kameo::actor::Spawn as _;
use replica::{ALPN_RPC, ALPN_SESSIONS, ByteStream, PeerEvent, PeerId, Transport};
use runtime::{Plugin as _, Runtime, SessionId};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use wire::RuntimeWire as _;

type Inbound = (PeerId, Box<dyn ByteStream>);

const VIEW: &str = "7e1a900000000001";

/// One side of a two-engine link carrying `theuth/rpc/1` and `theuth/sessions/1`.
struct Link {
    me: PeerId,
    peer: PeerId,
    to_peer: HashMap<&'static str, mpsc::UnboundedSender<Inbound>>,
    inbound: HashMap<&'static str, tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>>,
    events: broadcast::Sender<PeerEvent>,
}

fn link() -> (Arc<Link>, Arc<Link>) {
    let mut to_a = HashMap::new();
    let mut to_b = HashMap::new();
    let mut in_a = HashMap::new();
    let mut in_b = HashMap::new();
    for alpn in [ALPN_RPC, ALPN_SESSIONS] {
        let (tx, rx) = mpsc::unbounded_channel();
        to_a.insert(alpn, tx);
        in_a.insert(alpn, tokio::sync::Mutex::new(rx));
        let (tx, rx) = mpsc::unbounded_channel();
        to_b.insert(alpn, tx);
        in_b.insert(alpn, tokio::sync::Mutex::new(rx));
    }
    let a = Arc::new(Link {
        me: PeerId("a".into()),
        peer: PeerId("b".into()),
        to_peer: to_b,
        inbound: in_a,
        events: broadcast::channel(8).0,
    });
    let b = Arc::new(Link {
        me: PeerId("b".into()),
        peer: PeerId("a".into()),
        to_peer: to_a,
        inbound: in_b,
        events: broadcast::channel(8).0,
    });
    (a, b)
}

#[async_trait::async_trait]
impl Transport for Link {
    fn peers(&self) -> Vec<PeerId> {
        vec![self.peer.clone()]
    }

    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::ensure!(*peer == self.peer, "no such peer");
        let to = self
            .to_peer
            .get(alpn)
            .ok_or_else(|| anyhow::anyhow!("refused"))?;
        let (mine, theirs) = tokio::io::duplex(1 << 20);
        to.send((self.me.clone(), Box::new(theirs)))
            .map_err(|_| anyhow::anyhow!("peer gone"))?;
        Ok(Box::new(mine))
    }

    async fn accept(&self, alpn: &str) -> anyhow::Result<Inbound> {
        let Some(rx) = self.inbound.get(alpn) else {
            std::future::pending::<()>().await;
            unreachable!()
        };
        rx.lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("closed"))
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }
}

/// The owner's `theuth/sessions/1` accept loop: every stream to `relay`.
fn accept_sessions(relay: kameo::actor::ActorRef<sessionsync::events::EventRelay>, t: Arc<Link>) {
    use sessionsync::types::Msg;
    tokio::spawn(async move {
        while let Ok((_, stream)) = t.accept(ALPN_SESSIONS).await {
            let mut framed = sessionsync::events::framed(stream);
            let Some(Ok(first)) = framed.next().await else {
                continue;
            };
            let (session, from_seq, bus) = match Msg::decode(&first) {
                Ok(Msg::Events { session, from_seq }) => (session, from_seq, false),
                Ok(Msg::Watch { session, from_seq }) => (session, from_seq, true),
                _ => continue,
            };
            let r = relay.clone();
            tokio::spawn(async move {
                sessionsync::events::serve(&r, framed, session, from_seq, bus).await
            });
        }
    });
}

/// The next frame of `topic` on a wire feed (5 s at most).
async fn next_of(feed: &mut broadcast::Receiver<Arc<wire::WireFrame>>, topic: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let f = feed.recv().await.expect("wire feed");
            if f.descriptor.as_ref().is_some_and(|d| d.topic == topic) {
                return f.event.clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {topic} within 5 s"))
}

/// The artifact RPC dispatch of `rt` (forwarding hooks installed) on a fresh owner connection.
fn connection(rt: &Runtime) -> (jsonrpsee::Methods, rpc_core::RpcPeerGuard) {
    let mut methods: jsonrpsee::Methods = artifact::rpc_module(rpc_compose::new_server(rt.clone()))
        .module
        .into();
    let (peer, guard) = rpc_core::RpcPeer::open();
    methods.extensions_mut().insert(peer);
    (methods, guard)
}

async fn answer(on: &jsonrpsee::Methods, request: &Value, answer: WebViewCallAnswer) -> bool {
    let id: artifact::webview::WebViewCallId =
        serde_json::from_value(request["id"].clone()).unwrap();
    let session: SessionId = serde_json::from_value(request["session"].clone()).unwrap();
    on.call("answer_web_view_call", (id, answer, Some(session)))
        .await
        .expect("answer_web_view_call")
}

fn claimed() -> WebViewCallAnswer {
    WebViewCallAnswer::Claimed {
        client: "ipad".into(),
    }
}

fn done(result: Value) -> WebViewCallAnswer {
    WebViewCallAnswer::Done { result, png: None }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tile_call_is_answered_at_the_front_door() {
    let dir = tempfile::tempdir().unwrap();
    let (la, lb) = link();
    let session = SessionId::new();

    // The owner "a": the session's row, its event relay serving `Watch` streams, its RPC
    // answering forwarded calls, and the browser tools of its agent.
    let rt_a = Runtime::new(figment::Figment::new(), dir.path().join("a"));
    artifact::declare_wire(&rt_a).await.unwrap();
    let store_a = Store::open(
        &dir.path().join("a.sqlite"),
        Some(mesh::EngineId("a".into())),
    )
    .await
    .unwrap();
    store_a
        .create_session(session, "m", None, None, None)
        .await
        .unwrap();
    let relay_a = sessionsync::events::EventRelay::spawn_on(&rt_a, store_a.clone()).await;
    relay_a
        .tell(sessionsync::events::SetTransport(la.clone()))
        .await
        .unwrap();
    accept_sessions(relay_a, la.clone());
    let (api_a, _conn_a) = connection(&rt_a);
    tokio::spawn(agent::rpc_forward::serve(rt_a.clone(), la, api_a));
    let cfg = browser::BrowserConfig::default();
    let tools_a = browser::tools::all(
        browser::Browsers::spawn(browser::Browsers::new(cfg.clone())),
        cfg,
        rt_a.clone(),
    );
    // The session opened the tile (its show_web ran on a).
    artifact::webview::record_state(WebViewState {
        session,
        view: VIEW.into(),
        url: "https://mail.test/".into(),
        title: "Mail".into(),
        loading: false,
        closed: false,
    });

    // The front door "b": its replica row names "a" as the owner (written under a's identity),
    // its agent store, replica transport and sessions plugin (the relay and the forwarding).
    let rt_b = Runtime::new(figment::Figment::new(), dir.path().join("b"));
    std::fs::create_dir_all(&rt_b.data_dir).unwrap();
    let theirs = Store::open(
        &rt_b.data_dir.join("sessions.sqlite"),
        Some(mesh::EngineId("a".into())),
    )
    .await
    .unwrap();
    theirs
        .create_session(session, "m", None, None, None)
        .await
        .unwrap();
    theirs.close().await;
    agent::spawn_test_agents(rt_b.clone(), |_: &str| true)
        .await
        .unwrap();
    artifact::declare_wire(&rt_b).await.unwrap();
    Box::new(replica::ReplicaPlugin).start(&rt_b).await.unwrap();
    replica::actor()
        .unwrap()
        .ask(replica::RegisterTransport(lb))
        .await
        .unwrap();
    Box::new(sessionsync::SessionSyncPlugin)
        .start(&rt_b)
        .await
        .unwrap();
    let (api_b, _conn_b) = connection(&rt_b);
    let (other_b, _conn_other) = connection(&rt_b);

    // A client of b watches the session: b follows a's relay.
    let mut feed = rt_b.wire_events();
    let _following = sessionsync::events::follow(session).expect("relay running");
    let live = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if next_of(
                &mut feed,
                <sessionsync::events::FollowState as wire::WireEvent>::TOPIC,
            )
            .await["live"]
                == true
            {
                return;
            }
        }
    })
    .await;
    assert!(live.is_ok(), "b never followed a live");

    let call = |name: &str, args: Value| {
        let tool = tools_a
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap()
            .clone();
        let cx = tools::ToolCx {
            session,
            call_id: "c".into(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(dir.path()),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        };
        tokio::spawn(tools::egress::scope(true, async move {
            tool.call(args, cx).await.unwrap()
        }))
    };

    // Attach on a; the probe is claimed and answered at b.
    let attach = call("browser_attach", json!({ "view": format!("web:{VIEW}") }));
    let r = next_of(&mut feed, "artifact.web-view-call-requested").await;
    assert_eq!(r["view"], VIEW);
    assert_eq!(r["delegated"], false);
    assert_eq!(r["op"], "script");
    assert!(answer(&api_b, &r, claimed()).await, "the claim reaches a");
    assert!(
        answer(
            &api_b,
            &r,
            done(json!({ "url": "https://mail.test/inbox", "title": "Inbox" }))
        )
        .await
    );
    let out = attach.await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .starts_with(&format!("[tile web:{VIEW} \u{b7} mail.test]")),
        "{}",
        out.content
    );
    assert!(out.content.contains("\"Inbox\""), "{}", out.content);
    let resolved = next_of(&mut feed, "artifact.web-view-call-resolved").await;
    assert_eq!(resolved["id"], r["id"]);

    // A read: claimed by one client of b; another client of b cannot answer it.
    let read = call("browser_read", json!({}));
    let r = next_of(&mut feed, "artifact.web-view-call-requested").await;
    assert_eq!(r["args"]["arguments"]["cmd"], "read");
    assert!(answer(&api_b, &r, claimed()).await);
    assert!(
        !answer(&other_b, &r, claimed()).await,
        "the first claim wins"
    );
    let forged = json!({ "url": "https://mail.test/inbox", "title": "Inbox", "text": "FORGED" });
    assert!(!answer(&other_b, &r, done(forged)).await, "not the claimer");
    let real =
        json!({ "url": "https://mail.test/inbox", "title": "Inbox", "text": "Three new mails" });
    assert!(answer(&api_b, &r, done(real)).await);
    let out = read.await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.ends_with("Three new mails"), "{}", out.content);
    assert!(!out.content.contains("FORGED"), "{}", out.content);
    store_a.close().await;
}
