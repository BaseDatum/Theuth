//! Node records: `<data_dir>/cloudnodes/<name>.json`, one per node this engine created (design
//! doc 224 §13.1). What was asked, what the provider made, where the create job is, the mesh peer
//! once joined, how its vault is protected. Never a credential, the pairing ticket or the vault's
//! master password.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::{CloudError, Created, Result, check_name};
use crate::types::{NodeSpec, NodeState, Stage, VaultProtection};

/// One node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// What the user asked for.
    pub spec: NodeSpec,
    /// What `Provider::create` returned; `None` until it did.
    #[serde(default)]
    pub created: Option<Created>,
    /// Lifecycle.
    pub state: NodeState,
    /// While `Creating`: the job's stage.
    #[serde(default)]
    pub stage: Option<Stage>,
    /// Why the job stopped (`Failed`).
    #[serde(default)]
    pub error: Option<String>,
    /// Its mesh endpoint id once it joined.
    #[serde(default)]
    pub peer_id: Option<String>,
    /// Mesh peer ids before the ticket was minted: the node is the *new* peer with its name.
    #[serde(default)]
    pub peers_before: Vec<String>,
    /// When its ticket stops working (Unix ms); the create job gives up after it.
    #[serde(default)]
    pub ticket_expires_ms: Option<u64>,
    /// When the record was made (Unix ms).
    pub created_ms: u64,
    /// How the node's vault is protected (`Pending` while the create job holds a password).
    #[serde(default)]
    pub vault: Option<VaultProtection>,
    /// Phone-only nodes (224 §16.12): the link claim's public half; its PSK is in the vault
    /// (`cloud/link-claim.<name>`) until the app paired.
    #[serde(default)]
    pub link: Option<crate::types::NodeLink>,
}

/// The directory of records; every write is a whole-file replace (temp file + rename).
#[derive(Clone, Debug)]
pub struct Records {
    dir: PathBuf,
}

impl Records {
    /// Records under `dir` (created on first write).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        check_name(name)?;
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// One record; `None` when there is none.
    pub fn get(&self, name: &str) -> Result<Option<Record>> {
        let path = self.path(name)?;
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| CloudError(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CloudError(format!("{}: {e}", path.display()))),
        }
    }

    /// Every readable record, by name. An unreadable file is skipped with a warning.
    pub fn all(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Record> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let path = e.path();
                match std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
                {
                    Ok(r) => Some(r),
                    Err(e) => {
                        tracing::warn!("cloudnodes: skipping {}: {e}", path.display());
                        None
                    }
                }
            })
            .collect();
        out.sort_by(|a: &Record, b: &Record| a.spec.name.cmp(&b.spec.name));
        out
    }

    /// Write (create or replace) a record.
    pub fn put(&self, r: &Record) -> Result<()> {
        let path = self.path(&r.spec.name)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| CloudError(format!("{}: {e}", self.dir.display())))?;
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(r).map_err(|e| CloudError(e.to_string()))?;
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| CloudError(format!("{}: {e}", path.display())))
    }

    /// Delete a record (no error when absent).
    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CloudError(format!("{}: {e}", path.display()))),
        }
    }
}
