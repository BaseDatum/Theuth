use crate as secrets;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The secrets actor; absent when the plugin is not loaded or the vault failed to open.
pub fn secrets() -> RpcResult<ActorRef<secrets::SecretsActor>> {
    lookup::<secrets::SecretsActor>(secrets::SECRETS_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => {
            ErrorObjectOwned::owned(NOT_RUNNING, "secrets vault unavailable", None::<()>)
        }
        _ => e,
    })
}
