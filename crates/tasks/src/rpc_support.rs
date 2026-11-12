use crate as tasks;
use agent::{Agents, rpc_forward as forward};
use kameo::actor::ActorRef;
use rpc_core::lookup;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;
use rpc_core::{ask, failed};

/// 139 §9.2 (D18): the engine owning a question's session, when it is another
/// one — read from the replicated `questions` row, so an answer typed on any
/// engine finds the actor holding the oneshot; 183 W-19: a card relayed from a
/// followed owner before its row replicated names its session too. `None`: no
/// row (a pre-0033 question or a non-journaled kind), or the session is
/// unknown / ours.
pub async fn question_owner(
    store: &agent::store::Store,
    id: &Uuid,
) -> RpcResult<Option<replica::PeerId>> {
    let session = match store
        .question_get(&id.to_string())
        .await
        .map_err(|e| failed(format!("{e:#}")))?
    {
        Some(row) => row.session,
        None => match sessionsync::events::relayed_session(*id) {
            Some(s) => s,
            None => return Ok(None),
        },
    };
    let Some((owner, _)) = store
        .owner(session)
        .await
        .map_err(|e| failed(format!("{e:#}")))?
    else {
        return Ok(None);
    };
    Ok((!owner.is_empty() && owner != store.engine()).then_some(replica::PeerId(owner)))
}

/// 139 §9.2: `answer_ask` / `resolve_approval` to the owner of the question's
/// session — forward, never take: an answer must reach the actor holding the
/// oneshot, and a take would kill it. `Ok(None)`: answer locally (lone engine,
/// already forwarded once, no row, or the owner is here). Owner not connected
/// → [`forward::OWNER_UNREACHABLE`], the card the desktop already renders.
pub async fn route_question<T: DeserializeOwned>(
    ext: &Extensions,
    id: &Uuid,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    if ext.get::<forward::Forwarded>().is_some() || sessionsync::actor().is_none() {
        return Ok(None);
    }
    let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    route_question_via(ext, &store, id, method, params).await
}

/// [`route_question`] past the plugin gate, over a given store (testable without actors).
pub async fn route_question_via<T: DeserializeOwned>(
    ext: &Extensions,
    store: &agent::store::Store,
    id: &Uuid,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Some(owner) = question_owner(store, id).await? else {
        return Ok(None);
    };
    match forward::to_peer(ext, &owner, method, params).await? {
        Some(v) => Ok(Some(v)),
        None => Err(forward::unreachable(owner.0, "not connected").await),
    }
}

/// The tasks actor; absent when the plugin is not loaded or its store failed (67 §7).
pub fn tasks_actor() -> RpcResult<ActorRef<tasks::Tasks>> {
    lookup::<tasks::Tasks>(tasks::TASKS_ACTOR)
}
