# Meetings phase 3 — island, eye toggle, hotkeys, desktop surfaces

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 8, 9, 10
(the hotkey half), 11 (the island half) and §4.1 (start form), §4.3
(control-center live log), §4.6 (brief log); fills 53's missing §7. Depends
on phase 1's RPC and phase 2's events; buildable against a fake event feed
(§7). Everything here is `crates/desktop` (Rust shell + `ui/`) plus one
setter RPC. Contract: `README.md` §3.

Goal of the phase: start a meeting from the tray, see topic buttons and
briefs in a tiny always-on-top panel at the notch that never steals focus and
is excluded from screen capture, tap/dismiss/deeper/PTT from it, flip the eye
for both windows, and watch the live log in the main window.

## 0. Decisions local to this phase

1. **One React bundle, two windows, routed by label.** The island window
   loads the same `index.html` at `/island` (`WebviewUrl::App("index.html#/island")`
   or the router's `initialEntries` from `getCurrentWindow().label`). The
   island route renders only `<Island/>` — no sidebar, no `SearchHost`, no
   `useHotkeys()`: `routes/__root.tsx:16-46` `Root` mounts `AppSidebar`,
   `SearchHost`, `DragOverlay` unconditionally, so `Root` branches on the
   window label (or the `/island` path) and renders a bare `<Outlet/>` for
   the island. `useTauri()` (`ui/src/App.tsx:194-225`) registers ⌥Space/⌥Esc
   gated on `container() === 'macos'` only — add `getCurrentWindow().label
   === "main"` or a second webview double-registers; same for
   `installDictationListeners()` (`App.tsx:314`) and `useInvalidateOn`'s
   session polling if they are per-window. Rejected: a second Vite entry (a
   second build, a second RPC client init path).
2. **Panel conversion, capture exclusion and placement are Rust, in one new
   module `crates/desktop/src/island.rs`, exposed as three Tauri commands.**
   `tauri-nspanel` (git, ahkohd) turns the `island` `WebviewWindow` into an
   `NSPanel` with `NSWindowStyleMaskNonactivatingPanel`, level
   `NSStatusWindowLevel`, collection behaviour `canJoinAllSpaces |
   fullScreenAuxiliary | stationary`; `set_content_protected(true)` on both
   windows for the eye; `NSScreen` (`objc2-app-kit`, `NSScreen` +
   `safeAreaInsets` + `auxiliaryTopLeftArea`) for placement. The shell's rule
   "OS integration only" (`lib.rs` header) is exactly this. Rejected: doing
   placement in JS via `@tauri-apps/api/window` (no notch geometry there).
3. **Settings that are the desktop's live in `localStorage` + zustand
   (`ui/src/lib/nudges.ts:14-20` pattern: plain `create` + manual
   `localStorage`, no `persist` middleware); no daemon setting.**
   `hidden_from_capture` (default `true`), PTT key (default ``Alt+` ``), eye
   key (default `Alt+Shift+E`), island position per screen configuration —
   all desktop. (`meeting.allow_read_only` and its setter RPC were dropped
   with 02 §0.2 c on 2026-09-20.) 53 §2.2 named `tauri-plugin-store` for
   positions; the tree already persists per-app settings in `localStorage`
   and a second store is a second convention. Both webviews are the same
   origin in one app → one `localStorage` [INFERENCE: WKWebView shares the
   default website data store per app; verify in check 2; fallback is a
   Tauri event from main to island on change].
4. **The brief stack and cursor are island state, mirrored to the daemon.**
   Per 53 §4.6: `source == Auto` and cursor at top → show; `Auto` and
   stepped back → queue + `↑ N new`; any other source → show and jump.
   Every cursor move calls `meeting_cursor`. Reconnect: `meeting_briefs` +
   `meeting_topics` rebuild the stack (`Status.cursor` restores position;
   the ts binding is `MeetingStatus.ts` — 02 §10 deviation 8).
5. **Hotkeys are registered from the main webview only, via
   `@tauri-apps/plugin-global-shortcut` with `event.state`
   (`Pressed`/`Released`).** PTT: `Pressed` → `meeting_ptt(true)`,
   `Released` → `meeting_ptt(false)`. Eye: `Pressed` → toggle. Registered
   only while `Status.state` is `Recording|Paused` (a global ⌥` outside a
   meeting is a stolen key). Rebinding: a text field on the Meetings tab
   that captures a chord with a `keydown` reader (`settings/Shortcuts.tsx`
   is a static table; no capture component exists). Conflicts (`register`
   throws) → toast, keep the old binding.

## 1. Rust shell (`crates/desktop`)

- `Cargo.toml`: a new `[target.'cfg(target_os = "macos")'.dependencies]`
  table (none exists today; the iOS-only table at `:55-58` has `objc2 = "0.6"`,
  `block2`, `objc2-ui-kit`):
  `tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2" }`
  (2.0.1; depends on `cocoa 0.26`/`objc 0.2`/`objc_id` — the old objc
  crates, which coexist with `objc2` in one process; its `to_panel()`,
  `get_webview_panel`, `set_style_mask`, `set_level`,
  `set_collection_behaviour`, `order_front_regardless` are the surface used;
  register `tauri_nspanel::init()` in `configure`), `objc2 = "0.6"`,
  `objc2-app-kit = { version = "0.3", features = ["NSScreen", "NSWindow",
  "NSApplication"] }` (0.3.2 already transitive in the lock),
  `objc2-foundation = "0.3"`. Root `[workspace.dependencies]` entries with `# 53`.
  The `.app` bundles `Frameworks/`; nothing new to ship.
- `tauri.conf.json` `app.windows` gains
  `{ "label": "island", "url": "index.html#/island", "visible": false,
  "decorations": false, "transparent": true, "alwaysOnTop": true,
  "skipTaskbar": true, "resizable": false, "focus": false,
  "width": 440, "height": 44, "shadow": false }`. `bundle.macOS.minimumSystemVersion`
  `"12.0"` → `"15.0"` (53 §2.3; also makes `LSMinimumSystemVersion` 15).
  Note in 25 at integration.
- `Info.plist` gains `NSAudioCaptureUsageDescription` ("theuth records the
  audio of the meeting app you choose while a meeting is recording.") beside
  `NSMicrophoneUsageDescription` (update its text: it now also covers
  meetings).
- `capabilities/default.json`: `"windows": ["main", "island"]`; add
  `core:window:allow-set-position`, `allow-set-size`, `allow-outer-position`,
  `allow-current-monitor`, `allow-start-dragging` (already), `allow-set-ignore-cursor-events`
  (for a click-through collapsed strip if wanted; optional).
- `src/island.rs` (new, `#[cfg(target_os = "macos")]`):
  - `pub fn init(app: &AppHandle) -> anyhow::Result<()>` — called from
    `desktop::setup` (lib.rs:199): `app.get_webview_window("island")` →
    `to_panel()`, style mask non-activating, level status, collection
    behaviour, `set_content_protected(hidden_default)` on both windows
    where `hidden_default` is read from the `hidden_from_capture` command's
    last value (JS calls `set_hidden_from_capture` on boot; before that, both
    default to `true` — decision 9's default).
  - `#[tauri::command] set_hidden_from_capture(app, hidden: bool)` → both
    windows `set_content_protected(hidden)`; emits `capture-hidden` (bool)
    to all windows so the island's eye icon and the main window's badge
    agree.
  - `#[tauri::command] island_show(app, expanded: bool)` /
    `island_hide(app)` — show never focuses (`NSPanel` non-activating +
    `orderFrontRegardless`); `expanded` sets height 44 → 168.
  - `#[tauri::command] island_place(app, saved: Option<(f64, f64)>) ->
    (f64, f64, String /* config key */)` — config key = sorted list of
    `NSScreen.frame` rects; if `saved` is given, apply it; else default:
    on the screen with `safeAreaInsets.top > 0` (notched built-in) center
    the window on the notch with its top at the screen top (content under
    the bezel is hidden — 53 §2.3); otherwise center at the top of the
    main screen just under the menu bar. Returns the applied position and
    key so JS stores `positions[key] = (x, y)`; a drag end re-saves.
    Screen changes: `NSApplicationDidChangeScreenParametersNotification`
    → emit `screens-changed`; JS re-calls `island_place`.
- `lib.rs:183-188` `on_window_event` `CloseRequested → hide` already fits
  both windows; keep. `lib.rs:175-180` `invoke_handler!` gains the three
  commands. Tray menu (`lib.rs:212-222`) gains `("meeting", "Start
  meeting…")` before `settings`; `on_menu` (L293): `"meeting" → navigate
  (app, "/meetings/new")`.
- `ui/src/lib/tauri.ts` gains the three `invoke` wrappers and `onCaptureHidden`,
  `onScreensChanged`.

## 2. UI: island (`ui/src/routes/island.tsx`, `ui/src/components/meeting/Island*.tsx`)

Layout, 440 × 44 collapsed: `[●] [rec 00:14] [topic][topic][topic] … [👁] [⋮]`
where `●` is the meter pair (two 6 px bars: `mic_rms`, `system_rms`),
`rec mm:ss` is elapsed from `Started.at` on the recording clock (pause →
`⏸`), topic buttons are `Topic.label` (≤ 4 words, one line, `max_topics`
of them, oldest left; overflow never happens because the actor caps), the
eye is the capture toggle (struck through when hidden), `⋮` is pause/stop.
A right-click on a topic button = dismiss (also a tiny `×` on hover).
Expanded, 440 × 168 (a brief is showing): the strip above plus a brief pane:
title line, 2–3 bullets, a `deeper` link, and on the right `↑`/`↓` arrows
with the `↑ N new` badge (decision 11). No scrollbar; no live transcript
(53 §4.3). Badges in the strip (small text, not modal): *headphones
recommended* (`MicNoAec`), *transcription down* (`SttDown`), *model down*,
*disk low*, *10 min left* (`NearMaxLength`), *approval waiting*
(`ApprovalWaiting`), *listening…* while `Listening{held:true}`.

Interaction: tap topic → `meeting_tap` (the button greys until `Brief`
arrives, then vanishes on `TopicRemoved{Viewed}`); `deeper` →
`meeting_deeper(currentBrief)`; arrows → cursor move + `meeting_cursor`.
Clicks never focus the panel (NSPanel does this; verify — check 4). The
strip is draggable via `data-tauri-drag-region`.

State: a zustand `island` store fed by `on('MeetingEvent')`-style
subscription — `dispatch` (`ui/src/store/sessions.ts:553`) already fans out
every wire event via `on(type, cb)`; `MeetingEvent` is added to `WireEvent`
(`ui/src/rpc/events.ts:48-77`) and the meeting store subscribes to
`Started/State/Level/TopicAdded/TopicRemoved/Brief/Listening/ApprovalWaiting/
Summarized/Deleted`. `Level` updates a ref, not React state (10 Hz redraw of
two bars via CSS variables).

Showing/hiding the window: `Started` → `island_show(false)`; the first
`Brief` → `island_show(true)`; `State{Done|Failed}` → `island_hide`. The main
window may be hidden the whole time (Accessory policy; ⌥Space toggles it as
today).

## 3. UI: main window

- `ui/src/rpc/api.ts` `Api` + `METHODS` (sorted array at `:372`; `api.test.ts`
  checks it against `bindings/methods.json`): every `meeting_*` from phases
  1–2. `ui/src/lib/rpc.ts` `queryKeys` (`:15`):
  `meetings`, `meeting(id)`, `meetingStatus`, `meetingBriefs(id)`,
  `meetingLive(id)`, `meetingProcesses`, `meetingPermissions`;
  `useInvalidateOn`: `Started/State/Deleted → meetings, meetingStatus`;
  `Summarized → meeting(id), meetings`.
- Routes: `/meetings` (list: date, title, duration, state, process; click →
  `/meetings/$id`; a *Start meeting…* button), `/meetings/new` (start form),
  `/meetings/$id` (control center). Sidebar (`components/sidebar/AppSidebar.tsx:64-85`) gains a
  `Meetings` link under Artifacts; the session list shows a small meeting
  glyph for sessions with `project === "meetings"`.
- Start form (53 §4.1): process picker (`meeting_processes`, active first,
  last choice by `bundle_id` remembered in `localStorage`), title, context
  textarea dictated with `lib/dictation.ts` (54: `start(api, before, after,
  mic)` / `stop` / `discard` / `onFinish` / `useDictation`; the store is
  global with one `active`, so the form takes an `owner` flag exactly as
  `Composer.tsx:130-137,162-203` does — the 45 voice-note path is inline
  `MediaRecorder` code in `Composer.tsx:392-430`, not a component, and is
  not reused), consent checkbox (required;
  text from decision 14), model picker (existing model `Select` +
  `queryKeys.models` from `settings/Loop.tsx:57-60`, pre-filtered to the catalog's fast tier if the catalog
  exposes a tier; else unfiltered with a one-line hint), a permissions row
  (`meeting_permissions`, with the same "open System Settings" opener the
  Computer pane uses). *Start* → `meeting_start` → navigate to
  `/meetings/$id`.
- Control center (`/meetings/$id`, while recording): status header
  (state, elapsed, degradations, pause/resume/stop), a live log
  (`Utterance` events + `meeting_live` on load; `me:`/`them:` styling,
  `request` rows flagged; a `final: false` event replaces the open row for
  that track in place — dictation-style — and `final: true` commits it), a
  typed request box (`meeting_request`), the
  brief log (`meeting_briefs` + `Brief` events, cursor mirrored from
  `Status.cursor`, expired-topic briefs included per 53 §4.6), and a link to
  the session transcript (`/c/$session`) — approvals surface there as
  everywhere. After the meeting the same route shows the manifest and, in
  phase 4, review/summary.
- Settings tab `Meetings` (`SettingsDialog.tsx:46-62` `TABS` + `Pane` `:81-103`):
  hidden from capture (switch + the eye hotkey field), PTT hotkey field,
  a note that screenshots of hidden windows come out black (53 §2.3).

## 4. Hotkeys (main webview, `ui/src/lib/meeting-hotkeys.ts`)

`useMeetingHotkeys()` in `App.tsx` beside `useTauri()`: subscribes to the
meeting store's `state`; on enter `Recording|Paused` registers PTT and eye
with `register(shortcut, (e) => …)` (`e.state` is `'Pressed' | 'Released'`
in plugin-global-shortcut 2.x); on leave unregisters. PTT `Released` also
fires if the key was held across a state change (unregister → send
`meeting_ptt(false)` if held).

## 5. RPC additions (`crates/rpc/src/api.rs`, `crates/meeting`)

None. Every method the island and control center call exists after phase 2
(`meeting_processes/start/pause/resume/stop/status/list/get/delete/
permissions/tap/dismiss/deeper/request/ptt/cursor/briefs/topics/live`).
The only desktop↔daemon addition is the ts side: `MeetingEvent` into
`WireEvent`, the methods into `METHODS`.

## 6. Not in this phase

Review/summary panes (04). TTS into an earpiece, calendar briefs (53 "not
in v1"). Screen-share detection (decision 9: never automatic). Windows/Linux
(the `island` window is created only on macOS; other targets skip
`island::init` and hide the tray entry).

## 7. Building against a fake feed

Phase 2 may not be done. `ui/src/dev/meeting-fixture.ts` (dev-only, behind
`import.meta.env.DEV` and a `?fixture=meeting` search param on `/island`):
replays a scripted `MeetingEvent[]` through `dispatch` on a timer —
`Started`, `Level` at 10 Hz, three `TopicAdded`, a `Brief{Auto}`, a
`Brief{Tap}`, `Listening`, `ApprovalWaiting`, `State{Done}`. Deleted at
integration; it is a scaffold, not a test.

## 8. Check

1. `bun run check`, vitest (island store: stack/cursor/badge rules —
   `Auto` while stepped back queues; `Tap` jumps; viewed topic removed).
2. `bun run tauri dev` (or the `.app`): main hidden, `island` window
   appears on `Started` (fixture or a real phase-1 meeting) centred on the
   notch of the built-in display; move to an external display
   configuration → repositions under the menu bar; drag it, unplug/replug →
   position remembered per configuration. `localStorage` written from the
   Meetings tab is read by the island (decision 3's inference) — else
   implement the fallback event.
3. Full-screen Zoom (or a full-screen Safari) → the island stays visible
   over it, on every Space.
4. Click a topic button while Zoom has focus → Zoom keeps focus (the
   menu bar app name does not change; typing still goes to Zoom).
5. Capture exclusion: with `hidden_from_capture: true`, `cmd-shift-4` on
   the island region → black/absent; in Zoom → Share Screen, the remote side
   (or Zoom's own preview) does not show the island or the main window;
   toggle the eye hotkey → both appear in the share; toggle back. Record the
   Zoom/Meet/Teams results in 53 decision 8's honesty note at integration.
6. Hold PTT while the meeting app has focus → `Listening{true}`, *listening…*
   badge; release → `Listening{false}`; the key does nothing when no meeting
   records (unregistered).
7. Briefs: three `Auto` briefs arrive while at top → each replaces the view;
   step down one; a fourth arrives → `↑ 1 new`, view unchanged; a `Tap`
   brief arrives → jumps into view, badge cleared or recounted.
8. Tray → Start meeting… → form; consent unchecked → Start disabled;
   permissions row reflects `meeting_permissions`; start → control center
   live log shows `me:`/`them:` rows within seconds; typed request →
   brief; approval card in `/c/$session`; `ApprovalWaiting` badge on the
   island.
9. Meetings tab: rebind PTT to a conflicting chord → toast, old binding
   kept; flip *hidden from capture* → both windows flip (`capture-hidden`
   event) and the island's eye icon agrees.
10. `meeting.enabled: false` → tray entry present but the form shows
    "meetings are disabled" from the RPC error; island never created
    visible.

## 9. Status

Implemented 2026-09-20. Rust: `crates/desktop/src/island.rs` (+≈170), `lib.rs`
(plugin, commands, tray entry, window-state denylist), `tauri.conf.json`
(island window, `macOSPrivateApi`, minimum 15.0), `Info.plist`,
`capabilities/default.json`, workspace deps (`tauri-nspanel` v2 git,
`objc2-app-kit`, `objc2-foundation`, `block2`; `tauri` gains
`macos-private-api`). UI: `store/meeting.ts` (+test), `lib/meeting-settings.ts`,
`lib/meeting-hotkeys.ts`, `lib/meeting-line.ts` (+test),
`components/meeting/{Island,StartForm,ControlCenter,MeetingList,MeetingPill}.tsx`,
`components/chat/MeetingCard.tsx`, `components/settings/Meetings.tsx`, routes
`/island`, `/meetings`, `/meetings/new`, `/meetings/$id`, `dev/meeting-fixture.ts`;
`api.ts`/`events.ts`/`rpc.ts` wiring; sidebar link + glyph. Verified with
`bun run tauri dev` (`THEUTH_DATA_DIR=/tmp/theuth-meet`, the phase-2 scratch dir,
`target/debug/t`, haiku as `session_model`) on macOS 15, one external LG
2560×1440 (lid closed — no notch present during the shell-side checks),
QuickTime Player playing the phase-2 two-voice clip as the meeting app.

### Exercised

1. `bun run check` clean; vitest 34 files / 184 tests incl. `store/meeting.test.ts`
   (Auto at top replaces, Auto stepped back queues + `↑ N new` counts down on
   ↑, Tap jumps and clears, viewed topic removed, cues end with the
   recording) and `lib/meeting-line.test.ts` (every `session.rs` line shape);
   `cargo check -p desktop`, `cargo clippy -p desktop -p meeting --all-targets
   --no-deps -D warnings` clean; `cargo test -p meeting` 14/14 (+1).
   `api.test.ts` passes against `bindings/methods.json` (19 `meeting_*`).
2. (shell side) Island window created hidden, `to_panel` applied: CG reports
   layer 25 (`NSStatusWindowLevel`), `sharing 0` on both windows from boot
   (decision 9's default, before any JS), 440 × 44. On `Started` →
   `island_place` → centred under the LG's menu bar at (1060, 30) =
   `((2560−440)/2, visibleFrame top)`; `island_show`; on `Done` → hidden.
   Drag → `Moved` → saved under key `0,0,2560,1440`; the next show re-applied
   the saved point; the user's drags during dogfood were remembered across
   stop/start. Both webviews share one `localStorage` (one
   `localstorage.sqlite3` under `~/Library/WebKit/theuth-desktop`, the origin is
   the same) — decision 3's inference holds; each webview nonetheless
   persists what it reads and the `capture-hidden` event syncs the eye, so
   nothing depends on it. Notch placement and the external-display
   reposition were **not exercised** (no built-in display in the run) — the
   arithmetic is `island_place`'s `safeAreaInsets.top > 0` branch.
5. (shell side) `screencapture -R` of the island region with the eye hidden
   → the island is *absent* (the window behind shows through), not black;
   with the eye visible the strip renders (meters, `rec 00:33`, eye, ⋮),
   transparent corners. ⌥⇧E while recording flipped both windows'
   `kCGWindowSharingState` 0 ↔ 1 (sent through System Events; the
   `capture-hidden` event updated the settings store — the toggle persisted).
   Zoom/Meet/Teams share results: pending the user's call (below).
7. Live: the clip produced `meeting_topic` (*SRE-2386 image tags*) → button
   in the strip; tap → `Brief{Tap}` → the pane expanded with title, three
   bullets, arrows. The stack rules themselves are unit-tested; the fixture
   replays the §7 sequence (`/island?fixture=meeting` in a browser, or
   `VITE_MEETING_FIXTURE=1` for the Tauri island).
8. Over RPC on the running app: `meeting_start` → island shown; typed
   request → `[meeting request, typed]`; `meeting_deeper` →
   `Brief{Deeper, parent}` in ≈5 s (after deviation 8); `meeting_stop` →
   island hidden. The user drove tray → form → control center, the request
   box, taps and drags during dogfood (this session).

### Not exercised (as of this writing; the user's Zoom run fills these in)

- Check 2 notch/external-display reposition and position memory across
  unplug/replug; check 3 (full-screen Zoom, every Space); check 4 (focus stays
  with Zoom on a click — `NSPanel` non-activating + `becomesKeyOnlyIfNeeded`,
  and the request box's `resignKeyWindow` on blur); check 5's share results
  per app; check 6 PTT end to end (the hotkey registers/unregisters on
  `Recording|Paused` — exercised only as ⌥⇧E); check 9 conflicting-chord toast;
  check 10 disabled mode (needs a daemon relaunch with
  `THEUTH_MEETING__ENABLED=false`); the `.app` bundle (TCC prompts are the
  real ones only there).

### Deviations from the design above

1. **`island_show(width, height)`, not `expanded: bool`; the island is
   user-resizable.** Fixed 44/168 px truncated real briefs (bullets wrap to
   2–3 lines at 440 px). The webview measures its blocks (`ResizeObserver`
   on the strip, the pane's content and the request box) and asks for that
   height; the panel's style mask includes `Resizable`, an edge drag saves
   `sizes[key]` and from then on the pane scrolls inside the user's size.
   Clamp 300–1200 × 44–800.
2. **A request box on the island** (bottom row → `meeting_request`): the
   user asked for a way to steer without leaving the call. Typing makes the
   non-activating panel key (`becomesKeyOnlyIfNeeded`); blur/Enter/Escape
   → `island_resign_key` hands key back to the meeting app. 53 §4.3's "no
   live transcript on the island" still holds.
3. **`island_place(positions)` takes the whole map** and answers the key it
   used; §1's `saved: Option<(x, y)>` needed the key before the call.
4. **The island window is never ordered out.** A `WKWebView` whose window is
   off screen is throttled by WebKit to the point that its RPC client took
   30–45 s to connect and, in one run, never did (one client on the daemon
   instead of two; the user saw no island for a whole meeting). `init` shows
   the window once (tao's `show()`) and *conceals* it — `alphaValue 0` +
   `ignoresMouseEvents`, still ordered in at status level; `island_show`
   raises alpha and takes mouse events again, `island_hide` conceals.
   Measured after the change: both webviews connected 3 s after the port
   came up; the island appeared ≤ 3 s after `meeting_start`. (Earlier "it
   worked" runs were the ones where `tauri-plugin-window-state` had shown
   the island at boot before it was denylisted there.) The app stays
   inactive (non-activating panel); check 4 is the user's confirmation.
5. **`tauri-plugin-window-state` denylists `island`**: it restored and
   *showed* the island at boot (default state → `visible`), fighting
   `island_place`.
6. **`app.macOSPrivateApi: true` + `tauri/macos-private-api`**: a transparent
   window needs it; without, Tauri warns and paints opaque.
7. **`useMeetingHotkeys` gates on `live(state)`**; the eye hotkey is a
   `setHiddenFromCapture(!hidden)` from the settings store, the PTT handler
   tracks `held` and closes the span on unregister. Rebinding: the field
   probes `register`+`unregister` before saving; a conflict toasts.
8. **Daemon changes, against §5's "none"** (the user asked during dogfood):
   (a) `session::nudge_line` — when a `TurnFinished` (non-error) leaves a
   `Request`/`Deeper` prompt unanswered by `meeting_brief`, the actor steers
   one reminder (`Outstanding.nudged`; taps exempt); observed: text answer →
   nudge → `Brief{Typed}` in ≈5 s. (b) `deeper_line` carries `— parent:
   <BriefId>` and the prompt block says to pass it: the model had only the
   title and `meeting_brief.parent` failed to parse as a UUID. (c) the
   prompt block names the reminder line. `crates/rpc` untouched; bindings
   unchanged.
9. **Meeting cards in the session transcript** (`MeetingCard`, by parsing
   the `[meeting …]` marker as 45 §7.4 does for voice notes): batches as
   `me`/`them` rows, requests/taps/dismissals/deeper/reminders as chips.
   **A meeting pill** (`MeetingPill`) with clock, pause/resume, stop and a
   jump to the control center, inline at the right end of every header band
   (chat, search, knowledge, artifacts, meetings, settings) while a meeting
   is live — inline so it pushes the header's own controls left rather than
   covering them; the control center's own pause/stop moved into it.
   *Superseded 2026-09-22 by one `MeetingBar` along the bottom of the routed
   column (53 §4.8): the per-header pill mis-landed with split panes.* **Session activity** (`TurnStarted`/`ToolCall*`/
   `TurnFinished` of the meeting session) shows as a spinner + `thinking…`
   / tool name in the strip. All user requests from dogfood.
10. `useTauri`/`useEventSideEffects`/`installDictationListeners` are skipped
    in the island webview (`isIsland()`; `Root` renders `Outlet` there);
    `hydrateMeeting` runs in `loadInfo` so a reconnect or a late webview
    rebuilds the store from `meeting_status` + `meeting_briefs`.
11. Island window `url` omitted (memory history keyed on the label);
    `Info.plist` mic text covers dictation and meetings. The start form's
    model `Select` is unfiltered with a one-line hint (`ModelInfo` has no
    tier). Permissions row shows *Open System Settings* only for `Denied`;
    `NotDetermined` reads "asked on first start" (01 §0.6).
12. Not this phase (unchanged, noted for 53): a topic's precomputed brief is
    shown only on tap (53 §4.6 "stored unshown"); the user expected it on
    arrival. One line in the daemon's `AddTopic` if 53 changes its mind.

### Capture exclusion per app (decision 8's honesty note)

`screencapture` (CGWindowList): honoured — the island is absent from the
image. Zoom / Meet / Teams shares: pending the user's run; to be recorded
here and in 53 at integration.
