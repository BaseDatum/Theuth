# 158 as-built — F158 (fix the four live-proof defects)

Branch `fix/158-live-defects` from main `3380bc61`, worktree `theuth-158-fix`. Defects from `docs/design/158-live-proof.md`
("Defects found"); the closing notes are in its "Fix round" section. Failing test first for each, then the smallest change.

## Built

| # | Defect | Change | Test (before → after) |
|---|--------|--------|-----------------------|
| D1 | `Z` DTSTART + RRULE −1 h | `crates/meeting/src/calendar/ics.rs`: `utc_anchored()` (DTSTART has `Z`, no TZID) → the UID's sub-calendar expands with `Tz::UTC` as default; floating groups keep local | `tests/calendar.rs::utc_rrule_expands_in_utc`: `16:00Z ×6` → `17:00Z ×6` across 2026-10-25 |
| D2 | "I am <Name>" missed | `crates/meeting/src/post/suggest.rs` `INTRO_RE`: `+ i am` | `intro_cue_forms_and_false_positives`: no Alice → Alice/Bob/Carol/Dave, "I am going/not/sorry/Here/Fine" rejected |
| D3 | 12 h banner vs 24 h switcher | `DetectionBanner.tsx` → `eventLine()` from `lib/calendar`; `lib/detection.ts` `timeRange` deleted (no other caller) | `DetectionBanner.test.ts`: `11:00 AM–11:30 AM` → `<span>{eventLine(ev)}</span>`, no `AM/PM` |
| D4 | meeting-only projects not offered | `lib/meeting-project.ts` `useProjectChoices` + `useQuery(meetingsQuery(api, null))` → distinct `project`s; same `queryKeys.meetings` row the list caches, no new RPC. StartForm / ProjectChip / list filter all go through it | `meeting-project.test.ts` D4: `theuth` → `ops,theuth`; `StartForm.test.ts` mock `+ meeting_list` |

Root causes: D1 — calcard `expand_dates` resolves a TZID-less DTSTART to the *default* zone and iterates the RRULE in
it (`dates.rs:374`, `:136-144`), even when the value carried `Z`; single events are unaffected because they go through
`to_date_time_with_tz`, which honours the `Z` offset. D2–D4 as the live proof described.

## Libraries
- D1: stayed on calcard (already the ICS engine); the glue picks its default zone per series instead of a second
  recurrence library (rrule crate rejected: would duplicate calcard's VTIMEZONE/override handling).
- D3/D4: existing `lib/calendar.eventLine`, `lib/time.formatAt`, `meetingsQuery` — nothing new.

## Deviations / outside the file list
- `crates/desktop/ui/src/components/meeting/StartForm.test.ts`: one mock line (`meeting_list: async () => []`) — the
  form's picker now issues that query and the static-render helper awaits every query. Test-only, no behaviour.
- `lib/detection.ts` lost `timeRange` (clean cutover; DetectionBanner was its only caller).

## Hand-offs
None. W6E's files untouched.

## Open items
- `INTRO_RE` still captures tokens with apostrophes ("I'm Monday's host" → "Monday's") — pre-existing, not in this brief.

## Gates
- `build-lock mbx test -p meeting --no-run` → `mbx test -p meeting`: lib 121 passed / 2 ignored, `calendar` 16 passed,
  `note` 1, `oracle` 1 (before the fixes the two new tests failed as recorded above).
- `build-lock mbx clippy -p meeting` → clean.
- `build-lock bun install` (ui 623, apps 484); `build-lock bunx tsc --noEmit -p .` → 0.
- vitest (DetectionBanner, meeting-project, EventSwitcher, ProjectChip, StartForm, MeetingList, detection, calendar) → 27/27.
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → Finished, 0 errors / 0 warnings (only cargo's pre-existing
  `block v0.1.6` future-incompat notice).
