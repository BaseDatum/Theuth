//! The visitor stream preamble (§5.3): the edge writes a PROXY protocol v2 header (built with `ppp`) carrying the
//! visitor's address and port, the SNI in `PP2_TYPE_AUTHORITY`, and the edge's connection id in custom TLV `0xE0`;
//! then raw TLS both ways. Stream FIN = TCP FIN, reset = RST.

/// `PP2_TYPE_AUTHORITY`: the ClientHello's SNI, UTF-8 (PROXY v2 spec §2.2.1).
pub const TLV_AUTHORITY: u8 = 0x02;

/// Custom TLV (`PP2_TYPE_MIN_CUSTOM`..): the edge's visitor connection id, [`CONN_ID_LEN`] bytes big-endian.
pub const TLV_CONN_ID: u8 = 0xE0;

/// Length of the [`TLV_CONN_ID`] value.
pub const CONN_ID_LEN: usize = 8;

/// The [`TLV_CONN_ID`] value for `id`.
pub fn conn_id_bytes(id: u64) -> [u8; CONN_ID_LEN] {
    id.to_be_bytes()
}

/// Reads a [`TLV_CONN_ID`] value; `None` unless exactly 8 bytes.
pub fn conn_id_from(value: &[u8]) -> Option<u64> {
    value.try_into().ok().map(u64::from_be_bytes)
}

/// The edge never reads more than this much ClientHello before routing (§4.2).
pub const CLIENT_HELLO_MAX: usize = 16 * 1024;

/// … nor waits longer than this, in ms (§4.2).
pub const CLIENT_HELLO_TIMEOUT_MS: u64 = 5_000;

/// ALPN the edge resets on until P3's custom domains (§6.7).
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

/// ALPNs a hosting engine offers to visitors (§4.2).
pub const VISITOR_ALPNS: &[&[u8]] = &[b"h2", b"http/1.1"];

#[cfg(test)]
mod tests {
    #[test]
    fn conn_id() {
        assert_eq!(
            super::conn_id_from(&super::conn_id_bytes(0xdead_beef)),
            Some(0xdead_beef)
        );
        assert_eq!(super::conn_id_from(&[1, 2, 3]), None);
    }
}
