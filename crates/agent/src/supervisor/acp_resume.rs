//! Design doc 109 §7.3 (W4): the boot trigger that brings an ACP child
//! back when a parent's `child_result` wait names it (139 §7.4: the wait
//! row, scoped by the parent's owner, not the parent's open turn). The
//! child comes back through the ordinary [`Agents::session`] →
//! `acp_session` path (`session/load` on the stored harness id when there
//! is one) and is told [`Respawn`]; the actor decides between the restored
//! note and the delta handoff once it knows whether the load succeeded
//! (`acp_child.rs`).

use std::collections::HashSet;

use runtime::SessionId;

use super::{Agents, LiveActor};
use crate::acp_child::Respawn;
use crate::durability::ResumeMode;
use crate::store::Scope;

impl Agents {
    /// §4.6 step 10, after `resume_scan`: every `child_result` wait of a
    /// parent owned here (139 §7.4 — the parent's turn is usually finished
    /// by then, so not `open_turns`) whose target is an ACP child on a local
    /// harness respawns that child once. A child whose finished `result:`
    /// exchange the boot pass spared (§7.2) is not respawned — that result
    /// is delivered as the wait's answer. Honours `durability.resume`: only
    /// `auto` respawns anything.
    pub(super) async fn respawn_acp_children(&mut self) {
        if self.cfg.durability.resume != ResumeMode::Auto {
            return;
        }
        let waits = match self.store.child_waits(Scope::Here).await {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("109: child_waits: {e:#}");
                return;
            }
        };
        let mut seen = HashSet::new();
        for w in waits {
            let Ok(child) = w.target.parse::<SessionId>() else {
                continue;
            };
            if seen.insert(child)
                && let Err(e) = self.respawn_acp_child(w.session, child).await
            {
                tracing::error!(session = %child, parent = %w.session, "109: respawn: {e}");
            }
        }
    }

    /// One child of the table above. Native children are §4.6's business
    /// (their own open turn); a node's child cannot be respawned here. 139
    /// D13 gates, in order: the child is owned here (a child whose engine is
    /// gone is not respawned elsewhere), its harness is local, and its
    /// recorded `cwd` is a readable directory here. `Ok(true)` when an actor
    /// was spawned and told [`Respawn`].
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
        if !row.here {
            tracing::warn!(session = %child, owner = %row.owner, "139: child owned elsewhere; not respawned here");
            return Ok(false);
        }
        let (_, engine) = crate::peers::parse_agent(agent)?;
        if !acp::load(&self.rt.data_dir)
            .map_err(|e| e.to_string())?
            .contains_key(engine)
        {
            tracing::warn!(session = %child, "109: `{agent}` is not a local harness; child not respawned");
            return Ok(false);
        }
        // A recorded `cwd` must exist here; none recorded → the harness's
        // default, as the lazy `acp_session` path uses.
        if let Some(cwd) = row.cwd.as_deref()
            && std::fs::read_dir(cwd).is_err()
        {
            tracing::warn!(session = %child, cwd, "139: cwd is not a readable directory here; child not respawned");
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
    use tools::{Risk, WaitKind, WaitRow};

    use super::*;
    use crate::durability::StepKind;
    use crate::store::communication::AcceptExchange;
    use crate::store::durability::{calls_planned_in, step_in};
    use crate::supervisor::Live;
    use crate::supervisor::tests::agents_on;

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

    /// The §7.3 / 139 D13 table: a native child, a node's child, a child
    /// whose result the boot pass spared, a child with no row, a child owned
    /// by another engine and a child whose `cwd` is not here are not
    /// respawned; a local ACP child a parent here waits on is — once, and
    /// not again while its actor lives. A wait of a parent owned elsewhere
    /// is not this engine's trigger. `resume: off` respawns nothing.
    #[tokio::test]
    async fn respawn_decision_table() {
        let dir = std::env::temp_dir().join(format!("theuth-139-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        // Engine `a`: a parent waiting on its own ACP child, and an ACP child
        // a parent on `b` will wait on.
        let a = crate::store::Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let (a_parent, a_child, taken) = (SessionId::new(), SessionId::new(), SessionId::new());
        a.create_session(a_parent, "m", None, None, None)
            .await
            .unwrap();
        for (id, name) in [(a_child, "a-kid"), (taken, "taken")] {
            a.create_session(id, "omp", Some(a_parent), Some(name), None)
                .await
                .unwrap();
            a.set_acp(id, "omp", Some(&dir), &[]).await.unwrap();
        }
        a.wait_put(&wait(a_parent, "c1", a_child)).await.unwrap();
        a.close().await;

        let mut b = agents_on(dir.clone(), Some("b"), true).await;
        let store = b.store.clone();
        acp::save(
            &b.rt.data_dir,
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
        let children: Vec<SessionId> = (0..5).map(|_| SessionId::new()).collect();
        let [native, node, spared, acp_child, bad_cwd] = children[..] else {
            unreachable!()
        };
        let calls: Vec<(genai::chat::ToolCall, Risk)> = (1..=8)
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
            &json!({"call_ids": ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]}),
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
            ("bad_cwd", bad_cwd),
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
                let cwd = match name {
                    "bad_cwd" => Some(std::path::Path::new("/nonexistent/theuth-139-cwd")),
                    "acp" => Some(dir.as_path()),
                    _ => None,
                };
                store.set_acp(id, agent, cwd, &[]).await.unwrap();
            }
        }
        // Two waits on the ACP child (c4, c6): respawned once.
        let ghost = SessionId::new();
        for (i, child) in [
            native, node, spared, acp_child, ghost, acp_child, bad_cwd, taken,
        ]
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

        assert_eq!(b.respawn_acp_child(parent, native).await, Ok(false));
        assert_eq!(b.respawn_acp_child(parent, node).await, Ok(false));
        assert_eq!(b.respawn_acp_child(parent, spared).await, Ok(false));
        assert_eq!(b.respawn_acp_child(parent, ghost).await, Ok(false));
        assert_eq!(
            b.respawn_acp_child(parent, taken).await,
            Ok(false),
            "owned by a"
        );
        assert_eq!(
            b.respawn_acp_child(parent, bad_cwd).await,
            Ok(false),
            "cwd not here"
        );
        assert!(b.sessions.is_empty(), "nothing spawned so far");

        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = ResumeMode::Off;
        b.respawn_acp_children().await;
        assert!(b.sessions.is_empty(), "resume: off respawns nothing");

        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = ResumeMode::Auto;
        b.respawn_acp_children().await;
        assert_eq!(
            b.sessions.len(),
            1,
            "one actor for two waits on the same child, none for a's: {:?}",
            b.sessions.keys()
        );
        assert!(
            matches!(
                b.sessions.get(&acp_child),
                Some(Live {
                    actor: LiveActor::Acp(_),
                    ..
                })
            ),
            "the ACP child has a live actor"
        );
    }
}
