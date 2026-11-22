//! The profile's RPC methods (209 §4.4), owner audience like the credentials API. P1:
//! `web_profile_push` (tile → profile deltas) and `web_profile_sites` (the Settings list, counts
//! only). P2: `web_profile_pull` (profile → tile, the one method that answers values),
//! `web_profile_forget` (one site) and `web_profile_clear` (sign out everywhere); these three
//! also refuse a connection that is not the owner's (a share-link guest, an unauthenticated
//! call) before the actor sees it. None is forwarded: any engine reads and writes the replicated
//! table. With `browser.profile: off` push, sites and pull answer empty (`profile: "off"` in the
//! push and pull results); forget and clear still work (off leaves rows until cleared). An error
//! means the store is starting or unavailable, or the request was refused.

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{NOT_RUNNING, Server, lookup};

use crate::{
    Clear, ClearResult, CookieChange, Forget, ForgetResult, OriginStorage, ProfileError, Pull,
    PullResult, Push, PushResult, SiteSummary, Sites, WEBPROFILE_ACTOR, WRITER_TILE,
    WebProfileActor, caps,
};

/// Error code of `stale-epoch`, `unknown-epoch`, `profile full` and a refused request (the
/// ask-failed code of `rpc_core::failed`).
const FAILED: i32 = rpc_core::ASK_FAILED;

/// The error object of `e`: `stale-epoch` and `unknown-epoch` carry `{ "epoch": <current> }` as
/// data.
pub fn error(e: ProfileError) -> ErrorObjectOwned {
    match e {
        ProfileError::StaleEpoch { current } => ErrorObjectOwned::owned(
            FAILED,
            crate::STALE_EPOCH,
            Some(serde_json::json!({ "epoch": current })),
        ),
        ProfileError::UnknownEpoch { current } => ErrorObjectOwned::owned(
            FAILED,
            crate::UNKNOWN_EPOCH,
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

/// Ask the actor `msg`; a refusal becomes [`error`].
async fn ask<M, R>(msg: M) -> RpcResult<R>
where
    WebProfileActor: kameo::message::Message<M, Reply = Result<R, ProfileError>>,
    M: Send + 'static,
    R: Send + 'static,
{
    match actor()?.ask(msg).await {
        Ok(r) => Ok(r),
        Err(kameo::error::SendError::HandlerError(e)) => Err(error(e)),
        Err(e) => Err(rpc_core::failed(format!("webprofile actor: {e}"))),
    }
}

#[rpc_core_macros::rpc_descriptors(owner = "webprofile", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait WebProfileApi {
    /// 209 §4.4: the tile's cookie changes since the client's last push, and localStorage
    /// snapshots, from device `device` (cut to 64 characters) under profile epoch `epoch`. At most
    /// 2000 changes and 64 snapshots; each cookie needs a name, a path from `/`, a value of at most
    /// 4096 bytes and a domain that is not a public suffix (`invalid push: …` otherwise, nothing
    /// written). Refused with `stale-epoch` (data `{ epoch }`) when `epoch` is older than the
    /// current one (the client clears its store) and `unknown-epoch` when it is newer. With
    /// `browser.profile: off` nothing is written and the result says `profile: "off"`. Sets of a
    /// site forgotten less than 60 s ago are dropped (the client had not applied the forget yet).
    #[method(name = "web_profile_push")]
    async fn web_profile_push(
        &self,
        device: String,
        epoch: u64,
        changes: Vec<CookieChange>,
        storage: Option<Vec<OriginStorage>>,
    ) -> RpcResult<PushResult>;
    /// 209 §4.4: every live site with its cookie and origin counts, last write and writer. Never
    /// values; `[]` with `browser.profile: off`.
    #[method(name = "web_profile_sites")]
    async fn web_profile_sites(&self) -> RpcResult<Vec<SiteSummary>>;
    /// 209 §4.4, D5: the profile's cookies (every attribute, with each one's site, row writer and
    /// write time) and localStorage snapshots of `sites` (hosts or registrable domains, at most
    /// 500), or of every site without `sites`, for the client to set into its tile. Each site in
    /// the result's `sites` is answered whole, so the client deletes from its store that site's
    /// cookies it did not get (a forgotten site answers none), and skips cookies whose `writer` is
    /// its own `tile:<device>`. The only method that returns values: the owner's connections only,
    /// never logged. With `browser.profile: off` nothing is answered and `profile` is `"off"`.
    #[method(name = "web_profile_pull", with_extensions)]
    async fn web_profile_pull(&self, sites: Option<Vec<String>>) -> RpcResult<PullResult>;
    /// 209 §4.4, §6: forget one site (a host or registrable domain): its rows are tombstoned on
    /// every engine, its cookies leave every agent browser before that browser's next call, and a
    /// stale session's save or a client push made before the client applied the forget does not
    /// bring them back. Publishes `WebProfileChanged` naming the site. Owner connections only;
    /// works with `browser.profile: off`.
    #[method(name = "web_profile_forget", with_extensions)]
    async fn web_profile_forget(&self, site: String) -> RpcResult<ForgetResult>;
    /// 209 §4.4, §6: sign out everywhere: the epoch goes up by one and every row is tombstoned;
    /// every agent browser drops its profile cookies before its next call; a client learns the new
    /// epoch from `WebProfileChanged` or a `stale-epoch` push and clears its store. Owner
    /// connections only; works with `browser.profile: off`.
    #[method(name = "web_profile_clear", with_extensions)]
    async fn web_profile_clear(&self) -> RpcResult<ClearResult>;
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
        let device: String = device.chars().take(caps::DEVICE_CHARS).collect();
        ask(Push {
            writer: format!("{WRITER_TILE}{device}"),
            epoch,
            changes,
            storage: storage.unwrap_or_default(),
            basis_ms: None,
        })
        .await
    }

    async fn web_profile_sites(&self) -> RpcResult<Vec<SiteSummary>> {
        ask(Sites).await
    }

    async fn web_profile_pull(
        &self,
        ext: &Extensions,
        sites: Option<Vec<String>>,
    ) -> RpcResult<PullResult> {
        rpc_core::owner(ext)?;
        ask(Pull { sites }).await
    }

    async fn web_profile_forget(&self, ext: &Extensions, site: String) -> RpcResult<ForgetResult> {
        rpc_core::owner(ext)?;
        ask(Forget { site }).await
    }

    async fn web_profile_clear(&self, ext: &Extensions) -> RpcResult<ClearResult> {
        rpc_core::owner(ext)?;
        ask(Clear).await
    }
}

/// This plugin's RPC methods.
pub fn rpc_module(server: Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(WebProfileApiServer::into_rpc(server), rpc_descriptors())
        .expect("webprofile RPC descriptors")
}
