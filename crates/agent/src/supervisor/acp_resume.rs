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
use crate::store::Scope;

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
        // 139 §7.4 (W1): iterate `child_waits(Scope::Here)` instead, with the
        // `row.here` + readable-`cwd` gates.
        let open = match self.store.open_turns(Scope::Here).await {
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

#[cfg(test)]
mod tests {
    use runtime::TurnId;
    use serde_json::json;
    use tools::{Risk, WaitRow};

    use super::*;
    use crate::durability::StepKind;
    use crate::store::communication::AcceptExchange;
    use crate::store::durability::{calls_planned_in, step_in};
    use crate::supervisor::Live;
    use crate::supervisor::tests::agents_with;

    fn wait(parent: SessionId, call: &str, child: SessionId) -> WaitRow {
        WaitRow {
            session: parent,
            tool_call_id: call.into(),
            kind: WaitKind::ChildResult,
            target: child.to_string(),
            deadline: None,
            payload: json!({}),
        }
    }

    /// The §7.3 table: a native child, a node's child, a child whose result
    /// the boot pass spared and a child with no row are not respawned; a
    /// local ACP child whose parent's open turn waits on it is — once, and
    /// not again while its actor lives. `resume: off` respawns nothing.
    #[tokio::test]
    async fn respawn_decision_table() {
        let mut a = agents_with(true).await;
        let store = a.store.clone();
        acp::save(
            &a.rt.data_dir,
            &serde_yaml::from_str("omp:\n  command: /nonexistent/theuth-test-harness\n").unwrap(),
        )
        .unwrap();
        let parent = SessionId::new();
        store
            .create_session(parent, "m", None, None, None)
            .await
            .unwrap();
        let turn = TurnId::new();
        store
            .step(parent, turn, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        let children: Vec<SessionId> = (0..4).map(|_| SessionId::new()).collect();
        let [native, node, spared, acp_child] = children[..] else {
            unreachable!()
        };
        let calls: Vec<(genai::chat::ToolCall, Risk)> = (1..=6)
            .map(|i| {
                (
                    genai::chat::ToolCall {
                        call_id: format!("c{i}"),
                        fn_name: "send_message".into(),
                        fn_arguments: json!({}),
                        thought_signatures: None,
                    },
                    Risk::ReadOnly,
                )
            })
            .collect();
        let mut tx = store.begin_write().await.unwrap();
        step_in(
            &mut tx,
            parent,
            turn,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ["c1", "c2", "c3", "c4", "c5", "c6"]}),
        )
        .await
        .unwrap();
        calls_planned_in(&mut tx, parent, turn, 0, &calls)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for (name, id) in [
            ("native", native),
            ("node", node),
            ("spared", spared),
            ("acp", acp_child),
        ] {
            let agent = match name {
                "native" => "m",
                "node" => "omp@box",
                _ => "omp",
            };
            store
                .create_session(id, agent, Some(parent), Some(name), None)
                .await
                .unwrap();
            if name != "native" {
                store.set_acp(id, agent, None, &[]).await.unwrap();
            }
        }
        // Two waits on the ACP child (c4, c6): respawned once.
        let ghost = SessionId::new();
        for (i, child) in [native, node, spared, acp_child, ghost, acp_child]
            .iter()
            .enumerate()
        {
            store
                .wait_put(&wait(parent, &format!("c{}", i + 1), *child))
                .await
                .unwrap();
        }
        let (from, to) = (format!("session:{spared}"), format!("session:{parent}"));
        let request_id = format!("result:{}", TurnId::new());
        store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(spared),
                recipient_session: Some(parent),
                request_id: Some(&request_id),
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "done before the crash",
            })
            .await
            .unwrap();

        assert_eq!(a.respawn_acp_child(parent, native).await, Ok(false));
        assert_eq!(a.respawn_acp_child(parent, node).await, Ok(false));
        assert_eq!(a.respawn_acp_child(parent, spared).await, Ok(false));
        assert_eq!(a.respawn_acp_child(parent, ghost).await, Ok(false));
        assert!(a.sessions.is_empty(), "nothing spawned so far");

        std::sync::Arc::make_mut(&mut a.cfg).durability.resume = ResumeMode::Off;
        a.respawn_acp_children().await;
        assert!(a.sessions.is_empty(), "resume: off respawns nothing");

        std::sync::Arc::make_mut(&mut a.cfg).durability.resume = ResumeMode::Auto;
        a.respawn_acp_children().await;
        assert_eq!(
            a.sessions.len(),
            1,
            "one actor for two waits on the same child: {:?}",
            a.sessions.keys()
        );
        assert!(
            matches!(
                a.sessions.get(&acp_child),
                Some(Live {
                    actor: LiveActor::Acp(_),
                    ..
                })
            ),
            "the ACP child has a live actor"
        );
    }
}
