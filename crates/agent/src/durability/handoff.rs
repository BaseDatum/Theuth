//! Design doc 109 §7.3: the delta handoff an ACP child that could not be
//! restored with `session/load` is prompted with. **W4 fills this file.**

use runtime::SessionId;

use crate::store::Store;

/// Stub (contract wave): the composed handoff text for `session`'s open
/// turn within `max_tokens`. W4 reads the child's transcript since the last
/// point the harness is known to have seen and renders the delta.
pub async fn compose_handoff(_store: &Store, _session: SessionId, _max_tokens: usize) -> String {
    String::new()
}
