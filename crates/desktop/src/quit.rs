//! 163: the graceful quit. Every quit entry (tray and app menu Quit, `theuth://quit`, Settings › Install and
//! restart; 166: Dock › Quit and any other user `terminate:`, `dock.rs`) lands in [`request`]: the main
//! webview shows its shutdown overlay on `quit-requested`, runs RPC
//! `shutdown` when it owns the engine and acks with `quit_ack`. The ack — or [`ACK_TIMEOUT`] without one —
//! starts the quit worker, which stops the companion and then the engine on its own thread, reports each
//! step as `quit-progress` and calls [`update::exit`]. The main thread never waits: a window whose run loop
//! stops turning gets the beach ball. [`on_exit`] is `RunEvent::Exit`; after the worker it finds nothing left.
//! 171 §5.3: while the webview's pre-quit sheet lists live cell runs it sends [`HOLD`] instead of the ack; the
//! timer stands down until `quit_ack` (proceed) or [`CANCEL`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::launch::{self, Daemon, Launch};
use crate::{companion, update};

/// The webview acks `quit-requested` with `quit_ack` (after RPC `shutdown` when it owns the engine); without
/// an ack the worker starts after this.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);
/// `durability.drain_secs`' default (109 §9.2): how long the engine drains running turns after `shutdown`.
const DRAIN: Duration = Duration::from_secs(10);
/// D2: the worker has called exit this long after it started, or the engine is killed and the app exits.
const CAP: Duration = DRAIN
    .saturating_add(launch::EXIT_GRACE)
    .saturating_add(Duration::from_secs(3));

/// Event to every webview: [`Requested`]. The main window shows the overlay, AUX windows ignore it.
const REQUESTED: &str = "quit-requested";
/// Event to every webview: [`Progress`], one per worker step.
const PROGRESS: &str = "quit-progress";
/// 171 §5.3 (F6 a): from the main webview, instead of the ack, while its pre-quit sheet lists live cell runs —
/// the ack timer stands down; the sheet ends with `quit_ack` (Wait finished / Stop it and quit) or [`CANCEL`].
const HOLD: &str = "quit-hold";
/// From the main webview: the sheet's Cancel — the quit is off and the next entry asks again.
const CANCEL: &str = "quit-cancel";

/// Payload of [`REQUESTED`].
#[derive(Clone, Copy, Serialize)]
struct Requested {
    /// Settings › Install and restart: the overlay says "Restarting to update".
    restart: bool,
}

/// A worker step, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Step {
    /// Only where a companion runs (D4).
    Companion,
    Engine,
    Closing,
}

/// Payload of [`PROGRESS`]: `{ step, forced }`. `forced` comes as a second `engine` event when the daemon
/// outlived its grace and was killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct Progress {
    step: Step,
    forced: bool,
}

impl Progress {
    const fn of(step: Step) -> Self {
        Self {
            step,
            forced: false,
        }
    }
}

/// Managed: the latches of a quit — the handshake asked once, the worker run once — and 171's hold: which
/// request the ack timer belongs to (`round`) and whether the webview's pre-quit sheet holds it.
#[derive(Default)]
pub struct Quit {
    requested: AtomicBool,
    started: AtomicBool,
    round: AtomicU64,
    held: AtomicBool,
}

impl Quit {
    /// Run the worker unless it already ran; `plan` is only built for the first call. `false` for a repeat.
    fn start(&self, plan: impl FnOnce() -> Plan) -> bool {
        if self.started.swap(true, SeqCst) {
            return false;
        }
        run(plan());
        true
    }

    /// A new quit request: its round, or `None` while one is under way.
    fn begin(&self) -> Option<u64> {
        if self.requested.swap(true, SeqCst) {
            return None;
        }
        self.held.store(false, SeqCst);
        Some(self.round.fetch_add(1, SeqCst) + 1)
    }

    /// The pre-quit sheet holds the request: `false` when no request waits for its ack.
    fn hold(&self) -> bool {
        if !self.requested.load(SeqCst) || self.started.load(SeqCst) {
            return false;
        }
        self.held.store(true, SeqCst);
        true
    }

    /// The sheet's Cancel: the request is dropped (the next entry asks again); `false` once the worker runs.
    fn cancel(&self) -> bool {
        if self.started.load(SeqCst) || !self.requested.load(SeqCst) {
            return false;
        }
        self.held.store(false, SeqCst);
        self.requested.store(false, SeqCst);
        true
    }

    /// The ack timer of `round` ran out: the worker starts unless the sheet holds it or a later request owns the timer.
    fn timer_due(&self, round: u64) -> bool {
        self.round.load(SeqCst) == round && self.requested.load(SeqCst) && !self.held.load(SeqCst)
    }
}

/// What one worker run drives: [`app_plan`] fills it from the app, the tests with fakes and milliseconds.
struct Plan {
    /// `quit-progress` out.
    emit: Box<dyn Fn(Progress) + Send>,
    /// Stops the companion; `Some` only while one runs (D4).
    companion: Option<Box<dyn FnOnce() + Send>>,
    /// The daemon this app owns (`None`: a reused engine keeps running).
    daemon: Option<Daemon>,
    /// How long the daemon may take to exit after its hang-up before it is killed.
    grace: Duration,
    /// D2: from the start of the run, the worker has exited by now or the cap kills the daemon and exits.
    cap: Duration,
    /// [`update::exit`]; the worker or the cap calls it, the first one wins.
    exit: Box<dyn Fn() + Send + Sync>,
}

/// 25 §3 `/quit`, every entry: tell the webviews (the main one shows the overlay, runs RPC `shutdown` if it
/// owns the daemon, then `quit_ack`); start the worker anyway after [`ACK_TIMEOUT`]. D6: a hidden main
/// window stays hidden, the tray says what is happening instead.
pub fn request(app: &AppHandle) {
    let Some(round) = app.state::<Quit>().begin() else {
        return;
    };
    listen_hold(app);
    let shown = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if !shown {
        log_err(tray_shutting_down(app), "tray: shutting down");
    }
    let restart = app.state::<update::Pending>().restart.load(SeqCst);
    if let Err(e) = app.emit(REQUESTED, Requested { restart }) {
        tracing::warn!(error = %e, "emit {REQUESTED}; quitting without the webview");
        start(app);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(ACK_TIMEOUT);
        if app.state::<Quit>().timer_due(round) {
            start(&app);
        }
    });
}

/// 171 §5.3: the webview's [`HOLD`] / [`CANCEL`], subscribed once. A hold shows the main window, so a quit from
/// the tray or the companion with the window hidden still puts the sheet in front of the user.
fn listen_hold(app: &AppHandle) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let handle = app.clone();
        app.listen(HOLD, move |_| {
            if handle.state::<Quit>().hold()
                && let Some(w) = handle.get_webview_window("main")
            {
                log_err(w.show(), "show main window for the pre-quit sheet");
                log_err(w.set_focus(), "focus main window for the pre-quit sheet");
            }
        });
        let handle = app.clone();
        app.listen(CANCEL, move |_| {
            if handle.state::<Quit>().cancel() {
                tracing::info!("quit cancelled from the pre-quit sheet");
                log_err(tray_restore(&handle), "tray: restore after a cancelled quit");
            }
        });
    });
}

/// `quit_ack`, the ack timeout or a webview that cannot be told: the worker, once.
pub fn start(app: &AppHandle) {
    app.state::<Quit>().start(|| app_plan(app));
}

/// A quit is under way: 166's Dock click and ⌘Tab leave a hidden main window hidden then (D6).
pub fn requested(app: &AppHandle) -> bool {
    app.state::<Quit>().requested.load(SeqCst)
}

/// `RunEvent::Exit`. D3: every window goes first, so an exit that bypassed the handshake (logout, a system
/// quit) never leaves a window on screen while the main thread waits below. 39 §3.3 / 130 §5.8: the companion,
/// then the daemon, then the bundle swap — after the worker the first two return at once.
pub fn on_exit(app: &AppHandle) {
    for win in app.webview_windows().values() {
        log_err(win.hide(), "hide window at exit");
    }
    flush_window_server();
    companion::terminate();
    app.state::<Launch>().shutdown();
    update::install_pending(app);
}

/// macOS: commit the pending Core Animation transaction now, so the `orderOut:`s above reach the window server
/// before the main thread blocks instead of at the next run-loop turn, which never comes. Elsewhere a no-op.
fn flush_window_server() {
    #[cfg(target_os = "macos")]
    if let Some(cls) = objc2::runtime::AnyClass::get(c"CATransaction") {
        // SAFETY: `+[CATransaction flush]` takes no arguments and returns void; main thread (`RunEvent::Exit`).
        let () = unsafe { objc2::msg_send![cls, flush] };
    }
}

/// D6: the tooltip and a one-item, disabled tray menu until the process is gone; the attention count leaves the
/// title. The webview's badge writes nothing once its quit overlay is up (`App.tsx`).
fn tray_shutting_down(app: &AppHandle) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main") else {
        return Ok(());
    };
    tray.set_tooltip(Some("theuth \u{2014} shutting down\u{2026}"))?;
    tray.set_title(None::<&str>)?;
    let item = MenuItem::with_id(
        app,
        "shutting-down",
        "Shutting down\u{2026}",
        false,
        None::<&str>,
    )?;
    tray.set_menu(Some(Menu::with_items(app, &[&item])?))
}

/// 171 §5.3: the sheet's Cancel undoes [`tray_shutting_down`]; the webview's badge rewrites the title.
fn tray_restore(app: &AppHandle) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main") else {
        return Ok(());
    };
    tray.set_tooltip(Some("theuth"))?;
    tray.set_menu(Some(crate::desktop::tray_menu(app)?))
}

/// The real run: progress to every webview, the companion if it runs, the daemon out of [`Launch`].
fn app_plan(app: &AppHandle) -> Plan {
    let emitter = app.clone();
    let exiter = app.clone();
    Plan {
        emit: Box::new(move |p| log_err(emitter.emit(PROGRESS, p), "emit quit-progress")),
        companion: companion::alive()
            .then(|| Box::new(companion::terminate) as Box<dyn FnOnce() + Send>),
        daemon: app.state::<Launch>().take_daemon(),
        grace: launch::EXIT_GRACE,
        cap: CAP,
        exit: Box::new(move || update::exit(&exiter)),
    }
}

/// Start the worker and its cap; returns at once. The daemon's tree is taken before either runs so the cap
/// can kill it whichever step the worker is in.
fn run(plan: Plan) {
    let Plan {
        emit,
        companion,
        daemon,
        grace,
        cap,
        exit,
    } = plan;
    let exited = AtomicBool::new(false);
    let exit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        if !exited.swap(true, SeqCst) {
            exit();
        }
    });
    let (done, finished) = mpsc::channel::<()>();
    let tree = daemon.as_ref().and_then(Daemon::tree);
    let cap_exit = Arc::clone(&exit);
    std::thread::spawn(move || {
        // A message never comes: the worker drops `done` once it has exited, which ends the wait early.
        if finished.recv_timeout(cap) == Err(mpsc::RecvTimeoutError::Timeout) {
            tracing::warn!("quit: not done after {cap:?}; killing the engine and exiting");
            if let Some(tree) = tree {
                tree.kill();
            }
            cap_exit();
        }
    });
    std::thread::spawn(move || {
        if let Some(terminate) = companion {
            emit(Progress::of(Step::Companion));
            terminate();
        }
        emit(Progress::of(Step::Engine));
        if daemon.is_some_and(|d| d.stop(grace)) {
            tracing::warn!("quit: the engine did not exit in {grace:?}; killed");
            emit(Progress {
                step: Step::Engine,
                forced: true,
            });
        }
        emit(Progress::of(Step::Closing));
        exit();
        drop(done);
    });
}

fn log_err<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) {
    if let Err(e) = r {
        tracing::warn!(error = %e, "{what}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Survives the hang-up and never exits on its own within a test. Unix: the `echo` is the fake's ready signal.
    #[cfg(unix)]
    const STUBBORN: (&str, &[&str]) = ("/bin/sh", &["-c", "trap '' HUP; echo; exec /bin/sleep 30"]);
    #[cfg(windows)]
    const STUBBORN: (&str, &[&str]) = ("ping", &["-n", "30", "127.0.0.1"]);
    /// Exits at the hang-up (Unix: `sleep` dies of SIGHUP) or by itself (Windows has no hang-up).
    #[cfg(unix)]
    const COOPERATIVE: (&str, &[&str]) = ("/bin/sh", &["-c", "echo; exec /bin/sleep 30"]);
    #[cfg(windows)]
    const COOPERATIVE: (&str, &[&str]) = ("cmd", &["/c", "exit"]);

    const ENGINE: Progress = Progress::of(Step::Engine);
    const FORCED: Progress = Progress {
        step: Step::Engine,
        forced: true,
    };
    const COMPANION: Progress = Progress::of(Step::Companion);
    const CLOSING: Progress = Progress::of(Step::Closing);
    const LONG: Duration = Duration::from_secs(20);
    const WAIT: Duration = Duration::from_secs(5);

    fn fake((program, args): (&str, &[&str])) -> Daemon {
        Daemon::fake(program, args)
    }

    /// A plan over fakes: a companion step that sleeps, progress and `exit` calls (with their instant) out on channels.
    fn plan(
        companion: Option<Duration>,
        daemon: Option<Daemon>,
        grace: Duration,
        cap: Duration,
    ) -> (Plan, mpsc::Receiver<Progress>, mpsc::Receiver<Instant>) {
        let (progress, steps) = mpsc::channel();
        let (exit, exits) = mpsc::channel();
        let plan = Plan {
            emit: Box::new(move |p| progress.send(p).unwrap()),
            companion: companion
                .map(|d| Box::new(move || std::thread::sleep(d)) as Box<dyn FnOnce() + Send>),
            daemon,
            grace,
            cap,
            exit: Box::new(move || exit.send(Instant::now()).unwrap()),
        };
        (plan, steps, exits)
    }

    /// The worker emits every step before it exits, so after the exit they are all queued.
    fn steps(rx: &mpsc::Receiver<Progress>) -> Vec<Progress> {
        rx.try_iter().collect()
    }

    #[test]
    fn steps_in_order_and_back_at_once() {
        let (p, progress, exits) = plan(
            Some(Duration::from_millis(50)),
            Some(fake(COOPERATIVE)),
            LONG,
            LONG,
        );
        let t0 = Instant::now();
        run(p);
        assert!(
            t0.elapsed() < Duration::from_millis(20),
            "run blocked the caller for {:?}",
            t0.elapsed()
        );
        exits.recv_timeout(WAIT).expect("exit");
        assert_eq!(steps(&progress), [COMPANION, ENGINE, CLOSING]);
    }

    #[test]
    fn no_companion_step_without_one() {
        let (p, progress, exits) = plan(None, None, LONG, LONG);
        run(p);
        exits.recv_timeout(WAIT).expect("exit");
        assert_eq!(steps(&progress), [ENGINE, CLOSING]);
    }

    #[test]
    fn kills_after_the_grace_and_says_forced() {
        let grace = Duration::from_millis(300);
        let (p, progress, exits) = plan(None, Some(fake(STUBBORN)), grace, LONG);
        let t0 = Instant::now();
        run(p);
        assert!(t0.elapsed() < Duration::from_millis(20));
        let at = exits.recv_timeout(WAIT).expect("exit");
        assert!(at - t0 >= grace, "exited before the grace: {:?}", at - t0);
        assert_eq!(steps(&progress), [ENGINE, FORCED, CLOSING]);
        assert!(exits.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_second_trigger_is_a_no_op() {
        let quit = Quit::default();
        let (p, progress, exits) = plan(None, None, LONG, LONG);
        assert!(quit.start(|| p));
        assert!(!quit.start(|| unreachable!("the plan of a repeat is never built")));
        exits.recv_timeout(WAIT).expect("exit");
        assert!(exits.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(steps(&progress), [ENGINE, CLOSING]);
    }

    /// The worker is stuck in the companion step past the cap: the cap kills the daemon and exits; the worker,
    /// once free, finds the daemon already dead (no `forced`, well inside its 20 s grace) and exits no second time.
    #[test]
    fn the_cap_kills_and_exits() {
        let cap = Duration::from_millis(200);
        let stuck = Duration::from_millis(600);
        let (p, progress, exits) = plan(Some(stuck), Some(fake(STUBBORN)), LONG, cap);
        let t0 = Instant::now();
        run(p);
        let at = exits.recv_timeout(WAIT).expect("cap exit");
        assert!(
            at - t0 >= cap && at - t0 < stuck,
            "cap fired at {:?}",
            at - t0
        );
        assert_eq!(steps(&progress), [COMPANION]);
        assert_eq!(progress.recv_timeout(WAIT), Ok(ENGINE));
        assert_eq!(progress.recv_timeout(WAIT), Ok(CLOSING));
        assert!(exits.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// The wire shapes `lib/quit-progress.ts` reads.
    #[test]
    fn payloads() {
        assert_eq!(
            serde_json::to_value(FORCED).unwrap(),
            serde_json::json!({ "step": "engine", "forced": true })
        );
        assert_eq!(
            serde_json::to_value(Requested { restart: true }).unwrap(),
            serde_json::json!({ "restart": true })
        );
    }

    /// 171 §5.3: without a hold the ack timer starts the worker — 163's path unchanged.
    #[test]
    fn no_hold_the_timer_starts_the_quit() {
        let quit = Quit::default();
        let round = quit.begin().expect("first request");
        assert_eq!(quit.begin(), None, "a repeat request is a no-op");
        assert!(quit.timer_due(round));
    }

    /// The sheet holds: the timer stands down; its "proceed" is the ordinary ack, which still starts the worker.
    #[test]
    fn a_hold_stops_the_timer_and_the_ack_proceeds() {
        let quit = Quit::default();
        let round = quit.begin().unwrap();
        assert!(quit.hold());
        assert!(!quit.timer_due(round));
        let (p, progress, exits) = plan(None, None, LONG, LONG);
        assert!(quit.start(|| p), "quit_ack after the hold runs the worker");
        exits.recv_timeout(WAIT).expect("exit");
        assert_eq!(steps(&progress), [ENGINE, CLOSING]);
        assert!(!quit.hold(), "no hold once the worker runs");
        assert!(!quit.cancel(), "no cancel once the worker runs");
    }

    /// Cancel drops the request: its timer never fires, the next entry asks again with a fresh timer.
    #[test]
    fn cancel_drops_the_request_and_its_timer() {
        let quit = Quit::default();
        let first = quit.begin().unwrap();
        assert!(quit.hold());
        assert!(quit.cancel());
        assert!(!quit.timer_due(first));
        let second = quit.begin().expect("asks again after a cancel");
        assert_ne!(first, second);
        assert!(
            !quit.timer_due(first),
            "the first request's timer stays dead"
        );
        assert!(quit.timer_due(second), "the new request is not held");
    }

    /// A hold or cancel with no request pending (an in-app quit, a late event) changes nothing.
    #[test]
    fn hold_and_cancel_need_a_pending_request() {
        let quit = Quit::default();
        assert!(!quit.hold());
        assert!(!quit.cancel());
        let round = quit.begin().unwrap();
        assert!(quit.timer_due(round));
    }
}
