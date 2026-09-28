//! Wire and clock types of small-record replication (67 §5.1). Only
//! [`PeerId`] and [`PeerEvent`] are exported to TypeScript (they appear in
//! `mesh::MeshEvent`); `Change` stays Rust-only, `knowledge::Change` already
//! owns that binding name.

use std::fmt;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;

/// A mesh engine id (`mesh::EngineId`) as the replication layer sees it: an
/// opaque string, the iroh endpoint id in text form.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct PeerId(pub String);

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Hybrid logical clock stamp. Ordered by `wall_ms`, then `counter`, then
/// `origin` (the derived `Ord` follows field order), so two engines never
/// tie: the larger stamp wins a per-column LWW merge.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Hlc {
    /// Physical time in ms since the epoch, as seen by `origin`.
    pub wall_ms: u64,
    /// Ticks within one `wall_ms`.
    pub counter: u32,
    /// Engine that issued the stamp; the tie-break.
    pub origin: PeerId,
}

impl Hlc {
    /// Sortable text form stored in the `hlc` column and the clock tables:
    /// `<wall_ms hex16>-<counter hex8>-<origin>`; lexicographic order equals
    /// the derived `Ord`.
    pub fn to_text(&self) -> String {
        format!("{:016x}-{:08x}-{}", self.wall_ms, self.counter, self.origin)
    }

    /// Inverse of [`Hlc::to_text`]; `None` for anything else (a plugin that
    /// wrote its own value into `hlc`, NULL, an old format).
    pub fn parse(text: &str) -> Option<Self> {
        let (wall, rest) = text.split_once('-')?;
        let (counter, origin) = rest.split_once('-')?;
        if wall.len() != 16 || counter.len() != 8 || origin.is_empty() {
            return None;
        }
        Some(Self {
            wall_ms: u64::from_str_radix(wall, 16).ok()?,
            counter: u32::from_str_radix(counter, 16).ok()?,
            origin: PeerId(origin.to_owned()),
        })
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

/// One column (or whole-row) write as the change feed carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Change {
    /// Replicated table name in the owning plugin's sqlite file.
    pub table: String,
    /// Primary key as text (uuid, name, …).
    pub pk: String,
    /// Column written; `None` = the whole row (insert or delete).
    pub column: Option<String>,
    /// New value, serialized by the owner; `None` with `deleted: false` = NULL.
    pub value: Option<Vec<u8>>,
    /// When it was written.
    pub hlc: Hlc,
    /// Row tombstone.
    pub deleted: bool,
}

/// A run of changes from one origin, in `seq` order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChangeBatch {
    /// Engine whose feed this is.
    pub origin: PeerId,
    /// Position in `origin`'s feed; a receiver acks by `seq` (`sync_state`).
    pub seq: u64,
    /// The changes.
    pub changes: Vec<Change>,
}

/// Connectivity as the transport reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "peer", rename_all = "snake_case")]
pub enum PeerEvent {
    /// A QUIC connection to `peer` is up.
    Connected(PeerId),
    /// The connection to `peer` is gone.
    Disconnected(PeerId),
}

/// A bidirectional byte stream over the mesh (one QUIC stream).
pub trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> ByteStream for T {}

/// What the mesh gives replication (and every other ALPN user): who is
/// reachable, a stream to one of them, and connect/disconnect events. The
/// mesh worker implements it on the iroh endpoint and registers it with
/// [`crate::RegisterTransport`].
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Peers currently connected.
    fn peers(&self) -> Vec<PeerId>;
    /// Open a stream to `peer` on `alpn` (one of the `ALPN_*` constants).
    async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>>;
    /// The next stream a peer opened to us on `alpn`, with who opened it.
    async fn accept(&self, alpn: &str) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)>;
    /// Connect/disconnect events; a lagging receiver sees `Lagged`, never a stop.
    fn subscribe(&self) -> broadcast::Receiver<PeerEvent>;
}
