# 158 as-built — W3 (projects as the meeting series + the pre-meeting brief): the wave

Closed on `feat/158-w3e` (closer W3E). Base: main 23ea03c5 (contract, W1, W2, W5 on main); main not merged.
Per-worker detail: `158-as-built-W3E.md` (engine), `-W3M.md` (macOS island), `-W3W.md` (Windows island), `-W3U.md` (desktop UI).

| Worker | Branch / head | Scope | Merged at |
|---|---|---|---|
| W3E | `feat/158-w3e` `fc6bc946` | `meetings.project` + filter, project memory, `meeting_set_project`, prebrief writer + cache, `Upcoming` tick | — |
| W3M | `feat/158-w3m` `7dffaa52` | Swift island state (c) Upcoming + `ProjectChip`, Start from (c), Dismiss (local), `--probe-island upcoming` | `6558a8f8` |
| W3W | `feat/158-w3w` `9076137d` | C# twin: `MeetingUpcoming`, merged pane, `--island-upcoming(-auto)`, build-B green | `f77e5c4c` |
| W3U | `feat/158-w3u` `773ae0a4` | start-form Project combobox, header `ProjectChip`, `MeetingList` project filter, `lib/meeting-project.ts` | `5a50bda9` |
| close | this branch `308666b2`+ | seams below, writer fallback, gates, this file | — |

## What landed

- **Engine**: `Manifest.project` / `MeetingSummary.project` persisted (index row + `list_in` filter + `done_in_project`); start
  resolves the project (pick → `uid:` → `title:` → `meeting.project`), the session is created under it (else `meetings`, A2 (1));
  `meeting_set_project` rewrites manifest + index + memory rows + `agent::SetSessionProject`; `Detection.project`/`brief` filled on
  every prompt and re-pick; `cal_prebrief` cache (lazy invalidation on the Done set / event fingerprint); one writer turn per
  instance; the brief row (`briefs.sqlite`) with event label + attendee first names as aliases, tags `pre-meeting`/`<project>`;
  `Upcoming { event, project, brief }` from the one-minute tick.
- **macOS island**: `Prompt.upcoming`, state (c) alone or merged into (b) when the detection names the same event (`lend`), Start →
  `meeting_start { source: calendar, event, project, context, consent: true }`, Dismiss local (mutes that event id), clears at
  `start + 5` / on start / while a meeting is live. 281 Swift tests, `--probe-island upcoming` 9/9.
- **Windows island**: the same rules (W3M parity 1–5) on the W1W shapes; `MeetingWire` decodes `Upcoming` + `Detection.event/project/brief`;
  Core 500 / Rpc 114 tests; visible run on build-B (`158-upcoming-*.png`).
- **Desktop UI**: `ProjectChip` (cmdk combobox, "New project ‹name›"), start form "Project" (`Default` = engine's choice; key absent
  when not picked), header chip → `meeting_set_project` (returned manifest replaces the cache), list filter → `meeting_list(null, p)`.

## Seams checked at close (engine ↔ islands ↔ UI)

1. **`Upcoming` cadence** — both islands were built for the engine's *minute re-sends* (dismissed ids muted, an empty re-send never
   shadows shown bullets, W3W's late-connecting companion "shows the card at the next tick"). W3E emitted once per instance; the
   engine now re-sends every tick while `start − 5 ≤ now ≤ start + 5` (§9.4's wording). Fixed engine-side; test updated
   (`three_done_meetings_one_turn_brief_row_upcoming_at_t_minus_5`: T−5, T−4, T+4 each carry it from the cache, one turn total).
2. **Start from (c)/(b) with `context` = the event description** (both islands, ≤ 1 000 chars) — the W2 seed appends the event's
   context block (which holds the description) to `context`, so the model saw it twice. Fixed engine-side: a `context` equal to the
   event's capped description is dropped before the seed. `project` from the chip is honoured and remembered; no `process` → system
   audio as before.
3. **Detected + Upcoming of the same event** merge into one pane on both islands; the engine's `Detection.brief` is the cached
   bullets (`decorate`), so the merged pane shows the same brief with no second generation.
4. **W3U**: `meeting_list(null, project)` → `ListMeetings { project }` → `list_in`; `meeting_set_project` returns the updated
   `Manifest`; the start form omits `project` when not picked (`#[serde(default)]`). No change needed.

## The agent hop (orchestrator step 2)

- No cross-crate mock-model harness exists: `agent`'s rigs (`advisor/tests.rs`, `loop_/*_tests.rs`, `agents_on`, `test_auth`,
  `TestDir`) are `pub(crate)`; `oracle.rs` tests mock at the `decision::SideCaller` closure. No engine-internal Private root exists
  anywhere (every private owner today is a TUI/desktop RPC peer; dreaming/knowledge only read `Retention`), so the synthetic peer
  `u64::MAX − 158` is not an established pattern and there is no supported API for it.
- Therefore the writer is now **two-legged**: `private_turn` (the §9.4 Private root with read tools) and, on any refusal or the
  45 s timeout, `fallback` = one no-tools `runtime::SideCall` (the oracle's path, `CallKind::Escalate`, `ModelRole::Named(meeting.model)`
  else `Session`, system "facts only") on a throwaway **durable** session of the project that is `DeleteSession`ed right after.
  `prebrief::tests::fallback_side_call_shape_and_both_legs_reported` pins the call shape and that a refused private leg reaches the
  fallback (error names both legs). The hop against a live agent remains **unexercised in tests**; recipe below.

## Gates (merged tree, each its own command, BUILD_LABEL=158w3e)

- `build-lock mbx test -p meeting -p rpc --no-run` → 0; `mbx test -p meeting -p rpc` → meeting 100 passed (1 ignored, pre-existing)
  + calendar 15 + 1 + 1, rpc 56 + 1 + 1 passed, 0 failed.
- `build-lock mbx clippy -p meeting -p rpc` → 0, no warnings. `cargo fmt -p meeting -p rpc` bare.
- `build-lock bun install` (ui 623 packages, packages/apps) → 0. `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet
  export_bindings` → 8 passed; 871 files copied to `crates/desktop/ui/src/rpc/bindings/` (gitignored); the tracked
  `crates/rpc/bindings/MeetingSummary.ts` doc line regenerated and committed.
- `build-lock bunx tsc --noEmit -p .` → 0. `build-lock bunx vitest run` lib/meeting-project, meeting/ProjectChip, StartForm, MeetingList,
  ControlCenter, DetectionBanner, EventSwitcher, lib/session-tree, rpc/api → 9 files, 33 passed.
- `build-lock bash crates/desktop/companion/test.sh` → 281 tests in 45 suites passed.
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → 0 (only the pre-existing `block v0.1.6` future-incompat note).
- Windows: not re-run at close (C# untouched since W3W's build-B `20261010T160534-339` @ 4a148e3c: Core 500/500, Rpc 114/114).

## Live proof — owed (machine gate)

`vm_stat` free + speculative × 16 KiB = 0.71 GB at 17:59 (load 76); the rule is > 4 GB for a scratch engine. Recipe for the
orchestrator's pass: engine with `meeting.calendar.prebrief: true`, a model, Sky's secret address or macOS Calendar on; a recurring
event whose series was started once with a project (or `meeting_set_project` on a Done meeting of it, which writes `uid:`/`title:`)
and ≥ 1 Done meeting of that project with a `summary.md`. At T−15 the engine log shows one Private root (or the "falling back to a
side call" warning), `briefs.sqlite` gains a `pre-meeting` row with the attendee first names as aliases; from T−5 every minute an
`Upcoming` goes out — island (c) on the Mac (and `--island-upcoming` on build-B), Start from it records with `source: calendar`,
and the brief shows as a `Cache` topic at 00:00 through the alias scan.

## Deviations (wave level)

- `Upcoming` re-sent each minute inside the window (W3E first shipped once-per-instance; the islands dedupe by event id).
- The brief row is filed by the engine, not by the model's `brief_put`; the turn's toolset is read-only.
- Writer fallback (above) is a W3-close addition, not in §9.4; it charges `CallKind::Escalate` (no `Prebrief` kind in `runtime`).
- Headline copy `In N min · <Title>` (both islands) vs. the §5.2 sketch; source line "From the last meetings in <project>" (no count on
  the wire; W3W adds ` · attendees …`). Dismiss is local on both islands (no RPC in the contract).
- W3U list filter is a `Select`; start form shows "Default" (no "none" distinct from "default" in the frozen shape).
- Engine: only an explicit start pick writes memory; invalidation lazy; a one-bullet reply is cached but files no row (cache takes 2–3).
- Outside W3E's list, minimal: `rules.rs` `prompt_mut`, `SetPromptEvent` +2 fields, `oracle::CALENDAR` `pub(crate)`.

## Open items

- Live pass owed (below). The Private leg's `ParticipantsReady` behaviour with no RPC peer is unobserved; the fallback covers it.
- `dismissedUpcoming` (Swift) / `UpcomingDismissed` (C#) persist for the connection; an edited event keeps its id.
- `--probe-island clicks` keyboard steps still withheld when something of Sky's is frontmost (unchanged since W1M).
- W3U: filter resets per visit; no "unprojected" filter (`project IS NULL`).
- `tasks/ProjectCombobox` and `meeting/ProjectChip` are two near-identical pickers.
