use crate as agent;
use crate::{Agents, NewRootSession, PeerScope};
use jsonrpsee::core::RpcResult;
use runtime::SessionId;
use rpc_core::ask;

/// The owner console's root: `NewRootSession { scope: Owner }`, trusted
/// because the caller authenticated as the owner over the socket or is the
/// in-process TUI. Never promotes an existing root (50 §2.3).
pub async fn owner_root() -> RpcResult<SessionId> {
    ask::<Agents, _>(
        agent::AGENT_ACTOR,
        NewRootSession {
            cwd: None,
            scope: PeerScope::Owner,
        },
    )
    .await
}
