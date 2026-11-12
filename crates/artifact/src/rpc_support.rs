use crate as artifact;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The artifact actor; absent when `artifact.enabled: false` or the plugin is not loaded.
pub fn artifact() -> RpcResult<ActorRef<artifact::ArtifactActor>> {
    lookup::<artifact::ArtifactActor>(artifact::ARTIFACT_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "artifact is disabled", None::<()>),
        _ => e,
    })
}
