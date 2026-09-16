//! State shared by the actor, the uplink task and every conn task.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;
use runtime::Runtime;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::MethodsFactory;
use crate::devices::Devices;
use crate::identity::Identity;
use crate::pairing::Pending;
use crate::types::{LinkConfig, LinkStatus};

/// Everything a conn task needs: the runtime (events), the lazily built
/// jsonrpsee methods and the mutable [`State`].
pub struct Ctx {
    /// Publishes [`crate::LinkEvent`]s.
    pub rt: Runtime,
    factory: MethodsFactory,
    methods: tokio::sync::OnceCell<jsonrpsee::Methods>,
    state: Mutex<State>,
}

impl Ctx {
    /// Wrap.
    pub fn new(rt: Runtime, factory: MethodsFactory, state: State) -> Arc<Self> {
        Arc::new(Self {
            rt,
            factory,
            methods: tokio::sync::OnceCell::new(),
            state: Mutex::new(state),
        })
    }

    /// The methods object, built on first use.
    pub async fn methods(&self) -> &jsonrpsee::Methods {
        self.methods
            .get_or_init(|| (self.factory)(self.rt.clone()))
            .await
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
    /// Paired devices.
    pub devices: Devices,
    /// The single pending pairing.
    pub pairing: Option<Pending>,
    /// Uplink registered with the relay.
    pub connected: bool,
    /// Conns of the current uplink by relay conn id.
    pub conns: HashMap<u32, ConnHandle>,
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

    /// A session conn for `key` is open.
    pub fn online(&self, key: &[u8; 32]) -> bool {
        self.conns.values().any(|c| c.device.as_ref() == Some(key))
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
            devices: self
                .devices
                .rows()
                .iter()
                .map(|r| r.device(self.online(&r.key)))
                .collect(),
            pairing: self.live_pairing().map(Pending::pairing),
        }
    }
}
