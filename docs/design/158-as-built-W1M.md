# 158 W1M as-built — macOS companion: the island prompt states

Branch `feat/158-w1m`, worktree `~/Develop/theuth-158-w1m`, cut from the C158 contract PIN 715e22d3 (pre-gate). Swift only
(`crates/desktop/companion/**` + this file); every build/test under `BUILD_LABEL=158w1m build-lock`. Design §5, §4.5, §15.4;
mock states A, B, D (C "Upcoming" is W3). Nothing pushed, nothing merged.

## What was built
- `Sources/CompanionCore/MeetingReducer.swift` — `MeetingSlice.prompt: Prompt?` with `Prompt = .detected(Detection) |
  .stop(app:releasedForS:)` (§15.4; `.upcoming` arrives with W3's reducer + view) and `keptReleasedForS: UInt32?`.
  `reduce`: `.detected` → `.detected(d)`; `.detectionCleared(appId, _)` → clears only a shown `.detected` for that app;
  `.appReleased(meeting, app, s)` → `.stop` only for the live slice's own meeting (re-sends refresh the count); `.started` (via
  `.empty`) and a not-live `.state` clear any prompt; `.upcoming` inert. `seed` carries `MeetingStatus.detection`.
  `keepRecording(_:)` — Keep is local (no RPC in the contract): the prompt goes and the engine's 30 s re-sends of that release
  (a larger `releasedForS`) are muted; a smaller count is a fresh release and asks again. `eventLine(_:)` →
  `<Title> · <hh:mm>–<hh:mm>` (RFC 3339 with/without fractional seconds; otherwise the title alone).
- `Sources/CompanionCore/IslandModel.swift` — `visible = alwaysShow || live || prompt != nil` (§5.1);
  `attached(live:collapsed:prompt:)` — a prompt always opens the pane (a collapsed live island reopens for "Stop recording?").
- `Sources/Companion/Island/IslandViews.swift` — `PromptView` (state A/B): grip · `● <App> is using your microphone` · eye · ⋮;
  optional `eventLine` + project chip; **Start recording** (accent capsule) · **Not now**; ⋮ → inline **Never for <App>** · ×
  (the strip's own menu pattern, so the probe can click it); the lawful sentence verbatim (R3). One accessibility group labelled
  with the first line; buttons carry their titles. `StopRow` (state D) under the live strip: `<App> stopped using your microphone
  N s ago` · Stop recording? · **Stop** · **Keep recording**. `PromptButton`: `.borderedProminent` renders grey in a non-key
  panel, so the primary is an accent capsule (system accent; no new tints).
- `Sources/Companion/Island/Store+Meeting.swift` — `meetingStartDetected(_:)` → `meeting_start(StartMeeting(process: app.pid,
  title/context/event from `detection.event` when W2 fills it (context ≤ 1 000 chars), consent: true, source: .detected(appId),
  project: detection.project))`; `meetingNotNow` / `meetingNever` → `meeting_detection_dismiss(appId, never: false/true)`;
  `meetingKeepRecording` (local). Each clears the prompt at once; the engine's `DetectionCleared`/`Started` confirm. Probes log
  the labels in `Store.meetingActs` and send nothing.
- `Sources/Companion/Island/IslandPanel.swift` — `applyLayout` passes `prompt:`; keys while the island is key (§5.3): ⏎ = Start
  (detected) / Keep (stop) unless the request box is focused (then the TextField sends); ⎋ = Not now (then hands key back, as
  before); `N` inside the ⋮ menu = Never. ⌃⇧M hold = Stop was already the live stop chord. `.nonactivatingPanel`,
  `becomesKeyOnlyIfNeeded`, `sharingType .none` and `orderFrontRegardless` untouched; every answer clicks without key.
- `Sources/Companion/Island/IslandProbe.swift` — `--probe-island prompt`: the wire's `Detected` through `Store.meetingObserve`
  → A, click Start (asserts the `meeting_start(pid=4242, source=detected:us.zoom.xos, consent=true)` label and that the island
  hides once the prompt clears), re-feed → Not now, feed B (event + project) → ⋮ → Never, then the fixture recording +
  `AppReleased` → D, Keep (muted re-send at 60 s, fresh 30 s asks again), Stop.
- Tests: `MeetingReducerTests` 20 → 26 (`detectedShowsThePromptAndStartClearsIt`, `detectionClearedOnlyForTheShownApp`,
  `appReleasedAsksOnlyForTheLiveMeeting`, `keepRecordingMutesTheReSendsUntilAFreshRelease`, `seedCarriesTheDetectionAndUpcomingIsInert`,
  `eventLineFormatsTitleAndTimes`); `IslandModelTests` 12 → 13 (`promptShowsAndOpensWithoutAMeeting`).
- RpcClient: the contract's `meetingStart` / `meetingDetection` / `meetingDetectionDismiss` bodies were already real calls
  (`RpcClient.swift:352-360`); nothing to fill.

## Gates
- `BUILD_LABEL=158w1m build-lock swift build --package-path crates/desktop/companion` → `Build complete!` (pre-existing
  `NSEvent Sendable` / `shared` warnings only; `/tmp/158w1m-build.log`, `-build2.log`).
- `BUILD_LABEL=158w1m build-lock bash crates/desktop/companion/test.sh` → **268 tests in 44 suites passed** (`/tmp/158w1m-test.log`).
- `.build/debug/Companion --probe-island prompt --shareable --probe-log /tmp/158w1m-prompt.log` → **12/12 PASS**; PNGs
  `/tmp/149i-prompt-a-detected.png`, `-b-event.png`, `-b-menu.png`, `-d-stop.png` (the probe's output dir and prefix).
- `--probe-island stop` → 4/4 PASS (`/tmp/158w1m-stop.log`). `--probe-island clicks` → 17/24: every mouse step PASS; the 7
  keyboard steps failed because Sky's Preview was frontmost and the probe withholds keys unless it is frontmost (13 × `WITHHELD`,
  the 149i shared-machine guard; `/tmp/158w1m-clicks.log`). Not a regression of this change; see the open items.

## Libraries
- Date parsing/formatting: Foundation `ISO8601DateFormatter` + `Date.FormatStyle(time: .shortened)` (what `TheuthJSON` uses;
  its formatters are private to TheuthKit, which W1M does not own). No new dependencies.

## Deviations
- `Prompt.upcoming` not declared: W3 adds the case with its reducer and view (no dead case/EmptyView now).
- "Keep recording" has no RPC in the contract; it is local and mutes the engine's re-sends by `releasedForS` ordering. If the
  engine should learn Keep (leave `Ending`), that is a contract addition — orchestrator's call.
- The overflow is the strip's inline-menu pattern, not an `NSMenu`: same affordance, probe-clickable, keeps the panel non-key.
- Start/Not now/Never clear the prompt optimistically before the RPC answers (as `markTapped` does); a failed call is logged.

## Hand-offs
- None outside my files. W2: fill `Detection.event` → the event line and `StartMeeting.title/context/event` already flow; the
  `[▾]` "Which meeting is this?" list is W2's (`meetingDetectionSetEvent` is wired in RpcClient, unused here). W3: `.upcoming`.

## Open items
- Re-run `--probe-island clicks` when nothing of Sky's is frontmost to see the 7 keyboard steps (J/K/↑/↓/⎋) pass as in 149i;
  the touched key paths (`handle`: islandSend/N; `escape`) only act while `slice.prompt` is set.
- The lawful line, button titles and the headline are English-only, as the rest of the island.
