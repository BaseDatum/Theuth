//! One obscura isolate per session on its own OS thread (23 §3.2).
//! `Page`/`BrowserState` are `!Send` (RefCell + V8), so the thread owns a
//! `current_thread` runtime and one `BrowserState`; commands arrive over a
//! channel and the loop mirrors `obscura_mcp::run`: between commands it pumps
//! `advance_active_page_tasks` so timers and fetches keep running. Dropping
//! every [`Handle`] closes the channel, the loop ends and the isolate dies
//! with the thread; [`Handle::exited`] observes that (51 §4.5 close barrier).
//!
//! The isolate *is* the profile: `BrowserState` holds the cookie jar and
//! storage in memory and obscura writes nothing to disk, so a private root's
//! browser state never touches the data dir and is gone once the thread ends.

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use crate::BrowserConfig;

/// Error text when the browser thread is gone (panic inside the isolate, or
/// the thread failed to start).
pub const THREAD_EXITED: &str = "browser thread exited";

/// Command to the browser thread.
enum Cmd {
    /// One `obscura_mcp::call_tool`; the reply is its `result` object.
    Call {
        name: String,
        args: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
}

/// Clonable sender to one session's browser thread.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Cmd>,
    /// Cancelled when the thread ends (or never started).
    exited: CancellationToken,
}

impl Handle {
    /// Run one obscura-mcp tool on this session's isolate. `Ok` is the MCP
    /// `result` object (`{ content, isError }`, 23 §3.3); `Err` is
    /// [`THREAD_EXITED`] when the thread is gone.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Call {
                name: name.to_owned(),
                args,
                reply,
            })
            .await
            .map_err(|_| THREAD_EXITED.to_owned())?;
        rx.await.map_err(|_| THREAD_EXITED.to_owned())?
    }

    /// `true` once the browser thread has ended (its receiver dropped).
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Resolves once the browser thread has ended. Every [`Handle`] must be
    /// dropped first (the future holds none), or the loop never sees its
    /// channel close.
    pub fn exited(&self) -> WaitForCancellationFutureOwned {
        self.exited.clone().cancelled_owned()
    }
}

/// Start a browser thread for one session and return its handle. A thread
/// that fails to start (or panics later) is observed through the closed
/// channel: [`Handle::call`] returns [`THREAD_EXITED`] and the actor
/// spawns a fresh one on the next call (23 §4 errors).
pub fn spawn(cfg: &BrowserConfig) -> Handle {
    let (tx, rx) = mpsc::channel(16);
    let exited = CancellationToken::new();
    // Cancels on any exit path — normal end, panic unwinding, or the closure
    // being dropped because the thread never started.
    let guard = exited.clone().drop_guard();
    let proxy = cfg.proxy.clone();
    let user_agent = cfg.user_agent.clone();
    let stealth = cfg.stealth;
    // obscura_render::dom::build recurses per DOM nesting level with large
    // frames; a 12-deep page overflowed the 2 MiB default (release build).
    // Virtual reservation only, so size for the deepest page we expect.
    let spawned = std::thread::Builder::new()
        .name("browser".to_owned())
        .stack_size(64 << 20)
        .spawn(move || {
            let _exit = guard;
            run(rx, proxy, user_agent, stealth);
        });
    if let Err(e) = spawned {
        tracing::error!("cannot start browser thread: {e}");
    }
    Handle { tx, exited }
}

/// Thread body: `current_thread` runtime, one `BrowserState`, the pump loop.
/// A panic anywhere in here unwinds through `block_on`, drops `rx` and ends
/// the thread; nothing joins it, so the panic never propagates (the process
/// panic hook logs it).
fn run(
    mut rx: mpsc::Receiver<Cmd>,
    proxy: Option<String>,
    user_agent: Option<String>,
    stealth: bool,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot build browser runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut state = obscura_mcp::BrowserState::new(proxy, user_agent, stealth);
        // Armed while the active page has pending JS work (timers, fetches):
        // then the loop races the next command against one pump turn.
        let mut pump_armed = false;
        loop {
            let cmd = if pump_armed {
                tokio::select! {
                    biased;
                    cmd = rx.recv() => cmd,
                    pumped = state.advance_active_page_tasks() => {
                        pump_armed = match pumped {
                            Ok(reached_idle) => !reached_idle,
                            Err(e) => {
                                tracing::warn!("browser page task failed: {e}");
                                false
                            }
                        };
                        continue;
                    }
                }
            } else {
                rx.recv().await
            };
            let Some(Cmd::Call { name, args, reply }) = cmd else {
                break;
            };
            let out = obscura_mcp::call_tool(&mut state, &name, &args).await;
            pump_armed = state.has_active_page_runtime();
            // The caller may have given up (timeout); nothing to do about it.
            let _ = reply.send(out);
        }
    });
}
