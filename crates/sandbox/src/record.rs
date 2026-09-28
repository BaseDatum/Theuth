//! `<data_dir>/computers/<name>.yaml` (64 §4.7): one file per compute this
//! crate created, so `destroy` and the UI know what is ours. May be deleted by
//! hand — the compute then has to be adopted or removed by hand.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DIR, Handle};

/// The record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computer {
    /// Record name (`--name`, or the host).
    pub name: String,
    /// `"docker"` | `"ssh"`.
    pub backend: String,
    /// The backend's handle.
    pub handle: Handle,
    /// The engine id the compute joined the mesh as.
    pub engine: mesh::EngineId,
    /// When bring-up finished.
    pub created_ms: u64,
}

fn path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(DIR).join(format!("{name}.yaml"))
}

/// Write (overwrite) the record.
pub fn write(data_dir: &Path, rec: &Computer) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir.join(DIR))?;
    let text = serde_yaml::to_string(rec)?;
    std::fs::write(path(data_dir, &rec.name), text)?;
    Ok(())
}

/// Read one record; `None` when there is none.
pub fn read(data_dir: &Path, name: &str) -> anyhow::Result<Option<Computer>> {
    match std::fs::read_to_string(path(data_dir, name)) {
        Ok(text) => Ok(Some(serde_yaml::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Remove one record; missing is fine.
pub fn remove(data_dir: &Path, name: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(path(data_dir, name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Every parseable record, by name.
pub fn list(data_dir: &Path) -> anyhow::Result<Vec<Computer>> {
    let dir = data_dir.join(DIR);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in rd {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e == "yaml") {
            match std::fs::read_to_string(&p)
                .map_err(anyhow::Error::from)
                .and_then(|t| Ok(serde_yaml::from_str(&t)?))
            {
                Ok(rec) => out.push(rec),
                Err(e) => tracing::warn!("{}: {e:#}", p.display()),
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_list() {
        let dir = std::env::temp_dir().join(format!("sandbox-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rec = Computer {
            name: "box".into(),
            backend: "ssh".into(),
            handle: Handle {
                id: "me@box:22".into(),
            },
            engine: mesh::EngineId("abc".into()),
            created_ms: 7,
        };
        write(&dir, &rec).unwrap();
        assert_eq!(read(&dir, "box").unwrap(), Some(rec.clone()));
        assert_eq!(list(&dir).unwrap(), vec![rec]);
        remove(&dir, "box").unwrap();
        assert_eq!(read(&dir, "box").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
