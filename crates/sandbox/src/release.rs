//! "Get `t`" for a target triple (39 §3 assets, as `crates/update` reads
//! them): `latest.json` → `platforms["cli-<os>-<arch>"]` → the tarball,
//! verified against [`update::PUBKEY`] while it streams → the bare `t` entry.
//! The parent downloads and verifies, then `put`s the binary: the target
//! needs neither `curl` nor `minisign`, and the signature check happens once,
//! here.

use std::collections::HashMap;
use std::io::Read;

use anyhow::Context;
use base64::Engine;
use bytes::Bytes;
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

use crate::Progress;

#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: HashMap<String, Platform>,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: String,
}

/// Tauri's encoding: base64 of the minisign text file (`.pub` / `.sig`).
fn tauri_b64(s: &str) -> anyhow::Result<String> {
    let bin = base64::prelude::BASE64_STANDARD.decode(s.trim())?;
    Ok(String::from_utf8(bin)?)
}

/// The manifest key for a target: `cli-linux-x86_64`, `cli-macos-aarch64`.
pub fn platform_key(os: &str, arch: &str) -> String {
    format!("cli-{os}-{arch}")
}

async fn fetch_manifest(client: &reqwest::Client, url: &str) -> anyhow::Result<Manifest> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?
        .json()
        .await
        .with_context(|| format!("GET {url}"))
}

/// The latest release's CLI files for `(os, arch)` — `t` and the helpers
/// shipped beside it — verified. `(file name, bytes)`, `t` first. Whole
/// tarball in memory: a one-shot installer, not a service.
pub async fn fetch(os: &str, arch: &str, out: &Progress) -> anyhow::Result<Vec<(String, Bytes)>> {
    fetch_from(update::DEFAULT_URLS, update::PUBKEY, os, arch, out).await
}

async fn fetch_from(
    urls: &[&str],
    pubkey_b64: &str,
    os: &str,
    arch: &str,
    out: &Progress,
) -> anyhow::Result<Vec<(String, Bytes)>> {
    if urls.is_empty() {
        anyhow::bail!("no release endpoints configured");
    }
    let pk = PublicKey::decode(&tauri_b64(pubkey_b64)?)
        .map_err(|_| anyhow::anyhow!("release pubkey"))?;
    let client = reqwest::Client::new();
    let key = platform_key(os, arch);
    let mut errors: Vec<String> = Vec::new();

    for (idx, &endpoint) in urls.iter().enumerate() {
        let has_next = idx + 1 < urls.len();
        let m = match fetch_manifest(&client, endpoint).await {
            Ok(m) => m,
            Err(e) => {
                if has_next {
                    tracing::debug!(
                        "release: manifest {endpoint} failed ({e:#}); trying next endpoint"
                    );
                } else {
                    tracing::debug!("release: manifest {endpoint} failed: {e:#}");
                }
                errors.push(format!("{endpoint}: {e:#}"));
                continue;
            }
        };
        let Some(p) = m.platforms.get(&key) else {
            if has_next {
                tracing::warn!(
                    "release: {} from {endpoint} has no {key} asset; trying next endpoint",
                    m.version
                );
            } else {
                tracing::warn!("release: {} from {endpoint} has no {key} asset", m.version);
            }
            errors.push(format!(
                "{endpoint}: release {} has no {key} asset",
                m.version
            ));
            continue;
        };
        out.say("binary", format!("downloading {} {key}", m.version));

        let sig = Signature::decode(&tauri_b64(&p.signature)?)
            .map_err(|_| anyhow::anyhow!("release signature"))?;
        let mut verifier = pk
            .verify_stream(&sig)
            .map_err(|_| anyhow::anyhow!("release signature"))?;
        let mut resp = match client
            .get(&p.url)
            .send()
            .await
            .with_context(|| format!("GET {}", p.url))
            .and_then(|r| {
                r.error_for_status()
                    .with_context(|| format!("GET {}", p.url))
            }) {
            Ok(resp) => resp,
            Err(e) => {
                if has_next {
                    tracing::warn!(
                        "release: download {} (from {endpoint}) failed ({e:#}); trying next endpoint",
                        p.url
                    );
                } else {
                    tracing::warn!(
                        "release: download {} (from {endpoint}) failed: {e:#}",
                        p.url
                    );
                }
                errors.push(format!("{endpoint} (payload {}): {e:#}", p.url));
                continue;
            }
        };
        let mut tgz = Vec::new();
        let mut stream_err = None;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    verifier.update(&chunk);
                    tgz.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    stream_err = Some(anyhow::Error::new(e).context(format!("read {}", p.url)));
                    break;
                }
            }
        }
        if let Some(e) = stream_err {
            if has_next {
                tracing::warn!(
                    "release: download {} (from {endpoint}) failed ({e:#}); trying next endpoint",
                    p.url
                );
            } else {
                tracing::warn!(
                    "release: download {} (from {endpoint}) failed: {e:#}",
                    p.url
                );
            }
            errors.push(format!("{endpoint} (payload {}): {e:#}", p.url));
            continue;
        }
        verifier
            .finalize()
            .map_err(|_| anyhow::anyhow!("{}: signature does not verify", p.url))?;
        let files = tokio::task::spawn_blocking(move || extract(&tgz)).await??;
        return Ok(files);
    }

    anyhow::bail!("all release endpoints failed: {}", errors.join("; "))
}

/// The regular entries of a release tarball (bare files at the root, 39), `t` first.
pub fn extract(tgz: &[u8]) -> anyhow::Result<Vec<(String, Bytes)>> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(tgz));
    let mut files = Vec::new();
    for entry in ar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        files.push((name, Bytes::from(buf)));
    }
    files.sort_by_key(|(n, _)| n != "t");
    if files.first().is_none_or(|(n, _)| n != "t") {
        anyhow::bail!("release tarball has no `t` entry");
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgQURCQTVEOTQyQ0QwNzZFNApSV1RrZHRBc2xGMjZyZnliM1d3bmZJYUlsU1AzcTZzWDFRNzRCVmY2eWU0SFE3bEl2eDNsdnZzYQo=";
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUlVUa2R0QXNsRjI2cmE3MXg1bERJVXgzRDB2MXFHTGpSQk8za1VEME9OMnlHSm50ek40NTVXRlJhR2pLK2JWdW9EK0NUMUNLSGVnZFlrM1dsK3Z0Z05hVUMrRmFBTy8zRGd3PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NjkzNjEwCWZpbGU6cGF5bG9hZC50YXIuZ3oJaGFzaGVkCkgwdk1DQkQwNEhYZEtXS1MySnBkdTZTeFRxTm95TThZQm90NURyZDJGZG9HdXY5enREMFNKQ1IyQ1MxWUlSK0w4blQ1YUF2TzlRbE1ycWVuZnBNQUJ3PT0K";
    const TEST_TGZ_B64: &str = "H4sIAAAAAAAA/+3OMQqDQBAF0D3KXkAYA+p5FATTWJiV4O0Vm0CqNDEp3mv+8Jv5JX1fHLqmOfPwnhG3+nWffRttl3JcsC2tj9IvOV/x6h/N47Mq07iWqRruc79svx4EAAAAAAAAAADAR3Z4WsXXACgAAA==";

    struct MockServer {
        base_url: String,
        hits: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl MockServer {
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base_url)
        }
    }

    async fn spawn_mock_server(
        handler: impl Fn(&str) -> (u16, Vec<u8>) + Send + Sync + 'static,
    ) -> MockServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = Arc::clone(&hits);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let hits = Arc::clone(&hits_bg);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        req.extend_from_slice(&buf[..n]);
                        if req.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    hits.fetch_add(1, Ordering::SeqCst);
                    let first_line = String::from_utf8_lossy(&req);
                    let path = first_line
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, body) = handler(path);
                    let reason = match status {
                        200 => "OK",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Error",
                    };
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        MockServer {
            base_url: format!("http://{addr}"),
            hits,
            task,
        }
    }

    #[test]
    fn extracts_t_first_then_helpers() {
        let mut ar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_cksum();
        ar.append_data(&mut h, "theuth-documents-1.0", &b"x"[..])
            .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(3);
        h.set_mode(0o755);
        h.set_cksum();
        ar.append_data(&mut h, "t", &b"ELF"[..]).unwrap();
        let tgz = ar.into_inner().unwrap().finish().unwrap();
        let files = extract(&tgz).unwrap();
        assert_eq!(files[0], ("t".to_owned(), Bytes::from_static(b"ELF")));
        assert_eq!(files[1].0, "theuth-documents-1.0");
        assert!(extract(&[]).is_err());
        assert_eq!(platform_key("linux", "aarch64"), "cli-linux-aarch64");
    }

    #[tokio::test]
    async fn fetch_falls_back_when_primary_fails() {
        let tgz = base64::prelude::BASE64_STANDARD
            .decode(TEST_TGZ_B64)
            .unwrap();

        // Case 1: primary manifest 404 -> falls back to secondary and verifies minisign.
        let primary = spawn_mock_server(|_| (404, b"not found".to_vec())).await;
        let s_holder = Arc::new(std::sync::OnceLock::<String>::new());
        let s_ref = Arc::clone(&s_holder);
        let tgz_s = tgz.clone();
        let secondary = spawn_mock_server(move |path| match path {
            "/latest.json" => (
                200,
                serde_json::json!({
                    "version": "2.2.0",
                    "platforms": {
                        "cli-linux-x86_64": {
                            "url": s_ref.get().unwrap(),
                            "signature": TEST_SIG,
                        }
                    }
                })
                .to_string()
                .into_bytes(),
            ),
            "/payload.tar.gz" => (200, tgz_s.clone()),
            _ => (404, Vec::new()),
        })
        .await;
        s_holder.set(secondary.url("/payload.tar.gz")).unwrap();

        let urls = [primary.url("/latest.json"), secondary.url("/latest.json")];
        let url_refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let files = fetch_from(
            &url_refs,
            TEST_PUBKEY,
            "linux",
            "x86_64",
            &Progress::silent(),
        )
        .await
        .unwrap();
        assert_eq!(
            files,
            vec![("t".to_owned(), Bytes::from_static(b"new-theuth-binary"))]
        );
        assert_eq!(primary.hits(), 1);
        assert_eq!(secondary.hits(), 2);

        // Case 2: primary manifest 200 but payload 500 -> falls back to secondary.
        let p_holder = Arc::new(std::sync::OnceLock::<String>::new());
        let p_ref = Arc::clone(&p_holder);
        let primary_bad_payload = spawn_mock_server(move |path| match path {
            "/latest.json" => (
                200,
                serde_json::json!({
                    "version": "2.2.0",
                    "platforms": {
                        "cli-linux-x86_64": {
                            "url": p_ref.get().unwrap(),
                            "signature": TEST_SIG,
                        }
                    }
                })
                .to_string()
                .into_bytes(),
            ),
            "/payload.tar.gz" => (500, b"boom".to_vec()),
            _ => (404, Vec::new()),
        })
        .await;
        p_holder
            .set(primary_bad_payload.url("/payload.tar.gz"))
            .unwrap();

        let urls2 = [
            primary_bad_payload.url("/latest.json"),
            secondary.url("/latest.json"),
        ];
        let url_refs2: Vec<&str> = urls2.iter().map(String::as_str).collect();
        let files2 = fetch_from(
            &url_refs2,
            TEST_PUBKEY,
            "linux",
            "x86_64",
            &Progress::silent(),
        )
        .await
        .unwrap();
        assert_eq!(
            files2,
            vec![("t".to_owned(), Bytes::from_static(b"new-theuth-binary"))]
        );
        assert_eq!(primary_bad_payload.hits(), 2);
        assert_eq!(secondary.hits(), 4);
    }
}
