//! Update actor lookup for RPC method owners.

use crate as update;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The update actor; absent when `update.enabled: false` or the plugin is not loaded.
pub fn update_actor() -> RpcResult<ActorRef<update::UpdateActor>> {
    lookup::<update::UpdateActor>(update::UPDATE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "update is disabled", None::<()>),
        _ => e,
    })
}
