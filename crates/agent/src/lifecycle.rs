//! Session lifecycle contracts (design doc 51 §4, §8.2): the wire types of
//! explicit creation / leave and the `Agents` messages behind them. The
//! typed close barrier (`TreeOpened` / `CloseTree` / `CloseAck`) lives in
//! `tools` so owners below this crate can implement it; re-exported here.

use std::fmt;
use std::sync::Arc;

use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
pub use tools::{CloseAck, CloseTree, TreeOpened};
use tools::{Retention, SessionResources};

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
    /// 144 §8: bind the new root to this notebook (`DocId` text); durable sessions only.
    #[serde(default)]
    pub notebook: Option<String>,
    /// The client's view id; required for a private root, which it owns.
    #[serde(default)]
    pub view: Option<String>,
    /// Working directory recorded on the session row.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Model for this session only (catalog id); `None` = `agent.model`. Unlike
    /// [`crate::SetModel`] it never rewrites the default or `config.yaml`
    /// (`meetings/02-live.md` §0.2).
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort for this session only; `None` = `agent.effort`. Like
    /// `model`, never the default (a remote parent's pick, 67 §8).
    #[serde(default)]
    pub effort: Option<String>,
    /// Risk ceiling for this root and its descendants (67 §8: a remote
    /// parent's `tools`); installed on the registry like a subagent's cap.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cap: Option<tools::Risk>,
    /// The session on another engine that opens this root over the node conn
    /// (67 §8, `_meta.theuth.parent`): it becomes this root's `parent` peer.
    #[serde(default)]
    pub parent_session: Option<String>,
    /// That engine's id as this one knows it (`_meta.theuth.parent_engine`).
    #[serde(default)]
    pub parent_engine: Option<String>,
    /// Open the root as a child of this `acp/agents.yaml` harness of ours
    /// instead of a native session (67 §8, `agent: "<harness>@<engine>"`
    /// arriving as `_meta.theuth.harness`). `model` then goes through the
    /// harness's model option; `effort` and `cap` do not apply.
    #[serde(default)]
    pub harness: Option<String>,
    /// Memory mode (137 D1): `epochs` | `endless`; `None` = `agent.endless.default_chat`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub memory: Option<crate::endless::MemoryMode>,
    /// Advisor model/effort for this root. Installed on its owner before create
    /// returns: `set_advisor` is local-only, so a remote draft cannot use it.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub advisor: Option<(String, Option<String>)>,
    /// 183 §5.6 (W-10): the mesh engine id to create the session on; `None`
    /// (or this engine) = here. The attached engine forwards the whole call.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub engine: Option<String>,
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
    /// `rpc_core::RpcPeer::id()` of the holder.
    pub peer: u64,
    /// `rpc_core::RpcPeer`'s closed token: cancelled when the connection went.
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

/// 194 D4b–d: the effort a model setting stores beside `model` (a session-model setting: a
/// meeting, a routine, an autoresearch role). `info` is what [`effort_model`] found for `model`.
/// - `None`, blank or `default` is the Default chip: `Ok(None)`.
/// - No model (the setting's `none` row) clears the effort too (D4d): `Ok(None)`.
/// - A catalog model checks it the way `models::Model::validate_effort` does, with its message;
///   a model the catalog does not know (`Ok(None)`, or no agent) accepts any known effort word
///   (`effort_of`); a model that does not resolve (`Err`) refuses with that error.
/// - A refused effort is dropped (`Ok(None)`) when `model_changed` (D4c), else `Err` (D4b).
pub fn settle_effort(
    model: Option<&str>,
    info: &Result<Option<crate::ModelInfo>, String>,
    effort: Option<&str>,
    model_changed: bool,
) -> Result<Option<String>, String> {
    let Some(e) = effort
        .map(str::trim)
        .filter(|e| !e.is_empty() && *e != "default")
    else {
        return Ok(None);
    };
    if model.is_none_or(|m| m.trim().is_empty()) {
        return Ok(None);
    }
    let checked = match info {
        Err(why) => Err(why.clone()),
        Ok(Some(i)) => {
            let budget = i.budget_min.map(|min| (min, i.budget_max));
            let in_budget = |(min, max): (u32, Option<u32>)| {
                e.parse::<u32>()
                    .is_ok_and(|n| n >= min && n <= max.unwrap_or(u32::MAX))
            };
            if i.efforts.iter().any(|x| x == e) || budget.is_some_and(in_budget) {
                Ok(())
            } else {
                let mut accepted = i.efforts.join("|");
                if let Some((min, max)) = budget {
                    let max = max.map(|m| m.to_string()).unwrap_or_default();
                    let sep = if accepted.is_empty() { "" } else { " " };
                    accepted = format!("{accepted}{sep}or a token budget {min}..{max}");
                }
                if accepted.is_empty() {
                    accepted.push_str("no reasoning effort");
                }
                Err(format!("{} accepts {accepted}", i.id))
            }
        }
        Ok(None) if crate::effort_of(e).is_none() => Err(format!("unknown effort {e}")),
        Ok(None) => Ok(()),
    };
    match checked {
        Ok(()) => Ok(Some(e.to_owned())),
        Err(_) if model_changed => Ok(None),
        Err(why) => Err(why),
    }
}

/// 194: the catalog row [`settle_effort`] checks `model` against ([`model_meta`] on the
/// registered agent); `Ok(None)` without a model or without a running agent.
pub async fn effort_model(model: Option<&str>) -> Result<Option<crate::ModelInfo>, String> {
    let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    match kameo::actor::ActorRef::<crate::Agents>::lookup(crate::AGENT_ACTOR) {
        Ok(Some(agents)) => model_meta(&agents, model).await,
        _ => Ok(None),
    }
}

/// 194 D4b: `model`'s catalog metadata without the credentials check `ResolveModel` makes,
/// so a stored setting is validated against a known model whose provider has no key yet.
/// `Ok(None)`: the catalog does not know it (a pass-through id); `Err`: it does not resolve.
pub async fn model_meta(
    agents: &kameo::actor::ActorRef<crate::Agents>,
    model: &str,
) -> Result<Option<crate::ModelInfo>, String> {
    let model = model.to_owned();
    agents.ask(ModelMeta { model }).await.map_err(|e| match e {
        kameo::error::SendError::HandlerError(e) => e,
        e => e.to_string(),
    })
}

/// `ask` → `Result<Option<ModelInfo>, String>`: [`model_meta`]'s catalog resolution.
#[derive(Clone, Debug)]
pub struct ModelMeta {
    /// `provider/model` or a bare model id.
    pub model: String,
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
