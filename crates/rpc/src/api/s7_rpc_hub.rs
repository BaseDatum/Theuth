use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{peer, Server};
use crate::diag;

#[rpc(server, client)]
pub trait RpcHubApi {
    /// 176 C1: liveness only — answers `null` at once. Asks no actor and awaits nothing, so a
    /// busy engine still answers; probe with this, not `info`, to tell a dead socket from a
    /// slow handler.
    #[method(name = "ping")]
    fn ping(&self) -> RpcResult<()>;
    /// 176 C2: one client diagnostic line into the engine log, target `client`, fields
    /// `source` and `conn` (the `rpc: ws open conn=<n>` id; 0 off the local WebSocket).
    /// `level` `"warn"` logs at WARN, anything else at INFO; `source` is ≤32 chars of
    /// `[a-z0-9-]` (else logged as `-`); `message` is cut to 2,000 chars. 20 per minute per
    /// connection; the rest of that minute is dropped with one WARN. Asks no actor.
    #[method(name = "client_log", with_extensions)]
    fn client_log(&self, level: String, source: String, message: String) -> RpcResult<()>;
}

#[async_trait]
impl RpcHubApiServer for Server {

    fn ping(&self) -> RpcResult<()> {
        Ok(())
    }

    fn client_log(
        &self,
        ext: &Extensions,
        level: String,
        source: String,
        message: String,
    ) -> RpcResult<()> {
        let conn = ext.get::<rpc_core::ConnId>().map_or(0, |c| c.0);
        diag::client_log(&peer(ext)?, conn, &level, &source, &message);
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    RpcHubApiServer::into_rpc(server)
}
