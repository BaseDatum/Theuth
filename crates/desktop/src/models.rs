//! Model files and the ONNX Runtime the daemon loads from `<data_dir>/models/` (43 §4.6,
//! 45 §4.6, 54 §9, meetings 04 §0.1, 57 §2 and §6 step 2c): whatever `models.json` on the
//! release bucket lists and this machine lacks is fetched at start
//! (`scripts/release-models.sh` publishes both). Entries carrying a `platform` are for that
//! rust target triple only — the runtime is published once per platform under
//! `onnxruntime/<triple>/` and lands at `models/onnxruntime/`. Every checkpoint is opened
//! lazily by the daemon, so a file that lands after it started is used on the next
//! speak/dictate/meeting/ask without a restart. The webview hears `models-progress`.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::launch;

/// `models.json`; the files sit beside it under the same directory. `THEUTH_MODELS_URL` overrides.
const URL: &str = "https://s3.rwx.dev/theuth/releases/models/models.json";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-read stall budget; whole-request timeouts do not fit a 900 MB file.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Progress events at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
struct Manifest {
    files: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    /// `<model>/<file>`, relative to `<data_dir>/models/`, where the file lands.
    path: String,
    size: u64,
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
        self.platform.as_deref().is_none_or(|p| p == platform::target::target_triple())
    }
}

/// `models-progress` payload. `done`/`total` are bytes over every missing file.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum Progress<'a> {
    Downloading {
        file: &'a str,
        done: u64,
        total: u64,
    },
    Ready,
    Failed {
        message: String,
    },
}

/// Fetch missing models in the background; silent when nothing is missing.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match ensure(&app).await {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!("models: {n} files downloaded");
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
    });
}

fn emit(app: &AppHandle, p: &Progress<'_>) {
    if let Err(e) = app.emit("models-progress", p) {
        tracing::warn!(error = %e, "emit models-progress");
    }
}

/// Manifest → missing files → download each. Returns how many landed.
async fn ensure(app: &AppHandle) -> anyhow::Result<usize> {
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
    let missing: Vec<&Entry> = manifest
        .files
        .iter()
        .filter(|e| e.for_this_platform() && !present(&root, e))
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    let total: u64 = missing.iter().map(|e| e.size).sum();
    let mut done = 0u64;
    let mut last = Instant::now() - PROGRESS_EVERY;
    for e in &missing {
        // Trust boundary: the manifest names paths under `models/` only.
        for p in [e.path.as_str(), e.key()] {
            anyhow::ensure!(
                Path::new(p)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
                "manifest path {p:?} escapes the models directory"
            );
        }
        tracing::info!("models: downloading {} ({} bytes)", e.path, e.size);
        download(
            &client,
            &format!("{base}/{}", e.key()),
            e,
            &root.join(&e.path),
            &mut |n| {
                done += n;
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    emit(
                        app,
                        &Progress::Downloading {
                            file: &e.path,
                            done,
                            total,
                        },
                    );
                }
            },
        )
        .await
        .with_context(|| e.path.clone())?;
    }
    Ok(missing.len())
}

/// A file of the listed size is taken as the listed file; hashing 2 GB on every start
/// is not worth catching a same-size replacement.
fn present(root: &Path, e: &Entry) -> bool {
    std::fs::metadata(root.join(&e.path)).is_ok_and(|m| m.is_file() && m.len() == e.size)
}

/// Stream `url` to `<dest>.part`, hashing as it lands; rename on a verified size + sha256.
async fn download(
    client: &reqwest::Client,
    url: &str,
    e: &Entry,
    dest: &Path,
    on_chunk: &mut impl FnMut(u64),
) -> anyhow::Result<()> {
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
    tokio::fs::rename(&part, dest).await?;
    Ok(())
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
        assert_eq!(m.files[1].key(), "onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib");
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

    #[tokio::test]
    async fn verified_file_lands_and_a_bad_one_leaves_nothing() {
        const BODY: &[u8] = b"hello models";
        let dir = std::env::temp_dir().join(format!("theuth-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("m/f.bin");
        let client = reqwest::Client::new();

        let e = entry(BODY, BODY.len() as u64);
        let mut got = 0;
        download(&client, &serve_once(BODY).await, &e, &dest, &mut |n| {
            got += n
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert_eq!(got, BODY.len() as u64);
        assert!(present(&dir, &e));

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
        assert!(
            std::fs::read_dir(dir.join("m")).unwrap().count() == 1,
            "no .part left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
