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

/// The latest release's CLI files for `(os, arch)` — `t` and the helpers
/// shipped beside it — verified. `(file name, bytes)`, `t` first. Whole
/// tarball in memory: a one-shot installer, not a service.
pub async fn fetch(os: &str, arch: &str, out: &Progress) -> anyhow::Result<Vec<(String, Bytes)>> {
    let client = reqwest::Client::new();
    let m: Manifest = client
        .get(update::URL)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("GET {}", update::URL))?
        .json()
        .await
        .context("latest.json")?;
    let key = platform_key(os, arch);
    let p = m
        .platforms
        .get(&key)
        .with_context(|| format!("release {} has no {key} asset", m.version))?;
    out.say("binary", format!("downloading {} {key}", m.version));

    let pk = PublicKey::decode(&tauri_b64(update::PUBKEY)?).map_err(|_| anyhow::anyhow!("release pubkey"))?;
    let sig = Signature::decode(&tauri_b64(&p.signature)?).map_err(|_| anyhow::anyhow!("release signature"))?;
    let mut verifier = pk.verify_stream(&sig).map_err(|_| anyhow::anyhow!("release signature"))?;
    let mut resp = client
        .get(&p.url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("GET {}", p.url))?;
    let mut tgz = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        verifier.update(&chunk);
        tgz.extend_from_slice(&chunk);
    }
    verifier
        .finalize()
        .map_err(|_| anyhow::anyhow!("{}: signature does not verify", p.url))?;
    let files = tokio::task::spawn_blocking(move || extract(&tgz)).await??;
    Ok(files)
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

    #[test]
    fn extracts_t_first_then_helpers() {
        let mut ar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_cksum();
        ar.append_data(&mut h, "theuth-documents-1.0", &b"x"[..]).unwrap();
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
}
