use agent::rpc_support::RpcServerExt;
use browser::{BROWSER_ACTOR, Browsers, Screenshot};
use computer::{COMPUTER_ACTOR, Computer, Permissions, ScreenContext};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{NOT_RUNNING, Server, ask, failed, lookup, peer, send};
use runtime::SessionId;

#[rpc(server, client)]
pub trait ScreensApi {
    /// Current page of `session`'s embedded browser as a base64 PNG for a live
    /// view (23 §3.2); `None` when the session has no live isolate or no open
    /// page. Polling never spawns an isolate.
    #[method(name = "browser_screenshot", with_extensions)]
    async fn browser_screenshot(&self, session: SessionId) -> RpcResult<Option<String>>;
    /// Computer-control permissions and attached displays (design doc 27 §6);
    /// errors with "actor not running" when `computer.enabled` is false.
    #[method(name = "computer_permissions")]
    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus>;
    /// Capture the displays a pointer path visited, draw the path, read the
    /// accessibility tree under it, write the images under
    /// `<data_dir>/attachments/`, answer the `[screen …]` lines (design doc
    /// 96 §6.3). Local peers only; "disabled" when `computer.enabled` is
    /// false; a Screen Recording denial names the process.
    #[method(name = "screen_context", with_extensions)]
    async fn screen_context(
        &self,
        req: computer::ScreenContextReq,
    ) -> RpcResult<computer::ScreenContextReply>;
}

#[async_trait]
impl ScreensApiServer for Server {

    async fn browser_screenshot(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Option<String>> {
        self.authorize(ext, session).await?;
        ask::<Browsers, _>(BROWSER_ACTOR, Screenshot { session }).await
    }

    async fn computer_permissions(&self) -> RpcResult<computer::PermissionStatus> {
        ask::<Computer, _>(COMPUTER_ACTOR, Permissions).await
    }

    async fn screen_context(
        &self,
        ext: &Extensions,
        req: computer::ScreenContextReq,
    ) -> RpcResult<computer::ScreenContextReply> {
        if peer(ext)?.remote() {
            return Err(failed("the screen is on the harness host"));
        }
        let actor = lookup::<Computer>(COMPUTER_ACTOR).map_err(|e| match e.code() {
            NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "disabled", None::<()>),
            _ => e,
        })?;
        send(&actor, ScreenContext(req)).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    ScreensApiServer::into_rpc(server)
}
