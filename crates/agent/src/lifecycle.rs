//! Session lifecycle contracts (design doc 51 §4, §8.2): the wire types of
//! explicit creation / leave and the `Agents` messages behind them. The
//! typed close barrier (`TreeOpened` / `CloseTree` / `CloseAck`) lives in
//! `tools` so owners below this crate can implement it; re-exported here.

use std::fmt;
use std::sync::Arc;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tools::{Retention, SessionResources};
pub use tools::{CloseAck, CloseTree, TreeOpened};

use crate::private::Scratch;
use crate::store::Store;

/// Options of `create_session`. `view` is the client's view id, required
/// for a private root (it names the single owner); `project` is root
/// metadata; `cwd` is recorded on the row.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CreateSessionOptions {
    /// `durable` (default) or `private`.
    #[serde(default)]
    pub retention: Retention,
    /// Root project label; `None` = ungrouped.
    #[serde(default)]
    pub project: Option<String>,
    /// The client's view id; required for a private root, which it owns.
    #[serde(default)]
    pub view: Option<String>,
    /// Working directory recorded on the session row.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Reply to `create_session` / `resume_private`: the opened session, its
/// root, and for a private root the lease secret the owner presents on
/// resume and leave.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionOpen {
    /// The session that was opened or resumed.
    pub session: SessionId,
    /// Root of its tree (`session` itself for a fresh root).
    pub root: SessionId,
    /// Storage of the tree.
    pub retention: Retention,
    /// Private roots only: the secret presented on `resume_private` and
    /// `leave_session`; rotated on every resume.
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
    /// How far the scratch cleanup had got when the caller was answered.
    pub cleanup: CleanupStatus,
}

/// `ask` → `Result<SessionOpen, String>`: explicit creation (51 §4.1).
/// `peer` is the RPC peer id and its closed-token; required for a private
/// root (it becomes the owner), ignored for durable.
#[derive(Clone, Debug)]
pub struct CreateSession {
    /// What to create.
    pub options: CreateSessionOptions,
    /// `(RpcPeer::id(), RpcPeer::closed_token())` of the caller.
    pub peer: Option<(u64, CancellationToken)>,
}

/// `ask` → `Result<SessionOpen, String>`: a reconnecting owner rebinds its
/// private root (51 §4.3); the secret rotates on success.
#[derive(Clone)]
pub struct ResumePrivate {
    /// The private root (or any member of its tree).
    pub session: SessionId,
    /// Must equal the owner's view.
    pub view: String,
    /// The current secret; rotated by the reply.
    pub lease_secret: String,
    /// The new holder: peer id and its closed-token.
    pub peer: (u64, CancellationToken),
}

impl fmt::Debug for ResumePrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResumePrivate")
            .field("session", &self.session)
            .field("view", &self.view)
            .field("peer", &self.peer.0)
            .finish_non_exhaustive()
    }
}

/// `ask` → `Result<CloseResult, String>`: the owner leaves its private tree
/// (51 §4.4). Authorized by the owner peer, or by the current secret.
#[derive(Clone)]
pub struct LeaveSession {
    /// Any member of the private tree.
    pub session: SessionId,
    /// Authorizes a caller other than the owner peer.
    pub lease_secret: Option<String>,
    /// The calling RPC peer.
    pub peer: u64,
}

impl fmt::Debug for LeaveSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaveSession")
            .field("session", &self.session)
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

/// `ask` → `Result<Authorized, String>`: may `peer` touch `session`? Fails
/// closed for an unknown id, a closing tree and a private tree owned by
/// another peer; never creates.
#[derive(Clone, Copy, Debug)]
pub struct Authorize {
    /// The session addressed.
    pub session: SessionId,
    /// The calling RPC peer; `None` for a trusted in-process caller.
    pub peer: Option<u64>,
}

/// Reply to [`Authorize`]: the tree's root and its capability.
#[derive(Clone, Debug, kameo::Reply)]
pub struct Authorized {
    /// Root of the tree `session` belongs to.
    pub root: SessionId,
    /// The tree's retention.
    pub retention: Retention,
    /// Lifecycle generation of the tree (0 for durable).
    pub generation: u64,
    /// The tree's file capability.
    pub resources: Arc<SessionResources>,
    /// Owner peer of a private tree; `None` for durable.
    pub owner_peer: Option<u64>,
}

/// `ask` → `Vec<(member, root, owner peer)>` of every open private tree:
/// the RPC event filter's seed (51 §4.2).
#[derive(Clone, Copy, Debug)]
pub struct PrivateTrees;

/// The one client holding a private root's lease: its view, the RPC peer
/// id and that peer's closed-token, the generation it holds, and the
/// secret it must present. `Debug` redacts the secret.
#[derive(Clone)]
pub struct PrivateOwner {
    /// The client's view id given at `create_session`; must match on resume.
    pub view: String,
    /// `link::RpcPeer::id()` of the holder.
    pub peer: u64,
    /// `link::RpcPeer`'s closed token: cancelled when the connection went.
    pub peer_closed: CancellationToken,
    /// Lifecycle generation the lease was taken under.
    pub generation: u64,
    /// Lease secret presented on `resume_private` / `leave_session`.
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

/// One private root tree as `Agents` holds it (51 §4): its in-memory
/// store (shared by a fork of it), scratch dir, owner and members.
pub(crate) struct PrivateRoot {
    pub store: Arc<Store>,
    /// `None` once teardown took it.
    pub scratch: Option<Scratch>,
    pub owner: PrivateOwner,
    pub resources: Arc<SessionResources>,
    /// Root first, then every child created so far.
    pub members: Vec<SessionId>,
    /// Cancels the owner watcher; replaced on rebind, fired on close.
    pub cancel: CancellationToken,
    pub closing: bool,
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
