
pub use super::s7_agent_sessions::{AgentSessionsApiClient as SessionsApiClient, AgentSessionsApiServer as SessionsApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_agent_sessions::rpc_module(server.clone()),
        super::s7_sessionsync_sessions::rpc_module(server),
    ])
}
