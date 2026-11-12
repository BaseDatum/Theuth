use crate as tools;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use tools::{ListTools, ToolRegistry, ToolSpec};
use rpc_core::{ask, Server};

/// Tooling RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "tools", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ToolsToolingApi {
    /// Every registered tool.
    #[method(name = "list_tools")]
    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>>;
}

#[async_trait]
impl ToolsToolingApiServer for Server {
    async fn list_tools(&self) -> RpcResult<Vec<ToolSpec>> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, ListTools(None)).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ToolsToolingApiServer::into_rpc(server), rpc_descriptors())
        .expect("tools RPC descriptors")
}
