//! ACP RFD subagents (`unstable_subagents`, schema 1.10.2; 141 §6.1): the
//! `subagent_update` an agent sends on the **parent** session becomes a
//! [`ChildAnnounce`]; the child's own traffic then arrives as plain
//! `session/update`s whose outer `sessionId` is the child (routed by
//! `Update::Session.session`, not here).
//!
//! `MaybeUndefined` fields map to `Option` with absent = unchanged (`None`)
//! and `null` = an explicit clear value, so a status-only frame touches only
//! `state`.

use agent_client_protocol::MaybeUndefined;
use agent_client_protocol::schema::v1::{SessionId, StateUpdate, StopReason, SubagentUpdate};

use super::{ChildAnnounce, ChildSource, ChildState, CostRule};

/// The announce for `u`, sent on `parent` (the wire `params.sessionId` of
/// the notification). `own` is the harness's session id when known: a
/// `subagent_update` on it is a depth-1 child (`parent: None`); on any
/// other session it is nested under that session's external id.
///
/// Clears: `title: null` → the external id (the RFD leaves the fallback to
/// the client); `description: null` → `""`; `capabilities: null` →
/// `can_cancel: false`; `state: null` → [`ChildState::Unknown`] (activity
/// unset); `_meta: null` → `detail.meta = null`. An `idle` whose
/// `stopReason` is `cancelled` is [`ChildState::Cancelled`].
#[must_use]
pub fn announce(parent: &SessionId, own: Option<&str>, u: SubagentUpdate) -> ChildAnnounce {
    let child = u.session_id.0.to_string();
    let mut a = ChildAnnounce::new(ChildSource::Rfd, child.clone());
    a.parent = (own != Some(&*parent.0)).then(|| parent.0.to_string());
    a.title = match u.title {
        MaybeUndefined::Undefined => None,
        MaybeUndefined::Null => Some(child),
        MaybeUndefined::Value(t) => Some(t),
    };
    a.description = match u.description {
        MaybeUndefined::Undefined => None,
        MaybeUndefined::Null => Some(String::new()),
        MaybeUndefined::Value(d) => Some(d),
    };
    a.can_cancel = match u.capabilities {
        MaybeUndefined::Undefined => None,
        MaybeUndefined::Null => Some(false),
        MaybeUndefined::Value(c) => Some(c.cancel.is_some()),
    };
    a.state = match u.state {
        MaybeUndefined::Undefined => None,
        MaybeUndefined::Null => Some(ChildState::Unknown),
        MaybeUndefined::Value(s) => Some(state(s)),
    };
    match u.meta {
        MaybeUndefined::Undefined => {}
        MaybeUndefined::Null => {
            a.detail.insert("meta".into(), serde_json::Value::Null);
        }
        MaybeUndefined::Value(m) => {
            a.detail.insert(
                "meta".into(),
                serde_json::Value::Object(m.into_iter().collect()),
            );
        }
    }
    a.cost_rule = Some(CostRule::DisplayOnly);
    a
}

fn state(s: StateUpdate) -> ChildState {
    match s {
        StateUpdate::Running(_) => ChildState::Running,
        StateUpdate::RequiresAction(_) => ChildState::RequiresAction,
        StateUpdate::Idle(i) if i.stop_reason == Some(StopReason::Cancelled) => {
            ChildState::Cancelled
        }
        StateUpdate::Idle(i) => ChildState::Idle {
            stop_reason: i.stop_reason.map(super::stop_reason),
            usage: i.usage,
        },
        // `Unknown`, and any custom/future state the SDK could not name.
        _ => ChildState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::SessionNotification;
    use agent_client_protocol::schema::v1::SessionUpdate;

    use super::*;

    /// A hand-written frame (crates/acp/fixtures/rfd, synthetic from schema 1.10.2).
    fn fixture(name: &str) -> SessionNotification {
        let path = format!("{}/fixtures/rfd/{name}", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(&path);
        serde_json::from_value(v["params"].clone()).expect(&path)
    }

    fn announce_of(name: &str, own: Option<&str>) -> ChildAnnounce {
        let n = fixture(name);
        let SessionUpdate::SubagentUpdate(u) = n.update else {
            panic!("{name}: subagent_update expected");
        };
        announce(&n.session_id, own, u)
    }

    /// 141 §6.1: the first update announces identity, title, description,
    /// cancel capability and `running`; on the harness's own session the
    /// child is depth 1 (`parent: None`); on another session it is nested.
    #[test]
    fn running_announce_carries_identity_and_capabilities() {
        let a = announce_of("subagent_update.running.json", Some("sess_parent"));
        assert_eq!(a.source, ChildSource::Rfd);
        assert_eq!(a.child, "sess_child_1");
        assert_eq!(a.parent, None);
        assert_eq!(a.title.as_deref(), Some("Research: pricing page"));
        assert_eq!(
            a.description.as_deref(),
            Some("Collect the three plans' prices")
        );
        assert_eq!(a.can_cancel, Some(true));
        assert_eq!(a.state, Some(ChildState::Running));
        assert_eq!(a.cost_rule, Some(CostRule::DisplayOnly));
        assert_eq!(a.detail["meta"]["vendor/thread"], "t-9");

        let nested = announce_of("subagent_update.running.json", Some("sess_other"));
        assert_eq!(nested.parent.as_deref(), Some("sess_parent"));
        let unknown_own = announce_of("subagent_update.running.json", None);
        assert_eq!(unknown_own.parent.as_deref(), Some("sess_parent"));
    }

    /// `idle` carries the stop reason's serde name and the display-only
    /// usage; absent fields are unchanged (`None`); `stopReason: cancelled`
    /// is the cancelled state.
    #[test]
    fn idle_and_cancelled_states() {
        let a = announce_of("subagent_update.idle.json", Some("sess_parent"));
        assert_eq!(a.title, None);
        assert_eq!(a.description, None);
        assert_eq!(a.can_cancel, None);
        let Some(ChildState::Idle { stop_reason, usage }) = a.state else {
            panic!("idle expected: {:?}", a.state);
        };
        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
        let usage = usage.unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.total_tokens),
            (1200, 300, 1500)
        );
        assert!(a.detail.is_empty());

        let c = announce_of("subagent_update.cancelled.json", Some("sess_parent"));
        assert_eq!(c.state, Some(ChildState::Cancelled));
    }

    /// Explicit `null`s clear: title → the external id, description → "",
    /// capabilities → no cancel, state → unknown, `_meta` → null; a
    /// `requires_action` snapshot maps 1:1.
    #[test]
    fn nulls_clear_and_requires_action_maps() {
        let a = announce_of("subagent_update.cleared.json", Some("sess_parent"));
        assert_eq!(a.title.as_deref(), Some("sess_child_1"));
        assert_eq!(a.description.as_deref(), Some(""));
        assert_eq!(a.can_cancel, Some(false));
        assert_eq!(a.state, Some(ChildState::Unknown));
        assert_eq!(a.detail["meta"], serde_json::Value::Null);

        let r = announce_of("subagent_update.requires_action.json", Some("sess_parent"));
        assert_eq!(r.state, Some(ChildState::RequiresAction));
        let u = announce_of("subagent_update.unknown.json", Some("sess_parent"));
        assert_eq!(u.state, Some(ChildState::Unknown));
    }

    /// Child traffic is a plain `session/update` whose outer `sessionId`
    /// is the child; the typed handler forwards it unchanged with that id.
    #[test]
    fn child_traffic_and_messages_deserialize_on_the_child_session() {
        let n = fixture("child.agent_message_chunk.json");
        assert_eq!(&*n.session_id.0, "sess_child_1");
        assert!(matches!(n.update, SessionUpdate::AgentMessageChunk(_)));
        let n = fixture("session_message_chunk.json");
        assert_eq!(&*n.session_id.0, "sess_parent");
        let SessionUpdate::SessionMessageChunk(m) = n.update else {
            panic!("session_message_chunk expected");
        };
        assert_eq!(
            m.sender_session_id.as_ref().map(|s| &*s.0),
            Some("sess_child_1")
        );
        assert_eq!(
            m.recipient_session_id.as_ref().map(|s| &*s.0),
            Some("sess_parent")
        );
    }
}
