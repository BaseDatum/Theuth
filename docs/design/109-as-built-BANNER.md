# 109 as built — BANNER: the resume banner never leaves "resuming…"

Branch `fix/109-resume-banner-stale` (cut from main 0c605cc7).

## The bug as observed (2026-10-07)

Session `01a113a9-4ae1-74d0-b94e-f85179256de2` had an open turn when the engine restarted at
13:49:53. The engine resumed it at 13:50:14 (`109: resuming turn … turn=01a1169c-da01…`), the turn
ran and finished at 13:52:09 (`turn_steps` row 40 `turn_finished {"stop_reason":"end_turn"}`,
`store.open_turns()` empty for it). The desktop banner still read "Interrupted by a restart at
13:49 — resuming…" an hour later. The live `TurnResumed` was published at 13:50:13.95, ~100 ms
after boot, before the desktop had reconnected — the UI never saw the event that flips `resumed`.

## Two seams

1. **UI kept a stale banner when the row had none.** `resumeFromLoaded(cur, undefined)` returned
   `cur` unchanged, so after the turn closed every `load_session` (`resume: None`) left the
   `resumed: false` banner in place; only a new turn's `TurnStarted` cleared it.
2. **`Loaded.resume` could not say "already running".** `ResumeState { turn, kind, interrupted_at }`
   described the open turn but not whether the supervisor had relaunched it (auto) or was waiting
   for `resume_turn` (ask), so a late-connecting client always rendered "resuming…".

## What changed where

- `crates/agent/src/lib.rs` — `ResumeState.resumed: bool`, `#[serde(default)]` (old engines → `false`,
  the old wording). Doc comment per brief.
- `crates/agent/src/supervisor/drain.rs` — `resume_state(session, running)` copies `running` into
  `resumed`. `running` is the `LiveActor::is_running()` answer (`IsRunning` on the session actor —
  the same call `Drain` uses to decide what is busy), which the `load_session` handler already
  computes for `Loaded.running`; it is passed in rather than asked twice. New test
  `resume_state_reports_relaunched_turn`: no open turn → `None`; same open turn → `false`/`true` by
  `running`.
- `crates/agent/src/supervisor.rs` — the one caller passes `running` (one-line change).
- `crates/desktop/ui/src/store/reduce.ts` `resumeFromLoaded`: no row + `cur.resumed === false` →
  `{ ...cur, resumed: true }`; same turn → keep `cur` (and its report), `resumed: cur.resumed ||
  loaded.resumed`; other turn → built from the row with `resumed: loaded.resumed`.
- `crates/desktop/ui/src/store/reduce.test.ts` — existing case extended (row for another turn
  copies the flag) plus a new test: row says resumed → true; row gone → true (interrupted and
  suspended); already-resumed banner with report kept by identity; `(undefined, undefined)` →
  undefined.
- `ResumeBanner.tsx` untouched; its four strings ("resuming…", "resumed automatically", "resumes
  after the restart", "— resumed") are the ones the flipped state now reaches. No test change.
- Bindings regenerated (`just ts-desktop`); `bindings/ResumeState.ts` has `resumed: boolean`.

## Gates (worktree target, CARGO_INCREMENTAL=0)

- `cargo test -p agent --lib supervisor::drain`: 3 passed (was 2).
- `cargo test -p agent`: 450 passed, 0 failed, 1 ignored (40.3 s).
- `cargo clippy -p agent --tests -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` on lib.rs, drain.rs (+ supervisor.rs, rpc/api.rs): clean.
- `just ts-desktop`: ok. `bun run check` (tsc -b): clean.
- `bunx vitest run src/store src/components/chat`: 50 files, 545 tests passed.

## Deviations from the brief

- `crates/rpc/src/api.rs` (test at ~6443) constructs `agent::ResumeState` literally; the new
  field made it fail to compile, so it gained `resumed: false`. One line, test-only, outside the
  brief's file list but unavoidable without a `Default` impl (`TurnId` has none).
- Event publication order at boot, `loop_.rs`, `supervisor/resume.rs`, migrations: untouched.
