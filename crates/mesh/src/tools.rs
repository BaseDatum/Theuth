//! The `mesh_peers` tool (67 §4, §8), registered by `MeshPlugin::start` once
//! the tool registry is up.

use std::sync::{Arc, OnceLock};

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

/// Register the mesh tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>) {
    let _ = reg.ask(RegisterTool(Arc::new(MeshPeersTool))).await;
}

/// Text another plugin appends to a peer's `mesh_peers` line, by engine id
/// (sessionsync: the session-log replication with that engine).
pub type PeerNote = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

static PEER_NOTE: OnceLock<PeerNote> = OnceLock::new();

/// Install the [`PeerNote`]; once per process, later calls are ignored.
pub fn set_peer_note(note: PeerNote) {
    let _ = PEER_NOTE.set(note);
}

/// Arguments for [`MeshPeersTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct MeshPeersArgs {
    /// `true`: only engines reachable right now (default: every paired engine).
    #[serde(default)]
    pub reachable: bool,
}

/// `mesh_peers`: the engines in the mesh and what each advertises, so theuth
/// can place a task (`where`, `harness`).
pub struct MeshPeersTool;

#[async_trait::async_trait]
impl Tool for MeshPeersTool {
    const NAME: &'static str = "mesh_peers";
    const RISK: Risk = Risk::ReadOnly;
    type Args = MeshPeersArgs;

    fn description(&self) -> String {
        "List the engines in this harness's mesh: one line per engine with its name, id, whether it is \
         reachable now, its class (server / desktop / laptop / container / vm), which harnesses it has installed \
         (native, omp, opencode, claude), the repo paths it can open, whether it has a display and a GPU, and \
         which one holds the hub lease; for each other engine, when known, the state of session \
         replication with it (`sessions=up|down`, rows behind each way, held, last error). Use it before \
         creating or assigning a task to choose `where` (an engine name) and `harness` from what the task \
         needs; pass `reachable: true` to see only engines that can start work right now."
            .into()
    }

    async fn call(&self, args: MeshPeersArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = crate::actor() else {
            return Ok(ToolOutput::err("mesh is not running"));
        };
        let status = match actor.ask(crate::Status).await {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let stats = match actor.ask(crate::ListStats).await {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let hub = status.lease.holder.clone();
        let mut lines = Vec::new();
        let me = crate::Peer {
            id: status.self_id.clone(),
            name: "this engine".into(),
            addrs: vec![],
            reachable: true,
            last_seen_ms: 0,
            hub_pref: crate::HubPref::Eligible,
            path: crate::PeerPath::default(),
            rtt_ms: None,
        };
        for p in std::iter::once(&me).chain(status.peers.iter()) {
            if args.reachable && !p.reachable {
                continue;
            }
            let mut line = peer_line(p, stats.iter().find(|s| s.id == p.id), hub.as_ref());
            if p.id != status.self_id
                && let Some(note) = PEER_NOTE.get().and_then(|f| f(&p.id.0))
            {
                line.push(' ');
                line.push_str(&note);
            }
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push("no engines match".into());
        }
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

/// One `mesh_peers` line: `<name> id=<id> <reach> class=<class> harnesses=… repos=… display=… gpu=…[ accel=…][ hub]`.
/// `accel` (126 §8.4) is printed only when the engine advertised one.
fn peer_line(
    p: &crate::Peer,
    s: Option<&crate::EngineStats>,
    hub: Option<&crate::EngineId>,
) -> String {
    let (class, harnesses, repos, display, gpu, accel) = match s {
        Some(s) => (
            crate::store::class_str(s.class).to_owned(),
            s.harnesses.join(","),
            if s.repos.is_empty() {
                "-".to_owned()
            } else {
                s.repos.join(",")
            },
            s.display,
            s.gpu,
            s.accelerator
                .map(|a| format!(" accel={}", a.as_str()))
                .unwrap_or_default(),
        ),
        None => (
            "unknown".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
            false,
            false,
            String::new(),
        ),
    };
    format!(
        "{name} id={id} {reach} class={class} harnesses={harnesses} repos={repos} display={display} gpu={gpu}{accel}{hub}",
        name = p.name,
        id = p.id,
        reach = if p.reachable {
            "reachable"
        } else {
            "unreachable"
        },
        hub = if hub == Some(&p.id) { " hub" } else { "" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_line_prints_class_vm() {
        let p = crate::Peer {
            id: crate::EngineId("id1".into()),
            name: "lab".into(),
            addrs: vec![],
            reachable: true,
            last_seen_ms: 0,
            hub_pref: crate::HubPref::Never,
            path: crate::PeerPath::Offline,
            rtt_ms: None,
        };
        let s = crate::EngineStats {
            id: p.id.clone(),
            uptime_since_ms: 0,
            class: crate::EngineClass::Vm,
            availability_30d: 1.0,
            harnesses: vec!["native".into(), "omp".into()],
            repos: vec![],
            display: true,
            gpu: false,
            accelerator: None,
            os: String::new(),
            os_name: String::new(),
            arch: String::new(),
            host: String::new(),
        };
        assert_eq!(
            peer_line(&p, Some(&s), None),
            "lab id=id1 reachable class=vm harnesses=native,omp repos=- display=true gpu=false"
        );
        let s = crate::EngineStats {
            gpu: true,
            accelerator: Some(crate::Accelerator::Metal),
            ..s
        };
        assert!(
            peer_line(&p, Some(&s), None).ends_with("display=true gpu=true accel=metal"),
            "{}",
            peer_line(&p, Some(&s), None)
        );
        assert!(
            peer_line(&p, None, Some(&p.id))
                .ends_with("class=unknown harnesses=- repos=- display=false gpu=false hub")
        );
    }
}
