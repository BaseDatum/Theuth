//! Artifacts and spill over the mesh (68 §7): `theuth/artifacts/1`. A
//! replica holds every transcript but none of the files it names; when a
//! blob (`<data_dir>/artifacts/<sha16>.<ext>`, previews included), a spill
//! file (`<data_dir>/spill/<session>/<call_id>.txt`) or an artifact record is
//! missing locally, [`fetch_blob`] / [`fetch_spill`] / [`fetch_record`] ask
//! every connected peer in turn — any engine serves what it has, the owner
//! always can — and cache the answer under this engine's data dir. No peer
//! has it → "unavailable: owner offline".
//!
//! Wire: `LengthDelimitedCodec` frames of postcard. One [`Req`] per stream,
//! then [`Reply`] frames: `Missing`, or `Chunk`… `End` (files stream in
//! [`CHUNK`]-byte pieces, well under [`MAX_FRAME`]), or one `Record`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use replica::{ALPN_ARTIFACTS, ByteStream, PeerId, Transport};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::store::Stores;

/// Largest frame either side accepts.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// File bytes per `Chunk` frame.
const CHUNK: usize = 4 * 1024 * 1024;
/// Per-peer budget for one fetch.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// One request per stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Req {
    /// A blob by file name under `artifacts/` (`<sha16>.<ext>`, `<sha16>.preview.png`, …).
    Get {
        /// The file name; never a path.
        hash: String,
    },
    /// The spill file of one tool call.
    GetSpill {
        /// The session.
        session: SessionId,
        /// The tool call id (as spilled: `[A-Za-z0-9._-]`).
        call_id: String,
    },
    /// The artifact record of one `show` call.
    Record {
        /// The tool call id.
        call_id: String,
    },
}

/// Answer frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// This engine does not have it.
    Missing,
    /// File bytes, in order.
    Chunk(Vec<u8>),
    /// No more chunks.
    End,
    /// An [`crate::Artifact`] as JSON (paths are the serving engine's).
    Record(Vec<u8>),
}

type Stream = Framed<Box<dyn ByteStream>, LengthDelimitedCodec>;

fn framed(stream: Box<dyn ByteStream>) -> Stream {
    Framed::new(stream, LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME).new_codec())
}

fn encode<T: Serialize>(v: &T) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(postcard::to_allocvec(v)?))
}

/// A bare file name: no separators, not `.`/`..`, non-empty.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\']) && !s.contains('\0')
}

/// The spill file of `call_id` in `session` under `data_dir` (as `agent::spill::cap` names it).
pub fn spill_path(data_dir: &Path, session: SessionId, call_id: &str) -> PathBuf {
    data_dir
        .join("spill")
        .join(session.to_string())
        .join(format!("{}.txt", agent::spill::sanitize(call_id)))
}

/// Serve `theuth/artifacts/1` until shutdown; waits for the mesh transport first.
pub async fn serve(rt: Runtime, stores: Arc<Stores>) {
    let Some(transport) = replica::transport(&rt).await else {
        return;
    };
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_ARTIFACTS) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let (rt, stores) = (rt.clone(), stores.clone());
                tokio::spawn(async move {
                    if let Err(e) = answer(&rt, &stores, framed(stream)).await {
                        tracing::debug!(%peer, error = %e, "artifacts mesh stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "artifacts mesh accept");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

/// One accepted stream: request in, reply frames out.
pub async fn answer(rt: &Runtime, stores: &Stores, mut framed: Stream) -> anyhow::Result<()> {
    let first = framed.next().await.ok_or_else(|| anyhow::anyhow!("closed before the request"))??;
    let req: Req = postcard::from_bytes(&first)?;
    match req {
        Req::Get { hash } => {
            anyhow::ensure!(plain_name(&hash), "bad blob name");
            send_file(&mut framed, &stores.durable().dir.join(hash)).await
        }
        Req::GetSpill { session, call_id } => {
            send_file(&mut framed, &spill_path(&rt.data_dir, session, &call_id)).await
        }
        Req::Record { call_id } => {
            let reply = match stores.durable().get(&call_id).await {
                Ok(a) => Reply::Record(serde_json::to_vec(&a)?),
                Err(_) => Reply::Missing,
            };
            framed.send(encode(&reply)?).await?;
            Ok(())
        }
    }
}

/// `Chunk`… `End`, or `Missing` when `path` does not open.
async fn send_file(framed: &mut Stream, path: &Path) -> anyhow::Result<()> {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        framed.send(encode(&Reply::Missing)?).await?;
        return Ok(());
    };
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        framed.send(encode(&Reply::Chunk(buf[..n].to_vec()))?).await?;
    }
    framed.send(encode(&Reply::End)?).await?;
    Ok(())
}

/// The mesh transport, if the replica plugin has one right now.
async fn transport() -> Option<Arc<dyn Transport>> {
    replica::actor()?.ask(replica::GetTransport).await.ok().flatten()
}

/// `req` to every connected peer in turn; the first `Found` answer wins.
/// `Ok(None)`: no peer has it (or none is connected).
async fn ask_peers(req: &Req, mut on_found: impl FnMut(Reply) -> anyhow::Result<bool>) -> anyhow::Result<bool> {
    let Some(t) = transport().await else { return Ok(false) };
    let req = encode(req)?;
    for peer in t.peers() {
        match tokio::time::timeout(TIMEOUT, ask_one(t.as_ref(), &peer, req.clone(), &mut on_found)).await {
            Ok(Ok(true)) => return Ok(true),
            Ok(Ok(false)) => {}
            Ok(Err(e)) => tracing::debug!(%peer, error = %e, "artifacts fetch"),
            Err(_) => tracing::debug!(%peer, "artifacts fetch timed out"),
        }
    }
    Ok(false)
}

/// One peer: `false` on `Missing`; otherwise feed every reply frame to `on_found`
/// until it says the answer is complete.
async fn ask_one(
    t: &dyn Transport,
    peer: &PeerId,
    req: Bytes,
    on_found: &mut impl FnMut(Reply) -> anyhow::Result<bool>,
) -> anyhow::Result<bool> {
    let mut framed = framed(t.open(peer, ALPN_ARTIFACTS).await?);
    framed.send(req).await?;
    while let Some(frame) = framed.next().await {
        let reply: Reply = postcard::from_bytes(&frame?)?;
        if reply == Reply::Missing {
            return Ok(false);
        }
        if on_found(reply)? {
            return Ok(true);
        }
    }
    anyhow::bail!("peer closed mid-answer")
}

/// Fetch a file answered as `Chunk`… `End` into `dest` (written beside it,
/// renamed into place); `Ok(false)` when no connected peer has it. Each peer
/// attempt starts a fresh temp file, so one that fails mid-stream never
/// leaves its bytes in front of the next peer's.
async fn fetch_file(req: &Req, dest: &Path) -> anyhow::Result<bool> {
    let Some(t) = transport().await else { return Ok(false) };
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = dest.with_extension(format!("part-{}", std::process::id()));
    let req = encode(req)?;
    for peer in t.peers() {
        let outcome = tokio::time::timeout(TIMEOUT, receive_file(t.as_ref(), &peer, req.clone(), &tmp)).await;
        match outcome {
            Ok(Ok(true)) => {
                tokio::fs::rename(&tmp, dest).await?;
                return Ok(true);
            }
            Ok(Ok(false)) => {}
            Ok(Err(e)) => tracing::debug!(%peer, error = %e, "artifacts fetch"),
            Err(_) => tracing::debug!(%peer, "artifacts fetch timed out"),
        }
    }
    let _ = tokio::fs::remove_file(&tmp).await;
    Ok(false)
}

/// One peer's file answer into `tmp` (truncated first); `Ok(true)` once `End` arrived.
async fn receive_file(t: &dyn Transport, peer: &PeerId, req: Bytes, tmp: &Path) -> anyhow::Result<bool> {
    let mut file = std::fs::File::create(tmp)?;
    let mut done = false;
    let found = ask_one(t, peer, req, &mut |reply| match reply {
        Reply::Chunk(bytes) => {
            std::io::Write::write_all(&mut file, &bytes)?;
            Ok(false)
        }
        Reply::End => {
            done = true;
            Ok(true)
        }
        other => anyhow::bail!("unexpected {other:?} for a file request"),
    })
    .await?;
    Ok(found && done)
}

fn offline() -> anyhow::Error {
    anyhow::anyhow!("unavailable: owner offline")
}

/// `dir/name` (a blob under this engine's `artifacts/`): the existing file,
/// or the same bytes fetched from a peer and cached there.
pub async fn fetch_blob(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(plain_name(name), "{name}: not a blob name");
    let dest = dir.join(name);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    if fetch_file(&Req::Get { hash: name.to_owned() }, &dest).await? { Ok(dest) } else { Err(offline()) }
}

/// The spill file of `call_id` in `session`: local, or fetched from a peer
/// and cached at the path `agent::spill::cap` would have written.
pub async fn fetch_spill(data_dir: &Path, session: SessionId, call_id: &str) -> anyhow::Result<PathBuf> {
    let dest = spill_path(data_dir, session, call_id);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    let req = Req::GetSpill {
        session,
        call_id: call_id.to_owned(),
    };
    if fetch_file(&req, &dest).await? { Ok(dest) } else { Err(offline()) }
}

/// The artifact record of `call_id` from a peer, its `path`/`source`/`preview`
/// rewritten to this engine's `artifacts/` (the bytes come later through
/// [`fetch_blob`]). Not cached: the index is engine-local.
pub async fn fetch_record(data_dir: &Path, call_id: &str) -> anyhow::Result<crate::Artifact> {
    let mut found: Option<crate::Artifact> = None;
    let req = Req::Record {
        call_id: call_id.to_owned(),
    };
    ask_peers(&req, |reply| match reply {
        Reply::Record(json) => {
            found = Some(serde_json::from_slice(&json)?);
            Ok(true)
        }
        other => anyhow::bail!("unexpected {other:?} for a record request"),
    })
    .await?;
    let mut a = found.ok_or_else(offline)?;
    let dir = data_dir.join(crate::store::DIR);
    let local = |p: &Path| p.file_name().map(|n| dir.join(n));
    a.path = local(&a.path).unwrap_or(a.path);
    a.source = a.source.as_deref().and_then(local);
    a.preview = a.preview.as_deref().and_then(local);
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use tokio::sync::{broadcast, mpsc};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Loopback pair, as `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<replica::PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let inboxes = Arc::new(Mutex::new(HashMap::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("owner"), mk("replica"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes.lock().keys().filter(|p| **p != self.me).cloned().collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self.inboxes.lock().get(peer).cloned().ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs))).map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<replica::PeerEvent> {
            self.events.subscribe()
        }
    }

    /// The owner's accept loop over `answer`, with a data dir holding one
    /// blob and one spill file. The client side goes through `ask_one`
    /// directly: the production path only adds the transport lookup.
    async fn owner(t: Arc<Loop>, data_dir: PathBuf) -> Arc<Stores> {
        let rt = Runtime::new(figment::Figment::new(), data_dir.clone());
        let state = crate::State::open(&data_dir, rt.clone(), vec![]).await.unwrap();
        let stores = Arc::new(Stores::new(Arc::new(state)));
        let s = stores.clone();
        tokio::spawn(async move {
            while let Ok((_, stream)) = t.accept(ALPN_ARTIFACTS).await {
                let (rt, s) = (rt.clone(), s.clone());
                tokio::spawn(async move { answer(&rt, &s, framed(stream)).await });
            }
        });
        stores
    }

    async fn fetch_via(t: &Loop, peer: &PeerId, req: &Req, dest: &Path) -> anyhow::Result<bool> {
        receive_file(t, peer, encode(req)?, dest).await
    }

    #[tokio::test]
    async fn blob_and_spill_are_fetched_from_a_peer_and_a_missing_one_is_reported() {
        let owner_dir = tempfile::tempdir().unwrap();
        let replica_dir = tempfile::tempdir().unwrap();
        let (ot, rt_) = pair();
        let stores = owner(ot.clone(), owner_dir.path().to_path_buf()).await;
        let blob = (0..(CHUNK + 17)).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        tokio::fs::write(stores.durable().dir.join("abc123.bin"), &blob).await.unwrap();
        let session = SessionId::new();
        let spill = spill_path(owner_dir.path(), session, "call/1");
        tokio::fs::create_dir_all(spill.parent().unwrap()).await.unwrap();
        tokio::fs::write(&spill, "spilled").await.unwrap();

        let dest = replica_dir.path().join("abc123.bin");
        assert!(fetch_via(&rt_, &ot.me, &Req::Get { hash: "abc123.bin".into() }, &dest).await.unwrap());
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), blob, "two chunks reassembled");

        let dest = replica_dir.path().join("spill.txt");
        let req = Req::GetSpill { session, call_id: "call/1".into() };
        assert!(fetch_via(&rt_, &ot.me, &req, &dest).await.unwrap());
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), "spilled");

        let dest = replica_dir.path().join("none.bin");
        assert!(!fetch_via(&rt_, &ot.me, &Req::Get { hash: "none.bin".into() }, &dest).await.unwrap());
        // Path escapes are refused, not served.
        assert!(fetch_via(&rt_, &ot.me, &Req::Get { hash: "../artifacts.sqlite".into() }, &dest).await.is_err());
        assert_eq!(fetch_blob(replica_dir.path(), "x.bin").await.unwrap_err().to_string(), "unavailable: owner offline", "no transport");
    }
}
