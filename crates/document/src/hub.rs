//! The hub gate (69 §5, contract `69-contract-docs.md` §4): the yrs server is a hub duty. One engine
//! of a mesh — the lease holder — opens documents, accepts updates and awareness, serves subscribers,
//! admits guests, administers members and links, imports the derived files it watches and exports
//! them. Every other engine holds the replicated mirror (`doc_read`, `Outline`, `Markdown` serve from
//! it anywhere) and answers the rest with [`DocError::NotHub`], which the RPC layer turns into the
//! typed `-32011` error and Y turns into forwarding.
//!
//! Lease following is Slack's pattern (`sync_socket`): a [`mesh::lease::Follower`] fed by
//! [`mesh::MeshEvent::Lease`] with the self id from `lookup_when::<mesh::Mesh>`, and a reconcile
//! holding `Option<CancellationToken>` for the derived-file watcher. `holder: None` is a loss once the
//! self id is known; `Follower::default()` (no mesh) is the hub, so a lone engine behaves as before.

use std::fmt;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use mesh::lease::Follower;
use mesh::{EngineId, LeaseState};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::actor::DocumentActor;
use crate::meta::DocEvent;

/// Why a document call was refused, when it is more than a message.
///
/// The actor's hub-only handlers answer `Err(DocError::NotHub { hub })`; handlers whose reply type
/// stayed `String` (the owner-plugin surface: `Ensure`, `Edit`, `Snapshot`, …) answer its
/// [`fmt::Display`] form `NotHub:<engine id>` (empty id when no engine holds the lease), which
/// [`DocError::parse`] reads back. The RPC layer maps either to `-32011` with data
/// `{"NotHub": {"hub": "<engine id>" | null}}` (serde external tagging, like `OwnerUnreachable`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum DocError {
    /// This engine does not hold the hub lease; `hub` is the holder as last seen (`None`: nobody).
    NotHub {
        /// The lease holder, when there is one.
        hub: Option<EngineId>,
    },
    /// Any other refusal, as the handler worded it.
    Other(String),
}

/// The `Display` prefix of [`DocError::NotHub`].
const NOT_HUB: &str = "NotHub:";

impl DocError {
    /// Read a [`fmt::Display`] form back: `NotHub:<id>` → `NotHub`, anything else → `Other`.
    pub fn parse(s: &str) -> Self {
        match s.strip_prefix(NOT_HUB) {
            Some(id) => Self::NotHub {
                hub: (!id.is_empty()).then(|| EngineId(id.to_owned())),
            },
            None => Self::Other(s.to_owned()),
        }
    }

    /// The `NotHub` payload, if this is one.
    pub fn not_hub(&self) -> Option<&Option<EngineId>> {
        match self {
            Self::NotHub { hub } => Some(hub),
            Self::Other(_) => None,
        }
    }
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHub { hub } => write!(
                f,
                "{NOT_HUB}{}",
                hub.as_ref().map(|h| h.0.as_str()).unwrap_or("")
            ),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for DocError {}

impl From<String> for DocError {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}

impl From<&str> for DocError {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<DocError> for String {
    fn from(e: DocError) -> Self {
        e.to_string()
    }
}

/// Where this engine stands: the follower, the holder as last seen, the watcher's token while serving.
#[derive(Debug, Default)]
pub struct Hub {
    follower: Follower,
    holder: Option<EngineId>,
    watcher: Option<CancellationToken>,
}

impl Hub {
    /// Whether this engine serves documents now.
    pub fn holding(&self) -> bool {
        self.follower.holding
    }

    /// `Ok` on the hub; the refusal every hub-only handler returns elsewhere.
    pub fn gate(&self) -> Result<(), DocError> {
        if self.follower.holding {
            Ok(())
        } else {
            Err(DocError::NotHub {
                hub: self.holder.clone(),
            })
        }
    }

    /// [`Self::gate`] for handlers whose error is a `String`.
    pub fn gate_str(&self) -> Result<(), String> {
        self.gate().map_err(String::from)
    }

    /// Where a hub-only call goes: `Ok(None)` serve here, `Ok(Some(hub))` forward to the holder over
    /// `theuth/docs/1`, `Err` refuse (nobody holds the lease — nothing to forward to).
    pub fn route(&self) -> Result<Option<EngineId>, DocError> {
        match self.gate() {
            Ok(()) => Ok(None),
            Err(DocError::NotHub { hub: Some(hub) }) => Ok(Some(hub)),
            Err(e) => Err(e),
        }
    }

    /// The lease holder as last seen.
    pub fn holder(&self) -> Option<&EngineId> {
        self.holder.as_ref()
    }

    /// The mesh answered `Status`: `Some(gained)` when the duties flip.
    pub fn set_self(&mut self, id: EngineId) -> Option<bool> {
        self.follower.set_self(id.0)
    }

    /// A lease event: `Some(gained)` when the duties flip.
    pub fn observe(&mut self, lease: &LeaseState) -> Option<bool> {
        self.holder = lease.holder.clone();
        self.follower.observe(lease)
    }
}

/// `tell`: the mesh told us our own id (`lookup_when::<mesh::Mesh>` → `Status`).
#[derive(Clone, Debug)]
pub struct SelfId(pub EngineId);

/// `tell`: reconcile duties with the lease (at start, and after every flip).
#[derive(Clone, Copy, Debug, Default)]
pub struct Reconcile;

impl DocumentActor {
    /// 67 §4.4 / 69: start the derived-file watcher when this engine holds the lease; on loss stop it,
    /// flush pending exports, snapshot and unload every open document (its subscribers hear
    /// `Closed{hub_moved}`). Either flip drops the forwarding legs: their subscribers hear the same,
    /// and the next call opens a leg to the holder of the day (or serves here). Idempotent.
    pub(crate) fn sync_hub(&mut self, me: ActorRef<Self>) {
        let want = self.hub.holding();
        match (want, self.hub.watcher.take()) {
            (true, Some(tok)) => self.hub.watcher = Some(tok),
            (true, None) => {
                self.legs.clear();
                let tok = self.rt.shutdown.child_token();
                match crate::watch::start(&self.data_dir, me, tok.clone()) {
                    Ok(()) => tracing::info!("documents: serving (hub lease held here)"),
                    Err(e) => tracing::warn!("documents file watcher unavailable: {e:#}"),
                }
                self.hub.watcher = Some(tok);
            }
            (false, Some(tok)) => {
                tok.cancel();
                self.legs.clear();
                let open: Vec<_> = self.open.keys().cloned().collect();
                for id in &open {
                    if self
                        .open
                        .get(id)
                        .is_some_and(|o| o.export_due.is_some() && !o.exporting)
                    {
                        self.export(id, None);
                    }
                }
                for id in &open {
                    self.unload(id, HUB_MOVED);
                }
                tracing::info!(
                    hub = ?self.hub.holder().map(|h| h.0.as_str()),
                    closed = open.len(),
                    "documents: another engine holds the hub lease; serving stopped"
                );
            }
            (false, None) => {}
        }
    }
}

/// `DocEvent::Closed { reason }` every subscriber gets on a lease loss.
pub const HUB_MOVED: &str = "hub_moved";

impl Message<SelfId> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: SelfId, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.hub.set_self(m.0).is_some() {
            self.sync_hub(ctx.actor_ref().clone());
        }
    }
}

impl Message<mesh::MeshEvent> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: mesh::MeshEvent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let mesh::MeshEvent::Lease(lease) = m
            && self.hub.observe(&lease).is_some()
        {
            self.sync_hub(ctx.actor_ref().clone());
        }
    }
}

impl Message<Reconcile> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.sync_hub(ctx.actor_ref().clone());
    }
}

/// The `Closed` event of a loss, for the actor's unload path and tests.
pub fn closed() -> DocEvent {
    DocEvent::Closed {
        reason: HUB_MOVED.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_hub_round_trips_its_display_and_json() {
        let e = DocError::NotHub {
            hub: Some(EngineId("abc".into())),
        };
        assert_eq!(e.to_string(), "NotHub:abc");
        assert_eq!(DocError::parse(&e.to_string()), e);
        assert_eq!(
            DocError::from(String::from("NotHub:")),
            DocError::NotHub { hub: None }
        );
        assert_eq!(DocError::from("nope"), DocError::Other("nope".into()));
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"NotHub":{"hub":"abc"}}"#
        );
        assert_eq!(
            serde_json::to_string(&DocError::NotHub { hub: None }).unwrap(),
            r#"{"NotHub":{"hub":null}}"#
        );
    }

    #[test]
    fn gate_follows_the_lease_and_a_lone_engine_is_the_hub() {
        let mut hub = Hub::default();
        assert!(hub.gate().is_ok(), "no mesh: hub");
        let lease = |holder: Option<&str>| LeaseState {
            holder: holder.map(|h| EngineId(h.into())),
            since_ms: 0,
            reason: "test".into(),
        };
        // The lease arrives before the self id: remembered, nothing flips yet.
        assert_eq!(hub.observe(&lease(Some("other"))), None);
        assert!(hub.gate().is_ok());
        assert_eq!(hub.set_self(EngineId("me".into())), Some(false));
        assert_eq!(
            hub.gate(),
            Err(DocError::NotHub {
                hub: Some(EngineId("other".into()))
            })
        );
        assert_eq!(hub.gate_str(), Err("NotHub:other".to_owned()));
        assert_eq!(hub.observe(&lease(Some("me"))), Some(true));
        assert!(hub.gate().is_ok());
        // Nobody holds it: a loss, with no hub to name.
        assert_eq!(hub.observe(&lease(None)), Some(false));
        assert_eq!(hub.gate(), Err(DocError::NotHub { hub: None }));
        assert_eq!(hub.observe(&lease(None)), None);
    }
}
