//! Paired-device registry: `<data_dir>/link/devices.yaml` (31 §6.3).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity::{b64, unb64};
use crate::types::Device;

/// One YAML row. `pub` is base64url on disk, raw bytes in memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// First 8 chars of `b64url(pub)`.
    pub id: String,
    /// Device static public key.
    #[serde(rename = "pub", with = "pubkey")]
    pub key: [u8; 32],
    /// Display name.
    pub name: String,
    /// `ios` | `macos` | `web` | …
    pub platform: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last conn close; `0` never.
    pub last_seen_ms: u64,
}

impl Row {
    /// UI shape with `online` supplied by the caller.
    pub fn device(&self, online: bool) -> Device {
        Device {
            id: self.id.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            paired_at_ms: self.paired_at_ms,
            last_seen_ms: self.last_seen_ms,
            online,
        }
    }
}

/// `Device.id` for a static public key.
pub fn device_id(key: &[u8; 32]) -> String {
    let mut s = b64(key);
    s.truncate(8);
    s
}

mod pubkey {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(key: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::b64(key))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        super::unb64(&s)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| D::Error::custom("pub: expected 32 base64url bytes"))
    }
}

/// The registry: rows in pairing order plus the file they persist to.
/// Small (`max_devices` 16), so lookups are linear scans.
pub struct Devices {
    path: PathBuf,
    rows: Vec<Row>,
}

impl Devices {
    /// Load `<dir>/devices.yaml`; missing → empty; unparsable → warn and empty.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("devices.yaml");
        let rows = match std::fs::read_to_string(&path) {
            Ok(s) => serde_yaml::from_str::<Option<Vec<Row>>>(&s)
                .unwrap_or_else(|e| {
                    tracing::warn!("{}: {e}; ignoring", path.display());
                    None
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Self { path, rows }
    }

    /// Write every row back.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_yaml::to_string(&self.rows).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, text)
    }

    /// All rows.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Row count.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// No rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Lookup by static key.
    pub fn by_key(&self, key: &[u8; 32]) -> Option<&Row> {
        self.rows.iter().find(|r| &r.key == key)
    }

    /// Lookup by `Device.id`.
    pub fn by_id(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Mutable lookup by `Device.id`.
    pub fn by_id_mut(&mut self, id: &str) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| r.id == id)
    }

    /// Mutable lookup by static key.
    pub fn by_key_mut(&mut self, key: &[u8; 32]) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| &r.key == key)
    }

    /// Append (replacing any row with the same key) and save.
    pub fn insert(&mut self, row: Row) -> std::io::Result<()> {
        self.rows.retain(|r| r.key != row.key);
        self.rows.push(row);
        self.save()
    }

    /// Delete by `Device.id`; returns the row when it existed.
    pub fn remove(&mut self, id: &str) -> std::io::Result<Option<Row>> {
        let Some(i) = self.rows.iter().position(|r| r.id == id) else {
            return Ok(None);
        };
        let row = self.rows.remove(i);
        self.save()?;
        Ok(Some(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip() {
        let dir = std::env::temp_dir().join(format!("theuth-link-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = [7u8; 32];
        let row = Row {
            id: device_id(&key),
            key,
            name: "iPhone".into(),
            platform: "ios".into(),
            paired_at_ms: 1,
            last_seen_ms: 0,
        };
        let mut d = Devices::load(&dir);
        assert!(d.is_empty());
        d.insert(row.clone()).unwrap();
        let text = std::fs::read_to_string(dir.join("devices.yaml")).unwrap();
        assert!(text.contains(&format!("pub: {}", b64(&key))), "{text}");
        let d2 = Devices::load(&dir);
        assert_eq!(d2.rows(), std::slice::from_ref(&row));
        assert_eq!(d2.by_key(&key), Some(&row));
        assert_eq!(row.id.len(), 8);
        let mut d2 = d2;
        assert_eq!(d2.remove(&row.id).unwrap(), Some(row));
        assert!(Devices::load(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
