//! Host facts + blockers (79 §5.1). [`probe`] gathers [`Facts`] from the OS and the SDK;
//! [`evaluate`] turns them into a [`Preflight`] and is what the unit tests drive.
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Blocker, HostFacts, Preflight, Resources, RuntimeState, VirtState};
use crate::types::VmConfig;

const GIB: u64 = 1024 * 1024 * 1024;
/// Debian systemd base image + the desktop layers, rounded up (79 §5.1 "image size").
const IMAGE_BYTES: u64 = 2 * GIB;
const HEADROOM_BYTES: u64 = 2 * GIB;

/// Everything [`evaluate`] looks at, so tests can fake a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub host: HostFacts,
    pub runtime: RuntimeState,
    /// Why the last extraction of the embedded pair failed (cleared by a successful one).
    pub extract_failed: Option<String>,
    /// The version of the user's own `msb` in the default home when theuth runs in another home.
    pub other_msb: Option<String>,
    /// Linux only: `/dev/kvm` exists.
    pub kvm_present: bool,
    /// Linux only: `/dev/kvm` opens read/write.
    pub kvm_rw: bool,
    /// Linux only: `ldd --version` first line's `x.y`, when it could be read.
    pub glibc: Option<(u32, u32)>,
    pub docker_on_path: bool,
}

/// Gather the facts for this host. Blocking syscalls and two short child processes; call from a
/// blocking context or spawn_blocking.
pub fn probe(cfg: &VmConfig) -> Facts {
    let sdk = super::install::config(&cfg.home);
    let home = sdk.home();
    // Free space where the runtime keeps disks; fall back to `$HOME`, then `/`, when the home
    // does not exist yet.
    let probe_dir = if home.exists() {
        home
    } else {
        dirs::home_dir().unwrap_or_else(|| "/".into())
    };
    Facts {
        host: HostFacts {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            mem_bytes: host_mem_bytes(),
            disk_free_bytes: disk_free_bytes(&probe_dir),
        },
        runtime: super::install::resolve(&sdk),
        extract_failed: None,
        other_msb: super::install::other_msb(&cfg.home),
        kvm_present: std::path::Path::new("/dev/kvm").exists(),
        kvm_rw: std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok(),
        glibc: glibc_version(),
        docker_on_path: on_path("docker"),
    }
}

/// Judge `facts` (79 §5.1). Order of blockers: OS/arch, KVM, glibc, runtime, disk.
pub fn evaluate(facts: &Facts, cfg: &VmConfig) -> Preflight {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let block = |code: &str, message: String, fix: Option<&str>| Blocker {
        code: code.into(),
        message,
        fix: fix.map(Into::into),
        installable: false,
    };

    let virt = match (facts.host.os.as_str(), facts.host.arch.as_str()) {
        ("macos", "aarch64") => VirtState::Ok { via: "hvf".into() },
        ("macos", _) => {
            let why =
                "microsandbox needs Apple Silicon (M1 or later); this Mac is Intel".to_owned();
            blockers.push(block("unsupported_arch", why.clone(), None));
            VirtState::Unsupported { why }
        }
        ("windows", _) => {
            let why = "Windows support in microsandbox is a preview; not enabled in this version"
                .to_owned();
            blockers.push(block("unsupported_os", why.clone(), None));
            VirtState::Unsupported { why }
        }
        ("linux", _) => {
            if !facts.kvm_present {
                let why = "/dev/kvm is missing".to_owned();
                blockers.push(block(
                    "no_kvm",
                    format!("{why}; the host cannot run hardware-accelerated VMs."),
                    Some(
                        "enable virtualization in firmware, or nested virtualization in the outer hypervisor/LXC",
                    ),
                ));
                VirtState::Unsupported { why }
            } else if !facts.kvm_rw {
                let why = "/dev/kvm is not readable and writable by this user".to_owned();
                blockers.push(block(
                    "kvm_not_writable",
                    format!("{why}."),
                    Some("sudo usermod -aG kvm $USER, then log in again"),
                ));
                VirtState::Unsupported { why }
            } else {
                VirtState::Ok { via: "kvm".into() }
            }
        }
        (os, _) => {
            let why = format!("microsandbox does not run on {os}");
            blockers.push(block("unsupported_os", why.clone(), None));
            VirtState::Unsupported { why }
        }
    };

    if facts.host.os == "linux"
        && let Some((major, minor)) = facts.glibc
        && (major, minor) < (2, 28)
    {
        blockers.push(block(
            "glibc_too_old",
            format!("glibc {major}.{minor} found; the microsandbox runtime needs glibc ≥ 2.28."),
            None,
        ));
    }

    match &facts.runtime {
        RuntimeState::Embedded { .. } | RuntimeState::Installed { .. } => {}
        RuntimeState::Missing => blockers.push(Blocker {
            code: "runtime_missing".into(),
            message: "This build of t carries no microsandbox runtime.".into(),
            fix: None,
            installable: false,
        }),
        RuntimeState::Incomplete { why } => blockers.push(Blocker {
            code: "runtime_incomplete".into(),
            message: format!("The microsandbox runtime is only half installed ({why})."),
            fix: Some(
                "remove <home>/bin/msb and <home>/lib/libkrunfw*, then Set up runtime".into(),
            ),
            installable: true,
        }),
    }
    if let Some(why) = &facts.extract_failed {
        blockers.push(Blocker {
            code: "runtime_extract_failed".into(),
            message: format!("The embedded microsandbox runtime could not be set up ({why})."),
            fix: Some("check that the home is writable, then Set up runtime again".into()),
            installable: true,
        });
    }

    let defaults = Resources::derive(&facts.host, &cfg.defaults);
    let need = u64::from(defaults.root_disk_mib) * 1024 * 1024 + IMAGE_BYTES + HEADROOM_BYTES;
    if facts.host.disk_free_bytes < need {
        blockers.push(block(
            "disk_low",
            format!(
                "{} GiB free; a VM needs root_disk + image + 2 GiB (about {} GiB).",
                facts.host.disk_free_bytes / GIB,
                need.div_ceil(GIB)
            ),
            Some("free disk space or lower sandbox.vm.defaults.root_disk_mib"),
        ));
    }
    if u64::from(defaults.memory_mib) * 1024 * 1024 > facts.host.mem_bytes / 2 {
        warnings.push(block(
            "memory_low",
            format!(
                "the default VM memory ({} MiB) is more than half of this host's {} GiB.",
                defaults.memory_mib,
                facts.host.mem_bytes / GIB
            ),
            Some("lower sandbox.vm.defaults.memory_mib"),
        ));
    }
    if !facts.docker_on_path {
        warnings.push(block(
            "docker_missing",
            "docker is not on PATH; building a dev image locally needs it (pulling a released image does not)."
                .into(),
            None,
        ));
    }
    if let Some(v) = &facts.other_msb
        && let RuntimeState::Embedded { home, .. } | RuntimeState::Installed { home, .. } =
            &facts.runtime
    {
        warnings.push(block(
            "runtime_home",
            format!(
                "theuth's VMs live in {home}; your own msb ({v} in the default home) will not list them."
            ),
            Some("set sandbox.vm.home to the default home once its msb is the version theuth embeds"),
        ));
    }

    Preflight {
        runtime: facts.runtime.clone(),
        virt,
        host: facts.host.clone(),
        defaults,
        blockers,
        warnings,
        checked_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
    }
}

fn host_mem_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0)
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("MemTotal:"))
                    .and_then(|v| v.trim().split(' ').next()?.parse::<u64>().ok())
            })
            .map_or(0, |kib| kib * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

/// `df -Pk <dir>`: POSIX output, 1 KiB blocks, "Available" is column 4.
fn disk_free_bytes(dir: &std::path::Path) -> u64 {
    std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            let avail = s.lines().nth(1)?.split_whitespace().nth(3)?;
            avail.parse::<u64>().ok()
        })
        .map_or(0, |kib| kib * 1024)
}

fn glibc_version() -> Option<(u32, u32)> {
    if std::env::consts::OS != "linux" {
        return None;
    }
    let out = std::process::Command::new("ldd")
        .arg("--version")
        .output()
        .ok()?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .to_owned();
    let ver = first.rsplit(' ').next()?;
    let (major, minor) = ver.split_once('.')?;
    Some((major.parse().ok()?, minor.trim().parse().ok()?))
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(os: &str, arch: &str) -> Facts {
        Facts {
            host: HostFacts {
                os: os.into(),
                arch: arch.into(),
                cores: 8,
                mem_bytes: 16 * GIB,
                disk_free_bytes: 200 * GIB,
            },
            runtime: RuntimeState::Embedded {
                version: "0.7.4".into(),
                extracted: false,
                home: "/h".into(),
            },
            extract_failed: None,
            other_msb: None,
            kvm_present: true,
            kvm_rw: true,
            glibc: Some((2, 36)),
            docker_on_path: true,
        }
    }

    fn codes(p: &Preflight) -> Vec<&str> {
        p.blockers.iter().map(|b| b.code.as_str()).collect()
    }

    #[test]
    fn intel_mac_is_unsupported() {
        let p = evaluate(&facts("macos", "x86_64"), &VmConfig::default());
        assert_eq!(codes(&p), ["unsupported_arch"]);
        assert_eq!(
            p.blockers[0].message,
            "microsandbox needs Apple Silicon (M1 or later); this Mac is Intel"
        );
        assert!(matches!(p.virt, VirtState::Unsupported { .. }));
    }

    #[test]
    fn apple_silicon_and_linux_ok() {
        let p = evaluate(&facts("macos", "aarch64"), &VmConfig::default());
        assert!(p.blockers.is_empty(), "{:?}", p.blockers);
        assert_eq!(p.virt, VirtState::Ok { via: "hvf".into() });
        let p = evaluate(&facts("linux", "x86_64"), &VmConfig::default());
        assert!(p.blockers.is_empty(), "{:?}", p.blockers);
        assert_eq!(p.virt, VirtState::Ok { via: "kvm".into() });
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn linux_kvm_and_glibc() {
        let mut f = facts("linux", "x86_64");
        f.kvm_present = false;
        assert_eq!(codes(&evaluate(&f, &VmConfig::default())), ["no_kvm"]);
        f.kvm_present = true;
        f.kvm_rw = false;
        let p = evaluate(&f, &VmConfig::default());
        assert_eq!(codes(&p), ["kvm_not_writable"]);
        assert!(
            p.blockers[0]
                .fix
                .as_deref()
                .unwrap()
                .contains("usermod -aG kvm")
        );
        f.kvm_rw = true;
        f.glibc = Some((2, 17));
        assert_eq!(
            codes(&evaluate(&f, &VmConfig::default())),
            ["glibc_too_old"]
        );
    }

    #[test]
    fn runtime_states() {
        // Not yet extracted is not a blocker: the first create extracts.
        let mut f = facts("macos", "aarch64");
        assert!(evaluate(&f, &VmConfig::default()).blockers.is_empty());
        f.runtime = RuntimeState::Installed {
            version: "0.8.0".into(),
            home: "/h".into(),
            libkrunfw: "/h/lib/libkrunfw.5.dylib".into(),
        };
        assert!(evaluate(&f, &VmConfig::default()).blockers.is_empty());
        f.runtime = RuntimeState::Missing;
        let p = evaluate(&f, &VmConfig::default());
        assert_eq!(codes(&p), ["runtime_missing"]);
        assert!(!p.blockers[0].installable);
        f.runtime = RuntimeState::Incomplete {
            why: "no libkrunfw".into(),
        };
        let p = evaluate(&f, &VmConfig::default());
        assert_eq!(codes(&p), ["runtime_incomplete"]);
        assert!(p.blockers[0].installable && p.blockers[0].message.contains("no libkrunfw"));
        // A failed extraction is an installable blocker on top of the state.
        f.runtime = RuntimeState::Embedded {
            version: "0.7.4".into(),
            extracted: false,
            home: "/h".into(),
        };
        f.extract_failed = Some("read-only file system".into());
        let p = evaluate(&f, &VmConfig::default());
        assert_eq!(codes(&p), ["runtime_extract_failed"]);
        assert!(
            p.blockers[0].installable && p.blockers[0].message.contains("read-only file system")
        );
    }

    #[test]
    fn another_msb_in_the_default_home_warns() {
        let mut f = facts("macos", "aarch64");
        f.other_msb = Some("0.8.0".into());
        let p = evaluate(&f, &VmConfig::default());
        assert!(p.blockers.is_empty());
        let w: Vec<&str> = p.warnings.iter().map(|b| b.code.as_str()).collect();
        assert_eq!(w, ["runtime_home"]);
        assert!(p.warnings[0].message.contains("/h") && p.warnings[0].message.contains("0.8.0"));
    }

    #[test]
    fn low_disk_blocks_and_low_memory_warns() {
        let mut f = facts("macos", "aarch64");
        f.host.disk_free_bytes = 10 * GIB; // default root disk is 20 GiB
        f.host.mem_bytes = 4 * GIB; // derive clamps memory to 2 GiB = host/2, not more
        f.docker_on_path = false;
        let mut cfg = VmConfig::default();
        cfg.defaults.memory_mib = crate::types::AutoOr::Value(3072);
        let p = evaluate(&f, &cfg);
        assert_eq!(codes(&p), ["disk_low"]);
        assert!(
            p.blockers[0].message.contains("24 GiB"),
            "{}",
            p.blockers[0].message
        );
        let w: Vec<&str> = p.warnings.iter().map(|b| b.code.as_str()).collect();
        assert_eq!(w, ["memory_low", "docker_missing"]);
    }
}
