//! Bundle sync over the mesh (design doc 67 §5.2). On every `Connected` and
//! every `knowledge.sync_secs`, open `theuth/knowledge` to each peer and
//! exchange a manifest of HEAD — per concept path its blob id and the time
//! of the commit that last touched it, deletions included as tombstones —
//! then ship the files the other side lacks or has older. Received files
//! land as one commit `sync from <engine>`; the local `sync` then refreshes
//! the FTS rows and publishes the usual `KnowledgeChanged` events.
//!
//! Rules: disjoint files always merge; the same file changed on both sides →
//! the newer commit wins (tie: the higher blob id, so both sides agree), the
//! loser stays in history. Nothing is deleted because a peer lacks it — only
//! a peer's *deletion commit* newer than the local version removes a file.
//!
//! Frames are `u32 LE length ‖ JSON` (the mesh pairing idiom), two messages
//! each way per exchange; the dialer speaks first at each step so neither
//! side writes while the other is not reading.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use replica::{ALPN_KNOWLEDGE, ByteStream, PeerEvent, PeerId, Transport};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::repo::Repo;
use crate::store::Knowledge;

/// How long the plugin waits for the mesh to register a transport before
/// giving up on sync for this process (a single engine has no mesh).
const TRANSPORT_WAIT: Duration = Duration::from_secs(120);
/// Largest frame accepted (a manifest or a batch of files).
const MAX_FRAME: u32 = 64 * 1024 * 1024;

/// One path in a manifest: its blob at HEAD (`None` = deleted) and the Unix
/// seconds of the commit that last touched it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Blob id at HEAD; `None` = the path's last touch was a deletion.
    pub blob: Option<String>,
    /// Unix seconds of the commit that last touched the path.
    pub seconds: i64,
}

/// What each side says about its bundle.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Full HEAD id; `None` while unborn.
    pub head: Option<String>,
    /// Root-relative `/` paths.
    pub files: BTreeMap<String, Entry>,
}

/// One file shipped; `bytes: None` = the sender's newer deletion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File {
    /// Root-relative `/` path.
    pub path: String,
    /// File contents; `None` = the sender's newer deletion.
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Msg {
    Manifest(Manifest),
    Files(Vec<File>),
}

/// Start syncing `k` with every peer: accept loop, a dial on each
/// `Connected` and every `sync_secs`. Returns at once; logs and gives up
/// when no transport appears within [`TRANSPORT_WAIT`].
pub fn start(k: Arc<Knowledge>, sync_secs: u64) {
    tokio::spawn(async move {
        let Some(t) = replica::transport_within(TRANSPORT_WAIT).await else {
            tracing::info!("knowledge: no mesh transport; bundle sync off");
            return;
        };
        let (k2, t2) = (k.clone(), t.clone());
        tokio::spawn(async move {
            loop {
                match t2.accept(ALPN_KNOWLEDGE).await {
                    Ok((peer, stream)) => {
                        let k = k2.clone();
                        tokio::spawn(async move {
                            if let Err(e) = exchange(&k, stream, &peer, false).await {
                                tracing::warn!("knowledge sync from {peer}: {e:#}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("knowledge: accept: {e:#}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        let mut events = t.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(sync_secs.max(5)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            let peers = tokio::select! {
                _ = tick.tick() => t.peers(),
                ev = events.recv() => match ev {
                    Ok(PeerEvent::Connected(p)) => vec![p],
                    Ok(PeerEvent::Disconnected(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => t.peers(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            for peer in peers {
                if let Err(e) = dial(&k, &t, &peer).await {
                    tracing::warn!("knowledge sync with {peer}: {e:#}");
                }
            }
        }
    });
}

/// One exchange with `peer` as the dialer.
pub async fn dial(k: &Arc<Knowledge>, t: &Arc<dyn Transport>, peer: &PeerId) -> anyhow::Result<()> {
    let stream = t.open(peer, ALPN_KNOWLEDGE).await?;
    exchange(k, stream, peer, true).await
}

/// The protocol on one stream: manifests both ways, then the files each side
/// needs, then apply. `dialer` speaks first at each step.
pub async fn exchange(
    k: &Arc<Knowledge>,
    mut stream: Box<dyn ByteStream>,
    peer: &PeerId,
    dialer: bool,
) -> anyhow::Result<()> {
    let repo = k.repo.clone().context("no repository: sync unavailable")?;
    let mine = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || manifest(&repo)).await??
    };
    let theirs = if dialer {
        send(&mut stream, &Msg::Manifest(mine.clone())).await?;
        recv_manifest(&mut stream).await?
    } else {
        let m = recv_manifest(&mut stream).await?;
        send(&mut stream, &Msg::Manifest(mine.clone())).await?;
        m
    };
    let for_them = {
        let (repo, mine, theirs) = (repo.clone(), mine.clone(), theirs.clone());
        tokio::task::spawn_blocking(move || files_for(&repo, &mine, &theirs)).await??
    };
    let for_me = if dialer {
        send(&mut stream, &Msg::Files(for_them)).await?;
        recv_files(&mut stream).await?
    } else {
        let f = recv_files(&mut stream).await?;
        send(&mut stream, &Msg::Files(for_them)).await?;
        f
    };
    let _ = stream.shutdown().await;
    if !for_me.is_empty() {
        apply(k, &repo, for_me, peer).await?;
    }
    Ok(())
}

/// HEAD's paths with the time of the commit that last touched each, and
/// tombstones for paths whose last touch was a deletion.
pub fn manifest(repo: &Repo) -> anyhow::Result<Manifest> {
    let Some(head) = repo.head_id()? else {
        return Ok(Manifest::default());
    };
    let mut files = BTreeMap::new();
    // Newest first: the first commit naming a path is its last touch.
    for c in repo.changes_from(head)? {
        for f in c.files {
            files.entry(f.loc).or_insert(Entry {
                blob: f.after.map(|b| b.to_string()),
                seconds: c.seconds,
            });
        }
    }
    Ok(Manifest {
        head: Some(head.to_string()),
        files,
    })
}

/// `theirs` beats `mine` for one path: newer commit, tie by blob id.
fn newer(theirs: &Entry, mine: Option<&Entry>) -> bool {
    match mine {
        None => theirs.blob.is_some(),
        Some(m) => theirs.blob != m.blob && (theirs.seconds, &theirs.blob) > (m.seconds, &m.blob),
    }
}

/// The files `theirs` should take from `mine`.
fn files_for(repo: &Repo, mine: &Manifest, theirs: &Manifest) -> anyhow::Result<Vec<File>> {
    let mut out = Vec::new();
    for (path, e) in &mine.files {
        if !newer(e, theirs.files.get(path)) {
            continue;
        }
        let bytes = match &e.blob {
            Some(b) => Some(repo.blob(b.parse().map_err(|_| anyhow::anyhow!("bad blob id {b}"))?)?),
            None => None,
        };
        out.push(File {
            path: path.clone(),
            bytes,
        });
    }
    Ok(out)
}

/// Write the received files under the bundle root, commit them as
/// `sync from <peer>`, then let the store's `sync` refresh rows and publish.
///
/// ponytail: files are written outside the store's write lock; a local put
/// racing the same path in the same instant ends in whichever commit lands
/// second, both kept in history.
async fn apply(
    k: &Arc<Knowledge>,
    repo: &Arc<Repo>,
    files: Vec<File>,
    peer: &PeerId,
) -> anyhow::Result<()> {
    let root = k.root().to_path_buf();
    let message = format!("sync from {peer}");
    let n = files.len();
    let repo = repo.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut written = Vec::new();
        let mut removed = Vec::new();
        for f in files {
            let rel = PathBuf::from(&f.path);
            anyhow::ensure!(
                rel.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "refusing path {}",
                f.path
            );
            let full = root.join(&rel);
            match f.bytes {
                Some(bytes) => {
                    write_atomic(&full, &bytes)?;
                    written.push(rel);
                }
                None => {
                    match std::fs::remove_file(&full) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(e).with_context(|| format!("remove {}", full.display()));
                        }
                    }
                    removed.push(rel);
                }
            }
        }
        repo.commit(&written, &removed, &message)?;
        Ok(())
    })
    .await??;
    k.sync().await?;
    tracing::info!("knowledge: {n} files synced from {peer}");
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
}

async fn send(stream: &mut Box<dyn ByteStream>, msg: &Msg) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    anyhow::ensure!(
        bytes.len() <= MAX_FRAME as usize,
        "frame too large: {} bytes",
        bytes.len()
    );
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn recv(stream: &mut Box<dyn ByteStream>) -> anyhow::Result<Msg> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len);
    anyhow::ensure!(len <= MAX_FRAME, "frame too large: {len} bytes");
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

async fn recv_manifest(stream: &mut Box<dyn ByteStream>) -> anyhow::Result<Manifest> {
    match recv(stream).await? {
        Msg::Manifest(m) => Ok(m),
        other => anyhow::bail!("expected a manifest, got {other:?}"),
    }
}

async fn recv_files(stream: &mut Box<dyn ByteStream>) -> anyhow::Result<Vec<File>> {
    match recv(stream).await? {
        Msg::Files(f) => Ok(f),
        other => anyhow::bail!("expected files, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Put, Source};

    async fn bundle() -> (Arc<Knowledge>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("theuth-ksync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        (Knowledge::open(&dir, rt).await.unwrap(), dir)
    }

    fn put(id: &str, body: &str) -> Put {
        Put {
            id: id.into(),
            type_: "fact".into(),
            title: id.into(),
            description: None,
            tags: vec![],
            scope: None,
            body: body.into(),
            visibility: None,
        }
    }

    /// One exchange over an in-memory duplex: `a` dials `b`.
    async fn sync_pair(a: &Arc<Knowledge>, b: &Arc<Knowledge>) {
        let (sa, sb) = tokio::io::duplex(1 << 20);
        let (pa, pb) = (PeerId("a".into()), PeerId("b".into()));
        let (ra, rb) = tokio::join!(
            exchange(a, Box::new(sa), &pb, true),
            exchange(b, Box::new(sb), &pa, false),
        );
        ra.unwrap();
        rb.unwrap();
    }

    async fn ids(k: &Arc<Knowledge>) -> Vec<String> {
        k.list(None, None, None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect()
    }

    #[tokio::test]
    async fn two_bundles_converge_both_ways_and_deletions_travel() {
        let (a, da) = bundle().await;
        let (b, db) = bundle().await;
        a.put(put("facts/only-a", "from a"), Source::User)
            .await
            .unwrap();
        b.put(put("facts/only-b", "from b"), Source::User)
            .await
            .unwrap();

        sync_pair(&a, &b).await;
        assert_eq!(ids(&a).await, ["facts/only-a", "facts/only-b"]);
        assert_eq!(ids(&b).await, ["facts/only-a", "facts/only-b"]);
        assert!(b.get("facts/only-a").await.unwrap().text.contains("from a"));
        let hist = b.history(Some("facts/only-a"), 10).await.unwrap();
        assert_eq!(hist.changes[0].message, "sync from a");

        // Nothing to do: a second exchange writes no commit.
        let head_b = b.repo.as_ref().unwrap().head_id().unwrap();
        sync_pair(&a, &b).await;
        assert_eq!(b.repo.as_ref().unwrap().head_id().unwrap(), head_b);

        // A deletion is a commit and travels; a missing file alone never deletes.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        a.delete("facts/only-b").await.unwrap();
        sync_pair(&a, &b).await;
        assert_eq!(ids(&b).await, ["facts/only-a"]);
        assert!(!db.join("facts/only-b.md").exists());
        // …and the deletion does not come back from B on the next round.
        sync_pair(&b, &a).await;
        assert_eq!(ids(&a).await, ["facts/only-a"]);

        let _ = std::fs::remove_dir_all(da);
        let _ = std::fs::remove_dir_all(db);
    }

    #[tokio::test]
    async fn conflicting_edit_newer_wins_and_history_keeps_both() {
        let (a, da) = bundle().await;
        let (b, db) = bundle().await;
        a.put(put("facts/shared", "v0"), Source::User)
            .await
            .unwrap();
        sync_pair(&a, &b).await;

        // Both edit; B a second later (commit times are whole seconds).
        a.put(put("facts/shared", "a wins? no"), Source::User)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        b.put(put("facts/shared", "b is newer"), Source::User)
            .await
            .unwrap();

        sync_pair(&a, &b).await;
        assert!(
            a.get("facts/shared")
                .await
                .unwrap()
                .text
                .contains("b is newer")
        );
        assert!(
            b.get("facts/shared")
                .await
                .unwrap()
                .text
                .contains("b is newer")
        );
        let hist = a.history(Some("facts/shared"), 10).await.unwrap();
        assert_eq!(hist.changes[0].message, "sync from b");
        let messages: Vec<&str> = hist.changes.iter().map(|c| c.message.as_str()).collect();
        assert!(messages.len() >= 3, "{messages:?}");
        let loser = a
            .revision(&hist.changes[1].commit, Some("facts/shared"))
            .await
            .unwrap();
        assert!(
            loser.files[0].diff.contains("a wins? no"),
            "the loser stays in history"
        );

        // Converged: the manifests agree and another round is a no-op.
        let ma = manifest(a.repo.as_ref().unwrap()).unwrap();
        let mb = manifest(b.repo.as_ref().unwrap()).unwrap();
        assert_eq!(
            ma.files["facts/shared.md"].blob,
            mb.files["facts/shared.md"].blob
        );
        let head_a = a.repo.as_ref().unwrap().head_id().unwrap();
        sync_pair(&b, &a).await;
        assert_eq!(a.repo.as_ref().unwrap().head_id().unwrap(), head_a);

        let _ = std::fs::remove_dir_all(da);
        let _ = std::fs::remove_dir_all(db);
    }
}
