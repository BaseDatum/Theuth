//! One process-wide watchdog thread bounds every armed request with a deadline and stops an overrunning
//! isolate through its thread-safe `IsolateHandle::terminate_execution` (obscura-js `cdp_watchdog.rs`'s
//! pattern). Arm/disarm are a mutex plus a condvar notify; no thread per request.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use deno_core::v8::IsolateHandle;

struct Slot {
    deadline: Instant,
    handle: IsolateHandle,
    fired: Arc<AtomicBool>,
}

#[derive(Default)]
struct Slots {
    armed: HashMap<u64, Slot>,
    next: u64,
}

struct Shared {
    state: Mutex<Slots>,
    cv: Condvar,
}

static SHARED: LazyLock<Arc<Shared>> = LazyLock::new(|| {
    let s = Arc::new(Shared {
        state: Mutex::new(Slots::default()),
        cv: Condvar::new(),
    });
    let worker = s.clone();
    std::thread::Builder::new()
        .name("app-js-watchdog".into())
        .spawn(move || watch(&worker))
        .expect("spawn app-js watchdog");
    s
});

fn shared() -> &'static Shared {
    &SHARED
}

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, Slots> {
    s.state.lock().unwrap_or_else(|e| e.into_inner())
}

fn watch(s: &Shared) {
    let mut guard = lock(s);
    loop {
        let now = Instant::now();
        let expired: Vec<u64> = guard
            .armed
            .iter()
            .filter(|(_, slot)| slot.deadline <= now)
            .map(|(k, _)| *k)
            .collect();
        for k in expired {
            if let Some(slot) = guard.armed.remove(&k) {
                slot.fired.store(true, Ordering::SeqCst);
                slot.handle.terminate_execution();
            }
        }
        let next = guard
            .armed
            .values()
            .map(|slot| slot.deadline.saturating_duration_since(now))
            .min();
        guard = match next {
            None => s.cv.wait(guard).unwrap_or_else(|e| e.into_inner()),
            Some(d) => {
                s.cv.wait_timeout(guard, d)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
        };
    }
}

/// An armed deadline; pass to [`disarm`].
pub(crate) struct Armed {
    key: u64,
    fired: Arc<AtomicBool>,
}

/// Terminate `handle`'s execution if it is still armed `budget` from now.
pub(crate) fn arm(handle: IsolateHandle, budget: Duration) -> Armed {
    let s = shared();
    let mut guard = lock(s);
    guard.next += 1;
    let key = guard.next;
    let fired = Arc::new(AtomicBool::new(false));
    guard.armed.insert(
        key,
        Slot {
            deadline: Instant::now() + budget,
            handle,
            fired: fired.clone(),
        },
    );
    s.cv.notify_one();
    Armed { key, fired }
}

/// Disarm; true if the deadline already fired (the isolate was terminated).
pub(crate) fn disarm(armed: Armed) -> bool {
    let s = shared();
    let mut guard = lock(s);
    guard.armed.remove(&armed.key);
    s.cv.notify_one();
    armed.fired.load(Ordering::SeqCst)
}
