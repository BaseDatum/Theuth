//! Spawn under ProcessTree, two-lane writer, stdout reader, backoff, `.pgids` reaper (design §5, §6.1,
//! §6.2, §6.6, D15).

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use platform::process::{ProcessTree, Signal};
use serde_json::Value;
use std::time::Instant;
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader, BufWriter,
};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::config::PluginsConfig;
use crate::manifest::RestartPolicy;

/// Stray stdout lines tolerated within [`STRAY_WINDOW`] (design §6.1).
const STRAY_MAX: usize = 100;
/// Window of the stray-line count.
const STRAY_WINDOW: Duration = Duration::from_secs(10);
/// After spawn, runtimes print banners: stray lines are not counted this long.
const BANNER: Duration = Duration::from_secs(5);
/// Running this long resets the restart delay to `restart_first_ms` (design §5).
const RESET_AFTER: Duration = Duration::from_secs(60);
/// Bytes of a discarded long line kept to find its `"id"` (design §6.1).
const ID_PREFIX: usize = 4096;
/// Poll interval of [`stop_tree`].
const POLL: Duration = Duration::from_millis(50);
/// Control-lane ceiling in lines: the lane holds what the host itself waits on, so a plugin that
/// keeps asking without reading stdin overflows it and is treated as crashed (design §6.2).
const CONTROL_MAX_LINES: usize = 4096;
/// Control-lane ceiling in bytes.
const CONTROL_MAX_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// Spawn

/// What to run: argv already expanded, the complete environment (the child's env is cleared first).
#[derive(Clone, Debug)]
pub(crate) struct SpawnSpec {
    /// Program and arguments, expanded.
    pub argv: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// The whole environment of the child.
    pub env: BTreeMap<String, String>,
}

/// A started plugin process with its stdio taken.
#[derive(Debug)]
pub(crate) struct Spawned {
    /// The leader (`kill_on_drop`).
    pub child: Child,
    /// Its process group / Job Object.
    pub tree: Option<ProcessTree>,
    /// Unix process group id (recorded in `.pgids`); `None` elsewhere.
    pub pgid: Option<u32>,
    /// Host → plugin.
    pub stdin: ChildStdin,
    /// Plugin → host messages.
    pub stdout: ChildStdout,
    /// The plugin's log channel.
    pub stderr: ChildStderr,
}

/// Spawn `spec` in its own session/group, adopt its tree, piped stdio, `kill_on_drop`.
pub(crate) fn spawn(spec: &SpawnSpec) -> io::Result<Spawned> {
    let (program, args) = spec
        .argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty run command"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    platform::process::own_session(&mut cmd);
    platform::process::hide_console(cmd.as_std_mut());
    let mut child = cmd.spawn()?;
    let tree = child.id().and_then(|pid| ProcessTree::adopt(pid).ok());
    let pgid = if cfg!(unix) {
        tree.as_ref().map(ProcessTree::id)
    } else {
        None
    };
    let missing = |what: &str| io::Error::other(format!("child {what} not piped"));
    let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok(Spawned {
        child,
        tree,
        pgid,
        stdin,
        stdout,
        stderr,
    })
}

// ---------------------------------------------------------------------------------------------
// Outbox (D15): shutdown ahead of both lanes, `hello` ahead of everything else, control lane
// lossless up to a ceiling, event lane bounded drop-oldest.

/// Host → plugin outbox (D15). Clones share the same queues.
#[derive(Clone)]
pub(crate) struct Outbox(Arc<OutboxInner>);

struct OutboxInner {
    lanes: Mutex<Lanes>,
    wake: Notify,
    /// Cancelled when the control lane went over its ceiling.
    overflow: CancellationToken,
}

struct Lanes {
    cap: usize,
    /// `hello`: written before anything else (a pending `lagged` included).
    first: Option<Vec<u8>>,
    /// Encoded lines, newline included.
    control: VecDeque<Vec<u8>>,
    control_bytes: usize,
    events: VecDeque<Value>,
    /// Events lost since the last `lagged` was written; > 0 = one `lagged` pending.
    dropped: u64,
    /// Wall-clock ms of the first loss of the pending burst.
    since_ms: u64,
    shutdown: Option<Value>,
    /// No more lines are accepted.
    ended: bool,
}

impl Lanes {
    fn note_drop(&mut self, n: u64) {
        if n == 0 {
            return;
        }
        if self.dropped == 0 {
            self.since_ms = now_ms();
        }
        self.dropped = self.dropped.saturating_add(n);
    }

    fn end(&mut self) {
        self.ended = true;
        self.first = None;
        self.control.clear();
        self.control_bytes = 0;
        self.events.clear();
        self.dropped = 0;
    }
}

fn encode(v: &Value) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(v).ok()?;
    line.push(b'\n');
    Some(line)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Outbox {
    /// An empty outbox whose event lane holds `queue_frames`.
    pub(crate) fn new(queue_frames: usize) -> Self {
        Self(Arc::new(OutboxInner {
            lanes: Mutex::new(Lanes {
                cap: queue_frames,
                first: None,
                control: VecDeque::new(),
                control_bytes: 0,
                events: VecDeque::new(),
                dropped: 0,
                since_ms: 0,
                shutdown: None,
                ended: false,
            }),
            wake: Notify::new(),
            overflow: CancellationToken::new(),
        }))
    }

    fn lanes(&self) -> MutexGuard<'_, Lanes> {
        self.0.lanes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The line written before every other one (`hello`, design §6.2); queue it before the
    /// outbox is shared.
    pub(crate) fn first(&self, msg: Value) {
        let Some(line) = encode(&msg) else { return };
        {
            let mut l = self.lanes();
            if l.ended {
                return;
            }
            l.first = Some(line);
        }
        self.0.wake.notify_one();
    }

    /// Queue on the control lane: written before events, never dropped. Over
    /// [`CONTROL_MAX_LINES`] / [`CONTROL_MAX_BYTES`] the outbox ends and [`Outbox::overflowed`]
    /// fires: the supervisor treats that as a crash.
    pub(crate) fn control(&self, msg: Value) {
        let Some(line) = encode(&msg) else { return };
        {
            let mut l = self.lanes();
            if l.ended {
                return;
            }
            if l.control.len() >= CONTROL_MAX_LINES
                || l.control_bytes + line.len() > CONTROL_MAX_BYTES
            {
                l.end();
                drop(l);
                self.0.overflow.cancel();
                self.0.wake.notify_one();
                return;
            }
            l.control_bytes += line.len();
            l.control.push_back(line);
        }
        self.0.wake.notify_one();
    }

    /// Cancelled once the control lane went over its ceiling.
    pub(crate) fn overflowed(&self) -> CancellationToken {
        self.0.overflow.clone()
    }

    /// Queue on the event lane; when full the oldest event is dropped and counted.
    pub(crate) fn event(&self, msg: Value) {
        {
            let mut l = self.lanes();
            if l.ended {
                return;
            }
            if l.cap == 0 {
                l.note_drop(1);
            } else {
                if l.events.len() >= l.cap {
                    l.events.pop_front();
                    l.note_drop(1);
                }
                l.events.push_back(msg);
            }
        }
        self.0.wake.notify_one();
    }

    /// Feed-level loss (the host's broadcast receiver lagged): add `n` to the pending `lagged`.
    pub(crate) fn lagged(&self, n: u64) {
        {
            let mut l = self.lanes();
            if l.ended || n == 0 {
                return;
            }
            l.note_drop(n);
        }
        self.0.wake.notify_one();
    }

    /// Write `msg` ahead of both lanes, drop everything still queued, then end the writer
    /// (stdin closes). Ignored once the outbox has ended.
    pub(crate) fn shutdown(&self, msg: Value) {
        {
            let mut l = self.lanes();
            if l.ended {
                return;
            }
            l.end();
            l.shutdown = Some(msg);
        }
        self.0.wake.notify_one();
    }

    /// End the writer without a shutdown line; queued lines are dropped.
    pub(crate) fn close(&self) {
        self.lanes().end();
        self.0.wake.notify_one();
    }
}

enum Take {
    Line(Value),
    Raw(Vec<u8>),
    End(Option<Value>),
    Wait,
}

impl Outbox {
    fn take(&self) -> Take {
        let mut l = self.lanes();
        if let Some(m) = l.shutdown.take() {
            return Take::End(Some(m));
        }
        if l.ended {
            return Take::End(None);
        }
        if let Some(line) = l.first.take() {
            return Take::Raw(line);
        }
        if l.dropped > 0 {
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "lagged",
                "params": {"dropped": l.dropped, "since_ms": l.since_ms},
            });
            l.dropped = 0;
            return Take::Line(msg);
        }
        if let Some(line) = l.control.pop_front() {
            l.control_bytes -= line.len();
            return Take::Raw(line);
        }
        match l.events.pop_front() {
            Some(m) => Take::Line(m),
            None => Take::Wait,
        }
    }
}

/// Write one JSON line per message: `hello` first, then a pending `lagged`, then control, then
/// events; `shutdown` ahead of all. Ends after `shutdown`/`close` (flushing and shutting `w`
/// down) or on a write error; either way the outbox accepts nothing more.
pub(crate) async fn write_loop<W: AsyncWrite + Unpin>(outbox: Outbox, w: W) -> io::Result<()> {
    let res = pump(&outbox, BufWriter::new(w)).await;
    outbox.lanes().end();
    res
}

async fn pump<W: AsyncWrite + Unpin>(outbox: &Outbox, mut w: BufWriter<W>) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        match outbox.take() {
            Take::Line(v) => write_line(&mut w, &mut line, &v).await?,
            Take::Raw(bytes) => w.write_all(&bytes).await?,
            Take::End(last) => {
                if let Some(v) = last {
                    write_line(&mut w, &mut line, &v).await?;
                }
                w.flush().await?;
                return w.shutdown().await;
            }
            Take::Wait => {
                w.flush().await?;
                outbox.0.wake.notified().await;
            }
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(
    w: &mut W,
    line: &mut Vec<u8>,
    v: &Value,
) -> io::Result<()> {
    line.clear();
    serde_json::to_writer(&mut *line, v).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(line).await
}

// ---------------------------------------------------------------------------------------------
// Reader (§6.1)

/// One stdout line, or the size (and request id, if any) of a discarded over-long one.
#[derive(Debug, PartialEq)]
pub(crate) enum ReadItem {
    /// A line without its `\n`.
    Line(Vec<u8>),
    /// A line over `max_line_bytes`, discarded; `id` from its first bytes when they carried one.
    TooLong {
        /// Length of the discarded line, newline excluded.
        bytes: usize,
        /// The request id found in the line's prefix.
        id: Option<Value>,
    },
}

/// Newline-framed reader that discards an over-long line to its newline without buffering it.
pub(crate) struct LineReader<R> {
    r: BufReader<R>,
    max: usize,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    /// Read `r`, lines longer than `max_line_bytes` become [`ReadItem::TooLong`].
    pub(crate) fn new(r: R, max_line_bytes: usize) -> Self {
        Self {
            r: BufReader::with_capacity(64 * 1024, r),
            max: max_line_bytes,
            buf: Vec::new(),
        }
    }

    /// The next line; `None` at EOF. A last line without `\n` is still returned.
    pub(crate) async fn next(&mut self) -> io::Result<Option<ReadItem>> {
        self.buf.clear();
        // Some(total) once the line went over the cap: only an ID_PREFIX head is kept.
        let mut over: Option<usize> = None;
        loop {
            let avail = self.r.fill_buf().await?;
            if avail.is_empty() {
                return Ok(match over {
                    Some(bytes) => Some(self.too_long(bytes)),
                    None if self.buf.is_empty() => None,
                    None => Some(ReadItem::Line(std::mem::take(&mut self.buf))),
                });
            }
            let (len, done) = match avail.iter().position(|&b| b == b'\n') {
                Some(i) => (i, true),
                None => (avail.len(), false),
            };
            let chunk = &avail[..len];
            match over.as_mut() {
                Some(total) => {
                    *total += len;
                    let room = ID_PREFIX.saturating_sub(self.buf.len()).min(len);
                    self.buf.extend_from_slice(&chunk[..room]);
                }
                None if self.buf.len() + len > self.max => {
                    over = Some(self.buf.len() + len);
                    self.buf.truncate(ID_PREFIX);
                    let room = ID_PREFIX.saturating_sub(self.buf.len()).min(len);
                    self.buf.extend_from_slice(&chunk[..room]);
                }
                None => self.buf.extend_from_slice(chunk),
            }
            self.r.consume(len + usize::from(done));
            if done {
                return Ok(Some(match over {
                    Some(bytes) => self.too_long(bytes),
                    None => ReadItem::Line(std::mem::take(&mut self.buf)),
                }));
            }
        }
    }

    fn too_long(&mut self, bytes: usize) -> ReadItem {
        let id = scan_id(&self.buf);
        self.buf.clear();
        ReadItem::TooLong { bytes, id }
    }
}

/// The value of the first `"id": <number|string>` in a (possibly cut) JSON prefix.
fn scan_id(prefix: &[u8]) -> Option<Value> {
    const KEY: &[u8] = b"\"id\"";
    let skip_ws = |mut i: usize| {
        while prefix.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let mut from = 0;
    while let Some(pos) = prefix[from..].windows(KEY.len()).position(|w| w == KEY) {
        let at = skip_ws(from + pos + KEY.len());
        if prefix.get(at) == Some(&b':') {
            return scalar(&prefix[skip_ws(at + 1)..]);
        }
        from += pos + 1;
    }
    None
}

/// A JSON string or number at the start of `s` (a number must be terminated inside `s`).
fn scalar(s: &[u8]) -> Option<Value> {
    match s.first()? {
        b'"' => {
            let mut escaped = false;
            for (j, &b) in s.iter().enumerate().skip(1) {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    return serde_json::from_slice::<String>(&s[..=j])
                        .ok()
                        .map(Value::String);
                }
            }
            None
        }
        b'-' | b'0'..=b'9' => {
            let end = s
                .iter()
                .position(|&b| !matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))?;
            serde_json::from_slice::<serde_json::Number>(&s[..end])
                .ok()
                .map(Value::Number)
        }
        _ => None,
    }
}

/// A stdout line, classified (§6.1).
#[derive(Debug, PartialEq)]
pub(crate) enum Incoming {
    /// A JSON object with `"jsonrpc":"2.0"`.
    Msg(Value),
    /// Anything else (logged at warn as `stdout: <line>`).
    Stray(String),
}

/// JSON-RPC message or stray output.
pub(crate) fn classify(line: &[u8]) -> Incoming {
    if let Ok(v) = serde_json::from_slice::<Value>(line)
        && v.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
    {
        return Incoming::Msg(v);
    }
    Incoming::Stray(String::from_utf8_lossy(line).trim_end().to_owned())
}

/// Stray-output flood detector: more than 100 stray lines in 10 s is a crash, except during the
/// first 5 s after spawn.
pub(crate) struct StrayGuard {
    spawned: Instant,
    seen: VecDeque<Instant>,
}

impl StrayGuard {
    /// A guard for a process spawned at `spawned`.
    pub(crate) fn new(spawned: Instant) -> Self {
        Self {
            spawned,
            seen: VecDeque::new(),
        }
    }

    /// Count one stray line at `now`; true = flood, treat as a crash.
    pub(crate) fn note(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.spawned) < BANNER {
            return false;
        }
        while self
            .seen
            .front()
            .is_some_and(|&t| now.saturating_duration_since(t) >= STRAY_WINDOW)
        {
            self.seen.pop_front();
        }
        self.seen.push_back(now);
        self.seen.len() > STRAY_MAX
    }
}

// ---------------------------------------------------------------------------------------------
// Backoff (§5)

/// How a plugin process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitKind {
    /// Exit code 0.
    Clean,
    /// Exit code ≠ 0, a signal, a broken pipe or a stray-output flood.
    Failed,
}

/// What the supervisor does after an exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Stay `exited` until a file changes, Restart or Enable.
    Exited,
    /// `backoff`, then start again after the delay.
    Retry(Duration),
    /// `failed` (crash loop, or a crash under `restart: never`).
    Failed,
}

/// Restart delay and crash-loop count of one plugin.
pub(crate) struct Backoff {
    first: Duration,
    cap: Duration,
    crash_loop: usize,
    window: Duration,
    delay: Duration,
    crashes: VecDeque<Instant>,
}

impl Backoff {
    /// From `restart_first_ms`, `restart_cap_ms`, `crash_loop`, `crash_window_s`.
    pub(crate) fn new(cfg: &PluginsConfig) -> Self {
        let first = Duration::from_millis(cfg.restart_first_ms);
        Self {
            first,
            cap: Duration::from_millis(cfg.restart_cap_ms).max(first),
            crash_loop: (cfg.crash_loop as usize).max(1),
            window: Duration::from_secs(cfg.crash_window_s),
            delay: first,
            crashes: VecDeque::new(),
        }
    }

    /// Decide after an exit at `now` of a process that ran `ran_for`. Exit 0 is a crash only under
    /// `restart: always`; a crash under `restart: never` is `Failed` at once.
    pub(crate) fn after_exit(
        &mut self,
        policy: RestartPolicy,
        kind: ExitKind,
        ran_for: Duration,
        now: Instant,
    ) -> Next {
        if ran_for >= RESET_AFTER {
            self.delay = self.first;
        }
        match (kind, policy) {
            (ExitKind::Clean, RestartPolicy::Always)
            | (ExitKind::Failed, RestartPolicy::OnFailure | RestartPolicy::Always) => {}
            (ExitKind::Clean, _) => return Next::Exited,
            (ExitKind::Failed, RestartPolicy::Never) => return Next::Failed,
        }
        while self
            .crashes
            .front()
            .is_some_and(|&t| now.saturating_duration_since(t) > self.window)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        if self.crashes.len() >= self.crash_loop {
            self.reset();
            return Next::Failed;
        }
        let d = self.delay;
        self.delay = (d * 2).min(self.cap);
        Next::Retry(d)
    }

    /// Forget crashes and the grown delay (Restart, a file change, Enable).
    pub(crate) fn reset(&mut self) {
        self.delay = self.first;
        self.crashes.clear();
    }
}

// ---------------------------------------------------------------------------------------------
// .pgids and the orphan reaper (§5 Orphans)

/// One lock for every [`Pgids`] handle: supervisors of different plugins share the file.
static PGIDS_LOCK: Mutex<()> = Mutex::new(());

/// `<plugins_dir>/.pgids`: one line `<pgid>\t<leader start time>\t<argv joined by space>` per
/// live plugin group. The start time (`ps -o lstart`) keeps a recycled pid from matching.
pub(crate) struct Pgids {
    path: PathBuf,
}

fn pgids_lock() -> MutexGuard<'static, ()> {
    PGIDS_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pgids {
    /// The record at `path` (created on first `add`).
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Record a spawned group whose leader started at `start` ([`leader_start`]).
    pub(crate) fn add(&self, pgid: u32, start: &str, argv: &[String]) {
        let _g = pgids_lock();
        let clean = |s: &str| s.replace(['\n', '\r', '\t'], " ");
        let line = format!("{pgid}\t{}\t{}\n", clean(start), clean(&argv.join(" ")));
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = res {
            tracing::warn!(path = %self.path.display(), error = %e, "plugins: .pgids add failed");
        }
    }

    /// Drop a group whose tree is gone.
    pub(crate) fn remove(&self, pgid: u32) {
        let _g = pgids_lock();
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "plugins: .pgids read failed");
                return;
            }
        };
        let key = pgid.to_string();
        let mut kept = String::with_capacity(text.len());
        for l in text
            .lines()
            .filter(|l| l.split(['\t', ' ']).next() != Some(key.as_str()))
        {
            kept.push_str(l);
            kept.push('\n');
        }
        if let Err(e) = std::fs::write(&self.path, kept) {
            tracing::warn!(path = %self.path.display(), error = %e, "plugins: .pgids write failed");
        }
    }

    /// Empty the record (engine shutdown, after every tree was killed).
    pub(crate) fn clear(&self) {
        let _g = pgids_lock();
        truncate(&self.path);
    }
}

fn truncate(path: &Path) {
    match std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
    {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "plugins: .pgids truncate failed")
        }
    }
}

/// At start: for each recorded group still alive whose leader has the recorded start time and
/// still runs the recorded argv, SIGTERM, wait `grace`, SIGKILL; then truncate the file.
pub(crate) async fn reap_orphans(path: &Path, grace: Duration) {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "plugins: .pgids read failed");
            String::new()
        }
    };
    #[cfg(unix)]
    {
        let mut stops = tokio::task::JoinSet::new();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(pgid), Some(start), Some(run)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(pgid) = pgid.parse::<u32>() else {
                continue;
            };
            // 0 is our own group, 1 is init's: never signal either.
            if pgid <= 1 || start.trim().is_empty() {
                continue;
            }
            let Ok(tree) = ProcessTree::adopt(pgid) else {
                continue;
            };
            if !tree.alive() {
                continue;
            }
            let Some((started, cmd)) = leader(pgid).await else {
                continue;
            };
            if started != start.trim() || !command_matches(&cmd, run) {
                continue;
            }
            tracing::info!(pgid, run, "plugins: reaping orphan plugin group");
            stops.spawn(async move { stop_tree(&tree, grace).await });
        }
        while stops.join_next().await.is_some() {}
    }
    #[cfg(not(unix))]
    let _ = (text, grace);
    truncate(path);
}

/// The leader's start time as `ps -o lstart` prints it (stable across `exec`); `None` off unix or
/// when the process is gone.
pub(crate) async fn leader_start(pid: u32) -> Option<String> {
    #[cfg(unix)]
    {
        leader(pid).await.map(|(start, _)| start)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// The leader's start time and full command line (`ps -ww -o lstart= -o command=`, C locale).
#[cfg(unix)]
async fn leader(pid: u32) -> Option<(String, String)> {
    let out = Command::new("ps")
        .args(["-ww", "-o", "lstart=", "-o", "command=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    split_lstart(String::from_utf8_lossy(&out.stdout).trim())
}

/// `Wed Nov 25 10:00:00 2026 <command>` → (the five lstart words, the command).
#[cfg(unix)]
fn split_lstart(s: &str) -> Option<(String, String)> {
    let mut rest = s;
    let mut words = Vec::with_capacity(5);
    for _ in 0..5 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        words.push(&rest[..end]);
        rest = &rest[end..];
    }
    let cmd = rest.trim();
    (!cmd.is_empty()).then(|| (words.join(" "), cmd.to_owned()))
}

/// The recorded argv exactly, or (an interpreter shim exec'd into the real binary) the same
/// arguments after a different program.
#[cfg(unix)]
fn command_matches(cmd: &str, run: &str) -> bool {
    let run = run.trim();
    if run.is_empty() {
        return false;
    }
    if cmd == run {
        return true;
    }
    match run.split_once(' ') {
        Some((_, args)) if !args.trim().is_empty() => cmd.ends_with(&format!(" {}", args.trim())),
        _ => false,
    }
}

/// SIGTERM the tree, poll every 50 ms up to `grace`, then SIGKILL.
pub(crate) async fn stop_tree(tree: &ProcessTree, grace: Duration) {
    if !tree.alive() {
        return;
    }
    let _ = tree.signal(Signal::Term);
    let deadline = Instant::now() + grace;
    while tree.alive() {
        if Instant::now() >= deadline {
            tree.kill();
            return;
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PluginsConfig {
        PluginsConfig::default()
    }

    #[test]
    fn backoff_doubles_then_fails() {
        let mut b = Backoff::new(&cfg());
        let t0 = Instant::now();
        let short = Duration::from_millis(100);
        let s = Duration::from_secs;
        let mut at = |secs: u64| {
            b.after_exit(
                RestartPolicy::OnFailure,
                ExitKind::Failed,
                short,
                t0 + s(secs),
            )
        };
        assert_eq!(at(0), Next::Retry(s(1)));
        assert_eq!(at(2), Next::Retry(s(2)));
        assert_eq!(at(5), Next::Retry(s(4)));
        assert_eq!(at(10), Next::Retry(s(8)));
        assert_eq!(at(20), Next::Failed);

        // Spread-out crashes never fail; the delay caps at restart_cap_ms.
        let mut b = Backoff::new(&cfg());
        let mut last = Duration::ZERO;
        for i in 0..20u64 {
            match b.after_exit(
                RestartPolicy::OnFailure,
                ExitKind::Failed,
                short,
                t0 + s(i * 100),
            ) {
                Next::Retry(d) => last = d,
                other => panic!("crash {i}: {other:?}"),
            }
        }
        assert_eq!(last, Duration::from_millis(cfg().restart_cap_ms));
        // 60 s of running resets the delay.
        assert_eq!(
            b.after_exit(
                RestartPolicy::OnFailure,
                ExitKind::Failed,
                s(60),
                t0 + s(5000)
            ),
            Next::Retry(s(1))
        );
    }

    #[test]
    fn exit0_on_failure_stays_exited() {
        let now = Instant::now();
        let ran = Duration::from_secs(1);
        let mut b = Backoff::new(&cfg());
        assert_eq!(
            b.after_exit(RestartPolicy::OnFailure, ExitKind::Clean, ran, now),
            Next::Exited
        );
        assert_eq!(
            b.after_exit(RestartPolicy::Never, ExitKind::Clean, ran, now),
            Next::Exited
        );
        assert_eq!(
            b.after_exit(RestartPolicy::Always, ExitKind::Clean, ran, now),
            Next::Retry(Duration::from_secs(1))
        );
    }

    #[tokio::test]
    async fn long_line_discarded_next_read() {
        let mut data = br#"{"jsonrpc":"2.0","id":7,"method":"emit","params":{"x":""#.to_vec();
        data.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));
        data.extend_from_slice(b"\"}}\n");
        let long = data.len() - 1;
        data.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"log\"}\n");
        let mut r = LineReader::new(&data[..], 1024 * 1024);
        assert_eq!(
            r.next().await.unwrap(),
            Some(ReadItem::TooLong {
                bytes: long,
                id: Some(serde_json::json!(7))
            })
        );
        let Some(ReadItem::Line(line)) = r.next().await.unwrap() else {
            panic!("expected a line");
        };
        assert!(matches!(classify(&line), Incoming::Msg(_)));
        assert_eq!(r.next().await.unwrap(), None);
        assert_eq!(
            scan_id(br#"{"id" : "a\"b","#),
            Some(serde_json::json!("a\"b"))
        );
        assert_eq!(classify(b"hello"), Incoming::Stray("hello".into()));
    }

    #[test]
    fn banner_lines_in_first_5s_tolerated() {
        let t0 = Instant::now();
        let mut g = StrayGuard::new(t0);
        for i in 0..150u64 {
            assert!(
                !g.note(t0 + Duration::from_millis(i * 30)),
                "banner line {i}"
            );
        }
        let after = t0 + BANNER;
        for i in 0..100u64 {
            assert!(!g.note(after + Duration::from_millis(i * 50)), "line {i}");
        }
        assert!(g.note(after + Duration::from_millis(100 * 50)));
    }

    #[tokio::test]
    async fn control_lane_before_queued_events() {
        for cap in [5000usize, 1000] {
            let out = Outbox::new(cap);
            for i in 0..5000 {
                out.event(serde_json::json!({"jsonrpc":"2.0","method":"event","params":{"n":i}}));
            }
            out.control(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"action"}));
            let (w, r) = tokio::io::duplex(64 * 1024);
            let writer = tokio::spawn(write_loop(out.clone(), w));
            let mut lines = tokio::io::BufReader::new(r).lines();
            let mut got = Vec::new();
            let want = 1 + cap + usize::from(cap < 5000);
            while got.len() < want {
                let l = lines.next_line().await.unwrap().unwrap();
                got.push(serde_json::from_str::<Value>(&l).unwrap());
            }
            let method = |v: &Value| v["method"].as_str().unwrap_or_default().to_owned();
            let first_event = got.iter().position(|v| method(v) == "event").unwrap();
            let action = got.iter().position(|v| method(v) == "action").unwrap();
            assert!(action < first_event, "cap {cap}");
            if cap < 5000 {
                assert_eq!(method(&got[0]), "lagged");
                assert_eq!(got[0]["params"]["dropped"], 5000 - cap);
                assert!(got[0]["params"]["since_ms"].as_u64().unwrap() > 0);
                assert_eq!(got[first_event]["params"]["n"], 5000 - cap);
            } else {
                assert!(got.iter().all(|v| method(v) != "lagged"));
            }
            out.shutdown(serde_json::json!({"jsonrpc":"2.0","method":"shutdown"}));
            let l = lines.next_line().await.unwrap().unwrap();
            assert!(l.contains("shutdown"));
            assert_eq!(lines.next_line().await.unwrap(), None);
            writer.await.unwrap().unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn orphan_group_reaped_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let argv: Vec<String> = vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()];
        let spec = SpawnSpec {
            argv: argv.clone(),
            cwd: dir.path().to_path_buf(),
            env: [("PATH".to_owned(), "/bin:/usr/bin".to_owned())].into(),
        };
        let s = spawn(&spec).unwrap();
        let pgid = s.pgid.unwrap();
        let tree = s.tree.clone().unwrap();
        let mut child = s.child;
        // Reap the leader as it dies so the group stops counting a zombie.
        let waiter = tokio::spawn(async move { child.wait().await });
        let path = dir.path().join(".pgids");
        Pgids::new(path.clone()).add(pgid, &argv);
        // Let the fork exec so `ps` shows the plugin, not this test binary.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(tree.alive());

        let grace = Duration::from_secs(2);
        let start = std::time::Instant::now();
        reap_orphans(&path, grace).await;
        while tree.alive() {
            assert!(
                start.elapsed() < grace + Duration::from_secs(1),
                "group {pgid} still alive"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let _ = waiter.await;
    }
}
