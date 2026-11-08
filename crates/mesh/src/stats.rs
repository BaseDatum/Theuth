//! What this engine advertises (67 §4.5 election, §8 placement): machine
//! class, installed harnesses, repo roots, display, GPU. Best effort; every
//! probe answers `false`/empty when it cannot tell.

use std::path::{Path, PathBuf};

use crate::types::{Accelerator, EngineClass, EngineId, EngineStats};

/// Probe everything. `workspaces` = `mesh.workspaces`; `data_dir` for
/// `acp/agents.yaml`; `declared` = `mesh.class` (79 decision 9), which
/// short-circuits detection when set.
pub fn probe(
    id: EngineId,
    uptime_since_ms: u64,
    data_dir: &Path,
    workspaces: &[String],
    declared: Option<EngineClass>,
) -> EngineStats {
    let m = platform::process::machine();
    let display = has_display();
    let class = declared.unwrap_or_else(|| detect_class(has_battery(), display));
    EngineStats {
        id,
        uptime_since_ms,
        class,
        availability_30d: 1.0,
        harnesses: harnesses(data_dir),
        repos: repos(workspaces),
        display,
        gpu: has_gpu(),
        accelerator: Some(accelerator()),
        // 183 §9.2
        os: m.os.into(),
        os_name: m.os_name.clone(),
        arch: m.arch.into(),
        host: m.host.clone(),
    }
}

/// Detection order: container marker, battery → laptop, no display → server,
/// else desktop (67 §4.5).
fn detect_class(battery: bool, display: bool) -> EngineClass {
    if in_container() {
        EngineClass::Container
    } else if battery {
        EngineClass::Laptop
    } else if !display {
        EngineClass::Server
    } else {
        EngineClass::Desktop
    }
}

/// `/.dockerenv`, or a cgroup that names docker/containerd/kubepods/lxc.
fn in_container() -> bool {
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return true;
    }
    std::fs::read_to_string("/proc/1/cgroup")
        .map(|s| {
            ["docker", "containerd", "kubepods", "lxc", "podman"]
                .iter()
                .any(|k| s.contains(k))
        })
        .unwrap_or(false)
}

/// macOS: `pmset -g batt` names an internal battery; Linux: `/sys/class/power_supply/BAT*`.
fn has_battery() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("InternalBattery"))
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/sys/class/power_supply")
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("BAT"))
            })
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// macOS and Windows always drive a display (WindowServer / the console session);
/// Linux only with an X or Wayland session in the environment.
fn has_display() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        !in_container()
    }
}

/// Apple Silicon always has one; Linux when a DRM render node or an NVIDIA
/// device exists; `false` when unknown.
fn has_gpu() -> bool {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return true;
    }
    Path::new("/dev/nvidia0").exists()
        || std::fs::read_dir("/dev/dri")
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
            })
            .unwrap_or(false)
}

/// 126 §8.4, the cheap way: Apple silicon → Metal; an NVIDIA device → CUDA;
/// else CPU (an iGPU render node is a GPU, not a training-grade one).
fn accelerator() -> Accelerator {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Accelerator::Metal
    } else if Path::new("/dev/nvidia0").exists() {
        Accelerator::Cuda
    } else {
        Accelerator::Cpu
    }
}

/// `native` plus every key of `<data_dir>/acp/agents.yaml`.
fn harnesses(data_dir: &Path) -> Vec<String> {
    let mut out = vec!["native".to_owned()];
    if let Ok(text) = std::fs::read_to_string(data_dir.join("acp").join("agents.yaml"))
        && let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(&text)
    {
        out.extend(map.keys().filter_map(|k| k.as_str().map(str::to_owned)));
    }
    out
}

/// Git roots under each workspace: the workspace itself, or its children and
/// grandchildren (two levels), whichever contain a `.git`.
fn repos(workspaces: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for w in workspaces {
        let root = PathBuf::from(shellexpand_home(w));
        if root.join(".git").exists() {
            out.push(root.to_string_lossy().into_owned());
            continue;
        }
        walk(&root, 2, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, depth: u8, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() || e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.join(".git").exists() {
            out.push(p.to_string_lossy().into_owned());
        } else if depth > 1 {
            walk(&p, depth - 1, out);
        }
    }
}

fn shellexpand_home(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(h) => Path::new(&h).join(rest).to_string_lossy().into_owned(),
            None => p.to_owned(),
        },
        None => p.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_class_beats_detection() {
        let dir = tempfile::tempdir().unwrap();
        let detected = probe(EngineId("e".into()), 0, dir.path(), &[], None).class;
        // Pick a declared value the host cannot detect as, so the assertion bites.
        assert_eq!(
            probe(
                EngineId("e".into()),
                0,
                dir.path(),
                &[],
                Some(EngineClass::Vm)
            )
            .class,
            EngineClass::Vm
        );
        assert_ne!(detected, EngineClass::Vm, "detection never yields vm");
    }

    #[test]
    fn repos_finds_git_roots_two_levels_down() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("org/b/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("org/deep/c/.git")).unwrap();
        let found = repos(&[dir.path().to_string_lossy().into_owned()]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].ends_with("/a"));
        assert!(found[1].ends_with("/org/b"));
    }

    #[test]
    fn harnesses_always_has_native() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(harnesses(dir.path()), ["native"]);
        std::fs::create_dir_all(dir.path().join("acp")).unwrap();
        std::fs::write(
            dir.path().join("acp/agents.yaml"),
            "omp:\n  command: omp\nclaude:\n  command: x\n",
        )
        .unwrap();
        assert_eq!(harnesses(dir.path()), ["native", "omp", "claude"]);
    }
}
