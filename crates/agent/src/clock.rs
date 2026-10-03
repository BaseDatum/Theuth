//! Time awareness (design doc 74): a rate-limited `<time …/>` line the
//! harness prepends to inputs the model sees, and the `at="HH:MM"` clock on
//! wake-up wrappers. Pure functions; the loop owns the `last_stamp` state.

use chrono::{DateTime, Local, Utc};
use std::time::Duration;

/// Minimum wall time between two stamps in one session (74 §3). A user
/// turn or tool result landing sooner than this after the last stamp gets
/// none; the first input of a session (or after a restart with no stamp in
/// the frame) always gets one.
pub const TIME_STAMP_GAP: Duration = Duration::from_secs(5 * 60);

const OPEN: &str = "<time now=\"";

/// `NNs` / `Nm` / `Nh Nm` / `Nd Nh`.
pub fn humanise(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

/// Elapsed from `then` to `now`, zero when `then` is in the future.
fn since(then: DateTime<Utc>, now: DateTime<Utc>) -> Duration {
    (now - then).to_std().unwrap_or_default()
}

/// The stamp line: `<time now="2026-09-28 14:03 -07:00 (Mon)" since_last="1h 12m"/>`.
/// `now` is local with its numeric offset (chrono has no zone abbreviations;
/// the offset is unambiguous and [`parse`] reads it back); `since_last` is
/// omitted on the first stamp of a session.
pub fn stamp(now: DateTime<Local>, last: Option<DateTime<Utc>>) -> String {
    let mut s = format!("{OPEN}{}\"", now.format("%Y-%m-%d %H:%M %:z (%a)"));
    if let Some(last) = last {
        s.push_str(&format!(
            " since_last=\"{}\"",
            humanise(since(last, now.with_timezone(&Utc)))
        ));
    }
    s.push_str("/>");
    s
}

/// Prepend a stamp to `text` when one is due: no stamp yet, or at least
/// [`TIME_STAMP_GAP`] since the last one. A text that already opens with a
/// `<time` line is left alone. Updates `last` when a stamp is written.
pub fn prepend_due(text: &mut String, now: DateTime<Local>, last: &mut Option<DateTime<Utc>>) {
    if text.trim_start().starts_with("<time ") {
        return;
    }
    let now_utc = now.with_timezone(&Utc);
    if last.is_some_and(|l| since(l, now_utc) < TIME_STAMP_GAP) {
        return;
    }
    let line = stamp(now, *last);
    text.reserve(line.len() + 1);
    text.insert(0, '\n');
    text.insert_str(0, &line);
    *last = Some(now_utc);
}

/// `text` without a leading `<time …/>` line, for readers that inspect the
/// first characters of a user message (skill injections, titles, hints).
pub fn strip(text: &str) -> &str {
    let t = text.trim_start();
    if !t.starts_with(OPEN) {
        return text;
    }
    match t.find("/>") {
        Some(end) => t[end + 2..].strip_prefix('\n').unwrap_or(&t[end + 2..]),
        None => text,
    }
}

/// [`strip`] in place: `text` minus its leading stamp line, the allocation
/// kept. For texts handed to frontends (`TurnStarted`, history rows), which
/// never see the model's stamp (74 §3, §5).
pub fn strip_owned(mut text: String) -> String {
    let keep = text.len() - strip(&text).len();
    text.drain(..keep);
    text
}

/// The instant of a leading stamp, for recovering `last_stamp` on resume.
pub fn parse(text: &str) -> Option<DateTime<Utc>> {
    let rest = text.trim_start().strip_prefix(OPEN)?;
    let now = rest.split('"').next()?;
    let now = now.rsplit_once(" (").map_or(now, |(n, _)| n);
    DateTime::parse_from_str(now, "%Y-%m-%d %H:%M %:z")
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `HH:MM` local, for wrapper `at` attributes.
pub fn hhmm(now: DateTime<Local>) -> String {
    now.format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn local(s: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&Local)
    }

    #[test]
    fn humanised_elapsed() {
        assert_eq!(humanise(Duration::from_secs(0)), "0s");
        assert_eq!(humanise(Duration::from_secs(59)), "59s");
        assert_eq!(humanise(Duration::from_secs(60)), "1m");
        assert_eq!(humanise(Duration::from_secs(3599)), "59m");
        assert_eq!(humanise(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(humanise(Duration::from_secs(4320)), "1h 12m");
        assert_eq!(humanise(Duration::from_secs(90_000)), "1d 1h");
    }

    #[test]
    fn stamp_format_and_first_stamp_has_no_since_last() {
        let now = FixedOffset::west_opt(7 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 28, 14, 3, 0)
            .unwrap();
        let s = stamp(now.with_timezone(&Local), None);
        // The local zone is the machine's; the instant is the same.
        assert!(s.starts_with("<time now=\""), "{s}");
        assert!(s.ends_with("\"/>"), "{s}");
        assert!(!s.contains("since_last"));
        assert_eq!(parse(&s), Some(now.with_timezone(&Utc)));
        let later = now + chrono::Duration::seconds(4320);
        let s = stamp(later.with_timezone(&Local), Some(now.with_timezone(&Utc)));
        assert!(s.ends_with(" since_last=\"1h 12m\"/>"), "{s}");
    }

    #[test]
    fn stamp_emitted_or_suppressed_around_the_threshold() {
        let t0 = local("2026-09-28T14:00:00+02:00");
        let mut last = None;
        let mut a = String::from("first");
        prepend_due(&mut a, t0, &mut last);
        assert!(a.starts_with("<time now=\""), "{a}");
        assert!(a.ends_with("/>\nfirst"), "{a}");
        assert_eq!(last, Some(t0.with_timezone(&Utc)));

        let mut b = String::from("soon");
        prepend_due(&mut b, t0 + chrono::Duration::seconds(299), &mut last);
        assert_eq!(b, "soon");
        assert_eq!(last, Some(t0.with_timezone(&Utc)));

        let t1 = t0 + TIME_STAMP_GAP;
        let mut c = String::from("late");
        prepend_due(&mut c, t1, &mut last);
        assert!(c.contains(" since_last=\"5m\"/>\nlate"), "{c}");
        assert_eq!(last, Some(t1.with_timezone(&Utc)));

        // Already stamped: untouched, `last` unchanged.
        let mut d = String::from("<time now=\"x\"/>\nhello");
        prepend_due(&mut d, t1 + chrono::Duration::hours(2), &mut last);
        assert_eq!(d, "<time now=\"x\"/>\nhello");
        assert_eq!(last, Some(t1.with_timezone(&Utc)));
    }

    #[test]
    fn strip_and_parse_round_trip() {
        let t = local("2026-09-28T14:03:00-07:00");
        let mut text = String::from("<skill name=\"x\">body</skill> words");
        prepend_due(&mut text, t, &mut None);
        assert_eq!(strip(&text), "<skill name=\"x\">body</skill> words");
        assert_eq!(strip_owned(text.clone()), strip(&text));
        assert_eq!(strip_owned("  plain".into()), "  plain");
        assert_eq!(parse(&text), Some(t.with_timezone(&Utc)));
        assert_eq!(strip("plain"), "plain");
        assert_eq!(strip("<timer>x</timer>"), "<timer>x</timer>");
        assert_eq!(parse("plain"), None);
        assert_eq!(parse("<time now=\"garbage\"/>"), None);
    }
}
