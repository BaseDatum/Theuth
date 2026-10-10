//! 158 W2 owns this file (stub from C158).
//!
//! The `cal_sources` / `cal_events` / `cal_project_memory` / `cal_prebrief` tables in
//! `meetings.sqlite` (158 §7.4, §9.3, §9.4; migration `0003_calendar.sql`, applied by
//! [`crate::Store::open`]). Queries go through [`crate::Store::pool`]. The stub runs no SQL.

use super::{CalEvent, CalSource};
use crate::Store;

/// Every source row.
pub async fn sources(_store: &Store) -> anyhow::Result<Vec<CalSource>> {
    Ok(Vec::new())
}

/// Replace `source`'s rows inside `window` (RFC 3339 bounds) with `events`; returns the rows
/// written. Stub: refused.
pub async fn replace_window(
    _store: &Store,
    _source: &str,
    _window: (&str, &str),
    _events: &[CalEvent],
) -> anyhow::Result<u32> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// Rows overlapping `[from, to]` (RFC 3339), `start` ascending. Stub: none.
pub async fn events(_store: &Store, _from: &str, _to: &str) -> anyhow::Result<Vec<CalEvent>> {
    Ok(Vec::new())
}
