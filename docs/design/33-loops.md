# 33 — Loops: a schedule with a stop

Status: as-built v2 · 2026-09-16 · **extends `34-schedule.md`** (as-built the
same day); `00-architecture.md` §4 (tools), §5 (harness inputs);
`01-config-and-subagents.md` §2 (detached subagents, `agent:` external
harnesses via 06 §3.2); `24-message-user.md` §2 (delivery);
`28-untrusted-content.md` (shell output into the frame); `04-prompt-skills-mcp.md`
§1 (skills the binary owns). v0 (same day, one hour before 34 landed) designed
a `loops` crate with its own directory, actor, sqlite, six commands, five
tools, two events and five states; 34 then shipped every one of those parts
for timers. v1 folded the stop into 34. v2 adds what a *supervised
engineering loop* needs — arm without starting, status from chat, the model
declaring done, fan-out inside an iteration — after the port-a-project case
in §2 was walked through. Where 33 and 34 disagree, 34 wins for everything
it already does. Source material unchanged:
`../jarvis/crates/jarvis-loops/src/{spec,driver,service,ledger}.rs`,
`../oh-my-pi/packages/coding-agent/src/modes/{loop-limit,loop-condition}.ts`.

## 0. Decisions in one screen

1. **A loop is a schedule file with a stop.** `until:` and/or `max_runs:`
   on a 34 file. Nothing else is new: `~/.theuth/schedules/<name>.md`, the
   watcher, the tick, one session per schedule, the wrapper, `Deliver`,
   `schedule.sqlite`, the `schedule` tool, `/schedules`, Settings ›
   Schedules, the RPC — all 34. Rejected (v0): a `loops/` directory and
   `loops` crate beside `schedules/` — two registries, two watchers, two
   tables, two tools and a "which one do I want" paragraph for the model.
2. **Done is a shell command or the model's word.** `until: <cmd>` runs
   *before each fire*: exit 0 → done; 1 → fire, with the output tail in
   the wrapper; anything else → the check is broken, the loop ends with
   that error (oh-my-pi's exit-code contract, jarvis's `command` gate).
   `schedule_done { why }` from inside a run ends the calling loop with
   `done (model): <why>` — a deliberate act with a reason on record.
   Rejected: a completion phrase in the reply (ralph's `<promise>`: a
   string match on prose, no `why`); a side-model judge (jarvis `judge`:
   the run *is* the judge).
3. **Back to back is `every: 0s`.** 34's tick already defers a fire that
   comes due mid-run to the first tick after the run ends (34 §13); an
   interval of zero is "again as soon as it finishes". The 10 s floor
   (`file.rs` `MIN_EVERY`) is lifted only when the file has a stop.
4. **Ending is consumption, like `at:`.** A one-shot is removed by its
   fire; a loop is removed by its done. The last history row says why
   (`done: until passed after 4 runs`, `done (model): all 41 tests
   ported`, `max_runs 20 reached`, `until broken: exit 127 …`) and the
   user is told through `Deliver` regardless of `quiet`. No state enum, no
   rewriting the user's file at the end.
5. **A run is over when its subtree is idle, not at the parent's
   `TurnFinished`.** A supervisor iteration spawns children and goes idle;
   each `<subagent_result>` wakes it for another turn in the same
   iteration. The run closes on a `TurnFinished` after which `ListAgents`
   shows no live descendant; the deadline cancels descendants too. This
   also fixes 34: today the next fire's `Compact { Clear }` would wipe a
   session still awaiting its children.
6. **`enabled: false` is the draft; flipping it is the start.** The model
   designs the loop in chat and writes it disabled; the approval prompt is
   the design review; nothing fires. "Kick it off" → the model edits the
   one line (`edit` on the file — it has the tool and the prompt block
   names the directory), or you `/schedules resume <name>`, or the desktop
   toggle. `pause` is the same line the other way: the iteration in flight
   (and its children) finishes, nothing new fires. `/schedules run <name>`
   stays "one iteration now", for trying one while you watch.
7. **Status from chat is a tool.** `schedule_status { name? }` (ReadOnly)
   returns what `/schedules` and `/schedules history` show, plus each
   run's **report** — the run's final text, capped at 2 KiB, stored in a
   new `report` column. 34 §7 rejected storing text; "how is the loop
   going" is the case that earns it.
8. **Unattended means `approvals: false`, in the file.** Maps to the
   per-session switch (`tools::SetApprovals { session, enabled: false }`,
   `registry.rs:119`), re-sent on every fire; shown in the `schedule`
   approval prompt (the grant) and in every table.
9. **`cwd:` because a check needs a directory.** `until` runs there and
   the session is created there (`NewSession { cwd }`), so the run's
   `shell` starts in the repo.
10. **Memory is `<until>` output, the repo, and knowledge.** Fresh context
    per iteration (ralph). A supervisor keeps its plan in a file in `cwd`
    (`PORT.md` below) — git sees it, the user reads it, the next iteration
    starts from it. Rejected: `<previous_run>` (v0) and a continuous
    supervisor session (grows until compaction; the model then *thinks* it
    remembers).
11. **No `steps:`.** A staged brief is a numbered list in the body; the
    model has `todo`. Fan-out is `spawn_subagent`, which already takes
    `agent:` for an external harness.
12. **Counts, not states.** `max_runs` is compared with the fires since the
    loop's last **end row** (§3.2). Pausing, resuming and bumping
    `max_runs` continue the count; a re-created loop starts at 1.
13. **A skill, not a subsystem, teaches the supervisor pattern.**
    `skills/loops.md`, owned by the binary like `visualize.md` (04 §1,
    `main.rs:215`): the §2 file as a template, the plan-file contract,
    draft-disabled-then-flip, `quiet: true` on iterations, `schedule_status`
    when asked.

## 1. What v0 wanted, and where it went

| v0 | here |
|---|---|
| `loops/<name>.yaml`, `Loop` struct, 11 keys, `validate → Vec<SpecError>` | `schedules/<name>.md` + 4 keys (§2); 34's parser, one error string |
| crate `loops`, `LOOPS_ACTOR`, 6 commands | `crates/schedule`; asks `SetEnabled`, `Done` added |
| `LoopEvent::{LoopChanged, RunFinished}` | `ScheduleEvent::{Changed, Fired, Skipped}`; ending = file removed → `Changed` + history row + `Deliver` |
| `LoopState` ×5, `RunStatus` ×4 | `runs.result` strings (34 §7) + four more (§3.3) |
| a session per run, `sessions.origin`, `/sessions` folding, `LastText` | one session per schedule, clear per fire (34 §1.4); `report` column |
| `every: 10m` from the run's *end*, `Manual` (no `every`) | 34's `every` from `last` fire; `enabled: false` + `resume` / `run` |
| `until` before each run, exit 0/1/≥2, 120 s, 2 KiB tail | same (§3.1) |
| `Enable { false, why }` on done / broken / cap; `Enable` for pause | remove the file on end (§3.3); `SetEnabled` rewrites the `enabled:` line for pause/resume (§3.5) |
| steps as turns, descendant tracking for steps | descendant tracking for the *run* (§3.4); no steps |
| `loop_put/list/run/stop/delete` | `schedule` (+5 args) · `schedule_done` · `schedule_status`; `/schedules run`; `rm` |
| "a run may not make loops" | not enforceable under `approvals: false` (`write_file` into `schedules/`); §7 states the ceiling |
| `loops.{max_concurrent, timeout_secs, keep_runs, catch_up_secs}` | `schedule.run_timeout_secs`, `missed: run`; no cap, no pruning |
| `⟳` picker, sidebar section, card `Spec`, interject, badge | 34's table and tab gain columns and a toggle (§6) |

## 2. Keys, and the case they are measured against

Porting a project to another language, one test at a time, with a
specific external harness doing the work:

```markdown
---
every: 0s                          # again as soon as an iteration ends; allowed because there is a stop
until: ./scripts/missing-tests.sh  # exit 0 when every original test has a counterpart; 1 otherwise
max_runs: 200
approvals: false                   # nobody is watching; the user granted this when approving the file
cwd: ~/Develop/rewrite             # until runs here; the session starts here
quiet: true                        # iterations do not spam the user; the end always reaches them
enabled: false                     # drafted in chat; nothing fires until it is flipped
---
Read PORT.md. Pick one test in ../original that has no counterpart here (the
`<until>` block lists them). Spawn a subagent with `agent: opencode` in this
directory: write that test, change the code until the whole suite passes,
report what changed. Wait for its result, run the suite yourself, update
PORT.md (one line per test: name · state · notes). If nothing is missing,
call `schedule_done`.
```

One fire = one supervisor iteration with a fresh context; `PORT.md` and the
`<until>` output are its memory; the child is any harness `agents.yaml`
names. The child's result wakes the supervisor inside the same iteration
(§3.4); when the supervisor's turn ends with no live children the run
closes and `every: 0s` fires the next. Done when the script says so or the
model does.

`FrontMatter` (`file.rs:82`, `deny_unknown_fields` — `untill:` is an
error, not an unbounded loop) gains `until: Option<String>`, `max_runs:
Option<u32>`, `cwd: Option<PathBuf>`, `approvals: Option<bool>`; `Spec`
gains the same (`approvals` defaulting `true`). Rules added to 34 §2.2:

| key | rule |
|---|---|
| `every: 0s` | accepted iff `until` or `max_runs` is set; below 10 s otherwise remains an error |
| `until` | non-empty; nothing else checked — the first fire finds out (exit ≥ 2 ends the loop with the message) |
| `max_runs` | ≥ 1 |
| `cwd` | leading `~` expanded; must be a directory at parse |
| `at:` + `until`/`max_runs` | error: a one-shot has no loop to stop |

Other worked files (the skill's examples, §5.3): **fix the tests** —
`every: 0s`, `until: cargo test -p agent`, `max_runs: 20`, `approvals:
false`, `cwd`; **watch a URL** — `every: 10m`, no `until`, body ends "if
the page changed, message me and call `schedule_done`; else reply `nothing
to report`", `quiet: true`; **brief until the incident closes** — `cron:
"0 7 * * 1-5"`, `until: test "$(gh issue view 42 --json state -q .state)"
= CLOSED`.

## 3. Runtime (`crates/schedule/src/lib.rs`, changes to 34 §3–4)

### 3.1 The check

In `fire()`, before 34 §4 step 1, when `spec.until` is set and the fire is
not a forced `Fire` (`/schedules run` means *run*):

```rust
let out = tokio::time::timeout(UNTIL_TIMEOUT, Command::new(shell()).arg("-lc").arg(until).current_dir(cwd)
    .stdin(Stdio::null()).output()).await;                       // UNTIL_TIMEOUT = 120 s
match out.map(|o| (o.status.code(), tail_2k(&o.stdout, &o.stderr))) {
    Ok((Some(0), _))      => end(name, format!("done: until passed after {n} runs")),
    Ok((Some(1), tail))   => continue firing with `Some(tail)`,
    Ok((code, tail))      => end(name, format!("until broken: exit {code:?}: {tail}")),
    Err(_)                => end(name, "until broken: no exit in 120s"),
}
```

`shell()` is `$SHELL` else `/bin/sh` (the `shell` tool's rule); `cwd` is
`spec.cwd` else the harness's. No stdin, the harness's environment.
`tail_2k` keeps the last 2 KiB of stdout then stderr, lossy UTF-8.

### 3.2 Counting

`store.count_since_end(name) -> u32`: fires (`session IS NOT NULL AND
(result IS NULL OR result NOT LIKE 'skipped:%')`) with `id` greater than
the newest **end row** for the name — a row whose `result` starts `done`,
`max_runs`, `until broken` or `removed`. `lost` rows are fires (they
were). `max_runs` is checked after `until` (a check that passes ends the
loop as done, not as capped) and before the fire: `n >= max_runs` →
`end(name, "max_runs {max} reached")`. `ScheduleInfo.runs` is this count.

### 3.3 Ending

`end(name, why)`: `INSERT` a row `(name, session, now, now, why, NULL)`;
`remove_file` (the reconcile that follows drops the entry — the `at:` path
of 34 §4 step 4); `Deliver { origin: "schedule {name}", text: "schedule
{name} {why}" }` regardless of `quiet` (34 §4.1's `error | timeout` arm
gains these). `RunInfo.result` documents four more strings: `done: …`,
`done (model): …`, `max_runs … reached`, `until broken: …`; and
`removed` — written by reconcile when a file vanished other than by the
actor's own hand (an `at:` fire or an `end`), so history says "you deleted
it at 14:02" and the count restarts for a re-created name. `last_result`
shows end rows: they are the last word.

### 3.4 A run and its subtree

`Run` gains `children: HashSet<SessionId>`, `pending: HashSet<SessionId>`
(direct children whose result has not landed), `turn_open` and
`last_stop`. `AgentEvent::SubagentSpawned { parent, child }` with `parent`
the run's session → `children += child`, `pending += child`; `parent`
already in `children` → `children += child` (grandchildren count).
`PeerDelivered { session: run, origin.from: "session:<child>" }` (50 §10;
`MessageDelivered` no longer fires for peer bodies) → `pending -= child`.
`TurnStarted`/`TurnFinished` on the run's session track `turn_open`.

On `TurnFinished`: a run with no children (34's case) or a timed-out one
closes at once. A run that fanned out **settles** instead: `Settle` is
self-told after `SETTLE` = 500 ms, and closes the run only if no turn is
open, `pending` is empty and `ListAgents { session }` shows no descendant
`Running | Awaiting`. Reason for the grace: a child's result that arrives
while the parent is streaming its last text is queued, and the agent
starts the next turn *inside* `TurnDone` — its `TurnStarted` reaches this
actor milliseconds after the `TurnFinished`, so no check at `TurnFinished`
time can tell "consumed by this turn" from "queued for the next". A
`TurnFinished` of a child is ignored (the parent's wake follows). The
report is the text of every turn in the run (`spawned … child came back`).

Deadline: `Cancel` every live descendant, then the session; the run closes
as `timeout` on the parent's `TurnFinished`. `ponytail:` a child that is
cancelled without ever delivering keeps the run open until the deadline.

### 3.5 The rest

- 34 §4 step 2 (no session yet) → `NewSession { cwd: spec.cwd }`.
- Every fire, `approvals: false` → `SetApprovals { session: Some(id),
  enabled: false }` on `tools::REGISTRY_ACTOR` (in-memory; the registry
  forgets on restart, the session may be old).
- `TurnFinished` closing a run → `finish_run(id, now, result,
  Some(tail_2k(text)))`: the report.
- `SetEnabled { name, enabled }` (ask → `Result<ScheduleInfo, String>`):
  rewrite the `enabled:` line of the file (`file::set_enabled(text,
  enabled)`: replace the line inside the front matter, insert one after the
  opening `---` when absent; `write_atomic`) → the watcher reconciles →
  `Changed`. `Err("no such schedule")`. Resuming a `0s` loop fires it on
  the next tick; the count continues (§3.2).
- `Done { session, why }` (ask → `Result<String, String>`, from the tool):
  the schedule whose live run is `session` or has `session` in its
  `children` → `end(name, "done (model): {why}")`, reply `"ended {name}"`;
  `Err("not a schedule run")` otherwise. The run in flight finishes
  normally (its row closes as usual; the end row precedes it in id order,
  which is fine — the file is gone, nothing fires).
- Restart: a `lost` row counts as a fire; a `0s` loop is then due at boot
  and runs — bounded by its own stop.
- Closing a run self-tells `Tick`, as `Reconcile` already does: the tick
  loop computes its sleep once (up to 60 s) and nothing else wakes it, so
  without this an `every: 0s` loop waited up to a minute between
  iterations (§11). Deadlines are still checked only on ticks — a timeout
  can land up to 60 s late, which `run_timeout_secs` (minutes) absorbs.

## 4. The wrapper (34 §1.5, one block and two attributes added)

```
<schedule name="port-tests" at="2026-09-16T12:04:11+01:00" quiet="true" run="14" of="200">
<until exit="1">
test_parse_dates
test_roundtrip_unicode
</until>
Read PORT.md. …
</schedule>
```

`run` present when `until` or `max_runs` is set; `of` only with
`max_runs`; `<until>` only when there is a check. `<until>` content is
tool output by 28's rules: data inside a tag the prompt block names. The
TUI/desktop `schedule` card arm (34 §8) collapses the body; the block is
inside it.

## 5. Model-facing surface (`crates/schedule/src/tools.rs`)

### 5.1 Tools

| tool | risk | args | returns |
|---|---|---|---|
| `schedule` | Mutating | 34's + `until?`, `max_runs?`, `cwd?`, `approvals?` (default true), `enabled?` (default true) | 34's reply + ` · until: <cmd>` / ` · up to N runs` / ` · approvals off` / ` · disabled: resume with /schedules resume <name> or by editing enabled:` |
| `schedule_done` | ReadOnly | `{ why }` | `ended <name>: <why>`; `Err("not a schedule run")` outside one. Only ever prevents action; must work from an unattended run whose approvals would time out to deny (10 §6's `knowledge_put` argument) |
| `schedule_status` | ReadOnly | `{ name? }` | no name: one line per schedule `name · when · next · runs 14/200 · running 3m · last: end_turn`; name: those fields plus `until`, `approvals off`, `disabled`, then the last 10 rows `fired · took · result · report (first line, ≤ 120 chars)` and the newest report in full (≤ 2 KiB) |

`schedule_done` finds its loop through `ToolCx.session` (the field
`message_user` reads, 24 §3) via `Done`. `schedule_status` is
`ListSchedules` + `History` formatted; `structured` carries the same as
JSON.

### 5.2 Prompt block (appended to 34 §6's paragraph)

> A schedule with `until` (a shell command) or `max_runs` is a loop: it
> runs again until the command exits 0, the cap is reached, or a run calls
> `schedule_done` — use `every: 0s` for "keep at this until…". The check's
> last output arrives in `<until>`. Draft a loop with `enabled: false` and
> show the user the file; start it by editing that line when they say so.
> `approvals: false` lets a run act without permission prompts: propose it
> only when nobody will be there to answer, and say so. When asked how a
> loop is going, call `schedule_status`. Read `skills/loops.md` before
> designing one that supervises subagents.

Measured, not asserted (30 §6.4), through `theuth -p`: five requests that
should become loops ("keep fixing until the tests pass", "check this page
every 10 minutes until it changes", "port these files, at most 8 tries",
"brief me every weekday until #42 is closed", "set up a loop that ports
one test per iteration with opencode — don't start it yet") and three that
should not ("summarise this now", "remind me at 3", "every morning brief
me"); target ≥ 4/5 `schedule` calls with a stop, the last one `enabled:
false`, 3/3 without a stop. Counts into §11.

### 5.3 Skill `skills/loops.md` (`crates/schedule/loops.md`, `include_str!`, written by `first_run` like `visualize.md`)

Sections: the four keys and the `0s`-needs-a-stop rule; **the supervisor
template** (§2's file verbatim with the placeholders named); **the plan
file contract** (`PORT.md`/`PLAN.md` in `cwd`: one line per unit `name ·
todo|doing|done · note`; the iteration reads it first and rewrites it
last; never in the front matter, never in knowledge); **fan-out inside an
iteration** (`spawn_subagent { agent, cwd, brief }`, wait for the result,
verify yourself, then update the plan — the run stays open while children
live); **draft disabled, show the file, flip on go**; **quiet iterations,
loud ending**; **`schedule_done` when the plan is empty, `until` when a
script can tell**; the three other worked files; anti-patterns (a `0s`
loop without a stop; `approvals: false` for a loop that only reads; asking
the user a question from inside a run; keeping the plan in the prompt).

## 6. Surfaces (34 §8, columns and a toggle)

- `ScheduleInfo` gains `until: Option<String>`, `max_runs: Option<u32>`,
  `runs: u32`, `approvals: bool`, `cwd: Option<PathBuf>`. `RunInfo` gains
  `report: Option<String>`. Bindings via `just ts`.
- **RPC**: `enable_schedule(name, enabled) -> ScheduleInfo`.
- **TUI**: `/schedules` table gains `runs` (`14` or `14/200`; `—` without
  a stop); `/schedules pause <name>` / `resume <name>` →
  `Command::EnableSchedule(name, bool)`; `/schedules history <name>`
  header line adds `until: <cmd>` and `approvals off` when set, rows gain
  the report's first line.
- **Desktop** Settings › Schedules: an enabled toggle (`enable_schedule`),
  a `runs` column, the `until` command in mono under the describe line, an
  `approvals off` chip; history rows show the report's first line and
  expand to the full report.
- **Slack / Signal**: nothing new; ends arrive through `Deliver`.

## 7. Security (28) and ceilings

`until` is a shell command from the user's file or from a tool call the
user approved with its arguments visible; it runs in `cwd`, 120 s, output
tagged. `approvals: false` is the existing per-session switch, shown in the
approval prompt and every table. **A run with `approvals: false` can write
`schedules/*.md`** with `write_file` and so make further unattended
schedules; a tool refusal would not stop that, so this doc does not pretend
to — the prompt block states when to propose it and the grant is the
user's. Bounds: `max_runs`, `until`, the `0s`-needs-a-stop rule,
`schedule.run_timeout_secs` (now covering the subtree), `agent.max_tool_rounds`,
`agent.subagents.*`; one `Deliver` per fire end at most.

## 8. Data (34 §7)

Migration `0002`: `ALTER TABLE runs ADD COLUMN report TEXT` (the run's
final text, ≤ 2 KiB; NULL for skips, ends and `lost`). Five new `result`
strings (§3.3). `count_since_end` is one query. Deleting `schedule.sqlite`
resets every count and forgets every report — history, not truth.

## 9. Config

None added. `UNTIL_TIMEOUT` is a constant (`ponytail:` key it when a check
needs more than two minutes).

## 10. Plan and checks

1. **`file.rs`**: four keys, the `0s` rule, `at` + stop error, `cwd`
   existence, `set_enabled`, `render` for the new keys. Tests beside the
   existing ten: `every: 0s` rejected alone and accepted with `max_runs:
   1`; `at:` with `until` rejected; `cwd` missing rejected; `set_enabled`
   replaces an existing line, inserts a missing one, round-trips through
   `parse`. Check: `cargo test -p schedule`.
2. **`store.rs`**: migration, `report` on `finish_run`/`history`,
   `count_since_end` (test: fires · a skip · an end row · fires → count is
   the fires after the end row).
3. **`lib.rs`**: §3 entire. Check, headless on a scratch data dir:
   `every: 0s`, `until: test -f /tmp/theuth-x`, `max_runs: 3`, `approvals:
   false`, body "create /tmp/theuth-x with `touch`" → one run, the next
   tick's check passes, the file is gone, history ends `done: until passed
   after 1 runs`, log `delivered`/`not delivered`; body "do nothing" →
   three runs then `max_runs 3 reached`; `until: nonexistent-cmd` → no
   run, `until broken: exit 127`; body "call schedule_done with why
   'proof'" → `done (model): proof`, file gone; `enabled: false` draft →
   listed, `next: null`; `enable_schedule(name, true)` → fires within a
   tick; `pause` mid-loop → the run finishes, no new fire; a body that
   spawns a subagent → the run stays `running` until the child's result
   turn ends, then fires again; `schedule_status` from a chat session
   names runs and the last report; kill the daemon mid-run, restart →
   `lost`, the loop resumes with `run=` counting it; `rm` a loop's file →
   `removed` row.
4. **Tools + prompt block + skill**: §5. Check: "set up a loop that ports
   one test per iteration with opencode — don't start it yet" from a TUI
   session → one `schedule` call with `every: 0s`, a stop, `enabled:
   false`; "kick it off" → an `edit` flipping the line; "how's it going"
   → `schedule_status`; `load_skill loops` returns the template; then
   §5.2's 8-request measurement.
5. **Surfaces**: `ScheduleInfo`/`RunInfo` fields, RPC method, TUI
   commands/columns, desktop toggle/columns; `just ts-desktop`, `tsc -b`,
   vitest.
6. **Docs**: 34 §2.2/§4/§6/§8 gain "see 33" lines; 00 §12 phase 22 row,
   §9 skills list; README config example; §11 here.

## 11. Status

Shipped 2026-09-16, same day as the design. Diff: `file.rs` +190 (4 keys,
rules, `set_enabled`, `yaml_str`, 4 tests), `store.rs` +60 + migration
0002, `lib.rs` +330 (check, count, `end`, `removed`, subtree, `Settle`,
`SetEnabled`, `Done`), `tools.rs` +230 (`schedule_done`, `schedule_status`,
5 args, prompt), `loops.md` (skill, written by `first_run`), RPC
`enable_schedule`, TUI `pause`/`resume` + columns + history header,
desktop toggle/columns/report, README, 34 pointers, 00 phase 23. Gates:
`cargo clippy --workspace --all-targets -D warnings`, `cargo test` on
`schedule` (16) / `tui` (43) / `rpc` / `theuth`, `just ts-desktop`, `tsc -b`,
vitest 101 — green.

Smoke found three defects in the first build; each is in §3 now:

- **The tick loop slept 60 s through a run's end.** `NextWake` is computed
  once per sleep and nothing woke it; every back-to-back fire in the first
  fan-out test was actually driven by a reconcile-triggered `Tick` (a file
  write). Now a run's close self-tells `Tick` (§3.5); the next iteration of
  an `every: 0s` loop fires within 1 ms of the close.
- **`Idle` means "not started yet" too.** A parent that ends its turn
  before its just-spawned child begins saw no live descendant and closed.
  Direct children are `pending` until their `MessageDelivered` (§3.4).
- **A result queued during the parent's final streaming starts the next
  turn inside the agent's `TurnDone`**, so at `TurnFinished` time nothing
  distinguishes "consumed" from "queued" — the first `pending` build closed
  run 1 with "spawned" and booked "child came back" to run 2. A run with
  children now settles 500 ms after `TurnFinished` unless a `TurnStarted`
  landed meanwhile (§3.4 `Settle`). Plain runs close at once as in 34.

Also changed while smoking: `schedule_status <name>` for a loop whose file
is gone answers from history (`fanout · no file (ended or removed) · last:
max_runs 3 reached` + rows) instead of `no such schedule` — the file is
consumed by its end, the question "how did it go" outlives it.

Exercised headless (`claude-sonnet-4-5`, scratch data dir, no channel):
`until: test -f` loop → one run (the `shell` call ran under global
approvals on, so `approvals: false` held), check passed, file gone, row
`done: until passed after 1 runs`; `max_runs: 2` → two back-to-back runs
0.6 s apart then `max_runs 2 reached`; `until: nonexistent` → no run,
`until broken: exit 127: zsh: command not found …`; `enabled: false`
draft listed with `next: null`, `enable_schedule(true)` → fired, the run
called `schedule_done` → `done (model): proof`, file gone;
`enable_schedule(false)` mid-loop → line inserted after `---`, in-flight
run finished, no new fire; fan-out ×3 → each report `spawned … child came
back`, `Settle` logged `turn_open=true` once per iteration, cap end at the
same instant as the last close; `rm` of a running loop's file → `removed`
row; kill mid-run + restart → `lost`, loop resumed at boot with `runs
2/6`; a chat session asked "how did fanout go, how is slow doing" → two
`schedule_status` calls, correct one-line summary. Reports stored (≤ 2
KiB) and shown.

Not exercised: delivery of an end into a real owner DM; `run_timeout`
cancelling a subtree; `cwd` + `until` in a real repo with an external
`agent:` harness (the §2 port loop end to end); the TUI on a pty and
Settings › Schedules in a browser (unit tests / `tsc` + vitest only); the
§5.2 eight-request measurement; `load_skill loops` from a model.
