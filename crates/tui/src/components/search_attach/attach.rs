//! `/attach` (design doc 66 §8, §10): the naming table `lib/attach.ts` uses,
//! ported for the TUI — the title and `meta` a search hit or a recent
//! session becomes when it is attached, and the transcript spelling of a
//! leading marker line.

use std::fmt::Write as _;

use agent::SessionSummary;
use agent::attach::{AttachRef, Kind, session_title};
use chrono::{DateTime, Datelike, Local, Utc};

use super::state::Hit;

/// Local `MMM d` this year, `MMM d, yyyy` otherwise.
pub fn date(t: DateTime<Utc>) -> String {
    let t = t.with_timezone(&Local);
    if t.year() == Local::now().year() {
        t.format("%b %-d").to_string()
    } else {
        t.format("%b %-d, %Y").to_string()
    }
}

fn date_rfc3339(s: &str) -> String {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| s.to_owned(), |t| date(t.with_timezone(&Utc)))
}

fn date_ms(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .map(date)
        .unwrap_or_default()
}

fn date_secs(s: i64) -> String {
    DateTime::from_timestamp(s, 0).map(date).unwrap_or_default()
}

/// Decimal `kB`/`MB` (199 UI-2: one unit system), one decimal below 10.
pub fn size(bytes: u64) -> String {
    const K: f64 = 1000.0;
    let b = bytes as f64;
    let (n, unit) = if b < K {
        return format!("{bytes} B");
    } else if b < K * K {
        (b / K, "kB")
    } else {
        (b / K / K, "MB")
    };
    if n < 10.0 {
        format!("{n:.1} {unit}")
    } else {
        format!("{n:.0} {unit}")
    }
}

/// The reference a search hit attaches as.
pub fn from_hit(hit: &Hit) -> AttachRef {
    match hit {
        Hit::Session(h) => {
            let mut meta = format!("chat · updated {}", date_rfc3339(&h.updated_at));
            if h.archived {
                meta.push_str(" · archived");
            }
            AttachRef {
                kind: Kind::Session,
                id: h.session.to_string(),
                title: session_title(
                    &h.title,
                    None,
                    DateTime::parse_from_rfc3339(&h.created_at)
                        .ok()
                        .map(|t| t.with_timezone(&Utc)),
                ),
                meta,
            }
        }
        Hit::Artifact(h) => {
            let a = &h.artifact;
            let mut meta = format!("{} · v{}", a.kind.as_str(), a.version);
            if let Some(p) = a.pages {
                let _ = write!(meta, " · {p} pages");
            }
            let _ = write!(meta, " · {} · {}", size(a.bytes), date_secs(a.created_at));
            AttachRef {
                kind: Kind::Artifact,
                id: a.call_id.clone(),
                title: if a.title.trim().is_empty() {
                    a.name.clone()
                } else {
                    a.title.clone()
                },
                meta,
            }
        }
        Hit::Knowledge(h) => {
            let m = &h.meta;
            let mut meta = format!("{} · {}", m.type_, m.id);
            if let Some(t) = &m.timestamp {
                let _ = write!(meta, " · {}", date_rfc3339(t));
            }
            AttachRef {
                kind: Kind::Knowledge,
                id: m.id.clone(),
                title: m.title.clone(),
                meta,
            }
        }
        Hit::History(h) => AttachRef {
            kind: Kind::Knowledge,
            id: format!("{}@{}", h.id, h.commit),
            title: h.title.clone(),
            meta: format!(
                "revision {} · {}",
                h.commit.get(..7).unwrap_or(&h.commit),
                date_rfc3339(&h.time)
            ),
        },
        Hit::Note(h) => {
            let d = &h.doc;
            AttachRef {
                kind: Kind::Note,
                id: d.id.to_string(),
                title: if d.title.trim().is_empty() {
                    format!("Untitled note · {}", date_ms(d.created_ms))
                } else {
                    d.title.clone()
                },
                meta: format!(
                    "note · updated {} · {}",
                    date_ms(d.updated_ms),
                    size(d.bytes.max(0) as u64)
                ),
            }
        }
        Hit::Meeting(h) => {
            let m = &h.meeting;
            let mut meta = date_rfc3339(&m.started_at);
            if let Some(d) = m.duration_ms {
                let _ = write!(meta, " · {} min", d / 60_000);
            }
            if m.state != meeting::MeetingState::Done {
                let _ = write!(meta, " · {:?}", m.state);
            }
            let title = meeting::search::title_of(m);
            AttachRef {
                kind: Kind::Meeting,
                id: m.id.to_string(),
                title: if title.trim().is_empty() {
                    format!("Meeting · {}", date_rfc3339(&m.started_at))
                } else {
                    title.to_owned()
                },
                meta,
            }
        }
    }
}

/// The reference a recent-sessions row attaches as.
pub fn from_session(s: &SessionSummary) -> AttachRef {
    let mut meta = format!(
        "chat · {} · updated {}",
        s.project.as_deref().unwrap_or("no project"),
        date(s.updated_at)
    );
    if s.archived {
        meta.push_str(" · archived");
    }
    AttachRef {
        kind: Kind::Session,
        id: s.id.to_string(),
        title: session_title(&s.title, None, Some(s.created_at)),
        meta,
    }
}

