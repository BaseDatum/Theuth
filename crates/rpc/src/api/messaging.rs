
pub use super::s7_agent_messaging::{AgentMessagingApiClient as MessagingApiClient, AgentMessagingApiServer as MessagingApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_agent_messaging::rpc_module(server.clone()),
        super::s7_signal_messaging::rpc_module(server.clone()),
        super::s7_slack_messaging::rpc_module(server.clone()),
        super::s7_telegram_messaging::rpc_module(server.clone()),
        super::s7_a2a_messaging::rpc_module(server),
    ])
}
