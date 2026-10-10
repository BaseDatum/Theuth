//! 158 W2 owns this file (stub from C158).
//!
//! Matching (158 §8.1): `score = time + host + title + rsvp` over the `cal_events` rows in
//! `[start − 10 min, end + 15 min]`; the best is shown at `≥ 0.8` with a `0.25` margin, else
//! the list opens unselected. Tests over the §7.7 fixtures with a fake clock (§8.4).

use chrono::{DateTime, Utc};

use super::{CalEvent, EventMatch};

/// Show threshold for the best candidate.
pub const SHOW: f32 = 0.8;
/// Required margin over the runner-up.
pub const MARGIN: f32 = 0.25;
/// A lone candidate is shown from this `time` term on; nothing lower is ever preselected.
pub const LONE: f32 = 0.5;

/// The match for a detection of `app_id` at `now` among `candidates`; `None` when nothing is
/// in the window. `event` is the preselection (or the top row when none qualifies — the
/// caller shows it unselected when `score < SHOW`). Stub: never a match.
pub fn best(_now: DateTime<Utc>, _app_id: &str, _candidates: &[CalEvent]) -> Option<EventMatch> {
    None
}
