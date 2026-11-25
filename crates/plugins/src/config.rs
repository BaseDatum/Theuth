//! The `plugins:` config section (216 §3; contract §4). B0, frozen.

use serde::{Deserialize, Serialize};

/// `plugins:` in config.yaml (216 §3). Every key optional; a missing section is the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PluginsConfig {
    /// false: no actor, no processes, no plugin tools, no placements; folders untouched.
    pub enabled: bool,
    /// Plugin folders, relative to the data dir.
    pub dir: String,
    /// Hot-load plugin.yaml changes (300 ms debounce).
    pub watch: bool,
    /// Author mode: no code hash in grants, restart on any file change in the folder.
    pub dev: bool,
    /// Deliver frames of sessions this engine has seen `SessionOwnerChanged { here: false }` for
    /// (sessions owned by another engine of the mesh). Off by default (216 §17 Q2).
    pub remote_sessions: bool,
    /// First restart delay after a crash, ms; doubles per crash.
    pub restart_first_ms: u64,
    /// Ceiling of the restart delay, ms.
    pub restart_cap_ms: u64,
    /// Crashes within `crash_window_s` that mark a plugin `failed`.
    pub crash_loop: u32,
    /// Window of the crash-loop count, seconds.
    pub crash_window_s: u64,
    /// SIGTERM → SIGKILL grace on stop, ms.
    pub shutdown_grace_ms: u64,
    /// Longest stdout line read from a plugin, bytes; longer lines are discarded.
    pub max_line_bytes: usize,
    /// Event-lane queue length per plugin, frames; overflow sends one `lagged`.
    pub queue_frames: usize,
    /// `emit` calls per second per plugin.
    pub emit_per_s: u32,
    /// `ui` calls per second per plugin.
    pub ui_per_s: u32,
    /// OpenUI queries per second per plugin.
    pub query_per_s: u32,
    /// `input` requests per minute per plugin.
    pub input_per_min: u32,
    /// How long the host waits for a plugin's answer, ms.
    pub request_timeout_ms: u64,
    /// Longest string in an event delivered to a plugin, bytes.
    pub string_cap: usize,
    /// Size at which a plugin's log file rolls, bytes.
    pub log_max_bytes: u64,
    /// Days of plugin logs kept.
    pub log_days: u32,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: "plugins".into(),
            watch: true,
            dev: false,
            remote_sessions: false,
            restart_first_ms: 1000,
            restart_cap_ms: 300_000,
            crash_loop: 5,
            crash_window_s: 60,
            shutdown_grace_ms: 3000,
            max_line_bytes: 1_048_576,
            queue_frames: 2048,
            emit_per_s: 50,
            ui_per_s: 4,
            query_per_s: 10,
            input_per_min: 6,
            request_timeout_ms: 10_000,
            string_cap: 65_536,
            log_max_bytes: 10_485_760,
            log_days: 5,
        }
    }
}
