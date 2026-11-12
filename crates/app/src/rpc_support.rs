use crate as app;
use app::Apps;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The app actor; absent when `app.enabled: false` or the plugin is not loaded.
pub fn apps() -> RpcResult<ActorRef<Apps>> {
    lookup::<Apps>(app::APP_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "app is disabled", None::<()>),
        _ => e,
    })
}
