# 158 live proof — W1–W4 on a scratch engine (2026-10-10)

Worker LIVE, worktree `theuth-158-live`, branch `feat/158-live` from main `0ac16fa3`. Nothing in `crates/` or `packages/` was
changed; this file and `docs/design/assets/158-live-*.png` are the only additions. Defects are recorded, not fixed.

## Setup

- Memory gate: `vm_stat` free + speculative 6.75 GB at 18:29 (build), 4.34 GB at 18:39 (engine start). Load 17–29.
- `BUILD_LABEL=158live CARGO_BUILD_JOBS=4 build-lock mbx build -p theuth --bin t` → 5 m 42 s (mbx cache 2553 hits).
  `build-lock bun install` in `crates/desktop/ui` (623 packages) and `packages/apps` (484).
- Scratch engine: `env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH target/debug/t --data-dir /tmp/158-live-data --headless`,
  `config.yaml`: `rpc.listen 127.0.0.1:27977`, `secrets.key_store: file`, `agent.model` / `meeting.model`
  `anthropic/claude-haiku-4-5`, `meeting.calendar.prebrief: true`, `poll_secs: 60`. No models dir, no vault secrets; the only
  model credential was `ANTHROPIC_API_KEY` already in this worker's process environment (genai env fallback). Sky's engine,
  data dir, ports 7777/27788 and secrets were not touched.
  Trap worth knowing: this shell inherits `THEUTH_RPC__LISTEN=127.0.0.1:55266` (the parent engine's port); figment merges
  `THEUTH_*` over `config.yaml` (`crates/theuth/src/main.rs:1532`), so the first two starts died with "plugin rpc failed to
  start: Address already in use" until the variable was unset.
- UI: `build-lock bunx vite --port 25173 --host 127.0.0.1` with `VITE_THEUTH_URL=ws://127.0.0.1:27977` and
  `VITE_THEUTH_TOKEN=$(cat /tmp/158-live-data/token)`; headless managed Chromium 1280×860 (not Sky's Chrome); screenshots
  are the harness's 2× WebP captures converted to PNG with ffmpeg. Bus events captured over `subscribe_events` on a second
  WebSocket (`/tmp/158-live-events.jsonl`).
- Fixtures (hand-written, no audio): two `Done` meetings titled "Ops sync" (14 d and 7 d ago, each with `transcript.json` +
  `summary.md`), one `Review` meeting "Intro call" whose System-track segment at 00:04 is a self-introduction; ICS served by
  `python3 -m http.server 25180 --bind 127.0.0.1`: UID `ops-sync-weekly@158.live`, `RRULE:FREQ=WEEKLY;COUNT=10`, SEQUENCE 3,
  `DTSTART;TZID=Europe/London` so the 2026-10-10 instance ran 17:51–18:31 UTC (added at 17:45:38, T−5.5), three attendees
  (Alice Chen = organiser, Bob Stone, Sky Moore), a Zoom LOCATION and a three-item DESCRIPTION.
- Browser-mode caveat: `DetectionBanner` and the start form are gated to a Tauri container (`DetectionBanner.tsx:30`
  `isDesktop()`, `platform.ts:5` looks for `window.__TAURI_INTERNALS__`); a plain browser shows no banner and the start form
  says "Meetings are recorded from the Mac running the node". For proofs 1 and 4 the flag was set in-page *after* the
  WebSocket connection (`window.__TAURI_INTERNALS__ = {}` then an SPA `pushState`). The same flag blanks the meeting page
  (`meeting-hotkeys.ts:22` → `getCurrentWindow()`), so it was dropped (full reload) before every meeting-page step.

## Proof 1 — W1 detection → banner → Not now: **passed**

- `ffmpeg -nostdin -f avfoundation -i ":0" -t 75 -f null -` started 17:51:05 Z (pid 23327).
- `meeting_detection` at 17:51:39 Z →
  `{"app":{"pid":23327,"name":"ffmpeg","bundle_id":null,"active":true},"app_id":"ffmpeg","since":"…18:51:10+01:00",
  "also":[],"event":{"event":{"id":"<src>:ops-sync-weekly@158.live:2026-10-10T17:51:00Z","title":"Ops sync",…,"attendees":[3]},
  "score":1.0,"alternatives":[]},"project":"ops","brief":[3 bullets]}` — i.e. `Detected` at the t+16 s tick (since 18:51:10
  local), carrying the W2 match, the W3 project and the cached brief. Prompt still held at 17:52:27, after ffmpeg exited.
- Meetings page banner (`158-live-w1-banner-detected.png`): "ffmpeg is using your microphone", "Ops sync · 06:51 PM–07:31 PM",
  project badge `ops`, EventSwitcher "Ops sync · 18:51–19:31", **Start recording / Not now / ⋯**, and the sentence
  "By starting the recording you acknowledge the recording is lawful."
- Not now → `meeting_detection` = `null`; bus `17:52:34 {"type":"DetectionCleared","app_id":"ffmpeg","reason":"NotNow"}`;
  banner gone (`158-live-w1-banner-cleared.png`).
- Optional Start-recording-from-the-banner: see Proof 2b below (second detection after the 10-min Not-now cooldown).

## Proof 2 — W2 calendar: **passed** (with defect D1)

- `calendar_source_add("Live ICS fixture","http://127.0.0.1:25180/ops.ics")` → `{"id":"d2e0…","kind":"ics","enabled":true,
  "last_fetch":"2026-10-10T17:45:38Z","last_error":null}`; bus `SourcesChanged` on add/refresh/delete.
- `calendar_events(now−1h, now+24h)` → 1 row: `id d2e0…:ops-sync-weekly@158.live:2026-10-10T17:51:00Z`, `uid`, `title Ops sync`,
  `start 17:51Z`, `end 18:31Z`, `join_host zoom.us`, 3 attendees, `sequence 3`, description.
- Settings › Calendar (`158-live-w2-settings-calendar.png`, `…-upcoming.png`): source row "Live ICS fixture · synced just
  now" with Refresh / Delete; Pre-meeting briefs switch on; **Upcoming: "Ops sync · 18:51–19:31 / https://zoom.us/j/158158158
  · zoom.us"**. The Add form refuses `http://` (client-side `Calendar.tsx:147`, by design) — the loopback URL went in over RPC.
- "While it is now": the 17:51:39 detection above carried `Detection.event` = the match with `score 1.0`; and the meeting
  started from the start form at 17:55:43 (no event at start) got the event through the header EventSwitcher
  ("Which meeting is this?" → menuitemradio "Ops sync · 18:51–19:31") → `meeting_set_event` → `meeting_get` shows
  `manifest.event {id, uid ops-sync-weekly@158.live, title Ops sync, attendees 3}`; the header shows "Ops sync · 18:51–19:31"
  (`158-live-w2-header-eventswitcher.png`), the session got the steer line `event: Ops sync · Attendees: Alice Chen, Bob
  Stone, Sky Moore (organizer: Alice Chen)` (names only).
- **2b — Start recording from the banner (second detection after the 10-min Not-now cooldown).** `ffmpeg -t 240` from
  18:02:45 Z → bus `18:03:04 Detected {app ffmpeg pid 60230, since 19:02:48+01:00, event Ops sync score 1.0, project ops,
  brief 3}` (again the t+16 s tick) → banner **Start recording** → `meeting_start` with `source {kind: detected, app_id:
  ffmpeg}`, `event`, `project ops` → bus `18:03:18 DetectionCleared {reason: Started}`, `18:03:19 Started {process ffmpeg}`;
  `Status.detection = null`; `meeting_get` → `title "Ops sync"` (seeded from the event), `process {pid 60230, name ffmpeg}`,
  `project ops`, `event.uid ops-sync-weekly@158.live`, `template meeting` (no template remembered for the series — only
  `meeting_set_template` remembers, W4 decision 2). Stopped after 15 s (three tracks written; post pass failed on the
  missing ONNX runtime as in Proof 3).

## Proof 3 — W3 project + pre-meeting brief: **passed**

- Header ProjectChip on the older "Ops sync" fixture: combobox "Project" → type `ops` → "New project ops" →
  `meeting_get.project = "ops"`, index row `project = ops`, `cal_project_memory` = `title:ops sync | ops`
  (`158-live-w3-project-chip.png`). Second fixture via `meeting_set_project(id,"ops")`. (`session project: no such session`
  warnings in the log — fixture sessions do not exist; expected.)
- Brief: first prebrief tick after the source refresh generated it — `cal_prebrief` row `generated_at 17:46:17Z`, 3 bullets,
  `brief 01a126ec-…`, `sources {"meetings":[both Done ops meetings],"fingerprint":"3:03856a79c01051fa"}`; `briefs.sqlite` row
  title "Ops sync", aliases `["Ops sync","Alice","Bob","Sky"]`, tags `["pre-meeting","ops"]`. No "falling back to a side
  call" warning and zero rows in `sessions`/`model_calls` → `[INFERENCE]` the Private-root leg served it. Bullets were facts
  from the two fixture summaries (alert thresholds / runbook / rollout follow-up).
- `Upcoming` on the bus every minute from T−5 to T+4: 17:46:17, 17:47:17, 17:48:17, 17:49:17, 17:50:17, (17:51:17 missed —
  listener restarting), 17:52:17, 17:53:17, 17:54:17, 17:55:17; none from 17:56 on. Payload
  `{"type":"Upcoming","event":{id,uid,title "Ops sync",start,attendees 3,…},"project":"ops","brief":[3 bullets]}`.
- Start form (`158-live-w4-start-form.png`): Project `ops` + Note template `Standup` → `meeting_start` → Recording;
  `cal_project_memory` gained `title:live proof start | ops` at start; after the event pick, `meeting_set_project(id,"ops")`
  wrote `uid:ops-sync-weekly@158.live | ops`. On the meeting page the brief appeared as the **Cache** topic "Ops sync ·
  cached · 00:00:00" with the three bullets, and the sidebar grouped the meeting's chat under project **ops** (A2).
- Meetings list filter (`158-live-w3-list-filter.png`): "Project filter" → `ops` → the three ops meetings;
  `meeting_list(null,"ops")` → `["Live proof start/Failed","Ops sync/Done","Ops sync/Done"]`.
- Recording itself: 85 s, tracks `mic.opus / system.opus / mix.opus`; post pipeline **Failed** at Transcribing with
  "no libonnxruntime.dylib bundled, at /tmp/158-live-data/models/onnxruntime/…" — expected on a model-less scratch dir, not a
  158 defect.

## Proof 4 — W4 templates + speaker suggestions: **passed** (with defect D2)

- Settings › Meetings › Templates: Edit **Standup** (CodeMirror seeded from `Template.body`) → body replaced with a 302-byte
  text adding a "Blockers" section → Save → `/tmp/158-live-data/meetings/templates/standup.md` is **byte-identical** to
  the typed text (`Buffer.equals` true, 302 = 302); `meeting_templates` → `standup {builtin:true, edited:true, sections:
  [Done since last time, Next, Blockers]}`; the row grew "Reset to built-in" (`158-live-w4-templates-edited.png`).
  Reset to built-in → file **byte-identical** to `crates/meeting/templates/standup.md` (368 = 368), `edited:false`, button gone.
- Start form "Note template" = Standup → `manifest.template = "standup"` (and `project = "ops"`), see Proof 3.
- Review › Suggest names on "Intro call" (state Review): with the segment text **"Hi, I am Alice from the operations team"**
  `meeting_suggest_speakers` → `[]` (D2). With "Hi, I'm Alice …" → `[{"label":"s1","name":"Alice","confidence":0.8,
  "source":"Intro","evidence":"\"I'm Alice\" at 00:04"}]`; the UI row "Speaker 1 → Alice 0.80 / "I'm Alice" at 00:04 /
  Accept / Not them" (`158-live-w4-suggest-row.png`). Accept → `meeting_edit Rename` applied (speakers `s1 = Alice`, every
  segment relabelled) and the gated enrol refused with the toast **"Alice named; voice not enrolled — speaker models not
  installed under /tmp/158-live-data/models"** (`158-live-w4-suggest-accepted.png`); `meetings/speakers/` stays empty. The
  §10.4 quality gates themselves were not reached (no speaker model on the scratch engine).

## Defects found (not fixed)

1. **UTC `Z` DTSTART + RRULE expands one local offset early.** `crates/meeting/src/calendar/ics.rs:76`
   `sub.expand_dates(default_tz, LIMIT)` with `default_tz = Tz::Fixed(Local offset)` (`:45`): a feed with
   `DTSTART:20260926T175100Z` + `RRULE:FREQ=WEEKLY` produced instances at `16:51:00Z` (every one, −1 h on Europe/London
   BST); the same event with `DTSTART;TZID=Europe/London` and a single (non-recurring) UTC event were both correct. Outlook /
   Exchange ICS feeds commonly write recurring masters in UTC, so their events would match and brief an hour early. Likely
   calcard applying the default zone to `Z` times in `expand_dates`; worth a unit test in `tests/calendar.rs` either way.
2. **Intro cue misses "I am <Name>".** `crates/meeting/src/post/suggest.rs:93-95` `INTRO_RE` =
   `(?i)\b(?:i'?m|this is|my name is|it'?s)\s+…` matches "I'm" / "Im" but not "I am" — the very phrase the design brief uses
   (`say "Hi, I am Alice"`). Transcribers emit "I am" often; add `i am` to the alternation.
3. Minor UI: the detection banner shows the event line in 12-hour local time ("Ops sync · 06:51 PM–07:31 PM") next to the
   EventSwitcher in 24-hour ("Ops sync · 18:51–19:31") — two formats for the same event in one row
   (`DetectionBanner.tsx:63-67` vs `EventSwitcher.tsx`).
4. Minor: the start form's Project combobox and the header ProjectChip offer "New project ops" even when `ops` already
   exists as a meetings project (the list comes from agent session projects; meeting-only projects are not suggested). Not
   wrong, just no reuse hint.

## Not possible here

- Island prompts (macOS/Windows companion) — out of scope for the headless engine; islands were probed in W1M/W1W/W3M/W3W.
- §10.4 enrolment gates and the post pipeline need the speaker/STT models; the scratch dir has none (Sky's `~/.theuth/models`
  was not linked, per the never-touch rule).
- EventKit / a real secret ICS address — no real calendar URL was used.

## Teardown

Engine, vite, ICS server and ffmpeg stopped (only processes this worker started); `/tmp/158-live-data`,
`/tmp/158-live-ics`, `/tmp/158-live-events.jsonl`, `/tmp/158-live-fixture-ids.json` removed; Chromium tab closed.

## Fix round (F158, branch `fix/158-live-defects`)

Each defect got a failing test first, then the smallest change; all four are closed.

1. `crates/meeting/src/calendar/ics.rs`: calcard iterates an RRULE in the DTSTART's *resolved* zone, and a `Z` time with no
   TZID resolves to the default (local) — hence −1 h. A series whose DTSTART is `Z`-anchored (`utc_anchored`) is now
   expanded with `Tz::UTC` as its default. `tests/calendar.rs::utc_rrule_expands_in_utc` (weekly `T170000Z`, six
   instances across 2026-10-25) went `16:00:00Z ×6` → `17:00:00Z ×6`; the TZID (google/outlook) and single-UTC (icloud)
   tests are unchanged.
2. `crates/meeting/src/post/suggest.rs`: `INTRO_RE` gains `i am`. `intro_cue_forms_and_false_positives` failed with
   `[s2 Bob, s3 Carol, s4 Dave]` (no Alice), now passes; "I am going / not / sorry / Here / Fine" stay out via `NOT_NAMES`.
3. `DetectionBanner.tsx` renders `eventLine(ev)` from `lib/calendar` (the EventSwitcher's formatter); `lib/detection.ts`
   `timeRange` (12-hour `toLocaleTimeString`) is gone. The banner test pins `<span>${eventLine(ev)}</span>` and no `AM/PM`
   (before: `Weekly sync · 11:00 AM–11:30 AM`).
4. `lib/meeting-project.ts` `useProjectChoices` adds the distinct `project`s of the plain `meeting_list` (the cached
   `queryKeys.meetings` row the list already holds; no new RPC) — StartForm, ProjectChip and the list filter all read it.
   `meeting-project.test.ts` (sessions `theuth`, meetings `ops`) went `theuth` → `ops,theuth`. `StartForm.test.ts` mock
   gained `meeting_list`.

Gates: `mbx test -p meeting` 121 + 16 pass; `mbx clippy -p meeting` clean; `bunx tsc --noEmit -p .` 0; vitest on the
touched meeting/lib files 27/27; `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` clean. As-built:
`docs/design/158-as-built-F158.md`.
