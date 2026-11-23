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
/// Process-wide: an engine is one process.
// TODO(210-M1): every other plugin binding a TCP listener (a2a HTTP, browser debug port, …)
// registers here too; the process-tree check covers the rest once it exists.
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
// TODO(210-M1): implement over a socket-table crate (`listeners` or `netstat2`, chosen in M1;
// `procfs` on Linux) plus `platform`'s process tree. Needs a new dependency line.
pub trait ProcessCheck: Send + Sync {
    /// `Some(true)` when the listener on loopback `port` is theuth or a child of it; `None` when
    /// unknown.
    fn owned_by_theuth(&self, port: u16) -> Option<bool>;
}

/// No socket-table lookup wired.
pub struct NoProcessCheck;

impl ProcessCheck for NoProcessCheck {
    fn owned_by_theuth(&self, _: u16) -> Option<bool> {
        None
    }
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
    /// Static refusals: zero and own ports.
    pub fn check(&self, port: u16) -> Result<(), PortRefused> {
        if port == 0 {
            return Err(PortRefused::Zero);
        }
        if self.own.is_own(port) || self.process.owned_by_theuth(port) == Some(true) {
            return Err(PortRefused::Own);
        }
        Ok(())
    }

    /// Check, then connect to `127.0.0.1:port`, else `[::1]:port`. Called per upstream connect.
    pub async fn connect(&self, port: u16) -> Result<TcpStream, PortRefused> {
        self.check(port)?;
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

/// Validate a `dir` root at publish and return its canonical form (§5.6 table).
pub fn check_root(root: &str, data_dir: &Path, home: Option<&Path>) -> Result<PathBuf, DirRefused> {
    let p = Path::new(root);
    if !p.is_absolute() {
        return Err(DirRefused::Relative);
    }
    if p.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().starts_with('.'),
        Component::ParentDir | Component::CurDir => true,
        _ => false,
    }) {
        return Err(DirRefused::Dot);
    }
    let canon = p
        .canonicalize()
        .ok()
        .filter(|c| c.is_dir())
        .ok_or_else(|| DirRefused::NotDir(root.to_owned()))?;
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
    if !canon.starts_with(root) {
        return None;
    }
    if canon.is_dir() {
        canon = canon.join("index.html").canonicalize().ok()?;
        if !canon.starts_with(root) {
            return None;
        }
    }
    canon.is_file().then_some(canon)
}

/// Open a canonical path without following a link on any component where the OS offers it
/// (macOS `O_NOFOLLOW_ANY`), else `O_NOFOLLOW` on the last; then `fstat` must equal the
/// canonical path's device and inode, so a link swapped in after [`resolve`] fails.
// TODO(210-M1): Linux `openat2(RESOLVE_NO_SYMLINKS)`; today Linux has the O_NOFOLLOW + fstat path.
pub fn open_nofollow(canon: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = libc::O_NOFOLLOW_ANY; // with O_NOFOLLOW the kernel answers EINVAL
    #[cfg(not(target_os = "macos"))]
    const FLAGS: i32 = libc::O_NOFOLLOW;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FLAGS)
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
}
