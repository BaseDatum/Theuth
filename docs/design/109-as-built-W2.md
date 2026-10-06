# 109 as-built — W2 (jobs: journal rows, KillAll ordering, reaper, `lost` delivery, PTY screens)

Worktree `theuth-109-w2`, branch `feat/109-w2` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: everything in brief W2's build list. Jobs stay non-durable (R1/D3/D14); what was added is
the accounting that lets the model and the UI learn a job's fate after a restart, and the reaper
that kills what a SIGKILL of the engine leaked. Line numbers are from the final commit.

## 1. `jobs` rows through the `Journal` seam (`crates/tools/src/jobs.rs`, `pty.rs`)

- `Job` gained `call_id`, `started_ms` and `lost: Option<Lost>` (`jobs.rs:543-576`);
  `Job::row(session, state, exit, settled_ms) -> JobRow` (`:595`) mirrors it, `pgid` = the first
  recorded `ProcessTree`.
- Shell job (`jobs.rs::run`, `:1537`): the `running` row is written before `JobEvent::Started`
  (`pgid: None`); `setup`'s `on_spawn` hook sends the first group's pgid on an unbounded channel
  (`:1364`) and a new `select!` arm in the run loop rewrites the row with it (`:1589`) — so a crash
  a few ms after the spawn still leaves the pgid for the reaper; at exit the row is upserted
  `settled` with `exit`/`settled_ms` **before** the `Finished` tell and the bus event. A quiet job
  (37 §3.2) writes no row, as designed.
- PTY job (`pty.rs::run`, `:2020`): the `running` row (pgid = the session leader) is written first,
  the `settled` row after the final screen snapshot (`:2177-2180`).
- `Jobs.pty_snapshot: Duration` (`jobs.rs:692`), 5 s default, read in `jobs::start` from the
  runtime figment as `durability.pty_snapshot_secs` (`:777`) — the `durability:` section is top
  level, not under `tools:`, so the actor reads it itself instead of `registry.rs` (not mine).
- Waits (§4.4, "the job side"): the `shell` grace wait writes a `waits` row of kind `job`
  (`target` = job id, `deadline` = now + grace) and clears it when the wait ends
  (`jobs.rs:853-876`); `wait` writes a `pty_wait` row (`payload {pattern, cursor}`,
  deadline = the wait's timeout) and clears it on return (`pty.rs:1540-1565`). `pty::Wait` gained
  `call_id: String` for the row's key (`:994`); `WaitTool` passes `cx.call_id`. Empty call id
  (RPC / tests) → no row.

## 2. `KillAll` writes `lost` before the signal (`jobs.rs:1139-1180`)

For every running job: `journal.job(row{state: running, exit: lost, settled_ms: now})`, then every
live PTY's screen snapshot, then — only then — `kill(&job.pgids)`, `unknown(...)`, `task.abort()`.
`KillAll` is now `pub` (`:409`) so W5's drain can tell it after its window (§9.2 step 6); the
`rt.shutdown` trigger in `jobs::start` is unchanged until the drain exists.

## 3. Reaper — `crates/agent/src/durability/jobs.rs`

- `reap_jobs(&Store) -> ReapReport` (`:40`) = `reap(&Store) -> (ReapReport, Reaped)` (`:55`, the
  store half, what the tests call) + one `tell(Reaped)` to the `jobs` actor by registry name
  (the `rearm_reminders` shape: a `tell`, the actor admits off its mailbox, so the supervisor's
  `ActivityRegistered` handler that awaits `reap_jobs` is never asked back into).
- Per `jobs_open()` row, nothing skipped (a `resume: off` session's row included — the reaper
  reads no config): pgid alive (`ProcessTree::adopt(pgid).alive()`) → `kill()`, `killed += 1`,
  `info!("reaped job … leaked by a restart")`; `job_settle(…, Lost, Lost, now)` → `lost += 1`;
  session row missing → `orphaned += 1`, not reported (see deviations); PTY → the stored
  `pty_screen`; `tail` = last 8 KiB of the log file (`log_tail`, `:182`, seeks — never reads the
  whole file); `deliver = Some(body)` unless `(session, call_id)` is a `started` `tool_calls` row
  of an open turn (`open_turns` × `calls_of`), in which case the job rule reports it (§4.3).
- `lost_body` (`:141`): `this job died with the engine (restarted at HH:MM:SS); its exit code is
  unknown.` — or `killed at shutdown (engine stopped at HH:MM:SS, restarted at HH:MM:SS)` when the
  row already carried `exit = lost` (the `KillAll` stamp, §5.5) — then `log: <path>`, then
  `last 8 KiB of the log:` + tail, or `last screen (captured at …, revision N):` + the stored text.

## 4. `Reaped` on the jobs actor (`jobs.rs:417-436`, handler `:1182-1261`)

`Reaped { restarted_at, jobs: Vec<LostJob { row, screen, tail, deliver }> }`. Per row: skipped if
the id is already known; `sj.next = max(next, n)` so new jobs number past the lost ones (`j7` keeps
naming the lost job for this engine's life); an in-memory `Job` with `exit: Some(Lost)`,
`secs = (settled − started)/1000`, `Log::from_tail(tail)` (`:499`) so `JobView.tail`/`jobs()` show
the log tail, `lost: Some(Lost { restarted_at, screen })`, `op: OperationId(0)` (never in `live`);
`JobEvent::Finished { exit: Lost, inline: deliver.is_none(), pty }` published on the bus for UIs;
when `deliver` is `Some`, a task admits `Kind::Job(id)` under the installed sink, `set_result`s
that event and drops — `Settle::FinishedWithResult` → the supervisor's existing `settle` →
`job_result(…)` → `inject`: today's `<job_result id=… command=… exit="lost" secs=… [pty="true"]
at=…>` with the §5.4 body. No supervisor change was needed.

## 5. PTY screens (`crates/tools/src/pty.rs`)

- `PtyHandle.snapshotted: AtomicU64` (`:507`, `u64::MAX` until the first write);
  `PtyHandle::snapshot_to_store(journal, session, id)` (`:642`): render the live page (scrubbed),
  skip when `revision == snapshotted`, else write `PtyScreenRow { revision, rows, cols, cursor,
  text (rows joined by '\n'), at_ms }` and remember the revision.
- Triggers: the `start` stub reply (`:1308`), every `send` / `screen` / `wait` return
  (`Live::snapshot`, `:1106`; `Screen` spawns it off the mailbox), the `snap` interval in the run
  loop (`:2097`, `MissedTickBehavior::Delay`), `KillAll`, and exit (`:2176`, after the exit
  revision bump).
- Lost PTY after a restart: `Jobs::lost(session, id)` (`jobs.rs:716`, exact id or name) and
  `Jobs::lost_page(session, id, note)` (`pty.rs:1162`) render
  `job j7 (name) · lost · Ns` / `[process gone — the engine restarted at HH:MM:SS; this is the last
  screen captured at HH:MM:SS (revision N)]` / `screen:` rows / `cursor: row r col c`,
  `structured.lost = true`. `Screen` (`:1511`) serves it; `SendInput` (`:1471`) serves it with
  the note `the terminal is gone (engine restarted at HH:MM:SS); last screen follows`; `wait` on a
  lost job keeps today's `j7 lost — start it again`. `PtyScreen` (the RPC's actor message,
  `:1622`) answers `Updated(stored_page(row))` — plain cells, `exit: Some(Lost)`, `epoch: None`,
  `cursor: Some(..)`, `retained: 0` — or `Unchanged` when `if_revision` matches
  (`stored_page`, `:2540`). `JobView.state` is `lost` through `Job::state()`; `JobView.tail` is the
  stored text for a lost PTY. Nothing in `crates/rpc/**` needed a change: `list_jobs` and
  `pty_screen` already forward to the actor.

## 6. The job rule — `crates/agent/src/loop_/resume_calls.rs::resume_job_call` (`:49`)

Signature changed (see deviations):
`resume_job_call(store: &Store, session: SessionId, call: &JournaledCall, plan: &ResumePlan)
 -> Option<ToolOutput>`.
- `shell` / `start` with a `jobs` row whose `call_id` is this call → `Some(ERROR: interrupted: the
  engine restarted at HH:MM:SS while this command|terminal ran (job j7, started HH:MM:SS); the job
  died with the engine and its exit code is unknown — check before repeating it.\nLog: <path>
  (last N lines follow)\n…)`, N ≤ 40 from the 8 KiB tail; a PTY appends the stored screen instead.
  `plan.kind == Suspended` words it `the engine was stopped at … and restarted`.
- `shell` / `start` with **no** row → `None`: the kill fell between approval and spawn; the caller
  treats the row as `planned` and runs it (§5.6 row 2).
- `send` → the same note for the job named by `args.id` (id or name); unknown → the plain
  interrupted note.
- `wait` → `lost_wait_reply` (`:105`): `job j7 (py) · lost · Ns` + the §5.3 banner + `screen:` +
  rows + `cursor:`; `is_error: false`; `structured.exit = "lost"`. Unknown job → interrupted note.
- Anything else → the interrupted note.

## Deviations from the design / brief

| what | design / brief says | built | why |
|---|---|---|---|
| `KillAll` row | `state = 'lost', exit = 'lost', settled_ms` | `state = running, exit = lost, settled_ms = shutdown time` | the schema has no "reported" mark and `jobs_open()` is the reaper's only input; a `lost` state would be final and the §5.5 promise ("on resume the killed jobs are reported … `killed at shutdown`") could never be kept. The reaper settles the row `lost` on the next start and the `exit = lost` stamp selects the shutdown wording. `jobs.exit` is still `lost` before the signal — the ordering the brief asks for, verified by `killall_writes_lost_before_kill` (the pgid answers signal 0 at write time). |
| `resume_job_call` signature | `(call, plan) -> ToolOutput` | `(store, session, call, plan) -> Option<ToolOutput>` | the rule needs the `jobs`/`pty_screens` rows, and "no row → planned" is not expressible as a `ToolOutput`. **W1's call site must pass `&store, session` and run the call when `None`.** |
| orphaned rows | "if the session row is missing → delete the row (and its pty_screens)" | counted in `orphaned`, settled `lost`, not reported | the contract exposes no job delete; `delete_session` already deletes the journal and `jobs.session_id REFERENCES sessions(id)` forbids an orphan, so the branch is defensive only |
| `<job_result>` shape | `log="…"` attribute in the §5.4 example | today's element exactly (brief: "the exact element shape … plus `exit="lost"`"); the log path is the body's second line | one wrapper (`supervisor::job_result`) |
| delivery path | `Agents::inject` from the reaper | an admitted `Kind::Job` operation whose guard settles with the `Finished { exit: Lost }` result | `reap_jobs` only has the store (contract signature) and `supervisor.rs` is not mine; the ledger path is the one every `<job_result>` already takes (47 §4.1), and it works for the `ActivityRegistered` ordering without asking back into the supervisor |
| `KillAll` after the drain window | §5.5 / §9.2 step 6 | trigger unchanged; `KillAll` made `pub` | W5 owns `Drain`; nothing exists to run after yet |
| §3.6 32-row cap on `jobs` rows | oldest finished rows deleted beyond 32 | not done | no delete API in the contract; the PK upsert overwrites a reused id the same way the log file is overwritten today |
| `pty_snapshot_secs` | `(call)` on config plumbing | read from the runtime figment in `jobs::start` | the `durability:` section is top level, `registry.rs` is not mine; `Jobs::new` keeps its signature (5 s default) |
| `wait` on a lost PTY (live call) | §5.6 row 5 is about a `started` `wait` row | the live `wait` tool keeps `j7 lost — start it again`; the resumed `wait` call gets the exited reply | a model calling `wait` after the restart already saw the loss; the `start it again` error is the actionable one |
| reaper test shape | "drop the actor WITHOUT KillAll" | the previous engine runs on its own tokio runtime that is `shutdown_background()`; the job is a PTY `trap '' HUP; echo armed; exec sleep 600` | dropping the actor alone leaves its tasks running and settling the row; a process that ignores the HUP the closing master sends is exactly the one the reaper exists for. `ShellTool::build_shell` is `pub(crate)` to `tools`, so the PTY path is the one reachable from `agent` |

Not done: `questions`/`waits` writes other than `job`/`pty_wait` (W3); any `loop_.rs` wiring (W1); UI
(W5); `just ts` (no type changed shape — `PtyPage`, `JobView`, `JobEvent` are as before; `JobExit`
already had `lost`).

## Cross-seam touches

None. Files changed are all in the owns list: `crates/tools/src/jobs.rs`, `crates/tools/src/pty.rs`,
`crates/agent/src/durability/jobs.rs`, `crates/agent/src/loop_/resume_calls.rs`. `shell.rs` and
`crates/rpc/**` needed no edit (the `job` wait lives in the actor's `Start` task, which knows the
job id; the RPC handlers forward to actor messages whose lost-job branches are in `pty.rs`).

## Tests

`tools` (`jobs::tests`, `pty::tests`; `RecJournal` records rows and whether the pgid answered
signal 0 at write time):
`job_rows_follow_the_job`, `killall_writes_lost_before_kill`, `grace_wait_writes_and_clears_a_job_wait`,
`reaped_jobs_list_lost_and_deliver_through_the_ledger` (one `Seen::Result` for the delivered job,
none for the batch's; `jobs()` row `j7  lost`; next id `j8`), `screen_snapshot_on_screen_and_timer`
(row at spawn with pgid; snapshot after `send`; `screen` with an unchanged revision writes nothing;
exit snapshot + `settled` row), `screen_after_restart_serves_banner` (banner text, `send` note,
`wait` error, `PtyScreen` page `exit = lost`/`epoch = None`/`Unchanged` on `if_revision`,
`JobView` `lost` with the stored text).
`agent` (`durability::jobs::tests`): `reaper_kills_leaked_group` (per §11.2: `killpg(pgid, 0)`
fails within 5 s, row `lost`, `jobs_open` empty, body + `job_result` wrapper carry `exit="lost"`
and the log path), `reaper_skips_nothing` (dead pgid, open-batch job, other session's
shutdown-stamped row: all three settled, delivery only where due, `killed at shutdown` wording),
`job_rule_notes` (job note with the last 40 lines, `None` without a row, PTY `start` note with the
screen, `wait` exited reply, `send` note).

## Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-target`, cargo 1.98.1.

Finding first: the shared target dir aliases worktrees. Cargo fingerprints workspace-relative
paths, so `agent`'s test binary built from `theuth-109-w3` (`deps/agent-f72a1a1a5fdc2982.d` listed
`theuth-109-w3` sources) was "fresh" for this tree and ran instead of mine (0 of my tests found,
a W3-only test failing). `cargo clean -p tools -p agent` helped once and was undone by the next
sibling build. From then on every gate ran with `RUSTFLAGS="-Cmetadata=w2"`: same target dir,
distinct fingerprint/filename hash, so nothing is shared with a sibling. Siblings may see the same
effect; the integrator should know.

1. `cargo test -p tools` — `160 passed; 0 failed` (lib) + `0 passed` (doc). 154 before + 6 new.
2. `cargo test -p agent` — `392 passed; 0 failed` (lib) + `0 passed` (doc). 389 before + 3 new;
   `reaper_kills_leaked_group` passes (the group is alive after the engine runtime is gone, dead
   within 5 s of `reap`).
3. `cargo clippy -p tools -p agent --tests` — 0 warnings, 0 errors (whole output, not only the
   touched files).
4. `rustfmt --edition 2024` on the four files touched.

## Open items for the integrator

- W1: `resume_job_call(&store, session, call, plan).await` — `None` ⇒ run the call as `planned`.
- W5: tell `KillAll` (now `pub`, re-export from `tools::lib.rs` as needed) after the drain window;
  the row stamp it writes is what makes the next start say `killed at shutdown`.
- W5 UI: `JobEvent::Finished { exit: "lost", inline }` is on the bus for every reaped job;
  `pty_screen` returns a page with `exit: "lost"` for a lost PTY — render the §5.3 banner over it.
- `JobEvent::Finished` for a reaped job is also the activity settlement's payload, so the
  supervisor's spill cap applies to the 8 KiB tail as to any job output.
