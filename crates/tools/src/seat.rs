//! Where a session sits (140 F4E): the working directory the file tools and
//! the session shell resolve relative paths against, and — for a confined
//! session such as an autoresearch study — the only directory the writing
//! tools may touch.
//!
//! Process-wide and keyed by session id, like the schedule's confinement
//! registry: the schedule seats a session from its file's `cwd:` on every
//! fire, a plugin confines writes beside `SetToolset`, and the tool registry
//! copies a parent's seat to a new subagent ([`crate::registry::SetParent`]).
//! A session without a seat behaves as before: relative paths follow the
//! process cwd and nothing is refused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use runtime::SessionId;

use crate::jobs::lock;

/// One session's seat; both halves are independent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seat {
    /// Relative tool paths resolve here; the session shell starts here.
    pub cwd: Option<PathBuf>,
    /// `write_file` / `edit` refuse any path outside this directory.
    pub write_root: Option<PathBuf>,
}

static SEATS: LazyLock<Mutex<HashMap<SessionId, Seat>>> = LazyLock::new(Default::default);

/// Set (or with `None`, drop) the working directory of `session`.
pub fn set_cwd(session: SessionId, cwd: Option<PathBuf>) {
    update(session, |s| s.cwd = cwd);
}

/// Confine `session`'s writing tools to `root` (`None` lifts it).
pub fn set_write_root(session: SessionId, root: Option<PathBuf>) {
    update(session, |s| s.write_root = root);
}

fn update(session: SessionId, f: impl FnOnce(&mut Seat)) {
    let mut seats = lock(&SEATS);
    let seat = seats.entry(session).or_default();
    f(seat);
    if *seat == Seat::default() {
        seats.remove(&session);
    }
}

/// The seat of `session` (default when it has none).
pub fn seat_of(session: SessionId) -> Seat {
    lock(&SEATS).get(&session).cloned().unwrap_or_default()
}

/// A new subagent sits where its parent sits, unless it was seated first.
pub fn inherit(child: SessionId, parent: SessionId) {
    let mut seats = lock(&SEATS);
    if let Some(p) = seats.get(&parent).cloned() {
        seats.entry(child).or_insert(p);
    }
}

/// Forget `session`'s seat (tree close).
pub fn forget(session: SessionId) {
    lock(&SEATS).remove(&session);
}

/// `path` as the tools open it: joined to the session's cwd when relative
/// and a cwd is seated; unchanged otherwise (the OS resolves it against the
/// process cwd, as before).
pub fn resolve(session: SessionId, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match seat_of(session).cwd {
        Some(cwd) => cwd.join(path),
        None => path.to_path_buf(),
    }
}

/// The directory a tool with no `path` argument searches: the seated cwd,
/// else the process cwd.
pub fn cwd(session: SessionId) -> std::io::Result<PathBuf> {
    match seat_of(session).cwd {
        Some(cwd) => Ok(cwd),
        None => std::env::current_dir(),
    }
}

/// [`resolve`], then refuse the path when the session has a write root and
/// the path is not under it. Symlinks are followed on both sides, so a link
/// inside the root pointing out is refused and a root under a linked prefix
/// (`/tmp` on macOS) is not.
pub fn writable(session: SessionId, path: &Path) -> anyhow::Result<PathBuf> {
    let resolved = resolve(session, path);
    let Some(root) = seat_of(session).write_root else {
        return Ok(resolved);
    };
    let refuse = || {
        anyhow::anyhow!(
            "{}: outside this session's write root {} — this session may only write inside it",
            path.display(),
            root.display()
        )
    };
    let real_root = root.canonicalize().map_err(|_| refuse())?;
    match real(&resolved) {
        Some(p) if p.starts_with(&real_root) => Ok(resolved),
        _ => Err(refuse()),
    }
}

/// `path` with its longest existing prefix canonicalised and the rest (a
/// file about to be created) appended; `None` when the rest holds `..` or
/// nothing of the path exists.
fn real(path: &Path) -> Option<PathBuf> {
    let mut tail = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(mut out) = cur.canonicalize() {
            out.extend(tail.iter().rev());
            return Some(out);
        }
        let name = cur.file_name()?;
        tail.push(name.to_owned());
        cur = cur.parent()?;
    }
}

/// 171F B24: what `read_file`, `grep`, `glob`, `edit` and `write_file` answer
/// on a user cell's run record — `private` and the credential masks apply
/// only through the cell tools.
pub const CELL_RECORD: &str =
    "this file belongs to a user cell's run record — read results with cell_runs / cell_output";

/// The engine's data dir (set at plugin start): a private session's
/// resource root is a scratch dir, while cell records live in the data dir.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record the engine's data dir for [`refuse_cell_record`].
pub fn set_data_dir(dir: &Path) {
    let _ = DATA_DIR.set(dir.to_path_buf());
}

/// `root` (the session's resource root) and the engine's data dir, canonical.
fn data_dirs(root: &Path) -> Vec<PathBuf> {
    std::iter::once(root)
        .chain(DATA_DIR.get().map(PathBuf::as_path))
        .filter_map(|d| d.canonicalize().ok())
        .collect()
}

/// A canonical `path` is a cell record: under `<data dir>/cells/` (the cells
/// crate's dir) or a kernel job's log under `<data dir>/jobs/`.
fn held(dirs: &[PathBuf], path: &Path) -> bool {
    let kernel_log = || {
        path.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(crate::jobs::CELL_LOG))
    };
    dirs.iter().any(|d| {
        path.starts_with(d.join("cells")) || (path.starts_with(d.join("jobs")) && kernel_log())
    })
}

/// Refuse `path` (`..` and symlinks followed) when it is a user cell's run
/// record ([`CELL_RECORD`]).
pub fn refuse_cell_record(root: &Path, path: &Path) -> anyhow::Result<()> {
    match real(path) {
        Some(p) if held(&data_dirs(root), &p) => {
            anyhow::bail!("{}: {CELL_RECORD}", path.display())
        }
        _ => Ok(()),
    }
}

/// For a walk of `walk_root` (not itself a record): is entry `path` (under
/// `walk_root`, symlinks not followed by the walk) a cell record?
pub fn cell_record_filter(
    root: &Path,
    walk_root: &Path,
) -> impl Fn(&Path) -> bool + Send + Sync + use<> {
    let dirs = data_dirs(root);
    let walk_root = walk_root.to_path_buf();
    let real_root = real(&walk_root);
    move |path| {
        let (Some(real_root), Ok(rel)) = (&real_root, path.strip_prefix(&walk_root)) else {
            return false;
        };
        held(&dirs, &real_root.join(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_follow_the_seated_cwd_and_nothing_else_changes() {
        let s = SessionId::new();
        assert_eq!(resolve(s, Path::new("a/b")), PathBuf::from("a/b"));
        set_cwd(s, Some("/work".into()));
        assert_eq!(resolve(s, Path::new("a/b")), PathBuf::from("/work/a/b"));
        assert_eq!(resolve(s, Path::new("/abs")), PathBuf::from("/abs"));
        assert_eq!(cwd(s).unwrap(), PathBuf::from("/work"));
        set_cwd(s, None);
        assert_eq!(seat_of(s), Seat::default());
        assert_eq!(resolve(s, Path::new("a/b")), PathBuf::from("a/b"));
    }

    #[test]
    fn a_child_inherits_its_parents_seat_once() {
        let (p, c) = (SessionId::new(), SessionId::new());
        set_cwd(p, Some("/p".into()));
        set_write_root(p, Some("/p".into()));
        inherit(c, p);
        assert_eq!(seat_of(c), seat_of(p));
        set_write_root(c, Some("/p/cand-1".into()));
        inherit(c, p);
        assert_eq!(
            seat_of(c).write_root.as_deref(),
            Some(Path::new("/p/cand-1"))
        );
        forget(c);
        forget(p);
        assert_eq!(seat_of(c), Seat::default());
    }

    #[test]
    fn writes_stay_inside_the_root() {
        // Drop guard (141 R4): the scratch dir goes when the test ends, pass or fail.
        let scratch = crate::TestDir::new("seat");
        let dir = scratch.0.clone();
        let root = dir.join("wt");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        let outside = dir.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("o.rs"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("o.rs"), root.join("link.rs")).unwrap();

        let s = SessionId::new();
        set_cwd(s, Some(root.clone()));
        set_write_root(s, Some(root.clone()));
        // Existing, new, new under a new dir: all inside.
        assert!(writable(s, Path::new("src/a.rs")).is_ok());
        assert!(writable(s, Path::new("src/new.rs")).is_ok());
        assert!(writable(s, Path::new("deep/er/new.rs")).is_ok());
        assert!(writable(s, &root.join("src/a.rs")).is_ok());
        // Outside by absolute path, by `..`, through a symlink, or unresolvable.
        for p in [
            outside.join("o.rs"),
            PathBuf::from("../elsewhere/o.rs"),
            PathBuf::from("src/../../elsewhere/new.rs"),
            PathBuf::from("nope/../../x"),
        ] {
            let e = writable(s, &p).unwrap_err().to_string();
            assert!(
                e.contains("outside this session's write root"),
                "{p:?}: {e}"
            );
            assert!(e.contains(&root.display().to_string()), "{e}");
        }
        #[cfg(unix)]
        assert!(writable(s, Path::new("link.rs")).is_err());
        // Without a root the same paths pass.
        set_write_root(s, None);
        assert!(writable(s, &outside.join("o.rs")).is_ok());
        forget(s);
    }
}
