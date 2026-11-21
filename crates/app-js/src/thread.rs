//! [`BundleThread`]: the `Send + Clone` face of [`BundleJs`]. One OS thread per handle family (named after
//! [`Bundle::thread`], 16 MB stack, started on the first request) owns a current-thread tokio runtime for
//! waiting on jobs and the `JsRuntime`; jobs run to completion in enqueue order; a fatal failure fails the
//! job and every job queued behind it, then the runtime is dropped and rebuilt and
//! [`BundleThread::generation`] bumps. Each bundle gets its own thread and isolate (and its own limits).
//!
//! Copied from `crates/document-js/src/thread.rs` (feat/documents-inproc 0ca80bbf3); the only change is the
//! [`Bundle`] parameter.
//!
//! Every job carries the *epoch* it was enqueued under. The epoch moves on when the runtime it served is
//! given up: a job failed fatally (bumped by the thread before that job's reply is sent), or the client
//! called [`BundleThread::restart`]. A job from an older epoch never runs on a newer runtime: it fails
//! with the error that ended its epoch. So whatever a client enqueues after it has seen a fatal failure runs
//! on the fresh runtime, and nothing enqueued before it does.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{Bundle, BundleJs, Error, Limits, Reply, panic_message};

/// Stack of the runtime thread: V8 plus recursive parsers on deeply nested input (document-js's value).
const STACK: usize = 16 * 1024 * 1024;

enum Work {
    Call(String),
    Eval(String),
    Panic,
    /// End the thread as if `serve` had returned early (test hook for the respawn path).
    #[cfg(test)]
    Exit,
}

struct Job {
    work: Work,
    /// [`Shared::epoch`] when the job was enqueued.
    epoch: u64,
    /// The thread that served earlier jobs died before this one; it is the first job of a new thread.
    lost: bool,
    reply: oneshot::Sender<Settled>,
}

/// What a job's ticket resolves to.
#[derive(Debug)]
pub struct Settled {
    /// The output, or why there is none.
    pub result: Result<Reply, Error>,
    /// The runtime that served the jobs before this one is gone, with every mirror it held: this job's own
    /// failure gave it up, or the thread had died and this job respawned it. True for exactly one job per
    /// lost runtime (the jobs queued behind a failure fail with the same error but `false`); a runtime given
    /// up through [`BundleThread::restart`] is reported by the caller of `restart`.
    pub rebuilt: bool,
}

/// State the handles and the thread share.
struct Shared {
    epoch: AtomicU64,
    generation: AtomicU64,
    /// Runtime threads currently running (a respawned thread can overlap the end of the dead one).
    alive: AtomicUsize,
}

struct Inner {
    bundle: &'static Bundle,
    limits: Limits,
    shutdown: CancellationToken,
    /// `None` until the first request starts the thread.
    tx: Mutex<Option<mpsc::UnboundedSender<Job>>>,
    shared: Arc<Shared>,
}

/// Handle to the in-process JS runtime; cheap to clone, `Send + Sync`. The thread starts on the first
/// request and ends when the last handle is dropped or `shutdown` is cancelled.
#[derive(Clone)]
pub struct BundleThread {
    inner: Arc<Inner>,
}

/// A request already in the queue (enqueued synchronously, so ordering is the caller's enqueue order);
/// resolves to its [`Reply`].
#[must_use = "the request runs regardless; await the ticket for its reply"]
pub struct Ticket {
    epoch: u64,
    rx: oneshot::Receiver<Settled>,
}

impl Future for Ticket {
    type Output = Result<Reply, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|r| r.map_or(Err(Error::Stopped), |s| s.result))
    }
}

impl Ticket {
    /// A ticket already settled with `result` (lets a client substitute canned replies in its tests).
    #[doc(hidden)]
    pub fn ready(result: Result<Reply, Error>) -> Self {
        Self::settled_with(0, result)
    }

    fn settled_with(epoch: u64, result: Result<Reply, Error>) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Settled {
            result,
            rebuilt: false,
        });
        Self { epoch, rx }
    }

    /// The epoch the request was enqueued under; compare with what [`BundleThread::restart`] returned.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The result together with whether this request's failure gave up the runtime.
    pub async fn settled(self) -> Settled {
        self.rx.await.unwrap_or(Settled {
            result: Err(Error::Stopped),
            rebuilt: false,
        })
    }
}

impl BundleThread {
    /// A handle serving `bundle`; no thread and no isolate until the first request.
    pub fn new(bundle: &'static Bundle, limits: Limits, shutdown: CancellationToken) -> Self {
        Self {
            inner: Arc::new(Inner {
                bundle,
                limits,
                shutdown,
                tx: Mutex::new(None),
                shared: Arc::new(Shared {
                    epoch: AtomicU64::new(0),
                    generation: AtomicU64::new(0),
                    alive: AtomicUsize::new(0),
                }),
            }),
        }
    }

    /// The guards every runtime of this thread enforces.
    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    /// Enqueue one request line now; await the ticket for its output. A line over [`Limits::max_line`]
    /// fails with [`Error::TooLarge`] without reaching the thread.
    pub fn enqueue(&self, line: String) -> Ticket {
        if line.len() > self.inner.limits.max_line {
            return Ticket::settled_with(
                self.epoch(),
                Err(Error::TooLarge {
                    len: line.len(),
                    max: self.inner.limits.max_line,
                }),
            );
        }
        self.send(Work::Call(line))
    }

    /// [`Self::enqueue`] and await.
    pub async fn request(&self, line: String) -> Result<Reply, Error> {
        self.enqueue(line).await
    }

    /// How many runtimes have been built: 0 before the first request, 1 once serving, +1 per rebuild after a
    /// fatal failure (every mirror the previous runtime held is gone).
    pub fn generation(&self) -> u64 {
        self.inner.shared.generation.load(Ordering::SeqCst)
    }

    /// The current epoch (see the module docs): what a request enqueued now is stamped with.
    pub fn epoch(&self) -> u64 {
        self.inner.shared.epoch.load(Ordering::SeqCst)
    }

    /// Give up the current runtime although no request failed fatally: the client saw output it cannot
    /// trust (a reply naming another request). Every request already enqueued fails with
    /// [`Error::Crashed`] instead of running (one that is running now has its result replaced), and the next
    /// one runs on a fresh runtime. Returns the new epoch: tickets with a lower [`Ticket::epoch`] belong to
    /// the abandoned runtime. Reporting the lost mirrors is the caller's job ([`Settled::rebuilt`] stays
    /// false for these requests).
    pub fn restart(&self) -> u64 {
        // Under the queue lock: no enqueue interleaves with the bump.
        let _tx = self.inner.tx.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Whether the runtime thread is currently running.
    pub fn is_running(&self) -> bool {
        self.inner.shared.alive.load(Ordering::SeqCst) > 0
    }

    /// Run a raw classic script on the runtime under the request guards (containment tests).
    #[doc(hidden)]
    pub fn eval_raw(&self, script: String) -> Ticket {
        self.send(Work::Eval(script))
    }

    /// Panic on the runtime thread outside JS (containment tests).
    #[doc(hidden)]
    pub fn panic_on_thread(&self) -> Ticket {
        self.send(Work::Panic)
    }

    /// Whether a runtime thread is running, observable after every handle is dropped (shutdown tests).
    #[doc(hidden)]
    pub fn liveness(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let shared = self.inner.shared.clone();
        move || shared.alive.load(Ordering::SeqCst) > 0
    }

    /// End the runtime thread the way an early return from its loop would (respawn test).
    #[cfg(test)]
    pub(crate) fn exit_thread(&self) -> Ticket {
        self.send(Work::Exit)
    }

    fn send(&self, work: Work) -> Ticket {
        let (reply, rx) = oneshot::channel();
        // Held across the send: one total order of jobs over every handle.
        let mut tx = self.inner.tx.lock().unwrap_or_else(|e| e.into_inner());
        let epoch = self.epoch();
        let mut job = Job {
            work,
            epoch,
            lost: false,
            reply,
        };
        // A thread that ended without shutdown (its loop returned early) left a closed queue: respawn once.
        for _ in 0..2 {
            if tx.is_none() {
                if self.inner.shutdown.is_cancelled() {
                    break;
                }
                match self.spawn() {
                    Ok(sender) => *tx = Some(sender),
                    Err(e) => {
                        let _ = job.reply.send(Settled {
                            result: Err(e),
                            rebuilt: false,
                        });
                        return Ticket { epoch, rx };
                    }
                }
            }
            match tx.as_ref().map(|sender| sender.send(job)) {
                Some(Ok(())) => return Ticket { epoch, rx },
                Some(Err(mpsc::error::SendError(back))) => {
                    job = back;
                    // Its runtime (if it had built one) went with it.
                    job.lost = self.generation() > 0;
                    *tx = None;
                }
                None => unreachable!("a sender was just installed"),
            }
        }
        let _ = job.reply.send(Settled {
            result: Err(Error::Stopped),
            rebuilt: false,
        });
        Ticket { epoch, rx }
    }

    fn spawn(&self) -> Result<mpsc::UnboundedSender<Job>, Error> {
        let (tx, rx) = mpsc::unbounded_channel();
        let limits = self.inner.limits.clone();
        let bundle = self.inner.bundle;
        let shutdown = self.inner.shutdown.clone();
        let shared = self.inner.shared.clone();
        shared.alive.fetch_add(1, Ordering::SeqCst);
        let alive = Alive(shared.clone());
        std::thread::Builder::new()
            .name(bundle.thread.into())
            .stack_size(STACK)
            .spawn(move || {
                let alive = alive;
                serve(rx, bundle, &limits, &shutdown, &alive.0);
            })
            .map_err(|e| Error::Crashed(format!("spawn the app-js thread: {e}")))?;
        Ok(tx)
    }
}

/// Counts a runtime thread out however it ends (also when it never started: the closure is dropped).
struct Alive(Arc<Shared>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

fn build(bundle: &'static Bundle, limits: &Limits) -> Result<BundleJs, Error> {
    catch_unwind(AssertUnwindSafe(|| BundleJs::new(bundle, limits.clone()))).unwrap_or_else(|p| {
        Err(Error::Crashed(format!(
            "panic while building the runtime: {}",
            panic_message(&*p)
        )))
    })
}

/// Drop a runtime that may be in a bad state; a panic while dropping it must not end the thread.
fn discard(js: BundleJs) {
    if let Err(p) = catch_unwind(AssertUnwindSafe(move || drop(js))) {
        tracing::error!(panic = %panic_message(&*p), "JS runtime panicked while being dropped");
    }
}

fn run(js: &mut BundleJs, work: Work) -> Result<Reply, Error> {
    match work {
        Work::Call(line) => js.serve(line).and_then(Reply::from_lines),
        Work::Eval(script) => js.eval_raw(&script).map(|()| Reply {
            events: Vec::new(),
            reply: String::new(),
        }),
        Work::Panic => panic!("test: panic on the app-js thread"),
        #[cfg(test)]
        Work::Exit => unreachable!("handled by serve"),
    }
}

/// The thread body: wait for a job (or shutdown); fail a job from an older epoch with the error that ended
/// it; drop a runtime the epoch has moved past; build the runtime if there is none; run the job with panics
/// caught. A fatal failure moves the epoch on *before* its reply is sent, then the runtime is dropped and a
/// fresh one built.
fn serve(
    mut rx: mpsc::UnboundedReceiver<Job>,
    bundle: &'static Bundle,
    limits: &Limits,
    shutdown: &CancellationToken,
    shared: &Shared,
) {
    let waiter = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "app-js: no tokio runtime");
            return;
        }
    };
    // The runtime and the epoch it was built in.
    let mut js: Option<(BundleJs, u64)> = None;
    // The last epoch the thread ended itself, and why: the error its queued jobs fail with.
    let mut ended: Option<(u64, Error)> = None;
    let build_now = |js: &mut Option<(BundleJs, u64)>| -> Result<(), Error> {
        let epoch = shared.epoch.load(Ordering::SeqCst);
        let fresh = build(bundle, limits)?;
        *js = Some((fresh, epoch));
        shared.generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let stale = |job_epoch: u64, ended: &Option<(u64, Error)>| match ended {
        Some((epoch, e)) if *epoch == job_epoch => e.clone(),
        _ => Error::Crashed(
            "the JS runtime was given up before this request was answered; it is rebuilt"
                .into(),
        ),
    };
    loop {
        let job = waiter.block_on(async {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => None,
                job = rx.recv() => job,
            }
        });
        let Some(job) = job else { break };
        #[cfg(test)]
        {
            if matches!(job.work, Work::Exit) {
                // Close the queue before the job's ticket resolves, as a returning `serve` would.
                drop(rx);
                drop(job);
                break;
            }
        }
        let now = shared.epoch.load(Ordering::SeqCst);
        if job.epoch < now {
            let _ = job.reply.send(Settled {
                result: Err(stale(job.epoch, &ended)),
                rebuilt: false,
            });
            continue;
        }
        if js.as_ref().is_some_and(|(_, built)| *built < now)
            && let Some((dead, _)) = js.take()
        {
            discard(dead);
        }
        if js.is_none()
            && let Err(e) = build_now(&mut js)
        {
            tracing::warn!(error = %e, "JS runtime could not be built");
            let _ = job.reply.send(Settled {
                result: Err(e),
                rebuilt: job.lost,
            });
            continue;
        }
        let Some((runtime, _)) = js.as_mut() else {
            continue;
        };
        let lost = job.lost;
        let mut result =
            catch_unwind(AssertUnwindSafe(|| run(runtime, job.work))).unwrap_or_else(|p| {
                Err(Error::Crashed(format!(
                    "panic on the app-js thread: {}",
                    panic_message(&*p)
                )))
            });
        let fatal = result.as_ref().err().filter(|e| e.is_fatal()).cloned();
        let rebuilt = fatal.is_some() || lost;
        if let Some(e) = &fatal {
            // Before the reply: whoever reacts to this failure enqueues into the new epoch.
            let epoch = shared.epoch.fetch_add(1, Ordering::SeqCst);
            ended = Some((epoch, e.clone()));
        } else if shared.epoch.load(Ordering::SeqCst) > job.epoch {
            // `restart` while this job ran: its output came from the runtime the client gave up.
            result = Err(stale(job.epoch, &ended));
        }
        let _ = job.reply.send(Settled { result, rebuilt });
        if js
            .as_ref()
            .is_some_and(|(_, built)| *built < shared.epoch.load(Ordering::SeqCst))
        {
            if let Some(e) = &fatal {
                tracing::warn!(error = %e, "JS runtime failed; failing queued requests and rebuilding");
            }
            if let Some((dead, _)) = js.take() {
                discard(dead);
            }
            if let Err(e) = build_now(&mut js) {
                tracing::warn!(error = %e, "JS runtime could not be rebuilt; retrying on the next request");
            }
        }
    }
    if let Some((runtime, _)) = js.take() {
        discard(runtime);
    }
}
