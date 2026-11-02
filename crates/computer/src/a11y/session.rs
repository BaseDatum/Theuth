//! One theuth session's agent-desktop state on its worker thread (161 §3.2):
//! the platform adapter, macOS's retained AX identity, and the session
//! namespace on disk. Only the adapter constructor and `RetainedRefSession`
//! are cfg-gated (D7).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_desktop_core::context::CommandContext;
use agent_desktop_core::session::{
    self, ArtifactsMode, GcOptions, SessionManifest, SessionTraceMode,
};
use agent_desktop_core::{
    AdapterError, AppError, CursorOverlayConfig, DeliverySemantics, PlatformAdapter,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::command::Command;

/// Where a session worker's state comes from and goes: [`Backend::REAL`],
/// or fakes in tests so no test touches the desktop or `~/.agent-desktop`.
#[derive(Clone, Copy)]
pub(super) struct Backend {
    /// Builds the platform adapter, once per worker.
    pub(super) adapter: fn() -> Result<Box<dyn PlatformAdapter>, AppError>,
    /// Makes the namespace an active agent-desktop session named for the
    /// engine, before every command: `(namespace, engine)`.
    pub(super) open: fn(&str, &str) -> Result<(), AppError>,
    /// Reclaims a retired worker's namespace.
    pub(super) reclaim: fn(&str),
}

impl Backend {
    /// agent-desktop's adapter and session store.
    pub(super) const REAL: Self = Self {
        adapter,
        open: open_namespace,
        reclaim,
    };
}

/// Retained AX objects are pruned to the saved snapshots' tokens after this
/// many captures, as agent-desktop's command host does
/// (AD:`src/command_host/mod.rs:9`, `:56-69`).
#[cfg(target_os = "macos")]
const PRUNE_INTERVAL: usize = 4_096;

/// What a session worker holds once its first command opened it.
pub(super) struct Live {
    adapter: Box<dyn PlatformAdapter>,
    #[cfg(target_os = "macos")]
    retained: Retained,
}

/// macOS: retained AX identity, owned by this thread until dropped
/// (AD:`crates/macos/src/tree/resolve/retained.rs:13-39`).
#[cfg(target_os = "macos")]
struct Retained {
    owner: agent_desktop_macos::RetainedRefSession,
    next_prune: usize,
}

impl Live {
    fn open(backend: Backend) -> Result<Self, AppError> {
        Ok(Self {
            #[cfg(target_os = "macos")]
            retained: Retained {
                owner: agent_desktop_macos::RetainedRefSession::start()?,
                next_prune: PRUNE_INTERVAL,
            },
            adapter: (backend.adapter)()?,
        })
    }

    #[cfg(target_os = "macos")]
    fn prune(&mut self, namespace: &str) {
        let r = &mut self.retained;
        if r.owner.capture_count() < r.next_prune {
            return;
        }
        match agent_desktop_core::RefStore::for_session(Some(namespace))
            .and_then(|store| store.retained_object_tokens())
        {
            Ok(tokens) => r.owner.retain(&tokens),
            Err(error) => {
                tracing::warn!(%error, "computer: retained-token inventory failed; pruning deferred")
            }
        }
        r.next_prune = r.owner.capture_count().saturating_add(PRUNE_INTERVAL);
    }
}

/// Run `command` in `namespace`, opening the worker's state first if needed.
/// The namespace is checked before every command: a predecessor that idled
/// out may still be reclaiming it, and an ended namespace is reopened (§3.2).
pub(super) fn execute(
    live: &mut Option<Live>,
    backend: Backend,
    namespace: &str,
    engine: &str,
    command: Command,
) -> Result<Value, AppError> {
    let live = match live {
        Some(live) => live,
        None => live.insert(Live::open(backend).map_err(not_delivered)?),
    };
    #[cfg(target_os = "macos")]
    live.prune(namespace);
    (backend.open)(namespace, engine).map_err(not_delivered)?;
    let cx = CommandContext::new(Some(namespace.to_owned()), None, false)
        .map_err(not_delivered)?
        .with_headed(false);
    command.dispatch(live.adapter.as_ref(), &cx)
}

/// A failure before dispatch sent nothing (AD:`src/main.rs:314-325`).
fn not_delivered(error: AppError) -> AppError {
    let source = match error {
        AppError::Adapter(source) => source,
        other => AdapterError::internal(other.to_string()),
    };
    source
        .with_disposition(DeliverySemantics::not_delivered())
        .into()
}

/// The manifest name of this engine's namespaces: `theuth:` and a short
/// stable hash of its data dir (161 §3.2), so a sweep finds only the
/// leftovers of earlier runs of the same engine.
pub(super) fn engine_name(data_dir: &Path) -> String {
    let hash = Sha256::digest(data_dir.as_os_str().as_encoded_bytes());
    let hex: String = hash[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!("theuth:{hex}")
}

/// The namespace is a real agent-desktop session whose id is the theuth
/// session id and whose name is `engine`, trace off, so `end_session` and
/// `gc` can reclaim it (161 W-a). `start_session` would mint its own id. An
/// ended one is reopened.
fn open_namespace(id: &str, engine: &str) -> Result<(), AppError> {
    if session::read_manifest(id)?.is_some_and(|m| m.ended_at.is_none()) {
        return Ok(());
    }
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    session::write_manifest(&SessionManifest {
        id: id.to_owned(),
        name: Some(engine.to_owned()),
        created_at,
        ended_at: None,
        trace: SessionTraceMode::Off,
        artifacts: ArtifactsMode::Events,
        cursor_overlay: CursorOverlayConfig::default(),
    })
}

/// What a startup sweep does with the store's sessions.
#[derive(Debug, PartialEq, Eq)]
struct Leftovers<'a> {
    /// This engine's sessions still active: a dead run's namespaces.
    end: Vec<&'a str>,
    /// Whether any session carries this engine's name, so `gc` has work.
    collect: bool,
}

/// Only sessions named exactly `engine` are this engine's; every other
/// session in the store, theuth's or not, is left alone.
fn leftovers<'a>(sessions: &'a [SessionManifest], engine: &str) -> Leftovers<'a> {
    let ours = || {
        sessions
            .iter()
            .filter(|m| m.name.as_deref() == Some(engine))
    };
    Leftovers {
        end: ours()
            .filter(|m| m.ended_at.is_none())
            .map(|m| m.id.as_str())
            .collect(),
        collect: ours().next().is_some(),
    }
}

/// Reclaim the namespaces an earlier run of this engine left behind (its
/// threads died with the process, 161 §3.2): end them, then `gc` ended
/// sessions, all through agent-desktop's session API. Run at plugin start,
/// before any worker exists; blocks on the store.
pub(super) fn sweep(engine: &str) {
    match end_leftovers(engine) {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            engine,
            ended = n,
            "computer: reclaimed leftover a11y namespaces"
        ),
        Err(error) => tracing::warn!(engine, %error, "computer: a11y namespace sweep failed"),
    }
}

/// [`sweep`]'s body; the number of namespaces ended.
fn end_leftovers(engine: &str) -> Result<usize, AppError> {
    let sessions = session::list_sessions()?;
    let found = leftovers(&sessions, engine);
    for id in &found.end {
        session::end_session(id)?;
    }
    if found.collect {
        session::gc(GcOptions {
            ended_only: true,
            older_than: None,
        })?;
    }
    Ok(found.end.len())
}

/// The adapter the way agent-desktop's FFI builds it
/// (AD:`crates/ffi/src/adapter.rs:108-123`).
#[cfg(target_os = "macos")]
fn adapter() -> Result<Box<dyn PlatformAdapter>, AppError> {
    agent_desktop_macos::ensure_cocoa_multithreaded()?;
    Ok(Box::new(agent_desktop_macos::MacOSAdapter::new()))
}

/// The adapter the way agent-desktop's FFI builds it
/// (AD:`crates/ffi/src/adapter.rs:108-123`). The plugin made the process
/// per-monitor-v2 DPI-aware at start, so this bootstrap's DPI step finds
/// awareness already set and changes nothing mid-session (161 §2).
#[cfg(target_os = "windows")]
fn adapter() -> Result<Box<dyn PlatformAdapter>, AppError> {
    agent_desktop_windows::bootstrap_hosted_library()?;
    Ok(Box::new(agent_desktop_windows::WindowsAdapter::new()))
}

/// The adapter the way agent-desktop's FFI builds it
/// (AD:`crates/ffi/src/adapter.rs:108-123`).
#[cfg(target_os = "linux")]
fn adapter() -> Result<Box<dyn PlatformAdapter>, AppError> {
    Ok(Box::new(agent_desktop_linux::LinuxAdapter::new()))
}

/// [`Backend::REAL`]'s reclaim: end the namespace's session, then collect ended
/// sessions through agent-desktop's own API (161 W-b). `gc` has no
/// per-session form, so other already-ended sessions go too.
fn reclaim(namespace: &str) {
    if let Err(error) = end_and_collect(namespace) {
        tracing::warn!(namespace, %error, "computer: a11y namespace not reclaimed");
    }
}

fn end_and_collect(namespace: &str) -> Result<(), AppError> {
    if session::read_manifest(namespace)?.is_none() {
        return Ok(());
    }
    session::end_session(namespace)?;
    session::gc(GcOptions {
        ended_only: true,
        older_than: None,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, name: Option<&str>, ended: bool) -> SessionManifest {
        SessionManifest {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            created_at: 1,
            ended_at: ended.then_some(2),
            trace: SessionTraceMode::Off,
            artifacts: ArtifactsMode::Events,
            cursor_overlay: CursorOverlayConfig::default(),
        }
    }

    #[test]
    fn engine_name_is_stable_per_data_dir() {
        let a = engine_name(Path::new("/data/theuth"));
        assert_eq!(a, engine_name(Path::new("/data/theuth")));
        assert_ne!(a, engine_name(Path::new("/tmp/scratch-engine")));
        assert!(
            a.starts_with("theuth:") && a.len() == "theuth:".len() + 12,
            "{a}"
        );
    }

    #[test]
    fn sweep_ends_only_this_engines_active_sessions() {
        let ours = engine_name(Path::new("/data/theuth"));
        let other = engine_name(Path::new("/tmp/scratch-engine"));
        let sessions = [
            manifest("mine-live", Some(&ours), false),
            manifest("mine-ended", Some(&ours), true),
            manifest("scratch-live", Some(&other), false),
            manifest("cli-run", None, false),
            manifest("named-run", Some("theuth"), false),
        ];
        assert_eq!(
            leftovers(&sessions, &ours),
            Leftovers {
                end: vec!["mine-live"],
                collect: true
            }
        );
    }

    #[test]
    fn an_already_ended_leftover_still_needs_collecting() {
        let ours = engine_name(Path::new("/data/theuth"));
        let sessions = [manifest("mine-ended", Some(&ours), true)];
        assert_eq!(
            leftovers(&sessions, &ours),
            Leftovers {
                end: Vec::new(),
                collect: true
            }
        );
    }

    #[test]
    fn a_store_without_this_engine_is_left_alone() {
        let ours = engine_name(Path::new("/data/theuth"));
        let sessions = [
            manifest("scratch-live", Some(&engine_name(Path::new("/x"))), false),
            manifest("cli-ended", None, true),
        ];
        assert_eq!(
            leftovers(&sessions, &ours),
            Leftovers {
                end: Vec::new(),
                collect: false
            }
        );
    }
}
