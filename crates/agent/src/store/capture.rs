//! Change capture on the store's one write connection (68 §3): a
//! `sqlite3session` attached to the transcript tables, created in the write
//! pool's `after_connect` and deleted before that connection can close. The
//! engine-local tables (`projects`, `cost_owners`, `peer_*`, `a2a_*`; 68
//! decision 8) are never attached, so their writes leave no trace here.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{self, NonNull};

use libsqlite3_sys as ffi;

/// Every replicated table with the column that names its session
/// (`sessions` itself by its primary key; the accounting tables by their
/// `cost_owners.id`, which is the session id; `session_advisors` by the
/// advised session, 125 §4.14). The 109 journal tables ride here too
/// (§3.5); `pty_screens` joined them in 139 §5.3 so a taker has the
/// owner's last page, and is fenced like every other attached table; the
/// 137 endless tables ride by `session_id`.
pub const ATTACHED: [(&str, &str); 20] = [
    ("sessions", "id"),
    ("messages", "session_id"),
    ("epochs", "session_id"),
    ("todo_snapshots", "session_id"),
    ("errors", "session_id"),
    ("inner_calls", "session_id"),
    ("reminders", "session"),
    ("model_calls", "owner_id"),
    ("remote_meters", "owner_id"),
    ("turn_steps", "session_id"),
    ("tool_calls", "session_id"),
    ("questions", "session_id"),
    ("waits", "session_id"),
    ("jobs", "session_id"),
    ("pty_screens", "session_id"),
    ("resume_reports", "session_id"),
    ("session_advisors", "session"),
    ("endless_units", "session_id"),
    ("endless_nodes", "session_id"),
    ("endless_calls", "session_id"),
];

/// One `sqlite3_session` on one database handle. `Send` because the store
/// only touches it while holding the write connection (the pool has exactly
/// one), never from two threads at once.
pub(crate) struct Session {
    db: NonNull<ffi::sqlite3>,
    ptr: NonNull<ffi::sqlite3_session>,
}

unsafe impl Send for Session {}

impl Session {
    /// Create a session on `db` and attach [`ATTACHED`].
    ///
    /// # Safety
    /// `db` must be an open handle that outlives the returned value (the
    /// store deletes the session before the connection closes).
    pub(crate) unsafe fn create(db: NonNull<ffi::sqlite3>) -> anyhow::Result<Self> {
        let mut raw: *mut ffi::sqlite3_session = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3session_create(db.as_ptr(), c"main".as_ptr(), &mut raw) };
        let ptr = NonNull::new(raw)
            .filter(|_| rc == ffi::SQLITE_OK)
            .ok_or_else(|| anyhow::anyhow!("sqlite3session_create failed (rc {rc})"))?;
        let session = Self { db, ptr };
        for (table, _) in ATTACHED {
            let name = CString::new(table).expect("static table name");
            let rc = unsafe { ffi::sqlite3session_attach(ptr.as_ptr(), name.as_ptr()) };
            anyhow::ensure!(
                rc == ffi::SQLITE_OK,
                "sqlite3session_attach({table}) failed (rc {rc})"
            );
        }
        Ok(session)
    }

    /// The changes recorded since creation (or the last [`Session::reset`]),
    /// as one changeset; empty when nothing was written.
    pub(crate) fn changeset(&self) -> anyhow::Result<Vec<u8>> {
        let mut n: std::os::raw::c_int = 0;
        let mut p: *mut std::os::raw::c_void = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3session_changeset(self.ptr.as_ptr(), &mut n, &mut p) };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK,
            "sqlite3session_changeset failed (rc {rc})"
        );
        if p.is_null() || n <= 0 {
            return Ok(Vec::new());
        }
        let bytes = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), n as usize) }.to_vec();
        unsafe { ffi::sqlite3_free(p) };
        Ok(bytes)
    }

    /// Forget every recorded change: the extension has no reset, so this
    /// deletes and recreates the session on the same handle. The caller holds
    /// the connection's handle lock (`WriteTx::commit`), so the worker thread
    /// is idle and the delete cannot race the preupdate hook.
    pub(crate) fn reset(&mut self) -> anyhow::Result<()> {
        let fresh = unsafe { Self::create(self.db) }?;
        // Delete the old session after the new one exists so the handle never
        // loses its preupdate hook in between.
        let old = std::mem::replace(self, fresh);
        unsafe { ffi::sqlite3session_delete(old.ptr.as_ptr()) };
        std::mem::forget(old);
        Ok(())
    }

    /// Stop (or resume) recording: a remote changeset applied while disabled
    /// leaves no echo in the log (68 §6 "detached").
    pub(crate) fn enable(&self, on: bool) {
        unsafe { ffi::sqlite3session_enable(self.ptr.as_ptr(), c_int::from(on)) };
    }
}

/// Column layout of one attached table as `PRAGMA table_info` reports it,
/// which is the column order a changeset uses.
#[derive(Clone, Debug)]
pub(crate) struct TableMeta {
    pub(crate) name: String,
    pub(crate) cols: Vec<String>,
    pub(crate) pk: Vec<bool>,
    /// Index of the column naming the session (see [`ATTACHED`]).
    pub(crate) session_col: usize,
}

impl TableMeta {
    pub(crate) fn col(&self, name: &str) -> Option<usize> {
        self.cols.iter().position(|c| c == name)
    }

    /// The first primary-key column (every attached table's PK starts with
    /// a TEXT id).
    pub(crate) fn pk_col(&self) -> usize {
        self.pk.iter().position(|&p| p).unwrap_or(0)
    }
}

/// One operation of a changeset, as far as grouping needs it.
#[derive(Debug)]
pub(crate) struct Op {
    /// Index into the table list handed to [`scan`].
    pub(crate) table: usize,
    /// The session id when the record carries it; `None` for an UPDATE whose
    /// session column is not part of the key (undefined in old.* and new.*).
    pub(crate) session: Option<String>,
    /// The first PK value as text, to look the session up when `session` is `None`.
    pub(crate) pk: String,
    /// `(owner, epoch)` of a deleted `sessions` row: the post-state has no row
    /// left to ask, and the fence needs both.
    pub(crate) deleted_session: Option<(String, i64)>,
}

/// Owned iterator over a changeset; finalized on drop.
struct Iter(NonNull<ffi::sqlite3_changeset_iter>);

impl Iter {
    fn start(changeset: &[u8]) -> anyhow::Result<Self> {
        let mut raw: *mut ffi::sqlite3_changeset_iter = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3changeset_start(
                &mut raw,
                c_int::try_from(changeset.len())?,
                changeset.as_ptr().cast::<c_void>().cast_mut(),
            )
        };
        NonNull::new(raw)
            .filter(|_| rc == ffi::SQLITE_OK)
            .map(Self)
            .ok_or_else(|| anyhow::anyhow!("sqlite3changeset_start failed (rc {rc})"))
    }

    /// Advance; `false` at the end.
    fn next(&mut self) -> anyhow::Result<bool> {
        match unsafe { ffi::sqlite3changeset_next(self.0.as_ptr()) } {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => Ok(false),
            rc => anyhow::bail!("sqlite3changeset_next failed (rc {rc})"),
        }
    }

    /// `(table, op)` of the current record; `op` is `SQLITE_INSERT`,
    /// `SQLITE_UPDATE` or `SQLITE_DELETE`.
    fn op(&self) -> anyhow::Result<(String, c_int)> {
        let mut tab: *const c_char = ptr::null();
        let (mut ncol, mut op, mut indirect) = (0, 0, 0);
        let rc = unsafe {
            ffi::sqlite3changeset_op(self.0.as_ptr(), &mut tab, &mut ncol, &mut op, &mut indirect)
        };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK && !tab.is_null(),
            "sqlite3changeset_op failed (rc {rc})"
        );
        let name = unsafe { CStr::from_ptr(tab) }
            .to_string_lossy()
            .into_owned();
        Ok((name, op))
    }

    /// Column `i` of the record's old.* (`old = true`) or new.* values;
    /// `None` when undefined or NULL.
    fn value(&self, old: bool, i: usize) -> Option<Value> {
        let mut v: *mut ffi::sqlite3_value = ptr::null_mut();
        let f = if old {
            ffi::sqlite3changeset_old
        } else {
            ffi::sqlite3changeset_new
        };
        let rc = unsafe { f(self.0.as_ptr(), c_int::try_from(i).ok()?, &mut v) };
        if rc != ffi::SQLITE_OK || v.is_null() {
            return None;
        }
        unsafe {
            match ffi::sqlite3_value_type(v) {
                ffi::SQLITE_INTEGER => Some(Value::Int(ffi::sqlite3_value_int64(v))),
                ffi::SQLITE_NULL => None,
                _ => {
                    let p = ffi::sqlite3_value_text(v);
                    let n = ffi::sqlite3_value_bytes(v);
                    if p.is_null() || n < 0 {
                        return None;
                    }
                    let bytes = std::slice::from_raw_parts(p, n as usize);
                    Some(Value::Text(String::from_utf8_lossy(bytes).into_owned()))
                }
            }
        }
    }
}

impl Drop for Iter {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3changeset_finalize(self.0.as_ptr()) };
    }
}

enum Value {
    Int(i64),
    Text(String),
}

impl Value {
    fn text(self) -> String {
        match self {
            Self::Int(i) => i.to_string(),
            Self::Text(t) => t,
        }
    }

    fn int(self) -> i64 {
        match self {
            Self::Int(i) => i,
            Self::Text(t) => t.parse().unwrap_or_default(),
        }
    }
}

/// Pass one over a changeset: one [`Op`] per record, in iterator order.
/// INSERTs carry the session in new.*, DELETEs and keyed UPDATEs in old.*;
/// an UPDATE whose session column is not in the key leaves `session` unset
/// for the caller to resolve by `pk`.
pub(crate) fn scan(changeset: &[u8], tables: &[TableMeta]) -> anyhow::Result<Vec<Op>> {
    let mut it = Iter::start(changeset)?;
    let mut ops = Vec::new();
    while it.next()? {
        let (name, op) = it.op()?;
        let table = tables
            .iter()
            .position(|t| t.name == name)
            .ok_or_else(|| anyhow::anyhow!("changeset touches unattached table {name}"))?;
        let meta = &tables[table];
        let old = op != ffi::SQLITE_INSERT;
        let pk = it
            .value(old, meta.pk_col())
            .map(Value::text)
            .unwrap_or_default();
        let session = if op == ffi::SQLITE_UPDATE && !meta.pk[meta.session_col] {
            None
        } else {
            it.value(old, meta.session_col).map(Value::text)
        };
        let deleted_session = (op == ffi::SQLITE_DELETE && meta.name == "sessions")
            .then(|| {
                let owner = meta
                    .col("owner")
                    .and_then(|c| it.value(true, c))
                    .map(Value::text);
                let epoch = meta
                    .col("epoch")
                    .and_then(|c| it.value(true, c))
                    .map(Value::int);
                Some((owner?, epoch?))
            })
            .flatten();
        ops.push(Op {
            table,
            session,
            pk,
            deleted_session,
        });
    }
    Ok(ops)
}

/// Pass two: re-emit `changeset` as one changeset per session. `sessions[i]`
/// is the session of the i-th record (as [`scan`] ordered them, resolved).
/// Uses `sqlite3changegroup_add_change` (SQLite ≥ 3.44; vendored 3.50.4),
/// so every row goes to exactly one output.
pub(crate) fn split(
    changeset: &[u8],
    sessions: &[String],
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    let mut it = Iter::start(changeset)?;
    let mut i = 0;
    while it.next()? {
        let sid = sessions
            .get(i)
            .ok_or_else(|| anyhow::anyhow!("changeset longer than its scan"))?;
        i += 1;
        let group = match groups.iter_mut().position(|(s, _)| s == sid) {
            Some(k) => &mut groups[k].1,
            None => {
                groups.push((sid.clone(), Group::new()?));
                &mut groups.last_mut().expect("just pushed").1
            }
        };
        let rc = unsafe { ffi::sqlite3changegroup_add_change(group.0.as_ptr(), it.0.as_ptr()) };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK,
            "sqlite3changegroup_add_change failed (rc {rc})"
        );
    }
    groups
        .into_iter()
        .map(|(sid, g)| Ok((sid, g.output()?)))
        .collect()
}

struct Group(NonNull<ffi::sqlite3_changegroup>);

impl Group {
    fn new() -> anyhow::Result<Self> {
        let mut raw: *mut ffi::sqlite3_changegroup = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3changegroup_new(&mut raw) };
        NonNull::new(raw)
            .filter(|_| rc == ffi::SQLITE_OK)
            .map(Self)
            .ok_or_else(|| anyhow::anyhow!("sqlite3changegroup_new failed (rc {rc})"))
    }

    fn output(&self) -> anyhow::Result<Vec<u8>> {
        let mut n: c_int = 0;
        let mut p: *mut c_void = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3changegroup_output(self.0.as_ptr(), &mut n, &mut p) };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK,
            "sqlite3changegroup_output failed (rc {rc})"
        );
        if p.is_null() || n <= 0 {
            return Ok(Vec::new());
        }
        let bytes = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), n as usize) }.to_vec();
        unsafe { ffi::sqlite3_free(p) };
        Ok(bytes)
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3changegroup_delete(self.0.as_ptr()) };
    }
}

impl Drop for Session {
    /// Deliberately leaks the session (a few KB, once per store lifetime).
    /// Statements run on sqlx's worker thread, which cannot be stopped
    /// mid-`sqlite3_step`; a `Store` dropped while the worker is still inside
    /// the preupdate hook (a runtime shutting down, a background pump write)
    /// would otherwise `sqlite3session_delete` under the hook's feet — a
    /// use-after-free seen as `SIGSEGV` in `sessionPreupdateOneChange`. The
    /// only deletes are in [`Session::reset`], under the handle lock.
    fn drop(&mut self) {}
}
