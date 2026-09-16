//! The model's path to a node (32 §6.5): one mode-1 conn per remote child,
//! handed to `agent` as an [`acp::Dialer`] through `RegisterNode`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use acp::LineDuplex;
use futures::{SinkExt, StreamExt};
use link::identity::Identity;
use tokio_util::sync::PollSender;

use crate::conn::{self, Channel};

/// Presence wait on a fresh dial socket: the control conn already says online,
/// so the relay answers `PRESENCE 1` at once unless the node just vanished.
const PRESENCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Dials one node. Cloned into `agent` once per `RegisterNode`.
pub struct NodeDialer {
    /// Display name for error text.
    pub name: String,
    /// `ws(s)://host[:port]`.
    pub relay: String,
    /// The node's static.
    pub harness_pub: [u8; 32],
    /// Our static (the device key on the node).
    pub identity: Arc<Identity>,
    /// Set by the control conn task.
    pub online: Arc<AtomicBool>,
    /// Live mode-1 conns; `Node.children`.
    pub children: Arc<AtomicU32>,
}

impl fmt::Debug for NodeDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeDialer({})", self.name)
    }
}

#[async_trait::async_trait]
impl acp::Dialer for NodeDialer {
    async fn dial(&self) -> Result<LineDuplex, String> {
        if !self.online.load(Ordering::Relaxed) {
            return Err(format!("{} is offline", self.name));
        }
        let mut sock = conn::open(&self.relay, &self.harness_pub)
            .await
            .map_err(|e| format!("{}: {e:#}", self.name))?;
        match tokio::time::timeout(PRESENCE_TIMEOUT, conn::presence(&mut sock)).await {
            Ok(Some(true)) => {}
            Ok(Some(false)) | Err(_) => return Err(format!("{} is offline", self.name)),
            Ok(None) => return Err(format!("{}: relay closed the socket", self.name)),
        }
        let (transport, _) = conn::handshake(&mut sock, &self.identity, &self.harness_pub, 1)
            .await
            .map_err(|e| format!("{}: {e}", self.name))?;
        let (tx, mut rx, pump) = Channel::new(sock, transport).split();
        self.children.fetch_add(1, Ordering::Relaxed);
        let children = self.children.clone();
        tokio::spawn(async move {
            let _ = pump.await;
            children.fetch_sub(1, Ordering::Relaxed);
        });
        let outgoing = PollSender::new(tx)
            .sink_map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            .with(|s: String| std::future::ready(Ok::<_, std::io::Error>(s.into_bytes())));
        let incoming = futures::stream::poll_fn(move |cx| rx.poll_recv(cx)).map(|b| {
            String::from_utf8(b)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        });
        Ok(LineDuplex {
            outgoing: Box::pin(outgoing),
            incoming: Box::pin(incoming),
        })
    }
}
