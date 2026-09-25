//! A2A over the session conn (32 §13.1 as amended by 67 §8): a remote theuth
//! child addresses its parent with an ACP extension notification on the
//! very conn that carries its `session/prompt`s — no new link mode, no new
//! frame. The parent's `send_message` to the child keeps travelling as the
//! `<message>`-wrapped prompt it is today, so only the child → parent
//! direction needs a method of its own.

use agent_client_protocol::{Error, UntypedMessage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Wire method of [`PeerMessage`] (ACP reserves the `_` prefix for extensions).
pub const PEER_MESSAGE_METHOD: &str = "_theuth/peer_message";

/// `send_message{to: "parent"}` from a remote child, as its parent receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerMessage {
    /// The ACP session id of the sending child (the id `session/new` returned).
    pub session_id: String,
    /// The sender's id for this message; the parent's idempotency key.
    pub id: Uuid,
    /// The parent-side message this answers (the `id` of the `<message>` the
    /// child received), when it is a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Uuid>,
    /// Bounded, redacted body.
    pub text: String,
}

impl PeerMessage {
    /// The notification to send on the conn.
    pub fn notification(&self) -> Result<UntypedMessage, Error> {
        UntypedMessage::new(PEER_MESSAGE_METHOD, self)
    }

    /// The message a received notification carries; `None` for any other
    /// method or a malformed body (dropped, never an error to the peer).
    #[must_use]
    pub fn parse(n: &UntypedMessage) -> Option<Self> {
        (n.method == PEER_MESSAGE_METHOD)
            .then(|| serde_json::from_value(n.params.clone()).ok())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = PeerMessage {
            session_id: "0193b7c0-0000-7000-8000-000000000002".into(),
            id: Uuid::now_v7(),
            reply_to: Some(Uuid::now_v7()),
            text: "done: 3 files".into(),
        };
        let n = m.notification().unwrap();
        assert_eq!(n.method, PEER_MESSAGE_METHOD);
        assert_eq!(n.params["sessionId"], m.session_id);
        assert_eq!(PeerMessage::parse(&n), Some(m.clone()));
        // Through JSON, as the conn carries it.
        let back: UntypedMessage = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(PeerMessage::parse(&back), Some(m));
    }

    #[test]
    fn other_methods_and_bad_bodies_are_none() {
        let other = UntypedMessage::new("session/update", serde_json::json!({})).unwrap();
        assert_eq!(PeerMessage::parse(&other), None);
        let bad = UntypedMessage::new(PEER_MESSAGE_METHOD, serde_json::json!({"text": 1})).unwrap();
        assert_eq!(PeerMessage::parse(&bad), None);
        let no_reply = UntypedMessage::new(
            PEER_MESSAGE_METHOD,
            serde_json::json!({"sessionId": "s", "id": Uuid::nil(), "text": "hi"}),
        )
        .unwrap();
        assert_eq!(PeerMessage::parse(&no_reply).unwrap().reply_to, None);
    }
}
