# 158 W3M as-built — macOS companion: island state C "Upcoming" + the project chip

Branch `feat/158-w3m`, worktree `~/Develop/theuth-158-w3m`, cut from main 23ea03c5 (contract, W1, W2, W5 on main). Swift only
(`crates/desktop/companion/**` + this file); every build/test under `BUILD_LABEL=158w3m build-lock`. Design §5.2 (c), §5.3, §9.4,
§15.4; mock island C. Nothing pushed, nothing merged.

## What was built
- `Sources/CompanionCore/MeetingReducer.swift` — `Prompt.upcoming(event: EventRef, project: String?, brief: [String])` (§15.4;
  `nil` bullets → `[]`) and `MeetingSlice.dismissedUpcoming: String?`. `reduce(.upcoming)`: shown only while no meeting is live
  (`IslandModel.live`: Starting/Recording/Paused/Stopping), not for a dismissed event id, not past `start + 5 min`; a shown
  `.detected` for the same event takes the brief/project instead (`lend`), an unrelated `.detected` keeps the pane. `reduce(.detected)`
  over a shown `.upcoming` of the same event carries its brief/project into B (§5.2 c "above (b)"); the engine's own
  `Detection.brief`/`project` win, an empty brief never shadows a real one. `.started` (via `.empty`) clears it; a not-live `.state`
  now clears only a `.stop` question (before: any prompt — a stale `State{Done}` of another meeting would have eaten the brief).
  `dismissUpcoming` (local, mutes the engine's minute re-sends by event id), `tick(_:now:)` (clears at `start + 5 min`,
  `upcomingLinger = 300`), `upcomingLine` (`In N min · <Title>`, minutes rounded up; `Now · <Title>` from the start),
  `upcomingSource` (`From the last meetings in <project> · attendees A, B`, `isMe` dropped), `date(_:)` (the RFC 3339 parse
  `eventLine` had inline).
- `Sources/Companion/Island/IslandViews.swift` — `UpcomingView` (state C): grip · clock glyph · headline · project chip · eye;
  `BriefBullets`; the source line; **Start recording** · **Dismiss**; the lawful sentence verbatim (R3). One accessibility
  group labelled with the headline. `ProjectChip` extracted (B and C share it); `PromptView` (B) draws `Detection.brief`
  bullets when present. `OpenIsland` routes `.upcoming` to the view.
- `Sources/Companion/Island/Store+Meeting.swift` — `meetingStartUpcoming(_:project:)` → `meeting_start(StartMeeting(title,
  context ≤ 1 000 chars, consent: true, source: .calendar(event: id), event: id, project))`, no `process` (no detected app; the
  engine taps system audio); `meetingDismissUpcoming()` (local); `meetingTick()` from the panel's 1 s clock timer.
- `Sources/Companion/Island/IslandPanel.swift` — ⏎ = Start on `.upcoming` (unless the request box is focused); ⎋ = Dismiss (then
  hands key back, as for Not now); the tick calls `store.meetingTick()`; `upcoming` added to the input-probe dispatch.
- `Sources/Companion/Island/IslandProbe.swift` — `--probe-island upcoming`: `Upcoming` through the real decoder → C (headline,
  bullets, source, chip; PNG), Start (asserts `meeting_start(source=calendar:ek:u1:, consent=true)` + island hides), re-feed →
  Dismiss → minute re-send muted → another event shows (no bullets; PNG) → `Detected` for it + `Upcoming` with bullets → B with
  the brief (PNG) → Not now → an `Upcoming` at start − 6 min is refused.
- Tests: `MeetingReducerTests` 26 → 31 (`upcomingShowsBeforeAMeetingAndNeverDuringOne`, `upcomingTickExpiresAtStartPlusFiveMinutes`,
  `dismissUpcomingMutesTheReSendsOfThatEvent`, `upcomingAndDetectedForTheSameEventMergeIntoB`, `upcomingLinesFollowTheMock`;
  W1's `seedCarriesTheDetectionAndUpcomingIsInert` → `seedCarriesTheDetection`); `IslandModelTests.promptShowsAndOpensWithoutAMeeting`
  covers `.upcoming` visibility.
- Project chip on B: already built by W1M (`Detection.project` → chip); kept, now `ProjectChip`, asserted by both probes.

## Gates
- `BUILD_LABEL=158w3m build-lock swift build --package-path crates/desktop/companion` → `Build complete!` (pre-existing
  `PanelProbe.swift` self-capture warnings only; `/tmp/158w3m-build.log`, `-build2.log`).
- `BUILD_LABEL=158w3m build-lock bash crates/desktop/companion/test.sh` → **281 tests in 45 suites passed** (`/tmp/158w3m-test.log`).
- `.build/debug/Companion --probe-island upcoming --shareable --probe-log /tmp/158w3m-upcoming.log` → **9/9 PASS**; PNGs
  `/tmp/149i-upcoming-c-brief.png`, `-c-nobrief.png`, `-b-with-brief.png`. (First run 8/9: an empty brief shadowed the
  re-sent bullets — fixed with `lend`, test added.)
- `--probe-island prompt` → 12/12 PASS; `stop` → 4/4 PASS; `clicks` → 17/24: every mouse step PASS, the 7 keyboard steps
  WITHHELD (Sky's Preview frontmost; the shared-machine guard), as in W1M — not a regression.

## Libraries
- Foundation `ISO8601DateFormatter` / `Date.FormatStyle` for the countdown and times; SwiftUI/SF Symbols for the view. No new
  dependencies.

## Deviations
- Source line reads `From the last meetings in <project>` — the frozen `Upcoming { event, project, brief }` carries no meeting
  count, so the mock's `last 3` cannot be rendered truthfully.
- Headline `In N min · <Title>` (brief + mock) rather than the design sketch's `<Title> in N min`.
- Dismiss is local (no RPC in the contract): the engine's minute re-sends are muted by event id for the slice's lifetime. If the
  engine should learn Dismiss, that is a contract addition — orchestrator's call.
- "Both apply" is one pane, not two stacked: the detection prompt (B) shows the brief bullets; a detection with no/another event
  leaves the Upcoming out (it returns when the detection clears and the engine re-sends).
- `Start` from C sends no `process` (nothing detected yet); the engine's default capture applies.

## Hand-offs
- None outside my files. Windows island (C#) state C is not mine. Engine: `Upcoming` is W3's Rust half; the companion is ready for it.

## Open items
- Re-run `--probe-island clicks` when nothing of Sky's is frontmost (7 keyboard steps withheld, unchanged from W1M).
- `dismissedUpcoming` is never forgotten within a connection; a re-shown event after an edit (new `sequence`) keeps the same id.
