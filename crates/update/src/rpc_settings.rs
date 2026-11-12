use crate as update;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{send, Server};
use update::rpc_support::update_actor;

/// Settings RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait UpdateSettingsApi {
    /// The self-updater's latest state: `null`, or the version that is on disk
    /// (`applied`) / available but not writable (39 §4.3).
    #[method(name = "update_status")]
    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>>;
}

#[async_trait]
impl UpdateSettingsApiServer for Server {

    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>> {
        send(&update_actor()?, update::Status).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    UpdateSettingsApiServer::into_rpc(server)
}
