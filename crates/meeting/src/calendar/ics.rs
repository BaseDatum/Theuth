//! 158 W2 owns this file (stub from C158).
//!
//! ICS (158 §7.3, W-d): a bounded VCALENDAR line reader (unfold, split, VEVENT / VTIMEZONE,
//! `RECURRENCE-ID` overrides) plus `rrule` expansion with IANA zones (Windows zone names mapped
//! by a bounded table). Fixtures under `crates/meeting/tests/fixtures/calendar/` (§7.7).

use chrono::{DateTime, Utc};

use super::CalEvent;

/// Parse `text` (one VCALENDAR) and expand every VEVENT into the instances that overlap
/// `[from, to]`, as rows for `source`. Stub: refused.
pub fn expand(
    _text: &str,
    _source: &str,
    _from: DateTime<Utc>,
    _to: DateTime<Utc>,
) -> anyhow::Result<Vec<CalEvent>> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}
