//! W1 MEASURE — the machine bench slot and the load precondition (design doc
//! 140 §5.5): `bench.lock = builtin` is a mkdir slot under
//! `/tmp/theuth-bench-slots/0` with an owner file `pid label epoch argv` (the
//! build-lock layout); any other value is a wrapper command the sample runs
//! through. Load = 1-minute average ÷ cores, waited for up to `bench.wait`,
//! then the sample runs flagged `noisy`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

/// The builtin slot directory (one slot: a benchmark is exclusive).
pub const SLOT_DIR: &str = "/tmp/theuth-bench-slots";
/// `~/.theuth/bin/build-lock`'s slots, read for the sample row.
pub const BUILD_SLOT_DIR: &str = "/tmp/theuth-build-slots";
/// `bench.lock` value selecting the mkdir slot.
pub const BUILTIN: &str = "builtin";
/// Load poll spacing (140 §5.5).
const LOAD_POLL: Duration = Duration::from_secs(15);

/// A held bench slot; released on drop.
#[derive(Debug)]
pub struct Slot {
    _label: String,
    /// `None` = a wrapper lock (the runner prefixes the command) or `exclusive: false`.
    dir: Option<PathBuf>,
}

impl Slot {
    /// Did this take the builtin slot?
    pub fn held(&self) -> bool {
        self.dir.is_some()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Take the builtin slot for `label`, waiting until it is free or `cancel` fires.
pub async fn acquire(cfg: &crate::config::Bench, label: &str, cancel: &CancellationToken) -> Result<Slot> {
    if cfg.lock != BUILTIN {
        return Ok(Slot {
            _label: label.to_owned(),
            dir: None,
        });
    }
    let dir = Path::new(SLOT_DIR).join("0");
    acquire_at(&dir, label, cancel).await
}

/// [`acquire`] on an explicit slot directory (tests).
async fn acquire_at(dir: &Path, label: &str, cancel: &CancellationToken) -> Result<Slot> {
    let parent = dir.parent().expect("slot dir has a parent");
    let mut attempt = 0u64;
    loop {
        std::fs::create_dir_all(parent)?;
        match std::fs::create_dir(dir) {
            Ok(()) => {
                let epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                std::fs::write(
                    dir.join("owner"),
                    format!("{} {label} {epoch} bench\n", std::process::id()),
                )?;
                return Ok(Slot {
                    _label: label.to_owned(),
                    dir: Some(dir.to_path_buf()),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if owner_pid(dir).is_some_and(|pid| pid != std::process::id() && !platform::process::pid_alive(pid)) {
            // Dead owner: reclaim (build-lock's rule; a dir without an owner file is a taker mid-write).
            let _ = std::fs::remove_dir_all(dir);
            continue;
        }
        // Polite 5–10 s poll, like build-lock.
        let nap = Duration::from_secs(5 + attempt % 6);
        attempt += 1;
        tokio::select! {
            () = cancel.cancelled() => return Err(Error::refused("cancelled while waiting for the bench slot")),
            () = tokio::time::sleep(nap) => {}
        }
    }
}

/// The pid in `<dir>/owner`.
fn owner_pid(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("owner"))
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// 1-minute load average ÷ cores now.
pub fn load() -> Option<f64> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    platform::power::load1().map(|l| l / cores)
}

/// Wait until `load() <= max_load` or `wait` elapses; `Ok(true)` = the
/// precondition held, `Ok(false)` = ran out of patience (sample is `noisy`).
pub async fn wait_for_load(max_load: f64, wait: Duration, cancel: &CancellationToken) -> Result<bool> {
    let start = Instant::now();
    loop {
        if load().is_none_or(|l| l <= max_load) {
            return Ok(true);
        }
        let left = wait.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Ok(false);
        }
        tokio::select! {
            () = cancel.cancelled() => return Err(Error::refused("cancelled while waiting for the load precondition")),
            () = tokio::time::sleep(left.min(LOAD_POLL)) => {}
        }
    }
}

/// The load wait left for experiment `n` of `slug` out of `wait` total, and a
/// closure-free way to charge it: call [`charge_wait`] with what was actually spent.
/// `bench.wait` is a total per experiment (140 §5.5); the runner has no experiment
/// object, so the total lives here keyed by `(slug, n)`, one entry per study.
pub fn wait_budget(slug: &str, n: u32, wait: Duration) -> Duration {
    let ledger = WAITED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match ledger.iter().find(|(s, k, _)| s == slug && *k == n) {
        Some((_, _, spent)) => wait.saturating_sub(*spent),
        None => wait,
    }
}

/// Record `spent` waiting for the load precondition in experiment `n` of `slug`.
pub fn charge_wait(slug: &str, n: u32, spent: Duration) {
    let mut ledger = WAITED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    ledger.retain(|(s, k, _)| s != slug || *k == n); // only the current experiment per study
    match ledger.iter_mut().find(|(s, k, _)| s == slug && *k == n) {
        Some((_, _, total)) => *total += spent,
        None => ledger.push((slug.to_owned(), n, spent)),
    }
}

// ponytail: process-wide (slug, n, waited) list; a field on the experiment when W3 has one.
static WAITED: Mutex<Vec<(String, u32, Duration)>> = Mutex::new(Vec::new());

/// Who holds the build slots right now (`/tmp/theuth-build-slots/*/owner`), for the sample row.
pub fn build_slot_holders() -> Vec<String> {
    holders_in(Path::new(BUILD_SLOT_DIR))
}

fn holders_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("owner")).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn builtin_slot_is_exclusive_and_released_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("slots/0");
        let cancel = CancellationToken::new();
        let held = acquire_at(&dir, "bench:a", &cancel).await.unwrap();
        assert!(held.held());
        let owner = std::fs::read_to_string(dir.join("owner")).unwrap();
        assert!(owner.starts_with(&format!("{} bench:a ", std::process::id())), "{owner}");
        assert_eq!(holders_in(tmp.path().join("slots").as_path()).len(), 1);

        // A second taker waits; cancelling it returns Refused without touching the slot.
        let c2 = CancellationToken::new();
        c2.cancel();
        let second = acquire_at(&dir, "bench:b", &c2).await;
        assert!(matches!(second, Err(Error::Refused(_))));
        assert!(dir.join("owner").is_file());

        drop(held);
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn dead_owner_is_reclaimed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("slots/0");
        std::fs::create_dir_all(&dir).unwrap();
        // pid 4_000_000 does not exist on macOS or Linux.
        std::fs::write(dir.join("owner"), "4000000 stale 0 bench\n").unwrap();
        let cancel = CancellationToken::new();
        let held = acquire_at(&dir, "bench:c", &cancel).await.unwrap();
        assert!(held.held());
        assert!(std::fs::read_to_string(dir.join("owner")).unwrap().contains("bench:c"));
    }

    #[tokio::test]
    async fn wrapper_lock_takes_nothing() {
        let cfg = crate::config::Bench {
            lock: "/usr/bin/env".into(),
            max_load: 0.5,
            wait: "1s".into(),
        };
        let s = acquire(&cfg, "x", &CancellationToken::new()).await.unwrap();
        assert!(!s.held());
    }

    #[tokio::test]
    async fn load_wait_gives_up_after_budget() {
        // max_load below any real load → patience runs out at `wait`.
        let cancel = CancellationToken::new();
        let t = Instant::now();
        let ok = wait_for_load(-1.0, Duration::from_millis(50), &cancel).await.unwrap();
        assert!(!ok || load().is_none());
        assert!(t.elapsed() < Duration::from_secs(5));
        // An infinite threshold passes at once.
        assert!(wait_for_load(f64::INFINITY, Duration::ZERO, &cancel).await.unwrap());
    }

    #[test]
    fn wait_budget_is_per_experiment() {
        let w = Duration::from_secs(100);
        assert_eq!(wait_budget("s1", 7, w), w);
        charge_wait("s1", 7, Duration::from_secs(30));
        charge_wait("s1", 7, Duration::from_secs(30));
        assert_eq!(wait_budget("s1", 7, w), Duration::from_secs(40));
        charge_wait("s1", 8, Duration::from_secs(1));
        assert_eq!(wait_budget("s1", 7, w), w);
        assert_eq!(wait_budget("s1", 8, w), Duration::from_secs(99));
    }
}
