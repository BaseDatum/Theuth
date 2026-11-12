use crate as agent;
use crate::{Agents, NewRootSession, PeerScope, Authorize, Authorized};
use crate::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use runtime::SessionId;
use rpc_core::{ask, peer, Server};

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

/// Actor authorization shared by RPC method owners.
pub trait RpcServerExt {
    /// Admit an authenticated peer to the named session.
    fn authorize<'a>(
        &'a self,
        ext: &'a Extensions,
        session: SessionId,
    ) -> impl std::future::Future<Output = RpcResult<Authorized>> + Send + 'a;
    /// Admit a session when one is named.
    fn authorize_opt<'a>(
        &'a self,
        ext: &'a Extensions,
        session: Option<SessionId>,
    ) -> impl std::future::Future<Output = RpcResult<()>> + Send + 'a;
    /// Admit a forwardable session, including a creator-owned pending row.
    fn authorize_fwd<'a>(
        &'a self,
        ext: &'a Extensions,
        session: SessionId,
    ) -> impl std::future::Future<Output = RpcResult<()>> + Send + 'a;
    /// Admit an optional forwardable session.
    fn authorize_opt_fwd<'a>(
        &'a self,
        ext: &'a Extensions,
        session: Option<SessionId>,
    ) -> impl std::future::Future<Output = RpcResult<()>> + Send + 'a;
}

impl RpcServerExt for Server {
    /// The one admission check (51 §4.2): the calling connection may act on
    /// `session`. Durable sessions admit any authenticated peer; a private
    /// one only its owner; unknown or closing ids fail closed.
    async fn authorize(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Authorized> {
        let peer = peer(ext)?.id();
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Authorize {
                session,
                peer: Some(peer),
            },
        )
        .await
    }

    /// `authorize` when a session is named; `None` is the caller's own root
    /// or the global scope, nothing to admit.
    async fn authorize_opt(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<()> {
        if let Some(s) = session {
            self.authorize(ext, s).await?;
        }
        Ok(())
    }

    /// [`Self::authorize`] for a handler that forwards (`forward::route`,
    /// `forward::to_creator`): 183 §5.6, a chat another engine just created
    /// for us whose row is not here yet is admitted (durable chats only, see
    /// `create_session`), since the call goes to that engine, which admits
    /// it. Every other handler keeps the strict check and fails closed.
    async fn authorize_fwd(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<()> {
        peer(ext)?;
        if let Some(forward::Expected { here: false, .. }) = forward::expected(ext, session).await?
        {
            return Ok(());
        }
        self.authorize(ext, session).await.map(drop)
    }

    /// [`Self::authorize_fwd`] when a session is named.
    async fn authorize_opt_fwd(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<()> {
        if let Some(s) = session {
            self.authorize_fwd(ext, s).await?;
        }
        Ok(())
    }

}

