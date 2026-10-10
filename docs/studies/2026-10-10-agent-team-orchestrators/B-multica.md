# Multica — humans and AI agents as one team (recon for the agent-teams study, 2026-10-10)

Clone `multica` at c8d6a1f (shallow, 2026-10-10). Read-only; nothing built or run. Cites are `multica/<path>#L<a>-L<b>`;
docs paths are under `multica/apps/docs/content/docs/`. Where docs and code disagree, code wins and I say so.

## Executive summary
1. **What it is.** A source-available (Apache-2.0 + hosting/branding conditions) Linear-style issue board where agents are assignees: a Go/Postgres server records issues, comments and *runs*; a daemon on each machine claims runs and drives one of 26 local agent CLIs (Claude Code, Codex, Cursor, OpenCode, Hermes, omp …). 52.4k★ in nine months, 0.x, near-daily releases.
2. **Team model.** Flat: workspace roles owner/admin/member, agents with a per-agent "who may run me" ACL, and *squads* (one leader agent routes by @-mention). No reporting lines, no budgets, no memory — skills are the only shared learning.
3. **Idea 1 for theuth — the run ledger + claim discipline.** Every trigger is a durable run row (attempt, typed failure reason, session/workdir for resume, retry vs rerun lineage), claimed with `FOR UPDATE SKIP LOCKED`, serialised per (agent, issue), deduped by a partial unique index, leased, slot-before-claim on the worker, poisoned sessions never resumed.
4. **Idea 2 — typed mentions + a pre-send wake preview.** Only `mention://agent/<uuid>` links wake anyone; one predicate decides "will this write start a run, and for whom" for both the write and the UI preview; `/note` posts silently. This directly prevents theuth's 2026-10-05 "names in a note woke five desks" incident.
5. **Idea 3 — agent-set conditional wakeups.** Agents register wake rules on an issue (event/at/every/cron, conditions, expiry with on_timeout, max_fires = 20, receipts); the platform reuses them for stage barriers on sub-issues.
6. Also worth taking: originator-carried authority for agent→agent wakes, failure-rate auto-pause for automations, self-archiving failure notifications, the leader "delegate, record evaluation, stop" protocol, bounded delta reads.
7. **Where theuth is ahead:** hierarchy and escalation, action-level approvals and the double, budgets, knowledge + dreaming, mesh placement and sandbox VMs, direct peer messaging.
8. **Where Multica is ahead:** run durability/retry semantics, harness breadth, wake rules, work-level gates (confirm, preview, review-only status), a real plugin contract.
9. **Weaknesses:** unattended `bypassPermissions` with no sandbox; orchestration left to prompts (top open issues ask for workflows); field reports of queue/claim stalls; per-CLI maintenance tax; runs pinned to one machine; no spend caps.
10. **Do not copy:** per-harness backends (contradicts Sky's ACP-first ruling), auto-approval as default, flat squads instead of reporting lines.
11. Schema ground truth (598 migrations) is summarised under A3; licence quoted exactly under A11.
12. **Verdict:** not a replacement for theuth's org/desk model, but the best reference in the field for a *durable, auditable run queue and wake semantics* — borrow those (Q3 items 1–8 are S/M effort).

## A1 Agent model & identity

**What an agent is.** A row in `agent`, not a process: name, avatar, description (display-only), `instructions`
(used on every run), `runtime_id` (one bound runtime = one machine + one CLI), `model`, `thinking_level`,
`service_tier`, `custom_env`/`custom_args`/`mcp_config`, `max_concurrent_tasks`, `owner_id`, `permission_mode`,
`kind` (user|system) + `system_key`, `archived_at` — `multica/server/pkg/db/generated/models.go#L25-L57`.
The docs say the same in prose — "An agent is not a continuously running process … it only produces concrete runs
when work arrives" (`multica/apps/docs/content/docs/agents.mdx#L10`) and "The agent is the identity; the runtime is
the computer that executes it" (`multica/apps/docs/content/docs/concepts.mdx#L32-L34`).

- **Harness binding = runtime.** `agent_runtime` rows are one per (daemon, workspace, provider/profile) with
  `status`, `last_seen_at`, `visibility` (private|public), `profile_id` (`multica/server/pkg/db/generated/models.go#L83-L101`;
  docs `multica/apps/docs/content/docs/daemon-runtimes.mdx#L12-L15`). Switching an agent's model or instructions keeps its
  identity and history (`multica/apps/docs/content/docs/agents.mdx#L24`). Rebinding is guarded: a queued task's stored
  runtime is "not authority after an agent rebind" — the claim requires `a.runtime_id = atq.runtime_id`
  (`multica/server/pkg/db/queries/agent.sql#L770-L776`).
- **26 CLIs behind one interface.** `Backend.Execute(ctx, prompt, ExecOptions) → *Session` (`multica/server/pkg/agent/agent.go#L17-L23`);
  `New(agentType, cfg)` switches on a whitelist of types (`multica/server/pkg/agent/agent.go#L344-L346`, `#L432`), and
  each built-in runtime names a `ProtocolFamily` it dispatches to (`multica/server/pkg/agent/builtin_runtimes.go#L24-L28`).
  README lists 26 CLIs including Claude Code, Codex, Cursor, Copilot, OpenCode, Hermes, Pi and **Oh-My-Pi (`omp`)**
  (`multica/README.md#L165-L182`) — i.e. Multica drives the same omp that theuth spawns over ACP.
- **Custom runtime profiles** let an admin wrap an internal command in one of the existing protocol families
  (`runtime_profile`: `protocol_family`, `command_name`, `fixed_args`, `visibility`, `enabled` —
  `multica/server/pkg/db/generated/models.go#L1368-L1382`); argument order is `<command> <fixed args> <Multica protocol args>
  <agent custom args>` and protocol-critical flags are dropped (`multica/apps/docs/content/docs/daemon-runtimes.mdx#L127-L142`).
- **Instructions vs skills.** Instructions = this agent's long-lived role; skills = reusable methods attachable to many
  agents (`multica/apps/docs/content/docs/skills.mdx#L12-L18`). Description never enters the prompt (`agents.mdx#L16`).
- **Onboarding / templates.** "New agent" or **Build with AI** generates the config from a description (README
  `multica/README.md#L152-L154`; draft table `agent_builder_draft`, models.go `#L59-L64`). There is one built-in
  **system agent "Mika"** whose instructions make it a Chief-of-Staff router: "A member brings you a goal, not a routing
  decision … route it yourself" and "Route each issue to the smallest thing that fits: yourself / a teammate / a new
  specialist agent / a squad / an autopilot" (`multica/server/internal/service/builtin_agents/mika/INSTRUCTIONS.md#L1-L19`).
  `kind`/`system_key` were added for it (`multica/server/migrations/163_agent_builder.up.sql#L1-L4`). There is also
  `multica agent copy` (`multica/server/cmd/multica/cmd_agent_copy.go`) — a copy, not an export/import format.
- **Lifecycle.** Original status enum `idle|working|blocked|error|offline` (`multica/server/migrations/001_init.up.sql#L44`),
  but the UI now derives two orthogonal signals — availability from the runtime (online/offline/unstable) and workload
  from runs (working/queued/idle) (`multica/apps/docs/content/docs/agents.mdx#L67-L74`). Archive is soft and reversible
  and **cancels all unfinished runs** (`agents.mdx#L76-L82`).
- **No persona file, no org position.** There is no `reports_to`, no charter/title split, no budget on the agent row.
  The agent's "who am I" is the instructions string; its "where am I" is only its squad memberships.

**Verdict vs theuth.** theuth's agent is a markdown file with title/charter/`reports_to`/budget/capabilities and a
long-lived desk session; Multica's is a database row with no hierarchy and no standing session — every trigger
starts a fresh run that may *resume* a CLI session (A4). Multica is ahead on harness breadth (26 CLIs, custom
profiles, per-machine detection); theuth is ahead on identity depth (charter, reporting line, budget, desk).

## A2 Organisation & authority

- **Tenancy = workspace.** Everything hangs off `workspace` (`name`, `slug`, `settings`, `context`, `repos`,
  `issue_prefix`/`issue_counter`, `attribution_fail_closed`) — `multica/server/pkg/db/generated/models.go#L1723-L1738`.
  Licence allows "Internal use within a single organization (including multiple workspaces)" (`multica/LICENSE#L38-L39`).
- **Human roles** are a three-value enum on `member`: `owner|admin|member` (`multica/server/migrations/001_init.up.sql#L30`;
  struct `models.go#L1183-L1189`). Invitations and share links can only grant admin|member
  (`multica/server/migrations/041_workspace_invitation.up.sql#L7`, `327_workspace_share_link.up.sql#L14`).
- **Agent access (who may run an agent)** is separate from roles: `permission_mode` private|public_to
  (`multica/server/migrations/130_agent_invocation_permission.up.sql#L33`) plus an allow-list table
  `agent_invocation_target(target_type, target_id)` (`models.go#L67-L74`). UI calls it Only me / Entire workspace /
  Specific people; default Only me (`multica/apps/docs/content/docs/agents.mdx#L49-L57`). **Admins cannot bypass it**:
  "private (or any unknown mode) is deny-by-default: no admin bypass, no A2A bypass"
  (`multica/server/internal/handler/agent_access.go#L86-L90`).
- **A2A authority follows the human at the top of the chain.** "if user U triggers agent A and A @-mentions agent B,
  B is only invocable when U (the originator) is in B's allow-list. This prevents agents from forming a channel that
  bypasses the owner's white-list" (`multica/server/internal/handler/agent_access.go#L33-L36`); the gate judges
  agent/system actors by `originatorUserID` only (`#L73-L80`). Every run row carries `originator_user_id` (authorisation),
  `accountable_user_id` (audit/cost only, never authorisation) and `originator_source` = direct_human | delegation |
  comment_source | rule_owner | owner_fallback | backfill | unattributed (`multica/server/pkg/db/generated/models.go#L145-L168`).
  A workspace can refuse unattributed runs outright (`attribution_fail_closed`, `models.go#L1736-L1737`). Autopilot
  triggers fire **as their creator**, and edits never move a run's authority to the editor (`models.go#L315-L322`).
- **Runtime privacy.** A local runtime is private to its owner; admins included cannot put agents on someone else's
  machine, and only the owner can make it public — sharing routes runs, never credentials
  (`multica/apps/docs/content/docs/daemon-runtimes.mdx#L78-L80`).
- **Org chart: none.** No reporting lines, no manager, no escalation ladder. The only structure is the **squad**: one
  leader *agent* plus agent/member members with free-text role descriptions that "grant no permissions and never
  trigger members automatically" (`multica/apps/docs/content/docs/squads.mdx#L20-L25`; tables `squad`, `squad_member`
  — `models.go#L1445-L1467`; `member_type IN ('agent','member')` — `multica/server/migrations/084_squad.up.sql#L20`).
  Squads are flat (no squad-of-squads) and an issue may be assigned to member | agent | squad
  (`084_squad.up.sql#L33`).
- **Where humans sit.** Humans are peers on the same board (assignee, reviewer by convention, squad member) and the
  only principals that can authorise work; agents "have no inbox and don't receive `@all`. An @-mention of an agent is
  an execution trigger, not a notification" (`multica/apps/docs/content/docs/agents.mdx#L45`).

**Verdict vs theuth.** theuth has the richer *hierarchy* (reports_to, escalate up the line, desks); Multica has the
richer *authorisation* model (per-agent invoke ACL, originator-carried A2A authority, accountable-vs-authorising
human split, fail-closed attribution). theuth today relies on `tools.policy` + approvals per tool call; it has no
"who may wake this agent" rule and no originator chain on delegated work.

## A3 Work model

**Issue = the unit of work; run = one execution against it.** `issue` carries title, description, `status`,
`priority`, `assignee_type`/`assignee_id` (member | agent | squad), `creator_type`/`creator_id`, `parent_issue_id`,
`acceptance_criteria` (JSON), `context_refs`, `project_id`, `origin_type`/`origin_id`, `stage`, `properties`,
`revision`, `triage_state`, `duplicate_of_issue_id`, `first_executed_at` (`multica/server/pkg/db/generated/models.go#L818-L849`).
"A run completing does not by itself change the issue status" (`multica/apps/docs/content/docs/assigning-issues.mdx#L39`;
also `how-multica-works.mdx#L40-L42`).

- **States.** Built-in `backlog|todo|in_progress|in_review|done|blocked|cancelled`
  (`multica/server/migrations/001_init.up.sql#L58`), plus per-workspace custom statuses with categories (table
  `issue_status`, `models.go#L962-L976`) resolved through `issuestatus.Effective` — custom statuses "do not inherit
  parking" and terminal categories cannot start work (`multica/server/internal/service/issue_trigger.go#L107-L112`).
- **Agent-managed status contract.** The agent moves the issue itself: starting → `in_progress`, delivery →
  `in_review`, consultation leaves status alone; `done` is left to a human or an integration (PR merge with close
  intent) (`multica/apps/docs/content/docs/assigning-issues.mdx#L39-L41`; squads `squads.mdx#L37-L38`). A failed run
  with nothing else pending moves `in_progress` back to `todo` (`multica/apps/docs/content/docs/tasks.mdx#L86`).
  → This is a *review gate by convention*: agents may only reach `in_review`.
- **Backlog parks; everything else fires.** Assigning on backlog never starts a run; promoting out of backlog does,
  behind a confirmation dialog with "Don't start yet" (`multica/apps/docs/content/docs/assigning-issues.mdx#L43-L51`);
  CLI `--no-start` (`#L71-L82`).
- **One predicate for "will this write start a run, and for whom".** `WillEnqueueRun` is "the single predicate …
  shared by the issue update / batch-update write paths and the preview endpoint, replacing the per-site copies that
  drifted" (`multica/server/internal/service/issue_trigger.go#L72-L96`). The UI uses it to show a **trigger preview**
  before a write or a comment is sent ("which agents will start working once you send … whether a target is
  unavailable") (`multica/apps/docs/content/docs/mentioning-agents.mdx#L22-L30`). Squad assignment resolves to the
  leader agent inside the same predicate (`issue_trigger.go#L175-L201`).
- **Dedup in the schema.** Enqueue goes through `CreateAgentTask` "guarded by the (issue_id, agent_id) partial unique
  index over pending (queued/dispatched) tasks" (`issue_trigger.go#L83-L87`); `hasPendingRun` fails closed and keys on
  the reviewed head SHA so an old pending run doesn't shadow a request after HEAD moved (`issue_trigger.go#L206-L220`).
- **Hierarchy & dependencies.** Sub-issues via `parent_issue_id`; numbered **stages** with a platform-owned
  `child_done` wakeup that re-wakes the parent's assignee when a stage closes and once more when all sub-issues are
  closed (`multica/server/internal/service/issue_wakeup_system.go#L22-L30`; child events closed|reopened|attached|
  detached|restaged — `multica/server/migrations/558_issue_child_event.up.sql#L18`). `issue_dependency` with
  `blocks|blocked_by|related` exists since the first migration (`001_init.up.sql#L93`; `models.go#L863-L868`) but the
  only query that touches it today is the workspace-delete cascade (`multica/server/pkg/db/queries/workspace_delete.sql#L366`;
  no matches for `blocked_by|DependsOn` in `server/internal/service`) — dependencies are **not** a scheduling gate;
  ordering is done with sub-issue stages instead.
- **Projects** group issues and bind durable resources — `github_repo`, `local_directory` (in-place or worktree mode)
  — that every later run receives (`multica/apps/docs/content/docs/concepts.mdx#L18-L20`;
  `multica/server/internal/daemon/local_directory.go#L16-L25`); project status `planned|in_progress|paused|completed|
  cancelled` (`multica/server/migrations/034_projects.up.sql#L9`).
- **Acceptance criteria** exist as a JSON column on the issue (`models.go#L830`) but the only Go references are the
  search/select paths in `multica/server/internal/handler/issue.go#L1101` and `#L1188`; nothing evaluates them — review is human.
- **Planning/decomposition** is agent behaviour, not a platform feature: Mika "Route[s] each issue to the smallest thing
  that fits" and creates issues/agents/squads/autopilots (`multica/server/internal/service/builtin_agents/mika/INSTRUCTIONS.md#L14-L20`);
  squad leaders delegate by comment (A5).
- **External trackers.** No Linear/Jira import. Git hosts are first-class: GitHub App installs, PR/check-run tables,
  `issue_pr_automation` (close intent on merge) and a generic `vcs_connection` for GitLab/Gitea/Forgejo
  (`models.go#L737-L766` GithubPullRequest, `models.go#L1643-L1654` VcsConnection; README `multica/README.md#L101`).
  Inbound work also arrives from Slack/Feishu/DingTalk/WeCom/Telegram channels (`channel_*` tables, A10).

**Verdict vs theuth.** Very close to theuth's tasks board in spirit (one owner, states, review, sub-work), but Multica
has (a) the *run* as a separate, first-class, retryable record under each issue, (b) a schema-level dedup of pending
runs per (issue, agent), (c) a write-preview that tells the human who will wake before they press, and (d) stage
barriers for sub-issues. theuth has `done_when` acceptance criteria, dispositions and escalation that Multica lacks.

### Core tables (schema ground truth)

598 up-migrations numbered 001–572 (some numbers reused, e.g. two `109_*`); 145 distinct `CREATE TABLE` names over time,
124 structs in the current sqlc model (`multica/server/pkg/db/generated/models.go`, 1,774 lines). The team-relevant core:

| Table | Key columns | Cite |
|---|---|---|
| `workspace` | slug, settings, context, repos, issue_prefix/counter, attribution_fail_closed | `models.go#L1723-L1738` |
| `member` | workspace_id, user_id, role owner/admin/member | `models.go#L1183-L1189`; `001_init.up.sql#L30` |
| `agent` | instructions, runtime_id, model, thinking_level, custom_env/args, mcp_config, max_concurrent_tasks (default 6), owner_id, permission_mode, kind/system_key, archived_at | `models.go#L25-L57` |
| `agent_invocation_target` | agent_id, target_type, target_id (invoke allow-list) | `models.go#L67-L74` |
| `agent_runtime` | daemon_id, provider, status online/offline, last_seen_at, visibility, profile_id | `models.go#L83-L101`; `004_agent_runtime_loop.up.sql#L8` |
| `runtime_profile` | protocol_family, command_name, fixed_args, visibility, enabled | `models.go#L1368-L1382` |
| `issue` | status, priority, assignee member/agent/squad, parent_issue_id, stage, acceptance_criteria, project_id, triage_state, revision | `models.go#L818-L849`; `084_squad.up.sql#L33` |
| `issue_status` | per-workspace custom statuses with categories | `models.go#L962-L976` |
| `comment` | author member/agent/system/plugin, parent_id, type, source_task_id, resolved_*, suppressed_agent_ids | `models.go#L587-L608`; `362_plugin_hook_engine.up.sql#L64` |
| `agent_task_queue` (= a run) | agent, issue / chat_session / autopilot_run, status queued/dispatched/running/waiting_local_directory/deferred/completed/failed/cancelled, priority, session_id, work_dir, attempt/max_attempts, failure_reason, originator/accountable, coalesced/delivered comment ids, prepare lease, is_leader_task, squad_id | `models.go#L110-L181`; `128_comment_routing_escalation.up.sql#L5-L16` |
| `task_message` | per-run event log (seq, type, tool, input, output, call_id) | `models.go#L1494-L1506` |
| `task_supplement` | mid-run additions pending/delivering/delivered/failed | `models.go#L1508-L1521`; `538_task_supplement.up.sql#L23` |
| `task_usage` | tokens by kind + cost_usd_ticks per run × model | `models.go#L1542-L1555` |
| `issue_wakeup` + `issue_wakeup_receipt` | agent-set/system wake rules and per-event receipts | `models.go#L1028-L1079` |
| `squad` / `squad_member` | leader_id, instructions / member_type agent|member, role text | `models.go#L1445-L1467` |
| `skill` / `skill_file` / `agent_skill` | SKILL.md content + files + bindings | `models.go#L1417-L1437`, `#L103-L108` |
| `autopilot` / `autopilot_trigger` / `autopilot_run` | create_issue|run_only; schedule|webhook|api triggers firing as creator | `models.go#L207-L323`; `042_autopilot.up.sql#L15`, `#L33` |
| `inbox_item` | recipient, type, severity, issue, actor, details | `models.go#L789-L805` |
| `plugin_*` (21 tables) | packages, releases, installations, grants, secrets, storage, invocations, hook schedules | `319_remote_mcp_plugin_v1.up.sql#L1-L22` |

## A4 Execution & runtime

**Shape: central server + one daemon per machine.** "Multica records and coordinates the work; connected computers
execute it" (`multica/apps/docs/content/docs/how-multica-works.mdx#L3-L8`). The daemon "connects to the server,
discovers local tools, claims runs, and reports results back"; a runtime = one computer × one CLI (or custom profile)
× workspace (`multica/apps/docs/content/docs/daemon-runtimes.mdx#L12-L15`). Desktop starts the daemon itself;
headless boxes run `multica daemon start` (`daemon-runtimes.mdx#L27-L45`).

- **Wake = push hint + pull claim.** "When a new run enters the queue, the server notifies the matching daemon; the
  daemon also polls periodically as a backstop" (`daemon-runtimes.mdx#L55`). In code, `pollLoop` turns runtime-set
  changes and task wake-ups into a nudge for one machine-level batch poller (`multica/server/internal/daemon/daemon.go#L5348-L5394`).
- **Slot-before-claim.** `runBatchPoller` "acquires whatever execution slots are free (slot-before-claim, so a claimed
  task never sits server-side `dispatched` without local capacity to run it and race the dispatch-timeout sweeper),
  asks the server for up to that many tasks across all of the daemon's runtimes in one call"
  (`multica/server/internal/daemon/daemon.go#L5396-L5405`, slots drained at `#L5445`, claim at `#L5457`); an
  auto-update barrier refuses to claim while the binary is about to roll (`#L5447-L5455`).
- **Atomic claim with serialisation.** `ClaimAgentTask` is one `UPDATE … WHERE id = (SELECT … FOR UPDATE SKIP LOCKED)`
  that (i) requires the runtime online and fresh (`last_seen_at` within the stale window), (ii) refuses a task when the
  same agent already has a dispatched/running task **on the same issue** (or same chat session), and (iii) orders by
  priority then age (`multica/server/pkg/db/queries/agent.sql#L749-L810`; serialisation `#L789-L805`). "This allows
  different agents to work on the same issue in parallel while preventing a single agent from running duplicate
  tasks" (`agent.sql#L750-L755`). A claim sets a **prepare lease** (`prepare_lease_expires_at`, `#L760-L763`) refreshed
  every 15 s by the daemon (`multica/server/internal/daemon/daemon.go#L148`).
- **Delivery receipts are CAS-guarded.** `SetTaskDeliveredCommentIDs` only writes while the task is still `dispatched`,
  unstarted, at the same `dispatched_at`, and only for ids in the enqueue-time plan (`agent.sql#L812-L835`) — so a
  stale reclaim cannot ack comments it never delivered.
- **Concurrency.** Daemon-wide default 20 (`multica/server/internal/daemon/config.go#L72`, env
  `MULTICA_DAEMON_MAX_CONCURRENT_TASKS` at `#L474`), per-agent default 6 (`multica/server/migrations/023_agent_concurrency_default.up.sql#L1`);
  "Effective concurrency is the smaller of the two" (`daemon-runtimes.mdx#L72-L74`). Slot index is exported as
  `MULTICA_TASK_SLOT` for slot-indexed resources such as GPUs (`daemon-runtimes.mdx#L105`).
- **Liveness.** Heartbeat every 15 s (`config.go#L30`); the HTTP fallback window is 2× that (`daemon.go#L2030-L2037`);
  server marks a runtime offline after 150 s (`multica/server/internal/service/task.go#L189`, used as
  `staleThresholdSeconds` in `multica/server/cmd/server/runtime_sweeper.go#L34-L37`, sweep `#L218-L221`). Queued runs
  on an offline runtime fail only when **both** the runtime has been silent longer than the reconnect grace **and** the
  run has been queued that long (`daemon-runtimes.mdx#L63`); in-progress runs fail and may retry; on restart the daemon
  reclaims runs that did not end cleanly (`#L64-L65`); a WS reconnect broadcasts "reconcile now" to every coarse ticker
  loop (`multica/server/internal/daemon/reconcile.go#L8-L19`).
- **Stall detection inside a run.** No wall-clock deadline by default: "a zero (or negative) timeout imposes NO
  deadline, leaving liveness entirely to the daemon's inactivity watchdog so a session that keeps emitting events is
  never killed merely for running long" (`multica/server/pkg/agent/agent.go#L138-L149`). Separate knobs for semantic
  inactivity, first-turn no-progress, idle watchdog vs in-flight-tool watchdog, handshake and interrupt timeouts
  (`agent.go#L44-L75`; watchdog `multica/server/internal/daemon/daemon.go#L9896`).
- **Session resume & poisoning.** Each run stores `session_id`, `work_dir`, `branch_name`, `durable_work_dir`
  (`models.go#L124-L125`, `#L173-L174`); the next run on the same (agent, issue) resumes via `ResumeSessionID`, and if
  the backend lands on a fresh thread anyway it prepends a caller-worded **continuity notice** (`agent.go#L76-L96`).
  Sessions are marked **poisoned** (context-exhausted output, fallback markers, provider 400) so the next run starts
  fresh instead of inheriting a broken history (`multica/server/internal/daemon/poisoned.go#L10-L24`). Manual retry
  re-runs the *original* agent, keeps files, and resumes the session when safe (`tasks.mdx#L128-L130`).
- **Retries.** Typed failure reasons — runtime_offline/recovery/timeout/iteration_limit/agent_blocked/… plus
  `agent_error.*` provider classes (`multica/server/pkg/taskfailure/failure.go#L85-L267`); auth/quota are explicitly not
  retryable (`failure.go#L201-L208`). Default: at most two executions, three for tool network faults (`tasks.mdx#L80-L82`);
  lineage kept apart as `retry_of_task_id` vs `rerun_of_task_id` (`models.go#L157-L160`).
- **Workspaces.** Per-task directories under a workspaces root (`daemon-runtimes.mdx#L47`), git worktrees from a bare
  repo cache (`multica/server/internal/daemon/execenv/git.go#L76-L99`; cache `gc.go#L21-L23`), or a project
  `local_directory` used in place with a path mutex — tasks then wait in status `waiting_local_directory`
  (`local_directory.go#L41-L45`; status added in `multica/server/migrations/109_agent_task_waiting_local_directory.up.sql#L15`).
  GC: 24 h TTL, 72 h orphans, completed envs 14 days on Cloud / kept on self-host (`config.go#L73-L78`).
- **How the brief reaches the CLI.** The runtime brief is written as a per-task context file in the workdir —
  `CLAUDE.md`, `AGENTS.md`, `CODEBUDDY.md` or `QWEN.md` — and only inlined for providers that cannot read it
  (`agent.go#L32-L40`). It is assembled from sections such as "On Behalf Of", "Workspace Context", "Connected Apps",
  "Available Commands" (`multica/server/internal/daemon/execenv/runtime_config_sections.go#L168`, `#L185`, `#L219`, `#L257`).
- **Agents act through the CLI with a task token.** Five env vars are the integration contract (`MULTICA_TOKEN` =
  task-scoped `mat_` credential, `MULTICA_TASK_ID`, `MULTICA_AGENT_ID`, `MULTICA_WORKSPACE_ID`, `MULTICA_SERVER_URL`)
  (`daemon-runtimes.mdx#L90-L100`; prefix `multica/server/internal/cli/client.go#L77`); writes are attributed to the
  agent and task; children inherit the token, no per-child identity (`daemon-runtimes.mdx#L108-L111`).
- **Steering a live run.** Comments posted while an agent runs become `task_supplement` rows
  (pending|delivering|delivered|failed — `multica/server/migrations/538_task_supplement.up.sql#L23`) that the daemon
  claims serially and injects into the *current* turn via provider hooks after a capability handshake
  (`multica/server/internal/daemon/task_supplement.go#L24-L27`, `#L128-L131`; `ExecOptions.EnableTaskSupplement`
  `agent.go#L27-L29`). README: "Claude Code, Codex, and Grok today" (`multica/README.md#L90`).
- **Cloud runtimes** exist as a client (`server/internal/cloudruntime/client.go`) for the hosted product; runs are
  bound to their runtime and "never move to another machine" (`assigning-issues.mdx#L28`).

**Verdict vs theuth.** This is the most engineered part of Multica and the most useful to theuth: a durable run queue
with SKIP-LOCKED claims, per-(agent, issue) serialisation, prepare leases, slot-before-claim, a two-condition
offline rule, typed retryable failures and poisoned-session detection. theuth's desks are long-lived sessions on a
mesh hub with wakes/checkout in `tasks.sqlite`; it has no run record per wake with attempt lineage, no typed failure
taxonomy and no poisoned-session rule. Multica pays for breadth with ~26 per-CLI backends and per-provider
watchdogs (e.g. cursor-specific background/uncaptured watchdog and terminal-boundary test files in `server/internal/daemon/`) — exactly the per-harness plugin cost
Sky ruled out for theuth (ACP-first, 2026-10-09).

## A5 Coordination between agents

**Coordination is the issue thread.** There is no agent-to-agent message bus: agents coordinate by posting comments
that `@`-mention other agents (each mention = a run), by assignment, by sub-issues, and by platform wakeups.

- **Mentions are typed links, not names.** `[@Label](mention://agent|member|squad/<uuid>)`, `mention://issue/<id>` and
  `mention://all/all` (`multica/server/internal/util/mention.go#L11`; test cases `mention_test.go#L15-L48`). "Typing a
  plain `@name` won't trigger anyone" (`multica/apps/docs/content/docs/squads.mdx#L49`); the platform skill tells agents
  "A name is not an id … never invent a UUID" (`multica/server/internal/service/builtin_skills/multica-platform/SKILL.md#L49-L53`).
- **Reply routing without mentions.** A plain reply goes to the agent whose comment it answers, else the agent in that
  discussion, else the issue's agent assignee (squad → leader); explicit mentions switch the fallback off
  (`multica/apps/docs/content/docs/mentioning-agents.mdx#L36-L46`). `/note` and `@all` trigger no agent (`#L54-L61`).
- **Coalescing.** New comments while an agent has a waiting run merge into that run; comments during a running run
  wait and merge into one follow-up (`mentioning-agents.mdx#L48-L52`). The merge targets only a pre-claim task, and on
  merge **re-stamps the originator and connected-app overlay to the newest comment's author** so "a run answering user
  B's comment could [not] execute under user A's connected-app capabilities" (`multica/server/pkg/db/queries/agent.sql#L1893-L1920`);
  the backing unique index is `idx_one_pending_task_per_issue_agent` (`agent.sql#L1918-L1919`). Run rows keep
  `coalesced_comment_ids` (plan) and `delivered_comment_ids` (receipt) (`models.go#L149-L150`).
- **Squads = leader routing.** Assigning an issue to a squad enqueues only the leader; on claim the server appends a
  non-editable **Squad Operating Protocol**, a **Squad Roster** with exact mention markdown per member, and the
  squad's instructions (`multica/apps/docs/content/docs/squads.mdx#L29-L50`). The leader posts one delegation comment,
  records an evaluation with `multica squad activity <issue> action --reason …`, and **stops**; it is re-woken by
  member progress posts, non-member comments and stage barriers, but not by explicit `@` hand-offs or its own comments
  (`squads.mdx#L35-L38`, re-trigger table `#L54-L64`). Leadership is a **per-task role** sent on the wire
  (`is_leader_task`, `squad_id`), not inferred from instruction text any more (`multica/server/internal/daemon/prompt.go#L811-L846`).
  A leader turn that decides nothing records `no_action` (`multica/server/internal/service/squad_no_action.go#L10-L12`).
- **Agent-set wakeups (the most interesting primitive).** Any agent can register an `issue_wakeup` on an issue for
  itself or another agent: `kind` event|at|every|cron, `mode` once|continuous
  (`multica/server/migrations/509_issue_wakeup.up.sql#L5-L6`), with event filters (actor, agent, task), a
  platform-evaluated `condition`, `expires_at`/`expires_in_seconds` with `on_timeout` end|wake, and `max_fires`
  (`multica/server/internal/service/issue_wakeup.go#L30-L59`; table `models.go#L1028-L1066`). Repeating event rules
  default to **20 fires** (`multica/server/internal/service/issue_wakeup_activity.go#L20`). Each matched event writes a
  receipt row with an `event_key` and `coalesce_key` (`models.go#L1068-L1079`), and the claim refuses tasks whose
  wakeup was disabled or edited since enqueue (revision check, `agent.sql#L769`). The platform uses the same rows for
  its own `child_done` stage barrier, which agents cannot edit or delete (`issue_wakeup_system.go#L22-L30`).
- **Locks / checkout.** No explicit checkout. Mutual exclusion comes from (a) per-(agent, issue) claim serialisation
  (`agent.sql#L789-L805`), (b) the pending-run unique index, (c) the path mutex on in-place `local_directory`
  resources (`multica/server/internal/daemon/local_directory.go#L41-L45`) and (d) per-task git worktrees (A4).
  Different agents **may** work the same issue concurrently by design (`agent.sql#L753-L754`).
- **Merge/conflict handling.** None at platform level beyond branches per task and PR status sync
  (`issue_pull_request`, `github_pull_request_check_*`); conflicts are left to the agent and the reviewer.
- **Loop control.** Self-trigger guard for leaders, dedupe of queued/dispatched runs, and the A↔B case is explicitly
  *not* solved: "it does not decide when the collaboration should end — write the stop condition into the agent
  instructions" (`multica/apps/docs/content/docs/mentioning-agents.mdx#L71-L73`). Comments can carry
  `suppressed_agent_ids` (unticked in the trigger preview) (`models.go#L607`; `mentioning-agents.mdx#L30`).
- **Who decides who works next.** Humans (assignment), the squad leader (mentions), Mika (routing), or a wakeup rule.
  There is no scheduler that picks an idle agent for unowned work, no auction/voting, no capability matching.

**Verdict vs theuth.** theuth has direct peer messaging (`send_message`, A2A at the boundary) and checkout with
dispositions; Multica deliberately routes all coordination through the visible issue thread, which buys auditability
and human steerability at the price of latency and a turn per hop. The borrowable pieces are the wakeup rule
(conditions + expiry + max_fires + receipts), comment coalescing with re-stamped authority, and the leader protocol
("delegate, record evaluation, stop").

## A6 Human in the loop & governance

- **No per-tool approvals — by design.** "Multica runs agents unattended, so approval prompts are answered
  automatically … Codex runs with `sandbox_mode = "danger-full-access"` and Claude Code with `--permission-mode
  bypassPermissions`" (`multica/apps/docs/content/docs/security-model.mdx#L44`); code: Claude args carry
  `--permission-mode bypassPermissions` and user overrides of that flag are blocked
  (`multica/server/pkg/agent/claude.go#L1067`, `#L1084`). "By default a run executes with the full permissions of the
  operating-system user running the daemon" and "Multica makes no filesystem-sandbox guarantee"
  (`security-model.mdx#L12-L17`). The isolation boundary is wherever you put the daemon.
- **Gates are at the work level instead.** (1) The assignment confirmation dialog with "Don't start yet"
  (`multica/apps/docs/content/docs/assigning-issues.mdx#L12-L18`, `#L43-L49`); (2) the trigger preview under the comment
  box listing who will wake and why, with per-target untick (`mentioning-agents.mdx#L22-L30`); (3) agents can reach
  `in_review` but `done` is human or a merge integration (`assigning-issues.mdx#L39`); (4) invoke ACLs (A2).
- **Interrupts.** Stop a run from the execution log; changing assignee/status does *not* stop a started run
  (`assigning-issues.mdx#L67-L69`); steering a running agent by commenting into the current turn (A4, task supplements).
- **Inbox.** Members only; covers assignment, subscribed-issue changes, mentions, reactions, agent run failures,
  quick-create results, autopilot creations and auto-pauses (`multica/apps/docs/content/docs/inbox.mdx#L10-L22`);
  multiple notifications on one issue merge into one entry (`#L22`). **Run-failure notifications auto-archive once the
  issue reaches a status that hands it back to a person** (`in_review`, started/done/closed categories), except
  `blocked` (`inbox.mdx#L48`). Six toggleable groups (`#L61-L72`). Table: `inbox_item(type, severity, actor, details)`
  (`models.go#L789-L805`).
- **Audit.** `activity_log` (`models.go#L14-L23`), comment authorship as `member|agent|system|plugin`
  (`multica/server/migrations/362_plugin_hook_engine.up.sql#L64`), per-run originator/accountable/source attribution
  (A2), attribution badges in the issue UI (`packages/views/issues/components/attribution-badge.tsx`), and channel
  inbound audit tables (`channel_inbound_audit`). Agents' writes are attributed to agent + task via the task token
  (`daemon-runtimes.mdx#L108`).
- **Policy.** Workspace-level `attribution_fail_closed` (A2); runtime privacy (A2); plugin scopes (A10). There is no
  policy engine for what an agent may *do* on the machine.
- **Mobile/notifications.** iOS app (Expo, builds from source, not on the App Store) (`multica/README.md#L106`);
  chat channels Slack/Feishu/DingTalk/WeCom/Telegram (`README.md#L105`).

**Verdict vs theuth.** theuth is clearly ahead on *action-level* governance (tools.policy risk classes, approval cards,
the double answering as the owner, sandbox VMs). Multica is ahead on *work-level* gates: the pre-write trigger preview,
the "don't start yet" confirm, review-only status rights for agents, and self-archiving failure notifications.

## A7 Cost, budgets & runaway control

- **Cost attribution, not budgets.** `task_usage` per run × model with input/output/cache-read/cache-write tokens and
  provider-reported `cost_usd_ticks` (1e-10 USD; NULL → priced client-side from a static rate table)
  (`multica/server/pkg/db/generated/models.go#L1542-L1555`); hourly/daily/dashboard rollup tables
  (`task_usage_hourly`, `task_usage_daily`, `task_usage_dashboard_daily`, scheduler `jobs_task_usage.go`). README:
  "See what each run cost, per agent and per issue" (`multica/README.md#L91`); the dashboard has a usage trend card, an
  errors tab and a leaderboard (`packages/views/dashboard/components/`).
- **No spend caps.** A search for `budget` across `server/internal` finds only HTTP/request/retry budgets, no money
  budget, no per-agent or per-workspace spend limit, no auto-pause on cost. The only caps are Cloud entitlements:
  autopilot run quotas reserved in one transaction ("When policy is off, it intentionally uses the legacy direct INSERT
  so a self-hosted deployment never touches the quota tables" — `multica/server/internal/service/autopilot_quota.go#L108-L111`)
  and issue-count limits fetched from Cloud (`multica/server/internal/service/issue_limit.go#L14-L27`).
- **Runaway defences that do exist.**
  - Concurrency ceilings: 20 per daemon, 6 per agent (A4).
  - Retry caps: two executions by default, three for tool network faults; agent-side errors (auth, quota, config,
    model) are not auto-retried (`multica/apps/docs/content/docs/tasks.mdx#L80-L82`; `multica/server/pkg/taskfailure/failure.go#L201-L208`).
  - `iteration_limit` and `agent_blocked` failure reasons (`failure.go#L96`, `#L101`).
  - Wakeup `max_fires` (default 20) and expiry (A5).
  - **Autopilot failure monitor**: auto-pauses an autopilot when, over 7 days, ≥ 50 runs finished with a ≥ 90 % failure
    rate, and notifies the creator (`multica/apps/docs/content/docs/autopilots.mdx#L131`; `MinRuns: 50` at
    `multica/server/cmd/server/autopilot_failure_monitor.go#L43`, message `#L222`).
  - Dedupe/coalescing of triggers and the leader self-trigger guard (A5).
  - Inactivity watchdogs instead of wall-clock limits (A4).
- **Not defended:** agent ↔ agent mention ping-pong ("write the stop condition into the agent instructions",
  `mentioning-agents.mdx#L73`), cost of a squad leader re-woken by every member progress post (`squads.mdx#L54-L60`),
  provider quota exhaustion beyond classifying it (`agent_error.provider_quota_limit`, `failure.go#L206-L208`).
- **Model routing for cost:** per-agent model + thinking level + Codex service tier (`agents.mdx#L20`); nothing automatic.

**Verdict vs theuth.** theuth is ahead: budgets with auto-pause (Paperclip-derived) and a spend ledger. Multica is
ahead on *failure-rate* runaway control (autopilot failure monitor) and wakeup fire caps; both are cheap to add.

## A8 Memory, knowledge & learning

- **Skills are the only durable, shared learning artefact.** `skill(name, description, content, config, created_by,
  plugin_installation_id)` + `skill_file` + `agent_skill` bindings (`multica/server/pkg/db/generated/models.go#L1417-L1437`,
  `models.go#L103-L108` AgentSkill). Sources: manual, local folder/`.skill`/zip, URL (GitHub, ClawHub, Skills.sh), or **copy from a
  runtime** (scan skills installed on a connected computer) (`multica/apps/docs/content/docs/skills.mdx#L20-L31`);
  URL-sourced skills can be refreshed in place keeping identity and bindings (`#L41-L56`). Runtime-local Claude Code /
  Codex skills appear on the agent and can be disabled per agent (`#L96`; column `agent.disabled_runtime_skills`,
  `models.go#L54`). Imported skills are not reviewed, signed or sandboxed (`skills.mdx#L104-L105`).
- **"Compounding" = humans (or agents via CLI) curating skills.** README: "Turn a solved problem into a playbook every
  agent reuses" (`multica/README.md#L73`). The CLI exposes `skill create|update|import|refresh|files upsert`
  (`multica/server/cmd/multica/cmd_skill.go#L41-L102`) and agents drive Multica through the same CLI
  (`README.md#L107`), so an agent *can* write a skill — but nothing in the platform does it automatically, scores
  skills, or learns from run outcomes. No evals.
- **No per-agent memory, no knowledge base.** No memory tables in the migrations (only a comment about "resumed memory"
  in `multica/server/migrations/499_agent_task_issue_snapshot.up.sql#L11`); no `memory` symbols in
  `server/internal/service`. The daemon even disables Hermes' external memory provider for runs
  (`server/internal/daemon/execenv/hermes_home_test.go` `TestHermesDisablesExternalMemoryProvider`). Continuity comes from
  (a) the issue thread itself, (b) CLI session resume per (agent, issue) (A4), (c) **issue snapshots** that let the next
  run be told whether the issue changed since its resumed session was built (`agent_task_queue.issue_snapshot`,
  `models.go#L180`; query comment `multica/server/pkg/db/queries/agent.sql#L837-L839`), (d) workspace `context`
  (`models.go#L1731`) and project resources.
- **Users are asking for it.** Open discussion #1211 "should team knowledge compound across agent runs the way skills
  do?" (14 comments): "skills tell agents how to do things, but there's no equivalent first-class place for what's true
  about a given codebase or product … a Codex run won't see what a Claude run learned" (GitHub, opened 2026-04-16).
- **Context engineering is careful.** Bounded comment reads (`--roots-only --summary --compact`, `--thread … --tail 30`,
  server-computed `--since` deltas) are taught as an invariant in the built-in platform skill
  (`multica/server/internal/service/builtin_skills/multica-platform/SKILL.md#L68-L75`); the skill uses a routing table so
  agents open one reference file, not all eight (`#L18-L38`); the brief is a sectioned file in the workdir (A4).

**Verdict vs theuth.** theuth is far ahead: knowledge base with hints, nightly dreaming (136), learned skill sidecars,
cases and replay. Multica's lesson is product-side: make knowledge **runtime-agnostic and team-scoped**, and keep the
per-run read *bounded by construction* (delta reads computed by the server).

## A9 Observability & UX

- **Surfaces.** Web (Next.js), desktop (Electron; starts the daemon and registers the machine as a runtime), iOS/iPad
  (Expo; builds from source, not on the App Store), CLI, and five chat channels (`multica/README.md#L105-L107`,
  `#L113-L116`; package boundaries `multica/AGENTS.md#L13-L22`). Shared views live in `packages/views` (agents, issues,
  my-issues, inbox, squads, runtimes, autopilots, skills, projects, chat, dashboard, plugins, search …).
- **Board-first.** Kanban `board-view.tsx`/`board-card.tsx` with agents and humans as assignees
  (`packages/views/issues/components/`), saved issue views (`issue_view`, private|workspace — `multica/server/migrations/265_issue_view.up.sql#L15`),
  custom properties incl. actor/multi-actor types (`564_issue_property_list_types.up.sql#L17`).
- **Run transcript = execution log.** Every run streams `task_message(seq, type, tool, content, input, output,
  output_truncated, call_id)` (`models.go#L1494-L1506`) — "Replay every tool call, command, and error, timestamped"
  (`multica/README.md#L89`). Runs are listed per issue with their trigger summary and status (`trigger_summary`,
  `models.go#L133`).
- **Agent presence.** Availability (runtime) and workload (runs) shown side by side (`multica/apps/docs/content/docs/agents.mdx#L67-L72`);
  UI pieces `agent-presence-indicator.tsx`, `agent-live-peek-card.tsx`, activity hover and sparkline in
  `packages/views/agents/components/`. Runtimes page shows one row per detected CLI with version
  (`daemon-runtimes.mdx#L59`).
- **Dashboard.** Usage trend, errors tab, leaderboard (`packages/views/dashboard/components/`); cost per agent and per
  issue (A7).
- **Trigger preview** before sending a comment or changing assignee/status (A3/A6) — the single most distinctive UX
  idea here: the UI tells you which agents will wake, why, and which can't.
- **Chat** with an agent outside issues; Mika answers in chat and creates issues for real work
  (`multica/server/internal/service/builtin_agents/mika/INSTRUCTIONS.md#L8-L13`).
- **Onboarding to first result.** Three steps: connect a computer → create an agent (or "Build with AI") → assign an
  issue (`multica/README.md#L145-L157`); Desktop auto-detects installed CLIs (`#L114-L115`). Self-host is one script +
  `multica setup self-host` with Docker images (`#L127-L137`).
- **Missing:** no org chart view (there is no org), no trace/span view across agents, no cross-issue "what is every
  agent doing right now" timeline beyond presence + peek.

**Verdict vs theuth.** Comparable breadth (theuth: desktop, TUI, web, phone, Slack/Signal/Telegram). Multica's board is
the product; theuth's HUD/attention inbox and activity monitor (155) are richer for one operator. Borrow: trigger
preview, live peek card per agent, per-run replayable message log with seq + call_id.

## A10 Extensibility & integration

- **Plugin contract (v1).** "A plugin relates to Multica in exactly three ways: Action (plugin → Multica), Hook
  (Multica → plugin), Resource (no call at all)" (`multica/server/pkg/plugincontract/manifest.go#L5-L12`).
  Surfaces are host-mounted iframes at fixed places: `issue_panel`, `sidebar_panel`, `modal` (`#L43-L48`). Hook
  triggers `ui|manual|agent|event|schedule` — "Declaring a trigger says who may invoke the hook, never what the hook
  does" (`#L50-L59`); scheduled hooks have a 5-minute floor (`#L61-L65`); transports `http|mcp` (`#L69-L73`); config
  fields are a tiny schema (string/number/bool/enum/secret) from which the **host renders the form** (`#L80-L88`);
  scopes are a closed list plus `net:<domain>`, which drives both the iframe CSP and the hook host check (`#L90-L107`);
  subscribable events: issue created/updated/status-changed, comment created, task started/completed/failed
  (`#L109-L118`).
- **Agent-callable hooks without leaking secrets.** A hook with the `agent` trigger is rendered by the daemon as an MCP
  tool (hook description → tool description, input_schema → tool schema), but the call goes back to the server, which
  signs the request, so laptops never hold plugin signing secrets and rate limit / circuit breaker / `net:` check /
  invocation record are shared by all triggers (`multica/server/internal/daemon/plugin_hook_mcp.go#L16-L28`;
  `multica/server/internal/service/plugin_agent_tools.go#L16-L29`). No hook may sit *inside* the agent loop ("a third
  party holding the product's main loop open … no such position exists anywhere in this design", `#L20-L23`).
- **Example plugins.** `deploy-sentinel` (incident ↔ deploy correlation, rollback request; scopes, secret config,
  issue panel, agent/manual/ui hook — `multica/examples/plugins/deploy-sentinel/multica.plugin.json#L9-L78`),
  `hello-panel`, `release-checklist`, `schedule-pulse`, `triage-notify`.
- **Plugin packaging.** Immutable releases + workspace installations, grants, secrets, storage, health, invocations,
  hook schedules (`plugin_*` tables, 21 of them); contribution types `agent.skill.v1` and `tool.remote-mcp.v1` with an
  encrypted per-installation credential store (`multica/server/migrations/319_remote_mcp_plugin_v1.up.sql#L1-L22`).
- **MCP.** Per-agent `mcp_config`, workspace MCP servers (`workspace_mcp_server`), Composio toolkits mounted per agent
  using the *agent owner's* connection (`models.go#L48-L49`), a daemon-side remote-MCP broker (`server/internal/daemon/remote_mcp_broker.go`).
- **Public API & CLI.** Public API v1 with an OpenAPI document; the plugin API is its first consumer
  (`multica/server/pkg/publicapi/v1/routes.go#L1-L5`, `spec.go#L11`); PATs; "Every surface is scriptable. Agents drive
  Multica through the same CLI you do" (`multica/README.md#L107`) — the CLI is also how agents act (A4).
- **Webhooks in.** Autopilot triggers of kind `schedule|webhook|api` (`multica/server/migrations/042_autopilot.up.sql#L33`)
  with provider, signing secret and event filters (`models.go#L299-L314`); `webhook_delivery` queue with SKIP LOCKED
  claims (`multica/server/pkg/db/queries/webhook_delivery.sql#L72-L84`).
- **Chat channels.** Slack, Feishu/Lark, DingTalk, WeCom, Telegram (`multica/README.md#L105`) via a generalised
  `channel_*` schema (bindings, inbound dedup/audit, outbound/card messages, typing reactions, task delivery).
- **Git hosts.** GitHub App + GitLab/Gitea/Forgejo via `vcs_connection` (A3).
- **ACP.** Used internally as the wire to some CLIs (`server/pkg/agent/acp_session.go`; Grok `agent --always-approve
  stdio`, Qoder `--yolo --acp` — `multica/server/pkg/agent/qoder.go#L17-L30`). No A2A protocol; their "A2A" means
  agent-mentions-agent authorisation (A2).

**Verdict vs theuth.** theuth has MCP servers as files, skills repos, ACP subagents, A2A at the boundary, Slack/Signal/
Telegram. Multica's plugin contract is the more *product-shaped* design: closed scopes, host-rendered config, iframes,
and agent-visible tools that never carry the secret. theuth has no third-party UI surface contract at all.

## A11 Engineering, licence & traction

- **Stack.** Go 1.26.9 backend (Chi, sqlc, gorilla/websocket) on PostgreSQL 17 with `pgcrypto` + `pg_trgm`
  (`multica/README.md#L239-L241`; `server/go.mod`), Redis for liveness ("Redis liveness + DB stale" — `multica/server/cmd/server/runtime_sweeper.go#L85-L87`), TypeScript monorepo (pnpm 10.28, Node 22,
  turbo): Next.js web, Electron desktop, Expo mobile, Fumadocs docs (`multica/AGENTS.md#L13-L22`, `README.md#L249`).
- **Size (shallow clone c8d6a1f, counted with `find`).** 1,982 Go files of which 1,181 are `_test.go`; 2,463 TS/TSX files
  in `packages/` + `apps/` of which 783 are tests; **598 up-migrations numbered up to 572** (1,196 files incl. downs;
  last is `572_channel_typing_abandoned_idx`); `server/internal/daemon/daemon.go` alone is 10,633 lines /
  463 KB and `server/pkg/db/queries/agent.sql` 2,859 lines — a sign of a fast-moving, issue-ticket-driven codebase
  (comments cite `MUL-####` and `GH #####` everywhere, e.g. `multica/server/pkg/db/generated/models.go#L145`).
- **Tests and rules.** Strong test culture: DB-backed Go tests via fixtures, default tests may not execute real agent
  CLIs, real-agent smoke tests gated behind a build tag and an env var (`multica/AGENTS.md#L118-L120`); e2e with
  Playwright; i18n parity tests (`agents-i18n-parity.test.ts`); docs in en/zh/ja/ko/fr.
- **Release cadence.** "We release most weekdays" (`multica/README.md#L262`); GitHub releases v0.4.39 (2026-09-03) →
  v0.6.1 (2026-10-01): 12 releases in 4 weeks; still 0.x.
- **Traction (GitHub MCP, 2026-10-10).** 52,385 stars, 6,821 forks, created 2026-01-13, pushed 2026-10-10; 1,012 open
  and 1,776 closed issues, 843 open PRs; HEAD commit references PR #9128 — ~9.1k issues+PRs in nine months.
  Licence shows as "Other" (NOASSERTION) on GitHub.
- **Licence (exact).** "Multica is provided under the terms of the Apache License, Version 2.0 … subject to the
  additional conditions in Part I" (`multica/LICENSE#L3-L5`). Part I (`#L13-L118`):
  - 1a **Hosted or embedded service**: without a commercial licence from Index Labs (Hong Kong) Limited "you may not use
    the Multica source code to provide a hosted service to third parties, or embed Multica as a component of a product
    or service that is sold, licensed, or otherwise commercially distributed to third parties" (`#L24-L28`); applies
    even free of charge (`#L33-L37`); internal use within one organisation incl. multiple workspaces is fine (`#L38-L39`);
    publishing a fork's source is not itself a hosted service (`#L40-L47`).
  - 1b **Branding**: no removing/modifying the Multica logo, name or attribution in any "Multica user interface" —
    explicitly `apps/web/`, `apps/desktop/`, `apps/mobile/`, `packages/views/`, `packages/ui/`, the web image and any
    compiled app, even when moved or renamed (`#L49-L66`); not applicable to backend/daemon/CLI-only use (`#L67-L71`).
  - 1c **Attribution** for backend/daemon/CLI-only products: keep notices and state in user-facing docs that the product
    is built on Multica with a link (`#L73-L79`). 1d: branding waiver ≠ commercial licence (`#L81-L87`).
  - 2 **Contributions**: the producer may make the licence "more strict or relaxed as deemed necessary", and
    contributions may be used commercially including its cloud business (`#L89-L98`) — a CLA-in-licence.
  - 3 Part I controls on conflict; redistribution must ship the whole file (`#L100-L118`).
  - For theuth: irrelevant to *techniques* (Sky's rule), but it rules out lifting UI code or embedding the server.
- **Business model.** Open-core SaaS: Multica Cloud (multica.ai), entitlement client for quotas/limits (`server/internal/entitlement`,
  A7), seat capacity outbox, contact-sales table, telemetry from self-hosted instances (daily anonymous snapshot,
  `DO_NOT_TRACK=1` to disable — `multica/README.md#L139-L141`).
- **Maturity.** Pre-1.0 but heavily used; the codebase shows hardening after real incidents (attribution waterfall
  MUL-4302, invocation permission MUL-3963, comment-coalescing capability bleed MUL-4195). Docs say what is live
  and VISION says what is not (`multica/VISION.md#L94-L98`).

## Map to the three questions

### Q1
What Multica offers versus theuth, per axis (theirs / ours / gap). "ours" is from AXES' theuth paragraph and the theuth
tree read-only (`theuth/crates/tasks/migrations/0001_tasks.sql#L5-L63`); the `ours` report is the authority.

| Axis | Multica | theuth | Gap |
|---|---|---|---|
| A1 agent | DB row: instructions, model, runtime binding, ACL; 26 CLI backends; custom runtime profiles; built-in router agent Mika | markdown agent file: title, charter, reports_to, model/effort/tools/mcp/budget/capabilities; desk session; ACP harnesses | Multica ahead on harness breadth + router agent; theuth ahead on identity/charter/budget |
| A2 org | flat: workspace roles owner/admin/member, squads (leader agent + members), per-agent invoke ACL, originator-carried A2A authority | org chart + reporting lines + escalate; tools.policy; double | theuth ahead on hierarchy; Multica ahead on *who may wake whom* and attribution |
| A3 work | issue + many runs; custom statuses; agents may only reach in_review; sub-issue stages; trigger preview; pending-run unique index | tasks board (backlog…cancelled, owner, reviewer, done_when, dispositions, threads) | Multica: first-class run record, preview, stage barrier; theuth: acceptance criteria and dispositions actually used |
| A4 runtime | Postgres queue, SKIP LOCKED claim, per-(agent,issue) serialisation, prepare lease, slot-before-claim, 15 s heartbeat / 150 s stale, typed failures, poisoned sessions, worktrees, task tokens, live steering | mesh hub desks, wakes with coalescing key + claimed_by (`theuth/crates/tasks/migrations/0001_tasks.sql#L34-L44`), checkout epoch, ACP/omp children, sandbox VMs, remote nodes | Multica ahead on run durability/retry semantics; theuth ahead on placement (mesh, VMs) and long-lived context |
| A5 coordination | issue thread only: typed mention links, reply routing, comment coalescing, squad leader protocol, agent-set wakeups with conditions/expiry/max_fires | send_message peers, mentions in task threads, checkout, wakes | Multica ahead on wake rules + safe mentions; theuth ahead on direct messaging |
| A6 HITL | no tool approvals (bypassPermissions), gates at work level (confirm dialog, preview, review-only status), members-only inbox with auto-archiving failures | approvals per tools.policy, double, attention inbox/HUD | theuth ahead on action gating; Multica ahead on pre-write wake preview |
| A7 cost | per-run token/cost attribution, dashboard; no budgets; autopilot failure-rate auto-pause; wakeup max_fires | budgets with auto-pause, spend ledger | theuth ahead on caps; Multica ahead on failure-rate pause |
| A8 memory | skills only (curated, importable, refreshable); no memory; users asking (#1211) | knowledge base, dreaming, learned skills, autoresearch | theuth far ahead |
| A9 UX | board-first web/desktop/iOS/CLI/5 channels, execution log replay, presence + live peek, dashboard | desktop, TUI, web, phone, Slack/Signal/Telegram, HUD, activity (155) | parity; borrow trigger preview + peek |
| A10 ext | plugin contract (actions/hooks/resources, iframes, closed scopes, host-rendered config, server-signed agent tools), public API v1, webhooks | MCP files, skills repos, ACP, A2A, routines | Multica ahead on third-party UI/plugin contract |
| A11 eng | Go+TS, 598 migrations, 0.x, near-daily releases, 52k★, source-available licence | Rust, single owner | — |

### Q2
Borrow candidates — techniques only, each with the mechanism, the cite, and the cost of being wrong.

1. **Typed mention links + a pre-send wake preview + a silent-note prefix.** Mentions are `[@Name](mention://agent/<uuid>)`;
   plain `@name` and agent names in text never wake anyone (`multica/server/internal/util/mention.go#L11`,
   `multica/apps/docs/content/docs/squads.mdx#L49`); one predicate `WillEnqueueRun` answers "will this write start a run,
   and for whom" for both the write path and the preview (`multica/server/internal/service/issue_trigger.go#L72-L96`);
   `/note` posts without waking (`mentioning-agents.mdx#L54-L61`). theuth evidence: on 2026-10-05 a supervisor note that
   *named* five desks woke all five and their replies re-woke the manager three times (knowledge
   `facts/sre-supervisor-note-naming-desks-wakes-them-2026-10-05`). Cost of being wrong: low — mentions need a picker or
   an explicit token; plain names stop working as wake signals (that is the point).
2. **A run record under every wake.** One row per execution with `attempt/max_attempts`, typed `failure_reason`,
   `session_id`, `work_dir`, `trigger_comment_id`, `trigger_summary`, `retry_of_task_id` vs `rerun_of_task_id`
   (`multica/server/pkg/db/generated/models.go#L110-L181`, lineage `#L157-L160`). theuth's `wakes` row has
   agent/task/reason/key/count/claimed_by only (`theuth/crates/tasks/migrations/0001_tasks.sql#L34-L44`). Cost: additive
   table + UI; wrong only if desks stay one-session-forever, which they do not across restarts.
3. **Typed failure taxonomy with retryability.** `taskfailure.Reason` splits infra (runtime_offline, queued_expired,
   runtime_recovery, timeout) from `agent_error.*` provider classes (auth, quota, rate limit, 5xx, network, context
   overflow, missing executable) and marks auth/quota not retryable (`multica/server/pkg/taskfailure/failure.go#L85-L267`,
   `#L201-L208`; docs table `tasks.mdx#L96-L123`). Cost: low; mis-classification just means a wrong auto-retry.
4. **Poisoned-session rule + continuity notice.** Never resume a session whose last run ended in context exhaustion, a
   fallback marker or a provider 400; start fresh in the same workdir and prepend a caller-worded notice
   (`multica/server/internal/daemon/poisoned.go#L10-L24`, `multica/server/pkg/agent/agent.go#L76-L96`). Cost: low; worst case
   an unnecessary fresh session.
5. **Agent-set conditional wakeups.** `issue_wakeup` = kind event|at|every|cron × mode once|continuous, filters,
   platform-evaluated condition, `expires_*` + `on_timeout` end|wake, `max_fires` (default 20), per-event receipts with
   coalesce keys, revision-checked at claim (`multica/server/internal/service/issue_wakeup.go#L30-L59`,
   `issue_wakeup_activity.go#L20`, `agent.sql#L769`). theuth has session reminders and schedule files but no
   "wake desk X when task T gets event E, at most N times, give up at D". Cost: medium; wrong if conditions become a
   second rules engine nobody debugs — keep the condition grammar tiny.
6. **Stage barrier for sub-work.** A platform-owned wake of the parent's owner when a stage of children closes, and once
   when all are closed; agents cannot delete it (`multica/server/internal/service/issue_wakeup_system.go#L22-L30`). Cost: low.
7. **Claim discipline.** Per-(agent, task) serialisation inside the atomic claim, a partial unique index for one pending
   run per (task, agent), a prepare lease refreshed every 15 s, and slot-before-claim on the worker
   (`multica/server/pkg/db/queries/agent.sql#L749-L810`, `#L1918-L1919`; `multica/server/internal/daemon/daemon.go#L148`,
   `#L5396-L5405`). Cost: low; theuth already coalesces wakes by key — the lease and slot rules are the missing halves.
8. **Two-condition offline rule.** Fail queued work only when the worker has been silent beyond grace *and* the work has
   waited that long, so busy ≠ dead and late-assigned work gets a full grace (`multica/apps/docs/content/docs/daemon-runtimes.mdx#L63`;
   150 s window `multica/server/internal/service/task.go#L189`). Cost: very low; applies to mesh nodes and desks.
9. **Authority travels with the human originator.** Agent→agent wakes are authorised by the top-of-chain human, never
   by the immediate agent; accountable human (audit/cost) is stored separately from authorising human; a workspace can
   fail closed when no human resolves (`multica/server/internal/handler/agent_access.go#L33-L36`, `#L73-L90`;
   `models.go#L145-L168`, `#L1736-L1737`). Cost: medium; they needed a carve-out for workspace-wide automation
   (`agent_access.go#L97-L107`) — expect the same for routines.
10. **Coalescing that re-stamps authority.** When a new comment merges into a pending run, originator and capability
    overlay move to the newest author, so no run answers B under A's capabilities (`agent.sql#L1909-L1920`). Cost: low.
11. **Failure-rate auto-pause for routines.** ≥ 50 runs in 7 days with ≥ 90 % failures → pause + notify creator
    (`multica/apps/docs/content/docs/autopilots.mdx#L131`; `multica/server/cmd/server/autopilot_failure_monitor.go#L43`, `#L222`).
    Cost: very low.
12. **Self-archiving failure notifications** once the work is handed back to a person (review/done), except `blocked`
    (`multica/apps/docs/content/docs/inbox.mdx#L48`). Cost: very low.
13. **Leader protocol as a fixed, non-editable block + roster with exact mention syntax + "record evaluation, then
    stop"**, with a re-trigger table that skips no-op turns (`squads.mdx#L42-L64`; per-task role on the wire
    `multica/server/internal/daemon/prompt.go#L816-L846`). Cost: prompt-only; low.
14. **Bounded reads as a platform invariant** — server-computed `--since` deltas and summary-first thread scans taught
    in the built-in skill (`multica/server/internal/service/builtin_skills/multica-platform/SKILL.md#L68-L75`), plus an
    issue snapshot telling a resumed session whether the task changed (`agent.sql#L837-L839`). Cost: low.
15. **Plugin contract shape** (later): closed scope list, host-rendered config from a five-type schema, iframe surfaces
    at fixed slots, agent-callable hooks where the server signs the call so workers never hold the secret
    (`multica/server/pkg/plugincontract/manifest.go#L5-L107`; `multica/server/internal/daemon/plugin_hook_mcp.go#L16-L28`).
    Cost: high effort; wrong if theuth never opens a third-party UI surface.

**Do not borrow:** the 26 per-CLI backends and per-provider watchdogs (contradicts Sky's ACP-first, no-per-harness-plugin
ruling of 2026-10-09); unattended `bypassPermissions` as the default (`security-model.mdx#L44`) — theuth's approval
model and the double are differentiators; flat squads as a replacement for reporting lines.

### Q3
Proposals ranked by value ÷ effort (S/M/L), with the theuth seam where known.

1. **Typed mentions + wake preview + `/note`** (value H, effort S–M). Seam: task-thread mention parser in `crates/tasks`
   (wake reason `mention` exists — `theuth/crates/tasks/migrations/0001_tasks.sql#L38`) and the desktop task composer;
   expose the same predicate to the composer as a "will wake: …" line. Fixes a recorded incident.
2. **Failure-rate auto-pause for routines** (value M, effort S). Seam: `crates/schedule` run history → pause + attention
   item; mirrors the budget auto-pause theuth already has.
3. **Poisoned-session rule + continuity notice on desk/ACP resume** (value M–H, effort S). Seam: desk resume in
   `crates/roster`/`crates/agent` and ACP `session/load` in `crates/acp`.
4. **Self-archiving failure items** in the attention inbox when the task moves to review/done (value M, effort S). Seam:
   attention inbox (designs 104/132).
5. **Run ledger under wakes + typed failure taxonomy** (value H, effort M). Seam: a `runs` table beside `wakes` in
   `crates/tasks` (attempt, reason, session, workdir, lineage), surfaced in the task thread and activity monitor (155).
6. **Stage-barrier wake for parent tasks** (value M, effort S). Seam: `crates/tasks` ledger on child state change
   (tasks have `parent`, `0001_tasks.sql#L13`).
7. **Agent-set conditional wakeups with expiry and max_fires** (value H, effort M). Seam: `wakes` + schedule sensors;
   give desks a `wake_me(task, on=event|at|every, until, max_fires)` tool instead of free-form reminders.
8. **Claim lease + two-condition offline rule for mesh placement** (value M, effort S–M). Seam: checkout epoch
   (`checkout_json`, `0001_tasks.sql#L19`) and the mesh node liveness used by placement.
9. **Originator/accountable on every wake + per-agent "who may wake me" ACL** (value M now, H once the mesh is
   multi-user; effort M). Seam: `wakes` row + roster agent file (`capabilities`/new `invokers:` key) + tools.policy.
10. **Bounded "since your last run" delta reads in desk wakes** (value M, effort S). Seam: the desk manual and thread read.
11. **Plugin/UI surface contract** (value uncertain, effort L) — only if Sky wants third-party panels.

## Their weaknesses and failure modes

1. **No isolation, by design.** Runs execute as the daemon's OS user with approvals auto-answered and sandboxes off
   (`multica/apps/docs/content/docs/security-model.mdx#L12-L17`, `#L44`); the task token is inherited by every child
   process (`daemon-runtimes.mdx#L110-L111`); `custom_env` and MCP config live on the server
   (`how-multica-works.mdx#L32-L34`). A prompt-injected issue comment runs with your home directory.
2. **Orchestration is prompt-following.** Decomposition, gates and pacing are left to a leader agent's protocol text
   and Mika's instructions; the docs concede agent↔agent mention loops are unsolved (`mentioning-agents.mdx#L71-L73`).
   The two most-discussed open issues ask for exactly this: #1943 "Workflow Orchestration" (18 comments) and #815
   "Multica still manages AI the way it manages people" (9) — "who decomposes the large task? who decides the next
   stage? … who enforces approval / acceptance gates?".
3. **Queue/claim stalls in the field.** Open #4072 "Agent keeps queuing, task claim timed out", #2341 "Agent stays in
   `Queue` even though there's only 1 task", #3262 "Codex app-server stays at status=running with no output"; closed
   #5404 "tasks wedge in Queued; daemon skips HTTP claim fallback when WS claim outcome …", #2188 "Pi Agents stuck in
   working". The WS-first claim + HTTP fallback + leases + sweepers is powerful and fragile; the volume of
   commentary in `daemon.go` (10,633 lines) and `agent.sql` about races (`agent.sql#L1893-L1920`) is the tell.
4. **Per-CLI tax.** 26 backends with provider-specific invocation, stdout parsing, session locks and watchdogs
   (`server/pkg/agent/*`, e.g. `pi_session_lock_unix.go`, `cursor_background_process_*.go`, `openclaw_stdout.go`);
   each upstream CLI release can break a backend.
5. **Runs are pinned to one machine.** "a run is bound to its runtime and never moves to another machine"
   (`assigning-issues.mdx#L28`); open #1398 asks for a fallback runtime. A dead laptop strands its agents' queue.
6. **No budgets.** Cost is attributed, never capped (A7); quotas exist only as Cloud entitlements
   (`autopilot_quota.go#L108-L111`); open #4513 "Agent runtime token quota attribution is not isolated in team workspaces".
7. **No shared knowledge.** Skills only; #1211 asks for team knowledge that compounds across runtimes (A8).
8. **Schema churn and dead weight.** 598 migrations in nine months, upgrade notes that require backfills before
   migrate (#3015), derived legacy fields (`agent.visibility` "now a derived legacy field" — `models.go#L50-L51`;
   `handoff_note` legacy — `server/internal/handler/agent.go#L517`), an unused `issue_dependency` table and an unused
   `acceptance_criteria` column (A3).
9. **Squad cost.** Leaders are re-woken by most member progress posts (`squads.mdx#L54-L60`) — a turn per hop for
   routing that a scheduler could do.
10. **Agents cannot observe.** No inbox for agents (`agents.mdx#L45`); an agent only learns of change through a trigger
    or an explicit wakeup rule.
11. **Licence and governance risk.** Hosting/embedding needs a commercial licence; the producer may tighten terms
    unilaterally for contributions (`multica/LICENSE#L24-L28`, `#L89-L92`).

## Cites self-check

Re-opened at the end of the run with `read_file(offset, limit)` on the exact cited range and a keyword test (45 cites,
all load-bearing for the summary and Q2/Q3):

- held: `multica/server/pkg/db/queries/agent.sql#L749-L810` ("SKIP LOCKED"); `multica/server/pkg/db/queries/agent.sql#L789-L805` ("waiting_local_directory"); `multica/server/pkg/db/queries/agent.sql#L1918-L1919` ("idx_one_pending_task_per_issue_agent")
- held: `multica/server/pkg/db/queries/agent.sql#L769-L769` ("wakeup_revision"); `multica/server/pkg/db/queries/agent.sql#L837-L839` ("SetTaskIssueSnapshot"); `multica/server/internal/service/issue_trigger.go#L72-L96` ("single predicate")
- held: `multica/server/internal/handler/agent_access.go#L33-L36` ("top-of-chain"); `multica/server/internal/handler/agent_access.go#L86-L90` ("no admin bypass"); `multica/server/internal/daemon/daemon.go#L5396-L5405` ("slot-before-claim")
- held: `multica/server/internal/daemon/daemon.go#L148-L148` ("taskPrepareLeaseRefresh = 15"); `multica/server/internal/daemon/config.go#L30-L30` ("15 * time.Second"); `multica/server/internal/daemon/config.go#L72-L72` ("= 20")
- held: `multica/server/internal/service/task.go#L189-L189` ("150.0"); `multica/server/migrations/023_agent_concurrency_default.up.sql#L1-L1` ("DEFAULT 6"); `multica/server/pkg/agent/agent.go#L138-L149` ("inactivity watchdog")
- held: `multica/server/pkg/agent/agent.go#L32-L40` ("CLAUDE.md"); `multica/server/pkg/agent/agent.go#L17-L23` ("Execute(ctx context.Context"); `multica/server/internal/daemon/poisoned.go#L10-L24` ("poisoned")
- held: `multica/server/pkg/taskfailure/failure.go#L201-L208` ("Not retryable"); `multica/server/internal/service/issue_wakeup.go#L30-L59` ("MaxFires"); `multica/server/internal/service/issue_wakeup_activity.go#L20-L20` ("wakeupDefaultMaxFires    = 20")
- held: `multica/server/internal/service/issue_wakeup_system.go#L22-L30` ("child_done"); `multica/server/migrations/509_issue_wakeup.up.sql#L5-L6` ("'once','continuous'"); `multica/server/pkg/agent/claude.go#L1084-L1084` ("bypassPermissions")
- held: `multica/server/pkg/plugincontract/manifest.go#L5-L12` ("exactly three ways"); `multica/server/internal/daemon/plugin_hook_mcp.go#L16-L28` ("signed request"); `multica/server/cmd/server/autopilot_failure_monitor.go#L43-L43` ("MinRuns:      50")
- held: `multica/server/pkg/db/generated/models.go#L145-L146` ("OriginatorUserID"); `multica/server/pkg/db/generated/models.go#L1736-L1737` ("AttributionFailClosed"); `multica/server/pkg/db/queries/workspace_delete.sql#L366-L366` ("issue_dependency")
- held: `multica/LICENSE#L24-L28` ("hosted service"); `multica/apps/docs/content/docs/security-model.mdx#L44-L44` ("bypassPermissions"); `multica/apps/docs/content/docs/inbox.mdx#L48-L48` ("archive automatically")
- held: `multica/apps/docs/content/docs/daemon-runtimes.mdx#L63-L63` ("reconnect grace"); `multica/server/internal/daemon/prompt.go#L841-L846` ("LeaderRoleResolved"); `multica/server/internal/daemon/task_supplement.go#L128-L131` ("runTaskSupplementLoop")
- held: `multica/server/internal/util/mention.go#L11-L11` ("mention://"); `multica/server/internal/service/autopilot_quota.go#L108-L111` ("self-hosted deployment never touches"); `multica/server/migrations/558_issue_child_event.up.sql#L18-L18` ("restaged")
- held: `multica/server/internal/daemon/execenv/git.go#L76-L99` ("worktree"); `multica/server/internal/handler/agent.go#L517-L517` ("legacy assignment handoff"); `multica/server/cmd/multica/cmd_skill.go#L41-L102` ("upsert")
- held: `theuth/crates/tasks/migrations/0001_tasks.sql#L34-L44` ("claimed_by"); `multica/apps/docs/content/docs/autopilots.mdx#L131-L131` ("90%"); `multica/apps/docs/content/docs/mentioning-agents.mdx#L71-L73` ("stop condition")

Mechanical pass over **every** `#L` cite in this report (293 cites, 265 unique ranges, 80 files): each path resolves to an
existing file in the clone (or the theuth tree) and every range ends within the file's line count — 0 failures.

Corrected during the check:
- **corrected** — migration count: "572 numbered migrations" → 598 up-migrations numbered up to 572 (numbers are reused,
  e.g. `109_agent_task_waiting_local_directory` and `109_lark_integration`); `ls server/migrations/*.up.sql | wc -l` = 598.
- **corrected** — plugin tables: 22 → 21 distinct `plugin_*` names in `CREATE TABLE` statements.
- **corrected** — table count: 134 → 145 distinct `CREATE TABLE` names over time; 124 structs in today's sqlc model.
- **corrected** — three cites written as `StructName#L…` were normalised to `models.go#L…` (GithubPullRequest, VcsConnection,
  AgentSkill).

Partially verified / not verified:
- Autopilot auto-pause: `MinRuns: 50` verified in code (`autopilot_failure_monitor.go#L43`); the 7-day window and 90 %
  ratio are from the docs (`autopilots.mdx#L131`), the config defaults for them were not opened.
- Squad leader re-trigger table (`squads.mdx#L54-L64`) is docs only; I verified the per-task leader role on the wire
  (`prompt.go#L816-L846`) and the no_action evaluation helper, not the comment-trigger code path.
- "Steer a running agent" provider coverage (Claude Code, Codex, Grok) is the README's claim; I verified the supplement
  queue and capability gate, not each provider hook.
- Whether a task-scoped `mat_` token may call `multica skill create` (agents writing skills) was not verified.
- Docs vs code disagreements found: none material. The issue docs mention only parent-child relationships
  (`multica/apps/docs/content/docs/issues.mdx#L18`, `#L107`), consistent with `issue_dependency` being unused in queries.
