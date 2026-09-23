//! One engine per data dir (design doc 63 §2.1). The holder of an exclusive
//! advisory lock on `<data_dir>/engine.lock` is *the* engine; it writes
//! `<data_dir>/engine.json` so humans and clients can find it. The lock is
//! what arbitrates (the kernel releases it when the process dies, however it
//! dies); the record is advisory and may be stale.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `discover`'s TCP probe budget.
const PROBE: Duration = Duration::from_millis(500);

/// Which frontend started the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    /// `t` (the terminal UI, with or without `--serve`).
    Tui,
    /// `t --headless` from a terminal or a service.
    Headless,
    /// `t --headless` spawned by the desktop app (`THEUTH_LAUNCH=1`).
    Desktop,
    /// `t --acp`.
    Acp,
    /// `t -p`.
    Prompt,
    /// `t --link-signal`: the one-shot Signal pairing run.
    Signal,
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Host::Tui => "tui",
            Host::Headless => "headless",
            Host::Desktop => "desktop",
            Host::Acp => "acp",
            Host::Prompt => "prompt",
            Host::Signal => "signal",
        })
    }
}

/// `<data_dir>/engine.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The engine process.
    pub pid: u32,
    /// Where the engine's RPC server listens — `rpc.listen`, or the
    /// ephemeral loopback port an implicit engine bound (63 §6.4), filled by
    /// `set_listen` once the bind happened; `None` until then (and for
    /// `--link-signal`, which serves nothing).
    pub listen: Option<String>,
    /// When the lock was taken.
    pub started: DateTime<Utc>,
    /// Which frontend started it.
    pub host: Host,
    /// The engine's `CARGO_PKG_VERSION`.
    pub version: String,
    /// This engine instance.
    pub id: Uuid,
}

/// An engine that holds the lock.
#[derive(Clone, Debug)]
pub struct Running {
    /// Its record.
    pub record: Record,
    /// `ws://<listen>/?token=<token>` when the engine serves RPC and the
    /// token is readable.
    pub url: Option<String>,
}

impl fmt::Display for Running {
    /// 63 §0.5, the exact refusal line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = &self.record;
        let since = r.started.with_timezone(&Local).format("%H:%M");
        let ws = r
            .listen
            .as_deref()
            .map_or_else(|| "no socket yet".to_owned(), |l| format!("ws://{l}"));
        // Every engine serves (63 §6.4); `None` is the moment before its bind.
        let hint = if r.listen.is_some() {
            "run `t` to attach"
        } else {
            "retry in a moment to attach"
        };
        write!(
            f,
            "engine already running (pid {}, {}, since {since}, {ws}) — {hint}, or set THEUTH_DATA_DIR for a separate store",
            r.pid, r.host
        )
    }
}

/// Why `acquire` failed.
#[derive(Debug)]
pub enum Error {
    /// Another process holds the lock.
    Running(Running),
    /// The lock file could not be opened or the record not written.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Running(r) => r.fmt(f),
            Error::Io(e) => write!(f, "engine lock: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// The lock, held for the life of the engine. Dropping it unlinks
/// `engine.json`; the kernel releases the lock when the file closes.
#[derive(Debug)]
pub struct Lease {
    _lock: File,
    path: PathBuf,
    /// What `engine.json` says about this engine.
    pub record: Record,
}

impl Lease {
    fn write(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.record).map_err(io::Error::other)?;
        std::fs::write(&self.path, json)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The process's lease, parked here so whoever ends the process (`main`)
/// can drop it before `exit`: the cold start parks it after `acquire`, an
/// attached TUI after its takeover (63 §2.3).
static HELD: Mutex<Option<Lease>> = Mutex::new(None);

/// Park `lease` for the rest of the process.
pub fn hold(lease: Lease) {
    *HELD.lock().unwrap_or_else(PoisonError::into_inner) = Some(lease);
}

/// Drop the parked lease (unlinks `engine.json`); a no-op when none is held.
pub fn release() {
    HELD.lock().unwrap_or_else(PoisonError::into_inner).take();
}

/// The RPC server bound: record where (63 §6.4 — an implicit engine binds an
/// ephemeral port, known only now). No-op when no lease is parked.
pub fn set_listen(listen: String) {
    if let Some(lease) = HELD.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        lease.record.listen = Some(listen);
        if let Err(e) = lease.write() {
            tracing::warn!(error = %e, "engine.json rewrite");
        }
    }
}

/// Take the lock and write the record. `Err(Running)` names the holder.
pub fn acquire(data_dir: &Path, host: Host, listen: Option<String>) -> Result<Lease, Error> {
    std::fs::create_dir_all(data_dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("engine.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            // The holder writes its record right after locking; give it a beat.
            for _ in 0..10 {
                if let Some(record) = read(data_dir) {
                    return Err(Error::Running(running(data_dir, record)));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            return Err(Error::Io(io::Error::other(
                "engine.lock is held but engine.json is missing",
            )));
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(Error::Io(e)),
    }
    let record = Record {
        pid: std::process::id(),
        listen,
        started: Utc::now(),
        host,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        id: Uuid::now_v7(),
    };
    let lease = Lease {
        _lock: lock,
        path: data_dir.join("engine.json"),
        record,
    };
    lease.write()?;
    Ok(lease)
}

/// The running engine per its record: the file exists, its pid is alive and,
/// when it serves RPC, its port answers within 500 ms. Anything less is a
/// stale record and `None`. Advisory: `acquire` is what arbitrates.
pub fn discover(data_dir: &Path) -> Option<Running> {
    let record = read(data_dir)?;
    if !alive(record.pid) {
        return None;
    }
    if let Some(listen) = &record.listen {
        let addr = listen.to_socket_addrs().ok()?.next()?;
        TcpStream::connect_timeout(&addr, PROBE).ok()?;
    }
    Some(running(data_dir, record))
}

fn read(data_dir: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(data_dir.join("engine.json")).ok()?).ok()
}

fn running(data_dir: &Path, record: Record) -> Running {
    let url = record.listen.as_deref().and_then(|listen| {
        let token = std::fs::read_to_string(data_dir.join("token")).ok()?;
        Some(format!("ws://{listen}/?token={}", token.trim()))
    });
    Running { record, url }
}

/// Is a process with `pid` alive? Signal 0 on unix (`EPERM` is alive, just
/// not ours).
// ponytail: use platform::process::alive once 58 step 1 lands.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(2) with signal 0 only checks for the process.
    let ok = unsafe { libc::kill(pid, 0) } == 0;
    ok || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain Win32 calls; the handle is closed on every path.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(h);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("theuth-engine-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn second_acquire_in_one_process_is_running() {
        let d = dir();
        let lease = acquire(&d, Host::Tui, Some("127.0.0.1:1".into())).unwrap();
        match acquire(&d, Host::Headless, None) {
            Err(Error::Running(r)) => {
                assert_eq!(r.record, lease.record);
                assert!(r.to_string().starts_with(&format!(
                    "engine already running (pid {}, tui, since ",
                    std::process::id()
                )));
                assert!(r.to_string().contains("ws://127.0.0.1:1) — run `t` to attach"));
            }
            other => panic!("{other:?}"),
        }
        drop(lease);
        assert!(!d.join("engine.json").exists());
        acquire(&d, Host::Headless, None).unwrap();
    }

    #[test]
    fn stale_record_is_not_running() {
        let d = dir();
        let stale = Record {
            pid: u32::MAX - 1,
            listen: Some("127.0.0.1:1".into()),
            started: Utc::now(),
            host: Host::Desktop,
            version: "0".into(),
            id: Uuid::now_v7(),
        };
        std::fs::write(d.join("engine.json"), serde_json::to_vec(&stale).unwrap()).unwrap();
        assert!(discover(&d).is_none());
        let lease = acquire(&d, Host::Tui, None).unwrap();
        assert_ne!(read(&d).unwrap(), stale);
        assert_eq!(lease.record.pid, std::process::id());
        // Alive pid, no socket: running, unattachable.
        let found = discover(&d).unwrap();
        assert_eq!(found.record, lease.record);
        assert!(found.url.is_none());
        assert!(found.to_string().contains("no socket yet) — retry in a moment"));
    }

    /// 63 §6.4: an implicit engine learns its ephemeral port after `acquire`;
    /// `set_listen` puts it in the parked record and the file.
    #[test]
    fn ephemeral_listen_is_recorded_after_bind() {
        let d = dir();
        let lease = acquire(&d, Host::Prompt, None).unwrap();
        assert_eq!(read(&d).unwrap().listen, None);
        hold(lease);
        set_listen("127.0.0.1:49731".into());
        let r = read(&d).unwrap();
        assert_eq!(r.listen.as_deref(), Some("127.0.0.1:49731"));
        assert_eq!(serde_json::from_slice::<Record>(&serde_json::to_vec(&r).unwrap()).unwrap(), r);
        release();
        assert!(!d.join("engine.json").exists());
    }

    #[test]
    fn record_round_trips() {
        let r = Record {
            pid: 42,
            listen: None,
            started: Utc::now(),
            host: Host::Acp,
            version: "1.2.3".into(),
            id: Uuid::now_v7(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["host"], "acp");
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), r);
    }
}
