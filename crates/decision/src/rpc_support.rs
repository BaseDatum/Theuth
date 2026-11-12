use crate as decision;
use jsonrpsee::core::RpcResult;
use rpc_core::{ask, Server};

/// Decision mode operations shared by dreams and meetings.
pub trait RpcDecisionExt {
    /// Current mode and checkpoint availability.
    fn decision_mode<'a>(&'a self, prefix: String) -> impl std::future::Future<Output = RpcResult<decision::ModeInfo>> + Send + 'a;
    /// Persist and apply a family mode.
    fn set_decision_mode<'a>(
        &'a self,
        prefix: String,
        mode: decision::Mode,
    ) -> impl std::future::Future<Output = RpcResult<decision::ModeInfo>> + Send + 'a;
}

impl RpcDecisionExt for Server {
    async fn decision_mode(&self, prefix: String) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::GetMode { prefix }).await
    }

    async fn set_decision_mode(
        &self,
        prefix: String,
        mode: decision::Mode,
    ) -> RpcResult<decision::ModeInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetMode { prefix, mode })
            .await
    }

}
