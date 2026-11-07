//! Design doc 139 §7.4: the `waits(child_result)` row — written when a
//! child is spawned, cleared when its result reaches the parent, settled
//! when the engine that ran the child is gone after a takeover. The hooks:
//! `SpawnChild`'s success arm → [`child_spawned`], `Agents::hand_off` →
//! [`child_result_delivered`], `transfer::on_resume_here` (W2) →
//! [`settle_on_transfer`], `drop_queued` / `DeleteSession` → [`child_gone`]
//! (178 W8). `Store::child_waits(Scope)` (C) is the reader;
//! `cut_journal_in` leaves `child_result` rows alone (Revision 1 Q8).

use replica::PeerId;
use runtime::SessionId;
use serde_json::json;
use tools::{WaitKind, WaitRow};

use super::Agents;
use crate::durability::handoff::compose_handoff;
use crate::store::Scope;

/// A child of `parent` was spawned by the tool call `call_id` (`SpawnChild`
/// success, before `SubagentSpawned`): writes `waits { session: parent,
/// tool_call_id: call_id, kind: ChildResult, target: child, deadline: None,
/// payload: {"agent": agent} }`. A spawn with no call behind it (an empty
/// `call_id`: nothing will resume it) writes nothing.
pub async fn child_spawned(
    agents: &mut Agents,
    parent: SessionId,
    child: SessionId,
    call_id: &str,
    agent: Option<&str>,
) {
    if call_id.is_empty() {
        return;
    }
    let row = WaitRow {
        session: parent,
        tool_call_id: call_id.to_owned(),
        kind: WaitKind::ChildResult,
        target: child.to_string(),
        deadline: None,
        payload: json!({ "agent": agent }),
    };
    if let Err(e) = agents.store.wait_put(&row).await {
        tracing::warn!(session = %parent, %child, "139: child_result wait not written: {e:#}");
    }
}

/// `child`'s `result:` exchange was consumed by `parent` (`hand_off`): the
/// row `target = child` is cleared.
pub async fn child_result_delivered(agents: &mut Agents, parent: SessionId, child: SessionId) {
    for w in waits_of(agents, parent).await {
        if w.target == child.to_string()
            && let Err(e) = agents.store.wait_clear(parent, &w.tool_call_id).await
        {
            tracing::warn!(session = %parent, %child, "139: child_result wait not cleared: {e:#}");
        }
    }
}

/// 178 W8: `children` will never return a result — a queued spawn dropped
/// before it started, or a deleted session. Every `child_result` row on
/// them is cleared, and its parent told once (`why` after the child's
/// label) in [`settle_on_transfer`]'s style — unless the parent is in
/// `quiet` (deleted with them) or closing.
pub async fn child_gone(
    agents: &mut Agents,
    children: &[SessionId],
    why: &str,
    quiet: &[SessionId],
) {
    let rows = match agents.store.child_waits(Scope::Here).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("139: child_waits: {e:#}");
            return;
        }
    };
    for w in rows {
        let Some(child) = children.iter().copied().find(|c| w.target == c.to_string()) else {
            continue;
        };
        if let Err(e) = agents.store.wait_clear(w.session, &w.tool_call_id).await {
            tracing::warn!(session = %w.session, %child, "139: child_result wait not cleared: {e:#}");
            continue;
        }
        if quiet.contains(&w.session) || agents.closing.contains(&w.session) {
            continue;
        }
        let name = match agents.store.session(child).await {
            Ok(Some(row)) => row.name,
            _ => None,
        };
        let body = format!(
            "{} {why}; no result will come from it.",
            label(&name, child)
        );
        let wrapped = format!(
            "<message from=\"session:{child}\" at=\"{}\">\n{body}\n</message>",
            crate::clock::hhmm(chrono::Local::now())
        );
        agents
            .inject(w.session, &format!("session:{child}"), body, wrapped, None)
            .await;
    }
}

/// 139 §7.4 step 3, on the new owner of `parent` after a transfer or
/// takeover from `from`: a child owned by a reachable engine → nothing (its
/// result is forwarded, 69 P); a child owned by `from` while `from` is
/// unreachable → `wait_clear` + an injected `<message from="session:
/// <child>">` saying the result is lost, with `compose_handoff` when the
/// child's `cwd` is a readable directory here.
pub async fn settle_on_transfer(agents: &mut Agents, parent: SessionId, from: Option<&str>) {
    let Some(from) = from.filter(|f| !f.is_empty()) else {
        return;
    };
    let waits = waits_of(agents, parent).await;
    if waits.is_empty() {
        return;
    }
    let reachable = match agents.transport().await {
        Some(t) => t.peers().contains(&PeerId(from.to_owned())),
        None => false,
    };
    if reachable {
        return;
    }
    for w in waits {
        let Ok(child) = w.target.parse::<SessionId>() else {
            continue;
        };
        let Ok(Some(row)) = agents.store.session(child).await else {
            continue;
        };
        if row.here || row.owner != from {
            continue;
        }
        if let Err(e) = agents.store.wait_clear(parent, &w.tool_call_id).await {
            tracing::warn!(session = %parent, %child, "139: child_result wait not cleared: {e:#}");
        }
        let mut body = format!(
            "{} was running on {from}, which is unreachable; its result is lost. ",
            label(&row.name, child)
        );
        if row
            .cwd
            .as_deref()
            .is_some_and(|c| std::path::Path::new(c).is_dir())
        {
            body.push_str(
                &compose_handoff(&agents.store, child, agents.cfg.durability.handoff_tokens).await,
            );
        }
        let wrapped = format!(
            "<message from=\"session:{child}\" at=\"{}\">\n{body}\n</message>",
            crate::clock::hhmm(chrono::Local::now())
        );
        agents
            .inject(parent, &format!("session:{child}"), body, wrapped, None)
            .await;
    }
}

/// The `child_result` waits of `parent` (a session owned here).
async fn waits_of(agents: &Agents, parent: SessionId) -> Vec<WaitRow> {
    match agents.store.child_waits(Scope::Here).await {
        Ok(rows) => rows.into_iter().filter(|w| w.session == parent).collect(),
        Err(e) => {
            tracing::error!(session = %parent, "139: child_waits: {e:#}");
            Vec::new()
        }
    }
}

/// `name (session:<id>)` or `session:<id>`.
fn label(name: &Option<String>, child: SessionId) -> String {
    match name {
        Some(n) => format!("`{n}` (session:{child})"),
        None => format!("session:{child}"),
    }
}

#[cfg(test)]
mod tests {
    use kameo::actor::Spawn;
    use kameo::message::{Context, Message};
    use tokio::sync::mpsc::UnboundedSender;

    use super::*;
    use crate::AgentEvent;
    use crate::communication::mesh as peer_mesh;
    use crate::store::Store;
    use crate::supervisor::tests::agents_on;

    /// Every `AgentEvent` on the fixture's bus.
    #[derive(kameo::Actor)]
    struct Sink(UnboundedSender<AgentEvent>);

    impl Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    fn temp() -> crate::TestDir {
        crate::TestDir::new("139-w3")
    }

    /// 139 §7.4 (1): `child_spawned` writes the `child_result` row with the
    /// spawn's call id and agent; `child_result_delivered` clears exactly
    /// that child's row. An empty call id writes nothing.
    #[tokio::test]
    async fn child_result_wait_written_and_cleared() {
        let scratch = temp();
        let mut a = agents_on(scratch.0.clone(), Some("b"), true).await;
        let parent = SessionId::new();
        a.store
            .create_session(parent, "m", None, None, None)
            .await
            .unwrap();
        let (c1, c2, c3) = (SessionId::new(), SessionId::new(), SessionId::new());
        child_spawned(&mut a, parent, c1, "call-1", Some("worker@a")).await;
        child_spawned(&mut a, parent, c2, "call-2", None).await;
        child_spawned(&mut a, parent, c3, "", None).await;
        let rows = a.store.child_waits(Scope::Here).await.unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        let r1 = rows.iter().find(|w| w.tool_call_id == "call-1").unwrap();
        assert_eq!(
            (r1.session, r1.kind, r1.target.as_str(), r1.deadline),
            (parent, WaitKind::ChildResult, c1.to_string().as_str(), None)
        );
        assert_eq!(r1.payload, json!({"agent": "worker@a"}));
        assert_eq!(
            rows.iter()
                .find(|w| w.tool_call_id == "call-2")
                .unwrap()
                .payload,
            json!({"agent": null})
        );

        child_result_delivered(&mut a, parent, c1).await;
        let rows = a.store.child_waits(Scope::Here).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, c2.to_string());
        // Clearing again, or an unknown child: no-op.
        child_result_delivered(&mut a, parent, c1).await;
        child_result_delivered(&mut a, parent, c3).await;
        assert_eq!(a.store.child_waits(Scope::Here).await.unwrap().len(), 1);
    }

    /// 139 §7.4 (3): on `b` after a takeover from `a`, a child owned by `a`
    /// is settled lost — wait cleared, `<message from="session:<child>">`
    /// injected — with the handoff sections when the child's `cwd` is a
    /// directory here and without them when it is not; a child owned
    /// elsewhere, or `a` reachable, leaves everything in place.
    #[tokio::test]
    async fn settle_on_transfer_injects_the_lost_note() {
        let scratch = temp();
        let dir = scratch.0.clone();
        let parent = SessionId::new();
        let (with_cwd, without_cwd, on_c) = (SessionId::new(), SessionId::new(), SessionId::new());
        let worktree = dir.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        // `a` created the children (owner = a) and recorded their `cwd`
        // (an owner write); `c` owns a third one.
        for (engine, ids) in [("a", vec![with_cwd, without_cwd]), ("c", vec![on_c])] {
            let s = Store::open(
                &dir.join("sessions.sqlite"),
                Some(mesh::EngineId(engine.into())),
            )
            .await
            .unwrap();
            for id in ids {
                s.create_session(id, "m", None, Some("kid"), None)
                    .await
                    .unwrap();
            }
            if engine == "a" {
                s.set_cwd(with_cwd, &worktree).await.unwrap();
                s.set_cwd(without_cwd, &dir.join("gone")).await.unwrap();
            }
            s.close().await;
        }
        let mut b = agents_on(dir.clone(), Some("b"), true).await;
        b.store
            .create_session(parent, "m", None, None, None)
            .await
            .unwrap();
        for (i, child) in [with_cwd, without_cwd, on_c].iter().enumerate() {
            child_spawned(&mut b, parent, *child, &format!("call-{i}"), None).await;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        b.rt.subscribe::<Sink, AgentEvent>(&sink).await;

        // `a` reachable: nothing is settled.
        let net = peer_mesh::loopback::nodes(&["a", "b"]);
        b.transport = Some(net[1].clone());
        settle_on_transfer(&mut b, parent, Some("a")).await;
        assert_eq!(b.store.child_waits(Scope::Here).await.unwrap().len(), 3);
        // No previous owner known: nothing either.
        b.transport = None;
        settle_on_transfer(&mut b, parent, None).await;
        assert_eq!(b.store.child_waits(Scope::Here).await.unwrap().len(), 3);

        // `a` unreachable (no mesh): its two children are lost; `c`'s stays.
        settle_on_transfer(&mut b, parent, Some("a")).await;
        let rows = b.store.child_waits(Scope::Here).await.unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].target, on_c.to_string());
        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: parent,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let mut delivered = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let AgentEvent::MessageDelivered {
                session,
                from,
                text,
            } = e
                && session == parent
            {
                delivered.push((from, text));
            }
        }
        assert_eq!(delivered.len(), 2, "{delivered:?}");
        let text_of = |child: SessionId| {
            delivered
                .iter()
                .find(|(f, _)| *f == format!("session:{child}"))
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| panic!("no note for {child}: {delivered:?}"))
        };
        let with = text_of(with_cwd);
        assert!(
            with.starts_with(&format!(
                "`kid` (session:{with_cwd}) was running on a, which is unreachable; its result is lost. "
            )),
            "{with}"
        );
        assert!(
            with.contains("<resume_handoff>"),
            "handoff sections expected: {with}"
        );
        let without = text_of(without_cwd);
        assert_eq!(
            without,
            format!(
                "`kid` (session:{without_cwd}) was running on a, which is unreachable; its result is lost. "
            )
        );
    }
}
