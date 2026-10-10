# OpenAI Symphony — the tracker is the queue; isolated autonomous runs (recon, 2026-10-10)

Clone: `symphony/` = openai/symphony @ be10a1b (2026-09-15), Apache-2.0. Read end to end: `SPEC.md` (2,312 lines),
`elixir/lib` (51 files, 13,070 lines), `elixir/WORKFLOW.md`, both READMEs, `.codex/skills/land`. Nothing built or run.

## Executive summary
1. **What:** OpenAI's daemon that polls an issue tracker, makes one workspace per issue and runs Codex (app-server) in it until the issue leaves its active states.
2. **Form:** a language-neutral contract (`symphony/SPEC.md`, RFC 2119, 2,312 lines) plus an Elixir/OTP reference build (13.1k lib lines, 14.9k test lines).
3. **No agents, no org:** the "team" is up to N concurrent Codex runs (default 10). A role is one repo-owned WORKFLOW.md; routing uses tracker state, labels and assignee.
4. **Idea 1, reconcile before dispatch:** each tick re-reads running items, stops terminal or non-active ones, kills stalls, then dispatches by priority under caps (`symphony/SPEC.md#L733-L849`).
5. **Retries** are explicit timers: 1 s continuation after a clean exit, 10 s·2^(n-1) up to 5 min after failures. Continuation turns reuse the thread, up to 20 per worker.
6. **Idea 2, the tracker is the queue and the human's console:** success means the agent moved the ticket to `Human Review`; humans then move it to `Merging`, `Rework` or `Done`.
7. **Who writes:** the scheduler only reads (two calls plus an adapter-computed `dispatchable`). The agent writes through host-side tools that hold the token (`symphony/SPEC.md#L1310-L1322`).
8. **Idea 3, policy in the repo:** WORKFLOW.md is typed front matter plus a strict Liquid prompt, reloaded every second; a bad edit keeps the last good version.
9. **The reference prompt is a whole team process:** status map, one `## Codex Workpad` comment, PR feedback sweep, blocked-access escape hatch, land loop.
10. **Weak spots:** claims live in memory (one instance; a restart forgets blocked issues); no per-issue cap on runs or tokens; raw `linear_graphql`; review gates exist only in the prompt.
11. **theuth is ahead** on identity, org, budgets, approvals, memory, durability and multi-harness; it is behind on the run loop and on tracker ingestion.
12. **Verdict:** not a team framework, but the cleanest ticket-to-isolated-run scheduler spec. Borrow its reconcile/stall/continuation loop, the tracker read kernel and the repo workflow file.

## A1 Agent model & identity
- **There is no agent record.** The spec's entities are Issue, Workflow Definition, Service Config, Workspace, Run Attempt,
  Live Session, Retry Entry and Orchestrator Runtime State (`symphony/SPEC.md#L152-L293`); nothing names, persists or hires an agent.
  The orchestrator's moduledoc says it "dispatches repository copies to Codex-backed workers" (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1-L4`).
- **Identity is configuration.** One `WORKFLOW.md` is the whole "employee": prompt body + `codex.command` (model, effort,
  approval and sandbox policy). The repo's own file pins `gpt-5.5`, `model_reasoning_effort=xhigh`, `approval_policy: never`
  (`symphony/elixir/WORKFLOW.md#L33-L39`). Changing the role means editing that file; it hot-reloads (`symphony/SPEC.md#L560-L578`).
- **Harness binding is Codex-only.** `codex.command` "MUST speak a compatible app-server protocol over stdio"
  (`symphony/SPEC.md#L475-L478`); the runner launches it with `bash -lc` in the workspace (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L192-L214`). No adapters for other CLIs.
- **Per-run identity** = `session_id = <thread_id>-<turn_id>` (`symphony/SPEC.md#L315-L316`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L94-L97`) plus the issue identifier;
  the workspace key is the sanitized identifier with a 64-bit hash suffix when sanitizing changed it (`symphony/SPEC.md#L307-L312`,
  `symphony/elixir/lib/symphony_elixir/workspace.ex#L266-L287`).
- **Lifecycle states exist for runs, not agents:** PreparingWorkspace → BuildingPrompt → LaunchingAgentProcess →
  InitializingSession → StreamingTurn → Finishing → Succeeded/Failed/TimedOut/Stalled/CanceledByReconciliation
  (`symphony/SPEC.md#L676-L692`).
- **Skills as the only "template catalog":** `.codex/skills/{commit,push,pull,land,linear,debug,release}` are copied into
  target repos (`symphony/elixir/README.md#L44-L47`). No export/import, no catalog, no onboarding wizard.
- **Crude team composition is possible:** each instance can filter by `tracker.assignee` (a Linear user id or `me`) and by
  `required_labels` (`symphony/elixir/lib/symphony_elixir/linear/client.ex#L546-L554`, `symphony/SPEC.md#L390-L394`), so N instances with
  N workflows act like N specialists sharing one board. The docs never present it this way; it is an inference.
- **theuth:** agent = file on a roster with title, `reports_to`, charter, model/tools/mcp/budget/capabilities, paused, and
  a long-lived desk; subagents can be any ACP harness. **Gap:** none for theuth to close; theuth is ahead. Worth noting:
  Symphony shows that a *role* can be just a repo-owned prompt + harness config bound to a queue filter.

## A2 Organisation & authority
- **No org chart, no delegation, no escalation between agents.** The orchestrator "is the only component that mutates
  scheduling state" (`symphony/SPEC.md#L637-L638`); every worker is a peer that reports only to it.
- **Authority sits in the tracker and the human.** Operators control behaviour by editing `WORKFLOW.md`, by moving issue
  states (terminal → stop + clean, non-active → stop without cleanup) and by restarting (`symphony/SPEC.md#L1706-L1717`).
- **Permissions = tracker scope + credential scope.** Scope is the adapter's project/board filter (`symphony/SPEC.md#L1230-L1244`);
  the agent's write power is whatever the host credential can do — the Linear tool "can access whatever the configured
  Linear token can access" (`symphony/elixir/README.md#L229-L233`).
- **Isolation:** one tracker scope per instance (`tracker.kind` + `provider`), one workspace root; non-goal: "Rich web UI or
  multi-tenant control plane" (`symphony/SPEC.md#L62-L62`).
- **Where humans sit:** reviewers in the tracker. The reference workflow parks work in `Human Review` and waits for a human
  to choose `Merging` (approve) or `Rework` (send back) (`symphony/elixir/WORKFLOW.md#L107-L116`, `symphony/elixir/WORKFLOW.md#L244-L251`).
- **theuth:** roster with `reports_to`, escalate up the reporting line, reviewer per task, double answering approval cards.
  **theuth ahead.** The idea worth taking is the *state-machine hand-back*: a human's single state change is the whole
  review verdict, and the scheduler reacts to it without a message.

## A3 Work model
- **Issue = normalized record** with `id` (opaque dispatch identity), `native_ref`, `identifier`, title, description,
  priority, provider-native `state`, branch, url, assignee, lowercased labels, best-effort `blocked_by`, explicit
  `dispatchable`, timestamps (`symphony/SPEC.md#L156-L197`). `dispatchable` carries provider rules the generic scheduler "cannot infer
  safely" (assignment, board, blockers) (`symphony/SPEC.md#L192-L195`).
- **Two-call read kernel.** Adapters MUST implement `fetch_issues_by_states(states)` and `fetch_issues_by_ids(ids)`, and
  nothing else is required (`symphony/SPEC.md#L1186-L1206`); no generic comment/state CRUD "merely to make providers look alike"
  (`symphony/SPEC.md#L1179-L1184`). ID refresh returns full snapshots "because label, assignment, routing ... can change while a run is
  active" (`symphony/SPEC.md#L1227-L1228`), and omission means "no longer visible" (`symphony/SPEC.md#L1205-L1206`); a malformed requested record
  fails the call rather than being silently dropped (`symphony/SPEC.md#L1221-L1225`).
- **States mean what the workflow says.** `active_states` / `terminal_states` are config (`symphony/SPEC.md#L395-L400`). The reference
  board: active = Todo, In Progress, Merging, Rework; terminal = Closed, Cancelled, Canceled, Duplicate, Done
  (`symphony/elixir/WORKFLOW.md#L7-L17`). `Human Review` and `Backlog` are in neither list, so they are *parking* states: a worker stops when the
  ticket enters them and nothing re-dispatches until a human moves it back (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L156-L171`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1113-L1131`).
- **Dispatch eligibility** (all must hold): required fields, state active and not terminal, `dispatchable`, all required
  labels, not running, not claimed, global and per-state slots free (`symphony/SPEC.md#L754-L769`; code `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L816-L829`,
  `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L853-L868`). The Elixir build adds "not blocked" (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L825-L825`).
- **Ordering:** priority 1..4 first, then oldest `created_at`, then identifier (`symphony/SPEC.md#L771-L775`; `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L796-L807`).
- **Dependencies:** Linear marks a `Todo` issue non-dispatchable while any blocker is non-terminal
  (`symphony/elixir/lib/symphony_elixir/linear/client.ex#L496-L511`); GitHub excludes pull requests from dispatch
  (`symphony/elixir/lib/symphony_elixir/github/client.ex#L199-L199`). Blockers are best-effort; adapters "MUST NOT invent blocker
  semantics" (`symphony/SPEC.md#L1278-L1279`).
- **Acceptance criteria and planning live in the prompt, not the scheduler.** The workflow makes ticket-authored
  `Validation`/`Test Plan`/`Testing` sections "non-negotiable acceptance input" mirrored into the workpad (`symphony/elixir/WORKFLOW.md#L88-L88`),
  requires a hierarchical plan + acceptance checklist in one comment (`symphony/elixir/WORKFLOW.md#L140-L169`) and a "Completion bar before Human
  Review" (`symphony/elixir/WORKFLOW.md#L265-L273`). Scope creep becomes a new Backlog issue with `related`/`blockedBy` links (`symphony/elixir/WORKFLOW.md#L89-L94`).
- **Review loop:** Human Review → (human) Merging → agent runs the `land` skill until squash-merged → Done; or → Rework,
  which is "a full approach reset": close the PR, delete the workpad, fresh branch (`symphony/elixir/WORKFLOW.md#L244-L263`).
- **External trackers are THE queue:** Linear, GitHub Issues, GitLab, Jira Cloud, Asana and an in-memory tracker
  (`symphony/elixir/lib/symphony_elixir/tracker.ex#L13-L20`).
- **theuth:** its own board (backlog/todo/doing/review/blocked/done/cancelled, owner, reviewer, `done_when`, checkout,
  dispositions) with an `external` refs field (`theuth/crates/tasks/src/ledger.rs#L1065-L1069`) but no adapter that pulls
  a tracker into tasks. **Gap:** tracker-as-queue ingestion + "handoff state" semantics. theuth ahead on first-class
  acceptance criteria and review state inside its own store.
## A4 Execution & runtime
The heart of the spec. Each rule below is cited to the spec and, where it matters, to the Elixir code that implements it.

### A4.1 Startup and the poll tick
- **Startup:** validate config (fail startup if invalid), remove workspaces of issues already terminal, schedule an
  immediate tick, then repeat every `polling.interval_ms` (`symphony/SPEC.md#L737-L740`, `symphony/SPEC.md#L1801-L1829`). Code: `init` runs
  `run_terminal_workspace_cleanup()` then `schedule_tick(state, 0)` (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L55-L80`).
- **Tick sequence:** reconcile running issues → preflight validation → fetch candidates in active states → sort → dispatch
  while slots remain → notify observers; a failed validation skips dispatch but reconciliation still runs first
  (`symphony/SPEC.md#L742-L752`, `symphony/SPEC.md#L1831-L1861`).
- **Code shape:** the `{:tick, token}` handler marks "checking", pings the dashboard and starts the real cycle 20 ms later so
  "checking now…" can render (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L83-L98`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L13-L16`); `:run_poll_cycle` runs `maybe_dispatch` and schedules the next
  tick (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L118-L126`). `maybe_dispatch` reconciles running and blocked issues before `Config.validate!` + fetch + slot
  check (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L256-L265`) and turns every error class into a log line and a skipped dispatch (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L266-L307`).
- **Tick tokens:** `schedule_tick` cancels the previous timer and stores a fresh `make_ref()` token; ticks with a stale token
  are ignored (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1582-L1596`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L100-L100`). `POST /api/v1/refresh` reuses this and coalesces when a poll is already due or
  running (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1487-L1500`).
- **Live config:** poll interval and global cap are re-read every tick (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1632-L1640`). Spec default poll = 30 s
  (`symphony/SPEC.md#L406-L408`); the reference workflow polls Linear every 5 s (`symphony/elixir/WORKFLOW.md#L18-L19`).

### A4.2 Claiming, eligibility and dispatch
- **One authority:** "The orchestrator serializes state mutations through one authority to avoid duplicate dispatch";
  `claimed` and `running` checks are REQUIRED before launching (`symphony/SPEC.md#L725-L731`). In Elixir the authority is one GenServer.
- **Internal claim states** are separate from tracker states: Unclaimed, Claimed, Running, RetryQueued, Released
  (`symphony/SPEC.md#L640-L660`).
- **Stale-dispatch revalidation:** right before spawning, the issue is re-fetched by id and skipped if it stopped being a
  candidate (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L907-L938`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1006-L1022`).
- **Spawn:** `Task.Supervisor.start_child` running `AgentRunner.run/3`, `Process.monitor`, a running entry with zeroed token
  counters and `started_at`, then add to `claimed` and drop any retry entry (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L953-L991`); spawn failure schedules a
  retry (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L993-L1003`).
- **Concurrency:** `available_slots = max(max_concurrent_agents - running_count, 0)` (`symphony/SPEC.md#L779-L781`); per-state caps from
  `max_concurrent_agents_by_state`, counted over the running map (`symphony/SPEC.md#L783-L788`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L833-L851`); optional per-SSH-host
  cap (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1333-L1341`). Defaults: 10 agents, 20 turns, 5 min backoff cap (`symphony/elixir/lib/symphony_elixir/config/schema.ex#L152-L155`).

### A4.3 The worker: workspace, hooks, turns
- **Runner contract:** create/reuse workspace → build prompt → start app-server → forward events → on any error fail the
  attempt and let the orchestrator retry; workspaces are kept after success (`symphony/SPEC.md#L1163-L1177`).
- **Code:** create workspace, report `{worker_host, workspace_path}` to the orchestrator, run `before_run`, run turns, and
  run `after_run` in an `after` clause (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L38-L56`). Errors are raised so the task exits abnormally (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L28-L35`).
- **Workspace:** `<root>/<workspace_key>`, reused across runs, never auto-deleted on success (`symphony/SPEC.md#L853-L866`); an existing
  directory is reused, a non-directory at the path is replaced, and `created?` gates `after_create`; a failed hook on a new
  directory removes it (`symphony/elixir/lib/symphony_elixir/workspace.ex#L15-L52`, `symphony/elixir/lib/symphony_elixir/workspace.ex#L87-L91`).
- **Hooks:** `after_create`, `before_run`, `after_run`, `before_remove`, run as `sh -lc` in the workspace with
  `hooks.timeout_ms` (default 60 s) (`symphony/SPEC.md#L420-L442`, `symphony/elixir/lib/symphony_elixir/workspace.ex#L397-L418`). Failure semantics: after_create and before_run are
  fatal; after_run and before_remove are logged and ignored (`symphony/SPEC.md#L921-L926`). The reference workflow clones the repo and
  fetches deps in `after_create` and runs a mix task in `before_remove` (`symphony/elixir/WORKFLOW.md#L22-L29`).
- **Safety invariants:** agent cwd must equal the workspace path; the path must stay under the root; keys are sanitized
  (`symphony/SPEC.md#L928-L948`). The app-server client re-checks with canonical paths and reports symlink escape separately
  (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L150-L176`).
- **Turn loop:** turn 1 sends the full rendered prompt; later turns send a fixed "Continuation guidance" text on the same
  thread (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L142-L154`; spec `symphony/SPEC.md#L662-L674`). After each completed turn the worker re-fetches the issue and continues
  only while it is active, routable and `turn < max_turns` (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L101-L140`); at the cap it "returns control to the
  orchestrator" (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L128-L131`).

### A4.4 Driving Codex: app-server JSON-RPC over stdio
- **Not the CLI's exec mode and not ACP:** Symphony speaks the Codex app-server protocol and defers to its schema as the source
  of truth (`symphony/SPEC.md#L950-L964`). Launch: `bash -lc <codex.command>` in the workspace, default `codex app-server`
  (`symphony/SPEC.md#L966-L979`).
- **Handshake:** `initialize` (clientInfo `symphony-orchestrator`, capability `experimentalApi`) then `initialized`
  (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L275-L297`); `thread/start` with `approvalPolicy`, `sandbox`, `cwd` and the adapter's `dynamicTools`
  (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L314-L341`); `turn/start` with `threadId`, the prompt as text input, `cwd`, a title `<identifier>: <title>`,
  `approvalPolicy` and `sandboxPolicy` (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L343-L366`).
- **Streaming:** line-framed with a 1 MiB line buffer (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L12-L12`); `turn/completed` ends the turn; `turn/failed` and
  `turn/cancelled` are errors; other methods go to the approval/tool handler or become notifications; non-JSON lines are
  logged; silence longer than `turn_timeout_ms` → `:turn_timeout`; process exit → `{:port_exit, status}`
  (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L379-L478`).
- **Timeouts:** `read_timeout_ms` (5 s) for request/response (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L895-L897`); `turn_timeout_ms` (1 h) is a *silence*
  window reset by each output, "not a total turn runtime cap"; `stall_timeout_ms` (5 min) is enforced by the orchestrator
  (`symphony/SPEC.md#L1144-L1149`).
- **Approvals:** command and file-change approval requests are answered `acceptForSession` (or legacy
  `approved_for_session`) only when the effective `approval_policy == "never"` (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L55-L55`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L565-L686`,
  `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L761-L794`); otherwise the turn ends with `:approval_required`. MCP tool-approval questions arriving as
  `item/tool/requestUserInput` are auto-answered by picking "Approve this Session", "Approve Once" or any label that starts
  with approve/allow (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L688-L708`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L857-L893`). Other input requests end the turn as `:turn_input_required`
  (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L538-L547`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L1035-L1056`). The spec only requires that such requests "MUST NOT leave a run stalled
  indefinitely" (`symphony/SPEC.md#L1065-L1081`).
- **Default posture vs the repo's posture:** schema default = object `reject` policy (sandbox approvals, rules and MCP
  elicitations rejected), `workspace-write` thread sandbox, turn sandbox writable only at the workspace with network off
  (`symphony/elixir/lib/symphony_elixir/config/schema.ex#L180-L198`, `symphony/elixir/lib/symphony_elixir/config/schema.ex#L570-L579`). The repo's own WORKFLOW runs `approval_policy: never`, network on and
  `shell_environment_policy.inherit=all` (`symphony/elixir/WORKFLOW.md#L33-L39`), and the CLI refuses to start without
  `--i-understand-that-this-will-be-running-without-the-usual-guardrails`
  (`symphony/elixir/lib/symphony_elixir/cli.ex#L111-L125`).
- **Client-side tools:** `item/tool/call` runs host-side through the adapter bound at session start; the result is
  normalised to `success`/`output`/`contentItems`, and an unknown tool returns a structured failure while the session goes
  on (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L587-L620`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L723-L759`; spec `symphony/SPEC.md#L1083-L1089`). Binding to one session snapshot prevents a reload from
  advertising one provider and executing another (`symphony/SPEC.md#L1096-L1098`, `symphony/elixir/lib/symphony_elixir/tracker.ex#L43-L59`).
- **Secret scrubbing:** declared tracker secret env names are removed from the child env and `unset` in the launch line,
  locally and over SSH (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L221-L251`; spec `symphony/SPEC.md#L1748-L1758`).
- **Docs vs code:** the spec says to keep stderr apart from the protocol stream (`symphony/SPEC.md#L1036-L1037`); the Elixir port is opened
  with `:stderr_to_stdout` and non-JSON lines are filtered out (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L199-L209`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L461-L476`). Trust the code: they are
  merged.

### A4.5 Worker exit, retries and back-off
- **Normal exit:** mark `completed` (bookkeeping only) and schedule a *continuation retry* at attempt 1 with
  `delay_type: :continuation`, i.e. 1,000 ms (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L208-L224`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1238`; spec `symphony/SPEC.md#L702-L706`, `symphony/SPEC.md#L797-L799`).
- **Abnormal exit:** retry at the next attempt (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L242-L254`) after `min(10_000 * 2^min(n-1,10), max_retry_backoff_ms)`
  (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1240-L1243`; spec `symphony/SPEC.md#L800-L801`).
- **Timer hygiene:** a new retry cancels the old timer and mints a fresh token (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1034-L1073`); a fired timer only counts
  when its token matches the stored entry (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1075-L1091`).
- **On fire:** re-fetch by id; missing → release; terminal → clean workspace + release; still a candidate → dispatch if slots,
  else requeue with "no available orchestrator slots"; otherwise release (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1093-L1136`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1181-L1221`; spec
  `symphony/SPEC.md#L803-L811`).
- **Consequence:** a continuation always restarts at attempt 1, and failures retry forever at the 5 min cap. An issue that
  stays active after a clean exit is re-dispatched indefinitely (inference from `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L216-L222` and `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1238`).

### A4.6 Reconciliation, stall detection, cancellation
- **Part A, stalls:** elapsed time since the last Codex event (or start) above `stall_timeout_ms` → kill and retry; `<= 0`
  disables it (`symphony/SPEC.md#L823-L829`). Code: `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L581-L639`, activity clock `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L653-L655`. If the last event was an input or
  approval request, the issue is *blocked* instead of retried (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L615-L622`).
- **Part B, tracker refresh:** fetch all running ids; terminal → stop + clean workspace; active and routable → refresh the
  snapshot; unroutable or neither active nor terminal → stop without cleanup; refresh failure → keep workers
  (`symphony/SPEC.md#L831-L839`; code `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L310-L333`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L421-L441`). Ids missing from the answer are stopped (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L478-L496`).
- **Cancellation:** terminate the task under the supervisor (fallback `Process.exit(pid, :shutdown)`), demonitor with flush,
  add runtime to totals and drop running/claimed/blocked/retry (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L554-L579`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L723-L745`).
- **Probable docs-vs-code gap (not verified by running):** the spec says `after_run` runs after success, failure, timeout
  *or cancellation* (`symphony/SPEC.md#L431-L434`), but the hook sits in the worker's `after` clause (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L45-L51`), and an Erlang
  process killed by an exit signal it does not trap never runs `after`. So reconciliation and stall kills probably skip
  `after_run`.

### A4.7 The Elixir-only `blocked` state
- Input-required, approval-required and MCP-elicitation outcomes move the issue into a `blocked` map: out of `running`, still
  in `claimed`, shown in snapshot/API/dashboard (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L757-L779`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1453-L1470`). Blocked issues are re-read every tick and
  released only when terminal, unroutable, non-active or gone (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L456-L474`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L500-L518`). Memory-only: a restart
  clears them and an active issue becomes a candidate again (`symphony/elixir/README.md#L33-L36`). No API call unblocks one.

### A4.8 Restart recovery and idempotency
- Scheduler state is "intentionally in-memory": no timers or sessions survive; recovery = startup terminal cleanup + a fresh
  poll + re-dispatch, reusing kept workspaces (`symphony/SPEC.md#L1690-L1704`). Persisting the retry queue is a TODO (`symphony/SPEC.md#L2239-L2239`).
- Idempotency is pushed into the workflow: the prompt makes the agent find-or-create one workpad comment, reuse a branch
  unless its PR is closed/merged, and resume from the workspace (`symphony/elixir/WORKFLOW.md#L131-L137`, `symphony/elixir/WORKFLOW.md#L142-L147`, `symphony/elixir/WORKFLOW.md#L44-L51`).

### A4.9 Placement: SSH workers
- `worker.ssh_hosts` + `max_concurrent_agents_per_host`; Codex is launched over SSH stdio; the orchestrator stays the only
  source of truth (`symphony/SPEC.md#L2257-L2277`). The scheduler prefers the previous host on retry, else the least-loaded host, and
  waits instead of falling back to local when all hosts are full (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1281-L1316`; spec `symphony/SPEC.md#L2279-L2291`). "One worker
  lifetime never hops machines" (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L23-L24`).

### A4.10 End-to-end trace: one issue from poll to merge (reference WORKFLOW)
1. A human files `MT-649` in the configured Linear project in `Todo` (active) (`symphony/elixir/WORKFLOW.md#L2-L11`).
2. A tick fires; reconciliation runs first; `fetch_issues_by_states(active)` sends one GraphQL query filtered by project slug
   and state names, paged by 50 (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L256-L264`, `symphony/elixir/lib/symphony_elixir/linear/client.ex#L10-L15`). The adapter marks it dispatchable when the assignee
   filter matches and no non-terminal blocker exists while in Todo (`symphony/elixir/lib/symphony_elixir/linear/client.ex#L496-L511`).
3. `choose_issues` sorts and filters; `dispatch_issue` re-fetches by id, spawns the runner under the task supervisor,
   monitors it and claims the id (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L781-L829`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L907-L991`).
4. The runner creates `~/code/symphony-workspaces/MT-649`; it is new, so `after_create` clones the repo and fetches deps
   (`symphony/elixir/WORKFLOW.md#L20-L27`, `symphony/elixir/lib/symphony_elixir/workspace.ex#L15-L31`); host/path go back to the orchestrator (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L41-L44`).
5. `start_session` validates the cwd, launches `bash -lc "unset LINEAR_API_KEY && exec codex … app-server"` (the Linear profile always
   declares `LINEAR_API_KEY` as a secret, `symphony/elixir/lib/symphony_elixir/config/schema.ex#L401-L422`), handshakes and
   starts a thread with `dynamicTools=[linear_graphql]`; `turn/start` sends the strictly rendered prompt (attempt = nil)
   (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L38-L69`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L221-L228`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L314-L366`; strict render `symphony/elixir/lib/symphony_elixir/prompt_builder.ex#L8-L8`).
6. Codex follows the prompt: Todo → In Progress, find/create `## Codex Workpad`, plan, reproduce, `pull`, implement,
   validate, push, open a PR labelled `symphony`, attach it, run the PR feedback sweep, move to `Human Review`
   (`symphony/elixir/WORKFLOW.md#L118-L138`, `symphony/elixir/WORKFLOW.md#L140-L169`, `symphony/elixir/WORKFLOW.md#L199-L242`). Each event updates the running entry (last event, timestamps,
   token deltas) (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L58-L68`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L167-L186`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1508-L1536`); Linear writes run host-side (`symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L587-L620`).
7. `turn/completed`; the runner re-fetches: `Human Review` is not active → `{:done}` → stop session, `after_run`, normal
   exit (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L101-L140`, `symphony/elixir/lib/symphony_elixir/agent_runner.ex#L156-L171`).
8. The orchestrator sees `:DOWN normal` → 1 s continuation retry → re-fetch → neither active nor terminal → claim released
   (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L128-L148`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L208-L224`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1113-L1131`). (Had the agent left it `In Progress`, the worker would have run up
   to 20 continuation turns and then a fresh worker would start 1 s later with the "follow-up attempt" prompt block,
   `symphony/elixir/WORKFLOW.md#L44-L51`.)
9. A human reviews the PR and moves the ticket to `Merging` (`symphony/elixir/WORKFLOW.md#L244-L249`).
10. Next tick: `Merging` is active → dispatch again; the workspace is reused, so no `after_create` (`symphony/elixir/lib/symphony_elixir/workspace.ex#L40-L44`). The prompt
    routes to the `land` skill: check mergeability, pull and resolve conflicts, watch checks, answer Codex reviews,
    `gh pr merge --squash` (`symphony/.codex/skills/land/SKILL.md#L26-L41`, `symphony/.codex/skills/land/SKILL.md#L97-L98`);
    then the agent moves the ticket to `Done` (`symphony/elixir/WORKFLOW.md#L250-L251`).
11. The worker exits; the continuation retry finds `Done` (terminal) → `before_remove` + workspace deletion + release
    (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1117-L1121`, `symphony/elixir/WORKFLOW.md#L28-L29`). Had a human set `Done` mid-run, reconciliation would have killed the worker and
    cleaned up at the next tick (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L421-L426`).
12. Send-back path: `Rework` (active) → dispatch → the prompt does a full reset: close the PR, delete the workpad, fresh
    branch (`symphony/elixir/WORKFLOW.md#L253-L263`).

### A4 versus theuth
- **theuth today:** desks woken by wakes and routines; checkout with epoch and conflict (`theuth/crates/tasks/src/ledger.rs#L1405-L1415`),
  `release_wake(..., min_backoff_ms)` (`theuth/crates/tasks/src/ledger.rs#L2064-L2064`) and `release_claims(session)`
  (`theuth/crates/tasks/src/ledger.rs#L2111-L2111`); subagents over ACP; sandboxes, remote nodes, mesh placement; a durable
  SQLite store. A grep for "stall" in `crates/tasks` and `crates/roster` found only `Install…` identifiers; `crates/schedule`
  has hits I did not inspect.
- **Gaps:** reconcile-before-dispatch against a source of truth, kill-and-retry stall detection with distinct reasons,
  bounded continuation turns that re-check the item, per-state caps, and per-task workspaces with lifecycle hooks.
- **theuth ahead:** durable state across restarts, many harnesses, cross-machine placement, VM sandboxes.
## A5 Coordination between agents
- **Agents never talk to each other.** No messaging, mail or mentions between runs; each worker reports only to the
  orchestrator through `{:codex_worker_update, issue_id, msg}` messages (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L58-L68`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L167-L186`).
- **The orchestrator decides who works next:** priority/age order, one claim per issue, global and per-state caps
  (`symphony/SPEC.md#L754-L788`). It is the only writer of scheduling state (`symphony/SPEC.md#L637-L638`).
- **The tracker is the shared ledger.** Dependencies are tracker relations: Todo issues with live blockers are held back
  (`symphony/elixir/lib/symphony_elixir/linear/client.ex#L501-L511`), and the prompt makes agents file follow-ups in Backlog with `related`/`blockedBy` links instead of
  growing scope (`symphony/elixir/WORKFLOW.md#L89-L94`). That is the only agent-to-agent handoff.
- **One comment as the per-issue ledger:** one persistent `## Codex Workpad` comment with an environment stamp
  `<host>:<path>@<sha>`, Plan, Acceptance Criteria, Validation, Notes and Confusions, edited in place (`symphony/elixir/WORKFLOW.md#L79-L87`,
  `symphony/elixir/WORKFLOW.md#L142-L158`, `symphony/elixir/WORKFLOW.md#L295-L329`). A later run, or a run after Rework, reads it instead of a transcript.
- **Locks and checkout:** only the in-memory `claimed` set (`symphony/SPEC.md#L286-L291`). Two Symphony processes on one board would
  double-dispatch; open PR #82 ("Add Symphony claim lease heartbeat recovery") proposes leases with heartbeat and expiry
  published as tracker comment markers (GitHub MCP, `openai/symphony#82`, state open, mergeable_state dirty).
- **Merge conflicts are handled by the agent through git:** the `land` skill loops on mergeability, runs `pull` to merge
  `origin/main` and resolve conflicts, then watches checks (`symphony/.codex/skills/land/SKILL.md#L11-L18`,
  `symphony/.codex/skills/land/SKILL.md#L26-L41`). No merge queue.
- **theuth:** `send_message` between peers, desk mentions, escalation, checkout with epochs, task threads. **theuth ahead**
  on live coordination. Worth taking: the "one living workpad per work item" convention, and tracker relations as the
  dependency graph.

## A6 Human in the loop & governance
- **The tracker is the control surface.** Moving a ticket is the interrupt: terminal stops and cleans, non-active stops
  without cleanup (`symphony/SPEC.md#L1713-L1715`); `Human Review` parks; `Merging` approves; `Rework` sends back (`symphony/elixir/WORKFLOW.md#L107-L116`).
  Nothing needs a Symphony UI; the dashboard and API are observability only and "MUST NOT become REQUIRED for orchestrator
  correctness" (`symphony/SPEC.md#L1465-L1468`).
- **Review gate by convention only.** The rule "If approved, human moves the issue to `Merging`" (`symphony/elixir/WORKFLOW.md#L249-L249`) and "In `Human
  Review`, do not make changes" (`symphony/elixir/WORKFLOW.md#L290-L290`) are prompt text. Nothing in the code stops the agent from setting `Merging`
  itself through `linear_graphql`, which can run any mutation the token allows (`symphony/elixir/README.md#L229-L235`).
- **Permission prompts:** none reach a human. With `never` they are auto-accepted; otherwise the run is blocked and parked
  (A4.4, A4.7). The spec leaves the posture to the implementation but requires it to be documented (`symphony/SPEC.md#L1067-L1075`,
  `symphony/SPEC.md#L1721-L1732`).
- **Unattended by design:** "This is an unattended orchestration session. Do not ask a human to perform follow-up actions"
  (`symphony/elixir/WORKFLOW.md#L67-L73`). The only human-facing escape hatch is moving the ticket to `Human Review` with a blocker brief: what is
  missing, why it blocks, exact unblock action (`symphony/elixir/WORKFLOW.md#L187-L197`).
- **Guardrails:** workspace containment, sanitized keys, hook timeouts, secret scrubbing, loopback-only HTTP by default
  (`symphony/SPEC.md#L1734-L1769`, `symphony/SPEC.md#L1483-L1484`), plus a hardening list that suggests tighter approvals, VMs, scoped tools and
  issue filtering (`symphony/SPEC.md#L1771-L1797`). The CLI banner: "Codex will run without any guardrails ... not a supported product"
  (`symphony/elixir/lib/symphony_elixir/cli.ex#L119-L125`).
- **Audit:** structured `key=value` logs with `issue_id`, `issue_identifier`, `session_id` (`symphony/SPEC.md#L1361-L1377`), a rotating
  disk log of 10 MB × 5 (`symphony/elixir/lib/symphony_elixir/log_file.ex#L10-L11`), plus the tracker's own history. No
  approval log, because there are no approvals.
- **theuth:** `tools.policy` approvals, the double, attention inbox/HUD, escalation, review state, budgets. **theuth ahead.**
  Borrow the *handoff-state* pattern: a run ends by parking the item in a review state with a structured brief, and the
  human's state change is both the verdict and the next trigger.

## A7 Cost, budgets & runaway control
- **No budgets.** Tokens are counted, not capped: per-session input/output/total, aggregate totals, runtime seconds and the
  latest rate-limit payload (`symphony/SPEC.md#L280-L293`, `symphony/SPEC.md#L1419-L1448`). The token rule: prefer absolute thread totals, ignore
  `last_token_usage` deltas, subtract the last reported total to avoid double counting (`symphony/SPEC.md#L1421-L1433`; code
  `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1508-L1536`).
- **Caps that exist:** `max_concurrent_agents` (10), `max_turns` per worker (20), per-state and per-host caps, backoff cap of
  5 min, stall timeout of 5 min, turn silence timeout of 1 h, hook timeout of 60 s (`symphony/SPEC.md#L605-L633`).
- **Caps that do not exist:** total turns, runs or tokens per issue, or per day. A clean exit always schedules a 1 s
  continuation at attempt 1 (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L216-L222`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1238`), so a ticket the agent never moves out of an active state
  is re-dispatched indefinitely, up to 20 turns per worker. The prompt pushes the same way: "Do not end the turn while the
  work item remains in an active state" (`symphony/elixir/WORKFLOW.md#L50-L50`). Failures retry forever at the 5 min cap (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1240-L1243`).
- **No-progress defence:** stall detection only sees silence (`symphony/SPEC.md#L823-L829`); a chatty loop that keeps emitting events
  never stalls and never hits the silence-based turn timeout (`symphony/SPEC.md#L1147-L1148`).
- **Kill switches:** move the ticket out of active states, edit `WORKFLOW.md` (hot reload), or stop the service.
- **Rate limits:** shown on the dashboard (`symphony/elixir/test/fixtures/status_dashboard_snapshots/super_busy.snapshot.txt`)
  but they do not throttle dispatch; tracker rate limits are the adapter's job (`symphony/SPEC.md#L1235-L1235`).
- **theuth:** budgets with auto-pause and a spend ledger. **theuth ahead.** The lesson cuts the other way: Symphony shows the
  runaway shape that theuth should guard against when it adds continuation loops (A4 borrow) — cap re-dispatches per item.

## A8 Memory, knowledge & learning
- **No memory subsystem.** Continuity comes from three places: the kept workspace, which holds the git state
  (`symphony/SPEC.md#L863-L866`); the live Codex thread across continuation turns in one worker (`symphony/SPEC.md#L1028-L1031`); and the tracker
  workpad comment across workers (`symphony/elixir/WORKFLOW.md#L86-L87`).
- **Context engineering is prompt craft:** strict Liquid rendering with `issue` and `attempt` only, unknown variables and
  filters fail (`symphony/SPEC.md#L493-L516`; `symphony/elixir/lib/symphony_elixir/prompt_builder.ex#L8-L8`); `attempt` does not say whether
  this is a continuation or a failure retry (`symphony/SPEC.md#L1341-L1350`). The workflow tells the agent to "Compact context and proceed"
  after planning (`symphony/elixir/WORKFLOW.md#L169-L169`).
- **Skills:** repo-local `.codex/skills` (commit, push, pull, land, linear, debug, release) that the prompt names
  (`symphony/elixir/WORKFLOW.md#L99-L105`); the land skill ships a Python watcher (`symphony/.codex/skills/land/land_watch.py`).
- **Learning from outcomes:** only the `### Confusions` section of the workpad (`symphony/elixir/WORKFLOW.md#L228-L228`), for humans to read. No evals
  or feedback loop. The Elixir repo's own `AGENTS.md` sets conventions for agents working on Symphony.
- **theuth:** knowledge base, nightly dreaming, cases, skills with sidecars. **theuth ahead.** Borrow candidate: the
  "Confusions" section as cheap, structured hindsight that a dreaming pass could mine.

## A9 Observability & UX
- **Required:** structured logs where operators can see startup, validation and dispatch failures without a debugger
  (`symphony/SPEC.md#L1379-L1388`).
- **Snapshot interface (recommended):** running rows with `turn_count`, retry rows, token totals, rate limits; errors
  `timeout`/`unavailable` (`symphony/SPEC.md#L1390-L1409`). Code: `snapshot/2` with a 15 s timeout (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1392-L1406`) returns running,
  retrying, blocked, totals, rate limits and polling status (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1409-L1485`).
- **Terminal dashboard:** an ANSI status box with agents n/max, throughput (tps) and a sparkline, runtime, tokens, rate
  limits, project URL, next refresh, a running table (id, stage, pid, age/turn, tokens, session, humanised last event) and the
  backoff queue (`symphony/elixir/lib/symphony_elixir/status_dashboard.ex#L1-L27`,
  `symphony/elixir/test/fixtures/status_dashboard_snapshots/super_busy.snapshot.txt`). Humanised summaries are
  observability-only (`symphony/SPEC.md#L1450-L1457`; `symphony/elixir/lib/symphony_elixir/status_dashboard.ex#L1072-L1108`).
- **HTTP extension:** Phoenix LiveView dashboard at `/`, `GET /api/v1/state`, `GET /api/v1/<identifier>`,
  `POST /api/v1/refresh`, 405/404 JSON envelopes, loopback by default (`symphony/elixir/lib/symphony_elixir_web/router.ex#L17-L41`;
  spec `symphony/SPEC.md#L1459-L1632`). The per-issue payload names the workspace, attempts, the session log path and recent events
  (`symphony/SPEC.md#L1553-L1607`).
- **Where the real UX lives:** in Linear and GitHub. The demo shows agents attaching "proof of work: CI status, PR review
  feedback, complexity analysis, and walkthrough videos" (`symphony/README.md#L8-L8`); the workflow requires PR media for
  app-touching changes (`symphony/elixir/WORKFLOW.md#L218-L218`).
- **Onboarding:** copy `WORKFLOW.md`, set `LINEAR_API_KEY`, add the custom Linear states Rework, Human Review, Merging, run
  one binary (`symphony/elixir/README.md#L38-L53`); Burrito single-file releases (`symphony/elixir/README.md#L80-L93`).
- **theuth:** desktop/TUI/web/channels, activity monitor, attention inbox, transcripts. **theuth ahead** on surfaces. Borrow:
  the per-work-item debug endpoint (workspace, attempts, session log, recent events, last error in one payload) and a
  "backoff queue" row type next to running work.

## A10 Extensibility & integration
- **Tracker adapters:** a six-callback behaviour (`fetch_issues_by_states`, `fetch_issues_by_ids`, optional
  `agent_tool_specs`, `execute_agent_tool`, `validate_config`, plus `secret_environment_names`) and a static registry
  (`symphony/elixir/lib/symphony_elixir/tracker.ex#L13-L31`). Each adapter must publish a compact profile: config keys,
  scope, paging, normalisation, tools, error mapping (`symphony/SPEC.md#L1246-L1257`).
- **Provider-native tools, not generic CRUD:** `linear_graphql`, `github_api`, `jira_rest`, `asana_api`, `gitlab_api`
  (`symphony/elixir/README.md#L24-L28`); the spec forbids a lowest-common-denominator API (`symphony/SPEC.md#L1099-L1100`) and defers shared
  helpers until "multiple adapters demonstrate real duplication" (`symphony/SPEC.md#L2242-L2243`).
- **Workflow extensions:** unknown front-matter keys are ignored and extensions may add top-level keys (`symphony/SPEC.md#L366-L373`);
  `server` and `worker` are such extensions (`symphony/SPEC.md#L1470-L1486`, `symphony/SPEC.md#L2257-L2262`).
- **Harness:** Codex app-server only; no MCP server of its own, no A2A, no webhooks (polling only), no chat channels.
- **API:** the read-only JSON API plus `/refresh` (A9).
- **theuth:** MCP, ACP, A2A at the boundary, Slack/Signal/Telegram, sensors, many harnesses. **theuth ahead.** Borrow the
  read-kernel + `dispatchable` contract if theuth adds tracker ingestion, and the "tool bound to a session snapshot" rule.

## A11 Engineering, licence & traction
- **Stack:** Elixir ~1.19 / OTP, Phoenix 1.8 + LiveView, Ecto changesets for config, `solid` (Liquid) templates,
  `yaml_elixir`, `req`, Burrito releases for four targets (`symphony/elixir/mix.exs#L72-L85`).
- **Size:** `elixir/lib` 51 files, 13,070 lines (orchestrator 1,990, terminal dashboard 1,941, app-server client 1,071);
  tests 14,902 lines; `SPEC.md` 2,312 lines.
- **Quality bar:** a 100 % coverage threshold (`symphony/elixir/mix.exs#L11-L13`), a public `@spec` checker
  (`symphony/elixir/lib/symphony_elixir/specs_check.ex`), a PR-body linter, credo and dialyzer; live e2e tests per tracker
  (`symphony/elixir/test/symphony_elixir/*_live_e2e_test.exs`) and snapshot tests for the dashboard.
- **Licence:** Apache-2.0 (`symphony/README.md#L39-L41`).
- **Traction (GitHub MCP, 2026-10-10):** 27,622 stars, 2,873 forks, created 2026-02-26, last push 2026-09-15; 47 commits on
  `main` from 9 authors (34 by one maintainer); 81 PRs, 64 merged, 8 open; zero issues ever (the tracker appears closed);
  releases v0.0.1 (07-18), v0.0.2 (07-24), v0.0.3 (09-15) plus a rolling `nightly`. Commits by month: Mar 10, Apr 2,
  May 5, Jun 6, Jul 20, Aug 1, Sep 3.
- **Dogfooding:** the repo's WORKFLOW points at Symphony's own Linear project (`symphony/elixir/WORKFLOW.md#L2-L5`), and open PRs carry the
  `symphony` label and `codex/sd-NN-…` branches (PR #82), so Symphony is developed partly by Symphony runs.
- **Maturity and stance:** "a low-key engineering preview for testing in trusted environments" (`symphony/README.md#L10-L11`);
  "not a supported product and is presented as-is" (`symphony/elixir/lib/symphony_elixir/cli.ex#L119-L125`); it "works best in
  codebases that have adopted harness engineering" (`symphony/README.md#L17-L19`).
- **Why a spec plus a reference implementation:** the product is the contract. Option 1 is "Implement Symphony according to
  the following spec" with your own agent; option 2 is the "experimental" Elixir build (`symphony/README.md#L21-L35`). The spec
  is written for agents: a "Cheat Sheet ... intentionally redundant so a coding agent can implement the config layer
  quickly" (`symphony/SPEC.md#L605-L609`), reference algorithms (`symphony/SPEC.md#L1799-L2047`), a test matrix split into core, extension and real
  integration profiles (`symphony/SPEC.md#L2049-L2063`), and a definition-of-done checklist (`symphony/SPEC.md#L2204-L2232`).
- **Deliberate non-goals:** rich UI or multi-tenant control plane, a mandated dashboard, a general workflow engine or
  distributed scheduler, built-in ticket-editing logic, mandated sandboxing, one approval posture (`symphony/SPEC.md#L60-L69`).
## Map to the three questions

### Q1 What do they offer versus theuth?
| Axis | Symphony | theuth | Gap |
|---|---|---|---|
| A1 identity | none; a role is one WORKFLOW.md + Codex config | roster files, desks, many harnesses | theuth ahead |
| A2 org | none; humans act through tracker states | reporting lines, escalation, reviewer, double | theuth ahead |
| A3 work | external tracker IS the queue; handoff states; two-call read kernel | own durable board, `done_when`, `external` refs | **tracker ingestion + handoff-state semantics** |
| A4 runtime | reconcile-first tick, stall kill + backoff, continuation turns, per-state caps, per-issue workspaces + hooks, SSH pool | wakes, checkout epochs, ACP, mesh, VMs, durable | **reconcile/stall/continuation loop; workspace hooks** |
| A5 coordination | tracker relations + one workpad comment; no messaging | send_message, mentions, threads | theuth ahead; borrow the workpad |
| A6 HITL | state change = verdict; unattended; no prompts reach humans | policy, inbox, double, review state | theuth ahead; borrow handoff brief |
| A7 cost | token/rate-limit telemetry only; unbounded re-dispatch | budgets + auto-pause | theuth ahead |
| A8 memory | workspace + thread + workpad; Confusions section | knowledge, dreaming, skills | theuth ahead |
| A9 UX | ANSI dashboard, LiveView, JSON API; real UX in Linear/GitHub | desktop/TUI/web/channels, monitor | theuth ahead; borrow per-item debug payload |
| A10 integration | 5 tracker adapters, provider-native tools, Codex only | MCP, ACP, A2A, channels | theuth ahead |
| A11 engineering | spec + 100 % coverage reference build; preview | larger product, own process | theuth could adopt spec-first contracts |

### Q2 What can we learn / borrow? (techniques only)
1. **Reconcile before dispatch, every tick.** Mechanism: before choosing new work, re-read every running item from the source
   of truth; terminal → stop + clean, non-active or unroutable → stop and keep, read failure → keep running
   (`symphony/SPEC.md#L819-L839`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L310-L333`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L421-L441`). Cost of being wrong: low; a missed tick only delays a stop by one
   interval, and keeping workers on read failure avoids flapping.
2. **Stall detection on last-event time, kill and retry with typed reasons.** Mechanism: per-run `last_event_at`, a threshold
   (5 min), terminate and requeue with exponential backoff; distinct terminal reasons (Succeeded/Failed/TimedOut/Stalled/
   CanceledByReconciliation) because "retry logic and logs differ" (`symphony/SPEC.md#L676-L692`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L581-L639`). Cost: medium; a
   too-short threshold kills long silent builds, so treat a live child process as activity.
3. **Bounded continuation turns that re-check the item.** Mechanism: after each completed turn re-fetch the item; continue on
   the same thread with a short fixed continuation prompt, never the full brief; stop at `max_turns`
   (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L101-L154`, `symphony/SPEC.md#L662-L674`). Cost: low, if paired with a per-item re-dispatch cap (Symphony lacks one, A7).
4. **Explicit retry timers with tokens.** Mechanism: one timer per item; a new retry cancels the old; a fire counts only if
   its token matches; delay 1 s for continuation, `10 s·2^(n-1)` capped for failures (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1034-L1091`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1243`).
   Cost: low.
5. **Stale-dispatch revalidation.** Mechanism: re-read the item by id immediately before spawning, and again when a retry
   fires (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L907-L938`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1093-L1111`). Cost: one extra read per dispatch.
6. **Tracker as queue through a two-call read kernel.** Mechanism: `fetch_by_states` + `fetch_by_ids` returning full
   normalised snapshots with an adapter-computed `dispatchable`; omission means "not visible"; ID refresh fails on malformed
   records (`symphony/SPEC.md#L1186-L1228`, `symphony/SPEC.md#L192-L195`). Cost: medium; tracker latency and rate limits become theuth's problem.
7. **Handoff states instead of "done".** Mechanism: success = the agent reached a review state outside the active set; human
   state changes (`Merging`, `Rework`) re-enter the active set and re-dispatch; `Rework` means a full reset
   (`symphony/SPEC.md#L43-L44`, `symphony/elixir/WORKFLOW.md#L107-L116`, `symphony/elixir/WORKFLOW.md#L253-L263`). Cost: low; it maps onto theuth's existing `review` state.
8. **Policy in the repo.** Mechanism: one versioned file with typed front matter (states, caps, hooks, harness command) and a
   strict template body (unknown variables fail); hot reload by mtime+size+hash every second, keep last-known-good on a bad
   edit, re-validate before each dispatch (`symphony/SPEC.md#L560-L603`, `symphony/elixir/lib/symphony_elixir/workflow_store.ex#L143-L176`).
   Cost: medium; it adds a second config source next to agent files.
9. **Deterministic per-item workspaces with lifecycle hooks.** Mechanism: `<root>/<sanitized key>[--hash]`, reuse across runs,
   containment + symlink-escape checks, four hooks with timeouts and explicit failure semantics, terminal cleanup at startup
   (`symphony/SPEC.md#L853-L948`, `symphony/elixir/lib/symphony_elixir/workspace.ex#L15-L91`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L150-L176`). Cost: low to medium (disk use; clean on terminal).
10. **Host-side tools + secret scrubbing.** Mechanism: tracker tools execute in the orchestrator with its credential; the
    adapter declares secret env names, which are removed from the child env and `unset` in the launch line; the tool set is
    bound to one session snapshot (`symphony/SPEC.md#L1091-L1132`, `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L221-L251`). Cost: low; theuth's vault placeholders already
    cover much of this.
11. **One living workpad per work item.** Mechanism: find-or-create one marked comment; env stamp `<host>:<path>@<sha>`;
    Plan / Acceptance Criteria / Validation / Notes / Confusions; edit in place; no separate "done" comments
    (`symphony/elixir/WORKFLOW.md#L79-L87`, `symphony/elixir/WORKFLOW.md#L295-L329`). Cost: low; a prompt and convention change.
12. **Blocked as a parking state.** Mechanism: input or approval requests stop the run and park the item, still claimed and
    visible, until a human changes its state (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L757-L779`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L456-L474`). Cost: low; add an explicit unblock action,
    which Symphony lacks.
13. **Per-state and per-host concurrency caps; least-loaded placement with retry affinity.** (`symphony/SPEC.md#L783-L788`,
    `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1281-L1341`). Cost: low.
14. **Spec-first contract written for agents.** Mechanism: RFC 2119 spec + reference algorithms + test matrix + definition of
    done, so any harness can implement a conforming orchestrator (`symphony/SPEC.md#L1799-L2232`). Cost: high effort, moderate value for
    theuth (it is not a protocol others implement); useful for the desk protocol if third parties should join.

### Q3 How should theuth improve? (ranked by value ÷ effort)
1. **Stop runs whose task left the active set (reconcile).** When a task moves to `cancelled`, `done` or `backlog`, or its
   owner changes, interrupt the desk or subagent working it, and say why on the thread. Seam: `crates/tasks` ledger moves
   (`task_move`) → `crates/roster` desk wake/interrupt. Effort S, value high (it removes wasted runs). Borrow 1.
2. **Stall detector for desk episodes and subagents.** Track the last activity per checkout; past a threshold, interrupt and
   release the wake with backoff (`release_wake(min_backoff_ms)` already exists, `theuth/crates/tasks/src/ledger.rs#L2064-L2064`);
   record a typed reason (stalled / timed out / cancelled by reconcile). Seam: `crates/roster` + `crates/tasks` wakes; surface it
   in the activity monitor (155). Effort S–M, value high. Borrow 2 + 4.
3. **Workpad convention for task threads.** One `progress`-flagged note updated in place, with plan, `done_when` checklist,
   validation evidence, env stamp and a Confusions section. Seam: desk manual in `crates/roster/src/desk.rs` + the `task_note`
   tool description. Effort S, value medium-high (review gets faster; dreaming gets hindsight). Borrow 11 (+ A8).
4. **Bounded continuation with a per-task re-dispatch cap.** When a desk episode ends with its task still `doing`, re-wake it
   with a short continuation prompt up to N times, then escalate to the reviewer instead of looping. Seam: roster wake loop +
   budgets. Effort M, value high (closes the runaway hole Symphony leaves open). Borrow 3.
5. **External tracker as the queue (Linear / Jira / GitHub).** A sensor or routine that runs the two-call read kernel, mirrors
   issues in configured states into tasks with `external` refs, maps tracker states onto theuth states (handoff → `review`),
   and stops runs on reconcile; writes go back through the existing Atlassian/GitHub MCP tools, executed host-side. Seam:
   `crates/schedule` sensors (watchers, design 40) + `crates/tasks` (`external` field at
   `theuth/crates/tasks/src/ledger.rs#L1065-L1069`). Effort M–L, value high for Sky's SRE desks, which already live in Jira.
   Borrow 6 + 7.
6. **Per-task workspaces with hooks.** Give each coding task a deterministic worktree (`<root>/<task-key>`) with
   after_create / before_run / after_run / before_remove hooks and cleanup when the task ends, instead of hand-cut worktrees
   per wave. Seam: subagent spawn `cwd` in `crates/agent` / `crates/acp`, optionally `crates/sandbox`. Effort M, value medium.
   Borrow 9.
7. **Repo-owned workflow file per project.** A versioned `.theuth/WORKFLOW.md` (states, caps, hooks, harness, strict prompt
   template) hot-reloaded with last-known-good, overriding agent defaults for tasks in that project. Seam: projects + roster
   agent files (67) + routines. Effort M, value medium. Borrow 8.
8. **Per-item debug payload and backoff queue view.** One RPC/UI card per task: workspace, attempts, current session,
   last events, last error, next retry time. Seam: activity monitor (155) + tasks board UI. Effort S–M, value medium. A9.
9. **Per-state concurrency caps** for desks working the same board (for example at most two in `review`). Seam: roster
   placement. Effort S, value low-medium. Borrow 13.
10. **Spec-first contract for the desk protocol.** Only if outside harnesses are to join as desks. Effort L, value low now.
    Borrow 14.

## Their weaknesses and failure modes
1. **Single instance only.** Claims, retries and blocked issues live in one process's memory (`symphony/SPEC.md#L280-L293`,
   `symphony/SPEC.md#L1690-L1704`); two instances on one board would double-dispatch, and a restart forgets blocked issues and re-dispatches
   them (`symphony/elixir/README.md#L33-L36`). Persistence is a spec TODO (`symphony/SPEC.md#L2239-L2239`); leases are an open PR (#82).
2. **Unbounded re-dispatch.** A clean exit always schedules a 1 s continuation at attempt 1 (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L216-L222`,
   `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1238`), and the prompt forbids ending a turn while the issue is active (`symphony/elixir/WORKFLOW.md#L50-L50`). An agent that cannot finish
   but never moves the ticket burns 20 turns per worker forever. Failures also retry forever at the 5 min cap.
3. **Silence-only liveness.** Stall and turn timeouts reset on any event (`symphony/SPEC.md#L1147-L1149`), so a busy loop is never caught.
4. **Tool scope = token scope.** `linear_graphql` runs any query or mutation the token allows, with no scope guard,
   idempotency key or rate-limit policy (`symphony/elixir/README.md#L229-L235`). The spec itself recommends narrowing tools to
   the tracker scope (`symphony/SPEC.md#L1789-L1792`); the reference build does not.
5. **Governance by prompt.** The review gate, state map and workpad rules are 329 lines of prompt (`symphony/elixir/WORKFLOW.md#L42-L329`); nothing
   enforces "human moves to Merging" (`symphony/elixir/WORKFLOW.md#L249-L249`).
6. **High-trust defaults in practice.** The shipped workflow runs `approval_policy: never` with network on and the full shell
   environment inherited (`symphony/elixir/WORKFLOW.md#L33-L39`), behind a CLI acknowledgement flag (`symphony/elixir/lib/symphony_elixir/cli.ex#L111-L125`).
7. **Blocked issues have no unblock path** except changing the tracker state or restarting; no API call does it
   (`symphony/elixir/lib/symphony_elixir_web/router.ex#L31-L41`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L456-L474`).
8. **Probable skipped `after_run` on kills** (A4.6) and **merged stderr** contrary to the spec (A4.4).
9. **Polling cost.** Every tick reads candidates, every running id, plus a revalidation read per dispatch and a refresh read
   after every turn (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L256-L264`, `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L907-L938`, `symphony/elixir/lib/symphony_elixir/agent_runner.ex#L156-L171`); the reference workflow polls every 5 s
   (`symphony/elixir/WORKFLOW.md#L18-L19`). No webhooks.
10. **Codex lock-in and a preview label.** Only Codex app-server is supported (`symphony/SPEC.md#L475-L478`); "low-key engineering
    preview" (`symphony/README.md#L10-L11`); 3 releases in 8 months, 34 of 47 commits by one author, issues closed to the
    public, activity slowing since July.
11. **`attempt` is ambiguous**: the prompt cannot tell a continuation from a failure retry (`symphony/SPEC.md#L1348-L1350`), which the
    workflow papers over with "It may be a normal continuation or a retry after a failure" (`symphony/elixir/WORKFLOW.md#L47-L47`).
## Cites self-check
Method: 89 load-bearing cites re-opened at the end in one pass. Each range was read back and checked for the keyword the
claim rests on; all 89 held. Six first showed as misses only because the `cat` output was truncated at about 1,700 lines in
`SPEC.md` and theuth's `ledger.rs`; re-read with `read_file`, all six held. Separately, every one of the report's cites was checked
for a line range that exists in its file: no problems. Corrections: no cite needed one. One claim was made more specific after a
final read: trace step 5's scrubbed variable is `LINEAR_API_KEY` (`symphony/elixir/lib/symphony_elixir/config/schema.ex#L401-L422`).

| Cite | Claim it carries | Result |
|---|---|---|
| `symphony/SPEC.md#L725-L731` | one authority; claimed/running checked before launch | held |
| `symphony/SPEC.md#L742-L752` | tick order: reconcile → validate → fetch → sort → dispatch → notify | held |
| `symphony/SPEC.md#L754-L769` | dispatch eligibility list | held |
| `symphony/SPEC.md#L797-L801` | 1 s continuation; 10 s·2^(n-1) failure backoff, capped | held |
| `symphony/SPEC.md#L819-L839` | stall pass + tracker refresh rules | held |
| `symphony/SPEC.md#L1144-L1149` | turn timeout is a silence window, not a total cap | held |
| `symphony/SPEC.md#L1186-L1206` | two required adapter reads | held |
| `symphony/SPEC.md#L1690-L1704` | scheduler state intentionally in-memory (re-read with read_file) | held |
| `symphony/SPEC.md#L2239-L2239` | TODO persist retry queue (re-read with read_file) | held |
| `symphony/SPEC.md#L431-L434` | after_run promised on cancellation | held |
| `symphony/SPEC.md#L1036-L1037` | keep stderr apart from protocol stream | held |
| `symphony/SPEC.md#L1789-L1792` | narrow tools to tracker scope (re-read with read_file) | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L208-L224` | normal exit → continuation retry attempt 1 | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1232-L1243` | continuation 1 s / failure backoff formula | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L581-L639` | stall kill, or block when input was requested | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L421-L441` | reconcile: terminal / unroutable / active / other | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L816-L829` | should_dispatch incl. not blocked | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L907-L938` | stale-dispatch revalidation | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L1075-L1091` | retry token match | held |
| `symphony/elixir/lib/symphony_elixir/orchestrator.ex#L757-L779` | blocked entry stays claimed | held |
| `symphony/elixir/lib/symphony_elixir/agent_runner.ex#L101-L154` | turn loop, max_turns, continuation guidance | held |
| `symphony/elixir/lib/symphony_elixir/agent_runner.ex#L45-L51` | after_run in an after clause | held |
| `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L55-L55` | auto-approve only when policy is never | held |
| `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L199-L209` | port opened with stderr_to_stdout | held |
| `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L314-L366` | thread/start with dynamicTools; turn/start | held |
| `symphony/elixir/lib/symphony_elixir/codex/app_server.ex#L857-L893` | auto-answers MCP approval questions | held |
| `symphony/elixir/lib/symphony_elixir/config/schema.ex#L180-L198` | default reject policy, workspace-write | held |
| `symphony/elixir/WORKFLOW.md#L7-L17` | active vs terminal states (Human Review in neither) | held |
| `symphony/elixir/WORKFLOW.md#L33-L39` | repo runs approval never, network on | held |
| `symphony/elixir/lib/symphony_elixir/linear/client.ex#L496-L511` | dispatchable = assignee match and no live blocker in Todo | held |
| `symphony/elixir/README.md#L229-L235` | linear_graphql reaches whatever the token can | held |
| `symphony/elixir/mix.exs#L11-L13` | 100 % coverage threshold | held |
| `theuth/crates/tasks/src/ledger.rs#L1405-L1415` | theuth checkout with conflict/epoch | held |
| `theuth/crates/tasks/src/ledger.rs#L2064-L2064` | theuth release_wake(min_backoff_ms) (re-read with read_file) | held |

Not verifiable without running: that kills skip `after_run`. This is inferred from Erlang exit semantics (A4.6). Also not
verified: whether any production deployment runs more than one instance per board. The "team via assignee filters"
reading (A1) is my inference, not a documented feature. I did not inspect the `crates/schedule` "stall" hits in theuth.
