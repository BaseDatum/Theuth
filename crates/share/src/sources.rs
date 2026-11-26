//! Sources (210 §5.6): a loopback `port`, re-checked on every upstream connect, and a `dir`,
//! contained per request and opened at its canonical path without following links.

use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tokio::net::TcpStream;

/// Ports theuth itself listens on (§5.6): the own-listener registry.
pub trait OwnPorts: Send + Sync {
    /// `true` when `port` is one of theuth's own listeners.
    fn is_own(&self, port: u16) -> bool;
}

/// No registry wired: nothing is known to be own.
pub struct NoOwnPorts;

impl OwnPorts for NoOwnPorts {
    fn is_own(&self, _: u16) -> bool {
        false
    }
}

static OWN_LISTENERS: parking_lot::RwLock<std::collections::BTreeSet<u16>> =
    parking_lot::RwLock::new(std::collections::BTreeSet::new());

/// Record a TCP port this process listens on (the RPC listener from `rpc-compose`'s `serve`).
/// Process-wide: an engine is one process. Every other listener theuth binds (a2a HTTP, MCP
/// HTTP, OAuth callback, browser debug port, sandbox bring-up, …) lives in this process or one
/// of its children, which [`ProcessTree`] refuses by owner PID; it is not registered here
/// because those crates do not depend on `share`. This registry is the fast path that needs no
/// socket-table walk.
pub fn register_own_port(port: u16) {
    OWN_LISTENERS.write().insert(port);
}

/// [`OwnPorts`] over [`register_own_port`]'s registry.
pub struct OwnListeners;

impl OwnPorts for OwnListeners {
    fn is_own(&self, port: u16) -> bool {
        OWN_LISTENERS.read().contains(&port)
    }
}

/// The process-tree check (§5.6): the listening socket's owning PID is theuth or a descendant.
pub trait ProcessCheck: Send + Sync {
    /// `Some(true)` when the listener on loopback `port` is theuth or a child of it; `None` when
    /// unknown.
    fn owned_by_theuth(&self, port: u16) -> Option<bool>;
}

/// No socket-table lookup wired: every owner is unknown, so every port is refused.
pub struct NoProcessCheck;

impl ProcessCheck for NoProcessCheck {
    fn owned_by_theuth(&self, _: u16) -> Option<bool> {
        None
    }
}

/// Tests: every listener belongs to someone else.
#[cfg(test)]
pub(crate) struct NotTheuth;

#[cfg(test)]
impl ProcessCheck for NotTheuth {
    fn owned_by_theuth(&self, _: u16) -> Option<bool> {
        Some(false)
    }
}

/// [`ProcessCheck`] over the OS socket table (`listeners`: `proc_pidfdinfo` on macOS, `/proc` on
/// Linux): a TCP socket in `LISTEN` on `port` whose owner is this process or, walking parent
/// links up, one of its descendants (browser debug port, MCP servers, sandbox, a2a HTTP).
/// A daemon that double-forked away from theuth is no longer in the tree and is not caught here;
/// the own-listener registry covers the listeners theuth knows about.
pub struct ProcessTree;

impl ProcessCheck for ProcessTree {
    fn owned_by_theuth(&self, port: u16) -> Option<bool> {
        let all = listeners::get_all().ok()?;
        let me = std::process::id();
        Some(
            all.iter()
                .filter(|l| {
                    l.protocol == listeners::Protocol::TCP
                        && l.state == listeners::SocketState::Listen
                        && l.socket.port() == port
                })
                .any(|l| descends_from(l.process.pid, me)),
        )
    }
}

/// Deepest parent chain walked before giving up (a cycle or a pathological tree).
const TREE_DEPTH: usize = 64;

/// `true` when `pid` is `root` or a descendant of it.
fn descends_from(mut pid: u32, root: u32) -> bool {
    for _ in 0..TREE_DEPTH {
        if pid == root {
            return true;
        }
        match parent_pid(pid) {
            Some(p) if p != pid && p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: u32) -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is `size` writable bytes for the struct the call fills.
    let n = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: the call filled the whole struct.
    Some(unsafe { info.assume_init() }.pbi_ppid)
}

#[cfg(not(target_os = "macos"))]
fn parent_pid(pid: u32) -> Option<u32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces; fields resume after its ')': state, then ppid.
    let rest = s.get(s.rfind(')')? + 2..)?;
    rest.split_ascii_whitespace().nth(1)?.parse().ok()
}

/// Why a `port` source is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortRefused {
    /// Port 0.
    #[error("port 0 is not a port")]
    Zero,
    /// In the own-listener registry or owned by theuth's process tree.
    #[error("that port is theuth itself")]
    Own,
    /// Nothing listens on loopback.
    #[error("nothing is listening on localhost:{0}")]
    NotListening(u16),
}

/// The checks a `port` source passes before every upstream connect.
#[derive(Clone)]
pub struct PortGuard {
    /// Own-listener registry.
    pub own: Arc<dyn OwnPorts>,
    /// Process-tree check.
    pub process: Arc<dyn ProcessCheck>,
}

impl PortGuard {
    /// Static refusals: zero and own ports. An owner the process-tree check cannot determine
    /// (`None`: socket table unreadable, or no check wired) refuses too: fail closed.
    pub fn check(&self, port: u16) -> Result<(), PortRefused> {
        if port == 0 {
            return Err(PortRefused::Zero);
        }
        if self.own.is_own(port) || self.process.owned_by_theuth(port) != Some(false) {
            return Err(PortRefused::Own);
        }
        Ok(())
    }

    /// Check, then connect to `127.0.0.1:port`, else `[::1]:port`. Called per upstream connect.
    /// The socket-table walk is blocking work, so it runs off the async workers.
    pub async fn connect(&self, port: u16) -> Result<TcpStream, PortRefused> {
        let g = self.clone();
        tokio::task::spawn_blocking(move || g.check(port))
            .await
            .unwrap_or(Err(PortRefused::Own))?;
        for ip in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
        ] {
            if let Ok(s) = TcpStream::connect(ip).await {
                return Ok(s);
            }
        }
        Err(PortRefused::NotListening(port))
    }
}

/// Why a `dir` root is refused at publish.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DirRefused {
    /// Not absolute.
    #[error("the directory must be an absolute path")]
    Relative,
    /// Missing or not a directory.
    #[error("not a directory: {0}")]
    NotDir(String),
    /// theuth's data directory or one of its ancestors, or `$HOME` itself.
    #[error("{0} cannot be shared")]
    Protected(String),
    /// A dot-segment component (`.ssh`, `..`).
    #[error("a path with a hidden or dot component cannot be shared")]
    Dot,
}

fn has_dot(p: &Path) -> bool {
    p.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().starts_with('.'),
        Component::ParentDir | Component::CurDir => true,
        _ => false,
    })
}

/// Validate a `dir` root at publish and return its canonical form (§5.6 table). The dot and
/// protected-path rules apply to the typed path and again to the canonical one, so a root that
/// is (or passes through) a link into `~/.ssh` or the data directory is refused.
pub fn check_root(root: &str, data_dir: &Path, home: Option<&Path>) -> Result<PathBuf, DirRefused> {
    let p = Path::new(root);
    if !p.is_absolute() {
        return Err(DirRefused::Relative);
    }
    if has_dot(p) {
        return Err(DirRefused::Dot);
    }
    let canon = p
        .canonicalize()
        .ok()
        .filter(|c| c.is_dir())
        .ok_or_else(|| DirRefused::NotDir(root.to_owned()))?;
    if has_dot(&canon) {
        return Err(DirRefused::Dot);
    }
    let data = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_owned());
    if data.starts_with(&canon) || canon.starts_with(&data) {
        return Err(DirRefused::Protected(canon.display().to_string()));
    }
    if let Some(h) = home {
        let h = h.canonicalize().unwrap_or_else(|_| h.to_owned());
        if canon == h || h.starts_with(&canon) {
            return Err(DirRefused::Protected(canon.display().to_string()));
        }
    }
    Ok(canon)
}

/// A `dir` root as approved: its canonical path and the directory's device and inode. Every
/// request opens the canonical path and refuses a different directory, so replacing the approved
/// folder (by a link or a rename) after publish serves nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirPin {
    /// Canonical root.
    pub canon: PathBuf,
    dev: u64,
    ino: u64,
}

/// [`check_root`], then pin the directory it names.
pub fn pin_root(root: &str, data_dir: &Path, home: Option<&Path>) -> Result<DirPin, DirRefused> {
    let canon = check_root(root, data_dir, home)?;
    let meta = std::fs::symlink_metadata(&canon)
        .ok()
        .filter(|m| !m.file_type().is_symlink() && m.is_dir())
        .ok_or_else(|| DirRefused::NotDir(root.to_owned()))?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt as _;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = {
        let _ = meta;
        (0u64, 0u64)
    };
    Ok(DirPin { canon, dev, ino })
}

impl DirPin {
    /// A pin read back from the store (`share_pins`).
    pub fn from_parts(canon: PathBuf, dev: u64, ino: u64) -> Self {
        Self { canon, dev, ino }
    }

    /// The pinned device.
    pub fn dev(&self) -> u64 {
        self.dev
    }

    /// The pinned inode.
    pub fn ino(&self) -> u64 {
        self.ino
    }

    /// Open the canonical root without following links (every component on macOS, the last
    /// elsewhere) and require the pinned device and inode.
    #[cfg(unix)]
    pub fn check(&self) -> io::Result<()> {
        use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
        let d = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(NOFOLLOW | libc::O_DIRECTORY)
            .open(&self.canon)?;
        let m = d.metadata()?;
        if m.dev() != self.dev || m.ino() != self.ino || !m.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the shared folder was replaced after approval",
            ));
        }
        Ok(())
    }

    /// Verify the canonical root is still a real directory at the same canonical path (not
    /// replaced by a symlink or junction) on non-Unix platforms.
    #[cfg(not(unix))]
    pub fn check(&self) -> io::Result<()> {
        let m = std::fs::symlink_metadata(&self.canon)?;
        if m.file_type().is_symlink() || !m.is_dir() || std::fs::canonicalize(&self.canon)? != self.canon {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the shared folder was replaced after approval",
            ));
        }
        Ok(())
    }

    /// Resolve and open `url_path` below the pinned root: root check, [`resolve`],
    /// [`open_nofollow`], root check again (a swap during the open is caught before serving).
    pub fn open(&self, url_path: &str) -> Option<(PathBuf, File)> {
        self.check().ok()?;
        let canon = resolve(&self.canon, url_path)?;
        let f = open_nofollow(&canon).ok()?;
        self.check().ok()?;
        Some((canon, f))
    }
}

#[cfg(target_os = "macos")]
const NOFOLLOW: i32 = libc::O_NOFOLLOW_ANY; // with O_NOFOLLOW the kernel answers EINVAL
#[cfg(all(unix, not(target_os = "macos")))]
const NOFOLLOW: i32 = libc::O_NOFOLLOW;

/// Per-request resolution of a URL path below a canonical root (F-m8). `None` → 404.
/// Percent-decodes, refuses any dot component, canonicalises and checks containment, then maps a
/// directory to its `index.html`. The returned path is canonical and is what gets opened.
pub fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path)?;
    let mut p = root.to_path_buf();
    for seg in decoded.split('/').filter(|s| !s.is_empty()) {
        if seg.starts_with('.') || seg.contains('\\') || seg.contains('\0') {
            return None;
        }
        p.push(seg);
    }
    let mut canon = p.canonicalize().ok()?;
    if canon.is_dir() {
        canon = canon.join("index.html").canonicalize().ok()?;
    }
    // Containment and the dot rule on the canonical path below the root: a link inside the
    // root that leads to `.env` or into `.git/` is refused like the URL spelling would be.
    let rel = canon.strip_prefix(root).ok()?;
    if has_dot(rel) {
        return None;
    }
    canon.is_file().then_some(canon)
}

/// Open a canonical path without following a link on any component where the OS offers it
/// (macOS `O_NOFOLLOW_ANY`), else `O_NOFOLLOW` on the last; then `fstat` must equal the
/// canonical path's device and inode, so a link swapped in after [`resolve`] fails.
// TODO(210-M1): Linux `openat2(RESOLVE_NO_SYMLINKS)`; today Linux has the O_NOFOLLOW + fstat path.
#[cfg(unix)]
pub fn open_nofollow(canon: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(NOFOLLOW)
        .open(canon)?;
    let opened = f.metadata()?;
    let named = std::fs::symlink_metadata(canon)?;
    if opened.dev() != named.dev() || opened.ino() != named.ino() || !opened.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path changed under the share",
        ));
    }
    Ok(f)
}

/// Open a canonical path without following reparse points on Windows (`FILE_FLAG_OPEN_REPARSE_POINT`).
#[cfg(windows)]
pub fn open_nofollow(canon: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(canon)?;
    let opened = f.metadata()?;
    let named = std::fs::symlink_metadata(canon)?;
    if opened.file_type().is_symlink()
        || named.file_type().is_symlink()
        || !opened.is_file()
        || std::fs::canonicalize(canon)? != canon
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path changed under the share",
        ));
    }
    Ok(f)
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn dir_containment_refuses_symlink_escape_and_dots() {
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        let root = tmp.path().join("site");
        let secret = tmp.path().join("secret");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(root.join("index.html"), "hi").unwrap();
        std::fs::write(root.join("sub/a.txt"), "a").unwrap();
        std::fs::write(root.join(".env"), "x").unwrap();
        std::fs::write(secret.join("key"), "k").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("aws")).unwrap();
        std::os::unix::fs::symlink(secret.join("key"), root.join("k.txt")).unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let root = check_root(root.to_str().unwrap(), &data, None).unwrap();

        assert!(resolve(&root, "/").unwrap().ends_with("index.html"));
        assert!(resolve(&root, "/sub/a.txt").is_some());
        assert!(
            resolve(&root, "/aws/key").is_none(),
            "symlinked dir escapes"
        );
        assert!(resolve(&root, "/k.txt").is_none(), "symlinked file escapes");
        assert!(resolve(&root, "/.env").is_none());
        assert!(resolve(&root, "/%2e%2e/secret/key").is_none());
        assert!(resolve(&root, "/sub/../../secret/key").is_none());
        let f = resolve(&root, "/sub/a.txt").unwrap();
        assert!(open_nofollow(&f).is_ok());
        assert!(
            open_nofollow(&root.join("k.txt")).is_err(),
            "no-follow open of a link fails"
        );
    }

    /// FIX4 #17: a link inside the root that leads to a dot entry of the same root (`env` →
    /// `.env`, `git` → `.git/`) is refused on its canonical path, not only on the URL's spelling.
    #[cfg(unix)]
    #[test]
    fn link_to_a_dot_entry_inside_the_root_is_refused() {
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        let root = tmp.path().join("site");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".env"), "x").unwrap();
        std::fs::write(root.join(".git/config"), "c").unwrap();
        std::fs::write(root.join("ok.txt"), "ok").unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join("env")).unwrap();
        std::os::unix::fs::symlink(root.join(".git"), root.join("git")).unwrap();
        let root = root.canonicalize().unwrap();
        assert!(resolve(&root, "/ok.txt").is_some());
        assert!(resolve(&root, "/env").is_none());
        assert!(resolve(&root, "/git/config").is_none());
    }

    /// FIX4 #16: an unknown process-tree answer refuses the port (fail closed).
    #[test]
    fn unknown_owner_is_refused() {
        let g = PortGuard {
            own: Arc::new(NoOwnPorts),
            process: Arc::new(NoProcessCheck),
        };
        assert_eq!(g.check(5173), Err(PortRefused::Own));
        let g = PortGuard {
            own: Arc::new(NoOwnPorts),
            process: Arc::new(NotTheuth),
        };
        assert_eq!(g.check(5173), Ok(()));
    }

    #[test]
    fn dir_root_refusals() {
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        let data = tmp.path().join("d");
        std::fs::create_dir_all(data.join("x")).unwrap();
        let t = tmp.path().to_str().unwrap();
        assert!(matches!(
            check_root(t, &data, None),
            Err(DirRefused::Protected(_))
        ));
        assert!(matches!(
            check_root(data.join("x").to_str().unwrap(), &data, None),
            Err(DirRefused::Protected(_))
        ));
        assert_eq!(
            check_root("rel/dir", &data, None),
            Err(DirRefused::Relative)
        );
        assert_eq!(check_root("/a/.ssh", &data, None), Err(DirRefused::Dot));
        assert!(matches!(
            check_root(t, Path::new("/nonexistent"), Some(tmp.path())),
            Err(DirRefused::Protected(_))
        ));
    }

    struct Own(u16);
    impl OwnPorts for Own {
        fn is_own(&self, p: u16) -> bool {
            p == self.0
        }
    }
    struct Tree;
    impl ProcessCheck for Tree {
        fn owned_by_theuth(&self, p: u16) -> Option<bool> {
            Some(p == 7777)
        }
    }

    #[tokio::test]
    async fn port_deny() {
        let g = PortGuard {
            own: Arc::new(Own(9999)),
            process: Arc::new(Tree),
        };
        assert_eq!(g.check(9999), Err(PortRefused::Own));
        assert_eq!(g.check(7777), Err(PortRefused::Own));
        assert_eq!(g.check(0), Err(PortRefused::Zero));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(g.connect(port).await.is_ok());
        drop(l);
        assert_eq!(
            g.connect(port).await.err(),
            Some(PortRefused::NotListening(port))
        );
    }

    /// M1: the approved folder replaced by a link (or another directory) serves nothing, and a
    /// root whose canonical path passes through a dot directory is refused at publish.
    #[cfg(unix)]
    #[test]
    fn swapped_root_is_refused() {
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        let data = tmp.path().join("data");
        let site = tmp.path().join("site");
        let secrets = tmp.path().join(".theuth");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(site.join("index.html"), "hi").unwrap();
        std::fs::write(secrets.join("index.html"), "token").unwrap();

        let link = tmp.path().join("innocent");
        std::os::unix::fs::symlink(&secrets, &link).unwrap();
        assert_eq!(
            pin_root(link.to_str().unwrap(), &data, None),
            Err(DirRefused::Dot),
            "a link into a dot directory is refused on its canonical path"
        );

        let pin = pin_root(site.to_str().unwrap(), &data, None).unwrap();
        assert!(pin.open("/").is_some());
        std::fs::rename(&site, tmp.path().join("old")).unwrap();
        std::os::unix::fs::symlink(&secrets, &site).unwrap();
        assert!(pin.check().is_err());
        assert!(pin.open("/").is_none(), "link swapped in for the root");
        std::fs::remove_file(&site).unwrap();
        std::fs::create_dir(&site).unwrap();
        std::fs::write(site.join("index.html"), "other").unwrap();
        assert!(pin.open("/").is_none(), "another directory renamed in");
    }

    /// M2: a listener of this process or of a child is theuth's; a free port is not.
    #[test]
    fn process_tree_owns_own_and_child_listeners() {
        let own = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = own.local_addr().unwrap().port();
        assert_eq!(ProcessTree.owned_by_theuth(port), Some(true));
        drop(own);
        assert_eq!(ProcessTree.owned_by_theuth(port), Some(false));

        let Ok(mut child) = std::process::Command::new("python3")
            .args([
                "-c",
                "import socket,time\ns=socket.socket()\ns.bind(('127.0.0.1',0))\ns.listen()\n\
                 print(s.getsockname()[1],flush=True)\ntime.sleep(30)",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            return; // no python3: the child case cannot be staged here
        };
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let child_port: u16 = line.trim().parse().unwrap();
        let owned = ProcessTree.owned_by_theuth(child_port);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(owned, Some(true));
    }
}
