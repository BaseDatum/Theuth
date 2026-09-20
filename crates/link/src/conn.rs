//! One relay conn: responder handshake (IK, falling back to IKpsk2 while a
//! pairing is pending), then Noise transport records dispatched by the mode
//! byte in IK message 1 (31 §4.3, §6.2; 32 §4.2): mode 0 carries JSON-RPC
//! into `raw_json_request`, mode 1 hands the records to the ACP agent half
//! (32 §4.3).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use jsonrpsee::core::JsonRawValue;
use serde::{Deserialize, Serialize};
use snow::{Builder, TransportState};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::devices::{Row, device_id};
use crate::identity::{Identity, now_ms, params};
use crate::state::Ctx;
use crate::types::{LinkEvent, RpcPeer};
use crate::uplink::{self, CLOSE_NORMAL, CLOSE_REFUSED};

/// Noise message ceiling.
pub const MAX_MSG: usize = 65_535;
/// Plaintext bytes per record after the flags byte (65 535 − 16 tag − 1 flags).
pub const MAX_CHUNK: usize = 65_518;
/// Handshake (through the first transport record) deadline (31 §4.5).
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// Subscription notification buffer per request (31 §6.2).
const SUB_BUF: usize = 4096;

/// IKpsk2 msg1 payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairHello {
    /// Device display name.
    pub name: String,
    /// `ios` | `macos` | `web` | `theuth` | …
    pub platform: String,
}

/// IKpsk2 msg2 payload.
#[derive(Serialize)]
struct PairAccept<'a> {
    name: &'a str,
}

/// IK msg2 payload.
#[derive(Serialize)]
struct SessionHello<'a> {
    name: &'a str,
    version: &'static str,
}

/// Both halves of a mode-1 conn as the ACP agent sees them (32 §4.3). One
/// reassembled JSON-RPC message per item, no trailing newline, UTF-8 bytes.
/// Dropping the conn closes both; the ACP future ends on either.
pub struct RecordChannel {
    /// Device → harness.
    pub inbound: mpsc::Receiver<Vec<u8>>,
    /// Harness → device.
    pub outbound: mpsc::Sender<Vec<u8>>,
}

/// Outcome of reading msg1.
pub enum Handshake {
    /// Known device: continue as a session.
    Session {
        /// Transport state.
        transport: TransportState,
        /// Device static.
        key: [u8; 32],
        /// msg2 to send.
        msg2: Vec<u8>,
        /// Conn mode from the msg1 payload (32 §4.2); empty payload → 0.
        mode: u8,
    },
    /// Pairing: the row is written after the first record decrypts.
    Pair {
        /// Transport state.
        transport: TransportState,
        /// Device static.
        key: [u8; 32],
        /// What the device said.
        hello: PairHello,
        /// msg2 to send.
        msg2: Vec<u8>,
    },
}

fn static_of(hs: &snow::HandshakeState) -> Option<[u8; 32]> {
    hs.get_remote_static()?.try_into().ok()
}

/// Read `msg1` as IK; if it authenticates a known static → session (payload
/// `[]` or `[mode]`; anything longer is refused); else, with `psk`, re-read it
/// as IKpsk2 → pairing; else `None` (CLOSE 3).
pub fn respond(
    msg1: &[u8],
    identity: &Identity,
    name: &str,
    known: impl Fn(&[u8; 32]) -> bool,
    psk: Option<&[u8; 32]>,
) -> Option<Handshake> {
    let prologue = identity.prologue();
    let mut payload = vec![0u8; MAX_MSG];
    let mut msg2 = vec![0u8; MAX_MSG];

    if let Ok(mut hs) = Builder::new(params("IK").ok()?)
        .local_private_key(&identity.private)
        .ok()?
        .prologue(&prologue)
        .ok()?
        .build_responder()
        && let Ok(n) = hs.read_message(msg1, &mut payload)
        && let Some(key) = static_of(&hs)
        && known(&key)
    {
        let mode = match &payload[..n] {
            [] => 0,
            [mode] => *mode,
            _ => return None,
        };
        let hello = serde_json::to_vec(&SessionHello {
            name,
            version: env!("CARGO_PKG_VERSION"),
        })
        .ok()?;
        let n = hs.write_message(&hello, &mut msg2).ok()?;
        msg2.truncate(n);
        return Some(Handshake::Session {
            transport: hs.into_transport_mode().ok()?,
            key,
            msg2,
            mode,
        });
    }

    let psk = psk?;
    let mut hs = Builder::new(params("IKpsk2").ok()?)
        .local_private_key(&identity.private)
        .ok()?
        .prologue(&prologue)
        .ok()?
        .psk(2, psk)
        .ok()?
        .build_responder()
        .ok()?;
    let n = hs.read_message(msg1, &mut payload).ok()?;
    let hello: PairHello = serde_json::from_slice(&payload[..n]).ok()?;
    let key = static_of(&hs)?;
    let accept = serde_json::to_vec(&PairAccept { name }).ok()?;
    let n = hs.write_message(&accept, &mut msg2).ok()?;
    msg2.truncate(n);
    Some(Handshake::Pair {
        transport: hs.into_transport_mode().ok()?,
        key,
        hello,
        msg2,
    })
}

/// Split one JSON-RPC message into plaintext records (`flags ‖ chunk`, FIN on the last).
pub fn split(text: &[u8]) -> impl Iterator<Item = Vec<u8>> + '_ {
    let chunks = text.len().div_ceil(MAX_CHUNK).max(1);
    (0..chunks).map(move |i| {
        let start = i * MAX_CHUNK;
        let end = (start + MAX_CHUNK).min(text.len());
        let mut rec = Vec::with_capacity(1 + end - start);
        rec.push(u8::from(i + 1 == chunks));
        rec.extend_from_slice(&text[start..end]);
        rec
    })
}

/// Joins records until FIN.
#[derive(Default)]
pub struct Reassembler(Vec<u8>);

impl Reassembler {
    /// Feed one decrypted record; `Some` when it carried FIN.
    pub fn push(&mut self, plain: &[u8]) -> Option<Vec<u8>> {
        let (flags, body) = plain.split_first()?;
        self.0.extend_from_slice(body);
        ((flags & 1) != 0).then(|| std::mem::take(&mut self.0))
    }
}

/// A conn task's inputs.
pub struct Conn {
    /// Shared state.
    pub ctx: Arc<Ctx>,
    /// Relay conn id.
    pub id: u32,
    /// `DATA` payloads from the relay.
    pub inbound: mpsc::Receiver<Bytes>,
    /// Frames to the relay (the uplink writer owns the sink).
    pub out: mpsc::Sender<Message>,
    /// Revoke / uplink teardown.
    pub cancel: CancellationToken,
}

impl Conn {
    async fn send(&self, payload: Vec<u8>) -> bool {
        self.out
            .send(uplink::frame(uplink::OP_DATA, self.id, &payload))
            .await
            .is_ok()
    }

    async fn close(&self, reason: u8) {
        let _ = self
            .out
            .send(uplink::frame(uplink::OP_CLOSE, self.id, &[reason]))
            .await;
    }

    /// Encrypt one JSON-RPC message as records and send them as `DATA`.
    async fn write_records(
        &self,
        transport: &mut TransportState,
        text: &[u8],
        buf: &mut [u8],
    ) -> bool {
        for rec in split(text) {
            let Ok(n) = transport.write_message(&rec, buf) else {
                return false;
            };
            if !self.send(buf[..n].to_vec()).await {
                return false;
            }
        }
        true
    }
}

/// Run the conn to completion. Publishes `LinkDevice`/`LinkPaired`/`LinkPairing`;
/// persists `last_seen_ms` on close.
pub async fn run(mut c: Conn) {
    let deadline = Instant::now() + HANDSHAKE_DEADLINE;
    let msg1 = tokio::select! {
        _ = c.cancel.cancelled() => return c.close(CLOSE_NORMAL).await,
        m = tokio::time::timeout_at(deadline, c.inbound.recv()) => match m {
            Ok(Some(b)) => b,
            Ok(None) => return,
            Err(_) => return c.close(CLOSE_REFUSED).await,
        },
    };

    let (hs, psk) = {
        let st = c.ctx.lock();
        let psk = st
            .live_pairing()
            .filter(|_| st.devices.len() < st.cfg.max_devices)
            .map(|p| p.secret);
        let hs = st.identity.as_ref().and_then(|identity| {
            respond(
                &msg1,
                identity,
                st.name(),
                |k| st.devices.by_key(k).is_some(),
                psk.as_ref(),
            )
        });
        (hs, psk)
    };
    let (mut transport, key, pending, msg2, mode) = match hs {
        Some(Handshake::Session {
            transport,
            key,
            msg2,
            mode,
        }) => (transport, key, None, msg2, mode),
        Some(Handshake::Pair {
            transport,
            key,
            hello,
            msg2,
        }) => (transport, key, Some(hello), msg2, 0),
        None => return c.close(CLOSE_REFUSED).await,
    };
    if !c.send(msg2).await {
        return;
    }
    if pending.is_none() {
        mark_online(&c, &key).await;
    }

    let session = match mode {
        0 => run_api(&mut c, &mut transport, &key, pending, psk, deadline).await,
        1 => {
            run_acp(&mut c, &mut transport, deadline).await;
            true
        }
        _ => {
            c.close(CLOSE_REFUSED).await;
            true
        }
    };

    let (was_session, still_online) = {
        let mut st = c.ctx.lock();
        st.conns.remove(&c.id);
        if session && let Some(row) = st.devices.by_key_mut(&key) {
            row.last_seen_ms = now_ms();
            if let Err(e) = st.devices.save() {
                tracing::warn!("devices.yaml: {e}");
            }
            (true, st.online(&key))
        } else {
            (false, false)
        }
    };
    if was_session {
        c.ctx
            .rt
            .publish(LinkEvent::LinkDevice {
                id: device_id(&key),
                online: still_online,
            })
            .await;
    }
}

/// Mode 0 (31 §6.2): records are JSON-RPC requests into `raw_json_request`
/// on this conn's own clone of the dispatch `Methods`, whose extensions carry
/// the conn's [`RpcPeer`] (closed when this returns); a pairing conn enrols
/// the device on its first record. Waiting for `SetDispatch` counts against
/// the handshake deadline. Returns whether the conn became a session.
async fn run_api(
    c: &mut Conn,
    transport: &mut TransportState,
    key: &[u8; 32],
    mut pending: Option<PairHello>,
    psk: Option<[u8; 32]>,
    deadline: Instant,
) -> bool {
    let mut session = pending.is_none();
    let (peer, _peer_guard) = RpcPeer::open_remote();
    let methods = tokio::select! {
        _ = c.cancel.cancelled() => { c.close(CLOSE_NORMAL).await; return session }
        d = tokio::time::timeout_at(deadline, c.ctx.dispatch()) => match d {
            Ok(d) => Arc::new(peer_methods(&d.methods, peer)),
            Err(_) => { c.close(CLOSE_REFUSED).await; return session }
        },
    };
    let (resp_tx, mut resp_rx) = mpsc::channel::<Box<JsonRawValue>>(256);
    let mut asm = Reassembler::default();
    let mut buf = vec![0u8; MAX_MSG];
    let mut first = true;
    let first_deadline = tokio::time::sleep_until(deadline);
    tokio::pin!(first_deadline);

    loop {
        tokio::select! {
            _ = c.cancel.cancelled() => { c.close(CLOSE_NORMAL).await; break }
            _ = &mut first_deadline, if first => { c.close(CLOSE_REFUSED).await; break }
            m = c.inbound.recv() => {
                let Some(bytes) = m else { break };
                let Ok(n) = transport.read_message(&bytes, &mut buf) else {
                    c.close(CLOSE_REFUSED).await;
                    break;
                };
                first = false;
                if let Some(hello) = pending.take() {
                    if !commit_pairing(c, key, hello, psk).await {
                        c.close(CLOSE_REFUSED).await;
                        break;
                    }
                    session = true;
                }
                if let Some(text) = asm.push(&buf[..n]) {
                    tokio::spawn(dispatch(methods.clone(), text, resp_tx.clone()));
                }
            }
            r = resp_rx.recv() => {
                let Some(raw) = r else { break };
                if !c.write_records(transport, raw.get().as_bytes(), &mut buf).await { break }
            }
        }
    }
    session
}

/// Mode 1 (32 §4.3): hand a [`RecordChannel`] to the ACP agent half and pump
/// records both ways until cancel, relay EOF or the ACP future drops either
/// half. Waiting for `SetDispatch` counts against the handshake deadline.
async fn run_acp(c: &mut Conn, transport: &mut TransportState, deadline: Instant) {
    let acp = tokio::select! {
        _ = c.cancel.cancelled() => return c.close(CLOSE_NORMAL).await,
        d = tokio::time::timeout_at(deadline, c.ctx.dispatch()) => match d {
            Ok(d) => d.acp.clone(),
            Err(_) => return c.close(CLOSE_REFUSED).await,
        },
    };
    let (in_tx, inbound) = mpsc::channel::<Vec<u8>>(256);
    let (outbound, mut out_rx) = mpsc::channel::<Vec<u8>>(256);
    tokio::spawn(acp(c.ctx.rt.clone(), RecordChannel { inbound, outbound }));

    let mut asm = Reassembler::default();
    let mut buf = vec![0u8; MAX_MSG];
    let mut first = true;
    let first_deadline = tokio::time::sleep_until(deadline);
    tokio::pin!(first_deadline);

    loop {
        tokio::select! {
            _ = c.cancel.cancelled() => { c.close(CLOSE_NORMAL).await; break }
            _ = &mut first_deadline, if first => { c.close(CLOSE_REFUSED).await; break }
            m = c.inbound.recv() => {
                let Some(bytes) = m else { break };
                let Ok(n) = transport.read_message(&bytes, &mut buf) else {
                    c.close(CLOSE_REFUSED).await;
                    break;
                };
                first = false;
                if let Some(rec) = asm.push(&buf[..n]) && in_tx.send(rec).await.is_err() {
                    c.close(CLOSE_NORMAL).await;
                    break;
                }
            }
            r = out_rx.recv() => {
                let Some(text) = r else { c.close(CLOSE_NORMAL).await; break };
                if !c.write_records(transport, &text, &mut buf).await { break }
            }
        }
    }
}

async fn mark_online(c: &Conn, key: &[u8; 32]) {
    {
        let mut st = c.ctx.lock();
        if let Some(h) = st.conns.get_mut(&c.id) {
            h.device = Some(*key);
        }
    }
    c.ctx
        .rt
        .publish(LinkEvent::LinkDevice {
            id: device_id(key),
            online: true,
        })
        .await;
}

/// First transport record decrypted under the PSK: enrol the device, consume
/// the pairing. `false` when the pairing this handshake used is gone
/// (cancelled, expired or replaced).
async fn commit_pairing(c: &Conn, key: &[u8; 32], hello: PairHello, psk: Option<[u8; 32]>) -> bool {
    let row = {
        let mut st = c.ctx.lock();
        let same = st.live_pairing().is_some_and(|p| Some(p.secret) == psk);
        if !same || st.devices.len() >= st.cfg.max_devices {
            return false;
        }
        st.pairing = None;
        let row = Row {
            id: device_id(key),
            key: *key,
            name: hello.name,
            platform: hello.platform,
            paired_at_ms: now_ms(),
            last_seen_ms: 0,
        };
        if let Err(e) = st.devices.insert(row.clone()) {
            tracing::warn!("devices.yaml: {e}");
        }
        row
    };
    // `LinkPaired` first: a consumer that treats `LinkPairing { key: None }`
    // as "expired" (the CLI, the TUI card) must already know it succeeded.
    c.ctx
        .rt
        .publish(LinkEvent::LinkPaired {
            device: row.device(true),
        })
        .await;
    c.ctx
        .rt
        .publish(LinkEvent::LinkPairing {
            key: None,
            expires_at_ms: None,
        })
        .await;
    mark_online(c, key).await;
    true
}

/// `methods` with `peer` in its extensions: what one conn's requests dispatch into.
fn peer_methods(methods: &jsonrpsee::Methods, peer: RpcPeer) -> jsonrpsee::Methods {
    let mut m = methods.clone();
    m.extensions_mut().insert(peer);
    m
}

/// One request: response first, then every subscription notification for as
/// long as the conn (holder of `resp_rx`) lives.
async fn dispatch(
    methods: Arc<jsonrpsee::Methods>,
    text: Vec<u8>,
    tx: mpsc::Sender<Box<JsonRawValue>>,
) {
    let Ok(text) = String::from_utf8(text) else {
        let _ = tx.send(parse_error()).await;
        return;
    };
    match methods.raw_json_request(&text, SUB_BUF).await {
        Ok((resp, mut rx)) => {
            if tx.send(resp).await.is_err() {
                return;
            }
            while let Some(n) = rx.recv().await {
                if tx.send(n).await.is_err() {
                    return;
                }
            }
        }
        Err(_) => {
            let _ = tx.send(parse_error()).await;
        }
    }
}

fn parse_error() -> Box<JsonRawValue> {
    JsonRawValue::from_string(
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#.to_owned(),
    )
    .expect("static JSON is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> ([u8; 32], [u8; 32]) {
        let kp = Builder::new(params("IK").unwrap())
            .generate_keypair()
            .unwrap();
        let private: [u8; 32] = kp.private.as_slice().try_into().unwrap();
        (private, crate::identity::public_of(&private))
    }

    fn initiator(
        pattern: &str,
        dev: &[u8; 32],
        h: &Identity,
        psk: Option<&[u8; 32]>,
    ) -> snow::HandshakeState {
        let prologue = h.prologue();
        let mut b = Builder::new(params(pattern).unwrap())
            .local_private_key(dev)
            .unwrap()
            .remote_public_key(&h.public)
            .unwrap()
            .prologue(&prologue)
            .unwrap();
        if let Some(psk) = psk {
            b = b.psk(2, psk).unwrap();
        }
        b.build_initiator().unwrap()
    }

    #[test]
    fn split_reassemble_round_trip() {
        for len in [
            0usize,
            1,
            MAX_CHUNK - 1,
            MAX_CHUNK,
            MAX_CHUNK + 1,
            3 * MAX_CHUNK + 7,
        ] {
            let text: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let recs: Vec<Vec<u8>> = split(&text).collect();
            assert_eq!(recs.len(), len.div_ceil(MAX_CHUNK).max(1));
            assert!(recs.iter().all(|r| r.len() <= 1 + MAX_CHUNK));
            let mut asm = Reassembler::default();
            let mut out = None;
            for (i, r) in recs.iter().enumerate() {
                let got = asm.push(r);
                assert_eq!(got.is_some(), i + 1 == recs.len());
                out = got;
            }
            assert_eq!(out.unwrap(), text);
        }
    }

    /// `null` when the call carries no peer.
    async fn peer_id(m: &jsonrpsee::Methods) -> Option<u64> {
        let (resp, _) = m
            .raw_json_request(r#"{"jsonrpc":"2.0","id":1,"method":"peer"}"#, 1)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        v["result"].as_u64()
    }

    #[tokio::test]
    async fn peer_is_per_conn_clone() {
        let mut module = jsonrpsee::RpcModule::new(());
        module
            .register_method("peer", |_, _, ext| ext.get::<RpcPeer>().map(RpcPeer::id))
            .unwrap();
        let base: jsonrpsee::Methods = module.into();
        // No conn → no peer; nothing authenticates a bare dispatch.
        assert_eq!(peer_id(&base).await, None);

        let (a, guard_a) = RpcPeer::open();
        let (b, guard_b) = RpcPeer::open();
        assert_ne!(a.id(), b.id());
        let ma = peer_methods(&base, a.clone());
        let mb = peer_methods(&base, b.clone());
        // Every request on a conn sees that conn's peer; another conn sees its own.
        assert_eq!(peer_id(&ma).await, Some(a.id()));
        assert_eq!(peer_id(&ma).await, Some(a.id()));
        assert_eq!(peer_id(&mb).await, Some(b.id()));
        assert_eq!(peer_id(&base).await, None);

        // Closing one conn closes only its peer, even via clones taken earlier.
        let closed_a = a.closed();
        drop(guard_a);
        closed_a.await;
        assert!(a.is_closed());
        assert!(!b.is_closed());
        drop(guard_b);
        assert!(b.is_closed());
    }

    #[test]
    fn ik_then_ikpsk2_fallback() {
        let (hpriv, _) = keypair();
        let harness = Identity::from_private(hpriv);
        let (dpriv, dpub) = keypair();
        let psk = [9u8; 32];
        let mut msg1 = vec![0u8; MAX_MSG];
        let mut buf = vec![0u8; MAX_MSG];

        // Unknown device, IK, no pairing → refused.
        let mut dev = initiator("IK", &dpriv, &harness, None);
        let n = dev.write_message(&[], &mut msg1).unwrap();
        assert!(respond(&msg1[..n], &harness, "h", |_| false, None).is_none());

        // Unknown device, IKpsk2 msg1, pairing pending → Pair; IK read fails cleanly first.
        let mut dev = initiator("IKpsk2", &dpriv, &harness, Some(&psk));
        let hello = br#"{"name":"iPhone","platform":"ios"}"#;
        let n = dev.write_message(hello, &mut msg1).unwrap();
        // psk2 is mixed after `se`: msg1 reads under any secret, so a wrong
        // secret only shows when the device cannot read msg2 (and its first
        // record would not decrypt here — nothing is enrolled before that).
        let Some(Handshake::Pair { msg2: bad_msg2, .. }) =
            respond(&msg1[..n], &harness, "h", |_| false, Some(&[0u8; 32]))
        else {
            panic!("msg1 must read under any psk");
        };
        assert!(dev.read_message(&bad_msg2, &mut buf).is_err());
        let Some(Handshake::Pair {
            mut transport,
            key,
            hello,
            msg2,
        }) = respond(&msg1[..n], &harness, "Sky", |_| false, Some(&psk))
        else {
            panic!("expected pairing");
        };
        assert_eq!(key, dpub);
        assert_eq!(
            (hello.name.as_str(), hello.platform.as_str()),
            ("iPhone", "ios")
        );
        let n = dev.read_message(&msg2, &mut buf).unwrap();
        assert_eq!(&buf[..n], br#"{"name":"Sky"}"#);
        let mut dev_t = dev.into_transport_mode().unwrap();
        // First transport record both ways.
        let n = dev_t.write_message(&[1, b'{', b'}'], &mut buf).unwrap();
        let enc = buf[..n].to_vec();
        let n = transport.read_message(&enc, &mut buf).unwrap();
        assert_eq!(&buf[..n], &[1, b'{', b'}']);

        // Known device, IK → Session with SessionHello.
        let mut dev = initiator("IK", &dpriv, &harness, None);
        let n = dev.write_message(&[], &mut msg1).unwrap();
        let Some(Handshake::Session {
            mut transport,
            key,
            msg2,
            mode,
        }) = respond(&msg1[..n], &harness, "Sky", |k| k == &dpub, Some(&psk))
        else {
            panic!("expected session");
        };
        assert_eq!((key, mode), (dpub, 0));
        let n = dev.read_message(&msg2, &mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
        assert_eq!(v["name"], "Sky");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        let mut dev_t = dev.into_transport_mode().unwrap();
        let n = transport.write_message(b"\x01hi", &mut buf).unwrap();
        let enc = buf[..n].to_vec();
        let n = dev_t.read_message(&enc, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"\x01hi");

        // Known device but IK msg1 from a different (unpaired) key with a pairing pending
        // → falls through to IKpsk2, which fails (no psk mixed) → refused.
        let (opriv, _) = keypair();
        let mut other = initiator("IK", &opriv, &harness, None);
        let n = other.write_message(&[], &mut msg1).unwrap();
        assert!(respond(&msg1[..n], &harness, "h", |k| k == &dpub, Some(&psk)).is_none());
    }
}
