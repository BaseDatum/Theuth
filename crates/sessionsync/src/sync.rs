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
//! reorder buffer absorbs. Errors end the stream; the dialer backs off and
//! the `Hello` exchange resynchronizes.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent::AgentEvent;
use agent::store::Store;
use futures::{SinkExt as _, StreamExt as _};
use parking_lot::Mutex;
use replica::{ByteStream, PeerId};
use runtime::{Runtime, SessionId};
use tokio::sync::{Notify, OwnedMutexGuard, broadcast};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::apply::{Applied, apply_log_row, build_snapshot};
use crate::snapshot::{self, AppliedSession};
use crate::types::{LogRow, Msg};

/// Rows per [`Msg::Log`] frame during catch-up.
pub const BATCH_ROWS: usize = 5000;
/// Changeset bytes per [`Msg::Log`] frame during catch-up.
pub const BATCH_BYTES: usize = 32 * 1024 * 1024;
/// Relay feed depth before a slow stream is told to resync.
const RELAY_CAPACITY: usize = 1024;
/// How long a gap before out-of-order rows may stay open before the stream resyncs.
const GAP_WAIT: Duration = Duration::from_secs(10);

/// One `theuth/sessions/1` stream, framed (see [`crate::events::framed`]).
pub type Stream = Framed<Box<dyn ByteStream>, LengthDelimitedCodec>;

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
    pending: Mutex<HashMap<PeerId, (Instant, BTreeMap<u64, LogRow>)>>,
}

impl Node {
    /// A node over `store`; `rt` publishes `AgentEvent`s when present.
    pub fn new(store: Store, rt: Option<Runtime>) -> Arc<Self> {
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
        })
    }

    /// Wake every stream to ship whatever is unshipped.
    pub fn kick(&self) {
        self.kick.notify_waiters();
    }

    /// Every origin we hold rows for and how far: our log head, then every
    /// applied cursor. Our own origin is offered even at head 0 when we own
    /// sessions (a migrated store that has not written since: its peers
    /// bootstrap from a snapshot current to seq 0); a cursor row at 0 is
    /// likewise "held" — the row exists only once a snapshot landed.
    async fn held_here(&self) -> anyhow::Result<Vec<(PeerId, u64)>> {
        let mut out = Vec::new();
        let head = u64::try_from(self.store.log_seq()).unwrap_or(0);
        if head > 0 || self.store.owns_sessions().await? {
            out.push((self.me.clone(), head));
        }
        for c in self.store.cursors().await? {
            if c.peer.is_empty() && c.origin != self.me.0 {
                out.push((PeerId(c.origin), u64::try_from(c.seq).unwrap_or(0)));
            }
        }
        Ok(out)
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
        })
        .await;
    }
}

/// A multi-part snapshot of one origin being received on this stream.
struct InFlight {
    _guard: OwnedMutexGuard<()>,
    fenced: HashSet<String>,
    sessions: Vec<AppliedSession>,
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

async fn run(
    node: &Arc<Node>,
    remote: PeerId,
    mut framed: Stream,
    first: Option<Msg>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !node.me.0.is_empty(),
        "lone engine (no engine id): nothing ships"
    );
    // Subscribe before reading the cursors so nothing falls between.
    let mut relay = node.relay.subscribe();
    let mut log = node.store.log_watch();
    log.mark_unchanged();

    let ours = node.held_here().await?;
    framed
        .send(
            Msg::Hello {
                cursors: ours.clone(),
            }
            .encode()?,
        )
        .await?;
    let first = match first {
        Some(m) => m,
        None => Msg::decode(
            &framed
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed before hello"))??,
        )?,
    };
    let Msg::Hello { cursors: theirs } = first else {
        anyhow::bail!("expected hello");
    };
    node.learn(&remote, &theirs);
    if let Some((_, seq)) = theirs.iter().find(|(o, _)| *o == node.me) {
        node.store
            .set_cursor(&node.me.0, &remote.0, i64::try_from(*seq)?)
            .await?;
    }

    // Catch-up: every origin we hold above what they hold.
    for (origin, seq) in &ours {
        let after = theirs.iter().find(|(o, _)| o == origin).map(|(_, s)| *s);
        match after {
            Some(a) if a >= *seq => {}
            Some(a) if *origin == node.me => {
                if !ship_log(node, &remote, &mut framed, a).await? {
                    send_snapshot(node, &remote, origin, &mut framed).await?;
                }
            }
            // No cursor, or a relayed origin (we hold its rows, not its log).
            _ => send_snapshot(node, &remote, origin, &mut framed).await?,
        }
    }

    let mut inflight: HashMap<PeerId, InFlight> = HashMap::new();
    loop {
        tokio::select! {
            inbound = framed.next() => {
                let Some(bytes) = inbound else { return Ok(()) };
                match Msg::decode(&bytes?)? {
                    Msg::Hello { .. } => {}
                    Msg::Log { origin, rows } => {
                        let Some(last) = rows.last().map(|r| r.seq) else { continue };
                        if origin == node.me {
                            continue;
                        }
                        node.mark(&remote, &origin, last);
                        if let Some(seq) = receive_log(node, &origin, &rows).await? {
                            framed.send(Msg::Ack { origin, seq }.encode()?).await?;
                        }
                    }
                    part @ Msg::Snapshot { .. } => receive_snapshot(node, &remote, part, &mut inflight, &mut framed).await?,
                    Msg::Ack { origin, seq } => {
                        node.mark(&remote, &origin, seq);
                        if origin == node.me {
                            node.store.set_cursor(&node.me.0, &remote.0, i64::try_from(seq)?).await?;
                        }
                    }
                    other => tracing::debug!("sessions: frame {} from {remote} has no handler on this stream", other.tag()),
                }
            }
            changed = log.changed() => {
                changed?;
                ship_own(node, &remote, &mut framed).await?;
            }
            () = node.kick.notified() => ship_own(node, &remote, &mut framed).await?,
            relayed = relay.recv() => {
                match relayed {
                    Ok((origin, row)) => {
                        if origin == remote {
                            continue;
                        }
                        match node.holds(&remote, &origin) {
                            // They have nothing of this origin yet: bootstrap them.
                            0 => send_snapshot(node, &remote, &origin, &mut framed).await?,
                            held if held < row.seq => {
                                node.mark(&remote, &origin, row.seq);
                                framed.send(Msg::Log { origin, rows: vec![row] }.encode()?).await?;
                            }
                            _ => {}
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => anyhow::bail!("relay lagged by {n}; resyncing"),
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }
}

/// Our own rows after what `remote` holds; a snapshot if the log no longer
/// reaches back that far.
async fn ship_own(node: &Arc<Node>, remote: &PeerId, framed: &mut Stream) -> anyhow::Result<()> {
    let after = node.holds(remote, &node.me);
    if !ship_log(node, remote, framed, after).await? {
        send_snapshot(node, remote, &node.me, framed).await?;
    }
    Ok(())
}

/// Send our log rows with `seq > after` in frame-sized batches. `false`
/// when the log does not start right after `after` (pruned, or an orphaned
/// run we cannot tell apart): the caller sends a snapshot instead.
async fn ship_log(
    node: &Arc<Node>,
    remote: &PeerId,
    framed: &mut Stream,
    after: u64,
) -> anyhow::Result<bool> {
    let rows = node.store.log_after(i64::try_from(after)?).await?;
    let head = u64::try_from(node.store.log_seq()).unwrap_or(0);
    match rows.first() {
        None => return Ok(head <= after),
        Some(r) if u64::try_from(r.seq).unwrap_or(0) != after + 1 => return Ok(false),
        Some(_) => {}
    }
    let mut batch: Vec<LogRow> = Vec::new();
    let mut size = 0;
    for row in rows {
        let row = LogRow::from(row);
        if !batch.is_empty()
            && (batch.len() >= BATCH_ROWS || size + row.changeset.len() > BATCH_BYTES)
        {
            send_batch(node, remote, framed, std::mem::take(&mut batch)).await?;
            size = 0;
        }
        size += row.changeset.len();
        batch.push(row);
    }
    if !batch.is_empty() {
        send_batch(node, remote, framed, batch).await?;
    }
    Ok(true)
}

async fn send_batch(
    node: &Arc<Node>,
    remote: &PeerId,
    framed: &mut Stream,
    rows: Vec<LogRow>,
) -> anyhow::Result<()> {
    let last = rows.last().map_or(0, |r| r.seq);
    node.mark(remote, &node.me, last);
    framed
        .send(
            Msg::Log {
                origin: node.me.clone(),
                rows,
            }
            .encode()?,
        )
        .await?;
    Ok(())
}

/// Build and send the snapshot of `origin` in parts.
async fn send_snapshot(
    node: &Arc<Node>,
    remote: &PeerId,
    origin: &PeerId,
    framed: &mut Stream,
) -> anyhow::Result<()> {
    let snap = build_snapshot(&node.store, origin).await?;
    let Msg::Snapshot { current_to_seq, .. } = &snap else {
        anyhow::bail!("build_snapshot returned {}", snap.tag());
    };
    let seq = *current_to_seq;
    for part in snapshot::split(snap) {
        framed.send(part.encode()?).await?;
    }
    node.mark(remote, origin, seq);
    tracing::debug!("sessions: snapshot of {origin} (to seq {seq}) sent to {remote}");
    Ok(())
}

/// Apply a batch of `origin`'s rows **in seq order**: a row right after our
/// cursor applies (and drains what waited behind it), a row at or below it
/// is a duplicate, a row further ahead waits in the origin's reorder buffer
/// — a second stream (relay beside the direct link, a reconnect overlap)
/// may deliver ahead. A gap open for [`GAP_WAIT`] ends the stream: the
/// `Hello` exchange re-sends from the cursor, or snapshots over an orphaned
/// run. Returns the cursor when it moved (what to ack).
async fn receive_log(
    node: &Arc<Node>,
    origin: &PeerId,
    rows: &[LogRow],
) -> anyhow::Result<Option<u64>> {
    let lock = node.origin_lock(origin);
    let _guard = lock.lock().await;
    let start = node.cursor(origin).await?.unwrap_or(0);
    let mut cursor = start;
    // Session → its `(owner, epoch)` before the first row applied to it.
    let mut touched: HashMap<SessionId, Option<(String, u32)>> = HashMap::new();
    let mut queue: VecDeque<LogRow> = rows.iter().cloned().collect();
    while let Some(row) = queue.pop_front() {
        if row.seq <= cursor {
            continue;
        }
        if row.seq > cursor + 1 {
            let mut pending = node.pending.lock();
            let (since, map) = pending
                .entry(origin.clone())
                .or_insert_with(|| (Instant::now(), BTreeMap::new()));
            anyhow::ensure!(
                since.elapsed() < GAP_WAIT,
                "rows of {origin} after seq {cursor} missing for {GAP_WAIT:?}; resyncing"
            );
            map.insert(row.seq, row);
            continue;
        }
        let session: SessionId = row.session_id.parse()?;
        let before = match touched.get(&session) {
            Some(b) => b.clone(),
            None => node.store.owner(session).await?,
        };
        match apply_log_row(&node.store, origin, &row).await? {
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
/// lock, which the empty terminator releases after moving the cursor.
async fn receive_snapshot(
    node: &Arc<Node>,
    remote: &PeerId,
    part: Msg,
    inflight: &mut HashMap<PeerId, InFlight>,
    framed: &mut Stream,
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
        let applied = snapshot::apply(&node.store, &part, &mut state.fenced, last).await?;
        state.sessions.extend(applied);
    }
    if !last {
        return Ok(());
    }
    let state = inflight.remove(origin).expect("present");
    node.mark(remote, origin, *current_to_seq);
    framed
        .send(
            Msg::Ack {
                origin: origin.clone(),
                seq: *current_to_seq,
            }
            .encode()?,
        )
        .await?;
    tracing::debug!(
        "sessions: snapshot of {origin} (to seq {current_to_seq}) applied from {remote}"
    );
    for (id, owner, epoch) in state.sessions {
        if let Ok(session) = id.parse() {
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
        node_in(name, tempfile::tempdir().unwrap(), switch).await
    }

    /// A node over an existing data dir (its `sessions.sqlite` may hold rows already).
    async fn node_in(name: &str, dir: tempfile::TempDir, switch: &Arc<Switch>) -> Node {
        let me = PeerId(name.to_owned());
        let store = Store::open(&dir.path().join("sessions.sqlite"), Some(me.clone().into()))
            .await
            .unwrap();
        let shipper = Shipper::new(store.clone(), None, &SessionsConfig::default());
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
        let head = i64::try_from(a.store.log_seq()).unwrap();
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
        let a = node_in("A", dir, &switch).await;
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
            let head = i64::try_from(a.store.log_seq()).unwrap();
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
        let head = i64::try_from(a.store.log_seq()).unwrap();
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
        assert_eq!(left, ((head + 4)..=(head + 10)).collect::<Vec<_>>());
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
}
