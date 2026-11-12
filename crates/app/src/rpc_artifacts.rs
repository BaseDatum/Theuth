use crate as app;
use app::{AppDoc, AppToolResult, CallArgs, CallStatusOf, CallTool, CloseApp, PublishFromUi};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use rpc_core::{send, peer, Server};
use app::rpc_support::apps;
use agent::rpc_support::RpcServerExt;

/// Artifacts RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait AppArtifactsApi {
    /// Publish a reply fence's OpenUI source as an app artifact of `session` without a
    /// model turn (30 §9): validated and committed exactly like the `app` tool with
    /// `edit: false`, under a synthetic `ui-<uuid v7>` call id, no transcript entry. The
    /// error message is the tool's located diagnostics text. `session` is
    /// authorized: a private tree takes apps from its owner only.
    #[method(name = "publish_app", with_extensions)]
    async fn publish_app(
        &self,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc>;
    /// Run one pinned Query/Mutation binding of an open app view on behalf of
    /// the calling connection (46 §6). A `press` trigger is the approval; the
    /// tool name comes from the binding, never the client. `-32002` when no
    /// authenticated connection carries the call.
    #[method(name = "app_call_tool", with_extensions)]
    async fn app_call_tool(&self, args: CallArgs) -> RpcResult<AppToolResult>;
    /// Status (and stored result) of a pressed invocation by `request_id`,
    /// for reconnect and duplicate-press resolution (46 §6.4).
    #[method(name = "app_call_status", with_extensions)]
    async fn app_call_status(&self, request_id: String) -> RpcResult<AppToolResult>;
    /// Close one open view: stop its reads, drop queued work (46 §7). Idempotent.
    #[method(name = "app_close", with_extensions)]
    async fn app_close(&self, instance_id: String) -> RpcResult<()>;
}

#[async_trait]
impl AppArtifactsApiServer for Server {

    async fn publish_app(
        &self,
        ext: &Extensions,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc> {
        self.authorize(ext, session).await?;
        send(
            &apps()?,
            PublishFromUi {
                session,
                name,
                source,
            },
        )
        .await
    }

    async fn app_call_tool(&self, ext: &Extensions, args: CallArgs) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallTool { peer, args }).await
    }

    async fn app_call_status(
        &self,
        ext: &Extensions,
        request_id: String,
    ) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallStatusOf { peer, request_id }).await
    }

    async fn app_close(&self, ext: &Extensions, instance_id: String) -> RpcResult<()> {
        let peer = peer(ext)?;
        send(&apps()?, CloseApp { peer, instance_id }).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AppArtifactsApiServer::into_rpc(server)
}
