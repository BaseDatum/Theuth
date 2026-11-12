//! Code index actor lookup for RPC handlers.

use crate::{CODEINDEX_ACTOR, CodeindexActor};
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{NOT_RUNNING, lookup};

/// The codeindex actor; `codeindex.enabled: false` means it was never spawned (60 §10).
pub fn codeindex() -> RpcResult<ActorRef<CodeindexActor>> {
    lookup::<CodeindexActor>(CODEINDEX_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "codeindex is disabled", None::<()>),
        _ => e,
    })
}
