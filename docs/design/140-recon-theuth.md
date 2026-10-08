# Recon: theuth substrate for an autoresearch loop (recon-theuth-ar)

Date: 2026-10-08 03:40 +01:00. Repo: ~/Develop/theuth at local main 17769a5c unless a line says `origin/main` or `137-worktree`
(~/Develop/theuth-137, branch feat/137-endless). Paths are repo-relative. Each claim is [fact] (read in the tree tonight) or
[inference]. Read-only recon; nothing built or run.

## 1. Entry & lifecycle

**How a long-running loop is started today**
- The only built "run again until done" primitive is a **schedule file with a stop**: `until:` and/or `max_runs:` on a 34 file;
  `every: 0s` is allowed only with a stop (`every: 0s needs until or max_runs`) and the 10 s floor (`MIN_EVERY`) is lifted only
  then — crates/schedule/src/file.rs:14, 429, 481-483; design docs/design/33-loops.md §0.1-0.3 [fact]. The model writes it with
  the `schedule()` tool (Mutating), or proposes it as a card via `routine_draft` → `[app routine-drafts …]` press → `schedule()` —
  crates/app/src/routine_draft.rs:157-237 (via 136-recon §G) [fact].
- `until: <cmd>` runs **before each fire** in `cwd`: exit 0 ends the loop, exit 1 fires with the 2 KiB tail inside `<until
  exit="1">`, any other exit = `until broken: exit N` and the loop ends — crates/schedule/src/lib.rs:1067-1109, 1080-1082 [fact].
  The run can also end the loop itself with `schedule_done{why}` (ReadOnly) — crates/schedule/src/tools.rs:300-337 [fact]. Ending
  is consumption: the file is deleted, the last `runs` row says why, the user is told through `Deliver` regardless of `quiet` —
  docs/design/33-loops.md §0.4 [fact].
- `enabled: false` is the draft; flipping the line (edit, `/schedules resume`, desktop toggle) starts it; `enabled: false` again
  pauses after the iteration in flight — docs/design/33-loops.md §0.6; `set_enabled` in file.rs (33 §11) [fact]. The live harness
  has one such loop file right now, paused: `~/.theuth/schedules/sre-team-supervisor.md` (`every: 15m`, `quiet`, `run_timeout:
  5m`, `approvals: false`, `agent: theuth`, `runtime: keep`) — live file lines 1-10 [fact].
- Other entries that are *not* loops: `agent:` on a schedule makes each fire a **task** owned by a roster agent rather than a
  session (crates/schedule/src/file.rs:322-346 via 136-recon §G); tasks (`task_create` state `todo` wakes the owner once; a
  `backlog` task runs nothing) — prompt "Tasks" section; desks/agents are standing roles woken by tasks/schedules (design 67)
  [fact]. None of these re-prompt themselves after a turn ends (see §2).
- Chat slash commands: `/schedules [run|pause|resume|history]` exists (33 §0.6-0.7); no `/loop` or `/research` command exists —
  grep for `autoresearch|experiment` in docs/potential-features.md and docs/design-gaps-expanded.md: no matches [fact].

**Per-session mode switch — the pattern an "autoresearch mode" would copy**
- Private sessions (51): the root is opened with explicit storage `retention: durable|private` in the create options; `Retention`
  lives in `tools` and is carried on `NewSession`/`Open` — crates/rpc/src/api.rs:519, 2062, 2114; crates/agent/src/lib.rs:918,
  1026, 1899 [fact]. Private sessions leave no durable rows (crates/agent/src/store.rs:466 via 136-recon §F) [fact].
- 137 ENDLESS generalises this: a `sessions.memory = epochs|endless` column chosen at creation (`create_session.options.memory`, a
  New-chat ∞ switch beside the Private picker, an agents-file field `memory: endless`), flippable on an **idle** session with
  `/endless on|off`; config section `agent.endless` — 137-worktree docs/design/137-endless.md D1 (§0), §8.6, §11.1 [fact]. Status
  tonight: design a0046c22 + Revision 1 9bf9f6cd; W1 skeleton commit 3955ab2b (03:30), W2/W4 started from it (03:32), W3 UI
  as-built 9e3e2b75 (03:40); `crates/agent/src/endless/{compactor,fold,guards,pump,tree,units,tests}.rs` + `compact_prompt.txt`
  exist in ~/Develop/theuth-137-w1 — `git log` of the worktrees, read 03:45 [fact]. Not on main or origin/main [fact].
- So the precedent is: a column on `sessions` + a create-option + a slash command for idle sessions + an `agent.<mode>` config
  section + a per-session actor message (`SetX` in agent/lib.rs routed by `Agents` to `loop_::SetXNow`, the shape
  `SetToolset`/`SetBudget`/`ResetRuntime` already use — crates/agent/src/lib.rs:482-490; 136-worktree crates/agent/src/lib.rs:470,
  loop_.rs:355-358, supervisor.rs:3126) [fact]; an autoresearch mode fits the same seam [inference].

**Stop**
- 133 STOP-SCOPE (contract 2026-10-07, as-built 133-as-built.md): `Cancel{session, keep, scope: Root|Tree}`; **Root is the default
  everywhere** and touches one session; Tree is the old cascade bought by hold-Esc / triple-tap; children a root stop leaves alone
  keep working and their later `TurnFinished` lands in the parent's inbox — docs/design/133-stop-scope.md §1-§2 [fact]. The
  schedule's `cancel_tree` (deadline/cancel) is explicitly `Tree` — docs/design/133-stop-scope.md §3 table;
  crates/schedule/src/lib.rs:1447, 1625 [fact].
- Deadline: a run's `deadline` = per-file `run_timeout:` else live `schedule.run_timeout_secs` (default **1800 s**); the tick
  cancels the tree past it and the history row says `timeout` — crates/schedule/src/lib.rs:53-60, 1003, 1275, 1441-1447 [fact].
- Implication: today an autoresearch iteration that spawns workers and is stopped by a single tap keeps its workers alive (desired
  for "stop the planner, let the benchmark finish" — but there is no "stop at the next ratchet boundary" semantics) [inference].

## 2. Run model

- **Sessions.** One row per session; roots vs children derived (`parent`), `unattended = 1` for schedule-opened roots, `desk` for
  agent desks, `kind` only `chat|advisor` — crates/agent/migrations/0022_unattended.sql:1-3, 0024_desk.sql:1-6;
  crates/agent/src/lib.rs:1837-1843 (via 136-recon §F, spot-checked table list) [fact].
- **A schedule has one session, reused on every fire.** Created `NewSession{unattended:true}`; before each fire of a clock-only
  file the frame is cleared with `Compact{style: Clear}` and, unless `runtime: keep`, the Python runtime is reset with
  `ResetRuntime` (131) — crates/schedule/src/lib.rs:1189-1215 (Compact at :1196, ResetRuntime at :1208),
  docs/design/131-runtime-reset.md D1-D2 [fact]. Why 131 exists: an `every: 0s` / `max_runs: 800` loop reached 144 runs, 887
  names, a 168 MB runtime dump that crossed monty's `MAX_MEMORY`, and the dead dump was restored every turn —
  docs/design/131-runtime-reset.md "Problem" [fact]. `DUMP_HARD_BYTES = 64 MiB`, `DUMP_SOFT_BYTES = 32 MiB` — same doc D6 [fact].
- **The Run lifecycle.** `struct Run { …, deadline: Instant, children, pending, turn_open, last_stop }` —
  crates/schedule/src/lib.rs:805-823. A run with no children closes at its `TurnFinished`; one that fanned out **settles**:
  `Settle` is self-told after `SETTLE = 500 ms` and closes only when no turn is open, `pending` is empty and `ListAgents` shows no
  live descendant — lib.rs:845-849, 2088-2089, 2117-2128; design 33 §3.4 [fact]. The run's close self-tells `Tick`, so the next
  `every: 0s` iteration fires within ~1 ms — 33 §11 [fact]. A busy session at fire time → `skipped: busy` row — lib.rs:261,
  1046-1049 [fact].
- **Prompt delivery.** `<schedule name=… at=… quiet=… [run=k of=N]>` + optional `<until>` + body + optional `<when>` —
  lib.rs:1095-1116 (via 136-recon §G) [fact]. The run's reply is `Deliver`ed to the user's channels unless `quiet`; `message_user`
  still reaches them [fact, prompt "Schedules" paragraph].
- **What re-prompts a session after it ends a turn.** (a) The schedule tick (loops) — above. (b) A child's final message
  (`<message from=…>`) / `<subagent_result>` starts a new parent turn (01 §2.8, cited by 133 §1) [fact]. (c) `<job_result>` /
  `<job_idle>` from shell/PTY jobs, `<reminder>` from `remind_me_in` (reminders are rows in `reminders(id, session, message,
  set_at, fires_at, seconds)`, `tools.reminder_max_secs` 86 400) — crates/agent/migrations/0019_reminders.sql:5-12;
  crates/tools/src/lib.rs:474-475, 496 [fact]. (d) Peer messages (50/62) and task wakes for desks (67/70). (e) 136 dreaming's run
  is a routine fire; 137's pump is a background compactor, not a re-prompt [fact]. There is **no built "continue until done" turn
  re-entry inside one session other than reminders** — a Karpathy-style `while True` must today be either one very long turn
  (bounded by `agent.max_tool_rounds`, live 384 — ~/.theuth/config.yaml:7) or a schedule loop of fresh-context iterations
  [inference from the above].
- **Durability (109).** Jobs are rows (`jobs(session_id, id, kind, call_id, command, name, pgid, log, started_ms, state, exit,
  settled_ms)`), with `turn_steps`, `tool_calls`, `questions`, `waits`, `resume_reports(…, lost_jobs, children_restored,
  children_handed_off)` — crates/agent/migrations/0033_durability.sql:10-112, 0035_resume_reports.sql:7-18 (via 136-recon §F)
  [fact]. A harness restart still **kills every shell() job** (`JobExit::Lost`: "SIGKILL leaked it; its exit code is unknown") —
  crates/tools/src/jobs.rs:4, 86-96 [fact]; knowledge `facts/theuth-runtime-quirks-2026-09-24` ("a harness restart kills shell()
  jobs and reminders survive it") [fact].

## 3. Storage

- **Data dir** `~/.theuth`: `config.yaml`, `prompt.md`, `sessions.sqlite`, `schedule.sqlite`, `knowledge.sqlite` + `knowledge/`
  (git bundle), `artifacts.sqlite` + `artifacts/`, `tasks.sqlite`, `roster.sqlite`, `agents/`, `schedules/`, `sensors/`, `skills/`
  + `skills.sqlite`, `decisions/`, `documents/`, `spill/`, `runtime/`, `jobs/`, `sandbox/`, `mesh/` + `mesh.sqlite`,
  `replica.sqlite`, `logs/`, `bin/build-lock`, `briefs/` (orchestrator convention, not a plugin) — `ls ~/.theuth` 03:45 [fact].
- **sessions.sqlite**: 36 forward-only migrations on main (0001…0036_double; 137 takes 0037) — crates/agent/migrations/ (via
  136-recon §F; knowledge `facts/theuth-overnight-2026-10-08-landing-order-and-main-state`) [fact]. Tables relevant to an
  experiment ledger: `messages(session_id, seq, body, excluded, …)`, `epochs`, `todo_snapshots`, `model_calls(call_id,
  owner_id→cost_owners, kind, input/output/cache tokens, returned_nanos, estimated_nanos, …)`, `cost_owners(id, root_id,
  parent_id)` with **no FK to sessions on purpose** (deleting a conversation never deletes spend), `remote_meters`/`remote_turns`
  (external harness money/tokens), `jobs`, `tool_calls`, `turn_steps`, `questions`, `inner_calls` (tool calls made from python
  cells), `reminders` — crates/agent/migrations/0015_session_lifecycle.sql:7-66, 0020_inner_calls.sql:7-17,
  0033_durability.sql:10-112 [fact]. **No `events` table**: `AgentEvent`s are a live bus — 136-recon §F [fact].
- **Artifacts** (35): `artifacts.sqlite` + files; `show{content,name}` writes versioned files; `artifact_read` pages them; a
  markdown artifact names a document id [fact, prompt]. **Documents / notes / boards** (`crates/document`): block documents with
  stable block ids, `doc_create(kind="board", root=<repo>)`, `doc_insert/replace/move/delete`, exported by `doc_export` — prompt
  "Documents"/"Boards"; `crates/desktop/ui/src/components/board/{BoardReader,Sections,Tour,anchors,units}.tsx` [fact].
- **Knowledge** (`crates/knowledge`, gix-backed bundle with history: `knowledge_history`, `knowledge_get(id@commit)`,
  `knowledge_restore`) — crates/knowledge/Cargo.toml:10 (`gix`), crates/knowledge/src/tools.rs:331-376, 151-161 (via 136-recon §G)
  [fact]; bodies capped at 16 384 bytes — knowledge `facts/theuth-runtime-quirks-2026-09-24` [fact].
- **Schedules**: files in `~/.theuth/schedules/*.md`; run history in `schedule.sqlite`: `runs(id, name, session, fired_at,
  finished_at, result, report /*final text ≤ 2 KiB*/, envelope)`, `watch(…)` — crates/schedule/migrations/0001_runs.sql:1-9,
  0002_report.sql:1, 0003_watch.sql:2-16 (via 136-recon §G) [fact].
- **How a plugin owns on-disk data**: each plugin has its own sqlite beside the shared one (`schedule.sqlite`, `tasks.sqlite`,
  `roster.sqlite`, `knowledge.sqlite`, `artifacts.sqlite`, `mesh.sqlite`, `link.sqlite`, `secrets.sqlite`, `skills.sqlite`) plus a
  directory (`decisions/`, `double/double.sqlite` + `double/runs/<id>/epoch-<n>.safetensors`) — `ls ~/.theuth`;
  crates/double/migrations/0001_double.sql:1-94; docs/design/126-double.md:343 [fact]. 136 dreaming follows the same rule
  (`<data_dir>/dreaming/`, a journal document, a ledger) — 136-worktree crates/dreaming/src/{journal,ledger,undo}.rs exist (7 519
  LOC total) [fact].
- **Decision captures** are append-only JSONL: `<data_dir>/decisions/training/<prefix>/<YYYY-MM-DD>.jsonl`, schema
  `theuth.decision.v1` — crates/decision/src/training.rs:1-23 (via 136-recon §H) [fact].
- **Candidates for an experiment ledger** [inference]: (1) a new `<data_dir>/autoresearch/<run>.sqlite` (plugin-owned, like
  `double.sqlite`: `experiments(id, run, parent_commit, commit, metric, kept, cost_nanos, started, ended)`, `ideas`, `notes`) + a
  markdown **journal document** (136's pattern) the UI and `doc_read` can both show; (2) `model_calls`/`cost_owners` joined on the
  loop's session for money per iteration (128 R3 fold already does this per root —
  docs/design/128-as-built-SPEND-LEDGER.md:20-31); (3) the repo itself for the ratchet (git log of the experiment branch, as
  Karpathy's pattern), with the ledger pointing at shas; (4) knowledge concepts only for durable lessons, not per-iteration rows
  (16 KiB cap, hint noise — knowledge `facts/theuth-knowledge-hint-usage-baseline-2026-10-07`: 2.2 % of hints opened).

## 4. Benchmark execution

- **shell() jobs** (14): `shell{command, cwd?, env?, timeout_secs?}` is `Risk::Destructive`; it returns output if the command
  exits within `tools.job_grace_secs` (default **3 s**) else a `{job, log}` stub and the result arrives later as `<job_result>` —
  crates/tools/src/shell.rs:265-270; crates/tools/src/lib.rs:464-468, 492 [fact]. The kill deadline is `tools.job_timeout_secs`
  (default **1800 s** on main; `timeout_secs` on the call can only shorten it: "timed out after 2s (timeout_secs=100 capped to 2);
  process group killed") — lib.rs:493; shell.rs:434-460; jobs.rs:83-84 [fact]. (Knowledge from 2026-09-28 records a 900 s cap at
  that time — `facts/theuth-parallel-waves-cold-gates-and-disk-2026-09-28`; the default on main tonight is 1800 [fact]; the live
  config.yaml sets no `tools.job_timeout_secs` so 1800 applies [fact].) Output to the model is capped at `MAX_OUTPUT` 64 KiB, 8
  KiB tail kept per job, 32 finished rows per session; `SETTLE` 1 s, `KILL_WAIT` 2 s — jobs.rs:42-56 [fact]. A job killed by a
  harness restart is `lost` — jobs.rs:4, 90-96 [fact]. There is **no cap on concurrent shell jobs** in jobs.rs constants (only
  `MAX_ROWS` of finished rows) [fact, by absence]. Consequence: a benchmark longer than 30 min must run detached (`nohup … &`, log
  + EXIT marker) and be polled with `remind_me_in` — the pattern the knowledge base already prescribes
  (`procedures/theuth-remote-build-host-loop`, `procedures/offline-replay-of-a-memory-design-over-theuth-session-logs-2026-10-08`:
  "resumable DETACHED background job with a progress log (it survives a harness restart)") [fact].
- **PTY jobs** (22/61): `start{command, name?, ready?, idle_secs?}` / `send` / `screen` / `wait{pattern, timeout ≤ 120, default
  30}` / `cancel_job`; 120×40 screen (max 400×200), `READY_MAX` 300 s, `tools.pty_idle_secs` default 60 (floor `IDLE_MIN` 10, `0`
  = never) wakes the model with `<job_idle>`, nudges back off to `IDLE_BACKOFF_MAX` 3600 s; 512 rows scrollback; engine-wide
  retained-terminal budget `PTY_BUDGET` 512 MiB ≈ 80 live PTYs — crates/tools/src/pty.rs:38-69; lib.rs:469-473, 494-495;
  jobs.rs:59-62 [fact]. PTYs survive the harness only as rows (`pty_screens`); omp PTYs die on an engine restart — knowledge
  `facts/theuth-build-lock-queued-worker-looks-stuck-2026-10-07` [fact].
- **Sandbox VMs** (79; built — as-builts A…Z2, `crates/sandbox`): `sandbox_create{name, cpus?, memory_mib?, max_*, root_disk_mib?,
  shares?, network?, desktop?}`, `sandbox_share/unshare`, `sandbox_snapshot/restore`, `sandbox_pause/resume/stop/start/destroy`,
  `sandbox_resize` (live within ceilings) — crates/sandbox/src/tools.rs:256, 335, 378, 481, 535-542, 620, 658; `DEFAULT_TIMEOUT`
  600 s for runtime ops — crates/sandbox/src/lib.rs:126 [fact]. A VM is a mesh **engine**: `spawn_subagent{agent:"<vm>"}` runs
  work inside it; snapshots give a clean benchmark baseline per experiment (`sandbox_snapshot` → run → `sandbox_restore`) —
  docs/design/79-sandbox-vm.md "Scope" paragraph; prompt "sandbox VM" paragraph [fact]. Limits: Apple-Silicon/KVM hosts only, no
  GPU, Linux guests only — 79 "Not in v1" [fact]. Build cost inside a VM is uncached unless a shared target dir is mounted via
  `sandbox_share` [inference].
- **Nodes / mesh** (32/67): another theuth engine dialled from `nodes.yaml`, exposed to `agent` as a harness source by
  `RegisterNode` — crates/nodes/src/lib.rs:10-24, 129-136 [fact]; `mesh_peers()` lists engines [fact, runtime index]. A remote
  build host loop is a documented procedure (ssh in a PTY, detached commands with EXIT markers) — knowledge
  `procedures/theuth-remote-build-host-loop` [fact].
- **Machine build protocol**: `~/.theuth/bin/build-lock <cmd>` — a bash wrapper taking one of `BUILD_SLOTS` (default 2; a third
  adaptive slot while 1-min load < `BUILD_LOAD_FLOOR` 16) `mkdir` slots under `/tmp/theuth-build-slots/<i>/owner` (`pid label
  epoch argv`), stale-pid reclaim, 5–10 s polite polling; `BUILD_SLOTS=1` = workspace-class, contends for slot 0 only — live file
  ~/.theuth/bin/build-lock:1-50 (1 939 bytes, 2026-10-07 10:50) [fact]. It is **not in the repo** (grep `build-lock` in
  ~/Develop/theuth hits only design docs 124/126 that invoke it) — [fact]; 126 adopts it for the double's trainer
  (`BUILD_SLOTS=1`, label `double-train`) — docs/design/126-double.md:29, 357 [fact]. Known trap: a worker waiting on a slot looks
  stuck (no process, `wait` as last event) — knowledge `facts/theuth-build-lock-queued-worker-looks-stuck-2026-10-07` [fact].
  `/tmp` slots do not survive a reboot — `facts/theuth-macbook-rebooted-mid-wave-…-2026-10-07` [fact].
- **Resource caps that exist**: `agent.subagents.max_concurrent` 16 / `max_concurrent_external` 32 / `max_runtime_secs` 600 (live
  28 800) / `max_depth` 1 — crates/agent/src/lib.rs:2971-2974; ~/.theuth/config.yaml (`max_runtime_secs: 28800` per 136-recon §K)
  [fact]; monty runtime per-cell `MAX_MEMORY`, dump caps 64/32 MiB, sleep 300 s per call / 600 s per turn —
  docs/design/131-runtime-reset.md D6; knowledge `facts/monty-beta2-sleep-and-stdlib` [fact]. **No CPU/RAM cap on shell jobs, no
  nice/ionice, no per-session job quota** [fact, by absence in jobs.rs].

## 5. Tools

- **Registration**: a plugin is a workspace crate exporting one `runtime::Plugin`; plugins talk only by typed kameo **events** and
  **commands** (`ask` on a registry-named actor); a tool is a `tools::Tool` impl with `const NAME`, `const RISK: tools::Risk`
  ("the ONLY permission input"), a JSON-schema args type and `invoke`, registered with the `tools` registry
  (`ListTools`/`Invoke`/`ListPrompts` messages) — docs/plugins.md:5, 190-198; crates/tools/src/lib.rs:79, 107-113 [fact]. `Risk` =
  `ReadOnly | Mutating | Destructive`; MCP tools get a fourth `off` via mcp/config.yaml — lib.rs:107-141; prompt "MCP" paragraph
  [fact].
- **Direct tools vs runtime functions** (08 §0.4): with the Monty runtime on, only `python` and the tools that talk to the user or
  other agents are direct definitions; data-returning tools are **runtime functions** callable as `await name(...)` inside python;
  `agent.runtime_only: false` makes everything direct — crates/agent/src/lib.rs:2712-2717 [fact]. The runtime (`## Runtime`) index
  is appended to the system prompt by `Turn::rebase` after the plugin blocks — crates/agent/src/loop_.rs:3719-3726 (via 136-recon
  §E, spot-checked order list) [fact]. A new MCP tool added after a session started stays invisible to that session — knowledge
  `facts/theuth-runtime-traps-2026-10-07` [fact].
- **Prompt sections contributed by plugins**: optional per-plugin prompt block (`ListPrompts`), assembled each turn in
  `frame::baseline` in name order and **filtered by the session's `Toolset`**; unattended (schedule) sessions get
  `UNATTENDED_LINE` instead of the surface block and no `app` block — docs/plugins.md:218; crates/agent/src/frame.rs:375-395;
  crates/agent/src/loop_.rs:2878-2899 (via 136-recon §E) [fact]. Design rule: a block is a function of config only (no per-turn
  state), the long tail goes into a skill — docs/design/07-context.md:281-295 [fact].
- **Per-session tool visibility**: `Toolset` (an allowlist of tool names + `<knowledge>` lines per note) with `SetToolset{session,
  toolset: Option<Toolset>}` in memory, built for live meetings (`meetings/06-live-toolset.md`) — crates/agent/src/lib.rs:463-490
  [fact]. 136 §9.4 reuses it to confine the dreaming routine to a bounded tool set so `approvals: false` is safe —
  docs/design/136-dreaming.md:438 [fact]. Subagents: `tools: read_only|mutating` caps a child; above the cap is `denied: subagent
  is capped` [fact, prompt "MCP" paragraph].
- **Skills**: markdown under `~/.theuth/skills/`, `always:` skills inlined, others listed in `## Skills` and loaded with
  `load_skill`; plugin-bundled skills (`crates/schedule/loops.md` via `include_str!`, written by `first_run`) —
  docs/design/33-loops.md §5.3; live `~/.theuth/skills/loops.md` 159 lines, `routines.md` 247, `harness.md` 263 [fact].
- **What an autoresearch plugin would register** [inference]: ≤ 5 tools (`research_start/status/stop`, `experiment_record`,
  `idea_add`) at Mutating/ReadOnly risk, one prompt block (config-only), one skill carrying the protocol (the loop body, the
  keep/discard rule, how to detach a benchmark), and a `Toolset` for the loop's session when it runs `approvals: false`.

## 6. Evaluation & judgement

- **Local decision model (57/92, `crates/decision`)**: `decide{question}` / `decide_many{questions ≤ decision.tool.max_rows 2 000,
  batches, budget_ms 2 000 default}` / `decide_label{log_id,id}`; checkpoints `gliner` (invented labels) and `default` (laya /
  ModernBERT via candle); ~0.1 s and zero API tokens per row; `confidence` is a raw margin — crates/decision/src/tool.rs:307-354,
  271, 312-313 (via 136-recon §H; verified the file exists) [fact]; the `decide` skill carries the recipes [fact]. It picks among
  given labels; "it cannot reason, compute or read long documents" — prompt "Bulk classification" [fact]. Use for autoresearch:
  triaging an ideas backlog or classifying a failed run's log (compile error / test flake / regression) — not for judging a metric
  [inference].
- **Tier-2 escalation**: a typed question the oracle abstains on goes to `agent.escalate_model` (unset → `title_model` → session
  model) in one cheap no-tools side call — crates/agent/src/lib.rs:2737-2740 [fact].
- **Advisor (125)**: a second model session (`sessions.kind='advisor'`, `session_advisors(session, advisor, model, effort, cursor,
  state idle|reviewing|paused|halted)`) reads the transcript delta after each primary step and advises the agent;
  `messages.advisory` flags its injections — crates/agent/migrations/0034_advisor.sql:2-18 (via 136-recon §F);
  docs/design/125-advisor.md:1-30 [fact]. This is the nearest built "second opinion" seam; a reviewer-of-proposals for an
  experiment loop could ride it (advisor reads "I propose change X", answers "already tried / too big / fine") [inference].
- **Double (126, `crates/double`, `double.enabled` default false)**: learns the owner's approval/ask answers (`labels.source`
  direct 1.0, override 3.0, hindsight_* 0.25–0.5), ladder L0 observe → L1 suggest → L2 act with 30 s grace → L3 silent, promotion
  thresholds (≥ 20 rows, acc ≥ 0.90, ECE ≤ 0.05 for L1; ≥ 50, ≥ 0.97, zero overrides in last 20 for L2), hard caps destructive ≤
  L1 — crates/double/src/labels.rs:1-44, ladder.rs:1-42, config.rs:55, 200 (via 136-recon §H) [fact]. Relevance: it is the only
  built mechanism that answers approval cards for an unattended loop other than `approvals: false`; a loop at `approvals: false`
  bypasses it entirely [inference].
- **136 dreaming's evaluation vocabulary** (built in worktree, not on main): **cases** = mined units {intent, context excerpt,
  what happened, user's words, derived ideal, tags, source}, "the user's corrections are the benchmark"; ≤ 60 new cases a night,
  settled after 180 days — docs/design/136-dreaming.md:19, 240 [fact]. **Gate**: an edit to a skill/preferences is replayed over
  held-out cases as a *text replay* and accepted or rejected; what the gate refused goes to a rejected-edit buffer so the same
  edit is not re-proposed — 136-dreaming.md:7, 22, 140-154 [fact]. **judge ≠ proposer**: frontier model proposes, a *different
  frontier vendor* judges when configured; `decide_many` for classification, a mid model for tidying — 136-dreaming.md:24, 185
  [fact]. Evidence behind the gate: skillopt's gated Sleep +3.1…+5.6 points vs **gate off 0.554 → 0.026 in five nights** —
  136-dreaming.md:55 [fact]. Code: 136-worktree crates/dreaming/src/{cases,replay,ledger,undo}.rs [fact].
- **Statistics helpers in the tree**: essentially none for experiment judgement — `percentile` exists only in a bench example
  (crates/decision/examples/decide_bench.rs:64), `mean` in computer/src/pointer.rs:158 (pointer smoothing), `mean_nll` in
  double/src/train_b.rs:562 (training loss) [fact]. No Welch/bootstrap/median-of-N helper anywhere — grep `fn
  (median|percentile|quantile|bootstrap|welch|t_test|…)` over crates [fact]. An autoresearch loop needs at minimum "N repeats,
  median, noise floor, keep only if improvement > noise" — this must be added (or done in the python runtime cell by the model,
  which 131/`inner_calls` already persist) [inference].
- **Judgement rule precedents to borrow**: 136 D6/D11 (gate + different judge), 126's prequential window and
  override-demotes-one-rung (the ratchet as a ladder with evidence), 33's "the run is the judge" (rejected a side-model judge for
  loops — 33 §0.2) [fact].

## 7. Git & workspace

- **gix in the workspace**: `gix 0.87` (features revision, blob-diff, sha1, parallel) used by `crates/codeindex` and
  `crates/knowledge` — Cargo.toml:14; crates/codeindex/Cargo.toml:20; crates/knowledge/Cargo.toml:10 [fact].
  `codeindex/src/git.rs` has `Git{repo, prefix}`, `Range::{Worktree, Commits{old,new}}`, `Span`,
  `Status::{Added,Modified,Deleted}`, `Change{path,…}`, `hunks(old,new)` — git.rs:22-69, 382 [fact];
  `code_impact(git_range|files)` and `code_map(mode="changed")` diff HEAD vs worktree or a commit range —
  crates/codeindex/src/tools.rs:714, 873, 1025 [fact]. Knowledge uses gix for its bundle history (`knowledge_history`, `@commit`
  reads, `knowledge_restore`) [fact]. **No git write path exists in Rust** except `tools/src/git.rs` (clone/pull for MCP and skill
  repos — git.rs:78-101) and `skills.rs`'s test helper (`git worktree add` in a test — crates/agent/src/skills.rs:1155-1158)
  [fact]. Commits, branches, worktrees, resets are done by the model through `shell()` [fact].
- **13 snap is not git**: it is the PNG compaction style (rasterise folded turns) — docs/design/13-snap.md:1-20 [fact]. **29
  rewind is conversation git, not file git**: `cut_at` on a turn start; edit/regenerate destroy the tail, fork copies rows into a
  new session — docs/design/29-rewind.md §0.1-0.4 [fact]. Neither tracks or reverts files on disk [fact].
- **Per-session file-change tracking**: `tool_calls.files TEXT DEFAULT '[]'` (109) records the files a tool call touched; the
  compaction head carries a `Files written: …` truth line and `files_written` state is cleared at the fold —
  crates/agent/migrations/0033_durability.sql:41; crates/agent/src/loop_.rs:6111-6199, 6254-6260 (via 136-recon §C) [fact]. There
  is **no "revert this session's edits"** — 133 is about stopping turns, 29 about messages; the only revert is the model running
  `git checkout`/`git stash` itself [fact, by absence]. The t3code borrow list already names "per-turn hidden-ref git checkpoints
  with changes since turn N / revert" and "engine-managed worktrees (workspace as a session field, cleanup policy, overlap guard)"
  as *next* items — knowledge `facts/theuth-borrow-list-from-t3code-2026-10-03` [fact].
- **Worktree conventions (practice, not code)**: one worktree per wave/worker `~/Develop/theuth-<n>[-w<k>]` on `feat/<n>-…`,
  integration branch per design, cut from `origin/main` when local main is behind, a separate `CARGO_TARGET_DIR` per worktree
  (APFS-cloned from a warm one), deleted at merge; 27 worktrees exist tonight — `git worktree list` 03:45; knowledge
  `procedures/theuth-design-written-by-orchestrator-then-parallel-omp-wave-overnight-2026-10-08`,
  `procedures/theuth-warm-worker-target-dirs-by-apfs-clone-2026-10-07` [fact]. A `git worktree add` from a session is classed
  `approval:shell:mutating:git worktree:root` by the double — crates/double/src/class.rs:321-325 [fact].
- **What the ratchet would need** [inference]: (a) an experiment branch + one commit per kept change (model-driven `shell()` git,
  as Karpathy's loop), (b) `git reset --hard <last_good>` on discard — both exist as shell; (c) a Rust-side read of the branch log
  for the UI/ledger is one gix call away (`codeindex::git::Git` already opens the repo); (d) an engine-managed worktree per
  parallel variant does not exist and would be new code; (e) `code_impact(git_range="<base>..<head>")` already answers "what did
  this experiment touch" for a reviewer.

## 8. Prompting & context

- **System prompt assembly** (`frame::baseline`, once per turn in `Turn::rebase`): identity line → persona header + `Working
  directory` + `Date` + `Preferred subagent model` → `prompt.md` (omitted for agents with an identity; `<!-- root-only -->` spans
  stripped for unattended sessions) → `always` skills → `## Skills` index → `## Peers` → plugin prompt blocks (name order,
  Toolset-filtered; unattended = `UNATTENDED_LINE`, no `app` block) → `ADVISORY_PARAGRAPH` → `## Guidance` → `## Runtime` index —
  crates/agent/src/frame.rs:375-395, 348-357, 95-118, 33-82; crates/agent/src/loop_.rs:3676-3738, 2878-2899 (via 136-recon §E)
  [fact]. **No token budget** on the system prompt — docs/design/07-context.md:281-295 [fact]. `<knowledge>` hints are a tail
  *note* (user-role message before the model call), one of nine note kinds (context_pressure, todo, todo_continue, knowledge,
  recovery, continue, steer, rounds, surface_error) — docs/design/07-context.md:745-783; crates/agent/src/loop_.rs:4926-4973 (via
  136-recon §E) [fact]. 136 adds a `<budget spent cap>` note kind and a ≤ 20-line standing-preferences block —
  docs/design/136-dreaming.md:309; 136-contract.md:89 [fact].
- **Compaction (07 §8, 11, 13)**: styles `summary` (default; cache-hot LLM fold with `## Goal / Done / Open / Next / Notes`
  headings), `cut` (drop span, no model call), `snap` (PNG frames, needs a vision model, auto falls back to `cut`), `clear`;
  trigger `agent.compact_at` **0.8** of the model window; ceilings `tail_tokens 40k, image_tokens 30k, target_tokens 120k`; head
  `<context_summary epoch=n>` carries harness-truth lines (roster, todo, `Runtime:` inventory, `Knowledge: read`, `Jobs:`, `Files
  written:`, `Earlier turns verbatim: <spill path>`); loaded skills are **cleared at the fold** —
  crates/agent/src/lib.rs:2727-2730, 2777; crates/agent/src/compact.rs:557-568, 13-35; crates/agent/src/loop_.rs:6111-6199,
  6254-6260 (via 136-recon §C) [fact]. Live: `compact_style: snap` — ~/.theuth/config.yaml:10 [fact]. Epochs are rows
  (`epochs(session_id, n, at_seq, tail_from, head, …)`) plus spill files swept after **7 days** —
  crates/agent/migrations/0003_epochs.sql:1-9; crates/agent/src/spill.rs:146-148 [fact].
- **Todos (47)**: `todo_snapshots(session_id, at_seq, k, source, items)`; todo lines ride in the compaction head and
  `todo`/`todo_continue` notes nudge the model to keep going — crates/agent/migrations/0009_todos.sql:7-16; note kinds above
  [fact]. This is the built "don't stop until the list is done" nudge; it is per-turn, not cross-run [fact].
- **Turn limits**: `agent.max_tool_rounds` default 32 (live **384**), `stream_idle_secs` 120, `max_salvages` 2,
  `max_tool_output_chars` 32 000 — crates/agent/src/lib.rs:2770-2773; ~/.theuth/config.yaml:7 [fact]. Monty sleeps: 300 s per
  call, 600 s cumulative per turn — knowledge `facts/monty-beta2-sleep-and-stdlib` [fact].
- **What happens to a session that runs 500 turns**: `messages` rows grow unbounded (roots of 4 245 and 2 310 rows exist —
  knowledge `facts/theuth-desk-session-log-shape-and-volume-2026-10-08`); every time the frame nears 0.8 × window the fold
  produces a new epoch; summaries compound (a summary of a summary), loaded skills vanish at each fold and must be reloaded;
  knowledge hints re-fire per turn (2.2 % opened — `facts/theuth-knowledge-hint-usage-baseline-2026-10-07`); the python runtime
  heap grows until 131's caps (64 MiB dump) unless reset; spend accrues under one `cost_owners.root_id` [fact/inference]. The
  schedule-loop alternative (fresh context each iteration) is why 33 exists; it trades memory for the `<until>` tail + whatever
  the run re-reads from disk [fact].
- **137 ENDLESS** (the third option, in flight tonight): per-session `memory: endless` — every turn's first request is `tools ·
  system (byte-identical) · user[view blocks · input]`, **no carried history**; a background compactor summarises each turn's
  units into a tree of ≤ 512-byte lines (`endless_nodes`), the view is folded to `view_bytes` 64 000 with slack 0.125;
  auto-compaction is off in that mode; measured 84–96 % of view bytes identical turn to turn; `/endless on|off` on idle sessions;
  children keep their own mode and reach the parent as a `work` unit; `endless_calls` rows are `model_calls` kind `endless` —
  137-worktree docs/design/137-endless.md D1–D6, §8.1-8.7 [fact]. Build status 03:45: W1 skeleton + migration 0037 committed
  (3955ab2b), W2/W4 branched from it at 03:32, W3 UI as-built at 03:40; integration not yet done; lands after 134 W5 → 136 → 138
  per the agreed order — worktree git logs; knowledge `facts/theuth-overnight-2026-10-08-landing-order-and-main-state` [fact]. For
  autoresearch: an endless root session is the natural home for a multi-day loop's *planner* (durable memory of what was tried
  without compaction cliffs), while each experiment runs in a child or a schedule iteration [inference].

## 9. UI

Surfaces a live experiment dashboard would copy (all under `crates/desktop/ui/src/`, React + TanStack Query over the RPC bindings
in `rpc/bindings/`; bindings are generated by `just ts-desktop`):
- **Subagent board / strip (19, 112)**: `components/chat/SubagentStrip.tsx` — one dot per live subagent of the open session (any
  depth), blinking on `AgentInfo.beat`, green / amber-when-blocked, click opens the child's chat or drags it into a pane —
  SubagentStrip.tsx:11-16, 41-60 [fact]. Pattern: derive rows from the session tree the client already holds; no new RPC [fact].
- **HUD scene (128)**: `components/hud/{Hud.tsx, model.ts, queries.ts, scene/{Scene.tsx, SceneCompact.tsx, model.ts,
  fixtures.ts}}`; attention kinds `approval | decision | hire | blocked | review | budget | node | handoff | secret | ask | show |
  resume | double | finished` — Hud.tsx:162-177; empty-state text names "budgets at 80 %" — Hud.tsx:180 [fact]. **Spend ledger**
  (128 R3): engine `spend_report(days ≤ 31)` folds `model_calls ⟕ cost_owners ⟕ sessions` by root into kinds `agent | routine |
  user | other`, local-day buckets, top 8 owners — docs/design/128-as-built-SPEND-LEDGER.md:16-48 [fact]. An experiment loop's
  spend appears automatically as `routine:<name>` if it runs as a schedule, or under the user if it runs in a chat root [fact].
  Routine origin rings and root-session rings with subagent satellites exist in the scene (R1, R4) — knowledge
  `projects/theuth-hud-visual-overview-2026-10-07` [fact].
- **Attention inbox (104/132)**:
  `components/attention/{Page,Row,Screen,Bundle,PacketBoard,ApprovalScreen,AskScreen,DecisionScreen,ReviewScreen,ShowScreen,UnifiedDiff}.tsx`,
  `model.ts`, `queries.ts`; items are versioned/dismissable (119) [fact]. A "keep or discard this experiment?" review or an
  "experiment N regressed by 12 %" packet fits `review`/`decision`/`show` kinds without a new kind [inference].
- **Routines UI (108)**: `components/routines/{RoutinesView,RoutineDetail,RoutineChat,NativeSelect}.tsx`, `draft.ts`,
  `helpers.ts`, `queries.ts`; the form has a Radix Select model picker over `list_models` and a "Time limit per run" field (wave
  88) — knowledge `projects/theuth-wave-88-routine-limits-model-pin-2026-10-03` [fact]. `RoutineDetail` shows the run history
  table (`runs`) + the ≤ 2 KiB `report` — design 33 §0.7 [fact]. This is the closest existing "loop status" screen: per-run rows
  with result + report; it lacks a metric column or a chart [fact/inference].
- **Tasks board (115/118)**: `components/tasks/{Board,Queue,TaskDetail,NewTaskSheet,TaskChips,Attachments}.tsx`, `boardMemory.ts`
  — a kanban by state with per-project memory [fact]. An "ideas backlog" maps onto tasks in `backlog` with `done_when` as the
  hypothesis/metric target — `task_create{state: backlog}` — prompt "Tasks" [inference].
- **Apps (46)**: the `app` tool renders an interactive OpenUI-Lang app with state/Query/Mutation backed by installed tools,
  `http_request` or MCP (`apps` skill) [fact]. **json-render** fences (30/123): 57 typed components in
  `crates/surface/src/spec.rs` (one definition feeds serde validation, the schemars prompt catalog and ts-rs bindings —
  spec.rs:1-12), rendered by `components/chat/surface/registry.tsx` (`MetricGrid` :302, `Sparkline` :530; `Bars`, `Table`,
  `Checklist` also Slack-native) — knowledge `facts/theuth-agent-visual-surfaces-inventory-2026-10-06` [fact]. A
  metric-over-iterations chart is one `Sparkline`/`LineChart` fence in the loop's chat today; an app would give a live dashboard
  without new TS [inference].
- **Boards** (`doc kind=board`): `components/board/{BoardReader,Sections,Toc,Tour,SidePane,anchors,units}.tsx` — long-form
  explanation anchored to `path#L..` — a natural "experiment report" surface since every claim can point at the diff
  [fact/inference].
- **Notifications**: `Deliver` sends a run's reply to the user's chat channels (Slack/Telegram/Signal) unless `quiet`;
  `message_user` reaches them out of band; desktop + Windows/iPad companions (130/134) carry notifications and the Ask bar —
  prompt "Schedules", knowledge `projects/theuth-wave-137…` (134 W5 landing) [fact]. Slack can show `Alert, Callout, Table, Bars,
  Sparkline, Metric, MetricGrid, KeyValueList, Checklist` natively — prompt "Slack" [fact].
- **Pattern summary for a new dashboard** [inference]: RPC method + ts-rs type (`crates/rpc/src/api.rs` → `just ts-desktop`), a
  `queries.ts` with TanStack keys and a store slice fed by an `AgentEvent` variant (the `Dream*` events over RPC + UI union in 136
  Z is tonight's freshest example — 136 worktree commit 8e645c84), a `View.tsx` + `Detail.tsx`, fixtures + vitest, Settings row
  with Radix Select model pickers (136 S5, wave 88), rings/gauges in the HUD scene model.

## 10. Guards & budgets

- **tools.policy and approvals**: `ToolsConfig{policy: Policy /*per risk: allow|ask|deny*/, approvals: bool, approval_timeout_secs
  300, ask_timeout_secs 1800, …}`; `approvals: false` makes every `ask` run as `allow` while `deny` still denies; written back by
  `SetApprovals{session, enabled}` (per session, in the registry) — crates/tools/src/lib.rs:455-461, 485-499;
  crates/tools/src/registry.rs:434, 721-726, 1692 [fact]. Live: `tools.approvals: false` with policy read_only allow / mutating
  ask / destructive ask — ~/.theuth/config.yaml (`tools:` block; 136-recon §K) [fact]. A schedule's `approvals: false` re-sends
  `SetApprovals{enabled:false}` on every fire and is shown in the `schedule` approval card — crates/schedule/src/lib.rs:1222;
  design 33 §0.8 [fact]. Subagent caps `tools: read_only|mutating` → `denied: subagent is capped` [fact, prompt]. MCP per-tool
  risk/`off` gate in `~/.theuth/mcp/config.yaml` [fact, prompt].
- **Per-session money cap (136 D13)**: **built in the 136 worktree, not on main or origin/main** — `pub struct SetBudget`
  crates/agent/src/lib.rs:470, `SetBudgetNow(Option<Budget>)` loop_.rs:355-358 handled at loop_.rs:2028-2031, `impl
  Message<SetBudget> for Agents` supervisor.rs:3126 (all ~/Develop/theuth-136 @ 30a18e61); commit 8e645c84 "budget guard covers
  side calls (Shared.budget, 'budget exhausted')" [fact]. Contract: `SetBudget{session, nanos: Option<u64>, currency}`;
  `Reserved::open` refuses when `spent_nanos(owner) + estimate > budget.nanos`, the turn ends, the model sees one `<budget spent
  cap>` note; schedule front matter `budget: 1 USD` → `SetBudget` on every fire; `FireWith{name, overrides{budget_nanos,
  run_timeout_secs, payload}}` for a one-off run — docs/design/136-contract.md:71, 89, 98; 136-dreaming.md:26, 309, 438 [fact].
  `git grep SetBudget origin/main -- crates` → nothing [fact]. On main the only per-call limiter seam is `Reserved::open` at
  crates/agent/src/loop_.rs:3809 [fact].
- **Spend ledger (128)**: `model_calls` with returned vs estimated nanos per call, `cost_owners.root_id` attribution,
  `remote_meters`/`remote_turns` for external harnesses; `spend_report` RPC; all figures so far are estimates —
  crates/agent/migrations/0015_session_lifecycle.sql:7-66, 0029_remote_turns.sql:1-21;
  docs/design/128-as-built-SPEND-LEDGER.md:16-48 [fact]. Per-agent monthly caps live in `roster.sqlite` `budget`
  (`Budget::Monthly`) and raise an attention item at 80 % — 128-as-built-SPEND-LEDGER.md:12-14; Hud.tsx:180 [fact]. **No
  per-schedule or per-loop budget on main** [fact].
- **Time caps**: `run_timeout` per schedule (default 1800 s, live-settable), `agent.subagents.max_runtime_secs` (default 600; live
  28 800), `tools.job_timeout_secs` 1800, PTY `wait` ≤ 120 s, `remind_me_in` ≤ 86 400 s — §2, §4 [fact].
- **Stop scope (133)**: default Root stop; Tree by hold/triple-Esc; schedule deadline = Tree — §1 [fact]. No "pause at next safe
  point" primitive [fact, by absence].
- **Untrusted content (28)**: v1 draft (2026-09-27) = layer 1 a local tool-less classifier (`laya-prompt-guard` on the `decision`
  crate's checkpoint) attaching a verdict to every chunk of untrusted text before a tool-holding model sees it, layer 2 a fence +
  taint gate — docs/design/28-untrusted-content.md:1-25 [fact]; built status not verified tonight [unverified]. Relevance:
  benchmark output, repo READMEs and web pages a research loop reads are untrusted; a loop at `approvals: false` plus `shell` is
  exactly 28's threat model [inference].
- **Guard gaps for an unattended loop** [inference]: no per-session money cap on main (136 brings it), no disk/CPU quota for jobs,
  no "max consecutive failures → pause" (33 has `check broken ×3, disabled` only for the *sensor*/`until` check —
  crates/schedule/src/lib.rs:417), no destructive-command allowlist (git reset --hard on the wrong tree is one `shell()` away).

## 11. Parallelism

- **spawn_subagent**: native theuth child (same tools, chosen `model`/`effort`, optional `tools:` cap) or external harness via ACP
  (`agent:` omp / opencode / opencode2 / claude-code …, `cwd` = repo) — prompt "Guidance";
  `crates/acp/src/{harness,agents,registry,peer,mesh}.rs` (`category: "model"` option; omp and opencode announce one; opencode
  reads no `_meta`) — crates/acp/src/harness.rs:162-169, 608-618 [fact]. Limits: `max_depth` 1, `max_concurrent` 16 native /
  `max_concurrent_external` 32 as **per-orchestrator-tree permit pools** (`Semaphore`s keyed by root; the engine-wide pool stall
  was fixed in v2.10.0) — crates/agent/src/lib.rs:2971-2974; crates/agent/src/supervisor.rs:189, 370-379; knowledge
  `facts/theuth-spawn-stall-engine-wide-child-permit-pool-2026-10-07` [fact]. Child deadline `max_runtime_secs` (live 28 800 = 8
  h) → `tell_cancel` on that child only — 133 §3 table [fact].
- **Peer messaging (50/62)**: `send_message` to `session:<uuid>`/`peer:<name>`/`parent`/child name, up to 16 addresses,
  `await_reply`, `reply_to`; receipts `accepted` = queued; rows in `peer_exchanges` — prompt "Guidance";
  crates/agent/migrations/0011_communication.sql:17-100 (via 136-recon §F) [fact]. Communication is a harness-wide switch
  `agent.communication` (live `true`) — crates/agent/src/lib.rs:2718-2722; ~/.theuth/config.yaml [fact].
- **Sandboxes and nodes**: §4 — a VM or another engine is a `spawn_subagent{agent:"<name>"}` target [fact].
- **Worktree + build coordination**: practice in knowledge (one worktree per worker, per-worktree target dir, `build-lock` slots,
  one workspace check per wave, broadcast to active roots before heavy runs) — §7; prompt "Share the machine" [fact].
- **Known failure modes (knowledge, all 2026-09/10)**: omp ACP child killed by `max_runtime_secs` keeps working → busy-prompt
  retry fix (`facts/theuth-acp-child-busy-after-cancel-2026-09-24`); omp PTYs die on engine restart and never wake you on
  completion (`facts/omp-pty-start-shape-theuth`,
  `procedures/theuth-recover-omp-acp-workers-after-engine-restart-by-pty-resume-2026-10-07`); host reboot leaves children
  `stored`, resume via `send_message RESUME` or fresh child with commits named
  (`facts/theuth-macbook-rebooted-mid-wave-…-2026-10-07`); a worker waiting on `build-lock` looks stuck
  (`facts/theuth-build-lock-queued-worker-looks-stuck-2026-10-07`); never prune `omp acp` by time window — killed a sibling's
  workers (`facts/theuth-killed-sibling-omp-workers-by-time-window-2026-09-30`); the loop run that fanned out never closed before
  33's `Settle` fix (33 §11); shell jobs die with the harness and **jobs are not durable by ruling (109 D14)**
  (`decisions/theuth-durability-design-109-rulings-2026-10-06`); three concurrent waves → load 112, 60–100 min gates, disk to 375
  MB (`facts/theuth-parallel-waves-cold-gates-and-disk-2026-09-28`); the 2 h omp ACP ceiling named in the brief: not found as a
  constant tonight — the live `max_runtime_secs` is 28 800 s; the earlier default was 600 s [unverified: the 2 h figure].
- **For evolutionary/parallel variants** [inference]: N children each in its own worktree with its own target dir is already how
  waves run; the missing pieces are engine-managed worktrees, a shared benchmark queue (one benchmark at a time machine-wide — a
  `BUILD_SLOTS=1`-style slot), and a result channel back to the planner that is not free-text (today: the child's final message).

## 12. Testing

- **Crate tests**: each plugin crate carries `#[cfg(test)]` modules and `tests/` (e.g. `crates/schedule/src/lib.rs` tests at :2871
  `run_timeout_key_sets_the_run_deadline`, :3763-3793 the `Settle` test with `SETTLE + 300 ms` grace;
  `crates/codeindex/tests/codemap.rs:526`; `crates/tools/src/shell.rs:434-460` the timeout tests) — [fact]. Workspace gates in the
  `justfile`: `clippy` (:38), `fmt` (:47), `durability` (:34), `ts` (:90) / `ts-desktop` (:94, regenerates `bindings/` →
  `crates/desktop/ui/src/rpc/bindings/`), `desktop-check` (:223), `models-check` (:334), `vm-image-dev` (:356) — justfile recipe
  lines [fact]. `project_profile()` exists to brief a child with these commands rather than guess [fact, prompt].
- **UI tests**: vitest beside each component (`*.test.ts` in `components/{agents,attention,hud,routines,tasks,board}` and `lib/`),
  `tsc -b --noEmit`, vite build; fixtures under `components/hud/scene/fixtures.ts`, `components/board/fixtures/` — directory
  listings [fact]. Rendering proof for json-render components = vitest over fixtures; no agent self-check of a rendering —
  knowledge `facts/theuth-agent-visual-surfaces-inventory-2026-10-06` [fact].
- **Smoke docs**: `docs/design/79-smoke.md`, `67-single-engine-smoke.md`, `133-as-built.md`, the `*-as-built-*.md` per worker
  (gates listed with exit codes, e.g. 124-as-built-DESKTOP.md:37-52 `build-lock bun run check → tsc 0 errors`) [fact]. RPC-level
  verification recipe in plugins.md §5: `list_tools` shows your tools with the right risk, methods round-trip, `subscribe_events`
  delivers your events, `foo.enabled=false` + restart → tools gone — docs/plugins.md "## 5. Verify" block [fact].
- **Machine rules for gates** (binding on every wave tonight): crate-level gates only (`cargo check/test/clippy -p <crate>
  --no-fail-fast`), `CARGO_TARGET_DIR` per worker, one workspace check per wave behind `build-lock` —
  docs/design/136-contract.md:8-12, 182 [fact].
- **What a proof list looks like**: 136-contract.md's wave table gives per worker *files owned · forbidden · behaviour section ·
  gate command · `DONE <W> <sha>`* (lines 20-25); 137-endless.md §14 lists per crate the invariants a test must pin (fold
  tiling/never-split/slack band/purity on a lab fixture copied into `crates/agent/tests/fixtures/endless/`, guards one positive +
  one negative each, system bytes identical across two turns), `cargo test -p rpc` round-trips, `-p roster` parse, UI tsc 0 +
  three vitest suites + vite build, and a **live smoke on a scratch engine** asserting structure not quality
  (`facts/theuth-scratch-engine-smoke-recipe-2026-09-29`) — 137-worktree docs/design/137-endless.md §14 [fact].
- **For autoresearch** [inference]: the testable core is pure — the ratchet decision (keep iff metric better than best by > noise
  floor), the ledger fold (spend/iteration, best-so-far), the stop rules (budget, max iterations, consecutive failures, wall
  clock) and the state machine of a run (proposing → building → benchmarking → judging → kept|discarded|failed) — each a table
  test with fixtures; the loop body itself is a prompt + skill, smoke-tested on a scratch engine with a toy repo whose benchmark
  is a deterministic script.

## 13. Configuration

- `~/.theuth/config.yaml`: "Every plugin reads its own section"; env overlay `WTH_<SECTION>__<KEY>` / `THEUTH_FOO__ENABLED` —
  config.yaml:1-2; docs/plugins.md:235 [fact]. Live sections tonight: `agent` (favorite_models, vision_model, max_tool_rounds 384,
  subagents, communication true, aws_profile, compact_style snap, title_model haiku-4-5, effort max, model gpt-5.6-sol), `tools`
  (approvals false, policy), `tasks`, `rpc`, `slack`, `browser`, `link`, `decision`, `mesh`, `web`, `search_providers` —
  ~/.theuth/config.yaml (88 lines) [fact]. Not present (defaults apply): `knowledge`, `schedule`, `double`, `advisor`,
  `codeindex`, `scrape`, `surface` [fact].
- **Adding a plugin section**: `rt.section::<FooConfig>("foo")` with `#[serde(default)]`, missing section → `Default`; never read
  another plugin's section (ask by message); write back with `runtime::write_config_value(path, "foo", "key", "value")` which
  rewrites one line and keeps comments; add the section commented to `DEFAULT_CONFIG` in `crates/theuth/src/main.rs` (first-run
  template; written only when the file does not exist — main.rs:841, 1267) and a README bullet — docs/plugins.md:232-238 [fact].
  The template already documents `agent`, `knowledge`, `documents`, `surface`, `advisor`, `web`, `scrape`, `codeindex`,
  `search_providers`, `browser`, `computer`, `link`, `nodes`, `schedule`, `mesh`, … `double` — main.rs:844-1170 [fact].
- **Read-at-startup rule**: config.yaml is read once at startup; `mcp/`, `skills/`, `prompt.md` and the vault are live; a few keys
  are live-settable through RPC setters that also persist (`set_schedule_run_timeout`, model/effort, web read/budget keys,
  `SetApprovals`), and the template marks others "restart" (`double.enabled`, `decision.checkpoint`, `rpc.listen`) — prompt
  "harness" paragraph; main.rs:1146, 1165-1170; crates/schedule/src/lib.rs:122-128 [fact].
- **plugins.md §1 "is it a plugin?"**: a tool goes into the plugin owning the state; a change to how turns run goes into `agent`;
  a subsystem with its own on-disk state, actors, tools/events and config section is a plugin; two tests — must it exist before
  `main` starts anything (no plugin does) and could the user turn it off and still have a working harness (yes = plugin) —
  docs/plugins.md:7-21 [fact]. Verdict for autoresearch [inference]: a **plugin crate** (`crates/autoresearch`: ledger sqlite, run
  actor/state machine, 3–5 tools, prompt block, bundled skill, config section `autoresearch:`), with small `agent` amendments only
  where the turn loop must change (a per-session mode flag like 137's `memory`, the budget guard from 136) and `schedule`
  amendments only if the loop rides a schedule file (a `budget:`-style key is already coming from 136).

## 14. Overlapping designs

- **33 Loops** — *Gives*: the only built "run again until a stop" primitive: `until:` shell check before each fire, `max_runs`,
  `every: 0s`, `schedule_done`, the run-closes-when-subtree-idle rule (`Settle`), `report` column, pause/resume by one line,
  `approvals: false` as the unattended grant (33 §0, §3.4, §11; file.rs:429-483; lib.rs:805-849) [fact]. *Missing*: no metric in
  the stop condition (exit code only), no memory across iterations except `<until>`'s 2 KiB tail and what the run re-reads from
  disk, no ratchet/branch awareness, no per-loop budget, one session per file (no parallel iterations), history has no numeric
  column [fact/inference].
- **34 Schedule** — *Gives*: file-as-spec with approval card, one unattended session per schedule, `Compact{Clear}` per fire,
  `run_timeout`, `quiet`, `model:`, `cwd:`, `agent:` (task-routed fires), `runs` history, `fire_schedule` RPC, Routines UI
  (file.rs:313-371; lib.rs:1189-1275) [fact]. *Missing*: `effort:` key; `budget:` (coming from 136); no payload/arguments per fire
  except 136's `FireWith`; no "run this file N times in parallel" [fact].
- **40 Triggers / watchers** — *Gives*: `when: <sensor>.py` Python sensors in the Monty sandbox (20 s budget, 3 strikes →
  disabled), `fire: on_change|while_true|once`, `for`/`cooldown`/`mode`/`expires`, `memory: continue` — file.rs:128-176, 347-368
  [fact]. *For autoresearch*: a sensor can watch "benchmark result file changed" or "CI green", giving event-driven iterations;
  the `memory: continue` + `runtime: keep` pair is the only built way a schedule keeps Python state across fires [inference].
- **108 Routines UI** — *Gives*: RoutinesView/RoutineDetail with history + report, draft cards via `routine_draft`, model picker +
  time limit (wave 88) — components/routines/*.tsx (1 292 lines) [fact]. *Missing*: numeric/progress columns, a chart, a "best so
  far" header, per-run cost [inference].
- **125 Advisor** — *Gives*: a second-model reviewer session riding the turn journal, delta-rendered transcript, pill in the chat
  bottom stack; `kind='advisor'` sessions and `session_advisors` rows — 125-advisor.md:1-30; 0034_advisor.sql [fact]. *For
  autoresearch*: the reviewer-of-proposals / "judge ≠ proposer" slot can be an advisor with a different vendor; missing: an
  advisor that can veto (today it advises) [inference].
- **126 Double** — *Gives*: the only trained stand-in for the owner on approval cards, with a ladder, caps, free supervision and a
  training state machine that already respects idle/AC/build-lock preconditions (126 §8, :29, :343-372) [fact]. *For
  autoresearch*: the precondition machine (`precond.rs`, `sched.rs`) is the template for "run benchmarks only when the machine is
  idle / on AC"; the double itself is irrelevant under `approvals: false` [inference].
- **131 Runtime reset** — *Gives*: `runtime: clear|keep`, `ResetRuntime` at fire, dump caps 64/32 MiB, the lesson that an `every:
  0s` loop of 144 runs grows one heap (131 Problem, D1-D6) [fact]. *Missing*: a *selective* keep (keep the ledger variable, drop
  the rest) — a loop wanting Python-side state across iterations must either `runtime: keep` everything or persist to disk
  [inference].
- **133 Stop scope** — *Gives*: Root vs Tree cancel, hold/triple-Esc UI, children survive a root stop (133 §1-§3) [fact].
  *Missing*: "stop after the current experiment finishes" (graceful drain) and "stop and revert" [inference].
- **136 Dreaming** (worktree 30a18e61, landing ~04:30–05:30) — *Gives*: a bundled routine installed by the harness, a plugin crate
  with its own ledger/journal/undo, the per-session **money cap** (`SetBudget` + `Reserved::open` guard + `<budget>` note),
  `budget:` and `FireWith` on schedules, cases/gate/judge≠proposer vocabulary, rejected-edit buffer, `Toolset` confinement, a `t
  dreaming dry-run` CLI, `Dream*` RPC events + UI, Settings page with per-tier model pickers — 136-dreaming.md:12-27, 309, 332,
  438; 136-contract.md:20-25, 67-98 [fact]. *Missing for autoresearch*: it optimises the harness's own knowledge/skills, not a
  user repo's metric; no git, no benchmark execution, nightly cadence only [fact].
- **137 Endless** (worktree, W1 skeleton 03:30) — *Gives*: a per-session memory mode flipped at creation or `/endless`, constant
  system bytes, a summary tree with zoom/recall tools, the create-options + New-chat switch + agents-file field + config section
  pattern, `endless_calls` cost rows — 137-endless.md D1-D6, §8-§11 [fact]. *For autoresearch*: the planner's long memory and the
  exact UI/engine pattern for "a mode on a session"; missing: nothing it promised — it is simply not landed yet [inference].
- **79 Sandbox VM** — *Gives*: isolated Linux desktop VMs as mesh engines, snapshot/restore, shares, resize,
  `spawn_subagent{agent:"<vm>"}`, user take-over — crates/sandbox/src/tools.rs; 79 Scope [fact]. *Missing*: throwaway no-engine
  `sandbox_run` (named as a later seam), GPU, per-VM resource accounting in the ledger [fact].
- **19 Subagent board** — *Gives*: the parent/child session tree in every client, `SubagentStrip` dots, child chats openable in
  panes, `<subagent_result>`/final-message wake of the parent (19; 133 §1) [fact]. *Missing*: a structured result channel (metric,
  sha, status) from child to parent — today it is prose [inference].
- **potential-features.md / design-gaps-expanded.md**: no entry about experiments, research or optimisation loops (grep
  `experiment|research|optimi[sz]|benchmark|ratchet` → 0 hits in both); the only loop mention is a `person_ask` sketch citing 33
  §7 for unattended sends — potential-features.md:33-36 [fact].

## 15. Weaknesses

What would break or hurt in a multi-hour / multi-day loop today (evidence first):
1. **No per-session money cap on main** — `git grep SetBudget origin/main -- crates` is empty; the guard exists only in
   ~/Develop/theuth-136 (lib.rs:470, loop_.rs:355-358, supervisor.rs:3126) [fact]. A loop can spend until a human notices the HUD.
2. **Shell jobs die with the harness and at 1800 s** — `JobExit::Lost` (jobs.rs:4, 90-96), `tools.job_timeout_secs` 1800
   (lib.rs:493); 109 D14 ruled jobs *not* durable [fact]. Every benchmark > 30 min or spanning a restart must be detached by hand
   (`nohup`, log, EXIT marker) and polled with reminders — the knowledge base documents this as the standard workaround
   (`procedures/theuth-remote-build-host-loop`) [fact].
3. **Memory shape of one long session**: compaction at 0.8 × window with compounding summaries, loaded skills cleared at every
   fold (loop_.rs:6254-6260 via 136-recon §C), knowledge hints re-fired per turn with 2.2 % uptake
   (`facts/theuth-knowledge-hint-usage-baseline-2026-10-07`), Python heap growth until 131's caps [fact]. The schedule alternative
   discards everything each iteration except a 2 KiB `<until>` tail [fact]. 137 fixes the first; it is not landed [fact].
4. **Harness restarts and reboots mid-wave are routine on this machine** — engine restarts 10:19 and ~11:00 on 2026-10-07,
   children `stored`, PTY omps dead, `/tmp` build slots gone (`facts/theuth-macbook-rebooted-mid-wave-…`,
   `procedures/theuth-recover-omp-workers-across-engine-restarts-and-host-reboot-2026-10-07`) [fact]. A loop's state must live in
   sqlite/git, never in the actor.
5. **Benchmark noise on a shared, loaded machine** — load 23 with no cargo running
   (`facts/theuth-dev-machine-state-2026-10-08-midnight`), load 112 under three waves, gates 60–100 min
   (`facts/theuth-parallel-waves-cold-gates-and-disk-2026-09-28`) [fact]. Wall-clock or throughput metrics measured here are not
   comparable across iterations without repeats and an idle precondition (126's `precond.rs` has the idle/AC checks) [inference].
6. **Disk**: root disk hit 375 MB free with three waves; a 255 GB `target` deleted to recover; per-worktree targets 41–67 GB
   [fact, same knowledge]. A parallel-variant loop that cuts worktrees must own their cleanup.
7. **No structured child → parent result** — a child's final message is prose; `session_read` of a > 64 KiB turn loses the message
   (`facts/theuth-write-file-refuses-truncated-read-and-64kib-spill-sheets-2026-10-08`) [fact].
8. **No git safety net** — no engine-side checkpoint/revert; a wrong `git reset --hard` or a worktree cut on the wrong base is one
   `shell()` away under `approvals: false` [fact, by absence; §7].
9. **Loop closes only when the subtree is idle** — a child cancelled without delivering keeps the run open until the deadline (33
   §3.4 "ponytail") [fact]; with `max_runtime_secs` 28 800 live, one stuck worker can hold an iteration 8 h [inference].
10. **Permit pools**: 16 native / 32 external children per orchestrator tree; `max_depth` 1 — a planner → N variants → each with a
    tester is not expressible (depth 2) [fact: lib.rs:2971-2974].
11. **Attention noise**: unattended runs that hit `ask` time out after `ask_timeout_secs` 1800 with no one to answer (lib.rs:491);
    `approvals: false` is all-or-nothing per session [fact].
12. **Monty limits** for in-loop statistics: no numpy, 300 s sleep per call / 600 s per turn, cell type-check traps
    (`facts/monty-typecheck-union-dict-globals`) [fact].

## 16. Strengths to build on

Ranked by how much of the feature they already carry:
1. **33/34 schedule loops** — stop conditions, unattended session, per-run history + report, deadline cancel of the whole subtree,
   pause/resume, Routines UI, RPC `fire_schedule`. The outer loop exists; it needs a metric-aware stop and a ledger
   (crates/schedule 7 996 LOC) [fact].
2. **136's budget guard + `budget:` key + bundled-routine pattern** (landing tonight) — the money cap at the only per-call seam, a
   plugin crate with ledger/journal/undo/dry-run CLI/Settings page to copy file-for-file [fact].
3. **137's per-session mode pattern** (landing tonight) — column + create option + New-chat switch + `/cmd` + `agent.<mode>`
   config + `SetXNow` actor message; and a planner memory that does not compact [fact].
4. **Subagents + ACP harnesses + per-tree permit pools** — fan-out to omp/opencode workers in worktrees is proven nightly (waves
   122–137) [fact].
5. **Sandbox VMs with snapshot/restore and shares** — a clean, isolated benchmark box per experiment [fact].
6. **Spend ledger (`model_calls`/`cost_owners`, `spend_report`)** — cost per iteration is one join away [fact].
7. **Durability 109** — turn journal, `tool_calls.files`, `resume_reports`, questions/waits as rows; the loop's own state only has
   to be rows too [fact].
8. **codeindex + gix** — `code_impact(git_range)` for "what did this experiment touch", `Git`/`Range::Commits` for a Rust-side
   read of the ratchet branch [fact].
9. **Decision crate** — zero-token triage of logs/ideas; capture + `decide_label` for a learned "is this idea worth running"
   sorter [fact].
10. **Visual surfaces** — json-render `Sparkline`/`MetricGrid`/`Table`, apps, boards, HUD rings, attention kinds incl.
    `budget`/`review` — no new rendering grammar needed for v1 [fact].
11. **Knowledge base as institutional memory** — procedures for build-lock, detached jobs, worktree hygiene, restart recovery
    already written; the loop's skill can cite them instead of re-deriving [fact].
12. **`build-lock`** — a working machine-wide slot discipline that a benchmark queue can adopt verbatim (`BUILD_SLOTS=1`, a label)
    [fact].

## 17. Gaps

What the feature must add, by crate (all [inference] unless cited):
- **New `crates/autoresearch` (plugin)**: run state machine actor (idle → proposing → building → benchmarking → judging →
  kept|discarded|failed → …; stop reasons budget/iterations/wall-clock/consecutive-failures/user);
  `<data_dir>/autoresearch/<run>.sqlite` with `runs`, `experiments(iteration, parent_sha, sha, metric, repeats JSON, kept, reason,
  cost_nanos, duration_ms, log_path)`, `ideas(backlog, source, status)`, `notes`; a markdown journal document (136's journal
  pattern) and an undo receipt; the ratchet rule as a pure function with a noise floor (median of N, improvement > k × MAD); tools
  `research_start{repo, branch, benchmark_cmd, metric_regex, goal, budget, max_iterations, parallel?}`, `research_status`,
  `research_stop{at: now|after_iteration}`, `experiment_record`, `idea_add`; a prompt block; a bundled skill (`autoresearch.md`)
  carrying the protocol: detach benchmarks with a log + EXIT marker and poll with `remind_me_in` (because of §4's 1800 s /
  lost-job facts), commit on keep, `git reset --hard <best>` on discard, never touch another tree; RPC methods + `Research*`
  events (ts-rs) for the UI; config section `autoresearch:` (default budget, noise floor, repeats, idle precondition, judge model,
  vendor-different judge).
- **`crates/agent`**: reuse 136's `SetBudget`/`Reserved::open` guard once landed (no new code if the loop runs in a session with
  `budget:`); optionally a per-session mode column like 137's `memory` if the loop is a session mode rather than a plugin actor; a
  `work`-typed structured child result (metric, sha, status) on the subagent result path — today prose only (19/50); raise
  `max_depth` to 2 for planner → variant → tester, or document that variants are flat.
- **`crates/schedule`**: a `metric:`/`until_metric:` stop or simply let the plugin own the loop and use `until:` as a shell check
  over the ledger (`t research check <run>` exit 0 when done — zero schedule changes); `budget:` and `FireWith` from 136 reused as
  is; an `effort:` key is still missing for cheap iterations [fact: 136-recon §G].
- **`crates/tools`**: a detached-job mode for `shell()` (survives the harness, journals pid + log, re-adopted on resume) —
  currently rejected by ruling 109 D14 (`decisions/theuth-durability-design-109-rulings-2026-10-06`), so the alternative is the
  skill-level nohup pattern plus a `jobs`-row reaper; a benchmark slot (`BUILD_SLOTS=1` label `bench`) exposed as a tool or
  documented in the skill.
- **`crates/codeindex` / git**: a small Rust-side `git log <base>..<head>` / `git rev-parse` read via the existing `gix` `Git` for
  the ledger and UI; engine-managed worktrees (t3code borrow list "next" item) if parallel variants are in v1.
- **`crates/sandbox`**: nothing mandatory; a `sandbox_run` throwaway (79's named later seam) would make per-experiment clean VMs
  cheap.
- **`crates/desktop/ui`**: a Research view (run list, best-so-far metric, `Sparkline` of metric per iteration, kept/discarded
  table with sha links, spend), a Detail with the journal document and the current experiment's log tail (`lib/jobTail.ts`
  exists), Settings › Research (model pickers as 136 S5), HUD ring (routine-origin ring pattern), attention `review` packet for
  "keep?" when `approvals` are on; Slack digest via the existing Block-Kit-native components.
- **Skills / knowledge**: the autoresearch skill; `loops.md` gains a paragraph pointing to it; knowledge records per machine
  (benchmark idle rules) rather than prompt.md (Sky's ruling
  `decisions/theuth-shared-hardware-prompt-generic-in-source-machine-rules-in-knowledge-2026-10-08`) [fact].

## 18. Numbers

LOC (`wc -l`, main 17769a5c unless noted):
- `crates/schedule/src`: file.rs 1 776 · lib.rs 4 272 · store.rs 811 · tools.rs 736 · validate.rs 401 (= 7 996); design
  33-loops.md 466 lines, 34-schedule.md 670, 40-triggers.md 809, 108-routines-ui.md 880 [fact].
- `crates/tools/src`: jobs.rs 2 818 · pty.rs 4 182 · shell.rs 615 · registry.rs 2 932 [fact].
- `crates/agent/src`: loop_.rs 9 341 · supervisor.rs 13 076 · lib.rs 2 992 · frame.rs 929 · compact.rs 1 213; 36 migrations
  [fact].
- `crates/codeindex/src/git.rs` 572; `crates/decision/src` ≈ 9 237 (engine 2 496, tool 1 482, training 1 212, …);
  `crates/double/src` ≈ 14 000 (actor 2 235, train_a 1 394, sched 1 343, …); `crates/sandbox/src` ≈ 6 360 (actor 2 038, tools 979,
  bringup 952); `crates/acp/src` ≈ 2 490 (harness.rs 1 398) [fact].
- UI: routines 1 292 (RoutineDetail 932) · hud 1 200 (Scene 726, Hud 361) · tasks 1 409 (TaskDetail 475, Board 362) [fact].
- Worktrees tonight: 136 `crates/dreaming/src` 7 519 LOC (21 files) [fact]; 137-endless.md 342 lines, `crates/agent/src/endless/`
  8 files in W1 [fact]. 413 files in docs/design, 48 crates, 27 worktrees [fact].

Default limits (main):
- `schedule.run_timeout_secs` **1800** (lib.rs:60); `MIN_EVERY` 10 s, `MIN_EVERY_WHEN` 30 s (file.rs:14-16); `SETTLE` 500 ms
  (lib.rs:849); `until` tail 2 KiB, `report` ≤ 2 KiB (33 §0.7) [fact].
- `tools`: `job_grace_secs` 3, `job_timeout_secs` **1800**, `approval_timeout_secs` 300, `ask_timeout_secs` 1800,
  `pty_ready_timeout_secs` 30, `pty_idle_secs` 60, `reminder_max_secs` 86 400 (lib.rs:490-496); jobs `MAX_OUTPUT` 64 KiB, `TAIL` 8
  KiB, `MAX_ROWS` 32, `PTY_BUDGET` 512 MiB ≈ 80 PTYs (jobs.rs:42-62); PTY `WAIT_DEFAULT` 30 / `WAIT_MAX` 120 / `READY_MAX` 300 /
  `IDLE_MIN` 10 / `IDLE_BACKOFF_MAX` 3600 s / `HISTORY` 512 rows (pty.rs:54-69) [fact].
- `agent`: `max_tool_rounds` 32 (live 384), `stream_idle_secs` 120, `max_salvages` 2, `max_tool_output_chars` 32 000, `compact_at`
  0.8 (lib.rs:2770-2777); compaction ceilings tail 40k / image 30k / target 120k tokens, `MIN_HEADROOM` 8k, `SUMMARISE_TIMEOUT`
  300 s (136-recon §C); `subagents` `max_depth` 1, `max_concurrent` 16, `max_concurrent_external` 32, `max_runtime_secs` 600 (live
  **28 800**), `reply_timeout_secs` 120 (lib.rs:2971-2974; config.yaml) [fact].
- Runtime: monty sleep 300 s/call, 600 s/turn; dump caps 64 MiB hard / 32 MiB soft (131 D6) [fact]. Spill/epoch files swept at 7
  days (spill.rs:146-148) [fact].
- Budgets: **none per session on main**; 136 default `$1`/night fully configurable (136-dreaming.md:3, 170); roster monthly
  budgets with an attention item at 80 % (128 as-built :12-14; Hud.tsx:180); `decision.tool.max_rows` 2 000, batch budget 2 000 ms
  (tool.rs:307-354 via 136-recon §H) [fact].
- Sandbox: `DEFAULT_TIMEOUT` 600 s (sandbox/lib.rs:126) [fact].
- Counts: 14 attention kinds (Hud.tsx:162-177); 9 tail-note kinds (07 §…, loop_.rs:4926-4973 via 136-recon §E); 57 json-render
  components (knowledge inventory) [fact].

## 19. Citations checked

Re-opened at 03:50 (read_file / grep on main 17769a5c unless noted); ok = the cited lines say what the report says.
1. crates/schedule/src/file.rs:14, 429, 481-483 — `MIN_EVERY` 10 s, `has_stop`, "every: 0s needs until or max_runs" — **ok**.
2. crates/schedule/src/lib.rs:53-60 — `run_timeout_secs: 1800` default — **ok**.
3. crates/schedule/src/lib.rs:805-823, 845-849, 2117-2128 — `struct Run{deadline, …, last_stop}`, `SETTLE = 500 ms`, `Settle`
   handler — **ok**.
4. crates/schedule/src/lib.rs:1095-1102 — `<schedule name at quiet [run=]>` wrapper — **ok**; 1196 `Compact`, 1208 `ResetRuntime`,
   1222 `SetApprovals`, 1080-1082 `until broken` — **ok**.
5. crates/schedule/src/tools.rs:300-307 — `ScheduleDoneArgs{why}` / `ScheduleDoneTool` — **ok**.
6. crates/tools/src/lib.rs:455-499 — `ToolsConfig` fields and defaults (approvals true, approval 300, ask 1800, grace 3,
   job_timeout 1800, pty_idle 60, reminder 86 400) — **ok**.
7. crates/tools/src/jobs.rs:4, 42-62, 83-96 — lost-on-shutdown, `MAX_OUTPUT`/`TAIL`/`MAX_ROWS`/`PTY_BUDGET`, deadline wording,
   `JobExit::Lost` — **ok**.
8. crates/tools/src/shell.rs:265-270, 434-460 — `shell` Destructive, timeout tests "capped to" — **ok**.
9. crates/tools/src/pty.rs:38-69 — ROWS/COLS, WAIT/READY/IDLE constants, HISTORY 512 — **ok**.
10. crates/agent/src/lib.rs:2704-2730, 2770-2777, 2951-2974 — AgentConfig fields, defaults 32/120/2/32 000/0.8, subagents
    1/16/32/600 — **ok**.
11. crates/agent/src/lib.rs:463-490 — `Toolset`, `SetToolset` — **ok**; crates/agent/src/loop_.rs:2878-2883 `prompt_blocks`
    Toolset filter + UNATTENDED_LINE — **ok**; frame.rs:375-391 header/skills/guidance assembly — **ok**.
12. crates/agent/src/loop_.rs:3809 `Reserved::open` (the limiter seam); 6254-6260 hinted/knowledge_read/files_written/skills
    cleared at the fold; 4926-4931 hint note build — **ok**.
13. crates/agent/src/compact.rs:557-568 — `CompactStyle{Summary, Cut, Snap, Clear}` — **ok** (note: the `Snap` doc-comment still
    says "not built" although 13 is as-built and live config uses `snap` — stale comment, not a report error).
14. crates/agent/migrations/0015_session_lifecycle.sql:7-12 `cost_owners` (no FK to sessions) and 0033_durability.sql:41
    `tool_calls.files` — **ok**.
15. crates/codeindex/src/git.rs:22-69, 382 — `Git`, `Range::{Worktree, Commits}`, `Span`, `Status`, `Change`, `hunks` — **ok**;
    Cargo.toml:14 gix 0.87 and the two crate deps — **ok**.
16. crates/desktop/ui/src/components/hud/Hud.tsx:162-180 — 14 attention kinds, "budgets at 80 %" — **ok**.
17. docs/plugins.md:5, 7-21, 190-198, 232-238 — plugin definition, "is it a plugin" table, `const RISK` the only permission input,
    config section rules — **ok**.
18. docs/design/133-stop-scope.md §1-§3 — Root default, Tree cascade, caller table (schedule `cancel_tree` = Tree) — **ok**.
19. docs/design/131-runtime-reset.md Problem, D1-D6 — 144 runs / 887 names / 168 MB, `runtime: clear|keep`, 64/32 MiB caps —
    **ok**.
20. ~/Develop/theuth-136 @ 30a18e61: crates/agent/src/lib.rs:470 `SetBudget`, loop_.rs:355-358 `SetBudgetNow`, supervisor.rs:3126
    — **ok**; `git grep SetBudget origin/main -- crates` → empty — **ok**.
21. ~/Develop/theuth-137 docs/design/137-endless.md D1-D6, §8, §14, §15 and the W1-W4 worktree logs (3955ab2b 03:30,
    6291f367/bba57894 03:32, 9e3e2b75 03:40) — **ok**.
22. ~/.theuth/bin/build-lock:1-50 — slots dir `/tmp/theuth-build-slots`, `BUILD_SLOTS` default 2, adaptive third slot under load
    16 — **ok**.
Wrong/corrected: the brief's "900 s job cap" — the default on main is 1800 s (tools/lib.rs:493); 900 appears only in knowledge
from 2026-09-28. The brief's "omp ACP 2 h ceiling" — not found as a constant; live `max_runtime_secs` is 28 800 [unverified].
136-recon §C's "spill swept after 7 days": spill.rs:146-148 is the generic `sweep(dir, max_age)`; the 7-day constant was not
re-read tonight [unverified constant, mechanism ok].
