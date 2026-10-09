# 149 W1 — as built (One-parity: rail pills · agents panel by host · card anatomy · empty state · Ask bar level bars)

Branch `feat/149-w1-parity`, worktree `~/Develop/theuth-149-w1`, off the contract `0280c0a7` (`integrate/149`).
Contract: `docs/design/149-companion-native.md` §3.1–3.5, §3.9, §4 D1–D8/D18/D23, §5.4 A/B, §8, §10, §11 W1, Revision 1 rows 1–8, 18;
`docs/design/149-as-built-C.md` (models, RPC calls, stubs, insertion points, icons). `COMP` = `crates/desktop/companion/Sources`.
Swift only; Core bodies + 24 un-disabled tests (+1 new), three new surfaces, three probes. Mock: `docs/design/149/mock-round2.html` screens 1–5.

## What exists (path:line)

### CompanionCore (pure, tested)
- `COMP/CompanionCore/RailPillsModel.swift` — `RailPills.derive(rail:levels:hovered:)` `:51-60`: inbox `count` = `needsYouCount`; agents pill
  `dots` = `rail.marks` (gold needs-you first, ≤ 12) with `overflow` passed through (+ a trim when a hand-built rail exceeds the cap), `count`
  = the whole population; actions pill `levels` clamped 0–1 in order (newest right); `hovered` on one pill only. `subscript(kind)` `:34`,
  `stackHeight 244` `:37`, `sliverLabel` `:40-47` ("Theuth rail, 3 need you, 13 agents" — what the collapsed 4 × 120 capsule says to VoiceOver).
- `COMP/CompanionCore/AgentsModel.swift` — `AgentRow.deskName`/`deskPrefix "agent:"`/`isActive` `:28-39`; `HostGroup.node` `:48` (the peer's
  display name for "on ‹node›"); `AgentsModel.harnessKey` `:60-63` (D18: nil → `theuth`, unknown → `unknown`); `groups(…)` `:68-134`: roots from
  `list_sessions` (non-archived, owner `""`/`self_id` = this Mac, else remote) joined with `list_activity`, then activity-only roots (older
  engine), then asleep desks; children folded as `⑂N` from `activity.children`, else parents counted in either list; desks named
  "‹title› · desk" with id `agent:<name>`; remote rows `live false`, state `.idle`, `last ""` (never a guessed spinner, D2/§6); rows ranked
  running → waiting → idle → error → cancelled, then by name; groups "this Mac · ‹here›" first, peers in `mesh_status` order titled
  "on ‹name›" (+ " · ‹elsewhere›" when one remote group holds them all), unknown owners by id prefix. `firstWorking` `:137-139`.
- `COMP/CompanionCore/CardAnatomy.swift` — `make(…)` `:65-100`: age "just now" < 45 s else `RailModel.ago`; status state per kind ("Waiting
  for your answer", "Finished", …, "Stopped" when `activity.state == .cancelled`, §5.4 B); project chain `AgentInfo.project` → `SessionSummary
  .project` → `cwd` leaf → harness key (Revision 1 row 6); harness = `AgentsModel.harnessKey(activity?.agent ?? session?.agent)`; bubbles from
  E6 `context` else the interim `title` + `last`, ≤ 160 chars `:106-109`; footer verbs `reply/talk` when `replyAction != nil`, `point` with a
  session, `stop` only for ask/approval/blocked/decision/review **with an activity row running or waiting** (`stoppableKinds :103`,
  `isRunningOrWaiting :105`) — never on finished (Revision 1 row 2), `clear` when `clearable`, `snooze` when `needsYou` (row 3), `legend`;
  `ifWrong` from `Card.ifWrong`.
- `COMP/CompanionCore/CardModel.swift` — `CardAction.stopAgent(session:children:)` `:28`, `.replyWithScreen(session:text:files:)` `:30`;
  `Card.ifWrong` `:71` filled from `packet.ifWrong` `:80-82`. `Commit.swift:17-19` `StoreEngine` + `cancel`/`sendInput(files:)`, `:62-64` the two
  cases (`cancel(session, .tree)`; `send_input(session, text, files, nil, nil)`).
- `COMP/CompanionCore/PanelModel.swift:18-23` — order = group (needs-you · finished · receipts) → `rank` desc → `waiting_since` asc → id.
- `COMP/CompanionCore/PolicySettings.swift:18-19, 26, 44, 60` — `railPills` (default true; off = 130's strip).
- `COMP/CompanionCore/AskVoice.swift:145-146` — `talking` → "Meeting running — use the island to ask" (§3.5), beside `already dictating`.
- Tests (all un-disabled, written): `RailPillsModelTests` 5 · `AgentsModelTests` 8 · `CardAnatomyTests` 11 · `PanelModelTests
  .rankOrdersWithinAGroupOldestFirstOnTies` (new, `CompanionCoreTests.swift:120-127`) · `ParityTests.swift:49-53` recorder gains the two calls.

### Companion (AppKit/SwiftUI)
- `COMP/Companion/Palette.swift:98-171` — `Type` (headline 24 · question 17 · body 15 · meta 12.5 · caps 10.5 · mono; radii pill 26 / Ask bar
  28 / card 24 / option 10 / cap 4; `Type.dark = NSAppearance(.darkAqua)`; `malachiteHex`), `LevelBars` (one carnelian bar per band, D8),
  `Flags.probeRail/probeAgents/probePanel/w1Probe`, `W1Log` (`/tmp/149-W1-<probe>.log`).
- `COMP/Companion/RailPills.swift` (new) — `W1Hover.shared` `:8` (the one `HoverSync`), `RailAnchorKey` `:13` (dot/pill frames by `mark.id` /
  `"pill:<kind>"`), `RailPillsView` `:21-134`: four 52 pt capsules 12 pt apart — inbox (`Wordmark` + count, gold only > 0, carnelian 4 pt dot
  when disconnected), agents (`MarkView` per dot; `WorkingArc` `:137` 0.38 s faience arc for `.working`, `MarkView(.working)` under Reduce
  Motion; 1 column ≤ 6 dots, 2 above; `+N`; hover `scaleEffect(1.4)` 120 ms spring; hovered when the agents panel hovers the same session),
  actions (ask glyph · mic → `LevelBars` while `levels` non-empty · point glyph), more (`···`); tap on a dot → `onMark`, on a capsule →
  `onPill(kind, frame)`; the sliver = 4 × 120 capsule labelled `sliverLabel`. `RailProbe` `:152-213`.
- `COMP/Companion/RailPanel.swift:7-197` — `RailController` keeps the panel/monitor/presence plumbing (`:41-77, 116-163`); `init(store:hover:)`;
  `render()` `:85-98` picks `RailPillsView` vs the kept `RailView` strip (`:201+`) by `railPills`, live; re-derives on `$rail` and on
  `$ask.map(\.dictation.levels)` (the Ask bar's bars mirror into the actions pill), re-renders on `$connection`; `layout()` `:166-175` 52 ×
  max(244, fit) in pills mode, 26 × fit in strip mode, 4 × 120 collapsed; `panel.appearance = Type.dark`; `onPill` `:34`; `frame(of:)` `:185-191`
  from the preference anchors (strip-row maths as the fallback), `frame(ofPill:)` `:194`; probe hooks `probeCollapse` `:145`, `sideOverride` `:36`.
- `COMP/Companion/AgentsPanel.swift` (new) — `RowAction` `:11` (`stop/pauseDesk/send`), `AgentsPanelState` `:22` (groups, flat focus, reply
  field, `UndoQueue<RowAction>`, pill, receipt, scroll target), `AgentsPanelController` `:64-411`: `GlassPanel.make(width: 372, radius: 24)`,
  `installHotkeys()` `:121` (⌃⇧A toggles at the mouse's screen), `addMenuItem(to:)` `:129` ("Agents…" after "Ask theuth…"), `open(anchor:
  scrollTo:)` `:145` (scrolls to the session or `firstWorking`), `showWithoutKey` `:165`, regroup one run-loop turn after `objectWillChange`
  (`:204-215`, the last list kept while disconnected), hold/tick/commit `:261-331` mirroring `Store.hold` (pill texts "Stopping ‹name› and N
  children · ⎋ undo" / "Pausing ‹desk› · ⎋ undo" / "sending to ‹name› · ⎋ undo"; commit → `cancel(session, .tree)` / `agent_pause(name, true)` /
  `send_input(session, text, [], nil, nil)`; receipts "stopped · ‹name›" (never "undo" after commit, D3), "Failed · ‹error›"), ⎋ `:334-344`
  (undo → "Taken back"; else cancel a hold-to-talk; else drop the field; else close), keys `:349-371` via `KeyTable.action(for:in: .agents)`
  (J/K/↓/↑, ⏎/⌘O deep link `theuth://c/<session>`, V key-down/up hold-to-talk, X hold, ⎋), placement `:375-411` (= `PanelController.origin`).
  Views `:416-643`: header "Agents" · J K · ✕ esc; caps host titles; rows = state dot (`ArcSpinner` for running, gold `MarkView(.needsYou)`
  waiting, hollow idle, carnelian error; **remote = hollow sand whatever the state**) · `HarnessIcon` · name (never the harness key, layout
  priority) · `⑂N` · "on ‹node›" (remote) · muted `last` · mic · ✕; focused row faience bar + tint, hovered row sets `W1Hover.shared.session`;
  remote rows' mic/✕ disabled with "open it on ‹node›", idle local rows' ✕ "nothing is running"; reply field under a row with "⏎ send · ⎋ keeps
  editing"; `HoldPill` with the countdown bar; empty copy; "Connecting to the engine…" dims the last list. `AgentsProbe` `:645-708`.
- `COMP/Companion/Voice/HoldToTalk.swift` (new) — `HoldToTalk` `:10-171`: `AskVoiceMachine` + `VoiceRecorder` + the engine's dictation queue
  (`dictation_start(16000)` → `dictation_audio` in order → `dictation_stop`, the Ask bar's path) observing `store.$ask.dictation`; `press(target:)`
  / `release()` / `cancel()`; `recording · finishing · status · liveText · partial · levels`; `onText(trimmed)`; guarded by `AskFlags.automated`.
  `HoverSync` `:176-180`.
- `COMP/Companion/PanelView.swift` (re-cut) — `CardState` `:9` (reply text, P's attachment, V's `HoldToTalk`, `onPoint/onStop`), `Store.anatomy
  (for:)` `:30`, `PanelView` `:45-104` (header · card · other items as lines · Done zone · footer · receipt), `EmptyState` `:106` ("Inbox" · ✕ esc,
  `Wordmark` ×2, "Nothing needs you." 24 pt / "Connecting to the engine…", "Suspiciously quiet — questions and finished work land here."),
  `CardHeader` `:120` (name · `PositionDots` `:146` · ‹ J · K › · ? · ✕ esc as plain buttons → `store.perform`; the glance sentence as the
  sub-line), `HarnessIcon` `:218` (`Contents/Resources/harness/<key>-20.png` template tinted ink, `unknown` fallback, initial letter outside a
  bundle), `CardView` `:251-317` (status line · bubbles `BubbleView :320` · question 17 pt · why · "If wrong: …" · chips · "Next steps" caps
  header on finished cards · `OptionRow :340` with key cap, "(Recommended)", "→" · `ReplyRow :367` [▣ P] [field "Reply to ‹name›" / "Type your
  answer…" + `R` cap] [🎙 V hold → `LevelBars` + live text] [↑ ⏎] + "screen attached · N" chip + voice status · `FooterPill :448` Stop agent X
  (carnelian) / Clear E / Snooze S from `anatomy.footer`), `ReceiptView` `:519-568` renders a held `.stopAgent` as "Stopping ‹who› and N children"
  in carnelian.
- `COMP/Companion/PanelProbe.swift` (new) — `PanelController.handleW1` `:11-36` (`.talk` key-down press / key-up release, `.point`, `.stopAgent`;
  a focused field types, except V's key-up), `stopAgent()` `:40-45` (only when the anatomy offers `.stop`; `Held(.stopAgent(session, children))`
  through the Store's hold → `cancel(session, tree)`), `pointAndAttach()` `:49-72` (close → one run-loop turn → `screen_context` with a one-point
  path at the pointer, CG top-left y, `color = malachiteHex` → reopen with the attachment and the field focused; failure → "Failed · screen: …"),
  `Store.submitReplyWithScreen` `:78-85` (`[screen] ‹line›` appended, `files` attached → `.replyWithScreen`), `PanelProbe` `:92-195`.
- `COMP/Companion/AttentionPanel.swift` — `store` internal + `w1: CardState` `:27-29`; `panel.appearance = Type.dark` `:39`; radius 24 `:62, 74`;
  `[.keyDown, .keyUp]` monitor `:118`; `handle` `:152-162` table lookup → `handleW1` before `KeyGrammar`; `close` cancels the hold + attachment.
- `COMP/Companion/AskBar/AskBarPanel.swift:34, 364-396` — `GlassPanel.make(width:height:radius:)` (default 12) with `.darkAqua`; the Ask bar at 28.
  `AskBarView.swift:34-76` — 24 pt line (finals ink, partial sand), `LevelBars` in place of the mic while recording, one-line hint row
  "hold ⌃⇧Space to talk · release → sends in 3 s · ⎋ keeps editing · theuth routes it".
- `COMP/Companion/W1Install.swift` (new) — `AppDelegate.installW1(rail:attention:)`: creates `AgentsPanelController.shared` + hotkey; `rail.onMark`
  (item marks → the card beside the mark, agent dots → the agents panel scrolled to the row, D23); `rail.onPill` (inbox → Attention panel with
  the oldest needs-you focused · agents → agents panel · actions → Ask bar · more → `statusMenu()` popped at the pill); starts the probes.
- Shared-file edits (one line each, at the reserved points): `AppDelegate.swift:36` `installW1(rail:attention:)`; `:29` `Hotkeys.start` skipped
  under `Flags.anyProbe` (see Deviations); `:125-151` the status menu extracted to `statusMenu()` so the "more" pill can pop it, with the W1 line
  `AgentsPanelController.shared?.addMenuItem(to:)` at the reserved point; `Store.swift:122` `refetch` no-op once seeded, `:303-318` `w1Seed(…)`;
  `SettingsWindow.swift:89` "Separate pills" toggle; `Settings/Hotkeys.swift:11-12, 22-24` `KeyboardShortcuts.Name.openAgents` + `names` row;
  `Settings/SettingsStore.swift:66` `anyProbe` includes `w1Probe`; `main.swift:3-6` the single-instance guard skipped for probes.

## Gates
- `BUILD_LABEL=149-W1 build-lock bash crates/desktop/companion/test.sh` ✔ **177 tests in 28 suites passed**, 50 skipped (W2/W3's stubs:
  PointMachine 16 · MeetingReducer 21 · IslandModel 8 · OverlayModel 5) — `/tmp/149-W1-test-final.log`. W1 tests: 24 un-disabled + 1 new = 25.
- `BUILD_LABEL=149-W1 build-lock swift build -c release --package-path crates/desktop/companion` ✔ (`/tmp/149-W1-build1.log`, 165 s, first try).
- `bash scripts/bundle-companion.sh` ✔ → `target/release/companion/Theuth Companion.app` (2.13.0), `codesign --verify --deep --strict` OK,
  `Contents/Resources/harness/*.png` present (`/tmp/149-W1-bundle.log`).
- No permission dialog in any run: no mic (`AskFlags.automated`), no notification authorization, no Accessibility.

## Probes (every one `--shareable`, run from the in-bundle executable beside Sky's running login item; PNGs copied to `docs/design/149/w1/`)
Sky's clamshell + full-screen black Preview: the panels sit at `.statusBar` above it, so panel-only `screencapture -R` needed no change —
confirmed by the first screenshot (black background around the pills). Capture rects came from the probes' logged frames (AppKit bottom-left →
CG top-left with the 2560 × 1440 main display) and the CGWindowList bounds each probe logs before quitting.
- `--probe-rail 12` → `/tmp/149-W1-rail.log`: `phase=pills frame=(2508, 560, 52, 320)` (right, expanded, needsYou=3 marks=10 levels=6),
  `phase=sliver frame=(2556, 660, 4, 120)`, `phase=pills-left frame=(0, 560, 52, 320)`; never key, `ignoresMouse=true`, `active=false`.
  PNGs: `149-W1-rail.png` (inbox gold 3 · three gold needs-you rings + 3 faience arcs + hollow idle + carnelian error + dashed lapsed · level bars
  · ···), `149-W1-rail-sliver.png`, `149-W1-rail-left.png`.
- `--probe-agents 7` → `/tmp/149-W1-agents.log`: `groups=["this Mac:4", "on studio:1"] remote=["134-W7 WinUI rail"] pill="Stopping w3-149-island
  and 2 children · ⎋ undo" replyFor=s-1`, frame 372 × 383 at (1094, 745). PNG `149-W1-agents.png`: "this Mac" (theuth · desk working, w1 with the
  reply field "ship the pills first", w3 `⑂2`, docs-149 writer idle) · "on studio" (hollow sand row, "on studio", mic/✕ disabled) · the hold pill.
- `--probe-panel finished|ask|approval|review|empty` → `/tmp/149-W1-panel.log` (frames 372 × 516/484/443/518/228 at x 1094, `footer=[…]` per
  kind, `key=false`). PNGs `149-W1-panel-{finished,ask,approval,review,empty}.png`: finished = header dots · "Finished · just now · theuth" ·
  bubbles · "Next steps" with "(Recommended)"/→ · reply row · **Clear E, no Stop**; ask = bubbles · question · 3 options · reply row "Type your
  answer…" · **Stop agent X · Snooze S, no Clear**; review = "If wrong: …", chips, Stop + Snooze; empty = the Revision 1 row 8 copy.
- Bonus `--ask-probe 6 --shareable` (W3's probe, no dictation): `149-W1-askbar.png` — the 28 pt capsule, 24 pt line, one-line hint row.

## Live checks 1–4 (orchestrator; exact steps)
Prerequisite: the login-item companion holds the single-instance slot — quit it (status item ▸ right-click ▸ "Quit theuth Companion"), then
`open -g "~/Develop/theuth-149-w1/target/release/companion/Theuth Companion.app"`; relaunch the login item afterwards (`open -g
"/Applications/theuth.app/Contents/Library/LoginItems/Theuth Companion.app"`). Settings › Rail › "Separate pills" must be on (default).
1. Rail pills: the inbox count = the menu bar `●N`; hover an agent dot → it grows 1.4× and, with the agents panel open (⌃⇧A or click the agents
   pill), its row highlights; hover a row → the dot grows. The panel lists this Mac's sessions by `title`/`name` with `⑂N` from `list_activity`;
   a remote-owned session (if the mesh has one) sits under "on ‹node›" hollow, mic/✕ disabled with the tooltip "open it on ‹node›".
2. Per-row mic: `new_session` a scratch session (`theuth` CLI or the main window), find its row, hold the row's mic (or focus it with J/K and hold V)
   ≥ 0.25 s, speak, release → the text appears in the field under the row with "sending to ‹name› · ⎋ undo" for 3 s → "sent · ‹name›"; the
   session's transcript shows it as a user message; the row's dot becomes the spinner on `TurnStarted`.
3. Per-row ✕ mid-turn on that scratch session: the pill "Stopping ‹name› and N children · ⎋ undo"; press ⎋ once → "Taken back"; press ✕ again and
   let the 3 s elapse → `cancel(session, tree)` → the stream shows `TurnFinished { stop_reason: "cancelled" }`, the row's dot goes hollow, the
   receipt reads "stopped · ‹name›" (no undo offered).
4. Seeded ask item (an `ask_user` from any session): ⌃⇧Space → the F5 card (header dots · status line · bubbles · question · options with
   "(Recommended)" · reply row · Stop agent X · Snooze S); 1/2/3/⏎ answer after the hold; V held dictates into the field (level bars in place of the
   mic); P closes the panel, captures under the pointer and reopens with "screen attached · N" — ⏎ sends `send_input(session, text + [screen]
   lines, files)` and the desk's transcript shows the `ScreenContextCard`.

## Deviations / open items
- **Agents pill height.** 12 dots of 10 pt cannot fit a 52 pt capsule; the agents capsule has `minHeight 52` and grows with its dot grid (the
  other three stay 52), so the stack is 52 × 320 with ten dots instead of the nominal 244. Sky's nod wanted (§12.1 item 1 geometry).
- **Children are folded only.** `⑂N` is shown; "click the count to expand" (§3.2) is not wired — rows are roots and desks; the children count comes
  from `list_activity`/`list_sessions` parents.
- **Status-line "session gone".** `CardAnatomy.make` cannot tell "no row yet" from "vanished" with the contract's signature; not shown. "Stopped"
  after a cancel is shown (`activity.state == .cancelled`).
- **P in the Ask bar** (§8 `askPoint`, §3.5 last line) is not wired: the brief scopes W1's Ask-bar edits to the capsule, the type and the level
  bars; `KeyAction.askPoint` stays data. The card's P is complete.
- **Hotkeys.start skipped under any probe** (`AppDelegate.swift:29`) and the single-instance guard lifted for probes (`main.swift:5`): with Sky's
  login item running, a probe instance shares its UserDefaults suite and the exclusive-registration probe would have read his own ⌃⇧Space as "held by
  another app" and persisted `nil` for it. Probes register nothing and persist nothing; the probe's own status item shows for N seconds.
- **`refetch` is a no-op once `w1Seed` ran** (`Store.swift:122`): the first dry run showed the real engine's 24 needs-you overwriting the fixture
  between the subscribe and `onRefetched`; freezing the fetch is the honest fix (probes read the engine's event stream, never its lists).
- **Reply field focus ring.** The always-visible field draws macOS's focus ring in the screenshot even though the probe panel is not key; cosmetic.
- `DoneZone`/`AskItemLines`/W3's `ReplyField` are untouched; the card's reply row supersedes `ReplyField` (kept for W3's `replyFor` plumbing).
- Rust/UI untouched; `AskBarPanel.swift` beyond the two reserved spots untouched; `Legend` lists the new scopes from the table unchanged.
