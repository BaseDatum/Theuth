//! Sync over the mesh (67 §5.1): on every connected peer a `theuth/records`
//! stream carries length-delimited frames (`tokio_util` `LengthDelimitedCodec`),
//! each one tag byte + postcard: `0` = [`Hello`] with the sender's version
//! vector (highest `seq` held per origin), `1` = a [`ChangeBatch`]. Both
//! sides send `Hello`, then every batch the other lacks (own and relayed —
//! full-mesh catch-up), then stay live: every batch that lands on the feed
//! goes to every open stream whose peer does not have it yet.
//!
//! Both ends dial on `Connected`, so a pair may have two streams; the
//! per-remote version vector on the engine makes each batch go once. Errors
//! end the stream; the dialer backs off 1 s → 60 s and reconnects, and the
//! `Hello` exchange resynchronizes. A lagging feed receiver is treated as an
//! error for the same reason. Local writes never wait on a stream: capture
//! publishes to a broadcast channel and moves on.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::ALPN_RECORDS;
use crate::engine::Engine;
use crate::types::{ByteStream, ChangeBatch, PeerEvent, PeerId, Transport};

const TAG_HELLO: u8 = 0;
const TAG_BATCH: u8 = 1;
const MAX_FRAME: usize = 64 * 1024 * 1024;
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// 182 M2a: peers this process has caught up with — every batch their
/// `Hello` said they held has landed here.
static CAUGHT_UP: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// 182 M2a: this process holds everything `peer` held when one of its
/// streams opened (its first sync since the start is complete) — and no
/// change it received waits parked for a table whose plugin has not attached
/// yet (those land at `attach`).
pub fn caught_up_with(peer: &str) -> bool {
    CAUGHT_UP.lock().contains(peer) && AWAITING.lock().is_empty()
}

/// 182: tables with changes parked before their plugin attached; emptied by
/// each `attach` and, for tables no plugin here attaches, by [`plugins_started`].
static AWAITING: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Every plugin has started: later parks for an unattached table are not awaited.
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A change of `table` parked before its plugin attached.
pub(crate) fn await_attach(table: &str) {
    if !STARTED.load(std::sync::atomic::Ordering::Relaxed) {
        AWAITING.lock().insert(table.to_owned());
    }
}

/// `tables` attached; their parked changes were retried.
pub(crate) fn attached(tables: &[&str]) {
    let mut awaiting = AWAITING.lock();
    for t in tables {
        awaiting.remove(*t);
    }
}

/// The engine started every plugin: a table still unattached has no plugin
/// here — its parked changes wait out the TTL and no longer hold the first sync.
pub fn plugins_started() {
    STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
    let left: Vec<String> = AWAITING.lock().drain().collect();
    if !left.is_empty() {
        tracing::info!("replica: no plugin here attaches {left:?}; their changes stay parked");
    }
}

/// Note `remote` caught up once our version vector covers what its `Hello`
/// announced (our own origin aside). `true` once it has.
async fn note_caught_up(
    engine: &Engine,
    remote: &PeerId,
    wanted: &HashMap<PeerId, u64>,
) -> anyhow::Result<bool> {
    let have = engine.store.version_vector().await?;
    let done = wanted
        .iter()
        .all(|(o, s)| *o == engine.origin || have.get(o).copied().unwrap_or(0) >= *s);
    if done && CAUGHT_UP.lock().insert(remote.0.clone()) {
        tracing::info!("replica: caught up with {remote}");
    }
    Ok(done)
}

/// First frame each way.
#[derive(Serialize, Deserialize)]
struct Hello {
    origin: PeerId,
    version_vector: HashMap<PeerId, u64>,
}

impl Engine {
    /// Start syncing over `transport`: accept `theuth/records` streams, dial
    /// every connected peer now and on each `Connected`.
    pub fn register_transport(self: &Arc<Self>, transport: Arc<dyn Transport>) {
        *self.transport.lock() = Some(transport.clone());
        self.set_mesh_engine();
        let engine = self.clone();
        let t = transport.clone();
        tokio::spawn(async move {
            loop {
                match t.accept(ALPN_RECORDS).await {
                    Ok((peer, stream)) => {
                        let engine = engine.clone();
                        tokio::spawn(async move {
                            if let Err(e) = run_stream(&engine, stream, &peer).await {
                                tracing::warn!("replica: stream from {peer} ended: {e:#}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("replica: accept: {e:#}");
                        tokio::time::sleep(BACKOFF_MIN).await;
                    }
                }
            }
        });

        let engine = self.clone();
        tokio::spawn(async move {
            let mut events = transport.subscribe();
            let mut dialers: HashMap<PeerId, tokio::task::JoinHandle<()>> = HashMap::new();
            let ensure = |dialers: &mut HashMap<PeerId, tokio::task::JoinHandle<()>>,
                          peer: PeerId| {
                if dialers.get(&peer).is_some_and(|h| !h.is_finished()) {
                    return;
                }
                let (engine, t) = (engine.clone(), transport.clone());
                let p = peer.clone();
                dialers.insert(peer, tokio::spawn(async move { dial(engine, t, p).await }));
            };
            for p in transport.peers() {
                ensure(&mut dialers, p);
            }
            loop {
                match events.recv().await {
                    Ok(PeerEvent::Connected(p)) => ensure(&mut dialers, p),
                    Ok(PeerEvent::Disconnected(p)) => {
                        if let Some(h) = dialers.remove(&p) {
                            h.abort();
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        for p in transport.peers() {
                            ensure(&mut dialers, p);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// `true` once per (remote, batch): records that `remote` holds it.
    pub(crate) fn mark_sent(&self, remote: &PeerId, batch: &ChangeBatch) -> bool {
        let mut vv = self.remote_vv.lock();
        let held = vv
            .entry(remote.clone())
            .or_default()
            .entry(batch.origin.clone())
            .or_insert(0);
        if batch.seq > *held {
            *held = batch.seq;
            true
        } else {
            false
        }
    }

    /// A `Hello` is authoritative about what `remote` holds: replace, never
    /// merge, so an over-estimate from a failed send cannot starve the peer.
    pub(crate) fn learn(&self, remote: &PeerId, theirs: &HashMap<PeerId, u64>) {
        self.remote_vv.lock().insert(remote.clone(), theirs.clone());
    }
}

async fn dial(engine: Arc<Engine>, t: Arc<dyn Transport>, peer: PeerId) {
    let mut backoff = BACKOFF_MIN;
    loop {
        match t.open(&peer, ALPN_RECORDS).await {
            Ok(stream) => {
                backoff = BACKOFF_MIN;
                if let Err(e) = run_stream(&engine, stream, &peer).await {
                    tracing::warn!("replica: stream to {peer} ended: {e:#}");
                }
            }
            Err(e) => tracing::debug!("replica: open {peer}: {e:#}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

fn frame(tag: u8, body: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(1 + body.len());
    b.extend_from_slice(&[tag]);
    b.extend_from_slice(body);
    b.freeze()
}

/// The symmetric protocol on one stream, opened or accepted; `link` is the
/// transport's peer (the mesh engine id), `Hello.origin` the replica's.
async fn run_stream(
    engine: &Arc<Engine>,
    stream: Box<dyn ByteStream>,
    link: &PeerId,
) -> anyhow::Result<()> {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME)
        .new_codec();
    let mut framed = Framed::new(stream, codec);
    // Subscribe before snapshotting the vector so nothing falls between them.
    let mut feed = engine.subscribe();

    let hello = Hello {
        origin: engine.origin.clone(),
        version_vector: engine.store.version_vector().await?,
    };
    framed
        .send(frame(TAG_HELLO, &postcard::to_allocvec(&hello)?))
        .await?;
    let first = framed
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("closed before hello"))??;
    anyhow::ensure!(first.first() == Some(&TAG_HELLO), "expected hello");
    let theirs: Hello = postcard::from_bytes(&first[1..])?;
    let remote = theirs.origin.clone();
    engine.learn(&remote, &theirs.version_vector);
    let mut caught = note_caught_up(engine, link, &theirs.version_vector).await?;

    // Catch-up: everything we hold above what they hold, in seq order per origin. A peer whose
    // point lies below the oldest batch still held of some origin (a newcomer, an engine away
    // longer than the prune) cannot be rebuilt from batches: it gets the whole state first as
    // one batch, then the tail (whose columns then lose LWW to the same clocks — no partial rows).
    let mut behind = false;
    for (origin, seq) in &hello.version_vector {
        let after = theirs.version_vector.get(origin).copied().unwrap_or(0);
        if *seq > after
            && engine
                .store
                .oldest_seq(origin)
                .await?
                .is_some_and(|oldest| after + 1 < oldest)
        {
            behind = true;
        }
    }
    if behind && let Some(batch) = engine.snapshot_batch().await? {
        tracing::info!(
            "replica: {remote} is behind the pruned outbox; sent a snapshot of {} change(s) as seq {}",
            batch.changes.len(),
            batch.seq
        );
        if engine.mark_sent(&remote, &batch) {
            framed
                .send(frame(TAG_BATCH, &postcard::to_allocvec(&batch)?))
                .await?;
        }
    }
    for (origin, seq) in &hello.version_vector {
        let after = theirs.version_vector.get(origin).copied().unwrap_or(0);
        if *seq <= after {
            continue;
        }
        for bytes in engine.store.batches_after(origin, after).await? {
            let batch: ChangeBatch = postcard::from_bytes(&bytes)?;
            if engine.mark_sent(&remote, &batch) {
                if batch.origin != engine.origin {
                    engine.count_relayed(&batch.changes);
                }
                framed.send(frame(TAG_BATCH, &bytes)).await?;
            }
        }
    }

    loop {
        tokio::select! {
            inbound = framed.next() => {
                let Some(bytes) = inbound else { return Ok(()) };
                let bytes = bytes?;
                match bytes.first() {
                    Some(&TAG_BATCH) => {
                        let batch: ChangeBatch = postcard::from_bytes(&bytes[1..])?;
                        engine.mark_sent(&remote, &batch);
                        engine.apply(&batch).await?;
                        if !caught {
                            caught = note_caught_up(engine, link, &theirs.version_vector).await?;
                        }
                    }
                    Some(&TAG_HELLO) => {}
                    other => anyhow::bail!("bad frame tag {other:?}"),
                }
            }
            outbound = feed.recv() => {
                match outbound {
                    Ok(batch) => {
                        if engine.mark_sent(&remote, &batch) {
                            if batch.origin != engine.origin {
                                engine.count_relayed(&batch.changes);
                            }
                            framed.send(frame(TAG_BATCH, &postcard::to_allocvec(&batch)?)).await?;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => anyhow::bail!("feed lagged by {n}; resyncing"),
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }
}
