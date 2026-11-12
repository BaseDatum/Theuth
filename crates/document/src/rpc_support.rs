//! Document actor lookup, hub refusals and guest membership admission for RPC calls.

use crate as document;
use std::fmt::Display;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::Message;
use kameo::reply::Reply;
use rpc_core::{lookup, failed, peer, NOT_RUNNING};
use base64::Engine as _;

/// The document principal behind a call (59 §8.3): a mode-2 guest conn is `Member{pub, device}`,
/// everything else the owner on `device`. Returned with the peer so the caller can mark a guest
/// joined or refused.
pub fn principal(ext: &Extensions) -> RpcResult<(rpc_core::RpcPeer, document::Principal)> {
    let p = peer(ext)?;
    let device = Some(p.id());
    let by = match p.guest() {
        Some(g) => document::Principal::Member {
            pub_: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(g.key),
            device,
        },
        None => document::Principal::Owner { device },
    };
    Ok((p, by))
}

/// A member call's outcome as the guest's conn sees it (59 §8.3, §11): success scopes the conn
/// (no `doc_join` needed on a return visit), a membership or role refusal counts toward the close.
pub fn member_outcome<T>(peer: &rpc_core::RpcPeer, r: &RpcResult<T>) {
    let Some(g) = peer.guest() else {
        return;
    };
    match r {
        Ok(_) => g.mark_joined(),
        Err(e)
            if e.message().starts_with("not_member")
                || e.message().starts_with("viewer_readonly")
                || e.message().starts_with("sharing_disabled") =>
        {
            g.refuse();
        }
        Err(_) => {}
    }
}

/// The document actor; absent when `documents.enabled: false` or the plugin is not loaded.
pub fn documents() -> RpcResult<ActorRef<document::DocumentActor>> {
    lookup::<document::DocumentActor>(document::DOCUMENT_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "documents are disabled", None::<()>),
        _ => e,
    })
}

/// JSON-RPC error code: this engine does not hold the hub lease, and the document call is a hub duty
/// (69 §5). `data` is a `document::DocError` in serde's external enum form:
/// `{"NotHub": {"hub": "<engine id>" | null}}` — Y's forwarding keys on it.
pub const NOT_HUB: i32 = -32011;

/// [`send`] for the document actor: a `NotHub` refusal (typed, or in its `Display` form from a
/// handler whose error stayed a `String`) becomes [`NOT_HUB`] with the typed payload; anything else
/// is `ASK_FAILED` with the message.
pub async fn doc_send<M>(
    actor: &ActorRef<document::DocumentActor>,
    msg: M,
) -> RpcResult<<<document::DocumentActor as Message<M>>::Reply as Reply>::Ok>
where
    document::DocumentActor: Message<M>,
    M: Send + 'static,
    <<document::DocumentActor as Message<M>>::Reply as Reply>::Error: Display,
{
    actor.ask(msg).await.map_err(|e: SendError<M, _>| match e {
        SendError::HandlerError(e) => match document::DocError::parse(&e.to_string()) {
            err @ document::DocError::NotHub { .. } => {
                ErrorObjectOwned::owned(NOT_HUB, "another engine holds the hub lease", Some(err))
            }
            document::DocError::Other(msg) => failed(msg),
        },
        other => failed(other.to_string()),
    })
}

/// `doc_join`'s outcome on the guest's conn (59 §8.3): success scopes it; only a refusal counts
/// toward its close — the join's own (`JoinError`'s wire codes) and a member call's (Z184-R2/R3):
/// a retryable `busy` (184 §4.3 step 4), a `hub_moved` during a Move or any transient error never
/// locks a guest out.
pub fn join_outcome<T>(guest: &rpc_core::Guest, r: &RpcResult<T>) {
    const REFUSALS: [&str; 10] = [
        "not_member",
        "viewer_readonly",
        "sharing_disabled",
        "bad_proof",
        "link_unknown",
        "link_revoked",
        "link_expired",
        "members_full",
        "sharing_off",
        "member_revoked",
    ];
    match r {
        Ok(_) => guest.mark_joined(),
        Err(e) if REFUSALS.iter().any(|r| e.message().starts_with(r)) => {
            guest.refuse();
        }
        Err(_) => {}
    }
}

/// [`doc_send`]'s error mapping for the 144 delegation functions that answer `anyhow::Result`: a
/// `NotHub:<id>` message becomes [`NOT_HUB`] with the typed payload (so [`doc_forward`] re-issues the
/// call), anything else `ASK_FAILED` with the message.
pub fn nb_result<T>(r: anyhow::Result<T>) -> RpcResult<T> {
    r.map_err(|e| match document::DocError::parse(&format!("{e:#}")) {
        err @ document::DocError::NotHub { .. } => {
            ErrorObjectOwned::owned(NOT_HUB, "another engine holds the hub lease", Some(err))
        }
        document::DocError::Other(msg) => failed(msg),
    })
}
