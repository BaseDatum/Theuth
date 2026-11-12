
pub use super::s7_knowledge_concepts::{KnowledgeConceptsApiClient as ConceptsApiClient, KnowledgeConceptsApiServer as ConceptsApiServer};

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        super::s7_knowledge_concepts::rpc_module(server.clone()),
        super::s7_agent_concepts::rpc_module(server),
    ])
}
