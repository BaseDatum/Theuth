//! Artifacts, spill and attachments over the mesh (68 §7): `theuth/artifacts/1`.
//! A replica holds every transcript but none of the files it names; when a
//! blob (`<data_dir>/artifacts/<sha16>.<ext>`, previews included), a spill
//! file (`<data_dir>/spill/<session>/<call_id>.txt`), an attachment
//! (`<data_dir>/attachments/<sha16>.<ext>`) or an artifact record is missing
//! locally, [`fetch_blob`] / [`fetch_spill`] / [`fetch_attachment`] /
//! [`fetch_record`] ask every connected peer in turn — the preferred peer
//! (the session's owner, or the engine that forwarded the call) first; any
//! engine serves what it has — and cache the answer under this engine's data
//! dir. No peer has it → "unavailable: …".
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
use sha2::{Digest as _, Sha256};
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
    /// An attachment by file name under `attachments/` (`<sha16>.<ext>`;
    /// the hash prefix is the identity and is verified by the receiver).
    GetAttachment {
        /// The file name; never a path.
        name: String,
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
    Framed::new(
        stream,
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME)
            .new_codec(),
    )
}

fn encode<T: Serialize>(v: &T) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(postcard::to_allocvec(v)?))
}

/// A bare file name: no separators, not `.`/`..`, non-empty.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\']) && !s.contains('\0')
}

/// The 16 hex digits an attachment or blob name starts with (`<sha16>.<ext>`).
fn hash_prefix(name: &str) -> Option<&str> {
    let (hex, _) = name.split_once('.')?;
    (hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// `<data_dir>/attachments` (as `agent::context::save_blob` names it).
pub fn attachments_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("attachments")
}

/// The store name of `path` when it is an attachment
/// (`…/attachments/<sha16>.<ext>`); anything else → `None`.
pub fn attachment_name(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    (path.parent()?.file_name()? == "attachments" && hash_prefix(name).is_some()).then_some(name)
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
    if let Some(transport) = replica::transport(&rt).await {
        serve_on(rt, transport, stores).await;
    }
}

/// The accept loop of [`serve`] on a given transport.
pub async fn serve_on(rt: Runtime, transport: Arc<dyn Transport>, stores: Arc<Stores>) {
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
    let first = framed
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("closed before the request"))??;
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
        // Only `<data_dir>/attachments/`: a private tree's attachments live
        // under its scratch root (`agent::private::Scratch`) and are never here.
        Req::GetAttachment { name } => {
            anyhow::ensure!(plain_name(&name), "bad attachment name");
            send_file(&mut framed, &attachments_dir(&rt.data_dir).join(name)).await
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
        framed
            .send(encode(&Reply::Chunk(buf[..n].to_vec()))?)
            .await?;
    }
    framed.send(encode(&Reply::End)?).await?;
    Ok(())
}

/// The mesh transport, if the replica plugin has one right now.
async fn transport() -> Option<Arc<dyn Transport>> {
    replica::actor()?
        .ask(replica::GetTransport)
        .await
        .ok()
        .flatten()
}

/// Connected peers, `prefer` first when it is among them.
fn ordered(t: &dyn Transport, prefer: Option<&PeerId>) -> Vec<PeerId> {
    let mut peers = t.peers();
    if let Some(i) = prefer.and_then(|p| peers.iter().position(|q| q == p)) {
        peers.swap(0, i);
    }
    peers
}

/// `req` to every connected peer in turn (`prefer` first); the first `Found`
/// answer wins. `Ok(false)`: no peer has it (or none is connected).
async fn ask_peers(
    t: &dyn Transport,
    prefer: Option<&PeerId>,
    req: &Req,
    mut on_found: impl FnMut(Reply) -> anyhow::Result<bool>,
) -> anyhow::Result<bool> {
    let req = encode(req)?;
    for peer in ordered(t, prefer) {
        match tokio::time::timeout(TIMEOUT, ask_one(t, &peer, req.clone(), &mut on_found)).await {
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
/// leaves its bytes in front of the next peer's. With `expect_sha`, a reply
/// whose sha256 does not start with it is discarded and the next peer asked.
async fn fetch_file(
    t: &dyn Transport,
    prefer: Option<&PeerId>,
    req: &Req,
    dest: &Path,
    expect_sha: Option<&str>,
) -> anyhow::Result<bool> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = dest.with_extension(format!("part-{}", std::process::id()));
    let req = encode(req)?;
    for peer in ordered(t, prefer) {
        let outcome = tokio::time::timeout(
            TIMEOUT,
            receive_file(t, &peer, req.clone(), &tmp, expect_sha),
        )
        .await;
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

/// One peer's file answer into `tmp` (truncated first); `Ok(true)` once `End`
/// arrived and the bytes' sha256 starts with `expect_sha` (when given).
async fn receive_file(
    t: &dyn Transport,
    peer: &PeerId,
    req: Bytes,
    tmp: &Path,
    expect_sha: Option<&str>,
) -> anyhow::Result<bool> {
    let mut file = std::fs::File::create(tmp)?;
    let mut hasher = expect_sha.map(|_| Sha256::new());
    let mut done = false;
    let found = ask_one(t, peer, req, &mut |reply| match reply {
        Reply::Chunk(bytes) => {
            if let Some(h) = &mut hasher {
                h.update(&bytes);
            }
            std::io::Write::write_all(&mut file, &bytes)?;
            Ok(false)
        }
        Reply::End => {
            if let (Some(h), Some(want)) = (hasher.take(), expect_sha) {
                let got = hex::encode(h.finalize());
                anyhow::ensure!(
                    got.starts_with(want),
                    "sha256 {got} does not match the name {want}"
                );
            }
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
/// or the same bytes fetched from a peer (`prefer` first) and cached there.
pub async fn fetch_blob(
    dir: &Path,
    name: &str,
    prefer: Option<&PeerId>,
) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(plain_name(name), "{name}: not a blob name");
    let dest = dir.join(name);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    let t = transport().await.ok_or_else(offline)?;
    let req = Req::Get {
        hash: name.to_owned(),
    };
    if fetch_file(t.as_ref(), prefer, &req, &dest, None).await? {
        Ok(dest)
    } else {
        Err(offline())
    }
}

/// The spill file of `call_id` in `session`: local, or fetched from a peer
/// (`prefer` first) and cached at the path `agent::spill::cap` would have written.
pub async fn fetch_spill(
    data_dir: &Path,
    session: SessionId,
    call_id: &str,
    prefer: Option<&PeerId>,
) -> anyhow::Result<PathBuf> {
    let dest = spill_path(data_dir, session, call_id);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    let t = transport().await.ok_or_else(offline)?;
    let req = Req::GetSpill {
        session,
        call_id: call_id.to_owned(),
    };
    if fetch_file(t.as_ref(), prefer, &req, &dest, None).await? {
        Ok(dest)
    } else {
        Err(offline())
    }
}

/// `dir/name` (an attachment `<sha16>.<ext>` under this engine's
/// `attachments/`): the existing file, or the same bytes fetched from a peer
/// (`prefer` first), verified against the name's sha256 prefix, and cached there.
pub async fn fetch_attachment(
    dir: &Path,
    name: &str,
    prefer: Option<&PeerId>,
) -> anyhow::Result<PathBuf> {
    let dest = dir.join(name);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    let t = transport().await.ok_or_else(nobody)?;
    fetch_attachment_via(t.as_ref(), dir, name, prefer).await
}

/// [`fetch_attachment`] over a given transport (the mesh's, or a test's).
pub async fn fetch_attachment_via(
    t: &dyn Transport,
    dir: &Path,
    name: &str,
    prefer: Option<&PeerId>,
) -> anyhow::Result<PathBuf> {
    let sha = hash_prefix(name)
        .filter(|_| plain_name(name))
        .ok_or_else(|| anyhow::anyhow!("{name}: not an attachment name"))?;
    let dest = dir.join(name);
    if tokio::fs::try_exists(&dest).await? {
        return Ok(dest);
    }
    let req = Req::GetAttachment {
        name: name.to_owned(),
    };
    if fetch_file(t, prefer, &req, &dest, Some(sha)).await? {
        Ok(dest)
    } else {
        Err(nobody())
    }
}

fn nobody() -> anyhow::Error {
    anyhow::anyhow!("unavailable: no peer has it")
}

/// The artifact record of `call_id` from a peer (`prefer` first), its
/// `path`/`source`/`preview` rewritten to this engine's `artifacts/` (the
/// bytes come later through [`fetch_blob`]). Not cached: the index is engine-local.
pub async fn fetch_record(
    data_dir: &Path,
    call_id: &str,
    prefer: Option<&PeerId>,
) -> anyhow::Result<crate::Artifact> {
    let t = transport().await.ok_or_else(offline)?;
    let mut found: Option<crate::Artifact> = None;
    let req = Req::Record {
        call_id: call_id.to_owned(),
    };
    ask_peers(t.as_ref(), prefer, &req, |reply| match reply {
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

    /// Loopback switch, as `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<replica::PeerEvent>,
        /// Streams accepted here.
        hits: std::sync::atomic::AtomicUsize,
    }

    fn nodes(names: &[&str]) -> Vec<Arc<Loop>> {
        let inboxes = Arc::new(Mutex::new(HashMap::new()));
        names
            .iter()
            .map(|name| {
                let me = PeerId((*name).to_owned());
                let (tx, rx) = mpsc::unbounded_channel();
                inboxes.lock().insert(me.clone(), tx);
                Arc::new(Loop {
                    me,
                    inboxes: inboxes.clone(),
                    inbox: tokio::sync::Mutex::new(rx),
                    events: broadcast::channel(8).0,
                    hits: Default::default(),
                })
            })
            .collect()
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let mut v = nodes(&["owner", "replica"]);
        let replica = v.pop().unwrap();
        (v.pop().unwrap(), replica)
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<replica::PeerEvent> {
            self.events.subscribe()
        }
    }

    /// An engine's accept loop over `answer` on `data_dir`.
    async fn owner(t: Arc<Loop>, data_dir: PathBuf) -> Arc<Stores> {
        let rt = Runtime::new(figment::Figment::new(), data_dir.clone());
        let state = crate::State::open(&data_dir, rt.clone(), vec![])
            .await
            .unwrap();
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
        receive_file(t, peer, encode(req)?, dest, None).await
    }

    fn sha_name(bytes: &[u8], ext: &str) -> String {
        format!("{}.{ext}", &hex::encode(Sha256::digest(bytes))[..16])
    }

    #[tokio::test]
    async fn blob_and_spill_are_fetched_from_a_peer_and_a_missing_one_is_reported() {
        let owner_dir = tempfile::tempdir().unwrap();
        let replica_dir = tempfile::tempdir().unwrap();
        let (ot, rt_) = pair();
        let stores = owner(ot.clone(), owner_dir.path().to_path_buf()).await;
        let blob = (0..(CHUNK + 17))
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        tokio::fs::write(stores.durable().dir.join("abc123.bin"), &blob)
            .await
            .unwrap();
        let session = SessionId::new();
        let spill = spill_path(owner_dir.path(), session, "call/1");
        tokio::fs::create_dir_all(spill.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&spill, "spilled").await.unwrap();

        let dest = replica_dir.path().join("abc123.bin");
        assert!(
            fetch_via(
                &rt_,
                &ot.me,
                &Req::Get {
                    hash: "abc123.bin".into()
                },
                &dest
            )
            .await
            .unwrap()
        );
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            blob,
            "two chunks reassembled"
        );

        let dest = replica_dir.path().join("spill.txt");
        let req = Req::GetSpill {
            session,
            call_id: "call/1".into(),
        };
        assert!(fetch_via(&rt_, &ot.me, &req, &dest).await.unwrap());
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), "spilled");

        let dest = replica_dir.path().join("none.bin");
        assert!(
            !fetch_via(
                &rt_,
                &ot.me,
                &Req::Get {
                    hash: "none.bin".into()
                },
                &dest
            )
            .await
            .unwrap()
        );
        // Path escapes are refused, not served.
        assert!(
            fetch_via(
                &rt_,
                &ot.me,
                &Req::Get {
                    hash: "../artifacts.sqlite".into()
                },
                &dest
            )
            .await
            .is_err()
        );
        assert_eq!(
            fetch_blob(replica_dir.path(), "x.bin", None)
                .await
                .unwrap_err()
                .to_string(),
            "unavailable: owner offline",
            "no transport"
        );
    }

    #[tokio::test]
    async fn attachment_is_fetched_verified_and_cached_and_a_corrupt_reply_is_rejected() {
        let owner_dir = tempfile::tempdir().unwrap();
        let replica_dir = tempfile::tempdir().unwrap();
        let (ot, rt_) = pair();
        owner(ot.clone(), owner_dir.path().to_path_buf()).await;
        let good = b"\x89PNG good bytes".to_vec();
        let name = sha_name(&good, "png");
        let src = attachments_dir(owner_dir.path());
        tokio::fs::create_dir_all(&src).await.unwrap();
        tokio::fs::write(src.join(&name), &good).await.unwrap();
        // A file whose bytes do not hash to its name (tampered or truncated on the peer).
        let bad = sha_name(b"what the name promises", "png");
        tokio::fs::write(src.join(&bad), b"something else")
            .await
            .unwrap();

        let dir = attachments_dir(replica_dir.path());
        let dest = dir.join(&name);
        let req = Req::GetAttachment { name: name.clone() };
        assert!(
            fetch_file(rt_.as_ref(), None, &req, &dest, hash_prefix(&name))
                .await
                .unwrap()
        );
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            good,
            "cached under attachments/ by name"
        );

        let dest = dir.join(&bad);
        let req = Req::GetAttachment { name: bad.clone() };
        assert!(
            !fetch_file(rt_.as_ref(), None, &req, &dest, hash_prefix(&bad))
                .await
                .unwrap(),
            "hash mismatch → not found"
        );
        assert!(!dest.exists(), "nothing cached");
        assert!(
            !dest
                .with_extension(format!("part-{}", std::process::id()))
                .exists(),
            "temp removed"
        );

        assert!(
            fetch_via(
                &rt_,
                &ot.me,
                &Req::GetAttachment {
                    name: "../x.png".into()
                },
                &dest
            )
            .await
            .is_err(),
            "escape refused"
        );
        assert!(hash_prefix("not-a-hash.png").is_none());
        assert_eq!(
            fetch_attachment(&dir, "0123456789abcdef.png", None)
                .await
                .unwrap_err()
                .to_string(),
            "unavailable: no peer has it",
            "no transport"
        );
    }

    #[tokio::test]
    async fn the_preferred_peer_is_asked_first() {
        let mut v = nodes(&["replica", "owner", "other"]);
        let (replica, owner_t, other) = (v.remove(0), v.remove(0), v.remove(0));
        let owner_dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let replica_dir = tempfile::tempdir().unwrap();
        owner(owner_t.clone(), owner_dir.path().to_path_buf()).await;
        owner(other.clone(), other_dir.path().to_path_buf()).await;
        let bytes = b"owner has it".to_vec();
        let name = sha_name(&bytes, "jpg");
        let src = attachments_dir(owner_dir.path());
        tokio::fs::create_dir_all(&src).await.unwrap();
        tokio::fs::write(src.join(&name), &bytes).await.unwrap();

        assert_eq!(ordered(replica.as_ref(), Some(&owner_t.me))[0], owner_t.me);
        assert_eq!(ordered(replica.as_ref(), Some(&other.me))[0], other.me);
        assert_eq!(
            ordered(replica.as_ref(), Some(&PeerId("stranger".into()))).len(),
            2,
            "an unknown owner changes nothing"
        );

        let dest = attachments_dir(replica_dir.path()).join(&name);
        let req = Req::GetAttachment { name: name.clone() };
        assert!(
            fetch_file(
                replica.as_ref(),
                Some(&owner_t.me),
                &req,
                &dest,
                hash_prefix(&name)
            )
            .await
            .unwrap()
        );
        // The accept loop's first `accept` is the pending one; a served stream is a second.
        assert_eq!(
            other.hits.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "other was never asked"
        );
        assert_eq!(owner_t.hits.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
}
