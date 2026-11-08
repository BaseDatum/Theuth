//! Applying peers' rows to the local store (68 §6 "Apply"): a changeset goes
//! through `Store::begin_apply` (capture disabled, so nothing echoes into our
//! own log), `sqlite3changeset_apply` with owner-wins conflict handling,
//! then the cursor moves. The epoch fence is checked first.
//!
//! [`build_snapshot`] / [`apply_snapshot`] are the bootstrap path (68
//! decision 7); the contract commit leaves their bodies to worker L.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use agent::store::{ApplyTx, Store};
use libsqlite3_sys as ffi;
use replica::PeerId;

use crate::types::{LogRow, Msg};

/// What [`apply_log_row`] did with a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Rows written, cursor moved.
    Applied,
    /// `row.epoch` is below the fence for its session: a stale owner's row, dropped.
    StaleEpoch,
    /// Our cursor for `origin` is already at or past `row.seq`.
    Duplicate,
}

/// Apply one log row of `origin`. Idempotent under the fence: a duplicate or
/// a stale epoch changes nothing.
pub async fn apply_log_row(
    store: &Store,
    origin: &PeerId,
    row: &LogRow,
) -> anyhow::Result<Applied> {
    let session = row.session_id.parse()?;
    let before = store.fence(session).await?;
    if let Some((epoch, _owner)) = &before
        && row.epoch < *epoch
    {
        return Ok(Applied::StaleEpoch);
    }
    let applied = store
        .cursors()
        .await?
        .into_iter()
        .find(|c| c.origin == origin.0 && c.peer.is_empty())
        .map_or(0, |c| c.seq);
    if i64::try_from(row.seq)? <= applied {
        return Ok(Applied::Duplicate);
    }
    let mut tx = store.begin_apply().await?;
    let skipped = apply_changeset(&mut tx, &row.changeset).await?;
    warn_skipped(origin, Some(row.session_id.as_str()), &skipped);
    // The fence follows the session row: a take (`owner`, `epoch` moved) in
    // this changeset raises it; anything else is a no-op upsert.
    sqlx::query(
        "INSERT INTO session_epochs (session_id, epoch, owner) \
         SELECT id, epoch, owner FROM sessions WHERE id = ? \
         ON CONFLICT(session_id) DO UPDATE SET epoch = excluded.epoch, owner = excluded.owner \
         WHERE excluded.epoch > session_epochs.epoch",
    )
    .bind(&row.session_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO session_cursors (origin, peer, seq) VALUES (?, '', ?) \
         ON CONFLICT(origin, peer) DO UPDATE SET seq = excluded.seq",
    )
    .bind(&origin.0)
    .bind(i64::try_from(row.seq)?)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    // 68 §4.3: a row that raised the epoch of a session we owned demotes us (O).
    crate::owner::on_applied(store, origin, session, before, row.epoch).await;
    Ok(Applied::Applied)
}

/// A table of a changeset that `sqlite3changeset_apply` skips because the
/// local table has fewer columns (or does not exist): its rows are not
/// applied, with no error. 183 R1 (E-M3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Skipped {
    pub table: String,
    /// Columns in the changeset.
    pub changeset_cols: i64,
    /// Columns of the local table (0 = no such table).
    pub local_cols: i64,
}

/// Warn, loudly, for every table [`apply_changeset`] skipped. `session` is
/// the log row's session, `None` for a snapshot part.
pub(crate) fn warn_skipped(origin: &PeerId, session: Option<&str>, skipped: &[Skipped]) {
    for s in skipped {
        tracing::warn!(
            "sessions: apply SKIPPED table {} of session {} from {origin}: its changeset has {} columns, this store's table {}; \
             those rows are lost here until the schema matches and the origin re-sends them",
            s.table,
            session.unwrap_or("(snapshot)"),
            s.changeset_cols,
            if s.local_cols == 0 {
                "does not exist".to_owned()
            } else {
                format!("has {}", s.local_cols)
            },
        );
    }
}

/// `sqlite3changeset_apply` of `changeset` inside `tx` with [`owner_wins`].
/// Returns the tables SQLite skipped (more columns in the changeset than in
/// the local table: the apply still reports success).
pub(crate) async fn apply_changeset(
    tx: &mut ApplyTx<'_>,
    changeset: &[u8],
) -> anyhow::Result<Vec<Skipped>> {
    let mut skipped = Vec::new();
    for (table, cols) in changeset_tables(changeset)? {
        let local: i64 = sqlx::query_scalar("SELECT count(*) FROM pragma_table_info(?, 'main')")
            .bind(&table)
            .fetch_one(&mut **tx)
            .await?;
        if local < i64::from(cols) {
            skipped.push(Skipped {
                table,
                changeset_cols: i64::from(cols),
                local_cols: local,
            });
        }
    }
    let rc = tx
        .with_raw(|db| unsafe {
            ffi::sqlite3changeset_apply(
                db.as_ptr(),
                c_int::try_from(changeset.len()).unwrap_or(c_int::MAX),
                changeset.as_ptr().cast::<c_void>().cast_mut(),
                None,
                Some(owner_wins),
                ptr::null_mut(),
            )
        })
        .await?;
    anyhow::ensure!(
        rc == ffi::SQLITE_OK,
        "sqlite3changeset_apply failed (rc {rc})"
    );
    Ok(skipped)
}

/// `(table, columns)` of each table `changeset` touches, in order (a table's
/// records are contiguous).
fn changeset_tables(changeset: &[u8]) -> anyhow::Result<Vec<(String, c_int)>> {
    let mut it: *mut ffi::sqlite3_changeset_iter = ptr::null_mut();
    let rc = unsafe {
        ffi::sqlite3changeset_start(
            &mut it,
            c_int::try_from(changeset.len())?,
            changeset.as_ptr().cast::<c_void>().cast_mut(),
        )
    };
    anyhow::ensure!(
        rc == ffi::SQLITE_OK && !it.is_null(),
        "sqlite3changeset_start failed (rc {rc})"
    );
    let mut out: Vec<(String, c_int)> = Vec::new();
    let result = loop {
        match unsafe { ffi::sqlite3changeset_next(it) } {
            ffi::SQLITE_ROW => {}
            ffi::SQLITE_DONE => break Ok(()),
            rc => break Err(anyhow::anyhow!("sqlite3changeset_next failed (rc {rc})")),
        }
        let mut tab: *const c_char = ptr::null();
        let (mut cols, mut op, mut indirect) = (0, 0, 0);
        let rc =
            unsafe { ffi::sqlite3changeset_op(it, &mut tab, &mut cols, &mut op, &mut indirect) };
        if rc != ffi::SQLITE_OK || tab.is_null() {
            break Err(anyhow::anyhow!("sqlite3changeset_op failed (rc {rc})"));
        }
        let name = unsafe { CStr::from_ptr(tab) }.to_string_lossy();
        if out.last().is_none_or(|(t, _)| *t != name) {
            out.push((name.into_owned(), cols));
        }
    };
    unsafe { ffi::sqlite3changeset_finalize(it) };
    result.map(|()| out)
}

/// Conflict handler: the owner wrote it, so its row wins (`REPLACE` on
/// `DATA`/`CONFLICT`); a delete of a row we never had is nothing (`OMIT`).
unsafe extern "C" fn owner_wins(
    _ctx: *mut c_void,
    conflict: c_int,
    _iter: *mut ffi::sqlite3_changeset_iter,
) -> c_int {
    match conflict {
        ffi::SQLITE_CHANGESET_DATA | ffi::SQLITE_CHANGESET_CONFLICT => {
            ffi::SQLITE_CHANGESET_REPLACE
        }
        _ => ffi::SQLITE_CHANGESET_OMIT,
    }
}

/// Every current row this engine holds for `origin`, one changeset per
/// attached table and chunk of rows, tagged with `current_to_seq` of that
/// origin's log (68 §6). May exceed one frame: ship it through
/// [`crate::snapshot::split`].
pub async fn build_snapshot(store: &Store, origin: &PeerId) -> anyhow::Result<Msg> {
    crate::snapshot::build(store, origin).await
}

/// Apply a whole [`Msg::Snapshot`] in one transaction: every table's
/// changeset through [`Store::begin_apply`] with the incoming row winning,
/// except rows of a session whose local fence epoch is higher, then the
/// cursor for its origin jumps to `current_to_seq`.
pub async fn apply_snapshot(store: &Store, snapshot: &Msg) -> anyhow::Result<()> {
    crate::snapshot::apply(store, snapshot, &mut std::collections::HashSet::new(), true)
        .await
        .map(drop)
}
