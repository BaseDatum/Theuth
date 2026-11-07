//! Owner: E1 — the `cell_*` RPC handler bodies (design 171 §15.2), delegated to from
//! `api.rs` one line each. Every mutating method runs the press guard first (§12.1
//! invariants 1, 14), then routes: chat scope through `authorize` + `forward::route`
//! (the session owner's engine), note scope through `note::route` (the document's hub);
//! `Ok(None)` from the router means "this engine", and the actor's `Cells` handle answers.
//! Reads run the "owner, not guest" guard and answer neutral values when the plugin is
//! not running.

use std::collections::HashMap;
use std::sync::LazyLock;

use ::cells::{
    CellLatest, CellRef, CellRun, CellsError, DocId, Interpreter, KernelId, OutputPage,
    RunAllRequest, RunGet, RunId, RunMode, RunRequest, Scope, SessionId, StateReply,
};
use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult};
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::{NOT_RUNNING, Server, UNAUTHORIZED};

#[path = "cells_guard.rs"]
mod cells_guard;

/// A cells refusal (anything but "not running" / "not owner"); `data.reason` = `CellsError::code`.
pub const CELL_REFUSED: i32 = -32020;

/// `CellsError` → JSON-RPC: `NOT_RUNNING`, `UNAUTHORIZED`, else [`CELL_REFUSED`]; the message is
/// the text the cell shows, `data: {"reason": <code>}` what clients branch on.
pub(crate) fn cells_err(e: CellsError) -> ErrorObjectOwned {
    let code = match e {
        CellsError::NotRunning => NOT_RUNNING,
        CellsError::NotOwner => UNAUTHORIZED,
        _ => CELL_REFUSED,
    };
    ErrorObjectOwned::owned(
        code,
        e.to_string(),
        Some(serde_json::json!({ "reason": e.code() })),
    )
}

/// The running engine's handle.
pub(crate) async fn handle() -> RpcResult<::cells::Cells> {
    ::cells::Cells::lookup()
        .await
        .ok_or_else(|| cells_err(CellsError::NotRunning))
}

/// Runs this engine forwarded elsewhere (`cell_run` / `cell_run_all` answered by the owner or
/// hub): `cell_stop`, `cell_input`, `cell_ask` and the per-run reads on such an id follow the
/// same route. Process-local; a restart forgets it (the run is `lost` anyway, §5.3).
// ponytail: flat map capped at FORWARDED_CAP, an arbitrary entry is evicted when full; make it
// an LRU if a client ever presses that many remote cells in one engine lifetime.
static FORWARDED: LazyLock<parking_lot::Mutex<HashMap<RunId, Scope>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Ceiling of [`FORWARDED`].
const FORWARDED_CAP: usize = 4096;

fn remember_forwarded(run: RunId, scope: Scope) {
    let mut map = FORWARDED.lock();
    if map.len() >= FORWARDED_CAP
        && let Some(victim) = map.keys().next().cloned()
    {
        map.remove(&victim);
    }
    map.insert(run, scope);
}

fn forwarded_scope(run: &RunId) -> Option<Scope> {
    FORWARDED.lock().get(run).cloned()
}

/// Route one call by scope (§5.1 W-d). Chat: `authorize` then `forward::route`, exactly like
/// `send_input`. Note: `note::route`; a hub elsewhere gets the call over `theuth/rpc/1`.
/// `Ok(None)`: this engine runs it. `Ok(Some(r))`: the remote engine's reply.
async fn route_scope<T: DeserializeOwned>(
    server: &Server,
    ext: &Extensions,
    scope: &Scope,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    match scope {
        Scope::Session(s) => {
            server.authorize(ext, *s).await?;
            crate::forward::route(ext, *s, method, params).await
        }
        Scope::Doc(doc) => match ::cells::note::route(doc).await.map_err(cells_err)? {
            ::cells::note::Route::Local => Ok(None),
            ::cells::note::Route::Hub(hub) => {
                let hub = replica::PeerId(hub);
                match crate::forward::to_peer(ext, &hub, method, params).await? {
                    Some(r) => Ok(Some(r)),
                    None => Err(cells_err(CellsError::Failed(
                        "the note's hub engine is not connected".into(),
                    ))),
                }
            }
        },
    }
}

/// [`route_scope`] for a read (`cell_state`, `cell_runs`, a forwarded run's reads): a chat
/// owned elsewhere is asked of its owner and never taken — viewing a chat must not move it
/// (181: a moved session stayed put only until the origin reopened it). Owner not connected →
/// `Ok(None)`: this engine answers from what it has.
async fn read_scope<T: DeserializeOwned>(
    server: &Server,
    ext: &Extensions,
    scope: &Scope,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Scope::Session(s) = scope else {
        return route_scope(server, ext, scope, method, params).await;
    };
    server.authorize(ext, *s).await?;
    let Ok(store) =
        crate::api::ask::<agent::Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await
    else {
        return Ok(None);
    };
    read_via(ext, &store, *s, method, params).await
}

/// [`read_scope`]'s chat branch over `store`: the owner's answer when another engine owns
/// `s` and is connected, else `Ok(None)`. Never a take: nothing here writes `store`.
async fn read_via<T: DeserializeOwned>(
    ext: &Extensions,
    store: &agent::store::Store,
    s: SessionId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    match store.owner(s).await {
        Ok(Some((owner, _))) if !owner.is_empty() && owner != store.engine() => {
            crate::forward::to_peer(ext, &replica::PeerId(owner), method, params).await
        }
        _ => Ok(None),
    }
}

/// `cell_run`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    server: &Server,
    ext: &Extensions,
    cell: CellRef,
    seen: String,
    mode: RunMode,
    run: RunId,
    edited: Option<String>,
    member_ack: Option<String>,
    confirmed: Option<bool>,
) -> RpcResult<CellRun> {
    let p = cells_guard::press(ext)?;
    let scope = cell.scope();
    if let Some(r) = route_scope(
        server,
        ext,
        &scope,
        "cell_run",
        (&cell, &seen, &mode, &run, &edited, &member_ack, &confirmed),
    )
    .await?
    {
        remember_forwarded(run, scope);
        return Ok(r);
    }
    handle()
        .await?
        .run(RunRequest {
            cell,
            seen,
            mode,
            run,
            edited,
            member_ack,
            confirmed: confirmed.unwrap_or(false),
            pressed_by: cells_guard::pressed_by(ext, &p),
        })
        .await
        .map_err(cells_err)
}

/// `cell_run_all`.
pub(crate) async fn run_all(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    cells: Vec<(CellRef, String)>,
    pass: String,
) -> RpcResult<String> {
    let p = cells_guard::press(ext)?;
    if let Some(r) =
        route_scope(server, ext, &scope, "cell_run_all", (&scope, &cells, &pass)).await?
    {
        return Ok(r);
    }
    handle()
        .await?
        .run_all(RunAllRequest {
            scope,
            cells,
            pass,
            pressed_by: cells_guard::pressed_by(ext, &p),
        })
        .await
        .map_err(cells_err)
}

/// Where a run lives: `Ok(cells)` when it is this engine's (a chat scope is authorized
/// here), `Err(scope)` when this engine forwarded it, else `NotFound`.
async fn locate(
    server: &Server,
    ext: &Extensions,
    run: &RunId,
) -> RpcResult<Result<::cells::Cells, Scope>> {
    let cells = handle().await?;
    if let Some(scope) = cells.run_scope(run).await {
        if let Scope::Session(s) = &scope {
            server.authorize(ext, *s).await?;
        }
        return Ok(Ok(cells));
    }
    match forwarded_scope(run) {
        Some(scope) => Ok(Err(scope)),
        None => Err(cells_err(CellsError::NotFound(run.0.clone()))),
    }
}

/// `cell_stop`.
pub(crate) async fn stop(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    kill: bool,
) -> RpcResult<()> {
    cells_guard::press(ext)?;
    let cells = match locate(server, ext, &run).await? {
        Ok(cells) => cells,
        Err(scope) => {
            if let Some(()) = route_scope(server, ext, &scope, "cell_stop", (&run, kill)).await? {
                return Ok(());
            }
            handle().await?
        }
    };
    cells.stop(run, kill).await.map_err(cells_err)
}

/// `cell_input` (`data` is base64).
pub(crate) async fn input(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    data: String,
) -> RpcResult<()> {
    cells_guard::press(ext)?;
    let cells = match locate(server, ext, &run).await? {
        Ok(cells) => cells,
        Err(scope) => {
            if let Some(()) = route_scope(server, ext, &scope, "cell_input", (&run, &data)).await? {
                return Ok(());
            }
            handle().await?
        }
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|_| cells_err(CellsError::Invalid("data is not base64".into())))?;
    cells.input(run, bytes).await.map_err(cells_err)
}

/// `cell_ask`.
pub(crate) async fn ask(server: &Server, ext: &Extensions, run: RunId) -> RpcResult<()> {
    cells_guard::press(ext)?;
    let cells = match locate(server, ext, &run).await? {
        Ok(cells) => cells,
        Err(scope) => {
            if let Some(()) = route_scope(server, ext, &scope, "cell_ask", (&run,)).await? {
                return Ok(());
            }
            handle().await?
        }
    };
    cells.ask(run).await.map_err(cells_err)
}

/// `cell_attach` (unsubscribe `cell_detach`): items `PtyChunk` from mark C. Local runs only:
/// a forwarded run's bytes are read from the engine that runs it.
pub(crate) async fn attach(
    server: &Server,
    pending: PendingSubscriptionSink,
    ext: &Extensions,
    run: RunId,
) -> SubscriptionResult {
    let admit = async {
        cells_guard::read(ext)?;
        let cells = handle().await?;
        let Some(scope) = cells.run_scope(&run).await else {
            return Err(cells_err(CellsError::NotFound(run.0.clone())));
        };
        if let Scope::Session(s) = scope {
            server.authorize(ext, s).await?;
        }
        let rx = cells.attach(run.clone()).await.map_err(cells_err)?;
        Ok((cells, rx))
    };
    let (cells, mut rx) = match admit.await {
        Ok(a) => a,
        Err(e) => {
            pending.reject(e).await;
            return Ok(());
        }
    };
    let sink = pending.accept().await?;
    tokio::spawn(async move {
        loop {
            let chunk = tokio::select! {
                _ = sink.closed() => break,
                c = rx.recv() => match c {
                    Some(c) => c,
                    None => break,
                },
            };
            let Ok(raw) = serde_json::value::to_raw_value(&chunk) else {
                break;
            };
            if sink.send(SubscriptionMessage::from(raw)).await.is_err() {
                break;
            }
        }
        let _ = cells.detach(run).await;
    });
    Ok(())
}

/// `cell_state`.
pub(crate) async fn state(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    cells: Vec<CellRef>,
) -> RpcResult<StateReply> {
    cells_guard::read(ext)?;
    if let Some(mut r) =
        read_scope::<StateReply>(server, ext, &scope, "cell_state", (&scope, &cells)).await?
    {
        // §14.2 (L11): another engine runs these cells — the host chip names it.
        let host = host_of(&scope).await;
        for c in &mut r.cells {
            if c.host.is_none() {
                c.host.clone_from(&host);
            }
        }
        return Ok(r);
    }
    match ::cells::Cells::lookup().await {
        Some(c) => c.state(scope, cells).await.map_err(cells_err),
        None => Ok(StateReply {
            cells: Vec::new(),
            kernels: Vec::new(),
            pass: None,
        }),
    }
}

/// The engine a forwarded scope's cells run on (§14.2), by its mesh name: the chat session's
/// owner, or the note's hub. `None` when it cannot be told.
async fn host_of(scope: &Scope) -> Option<String> {
    let id = match scope {
        Scope::Session(s) => {
            let store =
                crate::api::ask::<agent::Agents, _>(agent::AGENT_ACTOR, agent::DurableStore)
                    .await
                    .ok()?;
            store
                .owner(*s)
                .await
                .ok()
                .flatten()
                .map(|(owner, _)| owner)
                .filter(|o| !o.is_empty())?
        }
        Scope::Doc(doc) => match ::cells::note::route(doc).await.ok()? {
            ::cells::note::Route::Hub(hub) => hub,
            ::cells::note::Route::Local => return None,
        },
    };
    // The chip shows the peer's name ("Sky's Mac mini"), its id prefix when it has none.
    let name = crate::api::ask::<mesh::Mesh, _>(mesh::MESH_ACTOR, mesh::Status)
        .await
        .ok()
        .and_then(|s| s.peers.into_iter().find(|p| p.id.0 == id))
        .map(|p| p.name.trim().to_owned())
        .filter(|n| !n.is_empty());
    Some(name.unwrap_or_else(|| id.chars().take(8).collect()))
}

/// A per-run read: local first; a `NotFound` for a run this engine forwarded is routed to the
/// engine that runs it.
async fn read_run<T: DeserializeOwned>(
    server: &Server,
    ext: &Extensions,
    run: &RunId,
    method: &str,
    params: impl Serialize,
    local: Result<T, CellsError>,
) -> RpcResult<T> {
    match local {
        Ok(v) => Ok(v),
        Err(CellsError::NotFound(_)) => match forwarded_scope(run) {
            Some(scope) => match read_scope(server, ext, &scope, method, params).await? {
                Some(r) => Ok(r),
                None => Err(cells_err(CellsError::NotFound(run.0.clone()))),
            },
            None => Err(cells_err(CellsError::NotFound(run.0.clone()))),
        },
        Err(e) => Err(cells_err(e)),
    }
}

/// `cell_run_get`.
pub(crate) async fn run_get(server: &Server, ext: &Extensions, run: RunId) -> RpcResult<RunGet> {
    cells_guard::read(ext)?;
    let local = handle().await?.run_get(run.clone()).await;
    read_run(server, ext, &run, "cell_run_get", (&run,), local).await
}

/// `cell_output`.
pub(crate) async fn output(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    offset: u64,
    limit: u32,
    ansi: bool,
) -> RpcResult<OutputPage> {
    cells_guard::read(ext)?;
    let local = handle()
        .await?
        .output(run.clone(), offset, limit, ansi)
        .await;
    read_run(
        server,
        ext,
        &run,
        "cell_output",
        (&run, offset, limit, ansi),
        local,
    )
    .await
}

/// `cell_output_file` → base64.
pub(crate) async fn output_file(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    name: String,
) -> RpcResult<String> {
    cells_guard::read(ext)?;
    let local = handle()
        .await?
        .output_file(run.clone(), name.clone())
        .await
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
    read_run(server, ext, &run, "cell_output_file", (&run, &name), local).await
}

/// `cell_runs`.
pub(crate) async fn runs(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    limit: u32,
) -> RpcResult<Vec<CellRun>> {
    cells_guard::read(ext)?;
    if let Some(r) = read_scope(server, ext, &scope, "cell_runs", (&scope, limit)).await? {
        return Ok(r);
    }
    match ::cells::Cells::lookup().await {
        Some(c) => c.runs(scope, limit).await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}

/// `cell_interpreters` (`engine`: the named engine answers, else this one).
pub(crate) async fn interpreters(
    server: &Server,
    ext: &Extensions,
    cwd: String,
    engine: Option<String>,
) -> RpcResult<Vec<Interpreter>> {
    let _ = server;
    cells_guard::read(ext)?;
    if let Some(e) = engine {
        let peer = replica::PeerId(e.clone());
        return match crate::forward::to_peer(
            ext,
            &peer,
            "cell_interpreters",
            (&cwd, None::<String>),
        )
        .await?
        {
            Some(v) => Ok(v),
            None => Err(cells_err(CellsError::Failed(format!(
                "engine {e} is not connected"
            )))),
        };
    }
    match ::cells::Cells::lookup().await {
        Some(c) => c.interpreters(cwd).await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}

/// `cell_kernel_restart`.
pub(crate) async fn kernel_restart(
    server: &Server,
    ext: &Extensions,
    kernel: KernelId,
) -> RpcResult<()> {
    cells_guard::press(ext)?;
    if let Some(()) = route_scope(
        server,
        ext,
        &kernel.scope,
        "cell_kernel_restart",
        (&kernel,),
    )
    .await?
    {
        return Ok(());
    }
    handle()
        .await?
        .kernel_restart(kernel)
        .await
        .map_err(cells_err)
}

/// `cell_kernel_close`.
pub(crate) async fn kernel_close(
    server: &Server,
    ext: &Extensions,
    kernel: KernelId,
) -> RpcResult<()> {
    cells_guard::press(ext)?;
    if let Some(()) =
        route_scope(server, ext, &kernel.scope, "cell_kernel_close", (&kernel,)).await?
    {
        return Ok(());
    }
    handle()
        .await?
        .kernel_close(kernel)
        .await
        .map_err(cells_err)
}

/// `cell_env_refresh`.
pub(crate) async fn env_refresh(server: &Server, ext: &Extensions) -> RpcResult<()> {
    let _ = server;
    cells_guard::press(ext)?;
    handle().await?.env_refresh().await.map_err(cells_err)
}

/// `cell_live`.
pub(crate) async fn live(server: &Server, ext: &Extensions) -> RpcResult<Vec<CellRun>> {
    let _ = server;
    cells_guard::read(ext)?;
    match ::cells::Cells::lookup().await {
        Some(c) => c.live().await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}

/// `cell_reply_note_get` (171F B9): the note reply `entry` was opened as.
pub(crate) async fn reply_note_get(
    server: &Server,
    ext: &Extensions,
    session: SessionId,
    entry: String,
) -> RpcResult<Option<DocId>> {
    cells_guard::read(ext)?;
    let scope = Scope::Session(session);
    if let Some(r) = route_scope(
        server,
        ext,
        &scope,
        "cell_reply_note_get",
        (&session, &entry),
    )
    .await?
    {
        return Ok(r);
    }
    match ::cells::Cells::lookup().await {
        Some(c) => c.reply_note_get(session, &entry).await.map_err(cells_err),
        None => Ok(None),
    }
}

/// `cell_reply_note_set` (171F B9): remember that reply `entry` was opened as `doc`.
pub(crate) async fn reply_note_set(
    server: &Server,
    ext: &Extensions,
    session: SessionId,
    entry: String,
    doc: DocId,
) -> RpcResult<()> {
    cells_guard::press(ext)?;
    let scope = Scope::Session(session);
    if let Some(()) = route_scope(
        server,
        ext,
        &scope,
        "cell_reply_note_set",
        (&session, &entry, &doc),
    )
    .await?
    {
        return Ok(());
    }
    handle()
        .await?
        .reply_note_set(session, &entry, &doc)
        .await
        .map_err(cells_err)
}

/// Re-exported for `cells_latest` (E2).
pub(crate) type Latest = Vec<CellLatest>;

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        Server::new(rt)
    }

    fn with(peer: link::RpcPeer) -> Extensions {
        let mut ext = Extensions::new();
        ext.insert(peer);
        ext
    }

    fn chat_cell() -> CellRef {
        CellRef::Chat {
            session: runtime::SessionId::new(),
            entry: "e1".into(),
            ordinal: 1,
        }
    }

    fn kernel() -> KernelId {
        KernelId {
            scope: Scope::Session(runtime::SessionId::new()),
            spec: ::cells::KernelSpec::Monty,
        }
    }

    /// Every press method's error code for `ext`, in a fixed order.
    async fn press_codes(server: &Server, ext: &Extensions) -> Vec<i32> {
        let r = RunId("r_1".into());
        let code = |e: RpcResult<()>| e.err().map(|e| e.code()).unwrap_or(0);
        vec![
            code(
                run(
                    server,
                    ext,
                    chat_cell(),
                    "d".into(),
                    RunMode::Cell,
                    r.clone(),
                    None,
                    None,
                    None,
                )
                .await
                .map(drop),
            ),
            code(
                run_all(
                    server,
                    ext,
                    Scope::Session(runtime::SessionId::new()),
                    vec![],
                    "p".into(),
                )
                .await
                .map(drop),
            ),
            code(stop(server, ext, r.clone(), false).await),
            code(input(server, ext, r.clone(), String::new()).await),
            code(ask(server, ext, r).await),
            code(kernel_restart(server, ext, kernel()).await),
            code(kernel_close(server, ext, kernel()).await),
            code(env_refresh(server, ext).await),
            code(
                reply_note_set(
                    server,
                    ext,
                    runtime::SessionId::new(),
                    "e1".into(),
                    DocId("d1".into()),
                )
                .await,
            ),
        ]
    }

    /// Every read method's error code for `ext`, in a fixed order.
    async fn read_codes(server: &Server, ext: &Extensions) -> Vec<i32> {
        let r = RunId("r_1".into());
        let code = |e: RpcResult<()>| e.err().map(|e| e.code()).unwrap_or(0);
        vec![
            code(
                state(
                    server,
                    ext,
                    Scope::Session(runtime::SessionId::new()),
                    vec![],
                )
                .await
                .map(drop),
            ),
            code(run_get(server, ext, r.clone()).await.map(drop)),
            code(output(server, ext, r.clone(), 0, 10, false).await.map(drop)),
            code(output_file(server, ext, r, "a".into()).await.map(drop)),
            code(
                runs(server, ext, Scope::Session(runtime::SessionId::new()), 1)
                    .await
                    .map(drop),
            ),
            code(interpreters(server, ext, ".".into(), None).await.map(drop)),
            code(live(server, ext).await.map(drop)),
            code(
                reply_note_get(server, ext, runtime::SessionId::new(), "e1".into())
                    .await
                    .map(drop),
            ),
        ]
    }

    #[test]
    fn refusals_carry_a_stable_reason() {
        let e = cells_err(CellsError::Changed);
        assert_eq!(e.code(), CELL_REFUSED);
        assert_eq!(e.message(), "this cell changed — look again");
        assert!(e.data().unwrap().get().contains("\"reason\":\"changed\""));
        assert_eq!(cells_err(CellsError::NotOwner).code(), UNAUTHORIZED);
        assert_eq!(cells_err(CellsError::NotRunning).code(), NOT_RUNNING);
    }

    /// §12.1 invariant 1: the internal peer (MCP, ACP, TUI) is not a press.
    #[tokio::test]
    async fn internal_peer_is_refused_for_every_press_method() {
        let server = server();
        let ext = with(link::RpcPeer::internal());
        assert_eq!(press_codes(&server, &ext).await, [UNAUTHORIZED; 9]);
    }

    /// §12.1 invariant 1: a guest is not a press.
    #[tokio::test]
    async fn guest_peer_is_refused_for_every_press_method() {
        let server = server();
        let (peer, _g) = link::RpcPeer::open_guest([1; 32], [2; 32]);
        let ext = with(peer);
        assert_eq!(press_codes(&server, &ext).await, [UNAUTHORIZED; 9]);
    }

    /// A forwarded request (the forwarder already applied the guard) passes as internal.
    #[tokio::test]
    async fn forwarded_internal_peer_passes_the_guard() {
        let server = server();
        let mut ext = with(link::RpcPeer::internal());
        ext.insert(crate::forward::Forwarded(replica::PeerId(
            "engine-b".into(),
        )));
        assert!(cells_guard::press(&ext).is_ok());
        let e = env_refresh(&server, &ext).await.unwrap_err();
        assert_ne!(e.code(), UNAUTHORIZED);
        assert_eq!(e.code(), NOT_RUNNING);
    }

    /// §15.2 "owner, not guest": every read refuses a guest.
    #[tokio::test]
    async fn every_read_method_refuses_guests() {
        let server = server();
        let (peer, _g) = link::RpcPeer::open_guest([1; 32], [2; 32]);
        let ext = with(peer);
        assert_eq!(read_codes(&server, &ext).await, [UNAUTHORIZED; 8]);
    }

    /// No authenticated connection is never "local".
    #[tokio::test]
    async fn no_peer_is_unauthorized() {
        let server = server();
        let ext = Extensions::new();
        assert_eq!(press_codes(&server, &ext).await, [UNAUTHORIZED; 9]);
        assert_eq!(read_codes(&server, &ext).await, [UNAUTHORIZED; 8]);
    }

    #[test]
    fn pressed_by_names_kind_and_device() {
        let (local, _g1) = link::RpcPeer::open();
        let mut ios = with(local.clone());
        ios.insert(crate::Client("ios"));
        assert!(cells_guard::pressed_by(&ios, &local).starts_with("phone · "));

        let (remote, _g2) = link::RpcPeer::open_remote();
        let ext = with(remote.clone());
        assert_eq!(cells_guard::pressed_by(&ext, &remote), "web · relay");

        let ext = with(local.clone());
        assert!(cells_guard::pressed_by(&ext, &local).starts_with("desktop · "));

        let mut fwd = with(link::RpcPeer::internal());
        fwd.insert(crate::forward::Forwarded(replica::PeerId(
            "engine-b".into(),
        )));
        assert_eq!(
            cells_guard::pressed_by(&fwd, &link::RpcPeer::internal()),
            "desktop · engine-b"
        );
    }

    /// CELLAFTER F1: an old client's `cell_run` — six params, no `confirmed` — parses and is
    /// answered exactly like one sending `null` or `false`: an unconfirmed press, never
    /// "invalid params".
    #[tokio::test]
    async fn cell_run_without_confirmed_is_an_unconfirmed_press() {
        use crate::api::ApiServer as _;
        use jsonrpsee::core::server::MethodsError;
        let mut methods: jsonrpsee::Methods = server().into_rpc().into();
        let (peer, _g) = link::RpcPeer::open();
        methods.extensions_mut().insert(peer);
        let (cell, run) = (chat_cell(), RunId("r_old".into()));
        let code = |r: Result<CellRun, MethodsError>| match r {
            Err(MethodsError::JsonRpc(e)) => e.code(),
            other => panic!("{other:?}"),
        };
        let none = None::<String>;
        let six = code(
            methods
                .call("cell_run", (&cell, "d", RunMode::Cell, &run, &none, &none))
                .await,
        );
        let null = code(
            methods
                .call(
                    "cell_run",
                    (&cell, "d", RunMode::Cell, &run, &none, &none, None::<bool>),
                )
                .await,
        );
        let unconfirmed = code(
            methods
                .call(
                    "cell_run",
                    (&cell, "d", RunMode::Cell, &run, &none, &none, false),
                )
                .await,
        );
        assert_ne!(six, jsonrpsee::types::error::INVALID_PARAMS_CODE);
        assert_eq!((six, null), (unconfirmed, unconfirmed));
    }

    /// 181: the chat view's cell reads on an engine that does not own the chat ask the owner
    /// (here: not connected → answered locally) and leave ownership where it is — they used
    /// to go through the taking front door, so viewing a moved session pulled it back.
    #[tokio::test]
    async fn cell_reads_of_a_chat_owned_elsewhere_leave_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (theirs, mine) = (runtime::SessionId::new(), runtime::SessionId::new());
        let them = agent::store::Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        them.create_session(theirs, "m", None, None, None)
            .await
            .unwrap();
        them.close().await;
        let store = agent::store::Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        store
            .create_session(mine, "m", None, None, None)
            .await
            .unwrap();
        let ext = Extensions::new();
        for s in [theirs, mine] {
            let scope = Scope::Session(s);
            let got: Option<StateReply> = read_via(
                &ext,
                &store,
                s,
                "cell_state",
                (&scope, Vec::<CellRef>::new()),
            )
            .await
            .unwrap();
            assert!(got.is_none(), "answered here");
        }
        assert_eq!(store.owner(theirs).await.unwrap(), Some(("them".into(), 1)));
        assert_eq!(store.owner(mine).await.unwrap(), Some(("me".into(), 1)));
        store.close().await;
    }
}
