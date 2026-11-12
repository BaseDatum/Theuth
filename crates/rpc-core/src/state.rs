use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, RwLock};
use runtime::{Runtime, SessionId};
use tokio::sync::broadcast;

use crate::{Following, RpcPeer};

/// Single feed-item seam; the wire-frame cutover changes this payload.
pub type Event = Arc<serde_json::Value>;
/// Insert-only private-tree membership, shared across local and socket servers.
pub type PrivateMap = Arc<RwLock<HashMap<SessionId, (SessionId, u64)>>>;
pub type FocusMap = Arc<RwLock<HashMap<u64, Arc<HashSet<String>>>>>;
pub type SeenMap = Arc<RwLock<HashMap<u64, HashSet<String>>>>;
pub type FollowMap = Arc<Mutex<HashMap<u64, HashMap<SessionId, Following>>>>;
static PRIVATE: LazyLock<PrivateMap> = LazyLock::new(Default::default);

/// All six fields are shared handles; module composition must only clone them.
#[derive(Clone)]
pub struct Server {
    pub events: broadcast::Sender<Event>,
    pub rt: Runtime,
    pub private: PrivateMap,
    pub focus: FocusMap,
    pub seen: SeenMap,
    pub follows: FollowMap,
}

impl Server {
    /// Local/no-feed server. Plugin registration belongs to the startup owner.
    pub fn new(rt: Runtime) -> Self {
        Self {
            events: broadcast::channel(1).0,
            rt,
            private: PRIVATE.clone(),
            focus: Default::default(),
            seen: Default::default(),
            follows: Default::default(),
        }
    }

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
