
pub use super::s7_document_documents::{DocumentDocumentsApiClient as DocumentsApiClient, DocumentDocumentsApiServer as DocumentsApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_document_documents::rpc_module(server.clone()),
        super::s7_roster_documents::rpc_module(server.clone()),
        super::s7_agent_documents::rpc_module(server),
    ])
}
