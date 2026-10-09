# 149 — COMPANION-NATIVE: the companion as One's promoted experience, with the meeting island and point-and-say native

*Status: design v0, 2026-10-09 01:10 (+01:00), written as the build contract for tonight's waves on both platforms.
Tree: worktree `~/Develop/theuth-149-int` = origin/main `dd19bf63` = v2.13.0. Every `path:line` below was read in that
worktree. Abbreviations: `COMP/` = `crates/desktop/companion/Sources/`, `WIN/` = `crates/desktop/companion-windows/src/`,
`UI/` = `crates/desktop/ui/src/`, `DESK/` = `crates/desktop/src/`, `api.rs` = `crates/rpc/src/api.rs`, `attention.rs` =
`crates/roster/src/attention.rs`. Recon facts are quoted with the recon's own cites (recon A/B/C read Sky's checkout at the
same sha; the api.rs lines recon C carried from design 130 were re-read here and corrected — `desk_open` is `api.rs:2083`,
`subscribe_events` `api.rs:2272`, `list_activity` `api.rs:246`). Inputs: Sky's words and rulings (knowledge
`decisions/theuth-design-149-companion-native-rulings-2026-10-09`), recon A (`/tmp/149/recon-A-notch.md`), recon B
(`recon-B-move.md`), recon C (`recon-C-gap.md`), the Windows builder's answer (§7, verbatim), designs 130 (+ Revisions
1–3, as-builts W0/W1/W1S/W2/W3/W6/PARITY), 96 (+ as-builts), 134 (+ as-builts W1–W6), 133, 132, 54, meetings/03 and
meetings/05, and the mock `docs/design/149/mock-round1.html` (seven screens, §3). Mock approval by Sky is pending: he
sleeps; the mock is round 1 and the frames F1–F5 are the target of record.*

## 0. Summary

Design 149 does two things to the companions and nothing to the engine's model of work. **(1) One-parity polish** — the
macOS Swift companion (130, after W6/W7/PARITY) and the Windows WinUI companion (134, after W6) are re-cut to the look of
One's five promo frames: a rail of separate pills (inbox with the gold count · one dot per agent · actions with the mic ·
more), an agents panel grouped by host with a per-row mic and ✕, the full card anatomy (header with position dots and
J/K/?/esc, a status line with the harness icon, the request and the agent's reply as context, the question in bold,
numbered options with "(Recommended)", a reply row with P/R/V/⏎, "Next steps" with Stop agent X and Clear E), a real
empty state and level bars while dictating. **(2) Two Tauri surfaces move into the companions as native windows** — the
meeting island with its briefs (today a `tauri-nspanel` webview, `DESK/island/macos.rs:35-57`, content
`UI/components/meeting/Island.tsx`) and design 96's Ctrl+S point-and-say (today a Carbon chord in the Tauri process,
`DESK/point/mod.rs:111-147`, overlays `DESK/point/mod.rs:573-631`, the voice panel in the island webview,
`UI/routes/island.tsx:19-37`). Everything privileged stays in the engine: audio capture, screenshots and accessibility
reads, the brief cache, dictation (recon B §B4, §B6) — the companions are views over RPCs that already exist, plus two
additive engine fields (§6).

Three parallel parts from one shared contract (Sky's ruling 2, §1.3): **W1 One-parity** (rail pills, agents panel, card
anatomy, empty state, Ask bar levels), **W2 point-and-say native**, **W3 island native**, after a **C contract** commit
(shared seams and stubs, the Tauri `companion-alive` gates on both shells, the per-platform voice-key default, the harness
icon set, the E6 skeleton) and a **W0 engine** wave (E6 `AttentionItem.context`; E7 mesh-wide activity OPTIONAL). The
Windows builder (session 01a11782) mirrors the three parts in WinUI from the same contract and lands the Windows part
itself (§7, §11). Each part lands on main only when feature-complete (Sky's 130 rule); the Mac parts land tonight,
engine + desktop rebuild owed in the morning. What a user sees when it lands: on the Mac, the rail becomes pills, the
agents pill opens the host-grouped list, cards look like F4/F5, the notch island is a native panel that can be dragged
anywhere, snaps back and remembers its place per display, and ⌃S draws the trail natively and asks theuth's desk; on
Windows the same, with a top-centre island, opaque pills, Ctrl+Alt+Space as the voice key and Direct2D overlays. The
Tauri island, overlays and voice-key binding stand down while a native companion is alive (both shells, one rule), and
keep working on Linux and on any desktop without a companion.

## 1. What Sky asked for

### 1.1 In his words
- 00:39: "OK the Mac companion app is decent, but it's not the full one experience you can see in the promotions here. It
  still needs a lot of work, you can see just based on the images. Also now that we have the companion app I think we need
  to move the meeting/notch window with the briefs and stuff to be part of it and native, and the ctrl+s draw say and ask
  should also be part of it too on macOS, so all of those interfaces are native. This is a good notch window reference but
  remember ours needs to be moveable and remember its position like the web tauri one does.
  https://github.com/saif0200/notchprompt"
- 00:44: "Once you come up with the plan brief the windows agent so it can build the same in parallel"
- 00:50: "Coordinate with the windows builder make sure it does the same for windows, it has access to a windows dev
  machine"
- 00:55: "Get this done and on main for me bud, thank you. Complete thorough implementation."

### 1.2 The five One frames (getone.one, the @ky__zo video) — the target look
- **F1 Inbox, empty.** A card titled "Inbox" with a round × top right; the body is a cloud glyph, the headline "Nothing
  needs you." and the muted sub-line "Suspiciously quiet. Questions and finished work land here." The rail beside it is
  SEPARATE pills: inbox · agent dots · actions (glyph, mic) · more.
- **F2 "See every agent. On your Mac and your VPS."** The agents panel grouped by host — a local group ("one") and a
  remote one ("api · on 🌐 remote vps"); a row is status dot/spinner · harness icon · name · ⑂children · mic · ×; the
  hovered dot grows.
- **F3 "Press to talk. One routes the message to the right agent."** A wide pill with the live dictated text; red level
  bars in the rail's mic.
- **F4 Finished card.** Header: agent name · position dots · ‹J K› · ? · × esc. Status "Finished just now · web". A
  message bubble. Reply row: camera S · field "Reply to …" with a `space` key cap · mic V · ↑⏎. "Next steps" with "Kill
  agent X" and "Discard E"; numbered options with →.
- **F5 Ask card.** "Waiting for your answer · web"; a blue user bubble and an agent bubble; the question in bold; options
  1/2/3 with "(Recommended)"; "Type your answer… space"; "Discard E".

### 1.3 Rulings and standing orders
1. **Run (ask_user 00:50): "Design now, build overnight unattended."** Recon → this design + mocks → waves while he
   sleeps; each of the three parts lands on main only when feature-complete (his 130 rule); engine + desktop rebuild owed
   in the morning.
2. **Order: "All three in parallel from a shared contract"** — One-parity polish · point-and-say native · island/briefs
   native.
3. **Per-agent mic in the agents panel: ALLOWED** — speaking into a row = `send_input` to that session. The Ask bar stays
   routed via theuth's desk (his 130 D7/R4: no agent picker, no ⌘1–9, no "New agent" row).
4. Standing orders: "correctness, polish and completeness … thorough work, is the priority" (2026-10-08); the Windows agent
   builds the same in parallel and has build-B (§7); nothing lands half-built.
5. Rulings this design inherits unchanged from 130 (recon C §1): Swift companion, cards native (D1); one engine
   connection, aggregation later (D2); gold is the only accent and means "needs you" (§4.1); E clears a finished receipt
   and never cancels or approves (§4.2); the hold is Settings › Undo delay 0–5 s, default 3 (D12); depth opens in the main
   window through `theuth://` links (D5); panels never activate (D15). What 149 amends: 130 D7/§4.3's "Point at it stays
   96's Ctrl+S in the Tauri process" (now native in the companion, §3.7) and 130 D4/R2's one-strip rail (now pills, §3.1).

## 2. Where we stand

### 2.1 The macOS companion after 130 W6/W7/PARITY
- **Package.** SwiftPM, `platforms: [.macOS(.v15)]`, three targets — `TheuthKit` (RPC + models), `CompanionCore` (pure,
  tested), `Companion` (AppKit/SwiftUI executable) — and `sindresorhus/KeyboardShortcuts` pinned `exact: "1.10.0"`
  (`crates/desktop/companion/Package.swift:9-41`). Sources: `COMP/TheuthKit/{Discovery,JSON,Models,RpcClient}.swift`;
  `COMP/CompanionCore/{AskRouter,AskRpc,AskVoice,Breakpoint,CardModel,Commit,KeyBindings,KeyGrammar,PanelModel,Policy,
  PolicySettings,RailModel,ReceiptModel,UndoQueue}.swift`; `COMP/Companion/{AppDelegate,MainApp,Store,RailPanel,
  AttentionPanel,PanelView,Palette}.swift` + `AskBar/`, `Voice/`, `Notifications/`, `Settings/`, `Breakpoints/`. Tests:
  `Tests/CompanionCoreTests/{AskTests,CompanionCoreTests,ParityTests,PolicyTests,ReceiptTests,W6Tests}.swift` and
  `Tests/TheuthKitTests/FixtureTests.swift` over ts-rs fixtures (92 tests after PARITY). Bundle: `scripts/bundle-companion.sh`
  copies the binary, dependency resource bundles, `tray.png`/`tray@2x.png` and the `.icns` into `Contents/Resources`
  (`:23-38`) and writes the Info.plist — `io.theuth.mac.companion`, `LSUIElement`, `LSMinimumSystemVersion 15.0`, only
  `NSMicrophoneUsageDescription` (`:46-62`); the entitlements file is an empty dict with a comment that the audio-input
  entitlement is "added when the Ask bar dictates" (`crates/desktop/companion.entitlements.plist:5-8`); `scripts/package-macos.sh:51-55`
  nests it at `Contents/Library/LoginItems/Theuth Companion.app` and signs inside-out.
- **Rail.** ONE never-ordered-out `NSPanel` — `[.borderless, .nonactivatingPanel]`, `.statusBar`,
  `canJoinAllSpaces | fullScreenAuxiliary | stationary`, `sharingType = .none` unless `--shareable`
  (`COMP/Companion/RailPanel.swift:26-36`); a global `mouseMoved`/`leftMouseDragged` monitor flips `ignoresMouseEvents`
  while the pointer is inside `frame.insetBy(-2)` (`:45-47`, `:92-104`); 26 pt wide at the right edge of
  `NSScreen.screens.first`, left via `Settings.railSide`, height from `hosting.fittingSize` (`:107-116`;
  `COMP/CompanionCore/RailModel.swift:63-64`); `frame(of:)` hard-codes the 16 pt mark pitch (`:118-124`); marks are
  16 × 16 in a `VStack(spacing: 6)` (`:140`, `:168-177`) under one `UnevenRoundedRectangle` with 10 pt inner radii and
  `surface.opacity(0.82)` (`:158-164`); wordmark + gold count (`:141-147`), needs-you marks · 12 × 1 hairline · agents ·
  `+N` · carnelian dot when disconnected (`:148-156`); the glyph table (gold ring at the kind's clock · faience disc with a
  0.38 s beat · hollow faience · malachite + check · carnelian + bar · dashed sand) at `:190-232`. Hover = a system tooltip
  only (`:173`); no grow, no highlight, no mic, no actions pill. Agents-zone rows come from `agents_list` desks and
  `list_activity` roots with `activity.live` (`RailModel.swift:99-144`), ≤ 12 then `+N` (`:87`); the label is
  `s.agent ?? s.model` (`:122`) — `AgentInfo.agent` is the external-harness key, not a name (`crates/agent/src/lib.rs:1220-1221`),
  so a Claude Code root reads "claude" (recon C C1.6). Presence: Expanded · Sliver when quiet · Always a sliver
  (`RailModel.swift:49-84`, 130 R2-3). Click: a needs-you mark focuses the panel beside the rail; an agent mark opens
  `theuth://c/<session>` in the main app — the 130 §4.1 peek card was never built (`COMP/Companion/AppDelegate.swift:183-191`).
- **Attention panel.** `KeyPanel` (key only when summoned), 372 × ≤ 560 (`COMP/Companion/AttentionPanel.swift:15-16`,
  `:36-48`), glass `NSGlassEffectView` on 26+ else `.hudWindow`, radius 12, 0.5 pt border (`:55-80`), placed on the screen
  under the mouse (`:170-178`). Fixed order header → focused card → other items as lines → footer → receipt: empty line
  "Nothing needs you." / "Connecting to the engine…" (`COMP/Companion/PanelView.swift:30-34`); header sentence "3 need you ·
  5 working · 2 done" + "1 of 3" with no buttons (`:45-62`; `COMP/CompanionCore/PanelModel.swift:52-63`); card header
  mark · who · model · task · "waiting 4 m" (`PanelView.swift:95-108`); options with key badges and "recommended · ⏎"
  (`:117-120`, `:162`); hint row S/E/O/D/⌘O/R (`:129-138`); footer "J K move · E clear · ⎋ …" (`:64-71`); receipt row with
  the 2 pt malachite countdown (`:223-258`). Order needs-you oldest → finished → receipts (`PanelModel.swift:19-22`);
  `priority`/`rank` of 132 are not decoded (`COMP/TheuthKit/Models.swift:45-62`). `Card.make` receives `agentModel` only
  (`COMP/CompanionCore/CardModel.swift:71-74`); E is refused unless `clearable` (`COMP/Companion/Store.swift:179-181`;
  `CardModel.swift:63-64`); the finished card's options are the `## Next` steps + Clear (`CardModel.swift:122-129`). The
  Store re-fetches `attention_list` + `agents_list` + `list_activity` together, debounced 250 ms, on events, never on a
  timer (`Store.swift:101-127`; `COMP/TheuthKit/RpcClient.swift:216-228`); `activity` is kept at `Store.swift:29`.
- **Ask bar.** 480 pt rounded rect (`COMP/Companion/AskBar/AskBarPanel.swift:12`, `GlassPanel.make` `:366-391`); field
  "Ask theuth…" 15 pt, finals in ink + the open partial in sand while live (`COMP/Companion/AskBar/AskBarView.swift:36-46`);
  mic glyph scaled by `level` (`:54-59`; `AskBarPanel.swift:300` = `levels.max()`); hint row "⏎ send to theuth · it routes ·
  hold ⌃⇧Space to talk" (`:62-70`). Voice: hold the hotkey → `VoiceRecorder` (`AVAudioEngine` → mono Int16 16 kHz,
  `COMP/Companion/Voice/VoiceRecorder.swift:8-35`) → `dictation_start(16000)` · `dictation_audio` every ~100 ms ·
  `dictation_stop` (`AskBarPanel.swift:165-213`; `COMP/CompanionCore/AskRpc.swift:27-48`; `api.rs:1486-1508`);
  `DictationLevel.levels` is already decoded (`COMP/Companion/AskBar/Store+Ask.swift:147-160`;
  `COMP/CompanionCore/AskVoice.swift:151-153`). Routing: `mesh_status` → hub → `desk_open("theuth")` → `send_input(desk,
  text)` (`Store+Ask.swift:79-97`; `AskRpc.swift:7-15`); `steer`/`sendInput` pass an empty `files` list
  (`AskRpc.swift:9`; `RpcClient.swift:261-262`). The menu's "Ask theuth…" path exists (`AskBarPanel.swift:334-337`).
- **Keys, legend, settings, menu.** One `KeyTable` with scopes anywhere · panel · askBar
  (`COMP/CompanionCore/KeyBindings.swift:167-190`): ⌃⇧Space open/hold-to-talk, 1 2 3 ⏎ J K, **S = snooze** (`:176`),
  **E = clear** (`:177`), D O ⌘O, **R = reply** (`:181`), ⌘L ⌘/ ⌘, ⎋; aliases ↓/↑/keypad ⏎/`?` (`:193-195`); the `.space`
  chord exists but is unbound (`:67`); `check` refuses duplicates in a scope (`:238-247`); the exclusive
  `RegisterEventHotKey` probe (`:286`); `Hotkeys.install` pushes global rows into KeyboardShortcuts
  (`COMP/Companion/Settings/Hotkeys.swift:6-89`); key-down and key-up both arrive (`AskBarPanel.swift:142-158`). Settings
  tabs Rail · Keys · Notifications · Engine (`COMP/Companion/Settings/SettingsWindow.swift:57-61`); values in the
  `io.theuth.mac.companion` defaults suite (`COMP/Companion/Settings/SettingsStore.swift:8-49`). Status-item menu: Open
  theuth · Keyboard shortcuts… ⌘/ · Quit theuth · Quit theuth Companion · Ask theuth… · Pause notifications ▸ · Launch at
  login · Settings… (`AppDelegate.swift:126-136`). Palette pairs gold `c47a2c` · faience `5b8f8a` · malachite `4a8a6e` ·
  carnelian `c45a48` · sand `9a8b72` · surface `1c1915` · border `3a3228` with "gold ONLY for needs-you"
  (`COMP/Companion/Palette.swift:5-20`); switches `--probe N`, `--shareable`, `--probe-settings <tab>` (`:69-77`).
- **Shell hand-over.** `DESK/companion/mod.rs:23-41` (`companion_alive` command), `DESK/companion/macos.rs:18-20`
  (nested path), `:79-83` (`running()` by bundle id — LSUIElement apps post no launch notifications, so the shell polls),
  `:123-139` (`set_alive` → `companion-alive` to every webview + tray hide), `DESK/lib.rs:524-551` (`theuth://attention/<id>`,
  `theuth://c/<session>`, `theuth://new`, `theuth://quit`); the UI listens at `UI/lib/companion.ts:10-27` and suppresses
  `notifyIfHidden` + the tray `●N` while alive (`UI/App.tsx:448-485`).

### 2.2 The Windows companion after 134 W6 (from the builder's answer, §7.1, cites re-read here)
- Landed on main by 134 W6 at `51c7d404`; baseline at `dd19bf63`: Core 146/146, Rpc 35/35 on the Mac; Debug build 0
  errors / 0 warnings in 42 s on build-B; build-B runs packaged 2.12.3+7fd8068f. Solution `crates/desktop/companion-windows/
  Theuth.Companion.slnx`: `WIN/Theuth.Companion` (WinUI 3 app), `WIN/Theuth.Companion.Core` (portable models, policy, key
  table; net10.0, tested on the Mac), `WIN/Theuth.Companion.Rpc` (engine.json discovery, JSON-RPC over WS); scripts
  `build.ps1`, `test.sh`, `probe.ps1`, `seed.ps1`, `w2/w3/w5/w6-gate.ps1`.
- Surfaces it has: tray + menu (`WIN/Theuth.Companion/Tray.cs`, `Windows/TrayMenu.cs`), the rail as ONE edge-docked tool
  window (`Windows/RailWindow.cs:21-26, 57` — `WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT`, focus hand-back on
  `WM_ACTIVATE`), the attention panel with cards (`Windows/AttentionPanelWindow.cs`), peek card and legend
  (`Windows/PeekCardWindow.cs`, `Windows/LegendWindow.cs`), the Ask bar with hold-to-talk (`AskBar/AskBarWindow.cs`,
  `ReplyField.cs`, `DictationCalls.cs:12` → `DictationStartAsync(AskAudio.SampleRate)` with
  `WIN/Theuth.Companion.Core/AskAudio.cs:13` = `16_000`, never null; recorder NAudio WASAPI `Voice/VoiceRecorder.cs:71`),
  toasts (`Notifications/`), Settings with the key recorder (`Settings/`). Every tool window: `IsShownInSwitchers =
  false`, `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, `WDA_EXCLUDEFROMCAPTURE` unless `--shareable`
  (`WIN/Theuth.Companion/Windows/ToolWindow.cs:15-18, 43-53`). Hotkeys: `RegisterHotKey` + `MOD_NOREPEAT`; the release is
  seen by polling `GetAsyncKeyState` every 50 ms (`WIN/Theuth.Companion/Hotkeys.cs:10-14, 102-106`) — hold and
  double-tap both work. Shell: `DESK/companion/windows.rs:58-74, 166` (alive via a named mutex, tray hand-over);
  installer step `[3b/6]` (`scripts/package-windows.ps1:18-23, 183`).
- Nothing of 149 exists on Windows yet: pills, agents panel, card additions, empty state, the wide Ask pill, island,
  point-and-say ("Next steps", "Suspiciously quiet" and "Stop agent" appear in neither companion today).

### 2.3 The Tauri island + briefs (what moves)
- **Window.** Declared in `crates/desktop/tauri.conf.json:27-40` (label `island`, hidden, undecorated, transparent,
  always-on-top, 440 × 44). macOS swizzles it with `tauri-nspanel`: style `NonactivatingPanel | Resizable`
  (`DESK/island/macos.rs:25-27`), level 25 = `NSStatusWindowLevel` (`:28-29`), `CanJoinAllSpaces | FullScreenAuxiliary |
  Stationary`, `hides_on_deactivate(false)`, `becomes_key_only_if_needed(true)` (`:35-49`); sizes 440/300–1200 × 44–800
  (`:17-24`). It is **never ordered out**: WebKit throttles an off-screen webview, so "hidden" = alpha 0 + mouse-transparent
  (`:51-54`, `conceal` `:59-62`). Commands: `set_hidden_from_capture` = `set_content_protected` on EVERY webview window +
  `capture-hidden` (`:64-71`, `:157-164`); `island_show(w,h)` clamps, sets alpha 1, `order_front_regardless` (`:75-90`);
  `island_resign_key` (`:92-100`); `island_hide` (`:103-107`); `island_place(positions) → (x, y, key)` with key = the
  sorted list of every screen frame (`:123-125`, `rect_key` `:166-171`), default = the first screen with
  `safeAreaInsets().top > 0` centred at the screen top, else under the primary menu bar (`:126-145`); `screens-changed`
  from `NSApplicationDidChangeScreenParametersNotification` (`:173-193`). Registered at `DESK/lib.rs:17`, `:278-280`
  (nspanel plugin, macOS only), `:299-304` (six commands), `:342` (`island::init`); window-state denylist `:265-270`.
  Windows/Linux share `DESK/island/mod.rs` (capabilities `:22-60`: macOS `notch`, Windows `top-centre`, Linux `user` with no
  capture hiding; constants `:80-91`; `THEUTH_ISLAND_PROBE` `:98-131`; placement by monitor key with
  `island-monitors.json` `:173-199`); `DESK/island/windows.rs:1-19, 36-52` (`WS_EX_TOOLWINDOW | NOACTIVATE | TOPMOST`,
  `SW_SHOWNOACTIVATE`, `WDA_EXCLUDEFROMCAPTURE` via `set_content_protected`); `DESK/island/linux.rs:15-24`.
- **Content.** `/island` renders `<VoicePanel/>` while the voice key is live, else `<Island/>`; `pointRefuse('meeting')`
  while a meeting is live (`UI/routes/island.tsx:9-37`). `UI/components/meeting/Island.tsx`: `MOVE_SAVE_MS 300`, `STRIP
  44`, `WIDTH 440` (`:32-35`); degradation badges (`:37-47`); `shown = live || Starting || Stopping` (`:70`); content-fit
  size via `ResizeObserver`, `sizes[key]` wins after an edge drag (`:73-92`); `islandShow/Hide/Place` (`:104-123`);
  `onMoved`/`onResized` → `setPosition`/`setSize` under the key, re-place on `screens-changed` (`:126-154`); the
  `meeting_cursor` mirror (`:165-168`); the drag strip is `data-tauri-drag-region` (`:182`); ⋮ pause/resume/stop
  (`:187-199`); topic buttons tap = `meeting_tap`, × = `meeting_dismiss`, Zap for Oracle-published, dot for cached
  (`:202-233`); the eye (`:247-266`); brief pane title + ≤ 3 bullets + deeper + newer/older + `↑N new` (`:276-321`);
  `OracleChip` with `decision_mode('meeting')` (`:328-370`); `RequestBox` → `meeting_request` (`:373-398`); `Meters` on the
  `Level` event at 10 Hz (`:435-454`); `Clock` `rec mm:ss` (`:456-470`); the transcript dropdown
  (`UI/components/meeting/IslandTranscript.tsx:1-12`). Store: `reduce()` over `Started, State, TopicAdded, TopicRemoved,
  Brief, Utterance, Listening, ApprovalWaiting, PostProgress, ReviewReady, Deleted, OracleDeciding, OracleDecision`
  (`UI/store/meeting.ts:156-220`), the brief-stack rule (`:181-190`), `hydrateMeeting` = `meeting_status` +
  `meeting_briefs` + `meeting_live` (`:257-265`), listeners incl. `TurnStarted/ToolCall*/TurnFinished` (`:268-297`), all on
  the ordinary event stream (`:19`). Settings in **webview localStorage** key `meeting-settings`: `hidden` (eye, default
  true), `ptt` = ``Alt+` ``, `eye` = `Alt+Shift+E`, `positions`, `sizes`, `model`, `split`
  (`UI/lib/meeting-settings.ts:9-28`, `:43`, `:45-67`); PTT/eye registered by the main webview only while live
  (`UI/lib/meeting-hotkeys.ts:16-33`, mounted `UI/App.tsx:678`). Shell glue `UI/lib/tauri.ts:80-104`.
- **RPC/event contract** (positional; TS mirror `UI/rpc/api.ts:680-723`): `meeting_processes` (`api.rs:1548`),
  `meeting_start/pause/resume/stop` (`:1556-1567`), `meeting_status` (`:1569`), `meeting_permissions` (`:1581`),
  `meeting_tap/dismiss/deeper/request/ptt/cursor/briefs/topics/live` (`:1585-1611`); the brief cache
  `brief_search/list/tags/get/put/delete` (`:1896-1919`) is engine state the island never reads directly (it gets `Brief`
  events). The briefs *library* (`UI/components/briefs/*`, route `/meetings/briefs`) and the Meetings tab are main-window
  surfaces and stay in Tauri on every OS (recon B §B1.1).
- **Observed gaps the native island must close** (recon A §A2): position keyed by a screen-topology string, not a display
  id (twin monitors collide, a resolution change forgets); a restored point is applied without validation
  (`macos.rs:126-127`, `:146-148`); no snap-back after a drag; no hover; hidden = alpha 0 (a transparent window object
  stays in the window list).

### 2.4 Point-and-say as built by 96
- **Shell.** `voice_key_bind(chord)` parses with the global-shortcut plugin, maps `Pressed/Released` into the driver's
  channel, unregisters the previous chord, `""` = unregister (`DESK/point/mod.rs:111-147`, `:119-120`, `:128-138`). The pure
  machine `DESK/point/machine.rs:6-18` (`HOLD_MS 200`, `DOUBLE_TAP_MS 300`, `MIN_NOTE_MS 800`, `SAMPLE_HZ 60`, `DONE_MS
  6000`, `HINT "Hold to talk · double-tap to lock"`) and `step(s, e, now, refused) → (State, Vec<Effect>)` (`:23-69`,
  17 tests). Driver effects (`:240-345`): `Start` reveals the overlays, emits `point:start`, registers Esc (`:364-387`),
  starts the 60 Hz sampler over `cursor_position` (`:487-536`); `Stop` stops it, **conceals the overlays and awaits that
  before** `point:stop` so the capture never sees the trail (`:307-322`); Esc only between press and send (`:38-42`).
  Overlays: one `WebviewWindowBuilder` window per monitor, `overlay-<n>`, built lazily and on `screens-changed`
  (`:82-107`, `:573-631`, `:635-645`); on macOS an `NSPanel` at `NSPopUpMenuWindowLevel` 101 with the island's collection
  flags + `IgnoresCycle` (`DESK/point/macos.rs:13-55`); `left_button_down()` = `NSEvent.pressedMouseButtons & 1`, no TCC
  (`:57-60`); Windows/Linux = show/hide only (`DESK/point/other.rs:1-26`).
- **UI.** `useVoiceKeyBinding` binds `'voice-key'` whenever `isDesktop()` (`UI/App.tsx:384-398`) — the default is
  `Control+S`, scope `global` (`UI/lib/shortcuts.ts:52`) and `enabled: true` (`UI/lib/voice-key.ts:47`), so **Windows and
  Linux take Ctrl+S from every app today** (the builder's finding, §7.3 item 2); `point_configure(annotate)` (`App.tsx:413-415`);
  `voiceKey.install()` in the island webview only (`App.tsx:604`). `UI/lib/voice-key.ts`: `install()` wires
  `onPointStart/Lock/Stop/Cancel/Refused` (`:14-21`); settings `{enabled, includeScreen, accessibility, annotate,
  hintsShown}` in localStorage (`:58`); phases `idle → listening → sending → working → speaking → done | error | hint`
  (`:100-144`); `DONE_MS 2000`, `HINT_MS 2000`, `ERROR_MS 4000`, `SPEAK_WAIT_MS 1500` (`:227-238`); on `point:start`
  **`dictation_start(null)` = the ENGINE's microphone** (`:295-318`; `AskRpc.swift:34` documents the difference); on
  `point:stop` → `dictation_stop` + `screen_context({samples, color, since_ms, accessibility})` (`:329-343`; the colour from
  `UI/lib/trail.ts:1-21`, `--status-success-fg`); `composeBody` = `[voice note, N s] <text>` + one `[screen …]` line per
  display, `desk_open('theuth')` cached per engine, `send_input(desk, body, files, null, clientId)` or `steer` when the
  desk is mid-turn, `speech.setVoiceMode(desk, true)` (`:213-222`, `:344-366`); `point_done` after `DONE_MS` (`:369-376`);
  the reply is spoken from the island webview (`:389-396`). The overlay route is one `<canvas>`, 6 px round-capped stroke,
  alpha 0.85, 14 px ring (`UI/routes/overlay.tsx:1-98`). The `[screen …]` line renders as a card in the desk's transcript
  (`UI/lib/screen-context.ts:1-30`; `UI/components/chat/ScreenContextCard.tsx:1-30`;
  `UI/components/chat/MessageUser.tsx:137-188`) — a main-window surface that stays.
- **Engine.** `screen_context(req) → {lines, files}` captures the displays the path visited, draws the path, reads the
  accessibility tree, writes PNGs under `<data_dir>/attachments/` (`api.rs:330-339`; `crates/computer/src/lib.rs:142-162`;
  `context.rs:52-143`; `pointer.rs:90-104`; `screen.rs:52-71` `CGPreflightScreenCaptureAccess`, `:155-168` SCK,
  `:215-242` `screencapture` fallback; `ax.rs:101` `AXIsProcessTrusted`). `dictation_start` is refused with `"talking"`
  while the `Utterer` is leased by a meeting and `"already dictating"` while another dictation runs
  (`crates/voice/src/dictation.rs:221-226`).

### 2.5 What is engine-owned and stays where it is
Audio capture (`crates/meeting/src/capture/system.rs:1-101` process tap, `mic.rs:1-19` VoiceProcessingIO, the blocking
TCC prompt at `capture/mod.rs:94`); `meeting_permissions` (`crates/meeting/src/lib.rs:741-764`); topics and briefs from the
meeting session's tools; the brief cache (`api.rs:1896-1919`); screenshots, AX reads, the pointer grammar and the stamped
images (`crates/computer/src/*`); the dictation stream (`api.rs:1486-1508`); desks and routing (`desk_open`,
`api.rs:2083`); `send_input` with `files: Vec<PathBuf>` (`api.rs:194-203`) and `steer` with `files` (`:218-226`);
`cancel(session, scope: root | tree)` (`:255-256`; `crates/agent/src/lib.rs:340-346`; 133 §4); `list_activity`
(`:246-247`), `list_sessions` with `running` and `owner` (`:485-486`; `crates/agent/src/lib.rs:2288-2295`),
`sessions_status` (`:549-550`), `mesh_status` (`:1963`), `agents_list` (`:2077`), `agent_pause` (`:2089`),
`attention_list/snooze/dismiss` (`:2138-2145`), `subscribe_events` (`:2272-2277`). Nothing of this moves; the companions
call it.

### 2.6 The gap table vs One (recon C §9, condensed; S/M/L = under a day / 1–3 days / a wave)
| # | Delta | Frames | Effort | Engine dep |
|---|---|---|---|---|
| 1 | Card header as controls: name · position dots · ‹J K› · ? · × esc (intents exist, `COMP/CompanionCore/KeyGrammar.swift:20-39`) | F4 F5 | S | none |
| 2 | Type scale body 15 / question 17 / headline 24; panel 372 → ≈ 420 (`AttentionPanel.swift:15`) | all | S | none |
| 3 | Empty state: glyph + 24 pt headline + sub-line + "Inbox" title row (`PanelView.swift:30-34`) | F1 | S | none |
| 4 | "(Recommended)" wording, "→" on next-step rows, "Next steps" header (`PanelView.swift:162`; `CardModel.swift:122-129`) | F4 F5 | S | none |
| 5 | Status line: harness icon + dot + "Finished just now · ‹project›" — join `AgentInfo.agent/project` by `item.session` (`CardModel.swift:71-74`) | F4 F5 | S | exists |
| 6 | Rail re-cut into separate pills (`RailPanel.swift:140-164`; `frame(of:)` `:118-124` → per-mark anchors) | F1–F3 | M | none |
| 7 | One dot per agent in the agents pill + hover-grow (`RailPanel.swift:168-177`) | F2 | S | none |
| 8 | Red level bars from `levels` in the Ask bar, mirrored in the rail's actions pill (`AskBarPanel.swift:300`) | F3 | S | exists |
| 9 | Actions pill = glyph + mic → the Ask bar; "more" pill → the status-item menu (`AppDelegate.swift:126-136`) | F1 F3 | S–M | none |
| 10 | Agents panel (local host): status · harness · name · ⑂N from `list_activity` ∪ `agents_list`, peek content (`AgentInfo.last`) | F2 | M | exists |
| 11 | Per-row mic: hold → dictate → `send_input(session, text)` (ruling 3; `AskBarPanel.swift:165-213` reused) | F2 | M | exists |
| 12 | Per-row ✕: `cancel(session, tree)` behind the hold; desks `agent_pause(name, true)`; add `cancel` to `RpcClient` | F2 | S–M | exists |
| 13 | Harness glyph set (theuth · omp · opencode · claude · codex · unknown); data `AgentInfo.agent` / `SessionSummary.agent` | F2 F4 F5 | S–M | exists |
| 14 | Reply row: field always visible, `space` → reply, V = voice reply, ↑⏎ send; resolve S (snooze, `KeyBindings.swift:176`) | F4 F5 | M | exists |
| 15 | Conversation bubbles — interim `SessionSummary.title` + `AgentInfo.last`; full = **E6** `AttentionItem.context` | F4 F5 | S / M | **E6 new** |
| 16 | Radius 12 → 24 on cards, capsule Ask bar — parameterise `GlassPanel.make` (`AskBarPanel.swift:366-391`) | all | S | none |
| 17 | Dark-glass-first on the three panels (`Palette.swift` pairs resolve dark) | all | S | none |
| 18 | Motion: 120–160 ms fade + rise on open; a spinner arc for working rows in the agents panel | F2 F4 | S | none |
| 19 | Order by `rank` → `waiting_since` → newest; decode `priority`/`rank` (`attention.rs:122-130`; `Models.swift:45-62`) | F4 F5 | S | exists |
| 20 | Show `Packet.if_wrong` above the options; review `Packet.options` after Approve (132 D2/D3; `RpcClient.swift:247`) | F5 | S–M | exists |
| 21 | Fix the agents-zone label (`RailModel.swift:122`): name from `SessionSummary.title`/`AgentInfo.name`, harness as icon | F2 | S | exists |
| 22 | Needs-you dot per agent row: `awaiting`/`approval` gold; join `attention_list` by `session` (`RailModel.swift:120`) | F2 | S | exists |
| 23 | Host grouping by `SessionSummary.owner` → `mesh_status` names; remote liveness honest (hollow dot) | F2 | S | **E7 L** for live remote state |
| 24 | "Stop agent" pill on cards whose agent runs or waits = `cancel(session, tree)`; none on finished cards (E3) | F4 | S | exists |
| 25 | P (camera) in the reply row + Ask bar: `screen_context` → `send_input`/`steer` with `files` (`RpcClient.swift:262` passes `[]`) | F3–F5 | M | exists |
| 26 | E stays clear-only by ruling; ask cards show S snooze where One shows Discard (`Store.swift:179-181`) | F5 | S | none |
| 27 | Ask-bar status names the hub ("theuth's desk on ‹self_name›") from the cached `mesh_status` (`Store+Ask.swift:79-97`) | F3 | S | exists |
| 28 | VoiceOver: position dots announce "card 2 of 3"; agents rows "‹name›, ‹state›, ‹harness›, N children" (`RailPanel.swift:174-176`) | F2 F4 | S | none |
| 29 | Native island (recon A §A5 stack) replacing `DESK/island/macos.rs` + `Island.tsx` on macOS | — | L | none |
| 30 | Native point-and-say replacing `DESK/point/*` + `voice-key.ts` on macOS | — | L | none |

## 3. The target experience

The mock is `docs/design/149/mock-round1.html` (seven screens, written 00:58–01:05 from the five frames; its numbering is
used below as "screen N"). Everything here is dark-first glass on the three floating surfaces (§3.9); the Settings and
legend windows keep the system appearance.

### 3.1 The rail — separate pills (F1–F3; screens 1, 2, 5)
- **Shape.** Four pills in one vertical stack at the right edge of the main display (left via Settings › Rail › Side), each
  a 52 pt capsule with 12 pt gaps, vertically centred as a group: **inbox** (the wordmark with the gold count beneath;
  gold only when needs-you > 0, like today's `RailPanel.swift:141-147`) · **agents** (one dot per agent, ≤ 12 then `+N`,
  the same sources as `RailModel.swift:99-144`) · **actions** (the Ask glyph and the mic; red level bars while a dictation
  records) · **more** (`···`). The hairline disappears — the gap is the separator; 130 R2's two zones survive as the first
  two pills. Hover on an agent dot grows it (`scaleEffect(1.4)`, 120 ms spring) and highlights its row in the agents panel
  when that is open (recon C C1.2/C2.8). The mouse monitor's inset test (`RailPanel.swift:92-104`) stays valid because one
  panel frame still bounds every pill; `frame(of:)` (`:118-124`) becomes a per-mark anchor read back from SwiftUI
  (`onPreferenceChange`), so the Attention panel still opens beside the right mark.
- **Dots.** gold dot = needs you (an item in `attention_list` joined by `session`, or `AgentState` awaiting/approval);
  spinner = working (`activity.state = running`; a 0.38 s arc instead of the opacity beat — the rail keeps the beat under
  Reduce Motion); green = idle/done (malachite; see §12 item 4 for the colour collapse this implies); carnelian = error;
  dashed sand = lapsed (desk paused). Shape distinguishes every state with colour removed (130 §4.1).
- **Behaviour.** Click inbox → the Attention panel beside the rail (`store.focus` on the oldest needs-you item, or the
  empty state); click an agent dot → the agents panel scrolled to that row (no deep link any more — the panel is the peek
  130 §4.1 asked for); click actions → the Ask bar (`AskBarPanel.swift:334-337`'s path); click more → the status-item menu
  (`AppDelegate.swift:126-136`) as a popup at the pill. Presence modes unchanged (`RailModel.swift:49-84`); the sliver is
  the whole stack collapsed to a 4 × 120 capsule.
- **Data.** `attention_list` (`api.rs:2138`), `agents_list` (`:2077`), `list_activity` (`:246`), `AttentionChanged`
  (`attention.rs:138`) and the agent events the Store already re-derives on (`Store.swift:101-127`); the mic bars read
  `AskBarState.levels` — the rail observes the Ask-bar state, not only `store.$rail`.
- **Empty/error.** Inbox pill with no count and no gold; agents pill shows nothing (no agents) — the pill itself stays so the
  rail keeps its shape; a carnelian 4 pt dot on the inbox pill when `store.connection != .open` (today `RailPanel.swift:148-156`).
- **Differs from One.** Gold, not green, marks "needs you" (ruling); the count rides on the inbox pill; the agents pill shows
  agent dots in theuth's state colours rather than One's green/blue; no "New agent" affordance anywhere (R4).

### 3.2 The agents panel grouped by host (F2; screen 2)
- **Shape.** A second `KeyPanel` using the Attention panel's glass (`AttentionPanel.swift:55-80`, radius 24), opened from
  the agents pill or ⌃⇧A (§8), header "Agents" · J K · ✕ esc. Groups by host: **"this Mac"** = rows whose
  `SessionSummary.owner` is `""` or `mesh_status.self_id`; then one group per other engine named from `mesh_status.peers`
  (`crates/mesh/src/types.rs:232-248`, `:94-108`), titled "‹name› · on 🌐 ‹mesh›". Rows: dot/spinner · harness icon
  (`AgentInfo.agent`, `crates/agent/src/lib.rs:1220-1221`, mapped through the shared icon set, §4 D18) · name
  (`AgentInfo.name` for children, `SessionSummary.title` for roots, `crates/agent/src/lib.rs:1214-1215`, `:2240-2241`) ·
  `⑂N` from `SessionActivity.children` (`:1272-1273`; decoded at `COMP/TheuthKit/Models.swift:179-189`) · a muted
  "what it is doing" line from `AgentInfo.last` (`:1230-1231`) or "running 4 m" from `turn_started_at` · mic · ✕. Desks
  from `agents_list` are rows too ("theuth · desk", the needs-you dot when their attention item exists). Roots first;
  live children indented one level or folded into `⑂N` when the row is collapsed (click the count to expand).
- **Per-row mic (ruling 3).** Hold the row's mic (or V with the row focused) → the same `VoiceRecorder` + dictation stream
  as the Ask bar (`AskBarPanel.swift:165-213`) with the row's session as the target → release → the text shows in a reply
  field under the row for the 3 s hold (⎋ keeps editing) → `send_input(session, text, [], null, clientId)`
  (`api.rs:194-203`; the engine's default `delivery` is `steer`, so a busy session hears it after its tool batch) → a
  receipt line under the row ("sent · ⎋ undo" during the hold, then "sent"). The Ask bar is untouched and still routes via
  theuth's desk.
- **Per-row ✕ (stop).** Press → the 3 s hold with the pill "Stopping ‹name› and N children · ⎋ undo" (N = `children`);
  commit → `cancel(session, "tree")` (`api.rs:255-256`; 133 §4). A cancel is final, so the hold IS the confirmation and
  the receipt never says "undo" after commit (recon C C2.7). Desk rows: `agent_pause(name, true)` (`api.rs:2089`) with the
  pill "Pausing ‹desk›". Rows whose session is not running show ✕ disabled with the tooltip "nothing is running".
- **Remote liveness, honestly.** `list_sessions` stamps `running` from THIS engine's `self.running` only
  (`crates/agent/src/supervisor.rs:6547-6557`) and `list_activity` is this engine's supervisor only, so a row owned
  elsewhere has no live state here: it is drawn with a hollow sand dot and the suffix "on ‹node›", its mic and ✕ disabled
  with the tooltip "open it on ‹node›", until E7 (§6) exists; `sessions_status` (`api.rs:549-550`) gives the counts owned
  here/elsewhere for the group header. The panel never fakes a remote spinner (recon C do-not-copy 10).
- **Keys.** J/K move, ⏎ opens the session in theuth (`theuth://c/<session>`), V hold-to-talk into the row, X stop (hold),
  ⌘O open, ⎋ close; the hovered dot grows in the rail too.
- **Data.** `list_activity` + `list_sessions` (`api.rs:485-486`, added to the Store's fetch set) + `sessions_status` +
  `mesh_status`; refreshed on the same 250 ms debounce as today (`Store.swift:101-127`).
- **Empty/error.** "No agents running." + "Desks appear here when they wake; `Ask theuth` starts one." When the engine is
  unreachable the panel shows the last list greyed with "Connecting to the engine…".
- **Differs from One.** One's green row highlight → faience; theuth shows desks and children counts from `list_activity`,
  never from terminal scraping (do-not-copy 4); the remote group is truthful, not live.

### 3.3 Card anatomy (F4 finished · F5 ask; screens 3, 4)
- **Header row.** ‹agent name› (bold) · position dots (● ● ▬, one per item, the focused one long; VoiceOver "card 2 of 3")
  · ‹J · K› buttons · ? (legend) · ✕ esc. All buttons act without activating the companion (`Intent.next/.previous/
  .legend/.escape`, `KeyGrammar.swift:20-39`; `Store.perform`, `Store.swift:161-193`). The glance sentence "3 need you ·
  5 working · 2 done" moves to a muted sub-line under the header (130 D13 keeps its four glance questions; §12 item 7).
- **Status line.** harness icon · state dot · "Finished just now · ‹project›" / "Waiting for your answer · 2 min ·
  ‹project›"; "just now" < 45 s (`RailModel.ago`, `RailModel.swift:169-175`); project from `AgentInfo.project`
  (`crates/agent/src/lib.rs:1232-1234`) or the last path component of `SessionSummary.cwd`, else the harness key as text
  (what screen 3 shows). The join is client-side in `Card.make`, which gains the item's `AgentInfo` (`CardModel.swift:71-74`).
- **Context bubbles (E6).** The user's request as a right-aligned faience bubble and the agent's last message as a dark
  bubble, both ≤ 160 chars, from `AttentionItem.context` (§6 E6); until E6 lands the interim is `SessionSummary.title` as
  the request and `AgentInfo.last` as the reply (recon C C3.3). One's blue → faience; never gold.
- **Question** 17 pt semibold ≤ 3 lines; **why** 15 pt (`Packet.summary`/`detail`); `Packet.if_wrong` as a one-line
  "If wrong: …" above the options (132 D2; `COMP/TheuthKit/Models.swift:365-379` gains the field); chips from `Packet.checks`.
- **Options.** "1 ‹label› (Recommended) →", "2 …", "3 …"; ⏎ = the recommended one; > 3 → "+N more — O opens the full list
  in theuth" as today (`PanelView.swift:117-120`). Review cards offer `Packet.options` as the moves after Approve and send
  `task_move(task, done, note, next)` (132 D3; `RpcClient.swift:247` gains the fourth arg).
- **Reply row.** [▣ P] [field "Reply to ‹name›" / "Type your answer…" with an `R` cap] [🎙 V] [↑ ⏎]: the field is always
  visible; `space` is an alias of R and focuses it (`.space` is defined and unbound, `KeyBindings.swift:67`); V =
  hold-to-talk into the field with the card's session (the Ask bar's machine, `AskBarPanel.swift:165-213`); ⏎ sends after
  the 3 s hold (`send_input`/`answer_ask` as today per kind); P = point: closes the panel, runs one `screen_context` with a
  one-point path at the pointer (`api.rs:335-339`) and attaches the returned `files` to the reply (`send_input(…, files)`),
  the `[screen …]` line appended to the text — the camera verb One puts on S (S stays snooze, 130 D6).
- **Footer.** "Next steps" header over the finished card's options (`CardModel.swift:122-129`), right-aligned pills
  **Stop agent X** (carnelian text; only on cards whose agent is running or waiting — ask, approval, blocked, decision;
  `cancel(session, "tree")` behind the hold) and **Clear E** (`attention_dismiss`, `api.rs:2144`; only when `clearable`).
  A Finished item is a receipt of a clean `end_turn` (`attention.rs:70-72`), so its footer shows Clear E and no Stop
  (§12 item 2 names the mock's deviation). Ask cards show "S snooze" where One shows Discard (ruling: E never answers).
- **Ordering.** `rank` desc → `waiting_since` asc → newest (`attention.rs:122-130`, 132 D4), Urgent group first; the
  Store decodes `priority`/`rank`.
- **Keys (panel scope, §8).** 1 2 3 ⏎ · J K ↓ ↑ · space/R reply · V talk · P point · X stop (hold) · E clear · S snooze ·
  D O ⌘O · ⌘L · ? ⌘/ · ⌘, · ⎋.
- **Empty/error.** A card whose session has vanished shows "session gone" in the status line and disables V/P/X; a failed
  `send_input` shows the receipt "Failed · ‹error›" as today (`PanelView.swift:223-258`).

### 3.4 Empty state (F1; screen 1)
Title row "Inbox" · ✕ esc; body: the wordmark glyph, "Nothing needs you." at 24 pt, the sub-line "Questions and finished
work land here; the menu bar count will tell you." (ours — One's "Suspiciously quiet." is a joke that reads wrong at 3 am
when an agent has died silently; the second sentence is true of theuth's menu bar `●N`). Below it the Done zone (last 24 h
of asks/answers, ≤ 4 rows, `COMP/Companion/AskBar/AskPanelViews.swift`) when it has entries. "Connecting to the engine…"
replaces the headline while `store.connection != .open` (`PanelView.swift:30-34`).

### 3.5 Ask bar + level bars (F3; screen 5)
A 28 pt-radius capsule (`GlassPanel.make(radius:)` parameterised, `AskBarPanel.swift:366-391`), 15 pt field, finals in ink
and the open partial in sand (`AskBarView.swift:36-46`). While recording, N red (carnelian) bars drawn from
`DictationLevel.levels` (one bar per band, newest right, 20 ms per frame as 54 §8's meter) replace the mic glyph and are
mirrored into the rail's actions pill (`Store+Ask.swift:147-160`; `AskVoice.swift:151-153`). Hint row: "hold ⌃⇧Space to
talk · release → sends to theuth in 3 s · ⎋ keeps editing · theuth routes it"; the status names the hub ("theuth's desk on
‹self_name›", `Store+Ask.swift:79-97`). P in the bar attaches the screen exactly as in §3.3. Routing, receipts and the
refusal off the hub are unchanged (130 D7/R4, PARITY). Error: "dictating elsewhere" when `dictation_start` answers `already
dictating`; "Meeting running — use the island to ask" when it answers `talking` (`crates/voice/src/dictation.rs:221-226`).

### 3.6 The native island (screen 6)
- **States.** *closed*: a black notch-hugging pill — the real notch's width from `auxiliaryTopRightArea.minX −
  auxiliaryTopLeftArea.maxX` and height `safeAreaInsets.top` (recon A §A3/§A5; on a screen without a notch a 300 ×
  menu-bar-height pill centred under the menu bar) showing meters · "Standup · Speaker 2" · `rec 12:31` · the topic count;
  *open* (hover 0.3 s or click; closes 100 ms after the pointer leaves unless pinned by a click): the strip (meters, clock,
  oracle chip, topic buttons, degradation badges, "approval waiting"/"listening…", eye, ⋮) + the brief pane (title, ≤ 3
  bullets, deeper D, newer/older, `↑N new`) + the collapsed transcript row + the request box — content parity with
  `Island.tsx:181-398`; *detached*: dragged away from the notch by its grip, same content, position remembered.
- **Drag, snap-back, memory.** A grip region (the left 24 pt of the strip and the whole strip when nothing is under the
  pointer) calls `window.performDrag(with:)` from `mouseDown`; on mouse-up a frame whose centre is within 40 pt of the
  notch default animates back and clears the saved frame; double-click the grip = snap back. Frames are saved in
  `CompanionSettings.islandFrames[displayUUID]` with `displayUUID = CGDisplayCreateUUIDFromDisplayID(NSScreenNumber)` in
  that screen's own coordinates, validated against `NSScreen.screens` (must intersect a `visibleFrame`) before use, else the
  default; re-placed on `didChangeScreenParametersNotification` and `NSWindow.didChangeScreenNotification`. This keeps
  "remembers its position like the web tauri one does" and fixes the Tauri island's topology-string key.
- **Eye.** The eye toggles `sharingType` on the companion's own panels (island, overlays, rail, Attention, Ask bar) and is
  labelled "best effort" — Apple documents `NSWindow.SharingType.none` as "A legacy constant that macOS no longer uses"
  (recon A §A3); the Tauri windows keep their own eye via `set_hidden_from_capture` (`DESK/island/macos.rs:64-71`), and
  the two are independent in v0 (§12 item 9).
- **Meeting flows (all RPCs exist).** Hydrate on connect: `meeting_status` + `meeting_briefs(id)` + `meeting_live(id)`
  (`api.rs:1569`, `:1604-1611`); events `Started State TopicAdded TopicRemoved Brief Utterance Listening ApprovalWaiting
  PostProgress ReviewReady Deleted OracleDeciding OracleDecision` on the stream the companion already holds
  (`RpcClient.swift:78-79`) + `Level` for the meters + `TurnStarted/ToolCall*/TurnFinished` for the activity spinner.
  Topic tap → `meeting_tap` (`:1585`); × → `meeting_dismiss` (`:1588`); deeper → `meeting_deeper` (`:1591`); request box
  → `meeting_request` (`:1594`); PTT chord held → `meeting_ptt(true/false)` (`:1598`) with "listening…"; cursor →
  `meeting_cursor(brief | null)` (`:1601`) — the companion is the only client calling it while alive; ⋮ → `meeting_pause/
  resume/stop` (`:1559-1567`); the oracle chip queries `decision_mode('meeting')` every 30 s (`api.rs:1656`). The brief
  stack rule is `UI/store/meeting.ts:181-190` ported verbatim.
- **Visibility.** Shown while `meeting_status.state` is Starting/Recording/Paused/Stopping (`Island.tsx:70` today) or a
  brief/topic arrived in the last 10 min; Settings › Island "Always show" keeps the closed pill (click → `theuth://meetings`,
  a new deep-link route, §5) when nothing is live. Hidden = `orderOut` (a native view is not throttled — the alpha-0 hack
  goes).
- **Keys (island scope).** PTT ``⌥` `` (default from `meeting-settings.ts:11-12`), eye `⌥⇧E`, M mute (= pause), ⌃⇧M stop
  (hold), E dismiss the focused topic, D deeper, ⎋ leaves the request box (resign key, as `island_resign_key` does today).
- **Empty/error.** No meeting → hidden (or the "Always show" pill); degradation badges as today (`Island.tsx:37-47`);
  "transcription down"/"model down" stay badges, never dialogs.
- **Differs from One / Tauri.** One has no island; vs the Tauri island: hover-open, snap-back, per-display memory,
  `orderOut`, a measured notch width, `.statusBar + 1` (26) instead of 25 so our own menus (101) still win.

### 3.7 Native point-and-say (screen 7)
Hold the voice key (⌃S on macOS, §8) → `PointMachine` (the port of `DESK/point/machine.rs:6-18, 23-69`, same constants,
same 17 tests) emits `Start` → one click-through overlay window per display orders front at `.popUpMenu` (101) and the
trail + 14 pt ring are stroked natively (`CAShapeLayer`, 6 pt, round caps, alpha 0.85, malachite from `Palette`) from a
60 Hz `.common` Timer over `NSEvent.mouseLocation` (+ `pressedMouseButtons & 1` when `annotate = mouse`); the companion
streams its own PCM (`dictation_start(16000)`, `api.rs:1496-1497`) and shows the live text in the island panel's voice
mode (96 §0.3's reuse rule holds: the island window shows `VoiceMode` while a gesture is live). Release ≥ 200 ms (or a tap
while locked) → `Stop`: the sampler stops, **every overlay is ordered out and the next run-loop turn awaited**, then
`dictation_stop(id)` and `screen_context({samples, color, since_ms, accessibility})` run concurrently (`api.rs:335-339`);
then `desk_open("theuth")` (cached per engine, `api.rs:2083`) and `send_input(desk, "[voice note, N s] …\n[screen 1/1 …]",
files, null, clientId)` or `steer(…)` when the desk is mid-turn (`api.rs:194-203`, `:218-226`). The answer surface is a
native **answer card** in the Ask bar's receipt style under the voice mode: the desk's `TurnStarted/ToolCall*/TurnFinished`
drive "working…", the final text streams in, "Open ⌘O" → `theuth://c/<desk>`; the spoken reply is OUT of scope (§12 item
10). Esc cancels between press and send (registered only then, as `DESK/point/mod.rs:38-42`); `point:refused` cases map to
hints: `talking` → "Meeting running — use the island to ask"; `already dictating` → "dictating elsewhere"; a Screen
Recording denial → "Screenshots off — sent without the screen" (`voice-key.ts:329-343` today). Off the hub → "theuth's
desk lives on ‹hub›", nothing sent. Shown on screen: the trail, the ring, the voice mode with "‹what was said› · ⌃S held ·
theuth's desk". Differs from 96/Tauri: native drawing (no canvas, no webview throttling), the companion's mic instead of
the engine's, a native answer card instead of the webview's spoken reply.

### 3.8 Settings additions
Rail: presence (as today) · **side** · **pills vs one strip** (off = today's strip, kept for a week as the escape hatch).
Island: **position reset** per display and for all · **Always show** · eye default (hidden from capture, best effort) ·
PTT chord · eye chord. Voice key: chord (⌃S default on macOS) · enabled · include the screen · read what the pointer
touched (accessibility auto/off; names the engine's grant) · annotate always / while the mouse button is held · the
privacy disclosure (52 §8). Keys: the new rows (space, V, P, X, agents panel ⌃⇧A, island rows) in the same recorder with
130 R2-4's collision policy. Every value lives in `CompanionSettings` (`SettingsStore.swift:8-49`); the Tauri settings for
the same things (`meeting-settings.ts`, `voice-key.ts:58`, Settings › Shortcuts `voice-key`) stay for Windows/Linux without
a companion, and the main app's Settings › Shortcuts row for `voice-key` becomes read-only "owned by the companion" while
alive (130 §4.3's pattern).

### 3.9 Visual language
Dark-first glass: the rail pills, the Attention/agents panels, the Ask bar and the island force `NSAppearance(named:
.darkAqua)` (every shipped notch app does; the real notch is black); Settings and the legend follow the system. Radii: pills
and the Ask bar capsules (26/28 pt), cards 24 pt, option rows 10 pt, key caps 4 pt. Type: `enum Type` — headline 24,
question 17, body 15, meta 12.5, caps 10.5 (today's 9–15 pt scale, `recon C C7`). Accent mapping by ruling: gold = needs
you only; One's green → malachite (done) and faience (working/idle, the user bubble); One's blue → faience/lapis; carnelian
only for Stop, errors and the recording bars. Motion: 120–160 ms fade + 6 pt rise on open, hover grow 120 ms, the
working arc 0.38 s; nothing animates under Reduce Motion except opacity. Harness icons from the shared set (§4 D18) as
template images tinted `ink`.

## 4. Decisions D1–D24

One line each: the decision · the default · the reason · the alternative rejected.

1. **D1 Rail geometry.** Four separate 52 pt capsule pills with 12 pt gaps at the right edge (left as a setting), hover-grow
   dots; gold dot = needs you, spinner = working, green (malachite) = idle/done — Sky's F1–F3 frames supersede 130 D4/R2's
   one 26 pt strip; rejected: keeping one strip with inner dividers (not what the frames show).
2. **D2 Agents panel data.** `list_activity` ∪ `agents_list` ∪ `list_sessions` (+ `sessions_status`, `mesh_status`),
   grouped by host — "this Mac" = `owner` `""`/`self_id`, remote = `SessionSummary.owner` named through `mesh_status`;
   remote liveness is FALSE on this engine (`supervisor.rs:6547-6557`), so remote rows get a hollow dot + "on ‹node›"
   until E7 — rejected: a second RPC connection per peer (130 D2: one engine; tokens are per data dir).
3. **D3 Per-row actions.** mic → dictation → `send_input(session, text)` to THAT session (ruling 3); ✕ → `cancel(session,
   "tree")` behind the 3 s hold with ⎋ undo-before-commit and no undo after (a cancel is final) — rejected: a bare click
   that stops (irreversible without a hold) and `scope: root` (a parent stopped alone keeps its children working, which is
   not what ✕ on a row means to a person).
4. **D4 Card context.** E6: the engine adds `AttentionItem.context { request: String?, last_agent_text: String? }` (≤ 160
   chars each) for session-born kinds; interim fallback `SessionSummary.title` + `AgentInfo.last` — rejected: `load_session`
   per card (too heavy for a glance surface).
5. **D5 Keys.** The shared table in §8; space = alias of R (focus reply); V = hold-to-talk reply; P = point/attach the
   screen via `screen_context`; X = stop agent on cards of running/waiting agents only, never on finished cards (recon C
   do-not-copy 5); E = clear, never approves or answers; S stays snooze; `?` and ⌘/ open the legend — rejected: One's
   "camera S" (S is snooze) and "Discard E" on ask cards (E must not dismiss a question).
6. **D6 Header controls.** Position dots + ‹J K› + ? + ✕ esc as real buttons that act without activating the companion
   (130 D15) — rejected: the text "1 of 3" alone.
7. **D7 Empty-state copy.** "Nothing needs you." + "Questions and finished work land here; the menu bar count will tell
   you." with the wordmark glyph — rejected: One's "Suspiciously quiet." verbatim (reads as a joke when something broke).
8. **D8 Level bars.** Drawn from `DictationLevel.levels` (one bar per value, newest right, carnelian) in the Ask bar and the
   actions pill — rejected: the single `levels.max()` scale (today) and a synthetic animation.
9. **D9 Island window stack** (recon A §A5). Borderless non-activating `NSPanel`, `level = .statusBar + 1` (26, re-asserted
   on every show), `canJoinAllSpaces | fullScreenAuxiliary | stationary | ignoresCycle`, `hidesOnDeactivate = false`, never
   `isFloatingPanel`, notch geometry from the auxiliary areas with a 300 × menu-bar pill fallback, an oversized window with
   the open/close animated inside SwiftUI (`.bouncy(0.4)` open, `.smooth(0.4)` close), `hitTest` click-through outside the
   drawn shape, a `performDrag` grip with snap-back within 40 pt, frames per display UUID in screen-relative coordinates
   with validation, re-place on `didChangeScreenParameters`, `sharingType = .none` kept but best-effort, notchprompt's
   `sendEvent`-makes-key and `acceptsFirstMouse` tricks, `.darkAqua` forced — rejected: `.screenSaver` (1000; above our own
   menus), `setFrameAutosaveName` (one frame per name, not per display), `isMovableByWindowBackground` (fights buttons and
   hover-open), private SkyLight/CGS calls (boring.notch's lock-screen trick; not signable, not needed).
10. **D10 Island visibility.** Visible while a meeting is Starting/Recording/Paused/Stopping or a brief/topic is pending
    (≤ 10 min); Settings › Island "Always show" keeps the closed pill — rejected: always visible by default (a notch pill
    with nothing to say).
11. **D11 Island content parity.** Meters · `rec mm:ss` · oracle chip · topic buttons · degradation badges · eye ·
    pause/stop · brief pane (title + 3 bullets + deeper + cursor + `↑N new`) · transcript dropdown · request box — exactly
    `Island.tsx:181-398`; rejected: a reduced "glance" island (the briefs ARE the island).
12. **D12 Point machine.** `DESK/point/machine.rs` ported into `CompanionCore/PointMachine.swift` with `HOLD 200 /
    DOUBLE_TAP 300 / MIN_NOTE 800 / DONE 6000 ms` and its 17 tests ported — rejected: reusing `AskVoiceMachine` (a hold-only
    machine without the double-tap lock).
13. **D13 Overlays.** One click-through window per display at `.popUpMenu` (101), trail + ring stroked natively, excluded
    from the screenshot **by ordering out and awaiting one run-loop turn before `screen_context`** (the mechanism 96's shell
    uses, `DESK/point/mod.rs:307-322`) — not by `sharingType`, which Apple now calls legacy (recon A §A3); `sharingType =
    .none` is still set as a second, best-effort layer for legacy CGWindowList consumers.
14. **D14 Answer surface.** A native answer card in the Ask bar's receipt style inside the island's voice mode (desk events
    drive it, "Open ⌘O" for the rest); the spoken reply is OUT of scope and named in §12 — rejected: `AVSpeechSynthesizer`
    locally (a different voice from the app's) and an engine `speak` device sink tonight (an engine change outside W0).
15. **D15 Tauri gates behind `companion-alive` on BOTH shells** (platform-neutral TS, done once in the contract):
    `UI/App.tsx:384-398` calls `voiceKeyBind("")` when alive turns true and re-binds the stored chord when it turns false;
    `UI/components/meeting/Island.tsx:104-123` never calls `islandShow` while alive and `routes/island.tsx` renders
    nothing; `UI/lib/meeting-hotkeys.ts:16-33` skips `register(ptt/eye)`; the island webview stops calling
    `meeting_cursor`. `island::init`/`point::init` stay (inert without a chord and a show) — rejected: a shell reorder of
    `island::init` after `companion::init` (`DESK/lib.rs:342-346`; the UI gate is enough and cross-platform).
16. **D16 Voice-key default per platform** in `UI/lib/shortcuts.ts:52`: macOS `Control+S` (96 ruling), Windows/Linux
    `Control+Alt+Space` — the builder's finding (Ctrl+S is Save everywhere and a registered hot key swallows it; today main
    binds Ctrl+S on every desktop, `App.tsx:389` — a bug); lands in the contract, not the Windows wave (parent's ruling
    01:04); rejected: `Alt+Space` (window menu, PowerToys Run), `Ctrl+Shift+S` (Save As), `Win+Shift+S` (Snipping Tool).
17. **D17 Companion voice key = the companion's own mic.** `dictation_start(16000)` + `dictation_audio` as the Ask bar
    (130 R2-2), not 96's `dictation_start(null)` (engine mic) — one audio path in the companion; rejected: the engine mic
    (needs the engine to be app-spawned for the TCC grant; a headless engine has none).
18. **D18 Shared harness icon set** at `crates/desktop/companion-assets/harness/{omp,opencode,claude,codex,theuth,unknown}.svg`
    + PNG 16/20/24/32 generated by `scripts/render-harness-icons.swift`, mapped from `AgentInfo.agent` / `SessionSummary.agent`
    (`None` = theuth; unknown key = `unknown`); both bundle scripts copy them — rejected: SF Symbol stand-ins (Windows has
    none; the builder asked for repo assets, §7.3 item 7).
19. **D19 Settings re-home.** Island frames/size, eye, PTT chord, eye chord and every `voiceKey.*` move from the webview's
    localStorage (`meeting-settings.ts:9-28`, `voice-key.ts:58`) into `CompanionSettings`; the Tauri settings stay for
    Windows/Linux without a companion — rejected: an engine-side settings store for them (54 §0.6: the daemon knows nothing
    about panels).
20. **D20 No new TCC on macOS.** Capture stays in the engine (`screen_context`), AX reads stay in the engine, the companion's
    mic is already its own grant (the usage string in `bundle-companion.sh:62`, exercised by the Ask bar since W6), no
    `CGEventTap` → no Input Monitoring — rejected: capturing in the companion (a second Screen Recording + Accessibility
    grant for a second bundle id, recon B §B4).
21. **D21 The Ask bar stays desk-routed; the agents panel's mic is direct** (ruling 3 + 130 D7/R4) — rejected: an agent
    picker or ⌘1–9 in the Ask bar.
22. **D22 Hand-over rule, one on both platforms.** While a native companion is alive the shell's island, overlays and
    voice-key binding stand down through the existing `companion-alive` signal (`DESK/companion/macos.rs:123-139`,
    `DESK/companion/windows.rs:166`) — no new RPC or flag; the companion owns the chord whenever it runs and Tauri binds only
    when `companion_alive()` is false at boot and on the alive→false edge (two Carbon registrations of one chord both fire).
23. **D23 Agents panel is the peek.** It replaces the never-built 130 §4.1 peek card; an agent dot click opens the panel
    scrolled to the row; ⏎ / ⌘O deep-link to the session — rejected: a separate peek popover (one more surface to keep).
24. **D24 Each part lands alone.** W1, W2, W3 are independently shippable behind the contract; a part that is not
    feature-complete by morning stays on its branch (Sky's 130 rule) and the Tauri surface keeps serving — rejected: one
    integrate branch that lands all-or-nothing.

## 5. Architecture

### 5.1 Module map — the Swift package (`crates/desktop/companion`)
| Layer | New / changed | What |
|---|---|---|
| `COMP/TheuthKit/Models.swift` | + `MeetingStatus`, `Topic`, `Brief`, `Utterance`, `MeetingEvent` (13 variants), `Level`, `ScreenContextReq/Reply`, `CancelScope`, `SessionSummary` (`title`, `cwd`, `project`, `running`, `owner`), `SessionsStatus`, `AttentionItem.context` (E6), `Packet.ifWrong`, `priority`/`rank` | hand-written `Codable`, each pinned by a fixture in `Tests/TheuthKitTests/Fixtures` exported from the ts-rs bindings (130 D11) |
| `COMP/TheuthKit/RpcClient.swift` | + `listSessions`, `sessionsStatus`, `cancel(session, scope)`, `agentPause`, `screenContext(req)`, `sendInput(session, text, files, delivery, id)`, `meeting*` (status, briefs, live, topics, tap, dismiss, deeper, request, ptt, cursor, pause, resume, stop, permissions), `decisionMode` | positional params in `api.rs` order, as `:216-262` today |
| `COMP/CompanionCore/RailPillsModel.swift` | new | pills from `Rail` (`RailModel.swift:99-144` unchanged): inbox count · agent dots · actions level · more; hover state; tests |
| `COMP/CompanionCore/AgentsModel.swift` | new | rows and host groups from `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status`; `AgentRow { id, session, name, harness, state, children, last, host, live: Bool }`; order roots → children; tests over fixtures incl. a remote-owned row |
| `COMP/CompanionCore/CardAnatomy.swift` | new (+ `CardModel.swift`) | header (position, controls), status line (harness, age, project), context bubbles (E6 or interim), options with Recommended/→, footer verbs (`stop` only when the agent runs or waits; `clear` when clearable), `ifWrong`; tests per kind |
| `COMP/CompanionCore/MeetingReducer.swift` + `IslandModel.swift` | new | the port of `UI/store/meeting.ts:156-220` + the brief-stack rule `:181-190`; island state closed/open/detached, visibility (D10), frame memory + validation; tests (reducer per event, snap-back distance, off-screen frame rejected) |
| `COMP/CompanionCore/PointMachine.swift` | new | port of `DESK/point/machine.rs` with the 17 tests; `OverlayModel.swift` = samples → per-display paths (logical points per `NSScreen`) |
| `COMP/CompanionCore/KeyBindings.swift` | rows added (§8) | `agents` and `island` and `pointAndSay` scopes; per-platform default table |
| `COMP/Companion/RailPills.swift` | replaces `RailView` in `RailPanel.swift` | SwiftUI pills; per-mark anchors via `onPreferenceChange`; the panel/monitor plumbing of `RailPanel.swift:26-116` kept |
| `COMP/Companion/AgentsPanel.swift` | new | `KeyPanel` + glass; rows with mic (reuses `VoiceRecorder` + `AskVoiceMachine` with a target session) and ✕ (`UndoQueue` hold) |
| `COMP/Companion/PanelView.swift` | re-cut | header controls, status line, bubbles, reply row P/R/V/⏎, footer pills, `Type` scale, radius 24 |
| `COMP/Companion/Island/IslandWindow.swift` + `IslandPanel.swift` + `IslandViews.swift` | new | the D9 window stack; the strip/brief/transcript/request views; `VoiceMode` view for W2 |
| `COMP/Companion/PointAndSay/{PointController,OverlayWindows,PointerSampler,AnswerCard}.swift` | new | hotkey down/up → machine → effects; overlays per display; 60 Hz sampler; the answer card |
| `COMP/Companion/Settings/*` | tabs added | Island · Voice key; new rows in Keys; `CompanionSettings` fields (D19) |
| `COMP/Companion/AppDelegate.swift` | 4 install calls | `agentsPanel.install(menu:)`, `island.install()`, `pointAndSay.install()`, icon set load — one line each at the W2/W3 pattern (`:126-136` menu, `:25-33` hotkeys) |
| `scripts/bundle-companion.sh` | + icons | copy `crates/desktop/companion-assets/harness/*` into `Contents/Resources/harness/` after `:37-38` |

### 5.2 The Tauri shell and UI edits (contract C, platform-neutral)
- `UI/lib/shortcuts.ts:52`: `'voice-key'` default = `isMacDesktop() ? 'Control+S' : 'Control+Alt+Space'` (helper
  `UI/lib/hotkeys.ts:39`), label unchanged; existing persisted overrides win.
- `UI/App.tsx:384-398` (`useVoiceKeyBinding`): subscribe `onCompanionAlive` (`UI/lib/companion.ts:10-27`); alive → `voiceKeyBind("")`
  (`DESK/point/mod.rs:119-120` treats `""` as unregister); dead → re-bind `want`.
- `UI/components/meeting/Island.tsx:104-123`: `if (companionAlive) return` before `islandShow`; `:165-168` no
  `meeting_cursor` while alive; `UI/routes/island.tsx:19-37` renders `null` while alive.
- `UI/lib/meeting-hotkeys.ts:16-33`: `if (companionAlive) return` before `register(ptt)`/`register(eye)`.
- `DESK/lib.rs:524-551` (`deep_link_route`): + `theuth://meetings` → navigate to `/meetings` (for the island's idle pill).
- Nothing else in the shell: `island::init`, `point::init`, the six island commands and `voice_key_bind` stay for
  Windows/Linux and for a Mac without the companion; `DESK/companion/macos.rs:123-139` already flips the signal.

### 5.3 Engine edits (W0)
E6 in `attention.rs` (`AttentionItem` + `Inputs` + `gather` + `derive`), E7 optional in `api.rs`/`crates/agent` — §6.

### 5.4 Sequences
**A. Agents-panel mic (ruling 3).**
1. Pointer down on a row's mic (or V with the row focused) → `AgentsPanel` calls `voice.press(target: row.session)`.
2. `VoiceRecorder.start()` (`VoiceRecorder.swift:8-35`) → `dictation_start(16000)` (`api.rs:1496-1497`) → every ~100 ms
   `dictation_audio(id, pcm)` (`:1502-1503`); `DictationText` events fill the row's reply field, `DictationLevel` the bars.
3. Release → `dictation_stop(id)` (`:1507-1508`) → `DictationFinished` → the text stays in the field → `UndoQueue` 3 s hold
   with "sending to ‹name› · ⎋ undo".
4. Commit → `send_input(session, text, [], null, clientId)` (`:194-203`) → receipt "sent"; `TurnStarted` from that session
   flips the row's dot to the spinner.
5. Refusals: `already dictating` → "dictating elsewhere"; `talking` → "Meeting running — use the island"; a dead session →
   `send_input` error shown verbatim in the receipt.

**B. Card ✕ / Stop agent X.**
1. X (or the Stop agent pill) on a card whose agent is running or waiting → the hold pill "Stopping ‹name› and N
   children · ⎋ undo" (N = `activity.children`).
2. ⎋ inside the hold → nothing sent, "Taken back". Hold elapses → `cancel(session, "tree")` (`api.rs:255-256`).
3. `TurnFinished { stop_reason: "cancelled" }` per stopped session arrives on the stream (133 §2); the card's status line
   says "stopped · just now"; the item itself leaves `attention_list` on the next `AttentionChanged` if the engine retires
   it, else the card stays until E clears it (a stop never implies an answer).
4. On a desk row the same hold commits `agent_pause(name, true)` (`api.rs:2089`).

**C. Island open-from-topic.**
1. `TopicAdded` (`MeetingEvent`) → `MeetingReducer` appends the topic → `IslandModel.visible = true` (D10) → `IslandWindow`
   places itself (saved frame for this display UUID, validated, else the notch default) and `orderFrontRegardless()`,
   level re-asserted (D9).
2. The closed pill shows the topic count; hover 0.3 s or click opens the strip; the topic button tap →
   `meeting_tap(topic)` (`api.rs:1585`) → `TopicRemoved{Viewed}` + `Brief{source: Tap}` → the brief pane opens, cursor →
   `meeting_cursor(brief)` (`:1601`).
3. "deeper" → `meeting_deeper(brief)` (`:1591`) → `Brief{source: Deeper}` stacks on top; newer/older move the cursor;
   `↑N new` when auto briefs queued behind a stepped-back cursor (`meeting.ts:181-190` rule).
4. The request box takes key only while typing (`becomesKeyOnlyIfNeeded`), ⏎ → `meeting_request(text)` (`:1594`), ⎋ or
   blur → `resignKey()` and the previous app is key again (as `island_resign_key` today, `DESK/island/macos.rs:92-100`).
5. `State{Done|Failed}` → `IslandModel.visible = false` after the pane's close animation → `orderOut`.

**D. Point-and-say end to end.**
1. ⌃S down (KeyboardShortcuts key-down, `AskBarPanel.swift:142-158`'s pattern) → `PointMachine.step(Pressed)` → `Start`.
2. `OverlayWindows.reveal()` (one per `NSScreen`, level 101, `ignoresMouseEvents = true` unless `annotate = mouse`),
   `PointerSampler.start()` (60 Hz `.common` Timer over `NSEvent.mouseLocation`; `pressedMouseButtons & 1`), the island
   shows `VoiceMode` ("listening"), `VoiceRecorder` + `dictation_start(16000)`; Esc registered.
3. Each tick → `OverlayModel.append(sample)` → the display's `CAShapeLayer` path + ring; `DictationText` → live words.
4. Key-up after ≥ 200 ms (or a tap while locked) → `Stop`: sampler off, **overlays `orderOut` + one awaited run-loop turn**,
   Esc unregistered; concurrently `dictation_stop(id)` → wait `DictationFinished` (cap `FINAL_TEXT_MS`) and
   `screen_context({samples, color: malachite hex, since_ms, accessibility})` (`api.rs:335-339`) → `{lines, files}`.
5. Body = `[voice note, N s] ‹text›` + the `lines`; `desk_open("theuth")` cached per engine (`api.rs:2083`; refused off the
   hub → hint, nothing sent); `send_input(desk, body, files, null, clientId)` or `steer(desk, body, id, files, null)` when
   the desk is mid-turn (`:194-203`, `:218-226`).
6. The answer card: `TurnStarted` → "working…", tool names as they start, the final text on `TurnFinished`; "Open ⌘O" →
   `theuth://c/<desk>`; `DONE_MS` 6000 after the final text → `Replying → Idle` and the island returns to its meeting
   content or hides.
7. Empty transcript → "Nothing recognized", nothing sent; a note < 800 ms → the hint "Hold to talk · double-tap to lock".

## 6. Engine and RPC changes

Two additive engine changes, one of them optional; everything else the companions need already exists.

### E6 — `AttentionItem.context` (required by W1's bubbles; the interim fallback ships without it)
- **Shape** (`attention.rs:78-131` gains, after `next_steps`):
  ```rust
  /// 149 E6: the conversation the item sits in, for session-born kinds (ask, approval, finished, resume,
  /// blocked-with-session): `request` = the user's last input to that session, `last_agent_text` = the first
  /// line of the agent's last assistant text before the item arose; both ≤ 160 chars; `None` when unknown.
  #[serde(default)]
  #[cfg_attr(feature = "ts", ts(optional = nullable))]
  pub context: Option<ItemContext>,
  pub struct ItemContext { pub request: Option<String>, pub last_agent_text: Option<String> }
  ```
- **Where it is filled.** `derive` is pure over `Inputs` (`attention.rs:313-315`), so the texts are gathered, not fetched:
  `Inputs` (`:258-294`) gains `session_context: HashMap<String, ItemContext>` built in `gather()` (`:941`) from the agent
  store for the sessions that have a pending ask/approval/finished row — the same source `AgentInfo.last` reads (first line
  of the last assistant text, ≤ 80 chars, `crates/agent/src/lib.rs:1230-1231`) widened to 160, and the session's last user
  input (what `SessionSummary.title` falls back to before the title model runs, `:2240-2241`). `derive` copies it onto the
  item by `session`. Cap with the existing `SUMMARY_MAX` (`attention.rs:167`).
- **Bindings and tolerance.** ts-rs export via `just ts-desktop`; the TS `AttentionItem` gets `context?: ItemContext |
  null`; the desktop Attention screens ignore it (no UI change owed); the Swift and C# models decode it as optional and
  fall back to the interim join when `nil` — so W1 does not block on W0.
- **Tests.** `derives_every_kind_and_hides_dismissed_or_snoozed` (`attention.rs:1707`) gains a case with `session_context`
  set: an ask item carries both strings; a task-born decision carries `None`; a 300-char input is cut at 160 on a char
  boundary.

### E7 — mesh-wide activity (OPTIONAL; honest fallback rules until it exists)
- **Shape.** `list_activity(scope: Option<"local" | "mesh">)` — a trailing optional positional like `cancel`'s `scope`
  (`api.rs:255-256`): `mesh` fans out to reachable peers from `mesh_status.peers` (`crates/mesh/src/types.rs:94-108`) with
  a 1 s budget and merges their `AgentInfo` rows stamped with `owner`; unreachable peers are omitted and named in a
  `partial: [EngineId]` side field (or a second RPC `list_activity_mesh` if a positional change ripples through
  `forward::route`). A client that never passes `scope` gets today's answer.
- **Fallback rule (binding on both companions until E7 lands):** a row whose `SessionSummary.owner` is not this engine is
  drawn with a hollow sand dot, the suffix "on ‹node›", no spinner, mic and ✕ disabled with the tooltip "open it on ‹node›"
  — never a guessed state (recon C do-not-copy 10, U2). The `sessions_status` counts (`api.rs:549-550`) label the group.

### Already there (cited so nobody re-adds it)
`send_input(session, text, files: Vec<PathBuf>, delivery, id)` (`api.rs:194-203`) and `steer(session, text, id, files,
delivery)` (`:218-226`) attach image files — the Swift client only passes `[]` today (`AskRpc.swift:9`;
`RpcClient.swift:261-262`); `screen_context` (`:335-339`) writes the PNGs the companion will pass back; `cancel(session,
scope)` (`:255-256`); `agent_pause` (`:2089`); `list_sessions` with `running`/`owner` (`:485-486`); `sessions_status`
(`:549-550`); `mesh_status` (`:1963`); every `meeting_*` verb (`:1548-1611`); `dictation_*` (`:1486-1508`);
`decision_mode` (`:1656`); `attention_dismiss/snooze` (`:2141-2145`); `AttentionChanged` (`attention.rs:138`); the
`Finished` kind (`:70-72`, `:153`).

### Migrations
None expected: E6 is a derived, in-memory field on an item the actor recomputes (`attention.rs:313-315`); E7 is a read
RPC. Checked: no `attention_*` table gains a column; `attention_finished`/`attention_shows` rows (`Inputs.finished`,
`Inputs.shows`, `:277-291`) are unchanged. If W0 finds the last user input is not retrievable from the agent store
without a schema change, E6 ships with `request: None` and the item text alone — say so in `149-as-built-W0.md`.

## 7. The Windows twin

The builder's answer (session `01a11782-11da-76f3-9f71-615f8d166440`, the design-134 orchestrator, 2026-10-09 00:57;
file `~/Develop/theuth-149-briefs/windows-builder-answer.md`) is pasted verbatim in 7.1–7.3; 7.4 is this design's
decisions for Windows. Lead-in, verbatim: "Windows builder for 149 here (design-134 orchestrator). Sky told me tonight to build the Windows side of 149 and land it on main. Answers below. Every path was verified on main dd19bf63 (2.13.0) unless marked."

### 7.1 State (verbatim)
> (a) WHERE THE WINDOWS COMPANION STANDS
> - Landed on main by 134 W6 at 51c7d404 (2026-10-08 07:43). No companion commits since. Tonight's baseline at dd19bf63:
>   - Mac: Core 146/146, Rpc 35/35.
>   - build-B: Debug build 0 errors / 0 warnings in 42 s.
>   - build-B runs packaged 2.12.3+7fd8068f (= W6). A 2.13.0 package of dd19bf63 is building now, done ~02:00.
> - Code: crates/desktop/companion-windows/ (Theuth.Companion.slnx):
>   - src/Theuth.Companion: the WinUI 3 app.
>   - src/Theuth.Companion.Core: portable models, policy and the key table in Windows form. net10.0, so it is unit-tested on the Mac.
>   - src/Theuth.Companion.Rpc: the engine client (engine.json discovery, JSON-RPC over WS).
>   - tests/*.Core.Tests and *.Rpc.Tests.
>   - Scripts: build.ps1, test.sh, probe.ps1, seed.ps1, w2/w3/w5/w6-gate.ps1.
> - Shell side:
>   - crates/desktop/src/companion/windows.rs: spawn, alive check and the tray hand-over (the shell hides its tray icon while the companion is alive).
>   - The installer bundles the companion in package-windows.ps1 step [3b/6].
> - Docs: docs/design/134-companion-windows.md (§4 "The surfaces (Windows form)", §5 Architecture) and 134-as-built-W1..W6.md.
> - What it HAS of 130's surfaces:
>   - Tray icon and menu: badge, Pause with its state, "Quit theuth" and "Quit theuth Companion". File: Tray.cs, Windows/TrayMenu.cs.
>   - Rail as ONE edge-docked tool window. File: Windows/RailWindow.cs.
>   - Attention panel with cards: approvals, questions with numbered options and "(Recommended)", hire/decline, blocked/unblock, decisions. File: Windows/AttentionPanelWindow.cs.
>   - Peek card (Windows/PeekCardWindow.cs) and the "?" legend (Windows/LegendWindow.cs).
>   - Ask bar with hold-to-talk dictation (AskBar/AskBarWindow.cs, ReplyField.cs, DictationCalls.cs, Voice/VoiceRecorder.cs). The recorder is NAudio WASAPI, 16 kHz mono PCM16, sent to the engine's dictation_prewarm/start/audio/stop. It always passes 16000, never null, so the engine host's own mic is never opened.
>   - Toasts with actions: Windows App SDK AppNotification, Do-not-disturb respected, Urgent opt-in, a toast-click relaunch applies its action. Folder: Notifications/.
>   - Settings window: key recorder, launch at login via HKCU Run, effects switches. Folder: Settings/.
>   - Also: pin-to-taskbar tip, failed-turn receipts, a 5 s grace on the engine-loss toast, breakpoints.
> - NEW on Windows for 149 (none of it exists yet):
>   - separate rail pills (inbox / agents / actions / more);
>   - the agents pill and panel: per-agent rows, host grouping, ⑂ children, per-agent mic, ✕ stop;
>   - card additions: position dots, a status line with harness icon + relative time + client, the request bubble with the agent's last message, and "Next steps" with Stop agent / Clear. "Next steps", "Suspiciously quiet" and "Stop agent" appear in neither companion today;
>   - the empty state;
>   - the wide Ask pill (muted open partial, level bars). I'll diff it against W4's AskBarWindow when building; treat it as new;
>   - a native island;
>   - native point-and-say.

### 7.2 Constraints (verbatim)
> (b) CONSTRAINTS
> - Stack:
>   - C# / .NET 10, WinUI 3 on Windows App SDK 2.5.1: trimmed, self-contained, unpackaged, with five runtime-MSIX files extracted for toast identity.
>   - CsWin32 for Win32 interop (Interop/NativeMethods.txt) and NAudio.Wasapi for capture.
>   - Not WPF (134 forks, Sky 2026-10-07).
> - Dev box: build-B, 192.168.8.151, a ThinkPad P50 on Win 11 Pro 25H2 (26200).
>   - Code is written on the Mac in worktrees.
>   - It is compiled and run on build-B through a LAN build service (:8933). The service runs one job at a time; a Debug build takes ~25–45 s; a desk lock covers UI rounds.
>   - The desktop is driven through windows-mcp (UIA, :8931) by fresh subagents. I also have an ssh PTY open to build-B.
> - build-B limits:
>   - ONE display: a 1024×768 virtual "WinDisc" with no monitor attached. Multi-display and mixed-DPI behaviour (island per display, overlay on every display) can be built but not verified live there.
>   - Mic: a real Realtek array. Capture is testable; speech only by playing audio through its speaker.
>   - No model credential in the installed app yet. Until there is one, live checks use seeded attention items (seed.ps1).
> - How I verify:
>   1. Core/Rpc unit tests on the Mac (~10 s).
>   2. A Debug build with 0 errors / 0 warnings on build-B for every commit.
>   3. Live UI checks by a verification-only subagent: screenshots, logs, UIA.
>   4. package-windows.ps1 on build-B: ~15 min when only the companion changed, ~60 min after engine changes.
>   5. Install /S over the running app.
>   6. A packaged recheck by a fresh subagent.
>
>   Your new RPCs reach my live tests only in a Windows build of t.exe (the package, or a cargo build on build-B), so I'll need your contract branch early.

### 7.3 Differences 1–8, plan and landing (verbatim)
> (c) WHAT IS DIFFERENT OR IMPOSSIBLE ON WINDOWS
> 1. ISLAND. There's no notch, so the island is a top-centre pill under the top edge of the chosen monitor's work area (a top taskbar is respected).
>    - The Tauri shell already has this Windows backend: crates/desktop/src/island/windows.rs + mod.rs use WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE|WS_EX_TOPMOST and SW_SHOWNOACTIVATE, exclude the window from capture with SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE), place it "top-centre" by default, and report island_capabilities().
>    - The native version uses the same styles. The drag is a manual move; the position is remembered per display, keyed by monitor device name, in companion.json; double-click snaps back to top-centre of that monitor.
>    - Materials: acrylic and Mica on topmost tool windows FAILED in W6. Rail acrylic showed no material and lost topmost; panel Mica painted light under Dark. Windows pills and the island are therefore OPAQUE: rounded, with a shadow.
> 2. VOICE KEY. RegisterHotKey + MOD_NOREPEAT gives the key-down together with a foreground grant. The release is seen by polling GetAsyncKeyState every 50 ms (Hotkeys.cs, which W4's hold-to-talk uses). So hold and double-tap lock both work.
>    - Ctrl+S cannot be the Windows default. It is Save in every Windows app, and a registered hot key swallows it system-wide.
>    - BUG ON MAIN NOW: crates/desktop/ui/src/lib/shortcuts.ts:52 has `'voice-key': { chord: 'Control+S', scope: 'global' }`. lib/voice-key.ts:47 defaults `enabled: true`. App.tsx:384-396 binds it on every desktop (`isDesktop()`, not macOS-only). So today the Windows app (and Linux/X11) takes Ctrl+S from every app while theuth runs.
>    - The shared key table needs per-platform defaults. My proposal for Windows: Ctrl+Alt+Space. It has no Windows or Office binding; it costs AltGr+Space on AltGr layouts.
>    - Avoid Alt+Space (window menu, PowerToys Run), Ctrl+Shift+S (Save As) and Win+Shift+S (Snipping Tool).
>    - The Ask bar stays Ctrl+Shift+Space.
>    - I'll fix the shell default for Windows/Linux in my wave unless you put it in the contract. Say which.
> 3. POINT-AND-SAY DRAWING. One click-through, layered, topmost tool window per monitor (WS_EX_LAYERED|TRANSPARENT|TOPMOST|NOACTIVATE|TOOLWINDOW).
>    - Drawn with Direct2D/Composition, not XAML, because a WinUI window can't be transparent and click-through reliably.
>    - Sized in physical pixels per monitor (PerMonitorV2), with the pointer sampled at 60 Hz via GetCursorPos.
>    - "annotate = mouse" needs a global button reader: GetAsyncKeyState(VK_LBUTTON), or a WH_MOUSE_LL hook. crates/desktop/src/point/other.rs has none today ("annotate = mouse draws nothing off macOS"); native fixes it.
>    - Overlays get WDA_EXCLUDEFROMCAPTURE so the trail never appears in the screenshot.
> 4. SCREEN CONTEXT. Windows has no TCC-style permission.
>    - Capture per monitor with Windows.Graphics.Capture. Borderless needs GraphicsCaptureAccess.RequestAccessAsync(Borderless); otherwise a yellow border flashes. DXGI Desktop Duplication needs neither border nor prompt.
>    - DRM-protected windows and WDA_EXCLUDEFROMCAPTURE windows come out black.
>    - Under the pointer, UI Automation ElementFromPoint is the AX equivalent: names, control types, ValuePattern. Browsers expose the address-bar URL that way.
>    - Elevated windows (UIPI) may refuse reads from the non-elevated companion.
> 5. MIC. WASAPI shared-mode capture.
>    - The only gate is Settings › Privacy › Microphone › "Let desktop apps access your microphone", global for unpackaged apps. When it is off we get access denied and show a clear error.
>    - Windows shows its own mic-in-use indicator.
>    - The per-agent mic is the same recorder, with its transcript sent to send_input for that session.
> 6. MEETINGS on Windows are real. crates/meeting/src/capture/system.rs captures through flexaudio WASAPI (process) loopback, and supported() returns Partial: "no echo cancellation; headphones recommended". So the island has real meetings to show.
> 7. ICONS. Windows has no SF Symbols; generic glyphs come from Segoe Fluent Icons. Harness icons need shared assets in the repo (PNG or SVG at 16/20/24/32 px). Please put the icon set in the contract.
> 8. HAND-OVER. While a native companion is alive, the shell's Tauri island, overlays and voice-key binding must stand down, as the tray already does through companion/windows.rs. It should be the same rule on both platforms. Please make it part of the contract (an RPC/flag, or the shell's alive check).
>
> PLAN. When your RPC list and key table arrive, I'll cut the Windows waves:
> 1. Rpc/Core client and models for the new RPCs (Mac-testable).
> 2. WinUI surfaces: rail pills, agents panel by host, card anatomy, empty state, Ask pill.
> 3. Native island + point-and-say: overlays, capture, UIA, voice key.
> 4. Live verification on build-B, package, packaged recheck.
>
> Worktree: ~/Develop/theuth-134-w7. I'll rebase it onto your contract branch once it exists; tell me its name. Briefs: ~/.theuth/briefs/2026-10-09/149-windows/.
>
> Landing: I'll fold your contract and land the Windows part on main myself, announced to the active roots. Your call whether that is before or after the Mac part.

### 7.4 The decisions this design makes for Windows
1. **Island = top-centre pill** under the top edge of the chosen monitor's work area (a top taskbar respected), the shell's
   Windows island styles (`DESK/island/windows.rs:1-19`: `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST`,
   `SW_SHOWNOACTIVATE`) on a `ToolWindow` (`WIN/Theuth.Companion/Windows/ToolWindow.cs:15-18, 43-53`); manual drag;
   position remembered **per display keyed by monitor device name** in `companion.json`; double-click snaps back to
   top-centre of that monitor; the same visibility rule (D10) and content (D11); **opaque**, rounded, with a shadow
   (acrylic/Mica failed on topmost tool windows in W6 — 7.3 item 1).
2. **Pills are opaque** too, same geometry as D1 in effective pixels; the rail keeps the W6 focus hand-back.
3. **Voice key = Ctrl+Alt+Space** (D16, in the contract); the Ask bar stays Ctrl+Shift+Space; `RegisterHotKey +
   MOD_NOREPEAT` down, 50 ms `GetAsyncKeyState` polling for the release (`WIN/Theuth.Companion/Hotkeys.cs:10-14, 102-106`)
   feeds the same `PointMachine` port (C#, same constants, same tests).
4. **Overlays = Direct2D/Composition** layered topmost tool windows per monitor (`WS_EX_LAYERED | TRANSPARENT | TOPMOST |
   NOACTIVATE | TOOLWINDOW`), physical pixels (PerMonitorV2), `GetCursorPos` at 60 Hz, `GetAsyncKeyState(VK_LBUTTON)` for
   `annotate = mouse`, **`WDA_EXCLUDEFROMCAPTURE`** so the trail never appears in the capture — and the overlays are still
   hidden before `screen_context` is called (D13's rule, belt and braces).
5. **Screen context stays the engine's** (`screen_context`, `api.rs:335-339`): on Windows the engine captures with
   WGC/DXGI and reads UIA `ElementFromPoint` for the `[screen …]` line (the builder's 7.3 item 4) — any Windows capture
   work lands in `crates/computer`, not in the companion; `accessibility: off` until the UIA reader exists (96 §0.14).
6. **Same key table, icons, RPCs** (§8, D18, §6); the C# `Core` mirrors `RailPillsModel`, `AgentsModel`, `CardAnatomy`,
   `MeetingReducer`, `IslandModel`, `PointMachine` by name so the tests mirror too.
7. **Same companion-alive stand-down rule** (D22): the shell's named-mutex alive check (`DESK/companion/windows.rs:58-74,
   166`) already flips `companion-alive`; the UI gates of D15 are platform-neutral, so nothing Windows-specific is owed.
8. **Mic:** WASAPI shared-mode capture as W4; the only gate is the global desktop-apps microphone privacy switch — access
   denied → a clear error in the Ask bar / row ("Microphone is off for desktop apps — Settings › Privacy › Microphone").
9. **Landing:** the builder folds `integrate/149` (the contract) into `~/Develop/theuth-134-w7`, cuts Windows waves 1–4
   (7.3 PLAN), verifies on build-B (one 1024×768 virtual display — multi-display behaviour is built but not verified there,
   said in the as-built), packages, and lands the Windows part on main itself, announced to the active roots, **after the
   Mac contract C is on main and independent of the Mac parts' landing order**.

## 8. The shared key table

One `KeyTable` (`COMP/CompanionCore/KeyBindings.swift:167-190` today; the C# `KeyBindings.cs` mirrors it) with five
scopes. Rows marked ★ are new in 149. macOS chords first, Windows after `/` where they differ (⌘ → Ctrl everywhere else,
134 D6). Fixed aliases (↓/↑, keypad ⏎, `?`) stay unrebindable; ⎋ is fixed in every scope.

| Scope | Action | macOS default / Windows | Notes |
|---|---|---|---|
| anywhere | Open the panel · hold to talk | ⌃⇧Space / Ctrl+Shift+Space | unchanged (`KeyBindings.swift:168`) |
| anywhere | Open the Ask bar | unbound | unchanged |
| anywhere ★ | Open the agents panel | ⌃⇧A / Ctrl+Shift+A | toggles; opens scrolled to the first working row |
| anywhere ★ | Voice key — point and say | **⌃S / Ctrl+Alt+Space** | D16; hold ≥ 200 ms = talk, double-tap = lock, Esc cancels between press and send |
| anywhere ★ | Meeting push-to-talk | ⌥` / Alt+` | from `meeting-settings.ts:11`; registered only while a meeting is live |
| anywhere ★ | Meeting eye (hide from capture) | ⌥⇧E / Alt+Shift+E | from `meeting-settings.ts:12`; only while live |
| panel | 1 · 2 · 3 · ⏎ · J/K · ↓/↑ · D · O · ⌘O · ⌘L · ⌘/ · ⌘, · ⎋ | as today (`:170-185`) | |
| panel | S snooze 30 m | S | stays (`:176`); One's camera-S is refused |
| panel | E clear | E | stays (`:177`); only when `clearable`; never answers |
| panel | R reply in words | R | stays (`:181`) |
| panel ★ | space = focus the reply field | space | alias of R (`.space` exists unbound, `:67`); typed as a space once the field has focus |
| panel ★ | V hold-to-talk into the reply | V | the Ask bar's machine with the card's session |
| panel ★ | P point — attach the screen | P | `screen_context` + `files` on the reply; closes the panel first |
| panel ★ | X stop agent (hold) | X | only on cards whose agent runs or waits; `cancel(session, "tree")` after the hold |
| askBar | ⏎ send · ⌘O · ⌘/ · ⎋ | as today (`:186-189`) | |
| askBar ★ | P point — attach the screen | P | same as the panel's P, when the field is empty or with ⌘P while typing |
| agents ★ | J/K · ↓/↑ move · ⏎ open session · ⌘O | | |
| agents ★ | V hold-to-talk into the row · X stop (hold) · ⎋ close | | the hold pill names N children |
| island ★ | M mute (pause) · ⌃⇧M stop (hold) · E dismiss topic · D deeper · ⏎ in the request box · ⎋ leave the box | | the island takes key only in the request box |
| pointAndSay ★ | ⎋ cancel | fixed | registered only between press and send (`DESK/point/mod.rs:38-42`'s rule) |

**Collision policy (130 R2-4, unchanged).** Same scope: the recorder refuses with "Already used by ‹label›" and offers
"Use here instead"; never two actions on one chord in one scope (the persisted-table test extends to the new scopes).
Across scopes the same chord is allowed (panel V and agents V never coexist). Global vs macOS: `CopySymbolicHotKeys()` and
modifier-less keys refused as "Reserved by macOS"; global vs another app: the exclusive `RegisterEventHotKey` probe
(`KeyBindings.swift:286`) — on failure the previous binding stays and the row says "held by another app"; at launch a
saved global chord that fails falls back to its default with a one-line notice. Windows: `RegisterHotKey` answering 0 is
the collision signal (`WIN/Theuth.Companion/Hotkeys.cs:77`); Win-key chords refused. Text input: while any field has
focus, single keys are typed, not actions (`KeyGrammar.swift:46-50`). The Tauri shell's `voice-key` row in Settings ›
Shortcuts becomes read-only while a companion is alive and points at the companion's Settings › Keys.

## 9. Permissions, signing, privacy

### macOS
- **No new TCC grant** (D20; recon B §B4). Microphone: the companion's own — usage string in `scripts/bundle-companion.sh:62`,
  `AVCaptureDevice.requestAccess` in `COMP/Companion/Voice/VoiceRecorder.swift:8-35`, exercised by the Ask bar since W6;
  the entitlement `com.apple.security.device.audio-input` is needed only when sandboxed (the companion is hardened,
  not sandboxed — `crates/desktop/companion.entitlements.plist:5-8` stays empty; verify once on the release Mac, 130 W1S
  addendum A2). Screen Recording and Accessibility: stay theuth.app's through the app-spawned engine
  (`crates/computer/src/screen.rs:52-71`; `ax.rs:101`; TCC trusts the responsible process — 130 SB/SC); a headless engine
  started by launchd or a terminal needs its own grant, true today, one line in Settings › Voice key says so. System
  Audio Recording Only: the engine's (`crates/desktop/Info.plist:6-12` carries the strings for theuth.app). Input
  Monitoring: not used — Carbon hot keys, `NSEvent.mouseLocation`/`pressedMouseButtons` polling, no `CGEventTap`
  (96 §0.6; `DESK/point/macos.rs:57-60`'s precedent).
- **Eye = best effort.** Apple: `NSWindow.SharingType.none` is "A legacy constant that macOS no longer uses … Don't use this
  value to hide or omit content from being captured" (recon A §A3). We still set it on every companion panel (legacy
  CGWindowList consumers such as browser `getDisplayMedia` honour it in field reports), label the toggle "Hide from screen
  sharing (best effort)", and §10 adds a self-capture check so the as-built states what it did on this macOS. The overlays'
  exclusion from the point-and-say screenshot never relies on it (D13).
- **Signing and release.** Unchanged from 130 W1S: the nested app at `Contents/Library/LoginItems/Theuth Companion.app`
  signed inside-out with hardened runtime before the outer app (`scripts/package-macos.sh:51-55`), notarization of the
  whole bundle, no provisioning profile needed (Time Sensitive Notifications remains the only restricted entitlement, not
  used). The new resources (`Resources/harness/*.png`) are sealed inside the bundle; nothing touches `Bundle.module`
  (130 R2-1).
- **Privacy.** Point-and-say sends screenshots that may contain private information: the Settings › Voice key tab keeps
  52 §8's disclosure and the "include the screen" switch; the images live under `<data_dir>/attachments/` as today
  (`api.rs:330-334`); the `[screen …]` line is untrusted content to the agent (27 §0.9). The agents panel's mic sends to
  one named session, shown in the hold pill before anything leaves the machine.

### Windows
- Microphone: the global "Let desktop apps access your microphone" switch is the only gate (7.3 item 5); denied → a clear
  error, never a crash; Windows shows its own mic-in-use indicator. Overlays and panels: `WDA_EXCLUDEFROMCAPTURE`
  (`WIN/Theuth.Companion/Windows/ToolWindow.cs:51-53`) — a real exclusion on Windows, so the eye is not "best effort"
  there; DRM/excluded windows come out black in our own capture (7.3 item 4) — acceptable. UIPI: elevated windows may
  refuse UIA reads and injected input from the non-elevated companion — the `[screen …]` line then says `accessibility:
  off` for that display. No permission dialog exists for `RegisterHotKey`, `GetCursorPos`, `GetAsyncKeyState`.
- Packaging unchanged (`scripts/package-windows.ps1:18-23, 183` step 3b/6); the icon set ships in the companion's
  `Assets/`.

## 10. Verification

### Unit tests (Swift Testing via `crates/desktop/companion/test.sh`; C# `test.sh` on the Mac)
- `RailPillsModel`: pills from a `Rail` fixture (count, dots ≤ 12 + `+N`, level bars from `levels`), hover state.
- `AgentsModel`: grouping by owner (`""`, `self_id`, a remote id) with `mesh_status` names; a remote row is `live: false`
  with mic/✕ disabled; children folding; desks as rows; the label is `title`/`name`, never the harness key (fixes C1.6).
- `CardAnatomy`: per kind — header position, status line ("just now" < 45 s, project fallback chain), bubbles from E6 or
  the interim pair, options "(Recommended)" + "→", footer verbs: `stop` present for ask/approval/blocked/decision with a
  running or waiting agent, absent for finished; `clear` only when clearable; `ifWrong` rendered when present.
- `MeetingReducer`: one test per `MeetingEvent` variant mirroring `UI/store/meeting.ts:156-220`, the brief-stack rule
  (`:181-190`: auto at the top shows, stepped-back queues + `↑N`, tap/request jumps); `IslandModel`: visibility (D10),
  snap-back within 40 pt, a frame that intersects no `visibleFrame` is rejected, frames keyed by UUID survive a topology
  change.
- `PointMachine`: the 17 tests of `DESK/point/machine.rs` ported one-to-one (hold/tap/double-tap-lock/min-note/refused/
  done timing); `OverlayModel`: samples → per-display paths in that display's logical points, ring at the newest point.
- `KeyTable`: the persisted-table uniqueness test over the five scopes; per-platform default table; `space` maps to
  `.reply` in the panel scope only.
- `TheuthKit` fixtures: `AttentionItem` with and without `context`, `Packet.if_wrong`, `priority`/`rank`,
  `SessionSummary` (`owner`, `running`, `cwd`, `project`), `SessionsStatus`, the 13 `MeetingEvent` variants, `Level`,
  `ScreenContextReply` — exported from the ts-rs bindings with `just ts-desktop`.
- Engine (W0): the `attention.rs:1707` test extended for E6 (§6); E7, if built, a two-engine test over the mesh test
  harness or an explicit "not verified live" line in the as-built.

### Probes (the companion binary, `Flags`, `Palette.swift:69-77` pattern) — every probe `--shareable`, screenshots to
`/tmp/149-<probe>.png`, never acting, never recording the mic, never posting
- `--probe-rail N` — the pills over a seeded `Rail` (needs-you 3, agents 7 incl. one error and one lapsed, level bars at
  0.6); the sliver; left side.
- `--probe-agents N` — the agents panel over a fixture with two hosts (one remote-owned row drawn hollow), a row in its
  hold pill ("Stopping w3 and 2 children"), a row with the reply field open.
- `--probe-panel <kind>` — F4 and F5 fixtures: finished (next steps, Clear E, no Stop), ask (bubbles, if_wrong, options
  with Recommended, Stop agent X, S snooze), approval, review with `Packet.options`; the empty state.
- `--probe-island <closed|open|detached|nonotch>` — the three states on the notched display and the pill fallback forced
  as if no screen had a notch; a frame restore from a saved UUID; a saved off-screen frame rejected (logged).
- `--probe-point` — overlays revealed with a synthetic 2 s spiral path on every display, then the Stop sequence with a
  **synthetic 16 kHz PCM tone** fed to `dictation_audio` and a real `screen_context` call against the running engine;
  asserts the overlays were ordered out before the call (the capture must not contain malachite pixels along the path —
  checked by sampling the returned overview PNG); nothing is sent (`--probe-point` stops before `desk_open`).
- `--probe-eye` — captures the companion's own island window with ScreenCaptureKit and reports whether `sharingType =
  .none` hid it on this macOS (the honesty line for §9).

### Live checks against the running engine (the orchestrator runs them; Sky's engine via `~/.theuth/engine.json`)
1. Rail pills show real counts; hover grows a dot; the agents panel lists this Mac's sessions with the right names and
   `⑂N`; a remote-owned row (if any) is hollow with "on ‹node›".
2. Per-row mic into a scratch session (`new_session` first): the text lands as a user message; the row's dot spins on
   `TurnStarted`.
3. Per-row ✕ on that scratch session mid-turn: the hold pill, ⎋ undo once, then commit → `TurnFinished { cancelled }`.
4. A seeded ask item: F5 anatomy, 1/2/3/⏎ answers, V dictates into the field, P attaches a screenshot and the desk's
   transcript shows the `[screen …]` card (`UI/components/chat/ScreenContextCard.tsx`).
5. Island: start a meeting from the main window; the native island appears, the Tauri island does not (`companion-alive`
   gate); drag it, quit and relaunch the companion → same place; drag it near the notch → snap-back; unplug/plug a display
   (or change resolution) → re-placed, no off-screen frame; a topic tap shows a brief; PTT ``⌥` `` says "listening…" and the
   spoken request becomes a brief; the no-notch screen gets the 300 × menu-bar pill.
6. Point-and-say: ⌃S in another app → trail on that display, live words, release → the desk's reply in the answer card;
   ⌃S during a meeting → the "Meeting running" hint; ⌃S off the hub → the refusal; Tauri's own ⌃S binding is gone while
   the companion runs (`voice_key_bind("")` logged) and returns when it quits.
7. Windows (the builder's rounds 1–6, 7.2): Core/Rpc tests on the Mac → Debug build 0/0 on build-B → live UIA/screenshot
   round by a verification-only subagent → package (~15 min companion-only, ~60 min after engine changes) → install /S
   over the running app → packaged recheck by a fresh subagent; seeded attention items until a model credential exists.

### Gates per wave (DONE shape in §11)
Swift tests green (`test.sh`), `swift build -c release` through `scripts/bundle-companion.sh` under the build lock, every
probe's PNG attached to the as-built, the live checks that apply ticked with what was seen, `cargo test -p roster` and
`just ts-desktop` for W0, `vitest` + `tsc` for the UI gates, no new permission dialog in any automated run.

## 11. Waves

Branch `integrate/149` off main `dd19bf63` (worktree `~/Develop/theuth-149-int`, this document on it). C is one commit
on that branch; W0–W3 are worktrees off C's sha; each merges back into `integrate/149` the moment it is DONE; a part
lands on main only feature-complete (D24). Machine rules for every worker brief, verbatim from the standing build
protocol: one `swift build`/`cargo` at a time per session under the shared build lock, `CARGO_BUILD_JOBS` capped, a
separate target directory per worktree, no benchmark or model-loading test while Sky is at the keyboard, never kill a
process you do not own.

### C — contract (one omp worker, ≤ 60 min)
- **Owns:** `COMP/TheuthKit/Models.swift` (+ the new shapes, §5.1 row 1, all `Codable`, optional where the engine may be
  older), `COMP/TheuthKit/RpcClient.swift` (+ every new call, bodies real since the RPCs exist), `COMP/CompanionCore/
  KeyBindings.swift` (the §8 rows, per-platform default table), `COMP/CompanionCore/{RailPillsModel,AgentsModel,
  CardAnatomy,MeetingReducer,IslandModel,PointMachine,OverlayModel}.swift` as **compiling stubs with their test files
  present and `@Test` names listed, bodies `fatalError("W1/W2/W3")` where not trivial** — the shared seams every wave
  fills; `Tests/TheuthKitTests/Fixtures/*149*.json`; `crates/desktop/companion-assets/harness/*.svg` + PNGs and
  `scripts/render-harness-icons.swift`; `scripts/bundle-companion.sh` (copy the icons after `:37-38`); the Tauri gates
  `UI/App.tsx:384-398`, `UI/components/meeting/Island.tsx:104-123, 165-168`, `UI/routes/island.tsx:19-37`,
  `UI/lib/meeting-hotkeys.ts:16-33`; `UI/lib/shortcuts.ts:52` per-platform default; `DESK/lib.rs:524-551`
  `theuth://meetings`; E6's Rust skeleton in `attention.rs` (`ItemContext`, the field with `#[serde(default)]`, `Inputs.
  session_context` empty, bindings exported) so W0 only fills `gather()`.
- **Shared-file insertion points it reserves (one line each, owned by the wave named):** `COMP/Companion/AppDelegate.swift`
  — after `:33` (hotkeys) `agentsPanel.installHotkeys()` (W1), `pointAndSay.install()` (W2), `island.install()` (W3); in
  the menu block `:126-136` `agentsPanel.addMenuItem(to:)` (W1); `COMP/Companion/Store.swift` — `listSessions` +
  `sessionsStatus` + `meshStatus` join the 250 ms fetch set at `:101-127` (C does it, W1 reads it), `meeting` slice
  (W3) and `point` slice (W2) as separate `@Published` properties; `COMP/Companion/Settings/SettingsWindow.swift:57-61` —
  two `tabItem`s `island` (W3) and `voiceKey` (W2) and the Rail tab's "pills" switch (W1); `COMP/CompanionCore/
  KeyBindings.swift` rows are C's, intents are wired by the owning wave.
- **Gate:** `test.sh` green with the stubs' tests marked `.disabled("W1")` etc., `swift build -c release`, `vitest` +
  `tsc` for the UI gates, `cargo test -p roster`, `just ts-desktop` clean; the builder gets the branch name and sha the
  moment C is pushed (hand-off H1).
- **DONE shape:** `DONE 149-C <sha> · models N · rpc calls N · key rows N · gates: tests/build/vitest/tsc/roster · icons 6×4`.

### W0 — engine (one omp worker; parallel with C's tail)
- **Owns:** `attention.rs` (`gather()` `:941` fills `session_context`; `derive` `:315` copies it; the `:1707` test case),
  the agent-store read it needs (`crates/agent`, read-only accessor), `just ts-desktop` output; OPTIONAL E7 in `api.rs` +
  `crates/agent` behind a flag, with its own test or an explicit "not built" line.
- **Gate:** `cargo test -p roster -p agent`, `cargo clippy -p roster`, bindings diff shows only `context` (+ E7 types).
- **DONE shape:** `DONE 149-W0 <sha> · E6 filled for kinds […] · E7 built|deferred · tests N`.

### W1 — One-parity (one omp worker)
- **Owns:** `COMP/Companion/RailPills.swift` (new) + the `RailView`/`frame(of:)` parts of `RailPanel.swift:118-232`,
  `COMP/Companion/AgentsPanel.swift` (new), `COMP/Companion/PanelView.swift`, `COMP/Companion/AskBar/AskBarView.swift`
  (capsule, level bars) + `AskBarPanel.swift:300, 366-391` (`GlassPanel.make(radius:)`), `COMP/CompanionCore/
  {RailPillsModel,AgentsModel,CardAnatomy}.swift` bodies + tests, `CardModel.swift`, `PanelModel.swift` (rank order),
  `Palette.swift` (`Type`, `.darkAqua`), the probes `--probe-rail/--probe-agents/--probe-panel`.
- **Gate:** tests green incl. the un-disabled W1 tests; probes' PNGs; live checks 1–4.
- **DONE shape:** `DONE 149-W1 <sha> · pills · agents panel (mic, ✕, hosts) · card anatomy · empty state · level bars ·
  tests N · probes 3 · live 1–4 seen`.

### W2 — point-and-say native (one omp worker)
- **Owns:** `COMP/Companion/PointAndSay/{PointController,OverlayWindows,PointerSampler,AnswerCard}.swift` (new),
  `COMP/CompanionCore/{PointMachine,OverlayModel}.swift` bodies + the 17 ported tests, the island's `VoiceMode` view
  (`COMP/Companion/Island/VoiceModeView.swift`, new — W3 owns the island window; W2 renders inside the stub window C
  provides, a plain `KeyPanel` at the notch default until W3 merges), `AskRpc.swift` (`screenContext`, `sendInput(files:)`),
  Settings › Voice key tab, `--probe-point`.
- **Gate:** tests; `--probe-point` with the synthetic tone and the no-malachite-pixels assertion; live check 6.
- **DONE shape:** `DONE 149-W2 <sha> · ⌃S native · overlays N displays · machine tests 17 · answer card · Tauri ⌃S stands
  down (seen) · live 6 seen`.

### W3 — island native (one omp worker)
- **Owns:** `COMP/Companion/Island/{IslandWindow,IslandPanel,IslandViews}.swift` (new), `COMP/CompanionCore/
  {MeetingReducer,IslandModel}.swift` bodies + tests, `RpcClient.swift`'s `meeting*` wrappers' use, Settings › Island
  tab, the PTT/eye global rows' wiring, `--probe-island`, `--probe-eye`.
- **Gate:** tests; the four island probes; live check 5 incl. the no-notch pill and the display-change re-place.
- **DONE shape:** `DONE 149-W3 <sha> · island closed/open/detached · drag/snap-back/UUID memory · meeting flows (tap,
  deeper, request, PTT, pause/stop, cursor) · eye best-effort result: <seen> · live 5 seen`.

### Closer (the orchestrator or one omp worker)
Merges W1–W3 into `integrate/149` (keep-both on `AppDelegate.swift`/`Store.swift`/`SettingsWindow.swift` lines — each wave
adds its own line at the reserved point), runs the full gate once, writes `docs/design/149-as-built-{C,W0,W1,W2,W3}.md`
from the DONE messages + screenshots, appends the Revision section here with the orchestrator's rulings on §12, lands each
feature-complete part on main with `merge --no-ff`, pushes, records the knowledge entry, leaves the engine + desktop
rebuild for the morning.

### The Windows waves (the builder's plan 1–4, mirrored) and the hand-off points
| Windows wave | Mirrors | Starts at | Hand-off |
|---|---|---|---|
| WW1 Rpc/Core models + the new calls (Mac-testable) | C | **H1** = C pushed (branch + sha sent to session 01a11782) | the Swift fixture JSONs are copied, not re-derived |
| WW2 WinUI surfaces: pills, agents panel by host, card anatomy, empty state, Ask pill | W1 | H1 | E6 arrives with **H2** = W0 merged (a Windows `t.exe` build or the 2.13.0 package carries it); until then the interim join |
| WW3 Native island + point-and-say: overlays (D2D), capture via the engine, UIA line, voice key Ctrl+Alt+Space | W2 + W3 | H1; the shortcuts default and the UI gates are already in C | **H3** = the Mac W2/W3 DONE messages (constants, refusal hints, visibility rule) forwarded verbatim |
| WW4 Live verification on build-B, package, packaged recheck | closer | after WW1–WW3 | the builder lands the Windows part on main itself and announces to the active roots; the Mac closer does not wait for it |

## 12. Risks, open questions, inconsistencies named

### 12.1 Inconsistencies between the rulings/brief, the recon reports and the mock — named, not resolved silently
1. **Rail geometry vs 130 R2.** The brief fixes 52 pt pills with 12 pt gaps (D1); 130 D4/R2 is Sky's own ruling for ONE
   26 pt strip with two zones, and recon C C1.1 proposed capsules with ~6 pt gaps as "a visual re-cut of the same zones".
   This design follows the brief because the F1–F3 frames are Sky's 00:39 target; R2 is treated as superseded. Sky to
   confirm (the Settings › Rail "pills vs one strip" switch, §3.8, keeps the old look reachable).
2. **"Stop agent X" on the finished card.** Mock screen 3 (F4) shows "Stop agent X" on a *Finished* card; the brief's key
   decision and recon C do-not-copy 5 say never on finished cards (a Finished item is a clean `end_turn`,
   `attention.rs:70-72`). The design follows the brief/recon (D5, §3.3); the mock needs a round 2.
3. **"Clear E" on the ask card.** Mock screen 4 (F5) shows "Clear E" on an ask card; 130 §4.2 says E clears a finished
   receipt only, the brief says "E = clear, never approves", recon C C3.10 says E on a needs-you card could only mean
   snooze/dismiss. The design shows "S snooze" there and E only when `clearable` (`Store.swift:179-181`); mock round 2.
4. **Dot colours.** The brief's "green = idle/done" merges 130 §4.1's idle (hollow faience) and finished (malachite +
   check) into one colour; recon C C7.4 maps One's green to malachite (done) AND faience (working/idle). The design keeps
   both shapes and uses malachite only for done, hollow faience for idle (§3.1) — i.e. "green" means done. Needs a nod.
5. **E6 field names and where it is filled.** Brief: `context { request, last_agent_text }`; recon C: `context { request,
   reply }` "filled in `derive` from the agent store (`question_get`)". `derive` is pure over `Inputs`
   (`attention.rs:313-315`), so the design gathers the texts in `gather()` and uses the brief's names (§6). Recon C also
   cited `durability.rs:1025-1034` for `question_get`; no such file exists in this tree (`api.rs:2517` calls it).
6. **Status-line suffix.** Mock screen 3: "Finished just now · omp" (the harness key); recon C C3.2 and One's "· web": the
   project. Design: project first (`AgentInfo.project`, then `cwd`'s last component), the harness as the icon, the harness
   key as text only when no project is known (§3.3).
7. **The glance sentence.** The mock's header "Inbox · esc" drops "3 need you · 5 working · 2 done"; 130 D13 and recon C
   do-not-copy 8 keep it. Design: a muted sub-line under the header whenever ≥ 1 item exists (§3.3, §3.4).
8. **Empty-state copy.** The brief offers One's verbatim "Suspiciously quiet. Questions and finished work land here." and a
   theuth variant; recon C C4 quotes One's. Design picks ours (D7) — a copy call Sky may reverse in one line.
9. **The eye's source of truth.** Recon B B5 row 4: "design decision needed" — the companion applies `sharingType` to its
   panels while Tauri's `set_hidden_from_capture` covers the webview windows (`DESK/island/macos.rs:64-71`). The brief lists
   the eye under Settings and the island but does not decide. Design: two independent eyes in v0, both default hidden
   (§3.6); an engine-side shared setting is a follow-up.
10. **The spoken reply disappears with the move.** Today the Tauri VoicePanel speaks the desk's reply (`voice-key.ts:358,
    389-396`); the brief puts the spoken reply out of scope (D14) and recon B B3.2 #7 names it an open question. Moving ⌃S
    into the companion therefore LOSES the spoken answer until an engine `speak` device sink or a local synthesiser exists —
    a regression against 96 §0.12 that Sky should see in the morning summary.
11. **Which microphone the voice key uses.** 96 §0.7 and recon B: the ENGINE's mic (`dictation_start(null)`,
    `voice-key.ts:295-318`); 130 R2-2 and the companion: its own PCM (`dictation_start(16000)`). Design: the companion's
    own (D17) — the same lease rules apply (`"talking"` during a meeting, `crates/voice/src/dictation.rs:221-226`), and the
    relay-engine nuance of `voice-key.ts` (`mic = active !== LOCAL`) is moot because the companion speaks to the local
    engine only (130 D2).
12. **Island window level.** Recon A §A5 recommends `.statusBar + 1` (26) with its own caveat (c) that it may sit under a
    full-screen app's auto-shown menu bar; the Tauri island is 25, notchprompt 1000. Design adopts 26 (D9) with a W3 live
    check and `.screenSaver` only as a measured fallback.
13. **"Pending brief/topic" visibility.** The brief's island visibility rule names "a brief/topic pending"; topics and
    briefs only exist as events of a live meeting (`UI/store/meeting.ts:156-220`), and the Tauri island hides on
    Done/Failed (`Island.tsx:70`). Design reads the rule as "≤ 10 min after the last brief/topic even if the meeting
    stopped" (D10) — an interpretation, flagged.
14. **New island keys on E.** Mock screen 6 and §8 give the island scope `E = dismiss topic` (`meeting_dismiss` bars the
    label for the meeting) while the panel scope's E "never cancels anything". Different scopes are allowed by 130 R2-4,
    but the same letter doing something irreversible in one surface deserves Sky's eye; the alternative is × only.
15. **`sessions_status` is not per-session.** The brief's agents-panel data list names `sessions_status`; it is this
    engine's replication summary ("sessions owned here / elsewhere", `api.rs:547-550`). The per-row owner comes from
    `SessionSummary.owner` (`crates/agent/src/lib.rs:2292-2295`); `sessions_status` only labels the groups (§3.2).
16. **Recon line drift.** Recon C/B carried `api.rs` lines from design 130's tree (`desk_open 1680-1682`, `subscribe_events
    1865-1881`, `list_activity 488-491` — that range is `list_agents` here) and `AskRpc.swift:7, :14` cite `api.rs:207-215`
    / `:1692-1694`; in `dd19bf63` they are `2083`, `2272-2277`, `246-247`, `218-226`. The contract uses this document's
    lines; the two Swift doc comments get corrected in C.
17. **130 §4.3's "Point at it stays in the Tauri process"** (recon C §1 lists it as a binding ruling) is superseded by Sky's
    00:39 words; recorded as an amendment in §1.3, not re-argued.
18. **Mock vs brief on the reply-field cap.** F4/F5 show a `space` cap on the field; the mock shows `R`. Design: both work
    (space = alias of R, D5); the cap reads `R` because `space` typed into a focused field must insert a space.
19. **The builder's fork "shell default in my wave or the contract?"** — resolved by the parent at 01:04: the contract (D16);
    recorded here because the builder's verbatim text (§7.3 item 2) still asks.

### 12.2 Risks
- **R1 Two registrations of one chord during hand-over.** Carbon fires both (knowledge: ordinary registrations are
  invisible to the collision probe). D15/D22: the companion owns the chord whenever it runs; Tauri binds only when
  `companion_alive()` is false at boot and on the alive→false edge. Live check 6 proves it.
- **R2 Non-activating panel + SwiftUI text input.** notchprompt needed the `sendEvent` trick and theuth `island_resign_key`
  (recon A §A5 risk b). W3 prototypes the request box first; if focus is unreliable, the request box opens the Ask bar's
  `KeyPanel` instead.
- **R3 Frames across arrangement changes** (recon A risk e): stored screen-relative per UUID and validated; a rejected
  frame falls back to the default and is logged.
- **R4 Unattended overnight.** Approval prompts to Sky go unanswered: worker briefs forbid real sends, mic recording and
  permission dialogs in automated runs; live checks that need a human (TCC prompts, a real meeting) are left for the
  morning and said so in the as-builts.
- **R5 Sky's engine is the shipped 2.13.0 binary** — E6 is only visible after the morning rebuild; W1 ships on the interim
  join and the as-built shows both.
- **R6 build-B has one 1024 × 768 virtual display** — Windows multi-display island/overlay behaviour is built unverified.
- **R7 The mock is round 1** and unapproved; items 2, 3, 6, 7 above change it. Round 2 is owed with the as-builts.
- **R8 `sharingType` best effort** — the eye may not hide the island from a browser share on this macOS; `--probe-eye`
  reports what it did.

### 12.3 Open questions (for Sky, none blocking tonight)
- Q1 One shared eye (engine-side setting) or two? · Q2 Spoken reply path: engine `speak` device sink (96 §0.12) or a local
  synthesiser? · Q3 E7 — build the mesh aggregate or accept the honest hollow dot? · Q4 Keep the "one strip" switch past a
  week? · Q5 The idle island pill's content when "Always show" is on (today: the wordmark + click → `theuth://meetings`).
  · Q6 Does the Windows part wait for the Mac closer or land first (the builder asked; D24 says independent).

## 13. Sources

- Sky's words and rulings: knowledge `decisions/theuth-design-149-companion-native-rulings-2026-10-09`; the five frames
  (getone.one promo, @ky__zo video) as described there; the parent's 01:04 note (contract owns the voice-key default and
  the UI gates).
- Recon A `/tmp/149/recon-A-notch.md` (notchprompt `bbe1e19a` MIT; Apple docs read 2026-10-09 — `NSScreen.safeAreaInsets`
  / `auxiliaryTop*Area` macOS 12+, window levels from `CGWindowLevel.h`, `performDrag(with:)` 10.11+, `setFrameAutosaveName`
  one frame per name, `NSWindow.SharingType.none` "a legacy constant"; boring.notch GPL-3 techniques only, NotchDrop MIT,
  DynamicNotchKit MIT, Notchmeister BSD-3, NotchNook closed; §A5 recommendation). Recon B `/tmp/149/recon-B-move.md`
  (inventories B1/B2, companion seams B3, TCC B4, gates B5, recommendation B6). Recon C `/tmp/149/recon-C-gap.md` (C1–C8,
  the 28 ranked deltas, the do-not-copy list, U1–U7).
- The Windows builder's answer, verbatim in §7 (`~/Develop/theuth-149-briefs/windows-builder-answer.md`).
- Designs: `docs/design/130-companion.md` (D1–D15, §4.1–4.6, Revisions 1–3) and `130-as-built-{W0,W1,W1S,W2,W3,W6,PARITY}.md`;
  `96-point-and-say.md` (§0.1–0.14) and `96-as-built-{SHELL,UI,DAEMON,FIX-CARD,FIX-PANEL,FIX-SHELL2,FIX-DAEMON2}.md`;
  `134-companion-windows.md` (D1–D8, §4, §5) and `134-as-built-W1..W6.md`; `133-stop-scope.md` (§1, §2, §4);
  `132-attention-rows-and-order.md` (D1–D10) + `132-as-built-{ENG,UI}.md`; `54-dictation.md` (§8 `DictationLevel`);
  `meetings/03-island.md` (§0, §9) and `meetings/05-brief-cache.md`; `130/spike-SB-input-breakpoints.md`,
  `130/spike-SC-entitlements-release.md`.
- Code read in `~/Develop/theuth-149-int` at `dd19bf63`: `COMP/**` (48 Swift files), `WIN/**`, `UI/App.tsx`,
  `UI/components/meeting/*`, `UI/lib/{meeting-settings,meeting-hotkeys,voice-key,shortcuts,hotkeys,tauri,companion,trail,
  screen-context}.ts`, `UI/store/meeting.ts`, `UI/routes/{island,overlay}.tsx`, `DESK/island/*`, `DESK/point/*`,
  `DESK/companion/*`, `DESK/lib.rs`, `DESK/launch.rs`, `crates/desktop/tauri.conf.json`, `crates/desktop/Info.plist`,
  `api.rs`, `attention.rs`, `crates/agent/src/{lib,supervisor}.rs`, `crates/mesh/src/types.rs`, `crates/computer/src/*`,
  `crates/meeting/src/{lib,capture/*}.rs`, `crates/voice/src/dictation.rs`, `scripts/{bundle-companion.sh,package-macos.sh,
  package-windows.ps1}`.
- Knowledge consulted: `facts/macos-companion-spike-sb-sc-findings-2026-10-07`, `facts/macos-carbon-hotkey-collision-detection-limits-2026-10-07`,
  `facts/sky-mac-build-toolchain-state-2026-10-07`, `facts/winui3-unpackaged-companion-spike-findings-build-b-2026-10-07`,
  `procedures/theuth-design-doc-house-style-and-writer-brief-2026-10-04`.

## Revision 1 (2026-10-09 01:35) — the orchestrator's rulings on §12.1 (Sky asleep; standing order "thorough, land when complete")

| # | Ruling | Consequence |
|---|---|---|
| 1 | Separate pills (the frames) — 130 R2's single strip is superseded; "Always a sliver" survives as the collapsed form of the pills. | W1 as designed (D1). |
| 2 | No "Stop agent X" on finished cards — X only on cards of RUNNING sessions (approval · ask · decision · review in flight). | Mock round 2 fixes screen 3; W1 follows §3.3. |
| 3 | Ask cards show "Snooze S", not "Clear E"; E clears finished/info items only. | Mock round 2 fixes screen 4. |
| 4 | Keep idle (hollow faience) and done (malachite + check) distinct. | D as written. |
| 5 | E6 field names `ItemContext { request: Option<String>, last_reply: Option<String> }`, gathered in `gather()` (derive stays pure). | C skeleton + W0 use these names; Windows models mirror them. |
| 6 | Status line = `‹state› · ‹relative time› · ‹project›` with the harness as the icon. | §3.3. |
| 7 | The glance sentence stays as the header's sub-line. | §3.3. |
| 8 | Empty-state copy: **"Nothing needs you."** + **"Suspiciously quiet — questions and finished work land here."** (One's tone, our line). | D7 amended. |
| 9 | Eye: while the companion is alive its setting is authoritative for ITS windows; the Tauri shell keeps applying its own setting to its own windows (both default hidden); one setting later. | v0 as written; Q for Sky. |
| 10 | The spoken reply does NOT regress: `speak` (`api.rs:1467`) and `stop_speaking` (`:1470`) exist — W2 calls `speak(text)` for the desk's reply when the 96 setting "speak the answer" is on (default as 96 shipped), device sink = the engine host. | W2 scope +1 small item; §3.7 amended. |
| 11 | Voice-key mic = the companion's own PCM (`dictation_start(16000)`), same as the Ask bar. | D17. |
| 12 | Island level 26 (statusBar+1) with W3's live check over a full-screen app; fall back to 25 if it misbehaves. | W3. |
| 13 | Island visibility: live meeting, or ≤ 10 min after the last topic/brief, or "Always show". | D as interpreted. |
| 14 | Island-scope `E` = dismiss topic is allowed (scopes are independent); the legend says so. | §8. |
| 15–16 | As the doc states (owner from `SessionSummary.owner`; this doc's line numbers are the contract's). | — |
| 17 | 130 §4.3 "Point at it stays in Tauri" is superseded by Sky's 00:39 words; C appends a one-line Revision 4 note to `docs/design/130-companion.md`. | C. |
| 18 | Reply-field key cap reads `R`; space is the alias. | §8. |
| 19 | Resolved: shortcuts default + UI gates in C (D16). | C. |

Operational rule added for every probe (Sky, 01:17–01:20): the Mac runs clamshell with a full-screen black Preview window on the
external monitors; panel-only probes render above it; desktop-visible probes exit/minimize Preview and restore it in a trap/finally.

## Revision history

- **v0 — 2026-10-09 01:25 (+01:00).** Written by writer-149 from recon A/B/C, the Windows builder's answer (verbatim in
  §7) and Sky's rulings of 00:39–00:55; the parent's 01:04 note folded into D15/D16/§11 C. Every `path:line` read in
  `~/Develop/theuth-149-int` at `dd19bf63`. The orchestrator's rulings on §12 go into a Revision 1 section after this
  one; the as-builts follow the wave names of §11.
- **R1 — 2026-10-09 01:35.** Orchestrator's rulings on the 19 named inconsistencies (table above); spoken reply kept via `speak`;
  empty-state copy fixed; clamshell/Preview probe rule.
