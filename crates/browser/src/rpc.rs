//! The browser's RPC methods (209 §4.4, P5), owner audience like the credentials API: the
//! `browser_show_element` card's answer and the cards still waiting. Both refuse a connection that
//! is not the owner's (a share-link guest, an unauthenticated call). A card another engine owns is
//! reached through the `session` the client names: the call is forwarded to that session's owner
//! (`agent::rpc_forward::route`), like `answer_web_view_call`. The answer's values are never
//! logged here; they go to the waiting tool and nowhere else ([`show_element::answer`]).

use agent::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, owner};
use runtime::SessionId;

use crate::show_element::{self, ElementFormAnswer, ElementFormId, ElementFormRequested};

/// Browser RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "browser", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait BrowserFormsApi {
    /// 209 D13, §4.4: the user's answer to an `ElementFormRequested` card: `{ submit: { values:
    /// { "<index>": String } } }` types the values into the page's real form and submits it,
    /// `{ cancel: {} }` closes the card. `true` while the card was waiting and this answer was
    /// taken (the first one wins). Owner connections only. `session` is the request's: a card
    /// another engine shows is answered there.
    #[method(name = "answer_element_form", with_extensions)]
    async fn answer_element_form(
        &self,
        id: ElementFormId,
        answer: ElementFormAnswer,
        session: Option<SessionId>,
    ) -> RpcResult<bool>;
    /// 209 D13, §4.4: the cards still waiting for an answer (`session`'s, forwarded to its owner,
    /// when given; else every one on this engine), for a client that attached late. Owner
    /// connections only.
    #[method(name = "element_forms_pending", with_extensions)]
    async fn element_forms_pending(
        &self,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<ElementFormRequested>>;
}

#[async_trait]
impl BrowserFormsApiServer for Server {
    async fn answer_element_form(
        &self,
        ext: &Extensions,
        id: ElementFormId,
        answer: ElementFormAnswer,
        session: Option<SessionId>,
    ) -> RpcResult<bool> {
        owner(ext)?;
        if !show_element::pending_here(&self.rt, id)
            && let Some(session) = session
            && let Some(taken) = forward::route(
                ext,
                session,
                "answer_element_form",
                (id, &answer, Some(session)),
            )
            .await?
        {
            return Ok(taken);
        }
        Ok(show_element::answer(&self.rt, id, answer))
    }

    async fn element_forms_pending(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<ElementFormRequested>> {
        owner(ext)?;
        if let Some(session) = session
            && let Some(forms) =
                forward::route(ext, session, "element_forms_pending", (Some(session),)).await?
        {
            return Ok(forms);
        }
        Ok(show_element::pending(&self.rt, session))
    }
}

/// Build this domain's RPC module.
#[must_use]
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(BrowserFormsApiServer::into_rpc(server), rpc_descriptors())
        .expect("browser RPC descriptors")
}
