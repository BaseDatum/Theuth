//! One relay conn: responder handshake (IK, falling back to IKpsk2 while a
//! pairing is pending), then Noise transport records dispatched by the mode
//! byte in IK message 1 (31 §4.3, §6.2; 32 §4.2): mode 0 carries JSON-RPC
//! into `raw_json_request`, mode 1 hands the records to the ACP agent half
//! (32 §4.3).
//!
//! Two uplinks feed conns here (69 §2), told apart by [`Uplink`]: the
//! ENGINE identity's (its in-memory single pairing) and the MESH identity's
//! (the replicated `devices`/`pending` rows; registered in `State::mesh_conns`).
//! An unknown static becomes a doc guest (59 §8.3) on either; on the mesh a
//! REVOKED static (its row stays) is refused with `CLOSE 3` instead.

use std::collections::HashMap;
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
use crate::state::{ConnHandle, Ctx, State};
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

/// IKpsk2 msg1 payload. Tolerates unknown fields (older and newer apps).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairHello {
    /// Device display name.
    pub name: String,
    /// `ios` | `macos` | `web` | `theuth` | …
    pub platform: String,
    /// The `pending` row id from the pairing string's 4th part (69 decision
    /// 4): psk2 is mixed in after msg1, so this is how the hub picks the PSK
    /// among several replicated pending rows. `None` (older apps, engine
    /// pairings) → the single live pending row, if exactly one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
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
    /// Unknown static that completed `IK` (59 §8.3): a guest conn, mode 2. Nothing but `doc_join`
    /// until `document` admits the static; the caller applies the rate limit before sending msg2.
    Guest {
        /// Transport state.
        transport: TransportState,
        /// Guest static.
        key: [u8; 32],
        /// msg2 to send.
        msg2: Vec<u8>,
        /// Noise handshake hash: what the join proof binds to.
        handshake_hash: [u8; 32],
    },
    /// Pairing: the row is written after the first record decrypts.
    Pair {
        /// Transport state.
        transport: TransportState,
        /// Device static.
        key: [u8; 32],
        /// What the device said.
        hello: PairHello,
        /// The PSK the chooser picked for this handshake.
        psk: [u8; 32],
        /// msg2 to send.
        msg2: Vec<u8>,
    },
}

fn static_of(hs: &snow::HandshakeState) -> Option<[u8; 32]> {
    hs.get_remote_static()?.try_into().ok()
}

/// Read `msg1` as IK; a known static → session (payload `[]` or `[mode]`; anything longer is
/// refused), an unknown one → guest (59 §8.3); if IK does not read, re-read it as IKpsk2 →
/// pairing, the PSK chosen by `psk` from the `PairHello.pending` hint (69 decision 4: psk2 is
/// mixed in after msg1, so the hello is readable first); `psk` returning `None` → `None` (CLOSE 3).
pub fn respond(
    msg1: &[u8],
    identity: &Identity,
    name: &str,
    known: impl Fn(&[u8; 32]) -> bool,
    psk: impl FnOnce(Option<&str>) -> Option<[u8; 32]>,
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
        if known(&key) {
            return Some(Handshake::Session {
                transport: hs.into_transport_mode().ok()?,
                key,
                msg2,
                mode,
            });
        }
        let handshake_hash: [u8; 32] = hs.get_handshake_hash().try_into().ok()?;
        return Some(Handshake::Guest {
            transport: hs.into_transport_mode().ok()?,
            key,
            msg2,
            handshake_hash,
        });
    }

    // IKpsk2's psk token is in msg2: msg1 reads with a placeholder, the real
    // PSK goes in once the hello says which pending row the device holds.
    let mut hs = Builder::new(params("IKpsk2").ok()?)
        .local_private_key(&identity.private)
        .ok()?
        .prologue(&prologue)
        .ok()?
        .psk(2, &[0u8; 32])
        .ok()?
        .build_responder()
        .ok()?;
    let n = hs.read_message(msg1, &mut payload).ok()?;
    let hello: PairHello = serde_json::from_slice(&payload[..n]).ok()?;
    let key = static_of(&hs)?;
    let psk = psk(hello.pending.as_deref())?;
    hs.set_psk(2, &psk).ok()?;
    let accept = serde_json::to_vec(&PairAccept { name }).ok()?;
    let n = hs.write_message(&accept, &mut msg2).ok()?;
    msg2.truncate(n);
    Some(Handshake::Pair {
        transport: hs.into_transport_mode().ok()?,
        key,
        hello,
        psk,
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

/// Which uplink's identity a conn answers with (69 §2, §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uplink {
    /// The engine's own identity: its single in-memory pairing, the engine-id share links.
    Engine,
    /// The mesh identity (`mesh/device_key`), served by the hub only: the replicated `devices` and
    /// `pending` rows, the mesh-id share links.
    Mesh,
}

impl Uplink {
    /// The conn map this uplink's conns live in.
    fn conns(self, st: &mut State) -> &mut HashMap<u32, ConnHandle> {
        match self {
            Uplink::Engine => &mut st.conns,
            Uplink::Mesh => &mut st.mesh_conns,
        }
    }
}

/// Run an engine-uplink conn to completion. Publishes `LinkDevice`/`LinkPaired`/`LinkPairing`;
/// persists `last_seen_ms` on close.
pub async fn run(c: Conn) {
    run_as(c, Uplink::Engine).await
}

/// Run a mesh-uplink conn to completion (69 §5 decision 12, Y): `IK` with the mesh static, a known
/// static = an active replicated `devices` row, the pairing PSK from the replicated `pending` rows
/// (`PairHello.pending` → `Pending::take`, else the single live row — consumed either way), and an
/// unknown static is a guest exactly as on the engine uplink ([`GUEST_METHODS`], the join deadline,
/// the rate limit). The mesh uplink supervisor (`mesh_uplink::run_once`) spawns this per `OPEN`.
pub async fn run_mesh(c: Conn) {
    run_as(c, Uplink::Mesh).await
}

/// Every exit — refused, timed out, closed — takes the conn's handle out of its map.
async fn run_as(mut c: Conn, uplink: Uplink) {
    let session = serve(&mut c, uplink).await;
    let closed = {
        let mut st = c.ctx.lock();
        uplink.conns(&mut st).remove(&c.id);
        session
            .filter(|key| st.devices.touch_last_seen(key, now_ms()))
            .map(|key| (device_id(&key), st.online(&key)))
    };
    if let Some((id, online)) = closed {
        c.ctx.rt.publish(LinkEvent::LinkDevice { id, online }).await;
    }
}

/// Handshake and records; `Some(key)` when the conn became a device session.
async fn serve(c: &mut Conn, uplink: Uplink) -> Option<[u8; 32]> {
    let deadline = Instant::now() + HANDSHAKE_DEADLINE;
    let msg1 = tokio::select! {
        _ = c.cancel.cancelled() => { c.close(CLOSE_NORMAL).await; return None }
        m = tokio::time::timeout_at(deadline, c.inbound.recv()) => match m {
            Ok(Some(b)) => b,
            Ok(None) => return None,
            Err(_) => { c.close(CLOSE_REFUSED).await; return None }
        },
    };

    let hs = {
        let mut guard = c.ctx.lock();
        let full = guard.devices.len() >= guard.cfg.max_devices;
        match uplink {
            // The ENGINE identity's PSK is the single in-memory pairing, hint ignored.
            Uplink::Engine => {
                let st = &*guard;
                let psk = st.live_pairing().filter(|_| !full).map(|p| p.secret);
                st.identity.as_ref().and_then(|identity| {
                    respond(
                        &msg1,
                        identity,
                        st.name(),
                        |k| st.devices.by_key(k).is_some(),
                        |_hint| psk,
                    )
                })
            }
            // 69 decision 4: the PSK is the replicated pending row the hello
            // names, else the only live one; both are consumed here (a
            // second handshake with the same id gets nothing).
            Uplink::Mesh => {
                let State {
                    cfg,
                    hostname,
                    mesh_identity,
                    devices,
                    pending,
                    ..
                } = &mut *guard;
                let name = if cfg.name.is_empty() {
                    hostname.as_str()
                } else {
                    cfg.name.as_str()
                };
                mesh_identity.as_ref().and_then(|identity| {
                    respond(
                        &msg1,
                        identity,
                        name,
                        |k| devices.by_key(k).is_some(),
                        |hint| {
                            if full {
                                return None;
                            }
                            hint.and_then(|id| pending.take(id)).or_else(|| {
                                pending.single_live().and_then(|(id, _)| pending.take(&id))
                            })
                        },
                    )
                })
            }
        }
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
            psk,
            msg2,
        }) => (transport, key, Some((hello, psk)), msg2, 0),
        Some(Handshake::Guest {
            mut transport,
            key,
            msg2,
            handshake_hash,
        }) => {
            // 69 §5: a revoked device (its row stays, `revoked_ms` set) is told 3 — the app shows
            // "revoked — pair again" — while a never-seen static is a doc guest.
            if uplink == Uplink::Mesh && c.ctx.lock().devices.is_revoked(&key) {
                tracing::debug!(conn = c.id, "revoked device on the mesh uplink; refused");
                c.close(CLOSE_REFUSED).await;
                return None;
            }
            // 59 §8.3, §11: the DH is spent; the budget decides whether msg2 goes out.
            let admitted = {
                let mut st = c.ctx.lock();
                let per_minute = st.cfg.guest_handshakes_per_minute;
                let ok = st.guests.take(per_minute, Instant::now().into_std());
                if ok && let Some(h) = uplink.conns(&mut st).get_mut(&c.id) {
                    h.device = Some(key);
                }
                ok
            };
            if !admitted {
                tracing::debug!(
                    conn = c.id,
                    "guest handshake over the per-minute budget; refused"
                );
                c.close(CLOSE_REFUSED).await;
                return None;
            }
            if !c.send(msg2).await {
                return None;
            }
            run_guest(c, &mut transport, key, handshake_hash, deadline).await;
            return None;
        }
        None => {
            c.close(CLOSE_REFUSED).await;
            return None;
        }
    };
    if !c.send(msg2).await {
        return None;
    }
    if pending.is_none() {
        mark_online(c, &key, uplink).await;
    }

    let session = match mode {
        0 => run_api(c, &mut transport, &key, pending, deadline, uplink).await,
        1 => {
            run_acp(c, &mut transport, deadline).await;
            true
        }
        _ => {
            c.close(CLOSE_REFUSED).await;
            true
        }
    };
    session.then_some(key)
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
    mut pending: Option<(PairHello, [u8; 32])>,
    deadline: Instant,
    uplink: Uplink,
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
                if let Some((hello, psk)) = pending.take() {
                    let ok = match uplink {
                        Uplink::Mesh => commit_mesh_pairing(c, key, hello).await,
                        Uplink::Engine => commit_pairing(c, key, hello, psk).await,
                    };
                    if !ok {
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

/// Methods a mode-2 conn may call (59 §8.3): `doc_join` to become a member, then the member
/// subset of `doc_*`. `document` checks membership and role on every one of them; this list only
/// keeps a stranger off the rest of the API.
pub const GUEST_METHODS: [&str; 11] = [
    "doc_join",
    "doc_open",
    "doc_subscribe",
    "doc_unsubscribe",
    "doc_update",
    "doc_awareness",
    "doc_set_name",
    // 144 §11: the notebook guest view's read-only methods; membership-checked per call.
    "notebook_get",
    "notebook_items_list",
    "source_get",
    "cite_resolve",
];

/// A guest conn has this long after `IK` to be admitted to some document (59 §8.3).
const JOIN_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    method: Option<&'a str>,
    #[serde(borrow)]
    id: Option<&'a JsonRawValue>,
}

/// `Some(response)` when `text` is a request a guest may not make: `-32601` with the request's id.
/// Unparseable requests pass through — jsonrpsee answers those with its own parse error.
pub fn guest_gate(text: &[u8]) -> Option<Box<JsonRawValue>> {
    let env: Envelope<'_> = serde_json::from_slice(text).ok()?;
    let method = env.method?;
    if GUEST_METHODS.contains(&method) {
        return None;
    }
    let id = env.id.map_or("null", JsonRawValue::get);
    JsonRawValue::from_string(format!(
        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"Method not found"}}}}"#
    ))
    .ok()
}

/// Mode 2 (59 §8.3): like [`run_api`] with a guest [`RpcPeer`] (static + handshake hash for the
/// join proof), [`guest_gate`] on every request, `CLOSE 3` when nothing admitted the static within
/// [`JOIN_DEADLINE`] or after [`crate::Guest::MAX_REFUSALS`] refused member calls. Return visits need
/// no join: a member's first `doc_open`/`doc_subscribe` succeeds and marks the conn joined.
async fn run_guest(
    c: &mut Conn,
    transport: &mut TransportState,
    key: [u8; 32],
    handshake_hash: [u8; 32],
    deadline: Instant,
) {
    let (peer, _peer_guard) = RpcPeer::open_guest(key, handshake_hash);
    let guest = peer.clone();
    let methods = tokio::select! {
        _ = c.cancel.cancelled() => return c.close(CLOSE_NORMAL).await,
        d = tokio::time::timeout_at(deadline, c.ctx.dispatch()) => match d {
            Ok(d) => Arc::new(peer_methods(&d.methods, peer)),
            Err(_) => return c.close(CLOSE_REFUSED).await,
        },
    };
    let guest = guest.guest().expect("opened as guest");
    let (resp_tx, mut resp_rx) = mpsc::channel::<Box<JsonRawValue>>(256);
    let mut asm = Reassembler::default();
    let mut buf = vec![0u8; MAX_MSG];
    let join_deadline = tokio::time::sleep_until(Instant::now() + JOIN_DEADLINE);
    tokio::pin!(join_deadline);

    loop {
        tokio::select! {
            _ = c.cancel.cancelled() => { c.close(CLOSE_NORMAL).await; break }
            _ = &mut join_deadline, if !guest.joined() => {
                tracing::debug!(conn = c.id, "guest did not join within {JOIN_DEADLINE:?}; refused");
                c.close(CLOSE_REFUSED).await;
                break
            }
            m = c.inbound.recv() => {
                let Some(bytes) = m else { break };
                let Ok(n) = transport.read_message(&bytes, &mut buf) else {
                    c.close(CLOSE_REFUSED).await;
                    break;
                };
                if let Some(text) = asm.push(&buf[..n]) {
                    match guest_gate(&text) {
                        Some(refused) => {
                            if !c.write_records(transport, refused.get().as_bytes(), &mut buf).await { break }
                        }
                        None => { tokio::spawn(dispatch(methods.clone(), text, resp_tx.clone())); }
                    }
                }
            }
            r = resp_rx.recv() => {
                let Some(raw) = r else { break };
                if !c.write_records(transport, raw.get().as_bytes(), &mut buf).await { break }
                if guest.exhausted() {
                    tracing::debug!(conn = c.id, "guest refused {} times; closed", crate::Guest::MAX_REFUSALS);
                    c.close(CLOSE_REFUSED).await;
                    break;
                }
            }
        }
    }
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

async fn mark_online(c: &Conn, key: &[u8; 32], uplink: Uplink) {
    {
        let mut st = c.ctx.lock();
        if let Some(h) = uplink.conns(&mut st).get_mut(&c.id) {
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

/// The engine path's commit: first transport record decrypted under the PSK —
/// enrol the device, consume the pairing. `false` when the pairing this
/// handshake used is gone (cancelled, expired or replaced).
async fn commit_pairing(c: &Conn, key: &[u8; 32], hello: PairHello, psk: [u8; 32]) -> bool {
    let row = {
        let mut st = c.ctx.lock();
        let same = st.live_pairing().is_some_and(|p| p.secret == psk);
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
            revoked_ms: None,
        };
        st.devices.insert(row.clone());
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
    // 69: the replicated registry changed (the engine pairing path; the mesh
    // path adds `DevicePaired { id }`).
    c.ctx.rt.publish(LinkEvent::DevicesChanged).await;
    mark_online(c, key, Uplink::Engine).await;
    true
}

/// The mesh path's commit (69 decision 3): the pending row was consumed at
/// the handshake, so the first record decrypting under its PSK is the proof;
/// write the replicated `devices` row. `false` when the registry filled up
/// meanwhile.
async fn commit_mesh_pairing(c: &Conn, key: &[u8; 32], hello: PairHello) -> bool {
    let id = {
        let mut st = c.ctx.lock();
        if st.devices.len() >= st.cfg.max_devices {
            return false;
        }
        let row = Row {
            id: device_id(key),
            key: *key,
            name: hello.name,
            platform: hello.platform,
            paired_at_ms: now_ms(),
            last_seen_ms: 0,
            revoked_ms: None,
        };
        st.devices.insert(row.clone());
        row.id
    };
    tracing::info!(device = %id, "device paired to the mesh");
    c.ctx.rt.publish(LinkEvent::DevicePaired { id }).await;
    c.ctx.rt.publish(LinkEvent::DevicesChanged).await;
    mark_online(c, key, Uplink::Mesh).await;
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

        // Unknown device, IK, no pairing → a guest conn (59 §8.3), with the handshake hash both sides hold.
        let mut dev = initiator("IK", &dpriv, &harness, None);
        let n = dev.write_message(&[], &mut msg1).unwrap();
        let Some(Handshake::Guest {
            key,
            msg2,
            handshake_hash,
            ..
        }) = respond(&msg1[..n], &harness, "h", |_| false, |_| None)
        else {
            panic!("expected guest");
        };
        assert_eq!(key, dpub);
        dev.read_message(&msg2, &mut buf).unwrap();
        assert_eq!(dev.get_handshake_hash(), &handshake_hash);
        // A payload longer than the mode byte is still refused.
        let mut dev = initiator("IK", &dpriv, &harness, None);
        let n = dev.write_message(&[2, 2], &mut msg1).unwrap();
        assert!(respond(&msg1[..n], &harness, "h", |_| false, |_| None).is_none());

        // Unknown device, IKpsk2 msg1 with a pending hint → Pair; IK read fails cleanly first.
        let mut dev = initiator("IKpsk2", &dpriv, &harness, Some(&psk));
        let hello = br#"{"name":"iPhone","platform":"ios","pending":"p1","future":1}"#;
        let n = dev.write_message(hello, &mut msg1).unwrap();
        // psk2 is mixed after `se`: msg1 reads under any secret, so a wrong
        // secret only shows when the device cannot read msg2 (and its first
        // record would not decrypt here — nothing is enrolled before that).
        let Some(Handshake::Pair { msg2: bad_msg2, .. }) =
            respond(&msg1[..n], &harness, "h", |_| false, |_| Some([0u8; 32]))
        else {
            panic!("msg1 must read under any psk");
        };
        assert!(dev.read_message(&bad_msg2, &mut buf).is_err());
        // No PSK for the hint → refused before msg2.
        assert!(
            respond(
                &msg1[..n],
                &harness,
                "h",
                |_| false,
                |hint| {
                    assert_eq!(hint, Some("p1"));
                    None
                }
            )
            .is_none()
        );
        let Some(Handshake::Pair {
            mut transport,
            key,
            hello,
            psk: chosen,
            msg2,
        }) = respond(
            &msg1[..n],
            &harness,
            "Sky",
            |_| false,
            |hint| (hint == Some("p1")).then_some(psk),
        )
        else {
            panic!("expected pairing");
        };
        assert_eq!(key, dpub);
        assert_eq!(chosen, psk);
        assert_eq!(
            (
                hello.name.as_str(),
                hello.platform.as_str(),
                hello.pending.as_deref()
            ),
            ("iPhone", "ios", Some("p1"))
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
        }) = respond(&msg1[..n], &harness, "Sky", |k| k == &dpub, |_| Some(psk))
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

        // Another (unpaired) key sends IK while a pairing is pending: IK reads → guest, not pairing.
        let (opriv, opub) = keypair();
        let mut other = initiator("IK", &opriv, &harness, None);
        let n = other.write_message(&[], &mut msg1).unwrap();
        assert!(matches!(
            respond(&msg1[..n], &harness, "h", |k| k == &dpub, |_| Some(psk)),
            Some(Handshake::Guest { key, .. }) if key == opub
        ));
    }

    #[test]
    fn guest_gate_admits_member_methods_only() {
        for m in GUEST_METHODS {
            let req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{m}","params":[]}}"#);
            assert!(guest_gate(req.as_bytes()).is_none(), "{m}");
        }
        for m in [
            "list_sessions",
            "doc_list",
            "doc_link_create",
            "doc_members",
            "info",
            "send_input",
        ] {
            let req = format!(r#"{{"jsonrpc":"2.0","id":"a-{m}","method":"{m}","params":[]}}"#);
            let refused = guest_gate(req.as_bytes()).expect(m);
            let v: serde_json::Value = serde_json::from_str(refused.get()).unwrap();
            assert_eq!(v["id"], format!("a-{m}"));
            assert_eq!(v["error"]["code"], -32601);
        }
        // No id → null id; no method → jsonrpsee's problem, not ours; garbage passes through too.
        let v: serde_json::Value = serde_json::from_str(
            guest_gate(br#"{"jsonrpc":"2.0","method":"info"}"#)
                .unwrap()
                .get(),
        )
        .unwrap();
        assert!(v["id"].is_null());
        assert!(guest_gate(br#"{"jsonrpc":"2.0","id":1}"#).is_none());
        assert!(guest_gate(b"not json").is_none());
    }

    #[test]
    fn guest_bucket_admits_ten_per_minute() {
        let mut b = crate::state::Bucket::default();
        let t0 = std::time::Instant::now();
        for _ in 0..10 {
            assert!(b.take(10, t0));
        }
        assert!(!b.take(10, t0));
        // Six seconds refill one token; a minute refills the lot but never past the cap.
        assert!(b.take(10, t0 + Duration::from_secs(6)));
        assert!(!b.take(10, t0 + Duration::from_secs(6)));
        let t1 = t0 + Duration::from_secs(600);
        for _ in 0..10 {
            assert!(b.take(10, t1));
        }
        assert!(!b.take(10, t1));
    }

    /// A mesh conn task over channels, registered in `mesh_conns` as the
    /// uplink does it: the map owns the only inbound sender, so removing the
    /// entry (a relay `CLOSE`) ends the task. Returns the conn's outbound frames.
    fn mesh_conn(ctx: &Arc<Ctx>, id: u32) -> mpsc::Receiver<Message> {
        let (inbound, rx) = mpsc::channel(16);
        let (out, out_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        ctx.lock().mesh_conns.insert(
            id,
            ConnHandle {
                device: None,
                inbound,
                cancel: cancel.clone(),
            },
        );
        tokio::spawn(run_mesh(Conn {
            ctx: ctx.clone(),
            id,
            inbound: rx,
            out,
            cancel,
        }));
        out_rx
    }

    /// One relay `DATA` payload into conn `id`.
    async fn data(ctx: &Ctx, id: u32, payload: &[u8]) {
        let tx = ctx.lock().mesh_conns[&id].inbound.clone();
        tx.send(Bytes::copy_from_slice(payload)).await.unwrap();
    }

    /// Next frame from the conn as `(op, conn, payload)`.
    async fn frame(out: &mut mpsc::Receiver<Message>) -> (u8, u32, Bytes) {
        let Some(Message::Binary(b)) = tokio::time::timeout(Duration::from_secs(5), out.recv())
            .await
            .expect("frame")
        else {
            panic!("binary frame");
        };
        uplink::parse(&b).unwrap()
    }

    /// Encrypt one JSON-RPC message as the device would and hand its records to conn `id`.
    async fn device_send(ctx: &Ctx, id: u32, t: &mut TransportState, text: &str) {
        let mut buf = vec![0u8; MAX_MSG];
        for rec in split(text.as_bytes()) {
            let n = t.write_message(&rec, &mut buf).unwrap();
            data(ctx, id, &buf[..n]).await;
        }
    }

    /// One JSON-RPC response as the device reads it (records until FIN).
    async fn device_recv(
        t: &mut TransportState,
        rx: &mut mpsc::Receiver<Message>,
    ) -> serde_json::Value {
        let mut asm = Reassembler::default();
        let mut buf = vec![0u8; MAX_MSG];
        loop {
            let (op, _, payload) = frame(rx).await;
            assert_eq!(op, uplink::OP_DATA);
            let n = t.read_message(&payload, &mut buf).unwrap();
            if let Some(text) = asm.push(&buf[..n]) {
                return serde_json::from_slice(&text).unwrap();
            }
        }
    }

    /// A device does IKpsk2 with `psk` and `hello`, then sends one record.
    /// `Ok(out)` once the record was answered; `Err(reason)` when refused.
    async fn pair_attempt(
        ctx: &Arc<Ctx>,
        id: u32,
        hub: &Identity,
        dev: &[u8; 32],
        psk: &[u8; 32],
        hello: &[u8],
    ) -> Result<mpsc::Receiver<Message>, u8> {
        let mut out = mesh_conn(ctx, id);
        let mut hs = initiator("IKpsk2", dev, hub, Some(psk));
        let mut msg1 = vec![0u8; MAX_MSG];
        let n = hs.write_message(hello, &mut msg1).unwrap();
        data(ctx, id, &msg1[..n]).await;
        let (op, c, payload) = frame(&mut out).await;
        assert_eq!(c, id);
        if op == uplink::OP_CLOSE {
            return Err(payload[0]);
        }
        assert_eq!(op, uplink::OP_DATA);
        let mut buf = vec![0u8; MAX_MSG];
        hs.read_message(&payload, &mut buf)
            .expect("msg2 under the right PSK");
        let mut t = hs.into_transport_mode().unwrap();
        // The first record is the proof; `{}` gets jsonrpsee's parse error back.
        let n = t.write_message(b"\x01{}", &mut buf).unwrap();
        data(ctx, id, &buf[..n]).await;
        let (op, _, _) = frame(&mut out).await;
        assert_eq!(op, uplink::OP_DATA, "a response record, not a CLOSE");
        Ok(out)
    }

    /// A dispatch with one member-only method: `info` answers a session, the guest gate refuses it.
    fn dispatch_with_info(ctx: &Ctx) {
        let mut module = jsonrpsee::RpcModule::new(());
        module.register_method("info", |_, _, _| true).unwrap();
        let acp: crate::types::AcpServe = Arc::new(|_, _| Box::pin(async {}));
        ctx.set_dispatch(crate::types::SetDispatch {
            methods: module.into(),
            acp,
        });
    }

    /// 69 decisions 3–4 on the mesh path: a device whose hello names a
    /// pending row another engine wrote pairs (the row is consumed, the
    /// `devices` row written, `last_seen_ms` set on close); once revoked its
    /// `IK` is refused with `CLOSE 3` before msg2 — not a guest, not a session.
    #[tokio::test]
    async fn mesh_conn_pairs_from_a_replicated_pending_row_and_refuses_revoked() {
        use crate::mesh_uplink::testing;
        let hub = testing::identity();
        let ctx = testing::ctx("ws://x", None, Some(hub.clone()));
        dispatch_with_info(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let store = crate::store::Store::open(&dir.path().join(crate::store::DB))
            .await
            .unwrap();
        ctx.lock().pending = crate::devices::Pending::new(Some(store.clone()), Some(vault.clone()));

        // Engine B shows the QR: its own mirror writes the row; ours learns it from the table.
        let mut b = crate::devices::Pending::new(Some(store.clone()), Some(vault));
        let (pending_id, psk, _) = b.create("work").unwrap();
        let rows = loop {
            let rows = store.pending().await.unwrap();
            if !rows.is_empty() {
                break rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(crate::follow::reload_pending(&ctx, rows.clone()));
        assert!(
            !crate::follow::reload_pending(&ctx, rows),
            "already mirrored"
        );

        let (dpriv, dpub) = keypair();
        let hello = format!(r#"{{"name":"iPhone","platform":"ios","pending":"{pending_id}"}}"#);
        let _out = pair_attempt(&ctx, 1, &hub, &dpriv, &psk, hello.as_bytes())
            .await
            .expect("paired");
        {
            let st = ctx.lock();
            let row = st
                .devices
                .by_key(&dpub)
                .expect("devices row written at the first record");
            assert_eq!(
                (row.name.as_str(), row.platform.as_str(), row.last_seen_ms),
                ("iPhone", "ios", 0)
            );
            assert_eq!(st.mesh_conns[&1].device, Some(dpub), "online");
            assert!(st.pending.is_empty(), "the pending row was consumed");
        }
        // Session close (a relay CLOSE): the task ends and `last_seen_ms` is written.
        ctx.lock().mesh_conns.remove(&1);
        testing::until(|| ctx.lock().devices.by_key(&dpub).unwrap().last_seen_ms > 0).await;

        // Known device: IK → session; `info` answers.
        let mut out = mesh_conn(&ctx, 2);
        let mut hs = initiator("IK", &dpriv, &hub, None);
        let mut msg1 = vec![0u8; MAX_MSG];
        let n = hs.write_message(&[], &mut msg1).unwrap();
        data(&ctx, 2, &msg1[..n]).await;
        let (op, _, msg2) = frame(&mut out).await;
        assert_eq!(op, uplink::OP_DATA);
        let mut buf = vec![0u8; MAX_MSG];
        hs.read_message(&msg2, &mut buf).unwrap();
        let mut t = hs.into_transport_mode().unwrap();
        testing::until(|| {
            ctx.lock()
                .mesh_conns
                .get(&2)
                .is_some_and(|h| h.device == Some(dpub))
        })
        .await;
        device_send(
            &ctx,
            2,
            &mut t,
            r#"{"jsonrpc":"2.0","id":1,"method":"info"}"#,
        )
        .await;
        assert_eq!(device_recv(&mut t, &mut out).await["result"], true);
        ctx.lock().mesh_conns.remove(&2);
        assert!(out.recv().await.is_none(), "conn task ended");

        // Revoked (here or, replicated, anywhere): the same IK is refused before msg2 — the app
        // shows "revoked — pair again" on 3 (69 smoke invariant 6); unknown statics are guests.
        let id = device_id(&dpub);
        assert!(ctx.lock().devices.remove(&id).is_some());
        let mut out = mesh_conn(&ctx, 3);
        let mut hs = initiator("IK", &dpriv, &hub, None);
        let n = hs.write_message(&[2], &mut msg1).unwrap();
        data(&ctx, 3, &msg1[..n]).await;
        assert_eq!(
            frame(&mut out).await,
            (uplink::OP_CLOSE, 3, Bytes::from_static(&[CLOSE_REFUSED]))
        );
        assert!(out.recv().await.is_none(), "conn task ended");
        assert!(
            ctx.lock().mesh_conns.is_empty(),
            "a refused conn leaves no handle behind"
        );
    }

    /// A hint-less hello (older app, 3-part string) pairs only while exactly
    /// one pending row is live; with two it is refused, and a wrong hint
    /// falls back the same way.
    #[tokio::test]
    async fn mesh_conn_without_a_hint_needs_exactly_one_live_pending_row() {
        use crate::mesh_uplink::testing;
        let hub = testing::identity();
        let ctx = testing::ctx("ws://x", None, Some(hub.clone()));
        dispatch_with_info(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        ctx.lock().pending = crate::devices::Pending::new(None, Some(vault));
        let hello = br#"{"name":"Mac","platform":"macos"}"#;

        // Two live rows: nothing to fall back to.
        let (_, psk1, _) = ctx.lock().pending.create("a").unwrap();
        let (id_b, _, _) = ctx.lock().pending.create("b").unwrap();
        let (d1, _) = keypair();
        assert_eq!(
            pair_attempt(&ctx, 1, &hub, &d1, &psk1, hello).await.err(),
            Some(CLOSE_REFUSED)
        );
        assert_eq!(ctx.lock().pending.len(), 2, "nothing consumed by a refusal");

        // Exactly one live row: a hint-less device gets it — so does one with a stale hint.
        assert!(ctx.lock().pending.take(&id_b).is_some());
        let (d2, d2pub) = keypair();
        let stale = br#"{"name":"Mac","platform":"macos","pending":"gone"}"#;
        let _out = pair_attempt(&ctx, 2, &hub, &d2, &psk1, stale)
            .await
            .expect("single live row");
        assert!(ctx.lock().devices.by_key(&d2pub).is_some());
        assert!(ctx.lock().pending.is_empty());

        // The registry is full: the PSK is not even chosen.
        let (_, psk3, _) = ctx.lock().pending.create("c").unwrap();
        ctx.lock().cfg.max_devices = 1;
        let (d3, _) = keypair();
        assert_eq!(
            pair_attempt(&ctx, 3, &hub, &d3, &psk3, hello).await.err(),
            Some(CLOSE_REFUSED)
        );
        assert_eq!(
            ctx.lock().pending.len(),
            1,
            "kept for when a device is revoked"
        );
    }

    /// 69 §5 (Y): a mesh-uplink conn answers `IK` with the MESH static; an unknown static is a guest
    /// on that identity exactly as on the engine uplink — gated to [`GUEST_METHODS`], registered in
    /// `State::mesh_conns` under its static (what `DropGuest` finds through `State::all_conns`),
    /// closed by that handle's token.
    #[tokio::test]
    async fn mesh_uplink_conn_admits_a_guest_against_the_mesh_static_and_drop_guest_closes_it() {
        use crate::mesh_uplink::testing;
        let mesh = testing::identity();
        let engine = testing::identity();
        let (gpriv, gpub) = keypair();
        let ctx = testing::ctx("ws://x", Some(engine.clone()), Some(mesh.clone()));
        // The dispatch: `doc_join` answers the guest's static from the peer the conn put in the extensions.
        let mut module = jsonrpsee::RpcModule::new(());
        module
            .register_method("doc_join", |_, _, ext| {
                let g = ext
                    .get::<RpcPeer>()
                    .and_then(|p| p.guest())
                    .expect("guest peer");
                g.mark_joined();
                crate::devices::device_id(&g.key)
            })
            .unwrap();
        ctx.set_dispatch(crate::types::SetDispatch {
            methods: module.into(),
            acp: Arc::new(|_, _| Box::pin(async {})),
        });
        let mut out = mesh_conn(&ctx, 1);

        // The guest dials the MESH identity (prologue and static from the share URL's mesh id).
        let mut dev = initiator("IK", &gpriv, &mesh, None);
        let mut msg1 = vec![0u8; MAX_MSG];
        let n = dev.write_message(&[2], &mut msg1).unwrap();
        data(&ctx, 1, &msg1[..n]).await;
        let (op, _, msg2) = frame(&mut out).await;
        assert_eq!(op, uplink::OP_DATA);
        let mut buf = vec![0u8; MAX_MSG];
        dev.read_message(&msg2, &mut buf).unwrap();
        let mut dev = dev.into_transport_mode().unwrap();
        assert_eq!(
            ctx.lock().mesh_conns[&1].device,
            Some(gpub),
            "registered under the guest static"
        );

        // Off the guest list: refused by the gate, never dispatched.
        device_send(
            &ctx,
            1,
            &mut dev,
            r#"{"jsonrpc":"2.0","id":1,"method":"doc_link_create","params":[]}"#,
        )
        .await;
        let v = device_recv(&mut dev, &mut out).await;
        assert_eq!(v["error"]["code"], -32601);
        // `doc_join` reaches the dispatch as this guest.
        device_send(
            &ctx,
            1,
            &mut dev,
            r#"{"jsonrpc":"2.0","id":2,"method":"doc_join","params":[]}"#,
        )
        .await;
        let v = device_recv(&mut dev, &mut out).await;
        assert_eq!(v["result"], crate::devices::device_id(&gpub));

        // `DropGuest` (59 §8.4, C2): the actor cancels every conn of both uplinks with this static —
        // so this one goes with `CLOSE 0`.
        let dropped: Vec<_> = ctx
            .lock()
            .all_conns()
            .filter(|c| c.device == Some(gpub))
            .map(|c| c.cancel.clone())
            .collect();
        assert_eq!(dropped.len(), 1);
        dropped[0].cancel();
        assert_eq!(
            frame(&mut out).await,
            (uplink::OP_CLOSE, 1, Bytes::from_static(&[CLOSE_NORMAL]))
        );
        assert!(out.recv().await.is_none(), "conn task ended");
        assert!(
            ctx.lock().mesh_conns.is_empty(),
            "the conn task removed itself"
        );

        // The engine identity does not answer for the mesh: a guest dialling the ENGINE static on the
        // mesh uplink fails the handshake and is refused.
        let mut out = mesh_conn(&ctx, 1);
        let mut dev = initiator("IK", &gpriv, &engine, None);
        let n = dev.write_message(&[2], &mut msg1).unwrap();
        data(&ctx, 1, &msg1[..n]).await;
        assert_eq!(
            frame(&mut out).await,
            (uplink::OP_CLOSE, 1, Bytes::from_static(&[CLOSE_REFUSED]))
        );
        assert!(out.recv().await.is_none(), "conn task ended");
    }
}
