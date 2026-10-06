//! Design doc 109 §7.3 (W4): the boot trigger that brings an ACP child
//! back when a resumed parent turn waits on its result. The child comes
//! back through the ordinary [`Agents::session`] → `acp_session` path
//! (`session/load` on the stored harness id when there is one) and is told
//! [`Respawn`]; the actor decides between the restored note and the delta
//! handoff once it knows whether the load succeeded (`acp_child.rs`).

use std::collections::HashSet;

use runtime::SessionId;
use tools::WaitKind;

use super::{Agents, LiveActor};
use crate::acp_child::Respawn;
use crate::durability::ResumeMode;

impl Agents {
    /// §4.6 step 10, after `resume_scan`: for every open turn, every
    /// `child_result` wait whose target is an ACP child (row with `agent`)
    /// on a local harness respawns that child once. A child whose finished
    /// `result:` exchange the boot pass spared (§7.2) is not respawned —
    /// that result is delivered as the wait's answer. Honours
    /// `durability.resume`: only `auto` respawns anything.
    pub(super) async fn respawn_acp_children(&mut self) {
        if self.cfg.durability.resume != ResumeMode::Auto {
            return;
        }
        let open = match self.store.open_turns().await {
            Ok(open) => open,
            Err(e) => {
                tracing::error!("109: open_turns: {e:#}");
                return;
            }
        };
        let mut seen = HashSet::new();
        for (parent, turn, _) in open {
            let waits = match self.store.waits_of(parent, turn).await {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(session = %parent, %turn, "109: waits_of: {e:#}");
                    continue;
                }
            };
            for w in waits
                .into_iter()
                .filter(|w| w.kind == WaitKind::ChildResult)
            {
                let Ok(child) = w.target.parse::<SessionId>() else {
                    continue;
                };
                if seen.insert(child)
                    && let Err(e) = self.respawn_acp_child(parent, child).await
                {
                    tracing::error!(session = %child, %parent, "109: respawn: {e}");
                }
            }
        }
    }

    /// One child of the table above. Native children are §4.6's business
    /// (their own open turn); a node's child cannot be respawned here.
    /// `Ok(true)` when an actor was spawned and told [`Respawn`].
    pub(super) async fn respawn_acp_child(
        &mut self,
        parent: SessionId,
        child: SessionId,
    ) -> Result<bool, String> {
        let row = match self.store.session(child).await {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(false),
            Err(e) => return Err(format!("{e:#}")),
        };
        let Some(agent) = row.agent.as_deref() else {
            return Ok(false);
        };
        let (_, engine) = crate::peers::parse_agent(agent)?;
        if !acp::load(&self.rt.data_dir)
            .map_err(|e| e.to_string())?
            .contains_key(engine)
        {
            tracing::warn!(session = %child, "109: `{agent}` is not a local harness; child not respawned");
            return Ok(false);
        }
        if self.live(child).is_some() {
            return Ok(false);
        }
        // §7.2: the child finished before the restart and its result is
        // spared `accepted` for the parent — nothing to respawn.
        let from = format!("session:{child}");
        let spared = self
            .store
            .pending_exchanges(parent, 64)
            .await
            .map_err(|e| format!("{e:#}"))?
            .iter()
            .any(|r| r.from == from);
        if spared {
            tracing::info!(session = %child, %parent, "109: child result already spared; not respawned");
            return Ok(false);
        }
        self.running.remove(&child);
        let actor = match self.session(Some(child), None, None, (None, None)).await {
            Ok((_, actor)) => actor,
            Err(e) => {
                let message = format!("{agent}: respawn after restart failed: {e:#}");
                self.error(child, message.clone()).await;
                return Err(message);
            }
        };
        if let LiveActor::Acp(a) = &actor {
            let _ = a.tell(Respawn(self.cfg.durability.handoff_tokens)).await;
        }
        Ok(true)
    }
}
