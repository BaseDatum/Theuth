# 149 C — as built (the contract: shared Swift seams, Tauri gates, voice-key default, icon set)

Branch `integrate/149`, worktree `~/Develop/theuth-149-int`, off `77fa225f` (origin/main `dd19bf63` + the design doc).
Contract: `docs/design/149-companion-native.md` §5.1/§5.2/§6/§8/§11-C + Revision 1 (rows 5, 8, 10, 17, 19).
`COMP` = `crates/desktop/companion/Sources`, `UI` = `crates/desktop/ui/src`, `DESK` = `crates/desktop/src`.

## What exists (path:line)

### TheuthKit — models (`COMP/TheuthKit/Models.swift`), 28 new types + 4 new fields
- `AttentionItem.context: ItemContext?` · `priority: Priority?` · `rank: UInt32` (`:63-67`, decode `:98-100`; absent `rank` = 5,
  the engine's value for an item without a priority). `ItemContext { request, lastReply }` `:109-113` (Revision 1 row 5),
  `Priority` `:116`.
- `Packet.ifWrong: String?` `:400-401`, decode `:429`.
- Sessions: `SessionSummary` `:497` (`id title name agent parent cwd project desk updatedAt running owner archived`; every
  `#[serde(default)]` field tolerated absent), `SessionsStatus` `:537`. `MeshStatus` is NOT redeclared — it already lives in
  `COMP/CompanionCore/AskRouter.swift:5` with `RpcClient.meshStatus()` in `AskRpc.swift:12` (reused; the first attempt to add
  one was ambiguous and was deleted).
- Agent control: `CancelScope` `:548` (`root`|`tree`), `Delivery` `:551` (`steer`|`next_turn`).
- Point-and-say: `PointerSample` `:555` (`tMs x y pen`), `ScreenContextReq` `:565`, `ScreenContextReply` `:576`.
- Decision: `DecisionModeKind` `:584`, `ModeInfo` `:586`.
- Meeting (`crates/meeting/src/lib.rs:139-243, 268-376, 415-520, 686-765`): `Track` `:598`, `PostStage` `:600`,
  `MeetingState` `:603` (externally tagged — bare word or `{"Post": stage}`, `.other` for newer words, `isLive`),
  `Degradation` `:635` (`{"SttDown": track}` / `{"PermissionDenied": p}`), `MeetingSupport` `:666` (`.full|.partial|.unsupported`),
  `BriefSource` `:693`, `TopicGone` `:697`, `Topic` `:699`, `Brief` `:711`, `Utterance` `:728`, `Level` `:741`,
  `MeetingProcess` `:749` (the Rust `ProcessInfo`; renamed — Foundation's `ProcessInfo` shadowed it in `Discovery.swift:9`),
  `OracleDecision` `:757` (`batchId`, `family` only), `PermissionState` `:762`, `MeetingPermissions` `:764`,
  `MeetingStatus` `:770` (every phase-2+ field defaulted).
- `MeetingEvent` `:807`: **15 variants** on the wire (the doc's "13" undercounts `Summarized` and the two `Oracle*`), all decoded;
  `MeetingEvent.decode(data) -> MeetingEvent?` `:861` answers `nil` for a non-meeting `type`, so it sits beside `WireEvent.decode`
  on the same bytes (W3 hooks it at `Store.swift:107`). A meeting item missing a field throws (never guessed).

### TheuthKit — RPC calls (`COMP/TheuthKit/RpcClient.swift`), 24 new, positional in `api.rs` order
`sendInput(session:text:files:delivery:id:)` `:262` (api.rs:194-203; the files-less form stays as an overload `:267`) ·
`listSessions` `:276` · `sessionsStatus` `:278` · `cancel(session:scope:)` `:281` (api.rs:255) · `agentPause(name:paused:)` `:286`
(api.rs:2088) · `steer(session:text:id:files:delivery:)` `:291` (api.rs:218-226) · `screenContext(_:)` `:297` (api.rs:335) ·
`speak(session:text:)` `:302` (api.rs:1467 — takes `session` too, not `speak(text)`) · `stopSpeaking(id:)` `:307` ·
`decisionMode(prefix:)` `:312` · `meetingPause/Resume/Stop/Status/Permissions/Tap/Dismiss/Deeper/Request/Ptt/Cursor/Briefs/Topics/Live`
`:315-342` (api.rs:1558-1611). `meshStatus()` pre-existed (`AskRpc.swift:12`).

### Fixtures + tests (`crates/desktop/companion/Tests/TheuthKitTests`)
15 fixtures `Fixtures/*149*.json` derived from the Rust structs (the ts-rs bindings dir is gitignored and absent in a fresh worktree;
regen was queued behind the build lock — shapes were cross-read from `crates/{agent,meeting,computer,decision,sessionsync,roster,tasks}/src`):
`attention_item_149_context` · `attention_item_149_no_context` · `packet_149_if_wrong` · `list_sessions_149` · `sessions_status_149` ·
`meeting_status_149{,_idle,_post}` · `meeting_topics_149` · `meeting_briefs_149` · `meeting_live_149` · `meeting_permissions_149` ·
`meeting_events_149` (16 items: all 15 variants, `Utterance` partial + final) · `screen_context_reply_149` · `decision_mode_149`.
Tests `FixtureTests.swift:147-264`: `attentionItemContextPriorityRank`, `packetIfWrong`, `sessionsAndStatus`, `meetingStatusInEveryShape`
(incl. `MeetingState` round-trip), `meetingLists`, `meetingEventsEveryVariant` (+ nil for `TurnFinished`, throw on a missing field),
`pointAndDecisionShapes` (`ScreenContextReq` encodes snake_case with `pen`; `CancelScope`/`Delivery` positional encoding).

### CompanionCore stubs (`COMP/CompanionCore/`), bodies `fatalError("149 Wn")`
- `RailPillsModel.swift` — `RailPills { pills }`, `Pill { kind count dots overflow levels hovered }`, `height 52`, `gap 12`, `maxDots 12`,
  `derive(rail:levels:hovered:)` (W1).
- `AgentsModel.swift` — `AgentRow { id session name harness state children last host live }`, `HostGroup`, `AgentsModel.groups(…)`,
  `firstWorking` (W1).
- `CardAnatomy.swift` — `Header`, `StatusLine`, `Bubble`, `FooterVerb` (`stop` / `clear` conditional), `ifWrong`, `justNow 45 s`,
  `make(…)` (W1).
- `MeetingReducer.swift` — `MeetingSlice` (the `meeting.ts` slice: topics/tapped/briefs/cursor/unseen/lines/oracle/level…, `.empty`,
  `linesKept 200`, `oracleKept 8`), `MeetingReducer.reduce/seed/elapsed` (W3).
- `IslandModel.swift` — `IslandState { closed, open, detached(frame) }`, `IslandModel { state frames alwaysShow }`, `snapBack 40`,
  `linger 10 min` (Revision 1 row 13), `fallbackPill 300×24`, `visible/dragEnded/frame(for:)` (W3).
- `PointMachine.swift` — `holdMs 200 · doubleTapMs 300 · minNoteMs 800 · sampleHz 60 · doneMs 6000 · hint` as public statics,
  `State/Event/Effect` mirroring `DESK/point/machine.rs:23-69`, `step(_:_:now:refused:)` (W2); `PointSlice` for `Store.point`.
- `OverlayModel.swift` — `OverlayDisplay`, `OverlayPath`, `strokeWidth 6 · ringDiameter 14 · alpha 0.85`, `paths(samples:displays:)` (W2).
- Tests, one file per type under `Tests/CompanionCoreTests/`, every `@Test(.disabled("149 Wn"))` with the names listed:
  `RailPillsModelTests` 5 · `AgentsModelTests` 8 · `CardAnatomyTests` 11 · `MeetingReducerTests` 21 · `IslandModelTests` 8 ·
  `PointMachineTests` 16 disabled (the 16 behavioural tests of `machine.rs:158-467`; its 17th, `state_serialises_tagged_snake_case`, has
  no Swift equivalent) + 1 ENABLED `constantsMatchTheShell` · `OverlayModelTests` 5. 74 disabled in total.

### Key table (`COMP/CompanionCore/KeyBindings.swift`), 23 new rows
- Scopes `agents` · `island` · `pointAndSay` appended (`:96-101`, titles `:108-110`).
- Actions appended (`:121-127`); `intent` (`:133-142`): `replyFocus → .reply` (space = R's alias, Revision 1 row 18), agents J/K/⏎/⌘O
  → next/previous/openSession, every ⎋ → escape, the rest `nil` until the owning wave wires them.
- Rows `:216-244`: anywhere ⌃⇧A `openAgents` · ⌃S `voiceKey` · ⌥` `meetingPtt` · ⌥⇧E `meetingEye`; panel space/V/P/X; askBar P; agents
  J/K/⏎/⌘O/V/X/⎋; island M · ⌃M `islandStop` (the §8 ⌃⇧M — local scopes store chords without ⇧, `Chord.normalizedLocal:37-40`, so ⌃⇧M typed
  resolves to it) · E · D · ⏎ · ⎋; pointAndSay ⎋ (fixed). Aliases `:266-270`: ↓/↑/keypad ⏎ for the agents panel, keypad ⏎ in the island box.
- Per-platform table `:247-263`: `KeyTable.Platform { macOS, windows, linux }`, `platformDefaults[.voiceKey] = { macOS ⌃S, windows/linux
  Ctrl+Alt+Space }`, `defaultChord(for:on:)` falls back to the table.
- Tests `Tests/CompanionCoreTests/W6Tests.swift:103-131`: `rows149UniqueAcrossSixScopesAndSpaceIsReplyInThePanelOnly`, `perPlatformDefaults`;
  the pre-existing uniqueness/round-trip/row-count tests still pass over the six scopes.
- `COMP/Companion/Settings/Hotkeys.swift:18-27`: the launch probe (`canRegister` → fallback + persisted nil) now runs only for rows listed in
  `Hotkeys.names` — otherwise the first companion launch while Tauri still holds ⌃S would have persisted the voice-key row as unassigned.
  W1/W2/W3 add their `KeyboardShortcuts.Name` to `names` when they wire the row.

### Reserved insertion points (comment lines, exactly as the contract names them)
- `COMP/Companion/AppDelegate.swift:36-38` — `// 149 W1: agentsPanel.installHotkeys()` · `// 149 W2: pointAndSay.install()` ·
  `// 149 W3: island.install()` (after the hotkeys block); `:139` — `// 149 W1: agentsPanel.addMenuItem(to: menu)` (menu block).
- `COMP/Companion/Store.swift:41-48` — `@Published sessions / sessionsStatus / meshStatus` (real: joined the 250 ms fetch at `:127-135`,
  each `try?` so an older engine without a verb keeps the last value) · `@Published var meeting: MeetingSlice?` (W3) ·
  `@Published var point: PointSlice?` (W2); `:107` — `// 149 W3: meetingObserve(data)` in `handle`.
- `COMP/Companion/Settings/SettingsWindow.swift:62-63` — the `island` (W3) and `voiceKey` (W2) `tabItem`s; `:89` — the Rail "Separate pills"
  switch (W1).

### Icon set
`crates/desktop/companion-assets/harness/{omp,opencode,claude,codex,theuth,unknown}.svg` — 64×64 viewBox, white 6 pt round strokes on
transparent, a letterform/mark each (o·m·p; `< / >`; a C arc; C + x; a T with a dot; `?`), no trademarked logos. `scripts/render-harness-icons.swift`
rasterises each with `NSImage` → `NSBitmapImageRep` to `<name>-{16,20,24,32}.png` in the same directory; run → 24 PNGs committed (6×4).
`scripts/bundle-companion.sh:39-40` copies the directory to `Contents/Resources/harness/` after the icons lines (single `cp -R`).

### Tauri shell / UI (platform-neutral)
- `UI/lib/companion.ts:30-44` — `useCompanionAlive()`: a zustand boolean fed by `companion_alive` + `companion-alive`, wired on first use per
  webview; `false` off Tauri.
- `UI/lib/shortcuts.ts:52-54` — `'voice-key'` default `container() === 'macos' ? 'Control+S' : 'Control+Alt+Space'`. Deviation: the brief names
  `isMacDesktop()` from `hotkeys.ts:39`, but `hotkeys.ts` imports `shortcuts.ts` and `DEFS` is evaluated at module init — importing it back would
  be a TDZ cycle; `container() === 'macos'` IS that helper's body, inlined with a comment.
- `UI/App.tsx:386-400` (`useVoiceKeyBinding`) — `want = enabled && !companion ? chord : ''`; alive → `voiceKeyBind("")`, dead → re-bind.
- `UI/components/meeting/Island.tsx:70-72` — `shown` is false while alive (hides the window, no `islandShow`, no `meeting_cursor` `:170`).
- `UI/routes/island.tsx:22-26, 44` — `islandHide()` on the alive edge and `null` while alive.
- `UI/lib/meeting-hotkeys.ts:17-22, 41` — no `register(ptt)/register(eye)` while alive.
- `DESK/lib.rs:573-574` — `theuth://meetings` → `/meetings`; test `deep_link_routes` `:614-616` (+ `meetings/` and a rejected `meetings/abc`).

### Docs
`docs/design/130-companion.md` — R4 note appended to the Revision history (Revision 1 row 17).

## Gates
- `BUILD_LABEL=149-C build-lock bash crates/desktop/companion/test.sh` — ✔ 176 tests in 28 suites passed, 74 skipped (the stubs) (`/tmp/149-C-test.log`).
- `BUILD_LABEL=149-C build-lock swift build -c release --package-path crates/desktop/companion` — see the DONE line (`/tmp/149-C-release.log`).
- `bash scripts/bundle-companion.sh` → `Contents/Resources/harness/*.png` — see the DONE line (`/tmp/149-C-bundle.log`).
- `BUILD_LABEL=149-C build-lock mbx test -p desktop --no-run` ✔ · `mbx test -p desktop deep_link` ✔ `deep_link_routes ... ok` · `cargo fmt -p desktop` ✔ no diff
  (`/tmp/149-C-desktop-build2.log`, `/tmp/149-C-desktop-test.log`).
- `bunx vitest run src/lib src/components/meeting` / `bunx tsc -b --noEmit` — see the DONE line (`/tmp/149-C-vitest.log`, `/tmp/149-C-tsc.log`).
- No probe was run: C adds no surface (comment-line insertion points, data rows, models); nothing launched, no permission dialog.

## Deviations / open items
- E6's Rust skeleton in `attention.rs` (§11-C's last owned item) is NOT in this wave: the brief's "own exactly these; nothing else" list omits it;
  W0 owns `attention.rs`. The Swift side is already tolerant (`context` optional, Revision 1 names).
- `MeetingEvent` has 15 variants, not 13; `speak` takes `(session, text)`; both as read in the source.
- `MeshStatus`/`meshStatus()` reused from CompanionCore (130 W3), not re-added to TheuthKit; `AgentsModel.groups` takes that type.
- Fixtures were derived from the Rust structs, not the ts-rs `.ts` files (absent in the fresh worktree; the regen command was issued).
- `Hotkeys.start` probe narrowed to registered rows (above) — a one-line guard in a shared file, needed by the new global rows.
