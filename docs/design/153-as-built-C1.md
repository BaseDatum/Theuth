# 153 as-built — C1 (companion command mode + one answer card + keys, macOS)

Worktree `~/Develop/theuth-153-c1`, branch `feat/153-c1`, cut from the CC PIN `5e0b9980` (+ `557eb083` as-built). Design
`153-voice-anywhere.md` §3.4–3.5, §10, §12, §15, §16 rows 16–18/20/22, §18 Q1/Q6/Q7/Q9 defaults applied. `COMP/` =
`crates/desktop/companion/Sources/`.

## Files
- `COMP/CompanionCore/PointMachine.swift` — `Event.switchMode(Mode)` → `Effect.relabel(Mode)` in armed/holding/tap1/locked/toggled
  (state and recording untouched; idle/sending/replying ignore it); `Gesture = ModeKeySettings.Gesture` as a defaulted `step`
  parameter (`.holdLock` = the 149 table verbatim); `.automatic`: `tapToggleMs = 400`, `armed → toggled` on release < 400 ms
  (`showLock`), `toggled → sending` on the next press (`stop(tap)`, the 800 ms short-note rule), hold tick at 400 ms → `holding`,
  push-to-talk on release; never `tap1`/`locked`. `hint(for:)`: "Tap to toggle · hold to talk" in automatic.
- `COMP/CompanionCore/CommandCard.swift` — reducer: `toolCallStarted` → `Row(kind,text)` from `parsePurpose` (`checking:`/`executing:`/
  `verifying:` prefix, case-insensitive, ≤ 60 chars, tool name fallback), `tools` in start order, `steps`; the 12th start sets
  `frozen = stoppedCopy` and answers `true` (controller cancels); later starts/text/`TurnFinished` never overwrite it, late
  `toolCallFinished` still colours its row. `turnFinished`: `error`/`cancelled` → `frozen = "✗ couldn't finish — …"`, else
  `verdict = parseVerdict(text)` (`✓`/`✗` first char → ok/failed, else `replied`). `Row.label(queued:)` = checked/executed/verified/ran ·
  checking/executing/verifying/running · next.
- `COMP/CompanionCore/SpokenApproval.swift` — normalise (lowercase, punctuation → space), exact phrase, then Levenshtein ≤ `len/5` for
  phrases ≥ 5 chars; a tie between the lists is `nil`. `missedCopy = "didn't catch that — ⏎ or ⎋"`.
- `COMP/CompanionCore/AnswerCardState.swift` — `Kind ask|voiceNote|command`, `turns: [Turn{question,text,tools,command}]` (the
  §12.2 thread; `current` for J/K, reducers write the last), `session`, `approval: Approval{id,tool,target,risk,listening,missed,outcome}`,
  `thinkingSince` (the view ticks "· N s"), `begin(question:kind:)` / `followUp(question:)` both bump `generation`, `hide(generation:)`,
  `armsDone = answer ∧ turnFinished ∧ !speaking ∧ !expanded ∧ no open approval`, `marker(kind, durationMs:)` (`[command, N s] `).
- `COMP/Companion/PointAndSay/AnswerCard.swift` — `AnswerCardModel` (view-model over the pure state + draft + ⌘L list),
  `AnswerCardView` (collapsed = header + `firstTwoLines` + chips + caps; expanded = dots, viewed turn, `MarkdownBlocks`, chips,
  follow-up field, caps; command rows with shimmer/green/red, verdict + "N steps tool · tool"; the amber approval box with Yes ⏎ /
  No ⎋ / "or say yes/no · listening" / missed copy / ✓✗ outcome line), `AnswerCard` panel (440 pt, `IslandArea.frame` at the
  press point, 180 ms resize, key monitor: approval ⏎/⎋ first, ⎋ closes list → collapses → dismisses, ⏎ expand/collapse, O, J/K,
  ⌘L; typing in the field keeps its keys).
- `COMP/Companion/PointAndSay/PointController.swift` — one controller, `mode` chosen at press (`keyDown(.ask|.command)`), the
  other chord while live → `switchMode` (its auto-repeats hit the gate as before; either key-up releases); `.commandKey` ⌃⌥C +
  `.commandEscapeHeld` ⌃⌥⎋ registered from the table when `ModeKeySettings.commandKeyEnabled`; `gestureKind` per mode at press
  (hold tick at 400 ms in automatic); send = `[command, N s] ‹text›` (+ screen only when `CommandSettings.includeScreen`, default
  off) or `[voice note, N s]` (+ screen per the voice key's setting) via `steer`/`send_input`; reply events feed `AskRouter` + the
  `CommandCard` (`purpose` = `args.purpose ?? args.reason`, cap → `cancel(desk, root)`); `ApprovalRequested` for the card's desk
  session while a command card is up → box + 8 s `dictation_start(opts: {surface: companion, destination: agentPrompt})` listen,
  first final utterance → `SpokenApproval.decide` → `resolve_approval` or the missed copy; `ApprovalResolved` → outcome (timedOut);
  ⎋ during thinking → `cancel(root)` + status "stopped", `.failed` (⎋ only); typed follow-up → same marker, `followUp`; a hold
  while the card is up in the same mode → the card steps aside for the pill and the send appends a block; `doneMs` arms only on
  `armsDone`, generation-checked; `Store.showAnswerCard` → `showExternal` (keeps the viewer's expansion across a streaming push).
- `COMP/Companion/PointAndSay/CommandProbe.swift` — `--probe-command <heard|working|approval|done>`, `--probe-card <collapsed|expanded|failed>`.
- `COMP/Companion/Settings/CommandSettings.swift` — switch + Keys row, `GesturePicker` (shared with `VoiceKeyTab`, keys
  `gesture.<mode>`), "Include the screen" (off), modifier-only row (disabled until `CGPreflightListenEventAccess`, a "Grant Input
  Monitoring…" button = the one place the prompt may appear, the 🌐-key hint). `VoiceKeyTab.swift` gains the picker.
- `COMP/Companion/Island/VoiceModeView.swift` — `VoiceModeState.label` ("Command · theuth desk"), `reset(chord:label:)`.
- Shared-file hunks (one line each, named for the integrator): `Store.swift:309` `showAnswerCard` body → `pointShowCard(state)`;
  `Settings/SettingsStore.swift:66` `anyProbe` + `probeCommand`/`probeCard`.

## Design vs built
| Design | Built |
|---|---|
| §10.1 one `PointController` per mode | One controller, `mode` per gesture: one sampler/pill/card/dictation; the switch is a field write + relabel. |
| §12.4 `Phase.speaking` | Dropped from `Phase`: speaking is the parallel glyph flag (the stub listed both). |
| §10.6 listen with `end_commands` off | `DictationOpts` has no per-call end-command switch (A built it per surface preset); the listen reads `text` only and ignores `command`. |
| §12.2 follow-up by hold = B3's machine | Voice follow-up = hold the mode key (this controller's gesture, `followUp`); typing goes through the same `deliver`. B3's machine untouched. |
| §12.6 modifier-only / fn chords | Rows + grant button only. Binding a lone modifier needs `Chord.keyCode` optional + `check(.needsInputMonitoring)` (CC's `KeyBindings.swift` / `ShortcutRecorder.swift`) and the `flagsChanged` monitor — hand-off below. |
| CC hand-off "add to `Hotkeys.names`" | Registered the voice key's way (per-controller `applyHotkey`, honours the enabled switch, logs); `Hotkeys.swift` untouched. `dictateAnywhere` registers nothing (wave D). |
| §12.2 ⌘L list = `activity_list` 24 h | `store.ask.receipts.entries.prefix(20)` (the same fetch, already on the Store). |
| §12.2 typed follow-up marker | `[command, 1 s]` (`marker(kind, durationMs: 0)` floors at 1 s) — the prompt matches the `[command` prefix. |

## Tests
`BUILD_LABEL=153-C1 build-lock ./test.sh` → `✔ Test run with 226 tests in 36 suites passed` (was 211). New: `PointMachineTests`
`.switchModeKeepsRecording`, `.automaticTapTogglesHoldTalks`, `.automaticNoDoubleTapLock` (the 17 ported + `VoiceKeyGateTests` 7
unmodified, green); `SpokenApprovalTests` ×5 (yes/go · no · near-miss · unknown · constants); `CommandCardTests` ×3 (rows with/without
purpose, cap 12 freezes, verdict parse); `AnswerCardStateTests` ×3 (stale hide ignored, doneMs arming, thread/markers);
`KeyTableTests.rows153ModeKeysDriveTheMachine`.

## Gates
- `BUILD_LABEL=153-C1 build-lock swift build` → `Build complete! (139.83s)` (pre-existing `shared`/`NSEvent Sendable` warnings only).
- Probes (`.build/debug/Companion --probe-… --shareable`, no engine needed, nothing sent, no TCC prompt), PNGs under `/tmp/153-c1/`:
  `153-command-heard.png`, `153-command-working.png`, `153-command-approval.png`, `153-command-done.png`, `153-card-collapsed.png`,
  `153-card-expanded.png`, `153-card-failed.png`; log `/tmp/153-c1/probe.log` (`frame=(1060, -140, 440, …)` = the island area).
- Not run (needs Sky at the keyboard + the C2 prompt block): §16 row 21 live ⌃⌥C → approval by voice → ✓ line; mode switch mid-hold.

## Hand-offs
- **C2 (engine, DONE 1fa78c3b — aligned)**: the card reads the top-level `purpose` key of `ToolCallStarted.args` (wire key `args`; `arguments` also accepted; fallback `reason`); prefixes `checking:` / `executing:` /
  `verifying:` (case-insensitive, colon required) type the row; the text after the colon ≤ 60 chars (clipped); no/unknown prefix → the
  row says `running`/`ran` with the text (or the tool name). Final line: first non-blank char `✓` or `✗`. The user line is
  `[command, N s] ‹words›` (N ≥ 1, ceil of the hold), `[screen …]` lines follow only if the user turned the screen on.
- **Windows twin**: constants `tapToggleMs 400`, `maxSteps 12`, `textCap 60`, `listenSecs 8`, `doneMs 6000`; machine transitions as
  `PointMachineTests` (new 3); grammar + `len/5` near-miss as `SpokenApprovalTests`; card anatomy = `AnswerCardView` (collapsed: header
  `who · status`, two lines, chips, caps; expanded: `N of M` dots, question, markdown, chips, field, caps; command rows label column 64 pt,
  mono text, shimmer on the first unfinished row, later ones `next`; approval box amber, Yes ⏎ / No ⎋ / listening / missed copy; outcome
  line `✓ approved — target` / `✗ denied — target` / `✗ denied (no answer in time) — target`); ⎋ order: approval deny → list → collapse → dismiss.
- **CC / wave D**: modifier-only chords need `Chord.keyCode` optional + `KeyTable.check` `.needsInputMonitoring` + a `flagsChanged`
  key-code monitor (fn 63, ⌘ 55/54, ⌥ 58/61, ⌃ 59/62, ⇧ 56/60) with chord protection; the settings row and the grant button exist.
- **B3**: push the Ask bar's reply as `AnswerCardState` (kind `.ask`, `session`, `text`, `turnFinished`, `phase`) through
  `store.showAnswerCard`; the card keeps the user's expansion when `generation` is unchanged; `visible = false` hides.
