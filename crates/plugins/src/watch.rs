//! Owner: B1 — recursive `notify` watcher on `plugins/`, 300 ms debounce; tells the actor which
//! plugin folders changed (the actor re-validates, detects widening and code changes).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kameo::actor::ActorRef;
use notify::Watcher as _;

use crate::actor::{FolderChanged, PluginsActor};

/// Same debounce as the MCP loader (`crates/tools/src/mcp.rs`).
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(300);

/// Folders inside a plugin whose changes are build output, caches or VCS state, never a reason to
/// re-validate or restart (a `cargo run` plugin writes `target/` on every start).
const IGNORED: &[&str] = &["target", "__pycache__", "node_modules", ".git"];

/// The plugin folder a changed path belongs to: the first component under `root`, skipping
/// host-owned entries (`grants.yaml`, dot names such as `.pgids`, `.trash`), temp files and
/// paths under an [`IGNORED`] folder.
pub(crate) fn plugin_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = rel.components().map(|c| c.as_os_str().to_str());
    let first = parts.next()??;
    if first.starts_with('.') || first.starts_with(crate::grants::FILE) {
        return None;
    }
    if parts.any(|p| p.is_some_and(|p| IGNORED.contains(&p))) {
        return None;
    }
    Some(first.to_owned())
}

/// Watch `root` until `rt.shutdown`; each debounced burst becomes one [`FolderChanged`].
pub(crate) async fn run(rt: runtime::Runtime, root: PathBuf, actor: ActorRef<PluginsActor>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "plugins: no folder watcher; edits need a restart");
            return;
        }
    };
    if let Err(e) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
        tracing::warn!(error = %e, dir = %root.display(), "plugins: cannot watch the plugins folder");
        return;
    }
    // The canonical root (macOS reports /private/var/... for /var/...).
    let canon = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    loop {
        let first = tokio::select! {
            _ = rt.shutdown.cancelled() => return,
            p = rx.recv() => match p { Some(p) => p, None => return },
        };
        tokio::time::sleep(DEBOUNCE).await;
        let mut names = BTreeSet::new();
        let mut add = |p: &Path| {
            if let Some(n) = plugin_of(&root, p).or_else(|| plugin_of(&canon, p)) {
                names.insert(n);
            }
        };
        add(&first);
        while let Ok(p) = rx.try_recv() {
            add(&p);
        }
        if names.is_empty() {
            continue;
        }
        if actor.tell(FolderChanged(names)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_files_are_not_plugins() {
        let root = Path::new("/d/plugins");
        assert_eq!(
            plugin_of(root, Path::new("/d/plugins/turns/main.py")).as_deref(),
            Some("turns")
        );
        assert_eq!(
            plugin_of(root, Path::new("/d/plugins/turns/lib/a.py")).as_deref(),
            Some("turns")
        );
        assert_eq!(plugin_of(root, Path::new("/d/plugins/grants.yaml")), None);
        assert_eq!(plugin_of(root, Path::new("/d/plugins/.pgids")), None);
        assert_eq!(
            plugin_of(root, Path::new("/d/plugins/.trash/x-1/plugin.yaml")),
            None
        );
        assert_eq!(
            plugin_of(root, Path::new("/d/plugins/turns")).as_deref(),
            Some("turns")
        );
        assert_eq!(plugin_of(root, Path::new("/elsewhere/x/y")), None);
        for ignored in [
            "/d/plugins/rs/target/debug/rs",
            "/d/plugins/py/__pycache__/main.cpython-312.pyc",
            "/d/plugins/js/node_modules/x/index.js",
            "/d/plugins/js/.git/index",
            "/d/plugins/py/lib/__pycache__/a.pyc",
        ] {
            assert_eq!(plugin_of(root, Path::new(ignored)), None, "{ignored}");
        }
        assert_eq!(
            plugin_of(root, Path::new("/d/plugins/rs/src/target.rs")).as_deref(),
            Some("rs")
        );
    }
}
