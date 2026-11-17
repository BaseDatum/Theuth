//! 199 D17 across the front door: the holder's file notice and its status query go through
//! `forward::route` to whichever machine owns the chat now. The chat moves from `b` to `c`
//! between the first notice and the status query (a chat moved mid-download): the status and
//! the re-sent notice reach `c`, never `b` again; with `c` gone the outbox gets the
//! unreachable error it retries on. `b` and `c` are stand-in owners that record what they
//! are asked. Its own binary: it registers the agent, replica and sessions actors, which
//! the process-wide kameo registry allows once.

use std::collections::HashMap;
use std::sync::Arc;

use agent::communication::files::notices::{FrontDoor, NoticeDoor};
use agent::communication::files::{NoticeAdmission, NoticeStatus};
use jsonrpsee::core::RpcResult;
use parking_lot::Mutex;
use replica::{ALPN_RPC, ByteStream, PeerEvent, PeerId, Transport};
use runtime::{Plugin as _, Runtime, SessionId};
use tokio::sync::{broadcast, mpsc};

type Inbound = (PeerId, Box<dyn ByteStream>);

/// One node of an in-memory switch that carries only `theuth/rpc/1`; `peers` are the nodes
/// it is connected to now.
struct Node {
    me: PeerId,
    peers: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>,
    rpc_in: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
    events: broadcast::Sender<PeerEvent>,
}

fn nodes(names: &[&str]) -> Vec<Arc<Node>> {
    let (txs, rxs): (Vec<_>, Vec<_>) = names.iter().map(|_| mpsc::unbounded_channel()).unzip();
    rxs.into_iter()
        .enumerate()
        .map(|(i, rx)| {
            let peers = names
                .iter()
                .zip(&txs)
                .filter(|(n, _)| **n != names[i])
                .map(|(n, tx)| (PeerId((*n).into()), tx.clone()))
                .collect();
            Arc::new(Node {
                me: PeerId(names[i].into()),
                peers: Mutex::new(peers),
                rpc_in: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        })
        .collect()
}

#[async_trait::async_trait]
impl Transport for Node {
    fn peers(&self) -> Vec<PeerId> {
        self.peers.lock().keys().cloned().collect()
    }

    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::ensure!(alpn == ALPN_RPC, "refused");
        let tx = self.peers.lock().get(peer).cloned();
        let tx = tx.ok_or_else(|| anyhow::anyhow!("not connected"))?;
        let (mine, theirs) = tokio::io::duplex(1 << 16);
        tx.send((self.me.clone(), Box::new(theirs)))
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

/// What a stand-in owner was asked: `(method, session, notice id)`.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

/// A stand-in owner: admits every notice; its status query answers `status`.
fn owner(seen: Seen, status: NoticeStatus) -> jsonrpsee::Methods {
    let mut m = jsonrpsee::RpcModule::new((seen, status));
    m.register_method(
        "contact_file_notice",
        |p, (seen, _), _| -> RpcResult<NoticeAdmission> {
            let v: Vec<serde_json::Value> = p.parse()?;
            seen.lock().push((
                "contact_file_notice".into(),
                v[0].as_str().unwrap_or_default().into(),
                v[1].as_str().unwrap_or_default().into(),
            ));
            Ok(NoticeAdmission::Accepted)
        },
    )
    .unwrap();
    m.register_method(
        "contact_file_notice_status",
        |p, (seen, status), _| -> RpcResult<NoticeStatus> {
            let v: Vec<serde_json::Value> = p.parse()?;
            seen.lock().push((
                "contact_file_notice_status".into(),
                v[0].as_str().unwrap_or_default().into(),
                v[1].as_str().unwrap_or_default().into(),
            ));
            Ok(*status)
        },
    )
    .unwrap();
    m.into()
}

#[tokio::test]
async fn a_file_notice_and_its_status_follow_the_chat_to_its_current_owner() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let [a, b, c] = <[_; 3]>::try_from(nodes(&["a", "b", "c"])).ok().unwrap();
    let (seen_b, seen_c) = (Seen::default(), Seen::default());
    for (node, seen, status) in [
        (b, seen_b.clone(), NoticeStatus::Unknown),
        (c, seen_c.clone(), NoticeStatus::Committed),
    ] {
        let there = tempfile::tempdir().unwrap().keep();
        let rt_x = Runtime::new(figment::Figment::new(), there);
        tokio::spawn(agent::rpc_forward::serve(rt_x, node, owner(seen, status)));
    }

    // The holder: a live agent store, the replica actor holding the switch, the sessions
    // plugin (the front door forwards only with it) and the production forwarding hooks.
    agent::spawn_test_agents(rt.clone(), |_: &str| true)
        .await
        .unwrap();
    Box::new(replica::ReplicaPlugin).start(&rt).await.unwrap();
    replica::actor()
        .unwrap()
        .ask(replica::RegisterTransport(a.clone()))
        .await
        .unwrap();
    Box::new(sessionsync::SessionSyncPlugin)
        .start(&rt)
        .await
        .unwrap();
    let _server = rpc_compose::new_server(rt.clone());

    // The chat, as its replicated row reads here: owned by b.
    let path = dir.path().join("sessions.sqlite");
    let chat = SessionId::new();
    agent::store::Store::open(&path, Some(mesh::EngineId("b".into())))
        .await
        .unwrap()
        .create_session(chat, "m", None, None, None)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let text = "<message_file from=\"peer:maya\" state=\"received\"/>\nFile notice from theuth: …";
    let (s, n) = (chat.to_string(), id.to_string());
    let asked = |seen: &Seen| seen.lock().clone();

    let door = FrontDoor;
    assert_eq!(
        door.notice(chat, id, "rec", text).await,
        Ok(NoticeAdmission::Accepted)
    );
    assert_eq!(door.status(chat, id).await, Ok(NoticeStatus::Unknown));
    assert_eq!(
        asked(&seen_b),
        [
            ("contact_file_notice".into(), s.clone(), n.clone()),
            ("contact_file_notice_status".into(), s.clone(), n.clone())
        ]
    );

    // The chat moves to c (its take row lands here): the status query and the re-sent
    // notice go to c; b hears nothing more.
    agent::store::Store::open(&path, Some(mesh::EngineId("c".into())))
        .await
        .unwrap()
        .set_owner(chat, "c", 2)
        .await
        .unwrap();
    assert_eq!(door.status(chat, id).await, Ok(NoticeStatus::Committed));
    assert_eq!(
        door.notice(chat, id, "rec", text).await,
        Ok(NoticeAdmission::Accepted)
    );
    assert_eq!(asked(&seen_b).len(), 2, "b is never asked again");
    assert_eq!(
        asked(&seen_c),
        [
            ("contact_file_notice_status".into(), s.clone(), n.clone()),
            ("contact_file_notice".into(), s, n)
        ]
    );

    // c goes offline: the outbox gets the unreachable error (and retries on its backoff).
    a.peers.lock().remove(&PeerId("c".into()));
    let e = door.notice(chat, id, "rec", text).await.unwrap_err();
    assert!(e.contains("(-32010)"), "{e}");
}
