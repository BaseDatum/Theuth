# 61 — PTY idle: a quiet terminal wakes the model

## 0. The problem

A PTY job (22) is how the model drives another coding agent: `start omp`,
`send` a brief, and then… nothing. The job stays running, so the todo nudge
of 47 §3.2 is (deliberately) blocked; `<job_result>` (14 §0.3) arrives only
at exit, and omp never exits on its own. The only wake left is a human. In
practice the orchestrator starts omps at the user's direction, ends its turn,
and sits until the user asks "how is it going?" — the opposite of managing.

The `wait` tool does not fix this: it is bounded at 120 s (22 §4.6) and
burns a tool round per poll. The model needs what it already has for pipe
jobs — the harness telling it something happened — where "something" is
**the program stopped talking**: omp finished a step and is showing its
prompt, a REPL is waiting, a wizard asked a question, a build hung.

## 1. Decisions in one screen

1. **A quiet PTY job wakes the model once per quiet stretch.** When the
   rendered screen of a running PTY job has not changed for `idle_secs`
   and nobody has touched the job meanwhile, the harness injects a
   `<job_idle>` harness input through `Agents::inject` — the same path as
   `<job_result>` (14 §0.3): idle session → a new turn; turn in flight →
   queued after the current tool batch. The job keeps running; nothing is
   settled, retired or killed.
2. **Quiet means the screen stopped changing, not that bytes stopped.**
   Programs redraw identical frames, answer nothing, or reply to our own
   terminal queries (22 §4.4); none of that is news. The job task already
   renders rows on its 500 ms tick when dirty — comparing those rows with
   the last ones is the detector. Cursor position is excluded (a blinking
   or moving cursor over the same text is still idle). A spinner that
   changes text is *not* idle, correctly: the program is busy.
3. **Armed by activity, disarmed by firing.** The timer restarts on any
   screen change and on any interaction with the job: `send`, `pty_input`
   (a human typing, 55), a `screen` reply, a `wait` returning. Once fired,
   nothing fires again until the screen changes or someone interacts —
   there is no heartbeat. Since the program sits there unchanged and the
   model has been told, telling it again adds nothing.
4. **Consecutive nudges nobody acts on back off.** If a nudge fires and the
   next quiet stretch begins with no model interaction (`send`/`screen`/
   `wait`) on that job in between, the interval doubles: 60 → 120 → 240 …
   capped at 3600 s. Any interaction resets it to `idle_secs`. A dev
   server that logs one request line an hour thus nudges a few times and
   then effectively never; an omp the model is actively steering nudges
   at full rate.
5. **An outstanding `wait` on the job suppresses the fire.** The model is
   watching; the `wait` reply is the wake. When the `wait` returns the
   timer restarts (rule 3).
6. **PTY jobs only, on by default, `idle_secs: 0` turns it off per job.**
   `start` gains `idle_secs?: u64` (default `tools.pty_idle_secs`, default
   **60**; values 1–9 are clamped to 10 at the tool boundary; `0` = never).
   Pipe jobs (14) have a deadline, no stdin, and a result at exit; they
   are not quiet in any sense that matters.
7. **The body is the screen.** Same convention as `<job_result pty="true">`
   (22 §2.2): the model has been watching the screen all along, and the
   screen is what it must read to decide. Attributes: `id`, `command`,
   `name` (if any), `quiet` (seconds since the screen last changed), `secs`
   (job age), `nudge` (1 for the first of a backoff run, 2, 3…), and
   `user_typed` seconds ago when a human touched it within 55's 30 s window.
8. **Delivery rides the activity guard, not the bus.** `ActivitySink` grows
   a third recipient `Notice`; `Activity::notify(JobEvent::Idle {…})` sends
   it. `Agents` handles `Message<Notice>`: the operation must still be
   admitted (a late notice for a retired op is dropped with a log line),
   the payload is wrapped by `job_idle()` next to `job_result()` and
   `inject`ed with the op's network scope (50 §6.3), so an exported task's
   omp reports to the task's scope, never to a private turn. The bus gets
   `JobEvent::Idle` as observation for UIs; delivery never happens on both
   paths (47 §4.1).
9. **47 is not weakened.** A running PTY still blocks the todo reminder;
   `<job_idle>` is a job-owned future-result delivery, the kind 47 §3.1
   names as work, admitted as `InputOrigin::Automatic` (no reminder
   budget, 47 §4.5). Its ticket blocks quiescence until consumed, like a
   result's.
10. **The model is told what a nudge means.** Tool description of `start`,
    the `<job_result>` sentence in the REPL prompt, and the `send`/`wait`
    descriptions each gain one sentence: a `<job_idle>` shows a program
    that has stopped changing its screen — read it, `send` what it needs,
    `wait` for a pattern, `cancel_job` it, or do nothing; a server that
    only needs to keep running is started with `idle_secs: 0`.

## 2. Model-facing contract

### 2.1 `start`

```
idle_secs?: u64   // seconds of unchanged screen before <job_idle>; default 60; 0 = never
```

Description sentence: "A job whose screen stops changing for `idle_secs`
(default 60) wakes you with `<job_idle>` and the screen; `0` for a server
you only need running."

### 2.2 The wrapper

```
<job_idle id="j4" command="omp" name="omp1" quiet="63" secs="912" nudge="1">
… screen rows, blank trailing rows dropped …
cursor: row 38 col 3
</job_idle>
```

`command` cut to 120 chars and attribute-escaped, as `job_result`.
`user_typed="12"` is added when a human typed into the job within 30 s.
The `cursor:` line is the screen convention of 22 §2.2 (it is in the body,
not an attribute).

### 2.3 Where it lands

`AgentEvent::MessageDelivered { from: "job j4", text: <screen> }` as today
for results. The TUI's injected-input classifier (`tui/src/model.rs`
`injected` — `"job_result" => (Some("id=\""), "</job_result>")`) and the
desktop's (`desktop/ui/src/lib/injected.ts` `CARD`) learn `job_idle` and
title the card `j4 idle 63s · omp1`.

### 2.4 What the model does

Nothing new: `screen`, `send`, `wait`, `cancel_job`, `read_file` on the log.
Ending the turn with no action is a valid answer and costs nothing until
the screen changes again.

## 3. Implementation

### 3.1 `crates/tools/src/lib.rs`

`ToolsConfig.pty_idle_secs: u64` (default 60), doc comment next to
`pty_ready_timeout_secs`.

### 3.2 `crates/tools/src/jobs.rs`

`JobEvent::Idle { session, id, command, name, quiet_secs, secs, nudge,
user_typed_secs: Option<u64>, output: String }` — `output` is the screen
text with the `cursor:` line, exactly what the body will be. `#[serde(rename
= "JobIdle")]`. Observation only on the bus.

### 3.3 `crates/tools/src/activity.rs`

```rust
pub enum Notice {
    /// A running PTY job's screen has been unchanged for `quiet_secs` (61).
    JobIdle { op: OperationId, event: JobEvent },
}
pub struct ActivitySink { admit, settle, notice: Recipient<Notice> }
impl Activity { pub fn notify(&self, event: JobEvent) }   // no-op when untracked or Unknown
```

`ActivitySink::new` requires `Message<Notice, Reply = ()>` too; `notify`
is fire-and-forget like `settle`, logged on a closed mailbox. An untracked
guard (`sink == None`) drops the notice: coverage certified only under a
supervisor, as with results. A guard already `Unknown` sends nothing.

### 3.4 `crates/tools/src/pty.rs`

In `Run`: `idle_after: Option<Duration>` (from `StartArgs.idle_secs` or
config), `last_rows: Vec<String>`, `last_change: Instant`, `armed: bool`,
`nudges: u32`, `interval: Duration`. Shared with the actor side through
`PtyHandle`: an `AtomicU64 touched` (monotonic ms of the last `send` /
`pty_input` / `screen` reply / `wait` return, bumped by the handlers) and
an `AtomicUsize waiting` (outstanding `wait`s), plus an `AtomicBool
interacted` that `send`/`screen`/`wait` set and the job task clears when
it fires — that is rule 4's "did the model touch it between nudges".

Select arm:

```rust
() = tokio::time::sleep_until(deadline), if armed && idle_after.is_some() => { … }
```

where `deadline = max(last_change, touched) + interval`. On fire: if
`waiting > 0` skip (re-evaluate when the wait returns — the handler bumps
`touched`, which recomputes the deadline); else build `JobEvent::Idle`,
`j.activity.notify(event.clone())`, `j.rt.publish(event)`, `armed =
false`, `nudges += 1`, `interval = if interacted.swap(false) { base } else
{ min(interval * 2, 3600 s) }`. On the existing 500 ms tick when `dirty`:
compare `rows` with `last_rows`; changed → `last_change = now`, `armed =
true`, `last_rows = rows`. A bump of `touched` also sets `armed = true`
(the handler cannot reach the task; the task reads the atomic at each
wake, so make `armed` derive from `touched > last_fire || last_change >
last_fire`). The exact atomics are omp's call; the observable rules are
§1.3–1.5 and the tests in §5.

`StartArgs.idle_secs: Option<u64>`; clamp 1–9 → 10 in the tool's `run`,
`0` → `None`; carried through `Launch<StartPty>` into `Run`.

### 3.5 `crates/agent/src/supervisor.rs`

```rust
impl Message<Notice> for Agents {
    async fn handle(&mut self, m: Notice, _) {
        let Notice::JobIdle { op, event: JobEvent::Idle { session, id, command, name, quiet_secs, secs, nudge, user_typed_secs, output } } = m else { return };
        if self.ledger.op_owner(op).is_none() { tracing::info!(%op, "idle notice for a settled job dropped"); return; }
        let text = spill::cap(session, &format!("job-{id}-idle"), &output, self.cfg.max_tool_output_chars, &self.rt.data_dir.join("spill"));
        let wrapped = job_idle(&id, &command, name.as_deref(), quiet_secs, secs, nudge, user_typed_secs, &text);
        let scope = self.network_operation_scope(op);
        self.inject(session, &format!("job {id}"), text, wrapped, scope).await;
    }
}
pub(crate) fn job_idle(…) -> String   // next to job_result, same escaping
```

`ActivitySink::new(agents_ref)` where it is built today needs no change
beyond the new bound. `agent/src/lib.rs` doc on `MessageDelivered.from`
mentions `<job_idle>`.

### 3.6 Prompt and descriptions

`crates/agent/src/repl.rs` line ~70 ("result arrives later as
<job_result>") gains: "A PTY job whose screen stops changing for a minute
wakes you with `<job_idle>` and the screen: the program is likely waiting
on you." `StartTool`, `SendTool`, `WaitTool` descriptions: one sentence
each (§1.10). Keep every description under its current length + 200 chars.

### 3.7 UIs

TUI `model.rs` injected classifier: `"job_idle" => (Some("id=\""),
"</job_idle>")`, card title `j4 idle 63s`. Desktop `injected.ts` `CARD`
adds `job_idle: 'id="'` and the same title. `just ts` regenerates the
`JobEvent` binding (`JobIdle` variant); `reduce.ts` needs no handling
beyond not crashing on the new variant (check the switch is non-exhaustive
or add a no-op case).

## 4. Docs to edit

- 22 §1.4 list of tools unchanged; §2.1 `start` gains `idle_secs`; §9
  "Streaming `Screen` events into the model mid-turn" stays rejected — add
  "(a *quiet* screen is one event per stretch, 61)". §11 status: one line
  pointing here.
- 14 §0.3: "…and a quiet PTY job wakes it with `<job_idle>` (61)".
- 47 §3.2 after "PTY services can therefore suppress nudges indefinitely":
  "A quiet PTY job wakes the model on its own through `<job_idle>` (61), a
  job-owned delivery like a result; this does not weaken the rule."
- 55 §2.3: `user attached` also appears as `user_typed` on `<job_idle>`.

## 5. Verification (done when)

Unit (`cargo test -p tools -p agent -p tui`):

1. `pty_idle_fires_once_after_quiet`: `printf 'hi\n'; sleep 30` with
   `idle_secs` small (the actor accepts any value; only the tool clamps) →
   exactly one `JobEvent::Idle` on the bus with `nudge == 1`, `output`
   containing `hi`; none in the following 2× interval.
2. `pty_idle_rearms_on_screen_change`: `sh -c 'echo a; sleep 2; echo b; sleep 30'` →
   two `Idle` events, the second `quiet_secs` measured from `b`.
3. `pty_idle_send_resets_and_resets_backoff`: after a fire, `SendInput`
   → no fire for a full base interval, then one with `nudge == 1`.
4. `pty_idle_backoff_doubles_without_interaction`: `while :; do echo x; sleep 1.5; done`
   with a 1 s interval is *never* idle (screen changes) — negative
   control; then `sh -c 'for i in 1 2 3; do echo $i; sleep 4; done; sleep 60'`
   with a 1 s interval → `nudge` 1, 2, 3 and gaps ≈ 1, 2, 4 s.
5. `pty_idle_suppressed_while_waiting`: outstanding `Wait` with a 10 s
   timeout on a quiet job → no `Idle` until the wait returns; one after.
6. `pty_idle_off`: `idle_secs: 0` → no event in 3× interval.
7. `pty_idle_ignores_cursor_only_redraws`: a program that re-prints the
   same frame every 200 ms with `\r` (same rows) → fires.
8. `activity`: `notify` on an untracked guard is a no-op; on an `Unknown`
   guard sends nothing; on a tracked guard the ledger recorder sees
   `Notice::JobIdle`.
9. `supervisor`: `job_idle` wrapper escaping (mirror
   `job_result_wrapper_escapes_and_names_exit`); a `Notice` for a retired
   op injects nothing; a `Notice` for a live op on an idle session starts a
   turn whose first input is the wrapper (mirror the existing
   `<job_result>` injection test).
10. TUI `injected_parses_job_idle`; desktop `bun test` in
    `crates/desktop/ui` for `injected.test.ts` with a `job_idle` case.

Smoke, with a throwaway `THEUTH_DATA_DIR`: `start python3 -q`, end the
turn, and within ~65 s the model is woken by `<job_idle>` showing `>>> `;
`send 1+1` → new stretch → a second `<job_idle>` ~60 s later with `nudge=1`;
leave it → the third comes ~120 s after the second. `start … idle_secs: 0`
→ nothing for 3 minutes. Then the real case: `start omp`, `send` a
one-line prompt, and confirm the wake arrives once omp shows its idle
prompt and not while its spinner runs.

## 6. Rejected

- **A heartbeat every N seconds while idle.** Cache-busting and noise (the
  reason 14 §9 / 22 §9 rejected streaming screens). One wake per quiet
  stretch, backing off, is enough: the model has the screen and can `wait`.
- **Bytes-quiet instead of screen-quiet.** Spinners, clocks and query
  replies would break both ways; the screen is what the model reads.
- **Heuristics on the command line** (omp → on, servers → off). 47 §3.2
  forbids command-name exemptions for quiescence; the same spirit here.
  `idle_secs: 0` and the backoff replace guessing.
- **A tool to change `idle_secs` after `start`.** Restart the job, or let
  the backoff quiet it. Revisit if transcripts show the need.
- **Weakening 47 §3.2 to "no job whose result seems useful".** The rule
  stands; this adds an event source, not an exemption.
- **Bus-delivered wake** (agent subscribes to `JobEvent::Idle`). 47 §4.1
  moved result delivery onto the guard so ownership and scope are known
  at delivery; a notice needs the same scope resolution.
- **Nudging `shell` jobs.** They exit or time out; there is nothing to
  type at them.

## 7. Order

A. Config + `JobEvent::Idle` + `Notice`/`notify` in `activity.rs` (tests 8).
B. `pty.rs` detector, `start.idle_secs`, handler touches (tests 1–7).
C. `supervisor.rs` `Notice` handler + `job_idle` wrapper (tests 9);
   prompt and description sentences.
D. TUI + desktop classifier, `just ts` (tests 10).
E. Docs (§4) and the as-built section below; smoke.

One commit per section.

## 8. Status (as-built)

_Appended by the implementer; append-only._
