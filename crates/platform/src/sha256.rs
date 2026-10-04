//! sha256 of a large file, remembered beside it so the hashing pass runs once per download,
//! not on every open: `<path>.sha256` holds `<hex> <size> <mtime_secs>\n`. A size or mtime
//! mismatch rehashes. Shared by the `decision` loader (57 §2.1, the log's model tag) and the
//! desktop model downloader (`crates/desktop/src/models.rs`, `present()` by hash), so the two
//! never disagree about the same file. Accepted cost: a file rewritten in place with the same
//! size and mtime second keeps its old sidecar.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

/// `<path>.sha256`.
fn sidecar_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

/// `<size> <mtime_secs>` of `path` as it is now.
fn stamp(path: &Path) -> io::Result<String> {
    let meta = std::fs::metadata(path)?;
    Ok(format!(
        "{} {}",
        meta.len(),
        meta.modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    ))
}

/// The remembered hex when the sidecar matches the file's current size and mtime; `None`
/// when there is none or it is stale (the caller will hash).
pub fn sidecar(path: &Path) -> io::Result<Option<String>> {
    let stamp = stamp(path)?;
    Ok(std::fs::read_to_string(sidecar_path(path))
        .ok()
        .and_then(|text| {
            let (hex, rest) = text.trim().split_once(' ')?;
            (rest == stamp && hex.len() == 64).then(|| hex.to_owned())
        }))
}

/// Write the sidecar for `path` from a hex the caller already has (the downloader hashed
/// while streaming). Best effort: a failure is a debug line, the next open hashes.
pub fn remember(path: &Path, hex: &str) {
    let sidecar = sidecar_path(path);
    let r = stamp(path).and_then(|stamp| std::fs::write(&sidecar, format!("{hex} {stamp}\n")));
    if let Err(e) = r {
        tracing::debug!("cannot write {}: {e}", sidecar.display());
    }
}

/// sha256 hex of `path`: the sidecar when it is current, else hash and remember.
pub fn cached(path: &Path) -> io::Result<String> {
    if let Some(hex) = sidecar(path)? {
        return Ok(hex);
    }
    let t = Instant::now();
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
    let hex = format!("{:x}", hasher.finalize());
    tracing::debug!(
        "{} sha256 {}… in {:?}",
        path.display(),
        &hex[..12],
        t.elapsed()
    );
    remember(path, &hex);
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_format_and_short_circuit() {
        let d = std::env::temp_dir().join(format!("platform-sha256-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("w.bin");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sidecar(&p).unwrap(), None);
        let hex = cached(&p).unwrap();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mtime = std::fs::metadata(&p)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(
            std::fs::read_to_string(d.join("w.bin.sha256")).unwrap(),
            format!("{hex} 3 {mtime}\n")
        );
        // Same size and mtime: the sidecar answers, the content is not read.
        let mtime_sys = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::fs::write(&p, b"xyz").unwrap();
        std::fs::File::open(&p)
            .unwrap()
            .set_modified(mtime_sys)
            .unwrap();
        assert_eq!(cached(&p).unwrap(), hex);
        // A size change rehashes and rewrites.
        std::fs::write(&p, b"abcd").unwrap();
        assert_eq!(sidecar(&p).unwrap(), None);
        let hex2 = cached(&p).unwrap();
        assert_ne!(hex2, hex);
        assert!(
            std::fs::read_to_string(d.join("w.bin.sha256"))
                .unwrap()
                .starts_with(&hex2)
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
