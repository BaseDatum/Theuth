//! Design doc 155 §4.2 / §5: the supervisor side of the activity monitor —
//! a child's stall episode ([`ChildStalled`]) rendered as `<child_stalled>`
//! to the parent, `wait_peer` registration ([`WaitPeer`]), the settlement
//! check every state transition runs ([`Agents::check_peer_waits`]), and the
//! boot re-arm of `waits(peer_state)` rows ([`Agents::rebuild_peer_waits`]).
//! Wrappers `child_stalled` / `peer_state` live beside `job_idle` in
//! `supervisor.rs`.

use kameo::message::{Context, Message};
use runtime::SessionId;

use super::Agents;
use crate::activity::{ChildStalled, PeerWaitRegistered, WaitPeer};

impl Agents {
    /// 155 §5.3: recompute the §5.1 word for `child` and settle every
    /// registered wait it satisfies (or every wait on a `not_running` child).
    #[allow(dead_code)] // 155 C skeleton: W1 calls it from the transitions listed in §5.3.
    pub(crate) async fn check_peer_waits(&mut self, child: SessionId) {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = (child, &self.activity);
    }

    /// 155 §5.4: boot — `Store::peer_state_waits(Scope::Here)` → re-arm,
    /// `timeout` or `not_running` each row. After `rearm_reminders_for`,
    /// before `resume_scan`.
    #[allow(dead_code)] // 155 C skeleton: W1 wires it into the boot order.
    pub(crate) async fn rebuild_peer_waits(&mut self) {
        // ponytail: skeleton (155 C); W1 fills.
    }
}

impl Message<ChildStalled> for Agents {
    type Reply = ();

    /// 155 §4.2: `Some` records the episode and injects `<child_stalled>`
    /// into the parent; `None` clears it. Either way `check_peer_waits`.
    async fn handle(&mut self, m: ChildStalled, _: &mut Context<Self, Self::Reply>) {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = (m, &self.activity);
    }
}

impl Message<WaitPeer> for Agents {
    type Reply = Result<PeerWaitRegistered, String>;

    /// 155 §5.2: resolve `to` among the caller's children (pinned), validate
    /// `until` against the harness's `reports`, answer at once when the word
    /// already holds, else write the `peer_state` row and the in-memory wait.
    async fn handle(&mut self, m: WaitPeer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // ponytail: skeleton (155 C); W1 fills.
        let _ = (&m.until, m.timeout_secs, &self.activity);
        Err(format!(
            "wait_peer: not available yet (155 skeleton); asked for {} by {}",
            m.to, m.session
        ))
    }
}
