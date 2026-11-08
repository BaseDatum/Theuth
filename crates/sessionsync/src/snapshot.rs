//! Snapshot bootstrap (68 §6, decision 7): a peer with no cursor for an
//! origin gets every current row of that origin's sessions as INSERT
//! changesets, one per attached table and chunk of rows, tagged with the
//! origin's log position they are current to.
//!
//! Build: inside one `Store::begin_apply` transaction (the write connection
//! is ours, nothing commits meanwhile, capture is off), every attached table
//! gets a `TEMP` twin with the same name, columns and primary key; a scratch
//! `sqlite3session` on the `temp` database watches one twin while the
//! filtered rows are copied in, and its changeset — named after the real
//! table — is the snapshot entry. The transaction is rolled back. Chosen over
//! `sqlite3session_diff` because `ATTACH` is refused inside a transaction and
//! the diff cannot filter by owner.
//!
//! Apply: every entry through `sqlite3changeset_apply` with the incoming row
//! winning, except rows of a session whose local fence epoch is higher than
//! the snapshot's `sessions` row says — those records are dropped through a
//! `sqlite3changegroup`. A snapshot larger than one frame travels as several
//! [`Msg::Snapshot`] parts followed by an empty terminator ([`split`]); the
//! cursor moves on the terminator only.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{self, NonNull};

use agent::store::{ATTACHED, ApplyTx, Store};
use libsqlite3_sys as ffi;
use replica::PeerId;
use sqlx::Row as _;

use crate::apply::apply_changeset;
use crate::types::{MAX_FRAME, Msg};

/// Rows per changeset entry.
const CHUNK_ROWS: i64 = 1000;
/// Payload per wire part: half a frame leaves room for postcard framing.
const PART_BYTES: usize = MAX_FRAME / 2;

/// One applied `sessions` record: id, owner, epoch, and the local fence
/// `(epoch, owner)` before it — `None` when the session was unknown here.
/// A snapshot that raises the epoch of a session this engine owned must
/// demote it like a log row does (68 §4.3; 139 T3: a partitioned owner
/// re-syncing from a pruned outbox).
pub type AppliedSession = (String, String, u32, Option<(u32, String)>);

/// Build the snapshot of `origin`'s sessions this engine holds.
pub async fn build(store: &Store, origin: &PeerId) -> anyhow::Result<Msg> {
    // ponytail: holds the store's write lock for the whole build (seconds on
    // a 100 MB store); a read connection with its own session would free it.
    let mut tx = store.begin_apply().await?;
    let current_to_seq = if origin.0 == store.engine() {
        u64::try_from(store.log_seq())?
    } else {
        store
            .cursors()
            .await?
            .into_iter()
            .find(|c| c.origin == origin.0 && c.peer.is_empty())
            .map_or(0, |c| u64::try_from(c.seq).unwrap_or(0))
    };
    let mut tables = Vec::new();
    for (table, _) in ATTACHED {
        let cols = table_info(&mut tx, table).await?;
        let pk: Vec<String> = {
            let mut keyed: Vec<&Column> = cols.iter().filter(|c| c.pk > 0).collect();
            keyed.sort_by_key(|c| c.pk);
            keyed.iter().map(|c| format!("\"{}\"", c.name)).collect()
        };
        anyhow::ensure!(!pk.is_empty(), "{table}: no primary key");
        let defs: Vec<String> = cols
            .iter()
            .map(|c| format!("\"{}\" {}", c.name, c.ty))
            .collect();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE TEMP TABLE \"{table}\" ({}, PRIMARY KEY ({}))",
            defs.join(", "),
            pk.join(", ")
        )))
        .execute(&mut *tx)
        .await?;
    }
    for (table, col) in ATTACHED {
        let filter = if table == "sessions" {
            "owner = ?".to_owned()
        } else {
            format!("\"{col}\" IN (SELECT id FROM main.sessions WHERE owner = ?)")
        };
        let mut last: i64 = 0;
        loop {
            let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT rowid FROM main.\"{table}\" WHERE rowid > ? AND {filter} ORDER BY rowid LIMIT {CHUNK_ROWS}"
            )))
            .bind(last)
            .bind(&origin.0)
            .fetch_all(&mut *tx)
            .await?;
            let (Some(&first), Some(&end)) = (ids.first(), ids.last()) else {
                break;
            };
            let scratch = tx
                .with_raw(|db| unsafe { Scratch::create(db, table) })
                .await??;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO temp.\"{table}\" SELECT * FROM main.\"{table}\" WHERE rowid BETWEEN ? AND ? AND {filter}"
            )))
            .bind(first)
            .bind(end)
            .bind(&origin.0)
            .execute(&mut *tx)
            .await?;
            let bytes = scratch.changeset()?;
            drop(scratch);
            if !bytes.is_empty() {
                tables.push((table.to_owned(), bytes));
            }
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM temp.\"{table}\"")))
                .execute(&mut *tx)
                .await?;
            last = end;
        }
    }
    for (table, _) in ATTACHED {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE temp.\"{table}\"")))
            .execute(&mut *tx)
            .await?;
    }
    drop(tx); // rollback: nothing of `main` changed
    Ok(Msg::Snapshot {
        origin: origin.clone(),
        current_to_seq,
        tables,
    })
}

/// Cut a snapshot into frame-sized parts, ending with an empty terminator
/// that carries the cursor. A snapshot that already fits is one part plus
/// the terminator.
pub fn split(snapshot: Msg) -> Vec<Msg> {
    let Msg::Snapshot {
        origin,
        current_to_seq,
        tables,
    } = snapshot
    else {
        return vec![snapshot];
    };
    let mut parts = Vec::new();
    let mut part: Vec<(String, Vec<u8>)> = Vec::new();
    let mut size = 0;
    for entry in tables {
        if !part.is_empty() && size + entry.1.len() > PART_BYTES {
            parts.push(Msg::Snapshot {
                origin: origin.clone(),
                current_to_seq,
                tables: std::mem::take(&mut part),
            });
            size = 0;
        }
        size += entry.1.len();
        part.push(entry);
    }
    if !part.is_empty() {
        parts.push(Msg::Snapshot {
            origin: origin.clone(),
            current_to_seq,
            tables: part,
        });
    }
    parts.push(Msg::Snapshot {
        origin,
        current_to_seq,
        tables: Vec::new(),
    });
    parts
}

/// Apply one part. `fenced` accumulates the sessions this snapshot may not
/// touch (their `sessions` record came in an earlier part); `move_cursor`
/// jumps `(origin, "")` to `current_to_seq` (never backwards) and raises the
/// fence of every session the snapshot owns. Returns the `sessions` records
/// applied by this part.
pub async fn apply(
    store: &Store,
    part: &Msg,
    fenced: &mut HashSet<String>,
    move_cursor: bool,
) -> anyhow::Result<Vec<AppliedSession>> {
    let Msg::Snapshot {
        origin,
        current_to_seq,
        tables,
    } = part
    else {
        anyhow::bail!("not a snapshot: {}", part.tag());
    };
    let mut tx = store.begin_apply().await?;
    let mut applied = Vec::new();
    let mut schemas: HashMap<String, Vec<Column>> = HashMap::new();
    for (table, changeset) in tables {
        let Some((_, col)) = ATTACHED.iter().find(|(t, _)| *t == table) else {
            anyhow::bail!("snapshot names an unreplicated table {table}");
        };
        if !schemas.contains_key(table) {
            schemas.insert(table.clone(), table_info(&mut tx, table).await?);
        }
        let cols = &schemas[table];
        let position = |name: &str| -> anyhow::Result<c_int> {
            Ok(c_int::try_from(
                cols.iter()
                    .position(|c| c.name == name)
                    .ok_or_else(|| anyhow::anyhow!("{table}.{name} missing"))?,
            )?)
        };
        let idx = position(col)?;
        if table == "sessions" {
            let local = local_fences(&mut tx).await?;
            for (id, own, ep) in
                scan_sessions(changeset, idx, position("owner")?, position("epoch")?)?
            {
                match local.get(&id) {
                    Some((e, _)) if *e > ep => {
                        fenced.insert(id);
                    }
                    before => applied.push((id, own, ep, before.cloned())),
                }
            }
        }
        let bytes = if fenced.is_empty() {
            Cow::Borrowed(changeset.as_slice())
        } else {
            Cow::Owned(filter(changeset, idx, fenced)?)
        };
        if !bytes.is_empty() {
            let skipped = apply_changeset(&mut tx, &bytes).await?;
            crate::apply::warn_skipped(origin, None, &skipped);
        }
    }
    if move_cursor {
        let seq = i64::try_from(*current_to_seq)?;
        sqlx::query(
            "INSERT INTO session_cursors (origin, peer, seq) VALUES (?, '', ?) \
             ON CONFLICT(origin, peer) DO UPDATE SET seq = max(session_cursors.seq, excluded.seq)",
        )
        .bind(&origin.0)
        .bind(seq)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO session_epochs (session_id, epoch, owner) \
             SELECT id, epoch, owner FROM sessions WHERE owner = ? \
             ON CONFLICT(session_id) DO UPDATE SET epoch = excluded.epoch, owner = excluded.owner \
             WHERE excluded.epoch > session_epochs.epoch",
        )
        .bind(&origin.0)
        .execute(&mut *tx)
        .await?;
        fenced.clear();
    }
    tx.commit().await?;
    Ok(applied)
}

/// `max(session_epochs, sessions row)` per session with its owner, as
/// `Store::fence` sees it.
async fn local_fences(tx: &mut ApplyTx<'_>) -> anyhow::Result<HashMap<String, (u32, String)>> {
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT id, epoch, owner FROM main.sessions \
         UNION ALL SELECT session_id, epoch, owner FROM main.session_epochs",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut out: HashMap<String, (u32, String)> = HashMap::new();
    for (id, epoch, owner) in rows {
        let e = u32::try_from(epoch).unwrap_or(u32::MAX);
        let slot = out.entry(id).or_insert((0, String::new()));
        if e >= slot.0 {
            *slot = (e, owner);
        }
    }
    Ok(out)
}

struct Column {
    name: String,
    ty: String,
    pk: i64,
}

async fn table_info(tx: &mut ApplyTx<'_>, table: &str) -> anyhow::Result<Vec<Column>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA main.table_info(\"{table}\")"
    )))
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .iter()
        .map(|r| Column {
            name: r.get("name"),
            ty: r.get("type"),
            pk: r.get("pk"),
        })
        .collect())
}

/// A `sqlite3session` on the `temp` database watching one table. `Send`
/// because it only lives while the store's write connection is held by the
/// transaction that created it.
struct Scratch(NonNull<ffi::sqlite3_session>);

unsafe impl Send for Scratch {}

impl Scratch {
    /// # Safety
    /// `db` must stay open for the session's lifetime.
    unsafe fn create(db: NonNull<ffi::sqlite3>, table: &str) -> anyhow::Result<Self> {
        let mut raw: *mut ffi::sqlite3_session = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3session_create(db.as_ptr(), c"temp".as_ptr(), &mut raw) };
        let ptr = NonNull::new(raw)
            .filter(|_| rc == ffi::SQLITE_OK)
            .ok_or_else(|| anyhow::anyhow!("sqlite3session_create(temp) failed (rc {rc})"))?;
        let name = CString::new(table)?;
        let rc = unsafe { ffi::sqlite3session_attach(ptr.as_ptr(), name.as_ptr()) };
        if rc != ffi::SQLITE_OK {
            unsafe { ffi::sqlite3session_delete(ptr.as_ptr()) };
            anyhow::bail!("sqlite3session_attach({table}) failed (rc {rc})");
        }
        Ok(Self(ptr))
    }

    fn changeset(&self) -> anyhow::Result<Vec<u8>> {
        let mut n: c_int = 0;
        let mut p: *mut c_void = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3session_changeset(self.0.as_ptr(), &mut n, &mut p) };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK,
            "sqlite3session_changeset failed (rc {rc})"
        );
        Ok(take_blob(p, n))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3session_delete(self.0.as_ptr()) };
    }
}

/// Copy an `sqlite3_malloc`ed buffer into a `Vec` and free it.
fn take_blob(p: *mut c_void, n: c_int) -> Vec<u8> {
    if p.is_null() || n <= 0 {
        return Vec::new();
    }
    let out = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), n as usize) }.to_vec();
    unsafe { ffi::sqlite3_free(p) };
    out
}

/// Owned changeset iterator; finalized on drop.
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

    fn next(&mut self) -> anyhow::Result<bool> {
        match unsafe { ffi::sqlite3changeset_next(self.0.as_ptr()) } {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => Ok(false),
            rc => anyhow::bail!("sqlite3changeset_next failed (rc {rc})"),
        }
    }

    fn table(&self) -> anyhow::Result<String> {
        let mut tab: *const c_char = ptr::null();
        let (mut ncol, mut op, mut indirect) = (0, 0, 0);
        let rc = unsafe {
            ffi::sqlite3changeset_op(self.0.as_ptr(), &mut tab, &mut ncol, &mut op, &mut indirect)
        };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK && !tab.is_null(),
            "sqlite3changeset_op failed (rc {rc})"
        );
        Ok(unsafe { CStr::from_ptr(tab) }
            .to_string_lossy()
            .into_owned())
    }

    /// Column `i` as text: new.* first (INSERT/UPDATE), then old.* (DELETE,
    /// or an unchanged column of an UPDATE); `None` when undefined or NULL.
    fn text(&self, i: c_int) -> Option<String> {
        for get in [ffi::sqlite3changeset_new, ffi::sqlite3changeset_old] {
            let mut v: *mut ffi::sqlite3_value = ptr::null_mut();
            let rc = unsafe { get(self.0.as_ptr(), i, &mut v) };
            if rc != ffi::SQLITE_OK || v.is_null() {
                continue;
            }
            unsafe {
                if ffi::sqlite3_value_type(v) == ffi::SQLITE_NULL {
                    return None;
                }
                let p = ffi::sqlite3_value_text(v);
                let n = ffi::sqlite3_value_bytes(v);
                if p.is_null() || n < 0 {
                    return None;
                }
                return Some(
                    String::from_utf8_lossy(std::slice::from_raw_parts(p, n as usize)).into_owned(),
                );
            }
        }
        None
    }
}

impl Drop for Iter {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3changeset_finalize(self.0.as_ptr()) };
    }
}

/// `(id, owner, epoch)` of every `sessions` record.
fn scan_sessions(
    changeset: &[u8],
    id: c_int,
    owner: c_int,
    epoch: c_int,
) -> anyhow::Result<Vec<(String, String, u32)>> {
    let mut it = Iter::start(changeset)?;
    let mut out = Vec::new();
    while it.next()? {
        if it.table()? != "sessions" {
            continue;
        }
        let Some(sid) = it.text(id) else { continue };
        let ep = it.text(epoch).and_then(|e| e.parse().ok()).unwrap_or(1);
        out.push((sid, it.text(owner).unwrap_or_default(), ep));
    }
    Ok(out)
}

/// `changeset` without the records whose column `col` names a session in
/// `fenced`, rebuilt through a changegroup.
fn filter(changeset: &[u8], col: c_int, fenced: &HashSet<String>) -> anyhow::Result<Vec<u8>> {
    let mut it = Iter::start(changeset)?;
    let mut grp: *mut ffi::sqlite3_changegroup = ptr::null_mut();
    let rc = unsafe { ffi::sqlite3changegroup_new(&mut grp) };
    anyhow::ensure!(
        rc == ffi::SQLITE_OK && !grp.is_null(),
        "sqlite3changegroup_new failed (rc {rc})"
    );
    let result = (|| {
        while it.next()? {
            if it.text(col).is_some_and(|s| fenced.contains(&s)) {
                continue;
            }
            let rc = unsafe { ffi::sqlite3changegroup_add_change(grp, it.0.as_ptr()) };
            anyhow::ensure!(
                rc == ffi::SQLITE_OK,
                "sqlite3changegroup_add_change failed (rc {rc})"
            );
        }
        let mut n: c_int = 0;
        let mut p: *mut c_void = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3changegroup_output(grp, &mut n, &mut p) };
        anyhow::ensure!(
            rc == ffi::SQLITE_OK,
            "sqlite3changegroup_output failed (rc {rc})"
        );
        Ok(take_blob(p, n))
    })();
    unsafe { ffi::sqlite3changegroup_delete(grp) };
    result
}
