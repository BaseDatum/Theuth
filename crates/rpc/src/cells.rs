//! Owner: E1 — the `cell_*` RPC handler bodies (design 171 §15.2), delegated to from
//! `api.rs` one line each. E1 adds here (and in `cells_guard.rs`) the press guard (§12.1
//! invariants 1, 14), `authorize`, `forward::route` and the `pressed_by` text. Skeleton: no
//! guard yet — so every mutating method refuses `NotBuilt("E1")` before reaching the actor;
//! reads answer the actor's neutral values (empty when the plugin is not running).

use ::cells::{
    CellLatest, CellRef, CellRun, CellsError, Interpreter, KernelId, OutputPage, RunGet, RunId,
    RunMode, Scope, StateReply,
};
use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult};
use jsonrpsee::server::PendingSubscriptionSink;
use jsonrpsee::types::ErrorObjectOwned;

use crate::api::{NOT_RUNNING, Server, UNAUTHORIZED};

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

/// Skeleton refusal of every mutating method until E1's guard lands.
fn not_built() -> ErrorObjectOwned {
    cells_err(CellsError::NotBuilt("E1"))
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
) -> RpcResult<CellRun> {
    let _ = (server, ext, cell, seen, mode, run, edited, member_ack);
    Err(not_built())
}

/// `cell_run_all`.
pub(crate) async fn run_all(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    cells: Vec<(CellRef, String)>,
    pass: String,
) -> RpcResult<String> {
    let _ = (server, ext, scope, cells, pass);
    Err(not_built())
}

/// `cell_stop`.
pub(crate) async fn stop(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    kill: bool,
) -> RpcResult<()> {
    let _ = (server, ext, run, kill);
    Err(not_built())
}

/// `cell_input` (`data` is base64).
pub(crate) async fn input(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    data: String,
) -> RpcResult<()> {
    let _ = (server, ext, run, data);
    Err(not_built())
}

/// `cell_ask`.
pub(crate) async fn ask(server: &Server, ext: &Extensions, run: RunId) -> RpcResult<()> {
    let _ = (server, ext, run);
    Err(not_built())
}

/// `cell_attach` (unsubscribe `cell_detach`): items `PtyChunk` from mark C.
pub(crate) async fn attach(
    server: &Server,
    pending: PendingSubscriptionSink,
    ext: &Extensions,
    run: RunId,
) -> SubscriptionResult {
    let _ = (server, ext, run);
    pending.reject(not_built()).await;
    Ok(())
}

/// `cell_state`.
pub(crate) async fn state(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    cells: Vec<CellRef>,
) -> RpcResult<StateReply> {
    let _ = (server, ext);
    match ::cells::Cells::lookup().await {
        Some(c) => c.state(scope, cells).await.map_err(cells_err),
        None => Ok(StateReply {
            cells: Vec::new(),
            kernels: Vec::new(),
        }),
    }
}

/// `cell_run_get`.
pub(crate) async fn run_get(server: &Server, ext: &Extensions, run: RunId) -> RpcResult<RunGet> {
    let _ = (server, ext);
    handle().await?.run_get(run).await.map_err(cells_err)
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
    let _ = (server, ext);
    handle()
        .await?
        .output(run, offset, limit, ansi)
        .await
        .map_err(cells_err)
}

/// `cell_output_file` → base64.
pub(crate) async fn output_file(
    server: &Server,
    ext: &Extensions,
    run: RunId,
    name: String,
) -> RpcResult<String> {
    let _ = (server, ext);
    let bytes = handle()
        .await?
        .output_file(run, name)
        .await
        .map_err(cells_err)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `cell_runs`.
pub(crate) async fn runs(
    server: &Server,
    ext: &Extensions,
    scope: Scope,
    limit: u32,
) -> RpcResult<Vec<CellRun>> {
    let _ = (server, ext);
    match ::cells::Cells::lookup().await {
        Some(c) => c.runs(scope, limit).await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}

/// `cell_interpreters` (`engine`: E1 routes to the named engine).
pub(crate) async fn interpreters(
    server: &Server,
    ext: &Extensions,
    cwd: String,
    engine: Option<String>,
) -> RpcResult<Vec<Interpreter>> {
    let _ = (server, ext, engine);
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
    let _ = (server, ext, kernel);
    Err(not_built())
}

/// `cell_kernel_close`.
pub(crate) async fn kernel_close(
    server: &Server,
    ext: &Extensions,
    kernel: KernelId,
) -> RpcResult<()> {
    let _ = (server, ext, kernel);
    Err(not_built())
}

/// `cell_env_refresh`.
pub(crate) async fn env_refresh(server: &Server, ext: &Extensions) -> RpcResult<()> {
    let _ = (server, ext);
    Err(not_built())
}

/// `cell_live`.
pub(crate) async fn live(server: &Server, ext: &Extensions) -> RpcResult<Vec<CellRun>> {
    let _ = (server, ext);
    match ::cells::Cells::lookup().await {
        Some(c) => c.live().await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}

/// Re-exported for `cells_latest` (E2).
pub(crate) type Latest = Vec<CellLatest>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_carry_a_stable_reason() {
        let e = cells_err(CellsError::Changed);
        assert_eq!(e.code(), CELL_REFUSED);
        assert_eq!(e.message(), "this cell changed — look again");
        assert!(e.data().unwrap().get().contains("\"reason\":\"changed\""));
        assert_eq!(cells_err(CellsError::NotOwner).code(), UNAUTHORIZED);
        assert_eq!(cells_err(CellsError::NotRunning).code(), NOT_RUNNING);
    }

    /// Skeleton: no mutating method may succeed before E1's guard exists.
    #[tokio::test]
    async fn mutating_methods_refuse_until_built() {
        let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let server = Server::new(rt);
        let ext = Extensions::new();
        let r = RunId("r_1".into());
        assert!(stop(&server, &ext, r.clone(), false).await.is_err());
        assert!(
            input(&server, &ext, r.clone(), String::new())
                .await
                .is_err()
        );
        assert!(ask(&server, &ext, r).await.is_err());
        assert!(env_refresh(&server, &ext).await.is_err());
    }
}
