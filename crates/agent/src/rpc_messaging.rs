use agent::rpc_forward as forward;
use agent::{Agents, AnswerPeerRequest, CommunicationState, GetCommunication, GetPeerMessage, ListPeerMessages, ListPeers, PeerMessagePage, PeerPage, PeerReceipt, PeerRecord, PeerScope, SetCommunication, SetPeerScope};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use uuid::Uuid;
use rpc_core::{ask, Server};
use agent::rpc_support::RpcServerExt;

#[rpc(server, client)]
pub trait AgentMessagingApi {
    /// Effective communication state (50 §10): the global switch alone with
    /// `None`, else `session`'s inherited/opted-out view, scope and alias.
    /// Authoritative: render it, never guess it.
    #[method(name = "get_communication", with_extensions)]
    async fn get_communication(&self, session: Option<SessionId>) -> RpcResult<CommunicationState>;
    /// `/communication on|off` for `session` (memory-only opt-out), or the
    /// persisted global ceiling with `None`. A session cannot override an
    /// ancestor or global off; the returned state says what blocks it.
    #[method(name = "set_communication", with_extensions)]
    async fn set_communication(
        &self,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState>;
    /// Owner-only root scope (`owner` | `isolated`) and alias (50 §2.3);
    /// `alias: None` keeps, `Some("")` clears. `Err` for a non-root.
    #[method(name = "set_peer_scope", with_extensions)]
    async fn set_peer_scope(
        &self,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState>;
    /// The directory `session` may address (50 §5.1), the same permission
    /// filter the model's `discover_peers` sees; bounded fields only.
    #[method(name = "list_peers", with_extensions)]
    async fn list_peers(
        &self,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage>;
    /// `session`'s own record of exchange `id` (50 §5.2); `Err` when it is
    /// neither sender nor recipient.
    #[method(name = "peer_message", with_extensions)]
    async fn peer_message(&self, session: SessionId, id: Uuid) -> RpcResult<PeerRecord>;
    /// `session`'s sent and received exchanges, newest first, with the exact
    /// ids a human may resolve now (`awaiting_answer` / `awaiting_reply`);
    /// `pending_only` restricts the rows to those. The durable snapshot a UI
    /// reloads after `PeerMessageChanged` or a reconnect.
    #[method(name = "list_peer_messages", with_extensions)]
    async fn list_peer_messages(
        &self,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage>;
    /// Resolve pending request `reply_to` on behalf of `session` (50 §5.3):
    /// as its recipient the answer travels to the sender as a peer message
    /// from `session`; as its blocked sender its own wait resolves. Either
    /// way `answered_by = owner:<session>`; never an arbitrary waiter.
    #[method(name = "answer_peer_request", with_extensions)]
    async fn answer_peer_request(
        &self,
        session: SessionId,
        reply_to: Uuid,
        text: String,
    ) -> RpcResult<PeerReceipt>;
    /// A peer message from `session` to `to` (`parent`, `session:<uuid>`,
    /// `peer:<name>`, a child's name), routed like the model's `send_message`
    /// tool without a wait; the MCP back-channel's `send_message` (75 §9).
    /// `Err` is a refused admission and nothing was sent.
    #[method(name = "send_message", with_extensions)]
    async fn send_message(
        &self,
        session: SessionId,
        to: String,
        text: String,
    ) -> RpcResult<PeerReceipt>;
}

#[async_trait]
impl AgentMessagingApiServer for Server {

    async fn get_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the owner's `agent.communication` decides.
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_communication", (session,)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session }).await
    }

    async fn set_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_communication", (session, enabled)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetCommunication { session, enabled }).await
    }

    async fn set_peer_scope(
        &self,
        ext: &Extensions,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_fwd(ext, root).await?;
        if let Some(r) = forward::route(ext, root, "set_peer_scope", (root, &scope, &alias)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetPeerScope { root, scope, alias }).await
    }

    async fn list_peers(
        &self,
        ext: &Extensions,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeers {
                session,
                query,
                cursor,
                limit,
                active_only: false,
                kind: None,
            },
        )
        .await
    }

    async fn peer_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: Uuid,
    ) -> RpcResult<PeerRecord> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, GetPeerMessage { session, id }).await
    }

    async fn list_peer_messages(
        &self,
        ext: &Extensions,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeerMessages {
                session,
                cursor,
                limit,
                pending_only,
            },
        )
        .await
    }

    async fn answer_peer_request(
        &self,
        ext: &Extensions,
        session: SessionId,
        reply_to: Uuid,
        text: String,
    ) -> RpcResult<PeerReceipt> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "answer_peer_request",
            (session, reply_to, &text),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AnswerPeerRequest {
                session,
                reply_to,
                text,
            },
        )
        .await
    }

    async fn send_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
        text: String,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        let d = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::supervisor::Deliver {
                from: session,
                to,
                text,
                await_reply: false,
                reply_to: None,
                request_id: None,
                admitted: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await?;
        Ok(d.receipt)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AgentMessagingApiServer::into_rpc(server)
}
