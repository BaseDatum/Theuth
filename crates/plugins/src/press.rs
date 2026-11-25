//! Owner: B2 — the human press (216 D14, Q3; contract §8). B0 wrote it complete.

use crate::error::PluginsError;

/// Proof that a request is a human press (216 D14; 171 §12.1 invariant 1): an authenticated RPC
/// peer that is not a share guest, not the engine-internal peer, not closed, and not forwarded
/// from another engine. Only `from_rpc` builds one; it is not `Deserialize`.
#[derive(Clone, Debug)]
pub struct HumanPress {
    peer: u64,
    client: String,
    remote: bool,
}

impl HumanPress {
    /// Admit the RPC call behind `ext` as a press, or `PluginsError::NotAPress`.
    pub fn from_rpc(ext: &jsonrpsee::Extensions) -> Result<Self, PluginsError> {
        let Some(p) = ext.get::<rpc_core::RpcPeer>() else {
            return Err(PluginsError::NotAPress);
        };
        if p.guest().is_some() || p.is_internal() || p.is_closed() {
            return Err(PluginsError::NotAPress);
        }
        if ext.get::<agent::rpc_forward::Forwarded>().is_some() {
            return Err(PluginsError::NotAPress);
        }
        let client = ext
            .get::<rpc_core::Client>()
            .map(|c| c.0)
            .unwrap_or("desktop")
            .to_owned();
        Ok(Self {
            peer: p.id(),
            client,
            remote: p.remote(),
        })
    }

    /// `x-theuth-client` of the pressing connection (`desktop`, `ios`, …; the CLI reports `desktop`).
    pub fn client(&self) -> &str {
        &self.client
    }

    /// The connection id.
    pub fn peer(&self) -> u64 {
        self.peer
    }

    /// A relay (phone/web) connection rather than this machine.
    pub fn remote(&self) -> bool {
        self.remote
    }

    /// A press for unit tests (B1's host test approves with it).
    #[cfg(test)]
    #[allow(dead_code)] // B1/B2 tests use it
    pub(crate) fn for_test() -> Self {
        Self {
            peer: 0,
            client: "desktop".into(),
            remote: false,
        }
    }
}
