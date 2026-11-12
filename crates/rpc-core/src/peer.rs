use tokio_util::sync::{CancellationToken, DropGuard, WaitForCancellationFutureOwned};

/// One authenticated RPC connection — a Noise session (mode 0), a guest conn
/// (mode 2, 59 §8.3) or a local WebSocket the bearer token admitted — as the
/// method it invokes sees it: the same peer for every request on that
/// connection, a different one for every other connection. Methods read it
/// from their jsonrpsee [`Extensions`](jsonrpsee::types::Extensions)
/// (`#[method(with_extensions)]`, `ext.get::<RpcPeer>()`); `None` there means
/// no connection authenticated the call — treat it as unauthenticated, never
/// as local. [`remote`](Self::remote) tells a relay conn (the caller's host is
/// not this machine, 54 §8) from a local socket or in-process caller;
/// [`guest`](Self::guest) tells a share-link guest from the owner's devices.
#[derive(Clone, Debug)]
pub struct RpcPeer {
    id: u64,
    closed: CancellationToken,
    remote: bool,
    guest: Option<std::sync::Arc<Guest>>,
    /// Set only by [`RpcPeer::internal`] (171 §12.1).
    internal: bool,
}

/// A mode-2 conn's identity (59 §8.3): the unknown static that completed `IK`
/// and that handshake's hash — what `doc_join`'s proof binds to. `joined` is
/// set by rpc once `document` admitted the static to some document; the conn
/// task closes the conn (`CLOSE 3`) when it is still unset 5 s after the
/// handshake, or after three refused member calls.
#[derive(Debug)]
pub struct Guest {
    /// The guest's X25519 static.
    pub key: [u8; 32],
    /// Noise handshake hash of this connection.
    pub handshake_hash: [u8; 32],
    joined: std::sync::atomic::AtomicBool,
    refusals: std::sync::atomic::AtomicU8,
}

impl Guest {
    /// `document` accepted a `doc_join` or a member call from this static.
    pub fn mark_joined(&self) {
        self.joined
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether some member call succeeded on this conn.
    pub fn joined(&self) -> bool {
        self.joined.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `document` refused a member call (not a member, viewer write); returns the running count (saturating).
    pub fn refuse(&self) -> u8 {
        use std::sync::atomic::Ordering::Relaxed;
        let _ = self
            .refusals
            .try_update(Relaxed, Relaxed, |n| Some(n.saturating_add(1)));
        self.refusals()
    }

    /// Refused member calls so far.
    pub fn refusals(&self) -> u8 {
        self.refusals.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Refused member calls that close a guest conn (59 §11: "three refusals close the conn").
    pub const MAX_REFUSALS: u8 = 3;

    /// Whether the conn has used up its refusals and must close (`CLOSE 3`).
    pub fn exhausted(&self) -> bool {
        self.refusals() >= Self::MAX_REFUSALS
    }
}

#[cfg(test)]
mod guest_tests {
    use super::Guest;

    #[test]
    fn three_refusals_exhaust_a_guest_and_a_join_stands_apart() {
        let g = Guest {
            key: [1; 32],
            handshake_hash: [2; 32],
            joined: Default::default(),
            refusals: Default::default(),
        };
        assert!(!g.joined() && !g.exhausted());
        assert_eq!(g.refuse(), 1);
        assert_eq!(g.refuse(), 2);
        assert!(!g.exhausted(), "two refusals keep the conn");
        assert_eq!(g.refuse(), 3);
        assert!(g.exhausted());
        // Joining does not forgive refusals; refusals do not unjoin.
        g.mark_joined();
        assert!(g.joined() && g.exhausted());
        for _ in 0..300 {
            g.refuse();
        }
        assert_eq!(g.refusals(), u8::MAX, "the counter saturates");
    }
}

/// Held by the connection for as long as it is open; dropping it closes the
/// peer ([`RpcPeer::closed`] resolves). Never stored in extensions.
#[derive(Debug)]
pub struct RpcPeerGuard {
    _cancel_on_drop: DropGuard,
}

impl RpcPeer {
    /// A fresh local peer with a random id. The connection keeps the guard
    /// until it is gone (disconnect, revocation, server stop).
    pub fn open() -> (Self, RpcPeerGuard) {
        Self::open_with(false, None)
    }

    /// A fresh peer for a connection that reached us through the relay: its
    /// host is elsewhere, so the microphone is not on it (54 §8). Guard as
    /// [`open`](Self::open).
    pub fn open_remote() -> (Self, RpcPeerGuard) {
        Self::open_with(true, None)
    }

    /// A fresh peer for a mode-2 guest conn (59 §8.3): remote, and carrying the
    /// static + handshake hash the join proof binds to.
    pub fn open_guest(key: [u8; 32], handshake_hash: [u8; 32]) -> (Self, RpcPeerGuard) {
        Self::open_with(
            true,
            Some(std::sync::Arc::new(Guest {
                key,
                handshake_hash,
                joined: Default::default(),
                refusals: Default::default(),
            })),
        )
    }

    fn open_with(remote: bool, guest: Option<std::sync::Arc<Guest>>) -> (Self, RpcPeerGuard) {
        let closed = CancellationToken::new();
        let guard = RpcPeerGuard {
            _cancel_on_drop: closed.clone().drop_guard(),
        };
        (
            Self {
                id: rand::random(),
                closed,
                remote,
                guest,
                internal: false,
            },
            guard,
        )
    }

    /// A peer for a trusted in-process caller (the TUI, tests): authenticated
    /// by construction, local, never closes. Construct once and reuse it, or
    /// the caller looks like a new connection every time.
    pub fn internal() -> Self {
        Self {
            id: rand::random(),
            closed: CancellationToken::new(),
            remote: false,
            guest: None,
            internal: true,
        }
    }

    /// Whether the connection came through the relay, i.e. the caller is not
    /// on this host (54 §8).
    pub fn remote(&self) -> bool {
        self.remote
    }

    /// The guest identity of a mode-2 conn; `None` for the owner's devices and local callers.
    pub fn guest(&self) -> Option<&Guest> {
        self.guest.as_deref()
    }

    /// Whether this is the in-process peer of [`RpcPeer::internal`]: the
    /// owner console behind the MCP server, the ACP agent half and the TUI,
    /// none of which is a human press (171 §12.1 invariant 1).
    pub fn is_internal(&self) -> bool {
        self.internal
    }

    /// Random per connection; collision-resistant across the Noise and local
    /// WebSocket domains.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Resolves once the connection is gone; `'static`, so it can outlive the
    /// request that read the peer (e.g. cleanup spawned per pending query).
    pub fn closed(&self) -> WaitForCancellationFutureOwned {
        self.closed.clone().cancelled_owned()
    }

    /// Whether the connection is already gone.
    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// The connection's closed token itself, for owners that hold it beside
    /// the peer id (a private root's lease, 51 §4.3).
    pub fn closed_token(&self) -> CancellationToken {
        self.closed.clone()
    }
}

#[cfg(test)]
mod internal_tests {
    use super::RpcPeer;

    #[test]
    fn only_internal_is_internal() {
        assert!(RpcPeer::internal().is_internal());
        assert!(!RpcPeer::open().0.is_internal());
        assert!(!RpcPeer::open_remote().0.is_internal());
        assert!(!RpcPeer::open_guest([1; 32], [2; 32]).0.is_internal());
    }
}
