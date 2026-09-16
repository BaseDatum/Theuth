---
name: loops
description: Design a schedule that loops until done — retry loops and supervisors that fan out to subagents.
---

# Loops: a schedule with a stop

A loop is a schedule file in `~/.wth/schedules/<name>.md` with a stop. Nothing
else is new: the `schedule` tool writes it, `/schedules` lists it, deleting the
file cancels it. Each fire is one **iteration**: a fresh context, the prompt
as the whole input, and the run's reply delivered when it ends (unless
`quiet`). When the stop fires the file is removed and the user is told why.

## The four keys

| key | meaning |
|---|---|
| `until: <cmd>` | shell command run in `cwd` **before each fire**. Exit 0 → done, file removed. Exit 1 → run again; the command's output tail arrives in the iteration as `<until>…</until>`. Anything else → the check is broken, the loop ends with that error. |
| `max_runs: N` | stop after N runs (`max_runs N reached`). Counted since the loop was created or last ended; pause/resume does not reset it. |
| `cwd: <dir>` | where `until` runs and where the run's session (its `shell`) starts. `~` expands. Must exist. |
| `approvals: false` | the run never asks permission. Only when nobody will be there to answer, and say so when you propose it. |

`every: 0s` means "again as soon as an iteration ends" and is accepted **only
with `until` or `max_runs`**. A one-shot (`at:`) cannot have a stop. A loop
without `until`/`max_runs` that should end on the model's word (the URL
watcher below) still has a stop: `schedule_done`.

## The supervisor template

Porting a project to another language, one test per iteration, with an
external harness doing the work:

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

Placeholders to replace: the `until` script (anything that exits 0 when the
work is finished and 1 with the remaining units on stdout), `max_runs` (a
ceiling, not a target), `cwd` (the repo), the plan file name (`PORT.md`), the
unit of work ("one test"), the harness (`agent: opencode` — any name from
`~/.wth/acp/agents.yaml`, or omit `agent` for a wth subagent), and the body's
verification step ("run the suite yourself").

## The plan file contract

The loop's memory is a file in `cwd` — `PORT.md`, `PLAN.md`, whatever the
body names. One line per unit: `name · todo|doing|done · note`. The
iteration reads it first and rewrites it last. Git sees it, the user reads
it, the next iteration starts from it. Never keep the plan in the front
matter, in the prompt body, or in `knowledge_put`: the prompt is fixed for
every iteration and knowledge is for facts, not a checklist.

## Fan-out inside an iteration

The supervisor picks a unit, then calls `spawn_subagent { task, agent, cwd }`
with a complete brief (the child sees nothing of the run). It waits for the
result, **verifies the work itself** (run the suite, read the diff), then
updates the plan. The run stays open while children live: the child's
result wakes the supervisor for another turn in the same iteration, and the
run closes only when a turn ends with no live child. The run timeout covers
the whole subtree.

## Draft disabled, show the file, flip on go

Design the loop in chat and write it with `enabled: false`; the approval
prompt for `schedule` is the design review and nothing fires. Show the user
the file. When they say go, `edit` the one line to `enabled: true` (or they
run `/schedules resume <name>` or use the desktop toggle). `/schedules run
<name>` fires one iteration now, for watching one before letting it loop.
`pause` is the same line the other way: the iteration in flight finishes,
nothing new fires.

## Quiet iterations, loud ending

`quiet: true` on any loop with more than a handful of iterations: the reply
of each run is not delivered; `message_user` still is, for something the user
must see now. The end — `done`, `max_runs`, `until broken`, `schedule_done` —
always reaches the user regardless of `quiet`.

## `schedule_done` when the plan is empty, `until` when a script can tell

Prefer `until` whenever a command can decide: it runs before the fire, so a
finished loop costs no model call. Use `schedule_done { why }` from inside a
run when only the model can tell (the page changed, the plan has no `todo`
left, the task turned out impossible): it ends the loop with `done (model):
<why>` on record and removes the file. Both may be present; whichever fires
first ends it. When asked how a loop is going, call `schedule_status
{ name }`: runs so far, last result, recent reports.

## Three other worked files

Fix the tests until they pass, at most 20 tries, unattended:

```markdown
---
every: 0s
until: cargo test -p agent
max_runs: 20
approvals: false
cwd: ~/Develop/wth
quiet: true
---
`cargo test -p agent` is failing (the `<until>` block has the tail). Fix one
failing test at a time: read the failure, change the code (not the test unless
the test is wrong), rerun that test. Reply with what you changed.
```

Watch a URL until it changes; the model decides when:

```markdown
---
every: 10m
quiet: true
---
Fetch https://example.com/status and compare it with the `status-page` note
(`knowledge_search status-page`). If it changed, `message_user` the difference,
store the new text with `knowledge_put`, and call `schedule_done` with why.
Else reply `nothing to report`.
```

A weekday brief until the incident closes; the check is a script:

```markdown
---
cron: "0 7 * * 1-5"
until: test "$(gh issue view 42 --json state -q .state)" = CLOSED
cwd: ~/Develop/wth
---
Summarise the last 24 h of activity on issue #42 (`gh issue view 42 --comments`)
in five lines: what changed, who is blocked, what happens today.
```

## Anti-patterns

- `every: 0s` without `until` or `max_runs` — the parser refuses it; a loop
  needs a stop.
- `approvals: false` on a loop that only reads or reports — it gains nothing
  and grants everything.
- Asking the user a question from inside a run — nobody answers; the turn
  ends. Decide from the prompt, the plan file and `<until>`, or `message_user`
  and end.
- Keeping the plan in the prompt body or in knowledge — the body is the same
  every iteration; the plan changes every iteration. It lives in `cwd`.
- Spawning a child and ending the turn without verifying — the child's
  result is a claim; the suite is the proof.
