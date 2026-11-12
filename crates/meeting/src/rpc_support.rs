use crate as meeting;
use jsonrpsee::{Extensions, core::RpcResult, types::ErrorObjectOwned};
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};

/// The meeting actor; absent when `meeting.enabled: false`, off macOS, or the plugin is not loaded.
pub fn meeting() -> RpcResult<ActorRef<meeting::Meeting>> {
    lookup::<meeting::Meeting>(meeting::MEETING_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "meetings are disabled", None::<()>),
        _ => e,
    })
}

/// 158 §7.2: the connection's `x-theuth-client` value for companion-only methods; `None` when
/// the call did not come over the socket (the in-process TUI).
pub fn client_of(ext: &Extensions) -> Option<&'static str> {
    ext.get::<rpc_core::Client>().map(|c| c.0)
}
