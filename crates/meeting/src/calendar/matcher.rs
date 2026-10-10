//! Matching (158 §8.1): `score = time + host + title + rsvp` over the `cal_events` rows in
//! `[start − 10 min, end + 15 min]`; the best is preselected at `≥ 0.8` with a `0.25` margin,
//! else the list opens unselected. `title` is `0` in W2: no window title reaches the engine.
//! The user's pick from the `[▾]` list (`meeting_detection_set_event`) is a process-wide
//! [`choose`] that [`best`] honours on every later tick, so W1's rules engine needs no state.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use super::ics::meeting_host;
use super::{CalEvent, EventId, EventMatch};

/// Show threshold for the best candidate.
pub const SHOW: f32 = 0.8;
/// Required margin over the runner-up.
pub const MARGIN: f32 = 0.25;
/// A lone candidate is shown from this `time` term on; nothing lower is ever preselected.
pub const LONE: f32 = 0.5;

/// Minutes before `start` a candidate enters the window.
pub const BEFORE_MIN: i64 = 10;
/// Minutes after `end` a candidate leaves it.
pub const AFTER_MIN: i64 = 15;

const BROWSERS: &[&str] = &[
    "com.google.chrome",
    "com.apple.safari",
    "org.mozilla.firefox",
    "company.thebrowser.browser",
    "com.microsoft.edgemac",
    "com.brave.browser",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
];

/// Canonical join host → the app ids that open it (§8.1 host map).
const HOSTS: &[(&str, &[&str])] = &[
    ("zoom.us", &["us.zoom.xos", "zoom.exe", "zoom"]),
    ("meet.google.com", BROWSERS),
    (
        "teams.microsoft.com",
        &[
            "com.microsoft.teams2",
            "com.microsoft.teams",
            "ms-teams.exe",
            "teams.exe",
            "teams",
        ],
    ),
    (
        "teams.live.com",
        &[
            "com.microsoft.teams2",
            "com.microsoft.teams",
            "ms-teams.exe",
            "teams.exe",
            "teams",
        ],
    ),
    (
        "webex.com",
        &["cisco-systems.spark", "webexmta.exe", "webex"],
    ),
    ("facetime.apple.com", &["com.apple.facetime"]),
];

fn apps_of(host: &str) -> Option<&'static [&'static str]> {
    let canonical = meeting_host(host)?;
    HOSTS.iter().find(|(h, _)| *h == canonical).map(|(_, a)| *a)
}

/// `+0.6` when the link opens in `app_id`; `−0.3` when it opens in another known app; `0`
/// when the link is unknown or the app is not one we map (a browser is "any browser").
pub fn host_term(join_host: Option<&str>, app_id: &str) -> f32 {
    let Some(apps) = join_host.and_then(apps_of) else {
        return 0.0;
    };
    let app = app_id.to_ascii_lowercase();
    if apps.contains(&app.as_str()) {
        0.6
    } else if HOSTS.iter().any(|(_, a)| a.contains(&app.as_str())) {
        -0.3
    } else {
        0.0
    }
}

/// `1` inside the event, falling linearly to `0` at `start − 10 min` and `end + 15 min`;
/// `None` outside the window.
pub fn time_term(now: DateTime<Utc>, start: DateTime<Utc>, end: DateTime<Utc>) -> Option<f32> {
    let before = Duration::from_secs(60 * BEFORE_MIN as u64);
    let after = Duration::from_secs(60 * AFTER_MIN as u64);
    if now < start {
        let gap = (start - now).to_std().ok()?;
        (gap <= before).then(|| 1.0 - gap.as_secs_f32() / before.as_secs_f32())
    } else if now > end {
        let gap = (now - end).to_std().ok()?;
        (gap <= after).then(|| 1.0 - gap.as_secs_f32() / after.as_secs_f32())
    } else {
        Some(1.0)
    }
}

/// `+0.2` accepted / tentative, `−0.2` declined, `0` otherwise (the `is_me` attendee).
pub fn rsvp_term(e: &CalEvent) -> f32 {
    match e
        .attendees
        .iter()
        .find(|a| a.is_me)
        .and_then(|a| a.status.as_deref())
    {
        Some("ACCEPTED" | "TENTATIVE") => 0.2,
        Some("DECLINED") => -0.2,
        _ => 0.0,
    }
}

/// `(score, time)` for one candidate, `None` when it is not one (cancelled, all-day, outside
/// the window).
pub fn score(now: DateTime<Utc>, app_id: &str, e: &CalEvent) -> Option<(f32, f32)> {
    if e.cancelled || e.all_day {
        return None;
    }
    let start = super::store::parse(&e.start).ok()?;
    let end = super::store::parse(&e.end).ok()?;
    let time = time_term(now, start, end)?;
    Some((
        time + host_term(e.join_host.as_deref(), app_id) + rsvp_term(e),
        time,
    ))
}

struct Choice {
    app_id: String,
    event: Option<EventId>,
    at: Instant,
}

static CHOICE: Mutex<Option<Choice>> = Mutex::new(None);

/// A pick stays in force this long (a prompt never lives longer).
const CHOICE_TTL: Duration = Duration::from_secs(30 * 60);

/// Remember the user's pick for the prompt of `app_id` (§8.2): `Some(id)` preselects it on
/// every later [`best`]; `None` ("None of these") clears the match.
pub fn choose(app_id: &str, event: Option<EventId>) {
    *CHOICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Choice {
        app_id: app_id.to_owned(),
        event,
        at: Instant::now(),
    });
}

/// Forget the pick (a prompt ended).
pub fn clear_choice() {
    *CHOICE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn chosen(app_id: &str) -> Option<Option<EventId>> {
    let g = CHOICE.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref()
        .filter(|c| c.app_id == app_id && c.at.elapsed() < CHOICE_TTL)
        .map(|c| c.event.clone())
}

/// The match for a detection of `app_id` at `now` among `candidates`; `None` when nothing is
/// in the window. `event` is the preselection (or the top row when none qualifies — the
/// caller shows it unselected when `score < SHOW`). `score` is the §8.1 sum when the row is
/// preselected; when the margin or lone rule says "list, no preselection" it is reported just
/// under [`SHOW`] so the frozen shape (no `shown` flag) still tells the caller.
pub fn best(now: DateTime<Utc>, app_id: &str, candidates: &[CalEvent]) -> Option<EventMatch> {
    let mut scored: Vec<(f32, f32, &CalEvent)> = candidates
        .iter()
        .filter_map(|e| score(now, app_id, e).map(|(s, t)| (s, t, e)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.start.cmp(&b.2.start)));
    if scored.is_empty() {
        return None;
    }
    if let Some(pick) = chosen(app_id) {
        let id = pick?;
        if let Some(i) = scored.iter().position(|(_, _, e)| e.id == id) {
            let (s, _, e) = scored.remove(i);
            return Some(EventMatch {
                event: e.to_ref(),
                score: s.max(SHOW),
                alternatives: scored.iter().map(|(_, _, e)| e.to_ref()).collect(),
            });
        }
    }
    let (top, time, event) = scored[0];
    let margin_ok = scored
        .get(1)
        .is_none_or(|(second, _, _)| top - second >= MARGIN);
    if scored.len() == 1 && time < LONE {
        return None;
    }
    let shown = top >= SHOW && margin_ok;
    Some(EventMatch {
        event: event.to_ref(),
        score: if shown { top } else { top.min(SHOW - 0.01) },
        alternatives: scored[1..].iter().map(|(_, _, e)| e.to_ref()).collect(),
    })
}

/// Did [`best`] preselect its event?
pub fn shown(m: &EventMatch) -> bool {
    m.score >= SHOW
}
