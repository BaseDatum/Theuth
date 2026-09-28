//! 67 §4.4, §4.6: the hub duties (dispatch, desks) follow the mesh lease.
//! [`Follower`] (the pure state machine) lives in `mesh::lease` since 69 so
//! `link` can follow the lease without depending on roster; [`Duties`] +
//! [`apply`] name what a gain or a loss does here. The actor mirrors `apply`
//! inline (it needs `&mut self`); other plugins (slack, signal) keep a
//! `Follower` of their own.

pub use mesh::lease::Follower;

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

    use mesh::{EngineId, LeaseState};

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
        assert_eq!(
            f.observe(&lease(Some("a"))),
            None,
            "same holder again is no change"
        );

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
}
