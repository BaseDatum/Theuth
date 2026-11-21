//! [`DocumentsThread`]: the `Send + Clone` face of [`DocumentsJs`]. One `documents-js` OS thread (16 MB stack,
//! started on the first request) owns a current-thread tokio runtime for waiting on jobs and the `JsRuntime`;
//! jobs run to completion in enqueue order; a fatal failure fails the job and every job queued behind it,
//! then the runtime is dropped and rebuilt and [`DocumentsThread::generation`] bumps.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::{DocumentsJs, Error, Limits, Reply, panic_message};

/// Stack of the runtime thread: V8 and BlockNote's recursive converters on deeply nested documents.
const STACK: usize = 16 * 1024 * 1024;

enum Work {
    Call(String),
    Eval(String),
    Panic,
}

struct Job {
    work: Work,
    reply: oneshot::Sender<Result<Reply, Error>>,
}

struct Inner {
    limits: Limits,
    shutdown: CancellationToken,
    /// `None` until the first request starts the thread.
    tx: Mutex<Option<mpsc::UnboundedSender<Job>>>,
    generation: Arc<watch::Sender<u64>>,
    alive: Arc<AtomicBool>,
}

/// Handle to the in-process documents runtime; cheap to clone, `Send + Sync`. The thread starts on the first
/// request and ends when the last handle is dropped or `shutdown` is cancelled.
#[derive(Clone)]
pub struct DocumentsThread {
    inner: Arc<Inner>,
}

/// A request already in the queue (enqueued synchronously, so ordering is the caller's enqueue order);
/// resolves to its [`Reply`].
#[must_use = "the request runs regardless; await the ticket for its reply"]
pub struct Ticket {
    rx: oneshot::Receiver<Result<Reply, Error>>,
}

impl Future for Ticket {
    type Output = Result<Reply, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|r| r.unwrap_or(Err(Error::Stopped)))
    }
}

impl Ticket {
    /// A ticket already settled with `result` (lets a client substitute canned replies in its tests).
    #[doc(hidden)]
    pub fn ready(result: Result<Reply, Error>) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(result);
        Self { rx }
    }

    fn failed(e: Error) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(e));
        Self { rx }
    }
}

impl DocumentsThread {
    /// A handle; no thread and no isolate until the first request.
    pub fn new(limits: Limits, shutdown: CancellationToken) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                shutdown,
                tx: Mutex::new(None),
                generation: Arc::new(watch::channel(0).0),
                alive: Arc::new(AtomicBool::new(false)),
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
            return Ticket::failed(Error::TooLarge {
                len: line.len(),
                max: self.inner.limits.max_line,
            });
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
        *self.inner.generation.borrow()
    }

    /// Watch [`Self::generation`]: a change to a value above 1 means "runtime was rebuilt; reopen mirrors".
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.generation.subscribe()
    }

    /// Whether the runtime thread is currently running.
    pub fn is_running(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
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

    /// The thread's liveness flag, observable after every handle is dropped (shutdown tests).
    #[doc(hidden)]
    pub fn liveness(&self) -> Arc<AtomicBool> {
        self.inner.alive.clone()
    }

    fn send(&self, work: Work) -> Ticket {
        let (reply, rx) = oneshot::channel();
        // Held across the send: one total order of jobs over every handle.
        let mut tx = self.inner.tx.lock().unwrap_or_else(|e| e.into_inner());
        if tx.is_none() {
            if self.inner.shutdown.is_cancelled() {
                return Ticket::failed(Error::Stopped);
            }
            match self.spawn() {
                Ok(sender) => *tx = Some(sender),
                Err(e) => return Ticket::failed(e),
            }
        }
        if let Some(sender) = tx.as_ref()
            && let Err(mpsc::error::SendError(job)) = sender.send(Job { work, reply })
        {
            let _ = job.reply.send(Err(Error::Stopped));
        }
        Ticket { rx }
    }

    fn spawn(&self) -> Result<mpsc::UnboundedSender<Job>, Error> {
        let (tx, rx) = mpsc::unbounded_channel();
        let limits = self.inner.limits.clone();
        let shutdown = self.inner.shutdown.clone();
        let generation = self.inner.generation.clone();
        let alive = self.inner.alive.clone();
        alive.store(true, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("documents-js".into())
            .stack_size(STACK)
            .spawn(move || {
                let _alive = Alive(alive);
                serve(rx, &limits, &shutdown, &generation);
            })
            .map_err(|e| {
                self.inner.alive.store(false, Ordering::SeqCst);
                Error::Crashed(format!("spawn the documents-js thread: {e}"))
            })?;
        Ok(tx)
    }
}

/// Clears the liveness flag however the thread ends.
struct Alive(Arc<AtomicBool>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn build(limits: &Limits) -> Result<DocumentsJs, Error> {
    catch_unwind(AssertUnwindSafe(|| DocumentsJs::new(limits.clone()))).unwrap_or_else(|p| {
        Err(Error::Crashed(format!(
            "panic while building the runtime: {}",
            panic_message(&*p)
        )))
    })
}

/// Drop a runtime that may be in a bad state; a panic while dropping it must not end the thread.
fn discard(js: DocumentsJs) {
    if let Err(p) = catch_unwind(AssertUnwindSafe(move || drop(js))) {
        tracing::error!(panic = %panic_message(&*p), "documents runtime panicked while being dropped");
    }
}

fn run(js: &mut DocumentsJs, work: Work) -> Result<Reply, Error> {
    match work {
        Work::Call(line) => js.serve(line).and_then(Reply::from_lines),
        Work::Eval(script) => js.eval_raw(&script).map(|()| Reply {
            events: Vec::new(),
            reply: String::new(),
        }),
        Work::Panic => panic!("test: panic on the documents-js thread"),
    }
}

/// The thread body: wait for a job (or shutdown), build the runtime if there is none, run the job with
/// panics caught, and after a fatal failure fail the queued jobs, drop the runtime and build a fresh one.
fn serve(
    mut rx: mpsc::UnboundedReceiver<Job>,
    limits: &Limits,
    shutdown: &CancellationToken,
    generation: &watch::Sender<u64>,
) {
    let waiter = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "documents-js: no tokio runtime");
            return;
        }
    };
    let mut js: Option<DocumentsJs> = None;
    let built = |js: &mut Option<DocumentsJs>| -> Result<(), Error> {
        let fresh = build(limits)?;
        *js = Some(fresh);
        generation.send_modify(|n| *n += 1);
        Ok(())
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
        if js.is_none()
            && let Err(e) = built(&mut js)
        {
            tracing::warn!(error = %e, "documents runtime could not be built");
            let _ = job.reply.send(Err(e));
            continue;
        }
        let Some(runtime) = js.as_mut() else { continue };
        let result =
            catch_unwind(AssertUnwindSafe(|| run(runtime, job.work))).unwrap_or_else(|p| {
                Err(Error::Crashed(format!(
                    "panic on the documents-js thread: {}",
                    panic_message(&*p)
                )))
            });
        let fatal = result.as_ref().err().filter(|e| e.is_fatal()).cloned();
        let _ = job.reply.send(result);
        if let Some(e) = fatal {
            tracing::warn!(error = %e, "documents runtime failed; failing queued requests and rebuilding");
            if let Some(dead) = js.take() {
                discard(dead);
            }
            // Like a crashed child: whatever was queued behind the failure fails with it.
            while let Ok(queued) = rx.try_recv() {
                let _ = queued.reply.send(Err(e.clone()));
            }
            if let Err(e) = built(&mut js) {
                tracing::warn!(error = %e, "documents runtime could not be rebuilt; retrying on the next request");
            }
        }
    }
    if let Some(runtime) = js.take() {
        discard(runtime);
    }
}
