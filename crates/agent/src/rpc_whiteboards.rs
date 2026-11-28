//! 218 §4.2: the agent side of whiteboards — which sessions a whiteboard is bound to, and the send
//! context the composer hands over just before `send_input`/`steer`.
use crate as agent;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{owner, Server};
use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// One session bound to a whiteboard (218 §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WhiteboardSession {
    /// The root session.
    pub session: SessionId,
    /// Its title (empty when it has none yet).
    pub title: String,
    /// When it was (last) bound, ms since the epoch.
    pub bound_ms: i64,
}

/// What the owner's send carried (218 §4.2): rendered into the next `<whiteboard_changes>` note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WbSendContext {
    /// ULIDs: frames fully in view, then loose cards in view (≤ 12).
    #[serde(default)]
    pub in_view: Vec<String>,
    /// ULIDs of the selected items.
    #[serde(default)]
    pub selected: Vec<String>,
    /// In the order of the message's image parts.
    #[serde(default)]
    pub images: Vec<WbSentImage>,
    /// Images left out.
    #[serde(default)]
    pub omitted: u32,
    /// `"no vision"` | `"budget"`.
    #[serde(default)]
    pub omitted_why: Option<String>,
}

/// One image part of a whiteboard send: the card it shows (`None` = a free mark, "canvas") and its marks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WbSentImage {
    /// Card ULID, or `None` for a free mark.
    pub card: Option<String>,
    /// Mark ULIDs.
    pub marks: Vec<String>,
}

/// Whiteboard RPC methods owned by the agent plugin (218 §4.2).
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentWhiteboardsApi {
    /// Bind `session` (its root) to `whiteboard` (upsert). Checks the document is a whiteboard. Owner only.
    #[method(name = "whiteboard_bind", with_extensions)]
    async fn whiteboard_bind(&self, session: SessionId, whiteboard: document::DocId) -> RpcResult<()>;
    /// Unbind `session` (its root) from `whiteboard`; idempotent. Owner only.
    #[method(name = "whiteboard_unbind", with_extensions)]
    async fn whiteboard_unbind(&self, session: SessionId, whiteboard: document::DocId) -> RpcResult<()>;
    /// Sessions bound to `whiteboard` on this engine, newest binding first. Owner only.
    #[method(name = "whiteboard_sessions", with_extensions)]
    async fn whiteboard_sessions(&self, whiteboard: document::DocId) -> RpcResult<Vec<WhiteboardSession>>;
    /// Whiteboards bound to `session` (resolved to its root), newest binding first. Owner only.
    #[method(name = "session_whiteboards", with_extensions)]
    async fn session_whiteboards(&self, session: SessionId) -> RpcResult<Vec<document::DocId>>;
    /// Hand over what the next send carries (in view, selected, image parts); consumed by the next turn
    /// of `session` (any session when `None`) for `whiteboard`, dropped after 120 s. Call it right before
    /// `send_input`/`steer`. Owner only.
    #[method(name = "whiteboard_send_context", with_extensions)]
    async fn whiteboard_send_context(
        &self,
        session: Option<SessionId>,
        whiteboard: document::DocId,
        send: WbSendContext,
    ) -> RpcResult<()>;
    /// Hand over the live Task Graph view context (active project, selected task, visible tasks)
    /// from the floating Task Graph composer; consumed by the next turn of `session`, dropped after
    /// 120 s. Call it right before `send_input`/`steer`. Owner only.
    #[method(name = "task_graph_send_context", with_extensions)]
    async fn task_graph_send_context(
        &self,
        session: SessionId,
        #[argument(rename = "context")] ctx: agent::task_graph::TaskGraphSendContext,
    ) -> RpcResult<()>;
}

#[async_trait]
impl AgentWhiteboardsApiServer for Server {
    async fn whiteboard_bind(
        &self,
        ext: &Extensions,
        session: SessionId,
        whiteboard: document::DocId,
    ) -> RpcResult<()> {
        owner(ext)?;
        agent::whiteboards::rpc::bind(session, whiteboard).await
    }

    async fn whiteboard_unbind(
        &self,
        ext: &Extensions,
        session: SessionId,
        whiteboard: document::DocId,
    ) -> RpcResult<()> {
        owner(ext)?;
        agent::whiteboards::rpc::unbind(session, whiteboard).await
    }

    async fn whiteboard_sessions(
        &self,
        ext: &Extensions,
        whiteboard: document::DocId,
    ) -> RpcResult<Vec<WhiteboardSession>> {
        owner(ext)?;
        agent::whiteboards::rpc::sessions(whiteboard).await
    }

    async fn session_whiteboards(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<document::DocId>> {
        owner(ext)?;
        agent::whiteboards::rpc::of_session(session).await
    }

    async fn whiteboard_send_context(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        whiteboard: document::DocId,
        send: WbSendContext,
    ) -> RpcResult<()> {
        owner(ext)?;
        agent::whiteboards::rpc::send_context(session, whiteboard, send).await
    }

    async fn task_graph_send_context(
        &self,
        ext: &Extensions,
        session: SessionId,
        ctx: agent::task_graph::TaskGraphSendContext,
    ) -> RpcResult<()> {
        owner(ext)?;
        agent::task_graph::put_context(session, ctx);
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentWhiteboardsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
