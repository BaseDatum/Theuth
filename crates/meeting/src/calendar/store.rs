//! The `cal_sources` / `cal_events` tables in `meetings.sqlite` (158 §7.4; migration
//! `0003_calendar.sql`, applied by [`crate::Store::open`]). Queries go through
//! [`crate::Store::pool`]. `start` / `end` are stored as `YYYY-MM-DDTHH:MM:SSZ` so string
//! comparison is time order; every bound crossing this module is normalised the same way.
//! `cal_project_memory` / `cal_prebrief` are W3's (`prebrief.rs`).

use anyhow::Context as _;
use chrono::{DateTime, SecondsFormat, Utc};

use super::{Attendee, CalEvent, CalSource, EventId};
use crate::Store;

/// The `eventkit` source's fixed row id (§7.2: one macOS Calendar source per engine).
pub const EVENTKIT: &str = "eventkit";

/// RFC 3339 UTC in the one spelling the tables compare by.
pub fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse any RFC 3339 and respell it as [`stamp`].
pub fn normalise(s: &str) -> anyhow::Result<String> {
    Ok(stamp(parse(s)?))
}

/// Any RFC 3339 → UTC.
pub fn parse(s: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.with_timezone(&Utc))
        .with_context(|| format!("not RFC 3339: {s:?}"))
}

type SourceRow = (
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn source_from(r: SourceRow) -> CalSource {
    CalSource {
        id: r.0,
        kind: r.1,
        name: r.2,
        enabled: r.3 != 0,
        last_fetch: r.4,
        last_error: r.5,
        state: r.6,
    }
}

/// Every source row, ICS after EventKit, then by name.
pub async fn sources(store: &Store) -> anyhow::Result<Vec<CalSource>> {
    let rows: Vec<SourceRow> = sqlx::query_as(
        "SELECT id, kind, name, enabled, last_fetch, last_error, state FROM cal_sources ORDER BY kind, name",
    )
    .fetch_all(store.pool())
    .await
    .context("list cal_sources")?;
    Ok(rows.into_iter().map(source_from).collect())
}

/// One source row.
pub async fn source(store: &Store, id: &str) -> anyhow::Result<Option<CalSource>> {
    let row: Option<SourceRow> = sqlx::query_as(
        "SELECT id, kind, name, enabled, last_fetch, last_error, state FROM cal_sources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(store.pool())
    .await
    .context("read cal_source")?;
    Ok(row.map(source_from))
}

/// Insert (or, for the fixed `eventkit` row, keep) a source. `secret` is the vault name for
/// ICS rows.
pub async fn insert_source(
    store: &Store,
    s: &CalSource,
    secret: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO cal_sources (id, kind, name, secret, enabled, state) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, state = COALESCE(excluded.state, state)",
    )
    .bind(&s.id)
    .bind(&s.kind)
    .bind(&s.name)
    .bind(secret)
    .bind(i64::from(s.enabled))
    .bind(&s.state)
    .execute(store.pool())
    .await
    .context("insert cal_source")?;
    Ok(())
}

/// Remove a source; its events cascade. `false` when there was no such row.
pub async fn delete_source(store: &Store, id: &str) -> anyhow::Result<bool> {
    // `ON DELETE CASCADE` needs the pragma, which sqlx's sqlite pool does not switch on.
    sqlx::query("DELETE FROM cal_events WHERE source = ?")
        .bind(id)
        .execute(store.pool())
        .await
        .context("delete cal_events of source")?;
    let n = sqlx::query("DELETE FROM cal_sources WHERE id = ?")
        .bind(id)
        .execute(store.pool())
        .await
        .context("delete cal_source")?
        .rows_affected();
    Ok(n > 0)
}

/// What a conditional GET needs from the last fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchMeta {
    /// `ETag` of the last 200.
    pub etag: Option<String>,
    /// `Last-Modified` of the last 200.
    pub last_modified: Option<String>,
}

/// The validators of the last successful fetch.
pub async fn fetch_meta(store: &Store, id: &str) -> anyhow::Result<FetchMeta> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT etag, last_modified FROM cal_sources WHERE id = ?")
            .bind(id)
            .fetch_optional(store.pool())
            .await
            .context("read fetch meta")?;
    Ok(row
        .map(|(etag, last_modified)| FetchMeta {
            etag,
            last_modified,
        })
        .unwrap_or_default())
}

/// A fetch (or push) succeeded: validators, `last_fetch = now`, error cleared.
pub async fn mark_fetched(store: &Store, id: &str, meta: &FetchMeta) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE cal_sources SET etag = ?, last_modified = ?, last_fetch = ?, last_error = NULL WHERE id = ?",
    )
    .bind(&meta.etag)
    .bind(&meta.last_modified)
    .bind(stamp(Utc::now()))
    .bind(id)
    .execute(store.pool())
    .await
    .context("mark fetched")?;
    Ok(())
}

/// A fetch failed: the (already redacted) message lands on the row; rows and validators stay.
pub async fn mark_failed(store: &Store, id: &str, error: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE cal_sources SET last_error = ? WHERE id = ?")
        .bind(error)
        .bind(id)
        .execute(store.pool())
        .await
        .context("mark failed")?;
    Ok(())
}

/// The companion-reported EventKit state (`granted` / `denied` / `not_determined`).
pub async fn set_state(store: &Store, id: &str, state: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE cal_sources SET state = ? WHERE id = ?")
        .bind(state)
        .bind(id)
        .execute(store.pool())
        .await
        .context("set state")?;
    Ok(())
}

/// Replace `source`'s rows whose `start` lies inside `window` (RFC 3339 bounds) with
/// `events`; returns the rows written. One transaction: a reader never sees the gap.
pub async fn replace_window(
    store: &Store,
    source: &str,
    window: (&str, &str),
    events: &[CalEvent],
) -> anyhow::Result<u32> {
    let (from, to) = (normalise(window.0)?, normalise(window.1)?);
    let now = stamp(Utc::now());
    let mut tx = store.pool().begin().await.context("begin")?;
    sqlx::query("DELETE FROM cal_events WHERE source = ? AND start >= ? AND start <= ?")
        .bind(source)
        .bind(&from)
        .bind(&to)
        .execute(&mut *tx)
        .await
        .context("clear window")?;
    let mut n = 0u32;
    for e in events {
        let (start, end) = (normalise(&e.start)?, normalise(&e.end)?);
        if start < from || start > to {
            continue;
        }
        sqlx::query(
            "INSERT INTO cal_events (id, source, uid, recurrence_id, sequence, title, start, \"end\", all_day, \
             tz, location, description, join_url, join_host, organizer_name, organizer_email, attendees, \
             cancelled, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET sequence = excluded.sequence, title = excluded.title, \
             start = excluded.start, \"end\" = excluded.\"end\", all_day = excluded.all_day, tz = excluded.tz, \
             location = excluded.location, description = excluded.description, join_url = excluded.join_url, \
             join_host = excluded.join_host, organizer_name = excluded.organizer_name, \
             organizer_email = excluded.organizer_email, attendees = excluded.attendees, \
             cancelled = excluded.cancelled, updated_at = excluded.updated_at",
        )
        .bind(&e.id.0)
        .bind(source)
        .bind(&e.uid)
        .bind(&e.recurrence_id)
        .bind(i64::from(e.sequence))
        .bind(&e.title)
        .bind(&start)
        .bind(&end)
        .bind(i64::from(e.all_day))
        .bind(&e.tz)
        .bind(&e.location)
        .bind(&e.description)
        .bind(&e.join_url)
        .bind(&e.join_host)
        .bind(e.organizer.as_ref().map(|o| o.name.clone()))
        .bind(e.organizer.as_ref().and_then(|o| o.email.clone()))
        .bind(serde_json::to_string(&e.attendees)?)
        .bind(i64::from(e.cancelled))
        .bind(&now)
        .execute(&mut *tx)
        .await
        .context("insert cal_event")?;
        n += 1;
    }
    tx.commit().await.context("commit window")?;
    Ok(n)
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: String,
    source: String,
    uid: String,
    recurrence_id: Option<String>,
    sequence: i64,
    title: String,
    start: String,
    end: String,
    all_day: i64,
    tz: Option<String>,
    location: Option<String>,
    description: Option<String>,
    join_url: Option<String>,
    join_host: Option<String>,
    organizer_name: Option<String>,
    organizer_email: Option<String>,
    attendees: String,
    cancelled: i64,
}

/// The `SELECT` list [`EventRow`] reads (sqlx wants literal SQL; keep the two queries in step).
macro_rules! event_cols {
    () => {
        "id, source, uid, recurrence_id, sequence, title, start, \"end\" AS end, all_day, tz, \
         location, description, join_url, join_host, organizer_name, organizer_email, attendees, cancelled"
    };
}

fn event_from(r: EventRow) -> CalEvent {
    let organizer = match (r.organizer_name, r.organizer_email) {
        (None, None) => None,
        (name, email) => Some(Attendee {
            name: name
                .or_else(|| email.as_deref().map(|m| super::local_part(m).to_owned()))
                .unwrap_or_default(),
            email,
            status: None,
            is_me: false,
        }),
    };
    CalEvent {
        id: EventId(r.id),
        source: r.source,
        uid: r.uid,
        recurrence_id: r.recurrence_id,
        sequence: u32::try_from(r.sequence).unwrap_or(0),
        title: r.title,
        start: r.start,
        end: r.end,
        all_day: r.all_day != 0,
        tz: r.tz,
        location: r.location,
        description: r.description,
        join_url: r.join_url,
        join_host: r.join_host,
        organizer,
        attendees: serde_json::from_str(&r.attendees).unwrap_or_default(),
        cancelled: r.cancelled != 0,
    }
}

/// Rows overlapping `[from, to]` (RFC 3339), `start` ascending.
pub async fn events(store: &Store, from: &str, to: &str) -> anyhow::Result<Vec<CalEvent>> {
    let (from, to) = (normalise(from)?, normalise(to)?);
    let rows: Vec<EventRow> = sqlx::query_as(concat!(
        "SELECT ",
        event_cols!(),
        " FROM cal_events WHERE \"end\" >= ? AND start <= ? ORDER BY start, id"
    ))
    .bind(&from)
    .bind(&to)
    .fetch_all(store.pool())
    .await
    .context("list cal_events")?;
    Ok(rows.into_iter().map(event_from).collect())
}

/// One row by id.
pub async fn event(store: &Store, id: &EventId) -> anyhow::Result<Option<CalEvent>> {
    let row: Option<EventRow> = sqlx::query_as(concat!(
        "SELECT ",
        event_cols!(),
        " FROM cal_events WHERE id = ?"
    ))
    .bind(&id.0)
    .fetch_optional(store.pool())
    .await
    .context("read cal_event")?;
    Ok(row.map(event_from))
}
