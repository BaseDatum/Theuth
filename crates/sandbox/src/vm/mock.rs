//! `MockVmRuntime`: an in-memory [`VmRuntime`] for tests — one state machine
//! per VM, canned `exec` output by command prefix, stored `put` bytes and a
//! call log (`calls()`) that the actor's tests assert the sequence on.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;

use super::{
    Blocker, HostFacts, LogLine, Mount, Pending, Port, Preflight, ResizeOutcome, ResizeStatus,
    ResourceChange, Resources, RuntimeState, SnapshotId, VmSnapshotInfo, VirtState, VmHandle,
    VmMetrics, VmRuntime, VmSpec, VmStatus,
};
use crate::types::VmDefaults;
use crate::{Cmd, ExecOutput, Progress};

/// One mock VM: its spec as the runtime currently applies it, its state,
/// files written with `put`, ceiling changes waiting for the next `start`.
#[derive(Clone, Debug)]
pub struct MockVm {
    pub spec: VmSpec,
    pub status: VmStatus,
    pub files: BTreeMap<String, (Bytes, u32)>,
    pub next_start: ResourceChange,
    /// Mounts or ports changed since the last boot.
    pub pending: bool,
}

#[derive(Default)]
struct Inner {
    vms: BTreeMap<String, MockVm>,
    /// Keyed by `SnapshotId.0`.
    snapshots: BTreeMap<String, VmSnapshotInfo>,
    /// `(command prefix, output)`, first match wins.
    canned: Vec<(String, ExecOutput)>,
    preflight: Option<Preflight>,
    calls: Vec<String>,
}

/// See the module doc.
#[derive(Default)]
pub struct MockVmRuntime {
    inner: Mutex<Inner>,
}

impl MockVmRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// What `preflight()` answers; default [`MockVmRuntime::ok_preflight`].
    pub fn with_preflight(self, p: Preflight) -> Self {
        self.inner.lock().preflight = Some(p);
        self
    }

    /// A green preflight: runtime installed, virtualization ok, 8 cores / 16 GiB.
    pub fn ok_preflight() -> Preflight {
        let host = HostFacts {
            os: "linux".into(),
            arch: "aarch64".into(),
            cores: 8,
            mem_bytes: 16 << 30,
            disk_free_bytes: 200 << 30,
        };
        Preflight {
            runtime: RuntimeState::Installed {
                version: "mock".into(),
                home: "/tmp/mock-msb".into(),
                libkrunfw: "mock".into(),
            },
            virt: VirtState::Ok { via: "mock".into() },
            defaults: Resources::derive(&host, &VmDefaults::default()),
            host,
            blockers: Vec::new(),
            warnings: Vec::new(),
            checked_at_ms: now_ms(),
        }
    }

    /// One blocker, for the "host cannot" paths.
    pub fn blocked_preflight(code: &str, message: &str) -> Preflight {
        let mut p = Self::ok_preflight();
        p.blockers.push(Blocker {
            code: code.into(),
            message: message.into(),
            fix: None,
            installable: false,
        });
        p
    }

    /// `exec` of a command line starting with `prefix` returns `out`.
    pub fn canned(&self, prefix: impl Into<String>, out: ExecOutput) {
        self.inner.lock().canned.push((prefix.into(), out));
    }

    /// Every runtime call so far, in order (`"create lab"`, `"exec lab: uname -sm"`, …).
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().calls.clone()
    }

    /// A VM by name (also `Gone` ones, until the runtime is dropped).
    pub fn vm(&self, name: &str) -> Option<MockVm> {
        self.inner.lock().vms.get(name).cloned()
    }
}

impl Inner {
    fn log(&mut self, s: String) {
        self.calls.push(s);
    }

    fn live(&mut self, h: &VmHandle) -> anyhow::Result<&mut MockVm> {
        match self.vms.get_mut(&h.name) {
            Some(vm) if vm.status != VmStatus::Gone => Ok(vm),
            _ => anyhow::bail!("no such vm: {}", h.name),
        }
    }

    fn insert(&mut self, spec: &VmSpec) -> VmHandle {
        self.vms.insert(
            spec.name.clone(),
            MockVm {
                spec: spec.clone(),
                status: VmStatus::Running,
                files: BTreeMap::new(),
                next_start: ResourceChange::default(),
                pending: false,
            },
        );
        handle(&spec.name)
    }
}

fn handle(name: &str) -> VmHandle {
    VmHandle {
        name: name.to_owned(),
        runtime_id: Some(format!("mock-{name}")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[async_trait::async_trait]
impl VmRuntime for MockVmRuntime {
    fn kind(&self) -> &'static str {
        "mock"
    }

    async fn preflight(&self) -> Preflight {
        let mut g = self.inner.lock();
        g.log("preflight".into());
        g.preflight.clone().unwrap_or_else(Self::ok_preflight)
    }

    async fn install(&self, out: &Progress) -> anyhow::Result<()> {
        self.inner.lock().log("install".into());
        out.say("runtime", "mock runtime installed");
        Ok(())
    }

    async fn create(&self, spec: &VmSpec, out: &Progress) -> anyhow::Result<VmHandle> {
        let mut g = self.inner.lock();
        g.log(format!("create {}", spec.name));
        out.say("create", format!("mock vm {} running", spec.name));
        Ok(g.insert(spec))
    }

    async fn status(&self, h: &VmHandle) -> anyhow::Result<VmStatus> {
        let mut g = self.inner.lock();
        g.log(format!("status {}", h.name));
        Ok(g.vms.get(&h.name).map_or(VmStatus::Gone, |vm| vm.status))
    }

    async fn start(&self, h: &VmHandle, _out: &Progress) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("start {}", h.name));
        let vm = g.live(h)?;
        match vm.status {
            VmStatus::Stopped | VmStatus::Crashed => {
                let n = std::mem::take(&mut vm.next_start);
                let s = &mut vm.spec;
                s.max_cpus = n.max_cpus.unwrap_or(s.max_cpus);
                s.max_memory_mib = n.max_memory_mib.unwrap_or(s.max_memory_mib);
                s.cpus = n.cpus.unwrap_or(s.cpus);
                s.memory_mib = n.memory_mib.unwrap_or(s.memory_mib);
                s.root_disk_mib = n.root_disk_mib.unwrap_or(s.root_disk_mib);
                vm.pending = false;
                vm.status = VmStatus::Running;
                Ok(())
            }
            VmStatus::Running => Ok(()),
            other => anyhow::bail!("cannot start a {other:?} vm"),
        }
    }

    async fn stop(&self, h: &VmHandle, _grace: Duration) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("stop {}", h.name));
        let vm = g.live(h)?;
        vm.status = VmStatus::Stopped;
        Ok(())
    }

    async fn pause(&self, h: &VmHandle) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("pause {}", h.name));
        let vm = g.live(h)?;
        if vm.status != VmStatus::Running {
            anyhow::bail!("cannot pause a {:?} vm", vm.status);
        }
        vm.status = VmStatus::Paused;
        Ok(())
    }

    async fn resume(&self, h: &VmHandle) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("resume {}", h.name));
        let vm = g.live(h)?;
        if vm.status != VmStatus::Paused {
            anyhow::bail!("cannot resume a {:?} vm", vm.status);
        }
        vm.status = VmStatus::Running;
        Ok(())
    }

    async fn destroy(&self, h: &VmHandle) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("destroy {}", h.name));
        let vm = g.live(h)?;
        vm.status = VmStatus::Gone;
        Ok(())
    }

    async fn exec(&self, h: &VmHandle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let mut g = self.inner.lock();
        g.log(format!("exec {}: {}", h.name, cmd.line));
        if g.live(h)?.status != VmStatus::Running {
            anyhow::bail!("vm {} is not running", h.name);
        }
        Ok(g.canned
            .iter()
            .find(|(p, _)| cmd.line.starts_with(p.as_str()))
            .map(|(_, o)| o.clone())
            .unwrap_or_default())
    }

    async fn put(&self, h: &VmHandle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("put {}: {path}", h.name));
        let vm = g.live(h)?;
        if vm.status != VmStatus::Running {
            anyhow::bail!("vm {} is not running", h.name);
        }
        vm.files.insert(path.to_owned(), (bytes, mode));
        Ok(())
    }

    /// Within the current ceilings a change applies now; a ceiling raise (or a
    /// value above the current ceiling) waits for the next `start`. Shrinking
    /// the root disk is refused.
    async fn resize(&self, h: &VmHandle, change: ResourceChange) -> anyhow::Result<ResizeOutcome> {
        let mut g = self.inner.lock();
        g.log(format!("resize {}: {change:?}", h.name));
        let vm = g.live(h)?;
        let mut applied = ResourceChange::default();
        let mut next = vm.next_start;
        if let Some(m) = change.max_cpus {
            next.max_cpus = Some(m);
        }
        if let Some(m) = change.max_memory_mib {
            next.max_memory_mib = Some(m);
        }
        if let Some(c) = change.cpus {
            if c <= vm.spec.max_cpus {
                vm.spec.cpus = c;
                applied.cpus = Some(c);
            } else {
                next.cpus = Some(c);
            }
        }
        if let Some(m) = change.memory_mib {
            if m <= vm.spec.max_memory_mib {
                vm.spec.memory_mib = m;
                applied.memory_mib = Some(m);
            } else {
                next.memory_mib = Some(m);
            }
        }
        if let Some(d) = change.root_disk_mib {
            if d < vm.spec.root_disk_mib {
                anyhow::bail!("root disk cannot shrink ({d} < {})", vm.spec.root_disk_mib);
            }
            vm.spec.root_disk_mib = d;
            applied.root_disk_mib = Some(d);
        }
        vm.next_start = next;
        Ok(ResizeOutcome {
            applied,
            next_start: next,
            status: ResizeStatus::Applied,
            restart_required: next != ResourceChange::default(),
        })
    }

    async fn set_mounts(&self, h: &VmHandle, mounts: &[Mount]) -> anyhow::Result<Pending> {
        let mut g = self.inner.lock();
        g.log(format!("set_mounts {}: {}", h.name, mounts.len()));
        let vm = g.live(h)?;
        vm.spec.mounts = mounts.to_vec();
        vm.pending = true;
        Ok(Pending {
            restart_required: vm.status != VmStatus::Stopped,
        })
    }

    async fn set_ports(&self, h: &VmHandle, ports: &[Port]) -> anyhow::Result<Pending> {
        let mut g = self.inner.lock();
        g.log(format!("set_ports {}: {}", h.name, ports.len()));
        let vm = g.live(h)?;
        vm.spec.ports = ports.to_vec();
        vm.pending = true;
        Ok(Pending {
            restart_required: vm.status != VmStatus::Stopped,
        })
    }

    async fn metrics(&self, h: &VmHandle) -> anyhow::Result<VmMetrics> {
        let mut g = self.inner.lock();
        g.log(format!("metrics {}", h.name));
        let vm = g.live(h)?;
        Ok(VmMetrics {
            cpu_percent: 1.0,
            memory_bytes: 512 << 20,
            memory_limit_bytes: u64::from(vm.spec.memory_mib) << 20,
            disk_used_bytes: 1 << 30,
            disk_free_bytes: (u64::from(vm.spec.root_disk_mib) << 20) - (1 << 30),
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            uptime_secs: 1,
            at_ms: now_ms(),
        })
    }

    async fn logs(&self, h: &VmHandle, tail: usize) -> anyhow::Result<Vec<LogLine>> {
        let mut g = self.inner.lock();
        g.log(format!("logs {}", h.name));
        g.live(h)?;
        Ok(vec![LogLine {
            at_ms: now_ms(),
            source: "system".into(),
            line: "mock".into(),
        }]
        .into_iter()
        .take(tail)
        .collect())
    }

    async fn snapshot(
        &self,
        h: &VmHandle,
        name: &str,
        full: bool,
        out: &Progress,
    ) -> anyhow::Result<SnapshotId> {
        let mut g = self.inner.lock();
        g.log(format!("snapshot {}: {name} full={full}", h.name));
        g.live(h)?;
        let id = SnapshotId(format!("{}:{name}", h.name));
        g.snapshots.insert(
            id.0.clone(),
            VmSnapshotInfo {
                id: id.clone(),
                label: name.to_owned(),
                full,
                at_ms: now_ms(),
                bytes: Some(1 << 20),
                auto: name.starts_with("auto-"),
            },
        );
        out.say("snapshot", format!("mock snapshot {}", id.0));
        Ok(id)
    }

    async fn restore(
        &self,
        id: &SnapshotId,
        as_name: &str,
        spec: &VmSpec,
        out: &Progress,
    ) -> anyhow::Result<VmHandle> {
        let mut g = self.inner.lock();
        g.log(format!("restore {}: as {as_name}", id.0));
        if !g.snapshots.contains_key(&id.0) {
            anyhow::bail!("no such snapshot: {}", id.0);
        }
        if g.vms
            .get(as_name)
            .is_some_and(|vm| vm.status != VmStatus::Gone)
        {
            anyhow::bail!("vm {as_name} exists");
        }
        let mut spec = spec.clone();
        spec.name = as_name.to_owned();
        out.say(
            "restore",
            format!("mock vm {as_name} restored from {}", id.0),
        );
        Ok(g.insert(&spec))
    }

    async fn snapshots(&self, h: &VmHandle) -> anyhow::Result<Vec<VmSnapshotInfo>> {
        let mut g = self.inner.lock();
        g.log(format!("snapshots {}", h.name));
        let prefix = format!("{}:", h.name);
        Ok(g.snapshots
            .values()
            .filter(|s| s.id.0.starts_with(&prefix))
            .cloned()
            .collect())
    }

    async fn delete_snapshot(&self, id: &SnapshotId) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        g.log(format!("delete_snapshot {}", id.0));
        g.snapshots
            .remove(&id.0)
            .map(drop)
            .ok_or_else(|| anyhow::anyhow!("no such snapshot: {}", id.0))
    }

    async fn list(&self) -> anyhow::Result<Vec<VmHandle>> {
        let mut g = self.inner.lock();
        g.log("list".into());
        Ok(g.vms
            .values()
            .filter(|vm| vm.status != VmStatus::Gone)
            .map(|vm| handle(&vm.spec.name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{ImageRef, Init, NetPolicy};

    fn spec(name: &str) -> VmSpec {
        VmSpec {
            name: name.into(),
            image: ImageRef::Oci {
                r#ref: "img:1".into(),
            },
            cpus: 2,
            max_cpus: 4,
            memory_mib: 2048,
            max_memory_mib: 4096,
            root_disk_mib: 20480,
            mounts: vec![],
            ports: vec![],
            network: NetPolicy::PublicHost,
            init: Init::Auto,
            hostname: name.into(),
            env: BTreeMap::new(),
            files: vec![],
            labels: BTreeMap::new(),
            ephemeral: false,
            idle_timeout_secs: None,
            max_duration_secs: None,
        }
    }

    #[tokio::test]
    async fn lifecycle_and_call_log() {
        let rt = MockVmRuntime::new();
        let out = Progress::silent();
        let h = rt.create(&spec("lab"), &out).await.unwrap();
        assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Running);
        rt.pause(&h).await.unwrap();
        assert!(
            rt.exec(&h, Cmd::new("true")).await.is_err(),
            "paused: no exec"
        );
        rt.resume(&h).await.unwrap();
        rt.stop(&h, Duration::ZERO).await.unwrap();
        assert!(rt.pause(&h).await.is_err(), "stopped: no pause");
        rt.start(&h, &out).await.unwrap();
        rt.destroy(&h).await.unwrap();
        assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Gone);
        assert!(rt.list().await.unwrap().is_empty());
        assert!(rt.start(&h, &out).await.is_err(), "gone: no start");
        assert_eq!(
            rt.calls(),
            [
                "create lab",
                "status lab",
                "pause lab",
                "exec lab: true",
                "resume lab",
                "stop lab",
                "pause lab",
                "start lab",
                "destroy lab",
                "status lab",
                "list",
                "start lab",
            ]
        );
    }

    #[tokio::test]
    async fn canned_exec_and_put() {
        let rt = MockVmRuntime::new();
        let h = rt.create(&spec("lab"), &Progress::silent()).await.unwrap();
        rt.canned(
            "uname",
            ExecOutput {
                status: 0,
                stdout: b"Linux aarch64\n".to_vec(),
                stderr: vec![],
            },
        );
        let o = rt.exec(&h, Cmd::new("uname -sm")).await.unwrap();
        assert_eq!(o.stdout_str(), "Linux aarch64");
        assert!(
            rt.exec(&h, Cmd::new("ls")).await.unwrap().ok(),
            "unmatched = exit 0"
        );
        rt.put(&h, "/etc/x", Bytes::from_static(b"hi"), 0o600)
            .await
            .unwrap();
        let (b, mode) = rt.vm("lab").unwrap().files["/etc/x"].clone();
        assert_eq!((b.as_ref(), mode), (&b"hi"[..], 0o600));
    }

    #[tokio::test]
    async fn resize_within_ceilings_is_live_and_ceilings_wait() {
        let rt = MockVmRuntime::new();
        let h = rt.create(&spec("lab"), &Progress::silent()).await.unwrap();
        let o = rt
            .resize(
                &h,
                ResourceChange {
                    cpus: Some(4),
                    memory_mib: Some(3072),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            (o.applied.cpus, o.applied.memory_mib),
            (Some(4), Some(3072))
        );
        assert!(!o.restart_required);
        let o = rt
            .resize(
                &h,
                ResourceChange {
                    cpus: Some(8),
                    max_cpus: Some(8),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(o.restart_required);
        assert_eq!(
            (o.next_start.cpus, o.next_start.max_cpus),
            (Some(8), Some(8))
        );
        assert_eq!(rt.vm("lab").unwrap().spec.cpus, 4, "not yet");
        rt.stop(&h, Duration::ZERO).await.unwrap();
        rt.start(&h, &Progress::silent()).await.unwrap();
        let vm = rt.vm("lab").unwrap();
        assert_eq!((vm.spec.cpus, vm.spec.max_cpus), (8, 8));
        assert!(
            rt.resize(
                &h,
                ResourceChange {
                    root_disk_mib: Some(1),
                    ..Default::default()
                }
            )
            .await
            .is_err(),
            "disk never shrinks"
        );
    }

    #[tokio::test]
    async fn mounts_pending_snapshots_restore() {
        let rt = MockVmRuntime::new();
        let out = Progress::silent();
        let h = rt.create(&spec("lab"), &out).await.unwrap();
        let m = Mount {
            host: "/tmp".into(),
            guest: "/shared/tmp".into(),
            readonly: true,
        };
        assert!(rt.set_mounts(&h, &[m]).await.unwrap().restart_required);
        assert!(rt.vm("lab").unwrap().pending);
        rt.stop(&h, Duration::ZERO).await.unwrap();
        assert!(!rt.set_ports(&h, &[]).await.unwrap().restart_required);
        rt.start(&h, &out).await.unwrap();
        assert!(!rt.vm("lab").unwrap().pending);

        let id = rt.snapshot(&h, "clean", false, &out).await.unwrap();
        assert_eq!(id.0, "lab:clean");
        assert_eq!(rt.snapshots(&h).await.unwrap().len(), 1);
        assert!(
            rt.restore(&id, "lab", &spec("lab"), &out).await.is_err(),
            "name taken"
        );
        let h2 = rt.restore(&id, "lab~", &spec("lab"), &out).await.unwrap();
        assert_eq!(rt.status(&h2).await.unwrap(), VmStatus::Running);
        assert_eq!(rt.list().await.unwrap().len(), 2);
        rt.delete_snapshot(&id).await.unwrap();
        assert!(rt.delete_snapshot(&id).await.is_err());
        assert!(rt.snapshots(&h).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn preflight_is_configurable() {
        let rt = MockVmRuntime::new();
        assert!(rt.preflight().await.blockers.is_empty());
        let rt = MockVmRuntime::new().with_preflight(MockVmRuntime::blocked_preflight(
            "no_kvm",
            "/dev/kvm missing",
        ));
        assert_eq!(rt.preflight().await.blockers[0].code, "no_kvm");
    }
}
