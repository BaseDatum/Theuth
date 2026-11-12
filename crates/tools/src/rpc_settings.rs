use crate as tools;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use tools::{SetPtyIdle, ToolRegistry};
use rpc_core::{ask, Server};

/// Settings RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "tools", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ToolsSettingsApi {
    /// Persist `tools.pty_idle_secs` (61 §1.6); `start` calls without `idle_secs` use it from then on. `0` = never.
    #[method(name = "set_pty_idle")]
    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()>;
}

#[async_trait]
impl ToolsSettingsApiServer for Server {

    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetPtyIdle { seconds }).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ToolsSettingsApiServer::into_rpc(server), rpc_descriptors())
        .expect("tools RPC descriptors")
}
