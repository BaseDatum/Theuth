use crate as roster;
use std::collections::HashSet;
use jsonrpsee::core::RpcResult;
use kameo::actor::ActorRef;
use rpc_core::lookup;

/// The roster actor; absent when the plugin is not loaded or its store failed (67 §6).
pub fn roster_actor() -> RpcResult<ActorRef<roster::Roster>> {
    lookup::<roster::Roster>(roster::ROSTER_ACTOR)
}


/// The attention actor; absent when the roster plugin is not loaded (70 HUD).
pub fn attention_actor() -> RpcResult<ActorRef<roster::attention::Attention>> {
    lookup::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
}

/// 155 §7.3 / §7.5: blank `alert` for this connection — every item whose
/// session it focuses (a focused `finished` alert is marked seen first) and
/// every `finished` alert it has already seen.
pub fn mask_alerts(
    items: &mut [roster::attention::AttentionItem],
    focus: Option<&HashSet<String>>,
    seen: &mut HashSet<String>,
) {
    for it in items {
        let Some(alert) = it.alert.as_ref() else {
            continue;
        };
        let focused = it
            .session
            .as_deref()
            .zip(focus)
            .is_some_and(|(s, f)| f.contains(s));
        let finished = alert.class == roster::alert::AlertClass::Finished;
        if focused && finished {
            seen.insert(alert.id.clone());
        }
        if focused || (finished && seen.contains(&alert.id)) {
            it.alert = None;
        }
    }
}
