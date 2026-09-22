//! The relay uplink (31 §4.2, §6.2): one WebSocket to `<relay>/h/<id>?role=harness`,
//! `NK` registration, then the single reader loop demuxing frames to conn tasks.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use snow::Builder;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::sync::CancellationToken;

use crate::conn;
use crate::identity::{PROTOCOL, params};
use crate::state::{ConnHandle, Ctx};
use crate::types::LinkEvent;

/// `HELLO` (conn 0): NK registration.
pub const OP_HELLO: u8 = 0x00;
/// `OPEN` relay→harness: new conn.
pub const OP_OPEN: u8 = 0x01;
/// `DATA` both ways.
pub const OP_DATA: u8 = 0x02;
/// `CLOSE` both ways, payload `reason:u8`.
pub const OP_CLOSE: u8 = 0x03;
/// `PRESENCE` relay→device (31 §4.2): payload `online:u8`, on connect and every change.
pub const OP_PRESENCE: u8 = 0x04;

/// CLOSE reason: normal.
pub const CLOSE_NORMAL: u8 = 0;
/// CLOSE reason: per-conn queue full.
pub const CLOSE_QUEUE_FULL: u8 = 1;
/// CLOSE reason: refused (handshake/auth failure, over limit).
pub const CLOSE_REFUSED: u8 = 3;

/// Uplink outbound queue (31 §4.5).
const OUT_QUEUE: usize = 4096;
/// Per-conn inbound queue (31 §4.5).
const CONN_QUEUE: usize = 256;
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Harness-side keepalive: a WebSocket `Ping` this often. The relay pings us too (30 s), but a
/// relay replaced behind its proxy leaves our socket ESTABLISHED and silent forever — we must
/// notice on our own or the harness reports "connected" while devices see it offline.
const PING_EVERY: Duration = Duration::from_secs(20);
/// No frame at all within this long after a `Ping` → the uplink is dead; `supervise` redials.
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// `op ‖ conn:u32 LE ‖ payload` as a binary frame.
pub fn frame(op: u8, conn: u32, payload: &[u8]) -> Message {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.push(op);
    v.extend_from_slice(&conn.to_le_bytes());
    v.extend_from_slice(payload);
    Message::Binary(v.into())
}

/// Inverse of [`frame`].
pub fn parse(b: &Bytes) -> Option<(u8, u32, Bytes)> {
    if b.len() < 5 {
        return None;
    }
    let conn = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
    Some((b[0], conn, b.slice(5..)))
}

/// `op ‖ payload` as a binary frame on a *device* socket (`/h/<id>` without
/// `role=harness`, 31 §4.2). What the `nodes` plugin speaks as a device.
pub fn device_frame(op: u8, payload: &[u8]) -> Message {
    let mut v = Vec::with_capacity(1 + payload.len());
    v.push(op);
    v.extend_from_slice(payload);
    Message::Binary(v.into())
}

/// Inverse of [`device_frame`]; `None` when empty.
pub fn parse_device(b: &Bytes) -> Option<(u8, Bytes)> {
    let op = *b.first()?;
    Some((op, b.slice(1..)))
}

/// Dial, register and serve until `cancel` or the socket drops; then back off
/// and repeat. Publishes `LinkRelay` on every transition.
pub async fn supervise(ctx: Arc<Ctx>, cancel: CancellationToken) {
    let mut backoff = BACKOFF_MIN;
    while !cancel.is_cancelled() {
        let started = tokio::time::Instant::now();
        match run_once(&ctx, &cancel).await {
            Ok(()) => {}
            Err(e) => tracing::info!("link uplink: {e:#}"),
        }
        set_connected(&ctx, false).await;
        if cancel.is_cancelled() {
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

async fn set_connected(ctx: &Ctx, connected: bool) {
    let changed = {
        let mut st = ctx.lock();
        let changed = st.connected != connected;
        st.connected = connected;
        if !connected {
            // Every conn belongs to the dead socket; their tasks end when the
            // inbound senders drop here.
            for c in st.conns.values() {
                c.cancel.cancel();
            }
            st.conns.clear();
        }
        changed
    };
    if changed {
        ctx.rt.publish(LinkEvent::LinkRelay { connected }).await;
    }
}

async fn run_once(ctx: &Arc<Ctx>, cancel: &CancellationToken) -> anyhow::Result<()> {
    let (url, identity) = {
        let st = ctx.lock();
        let identity = st
            .identity
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no identity"))?;
        (
            format!(
                "{}/h/{}?role=harness",
                st.cfg.relay.trim_end_matches('/'),
                identity.id
            ),
            identity,
        )
    };
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", PROTOCOL.parse()?);
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

    // HELLO: NK msg1 from the relay, msg2 from us.
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
    set_connected(ctx, true).await;

    // The only `next()` caller: demux by conn. Any inbound frame counts as life; a `Ping` we
    // sent that nothing answers within `PONG_TIMEOUT` ends the socket.
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
                let token = cancel.child_token();
                ctx.lock().conns.insert(
                    id,
                    ConnHandle {
                        device: None,
                        inbound: tx,
                        cancel: token.clone(),
                    },
                );
                tokio::spawn(conn::run(conn::Conn {
                    ctx: ctx.clone(),
                    id,
                    inbound: rx,
                    out: out.clone(),
                    cancel: token,
                }));
            }
            OP_DATA => {
                let full = match ctx.lock().conns.get(&id) {
                    Some(c) => c.inbound.try_send(payload).is_err(),
                    None => continue,
                };
                if full {
                    ctx.lock().conns.remove(&id);
                    let _ = out.send(frame(OP_CLOSE, id, &[CLOSE_QUEUE_FULL])).await;
                }
            }
            OP_CLOSE => {
                // Dropping the sender ends the conn task without a CLOSE echo.
                ctx.lock().conns.remove(&id);
            }
            other => break Err(anyhow::anyhow!("unknown op {other}")),
        }
    };
    drop(out);
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let Message::Binary(b) = frame(OP_DATA, 0x0102_0304, b"xyz") else {
            panic!("binary")
        };
        assert_eq!(&b[..5], &[2, 4, 3, 2, 1]);
        let (op, conn, payload) = parse(&b).unwrap();
        assert_eq!(
            (op, conn, payload.as_ref()),
            (OP_DATA, 0x0102_0304, &b"xyz"[..])
        );
        assert!(parse(&Bytes::from_static(&[1, 2])).is_none());
    }

    #[test]
    fn device_frame_round_trip() {
        let Message::Binary(b) = device_frame(OP_PRESENCE, &[1]) else {
            panic!("binary")
        };
        assert_eq!(b.as_ref(), &[OP_PRESENCE, 1]);
        let (op, payload) = parse_device(&b).unwrap();
        assert_eq!((op, payload.as_ref()), (OP_PRESENCE, &[1u8][..]));
        let (op, payload) = parse_device(&Bytes::from_static(&[OP_CLOSE])).unwrap();
        assert!(op == OP_CLOSE && payload.is_empty());
        assert!(parse_device(&Bytes::new()).is_none());
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    use crate::identity::Identity;
    use crate::state::State;
    use crate::types::LinkConfig;

    /// A relay that completes `NK` and then holds the socket open without ever polling it — a
    /// replaced backend behind a proxy as the harness experiences it: ESTABLISHED, silent, no pongs
    /// (an unpolled tungstenite server never auto-answers a `Ping`).
    async fn silent_relay(harness: &Identity) -> (String, tokio::sync::oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let harness_pub = harness.public;
        let prologue = harness.prologue();
        let (registered, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(
                tcp,
                |_req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 mut res: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    res.headers_mut()
                        .insert("Sec-WebSocket-Protocol", PROTOCOL.parse().unwrap());
                    Ok(res)
                },
            )
            .await
            .unwrap();
            let mut hs = Builder::new(params("NK").unwrap())
                .remote_public_key(&harness_pub)
                .unwrap()
                .prologue(&prologue)
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
            registered.send(()).unwrap();
            // Registered — and gone. Never poll `ws` again.
            std::future::pending::<()>().await;
            drop(ws);
        });
        (format!("ws://{addr}"), rx)
    }

    /// A dead uplink must end `run_once` on its own (so `supervise` redials) instead of reporting
    /// "connected" forever while every device sees the harness offline.
    #[tokio::test]
    async fn a_silent_relay_ends_the_uplink() {
        let kp = Builder::new(params("NK").unwrap()).generate_keypair().unwrap();
        let identity = Identity::from_private(kp.private.as_slice().try_into().unwrap());
        let (relay, registered) = silent_relay(&identity).await;
        let dir = std::env::temp_dir().join(format!("theuth-link-uplink-{}", rand::random::<u64>()));
        let ctx = Ctx::new(
            runtime::Runtime::new(Default::default(), dir.clone()),
            State {
                cfg: LinkConfig {
                    relay,
                    ..LinkConfig::default()
                },
                hostname: "test".into(),
                identity: Some(identity),
                devices: crate::devices::Devices::load(&dir),
                pairing: None,
                connected: false,
                conns: Default::default(),
                guests: Default::default(),
            },
        );
        let cancel = CancellationToken::new();
        let uplink = tokio::spawn({
            let ctx = ctx.clone();
            let cancel = cancel.clone();
            async move { run_once(&ctx, &cancel).await }
        });
        registered.await.unwrap();
        assert!(ctx.lock().connected, "registered with the relay");
        // Real time for the dial and NK; from here the clock only moves when everything is idle,
        // so the ping and pong timers fire without a 30 s wall-clock wait.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(PING_EVERY + PONG_TIMEOUT + Duration::from_secs(5), uplink)
            .await
            .expect("run_once returned")
            .unwrap();
        let err = result.expect_err("a silent relay is an error, not a clean close");
        assert!(err.to_string().contains("silent"), "{err}");
        assert!(started.elapsed() >= PING_EVERY + PONG_TIMEOUT, "{:?}", started.elapsed());
    }
}
