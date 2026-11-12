
pub use super::s7_agent_notebooks::{AgentNotebooksApiClient as NotebooksApiClient, AgentNotebooksApiServer as NotebooksApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_agent_notebooks::rpc_module(server.clone()),
        super::s7_document_notebooks::rpc_module(server),
    ])
}
