# 108 — Routines you can understand: a chat-first, cognitive-load-first Routines surface

Status: design v0 · 2026-10-06 · extends `34-schedule.md` (file, tick, runs), `33-loops.md`
(stops), `40-triggers.md` (watchers, sensors, verdict), `70-as-built-ROUTINES.md` (the surface as
it stands), `97b` (`SectionBand` / `SectionRow`), `101-decision-review-surface.md` (master–detail
with an embedded pane), `71-whiteboard-visuals.md` (the lens). Every `file:line` was read on
`~/Develop/theuth` main (f39343bc, 2026-10-06 11:10). Where this doc disagrees with 34/33/40
about a *surface* or the `schedule` tool's `enabled` rule for watchers (§1 D12), this doc wins;
the file grammar is unchanged. Workers write their as-built notes as
`docs/design/108-as-built-<UI|ENG|RSS>.md`.

Sky's words (2026-10-06 11:08): *"make the routines UI significantly better, perhaps with a chat
interface for making a routine that has the skill for it pre-loaded. It needs to reduce cognitive
load and make it easy for the user to understand and set up routines with any combination of the
features."* The lens is 71's: *"how can we … produce something that truly reduces the cognitive
load"* (`71-whiteboard-visuals.md:5`).

## 0. Why

The file grammar is rich and the surface shows keys, not meaning. Verified on main:

- **A watcher looks like an interval routine.** The list row prints `cadence(s)` for every
  file (`RoutinesView.tsx:140`); `cadence` reads only the clock clause (`helpers.ts:64-71`), so
  `every: 60s` + `when: pr_watch.py …` renders as `every 60s`. The only tells are the
  `check hh:mm:ss · outcome · 1.2 s` fragment (`RoutinesView.tsx:151-155`, `lastCheckText`
  `helpers.ts:85-88`) and a Test button gated on `when_sensor` (`RoutinesView.tsx:162-166`).
  Nothing names the sensor or says "watcher".
- **The form cannot make or edit a watcher.** `NEW` carries `when / fire / for / cooldown / mode
  / memory / expires / timeout` as nulls with the comment "the form does not edit them yet"
  (`RoutineDetail.tsx:55-63`); no field renders them. The only creation path is the `schedule`
  tool from chat (`tools.rs:80-126`), which the user cannot see from Routines.
- **History does not know a check from a run.** Gate outcomes are run rows with `session: None`
  (`lib.rs:1549-1558`) and their `result` is the `CheckOutcome` string (`lib.rs:204-222`:
  `quiet`, `unchanged`, `holding: 2m of 10m`, `cooldown: 3m left`, `deferred`, `check broken: …`).
  The UI's `resultKind` maps everything that is not `end_turn` / `done…` / `max_runs…` /
  `skipped…` to **failed** (`helpers.ts:93-98`), so a healthy watcher's `quiet` row — and its
  `last_result` chip, since the gate row sets `last_result` (`lib.rs:1555-1557`) — shows red
  `failed`. The History table has one `result` column for all of them (`RoutineDetail.tsx:389-433`).
- **Composition is invisible.** The form is one flat grid: cadence fieldset, three switches,
  Agent/Project/Model/Directory/Time limit, a Loop fieldset (`RoutineDetail.tsx:271-360`). A user
  cannot tell that *trigger × stop × run × concurrency* are four independent choices, and
  `agent:` is only settable here — the `schedule` tool hard-codes `agent: None, project: None`
  (`tools.rs:182-183`).
- **Watchers are always written paused by the tool** (`tools.rs:179-180`,
  40 §1.12) and the model has no `enable` tool: "create and start" from chat means an `edit` on the
  `enabled:` line (33 §0.6).
- **Sensors have no self-description.** The header grammar is `# theuth-sensor:` / `# needs:` /
  `# hosts:` (`sensor/mod.rs:242-277`); arguments live only in the docstring
  (`pr_watch.py:5`, `fs_exists.py:4`). No RPC lists sensors; the form could not offer them.

Everything after the fire already works (one session per routine, history, `quiet`, stops,
`Deliver`, the verdict). What is missing is the *reading* of a routine and a creation path that
does not require knowing the keys.

## 1. Decisions in one screen

Sky ruled 1–3; the rest are this doc's, each with the alternative it beat.

1. **Chat-first entry (Sky).** "New routine" → **Describe it**: a session with the `routines`
   skill loaded; the model asks only what it must and proposes a draft card (like hiring) with
   *Create paused* / *Create and start* / *Open in form*. The guided form stays as the editor and
   the expert path. Rejected: form-first with a "help me" link — the form is where the keys leak.
2. **The chat is embedded on the Routines page (Sky).** The session renders in a pane beside the
   list/form (`LiveSession` with `first={false}`, the meetings precedent
   `ControlCenter.tsx:200`); the draft fills the form live; on phones it is a full-screen sheet.
   Rejected: navigating to `/c/<session>` — the user loses the list and the form.
3. **Three parallel omp workers (Sky):** UI, ENG, RSS (§11). Each owns files; the contract is
   the TS types in §9/§8 that the UI mocks until `just ts-desktop` regenerates bindings.
4. **One mental model: Trigger × Stop × Run × Concurrency.** Every key belongs to exactly one
   part (§2.1); the UI, the skill and the sentence grammar use the same four words. Rejected: a
   kind-per-file taxonomy (clock / loop / watcher as disjoint types) — a watcher with `until` and
   `quiet` is all three; composition must read as composition.
5. **One deterministic sentence per routine** (§2.2), rendered in TS from `ScheduleInfo` /
   `ScheduleFile` and written by the model from the same rules in its card. `cadence()` is kept
   and becomes the Trigger clause of the sentence. Rejected: a Rust implementation in the engine
   (a third copy to keep in step; `describe` already exists for cron and stays the fallback).
6. **Row badge = what wakes it: `Clock` · `Once` · `Watcher` · `Loop`.** `Loop` is the badge
   only for `every: 0s` (back-to-back: the trigger *is* the stop, 33 §0.3); any other routine with
   a stop carries a stop chip (`until` · `3/10` · `expires 14d`). Rejected: "Loop" for every file
   with `until` — a weekday-09:00 routine with `max_runs: 10` is a clock routine that stops.
7. **History rows carry a `kind`** (`run | check | end | skip`, engine-side, §9.3) and check rows
   are collapsed into runs of "n quiet checks" with the distinct outcomes spelled out. Rejected:
   string-sniffing `CheckOutcome` text in TS — the `Display` strings (`lib.rs:204-222`) are not a
   contract.
8. **`routine_draft` mirrors `agent_draft` exactly** (`agent_draft.rs`): an OpenUI card set under
   app name `routine-drafts`, `@ToAssistant("routine draft <id>: <choice>")` buttons, the press
   arrives as `[app routine-drafts …]`, then the model calls `schedule` (§8). Rejected: a draft
   store / RPC — drafts are never written, like agent drafts (`agent_draft.rs:9`).
9. **"Open in form" is a UI event, not a navigation the card performs.** `@OpenUrl` accepts only
   http(s) (`AppHost.tsx:421-428`, `check.ts:356-363`), so the press is a `@ToAssistant` like the
   others; `AppHost` dispatches a `theuth:app-press` window event before it sends, the Routines
   pane (or a root-level bridge when the card is in an ordinary chat) reads the draft from the
   `routine_draft` tool call's `args` already in the session store (`sessions.ts:102-106`) and
   fills the form. The model answers one line and does nothing. Rejected: a `/routines/new?draft=`
   fetch of a stored draft (nothing is stored); the model navigating (it cannot).
10. **`list_sensors` RPC + a hand-writable `# arg:` header line per argument** (§9.1–9.2):
    `# arg: <name> <required|optional> <example> — <help>`. Rejected: one `# args:` line (a
    help text per arg does not fit one line); JSON in a comment (not hand-writable).
11. **The `routines` skill ships from `crates/schedule/routines.md`** like `loops`
    (`tools.rs:36-38`, `main.rs:1168-1181`): binary-owned, rewritten whenever the text differs.
    Rejected: a skill under `skills/` repos (it must match the running engine's keys).
12. **The `schedule` tool honours an explicit `enabled: true` for a watcher when the verdict is
    clean** (amends 40 §1.12 and `tools.rs:180`). The dry-run, probe and refusal on error are
    unchanged; the default without `enabled` stays `false`. Reason: *Create and start* on the card
    is the user's grant after reading the sentence, and the tool still refuses to write a broken
    watcher. Rejected: model `edit`s the `enabled:` line after creation (a second mutating step the
    user did not press for).
13. **`ScheduleArgs` gains `agent` and `project`** so a chat draft can be agent-routed
    (`Spec.agent`, `file.rs:309-315`; the fire creates a task, `lib.rs:2168`). Rejected: leaving
    agent-routing form-only.
14. **"Edit by chat" on the detail page uses the same pane**; the first send carries the file's
    current text as context (§6.6). Cheap because the pane is one component; rejected only if the
    UI worker runs out of time — it is the last item in §11's order.
15. **Validation messages are the engine's, re-worded in TS by exact match** (§5.5 table over the
    `file::parse` error list, `file.rs:368-378`); unknown strings pass through verbatim. Rejected:
    duplicating the rules client-side.
16. **Concurrency is shown, not edited, for clock routines.** 34 as built *defers* a fire that
    comes due mid-run to the first tick after the run ends (`34-schedule.md:620-628`); only a
    forced Run now while running is `skipped: busy` (`lib.rs:913`). Watchers edit `mode`
    (`skip | queue | restart`, 40 §1.9). Rejected: inventing a `mode` for clock files.
17. **RSS sensor `rss.py`** as specified in §10 — regex XML, bounded seen-FIFO, conditional GET,
    seed-on-first-check. Rejected: resolving YouTube `@handle`s in the sensor (the channel page can
    exceed the 1 MiB fetch cap, 40 §5); the skill resolves them in conversation.


## 2. The mental model and the plain-words grammar

### 2.1 Four parts, every key in one of them

| part | question it answers | keys (`file.rs:284-337`) |
|---|---|---|
| **Trigger** | what wakes it | `cron` / `every` / `at`; with `every`: `when` (sensor + args) and the gate keys `fire`, `for`, `cooldown`, `timeout`; `missed` (cron only) |
| **Stop** | when it is over | `until`, `max_runs`, `expires` (watchers); an `at:` file stops by firing |
| **Run** | what the turn is like | prompt, `model`, `cwd`, `approvals`, `quiet`, `run_timeout`, `agent` + `project` (a task instead of a session), `memory` (watchers: keep the session between fires) |
| **Concurrency** | what happens if it is due while running | watchers: `mode` (`skip` / `queue` / `restart`, 40 §1.9); clock and loop files: deferred to the first tick after the run ends (`34-schedule.md:620-628`), forced runs `skipped: busy` (`lib.rs:913`) |

Reading rule for the UI: one sentence on top (§2.2); four sections underneath in that order; each
field says which part it belongs to and nothing else. Changing one part never requires reading
another.

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
`run_timeout`): `quietly` · `as agent <name>` (+ ` in project <p>`) · `no approvals` · `with
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


## 3. Routines list (`/routines`)

Keeps the 97b chrome (`SectionBand group="agents"` + `SectionRow`, `RoutinesView.tsx:30-37`),
the order (`sortRoutines`, `helpers.ts:101-105`), the query (`queries.ts:17-33`) and the
`max-w-[880px]` column. The row is rebuilt:

| zone | content | source |
|---|---|---|
| line 1 | **badge** `Clock` / `Once` / `Watcher` / `Loop` (D6; `kindOf(info)` in helpers: `when_sensor` → Watcher; `when` starts `at` → Once; `when === 'every 0s'` → Loop; else Clock) · **name** · state word `broken` / `paused` (as today) · stop chip when a stop exists and the badge is not Loop: `until` (title = the command) · `runs 3/10` (`runCount`, `helpers.ts:23-26`) · `expires 14d` | `ScheduleInfo` |
| line 2 | the **sentence** (§2.2, no prompt gist), truncated with `…` at one line; title attribute = the raw clauses (`when` + `when_sensor`) | `sentence(fromInfo(s))` |
| line 3 | **health**, left to right: `next in 2d` · `last 3m ago · ok` (`relative`, `resultKind`) · for a watcher `last check 10:15:03 · quiet · 1.2 s` (`lastCheckText`) · chips `running` / `not hub` / `@agent` / `project` / `approvals off` (`RoutineChips`, `RoutinesView.tsx:70-106`, unchanged) | as today |
| right | **Run now** · **Test** (watchers; verdict in a toast as today, `RoutinesView.tsx:118-121`) · pause Switch | unchanged |

`resultKind` (`helpers.ts:93-98`) learns the check outcomes: `quiet` / `unchanged` / `holding…`
/ `cooldown…` / `deferred` / `queued…` → a fourth kind `check` (grey, label = the word before
the colon); `check broken…` stays `failed`. Until ENG's `RunInfo.kind` lands (§9.3) this is
string-based for `last_result` only — `last_result` is a bare `CheckOutcome` string
(`lib.rs:1556`) and has no `kind`; that is acceptable for one chip and is pinned by a test.

**Header controls.** `SectionRow` right side: **Describe it** (primary, `MessageSquarePlus`) and
**New routine** (secondary — the form). Both go to `/routines/new`; Describe it adds
`?chat=1` so the page opens with the pane (§6).

**Empty state** (`RoutinesView.tsx:47-51` replaced): one sentence — "A routine is a prompt that
runs on a clock, when a sensor says so, or again until done." — then two buttons, **Describe
it** and **Use the form**, and the existing file-model footnote (`RoutinesView.tsx:59-62`).

**Mobile.** Below `md` the row stacks: line 1, line 2, line 3 wrap; the three controls move to
a trailing row (`flex-wrap`); Test is hidden behind the detail page (the row keeps Run now and
the Switch). Badge text stays (it is the cheapest explanation on a phone).

## 4. Detail page (`/routines/$name/$section`)

Sections stay `overview · prompt · history · danger` (`SECTIONS`, `helpers.ts:107`; routes
`routines.$name.$section.tsx:5-10`). The header row gains **Edit by chat** (§6.6) beside Run now
/ Test / Switch (`RoutineDetail.tsx:102-122`).

**Overview** = sentence on top, then four fact sections, then the form (§5):

```
<sentence with prompt gist>                                   ← h3, wraps, title = raw clauses
Trigger      Every 60s · checks pr_watch.py · next in 40s · last check 10:15:03 quiet 1.2 s
Stop         at most 40 times (3 so far) · expires 14d (12d 4h left)
Run          session: open · quietly · no approvals · model: engine default · in ~/Develop/wth · 30m per run
Concurrency  mode skip — a check that fires while a run is going is a `skipped: busy` row
```

`Facts` (`RoutineDetail.tsx:164-206`) becomes these four labelled rows (`Fact` cells kept, one
row per part on `sm:` and up, stacked below). The Concurrency row's text for clock files:
"a fire due mid-run waits for the first tick after the run ends" (`34-schedule.md:620-623`);
for watchers it names the mode and one clause from 40 §1.9. `expires … left` is computed from
`created` (`ScheduleFile.created`) + `expires` when both are present, else just the duration.

**History** (`RoutineDetail.tsx:383-435`) — rows carry `RunInfo.kind` (§9.3):

| kind | row | notes |
|---|---|---|
| `run` | `fired` time · `took` · result chip (`ok`/`failed`/`skipped`) · report first line (unfold) · `session` link | as today; a watcher's fired row adds the **envelope** line under the time: `subject=pr/482 stage=waiting_ci events=ci_failed,new_commits` rendered as chips `subject` `stage` `events` (`parse_envelope` shape, 40 §1.5), the rest of the first line as mono text |
| `check` | collapsed: consecutive check rows fold into one line `12 checks · quiet ×11, unchanged ×1 · 10:03–10:15` with a chevron; unfolded rows show time · outcome · (hash prefix in title) | grey; never a red chip |
| `end` | `done: until passed after 4 runs` / `max_runs 20 reached` / `expired` / `removed` / `check broken ×3, disabled` | a full-width muted line with the reason (33 §0.4, 40 §5) |
| `skip` | `skipped: busy` / `skipped: check still running` | amber `skipped` chip as today |

The payload itself is in the run's session (`<when>` block, `40-triggers.md:280-291`); the
history row links there. **Test verdict inline:** on a watcher's Overview the Test button writes
the `Verdict` into a card under the Trigger row (`verdictText`, `helpers.ts:74-82`, plus the
three fields spelled out: *would fire now* / *would not* · *payload stable* / *flaps* · *probe:
…*); the toast stays on the list.


## 5. The guided form (`/routines/new` and Overview/Prompt)

One form, one `ScheduleFile` state, saved whole with `put_schedule` (`RoutineDetail.tsx:246-254`,
kept). The layout becomes four collapsible sections in the §2.1 order, each a `fieldset` with
the part's name as legend and a one-line "what this is" hint; **progressive disclosure**: a
section opens with only its common fields, an *Advanced* disclosure (`components/ui/collapsible`)
holds the rest. Nothing is removed from what the form can set today.

### 5.1 Trigger

A segmented picker (`NativeSelect` today, `RoutineDetail.tsx:273`; keep it) with four options,
each a sentence fragment, not a key: **On a clock** (`cron`) · **Every so often** (`every`) ·
**Once** (`at`) · **When a sensor says so** (`every` + `when`).

| option | fields | advanced |
|---|---|---|
| On a clock | cron input (as today) + the live words | `missed: run` switch |
| Every so often | interval input (as today) | — |
| Once | `datetime-local` (as today) | — |
| When a sensor says so | interval (default `60s`, min `30s` — `MIN_EVERY_WHEN`, `file.rs:16`) · **Sensor** select fed by `list_sensors` (§9.2): each option shows `name · description`, disabled with its reason when `error` or `needs_cpython && !cpython_available`, with an `edited` chip; under it one input per `# arg:` (label = name, placeholder = example, `required` marked, help as the hint; a `token` arg with a `secret` example gets a datalist of `list_secrets` names) · **Fire rule** radio `on change · while true · once` with one line each (40 §1.6) | `for` · `cooldown` · `mode` radio `skip · queue · restart` (one line each, 40 §1.9) · `memory` switch "keep the session between fires" (default on) · `timeout` |

The `when:` string is assembled as `<file> k=v …` in header order (quoted when a value has
spaces; `Sensor::render`'s rule, `file.rs:67-69`). A sensor whose header has no `# arg:` lines
(a user's own) gets a single free-text **Arguments** input with the `k=v` rule as hint.

### 5.2 Stop

Common: **Stop after N runs** (`max_runs`). Advanced: **Until a command passes** (`until`, hint
"runs in the directory before each fire; exit 0 = done") · **Expires after** (`expires`; shown
non-advanced and *required* when the Trigger is a sensor — 40 §2: "with `when` at least one of
`until` / `max_runs` / `expires`"; the section's hint says so). Hidden for Once (`at:` cannot
loop, `RoutineDetail.tsx:343`).

### 5.3 Run

Common: **Prompt** (the Textarea, `RoutineDetail.tsx:363-367` — on `/routines/new` it lives
here; on the detail page the Prompt tab still shows it alone), **Quietly** switch, **Approvals**
switch (hint unchanged). Advanced: Agent + Project (`RoutineDetail.tsx:303-319`), Model Select
(`:320-335`), Directory (`:336-338`), Time limit per run (`:339-341`). The `approvals` hint gains
the verdict's rule: "a watcher whose prompt pushes, merges, commits or writes needs this off"
(`validate.rs:148-153`).

### 5.4 Concurrency

Read-only text for clock/loop files (§4). For a sensor trigger it *is* the `mode` radio, which
lives here rather than under Trigger › Advanced (the `mode` key answers the concurrency
question; the Trigger section links to it). The sentence preview and the file are the same
whichever section a key sits in.

### 5.5 Live sentence, live test, create buttons, validation

- **Live sentence**: `sentence(fromFile(f))` in an `aria-live="polite"` line pinned under the
  name field (new) / above the sections (edit), replacing `preview(f)` (`RoutineDetail.tsx:221-226`).
  It updates on every keystroke; it is the user's proof they built what they meant.
- **Live test before saving (watchers)**: a **Test now** button in the Trigger section calls a
  new RPC `test_schedule_file(file: ScheduleFile) -> Verdict` (§9.4: the §1.12 dry-run on an
  unsaved file, nothing written), rendering the verdict card of §4. Disabled until the sensor and
  its required args are set.
- **Buttons**: `/routines/new` has **Create paused** and **Create and start** (the latter sets
  `enabled: true` before `put_schedule`; for a watcher it is enabled only when the user has run
  Test now and the verdict had no error — otherwise the button is disabled with the hint "test
  the sensor first"). The detail page keeps **Save** / **Revert**.
- **Validation** — `put_schedule` rejects with `file::parse`'s messages (`file.rs:368-378`). The
  form keeps the message under the field it names and prefixes a plain-words line from this map
  (exact match on the engine string; anything else is shown verbatim):

| engine (`file.rs:368-378`) | plain words | field |
|---|---|---|
| `exactly one of cron / every / at` | Pick one trigger. | Trigger |
| `every: below 10s` / `every: below 30s with when:` | The interval is too short: 10 s for a clock, 30 s for a sensor. | interval |
| `every: 0s needs until or max_runs` | "Again as soon as a run ends" needs a stop. | Stop |
| `at: in the past` | That moment has passed. | at |
| `at: a one-shot has no loop to stop` / `at: with when: a one-shot has no cadence to poll on` | A one-off cannot loop or watch. | Trigger |
| `when: requires every` / `when: exclusive with cron` | A sensor needs an interval, not a clock. | Trigger |
| `when: needs a stop (until, max_runs or expires)` | A watcher needs an end: a run cap, an expiry, or a command. | Stop |
| `<key>: needs when:` | That setting only applies with a sensor. | the key |
| `missed: not allowed with when:` | Catch-up only applies to a clock. | missed |
| `expires: below every` | The expiry must be longer than the interval. | expires |
| `timeout: above 60s` / `timeout: must be below every / 2` | The check limit must be at most 60 s and under half the interval. | timeout |
| `cwd: not a directory: …` | That directory does not exist on this machine. | cwd |
| `run_timeout: below 30s` | At least 30 s per run. | run_timeout |
| `empty prompt` / `until: empty` / `max_runs: at least 1` | Write the prompt. / Write the command. / At least one run. | the field |
| `name: [a-z0-9][a-z0-9-]*, at most 40 chars` (`tools.rs:158`) | Lower-case letters, digits and dashes. | name |
| sensor errors (`sensor/mod.rs:105-120`: `when: no such sensor: x`, `when: needs cpython, none found`, `sensor header: …`) | That sensor is not installed / needs a Python this machine lacks / has a bad header. | Sensor |


## 6. The embedded chat pane ("Describe it")

### 6.1 Component

`components/routines/RoutineChat.tsx` exporting `RoutineChat({ session, onSession, seed, onDraft,
onOpenForm, onClose })`:

- `session: string | null` — the chat shown; `null` renders the **intro state**: a Textarea
  ("Describe the routine — when it should run, what it should do, when it should stop") with a
  **Describe it** button and three example chips that fill the box (a weekday brief, a PR
  babysitter, a YouTube channel watch).
- `onSession(id)` — called once the session exists (the page keeps it in the URL, §6.3).
- `seed?: { name: string; text: string }` — Edit by chat (§6.6): prepended to the first send.
- `onDraft(draft: RoutineDraftArgs, id: string)` — the latest `routine_draft` call's args (§6.4).
- `onOpenForm()` / `onClose()` — the pane asks the page to switch or close.

With a session it mounts exactly what the meetings page mounts: `<LiveSession session={id}
first={false} />` from `routes/c.$session.tsx:74` (header, transcript, todo card, subagent strip,
composer — `c.$session.tsx:126-142`), inside the same bordered column as
`ControlCenter.tsx:196-201`. `LiveSession` already loads the page and keeps it live
(`useSessionLoad`, `c.$session.tsx:150-185`); `Header` with `first={false}` draws no sidebar
toggle or inset (`Header.tsx:149-151,197`). Nothing is extracted from `c.$session.tsx`; the only
prop the pane needs that `LiveSession` lacks is a close control, which the pane draws itself in a
thin row above (`✕ Close chat`, plus `Open in chat →` linking to `/c/$session` for a wide
window — pop-outs stay as they are, `lib/popout.ts`).

### 6.2 The first send — exact call sequence

```
1. id  = await api.new_session()                                    // api.ts:644; a fresh root
2.       await api.rename_session(id, 'New routine')                // api.ts:669 (Edit by chat: `Routine <name>`)
3.       useSessions.getState().loadHistory(id, FRESH, 'replace')   // slash.ts:231, :267 — seed the slice before the turn
4.       onSession(id)                                              // the page writes ?chat=<id>; <LiveSession> mounts
5.       await api.use_skill(id, 'routines', text)                  // api.ts:809; rpc api.rs:750-756 → agent UseSkill
```

`use_skill(null, …)` would land on `owner_root()` (`api.rs:3702-3705`), which is why step 1
creates the session first. The engine wraps the skill body in `<skill name="routines">` and
appends `text` as the first turn (04 §2.4, `api.rs:747-749`); `TurnStarted` echoes it as the skill
card, so nothing is marked sent locally (the same reasoning as `Composer.tsx:655-663`). The session
is an ordinary durable root; it appears in the sidebar under its title.

### 6.3 Layout and URL

`/routines/new?chat=<session|1>` and `/routines/$name/$section?chat=<session|1>` (TanStack
`validateSearch`, the pattern of `c.$session.tsx:30-34`). `chat=1` = pane open in intro state;
`chat=<id>` = that session. The pane is state in the URL so reload, back and a sidebar click on
the session's row both restore it.

- **≥ `md`**: two columns inside the existing `main` — form/list column `flex: ratio` and the
  pane `flex: 1 − ratio`, default `0.5`, with `Divider dir="col"` (`Panes.tsx:71`) between them,
  exactly `ControlCenter.tsx:153-203`. The band and row stay full width above both. The list page
  never shows the pane; only `/routines/new` and the detail page do.
- **phones** (`useIsMobile`, `hooks/use-mobile.ts:21`): the pane is a full-screen
  `Sheet` (`components/ui/sheet.tsx`, side `bottom`, `h-[100dvh]`) over the form; the sheet's
  header has ✕ (close → form) and the title. *Open in form* closes the sheet. The composer's
  keyboard handling inside a Sheet is the same `Composer` the chat page uses.

### 6.4 The draft flows into the form

The page subscribes to the pane's session slice (`useSessions`, the `entries` of `sessions[id]`)
and finds the newest entry with `kind: 'tool'`, `name: 'routine_draft'`, `args` defined
(`sessions.ts:101-116`). Its `args.routines[0]` (§8.1) is mapped by `draftToFile(d): ScheduleFile`
and pushed into the form state **live** — every time the model revises the card the form follows,
the sentence line updates, and a toast-less inline note under the sentence says "from the chat
draft · <time>". The user's own edits after that are kept until the next draft arrives (the form
marks itself `dirty` and asks before overwriting: "Replace your edits with the new draft?").

### 6.5 What a button press does

`AppHost.onAction` (`AppHost.tsx:420-453`) gains one line before the send:
`window.dispatchEvent(new CustomEvent('theuth:app-press', { detail: { session, app: name, message } }))`.

| press (`@ToAssistant` text) | model (skill §7) | UI |
|---|---|---|
| `routine draft <id>: create paused` | `schedule({…draft, enabled: false})`; replies with the tool's line + the verdict for a watcher | on `ScheduleEvent::Changed` the list refetches; the page navigates to `/routines/<name>/overview?chat=<id>` once `list_schedules` has the name — the pane stays open beside the new routine's Overview |
| `routine draft <id>: create and start` | `schedule({…draft, enabled: true})` (D12 for watchers) | same navigation; the row shows `next …` |
| `routine draft <id>: open in form` | one line: "It's in the form on the left — edit and press Create." No tool call. | the Routines page already has the draft (§6.4); it focuses the form (desktop: scroll the name field into view; phone: close the sheet). If the card was pressed in an ordinary chat (`/c/<id>`), a root-level `RoutineDraftBridge` (mounted once in `routes/__root.tsx`) hears the event and navigates to `/routines/new?chat=<id>` — the page then loads the slice (already in the store) and fills the form |
| `routine draft <id>: discard` | "Discarded." | nothing |

The press reaches the model as the user's message `[app routine-drafts v<n> · <record>]\nroutine
draft <id>: <choice>` (`lifecycle.ts:351-352`); the skill tells the model that line is the user's
approval and names the next call (§8.4).

### 6.6 Edit by chat

Detail page button → `?chat=1` with `seed = { name, text: <the file as get_schedule returned
it, re-rendered as front matter + body> }`. The pane's first send becomes
`use_skill(id, 'routines', "Edit the existing routine `" + name + "`. Current file:\n\n```markdown\n" + text + "\n```\n\nChange: " + words)`,
session titled `Routine <name>`. The skill's edit rule (§7): propose a card with the **same name**
(the card says "replaces the existing file"); *Create and start* / *Create paused* overwrite via
`schedule` (an existing name is replaced, `tools.rs:81`); watcher state and session binding
survive a replace with the same name (34 §2.3: changed text → re-parse, keep `session` and `last`).

### 6.7 Closing

✕ closes the pane (URL loses `chat`); the session persists in the sidebar like any chat. Leaving
`/routines` closes it. A session that was closed can be reopened from the sidebar — in the chat
page, not the pane; the pane is for making one routine.

## 7. The `routines` skill

Source `crates/schedule/routines.md`, exported as `tools::ROUTINES_SKILL` beside `LOOPS_SKILL`
(`tools.rs:36-38`) and added to the `first_run` list (`main.rs:1168-1176`) under the
binary-owned rewrite rule (`main.rs:1177-1180`). Front matter `name: routines`, `description:
Design or edit a routine — a prompt that runs on a clock, when a sensor says so, or again until
done — and propose it as a draft card.` The `schedule` plugin's prompt block (`tools.rs:21-34`)
gains one sentence: "To design a routine with the user, load the `routines` skill." Content, in
order (≈ 250 lines; the examples are the §2.2 table):

1. **The mental model** — §2.1's table, verbatim, and the rule "one sentence first".
2. **The keys per part** with their defaults, floors and exclusions (34 §2.2, 33 §2, 40 §2 — the
   same facts, one table), and the three *recommendations*: clock when the time is the trigger;
   watcher when the world is the trigger and a sensor exists (`list_sensors`); loop when the stop
   is the point (`every: 0s` + `until`/`max_runs`); agent-routed when a roster agent should own
   each run as a task.
3. **What to ask, what not to ask.** Ask at most two questions, only when the answer changes the
   file: the stop for a watcher (cap / expiry), the directory for a loop or a `cwd`-dependent
   sensor, which agent when the user named a team. Never ask about `mode`, `memory`, `for`,
   `cooldown`, `timeout`, `model`, `run_timeout`, `missed` — pick the default and say it in the
   sentence; the user changes it in the form. Never ask for a secret value (vault names only;
   `secret_request` if a token is missing).
4. **Sensors.** Call `list_sensors()`; read each `SensorInfo.args` and `usage`; offer only
   installed sensors; a `needs_cpython` sensor without CPython is not offered. **YouTube rule:**
   a channel URL or `@handle` is resolved *in the conversation* — `read_url` the channel page (or
   `search_web`), find `channel_id=UC…` (or the `playlist_id`), and pin
   `urls=https://www.youtube.com/feeds/videos.xml?channel_id=UC…` in `when:`; the sensor never
   resolves handles (§10.6).
5. **The sentence grammar** — §2.2 in full (shape, clause tables, the examples). The model writes
   `sentence` for the card by these rules; the UI renders its own from the fields, so a mismatch is
   visible.
6. **The draft card** — `routine_draft` args (§8.1), one draft per card, the three buttons, the
   press grammar (§8.3), and the table of what to call next (§6.5 / §8.4). Always draft; never call
   `schedule` before a press unless the user typed the file themselves.
7. **Validation.** For a watcher, `schedule` dry-runs and probes at creation and refuses on error
   (`tools.rs:198-200`); read the `verdict` in the structured reply and tell the user in one line
   (`would fire now` / `payload flaps` / `needs approvals: false`). After creation,
   `schedule_status(name)` for the verdict again. If the user has the form open they can press
   Test now before creating.
8. **Editing** — the §6.6 rule: same name, say "replaces".
9. **Examples** — five complete exchanges: weekday brief · PR babysitter · YouTube watch · port
   loop (33 §2) · agent-routed nightly task.


## 8. The `routine_draft` tool and card contract (ENG)

Module `crates/app/src/routine_draft.rs`, registered beside `TaskDraftTool` / `AgentDraftTool`
(`app/src/lib.rs:349-358`), re-exported from `app/src/lib.rs:32-33`; rendered by the same
`ArtifactCard` path (`MessageTool.tsx:131` gains `|| tool.name === 'routine_draft'`, UI-owned).

### 8.1 Args

```rust
/// One proposed routine: the fields of `schedules/<name>.md`, named as `ScheduleArgs` names them
/// (tools.rs:80-126) so the model's next call is a field-for-field copy.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineDraft {
    /// `[a-z0-9][a-z0-9-]*`, ≤ 40 chars (`file::valid_name`). An existing name is replaced (the card says so).
    pub name: String,
    /// The §2.2 sentence, written by the model from the skill's grammar; shown on the card.
    pub sentence: String,
    pub cron: Option<String>,
    pub every: Option<String>,
    pub at: Option<String>,
    pub prompt: String,
    #[serde(default)] pub quiet: bool,
    pub model: Option<String>,
    pub until: Option<String>,
    pub max_runs: Option<u32>,
    pub run_timeout: Option<String>,
    pub cwd: Option<String>,
    pub approvals: Option<bool>,
    /// New in this wave on `ScheduleArgs` too (D13).
    pub agent: Option<String>,
    pub project: Option<String>,
    pub when: Option<String>,
    pub fire: Option<String>,
    #[serde(rename = "for")] pub for_: Option<String>,
    pub cooldown: Option<String>,
    pub mode: Option<String>,
    pub memory: Option<String>,
    pub expires: Option<String>,
    pub timeout: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineDraftArgs {
    /// One or more proposed routines (normally one).
    pub routines: Vec<RoutineDraft>,
}
```

`NAME = "routine_draft"`, `RISK = Risk::ReadOnly`, `RETURNS = "dict"` (as `agent_draft.rs:123-126`).
**Validation before any card** (`agent_draft.rs:238-271` pattern): `file::valid_name`; render the
draft through `file::render(&FileArgs{…})` and `file::parse` it — the `schedule` tool's own
pre-write path (`tools.rs:164-197`), with `created` stamped as `tools.rs:165-166` does — and refuse
with that message (`routine_draft: <name>: <parse error>`) so a card never proposes a file the
engine would reject; sentence non-empty; a duplicate name in one call refused. The sensor is
*resolved* (`SensorRunner::resolve`, `sensor/mod.rs:221-282`) but **not** dry-run here: the
dry-run is `schedule`'s job after the press (a card must appear in under a second).

### 8.2 Card anatomy (OpenUI source, `agent_draft.rs:276-387` shape)

```
root = Stack([intro, c1])
intro = TextContent("**1 routine draft** — *Create paused* writes the file disabled, *Create and start* writes it live, *Open in form* puts it in the editor beside this chat. Nothing is written until you press.", "small")
c1 = Card([c1_head, c1_sentence, c1_parts, c1_prompt, (c1_note), c1_buttons])
c1_head     = CardHeader("<name>", "routine draft r1-<suffix>")
c1_sentence = TextContent("**<sentence>**")
c1_parts    = TextContent("Trigger `<clause>` · Stop `<clause or never>` · Run `<clause or defaults>` · Concurrency `<mode or deferred>`", "small")
c1_prompt   = TextContent("<first 600 chars of the prompt>…")           // PREVIEW = 600 as agent_draft.rs:26
c1_note     = TextContent("`<name>` exists — *Create…* replaces schedules/<name>.md.", "small")   // only when the name exists
c1_buttons  = Buttons([c1_paused, c1_start, c1_form])
c1_paused   = Button("Create paused",    Action([@ToAssistant("routine draft r1-<suffix>: create paused")]))
c1_start    = Button("Create and start", Action([@ToAssistant("routine draft r1-<suffix>: create and start")]), "secondary")
c1_form     = Button("Open in form",     Action([@ToAssistant("routine draft r1-<suffix>: open in form")]), "tertiary")
```

Ids `r<i>-<last 4 alnum of call_id>` (`agent_draft.rs:173-178, 282-290`). `APP_NAME =
"routine-drafts"`; title `Routine draft` / `<n> routine drafts`. The `c1_parts` line is built in
Rust from the fields with the clause *labels* only (no sentence grammar in Rust): Trigger = the
clock clause or `every <d> when <file>`; Stop = `until`/`max_runs`/`expires` joined or `never`;
Run = the non-default run keys joined or `defaults`; Concurrency = `mode` or `deferred`.
"Discard" is deliberately absent: a draft that is not pressed is nothing (`agent_draft.rs:9`); the
user types instead. The existence check reads `<dir>/<name>.md` (the tool gets the schedules
dir like `ScheduleTool`, `tools.rs:42,50`).

### 8.3 Tool result and the press message

Result text (`agent_draft.rs:219-227` shape): `drafted 1 routine card (record_call_id <id>):\nr1-ab12 ·
<name> — <sentence>\nWait for the user. Their press arrives as a message beginning
`[app routine-drafts …]` with `routine draft <id>: create paused | create and start | open in
form`. Then: create paused → `schedule` with that draft's fields and `enabled: false`; create and
start → `schedule` with `enabled: true`; open in form → answer one line, call nothing.`
Structured: `{ record_call_id, drafts: [{ id, name, exists }] }`.

The press is the user message `[app routine-drafts v<version> · <record_call_id>]\nroutine draft
<id>: <choice>` — produced by `assistantInput` (`lifecycle.ts:344-358`), parsed back by
`parseAppInput` (`lifecycle.ts:372-379`). Grammar: `routine draft <id>: <choice>`, `<id>` =
`r[0-9]+(-[a-z0-9]{1,4})?`, `<choice>` ∈ {`create paused`, `create and start`, `open in form`}.
A pure function `pub fn approval(choice: &str) -> Option<Enabled>` mirrors
`agent_draft::approval` (`agent_draft.rs:84-100`) and is unit-tested.

### 8.4 What the model does next

| choice | call | note |
|---|---|---|
| `create paused` | `schedule({ name, cron, every, at, prompt, quiet, model, until, max_runs, run_timeout, cwd, approvals, agent, project, when, fire, for, cooldown, mode, memory, expires, timeout, enabled: false })` | the tool's reply (next fire · `disabled: resume with …`, verdict for a watcher) is relayed in one line |
| `create and start` | the same with `enabled: true` | D12: a watcher with a clean verdict is written enabled; an `Err` verdict refuses the write as today — the model relays the verdict and offers to fix |
| `open in form` | none | "It's in the form — edit and press Create." |

Tests (`crates/app`): source contains the three buttons with the exact `@ToAssistant` strings;
ids for one and two drafts; refusal on an unparsable draft (`every: 5s`), on a bad name, on a
watcher without a stop; `approval()` table.

## 9. `list_sensors`, the `# arg:` header, `SensorInfo`, and RPC additions (ENG)

### 9.1 `# arg:` header grammar (adds to `assets/sensors/README.md` §Header)

One line per argument inside the first 16 header lines (`HEADER_LINES`, `sensor/mod.rs:24`), after
`# theuth-sensor:` and before the docstring:

```
# arg: <name> <required|optional> <example> — <help text to end of line>
```

- `<name>` matches `[a-z][a-z0-9_]*` (the `when:` key rule, `file.rs:51-53`); `cwd` refused.
- `<required|optional>` is literal; anything else is a header error.
- `<example>` is one token (no spaces) shown as the field's placeholder; `-` means none. A
  value that names a vault secret uses the example `secret` (the form offers `list_secrets`
  names); a URL list uses `url[,url]`.
- ` — ` (space, em dash, space) or ` -- ` separates the help; the help is free text to the end of
  the line, may be empty.
- Order of lines = order in the form and in the sentence's subject (§2.2).
- Unknown `# key:` lines stay ignored (`sensor/mod.rs:275`); a malformed `# arg:` is a
  `SensorError::Header` like a bad `# hosts:` (`sensor/mod.rs:263-274`) — the sensor is listed
  with `error` and cannot be picked.

The two built-ins gain headers (RSS owns `rss.py`'s; ENG edits these two, bumping neither
version — a header comment changes the sha, so ENG bumps `# theuth-sensor:` to `pr_watch 2` /
`fs_exists 3` and the test expectations in `sensor/mod.rs:858-901` so unedited installs update):

```
# theuth-sensor: pr_watch 2
# hosts: api.github.com
# arg: repo  required owner/name — the repository
# arg: pr    required 482        — the pull request number
# arg: token required secret     — vault name of a GitHub token with repo scope
# arg: stale_hours optional 72   — hours without activity before `stale` fires
```
```
# theuth-sensor: fs_exists 3
# arg: path required drop/*.txt — glob under the routine's directory; fires while it matches
```

### 9.2 `SensorInfo` and the RPC

`agent::sensor::SensorMeta` gains `args: Vec<SensorArg>` and `description: Option<String>`
(the first non-empty line of the module docstring — the line after the opening `"""`
(`pr_watch.py:3`); `usage: Option<String>` = the first docstring line starting `when:`
(`pr_watch.py:5`, `fs_exists.py:4`)). `resolve()` parses them (`sensor/mod.rs:242-277`).

```rust
// crates/agent/src/sensor/mod.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SensorArg { pub name: String, pub required: bool, pub example: Option<String>, pub help: String }

// crates/schedule/src/lib.rs (exported through rpc like ScheduleInfo, export.rs:210-215)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SensorInfo {
    pub name: String,                 // `pr_watch.py`
    pub description: Option<String>,  // docstring line 1
    pub usage: Option<String>,        // the `when:` line
    pub args: Vec<SensorArg>,
    pub version: Option<String>,      // `# theuth-sensor:` version; None for a user file
    pub builtin: bool,                // name is in BUILTINS
    pub edited: bool,                 // builtin whose sha256 ≠ sensors.json's record (install_builtins' rule, mod.rs:385-416)
    pub shipped_version: Option<String>, // the binary's version of a builtin (differs when `edited` blocks an update)
    pub needs_cpython: bool,
    pub cpython_available: bool,      // `find_cpython().is_some()` (mod.rs:577)
    pub hosts: Vec<String>,
    pub has_probe: bool,
    pub error: Option<String>,        // header error; other fields best-effort
}
```

`ListSensors` message on the `Schedule` actor (it owns the `SensorRunner` it hands to
`tools::register`, `tools.rs:42`); lists `<data_dir>/sensors/*.py` (non-recursive, dotfiles
ignored, `sensor/mod.rs:43-44` rule) and `resolve`s each. RPC `#[method(name = "list_sensors")]
async fn list_sensors(&self) -> RpcResult<Vec<schedule::SensorInfo>>` next to `list_schedules`
(`api.rs:1196-1198`), added to `export.rs:210-215`, `api.ts` + `METHODS`. Also a runtime
function `list_sensors()` for the model: a `ListSensorsTool` (`Risk::ReadOnly`) in
`schedule/src/tools.rs` returning the same rows as text + structured, registered in
`tools::register` (`tools.rs:42-54`).

### 9.3 `RunInfo.kind` and `ScheduleInfo` additions

```rust
#[derive(…, ts(export))] #[serde(rename_all = "snake_case")]
pub enum RunKind { Run, Check, End, Skip }
pub struct RunInfo { …, pub kind: RunKind }   // lib.rs:349-364
```

Derived in `Store::history` (`store.rs`, the `RunRow` select) from what the engine already
knows: `session.is_some() && result ∉ {skipped: …}` → `Run`; `result` starts with `skipped:` →
`Skip`; `result` ∈ end strings (`done…`, `max_runs…`, `until broken…`, `expired`, `removed`,
`check broken ×3, disabled`, 33 §3.3 / 40 §5) → `End`; else (`session: None`, a gate row,
`lib.rs:1549-1558`) → `Check`. A migration is **not** needed: `kind` is computed on read. Tests:
one row of each kind through `history()`.

`ScheduleInfo` (`lib.rs:295-344`) gains `for_: Option<String>` (serde `for`), `cooldown:
Option<String>`, `timeout: Option<String>`, `model: Option<String>`, `run_timeout:
Option<String>`, `created: Option<DateTime<Local>>`, `sensor: Option<String>` (the file name
alone) — all as written / resolved from `Spec` (`file.rs:284-337`), so the list sentence needs
no second fetch.

### 9.4 `test_schedule_file`

`#[method(name = "test_schedule_file")] async fn test_schedule_file(&self, file:
schedule::file::ScheduleFile) -> RpcResult<schedule::Verdict>`: `file::render` + `file::parse`
(`put_schedule`'s path, 70 §as-built line 18), then the §1.12 dry-run that `ScheduleTool::call`
runs before writing (`tools.rs:198-200`, `validate.rs`), **without** writing, binding or
reconciling. `Err` for a parse error or `not a watcher`. The Rust lives in `validate.rs` so the
tool and the RPC share it.

### 9.5 `ScheduleArgs` and the `schedule` tool

`agent: Option<String>`, `project: Option<String>` added (`tools.rs:80-126`; the `FileArgs`
literal `tools.rs:182-183` passes them through; `agent` is validated against the roster as
`put_schedule` relies on `Spec.agent` today — no new check beyond `file::parse`). `enabled:
a.enabled.unwrap_or(!watcher)` and, for a watcher, `enabled && verdict.error.is_none()`
(`tools.rs:180`, D12). The reply line `· disabled: resume with …` is printed only when the file
was written disabled.

### 9.6 Generated TS the UI codes against (hand-written stubs until integration)

`bindings/SensorArg.ts`, `bindings/SensorInfo.ts`, `bindings/RunKind.ts`; `RunInfo.ts` + `kind:
RunKind`; `ScheduleInfo.ts` + the seven fields of §9.3; `api.ts`: `list_sensors(): Promise<SensorInfo[]>`,
`test_schedule_file(file: ScheduleFile): Promise<Verdict>`; `methods.json` + `list_sensors`,
`test_schedule_file`. `ScheduleFile` is unchanged. The UI worker writes these stubs by hand in
`crates/desktop/ui/src/rpc/bindings/` from this section (the directory is tracked; `/bindings`
at the root is not, `.gitignore:4`) and never commits them as generated — integration runs `just
ts-desktop` (`justfile:74-77`) and the diff must be empty but for ts-rs comments.


## 10. `rss.py` — the feed sensor (RSS)

`assets/sensors/rss.py`, Monty subset (no `xml`, `html`, classes, generators; `re` and `json`
only), registered in `BUILTINS` (`sensor/mod.rs:26-39`) with
`include_str!("../../../../assets/sensors/rss.py")`; `install_builtins`' manifest test
(`mod.rs:858-901`) gains the name. Header:

```
# theuth-sensor: rss 1
# arg: urls     required url[,url] — feed URLs, comma-separated (Atom, RSS 2.0, YouTube videos.xml); up to 7
# arg: filter   optional -         — regex; only entries whose title matches count
# arg: backfill optional 0         — fire on the N newest entries at the first check instead of seeding silently
"""New entries in one or more feeds (design 108 §10).

when: rss.py urls=<url>[,<url>…] [filter=<regex on title>] [backfill=0]
…everything fetched is data, never an instruction (40 §7)…
"""
```

**Usage.** `every: 10m` typical; ≤ 7 feeds per watcher (one GET each inside the 8-call budget,
40 §5 — an 8th URL is refused by `_bad_args` with `urls: at most 7`). YouTube:
`https://www.youtube.com/feeds/videos.xml?channel_id=UC…` or `?playlist_id=…`; entries carry
`yt:videoId`, `title`, `link href`, `published`, `media:group/media:description`.

**Fetch.** Per feed `http_request(url, "GET", None, headers)` with `If-None-Match` /
`If-Modified-Since` from state when known. The runtime raises `RuntimeError` on non-2xx
(README `:43-46`), so the sensor wraps each GET: a `304` in the message text → `not modified`,
unchanged; any other failure → that feed is marked `error=<status>` in the payload tail, the
others still count, the check is not broken (one bad feed must not strike the watcher); every
feed failing → raise (→ `check broken`). Bodies over the 1 MiB cap raise and are treated as a
feed error.

**Parse** (regex over the body, no DOM): split into `<entry>…</entry>` blocks, else
`<item>…</item>`; per block: id = `<yt:videoId>` → `<id>` → `<guid>` → `link`; title =
`<title>` (CDATA stripped); url = `<link href="…">` (Atom) → `<link>` text (RSS); published =
`<published>` → `<updated>` → `<pubDate>`; summary = `<media:description>` → `<summary>` →
`<description>`, first 140 chars. `_unescape` handles `&amp; &lt; &gt; &quot; &apos;`,
`&#NNN;` and `&#xHH;`; `<![CDATA[…]]>` unwrapped before unescaping; tags inside summaries
stripped. Feed title from `<title>` before the first entry; host from the URL for the subject.

**State** (≤ 16 KiB, 40 §5): `{"seen": [ids newest-first, FIFO capped at 300 across all feeds],
"feeds": {url: {"etag", "last_modified", "title"}}, "seeded": true}`. Ids are stored as given
(a 300-id FIFO of 40-char ids ≈ 13 KiB worst case; ids are truncated to 120 chars).

**First check** (`state == {}`): seed every entry into `seen`, `fire: false`, payload
`subject=feed/<host-or-channel> events=seeded count=N` + `seeded N entries from M feeds` — so the
creation dry-run says *would not fire* (40 §1.12) instead of firing on the back-catalogue.
`backfill=N` instead treats the N newest (by `published`, across feeds) as new on the first check.

**Fire** = any entry id not in `seen` (after `filter`). Payload (≤ 4 KiB): envelope
`subject=feed/<host-or-channel> events=new count=N` (`subject` = the single feed's host or
`channel/<title>` for YouTube, `feeds/<M>` when several), then one line per new entry newest first:
`id=<id> published=<iso> title=<title> url=<url>`, then `+K more` when the 4 KiB budget stops it.
State's `seen` is updated in the returned state; the gate commits it only after the run succeeds
(40 §1.7) → at-least-once per entry; `fire: on_change` is harmless because a new entry changes the
payload hash. **`next`**: from `Cache-Control: max-age` or `<sy:updatePeriod>` when present, else
`None`; the gate clamps to `[every, 10 × every]` (40 §1.8) — the sensor returns the raw seconds.

**`probe(args)`**: GET each feed once (no conditional headers), return `"<host>: <N> entries,
parsed as <atom|rss|youtube>"` per feed joined by `; `, or `"<host>: <error>"` — never raise.
Handles: resolving a YouTube `@handle` is *not* done here (the channel HTML can exceed the 1 MiB
cap); the skill does it in conversation (§7.4) and pins `channel_id`.

**Tests** — `assets/sensors/tests/test_rss.py` in the `test_pr_watch.py` pattern (`monty_shim.py`
`FakeHttp`/`load`/`run`, `monty_shim.py:14-43`; `FakeHttp` keys on `(METHOD, path)` after
stripping the GitHub origin — the RSS test passes full URLs, so RSS adds a `FakeFeeds` responder in
the test file that keys on the full URL and can answer `(304, "")`): fixtures as string constants
for Atom, RSS 2.0 (with CDATA and `&amp;`), YouTube (`yt:videoId`, `media:description`), a
malformed body, a 304; cases: seed on empty state (no fire, payload says seeded); one new entry
fires with the exact envelope; two feeds, one 304; `filter`; `backfill=2`; FIFO cap at 300; 8 URLs
refused; one feed erroring does not break the check; all feeds erroring raises; `probe` text;
payload under 4 KiB with 200 new entries (`+K more`). Run as the README says:
`python3 -m unittest discover -s assets/sensors/tests` (`README.md:100-104`). README gains an
`## rss` section modelled on `## pr_watch` (`README.md:72-98`).

## 11. Seams

| | **UI** (`crates/desktop/ui`) | **ENG** (engine) | **RSS** (sensor) |
|---|---|---|---|
| owns | `src/components/routines/*` (new: `RoutineChat.tsx`, `sentence.ts` if split out, `draft.ts` = `draftToFile`/`fromInfo`/`fromFile`), `src/routes/routines.*.tsx`, `src/components/apps/AppHost.tsx` (the one `dispatchEvent` line), `src/components/chat/MessageTool.tsx:131` (one `||`), `src/routes/__root.tsx` (mount `RoutineDraftBridge`), `src/rpc/api.ts` (+2 methods), hand-written stubs under `src/rpc/bindings/` (§9.6), `src/rpc/bindings/methods.json` (+2 names) | `crates/schedule/{src/lib.rs,src/tools.rs,src/store.rs,src/validate.rs,routines.md}`, `crates/agent/src/sensor/mod.rs` (header parse, `SensorArg`, `description`/`usage`, BUILTINS entry for `rss.py` — the *line*; the file is RSS's), `crates/app/src/{routine_draft.rs,lib.rs}`, `crates/rpc/src/{api.rs,export.rs}`, `crates/theuth/src/main.rs:1168-1176` (one tuple), `assets/sensors/{pr_watch.py,fs_exists.py}` headers only (§9.1), `assets/sensors/README.md` §Header (`# arg:` grammar paragraph only) | `assets/sensors/rss.py`, `assets/sensors/tests/test_rss.py`, `assets/sensors/README.md` `## rss` section |
| must not touch | any `.rs`; `assets/`; `bindings/` at the root; `routeTree.gen.ts` is regenerated by `vite build`, not edited | `crates/desktop/ui/**`; `rss.py`, `test_rss.py`; `README.md` outside its paragraph | everything except its three files; in particular `sensor/mod.rs` (ENG adds the `BUILTINS` line and `include_str!` — RSS's file must exist at that path, so RSS creates `rss.py` first thing and ENG's compile depends on the integration merge, not on RSS's branch: ENG adds the `BUILTINS` entry in a commit that also adds a 10-line placeholder `rss.py` **only if** `assets/sensors/rss.py` is absent in its tree — it will be, so ENG's placeholder is overwritten by RSS's file at integration, same-new-file conflict resolved in RSS's favour) |
| contract | codes against §9.6 TS and §8.2's `routine_draft` args shape (`draftToFile`); mocks `list_sensors` / `test_schedule_file` / `RunInfo.kind` in a `dev/routines-fixture.ts` behind `import.meta.env.DEV` so the pages render without the engine | produces exactly §8.1, §9.2, §9.3, §9.4, §9.5; the §2.2 grammar in `routines.md` verbatim (copy the section) | header per §9.1, envelope per §10, tests runnable by the README command |
| siblings named in the brief | ENG, RSS | UI, RSS | UI, ENG |

**Integration order** (the 101 procedure, one warm tree): RSS first (pure additions), then ENG
(its `BUILTINS` line now points at the real file; `cargo test -p agent -p schedule -p app -p
rpc`), then `just ts-desktop` (`justfile:70-77`), then UI merged on top; the UI's hand-written
stubs are replaced by the generated files (diff must be empty but for comments); `bun run check`,
`CI=1 bun run test`, `bun run build`; a vite-dev screenshot pass of `/routines`,
`/routines/new?chat=1` and a detail page at 1280 px and 390 px, like 101 §6.

## 12. Gates

- **UI**: in `crates/desktop/ui`: `bun run check` (= `tsc -b --noEmit`, `package.json:11`),
  `CI=1 bun run test` (`vitest run`, `package.json:10`; `pairing.test.ts` is a known load flake —
  rerun it alone), `bun run build` (`tsc -b && vite build`, `package.json:8`; regenerates
  `routeTree.gen.ts`). Never the Tauri CLI. New tests: `helpers.test.ts` (every §2.2 example,
  `kindOf`, `resultKind` check kinds, `draftToFile` round trip, the §5.5 message map),
  `RoutineChat` first-send sequence with a mocked `api` (order of the five calls).
- **ENG**: `cargo test -p schedule -p app -p rpc -p agent --no-fail-fast`; `cargo check
  --workspace`; `cargo clippy --workspace --all-targets -- -D warnings` (`justfile:24-25`, the
  repo's rule); rustfmt on touched files only (never `cargo fmt --all`); `just ts` must export
  cleanly. Monty sensor tests need `THEUTH_WORKER` or `t` on PATH (40 §13 as built) — the
  header-parse tests do not.
- **RSS**: `python3 -m unittest discover -s assets/sensors/tests` (`README.md:102-104`) green
  including the existing 37 `test_pr_watch.py` cases; `rss.py` type-checks under the sensor
  profile — the ENG tree's `cargo test -p agent sensor::` runs `pr_watch.py` through the checker
  today (40 §13: "pr_watch.py type-checks under the profile"); the integration tree runs the same
  for `rss.py`, which is why RSS writes Monty-subset Python only (no f-string nesting beyond one
  level, no `dict.setdefault` chains the checker lacks — follow `pr_watch.py`'s idioms).

## 13. Open questions (for Sky; not blocking)

1. Should **Create and start** on a watcher card skip the pre-press dry-run entirely (as
   designed: the `schedule` tool dry-runs after the press, so a broken sensor is a refusal the
   user sees once) — or should the skill always propose paused for watchers and let the Overview's
   Test decide? Designed: trust the press (D12).
2. The sentence capitalises `cronWords` output and inserts *at* (`Weekdays at 09:00`), changing
   seven existing list strings; keep the old lower-case forms instead?
3. Phone: the draft pane as a bottom `Sheet` means the form is not visible while chatting; an
   alternative is a two-tab page (Chat · Form). Designed: Sheet, with *Open in form* closing it.

## 14. Not in this wave

`on: fs` and any event source other than a sensor (40 §4.6); a `t trigger` CLI or payload-carrying
`fire_schedule`; MCP tools or secrets in CPython sensors; a sensor editor or sensor install from
the UI (`list_sensors` is read-only; files are edited on disk); YouTube `@handle` resolution in
`rss.py`; `until: <sensor>.py` (40 §11); a Rust implementation of the sentence grammar; TUI
`/schedules` changes (it keeps `describe`); drafts that persist beyond the card; a routine
"template gallery" beyond the three intro chips.
