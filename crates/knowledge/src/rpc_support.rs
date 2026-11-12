use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use crate::{KNOWLEDGE_ACTOR, KnowledgeActor};
use rpc_core::{lookup, NOT_RUNNING};

/// The knowledge actor; `knowledge.enabled: false` means it was never
/// spawned (12 §9.3).
pub fn knowledge() -> RpcResult<ActorRef<KnowledgeActor>> {
    lookup::<KnowledgeActor>(KNOWLEDGE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "knowledge is disabled", None::<()>),
        _ => e,
    })
}
