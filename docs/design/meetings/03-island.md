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
   `useTauri()` shortcut registration (`ui/src/App.tsx:225-253` registers
   ⌥Space/⌥Esc from the webview; gate it on `label === "main"` or a second
   webview double-registers). Rejected: a second Vite entry (a second build,
   a second RPC client init path).
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
   (`ui/src/lib/nudges.ts` pattern); one daemon setting gets a setter RPC.**
   `hidden_from_capture` (default `true`), PTT key (default ``Alt+` ``), eye
   key (default `Alt+Shift+E`), island position per screen configuration —
   all desktop. `meeting.allow_read_only` is daemon config →
   `meeting_set_allow_read_only(bool)` RPC (`write_config_value("meeting",
   "allow_read_only")` + in-memory update on the actor), the `set_compact_style`
   pattern (`General.tsx:85`). 53 §2.2 named `tauri-plugin-store` for
   positions; the tree already persists per-app settings in `localStorage`
   and a second store is a second convention. Both webviews are the same
   origin in one app → one `localStorage` [INFERENCE: WKWebView shares the
   default website data store per app; verify in check 2; fallback is a
   Tauri event from main to island on change].
4. **The brief stack and cursor are island state, mirrored to the daemon.**
   Per 53 §4.6: `source == Auto` and cursor at top → show; `Auto` and
   stepped back → queue + `↑ N new`; any other source → show and jump.
   Every cursor move calls `meeting_cursor`. Reconnect: `meeting_briefs` +
   `meeting_topics` rebuild the stack (`Status.cursor` restores position).
5. **Hotkeys are registered from the main webview only, via
   `@tauri-apps/plugin-global-shortcut` with `event.state`
   (`Pressed`/`Released`).** PTT: `Pressed` → `meeting_ptt(true)`,
   `Released` → `meeting_ptt(false)`. Eye: `Pressed` → toggle. Registered
   only while `Status.state` is `Recording|Paused` (a global ⌥` outside a
   meeting is a stolen key). Rebinding: a text field on the Meetings tab
   that captures a chord (`Shortcuts` tab's capture component if one exists;
   else a `keydown` reader). Conflicts (`register` throws) → toast, keep the
   old binding.

## 1. Rust shell (`crates/desktop`)

- `Cargo.toml` `[target.'cfg(target_os = "macos")'.dependencies]`:
  `tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2" }`,
  `objc2 = "0.6"` (already a workspace-known version, iOS side), `objc2-app-kit
  = { version = "0.3", features = ["NSScreen", "NSWindow", "NSApplication"] }`,
  `objc2-foundation`. Root `[workspace.dependencies]` entries with `# 53`.
- `tauri.conf.json` `app.windows` gains
  `{ "label": "island", "url": "index.html#/island", "visible": false,
  "decorations": false, "transparent": true, "alwaysOnTop": true,
  "skipTaskbar": true, "resizable": false, "focus": false,
  "width": 440, "height": 44, "shadow": false }`. `bundle.macOS.minimumSystemVersion`
  `"12.0"` → `"15.0"` (53 §2.3; also makes `LSMinimumSystemVersion` 15).
  Note in 25 at integration.
- `Info.plist` gains `NSAudioCaptureUsageDescription` ("wth records the
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

- `ui/src/rpc/api.ts` `Api` + `METHODS`: every `meeting_*` from phases 1–2
  and `meeting_set_allow_read_only`. `ui/src/lib/rpc.ts` `queryKeys`:
  `meetings`, `meeting(id)`, `meetingStatus`, `meetingBriefs(id)`,
  `meetingLive(id)`, `meetingProcesses`, `meetingPermissions`;
  `useInvalidateOn`: `Started/State/Deleted → meetings, meetingStatus`;
  `Summarized → meeting(id), meetings`.
- Routes: `/meetings` (list: date, title, duration, state, process; click →
  `/meetings/$id`; a *Start meeting…* button), `/meetings/new` (start form),
  `/meetings/$id` (control center). Sidebar (`AppSidebar.tsx:69-85`) gains a
  `Meetings` link under Artifacts; the session list shows a small meeting
  glyph for sessions with `project === "meetings"`.
- Start form (53 §4.1): process picker (`meeting_processes`, active first,
  last choice by `bundle_id` remembered in `localStorage`), title, context
  textarea with the existing composer mic button (`transcribe` RPC via
  `putAttachment` — 45 §7.2's component, reused), consent checkbox (required;
  text from decision 14), model picker (existing model `Select` from
  `General.tsx`, pre-filtered to the catalog's fast tier if the catalog
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
- Settings tab `Meetings` (`SettingsDialog.tsx:42-58` `TABS` + `Pane`):
  hidden from capture (switch + the eye hotkey field), PTT hotkey field,
  `allow read-only tools without asking` (→ `meeting_set_allow_read_only`),
  a note that screenshots of hidden windows come out black (53 §2.3).

## 4. Hotkeys (main webview, `ui/src/lib/meeting-hotkeys.ts`)

`useMeetingHotkeys()` in `App.tsx` beside `useTauri()`: subscribes to the
meeting store's `state`; on enter `Recording|Paused` registers PTT and eye
with `register(shortcut, (e) => …)` (`e.state` is `'Pressed' | 'Released'`
in plugin-global-shortcut 2.x); on leave unregisters. PTT `Released` also
fires if the key was held across a state change (unregister → send
`meeting_ptt(false)` if held).

## 5. RPC additions (`crates/rpc/src/api.rs`, `crates/meeting`)

`meeting_set_allow_read_only(bool)` → actor `SetAllowReadOnly { value }`:
`write_config_value(&data_dir/config.yaml, "meeting", "allow_read_only",
value)` + `cfg.allow_read_only = value` (+ if a meeting records,
`SetApprovals` re-sent for its session). Nothing else new on the daemon.

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
9. Meetings tab: flip allow-read-only → `config.yaml` updated; rebind PTT
   to a conflicting chord → toast, old binding kept.
10. `meeting.enabled: false` → tray entry present but the form shows
    "meetings are disabled" from the RPC error; island never created
    visible.

## 9. Status

Not started.
