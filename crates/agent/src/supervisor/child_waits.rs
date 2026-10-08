//! Design doc 139 §7.4: the `waits(child_result)` row — written when a
//! child is spawned, cleared when its result reaches the parent, settled
//! when the engine that ran the child is gone after a takeover.
//! **Contract C stubs — W3 owns every body in this file.** The hooks are
//! already in place: `SpawnChild`'s success arm → [`child_spawned`],
//! `Agents::hand_off` → [`child_result_delivered`],
//! `transfer::on_resume_here` (W2) → [`settle_on_transfer`].
//! `Store::child_waits(Scope)` (C) is the reader; `cut_journal_in` leaves
//! `child_result` rows alone (Revision 1 Q8).

use runtime::SessionId;

use super::Agents;

/// A child of `parent` was spawned by the tool call `call_id`
/// (`SpawnChild` success, before `SubagentSpawned`). **W3:** writes
/// `waits { session: parent, tool_call_id: call_id, kind: ChildResult,
/// target: child, deadline: None, payload: {"agent": agent} }` through
/// `Store::wait_put`. No-op in the C contract.
pub async fn child_spawned(
    agents: &mut Agents,
    parent: SessionId,
    child: SessionId,
    call_id: &str,
    agent: Option<&str>,
) {
    let _ = (agents, parent, child, call_id, agent);
}

/// `child`'s `result:` exchange was consumed by `parent` (`hand_off`).
/// **W3:** `wait_clear` of the row `target = child`. No-op in the C contract.
pub async fn child_result_delivered(agents: &mut Agents, parent: SessionId, child: SessionId) {
    let _ = (agents, parent, child);
}

/// 139 §7.4 step 3, on the new owner of `parent` after a transfer or
/// takeover from `from`: a child owned by a reachable engine → nothing (its
/// result is forwarded, 69 P); a child owned by `from` while `from` is
/// unreachable → `wait_clear` + an injected `<message from="session:
/// <child>">` saying the result is lost, with `compose_handoff` when the
/// child's `cwd` is readable here. **W3 body.** No-op in the C contract.
pub async fn settle_on_transfer(agents: &mut Agents, parent: SessionId, from: Option<&str>) {
    let _ = (agents, parent, from);
}
