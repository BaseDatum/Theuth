//! Calendar (158 §7, §8): EventKit rows pushed by the macOS companion and ICS subscriptions
//! fetched by the engine land in one store in `meetings.sqlite` (migration `0003_calendar.sql`);
//! the matcher scores them against a detected app. Submodules: `ics` (calcard glue),
//! `fetch` (subscription GET), `store` (the `cal_*` tables), `matcher` (§8.1), `prebrief` (W3,
//! §9). The RPC delegates below reach the [`Meeting`] actor for its store and config
//! ([`Handles`]); the poll task starts on first use ([`ensure_poll`]).

pub mod fetch;
pub mod ics;
pub mod matcher;
pub mod prebrief;
pub mod store;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::detect::Detection;
use crate::{MEETING_ACTOR, Manifest, Meeting, MeetingEvent, MeetingId, Store};

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

/// Rolling window every source is kept for: −1 day … +14 days (§7.2).
pub fn window() -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    (
        now - chrono::Duration::days(1),
        now + chrono::Duration::days(14),
    )
}

/// Description cap in the seed and the manifest (§7.5).
pub const DESCRIPTION_CHARS: usize = 1000;

/// `local` of `local@host`.
pub fn local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}

fn head(s: &str, chars: usize) -> &str {
    match s.char_indices().nth(chars) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

impl CalEvent {
    /// The manifest-side view (§8.3): description capped at [`DESCRIPTION_CHARS`].
    pub fn to_ref(&self) -> EventRef {
        EventRef {
            id: self.id.clone(),
            uid: self.uid.clone(),
            title: self.title.clone(),
            start: self.start.clone(),
            end: self.end.clone(),
            join_url: self.join_url.clone(),
            join_host: self.join_host.clone(),
            organizer: self.organizer.clone(),
            attendees: self.attendees.clone(),
            description: self
                .description
                .as_deref()
                .map(|d| head(d, DESCRIPTION_CHARS).trim().to_owned())
                .filter(|d| !d.is_empty()),
        }
    }
}

impl EventRef {
    /// `A, B, C (organizer: A)` — names only, never emails (§7.5).
    pub fn attendee_line(&self) -> Option<String> {
        let names: Vec<&str> = self
            .attendees
            .iter()
            .map(|a| a.name.as_str())
            .filter(|n| !n.is_empty())
            .collect();
        if names.is_empty() {
            return None;
        }
        let mut s = format!("Attendees: {}", names.join(", "));
        if let Some(o) = self.organizer.as_ref().filter(|o| !o.name.is_empty()) {
            s.push_str(&format!(" (organizer: {})", o.name));
        }
        Some(s)
    }

    /// The lines the seed adds to the start context (§8.3): title, description, attendees.
    pub fn context_block(&self) -> String {
        let mut s = format!("Event: {} ({} – {})", self.title, self.start, self.end);
        if let Some(d) = &self.description {
            s.push('\n');
            s.push_str(d);
        }
        if let Some(a) = self.attendee_line() {
            s.push('\n');
            s.push_str(&a);
        }
        s
    }
}

/// §8.3: what a matched event contributes to a manifest at start (or on `meeting_set_event`):
/// `event`, the title when none was typed, and the context block the `started.` line and the
/// alias scan read (attendee names reach the brief cache through it).
pub fn seed(manifest: &mut Manifest, e: &CalEvent) {
    let r = e.to_ref();
    if manifest
        .title
        .as_deref()
        .is_none_or(|t| t.trim().is_empty())
    {
        manifest.title = Some(r.title.clone());
    }
    let block = r.context_block();
    manifest.context = Some(match manifest.context.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => format!("{c}\n\n{block}"),
        _ => block,
    });
    manifest.event = Some(r);
}

/// The `[▾]` list's candidates and the §8.1 match for `app_id` at `now`, from the store.
pub async fn match_for(store: &Store, now: DateTime<Utc>, app_id: &str) -> Option<EventMatch> {
    let from = store::stamp(now - chrono::Duration::minutes(matcher::AFTER_MIN));
    let to = store::stamp(now + chrono::Duration::minutes(matcher::BEFORE_MIN));
    match store::events(store, &from, &to).await {
        Ok(rows) => matcher::best(now, app_id, &rows),
        Err(e) => {
            tracing::warn!("calendar: candidates: {e:#}");
            None
        }
    }
}

// ---- actor messages: the delegates below get a `Runtime`; the store and config live on the actor.

/// `ask` → `(store, calendar config)`.
pub(crate) struct Handles;

impl Message<Handles> for Meeting {
    type Reply = (Arc<Store>, CalendarConfig);

    async fn handle(&mut self, _: Handles, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        (self.store.clone(), self.cfg.calendar.clone())
    }
}

/// `tell`: `calendar_settings_set` applied the file; the actor follows.
pub(crate) struct SetCalendarConfig(pub CalendarConfig);

impl Message<SetCalendarConfig> for Meeting {
    type Reply = ();

    async fn handle(&mut self, m: SetCalendarConfig, _: &mut Context<Self, Self::Reply>) {
        self.cfg.calendar = m.0;
    }
}

/// `ask` → the manifest after `meeting_set_event` (§8.2).
pub(crate) struct SetEvent {
    pub meeting: MeetingId,
    pub event: Option<CalEvent>,
}

impl Message<SetEvent> for Meeting {
    type Reply = Result<Manifest, String>;

    async fn handle(&mut self, m: SetEvent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let live = self
            .current
            .as_ref()
            .is_some_and(|r| r.manifest.id == m.meeting);
        let mut manifest = match self.current.as_ref().filter(|_| live) {
            Some(r) => r.manifest.clone(),
            None => self
                .store
                .manifest(m.meeting)
                .map_err(|e| format!("{e:#}"))?,
        };
        let steer = match &m.event {
            Some(e) => {
                let r = e.to_ref();
                if manifest
                    .title
                    .as_deref()
                    .is_none_or(|t| t.trim().is_empty())
                {
                    manifest.title = Some(r.title.clone());
                }
                let line = format!(
                    "event: {} · {}",
                    r.title,
                    r.attendee_line()
                        .unwrap_or_else(|| "attendees: none listed".into())
                );
                manifest.event = Some(r);
                line
            }
            None => {
                manifest.event = None;
                "event: none".to_owned()
            }
        };
        if let Some(rec) = self.current.as_mut().filter(|r| r.manifest.id == m.meeting) {
            rec.manifest = manifest.clone();
            let at = crate::session::hms(
                rec.clock.elapsed_at(std::time::Instant::now()).as_millis() as u64,
            );
            // Attendee names become aliases the live scan can hit from now on.
            if let Some(e) = &manifest.event {
                let hits = self.scan(&e.attendee_line().unwrap_or_default());
                self.cached_topics(hits, 0, std::time::Instant::now()).await;
            }
            self.deliver(format!("[meeting {at}] {steer}")).await;
        }
        self.save(&manifest).await;
        Ok(manifest)
    }
}

/// The running actor, or why not.
pub(super) fn actor() -> anyhow::Result<ActorRef<Meeting>> {
    ActorRef::<Meeting>::lookup(MEETING_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("meeting is not running"))
}

/// The store and the calendar config from the actor; starts the poll task on first use.
pub(crate) async fn handles() -> anyhow::Result<(Arc<Store>, CalendarConfig)> {
    let (store, cfg) = actor()?
        .ask(Handles)
        .await
        .map_err(|e| anyhow::anyhow!("meeting: {e}"))?;
    Ok((store, cfg))
}

static POLL: OnceLock<()> = OnceLock::new();

/// Start the subscription poll task once per process (§7.3): every `poll_secs` each enabled
/// ICS source is refreshed; `SourcesChanged` goes out after a pass that changed a row.
pub fn ensure_poll(rt: &Runtime) {
    let rt = rt.clone();
    POLL.get_or_init(move || {
        tokio::spawn(async move {
            loop {
                let secs = match handles().await {
                    Ok((store, cfg)) => {
                        poll_once(&rt, &store).await;
                        cfg.poll_secs.max(60)
                    }
                    Err(_) => 60,
                };
                tokio::select! {
                    () = rt.shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(u64::from(secs))) => {}
                }
            }
        });
    });
}

async fn poll_once(rt: &Runtime, store: &Store) {
    let sources = match store::sources(store).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("calendar: sources: {e:#}");
            return;
        }
    };
    let mut changed = false;
    for s in sources.iter().filter(|s| s.kind == "ics" && s.enabled) {
        let before = (s.last_fetch.clone(), s.last_error.clone());
        if let Ok(after) = fetch::refresh(rt, store, &s.id).await {
            changed |= (after.last_fetch, after.last_error) != before;
        }
    }
    if changed {
        rt.publish(CalendarEvent::SourcesChanged).await;
    }
}

async fn store_and_poll(rt: &Runtime) -> anyhow::Result<Arc<Store>> {
    let (store, _) = handles().await?;
    ensure_poll(rt);
    Ok(store)
}

// ---- RPC delegates (`crates/rpc/src/api.rs` W2 rows).

/// `meeting_detection_set_event(event) -> Option<Detection>` (§8.2): remember the pick for the
/// current prompt's app, re-match, re-publish the prompt. `None` when there is no prompt.
pub async fn rpc_detection_set_event(
    rt: &Runtime,
    event: Option<EventId>,
) -> anyhow::Result<Option<Detection>> {
    let Some(mut detection) = crate::detect::rpc_detection(rt).await? else {
        return Ok(None);
    };
    let store = store_and_poll(rt).await?;
    matcher::choose(&detection.app_id, event);
    detection.event = match_for(&store, Utc::now(), &detection.app_id).await;
    // 158 §9.3 / §9.4 (W3): the project chip and a cached brief follow the new pick.
    let cx = prebrief::ctx().await?;
    prebrief::decorate(&store, cx.default_project.as_deref(), &mut detection).await;
    // W1 close: the engine's held prompt follows, so `meeting_detection` / `Status.detection` agree.
    if let Ok(Some(actor)) = ActorRef::<Meeting>::lookup(crate::MEETING_ACTOR) {
        let _ = actor
            .ask(crate::detect::SetPromptEvent {
                app_id: detection.app_id.clone(),
                event: detection.event.clone(),
                project: detection.project.clone(),
                brief: detection.brief.clone(),
            })
            .await;
    }
    rt.publish(MeetingEvent::Detected(detection.clone())).await;
    Ok(Some(detection))
}

/// `meeting_set_event(id, event) -> Manifest` (§8.2).
pub async fn rpc_set_event(
    rt: &Runtime,
    id: MeetingId,
    event: Option<EventId>,
) -> anyhow::Result<Manifest> {
    let store = store_and_poll(rt).await?;
    let event = match event {
        Some(e) => Some(
            store::event(&store, &e)
                .await?
                .ok_or_else(|| anyhow::anyhow!("unknown calendar event {e}"))?,
        ),
        None => None,
    };
    actor()?
        .ask(SetEvent { meeting: id, event })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// `calendar_sources() -> Vec<CalSource>` (§7.4).
pub async fn rpc_sources(rt: &Runtime) -> anyhow::Result<Vec<CalSource>> {
    let store = store_and_poll(rt).await?;
    store::sources(&store).await
}

/// `calendar_source_add(name, url) -> CalSource` (§7.3): vault row `calendar.ics.<id>` +
/// `cal_sources` row + first fetch. The URL is validated, stored, and never returned.
pub async fn rpc_source_add(rt: &Runtime, name: String, url: String) -> anyhow::Result<CalSource> {
    let store = store_and_poll(rt).await?;
    let url = fetch::normalise_url(&url)?;
    let name = name.trim().to_owned();
    let name = if name.is_empty() {
        url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| "Calendar".into())
    } else {
        name
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let secrets = secrets::actor().ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    secrets
        .ask(secrets::Set {
            name: fetch::secret_name(&id),
            value: url,
            note: Some(format!("calendar subscription {name}")),
            source: "calendar".into(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let row = CalSource {
        id: id.clone(),
        kind: "ics".into(),
        name,
        enabled: true,
        last_fetch: None,
        last_error: None,
        state: None,
    };
    store::insert_source(&store, &row, Some(&fetch::secret_name(&id))).await?;
    let row = fetch::refresh(rt, &store, &id).await?;
    rt.publish(CalendarEvent::SourcesChanged).await;
    Ok(row)
}

/// `calendar_source_delete(id)`: the row, its events and the vault secret.
pub async fn rpc_source_delete(rt: &Runtime, id: String) -> anyhow::Result<()> {
    let store = store_and_poll(rt).await?;
    let Some(row) = store::source(&store, &id).await? else {
        anyhow::bail!("unknown calendar source");
    };
    store::delete_source(&store, &id).await?;
    if row.kind == "ics"
        && let Some(secrets) = secrets::actor()
        && let Err(e) = secrets
            .ask(secrets::Delete {
                name: fetch::secret_name(&id),
            })
            .await
    {
        tracing::warn!(source = id, "calendar: delete secret: {e}");
    }
    rt.publish(CalendarEvent::SourcesChanged).await;
    Ok(())
}

/// `calendar_source_refresh(id) -> CalSource`: one fetch now.
pub async fn rpc_source_refresh(rt: &Runtime, id: String) -> anyhow::Result<CalSource> {
    let store = store_and_poll(rt).await?;
    let Some(row) = store::source(&store, &id).await? else {
        anyhow::bail!("unknown calendar source");
    };
    if row.kind != "ics" {
        anyhow::bail!("macOS Calendar is pushed by the companion; nothing to fetch");
    }
    let row = fetch::refresh(rt, &store, &id).await?;
    rt.publish(CalendarEvent::SourcesChanged).await;
    Ok(row)
}

fn companion_only(client: Option<&str>) -> anyhow::Result<()> {
    if client == Some("companion") {
        Ok(())
    } else {
        anyhow::bail!("companion only")
    }
}

/// `calendar_push(source, window, events) -> u32` (§7.2) — companion only: a full replace of
/// the `eventkit` source's rows inside `window`. Returns the rows written.
pub async fn rpc_push(
    rt: &Runtime,
    client: Option<&str>,
    source: String,
    window: (String, String),
    events: Vec<CalEvent>,
) -> anyhow::Result<u32> {
    companion_only(client)?;
    if source != store::EVENTKIT {
        anyhow::bail!("only the eventkit source is pushed");
    }
    let (store, cfg) = handles().await?;
    ensure_poll(rt);
    if !cfg.eventkit {
        anyhow::bail!("macOS Calendar is switched off in Settings › Calendar");
    }
    store::insert_source(&store, &eventkit_row(), None).await?;
    let events: Vec<CalEvent> = events
        .into_iter()
        .map(|mut e| {
            e.source = source.clone();
            e.id = EventId(format!(
                "{source}:{}:{}",
                e.uid,
                e.recurrence_id.as_deref().unwrap_or("")
            ));
            e
        })
        .collect();
    let n = store::replace_window(&store, &source, (&window.0, &window.1), &events).await?;
    store::mark_fetched(&store, &source, &store::FetchMeta::default()).await?;
    rt.publish(CalendarEvent::SourcesChanged).await;
    Ok(n)
}

fn eventkit_row() -> CalSource {
    CalSource {
        id: store::EVENTKIT.into(),
        kind: "eventkit".into(),
        name: "macOS Calendar".into(),
        enabled: true,
        last_fetch: None,
        last_error: None,
        state: None,
    }
}

/// `calendar_source_state(source, state)` (§7.2) — companion only: `granted` / `denied` /
/// `not_determined` onto the `eventkit` row.
pub async fn rpc_source_state(
    rt: &Runtime,
    client: Option<&str>,
    source: String,
    state: String,
) -> anyhow::Result<()> {
    companion_only(client)?;
    if source != store::EVENTKIT {
        anyhow::bail!("only the eventkit source reports state");
    }
    let store = store_and_poll(rt).await?;
    let mut row = eventkit_row();
    row.state = Some(state.to_ascii_lowercase());
    store::insert_source(&store, &row, None).await?;
    store::set_state(&store, &source, row.state.as_deref().unwrap_or_default()).await?;
    rt.publish(CalendarEvent::SourcesChanged).await;
    Ok(())
}

/// `calendar_events(from, to) -> Vec<CalEvent>` (§7.4), RFC 3339 bounds.
pub async fn rpc_events(rt: &Runtime, from: String, to: String) -> anyhow::Result<Vec<CalEvent>> {
    let store = store_and_poll(rt).await?;
    store::events(&store, &from, &to).await
}

/// `calendar_settings_get() -> CalendarConfig` (§7.6): the actor's live block, else the file.
pub async fn rpc_settings_get(rt: &Runtime) -> anyhow::Result<CalendarConfig> {
    if let Ok((_, cfg)) = handles().await {
        ensure_poll(rt);
        return Ok(cfg);
    }
    let cfg: crate::MeetingConfig = rt.section("meeting").unwrap_or_default();
    Ok(cfg.calendar)
}

/// `calendar_settings_set(CalendarConfig) -> CalendarConfig`: `meeting.calendar.*` in
/// `config.yaml`, the actor's copy, and `CalendarEvent::EventKit` when that switch moved.
pub async fn rpc_settings_set(
    rt: &Runtime,
    config: CalendarConfig,
) -> anyhow::Result<CalendarConfig> {
    let before = rpc_settings_get(rt).await?;
    let path = rt.data_dir.join("config.yaml");
    for (key, value) in [
        ("eventkit", config.eventkit.to_string()),
        ("prebrief", config.prebrief.to_string()),
        ("poll_secs", config.poll_secs.to_string()),
    ] {
        runtime::write_nested_config_value(&path, "meeting", "calendar", key, &value)
            .map_err(|e| anyhow::anyhow!("write meeting.calendar.{key}: {e}"))?;
    }
    if let Ok(a) = actor() {
        let _ = a.tell(SetCalendarConfig(config.clone())).await;
    }
    if before.eventkit != config.eventkit {
        rt.publish(CalendarEvent::EventKit {
            enabled: config.eventkit,
        })
        .await;
    }
    Ok(config)
}
