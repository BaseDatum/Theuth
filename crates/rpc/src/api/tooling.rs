
pub use super::s7_tools_tooling::{ToolsToolingApiClient as ToolingApiClient, ToolsToolingApiServer as ToolingApiServer};
pub use super::s7_agent_tooling::SESSION_TOOLS;

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_tools_tooling::rpc_module(server.clone()),
        super::s7_agent_tooling::rpc_module(server),
    ])
}
