//! Model files and the ONNX Runtime the daemon loads from `<data_dir>/models/` (43 §4.6,
//! 45 §4.6, 54 §9, meetings 04 §0.1, 57 §2 and §6 step 2c): at start, every entry of
//! `models.json` on the release bucket (`scripts/release-models.sh` publishes both) that this
//! machine lacks **or holds another version of** is fetched. "The listed file" means size and
//! sha256. The hash is computed once and remembered in `<file>.sha256` ([`platform::sha256`],
//! the sidecar the `decision` loader keeps too), so a start reads the 3 GB only the first time
//! after this landed (the webview hears `verifying`); a download writes the sidecar from the
//! hash it streamed. A manifest row without `sha256` is compared by size only (older manifests).
//!
//! Replacing a file the daemon may have open: every model is opened lazily and `.part` + rename
//! is atomic, so STT/TTS/ORT files are swapped in place and the next open gets the new one. A
//! decision checkpoint (`decision*/`, 57 §2.1) that `decision_loaded` reports resident is not
//! swapped under its session: the verified `.part` waits and the rename is retried every minute
//! until the checkpoint unloads. Pruning is not done — entries that leave the manifest stay on
//! disk. Entries carrying a `platform` are for that rust target triple only — the runtime is
//! published once per platform under `onnxruntime/<triple>/` and lands at `models/onnxruntime/`.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::launch;

/// `models.json`; the files sit beside it under the same directory. `THEUTH_MODELS_URL` overrides.
const URL: &str = "https://s3.rwx.dev/theuth/releases/models/models.json";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-read stall budget; whole-request timeouts do not fit a 900 MB file.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Progress events at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How often a replacement waiting on a loaded checkpoint retries its rename.
const PENDING_RETRY: Duration = Duration::from_secs(60);
/// `decision_loaded` over loopback; no answer in this long counts as nothing loaded.
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Manifest {
    files: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    /// `<model>/<file>`, relative to `<data_dir>/models/`, where the file lands.
    path: String,
    size: u64,
    /// Empty when the manifest predates hashing: size-only compare.
    #[serde(default)]
    sha256: String,
    /// Rust target triple this entry is for; absent = every platform.
    #[serde(default)]
    platform: Option<String>,
    /// Remote key relative to `models/` when it differs from `path` (per-platform storage).
    #[serde(default)]
    key: Option<String>,
}

impl Entry {
    /// What to download: `key` when set, else `path`.
    fn key(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.path)
    }

    /// Meant for this build ([`platform::target::target_triple`]).
    fn for_this_platform(&self) -> bool {
        self.platform
            .as_deref()
            .is_none_or(|p| p == platform::target::target_triple())
    }

    /// The decision checkpoint this file belongs to, by the directory convention
    /// (`decision/` = `default`, `decision-<name>/` = `<name>`; decision `lib.rs`); `None`
    /// for anything else. `decision.checkpoints.<name>.dir` overrides are not read here.
    fn checkpoint(&self) -> Option<&str> {
        let dir = self.path.split_once('/')?.0;
        match dir {
            "decision" => Some("default"),
            _ => dir.strip_prefix("decision-"),
        }
    }
}

/// `models-progress` payload. `done`/`total` are bytes over every file to fetch.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum Progress<'a> {
    Downloading {
        file: &'a str,
        done: u64,
        total: u64,
    },
    /// Hashing a file that has no current sidecar yet (the first start after an install
    /// that predates sidecars).
    Verifying {
        file: &'a str,
    },
    Ready,
    Failed {
        message: String,
    },
}

/// What one pass did; `start` is silent when both are zero.
#[derive(Debug, Default, PartialEq, Eq)]
struct Outcome {
    /// Files hashed for want of a sidecar.
    verified: usize,
    /// Files fetched (landed or waiting in `pending`).
    downloaded: usize,
}

/// A verified `.part` whose checkpoint was loaded when it finished; renamed once it is not.
struct Pending {
    part: PathBuf,
    dest: PathBuf,
    path: String,
    checkpoint: String,
    sha256: String,
}

/// Fetch missing and changed models in the background; silent when there is nothing to do.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut pending = Vec::new();
        match ensure(&app, &mut pending).await {
            Ok(Outcome {
                verified: 0,
                downloaded: 0,
            }) => {}
            Ok(o) => {
                tracing::info!(
                    "models: {} files verified, {} downloaded",
                    o.verified,
                    o.downloaded
                );
                emit(&app, &Progress::Ready);
            }
            Err(e) => {
                tracing::warn!("models: {e:#}");
                emit(
                    &app,
                    &Progress::Failed {
                        message: format!("{e:#}"),
                    },
                );
            }
        }
        while !pending.is_empty() {
            tokio::time::sleep(PENDING_RETRY).await;
            let loaded = loaded_checkpoints(&app).await;
            apply_pending(&mut pending, &loaded).await;
        }
    });
}

fn emit(app: &AppHandle, p: &Progress<'_>) {
    if let Err(e) = app.emit("models-progress", p) {
        tracing::warn!(error = %e, "emit models-progress");
    }
}

/// Manifest → [`sync`] against `<data_dir>/models/`.
async fn ensure(app: &AppHandle, pending: &mut Vec<Pending>) -> anyhow::Result<Outcome> {
    let url = std::env::var("THEUTH_MODELS_URL").unwrap_or_else(|_| URL.to_owned());
    let base = url.rsplit_once('/').map(|(b, _)| b).unwrap_or(&url);
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()?;
    let manifest: Manifest = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("parse {url}"))?;
    let root = launch::data_dir().join("models");
    sync(
        &client,
        base,
        &root,
        &manifest,
        || loaded_checkpoints(app),
        pending,
        &mut |p| emit(app, p),
    )
    .await
}

/// Every entry for this platform: keep, hash, or fetch; a fetched file replacing one whose
/// checkpoint `loaded()` reports resident goes to `pending` instead of over the old file.
async fn sync<F: Future<Output = Vec<String>>>(
    client: &reqwest::Client,
    base: &str,
    root: &Path,
    manifest: &Manifest,
    mut loaded: impl FnMut() -> F,
    pending: &mut Vec<Pending>,
    emit: &mut impl FnMut(&Progress<'_>),
) -> anyhow::Result<Outcome> {
    let mut out = Outcome::default();
    let mut fetch: Vec<&Entry> = Vec::new();
    for e in manifest.files.iter().filter(|e| e.for_this_platform()) {
        // Trust boundary: the manifest names paths under `models/` only.
        for p in [e.path.as_str(), e.key()] {
            anyhow::ensure!(
                Path::new(p)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
                "manifest path {p:?} escapes the models directory"
            );
        }
        let dest = root.join(&e.path);
        match status(&dest, e) {
            Status::Keep => {}
            Status::Fetch => fetch.push(e),
            Status::Hash => {
                emit(&Progress::Verifying { file: &e.path });
                out.verified += 1;
                let hex = tokio::task::spawn_blocking(move || platform::sha256::cached(&dest))
                    .await?
                    .with_context(|| format!("hash {}", e.path))?;
                if hex != e.sha256 {
                    fetch.push(e);
                }
            }
        }
    }
    if fetch.is_empty() {
        return Ok(out);
    }
    let total: u64 = fetch.iter().map(|e| e.size).sum();
    let mut done = 0u64;
    let mut last = Instant::now() - PROGRESS_EVERY;
    for e in fetch {
        tracing::info!("models: downloading {} ({} bytes)", e.path, e.size);
        let dest = root.join(&e.path);
        let part = download(client, &format!("{base}/{}", e.key()), e, &dest, &mut |n| {
            done += n;
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                emit(&Progress::Downloading {
                    file: &e.path,
                    done,
                    total,
                });
            }
        })
        .await
        .with_context(|| e.path.clone())?;
        out.downloaded += 1;
        let replacing = dest.exists();
        if let Some(cp) = e.checkpoint()
            && replacing
            && loaded().await.iter().any(|l| l == cp)
        {
            tracing::info!(
                "models: {} updated on disk, applied when the checkpoint unloads",
                e.path
            );
            pending.push(Pending {
                part,
                dest,
                path: e.path.clone(),
                checkpoint: cp.to_owned(),
                sha256: e.sha256.clone(),
            });
        } else {
            land(&part, &dest, &e.sha256)
                .await
                .with_context(|| e.path.clone())?;
        }
    }
    Ok(out)
}

enum Status {
    /// Right size and (when the manifest has one) the remembered hash.
    Keep,
    /// Right size, no current sidecar: hash to know.
    Hash,
    /// Absent, wrong size, or a remembered hash that is not the manifest's.
    Fetch,
}

/// Cheap part of "is this the listed file": metadata and sidecar only, no hashing.
fn status(dest: &Path, e: &Entry) -> Status {
    let Ok(m) = std::fs::metadata(dest) else {
        return Status::Fetch;
    };
    if !m.is_file() || m.len() != e.size {
        return Status::Fetch;
    }
    if e.sha256.is_empty() {
        tracing::debug!(
            "models: {} has no sha256 in the manifest; size only",
            e.path
        );
        return Status::Keep;
    }
    match platform::sha256::sidecar(dest) {
        Ok(Some(hex)) if hex == e.sha256 => Status::Keep,
        Ok(Some(_)) => Status::Fetch,
        Ok(None) | Err(_) => Status::Hash,
    }
}

/// Stream `url` to `<dest>.part`, hashing as it lands; the verified (size + sha256) `.part`
/// path comes back for [`land`], a bad one is removed.
async fn download(
    client: &reqwest::Client,
    url: &str,
    e: &Entry,
    dest: &Path,
    on_chunk: &mut impl FnMut(u64),
) -> anyhow::Result<PathBuf> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(&part).await?;
    let mut hasher = Sha256::new();
    let mut n = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        n += chunk.len() as u64;
        on_chunk(chunk.len() as u64);
    }
    file.flush().await?;
    drop(file);
    let sha = format!("{:x}", hasher.finalize());
    if n != e.size || sha != e.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        anyhow::bail!(
            "got {n} bytes sha256 {sha}, manifest says {} bytes {}",
            e.size,
            e.sha256
        );
    }
    // The runtime is dlopened: executable bit on, like Microsoft ships it.
    if e.path.starts_with("onnxruntime/") && !e.path.ends_with("/VERSION") {
        platform::files::set_executable(&part)?;
    }
    Ok(part)
}

/// Rename the verified `.part` over `dest` and remember its hash, so no start hashes it.
async fn land(part: &Path, dest: &Path, sha256: &str) -> anyhow::Result<()> {
    tokio::fs::rename(part, dest).await?;
    platform::sha256::remember(dest, sha256);
    Ok(())
}

/// Land every pending replacement whose checkpoint is no longer in `loaded`.
async fn apply_pending(pending: &mut Vec<Pending>, loaded: &[String]) {
    for p in std::mem::take(pending) {
        if loaded.contains(&p.checkpoint) {
            pending.push(p);
            continue;
        }
        match land(&p.part, &p.dest, &p.sha256).await {
            Ok(()) => tracing::info!("models: {} applied", p.path),
            Err(e) => tracing::warn!("models: apply {}: {e:#}", p.path),
        }
    }
}

/// Checkpoint names the daemon has resident (`decision_loaded`, 57 §2.1 item 4). `[]` when
/// no daemon is reachable — nothing can be loaded then — or it does not answer in time.
// ponytail: only decision checkpoints have a loaded-set RPC; a generic "open model files" RPC
// would let STT/TTS/ORT swaps wait too, if an in-place swap ever bites.
async fn loaded_checkpoints(app: &AppHandle) -> Vec<String> {
    let peek = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || app.state::<launch::Launch>().peek())
    };
    let Ok(Some(conn)) = peek.await else {
        return Vec::new();
    };
    match decision_loaded(&conn.url).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("models: decision_loaded: {e:#}");
            Vec::new()
        }
    }
}

/// One JSON-RPC call over plain HTTP (the daemon serves both; `?token=` authorises) to the
/// daemon at `ws_url` (`ws://<listen>/?token=<token>`).
async fn decision_loaded(ws_url: &str) -> anyhow::Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        result: Vec<Info>,
        #[serde(default)]
        error: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Info {
        checkpoint: String,
    }
    let url = ws_url.replacen("ws://", "http://", 1);
    let reply: Reply = reqwest::Client::builder()
        .timeout(RPC_TIMEOUT)
        .build()?
        .post(url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "decision_loaded", "params": []
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(e) = reply.error {
        anyhow::bail!("{e}");
    }
    Ok(reply.result.into_iter().map(|i| i.checkpoint).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// One HTTP/1.1 response with `body`, then the socket closes.
    async fn serve_once(body: &'static [u8]) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/m/f.bin", l.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body).await.unwrap();
        });
        url
    }

    fn entry(body: &[u8], size: u64) -> Entry {
        Entry {
            path: "m/f.bin".into(),
            size,
            sha256: format!("{:x}", Sha256::digest(body)),
            platform: None,
            key: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("theuth-models-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn present(root: &Path, e: &Entry) -> bool {
        matches!(status(&root.join(&e.path), e), Status::Keep)
    }

    #[test]
    fn platform_and_key_fields() {
        let m: Manifest = serde_json::from_str(
            r#"{"files":[
                {"path":"parakeet-tdt/vocab.txt","size":1,"sha256":"00"},
                {"path":"onnxruntime/libonnxruntime.dylib","key":"onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib","platform":"aarch64-apple-darwin","size":2,"sha256":"11"},
                {"path":"onnxruntime/libonnxruntime.so","key":"onnxruntime/x86_64-unknown-linux-gnu/libonnxruntime.so","platform":"x86_64-unknown-linux-gnu","size":3,"sha256":"22"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(m.files[0].key(), "parakeet-tdt/vocab.txt");
        assert!(m.files[0].for_this_platform());
        assert_eq!(
            m.files[1].key(),
            "onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib"
        );
        // Exactly one of the two runtime rows is for this build (whichever it is).
        let mine: Vec<&str> = m.files[1..]
            .iter()
            .filter(|e| e.for_this_platform())
            .map(|e| e.path.as_str())
            .collect();
        assert!(mine.len() <= 1, "{mine:?}");
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(mine, ["onnxruntime/libonnxruntime.dylib"]);
        }
    }

    /// release-models.sh adds `revision` / `origin` (and may add more): the reader takes only what it knows.
    #[test]
    fn manifest_ignores_unknown_fields() {
        let m: Manifest = serde_json::from_str(
            r#"{"files":[{"path":"decision/model.safetensors","size":5,"sha256":"aa",
                "revision":"55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851","origin":"hf:convaiinnovations/laya/model.safetensors",
                "later":{"nested":true}}],"generated":"2026-09-27"}"#,
        )
        .unwrap();
        assert_eq!(m.files.len(), 1);
        assert_eq!(
            (m.files[0].key(), m.files[0].size),
            ("decision/model.safetensors", 5)
        );
        assert!(m.files[0].for_this_platform());
        assert_eq!(m.files[0].checkpoint(), Some("default"));
    }

    #[test]
    fn checkpoint_by_directory() {
        let cp = |p: &str| {
            Entry {
                path: p.into(),
                ..entry(b"", 0)
            }
            .checkpoint()
            .map(str::to_owned)
        };
        assert_eq!(cp("decision/model.safetensors").as_deref(), Some("default"));
        assert_eq!(
            cp("decision-browser/model.safetensors").as_deref(),
            Some("browser")
        );
        assert_eq!(
            cp("decision-gliner/tokenizer.json").as_deref(),
            Some("gliner")
        );
        assert_eq!(cp("parakeet-tdt/vocab.txt"), None);
        assert_eq!(cp("onnxruntime/VERSION"), None);
    }

    #[tokio::test]
    async fn verified_file_lands_and_a_bad_one_leaves_nothing() {
        const BODY: &[u8] = b"hello models";
        let dir = scratch("land");
        let dest = dir.join("m/f.bin");
        let client = reqwest::Client::new();

        let e = entry(BODY, BODY.len() as u64);
        let mut got = 0;
        let part = download(&client, &serve_once(BODY).await, &e, &dest, &mut |n| {
            got += n
        })
        .await
        .unwrap();
        assert!(
            !dest.exists() && part.exists(),
            "verified but not renamed yet"
        );
        land(&part, &dest, &e.sha256).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert_eq!(got, BODY.len() as u64);
        assert!(present(&dir, &e));
        // The sidecar carries the streamed hash: no start hashes this file.
        assert!(
            std::fs::read_to_string(dir.join("m/f.bin.sha256"))
                .unwrap()
                .starts_with(&e.sha256)
        );

        // Size matches, hash does not: rejected, no file, no .part.
        let bad = Entry {
            sha256: "00".repeat(32),
            ..entry(BODY, BODY.len() as u64)
        };
        let dest2 = dir.join("m/g.bin");
        let err = download(&client, &serve_once(BODY).await, &bad, &dest2, &mut |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("manifest says"), "{err}");
        assert!(!dest2.exists());
        assert_eq!(
            std::fs::read_dir(dir.join("m")).unwrap().count(),
            2,
            "f.bin and its sidecar; no .part left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `sync` against a one-file manifest served from `url`'s directory.
    async fn run(
        root: &Path,
        e: Entry,
        url: &str,
        loaded: Vec<String>,
        pending: &mut Vec<Pending>,
    ) -> (Outcome, Vec<String>) {
        let mut states = Vec::new();
        let base = url.rsplit_once('/').unwrap().0;
        let o = sync(
            &reqwest::Client::new(),
            base,
            root,
            &Manifest { files: vec![e] },
            || {
                let l = loaded.clone();
                async move { l }
            },
            pending,
            &mut |p| states.push(serde_json::to_string(p).unwrap()),
        )
        .await
        .unwrap();
        (o, states)
    }

    /// (a) v17s → v19s: same size, other bytes. (b) the sidecar then answers for the new file
    /// without hashing — even when the content changes underneath it (accepted cost).
    /// (e) `verifying` fires once, for the pre-existing file on the first pass only.
    #[tokio::test]
    async fn same_size_replacement_is_fetched_once_and_remembered() {
        const OLD: &[u8] = b"laya-browser v17s";
        const NEW: &[u8] = b"laya-browser v19s";
        let dir = scratch("replace");
        let dest = dir.join("m/f.bin");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, OLD).unwrap();
        let mut pending = Vec::new();

        // Pass 1, manifest still lists the old bytes: hashed once (no sidecar yet), kept.
        let old = entry(OLD, OLD.len() as u64);
        let url = serve_once(NEW).await; // not consumed by this pass
        let (o, states) = run(&dir, old, &url, vec![], &mut pending).await;
        assert_eq!(
            o,
            Outcome {
                verified: 1,
                downloaded: 0
            }
        );
        assert_eq!(states, [r#"{"state":"verifying","file":"m/f.bin"}"#]);
        assert!(dir.join("m/f.bin.sha256").exists());

        // Pass 2, manifest lists the new bytes: the sidecar says "not it", fetched, no re-hash.
        let new = entry(NEW, NEW.len() as u64);
        let (o, states) = run(&dir, new, &url, vec![], &mut pending).await;
        assert_eq!(
            o,
            Outcome {
                verified: 0,
                downloaded: 1
            }
        );
        assert!(
            states.iter().all(|s| !s.contains("verifying")),
            "{states:?}"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), NEW);
        assert!(pending.is_empty());

        // Pass 3: present via the sidecar; nothing hashed, nothing fetched (the server is gone).
        let new = entry(NEW, NEW.len() as u64);
        let (o, states) = run(&dir, new, &url, vec![], &mut pending).await;
        assert_eq!((o, states.len()), (Outcome::default(), 0));

        // (b) Same size and mtime, other bytes: the sidecar still answers. Documented ceiling.
        let mtime = std::fs::metadata(&dest).unwrap().modified().unwrap();
        std::fs::write(&dest, b"laya-browser v20s").unwrap();
        std::fs::File::open(&dest)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(present(&dir, &entry(NEW, NEW.len() as u64)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (c) A manifest row without `sha256` (older manifests): size decides, nothing is hashed.
    #[tokio::test]
    async fn manifest_without_sha256_compares_size_only() {
        let dir = scratch("nosha");
        let dest = dir.join("m/f.bin");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"whatever").unwrap();
        let m: Manifest =
            serde_json::from_str(r#"{"files":[{"path":"m/f.bin","size":8}]}"#).unwrap();
        assert!(present(&dir, &m.files[0]));
        assert!(!present(
            &dir,
            &Entry {
                size: 9,
                ..entry(b"", 0)
            }
        ));
        let mut pending = Vec::new();
        let (o, states) = run(
            &dir,
            entry_without_sha(8),
            "http://127.0.0.1:1/m/f.bin",
            vec![],
            &mut pending,
        )
        .await;
        assert_eq!((o, states.len()), (Outcome::default(), 0));
        assert!(!dir.join("m/f.bin.sha256").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn entry_without_sha(size: u64) -> Entry {
        Entry {
            sha256: String::new(),
            ..entry(b"", size)
        }
    }

    /// (d) A replacement for a loaded checkpoint waits as `.part`; a missing file never waits.
    #[tokio::test]
    async fn loaded_checkpoint_replacement_is_deferred_until_unload() {
        const OLD: &[u8] = b"laya-browser v17s";
        const NEW: &[u8] = b"laya-browser v19s";
        let dir = scratch("defer");
        let path = "decision-browser/model.safetensors";
        let dest = dir.join(path);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, OLD).unwrap();
        let e = || Entry {
            path: path.into(),
            ..entry(NEW, NEW.len() as u64)
        };
        let mut pending = Vec::new();

        let url = serve_once(NEW).await;
        let (o, _) = run(&dir, e(), &url, vec!["browser".into()], &mut pending).await;
        assert_eq!(
            o,
            Outcome {
                verified: 1,
                downloaded: 1
            }
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            OLD,
            "not swapped under the session"
        );
        assert!(dir.join(format!("{path}.part")).exists());
        assert_eq!(pending.len(), 1);

        // Still loaded: still waiting. Unloaded: landed with its sidecar.
        apply_pending(&mut pending, &["browser".to_owned()]).await;
        assert_eq!(
            (pending.len(), std::fs::read(&dest).unwrap()),
            (1, OLD.to_vec())
        );
        apply_pending(&mut pending, &[]).await;
        assert!(pending.is_empty());
        assert_eq!(std::fs::read(&dest).unwrap(), NEW);
        assert!(!dir.join(format!("{path}.part")).exists());
        assert!(present(&dir, &e()));

        // A file that is not there yet lands at once even while the checkpoint is "loaded".
        std::fs::remove_file(&dest).unwrap();
        let url = serve_once(NEW).await;
        let (o, _) = run(&dir, e(), &url, vec!["browser".into()], &mut pending).await;
        assert_eq!(
            o,
            Outcome {
                verified: 0,
                downloaded: 1
            }
        );
        assert!(pending.is_empty());
        assert_eq!(std::fs::read(&dest).unwrap(), NEW);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The JSON the webview matches on (`ui/src/lib/tauri.ts` `ModelsProgress`).
    #[test]
    fn progress_json() {
        let j = |p: &Progress<'_>| serde_json::to_string(p).unwrap();
        assert_eq!(
            j(&Progress::Downloading {
                file: "a/b",
                done: 1,
                total: 2
            }),
            r#"{"state":"downloading","file":"a/b","done":1,"total":2}"#
        );
        assert_eq!(
            j(&Progress::Verifying { file: "a/b" }),
            r#"{"state":"verifying","file":"a/b"}"#
        );
        assert_eq!(j(&Progress::Ready), r#"{"state":"ready"}"#);
        assert_eq!(
            j(&Progress::Failed {
                message: "x".into()
            }),
            r#"{"state":"failed","message":"x"}"#
        );
    }

    /// The daemon's JSON-RPC answer over plain HTTP (smoked 2026-10-04 against `t --headless`:
    /// `{"jsonrpc":"2.0","id":1,"result":[]}`; 401 without `?token=`), names only.
    #[tokio::test]
    async fn decision_loaded_names_checkpoints_and_refuses_nothing_reachable() {
        let url = serve_once(
            br#"{"jsonrpc":"2.0","id":1,"result":[{"checkpoint":"browser","since_secs":4,"rss_mb":903}]}"#,
        )
        .await;
        let ws = url.replacen("http://", "ws://", 1) + "?token=t";
        assert_eq!(decision_loaded(&ws).await.unwrap(), ["browser"]);
        // Nothing listening: an error, which `loaded_checkpoints` turns into "nothing loaded".
        assert!(decision_loaded("ws://127.0.0.1:1/?token=t").await.is_err());
    }
}
