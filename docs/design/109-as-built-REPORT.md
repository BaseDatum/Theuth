# 109 as built — REPORT: the resume banner's counts survive a reload

Branch `fix/109-resume-report-persist` (cut from main 191958d0).

## The gap

The counts (`N replayed, M skipped, K jobs lost`) rode only the live `AgentEvent::ResumeReport`,
published ~100 ms after boot. A desktop that reconnects seconds later never saw it ("resumed
automatically", no counts); a session opened after the resumed turn finished showed no banner at
all, because `Loaded.resume` only described an open turn.

## What changed where

- `crates/agent/migrations/0035_resume_reports.sql` — `resume_reports(session_id, turn_id, kind,
  interrupted_at, at, replayed, interrupted, lost_jobs, children_restored, children_handed_off)`,
  `PRIMARY KEY (session_id, turn_id)`; lists are JSON text. One row per turn, upserted.
- `crates/agent/src/durability/mod.rs` — `ResumeKind::{as_str, parse}` (column spelling);
  `ResumeReportRow { session, turn, kind, interrupted_at, at, report: ResumeReport }`.
- `crates/agent/src/lib.rs` — `ResumeReport { replayed, interrupted, lost_jobs, children_restored,
  children_handed_off }` (ts export, `Default`); `ResumeState.report: Option<ResumeReport>`,
  `#[serde(default)]`, `ts(optional)`.
- `crates/agent/src/store/durability.rs` — `resume_report_put(&row)`, `resume_report(session, turn)`,
  `latest_resume_report(session)` (the report of the newest turn by the `NEWEST` CTE, else `None`);
  `journal_prune` deletes the pruned turns' rows; `delete_journal_in` takes the table. Test
  `resume_reports_round_trip_latest_and_prune` (round trip, upsert, `latest` hides behind a newer
  turn, prune, delete, one `session_log` row per put).
- `crates/agent/src/store/capture.rs` — `resume_reports` added to `ATTACHED` (0033's tables are
  there; 68 replication and the sessionsync snapshot read the list, nothing else to touch).
- `crates/agent/src/loop_.rs` — `report(plan, replayed, interrupted)` builds the row from the
  `ResumePlan` every caller already holds, `resume_report_put` (error → `tracing::warn!`), then
  publishes the event with the same values.
- `crates/agent/src/supervisor/drain.rs` — `resume_state`: open turn → as before plus
  `report: store.resume_report(..)`; no open turn → `latest_resume_report`, and when `Some` returns
  `ResumeState { turn, kind, interrupted_at, resumed: true, report }` with no partial.
  `resume_state_reports_relaunched_turn` extended: report rides the open turn, survives
  `finish_turn`, a newer turn's step clears it.
- `crates/desktop/ui/src/store/reduce.ts` — `reportCounts()` shared by the `ResumeReport` event arm
  and `resumeFromLoaded`; same turn: a held live report wins, otherwise the row's; other turn: the
  row's state + report. `sessions.ts` doc line only. `ResumeBanner.tsx` untouched.
- `crates/desktop/ui/src/store/reduce.test.ts` — `109 REPORT` test: row with report → counts (cold
  and over a "resuming…" banner); live report kept over the row's and over a row without one;
  other turn replaces; `TurnStarted` for another turn still clears.
- TUI (`crates/tui`): `Model::load` does not read `Loaded.resume` (only live `TurnResumed` /
  `ResumeReport` render a divider / status note), so left alone per brief E.

## Gates (worktree target, CARGO_INCREMENTAL=0)

- `cargo test -p agent --lib store::durability`: 6 passed (was 5); `supervisor::drain`: 3 passed.
- `cargo test -p agent`: 451 passed, 0 failed, 1 ignored (54.0 s). `cargo test -p rpc --lib`: 49 passed.
- `cargo clippy -p agent -p rpc --tests -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` on the seven touched .rs files: clean (durability.rs reformatted once).
- `just ts-desktop`: ok; `bindings/ResumeState.ts` has `report?: ResumeReport`, `ResumeReport.ts` new.
- `crates/desktop/ui`: `bun run check` (tsc -b) clean after `bun install` in `packages/apps` too;
  `bunx vitest run src/store src/components/chat`: 50 files, 546 tests passed (was 545).

## Deviations from the brief

- Store API shape: `resume_report_put(&ResumeReportRow)` (row carries `session`, `turn`, `at`, like
  `job_put(&JobRow)`) instead of `(session, turn, &row)`: `latest_resume_report` must hand the turn
  back for the no-open-turn `ResumeState`; `at` from the caller lets the row round-trip in tests.
- `ResumeReportRow` lives in `crate::durability` beside `ResumePlan` (loop and drain both use it).
- `crates/rpc/src/api.rs` test literal gained `report: None` (compile fix, as BANNER did).

## Out of scope (recorded, not built)

- `report()` still hard-codes `lost_jobs: Vec::new(), children_restored: 0, children_handed_off: 0`;
  the W2/W4 counts were never wired into the event, so the row persists the same zeros.
- The roster's "resumed with gaps" attention items are rebuilt from memory only.
- `cut_journal_in` skips `resume_reports` (no `at_seq`); a cut turn's orphan row is unreachable
  (`latest_resume_report` keys on the newest journaled turn) and goes with the session.
