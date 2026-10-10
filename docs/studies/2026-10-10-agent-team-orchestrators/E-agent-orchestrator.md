# Agent Orchestrator (Orchestrator.inc, ex-Composio) — source recon for the agent-teams study (2026-10-10)

Clone: `agent-orchestrator` (shallow, HEAD 4273bec, 2026-10-10 14:17 +0530, "fix: refuse to rename the project orchestrator (#6475)").
Read-only recon. Cites are `agent-orchestrator/<path>#L<a>-L<b>`; theuth cites are `theuth/<path>#L..`. Where docs and code disagree the code wins and the text says so.

## Executive summary

1. **What it is.** AO is a long-running local Go daemon that supervises parallel coding-agent sessions. It ships with an Electron desktop app, an Expo phone app, a thin `ao` CLI and a hosted "AO Cloud" control plane (Go, public). Licence Apache-2.0; 13.0k stars; created 2026-02-13.
2. **Unit of work.** A *worker* is one task, one harness (36 adapters compiled in), one git worktree and one PR loop. A per-project *orchestrator* is just another harness session whose system prompt says "coordinate, never implement". It plans, then spawns and steers workers through the `ao` CLI. There is no roster, org chart, budget or memory layer.
3. **Core discipline.** Observe → durable facts → derived status. Hooks, a terminal-screen reconciler, a 5 s runtime reaper and a 30 s SCM poller all feed one lifecycle reducer. Display status and Kanban lane are pure functions in a shared `contract` package and are never stored. A trigger-fed `change_log` drives SSE to every client.
4. **Idea 1, owner-routed feedback.** CI failures, each unresolved review comment, changes-requested reviews and merge conflicts become "nudges" pasted into the *same* worker. Each nudge is deduped by key and signature, has an attempt budget, and is persisted per PR so a restart does not re-nudge. Every condition is queued on its own, so one can never hide another.
5. **Idea 2, one guarded write path.** Every byte written into an agent's input goes through `sessionguard`. It re-reads the session just before writing and refuses when the paste could answer a permission dialog. An input lease closes admission during kills, switches and restores.
6. **Idea 3, attention that tidies itself.** The board lane is derived from "whose turn is it, AO's loop or a person's". Notifications are intents with an idempotency key. They are resolved only by lifecycle facts, never by hand, and reconciled after downtime.
7. **Also worth knowing:**
   - detached per-session provider hosts with an ACP relay and a prompt journal, so a daemon or app update does not kill an in-flight turn;
   - an isolated adversarial reviewer agent;
   - an issue-intake observer that starts one worker per open issue;
   - two-phase, revision-fenced termination ("a failed probe is not death");
   - a merge button guarded by the expected head SHA.
8. **Docs as contract.** `docs/documentation-map.md` splits human prose from machine contracts (OpenAPI, sqlc, `AGENTS.md`, skills) and says generated contracts win on drift. CI drift gates back it.
9. **Verdict.** AO is the strongest open-source *coding-PR supervisor* in the field. theuth should borrow its fact → derived-status discipline, PR feedback reactions, write guard and self-resolving attention. It should not copy its product shape: one human, one repo, prompt-only authority, no budgets, no memory.

## A1 Agent model & identity

**What an "agent" is.** AO has no agent record or persona. An agent is a *harness adapter* compiled into the daemon. A session is the pair (kind, harness) plus model, interface mode and workspace.
- **Registry.** `agent-orchestrator/backend/internal/adapters/agent/registry/registry.go#L1-L7` calls itself "the single source of truth for the agent adapters the daemon ships", so adding a harness is one edit to `Constructors()` (`#L54-L57`). That function has 36 `New()` entries: Claude Code, Codex, Cursor, opencode, Aider, Copilot, Gemini CLI, Pi, Amp, Goose, Kiro, Devin, OMP, OpenHands and more. README lists 27 by name (`agent-orchestrator/README.md#L130-L132`); the repo description says "+25 more".
- **Docs vs code.** `agent-orchestrator/docs/architecture.md#L246` still says "23+ agent harnesses". The code has more; trust the registry.
- **Session kind.** Exactly two values, `worker` and `orchestrator` (`agent-orchestrator/backend/internal/domain/session.go#L19-L25`). Reviewer runs are a third, short-lived actor with their own conversation owner kind (`agent-orchestrator/backend/internal/domain/conversation.go#L50-L53`).
- **Durable identity.** The `sessions` table holds `harness`, `session_mode`, `runtime_handle_id`, `provider_conversation_id`, `controller_generation`, `activity_state` and `is_terminated` (`agent-orchestrator/docs/architecture.md#L595-L606`).

**Capabilities are optional Go interfaces, not a manifest.** `agent-orchestrator/backend/internal/ports/agent.go#L77` is the base `Agent`. On top sit around 30 opt-in interfaces, for example:
- `AgentInterfaceHandoff` (`#L185`);
- `AgentModelDiscoverer` (`#L348`);
- `TerminalActivityDetector`, `ContinuousTerminalActivityDetector` and `WaitingTerminalActivityDetector` (`#L402-L436`);
- `BlockedActivitySignaler` (`#L506`);
- `ActiveTurnSteerer` (`#L525-L527`, "can this harness be steered mid-turn").

The documentation map tells contributors to "compare the worker, Chat-driver, and reviewer registries separately" for capability claims (`agent-orchestrator/docs/documentation-map.md#L40-L41`). Support is a per-harness matrix, not uniform.

**Roles are system prompts.**
- `orchestratorSystemPrompt` (`agent-orchestrator/backend/internal/session_manager/prompt.go#L197-L221`).
- `workerSystemPrompt`, whose task-source rules decide whether a PR is expected (`#L267-L275`).
- The native-review block (`#L367-L380`).
- Project rules come from inline config plus a repo-relative rules file. A missing file fails the spawn rather than silently dropping rules (`#L147-L150`).

**Lifecycle states.**
- `activity_state` ∈ active / idle / waiting_input / blocked / exited, plus `is_terminated` (`agent-orchestrator/backend/pkg/contract/status.go#L5-L15`).
- 15 derived display statuses (`#L17-L41`).
- Chat sessions can also be *hibernated*: the provider host is stopped after a settled idle turn and resumed on new work (`agent-orchestrator/docs/architecture.md#L67-L80`).

**Switching and handoff.**
- A session can move between the native TUI and structured Chat through a durable handoff. Both controllers are never live at once (`agent-orchestrator/docs/architecture.md#L16-L19`).
- A session can also be *switched to another harness* by a "deliberate, user-requested provider replacement" carrying an idempotency key (`agent-orchestrator/backend/internal/session_manager/agent_switching.go#L52-L58`). It is backed by a handoff artifact (`session_manager/handoff_artifact.go`, 726 lines).

**Hiring and templates.** Neither exists. "Adding" an agent means installing its CLI and letting AO probe it; several open issues are about that probe (#5887 "version probe: signal: killed"). There is no export/import of agents. The only catalog is the `using-ao` skill installed into workspaces (`agent-orchestrator/docs/documentation-map.md#L58`).

**vs theuth.** theuth's agent is a file with charter, `reports_to`, model, tools, budget and capabilities, held on a roster. AO has nothing comparable, so theuth is ahead on identity. AO is far ahead on harness *breadth* (36 adapters with per-harness hooks, model discovery and auth probes, against theuth's ACP omp/opencode). It is also ahead on *mid-session harness switching* with a handoff artifact.

## A2 Organisation & authority

**Topology is fixed at two levels.** One human, one orchestrator per project, N workers.
- Singleton orchestrator: a per-project mutex map (`agent-orchestrator/backend/internal/service/session/service.go#L202-L203`).
- The orchestrator gets its own worktree at `<managedRoot>/<project>/orchestrator/<prefix>-orchestrator` (`agent-orchestrator/backend/internal/adapters/workspace/gitworktree/workspace.go#L2178-L2187`).
- HEAD's own commit is "refuse to rename the project orchestrator".

**Authority lives in prompts, not in tool gates.** The orchestrator is told:
- "coordination-only by default";
- "Never ever make code changes directly in the orchestrator session";
- "Do not use the agent runtime's built-in subagent or task-delegation tools";
- "AO workers only";
- "Do not merge unless explicitly asked" (`agent-orchestrator/backend/internal/session_manager/prompt.go#L206-L221`, `#L257-L262`).

If the human insists, it must ask for explicit confirmation first (`#L212`). Workers message the orchestrator "only for true blockers, cross-session coordination, or decisions you cannot resolve locally" (`#L383-L391`). They ask it for extra workers instead of spawning native subagents (`#L294-L297`). I found no tool-level enforcement: an orchestrator harness with shell access can still run git.

**Escalation path:**
1. Worker → orchestrator: `ao send`.
2. Orchestrator → human: chat summaries ("Summarize status and blockers for the human", `#L240-L248`).
3. Any session → human: the "Needs you" zone and notifications (A6).

**Isolation.**
- Locally: projects plus per-session worktrees.
- Cloud: organisations with "organization membership checks backed by PostgreSQL row-level security" and WorkOS auth (`agent-orchestrator/cloud/README.md#L9-L13`).
- The phone's LAN listener serves the app API but "not daemon-control routes (shutdown/lifecycle stay Loopback-only)" (`agent-orchestrator/CONTEXT.md#L12-L15`).

**Humans.** Humans are the user plus GitHub reviewers. AO's own review approval is "internal. It is not a GitHub approval and does not satisfy required or independent-account reviews or branch protection" (`agent-orchestrator/backend/internal/session_manager/prompt.go#L262`).

**vs theuth.** theuth is well ahead: org chart, reporting lines, escalate up the line, desks, a reviewer on tasks and the double. Two small AO techniques are worth noting: role prompts written as *negative rules* ("never claim a PR into the orchestrator session"), and a per-project singleton lock on the coordinator.

## A3 Work model

**There is no task table.** The work item is the worker session plus the PRs it owns, plus optionally a tracker issue.
- `sessions ||--o{ pull_requests`, `pr_checks`, `pr_review_threads`, `pr_comments` (`agent-orchestrator/docs/architecture.md#L579-L583`).
- A delegated task is `DelegateTaskInput`: a brief (which "may be empty to open an idle worker") and an optional agent (`agent-orchestrator/backend/internal/service/session/delegation.go#L26-L28`).

**Creation paths.**
- **New Task modal.** Opening it creates a *speculative worktree*. The session id is reserved and the row stays hidden until Start Task claims it (`agent-orchestrator/backend/internal/session_manager/task_preparation.go#L18-L41`).
- **Orchestrator delegation.** `DelegateTask` "spawns the worker directly, matching `ao spawn`, with a provisional display name derived from the task brief", then refines the title in the background with a short call to the worker's own harness (`agent-orchestrator/backend/internal/service/session/delegation.go#L66-L69`). `--name` labels are mandatory and at most 100 characters (`prompt.go#L229-L230`).
- **Tracker as queue.** The opt-in `trackerintake` observer "polls a project's configured tracker for eligible issues and starts one worker session per issue" (`agent-orchestrator/backend/internal/observe/trackerintake/observer.go#L1-L3`). It lists open issues and filters by assignee: "" = any, `*` = assigned, `none` = unassigned, or a login (`#L183-L231`). It spawns `KindWorker` with `BuildIssuePrompt(issue)` and remembers `seen` issue ids (`#L202-L216`). Trackers are GitHub and GitLab (`backend/internal/adapters/tracker/{github,gitlab,multi}`).

**Dependencies are PR stacks.**
- `BuildStacks` marks a PR blocked when its target branch is another open PR's source (`agent-orchestrator/backend/pkg/contract/status.go#L158-L176`).
- The session's PR status is the *worst* actionable status across open PRs. Stacked children count only when they carry an actionable signal (`#L197-L229`).
- Branch-namespace conventions (`ao/<session-id>`, `<branch>/<topic>`) let AO attribute every PR a worker opens to that worker (`agent-orchestrator/backend/internal/session_manager/prompt.go#L393-L405`).

**Acceptance and review.** There is no `done_when`. "Done" is SCM state:
- A merged PR terminates the session only when no open PR remains, and never while the agent is still active (#2879). A session may be "raising the next PR" (`agent-orchestrator/backend/internal/lifecycle/reactions.go#L81-L118`).
- Review is AO's *native adversarial reviewer* (A5). It can start automatically once a worker has been idle for 1 minute, with at most 3 retries per head (`agent-orchestrator/backend/internal/autoreview/coordinator.go#L18-L22`).

**Planning and decomposition.** These live in the orchestrator's conversation: "For complex planning ... write a short plan first" (`prompt.go#L215`). There is no plan or goal object.

**Recurring work.**
- `service/automation` holds durable recurring definitions with an embedded IANA tz database.
- `domain/cue.go` defines "cues": command cues and agent-prompt cues.
- An automations observer advances due work (`backend/internal/observe/automations/observer.go`).

**vs theuth.** theuth's board is richer: states, owner, reviewer, `done_when`, threads and escalation. What theuth lacks:
- *work state derived from external facts* (PR/CI/review), so the lane cannot drift from reality;
- a first-class "tracker issue → one worker" intake loop.

## A4 Execution & runtime

### A4.1 The daemon and its boundaries

- **The daemon.** "A long-running Go daemon that supervises multiple parallel AI coding-agent sessions." Project sessions own isolated git worktrees; standalone workers own AO-managed plain directories (`agent-orchestrator/docs/architecture.md#L3-L6`).
- **Load-bearing rules** (`#L1141-L1155`):
  - never store display status;
  - never treat failed probes as death;
  - never force-delete dirty worktrees;
  - keep all app state under `~/.ao`;
  - keep the primary daemon on loopback;
  - keep the CLI thin;
  - CDC is the source of truth for events;
  - adapters are leaves;
  - hooks stay gitignored;
  - never change migrations.
- **Package layout.**
  - `domain`, `ports`, `service/*`, `session_manager`, `lifecycle` (the "durable session fact reducer");
  - `observe/{scm,reaper,activity,trackerintake,usage,automations,artifacts}`;
  - `storage/sqlite`, `cdc`, `httpd`;
  - `adapters/{agent,chatdriver,runtime,workspace,scm,tracker}` (`#L223-L254`, plus `ls backend/internal/observe`).

### A4.2 How agents are driven: two interface modes, 36 harness adapters

**TUI mode.** The harness's native terminal UI runs "inside a native PTY/ConPTY runtime, with tmux as a legacy or fallback runtime" (`agent-orchestrator/docs/architecture.md#L8-L9`). The PTY layer sizes the PTY from birth, avoiding a SIGWINCH race. Its `Close` gives up after a kill grace so a D-state process on a dead mount cannot hang teardown (`backend/internal/adapters/runtime/ptyexec/spawn_unix.go`, `ptyexec/wait.go`).

**Chat mode.** A native protocol controller with no terminal. The drivers in `backend/internal/adapters/chatdriver/` are a reusable `acp` transport plus `claudeacp`, `codexappserver`, `cursoracp`, `droidacp`, `geminiacp`, `kimiacp`, `kimchiacp`, `ompacp`, `opencodeacp`, `opencodev2acp`, `piacp`, `qwenacp`, `deepseekharnessacp`, `nativeacp` and `unrealagent`. ACP is AO's main structured driver, as in theuth.

**Detached provider hosts survive daemon replacement.** "Codex and all ACP Chat processes run in detached per-session hosts, so replacing the daemon or desktop app does not stop an in-flight turn. The ACP host also preserves connection setup, JSON-RPC correlation, pending interactions, and acknowledged prompt replay" (`agent-orchestrator/docs/architecture.md#L10-L14`). In code:
- `persistenthost` "keeps a provider stdio process alive while AO's daemon is replaced" (`agent-orchestrator/backend/internal/adapters/chatdriver/persistenthost/host.go#L1-L4`).
- A `ProtocolVersion` fences daemon/host compatibility, and a 1 s handshake "must not stall daemon recovery" (`#L30-L37`).
- The relay "is deliberately only a JSON-RPC correlation layer ... the host owns only the connection state that cannot be reconstructed after a daemon dies" (`persistenthost/acp_relay.go#L58-L60`).
- A bounded, host-owned WAL journals provider notifications during one active prompt. The host appends bytes *before* delivery; the daemon owns normalisation and dedup (`persistenthost/acp_journal.go#L15-L18`).
- On Windows the bridge sits in a kill-on-close Job Object (`persistenthost/child_windows.go`).

**Mode handoff.** A durable handoff moves a compatible native conversation between TUI and Chat, and "both controllers are never live at once" (`agent-orchestrator/docs/architecture.md#L16-L19`). Checkpoints sit in `session_interface_transitions`, and messages that arrive during the gap go into `session_interface_transition_messages` (`#L574-L575`).

### A4.3 Watching agent activity: hooks first, screen second, "no signal" when both are silent

1. **Hooks are installed into the worktree, not the user's global config.** For Claude Code, AO writes `.claude/settings.local.json` with commands prefixed `ao hooks claude-code ` and a 30 s timeout (`agent-orchestrator/backend/internal/adapters/agent/claudecode/hooks.go#L11-L16`). It covers 11 events: SessionStart with a `startup|resume|clear|compact|fork` matcher, UserPromptSubmit, Pre/PostToolUse(+Failure), PermissionRequest, Stop, Notification, SubagentStart/Stop and SessionEnd (`#L42-L54`). User-defined hooks are preserved. For worker sessions `ao hooks` writes nothing to stdout, so "installing it never injects a permission decision" (`#L36-L41`). Load-bearing rule 9 requires every adapter-written file to be gitignored (`agent-orchestrator/docs/architecture.md#L1154`).
2. **The hook CLI is best-effort.** It "must never break the user's agent, so a non-AO session (no AO_SESSION_ID), an event that carries no activity signal, or an unreachable daemon all exit 0" (`agent-orchestrator/backend/internal/cli/hooks.go#L544-L551`). Session identity comes from `AO_SESSION_ID`, validated against a pattern before it is used in a request path. Reviewer sessions use `AO_REVIEW_SESSION_ID` (`#L571-L584`).
3. **Hooks are authoritative; the screen only reconciles staleness.** Claude's `DetectTerminalActivity` reports idle "only when Claude's rendered surface positively shows an idle composer". It exists because "a turn that dies without emitting its Stop hook (auth expiry, CLI crash, network loss) leaves the session stuck active forever" (`agent-orchestrator/backend/internal/adapters/agent/claudecode/terminal_activity.go#L10-L21`). It also maps "login expired … run /login" on screen to `waiting_input` (`#L22-L31`). The activity observer polls every 30 s. For non-continuous detectors it reads the screen only after an `active` state has been quiet for `DefaultStaleAfter = 2 min` (`agent-orchestrator/backend/internal/observe/activity/observer.go#L15-L16`, `#L120-L135`). Its write carries `ExpectedRevision` and the runtime `LaunchID`, so a stale read cannot overwrite a newer hook fact (`#L150-L157`).
4. **Subagent-aware activity.** AO keeps "the current parent turn and native subagent IDs ... per runtime launch so activity stays active while a background subagent works" (`agent-orchestrator/docs/architecture.md#L55`; `backend/internal/lifecycle/subagent_activity.go`, 300 lines).
5. **`no_signal` instead of a false "idle".** `noSignalGrace = 90 s`. After it, "a silent session is indistinguishable from one with a broken hook pipeline, and the dashboard must not claim a confident 'idle'" (`agent-orchestrator/backend/internal/service/session/status.go#L10-L17`). The predicate requires signal expected, never signalled and quiet past grace (`agent-orchestrator/backend/pkg/contract/status.go#L139-L144`).

### A4.4 Watching source control

- The SCM observer ticks every 30 s; review threads refresh every 2 min (`agent-orchestrator/backend/internal/observe/scm/observer.go#L30-L33`).
- It fetches PRs "via ETags", discovers new PRs, then checks and review threads (`agent-orchestrator/docs/architecture.md#L834-L857`).
- It writes rows and then calls `lifecycle.ApplySCMObservation` (`agent-orchestrator/backend/internal/lifecycle/reactions.go#L436-L440`).
- With no credentials it runs in "disabled mode" rather than failing (`architecture.md#L840-L841`).

### A4.5 Status derivation: stored facts, computed status

"Display status is never stored. It is computed at read time from durable facts" (`agent-orchestrator/docs/architecture.md#L49`, `#L53-L65`). The derivation is one pure function in the public `backend/pkg/contract` package, shared with Cloud and the TS clients (`agent-orchestrator/backend/pkg/contract/status.go#L107-L137`). Precedence:
1. terminated (merged if every PR merged);
2. active → `working`;
3. exited;
4. waiting_input/blocked → `needs_input`;
5. else the worst PR pipeline status;
6. else `no_signal` or `idle`.

PR severity runs ci_failed < changes_requested < commented < draft < review_pending < pr_open < approved < mergeable (`#L231-L252`).

**Docs vs code.** The architecture diagram (`architecture.md#L671-L702`) checks `waiting_input/blocked` *before* `active` and never mentions `exited`. Code returns `working` first and has a separate `exited` status (`status.go#L120-L127`). Trust the code.

### A4.6 Persistence and CDC

- SQLite with 175 migrations under `backend/internal/storage/sqlite/migrations/`, sqlc-generated queries, and "never change migrations".
- 195 `CREATE TRIGGER` statements append INSERT/UPDATE/DELETE rows (old/new JSON) to `change_log`. The first ones are in `agent-orchestrator/backend/internal/storage/sqlite/migrations/0001_init.sql#L123-L147` (`sessions_cdc_insert`, `sessions_cdc_update`, `pr_cdc_insert`).
- The poller ticks every 100 ms (`agent-orchestrator/backend/internal/cdc/poller.go#L13`). It "holds only an in-memory cursor ... it is the LIVE push path, while durable catch-up is the client's job (read change_log from its own offset). A restart re-seeks to head, so the poller never re-broadcasts history" (`#L25-L30`).
- The broadcaster fans out to terminal fan-out, the SSE writer and cache invalidation (`agent-orchestrator/docs/architecture.md#L645-L661`). `cdc/retention.go` prunes the log.

### A4.7 Liveness, death and reconciliation

- **Reaper.** Every 5 s it probes each non-terminated session's runtime handle. Chat sessions have none and are skipped (`agent-orchestrator/backend/internal/observe/reaper/reaper.go#L21`; `architecture.md#L859-L890`).
- **Runtime alive, workload dead.** If the tmux/PTY is alive but the agent process is dead, the session becomes `exited`, not terminated (`agent-orchestrator/backend/internal/lifecycle/manager.go#L478-L486`).
- **Two-phase, fenced termination.**
  1. Decide `runtimeClearlyDead` (probe says dead *and* no recent activity inside the window: `agent-orchestrator/backend/internal/lifecycle/runtime.go#L25-L28`), and skip if a session mutation is in progress.
  2. Finalise usage.
  3. Re-check the *same* predicates against `Revision` and `RuntimeLaunchID` before committing `is_terminated`.
  4. Release tool-flight state and reap the session's labelled Docker containers (`manager.go#L462-L533`).
- **The principle.** "Failed probes are NOT proof of death" (`architecture.md#L826`). Open issue #5790 ("session shows Exited while its agent is still running") shows this is still hard.

### A4.8 One workspace per task

- **Path layout.** `<managedRoot>/<project>/<session>`. A path-component validator rejects `/`, `\`, `.` and `..` so ids cannot escape the managed root (`agent-orchestrator/backend/internal/adapters/workspace/gitworktree/workspace.go#L2163-L2187`).
- **Never destroy uncommitted work.** `Destroy` refuses dirty worktrees with `ErrWorkspaceDirty`. It tells "not dirty" apart from "couldn't check", because "a failed probe must stay visible" (`#L943-L963`). `ForceDestroy` is "only safe to call AFTER the session's uncommitted work has been captured via StashUncommitted" (`#L973-L980`).
- **Stale registrations.** Recovered with `worktree add --force`, which still refuses a non-empty path, so it "can never clobber a live worktree" (`gitworktree/commands.go#L21-L30`).
- **Teardown off the request path.** Removing node_modules or target took "~2s per 60,000 files" and blew the 60 s REST timeout, leaving half-dead sessions. The directory is now *moved to a trash* inside the request and deleted later (`gitworktree/discard.go#L13-L30`).
- **Locks and retries.** Per-repository teardown locks guard the shared `.git/worktrees`. Windows sharing-violation errors are retried (`gitworktree/remove.go`).
- **Speculative pre-creation.** The New Task modal starts the worktree while the user is still typing (`session_manager/task_preparation.go#L18-L41`).
- **Docker cleanup by label.** Workers are told to label containers `ao.session=$AO_SESSION_ID` (`ao.spare=true` to keep one). AO force-removes labelled containers when the session ends (`agent-orchestrator/backend/internal/session_manager/prompt.go#L412-L420`; reap hook `lifecycle/manager.go#L189`, `#L527-L533`).
- **Standalone workers.** They get "AO-managed branchless directories" (`agent-orchestrator/README.md#L92`).

### A4.9 An isolated browser per worker

- Electron owns one `WebContentsView` per session and attaches its debugger only to that view. The transport "cannot enumerate or attach to the AO renderer or a different session", and no unauthenticated remote-debugging port is opened (`agent-orchestrator/docs/architecture.md#L1116-L1127`).
- Temporary profiles isolate sessions by default. Named profiles share cookies deliberately.
- Request capture is opt-in, at most 200 metadata entries, expires within 5 minutes, and never stores bodies (`#L1129-L1137`).
- The loopback `/api/v1/browser` surface is blocked on the LAN listener (`#L1126-L1127`).

### A4.10 Delivery, hibernation, resources

- **Delivery.** `WaitForMessageDeliveryReady` blocks until a TUI "can safely accept an injected message", because a TUI still booting can silently discard a paste (`agent-orchestrator/backend/internal/session_manager/message_delivery.go#L19-L21`). Every write then passes `sessionguard` (A5).
- **Chat hibernation.** Off by default; enabled through Developer Mode. Only an idle session after a settled turn, with no open Chat view, is stopped. The view lease renews every 10 s (`architecture.md#L67-L80`).
- **Resources.** `procmem` snapshots the whole process table once "so many sessions can be costed from a single `ps` run" (`agent-orchestrator/backend/internal/procmem/procmem.go#L1-L3`). `procmem/system.go` answers "can the machine take more?" (`#L10-L11`). These are readings, not caps: per-spawn memory ceilings are open issue #2523.

### A4.11 Placement

**Local.** Everything runs on one machine. There is no mesh.

**AO Cloud.** Workers run in remote sandboxes.
- Local dev provisions sibling Docker containers with a per-session volume (`agent-orchestrator/cloud/README.md#L40-L45`). There are `cloud/coder` and `cloud/coder-azure-vm` directories.
- A reconciler "converges durable sandbox intent with provider reality. HTTP handlers record intent only; every slow provider call happens here" (`agent-orchestrator/cloud/internal/reconcile/reconciler.go#L1-L3`).
- `ErrNotFound` is "the only error the reconciler accepts as proof that an environment is gone" (`agent-orchestrator/cloud/internal/sandbox/provider.go#L11-L13`).
- `idlepause` pauses sandboxes whose session went quiet (`agent-orchestrator/cloud/internal/idlepause/idlepause.go#L1`).

### A4.12 vs theuth

**theuth is ahead on:**
- placement (mesh, remote nodes, sandbox VMs);
- the scheduler (routines, cron/every/until, sensors);
- wake semantics for long-lived desks.

**AO is ahead on:**
- per-task worktree management with a strict no-data-loss policy. theuth only manages worktrees for autoresearch studies (`theuth/crates/autoresearch/src/git.rs#L3`);
- hook-driven activity facts with screen-proven staleness and an explicit `no_signal`;
- revision- and launch-fenced termination;
- provider hosts that outlive an engine restart (theuth's ACP children die with the engine; see design 146);
- an isolated browser per worker.

## A5 Coordination between agents

**Hub and spoke through the CLI.**
- The orchestrator's toolkit is `ao status`, `ao session ls/get`, `ao spawn --project --name --prompt|--issue [--agent] [--model]`, `ao send --session --message`, `ao session claim-pr`, `ao session kill`, and `ao review trigger/ls` (`agent-orchestrator/backend/internal/session_manager/prompt.go#L222-L239`).
- "Use `ao send` for session communication. Do not bypass AO by writing directly to tmux, PTY, pipes, or runtime internals" (`#L220`).
- Workers never talk to each other. They go through the orchestrator (`#L294-L297`, `#L383-L391`).
- The CLI is thin; all logic is in the daemon. Its catalog is a skill embedded in the binary and installed into each workspace (`backend/internal/skillassets/using-ao/commands/*.md`, 17 command pages).

**The feedback loop: PR, CI and review routed back to the same agent.** This is the core of AO.
1. **Observe.** The SCM observer persists PR rows, checks, reviews and threads, then calls the lifecycle reducer (`agent-orchestrator/docs/architecture.md#L531-L561`).
2. **Queue every condition on its own.** "A single PR can trip several actionable conditions at once ... Queue every applicable nudge and send them together, so each surfaces independently instead of one returning early and hiding the rest — the bug this reducer had" (`agent-orchestrator/backend/internal/lifecycle/reactions.go#L158-L164`).
3. **CI.** On `CIFailing`, and only when the PR's `AutoInjectCI` flag is set, one nudge lists the failed checks. Its signature is the failure set and attempts are unbounded (`#L180-L199`).
4. **Review comments.** One nudge *per unresolved comment* with `AutoInjectReview`, budget `reviewMaxNudge = 3` (`#L18`, `#L202-L223`). One per changes-requested review (`#L226-L240`).
5. **Merge conflicts.** "Rebase onto the base branch and resolve them". Sent only for the *bottom* of a stack, because stacked PRs conflict against their parent until it merges. Marked `urgent` (`#L251-L267`).
6. **Delivery gate.** CI and review nudges are deferred while the agent sits in needs-input. Merge conflicts pass through because the parked human may be the one who must act. Even then, the urgent path refuses while a live permission dialog is on screen (`#L144-L157`).
7. **Dedup and persistence.** `sendOnce` returns early when `seen[key] == sig` or the attempt budget is spent (`#L943-L949`). Order is send → in-memory → persist. "A persist failure that survives until a daemon restart degrades to one extra nudge — preferred over the inverse ... which would silently lose a real nudge" (`#L981-L986`). State is JSON `{seen, attempts}` in `pr.last_nudge_signature`, "so the daemon [can] restart and resume the existing dedup state without re-nudging an agent" (`#L41-L47`). A *suppressed* send is not stamped delivered and re-fires later (`#L911-L927`).

**The write guard (sessionguard).**
- "Every pane-writing path (user sends, post-send Enter nudges, lifecycle reaction nudges) funnels through this guard ... re-read the session immediately before writing and refuse when the paste could land somewhere only the user may act". The runtime appends Enter, so a stray paste "would answer the decision on the user's behalf — an unrecoverable action" (`agent-orchestrator/backend/internal/sessionguard/guard.go#L1-L9`).
- Outcomes are typed. The zero value `SuppressedUnknown` means a forgotten assignment can never read as a send. The others are `Sent`, `SuppressedTerminated`, `SuppressedExited` (the pane may now be a shell), `SuppressedAwaitingUser`, `SuppressedBusy` (mid-turn on a harness that cannot steer), `SuppressedInputGated` and `SuppressedStartupPending` (a pre-session dialog such as Cursor's MCP approval) (`#L37-L75`).
- An `InputLease` is acquired before each write and held until it returns. Mutations "close admission first and then wait for already-issued leases to drain, eliminating the check-then-write race" (`#L28-L35`).
- `Deliver` refuses only at `blocked`. `Nudge` also refuses at `waiting_input` (`#L130-L148`).

**Ownership and locks.**
- **PR ownership.** `ao session claim-pr <worker> <pr>`; orchestrators "must pass the target worker session explicitly" (`prompt.go#L235`). A review-only task must *not* claim, "because claiming mutates AO ownership metadata" (`#L274`).
- **Locks.** One orchestrator per project (`service/session/service.go#L202-L203`). Per-repo teardown locks. Input leases.

**Merge handling: human-triggered and head-guarded.**
- The orchestrator reports "green and approved" work but never merges unless asked (`prompt.go#L261`).
- The desktop's merge action:
  1. requires an `ExpectedHeadSHA`;
  2. rejects draft, merged or closed PRs;
  3. compares the expected head against the stored head;
  4. *re-fetches* the PR and compares again;
  5. checks readiness;
  6. squash-merges with the SHA as the provider precondition (`agent-orchestrator/backend/internal/service/pr/action_service.go#L103-L141`; GitHub side "guarded by the reviewed head", `agent-orchestrator/backend/internal/adapters/scm/github/merge_action.go#L19-L21`).
- There is no merge queue. Conflicts are handled by the rebase nudge to the owner.

**The native reviewer.** AO's reviewer is an isolated, headless AO agent over the worker's worktree.
- It posts a provider review with one inline comment per required change. Those comments then flow back to the worker as review-comment nudges (`prompt.go#L367-L380`).
- In headless reviewer sessions nobody can answer a dialog, so the PermissionRequest hook *answers for it*. The default is `deny`, and non-Bash tools stay denied (`agent-orchestrator/backend/internal/cli/hooks.go#L515-L527`).
- Reviewers with different harnesses can run on the same head at once (`prompt.go#L377`).
- Kanban facts are scoped to the *current head*: "a stale run can never decide the column" (`agent-orchestrator/backend/pkg/contract/kanban.go#L45-L65`).

**vs theuth.** theuth's `send_message` mesh with A2A at the edge is more general: peers, children, siblings and reply_to. AO wins on *closing the loop without a coordinator turn*. A CI failure reaches the owner within one poll, with no LLM in the path. theuth's nearest equivalent is the `pr_watch.py` watcher sensor, which wakes a *routine*, not the owning session (`theuth/crates/schedule/src/lib.rs#L3570-L3583` test fixture; `~/.theuth/sensors/pr_watch.py`).

## A6 Human in the loop & governance

**Approvals stay in the harness.** AO does not run a permission policy. It *detects* a pending decision (PermissionRequest hook → `blocked`; Chat approvals → `waiting_input`) and surfaces it as `needs_input` and "Needs you". It guarantees it will never paste into it (sessionguard, A5).

**Security gaps.** Open security issues show what this costs: #4840 "Codex Default permission falls through to bypass approvals and sandbox", and #2771 "markdown preview served same-origin with no CSP enables RCE via postCreate config write".

**Notifications are derived intents with automatic resolution.**
- **Types.** needs_input, ready_to_merge, pr_merged, pr_closed_unmerged, review_completed, review_changes_requested (`agent-orchestrator/backend/internal/domain/notification.go#L13-L23`).
- **Intents.** Each carries a `SourceKey` that "makes one terminal fact idempotent across retries and restarts", plus enrichment hints so the hot path does no storage reads (`agent-orchestrator/backend/internal/ports/notifications.go#L12-L29`). Duplicate unread rows are a no-op (`agent-orchestrator/backend/internal/notify/manager.go#L81-L83`).
- **Resolution.** "Resolution is always derived from a lifecycle fact — the session got its input, the PR stopped waiting on a merge — never from a user action, so there is no manual counterpart" (`#L110-L114`). PR-scoped resolutions outlive session churn (`ports/notifications.go#L31-L42`). The lifecycle computes ready-to-merge resolutions alongside each SCM observation (`lifecycle/reactions.go#L464-L476`).
- **Reconcile.** At startup it "closes notifications whose resolving transition happened while the daemon was down ... without this a restart could strand a needs-input row as unresolved forever" (`notify/manager.go#L145-L148`).

**Phone.**
- The LAN listener is off by default and exists only while Connect Mobile is on (`agent-orchestrator/CONTEXT.md#L12-L15`).
- A single rotating 8-character Connection Password; the v2 Pairing QR carries it as a bearer token in the URI fragment (`#L24-L34`).
- Per-source lockout, "so a hostile device cannot lock out the real phone" (`#L46-L48`).
- Push goes out through Expo. Muted devices stay listed but get nothing (`agent-orchestrator/backend/internal/push/dispatcher.go#L131-L139`).
- Phone "presence" comes from the app's 8 s REST poll with a 20 s TTL, not from a socket (`agent-orchestrator/backend/internal/presence/tracker.go#L3-L18`).

**Review gates.** AO's approval is explicitly not a GitHub approval and "never authorizes merging" (`prompt.go#L262`, `#L380`).

**Audit.** `change_log` stores old/new JSON for every change to tracked tables (`architecture.md#L635-L642`), with retention. There is no separate human-readable audit trail.

**vs theuth.** theuth's `tools.policy`, approval cards, the double, the attention inbox and the HUD are a far richer governance layer. The pieces worth borrowing are *auto-resolving, source-keyed attention rows*, *startup reconcile*, and *the guarantee that the harness never types into a pending permission prompt*.

## A7 Cost, budgets & runaway control

**No spend budgets, caps or kill switch.** `rg -i budget` over `backend/internal` hits only context and prompt budgets (`handoff_artifact.go`, `cue.go`, `dto.go`); I found no spend limit.

**Usage is measured, not limited.**
- An exact-file transcript watcher feeds per-harness parsers (`parser.go` 925 lines, `parser_kimi.go`) and an ingestor with durable cursors. It watches exact files because directory watches on kqueue "can open every file inside it" (`agent-orchestrator/backend/internal/observe/usage/watcher.go#L28-L31`).
- `SummaryReader` "derives token and estimated-cost summaries" (`agent-orchestrator/backend/internal/service/usage/summary.go#L20`).
- Prices come from a vendored catalog generated from LiteLLM's `model_prices_and_context_window.json`, pinned per provider by sha256 (`agent-orchestrator/pricing/catalog/v1/manifest.json#L1`) and checked by `pricing-catalog-validate.yml`.

**Runaway defences are local and per loop.**
- Review-comment nudges: at most 3 attempts.
- CI nudges: deduped by failure-set signature.
- Auto-review: at most 3 failed retries per head, and only after 1 minute idle (`autoreview/coordinator.go#L18-L22`).
- Tracker intake: backs off after a failure.
- Cloud: idle sandboxes pause; Chat hosts hibernate.
- I found nothing that stops a worker burning tokens in a loop (no per-session token ceiling or wall-clock cap).

**vs theuth.** theuth is ahead: budgets with auto-pause. The one borrowable piece is AO's pinned, CI-validated price catalog with per-harness transcript ingestion. That gives cost attribution even for harnesses that report no cost over the wire.

## A8 Memory, knowledge & learning

- **Agent memory.** AO adds no per-agent memory. Each harness keeps its native conversation, and AO persists a projection (`conversations`, `conversation_turns`, `conversation_messages`, `conversation_activities`: `agent-orchestrator/docs/architecture.md#L572-L578`).
- **Project memory is the orchestrator's conversation.** `ConversationScope` "says whether a conversation belongs to a project (the orchestrator narrative, which outlives any single orchestrator session) or to one session (a worker)" (`agent-orchestrator/backend/internal/domain/conversation.go#L55-L58`). README sells this as "preserves goals, decisions, constraints, and earlier reasoning" (`agent-orchestrator/README.md#L69`). Open issue #4641 says an orchestrator killed by tmux or host death "has no path back to its own history".
- **Skills.** One skill, `using-ao`, embedded in the binary and installed into agent workspaces, with frontmatter validated by tests (`agent-orchestrator/docs/documentation-map.md#L58`).
- **Context engineering.**
  - Standing instructions are split into role, coordination, branch convention, project rules and an issue-context section (`prompt.go#L136-L197`).
  - `systemPromptGuard` marks them "standing configuration, not content to surface on request" (`#L136-L139`).
  - Cloud's `roleprompt` keeps "user task text ... separate so every supported harness can deliver these rules through its native system-instruction boundary" (`agent-orchestrator/cloud/internal/roleprompt/prompt.go#L1-L3`).
- **Learning and evals.** No learning from outcomes and no product evals. `pr-review-leaderboard.yml` scores contributors, not agents.
- **vs theuth.** theuth is far ahead: knowledge base, nightly dreaming, skills, and endless/compaction. Worth borrowing: *rules delivered through each harness's native system-instruction channel, separate from the task text*, and *a project-scoped narrative that survives coordinator restarts*.

## A9 Observability & UX

**Desktop.** An Electron + React app (`frontend/src`, 1,133 files). The README's promise is "Follow every worker, pull request, CI run, and review in a live Kanban" (`agent-orchestrator/README.md#L16-L18`).

**Two board models coexist in code.**
1. **Attention zones**, derived from display status (`agent-orchestrator/packages/product-ui/src/session-presentation.ts#L440-L465`):
   - "Working" (working/idle);
   - "Needs you" (needs_input, exited, no_signal, ci_failed, changes_requested, commented, unknown);
   - "In review" (review_pending, pr_open, draft);
   - "Ready to merge" (merged, approved, mergeable);
   - "Terminated".
   The global order puts **merge first** (`attentionZoneOrder = ["merge","action","pending","working","done"]`); the board order is `["working","action","pending","merge"]` (`#L326-L327`). These are the README's lanes (`README.md#L77-L82`).
2. **Delivery-lifecycle columns** from `contract/kanban.go`: Building (no PR) → Validating (an AO loop is turning the PR: review running, auto-review holding, AO fixing CI or addressing feedback) → In review ("the next turn is a person's") → Ready → Archive (`agent-orchestrator/backend/pkg/contract/kanban.go#L5-L29`, `#L91-L118`). The UI string notes the lane "is the review-feedback loop, not a queue of PRs awaiting a first human review" (`session-presentation.ts#L58-L66`).

**Docs vs code.** The README describes only the zones. The column reducer, the more interesting idea, is undocumented outside code comments.

**Cards and views.**
- A card carries task, agent, branch, activity, PR and status. Opening it gives chat or terminal, changed files, PR summary, reviews and preview (`README.md#L84`).
- File changes refresh through a bounded fsnotify invalidation stream (`backend/internal/workspacewatch/watcher.go`).
- In-app deep links `ao://sessions/{project}/{session}` are taught to the orchestrator (`prompt.go#L250-L255`).

**Other surfaces.**
- **Phone.** `packages/mobile`, an Expo app of 530 files, over the authenticated LAN listener with a terminal mux. Cloud mode for mobile is still an open P2 (#4586).
- **CLI.** `ao status` and friends. `ao mcp` gives chat agents `html_preview`/`html_render` (A10).
- **Telemetry.** PostHog product telemetry, with an opt-out documented in `docs/telemetry.md`. Sentry errors carry an "observability owner" annotation so exactly one layer reports each failure (`backend/internal/observe/ownership/ownership.go`).
- **Onboarding.** "Open Orchestrator.inc and point it at the repository ... The desktop app runs the daemon for you, so no CLI is required" (`README.md#L53`).

**vs theuth.** theuth has more surfaces: TUI, web, Slack, Signal, Telegram, HUD, activity monitor (155) and attention inbox. AO's edges:
- the board is *about the delivery loop*, with each card bound to one PR pipeline;
- ready-to-merge is ranked first in attention order;
- the isolated browser sits beside each worker.

## A10 Extensibility & integration

- **Harness adapters.** Compiled-in Go packages; no plugin ABI or manifest. The `docs.orchestrator.inc/plugins/agents` pages are setup guides (`agent-orchestrator/README.md#L195`). Adding a harness means an adapter package, a `domain.AgentHarness` constant and one registry line (`registry.go#L1-L7`).
- **ACP.** The reusable `chatdriver/acp` transport carries about 15 harness-specific drivers, so any ACP harness is cheap to add, including OMP (`chatdriver/ompacp`).
- **SCM and trackers.** GitHub and GitLab for both, with a `multi` router that dispatches by repo provider (`backend/internal/adapters/scm/multi/merger.go`). Cloud adds a GitHub App with "durable webhook processing" (`agent-orchestrator/cloud/README.md#L18-L19`). I found no Linear or Jira adapters under `adapters/tracker`.
- **MCP.**
  - "`ao mcp` is the stdio MCP server AO passes to every chat session, so the agent sees html_preview and html_render", spawned outside the shell sandbox so it reaches the loopback daemon (`agent-orchestrator/backend/internal/cli/mcp.go#L17-L22`).
  - AO does not act as an MCP *client* hub for agents, and offers no A2A.
- **APIs.**
  - **Daemon HTTP.** Generated `openapi.yaml` with route-parity and embedded-spec tests, plus a generated TS client (`agent-orchestrator/docs/documentation-map.md#L54-L55`).
  - **Cloud.** A public contract in `contracts/cloud/openapi.yaml` (the only file in `contracts/`) and the `@aoagents/cloud-client` package.
  - **CLI.** DTOs are hand-mirrored on purpose and covered by tests (`documentation-map.md#L62-L65`).
- **Chat channels.** No Slack, Telegram or Signal adapters exist (none under `adapters/`); the phone app fills that role.
- **vs theuth.** theuth is ahead on channels, MCP hub, A2A and the mesh. AO is ahead on harness count and on the generated-contract discipline.

## A11 Engineering, licence & traction

**Stack.**
- Go workspace (`go.work`: `backend`, `cloud`).
- Electron + React desktop, Expo + React Native phone app.
- Shared TS packages (`packages/product-ui`, `packages/cloud-client`, `packages/shared`) and per-OS `ao` binary packages (`packages/ao-*`). Nix flake.

**Size.**
- About 251k lines of non-test Go in `backend/`, including sqlc-generated code.
- 1,008 Go test files across backend and cloud; 631 TS test/spec files.
- 175 SQLite migrations.
- Very large files: `session_manager/agent_switching.go` 3,779 lines, `gitworktree/workspace.go` 2,343, `lifecycle/manager.go` 2,013, `lifecycle/manager_test.go` 5,367.

**Test strategy.**
- Contract drift gates: `TestBuild_MatchesEmbedded`, `TestRouteSpecParity`, the `sqlc-drift` CI job, and embedded-skill YAML tests (`agent-orchestrator/docs/documentation-map.md#L54-L58`).
- Regression tests named after issues. Code comments cite issue numbers (#2879, #4528, #2652, #4810, #4122) as the reason for each guard.
- 14 workflows: `go`, `frontend`, `mobile`, `cli-e2e`, `e2e-gate-tests`, `mac-update-e2e`, `gitleaks`, `pricing-catalog-validate`, `version-floor-guard`, `release-latest-guard`, `pr-review-leaderboard`, `optional-private-submodule`, `build-artifacts`, `deploy-docs`. A `desktop-version-floor.json` at the root drives forced upgrades.

**Release cadence (GitHub MCP `list_releases`).** v0.13.5 stable on 2026-10-08 and v0.13.6 on 2026-10-09. Several nightlies a day (`v0.13.6-nightly.202610091332` … `v0.13.7-nightly.202610100346`) and per-PR builds (`v0.13.6-pr6476`). The public manual's baseline is v0.13.3 of 2026-10-01 (`documentation-map.md#L42-L43`).

**Traction (GitHub MCP `search_repositories`, 2026-10-10).**
- 13,024 stars, 1,787 forks, 813 open issues+PRs.
- Created 2026-02-13. PR/issue numbers have reached #6499 in under 8 months.
- Topics: agent-fleet, agent-swarm, git-worktrees, parallel-agents.
- Top open issues by comments: #3738 (sticky `waiting_input` freezes sessions), #2523 (resource guardrails), #4840 (Codex permission bypass), #4577 (Cloud chat mode P0), #5790, #4700, #4187.

**Licence and business model.**
- Apache-2.0 for the whole public tree, including the Cloud **control plane** in `cloud/`: a 28-table Postgres schema, WorkOS auth, GitHub App, sandbox reconciler (`agent-orchestrator/cloud/README.md#L1-L22`).
- The hosted product runs at `api.aoagents.dev` (`#L52-L54`). Its Next.js **web app is private**: `private/ao-cloud` is a submodule pointing at `github.com/Untrivial-ai/ao-cloud` with `update = none`, so it is empty in every public clone (`agent-orchestrator/.gitmodules#L1-L4`; `cloud/README.md#L22-L31`).
- `pricing/` is the *model-price* catalog (A7), not product pricing. `experiments/` holds one macOS blockmap experiment.
- Open-core shape: a free local desktop, plus a paid hosted service for remote workers, org sharing and the web app.

**The documentation-map idea.**
- Two named layers: human-facing prose ("can lag behind it") and a machine-readable contract layer ("generated from source and/or checked by CI drift gates") (`documentation-map.md#L10-L13`).
- The rule: "if an artifact in the contract layer disagrees with prose, generated contracts and their source win" (`#L15-L18`).
- Each contract artifact gets a row: source of truth, generator and drift gate (`#L52-L60`).
- Explicit sync rules per change type (`#L67-L78`) and a "where to add new documentation" router (`#L80-L88`).
- It is honest about limits: "being listed in the contract layer does not make their prose drift-proof" (`#L17-L18`). `AGENTS.md` is "Not machine-checked"; its hard rules are enforced by tests (`#L57`).

## Map to the three questions

### Q1 What do they offer versus theuth?

| Axis | Theirs (AO) | Ours (theuth, per AXES "theuth today") | Gap |
|---|---|---|---|
| A1 Identity | A harness adapter (36) plus a role prompt; no agent record | Agent file with charter, reports_to, model/tools/budget; roster | theuth ahead on identity; AO ahead on harness breadth and mid-session harness switching |
| A2 Authority | Flat: human → one orchestrator per project → workers; authority by prompt | Org chart, reporting lines, escalate, desks, reviewer, double | theuth ahead |
| A3 Work | The session+PR *is* the task; tracker intake; PR stacks; done = merged | Tasks board with states, owner, reviewer, done_when, threads | theuth richer; AO's lane *derived from SCM facts* and tracker-as-queue are missing in theuth |
| A4 Runtime | Worktree per task; hooks + screen staleness + no_signal; 5 s reaper with fenced termination; provider hosts survive daemon restart; isolated browser per worker | Mesh placement, sandbox VMs, routines/sensors, ACP children, desks | Mixed: theuth ahead on placement and scheduling; AO ahead on per-task isolation, activity truth and restart survival |
| A5 Coordination | Hub-and-spoke `ao` CLI; owner-routed CI/review/conflict nudges with persisted dedup; sessionguard; head-guarded merge | send_message mesh, A2A, checkout/disposition, escalate | AO's automatic feedback loop and write guard are missing in theuth |
| A6 HITL | Harness owns approvals; AO never pastes into a dialog; self-resolving notifications; phone over LAN with pairing QR | tools.policy, approval cards, double, attention inbox, HUD, phone/web/chat channels | theuth ahead overall; AO's auto-resolve and reconcile are missing |
| A7 Cost | Estimated cost from transcripts + pinned price catalog; no caps | Budgets with auto-pause, spend ledger | theuth ahead |
| A8 Memory | Project narrative conversation; one CLI skill; rules via native system channel | Knowledge base, dreaming, skills, endless | theuth far ahead |
| A9 UX | Delivery-loop Kanban (zones + "whose turn" columns), PR panel, browser, phone | Desktop/TUI/web/Slack/Signal/Telegram, HUD, activity monitor, boards | theuth broader; AO's PR-pipeline board is sharper for coding work |
| A10 Integration | Compiled adapters, ACP transport, GitHub/GitLab, `ao mcp`, generated OpenAPI + TS client | MCP hub, A2A, channels, mesh | theuth ahead on breadth; AO ahead on generated-contract discipline |
| A11 Engineering | Go + Electron + Expo; ~1k Go test files; nightly releases; open-core Cloud | Rust engine + Tauri desktop | Not comparable on traction; AO's documentation map and drift gates are a process idea |

### Q2 What can we learn / borrow?

Techniques only. Each item gives the mechanism, the cite, and the cost of being wrong.

1. **Store facts, derive status and lane in one pure shared function.** `DeriveStatus(session, prs, now, grace)` and `derivePRKanbanColumn` live in a public package used by daemon, Cloud and clients. Nothing derived is ever persisted (`agent-orchestrator/backend/pkg/contract/status.go#L107-L137`, `agent-orchestrator/backend/pkg/contract/kanban.go#L91-L118`, rule `agent-orchestrator/docs/architecture.md#L1146`). *If wrong:* low. A bad rule makes a lane flicker but never corrupts state; the cost is compute per read and the discipline of keeping facts minimal.
2. **Owner-routed feedback reactions with persisted dedup.** Each actionable condition (CI failure set, each unresolved comment, changes-requested review, merge conflict) is queued on its own. Each carries `(key, signature, attempts)` with per-type budgets and is sent in the order send → memory → persist; a suppressed send is never stamped delivered (`agent-orchestrator/backend/internal/lifecycle/reactions.go#L158-L267`, `#L929-L995`, `#L41-L47`). *If wrong:* duplicate nudges spam a busy agent, or a lost nudge leaves CI red. Budgets bound the first; the persist order makes the second rarer.
3. **One guarded write path into an agent.** Re-read state immediately before writing. Return typed suppression outcomes whose zero value is "not sent". Hold an admission lease that mutations close and drain (`agent-orchestrator/backend/internal/sessionguard/guard.go#L1-L9`, `#L28-L75`). *If wrong:* too strict and messages stall silently. AO's own #3634 is the mirror image: "reports success immediately but delivery waits". So the sender must see the suppression.
4. **Attention items as source-keyed intents that resolve only on facts, with boot reconcile** (`agent-orchestrator/backend/internal/ports/notifications.go#L12-L19`, `agent-orchestrator/backend/internal/notify/manager.go#L110-L114`, `#L145-L148`). *If wrong:* a row disappears before the human saw it. Keep resolved rows browsable.
5. **Hooks are authoritative; the screen or transcript only proves staleness; explicit `no_signal`** (`agent-orchestrator/backend/internal/adapters/agent/claudecode/terminal_activity.go#L10-L21`, `agent-orchestrator/backend/internal/observe/activity/observer.go#L120-L157`, `agent-orchestrator/backend/internal/service/session/status.go#L10-L17`). *If wrong:* per-harness screen heuristics rot as UIs change, and `no_signal` false alarms appear during slow boots. The grace and stale windows are the knobs.
6. **Two-phase, revision- and launch-fenced termination; "a failed probe is not death"; "runtime alive, workload dead" → exited** (`agent-orchestrator/backend/internal/lifecycle/manager.go#L462-L533`, `agent-orchestrator/backend/internal/lifecycle/runtime.go#L25-L28`). *If wrong:* zombie sessions linger a little longer, a cost AO accepts on purpose.
7. **A detached provider host per ACP child.** It owns only the connection state a dead daemon cannot rebuild (init, session, request correlation, in-flight prompt) plus a bounded prompt journal for replay. The control protocol is versioned (`agent-orchestrator/backend/internal/adapters/chatdriver/persistenthost/host.go#L1-L4`, `#L30-L37`, `acp_relay.go#L58-L60`, `acp_journal.go#L15-L18`). *If wrong:* high complexity, orphaned hosts, and protocol-version skew after upgrades.
8. **Engine-managed worktree per code task.**
   - managed root with path-component validation;
   - refuse to remove a dirty tree, and say why;
   - force only after stashing;
   - move to trash inside the request, delete off-request;
   - speculative pre-creation;
   - container labels reaped on termination.
   (`agent-orchestrator/backend/internal/adapters/workspace/gitworktree/workspace.go#L943-L980`, `#L2163-L2187`, `gitworktree/discard.go#L13-L30`, `agent-orchestrator/backend/internal/session_manager/task_preparation.go#L18-L41`, `agent-orchestrator/backend/internal/session_manager/prompt.go#L412-L420`). *If wrong:* disk fills, which Sky's Mac has already suffered from build targets. Pair this with a disk budget and cleanup policy.
9. **Tracker-as-queue intake.** Poll open issues, filter by assignee, one worker per issue, a seen-set, back off on failure (`agent-orchestrator/backend/internal/observe/trackerintake/observer.go#L183-L231`). *If wrong:* a bad filter spawns many workers. theuth's budgets and a max-open cap must gate it.
10. **An isolated adversarial reviewer agent.**
    - Its findings land as review comments that flow back to the owner.
    - Its permission hook is deny-by-default because nobody can answer it.
    - Review facts are scoped to the *current head*.
    - Auto-review starts after 1 min idle, with at most 3 retries.
    (`prompt.go#L367-L380`, `agent-orchestrator/backend/internal/cli/hooks.go#L515-L527`, `kanban.go#L45-L65`, `agent-orchestrator/backend/internal/autoreview/coordinator.go#L18-L22`). *If wrong:* review spend per head, and change-request ping-pong; the caps bound it.
11. **Merge only the expected head.** Compare the stored head, re-fetch and compare again, check readiness, then merge with the SHA as the provider precondition (`agent-orchestrator/backend/internal/service/pr/action_service.go#L103-L141`). *If wrong:* almost nothing; one extra API call.
12. **A documentation map.** Name the contract layer versus prose, state which wins on drift, give a drift gate per artifact, and give sync rules per change type (`agent-orchestrator/docs/documentation-map.md#L10-L18`, `#L52-L78`). *If wrong:* the map itself drifts. Keep it short and CI-checked where possible.
13. **A "whose turn is it" board column, plus merge-first attention order** (`kanban.go#L91-L140`, `agent-orchestrator/packages/product-ui/src/session-presentation.ts#L326-L327`). *If wrong:* two coexisting board models confuse users, as AO's README/zones versus contract columns already do.
14. **Coordinator role prompts with explicit negative rules, plus a singleton coordinator per project** (`prompt.go#L206-L221`, `agent-orchestrator/backend/internal/service/session/service.go#L202-L203`). *If wrong:* prompt-only authority gets ignored. theuth should keep `tools.policy` as the enforcer and use prompts only as the explanation.
15. **A pinned, CI-validated model-price catalog with per-harness transcript ingestion**, for cost attribution of harnesses that report no cost (`agent-orchestrator/pricing/catalog/v1/manifest.json#L1`, `agent-orchestrator/backend/internal/observe/usage/watcher.go#L28-L31`). *If wrong:* stale prices skew budgets. Pin by hash and show the age.

### Q3 How should theuth improve?

Ranked by value ÷ effort (S/M/L), each with the theuth seam it would land in.

1. **Self-resolving attention rows (S, high).** Give each attention item a `source_key` and a resolving fact: task left `blocked`, approval answered, PR merged, child resumed. Resolve when the fact lands, and reconcile at engine boot. *Seam:* attention inbox, designs 104 / 119 / 132; the activity monitor 155 for the facts. (Borrow 4.)
2. **A guarded delivery path for every injected turn (S–M, high).** Before `send_message` or a relay writes into an ACP child, a desk or a PTY job: re-read its state, refuse when a permission or approval prompt is pending, and return a typed "suppressed: awaiting_user / busy / restarting" receipt instead of `accepted`. Take an admission lease during restart and model switch. *Seam:* send_message delivery (design 50), ACP subagents (141), restart (146). (Borrow 3.)
3. **Owner-routed PR feedback (M, high).** Teach the `pr_watch.py` sensor path to deliver CI-failed, review-comment and conflict events *to the owning task's session* rather than waking a routine. Store `(subject, key, signature, attempts)` on the task row and let the task's budget cap the loop. *Seam:* `theuth/crates/agent/src/sensor`, the schedule `when` path (`theuth/crates/schedule/src/lib.rs#L3570-L3583`), tasks.sqlite (designs 69/118), send_message (50). (Borrow 2.)
4. **`no_signal` and staleness reconciliation in the activity monitor (S, medium).** A child or desk expected to report that has not within a grace window shows "no signal", not "idle". Check the screen or transcript only after a stale "active". *Seam:* activity (155), the desk liveness rules (WAKE-LIVE-CHILD decision). (Borrow 5.)
5. **Head-guarded merge in the reviewer flow (S, medium).** When a task with a PR is approved, merge only the head the reviewer saw (refetch + SHA precondition). *Seam:* GitHub MCP tools plus task review/disposition (69). (Borrow 11.)
6. **Engine-managed worktrees for code tasks (M, medium-high).**
   - add `where: worktree` on `task_create` / `spawn_subagent`;
   - dirty-refusal, trash-move teardown, container labels;
   - a disk budget.
   *Seam:* spawn_subagent `cwd`, the task `where` field; reuse the study-worktree code in `theuth/crates/autoresearch/src/git.rs`. (Borrow 8.)
7. **A derived "whose turn" lane on the tasks board (M, medium).** Compute "theuth/agent loop running", "a person's turn" and "ready" from facts (pending approvals, child activity, PR/CI state) next to the stored state. *Seam:* tasks board UI (118) plus activity (155). (Borrow 13.)
8. **An issue-intake routine template (S, medium).** A watcher routine with a GitHub or Jira sensor creates one task per eligible issue, with owner, budget and a max-open cap; the seen-set lives in knowledge. *Seam:* routines and sensors (design 40) plus `task_create`; ships as a skill or routine, not engine code. (Borrow 9.)
9. **A documentation map for `theuth/docs/design` (S, low-medium).** It has 575 files. Say which are as-built, which are proposals, which machine contracts (RPC bindings, ts-rs types) win on drift, and which CI gates hold them. *Seam:* `docs/README` and `AGENTS.md`. (Borrow 12.)
10. **A detached ACP host so engine restarts do not kill children (L, medium).** Relay plus prompt journal plus reattach, with a versioned control protocol. *Seam:* ACP runtime (141), restart clarity (146). Do this after item 2. (Borrow 7.)
11. **A price catalog and transcript ingestion for external harnesses (M, low-medium).** *Seam:* spend ledger / budgets. (Borrow 15.)

## Their weaknesses and failure modes

**Activity truth is fragile.** AO's whole design rests on hooks and screen heuristics per harness. The open issues show the edges:
- #3738: a post-Stop `agent_needs_input` notification overwrites Idle with a sticky `waiting_input`, so "sessions freeze indefinitely";
- #5790: "session shows Exited while its agent is still running";
- #3649: `ao spawn` "intermittently fails ... leaving orphaned worktrees and unrecoverable 'no_signal' sessions";
- #3873: high spawn failure rate, mostly from a missing agent binary or git branch state.

**Orchestrator durability.**
- #4700: "Restart can leave a project without a working orchestrator" (P1);
- #4641: an orchestrator killed by tmux or host death "has no path back to its own history";
- #3875: a fresh orchestrator spawn crashes Claude with "invalid session id".

**Delivery semantics.** #3634: `ao send` "reports success immediately but delivery waits for the worker's next turn boundary — a busy worker misses a stop message for minutes".

**Security.** #4840: Codex "Default" permission "falls through to bypass approvals and sandbox". #2771: markdown preview served same-origin with no CSP enables RCE. Authority over the orchestrator is prompt-only (`prompt.go#L206-L221`); nothing stops it from editing code.

**Board drift.** #6108: "Despite merging all the PRs, the GitHub task is still marked as 'in review'". Two board models (zones and columns) coexist, and the README documents only one.

**Operational load.**
- #4187: reaper WARN-spam left boot un-ready for 10+ minutes with about 11 sessions;
- #2523: no per-spawn memory ceiling or orphan-process reaper yet;
- 30 s SCM polling against provider rate limits, mitigated by ETags (`architecture.md#L842`).

**Platform friction.** #4479: ConPTY rejects dotted session ids. #3852: the iOS app cannot connect over Tailscale because ATS blocks cleartext. #3562: the app must call its bundled binary, not the one on PATH. Several updater issues (#4175, #3528).

**Code smells.**
- Very large files (`agent_switching.go` 3,779 lines; `workspace.go` 2,343; `lifecycle/manager.go` 2,013).
- Comments that are largely incident history (#2879, #4528, #4122, #2652). The guards are real but the reducers are hard to reason about.
- 813 open issues+PRs at about 27 new numbers per day (#6499 in 239 days).

**Docs lag code.**
- `architecture.md#L246` says "23+" harnesses; the registry has 36.
- The status-precedence diagram (`architecture.md#L671-L702`) disagrees with `contract/status.go#L120-L127` on active versus waiting and on `exited`.
- The documentation map admits contributor docs can be ahead of the stable release (`documentation-map.md#L42-L48`).

**Scope limits.** One machine, one human, coding only. No budgets, no memory, no org model. Non-code work (research, ops, writing) has no first-class home.

## Cites self-check

**Method.** At the end I re-opened every load-bearing cite programmatically. That was 185 (file, line-range, expected text) checks across 57 files, each reading the cited lines from the clone and asking whether the quoted phrase or symbol is inside the range. 178 held on the first pass. Of the 7 misses, 5 were artefacts: a phrase wrapped across comment lines, or `workspace.go` (2,343 lines) truncated by the bulk read and re-read with an offset. 2 were real line drifts, now corrected in the text. A selection of the load-bearing ones:

| # | Cite | Checked for | Result |
|---|---|---|---|
| 1 | `agent-orchestrator/backend/pkg/contract/status.go#L107-L137` | `func DeriveStatus` precedence | held |
| 2 | `agent-orchestrator/backend/pkg/contract/status.go#L120-L127` | `StatusWorking` returned before needs-input (docs disagree) | held |
| 3 | `agent-orchestrator/backend/pkg/contract/kanban.go#L91-L118` | `derivePRKanbanColumn` | held |
| 4 | `agent-orchestrator/backend/pkg/contract/kanban.go#L45-L65` | review facts scoped to the "current head" | held |
| 5 | `agent-orchestrator/backend/internal/lifecycle/reactions.go#L158-L164` | "Queue every applicable nudge" | held |
| 6 | `agent-orchestrator/backend/internal/lifecycle/reactions.go#L251-L267` | merge-conflict nudge, bottom of stack, urgent | held |
| 7 | `agent-orchestrator/backend/internal/lifecycle/reactions.go#L943-L949` / `#L981-L986` | seen/attempt dedup; send → memory → persist | held |
| 8 | `agent-orchestrator/backend/internal/lifecycle/reactions.go#L81-L118` | no termination while the agent is active (#2879) | held |
| 9 | `agent-orchestrator/backend/internal/sessionguard/guard.go#L1-L9` / `#L28-L35` | one write invariant; InputLease | held |
| 10 | `agent-orchestrator/backend/internal/adapters/agent/claudecode/hooks.go#L36-L41` | "never injects a permission decision" | held (phrase wraps a line; first-pass false miss) |
| 11 | `agent-orchestrator/backend/internal/adapters/agent/claudecode/terminal_activity.go#L10-L21` | "stuck active forever" | held |
| 12 | `agent-orchestrator/backend/internal/observe/activity/observer.go#L15-L16` / `#L150-L157` | 2 min stale; ExpectedRevision + LaunchID | held |
| 13 | `agent-orchestrator/backend/internal/cli/hooks.go#L544-L551` / `#L515-L527` | best-effort exit 0; reviewer deny default | held |
| 14 | `agent-orchestrator/backend/internal/lifecycle/manager.go#L462-L533` | two-phase fenced termination + container reap | held |
| 15 | `agent-orchestrator/backend/internal/adapters/workspace/gitworktree/workspace.go#L943-L963` / `#L973-L980` | ErrWorkspaceDirty; ForceDestroy only after StashUncommitted | held |
| 16 | `agent-orchestrator/backend/internal/adapters/workspace/gitworktree/workspace.go#L2163-L2187` | path-component guard, orchestrator path | held (bulk read truncated at 2,000 lines; re-read with offset) |
| 17 | `agent-orchestrator/backend/internal/adapters/chatdriver/persistenthost/host.go#L1-L4`, `acp_journal.go#L15-L18`, `acp_relay.go#L58-L60` | survives daemon replacement; host WAL; correlation-only relay | held |
| 18 | `agent-orchestrator/backend/internal/service/pr/action_service.go#L103-L141` | expected-head checks before squash merge | held |
| 19 | `agent-orchestrator/backend/internal/notify/manager.go#L110-L114` / `#L145-L148` | fact-only resolution; reconcile after downtime | held (wrapped phrase) |
| 20 | `agent-orchestrator/backend/internal/ports/notifications.go#L12-L29` | `SourceKey` idempotency | held |
| 21 | `agent-orchestrator/backend/internal/observe/trackerintake/observer.go#L183-L231` | open-issue list, assignee filter, spawn per issue | held |
| 22 | `agent-orchestrator/backend/internal/autoreview/coordinator.go#L18-L22` | 1 min idle, 1 min sweep, 3 retries | held |
| 23 | `agent-orchestrator/backend/internal/service/session/status.go#L10-L17` | `noSignalGrace = 90 s` | held |
| 24 | `agent-orchestrator/backend/internal/cdc/poller.go#L13` / `#L25-L30` | 100 ms; restart re-seeks to head | held |
| 25 | `agent-orchestrator/backend/internal/storage/sqlite/migrations/0001_init.sql#L123-L147` | `CREATE TRIGGER sessions_cdc_insert` | held |
| 26 | `agent-orchestrator/backend/internal/session_manager/prompt.go#L206-L221` / `#L261-L262` / `#L383-L391` / `#L412-L420` | coordination-only; no merge unless asked; blockers only; container labels | held |
| 27 | `agent-orchestrator/backend/internal/adapters/agent/registry/registry.go#L54-L57` | `Constructors()`, 36 `New()` entries, no fake adapter | held |
| 28 | `agent-orchestrator/packages/product-ui/src/session-presentation.ts#L326-L327` / `#L440-L465` | merge-first zone order; zone mapping | held |
| 29 | `agent-orchestrator/docs/documentation-map.md#L15-L18` / `#L52-L60` | contracts win; drift-gate table | held |
| 30 | `agent-orchestrator/docs/architecture.md#L246` | "23+ agent harnesses" (docs lag code) | held |
| 31 | `agent-orchestrator/cloud/README.md` private web-app paragraph | `private/ao-cloud` checkout | **corrected**: was `#L24-L36`, now `#L22-L31` |
| 32 | `agent-orchestrator/cloud/README.md` production host | `api.aoagents.dev` | **corrected**: was `#L54-L56`, now `#L52-L54` |
| 33 | `agent-orchestrator/.gitmodules#L1-L4` | submodule `Untrivial-ai/ao-cloud`, `update = none` | held |
| 34 | `theuth/crates/schedule/src/lib.rs#L3570-L3583`, `theuth/crates/autoresearch/src/git.rs#L3` | pr_watch sensor fixture; study-only worktrees | held (from rg on theuth main c22556ea) |

**Other counts re-checked.**
- The `using-ao` command catalog has **17** pages, not 18 (corrected in A5).
- `chatdriver/` has 15 harness-specific drivers plus the shared `acp`, `persistenthost` and helper packages.
- `ports/agent.go` declares 31 interface types.

**Not verified.**
- Runtime behaviour: nothing was built or run.
- Exact contributor counts. The GitHub MCP gave stars, forks and open issue+PR count but no contributor total.
- The reviewer hook's Bash allow-list beyond `cli/hooks.go#L527`.
- Whether the Cloud product is paid today. Hosted environments exist; I read no pricing page.
- Content of `private/ao-cloud`, which is empty in the clone.
