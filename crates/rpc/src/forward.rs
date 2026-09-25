//! The front door of a session written elsewhere (68 §5, ruling 5): every
//! session-mutating RPC first asks [`route`]. The owner is here → the call
//! proceeds. Not here and a **user session** (`sessions.agent` empty) → the
//! session is taken here (`TakeSession{reason: Write}`), then the call
//! proceeds. Not here and an **agent desk** (`sessions.agent` set) → the call
//! is forwarded verbatim to the owner as JSON-RPC over `theuth/rpc/1`: one
//! request line, one response line, one QUIC stream, 30 s. An owner that
//! cannot be reached or taken from answers [`OWNER_UNREACHABLE`] with an
//! [`OwnerUnreachable`] payload; the desktop renders the "continue here?" card.
//!
//! The owner side ([`serve`]) dispatches each forwarded request into its own
//! [`crate::api::Server`] as the internal peer, with a [`Forwarded`] marker in
//! the extensions so the request is never forwarded a second hop: if the owner
//! moved meanwhile the store's `NotOwner` guard answers, not a relay loop.

use std::sync::Arc;
use std::time::Duration;

use agent::Agents;
use agent::store::Store;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::{ErrorObjectOwned, Id, Request, Response, ResponsePayload};
use replica::{ALPN_RPC, PeerId, Transport};
use runtime::{Runtime, SessionId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// JSON-RPC error code: the session's owner cannot be reached (or taken from).
/// `data` is a [`FrontDoorError`] in serde's external enum form:
/// `{"OwnerUnreachable": {"owner": "<engine id>", "last_seen": <ms epoch | null>}}`
/// — the shape U's desktop card and TUI parse.
pub const OWNER_UNREACHABLE: i32 = -32010;

/// How long a forwarded call waits for the owner's answer.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Longest request or response line accepted on `theuth/rpc/1`.
const MAX_LINE: usize = 64 * 1024 * 1024;

/// `data` of a front-door error (external tagging: the variant name wraps its fields).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum FrontDoorError {
    /// The owner engine did not answer (not connected, timed out, refused the take).
    OwnerUnreachable {
        /// Owner engine (mesh `EngineId` as text).
        owner: String,
        /// Last time this engine heard from the owner (ms since epoch); `null` = never.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        last_seen: Option<u64>,
    },
}

/// Where a session-mutating call goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The owner is this engine (or the session is unknown here): proceed locally.
    Here,
    /// A user session owned elsewhere: take it here, then proceed locally.
    Take,
    /// An agent desk owned elsewhere: forward to this engine.
    Forward(PeerId),
}

/// Extension marker on a request the owner received over `theuth/rpc/1`.
#[derive(Clone, Copy, Debug)]
pub struct Forwarded;

/// The mesh peer owning `session`, when it is another engine and the
/// sessions plugin is running: the first peer to ask for its files.
pub async fn owner_peer(session: SessionId) -> Option<PeerId> {
    sessionsync::actor()?;
    let store = crate::api::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await.ok()?;
    let (owner, _) = store.owner(session).await.ok().flatten()?;
    (!owner.is_empty() && owner != store.engine()).then_some(PeerId(owner))
}

/// Classify `session` from its row: `Here` when unknown, unowned or ours;
/// `Forward(owner)` for an agent desk — an external harness (`agent` set) or
/// a roster desk: a pinned root the roster titled `agent:<name>`
/// (`roster::AgentName::peer`; the roster keeps its session → agent map in
/// memory on the hub, so the title is the mark every replica can see);
/// `Take` for everything else (a user session).
pub async fn classify(store: &Store, session: SessionId) -> anyhow::Result<Route> {
    let Some(row) = store.session(session).await? else {
        return Ok(Route::Here);
    };
    if row.owner.is_empty() || row.owner == store.engine() {
        return Ok(Route::Here);
    }
    // ponytail: title match; a renamed desk falls back to take-on-write. A
    // durable desk mark on the row (or a replicated roster map) replaces it.
    let desk = row.agent.is_some() || (row.pinned && row.parent.is_none() && row.title.starts_with("agent:"));
    Ok(if desk { Route::Forward(PeerId(row.owner)) } else { Route::Take })
}

/// The front door for one call. `Ok(None)`: proceed locally (the owner is
/// here, or the session was just taken). `Ok(Some(reply))`: the owner
/// answered the forwarded call. `Err`: the owner's error verbatim, or
/// [`OWNER_UNREACHABLE`]. `params` is the method's positional parameter
/// list in trait order, as the generated client sends it.
pub async fn route<T: DeserializeOwned>(
    ext: &Extensions,
    session: SessionId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    // Lone engine (no sessions plugin) or already forwarded once: local.
    if ext.get::<Forwarded>().is_some() {
        return Ok(None);
    }
    let Some(sessions) = sessionsync::actor() else {
        return Ok(None);
    };
    let store = crate::api::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    let owner = match classify(&store, session).await.map_err(|e| internal(format!("{e:#}")))? {
        Route::Here => return Ok(None),
        Route::Take => {
            let taken = sessions
                .ask(sessionsync::TakeSession {
                    id: session,
                    reason: sessionsync::TakeReason::Write,
                })
                .await;
            return match taken {
                Ok(_) => Ok(None),
                Err(kameo::error::SendError::HandlerError(why)) => {
                    let (owner, _) = store.owner(session).await.ok().flatten().unwrap_or_default();
                    Err(unreachable(owner, &why).await)
                }
                Err(e) => Err(internal(e.to_string())),
            };
        }
        Route::Forward(owner) => owner,
    };
    let transport = match replica::actor() {
        Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
        None => None,
    };
    let Some(transport) = transport.filter(|t| t.peers().contains(&owner)) else {
        return Err(unreachable(owner.0, "not connected").await);
    };
    let params = serde_json::value::to_raw_value(&params).map_err(|e| internal(e.to_string()))?;
    match forward(transport.as_ref(), &owner, method, &params).await {
        Ok(raw) => serde_json::from_str(raw.get()).map(Some).map_err(|e| internal(e.to_string())),
        Err(Err(e)) => Err(e),
        Err(Ok(why)) => Err(unreachable(owner.0, &why).await),
    }
}

/// One forwarded call: open `theuth/rpc/1` to `owner`, write the request
/// line, read the response line. `Err(Err(e))`: the owner answered with a
/// JSON-RPC error (passed through). `Err(Ok(why))`: no usable answer within
/// [`TIMEOUT`] (transport failure, timeout, malformed reply).
pub async fn forward(
    transport: &dyn Transport,
    owner: &PeerId,
    method: &str,
    params: &RawValue,
) -> Result<Box<RawValue>, Result<String, ErrorObjectOwned>> {
    let exchange = async {
        let stream = transport.open(owner, ALPN_RPC).await.map_err(|e| format!("{e:#}"))?;
        let (read, mut write) = tokio::io::split(stream);
        let req = Request::borrowed(method, Some(params), Id::Number(1));
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        write.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        write.flush().await.map_err(|e| e.to_string())?;
        let mut reply = String::new();
        BufReader::with_capacity(64 * 1024, read)
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
        Ok(Err(why)) => return Err(Ok(why)),
        Err(_) => return Err(Ok(format!("no answer within {}s", TIMEOUT.as_secs()))),
    };
    let rp: Response<&RawValue> = serde_json::from_str(reply.trim_end()).map_err(|e| Ok(format!("malformed reply: {e}")))?;
    match rp.payload {
        ResponsePayload::Success(v) => Ok(<RawValue as ToOwned>::to_owned(*v)),
        ResponsePayload::Error(e) => Err(Err(e.into_owned())),
    }
}

/// The owner side: accept `theuth/rpc/1` streams until shutdown and answer
/// each with one dispatch of `methods` as the internal peer, marked
/// [`Forwarded`]. `methods` is the full API ([`crate::methods`]).
pub async fn serve(rt: Runtime, transport: Arc<dyn Transport>, mut methods: jsonrpsee::Methods) {
    methods.extensions_mut().insert(link::RpcPeer::internal());
    methods.extensions_mut().insert(Forwarded);
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_RPC) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let methods = methods.clone();
                tokio::spawn(async move {
                    if let Err(e) = answer(stream, &methods).await {
                        tracing::debug!(%peer, error = %e, "rpc mesh stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "rpc mesh accept");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// One accepted stream: request line in, response line out.
async fn answer(stream: Box<dyn replica::ByteStream>, methods: &jsonrpsee::Methods) -> anyhow::Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut line = String::new();
    BufReader::with_capacity(64 * 1024, read).take(MAX_LINE as u64).read_line(&mut line).await?;
    anyhow::ensure!(!line.trim().is_empty(), "empty request");
    let (rp, _) = methods.raw_json_request(line.trim_end(), 1).await?;
    let mut out = rp.get().to_owned();
    out.push('\n');
    write.write_all(out.as_bytes()).await?;
    write.flush().await?;
    Ok(())
}

/// An [`OWNER_UNREACHABLE`] error with the mesh's last sighting of `owner`
/// (`None` when the mesh is not running or never saw it).
async fn unreachable(owner: String, why: &str) -> ErrorObjectOwned {
    let last_seen = match kameo::actor::ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR) {
        Ok(Some(m)) => m
            .ask(mesh::Status)
            .await
            .ok()
            .and_then(|s| s.peers.into_iter().find(|p| p.id.0 == owner).map(|p| p.last_seen_ms))
            .filter(|&ms| ms > 0),
        _ => None,
    };
    let short = owner.get(..8).unwrap_or(&owner);
    ErrorObjectOwned::owned(
        OWNER_UNREACHABLE,
        format!("owner {short} unreachable: {why}"),
        Some(FrontDoorError::OwnerUnreachable { owner, last_seen }),
    )
}

fn internal(msg: String) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(jsonrpsee::types::error::INTERNAL_ERROR_CODE, msg, None::<()>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use replica::{ByteStream, PeerEvent};
    use std::collections::HashMap;
    use tokio::sync::{broadcast, mpsc};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Two-node loopback switch, as `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<parking_lot::Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let inboxes = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("a"), mk("b"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes.lock().keys().filter(|p| **p != self.me).cloned().collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self.inboxes.lock().get(peer).cloned().ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs))).map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    /// A fake owner: one `send_input` method that records what reached it.
    fn fake_owner(seen: mpsc::UnboundedSender<(String, Option<Extensions>)>) -> jsonrpsee::Methods {
        let mut m = jsonrpsee::RpcModule::new(seen);
        m.register_async_method("send_input", |params, seen, ext| async move {
            let (session, text): (Option<String>, String) = params.parse()?;
            let _ = seen.send((text, Some(ext)));
            RpcResult::Ok(session.unwrap_or_default())
        })
        .unwrap();
        m.register_method("rename_session", |_, _, _| -> RpcResult<()> {
            Err(ErrorObjectOwned::owned(-32000, "no such session", None::<()>))
        })
        .unwrap();
        m.into()
    }

    #[tokio::test]
    async fn forwarded_send_input_reaches_the_owner_dispatch_as_internal_forwarded_peer() {
        let (a, b) = pair();
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(rt, b.clone(), fake_owner(seen_tx)));
        let params = serde_json::value::to_raw_value(&(Some("s-1"), "hello")).unwrap();
        let reply = forward(a.as_ref(), &b.me, "send_input", &params).await.unwrap();
        assert_eq!(reply.get(), "\"s-1\"");
        let (text, ext) = seen.recv().await.unwrap();
        assert_eq!(text, "hello");
        let ext = ext.unwrap();
        assert!(ext.get::<Forwarded>().is_some(), "owner dispatch carries the one-hop marker");
        assert!(ext.get::<link::RpcPeer>().is_some(), "owner dispatch runs as the internal peer");
    }

    #[tokio::test]
    async fn owner_errors_pass_through_verbatim() {
        let (a, b) = pair();
        let (seen_tx, _seen) = mpsc::unbounded_channel();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(rt, b.clone(), fake_owner(seen_tx)));
        let params = serde_json::value::to_raw_value(&("s-1", "t")).unwrap();
        let e = forward(a.as_ref(), &b.me, "rename_session", &params).await.unwrap_err().unwrap_err();
        assert_eq!(e.code(), -32000);
        assert_eq!(e.message(), "no such session");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_owner_is_unreachable_after_the_timeout() {
        let (a, b) = pair();
        // b accepts but never answers.
        tokio::spawn(async move {
            let (_peer, _stream) = b.accept(ALPN_RPC).await.unwrap();
            std::future::pending::<()>().await;
        });
        let params = serde_json::value::to_raw_value(&("s-1",)).unwrap();
        let why = forward(a.as_ref(), &PeerId("b".into()), "cancel", &params).await.unwrap_err().unwrap();
        assert!(why.contains("no answer within 30s"), "{why}");
    }

    /// The shape U parses: `data = {"OwnerUnreachable": {"owner", "last_seen"}}`, code -32010.
    #[tokio::test]
    async fn owner_unreachable_error_is_the_external_enum_form() {
        let e = unreachable("engine-a".into(), "not connected").await;
        assert_eq!(e.code(), OWNER_UNREACHABLE);
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "OwnerUnreachable": { "owner": "engine-a", "last_seen": null } }),
            "no mesh in a test: never seen"
        );
        assert!(e.message().starts_with("owner engine-a unreachable"), "{}", e.message());
    }

    #[tokio::test]
    async fn classify_user_vs_desk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (user, desk, roster, mine) = (SessionId::new(), SessionId::new(), SessionId::new(), SessionId::new());
        // Rows written by another engine ("them") ...
        let theirs = Store::open(&path, Some(mesh::EngineId("them".into()))).await.unwrap();
        theirs.create_session(user, "m", None, None, None).await.unwrap();
        theirs.create_session(desk, "m", None, None, None).await.unwrap();
        theirs.set_acp(desk, "omp", None).await.unwrap();
        // ... a roster desk: pinned root titled `agent:<name>` (roster::open_desk) ...
        theirs.create_session(roster, "m", None, None, None).await.unwrap();
        theirs.set_title(roster, "agent:ops").await.unwrap();
        theirs.set_pinned(roster, true).await.unwrap();
        theirs.close().await;
        // ... seen from this engine ("me"), which also has a session of its own.
        let store = Store::open(&path, Some(mesh::EngineId("me".into()))).await.unwrap();
        store.create_session(mine, "m", None, None, None).await.unwrap();
        assert_eq!(classify(&store, mine).await.unwrap(), Route::Here);
        assert_eq!(classify(&store, SessionId::new()).await.unwrap(), Route::Here, "unknown → local error path");
        assert_eq!(classify(&store, user).await.unwrap(), Route::Take);
        assert_eq!(classify(&store, desk).await.unwrap(), Route::Forward(PeerId("them".into())));
        assert_eq!(classify(&store, roster).await.unwrap(), Route::Forward(PeerId("them".into())));
        store.close().await;
    }
}
