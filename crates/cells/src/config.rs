//! The `cells:` section of config.yaml (owner: C, FROZEN; design 171 §15.4). Read once at
//! start like every plugin section; an absent section is the defaults below. Each worker adds
//! its own keys in its own struct, flattened here, so this file never changes: `ShellConfig`
//! (E1, `shell.rs`), `PromptConfig` (A1, `prompt.rs`), `PythonConfig` (E2, `python.rs`),
//! `MontyConfig` (E2, `monty.rs`). A worker key must not reuse a name below.

use serde::{Deserialize, Serialize};

use crate::monty::MontyConfig;
use crate::prompt::PromptConfig;
use crate::python::PythonConfig;
use crate::shell::ShellConfig;

/// `cells:` (§15.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CellsConfig {
    /// `false`: no actor, no prompt block, every RPC answers "not running".
    pub enabled: bool,
    /// D4: an idle kernel closes after this many hours.
    pub kernel_idle_hours: u64,
    /// §5.10: live kernels engine-wide.
    pub max_kernels: u32,
    /// §5.9: the cells-owned Monty pool size.
    pub monty_workers: u32,
    /// §5.10: fixed kernel PTY width.
    pub pty_cols: u16,
    /// §5.10: fixed kernel PTY height.
    pub pty_rows: u16,
    /// §7.3: Kill is enabled this long after Stop; a timeout sends Kill this long after Stop.
    pub kill_after_secs: u64,
    /// D11: the agent's head+tail cap.
    pub agent_bytes: usize,
    /// §9.2: `<cell_error>` rounds per turn.
    pub correct_rounds: u32,
    /// D16: notify when a longer run ends unfocused.
    pub notify_after_secs: u64,
    /// §7.6: an `after` gate's run must be younger than this.
    pub after_max_age_mins: u64,
    /// §8.3: render-only parser scrollback rows.
    pub render_scrollback: usize,
    /// §8.6: lines the finished static view shows.
    pub ui_tail_lines: u32,
    /// §4.4: `output` fence cap in exported markdown.
    pub export_output_bytes: usize,
    /// §13.2: replicated latest-run text cap.
    pub replicate_bytes: usize,
    /// §8.4: run retention.
    pub retention_days: u64,
    /// §8.4: run store size cap.
    pub store_max_mb: u64,
    /// §5.4: login environment capture timeout.
    pub login_env_timeout_secs: u64,
    /// E1's keys.
    #[serde(flatten)]
    pub shell: ShellConfig,
    /// A1's keys.
    #[serde(flatten)]
    pub prompt: PromptConfig,
    /// E2's python keys.
    #[serde(flatten)]
    pub python: PythonConfig,
    /// E2's Monty keys.
    #[serde(flatten)]
    pub monty: MontyConfig,
}

impl Default for CellsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            kernel_idle_hours: 12,
            max_kernels: 16,
            monty_workers: 4,
            pty_cols: 100,
            pty_rows: 30,
            kill_after_secs: 5,
            agent_bytes: 8192,
            correct_rounds: 1,
            notify_after_secs: 30,
            after_max_age_mins: 60,
            render_scrollback: 10_000,
            ui_tail_lines: 200,
            export_output_bytes: 8192,
            replicate_bytes: 16_384,
            retention_days: 30,
            store_max_mb: 2048,
            login_env_timeout_secs: 10,
            shell: ShellConfig::default(),
            prompt: PromptConfig::default(),
            python: PythonConfig::default(),
            monty: MontyConfig::default(),
        }
    }
}

impl CellsConfig {
    /// The `cells:` section; defaults when absent or unreadable (logged).
    pub fn from_runtime(rt: &runtime::Runtime) -> Self {
        rt.section(crate::PLUGIN).unwrap_or_else(|e| {
            tracing::debug!("no [cells] config section, using defaults: {e}");
            Self::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_partial_sections_take_the_design_defaults() {
        let c: CellsConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(c, CellsConfig::default());
        let c: CellsConfig = serde_yaml::from_str("max_kernels: 3\nenabled: false").unwrap();
        assert_eq!((c.max_kernels, c.enabled, c.pty_cols), (3, false, 100));
    }
}
