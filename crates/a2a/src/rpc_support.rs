//! A2A actor lookup, contact routing and paired-engine decoration for RPC calls.

use crate as a2a;
use std::fmt::Display;
use a2a::{A2A_ACTOR, A2aActor};
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use kameo::message::Message;
use kameo::reply::Reply;
use runtime::Runtime;
use rpc_core::{send, lookup, failed, NOT_RUNNING};
use mesh::rpc_support::mesh_actor;

/// 191: `a2a_forward` from a machine `mesh.messaging_off` lists: the words the contact hears
/// (through the receiving machine), naming no machine.
pub const A2A_FORWARD_MESSAGING_OFF: &str = "export unavailable: messages are off between our machines";

/// 191 (D8): each contact's `paired_on` engine id by that machine's name, looked up as
/// `session_move` does: this machine's `self_name`, a mesh peer's name, else the id's first
/// 8 characters (the machine left the mesh, or the mesh cannot answer). Display only.
pub async fn name_paired_on(contacts: &mut [a2a::contacts::Contact]) {
    if contacts.iter().all(|c| c.paired_on.is_none()) {
        return;
    }
    let status = match mesh_actor() {
        Ok(m) => m.ask(mesh::Status).await.ok(),
        Err(_) => None,
    };
    for id in contacts.iter_mut().filter_map(|c| c.paired_on.as_mut()) {
        let name = status.as_ref().and_then(|s| {
            if s.self_id.0 == *id {
                return Some(s.self_name.clone());
            }
            s.peers
                .iter()
                .find(|p| p.id.0 == *id)
                .map(|p| p.name.clone())
        });
        *id = name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.get(..8).unwrap_or(id.as_str()).to_owned());
    }
}

/// The A2A actor, or `None` for the one silent case: `a2a.enabled: false`
/// (or no `a2a` section) and nothing registered. A malformed section, or
/// `enabled: true` without a registered actor (plugin missing, crashed), is
/// an error the owner must see, never a fake "disabled".
pub fn a2a(rt: &Runtime) -> RpcResult<Option<ActorRef<A2aActor>>> {
    match lookup::<A2aActor>(A2A_ACTOR) {
        Ok(actor) => return Ok(Some(actor)),
        Err(e) if e.code() != NOT_RUNNING => return Err(e),
        Err(_) => {}
    }
    let enabled = if rt.config.find_value("a2a").is_ok() {
        rt.section::<a2a::config::A2aConfig>("a2a")
            .map_err(|e| failed(format!("a2a config: {e:#}")))?
            .enabled
    } else {
        false
    };
    if enabled {
        return Err(ErrorObjectOwned::owned(
            NOT_RUNNING,
            "a2a is enabled but its actor is not running",
            None::<()>,
        ));
    }
    Ok(None)
}

/// 183 §10.1: a contact RPC: the A2A actor's answer (it runs without `a2a.enabled` too,
/// W-26), or `off` in a process without the a2a plugin.
pub async fn contacts<M>(
    rt: &Runtime,
    msg: M,
    off: RpcResult<<<A2aActor as Message<M>>::Reply as Reply>::Ok>,
) -> RpcResult<<<A2aActor as Message<M>>::Reply as Reply>::Ok>
where
    A2aActor: Message<M>,
    M: Send + 'static,
    <<A2aActor as Message<M>>::Reply as Reply>::Error: Display,
{
    match a2a(rt)? {
        Some(actor) => send(&actor, msg).await,
        None => off,
    }
}
