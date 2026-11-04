//! Owner: E1 — the `cell_*` admission guards (design 171 §12.1 invariant 1) and the
//! `pressed_by` text the run row keeps (§8.4).

use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;

use crate::api::peer;
use crate::cells::cells_err;

/// Invariant 1, a human press: an authenticated peer that is not a guest and not the internal
/// peer — unless the request carries [`crate::forward::Forwarded`], in which case the engine
/// that forwarded it already applied this check. No peer → `UNAUTHORIZED`; else `NotOwner`.
pub(crate) fn press(ext: &Extensions) -> RpcResult<link::RpcPeer> {
    let p = read(ext)?;
    if p.is_internal() && ext.get::<crate::forward::Forwarded>().is_none() {
        return Err(cells_err(::cells::CellsError::NotOwner));
    }
    Ok(p)
}

/// "Owner, not guest" (§15.2 reads): an authenticated peer that is not a guest; the internal
/// peer (MCP server, ACP half, TUI) may read.
pub(crate) fn read(ext: &Extensions) -> RpcResult<link::RpcPeer> {
    let p = peer(ext)?;
    if p.guest().is_some() {
        return Err(cells_err(::cells::CellsError::NotOwner));
    }
    Ok(p)
}

/// `"<kind> · <device>"` for the run row. `kind`: `phone` when the socket declared
/// [`crate::Client`] `ios`, `web` for a relay connection, else `desktop`. `device`: this
/// engine's hostname for a local connection, `relay` for a remote one. A forwarded press
/// (`Forwarded`) is recorded as `desktop · <forwarding engine's peer id>`: the original client
/// kind does not travel with the forwarded call and the forwarder's id is the one fact we hold.
pub(crate) fn pressed_by(ext: &Extensions, peer: &link::RpcPeer) -> String {
    if let Some(fwd) = ext.get::<crate::forward::Forwarded>() {
        return format!("desktop · {}", fwd.0.0);
    }
    let ios = ext.get::<crate::Client>().is_some_and(|c| c.0 == "ios");
    let kind = if ios {
        "phone"
    } else if peer.remote() {
        "web"
    } else {
        "desktop"
    };
    let device = if peer.remote() {
        "relay"
    } else {
        platform::process::hostname()
    };
    format!("{kind} · {device}")
}
