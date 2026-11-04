# 171F3-R notes — cells × engine restart

Base `73875543` (v2.18.2). Evidence read only (copied queries against `~/.theuth/sessions.sqlite?mode=ro`
and `~/.theuth/logs/theuth.log.2026-11-04`).

## What happened on 2026-11-04 (why only `j6` reported, why ids restarted at `j1`)

- 09:47:27 graceful shutdown: `KillAll` stamped `j6` (zsh), `j7` (sh), `j10` (bash) `exit = lost`. All
  three rows carry the `cell:` name and the ` (user cell kernel)` command; `call_id` is empty.
- 09:47:51 engine #1: the boot reaper settled all three `lost` (`ReapReport { lost: 3 }`) and built a
  `deliver` body for each (empty `call_id` never matches an open batch). The jobs actor admitted all
  three; the supervisor's only cell filter (`CellJobs`) knows ids from live `JobEvent::Started` events
  and was empty after the restart, so `j6`'s body was injected (message seq 353, 09:47:51.899) and
  started a turn. `j7` and `j10` were queued behind that turn.
- ~09:48:00 engine #1 died without a shutdown log line (new engine pid 85226 started 09:48:01).
- 09:48:11 engine #2: the rows were already `lost`, so its reaper found nothing — no `Reaped`, so the
  queued `j7` / `j10` bodies were gone with engine #1, and engine #2's jobs actor had no lost row to
  raise its counter: the session's next shell job was `j1`, and `j4`'s `File::create` truncated the
  2.18.0 bash kernel's `j4.log` (rounds A–C runs point at it).

So the leaking path is the boot reaper → `Reaped` → admission → `Settle::FinishedWithResult`, for every
kernel; the zsh/sh/bash difference was timing, not a marker. [INFERENCE: the queued `j7`/`j10` inputs
were lost with engine #1; no log line records them.] That an agent job's lost `<job_result>` queued at
boot can be lost by a second crash is the 109 design's known limit (no "reported" mark), unchanged here.

## Decisions

- **R1** — one predicate, `tools::JobRow::is_cell_kernel`: `cell:` name OR ` (user cell kernel)`
  command suffix (`tools::jobs::CELL_COMMAND`, now used by `cells::shell`) OR a `.cell.log` log. Any one
  suffices, so a row missing a newer marker is still caught. Applied where a persisted row becomes model
  input: `durability::jobs::reap` and `settle_transferred` (no `deliver`, no stored screen, no log tail),
  `transfer::on_resume_here` (kernel ids left out of the resume report's `lost_jobs`), and
  `resume_job_call` (a resumed `send` / `wait` naming a kernel answers `CELL_REFUSED`, the live B23
  reply, instead of the stored screen; `shell` / `start` never match a kernel row). The kernel's row is
  still settled `lost` and handed to the jobs actor (listing/refusal keep working); the lost cell run is
  the cells actor's `<cell_result exit="lost">`, unchanged.
- **R2** — `SessionJobs::alloc(dir)` for `Start`, `StartPty` and `Adopt`: on a session's first
  allocation in this engine (also after a closed tree drops the entry) `next` is raised to the highest
  `j<n>.log` / `j<n>.cell.log` in `<root>/jobs/<session>`, and an id whose log exists is skipped. Logs
  are the cheap authority in the tools crate (the `jobs` table lives behind the write-only `Journal`).
  Log files are now opened `create_new`, so even a race cannot truncate one (the job fails to start).
  Ceiling (marked `ponytail:`): a settled promoted job (no log) as the last job before a restart can
  have its id reused. Quiet jobs keep reusing the last number (no row, no log), as before.
- **R3** — `cells::Store::open` (the cells plugin start) renames every `cell_runs.log` ending `.log` but
  not `.cell.log` to `<stem>.cell.log` and repoints the rows in one transaction; idempotent (an already
  renamed file is just repointed). A missing file is logged and left. A file shorter than the largest
  `log_to`/`log_from` its runs recorded is left too: a reused id overwrote it (the `j4.log` case), so it
  is an agent job's log now, and moving it would hide that job's log from its own row. The agent
  store's `jobs.log` for an old kernel is not rewritten (nothing model-facing reads it any more).
