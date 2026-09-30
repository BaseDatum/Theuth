//! `sandbox:` config (79 §4.7) as the plugin reads it, and what it means for
//! what starts: nothing, the guest half, or the host actor with or without
//! the VM runtime.

use std::time::Duration;

use runtime::Runtime;

use crate::types::{GuestConfig, SandboxConfig};

/// What `SandboxPlugin::start` does with the section.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// `enabled: false` and no `guest`: no actor, no tools, no RPC; records untouched.
    Disabled,
    /// `guest: Some(_)` — this engine runs inside a sandbox VM: the guest
    /// prompt block and `sandbox_takeover` (hand back), nothing else. The host
    /// writes the guest's config as `enabled: false, guest: {…}` (§5.3: no VMs
    /// in VMs), so `guest` wins over `enabled`.
    Guest(GuestConfig),
    /// The host: the actor, records, tools; `vm` says whether VMs may be made.
    Host {
        /// `sandbox.vm.enabled`.
        vm: bool,
    },
}

/// Read `sandbox:`; a missing section is the defaults.
pub fn load(rt: &Runtime) -> SandboxConfig {
    rt.section("sandbox").unwrap_or_else(|e| {
        tracing::debug!("no [sandbox] config section, using defaults: {e}");
        SandboxConfig::default()
    })
}

/// The mode a config selects.
pub fn mode(cfg: &SandboxConfig) -> Mode {
    if let Some(g) = &cfg.guest {
        return Mode::Guest(g.clone());
    }
    if !cfg.enabled {
        return Mode::Disabled;
    }
    Mode::Host { vm: cfg.vm.enabled }
}

/// `step_timeout_secs` as a duration (replaces the old `STEP_TIMEOUT` constant).
pub fn step_timeout(cfg: &SandboxConfig) -> Duration {
    Duration::from_secs(cfg.step_timeout_secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_follows_the_switches() {
        let mut cfg = SandboxConfig::default();
        assert_eq!(mode(&cfg), Mode::Host { vm: true });
        cfg.vm.enabled = false;
        assert_eq!(mode(&cfg), Mode::Host { vm: false });
        cfg.enabled = false;
        assert_eq!(mode(&cfg), Mode::Disabled);
        // The guest config the host writes (§5.3) is `enabled: false, guest: {…}`.
        cfg.guest = Some(GuestConfig {
            name: "lab".into(),
            host: "mac".into(),
            shares: vec![],
        });
        assert!(matches!(mode(&cfg), Mode::Guest(_)));
        cfg.enabled = true;
        assert!(matches!(mode(&cfg), Mode::Guest(_)));
        assert_eq!(
            step_timeout(&SandboxConfig::default()),
            Duration::from_secs(600)
        );
    }
}
