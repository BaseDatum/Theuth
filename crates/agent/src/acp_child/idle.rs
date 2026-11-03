//! 176 A1: when an external ACP harness child may be parked — its process
//! stopped until the next input resumes it through `session/load`.

use std::time::Duration;

/// Everything the park decision reads; the supervisor fills `pending` and
/// `jobs`, the child the rest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IdleFacts {
    /// A local harness process (`agents.yaml`), not a dialled node.
    pub process: bool,
    /// `initialize` + `session/new|load` succeeded and the connection is up.
    pub connected: bool,
    /// A prompt or continuation turn in flight, inputs queued, a respawn
    /// pending or option changes waiting for a turn's end.
    pub busy: bool,
    /// Supervisor side: a turn running, an approval or ask unanswered, a
    /// peer wait, a spawn waiting for its permit.
    pub pending: bool,
    /// Running jobs and PTYs of the session.
    pub jobs: u32,
    /// A watch-only child still running inside the harness (141).
    pub observed: bool,
    /// Since the last ACP traffic in either direction.
    pub quiet: Duration,
}

/// Whether a child with `f` has been idle for at least `after`.
pub(crate) fn parkable(f: &IdleFacts, after: Duration) -> bool {
    f.process
        && f.connected
        && !f.busy
        && !f.pending
        && f.jobs == 0
        && !f.observed
        && f.quiet >= after
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: Duration = Duration::from_secs(1800);

    fn idle() -> IdleFacts {
        IdleFacts {
            process: true,
            connected: true,
            busy: false,
            pending: false,
            jobs: 0,
            observed: false,
            quiet: AFTER,
        }
    }

    /// Exactly at the threshold parks; every exclusion alone keeps it.
    #[test]
    fn each_exclusion_keeps_the_child() {
        assert!(parkable(&idle(), AFTER));
        let kept = [
            (
                "dialled node",
                IdleFacts {
                    process: false,
                    ..idle()
                },
            ),
            (
                "not connected",
                IdleFacts {
                    connected: false,
                    ..idle()
                },
            ),
            (
                "turn or queue",
                IdleFacts {
                    busy: true,
                    ..idle()
                },
            ),
            (
                "approval/ask/wait",
                IdleFacts {
                    pending: true,
                    ..idle()
                },
            ),
            ("job or pty", IdleFacts { jobs: 1, ..idle() }),
            (
                "watch-only child",
                IdleFacts {
                    observed: true,
                    ..idle()
                },
            ),
            (
                "recent traffic",
                IdleFacts {
                    quiet: AFTER - Duration::from_millis(1),
                    ..idle()
                },
            ),
        ];
        for (why, f) in kept {
            assert!(!parkable(&f, AFTER), "{why}");
        }
    }
}
