use crate as mesh;
use jsonrpsee::core::RpcResult;
use kameo::actor::ActorRef;
use rpc_core::lookup;

/// The mesh actor; absent when the plugin is not loaded or its store failed (67 §4).
pub fn mesh_actor() -> RpcResult<ActorRef<mesh::Mesh>> {
    lookup::<mesh::Mesh>(mesh::MESH_ACTOR)
}
