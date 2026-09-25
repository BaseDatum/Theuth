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
//!
//! Attachments (68 follow-up A1): a forwarded `files` list carries **names**,
//! not the forwarder's absolute paths — each `<data_dir>/attachments/<sha16>.<ext>`
//! is sent as the relative marker `attachments/<sha16>.<ext>` ([`ByName`]);
//! the owner resolves every marker with `artifact::mesh::fetch_attachment`,
//! asking the forwarder first ([`resolve_files`]), before the actor sees it.

use std::path::{Path, PathBuf};
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

/// Extension marker on a request the owner received over `theuth/rpc/1`:
/// the engine that forwarded it (a connected peer, and the one holding any
/// attachment the request names).
#[derive(Clone, Debug)]
pub struct Forwarded(pub PeerId);

/// `files` as a forwarded call sends them: every attachment-store path
/// (`…/attachments/<sha16>.<ext>`) becomes the relative marker
/// `attachments/<sha16>.<ext>`, anything else travels verbatim. Serialized
/// only when the call is actually forwarded ([`route`]); nothing on the
/// local path.
pub struct ByName<'a>(pub &'a [PathBuf]);

impl Serialize for ByName<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(|f| match artifact::mesh::attachment_name(f) {
            Some(name) => Path::new("attachments").join(name),
            None => f.clone(),
        }))
    }
}

/// The marker's name: `attachments/<name>` → `<name>`.
fn marker(f: &Path) -> Option<&str> {
    f.strip_prefix("attachments").ok().and_then(Path::to_str)
}

/// The owner side of [`ByName`]: each `attachments/<name>` marker in `files`
/// becomes this engine's `<data_dir>/attachments/<name>`, fetched from the
/// forwarder (first) or any peer when not already here. Other entries pass.
pub async fn resolve_files(ext: &Extensions, data_dir: &Path, files: Vec<PathBuf>) -> RpcResult<Vec<PathBuf>> {
    if !files.iter().any(|f| marker(f).is_some()) {
        return Ok(files);
    }
    let transport = match replica::actor() {
        Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
        None => None,
    };
    let Some(t) = transport else {
        return Err(crate::api::failed("attachments: unavailable: no peer has it"));
    };
    resolve_files_via(ext, t.as_ref(), data_dir, files).await
}

/// [`resolve_files`] over a given transport.
pub async fn resolve_files_via(
    ext: &Extensions,
    t: &dyn Transport,
    data_dir: &Path,
    mut files: Vec<PathBuf>,
) -> RpcResult<Vec<PathBuf>> {
    let prefer = ext.get::<Forwarded>().map(|f| &f.0);
    let dir = artifact::mesh::attachments_dir(data_dir);
    for f in &mut files {
        let Some(name) = marker(f) else { continue };
        *f = artifact::mesh::fetch_attachment_via(t, &dir, name, prefer)
            .await
            .map_err(|e| crate::api::failed(format!("{}: {e:#}", f.display())))?;
    }
    Ok(files)
}

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
    match to_peer(ext, &owner, method, params).await? {
        Some(v) => Ok(Some(v)),
        None => Err(unreachable(owner.0, "not connected").await),
    }
}

/// One call to `peer` verbatim over `theuth/rpc/1`, never a second hop. `Ok(None)`: it cannot be
/// sent from here (this request was itself forwarded, no mesh transport, `peer` not connected) —
/// the caller keeps its local answer. `Err`: the peer's error verbatim, or [`OWNER_UNREACHABLE`]
/// when it gave no usable answer within [`TIMEOUT`]. 69: the document hub-only owner calls
/// (`doc_members`, links, `doc_create`, …) ride this to the lease holder.
pub async fn to_peer<T: DeserializeOwned>(
    ext: &Extensions,
    peer: &PeerId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    if ext.get::<Forwarded>().is_some() {
        return Ok(None);
    }
    let transport = match replica::actor() {
        Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
        None => None,
    };
    let Some(transport) = transport.filter(|t| t.peers().contains(peer)) else {
        return Ok(None);
    };
    let params = serde_json::value::to_raw_value(&params).map_err(|e| internal(e.to_string()))?;
    match forward(transport.as_ref(), peer, method, &params).await {
        Ok(raw) => serde_json::from_str(raw.get()).map(Some).map_err(|e| internal(e.to_string())),
        Err(Err(e)) => Err(e),
        Err(Ok(why)) => Err(unreachable(peer.0.clone(), &why).await),
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
/// [`Forwarded`] with the opening peer. `methods` is the full API ([`crate::methods`]).
pub async fn serve(rt: Runtime, transport: Arc<dyn Transport>, mut methods: jsonrpsee::Methods) {
    methods.extensions_mut().insert(link::RpcPeer::internal());
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_RPC) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let mut methods = methods.clone();
                methods.extensions_mut().insert(Forwarded(peer.clone()));
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

    /// What the fake owner records per `send_input`: text, resolved files, extensions.
    type Seen = (String, Vec<PathBuf>, Option<Extensions>);

    /// The fake owner's context: where it reports, and the transport + data
    /// dir it resolves forwarded attachments with.
    struct Owner {
        seen: mpsc::UnboundedSender<Seen>,
        t: Arc<Loop>,
        data_dir: PathBuf,
    }

    /// A fake owner: one `send_input` method that resolves `files` like the
    /// real one and records what reached it.
    fn fake_owner(seen: mpsc::UnboundedSender<Seen>, t: Arc<Loop>, data_dir: PathBuf) -> jsonrpsee::Methods {
        let mut m = jsonrpsee::RpcModule::new(Owner { seen, t, data_dir });
        m.register_async_method("send_input", |params, o, ext| async move {
            let mut seq = params.sequence();
            let session: Option<String> = seq.next()?;
            let text: String = seq.next()?;
            let files: Vec<PathBuf> = seq.optional_next()?.unwrap_or_default();
            let files = resolve_files_via(&ext, o.t.as_ref(), &o.data_dir, files).await?;
            let _ = o.seen.send((text, files, Some(ext)));
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
        tokio::spawn(serve(rt, b.clone(), fake_owner(seen_tx, b.clone(), std::env::temp_dir())));
        let params = serde_json::value::to_raw_value(&(Some("s-1"), "hello")).unwrap();
        let reply = forward(a.as_ref(), &b.me, "send_input", &params).await.unwrap();
        assert_eq!(reply.get(), "\"s-1\"");
        let (text, files, ext) = seen.recv().await.unwrap();
        assert_eq!(text, "hello");
        assert!(files.is_empty());
        let ext = ext.unwrap();
        assert_eq!(ext.get::<Forwarded>().unwrap().0, a.me, "owner dispatch carries the one-hop marker naming the forwarder");
        assert!(ext.get::<link::RpcPeer>().is_some(), "owner dispatch runs as the internal peer");
    }

    /// A1: the forwarder sends `files` by name (`attachments/<sha16>.<ext>`);
    /// the owner fetches each from the forwarder over `theuth/artifacts/1`
    /// into its own `attachments/` before the actor sees the paths.
    #[tokio::test]
    async fn forwarded_send_input_carries_attachments_by_name_and_the_owner_resolves_them() {
        let (a, b) = pair();
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // The forwarder holds two attachments and serves the artifacts ALPN.
        let (png, pdf) = (b"\x89PNG one".to_vec(), b"%PDF two".to_vec());
        let p1 = agent::context::save_blob(&png, "png", a_dir.path()).unwrap();
        let p2 = agent::context::save_blob(&pdf, "pdf", a_dir.path()).unwrap();
        let rt_a = Runtime::new(figment::Figment::new(), a_dir.path().to_path_buf());
        let state = artifact::State::open(a_dir.path(), rt_a.clone(), vec![]).await.unwrap();
        tokio::spawn(artifact::mesh::serve_on(rt_a, a.clone(), Arc::new(artifact::Stores::new(Arc::new(state)))));
        // The owner answers `theuth/rpc/1`.
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let rt_b = Runtime::new(figment::Figment::new(), b_dir.path().to_path_buf());
        tokio::spawn(serve(rt_b, b.clone(), fake_owner(seen_tx, b.clone(), b_dir.path().to_path_buf())));

        let files = vec![p1.clone(), p2.clone(), PathBuf::from("/elsewhere/x.png")];
        let wire = serde_json::to_value(ByName(&files)).unwrap();
        let (n1, n2) = (p1.file_name().unwrap().to_str().unwrap(), p2.file_name().unwrap().to_str().unwrap());
        assert_eq!(
            wire,
            serde_json::json!([format!("attachments/{n1}"), format!("attachments/{n2}"), "/elsewhere/x.png"]),
            "store paths travel as names, anything else verbatim"
        );
        let params = serde_json::value::to_raw_value(&(Some("s-1"), "look", ByName(&files[..2]), None::<()>)).unwrap();
        let reply = forward(a.as_ref(), &b.me, "send_input", &params).await.unwrap();
        assert_eq!(reply.get(), "\"s-1\"");
        let (text, resolved, _) = seen.recv().await.unwrap();
        assert_eq!(text, "look");
        let dir = artifact::mesh::attachments_dir(b_dir.path());
        assert_eq!(resolved, vec![dir.join(n1), dir.join(n2)], "the owner's own attachments/ paths");
        assert_eq!(std::fs::read(&resolved[0]).unwrap(), png);
        assert_eq!(std::fs::read(&resolved[1]).unwrap(), pdf);
    }

    #[tokio::test]
    async fn owner_errors_pass_through_verbatim() {
        let (a, b) = pair();
        let (seen_tx, _seen) = mpsc::unbounded_channel();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(rt, b.clone(), fake_owner(seen_tx, b.clone(), std::env::temp_dir())));
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

    /// 69 P: the agent crate's hand-written `peer_deliver` request line is
    /// dispatched by [`serve`] like any forwarded RPC (positional params,
    /// internal peer, `Forwarded` marker), and jsonrpsee's response line is
    /// read back as the owner's receipt; an owner-side refusal is `Refused`.
    #[tokio::test]
    async fn peer_deliver_line_round_trips_through_the_owner_serve() {
        use agent::communication::mesh::{self as pm, Failure};
        use agent::{PeerEnvelope, PeerMessageState, PeerReceipt};

        let (a, b) = pair();
        let (seen_tx, mut seen) = mpsc::unbounded_channel::<(SessionId, PeerEnvelope, Option<Extensions>)>();
        let mut m = jsonrpsee::RpcModule::new(seen_tx);
        m.register_async_method("peer_deliver", |params, seen, ext| async move {
            let (session, env): (SessionId, PeerEnvelope) = params.parse()?;
            if env.text == "no" {
                return Err(ErrorObjectOwned::owned(-32000, "send_message: communication disabled: off", None::<()>));
            }
            let receipt = PeerReceipt {
                id: env.id,
                to: format!("session:{session}"),
                state: PeerMessageState::Accepted,
                reply_to: env.reply_to,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            };
            let _ = seen.send((session, env, Some(ext)));
            RpcResult::Ok(receipt)
        })
        .unwrap();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(rt, b.clone(), m.into()));

        let session = SessionId::new();
        let env = PeerEnvelope {
            id: uuid::Uuid::now_v7(),
            from: SessionId::new(),
            text: "hello".into(),
            reply_to: Some(uuid::Uuid::now_v7()),
            request_id: Some("r1".into()),
        };
        let receipt = pm::forward(a.as_ref(), &b.me, session, &env).await.unwrap();
        assert_eq!((receipt.id, receipt.state, receipt.reply_to), (env.id, PeerMessageState::Accepted, env.reply_to));
        assert_eq!(receipt.to, format!("session:{session}"));
        let (s, e, ext) = seen.recv().await.unwrap();
        assert_eq!((s, e), (session, env.clone()));
        let ext = ext.unwrap();
        assert_eq!(ext.get::<Forwarded>().unwrap().0, a.me);
        assert!(ext.get::<link::RpcPeer>().is_some());

        let refused = pm::forward(a.as_ref(), &b.me, session, &PeerEnvelope { text: "no".into(), ..env })
            .await
            .unwrap_err();
        assert_eq!(refused, Failure::Refused("send_message: communication disabled: off".into()));
    }
}
