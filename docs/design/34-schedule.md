# 34 — Schedule: a prompt file that fires on a clock

Status: as-built v0 · 2026-09-16 · extends `00-architecture.md` §4 (plugins), §5
(agent), `01-config-and-subagents.md` §1.4 (drop-in directory, hot reload),
§2.3 (delivery), `14-jobs.md` §3 (a later result wakes a session),
`24-message-user.md` §2–3 (reaching the user). Where this doc disagrees with
those, this doc wins; §12 lists the edits to 00. Built 2026-09-16; §13 is
the as-built record.

## 0. Why, and what jarvis built

The user wants the harness to do things when nobody is typing: a morning
brief, an hourly check of a queue, "remind me at three". jarvis had this
as `jarvis-scheduler`. What it had become by the end (mapped 2026-09-16
against `../jarvis`, file:line in the map):

| | jarvis |
|---|---|
| Crate source | 4 276 lines (`service.rs` 2 899, `dag.rs` 609, `heartbeat.rs` 329, `mock.rs` 313) + 3 884 lines of tests |
| Job record | `JobSpec`, **22 fields**, across **9 migrations** (`jobs` table + `job_runs`) |
| Schedule syntax | 6-field cron, **UTC only**; 5-field rejected while the agent tool's own example was 5-field |
| Timezone | goals converted local→UTC with the *current* offset: "a DST change shifts the fire time by an hour until the next config reload" (`briefing.rs:331`) |
| Result to user | `notify_signal`, `notify_slack`, `notify_on` filter, "longest assistant message in the last 50" heuristic, `goals:` prefix exemption |
| Same-crate passengers | a petgraph task DAG (~2 100 LOC, shares the scheduler's mutex and semaphore), a built-in hourly heartbeat with a 150-line prompt, preflight and initiative traits, a tool allow-list keyed on `sender.contains("jarvis: heartbeat")` |
| Failure handling | wall-clock watchdog → truncate the conversation → re-fire once; continuous-context token trimming through `agent.compact`; `on_overlap`, `on_timeout`, `max_context_tokens`, `max_wall_minutes` |
| Dead weight | `phase_gating` persisted per job and never applied at fire (`fire_job` passes `None`); `update_job` = list + find + merge + full replace ("O(n) and racy", its own design doc); briefing jobs whose agent run was a one-sentence "ack" while goals did the work off the `JobFired` event |
| Core | "run prompt X at time Y, one conversation per job, record last status" ≈ **300 of the 2 899 lines** |

Every passenger solved a problem the harness around it had. theuth already has
the primitives those passengers were substitutes for: a session that can be
woken by a harness input (14 §3), `/clear` as an epoch (11 §2.4), one
delivery path to the user (24), `Risk` as the whole permission contract,
`knowledge_*` for memory across runs, a drop-in directory that is the
registry (01 §1.4, 04 §0), and a turn cancel. So the scheduler here is the
300 lines, and nothing that the rest of the harness already does.

## 1. Decisions in one screen

1. **A schedule is a markdown file.** `~/.theuth/schedules/<name>.md`: front
   matter says *when*, the body is the prompt. The directory is the
   registry; there is no table, no id, no `update` — edit the file. Same
   rule as `mcp/` and `skills/` (04: "a directory is the registry; tools
   write files, nothing else holds state"). Hot-reloaded with the `mcp/`
   watcher pattern (`notify`, 300 ms debounce, reconcile by file stem).
2. **Three ways to say when, one per file:** `cron:` (standard 5-field,
   optional seconds, **local time**), `every:` (a duration: `30m`, `6h`),
   `at:` (one instant; the file is consumed by its fire). `croner` computes
   the next `cron` occurrence from a `DateTime<Local>`, so DST is the OS's
   problem, not ours. Rejected: 6-field UTC (jarvis, and every complaint
   above); a `timezone:` key — the harness runs where the user is;
   `TZ=` exists for the other case.
3. **A tick, not a long sleep.** The actor wakes at the earliest `next`
   or every 60 s, whichever is sooner, and fires everything due. Reason:
   `std::time::Instant` may exclude time the machine is suspended (it
   does on macOS), so a `sleep_until(09:00)` armed at 23:00 on a laptop
   that sleeps fires hours late. A tick sees a due schedule within a
   minute of wake, which is also the catch-up story for the same process.
   Across a *restart*, `cron` schedules start from now unless the file
   says `missed: run` (jarvis `catch_up`; a morning brief on a laptop that
   was closed is the case), and `every` schedules run at once when
   `last + every` has passed — an interval is defined relative to the
   last run, so that is not a catch-up, it is the schedule.
4. **One session per schedule; every fire is a fresh epoch.** The first
   fire creates the session (title = the schedule name) and
   `schedule.sqlite` remembers it; later fires `Compact { style: Clear }` then send the
   prompt. `/sessions` shows one `morning-brief`, `updated_at` = last
   fire, and opening it shows the last run; earlier runs are the earlier
   epochs. Memory across runs is `knowledge_put`/`knowledge_search`, which
   survives `/clear` and compaction and is what the harness already tells
   the model to use. Rejected: a session per fire (288 sessions a day for
   a five-minute monitor); jarvis `context_mode: continuous` plus token
   trimming (the frame fills, compaction summarises, and the model then
   *thinks* it remembers — knowledge is explicit).
5. **The fire is a wrapped input.** `<schedule name="morning-brief"
   at="2026-09-16T09:00:00-07:00" quiet="false">…prompt…</schedule>` via
   `agent::SendInput { session: Some(id) }`; the TUI's wrapper parser
   cards it (`tui::model::injected`, one new arm). The model knows the
   clock is speaking, not a person. If the session is mid-turn when the
   next fire is due (a five-minute monitor whose run took seven), the fire
   is **skipped** and `ScheduleEvent::Skipped { reason: "busy" }` is
   published. Rejected: queue it as a steer (the timer's prompt would land
   inside someone else's turn); `on_overlap: cancel_previous` (a run that
   is still going is the interesting one).
6. **The reply reaches the user.** On `TurnFinished` the run's text is
   delivered through the existing `Deliver` on `USER_ACTOR` (24 §2): the
   session is unbound, so it fans out to every configured channel's owner,
   prefixed `schedule morning-brief`. The model does not have to remember
   to call a tool to be heard, which is the failure jarvis fought with
   claim-check, a "revise" pass and the `decision_required` machinery.
   Nothing is delivered when the run already called `message_user`
   (that *was* the delivery) or when the file says `quiet: true` (a
   monitor: only `message_user` reaches the user, and only when the
   model has something). A run that ends `error` or hits the timeout is
   reported regardless of `quiet`. No channel configured → the transcript
   is the record and a log line says so.
7. **One deadline, one cap, no retries.** `schedule.run_timeout_secs`
   (default 1800) → `agent::Cancel { session }`, recorded as `timeout`,
   reported. A run that stops early is already bounded by
   `agent.max_tool_rounds`; this is the leash for a turn that does not.
   No `max_concurrent`: schedules that fire together run together, one
   session each, like subagents do. No truncate-and-retry, no continuous
   trimming, no `on_timeout` — the next fire is the retry.
8. **The model gets one tool, `schedule`.** It validates and writes the
   file and answers with the next fire time. Everything else is the file
   system it already has: `read_file` to see a schedule, `list_dir
   ~/.theuth/schedules` to list, `shell rm` to cancel. Every one of those is
   gated by its own `Risk` as today. Rejected: `list_jobs`, `get_job`,
   `update_job`, `cancel_job`, `run_job`, `job_decision` (jarvis, six
   tools + three MCP mirrors for one file).
9. **No heartbeat, no goals, no DAG.** A heartbeat is a schedule file the
   user writes (`heartbeat.md`, `every: 1h`); the harness ships none. The
   task DAG and goal briefings were subsystems jarvis never used daily
   (00 §13) and are not this doc's to return.

## 2. Files

### 2.1 Directory

```
~/.theuth/schedules/
  morning-brief.md        one schedule; the stem is its name
  queue-watch.md
```

Non-recursive. Anything not `*.md`, or starting with `.`, is ignored.
Name = file stem, so it is what the user typed and what `/sessions` shows.

### 2.2 Format

Loops (33 §2) add `until`, `max_runs`, `cwd`, `approvals` and lift the
`every` floor to `0s` when a stop is present.

```markdown
---
cron: "0 9 * * 1-5"      # or   every: 30m     or   at: 2026-09-16T15:00
quiet: true               # optional; default false (§1.6)
missed: run               # optional, cron only; default skip (§1.3)
model: claude-haiku-4-5   # optional; default agent.model
enabled: false            # optional; listed as off, never fires
---
Summarise my calendar for today and the unread mail that needs a reply.
```

Front matter through `gray_matter` as skills do (04 §1.2). Exactly one of
`cron` / `every` / `at`; zero or two is an error. Parse rules:

| key | parser | notes |
|---|---|---|
| `cron` | `croner::Cron::from_str(expr)` (croner 4, `chrono` feature) | 5 fields, or 6 with leading seconds (optional by default); `L`, `W`, `#` as croner accepts them; evaluated in `chrono::Local`. `Cron::describe()` gives the English line the table and the tool reply show |
| `every` | `humantime::parse_duration` | `20s`, `30m`, `1h 30m`, `2d`; minimum `10s` |
| `at` | `DateTime::parse_from_rfc3339`, else `NaiveDateTime` (`%Y-%m-%dT%H:%M[:%S]`) read as `Local` | past instants are an error at parse |
| `budget` | `<amount> <currency>` (`1 USD`) | 136 §15: on every fire the plugin sends `agent::SetBudget{session, nanos, currency}` (`nanos: None` when the key is absent); enforced at `Reserved::open` — a model call that would pass the cap is refused, the turn ends with one `<budget>` note, a side call answers `budget exhausted`. `FireWith{name, overrides}` (RPC `dreaming.run_now`) fires once with a budget / `run_timeout` / `<request>` payload override for that run only |

136 §15: the harness installs the bundled routine `schedules/dreaming.md`
(written once, when absent, by `dreaming::DreamingPlugin::start`; the user's
`enabled:` toggle survives restarts); `approvals: false` is permitted for it
because its tool surface is the bounded set in 136 §9.4.

A file that fails to parse is **listed with its error** (`/schedules`
shows `morning-brief · error: cron: invalid day-of-week`) and never fires;
the other files are unaffected. jarvis logged and disabled the row; here
the user sees the reason where they look for the schedule.

### 2.3 Reload

`Schedule` watches the directory exactly as `tools::mcp` does
(`mcp.rs:1496-1509`: `notify::recommended_watcher`, `NonRecursive`, 300 ms
debounce, drain, one `Reconcile` tell). Reconcile reads every file, diffs by
name against the live set: new → parse, compute `next`; changed (text
differs) → re-parse, recompute `next`, keep `session` and `last`; removed →
drop it (the session stays in the store, the rows stay in `schedule.sqlite`).
Startup is the same reconcile with an empty live set, then §7's latest row
per name is joined in.
Every reconcile ends with `ScheduleEvent::Changed` so a `/schedules` table
that is open can refresh.

## 3. Time

```rust
enum When { Cron(croner::Cron), Every(Duration), At(DateTime<Local>) }

fn next(&self, now: DateTime<Local>, last: Option<DateTime<Local>>) -> Option<DateTime<Local>> {
    match self {
        When::Cron(c)  => c.find_next_occurrence(&now, false).ok(),
        When::Every(d) => Some(last.map_or(now, |l| l + *d)),   // no last: first fire is now
        When::At(t)    => (*t > now).then_some(*t),             // past: never
    }
}
```

The actor loop:

```rust
loop {
    let now = Local::now();
    for s in due(now) { fire(s).await }                          // next <= now, enabled, no error, not running
    let wake = earliest_next().map_or(TICK, |t| (t - now).to_std().unwrap_or_default()).min(TICK);
    tokio::select! { _ = sleep(wake) => {}, _ = reconcile_rx.recv() => {} }
}
const TICK: Duration = Duration::from_secs(60);
```

A schedule fires **at most once per tick** however many occurrences have
elapsed: after eight hours asleep a five-minute monitor runs once, not 96
times. `every` measures from the fire that happened (`last`), not from the
one that should have: `every: 6h` fired late at 15:10 fires next at 21:10.
That is what an interval means; users who want wall-clock instants write
`cron`.

Boot with `missed: run`: after the store is read, a `cron` schedule whose
`last` is before the most recent occurrence before `now`
(`find_next_occurrence` from `last` ≤ `now`) is due immediately. Without
`last` (never fired) nothing is missed. Default `skip`: a laptop closed
through 09:00 does not brief at 14:00 unless the file asks.

Second-resolution cron (`*/20 * * * * *`) is accepted and honoured: the
tick is `min(next − now, 60 s)`, so a schedule due in 20 s wakes in 20 s;
60 s is the ceiling for the suspended-clock case, not the resolution.

## 4. Runs

Loops (33 §3) run the `until` check and the `max_runs` count before step 1,
keep a run open while its subtree is live, and end by removing the file.

`fire(s)`, in order — every step is an existing `agent` message:

1. `running.contains(name)` → publish `Skipped { reason: "busy" }`, return.
2. No session yet → `NewSession { cwd: None }` → `RenameSession { title:
   name }` → if `model:` → `SetModel { session: Some(id), model }`. Persist
   `session` (§7).
3. Session exists and has turns → `Compact { session, style: Some(Clear) }`
   (`Err("nothing to clear")` on an empty session is ignored).
4. `SendInput { session: Some(id), text: wrapped, files: [] }` with the §1.5
   wrapper; body is the file body trimmed. `at:` schedules: remove the file
   now (the fire is the file's purpose; the session is the record; a
   reconcile follows and drops the entry).
5. `INSERT` the run row (§7); `running.insert(name, Run { id, session,
   quiet, text: String::new(), messaged: false, deadline })`; publish
   `Fired { name, session, at }`.

The actor subscribes to `AgentEvent` (unbounded mailbox, 00 §3) and, for
sessions in `running`:

- `TextDelta` → append to `text` (the way `signal::channel` collects a
  bound session's reply, `channel.rs:1266`).
- `ToolCallStarted { name: "message_user" }` → `messaged = true`.
- `TurnFinished { stop_reason }` → remove from `running`; `last_result` =
  `timeout` if the actor cancelled it, else `stop_reason` (with the last
  `Error { message }` appended for `error`); `UPDATE` the run row; then
  deliver (§4.1).

Deadline: `sleep_until(deadline)` in the loop's `select!` (earliest of the
running deadlines); on fire `Cancel { session }`, mark `timed_out`, and the
resulting `TurnFinished { "cancelled" }` closes the run as `timeout`.

The user may open a schedule's session in any UI and steer or chat; it is
a session. A human turn in flight when the fire comes is `busy` like any
other; `/schedules run <name>` while busy answers `busy`.

### 4.1 Delivery

```rust
let report = match (stop_reason, quiet, messaged) {
    ("error" | "timeout", ..)  => Some(format!("schedule {name} {stop_reason}: {text_or_last_error}")),
    ("cancelled", ..)           => None,                    // a human did that
    (_, false, false) if !text.trim().is_empty() => Some(text),
    _                           => None,                    // quiet, or message_user already spoke
};
if let Some(text) = report {
    match user.ask(Deliver { m: Outbound { sessions: vec![session], origin: format!("schedule {name}"), text, files: vec![] }, via: None }).await {
        Ok(where_) => tracing::info!(%name, ?where_, "delivered"),
        Err(e)     => tracing::info!(%name, "not delivered: {e}"),   // no channel: the transcript is the record
    }
}
```

`Deliver` runs 24 §2 unchanged: bound pass (a schedule session is never
bound, unless the user bound it by talking to it from a Slack thread —
then it lands there, which is right), else every owner with the `origin`
line. The error text on `error` is the last `AgentEvent::Error { message }`
seen for the session during the run.

## 5. Actor, commands, events (`crates/schedule/src/lib.rs`)

```rust
pub const SCHEDULE_ACTOR: &str = "schedule";

/// `schedule:` config section (§9).
#[derive(Clone, Debug, Serialize, Deserialize)] #[serde(default)]
pub struct ScheduleConfig { pub enabled: bool, pub run_timeout_secs: u64 }   // true, 1800

/// `ask` → `Vec<ScheduleInfo>`: every file in the directory, parsed or not.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSchedules;

/// `ask` → `Result<SessionId, String>`: fire now (`/schedules run`), ignoring `next`.
/// `Err("no such schedule" | "busy" | "disabled" | "<parse error>")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fire { pub name: String }

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScheduleInfo {
    pub name: String,
    /// The front-matter clause as written: `cron 0 9 * * 1-5` / `every 30m` / `at 2026-09-16T15:00`.
    pub when: String,
    /// `croner::Cron::describe()` for cron; the clause itself otherwise.
    pub describe: String,
    pub enabled: bool, pub quiet: bool,
    pub next: Option<DateTime<Local>>,
    pub last: Option<DateTime<Local>>,
    /// `result` of the last completed run (§7).
    pub last_result: Option<String>,
    pub session: Option<SessionId>,
    pub running: bool,
    /// Parse error; such a schedule never fires.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum ScheduleEvent {
    Fired   { name: String, session: SessionId, at: DateTime<Local> },
    Skipped { name: String, at: DateTime<Local>, reason: String },      // "busy"
    Changed,                                                             // directory reconciled
}

/// `ask` → `Vec<RunInfo>`: the last `limit` fires and skips of `name`, newest first (§7).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History { pub name: String, pub limit: u32 }

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RunInfo {
    pub session: Option<SessionId>,
    pub fired_at: DateTime<Local>,
    pub finished_at: Option<DateTime<Local>>,
    /// `end_turn` / `error: …` / `timeout` / `cancelled` / `skipped: busy` / `lost`; `None` while running.
    pub result: Option<String>,
}
```

Internal messages: `Reconcile` (tell, from the watcher task). No `Update`,
`Delete`, `ClearContext`: the file, `rm`, and `/clear` in the session are
those.

`start`: read config → disabled → `Ok(())` with a log line; create the
directory; open `schedule.sqlite` and run the migration (failure → warn,
return `Ok(())`: no schedules rather than no harness); spawn the actor with the subscriber
mailbox; `register(SCHEDULE_ACTOR)`; `rt.subscribe::<Schedule,
AgentEvent>`; spawn the watcher; in a task `lookup_when::<ToolRegistry>`
→ `RegisterTool(ScheduleTool)` + `RegisterPrompt`. Every `agent` ask is a
`lookup(AGENT_ACTOR)` at first use — a fire before the agent is up is
retried next tick. Dependency order: `agent ← schedule ← rpc`, beside
`signal`/`slack`; `schedule` imports `agent`'s messages and events and
`tools`' `Tool`.

## 6. Tool and prompt block (`crates/schedule/src/tools.rs`)

Loops (33 §5) add `until`, `max_runs`, `cwd`, `approvals`, `enabled` to
`ScheduleArgs`, the `schedule_done` and `schedule_status` tools, a paragraph
to the prompt block, and `skills/loops.md`.

```rust
pub struct ScheduleArgs {
    /// File stem: `[a-z0-9][a-z0-9-]*`, ≤ 40 chars. An existing schedule of this name is replaced.
    pub name: String,
    /// Standard cron, local time (`0 9 * * 1-5`). Exactly one of cron / every / at.
    pub cron: Option<String>,
    /// Interval (`30m`, `6h`).
    pub every: Option<String>,
    /// One instant, local (`2026-09-16T15:00`) or RFC 3339. Runs once; the schedule is then gone.
    pub at: Option<String>,
    /// What to do when it fires. Written for a run with nobody present.
    pub prompt: String,
    /// `true`: the run's reply is not sent to the user; only `message_user` reaches them.
    #[serde(default)] pub quiet: bool,
    /// Model for the run; default the harness default.
    pub model: Option<String>,
}
```

`NAME = "schedule"`, `RISK = Risk::Mutating`. The tool writes a file inside
the data dir, which 10 §6 priced `ReadOnly` for `knowledge_put`; the
difference is that this file makes the harness *act later, unattended,
with every tool it has*. That is the model granting itself standing
autonomy, and under the default policy the user should see it once per
grant. Reply: `scheduled morning-brief: cron 0 9 * * 1-5 · next
Wed 17 Sep 09:00 (in 14h)` or the parse error, unchanged from §2.2.
The file is written atomically (temp + rename) so the watcher never reads
half of it; the tool then asks `ListSchedules` for the `next` it reports,
so the answer is what the actor will actually do.

Prompt block (`RegisterPrompt { plugin: "schedule", text: PROMPT }`, one
static paragraph, 00 §4):

> Schedules are markdown files in `~/.theuth/schedules/` (front matter
> `cron:` in local time, `every:`, or `at:`; body = the prompt). Create or
> replace one with `schedule`; read them with `read_file`, list with
> `list_dir`, cancel by deleting the file. An input wrapped in
> `<schedule name=… at=…>` is a timer firing, not a person: nobody will
> answer a question, so act on the prompt as written. Your reply is
> delivered to the user's chat channels when the run ends unless the
> wrapper says `quiet="true"`, in which case only `message_user` reaches
> them — stay silent when there is nothing to say. Use `knowledge_put` for
> anything the next run should know; each run starts with a clear context.

Description of the tool is a pure function of nothing (no clock), so the
system prompt stays cache-stable (07 §2.2.5).

## 7. Data on disk

| path | truth or derived | contents |
|---|---|---|
| `schedules/*.md` | **truth** — the user's files | when + prompt |
| `schedule.sqlite` | derived, beside the directory like `knowledge.sqlite` beside `knowledge/` (00 §9) | one table, every fire and skip |

Loops (33 §8) add a `report` column (migration 0002) and five end-row
`result` strings; see 33 §3.3.

```sql
CREATE TABLE runs (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,            -- file stem at the time; a renamed file is a new name
    session     TEXT,                     -- the schedule's session; NULL for a skip before the first fire
    fired_at    TEXT NOT NULL,            -- RFC 3339, local offset
    finished_at TEXT,                     -- NULL while running
    result      TEXT                      -- end_turn | error: <message> | timeout | cancelled | skipped: busy
);
CREATE INDEX runs_name ON runs (name, fired_at DESC);
```

`sqlx` sqlite with `sqlx::migrate!`, as the agent's store (00 §5); one
migration. Everything the actor needs at boot is a query over this table,
so there is no second table to keep consistent with it:

- `session` of a schedule = `session` of its latest row;
- `last` (for `every` and `missed: run`) = latest `fired_at` where
  `result` is not a skip;
- `last_result` = `result` of the latest row that is not a skip.

Writes: one `INSERT` per fire and per skip, one `UPDATE … SET finished_at,
result` per finish. A row still `NULL` at boot (the harness died mid-run)
is closed as `lost` on load, like 14 §1.2's job exit code. Skips are rows
because "did it run overnight, and if not why" is the question this table
exists to answer; the session cannot show a fire that never entered it.

No pruning: a five-minute monitor writes 105 k rows a year, which sqlite
does not notice. `ponytail:` cap per name if `/schedules` ever measures
slow. Deleting the file loses: which session each schedule uses (the next
fire creates a new one; the old stays in `/sessions`), `last` (so
`missed: run` cannot detect a miss and `every` schedules start from now),
and the history. Deleting a schedule's `.md` keeps its rows: the history
of a schedule that existed is still history; `list_schedules` shows files.

Rejected: a `.state.yaml` in `schedules/` (earlier draft) — no history,
skips and errors left no trace, every write woke the directory watcher,
and a dotfile in the user's directory is a file the user deletes; a table
in `sessions.sqlite` (the agent's store, only the agent writes it, 00 §5);
writing state into the front matter (the user's file, watcher storm).

## 8. Surfaces

- **TUI**: `/schedules` → `Entry::Info` table: `name · when · next · last
  result · running`. Parse errors show in place of `next`. `/schedules run
  <name>` → `Command::FireSchedule`; `/schedules history <name>` → the
  last 20 rows as `fired · took · result`, `Command::ScheduleHistory`.
  The fire card: `injected()` gains a
  `"schedule"` arm (key `name="`), rendering as `◷ morning-brief · 09:00`
  with the prompt body collapsed like a `⟵` card; the run's text streams
  below it as any turn. No status-line badge: a running schedule is a
  running session, and the roster already shows those.
- **RPC**: `list_schedules() → Vec<ScheduleInfo>`, `fire_schedule(name) →
  SessionId`, `schedule_history(name, limit) → Vec<RunInfo>`.
  `ScheduleEvent` on `subscribe_events` (`forward!`, `export.rs`);
  `ScheduleInfo`/`RunInfo`/`ScheduleEvent` in `bindings/` via `just ts`.
- **Desktop** (`crates/desktop/ui`, 25 §3 parity): **Settings › Schedules**,
  beside Skills and Tools — harness-wide state, so not a session menu.
  Table from `list_schedules`: name · `describe()` line · next (relative,
  future-aware: 25's "next_run said now" was a jarvis bug) · last result ·
  a pulsing dot while `running`. A row expands to its last 20 runs
  (`schedule_history`): fired · duration · result, each opening the
  session. Row click opens the schedule's session
  (`/c/<session>`); a row without a session yet is not a link. **Run** →
  `fire_schedule`, then navigate to the returned session; `busy` /
  `disabled` / parse errors surface as the tool would say them (toast).
  Parse errors render in the `next` column in red with the text. No
  create/edit form: the file is the editor, and the model has `schedule`;
  the tab's footer names the directory. Refresh: invalidate the query on
  `ScheduleEvent::{Changed, Fired, Skipped}` and on `TurnFinished` for a
  session the table shows. The `◷` fire card: the TS `injected()`
  classifier (`describe.ts`'s sibling, 25 §3 row 3) gains the same
  `schedule` arm as the TUI, so the wrapper renders as a card with the
  name and time, body collapsed, never as a user bubble.
- **Slack / Signal**: nothing new. Replies arrive through `Deliver` with
  the `origin` line exactly as an unbound `message_user` does (24 §2);
  `/schedules` on those channels is not added until someone wants it.

## 9. Config

```yaml
schedule:
  enabled: true            # false: no watcher, no actor, no tool, no prompt block
  run_timeout_secs: 1800   # a run still going after this is cancelled and reported as `timeout`
```

`THEUTH_SCHEDULE__RUN_TIMEOUT_SECS` as everywhere. No `max_concurrent`, no
`timezone`, no `tick`, no heartbeat keys (jarvis: `max_concurrent_jobs`,
`heartbeat_enabled`, `heartbeat_interval_mins`, `heartbeat_run_timeout_mins`,
`[goals].timezone`).

## 10. Not ported, and what replaces it

| jarvis | here |
|---|---|
| `jobs` + `job_runs` tables, 9 migrations, `JobRepo`, `JOB_RUNS_RETAIN` pruning | one `runs` table in `schedule.sqlite`; the file is the job row; no pruning |
| `context_mode` fresh/continuous, `max_context_tokens`, `trim_continuous_context`, `clear_job_context`, orphan-conversation GC | every fire is `/clear`; memory is `knowledge_*` |
| `notify_signal`, `notify_slack`, `notify_on`, `best_job_result_text`, two `OnceLock` late-binds | `Deliver` on `USER_ACTOR`; `quiet: true` |
| `decision_required`, `required_tools`, `job_decision` tool, claim-check hard gate, "narrating a tool call is not a tool call" revise prompt | the reply *is* the delivery; nothing to narrate |
| `phase_profile`, `phase_gating`, `workspace_root`, `model_provider`, `model_endpoint` | `model:` only; the loop has no phases |
| `on_overlap` skip/cancel-previous, `Semaphore(4)`, `skipped: max_concurrent_jobs` | busy → skip; no cap |
| `on_timeout` fail/retry-once, wall watchdog, `truncate_job_conversation_for_retry`, 250 ms "bus settle" sleeps | one `Cancel`, reported; the next fire is the retry |
| `catch_up` + one-shot `missed` rows | `missed: run` (cron); `every` is relative to `last`; `at` in the past is a parse error |
| heartbeat job, preflight/initiative traits, `HEARTBEAT_ALLOWLIST`, sender-string detection | none; write `heartbeat.md` |
| goals briefing jobs, `goals:`/`jarvis:` prefixes, cancel warnings, `jobsConstants.ts` | none |
| task DAG, 14 trait methods, petgraph | none (`todo` is turn state; not a tracker) |
| `SchedulerHandle` (22 methods), `LateScheduler` forwarding handle, `MockScheduler` | one actor, three asks; tests use the parser and `next()` (§11) |
| six agent tools + three MCP mirrors + seven `job.*` RPCs | `schedule` tool; `list_schedules`, `fire_schedule`, `schedule_history` |
| `tokio-cron-scheduler` (engine, job store, metadata store, 6-field UTC) | `croner` (what that engine used underneath) + a 15-line loop |

## 11. Plan and checks

Each step ends with something runnable (00 §12), not a suite. Estimated
crate: `lib.rs` (config, actor, loop, delivery) ≈ 350 lines, `file.rs`
(parse, `When`, `next`) ≈ 120, `store.rs` (open, migrate, four queries)
≈ 60, `tools.rs` ≈ 80.

1. **`file.rs`** — parse + `When::next`. Unit tests, the only permanent
   ones in this doc, because a plausible bug fails them: exactly-one-of
   `cron`/`every`/`at`; `every` below 10 s rejected; `at` in the past
   rejected; `cron` next across a DST boundary in `America/Los_Angeles`
   lands on the wall-clock hour; `every` next from `last`, and from
   `None` is `now`; a schedule with `error` has `next: None`.
   Check: `cargo test -p schedule`.
2. **Actor + loop + reconcile + store.** Check, headless on a scratch
   data dir: write `every: 20s` with body "say the time" → within 20 s a
   session titled by the stem appears in `list_sessions`, `Fired` on the
   event stream, the reply streams; a second fire 20 s after the first
   `TurnFinished`; edit the file to `every: 1h` → `list_schedules` shows
   the new `next`; `rm` it → gone from the list, session still in
   `list_sessions`. Restart with `schedule.sqlite` present → same session
   id reused; `schedule_history` lists both fires and the `skipped: busy`
   row from a fire forced during a run. Delete `schedule.sqlite`, restart →
   new session on next fire, empty history.
3. **Delivery.** With the Slack app of 24 §9: a `cron` schedule fired
   with `fire_schedule` lands in the owner DM as `_schedule <name>_` +
   the reply; the same with `quiet: true` and a prompt that calls
   `message_user` delivers once; `quiet: true` with a prompt that says
   nothing to report delivers nothing; a prompt that makes the model
   exceed `run_timeout_secs: 30` is cancelled and the DM says
   `schedule <name> timeout`.
4. **Tool + prompt block.** From a TUI session: "every weekday at 9
   brief me on my calendar" → one `schedule` call, approval prompt,
   `~/.theuth/schedules/<name>.md` exists with `cron: "0 9 * * 1-5"`, reply
   names the next fire; "remind me in 20 minutes to stretch" →
   `at:`, the file disappears after it fires, the reminder arrives;
   "cancel it" → `shell rm`. `/prompt` shows the block.
5. **TUI + RPC + desktop.** `/schedules` table on a pty, `/schedules run`
   fires, the `◷` card renders on the fire, the `◆ you` echo is
   suppressed; `cargo check -p rpc --features ts` exports `ScheduleInfo` /
   `ScheduleEvent` and `just ts` lands them in `bindings/`; in the browser
   build, Settings › Schedules lists the scratch dir's files with the
   parse error of a deliberately broken one, Run opens the session as it
   streams, and the fire shows as a `◷` card, not a bubble.
6. **Suspended clock** (manual, once): `every: 5m`, close the laptop for
   15 minutes, open it: exactly one fire within a minute of wake.
7. Docs: §13 here; 00 §10 line, §12 phase 21 row, §13 returned-as row;
   `plugins.md` gains nothing (this crate follows it); README config
   bullet and crate list; `DEFAULT_CONFIG` section.

## 12. Amendments to 00

- §10 workspace: `schedule/  plugin: schedules/ directory, croner tick,
  one session per schedule, reply delivery (34)`; dependency order gains
  `schedule` in the set beside `signal`, `slack`, `nodes`.
- §9 config / data dir: `schedule.sqlite` joins `knowledge.sqlite` in the
  list of derived files beside their directories ("delete to rebuild" does
  not apply: it is history, not an index; deleting it forgets, §7).
- §10 dependency table: `croner 4 (chrono) — cron parsing, next occurrence
  in a chrono `TimeZone`, `describe()` (34)`, `humantime 2 — `every:`
  durations (34)`.
- §12 phases: `21 | schedule (34) | an `every: 20s` file fires a titled
  session twice; a `cron` reply lands in the owner DM; a laptop-sleep
  fire arrives once on wake | —`.
- §13 returned-as: `scheduler | `schedule` crate: files, tick, one
  session per schedule, `Deliver` | plugin | 34 (heartbeat, goals
  briefings, task DAG, notify flags, phase plumbing not ported: each is a
  file the user writes or a primitive the harness already has)`.
- `25-desktop.md` §3: one row — `/schedules`, `/schedules run` | Settings ›
  Schedules (`list_schedules` table, Run → `fire_schedule`, row opens the
  session); `injected()` row gains `<schedule>` beside `<skill>`.

## 13. Status

Shipped 2026-09-16: `crates/schedule` (`lib.rs` actor + plugin ≈ 560 lines,
`file.rs` ≈ 400 incl. 10 tests, `store.rs` ≈ 230 incl. 2 tests, `tools.rs`
≈ 110, one migration); `rpc` methods `list_schedules` / `fire_schedule` /
`schedule_history` + `ScheduleEvent` on the stream + bindings; TUI
`/schedules [run|history]` and the `◷` card; desktop Settings › Schedules
(`Schedules.tsx`, classifier arm, `events.ts`, `reduce.ts`); `theuth` plugin
vector + `DEFAULT_CONFIG`; README; 00 §4/§9/§10/§12/§13; 25 §3. Gates:
`cargo clippy --workspace --all-targets -D warnings`, `cargo test` on
`schedule`/`agent`/`tui`/`rpc`/`theuth`, `just ts-desktop`, desktop `tsc -b`
and vitest (96) all green. Doc number: 33 was taken by `33-loops.md` an
hour earlier; the user chose to build this and fold loops in later.

Deviations from §§1–9:

- `ScheduleInfo.describe` (croner's English line) is a field beside `when`;
  the TUI/desktop show it without re-parsing.
- `NextWake` (internal) replies `Option<Duration>`, and **includes entries
  whose run is in flight** — the first build excluded them, so a 20 s
  schedule whose run finished in 2 s slept until the 60 s ceiling (found in
  smoke: fires at :08, :08+60, :08+120).
- **§1.5 / §4 step 1 as built: a fire that comes due mid-run is deferred,
  not skipped.** `Tick` leaves running entries out of `due`; the entry stays
  overdue (the loop ticks at its 1 s floor meanwhile) and fires on the first
  tick after `TurnFinished`. Only a forced `Fire` (`/schedules run`,
  `fire_schedule`) on a running schedule writes `skipped: busy` and
  publishes `Skipped`, and that skip advances `next`. Reason kept: the
  five-minute monitor whose run took seven then runs at 7, not 10 — an
  interval measured from `last` already says so; and 33 (loops) gets
  back-to-back for free.
- Tick sleep floors at 1 s (a fire that keeps failing does not spin); no
  initial `Reconcile` tell — `load` builds the set before spawn.
- Broken files list `enabled: false`, `when`/`describe` = `—`.
- Tool reply for `every:` says `next: now (first fire within a minute)`.
- **`agent` fix, not in this doc's plan**: `RenameSession` now tells the
  live session `Titled`; before it, `Session::start_turn` overwrote any
  pre-first-turn title with the input placeholder (the first smoke showed
  the schedule session titled `<schedule name="tick" at=…`). Affects every
  renamed-before-first-turn session, desktop "rename chat" included.
- `write_atomic` uses `<dir>/.<name>.md.tmp` (dotfile → invisible to the
  loader) then rename.

Smoke (headless, `claude-sonnet-4-5`, `/tmp/theuth-sched`, `RUST_LOG=schedule=debug`):

- `every: 20s` file → session created, titled (after the agent fix; the
  pre-fix session kept its wrapper title), run `end_turn` in ~2 s, `Fired`,
  history row; cadence `:27 · :47 · :07`; `broken.md` (`0 9 * * 8`) listed
  with `cron: Component error: Number out of bounds.` and never fired.
- Restart on the same data dir → same session id, `Compact { Clear }` on
  it (`epoch=4 style=clear` in the log), the run killed by the stop closed
  as `lost`, the overdue `every` fired at boot.
- `at:` file added while running (hot reload) → fired at its instant,
  file removed, session titled `tock`.
- `fire_schedule` twice within a run → session id, then `busy`; the skip
  is a `skipped: busy` history row.
- Model with approvals off: "create a schedule named demo-cron every
  weekday at 9am" → exactly one `schedule { name, cron: "0 9 * * 1-5",
  prompt }` call, reply `scheduled demo-cron: cron 0 9 * * 1-5 · At 09:00,
  on Monday, … Friday. · next Thu 17 Sep 09:00 (in 21h 17m)`, file on
  disk, listed with `next 2026-09-17T09:00:00+01:00`.
- Delivery: no channel in the scratch dir → `not delivered: signal failed:
  … no owner configured; slack failed: … owner DM not opened yet` at info,
  run recorded `end_turn`.

Not exercised: delivery landing in a real owner DM (24 §9's Slack app was
not pointed at the scratch dir), `quiet: true` suppression vs `message_user`
de-dup (the one-shot had `quiet: true` but nothing could have delivered
anyway), `run_timeout_secs` cancel → `timeout`, `missed: run` across a
restart, a `cron` fire at its instant (only `next` computed), the TUI on a
pty (`/schedules` parsing and tables unit-tested; card arm tested),
Settings › Schedules in a browser (tsc + vitest only), laptop sleep (§11
step 6), the `◷` card in either UI on a live fire.
