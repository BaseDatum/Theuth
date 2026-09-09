//! RPC plugin. Methods are `ask`s on named actors; `subscribe_events` streams
//! every wire-visible bus event as `{"type": ..., ...}`. See design doc §7.

use serde::{Deserialize, Serialize};

/// `[rpc]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RpcConfig {
    /// Bind address; loopback only by design.
    pub listen: String,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self { listen: "127.0.0.1:7777".into() }
    }
}

/// The plugin. Reads `<data_dir>/token` (creating it) and requires it as
/// `Authorization: Bearer <token>` on the WebSocket upgrade.
pub struct RpcPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for RpcPlugin {
    fn name(&self) -> &'static str {
        "rpc"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let _ = rt;
        todo!("rpc implementation")
    }
}
