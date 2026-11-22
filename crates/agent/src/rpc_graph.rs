//! 208: the Attention graph RPC (`agent_graph`).

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, ask};
use runtime::SessionId;

use crate::graph::{GRAPH_ACTOR, Graph, Snapshot};

/// Attention graph RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentGraphApi {
    /// 208 §4: every active and recently active session tree and the peer messages between them, in one reply.
    /// `window_ms` null = 24 h (clamped to 7 d); `include`: sessions to show regardless of the window (with their
    /// ancestors; at most 200); a current `cursor` answers `unchanged: true` with empty lists. Each call keeps the
    /// `agent.graph` delta stream alive for 300 s.
    #[method(name = "agent_graph", with_extensions)]
    async fn agent_graph(
        &self,
        window_ms: Option<u64>,
        include: Vec<SessionId>,
        cursor: Option<crate::GraphCursor>,
    ) -> RpcResult<crate::AgentGraph>;
}

#[async_trait]
impl AgentGraphApiServer for Server {
    async fn agent_graph(
        &self,
        _ext: &Extensions,
        window_ms: Option<u64>,
        include: Vec<SessionId>,
        cursor: Option<crate::GraphCursor>,
    ) -> RpcResult<crate::AgentGraph> {
        ask::<Graph, _>(
            GRAPH_ACTOR,
            Snapshot {
                window_ms,
                include,
                cursor,
            },
        )
        .await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentGraphApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
