//! User + learned entries and negatives (153 §5.5, A2 variant): one JSON file
//! `<data_dir>/voice/dictionary.json`, rewritten atomically (temp + rename) on
//! every change. The user layer is small; a full rewrite is cheaper than a db.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Entry, Negative};

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
    #[serde(default)]
    negatives: Vec<Negative>,
}

/// `<data_dir>/voice/dictionary.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("voice").join("dictionary.json")
}

/// Read the file; a missing file is an empty layer.
pub fn read(path: &Path) -> anyhow::Result<(Vec<Entry>, Vec<Negative>)> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let f: File = serde_json::from_slice(&bytes)?;
            Ok((f.entries, f.negatives))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), Vec::new())),
        Err(e) => Err(e.into()),
    }
}

/// Write `<path>.tmp` then rename over `path`.
pub fn write(path: &Path, entries: &[Entry], negatives: &[Negative]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = File {
        version: 1,
        entries: entries.to_vec(),
        negatives: negatives.to_vec(),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
