//! When a stored secret expires: the grammar the agent's `expires` suggestion and
//! the TUI card's expiry input share, and the local-time text every surface shows.
//!
//! - relative: `30m`, `2h`, `3d` (a positive whole number of minutes, hours or days)
//! - absolute: `YYYY-MM-DD` (local midnight), `YYYY-MM-DD HH:MM[:SS]` or
//!   `YYYY-MM-DDTHH:MM[:SS]` (local), or RFC 3339 with an offset.
//!
//! The expiry itself is always stored as absolute Unix ms (`vault.expires_ms`).

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone as _};

/// A parsed expiry: relative to the moment it is applied, or an absolute instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    /// This many ms after "now".
    In(u64),
    /// This absolute Unix ms.
    At(u64),
}

impl Expiry {
    /// The absolute Unix ms, `now_ms` being "now".
    pub fn at(self, now_ms: u64) -> u64 {
        match self {
            Self::In(d) => now_ms.saturating_add(d),
            Self::At(t) => t,
        }
    }
}

/// Parse `text` (see the module docs). `Err` names what is wrong: an unknown shape, a zero
/// amount, or an absolute time at or before `now_ms`.
pub fn parse(text: &str, now_ms: u64) -> Result<Expiry, String> {
    let s = text.trim();
    if s.is_empty() {
        return Err("expires: empty".into());
    }
    if let Some(unit) = s.chars().last().filter(|c| matches!(c, 'm' | 'h' | 'd')) {
        let digits = s[..s.len() - 1].trim();
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            let n: u64 = digits
                .parse()
                .map_err(|_| format!("expires {s:?}: too large"))?;
            if n == 0 {
                return Err(format!("expires {s:?}: must be more than zero"));
            }
            let unit_ms: u64 = match unit {
                'm' => 60_000,
                'h' => 3_600_000,
                _ => 86_400_000,
            };
            return n
                .checked_mul(unit_ms)
                .map(Expiry::In)
                .ok_or_else(|| format!("expires {s:?}: too large"));
        }
    }
    let at = absolute(s).ok_or_else(|| {
        format!(
            "expires {s:?}: use 30m, 2h, 3d, YYYY-MM-DD, YYYY-MM-DD HH:MM or an ISO-8601 date-time"
        )
    })?;
    if at <= now_ms as i64 {
        return Err(format!("expires {s:?}: {} is in the past", local_time(at.max(0) as u64)));
    }
    Ok(Expiry::At(at as u64))
}

/// The agent's suggestion as the card receives it: a relative amount as written (`2h`;
/// the card counts from when the user stores), an absolute time as RFC 3339 UTC (resolved
/// here, in this engine's time zone, so the card on any device shows the same instant).
pub fn normalize(text: &str, now_ms: u64) -> Result<String, String> {
    Ok(match parse(text, now_ms)? {
        Expiry::In(_) => text.trim().to_owned(),
        Expiry::At(ms) => rfc3339(ms),
    })
}

/// `Sun 22 Nov 2026 14:00` in this machine's time zone.
pub fn local_time(ms: u64) -> String {
    match Local.timestamp_millis_opt(ms as i64).single() {
        Some(t) => t.format("%a %-d %b %Y %H:%M").to_string(),
        None => rfc3339(ms),
    }
}

/// RFC 3339 UTC, seconds precision.
pub fn rfc3339(ms: u64) -> String {
    DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// [`rfc3339`]'s inverse (any offset): `SecretInfo::expires` back to Unix ms.
pub fn from_rfc3339(s: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

/// For lists: `expires in 40 min` / `expires in 3 h` within a day, else
/// `expires Sun 22 Nov 2026 14:00`; `expired …` once it passed (until the sweeper ran).
pub fn describe(ms: u64, now_ms: u64) -> String {
    if ms <= now_ms {
        return format!("expired {}", local_time(ms));
    }
    let left = ms - now_ms;
    if left < 3_600_000 {
        format!("expires in {} min", left.div_ceil(60_000))
    } else if left < 86_400_000 {
        format!("expires in {} h", (left + 1_800_000) / 3_600_000)
    } else {
        format!("expires {}", local_time(ms))
    }
}

/// Unix ms of an absolute form, `None` when `s` is none of them.
fn absolute(s: &str) -> Option<i64> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    let local = |n: NaiveDateTime| Local.from_local_datetime(&n).earliest();
    for fmt in [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return local(n).map(|t| t.timestamp_millis());
        }
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    local(d.and_hms_opt(0, 0, 0)?).map(|t| t.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_790_000_000_000; // 2026-09-21

    fn local_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64
    }

    #[test]
    fn relative_amounts() {
        assert_eq!(parse("30m", NOW), Ok(Expiry::In(30 * 60_000)));
        assert_eq!(parse(" 2h ", NOW), Ok(Expiry::In(2 * 3_600_000)));
        assert_eq!(parse("3d", NOW), Ok(Expiry::In(3 * 86_400_000)));
        assert_eq!(parse("3d", NOW).unwrap().at(NOW), NOW + 3 * 86_400_000);
        assert_eq!(normalize("2h", NOW).unwrap(), "2h");
    }

    #[test]
    fn absolute_dates_and_date_times() {
        assert_eq!(parse("2030-01-02", NOW), Ok(Expiry::At(local_ms(2030, 1, 2, 0, 0))));
        assert_eq!(
            parse("2030-01-02 14:30", NOW),
            Ok(Expiry::At(local_ms(2030, 1, 2, 14, 30)))
        );
        assert_eq!(
            parse("2030-01-02T14:30:00", NOW),
            Ok(Expiry::At(local_ms(2030, 1, 2, 14, 30)))
        );
        let utc = DateTime::parse_from_rfc3339("2030-01-02T14:30:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(parse("2030-01-02T14:30:00Z", NOW), Ok(Expiry::At(utc)));
        assert_eq!(
            parse("2030-01-02T16:30:00+02:00", NOW),
            Ok(Expiry::At(utc)),
            "an offset is honoured"
        );
        assert_eq!(
            normalize("2030-01-02T16:30:00+02:00", NOW).unwrap(),
            "2030-01-02T14:30:00Z"
        );
    }

    #[test]
    fn invalid_zero_and_past_are_errors() {
        for bad in ["", "soon", "2w", "-1h", "1.5h", "h", "30 minutes", "2030-13-01", "99999999999999999999d"] {
            assert!(parse(bad, NOW).is_err(), "{bad:?} must be rejected");
        }
        assert!(parse("0m", NOW).unwrap_err().contains("more than zero"));
        assert!(parse("2020-01-01", NOW).unwrap_err().contains("in the past"));
        assert!(parse("2020-01-01T00:00:00Z", NOW).unwrap_err().contains("in the past"));
        let past = rfc3339(NOW);
        assert!(parse(&past, NOW).is_err(), "now itself is not in the future");
    }

    #[test]
    fn local_time_reads_like_a_calendar() {
        let ms = local_ms(2026, 11, 22, 14, 0);
        assert_eq!(local_time(ms), "Sun 22 Nov 2026 14:00");
    }

    #[test]
    fn describe_for_lists() {
        assert_eq!(describe(NOW + 40 * 60_000, NOW), "expires in 40 min");
        assert_eq!(describe(NOW + 3 * 3_600_000, NOW), "expires in 3 h");
        let far = NOW + 3 * 86_400_000;
        assert_eq!(describe(far, NOW), format!("expires {}", local_time(far)));
        assert!(describe(NOW, NOW).starts_with("expired "));
        assert_eq!(from_rfc3339(&rfc3339(far)), Some(far / 1000 * 1000));
    }
}
