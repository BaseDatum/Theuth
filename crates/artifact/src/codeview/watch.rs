//! Live follow for `show_file` tiles (design 212 §6): one watcher per path shared by every
//! `subscribe_file_view` subscriber (refcounted, dropped with the last one). The watcher is
//! `notify-debouncer-full` on the file's parent directory, non-recursive, filtered to the file
//! name (editors save by writing a temp file and renaming it over), 150 ms debounce; a settled
//! change whose `(mtime, size, deleted)` differs from the last one sent becomes one
//! [`FileViewChange`]. Without a watcher (the platform refused one) the path is polled every 2 s.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::codeview::{FileViewChange, mtime_ms};

/// How long a burst of events settles before one change is reported.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// The poll interval when no watcher could be created.
pub const POLL: Duration = Duration::from_secs(2);

/// Changes a slow subscriber may fall behind before it skips to newer ones.
const BACKLOG: usize = 16;

/// What keeps one path watched (dropping it stops the watch).
enum Source {
    Notify(
        #[expect(dead_code, reason = "held only: dropping the debouncer stops the watcher")]
        Debouncer<RecommendedWatcher, RecommendedCache>,
    ),
    Poll(tokio::task::JoinHandle<()>),
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Source::Poll(task) = self {
            task.abort();
        }
    }
}

struct Entry {
    tx: broadcast::Sender<FileViewChange>,
    subs: usize,
    _source: Source,
}

static WATCHES: LazyLock<Mutex<HashMap<PathBuf, Entry>>> = LazyLock::new(Default::default);

/// One subscriber's view of a watched path; dropping it releases the watcher.
pub struct FileWatch {
    path: PathBuf,
    rx: broadcast::Receiver<FileViewChange>,
}

impl FileWatch {
    /// The next settled change; `None` once the watcher is gone. A subscriber that fell behind
    /// skips to the newest changes (each item is the file's whole state, so none is lost).
    pub async fn next(&mut self) -> Option<FileViewChange> {
        loop {
            match self.rx.recv().await {
                Ok(c) => return Some(c),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for FileWatch {
    fn drop(&mut self) {
        let gone = {
            let mut all = WATCHES.lock();
            match all.get_mut(&self.path) {
                Some(e) if e.subs > 1 => {
                    e.subs -= 1;
                    None
                }
                Some(_) => all.remove(&self.path),
                None => None,
            }
        };
        // The debouncer stops outside the lock.
        drop(gone);
    }
}

/// Watch `path` (absolute; a canonical path matches the platform's event paths). Must run inside
/// a Tokio runtime (the poll fallback is a task).
pub fn watch(path: &Path) -> FileWatch {
    let mut all = WATCHES.lock();
    if let Some(e) = all.get_mut(path) {
        e.subs += 1;
        return FileWatch {
            path: path.to_path_buf(),
            rx: e.tx.subscribe(),
        };
    }
    let (tx, rx) = broadcast::channel(BACKLOG);
    let source = match start_notify(path, tx.clone()) {
        Ok(d) => Source::Notify(d),
        Err(e) => {
            tracing::debug!(path = %path.display(), "file view watcher unavailable, polling: {e}");
            Source::Poll(tokio::spawn(poll(path.to_path_buf(), tx.clone())))
        }
    };
    all.insert(
        path.to_path_buf(),
        Entry {
            tx,
            subs: 1,
            _source: source,
        },
    );
    FileWatch {
        path: path.to_path_buf(),
        rx,
    }
}

/// Subscribers of `path` (0 when not watched).
pub fn subscribers(path: &Path) -> usize {
    WATCHES.lock().get(path).map_or(0, |e| e.subs)
}

/// The file's state as a change item.
fn state(path: &Path) -> FileViewChange {
    match std::fs::metadata(path) {
        Ok(m) => FileViewChange {
            mtime_ms: mtime_ms(&m),
            size: m.len(),
            deleted: false,
        },
        Err(_) => FileViewChange {
            deleted: true,
            ..FileViewChange::default()
        },
    }
}

fn start_notify(
    path: &Path,
    tx: broadcast::Sender<FileViewChange>,
) -> anyhow::Result<Debouncer<RecommendedWatcher, RecommendedCache>> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        anyhow::bail!("{} has no parent directory", path.display());
    };
    let name: OsString = name.to_owned();
    let target = path.to_path_buf();
    let mut last = state(path);
    let mut d = new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let ours = events
            .iter()
            .any(|e| e.paths.iter().any(|p| p.file_name() == Some(name.as_os_str())));
        if !ours {
            return;
        }
        let now = state(&target);
        if now != last {
            last = now;
            let _ = tx.send(now);
        }
    })?;
    d.watch(parent, RecursiveMode::NonRecursive)?;
    Ok(d)
}

async fn poll(path: PathBuf, tx: broadcast::Sender<FileViewChange>) {
    let mut last = state(&path);
    loop {
        tokio::time::sleep(POLL).await;
        let now = state(&path);
        if now != last {
            last = now;
            // No receiver: the entry is being dropped and aborts this task.
            let _ = tx.send(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn dir() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = std::fs::canonicalize(d.path()).unwrap().join("f.rs");
        std::fs::write(&p, "one\n").unwrap();
        (d, p)
    }

    async fn next(w: &mut FileWatch) -> FileViewChange {
        tokio::time::timeout(WAIT, w.next())
            .await
            .expect("a change within 10 s")
            .expect("watcher alive")
    }

    /// No item arrives within `d`.
    async fn quiet(w: &mut FileWatch, d: Duration) {
        if let Ok(c) = tokio::time::timeout(d, w.next()).await {
            panic!("unexpected change {c:?}");
        }
    }

    /// FSEvents may deliver the creation that preceded the watch; let it settle.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    #[tokio::test]
    async fn fires_on_write_and_on_rename_replace() {
        let (d, p) = dir();
        let mut w = watch(&p);
        settle().await;
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let c = next(&mut w).await;
        assert_eq!((c.size, c.deleted), (8, false));
        // An editor's save: write a temp file beside it, rename over.
        let tmp = d.path().join(".f.rs.swp");
        std::fs::write(&tmp, "replaced!!\n").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        let c = next(&mut w).await;
        assert_eq!((c.size, c.deleted), (11, false));
        // Other files in the directory are not this file.
        std::fs::write(d.path().join("other.rs"), "x").unwrap();
        quiet(&mut w, Duration::from_millis(600)).await;
    }

    #[tokio::test]
    async fn deleted_then_back() {
        let (_d, p) = dir();
        let mut w = watch(&p);
        settle().await;
        std::fs::remove_file(&p).unwrap();
        assert_eq!(next(&mut w).await, FileViewChange { mtime_ms: 0, size: 0, deleted: true });
        std::fs::write(&p, "back\n").unwrap();
        let c = next(&mut w).await;
        assert_eq!((c.size, c.deleted), (5, false));
        assert!(c.mtime_ms > 0);
    }

    #[tokio::test]
    async fn a_burst_coalesces() {
        let (_d, p) = dir();
        let mut w = watch(&p);
        settle().await;
        for i in 0..10 {
            std::fs::write(&p, "x".repeat(i + 10)).unwrap();
        }
        let c = next(&mut w).await;
        assert_eq!(c.size, 19, "the settled state");
        quiet(&mut w, Duration::from_millis(600)).await;
    }

    #[tokio::test]
    async fn shared_and_released() {
        let (_d, p) = dir();
        let mut a = watch(&p);
        let mut b = watch(&p);
        assert_eq!(subscribers(&p), 2);
        settle().await;
        std::fs::write(&p, "shared\n").unwrap();
        assert_eq!(next(&mut a).await.size, 7);
        assert_eq!(next(&mut b).await.size, 7);
        drop(a);
        assert_eq!(subscribers(&p), 1);
        drop(b);
        assert_eq!(subscribers(&p), 0);
        assert!(!WATCHES.lock().contains_key(&p));
    }
}
