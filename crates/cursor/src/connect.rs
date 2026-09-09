//! Connect-RPC envelope framing: `flags:u8 | len:u32be | payload`. Flag
//! [`END_STREAM`] marks the trailing JSON `{error:{code,message}}` (or `{}`)
//! frame of a streaming response. Compression is never negotiated.

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// End-of-stream flag; payload is JSON, not protobuf.
pub const END_STREAM: u8 = 0b10;
const HEADER: usize = 5;

/// One framed message.
pub fn frame(flags: u8, payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(HEADER + payload.len());
    out.put_u8(flags);
    out.put_u32(payload.len() as u32);
    out.put_slice(payload);
    out.freeze()
}

/// Incremental frame splitter over arbitrary chunk boundaries.
#[derive(Default)]
pub struct Decoder {
    buf: BytesMut,
}

impl Decoder {
    /// Append bytes from the wire.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Next complete `(flags, payload)`, if buffered.
    pub fn next(&mut self) -> Option<(u8, Bytes)> {
        if self.buf.len() < HEADER {
            return None;
        }
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < HEADER + len {
            return None;
        }
        let flags = self.buf[0];
        self.buf.advance(HEADER);
        Some((flags, self.buf.split_to(len).freeze()))
    }
}

/// Error text carried by an end-stream (`{error:{code,message}}`) or unary
/// (`{code,message}`) JSON body; `None` when the body reports success.
pub fn error_text(json: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let e = v.get("error").unwrap_or(&v);
    let code = e.get("code")?.as_str()?;
    let message = e.get("message").and_then(|m| m.as_str()).unwrap_or("");
    Some(format!("Connect error {code}: {message}"))
}

/// Unary body: the first non-end-stream frame when the body is framed, else
/// the body itself (Cursor answers `application/proto` both ways).
pub fn unary_payload(body: &[u8]) -> &[u8] {
    let mut rest = body;
    while rest.len() >= HEADER {
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if rest[0] & !END_STREAM != 0 || rest.len() < HEADER + len {
            break;
        }
        if rest[0] & END_STREAM == 0 {
            return &rest[HEADER..HEADER + len];
        }
        rest = &rest[HEADER + len..];
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_across_chunk_boundaries() {
        let a = frame(0, b"hello");
        let b = frame(
            END_STREAM,
            br#"{"error":{"code":"unauthenticated","message":"bad token"}}"#,
        );
        let wire = [a.as_ref(), b.as_ref()].concat();
        let mut d = Decoder::default();
        d.push(&wire[..3]);
        assert!(d.next().is_none());
        d.push(&wire[3..12]);
        assert_eq!(d.next(), Some((0, Bytes::from_static(b"hello"))));
        assert!(d.next().is_none());
        d.push(&wire[12..]);
        let (flags, payload) = d.next().unwrap();
        assert_eq!(flags, END_STREAM);
        assert_eq!(
            error_text(&payload).as_deref(),
            Some("Connect error unauthenticated: bad token")
        );
        assert!(error_text(b"{}").is_none());
        assert_eq!(
            error_text(br#"{"code":"not_found","message":"x"}"#).as_deref(),
            Some("Connect error not_found: x")
        );
    }

    #[test]
    fn unary_payload_accepts_raw_and_framed() {
        let raw = [0x0a, 0x03, b'a', b'b', b'c'];
        assert_eq!(unary_payload(&raw), &raw);
        let framed = [frame(0, &raw).as_ref(), frame(END_STREAM, b"{}").as_ref()].concat();
        assert_eq!(unary_payload(&framed), &raw);
    }
}
