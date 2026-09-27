//! `published` normalisation (design doc 16 §4.6): providers hand back
//! anything from RFC 3339 to "3 days ago" to, when a scraper slips, snippet
//! text. Agents get RFC 3339 UTC or nothing.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::provider::Recency;

/// Absolute formats tried after RFC 3339 / RFC 2822, most common first.
const ABSOLUTE: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
    "%Y/%m/%d",
    "%B %d, %Y",
    "%b %d, %Y",
    "%B %d %Y",
    "%b %d %Y",
    "%d %B %Y",
    "%d %b %Y",
    "%d %B, %Y",
    "%m/%d/%Y",
    "%d.%m.%Y",
];

/// `raw` as RFC 3339 UTC (`2026-09-27T00:00:00Z`), or `None` when it is not a
/// date. Relative ages ("2 days ago", "3 hours ago") resolve against `now`.
/// A bare year or anything longer than a date line (a snippet) is rejected.
#[must_use]
pub fn normalize(raw: &str, now: DateTime<Utc>) -> Option<String> {
    parse(raw, now).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// `raw` as a UTC instant; see [`normalize`].
#[must_use]
pub fn parse(raw: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let s = raw.trim().trim_end_matches('.');
    if s.is_empty() || s.chars().count() > 40 {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(s) {
        return Some(d.with_timezone(&Utc));
    }
    let head = s.split(" · ").next().unwrap_or(s).trim();
    for f in ABSOLUTE {
        if let Ok(d) = NaiveDateTime::parse_from_str(head, f) {
            return Some(Utc.from_utc_datetime(&d));
        }
        if let Ok(d) = NaiveDate::parse_from_str(head, f) {
            let year = d.year();
            if (1990..=now.year() + 1).contains(&year) {
                return Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?));
            }
        }
    }
    relative(head, now)
}

/// "N unit(s) ago", "yesterday", "today"; SERP scrapers emit these.
fn relative(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "today" | "just now" => return Some(now),
        "yesterday" => return Some(now - chrono::Duration::days(1)),
        _ => {}
    }
    let mut it = lower.split_whitespace();
    let n: i64 = match it.next()? {
        "a" | "an" | "one" => 1,
        w => w.parse().ok()?,
    };
    let unit = it.next()?;
    if it.next()? != "ago" || it.next().is_some() {
        return None;
    }
    let days = match unit.trim_end_matches('s') {
        "second" | "sec" => return Some(now - chrono::Duration::seconds(n)),
        "minute" | "min" => return Some(now - chrono::Duration::minutes(n)),
        "hour" | "hr" => return Some(now - chrono::Duration::hours(n)),
        "day" => n,
        "week" => n * 7,
        "month" => n * 30,
        "year" => n * 365,
        _ => return None,
    };
    Some(now - chrono::Duration::days(days))
}

/// Length of a [`Recency`] window.
#[must_use]
pub fn window(r: Recency) -> chrono::Duration {
    chrono::Duration::days(match r {
        Recency::Day => 1,
        Recency::Week => 7,
        Recency::Month => 31,
        Recency::Year => 366,
    })
}

/// `true` when `published` (RFC 3339) is inside the `recent` window ending at `now`.
#[must_use]
pub fn within(published: &str, recent: Recency, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(published)
        .map(|d| now - d.with_timezone(&Utc) <= window(recent))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap()
    }

    #[test]
    fn absolute_and_relative_forms_normalise() {
        let n = now();
        assert_eq!(
            normalize("2024-03-12T09:12:45", n).as_deref(),
            Some("2024-03-12T09:12:45Z")
        );
        assert_eq!(
            normalize("2024-03-12", n).as_deref(),
            Some("2024-03-12T00:00:00Z")
        );
        assert_eq!(
            normalize("March 12, 2024", n).as_deref(),
            Some("2024-03-12T00:00:00Z")
        );
        assert_eq!(
            normalize("Mar 12, 2024 · Some snippet", n).as_deref(),
            Some("2024-03-12T00:00:00Z")
        );
        assert_eq!(
            normalize("2 days ago", n).as_deref(),
            Some("2026-09-25T12:00:00Z")
        );
        assert_eq!(
            normalize("Sun, 27 Sep 2026 10:00:00 GMT", n).as_deref(),
            Some("2026-09-27T10:00:00Z")
        );
    }

    #[test]
    fn snippet_text_and_bare_years_are_not_dates() {
        let n = now();
        assert_eq!(normalize("Bun is a fast JavaScript runtime", n), None);
        assert_eq!(normalize("2024", n), None);
        assert_eq!(normalize("", n), None);
        assert_eq!(
            normalize(
                "Install Bun 1.3 with curl -fsSL https://bun.sh/install | bash and read the release notes",
                n
            ),
            None
        );
    }

    #[test]
    fn within_window() {
        let n = now();
        assert!(within("2026-09-26T00:00:00Z", Recency::Week, n));
        assert!(!within("2026-08-01T00:00:00Z", Recency::Week, n));
        assert!(within("2026-08-01T00:00:00Z", Recency::Year, n));
        assert!(!within("not a date", Recency::Year, n));
    }
}
