use crate as nodes;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The nodes actor; absent when `nodes.enabled: false` or the plugin is not loaded.
pub fn nodes() -> RpcResult<ActorRef<nodes::NodesActor>> {
    lookup::<nodes::NodesActor>(nodes::NODES_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "nodes is disabled", None::<()>),
        _ => e,
    })
}
