//! 158 W2 owns this file (stub from C158).
//!
//! Calendar (158 §7, §8): EventKit rows pushed by the macOS companion and ICS subscriptions
//! fetched by the engine land in one store in `meetings.sqlite` (migration `0003_calendar.sql`);
//! the matcher scores them against a detected app. Submodules: `ics` (reader + expansion),
//! `fetch` (subscription GET), `store` (the `cal_*` tables), `matcher` (§8.1), `prebrief` (W3,
//! §9). Every function below is inert until its wave fills it: no network, no SQL, no bus.

pub mod fetch;
pub mod ics;
pub mod matcher;
pub mod prebrief;
pub mod store;

use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::detect::Detection;
use crate::{Manifest, MeetingId};

/// `meeting.calendar.*` (158 §6.1). Nested in [`crate::MeetingConfig`]; every key defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct CalendarConfig {
    /// Accept EventKit pushes from the macOS companion (§7.2).
    pub eventkit: bool,
    /// Generate pre-meeting briefs (§9.4).
    pub prebrief: bool,
    /// ICS subscription poll in seconds (§7.3).
    pub poll_secs: u32,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            eventkit: false,
            prebrief: true,
            poll_secs: 900,
        }
    }
}

/// `source:uid:recurrence_id` — the `cal_events.id` key (§7.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct EventId(pub String);

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One attendee as the seed and the speaker suggestions see it (§8.3, §10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Attendee {
    /// Display name (the email's local part when the source has no CN).
    pub name: String,
    /// Mail address, when the source has one.
    #[serde(default)]
    pub email: Option<String>,
    /// `ACCEPTED` / `TENTATIVE` / `DECLINED` / `NEEDS-ACTION` as the source says.
    #[serde(default)]
    pub status: Option<String>,
    /// The user (EventKit `isCurrentUser`; ICS: the organiser's own address list).
    #[serde(default)]
    pub is_me: bool,
}

/// The event a meeting is tied to, persisted in the manifest because the calendar row
/// rotates out of the window (§8.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EventRef {
    /// Store key.
    pub id: EventId,
    /// The series (`UID` / `calendarItemExternalIdentifier`), not the instance.
    pub uid: String,
    /// Summary line.
    pub title: String,
    /// RFC 3339 UTC.
    pub start: String,
    /// RFC 3339 UTC.
    pub end: String,
    /// First meeting link in location/description/conference fields.
    #[serde(default)]
    pub join_url: Option<String>,
    /// Its host (`zoom.us`, `meet.google.com`, …) for the §8.1 host term.
    #[serde(default)]
    pub join_host: Option<String>,
    /// Organiser.
    #[serde(default)]
    pub organizer: Option<Attendee>,
    /// Everyone invited, the user included (`is_me`).
    #[serde(default)]
    pub attendees: Vec<Attendee>,
    /// Description / agenda, first 1 000 chars.
    #[serde(default)]
    pub description: Option<String>,
}

/// The matcher's answer for one detection (§8.1): the shown event, its score, the rest of the
/// window ordered by score for the `[▾]` list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EventMatch {
    /// Best candidate.
    pub event: EventRef,
    /// Its §8.1 score.
    pub score: f32,
    /// Other candidates in the window, best first.
    #[serde(default)]
    pub alternatives: Vec<EventRef>,
}

/// One `cal_events` row on the wire (§7.4): [`EventRef`]'s fields plus the store's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CalEvent {
    /// Store key.
    pub id: EventId,
    /// Owning `cal_sources.id`.
    pub source: String,
    /// Series id.
    pub uid: String,
    /// Title.
    pub title: String,
    /// RFC 3339 UTC.
    pub start: String,
    /// RFC 3339 UTC.
    pub end: String,
    /// Meeting link.
    #[serde(default)]
    pub join_url: Option<String>,
    /// Link host.
    #[serde(default)]
    pub join_host: Option<String>,
    /// Organiser.
    #[serde(default)]
    pub organizer: Option<Attendee>,
    /// Attendees.
    #[serde(default)]
    pub attendees: Vec<Attendee>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// `VALUE=DATE` events never match (§8.4).
    #[serde(default)]
    pub all_day: bool,
    /// The event's own zone for display; `start`/`end` stay UTC.
    #[serde(default)]
    pub tz: Option<String>,
    /// `STATUS:CANCELLED` / declined instance.
    #[serde(default)]
    pub cancelled: bool,
    /// `SEQUENCE`; a bump invalidates the prebrief cache (§9.4).
    #[serde(default)]
    pub sequence: u32,
    /// `LOCATION`.
    #[serde(default)]
    pub location: Option<String>,
    /// `RECURRENCE-ID` of an overridden instance.
    #[serde(default)]
    pub recurrence_id: Option<String>,
}

/// One `cal_sources` row on the wire (§7.4, §7.6). The ICS URL itself never leaves the vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CalSource {
    /// Row id.
    pub id: String,
    /// `eventkit` | `ics`.
    pub kind: String,
    /// User-given name.
    pub name: String,
    /// Fetched / pushed at all.
    pub enabled: bool,
    /// RFC 3339 of the last successful fetch or push.
    #[serde(default)]
    pub last_fetch: Option<String>,
    /// Redacted error of the last attempt.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Companion-reported state for EventKit (`granted` / `denied` / `not_running`).
    #[serde(default)]
    pub state: Option<String>,
}

/// Bus events of the calendar side (§15.1); W2 subscribes the RPC bridge to them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum CalendarEvent {
    /// `meeting.calendar.eventkit` flipped; the companion starts or stops pushing.
    EventKit {
        /// New value.
        enabled: bool,
    },
    /// A source was added, removed, refreshed or errored; Settings re-reads the list.
    SourcesChanged,
}

/// `meeting_detection_set_event(event) -> Option<Detection>` (§8.2, W2). Stub: no prompt.
pub async fn rpc_detection_set_event(
    _rt: &Runtime,
    _event: Option<EventId>,
) -> anyhow::Result<Option<Detection>> {
    Ok(None)
}

/// `meeting_set_event(id, event) -> Manifest` (§8.2, W2). Stub: refused.
pub async fn rpc_set_event(
    _rt: &Runtime,
    _id: MeetingId,
    _event: Option<EventId>,
) -> anyhow::Result<Manifest> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_sources() -> Vec<CalSource>` (§7.4, W2). Stub: none.
pub async fn rpc_sources(_rt: &Runtime) -> anyhow::Result<Vec<CalSource>> {
    Ok(Vec::new())
}

/// `calendar_source_add(name, url) -> CalSource` (§7.3, W2): vault row `calendar.ics.<id>` +
/// `cal_sources` row + first fetch. Stub: refused.
pub async fn rpc_source_add(
    _rt: &Runtime,
    _name: String,
    _url: String,
) -> anyhow::Result<CalSource> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_source_delete(id)` (W2). Stub: refused.
pub async fn rpc_source_delete(_rt: &Runtime, _id: String) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_source_refresh(id) -> CalSource` (W2). Stub: refused.
pub async fn rpc_source_refresh(_rt: &Runtime, _id: String) -> anyhow::Result<CalSource> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_push(source, window, events) -> u32` (§7.2, W2) — companion only: `client` is the
/// connection's `x-theuth-client` header (`None` when the call did not come over the socket);
/// W2 refuses anything but `companion`. Returns the rows written. Stub: refused.
pub async fn rpc_push(
    _rt: &Runtime,
    _client: Option<&str>,
    _source: String,
    _window: (String, String),
    _events: Vec<CalEvent>,
) -> anyhow::Result<u32> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_source_state(source, state)` (§7.2, W2) — companion only, same `client` rule as
/// [`rpc_push`]. Stub: refused.
pub async fn rpc_source_state(
    _rt: &Runtime,
    _client: Option<&str>,
    _source: String,
    _state: String,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}

/// `calendar_events(from, to) -> Vec<CalEvent>` (§7.4, W2), RFC 3339 bounds. Stub: none.
pub async fn rpc_events(
    _rt: &Runtime,
    _from: String,
    _to: String,
) -> anyhow::Result<Vec<CalEvent>> {
    Ok(Vec::new())
}

/// `calendar_settings_get() -> CalendarConfig` (§7.6, W2): the `meeting.calendar` block as
/// configured. Reads config only — inert before W2.
pub async fn rpc_settings_get(rt: &Runtime) -> anyhow::Result<CalendarConfig> {
    let cfg: crate::MeetingConfig = rt.section("meeting").unwrap_or_default();
    Ok(cfg.calendar)
}

/// `calendar_settings_set(CalendarConfig) -> CalendarConfig` (W2): persist and apply. Stub: refused.
pub async fn rpc_settings_set(
    _rt: &Runtime,
    _config: CalendarConfig,
) -> anyhow::Result<CalendarConfig> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}
