//! The `theuth/sessions/1` protocol on one stream (68 §6), shaped like
//! `replica::sync`: both sides send [`Msg::Hello`] with the origins they hold
//! (own log head, applied cursors); each then answers per origin — a
//! [`Msg::Snapshot`] when the peer has no cursor for it or when we hold no
//! log to replay (a relayed origin, or our log pruned past their cursor),
//! else [`Msg::Log`] rows after their cursor — and stays live: own rows as
//! the store logs them, other origins' rows as we apply them (relay), acks
//! back for every batch. The lower id of a pair dials, so a pair normally
//! has one stream and rows arrive in order; a relay beside the direct link
//! or a reconnect overlap may still deliver ahead, which the per-origin
//! reorder buffer absorbs. 183 R1 (E-M3): `Hello` carries the sender's agent
//! schema; a peer on an older one (or none) is shipped no log row or
//! snapshot, and its cursors do not move, until it reconnects current.
//!
//! After the `Hello`s the stream is two halves run side by side: a reader
//! that applies and acks what arrives, and a writer that ships (catch-up,
//! then live). Neither waits on the other — a catch-up one way never stops
//! the other way's rows from being read (both sides shipping a backlog
//! larger than the transport's window at once used to wedge the stream
//! forever). A shipping failure of one origin is reported and retried on
//! [`HARNESS_RETRY`]; a broken stream, a gap, or a row that fails to apply
//! ends the stream (the row's origin is then asked for a snapshot) and the
//! dialer resynchronizes. Every failure is logged at WARN, rate-limited per
//! (peer, phase, kind), and kept for [`Node::sync_status`].

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use agent::AgentEvent;
use agent::store::Store;
use anyhow::Context as _;
use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::ActorRef;
use parking_lot::Mutex;
use replica::{ByteStream, PeerId};
use runtime::{Runtime, SessionId};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Notify, OwnedMutexGuard, broadcast, mpsc};
use tokio_util::codec::{Framed, FramedParts, LengthDelimitedCodec};

use crate::apply::{Applied, apply_log_row};
use crate::snapshot::{self, AppliedSession};
use crate::types::{LogRow, MAX_FRAME, Msg, PeerSync, schema_behind, schema_label};

/// Rows per [`Msg::Log`] frame during catch-up (and per log page read);
/// also how many rows of a relayed origin may wait behind a gap.
pub const BATCH_ROWS: usize = 5000;
/// Changeset bytes per [`Msg::Log`] frame during catch-up: small frames
/// keep a slow link moving (a single bigger row still goes alone).
pub const BATCH_BYTES: usize = 4 * 1024 * 1024;
/// Largest changeset one log row may carry in a [`Msg::Log`] frame; a
/// bigger row (a fork of a long session) is shipped as a snapshot instead.
pub const ROW_MAX: usize = MAX_FRAME - 1024 * 1024;
/// Relay feed depth before a slow stream re-checks the relayed origins.
const RELAY_CAPACITY: usize = 1024;
/// How long a gap before out-of-order rows may stay open before the stream resyncs.
const GAP_WAIT: Duration = Duration::from_secs(10);
/// Outbox re-check period while harness rows remain (139 R2 D3); also when
/// an origin whose shipping failed is retried.
const HARNESS_RETRY: Duration = Duration::from_secs(60);
/// How often a waiting send checks whether the transport took any byte.
const SEND_TICK: Duration = Duration::from_secs(1);
/// No byte taken by the peer for this long ends the stream.
const SEND_STALL: Duration = Duration::from_secs(300);
/// No byte taken for this long: a WARN (peer, phase, bytes, seconds), once per stall.
const SEND_WARN: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 30 });
/// Repeats of one failure (peer, phase, kind) log at most this often.
const REPORT_EVERY: Duration = Duration::from_secs(10 * 60);
/// Failures to apply the same row before a snapshot of its origin is asked for.
const APPLY_TRIES: u32 = 3;
/// At most one snapshot asked per origin this often; else retried from the cursor.
const RESNAP_EVERY: Duration = Duration::from_secs(6 * 3600);

/// One `theuth/sessions/1` stream, framed (see [`crate::events::framed`]).
pub type Stream = Framed<Box<dyn ByteStream>, LengthDelimitedCodec>;

/// Where a sessions stream failed; the context of a stream's error and
/// part of the rate-limit key of its report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Opening the stream.
    Open,
    /// The `Hello` exchange.
    Hello,
    /// Shipping what the peer missed while away.
    CatchUp,
    /// Shipping our rows live.
    Ship,
    /// Building or sending a snapshot.
    Snapshot,
    /// The harness outbox (139 R2).
    Harness,
    /// Reading a frame.
    Receive,
    /// Applying a peer's row or snapshot.
    Apply,
    /// Rows missing before rows that arrived ahead.
    Gap,
    /// The stream as a whole.
    Stream,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Hello => "hello",
            Self::CatchUp => "catch-up",
            Self::Ship => "ship",
            Self::Snapshot => "snapshot",
            Self::Harness => "harness",
            Self::Receive => "receive",
            Self::Apply => "apply",
            Self::Gap => "gap resync",
            Self::Stream => "stream",
        })
    }
}

/// A send on the stream failed or stalled: the stream is gone, unlike a
/// failure to build what one origin needs.
#[derive(Debug)]
struct Broken(String);

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream broken: {}", self.0)
    }
}

impl std::error::Error for Broken {}

/// `e` with phase `p`, unless it already names one.
fn phased(e: anyhow::Error, p: Phase) -> anyhow::Error {
    if e.downcast_ref::<Phase>().is_some() {
        e
    } else {
        e.context(p)
    }
}

/// What every stream of this engine shares: the store, who we are, the
/// relay feed of applied foreign rows, and what each peer is known to hold.
pub struct Node {
    /// The durable store.
    pub store: Store,
    /// Our origin (`Store::engine`); empty on a lone engine, which never ships.
    pub me: PeerId,
    rt: Option<Runtime>,
    /// Rows of other origins applied here, for streams to forward.
    relay: broadcast::Sender<(PeerId, LogRow)>,
    /// `ShipNow`: every stream re-checks the log.
    kick: Notify,
    /// Per remote, the highest seq per origin it holds (its `Hello`, then
    /// advanced by what went each way).
    held: Mutex<HashMap<PeerId, HashMap<PeerId, u64>>>,
    /// `(session, epoch)` pairs already warned about as stale.
    stale_logged: Mutex<HashSet<(String, u32)>>,
    /// Per origin: applies of its rows and snapshots are serialized so a
    /// multi-part snapshot cannot interleave with newer log rows.
    origins: Mutex<HashMap<PeerId, Arc<tokio::sync::Mutex<()>>>>,
    /// Per origin: rows that arrived ahead of the cursor, and since when.
    pending: Mutex<HashMap<PeerId, Ahead>>,
    /// Origins whose cursor the next `Hello` omits, which asks the peer for
    /// a snapshot (it supersedes a row that keeps failing to apply); taken
    /// by that `Hello`, so one request each.
    resnap: Mutex<HashSet<PeerId>>,
    /// Per origin: the row that last failed to apply here ([`Node::apply_failed`]).
    fails: Mutex<HashMap<PeerId, Fails>>,
    /// Per remote: the sync-status surface ([`Node::sync_status`]).
    status: Mutex<HashMap<PeerId, Track>>,
    /// Failure reports: `(peer, phase, kind)` → when last logged, repeats since.
    reports: Mutex<HashMap<ReportKey, (Instant, u64)>>,
    /// `sessions.harness_outbox_ttl_secs`: an outbox row older than this is dropped (139 R2 D3).
    harness_ttl: Duration,
    /// 183 R1 (E-M3): this binary's agent schema version, sent in `Hello`.
    schema: AtomicU32,
    /// Tests: origins whose shipping fails on purpose.
    #[cfg(test)]
    fail_ship: Mutex<HashSet<PeerId>>,
}

/// Rows from one origin that arrived ahead of the cursor, keyed by seq, and when the first did.
type Ahead = (Instant, BTreeMap<u64, LogRow>);

/// A failure report's rate-limit key: peer, phase, the error with its digits dropped.
type ReportKey = (PeerId, Phase, String);

/// An origin's row failing to apply: its seq, failures of it in a row, and
/// when a snapshot of the origin was last asked for.
#[derive(Default)]
struct Fails {
    seq: u64,
    count: u32,
    asked: Option<Instant>,
}

/// One remote's entry of the sync-status surface; `streams` counts the
/// live streams with it (a reconnect may overlap the old one).
#[derive(Default)]
struct Track {
    streams: u32,
    sync: PeerSync,
}

fn now_text() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Node {
    /// A node over `store`; `rt` publishes `AgentEvent`s when present;
    /// `harness_ttl` bounds how long a harness row waits for its owner.
    pub fn new(store: Store, rt: Option<Runtime>, harness_ttl: Duration) -> Arc<Self> {
        let me = PeerId(store.engine().to_owned());
        Arc::new(Self {
            store,
            me,
            rt,
            relay: broadcast::channel(RELAY_CAPACITY).0,
            kick: Notify::new(),
            held: Mutex::default(),
            stale_logged: Mutex::default(),
            origins: Mutex::default(),
            pending: Mutex::default(),
            resnap: Mutex::default(),
            fails: Mutex::default(),
            status: Mutex::default(),
            reports: Mutex::default(),
            harness_ttl,
            schema: AtomicU32::new(agent::store::SESSIONS_WIRE_SCHEMA),
            #[cfg(test)]
            fail_ship: Mutex::default(),
        })
    }

    fn schema(&self) -> u32 {
        self.schema.load(Ordering::Relaxed)
    }

    /// Tests: pretend to be a binary on agent schema `v` from the next stream on.
    #[cfg(test)]
    pub(crate) fn set_schema(&self, v: u32) {
        self.schema.store(v, Ordering::Relaxed);
    }

    /// Wake every stream to ship whatever is unshipped.
    pub fn kick(&self) {
        self.kick.notify_waiters();
    }

    /// Every origin we hold rows for and how far: our committed log head,
    /// then every applied cursor. Our own origin is offered even at head 0
    /// when we own sessions (a migrated store that has not written since:
    /// its peers bootstrap from a snapshot current to seq 0); a cursor row
    /// at 0 is likewise "held" — the row exists only once a snapshot landed.
    /// Origins in `omit` are left out: the peer answers with a snapshot.
    async fn held_here(&self, omit: &HashSet<PeerId>) -> anyhow::Result<Vec<(PeerId, u64)>> {
        let mut out = Vec::new();
        let head = u64::try_from(self.store.log_head()).unwrap_or(0);
        if head > 0 || self.store.owns_sessions().await? {
            out.push((self.me.clone(), head));
        }
        for c in self.store.cursors().await? {
            if c.peer.is_empty()
                && c.origin != self.me.0
                && !omit.contains(&PeerId(c.origin.clone()))
            {
                out.push((PeerId(c.origin), u64::try_from(c.seq).unwrap_or(0)));
            }
        }
        Ok(out)
    }

    /// Row `seq` of `origin` failed to apply: what the stream's error says
    /// happens next. The same seq failing [`APPLY_TRIES`] times asks for a
    /// snapshot of the origin, at most once per [`RESNAP_EVERY`]; otherwise
    /// the next stream retries from the cursor (the dialer backs off).
    fn apply_failed(&self, origin: &PeerId, seq: u64) -> String {
        let mut fails = self.fails.lock();
        let f = fails.entry(origin.clone()).or_default();
        if f.seq == seq {
            f.count += 1;
        } else {
            (f.seq, f.count) = (seq, 1);
        }
        if f.count < APPLY_TRIES {
            return format!(
                "failure {} of {APPLY_TRIES}; the next stream retries from the cursor",
                f.count
            );
        }
        if f.asked.is_some_and(|t| t.elapsed() < RESNAP_EVERY) {
            return format!(
                "failed {} times; a snapshot of {origin} was asked less than {} h ago, so the \
                 next stream retries from the cursor",
                f.count,
                RESNAP_EVERY.as_secs() / 3600
            );
        }
        (f.asked, f.count) = (Some(Instant::now()), 0);
        self.resnap.lock().insert(origin.clone());
        format!("failed {APPLY_TRIES} times; the next hello asks for a snapshot of {origin}")
    }

    /// Our applied cursor for `origin`; `None` when no snapshot or row of it
    /// ever landed here.
    async fn cursor(&self, origin: &PeerId) -> anyhow::Result<Option<u64>> {
        Ok(self
            .store
            .cursors()
            .await?
            .into_iter()
            .find(|c| c.origin == origin.0 && c.peer.is_empty())
            .map(|c| u64::try_from(c.seq).unwrap_or(0)))
    }

    /// A `Hello` is authoritative about what `remote` holds: replace.
    fn learn(&self, remote: &PeerId, theirs: &[(PeerId, u64)]) {
        self.held
            .lock()
            .insert(remote.clone(), theirs.iter().cloned().collect());
    }

    /// Record that `remote` holds `origin` up to `seq`.
    fn mark(&self, remote: &PeerId, origin: &PeerId, seq: u64) {
        let mut held = self.held.lock();
        let slot = held
            .entry(remote.clone())
            .or_default()
            .entry(origin.clone())
            .or_insert(0);
        *slot = (*slot).max(seq);
    }

    fn holds(&self, remote: &PeerId, origin: &PeerId) -> u64 {
        self.held
            .lock()
            .get(remote)
            .and_then(|m| m.get(origin))
            .copied()
            .unwrap_or(0)
    }

    /// The snapshot requests a `Hello` to `remote` carries: of `remote`'s
    /// own origin, or of an origin its last `Hello` said it holds. The rest
    /// stay pending for a peer that can answer them.
    fn take_resnap(&self, remote: &PeerId) -> HashSet<PeerId> {
        let holds: HashSet<PeerId> = self
            .held
            .lock()
            .get(remote)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut resnap = self.resnap.lock();
        let (asked, rest) = std::mem::take(&mut *resnap)
            .into_iter()
            .partition(|o| o == remote || holds.contains(o));
        *resnap = rest;
        asked
    }

    fn origin_lock(&self, origin: &PeerId) -> Arc<tokio::sync::Mutex<()>> {
        self.origins
            .lock()
            .entry(origin.clone())
            .or_default()
            .clone()
    }

    /// The pending row right after `cursor`, if any; drops what fell behind.
    fn take_pending(&self, origin: &PeerId, cursor: u64) -> Option<LogRow> {
        let mut pending = self.pending.lock();
        let (_, map) = pending.get_mut(origin)?;
        while map.first_key_value().is_some_and(|(s, _)| *s <= cursor) {
            map.pop_first();
        }
        let next = map.remove(&(cursor + 1));
        if map.is_empty() {
            pending.remove(origin);
        }
        next
    }

    async fn publish(&self, ev: AgentEvent) {
        if let Some(rt) = &self.rt {
            rt.publish(ev).await;
        }
    }

    async fn owner_changed(&self, session: SessionId, owner: String, epoch: u32) {
        let here = owner == self.me.0;
        self.publish(AgentEvent::SessionOwnerChanged {
            session,
            owner,
            epoch,
            here,
            moved_from: None,
        })
        .await;
    }

    fn track(&self, remote: &PeerId, f: impl FnOnce(&mut PeerSync)) {
        f(&mut self.status.lock().entry(remote.clone()).or_default().sync);
    }

    /// The sync-status surface: one entry per peer a stream ran with since
    /// start, plus an empty one for every `connected` peer without one.
    /// In memory only (no store read).
    pub fn sync_status(&self, connected: &[PeerId]) -> Vec<PeerSync> {
        let head = u64::try_from(self.store.log_head()).unwrap_or(0);
        let status = self.status.lock();
        let mut out: Vec<PeerSync> = status
            .iter()
            .map(|(p, t)| (p, t.sync.clone()))
            .chain(
                connected
                    .iter()
                    .filter(|p| !status.contains_key(*p))
                    .map(|p| (p, PeerSync::default())),
            )
            .map(|(p, mut s)| {
                s.peer = p.0.clone();
                s.head = head;
                s.behind_out = s.acked.map(|a| head.saturating_sub(a));
                s.behind_in = s
                    .peer_head
                    .map(|h| h.saturating_sub(s.applied.unwrap_or(0)));
                s
            })
            .collect();
        out.sort_by(|a, b| a.peer.cmp(&b.peer));
        out
    }

    /// The `mesh_peers` text for `peer` ([`PeerSync::line`]).
    pub fn peer_note(&self, peer: &str) -> Option<String> {
        self.sync_status(&[])
            .into_iter()
            .find(|s| s.peer == peer)
            .map(|s| s.line())
    }

    /// Our position against `remote`'s, for a log line.
    fn cursors_line(&self, remote: &PeerId) -> String {
        let s = self
            .sync_status(&[])
            .into_iter()
            .find(|s| s.peer == remote.0)
            .unwrap_or_default();
        let num = |v: Option<u64>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
        format!(
            "our head {}, {remote} acked {}; we applied {} of its {}",
            s.head,
            num(s.acked),
            num(s.applied),
            num(s.peer_head)
        )
    }

    /// Log a sessions-stream failure with `remote` at WARN and keep it for
    /// the status surface. The first of a (peer, phase, kind) logs at once,
    /// repeats at most every [`REPORT_EVERY`] with how many were held back;
    /// `then` says what happens next.
    pub(crate) fn report(&self, remote: &PeerId, phase: Phase, err: &anyhow::Error, then: &str) {
        let phase_name = phase.to_string();
        let text = err
            .chain()
            .map(ToString::to_string)
            .filter(|c| *c != phase_name)
            .collect::<Vec<_>>()
            .join(": ");
        self.track(remote, |s| {
            s.last_error = Some(format!("{phase}: {text}"));
            s.last_error_at = Some(now_text());
        });
        let kind: String = text
            .chars()
            .filter(|c| !c.is_ascii_digit())
            .take(120)
            .collect();
        let held_back = {
            use std::collections::hash_map::Entry;
            match self.reports.lock().entry((remote.clone(), phase, kind)) {
                Entry::Occupied(mut seen) => {
                    let (at, n) = seen.get_mut();
                    if at.elapsed() < REPORT_EVERY {
                        *n += 1;
                        return;
                    }
                    *at = Instant::now();
                    std::mem::take(n)
                }
                Entry::Vacant(first) => {
                    first.insert((Instant::now(), 0));
                    0
                }
            }
        };
        let again = if held_back > 0 {
            format!(" ({held_back} more like it since the last report)")
        } else {
            String::new()
        };
        tracing::warn!(
            "sessions: {phase} with {remote} failed: {text} [{}]; {then}{again}",
            self.cursors_line(remote)
        );
    }

    /// A stream with `remote` finished: report how (an error at WARN with its
    /// phase, a clean close at INFO), rate-limited as [`Node::report`].
    fn ended(&self, remote: &PeerId, r: &anyhow::Result<()>) {
        match r {
            Err(e) => {
                let phase = e.downcast_ref::<Phase>().copied().unwrap_or(Phase::Stream);
                self.report(remote, phase, e, "the stream ended; the dialer resyncs");
            }
            Ok(()) => tracing::info!(
                "sessions: stream with {remote} closed by the peer [{}]",
                self.cursors_line(remote)
            ),
        }
    }
}

/// While alive: a stream with `remote` is up (status surface).
struct Up<'a> {
    node: &'a Node,
    remote: PeerId,
}

impl<'a> Up<'a> {
    fn new(node: &'a Node, remote: &PeerId) -> Self {
        let mut status = node.status.lock();
        let t = status.entry(remote.clone()).or_default();
        t.streams += 1;
        if !t.sync.up {
            t.sync.up = true;
            t.sync.since = Some(now_text());
        }
        Self {
            node,
            remote: remote.clone(),
        }
    }
}

impl Drop for Up<'_> {
    fn drop(&mut self) {
        let mut status = self.node.status.lock();
        if let Some(t) = status.get_mut(&self.remote) {
            t.streams = t.streams.saturating_sub(1);
            if t.streams == 0 {
                t.sync.up = false;
                t.sync.since = Some(now_text());
            }
        }
    }
}

/// A multi-part snapshot of one origin being received on this stream.
struct InFlight {
    _guard: OwnedMutexGuard<()>,
    fenced: HashSet<String>,
    sessions: Vec<AppliedSession>,
}

/// The stream under the codec, counting the bytes the transport took: the
/// writer's progress, which the stall timer watches.
struct Counted {
    inner: Box<dyn ByteStream>,
    written: Arc<AtomicU64>,
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Counted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &r {
            self.written.fetch_add(*n as u64, Ordering::Relaxed);
        }
        r
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// `framed` over a [`Counted`] stream (buffers kept), and its byte counter.
fn counted(framed: Stream) -> (Stream, Arc<AtomicU64>) {
    let written = Arc::new(AtomicU64::new(0));
    let parts = framed.into_parts();
    let io: Box<dyn ByteStream> = Box::new(Counted {
        inner: parts.io,
        written: written.clone(),
    });
    let mut next = FramedParts::new::<Bytes>(io, parts.codec);
    next.read_buf = parts.read_buf;
    next.write_buf = parts.write_buf;
    (Framed::from_parts(next), written)
}

/// The writing half of a stream, and the replies (acks) the reading half
/// queued for it; replies go out ahead of every frame the writer sends.
struct Out {
    node: Arc<Node>,
    remote: PeerId,
    sink: SplitSink<Stream, Bytes>,
    replies: mpsc::UnboundedReceiver<Msg>,
    /// Bytes the transport took on this stream ([`Counted`]).
    written: Arc<AtomicU64>,
}

impl Out {
    /// Send the queued replies, then `msg`.
    async fn send(&mut self, msg: Msg) -> anyhow::Result<()> {
        while let Ok(reply) = self.replies.try_recv() {
            self.put(&reply).await?;
        }
        self.put(&msg).await
    }

    /// One frame. Over [`MAX_FRAME`] is refused before it reaches the codec
    /// (an error of what was being shipped, not of the stream). The timer
    /// is for no progress, not for the frame: while the transport takes
    /// bytes a frame may take as long as the link needs. No byte for
    /// [`SEND_WARN`] is reported; for [`SEND_STALL`], or a failed send, the
    /// stream is [`Broken`].
    async fn put(&mut self, msg: &Msg) -> anyhow::Result<()> {
        let frame = msg.encode()?;
        let len = frame.len();
        anyhow::ensure!(
            len <= MAX_FRAME,
            "a frame of {len} bytes (tag {}) is over the {MAX_FRAME}-byte limit",
            msg.tag()
        );
        let phase = match msg {
            Msg::Log { .. } => Phase::Ship,
            Msg::Snapshot { .. } => Phase::Snapshot,
            Msg::Harness { .. } => Phase::Harness,
            Msg::Ack { .. } | Msg::HarnessAck { .. } => Phase::Receive,
            _ => Phase::Stream,
        };
        let mut seen = (self.written.load(Ordering::Relaxed), Instant::now());
        let mut warned = false;
        let send = self.sink.send(frame);
        tokio::pin!(send);
        loop {
            match tokio::time::timeout(SEND_TICK, &mut send).await {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(e)) => return Err(Broken(e.to_string()).into()),
                Err(_) => {}
            }
            let written = self.written.load(Ordering::Relaxed);
            if written != seen.0 {
                (seen, warned) = ((written, Instant::now()), false);
                continue;
            }
            let idle = seen.1.elapsed();
            if idle >= SEND_STALL {
                return Err(Broken(format!(
                    "the peer took no byte for {} s",
                    SEND_STALL.as_secs()
                ))
                .into());
            }
            if idle >= SEND_WARN && !warned {
                warned = true;
                self.node.report(
                    &self.remote,
                    phase,
                    &anyhow::anyhow!(
                        "no byte of a {len}-byte frame (tag {}) taken for {} s, waiting for the \
                         peer to read; {written} bytes sent on this stream",
                        msg.tag(),
                        idle.as_secs(),
                    ),
                    &format!(
                        "still waiting; the stream ends after {} s without progress",
                        SEND_STALL.as_secs()
                    ),
                );
            }
        }
    }
}

/// A shipping result: `Ok(true)` shipped; a failure of what was shipped is
/// reported and `Ok(false)` (the stream goes on, the caller retries); a
/// broken stream is the stream's error, with `phase`.
fn survive(
    node: &Node,
    remote: &PeerId,
    phase: Phase,
    r: anyhow::Result<()>,
) -> anyhow::Result<bool> {
    match r {
        Ok(()) => Ok(true),
        Err(e) if e.downcast_ref::<Broken>().is_some() => Err(phased(e, phase)),
        Err(e) => {
            node.report(
                remote,
                phase,
                &e,
                "the stream goes on; retried every minute",
            );
            Ok(false)
        }
    }
}

/// The symmetric protocol on one stream to `remote`, opened or accepted.
pub async fn run_stream(
    node: &Arc<Node>,
    remote: PeerId,
    stream: Box<dyn ByteStream>,
) -> anyhow::Result<()> {
    run(node, remote, crate::events::framed(stream), None).await
}

/// [`run_stream`] on an accepted stream whose first frame (their `Hello`)
/// the dispatcher already read to tell shipping from an events subscription.
pub async fn run_from(
    node: &Arc<Node>,
    remote: PeerId,
    framed: Stream,
    first: Msg,
) -> anyhow::Result<()> {
    run(node, remote, framed, Some(first)).await
}

/// [`stream`], its end reported ([`Node::ended`]).
async fn run(
    node: &Arc<Node>,
    remote: PeerId,
    framed: Stream,
    first: Option<Msg>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !node.me.0.is_empty(),
        "lone engine (no engine id): nothing ships"
    );
    let r = stream(node, &remote, framed, first).await;
    node.ended(&remote, &r);
    r
}

async fn stream(
    node: &Arc<Node>,
    remote: &PeerId,
    mut framed: Stream,
    first: Option<Msg>,
) -> anyhow::Result<()> {
    // Subscribe before reading the cursors so nothing falls between.
    let mut relay = node.relay.subscribe();
    let mut log = node.store.log_watch();
    log.mark_unchanged();
    let mut harness = node.store.harness_watch();
    harness.mark_unchanged();
    let mut retry = tokio::time::interval(HARNESS_RETRY);
    retry.tick().await;

    let ours_schema = node.schema();
    // A snapshot request goes out with one `Hello` that can answer it (see
    // `apply_failed`, `take_resnap`).
    let asked = node.take_resnap(remote);
    let ours = node.held_here(&asked).await.context(Phase::Hello)?;
    framed
        .send(
            Msg::Hello {
                cursors: ours.clone(),
                schema: Some(ours_schema),
            }
            .encode()?,
        )
        .await
        .context(Phase::Hello)?;
    let first = match first {
        Some(m) => m,
        None => Msg::decode(
            &framed
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed before hello"))
                .context(Phase::Hello)?
                .context(Phase::Hello)?,
        )
        .context(Phase::Hello)?,
    };
    let Msg::Hello {
        cursors: theirs,
        schema,
    } = first
    else {
        return Err(
            anyhow::anyhow!("expected hello, got frame {}", first.tag()).context(Phase::Hello)
        );
    };
    node.learn(remote, &theirs);
    let acked = theirs.iter().find(|(o, _)| *o == node.me).map(|(_, s)| *s);
    if let Some(seq) = acked {
        node.store
            .set_cursor(&node.me.0, &remote.0, i64::try_from(seq)?)
            .await
            .context(Phase::Hello)?;
    }
    // 183 R1 (E-M3): a peer on an older agent schema (or a pre-183 one,
    // which reports none) cannot apply the 53+ `sessions`/`messages`
    // columns: `sqlite3changeset_apply` skips such a table yet its cursor
    // would advance. So no log row or snapshot goes to it on this stream,
    // and no cursor of its moves; its next `Hello` (after it upgrades)
    // restarts shipping from the cursors it still holds. Harness frames
    // carry no rows and still flow. The held peer sees our newer schema in
    // our `Hello` and says so on its side.
    let hold = schema_behind(schema, ours_schema);
    let held_by_peer = schema.is_some_and(|v| v > ours_schema);
    if hold {
        tracing::warn!(
            "sessions: holding log for {remote}: its agent schema {} < ours {ours_schema}; \
             nothing of ours ships to it until it is upgraded",
            schema_label(schema),
        );
    }
    if held_by_peer {
        tracing::warn!(
            "sessions: {remote} holds its log from us: its agent schema {} > ours {ours_schema}; \
             nothing of its sessions arrives here until this engine is upgraded",
            schema_label(schema),
        );
    }
    node.track(remote, |s| {
        s.last_hello = Some(now_text());
        s.schema = schema;
        s.held = hold;
        s.held_by_peer = held_by_peer;
        s.acked = Some(acked.unwrap_or(0));
        s.applied = ours.iter().find(|(o, _)| o == remote).map(|(_, s)| *s);
        s.peer_head = theirs.iter().find(|(o, _)| o == remote).map(|(_, s)| *s);
    });
    let _up = Up::new(node, remote);

    let (framed, written) = counted(framed);
    let (sink, inbound) = framed.split();
    let (replies, queued) = mpsc::unbounded_channel();
    let mut out = Out {
        node: node.clone(),
        remote: remote.clone(),
        sink,
        replies: queued,
        written,
    };
    let reader = read(node, remote, inbound, replies);
    let writer = async {
        // Origins whose shipping failed on this stream: retried on `retry`,
        // their live relay skipped meanwhile.
        let mut failed: HashSet<PeerId> = HashSet::new();
        // Catch-up: every origin we hold above what they hold (none while holding).
        for (origin, seq) in ours.iter().filter(|_| !hold) {
            let after = theirs.iter().find(|(o, _)| o == origin).map(|(_, s)| *s);
            if after.is_some_and(|a| a >= *seq) {
                continue;
            }
            if !survive(
                node,
                remote,
                Phase::CatchUp,
                catch_up(node, remote, &mut out, origin, after).await,
            )? {
                failed.insert(origin.clone());
            }
        }
        // 139 R2 D3: whatever waited for this peer while the link was down.
        survive(
            node,
            remote,
            Phase::Harness,
            flush_harness(node, remote, &mut out).await,
        )?;
        loop {
            tokio::select! {
                Some(reply) = out.replies.recv() => out.put(&reply).await.map_err(|e| phased(e, Phase::Receive))?,
                changed = log.changed() => {
                    changed?;
                    if !hold && !survive(node, remote, Phase::Ship, ship_own(node, remote, &mut out).await)? {
                        failed.insert(node.me.clone());
                    }
                }
                () = node.kick.notified() => {
                    if !hold && !survive(node, remote, Phase::Ship, ship_own(node, remote, &mut out).await)? {
                        failed.insert(node.me.clone());
                    }
                }
                changed = harness.changed() => {
                    changed?;
                    survive(node, remote, Phase::Harness, flush_harness(node, remote, &mut out).await)?;
                }
                _ = retry.tick() => {
                    survive(node, remote, Phase::Harness, flush_harness(node, remote, &mut out).await)?;
                    for origin in failed.clone().iter().filter(|_| !hold) {
                        let after = (*origin == node.me).then(|| node.holds(remote, origin));
                        if survive(node, remote, Phase::CatchUp, catch_up(node, remote, &mut out, origin, after).await)? {
                            failed.remove(origin);
                        }
                    }
                }
                relayed = relay.recv() => match relayed {
                    Ok((origin, row)) => {
                        if hold || origin == *remote || failed.contains(&origin) {
                            continue;
                        }
                        let (phase, r) = match node.holds(remote, &origin) {
                            // They have nothing of this origin yet: bootstrap them.
                            0 => (Phase::Snapshot, send_snapshot(node, remote, &origin, &mut out).await),
                            held if held < row.seq => {
                                let seq = row.seq;
                                let sent = out.send(Msg::Log { origin: origin.clone(), rows: vec![row] }).await;
                                if sent.is_ok() {
                                    node.mark(remote, &origin, seq);
                                }
                                (Phase::Ship, sent)
                            }
                            _ => (Phase::Ship, Ok(())),
                        };
                        if !survive(node, remote, phase, r)? {
                            failed.insert(origin);
                        }
                    }
                    // Relayed rows were missed: every relayed origin they hold
                    // less of than we do gets a snapshot (none on a pair).
                    Err(broadcast::error::RecvError::Lagged(_)) if !hold => {
                        let held = node.held_here(&HashSet::new()).await.context(Phase::Ship)?;
                        for (origin, seq) in held.iter().filter(|(o, _)| *o != node.me && o != remote) {
                            if node.holds(remote, origin) < *seq
                                && !survive(node, remote, Phase::Snapshot, send_snapshot(node, remote, origin, &mut out).await)?
                            {
                                failed.insert(origin.clone());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
            }
        }
    };
    tokio::select! {
        r = reader => r,
        r = writer => r,
    }
}

/// The reading half: apply what arrives, queue the acks for the writer.
async fn read(
    node: &Arc<Node>,
    remote: &PeerId,
    mut inbound: SplitStream<Stream>,
    replies: mpsc::UnboundedSender<Msg>,
) -> anyhow::Result<()> {
    let mut inflight: HashMap<PeerId, InFlight> = HashMap::new();
    while let Some(bytes) = inbound.next().await {
        let msg = Msg::decode(&bytes.context(Phase::Receive)?).context(Phase::Receive)?;
        match msg {
            Msg::Hello { .. } => {}
            Msg::Log { origin, rows } => {
                let Some(last) = rows.last().map(|r| r.seq) else {
                    continue;
                };
                if origin == node.me {
                    continue;
                }
                node.mark(remote, &origin, last);
                if origin == *remote {
                    node.track(remote, |s| {
                        s.peer_head = Some(s.peer_head.unwrap_or(0).max(last))
                    });
                }
                let moved = receive_log(node, &origin, &rows, origin == *remote)
                    .await
                    .map_err(|e| phased(e, Phase::Apply))?;
                if let Some(seq) = moved {
                    if origin == *remote {
                        node.track(remote, |s| s.applied = Some(seq));
                    }
                    let _ = replies.send(Msg::Ack { origin, seq });
                }
            }
            part @ Msg::Snapshot { .. } => {
                receive_snapshot(node, remote, part, &mut inflight, &replies)
                    .await
                    .map_err(|e| phased(e, Phase::Apply))?
            }
            Msg::Ack { origin, seq } => {
                node.mark(remote, &origin, seq);
                if origin == node.me {
                    node.track(remote, |s| s.acked = Some(s.acked.unwrap_or(0).max(seq)));
                    node.store
                        .set_cursor(&node.me.0, &remote.0, i64::try_from(seq)?)
                        .await
                        .context(Phase::Receive)?;
                }
            }
            Msg::Harness {
                session,
                kind,
                id,
                text,
                wrapped,
                ..
            } => {
                let error = match session.parse::<SessionId>() {
                    Ok(session) => {
                        let m = agent::supervisor::HarnessDeliver {
                            session,
                            kind,
                            text,
                            wrapped,
                            wake: true,
                        };
                        receive_harness(node, remote, &id, m).await.err()
                    }
                    Err(_) => Some("malformed session id".to_owned()),
                };
                let _ = replies.send(Msg::HarnessAck { id, error });
            }
            Msg::HarnessAck { id, error } => match error {
                None => node
                    .store
                    .harness_delete(&id)
                    .await
                    .context(Phase::Harness)?,
                Some(e) => {
                    tracing::info!("sessions: harness {id} refused by {remote}: {e}");
                    node.store
                        .harness_refused(&id, &e)
                        .await
                        .context(Phase::Harness)?;
                }
            },
            other => tracing::debug!(
                "sessions: frame {} from {remote} has no handler on this stream",
                other.tag()
            ),
        }
    }
    Ok(())
}

/// 139 R2 D2/D3: send every `harness_outbox` row whose session `remote`
/// owns now (the owner is re-resolved per attempt), deliver here the rows
/// of sessions that came back, drop rows past the TTL with a local note.
/// Called after the `Hello` exchange, on every enqueue and every
/// [`HARNESS_RETRY`]. The ack deletes the row ([`Msg::HarnessAck`]).
async fn flush_harness(node: &Arc<Node>, remote: &PeerId, out: &mut Out) -> anyhow::Result<()> {
    let rows = node.store.harness_outbox().await?;
    if rows.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp_millis();
    let ttl = i64::try_from(node.harness_ttl.as_millis()).unwrap_or(i64::MAX);
    for row in rows {
        let session: SessionId = match row.session.parse() {
            Ok(s) => s,
            Err(_) => {
                node.store.harness_delete(&row.id).await?;
                continue;
            }
        };
        if now.saturating_sub(row.at) > ttl {
            tracing::warn!(%session, id = %row.id, attempts = row.attempts, "139: {} for a session owned elsewhere dropped after {:?}: {}", row.kind, node.harness_ttl, row.last_error.as_deref().unwrap_or("owner never reachable"));
            // ponytail: the owner's next message replaces the note (owner wins), as 68's orphan note.
            let _ = node
                .store
                .append_local_note(
                    session,
                    &format!(
                        "{} produced on {} was not forwarded to the session's owner within {} s and was dropped",
                        row.kind,
                        node.me.0,
                        node.harness_ttl.as_secs()
                    ),
                )
                .await;
            node.store.harness_delete(&row.id).await?;
            continue;
        }
        match node.store.owner(session).await? {
            Some((owner, _)) if owner == remote.0 => {
                node.store.harness_sent(&row.id).await?;
                out.send(Msg::Harness {
                    session: row.session,
                    kind: row.kind,
                    origin: PeerId(row.origin),
                    id: row.id,
                    text: row.text,
                    wrapped: row.wrapped,
                    scope: row.scope,
                    at: row.at,
                })
                .await?;
            }
            // The session came back here: the supervisor delivers it.
            Some((owner, _)) if owner.is_empty() || owner == node.me.0 => {
                if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    && agents
                        .tell(agent::supervisor::HarnessDeliver {
                            session,
                            kind: row.kind,
                            text: row.text,
                            wrapped: row.wrapped,
                            wake: true,
                        })
                        .await
                        .is_ok()
                {
                    node.store.harness_delete(&row.id).await?;
                }
            }
            _ => {}
        }
    }
    node.store
        .harness_prune_seen(now.saturating_sub(ttl))
        .await?;
    Ok(())
}

/// 139 R2 D2: one received `Msg::Harness`. `Err` is the ack's `error`: the
/// sender keeps the row. A known session is handed to the supervisor as
/// [`agent::supervisor::HarnessDeliver`] once per `id` (`harness_seen`);
/// `inject` there forwards again if the session has moved on.
async fn receive_harness(
    node: &Arc<Node>,
    remote: &PeerId,
    id: &str,
    m: agent::supervisor::HarnessDeliver,
) -> Result<(), String> {
    let session = m.session;
    match node.store.session(session).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err("unknown session".to_owned()),
        Err(e) => return Err(e.to_string()),
    }
    let now = chrono::Utc::now().timestamp_millis();
    if !node
        .store
        .harness_mark_seen(id, now)
        .await
        .map_err(|e| e.to_string())?
    {
        tracing::debug!(%session, id, "sessions: harness envelope seen before; acked again");
        return Ok(());
    }
    tracing::info!(%session, id, "sessions: {} from {remote} handed to the supervisor", m.kind);
    let handed = match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => agents.tell(m).await.map_err(|e| e.to_string()),
        _ => Err("no supervisor here".to_owned()),
    };
    if handed.is_err() {
        let _ = node.store.harness_unsee(id).await;
    }
    handed
}

/// What `remote` needs of `origin` after `after`: our log rows when it is
/// our own origin and the log reaches back, else a snapshot.
async fn catch_up(
    node: &Arc<Node>,
    remote: &PeerId,
    out: &mut Out,
    origin: &PeerId,
    after: Option<u64>,
) -> anyhow::Result<()> {
    #[cfg(test)]
    if node.fail_ship.lock().contains(origin) {
        anyhow::bail!("test: shipping {origin} fails");
    }
    match after {
        Some(a) if *origin == node.me => {
            if !ship_log(node, remote, out, a).await? {
                send_snapshot(node, remote, origin, out).await?;
            }
            Ok(())
        }
        // No cursor, or a relayed origin (we hold its rows, not its log).
        _ => send_snapshot(node, remote, origin, out).await,
    }
}

/// Our own rows after what `remote` holds; a snapshot if the log no longer
/// reaches back that far.
async fn ship_own(node: &Arc<Node>, remote: &PeerId, out: &mut Out) -> anyhow::Result<()> {
    let after = node.holds(remote, &node.me);
    catch_up(node, remote, out, &node.me, Some(after)).await
}

/// Send our log rows with `seq > after` in frame-sized batches, reading
/// the log a page at a time. `false` when the log does not reach back to
/// `after` (pruned past it, or neither row `after + 1` nor row `after` is
/// there) or a row is over [`ROW_MAX`]: the caller sends a snapshot
/// instead. A batch never ends right before a hole in the log (a burned or
/// orphaned seq), and a frame that must start past one starts with the row
/// before it ([`anchor`]), so the receiver sees every hole inside one frame
/// and steps over it.
async fn ship_log(
    node: &Arc<Node>,
    remote: &PeerId,
    out: &mut Out,
    after: u64,
) -> anyhow::Result<bool> {
    let head = u64::try_from(node.store.log_head()).unwrap_or(0);
    let page = i64::try_from(BATCH_ROWS)?;
    let mut from = after;
    let mut batch: Vec<LogRow> = Vec::new();
    let mut size = 0;
    loop {
        let rows = node.store.log_page(i64::try_from(from)?, page).await?;
        if from == after {
            match rows.first() {
                None => return Ok(head <= after),
                Some(r) if u64::try_from(r.seq).unwrap_or(0) == after + 1 => {}
                // A hole right after `after`. Row `after` still in the log
                // (prune keeps the lowest cursor's row) means nothing past it
                // was pruned: the hole is a burned or orphaned seq. The frame
                // starts with row `after` as an anchor, which the peer drops
                // as a duplicate and then steps over the hole. An anchor that
                // cannot share a frame with the row after the hole would go
                // alone and leave the hole at a frame start: a snapshot instead.
                Some(first) => match anchor(node, after).await? {
                    Some(a) if a.changeset.len() + first.changeset.len() <= ROW_MAX => {
                        size = a.changeset.len();
                        batch.push(a);
                    }
                    _ => return Ok(false),
                },
            }
        }
        let full = rows.len() == BATCH_ROWS;
        for row in rows {
            let row = LogRow::from(row);
            let len = row.changeset.len();
            if len > ROW_MAX {
                tracing::warn!(
                    "sessions: log row {} of session {} is {len} bytes, over one {MAX_FRAME}-byte frame: \
                     {remote} gets a snapshot of {} instead of the log after seq {after}",
                    row.seq,
                    row.session_id,
                    node.me
                );
                return Ok(false);
            }
            let follows = batch.last().is_some_and(|l| l.seq + 1 == row.seq);
            let filled = batch.len() >= BATCH_ROWS || size + len > BATCH_BYTES;
            if !batch.is_empty() && ((follows && filled) || size + len > ROW_MAX) {
                // Forced to cut at a hole: the next frame starts with the
                // last row again as its anchor, when the two fit one frame.
                let again = batch
                    .last()
                    .filter(|l| !follows && l.changeset.len() + len <= ROW_MAX)
                    .cloned();
                send_batch(node, remote, out, std::mem::take(&mut batch)).await?;
                size = 0;
                if let Some(a) = again {
                    size = a.changeset.len();
                    batch.push(a);
                }
            }
            size += len;
            from = row.seq;
            batch.push(row);
        }
        if !full {
            break;
        }
    }
    if !batch.is_empty() {
        send_batch(node, remote, out, batch).await?;
    }
    Ok(true)
}

/// Row `seq` of our log, when it is still there (not pruned, not orphaned).
async fn anchor(node: &Arc<Node>, seq: u64) -> anyhow::Result<Option<LogRow>> {
    if seq == 0 {
        return Ok(None);
    }
    let rows = node.store.log_page(i64::try_from(seq - 1)?, 1).await?;
    Ok(rows
        .into_iter()
        .next()
        .map(LogRow::from)
        .filter(|r| r.seq == seq))
}

async fn send_batch(
    node: &Arc<Node>,
    remote: &PeerId,
    out: &mut Out,
    rows: Vec<LogRow>,
) -> anyhow::Result<()> {
    // Debug builds only: `THEUTH_DEBUG_SHIP_DELAY_SECS=N` holds each batch of
    // our rows N seconds before it goes, to reproduce a slow mesh (183 §5.6).
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("THEUTH_DEBUG_SHIP_DELAY_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }
    let last = rows.last().map_or(0, |r| r.seq);
    out.send(Msg::Log {
        origin: node.me.clone(),
        rows,
    })
    .await?;
    node.mark(remote, &node.me, last);
    Ok(())
}

/// Build and send the snapshot of `origin` in parts.
async fn send_snapshot(
    node: &Arc<Node>,
    remote: &PeerId,
    origin: &PeerId,
    out: &mut Out,
) -> anyhow::Result<()> {
    let (snap, left_out) = snapshot::build_counted(&node.store, origin)
        .await
        .with_context(|| format!("building the snapshot of {origin}"))?;
    let Msg::Snapshot { current_to_seq, .. } = &snap else {
        anyhow::bail!("snapshot::build returned {}", snap.tag());
    };
    let seq = *current_to_seq;
    for part in snapshot::split(snap) {
        out.send(part).await?;
    }
    node.mark(remote, origin, seq);
    tracing::info!("sessions: snapshot of {origin} (to seq {seq}) sent to {remote}");
    if left_out > 0 {
        // ponytail: the wire has no field for it, so only this side's status shows it.
        node.report(
            remote,
            Phase::Snapshot,
            &anyhow::anyhow!(
                "{left_out} rows of {origin}, each over one frame, were left out of the snapshot"
            ),
            "the peer lacks them until they change",
        );
    }
    Ok(())
}

/// Apply a batch of `origin`'s rows **in seq order**: a row right after our
/// cursor applies (and drains what waited behind it), a row at or below it
/// is a duplicate. On the `direct` stream (the origin's own rows) a frame
/// is consecutive rows of its log, so a seq missing between two rows of one
/// frame is a hole there (burned or orphaned) and is stepped over. Any other
/// row further ahead waits in the origin's reorder buffer — a relay beside
/// the direct link, or a reconnect overlap, may deliver ahead; a gap open
/// for [`GAP_WAIT`], or more than [`BATCH_ROWS`] rows waiting, ends the
/// stream (the `Hello` exchange re-sends from the cursor). A row that fails
/// to apply ends it too ([`Node::apply_failed`]). Returns the cursor when it
/// moved (what to ack).
async fn receive_log(
    node: &Arc<Node>,
    origin: &PeerId,
    rows: &[LogRow],
    direct: bool,
) -> anyhow::Result<Option<u64>> {
    let lock = node.origin_lock(origin);
    let _guard = lock.lock().await;
    let start = node.cursor(origin).await?.unwrap_or(0);
    let mut cursor = start;
    // Seq → the seq before it in this frame (direct stream only).
    let follows: HashMap<u64, u64> = if direct {
        rows.windows(2).map(|w| (w[1].seq, w[0].seq)).collect()
    } else {
        HashMap::new()
    };
    // Session → its `(owner, epoch)` before the first row applied to it.
    let mut touched: HashMap<SessionId, Option<(String, u32)>> = HashMap::new();
    let mut queue: VecDeque<LogRow> = rows.iter().cloned().collect();
    while let Some(row) = queue.pop_front() {
        if row.seq <= cursor {
            continue;
        }
        if row.seq > cursor + 1 && follows.get(&row.seq) != Some(&cursor) {
            let mut pending = node.pending.lock();
            let (since, map) = pending
                .entry(origin.clone())
                .or_insert_with(|| (Instant::now(), BTreeMap::new()));
            if since.elapsed() >= GAP_WAIT || map.len() >= BATCH_ROWS {
                let waiting = map.len();
                pending.remove(origin);
                return Err(anyhow::anyhow!(
                    "rows of {origin} after seq {cursor} missing ({waiting} rows waiting, row {} \
                     arrived); resyncing",
                    row.seq
                )
                .context(Phase::Gap));
            }
            map.insert(row.seq, row);
            continue;
        }
        let applied = async {
            let session: SessionId = row.session_id.parse()?;
            let before = match touched.get(&session) {
                Some(b) => b.clone(),
                None => node.store.owner(session).await?,
            };
            let applied = apply_log_row(&node.store, origin, &row).await?;
            anyhow::Ok((session, before, applied))
        }
        .await;
        let (session, before, applied) = match applied {
            Ok(v) => v,
            Err(e) => {
                let next = node.apply_failed(origin, row.seq);
                return Err(e
                    .context(format!(
                        "row {} of session {} from {origin} ({} bytes) did not apply; {next}",
                        row.seq,
                        row.session_id,
                        row.changeset.len()
                    ))
                    .context(Phase::Apply));
            }
        };
        match applied {
            Applied::Applied => {
                touched.entry(session).or_insert(before);
                let _ = node.relay.send((origin.clone(), row.clone()));
            }
            Applied::StaleEpoch => {
                // Seen, not applied: the cursor still moves past it.
                node.store
                    .set_cursor(&origin.0, "", i64::try_from(row.seq)?)
                    .await?;
                if node
                    .stale_logged
                    .lock()
                    .insert((row.session_id.clone(), row.epoch))
                {
                    tracing::warn!(
                        "sessions: dropped stale rows of {} epoch {} from {origin} (fence is higher)",
                        row.session_id,
                        row.epoch
                    );
                }
            }
            Applied::Duplicate => {}
        }
        cursor = row.seq;
        if let Some(next) = node.take_pending(origin, cursor) {
            queue.push_front(next);
        }
    }
    for (session, before) in touched {
        let now = node.store.owner(session).await?;
        if now != before
            && let Some((owner, epoch)) = now
        {
            node.owner_changed(session, owner, epoch).await;
        }
        node.publish(AgentEvent::ContextChanged {
            session,
            revision: node
                .store
                .context_revision(session)
                .await
                .unwrap_or_default(),
            reason: "replicated".into(),
        })
        .await;
    }
    Ok((cursor > start).then_some(cursor))
}

/// One part of a snapshot from `remote`. The first part takes the origin's
/// lock, which the empty terminator releases after moving the cursor. A
/// part that fails to apply ends the stream; the next stream retries from
/// the cursor (a snapshot is not asked again: see [`Node::apply_failed`]).
async fn receive_snapshot(
    node: &Arc<Node>,
    remote: &PeerId,
    part: Msg,
    inflight: &mut HashMap<PeerId, InFlight>,
    replies: &mpsc::UnboundedSender<Msg>,
) -> anyhow::Result<()> {
    let Msg::Snapshot {
        origin,
        current_to_seq,
        tables,
    } = &part
    else {
        return Ok(());
    };
    if *origin == node.me {
        return Ok(());
    }
    let last = tables.is_empty();
    if !inflight.contains_key(origin) {
        let guard = node.origin_lock(origin).lock_owned().await;
        inflight.insert(
            origin.clone(),
            InFlight {
                _guard: guard,
                fenced: HashSet::new(),
                sessions: Vec::new(),
            },
        );
    }
    let state = inflight.get_mut(origin).expect("inserted above");
    if node
        .cursor(origin)
        .await?
        .is_none_or(|c| c < *current_to_seq)
    {
        let applied = snapshot::apply(&node.store, &part, &mut state.fenced, last)
            .await
            .map_err(|e| {
                e.context(format!(
                    "snapshot of {origin} (to seq {current_to_seq}) from {remote} did not apply; \
                     the next stream retries from the cursor"
                ))
                .context(Phase::Apply)
            })?;
        state.sessions.extend(applied);
    }
    if !last {
        return Ok(());
    }
    let state = inflight.remove(origin).expect("present");
    node.mark(remote, origin, *current_to_seq);
    if origin == remote {
        node.track(remote, |s| {
            s.applied = Some(s.applied.unwrap_or(0).max(*current_to_seq));
            s.peer_head = Some(s.peer_head.unwrap_or(0).max(*current_to_seq));
        });
    }
    let _ = replies.send(Msg::Ack {
        origin: origin.clone(),
        seq: *current_to_seq,
    });
    tracing::info!(
        "sessions: snapshot of {origin} (to seq {current_to_seq}) applied from {remote}"
    );
    for (id, owner, epoch, before) in state.sessions {
        if let Ok(session) = id.parse() {
            // 68 §4.3 / 139 T3: the snapshot raised the epoch of a session
            // this engine owned — demote exactly as `apply_log_row` does.
            crate::owner::on_applied(&node.store, origin, session, before, epoch).await;
            node.owner_changed(session, owner, epoch).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Two (then three) stores in temp dirs over a `tokio::io::duplex`
    //! transport (the loopback of `replica/tests/sync_loopback.rs`).

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use agent::store::Store;
    use genai::chat::{ChatMessage, MessageContent};
    use parking_lot::Mutex;
    use replica::{ALPN_SESSIONS, ByteStream, PeerEvent, PeerId, Transport};
    use runtime::SessionId;
    use tokio::sync::{broadcast, mpsc};

    use crate::SessionsConfig;
    use crate::apply::{Applied, apply_log_row};
    use crate::ship::Shipper;
    use crate::types::LogRow;

    type Inbound = (PeerId, Box<dyn ByteStream>);

    #[derive(Default)]
    struct Switch {
        inboxes: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>,
        links: Mutex<Vec<(PeerId, PeerId)>>,
    }

    impl Switch {
        fn linked(&self, a: &PeerId, b: &PeerId) -> bool {
            self.links
                .lock()
                .iter()
                .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
        }
    }

    struct Loop {
        me: PeerId,
        switch: Arc<Switch>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            let inboxes = self.switch.inboxes.lock();
            inboxes
                .keys()
                .filter(|p| **p != self.me && self.switch.linked(&self.me, p))
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            anyhow::ensure!(self.switch.linked(&self.me, peer), "{peer} is unreachable");
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let inboxes = self.switch.inboxes.lock();
            let to = inboxes
                .get(peer)
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("switch closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    struct Node {
        store: Store,
        shipper: Arc<Shipper>,
        transport: Arc<Loop>,
        _dir: tempfile::TempDir,
    }

    impl Node {
        fn me(&self) -> PeerId {
            self.transport.me.clone()
        }
    }

    /// A store opened as engine `name`, its shipper wired to a loopback
    /// transport the way `SessionSync::start_transport` wires the real one.
    async fn node(name: &str, switch: &Arc<Switch>) -> Node {
        node_in(
            name,
            tempfile::tempdir().unwrap(),
            switch,
            &SessionsConfig::default(),
        )
        .await
    }

    /// A node over an existing data dir (its `sessions.sqlite` may hold rows already).
    async fn node_in(
        name: &str,
        dir: tempfile::TempDir,
        switch: &Arc<Switch>,
        config: &SessionsConfig,
    ) -> Node {
        let me = PeerId(name.to_owned());
        let store = Store::open(&dir.path().join("sessions.sqlite"), Some(me.clone().into()))
            .await
            .unwrap();
        let shipper = Shipper::new(store.clone(), None, config);
        let (tx, rx) = mpsc::unbounded_channel();
        switch.inboxes.lock().insert(me.clone(), tx);
        let transport = Arc::new(Loop {
            me,
            switch: switch.clone(),
            inbox: tokio::sync::Mutex::new(rx),
            events: broadcast::channel(64).0,
        });
        let (accept, s) = (transport.clone(), shipper.clone());
        tokio::spawn(async move {
            while let Ok((peer, stream)) = accept.accept(ALPN_SESSIONS).await {
                let s = s.clone();
                tokio::spawn(async move {
                    if let Err(e) = s.serve(peer.clone(), stream).await {
                        eprintln!("stream from {peer} ended: {e:#}");
                    }
                });
            }
        });
        let (t, s) = (transport.clone(), shipper.clone());
        let mut events = t.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(PeerEvent::Connected(p)) => s.dial(t.clone(), p),
                    Ok(PeerEvent::Disconnected(p)) => s.hangup(&p),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Node {
            store,
            shipper,
            transport,
            _dir: dir,
        }
    }

    fn link(switch: &Switch, a: &Node, b: &Node, up: bool) {
        let (pa, pb) = (a.me(), b.me());
        let mut links = switch.links.lock();
        links.retain(|(x, y)| !((*x == pa && *y == pb) || (*x == pb && *y == pa)));
        if up {
            links.push((pa.clone(), pb.clone()));
        }
        drop(links);
        let ev = |p: PeerId| {
            if up {
                PeerEvent::Connected(p)
            } else {
                PeerEvent::Disconnected(p)
            }
        };
        let _ = a.transport.events.send(ev(pb));
        let _ = b.transport.events.send(ev(pa));
    }

    /// `(seq, role, text)` of every message row.
    async fn texts(store: &Store, id: SessionId) -> Vec<(i64, String, String)> {
        store
            .messages(id)
            .await
            .unwrap()
            .into_iter()
            .map(|(seq, m)| {
                (
                    seq,
                    format!("{:?}", m.role),
                    m.content.joined_texts().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// `(id, owner, epoch, title)` of every session.
    async fn sessions(store: &Store) -> Vec<(String, String, u32, String)> {
        let mut v: Vec<_> = store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.id.to_string(), s.owner, s.epoch, s.title))
            .collect();
        v.sort();
        v
    }

    /// Poll `f` until it holds or 90 s pass (converges in about a second
    /// here; the margin is for a loaded CI box).
    async fn until<F: Future<Output = bool>>(what: &str, mut f: impl FnMut() -> F) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while !f().await {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn same_session(a: &Store, b: &Store, id: SessionId) -> bool {
        texts(a, id).await == texts(b, id).await && sessions(a).await == sessions(b).await
    }

    fn user(t: &str) -> ChatMessage {
        ChatMessage::user(t)
    }

    fn asst(t: &str) -> ChatMessage {
        ChatMessage::assistant(MessageContent::from_text(t))
    }

    #[tokio::test]
    async fn append_on_a_lands_on_b_with_the_same_content() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);

        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("hi")).await.unwrap();
        a.store.append(id, &asst("hello")).await.unwrap();
        until("B mirrors A", || same_session(&a.store, &b.store, id)).await;
        assert_eq!(texts(&b.store, id).await.len(), 2);
        let s = b.store.session(id).await.unwrap().unwrap();
        assert_eq!((s.owner.as_str(), s.epoch, s.here), ("A", 1, false));
        // B acked: A knows what B holds, and B's cursor for A is A's head.
        let head = a.store.log_seq();
        until("A records B's ack", || async {
            a.store
                .cursors()
                .await
                .unwrap()
                .iter()
                .any(|c| c.origin == "A" && c.peer == "B" && c.seq == head)
        })
        .await;
        assert!(
            b.store
                .cursors()
                .await
                .unwrap()
                .iter()
                .any(|c| c.origin == "A" && c.peer.is_empty() && c.seq == head)
        );
    }

    /// A store whose sessions predate 68 (owner backfilled at open, no log
    /// row ever written) still bootstraps a peer: its origin is offered at
    /// head 0, the snapshot is current to seq 0, and live rows follow.
    #[tokio::test]
    async fn a_store_with_sessions_but_no_log_bootstraps_a_peer() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::new();
        {
            let store = Store::open(
                &dir.path().join("sessions.sqlite"),
                Some(PeerId("A".into()).into()),
            )
            .await
            .unwrap();
            store
                .create_session(id, "m", None, None, None)
                .await
                .unwrap();
            store.append(id, &user("before 68")).await.unwrap();
            // What a pre-68 store looks like after migration: rows, no log.
            let mut tx = store.begin_apply().await.unwrap();
            sqlx::query("DELETE FROM session_log")
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            store.close().await;
        }
        let switch = Arc::new(Switch::default());
        let a = node_in("A", dir, &switch, &SessionsConfig::default()).await;
        assert_eq!(a.store.log_seq(), 0);
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);

        until("B bootstraps from A's empty-log snapshot", || {
            same_session(&a.store, &b.store, id)
        })
        .await;
        assert_eq!(texts(&b.store, id).await.len(), 1);
        assert!(
            b.store
                .cursors()
                .await
                .unwrap()
                .iter()
                .any(|c| c.origin == "A" && c.peer.is_empty() && c.seq == 0)
        );
        // Live rows follow from seq 1.
        a.store.append(id, &asst("after")).await.unwrap();
        until("live row after the snapshot", || {
            same_session(&a.store, &b.store, id)
        })
        .await;
        assert_eq!(texts(&b.store, id).await.len(), 2);
    }

    #[tokio::test]
    async fn cut_on_a_deletes_b_tail() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);

        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("one")).await.unwrap();
        a.store.append(id, &asst("1")).await.unwrap();
        let turn = a.store.append(id, &user("two")).await.unwrap();
        a.store.append(id, &asst("2")).await.unwrap();
        until("B has four rows", || async {
            texts(&b.store, id).await.len() == 4
        })
        .await;

        a.store.cut_to(id, turn).await.unwrap();
        a.store.commit_cut(id).await.unwrap();
        assert_eq!(texts(&a.store, id).await.len(), 2);
        until("B's tail is gone", || same_session(&a.store, &b.store, id)).await;
        assert_eq!(texts(&b.store, id).await.len(), 2);
    }

    #[tokio::test]
    async fn stale_epoch_rows_are_dropped() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("late")).await.unwrap();
        let rows: Vec<LogRow> = a
            .store
            .log_after(0)
            .await
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(rows.len(), 2);

        // B already saw epoch 2 of this session (a take elsewhere).
        assert!(b.store.set_fence(id, 2, "C").await.unwrap());
        for row in &rows {
            assert_eq!(
                apply_log_row(&b.store, &a.me(), row).await.unwrap(),
                Applied::StaleEpoch
            );
        }
        assert!(b.store.session(id).await.unwrap().is_none());
        // The same rows on a fence-free store apply, then repeat as duplicates.
        let c = node("C", &switch).await;
        for row in &rows {
            assert_eq!(
                apply_log_row(&c.store, &a.me(), row).await.unwrap(),
                Applied::Applied
            );
        }
        assert_eq!(
            apply_log_row(&c.store, &a.me(), &rows[1]).await.unwrap(),
            Applied::Duplicate
        );
        assert_eq!(texts(&c.store, id).await, texts(&a.store, id).await);
        // Applying set the fence from the session row.
        assert_eq!(c.store.fence(id).await.unwrap(), Some((1, "A".to_owned())));
    }

    #[tokio::test]
    async fn snapshot_bootstrap_equals_a_full_replay() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let c = node("C", &switch).await;
        link(&switch, &a, &c, true);

        // 1 000 messages over three sessions, replayed live to C.
        let ids: Vec<SessionId> = (0..3).map(|_| SessionId::new()).collect();
        for (i, id) in ids.iter().enumerate() {
            a.store
                .create_session(*id, "m", None, None, None)
                .await
                .unwrap();
            a.store
                .set_title(*id, &format!("session {i}"))
                .await
                .unwrap();
        }
        for n in 0..1000 {
            let id = ids[n % 3];
            let m = if n % 2 == 0 {
                user(&format!("u{n}"))
            } else {
                asst(&format!("a{n}"))
            };
            a.store.append(id, &m).await.unwrap();
        }
        until("C replayed everything", || async {
            for id in &ids {
                if texts(&a.store, *id).await != texts(&c.store, *id).await {
                    return false;
                }
            }
            sessions(&a.store).await == sessions(&c.store).await
        })
        .await;

        // Prune A's log (C acked it all): a newcomer can only be served a snapshot.
        until("C's ack reached A", || async {
            let head = a.store.log_seq();
            a.store
                .cursors()
                .await
                .unwrap()
                .iter()
                .any(|c| c.origin == "A" && c.peer == "C" && c.seq == head)
        })
        .await;
        crate::prune::tick(&a.store, &a.me(), 7).await.unwrap();
        assert!(a.store.log_after(0).await.unwrap().len() <= 1);

        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        until("B bootstrapped from a snapshot", || async {
            for id in &ids {
                if texts(&a.store, *id).await != texts(&b.store, *id).await {
                    return false;
                }
            }
            sessions(&a.store).await == sessions(&b.store).await
        })
        .await;
        assert_eq!(sessions(&b.store).await, sessions(&c.store).await);
        let head = a.store.log_seq();
        assert!(
            b.store
                .cursors()
                .await
                .unwrap()
                .iter()
                .any(|c| c.origin == "A" && c.peer.is_empty() && c.seq == head)
        );

        // Live rows keep flowing to the snapshot-bootstrapped peer.
        a.store
            .append(ids[0], &user("after snapshot"))
            .await
            .unwrap();
        until("B follows live", || {
            same_session(&a.store, &b.store, ids[0])
        })
        .await;
    }

    #[tokio::test]
    async fn prune_keeps_at_most_one_snapshot_of_history() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        for n in 0..50 {
            a.store.append(id, &user(&format!("{n}"))).await.unwrap();
        }
        let head = a.store.log_seq();
        assert_eq!(a.store.log_after(0).await.unwrap().len(), 51);

        // No known peer: everything but the head row goes (a peer bootstraps
        // from a snapshot); the head seeds `seq` at the next open.
        crate::prune::tick(&a.store, &a.me(), 7).await.unwrap();
        let left = a.store.log_after(0).await.unwrap();
        assert_eq!(left.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![head]);
        assert_eq!(a.store.log_seq(), head);

        // A known peer behind the head: prune stops at its cursor.
        for n in 50..60 {
            a.store.append(id, &user(&format!("{n}"))).await.unwrap();
        }
        a.store.set_cursor("A", "B", head + 3).await.unwrap();
        crate::prune::tick(&a.store, &a.me(), 7).await.unwrap();
        let left: Vec<i64> = a
            .store
            .log_after(0)
            .await
            .unwrap()
            .iter()
            .map(|r| r.seq)
            .collect();
        // The row at the cursor stays (the anchor across a hole after it).
        assert_eq!(left, ((head + 3)..=(head + 10)).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn relays_a_to_c_through_b() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);

        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("hi")).await.unwrap();
        until("B mirrors A", || same_session(&a.store, &b.store, id)).await;

        // C only ever talks to B: bootstrap of A's rows comes from B's copy...
        let c = node("C", &switch).await;
        link(&switch, &b, &c, true);
        until("C bootstrapped via B", || {
            same_session(&a.store, &c.store, id)
        })
        .await;
        // ...and live rows are relayed.
        a.store.append(id, &asst("relayed")).await.unwrap();
        until("C follows A live via B", || {
            same_session(&a.store, &c.store, id)
        })
        .await;
        assert_eq!(texts(&c.store, id).await.len(), 2);
        let s = c.store.session(id).await.unwrap().unwrap();
        assert_eq!((s.owner.as_str(), s.here), ("A", false));
        assert!(!c.shipper.node().me.0.is_empty());
    }

    // ---- 139 R2: the harness outbox over the stream (no supervisor on either side) ----

    fn harness_row(session: SessionId, id: &str) -> agent::store::harness::HarnessRow {
        agent::store::harness::HarnessRow {
            id: id.into(),
            session: session.to_string(),
            kind: "job j1".into(),
            origin: "A".into(),
            text: "done".into(),
            wrapped: "<job_result id=\"j1\">done</job_result>".into(),
            scope: None,
            at: chrono::Utc::now().timestamp_millis(),
            attempts: 0,
            last_error: None,
        }
    }

    /// A session created on A, replicated to every node in `others`, then
    /// taken by each of `takers` in turn (epoch 2, 3, …) with every node
    /// seeing the final owner.
    async fn taken(a: &Node, others: &[&Node], takers: &[&Node]) -> SessionId {
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        for o in others {
            until("replicated", || same_session(&a.store, &o.store, id)).await;
        }
        for (i, t) in takers.iter().enumerate() {
            let epoch = u32::try_from(i).unwrap() + 2;
            t.store.set_owner(id, &t.me().0, epoch).await.unwrap();
            t.store.set_fence(id, epoch, &t.me().0).await.unwrap();
            for o in others.iter().chain(std::iter::once(&a)) {
                until("owner row landed", || async {
                    o.store.owner(id).await.unwrap() == Some((t.me().0.clone(), epoch))
                })
                .await;
            }
        }
        id
    }

    async fn outbox(n: &Node) -> Vec<(String, i64, Option<String>)> {
        n.store
            .harness_outbox()
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.attempts, r.last_error))
            .collect()
    }

    #[tokio::test]
    async fn harness_outbox_row_waits_for_the_link_and_is_sent_on_connect() {
        let switch = Arc::new(Switch::default());
        let (a, b) = (node("A", &switch).await, node("B", &switch).await);
        link(&switch, &a, &b, true);
        let id = taken(&a, &[&b], &[&b]).await;
        link(&switch, &a, &b, false);
        tokio::time::sleep(Duration::from_millis(200)).await;

        a.store
            .harness_enqueue(&harness_row(id, "u1"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            outbox(&a).await,
            vec![("u1".into(), 0, None)],
            "no link: nothing sent"
        );

        // The link returns: the row goes with the `Hello`; B has no supervisor
        // (a pairing run), so its ack refuses and A keeps the row with the reason.
        link(&switch, &a, &b, true);
        until("B answered", || async {
            outbox(&a).await == vec![("u1".into(), 1, Some("no supervisor here".into()))]
        })
        .await;
        assert!(
            b.store.harness_mark_seen("u1", 0).await.unwrap(),
            "a refused envelope is not remembered as seen"
        );
    }

    #[tokio::test]
    async fn harness_duplicate_id_is_acked_without_a_second_handoff() {
        let switch = Arc::new(Switch::default());
        let (a, b) = (node("A", &switch).await, node("B", &switch).await);
        link(&switch, &a, &b, true);
        let id = taken(&a, &[&b], &[&b]).await;
        // B handed this id to its supervisor before (a reconnect overlap resent it).
        assert!(b.store.harness_mark_seen("u2", 0).await.unwrap());
        a.store
            .harness_enqueue(&harness_row(id, "u2"))
            .await
            .unwrap();
        until("acked as seen: the row is gone", || async {
            outbox(&a).await.is_empty()
        })
        .await;
        assert!(
            !b.store.harness_mark_seen("u2", 0).await.unwrap(),
            "still seen, never unseen"
        );
    }

    #[tokio::test]
    async fn harness_row_follows_the_owner_when_the_session_moved_again() {
        let switch = Arc::new(Switch::default());
        let (a, b, c) = (
            node("A", &switch).await,
            node("B", &switch).await,
            node("C", &switch).await,
        );
        link(&switch, &a, &b, true);
        link(&switch, &a, &c, true);
        link(&switch, &b, &c, true);
        // B took it, then C did: the row's owner is re-resolved per attempt.
        let id = taken(&a, &[&b, &c], &[&b, &c]).await;
        link(&switch, &a, &c, false);
        link(&switch, &b, &c, false);
        tokio::time::sleep(Duration::from_millis(200)).await;

        a.store
            .harness_enqueue(&harness_row(id, "u3"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            outbox(&a).await,
            vec![("u3".into(), 0, None)],
            "B is linked but is not the owner"
        );

        assert!(c.store.harness_mark_seen("u3", 0).await.unwrap());
        link(&switch, &a, &c, true);
        until("sent to C and acked", || async {
            outbox(&a).await.is_empty()
        })
        .await;
        assert!(
            b.store.harness_mark_seen("u3", 0).await.unwrap(),
            "B never saw it"
        );
    }

    #[tokio::test]
    async fn harness_rows_past_the_ttl_are_dropped_with_a_local_note() {
        let switch = Arc::new(Switch::default());
        let cfg = SessionsConfig {
            harness_outbox_ttl_secs: 0,
            ..SessionsConfig::default()
        };
        let a = node_in("A", tempfile::tempdir().unwrap(), &switch, &cfg).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let id = taken(&a, &[&b], &[&b]).await;
        let before = texts(&a.store, id).await.len();
        let mut row = harness_row(id, "u4");
        row.at -= 1;
        a.store.harness_enqueue(&row).await.unwrap();
        until("dropped", || async { outbox(&a).await.is_empty() }).await;
        let t = texts(&a.store, id).await;
        assert_eq!(t.len(), before + 1);
        assert!(
            t.last()
                .unwrap()
                .2
                .contains("job j1 produced on A was not forwarded"),
            "{t:?}"
        );
        assert_eq!(
            texts(&b.store, id).await.len(),
            before,
            "the note is local to A"
        );
        assert!(
            b.store.harness_mark_seen("u4", 0).await.unwrap(),
            "never sent"
        );
    }

    // ---- 183 R1 (E-M3): mixed agent schemas ----

    /// A store opened as engine `A` with one session and two messages.
    async fn engine_a() -> (Store, tempfile::TempDir, SessionId) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("sessions.sqlite"),
            Some(PeerId("A".into()).into()),
        )
        .await
        .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.append(id, &user("hi")).await.unwrap();
        store.append(id, &asst("hello")).await.unwrap();
        (store, dir, id)
    }

    /// Connect a raw peer `Z` to a sync node over `store` and exchange
    /// `Hello`s: Z claims `schema` (`None` = a pre-183 peer) and holds
    /// `cursors`. Returns Z's end of the stream and A's `Hello` schema.
    async fn raw_peer(
        store: &Store,
        schema: Option<u32>,
        cursors: Vec<(PeerId, u64)>,
    ) -> (
        super::Stream,
        tokio::task::JoinHandle<anyhow::Result<()>>,
        Option<u32>,
    ) {
        let node = super::Node::new(store.clone(), None, Duration::from_secs(60));
        let (z, run, _, ours) = raw_on(node, schema, cursors).await;
        (z, run, ours)
    }

    /// [`raw_peer`] against an existing `node`; also returns the cursors of its `Hello`.
    async fn raw_on(
        node: Arc<super::Node>,
        schema: Option<u32>,
        cursors: Vec<(PeerId, u64)>,
    ) -> (
        super::Stream,
        tokio::task::JoinHandle<anyhow::Result<()>>,
        Vec<(PeerId, u64)>,
        Option<u32>,
    ) {
        use futures::{SinkExt as _, StreamExt as _};
        let (mine, theirs) = tokio::io::duplex(1 << 20);
        let run = tokio::spawn(async move {
            super::run_stream(&node, PeerId("Z".into()), Box::new(mine)).await
        });
        let mut z = crate::events::framed(Box::new(theirs));
        let hello = crate::types::Msg::decode(&z.next().await.unwrap().unwrap()).unwrap();
        let crate::types::Msg::Hello {
            cursors: held,
            schema: ours,
        } = hello
        else {
            panic!("first frame is not a Hello: {hello:?}");
        };
        z.send(
            crate::types::Msg::Hello { cursors, schema }
                .encode()
                .unwrap(),
        )
        .await
        .unwrap();
        (z, run, held, ours)
    }

    /// Every frame Z receives until `quiet` passes with none.
    async fn drain(z: &mut super::Stream, quiet: Duration) -> Vec<crate::types::Msg> {
        use futures::StreamExt as _;
        let mut out = Vec::new();
        while let Ok(Some(frame)) = tokio::time::timeout(quiet, z.next()).await {
            out.push(crate::types::Msg::decode(&frame.unwrap()).unwrap());
        }
        out
    }

    /// A peer whose `Hello` has no schema (pre-183) or an older one is shipped
    /// nothing, neither the log tail nor a snapshot, at connect or live, and
    /// the cursor A keeps for it stays where its `Hello` put it.
    #[tokio::test]
    async fn an_unknown_or_older_peer_is_held_without_moving_its_cursor() {
        let ours = agent::store::SESSIONS_WIRE_SCHEMA;
        for schema in [None, Some(ours - 1)] {
            let (store, _dir, id) = engine_a().await;
            let a = PeerId("A".into());
            let (mut z, run, hello) = raw_peer(&store, schema, vec![(a, 1)]).await;
            assert_eq!(hello, Some(ours), "A announces its own schema");
            assert!(
                drain(&mut z, Duration::from_millis(400)).await.is_empty(),
                "{schema:?}: the tail 2.. is held"
            );
            store.append(id, &user("later")).await.unwrap();
            assert!(
                drain(&mut z, Duration::from_millis(400)).await.is_empty(),
                "{schema:?}: a live row is held too"
            );
            let cursor = store
                .cursors()
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.origin == "A" && c.peer == "Z")
                .map(|c| c.seq);
            assert_eq!(cursor, Some(1), "{schema:?}: Z's cursor is not advanced");
            drop(z);
            let _ = run.await;
            store.close().await;
        }
    }

    /// After the held peer reconnects with the current schema (it upgraded),
    /// A ships from the cursor it held: every row after it arrives, once and
    /// in order, and live rows follow.
    #[tokio::test]
    async fn an_upgraded_peer_resumes_from_the_held_cursor() {
        let ours = agent::store::SESSIONS_WIRE_SCHEMA;
        let (store, _dir, id) = engine_a().await;
        let a = PeerId("A".into());
        let (mut z, run, _) = raw_peer(&store, None, vec![(a.clone(), 1)]).await;
        store.append(id, &asst("while held")).await.unwrap();
        assert!(drain(&mut z, Duration::from_millis(400)).await.is_empty());
        drop(z);
        let _ = run.await;

        // The upgraded Z: same cursors, now with a schema.
        let (mut z, _run, _) = raw_peer(&store, Some(ours), vec![(a, 1)]).await;
        let head = u64::try_from(store.log_seq()).unwrap();
        let mut seqs: Vec<u64> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while seqs.last() != Some(&head) {
            assert!(tokio::time::Instant::now() < deadline, "got {seqs:?}");
            for m in drain(&mut z, Duration::from_millis(200)).await {
                match m {
                    crate::types::Msg::Log { rows, .. } => {
                        seqs.extend(rows.iter().map(|r| r.seq));
                    }
                    other => panic!("expected the log tail, got {other:?}"),
                }
            }
        }
        assert_eq!(seqs, (2..=head).collect::<Vec<_>>());
        store.append(id, &user("live")).await.unwrap();
        let live: Vec<u64> = drain(&mut z, Duration::from_millis(500))
            .await
            .into_iter()
            .flat_map(|m| match m {
                crate::types::Msg::Log { rows, .. } => {
                    rows.into_iter().map(|r| r.seq).collect::<Vec<_>>()
                }
                other => panic!("expected a log row, got {other:?}"),
            })
            .collect();
        assert_eq!(live.first(), Some(&(head + 1)), "{live:?}");
    }

    // ---- 2026-11 stall: catch-ups both ways, oversized rows, failures, status ----

    /// `n` bytes of text, distinct per `tag`.
    fn big(n: usize, tag: usize) -> String {
        format!("{tag}:{}", "x".repeat(n))
    }

    /// `session_cursors(origin, peer)`.
    async fn acked(store: &Store, origin: &str, peer: &str) -> Option<i64> {
        store
            .cursors()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.origin == origin && c.peer == peer)
            .map(|c| c.seq)
    }

    /// `n`'s sync-status entry for `peer` (empty when it has none).
    fn sync_of(n: &Node, peer: &str) -> crate::types::PeerSync {
        n.shipper
            .node()
            .sync_status(&[])
            .into_iter()
            .find(|s| s.peer == peer)
            .unwrap_or_default()
    }

    /// A held (older) peer upgrades while each side holds a backlog for the
    /// other larger than the transport's window (the duplex buffers 1 MiB):
    /// both catch-ups run at once and both arrive. While each side wrote its
    /// catch-up before reading anything, this wedged the stream forever
    /// without one error.
    #[tokio::test]
    async fn a_held_peer_that_upgrades_gets_the_backlog_while_shipping_its_own() {
        let ours = agent::store::SESSIONS_WIRE_SCHEMA;
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        b.shipper.node().set_schema(ours - 1);
        let (sa, sb) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(sa, "m", None, None, None)
            .await
            .unwrap();
        b.store
            .create_session(sb, "m", None, None, None)
            .await
            .unwrap();
        b.store.append(sb, &user("from B")).await.unwrap();
        link(&switch, &a, &b, true);
        until("A applies B's rows while holding its own", || async {
            texts(&a.store, sb).await.len() == 1
        })
        .await;
        for n in 0..24 {
            a.store.append(sa, &user(&big(64 * 1024, n))).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            b.store.session(sa).await.unwrap().is_none(),
            "held: nothing of A's reached B"
        );
        assert!(sync_of(&a, "B").held && sync_of(&b, "A").held_by_peer);

        // B goes away, writes past the window itself, and returns upgraded.
        link(&switch, &a, &b, false);
        until("the stream is down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
        for n in 0..24 {
            b.store.append(sb, &asst(&big(64 * 1024, n))).await.unwrap();
        }
        b.shipper.node().set_schema(ours);
        link(&switch, &a, &b, true);
        until("both backlogs crossed", || async {
            texts(&a.store, sa).await == texts(&b.store, sa).await
                && texts(&a.store, sb).await == texts(&b.store, sb).await
                && sessions(&a.store).await == sessions(&b.store).await
        })
        .await;
        assert_eq!(texts(&b.store, sa).await.len(), 24);
        assert_eq!(texts(&a.store, sb).await.len(), 25);
        let (ha, hb) = (a.store.log_seq(), b.store.log_seq());
        until("each side's ack is back", || async {
            acked(&a.store, "A", "B").await == Some(ha)
                && acked(&b.store, "B", "A").await == Some(hb)
        })
        .await;
        assert!(!sync_of(&a, "B").held && !sync_of(&b, "A").held_by_peer);
    }

    /// A log row over one frame (a fork of a long session: one transaction,
    /// one changeset) never goes on the wire — the codec refused it and the
    /// stream failed at the same row on every redial. The peer gets a
    /// snapshot, every part under a frame, and both directions converge.
    #[tokio::test]
    async fn a_log_row_over_one_frame_goes_as_a_snapshot_and_both_ways_converge() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        for n in 0..3 {
            a.store.append(id, &user(&big(23 << 20, n))).await.unwrap();
        }
        let fork = a.store.fork(id, None, None).await.unwrap();
        let widest = a
            .store
            .log_after(0)
            .await
            .unwrap()
            .iter()
            .map(|r| r.changeset.len())
            .max()
            .unwrap();
        assert!(
            widest > super::ROW_MAX,
            "the fork is one row over a frame: {widest}"
        );
        let sb = SessionId::new();
        b.store
            .create_session(sb, "m", None, None, None)
            .await
            .unwrap();
        b.store.append(sb, &user("from B")).await.unwrap();

        link(&switch, &a, &b, true);
        let head = a.store.log_seq();
        until("B applied A to its head, A applied B", || async {
            acked(&b.store, "A", "").await == Some(head) && texts(&a.store, sb).await.len() == 1
        })
        .await;
        assert_eq!(texts(&a.store, fork).await, texts(&b.store, fork).await);
        assert_eq!(texts(&a.store, id).await, texts(&b.store, id).await);
        assert_eq!(texts(&b.store, fork).await.len(), 3);
        until("A has B's ack", || async {
            acked(&a.store, "A", "B").await == Some(head)
        })
        .await;
        // Live rows follow the snapshot, both ways.
        a.store.append(fork, &asst("after")).await.unwrap();
        b.store.append(sb, &asst("after")).await.unwrap();
        until("live both ways", || async {
            acked(&b.store, "A", "").await == Some(head + 1) && texts(&a.store, sb).await.len() == 2
        })
        .await;
    }

    /// Shipping one origin keeps failing (here on purpose): it is reported
    /// and retried, and the stream stays up — the other side's rows are
    /// still applied and acked. Once the failure clears, the rows go.
    #[tokio::test]
    async fn a_failing_origin_does_not_stop_the_other_way() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let sa = SessionId::new();
        a.store
            .create_session(sa, "m", None, None, None)
            .await
            .unwrap();
        until("B mirrors A", || same_session(&a.store, &b.store, sa)).await;

        a.shipper.node().fail_ship.lock().insert(a.me());
        a.store.append(sa, &user("waits on A")).await.unwrap();
        let sb = SessionId::new();
        b.store
            .create_session(sb, "m", None, None, None)
            .await
            .unwrap();
        for n in 0..3 {
            b.store.append(sb, &user(&format!("b{n}"))).await.unwrap();
        }
        let hb = b.store.log_seq();
        until("A applies and acks B's rows", || async {
            texts(&a.store, sb).await.len() == 3 && acked(&b.store, "B", "A").await == Some(hb)
        })
        .await;
        assert!(texts(&b.store, sa).await.is_empty(), "A's row waits on A");
        let s = sync_of(&a, "B");
        assert!(s.up, "the stream stayed up: {s:?}");
        assert!(
            s.last_error
                .as_deref()
                .is_some_and(|e| e.starts_with("ship: ") && e.contains("test: shipping A fails")),
            "{s:?}"
        );

        a.shipper.node().fail_ship.lock().clear();
        a.store.append(sa, &asst("goes now")).await.unwrap();
        until("A's rows follow", || async {
            texts(&b.store, sa).await.len() == 2
        })
        .await;
    }

    /// A peer away so long that the sender pruned its log past the peer's
    /// cursor gets a snapshot; the backlogs of both sides converge.
    #[tokio::test]
    async fn a_peer_offline_past_the_pruned_log_gets_a_snapshot() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let (sa, sb) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(sa, "m", None, None, None)
            .await
            .unwrap();
        b.store
            .create_session(sb, "m", None, None, None)
            .await
            .unwrap();
        let both = || async {
            texts(&a.store, sa).await == texts(&b.store, sa).await
                && texts(&a.store, sb).await == texts(&b.store, sb).await
                && sessions(&a.store).await == sessions(&b.store).await
        };
        until("mirrored", both).await;
        let ha = a.store.log_seq();
        until("B acked A", || async {
            acked(&a.store, "A", "B").await == Some(ha)
        })
        .await;

        link(&switch, &a, &b, false);
        until("the stream is down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
        for n in 0..30 {
            a.store.append(sa, &user(&big(48 * 1024, n))).await.unwrap();
            b.store.append(sb, &asst(&big(48 * 1024, n))).await.unwrap();
        }
        // Days later: A's log no longer reaches back to B's cursor.
        a.store.prune_log_below(a.store.log_seq()).await.unwrap();
        assert!(
            a.store
                .log_after(0)
                .await
                .unwrap()
                .iter()
                .all(|r| r.seq > ha + 1)
        );
        link(&switch, &a, &b, true);
        until("both converge", both).await;
        assert_eq!(texts(&b.store, sa).await.len(), 30);
        let head = a.store.log_seq();
        until("B's cursor for A is A's head", || async {
            acked(&b.store, "A", "").await == Some(head)
        })
        .await;
    }

    /// The status surface (`sessions_sync`, `mesh_peers`): held either way,
    /// both log positions, and the rows behind each way.
    #[tokio::test]
    async fn the_sync_status_reports_lag_and_hold() {
        let ours = agent::store::SESSIONS_WIRE_SCHEMA;
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        b.shipper.node().set_schema(ours - 1);
        let (sa, sb) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(sa, "m", None, None, None)
            .await
            .unwrap();
        for n in 0..3 {
            a.store.append(sa, &user(&format!("a{n}"))).await.unwrap();
        }
        b.store
            .create_session(sb, "m", None, None, None)
            .await
            .unwrap();
        b.store.append(sb, &user("b")).await.unwrap();
        let ha = u64::try_from(a.store.log_seq()).unwrap();
        let hb = u64::try_from(b.store.log_seq()).unwrap();
        assert_eq!(sync_of(&a, "B"), crate::types::PeerSync::default());

        link(&switch, &a, &b, true);
        until("A applied and acked B", || async {
            texts(&a.store, sb).await.len() == 1 && sync_of(&b, "A").acked == Some(hb)
        })
        .await;
        let s = sync_of(&a, "B");
        assert!(s.up && s.since.is_some() && s.last_hello.is_some(), "{s:?}");
        assert_eq!(
            (s.schema, s.held, s.held_by_peer),
            (Some(ours - 1), true, false)
        );
        assert_eq!((s.head, s.acked, s.behind_out), (ha, Some(0), Some(ha)));
        assert_eq!(
            (s.peer_head, s.applied, s.behind_in),
            (Some(hb), Some(hb), Some(0))
        );
        let s = sync_of(&b, "A");
        assert_eq!(
            (s.schema, s.held, s.held_by_peer),
            (Some(ours), false, true)
        );
        assert_eq!((s.head, s.acked, s.behind_out), (hb, Some(hb), Some(0)));
        assert_eq!(
            (s.peer_head, s.applied, s.behind_in),
            (Some(ha), None, Some(ha))
        );
        let line = b.shipper.node().peer_note("A").unwrap();
        assert!(
            line.starts_with("sessions=up since ")
                && line.contains(&format!(" in={ha} behind (applied ?/{ha})"))
                && line.contains("held_by_peer"),
            "{line}"
        );

        link(&switch, &a, &b, false);
        until("both see the stream down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
    }

    /// A row that keeps failing to apply: each failure ends the stream and
    /// the next one retries from the cursor; the third failure of the same
    /// seq asks for one snapshot (that `Hello` leaves the cursor out). When
    /// the snapshot fails too, none is asked again within `RESNAP_EVERY`:
    /// the streams go on retrying from the cursor (the dialer backs off).
    #[tokio::test]
    async fn a_row_that_keeps_failing_asks_for_one_snapshot_then_retries_from_the_cursor() {
        use futures::SinkExt as _;
        let ours = Some(agent::store::SESSIONS_WIRE_SCHEMA);
        let (store, _dir, _) = engine_a().await;
        store.set_cursor("Z", "", 5).await.unwrap();
        let node = super::Node::new(store.clone(), None, Duration::from_secs(60));
        let z = PeerId("Z".into());
        let bad_row = crate::types::Msg::Log {
            origin: z.clone(),
            rows: vec![LogRow {
                seq: 6,
                session_id: SessionId::new().to_string(),
                epoch: 1,
                changeset: vec![0xff; 32],
                hlc: String::new(),
            }],
        };
        let bad_snapshot = crate::types::Msg::Snapshot {
            origin: z.clone(),
            current_to_seq: 9,
            tables: vec![("not_replicated".into(), vec![1])],
        };
        let mut hellos = Vec::new();
        for _ in 0..9 {
            let (mut peer, run, held, _) = raw_on(node.clone(), ours, vec![]).await;
            let at = held.iter().find(|(o, _)| *o == z).map(|(_, s)| *s);
            hellos.push(at);
            // Asked for a snapshot, Z sends one that does not apply either.
            let frame = if at.is_none() {
                &bad_snapshot
            } else {
                &bad_row
            };
            peer.send(frame.encode().unwrap()).await.unwrap();
            let err = run.await.unwrap().unwrap_err();
            assert_eq!(
                err.downcast_ref::<super::Phase>(),
                Some(&super::Phase::Apply),
                "{err:#}"
            );
        }
        let c = Some(5);
        assert_eq!(hellos, [c, c, c, None, c, c, c, c, c]);
        assert!(
            node.sync_status(&[]).iter().any(|s| s.peer == "Z"
                && s.last_error.as_deref().is_some_and(
                    |e| e.starts_with("apply: row 6 ") && e.contains("retries from the cursor")
                )),
            "{:?}",
            node.sync_status(&[])
        );
    }

    /// A hole in the origin's own log (a seq an older build burned) is
    /// stepped over on the direct stream: every row after it applies at
    /// once — no rows parked behind it, no gap wait, no resync, no snapshot.
    #[tokio::test]
    async fn a_hole_in_the_origins_log_is_stepped_over_on_the_direct_stream() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let first = a.store.log_head();
        until("B holds A's first row", || async {
            acked(&b.store, "A", "").await == Some(first)
        })
        .await;
        link(&switch, &a, &b, false);
        until("the stream is down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
        for n in 0..10 {
            a.store.append(id, &user(&format!("{n}"))).await.unwrap();
        }
        let head = a.store.log_head();
        // Seq head - 5 was never committed: it is not in A's log.
        let mut tx = a.store.begin_apply().await.unwrap();
        sqlx::query("DELETE FROM session_log WHERE seq = ?")
            .bind(head - 5)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        link(&switch, &a, &b, true);
        until("B applied A to its head", || async {
            acked(&b.store, "A", "").await == Some(head)
        })
        .await;
        assert_eq!(
            texts(&b.store, id).await.len(),
            9,
            "all but the hole's message"
        );
        let s = sync_of(&b, "A");
        assert!(s.last_error.is_none(), "no gap, no resync: {s:?}");
    }

    /// Rows of a relayed origin may wait behind a gap, but at most
    /// `BATCH_ROWS` of them: one more ends the stream as a gap resync and
    /// drops what waited (bounded memory).
    #[tokio::test]
    async fn rows_waiting_behind_a_relayed_gap_are_capped() {
        use futures::SinkExt as _;
        let (store, _dir, _) = engine_a().await;
        let node = super::Node::new(store.clone(), None, Duration::from_secs(60));
        let ours = Some(agent::store::SESSIONS_WIRE_SCHEMA);
        let (mut z, run, _, _) = raw_on(node.clone(), ours, vec![]).await;
        let row = |seq| LogRow {
            seq,
            session_id: SessionId::new().to_string(),
            epoch: 1,
            changeset: Vec::new(),
            hlc: String::new(),
        };
        // C's rows relayed by Z from seq 2 on: row 1 never comes.
        let rows: Vec<LogRow> = (2..=super::BATCH_ROWS as u64 + 2).map(row).collect();
        z.send(
            crate::types::Msg::Log {
                origin: PeerId("C".into()),
                rows,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
        let err = run.await.unwrap().unwrap_err();
        assert_eq!(
            err.downcast_ref::<super::Phase>(),
            Some(&super::Phase::Gap),
            "{err:#}"
        );
        assert!(node.pending.lock().is_empty());
    }

    /// A peer that stops reading (the 2026-11 wedge, seen from one side):
    /// the waiting send is reported with peer, phase, bytes and seconds
    /// while the stream is still up, instead of hanging in silence.
    #[tokio::test]
    async fn a_send_the_peer_does_not_take_is_reported_while_it_waits() {
        let (store, _dir, id) = engine_a().await;
        for n in 0..24 {
            store.append(id, &user(&big(64 * 1024, n))).await.unwrap();
        }
        let node = super::Node::new(store.clone(), None, Duration::from_secs(60));
        let ours = Some(agent::store::SESSIONS_WIRE_SCHEMA);
        // Z holds A through 1 and never reads: the 1.5 MB tail outgrows the 1 MiB duplex.
        let (_z, run, _, _) = raw_on(node.clone(), ours, vec![(PeerId("A".into()), 1)]).await;
        until("the stalled send is reported", || async {
            node.sync_status(&[]).iter().any(|s| {
                s.peer == "Z"
                    && s.up
                    && s.last_error.as_deref().is_some_and(|e| {
                        e.starts_with("ship: ") && e.contains("waiting for the peer to read")
                    })
            })
        })
        .await;
        assert!(
            !run.is_finished(),
            "the stream waits; it ends only after SEND_STALL"
        );
    }

    /// A write dropped after its commit took a seq but before COMMIT: the
    /// next write reuses that seq, so a snapshot built meanwhile must not
    /// claim it — a peer whose cursor passed it would drop the reused row as
    /// a duplicate, forever.
    #[tokio::test]
    async fn a_snapshot_does_not_claim_the_seq_of_a_cancelled_commit() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("one")).await.unwrap();
        let before = a.store.log_seq();
        let msg = user("cancelled");
        let mut write = Box::pin(a.store.append(id, &msg));
        // Poll until the commit took its seq, then drop it (before COMMIT).
        std::future::poll_fn(|cx| match write.as_mut().poll(cx) {
            std::task::Poll::Ready(r) => panic!("finished before it could be dropped: {r:?}"),
            std::task::Poll::Pending if a.store.log_seq() > before => std::task::Poll::Ready(()),
            std::task::Poll::Pending => std::task::Poll::Pending,
        })
        .await;
        drop(write);

        // B has no cursor for A: it bootstraps from a snapshot of A.
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        until("B bootstrapped from A", || async {
            texts(&b.store, id).await == texts(&a.store, id).await
        })
        .await;
        a.store.append(id, &asst("after")).await.unwrap();
        until("B has the row written after the cancelled one", || async {
            texts(&b.store, id).await == texts(&a.store, id).await
        })
        .await;
        assert_eq!(
            texts(&b.store, id).await.last().map(|t| t.2.clone()),
            Some("after".to_owned())
        );
    }

    /// A hole right after the peer's cursor: the frame starts with the row
    /// at the cursor as an anchor (a duplicate there), and the peer steps
    /// over the hole — log rows, not a full snapshot.
    #[tokio::test]
    async fn a_hole_right_after_the_cursor_ships_the_log_with_an_anchor() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user("0")).await.unwrap();
        let cursor = a.store.log_head();
        until("B acked A", || async {
            acked(&a.store, "A", "B").await == Some(cursor)
        })
        .await;
        link(&switch, &a, &b, false);
        until("the stream is down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
        for n in 1..=5 {
            a.store.append(id, &user(&format!("{n}"))).await.unwrap();
        }
        let head = a.store.log_head();
        // Seq cursor + 1 was never committed; prune kept the cursor's row.
        crate::prune::tick(&a.store, &a.me(), 7).await.unwrap();
        let mut tx = a.store.begin_apply().await.unwrap();
        sqlx::query("DELETE FROM session_log WHERE seq = ?")
            .bind(cursor + 1)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            a.store.log_page(cursor - 1, 2).await.unwrap()[0].seq,
            cursor,
            "the anchor row is there"
        );

        link(&switch, &a, &b, true);
        until("B applied A to its head", || async {
            acked(&b.store, "A", "").await == Some(head)
        })
        .await;
        // A snapshot would have carried the hole's message too.
        assert_eq!(
            texts(&b.store, id).await.len(),
            5,
            "log rows, not a snapshot"
        );
        assert!(sync_of(&b, "A").last_error.is_none());
    }

    /// A snapshot request waits for a `Hello` that can answer it: a stream
    /// to a peer that never said it holds the origin leaves it pending; the
    /// next `Hello` to one whose last `Hello` listed the origin takes it.
    #[tokio::test]
    async fn a_snapshot_request_waits_for_a_peer_that_holds_the_origin() {
        let ours = Some(agent::store::SESSIONS_WIRE_SCHEMA);
        let (store, _dir, _) = engine_a().await;
        store.set_cursor("C", "", 4).await.unwrap();
        let node = super::Node::new(store.clone(), None, Duration::from_secs(60));
        let c = PeerId("C".into());
        node.resnap.lock().insert(c.clone());
        let c_at = |held: &[(PeerId, u64)]| held.iter().find(|(o, _)| *o == c).map(|(_, s)| *s);

        // Z has said nothing about C yet: the request stays pending.
        let (z, run, held, _) = raw_on(node.clone(), ours, vec![(c.clone(), 4)]).await;
        assert_eq!(c_at(&held), Some(4));
        assert!(node.resnap.lock().contains(&c));
        drop(z);
        let _ = run.await;
        // Z's last `Hello` listed C: this `Hello` asks it for C's snapshot.
        let (_z, _run, held, _) = raw_on(node.clone(), ours, vec![(c.clone(), 4)]).await;
        assert_eq!(c_at(&held), None);
        assert!(node.resnap.lock().is_empty());
    }

    /// An anchor that cannot share a frame with the row after the hole (the
    /// two over `ROW_MAX`) would leave the hole at a frame start, where the
    /// peer waits, resyncs and is sent the same frames again: a snapshot
    /// instead, and the peer converges.
    #[tokio::test]
    async fn an_anchor_too_big_to_share_a_frame_falls_back_to_a_snapshot() {
        let switch = Arc::new(Switch::default());
        let a = node("A", &switch).await;
        let b = node("B", &switch).await;
        link(&switch, &a, &b, true);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &user(&big(34 << 20, 0))).await.unwrap();
        let cursor = a.store.log_head();
        until("B acked A", || async {
            acked(&a.store, "A", "B").await == Some(cursor)
        })
        .await;
        link(&switch, &a, &b, false);
        until("the stream is down", || async {
            !sync_of(&a, "B").up && !sync_of(&b, "A").up
        })
        .await;
        a.store.append(id, &user("hole")).await.unwrap();
        a.store.append(id, &user(&big(34 << 20, 1))).await.unwrap();
        let head = a.store.log_head();
        let mut tx = a.store.begin_apply().await.unwrap();
        sqlx::query("DELETE FROM session_log WHERE seq = ?")
            .bind(cursor + 1)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        link(&switch, &a, &b, true);
        until("B applied A to its head", || async {
            acked(&b.store, "A", "").await == Some(head)
        })
        .await;
        // Only a snapshot carries the hole row's message.
        assert_eq!(texts(&b.store, id).await, texts(&a.store, id).await);
        assert_eq!(texts(&b.store, id).await.len(), 3);
    }
}
