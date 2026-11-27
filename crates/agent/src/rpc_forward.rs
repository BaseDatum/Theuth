//! The front door of a session written elsewhere (68 §5; 183 §4.1, D1/D2):
//! every session-mutating RPC first asks [`route`]. The owner is here (or the
//! row is unknown or unowned) → the call proceeds. Owned by another engine
//! that is connected → the call is forwarded verbatim to the owner as
//! JSON-RPC over `theuth/rpc/1`: one request line, one response line, one
//! QUIC stream, 30 s. This holds for user sessions and agent desks alike;
//! the front door never takes a session (only `session_take`, `session_move`
//! and the offline card's resume-here move one). An owner that is not
//! connected, or gives no answer, is [`OWNER_UNREACHABLE`] at once with an
//! [`OwnerUnreachable`] payload; the desktop renders the "continue here?" card.
//!
//! 183 §5.6: a chat created on another engine (`create_session{engine}`) is
//! routed to that engine by id while it is in use ([`expected`]): before its
//! row replicates, the mutating calls through [`classify`]; the reads the chat
//! route makes through [`to_creator`] until it goes unused, so a lagging
//! replica never answers for it.
//!
//! The owner side ([`serve`]) dispatches each forwarded request into its own
//! [`rpc_core::Server`] as the internal peer, with a [`Forwarded`] marker in
//! the extensions so the request is never forwarded a second hop. A forwarded
//! call for a row this engine does not own (it moved meanwhile) answers
//! [`NOT_OWNER`] (183 §4.8), not a relay loop.
//!
//! Attachments (68 follow-up A1): a forwarded `files` list carries **names**,
//! not the forwarder's absolute paths — each `<data_dir>/attachments/<sha16>.<ext>`
//! is sent as the relative marker `attachments/<sha16>.<ext>` ([`ByName`]);
//! the owner resolves every marker with `artifact::mesh::fetch_attachment`,
//! asking the forwarder first ([`resolve_files`]), before the actor sees it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate as agent;
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

/// Async artifact fetch using the borrowed transport and forwarding preference.
pub type FetchAttachment = for<'a> fn(
    &'a dyn Transport, &'a Path, &'a str, Option<&'a PeerId>,
) -> futures::future::BoxFuture<'a, anyhow::Result<PathBuf>>;

/// 199 P2: fetch `(holder, id)` into this data dir's `files/cache/`, verified against
/// the notice; `Ok(None)` when the holder answers `Missing` (`artifact::mesh`).
pub type FetchContactFile =
    for<'a> fn(
        &'a dyn Transport,
        &'a Path,
        &'a PeerId,
        &'a str,
        &'a crate::communication::files::ReceivedNotice,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<PathBuf>>>;

/// Reverse-plugin operations installed once before RPC dispatch.
pub struct ForwardHooks {
    /// Whether the sessions plugin is registered now.
    pub sessions_running: fn() -> bool,
    /// Expected owner, looked up at call time.
    pub expected: fn(SessionId) -> Option<PeerId>,
    /// Preserve the artifact owner's attachment validation/naming.
    pub attachment_name: for<'a> fn(&'a Path) -> Option<&'a str>,
    /// Attachment directory for this request's data directory.
    pub attachments_dir: fn(&Path) -> PathBuf,
    /// Fetch an actual attachment marker, preferring the forwarder.
    pub fetch_attachment: FetchAttachment,
    /// 199 P2: fetch a held contact file from its holder.
    pub fetch_contact_file: FetchContactFile,
}

static HOOKS: std::sync::OnceLock<ForwardHooks> = std::sync::OnceLock::new();

/// Install the same immutable registry-reading operations for every runtime.
pub fn install_forward_hooks(hooks: ForwardHooks) {
    HOOKS.get_or_init(|| hooks);
}

pub(crate) fn hooks() -> &'static ForwardHooks {
    HOOKS.get().expect("rpc forwarding hooks must be installed before dispatch")
}


/// JSON-RPC error code: the session's owner cannot be reached (or taken from).
/// `data` is a [`FrontDoorError`] in serde's external enum form:
/// `{"OwnerUnreachable": {"owner": "<engine id>", "last_seen": <ms epoch | null>}}`
/// — the shape U's desktop card and TUI parse.
pub const OWNER_UNREACHABLE: i32 = -32010;

/// JSON-RPC error code (139 §6.1, D16): the session is mid-release-drain on
/// this engine; retryable. `data` is [`FrontDoorError::SessionDraining`] in
/// the same external enum form. The supervisor raises
/// `SessionDraining:<reason>` from the input handler; [`crate::rpc_support::session_draining`]
/// maps it here, the composer retries once. `-32011` is `NOT_HUB` (69 Y).
pub const SESSION_DRAINING: i32 = -32012;

/// JSON-RPC error code (183 §4.8): a forwarded call reached an engine that no
/// longer owns the row. `data` is [`FrontDoorError::NotOwner`].
pub const NOT_OWNER: i32 = -32013;

/// JSON-RPC error code (183 §4.3): `cell_attach` / `subscribe_pty` for a run
/// or job on another engine. `data` is [`FrontDoorError::RunsElsewhere`].
pub const RUNS_ELSEWHERE: i32 = -32014;

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
    /// 139 D16: the session is draining for a release (or another reason)
    /// on this engine; retry in a moment. *"<session> is moving to <taker> —
    /// retry in a moment"* / *"<session> is pausing for <reason> — resumes on
    /// the next start"*.
    SessionDraining {
        /// The draining session.
        session: SessionId,
        /// `release` on the ownership path; the drain's reason otherwise.
        reason: String,
        /// The taker, when known (`release`).
        #[cfg_attr(feature = "ts", ts(optional))]
        next_owner: Option<String>,
    },
    /// 183 §4.8: this engine does not own the forwarded call's session; the
    /// replicated owner change follows.
    NotOwner {
        /// The owner as this engine knows it (mesh `EngineId` as text).
        owner: String,
    },
    /// 183 §4.3: the run or job streams on another engine (live output stays there).
    RunsElsewhere {
        /// The engine running it (mesh `EngineId` as text).
        owner: String,
    },
}

/// Auxiliary client error contract; RpcResult success metadata cannot carry it.
pub fn client_error_schema() -> serde_json::Value {
    schemars::generate::SchemaSettings::draft07().for_serialize().into_generator()
        .into_root_schema_for::<FrontDoorError>().to_value()
}

/// Where a session-mutating call goes (183 §4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The owner is this engine (or the session is unknown here): proceed locally.
    Here,
    /// Owned elsewhere and the owner is connected: forward to it.
    Forward(PeerId),
    /// 183 §4.4 (D2): owned elsewhere and the owner is not connected: `-32010` at once.
    Offline(PeerId),
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
        s.collect_seq(
            self.0
                .iter()
                .map(|f| match (hooks().attachment_name)(f) {
                    Some(name) => Path::new("attachments").join(name),
                    None => f.clone(),
                }),
        )
    }
}

/// The marker's name: `attachments/<name>` → `<name>`.
fn marker(f: &Path) -> Option<&str> {
    f.strip_prefix("attachments").ok().and_then(Path::to_str)
}

/// The owner side of [`ByName`]: each `attachments/<name>` marker in `files`
/// becomes this engine's `<data_dir>/attachments/<name>`, fetched from the
/// forwarder (first) or any peer when not already here. Other entries pass.
pub async fn resolve_files(
    ext: &Extensions,
    data_dir: &Path,
    files: Vec<PathBuf>,
) -> RpcResult<Vec<PathBuf>> {
    if !files.iter().any(|f| marker(f).is_some()) {
        return Ok(files);
    }
    let transport = match replica::actor() {
        Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
        None => None,
    };
    let Some(t) = transport else {
        return Err(rpc_core::failed(
            "attachments: unavailable: no peer has it",
        ));
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
    let dir = (hooks().attachments_dir)(data_dir);
    for f in &mut files {
        let Some(name) = marker(f) else { continue };
        *f = (hooks().fetch_attachment)(t, &dir, name, prefer)
            .await
            .map_err(|e| rpc_core::failed(format!("{}: {e:#}", f.display())))?;
    }
    Ok(files)
}

/// The mesh peer owning `session`, when it is another engine and the
/// sessions plugin is running: the first peer to ask for its files.
pub async fn owner_peer(session: SessionId) -> Option<PeerId> {
    if !(hooks().sessions_running)() {
        return None;
    }
    let store = rpc_core::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore)
        .await
        .ok()?;
    let (owner, _) = store.owner(session).await.ok().flatten()?;
    (!owner.is_empty() && owner != store.engine()).then_some(PeerId(owner))
}

/// Classify `session` from its row (183 §4.1): `Here` when unknown, unowned
/// or ours; otherwise `Forward(owner)` when the owner is in `connected` (the
/// transport's peers at call time, W-3), else `Offline(owner)`. User sessions
/// and desks follow the same rule. 183 §5.6: an unknown row another engine
/// just created for us (the session owner's expected-row registry) is that engine's.
pub async fn classify(
    store: &Store,
    session: SessionId,
    connected: &[PeerId],
) -> anyhow::Result<Route> {
    let owner = match store.session(session).await? {
        None => match (hooks().expected)(session) {
            Some(engine) => engine,
            None => return Ok(Route::Here),
        },
        Some(row) if row.owner.is_empty() || row.owner == store.engine() => {
            return Ok(Route::Here);
        }
        Some(row) => PeerId(row.owner),
    };
    Ok(if connected.contains(&owner) {
        Route::Forward(owner)
    } else {
        Route::Offline(owner)
    })
}

/// 183 §5.6: a chat another engine created at this engine's request and still
/// in use (the session owner's expected-row registry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The engine that created it.
    pub engine: PeerId,
    /// Its row has replicated here (owned by `engine`).
    pub here: bool,
}

/// [`Expected`] for `session`: `None` on a forwarded call, without the
/// sessions plugin, when nothing was created elsewhere for it (or that went
/// unused for the session owner's expected-row timeout), or when its row here names
/// another owner (it moved: normal routing applies).
pub async fn expected(ext: &Extensions, session: SessionId) -> RpcResult<Option<Expected>> {
    if ext.get::<Forwarded>().is_some() || !(hooks().sessions_running)() {
        return Ok(None);
    }
    let Some(engine) = (hooks().expected)(session) else {
        return Ok(None);
    };
    let store = rpc_core::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    match store
        .owner(session)
        .await
        .map_err(|e| internal(format!("{e:#}")))?
    {
        None => Ok(Some(Expected {
            engine,
            here: false,
        })),
        Some((owner, _)) if owner == engine.0 => Ok(Some(Expected { engine, here: true })),
        Some(_) => Ok(None),
    }
}

/// How long [`to_creator`] waits for the creator when the row is here and a
/// local answer is the fallback (a connected but hung creator must not cost
/// [`TIMEOUT`] per read).
const CREATOR_WAIT: Duration = Duration::from_secs(5);

/// A read the front door answers from its replica, for a chat created
/// elsewhere and in use ([`expected`]): forwarded to its creator, so a
/// replica that has not caught up (or has no row yet) never answers for it.
/// `Ok(None)`: answer here — nothing expected, or the creator cannot be
/// reached (within [`CREATOR_WAIT`]) and the row is here.
pub async fn to_creator<T: DeserializeOwned>(
    ext: &Extensions,
    session: SessionId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Some(Expected { engine, here }) = expected(ext, session).await? else {
        return Ok(None);
    };
    let wait = if here { CREATOR_WAIT } else { TIMEOUT };
    match to_peer_within(ext, &engine, method, params, wait).await {
        Ok(Some(v)) => Ok(Some(v)),
        Ok(None) if here => Ok(None),
        Err(e) if here && e.code() == OWNER_UNREACHABLE => Ok(None),
        Ok(None) => Err(unreachable(engine.0, "not connected").await),
        Err(e) => Err(e),
    }
}

/// 183 §4.8.2: the owner side of a forwarded call. A row this engine does not
/// own answers [`NOT_OWNER`] (the forwarder's view was stale); an unknown row
/// proceeds to the local "no such session" error.
pub async fn admit_forwarded(store: &Store, session: SessionId) -> RpcResult<()> {
    match store
        .owner(session)
        .await
        .map_err(|e| internal(format!("{e:#}")))?
    {
        Some((owner, _)) if !owner.is_empty() && owner != store.engine() => Err(not_owner(owner)),
        _ => Ok(()),
    }
}

/// The front door for one call. `Ok(None)`: proceed locally (the owner is
/// here). `Ok(Some(reply))`: the owner answered the forwarded call. `Err`:
/// the owner's error verbatim, [`OWNER_UNREACHABLE`], or (on a forwarded call
/// for a row owned elsewhere) [`NOT_OWNER`]. `params` is the method's
/// positional parameter list in trait order, as the generated client sends it.
pub async fn route<T: DeserializeOwned>(
    ext: &Extensions,
    session: SessionId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    // Lone engine (no sessions plugin): local.
    if !(hooks().sessions_running)() {
        return Ok(None);
    }
    let store = rpc_core::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    // Already forwarded once: run here if ours, never a second hop.
    if ext.get::<Forwarded>().is_some() {
        return admit_forwarded(&store, session).await.map(|()| None);
    }
    let connected = match transport().await {
        Some(t) => t.peers(),
        None => Vec::new(),
    };
    let owner = match classify(&store, session, &connected)
        .await
        .map_err(|e| internal(format!("{e:#}")))?
    {
        Route::Here => return Ok(None),
        Route::Forward(owner) => owner,
        Route::Offline(owner) => return Err(unreachable(owner.0, "not connected").await),
    };
    match to_peer(ext, &owner, method, params).await? {
        Some(v) => Ok(Some(v)),
        None => Err(unreachable(owner.0, "not connected").await),
    }
}

/// The mesh transport, when the replica plugin runs and the mesh is up.
pub(crate) async fn transport() -> Option<Arc<dyn Transport>> {
    replica::actor()?
        .ask(replica::GetTransport)
        .await
        .ok()
        .flatten()
}

/// Where a call naming engine `engine` runs: `None` here (`engine` unset,
/// empty, this engine `me`, or the request was itself `forwarded`), else
/// that peer. The pure decision behind [`engine_peer`].
pub fn engine_target(engine: Option<&str>, forwarded: bool, me: &str) -> Option<PeerId> {
    let engine = engine.filter(|e| !e.is_empty())?;
    (!forwarded && engine != me).then(|| PeerId(engine.to_owned()))
}

/// [`engine_target`] for this request: the peer an `engine` parameter names,
/// `None` to run here.
pub async fn engine_peer(ext: &Extensions, engine: Option<&str>) -> RpcResult<Option<PeerId>> {
    let forwarded = ext.get::<Forwarded>().is_some();
    if forwarded || engine.is_none_or(str::is_empty) {
        return Ok(None);
    }
    let store = rpc_core::ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    Ok(engine_target(engine, forwarded, store.engine()))
}

/// [`to_peer`] where the peer must answer: one it cannot reach from here is
/// [`OWNER_UNREACHABLE`].
pub async fn call_peer<T: DeserializeOwned>(
    ext: &Extensions,
    peer: &PeerId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<T> {
    match to_peer(ext, peer, method, params).await? {
        Some(v) => Ok(v),
        None => Err(unreachable(peer.0.clone(), "not connected").await),
    }
}

/// 183 §5.6 (W-10, W-12): a call aimed at engine `engine` by id. `Ok(None)`:
/// run it here (`engine` unset, empty, this engine, or this request was itself
/// forwarded). Otherwise the target's answer, or [`OWNER_UNREACHABLE`] when it
/// is not connected.
pub async fn to_engine<T: DeserializeOwned>(
    ext: &Extensions,
    engine: Option<&str>,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    match engine_peer(ext, engine).await? {
        Some(peer) => call_peer(ext, &peer, method, params).await.map(Some),
        None => Ok(None),
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
    to_peer_within(ext, peer, method, params, TIMEOUT).await
}

/// [`to_peer`] waiting `wait` for the answer (181: a move's take outlasts [`TIMEOUT`]).
pub async fn to_peer_within<T: DeserializeOwned>(
    ext: &Extensions,
    peer: &PeerId,
    method: &str,
    params: impl Serialize,
    wait: Duration,
) -> RpcResult<Option<T>> {
    if ext.get::<Forwarded>().is_some() {
        return Ok(None);
    }
    let Some(transport) = transport().await.filter(|t| t.peers().contains(peer)) else {
        return Ok(None);
    };
    let params = serde_json::value::to_raw_value(&params).map_err(|e| internal(e.to_string()))?;
    match forward_within(transport.as_ref(), peer, method, &params, wait).await {
        Ok(raw) => serde_json::from_str(raw.get())
            .map(Some)
            .map_err(|e| internal(e.to_string())),
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
    forward_within(transport, owner, method, params, TIMEOUT).await
}

/// [`forward`] waiting `wait` for the answer.
pub async fn forward_within(
    transport: &dyn Transport,
    owner: &PeerId,
    method: &str,
    params: &RawValue,
    wait: Duration,
) -> Result<Box<RawValue>, Result<String, ErrorObjectOwned>> {
    let exchange = async {
        let stream = transport
            .open(owner, ALPN_RPC)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let (read, mut write) = tokio::io::split(stream);
        let req = Request::borrowed(method, Some(params), Id::Number(1));
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        write
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
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
    let reply = match tokio::time::timeout(wait, exchange).await {
        Ok(Ok(r)) => r,
        Ok(Err(why)) => return Err(Ok(why)),
        Err(_) => return Err(Ok(format!("no answer within {}s", wait.as_secs()))),
    };
    let rp: Response<&RawValue> =
        serde_json::from_str(reply.trim_end()).map_err(|e| Ok(format!("malformed reply: {e}")))?;
    match rp.payload {
        ResponsePayload::Success(v) => Ok(<RawValue as ToOwned>::to_owned(*v)),
        ResponsePayload::Error(e) => Err(Err(e.into_owned())),
    }
}

/// The owner side: accept `theuth/rpc/1` streams until shutdown and answer
/// each with one dispatch of `methods` as the internal peer, marked
/// [`Forwarded`] with the opening peer. `methods` is the full API ([`crate::methods`]).
pub async fn serve(rt: Runtime, transport: Arc<dyn Transport>, mut methods: jsonrpsee::Methods) {
    methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
    methods.extensions_mut().insert(rpc_core::DiscoveryView::Mesh);
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
async fn answer(
    stream: Box<dyn replica::ByteStream>,
    methods: &jsonrpsee::Methods,
) -> anyhow::Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut line = String::new();
    BufReader::with_capacity(64 * 1024, read)
        .take(MAX_LINE as u64)
        .read_line(&mut line)
        .await?;
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
pub async fn unreachable(owner: String, why: &str) -> ErrorObjectOwned {
    let last_seen = match kameo::actor::ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR) {
        Ok(Some(m)) => m
            .ask(mesh::Status)
            .await
            .ok()
            .and_then(|s| {
                s.peers
                    .into_iter()
                    .find(|p| p.id.0 == owner)
                    .map(|p| p.last_seen_ms)
            })
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

/// A [`NOT_OWNER`] error naming the owner this engine knows (183 §4.8.2).
pub fn not_owner(owner: String) -> ErrorObjectOwned {
    let short = owner.get(..8).unwrap_or(&owner).to_owned();
    ErrorObjectOwned::owned(
        NOT_OWNER,
        format!("not the owner: the session is owned by {short}"),
        Some(FrontDoorError::NotOwner { owner }),
    )
}

/// A [`RUNS_ELSEWHERE`] error: the run's live output stays on `owner` (183 §4.3).
pub fn runs_elsewhere(owner: String) -> ErrorObjectOwned {
    let short = owner.get(..8).unwrap_or(&owner).to_owned();
    ErrorObjectOwned::owned(
        RUNS_ELSEWHERE,
        format!("live output streams on {short}"),
        Some(FrontDoorError::RunsElsewhere { owner }),
    )
}

fn internal(msg: String) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        jsonrpsee::types::error::INTERNAL_ERROR_CODE,
        msg,
        None::<()>,
    )
}

