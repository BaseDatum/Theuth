//! A remote child over the mesh (67 §4.2, decision 4): the ACP a node serves
//! over a link mode-1 conn, carried on a `theuth/acp/1` QUIC stream instead.
//! One stream per child; newline-delimited JSON-RPC, as on stdio. Both ends
//! frame with [`line_duplex`]: the dialer here, the accept loop in `rpc::acp`.

use std::fmt;
use std::io;
use std::sync::Arc;

use replica::{ALPN_ACP, ByteStream, PeerId, Transport};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::harness::{Dialer, LineDuplex};

/// Dials one mesh engine: `Transport::open(peer, ALPN_ACP)`.
pub struct MeshDialer {
    /// Display name for error text (the engine's `mesh.name`).
    pub name: String,
    /// The engine.
    pub peer: PeerId,
    /// This engine's id, for `_meta.theuth.parent_engine` (display only).
    pub self_id: String,
    /// The mesh.
    pub transport: Arc<dyn Transport>,
}

impl fmt::Debug for MeshDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshDialer({} {})", self.name, self.peer)
    }
}

#[async_trait::async_trait]
impl Dialer for MeshDialer {
    async fn dial(&self) -> Result<LineDuplex, String> {
        if !self.transport.peers().contains(&self.peer) {
            return Err(format!("{} is not reachable over the mesh", self.name));
        }
        let stream = self
            .transport
            .open(&self.peer, ALPN_ACP)
            .await
            .map_err(|e| format!("{}: {e:#}", self.name))?;
        Ok(line_duplex(stream))
    }

    fn caller_id(&self) -> Option<String> {
        Some(self.self_id.get(..8).unwrap_or(&self.self_id).to_owned())
    }

    fn engine_id(&self) -> Option<&str> {
        Some(&self.peer.0)
    }
}

/// One JSON-RPC message per line on a mesh stream, as [`LineDuplex`] wants
/// it: the record is the line, the newline stays on the wire.
#[must_use]
pub fn line_duplex(stream: Box<dyn ByteStream>) -> LineDuplex {
    let (read, write) = tokio::io::split(stream);
    let incoming = futures::stream::unfold(BufReader::new(read).lines(), async |mut lines| {
        match lines.next_line().await {
            Ok(Some(line)) => Some((Ok(line), lines)),
            Ok(None) => None,
            Err(e) => Some((Err(e), lines)),
        }
    });
    let outgoing = futures::sink::unfold(write, async |mut w, mut line: String| {
        line.push('\n');
        w.write_all(line.as_bytes()).await?;
        w.flush().await?;
        Ok::<_, io::Error>(w)
    });
    LineDuplex {
        outgoing: Box::pin(outgoing),
        incoming: Box::pin(incoming),
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};

    use super::*;

    /// Lines out, lines in, over one in-memory duplex; EOF ends the stream.
    #[tokio::test]
    async fn frames_one_message_per_line() {
        let (a, b) = tokio::io::duplex(1024);
        let mut left = line_duplex(Box::new(a));
        let mut right = line_duplex(Box::new(b));
        left.outgoing.send(r#"{"jsonrpc":"2.0","id":1}"#.into()).await.unwrap();
        assert_eq!(
            right.incoming.next().await.unwrap().unwrap(),
            r#"{"jsonrpc":"2.0","id":1}"#
        );
        right.outgoing.send("reply".into()).await.unwrap();
        assert_eq!(left.incoming.next().await.unwrap().unwrap(), "reply");
        drop(left);
        assert!(right.incoming.next().await.is_none());
    }
}
