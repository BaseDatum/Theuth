//! This harness as a *device* of another harness (32 §4.1): the initiator side
//! of the 31 §4.3 handshakes, byte-identical to a phone's. `IKpsk2` pairs with
//! a node from its pairing string; `IK` opens a session conn whose msg1
//! payload is `[mode]` (32 §4.2). The `nodes` plugin drives these; the relay
//! socket and record pumping live there.

use snow::{Builder, HandshakeState, TransportState};

use crate::conn::{MAX_MSG, PairHello};
use crate::identity::{Identity, PROTOCOL, b64, params};

/// `"theuth-link-1" ‖ base64url(harness_pub)` — the prologue a harness with this
/// static expects ([`Identity::prologue`] from the other side).
pub fn prologue_for(harness_pub: &[u8; 32]) -> Vec<u8> {
    let id = b64(harness_pub);
    let mut p = Vec::with_capacity(PROTOCOL.len() + id.len());
    p.extend_from_slice(PROTOCOL.as_bytes());
    p.extend_from_slice(id.as_bytes());
    p
}

fn builder<'a>(
    pattern: &str,
    identity: &'a Identity,
    harness_pub: &'a [u8; 32],
    prologue: &'a [u8],
) -> anyhow::Result<Builder<'a>> {
    Ok(Builder::new(params(pattern)?)
        .local_private_key(&identity.private)?
        .remote_public_key(harness_pub)?
        .prologue(prologue)?)
}

fn msg1(mut hs: HandshakeState, payload: &[u8]) -> anyhow::Result<(HandshakeState, Vec<u8>)> {
    let mut msg = vec![0u8; MAX_MSG];
    let n = hs.write_message(payload, &mut msg)?;
    msg.truncate(n);
    Ok((hs, msg))
}

/// IKpsk2 initiator: msg1 carrying `hello` (JSON). Returns the handshake state
/// and msg1 bytes.
pub fn pair(
    identity: &Identity,
    harness_pub: &[u8; 32],
    psk: &[u8; 32],
    hello: &PairHello,
) -> anyhow::Result<(HandshakeState, Vec<u8>)> {
    let prologue = prologue_for(harness_pub);
    let hs = builder("IKpsk2", identity, harness_pub, &prologue)?
        .psk(2, psk)?
        .build_initiator()?;
    msg1(hs, &serde_json::to_vec(hello)?)
}

/// IK initiator: msg1 payload `[mode]` (mode 0 → empty payload for byte-compat
/// with 31 devices).
pub fn session(
    identity: &Identity,
    harness_pub: &[u8; 32],
    mode: u8,
) -> anyhow::Result<(HandshakeState, Vec<u8>)> {
    let prologue = prologue_for(harness_pub);
    let hs = builder("IK", identity, harness_pub, &prologue)?.build_initiator()?;
    let payload: &[u8] = if mode == 0 { &[] } else { &[mode] };
    msg1(hs, payload)
}

/// Read msg2; returns transport state and the decrypted msg2 payload
/// (`PairAccept` / `SessionHello` JSON).
pub fn finish(mut hs: HandshakeState, msg2: &[u8]) -> anyhow::Result<(TransportState, Vec<u8>)> {
    let mut payload = vec![0u8; MAX_MSG];
    let n = hs.read_message(msg2, &mut payload)?;
    payload.truncate(n);
    Ok((hs.into_transport_mode()?, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::{Handshake, respond};

    fn identity() -> Identity {
        let kp = Builder::new(params("IK").unwrap())
            .generate_keypair()
            .unwrap();
        Identity::from_private(kp.private.as_slice().try_into().unwrap())
    }

    #[test]
    fn prologue_matches_identity() {
        let h = identity();
        assert_eq!(prologue_for(&h.public), h.prologue());
    }

    #[test]
    fn session_mode_1_round_trip() {
        let (h, d) = (identity(), identity());
        let (hs, msg1) = session(&d, &h.public, 1).unwrap();
        let Some(Handshake::Session {
            mut transport,
            key,
            msg2,
            mode,
        }) = respond(&msg1, &h, "Studio", |k| k == &d.public, |_| None)
        else {
            panic!("expected session");
        };
        assert_eq!((key, mode), (d.public, 1));
        let (mut dev, hello) = finish(hs, &msg2).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&hello).unwrap();
        assert_eq!(v["name"], "Studio");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        // First record both ways under the negotiated keys.
        let mut buf = vec![0u8; MAX_MSG];
        let n = dev.write_message(b"\x01{}", &mut buf).unwrap();
        let enc = buf[..n].to_vec();
        let n = transport.read_message(&enc, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"\x01{}");
    }

    #[test]
    fn session_mode_0_is_empty_payload() {
        let (h, d) = (identity(), identity());
        let (_, msg1) = session(&d, &h.public, 0).unwrap();
        // IK msg1: e (32) ‖ enc(s) (32+16) ‖ enc(payload) (0+16).
        assert_eq!(msg1.len(), 32 + 48 + 16);
        let Some(Handshake::Session { mode, .. }) =
            respond(&msg1, &h, "h", |k| k == &d.public, |_| None)
        else {
            panic!("expected session");
        };
        assert_eq!(mode, 0);
    }

    #[test]
    fn pair_round_trip() {
        let (h, d) = (identity(), identity());
        let psk = [7u8; 32];
        let hello = PairHello {
            name: "Studio Mac".into(),
            platform: "theuth".into(),
            pending: None,
        };
        let (hs, msg1) = pair(&d, &h.public, &psk, &hello).unwrap();
        let Some(Handshake::Pair {
            key, hello, msg2, ..
        }) = respond(
            &msg1,
            &h,
            "Node",
            |_| false,
            |hint| hint.is_none().then_some(psk),
        )
        else {
            panic!("expected pairing");
        };
        assert_eq!(key, d.public);
        assert_eq!(
            (hello.name.as_str(), hello.platform.as_str(), hello.pending),
            ("Studio Mac", "theuth", None)
        );
        let (_, accept) = finish(hs, &msg2).unwrap();
        assert_eq!(accept, br#"{"name":"Node"}"#);
    }

    #[test]
    fn two_byte_payload_is_refused() {
        let (h, d) = (identity(), identity());
        let prologue = prologue_for(&h.public);
        let hs = builder("IK", &d, &h.public, &prologue)
            .unwrap()
            .build_initiator()
            .unwrap();
        let (_, m1) = msg1(hs, &[1, 1]).unwrap();
        assert!(respond(&m1, &h, "h", |k| k == &d.public, |_| None).is_none());
    }
}
