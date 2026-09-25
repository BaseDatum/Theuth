//! 67 §4.4, §4.6: the hub duties (dispatch, desks) follow the mesh lease.
//! [`Follower`] is the pure state machine; [`Duties`] + [`apply`] name what
//! a gain or a loss does. The actor mirrors `apply` inline (it needs `&mut
//! self`); other plugins (slack, signal) keep a `Follower` of their own.

use mesh::LeaseState;

/// Where this engine stands with the lease. Duties are on until the mesh
/// says otherwise: an engine without a mesh is its own hub.
#[derive(Clone, Debug)]
pub struct Follower {
    /// This engine's id, once the mesh answered `Status`.
    pub self_id: Option<String>,
    /// Whether the duties are on.
    pub holding: bool,
    /// The last lease seen: `Some(holder)` once one arrived.
    seen: Option<Option<String>>,
}

impl Default for Follower {
    fn default() -> Self {
        Self {
            self_id: None,
            holding: true,
            seen: None,
        }
    }
}

impl Follower {
    /// The engine id is known: re-evaluate the last lease seen.
    /// `Some(true)` = just gained, `Some(false)` = just lost, `None` = unchanged.
    pub fn set_self(&mut self, id: String) -> Option<bool> {
        self.self_id = Some(id);
        self.settle()
    }

    /// A lease event. `Some(true)` = just gained (holder is us), `Some(false)`
    /// = just lost (someone else or nobody holds), `None` = unchanged. Before
    /// `self_id` is known nothing changes: the lease is remembered and
    /// [`Self::set_self`] resolves it.
    pub fn observe(&mut self, lease: &LeaseState) -> Option<bool> {
        self.seen = Some(lease.holder.as_ref().map(|h| h.0.clone()));
        self.settle()
    }

    fn settle(&mut self) -> Option<bool> {
        let me = self.self_id.as_deref()?;
        let holder = self.seen.as_ref()?;
        let want = holder.as_deref() == Some(me);
        if want == self.holding {
            return None;
        }
        self.holding = want;
        Some(want)
    }
}

/// What a hub does that a non-hub does not (67 §4.4).
#[async_trait::async_trait]
pub trait Duties {
    /// Wakes are dispatched (or held) on this engine.
    async fn dispatching(&self, on: bool);
    /// Open the desks the roster wants.
    async fn open_desks(&self);
    /// Close every desk, recording `reason` on the episodes.
    async fn close_desks(&self, reason: &str);
}

/// Act on a [`Follower`] change: gained → dispatch on, desks open; lost →
/// desks closed (`"lease"`), dispatch off; unchanged → nothing.
pub async fn apply(change: Option<bool>, d: &dyn Duties) {
    match change {
        Some(true) => {
            d.dispatching(true).await;
            d.open_desks().await;
        }
        Some(false) => {
            d.close_desks("lease").await;
            d.dispatching(false).await;
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mesh::EngineId;

    use super::*;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl Log {
        fn push(&self, s: String) {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).push(s);
        }
    }

    fn lease(holder: Option<&str>) -> LeaseState {
        LeaseState {
            holder: holder.map(|h| EngineId(h.to_owned())),
            since_ms: 0,
            reason: "test".into(),
        }
    }

    #[async_trait::async_trait]
    impl Duties for Log {
        async fn dispatching(&self, on: bool) {
            self.push(format!("dispatching({on})"));
        }
        async fn open_desks(&self) {
            self.push("open_desks".into());
        }
        async fn close_desks(&self, reason: &str) {
            self.push(format!("close_desks({reason})"));
        }
    }

    #[tokio::test]
    async fn duties_follow_the_lease() {
        let log = Log::default();
        let mut f = Follower::default();
        assert!(f.holding, "duties on until the mesh says otherwise");

        // Unknown self: the lease is remembered, nothing moves.
        let c = f.observe(&lease(Some("b")));
        assert_eq!(c, None);
        assert!(f.holding);
        apply(c, &log).await;

        // Self known: the remembered lease says someone else holds → lost.
        let c = f.set_self("a".into());
        assert_eq!(c, Some(false));
        apply(c, &log).await;

        // Gained.
        let c = f.observe(&lease(Some("a")));
        assert_eq!(c, Some(true));
        apply(c, &log).await;
        assert_eq!(f.observe(&lease(Some("a"))), None, "same holder again is no change");

        // Other holder, then nobody: lost once.
        let c = f.observe(&lease(Some("b")));
        assert_eq!(c, Some(false));
        apply(c, &log).await;
        let c = f.observe(&lease(None));
        assert_eq!(c, None);
        apply(c, &log).await;

        // Regained.
        let c = f.observe(&lease(Some("a")));
        assert_eq!(c, Some(true));
        apply(c, &log).await;

        assert_eq!(
            *log.0.lock().unwrap_or_else(|p| p.into_inner()),
            [
                "close_desks(lease)",
                "dispatching(false)",
                "dispatching(true)",
                "open_desks",
                "close_desks(lease)",
                "dispatching(false)",
                "dispatching(true)",
                "open_desks",
            ]
        );
    }

    #[test]
    fn set_self_without_a_lease_is_no_change() {
        let mut f = Follower::default();
        assert_eq!(f.set_self("a".into()), None);
        assert!(f.holding);
        assert_eq!(f.observe(&lease(None)), Some(false));
        assert_eq!(f.observe(&lease(None)), None);
    }
}
