
pub use super::s7_agent_settings::{AgentSettingsApiClient as SettingsApiClient, AgentSettingsApiServer as SettingsApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_agent_settings::rpc_module(server.clone()),
        super::s7_tools_settings::rpc_module(server.clone()),
        super::s7_update_settings::rpc_module(server.clone()),
        super::s7_rpc_compose_settings::rpc_module(server.clone()),
        super::s7_web_settings::rpc_module(server),
    ])
}
