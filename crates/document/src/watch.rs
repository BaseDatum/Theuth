//! Derived-file watcher (design 59 §4.4): `notify` on `<data_dir>/knowledge/`,
//! 300 ms debounce per path, then [`FileChanged`] to the actor, which skips
//! its own exports by sha and imports everything else as `Principal::File`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use notify::Watcher;
use tokio::sync::mpsc;

use crate::actor::{DocumentActor, FileChanged};
use crate::meta::Binding;

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Watch `<data_dir>/knowledge` recursively for the process lifetime.
pub fn start(data_dir: &Path, actor: ActorRef<DocumentActor>) -> anyhow::Result<()> {
    let root = data_dir.join("knowledge");
    std::fs::create_dir_all(&root)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })?;
    watcher.watch(&root, notify::RecursiveMode::Recursive)?;
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut due: HashMap<PathBuf, Instant> = HashMap::new();
        loop {
            let next = due.values().min().copied();
            tokio::select! {
                p = rx.recv() => {
                    let Some(p) = p else { break };
                    if binding_of(&root, &p).is_some() {
                        due.insert(p, Instant::now() + DEBOUNCE);
                    }
                }
                _ = async {
                    match next {
                        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                        None => std::future::pending().await,
                    }
                } => {
                    let now = Instant::now();
                    let ready: Vec<PathBuf> = due.iter().filter(|(_, t)| **t <= now).map(|(p, _)| p.clone()).collect();
                    for p in ready {
                        due.remove(&p);
                        if let Some(binding) = binding_of(&root, &p) {
                            let _ = actor.tell(FileChanged { binding, path: p }).await;
                        }
                    }
                }
            }
        }
    });
    Ok(())
}

/// `knowledge/<id>.md` → `Binding::Knowledge(id)`; `.git`, `index.md`, `log.md` and non-markdown are not documents.
pub fn binding_of(root: &Path, path: &Path) -> Option<Binding> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_str()?;
    if s.starts_with(".git") || s.contains("/.") {
        return None;
    }
    let id = s.strip_suffix(".md")?;
    let last = id.rsplit('/').next().unwrap_or(id);
    if id.is_empty() || last == "index" || last == "log" || id.contains(".md.tmp") {
        return None;
    }
    Some(Binding::Knowledge(id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_concept_files_only() {
        let root = Path::new("/d/knowledge");
        assert_eq!(
            binding_of(root, Path::new("/d/knowledge/facts/foo.md")),
            Some(Binding::Knowledge("facts/foo".into()))
        );
        assert_eq!(binding_of(root, Path::new("/d/knowledge/index.md")), None);
        assert_eq!(
            binding_of(root, Path::new("/d/knowledge/facts/index.md")),
            None
        );
        assert_eq!(binding_of(root, Path::new("/d/knowledge/log.md")), None);
        assert_eq!(binding_of(root, Path::new("/d/knowledge/.git/HEAD")), None);
        assert_eq!(
            binding_of(root, Path::new("/d/knowledge/facts/foo.md.tmp")),
            None
        );
        assert_eq!(binding_of(root, Path::new("/d/elsewhere/foo.md")), None);
    }
}
