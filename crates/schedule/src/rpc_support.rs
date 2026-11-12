use crate as schedule;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The schedule actor; absent when `schedule.enabled: false` or the plugin is not loaded.
pub fn schedule() -> RpcResult<ActorRef<schedule::Schedule>> {
    lookup::<schedule::Schedule>(schedule::SCHEDULE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "schedule is disabled", None::<()>),
        _ => e,
    })
}
