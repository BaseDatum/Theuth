//! 176 R2: one `rpc: ws open` and one `rpc: ws closed` INFO line per local WebSocket.
//! jsonrpsee keeps the close frame to itself, so [`Watched`] reads the client's close code
//! off the bytes it sends and keeps the socket's first I/O error.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// Ids of accepted connections, process-wide: the `conn=` of the log lines.
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// One accepted TCP connection as its log lines see it.
pub(crate) struct Conn {
    pub(crate) id: u64,
    addr: SocketAddr,
    accepted: Instant,
    /// The `client=` tag (`-` when absent or malformed), set when the connection
    /// upgrades to a WebSocket; a plain HTTP call logs nothing.
    ws: OnceLock<Box<str>>,
    /// The code of the client's close frame (1005 when it carried none).
    close: OnceLock<u16>,
    /// The socket's first I/O error.
    error: OnceLock<io::ErrorKind>,
}

impl Conn {
    pub(crate) fn new(addr: SocketAddr) -> Self {
        Self {
            id: NEXT_CONN.fetch_add(1, Ordering::Relaxed),
            addr,
            accepted: Instant::now(),
            ws: OnceLock::new(),
            close: OnceLock::new(),
            error: OnceLock::new(),
        }
    }

    /// The open line, once, on the first admitted upgrade request; `query` is the URL's,
    /// whose `client=` (176 C3) it names. The token is never logged.
    pub(crate) fn log_open(&self, query: Option<&str>) {
        let client = query
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("client=")))
            .and_then(crate::diag::tag)
            .unwrap_or("-");
        if self.ws.set(client.into()).is_ok() {
            tracing::info!(
                "rpc: ws open conn={} client={client} peer={}",
                self.id,
                self.addr
            );
        }
    }

    /// The close line: the client's close code, else the first I/O error, else
    /// `shutdown` (server stop) or `eof`.
    pub(crate) fn log_closed(&self, stopped: bool) {
        let Some(client) = self.ws.get() else {
            return;
        };
        let (id, after) = (self.id, self.accepted.elapsed().as_secs_f64());
        let reason = match (self.close.get(), self.error.get()) {
            (Some(code), _) => format!("close({code})"),
            (None, Some(kind)) => format!("error({kind:?})"),
            (None, None) => (if stopped { "shutdown" } else { "eof" }).to_owned(),
        };
        tracing::info!(
            "rpc: ws closed conn={id} client={client} after={after:.1}s reason={reason}"
        );
    }
}

/// Just enough of RFC 6455 §5.2 to find the close frame in what a client sends: frame
/// headers are collected byte by byte (≤ 14), payloads skipped whole.
#[derive(Default)]
struct Frames {
    head: [u8; 14],
    have: usize,
    /// Payload bytes of the current frame still to pass.
    skip: u64,
    /// In a close frame: its mask and the code bytes unmasked so far.
    close: Option<([u8; 4], [u8; 2], usize)>,
}

impl Frames {
    /// Feed client bytes; the close code once the close frame's first two payload bytes
    /// are in (1005 for a close frame without a payload).
    fn feed(&mut self, mut buf: &[u8]) -> Option<u16> {
        while !buf.is_empty() {
            if self.skip > 0 {
                let n = usize::try_from(self.skip).map_or(buf.len(), |s| s.min(buf.len()));
                if let Some((mask, code, got)) = &mut self.close {
                    for &b in &buf[..n.min(2 - *got)] {
                        code[*got] = b ^ mask[*got];
                        *got += 1;
                    }
                    if *got == 2 {
                        return Some(u16::from_be_bytes(*code));
                    }
                }
                self.skip -= n as u64;
                buf = &buf[n..];
                continue;
            }
            self.head[self.have] = buf[0];
            self.have += 1;
            buf = &buf[1..];
            let h = &self.head[..self.have];
            if h.len() < 2 {
                continue;
            }
            let ext = match h[1] & 0x7f {
                126 => 2,
                127 => 8,
                _ => 0,
            };
            let masked = h[1] & 0x80 != 0;
            let need = 2 + ext + if masked { 4 } else { 0 };
            if h.len() < need {
                continue;
            }
            let len = match ext {
                0 => u64::from(h[1] & 0x7f),
                _ => h[2..2 + ext].iter().fold(0, |a, &b| a << 8 | u64::from(b)),
            };
            let mut mask = [0; 4];
            if masked {
                mask.copy_from_slice(&h[need - 4..need]);
            }
            if h[0] & 0x0f == 0x8 {
                if len < 2 {
                    return Some(1005);
                }
                self.close = Some((mask, [0; 2], 0));
            }
            self.skip = len;
            self.have = 0;
        }
        None
    }
}

/// The TCP stream, feeding [`Conn`] its close code and first I/O error.
pub(crate) struct Watched {
    io: TcpStream,
    conn: Arc<Conn>,
    frames: Frames,
}

impl Watched {
    pub(crate) fn new(io: TcpStream, conn: Arc<Conn>) -> Self {
        Self {
            io,
            conn,
            frames: Frames::default(),
        }
    }

    fn note<T>(&self, r: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if let Poll::Ready(Err(e)) = &r {
            let _ = self.conn.error.set(e.kind());
        }
        r
    }
}

impl AsyncRead for Watched {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let r = Pin::new(&mut this.io).poll_read(cx, buf);
        // Bytes after the upgrade are frames; the HTTP request before it is not.
        if matches!(r, Poll::Ready(Ok(())))
            && this.conn.ws.get().is_some()
            && this.conn.close.get().is_none()
            && let Some(code) = this.frames.feed(&buf.filled()[before..])
        {
            let _ = this.conn.close.set(code);
        }
        this.note(r)
    }
}

impl AsyncWrite for Watched {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.io).poll_write(cx, buf);
        this.note(r)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.io).poll_write_vectored(cx, bufs);
        this.note(r)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.io).poll_flush(cx);
        this.note(r)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.io).poll_shutdown(cx);
        this.note(r)
    }
}

#[cfg(test)]
mod tests {
    use super::Frames;

    /// A masked client frame (`opcode`, `payload`), as a browser sends it.
    fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut f = vec![0x80 | opcode];
        match payload.len() {
            n @ 0..=125 => f.push(0x80 | n as u8),
            n @ 126..=0xffff => {
                f.push(0x80 | 126);
                f.extend((n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend((n as u64).to_be_bytes());
            }
        }
        f.extend(mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn finds_the_close_code_behind_data_frames_split_anywhere() {
        let mut close = 4001u16.to_be_bytes().to_vec();
        close.extend(b"reset");
        let mut bytes = frame(0x1, b"{\"method\":\"ping\"}");
        bytes.extend(frame(0x1, &[b'x'; 300]));
        bytes.extend(frame(0x2, &vec![0x88; 70_000]));
        bytes.extend(frame(0x8, &close));
        for chunk in [1, 2, 7, 131, bytes.len()] {
            let mut f = Frames::default();
            let mut got = None;
            for c in bytes.chunks(chunk) {
                got = got.or(f.feed(c));
            }
            assert_eq!(got, Some(4001), "chunks of {chunk}");
        }
        assert_eq!(Frames::default().feed(&frame(0x8, &[])), Some(1005));
        assert_eq!(Frames::default().feed(&frame(0x1, b"no close")), None);
    }
}
