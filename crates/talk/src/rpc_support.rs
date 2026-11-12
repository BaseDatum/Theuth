//! Actor and history helpers shared by speech RPC and the remaining hub methods.

use crate as talk;
use jsonrpsee::{Extensions, core::RpcResult, types::ErrorObjectOwned};
use kameo::actor::ActorRef;
use rpc_core::{lookup, peer, send, failed, NOT_RUNNING};

/// The voice actor; absent when `voice.enabled: false` or the plugin is not loaded.
pub fn voice() -> RpcResult<ActorRef<voice::Voice>> {
    lookup::<voice::Voice>(voice::VOICE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The STT actor; absent when `voice.enabled: false` or the plugin is not loaded.
pub fn transcriber() -> RpcResult<ActorRef<voice::Transcriber>> {
    lookup::<voice::Transcriber>(voice::TRANSCRIBER_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The dictation actor; absent when `voice.enabled: false` or the plugin is not loaded.
pub fn dictation() -> RpcResult<ActorRef<voice::Dictation>> {
    lookup::<voice::Dictation>(voice::DICTATION_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "voice is disabled", None::<()>),
        _ => e,
    })
}

/// The 153 history store (§6.5): local peers only, like the daemon mic
/// (`dictation_start`); `ASK_FAILED` "history disabled" when it is off.
pub async fn history_store(ext: &Extensions) -> RpcResult<voice::Store> {
    if peer(ext)?.remote() {
        return Err(failed("dictation history is on the harness host"));
    }
    send(&dictation()?, voice::HistoryStore)
        .await?
        .ok_or_else(|| failed("history disabled"))
}

/// The talk actor; absent when `talk.enabled: false` or the plugin is not loaded.
pub fn talk() -> RpcResult<ActorRef<talk::Talk>> {
    lookup::<talk::Talk>(talk::TALK_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "talk is disabled", None::<()>),
        _ => e,
    })
}
