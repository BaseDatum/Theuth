//! The shipping worker: one [`Node`] per engine, a dialer per connected peer
//! (`PeerConnected` → open a `theuth/sessions/1` stream, back off 1 s → 60 s
//! on failure, as `replica::sync::dial`), accepted streams served by the
//! same protocol, and the prune timer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::store::Store;
use parking_lot::Mutex;
use replica::{ByteStream, PeerId, Transport, ALPN_SESSIONS};
use runtime::Runtime;
use tokio::task::JoinHandle;

use crate::sync::{run_from, run_stream, Node, Stream};
use crate::types::Msg;
use crate::SessionsConfig;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Owns the streams of this engine.
pub struct Shipper {
    node: Arc<Node>,
    dialers: Mutex<HashMap<PeerId, JoinHandle<()>>>,
    _prune: JoinHandle<()>,
}

impl Shipper {
    /// Start over `store`; events go to `rt` when given. Starts the prune timer.
    pub fn new(store: Store, rt: Option<Runtime>, config: &SessionsConfig) -> Arc<Self> {
        let node = Node::new(store, rt);
        let prune = crate::prune::start(node.clone(), config.orphan_keep_days);
        Arc::new(Self {
            node,
            dialers: Mutex::default(),
            _prune: prune,
        })
    }

    /// The shared node.
    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }

    /// A lone engine (no engine id) has nothing to ship.
    fn lone(&self) -> bool {
        self.node.me.0.is_empty()
    }

    /// Keep one dialer to `peer` alive. Only the lower id of a pair dials —
    /// one stream per pair keeps an origin's rows in order on the receiver;
    /// the higher id serves the accepted stream.
    pub fn dial(&self, transport: Arc<dyn Transport>, peer: PeerId) {
        if self.lone() || self.node.me >= peer {
            return;
        }
        let mut dialers = self.dialers.lock();
        if dialers.get(&peer).is_some_and(|h| !h.is_finished()) {
            return;
        }
        let node = self.node.clone();
        let p = peer.clone();
        dialers.insert(
            peer,
            tokio::spawn(async move {
                let mut backoff = BACKOFF_MIN;
                loop {
                    match transport.open(&p, ALPN_SESSIONS).await {
                        Ok(stream) => {
                            backoff = BACKOFF_MIN;
                            if let Err(e) = run_stream(&node, p.clone(), stream).await {
                                tracing::debug!("sessions: stream to {p} ended: {e:#}");
                            }
                        }
                        Err(e) => tracing::debug!("sessions: open {p}: {e:#}"),
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }),
        );
    }

    /// The peer went away: stop dialing it (a reconnect dials afresh).
    pub fn hangup(&self, peer: &PeerId) {
        if let Some(h) = self.dialers.lock().remove(peer) {
            h.abort();
        }
    }

    /// Serve a stream a peer opened to us: read their first frame, then
    /// [`Self::serve_with_first`].
    pub async fn serve(&self, peer: PeerId, stream: Box<dyn ByteStream>) -> anyhow::Result<()> {
        let mut framed = crate::events::framed(stream);
        let first = futures::StreamExt::next(&mut framed).await.ok_or_else(|| anyhow::anyhow!("closed before hello"))??;
        self.serve_with_first(peer, framed, Msg::decode(&first)?).await
    }

    /// Serve an accepted stream whose first frame the dispatcher already read
    /// (it was not an events subscription).
    pub async fn serve_with_first(&self, peer: PeerId, framed: Stream, first: Msg) -> anyhow::Result<()> {
        if self.lone() {
            return Ok(());
        }
        run_from(&self.node, peer, framed, first).await
    }

    /// `ShipNow`: every stream re-checks the log.
    pub fn ship_now(&self) {
        self.node.kick();
    }
}

impl Drop for Shipper {
    fn drop(&mut self) {
        for (_, h) in self.dialers.lock().drain() {
            h.abort();
        }
        self._prune.abort();
    }
}
