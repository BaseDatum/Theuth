//! `<data_dir>/computers/<name>.yaml` (64 §4.7, 79 §4.3): one file per compute
//! this crate created, so `destroy` and the UI know what is ours. May be
//! deleted by hand — the compute then has to be adopted or removed by hand.
//! A VM record carries `vm:` (79 contract §2.6); docker/ssh records do not.
//! `vm:` fields are the 79 contract (§2.6) verbatim; the contract documents them.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::types::{DesktopView, PendingView, SandboxInfo, SandboxState, VmView};
use crate::vm::{ImageRef, Mount, NetPolicy, Resources, VmSnapshotInfo, VmHandle, VmMetrics};
use crate::{DIR, Handle};

/// The record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Computer {
    /// Record name (`--name`, or the host).
    pub name: String,
    /// `"docker"` | `"ssh"` | `"vm"`.
    pub backend: String,
    /// The backend's handle (for a VM: the record name; the live runtime name is `vm.runtime_name`).
    pub handle: Handle,
    /// The engine id the compute joined the mesh as; `None` while a VM is still being created.
    #[serde(default)]
    pub engine: Option<mesh::EngineId>,
    /// When the record was written (bring-up finished for docker/ssh; `Create` for a VM).
    pub created_ms: u64,
    /// The VM half (79 §2.6); `None` for docker/ssh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm: Option<VmRecord>,
}

/// `vm:` of a record — what the VM should be; the runtime is reconciled to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VmRecord {
    /// `microsandbox` | `mock`.
    pub runtime: String,
    pub image: ImageRef,
    pub resources: Resources,
    #[serde(default)]
    pub shares: Vec<Mount>,
    pub desktop: DesktopRecord,
    pub mesh_port: MeshPort,
    #[serde(default)]
    pub network: NetPolicy,
    #[serde(default = "yes")]
    pub autostart: bool,
    pub state: SandboxState,
    #[serde(default)]
    pub pending: PendingView,
    pub setup: Setup,
    #[serde(default)]
    pub snapshots: Vec<VmSnapshotInfo>,
    /// The runtime sandbox name currently backing the record (`lab`, or `lab~` after a restore).
    pub runtime_name: String,
    /// A `t` built here (`--local-t`); bring-up `put`s it instead of the image's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_t: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Last `updated` stamp (state, shares, resources).
    #[serde(default)]
    pub updated_ms: u64,
}

fn yes() -> bool {
    true
}

/// `desktop:` — noVNC published on the host loopback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopRecord {
    pub enabled: bool,
    pub geometry: String,
    /// Host loopback TCP port for guest 6080.
    pub host_port: Option<u16>,
}

/// `mesh_port:` — the iroh UDP port published on the host loopback (79 §6.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshPort {
    pub guest: u16,
    pub host_port: Option<u16>,
}

/// `setup:` — bring-up progress, resumable by `Retry`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Setup {
    pub step: String,
    pub at_ms: u64,
    pub error: Option<String>,
}

/// Bring-up steps for a VM, in order (79 §5.3).
pub const VM_STEPS: [&str; 8] = [
    "create",
    "detect",
    "config",
    "binary",
    "first_run",
    "daemon",
    "pair",
    "verify",
];

/// Unix ms now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Computer {
    /// The VM half, or `Err` naming the backend.
    pub fn vm(&self) -> Result<&VmRecord, String> {
        self.vm
            .as_ref()
            .ok_or_else(|| format!("{} is a {} engine, not a sandbox VM", self.name, self.backend))
    }

    /// Mutable VM half; `Err` for docker/ssh.
    pub fn vm_mut(&mut self) -> Result<&mut VmRecord, String> {
        let name = self.name.clone();
        let backend = self.backend.clone();
        self.vm
            .as_mut()
            .ok_or_else(|| format!("{name} is a {backend} engine, not a sandbox VM"))
    }

    /// The runtime handle for the live sandbox.
    pub fn vm_handle(&self) -> Result<VmHandle, String> {
        Ok(VmHandle {
            name: self.vm()?.runtime_name.clone(),
            runtime_id: None,
        })
    }

    /// The state as the wire sees it (`ready` for docker/ssh — they have no runtime to ask).
    pub fn state(&self) -> SandboxState {
        self.vm
            .as_ref()
            .map_or(SandboxState::Ready, |v| v.state.clone())
    }

    /// Set the VM state and stamp `updated_ms`; a no-op on docker/ssh.
    pub fn set_state(&mut self, state: SandboxState) {
        if let Some(vm) = self.vm.as_mut() {
            vm.state = state;
            vm.updated_ms = now_ms();
        }
    }

    /// `Computer → SandboxInfo` (79 §2.2). `host_engine` is this engine's id;
    /// `metrics` the last sample the actor holds.
    pub fn info(&self, host_engine: &str, metrics: Option<VmMetrics>) -> SandboxInfo {
        SandboxInfo {
            name: self.name.clone(),
            backend: self.backend.clone(),
            state: self.state(),
            engine: self.engine.as_ref().map(|e| e.0.clone()),
            vm: self.vm.as_ref().map(|v| VmView {
                image: v.image.clone(),
                resources: v.resources,
                shares: v.shares.clone(),
                desktop: v.desktop.enabled.then(|| DesktopView {
                    enabled: true,
                    geometry: v.desktop.geometry.clone(),
                    host_port: v.desktop.host_port,
                }),
                network: v.network,
                autostart: v.autostart,
                pending: v.pending,
                metrics,
                snapshots: v.snapshots.clone(),
                mesh_port: v.mesh_port.host_port,
                host_engine: host_engine.to_owned(),
            }),
            created_ms: self.created_ms,
            updated_ms: self.vm.as_ref().map_or(self.created_ms, |v| v.updated_ms),
        }
    }
}

fn path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(DIR).join(format!("{name}.yaml"))
}

/// Write (overwrite) the record atomically: temp file + rename.
pub fn write(data_dir: &Path, rec: &Computer) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir.join(DIR))?;
    let text = serde_yaml::to_string(rec)?;
    let final_path = path(data_dir, &rec.name);
    let tmp = final_path.with_extension(format!("yaml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &final_path)?;
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

/// The records directory with one lock, so the actor and its bring-up tasks
/// read-modify-write a record without losing each other's fields.
#[derive(Clone)]
pub struct Store {
    data_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl Store {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// The data dir the records live under.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// One record, or `Err("no sandbox named …")`.
    pub fn get(&self, name: &str) -> Result<Computer, String> {
        let _g = self.lock.lock();
        read(&self.data_dir, name)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("no sandbox named {name}"))
    }

    /// Every record.
    pub fn load_all(&self) -> Vec<Computer> {
        let _g = self.lock.lock();
        list(&self.data_dir).unwrap_or_else(|e| {
            tracing::warn!("computers: {e:#}");
            Vec::new()
        })
    }

    /// Write one record.
    pub fn save(&self, rec: &Computer) -> Result<(), String> {
        let _g = self.lock.lock();
        write(&self.data_dir, rec).map_err(|e| format!("{e:#}"))
    }

    /// Read-modify-write under the lock; `f` returns what to hand back.
    pub fn update<T>(
        &self,
        name: &str,
        f: impl FnOnce(&mut Computer) -> Result<T, String>,
    ) -> Result<(T, Computer), String> {
        let _g = self.lock.lock();
        let mut rec = read(&self.data_dir, name)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("no sandbox named {name}"))?;
        let t = f(&mut rec)?;
        write(&self.data_dir, &rec).map_err(|e| format!("{e:#}"))?;
        Ok((t, rec))
    }

    /// Remove one record.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let _g = self.lock.lock();
        remove(&self.data_dir, name).map_err(|e| format!("{e:#}"))
    }
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
            engine: Some(mesh::EngineId("abc".into())),
            created_ms: 7,
            vm: None,
        };
        write(&dir, &rec).unwrap();
        assert_eq!(read(&dir, "box").unwrap(), Some(rec.clone()));
        assert_eq!(list(&dir).unwrap(), vec![rec]);
        remove(&dir, "box").unwrap();
        assert_eq!(read(&dir, "box").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_docker_record_loads_and_projects() {
        let old = "name: eng\nbackend: docker\nhandle: { id: abc123 }\nengine: 7f3a\ncreated_ms: 5\n";
        let rec: Computer = serde_yaml::from_str(old).unwrap();
        assert_eq!(rec.vm, None);
        let info = rec.info("host", None);
        assert_eq!(info.state, SandboxState::Ready);
        assert_eq!(info.engine.as_deref(), Some("7f3a"));
        assert!(info.vm.is_none());
    }
}
