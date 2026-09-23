//! [`Kind::Archive`](crate::Kind::Archive): what is inside a zip / tar(.gz|.bz2)
//! without extracting it (35 §3.2). The listing is computed once at show time
//! from headers only — zip's central directory, tar's entry headers — and kept
//! as a `<blob>.listing.json` sidecar beside the blob, never in the record.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Listing cap; past it `truncated` is set and the rest is counted only.
pub const MAX_ENTRIES: usize = 5_000;

/// Container / compression, by file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `.zip`.
    Zip,
    /// `.tar`.
    Tar,
    /// `.tar.gz`, `.tgz`.
    TarGz,
    /// `.tar.bz2`, `.tbz2`.
    TarBz2,
}

/// The archive format `name` announces, or `None` for anything else. Double
/// suffixes (`.tar.gz`) are checked before the last extension.
pub fn format(name: &str) -> Option<Format> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(Format::TarGz)
    } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
        Some(Format::TarBz2)
    } else if lower.ends_with(".tar") {
        Some(Format::Tar)
    } else if lower.ends_with(".zip") {
        Some(Format::Zip)
    } else {
        None
    }
}

/// One member, as its header describes it. `path` is display data: never a
/// link target or a filesystem path (28).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ArchiveEntry {
    /// Member path as stored (`/`-separated).
    pub path: String,
    /// Uncompressed size.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// Stored size inside the archive (zip only).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub compressed: Option<u64>,
    /// Unix seconds.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub mtime: Option<i64>,
    /// A directory entry.
    pub is_dir: bool,
    /// Unix permission bits (`0o7777` masked: no file-type bits) when the
    /// archive carries them.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub mode: Option<u32>,
}

/// The sidecar: up to [`MAX_ENTRIES`] entries and the totals over all of them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ArchiveListing {
    /// The first [`MAX_ENTRIES`] members, in archive order.
    pub entries: Vec<ArchiveEntry>,
    /// Members in the archive, listed or not.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_entries: u64,
    /// Sum of uncompressed sizes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_size: u64,
    /// `total_entries` exceeded [`MAX_ENTRIES`]: `entries` is a prefix.
    pub truncated: bool,
}

impl ArchiveListing {
    fn push(&mut self, e: ArchiveEntry) {
        self.total_entries += 1;
        self.total_size += e.size;
        if self.entries.len() < MAX_ENTRIES {
            self.entries.push(e);
        } else {
            self.truncated = true;
        }
    }
}

/// `<blob>.listing.json` beside the blob; shares the blob's stem so the sweep keeps it.
pub fn sidecar(blob: &Path) -> PathBuf {
    let mut s = blob.as_os_str().to_owned();
    s.push(".listing.json");
    PathBuf::from(s)
}

/// List `bytes` as the format `name` announces. Headers only: zip walks the
/// central directory, tar streams entry headers (a `.gz`/`.bz2` wrapper is
/// decoded, the member bodies are skipped, nothing is written anywhere).
pub fn list(bytes: &[u8], name: &str) -> anyhow::Result<ArchiveListing> {
    match format(name) {
        Some(Format::Zip) => zip_listing(bytes),
        Some(Format::Tar) => tar_listing(Cursor::new(bytes)),
        Some(Format::TarGz) => tar_listing(flate2::read::GzDecoder::new(bytes)),
        Some(Format::TarBz2) => tar_listing(bzip2::read::BzDecoder::new(bytes)),
        None => anyhow::bail!("{name}: not an archive"),
    }
}

fn zip_listing(bytes: &[u8]) -> anyhow::Result<ArchiveListing> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = ArchiveListing::default();
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i)?;
        let mtime = f.last_modified().and_then(|d| {
            chrono::NaiveDate::from_ymd_opt(d.year().into(), d.month().into(), d.day().into())?
                .and_hms_opt(d.hour().into(), d.minute().into(), d.second().into())
                .map(|t| t.and_utc().timestamp())
        });
        out.push(ArchiveEntry {
            path: f.name().to_owned(),
            size: f.size(),
            compressed: Some(f.compressed_size()),
            mtime,
            is_dir: f.is_dir(),
            mode: f.unix_mode().map(|m| m & 0o7777),
        });
    }
    Ok(out)
}

fn tar_listing<R: Read>(reader: R) -> anyhow::Result<ArchiveListing> {
    let mut tar = tar::Archive::new(reader);
    let mut out = ArchiveListing::default();
    for entry in tar.entries()? {
        let entry = entry?;
        let h = entry.header();
        out.push(ArchiveEntry {
            path: entry.path()?.to_string_lossy().into_owned(),
            size: entry.size(),
            compressed: None,
            mtime: h.mtime().ok().and_then(|t| i64::try_from(t).ok()),
            is_dir: h.entry_type().is_dir(),
            mode: h.mode().ok().map(|m| m & 0o7777),
        });
    }
    Ok(out)
}

/// Read the sidecar of `blob`; `Err` when it is missing (a corrupt archive at
/// show time) or unreadable.
pub async fn read_listing(blob: &Path) -> Result<ArchiveListing, String> {
    let path = sidecar(blob);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("{}: no listing: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    pub(crate) fn zip_fixture(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        for (name, body) in files {
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(body).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    pub(crate) fn tar_gz_fixture(files: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o600);
            h.set_mtime(1_700_000_000);
            h.set_cksum();
            b.append_data(&mut h, name, *body).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn formats_by_suffix() {
        assert_eq!(format("a.tar.gz"), Some(Format::TarGz));
        assert_eq!(format("A.TGZ"), Some(Format::TarGz));
        assert_eq!(format("a.tar.bz2"), Some(Format::TarBz2));
        assert_eq!(format("a.tar"), Some(Format::Tar));
        assert_eq!(format("a.zip"), Some(Format::Zip));
        assert_eq!(format("a.gz"), None);
        assert_eq!(format("tar.md"), None);
        assert_eq!(
            sidecar(Path::new("/d/abc.zip")),
            PathBuf::from("/d/abc.zip.listing.json")
        );
    }

    #[test]
    fn zip_lists_without_inflating() {
        let bytes = zip_fixture(&[("dir/", b""), ("dir/a.txt", b"hello"), ("b.bin", &[0u8; 300])]);
        let l = list(&bytes, "x.zip").unwrap();
        assert_eq!(l.total_entries, 3);
        assert_eq!(l.total_size, 305);
        assert!(!l.truncated);
        assert_eq!(l.entries[0].path, "dir/");
        assert!(l.entries[0].is_dir);
        assert_eq!(l.entries[1].path, "dir/a.txt");
        assert_eq!(l.entries[1].size, 5);
        assert_eq!(l.entries[1].compressed, Some(5));
        assert_eq!(l.entries[1].mode, Some(0o644));
        assert!(l.entries[1].mtime.is_some_and(|t| t > 0));
    }

    #[test]
    fn tar_gz_lists_headers_only() {
        let bytes = tar_gz_fixture(&[("src/main.rs", b"fn main() {}"), ("README", b"hi")]);
        let l = list(&bytes, "src.tar.gz").unwrap();
        assert_eq!(l.total_entries, 2);
        assert_eq!(l.total_size, 14);
        assert_eq!(l.entries[0].path, "src/main.rs");
        assert_eq!(l.entries[0].size, 12);
        assert_eq!(l.entries[0].compressed, None);
        assert_eq!(l.entries[0].mtime, Some(1_700_000_000));
        assert_eq!(l.entries[0].mode, Some(0o600));
        assert!(!l.entries[1].is_dir);
    }

    #[test]
    fn listing_caps_at_max_entries() {
        let names: Vec<String> = (0..MAX_ENTRIES + 7).map(|i| format!("f{i}")).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        let l = list(&zip_fixture(&files), "many.zip").unwrap();
        assert_eq!(l.entries.len(), MAX_ENTRIES);
        assert_eq!(l.total_entries, (MAX_ENTRIES + 7) as u64);
        assert_eq!(l.total_size, (MAX_ENTRIES + 7) as u64);
        assert!(l.truncated);
    }

    #[test]
    fn corrupt_is_an_error() {
        assert!(list(b"not a zip", "x.zip").is_err());
        assert!(list(b"not gzip", "x.tar.gz").is_err());
        assert!(list(b"x", "x.txt").is_err());
    }
}
