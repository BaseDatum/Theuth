//! Design doc 18: reach a `t --headless` daemon. Probe the running engine
//! first (63: its `engine.json` record, else the configured port — a terminal
//! `t --serve` or another app instance is reused, 18 §3); otherwise
//! be a terminal for one command: `$SHELL -l -i -c 'exec t --headless …'` on a
//! pty, so the daemon inherits exactly the environment a terminal tab would give
//! it (18 §2). The pty transcript is drained into a 64 KiB ring and streamed to
//! the webview as `launch-log` events; readiness is the port, never the log.
//! 222: while it starts, the engine's `<data_dir>/startup.json` (phase, step,
//! heartbeat) is tailed into `startup-progress` events and decides how long the
//! wait may last — there is no fixed cap while the engine shows progress.
//!
//! Windows has no login-shell environment (58 §0.3), and a ConPTY child drags a
//! console window along and hangs in conhost's handshake on real installs, so
//! there `t.exe --headless …` is a plain hidden child (`CREATE_NO_WINDOW`) with
//! stdout/stderr piped into the same ring.

use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
#[cfg(unix)]
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

const DEFAULT_LISTEN: &str = "127.0.0.1:7777";
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a port connect may take per tick of the wait.
const POLL: Duration = Duration::from_millis(200);
/// 222: the wait's tick — port, child, `startup.json`, `startup-progress` event.
const TICK: Duration = Duration::from_millis(250);
/// 222: this long without a `seq` change marks the start `stalled` (the wait goes on).
const SOFT_STALL: Duration = Duration::from_secs(90);
/// 222: this long without a heartbeat (or progress) fails the start.
const HARD_STALL: Duration = Duration::from_secs(10 * 60);
/// 222: an engine that never writes `startup.json` for its pid (older than 222) gets the
/// pre-222 port-only wait with this cap.
const FALLBACK_CAP: Duration = Duration::from_secs(60);
/// 222: the engine's startup record in the data dir.
const STARTUP_FILE: &str = "startup.json";
/// 222: the Tauri event carrying [`StartupProgress`].
const PROGRESS_EVENT: &str = "startup-progress";
/// How long a daemon told `shutdown` may take to exit before it is killed.
/// 109 §9.2: the daemon drains in-flight turns for `durability.drain_secs`
/// (default 10 s) plus a second of slack before it exits; this covers that
/// default so a quit never SIGKILLs a drain in progress. A daemon with
/// nothing to drain exits in well under a second as before.
pub const EXIT_GRACE: Duration = Duration::from_secs(12);
/// How often [`Daemon::stop`] looks for the daemon's exit.
const EXIT_POLL: Duration = Duration::from_millis(100);
const LOG_CAP: usize = 64 * 1024;

/// What the webview needs to open the WebSocket (18 §2 step 5).
#[derive(Clone, Serialize)]
pub struct Connection {
    /// `ws://<listen>/?token=<token>`.
    pub url: String,
    /// Contents of `<data_dir>/token`.
    pub token: String,
    /// The daemon's data directory (attachments are written under it).
    pub data_dir: PathBuf,
    /// True when this app started the daemon and will hang it up on quit.
    pub owned: bool,
}

/// The daemon we started: on Unix the pty master and the shell-turned-daemon
/// child; on Windows the plain `t.exe` child and the Job Object it lives in.
pub struct Daemon {
    #[cfg(unix)]
    master: Box<dyn MasterPty + Send>,
    /// `KILL_ON_JOB_CLOSE`, no breakaway: when this handle closes — `stop()`,
    /// or the kernel closing our handles because the app died by any route —
    /// the daemon, its `t subprocess` and the helper grandchildren go with it.
    /// The pty's SIGHUP on Unix has no Windows equivalent (58 §0.4).
    #[cfg(windows)]
    tree: platform::process::ProcessTree,
    child: Child,
}

#[cfg(unix)]
type Child = Box<dyn portable_pty::Child + Send + Sync>;
#[cfg(windows)]
type Child = std::process::Child;

/// `Some(exit code)` once the daemon has exited.
#[cfg(unix)]
fn exited(child: &mut Child) -> std::io::Result<Option<u32>> {
    Ok(child.try_wait()?.map(|s| s.exit_code()))
}

/// `Some(exit code)` once the daemon has exited (no signals on Windows: always a code).
#[cfg(windows)]
fn exited(child: &mut Child) -> std::io::Result<Option<u32>> {
    Ok(child
        .try_wait()?
        .map(|s| s.code().unwrap_or_default() as u32))
}

#[derive(Default)]
struct Inner {
    connection: Option<Connection>,
    daemon: Option<Daemon>,
    /// The start in flight (222): every `connection` joins it, only a relaunch replaces it.
    attempt: Option<Arc<Attempt>>,
    /// Bumped by every new attempt; a superseded attempt's thread notices and gives up.
    generation: u64,
}

/// Managed state: the established connection, the daemon we own (if any), the
/// start in flight, the pty transcript and the start's progress. `inner` is held
/// only for short reads and writes — a start runs on its own thread and callers
/// wait on its [`Attempt`] — so `peek`, quit and status reads never block on a
/// launch. The log and the progress have their own locks; lock order is `inner`
/// before `progress`.
#[derive(Default)]
pub struct Launch {
    inner: Mutex<Inner>,
    log: Arc<Mutex<Vec<u8>>>,
    progress: Mutex<Shown>,
}

/// The last published progress, plus what a non-forced relaunch refuses on.
#[derive(Default)]
struct Shown {
    payload: StartupProgress,
    /// `Some(label)` while the engine is alive, not ready and moved within [`SOFT_STALL`].
    busy: Option<String>,
}

/// 222: payload of the `startup-progress` event and of the `startup_status` command.
#[derive(Clone, Default, Serialize)]
pub struct StartupProgress {
    /// `startup.json` as the engine wrote it, when it is this start's pid; else `null`.
    pub status: Option<serde_json::Value>,
    /// The engine process we started is still running.
    pub alive: bool,
    /// No `seq` change for [`SOFT_STALL`]; the wait continues.
    pub stalled: bool,
    /// Since the engine was spawned.
    pub waited_ms: u64,
}

/// One start: run by its own thread, joined by every caller that wants a connection.
struct Attempt {
    generation: u64,
    outcome: Mutex<Option<Outcome>>,
    done: Condvar,
}

#[derive(Clone)]
enum Outcome {
    Ready(Connection),
    Failed(String),
    /// A relaunch replaced this attempt; join the current one.
    Superseded,
}

impl Attempt {
    fn wait(&self) -> Outcome {
        let mut outcome = lock(&self.outcome);
        loop {
            if let Some(o) = &*outcome {
                return o.clone();
            }
            outcome = self
                .done
                .wait(outcome)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The first outcome wins (a relaunch marks the old attempt superseded before its thread ends).
    fn finish(&self, outcome: Outcome) {
        let mut slot = lock(&self.outcome);
        if slot.is_none() {
            *slot = Some(outcome);
            self.done.notify_all();
        }
    }
}

impl Launch {
    /// The cached connection, else the start in flight (joined, never restarted),
    /// else a new probe-then-spawn (on the first call and after a failed one).
    pub fn connection(&self, app: &AppHandle) -> anyhow::Result<Connection> {
        let attempt = {
            let mut inner = lock(&self.inner);
            if let Some(conn) = &inner.connection {
                return Ok(conn.clone());
            }
            match &inner.attempt {
                Some(a) => Arc::clone(a),
                None => self.begin(&mut inner, app),
            }
        };
        self.join(attempt, app)
    }

    /// Stop the daemon we own (if any) and connect again. 222: without `force`, an engine that
    /// is alive, not ready and moved within [`SOFT_STALL`] is left alone: the error is
    /// `starting: <label>`. A dead, failed or stalled engine is replaced either way.
    pub fn relaunch(&self, app: &AppHandle, force: bool) -> anyhow::Result<Connection> {
        let attempt = {
            let mut inner = lock(&self.inner);
            if !force
                && inner.attempt.is_some()
                && let Some(label) = &lock(&self.progress).busy
            {
                bail!("starting: {label}");
            }
            if let Some(old) = inner.attempt.take() {
                old.finish(Outcome::Superseded);
            }
            self.begin(&mut inner, app)
        };
        self.join(attempt, app)
    }

    /// 222: the last `startup-progress` payload, for a UI that mounts mid-start.
    pub fn startup_status(&self) -> StartupProgress {
        lock(&self.progress).payload.clone()
    }

    /// The cached connection, else a probe for a live engine (18 §3); never spawns one.
    /// `models.rs` asks before swapping a checkpoint the daemon may have loaded.
    pub fn peek(&self) -> Option<Connection> {
        lock(&self.inner)
            .connection
            .clone()
            .or_else(|| probe(&data_dir()))
    }

    /// 18 §3: hang up the pty; kill the daemon if it is still alive after [`EXIT_GRACE`].
    /// A reused daemon is left running. `RunEvent::Exit`'s fallback: after the quit worker
    /// (163) there is nothing left here and this returns at once.
    pub fn shutdown(&self) {
        if let Some(daemon) = self.take_daemon() {
            daemon.stop(EXIT_GRACE);
        }
    }

    /// The daemon we own, out of the managed state: the quit worker stops it off the main thread (163 D1).
    /// A start in flight then fails ("daemon stopped").
    pub fn take_daemon(&self) -> Option<Daemon> {
        lock(&self.inner).daemon.take()
    }

    /// The pty transcript so far (lossy UTF-8).
    pub fn log(&self) -> String {
        String::from_utf8_lossy(&lock(&self.log)).into_owned()
    }

    /// Wait for `attempt`; a superseded one hands over to whatever is current then.
    fn join(&self, mut attempt: Arc<Attempt>, app: &AppHandle) -> anyhow::Result<Connection> {
        loop {
            match attempt.wait() {
                Outcome::Ready(conn) => return Ok(conn),
                Outcome::Failed(e) => return Err(anyhow!(e)),
                Outcome::Superseded => {
                    let mut inner = lock(&self.inner);
                    if let Some(conn) = &inner.connection {
                        return Ok(conn.clone());
                    }
                    attempt = match &inner.attempt {
                        Some(a) => Arc::clone(a),
                        None => self.begin(&mut inner, app),
                    };
                }
            }
        }
    }

    /// A new attempt on its own thread; the caller holds `inner`.
    fn begin(&self, inner: &mut Inner, app: &AppHandle) -> Arc<Attempt> {
        inner.generation += 1;
        inner.connection = None;
        let attempt = Arc::new(Attempt {
            generation: inner.generation,
            outcome: Mutex::new(None),
            done: Condvar::new(),
        });
        inner.attempt = Some(Arc::clone(&attempt));
        *lock(&self.progress) = Shown::default();
        let spawned = std::thread::Builder::new()
            .name("engine-start".into())
            .spawn({
                let attempt = Arc::clone(&attempt);
                let app = app.clone();
                move || {
                    let launch = app.state::<Launch>();
                    let outcome = match launch.start(attempt.generation, &app) {
                        Ok(Some(conn)) => Outcome::Ready(conn),
                        Ok(None) => Outcome::Superseded,
                        Err(e) => Outcome::Failed(format!("{e:#}")),
                    };
                    launch.settle(&attempt, outcome);
                }
            });
        if let Err(e) = spawned {
            inner.attempt = None;
            attempt.finish(Outcome::Failed(format!("start thread: {e}")));
        }
        attempt
    }

    /// Record the outcome when `attempt` is still current, then wake its joiners.
    fn settle(&self, attempt: &Attempt, outcome: Outcome) {
        let mut inner = lock(&self.inner);
        let outcome = if inner.generation == attempt.generation {
            inner.attempt = None;
            if let Outcome::Ready(conn) = &outcome {
                inner.connection = Some(conn.clone());
            }
            outcome
        } else {
            Outcome::Superseded
        };
        drop(inner);
        attempt.finish(outcome);
    }

    /// Probe, else spawn and [`watch`](Self::watch). `Ok(None)` when a relaunch superseded it.
    fn start(&self, generation: u64, app: &AppHandle) -> anyhow::Result<Option<Connection>> {
        let old = {
            let mut inner = lock(&self.inner);
            if inner.generation != generation {
                return Ok(None);
            }
            inner.daemon.take()
        };
        if let Some(daemon) = old {
            daemon.stop(EXIT_GRACE);
        }
        lock(&self.log).clear();
        let data_dir = data_dir();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("create {}", data_dir.display()))?;
        if let Some(conn) = probe(&data_dir) {
            return Ok(Some(conn));
        }

        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let (daemon, readers) = spawn(&data_dir, port, &self.log)?;
        let pid = daemon.pid();
        {
            let mut inner = lock(&self.inner);
            if inner.generation != generation {
                drop(inner);
                daemon.stop(EXIT_GRACE);
                return Ok(None);
            }
            // Kept either way: on failure the transcript stays inspectable until relaunch.
            inner.daemon = Some(daemon);
        }
        for reader in readers {
            std::thread::spawn({
                let log = Arc::clone(&self.log);
                let app = app.clone();
                move || drain(reader, &log, &app)
            });
        }
        match self.watch(generation, addr, pid, &data_dir, app) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(e) => {
                // 146 D6: the daemon exited 0 because another engine already holds
                // `engine.lock` (a launcher race): attach to that one instead.
                return match probe(&data_dir) {
                    Some(conn) => Ok(Some(conn)),
                    None => Err(e),
                };
            }
        }
        let token = token(&data_dir).context("daemon did not write <data_dir>/token")?;
        Ok(Some(Connection {
            url: ws_url(&addr.to_string(), &token),
            token,
            data_dir,
            owned: true,
        }))
    }

    /// 18 §2 step 5 / 222: every [`TICK`] check the port (ready), the child (an early exit
    /// fails fast) and `startup.json` for this pid, publish `startup-progress`, and let
    /// [`decide`] end the wait. `Ok(false)` when superseded.
    fn watch(
        &self,
        generation: u64,
        addr: SocketAddr,
        pid: Option<u32>,
        data_dir: &Path,
        app: &AppHandle,
    ) -> anyhow::Result<bool> {
        let started = Instant::now();
        let mut tracker = Tracker::new(started);
        loop {
            let tick = Instant::now();
            if TcpStream::connect_timeout(&addr, POLL).is_ok() {
                tracker.observe(read_status(data_dir, pid), Instant::now());
                self.publish(generation, app, tracker.payload(started, true, false), None);
                return Ok(true);
            }
            let exit = {
                let mut inner = lock(&self.inner);
                if inner.generation != generation {
                    return Ok(false);
                }
                let daemon = inner.daemon.as_mut().context("daemon stopped")?;
                exited(&mut daemon.child)?
            };
            let now = Instant::now();
            tracker.observe(read_status(data_dir, pid), now);
            let verdict = decide(&tracker.tick(started, now, exit.is_none()));
            let stalled = matches!(verdict, Verdict::Wait { stalled: true });
            let busy = match (&verdict, tracker.status()) {
                (Verdict::Wait { stalled: false }, Some(s)) if s.state == "running" => {
                    Some(s.describe())
                }
                _ => None,
            };
            self.publish(
                generation,
                app,
                tracker.payload(started, exit.is_none(), stalled),
                busy,
            );
            let label = tracker
                .status()
                .map_or_else(|| "engine start".to_owned(), StartupStatus::describe);
            match verdict {
                Verdict::Wait { .. } => {}
                Verdict::Failed(error) => bail!("engine startup failed at {label}: {error}"),
                Verdict::Exited => bail!(
                    "daemon exited before listening (exit code {})",
                    exit.unwrap_or_default()
                ),
                Verdict::Stalled => bail!(
                    "engine stalled at {label}: no heartbeat for {} min",
                    HARD_STALL.as_secs() / 60
                ),
                Verdict::TimedOut => bail!(self.timeout_message(addr)),
            }
            std::thread::sleep(TICK.saturating_sub(tick.elapsed()));
        }
    }

    /// The pre-222 timeout (an engine without `startup.json`): names the likely cause (Unix: a
    /// shell startup stuck on a prompt) and quotes the transcript's tail.
    fn timeout_message(&self, addr: SocketAddr) -> String {
        let text = strip_controls(&lock(&self.log));
        let mut msg = format!("no daemon on {addr} after {} s", FALLBACK_CAP.as_secs());
        if cfg!(unix) && waiting_for_input(&text) {
            msg.push_str(": your shell startup is waiting for input");
        }
        let tail = last_lines(&text, 5);
        if !tail.is_empty() {
            msg.push('\n');
            msg.push_str(&tail);
        }
        msg
    }

    /// Emit `startup-progress` and keep it for `startup_status`, unless a relaunch superseded
    /// `generation` (its reset must not be overwritten by the old start).
    fn publish(
        &self,
        generation: u64,
        app: &AppHandle,
        payload: StartupProgress,
        busy: Option<String>,
    ) {
        {
            let inner = lock(&self.inner);
            if inner.generation != generation {
                return;
            }
            *lock(&self.progress) = Shown {
                payload: payload.clone(),
                busy,
            };
        }
        if let Err(e) = app.emit(PROGRESS_EVENT, &payload) {
            tracing::warn!(error = %e, "startup-progress emit");
        }
    }
}

/// 222: the fields of `startup.json` the shell decides on; unknown fields are ignored and
/// missing ones default, the event forwards the raw JSON.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct StartupStatus {
    pid: u32,
    seq: u64,
    hb_ms: u64,
    /// `running` | `ready` | `failed`.
    state: String,
    phase: String,
    label: String,
    step: Option<Step>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Step {
    n: u64,
    of: u64,
}

impl StartupStatus {
    /// `Migrating agent.db (3/78)`: the label (else the phase id) and the step when its total is known.
    fn describe(&self) -> String {
        let base = [&self.label, &self.phase]
            .into_iter()
            .find(|s| !s.trim().is_empty())
            .map_or("engine start", |s| s.as_str());
        match &self.step {
            Some(Step { n, of }) if *of > 0 => format!("{base} ({n}/{of})"),
            _ => base.to_owned(),
        }
    }
}

/// `startup.json` parsed, with the raw JSON kept for the event; `None` when unreadable.
fn parse_status(bytes: &[u8]) -> Option<(StartupStatus, serde_json::Value)> {
    let raw: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let status = StartupStatus::deserialize(&raw).ok()?;
    Some((status, raw))
}

/// `<data_dir>/startup.json` when it is `pid`'s (a file from an earlier boot is not this start's).
fn read_status(data_dir: &Path, pid: Option<u32>) -> Option<(StartupStatus, serde_json::Value)> {
    let pid = pid?;
    let bytes = std::fs::read(data_dir.join(STARTUP_FILE)).ok()?;
    parse_status(&bytes).filter(|(s, _)| s.pid == pid)
}

/// This start's view of `startup.json` over time: the last record seen and when `seq` and the
/// heartbeat last moved (by our clock, so the engine's wall clock and a sleeping laptop do not count).
struct Tracker {
    seen: Option<(StartupStatus, serde_json::Value)>,
    seq_at: Instant,
    hb_at: Instant,
}

impl Tracker {
    fn new(now: Instant) -> Self {
        Self {
            seen: None,
            seq_at: now,
            hb_at: now,
        }
    }

    /// A missing or unreadable file keeps the last record seen.
    fn observe(&mut self, next: Option<(StartupStatus, serde_json::Value)>, now: Instant) {
        let Some(next) = next else { return };
        match &self.seen {
            Some((prev, _)) if prev.seq == next.0.seq => {
                if prev.hb_ms != next.0.hb_ms {
                    self.hb_at = now;
                }
            }
            // First sight or progress: both clocks restart (progress is liveness too).
            _ => {
                self.seq_at = now;
                self.hb_at = now;
            }
        }
        self.seen = Some(next);
    }

    fn status(&self) -> Option<&StartupStatus> {
        self.seen.as_ref().map(|(s, _)| s)
    }

    fn tick(&self, started: Instant, now: Instant, alive: bool) -> Tick<'_> {
        Tick {
            waited: now.duration_since(started),
            alive,
            status: self.status(),
            since_seq: now.duration_since(self.seq_at),
            since_hb: now.duration_since(self.hb_at),
        }
    }

    fn payload(&self, started: Instant, alive: bool, stalled: bool) -> StartupProgress {
        StartupProgress {
            status: self.seen.as_ref().map(|(_, raw)| raw.clone()),
            alive,
            stalled,
            waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// What [`decide`] sees on one tick of the wait (the port did not answer).
struct Tick<'a> {
    /// Since the spawn.
    waited: Duration,
    /// The child has not exited.
    alive: bool,
    /// The last `startup.json` seen for this pid.
    status: Option<&'a StartupStatus>,
    /// Since `seq` last changed (or the record was first seen).
    since_seq: Duration,
    /// Since the heartbeat (or `seq`) last changed.
    since_hb: Duration,
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Wait {
        stalled: bool,
    },
    /// `startup.json` says `failed`, with its `error`.
    Failed(String),
    Exited,
    /// No heartbeat for [`HARD_STALL`].
    Stalled,
    /// No `startup.json` (older engine) and no port within [`FALLBACK_CAP`].
    TimedOut,
}

/// 222: the wait's decision, port aside. An engine that writes `startup.json` gets no cap while
/// its heartbeat beats; one that does not (older than 222) gets the pre-222 port wait, capped at
/// [`FALLBACK_CAP`]. A record that shows up late, inside that cap, still switches to the first.
fn decide(t: &Tick<'_>) -> Verdict {
    if let Some(s) = t.status
        && s.state == "failed"
    {
        return Verdict::Failed(s.error.clone().unwrap_or_else(|| "unknown error".into()));
    }
    if !t.alive {
        return Verdict::Exited;
    }
    match t.status {
        Some(_) if t.since_hb >= HARD_STALL => Verdict::Stalled,
        Some(_) => Verdict::Wait {
            stalled: t.since_seq >= SOFT_STALL,
        },
        None if t.waited >= FALLBACK_CAP => Verdict::TimedOut,
        None => Verdict::Wait { stalled: false },
    }
}

/// `$THEUTH_DATA_DIR`, else `~/.theuth` (the same directory the terminal uses, 25 §5).
pub fn data_dir() -> PathBuf {
    platform::env::data_dir()
}

/// 18 §3 / 63 §2.4: a live engine is reused, not duplicated. The engine
/// record (`engine.json`: pid alive, `listen` set) says where it listens; the
/// configured `rpc.listen` is the fallback for records written by older engines.
fn probe(data_dir: &Path) -> Option<Connection> {
    let token = token(data_dir)?;
    let listen = recorded_listen(data_dir).unwrap_or_else(|| configured_listen(data_dir));
    let addr = listen.to_socket_addrs().ok()?.next()?;
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).ok()?;
    Some(Connection {
        url: ws_url(&listen, &token),
        token,
        data_dir: data_dir.to_owned(),
        owned: false,
    })
}

/// `listen` of `<data_dir>/engine.json` when its pid is alive (63 §2.1). The
/// desktop does not link the harness, so the two fields are read here.
fn recorded_listen(data_dir: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Record {
        pid: u32,
        listen: Option<String>,
    }
    let bytes = std::fs::read(data_dir.join("engine.json")).ok()?;
    let r: Record = serde_json::from_slice(&bytes).ok()?;
    // A dead engine leaves a stale record (63 §2.1); `platform` answers per OS.
    platform::process::pid_alive(r.pid)
        .then_some(r.listen)
        .flatten()
}

fn configured_listen(data_dir: &Path) -> String {
    std::fs::read_to_string(data_dir.join("config.yaml"))
        .ok()
        .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(&s).ok())
        .and_then(|v| v.get("rpc")?.get("listen")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_LISTEN.to_owned())
}

fn token(data_dir: &Path) -> Option<String> {
    let t = std::fs::read_to_string(data_dir.join("token")).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn ws_url(listen: &str, token: &str) -> String {
    format!("ws://{listen}/?token={token}")
}

/// Readers the drain threads pull the transcript from.
type Readers = Vec<Box<dyn Read + Send>>;

/// Open the pty and start the shell; the returned reader is the master's dup for
/// the drain thread.
#[cfg(unix)]
fn spawn(data_dir: &Path, port: u16, log: &Mutex<Vec<u8>>) -> anyhow::Result<(Daemon, Readers)> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let (program, args) = daemon_command(data_dir, log);
    let child = pair
        .slave
        .spawn_command(command(&program, &args, port))
        .with_context(|| format!("spawn {}", program.display()))?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader()?;
    Ok((
        Daemon {
            master: pair.master,
            child,
        },
        vec![reader],
    ))
}

/// Start `t.exe` as a hidden plain child: no pty (a ConPTY shows a console
/// window and its conhost handshake never got the daemon listening on a real
/// install), stdin null, stdout and stderr piped into the ring. The child is
/// put in a kill-on-close Job Object and told our pid (`THEUTH_PARENT_PID`) so
/// it can also notice on its own that we are gone.
#[cfg(windows)]
fn spawn(data_dir: &Path, port: u16, log: &Mutex<Vec<u8>>) -> anyhow::Result<(Daemon, Readers)> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    /// `CREATE_NO_WINDOW`: no console for a subsystem:console child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (program, args) = daemon_command(data_dir, log);
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .envs(env(port))
        .env("THEUTH_PARENT_PID", std::process::id().to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", program.display()))?;
    // A daemon we cannot tie to our lifetime is the leak this exists to stop:
    // refuse it rather than leave it behind.
    let tree = match platform::process::ProcessTree::adopt(child.id()) {
        Ok(t) => t,
        Err(e) => {
            let _ = child.kill();
            return Err(e).context("job object for the daemon");
        }
    };
    let readers: Readers = vec![
        Box::new(child.stdout.take().context("stdout not piped")?),
        Box::new(child.stderr.take().context("stderr not piped")?),
    ];
    Ok((Daemon { tree, child }, readers))
}

/// The daemon's command line (18 §2 step 2): on Unix the user's login,
/// interactive shell running `exec t --headless …` so the daemon inherits a
/// terminal tab's environment; on Windows, which has no login-shell
/// environment, `t.exe --headless …` itself (58 §0.3).
fn daemon_command(data_dir: &Path, log: &Mutex<Vec<u8>>) -> (PathBuf, Vec<String>) {
    let bin = cli_bin();
    if cfg!(windows) {
        let args = ["--headless", "--data-dir", &data_dir.to_string_lossy()].map(str::to_owned);
        return (bin, args.to_vec());
    }
    let script = format!(
        "exec {} --headless --data-dir {}",
        platform::shell::single_quote(&bin.to_string_lossy()),
        platform::shell::single_quote(&data_dir.to_string_lossy())
    );
    (
        shell(log),
        ["-l", "-i", "-c", &script].map(str::to_owned).to_vec(),
    )
}

/// The app's environment plus `TERM`, the `THEUTH_LAUNCH=1` guard (18 §4) and
/// the port override (01 §1).
fn env(port: u16) -> [(&'static str, String); 3] {
    [
        ("TERM", "xterm-256color".to_owned()),
        ("THEUTH_LAUNCH", "1".to_owned()),
        ("THEUTH_RPC__LISTEN", format!("127.0.0.1:{port}")),
    ]
}

/// `program args` on the pty with [`env`]; cwd is `$HOME`, as for a new terminal tab.
#[cfg(unix)]
fn command(program: &Path, args: &[String], port: u16) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    for (k, v) in env(port) {
        cmd.env(k, v);
    }
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }
    cmd
}

/// The login shell when `$SHELL` is unset: zsh on macOS, `sh` elsewhere.
const FALLBACK_SHELL: &str = if cfg!(target_os = "macos") {
    "/bin/zsh"
} else {
    "/bin/sh"
};

/// `$SHELL` when it exists, else [`FALLBACK_SHELL`] — said in the transcript (18 §4).
fn shell(log: &Mutex<Vec<u8>>) -> PathBuf {
    match std::env::var_os("SHELL").map(PathBuf::from) {
        Some(s) if s.is_file() => s,
        _ => {
            append(
                log,
                format!("SHELL unset or not executable; using {FALLBACK_SHELL}\r\n").as_bytes(),
            );
            PathBuf::from(FALLBACK_SHELL)
        }
    }
}

/// `$THEUTH_BIN`, else the bundled sidecar beside this executable (25 §5), else
/// `t` on the shell's PATH.
fn cli_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("THEUTH_BIN") {
        return PathBuf::from(p);
    }
    let t = format!("t{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(&t))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(t))
}

/// Master → ring + `launch-log` events until EOF/EIO (the daemon exited).
fn drain(mut reader: Box<dyn Read + Send>, log: &Mutex<Vec<u8>>, app: &AppHandle) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                append(log, &buf[..n]);
                if let Err(e) = app.emit("launch-log", String::from_utf8_lossy(&buf[..n])) {
                    tracing::warn!(error = %e, "launch-log emit");
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn append(log: &Mutex<Vec<u8>>, chunk: &[u8]) {
    let mut log = lock(log);
    log.extend_from_slice(chunk);
    if log.len() > LOG_CAP {
        let excess = log.len() - LOG_CAP;
        log.drain(..excess);
    }
}

/// The transcript without terminal control sequences (CSI, OSC/DCS/PM/APC and
/// two-byte escapes) and stray C0 bytes: conhost's pseudo-console handshake
/// (`\x1b[?9001h`, `\x1b]0;…\x07`, `\x1b[6n`) is not something a shell said.
fn strip_controls(bytes: &[u8]) -> String {
    let mut text =
        String::from_utf8_lossy(&anstream::adapter::strip_bytes(bytes).into_vec()).into_owned();
    text.retain(|c| c >= ' ' || matches!(c, '\n' | '\r' | '\t'));
    text
}

/// A prompt left hanging: the stripped transcript ends in a partial line
/// (`Password: `, `Continue? (y/n) `) — a daemon or shell that finished its
/// output ended it with a newline.
fn waiting_for_input(text: &str) -> bool {
    !text
        .rsplit(['\n', '\r'])
        .next()
        .unwrap_or("")
        .trim()
        .is_empty()
}

/// The last `n` non-blank lines, newline-joined.
fn last_lines(text: &str, n: usize) -> String {
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(n)
        .collect();
    lines.reverse();
    lines.join("\n")
}

impl Daemon {
    /// Hang up (Unix: drop the master, SIGHUP once every fd is closed; Windows has
    /// no line to hang up — stdin is already null), give the daemon `grace` to exit
    /// on its own (the webview called RPC `shutdown` first), then kill. On Windows the
    /// kill is the whole Job Object, and dropping its handle afterwards kills
    /// whatever a clean daemon exit left behind. `true` when it had to kill.
    pub fn stop(self, grace: Duration) -> bool {
        let Daemon {
            #[cfg(unix)]
            master,
            #[cfg(windows)]
            tree,
            mut child,
        } = self;
        #[cfg(unix)]
        drop(master);
        let deadline = Instant::now() + grace;
        loop {
            if !matches!(exited(&mut child), Ok(None)) {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(EXIT_POLL.min(deadline - now));
        }
        #[cfg(windows)]
        tree.kill();
        #[cfg(unix)]
        if let Err(e) = child.kill() {
            tracing::warn!(error = %e, "kill daemon");
        }
        true
    }

    /// The daemon's process tree, so another thread can kill it while [`Daemon::stop`] waits
    /// (the quit cap, 163 D2). Unix: the pty child's session (portable-pty `setsid`s it, so the
    /// pgid is its pid); Windows: the Job Object.
    pub fn tree(&self) -> Option<platform::process::ProcessTree> {
        #[cfg(unix)]
        return self
            .child
            .process_id()
            .and_then(|pid| platform::process::ProcessTree::adopt(pid).ok());
        #[cfg(windows)]
        return Some(self.tree.clone());
    }

    /// The engine's pid (Unix: the shell `exec`s into it, so the pty child's pid is the engine's):
    /// `startup.json` with any other pid is an earlier boot's.
    fn pid(&self) -> Option<u32> {
        #[cfg(unix)]
        return self.child.process_id();
        #[cfg(windows)]
        return Some(self.child.id());
    }
}

/// Stand-in daemons for the quit tests: `program args` spawned the way [`spawn`] does it.
#[cfg(test)]
impl Daemon {
    /// Returns once the child has written its first byte, so a hang-up never races its startup (a `trap`
    /// not yet installed): the command must print something before it settles.
    #[cfg(unix)]
    pub fn fake(program: &str, args: &[&str]) -> Daemon {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("openpty");
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        let child = pair.slave.spawn_command(cmd).expect("spawn fake daemon");
        drop(pair.slave);
        let mut first = [0u8; 1];
        pair.master
            .try_clone_reader()
            .expect("pty reader")
            .read_exact(&mut first)
            .expect("fake daemon's first output");
        Daemon {
            master: pair.master,
            child,
        }
    }

    #[cfg(windows)]
    pub fn fake(program: &str, args: &[&str]) -> Daemon {
        use std::process::{Command, Stdio};
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn fake daemon");
        let tree = platform::process::ProcessTree::adopt(child.id()).expect("job object");
        Daemon { tree, child }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 18 §7.1: the login+interactive shell on a pty passes our variables through.
    #[cfg(unix)]
    #[test]
    fn shell_on_pty_carries_env() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let shell = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|s| s.is_file())
            .unwrap_or_else(|| PathBuf::from(FALLBACK_SHELL));
        let mut child = pair
            .slave
            .spawn_command(command(
                &shell,
                &["-l", "-i", "-c", "exec env"].map(str::to_owned),
                7802,
            ))
            .unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        drop(pair.master);
        let mut out = Vec::new();
        let _ = reader.read_to_end(&mut out);
        child.wait().unwrap();
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("TERM=xterm-256color"), "{out}");
        assert!(out.contains("THEUTH_LAUNCH=1"), "{out}");
        assert!(out.contains("THEUTH_RPC__LISTEN=127.0.0.1:7802"), "{out}");
    }

    #[test]
    fn single_quote_escapes() {
        assert_eq!(platform::shell::single_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn ring_is_capped() {
        let log = Mutex::new(Vec::new());
        append(&log, &[b'a'; LOG_CAP]);
        append(&log, b"tail");
        let log = lock(&log);
        assert_eq!(log.len(), LOG_CAP);
        assert!(log.ends_with(b"tail"));
    }

    /// The Windows 11 box, 2026-09-23: conhost's pseudo-console handshake on the
    /// pty read as "waiting for input". Nothing in it is a prompt.
    const CONHOST: &[u8] = b"\x1b[?9001h\x1b[?1004h\x1b[?25h\x1b]0;C:\\Users\\user\\AppData\\Local\\theuth\\t.exe\x07\x1b[6n";

    #[test]
    fn conhost_handshake_is_not_a_prompt() {
        assert_eq!(strip_controls(CONHOST), "");
        assert!(!waiting_for_input(&strip_controls(CONHOST)));
        // An OSC the stream never terminated swallows the rest: still no prompt.
        assert!(!waiting_for_input(&strip_controls(
            b"\x1b]0;C:\\t.exe \x1b[6n"
        )));
        assert!(!waiting_for_input(&strip_controls(b"loaded 3 skills\r\n")));
        assert!(!waiting_for_input(""));
    }

    #[test]
    fn real_prompts_are() {
        assert!(waiting_for_input(&strip_controls(
            b"Last login: Tue\r\nPassword: "
        )));
        assert!(waiting_for_input(&strip_controls(
            b"\x1b[1mUpdate zsh plugins? (y/n) \x1b[0m"
        )));
    }

    #[test]
    fn strip_keeps_text_between_sequences() {
        assert_eq!(
            strip_controls(b"\x1b(Ba\x1b[31mb\x1b[0m\x07c\x1bP..\x1b\\d"),
            "abcd"
        );
        assert_eq!(last_lines("a\r\n\r\nb\nc\n", 2), "b\nc");
    }

    const RECORD: &[u8] = br#"{"v":1,"pid":123,"started_ms":1764160000000,"seq":42,"hb_ms":1764160003000,
        "state":"running","phase":"migrate:agent","label":"Migrating agent.db","step":{"n":3,"of":78},
        "done":[{"phase":"config","label":"Reading config","ms":12}],"error":null,"later":{"x":1}}"#;

    #[test]
    fn startup_record_parses_and_keeps_raw() {
        let (s, raw) = parse_status(RECORD).expect("parses");
        assert_eq!((s.pid, s.seq, s.state.as_str()), (123, 42, "running"));
        assert_eq!(s.step, Some(Step { n: 3, of: 78 }));
        assert_eq!(s.describe(), "Migrating agent.db (3/78)");
        // The event forwards what the engine wrote, unknown fields included.
        assert_eq!(raw["done"][0]["ms"], 12);
        assert_eq!(raw["later"]["x"], 1);
    }

    #[test]
    fn startup_record_tolerates_missing_fields() {
        let (s, _) = parse_status(
            br#"{"pid":7,"state":"failed","error":"db locked","phase":"migrate:mesh"}"#,
        )
        .expect("parses");
        assert_eq!((s.seq, s.step.clone()), (0, None));
        assert_eq!(s.error.as_deref(), Some("db locked"));
        // No label: the phase id; a step without a total shows no count.
        let s = StartupStatus {
            step: Some(Step { n: 4, of: 0 }),
            ..s
        };
        assert_eq!(s.describe(), "migrate:mesh");
        assert_eq!(StartupStatus::default().describe(), "engine start");
        assert!(parse_status(b"{\"pid\":").is_none());
        assert!(parse_status(br#"{"pid":"seven"}"#).is_none());
    }

    fn running() -> StartupStatus {
        parse_status(RECORD).unwrap().0
    }

    fn tick(
        waited: u64,
        alive: bool,
        status: Option<&StartupStatus>,
        since_seq: u64,
        since_hb: u64,
    ) -> Tick<'_> {
        Tick {
            waited: Duration::from_secs(waited),
            alive,
            status,
            since_seq: Duration::from_secs(since_seq),
            since_hb: Duration::from_secs(since_hb),
        }
    }

    #[test]
    fn progressing_engine_has_no_cap() {
        let s = running();
        let wait = Verdict::Wait { stalled: false };
        assert_eq!(decide(&tick(15, true, Some(&s), 1, 1)), wait);
        assert_eq!(decide(&tick(3600, true, Some(&s), 89, 1)), wait);
    }

    #[test]
    fn quiet_engine_is_stalled_then_failed() {
        let s = running();
        // Heartbeat but no progress for 90 s: flagged, still waiting.
        assert_eq!(
            decide(&tick(200, true, Some(&s), 90, 1)),
            Verdict::Wait { stalled: true }
        );
        // No heartbeat for 10 min: give up.
        assert_eq!(
            decide(&tick(700, true, Some(&s), 600, 599)),
            Verdict::Wait { stalled: true }
        );
        assert_eq!(
            decide(&tick(700, true, Some(&s), 600, 600)),
            Verdict::Stalled
        );
    }

    #[test]
    fn failure_and_exit_end_the_wait() {
        let failed = StartupStatus {
            state: "failed".into(),
            error: Some("migration 12 failed".into()),
            ..running()
        };
        // `failed` wins over the exit it usually comes with: it says why.
        assert_eq!(
            decide(&tick(5, false, Some(&failed), 0, 0)),
            Verdict::Failed("migration 12 failed".into())
        );
        assert_eq!(
            decide(&tick(5, false, Some(&running()), 0, 0)),
            Verdict::Exited
        );
        assert_eq!(decide(&tick(5, false, None, 0, 0)), Verdict::Exited);
    }

    #[test]
    fn engine_without_record_gets_the_old_capped_wait() {
        let wait = Verdict::Wait { stalled: false };
        assert_eq!(decide(&tick(20, true, None, 20, 20)), wait);
        assert_eq!(decide(&tick(59, true, None, 59, 59)), wait);
        assert_eq!(decide(&tick(60, true, None, 60, 60)), Verdict::TimedOut);
    }

    #[test]
    fn tracker_clocks_follow_seq_and_heartbeat() {
        let t0 = Instant::now();
        let mut tracker = Tracker::new(t0);
        let at = |s: u64| t0 + Duration::from_secs(s);
        let record = |seq: u64, hb_ms: u64| {
            let s = StartupStatus {
                seq,
                hb_ms,
                ..running()
            };
            Some((s, serde_json::Value::Null))
        };
        tracker.observe(record(1, 1000), at(10));
        // Heartbeat only: liveness moves, progress does not.
        tracker.observe(record(1, 2000), at(100));
        let t = tracker.tick(t0, at(110), true);
        assert_eq!(
            (t.since_seq, t.since_hb),
            (Duration::from_secs(100), Duration::from_secs(10))
        );
        assert_eq!(decide(&t), Verdict::Wait { stalled: true });
        // A missing file keeps the last record; progress restarts both clocks.
        tracker.observe(None, at(120));
        tracker.observe(record(2, 2000), at(130));
        let t = tracker.tick(t0, at(131), true);
        assert_eq!(
            (t.since_seq, t.since_hb),
            (Duration::from_secs(1), Duration::from_secs(1))
        );
    }
}
