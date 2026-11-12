use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, RwLock};
use runtime::{Runtime, SessionId};
use tokio::sync::broadcast;
use wire::{RuntimeWire as _, WireFrame};

use crate::{Following, RpcPeer};

/// Insert-only private-tree membership, shared across local and socket servers.
pub type PrivateMap = Arc<RwLock<HashMap<SessionId, (SessionId, u64)>>>;
/// Per-connection delta focus, absent for an unrestricted connection.
pub type FocusMap = Arc<RwLock<HashMap<u64, Arc<HashSet<String>>>>>;
/// Finished alert identifiers already seen by each connection.
pub type SeenMap = Arc<RwLock<HashMap<u64, HashSet<String>>>>;
/// Focused-session leases, released with their connection.
pub type FollowMap = Arc<Mutex<HashMap<u64, HashMap<SessionId, Following>>>>;
static PRIVATE: LazyLock<PrivateMap> = LazyLock::new(Default::default);

/// All six fields are shared handles; module composition must only clone them.
#[derive(Clone)]
pub struct Server {
    /// Shared event feed supplied by the transport owner.
    pub events: broadcast::Sender<Arc<WireFrame>>,
    /// Runtime and shutdown handle.
    pub rt: Runtime,
    /// Process-wide insert-only private-tree membership.
    pub private: PrivateMap,
    /// Connection-local delta focus.
    pub focus: FocusMap,
    /// Connection-local finished alerts.
    pub seen: SeenMap,
    /// Connection-local focused-session leases.
    pub follows: FollowMap,
}

impl Server {
    /// Shared wire feed. Plugin registration belongs to the startup owner.
    pub fn new(rt: Runtime) -> Self {
        Self {
            events: rt.wire_feed(),
            rt,
            private: PRIVATE.clone(),
            focus: Default::default(),
            seen: Default::default(),
            follows: Default::default(),
        }
    }

    /// Remove only this connection's state once its original peer closes.
    pub fn forget_on_close(&self, peer: RpcPeer) {
        let (focus, seen, follows) = (self.focus.clone(), self.seen.clone(), self.follows.clone());
        tokio::spawn(async move {
            peer.closed().await;
            focus.write().remove(&peer.id());
            seen.write().remove(&peer.id());
            follows.lock().remove(&peer.id());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clones_share_connection_state_but_new_servers_only_share_privacy() {
        let rt = Runtime::new(Default::default(), std::env::temp_dir());
        let server = Server::new(rt.clone());
        let clone = server.clone();
        let other = Server::new(rt);
        assert!(Arc::ptr_eq(&server.private, &clone.private));
        assert!(Arc::ptr_eq(&server.private, &other.private));
        assert!(Arc::ptr_eq(&server.focus, &clone.focus));
        assert!(Arc::ptr_eq(&server.seen, &clone.seen));
        assert!(Arc::ptr_eq(&server.follows, &clone.follows));
        assert!(!Arc::ptr_eq(&server.focus, &other.focus));
        assert!(!Arc::ptr_eq(&server.seen, &other.seen));
        assert!(!Arc::ptr_eq(&server.follows, &other.follows));
    }
}
