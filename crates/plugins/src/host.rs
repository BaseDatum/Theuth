//! Owner: B1 — boot (orphan reap, scan, start), per-plugin supervisors, lifecycle state machine.

use kameo::actor::ActorRef;

use crate::actor::PluginsActor;
use crate::config::PluginsConfig;

/// Background start: reap orphans from `.pgids`, scan `plugins/`, validate, start what the
/// authenticated grants cover, file cards for the rest, register the attention Source and the
/// tool registry entries, start the watcher. Never panics; logs and returns on shutdown.
pub(crate) async fn boot(rt: runtime::Runtime, actor: ActorRef<PluginsActor>, cfg: PluginsConfig) {
    let _ = (actor, cfg);
    tracing::debug!(data_dir = %rt.data_dir.display(), "plugins: host not implemented yet (216 B0)");
}
