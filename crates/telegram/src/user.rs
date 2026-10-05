//! NEEDS(ROUTE): `message_user` delivery (design docs 24 §5, 98 §8.10).
//! Stage 1 registers the sink and declines every message.

use agent::{Outbound, UserSink};

use crate::channel::Handle;

#[async_trait::async_trait]
impl UserSink for Handle {
    fn name(&self) -> &'static str {
        "telegram"
    }

    // NEEDS(ROUTE): bound pass → the session's (or nearest bound ancestor's)
    // thread; fallback pass → the owner's chat.
    async fn deliver(&self, _m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
        if fallback {
            Err("telegram: message_user not implemented yet".into())
        } else {
            Ok(None)
        }
    }
}
