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
    /// Windows only (§13.7): the member count at the first sample; more
    /// members than that is the weaker "something is in front".
    baseline: Option<u32>,
    foreground_is_leader: bool,
    cpu_quiet: bool,
    moved_at: Option<Instant>,
}

impl TreeQuiet {
    /// A detector with nothing seen yet.
    #[must_use]
    pub fn new(thresholds: PtyThresholds) -> Self {
        Self {
            thresholds,
            last: None,
            baseline: None,
            foreground_is_leader: true,
            cpu_quiet: true,
            moved_at: None,
        }
    }

    /// One sample (`None` when the tree could not be read: the verdict and
    /// the previous sample stand) and whether the terminal's foreground
    /// process group is still the leader's.
    pub fn saw(&mut self, now: Instant, sample: Option<TreeSample>, foreground_is_leader: bool) {
        let mut foreground = foreground_is_leader;
        if let Some(s) = sample {
            if cfg!(windows) {
                let base = *self.baseline.get_or_insert(s.processes);
                foreground &= s.processes <= base;
            }
            if let Some((at, prev)) = self.last {
                let window = now.saturating_duration_since(at);
                if !window.is_zero() {
                    let used = s.cpu.saturating_sub(prev.cpu).as_secs_f64();
                    let pct = used / window.as_secs_f64() * 100.0;
                    self.cpu_quiet = pct <= f64::from(self.thresholds.tree_cpu_pct);
                }
            }
            self.last = Some((now, s));
        }
        if !self.cpu_quiet || foreground != self.foreground_is_leader {
            self.moved_at = Some(now);
        }
        self.foreground_is_leader = foreground;
    }

    /// Nothing in the tree is working: the quiet-screen rule may fire.
    #[must_use]
    pub fn quiet(&self) -> bool {
        self.cpu_quiet && self.foreground_is_leader
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

#[cfg(test)]
mod tests {
    use super::*;

    const T: PtyThresholds = PtyThresholds {
        reaction: Duration::from_secs(10),
        tree_cpu_pct: 2,
    };

    fn sample(ms: u64) -> Option<TreeSample> {
        Some(TreeSample {
            processes: 1,
            cpu: Duration::from_millis(ms),
        })
    }

    /// §11: quiet with no samples, busy over the CPU threshold, busy when the
    /// leader is not in front, quiet again after one quiet sample; `moved_since`.
    #[test]
    fn tree_quiet_rule() {
        let t0 = Instant::now();
        let s = |secs| t0 + Duration::from_secs(secs);
        let mut q = TreeQuiet::new(T);
        assert!(q.quiet());
        assert!(!q.moved_since(t0));

        q.saw(s(1), sample(100), true);
        assert!(q.quiet(), "one sample is not a rate");
        assert!(!q.moved_since(t0));
        assert_eq!(q.last_sample().map(|(at, _)| at), Some(s(1)));

        // 10 ms over 1 s = 1 %: under the 2 % threshold.
        q.saw(s(2), sample(110), true);
        assert!(q.quiet());
        assert!(!q.moved_since(t0));

        // 50 ms over 1 s = 5 %: busy, and that is movement at s(3).
        q.saw(s(3), sample(160), true);
        assert!(!q.quiet());
        assert!(q.moved_since(s(3)));
        assert!(!q.moved_since(s(4)));

        // A quiet sample makes it quiet again at once.
        q.saw(s(4), sample(165), true);
        assert!(q.quiet());
        assert!(!q.moved_since(s(4)));

        // Something else in front: busy; the change is movement.
        q.saw(s(5), sample(165), false);
        assert!(!q.quiet());
        assert!(q.moved_since(s(5)));
        // Still in front, unchanged: busy but no new movement.
        q.saw(s(6), sample(165), false);
        assert!(!q.quiet());
        assert!(!q.moved_since(s(6)));
        // Back to the leader: quiet; the change is movement.
        q.saw(s(7), sample(165), true);
        assert!(q.quiet());
        assert!(q.moved_since(s(7)));

        // An unreadable tree keeps the last verdict and sample.
        q.saw(s(8), sample(300), true);
        assert!(!q.quiet());
        q.saw(s(9), None, true);
        assert!(!q.quiet());
        assert_eq!(q.last_sample().map(|(at, _)| at), Some(s(8)));
    }
}
