---
name: routines
description: Design or edit a routine — a prompt that runs on a clock, when a sensor says so, or again until done — and propose it as a draft card.
---

# Routines: one sentence, four parts, one card

A routine is a markdown file in `~/.theuth/schedules/<name>.md`: front matter says *when*, the body
is the prompt. You never write the file yourself in this skill — you propose it with
`routine_draft`, the user presses a button on the card, and then you call `schedule`. Say the
routine in **one sentence first** (§5), always, before any key; the user should be able to
read what you built without knowing a single key name.

## 1. The mental model — Trigger × Stop × Run × Concurrency

Every key belongs to exactly one part. The UI, this skill and the sentence use the same four words.

| part | question it answers | keys |
|---|---|---|
| **Trigger** | what wakes it | `cron` / `every` / `at`; with `every`: `when` (sensor + args) and the gate keys `fire`, `for`, `cooldown`, `timeout`; `missed` (cron only) |
| **Stop** | when it is over | `until`, `max_runs`, `expires` (watchers); an `at:` file stops by firing |
| **Run** | what the turn is like | prompt, `model`, `cwd`, `approvals`, `quiet`, `run_timeout`, `agent` + `project` (a task instead of a session), `memory` (watchers: keep the session between fires), `runtime` (keep the python globals between runs) |
| **Concurrency** | what happens if it is due while running | watchers: `mode` (`skip` / `queue` / `restart`); clock and loop files: deferred to the first tick after the run ends, forced runs `skipped: busy` |

Changing one part never requires touching another. A watcher with `until` and `quiet` is a
trigger *and* a stop *and* a run choice — say it as composition, not as a "type".

## 2. The keys per part

| part | key | values · default · floors | rule |
|---|---|---|---|
| Trigger | `cron` | 5-field cron, local time (`"0 9 * * 1-5"`) | exactly one of `cron` / `every` / `at` |
| Trigger | `every` | duration (`30m`, `6h`); floor `10s`; `0s` = again as soon as a run ends | `0s` only with `until` or `max_runs`; floor `30s` with `when` |
| Trigger | `at` | one instant, local (`2026-10-07T15:00`) | runs once, then the file is gone; no `until` / `max_runs` / `when` |
| Trigger | `missed` | `run` (default: skip) | cron only; a missed occurrence fires at boot |
| Trigger | `when` | `<file>.py key=value …` (`"quoted with spaces"` allowed; keys `[a-z][a-z0-9_]*`; never `cwd`) | requires `every`; exclusive with `cron` / `at`; needs a stop: `until`, `max_runs` or `expires` |
| Trigger | `fire` | `on_change` (default) / `while_true` / `once` | `on_change`: fire when true *and* the payload changed; `while_true`: every true check; `once`: the first true check only |
| Trigger | `for` | duration, default `0s` | consecutive true checks must span this long before the first fire |
| Trigger | `cooldown` | duration, default `0s` | minimum spacing between fires |
| Trigger | `timeout` | duration, default `20s`; `≤ 60s`, `< every / 2` | the sensor's wall clock per check |
| Stop | `until` | shell command, runs in `cwd` before each fire | exit 0 = done (file removed), 1 = run again (tail arrives in `<until>`), else broken |
| Stop | `max_runs` | integer ≥ 1 | stop after N runs (`max_runs N reached`) |
| Stop | `expires` | duration ≥ `every` (`14d`) | watchers; counted from the file's `created:`, which `schedule` writes |
| Run | prompt | the body | written for a run with nobody present; never empty |
| Run | `quiet` | `true` / `false` (default) | the reply is not delivered; only `message_user` reaches the user |
| Run | `approvals` | `true` (default) / `false` | `false`: the run never asks permission — only when nobody will answer, and say so; a watcher whose prompt pushes, merges, commits or writes needs it |
| Run | `model` | model id | default: the engine's |
| Run | `effort` | a reasoning level `model` takes (`low`, `high`, …) | needs `model`; default: the engine's |
| Run | `cwd` | directory, `~` allowed | must exist; where `until` and the sensor run and the session starts |
| Run | `run_timeout` | duration ≥ `30s` (`45m`) | default: Settings › Agent loop › Routines |
| Run | `agent` + `project` | roster agent name; project label | each fire creates a task owned by the agent instead of running in the routine's session |
| Run | `memory` | `continue` (default) / `clear` | watchers: keep the session between fires or start fresh |
| Run | `runtime` | `clear` (default) / `keep` | reset the python globals before each run, or keep them; a watcher with `memory: continue` keeps them either way |
| Concurrency | `mode` | `skip` (default) / `queue` / `restart` | watchers: a fire while a run is going is `skipped: busy` / queued into one pending payload / cancels the run and starts over |

**Recommendations.** Clock (`cron` / `every`) when the *time* is the trigger. Watcher (`every` +
`when`) when the *world* is the trigger and a sensor for it exists (`list_sensors`). Loop
(`every: 0s` + `until` / `max_runs`) when the *stop* is the point — "keep at this until…". Agent-routed
(`agent`, optionally `project`) when a roster agent should own each run as a task.

## 3. What to ask, what not to ask

Ask **at most two questions**, and only when the answer changes the file:

- the **stop** for a watcher — a run cap (`max_runs`) or an expiry (`expires`), when the user gave neither;
- the **directory** for a loop or a `cwd`-dependent sensor (`fs_exists.py`), when none is implied;
- **which agent**, when the user named a team but not a person.

Never ask about `mode`, `memory`, `runtime`, `for`, `cooldown`, `timeout`, `model`, `run_timeout`, `missed`:
pick the default (or the obvious value) and let the sentence say it; the user changes it in the
form. Never ask for a secret *value* — secrets are vault names (`token=gh_token`); if the vault
lacks one, `secret_request` it. Never ask the user to write or edit a file.

## 4. Sensors

Call `list_sensors()` before proposing a watcher. Each row is a `SensorInfo`: `name`,
`description`, `usage` (the `when:` line), `args` (name · required/optional · example · help, in
header order), `needs_cpython` + `cpython_available`, `error`. Offer only sensors with no `error`;
a `needs_cpython` sensor is offered only when `cpython_available` is true. Fill every `required`
arg; an `example` of `secret` means a vault name.

Built-ins: `pr_watch.py` (a GitHub PR: `repo`, `pr`, `token`), `fs_exists.py` (a glob under `cwd`),
`rss.py` (new entries in feeds: `urls`, optional `filter` regex and `backfill`).

**YouTube rule.** A channel URL or `@handle` is resolved *in the conversation*, never by the sensor:
`read_url` the channel page (or `search_web` the handle), find `channel_id=UC…` (or the
`playlist_id`), and pin `urls=https://www.youtube.com/feeds/videos.xml?channel_id=UC…` in `when:`.
Tell the user which channel you resolved to.

## 5. The sentence grammar

Write the card's `sentence` by these rules — the UI renders its own from the fields, so a
mismatch is visible. Copied from design 108 §2.2:

### 2.2 The sentence grammar

One function, `sentence(r: Readable): string`, in `components/routines/helpers.ts`, where
`Readable` is the subset both `ScheduleInfo` and `ScheduleFile` can supply (the UI worker writes
`fromInfo(info)` and `fromFile(file)` adapters; `when_sensor` ↔ `when`, `expires` as written,
`until`, `max_runs`, `agent`, `quiet`, `approvals`, `model`, `cwd`, `run_timeout`, `mode`,
`memory`, `fire`, `for`, `cooldown`, `missed_run`). The model writes the same sentence by hand
from the rules below (they are reproduced in the skill, §7).

**Shape.** `TRIGGER [· GATE] [· STOP] [· RUN]` — clauses joined by ` · `; a clause is omitted
when it has nothing non-default to say; nothing else is ever added. Lower case except names and
sensor files; numbers as written in the file; durations as written (`5m`, `14d`), not expanded.

**TRIGGER** (extends `cadence()`, `helpers.ts:64-71`; `cronWords` `helpers.ts:35-51` unchanged):

| source | clause |
|---|---|
| `cron` with words | `cronWords` output capitalised: `Every day at 09:00`, `Weekdays at 09:00`, `Every Friday at 18:00`, `Every 15 min`, `Hourly at :05`, `Monthly on day 1 at 09:00` (the existing strings gain the word *at* before a clock time; `helpers.test.ts` is updated, not appended-to) |
| `cron` without words | `On cron <expr>` — the raw expression; the engine's `describe` goes in the title attribute, not the sentence |
| `cron` + `missed: run` | append `, catching up a missed run` |
| `every: <d>` without `when` | `Every <d>` — `Every 30m`, `Every 6h` |
| `every: 0s` (only legal with a stop) | `Again as soon as a run ends` |
| `every: <d>` + `when: <file> k=v…` | `Every <d> checks <file> on <subject>` where *subject* = the args in file order as `k=v` joined by spaces, secrets shown by vault name (`token=gh_token` is already a name, 40 §1.4); if the sensor's `# arg:` header is known (§9) the required args only: `pr_watch.py on repo=skymoore/wth pr=482` |
| `at: <t>` | `Once at <d Mon HH:MM>` (the existing `onceAt`, `helpers.ts:54-58`) |

**GATE** (watchers only). The fire rule is always said — it is the one thing a reader must know
about a watcher; every other default is silent (`for: 0s`, `cooldown: 0s`, `mode: skip`,
`memory: continue`, `timeout: 20s`). Parts in this order, comma-separated inside one clause:
`fires on change` / `fires while true` / `fires once` · `after <for> true` · `<cooldown>
cooldown` · `queues while running` / `restarts the run` · `fresh context each fire`
(`memory: clear`) · `check limit <timeout>`.

**STOP**: `runs until \`<until>\` passes` · `at most <max_runs> times` (both: `runs until
\`cmd\` passes, at most 10 times`) · `expires after <expires>` (`, ` between parts). No stop → no
clause (a one-shot's `Once at` already says it).

**RUN** (defaults silent — `quiet: false`, `approvals: true`, engine model, no cwd, default
`run_timeout`, `runtime: clear`): `quietly` · `as agent <name>` (+ ` in project <p>`) · `no approvals` · `with
<model>` · `in <cwd>` (as written, `~` kept) · `<run_timeout> per run`. Then, after a final ` · `,
the **prompt gist**: the first line of the body, trimmed to 60 chars with `…` — only in the detail
page's sentence, never in the list row (the row has the name).

**Examples** (the test fixtures; the UI worker's `helpers.test.ts` asserts each string exactly):

| file (front matter) | sentence |
|---|---|
| `cron: "0 9 * * 1-5"` | `Weekdays at 09:00` |
| `cron: "0 9 * * 1-5"`, `until: make test`, `max_runs: 10`, `quiet: true`, `agent: nikolai`, `approvals: false` | ``Weekdays at 09:00 · runs until `make test` passes, at most 10 times · quietly, as agent nikolai, no approvals`` |
| `every: 60s`, `when: pr_watch.py repo=skymoore/wth pr=482 token=gh_token`, `cooldown: 5m`, `max_runs: 40`, `expires: 14d`, `cwd: ~/Develop/wth`, `quiet: true`, `approvals: false` | `Every 60s checks pr_watch.py on repo=skymoore/wth pr=482 · fires on change, 5m cooldown · at most 40 times, expires after 14d · quietly, no approvals, in ~/Develop/wth` |
| `every: 10m`, `when: rss.py urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCabc`, `expires: 90d` | `Every 10m checks rss.py on urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCabc · fires on change · expires after 90d` |
| `every: 30s`, `when: fs_exists.py path=drop/*.txt`, `fire: while_true`, `for: 2m`, `cooldown: 1h`, `mode: queue`, `memory: clear`, `timeout: 10s`, `until: test ! -d drop` | ``Every 30s checks fs_exists.py on path=drop/*.txt · fires while true, after 2m true, 1h cooldown, queues while running, fresh context each fire, check limit 10s · runs until `test ! -d drop` passes`` |
| `every: 60s`, `when: pr_watch.py repo=o/r pr=7 token=gh_token`, `fire: once`, `mode: restart`, `max_runs: 1` | `Every 60s checks pr_watch.py on repo=o/r pr=7 · fires once, restarts the run · at most 1 times` |
| `every: 0s`, `until: ./scripts/missing-tests.sh`, `max_runs: 200`, `cwd: ~/port`, `approvals: false`, `run_timeout: 45m` | ``Again as soon as a run ends · runs until `./scripts/missing-tests.sh` passes, at most 200 times · no approvals, in ~/port, 45m per run`` |
| `every: 6h`, `model: claude-haiku-4-5` | `Every 6h · with claude-haiku-4-5` |
| `at: 2026-10-07T15:00` | `Once at 7 Oct 15:00` |
| `at: 2026-10-07T15:00`, `quiet: true`, `cwd: ~/src/app` | `Once at 7 Oct 15:00 · quietly, in ~/src/app` |
| `cron: "0 7 * * *"`, `missed: run`, `agent: alice`, `project: web` | `Every day at 07:00, catching up a missed run · as agent alice in project web` |
| `cron: "15 */2 * * 1,3"` | `On cron 15 */2 * * 1,3` |
| `every: 1h`, `max_runs: 3` | `Every 1h · at most 3 times` |
| `every: 60s`, `when: pr_watch.py repo=o/r pr=9 token=t`, `expires: 7d`, `agent: ops` | `Every 60s checks pr_watch.py on repo=o/r pr=9 · fires on change · expires after 7d · as agent ops` |

Rules the tests pin: (a) `at most 1 times` is accepted — no pluralisation logic, the number is
the file's; (b) a broken file (`ScheduleInfo.error`) has no sentence: the row shows the error in
red as today (`RoutinesView.tsx:140`); (c) the sentence never contains the prompt in the list;
(d) `fromFile` of a half-filled form yields a partial sentence (`Every 30m` while the rest is
default) — that *is* the live preview (§5.4).

## 6. The draft card — `routine_draft`

Always draft; **never call `schedule` before a press** unless the user typed the file themselves.
One routine per card (one element in `routines`, normally). The args are the `schedule` keys plus
`sentence`:

```json
{"routines": [{
  "name": "argo-482", "sentence": "Every 60s checks pr_watch.py on repo=skymoore/wth pr=482 · fires on change, 5m cooldown · at most 40 times, expires after 14d · quietly, no approvals, in ~/Develop/wth",
  "every": "60s", "when": "pr_watch.py repo=skymoore/wth pr=482 token=gh_token",
  "cooldown": "5m", "max_runs": 40, "expires": "14d",
  "cwd": "~/Develop/wth", "quiet": true, "approvals": false,
  "prompt": "You babysit PR #482 …"
}]}
```

Keys: `name`, `sentence`, `cron` | `every` | `at`, `prompt`, `quiet`, `model`, `effort`, `until`, `max_runs`,
`run_timeout`, `cwd`, `approvals`, `agent`, `project`, `when`, `fire`, `for`, `cooldown`, `mode`,
`memory`, `expires`, `timeout`. The tool validates the draft as `schedule` would (name rule, the
rendered file parses, the sensor resolves) and refuses with the engine's message — fix and call
again. It does **not** dry-run the sensor; that happens after the press.

The card shows the sentence, `Trigger · Stop · Run · Concurrency`, the prompt, and three buttons:
**Create paused**, **Create and start**, **Open in form**. There is no Discard: a draft nobody presses
is nothing. After the call, **stop and wait**. The press arrives as the user's message

```
[app routine-drafts v<n> · <record_call_id>]
routine draft <id>: create paused | create and start | open in form
```

That line is the user's approval. What to do next:

| press | call | then say |
|---|---|---|
| `create paused` | `schedule({…the draft's fields, "enabled": false})` | the tool's line (next fire · `disabled: resume with …`); for a watcher, the verdict in one line |
| `create and start` | `schedule({…the draft's fields, "enabled": true})` | the same; a watcher with a clean verdict is written live; an error refuses the write — relay it and offer a fix |
| `open in form` | nothing | one line: "It's in the form on the left — edit and press Create." |

## 7. Validation

For a watcher, `schedule` resolves the sensor, dry-runs `check` twice, runs `probe`, and refuses on
error — nothing is written. Read `verdict` in the structured reply and tell the user in one line:
`would fire now` / `would not fire now`, `payload flaps` (the sensor returns unstable fields),
`needs approvals: false` (the body mutates). After creation, `schedule_status(name)` shows the
verdict again. If the user has the form open they can press *Test now* before creating.

## 8. Editing an existing routine

When asked to edit (the first message says `Edit the existing routine …` with the current file, or
the user names one): propose a card with the **same name** — the card says it replaces
`schedules/<name>.md`. *Create paused* / *Create and start* overwrite through `schedule`; the
watcher's state and session survive a replace under the same name. Keep every key the user did not
ask to change.

## 9. Examples

**Weekday brief.** User: "every weekday morning, brief me on my calendar and unread mail."
No question needed. Card: `brief` · `Weekdays at 09:00` · `cron: "0 9 * * 1-5"` · prompt: the brief.
Press `create and start` → `schedule({"name":"brief","cron":"0 9 * * 1-5","prompt":"…","enabled":true})`
→ "Scheduled — next Mon 09:00."

**PR babysitter.** User: "babysit PR 482 on skymoore/wth." `list_sensors()` → `pr_watch.py` needs
`repo`, `pr`, `token` (a vault name). One question: "Stop after how long? I'd say 14 days or 40
runs." Card: `wth-482` · `Every 60s checks pr_watch.py on repo=skymoore/wth pr=482 · fires on
change, 5m cooldown · at most 40 times, expires after 14d · quietly, no approvals, in ~/Develop/wth`
(`approvals: false` because the prompt pushes and merges — say so). Press `create paused` →
`schedule({…, "enabled": false})` → relay `sensor pr_watch.py · would NOT fire now · probe ok` and
"resume from the Routines page when ready."

**YouTube watch.** User: "tell me when @veritasium posts." `read_url("https://www.youtube.com/@veritasium")`
→ `channel_id=UCHnyfMqiRRG1u-2MsSQLbXA`. Card: `veritasium` · `Every 10m checks rss.py on
urls=https://www.youtube.com/feeds/videos.xml?channel_id=UCHnyfMqiRRG1u-2MsSQLbXA · fires on
change · expires after 90d` · prompt: "A new video is in `<when>`; message_user the title and
link." No question: `expires: 90d` is the stop, said in the sentence.

**Port loop.** User: "keep porting the tests to Rust until `./scripts/missing-tests.sh` passes, in
~/port." Card: `port` · ``Again as soon as a run ends · runs until `./scripts/missing-tests.sh`
passes, at most 200 times · no approvals, in ~/port, 45m per run`` · `every: 0s`, `until`,
`max_runs: 200`, `cwd`, `approvals: false`, `run_timeout: 45m` · prompt: one test per run, the
supervisor pattern from `skills/loops.md`. Say why approvals are off.

**Agent-routed nightly task.** User: "every night have alice tidy the web project's backlog."
One question only if the roster has no `alice`. Card: `backlog-tidy` · `Every day at 07:00 · as
agent alice in project web` · `cron: "0 7 * * *"`, `agent: alice`, `project: web`. Each fire
creates a task owned by alice instead of a session. (`missed: run` is form-only — the card cannot
set it; say so if the user wants catch-up.)
