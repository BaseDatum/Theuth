# 14 — Jobs: every process the model starts runs in the background

Status: as-built v0 · 2026-09-11 · extends `00-architecture.md` §4 (tools),
`01-config-and-subagents.md` §2.3 (delivery rules), `08-repl.md` §4
(runtime `shell`). Where this doc disagrees with those, this doc wins; §10
lists the edits. Grace window confirmed at 3 s (§8).
Verified headless (claude-sonnet-4-5, real model): `sleep 8; echo …` → the
tool returned the §1.1 stub, the model answered and ended the turn, ~8 s
later `TurnStarted` carried `<job_result id="j1" … exit="0" secs="8">` and
the model reported the output; a `sleep 5` job inside a six-tool turn was
injected after the second tool batch (one turn, `<job_result id="j2">`
persisted between the tool rows) and the model quoted its value; `echo`
jobs finished inside the grace window returned inline and were **not**
injected (`JobFinished.inline == true` — found and fixed during smoke: the
first build delivered them twice); `list_jobs` showed `exited 0` /
`running` rows with tails; `sh -c 'echo $$; exec sleep 300'` was dead one
second after RPC `shutdown` (`kill -0` failed, no `ps` row). Not exercised:
TUI `/jobs` on a pty, Signal `/jobs`, `job_timeout_secs` at 1800 (the 500 ms
unit test covers the arm), a `<job_result>` arriving for a session whose
actor was evicted (no eviction exists).

## What exists today

- `shell` (`crates/tools/src/shell.rs`) runs an in-process brush `Shell`,
  one per session behind a `tokio::sync::Mutex` (`:36-40`), so calls
  within a session are serialised. The call **blocks the tool batch until
  the command exits** or the 120 s `shell_timeout_secs` fires, at which
  point the recorded process groups get `SIGKILL` (`:154-173`). Output is
  capped at 64 KiB (`:20`). The only way a command returns early is that
  timeout, or a bash `&` whose child then holds the pipe open until a 1 s
  drain grace gives up on it (`:176-186`) — the child is never reaped,
  never observed, and never killed.
- The loop runs one assistant message's tool calls concurrently with
  `join_all` under the turn's `CancellationToken` (`loop_.rs:1222-1234`)
  and **the round blocks until every tool returns**. There is no per-tool
  timeout in the loop or the registry. Cancel drops the `ask` future; the
  registry's detached `tokio::spawn` (`registry.rs:268-271`) keeps the
  command running to completion with nobody listening.
- Nothing in the tree is a background job: no handle registry, no
  `JoinHandle`/`AbortHandle` except the subagent `max_runtime_secs` timer
  (`supervisor.rs:84-87`). On shutdown, shell children are orphaned (00
  §12 known gap).
- The **one mechanism that already delivers a later result as a new
  turn** is the subagent path: child `TurnFinished` → `Agents::child_finished`
  → `inject` → `UserInput { id: None, text: "<subagent_result …>" }` →
  `start_turn` when the parent is idle, else `Shared::steer` queue drained
  after the current tool batch (`supervisor.rs:317-345, 252-305`;
  `loop_.rs:371-396, 557-571, 952-975`). Harness inputs (`id: None`)
  publish no `Input*` events and render as `Entry::Card(Peer)` via the
  TUI's wrapper parser (`model.rs:406-421`).
- Python cells (`08`) cannot start anything: monty has no subprocess; the
  worker is dumped after every cell. Runtime `shell()` is a suspension
  answered by the same `ShellTool::call`, so it blocks the cell exactly as
  the direct tool blocks the batch.

So the requested behaviour ("start it, keep working, get woken when it
ends") has its delivery half built and its execution half missing.

## 0. Decisions in one screen

1. **Every `shell` call is a job.** The tool starts the command in the
   background and returns. The model has no `async`, `wait`, `timeout` or
   `foreground` argument; there is nothing to opt into or out of.
2. **A grace window, not a second round trip for `ls`.** The tool waits up
   to `tools.job_grace_secs` (default **3**) for the job to exit. Exited:
   the reply is the finished output, same shape as today. Still running:
   the reply is a stub naming the job and its log file, and the model
   continues. The harness decides; the model never does. `0` makes every
   command a stub. *(The one decision the user should confirm — see §8.)*
3. **A job that outlives the turn wakes the model.** On exit, the result is
   injected as a harness input `<job_result>` through the existing
   `Agents::inject` path: idle session → a new turn starts; turn in flight
   → queued and injected after the current tool batch. No new lifecycle,
   no polling tool. A quiet PTY job wakes it the same way with `<job_idle>` (61).
4. **Jobs live in one `Jobs` actor in `tools`, addressed by the bus.** It
   owns every running process group in the harness, publishes
   `JobStarted` / `JobOutput` / `JobFinished`, and kills everything on
   `rt.shutdown`. The agent crate subscribes and injects; the TUI/RPC
   subscribe and render. No crate holds another's handle (00 §1.4).
5. **Shell state is last-finished-wins.** A job runs on a `Shell::clone()`
   (brush shells are `Clone`; that is how subshells work). When it exits,
   its shell (cwd, variables, functions) replaces the session shell.
   Sequential `cd` / `export` keep working; concurrent jobs are
   independent snapshots and the later exit wins.
6. **The model gets two tiny tools, not a job API.** `jobs` lists the
   session's jobs (id, command, state, seconds, last lines); `cancel_job`
   kills one. Output-so-far is the log file the stub already names —
   `read_file` is the retrieval tool, as with `spill` (07 §6).
7. **Deadline is long and per job: `tools.job_timeout_secs`, default
   1800.** Replaces `shell_timeout_secs` (120). A background job with a
   list-and-cancel surface does not need a two-minute leash; a runaway
   still dies. `<job_result exit="timeout">` reports it.
8. **Only processes are jobs.** `shell` today; any future tool that spawns
   a process goes through `Jobs`. MCP calls (request/response, no pgid)
   and Python cells (heap-dump model) stay synchronous. Subagents are
   already detached (01 §2) and keep their own wrapper. PTY jobs (22) are
   jobs with stdin and a rendered screen; same actor, no deadline.

## 1. Model-facing contract

### 1.1 `shell` reply

Finished within grace (unchanged shape):

```
{stdout+stderr}
exit code: 0
```

Still running:

```
job j3 started 3s ago, still running.
log: /Users/x/.theuth/jobs/<session>/j3.log      ← read_file for output so far
output so far:
{last ≤ 2 KiB}
Result arrives as <job_result id="j3"> when it exits — keep working, or end
the turn and it will start a new one.
```

`structured: { "job": "j3", "log": "<path>" }`. `is_error: false` — a running
job is not an error.

### 1.2 `<job_result>`

```
<job_result id="j3" command="cargo test" exit="0" secs="47">
{output, spill-capped (07 §6) to 32k chars, full text in the log}
</job_result>
```

`exit` is the code, `timeout` (deadline hit, group killed), `killed`
(`cancel_job` / `/jobs kill`), or `lost` (harness stopped while running —
only ever seen by a resumed session, §5). One `<job_result>` per job;
several finishing during one tool batch arrive as one queued input joined
by newlines, like `<subagent_result>` today (01 §2.3).

**As-built 2026-09-22 — who killed it decides what arrives.** `CancelJob`
carries a `CancelOrigin`. `Tool` (the owning model called `cancel_job`):
the model read the screen it killed — for a PTY job that was the
`<job_idle>` it acted on (61) — and the tool reply is the acknowledgement,
so `JobFinished` is published `inline: true` with `output` `killed by
cancel_job`; nothing is injected (UIs still close the row, and a PTY job's
final `Screen` event still carries the last frame). `Ui` (`/jobs kill`, the
desktop row, Signal — every RPC caller) and `Shutdown` (`CloseSessions`):
`<job_result … exit="killed">` arrives with the one-line body `killed by
the user` / `killed at shutdown`, never the output or the screen. `timeout`
and a normal exit are unchanged: the model did not ask for those and
needs the output. The kill-at-shutdown path (`KillAll`) aborts the task
and already reports through `Activity::unknown`; it publishes no
`Finished`.

### 1.3 `jobs` / `cancel_job`

```rust
struct JobsArgs {}                         // → text table: id, state, secs, command, last line
struct CancelJobArgs { id: String }        // → "killed j3" | "j3 already finished" | "no such job"
```

Both `Risk::ReadOnly`-priced except `cancel_job` (`Mutating`: it kills).
Registered by the tools plugin next to `shell`; visible per turn like any
registry tool.

### 1.4 Python runtime

Runtime `shell(cmd)` follows the same rule: returns the output `str` when
the job finished within grace, otherwise a dict `{"job": "j3", "log":
"…"}`; the result arrives as `<job_result>` on a later turn. A non-zero
exit still raises when the job finished inline; a stub never raises.
`RUNTIME_BLOCK` gains one sentence. `asyncio.gather(shell(a), shell(b))`
starts two jobs.

### 1.5 System prompt

One line in the tool description, none in `GUIDANCE`:
`shell`: "Run a bash command as a background job. Returns the output if it
finishes within a few seconds, otherwise a job id and log path; the result
is delivered later as <job_result>. cwd, variables and functions persist
between calls."

## 2. `Jobs` actor (`crates/tools/src/jobs.rs`)

```rust
pub const JOBS_ACTOR: &str = "jobs";

pub struct JobId(pub String);              // "j{n}", n per session, starts at 1; ts-exported
pub struct Job {
    pub id: JobId, pub session: SessionId, pub command: String,
    pub started: Instant, pub state: JobState,    // Running | Exited(code) | Timeout | Killed
    pub log: PathBuf,                             // <data_dir>/jobs/<session>/<id>.log
    tail: VecDeque<u8>,                            // last 8 KiB, for stubs / `jobs` / TUI
    pgids: Arc<Mutex<Vec<i32>>>, task: AbortHandle,
}

// messages
struct Start { session, command, call_id, shell: Shell, put_back }   // ask → StartReply::{Done(ToolOutput), Running { id, log, tail }}
struct List(SessionId)                     // ask → Vec<JobView>
struct Cancel { session, id }              // ask → Result<(), String>
// events (bus, Clone + Serialize, ts-exported)
pub enum JobEvent {
    Started  { session, id, command, call_id, log },   // call_id: the `shell` call, so a UI can find its card
    Output   { session, id, chunk: String },          // coalesced ≥ 500 ms apart
    Finished { session, id, command, exit: JobExit, secs: u64, output: String /* spill-capped */,
               inline: bool /* the tool call already returned this result (exited within grace) */ },
}
```

`Start` does what `ShellTool::call` does today (pipes, `NewProcessGroup`,
`on_spawn` pgid capture, drains) but in a spawned task that owns the
`Shell` clone, appends to the log file as bytes arrive, publishes
`Output` at most twice a second, and on exit publishes `Finished`, hands
the shell back to `ShellTool` (last-finished-wins, §0.5) and keeps the
`Job` row for the session (finished rows are kept until the session has
more than 32, then the oldest go). The `ask` for `Start` resolves when
either the task exits or `grace` elapses — `tokio::select!` on the
task's completion oneshot and `sleep(grace)`; the task keeps running.
Which side owns the result is decided once, by a per-job `AtomicBool`
swapped by whichever of the grace waiter (reply `Running`) or the task
(reply `Done`, event `inline: true`) gets there first; `grace: 0` has
the waiter swap immediately.

Deadline: `sleep(job_timeout)` in the task's `select!`; on fire, `killpg`
every recorded pgid (today's code) and finish as `Timeout`. `Cancel`
does the same and finishes as `Killed`. On `rt.shutdown` the actor
kills every running group before the binary's 100 ms flush sleep
(`main.rs:218-223`) — this closes part of the orphan gap in 00 §12
(the `setsid` escape stays).

Why an actor and not a `HashMap` inside `ShellTool`: the agent, TUI and
RPC need to *observe* jobs across the plugin boundary, and the registry
already gives us the pattern (one named actor, bus events, `ask` for
state). Why in `tools` and not `agent`: the tools crate owns the process
groups; dependency order stays `runtime ← tools ← knowledge ← agent`.

## 3. Delivery (agent)

`Agents` already subscribes to `AgentEvent` for child `TurnFinished`. It
additionally subscribes to `JobEvent` and on `Finished` (unless
`inline`: that result was the tool call's own reply and is not injected):

```rust
let text = e.output;                                  // already spill-capped by Jobs
let wrapped = format!("<job_result id=\"{id}\" command=\"{cmd}\" exit=\"{exit}\" secs=\"{secs}\">\n{text}\n</job_result>");
self.inject(e.session, &format!("job {id}"), text, wrapped).await;
```

`inject` → `input(Some(session), None, wrapped)` → `UserInput { id: None }`.
Idle → `start_turn`; in flight → queued (`Shared::steer`), injected after
the current tool batch, before the next model call — the model sees the
result at the earliest point the frame can take a user message (07 §2.2).
`MessageDelivered { from: "job j3" }` is published as for peers so live
UIs can card it. The `command` attribute is truncated to 120 chars and
attribute-escaped.

No `Session` change. No `Loaded` change: running jobs are asked from
`Jobs` (`List`) by whoever loads, not stored on the session.

Cancel: `/cancel` aborts the turn as today and **does not** kill jobs; the
model or operator kills with `cancel_job` / `/jobs kill`. The registry's
dropped-reply race (`registry.rs:268-271`) becomes irrelevant for shell:
the reply is the stub or the finished output, both fast.

Undo (07 §8.8): a `<job_result>` turn is a turn; `/undo` hides it like any
other. Redo restores it. The job row is unaffected.

## 4. Surfaces

### 4.1 TUI

- `shell` tool card: result text is the stub or the output, unchanged
  widget. Below it a live block (added 2026-09-14; v0 had none): `j3 ·
  running` and the last 10 output lines, fed by `Output` (colours stripped,
  `\r` rewinds the line, rows clipped not wrapped so the block never grows
  past 11 rows). `Started.call_id` picks the card, so two identical
  commands in one batch each get their own. A job that exits within grace
  loses the block (`Finished.inline`: the result carries the output); a
  backgrounded one freezes it with `j3 · exited 0 · 47s`, and the
  `<job_result>` card follows when the model gets it. Bounded per job and
  built only while the card is in the drawn window, so a session with many
  jobs costs no more per frame. Not restored on `--resume` (the log file
  and the result card are).
- Status line gains `· 2 jobs` while any job of the shown session runs
  (`JobEvent::Started/Finished` keep the count).
- `/jobs` → `Entry::Info` table (same text as the tool). `/jobs kill j3` →
  `Command::CancelJob`.
- `<job_result>` renders as `Entry::Card(CardKind::Job, "j3 · cargo test ·
  exit 0 · 47s", body)` via the wrapper parser (`injected()` gains the
  `job_result` arm; key `id="`). The `TurnStarted` echo suppression already
  covers wrapped inputs.
- History (`--resume`): the stored user message renders through the same
  parser; nothing new persisted.

### 4.2 RPC

`list_jobs(session) → Vec<JobView>`, `cancel_job(session, id) → ()`.
`JobEvent` forwarded on the event stream (`forward!` in the bridge, ts
export). `JobView { id, command, state, secs, log, tail }`.

### 4.3 Signal

`/jobs`, `/jobs kill <id>` mirror the TUI. A `<job_result>` turn produces a
reply like any turn; nothing else.

## 5. Persistence and restart

Jobs are in memory; the log file is durable. If the harness stops while a
job runs, the process group is killed (§2) and the job is gone. The next
time that session's actor is created (resume / first input), `Jobs` has no
row, so nothing is injected. **Rejected for v0:** a `jobs` table so a
resumed session gets `<job_result exit="lost">`; it costs a migration and
a startup scan for a case the operator can see in the log directory. Add
it when a transcript shows the model confused by a vanished job.

## 6. Config

```yaml
tools:
  job_grace_secs: 3        # inline reply if the job exits within this; 0 = always a stub
  job_timeout_secs: 1800   # kill the process group after this; result exit="timeout"
```

`shell_timeout_secs` is removed (clean cutover; README line and 00 §4
sentence updated). `approval_timeout_secs` unchanged — approval happens
in the registry *before* `Start`, so a job never runs unapproved.

## 7. Verification

1. `jobs::tests::inline_when_fast` — `Start { command: "echo hi", grace: 3s }`
   → `Done` with `hi\nexit code: 0`; `List` shows one `Exited(0)` row.
2. `jobs::tests::stub_then_finished_event` — `sleep 2; echo late` with
   `grace: 200ms` → `Running`; a `Finished { exit: Exited(0), output:
   "late…" }` event within 3 s; log file contains `late`.
3. `jobs::tests::cancel_kills_group` — `sleep 30` → `Cancel` → `Finished
   { exit: Killed }` in < 1 s; `kill -0 pgid` fails.
4. `jobs::tests::deadline` — `job_timeout: 500ms`, `sleep 5` → `Timeout`.
5. `jobs::tests::state_last_finished_wins` — `cd /tmp` job, then `pwd` job
   after it finished → `/tmp`.
6. `supervisor::tests::job_result_wrapper_escapes_and_names_exit` — the
   wrapper's attribute escaping, `exit` wording, 120-char command cap. The
   idle→turn / in-flight→queue behaviour is the existing `inject` path
   (01 §2.3) and was verified headless (Status) rather than unit-tested:
   an `Agents` fixture needs a store, client, catalog and pool.
7. `tui::model::tests::injected_parses_job_result` and `/jobs kill j3` →
   `Command::CancelJob`.
8. Headless smoke (07 §12.5 shape): real model, "run `sleep 8; echo done`
   then tell me the date" → stub reply, model answers, `TurnFinished`;
   ~8 s later `TurnStarted` with `<job_result id="j1" exit="0">`, model's
   reply mentions `done`. Then the same with the sleep inside a turn that
   keeps calling tools → `<job_result>` lands after a tool batch, no new
   turn. Shutdown with `sleep 300` running → no `sleep` process survives.

## 8. The decision to confirm

Grace window (§0.2). The literal reading of "always background, no way to
foreground" is `job_grace_secs: 0`: every `pwd` is a stub plus a second
model round to read the log — roughly double the tool round trips for the
majority of commands, which finish in milliseconds. A 3 s window keeps
those single-round while anything that is genuinely long-running (build,
test, server, watcher) is a job. The model still has no argument to
influence it; only the operator's config does. If the intent really is
zero grace, set the default to `0` in §6 and delete this section — nothing
else in the design changes.

## 9. Rejected

- **`async: bool` / `timeout` arguments on `shell`.** The whole point: the
  model does not choose. omp's `bash` has both and its docs spend two
  pages on when the harness overrides them.
- **A `wait_job` tool.** Foregrounding by another name. Ending the turn *is*
  the wait; the wake-up is free. Revisited in 22 (§1, §4.6): the 22
  versions are bounded, return the screen, and are for prompts and
  readiness, not for foregrounding a finishing job.
- **Backgrounding MCP calls.** No process to own; an MCP server that takes
  minutes should be exposed as a shell job by its author. A per-call MCP
  timeout is a separate, smaller fix (00 §12 gap).
- **Backgrounding Python cells.** A cell is the runtime's transaction
  boundary (dump after every cell); a cell that outlives its dump has no
  heap to return to. `shell()` inside a cell is a job (§1.4) — that covers
  the long thing a cell would want to start.
- **A `services`/`start` concept (omp `hub start`) for never-exiting
  processes.** A job with a 30-minute deadline and `cancel_job` covers dev
  servers and watchers for one session. Readiness probes, restart
  policies and cross-session names are a plugin if ever needed. Revisited
  in 22 (§1, §4.6): the 22 versions are bounded, return the screen, and
  are for prompts and readiness, not for foregrounding a finishing job.
- **Streaming job output into the model mid-turn.** A user message per
  chunk would bust the prefix every round. `read_file` on the log is
  explicit and cache-safe.
- **Killing jobs on `/cancel`.** Cancel means "stop thinking", not "stop
  the build". Two verbs.
- **Persisting jobs (v0).** §5.
- **One shell per job without copy-back.** Would silently break `cd` /
  `export` between calls — the most common shell pattern in our own
  transcripts (`export PATH=… && cargo …`).

## 10. Edits to other docs

- **00 §4.** "Tool calls in one assistant message run concurrently" gains
  "; `shell` returns within `job_grace_secs`, the process continues as a
  job (14)". §12 phase row; known-gaps sentence: orphan kill on shutdown
  now done by `Jobs`, `setsid` escape stays.
- **01 §2.3 delivery table.** New row: `Jobs Finished → parent.tell(UserInput(<job_result>))`.
- **07 §2.2 / §8.8.** `<job_result>` is a harness input (no id, no `Input*`
  events); a job turn is undoable.
- **08 §5.** Runtime `shell()` return shape (§1.4).
- **README.** Config block (`job_grace_secs`, `job_timeout_secs`), TUI
  sentence (`/jobs`), remove `shell_timeout_secs`.

## 11. Implementation order

Each step leaves `theuth` green; steps 2–4 are independent of each other
after 1.

1. **`Jobs` actor + shell cutover** (`tools`): `jobs.rs`, `ShellTool::call`
   → `Start`, `jobs`/`cancel_job` tools, config keys, shutdown kill, tests
   1–5. `shell` behaves as today for fast commands.
2. **Delivery** (`agent`): `Agents` subscribes to `JobEvent`, injects
   `<job_result>`; runtime `shell()` stub shape; test 6.
3. **TUI + Signal**: status count, `/jobs`, `/jobs kill`, `CardKind::Job`,
   wrapper arm; test 7.
4. **RPC + docs**: `list_jobs`, `cancel_job`, event forward, bindings; §10
   edits; README.
5. **Smoke** (test 8), record in Status.

Size: ~300 lines `jobs.rs`, shell.rs net −40, agent ~50, tui ~80, rpc
~40, signal ~30. Two people: A does 1→2, B does 3→4 once 1's event
types land.

## 12. As-built 2026-09-22: per-call `shell` arguments

`ShellArgs` gained `cwd`, `timeout_secs` and `env`, each for one call only.
`cwd` (relative → session cwd; must be a directory) and `env` (exported)
are applied to the job's clone of the session shell, and that clone is
not `put_back`, so neither the overrides nor a `cd` inside the command
persist; `command`-only calls are unchanged. `timeout_secs` travels as
`Start::timeout` and becomes the job's own deadline, clamped to
`1..=job_timeout_secs`; the result reads `timed out after 30s
(timeout_secs=30)` (or `… capped to N`) and the row state is `timeout`.
`build_shell` still skips profile/rc files but seeds `PATH` once per
process from `$SHELL -l -i -c 'printf %s "$PATH"'` (3 s budget, stdin
`/dev/null`, stderr dropped; inherited `PATH` kept and warned once on
failure), off with `tools.login_path: false` or `THEUTH_NO_LOGIN_PATH=1`.
PTY `start` already runs `$SHELL -lc`, so it needed nothing.
