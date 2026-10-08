//! Design doc 18: reach a `t --headless` daemon. Probe the running engine
//! first (63: its `engine.json` record, else the configured port — a terminal
//! `t --serve` or another app instance is reused, 18 §3); otherwise
//! be a terminal for one command: `$SHELL -l -i -c 'exec t --headless …'` on a
//! pty, so the daemon inherits exactly the environment a terminal tab would give
//! it (18 §2). The pty transcript is drained into a 64 KiB ring and streamed to
//! the webview as `launch-log` events; readiness is the port, never the log.
//!
//! Windows has no login-shell environment (58 §0.3), and a ConPTY child drags a
//! console window along and hangs in conhost's handshake on real installs, so
//! there `t.exe --headless …` is a plain hidden child (`CREATE_NO_WINDOW`) with
//! stdout/stderr piped into the same ring.

use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
#[cfg(unix)]
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const DEFAULT_LISTEN: &str = "127.0.0.1:7777";
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(200);
/// How long a daemon told `shutdown` may take to exit before it is killed.
/// 109 §9.2: the daemon drains in-flight turns for `durability.drain_secs`
/// (default 10 s) plus a second of slack before it exits; this covers that
/// default so a quit never SIGKILLs a drain in progress. A daemon with
/// nothing to drain exits in well under a second as before.
const EXIT_GRACE: Duration = Duration::from_secs(12);
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
struct Daemon {
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
}

/// Managed state: the established connection, the daemon we own (if any) and
/// the pty transcript. `connection`/`relaunch` serialise on one lock; the log
/// has its own so it stays readable while a launch is in flight.
#[derive(Default)]
pub struct Launch {
    inner: Mutex<Inner>,
    log: Arc<Mutex<Vec<u8>>>,
}

impl Launch {
    /// The cached connection, or probe-then-spawn on the first call (and after a
    /// failed one).
    pub fn connection(&self, app: &AppHandle) -> anyhow::Result<Connection> {
        let mut inner = lock(&self.inner);
        if let Some(conn) = &inner.connection {
            return Ok(conn.clone());
        }
        self.start(&mut inner, app)
    }

    /// Stop the daemon we own (if any) and connect again.
    pub fn relaunch(&self, app: &AppHandle) -> anyhow::Result<Connection> {
        let mut inner = lock(&self.inner);
        inner.connection = None;
        self.start(&mut inner, app)
    }

    /// The cached connection, else a probe for a live engine (18 §3); never spawns one.
    /// `models.rs` asks before swapping a checkpoint the daemon may have loaded.
    pub fn peek(&self) -> Option<Connection> {
        lock(&self.inner)
            .connection
            .clone()
            .or_else(|| probe(&data_dir()))
    }

    /// 18 §3: hang up the pty; kill the daemon if it is still alive after 2 s.
    /// A reused daemon is left running.
    pub fn shutdown(&self) {
        stop(lock(&self.inner).daemon.take());
    }

    /// The pty transcript so far (lossy UTF-8).
    pub fn log(&self) -> String {
        String::from_utf8_lossy(&lock(&self.log)).into_owned()
    }

    fn start(&self, inner: &mut Inner, app: &AppHandle) -> anyhow::Result<Connection> {
        stop(inner.daemon.take());
        lock(&self.log).clear();
        let data_dir = data_dir();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("create {}", data_dir.display()))?;
        if let Some(conn) = probe(&data_dir) {
            inner.connection = Some(conn.clone());
            return Ok(conn);
        }

        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let (mut daemon, readers) = spawn(&data_dir, port, &self.log)?;
        for reader in readers {
            std::thread::spawn({
                let log = Arc::clone(&self.log);
                let app = app.clone();
                move || drain(reader, &log, &app)
            });
        }
        let ready = wait_ready(addr, &mut daemon.child, &self.log);
        // Kept either way: on timeout the transcript stays inspectable until relaunch.
        inner.daemon = Some(daemon);
        if let Err(e) = ready {
            // 146 D6: the daemon exited 0 because another engine already holds
            // `engine.lock` (a launcher race): attach to that one instead.
            if let Some(conn) = probe(&data_dir) {
                inner.connection = Some(conn.clone());
                return Ok(conn);
            }
            return Err(e);
        }
        let token = token(&data_dir).context("daemon did not write <data_dir>/token")?;
        let conn = Connection {
            url: ws_url(&addr.to_string(), &token),
            token,
            data_dir,
            owned: true,
        };
        inner.connection = Some(conn.clone());
        Ok(conn)
    }
}

/// `$THEUTH_DATA_DIR`, else `~/.theuth` (the same directory the terminal uses, 25 §5).
pub fn data_dir() -> PathBuf {
    std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".theuth")))
        .unwrap_or_else(|| PathBuf::from(".theuth"))
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
        sq(&bin.to_string_lossy()),
        sq(&data_dir.to_string_lossy())
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

/// POSIX single-quote for `sh -c`.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

/// 18 §2 step 5 / §4: poll the port every 200 ms for 15 s; an early child exit
/// (shell error, missing binary) fails fast, a timeout names the likely cause
/// (Unix: a shell startup stuck on a prompt) and quotes the transcript's tail.
fn wait_ready(addr: SocketAddr, child: &mut Child, log: &Mutex<Vec<u8>>) -> anyhow::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(&addr, POLL).is_ok() {
            return Ok(());
        }
        if let Some(code) = exited(child)? {
            bail!("daemon exited before listening (exit code {code})");
        }
        if Instant::now() >= deadline {
            let text = strip_controls(&lock(log));
            let mut msg = format!("no daemon on {addr} after {} s", READY_TIMEOUT.as_secs());
            if cfg!(unix) && waiting_for_input(&text) {
                msg.push_str(": your shell startup is waiting for input");
            }
            let tail = last_lines(&text, 5);
            if !tail.is_empty() {
                msg.push('\n');
                msg.push_str(&tail);
            }
            bail!(msg);
        }
        std::thread::sleep(POLL);
    }
}

/// The transcript without terminal control sequences (CSI, OSC/DCS/PM/APC and
/// two-byte escapes) and stray C0 bytes: conhost's pseudo-console handshake
/// (`\x1b[?9001h`, `\x1b]0;…\x07`, `\x1b[6n`) is not something a shell said.
fn strip_controls(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        if b != 0x1b {
            if b >= 0x20 || matches!(b, b'\n' | b'\r' | b'\t') {
                out.push(b);
            }
            continue;
        }
        match bytes.get(i) {
            // CSI: parameters/intermediates 0x20..=0x3f, final byte 0x40..=0x7e.
            Some(b'[') => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            // OSC/DCS/PM/APC: string until BEL or ST (`ESC \`).
            Some(b']' | b'P' | b'^' | b'_') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // `ESC <intermediates> <final>` (e.g. `ESC ( B`, `ESC =`).
            Some(_) => {
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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

/// Hang up (Unix: drop the master, SIGHUP once every fd is closed; Windows has
/// no line to hang up — stdin is already null), give the daemon [`EXIT_GRACE`]
/// to exit on its own (the webview called RPC `shutdown` first), then kill. On Windows the
/// kill is the whole Job Object, and dropping its handle afterwards kills
/// whatever a clean daemon exit left behind.
fn stop(daemon: Option<Daemon>) {
    let Some(Daemon {
        #[cfg(unix)]
        master,
        #[cfg(windows)]
        tree,
        mut child,
    }) = daemon
    else {
        return;
    };
    #[cfg(unix)]
    drop(master);
    let deadline = Instant::now() + EXIT_GRACE;
    while Instant::now() < deadline {
        if !matches!(exited(&mut child), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    #[cfg(windows)]
    tree.kill();
    #[cfg(unix)]
    if let Err(e) = child.kill() {
        tracing::warn!(error = %e, "kill daemon");
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
        assert_eq!(sq("a'b"), r"'a'\''b'");
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
}
