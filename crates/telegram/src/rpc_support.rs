//! Telegram actor lookup for RPC method owners.

use crate as telegram;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The telegram actor; absent when `telegram.enabled: false` or the plugin is not loaded.
pub fn telegram() -> RpcResult<ActorRef<telegram::Telegram>> {
    lookup::<telegram::Telegram>(telegram::TELEGRAM_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "telegram is disabled", None::<()>),
        _ => e,
    })
}
