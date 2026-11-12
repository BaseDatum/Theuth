//! Sandbox actor lookup for RPC handlers.

use crate as sandbox;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{NOT_RUNNING, lookup};

/// The sandbox actor; absent when `sandbox.enabled: false` or the plugin is not loaded (79).
pub fn sandbox() -> RpcResult<ActorRef<sandbox::SandboxActor>> {
    lookup::<sandbox::SandboxActor>(sandbox::SANDBOX_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "sandbox is disabled", None::<()>),
        _ => e,
    })
}
