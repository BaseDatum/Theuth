//! Session lifecycle contracts (design doc 51 §4, §8.2): the wire types of
//! explicit creation / leave, and the typed close barrier `Agents` runs
//! against the owners the binary registered. Agent defines these; owners
//! implement them, so nothing here depends on a higher plugin crate.

use std::fmt;

use kameo::actor::ReplyRecipient;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tools::Retention;

/// Options of `create_session`. `view` is the client's view id, required
/// for a private root (it names the single owner); `project` is root
/// metadata; `cwd` is recorded on the row.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CreateSessionOptions {
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Reply to `create_session` / `resume_private`: the opened session, its
/// root, and for a private root the lease secret the owner presents on
/// resume and leave.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionOpen {
    pub session: SessionId,
    pub root: SessionId,
    pub retention: Retention,
    pub lease_secret: Option<String>,
}

/// How far a private close got when the caller was answered (51 §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    /// Every participant acknowledged and the scratch root is gone.
    Complete,
    /// Cleanup still running off-mailbox.
    Pending,
    /// A participant failed or timed out; bytes may remain.
    Incomplete,
}

/// Reply to `leave_session`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CloseResult {
    /// The caller's lease was revoked (it was the owner).
    pub revoked: bool,
    pub cleanup: CleanupStatus,
}

/// Close barrier sent to every registered participant: stop and remove
/// everything owned for `subtree` under `generation`; `cancel` fires when
/// `Agents` stops waiting.
#[derive(Clone, Debug)]
pub struct CloseTree {
    pub subtree: Vec<SessionId>,
    pub generation: u64,
    pub cancel: CancellationToken,
}

/// A participant's answer to [`CloseTree`].
#[derive(Clone, Debug, kameo::Reply)]
pub struct CloseAck {
    pub owner: String,
    pub ok: bool,
    pub detail: Option<String>,
}

/// `tell` to `Agents` at plugin start: `owner` will answer [`CloseTree`]
/// through `sink`.
#[derive(Clone)]
pub struct RegisterCloseParticipant {
    pub owner: &'static str,
    pub sink: ReplyRecipient<CloseTree, CloseAck>,
}

impl fmt::Debug for RegisterCloseParticipant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterCloseParticipant")
            .field("owner", &self.owner)
            .field("alive", &self.sink.is_alive())
            .finish()
    }
}

/// `tell` to `Agents` from composition: the enabled owners that must
/// register before a private root may be created (51 §8.2).
#[derive(Clone, Debug, Default)]
pub struct CloseParticipants {
    pub expected: Vec<&'static str>,
}

/// The one client holding a private root's lease: its view, the RPC peer
/// id and that peer's closed-token, the generation it holds, and the
/// secret it must present. `Debug` redacts the secret.
#[derive(Clone)]
pub struct PrivateOwner {
    pub view: String,
    /// `link::RpcPeer::id()` of the holder.
    pub peer: u64,
    /// `link::RpcPeer`'s closed token: cancelled when the connection went.
    pub peer_closed: CancellationToken,
    pub generation: u64,
    pub secret: String,
}

impl PrivateOwner {
    /// 32 random bytes, hex.
    pub fn mint_secret() -> String {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    }
}

impl fmt::Debug for PrivateOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateOwner")
            .field("view", &self.view)
            .field("peer", &self.peer)
            .field("generation", &self.generation)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_durable_and_debug_redacts_secret() {
        let o: CreateSessionOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o.retention, Retention::Durable);
        assert!(o.view.is_none());
        let o: CreateSessionOptions =
            serde_json::from_str(r#"{"retention":"private","view":"v1"}"#).unwrap();
        assert_eq!(o.retention, Retention::Private);

        let secret = PrivateOwner::mint_secret();
        assert_eq!(secret.len(), 64);
        assert_ne!(secret, PrivateOwner::mint_secret());
        let owner = PrivateOwner {
            view: "v1".into(),
            peer: 7,
            peer_closed: CancellationToken::new(),
            generation: 1,
            secret: secret.clone(),
        };
        let shown = format!("{owner:?}");
        assert!(!shown.contains(&secret));
        assert!(shown.contains("<redacted>"));
    }
}
