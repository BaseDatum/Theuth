//! Design doc 18: reach a `wth --headless` daemon. Probe the configured port first
//! (a terminal `wth --serve` or another app instance is reused, 18 §3); otherwise
//! be a terminal for one command: `$SHELL -l -i -c 'exec wth --headless …'` on a
//! pty, so the daemon inherits exactly the environment a terminal tab would give
//! it (18 §2). The pty transcript is drained into a 64 KiB ring and streamed to
//! the webview as `launch-log` events; readiness is the port, never the log.

use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const DEFAULT_LISTEN: &str = "127.0.0.1:7777";
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(200);
const EXIT_GRACE: Duration = Duration::from_secs(2);
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

/// The daemon we started: the pty master and the shell-turned-daemon child.
struct Daemon {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
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
        let (mut daemon, reader) = spawn(&data_dir, port, &self.log)?;
        std::thread::spawn({
            let log = Arc::clone(&self.log);
            let app = app.clone();
            move || drain(reader, &log, &app)
        });
        let ready = wait_ready(addr, daemon.child.as_mut());
        // Kept either way: on timeout the transcript stays inspectable until relaunch.
        inner.daemon = Some(daemon);
        ready?;
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

/// `$WTH_DATA_DIR`, else `~/.wth` (the same directory the terminal uses, 25 §5).
pub fn data_dir() -> PathBuf {
    std::env::var_os("WTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wth")))
        .unwrap_or_else(|| PathBuf::from(".wth"))
}

/// 18 §3: a live daemon on the configured `rpc.listen` is reused, not duplicated.
fn probe(data_dir: &Path) -> Option<Connection> {
    let token = token(data_dir)?;
    let listen = configured_listen(data_dir);
    let addr = listen.to_socket_addrs().ok()?.next()?;
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).ok()?;
    Some(Connection {
        url: ws_url(&listen, &token),
        token,
        data_dir: data_dir.to_owned(),
        owned: false,
    })
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

/// Open the pty and start the shell; the returned reader is the master's dup for
/// the drain thread.
fn spawn(
    data_dir: &Path,
    port: u16,
    log: &Mutex<Vec<u8>>,
) -> anyhow::Result<(Daemon, Box<dyn Read + Send>)> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let shell = shell(log);
    let script = format!(
        "exec {} --headless --data-dir {}",
        sq(&wth_bin().to_string_lossy()),
        sq(&data_dir.to_string_lossy())
    );
    let child = pair
        .slave
        .spawn_command(command(&shell, &script, port))
        .with_context(|| format!("spawn {}", shell.display()))?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader()?;
    Ok((
        Daemon {
            master: pair.master,
            child,
        },
        reader,
    ))
}

/// 18 §2 step 2: the user's login, interactive shell running one command, with
/// the app's environment plus `TERM`, the `WTH_LAUNCH=1` guard (18 §4) and the
/// port override (01 §1). cwd is `$HOME`, as for a new terminal tab.
fn command(shell: &Path, script: &str, port: u16) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(["-l", "-i", "-c", script]);
    cmd.env("TERM", "xterm-256color");
    cmd.env("WTH_LAUNCH", "1");
    cmd.env("WTH_RPC__LISTEN", format!("127.0.0.1:{port}"));
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }
    cmd
}

/// `$SHELL` when it exists, else `/bin/zsh` — said in the transcript (18 §4).
fn shell(log: &Mutex<Vec<u8>>) -> PathBuf {
    match std::env::var_os("SHELL").map(PathBuf::from) {
        Some(s) if s.is_file() => s,
        _ => {
            append(log, b"SHELL unset or not executable; using /bin/zsh\r\n");
            PathBuf::from("/bin/zsh")
        }
    }
}

/// `$WTH_BIN`, else the bundled sidecar beside this executable (25 §5), else
/// `wth` on the shell's PATH.
fn wth_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("WTH_BIN") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("wth"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("wth"))
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
/// (shell error, missing binary) fails fast, a timeout names the likely cause.
fn wait_ready(addr: SocketAddr, child: &mut dyn Child) -> anyhow::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(&addr, POLL).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "daemon exited before listening (exit code {})",
                status.exit_code()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "no daemon on {addr} after {} s: your shell startup is waiting for input",
                READY_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL);
    }
}

/// Drop the master (SIGHUP once every fd is closed), give the daemon 2 s to
/// exit on its own (the webview called RPC `shutdown` first), then kill.
fn stop(daemon: Option<Daemon>) {
    let Some(Daemon { master, mut child }) = daemon else {
        return;
    };
    drop(master);
    let deadline = Instant::now() + EXIT_GRACE;
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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
            .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
        let mut child = pair
            .slave
            .spawn_command(command(&shell, "exec env", 7802))
            .unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        drop(pair.master);
        let mut out = Vec::new();
        let _ = reader.read_to_end(&mut out);
        child.wait().unwrap();
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("TERM=xterm-256color"), "{out}");
        assert!(out.contains("WTH_LAUNCH=1"), "{out}");
        assert!(out.contains("WTH_RPC__LISTEN=127.0.0.1:7802"), "{out}");
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
}
