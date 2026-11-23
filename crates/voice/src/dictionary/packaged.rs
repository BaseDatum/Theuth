//! The packaged technical dataset (153 §5.1–5.2): `data/dictionary.v1.json.gz`
//! shipped in the binary via `include_bytes!` (340 KiB, under the 1.5 MiB
//! budget), verified once at load against the sha256 in `data/dictionary.json`
//! — the record `crates/voice/build-dictionary/build.py` writes.

use std::io::Read;

use serde::Deserialize;

use super::matcher::Index;
use super::{Entry, EntryId, Kind, Scope, Source};

const GZ: &[u8] = include_bytes!("../../data/dictionary.v1.json.gz");
const RECORD: &str = include_str!("../../data/dictionary.json");

/// `data/dictionary.json`: what the build tool produced and the loader checks.
#[derive(Debug, Deserialize)]
pub struct Record {
    /// Data format version (1).
    pub version: u32,
    /// `dictionary.v1.json.gz`.
    pub file: String,
    /// Hex sha256 of the gz.
    pub sha256: String,
    /// Size of the gz.
    pub bytes: usize,
    /// Canonical terms.
    pub terms: usize,
    /// Spoken-form pairs (symbols, formatters, abbreviations, number words).
    pub pairs: usize,
    /// ISO-8601 UTC.
    pub built_at: String,
}

#[derive(Deserialize)]
struct Set {
    version: u32,
    built_at: String,
    terms: Vec<Term>,
}

/// One packed term: canonical, kind, spoken forms, tags (`build.py` shape).
#[derive(Deserialize)]
struct Term {
    c: String,
    k: Kind,
    #[serde(default)]
    s: Vec<String>,
    #[serde(default)]
    t: Vec<String>,
}

/// The read-only layer: entries in catalog order plus their index.
#[derive(Debug, Default)]
pub struct Packaged {
    /// `v1 2026-10-09` — for `voice_status` (§5.6).
    pub version: String,
    /// Catalog order; ids `pkg-<n>`.
    pub entries: Vec<Entry>,
    /// §5.4 step-7 buckets over `entries`.
    pub index: Index,
}

/// The committed record.
pub fn record() -> anyhow::Result<Record> {
    Ok(serde_json::from_str(RECORD)?)
}

/// Hex sha256 of the embedded file.
pub fn sha256() -> String {
    platform::sha256::hex(GZ)
}

/// Verify the hash (proof row 3), gunzip, index.
pub fn load() -> anyhow::Result<Packaged> {
    let rec = record()?;
    anyhow::ensure!(
        rec.bytes == GZ.len() && rec.sha256 == sha256(),
        "packaged dictionary {} does not match its record (bytes {} vs {})",
        rec.file,
        GZ.len(),
        rec.bytes
    );
    let mut json = Vec::with_capacity(GZ.len() * 8);
    flate2::read::GzDecoder::new(GZ).read_to_end(&mut json)?;
    let set: Set = serde_json::from_slice(&json)?;
    anyhow::ensure!(
        set.version == rec.version,
        "packaged dictionary version mismatch"
    );
    let entries: Vec<Entry> = set
        .terms
        .into_iter()
        .enumerate()
        .map(|(i, t)| Entry {
            id: EntryId(format!("pkg-{i}")),
            canonical: t.c,
            kind: t.k,
            spoken: t.s,
            variants: Vec::new(),
            scope: Scope::Global,
            source: Source::Packaged,
            tags: t.t,
        })
        .collect();
    let index = Index::build(&entries);
    Ok(Packaged {
        version: format!(
            "v{} {}",
            set.version,
            set.built_at.get(..10).unwrap_or(&set.built_at)
        ),
        entries,
        index,
    })
}
