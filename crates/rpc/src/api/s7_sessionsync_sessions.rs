use agent::rpc_forward as forward;
use agent::{Agents, ListSessions};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use sessionsync::types::{SessionOwner, SessionsStatus};
use rpc_core::{ask, Server};
use agent::rpc_support::RpcServerExt;

#[rpc(server, client)]
pub trait SessionsyncSessionsApi {
    /// Take `session` here (68 §4.2): `reason` is `user` (continue here, at
    /// once), `orphan` (a resumed checkout), `write` (owner unreachable ≥
    /// `sessions.takeover_secs`) or `moved` (181: the owner asked, see
    /// `session_move`). Returns the new owner/epoch;
    /// `SessionOwnerChanged` follows on the stream.
    #[method(name = "session_take", with_extensions)]
    async fn session_take(&self, session: SessionId, reason: String) -> RpcResult<SessionOwner>;
    /// This engine's replication summary (68 §10): id, sessions owned here
    /// / elsewhere, log head, last snapshot.
    #[method(name = "sessions_status")]
    async fn sessions_status(&self) -> RpcResult<SessionsStatus>;
}

#[async_trait]
impl SessionsyncSessionsApiServer for Server {

    async fn session_take(
        &self,
        ext: &Extensions,
        session: SessionId,
        reason: String,
    ) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let reason = sessionsync::TakeReason::parse(&reason).ok_or_else(|| {
            ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("reason {reason:?}: expected user | orphan | write | moved"),
                None::<()>,
            )
        })?;
        let (owner, epoch) = ask::<sessionsync::SessionSync, _>(
            sessionsync::SESSIONS_ACTOR,
            sessionsync::TakeSession {
                id: session,
                reason,
                // The explicit `reason` is the caller's intent (`user` resumes).
                by_user: false,
                // 181: `moved` counts only from the session's owner (the engine forwarding it).
                caller: ext.get::<forward::Forwarded>().map(|f| f.0.clone()),
            },
        )
        .await?;
        Ok(SessionOwner {
            owner,
            epoch,
            here: true,
        })
    }

    async fn sessions_status(&self) -> RpcResult<SessionsStatus> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let sessions = ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await?;
        let here = sessions.iter().filter(|s| s.here).count();
        Ok(SessionsStatus {
            engine: store.engine().to_owned(),
            here: u32::try_from(here).unwrap_or(u32::MAX),
            elsewhere: u32::try_from(sessions.len() - here).unwrap_or(u32::MAX),
            log_seq: u64::try_from(store.log_seq()).unwrap_or_default(),
            // ponytail: worker L owns snapshots; surface its timestamp here once it records one.
            last_snapshot: None,
        })
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    SessionsyncSessionsApiServer::into_rpc(server)
}
