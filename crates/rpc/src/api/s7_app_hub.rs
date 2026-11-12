use app::{AppDoc, GetApp};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use rpc_core::{send, Server};
use app::rpc_support::apps;
use artifact::rpc_support::artifact;
use agent::rpc_support::RpcServerExt;

#[rpc(server, client)]
pub trait AppHubApi {
    /// A committed app record: its artifact, source, compatibility hash,
    /// pinned tool bindings and diagnostics (46 §6.3, §8.3). Rejects with
    /// `-32001` when the app plugin is disabled: the client falls back to
    /// the artifact's readable source. A private record needs its owning
    /// `session` (authorized, 51 §4.2).
    #[method(name = "get_app", with_extensions)]
    async fn get_app(
        &self,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc>;
}

#[async_trait]
impl AppHubApiServer for Server {

    async fn get_app(
        &self,
        ext: &Extensions,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc> {
        let apps = apps()?;
        self.authorize_opt(ext, session).await?;
        // The app plugin reads any store by call id: the record must be one this caller may see.
        send(
            &artifact()?,
            artifact::search::Get {
                call_id: record_call_id.clone(),
                session,
            },
        )
        .await?;
        send(&apps, GetApp { record_call_id }).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AppHubApiServer::into_rpc(server)
}
