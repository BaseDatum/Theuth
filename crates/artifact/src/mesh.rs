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

#[cfg(unix)]
use agent::communication::files::disk;
use agent::communication::files::{self as contact, ReceivedNotice};
#[cfg(unix)]
use agent::communication::files::{HeldContactFile, HeldFile};
#[cfg(unix)]
use kameo::actor::ActorRef;

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
    /// 199 P2: a received contact file by its holder's local id (32 lowercase hex). Only
    /// a `received` `in` row is served, to a paired machine.
    GetContactFile {
        /// The holder's local id; never a path.
        id: String,
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
        // 199 P2: only a `received` `in` row, through `Agents`. The mesh transport carries
        // paired machines only (a stream of an unpaired one fails, `mesh::net`).
        #[cfg(unix)]
        Req::GetContactFile { id } => {
            send_contact_file(&mut framed, &rt.data_dir, held(id).await).await
        }
        #[cfg(not(unix))]
        Req::GetContactFile { .. } => contact_missing(&mut framed).await,
    }
}

/// 199 P2 (SEC-N2): a contact file's `Missing` answer, on every platform within
/// [`contact::FILE_IDLE`] like every other write of that path.
async fn contact_missing(framed: &mut Stream) -> anyhow::Result<()> {
    tokio::time::timeout(contact::FILE_IDLE, framed.send(encode(&Reply::Missing)?))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the fetcher took nothing for {} s",
                contact::FILE_IDLE.as_secs()
            )
        })??;
    Ok(())
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

/// 199 P2: the holder's `received` row `id`; `None` without the supervisor.
#[cfg(unix)]
async fn held(id: String) -> Option<HeldFile> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()?;
    agents.ask(HeldContactFile { id }).await.ok().flatten()
}

/// 199 P2: a held file as `Chunk`… `End`, exactly its recorded size, or `Missing` when
/// there is none or its path no longer opens through the gate as the recorded
/// `(dev, inode)` and size. D8: each frame must leave within [`contact::FILE_IDLE`];
/// there is no whole-transfer budget.
#[cfg(unix)]
async fn send_contact_file(
    framed: &mut Stream,
    root: &Path,
    held: Option<HeldFile>,
) -> anyhow::Result<()> {
    use std::os::unix::fs::FileExt as _;
    let idle = |_| {
        anyhow::anyhow!(
            "the fetcher took nothing for {} s",
            contact::FILE_IDLE.as_secs()
        )
    };
    let opened = held.and_then(|h| disk::egress_reopen(&h.path, root, h.dev, h.inode, h.size).ok());
    let Some(e) = opened else {
        // SEC-N2: even the short answer is bounded.
        return contact_missing(framed).await;
    };
    let mut buf = vec![0u8; CHUNK];
    let mut at = 0u64;
    while at < e.size {
        let want = usize::try_from(e.size - at).map_or(CHUNK, |left| left.min(CHUNK));
        let n = e.file.read_at(&mut buf[..want], at)?;
        anyhow::ensure!(n > 0, "the held file shrank while it was served");
        let chunk = encode(&Reply::Chunk(buf[..n].to_vec()))?;
        tokio::time::timeout(contact::FILE_IDLE, framed.send(chunk))
            .await
            .map_err(idle)??;
        at += n as u64;
    }
    tokio::time::timeout(contact::FILE_IDLE, framed.send(encode(&Reply::End)?))
        .await
        .map_err(idle)??;
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

/// 199 P2 (D11): `(holder, id)` as `<data_dir>/files/cache/<holder>/<id>/<name>`, asked of
/// the holder alone. The expected size and BLAKE3 are `notice`'s, from this engine's own
/// copy of the session, never the holder's answer. A cached copy is returned only once it
/// verifies again (never the cache-hit shortcut of the fetches above); otherwise the
/// bytes stream into a partial under D8's idle timer, with no whole-fetch budget, are
/// checked, and are published no-clobber. `Ok(None)`: the holder answered `Missing`.
pub async fn fetch_contact_file_via(
    t: &dyn Transport,
    data_dir: &Path,
    holder: &PeerId,
    id: &str,
    notice: &ReceivedNotice,
) -> anyhow::Result<Option<PathBuf>> {
    #[cfg(not(unix))]
    {
        let _ = (t, data_dir, holder, id, notice);
        anyhow::bail!("contact files need a unix engine");
    }
    #[cfg(unix)]
    {
        anyhow::ensure!(
            plain_name(&holder.0) && plain_name(id),
            "{holder}/{id}: not a contact file"
        );
        let dir = data_dir.join(contact::CACHE_DIR).join(&holder.0).join(id);
        let (d, size, want) = (dir.clone(), notice.size, notice.blake3.clone());
        if let Some(hit) = tokio::task::spawn_blocking(move || verified(&d, size, &want)).await? {
            return Ok(Some(hit));
        }
        let partials = data_dir.join(contact::PARTIAL_DIR);
        private_dir(&dir)?;
        private_dir(&partials)?;
        let partial = partials.join(format!("cache-{}", uuid::Uuid::new_v4().simple()));
        let published = match receive_contact_file(t, holder, id, &partial, notice).await {
            Ok(true) => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let value = disk::quarantine_value(secs, uuid::Uuid::new_v4());
                let name = contact::sanitise(&notice.name);
                // No row to record: the cache is keyed by its path alone.
                disk::publish(&partial, &dir, &name, &value, async |_: &Path, _, _| Ok(()))
                    .await
                    .map(Some)
                    .map_err(anyhow::Error::from)
            }
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        };
        // A published target keeps its own link.
        disk::finish(&partial)?;
        published
    }
}

/// The bytes of a contact file differ from its notice's.
#[cfg(unix)]
const VERIFY_FAILED: &str = "verification failed";

/// One stream to `holder` into `partial` (created 0600), sized and hashed as frames
/// arrive. D8: each frame must come within [`contact::FILE_IDLE`]. `Ok(false)`: `Missing`.
#[cfg(unix)]
async fn receive_contact_file(
    t: &dyn Transport,
    holder: &PeerId,
    id: &str,
    partial: &Path,
    notice: &ReceivedNotice,
) -> anyhow::Result<bool> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let idle = |_| {
        anyhow::anyhow!(
            "{holder} sent nothing for {} s",
            contact::FILE_IDLE.as_secs()
        )
    };
    let stream = tokio::time::timeout(contact::FILE_IDLE, t.open(holder, ALPN_ARTIFACTS))
        .await
        .map_err(idle)??;
    let mut framed = framed(stream);
    // SEC-N2: the request write is bounded like every frame.
    tokio::time::timeout(
        contact::FILE_IDLE,
        framed.send(encode(&Req::GetContactFile { id: id.to_owned() })?),
    )
    .await
    .map_err(idle)??;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(partial)?;
    let mut hasher = blake3::Hasher::new();
    let mut done = 0u64;
    loop {
        let frame = tokio::time::timeout(contact::FILE_IDLE, framed.next())
            .await
            .map_err(idle)?
            .ok_or_else(|| anyhow::anyhow!("{holder} closed mid-answer"))??;
        match postcard::from_bytes(&frame)? {
            Reply::Missing if done == 0 => return Ok(false),
            Reply::Chunk(bytes) => {
                // SEC-N3: an empty frame is no progress; a holder sending them would
                // keep the idle timer from ever firing.
                anyhow::ensure!(!bytes.is_empty(), "{holder} sent an empty chunk");
                done += bytes.len() as u64;
                anyhow::ensure!(done <= notice.size, VERIFY_FAILED);
                hasher.update(&bytes);
                std::io::Write::write_all(&mut file, &bytes)?;
            }
            Reply::End => {
                anyhow::ensure!(
                    done == notice.size && hasher.finalize().to_hex().as_str() == notice.blake3,
                    VERIFY_FAILED
                );
                file.sync_all()?;
                return Ok(true);
            }
            other => anyhow::bail!("unexpected {other:?} for a contact file"),
        }
    }
}

/// A regular file in `dir` of exactly `size` bytes whose BLAKE3 is `blake3`. Every hit is
/// hashed again, so a tampered or truncated entry is skipped, never served.
#[cfg(unix)]
fn verified(dir: &Path, size: u64, blake3: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        // `file_type` does not follow a symlink.
        if !entry.file_type().ok()?.is_file() {
            return None;
        }
        let file = std::fs::File::open(entry.path()).ok()?;
        let meta = file.metadata().ok()?;
        (meta.is_file() && meta.len() == size && disk::blake3_of(&file, size).ok()? == blake3)
            .then(|| entry.path())
    })
}

/// `dir` and its missing parents, created 0700 (Storage).
#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
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
        /// 199 P2: a slow link, pausing this long after every MiB its opened streams read.
        pace: Mutex<Option<std::time::Duration>>,
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
                    pace: Mutex::new(None),
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
            Ok(match *self.pace.lock() {
                Some(pace) => Box::new(Slow {
                    inner: mine,
                    pace,
                    read: 0,
                    sleep: None,
                }),
                None => Box::new(mine),
            })
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
        format!("{}.{ext}", &platform::sha256::hex(bytes)[..16])
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

    /// A stream whose reads pause `pace` after every MiB (a duplex read is at most 1 MiB).
    struct Slow {
        inner: tokio::io::DuplexStream,
        pace: std::time::Duration,
        read: usize,
        sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }

    impl tokio::io::AsyncRead for Slow {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Some(s) = self.sleep.as_mut() {
                std::task::ready!(s.as_mut().poll(cx));
                self.sleep = None;
            }
            let before = buf.filled().len();
            std::task::ready!(std::pin::Pin::new(&mut self.inner).poll_read(cx, buf))?;
            let read = self.read + buf.filled().len() - before;
            if read >> 20 > self.read >> 20 {
                self.sleep = Some(Box::pin(tokio::time::sleep(self.pace)));
            }
            self.read = read;
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl tokio::io::AsyncWrite for Slow {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    /// 199-Z2 (network review of `b5bbd2c0f`): a contact file's `Missing` answer is bounded
    /// by the idle limit on every platform (the non-unix arm answers through this alone).
    #[tokio::test(start_paused = true)]
    async fn the_contact_missing_answer_is_bounded_on_every_platform() {
        let (mine, _theirs) = tokio::io::duplex(1);
        let mut framed = framed(Box::new(mine));
        let bound = contact::FILE_IDLE + std::time::Duration::from_secs(1);
        let sent = tokio::time::timeout(bound, contact_missing(&mut framed))
            .await
            .expect("the Missing answer is bounded");
        assert!(sent.is_err(), "{sent:?}");
    }

    /// 199 P2 (the holder's serving and the cache use the unix-only `disk`).
    #[cfg(unix)]
    mod contact_files {
        use super::*;

        const ID: &str = "9d0c4b7e21a35f6880e1c2d3b4a59687";

        /// `bytes` written to `dir/name` as a holder records a received file.
        fn hold(dir: &Path, name: &str, bytes: &[u8]) -> HeldFile {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let (dev, inode) = disk::inode_of(&path).unwrap();
            HeldFile {
                path,
                dev,
                inode,
                size: bytes.len() as u64,
                blake3: blake3::hash(bytes).to_hex().to_string(),
            }
        }

        /// What the fetcher's own notice of `h` says.
        fn notice_of(h: &HeldFile, name: &str) -> ReceivedNotice {
            ReceivedNotice {
                name: name.into(),
                size: h.size,
                blake3: h.blake3.clone(),
                machine: "Sky's MacBook Pro".into(),
            }
        }

        /// A holder's accept loop: `answer`'s `GetContactFile` arm once `Agents` answered
        /// from `held` (local id → row).
        fn holder(t: Arc<Loop>, root: PathBuf, held: HashMap<String, HeldFile>) {
            tokio::spawn(async move {
                while let Ok((_, stream)) = t.accept(ALPN_ARTIFACTS).await {
                    let (root, held) = (root.clone(), held.clone());
                    tokio::spawn(async move {
                        let mut framed = framed(stream);
                        let Some(Ok(first)) = framed.next().await else {
                            return;
                        };
                        if let Ok(Req::GetContactFile { id }) = postcard::from_bytes(&first) {
                            let _ =
                                send_contact_file(&mut framed, &root, held.get(&id).cloned()).await;
                        }
                    });
                }
            });
        }

        fn hits(t: &Loop) -> usize {
            t.hits.load(std::sync::atomic::Ordering::Relaxed)
        }

        fn entries(dir: &Path) -> usize {
            std::fs::read_dir(dir).map_or(0, |d| d.count())
        }

        /// 199 P2: a transfer far longer than 60 s completes under the idle timer (paused
        /// clock), lands verified in the cache, and every later hit is hashed again: a
        /// tampered entry is never served. A holder silent for over 60 s is cut.
        #[tokio::test(start_paused = true)]
        async fn a_contact_file_slower_than_60_s_arrives_and_every_cache_hit_is_rehashed() {
            let (holder_dir, fetcher_dir) =
                (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let (ht, ft) = pair();
            let bytes = (0..(3 * CHUNK + 5))
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>();
            let h = hold(holder_dir.path(), "survey.zip", &bytes);
            let stalled = hold(holder_dir.path(), "stalled.bin", &bytes[..2 << 20]);
            let stalled_id = "41b8d2f0c7e65a1394b2a8d0e3f1c5a7";
            holder(
                ht.clone(),
                holder_dir.path().into(),
                HashMap::from([(ID.into(), h.clone()), (stalled_id.into(), stalled.clone())]),
            );
            let notice = notice_of(&h, "site survey.zip");
            let root = fetcher_dir.path();

            // 12 MiB at a MiB every 6 s: about 72 s in all, never 60 s without a byte.
            *ft.pace.lock() = Some(std::time::Duration::from_secs(6));
            let start = tokio::time::Instant::now();
            let got = fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice)
                .await
                .unwrap()
                .unwrap();
            assert!(
                start.elapsed() > std::time::Duration::from_secs(60),
                "{:?}",
                start.elapsed()
            );
            let cache = root.join("files/cache/owner").join(ID);
            assert_eq!(got, cache.join("site survey.zip"));
            assert_eq!(std::fs::read(&got).unwrap(), bytes);
            assert_eq!(
                entries(&root.join(contact::PARTIAL_DIR)),
                0,
                "partial dropped"
            );
            assert_eq!(hits(&ht), 2);
            *ft.pace.lock() = None;

            // A repeat is a verified hit: the holder is not asked.
            let again = fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice)
                .await
                .unwrap();
            assert_eq!(again.as_deref(), Some(got.as_path()));
            assert_eq!(hits(&ht), 2, "served from the cache");

            // A tampered entry (same size) is skipped and fetched afresh beside it, no clobber.
            let mut bad = bytes.clone();
            bad[CHUNK] ^= 1;
            std::fs::write(&got, &bad).unwrap();
            let fresh = fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(fresh, cache.join("site survey (2).zip"));
            assert_eq!(std::fs::read(&fresh).unwrap(), bytes);
            assert_eq!(std::fs::read(&got).unwrap(), bad, "never replaced");
            assert_eq!(hits(&ht), 3, "the holder was asked again");
            let hit = fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice)
                .await
                .unwrap();
            assert_eq!(
                hit.as_deref(),
                Some(fresh.as_path()),
                "only the verified entry is a hit"
            );
            assert_eq!(hits(&ht), 3);

            // No byte for over 60 s: cut at idle, nothing cached.
            *ft.pace.lock() = Some(std::time::Duration::from_secs(61));
            let err = fetch_contact_file_via(
                ft.as_ref(),
                root,
                &ht.me,
                stalled_id,
                &notice_of(&stalled, "stalled.bin"),
            )
            .await
            .unwrap_err();
            assert_eq!(err.to_string(), "owner sent nothing for 60 s");
            assert_eq!(entries(&root.join("files/cache/owner").join(stalled_id)), 0);
            assert_eq!(entries(&root.join(contact::PARTIAL_DIR)), 0);
        }

        /// 199 P2: bytes that are not the notice's (another BLAKE3, or more than its size)
        /// are refused and leave nothing behind.
        #[tokio::test]
        async fn a_holder_answering_other_bytes_is_refused() {
            let (holder_dir, fetcher_dir) =
                (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let (ht, ft) = pair();
            let h = hold(holder_dir.path(), "a.pdf", b"what the holder has now");
            holder(
                ht.clone(),
                holder_dir.path().into(),
                HashMap::from([(ID.into(), h.clone())]),
            );
            let root = fetcher_dir.path();
            let other = ReceivedNotice {
                blake3: blake3::hash(b"what the notice promised")
                    .to_hex()
                    .to_string(),
                ..notice_of(&h, "a.pdf")
            };
            let shorter = ReceivedNotice {
                size: h.size - 1,
                ..notice_of(&h, "a.pdf")
            };
            for notice in [other, shorter] {
                let err = fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice)
                    .await
                    .unwrap_err();
                assert_eq!(err.to_string(), "verification failed");
            }
            assert_eq!(entries(&root.join("files/cache/owner").join(ID)), 0);
            assert_eq!(entries(&root.join(contact::PARTIAL_DIR)), 0);
        }

        /// 199 P2: only the holder named is asked, whoever owns the chat now (it moved to
        /// `node3`); an unknown id, a file replaced since it was recorded, and the engine's
        /// own arm without a `received` row (no supervisor here) all answer `Missing`.
        #[tokio::test]
        async fn only_the_holder_is_asked_and_anything_but_its_received_file_is_missing() {
            let mut v = nodes(&["fetcher", "holder", "node3"]);
            let (ft, ht, node3) = (v.remove(0), v.remove(0), v.remove(0));
            let (holder_dir, node3_dir, fetcher_dir) = (
                tempfile::tempdir().unwrap(),
                tempfile::tempdir().unwrap(),
                tempfile::tempdir().unwrap(),
            );
            owner(node3.clone(), node3_dir.path().to_path_buf()).await;
            let h = hold(holder_dir.path(), "a.png", b"held bytes");
            let replaced = hold(holder_dir.path(), "b.png", b"old bytes");
            let replaced_id = "0123456789abcdef0123456789abcdef";
            holder(
                ht.clone(),
                holder_dir.path().into(),
                HashMap::from([
                    (ID.into(), h.clone()),
                    (replaced_id.into(), replaced.clone()),
                ]),
            );
            let root = fetcher_dir.path();

            let got =
                fetch_contact_file_via(ft.as_ref(), root, &ht.me, ID, &notice_of(&h, "a.png"))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(std::fs::read(got).unwrap(), b"held bytes");
            assert_eq!(hits(&node3), 1, "the chat's new owner was never asked");

            let unknown = "ffffffffffffffffffffffffffffffff";
            let n = notice_of(&h, "a.png");
            assert_eq!(
                fetch_contact_file_via(ft.as_ref(), root, &ht.me, unknown, &n)
                    .await
                    .unwrap(),
                None
            );
            // Same path, new inode: not the file that was recorded.
            std::fs::remove_file(&replaced.path).unwrap();
            std::fs::write(&replaced.path, b"old bytes").unwrap();
            let n = notice_of(&replaced, "b.png");
            assert_eq!(
                fetch_contact_file_via(ft.as_ref(), root, &ht.me, replaced_id, &n)
                    .await
                    .unwrap(),
                None
            );
            // `answer` itself: no `Agents`, so no `received` row.
            assert_eq!(
                fetch_contact_file_via(ft.as_ref(), root, &node3.me, ID, &n)
                    .await
                    .unwrap(),
                None
            );
            assert!(
                fetch_contact_file_via(ft.as_ref(), root, &ht.me, "../x", &n)
                    .await
                    .is_err(),
                "an id is never a path"
            );
        }

        /// 199-Z1 SEC-N3: a holder sending empty chunks forever is refused at the first one
        /// (an empty frame is no progress), nothing kept.
        #[tokio::test(start_paused = true)]
        async fn empty_chunks_are_refused() {
            let fetcher_dir = tempfile::tempdir().unwrap();
            let (ht, ft) = pair();
            tokio::spawn(async move {
                while let Ok((_, stream)) = ht.accept(ALPN_ARTIFACTS).await {
                    tokio::spawn(async move {
                        let mut framed = framed(stream);
                        let _ = framed.next().await;
                        while framed
                            .send(encode(&Reply::Chunk(Vec::new())).unwrap())
                            .await
                            .is_ok()
                        {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    });
                }
            });
            let h = HeldFile {
                path: "/x".into(),
                dev: 0,
                inode: 0,
                size: 10,
                blake3: "0".repeat(64),
            };
            let root = fetcher_dir.path();
            let err = tokio::time::timeout(
                contact::FILE_IDLE,
                fetch_contact_file_via(
                    ft.as_ref(),
                    root,
                    &PeerId("owner".into()),
                    ID,
                    &notice_of(&h, "a"),
                ),
            )
            .await
            .expect("refused long before the idle timer")
            .unwrap_err();
            assert!(err.to_string().contains("empty chunk"), "{err}");
            assert_eq!(entries(&root.join(contact::PARTIAL_DIR)), 0);
        }

        /// A link whose peer never reads: every write blocks once its small buffer is full.
        struct Stuck(std::sync::Mutex<Vec<tokio::io::DuplexStream>>);

        #[async_trait::async_trait]
        impl Transport for Stuck {
            fn peers(&self) -> Vec<PeerId> {
                vec![PeerId("owner".into())]
            }
            async fn open(&self, _: &PeerId, _: &str) -> anyhow::Result<Box<dyn ByteStream>> {
                let (mine, theirs) = tokio::io::duplex(1);
                self.0.lock().unwrap().push(theirs);
                Ok(Box::new(mine))
            }
            async fn accept(&self, _: &str) -> anyhow::Result<Inbound> {
                std::future::pending().await
            }
            fn subscribe(&self) -> broadcast::Receiver<replica::PeerEvent> {
                broadcast::channel(1).1
            }
        }

        /// 199-Z1 SEC-N2: a peer withholding stream credit never holds either side past the
        /// idle bound: the fetcher's request write and the holder's `Missing` answer both end.
        #[tokio::test(start_paused = true)]
        async fn blocked_control_writes_end_within_the_idle_bound() {
            let dir = tempfile::tempdir().unwrap();
            let bound = contact::FILE_IDLE + std::time::Duration::from_secs(1);
            let stuck = Stuck(std::sync::Mutex::default());
            let h = HeldFile {
                path: "/x".into(),
                dev: 0,
                inode: 0,
                size: 10,
                blake3: "0".repeat(64),
            };
            let (owner, notice) = (PeerId("owner".into()), notice_of(&h, "a"));
            let fetch = fetch_contact_file_via(&stuck, dir.path(), &owner, ID, &notice);
            let got = tokio::time::timeout(bound, fetch)
                .await
                .expect("the request write is bounded");
            assert!(got.is_err(), "{got:?}");

            let (mine, _theirs) = tokio::io::duplex(1);
            let mut framed = framed(Box::new(mine));
            let sent =
                tokio::time::timeout(bound, send_contact_file(&mut framed, dir.path(), None))
                    .await
                    .expect("the Missing answer is bounded");
            assert!(sent.is_err());
        }
    }
}
