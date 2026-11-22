//! The profile's RPC methods (209 §4.4), owner audience like the credentials API. P1:
//! `web_profile_push` (tile → profile deltas) and `web_profile_sites` (the Settings list, counts
//! only). Neither is forwarded: any engine writes the replicated table.

use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{NOT_RUNNING, Server, lookup};

use crate::{
    CookieChange, OriginStorage, ProfileError, Push, PushResult, SiteSummary, Sites,
    WEBPROFILE_ACTOR, WRITER_TILE, WebProfileActor,
};

/// Error code of `stale-epoch` and `profile full` (the ask-failed code of `rpc_core::failed`).
const FAILED: i32 = rpc_core::ASK_FAILED;

/// The error object of `e`: `stale-epoch` carries `{ "epoch": <current> }` as data.
pub fn error(e: ProfileError) -> ErrorObjectOwned {
    match e {
        ProfileError::StaleEpoch { current } => ErrorObjectOwned::owned(
            FAILED,
            crate::STALE_EPOCH,
            Some(serde_json::json!({ "epoch": current })),
        ),
        other => ErrorObjectOwned::owned(FAILED, other.to_string(), None::<()>),
    }
}

fn actor() -> RpcResult<kameo::actor::ActorRef<WebProfileActor>> {
    lookup::<WebProfileActor>(WEBPROFILE_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "browser profile is off", None::<()>),
        _ => e,
    })
}

#[rpc_core_macros::rpc_descriptors(owner = "webprofile", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait WebProfileApi {
    /// 209 §4.4: the tile's cookie changes since the client's last push, and localStorage
    /// snapshots, from device `device` under profile epoch `epoch`. Refused with `stale-epoch`
    /// (data `{ epoch }`) when `epoch` is older than the current one: the client clears its store.
    #[method(name = "web_profile_push")]
    async fn web_profile_push(
        &self,
        device: String,
        epoch: u64,
        changes: Vec<CookieChange>,
        storage: Option<Vec<OriginStorage>>,
    ) -> RpcResult<PushResult>;
    /// 209 §4.4: every live site with its cookie and origin counts, last write and writer. Never values.
    #[method(name = "web_profile_sites")]
    async fn web_profile_sites(&self) -> RpcResult<Vec<SiteSummary>>;
}

#[async_trait]
impl WebProfileApiServer for Server {
    async fn web_profile_push(
        &self,
        device: String,
        epoch: u64,
        changes: Vec<CookieChange>,
        storage: Option<Vec<OriginStorage>>,
    ) -> RpcResult<PushResult> {
        let msg = Push {
            writer: format!("{WRITER_TILE}{device}"),
            epoch,
            changes,
            storage: storage.unwrap_or_default(),
        };
        match actor()?.ask(msg).await {
            Ok(r) => Ok(r),
            Err(kameo::error::SendError::HandlerError(e)) => Err(error(e)),
            Err(e) => Err(rpc_core::failed(format!("webprofile actor: {e}"))),
        }
    }

    async fn web_profile_sites(&self) -> RpcResult<Vec<SiteSummary>> {
        match actor()?.ask(Sites).await {
            Ok(r) => Ok(r),
            Err(kameo::error::SendError::HandlerError(e)) => Err(error(e)),
            Err(e) => Err(rpc_core::failed(format!("webprofile actor: {e}"))),
        }
    }
}

/// This plugin's RPC methods.
pub fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(WebProfileApiServer::into_rpc(server), rpc_descriptors())
        .expect("webprofile RPC descriptors")
}
