//! Runtime resolve / extract (`setup::*` of the microsandbox SDK; 79 §5.1, embed ruling).
//!
//! `t` carries the `msb` + `libkrunfw` pair for its target (`embed-binaries`) and writes it into
//! the SDK home on first use — no download. The SDK resolves `MSB_PATH`, `config.paths` and
//! `<home>/bin/msb` + `<home>/lib/libkrunfw`; theuth adds one rule, [`effective_home`]: share the
//! user's `~/.microsandbox` when it is empty or holds the embedded version, use a private home
//! under the data dir otherwise.
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use microsandbox::{MicrosandboxError, config::GlobalConfig, setup};

use super::RuntimeState;
use crate::Progress;

/// The version of the pair compiled into this binary (the SDK's pinned release).
pub fn embedded_version() -> String {
    setup::InstallOptions::default().version
}

/// SDK config for `home` (`""` = `MSB_HOME` or `~/.microsandbox`).
pub fn config(home: &str) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    if !home.is_empty() {
        cfg.home = Some(PathBuf::from(home));
    }
    cfg
}

/// The home theuth runs VMs in. An explicit `sandbox.vm.home` wins. Otherwise the shared
/// default home — when it is empty (we extract there) or already holds the embedded version
/// (the user's own install of the same release is ours to use) — and `<data_dir>/sandbox/msb-home`
/// when it holds another version or half a pair, so neither side breaks the other's runtime.
/// Decided once per process: a later change to `~/.microsandbox` is seen after a restart.
pub fn effective_home(home: &str, data_dir: &Path) -> String {
    if !home.is_empty() {
        return home.to_owned();
    }
    match setup::resolve_runtime(&GlobalConfig::default()) {
        Err(MicrosandboxError::RuntimeNotInstalled(_)) => String::new(),
        Ok(rt) if version_of(&rt.msb_path) == embedded_version() => String::new(),
        _ => data_dir
            .join("sandbox")
            .join("msb-home")
            .display()
            .to_string(),
    }
}

/// The user's own pair in the default home when theuth runs in a different `home`: the version
/// found there, so preflight can warn that its `msb ls` will not show theuth's VMs.
pub fn other_msb(home: &str) -> Option<String> {
    let shared = GlobalConfig::default();
    if home.is_empty() || Path::new(home) == shared.home() {
        return None;
    }
    setup::resolve_runtime(&shared)
        .ok()
        .map(|rt| version_of(&rt.msb_path))
}

fn version_of(msb: &Path) -> String {
    setup::resolve_runtime_version(msb)
        .ok()
        .flatten()
        .map_or_else(|| "unknown".into(), |v| v.to_string())
}

/// What `cfg` resolves to, without touching the disk beyond a stat and a version read.
pub fn resolve(cfg: &GlobalConfig) -> RuntimeState {
    let home = cfg.home().display().to_string();
    match setup::resolve_runtime(cfg) {
        Ok(rt) => {
            let version = version_of(&rt.msb_path);
            if version == embedded_version() {
                RuntimeState::Embedded {
                    version,
                    extracted: true,
                    home,
                }
            } else {
                RuntimeState::Installed {
                    version,
                    home,
                    libkrunfw: rt.libkrunfw_path.display().to_string(),
                }
            }
        }
        Err(MicrosandboxError::RuntimeNotInstalled(_)) => RuntimeState::Embedded {
            version: embedded_version(),
            extracted: false,
            home,
        },
        Err(e) => RuntimeState::Incomplete { why: e.to_string() },
    }
}

/// The `msb` binary the SDK would run (`<home>/bin/msb` or `MSB_PATH`).
pub fn msb_path(home: &str) -> Option<PathBuf> {
    setup::resolve_runtime(&config(home))
        .ok()
        .map(|rt| rt.msb_path)
}

/// Write the embedded pair into `cfg.home()` (seconds, no network). Idempotent: a present pair —
/// ours or a user-provided one — returns at once. A half pair is never repaired by the SDK, so
/// the error names the two files to remove.
pub async fn install(cfg: &GlobalConfig, out: &Progress) -> anyhow::Result<RuntimeState> {
    let before = resolve(cfg);
    let home = cfg.home();
    match &before {
        RuntimeState::Embedded {
            extracted: true,
            version,
            ..
        }
        | RuntimeState::Installed { version, .. } => {
            out.say(
                "install",
                format!("runtime {version} already set up in {}", home.display()),
            );
            return Ok(before);
        }
        RuntimeState::Embedded { version, .. } => out.say(
            "install",
            format!(
                "setting up the embedded microsandbox {version} in {} (no download)",
                home.display()
            ),
        ),
        RuntimeState::Incomplete { why } => anyhow::bail!(
            "the microsandbox runtime in {} is half installed ({why}); remove {}/bin/msb and \
             {}/lib/libkrunfw*, then set up the runtime again",
            home.display(),
            home.display(),
            home.display()
        ),
        RuntimeState::Missing => anyhow::bail!("this build of t carries no microsandbox runtime"),
    }
    let opts = setup::InstallOptions {
        source: setup::InstallSource::EmbeddedArchive,
        ..Default::default()
    };
    let started = Instant::now();
    let rt = setup::install_runtime(cfg, opts).await.map_err(|e| {
        anyhow::anyhow!(
            "could not set up the microsandbox runtime in {}: {e}",
            home.display()
        )
    })?;
    out.say(
        "install",
        format!(
            "extracted {} + {} in {:.1} s",
            rt.msb_path.display(),
            rt.libkrunfw_path.display(),
            started.elapsed().as_secs_f32()
        ),
    );
    Ok(resolve(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sandbox-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Empty home → `Embedded { extracted: false }`; extraction writes the pinned version;
    /// a second call is a no-op; the pair is what the SDK resolves.
    #[tokio::test]
    async fn extraction_is_versioned_and_idempotent() {
        let home = tmp("extract");
        let cfg = config(home.to_str().unwrap());
        let version = embedded_version();
        assert_eq!(
            resolve(&cfg),
            RuntimeState::Embedded {
                version: version.clone(),
                extracted: false,
                home: home.display().to_string()
            }
        );
        let after = install(&cfg, &Progress::silent()).await.unwrap();
        assert_eq!(
            after,
            RuntimeState::Embedded {
                version: version.clone(),
                extracted: true,
                home: home.display().to_string()
            }
        );
        let msb = msb_path(home.to_str().unwrap()).unwrap();
        assert_eq!(msb, home.join("bin").join("msb"));
        let mtime = std::fs::metadata(&msb).unwrap().modified().unwrap();
        assert_eq!(install(&cfg, &Progress::silent()).await.unwrap(), after);
        assert_eq!(std::fs::metadata(&msb).unwrap().modified().unwrap(), mtime);
        // An explicit home is honoured as is; so is a home the user filled with the same version.
        assert_eq!(
            effective_home(home.to_str().unwrap(), Path::new("/d")),
            home.to_str().unwrap()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A pair of another version → `Installed`, never overwritten by `install`; a lone `msb` →
    /// `Incomplete` and `install` refuses with the files to remove.
    #[tokio::test]
    async fn foreign_and_partial_pairs() {
        let home = tmp("foreign");
        let cfg = config(home.to_str().unwrap());
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::write(home.join("bin").join("msb"), b"not a real msb").unwrap();
        assert!(matches!(resolve(&cfg), RuntimeState::Incomplete { .. }));
        let e = install(&cfg, &Progress::silent())
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("half installed") && e.contains("bin/msb"), "{e}");
        let lib = if cfg!(target_os = "macos") {
            "libkrunfw.5.dylib"
        } else {
            "libkrunfw.so.5.6.1"
        };
        std::fs::write(home.join("lib").join(lib), b"not a real lib").unwrap();
        match resolve(&cfg) {
            RuntimeState::Installed { version, .. } => assert_eq!(version, "unknown"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            install(&cfg, &Progress::silent()).await.unwrap(),
            RuntimeState::Installed { .. }
        ));
        assert_eq!(
            std::fs::read(home.join("bin").join("msb")).unwrap(),
            b"not a real msb"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
