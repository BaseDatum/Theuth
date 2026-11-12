
pub use super::s7_artifact_artifacts::{ArtifactArtifactsApiClient as ArtifactsApiClient, ArtifactArtifactsApiServer as ArtifactsApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_agent_artifacts::rpc_module(server.clone()),
        super::s7_artifact_artifacts::rpc_module(server.clone()),
        super::s7_app_artifacts::rpc_module(server),
    ])
}
