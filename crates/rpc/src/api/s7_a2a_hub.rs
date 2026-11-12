use agent::rpc_forward as forward;
use agent::{Agents, ArchiveSession};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use rpc_core::{ask, ASK_FAILED, UNAUTHORIZED, Server};
use a2a::rpc_support::{A2A_FORWARD_MESSAGING_OFF, a2a};
use agent::rpc_support::RpcServerExt;

#[rpc(server, client)]
pub trait A2aHubApi {
    /// 191 (D3): a contact's unary call (`POST path` with the JSON-RPC `body`) that another
    /// machine of the mesh received for a session owned here: this machine checks `caller`
    /// itself and answers it in its own a2a router. Mesh-internal like `peer_deliver`: a call
    /// without the `Forwarded` marker is `-32002`. `NOT_OWNER` (-32013) when the session is
    /// not owned here either, `OWNER_UNREACHABLE` (-32010) before `contacts_ready`; every
    /// outcome of the router is `{status, body}`.
    #[method(name = "a2a_forward", with_extensions)]
    async fn a2a_forward(
        &self,
        caller: String,
        path: String,
        body: String,
    ) -> RpcResult<agent::communication::mesh::A2aAnswer>;
    /// Archive or unarchive a root; archived trees leave the sidebar but stay
    /// in `list_sessions` (flagged) and search (51 §3).
    #[method(name = "archive_session", with_extensions)]
    async fn archive_session(&self, session: SessionId, archived: bool) -> RpcResult<()>;
}

#[async_trait]
impl A2aHubApiServer for Server {

    async fn a2a_forward(
        &self,
        ext: &Extensions,
        caller: String,
        path: String,
        body: String,
    ) -> RpcResult<agent::communication::mesh::A2aAnswer> {
        use agent::communication::mesh::ForwardFailure;
        let Some(forward::Forwarded(peer)) = ext.get::<forward::Forwarded>() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "a2a_forward is mesh-internal",
                None::<()>,
            ));
        };
        // 183 §6.2 (W-14), 191: `mesh.messaging_off` refuses the forwarder's carried calls as
        // it refused its deliveries; the contact hears words that name no machine.
        if mesh::messaging_off(&peer.0) {
            tracing::info!(forwarder = %peer.0, "191: a2a_forward refused: messaging with this machine is off");
            return Err(ErrorObjectOwned::owned(
                ASK_FAILED,
                A2A_FORWARD_MESSAGING_OFF,
                None::<()>,
            ));
        }
        // Off the a2a mailbox: the plugin's published context, never an actor ask.
        a2a::contacts::forward::serve(&peer.0, &caller, &path, body)
            .await
            .map_err(|e| match e {
                ForwardFailure::NotOwner => ErrorObjectOwned::owned(
                    forward::NOT_OWNER,
                    agent::communication::network::NOT_OWNER_TEXT,
                    None::<()>,
                ),
                ForwardFailure::Offline(why) => {
                    ErrorObjectOwned::owned(forward::OWNER_UNREACHABLE, why, None::<()>)
                }
                ForwardFailure::Refused(why) => {
                    ErrorObjectOwned::owned(ASK_FAILED, why, None::<()>)
                }
            })
    }

    async fn archive_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        archived: bool,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        match forward::route(ext, session, "archive_session", (session, archived)).await? {
            Some(r) => r,
            None => {
                ask::<Agents, _>(agent::AGENT_ACTOR, ArchiveSession { session, archived }).await?
            }
        }
        // 183 W-41, §12.10: an exported chat's export row reads Paused while it is archived;
        // this engine holds the export (contacts are local), so `A2aChanged` fires here.
        if let Ok(Some(actor)) = a2a(&self.rt) {
            let _ = actor.ask(a2a::contacts::ChatArchived { session }).await;
        }
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    A2aHubApiServer::into_rpc(server)
}
