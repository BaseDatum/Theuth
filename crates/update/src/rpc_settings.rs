use crate as update;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{failed, send, Server};
use update::rpc_support::update_actor;

/// Settings RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "update", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait UpdateSettingsApi {
    /// The self-updater's latest state: `null`, or the version that is on disk
    /// (`applied`) / available but not writable (39 §4.3).
    #[method(name = "update_status")]
    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>>;

    /// Check for a newer same-major release, verify and install it on disk, and
    /// restart a managed daemon (drain + exit 75) without waiting for the tick interval.
    #[method(name = "update_apply")]
    async fn update_apply(&self) -> RpcResult<update::UpdateApplyResult>;
}

#[async_trait]
impl UpdateSettingsApiServer for Server {
    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>> {
        send(&update_actor()?, update::Status).await
    }

    async fn update_apply(&self) -> RpcResult<update::UpdateApplyResult> {
        update::apply_now(&update_actor()?).await.map_err(failed)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(UpdateSettingsApiServer::into_rpc(server), rpc_descriptors())
        .expect("update RPC descriptors")
}
