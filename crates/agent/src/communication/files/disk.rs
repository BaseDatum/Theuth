//! The file-system half of 199: the egress gate (D6) and the protected set, BLAKE3 from
//! a handle (D9), the macOS quarantine and no-clobber publication by inode (Storage).

use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{FILE_MAX, candidate};

// ---------------------------------------------------------------------------
// Egress gate (D6)
// ---------------------------------------------------------------------------

/// SQLite stores, relative to a root; each comes with its `-wal`, `-shm` and
/// `-journal` siblings (the Protected set table).
const PROTECTED_STORES: &[&str] = &[
    "secrets.sqlite",
    "signal/presage.db",
    "sessions.sqlite",
    "skills.sqlite",
    "artifacts.sqlite",
    "knowledge.sqlite",
    "replica.sqlite",
    "mesh.sqlite",
    "roster.sqlite",
    "link.sqlite",
    "double/double.sqlite",
    "schedule.sqlite",
    "tasks.sqlite",
    "mcp.sqlite",
    "voice.db",
    "meetings/meetings.sqlite",
    "meetings/briefs.sqlite",
    "documents/index.sqlite",
    "documents/mirror.sqlite",
    "autoresearch/autoresearch.sqlite",
    "cells/cells.sqlite",
];

/// Keys, the vault and the RPC bearer, relative to a root.
const PROTECTED_FILES: &[&str] = &[
    "secrets.key",
    "secrets.vault",
    "secrets.vault.migrated",
    "mesh/identity",
    "link/identity",
    "token",
];

/// The siblings SQLite writes beside a store.
const SQLITE_SIBLINGS: [&str; 4] = ["", "-wal", "-shm", "-journal"];

/// Every protected path under `root` that exists now: the fixed files, each store and
/// its siblings, `oauth/*.json`, `oauth/mcp/*` and `codeindex/*.sqlite*`.
fn protected_paths(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = PROTECTED_FILES.iter().map(|p| root.join(p)).collect();
    for store in PROTECTED_STORES {
        out.extend(
            SQLITE_SIBLINGS
                .iter()
                .map(|s| root.join(format!("{store}{s}"))),
        );
    }
    let listed = |dir: PathBuf, keep: fn(&str) -> bool| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(keep))
            .map(|e| e.path())
            .collect()
    };
    out.extend(listed(root.join("oauth"), |n| n.ends_with(".json")));
    out.extend(listed(root.join("oauth/mcp"), |_| true));
    out.extend(listed(root.join("codeindex"), |n| n.contains(".sqlite")));
    out
}

/// `(dev, inode)` of every protected file of every root (missing files are skipped).
/// Matching the opened handle against inodes defeats symlinks, hardlinks and case
/// variants; a copy made first has another inode (the stated limit, D6).
pub fn protected_set(roots: &[&Path]) -> HashSet<(u64, u64)> {
    roots
        .iter()
        .flat_map(|r| protected_paths(r))
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| (m.dev(), m.ino()))
        .collect()
}

/// The roots whose protected set the gate builds: this runtime's data dir and, when
/// different, the default `~/.theuth`.
fn gate_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    if let Some(home) = std::env::var_os("HOME") {
        let default = PathBuf::from(home).join(".theuth");
        if default != root {
            roots.push(default);
        }
    }
    roots
}

/// Why the gate refused a file. [`EgressError`]'s text is the `send_message` error.
#[derive(Debug)]
pub enum Refusal {
    /// The path is relative.
    NotAbsolute,
    /// Nothing there.
    NotFound,
    /// A directory, FIFO, socket or device.
    NotRegular,
    /// Over [`FILE_MAX`]; the size.
    TooLarge(u64),
    /// Theuth's own secret state.
    Protected,
    /// A reopen found another file, or another size, than the one offered.
    Changed,
    /// Any other open or stat failure.
    Io(io::Error),
}

/// A refused path and why.
#[derive(Debug)]
pub struct EgressError {
    /// As the caller named it.
    pub path: PathBuf,
    /// Why.
    pub refusal: Refusal,
}

impl fmt::Display for EgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.path.display();
        match &self.refusal {
            Refusal::NotAbsolute => write!(f, "files: {p} is not an absolute path"),
            Refusal::NotFound => write!(f, "files: {p}: no such file"),
            Refusal::NotRegular => write!(f, "files: {p}: not a regular file"),
            Refusal::TooLarge(n) => {
                write!(f, "files: {p} is {n} bytes (over the 5 GB limit per file)")
            }
            Refusal::Protected => write!(
                f,
                "files: {p} is theuth's own secret state and is never sent"
            ),
            Refusal::Changed => write!(f, "files: {p} changed since it was offered"),
            Refusal::Io(e) => write!(f, "files: {p}: {e}"),
        }
    }
}

impl std::error::Error for EgressError {}

/// A file the gate let out: the opened handle (blocking again) and what it was checked as.
/// Hashing, cloning and serving use this handle, never the path.
#[derive(Debug)]
pub struct Egress {
    /// Open read-only, close-on-exec.
    pub file: File,
    /// The handle's device.
    pub dev: u64,
    /// The handle's inode.
    pub inode: u64,
    /// Its size at the check.
    pub size: u64,
}

/// D6: open `path` for sending, with `root` this engine's data dir.
pub fn egress_open(path: &Path, root: &Path) -> Result<Egress, EgressError> {
    let roots = gate_roots(root);
    let roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    open_checked(path, &protected_set(&roots))
}

/// D6: a reopen (serving the original without a clone, or after a restart) repeats
/// every check and also requires the recorded `(dev, inode)` and size.
pub fn egress_reopen(
    path: &Path,
    root: &Path,
    dev: u64,
    inode: u64,
    size: u64,
) -> Result<Egress, EgressError> {
    let e = egress_open(path, root)?;
    if (e.dev, e.inode, e.size) != (dev, inode, size) {
        return Err(EgressError {
            path: path.to_path_buf(),
            refusal: Refusal::Changed,
        });
    }
    Ok(e)
}

/// Steps 1–5 of D6 against a given protected set.
pub fn open_checked(path: &Path, protected: &HashSet<(u64, u64)>) -> Result<Egress, EgressError> {
    let refuse = |refusal| EgressError {
        path: path.to_path_buf(),
        refusal,
    };
    if !path.is_absolute() {
        return Err(refuse(Refusal::NotAbsolute));
    }
    // 1. Non-blocking, so a FIFO with no writer answers at once (std sets O_CLOEXEC).
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            refuse(match e.kind() {
                io::ErrorKind::NotFound => Refusal::NotFound,
                // A socket does not open (ENXIO on Linux, EOPNOTSUPP on macOS): name what it
                // is. The stat only words the refusal; nothing is let out on it.
                _ if std::fs::metadata(path).is_ok_and(|m| !m.is_file()) => Refusal::NotRegular,
                _ => Refusal::Io(e),
            })
        })?;
    // 2. fstat the handle.
    let meta = file.metadata().map_err(|e| refuse(Refusal::Io(e)))?;
    if !meta.file_type().is_file() {
        return Err(refuse(Refusal::NotRegular));
    }
    // 3–4.
    if meta.len() > FILE_MAX {
        return Err(refuse(Refusal::TooLarge(meta.len())));
    }
    let (dev, inode) = (meta.dev(), meta.ino());
    if protected.contains(&(dev, inode)) {
        return Err(refuse(Refusal::Protected));
    }
    // 5. Blocking again for the reads that follow.
    clear_nonblock(&file).map_err(|e| refuse(Refusal::Io(e)))?;
    Ok(Egress {
        file,
        dev,
        inode,
        size: meta.len(),
    })
}

fn clear_nonblock(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is open for the duration of both calls (borrowed from `file`).
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BLAKE3 (D9)
// ---------------------------------------------------------------------------

/// BLAKE3 of the first `len` bytes of `file`, 64 lowercase hex, read with `pread`
/// (the handle's offset is untouched). A file shorter than `len` is `UnexpectedEof`;
/// bytes past `len` (an original that grew) are never read.
pub fn blake3_of(file: &File, len: u64) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut at = 0u64;
    while at < len {
        let want = usize::try_from(len - at)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = file.read_at(&mut buf[..want], at)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        hasher.update(&buf[..n]);
        at += n as u64;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

// ---------------------------------------------------------------------------
// Quarantine (Storage, step 3)
// ---------------------------------------------------------------------------

/// The `com.apple.quarantine` value: `0081;<unix seconds, lowercase hex>;theuth;<UUID, uppercase>`.
pub fn quarantine_value(unix_secs: u64, id: Uuid) -> String {
    format!(
        "0081;{unix_secs:08x};theuth;{}",
        id.as_hyphenated().to_string().to_uppercase()
    )
}

/// Set `com.apple.quarantine` to `value` on `file`, read it back and compare
/// (macOS; a no-op elsewhere).
pub fn quarantine(file: &File, value: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd as _;
        const NAME: &std::ffi::CStr = c"com.apple.quarantine";
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is open; `NAME` is NUL-terminated; `value` is valid for its length.
        let rc =
            unsafe { libc::fsetxattr(fd, NAME.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut back = vec![0u8; value.len() + 64];
        // SAFETY: `back` is writable for its length.
        let n = unsafe {
            libc::fgetxattr(
                fd,
                NAME.as_ptr(),
                back.as_mut_ptr().cast(),
                back.len(),
                0,
                0,
            )
        };
        let n = usize::try_from(n).map_err(|_| io::Error::last_os_error())?;
        if &back[..n] != value.as_bytes() {
            return Err(io::Error::other(format!(
                "quarantine read back {:?}, not {value:?}",
                String::from_utf8_lossy(&back[..n])
            )));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (file, value);
    Ok(())
}

// ---------------------------------------------------------------------------
// Publication (Storage, steps 3–6)
// ---------------------------------------------------------------------------

/// `(dev, inode)` of `path` itself, not following a symlink.
pub fn inode_of(path: &Path) -> io::Result<(u64, u64)> {
    let m = std::fs::symlink_metadata(path)?;
    Ok((m.dev(), m.ino()))
}

/// Steps 3–5: quarantine the verified partial (macOS), then for each candidate name
/// in `dir` persist the intent (`record(target, dev, inode)` writes `publishing`, the
/// target and the partial's inode in one write) and `hard_link`, which never replaces
/// an existing name. Returns the published target. The partial keeps its link until
/// the caller commits step 6 and calls [`finish`].
pub async fn publish(
    partial: &Path,
    dir: &Path,
    name: &str,
    quarantine_value: &str,
    mut record: impl AsyncFnMut(&Path, u64, u64) -> io::Result<()>,
) -> io::Result<PathBuf> {
    quarantine(&File::open(partial)?, quarantine_value)?;
    let (dev, inode) = inode_of(partial)?;
    let mut n = 1;
    loop {
        let target = dir.join(candidate(name, n));
        record(&target, dev, inode).await?;
        match std::fs::hard_link(partial, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// After step 6 committed: drop the partial's link; the target keeps the bytes.
pub fn finish(partial: &Path) -> io::Result<()> {
    remove_if_present(partial)
}

/// A failure at any step: remove `target` only if it is still our inode, then the partial.
pub fn abandon(partial: &Path, target: Option<(&Path, u64, u64)>) -> io::Result<()> {
    if let Some((target, dev, inode)) = target
        && inode_of(target).ok() == Some((dev, inode))
    {
        remove_if_present(target)?;
    }
    remove_if_present(partial)
}

fn remove_if_present(p: &Path) -> io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What a `publishing` row needs after a restart, resolved before `files/partial/` is
/// cleaned up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// The target is ours and the partial is here: commit step 6, then [`finish`].
    Finish,
    /// The partial is here but the target is not ours (or absent): [`publish`] again.
    Republish,
    /// The partial is gone (any target of ours was removed): download again.
    Requeue,
}

/// Resolve a `publishing` row whose recorded target is `target` and inode `(dev, inode)`.
/// A target with another inode is never removed and never adopted.
pub fn resolve(partial: &Path, target: &Path, dev: u64, inode: u64) -> io::Result<Restart> {
    let partial_here = partial.exists();
    let ours = inode_of(target).ok() == Some((dev, inode));
    Ok(match (ours, partial_here) {
        (true, true) => Restart::Finish,
        (true, false) => {
            remove_if_present(target)?;
            Restart::Requeue
        }
        (false, true) => Restart::Republish,
        (false, false) => Restart::Requeue,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    use super::*;

    fn write(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn refusal(r: Result<Egress, EgressError>) -> Refusal {
        r.expect_err("refused").refusal
    }

    fn mkfifo(p: &Path) {
        use std::os::unix::ffi::OsStrExt as _;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }

    #[test]
    fn gate_refuses_special_files_at_once() {
        let d = tempfile::tempdir().unwrap();
        let none = HashSet::new();
        let fifo = d.path().join("fifo");
        mkfifo(&fifo);
        let t = Instant::now();
        assert!(matches!(
            refusal(open_checked(&fifo, &none)),
            Refusal::NotRegular
        ));
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "a FIFO with no writer must not block"
        );
        let sock = d.path().join("sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert!(matches!(
            refusal(open_checked(&sock, &none)),
            Refusal::NotRegular
        ));
        assert!(matches!(
            refusal(open_checked(d.path(), &none)),
            Refusal::NotRegular
        ));
        assert!(matches!(
            refusal(open_checked(&d.path().join("nope"), &none)),
            Refusal::NotFound
        ));
        assert!(matches!(
            refusal(open_checked(Path::new("rel/x"), &none)),
            Refusal::NotAbsolute
        ));
        // 5,000,000,001 sparse bytes: over the cap, refused before a byte is read.
        let big = d.path().join("big");
        File::create(&big).unwrap().set_len(FILE_MAX + 1).unwrap();
        assert!(
            matches!(refusal(open_checked(&big, &none)), Refusal::TooLarge(n) if n == FILE_MAX + 1)
        );
        // A regular file passes, blocking again.
        let ok = d.path().join("ok.txt");
        write(&ok, b"hello");
        let e = open_checked(&ok, &none).unwrap();
        assert_eq!(e.size, 5);
        // SAFETY: the fd is open.
        let fl = unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&e.file), libc::F_GETFL) };
        assert_eq!(fl & libc::O_NONBLOCK, 0);
    }

    #[test]
    fn protected_set_refuses_every_listed_file_and_its_aliases() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mut listed: Vec<String> = PROTECTED_FILES.iter().map(|s| s.to_string()).collect();
        for store in PROTECTED_STORES {
            listed.extend(SQLITE_SIBLINGS.iter().map(|s| format!("{store}{s}")));
        }
        listed.extend(
            [
                "oauth/google.json",
                "oauth/mcp/linear",
                "codeindex/proj-1a2b3c4d.sqlite",
                "codeindex/proj-1a2b3c4d-map.sqlite-wal",
            ]
            .map(String::from),
        );
        for (i, rel) in listed.iter().enumerate() {
            write(&root.join(rel), format!("secret {i}").as_bytes());
        }
        let set = protected_set(&[&root]);
        for rel in &listed {
            assert!(
                matches!(
                    refusal(open_checked(&root.join(rel), &set)),
                    Refusal::Protected
                ),
                "{rel} must be refused"
            );
        }
        let identity = root.join("mesh/identity");
        let elsewhere = tempfile::tempdir().unwrap();
        // A symlink and a hardlink outside the root.
        let link = elsewhere.path().join("innocent.txt");
        std::os::unix::fs::symlink(&identity, &link).unwrap();
        assert!(matches!(
            refusal(open_checked(&link, &set)),
            Refusal::Protected
        ));
        let hard = root.join("files/hard");
        std::fs::create_dir_all(hard.parent().unwrap()).unwrap();
        std::fs::hard_link(&identity, &hard).unwrap();
        assert!(matches!(
            refusal(open_checked(&hard, &set)),
            Refusal::Protected
        ));
        // A case variant: the same inode on a case-insensitive volume, nothing otherwise.
        let variant = root.join("MESH/Identity");
        let r = refusal(open_checked(&variant, &set));
        if variant.exists() {
            assert!(matches!(r, Refusal::Protected));
        } else {
            assert!(matches!(r, Refusal::NotFound));
        }
        // A received store under the root is just a file.
        let research = root.join("files/received/maya/2026-11-17/research.sqlite");
        write(&research, b"SQLite format 3\0");
        assert!(open_checked(&research, &set).is_ok());
    }

    #[test]
    fn egress_open_uses_the_runtime_root() {
        let root = tempfile::tempdir().unwrap();
        let token = root.path().join("token");
        write(&token, b"bearer");
        assert!(matches!(
            refusal(egress_open(&token, root.path())),
            Refusal::Protected
        ));
        let note = root.path().join("note.md");
        write(&note, b"hi");
        assert!(egress_open(&note, root.path()).is_ok());
    }

    #[test]
    fn a_swapped_file_is_caught_on_reopen() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("report.pdf");
        write(&p, b"original");
        let e = egress_open(&p, root.path()).unwrap();
        let (dev, inode, size) = (e.dev, e.inode, e.size);
        assert!(egress_reopen(&p, root.path(), dev, inode, size).is_ok());
        let other = root.path().join("other");
        write(&other, b"original");
        std::fs::rename(&other, &p).unwrap();
        assert!(matches!(
            refusal(egress_reopen(&p, root.path(), dev, inode, size)),
            Refusal::Changed
        ));
        // The handle opened before the swap still reads the original.
        assert_eq!(
            blake3_of(&e.file, size).unwrap(),
            blake3::hash(b"original").to_hex().as_str()
        );
    }

    #[test]
    fn blake3_reads_the_declared_length_from_the_handle() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        write(&p, &data);
        let f = File::open(&p).unwrap();
        assert_eq!(
            blake3_of(&f, data.len() as u64).unwrap(),
            blake3::hash(&data).to_hex().as_str()
        );
        // A grown original is hashed at its declared length …
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"more")
            .unwrap();
        assert_eq!(
            blake3_of(&f, data.len() as u64).unwrap(),
            blake3::hash(&data).to_hex().as_str()
        );
        // … and a short one is an error, not a digest.
        let e = blake3_of(&f, data.len() as u64 + 100).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            blake3_of(&f, 0).unwrap(),
            blake3::hash(b"").to_hex().as_str()
        );
    }

    fn quarantine_of(p: &Path) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
            let mut buf = vec![0u8; 256];
            // SAFETY: valid path and buffer.
            let n = unsafe {
                libc::getxattr(
                    c.as_ptr(),
                    c"com.apple.quarantine".as_ptr(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                    0,
                )
            };
            usize::try_from(n)
                .ok()
                .map(|n| String::from_utf8_lossy(&buf[..n]).into_owned())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = p;
            None
        }
    }

    const Q: &str = "0081;6744f1a0;theuth;0193F0A2-6B1C-7D3E-9F40-1A2B3C4D5E6F";

    #[test]
    fn quarantine_value_matches_the_spec() {
        let id = Uuid::parse_str("0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f").unwrap();
        assert_eq!(quarantine_value(0x6744_f1a0, id), Q);
    }

    /// A publication with `record` noting each intent.
    async fn publish_into(partial: &Path, dir: &Path, name: &str) -> (PathBuf, Vec<PathBuf>) {
        let mut intents = Vec::new();
        let target = publish(partial, dir, name, Q, async |t: &Path, dev, ino| {
            assert_eq!(inode_of(partial).unwrap(), (dev, ino));
            // Step 3 happened before the intent and the link.
            if cfg!(target_os = "macos") {
                assert_eq!(quarantine_of(partial).as_deref(), Some(Q));
            }
            intents.push(t.to_path_buf());
            Ok(())
        })
        .await
        .unwrap();
        (target, intents)
    }

    fn partial(root: &Path, id: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(super::super::PARTIAL_DIR).join(id);
        write(&p, bytes);
        p
    }

    #[tokio::test]
    async fn publication_never_clobbers() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("files/received/maya/2026-11-17");
        std::fs::create_dir_all(&dir).unwrap();
        // An existing name with other bytes survives; ours takes the next name.
        write(&dir.join("report.pdf"), b"theirs");
        let p1 = partial(root.path(), "a", b"ours");
        let (t1, intents) = publish_into(&p1, &dir, "report.pdf").await;
        assert_eq!(t1, dir.join("report (2).pdf"));
        assert_eq!(
            intents,
            vec![dir.join("report.pdf"), dir.join("report (2).pdf")]
        );
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"theirs");
        assert_eq!(
            quarantine_of(&t1).as_deref(),
            cfg!(target_os = "macos").then_some(Q)
        );
        finish(&p1).unwrap();
        assert!(!p1.exists());
        assert_eq!(std::fs::read(&t1).unwrap(), b"ours");
        // An identical pre-existing file is never adopted.
        write(&dir.join("same.txt"), b"same");
        let p2 = partial(root.path(), "b", b"same");
        assert_eq!(
            publish_into(&p2, &dir, "same.txt").await.0,
            dir.join("same (2).txt")
        );
        // `Report.pdf` beside `report.pdf`: a name of its own on a case-insensitive
        // volume, the exact name on a case-sensitive one; never the other file.
        let p3 = partial(root.path(), "c", b"upper");
        let (t3, _) = publish_into(&p3, &dir, "Report.pdf").await;
        let case_insensitive = dir.join("REPORT.PDF").exists();
        assert_eq!(
            t3,
            dir.join(if case_insensitive {
                "Report (3).pdf"
            } else {
                "Report.pdf"
            })
        );
        // NFC and NFD of one name: separate files whatever the volume decides.
        let (nfc, nfd) = ("caf\u{e9}.txt", "cafe\u{301}.txt");
        let p4 = partial(root.path(), "d", b"nfc");
        let p5 = partial(root.path(), "e", b"nfd");
        let (t4, _) = publish_into(&p4, &dir, nfc).await;
        let (t5, _) = publish_into(&p5, &dir, nfd).await;
        assert_ne!(inode_of(&t4).unwrap(), inode_of(&t5).unwrap());
        assert_eq!(std::fs::read(&t4).unwrap(), b"nfc");
        assert_eq!(std::fs::read(&t5).unwrap(), b"nfd");
        // Every file kept its bytes.
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&t3).unwrap(), b"upper");
    }

    /// An existing `report.pdf` survives a crash after each step and each restart branch.
    #[tokio::test]
    async fn crash_after_each_step_keeps_the_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("files/received/maya/2026-11-17");
        let theirs = dir.join("report.pdf");
        write(&theirs, b"theirs");
        let theirs_ino = inode_of(&theirs).unwrap();
        let check = || {
            assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs");
            assert_eq!(inode_of(&theirs).unwrap(), theirs_ino);
        };
        // Steps 1–3 (streamed, verified, quarantined), no row yet: cleanup removes the partial.
        let p = partial(root.path(), "s1", b"ours");
        quarantine(&File::open(&p).unwrap(), Q).unwrap();
        abandon(&p, None).unwrap();
        assert!(!p.exists());
        check();
        // Step 4 recorded `report.pdf` (taken), crash before the link.
        let p = partial(root.path(), "s4", b"ours");
        let (dev, ino) = inode_of(&p).unwrap();
        assert_eq!(resolve(&p, &theirs, dev, ino).unwrap(), Restart::Republish);
        check();
        let (t, _) = publish_into(&p, &dir, "report.pdf").await;
        assert_eq!(t, dir.join("report (2).pdf"));
        check();
        // Step 5 linked, crash before step 6: ours, partial here → finish.
        assert_eq!(resolve(&p, &t, dev, ino).unwrap(), Restart::Finish);
        finish(&p).unwrap();
        check();
        // Step 6 committed and the partial removed: nothing left to resolve.
        assert_eq!(std::fs::read(&t).unwrap(), b"ours");
        // Ours linked but the partial vanished: our target goes, the download requeues.
        let p = partial(root.path(), "s5", b"again");
        let (dev, ino) = inode_of(&p).unwrap();
        let (t, _) = publish_into(&p, &dir, "report.pdf").await;
        assert_eq!(t, dir.join("report (3).pdf"));
        std::fs::remove_file(&p).unwrap();
        assert_eq!(resolve(&p, &t, dev, ino).unwrap(), Restart::Requeue);
        assert!(!t.exists());
        check();
        // The recorded target holds another inode and the partial is gone: requeue, keep it.
        assert_eq!(resolve(&p, &theirs, dev, ino).unwrap(), Restart::Requeue);
        check();
        // No target at all, partial here: publish again (step 5 retried).
        let p = partial(root.path(), "s6", b"late");
        let (dev, ino) = inode_of(&p).unwrap();
        let gone = dir.join("vanished.pdf");
        assert_eq!(resolve(&p, &gone, dev, ino).unwrap(), Restart::Republish);
        // A failure after the link removes only our inode.
        let (t, _) = publish_into(&p, &dir, "report.pdf").await;
        abandon(&p, Some((&t, dev, ino))).unwrap();
        assert!(!t.exists() && !p.exists());
        abandon(&p, Some((&theirs, dev, ino))).unwrap();
        check();
    }
}
