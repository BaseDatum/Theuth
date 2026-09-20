//! Paired-node registry: `<data_dir>/nodes.yaml` (32 §6.4). Deletable: every
//! node then lists a device that never returns; `revoke_pending` is the only
//! non-derivable state.

use std::path::{Path, PathBuf};

use link::identity::{b64, unb64};
use serde::{Deserialize, Serialize};

use crate::types::Node;

/// One YAML row. `pub` is base64url on disk, raw bytes in memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// `b64url(pub)`, 43 chars.
    pub id: String,
    /// Node static public key.
    #[serde(rename = "pub", with = "pubkey")]
    pub key: [u8; 32],
    /// Local alias.
    pub name: String,
    /// `ws(s)://host[:port]`.
    pub relay: String,
    /// From `info`; `""` until the first connect.
    #[serde(default)]
    pub platform: String,
    /// From `info`; `""` until the first connect.
    #[serde(default)]
    pub version: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last control-conn close; `0` never.
    #[serde(default)]
    pub last_seen_ms: u64,
    /// Device ids revoked here while the node was offline (32 §8.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoke_pending: Vec<String>,
    /// The node refused our `IK` (32 §6.3). Not persisted: the next start retries.
    #[serde(skip)]
    pub revoked: bool,
}

impl Row {
    /// UI shape with the live bits supplied by the caller.
    pub fn node(&self, online: bool, children: u32) -> Node {
        Node {
            id: self.id.clone(),
            name: self.name.clone(),
            relay: self.relay.clone(),
            platform: self.platform.clone(),
            version: self.version.clone(),
            paired_at_ms: self.paired_at_ms,
            last_seen_ms: self.last_seen_ms,
            online,
            revoked: self.revoked,
            children,
        }
    }
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
/// A handful of rows, so lookups are linear scans.
pub struct Store {
    path: PathBuf,
    rows: Vec<Row>,
}

impl Store {
    /// Load `<data_dir>/nodes.yaml`; missing → empty; unparsable → warn and empty.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("nodes.yaml");
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

    /// Lookup by node id.
    pub fn by_id(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Mutable lookup by node id.
    pub fn by_id_mut(&mut self, id: &str) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| r.id == id)
    }

    /// Append (replacing any row with the same id) and save.
    pub fn insert(&mut self, row: Row) -> std::io::Result<()> {
        self.rows.retain(|r| r.id != row.id);
        self.rows.push(row);
        self.save()
    }

    /// Delete by node id; returns the row when it existed.
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
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("theuth-nodes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir);
        assert!(s.rows().is_empty());
        let key = [7u8; 32];
        s.insert(Row {
            id: b64(&key),
            key,
            name: "build-box".into(),
            relay: "wss://relay.example".into(),
            platform: String::new(),
            version: String::new(),
            paired_at_ms: 1,
            last_seen_ms: 0,
            revoke_pending: vec!["abcdefgh".into()],
            revoked: true,
        })
        .unwrap();
        let text = std::fs::read_to_string(dir.join("nodes.yaml")).unwrap();
        assert!(text.contains(&format!("pub: {}", b64(&key))), "{text}");
        assert!(!text.contains("revoked"), "{text}");
        let again = Store::load(&dir);
        let r = again.by_id(&b64(&key)).unwrap();
        assert_eq!(
            (r.key, r.name.as_str(), r.revoked),
            (key, "build-box", false)
        );
        assert_eq!(r.revoke_pending, ["abcdefgh"]);
        let mut again = again;
        assert!(again.remove(&b64(&key)).unwrap().is_some());
        assert!(Store::load(&dir).rows().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
