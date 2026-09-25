//! Applying peers' rows to the local store (68 §6 "Apply"): a changeset goes
//! through `Store::begin_apply` (capture disabled, so nothing echoes into our
//! own log), `sqlite3changeset_apply` with owner-wins conflict handling,
//! then the cursor moves. The epoch fence is checked first.
//!
//! [`build_snapshot`] / [`apply_snapshot`] are the bootstrap path (68
//! decision 7); the contract commit leaves their bodies to worker L.

use std::ffi::{c_int, c_void};
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
pub async fn apply_log_row(store: &Store, origin: &PeerId, row: &LogRow) -> anyhow::Result<Applied> {
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
    apply_changeset(&mut tx, &row.changeset).await?;
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

/// `sqlite3changeset_apply` of `changeset` inside `tx` with [`owner_wins`].
pub(crate) async fn apply_changeset(tx: &mut ApplyTx<'_>, changeset: &[u8]) -> anyhow::Result<()> {
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
    anyhow::ensure!(rc == ffi::SQLITE_OK, "sqlite3changeset_apply failed (rc {rc})");
    Ok(())
}

/// Conflict handler: the owner wrote it, so its row wins (`REPLACE` on
/// `DATA`/`CONFLICT`); a delete of a row we never had is nothing (`OMIT`).
unsafe extern "C" fn owner_wins(_ctx: *mut c_void, conflict: c_int, _iter: *mut ffi::sqlite3_changeset_iter) -> c_int {
    match conflict {
        ffi::SQLITE_CHANGESET_DATA | ffi::SQLITE_CHANGESET_CONFLICT => ffi::SQLITE_CHANGESET_REPLACE,
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
