//! 191 P2 review (owner and routing side) end to end on loopback: the real a2a router's
//! `not owner` and the contact's client moving on, a member's gate refusal included.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agent::communication::network::{NOT_OWNER_CODE, NOT_OWNER_TEXT};

use super::*;

/// A stand-in contact machine: its gate admits while `open` holds; every call answers `code`
/// as an error (or a result), counted.
struct StandIn {
    code: Option<i32>,
    open: Arc<AtomicBool>,
    hits: Arc<AtomicUsize>,
}

impl mesh::net::ContactAlpn for StandIn {
    fn admit(&self, _: &str) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    fn stream(
        &self,
        _: String,
        stream: Box<dyn replica::ByteStream>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        let (code, hits) = (self.code, self.hits.clone());
        Box::pin(async move {
            let svc = hyper::service::service_fn(move |_req| {
                hits.fetch_add(1, Ordering::SeqCst);
                let body = match code {
                    Some(c) => serde_json::json!({"jsonrpc": "2.0", "id": "x",
                        "error": {"code": c, "message": NOT_OWNER_TEXT}}),
                    None => serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": {"ok": 1}}),
                };
                async move {
                    Ok::<_, std::convert::Infallible>(hyper::Response::new(
                        http_body_util::Full::new(bytes::Bytes::from(body.to_string())),
                    ))
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                .await;
        })
    }
}

struct Node {
    net: Arc<mesh::net::Net>,
    open: Arc<AtomicBool>,
    hits: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

impl Node {
    fn eid(&self) -> String {
        self.net.id.0.clone()
    }

    fn addrs(&self) -> Vec<std::net::SocketAddr> {
        self.net
            .a2a_reach()
            .0
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect()
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn stand_in(code: Option<i32>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let net = node("m", dir.path()).await;
    let (open, hits) = (Arc::new(AtomicBool::new(true)), Arc::default());
    net.set_contacts(Arc::new(StandIn {
        code,
        open: open.clone(),
        hits: Arc::clone(&hits),
    }));
    Node {
        net,
        open,
        hits,
        _dir: dir,
    }
}

/// The dialer's side: it serves nothing and knows the stand-ins' addresses, as the gate's
/// hints would give them.
struct Hints(Vec<(String, Vec<std::net::SocketAddr>)>);

impl mesh::net::ContactAlpn for Hints {
    fn admit(&self, _: &str) -> bool {
        false
    }

    fn stream(
        &self,
        _: String,
        _: Box<dyn replica::ByteStream>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn hints(&self, remote: &str) -> Vec<std::net::SocketAddr> {
        self.0
            .iter()
            .find(|(e, _)| e == remote)
            .map(|(_, a)| a.clone())
            .unwrap_or_default()
    }
}

async fn call(t: &reach::IrohTransport) -> Result<serde_json::Value, a2a_types::A2AError> {
    t.call(&Default::default(), "SendMessage", serde_json::json!({}))
        .await
}

/// 191 (D3, S4, P2 review SHOULD 4): a member whose gate refuses (it closes the connection
/// `not a contact`, here on the open connection after the request left) admitted nothing, so
/// the call moves to the next member; every member refusing, one of them `not owner`, is
/// `not owner`, each tried once; the gate alone is a refusal, never an internal error.
#[tokio::test]
async fn a_members_gate_refusal_moves_the_call_to_the_next_member() {
    let refusing = stand_in(None).await;
    let owner = stand_in(None).await;
    let elsewhere = stand_in(Some(NOT_OWNER_CODE)).await;
    let dir = tempfile::tempdir().unwrap();
    let me = node("me", dir.path()).await;
    me.set_contacts(Arc::new(Hints(vec![
        (owner.eid(), owner.addrs()),
        (elsewhere.eid(), elsewhere.addrs()),
    ])));
    let transport = |members: Vec<String>| reach::IrohTransport {
        net: me.clone(),
        eid: refusing.eid(),
        hints: reach::Hints {
            addrs: refusing.addrs(),
            relay: None,
        },
        export: None,
        members,
        answered: Default::default(),
    };
    let t = transport(vec![owner.eid()]);
    call(&t).await.unwrap();
    assert_eq!((refusing.hits(), owner.hits()), (1, 0));
    assert_eq!(t.answered.lock().clone(), Some(refusing.eid()));
    // Its gate refuses now: the request goes out on the open connection, which it closes.
    refusing.open.store(false, Ordering::SeqCst);
    assert_eq!(call(&t).await.unwrap(), serde_json::json!({"ok": 1}));
    assert_eq!((refusing.hits(), owner.hits()), (1, 1));
    assert_eq!(t.answered.lock().clone(), Some(owner.eid()));
    // The gate and `not owner`: `not owner`, each member once.
    let e = call(&transport(vec![elsewhere.eid()])).await.unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (NOT_OWNER_CODE, NOT_OWNER_TEXT)
    );
    assert_eq!((refusing.hits(), elsewhere.hits()), (1, 1));
    // The gate alone: refused (the sender records it so), never an unknown outcome.
    let e = call(&transport(Vec::new())).await.unwrap_err();
    assert_eq!(
        e.code,
        a2a_types::error_code::UNSUPPORTED_OPERATION,
        "{}",
        e.message
    );
}

/// 191 (D3, S4, P2 review Q1 and MUST 1): through the REAL router, a contact's message into
/// a chat another of our machines owns (W-40's case as it now stands) is `{code: -32030,
/// message: "not owner"}` before any task exists, and the contact's real client recognises
/// it: alone it returns it, with another member it moves on and that member answers.
#[tokio::test]
async fn the_real_router_at_a_non_owner_answers_not_owner_and_the_client_moves_on() {
    let lan = Lan::default();
    let n = engine("n", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n, &c).await;
    let agents = n.ctx.agents().unwrap();
    let chat = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session;
    publish(
        &n,
        "proj",
        ExportTarget::Chat { session: chat },
        "The project chat",
    )
    .await;
    grant_ask(&n, &alias, &["proj"]).await;
    agents
        .ask(agent::communication::network::OwnedElsewhereForTest(
            chat,
            "other-engine".into(),
        ))
        .await
        .unwrap();
    let owner = stand_in(None).await;
    c.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::SetContactAddrs {
            eid: owner.eid(),
            addrs: owner.addrs().iter().map(ToString::to_string).collect(),
        })
        .await
        .unwrap();
    let _ = c.ctx.tick().await;
    let client = |members: Vec<String>| {
        a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
            net: c.net.clone(),
            eid: n.eid(),
            hints: reach::Hints {
                addrs: n.addrs(),
                relay: None,
            },
            export: Some("proj".into()),
            members,
            answered: Default::default(),
        }) as Box<dyn a2a_client::Transport>)
    };
    let send = |id: &str| {
        let mut m =
            a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
        m.message_id = id.into();
        a2a_types::SendMessageRequest {
            message: m,
            configuration: Some(a2a_types::SendMessageConfiguration {
                accepted_output_modes: Some(vec!["text/plain".into()]),
                task_push_notification_config: None,
                history_length: Some(0),
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        }
    };
    let e = client(Vec::new())
        .send_message(&send("m-1"))
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (NOT_OWNER_CODE, NOT_OWNER_TEXT),
        "the router's own error, fixed text"
    );
    assert_eq!(owner.hits(), 0);
    // With another member: moved on, and that member answered.
    let moved = client(vec![owner.eid()]).send_message(&send("m-2")).await;
    assert_eq!(owner.hits(), 1, "{moved:?}");
}
