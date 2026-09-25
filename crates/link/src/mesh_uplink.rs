//! The MESH identity's uplink (69 §2): a second relay registration at
//! `/h/<mesh device id>` that only the hub runs. [`crate::follow`] starts it
//! on lease gain and stops it on loss; the loop here mirrors
//! [`crate::uplink`] for the mesh identity, its conns run by
//! [`conn::run_mesh`] so they handshake against the replicated rows.
//!
//! Lease semantics (69 decision 2, recon constraint): on lease loss the
//! reconcile calls [`MeshUplink::stop`] — the token is cancelled **and**
//! `serve` flips to `false`, and [`supervise`] does NOT redial while `serve`
//! is false. The relay's `register` replaces the previous holder, so a loser
//! that kept redialing would steal the registration back from the new hub
//! and the two would flap. Device conns are closed with
//! [`crate::uplink::CLOSE_HUB_MOVED`] on such a graceful handover; on a
//! socket loss the relay tells them `HARNESS_GONE` itself.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use snow::Builder;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::sync::CancellationToken;

use crate::conn;
use crate::identity::{Identity, PROTOCOL, params};
use crate::state::{ConnHandle, Ctx};
use crate::types::LinkEvent;
use crate::uplink::{
    CLOSE_HUB_MOVED, CLOSE_QUEUE_FULL, OP_CLOSE, OP_DATA, OP_HELLO, OP_OPEN, frame, parse,
};

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A registration that lived this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Uplink outbound queue (31 §4.5).
const OUT_QUEUE: usize = 4096;
/// Per-conn inbound queue (31 §4.5).
const CONN_QUEUE: usize = 256;
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// Harness-side keepalive, as the engine uplink's: a silent relay must be noticed.
const PING_EVERY: Duration = Duration::from_secs(20);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// A running mesh uplink: its token and the serve gate.
pub struct MeshUplink {
    cancel: CancellationToken,
    serve: watch::Sender<bool>,
}

impl MeshUplink {
    /// Spawn [`supervise`] for `identity` (the vault's `mesh/device_key`) at
    /// `relay`; `token` is the caller's (a child of the plugin's shutdown).
    pub fn start(ctx: Arc<Ctx>, identity: Identity, relay: String, token: CancellationToken) -> Self {
        let (serve, rx) = watch::channel(true);
        tokio::spawn(supervise(ctx, identity, relay, token.clone(), rx));
        Self { cancel: token, serve }
    }

    /// Lease lost or shutdown: stop serving and never redial. The supervisor
    /// closes device conns with `CLOSE_HUB_MOVED` and ends.
    pub fn stop(&self) {
        let _ = self.serve.send(false);
        self.cancel.cancel();
    }

    /// Still meant to serve (the gate, for tests and status).
    pub fn serving(&self) -> bool {
        *self.serve.borrow()
    }
}

/// Dial, register `identity` at `<relay>/h/<identity.id>?role=harness` and
/// serve device conns until `cancel` or the socket drops; then back off and
/// redial — **only while `serve` is true**.
pub async fn supervise(
    ctx: Arc<Ctx>,
    identity: Identity,
    relay: String,
    cancel: CancellationToken,
    serve: watch::Receiver<bool>,
) {
    let mut backoff = BACKOFF_MIN;
    while !cancel.is_cancelled() && *serve.borrow() {
        let started = tokio::time::Instant::now();
        match run_once(&ctx, &identity, &relay, &cancel, &serve).await {
            Ok(()) => {}
            Err(e) => tracing::info!("mesh uplink: {e:#}"),
        }
        set_connected(&ctx, false, None).await;
        if cancel.is_cancelled() || !*serve.borrow() {
            break;
        }
        if started.elapsed() >= STABLE_AFTER {
            backoff = BACKOFF_MIN;
        }
        let jitter = backoff.mul_f64(rand::random_range(0.8..1.2));
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(jitter) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// `State::mesh.connected` + `MeshRelay` on change. Going down drops every
/// mesh conn: `hub_moved` tells each device `CLOSE 5` through `out` first (a
/// graceful handover, 69 decision 6); otherwise the dead socket already cost
/// them and the relay says `HARNESS_GONE`.
async fn set_connected(ctx: &Ctx, connected: bool, hub_moved: Option<&mpsc::Sender<Message>>) {
    let (changed, moved) = {
        let mut st = ctx.lock();
        let changed = st.mesh.as_ref().is_some_and(|m| m.connected != connected);
        if let Some(m) = st.mesh.as_mut() {
            m.connected = connected;
        }
        let mut moved = Vec::new();
        if !connected {
            // Dropping the inbound senders ends the conn tasks without a CLOSE
            // echo (the same path as a relay CLOSE); a session close still
            // writes `last_seen_ms`.
            if hub_moved.is_some() {
                moved.extend(st.mesh_conns.keys().copied());
            }
            st.mesh_conns.clear();
        }
        (changed, moved)
    };
    if let Some(out) = hub_moved {
        for id in moved {
            let _ = out.send(frame(OP_CLOSE, id, &[CLOSE_HUB_MOVED])).await;
        }
    }
    if changed {
        ctx.rt.publish(LinkEvent::MeshRelay { connected }).await;
    }
}

/// One registration lifetime: [`crate::uplink::run_once`] against the mesh
/// identity. Device conns run as [`conn::run_mesh`] (the
/// replicated `devices`/`pending` rows). Each conn gets its own token — not a
/// child of `cancel` — so a handover can tell the device `CLOSE 5` before its
/// task ends instead of racing it for `CLOSE 0`.
async fn run_once(
    ctx: &Arc<Ctx>,
    identity: &Identity,
    relay: &str,
    cancel: &CancellationToken,
    serve: &watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let url = format!("{}/h/{}?role=harness", relay.trim_end_matches('/'), identity.id);
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut().insert("Sec-WebSocket-Protocol", PROTOCOL.parse()?);
    let (ws, _) = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        r = tokio::time::timeout(DIAL_TIMEOUT, tokio_tungstenite::connect_async(req)) => r??,
    };
    let (mut sink, mut stream) = ws.split();
    let (out, mut out_rx) = mpsc::channel::<Message>(OUT_QUEUE);
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // HELLO: NK msg1 from the relay, msg2 from us (the relay proves we hold the mesh static).
    let hello = tokio::select! {
        _ = cancel.cancelled() => None,
        m = tokio::time::timeout(DIAL_TIMEOUT, stream.next()) => m.ok().flatten().and_then(Result::ok),
    };
    let Some(Message::Binary(b)) = hello else {
        writer.abort();
        anyhow::bail!("no HELLO");
    };
    let Some((OP_HELLO, 0, msg1)) = parse(&b) else {
        writer.abort();
        anyhow::bail!("expected HELLO, got {:?}", b.first());
    };
    let prologue = identity.prologue();
    let mut hs = Builder::new(params("NK")?)
        .local_private_key(&identity.private)?
        .prologue(&prologue)?
        .build_responder()?;
    let mut buf = vec![0u8; conn::MAX_MSG];
    hs.read_message(&msg1, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    if out.send(frame(OP_HELLO, 0, &buf[..n])).await.is_err() {
        anyhow::bail!("writer gone");
    }
    tracing::info!(id = %identity.id, "mesh uplink registered: serving paired devices");
    set_connected(ctx, true, None).await;

    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let pong = tokio::time::sleep(PONG_TIMEOUT);
    tokio::pin!(pong);
    let mut awaiting_pong = false;
    let result = loop {
        let msg = tokio::select! {
            _ = cancel.cancelled() => break Ok(()),
            m = stream.next() => m,
            _ = ping.tick() => {
                if out.send(Message::Ping(Bytes::new())).await.is_err() {
                    break Err(anyhow::anyhow!("writer gone"));
                }
                awaiting_pong = true;
                pong.as_mut().reset(tokio::time::Instant::now() + PONG_TIMEOUT);
                continue;
            }
            _ = &mut pong, if awaiting_pong => {
                break Err(anyhow::anyhow!("relay silent for {PONG_TIMEOUT:?} after a ping; redialing"));
            }
        };
        awaiting_pong = false;
        let b = match msg {
            Some(Ok(Message::Binary(b))) => b,
            Some(Ok(Message::Close(_))) | None => break Ok(()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => break Err(e.into()),
        };
        let Some((op, id, payload)) = parse(&b) else {
            break Err(anyhow::anyhow!("short frame"));
        };
        match op {
            OP_OPEN => {
                let (tx, rx) = mpsc::channel(CONN_QUEUE);
                let token = CancellationToken::new();
                ctx.lock().mesh_conns.insert(
                    id,
                    ConnHandle {
                        device: None,
                        inbound: tx,
                        cancel: token.clone(),
                    },
                );
                tokio::spawn(conn::run_mesh(conn::Conn {
                    ctx: ctx.clone(),
                    id,
                    inbound: rx,
                    out: out.clone(),
                    cancel: token,
                }));
            }
            OP_DATA => {
                let full = match ctx.lock().mesh_conns.get(&id) {
                    Some(c) => c.inbound.try_send(payload).is_err(),
                    None => continue,
                };
                if full {
                    ctx.lock().mesh_conns.remove(&id);
                    let _ = out.send(frame(OP_CLOSE, id, &[CLOSE_QUEUE_FULL])).await;
                }
            }
            OP_CLOSE => {
                ctx.lock().mesh_conns.remove(&id);
            }
            other => break Err(anyhow::anyhow!("unknown op {other}")),
        }
    };
    // A handover (`serve` down) says goodbye to every device before the socket goes.
    let moved = !*serve.borrow();
    set_connected(ctx, false, moved.then_some(&out)).await;
    drop(out);
    let mut writer = writer;
    if !moved || tokio::time::timeout(Duration::from_secs(2), &mut writer).await.is_err() {
        // Nothing to flush — or the CLOSE 5 frames had their chance (the writer
        // ends once every conn task dropped its sender clone).
        writer.abort();
    }
    if !*serve.borrow() {
        tracing::info!(id = %identity.id, "mesh uplink stopped: the hub lease is held elsewhere");
    }
    result
}

/// Test doubles shared by the mesh uplink, `follow` and `conn` tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::state::State;
    use crate::types::{LinkConfig, MeshLink};

    /// A `State` for a test: `relay`, the engine identity, the mesh identity
    /// (with its `MeshLink`), link enabled.
    pub fn state(relay: &str, engine: Option<Identity>, mesh: Option<Identity>) -> State {
        State {
            cfg: LinkConfig {
                enabled: true,
                relay: relay.to_owned(),
                ..LinkConfig::default()
            },
            hostname: "hub".into(),
            identity: engine,
            devices: crate::devices::Devices::empty(),
            pairing: None,
            pending: crate::devices::Pending::new(None, None),
            mesh_identity: mesh.clone(),
            mesh: mesh.map(|m| MeshLink {
                device_id: m.id,
                connected: false,
            }),
            connected: false,
            conns: Default::default(),
            mesh_conns: Default::default(),
            guests: Default::default(),
        }
    }

    /// A `Ctx` over [`state`] with a throwaway runtime.
    pub fn ctx(relay: &str, engine: Option<Identity>, mesh: Option<Identity>) -> Arc<Ctx> {
        let dir = std::env::temp_dir().join(format!("theuth-link-test-{}", rand::random::<u64>()));
        Ctx::new(runtime::Runtime::new(Default::default(), dir), state(relay, engine, mesh))
    }

    /// A fresh X25519 identity.
    pub fn identity() -> Identity {
        let kp = Builder::new(params("NK").unwrap()).generate_keypair().unwrap();
        Identity::from_private(kp.private.as_slice().try_into().unwrap())
    }

    /// Poll `f` until true (5 s).
    pub async fn until(mut f: impl FnMut() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached");
    }

    /// What the fake relay saw, tagged with the harness id of the socket.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Event {
        /// `NK` completed on `/h/<id>?role=harness`.
        Registered(String),
        /// An uplink frame `(id, op, conn, payload)` from the harness.
        Frame(String, u8, u32, Bytes),
        /// The harness socket ended.
        Closed(String),
    }

    /// An in-process relay for the harness side of 31 §4.2: completes `NK`
    /// as the initiator against the identity the path names, then opens conn
    /// 1 at once (a device arrived) and reports every frame. No devices, no
    /// forwarding — the assertions are about what the harness sends.
    pub struct FakeRelay {
        /// `ws://127.0.0.1:<port>`.
        pub url: String,
        events: mpsc::UnboundedReceiver<Event>,
    }

    impl FakeRelay {
        pub async fn start(identities: Vec<Identity>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let (tx, events) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                loop {
                    let Ok((tcp, _)) = listener.accept().await else { return };
                    tokio::spawn(serve(tcp, identities.clone(), tx.clone()));
                }
            });
            Self { url, events }
        }

        /// The next event within 5 s.
        pub async fn next(&mut self) -> Event {
            tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("relay event")
                .expect("relay alive")
        }

        /// Drain until `want` matches (5 s), returning the events skipped.
        pub async fn expect(&mut self, mut want: impl FnMut(&Event) -> bool) -> Vec<Event> {
            let mut skipped = Vec::new();
            loop {
                let e = self.next().await;
                if want(&e) {
                    return skipped;
                }
                skipped.push(e);
            }
        }

        /// Everything that arrives within `d`.
        pub async fn drain(&mut self, d: Duration) -> Vec<Event> {
            let mut out = Vec::new();
            let end = tokio::time::Instant::now() + d;
            while let Ok(Some(e)) = tokio::time::timeout_at(end, self.events.recv()).await {
                out.push(e);
            }
            out
        }
    }

    async fn serve(tcp: tokio::net::TcpStream, identities: Vec<Identity>, tx: mpsc::UnboundedSender<Event>) {
        let mut path = String::new();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            tcp,
            |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
             mut res: tokio_tungstenite::tungstenite::handshake::server::Response| {
                path = req.uri().to_string();
                res.headers_mut().insert("Sec-WebSocket-Protocol", PROTOCOL.parse().unwrap());
                Ok(res)
            },
        )
        .await
        .unwrap();
        let id = path.trim_start_matches("/h/").split('?').next().unwrap_or_default().to_owned();
        let Some(identity) = identities.iter().find(|i| i.id == id) else {
            panic!("unknown harness id {id} on {path}");
        };
        let mut hs = Builder::new(params("NK").unwrap())
            .remote_public_key(&identity.public)
            .unwrap()
            .prologue(&identity.prologue())
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut buf = vec![0u8; conn::MAX_MSG];
        let n = hs.write_message(&[], &mut buf).unwrap();
        ws.send(frame(OP_HELLO, 0, &buf[..n])).await.unwrap();
        let Some(Ok(Message::Binary(b))) = ws.next().await else {
            panic!("expected HELLO msg2");
        };
        let (OP_HELLO, 0, msg2) = parse(&b).unwrap() else {
            panic!("expected HELLO msg2");
        };
        hs.read_message(&msg2, &mut buf).unwrap();
        let _ = tx.send(Event::Registered(id.clone()));
        ws.send(frame(OP_OPEN, 1, &[])).await.unwrap();
        while let Some(Ok(m)) = ws.next().await {
            match m {
                Message::Binary(b) => {
                    let (op, c, p) = parse(&b).unwrap();
                    let _ = tx.send(Event::Frame(id.clone(), op, c, p));
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = tx.send(Event::Closed(id));
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Event, FakeRelay, ctx, identity, until};
    use super::*;

    /// The supervisor is gated: `stop` flips `serve` and cancels, and a
    /// cancelled token alone also ends the loop instead of redialing.
    #[tokio::test(start_paused = true)]
    async fn stop_ends_the_supervisor_without_redial() {
        let (serve, rx) = watch::channel(true);
        let cancel = CancellationToken::new();
        let up = MeshUplink { cancel: cancel.clone(), serve };
        assert!(up.serving());
        up.stop();
        assert!(!up.serving());
        assert!(cancel.is_cancelled());
        assert!(!*rx.borrow());
        // A fresh supervisor with the gate already down returns at once.
        let ctx = ctx("ws://x", None, None);
        let id = Identity::from_private([1u8; 32]);
        tokio::time::timeout(Duration::from_secs(1), supervise(ctx, id, "ws://x".into(), cancel, rx))
            .await
            .expect("no redial once stopped");
    }

    /// A registered mesh uplink with a device conn open: `stop` (the lease
    /// went elsewhere) tells the device `CLOSE 5`, ends the socket, marks
    /// `mesh.connected = false` — and never dials the relay again, so the
    /// new hub's registration is not stolen back (69 decision 2).
    #[tokio::test]
    async fn stop_says_hub_moved_and_never_redials() {
        let mesh = identity();
        let mut relay = FakeRelay::start(vec![mesh.clone()]).await;
        let ctx = ctx(&relay.url, None, Some(mesh.clone()));
        let up = MeshUplink::start(ctx.clone(), mesh.clone(), relay.url.clone(), CancellationToken::new());

        assert_eq!(relay.next().await, Event::Registered(mesh.id.clone()));
        until(|| ctx.lock().mesh.as_ref().is_some_and(|m| m.connected)).await;
        until(|| ctx.lock().mesh_conns.contains_key(&1)).await;

        up.stop();
        let skipped = relay
            .expect(|e| matches!(e, Event::Frame(_, OP_CLOSE, 1, p) if p.as_ref() == [CLOSE_HUB_MOVED]))
            .await;
        assert!(skipped.is_empty(), "CLOSE 5 is the only goodbye: {skipped:?}");
        assert_eq!(relay.next().await, Event::Closed(mesh.id.clone()));
        assert!(ctx.lock().mesh_conns.is_empty());
        assert!(!ctx.lock().mesh.as_ref().unwrap().connected);

        // Backoff is 1 s (×1.2 at most): a loser that redialed would have by now.
        let later = relay.drain(Duration::from_millis(2500)).await;
        assert!(later.is_empty(), "no redial after stop: {later:?}");
    }
}
