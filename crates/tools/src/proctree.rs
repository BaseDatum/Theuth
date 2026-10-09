//! 155 §6.2: is a PTY job's process tree busy or quiet? The rule over
//! successive [`platform::TreeSample`]s (taken about once a second while the
//! screen is unchanged) plus the foreground check: quiet means the tree's CPU
//! over the last sample stayed under `pty_tree_cpu_pct` of one core and the
//! program theuth started is still the terminal's foreground process group.
//! Pure — every method takes `now`; the job task (`pty::run`) feeds it.

use std::time::{Duration, Instant};

use platform::TreeSample;

use crate::ToolsConfig;

/// The two `tools.pty_*` thresholds (155 §6.6), wired through `StartTool::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyThresholds {
    /// `tools.pty_reaction_secs`; zero = the reaction check is off.
    pub reaction: Duration,
    /// `tools.pty_tree_cpu_pct`.
    pub tree_cpu_pct: u8,
}

impl PtyThresholds {
    /// From the `tools:` section.
    #[must_use]
    pub fn from_config(cfg: &ToolsConfig) -> Self {
        Self {
            reaction: Duration::from_secs(cfg.pty_reaction_secs),
            tree_cpu_pct: cfg.pty_tree_cpu_pct,
        }
    }
}

/// The §6.2 rule. Quiet until two samples exist (a tree that cannot be
/// sampled keeps the design-61 behaviour).
#[derive(Debug)]
pub struct TreeQuiet {
    thresholds: PtyThresholds,
    last: Option<(Instant, TreeSample)>,
    foreground_is_leader: bool,
    quiet: bool,
    moved_at: Option<Instant>,
}

impl TreeQuiet {
    /// A detector with nothing seen yet.
    #[must_use]
    pub fn new(thresholds: PtyThresholds) -> Self {
        Self {
            thresholds,
            last: None,
            foreground_is_leader: true,
            quiet: true,
            moved_at: None,
        }
    }

    /// One sample (`None` when the tree could not be read) and whether the
    /// terminal's foreground process group is still the leader's.
    pub fn saw(&mut self, now: Instant, sample: Option<TreeSample>, foreground_is_leader: bool) {
        // ponytail: skeleton (155 C); W2 fills the rule — this only records.
        let _ = self.thresholds;
        self.last = sample.map(|s| (now, s));
        self.foreground_is_leader = foreground_is_leader;
    }

    /// Nothing in the tree is working: the quiet-screen rule may fire.
    #[must_use]
    pub fn quiet(&self) -> bool {
        self.quiet && self.foreground_is_leader
    }

    /// The newest sample and when it was taken.
    #[must_use]
    pub fn last_sample(&self) -> Option<(Instant, TreeSample)> {
        self.last
    }

    /// The tree moved (CPU above the threshold, a foreground change) at or
    /// after `t` — the reaction check's "something happened".
    #[must_use]
    pub fn moved_since(&self, t: Instant) -> bool {
        self.moved_at.is_some_and(|m| m >= t)
    }
}
