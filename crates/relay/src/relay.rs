//! Relay state and the two socket loops (design doc 31 §4.2, §4.5, §5.1).
//!
//! One task per socket. A device socket is *parked* until its harness
//! registers, then owns one `conn` on that harness's [`Uplink`]. The copy
//! path is header-in-place, payload as a [`Bytes`] slice, `try_send` into
//! the peer's bounded queue; no locks per frame beyond a `DashMap` shard read.

use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::{Notify, mpsc};
use tokio::time::{self, Interval, MissedTickBehavior, Sleep};
use tokio_util::sync::CancellationToken;

use crate::frame::{self, reason};

/// WebSocket subprotocol every socket must request.
pub const PROTOCOL: &str = "theuth-link-1";
const NOISE: &str = "Noise_NK_25519_AESGCM_SHA256";
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(30);
const PONG_TIMEOUT: Duration = Duration::from_secs(15);
const DEVICE_IDLE: Duration = Duration::from_secs(600);
const UPLINK_QUEUE: usize = 4096;
const CONN_QUEUE: usize = 256;
const RATE_PER_SEC: f64 = 10.0;
const RATE_BURST: f64 = 30.0;

/// Plain-text counters for `/metrics`.
#[derive(Default)]
pub struct Metrics {
    /// Open WebSockets (uplinks + devices).
    pub sockets: AtomicU64,
    /// Device sockets waiting for their harness.
    pub parked: AtomicU64,
    /// Binary DATA frames relayed (both directions).
    pub frames_total: AtomicU64,
    /// DATA payload bytes relayed.
    pub bytes_total: AtomicU64,
    /// CLOSE frames sent, by reason.
    pub close_reason: [AtomicU64; reason::COUNT],
    /// Upgrades refused with 429.
    pub rate_limited: AtomicU64,
    /// Upgrades refused with 403 (same-origin without a valid cookie).
    pub cookie_rejected: AtomicU64,
}

impl Metrics {
    fn frame(&self, len: usize) {
        self.frames_total.fetch_add(1, Ordering::Relaxed);
        self.bytes_total.fetch_add(len as u64, Ordering::Relaxed);
    }

    fn closed(&self, r: u8) {
        if let Some(c) = self.close_reason.get(r as usize) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Increments a gauge for its lifetime.
struct Gauge<'a>(&'a AtomicU64);

impl<'a> Gauge<'a> {
    fn new(g: &'a AtomicU64) -> Self {
        g.fetch_add(1, Ordering::Relaxed);
        Self(g)
    }
}

impl Drop for Gauge<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Outbound uplink frame; the uplink loop prepends the 5-byte header.
enum UpOut {
    Open(u32),
    Data(u32, Bytes),
    Close(u32, u8),
}

/// One device conn on an uplink. The map owns the only sender of the
/// per-conn DATA queue, so removing the conn ends the device's `rx`;
/// `reason` (shared with the device task) says why. Default `HARNESS_GONE`:
/// the only path that drops a sender without storing a reason first is
/// [`Uplink::kill`].
struct Conn {
    tx: mpsc::Sender<Bytes>,
    reason: Arc<AtomicU8>,
}

/// A registered harness uplink.
pub struct Uplink {
    tx: mpsc::Sender<UpOut>,
    conns: DashMap<u32, Conn>,
    next: AtomicU32,
    gone: CancellationToken,
}

impl Uplink {
    /// Number of live conns.
    pub fn conn_count(&self) -> usize {
        self.conns.len()
    }

    fn alloc(&self) -> u32 {
        loop {
            let c = self.next.fetch_add(1, Ordering::Relaxed);
            if c != 0 {
                return c;
            }
        }
    }

    /// Remove `conn`, recording why its device should be told.
    fn close_conn(&self, conn: u32, r: u8) -> bool {
        match self.conns.remove(&conn) {
            Some((_, c)) => {
                c.reason.store(r, Ordering::Release);
                true
            }
            None => false,
        }
    }

    /// Tear down: cancel, then drop every conn with `HARNESS_GONE`. Cancel
    /// happens-before `clear`, so a device inserting afterwards sees it.
    fn kill(&self) {
        if self.gone.is_cancelled() {
            return;
        }
        self.gone.cancel();
        self.conns.clear();
    }
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Shared relay state.
pub struct Relay {
    /// Harness id → live uplink.
    pub uplinks: DashMap<[u8; 32], Arc<Uplink>>,
    parked: DashMap<[u8; 32], Arc<Notify>>,
    buckets: DashMap<IpAddr, Bucket>,
    /// Counters.
    pub metrics: Metrics,
    /// `--max-conns`.
    pub max_conns: usize,
    /// Cancelled on SIGINT/SIGTERM.
    pub shutdown: CancellationToken,
}

impl Relay {
    /// Empty state.
    pub fn new(max_conns: usize) -> Self {
        Self {
            uplinks: DashMap::new(),
            parked: DashMap::new(),
            buckets: DashMap::new(),
            metrics: Metrics::default(),
            max_conns,
            shutdown: CancellationToken::new(),
        }
    }

    /// Per-IP token bucket (10/s, burst 30) for new sockets.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        if self.buckets.len() > 100_000 {
            // ponytail: prune only under pressure; a bucket idle 3 s is full again anyway.
            self.buckets
                .retain(|_, b| now.duration_since(b.at) < Duration::from_secs(3));
        }
        let mut b = self.buckets.entry(ip).or_insert(Bucket {
            tokens: RATE_BURST,
            at: now,
        });
        b.tokens =
            (b.tokens + now.duration_since(b.at).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            self.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Cancel every socket loop; each closes its peer with CLOSE 4 / PRESENCE 0.
    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// `/metrics` body.
    pub fn render_metrics(&self) -> String {
        use std::fmt::Write;
        let m = &self.metrics;
        let conns: usize = self.uplinks.iter().map(|u| u.conn_count()).sum();
        let mut s = String::with_capacity(256);
        let _ = writeln!(s, "sockets {}", m.sockets.load(Ordering::Relaxed));
        let _ = writeln!(s, "uplinks {}", self.uplinks.len());
        let _ = writeln!(s, "conns {conns}");
        let _ = writeln!(s, "parked {}", m.parked.load(Ordering::Relaxed));
        let _ = writeln!(s, "frames_total {}", m.frames_total.load(Ordering::Relaxed));
        let _ = writeln!(s, "bytes_total {}", m.bytes_total.load(Ordering::Relaxed));
        for (i, c) in m.close_reason.iter().enumerate() {
            let _ = writeln!(s, "close_reason_{i} {}", c.load(Ordering::Relaxed));
        }
        let _ = writeln!(s, "rate_limited {}", m.rate_limited.load(Ordering::Relaxed));
        let _ = writeln!(
            s,
            "cookie_rejected {}",
            m.cookie_rejected.load(Ordering::Relaxed)
        );
        s
    }

    fn register(&self, id: [u8; 32], up: Arc<Uplink>) {
        if let Some(old) = self.uplinks.insert(id, up) {
            old.kill();
        }
        if let Some((_, n)) = self.parked.remove(&id) {
            n.notify_waiters();
        }
    }

    /// Kill `up` and unregister it if it is still the live one for `id`.
    fn drop_uplink(&self, id: &[u8; 32], up: &Arc<Uplink>) {
        up.kill();
        self.uplinks.remove_if(id, |_, cur| Arc::ptr_eq(cur, up));
    }

    /// The live (not killed) uplink for `id`.
    fn live(&self, id: &[u8; 32]) -> Option<Arc<Uplink>> {
        self.uplinks
            .get(id)
            .filter(|u| !u.gone.is_cancelled())
            .map(|u| Arc::clone(&u))
    }

    fn parked_notify(&self, id: &[u8; 32]) -> Arc<Notify> {
        self.parked.entry(*id).or_default().clone()
    }
}

/// Keepalive and idle timers of one socket.
struct Timers {
    ping: Interval,
    pong: Pin<Box<Sleep>>,
    awaiting_pong: bool,
    idle: Pin<Box<Sleep>>,
}

impl Timers {
    async fn new() -> Self {
        let mut ping = time::interval(PING_EVERY);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ping.tick().await;
        Self {
            ping,
            pong: Box::pin(time::sleep(PONG_TIMEOUT)),
            awaiting_pong: false,
            idle: Box::pin(time::sleep(DEVICE_IDLE)),
        }
    }

    /// Send a ping and arm the pong deadline; `false` when the socket is gone.
    async fn send_ping(&mut self, ws: &mut WebSocket) -> bool {
        if ws.send(Message::Ping(Bytes::new())).await.is_err() {
            return false;
        }
        self.awaiting_pong = true;
        self.pong
            .as_mut()
            .reset(time::Instant::now() + PONG_TIMEOUT);
        true
    }

    fn touched(&mut self) {
        self.idle.as_mut().reset(time::Instant::now() + DEVICE_IDLE);
    }
}

async fn close(ws: &mut WebSocket, code: u16) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: "".into(),
        })))
        .await;
}

async fn send_device(ws: &mut WebSocket, op: u8, payload: &[u8]) -> bool {
    ws.send(Message::Binary(frame::encode_device(op, payload)))
        .await
        .is_ok()
}

/// Close code for a read error: tungstenite's size cap → 1009, anything else 1002.
fn error_code(e: &axum::Error) -> u16 {
    if e.to_string().contains("too long") {
        close_code::SIZE
    } else {
        close_code::PROTOCOL
    }
}

/// Serve one harness uplink: NK registration, then demux frames by conn.
pub async fn uplink(relay: Arc<Relay>, id: [u8; 32], mut ws: WebSocket) {
    let _sockets = Gauge::new(&relay.metrics.sockets);
    let m = &relay.metrics;

    let Some((up, mut rx)) = hello(&id, &mut ws).await else {
        return;
    };
    relay.register(id, up.clone());
    tracing::debug!(id = %base64url(&id), "uplink registered");
    let mut t = Timers::new().await;

    loop {
        tokio::select! {
            _ = up.gone.cancelled() => { close(&mut ws, close_code::AWAY).await; break }
            _ = relay.shutdown.cancelled() => { close(&mut ws, close_code::AWAY).await; break }
            out = rx.recv() => {
                let Some(out) = out else { break };
                let f = match out {
                    UpOut::Open(c) => frame::encode_uplink(frame::OPEN, c, &[]),
                    UpOut::Data(c, p) => { m.frame(p.len()); frame::encode_uplink(frame::DATA, c, &p) }
                    UpOut::Close(c, r) => { m.closed(r); frame::encode_uplink(frame::CLOSE, c, &[r]) }
                };
                if ws.send(Message::Binary(f)).await.is_err() { break }
            }
            msg = ws.recv() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    let Some(f) = frame::decode_uplink(&b) else { close(&mut ws, close_code::PROTOCOL).await; break };
                    match f.op {
                        frame::DATA => {
                            m.frame(f.payload.len());
                            let full = match up.conns.get(&f.conn) {
                                Some(c) => c.tx.try_send(f.payload).is_err(),
                                None => false,
                            };
                            if full && up.close_conn(f.conn, reason::QUEUE_FULL) {
                                m.closed(reason::QUEUE_FULL);
                                let f = frame::encode_uplink(frame::CLOSE, f.conn, &[reason::QUEUE_FULL]);
                                if ws.send(Message::Binary(f)).await.is_err() { break }
                            }
                        }
                        frame::CLOSE => {
                            let r = f.payload.first().copied().unwrap_or(reason::NORMAL);
                            up.close_conn(f.conn, r);
                        }
                        _ => { close(&mut ws, close_code::PROTOCOL).await; break }
                    }
                }
                Some(Ok(Message::Pong(_))) => t.awaiting_pong = false,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Text(_))) => { close(&mut ws, close_code::PROTOCOL).await; break }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => { close(&mut ws, error_code(&e)).await; break }
            },
            _ = t.ping.tick() => if !t.send_ping(&mut ws).await { break },
            _ = &mut t.pong, if t.awaiting_pong => break,
        }
    }
    relay.drop_uplink(&id, &up);
    tracing::debug!(id = %base64url(&id), "uplink gone");
}

/// NK registration: send msg1, verify msg2 within 10 s. Returns the new
/// uplink and its outbound queue receiver; `None` after closing the socket.
async fn hello(id: &[u8; 32], ws: &mut WebSocket) -> Option<(Arc<Uplink>, mpsc::Receiver<UpOut>)> {
    let mut prologue = Vec::with_capacity(10 + 43);
    prologue.extend_from_slice(PROTOCOL.as_bytes());
    prologue.extend_from_slice(base64url(id).as_bytes());
    let params = NOISE.parse().ok()?;
    let hs = snow::Builder::new(params)
        .remote_public_key(id)
        .and_then(|b| b.prologue(&prologue))
        .and_then(|b| b.build_initiator());
    let mut hs = match hs {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "noise init");
            close(ws, close_code::ERROR).await;
            return None;
        }
    };
    let mut buf = [0u8; 128];
    let n = hs.write_message(&[], &mut buf).ok()?;
    ws.send(Message::Binary(frame::encode_uplink(
        frame::HELLO,
        0,
        &buf[..n],
    )))
    .await
    .ok()?;

    let msg2 = match time::timeout(HELLO_TIMEOUT, ws.recv()).await {
        Ok(Some(Ok(Message::Binary(b)))) => b,
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => return None,
        Ok(Some(Err(e))) => {
            close(ws, error_code(&e)).await;
            return None;
        }
        Ok(Some(Ok(_))) => {
            close(ws, close_code::PROTOCOL).await;
            return None;
        }
        Err(_) => {
            close(ws, close_code::POLICY).await;
            return None;
        }
    };
    let ok = match frame::decode_uplink(&msg2) {
        Some(f) if f.op == frame::HELLO && f.conn == 0 => {
            hs.read_message(&f.payload, &mut buf).is_ok() && hs.is_handshake_finished()
        }
        _ => false,
    };
    if !ok {
        close(ws, close_code::POLICY).await;
        return None;
    }
    let (tx, rx) = mpsc::channel(UPLINK_QUEUE);
    let up = Arc::new(Uplink {
        tx,
        conns: DashMap::new(),
        next: AtomicU32::new(1),
        gone: CancellationToken::new(),
    });
    Some((up, rx))
}

/// Serve one device socket: park until the harness is online, then relay.
pub async fn device(relay: Arc<Relay>, id: [u8; 32], mut ws: WebSocket) {
    let _sockets = Gauge::new(&relay.metrics.sockets);
    let mut t = Timers::new().await;
    loop {
        let Some(up) = parked(&relay, &id, &mut ws, &mut t).await else {
            return;
        };
        if !online(&relay, &id, up, &mut ws, &mut t).await {
            return;
        }
    }
}

/// Wait for a live uplink. `None` when the socket is finished (closed by us or the device).
async fn parked(
    relay: &Relay,
    id: &[u8; 32],
    ws: &mut WebSocket,
    t: &mut Timers,
) -> Option<Arc<Uplink>> {
    let m = &relay.metrics;
    let mut announced = false;
    loop {
        if let Some(up) = relay.live(id) {
            return Some(up);
        }
        // Register interest before the second lookup so a registration
        // between the two cannot be missed.
        let notified = relay.parked_notify(id).notified_owned();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(up) = relay.live(id) {
            return Some(up);
        }
        if !announced {
            if !send_device(ws, frame::PRESENCE, &[0]).await {
                return None;
            }
            announced = true;
        }
        let _parked = Gauge::new(&m.parked);
        tokio::select! {
            _ = &mut notified => continue,
            _ = relay.shutdown.cancelled() => { close(ws, close_code::AWAY).await; return None }
            msg = ws.recv() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    match frame::decode_device(&b).map(|f| f.op) {
                        Some(frame::DATA) => {
                            m.closed(reason::REFUSED);
                            send_device(ws, frame::CLOSE, &[reason::REFUSED]).await;
                            close(ws, close_code::NORMAL).await;
                        }
                        _ => close(ws, close_code::PROTOCOL).await,
                    }
                    return None;
                }
                Some(Ok(Message::Pong(_))) => t.awaiting_pong = false,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Text(_))) => { close(ws, close_code::PROTOCOL).await; return None }
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Err(e)) => { close(ws, error_code(&e)).await; return None }
            },
            _ = t.ping.tick() => if !t.send_ping(ws).await { return None },
            _ = &mut t.pong, if t.awaiting_pong => return None,
            _ = &mut t.idle => {
                m.closed(reason::IDLE);
                send_device(ws, frame::CLOSE, &[reason::IDLE]).await;
                close(ws, close_code::NORMAL).await;
                return None;
            }
        }
    }
}

/// Own one conn on `up` until it ends. `true` → re-park (harness gone); `false` → socket finished.
async fn online(
    relay: &Relay,
    id: &[u8; 32],
    up: Arc<Uplink>,
    ws: &mut WebSocket,
    t: &mut Timers,
) -> bool {
    let m = &relay.metrics;
    if up.conn_count() >= relay.max_conns {
        m.closed(reason::REFUSED);
        send_device(ws, frame::CLOSE, &[reason::REFUSED]).await;
        close(ws, close_code::NORMAL).await;
        return false;
    }
    let conn = up.alloc();
    let (tx, mut rx) = mpsc::channel(CONN_QUEUE);
    let why = Arc::new(AtomicU8::new(reason::HARNESS_GONE));
    up.conns.insert(
        conn,
        Conn {
            tx,
            reason: why.clone(),
        },
    );
    if up.gone.is_cancelled() {
        up.conns.remove(&conn);
        return true;
    }
    if up.tx.try_send(UpOut::Open(conn)).is_err() {
        up.conns.remove(&conn);
        relay.drop_uplink(id, &up);
        return true;
    }
    if !send_device(ws, frame::PRESENCE, &[1]).await {
        if up.close_conn(conn, reason::NORMAL) {
            let _ = up.tx.try_send(UpOut::Close(conn, reason::NORMAL));
        }
        return false;
    }

    // Some(r): device side ended, harness hears CLOSE r. None: harness side ended (or shutdown).
    let device_ended: Option<u8> = loop {
        tokio::select! {
            _ = relay.shutdown.cancelled() => {
                m.closed(reason::HARNESS_GONE);
                send_device(ws, frame::CLOSE, &[reason::HARNESS_GONE]).await;
                send_device(ws, frame::PRESENCE, &[0]).await;
                close(ws, close_code::AWAY).await;
                break None;
            }
            item = rx.recv() => match item {
                Some(p) => {
                    t.touched();
                    if !send_device(ws, frame::DATA, &p).await { break Some(reason::NORMAL) }
                }
                None => {
                    let r = why.load(Ordering::Acquire);
                    m.closed(r);
                    if !send_device(ws, frame::CLOSE, &[r]).await { return false }
                    // 4: the uplink died; 5: the hub sent HUB_MOVED before it left (69 decision
                    // 6). Either way the device stays parked for the next registration.
                    if reason::reparks(r) { return true }
                    close(ws, close_code::NORMAL).await;
                    break None;
                }
            },
            msg = ws.recv() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    t.touched();
                    match frame::decode_device(&b) {
                        Some(f) if f.op == frame::DATA => {
                            // Full or the uplink task is gone: kill it; the harness reconnects and
                            // this socket re-parks when its conn queue closes with HARNESS_GONE.
                            if up.tx.try_send(UpOut::Data(conn, f.payload)).is_err() {
                                relay.drop_uplink(id, &up);
                            }
                        }
                        _ => { close(ws, close_code::PROTOCOL).await; break Some(reason::NORMAL) }
                    }
                }
                Some(Ok(Message::Pong(_))) => t.awaiting_pong = false,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Text(_))) => { close(ws, close_code::PROTOCOL).await; break Some(reason::NORMAL) }
                Some(Ok(Message::Close(_))) | None => break Some(reason::NORMAL),
                Some(Err(e)) => { close(ws, error_code(&e)).await; break Some(reason::NORMAL) }
            },
            _ = t.ping.tick() => if !t.send_ping(ws).await { break Some(reason::NORMAL) },
            _ = &mut t.pong, if t.awaiting_pong => break Some(reason::NORMAL),
            _ = &mut t.idle => {
                m.closed(reason::IDLE);
                send_device(ws, frame::CLOSE, &[reason::IDLE]).await;
                close(ws, close_code::NORMAL).await;
                break Some(reason::IDLE);
            }
        }
    };
    if let Some(r) = device_ended
        && up.close_conn(conn, r)
    {
        let _ = up.tx.try_send(UpOut::Close(conn, r));
    }
    false
}

/// base64url without padding.
pub fn base64url(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}
