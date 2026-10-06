# 109 — Durability: kill the engine at any instant, start it again, and every session picks up exactly where it left off

Status: design v0 · 2026-10-06 · extends 07 (frames, resume §8.4), 08 (runtime dumps), 14 (jobs),
36 (pair invariant), 51 (lifecycle), 68 (replication), 69 (tasks), 96 (daemon), 104 (attention).
Recon in `109-recon.md` (read-only, verified; cited below as `109-recon.md §n`). Every `file:line`
in this document was read on `~/Develop/theuth-109-durability` at `ef1ad0a3` (2026-10-06); where a
recon citation had drifted the corrected line is used here and noted in §13. Waves write their
as-built notes as `docs/design/109-as-built-W<n>.md`. Calls the brief left open are marked `(call)`.

Revision 1 (2026-10-06 12:28): jobs are not durable — job adoption, no-kill shutdown and log-file
redirection removed after Sky's correction (*"I thought we decided jobs aren't durable"*). §0
R1/D3/D6/D8,
§2 I4, §3.1 `jobs`, §4.2–§4.6 job rows, §5, §8.4, §9.2, §10, §11, §12 W2, §13, §14 and §15 changed;
nothing
else moved.

Sky's words (2026-10-06 11:39): *"ok we need the full durability feature in theuth. I want to be
able
to kill the harness and re-open it and it just picks up exactly where it left off. create a design
doc
to plan the changes now."*

Rulings (ask_user, 2026-10-06 11:54):

1. **Work that lives in child processes** (shell jobs, PTYs, omp/opencode ACP workers): **Journal +
   native resume.** Jobs are journaled; after a restart, dead jobs are reported to the model as
   interrupted (logs and screens kept); ACP children are resumed through their own resume ids with a
   delta handoff. A detached job-keeper process (`t jobd`) was offered and not chosen for now (§14).
2. **A turn interrupted mid-flight**: **Auto-continue with safe replay.** Re-issue the lost model
   call
   from the persisted frame; re-run tool calls that had not started or whose tool is
   `Risk::ReadOnly`;
   a mutating or destructive call whose outcome is unknown is not re-run — its result says it was
   interrupted and its effect is unknown, and the model decides. No human needed; routines and
   subagents resume overnight. "Pause and ask" and "auto for routines, pause for chats" were offered
   and not chosen (§14).

## 0. Decisions in one place

| # | Decision | Recommendation / Ruled | Why |
|---|---|---|---|
| R1 | Child processes | **Ruled**: journal + native resume. **Jobs are not durable**: a shell or PTY job dies with the engine. Its `jobs` row exists so that on resume the model gets `<job_result exit="lost">` with the log path, `screen()` serves the last snapshot with a "process gone" banner, and a process group leaked by a SIGKILL is reaped on the next start (§5). ACP children resumed via `session/load` with a delta handoff (§7.3); no job-keeper daemon | the honest contract for a process the engine owns; the keeper stays a later stage (§14) |
| R2 | Interrupted turn | **Ruled**: auto-continue with safe replay (§4) | the frame is byte-identical, so the re-issued call is a cache hit; the only thing that cannot be replayed is an effect, and that is reported, not repeated |
| D1 | Where the journal lives | **`sessions.sqlite`, written inside the same `WriteTx` as every other store write** (`crates/agent/src/store.rs:164-266`); new tables `turn_steps`, `tool_calls`, `questions`, `waits`, `jobs`, `pty_screens` in migration `0033_durability.sql` (§3) | one commit = one `session_log` row, so 68 replicates the journal for free and a taken-over session can later resume elsewhere; no second file, no second fsync discipline |
| D2 | Resume at boot | **Eager `ResumeTurn` for every session with an open turn**, most recently active first, `durability.resume_parallel` (4) at a time; every other session stays lazy (§4.6) | today boot only marks (`unresolved`, `failed: interrupted_by_restart`, `lost`) and "auto-runs nothing" (`docs/design/00-architecture.md:484-485`); routines and children must come back without a human |
| D3 | Jobs and PTYs | **`jobs` + `pty_screens` tables for accounting and last-known state only; NO adoption; graceful shutdown keeps killing job groups exactly as today (`KillAll` after the drain window); at start, before any `ResumeTurn` runs, journaled pgids still alive (leaked by a SIGKILL) are killed and their rows settled `lost`, so a re-run never races a half-dead build; pipes stay as they are; behaviour identical on every OS** (§5) | reverses 14 §5's "no jobs table for v0" for accounting only; the model learns a job's fate instead of guessing |
| D4 | Streaming checkpoints | **`assistant_partial` row per model call, updated at content-block boundaries and every `partial_checkpoint_ms` (1000)**; never per token; the UI shows it as interrupted, the model never sees it (§8.1) | a kill mid-stream loses the text today (`crates/agent/src/loop_.rs:4025-4043`); the re-issued call regenerates it from the identical frame, so the checkpoint is for the human, not the frame |
| D5 | ACP children | **Respawn with `session/load` when a parent's tool call waits on the child; on a failed load, prompt a fresh child with a composed delta handoff** (§7.3); theuth-subagent children are sessions with open turns and are covered by D2 | `session/load` already exists on demand (`crates/agent/src/supervisor.rs:887-911`, `crates/acp/src/harness.rs:478-483`); only the trigger and the fallback are new |
| D6 | Graceful shutdown | **Drain up to `drain_secs` (10), checkpoint, mark open turns `suspended`, dump and release Monty, then `KillAll` the job groups as today (journaled `lost` first)**; SIGTERM, app close and `t update` converge on the same resume protocol (§9) | a planned stop should look like a crash to the resume code and only differ in wording |
| D7 | UI | **One session banner, one Attention item (104) when a human is needed; `AgentEvent::TurnResumed` / `TurnSuspended` / `PartialDropped`** (§8) | engine-first; as-built UI docs follow |
| D8 | Config | **`durability:` section** — `resume: auto \| ask \| off` (auto), `resume_parallel` 4, `drain_secs` 10, `partial_checkpoint_ms` 1000, `pty_snapshot_secs` 5 (§10); the job reaper has no key | `resume: ask` is the pause-and-ask behaviour as a setting, not the default |
| D9 | Out of scope | detached job keeper (later), process-surviving jobs by adoption (rejected by Sky, D14), event-sourcing rewrite, cross-engine resume of an in-flight turn (follow-on), meeting live meters, replaying tool side effects | §14 |
| D10 | Verification | **crash-injection harness** `THEUTH_CRASH_AT=<step>` aborting at every journal boundary; transcript equality with the uninterrupted run; job adoption and PTY snapshot tests; manual smoke (§11) | the invariant is testable only by killing |
| D11 | Build order | **W1 journal + safe replay → W2 jobs → W3 questions/waits → W4 ACP → W5 drain + UI → W6 crash suite**, seams in §12; W2 ∥ W3 and W4 ∥ W5 | file ownership keeps two omp waves apart |
| D12 | `fail_interrupted` | **Narrowed**: `result:<turn>` exchanges from `session:*` senders stay `accepted` and are delivered on resume; other accepted exchanges and `a2a_tasks` keep today's behaviour (§7.2) | a child's finished result is the one accepted exchange a restart must not throw away (`109-recon.md §10` fact 8) |
| D13 | Per-call tool journaling | **`tool_calls.finished` per call as each returns; the model-facing tool message stays one per batch and is assembled from the journal** (§3.3) | today one row after the whole batch (`loop_.rs:4202-4333`); a kill after call 1 of 3 must not re-run call 1 |
| D14 | Job adoption | **Rejected by Sky (2026-10-06 12:28)**: the orchestrator's v0 proposal — adopt live process groups at boot, stop killing them at shutdown, redirect job output to log-file fds and tail them — is withdrawn; §14 records it | "I thought we decided jobs aren't durable" — R1 means the job dies with the engine and the journal only reports it |

## 1. Today

`109-recon.md` is the factual base; the short version:

- Boot marks and never re-runs: open `model_calls` → `unresolved`
  (`crates/agent/src/store/accounting.rs:467-472`, called
  `crates/agent/src/supervisor.rs:7325-7330`), every `accepted` peer exchange and every non-terminal
  a2a task → `failed: interrupted_by_restart` (`crates/agent/src/store/communication.rs:1445-1476`,
  `supervisor.rs:7442-7450`), schedule runs → `lost` (`crates/schedule/src/store.rs:63`). Sessions
  load lazily on first input through `store.frame` (`supervisor.rs:596-628`); nothing is loaded at
  boot (`109-recon.md §2`, §10 facts 1–2).
- The turn loop under a kill (`109-recon.md §4`): the user row is committed before the ticket is
  acknowledged (`loop_.rs:3203-3213`); assistant text is persisted only at stream `End`
  (`loop_.rs:4041-4043`); the `tool_use` row is persisted before execution (`loop_.rs:4082`) and one
  tool message after the whole batch (`loop_.rs:4329-4333`); a dangling `tool_use` is repaired on
  the next turn start with `ERROR: not run: the harness exited before this tool completed` whether
  or not the effect happened (`loop_.rs:2758-2782`, called at `loop_.rs:3401`).
- Nothing in flight has a table: approvals (`crates/tools/src/approval.rs:95-96`), asks and
  hand-offs (`crates/tools/src/ask.rs:164-189`), `send_message` waits (`supervisor.rs:239-250`,
  `:7473`), jobs and PTY screens (`docs/design/14-jobs.md:304-311`;
  `crates/tools/src/pty.rs:467-504`), owned children / `last_stop` / `seen`
  (`supervisor.rs:186-199`). Reminders are the one in-flight waiter that survives: a row at set,
  re-armed at start, overdue fire `late` (`supervisor.rs:1949-1980`;
  `crates/tools/src/reminder.rs:289-290`).
- The Monty heap is dumped atomically after every cell (`crates/agent/src/repl.rs:1292`,
  `:1353-1379`); a worker crash tells the model `state restored to after the previous cell — this
  cell's effects are lost` (`repl.rs:1231-1244`), an engine kill tells it nothing but the "not run"
  repair.
- Every store write is one `WriteTx` that also appends a per-session `session_log` changeset
  (`store.rs:164-266`); it is a log of committed rows, not of intents (`109-recon.md §7`). The
  attached tables are the nine in `capture::ATTACHED` (`crates/agent/src/store/capture.rs:15-25`).
- Graceful shutdown SIGKILLs every job group (`crates/tools/src/jobs.rs:645-660`, `:976-994`,
  `:1018-1024`); under SIGKILL of the engine the groups are orphaned with their stdout on a pipe
  nobody reads (`jobs.rs:1066-1067`, drained at `:1264-1265`).
- ACP children resume only on demand: `sessions.acp_session` + stored `cwd` → `session/load`
  (`supervisor.rs:887-911`, `crates/acp/src/harness.rs:478-483`); a failed load logs and starts
  fresh (`harness.rs:554-557`, `crates/agent/src/acp_child.rs:1476-1495`).

Gap list (recon §8, numbered as there): 1 streaming text · 2 in-flight tool results and effects
recorded as "not run" · 3 approvals, asks, hand-offs · 4 `send_message` waits · 5
accepted-unconsumed exchanges and a2a tasks failed · 6 jobs, PTYs, screens,
`<job_result>`/`<job_idle>` · 7 Monty cell in flight, `inner_calls` left `output NULL` · 8
`last_stop`/`finished_at`, input dedup `seen` · 9 owned children / child turns / permits · 10
network a2a correlation · 11 per-session opt-outs, compaction overrides, pin · 12 sandbox pending
mounts, decision capture map · 13 meeting meters · 14 operation ids / live proof · 15 schedule run
in flight → `lost` · 16 ACP child process and context unless `session/load` succeeds. This document
closes 1–9, 15 and 16; 10–14 are either rebuilt by their owners on first use or explicitly out of
scope (§14).

The ten sharpest facts are `109-recon.md §10`; the ones this design turns on are 3 (user row before
ack, text only at End), 4 (tool-use row before execution, one result row per batch, "not run"
regardless of effect), 6 (every human wait is an oneshot), 7 (the write transaction is the
replication unit) and 8 (`fail_interrupted` eats a child's finished result).

## 2. Goal and invariants

"Exactly where it left off" means, precisely:

**Resume Invariant.** After any kill of the engine at any instant and a subsequent start, for every
session: (a) the transcript the model sees and the transcript the user sees are identical to those
of an uninterrupted run, except for the interruption notes defined in §4.3 — each of which names a
concrete interrupted thing; (b) no mutating or destructive effect is ever applied a second time by
the harness on account of the restart; (c) nothing that was accepted (an input, a child's result, a
peer message, a question's answer, a job's output) is silently dropped — it is delivered, or its
loss is written into the transcript.

Sub-invariants the sections below implement:

- **I1 Effects before results, journaled as such.** A tool call is `started` in the journal before
  its body runs and `finished` in the same transaction as its result; between the two, the effect
  may or may not have happened and the journal says exactly that (§3.3, §4.2).
- **I2 Waits survive.** Anything a turn is blocked on — a peer reply, a child result, a job, a human
  — is a `waits` row with its deadline; resume re-arms it with the time left instead of declaring
  the call not run (§4.4, §7).
- **I3 Questions survive.** A pending approval, `ask_user`, `desktop_handoff` or `secret_request` is
  a `questions` row; after a restart the user sees the same card, with the same id and the remaining
  deadline, and an answer given before the kill is honoured (§6).
- **I4 Jobs are accounted for, not revived.** Every shell/PTY job has a row; after a restart every
  running row ends `lost` — the process is dead, or is killed by the reaper if a SIGKILL of the
  engine leaked it — the log and the last PTY screen are kept, and the model receives exactly one
  report of the loss (§5).
- **I5 The frame is the truth.** The model never sees a checkpointed partial; a re-issued model call
  is built from the persisted frame alone, so it is byte-identical to the lost one and a provider
  cache hit (07 §8.4; §8.1).
- **I6 Pair invariant kept.** Every assistant `tool_use` row still gets exactly one tool message
  answering every call (36 §3); resume now fills it from the journal instead of from a constant.
- **I7 One writer, one transaction.** Every journal write is part of the `WriteTx` of the store
  operation it describes, so the journal can never be ahead of or behind the transcript it annotates
  (68 §3).

Not promised: an effect whose outcome is unknown is not made known — the model is told and decides;
a process that died with the engine is not brought back — its log and last screen are; a process
that outlived a SIGKILL of the engine is reaped, not adopted (R1).

## 3. The journal

### 3.1 Tables (migration `0033_durability.sql`)

The latest migration is `0032_thinking_ms.sql` (`crates/agent/migrations/`); 109 adds
`0033_durability.sql` with six tables. Names follow the repo's plural-snake style (`inner_calls`,
`reminders`, `model_calls`). Times are RFC 3339 text like `model_calls.created_at`
(`crates/agent/migrations/0015_session_lifecycle.sql:44-46`), except `jobs`/`pty_screens`, which use
unix milliseconds for cheap liveness arithmetic `(call)`.

```sql
-- 109 §3: one row per step of a turn. A turn is open while its newest row is
-- not `turn_finished`. `payload` is JSON whose shape depends on `kind`
-- (§3.2). `(session_id, turn_id, n)` orders the steps of one turn.
CREATE TABLE turn_steps (
    session_id  TEXT    NOT NULL REFERENCES sessions(id),
    turn_id     TEXT    NOT NULL,
    n           INTEGER NOT NULL,
    kind        TEXT    NOT NULL,   -- input_accepted | model_call | assistant_partial |
                                    -- tool_batch | turn_finished | suspended
    at_seq      INTEGER NOT NULL,   -- last message seq when the step was written
    call_id     TEXT,               -- model_calls.call_id for model_call / assistant_partial
    payload     TEXT    NOT NULL DEFAULT '{}',
    at          TEXT    NOT NULL,
    PRIMARY KEY (session_id, turn_id, n)
);
CREATE INDEX turn_steps_open ON turn_steps (session_id, kind, at);

-- 109 §3.3: one row per provider tool call, written in the same transaction
-- as the assistant `tool_use` row (planned), updated before the body runs
-- (started) and in the same transaction as the call's output (finished).
CREATE TABLE tool_calls (
    session_id  TEXT    NOT NULL REFERENCES sessions(id),
    call_id     TEXT    NOT NULL,
    turn_id     TEXT    NOT NULL,
    at_seq      INTEGER NOT NULL,   -- seq of the assistant row that carries the tool_use
    name        TEXT    NOT NULL,
    args        TEXT    NOT NULL,
    risk        TEXT    NOT NULL,   -- read_only | mutating | destructive (tools::Risk::as_str)
    state       TEXT    NOT NULL,   -- planned | started | finished | interrupted | skipped
    attempt     INTEGER NOT NULL DEFAULT 1,
    started_at  TEXT,
    finished_at TEXT,
    output      TEXT,               -- the spill-capped content (07 §6), NULL until finished
    is_error    INTEGER NOT NULL DEFAULT 0,
    files       TEXT    NOT NULL DEFAULT '[]',
    PRIMARY KEY (session_id, call_id)
);
CREATE INDEX tool_calls_turn ON tool_calls (session_id, turn_id, state);

-- 109 §6: a human wait. `tool_call_id` is the call that asked (NULL for an
-- approval raised by the registry before the call is `started`).
CREATE TABLE questions (
    id            TEXT    PRIMARY KEY,         -- ApprovalId / AskId / HandoffId / SecretRequestId
    session_id    TEXT    NOT NULL REFERENCES sessions(id),
    tool_call_id  TEXT,
    kind          TEXT    NOT NULL,            -- approval | ask | handoff | secret
    payload       TEXT    NOT NULL,            -- ApprovalRequested / AskRequested / … as JSON
    state         TEXT    NOT NULL,            -- pending | answered | superseded | timed_out
    asked_at      TEXT    NOT NULL,
    deadline      TEXT    NOT NULL,            -- asked_at + the kind's timeout
    answered_at   TEXT,
    answer        TEXT                         -- Decision / answers JSON / outcome
);
CREATE INDEX questions_pending ON questions (session_id, state);

-- 109 §4.4: what a `started` tool call is blocked on, so resume re-arms the
-- wait with the time left instead of declaring the call not run.
CREATE TABLE waits (
    session_id    TEXT    NOT NULL REFERENCES sessions(id),
    tool_call_id  TEXT    NOT NULL,
    kind          TEXT    NOT NULL,   -- peer_reply | child_result | job | pty_wait | question
    target        TEXT    NOT NULL,   -- exchange id / child session / job id / question id
    deadline      TEXT,               -- NULL = no deadline (a child result)
    payload       TEXT    NOT NULL DEFAULT '{}',   -- pty_wait: {pattern, cursor}; peer_reply: {request_id}
    PRIMARY KEY (session_id, tool_call_id)
);

-- 109 §5: every shell / PTY job, for accounting and last-known state — a job
-- is never revived. `pgid` is the leader's id (platform::ProcessTree::id), kept
-- so the reaper can kill a group a SIGKILL of the engine leaked.
CREATE TABLE jobs (
    session_id   TEXT    NOT NULL REFERENCES sessions(id),
    id           TEXT    NOT NULL,              -- j<n> per session
    kind         TEXT    NOT NULL,              -- shell | pty
    call_id      TEXT    NOT NULL,              -- the `shell` / `start` call
    command      TEXT    NOT NULL,
    name         TEXT,                          -- `start`'s label
    pgid         INTEGER,
    log          TEXT,                          -- log path; NULL for a quiet job
    started_ms   INTEGER NOT NULL,
    state        TEXT    NOT NULL,              -- running | settled | lost
    exit         TEXT,                          -- exited <n> | timeout | killed | lost
    settled_ms   INTEGER,
    PRIMARY KEY (session_id, id)
);
CREATE INDEX jobs_running ON jobs (state) WHERE state = 'running';

-- 109 §5.3: the last rendered screen of a PTY job, scrubbed (37 §3.4).
CREATE TABLE pty_screens (
    session_id   TEXT    NOT NULL REFERENCES sessions(id),
    job_id       TEXT    NOT NULL,
    revision     INTEGER NOT NULL,              -- PtyHandle.revision at capture
    rows         INTEGER NOT NULL,
    cols         INTEGER NOT NULL,
    cursor_row   INTEGER NOT NULL,
    cursor_col   INTEGER NOT NULL,
    text         TEXT    NOT NULL,              -- rows joined by '\n'
    at_ms        INTEGER NOT NULL,
    PRIMARY KEY (session_id, job_id),
    FOREIGN KEY (session_id, job_id) REFERENCES jobs (session_id, id) ON DELETE CASCADE
);
```

Two shorter changes ride in the same migration:

```sql
-- §4.5: the turn's end survives the process (109-recon.md §8 gap 8).
ALTER TABLE sessions ADD COLUMN last_stop   TEXT NULL;   -- stop_reason of the last finished turn
ALTER TABLE sessions ADD COLUMN finished_at TEXT NULL;   -- when it finished
```

### 3.2 `turn_steps` kinds and payloads

| kind | written | payload |
|---|---|---|
| `input_accepted` | in `take_input`'s transaction, beside the user row (`loop_.rs:3203`) — one row per input the turn took | `{"input_id": …, "source": "user\|peer\|inject\|replay"}` |
| `model_call` | in the same transaction as `reserve_call` (`accounting.rs:331-362`); `call_id` set | `{"request_id", "attempt", "kind"}` |
| `assistant_partial` | one row per `model_call`, **updated in place** at every content-block boundary and on the `partial_checkpoint_ms` timer when the captured content grew (§8.1) | `{"content": <MessageContent JSON>, "thinking_ms": …}` |
| `tool_batch` | in the transaction that persists the assistant `tool_use` row (`loop_.rs:4082`), together with the batch's `tool_calls` rows in `planned` | `{"call_ids": […], "round": n}` |
| `turn_finished` | in the transaction of the final assistant row (`loop_.rs:4041-4043`), or of the cancel/error path's last row | `{"stop_reason": …}` |
| `suspended` | by the graceful-shutdown drain (§9) when the turn could not finish inside `drain_secs` | `{"reason": "shutdown\|update\|parent_gone"}` |

A turn is **open** when its newest row is anything but `turn_finished`. `suspended` is open: it only
changes the wording of the resume (§8.2). `at_seq` anchors every row to the message it belongs with,
so `commit_cut`, fork and undo treat journal rows the way they treat `errors` and `inner_calls`
(`crates/agent/migrations/0020_inner_calls.sql:1-6`): a cut below `at_seq` deletes the row.

### 3.3 Write points in the loop

The loop today, with the journal writes added (each "+" is inside the transaction of the existing
write it names; nothing adds a transaction):

1. `take_input` persists the user row (`loop_.rs:3203`) **+ `turn_steps(input_accepted)`**. The
   ticket is acknowledged after the commit as today (`:3208-3213`), so an acknowledged input is
   always journaled.
2. `Reserved::open` reserves and dispatches the model call (`loop_.rs:2137-2173`;
   `accounting.rs:331-380`) **+ `turn_steps(model_call)`** in `reserve_call`'s transaction.
3. `stream_once` captures content in memory (`loop_.rs:3044`); **+ `assistant_partial` upserts** on
   the cadence of §8.1. These are the only journal writes that are not piggy-backed on an existing
   write; they are small `UPDATE`s of one row, so each produces one short `session_log` changeset.
4. Text reply: `persist_timed(assistant)` (`loop_.rs:4041-4043`) **+ `turn_steps(turn_finished)`**
   when the round yields.
5. Tool reply: `persist_timed(assistant tool_use)` (`loop_.rs:4082`) **+ `turn_steps(tool_batch)` +
   one `tool_calls` row per call in `planned`**, `risk` from the registry's spec for registry tools,
   `read_only` for the loop-local `todo` and `python` (`crates/agent/src/repl.rs:117`), `args` as
   sent.
6. Per call, immediately before the body runs — for registry tools inside `ToolRegistry::invoke`
   after the approval resolved and before anything is expanded or dispatched
   (`crates/tools/src/registry.rs:1501-1533`); for `python` just before `Repl::run`
   (`loop_.rs:4440`): **`tool_calls → started`, `started_at`**. This is a store write from the tools
   crate; it goes through a new `tools::Journal` trait object the agent installs on the registry
   (like `InstallActivity`, `jobs.rs:997-1007`), so `tools` keeps no `sqlx` dependency `(call)`.
7. Per call, as each returns — the registry's `dispatch` completion for registry tools, the `python`
   branch's return (`loop_.rs:4183-4192`): **`tool_calls → finished`, `output` (spill-capped exactly
   as `loop_.rs:4221-4227` does today), `is_error`, `files`**. This is the change behind D13: the
   result row is per call, committed when the call returns, even though the model-facing message is
   still assembled once per batch.
8. Batch end: the one tool message is built and persisted as today (`loop_.rs:4329-4333`, or
   `append_with_todos` `:4336-4339`). The content of each `ToolResponse` is read **from the
   `tool_calls` rows** (not from the in-memory `outs`), so the message the model sees is by
   construction what the journal says — on a normal run they are the same bytes; on a resumed run
   they differ exactly by the §4.3 notes.
9. Cancel path (`cancelled by user`, `loop_.rs:4197`) marks the unstarted rows `skipped` and the
   started ones `finished` with the cancel text; landing (`:4086-4101`) marks them `skipped` with
   `NOT_RUN`.

Inner calls are unchanged: `inner_call_started`/finished (`store.rs:1392-1456`) already give the
per-inner-call journal the §4.2 python rule reads.

### 3.4 Relationship to the existing tables

| table | relationship |
|---|---|
| `messages` | still the only thing in the frame. `turn_steps.at_seq` / `tool_calls.at_seq` point into it; nothing in the journal is ever rendered into a frame except through the §4.3 notes, which become ordinary `ToolResponse` content |
| `model_calls` | `turn_steps.call_id` → `model_calls.call_id`; the accounting status (`unresolved` at boot, `accounting.rs:467-472`) is untouched — a re-issued call is a new attempt of the same `request_id` (`loop_.rs:2941-2956` already loops attempts) |
| `inner_calls` | the python-cell sub-journal; read by §4.2, not written by 109 |
| `peer_exchanges` | the durable side of a `send_message`; `waits(kind = peer_reply)` points at an exchange id; `fail_interrupted` narrowed (§7.2) |
| `reminders` | unchanged; already the model for "row at set, re-arm at start" |
| `epochs`, `todo_snapshots`, `errors` | unchanged; journal rows anchor on `at_seq` like `errors` |
| `session_log` | gains the journal tables' changesets (§3.5) |

### 3.5 Replication through `session_log`

`capture::ATTACHED` lists the nine tables whose changes the write connection's `sqlite3session`
records (`capture.rs:15-25`); every attached table must have a column naming its session
(`store.rs:2564-2567`). 109 attaches `turn_steps`, `tool_calls`, `questions`, `waits` and `jobs` by
`session_id`. `pty_screens` is **not** attached `(call)`: a screen row rewrites up to ~5 KB every
few seconds per live PTY and a process never outlives its engine anyway (R1), so the row is
engine-local like `peer_exchanges` (68 decision 8). Consequences:

- A taken-over session (68 §4.2) arrives on the taker with its open turn, its tool-call states, its
  questions and its waits — enough for the follow-on "resume on another engine" (§14) without a
  further schema change.
- `WriteTx::commit`'s owner fence applies to journal rows (`store.rs:221-247`): a non-owner cannot
  journal, which is right.
- Volume: `assistant_partial` upserts are the only new high-frequency write — ≤ 1 per second per
  streaming session, each a one-row `UPDATE` changeset of a few hundred bytes plus the delta of the
  text. 68 §6's snapshot prune bounds `session_log` as before. `(call)` if a transcript shows the
  log growing faster than the prune, lower the cadence per session with `partial_checkpoint_ms`; do
  not move the row out of the log.

### 3.6 Retention

- `turn_steps`, `tool_calls`, `waits` rows of a **finished** turn are deleted when the turn's
  `at_seq` falls below a compaction cut (as `errors`/`inner_calls`) and, independently, by the
  weekly spill sweep (`supervisor.rs:7357-7428` runs `spill::sweep` at start) once older than
  `durability.journal_keep_days` (7) `(call)`. Open turns are never pruned.
- `questions` rows in a terminal state are deleted with their turn; pending ones never age out by
  the sweeper — they time out (§6.3).
- `jobs` rows: finished rows are kept up to the in-memory cap of 32 per session (`jobs.rs:544-554`,
  `MAX_ROWS`) and the oldest finished are deleted beyond it, so `jobs()` after a restart lists what
  it listed before; `pty_screens` cascade.

## 4. The resume protocol

### 4.1 `ResumeTurn`

A new supervisor message, `ResumeTurn { session, turn, kind: Interrupted | Suspended }`. The
supervisor creates the actor exactly as `Agents::session` does for a lazy load
(`supervisor.rs:596-628`: row, `store.frame`, todos, `resumed = true`) and then, instead of waiting
for an input, hands it `Turn::resume(plan)` where `plan` is computed from the journal by the store:

```rust
pub struct ResumePlan {
    pub turn: TurnId,
    pub kind: ResumeKind,                     // Interrupted (crash) | Suspended (drain)
    pub interrupted_at: DateTime<Utc>,        // newest journal row's `at`
    pub last: LastStep,                       // what to act on (§4.2)
    pub calls: Vec<JournaledCall>,            // the open batch's tool_calls rows, if any
    pub partial: Option<PartialContent>,      // assistant_partial of the open model call (§8.1)
}
```

`Turn::resume` rebuilds the frame from `store.frame` (07 §8.4: head + tail, byte-identical to what
the model last saw), runs `rebase` as a normal turn start does (`loop_.rs:3400`), and **does not
call `repair_dangling`** — the journal replaces it (§4.2). `repair_dangling` stays as the
last-resort path for a frame with a dangling `tool_use` and *no* `tool_calls` rows (a transcript
written before 0033): its message is kept verbatim for that case only.

### 4.2 State machine: what the newest journal step says, and what resume does

| newest step | meaning | resume action |
|---|---|---|
| `input_accepted` (no `model_call` after it) | the input is in the frame, the turn never reached the model (today: "auto-runs nothing", `00-architecture.md:484-485`) | start the turn now: the frame ends in a user message, so this is an ordinary `round` |
| `model_call` (no `tool_batch`/`turn_finished` after it) | the call was dispatched and never settled; accounting already marked it `unresolved` at boot (`accounting.rs:467-472`) | re-issue the call from the frame as a new attempt: same `request_id`, `attempt + 1`, through the existing attempt loop (`loop_.rs:2941-2956`). The `assistant_partial`, if any, is published to the UI as dropped (§8.2) and discarded from the frame. No note to the model: the frame is identical, so the regenerated reply is the reply |
| `tool_batch` with every `tool_calls` row `finished`/`skipped` | every call returned; the kill fell between the last `finished` and the batch message | assemble the batch message from the rows and persist it (§3.3 step 8); continue the loop with the next `round` |
| `tool_batch` with rows in `planned`/`started` | the batch was running | apply the per-call rule below to each row, run what it says (planned and read-only calls in parallel as a normal batch, python cells after, as `loop_.rs:4115-4193`), then assemble and persist the batch message, prepending the `<engine_restart>` note (§4.3) to the first response, and continue |
| `turn_finished` | the turn is closed | not open; the session is loaded lazily as today. If `sessions.last_stop` is NULL but the row exists (a pre-0033 session), nothing to do |
| `suspended` | a drain could not finish the turn (§9) | identical to the row *before* it, with `kind = Suspended` wording |
| final assistant row persisted but no `turn_finished` row | the kill fell between `persist_timed` and the step write — impossible by I7 once both are in one transaction; listed for the pre-0033 case | treat as `turn_finished`; backfill `last_stop = "end_turn"`, `finished_at = messages.at` |

**Per-call rule** (R2), by `tool_calls.state` and `risk`, with the tool-specific cases that override
it:

| state | risk | action | note to the model |
|---|---|---|---|
| `finished` | any | reuse `output`/`is_error`/`files` from the row | none |
| `skipped` | any | reuse the row's text (`cancelled by user`, `NOT_RUN`) | none |
| `planned` | any | run now (approval asked again if the gate wants it, §6) | none |
| `started` | `read_only` | run again as `attempt + 1`; the first attempt only observed | none |
| `started` | `mutating` / `destructive` | **do not run**; row → `interrupted` | the *interrupted* note (§4.3) |
| `started` | `python` (spec risk `read_only`, `repl.rs:117`) | the **python rule** below | the *cell* note |
| `started` | `shell` (`destructive`, `crates/tools/src/shell.rs:266`) | the **job rule** (§5.4): row → `interrupted`; the job, if it was spawned, is dead or reaped and its row is `lost` | the *job* note: the interrupted note plus the job id, log path and log tail |
| `started` | `start` (`destructive`, `pty.rs:2424`) / `send` (`mutating`, `pty.rs:2463`) | job rule; the PTY is gone; `send`'s typed bytes are an unknown effect | interrupted note plus the last screen (§5.3) |
| `started` | `wait` (`read_only`, `pty.rs:2514`) | the job is gone after any restart (R1): the tool's own "exited" reply with the last screen and `exit="lost"` | none |
| `started` | `send_message` with `await_reply` | the **wait rule** (§4.4) | none, or the wait's own expiry text |
| `started` | `spawn_subagent` | if a child session row with `parent = this` was created by this call (the call id is stored on the child's `sessions` row `(call)`) the effect happened: finish the row with the child's id and the child's current state; else `planned` | none |
| `started` | `remind_me_in` (`read_only`, `reminder.rs:405`) | a `reminders` row for this session with `set_at` ≥ `started_at` exists → finish with its `{id, fires_at, seconds}`; else run again | none |
| `started` | `ask_user` / `desktop_handoff` / `secret_request*` (`ask_user` is `read_only`, `ask.rs:253`) | the **question rule** (§6.2): re-publish the pending `questions` row, same id, remaining deadline; do not create a second question | none |
| `started` | `todo` (loop-local, working copy committed with the batch, `loop_.rs:4169-4179`) | run again; its effect is the batch's `todo_snapshots` row, which was never written | none |

The order of precedence is: tool-specific row above → `risk` row. The table is the whole policy;
nothing else in the engine decides whether to re-run a tool after a restart.

**Python rule.** A `started` `python` cell has `inner_calls` rows for each tool call it made
(`store.rs:1392-1456`): a row with `output` is finished, a row with `output NULL` was started and
never finished. Let F = the finished inner calls, U = the unfinished ones, each with the risk of its
tool from the registry's current spec.

- If every call in F ∪ U is `read_only` → re-run the cell as `attempt + 1` from the previous heap
  dump (08 §5: the dump after the previous cell is the state the cell started from). No note.
- Otherwise → do not re-run. The cell's result is the cell note (§4.3) listing F (name, ok/error)
  and U (name, unknown); `Repl::open` restores the previous dump as it does after a worker crash
  (`repl.rs:1231-1244`). The inner rows in U are set to `is_error = 1, output = "interrupted by an
  engine restart; effect unknown"` so the reloaded transcript shows them closed.

The dump is only written after a *completed* cell (`repl.rs:1292`, `:1353-1379`), so "the previous
dump" is exactly the state before the interrupted cell — the model-facing wording from 08 (`state
restored to after the previous cell`) is literally true.

### 4.3 Model-facing wording

Every note is a `ToolResponse` content, prefixed `ERROR:` where today's loop would
(`loop_.rs:4228-4237`), so the model's existing handling of tool errors applies. Times are local,
`HH:MM:SS`, the restart gap stated once.

The **restart note**, prepended to the first response of a resumed batch the way the clock stamp is
(`loop_.rs:4317-4324`), and the only turn-level trace of the restart:

```
<engine_restart at="12:05:02" after="81s" kind="crash" replayed="2" interrupted="1"/>
```

`kind` is `crash` (no `suspended` row) or `shutdown`; `replayed` counts calls run as a new attempt;
`interrupted` counts rows marked `interrupted`. When the batch had nothing to replay or interrupt
(every row was `finished`), the note is still prepended: the model should know a gap of `after`
seconds passed `(call)`.

The **interrupted note** (`started` mutating/destructive, not a job):

```
ERROR: interrupted: the engine restarted at 12:05:02 while this call was running (started 12:03:41);
its effect is unknown — check before repeating it.
```

The **job note** for a `shell` call that was `started` when the engine died. The job is gone (R1):
dead with the engine, or killed by the reaper (§5.4) if a SIGKILL leaked it. The call's result is
the interrupted note extended with the job row:

```
ERROR: interrupted: the engine restarted at 12:05:02 while this command ran (job j7, started 12:03:41); the job
died with the engine and its exit code is unknown — check before repeating it.
Log: /Users/sky/.theuth/jobs/<session>/j7.log (last 40 lines follow)
…
```

No separate `<job_result>` follows for this job: the model never received its stub, so the note is
its one report. A job whose `shell` call had already returned the `{"job": …}` stub in an earlier
batch or turn is reported instead by an injected `<job_result exit="lost">` (§5.4). A `shell` call
`started` with no `jobs` row (the kill fell between approval and spawn) is treated as `planned` —
nothing ran.

The **cell note** (`python`, mixed effects):

```
RuntimeError: the engine restarted at 12:05:02 while this cell ran; state restored to after the previous
cell — this cell's effects are lost from the runtime, not from the world. Inner calls that finished before
the restart: read_file (ok), http_request (ok). Started, outcome unknown: write_file.
```

The **wait expiry** texts are the tools' own (`send_message`'s timeout receipt, `wait`'s `timed
out`, the approval's `approval timed out`, `registry.rs:1526-1528`) — a restart that eats the whole
deadline looks like a timeout, not like a restart.

### 4.4 Waits

A `started` call that blocks on something external writes a `waits` row when it begins blocking and
deletes it when it unblocks (I2). Kinds and the resume action:

| kind | written by | target | resume |
|---|---|---|---|
| `peer_reply` | `send_message{await_reply}` once the exchange is accepted (`supervisor.rs:239-250` holds it in memory today) | the exchange id; `payload.request_id` | a `replied` exchange exists → deliver its reply as the call's result; else re-create the `Wait` with `deadline - now`; ≤ 0 → the timeout receipt |
| `child_result` | the same tool when the peer is an owned child's `result:` (and `spawn_subagent`'s implicit wait on its child's first turn) | the child session id | a `result:<turn>` exchange from `session:<child>` in `accepted` → deliver it (§7.2); else the child is itself a session with an open turn and is resumed by §4.6; the parent's wait is re-created with no deadline |
| `job` | the `shell` call while it waits out `job_grace_secs` (14 §2) | the job id | the job rule (§5.4): the job is gone, the call gets the job note; the grace is not re-waited |
| `pty_wait` | `wait` | job id; `payload.pattern`, `payload.cursor` (the log read cursor, `pty.rs:500-503`) | the job is gone after a restart: the tool's exited reply with the last screen; the row exists so a *drain* (§9) that outlives the wait can still answer it |
| `question` | the human-wait tools and the registry's approval gate | the question id | the question rule (§6.2) |

Re-arming reuses the in-memory structures as they are (`Agents.waits`, the jobs actor's waiters);
the row is the source they are rebuilt from, in the same way `rearm_reminders` rebuilds the reminder
timers from `reminders` (`supervisor.rs:1953-1980`).

### 4.5 Turn end bookkeeping

`TurnFinished` is published after the final persist; `last_stop` lives in `Agents.last_stop` and is
lost on restart (`supervisor.rs:190-191`, "ponytail: process-local"). 109 writes
`sessions.last_stop` and `sessions.finished_at` in the same transaction as the `turn_finished` step,
and `Agents::session` reads them when it loads a row, so a cold child after a restart reports its
real `finished_at` (the known WAKE-LIVE-CHILD blindness, `109-recon.md §4`). `seen` (input dedup,
`supervisor.rs:195-197`) is not persisted: `turn_steps(input_accepted).payload.input_id` is queried
instead when a retried input id arrives for a session with a journal `(call)`.

### 4.6 Boot ordering

Today's sequence is `109-recon.md §2` steps 1–13. With 109:

| step | today | 109 |
|---|---|---|
| 2 `Store::open` | migrations, seeds | + `0033` |
| 3 `unresolve_open_calls` | marks | **stays** — the accounting row of a lost call is `unresolved`; the resumed turn reserves a new attempt |
| 8 `fail_interrupted` | fails every accepted exchange and every non-terminal a2a task | **narrowed** (§7.2): `WHERE state = 'accepted' AND NOT (sender LIKE 'session:%' AND request_id LIKE 'result:%')`; a2a unchanged |
| 9 `Agents` spawned | maps empty | unchanged; `waits`, `approvals` etc. are rebuilt per session by `ResumeTurn`, not globally |
| 10 `ActivityRegistered` → `rearm_reminders` | | + **`reap_jobs`** (§5.4) right after the reminders — every `running` row's pgid that is still alive is killed, every `running` row is settled `lost`, and `<job_result exit="lost">` is injected for jobs whose stub the model already had; then **`resume_scan`**: `SELECT session_id, turn_id, MAX(at) FROM turn_steps GROUP BY session_id, turn_id HAVING kind-of-newest != 'turn_finished' ORDER BY MAX(at) DESC` (written as a window query in the store), each row → `tell(ResumeTurn)` through a `Semaphore(resume_parallel)` |
| 11 lazy `Agents::session` | | unchanged for every session without an open turn |
| 12 schedule plugin marks runs `lost` | `UPDATE runs SET result = 'lost' WHERE finished_at IS NULL` (`schedule/src/store.rs:63`) | the schedule plugin starts after `agent` (`crates/theuth/src/main.rs:1546-1591`); it now asks the supervisor `OpenTurn(session)` before closing a row and keeps the run open (re-attaching its watcher to the session's `TurnFinished`) when the session is being resumed; `lost` only when it is not `(call)` |
| 12 tasks | releases `dispatching` claims | unchanged; a desk whose turn resumes is not idle, so the sweep's liveness test sees a running session |

`durability.resume = ask`: `resume_scan` publishes an Attention item per open turn (§8.3) and
`ResumeTurn` runs when the user presses Resume; `off`: the scan runs nothing and the next input into
the session resumes it lazily (the journal still applies — the per-call rule runs at that point
instead of at boot). Private trees (`:memory:` stores, 51 §4) have no journal to resume and are
unaffected.

Ordering invariant: no `ResumeTurn` runs before `reap_jobs` has finished, so a replayed or re-issued
command never races a half-dead build left by the previous process, and a `shell` row's job rule
always finds its job settled `lost`; and no lazy `Agents::session` for a session with an open turn
is served before its `ResumeTurn` has been queued — `Agents` checks the open-turn set it built in
step 10 and routes the input to the resuming actor as a queued steer.

## 5. Jobs and PTYs

Jobs are **not durable** (R1). A shell or PTY job is a child of the engine process and dies with it;
109 does not try to keep it alive or re-attach to it. What 109 adds is accounting: a `jobs` row so
the model and the UI learn the job's fate after a restart, a `pty_screens` row so `screen()` can
show the last thing the model saw, and a reaper so a process group that a SIGKILL of the engine
leaked (`109-recon.md §1`: own pgid, stdout on a pipe nobody reads) is killed on the next start
instead of running half-dead beside a replayed command. Behaviour is identical on every OS: on
Windows the Job Object's `KILL_ON_JOB_CLOSE` already kills the group with the engine
(`crates/platform/src/process.rs:24-27`, `:255`); on Unix the reaper does what the OS did not.

### 5.1 The table and its writes

`jobs` (§3.1) mirrors the in-memory `Job` (`jobs.rs:479-501`) the way `reminders` mirrors the
reminder timers. Writes, each through the `tools::Journal` seam of §3.3 step 6:

| event | write |
|---|---|
| `Start` spawned the process (the `on_spawn` hook reports the first `(pid, pgid)`, `jobs.rs:1079-1092`) | `INSERT jobs (…, state = 'running', pgid, log, started_ms)` |
| a later pipeline member joins | no write (one group) |
| exit / timeout / cancel (`JobExit`, `jobs.rs:78-87`) | `state = 'settled', exit, settled_ms` |
| `KillAll` at shutdown (§5.5) | `state = 'lost', exit = 'lost', settled_ms` — written **before** the kill |
| reaper at start (§5.4) | `state = 'lost', exit = 'lost', settled_ms` |
| session deleted | rows deleted (the log directory already goes) |

The `call_id` column lets a resumed `shell`/`start` tool call find its job (§4.2); `command`, `name`
and `log` let `<job_result>` and `jobs()` read as today without the actor's memory. Pipes, drains
and the log writer stay exactly as they are (`jobs.rs:1066-1067`, `:1225`, `:1264-1265`).

### 5.2 Why a job dies with the engine, and why that is the contract

A shell job's stdout and stderr are pipes the engine drains into the log (`jobs.rs:1066-1067`,
`:1264-1265`); a PTY job's output is the master side of a pseudo-terminal the engine holds
(`PtyHandle.master`, `pty.rs:481`). When the engine dies the pipe loses its reader and the master
closes: the child gets `SIGPIPE` on its next write or `SIGHUP` at once, and nearly everything exits.
A child that happens to survive (it ignored the signal, or has not written yet) is a process nobody
can observe, steer or wait on. Keeping such a process alive and pretending to own it was the
orchestrator's v0 proposal (adoption, log-file fds, tailing) and Sky rejected it (D14): the only
honest statement after a restart is *this job is gone; here is its log*. The detached keeper (§14)
is the way to make jobs genuinely durable later, behind the same rows.

### 5.3 PTY screen snapshots

`PtyHandle` renders the screen from a `vt100::Parser` and bumps `revision` after every processed
chunk (`pty.rs:469-474`); the `screen` tool, `JobView.tail`, `JobEvent::Screen` and the final
`<job_result>` all read `PtyHandle::screen()` (`pty.rs:569-576`), which scrubs vault values. 109
adds `PtyHandle::snapshot_to_store`: write a `pty_screens` row (`rows`, `cols`, cursor, scrubbed
text, `revision`) **when the revision changed since the last snapshot**, on three triggers:

- every `screen()` / `send()` / `wait()` return — the model just saw this screen, so this is what
  "where it left off" means for a PTY;
- a timer, `durability.pty_snapshot_secs` (5) `(call)` — bounds the loss for a busy TUI nobody is
  reading;
- the job's exit, with the final screen.

Equal revisions skip the write (51 §7.3 already defines the revision for exactly this). A 120×40
screen is ≤ 5 KB; at most one write per 5 s per live PTY, engine-local (§3.5).

After a restart, `screen(id)` for a `lost` PTY returns the stored rows with the banner on top:

```
[process gone — the engine restarted at 12:05:02; this is the last screen captured at 12:04:58 (revision 418)]
```

`send(id)` answers `the terminal is gone (engine restarted at 12:05:02); last screen follows` with
the same rows, and `jobs()` shows the row as `lost (pty)`.

### 5.4 The reaper at start (`reap_jobs`)

Runs once after `rearm_reminders` and **before** `resume_scan` (§4.6 step 10); always on, no config
key — a test calls the function directly on a store. For every `jobs` row in `running`:

```
if pgid alive (wrap pgid in a ProcessTree, process.rs:28-30; alive() = killpg(pgid, 0), :154-156)
                                → kill() the group (:45-47), log "reaped job <id> of <session> leaked by a restart"
settle: state = lost, exit = lost, settled_ms = now
if the session row is missing   → delete the row (and its pty_screens)
elif the job's call_id is a `started` tool_calls row of an open turn
                                → nothing more: the job rule (§4.2) reports it inside the resumed batch
else                            → publish JobEvent::Finished { exit: Lost, output: last 8 KiB of the log (TAIL, jobs.rs:43) }
                                   and inject <job_result exit="lost"> (Agents::inject, supervisor.rs:1114; 14 §3)
```

`JobExit` gains `Lost` (`jobs.rs:78-87`; display `lost`), the value 14 §1.2 reserved for exactly
this. The injected result:

```
<job_result id="j7" command="cargo build" exit="lost" secs="…" log="/…/j7.log">
this job died with the engine (restarted at 12:05:02); its exit code is unknown. Last 8 KiB of the log:
…
</job_result>
```

For a PTY job the body is the stored screen instead of the log tail, as a PTY's final `<job_result>`
is today (`pty.rs:569-572`). Injection goes into a running turn after its batch, else as a new turn,
like every `<job_result>` (14 §3) — so a session whose turn is being resumed sees the loss right
after the resumed batch, and an idle session is woken by it.

### 5.5 Graceful shutdown keeps killing job groups

`tools::jobs::start` tells `KillAll` on `rt.shutdown` (`jobs.rs:645-660`) and the handler SIGKILLs
every running group, aborts the task and releases the activity charge as `killed at shutdown before
it settled` (`jobs.rs:976-994`), publishing no `Finished` (14 §1.2 as-built). Unchanged, with one
addition and one reordering: the handler writes each running row `lost` **before** it kills (so a
crash during the kill still leaves the truth on disk), and it runs **after** the drain window of
§9.2 rather than at the instant the token fires, so a `shell` call that would have returned inside
`drain_secs` returns normally. On resume the killed jobs are reported as `<job_result exit="lost">`
with the body `killed at shutdown (engine stopped at 12:05:02)` — today they vanish (`109-recon.md
§5`). `CloseSessions` (`jobs.rs:362`), `cancel_job`, the deadline and `/jobs kill` are untouched.

### 5.6 What the model is told, in one place

| situation | reply |
|---|---|
| `shell` row `started`, job row exists | the job note (§4.3): interrupted, job id, log path, log tail; the row is `lost`; no further `<job_result>` |
| `shell` row `started`, no `jobs` row (the kill fell between approval and spawn) | the job never started: `planned` → run now |
| job running in the background (its stub already delivered) when the engine died or was stopped | `<job_result exit="lost">` injected on the next start, with the log tail or the last screen |
| PTY `start`/`send` row `started` | the interrupted note with the last screen; `screen()` thereafter shows the banner |
| `wait` row `started` | the tool's exited reply, `exit="lost"`, with the last screen |
| `jobs()` | lost rows as `lost`, with the log path; finished rows as before |

## 6. Questions

### 6.1 The four kinds and where they are raised

| kind | raised by | today | timeout |
|---|---|---|---|
| `approval` | `ToolRegistry::invoke` before the body (`registry.rs:1513-1531`), through `Approval` (`approval.rs:110-147`) | `pending: HashMap<ApprovalId, (ApprovalRequested, oneshot)>` (`approval.rs:96`), timeout → `Deny` | `tools.approval_timeout_secs` 300 (`crates/tools/src/lib.rs:454`, `:483`) |
| `ask` | the `ask_user` tool body, through `AskUser` (`ask.rs:164-199`) | `pending` oneshot, timeout → dismissed | `tools.ask_timeout_secs` 1800 (`crates/tools/src/lib.rs:456`, `:484`) |
| `handoff` | `desktop_handoff` (roster) | in memory | its `timeout_secs`, default `sandbox.handoff_timeout_secs` |
| `secret` | `secret_request` / `secret_request_form` (`crates/secrets/src/tools.rs:148`) | in memory | the tool's own |

Each actor gains a `questions` row on raise (`state = pending`, `deadline = asked_at + timeout`) and
an update on resolve (`answered`, `answer`; or `timed_out`). The write goes through the same
`tools::Journal` seam (§3.3 step 6); the actors keep their oneshots for the live path. The
`AttentionKind::Ask` item (104 A1) is unchanged — it already derives from the actor's pending set,
which is now rebuilt from the table.

### 6.2 The two resume paths

Decided, per the brief's recommendation:

- **Approvals re-ask naturally.** A tool call waiting for approval is still `planned` (approval
  precedes `started`, §3.3 step 6 — and precedes `Start` for jobs, so a job never runs unapproved,
  `docs/design/14-jobs.md:322-324`). Resume runs the `planned` call, the registry gates it again, a
  new `questions` row is raised with a fresh id and the old pending row is set `superseded`. The
  user sees the same card content again; the deadline restarts. One exception `(call)`: if the old
  row is `answered` with `allow` and the call is still `planned` — the kill fell in the microseconds
  between the answer and `started` — the registry honours the stored answer and does not ask again;
  a `deny` is likewise honoured (the call is `skipped` with `denied by user`).
- **Human-wait tools re-publish.** `ask_user`, `desktop_handoff`, `secret_request*` are `started`
  while they wait (their body *is* the wait). Resume does not re-run them: the actor is told
  `Republish(question)` and inserts the row's `(id, session, payload, oneshot)` back into `pending`
  with `deadline - now` as the timeout, then publishes the same
  `AskRequested`/`ApprovalRequested`-shaped event with the **same id**, so a desktop or phone that
  reconnects renders the card it would have rendered, and 104's Attention item reappears under the
  same key (`ask:<id>`). The tool call's result is whatever the answer turns out to be, exactly as
  if nothing had happened. A row whose deadline already passed → `timed_out` and the tool's own
  timeout result (`approval timed out` / dismissed).

Why not re-publish approvals too: an approval is tied to a registry invocation that no longer exists
(the suspended `invoke` future died with the process, `registry.rs:1520-1533`); re-creating it means
re-running `invoke`, which is what re-asking is.

### 6.3 Deadlines and supersession

- `deadline` is absolute, so the time the engine was down counts against the wait: a 5-minute
  approval raised 10 minutes before a crash is `timed_out` at resume and the call answers `approval
  timed out`. Routines that must not stall on a human already set `approvals: false` (34).
- An answer arriving for a `superseded` or `timed_out` id is refused with the state
  (`answer_ask(id)` → `Err("superseded")`), so a stale card on a phone cannot answer the wrong
  question.
- Supersession never cascades: one old row, one new row.

### 6.4 Invariants kept

- **Approval before `Start`**: unchanged in code path and strengthened in the journal — a `jobs` row
  can exist only for a `started` call, and a call is `started` only after the gate.
- **No duplicate human prompts**: at most one `pending` row per `(session, tool_call_id, kind)`;
  `Republish` is idempotent on the id.
- **No silent drop**: every pending row ends `answered`, `timed_out` or `superseded`, and the tool
  call that asked ends `finished` or `skipped` with text that says which.

## 7. Peers, children and external harnesses

### 7.1 Exchanges and waits

`peer_exchanges` is already durable with `UNIQUE(sender, request_id)` and a fingerprint
(`crates/agent/migrations/0011_communication.sql:10-41`); what dies today is the *waiter*
(`Agents.waits`, `supervisor.rs:239-250`; "Pending sets derive from live waiters, never abandoned
oneshots", `crates/agent/src/communication.rs:520-522`). §4.4's `waits(peer_reply)` row is the
missing half: at resume the store answers "is there a `replied` row whose `reply_to` is this
exchange?" — yes → the reply is the tool result; no → a new `Wait` with the remaining deadline. The
reply path (`peer_changed`, delivery of a `replied` receipt to the waiting session) is unchanged.

### 7.2 Child results: narrow `fail_interrupted`

Today every `accepted` exchange is failed at boot (`store/communication.rs:1445-1456`), including a
child's finished result (`request_id = "result:{turn}"`, `supervisor.rs:1612-1626`) the parent had
not yet consumed — so the parent never learns the child finished (`109-recon.md §4`, §10 fact 8).
109:

```sql
UPDATE peer_exchanges SET state = 'failed', error = ?, updated_at = ?
 WHERE state = 'accepted'
   AND NOT (sender LIKE 'session:%' AND request_id LIKE 'result:%')
```

The spared rows are delivered on resume: `ResumeTurn` for the parent (if its turn is open and waits
on `child_result`) consumes them as the wait's answer; a parent with no open turn gets them injected
as a new input by the same path that delivers a fresh result (`hand_off` → `peer_changed`,
`supervisor.rs:1639`), triggered by a boot pass over the spared rows after `resume_scan`. The dedup
key (`sender`, `request_id`) and `enqueued` (`supervisor.rs:1635-1638`) make a double delivery
impossible. `a2a_tasks` keep today's behaviour (out of scope).

### 7.3 ACP children (omp, opencode)

What exists: `sessions.acp_session` is set on `Connected` (`acp_child.rs:1496`); `acp_session(id,
…)` passes `resume = store.acp_session(id)` and the stored `cwd` to `AcpChild::spawn`
(`supervisor.rs:887-911`); `Link::open` runs `initialize` then `session/load` when `resume` is set
and the agent advertises `loadSession`, falling back to `session/new` on a failing load
(`harness.rs:478-483`, `:554-557`); a stored id that was not restored records `"{name}: previous
harness session not restored; starting fresh"` (`acp_child.rs:1476-1495`); `Shutdown` is documented
as "the supervisor then respawns from the row with `session/load`" (`acp_child.rs:350-353`). Nothing
triggers any of this at boot.

109 adds two things:

1. **Trigger.** When `ResumeTurn` of a parent finds a `child_result` wait on a session whose row has
   `agent` set (an ACP child), or `resume_scan` finds an ACP child session whose own `turn_steps`
   are open (an ACP child's turn is journaled like any other: `input_accepted` at prompt,
   `turn_finished` at `PromptResponse`), the supervisor respawns it through the existing
   `acp_session` path with `resume = Some(acp_session)`.
2. **Delta handoff.** If the opened link reports `resumed() == false` (`acp_child.rs:1478`) (load
   failed or the harness has no `loadSession`), instead of the bare fresh start the supervisor
   composes a handoff and sends it as the first `session/prompt` (the `Note` mechanism,
   `acp_child.rs:357-366`, carries it before the user's text):

```
<resume_handoff>
Your previous session with this harness was interrupted by a restart of theuth at 12:05:02 and could not be
restored. You are a fresh process in the same working directory; the files and the git state are as you left them.
What you had reported last (12:03:12): "…last assistant text of the child, capped…"
Uncommitted work in /Users/sky/Develop/x (git status --porcelain, 7 lines): …
Diff since the last commit (git diff --stat, 5 files): …
Messages queued for you while you were away (2): …
Open todos (3): …
The full earlier transcript is available with session_read("<child session id>"); read turns N–M if you need the
exact brief. Continue from the uncommitted state; do not redo finished steps.
</resume_handoff>
```

Composition is a pure function in `acp_child.rs` over theuth's records (last assistant row, queued
`accepted` exchanges to the child, `todo_snapshots`, and two `git` reads in `cwd` with a 5 s cap
each) under `durability.handoff_tokens` (4000) `(call)`; each section is truncated tail-first and
says how much was cut. A child that *was* restored gets no handoff — `session/load` replayed its own
context — but still gets the existing `CANCELLED_NOTE` when the replayed turn had been cancelled
(`acp_child.rs:364-366`).

theuth-subagent children are sessions with open turns: §4.6's `resume_scan` resumes them with no
special path (D5), and their parents' `child_result` waits are answered when they finish (§7.2).

### 7.4 Sandbox VMs

Already handled by the sandbox crate: the microsandbox actor re-issues mounts and ports from its
record after a restart (`crates/sandbox/src/vm/microsandbox.rs:55-57`) and `create` re-uses an
existing VM by name (`:451`) — a VM is the msb runtime's process, not the engine's child. A subagent
running *inside* a sandbox is a session on that engine, journaled there; nothing in 109 changes the
sandbox crate.

## 8. Streaming checkpoints and what the UI shows

### 8.1 Partials

`stream_once` accumulates text and tool-call arguments in memory and persists at `End`
(`loop_.rs:4025-4043`, `:4079-4082`). 109 adds one `turn_steps(assistant_partial)` row per model
call, keyed by `call_id`, **upserted** (never appended) when:

- a content block closes (text block → tool-use block, reasoning → text), and
- a timer fires: `durability.partial_checkpoint_ms` (1000) since the last upsert *and* the captured
  content grew.

Never per token. The payload is the same `MessageContent` JSON `persist` would write plus
`thinking_ms`, so the UI renders it with the existing `HistoryEntry` code. The row is deleted in the
transaction that persists the final assistant row (text or `tool_use`), so a finished model call
leaves no partial behind and the common case costs one `DELETE` folded into a write that happens
anyway.

The model never sees a partial: `Store::frame` does not read `turn_steps`. The re-issued call is
built from the identical frame (I5); the partial is for the human.

### 8.2 Banners

On `ResumeTurn` the actor publishes one event before it does anything (so the UI changes state at
once), and one when the batch is reassembled:

```rust
AgentEvent::TurnResumed {
    session: SessionId, turn: TurnId,
    kind: ResumeKind,                 // Interrupted | Suspended
    interrupted_at: DateTime<Utc>, resumed_at: DateTime<Utc>,
    partial_dropped: bool,            // an assistant_partial existed and was discarded
}
AgentEvent::ResumeReport {
    session: SessionId, turn: TurnId,
    replayed: Vec<(String /* call_id */, String /* name */)>,
    interrupted: Vec<(String, String)>,     // marked `interrupted`: effect unknown
    lost_jobs: Vec<JobId>,                  // jobs that died with the engine (§5)
    children_restored: u32, children_handed_off: u32,
}
AgentEvent::TurnSuspended { session: SessionId, turn: TurnId, reason: String }   // §9
```

The desktop renders `TurnResumed` as a session banner above the composer, wording by `kind`:

- `Interrupted by a restart at 12:05 — resumed automatically (2 tool calls replayed, 1 skipped: git
  push, effect unknown)` — once `ResumeReport` arrives the counts fill in;
- `Paused for an update at 12:05 — resumed` for `Suspended`.

A dropped partial is shown in place as the greyed partial text with `interrupted — regenerating`
until the re-issued call's first delta replaces it (the `Model::interject` path from 00 §8 for the
TUI; the stream reducer for the desktop). `load_session` after a restart returns the partial as
`HistoryEntry::Interrupted { content, at }` for the open turn, so a reload during the resume shows
the same thing.

### 8.3 Attention items (104)

`AttentionKind::Resume` (`resume:<session>:<turn>`) is raised when a resume needs a human:
`interrupted` non-empty (a mutating effect is unknown), a replay failed (the re-issued model call
errored terminally), an ACP child could not be restored and got a handoff, or `durability.resume =
ask` (one item per open turn with a Resume button). The item carries the session (104 A3) and the
`ResumeReport`; it clears when the user opens the session or presses Dismiss, and under `resume =
ask` when they press Resume. A clean resume (nothing skipped, nothing lost) raises no item: the
banner is enough.

### 8.4 Job events

`JobEvent::Finished` carries `exit: JobExit::Lost` (§5.4) and the desktop job row shows `lost` with
the log link; the `screen` viewer renders the stored page under the §5.3 banner. The TUI's existing
`Daemon restarted.` note on PTY pages (`crates/tui/src/model.rs:2027-2030`, `:2230`) stays and now
sits beside a screen that is the stored one.

## 9. Graceful shutdown and upgrade

### 9.1 Today

`main.rs::run` selects on the shutdown token, SIGINT and `parent_gone()`, sleeps 100 ms "to flush",
releases the engine record and exits (`crates/theuth/src/main.rs:1509-1520`); `parent_gone` polls
`THEUTH_PARENT_PID` every 5 s (`:1527-1541`). Every plugin that cares watches `rt.shutdown`; the
jobs actor kills everything (`jobs.rs:645-660`). There is no drain: a tool mid-flight, a stream
mid-token and a cell mid-run are all cut by `process::exit`.

### 9.2 Drain

On `rt.shutdown` (SIGTERM, SIGINT, rpc `shutdown`, `parent_gone`, `t update`'s relaunch), `main.rs`
no longer sleeps a fixed 100 ms; it asks the supervisor `Drain { deadline: now +
durability.drain_secs }` and waits for the reply or the deadline, then exits as before:

1. **Stop accepting input.** `Agents` sets `draining`; new inputs are accepted into the store (the
   user row is committed and acknowledged, I-(c)) and journaled `input_accepted`, but no turn starts
   — resume will (§4.2 row 1).
2. **Let in-flight tool calls finish**, up to the deadline. The running batch keeps running; calls
   that return are `finished` per call (§3.3 step 7) so the journal is as complete as the time
   allows. No new model call is opened during a drain: a `round` that would open one writes
   `suspended` instead and returns `Round::Stop("suspended")`.
3. **Checkpoint.** Every live actor upserts its current `assistant_partial`, every PTY writes its
   screen row, every tool call still running at the deadline stays `started` — the resume rule for a
   `started` call is exactly right for one cut by a drain.
4. **Mark.** Every open turn gets a `turn_steps(suspended)` row with the reason; the UI's wording
   differs (§8.2), nothing else does.
5. **Monty.** `Repl::close` on every live repl (dump + finish, 08 §5) — a cell still running at the
   deadline is dropped, and the previous dump stands (the python rule, §4.2).
6. **Jobs.** `KillAll` exactly as today, now after the drain window instead of at the instant the
   token fires; every running row is written `lost` first (§5.5). A job cannot outlive the engine
   (R1).
7. **Release.** Actors stop; `rpc::engine::release()`; exit.

`drain_secs` is 10 by default: long enough for a `write_file` or a `git commit`, short enough for
launchd's `ThrottleInterval` 5 s / systemd `RestartSec=2` (`crates/platform/src/service.rs:216-224`,
`:243`) and the desktop's relaunch not to look hung. A SIGKILL skips all of it and the resume
protocol does not care: that is the point of D6 — both paths converge on §4.

### 9.3 The paths that end the engine

| path | today | 109 |
|---|---|---|
| SIGTERM / SIGINT | 100 ms, exit | drain, exit |
| rpc `shutdown` (desktop quit, `/quit`) | same | same |
| `parent_gone` (desktop app died; Windows Job Object kills the daemon with the app, `crates/desktop/src/launch.rs:259-271`, `:519`) | same; on Windows no code runs | drain where code runs; Windows: crash path, resume on next start |
| `t update` (39) | atomic rename over `current_exe()`, relaunch | the relaunch sends `shutdown` first, so a drain precedes the swap; after it the service's `KeepAlive` (launchd, `service.rs:218-222`) / `Restart=on-failure` (systemd, `:243`) or the desktop's `relaunch()` (`launch.rs:106`) starts the new binary, which resumes |
| crash / `kill -9` / power loss | nothing | resume on next start; `kind = Interrupted` |
| sandbox VM engine | same as above inside the VM | same |

The desktop app does not restart a daemon that dies on its own today (`109-recon.md §1`); that is a
96/25 concern and not changed here, but the resume protocol makes an automatic relaunch safe to add
later `(call)`.

## 10. Config, types and RPC

### 10.1 Config

```yaml
durability:
  resume: auto              # auto | ask | off — what boot does with open turns (§4.6)
  resume_parallel: 4        # ResumeTurn concurrency; the rest queue, most recent first
  drain_secs: 10            # graceful-shutdown budget for in-flight tool calls (§9.2)
  partial_checkpoint_ms: 1000   # assistant_partial cadence (§8.1); never per token
  pty_snapshot_secs: 5      # PTY screen snapshot timer (§5.3)
  journal_keep_days: 7      # finished-turn journal retention (§3.6)
  handoff_tokens: 4000      # ACP delta handoff budget (§7.3)
  # the job reaper (§5.4) has no key: it always runs before any ResumeTurn
```

Read once at startup like every other section (`agent_config(rt)`, `supervisor.rs:7309`);
`durability` is its own top-level section carried on `AgentConfig` the way `surface` is
(`:7311-7313`).

### 10.2 Types (sketch)

```rust
// agent::durability (new module; the store half in store/durability.rs)
pub struct DurabilityConfig { pub resume: ResumeMode, pub resume_parallel: usize, pub drain_secs: u64,
    pub partial_checkpoint_ms: u64, pub pty_snapshot_secs: u64,
    pub journal_keep_days: u32, pub handoff_tokens: usize }
pub enum ResumeMode { Auto, Ask, Off }
pub enum StepKind { InputAccepted, ModelCall, AssistantPartial, ToolBatch, TurnFinished, Suspended }
pub enum CallState { Planned, Started, Finished, Interrupted, Skipped }
pub struct JournaledCall { pub call_id: String, pub name: String, pub args: Value, pub risk: tools::Risk,
    pub state: CallState, pub attempt: u32, pub started_at: Option<DateTime<Utc>>,
    pub output: Option<String>, pub is_error: bool, pub files: Vec<PathBuf> }
pub enum LastStep { InputAccepted, ModelCall { request_id: String, attempt: u32 },
    ToolBatch { all_finished: bool }, Finished }
pub struct ResumePlan { … }                                   // §4.1
pub enum ResumeKind { Interrupted, Suspended }

// store API (every fn is one WriteTx or joins the caller's)
impl Store {
    pub(crate) async fn step_in(tx: &mut WriteTx<'_>, session, turn, kind, call_id, payload) -> Result<()>;
    pub async fn partial_upsert(&self, session, turn, call_id, content: &MessageContent, thinking_ms) -> Result<()>;
    pub(crate) async fn calls_planned_in(tx, session, turn, at_seq, calls: &[(ToolCall, Risk)]) -> Result<()>;
    pub async fn call_started(&self, session, call_id) -> Result<()>;
    pub async fn call_finished(&self, session, call_id, out: &ToolOutput) -> Result<()>;
    pub async fn call_interrupted(&self, session, call_id, note: &str) -> Result<()>;
    pub async fn open_turns(&self) -> Result<Vec<(SessionId, TurnId, DateTime<Utc>)>>;
    pub async fn resume_plan(&self, session, turn) -> Result<ResumePlan>;
    pub async fn question_put / question_resolve / questions_pending(session) …;
    pub async fn wait_put / wait_clear / waits_of(session, turn) …;
    pub async fn job_put / job_settle / jobs_open() / pty_screen_put / pty_screen(session, id) …;
    pub async fn fail_interrupted(&self, reason) -> Result<Interrupted>;   // narrowed, §7.2
}

// tools: the journal seam the agent installs (like InstallActivity)
pub trait Journal: Send + Sync {
    fn call_started(&self, session: SessionId, call_id: &str) -> BoxFuture<'_, ()>;
    fn call_finished(&self, session: SessionId, call_id: &str, out: &ToolOutput) -> BoxFuture<'_, ()>;
    fn question(&self, q: QuestionRow) -> BoxFuture<'_, ()>;
    fn wait(&self, w: Option<WaitRow>, session: SessionId, call_id: &str) -> BoxFuture<'_, ()>;
    fn job(&self, j: JobRow) -> BoxFuture<'_, ()>;
    fn pty_screen(&self, s: PtyScreenRow) -> BoxFuture<'_, ()>;
}
pub struct InstallJournal(pub Arc<dyn Journal>);     // registry, jobs, approval, ask actors
pub enum JobExit { Exited(u8), Timeout, Killed, Lost }

// supervisor messages
pub struct ResumeTurn { pub session: SessionId, pub turn: TurnId }
pub struct Drain { pub deadline: Instant }           // ask → DrainReport { suspended: u32, finished: u32 }
pub struct OpenTurn(pub SessionId);                  // ask → Option<TurnId>, for the schedule plugin (§4.6)
```

### 10.3 RPC and surfaces

- `load_session` → `Loaded.history` gains `HistoryEntry::Interrupted { content, at }` for an open
  turn's partial and `Loaded.resume: Option<ResumeState>` (`kind`, `interrupted_at`, `report`).
- `list_jobs` / `JobView.state` gains `lost`; `pty_screen` for a lost job returns the stored page
  with `epoch: None` and a `banner: Option<String>`.
- `attention_list` gains `resume:` items (§8.3); `resume_turn(session)` RPC for the `ask` mode's
  button; `attention_dismiss` clears as today.
- Events: `TurnResumed`, `ResumeReport`, `TurnSuspended` (ts-exported like every `AgentEvent`);
  `JobEvent::Finished` with `exit: lost`.
- TUI: the banner as a status-line note and a `── resumed after restart ──` divider in the
  transcript (the `Compacted` divider shape, 07 §8.4); `/jobs` shows the two new states.
- Tools: no new tool. `jobs()` and `screen()` read as described in §5.

## 11. Verification

### 11.1 The crash-injection harness

A fault hook in the loop and the store: `durability::crash_point(name)` is a no-op unless
`THEUTH_CRASH_AT=<name>` (tests) or an injected `Arc<dyn Fn(&str) -> bool>` on `Shared` (in-process
tests) matches, in which case it `std::process::abort()`s (env) or returns `Err(Aborted)` that the
test harness turns into "drop the actor and the store handle" (in-process). Points, named after the
§3.3 steps and placed on both sides of each write:

```
input.before_persist · input.after_persist · model.after_reserve · model.after_dispatch · stream.mid (after the
first text delta) · stream.after_partial · stream.before_end_persist · batch.after_tool_use_persist ·
call.<i>.before_started · call.<i>.after_started · call.<i>.mid (inside a fake tool) · call.<i>.before_finished ·
call.<i>.after_finished · batch.before_message · batch.after_message · turn.before_finished · turn.after_finished
```

The core test (`crates/agent/src/durability/tests.rs`, in-process): a scripted turn with a fake
provider (two rounds: a batch of three tools — read-only, mutating, python — then a text reply) and
fake tools whose bodies record their invocations in a `Vec`. For every crash point: run until it
fires, drop everything, reopen the same temp store, run `resume_scan` + `ResumeTurn`, let the turn
finish, then assert:

1. the `messages` of the session equal the uninterrupted run's messages **modulo** the
   `<engine_restart>` attribute line and the §4.3 notes, which must appear iff the crash point is
   inside a `started` call of the given risk (a table of expected notes per point is part of the
   test);
2. the fake mutating tool's body ran **exactly once** across both lives (I-(b)); the read-only one
   ran once or twice; the python cell ran twice only when its inner calls were all read-only;
3. `tool_calls` rows are all terminal; `turn_steps` ends in `turn_finished`; `assistant_partial`
   rows are gone; `sessions.last_stop` is set;
4. the provider saw the same request bytes for the re-issued call as for the lost one (frame
   identity, I5) — the fake provider hashes requests;
5. `model_calls` shows `unresolved` for the lost attempt and a settled `attempt + 1` with the same
   `request_id`.

The same loop runs with `kind = Suspended` by driving `Drain` to the deadline at each point instead
of aborting.

### 11.2 Per wave

| wave | tests (crate-level) |
|---|---|
| W1 | the §11.1 matrix; `store::durability::tests` for every store fn (round trip, `at_seq` anchoring, cut deletes journal rows, `open_turns` ordering); a replication test that the new tables ship in `session_log` (extend 68's capture test with `turn_steps`); `fail_interrupted_spares_child_results` |
| W2 | `jobs::tests::reaper_kills_leaked_group` (spawn `sleep 600` through `Start`, drop the actor without `KillAll` — the SIGKILL shape — reopen the store, `reap_jobs`, assert `killpg(pgid, 0)` fails, the row is `lost`, and `<job_result exit="lost">` with the log tail was injected); `reaper_skips_open_batch_job` (the job's call is a `started` row of an open turn → no injection; the job rule reports it); `killall_writes_lost_before_kill`; `pty::tests::screen_snapshot_on_screen_and_timer` (revision gate, scrubbed text, row after exit); `screen_after_restart_serves_banner` |
| W3 | `approval::tests::answered_row_honoured_on_replay`, `superseded_on_reask`, `deadline_elapsed_is_timeout`; `ask::tests::republish_same_id`; `waits` re-arm with remaining deadline; a `send_message await_reply` reply that landed while down is delivered |
| W4 | `acp_child::tests::handoff_composition` (budget, truncation markers, git sections from a temp repo); the recorded-fixture harness (84) driven through `session/load` success and failure; `resume_scan` respawns an ACP child with an open turn |
| W5 | `Drain` finishes a quick batch inside the deadline and suspends a slow one; `TurnResumed`/`ResumeReport` emitted with the right counts; desktop `vitest` for the banner, the interrupted partial and the two job states; attention item lifecycle |
| W6 | the §11.1 harness made exhaustive (every point × every tool-specific row of §4.2) and wired into `just` as `just durability` |

Gates per wave are crate-level: `cargo test -p agent` (W1, W3, W4, W5), `cargo test -p tools` (W2,
W3), `cargo test -p acp` (W4), `just ts-desktop` + `bun run test` in `crates/desktop/ui` (W5). The
`--test-threads=1` workaround for the sqlx teardown hang stands (00 §12).

### 11.3 Manual smoke (Sky)

1. In a chat: `shell("cargo build -p agent 2>&1 | tail -20")`, `start("bash -l")`,
   `remind_me_in(300, "x")`, and `ask_user` with one question. Do not answer.
2. `kill -9 $(pgrep -f 't --headless')`.
3. Reopen the app. Expect: the session shows the banner within a few seconds; the `ask_user` card is
   back with the same question; `<job_result exit="lost">` for the build arrives with the log tail
   and `ps` shows no leaked `cargo` (the reaper killed it if it survived the kill); `jobs()` lists
   both jobs as `lost`; `screen(<pty>)` shows the banner over the last prompt; the reminder fires on
   time.
4. Repeat with `kill -TERM`: the banner says paused, both jobs are `lost` with `killed at shutdown`,
   and a `write_file` in flight completed before the exit (check the file).
5. Spawn an omp child on a worktree, let it start editing, `kill -9` the engine, reopen: the child
   comes back via `session/load` (omp supports it) and continues; break the load (`rm` omp's session
   file) and repeat: the child restarts with the `<resume_handoff>` naming the uncommitted files.

### 11.4 Done means

- W1: the §11.1 matrix green; a real chat survives `kill -9` mid-batch with a correct transcript.
- W2: smoke steps 3–4 for jobs: the loss is reported once, the log is intact, nothing is leaked.
- W3: the smoke's `ask_user` card returns; an approval raised before a crash is re-asked, an
  answered one is honoured.
- W4: smoke step 5 both ways.
- W5: banners, items, drain under 10 s on a loaded machine.
- W6: `just durability` in CI.

## 12. Implementation order

Six omp waves with seam ownership (the 2026-10 wave procedure); each brief carries the full "owns /
must not touch" list below and names the sibling worktrees.

| wave | builds | owns | must not touch | gate | size |
|---|---|---|---|---|---|
| **W1** journal + safe replay | `0033_durability.sql` (all six tables, so later waves add no migration); `store/durability.rs`; `turn_steps`/`tool_calls` writes in `loop_.rs` (§3.3 steps 1–5, 7–9); `ResumeTurn`, `resume_scan`, `resume_plan`; the per-call rule minus the job/question/wait rows; python rule; `repair_dangling` demoted; `last_stop`/`finished_at`; `assistant_partial` upserts; `fail_interrupted` narrowed + boot pass | `crates/agent/migrations/0033_durability.sql`, `crates/agent/src/store/durability.rs`, `store.rs` (new fns + `ATTACHED`), `store/capture.rs:15-25`, `store/communication.rs:1442-1476`, `loop_.rs`, `supervisor.rs` (start sequence, `ResumeTurn`, `last_stop`), `lib.rs` (config, events) | `crates/tools/**`, `crates/acp/**`, `acp_child.rs`, `crates/desktop/**`, `crates/schedule/**` | `cargo test -p agent` | ~2.5 k lines, 1 omp, the critical path |
| **W2** jobs | `jobs` writes, `KillAll` writes `lost` first and runs after the drain window, `reap_jobs`, `JobExit::Lost` + the `<job_result exit="lost">` delivery, `pty_screens` snapshots and the banner, `tools::Journal` trait + `InstallJournal`; agent side: the `Journal` impl over the store, the job rule rows of §4.2 | `crates/tools/src/jobs.rs`, `pty.rs`, `shell.rs`, `crates/tools/src/lib.rs` (trait, `JobExit`), `crates/agent/src/durability/journal.rs` (new), the job-rule arm in `loop_.rs` (one function W1 leaves as a stub `resume_job_call`) | `store.rs`, `supervisor.rs` start sequence except the one `reap_jobs` call W1 leaves as a stub, `crates/platform/**`, `registry.rs`, `approval.rs`, `ask.rs` | `cargo test -p tools`, `cargo test -p agent` | ~800 lines |
| **W3** questions + waits | `questions` and `waits` writes in the registry gate, `Approval`, `AskUser`, handoff and secret tools; `Republish`; answered-row honouring; `waits` re-arm for `peer_reply`/`child_result`/`pty_wait`/`question`; the question/wait rows of §4.2 | `crates/tools/src/registry.rs`, `approval.rs`, `ask.rs`, `crates/secrets/src/tools.rs`, `crates/roster/src/handoff.rs`, `supervisor.rs` (`Wait` rebuild, `inject` of spared results), the question/wait arms in `loop_.rs` (stubs `resume_question_call`, `resume_wait_call`) | `jobs.rs`, `pty.rs`, `store.rs` beyond calling W1's fns, `acp_child.rs` | `cargo test -p tools`, `cargo test -p agent` | ~1.2 k lines; **runs parallel to W2** (disjoint files; both call W1's store API) |
| **W4** ACP | boot respawn trigger, handoff composition, `Note` delivery, `resumed()` branch | `crates/agent/src/acp_child.rs`, `crates/acp/src/harness.rs` (only if `open` must expose more), `supervisor.rs::acp_session` | `loop_.rs`, `store.rs`, `crates/tools/**`, `crates/desktop/**` | `cargo test -p agent`, `cargo test -p acp` | ~800 lines |
| **W5** drain + UI | `Drain`, `main.rs` wiring, `suspended` rows, `TurnResumed`/`ResumeReport`/`TurnSuspended` publication, `HistoryEntry::Interrupted`, `Loaded.resume`, attention `Resume` kind, desktop banner/partial/job states, TUI divider, schedule `OpenTurn` | `crates/theuth/src/main.rs:1508-1520`, `supervisor.rs` (`Drain` handler), `crates/rpc/**` (new fields), `crates/roster/src/attention.rs`, `crates/desktop/ui/**`, `crates/tui/**`, `crates/schedule/src/lib.rs` + `store.rs:46-67` | `loop_.rs` except publishing the two events at the points W1 marks, `store.rs`, `crates/tools/**`, `acp_child.rs` | `cargo test -p agent -p rpc -p roster -p schedule`, `just ts-desktop`, `bun run test` | ~1.5 k lines; **runs parallel to W4** |
| **W6** crash suite | §11.1 exhaustive, `just durability`, CI job | `crates/agent/src/durability/tests.rs`, `justfile`, CI | everything else (fixes go back to the owning wave as a fix round) | `just durability` | ~800 lines |

Order of landing: W1 → (W2 ∥ W3) → (W4 ∥ W5) → W6. W1 is docs-free but contract-heavy: its brief
includes the §10.2 sketch verbatim as the API the others code against, and W1 leaves three named
stub functions in `loop_.rs` (`resume_job_call`, `resume_question_call`, `resume_wait_call`, each
returning the interrupted note) so W2/W3 own one function each and the tree compiles at every merge.
The rebuilt engine and desktop are owed after W1 lands (every running engine predates the
migration).

## 13. Edits to earlier docs

For the integrator, one list:

1. **07 §8.4 Resume** (`docs/design/07-context.md:655-665`): after "The frame on resume is
   byte-identical…", add: *"109: a session whose newest `turn_steps` row is not `turn_finished` is
   resumed eagerly at boot (`ResumeTurn`), the lost model call re-issued from this frame as a new
   attempt, and the open tool batch completed from the `tool_calls` journal (109 §4). The frame
   itself is unchanged: journal rows never enter it."*
2. **14 §5 Persistence and restart** (`docs/design/14-jobs.md:304-311`): replace the paragraph with
   *"Superseded by 109 §5: a `jobs` table (migration 0033) mirrors every job for accounting; a job
   still dies with the engine, and on the next start its row yields `<job_result exit="lost">` with
   the log path (a PTY: the last screen under a banner); process groups a SIGKILL leaked are reaped
   before any turn resumes. The v0 rejection stands as history."* **14 §1.2**: `exit` gains `lost`
   in practice (the value was reserved; now delivered). **14 §2** "On `rt.shutdown` the actor kills
   every running group": unchanged — add "after 109's drain window; rows are written `lost` first".
3. **36 §3 Resume repair** (`docs/design/36-survival.md:178-192`): add *"109: `repair_dangling` is
   the fallback for transcripts with no `tool_calls` rows (pre-0033). With the journal, a dangling
   `tool_use` is answered per call from the journal — finished results reused, unstarted and
   read-only calls run, mutating calls marked interrupted with their effect unknown — so 'not run'
   is no longer asserted when the effect may have happened."* The pair invariant statement is
   unchanged.
4. **51** Status / §6.4: `sessions.last_stop`, `sessions.finished_at` added (0033);
   `Agents.last_stop` is a cache of them. §7 (PTY observation): `pty_screens` is the retained
   screen's durable twin, engine-local; `pty_screen` for a lost job serves it with a banner.
5. **68 §3 / §8** (`docs/design/68-sessions.md`): `capture::ATTACHED` grows by `turn_steps`,
   `tool_calls`, `questions`, `waits`, `jobs` (not `pty_screens`); the "what each engine holds"
   statement now includes the open-turn journal, which is what makes §4.4's resume-elsewhere a
   follow-on rather than a redesign. **68-contract.md:46** attached-table list likewise.
6. **00 §5** (`docs/design/00-architecture.md:484-485`): "restart makes unconsumed work `failed:
   interrupted_by_restart` and auto-runs nothing" → *"restart fails unconsumed peer work except a
   child's finished result, and resumes every open turn (109 §4)"*. **00 §4 tool contract**: add one
   sentence under the registry's execution seam: *"every registry invocation is journaled `started`
   before its body and `finished` as it returns (109 §3.3), through `tools::Journal`."* **00 §12
   known gaps**: the "`Jobs` actor kills every process group on shutdown" line stays — it is the
   ruled behaviour. Status table: the row added with this document.
7. **34** (schedule): a run whose session has an open turn at boot is kept open and re-attached, not
   marked `lost` (109 §4.6 step 12).
8. **104**: `AttentionKind::Resume` added to the kinds and the inbox group order (under "Needs a
   hand") (109 §8.3).
9. **109-recon.md** (not edited; corrections recorded here): the citation `14-jobs.md#L317-L319` for
   "approval precedes `Start`, so a job never runs unapproved" is the config block; the sentence is
   at `14-jobs.md:322-324`. Everything else re-read at `ef1ad0a3` matched.

## 14. Rejected

- **A detached job keeper (`t jobd`, setsid, owning PTYs, jobs and ACP children so they outlive the
  engine).** The most faithful answer to R1 — a PTY would survive, an ACP child would keep its stdio
  — and the most work: a second long-lived process with its own lifecycle, IPC for every job
  operation, its own crash story, and a second place to upgrade. Offered to Sky and not chosen for
  now; the omp-worker case is covered by `session/load` + the delta handoff (§7.3), and a lost build
  is one `shell()` away. Later stage: when a transcript shows a PTY or an un-loadable harness
  session that mattered, build the keeper *behind* the same `jobs`/`waits` rows, so nothing above
  the tools crate changes.
- **Process-surviving jobs by adoption** — the orchestrator's middle ground between the keeper and
  plain journaling: adopt a live process group at boot (`ProcessTree` + `killpg(pgid, 0)`), stop
  `KillAll` at shutdown, redirect job stdout/stderr to the log file and tail it, report
  `exit="unknown"` when the adopted group ends. **Rejected by Sky 2026-10-06 12:28** ("I thought we
  decided jobs aren't durable"): it keeps alive a process the engine can neither steer nor wait on,
  it needed a different I/O path on Unix and could not work on Windows at all, and R1 had already
  drawn the line at journaling. Design v0 carried it as D3; Revision 1 removed it.
- **Pause and ask as the default** (and the hybrid "auto for routines, pause for chats"). Offered,
  not chosen: an overnight restart would strand every routine and child until morning, and the
  hybrid needs a classification of sessions the engine does not have cleanly (a chat can spawn a
  routine's child). `durability.resume: ask` keeps the behaviour for whoever wants it.
- **Per-token checkpoints.** One `UPDATE` plus one `session_log` row per token is 50–100 writes a
  second per streaming session for a partial the model never sees. Block boundaries plus a
  one-second timer bound the loss to a second of text.
- **Replaying mutating effects** (re-running a `started` `git push`, `write_file`, `send_message`
  because "it probably did not happen"). Violates I-(b); the model, told the effect is unknown, can
  check in one read-only call. The only effects the harness replays are observations.
- **An event-sourcing rewrite** (t3code-style: every state change an event, projections rebuilt).
  Ruled out by Sky in the 2026-10-03 borrow analysis; the transcript tables plus 68's physical
  changeset log already give ordered, exactly-once replay of committed state, and 109 adds the
  intents that were missing without moving anything.
- **A separate journal file outside `sessions.sqlite`** (a WAL of intents in `<data_dir>/journal/`).
  Loses D1's property that the journal and the transcript commit together (I7) and that 68 ships it;
  two files can disagree after a crash, one cannot.
- **Resuming an in-flight turn on another engine** at the moment of takeover. 68 replicates the
  journal, so a taker *could* run `ResumeTurn` on its copy; the local processes (jobs, PTYs, Monty
  dumps, ACP children) are not there, so the per-call rule would mark everything local interrupted.
  Correct but blunt; a follow-on once the single-engine protocol has run for a while.
- **Marking every `started` call "not run" (today)** or every `started` call "done". Both lie half
  the time; the journal lets the engine say what it knows and no more.
- **No `jobs` table at all** (14 §5's v0 ruling, kept until now). Without the row the model is never
  told a job vanished and a leaked group is never reaped; the table costs one migration and the scan
  14 §5 declined, and buys exactly the `exit="lost"` reporting 14 §1.2 reserved.

## 15. Open questions

Each with the default this design assumes.

1. **Should `assistant_partial` be replicated at all?** Default yes (it is in `turn_steps`,
   attached): a taker's UI shows the interrupted text. If `session_log` growth on long streams shows
   up in a transcript, move the partial to its own unattached table — a one-line `ATTACHED` change.
   *(call: attached)*
2. **Read-only re-run of long observations.** A `started` `read_url`/`search_web` is re-run on
   resume; a `started` `crawl` could be minutes. Default: re-run (correct), with
   `durability.replay_readonly_max_secs` deferred until it hurts.
3. **Approval deadline across a drain.** Should a graceful stop pause the approval clock (the user
   may be the one restarting)? Default no: absolute deadlines, one rule; `resume = ask` exists for
   people who restart often.
4. **Where `spawn_subagent` records its call id on the child row.** Default: a new
   `sessions.spawned_by_call TEXT NULL` column in 0033 so the §4.2 row is a lookup, not a heuristic
   over `created_at`.
5. **Desktop auto-relaunch of a dead daemon.** Default: not in 109 (96/25 own the launcher); the
   protocol makes it safe.
6. **Should the reaper spare a leaked group whose session has `resume: off`?** Default no: a
   half-dead build beside a session that may be resumed by hand later is still a leak; one rule,
   every row.
7. **Resume ordering across engines in a mesh.** Two engines never own the same session (68), so
   there is no race; default: no cross-engine coordination, each engine resumes its own open turns.

