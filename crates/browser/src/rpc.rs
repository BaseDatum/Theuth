//! The browser's RPC methods (209 §4.4, P5), owner audience like the credentials API: the
//! `browser_show_element` card's answer, the card on screen, the cards still waiting, and a tile
//! fill's values for the client that claimed it. All refuse a connection that is not the owner's
//! (a share-link guest, an unauthenticated call). A card another engine owns is reached through
//! the `session` the client names: the call is forwarded to that session's owner
//! (`agent::rpc_forward::route`), like `answer_web_view_call`. Values are never logged here; the
//! answer's go to the waiting tool and nowhere else ([`show_element::answer`]), a tile fill's to the
//! claiming connection only ([`show_element::take_values`]).

use std::collections::BTreeMap;

use agent::rpc_forward as forward;
use artifact::webview::{self, WebViewCallId};
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
    /// { "<index>": String } } }` types the values sent (only the controls the user changed) into
    /// the page's real form and submits it, `{ cancel: {} }` closes the card. `true` while the
    /// card was waiting and this answer was taken (the first one wins); a submit naming no field
    /// the card fills is refused (`false`) and the card stays. Owner connections only. `session`
    /// is the request's: a card another engine shows is answered there.
    #[method(name = "answer_element_form", with_extensions)]
    async fn answer_element_form(
        &self,
        id: ElementFormId,
        answer: ElementFormAnswer,
        session: Option<SessionId>,
    ) -> RpcResult<bool>;
    /// 209 D13, §4.10: a client renders card `id`. A card no client shows within 120 s ends as
    /// `cancelled`. `true` while the card waits. Owner connections only; `session` as in
    /// `answer_element_form`.
    #[method(name = "element_form_shown", with_extensions)]
    async fn element_form_shown(
        &self,
        id: ElementFormId,
        session: Option<SessionId>,
    ) -> RpcResult<bool>;
    /// 209 D13, §4.4: the values of the `fill_form` tile op `call_id` (a `WebViewCallRequested`
    /// whose `args.arguments.params.values_from` is `element_form_values`), once, to the
    /// connection whose `Claimed` the call took; `null` to anyone else, after the first fetch, or
    /// once the call is over. Owner connections only; `session` is the call's.
    #[method(name = "element_form_values", with_extensions)]
    async fn element_form_values(
        &self,
        call_id: WebViewCallId,
        session: Option<SessionId>,
    ) -> RpcResult<Option<BTreeMap<String, String>>>;
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

    async fn element_form_shown(
        &self,
        ext: &Extensions,
        id: ElementFormId,
        session: Option<SessionId>,
    ) -> RpcResult<bool> {
        owner(ext)?;
        if !show_element::pending_here(&self.rt, id)
            && let Some(session) = session
            && let Some(waits) =
                forward::route(ext, session, "element_form_shown", (id, Some(session))).await?
        {
            return Ok(waits);
        }
        Ok(show_element::shown(&self.rt, id))
    }

    async fn element_form_values(
        &self,
        ext: &Extensions,
        call_id: WebViewCallId,
        session: Option<SessionId>,
    ) -> RpcResult<Option<BTreeMap<String, String>>> {
        let peer = owner(ext)?;
        // The fetcher is this connection, or the engine a forwarded fetch came from (that engine
        // checked its own connection holds the claim), as `answer_web_view_call` names it.
        let by = match ext.get::<forward::Forwarded>() {
            Some(f) => format!("engine:{}", f.0.0),
            None => format!("conn:{}", peer.id()),
        };
        if show_element::values_here(&self.rt, call_id) {
            return Ok(show_element::take_values(&self.rt, call_id, &by));
        }
        let Some(session) = session.or_else(|| webview::session_of(call_id)) else {
            return Ok(None);
        };
        if !webview::relayed_claim_by(call_id, &by) {
            return Ok(None);
        }
        Ok(forward::route(
            ext,
            session,
            "element_form_values",
            (call_id, Some(session)),
        )
        .await?
        .flatten())
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
