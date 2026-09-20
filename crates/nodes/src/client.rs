//! jsonrpsee's core client over a mode-0 [`Channel`] (32 §6.3): one JSON-RPC
//! message per record, typed by method name without `rpc::Api`.

use std::time::Duration;

use jsonrpsee::core::client::{
    Client, ClientBuilder, ClientT, ReceivedMessage, TransportReceiverT, TransportSenderT,
};
use jsonrpsee::rpc_params;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::conn::Channel;

/// Every `Api` method this plugin calls on a node; a test checks each is in
/// `bindings/methods.json` so a rename fails a test instead of a node.
pub const METHODS: [&str; 4] = ["info", "link_pair", "link_revoke_device", "link_status"];

/// Per-request deadline over the relay.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The fields of `rpc::Info` a parent reads (32 §6.6); the rest is ignored.
#[derive(Clone, Debug, Deserialize)]
pub struct HarnessInfo {
    /// `theuth` version.
    pub version: String,
    /// `std::env::consts::OS` of the node.
    pub platform: String,
    /// The node's own display name.
    #[serde(default)]
    pub name: String,
}

/// The pump ended (relay closed, node gone, or the client was dropped).
#[derive(Debug, thiserror::Error)]
#[error("node conn closed")]
pub struct Closed;

struct Sender(mpsc::Sender<Vec<u8>>);

impl TransportSenderT for Sender {
    type Error = Closed;

    async fn send(&mut self, msg: String) -> Result<(), Closed> {
        self.0.send(msg.into_bytes()).await.map_err(|_| Closed)
    }
}

struct Receiver(mpsc::Receiver<Vec<u8>>);

impl TransportReceiverT for Receiver {
    type Error = Closed;

    async fn receive(&mut self) -> Result<ReceivedMessage, Closed> {
        self.0
            .recv()
            .await
            .map(ReceivedMessage::Bytes)
            .ok_or(Closed)
    }
}

/// A node's `Api` over its control conn. Dropping it closes the conn.
pub struct Rpc {
    client: Client,
}

impl Rpc {
    /// Start the pump and the client over a finished mode-0 conn.
    pub fn new(channel: Channel) -> Self {
        let (tx, rx, _pump) = channel.split();
        let client = ClientBuilder::default()
            .request_timeout(REQUEST_TIMEOUT)
            .build_with_tokio(Sender(tx), Receiver(rx));
        Self { client }
    }

    /// Resolves when the conn is gone.
    pub async fn closed(&self) {
        let _ = self.client.on_disconnect().await;
    }

    /// `info()`.
    pub async fn info(&self) -> Result<HarnessInfo, String> {
        self.call(METHODS[0], rpc_params![]).await
    }

    /// `link_pair()` on the node (32 §4.4).
    pub async fn link_pair(&self) -> Result<link::Pairing, String> {
        self.call(METHODS[1], rpc_params![]).await
    }

    /// `link_revoke_device(id)` on the node (32 §8.4).
    pub async fn link_revoke_device(&self, id: &str) -> Result<(), String> {
        self.call(METHODS[2], rpc_params![id]).await
    }

    /// `link_status()`: the cheapest request; keeps the relay's idle timer off
    /// the control conn and proves the node still answers.
    pub async fn link_status(&self) -> Result<(), String> {
        self.call::<serde_json::Value>(METHODS[3], rpc_params![])
            .await
            .map(drop)
    }

    async fn call<R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: jsonrpsee::core::params::ArrayParams,
    ) -> Result<R, String> {
        self.client
            .request(method, params)
            .await
            .map_err(|e| format!("{method}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn methods_exist_in_bindings() {
        let names: Vec<String> =
            serde_json::from_str(include_str!("../../../bindings/methods.json")).unwrap();
        for m in super::METHODS {
            assert!(
                names.iter().any(|n| n == m),
                "{m} missing from bindings/methods.json"
            );
        }
    }
}
