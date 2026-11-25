//! Agent-owned document forwarding, notebook decoration and insight startup hooks.

use crate as agent;
use std::sync::Arc;
use agent::{Agents, rpc_forward as forward};
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use kameo::actor::ActorRef;
use runtime::Runtime;
use serde::de::DeserializeOwned;
use serde::Serialize;
use rpc_core::lookup;
use document::rpc_support::{NOT_HUB, doc_send};

/// 69 Y: a hub-only owner call this engine refused with `NotHub{hub}` is re-issued verbatim to `hub`
/// as one unary JSON-RPC over `theuth/rpc/1` (`forward::to_peer`; request/reply, so no stream). The
/// hub dispatches it as its internal peer: these are owner-only or owner-side methods, so the
/// principal is right there. `params` is the positional list in trait order. `hub: null`, no
/// transport, or a hub not connected → the refusal stands (the client sees `-32011`).
pub async fn doc_forward<T: DeserializeOwned>(
    ext: &Extensions,
    method: &str,
    params: impl Serialize,
    r: RpcResult<T>,
) -> RpcResult<T> {
    let Err(e) = &r else { return r };
    if e.code() != NOT_HUB {
        return r;
    }
    let hub = e
        .data()
        .and_then(|d| serde_json::from_str::<document::DocError>(d.get()).ok())
        .and_then(|d| d.not_hub().cloned().flatten());
    let Some(hub) = hub else { return r };
    match forward::to_peer(ext, &replica::PeerId(hub.0), method, params).await? {
        Some(v) => Ok(v),
        None => r,
    }
}

/// 184 §4.8, §7.2: an owner call on `doc` re-issued to the engine that serves it (`DocServer`), so
/// that engine checks its own state (its Link, the home). `Ok(None)`: run it here — this engine
/// serves it or nobody does, the server is not connected, or this call was itself forwarded.
pub async fn to_doc_server<T: DeserializeOwned>(
    ext: &Extensions,
    docs: &ActorRef<document::DocumentActor>,
    doc: &document::DocId,
    method: &str,
    params: impl Serialize,
) -> RpcResult<Option<T>> {
    let Ok(Some(server)) = doc_send(docs, document::DocServer { doc: doc.clone() }).await else {
        return Ok(None);
    };
    forward::to_peer(ext, &replica::PeerId(server.0), method, params).await
}

/// 144 §4.3 (E1): the insight job's model call and skill lookup live in `agent`; the document crate
/// cannot name the agent actor, so this crate — which knows both ends — installs the two closures once
/// (the pattern of `codeindex::map::generator::install_side_caller`). Looked up per call.
pub fn install_insight_hooks(rt: &Runtime) {
    use document::ingest::insights::{Hooks, InsightReply, install};
    let dir = agent::skills::dir(&rt.data_dir);
    install(Hooks {
        side_call: Arc::new(|call| {
            Box::pin(async move {
                let agents =
                    lookup::<Agents>(agent::AGENT_ACTOR).map_err(|e| e.message().to_owned())?;
                let notebooks: Vec<String> =
                    call.notebooks.iter().map(ToString::to_string).collect();
                agent::notebook_context::insight_side_call(
                    &agents,
                    call.session,
                    &notebooks,
                    call.model,
                    call.system,
                    call.user,
                    call.budget,
                )
                .await
                .map(|r| InsightReply {
                    text: r.text,
                    model: r.model,
                })
            })
        }),
        skill: Arc::new(move |name| agent::notebook_context::insight_skill(&dir, name)),
    });
    // 218 §5 (c): `wb_*` writes, `wb_create` and `show_whiteboard` bind their session.
    agent::whiteboards::install_hooks();
}

/// A `ContextLevel` as its wire word (`off | mention | summary | full`) — what the agent's store keeps.
pub fn level_str(level: document::ContextLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// 144 §8 (W3 R1): `Notebook.chats` from the agent store (`sessions.notebook`); the document crate holds no
/// session rows. Best effort — no agent actor or a store error leaves the count at 0.
pub async fn notebook_chat_counts() -> std::collections::HashMap<String, u32> {
    match lookup::<Agents>(agent::AGENT_ACTOR) {
        Ok(agents) => agent::notebooks::chat_counts(&agents)
            .await
            .unwrap_or_default(),
        Err(_) => Default::default(),
    }
}

/// 144 §6.4 (W3 R1): the document crate answers `same`/available for `theuth://sessions/<id>#<turn>` unchecked;
/// the agent store decides — no such (readable) session → `available: false`, a turn past the end → `missing`;
/// the title becomes the session's display name. Best effort when the agent actor is not running.
pub async fn check_session_cites(cites: &mut [document::Citation]) {
    let Ok(agents) = lookup::<Agents>(agent::AGENT_ACTOR) else {
        return;
    };
    for c in cites
        .iter_mut()
        .filter(|c| c.kind == document::CiteKind::Sessions)
    {
        let Ok(anchor) = surface::anchor::TheuthAnchor::parse(&c.href) else {
            continue;
        };
        match agent::notebooks::session_cite(&agents, &anchor.id).await {
            Ok(Some(s)) => {
                c.title = s.title;
                if let Some(surface::anchor::Locator::Turn(t)) = anchor.locator
                    && t as usize > s.turns
                {
                    c.status = codeindex::LocateStatus::Missing;
                }
            }
            Ok(None) => c.available = false,
            Err(e) => tracing::debug!(href = %c.href, "session citation check skipped: {e:#}"),
        }
    }
}

/// 184 §12.4: `msg` with engine id `id` replaced by its machine name, else by the id's first 8
/// characters, as the UI shows an unknown machine.
pub fn named(msg: &str, id: &str, name: Option<&str>) -> String {
    let short: String = id.chars().take(8).collect();
    msg.replace(id, name.unwrap_or(&short))
}

/// `id`'s machine name in `st`: this engine's `self_name` or a peer's `name`.
pub fn machine_name<'a>(st: &'a mesh::MeshStatus, id: &str) -> Option<&'a str> {
    if st.self_id.0 == id {
        return Some(&st.self_name);
    }
    st.peers
        .iter()
        .find(|p| p.id.0 == id)
        .map(|p| p.name.as_str())
}
