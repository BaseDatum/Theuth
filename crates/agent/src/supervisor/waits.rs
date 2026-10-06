//! Design doc 109 §4.4: re-arm the `waits` rows of resumed turns into the
//! in-memory structures (`Agents.waits`, the jobs actor's waiters), the way
//! `rearm_reminders` rebuilds reminder timers. **W3 fills this file.**

use super::Agents;

impl Agents {
    /// Stub (contract wave): a no-op. W3 reads `Store::waits_of` per open
    /// turn and re-creates each wait with its remaining deadline
    /// (`peer_reply`, `child_result`, `pty_wait`, `question`); the boot pass
    /// over spared `result:` exchanges (§7.2) lives here too.
    pub(super) async fn rebuild_waits(&mut self) {}
}
