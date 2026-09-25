//! A device-side conn to one node through the relay (32 §4; the Rust mirror of
//! 31 §7.1's `LinkTransport`): the `/h/<id>` WebSocket, presence, the `IK` /
//! `IKpsk2` initiator handshakes and the record codec both conn modes share.

use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use link::conn::{MAX_MSG, PairHello, Reassembler, split};
use link::identity::{Identity, PROTOCOL, b64};
use link::initiator;
use link::pairing::Parsed;
use link::uplink::{CLOSE_REFUSED, OP_CLOSE, OP_DATA, OP_PRESENCE, device_frame, parse_device};
use serde::Deserialize;
use snow::{HandshakeState, TransportState};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// WebSocket connect deadline.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// msg1 → msg2 deadline (31 §4.5).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Records queued per direction between the pump and its consumer.
const QUEUE: usize = 256;

/// `IKpsk2` msg2 payload.
#[derive(Clone, Debug, Deserialize)]
pub struct PairAccept {
    /// The node's display name.
    pub name: String,
}

/// Why a handshake did not produce a transport.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// `PRESENCE 0`: the node is not registered at the relay.
    #[error("node is offline")]
    Offline,
    /// `CLOSE 3` before msg2: unknown device, expired/used pairing, or over limit.
    #[error("refused by the node")]
    Refused,
    /// Socket or Noise failure.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// One `/h/<id>` device socket.
pub struct Socket {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// `ws[s]://<relay_host>` for a parsed pairing string.
pub fn relay_url(parsed: &Parsed) -> String {
    let scheme = if parsed.secure { "wss" } else { "ws" };
    format!("{scheme}://{}", parsed.relay_host)
}

/// Connect to `<relay>/h/<b64(harness_pub)>` with the `theuth-link-1` subprotocol.
pub async fn open(relay: &str, harness_pub: &[u8; 32]) -> anyhow::Result<Socket> {
    let url = format!("{}/h/{}", relay.trim_end_matches('/'), b64(harness_pub));
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", PROTOCOL.parse()?);
    let (ws, _) = tokio::time::timeout(DIAL_TIMEOUT, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| anyhow::anyhow!("relay {relay}: connect timed out"))??;
    Ok(Socket { ws })
}

impl Socket {
    /// Next `(op, payload)` frame; `None` on EOF, close or error.
    pub async fn recv(&mut self) -> Option<(u8, Bytes)> {
        loop {
            match self.ws.next().await? {
                Ok(Message::Binary(b)) => {
                    if let Some(f) = parse_device(&b) {
                        return Some(f);
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// One `DATA` frame.
    pub async fn send_data(&mut self, payload: &[u8]) -> anyhow::Result<()> {
        Ok(self.ws.send(device_frame(OP_DATA, payload)).await?)
    }

    /// Close the WebSocket (the relay tells the node `CLOSE 0`).
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// Read until the first `PRESENCE`; `Some(online)`, or `None` when the socket ended first.
pub async fn presence(sock: &mut Socket) -> Option<bool> {
    loop {
        match sock.recv().await? {
            (OP_PRESENCE, p) => return Some(p.first() == Some(&1)),
            (OP_CLOSE, _) => return None,
            _ => {}
        }
    }
}

/// `IK` initiator with `mode` (32 §4.2); the socket must have seen `PRESENCE 1`.
/// Returns the transport and the `SessionHello` JSON.
pub async fn handshake(
    sock: &mut Socket,
    identity: &Identity,
    harness_pub: &[u8; 32],
    mode: u8,
) -> Result<(TransportState, Vec<u8>), HandshakeError> {
    let (hs, msg1) = initiator::session(identity, harness_pub, mode)?;
    complete(sock, hs, &msg1).await
}

/// `IKpsk2` initiator from a pairing string (32 §4.1). The node enrols us on
/// the first transport record: the caller must send one right away.
pub async fn pair(
    sock: &mut Socket,
    identity: &Identity,
    parsed: &Parsed,
    hello: &PairHello,
) -> Result<(TransportState, PairAccept), HandshakeError> {
    let (hs, msg1) = initiator::pair(identity, &parsed.harness_pub, &parsed.secret, hello)?;
    let (transport, payload) = complete(sock, hs, &msg1).await?;
    let accept = serde_json::from_slice(&payload)
        .map_err(|e| HandshakeError::Other(anyhow::anyhow!("PairAccept: {e}")))?;
    Ok((transport, accept))
}

/// Send msg1, read msg2 within [`HANDSHAKE_TIMEOUT`], finish.
async fn complete(
    sock: &mut Socket,
    hs: HandshakeState,
    msg1: &[u8],
) -> Result<(TransportState, Vec<u8>), HandshakeError> {
    sock.send_data(msg1).await?;
    let msg2 = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match sock.recv().await {
                Some((OP_DATA, p)) => return Ok(p),
                Some((OP_CLOSE, r)) if r.first() == Some(&CLOSE_REFUSED) => {
                    return Err(HandshakeError::Refused);
                }
                Some((OP_CLOSE, r)) => {
                    return Err(anyhow::anyhow!("closed by relay (reason {:?})", r.first()).into());
                }
                Some((OP_PRESENCE, p)) if p.first() != Some(&1) => {
                    return Err(HandshakeError::Offline);
                }
                Some(_) => {}
                None => return Err(anyhow::anyhow!("relay closed the socket").into()),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("handshake timed out"))??;
    Ok(initiator::finish(hs, &msg2)?)
}

/// The record codec (31 §4.3): one JSON-RPC message ↔ encrypted `flags ‖ chunk`
/// records, FIN on the last. Socket-free so it is testable against a responder.
pub struct Codec {
    transport: TransportState,
    asm: Reassembler,
    buf: Vec<u8>,
}

impl Codec {
    /// Wrap a finished handshake.
    pub fn new(transport: TransportState) -> Self {
        Self {
            transport,
            asm: Reassembler::default(),
            buf: vec![0u8; MAX_MSG],
        }
    }

    /// Encrypt one message into `DATA` payloads.
    pub fn encode(&mut self, text: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
        split(text)
            .map(|rec| {
                let n = self.transport.write_message(&rec, &mut self.buf)?;
                Ok(self.buf[..n].to_vec())
            })
            .collect()
    }

    /// Decrypt one `DATA` payload; `Some` when it completed a message.
    pub fn decode(&mut self, data: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let n = self.transport.read_message(data, &mut self.buf)?;
        Ok(self.asm.push(&self.buf[..n]))
    }
}

/// A conn after the handshake: the codec over its socket.
pub struct Channel {
    codec: Codec,
    sock: Socket,
}

impl Channel {
    /// Pair the socket with its negotiated transport.
    pub fn new(sock: Socket, transport: TransportState) -> Self {
        Self {
            codec: Codec::new(transport),
            sock,
        }
    }

    /// Encrypt and send one message.
    pub async fn send_message(&mut self, text: &[u8]) -> anyhow::Result<()> {
        for rec in self.codec.encode(text)? {
            self.sock.send_data(&rec).await?;
        }
        Ok(())
    }

    /// Next complete message; `None` when the conn ended.
    pub async fn recv_message(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.sock.recv().await? {
                (OP_DATA, p) => match self.codec.decode(&p) {
                    Ok(Some(msg)) => return Some(msg),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!("node conn: bad record: {e}");
                        return None;
                    }
                },
                (OP_CLOSE, _) => return None,
                _ => {}
            }
        }
    }

    /// Hand the conn to a pump task: messages in via the receiver, out via
    /// the sender. The pump ends (and closes the socket) when the relay
    /// closes, every sender is dropped, or the receiver is dropped.
    pub fn split(
        self,
    ) -> (
        mpsc::Sender<Vec<u8>>,
        mpsc::Receiver<Vec<u8>>,
        JoinHandle<()>,
    ) {
        let (in_tx, in_rx) = mpsc::channel(QUEUE);
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let task = tokio::spawn(async move {
            let mut ch = self;
            'pump: loop {
                tokio::select! {
                    m = out_rx.recv() => match m {
                        Some(text) => {
                            if ch.send_message(&text).await.is_err() {
                                break 'pump;
                            }
                        }
                        None => break 'pump,
                    },
                    m = ch.recv_message() => match m {
                        Some(msg) => {
                            if in_tx.send(msg).await.is_err() {
                                break 'pump;
                            }
                        }
                        None => break 'pump,
                    },
                }
            }
            ch.sock.close().await;
        });
        (out_tx, in_rx, task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use link::conn::{Handshake, respond};
    use link::identity::params;

    fn identity() -> Identity {
        let kp = snow::Builder::new(params("IK").unwrap())
            .generate_keypair()
            .unwrap();
        Identity::from_private(kp.private.as_slice().try_into().unwrap())
    }

    /// Initiator mode 1 → `link::conn::respond` as the node; one message each
    /// way, the device→node one spanning two records.
    #[test]
    fn codec_round_trip_with_link_responder() {
        let (h, d) = (identity(), identity());
        let (hs, msg1) = initiator::session(&d, &h.public, 1).unwrap();
        let Some(Handshake::Session {
            mut transport,
            msg2,
            mode,
            ..
        }) = respond(&msg1, &h, "node", |k| k == &d.public, |_| None)
        else {
            panic!("expected session");
        };
        assert_eq!(mode, 1);
        let (dev, _) = initiator::finish(hs, &msg2).unwrap();
        let mut codec = Codec::new(dev);

        // device → node: 70 000 bytes → two records, reassembled on the node.
        let big = vec![b'x'; 70_000];
        let recs = codec.encode(&big).unwrap();
        assert_eq!(recs.len(), 2);
        let mut asm = Reassembler::default();
        let mut buf = vec![0u8; MAX_MSG];
        let mut got = None;
        for r in &recs {
            let n = transport.read_message(r, &mut buf).unwrap();
            if let Some(m) = asm.push(&buf[..n]) {
                got = Some(m);
            }
        }
        assert_eq!(got.unwrap(), big);

        // node → device
        let reply = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let mut out = Vec::new();
        for rec in split(reply) {
            let n = transport.write_message(&rec, &mut buf).unwrap();
            out.push(buf[..n].to_vec());
        }
        assert_eq!(out.len(), 1);
        assert_eq!(codec.decode(&out[0]).unwrap().unwrap(), reply);
    }

    #[test]
    fn relay_url_from_parsed() {
        let key = link::pairing::build("ws://127.0.0.1:8787", &[1; 32], &[2; 32], "n", None);
        let p = link::pairing::parse(&key).unwrap();
        assert_eq!(relay_url(&p), "ws://127.0.0.1:8787");
        let key = link::pairing::build("wss://relay.example", &[1; 32], &[2; 32], "n", None);
        assert_eq!(
            relay_url(&link::pairing::parse(&key).unwrap()),
            "wss://relay.example"
        );
    }
}
