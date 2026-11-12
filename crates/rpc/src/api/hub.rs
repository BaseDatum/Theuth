
pub use super::s7_rpc_compose_hub::{RpcComposeHubApiClient as CoreApiClient, RpcComposeHubApiServer as CoreApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_rpc_compose_hub::rpc_module(server.clone()),
        super::s7_a2a_hub::rpc_module(server.clone()),
        super::s7_agent_hub::rpc_module(server.clone()),
        super::s7_sessionsync_hub::rpc_module(server.clone()),
        super::s7_tasks_hub::rpc_module(server.clone()),
        super::s7_artifact_hub::rpc_module(server.clone()),
        super::s7_link_hub::rpc_module(server.clone()),
        super::s7_talk_hub::rpc_module(server.clone()),
        super::s7_search_providers_hub::rpc_module(server.clone()),
        super::s7_app_hub::rpc_module(server.clone()),
        super::s7_rpc_hub::rpc_module(server),
    ])
}
