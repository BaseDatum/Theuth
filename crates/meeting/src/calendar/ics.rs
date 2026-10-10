//! ICS (158 §7.3): [`calcard`] parses the VCALENDAR and expands recurrences (RRULE / RDATE /
//! EXDATE, `RECURRENCE-ID` overrides incl. `THISANDFUTURE`, VTIMEZONE and Windows zone names);
//! this module is the glue that turns its instances into [`CalEvent`] rows. Two things calcard
//! does not do for a subscription feed: it keys overrides by `(SEQUENCE, RECURRENCE-ID)` without
//! the UID, so every UID is expanded on its own with `SEQUENCE` stripped; and it has no join-link
//! notion, so [`join_link`] is ours. Fixtures under `crates/meeting/tests/fixtures/calendar/`.

use calcard::common::timezone::Tz;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarParameterName,
    ICalendarProperty, ICalendarStatus, ICalendarValue,
};
use chrono::{DateTime, Utc};

use super::store::stamp;
use super::{Attendee, CalEvent, EventId, local_part};

/// Instances expanded per series before giving up; the window filter runs afterwards.
// ponytail: calcard iterates from DTSTART, so a daily series older than ~13 years shows nothing;
// raise or pre-trim DTSTART if a user hits it.
const LIMIT: usize = 5000;

/// Meeting-link hosts (OpenWhispr's rules, §7.3), as `(suffix, canonical host)`; a host
/// matches when it equals the suffix or ends with `.suffix`.
pub const JOIN_HOSTS: &[(&str, &str)] = &[
    ("zoom.us", "zoom.us"),
    ("meet.google.com", "meet.google.com"),
    ("teams.microsoft.com", "teams.microsoft.com"),
    ("teams.live.com", "teams.live.com"),
    ("webex.com", "webex.com"),
    ("facetime.apple.com", "facetime.apple.com"),
];

/// Parse `text` (one VCALENDAR) and expand every VEVENT into the instances that overlap
/// `[from, to]`, as rows for `source`.
pub fn expand(
    text: &str,
    source: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> anyhow::Result<Vec<CalEvent>> {
    let ical = ICalendar::parse(text)
        .map_err(|_| anyhow::anyhow!("not an iCalendar document (no BEGIN:VCALENDAR)"))?;
    // Floating times (no TZID, no Z) are local (§7.3).
    let default_tz = Tz::Fixed(*chrono::Local::now().offset());
    let timezones: Vec<ICalendarComponent> = ical.timezones().cloned().collect();
    let resolver = ical.build_tz_resolver().with_default(default_tz);

    // Group VEVENTs by UID, keeping file order.
    let mut groups: Vec<(String, Vec<&ICalendarComponent>)> = Vec::new();
    for (i, comp) in ical.components.iter().enumerate() {
        if comp.component_type != ICalendarComponentType::VEvent {
            continue;
        }
        let uid = comp
            .uid()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("nouid-{i}"));
        match groups.iter_mut().find(|(u, _)| *u == uid) {
            Some((_, v)) => v.push(comp),
            None => groups.push((uid, vec![comp])),
        }
    }

    let mut out = Vec::new();
    for (uid, comps) in groups {
        let mut sub = ICalendar {
            components: timezones.clone(),
        };
        let base = sub.components.len();
        for c in &comps {
            let mut c = (*c).clone();
            c.entries.retain(|e| e.name != ICalendarProperty::Sequence);
            sub.components.push(c);
        }
        let expanded = sub.expand_dates(default_tz, LIMIT);
        for err in &expanded.errors {
            tracing::debug!(uid = %uid, "ics: {}", err.error);
        }
        for inst in expanded.events {
            let (s, e) = inst.timestamps();
            let (Some(start), Some(end)) = (
                DateTime::<Utc>::from_timestamp(s, 0),
                DateTime::<Utc>::from_timestamp(e.max(s), 0),
            ) else {
                continue;
            };
            if end < from || start > to {
                continue;
            }
            let Some(comp) = (inst.comp_id as usize)
                .checked_sub(base)
                .and_then(|i| comps.get(i))
            else {
                continue;
            };
            out.push(row(source, &uid, comp, start, end, &resolver));
        }
    }
    Ok(out)
}

fn text(comp: &ICalendarComponent, prop: &ICalendarProperty) -> Option<String> {
    comp.property(prop)
        .and_then(|e| e.values.first())
        .and_then(ICalendarValue::as_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn row(
    source: &str,
    uid: &str,
    comp: &ICalendarComponent,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    resolver: &calcard::icalendar::timezone::TzResolver<&str>,
) -> CalEvent {
    let dtstart = comp.property(&ICalendarProperty::Dtstart);
    let all_day = dtstart
        .and_then(|e| e.values.first())
        .is_some_and(|v| matches!(v, ICalendarValue::PartialDateTime(dt) if !dt.has_time()));
    let tz = dtstart.and_then(|e| e.tz_id()).map(str::to_owned);
    let recurring = comp.has_property(&ICalendarProperty::Rrule)
        || comp.has_property(&ICalendarProperty::Rdate);
    // The original occurrence an override replaces, when this instance is that override.
    let rid = comp
        .property(&ICalendarProperty::RecurrenceId)
        .and_then(|e| match e.values.first() {
            Some(ICalendarValue::PartialDateTime(dt)) => {
                let own_start = dtstart.and_then(|d| match d.values.first() {
                    Some(ICalendarValue::PartialDateTime(sdt)) => sdt
                        .to_date_time_with_tz(resolver.resolve_or_default(d.tz_id()))
                        .map(|t| t.timestamp()),
                    _ => None,
                });
                let rid = dt
                    .to_date_time_with_tz(resolver.resolve_or_default(e.tz_id()))
                    .map(|t| t.timestamp())?;
                // A THISANDFUTURE override also templates the later instances; those keep
                // their own start as the occurrence key.
                (own_start == Some(start.timestamp()))
                    .then(|| DateTime::<Utc>::from_timestamp(rid, 0))
                    .flatten()
            }
            _ => None,
        });
    let recurrence_id = match rid {
        Some(r) => Some(stamp(r)),
        None if recurring || comp.has_property(&ICalendarProperty::RecurrenceId) => {
            Some(stamp(start))
        }
        None => None,
    };
    let sequence = comp
        .property(&ICalendarProperty::Sequence)
        .and_then(|e| match e.values.first() {
            Some(ICalendarValue::Integer(n)) => u32::try_from(*n).ok(),
            _ => None,
        })
        .unwrap_or(0);
    let cancelled = comp
        .property(&ICalendarProperty::Status)
        .and_then(|e| e.values.first())
        .is_some_and(|v| matches!(v, ICalendarValue::Status(ICalendarStatus::Cancelled)));
    let location = text(comp, &ICalendarProperty::Location);
    let description = text(comp, &ICalendarProperty::Description);
    let (join_url, join_host) = join_link(
        text(comp, &ICalendarProperty::Url).as_deref(),
        comp.entries.iter().filter_map(|e| match &e.name {
            ICalendarProperty::Other(n)
                if n.eq_ignore_ascii_case("X-GOOGLE-CONFERENCE")
                    || n.eq_ignore_ascii_case("X-MICROSOFT-SKYPETEAMSMEETINGURL") =>
            {
                e.values.first().and_then(ICalendarValue::as_text)
            }
            _ => None,
        }),
        location.as_deref(),
        description.as_deref(),
    )
    .unzip();
    let organizer = comp
        .property(&ICalendarProperty::Organizer)
        .and_then(participant);
    let attendees = comp
        .properties(&ICalendarProperty::Attendee)
        .filter(|e| {
            !e.parameter(&ICalendarParameterName::Cutype)
                .and_then(|v| v.as_text())
                .is_some_and(|t| {
                    t.eq_ignore_ascii_case("RESOURCE") || t.eq_ignore_ascii_case("ROOM")
                })
        })
        .filter_map(participant)
        .collect();
    CalEvent {
        id: EventId(format!(
            "{source}:{uid}:{}",
            recurrence_id.as_deref().unwrap_or("")
        )),
        source: source.to_owned(),
        uid: uid.to_owned(),
        title: text(comp, &ICalendarProperty::Summary).unwrap_or_else(|| "(no title)".into()),
        start: stamp(start),
        end: stamp(end),
        join_url,
        join_host,
        organizer,
        attendees,
        description,
        all_day,
        tz,
        cancelled,
        sequence,
        location,
        recurrence_id,
    }
}

/// `ATTENDEE` / `ORGANIZER`: `CN`, `mailto:`, `PARTSTAT`. `is_me` is never known from a feed.
fn participant(e: &calcard::icalendar::ICalendarEntry) -> Option<Attendee> {
    let value = e.values.first().and_then(ICalendarValue::as_text)?.trim();
    let email = value
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("mailto:"))
        .map(|_| value[7..].trim().to_owned())
        .filter(|s| !s.is_empty());
    let cn = e
        .parameter(&ICalendarParameterName::Cn)
        .and_then(|v| v.as_text())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let name = match (cn, &email) {
        (Some(cn), _) => cn.to_owned(),
        (None, Some(mail)) => local_part(mail).to_owned(),
        (None, None) => value.to_owned(),
    };
    Some(Attendee {
        name,
        email,
        status: e
            .parameter(&ICalendarParameterName::Partstat)
            .and_then(|v| v.as_text())
            .map(|s| s.to_ascii_uppercase()),
        is_me: false,
    })
}

/// The canonical meeting host for `host`, when it matches a rule.
pub fn meeting_host(host: &str) -> Option<&'static str> {
    let host = host.to_ascii_lowercase();
    JOIN_HOSTS
        .iter()
        .find(|(suffix, _)| {
            host == *suffix || host.strip_suffix(suffix).is_some_and(|h| h.ends_with('.'))
        })
        .map(|(_, canonical)| *canonical)
}

/// `URL`, then the conference X-props, then the first URL in `LOCATION` / `DESCRIPTION` whose
/// host matches a rule (Google redirect wrappers unwrapped); as `(url, canonical host)`. A
/// `URL` property with an unknown host is kept as a last resort with its own host.
pub fn join_link<'a>(
    url: Option<&str>,
    xprops: impl Iterator<Item = &'a str>,
    location: Option<&str>,
    description: Option<&str>,
) -> Option<(String, String)> {
    let candidates = url
        .into_iter()
        .map(str::to_owned)
        .chain(xprops.map(str::to_owned))
        .chain(location.into_iter().flat_map(urls_in))
        .chain(description.into_iter().flat_map(urls_in))
        .filter_map(|u| unwrap_redirect(&u))
        .map(|u| {
            let host = u.host_str().unwrap_or("").to_owned();
            (u.to_string(), host)
        })
        .collect::<Vec<_>>();
    candidates
        .iter()
        .find_map(|(u, h)| meeting_host(h).map(|c| (u.clone(), c.to_owned())))
        .or_else(|| {
            url.and_then(|_| candidates.first().cloned())
                .filter(|(_, h)| !h.is_empty())
        })
}

/// `https://www.google.com/url?q=<target>` → `<target>`; anything else parsed as is.
fn unwrap_redirect(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if u.host_str()
        .is_some_and(|h| h == "www.google.com" || h == "google.com")
        && u.path() == "/url"
        && let Some((_, q)) = u.query_pairs().find(|(k, _)| k == "q")
    {
        return url::Url::parse(&q).ok();
    }
    Some(u)
}

/// Every `http(s)://…` token in free text, trailing punctuation trimmed.
fn urls_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("http") {
        let tail = &rest[i..];
        let is_url = tail.starts_with("https://") || tail.starts_with("http://");
        if !is_url {
            rest = &tail[4..];
            continue;
        }
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '\\'))
            .unwrap_or(tail.len());
        let token =
            tail[..end].trim_end_matches(|c| matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '}'));
        if token.len() > 8 {
            out.push(token.to_owned());
        }
        rest = &tail[end..];
    }
    out
}
