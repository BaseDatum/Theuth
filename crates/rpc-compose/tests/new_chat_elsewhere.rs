//! 183 §5.6: "New chat on <another machine>" — the target engine creates and
//! owns the row, and the first `send_input` (and the chat route's reads) must
//! reach it even while that row has not replicated to the caller. Here the
//! target never ships its log at all (a session log held indefinitely): the
//! caller's `create_session` waits 1 s for the row and answers, every
//! following call is forwarded by id, and the row still missing after 6 s
//! logs the WARN. Its own binary: it registers the agent, replica and
//! sessions actors, which the process-wide kameo registry allows once.

use std::io::Write;
use std::sync::Arc;

use agent::{CreateSessionOptions, SessionOpen};
use jsonrpsee::core::RpcResult;
use parking_lot::Mutex;
use replica::{ALPN_RPC, ByteStream, PeerEvent, PeerId, Transport};

use runtime::{Plugin as _, Runtime, SessionId};
use tokio::sync::{broadcast, mpsc};

type Inbound = (PeerId, Box<dyn ByteStream>);

/// One side of a two-engine link that carries only `theuth/rpc/1`: any other
/// ALPN is refused on open and never accepted, so the caller's session log,
/// record sync and event streams get nothing from the target.
struct Link {
    me: PeerId,
    peer: PeerId,
    to_peer: mpsc::UnboundedSender<Inbound>,
    rpc_in: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
    events: broadcast::Sender<PeerEvent>,
}

fn link() -> (Arc<Link>, Arc<Link>) {
    let (a_tx, a_rx) = mpsc::unbounded_channel();
    let (b_tx, b_rx) = mpsc::unbounded_channel();
    let mk = |me: &str, peer: &str, to_peer, rpc_in| {
        Arc::new(Link {
            me: PeerId(me.into()),
            peer: PeerId(peer.into()),
            to_peer,
            rpc_in: tokio::sync::Mutex::new(rpc_in),
            events: broadcast::channel(8).0,
        })
    };
    (mk("a", "b", b_tx, a_rx), mk("b", "a", a_tx, b_rx))
}

#[async_trait::async_trait]
impl Transport for Link {
    fn peers(&self) -> Vec<PeerId> {
        vec![self.peer.clone()]
    }

    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::ensure!(*peer == self.peer && alpn == ALPN_RPC, "refused");
        let (mine, theirs) = tokio::io::duplex(1 << 20);
        self.to_peer
            .send((self.me.clone(), Box::new(theirs)))
            .map_err(|_| anyhow::anyhow!("peer gone"))?;
        Ok(Box::new(mine))
    }

    async fn accept(&self, alpn: &str) -> anyhow::Result<Inbound> {
        if alpn != ALPN_RPC {
            std::future::pending::<()>().await;
        }
        self.rpc_in
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("closed"))
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }
}

/// What reached the target: `(method, session)`.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// The target engine's front: `create_session` makes a row only it holds;
/// every other method records that it was asked.
fn target(seen: Seen) -> jsonrpsee::Methods {
    let mut m = jsonrpsee::RpcModule::new(seen);
    m.register_method("create_session", |_, seen, _| -> RpcResult<SessionOpen> {
        let session = SessionId::new();
        seen.lock()
            .push(("create_session".into(), session.to_string()));
        Ok(SessionOpen {
            session,
            root: session,
            retention: tools::Retention::Durable,
            lease_secret: None,
        })
    })
    .unwrap();
    m.register_method("send_input", |p, seen, _| -> RpcResult<String> {
        let session: Option<String> = p.sequence().next()?;
        let session = session.unwrap_or_default();
        seen.lock().push(("send_input".into(), session.clone()));
        Ok(session)
    })
    .unwrap();
    m.register_method("load_session", |p, seen, _| -> RpcResult<agent::Loaded> {
        let session: String = p.sequence().next()?;
        seen.lock().push(("load_session".into(), session));
        Ok(agent::Loaded::default())
    })
    .unwrap();
    m.register_method("list_agents", |p, seen, _| -> RpcResult<Vec<()>> {
        let session: String = p.sequence().next()?;
        seen.lock().push(("list_agents".into(), session));
        Ok(Vec::new())
    })
    .unwrap();
    m.register_method(
        "get_todos",
        |p, seen, _| -> RpcResult<agent::todo::TodoSnapshot> {
            let session: String = p.sequence().next()?;
            seen.lock().push(("get_todos".into(), session));
            Ok(agent::todo::TodoSnapshot::default())
        },
    )
    .unwrap();
    m.register_method("cell_state", |p, seen, _| -> RpcResult<serde_json::Value> {
        let scope: serde_json::Value = p.sequence().next()?;
        let session = scope["id"].as_str().unwrap_or_default().to_owned();
        seen.lock().push(("cell_state".into(), session));
        Ok(serde_json::json!({ "cells": [], "kernels": [], "pass": null }))
    })
    .unwrap();
    // The owner's registry holds the chat's hides: its answer says `read` is off.
    m.register_method(
        "list_mcp",
        |p, seen, _| -> RpcResult<Vec<tools::McpServer>> {
            let session: Option<String> = p.sequence().next()?;
            seen.lock()
                .push(("list_mcp".into(), session.unwrap_or_default()));
            Ok(vec![hidden_read()])
        },
    )
    .unwrap();
    m.into()
}

/// One server whose `read` the owner hid for the chat.
fn hidden_read() -> tools::McpServer {
    tools::McpServer {
        name: "fs".into(),
        connected: true,
        gate: None,
        tools: vec![tools::McpToolInfo {
            name: "read".into(),
            description: String::new(),
            gate: None,
            risk: tools::Risk::ReadOnly,
            action: tools::PolicyAction::Allow,
            registered: Some("read".into()),
            hidden: true,
        }],
        repo: None,
        auth: tools::McpAuth::None,
        home: None,
        home_name: None,
        home_here: true,
        everywhere: false,
        state: tools::McpState::Running,
        reason: None,
    }
}

/// Log lines, for the WARN.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_chat_created_elsewhere_is_forwarded_by_id_while_in_use() {
    let logs = Captured::default();
    let sink = logs.clone();
    tracing_subscriber::fmt()
        .with_env_filter("rpc=warn")
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .init();

    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let (a, b) = link();

    // The target: answers forwarded calls, never ships a row.
    let seen = Seen::default();
    let there = tempfile::tempdir().unwrap();
    let rt_b = Runtime::new(figment::Figment::new(), there.path().to_path_buf());
    tokio::spawn(agent::rpc_forward::serve(rt_b, b, target(seen.clone())));

    // The caller: a live agent store, the replica actor holding the link,
    // and the sessions plugin (the front door forwards only with it).
    agent::spawn_test_agents(rt.clone(), |_: &str| true)
        .await
        .unwrap();
    Box::new(replica::ReplicaPlugin).start(&rt).await.unwrap();
    replica::actor()
        .unwrap()
        .ask(replica::RegisterTransport(a))
        .await
        .unwrap();
    Box::new(sessionsync::SessionSyncPlugin)
        .start(&rt)
        .await
        .unwrap();

    let (peer, _guard) = rpc_core::RpcPeer::open();
    let mut api: jsonrpsee::Methods = rpc_module(rpc_compose::new_server(rt)).into();
    api.extensions_mut().insert(peer);

    let open: SessionOpen = api
        .call(
            "create_session",
            (CreateSessionOptions {
                engine: Some("b".into()),
                ..Default::default()
            },),
        )
        .await
        .unwrap();
    let id = open.session.to_string();
    // Not after 1 s: replication taking a few seconds is normal and logs nothing.
    let warn = format!("sessions: new row {id} from b not here after 6s; forwarding by id");
    let logged = || String::from_utf8(logs.0.lock().clone()).unwrap();
    assert!(!logged().contains(&warn), "{}", logged());

    // The row is still not here: the first message and the chat route's reads
    // go to the target, none fails as "no such session".
    let sent: String = api
        .call(
            "send_input",
            (
                Some(open.session),
                "hello",
                Vec::<String>::new(),
                None::<()>,
                None::<()>,
            ),
        )
        .await
        .unwrap();
    assert_eq!(sent, id);
    let _: agent::Loaded = api
        .call(
            "load_session",
            (open.session, None::<i64>, Some(60), None::<bool>),
        )
        .await
        .unwrap();
    let _: Vec<agent::AgentInfo> = api.call("list_agents", (open.session,)).await.unwrap();
    let _: agent::todo::TodoSnapshot = api.call("get_todos", (open.session,)).await.unwrap();
    // The Tools switches read the owner's hides, not this engine's (empty) set.
    let mcp: Vec<tools::McpServer> = api.call("list_mcp", (Some(open.session),)).await.unwrap();
    assert!(mcp[0].tools[0].hidden, "{mcp:?}");

    let methods: Vec<(String, String)> = seen.lock().clone();
    assert_eq!(
        methods,
        [
            "create_session",
            "send_input",
            "load_session",
            "list_agents",
            "get_todos",
            "list_mcp"
        ]
        .map(|m| (m.to_owned(), id.clone()))
        .to_vec()
    );

    // The cell strip's read and a per-session event subscriber (TUI/CLI)
    // attach to the chat before its row is here; the read goes to b.
    let state: serde_json::Value = api
        .call(
            "cell_state",
            (
                serde_json::json!({ "type": "session", "id": id }),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(state["cells"], serde_json::json!([]));
    assert_eq!(
        seen.lock().last().cloned(),
        Some(("cell_state".to_owned(), id.clone()))
    );
    let subscribe = |s: SessionId| {
        api.subscribe_unbounded(
            "subscribe_events",
            jsonrpsee::rpc_params![s, serde_json::Value::Null],
        )
    };
    let _sub = subscribe(open.session).await.unwrap();
    assert!(
        subscribe(SessionId::new()).await.is_err(),
        "an unknown id is refused"
    );

    // An id nobody created for us is still this engine's "no such session".
    let e = api
        .call::<_, agent::Loaded>(
            "load_session",
            (SessionId::new(), None::<i64>, Some(60), None::<bool>),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no such session"), "{e}");
    let asked = || seen.lock().len();
    let before = asked();

    // A handler that never forwards keeps the strict check: it fails closed
    // for the chat whose row is not here, instead of running locally.
    let e = api
        .call::<_, Vec<agent::QueuedInput>>("list_queued", (open.session,))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no such session"), "{e}");

    // A call that arrived forwarded is never sent on again, mapped or not.
    let mut forwarded = api.clone();
    forwarded
        .extensions_mut()
        .insert(agent::rpc_forward::Forwarded(PeerId("b".into())));
    let e = forwarded
        .call::<_, agent::Loaded>(
            "load_session",
            (open.session, None::<i64>, Some(60), None::<bool>),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no such session"), "{e}");

    // A private tree belongs to this connection and never replicates: it is
    // refused before anything reaches the other engine.
    let e = api
        .call::<_, SessionOpen>(
            "create_session",
            (CreateSessionOptions {
                engine: Some("b".into()),
                retention: tools::Retention::Private,
                ..Default::default()
            },),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("private chat"), "{e}");
    assert_eq!(asked(), before, "nothing reached b");

    // Still no row 6 s after the create: that is the WARN. Real time: the lag
    // is measured by a task the create spawned, nothing here can advance it.
    for _ in 0..100 {
        if logged().contains(&warn) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        logged().contains("WARN") && logged().contains(&warn),
        "{}",
        logged()
    );

    // The row lands (owned by b): the chat route's reads still go to b while
    // the chat is in use, so a replica that has not caught up never answers;
    // the strict check now admits it locally too.
    let other = SessionId::new();
    let theirs = agent::store::Store::open(
        &dir.path().join("sessions.sqlite"),
        Some(mesh::EngineId("b".into())),
    )
    .await
    .unwrap();
    theirs
        .create_session(open.session, "m", None, None, None)
        .await
        .unwrap();
    // A chat expected from b whose row names another owner (it moved): its
    // row decides, nothing goes to b.
    let moved = agent::store::Store::open(
        &dir.path().join("sessions.sqlite"),
        Some(mesh::EngineId("c".into())),
    )
    .await
    .unwrap();
    moved
        .create_session(other, "m", None, None, None)
        .await
        .unwrap();
    sessionsync::owner::expect(other, PeerId("b".into()));
    let _: agent::Loaded = api
        .call(
            "load_session",
            (open.session, None::<i64>, Some(60), None::<bool>),
        )
        .await
        .unwrap();
    let _: Vec<agent::QueuedInput> = api.call("list_queued", (open.session,)).await.unwrap();
    let _: agent::Loaded = api
        .call("load_session", (other, None::<i64>, Some(60), None::<bool>))
        .await
        .unwrap();
    // A landed row owned by b: list_mcp still asks b. One owned by c, which is
    // not connected: `-32010`, never this engine's registry as if it were c's.
    let mcp: Vec<tools::McpServer> = api.call("list_mcp", (Some(open.session),)).await.unwrap();
    assert!(mcp[0].tools[0].hidden, "{mcp:?}");
    let e = api
        .call::<_, Vec<tools::McpServer>>("list_mcp", (Some(other),))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("unreachable"), "{e}");
    assert_eq!(
        seen.lock()[before..].to_vec(),
        vec![
            ("load_session".to_owned(), id.clone()),
            ("list_mcp".to_owned(), id.clone())
        ],
        "after landing: b's chat reads from b, the moved one locally"
    );
    theirs.close().await;
    moved.close().await;
}

fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        rpc_compose::rpc_module(server.clone()),
        a2a::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        sessionsync::rpc_module(server.clone()),
        tasks::rpc_module(server.clone()),
        artifact::rpc_module(server.clone()),
        link::rpc_module(server.clone()),
        talk::rpc_module(server.clone()),
        search_providers::rpc_module(server.clone()),
        app::rpc_module(server.clone()),
        rpc::rpc_module(server.clone()),
        mcp_server::rpc_module(server.clone()),
        cells::rpc_module(server),
    ])
}
