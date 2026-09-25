//! State shared by the actor, the uplink task and every conn task.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;
use runtime::Runtime;
use tokio::sync::{Notify, OnceCell, mpsc};
use tokio_util::sync::CancellationToken;

use crate::devices::{Devices, Pending as PendingRows};
use crate::identity::Identity;
use crate::pairing::Pending;
use crate::types::{LinkConfig, LinkStatus, MeshLink, SetDispatch};

/// Everything a conn task needs: the runtime (events), the dispatch `rpc`
/// hands over by message (32 §13) and the mutable [`State`].
pub struct Ctx {
    /// Publishes [`crate::LinkEvent`]s.
    pub rt: Runtime,
    dispatch: OnceCell<SetDispatch>,
    dispatch_set: Notify,
    state: Mutex<State>,
}

impl Ctx {
    /// Wrap.
    pub fn new(rt: Runtime, state: State) -> Arc<Self> {
        Arc::new(Self {
            rt,
            dispatch: OnceCell::new(),
            dispatch_set: Notify::new(),
            state: Mutex::new(state),
        })
    }

    /// Store the dispatch; a second call is ignored.
    pub fn set_dispatch(&self, d: SetDispatch) {
        if self.dispatch.set(d).is_ok() {
            self.dispatch_set.notify_waiters();
        }
    }

    /// The dispatch, waiting until `rpc` has sent it (the caller's handshake
    /// deadline bounds the wait).
    pub async fn dispatch(&self) -> &SetDispatch {
        loop {
            // Register before re-checking so a `set` between the check and the
            // await cannot be missed.
            let notified = self.dispatch_set.notified();
            if let Some(d) = self.dispatch.get() {
                return d;
            }
            notified.await;
        }
    }

    /// Lock the state. Never hold across an `await`.
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A live relay conn as the uplink and the actor see it.
pub struct ConnHandle {
    /// Device static once a session is established (pairing committed).
    pub device: Option<[u8; 32]>,
    /// Relay → conn task bytes (`DATA` payloads).
    pub inbound: mpsc::Sender<Bytes>,
    /// Cancelling makes the conn task send `CLOSE 0` and stop.
    pub cancel: CancellationToken,
}

/// Mutable plugin state.
pub struct State {
    /// `link:` section (kept current by `Enable`/`Rename`).
    pub cfg: LinkConfig,
    /// Fallback display name.
    pub hostname: String,
    /// `None` until enabled once.
    pub identity: Option<Identity>,
    /// Paired devices (engine and mesh pairings share the replicated table).
    pub devices: Devices,
    /// The single pending pairing of the ENGINE identity (nodes, doc guests).
    pub pairing: Option<Pending>,
    /// The replicated `pending` rows of the MESH identity (69 decision 4).
    pub pending: PendingRows,
    /// The mesh identity (`mesh/device_key`, 69 decision 1): loaded on first
    /// use (`device_pair_start`, lease gain); the same on every engine.
    pub mesh_identity: Option<Identity>,
    /// Its uplink as `link_status` shows it: `None` until the identity was
    /// loaded; `connected` is kept by the mesh uplink supervisor.
    pub mesh: Option<MeshLink>,
    /// Uplink registered with the relay.
    pub connected: bool,
    /// Conns of the current ENGINE uplink by relay conn id.
    pub conns: HashMap<u32, ConnHandle>,
    /// Conns of the current MESH uplink (69 §2) by relay conn id — a second
    /// socket, so a second id space.
    pub mesh_conns: HashMap<u32, ConnHandle>,
    /// Guest handshakes admitted lately (59 §8.3 rate limit): tokens left and when they were last refilled.
    pub guests: Bucket,
}

/// A token bucket refilled continuously to `cfg.guest_handshakes_per_minute` per minute.
#[derive(Clone, Copy, Debug)]
pub struct Bucket {
    tokens: f64,
    last: std::time::Instant,
}

impl Default for Bucket {
    fn default() -> Self {
        Self {
            tokens: f64::NAN,
            last: std::time::Instant::now(),
        }
    }
}

impl Bucket {
    /// Take one token at `now`; `false` when the minute's budget is spent. A fresh bucket is full.
    pub fn take(&mut self, per_minute: u32, now: std::time::Instant) -> bool {
        let cap = f64::from(per_minute);
        if self.tokens.is_nan() {
            self.tokens = cap;
        }
        let refill = now.saturating_duration_since(self.last).as_secs_f64() * cap / 60.0;
        self.tokens = (self.tokens + refill).min(cap);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

impl State {
    /// Effective display name.
    pub fn name(&self) -> &str {
        if self.cfg.name.is_empty() {
            &self.hostname
        } else {
            &self.cfg.name
        }
    }

    /// A session conn for `key` is open on either uplink.
    pub fn online(&self, key: &[u8; 32]) -> bool {
        self.all_conns().any(|c| c.device.as_ref() == Some(key))
    }

    /// Every live conn of both uplinks.
    pub fn all_conns(&self) -> impl Iterator<Item = &ConnHandle> {
        self.conns.values().chain(self.mesh_conns.values())
    }

    /// The pending pairing if it has not expired.
    pub fn live_pairing(&self) -> Option<&Pending> {
        self.pairing.as_ref().filter(|p| p.live())
    }

    /// `link_status`.
    pub fn status(&self) -> LinkStatus {
        LinkStatus {
            enabled: self.cfg.enabled,
            relay: self.cfg.relay.clone(),
            name: self.name().to_owned(),
            harness_id: self.identity.as_ref().map(|i| i.id.clone()),
            connected: self.connected,
            devices: self.devices.rows().map(|r| r.device(self.online(&r.key))).collect(),
            pairing: self.live_pairing().map(Pending::pairing),
            mesh: self.mesh.clone(),
        }
    }
}
