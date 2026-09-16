//! Relay-layer frame codec (design doc 31 §4.2).
//!
//! Uplink frames are `op:u8 ‖ conn:u32 LE ‖ payload`; device frames are
//! `op:u8 ‖ payload`. Decoding is in place: the header is read from the
//! received [`Bytes`] and the payload is a refcount-bumping slice.

use bytes::{BufMut, Bytes, BytesMut};

/// Registration handshake (uplink only, `conn = 0`).
pub const HELLO: u8 = 0x00;
/// A device conn opened (uplink only, relay → harness).
pub const OPEN: u8 = 0x01;
/// One channel-layer message.
pub const DATA: u8 = 0x02;
/// Conn closed; payload = reason byte.
pub const CLOSE: u8 = 0x03;
/// Harness presence (device only, relay → device); payload = `online:u8`.
pub const PRESENCE: u8 = 0x04;

/// Largest payload either side may carry.
pub const MAX_PAYLOAD: usize = 65_535;
/// Largest WebSocket message: header (5) + [`MAX_PAYLOAD`].
pub const MAX_MESSAGE: usize = MAX_PAYLOAD + 5;

/// CLOSE reasons.
pub mod reason {
    /// Peer closed normally.
    pub const NORMAL: u8 = 0;
    /// Outbound queue overflowed.
    pub const QUEUE_FULL: u8 = 1;
    /// Device socket idle 10 min.
    pub const IDLE: u8 = 2;
    /// Refused: DATA before OPEN, conn limit, or the harness rejected it.
    pub const REFUSED: u8 = 3;
    /// The harness uplink went away.
    pub const HARNESS_GONE: u8 = 4;
    /// Number of reasons (for counters).
    pub const COUNT: usize = 5;
}

/// A parsed uplink frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UplinkFrame {
    /// Operation byte.
    pub op: u8,
    /// Conn id (0 for HELLO).
    pub conn: u32,
    /// Payload after the 5-byte header.
    pub payload: Bytes,
}

/// A parsed device frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFrame {
    /// Operation byte.
    pub op: u8,
    /// Payload after the 1-byte header.
    pub payload: Bytes,
}

/// Parse an uplink frame; `None` when shorter than the header.
pub fn decode_uplink(msg: &Bytes) -> Option<UplinkFrame> {
    if msg.len() < 5 {
        return None;
    }
    let conn = u32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
    Some(UplinkFrame {
        op: msg[0],
        conn,
        payload: msg.slice(5..),
    })
}

/// Parse a device frame; `None` when empty.
pub fn decode_device(msg: &Bytes) -> Option<DeviceFrame> {
    if msg.is_empty() {
        return None;
    }
    Some(DeviceFrame {
        op: msg[0],
        payload: msg.slice(1..),
    })
}

/// Encode an uplink frame in one allocation.
pub fn encode_uplink(op: u8, conn: u32, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(5 + payload.len());
    b.put_u8(op);
    b.put_u32_le(conn);
    b.put_slice(payload);
    b.freeze()
}

/// Encode a device frame in one allocation.
pub fn encode_device(op: u8, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(1 + payload.len());
    b.put_u8(op);
    b.put_slice(payload);
    b.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uplink_roundtrip() {
        let f = encode_uplink(DATA, 0x0102_0304, b"hi");
        assert_eq!(&f[..], &[0x02, 0x04, 0x03, 0x02, 0x01, b'h', b'i']);
        let d = decode_uplink(&f).unwrap();
        assert_eq!(
            d,
            UplinkFrame {
                op: DATA,
                conn: 0x0102_0304,
                payload: Bytes::from_static(b"hi")
            }
        );
        let c = decode_uplink(&encode_uplink(CLOSE, 7, &[reason::IDLE])).unwrap();
        assert_eq!((c.op, c.conn, &c.payload[..]), (CLOSE, 7, &[2u8][..]));
    }

    #[test]
    fn uplink_short_rejected() {
        assert!(decode_uplink(&Bytes::from_static(&[0, 0, 0, 0])).is_none());
        let empty = decode_uplink(&Bytes::from_static(&[OPEN, 1, 0, 0, 0])).unwrap();
        assert_eq!((empty.op, empty.conn, empty.payload.len()), (OPEN, 1, 0));
    }

    #[test]
    fn device_roundtrip() {
        let f = encode_device(PRESENCE, &[1]);
        assert_eq!(&f[..], &[0x04, 1]);
        let d = decode_device(&f).unwrap();
        assert_eq!(
            d,
            DeviceFrame {
                op: PRESENCE,
                payload: Bytes::from_static(&[1])
            }
        );
        assert!(decode_device(&Bytes::new()).is_none());
        let data = decode_device(&encode_device(DATA, b"")).unwrap();
        assert_eq!((data.op, data.payload.len()), (DATA, 0));
    }
}
