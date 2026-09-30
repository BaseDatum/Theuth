//! Runtime install / resolve (`setup::*` of the microsandbox SDK; 79 §5.1).
//!
//! The SDK resolves only `MSB_PATH`, `config.paths`, `<home>/bin/msb` + `<home>/lib/libkrunfw`
//! and its own packaged binary — never `PATH`. [`config`] adds one step: an `msb` on `PATH`
//! with a `../lib/libkrunfw.<abi>` beside it is handed to the SDK as an explicit pair, so a
//! Homebrew/script install outside `home` is used instead of downloaded again.
use std::path::{Path, PathBuf};

use microsandbox::{MicrosandboxError, config::GlobalConfig, setup};

use super::RuntimeState;
use crate::Progress;

const LIBKRUNFW: &str = if cfg!(target_os = "macos") {
    "libkrunfw.5.dylib"
} else {
    "libkrunfw.so.5"
};

/// SDK config for `home` (`""` = `MSB_HOME` or `~/.microsandbox`), with a `PATH` `msb` as an
/// explicit pair when `home` has none.
pub fn config(home: &str) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    if !home.is_empty() {
        cfg.home = Some(PathBuf::from(home));
    }
    if cfg.paths.msb.is_none()
        && !setup::is_runtime_installed(&cfg)
        && let Some((msb, lib)) = msb_on_path()
    {
        cfg.paths.msb = Some(msb);
        cfg.paths.libkrunfw = Some(lib);
    }
    cfg
}

fn msb_on_path() -> Option<(PathBuf, PathBuf)> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .filter(|d| !d.is_empty())
        .filter_map(|d| {
            let msb = std::fs::canonicalize(Path::new(d).join("msb")).ok()?;
            let lib = msb.parent()?.parent()?.join("lib").join(LIBKRUNFW);
            lib.is_file().then_some((msb, lib))
        })
        .next()
}

/// What is installed, without touching the network or the disk.
pub fn resolve(cfg: &GlobalConfig) -> RuntimeState {
    match setup::resolve_runtime(cfg) {
        Ok(rt) => RuntimeState::Installed {
            version: setup::resolve_runtime_version(&rt.msb_path)
                .ok()
                .flatten()
                .map_or_else(|| "unknown".into(), |v| v.to_string()),
            home: cfg.home().display().to_string(),
            libkrunfw: rt.libkrunfw_path.display().to_string(),
        },
        Err(MicrosandboxError::RuntimeNotInstalled(_)) => RuntimeState::Missing,
        Err(e) => RuntimeState::Incomplete { why: e.to_string() },
    }
}

/// `msb` + `libkrunfw` both present.
pub fn is_installed(cfg: &GlobalConfig) -> bool {
    setup::is_runtime_installed(cfg)
}

/// The `msb` binary the SDK would run (`<home>/bin/msb`, `MSB_PATH`, or the `PATH` pair).
pub fn msb_path(home: &str) -> Option<PathBuf> {
    setup::resolve_runtime(&config(home)).ok().map(|rt| rt.msb_path)
}

/// Install (or repair, when [`resolve`] says `Incomplete`) the SDK's pinned release into
/// `home`. Idempotent: an `Installed` state returns at once. On a download failure the error
/// names the URL and the two manual routes.
pub async fn install(cfg: &GlobalConfig, out: &Progress) -> anyhow::Result<RuntimeState> {
    let before = resolve(cfg);
    let force = match &before {
        RuntimeState::Installed { version, .. } => {
            out.say("install", format!("runtime {version} already installed"));
            return Ok(before);
        }
        RuntimeState::Incomplete { why } => {
            out.say("install", format!("runtime incomplete ({why}); reinstalling"));
            true
        }
        RuntimeState::Missing => false,
    };
    let opts = setup::InstallOptions {
        force,
        ..Default::default()
    };
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let url = format!(
        "https://github.com/superradcompany/microsandbox/releases/download/v{}/microsandbox-{os}-{arch}.tar.gz",
        opts.version
    );
    out.say(
        "install",
        format!(
            "downloading microsandbox {} into {} ({url})",
            opts.version,
            cfg.home().display()
        ),
    );
    let rt = setup::install_runtime(cfg, opts).await.map_err(|e| {
        anyhow::anyhow!(
            "microsandbox runtime install failed: {e}; download {url} by hand, or run \
             `brew install superradcompany/tap/microsandbox` or \
             `curl -fsSL https://install.microsandbox.dev | sh`"
        )
    })?;
    out.say(
        "install",
        format!(
            "installed {} + {}",
            rt.msb_path.display(),
            rt.libkrunfw_path.display()
        ),
    );
    Ok(resolve(cfg))
}
