//! Connection ownership tracking for newly opened private session trees.

use crate::SessionOpen;
use tools::Retention;
use rpc_core::Server;

/// Track private-session ownership for an RPC connection.
pub trait RpcSessionTrackExt {
    /// Bind the opened private tree and its known members to `peer`.
    fn track(&self, open: &SessionOpen, peer: u64);
}

impl RpcSessionTrackExt for Server {
    /// Record a freshly opened private tree as `peer`'s: every known member
    /// of its root moves to `peer` (a resume rebinds the whole tree), and
    /// the opened session and root join.
    fn track(&self, open: &SessionOpen, peer: u64) {
        if open.retention != Retention::Private {
            return;
        }
        let mut map = self.private.write();
        for v in map.values_mut() {
            if v.0 == open.root {
                v.1 = peer;
            }
        }
        map.insert(open.root, (open.root, peer));
        map.insert(open.session, (open.root, peer));
    }

}
