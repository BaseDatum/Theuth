use crate as agent;
use agent::rpc_forward as forward;
use agent::{Agents, Harvest};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use knowledge::HarvestResult;
use runtime::SessionId;
use rpc_core::{ask, Server};
use agent::rpc_support::RpcServerExt;

/// Concepts RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait AgentConceptsApi {
    /// Harvest `session`'s finished frame now (design doc 10 §5.2). Refused while a turn is running.
    #[method(name = "knowledge_harvest", with_extensions)]
    async fn knowledge_harvest(&self, session: SessionId) -> RpcResult<HarvestResult>;
}

#[async_trait]
impl AgentConceptsApiServer for Server {

    async fn knowledge_harvest(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<HarvestResult> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "knowledge_harvest", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Harvest { session }).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AgentConceptsApiServer::into_rpc(server)
}
