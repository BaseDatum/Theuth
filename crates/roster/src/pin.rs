//! The agent pin (79 §4.9, decision 14): `runs_in` on the agent file is a
//! placement constraint. The roster turns it into a [`tasks::place::Pin`] by
//! asking the sandbox actor to make sure the VM exists and runs
//! ([`sandbox::EnsureVm`]) and mapping the record's engine to the mesh peer
//! name `spawn_subagent` resolves. While the VM is not ready the task waits
//! `blocked` on [`blocker`]; [`Roster`](crate::Roster) clears that blocker
//! when [`sandbox::SandboxEvent::Changed`] says the VM took work again, and
//! the owner's desk re-runs `task_place`.

use tasks::place::Pin;

use crate::types::RunsIn;

/// The sandbox record a pin names: `vm` → `vm-<agent>`, `vm:<n>` → `n`.
pub fn record_name(agent: &str, runs_in: &RunsIn) -> Option<String> {
    match runs_in {
        RunsIn::Any => None,
        RunsIn::OwnVm => Some(format!("vm-{agent}")),
        RunsIn::Vm(n) => Some(n.clone()),
    }
}

/// Blocker text a pinned task waits on: `sandbox:<record name>`.
pub fn blocker(name: &str) -> String {
    format!("sandbox:{name}")
}

/// The record name a blocker names, when it is one of ours.
pub fn blocked_on(blocker: &str) -> Option<&str> {
    blocker.strip_prefix("sandbox:")
}

/// A VM in this state can take a work session now.
pub fn takes_work(state: &sandbox::SandboxState) -> bool {
    matches!(
        state,
        sandbox::SandboxState::Ready | sandbox::SandboxState::Running
    )
}

/// The mesh engine name behind `info`: the peer whose id is the record's
/// `engine`, or this engine's own name. `None` while the record has no engine,
/// is not in a working state, or the peer is not paired yet.
pub fn engine_name(info: &sandbox::SandboxInfo, mesh: &mesh::MeshStatus) -> Option<String> {
    if !takes_work(&info.state) {
        return None;
    }
    let id = info.engine.as_deref()?;
    if mesh.self_id.0 == id {
        return Some(mesh.self_name.clone());
    }
    mesh.peers
        .iter()
        .find(|p| p.id.0 == id)
        .map(|p| p.name.clone())
}

/// Resolve `agent`'s pin to an engine, or the blocker its task waits on.
/// `Ok(None)` = unpinned.
pub async fn resolve(agent: &str, runs_in: &RunsIn) -> Result<Option<Pin>, String> {
    let Some(name) = record_name(agent, runs_in) else {
        return Ok(None);
    };
    let blocked = || Err(blocker(&name));
    let Some(sandbox) = sandbox::actor() else {
        tracing::warn!(agent, sandbox = %name, "pinned agent: sandbox plugin is not running");
        return blocked();
    };
    let info = match sandbox
        .ask(sandbox::EnsureVm {
            name: name.clone(),
            agent: Some(agent.to_owned()),
        })
        .await
    {
        Ok(info) => info,
        Err(kameo::error::SendError::HandlerError(e)) => {
            tracing::warn!(agent, sandbox = %name, error = %e, "pinned agent: sandbox unavailable");
            return blocked();
        }
        Err(e) => {
            tracing::warn!(agent, sandbox = %name, error = %e, "pinned agent: sandbox actor");
            return blocked();
        }
    };
    let Some(mesh) = mesh::actor() else {
        return blocked();
    };
    let status = mesh.ask(mesh::Status).await.map_err(|e| e.to_string())?;
    match engine_name(&info, &status) {
        Some(engine) => Ok(Some(Pin::Engine(engine))),
        None => {
            tracing::info!(agent, sandbox = %name, state = ?info.state, engine = ?info.engine, "pinned agent: waiting for the VM");
            blocked()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(state: sandbox::SandboxState, engine: Option<&str>) -> sandbox::SandboxInfo {
        sandbox::SandboxInfo {
            name: "lab".into(),
            backend: "vm".into(),
            state,
            engine: engine.map(str::to_owned),
            vm: None,
            created_ms: 0,
            updated_ms: 0,
        }
    }

    fn status() -> mesh::MeshStatus {
        mesh::MeshStatus {
            self_id: mesh::EngineId("me".into()),
            self_name: "mac".into(),
            mesh_name: None,
            peers: vec![mesh::Peer {
                id: mesh::EngineId("vm1".into()),
                name: "lab".into(),
                addrs: vec![],
                reachable: true,
                last_seen_ms: 0,
                hub_pref: mesh::HubPref::Never,
                path: mesh::PeerPath::Offline,
                rtt_ms: None,
            }],
            lease: mesh::LeaseState {
                holder: None,
                since_ms: 0,
                reason: String::new(),
            },
            traffic: vec![],
            replica: None,
            stats: vec![],
            relay: None,
            messaging_off: vec![],
            nearby: true,
            quarantined: vec![],
        }
    }

    #[test]
    fn record_names_and_blockers() {
        assert_eq!(record_name("ops", &RunsIn::Any), None);
        assert_eq!(
            record_name("ops", &RunsIn::OwnVm).as_deref(),
            Some("vm-ops")
        );
        assert_eq!(
            record_name("ops", &RunsIn::Vm("lab".into())).as_deref(),
            Some("lab")
        );
        assert_eq!(blocked_on(&blocker("vm-ops")), Some("vm-ops"));
        assert_eq!(blocked_on("engine:lab"), None);
    }

    #[test]
    fn engine_name_needs_a_working_vm_with_a_known_engine() {
        use sandbox::SandboxState as S;
        let s = status();
        assert_eq!(
            engine_name(&info(S::Ready, Some("vm1")), &s).as_deref(),
            Some("lab")
        );
        assert_eq!(
            engine_name(&info(S::Running, Some("me")), &s).as_deref(),
            Some("mac")
        );
        assert_eq!(
            engine_name(&info(S::Ready, Some("unknown")), &s),
            None,
            "not paired yet"
        );
        assert_eq!(engine_name(&info(S::Ready, None), &s), None);
        assert_eq!(
            engine_name(
                &info(
                    S::Creating {
                        step: "pair".into()
                    },
                    Some("vm1")
                ),
                &s
            ),
            None
        );
        assert_eq!(engine_name(&info(S::Stopped, Some("vm1")), &s), None);
    }
}
