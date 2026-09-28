//! Scratch directory of a private root tree (design doc 51 §4.5): a
//! mode-0700 random directory under the OS temp location, held under an
//! OS-backed exclusive lock for its lifetime. Cleanup is honest best effort:
//! a crash may leave bytes behind, and [`sweep_stale`] removes them only
//! after taking the lock that proves the owner is gone — never on age or PID.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// Names of every scratch directory this process (or a dead one) made.
const PREFIX: &str = "theuth-private-";

/// A live scratch root. Dropping it releases the lock and leaves the
/// directory for [`sweep_stale`]; [`Scratch::remove`] deletes it now.
#[derive(Debug)]
pub struct Scratch {
    /// The mode-0700 scratch directory owners write under.
    pub root: PathBuf,
    /// `<root>/.lock`, exclusively locked; the field order drops it last.
    lock: File,
}

impl Scratch {
    /// `temp_dir()/theuth-private-<128 random bits>`, created 0700 and locked.
    pub fn create() -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
        platform::files::private_dir(&root)?;
        let lock = lock_file(&root)?;
        Ok(Self { root, lock })
    }

    /// Release the lock and delete the directory. Refuses a root that has
    /// become a symlink (nothing outside it is ever followed).
    pub fn remove(self) -> io::Result<()> {
        let Self { root, lock } = self;
        drop(lock);
        remove_root(&root)
    }
}

/// Open (creating) `<root>/.lock` and take the exclusive lock without
/// blocking; `WouldBlock` when another holder is alive.
fn lock_file(root: &Path) -> io::Result<File> {
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".lock"))?;
    f.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
        std::fs::TryLockError::Error(e) => e,
    })?;
    Ok(f)
}

fn remove_root(root: &Path) -> io::Result<()> {
    if fs::symlink_metadata(root)?.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "scratch root is a symlink",
        ));
    }
    fs::remove_dir_all(root)
}

/// Remove every `theuth-private-*` directory in the temp dir whose lock can be
/// taken (its owner is dead); return how many went. Symlinks, live roots
/// and anything whose lock cannot be proven free are left alone.
pub fn sweep_stale() -> usize {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let root = entry.path();
        if !fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let Ok(lock) = lock_file(&root) else {
            continue;
        };
        // Held while removing so a concurrent sweeper skips it; the unlink
        // of `.lock` itself is fine under the lock.
        let gone = remove_root(&root).is_ok();
        drop(lock);
        if gone {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_is_private_and_locked_until_removed() {
        let s = Scratch::create().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&s.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        // A second holder cannot take the lock while `s` lives…
        assert_eq!(
            lock_file(&s.root).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        // …so a sweep leaves it alone (other stale roots on this machine may go).
        sweep_stale();
        assert!(s.root.is_dir());

        // Released without removal: the next sweep proves the owner dead and
        // removes it.
        let root = s.root.clone();
        drop(s);
        assert!(root.is_dir());
        sweep_stale();
        assert!(!root.exists());

        let s = Scratch::create().unwrap();
        let root = s.root.clone();
        s.remove().unwrap();
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn open_memory_is_migrated_and_disk_free() {
        use crate::store::Store;
        use runtime::SessionId;
        use sqlx::Row;
        let store = Store::open_memory().await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let ids: Vec<_> = store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![id]);
        let temp_store: i64 = sqlx::query("PRAGMA temp_store")
            .fetch_one(store.pool())
            .await
            .unwrap()
            .get(0);
        assert_eq!(temp_store, 2, "temp_store=MEMORY");
        let journal: String = sqlx::query("PRAGMA journal_mode")
            .fetch_one(store.pool())
            .await
            .unwrap()
            .get(0);
        assert_eq!(journal, "memory");
    }
}
