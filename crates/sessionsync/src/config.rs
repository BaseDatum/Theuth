//! `sessions:` config section (68 §9). Every key has a default; an absent
//! section is the defaults. Documented in `crates/theuth/harness.md`.

use serde::{Deserialize, Serialize};

/// `sessions:` in `config.yaml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SessionsConfig {
    /// Owner unseen this long → another engine may take a session it needs
    /// to write (68 §4.2 (c)); `reason = user` may take at once.
    pub takeover_secs: u64,
    /// How long a taker waits for a reachable owner to flush after `Release`.
    pub release_wait_secs: u64,
    /// How long a demoted owner keeps undelivered (orphaned) turns for viewing (68 §4.3).
    pub orphan_keep_days: u64,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            takeover_secs: 300,
            release_wait_secs: 10,
            orphan_keep_days: 7,
        }
    }
}
