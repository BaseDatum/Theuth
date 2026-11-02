//! Worker threads that own non-`Send` state, one per key (design doc 161 §3.1).
//! The input thread's shape (27 §3.4) made generic: one named `std::thread`,
//! jobs in over a channel, results out on a tokio oneshot, `catch_unwind` per
//! job, a deadline on the async side. A worker is spawned by the first job for
//! its key, retired on demand or when idle, and abandoned when a job misses
//! its deadline. An abandoned worker never runs its close hook (161 §3.3).

use std::collections::HashMap;
use std::hash::Hash;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::input::panic_message;

/// One job's deadline on the async side, queue time included: the input
/// thread's (27 §3.4), the a11y workers' and the clipboard's (161 D3).
pub(crate) const DEADLINE: Duration = Duration::from_secs(30);

/// Why a job produced no result.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// No result within the deadline; the worker was abandoned.
    Timeout,
    /// The job panicked; the worker's state was reset to its default.
    Panicked(String),
    /// The thread could not be spawned, or went away without answering.
    Gone(String),
}

/// One job on the worker thread; `true` when it panicked.
type Job<S> = Box<dyn FnOnce(&mut S) -> bool + Send>;

/// What a worker receives.
enum Msg<S> {
    Run(Job<S>),
    Retire,
}

/// Runs with the final state when a worker retires (on its own thread), or
/// with the default state when a key with no live worker is retired.
type Close<K, S> = dyn Fn(&K, S) + Send + Sync;

/// A live worker. Only the slot holds the sender, so dropping the slot
/// disconnects the channel.
struct Slot<S> {
    generation: u64,
    tx: mpsc::Sender<Msg<S>>,
    thread: JoinHandle<()>,
}

struct Inner<K, S> {
    name: fn(&K) -> String,
    deadline: Duration,
    idle: Option<Duration>,
    close: Arc<Close<K, S>>,
    slots: Mutex<HashMap<K, Slot<S>>>,
    generations: AtomicU64,
}

impl<K: Eq + Hash, S> Inner<K, S> {
    fn slots(&self) -> MutexGuard<'_, HashMap<K, Slot<S>>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drop `key`'s slot if it still belongs to worker `generation`.
    fn release(&self, key: &K, generation: u64) -> bool {
        let mut slots = self.slots();
        if slots.get(key).is_some_and(|s| s.generation == generation) {
            slots.remove(key);
            return true;
        }
        false
    }
}

/// Worker threads keyed by `K`, each owning an `S` that starts as
/// `S::default()` (161 §3.1). Real setup belongs in the first job, so a
/// failed setup is that job's error and the next job tries again.
pub(crate) struct Workers<K, S> {
    inner: Arc<Inner<K, S>>,
}

impl<K, S> Workers<K, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    S: Default + 'static,
{
    /// A key's thread is named `name(key)`; a job waits at most `deadline`,
    /// queue time included; a worker with no job for `idle` retires itself;
    /// `close` receives the state of every retiring worker.
    pub(crate) fn new(
        name: fn(&K) -> String,
        deadline: Duration,
        idle: Option<Duration>,
        close: impl Fn(&K, S) + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                name,
                deadline,
                idle,
                close: Arc::new(close),
                slots: Mutex::new(HashMap::new()),
                generations: AtomicU64::new(0),
            }),
        }
    }

    /// Run `job` on `key`'s worker, spawning it first if there is none. On a
    /// missed deadline the worker is dropped from the registry and the next
    /// job for `key` starts a fresh one. A job whose caller is gone (timed
    /// out, or cancelled) by the time the worker reaches it is skipped, so it
    /// can never land late.
    pub(crate) async fn run<R: Send + 'static>(
        &self,
        key: &K,
        job: impl FnOnce(&mut S) -> R + Send + 'static,
    ) -> Result<R, Failure> {
        let (tx, rx) = oneshot::channel();
        let job: Job<S> = Box::new(move |state| {
            if tx.is_closed() {
                return false;
            }
            match catch_unwind(AssertUnwindSafe(|| job(state))) {
                Ok(r) => {
                    let _ = tx.send(Ok(r));
                    false
                }
                Err(payload) => {
                    let _ = tx.send(Err(panic_message(payload.as_ref())));
                    true
                }
            }
        });
        let generation = self.send(key, Msg::Run(job))?;
        match tokio::time::timeout(self.inner.deadline, rx).await {
            Ok(Ok(Ok(r))) => Ok(r),
            Ok(Ok(Err(panic))) => Err(Failure::Panicked(panic)),
            Ok(Err(_)) => Err(Failure::Gone(format!(
                "{} exited without answering",
                (self.inner.name)(key)
            ))),
            Err(_) => {
                if self.inner.release(key, generation) {
                    tracing::warn!(
                        worker = (self.inner.name)(key),
                        deadline_s = self.inner.deadline.as_secs(),
                        "computer: worker missed its deadline; abandoned"
                    );
                }
                Err(Failure::Timeout)
            }
        }
    }

    /// Retire `keys`. A live worker runs its close hook after the jobs
    /// already queued and exits; keys with no live worker are closed with
    /// the default state on one detached thread. Join the handles to wait
    /// for that; dropping them detaches.
    pub(crate) fn retire(&self, keys: &[K]) -> Vec<JoinHandle<()>> {
        let mut slots = self.inner.slots();
        let mut idle = Vec::new();
        let mut threads = Vec::new();
        for key in keys {
            match slots.remove(key) {
                Some(slot) => {
                    let _ = slot.tx.send(Msg::Retire);
                    threads.push(slot.thread);
                }
                None => idle.push(key.clone()),
            }
        }
        drop(slots);
        if let Some(first) = idle.first() {
            let close = Arc::clone(&self.inner.close);
            let name = format!("{}-close", (self.inner.name)(first));
            let spawned = thread::Builder::new().name(name).spawn(move || {
                for key in &idle {
                    close(key, S::default());
                }
            });
            match spawned {
                Ok(thread) => threads.push(thread),
                Err(e) => tracing::warn!(error = %e, "computer: could not spawn a close thread"),
            }
        }
        threads
    }

    /// Send under the registry lock, so a worker that retires itself (idle)
    /// can never miss a job; a dead worker is replaced. Returns the worker's
    /// generation.
    fn send(&self, key: &K, msg: Msg<S>) -> Result<u64, Failure> {
        let mut slots = self.inner.slots();
        let msg = match slots.get(key) {
            Some(slot) => match slot.tx.send(msg) {
                Ok(()) => return Ok(slot.generation),
                Err(mpsc::SendError(msg)) => msg,
            },
            None => msg,
        };
        let slot = self.spawn(key)?;
        let generation = slot.generation;
        slot.tx
            .send(msg)
            .map_err(|_| Failure::Gone(format!("{} exited at start", (self.inner.name)(key))))?;
        slots.insert(key.clone(), slot);
        Ok(generation)
    }

    fn spawn(&self, key: &K) -> Result<Slot<S>, Failure> {
        let (tx, rx) = mpsc::channel();
        let generation = self.inner.generations.fetch_add(1, Ordering::Relaxed);
        let registry = Arc::downgrade(&self.inner);
        let close = Arc::clone(&self.inner.close);
        let idle = self.inner.idle;
        let name = (self.inner.name)(key);
        let key = key.clone();
        let thread = thread::Builder::new()
            .name(name.clone())
            .spawn(move || serve(&key, generation, &rx, idle, &registry, &*close))
            .map_err(|e| Failure::Gone(format!("could not spawn {name}: {e}")))?;
        Ok(Slot {
            generation,
            tx,
            thread,
        })
    }

    /// Live workers.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.slots().len()
    }
}

/// The worker loop. A closed channel means abandoned: exit without closing.
fn serve<K: Eq + Hash, S: Default>(
    key: &K,
    generation: u64,
    rx: &mpsc::Receiver<Msg<S>>,
    idle: Option<Duration>,
    registry: &Weak<Inner<K, S>>,
    close: &Close<K, S>,
) {
    let mut state = S::default();
    loop {
        let msg = match idle {
            Some(idle) => rx.recv_timeout(idle),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Run(job)) => run_job(&mut state, job),
            Ok(Msg::Retire) => break,
            Err(RecvTimeoutError::Timeout) => {
                let Some(inner) = registry.upgrade() else {
                    return;
                };
                if !inner.release(key, generation) {
                    // Retired or abandoned meanwhile: the queued `Retire`, or
                    // the closed channel, decides on the next receive.
                    continue;
                }
                // Jobs sent before the release still run; none can follow it.
                while let Ok(msg) = rx.try_recv() {
                    if let Msg::Run(job) = msg {
                        run_job(&mut state, job);
                    }
                }
                break;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
    close(key, state);
}

/// One job; a panicked job leaves the state reset to its default.
fn run_job<S: Default>(state: &mut S, job: Job<S>) {
    if job(state) {
        *state = S::default();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;

    /// Keys whose close hook ran, with the state it got.
    type Closed = Arc<Mutex<Vec<(u8, u32)>>>;

    fn workers(deadline_ms: u64, idle_ms: Option<u64>) -> (Workers<u8, u32>, Closed) {
        let closed = Closed::default();
        let log = Arc::clone(&closed);
        let w = Workers::new(
            |k| format!("test-worker-{k}"),
            Duration::from_millis(deadline_ms),
            idle_ms.map(Duration::from_millis),
            move |k: &u8, s: u32| {
                log.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((*k, s))
            },
        );
        (w, closed)
    }

    fn closed(c: &Closed) -> Vec<(u8, u32)> {
        c.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn bump(s: &mut u32) -> u32 {
        *s += 1;
        *s
    }

    fn sleep_ms(ms: u64) -> impl FnOnce(&mut u32) -> thread::ThreadId {
        move |s| {
            thread::sleep(Duration::from_millis(ms));
            bump(s);
            thread::current().id()
        }
    }

    #[tokio::test]
    async fn state_persists_across_jobs_and_resets_after_a_panic() {
        let (w, _) = workers(5_000, None);
        assert_eq!(w.run(&1, bump).await, Ok(1));
        assert_eq!(w.run(&1, bump).await, Ok(2));
        assert_eq!(w.run(&2, bump).await, Ok(1), "one state per key");
        let panicked = w.run(&1, |_: &mut u32| panic!("boom")).await;
        assert_eq!(panicked, Err(Failure::Panicked("boom".to_owned())));
        assert_eq!(w.run(&1, bump).await, Ok(1), "state reset after the panic");
        assert_eq!(w.len(), 2);
    }

    #[tokio::test]
    async fn a_job_whose_caller_timed_out_never_runs() {
        let (w, _) = workers(100, None);
        let flipped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&flipped);
        let (a, b) = tokio::join!(
            w.run(&1, sleep_ms(400)),
            w.run(&1, move |_: &mut u32| flag.store(true, Ordering::SeqCst)),
        );
        assert_eq!(
            (a.err(), b.err()),
            (Some(Failure::Timeout), Some(Failure::Timeout))
        );
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            !flipped.load(Ordering::SeqCst),
            "the queued job landed late"
        );
    }

    #[tokio::test]
    async fn a_stale_timeout_never_drops_its_replacement() {
        let (w, _) = workers(200, None);
        let late = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            w.run(&1, bump).await // queued on the first worker; times out at ~300 ms
        };
        let replacement = async {
            tokio::time::sleep(Duration::from_millis(250)).await;
            w.run(&1, sleep_ms(0)).await // the first worker is abandoned at ~200 ms
        };
        let (first, late, replacement) =
            tokio::join!(w.run(&1, sleep_ms(1_000)), late, replacement);
        assert_eq!(
            (first.err(), late.err()),
            (Some(Failure::Timeout), Some(Failure::Timeout))
        );
        let replacement = replacement.unwrap();
        assert_eq!(w.len(), 1, "the late timeout kept the replacement");
        assert_eq!(w.run(&1, sleep_ms(0)).await, Ok(replacement), "same thread");
    }

    #[tokio::test]
    async fn retire_runs_after_the_queued_jobs_then_closes() {
        let (w, closed_log) = workers(5_000, None);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let queued = async {
            let a = w.run(&1, sleep_ms(100));
            let b = w.run(&1, move |s: &mut u32| {
                flag.store(true, Ordering::SeqCst);
                bump(s)
            });
            tokio::join!(a, b)
        };
        let retire = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            w.retire(&[1])
        };
        let ((_, b), handles) = tokio::join!(queued, retire);
        for h in handles {
            h.join().unwrap();
        }
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(b, Ok(2));
        assert_eq!(
            closed(&closed_log),
            vec![(1, 2)],
            "closed with the final state"
        );
        assert_eq!(w.len(), 0);
    }

    #[test]
    fn retiring_a_key_with_no_worker_closes_its_default_state() {
        let (w, closed_log) = workers(5_000, None);
        for h in w.retire(&[7, 8]) {
            h.join().unwrap();
        }
        assert_eq!(closed(&closed_log), vec![(7, 0), (8, 0)]);
    }

    #[tokio::test]
    async fn retire_during_the_idle_release_still_closes() {
        let (w, closed_log) = workers(5_000, Some(20));
        w.run(&1, bump).await.unwrap();
        // Hold the registry while the idle timer fires, then retire under the
        // same lock: the worker's release finds no slot and must read `Retire`.
        let mut slots = w.inner.slots();
        std::thread::sleep(Duration::from_millis(200));
        let slot = slots.remove(&1).unwrap();
        slot.tx.send(Msg::Retire).unwrap();
        drop(slots);
        slot.thread.join().unwrap();
        assert_eq!(closed(&closed_log), vec![(1, 1)]);
    }

    #[tokio::test]
    async fn an_idle_worker_closes_itself() {
        let (w, closed_log) = workers(5_000, Some(20));
        w.run(&1, bump).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while closed(&closed_log).is_empty() {
            assert!(Instant::now() < deadline, "never idled out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(closed(&closed_log), vec![(1, 1)]);
        assert_eq!(w.len(), 0);
    }
}
