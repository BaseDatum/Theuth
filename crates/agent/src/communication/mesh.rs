//! Peer messages follow the owner (69 P). A `Deliver` whose recipient row
//! says `owner == another engine` is forwarded to that engine as one
//! JSON-RPC request line on a `theuth/rpc/1` stream — the same framing 68 F
//! uses for session RPCs — calling [`METHOD`]`(session, envelope)`; the
//! owner's handler runs its own supervisor intake and answers with its
//! receipt. Receipts stay engine-local (68 decision 8): the sender records
//! `accepted` on an ok answer, `failed` otherwise, with the F error shape
//! (`{"OwnerUnreachable": {owner, last_seen}}`) when the owner is out of
//! reach so the model reads the same words the desktop shows.
//!
//! The agent crate has no jsonrpsee: the line is written and read with
//! `serde_json` only (`{"jsonrpc":"2.0","id":1,"method":…,"params":[…]}`
//! → `{"result":…}` | `{"error":{code,message,data}}`).

use std::time::Duration;

use replica::{ALPN_RPC, PeerId, Transport};
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::{PeerEnvelope, PeerReceipt};

/// The RPC the owner serves (`crates/rpc/src/api.rs`, beside `send_input`).
pub const METHOD: &str = "peer_deliver";

/// JSON-RPC error code the front door uses for an unreachable owner (68 F);
/// its `data` is already the receipt's error shape.
pub const OWNER_UNREACHABLE: i32 = -32010;

/// How long the sender waits for the owner's receipt (as 68 F's `TIMEOUT`).
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Longest response line accepted.
const MAX_LINE: usize = 1024 * 1024;

/// Why a forward yielded no `accepted` receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No usable answer: not connected, transport failure, timeout, malformed
    /// reply, or the owner itself said `-32010` (it moved the session on).
    Unreachable {
        /// The owner engine id the row named.
        owner: String,
        /// The failure, for the log; the receipt carries the F shape instead.
        why: String,
    },
    /// The owner's intake refused it (its policy, size, quota, unknown
    /// `reply_to`, …): its message, which the receipt carries verbatim.
    Refused(String),
}

impl Failure {
    /// The receipt's `error` text. Unreachable → F's error data in serde's
    /// external enum form, `last_seen` from the mesh (ms since epoch or
    /// `null`). Refused → `denied: <why>` for a policy refusal on the
    /// owner's engine (design 50: both engines must allow), else the
    /// owner's message as it was.
    pub async fn receipt_error(&self) -> String {
        match self {
            Self::Unreachable { owner, .. } => {
                serde_json::to_string(&FrontDoorError::OwnerUnreachable {
                    last_seen: last_seen(owner).await,
                    owner: owner.clone(),
                })
                .expect("plain struct")
            }
            Self::Refused(why) if why.contains(tools::communication::DISABLED) => {
                format!("denied: {why}")
            }
            Self::Refused(why) => why.clone(),
        }
    }
}

/// 68 F's `rpc::forward::FrontDoorError`, byte for byte (external enum
/// form, `owner` then `last_seen`): the one shape the desktop, the TUI and
/// now a `failed` receipt carry for an unreachable owner.
#[derive(Serialize)]
enum FrontDoorError {
    OwnerUnreachable {
        owner: String,
        last_seen: Option<u64>,
    },
}

/// One forwarded peer message: open `theuth/rpc/1` to `owner`, write the
/// request line, read the response line, within [`TIMEOUT`].
pub async fn forward(
    transport: &dyn Transport,
    owner: &PeerId,
    session: SessionId,
    envelope: &PeerEnvelope,
) -> Result<PeerReceipt, Failure> {
    let unreachable = |why: String| Failure::Unreachable {
        owner: owner.0.clone(),
        why,
    };
    if !transport.peers().contains(owner) {
        return Err(unreachable("not connected".into()));
    }
    let exchange = async {
        let stream = transport
            .open(owner, ALPN_RPC)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let (read, mut write) = tokio::io::split(stream);
        let mut line = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": METHOD, "params": [session, envelope]
        })
        .to_string();
        line.push('\n');
        write
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        write.flush().await.map_err(|e| e.to_string())?;
        let mut reply = String::new();
        BufReader::with_capacity(16 * 1024, read)
            .take(MAX_LINE as u64)
            .read_line(&mut reply)
            .await
            .map_err(|e| e.to_string())?;
        if reply.is_empty() {
            return Err("owner closed the stream without answering".to_owned());
        }
        Ok(reply)
    };
    let reply = match tokio::time::timeout(TIMEOUT, exchange).await {
        Ok(Ok(r)) => r,
        Ok(Err(why)) => return Err(unreachable(why)),
        Err(_) => {
            return Err(unreachable(format!(
                "no answer within {}s",
                TIMEOUT.as_secs()
            )));
        }
    };
    let rp: Response = serde_json::from_str(reply.trim_end())
        .map_err(|e| unreachable(format!("malformed reply: {e}")))?;
    match (rp.result, rp.error) {
        (Some(receipt), _) => Ok(receipt),
        (None, Some(e)) if e.code == OWNER_UNREACHABLE => Err(unreachable(e.message)),
        (None, Some(e)) => Err(Failure::Refused(e.message)),
        (None, None) => Err(unreachable(
            "malformed reply: neither result nor error".into(),
        )),
    }
}

/// The response line, either half.
#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    result: Option<PeerReceipt>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// A JSON-RPC error object (only what the receipt needs).
#[derive(Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// The mesh's last sighting of `owner` in ms since epoch; `None` when the
/// mesh is not running or never saw it (as 68 F's `unreachable`).
pub async fn last_seen(owner: &str) -> Option<u64> {
    let m = kameo::actor::ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR)
        .ok()
        .flatten()?;
    m.ask(mesh::Status)
        .await
        .ok()
        .and_then(|s| {
            s.peers
                .into_iter()
                .find(|p| p.id.0 == owner)
                .map(|p| p.last_seen_ms)
        })
        .filter(|&ms| ms > 0)
}

/// Engine display names for `discover_peers` (69 P): `(this engine's name,
/// id → name for every paired engine)` from the mesh; without a mesh actor
/// this engine is its hostname and the map is empty (a caller shortens the
/// id). Asked once per listing, never per row.
pub async fn engine_names() -> (String, std::collections::HashMap<String, String>) {
    let status = match kameo::actor::ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR) {
        Ok(Some(m)) => m.ask(mesh::Status).await.ok(),
        _ => None,
    };
    match status {
        Some(s) => (
            s.self_name,
            s.peers.into_iter().map(|p| (p.id.0, p.name)).collect(),
        ),
        None => (platform::process::hostname().to_owned(), Default::default()),
    }
}

/// `owner` as a display name: the mesh's, else its first eight characters.
pub fn engine_name(names: &std::collections::HashMap<String, String>, owner: &str) -> String {
    names
        .get(owner)
        .cloned()
        .unwrap_or_else(|| owner.get(..8).unwrap_or(owner).to_owned())
}

/// 183 R1 (H2): an engine as text a model or a card reads — its mesh name,
/// else "another machine"; never a hex id.
pub fn machine_name(names: &std::collections::HashMap<String, String>, id: &str) -> String {
    names
        .get(id)
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| "another machine".to_owned())
}

/// [`machine_name`] with the mesh asked now (one `Status`).
pub async fn machine_name_now(id: &str) -> String {
    machine_name(&engine_names().await.1, id)
}

/// A two-node loopback switch for tests (as `replica/tests/sync_loopback.rs`
/// and `rpc::forward::tests`): every node sees every other as connected; a
/// stream opened to a peer lands in its `accept`.
#[cfg(test)]
pub(crate) mod loopback {
    use std::collections::HashMap;
    use std::sync::Arc;

    use replica::{ByteStream, PeerEvent, PeerId, Transport};
    use tokio::sync::{broadcast, mpsc};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    pub(crate) struct Loop {
        pub(crate) me: PeerId,
        inboxes: Arc<parking_lot::Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    /// One switch with a node per name.
    pub(crate) fn nodes(names: &[&str]) -> Vec<Arc<Loop>> {
        let inboxes = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        names
            .iter()
            .map(|name| {
                let me = PeerId((*name).to_owned());
                let (tx, rx) = mpsc::unbounded_channel();
                inboxes.lock().insert(me.clone(), tx);
                Arc::new(Loop {
                    me,
                    inboxes: inboxes.clone(),
                    inbox: tokio::sync::Mutex::new(rx),
                    events: broadcast::channel(8).0,
                })
            })
            .collect()
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
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
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::loopback::nodes;
    use super::*;
    use crate::communication::PeerMessageState;
    use uuid::Uuid;

    fn envelope() -> PeerEnvelope {
        PeerEnvelope {
            id: Uuid::now_v7(),
            from: SessionId::new(),
            text: "hi".into(),
            reply_to: None,
            request_id: None,
            chain: 0,
        }
    }

    /// One owner answering each stream with `answer` (a JSON-RPC response
    /// line), recording the request line.
    fn fake_owner(
        t: std::sync::Arc<super::loopback::Loop>,
        answer: String,
    ) -> tokio::sync::mpsc::UnboundedReceiver<String> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((_, stream)) = t.accept(ALPN_RPC).await {
                let (read, mut write) = tokio::io::split(stream);
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let _ = tx.send(line);
                write
                    .write_all(format!("{answer}\n").as_bytes())
                    .await
                    .unwrap();
                write.flush().await.unwrap();
            }
        });
        rx
    }

    #[tokio::test]
    async fn the_request_line_is_jsonrpc_and_the_owners_receipt_comes_back() {
        let n = nodes(&["a", "b"]);
        let (a, b) = (n[0].clone(), n[1].clone());
        let env = envelope();
        let session = SessionId::new();
        let receipt = PeerReceipt {
            id: env.id,
            to: format!("session:{session}"),
            state: PeerMessageState::Accepted,
            reply_to: None,
            reply: None,
            task_id: None,
            error: None,
            answered_by: None,
        };
        let answer = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": receipt}).to_string();
        let mut seen = fake_owner(b.clone(), answer);
        let got = forward(a.as_ref(), &b.me, session, &env).await.unwrap();
        assert_eq!(got, receipt);
        let line: serde_json::Value = serde_json::from_str(&seen.recv().await.unwrap()).unwrap();
        assert_eq!(line["method"], METHOD);
        assert_eq!(line["params"][0], serde_json::to_value(session).unwrap());
        assert_eq!(line["params"][1], serde_json::to_value(&env).unwrap());
    }

    #[tokio::test]
    async fn owner_errors_become_refused_or_unreachable() {
        let n = nodes(&["a", "b"]);
        let (a, b) = (n[0].clone(), n[1].clone());
        let denied = serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {
            "code": -32000, "message": format!("send_message: {}: off", tools::communication::DISABLED)
        }})
        .to_string();
        let _seen = fake_owner(b.clone(), denied);
        let e = forward(a.as_ref(), &b.me, SessionId::new(), &envelope())
            .await
            .unwrap_err();
        assert!(matches!(e, Failure::Refused(_)));
        assert!(e.receipt_error().await.starts_with("denied: send_message"));

        // A `-32010` from the owner (it no longer holds the session) is unreachable.
        let n = nodes(&["c", "d"]);
        let (c, d) = (n[0].clone(), n[1].clone());
        let moved = serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {
            "code": OWNER_UNREACHABLE, "message": "owner d unreachable"
        }})
        .to_string();
        let _seen = fake_owner(d.clone(), moved);
        let e = forward(c.as_ref(), &d.me, SessionId::new(), &envelope())
            .await
            .unwrap_err();
        assert!(matches!(e, Failure::Unreachable { .. }));
        assert_eq!(
            e.receipt_error().await,
            r#"{"OwnerUnreachable":{"owner":"d","last_seen":null}}"#,
            "F's error data shape, no mesh → last_seen null"
        );
    }

    #[tokio::test]
    async fn a_peer_that_is_not_connected_is_unreachable_without_a_stream() {
        let n = nodes(&["a"]);
        let e = forward(
            n[0].as_ref(),
            &PeerId("ghost".into()),
            SessionId::new(),
            &envelope(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            e,
            Failure::Unreachable {
                owner: "ghost".into(),
                why: "not connected".into()
            }
        );
    }
}
