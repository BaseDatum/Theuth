# 158 as-built — CONTRACT (C158)

Branch `feat/158-contract` from origin/main f2d37aac; PIN commit `715e22d3` (`C158 PIN: 158 contract stubs (pre-gate)`).
Behaviour-neutral: every stub is inert, every new field is `#[serde(default)]`, every new config key defaults to today.
Frozen signatures below are copied from the source; waves fill bodies, never widen or rename.

## Stub map by wave

**W1 — `crates/meeting/src/detect/{mod,sensor,rules}.rs`, `MeetingConfig`/`MeetingSettings` (lib.rs)**
- `detect/mod.rs`: `DetectConfig { enabled: true, ignore: DEFAULT_IGNORE, poll_secs: 2 (5 linux) }`, `DEFAULT_IGNORE: &[&str]`
  (§4.3 list — W1 verifies the bundle ids / image names), `enum Cleared { NotNow, Never, Released, Started }`,
  `struct Detection { app, app_id, since, also, event: Option<EventMatch>, project, brief }`,
  `enum Source { Manual, Detected { app_id }, Calendar { event: EventId } }` (`#[serde(tag = "kind", rename_all = "snake_case")]`, TS `StartSource`).
  - `pub async fn rpc_detection(_rt: &Runtime) -> anyhow::Result<Option<Detection>>` → `Ok(None)`
  - `pub async fn rpc_detection_dismiss(_rt: &Runtime, _app_id: String, _never: bool) -> anyhow::Result<()>` → Err("158 W1: not built yet")
- `detect/sensor.rs`: `pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>>` → `Ok(vec![])`
- `detect/rules.rs`: `HOLD_SECS = 15`, `COOLDOWN_SECS = 600`, `RELEASE_SECS = 30`; `pub fn app_id(p: &ProcessInfo) -> String` (bundle id else name).
- `lib.rs`: `MeetingConfig` + `model, summary_model: Option<String>`, `template: String ("meeting")`, `language: String ("en")`,
  `project: Option<String>`, `detect: DetectConfig`, `calendar: CalendarConfig`; `MeetingSettings { model, summary_model, template,
  language, live_diarization, live_tools, project, detect, calendar }` with `From<&MeetingConfig>`.
  - `pub async fn rpc_settings_get(rt: &Runtime) -> anyhow::Result<MeetingSettings>` — real: reads `rt.section("meeting")`.
  - `pub async fn rpc_settings_set(_rt: &Runtime, _settings: MeetingSettings) -> anyhow::Result<MeetingSettings>` → Err.
- `MeetingEvent` + `Detected(Detection)`, `DetectionCleared { app_id, reason: Cleared }`, `AppReleased { meeting, app, released_for_s: u32 }`
  (`Upcoming` is W3's, below). `Status.detection: Option<Detection>` (always `None`). `StartMeeting.source: Option<detect::Source>`.
- Nothing publishes the new events; no exhaustive Rust match needed fixing (`check --workspace --tests` green).

**W2 — `crates/meeting/src/calendar/{mod,ics,fetch,store,matcher}.rs`, `migrations/0003_calendar.sql`**
- `calendar/mod.rs`: `CalendarConfig { eventkit: false, prebrief: true, poll_secs: 900 }`, `EventId(pub String)` (transparent),
  `Attendee`, `EventRef`, `EventMatch { event, score: f32, alternatives }`, `CalEvent`, `CalSource`, `enum CalendarEvent { EventKit { enabled }, SourcesChanged }`
  (`#[serde(tag = "type")]`; **not** subscribed in `rpc/bridge.rs` — hand-off, see below).
  - `rpc_detection_set_event(_rt, _event: Option<EventId>) -> Result<Option<Detection>>` → `Ok(None)`
  - `rpc_set_event(_rt, _id: MeetingId, _event: Option<EventId>) -> Result<Manifest>` → Err
  - `rpc_sources(_rt) -> Result<Vec<CalSource>>` → `Ok(vec![])`; `rpc_source_add(_rt, _name: String, _url: String) -> Result<CalSource>` → Err
  - `rpc_source_delete(_rt, _id: String) -> Result<()>` → Err; `rpc_source_refresh(_rt, _id: String) -> Result<CalSource>` → Err
  - `rpc_push(_rt, _client: Option<&str>, _source: String, _window: (String, String), _events: Vec<CalEvent>) -> Result<u32>` → Err
  - `rpc_source_state(_rt, _client: Option<&str>, _source: String, _state: String) -> Result<()>` → Err
    (`client` = the connection's `x-theuth-client`, `None` in-process; W2 refuses anything but `"companion"`)
  - `rpc_events(_rt, _from: String, _to: String) -> Result<Vec<CalEvent>>` → `Ok(vec![])`
  - `rpc_settings_get(rt) -> Result<CalendarConfig>` — real (config read); `rpc_settings_set(_rt, _config) -> Result<CalendarConfig>` → Err
- `calendar/ics.rs`: `pub fn expand(_text: &str, _source: &str, _from: DateTime<Utc>, _to: DateTime<Utc>) -> anyhow::Result<Vec<CalEvent>>` → Err
- `calendar/fetch.rs`: `pub async fn fetch(_rt: &Runtime, _source: &CalSource) -> anyhow::Result<Option<Vec<CalEvent>>>` → Err
- `calendar/store.rs`: `sources(&Store)`, `replace_window(&Store, source: &str, window: (&str, &str), events: &[CalEvent]) -> Result<u32>`,
  `events(&Store, from: &str, to: &str)`; `Store::pool() -> &SqlitePool` added for them.
- `calendar/matcher.rs`: `SHOW = 0.8`, `MARGIN = 0.25`, `LONE = 0.5`; `pub fn best(_now: DateTime<Utc>, _app_id: &str, _candidates: &[CalEvent]) -> Option<EventMatch>` → `None`
- `0003_calendar.sql` (wired by `sqlx::migrate!("./migrations")`): `ALTER TABLE meetings ADD COLUMN project TEXT`; `cal_sources`,
  `cal_events` (+ index `cal_events_window`), `cal_project_memory`, `cal_prebrief(event_id PK, generated_at, bullets, brief, sources)`.
- `Manifest.event: Option<EventRef>`, `StartMeeting.event: Option<EventId>`. `Cargo.toml` deps (`reqwest`, `rrule`, `chrono-tz`) are W2's.

**W3 — `crates/meeting/src/calendar/prebrief.rs`, project fields**
- `rpc_set_project(_rt, _id: MeetingId, _project: Option<String>) -> Result<Manifest>` → Err
- `remembered_project(_rt, _event: Option<&EventRef>, _title: Option<&str>) -> Result<Option<String>>` → `Ok(None)`
- `brief_for(_rt, _event: &EventRef, _project: &str) -> Result<Option<Vec<String>>>` → `Ok(None)`
- `MeetingEvent::Upcoming { event: EventRef, project, brief }`; `Manifest.project`, `MeetingSummary.project` (index row writes it from the
  manifest; `row_to_summary` returns `None` — W3 adds the column to `upsert`/`list`), `StartMeeting.project`, `ListMeetings.project`
  (carried through `meeting_list(limit, project)`, ignored by the handler), `MeetingConfig.project`.

**W4 — `crates/meeting/src/post/suggest.rs`, `crates/meeting/src/templates.rs`**
- `suggest.rs`: `enum SuggestionSource` (TS `SpeakerSuggestionSource`), `struct Suggestion { label, name, email, profile: Option<ProfileId>, confidence: f32, source, evidence }`
  (TS `SpeakerSuggestion`); `rpc_suggest(_rt, _id: MeetingId) -> Result<Vec<Suggestion>>` → `Ok(vec![])`.
- `templates.rs`: `DIR = "templates"`, `DEFAULT = "meeting"`, `Section` (TS `TemplateSection`), `Template` (TS `MeetingTemplate`);
  `rpc_templates(_rt) -> Result<Vec<Template>>` → `Ok(vec![])`; `rpc_template_put(_rt, _slug: String, _body: String) -> Result<Template>` → Err;
  `rpc_template_delete(_rt, _slug: String) -> Result<()>` → Err; `rpc_set_template(_rt, _id: MeetingId, _template: String) -> Result<Manifest>` → Err.
- `Manifest.template: String` (default `"meeting"`), `StartMeeting.template`, `MeetingConfig.template`.

**W5 — `crates/mcp-server` (workspace member; `rpc` depends on it, never the reverse)**
- `McpServerConfig { enabled: false, port: 7788 }` (section `mcp_server`, `DEFAULT_PORT`), `McpServerStatus { enabled, port, running, url, token: Option<String> }`,
  `AuditLine { at, transport, client, tool, args_sha256, ok, ms }` (TS `McpAuditLine`).
- `config(rt) -> McpServerConfig`; `start(_rt)` / `stop(_rt)` → `Ok(())`; `status(rt) -> Result<McpServerStatus>` (config read, `running: false`);
  `set(_rt, _config) -> Result<McpServerStatus>` → Err; `rotate_token(_rt) -> Result<()>` → Err; `audit_tail(_rt, _limit: u32) -> Result<Vec<AuditLine>>` → `Ok(vec![])`.
- Workspace `rmcp` line gained `"server", "transport-streamable-http-server"` (rmcp 3.2.0 names verified). No plugin, no listener.
- W5 wires the `trait Rpc` impls in `crates/theuth` (the catalogue cannot import `rpc`).

**W6** — `Manifest.stats: Option<Stats { echo_dupes, mic_segments }>` (TS `MeetingStats`), always `None`.

## RPC (`crates/rpc/src/api.rs`, after `set_meeting_live_diarization`; impls after it too)
`meeting_detection` → `detect::rpc_detection`; `meeting_detection_dismiss(app_id, never)` → `detect::rpc_detection_dismiss`;
`meeting_detection_set_event(event)` → `calendar::rpc_detection_set_event`; `meeting_settings_get/set` → `meeting::rpc_settings_get/set`;
`meeting_set_project(id, project)` → `calendar::prebrief::rpc_set_project`; `meeting_set_event(id, event)` → `calendar::rpc_set_event`;
`meeting_set_template(id, template)` → `templates::rpc_set_template`; `meeting_suggest_speakers(id)` → `post::suggest::rpc_suggest`;
`meeting_templates` / `meeting_template_put(slug, body)` / `meeting_template_delete(slug)` → `templates::rpc_*`;
`calendar_sources` / `calendar_source_add(name, url)` / `calendar_source_delete(id)` / `calendar_source_refresh(id)` → `calendar::rpc_*`;
`calendar_push(source, window, events)` and `calendar_source_state(source, state)` (`with_extensions`; `client_of(ext)` passes the header) → `calendar::rpc_push` / `rpc_source_state`;
`calendar_events(from, to)` → `calendar::rpc_events`; `calendar_settings_get/set` → `calendar::rpc_settings_get/set`;
`mcp_server_get` / `mcp_server_set(config)` / `mcp_server_rotate_token` / `mcp_server_audit(limit)` → `mcp_server::status/set/rotate_token/audit_tail`.
`meeting_list(limit, project)` — third param added, passed into `ListMeetings`. Errors map through `stub_err` (`anyhow` → `ASK_FAILED`).

## UI (`crates/desktop/ui`)
- `rpc/api.ts`: every method above in `Api` and `METHODS`; `meeting_list(limit, project?)`. Bindings regenerated (gitignored) — new files:
  `Detection`, `DetectConfig`, `Cleared`, `StartSource`, `MeetingSettings`, `CalendarConfig`, `EventId`, `Attendee`, `EventRef`, `EventMatch`, `CalEvent`,
  `CalSource`, `CalendarEvent`, `SpeakerSuggestion`, `SpeakerSuggestionSource`, `MeetingTemplate`, `TemplateSection`, `MeetingStats`,
  `McpServerConfig`, `McpServerStatus`, `McpAuditLine`.
- `settings/Calendar.tsx` (W2) and `settings/Mcp.tsx` (W5): placeholder component + `export const ready = false`; `SettingsDialog.tsx` TABS gained
  `['calendar', 'Calendar', CalendarDays]` after Meetings and `['mcp', 'MCP server', Plug]` last; `NOT_READY` filters them out until a wave flips its flag
  (the route still resolves, showing the placeholder). `meeting/DetectionBanner.tsx` (W1) and `meeting/EventSwitcher.tsx` (W2) render `null`;
  `MeetingList.tsx` renders `<DetectionBanner />` under the header.

## Swift (`crates/desktop/companion/Sources`)
- `TheuthKit/Models.swift`: `DetectionCleared`, `Attendee`, `EventRef`, `EventMatch`, `Detection`, `CalEvent`, `StartMeeting`, `StartSource`;
  `MeetingStatus.detection: Detection?`; `MeetingEvent` cases `.detected(Detection)`, `.detectionCleared(appId:reason:)`, `.appReleased(meeting:app:releasedForS:)`,
  `.upcoming(event:project:brief:)` — the three pre-meeting ones decode from a `type`-only probe before `Raw` (they carry no `meeting`; `brief` is a string list).
  Unknown `type`s still return `nil`.
- `TheuthKit/RpcClient.swift`: `meetingStart(_:) -> MeetingStarted`, `meetingDetection() -> Detection?`, `meetingDetectionDismiss(appId:never:)`,
  `meetingDetectionSetEvent(_:) -> Detection?`, `calendarPush(source:window:events:) -> UInt32`, `calendarSourceState(source:state:)`.

## Deviations from §15
1. `detect::Source` exports to TS as `StartSource` (`voice::Source` owns `Source.ts`); likewise `Stats`→`MeetingStats`, `Section`→`TemplateSection`,
   `Template`→`MeetingTemplate`, `Suggestion`→`SpeakerSuggestion`, `AuditLine`→`McpAuditLine`. Rust names unchanged.
2. `MeetingReducer.swift` gained one arm (`case .detected, .detectionCleared, .appReleased, .upcoming: return s`): Swift's exhaustive switch
   would not compile otherwise. W1 replaces it with the prompt logic.
3. `McpServerStatus` carries `token: Option<String>` so Settings can copy the JSON block (§12.6); W5 may keep or move it. `mcp_server::set`
   exists beside the five named stubs because `mcp_server_set` needs a delegate.
4. §4.5 names `meeting_detection_enabled` / `set_meeting_detection_enabled`; §15.2 does not — not declared (`meeting_settings_*` carries `detect.enabled`).
5. `ListMeetings` lost `Copy` (it holds `project: Option<String>`); no caller relied on it.

## Hand-offs
- W2: subscribe `CalendarEvent` in `crates/rpc/src/bridge.rs` (`Event` enum + `rt.subscribe`) — `bridge.rs` was outside C158's files.
- W1: verify/complete `DEFAULT_IGNORE` identifiers per OS; Windows C# decoder (`MeetingWire.cs`) untouched by design.
- W3: `store.rs` `upsert`/`list`/`Row` do not read or write `meetings.project` yet.

## Gates (all run after the PIN; fixes in `d682387b`, `cc0dd44f`)
- `BUILD_LABEL=158c build-lock mbx test -p meeting -p rpc -p mcp-server --no-run` → exit 0, no warnings.
- `mbx test -p meeting -p rpc -p mcp-server` → all green (meeting 75 + 56 + rpc + mcp-server 1; 0 failed).
- `BUILD_LABEL=158c build-lock mbx clippy -p meeting -p rpc -p mcp-server` → exit 0, no warnings (`clippy::all` deny).
- `BUILD_SLOTS=1 BUILD_LABEL=158c build-lock mbx check --workspace --tests` → first run failed in `nodes` (lib test includes
  `bindings/methods.json`, absent in a fresh worktree — pre-existing ordering); after the bindings export → exit 0.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; 870 files copied into
  `crates/desktop/ui/src/rpc/bindings/` (gitignored); the stale tracked `crates/rpc/bindings/MeetingSummary.ts` regenerated and committed.
- `crates/desktop/ui`: `build-lock bun install` (+ `packages/apps`), `bunx tsc --noEmit -p .` → exit 0 after two fixture fixes
  (`dev/meeting-page-fixture.ts` Manifest, `lib/attach.test.ts` MeetingSummary); `bunx vitest run` on `rpc/api.test.ts`, `lib/attach.test.ts`,
  `components/meeting/MeetingList.test.ts`, `store/meeting.test.ts` → 4 files, 58 passed (METHODS equals `methods.json`).
- Companion: `build-lock swift test` with `test.sh`'s flags → 261 tests in 44 suites passed.
