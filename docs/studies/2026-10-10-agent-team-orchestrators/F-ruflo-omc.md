# Teams inside one harness — Ruflo (ex claude-flow) and oh-my-claudecode vs theuth (recon, 2026-10-10)

Clones: `ruflo` HEAD 6c04654 (2026-10-09, MIT) · `oh-my-claudecode` HEAD 454bae0 (2026-10-06, MIT, npm `oh-my-claude-sisyphus` 5.6.2). Both shallow (one commit), so no history mining. theuth cites are against main c22556ea, read-only.

## Executive summary

1. **What they are.** Both are Claude Code add-ons that drive a "team" from inside one harness session: hooks, MCP tools, slash commands/skills, agent markdown files, a statusline/HUD. Ruflo also ships a background daemon and a Claude-plus-Codex runner. oh-my-claudecode (omc) also ships a tmux runtime that launches Claude, Codex, Gemini, Cursor, Grok or Antigravity CLIs as workers.
2. **Ruflo's swarm is mostly bookkeeping.** `swarm init` and `agent_spawn` write JSON records. The CLI says so itself: "Use Claude Code Task tool or hive-mind spawn --claude to drive actual agent execution" (`ruflo/v3/@claude-flow/cli/src/commands/swarm.ts#L1046-L1047`). The "hive mind" is one Claude Code process given a prompt telling it to act as Queen (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L163-L169`). Its "consensus" is vote-count thresholds over a JSON file (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L83-L105`). The 22k-line `@claude-flow/swarm` package (queen, Raft, Byzantine, gossip) is not a dependency of the CLI.
3. **omc's team runtime is real, and the LLM is the scheduler.** It runs a file protocol under `.omc/state/team/<team>/`: tasks with claim tokens, 15-minute leases and CAS versions, mailboxes, heartbeats, launch ACKs, checkpoints and an events log. Workers are told in their prompt to drive that protocol through `omc team api … --json` (`oh-my-claudecode/src/team/worker-bootstrap.ts#L284-L344`). That includes writing their own heartbeat file.
4. **Idea 1 for theuth: evidence-gated completion.** omc refuses `completed` unless the result carries verification evidence (`oh-my-claudecode/src/team/state/tasks.ts#L311-L318`). It also keeps an amendment ledger, so a falsified acceptance criterion can be replaced on the record rather than silently dropped (`oh-my-claudecode/src/hooks/ralph/prd.ts#L1-L30`).
5. **Idea 2: a user-global launch fuse for unattended model calls.** Ruflo shipped this after a P0 quota blow-up (#2661). It caps concurrent launches (1), launches per hour (2) and per day (12), and pauses for 60 minutes on a 429. All daemons in all worktrees share one lock, ledger and receipts file (`ruflo/v3/@claude-flow/cli/src/services/global-ai-budget.ts#L1-L48`). One supervisor per repository is elected by heartbeat (`ruflo/v3/@claude-flow/cli/src/services/repo-supervisor.ts#L1-L40`).
6. **Idea 3: shadow first, then active, for every new guard or judgement.** omc runs its budget guard and its "Jev" classifier points in shadow by default: they log what they would have decided and act only once promoted (`oh-my-claudecode/scripts/budget-guard.mjs#L1-L23`, `oh-my-claudecode/src/hooks/jev/config.ts#L1-L28`).
7. **Runners-up.** omc's merge orchestrator: a merger worktree auto-merges each worker's branch, rebases the siblings, and posts conflicts to the leader's inbox. Ruflo's plan-time rule that concurrent writers must use distinct worktrees. omc's reply-to-continue from Discord/Telegram/Slack and its rate-limit wait-and-resume daemon.
8. **Memory claims vs code.** Ruflo's prompt-time "intelligence" is trigram Jaccard (0.6) plus PageRank (0.4), top 5 (`ruflo/.claude/helpers/intelligence.cjs#L700-L719`). Its router is a static keyword table that admits it is "*not* a learned model" (`ruflo/.claude/helpers/router.cjs#L1-L14`), yet the README claims "Intelligent routing (89% accuracy)". Ruflo's own audit found mock embeddings mislabelled as ONNX and a "Flash Attention" speedup that is fabricated with `Math.random()`.
9. **Does memory help?** Neither project measures outcomes. Ruflo's only multi-agent benchmark run in the tree is `"backend": "mock"`: 100 runs per task finished in 4 ms. omc's notepad, project memory and learner are heuristic injectors (regex directives, a 500-char priority block).
10. **Why the stars.** Zero-install entry inside the tool users already live in (plugin marketplace, `/team`, `ralph`, the HUD), heavy README marketing and a fast release cadence. omc's "keep going until done" Stop-hook loop and its HUD are what users actually exercise; the issues are about loops, tmux and keyword false-triggers.
11. **Where theuth is ahead.** Engine-owned protocol (checkout and epoch, sweeps with liveness probes, budgets that really pause), real separate sessions on any harness, placement across machines, approvals enforced by policy rather than by prompt. Neither project has an org chart, reporting lines or a board.
12. **Verdict.** Borrow three cheap mechanisms (evidence-gated done, the global launch fuse, shadow rollout) and one bigger one (the merge coordinator for worktree waves). Do not borrow topologies, "hive mind" or consensus; in both repos they are prompt text over JSON.

## A1 Agent model & identity

### Ruflo
- **What an agent is.** It depends on the path.
  - An **agent definition** is a Claude Code subagent markdown file: 108 under `ruflo/.claude/agents/`. The frontmatter holds only `name` and `description` (`ruflo/.claude/agents/core/coder.md#L1-L4`). The body is a long role prompt that points at `docs/SPEC.md` and `docs/adr/*.md` as binding contracts between parallel agents (`ruflo/.claude/agents/core/coder.md#L10-L19`).
  - A **runtime agent** is a JSON `AgentRecord` in `.claude-flow/agents/store.json`. Fields: `agentId`, `agentType`, `status idle|busy|terminated`, `health`, `taskCount`, `model`, `modelRoutedBy`, `provider`, optional `memoryBranch` (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L16-L60`).
  - Hive-mind workers live in a *different* file, `.claude-flow/agents.json`. The code merges the two after a bug (#1916) (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L20-L24`).
- **Model binding.** "ADR-026 3-tier routing" picks a model per agent type, then falls back to `sonnet` (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L270-L277`). The provider can be `anthropic | openrouter | ollama` (`#L332-L358` of the same file).
- **Catalogue.** 108 agent files, 168 command files under `.claude/commands`, 135 skills under `.agents/skills`, 47 plugins under `plugins/`. The README advertises "100+ Agents" and "98 agents, 60+ commands, 30 skills" (`ruflo/README.md#L165`, `#L316`). The counts disagree with each other, but the order of magnitude is right.
- **Lifecycle.** `idle/busy/terminated` on the record only. No process is behind it unless one of three execution paths is taken (see A4). No hiring, onboarding, paused state or export of a roster.

### oh-my-claudecode
- **Agent catalogue.** 19 markdown agents in `agents/`: analyst, architect, critic, executor, planner, verifier and others. The frontmatter adds `model:` (haiku/sonnet/opus) and `level:` (`oh-my-claudecode/agents/executor.md#L1-L5`). Bodies are XML-tagged prompts (`<Role>`, `<Why_This_Matters>`).
- **Model enforcement.** A "delegation enforcer" injects the agent's default model into every Task/Agent call, because Claude Code does not apply agent models by itself. For non-Claude providers it strips the tier names (`oh-my-claudecode/src/features/delegation-enforcer.ts#L1-L13`).
- **Cross-harness identity.** A tmux worker has a `CliAgentType`: `claude | codex | gemini | cursor | grok | antigravity`. Its contract covers binary, launch args, output parser and prompt-mode flag (`oh-my-claudecode/src/team/model-contract.ts#L10-L22`). The binary must resolve under a trusted prefix (`/usr/bin`, `/opt/homebrew/`, `~/.local/bin`…, plus `OMC_TRUSTED_CLI_DIRS`) and never under `/tmp` (`oh-my-claudecode/src/team/model-contract.ts#L54-L98`).
- **Per-worker identity on disk.** `workers/<name>/identity.json`, an `AGENTS.md` overlay with role guidance, and launch attempts with ids. A status write must carry the exact `launch_attempt_id`, or it is ignored as evidence from another attempt (`oh-my-claudecode/src/team/state-paths.ts#L119-L153`, `oh-my-claudecode/src/team/worker-bootstrap.ts#L305-L312`).

**vs theuth.** theuth's agent is a durable roster file with title, `reports_to`, charter, model/effort/tools/mcp/budget and paused, plus a desk session (design 67). Neither project has a roster or a durable agent that outlives a session. omc's per-attempt launch id is worth noting: every status write is tagged with the attempt that produced it.

## A2 Organisation & authority

### Ruflo
- There is no org chart. "Queen" and "workers" are roles written into one prompt: the Queen is told to decompose, delegate, aggregate and "resolve conflicts using ${consensusAlgorithm} consensus" (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L150-L161`).
- **Authority over hive state.** Remote/HTTP callers need a capability token minted by `hive-mind_init`, or an operator secret (ADR-476). Local stdio callers skip both. Membership changes and proposals "fail closed" without a token (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L700-L728`, `#L762-L774`).
- **Humans.** The claims service models `Claimant = human | agent`, with `handoff-pending`, `review-requested` and work-stealing states (`ruflo/v3/@claude-flow/cli/src/services/claim-service.ts#L1-L43`). This is the only place a human appears as a peer of agents.
- Federation (mTLS + ed25519, trust levels that upgrade or downgrade "without a human in the loop") is README-heavy (`ruflo/README.md#L342`). Not traced in depth here.

### oh-my-claudecode
- **Two-level authority.** One leader (the user's Claude Code session) and N workers. Worker prompts state "You are NOT the leader", "Do NOT spawn sub-agents", "Do NOT create tmux panes", and only `omc team api … --json` is allowed as a control surface (`oh-my-claudecode/src/team/worker-bootstrap.ts#L336-L344`).
- **Team governance flags, with defaults** (`oh-my-claudecode/src/team/governance.ts#L19-L25`):
  - `delegation_only: false`
  - `plan_approval_required: false`
  - `nested_teams_allowed: false`
  - `one_team_per_leader_session: true`
  - `cleanup_requires_all_workers_inactive: true`
- **Worker cap.** 20, clamped (`oh-my-claudecode/src/team/governance.ts#L27-L41`).
- **Humans** sit outside: plan approvals as files (`approvals/<taskId>.json`, `oh-my-claudecode/src/team/state-paths.ts#L181-L182`), and remote approval decisions (A6).

**vs theuth.** theuth has reporting lines, escalation up the line, reviewers and the double (126). theuth is ahead here. omc's explicit worker "MUST NOT" list is a cheap analogue of theuth's desk manual, enforced only by prompt.

## A3 Work model

### Ruflo
- **Tasks.** MCP `task_create/assign/complete` over a JSON record store (`ruflo/v3/@claude-flow/cli/src/mcp-tools/task-tools.ts`, `name: 'task_complete'` at `#L287` and `name: 'task_assign'` at `#L435`). The hive prompt tells the Queen to "use mcp__ruflo__task_assign … then mcp__ruflo__task_complete when done" (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L175`).
- **Claims (ADR-016)** add the useful extras:
  - statuses `active | paused | handoff-pending | review-requested | blocked | stealable | completed`;
  - steal reasons `overloaded | stale | blocked-timeout | voluntary`;
  - a 0–100 progress field (`ruflo/v3/@claude-flow/cli/src/services/claim-service.ts#L26-L57`);
  - a stale claim (default 30 min) becomes `stealable` (`ruflo/v3/@claude-flow/cli/src/services/claim-service.ts#L162`, `#L709-L720`).
- **Planning.** Methodology commands (SPARC, ADR/DDD plugins) and prompts. There is no engine-level decomposition.

### oh-my-claudecode
- **Team tasks are JSON files** with `depends_on` (canonical) and `blocked_by` (legacy alias), statuses and an owner (`oh-my-claudecode/src/team/state/tasks.ts#L37-L60`).
- **Claiming.** Compare-and-swap on `version` (`claim_conflict`), a fresh UUID claim token and a 15-minute lease (`oh-my-claudecode/src/team/state/tasks.ts#L170-L223`).
- **Transitions** need the matching token and an unexpired lease (`lease_expired`). They are refused without delegation-compliance evidence (`oh-my-claudecode/src/team/state/tasks.ts#L287-L318`).
- **Pipeline.** Staged: `team-plan -> team-prd -> team-exec -> team-verify -> team-fix (loop)`, with a routing table per stage. For example, `team-verify` always runs `verifier` and adds `security-reviewer` for auth or crypto changes (`oh-my-claudecode/skills/team/SKILL.md#L99-L111`).
- **Ralph PRD.** `prd.json` holds user stories with `acceptanceCriteria`, `priority` and `passes` (`oh-my-claudecode/src/hooks/ralph/prd.ts#L1-L13`).
  - A completion claim is bound to a criteria revision.
  - A criterion that the work disproves is amended with evidence and the original kept verbatim, not deleted (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1533-L1535`).
  - This came from user issue #3664: "a criterion disproved mid-run cannot be amended, so the loop verifies against a falsified requirement".

**vs theuth.** theuth's board already has states, an owner, a reviewer, `done_when`, blockers and dispositions (`theuth/crates/tasks/src/ledger.rs#L1405-L1424`). What is new to borrow: (a) refusing `done` without evidence, (b) an amendment ledger for `done_when`, (c) work-stealing semantics on stale claims. theuth's sweep re-wakes the owner instead (A4).

## A4 Execution & runtime

### Ruflo — four execution paths
1. **Inside the user's Claude Code session.** The user or the model calls the native Task tool. Ruflo only records metadata. The `agent_spawn` response says: "Agent registered for coordination. Three execution paths: (1) agent_execute … (2) Claude Code Task tool — spawns a real subagent; (3) claude -p" (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L444-L450`).
2. **`agent_execute`.** One Anthropic Messages API call with `max_tokens` defaulting to 1024. The request has no `tools` field, so the agent cannot act on files (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-execute-core.ts#L245-L262`). The code admits the gap it closed: "Previously agent_spawn registered metadata but nothing dispatched work to a provider … as the April audit called out" (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L467-L474`).
3. **`hive-mind spawn --claude`.**
   - Writes a Queen prompt to `.hive-mind/sessions/hive-mind-prompt-<id>.txt` (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L228-L235`).
   - Spawns **one** `claude` child, interactive or `-p --output-format stream-json`, with a generated `--mcp-config`. `--dangerously-skip-permissions` is opt-in since a strict-boolean fix (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L250-L316`).
   - Resume means re-running the saved prompt file (`#L338`, `#L399-L401`).
4. **Background daemon.**
   - Interval workers: `map`, `audit`, `optimize`, `consolidate`, `testgaps`, … (`ruflo/v3/@claude-flow/cli/src/services/worker-daemon.ts#L1-L12`, `#L44-L59`).
   - Some can be promoted to headless `claude --print --output-format json` (`ruflo/v3/@claude-flow/cli/src/services/headless-worker-executor.ts#L1397`).
   - Hard-won lifecycle limits: `resourceThresholds {maxCpuLoad, minFreeMemoryPercent}`; `ttlMs` and `idleShutdownMs`, so "a forgotten daemon cannot run for days"; and an explicit consent gate `aiWorkersEnabled`, default false, so finding `claude` on PATH does not authorise recurring model calls (`ruflo/v3/@claude-flow/cli/src/services/worker-daemon.ts#L111-L135`).
   - Crash accounting: `lastStartedAt > lastRun` means the run failed (`#L77-L83`).

**Cross-harness (Ruflo dual-mode).**
- Runs `claude -p` and `codex exec --sandbox read-only|workspace-write` workers as child processes. Workers form `dependsOn` levels, honour a per-worker `worktreePath` and `readOnly`, and run under a capability envelope (`ruflo/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts#L19-L43`, `#L211-L243`).
- A real fix: `codex exec` hung waiting for stdin EOF until stdin was closed (`#L245-L251`).
- **Plan-time isolation:** concurrent writers must use distinct worktrees, else it throws, and each batch is capped at `maxWriters` (`ruflo/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts#L445-L476`).

**Hooks into Claude Code** (`ruflo/.claude/settings.json#L45-L160`):

| Event | Ruflo handler |
|---|---|
| PreToolUse(Bash) | `pre-bash`: a short dangerous-command list |
| PostToolUse(Write\|Edit\|MultiEdit) | `post-edit`: logs the edit, including failures |
| UserPromptSubmit | `route`: memory injection plus keyword router |
| SessionStart | `session-restore` and auto-memory import |
| SessionEnd | `session-end` |
| Stop | auto-memory sync |
| PreCompact (manual, auto) | compact handlers |
| SubagentStop | `post-task`: confidence feedback |

- The statusline is a 1,328-line Node script (`ruflo/.claude/settings.json#L266`).
- Hooks "never crash Claude Code": every handler error is swallowed (`ruflo/.claude/helpers/hook-handler.cjs#L738-L745`).

**Liveness.** Only the daemon's own heartbeat (repo supervisor stale after 3 min, `ruflo/v3/@claude-flow/cli/src/services/repo-supervisor.ts#L35-L40`). Agent records have a `health` number that nothing measures.

### oh-my-claudecode — two team runtimes that "do not share a result"
1. **`/team` skill.** Uses Claude Code's implicit agent team (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`). Teammates are spawned with the Agent/Task tool under distinct names, after Claude Code 2.1.178 removed native `TeamCreate`/`TeamDelete` (`oh-my-claudecode/skills/team/SKILL.md#L1-L11`). The lead monitors through the native team surface (`#L49-L80`).
2. **`omc team` (tmux runtime v2).** "Event-driven … NO done.json polling". Completion comes from CLI API lifecycle transitions, monitor snapshots and worker heartbeat/status files (`oh-my-claudecode/src/team/runtime-v2.ts#L1-L17`).
   - **Workers.** CLI processes in owned tmux panes. The startup inbox is delivered and its ACK awaited, with retries (`oh-my-claudecode/src/team/runtime-v2.ts#L74-L80`).
   - **Transport policy:** `split_pane`, `interactive`, `hook_preferred_with_fallback`, 15 s dispatch ACK timeout (`oh-my-claudecode/src/team/governance.ts#L12-L17`).
   - **Liveness by screen-scraping.** A pane "has an active task" if its last 40 lines show `esc to interrupt`, `background terminal running`, or a spinner glyph followed by a gerund. It "looks ready" on an idle prompt, with special cases for trust prompts (`oh-my-claudecode/src/team/tmux-session.ts#L3198-L3227`).
   - **Idle nudge.** Idle worker panes get tmux `send-keys` continuation nudges, never the leader (`oh-my-claudecode/src/team/idle-nudge.ts#L1-L12`).
   - **Resume.** `resumeTeamV2` re-reads config and checks the tmux session is alive (`oh-my-claudecode/src/team/runtime-v2.ts#L6513-L6522`). Task recovery requeues a task to a replacement worker from its last checkpoint, keyed by the claim token (`oh-my-claudecode/src/team/state/tasks.ts#L547-L603`, `oh-my-claudecode/src/team/state-paths.ts#L209-L214`).

**Hooks into Claude Code.** 30 hook commands on 11 events (`oh-my-claudecode/hooks/hooks.json#L4-L230`):
- **UserPromptSubmit:** keyword detector and skill injector.
- **SessionStart:** project memory, wiki, stale-run reporter, runs reconciler.
- **PreToolUse:** enforcer and git guardrails. **PermissionRequest(Bash):** permission handler.
- **PostToolUse:** verifier, project memory, rules and directory-context injectors. **PostToolUseFailure** is also handled.
- **SubagentStart/Stop:** tracker and `verify-deliverables`.
- **PreCompact:** notepad and project-memory handlers.
- **Stop:** context guard, workflow-drift guard, **persistent-mode**, **budget guard**, code simplifier.
- **SessionEnd:** session end and wiki.

**The persistence loop.** The Stop hook blocks the stop while a mode (ralph, autopilot, team, ultragoal) is active and re-prompts with `[RALPH - ITERATION n/max] The task is NOT complete yet. Continue working.` (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1537-L1549`).
- The soft `max_iterations` auto-extends by 10, "so user-visible cancellation remains the only explicit termination path".
- A hard maximum of 500 (200 in strict mode) auto-disables the mode (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1483-L1518`, `oh-my-claudecode/src/lib/security-config.ts#L31-L57`).
- The loop does not fight context-limit, user-abort, rate-limit or auth-error stops (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L62`, `#L1032`).

**Rate-limit wait.** A daemon scans tmux panes for sessions blocked by the 5-hour limit and sends a resume sequence when the limit resets (`oh-my-claudecode/src/features/rate-limit-wait/daemon.ts#L1-L13`).

**vs theuth.** theuth's engine owns checkout and epoch, and the sweep probes liveness (session or descendant working) before declaring a checkout orphaned and re-waking the owner (`theuth/crates/tasks/src/ledger.rs#L2120-L2131`). That is stronger than omc's model-written heartbeat JSON and pane scraping. What theuth lacks: (a) a per-claim **checkpoint** that a replacement can resume from, (b) **rate-limit park-and-resume** for subscription-backed harnesses, (c) a "keep going" **completion loop** for one session. theuth's schedule `until` loops cover only part of (c).

## A5 Coordination between agents

### Ruflo
- **Topologies are labels.** `swarm init --v3-mode` prints a 15-agent table, calls MCP `swarm_init {topology: 'hierarchical-mesh', maxAgents, strategy}`, then tells the user that this "sets up the topology" and that real execution needs the Task tool or `hive-mind spawn --claude` (`ruflo/v3/@claude-flow/cli/src/commands/swarm.ts#L1032-L1047`).
- **Spawning is a JSON write.** `agent_spawn` validates input, routes a model, writes the record to `.claude-flow/agents/store.json` and the swarm store, and returns `status: 'registered'` (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L343-L363`, `#L444-L450`).
- **The "hive mind" is one process told to orchestrate through MCP tools that only write state.** "Do NOT use Claude native Task/Agent tools for swarm coordination — use mcp__ruflo__agent_spawn, mcp__ruflo__task_assign" (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L163-L169`). Read with point 1, this tells the one Claude to avoid the only tool that would create a second agent context. Users report the result: "Hive-mind swarm returns without work being done" (#789, open) and "`hive-mind spawn` hangs … workers stay `idle`, tasks remain `0`" (#655, open).
- **"Consensus" (BFT, Raft, quorum)** is `calculateRequiredVotes`: ⌊2n/3⌋+1, ⌊n/2⌋+1 or n over the worker list in `hive-mind/state.json`. Votes are cast by whoever calls `hive-mind_consensus vote`, usually the same Queen model voting on behalf of named workers (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L83-L105`, `#L731-L760`). Raft keeps only a "one pending proposal per term" rule. There is no leader election, log or network.
- **The real protocol library is not wired in.** `v3/@claude-flow/swarm/src/consensus/{raft,byzantine,gossip}.ts` (583/550/624 lines) and `queen-coordinator.ts` (2,035 lines) exist, but `@claude-flow/cli` does not depend on `@claude-flow/swarm` (`ruflo/v3/@claude-flow/cli/package.json#L100-L143`). Outside the package, only `v3/index.ts` (a re-export) and `@claude-flow/testing` import it. The CLI holds just a string ownership map (`ruflo/v3/@claude-flow/cli/src/mcp-tools/capability-brain.ts#L669`). The README still sells "Queen-led hierarchy (Raft, Byzantine, Gossip)" (`ruflo/README.md#L397`).
- **What does coordinate for real:**
  - *Dual-mode workers* share a memory namespace. Each worker is told to run `npx ruflo@latest memory search … --namespace <ns>` before work and `memory store --key "<id>-result"` after it (`ruflo/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts#L290-L302`). This is a blackboard by CLI, again protocol by prompt.
  - *Claims* with work-stealing (A3).
  - The *repo supervisor election* among daemons (A7).

### oh-my-claudecode
- **File mailboxes with locks.** `mailbox/<worker>.json` plus `.lock-<worker>`, `dispatch/requests.json` plus lock, a notification lock per request, `worker-idle-notify.json` and `events.jsonl` (`oh-my-claudecode/src/team/state-paths.ts#L155-L179`).
  - Workers send to `leader-fixed` with `team api send-message`, list their mailbox and mark messages delivered (`oh-my-claudecode/src/team/worker-bootstrap.ts#L318-L322`).
  - The first act of any worker is exactly one startup ACK (`#L324-L326`).
  - Shutdown is a request/ACK handshake in which the worker may *reject* with "still working" (`#L328-L334`).
- **Locks and checkout.** Claim tokens, CAS versions and leases (A3). A per-team config mutation lock and a scaling lock (`oh-my-claudecode/src/team/state-paths.ts#L196-L199`).
- **Merge queue for parallel worktrees** (`oh-my-claudecode/src/team/merge-orchestrator.ts#L1-L21`):
  - watches each worker's branch HEAD;
  - auto-merges into the leader branch from a dedicated "merger" worktree, mutex-serialised;
  - on a clean merge, rebases every other worker;
  - persists last-seen SHAs for restart recovery;
  - refuses `main`/`master` as the leader branch;
  - on stop, drains pending merges and writes an audit row for any worker it could not merge.
- **Conflicts become inbox messages.** Paths from `git status --porcelain` are sanitised before they are put into markdown, because "could … be interpreted as additional instructions by the leader's LLM" (`oh-my-claudecode/src/team/conflict-mailbox.ts#L1-L25`).
- **Who decides who works next.** The leader pre-assigns owners; `allocateTasksToWorkers` allocates the rest (`oh-my-claudecode/src/team/runtime-v2.ts#L25-L26`). There is no voting.

**vs theuth.** theuth peers talk through `send_message` with receipts (design 50). Tasks have one owner and a checkout. theuth has nothing like omc's merge coordinator, and its orchestrators hand-merge worktree waves (see the wave procedures in knowledge). Consensus and voting: nobody in this slice has a real one, and theuth does not need one.

## A6 Human in the loop & governance

### Ruflo
- **Permissions.** `hive-mind spawn` adds `--dangerously-skip-permissions` only on an explicit flag ("HIGH-02: Strict boolean check"), and `--no-auto-permissions` denies it (`ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L284-L301`).
- **PreToolUse(Bash).** Matches a four-item string list of dangerous commands (`rm -rf /`, `format c:`, a fork bomb…) (`ruflo/.claude/helpers/hook-handler.cjs#L478-L486`). This is decoration rather than policy.
- **Console "cockpit"** (`plugins/ruflo-console`): a `/ruflo` command, pages and "a band above the prompt"; anything it cannot measure reads `n/a` (`ruflo/plugins/ruflo-console/README.md#L1-L3`). Its e2e driver runs a real headless Claude through console tools, asserts on the output, caps spend at $0.40 (`--max-budget-usd`) and disallows Bash/Write/Edit (`ruflo/plugins/ruflo-console/README.md#L7-L30`). This "drive the UI through its own model tools and grep the results" check is a neat CI trick.
- **Audit and security.** AIDefence (prompt-injection detection), `policy-tools.ts`, hive capability tokens (ADR-476). Not traced in depth. No approval inbox for long-running work: a human sees the terminal.

### oh-my-claudecode
- **PermissionRequest(Bash)** has its own handler. `git-guardrails` runs on PreToolUse(Bash) (`oh-my-claudecode/hooks/hooks.json#L73-L106`).
- **Remote approval gate** (graph runtime) (`oh-my-claudecode/src/graph/runtime/remote-approval.ts#L1-L21`):
  - a file exchange: `approvals/pending/<id>.json` written by the gate, `approvals/decisions/<id>.json` written by a decider;
  - fail-closed: malformed decisions never resolve, and an expired request takes the timeout policy (default *denied*);
  - "decision artifacts carry no trust": the runner journals the outcome itself.
- **Reply-to-continue from the phone** (`oh-my-claudecode/src/notifications/reply-listener.ts#L1-L15`):
  - a daemon polls Discord and Telegram, and listens on Slack Socket Mode, for replies to OMC's notifications;
  - it maps the reply to the right pane by message id, sanitises it twice (reply text, then tmux), checks the pane, and types the reply into the session;
  - only configured user or chat ids can inject, and replies are rate-limited.
- **Governance knobs:** `plan_approval_required`, `delegation_only` (A2). Strict security profile: path restrictions, a python sandbox, no project skills, no auto-update, no remote MCP, no external LLMs, hard max 200 (`oh-my-claudecode/src/lib/security-config.ts#L39-L57`).

**vs theuth.** theuth's approvals go through `tools.policy` and cards in the app or channel, the double answers as the owner (126), and there is an attention inbox. theuth is ahead on policy. Check one thing: whether a *reply in a Slack/Telegram/Signal thread to an attention notification* reaches the waiting session the way omc's listener does. If it does not, that is the cheap gap.

## A7 Cost, budgets & runaway control

### Ruflo
- **Global launch fuse** (#2661, a closed P0: "Worktree daemon fanout exhausts Claude quota: add global scheduler, leases, and cost fuse"):
  - every autonomous `claude --print` launch, from every daemon in every worktree, passes through `~/.claude-flow/ai-budget.json`, an `O_EXCL` lock and `ai-budget-receipts.jsonl`;
  - files are 0600 and symlinks are rejected; the check happens before the process is created;
  - defaults: `maxConcurrentGlobal 1`, `maxLaunchesPerHour 2`, `maxLaunchesPerDay 12`, `pauseOnQuotaErrorMinutes 60` (`ruflo/v3/@claude-flow/cli/src/services/global-ai-budget.ts#L1-L48`).
  - Sibling incidents: #2484 "Multiple daemon instances spawned per Claude Code session" and #2098 "daemon dispatch loop spawns Claude subprocess every 5s" (both closed).
- **One scheduler per repository.** Ten worktree daemons of one repo elect one supervisor (lock takeover on a dead PID or a heartbeat older than 3 min). The others run only their $0 local workers (`ruflo/v3/@claude-flow/cli/src/services/repo-supervisor.ts#L1-L40`).
- **Consent and lifetime.** AI workers are off by default. The daemon has a TTL and idle shutdown, and thresholds on CPU load and free memory (`ruflo/v3/@claude-flow/cli/src/services/worker-daemon.ts#L111-L135`).
- **Loop breaker.** The same command failing 3× in a row draws a warning, 5× a block. It is advisory by default: "An orchestration layer that doesn't own the UI should not hard-stop" (`ruflo/v3/@claude-flow/cli/src/mcp-tools/tool-loop-guardrail.ts#L1-L27`).
- **Cost tracker plugin: after the fact, from transcripts.**
  - A Stop hook auto-runs `cost-track` (`ruflo/plugins/ruflo-cost-tracker/hooks/hooks.json#L1-L15`). It reads the newest `~/.claude/projects/<cwd>/*.jsonl` (`ruflo/plugins/ruflo-cost-tracker/scripts/track.mjs#L2-L23`) and the Codex/Grok logs (`ruflo/plugins/ruflo-cost-tracker/scripts/_ledger.mjs#L20-L33`).
  - Budget alerts are a ladder: 50 INFO / 75 WARNING / 90 CRITICAL / 100 HARD_STOP. HARD_STOP is advice ("halt non-essential agent spawns") plus `exit 1` for CI (`ruflo/plugins/ruflo-cost-tracker/scripts/budget.mjs#L102-L115`, `#L201`). Nothing pauses a session.
- **Model routing for cost.** `hooks_route` returns a three-tier recommendation: Tier 1 is a deterministic codemod ($0, e.g. var→const), Tier 2 Haiku, Tier 3 Sonnet/Opus (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hooks-tools.ts#L1280-L1281`).

### oh-my-claudecode
- **Run budget, enforced at Stop.** `OMC_RUN_BUDGET_TOKENS` applies to unattended modes (ralph/autopilot/team/ultragoal). When spend crosses it, "the hook stops the stop": the model is sent back to write a resumable budget report.
  - Rollout is tri-state `off|shadow|active`, default **shadow**: log only.
  - Token spend is read from a bounded transcript tail. A missing transcript passes with a `degraded` log line: "this hook must never block on absent evidence" (`oh-my-claudecode/scripts/budget-guard.mjs#L1-L31`).
- **Iteration caps.** Soft `max_iterations` (+10 when reached) and a hard 500/200 (A4).
- **Quota visibility.** The HUD reads Anthropic's OAuth usage endpoint for 5-hour and 7-day utilisation, with the token from the macOS Keychain or `~/.claude/.credentials.json` (`oh-my-claudecode/src/hud/usage-api.ts#L1-L14`). The rate-limit daemon resumes blocked panes when the limit resets (A4).
- **Model tiers per agent** through the delegation enforcer (A1). An external-LLM kill switch: `disableExternalLLM` (`oh-my-claudecode/src/lib/security-config.ts#L35-L36`).

**vs theuth.** theuth budgets *pause the agent at the cap* and file an override task (`theuth/crates/roster/src/lib.rs#L1464-L1475`). That is stronger than either project's advisory ladder. Two gaps:
- **Launch-count and 429 fuses.** A subscription-backed harness (Claude Code over ACP, omp) burns *quota*, not dollars, so a dollar cap never fires. Ruflo's per-hour/per-day launch limits and its "pause 60 min after a quota error" breaker cover exactly that case.
- **Shadow mode** for any new guard.

## A8 Memory, knowledge & learning

### Ruflo
- **What is injected on every prompt.** `UserPromptSubmit → route` prints `[INTELLIGENCE] Relevant patterns for this task:` with the top 5 entries from `.claude-flow/data/ranked-context.json`, then the router's "Primary Recommendation" box (`ruflo/.claude/helpers/hook-handler.cjs#L380-L420`).
  - Score = 0.6 × trigram Jaccard + 0.4 × PageRank, threshold 0.05, top-K 5 (`ruflo/.claude/helpers/intelligence.cjs#L700-L719`).
  - Data files: `auto-memory-store.json`, `graph-state.json`, `ranked-context.json`, `pending-insights.jsonl` (`ruflo/.claude/helpers/intelligence.cjs#L8-L13`).
- **"Learning" is confidence nudging.**
  - Entries matched on the previous prompt but not this one get +0.03, as "implicit success: user continued working" (`ruflo/.claude/helpers/intelligence.cjs#L721-L733`).
  - SubagentStop gives +0.05 on success and −0.02 on failure to whatever was last matched (`ruflo/.claude/helpers/intelligence.cjs#L789-L795`, `ruflo/.claude/helpers/hook-handler.cjs#L717-L727`).
  - Credit assignment is "the last five things shown". Nothing checks that a pattern was *used*.
- **Router.** "Static keyword router … NOTE: This is *not* a learned model … confidence is reported as a heuristic prior". It returns 0.6 on a keyword hit and 0.3 for the default `coder` (`ruflo/.claude/helpers/router.cjs#L1-L14`, `#L72-L91`). The README and USERGUIDE still say "Intelligent routing (89% accuracy)" and "based on learned patterns", with no benchmark in the tree (`ruflo/README.md#L400`).
- **Embeddings.**
  - The CLI path is the AgentDB bridge first, then the local chain (`ruflo/v3/@claude-flow/cli/src/memory/memory-initializer.ts#L2523-L2543`).
  - The last resort is a hash embedding that the code itself labels `backend: 'mock'`: "these vectors do NOT carry real semantics" (`ruflo/v3/@claude-flow/cli/src/memory/memory-initializer.ts#L2609-L2617`).
  - The embeddings package's `auto` provider tries the RVF service *first* (`ruflo/v3/@claude-flow/embeddings/src/embedding-service.ts#L944-L955`). That service hashes the **whole text** with FNV-1a into a pseudo-random unit vector (`ruflo/v3/@claude-flow/embeddings/src/rvf-embedding-service.ts#L319-L349`), so two different texts never come out close.
  - Open issue #3113: "ReasoningBank embedding model registered as object, never used — every store falls back to hash embeddings".
- **Ruflo's own audit** (`ruflo/docs/reviews/intelligence-system-audit-2026-05-29.md#L6-L13`, `#L44`):
  - "Headline performance multipliers in `CLAUDE.md` are largely hardcoded doc strings … one is fabricated at runtime";
  - "Flash Attention 2.49–7.47×" is "fabricated with `Math.random()`";
  - HNSW measured 1.48× peak against a claimed 150×–12,500×;
  - mock embeddings were mislabelled `Xenova/all-MiniLM-L6-v2`, with synonyms scoring −0.988 and unrelated text +0.775;
  - but the confidence/Q-value loop "is real", in the sense that it persists and changes later routing.
- **Does it help?** Nothing measures task outcomes. The one multi-agent benchmark run in the tree is `"backend": "mock"`, 100 runs per task, finished 4 ms after it started (`ruflo/docs/benchmarks/multi-agent/multiagent-mock-2026-06-22T17-02-43-846Z.json#L1-L20`).
- **Distillation.** The daemon's `consolidate` worker turns `memory_entries` into episodes, reasoning patterns and causal edges every 30 min. It is incremental, non-destructive and quick_check-gated (`ruflo/v3/@claude-flow/cli/src/services/worker-daemon.ts#L9-L10`, `#L29-L34`).

### oh-my-claudecode
- **Notepad, three tiers** (`oh-my-claudecode/src/hooks/notepad/index.ts#L1-L23`):
  - Priority Context: always loaded, at most 500 characters;
  - Working Memory: pruned after 7 days;
  - MANUAL: never pruned.
  - It is meant to survive compaction (PreCompact hooks).
- **Project memory.** An auto-detected environment (deps, scripts, workspaces) that rescans overwrite, plus `customNotes`, `userDirectives` and `hotPaths` that survive rescans (`oh-my-claudecode/src/hooks/project-memory/index.ts#L17-L33`). Directives are regex-mined from messages ("always use …", "never touch …", "rule: …") (`oh-my-claudecode/src/hooks/project-memory/directive-detector.ts#L8-L25`).
- **Learner.** It "detects skill-worthy patterns" (problem/solution pairs scored 0–100) and suggests skill extraction (`oh-my-claudecode/src/hooks/learner/auto-learner.ts#L1-L6`). Learnings from Ralph progress can be promoted to full skills (`oh-my-claudecode/src/hooks/learner/promotion.ts#L1-L5`).
- **Wiki.** Session-start, pre-compact and session-end hooks run ingest, query and lint (`oh-my-claudecode/src/hooks/wiki/`). Not traced.
- **Judgement points ("Jev").** Hooks consult a remote classifier (`https://api.typesafe.ai/v1/systemone`) for questions such as "which skill trigger fires" (`oh-my-claudecode/src/hooks/jev/config.ts#L1-L34`, `oh-my-claudecode/src/hooks/jev/points.ts#L1-L14`).
  - Zero egress by default: points must be listed in `OMC_JEV`.
  - Every point starts in shadow and is promoted per point.
  - Points are either gate-type (blocking) or detector-type (background).
  - This is the same idea as theuth's local decision model (`decide`), run remotely and rolled out more carefully.
- **Does it help?** No outcome evals. Shadow logs are the only evidence collected (`oh-my-claudecode/src/hooks/jev/config.ts#L19-L25`).

**vs theuth.** theuth's knowledge base, nightly dreaming with a replay gate (136) and endless memory (137) are richer. theuth already has a gate, which neither project has. Things to borrow:
- omc's **always-loaded ≤ 500-char priority block** that survives compaction;
- **explicit directive capture** ("always/never …") into standing preferences;
- **per-recall logging** (Ruflo's `recall-log.jsonl` of ids and scores, never the prompt; `ruflo/.claude/helpers/intelligence.cjs#L43-L46`), so a later dream can score hints against outcomes.

## A9 Observability & UX

### Ruflo
- **Statusline** (1,328 lines). It shows git, model, context, cost from stdin, ADR count, AgentDB, tests, hooks and security (`ruflo/.claude/helpers/statusline.cjs#L210-L667`).
- It also renders a **server-served promo row**. The text is sanitised and capped at 100 characters, with a "manage: ruflo settings" tail (`ruflo/.claude/helpers/statusline.cjs#L1205-L1221`).
- When the user is rate-limited, the prompt hook prints `[COGNITUM] Hit your Claude usage limit? Free sponsored capacity is available at cognitum.one/meta-llm` (`ruflo/.claude/helpers/hook-handler.cjs#L426-L475`).
- **Console plugin** (in-session pages, "the Room": what is waiting for a yes). There is no web dashboard in the core path. `ruflo/src/chat-ui` is a separate chat product (`ruvocal`).
- **Session restore can show invented counts.** Without the session module it prints a fake "Restored State" table: Tasks 0 / Agents 0 / Memory 0, with `%SESSION_ID%` placeholders (`ruflo/.claude/helpers/hook-handler.cjs#L655-L671`).

### oh-my-claudecode
- **HUD.** 28 statusline elements (`oh-my-claudecode/src/hud/elements/`):
  - agents, background tasks, call counts, context and context warning, git, last tool, limits;
  - mission board, model, permission, PRD, prompt time, ralph iteration;
  - session summary, skills, thinking, todos, token usage, enterprise cost.
  - Fed by the OAuth usage API (`oh-my-claudecode/src/hud/usage-api.ts#L1-L14`).
- **Team views.** Monitor and summary snapshots, `events.jsonl`, `phase-state.json` (`oh-my-claudecode/src/team/state-paths.ts#L178-L194`), a worker health report that merges heartbeat, tmux, task history and the audit log (`oh-my-claudecode/src/team/worker-health.ts#L1-L30`), and tmux split panes as the live view.
- **Lifecycle notifications.** Discord/Telegram/Slack/webhook/CLI, with a template engine and redaction (`oh-my-claudecode/src/notifications/`).

**vs theuth.** theuth has a desktop app, web, TUI, phone, the HUD/attention inbox and the activity monitor (155), and is far ahead on surfaces. omc's **HUD elements** (ralph iteration, PRD progress, 5h/7d quota) are a good checklist for theuth's status surfaces, especially quota utilisation for subscription harnesses.

## A10 Extensibility & integration

### Ruflo
- **Inventory.** 47 plugins in `plugins/` (cost-tracker, console, swarm, rag-memory, neural-trader, music…). An MCP server with ~60 tool modules (`ruflo/v3/@claude-flow/cli/src/mcp-tools/`). Claude Code plugin marketplace metadata in `.claude-plugin/`. A Codex flavour (`@claude-flow/codex`, "dual-mode"). Agent federation (mTLS/ed25519). "Hermes" and other runtimes appear in the repo description.
- **Tool descriptions argue against the native tools.** Each MCP tool description explains when *not* to use the native one ("Use when native Task tool is wrong because…"), to steer the model towards Ruflo tools (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L281-L282`, `ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L731-L732`).

### oh-my-claudecode
- **Inventory.** A Claude Code plugin with 47 skills, 19 agents, a hooks.json, an MCP server (`src/mcp`), a CLI (`omc`), a `bridge/` and an "SDK-based integration path built for OpenClaw, Hermes, Grokbot" (`oh-my-claudecode/README.md#L15`). Tmux workers for six vendor CLIs (A1). Notification channels (A9).

**vs theuth.** theuth's MCP files, ACP harnesses, A2A at the boundary and three chat channels cover more. The lesson is distribution, not extensibility: both projects install as a Claude Code plugin in one command and show up as slash commands where the user already works.

## A11 Engineering, licence & traction

### Ruflo
- **Repo.** `ruvnet/ruflo`, MIT, TypeScript. **74,236 stars, 8,826 forks, 1,040 open issues/PRs** (GitHub MCP, 2026-10-10). Created 2025-06-02 (GitHub keeps the creation date across the claude-flow → ruflo rename), pushed 2026-10-10, HEAD merges PR #3957.
- **Size.** `v3/@claude-flow/cli` alone is 892 TS/JS files and ~262k TS lines. The other packages: guidance 42k, memory 35k, swarm 22k, plugins 49k. 898 test files, 32 workflows.
- **Business model.** `homepage: Cognitum.One`. A sponsored-capacity funnel and promo rows inside the user's Claude Code (A9). Satellite products (ruvector, agentdb, agentic-flow, metaharness) come in as optional dependencies (`ruflo/v3/@claude-flow/cli/package.json#L130-L143`).
- **Maturity.** Very high churn. Comments everywhere cite issue numbers (#1916, #2085, #2269, #2661, #2962…) that fixed wiring between stores. Docs and code drift. An internal audit admits fabricated numbers.

### oh-my-claudecode
- **Repo.** `Yeachan-Heo/oh-my-claudecode`, MIT. **39,757 stars, 3,559 forks, 11 open issues** (GitHub MCP, 2026-10-10). Created 2026-01-09, so roughly 40k stars in 9 months. Issue numbers reach ~#4261, so triage closes almost everything.
- **Size.** `src/` ~455k TS lines (wc over `src/*`, including `src/__tests__` 81k and per-module `__tests__`); `src/team` 207 files / 89k lines; `src/hooks` 334 files / 104k lines. 817 test files, 8 workflows. README in 12 languages.
- **Maturity.** Every module is guarded against a past incident: hardening flags M1–M6 in the merge orchestrator, launch-attempt fencing, owner epochs. But there are **two parallel implementations** of key hooks: TypeScript `src/hooks/*` and shipped `scripts/*.mjs`. Issue #4256: "Stop hook scripts don't enforce ralph hardMaxIterations: TS-only fixes from #2320/#2321/#2322/#2331 never reached scripts/persistent-mode.mjs". The current `.mjs` does enforce it (`oh-my-claudecode/scripts/persistent-mode.mjs#L1362-L1375`).
- **Why the stars (my read).** Install-in-one-line plugins for the most popular coding agent. Magic words (`ralph`, `ultrawork`, `autopilot`, `team`). A HUD people screenshot. Multilingual READMEs. Fast releases (v5.6.2). The issues show what people really use: the persistence loop and keyword triggers (#3411, #4111 false-triggers), tmux team launch (#4252, #4261), and PRD criteria (#3664).

## Map to the three questions

### Q1 What do they offer versus theuth?

| Axis | Ruflo | oh-my-claudecode | theuth | Gap / verdict |
|---|---|---|---|---|
| A1 Agent | Claude subagent `.md` (name, description) + JSON record | `.md` with model tier; six vendor-CLI worker contracts | roster file + desk session (67) | theuth ahead; omc's per-attempt launch id is a nice fence |
| A2 Org | Queen as prompt; hive token for remote callers | leader/worker + governance flags | reporting lines, escalation, reviewer, double | theuth ahead |
| A3 Work | JSON tasks; claims with handoff/steal | leased claim tokens, CAS, `depends_on`, evidence-gated done, PRD amendment ledger | board with `done_when`, checkout, blockers | **borrow evidence gate + amendment ledger** |
| A4 Runtime | Task tool / one `claude` / tool-less API call / daemon `claude -p`; Codex dual-mode | tmux CLI workers, pane scraping, idle nudges, Stop-hook loop, rate-limit resume | ACP/native sessions, mesh placement, sweep with liveness probe | theuth ahead; **borrow rate-limit park/resume and per-claim checkpoints** |
| A5 Coordination | labels + JSON; vote counting; shared-memory CLI blackboard | file mailboxes with locks; merge orchestrator; conflict inbox | `send_message` receipts, one owner per task | **borrow merge coordinator**; skip consensus |
| A6 HITL | skip-perms opt-in; 4-string Bash check; console | PermissionRequest hook; fail-closed remote approvals; phone reply-to-continue | `tools.policy`, cards, double, attention | theuth ahead; verify channel reply → session |
| A7 Cost | **global launch fuse**, repo supervisor, daemon consent/TTL; advisory $ ladder | token budget at Stop (shadow default); hard cap 500; HUD quota | per-agent budget pauses at cap | **borrow launch/429 fuse + shadow mode** |
| A8 Memory | trigram+PageRank top-5 injection; static router; hash embeddings; audit admits fakes | notepad tiers, project memory, regex directives, learner, remote judgement points | knowledge + dreaming with replay gate + endless | theuth ahead; borrow priority block + recall log |
| A9 UX | statusline with promo row; console pages | rich HUD (28 elements), notifications | app/web/TUI/phone/HUD/activity | theuth ahead; take the HUD checklist (quota, loop iteration) |
| A10 Ext | 47 plugins, ~60 MCP tool modules, marketplace | plugin, 47 skills, MCP, SDK bridge | MCP, ACP, A2A, channels | parity or ahead; their edge is *distribution* |
| A11 Traction | 74.2k★, 1,040 open, marketing-heavy | 39.8k★ in 9 months, 11 open | — | lesson: ship inside the tool users already run |

**What a meta-harness gains and loses by NOT living inside one harness.** This is the question both projects answer most clearly.

What theuth **gains**:
- **Real agents.** Each one is a separate session with its own model, tools, approval policy and budget. Ruflo's swarm tries to fake this inside one process. omc shows how much machinery it costs to rebuild it with tmux panes and files: a 6.5k-line runtime, a 4.2k-line tmux layer, launch-attempt fencing, owner epochs.
- **Protocol owned by the engine, not by the prompt.** omc workers must *remember* to write heartbeat JSON and call `team api` (`oh-my-claudecode/src/team/worker-bootstrap.ts#L305-L322`). Ruflo's Queen must *remember* to call MCP tools that only write JSON. theuth's checkout, epoch and sweep run whether or not the model cooperates (`theuth/crates/tasks/src/ledger.rs#L2120-L2131`).
- **Immunity to host churn.** Claude Code 2.1.178 removed `TeamCreate`/`TeamDelete` and omc had to rebuild `/team` on the "implicit team" (`oh-my-claudecode/skills/team/SKILL.md#L1-L11`).
- **Any harness on any machine.** omp, opencode, Claude Code over ACP, placed across a mesh. Durable state in SQLite, not `.omc/state` or `.claude-flow/` in the repo.
- **Approvals and budgets that bind**, rather than advice printed into the transcript.

What theuth **loses**:
- **Zero-friction adoption.** A one-line plugin install inside the tool developers already have open; slash commands and magic words where they type.
- **Per-tool-call hooks inside the host.** PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest, PreCompact and SubagentStart/Stop let both projects inject context, guard Bash and record outcomes on *every* call. theuth sees an ACP child's tool calls only as the adapter reports them.
- **The host's own UI.** Its statusline and its native subagent and agent-team features.
- **The user's subscription quota without an API key.** A one-harness plugin rides the logged-in CLI. theuth can do the same over ACP, but must then manage *quota*, not just dollars (Q3 #2).

### Q2 What can we learn / borrow? (techniques only)

1. **Evidence-gated completion.** A move to `completed` is refused with a specific error unless the result carries a "Summary + Verification" string. The worker prompt carries the recipe (`oh-my-claudecode/src/team/state/tasks.ts#L311-L318`, `oh-my-claudecode/src/team/worker-bootstrap.ts#L294-L298`). *Cost if wrong:* desks write boilerplate ("Verification: n/a"), and trivial tasks get friction. Mitigate by scoping it to tasks that have `done_when` and showing the evidence to the reviewer.
2. **An amendment ledger for acceptance criteria.** A criterion the work disproves is *replaced or superseded*, with the original kept verbatim plus reason, evidence, authority and timestamp. Completion claims bind to the current criteria revision (`oh-my-claudecode/src/hooks/ralph/prd.ts#L27-L30`, `oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1533-L1535`). *Cost if wrong:* agents amend criteria to make work pass. Require owner or reviewer authority for an amendment.
3. **A user-global launch fuse for unattended model launches.** One lock, ledger and receipts file shared by every runner on the machine. It counts concurrent launches and launches per hour and per day, and pauses N minutes after a quota/429 error. The check runs before the process is created (`ruflo/v3/@claude-flow/cli/src/services/global-ai-budget.ts#L1-L48`). *Cost if wrong:* starving legitimate routine fires when the limits are too low, or double-limiting alongside per-agent budgets. Make it count launches and quota errors only, never dollars.
4. **Elect one scheduler per resource.** Lock takeover on a dead PID or a stale heartbeat; non-elected runners do only $0 work (`ruflo/v3/@claude-flow/cli/src/services/repo-supervisor.ts#L1-L40`). *Cost if wrong:* low. theuth's mesh may already de-duplicate routine fires; check before building.
5. **Shadow, then active, for every new guard or judgement.** Each blocking rule ships tri-state `off|shadow|active`, defaults to shadow (logs the would-block decision) and is promoted per point. A missing input never blocks: "degraded" passes (`oh-my-claudecode/scripts/budget-guard.mjs#L1-L23`, `oh-my-claudecode/src/hooks/jev/config.ts#L19-L28`). *Cost if wrong:* protection arrives later and shadow logs go unread. Surface would-block counts where a person will see them.
6. **Leased claims with per-claim checkpoints.** A claim token plus `leased_until`. Workers write numbered checkpoints (`resume_payload` at safe boundaries) keyed by the claim token. Recovery requeues to a replacement worker, which adopts the latest checkpoint (`oh-my-claudecode/src/team/state/tasks.ts#L170-L223`, `#L547-L603`). *Cost if wrong:* poor checkpoints, and overlap with theuth's epoch plus re-wake. The value is only for long tasks that change hands.
7. **A merge coordinator for parallel worktrees.** It watches worker branch HEADs and merges from a dedicated merger worktree under a mutex into an integration branch. It rebases the siblings, persists SHAs for restart, and refuses `main`/`master`. Conflicts reach the leader and the worker as *sanitised* inbox messages, because file paths could carry instructions (`oh-my-claudecode/src/team/merge-orchestrator.ts#L1-L21`, `oh-my-claudecode/src/team/conflict-mailbox.ts#L1-L25`). *Cost if wrong:* a rebase under a running worker breaks its build and target dir. Rebase only when the worker is idle, and never touch main.
8. **Writer isolation at plan time.** When fanning out, refuse two concurrent *writers* in the same cwd and cap writers per batch; readers are unlimited (`ruflo/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts#L445-L476`). *Cost if wrong:* false refusals when a shared directory is intended. Allow an explicit override.
9. **Provider-aware idle and active detection for TUI workers.** Read the screen tail for spinner-plus-gerund, `esc to interrupt` or `background terminal running` (active) against an idle prompt, and nudge only idle panes (`oh-my-claudecode/src/team/tmux-session.ts#L3198-L3227`, `oh-my-claudecode/src/team/idle-nudge.ts#L1-L12`). *Cost if wrong:* breaks whenever a vendor changes its TUI. Use it only for PTY-hosted workers; ACP status is better.
10. **Reply-to-continue from chat channels.** A notification message id maps to a session. An authorised reply is sanitised and injected into that session, with rate limits (`oh-my-claudecode/src/notifications/reply-listener.ts#L1-L15`). *Cost if wrong:* an injection path from chat. Restrict it to owner ids.
11. **Park and resume on rate limits.** Detect subscription-quota exhaustion, park the session, and resume at reset with one line (`oh-my-claudecode/src/features/rate-limit-wait/daemon.ts#L1-L13`). The quota numbers come from the 5h/7d usage endpoint (`oh-my-claudecode/src/hud/usage-api.ts#L1-L14`). *Cost if wrong:* reading OAuth tokens from the Keychain is sensitive. Prefer the harness's own error and retry-after.
12. **A tiny always-loaded priority block.** At most 500 characters, re-injected after compaction, next to a 7-day pruned working memory and a never-pruned manual section (`oh-my-claudecode/src/hooks/notepad/index.ts#L1-L23`). *Cost if wrong:* low; it overlaps with standing preferences.
13. **A recall log for injected hints.** One bounded JSONL record per prompt of *which* entries were surfaced and their scores, never the prompt (`ruflo/.claude/helpers/intelligence.cjs#L43-L46`). It lets an offline pass score hints against outcomes. *Cost if wrong:* low.
14. **A "keep going until verified" loop with three exits.** Block the end of turn while an *explicit* goal is active and unverified. A soft cap extends, a hard cap disables, and the loop never fights a user abort, context limit or rate limit (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1483-L1518`). *Cost if wrong:* runaway spend, and keyword false-triggers like omc's (#3411, #4111). Arm it only from an explicit goal, never from words in a prompt.

**Do not borrow:**
- topologies, "hive mind" or consensus (prompt text over JSON);
- static keyword routers that report "confidence";
- magic-word mode triggers;
- model-written heartbeats;
- promo or funnel rows in the user's tool;
- hash "embeddings";
- benchmark numbers without a run file.

### Q3 How should theuth improve? (ranked by value ÷ effort)

1. **Refuse `done` without evidence on tasks with `done_when`.** S effort, high value. The refusal text is the recipe, as with the existing `NEEDS_BOARD_*` packet refusals. Lands in `crates/tasks`: the disposition/transition path, plus the desk manual (design 70 desk protocol).
2. **Launch and quota fuse for unattended fires.** S–M, high. Covers routine fires, `until` loops, desk wakes, autoresearch experiments and ACP/omp children. Caps concurrent launches and launches per hour and day, plus a quota-error breaker with receipts. Lands in the schedule runner and the roster's wake dispatch (`crates/roster`), next to the per-agent budget that pauses (`theuth/crates/roster/src/lib.rs#L1464-L1475`). Surface it in the activity monitor (155) and the attention inbox.
3. **Shadow mode as the default for new blocking rules.** S, medium-high. Applies to new `tools.policy` rules, write caps, sweep actions and double rungs (126): a `mode: shadow|active` key and "would have blocked" rows in the activity log.
4. **Plan-time writer isolation in `spawn_subagent`.** S, medium. Refuse a second live writing child in the same `cwd` and suggest a worktree. Lands in subagent spawn (`crates/agent`).
5. **Merge coordinator for worktree waves.** M–L effort, high value for Sky's wave workflow.
   - An integration branch and a merger worktree.
   - SHA watch on each worker branch; merge under a mutex; rebase siblings only when idle; persisted SHAs.
   - Conflict notes to the orchestrator's inbox, with paths sanitised.
   - Must obey the machine's build-lock and disk rules.
   - Lands as an engine service or a theuth skill plus runtime helper. It replaces hand-merging in the wave procedures.
6. **Rate-limit park and resume for subscription harnesses.** M, medium. The ACP adapter detects the quota error, parks the child, and sets an `at:` timer that resumes it with one line. Show 5h/7d quota on the HUD when the harness exposes it.
7. **Amendment ledger for `done_when`.** M, medium. Store a revision and an amendments list on the task (`crates/tasks`), shown on the board and in review packets.
8. **Recall log and priority block for knowledge hints.** S–M, medium. Log which hints were shown per turn (ids and scores) so dreaming (136) can measure their use. Optionally add a ≤500-char per-project priority block that survives compaction (137).
9. **Reply-to-continue across channels.** S *if* missing. Verify that a thread reply to an attention notification reaches the waiting session on Slack, Telegram and Signal. If it does not, add the message-id → session map.
10. **Per-claim checkpoints for long desk tasks.** M, low-medium. A progress note can carry a resume payload; on orphaning, the sweep hands the newest one to the re-woken desk (`theuth/crates/tasks/src/ledger.rs#L2120-L2131`).

## Their weaknesses and failure modes

### Ruflo
- **Bookkeeping presented as agents.**
  - `agent_spawn` and `swarm_init` write JSON (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L343-L363`).
  - "Consensus" counts votes (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L83-L105`).
  - The real protocol package is not a CLI dependency (`ruflo/v3/@claude-flow/cli/package.json#L100-L143`).
  - Open issues: "Hive-mind swarm returns without work being done" (#789) and "`hive-mind spawn` hangs … workers stay `idle`" (#655).
- **Split-brain stores.** The agent store and the hive store were separate files (#1916). `agent_spawn` and `swarm_status` read different stores "confirmed for all topologies" (#2085) (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L20-L24`, `#L365-L369`).
- **Unmeasured or fabricated numbers.** In the project's own audit (`ruflo/docs/reviews/intelligence-system-audit-2026-05-29.md#L6-L13`). The routing accuracy claim (`ruflo/README.md#L400`) sits against a static router (`ruflo/.claude/helpers/router.cjs#L1-L14`). The multi-agent benchmark is a mock run.
- **Embeddings that carry no semantics** on fallback paths (`ruflo/v3/@claude-flow/cli/src/memory/memory-initializer.ts#L2609-L2617`, `ruflo/v3/@claude-flow/embeddings/src/rvf-embedding-service.ts#L319-L349`; #3113 open).
- **Quota incidents** from daemon fan-out (#2661 P0, #2484, #2098), now fused. The fuse defaults are very low: two launches an hour.
- **Commerce inside the user's tool.** A server-served promo row in the statusline and a sponsored-capacity nudge on rate limit (`ruflo/.claude/helpers/statusline.cjs#L1205-L1221`, `ruflo/.claude/helpers/hook-handler.cjs#L426-L475`).
- **Silent failure by design.** Hook errors are swallowed (`ruflo/.claude/helpers/hook-handler.cjs#L738-L745`). The fallback session restore prints a table of zeros and placeholders (`#L655-L671`).
- **An "agent" execution path that cannot act.** `agent_execute` is a tool-less completion with 1,024 max tokens by default (`ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-execute-core.ts#L245-L262`).
- **Scale of the surface.** 1,040 open issues and PRs; 262k lines in one CLI package. Comments are dense with issue numbers for broken wiring.

### oh-my-claudecode
- **Protocol by prompt.** Liveness and lifecycle depend on the model writing status and heartbeat JSON and calling `omc team api` (`oh-my-claudecode/src/team/worker-bootstrap.ts#L305-L344`). The fallback is pane screen-scraping, tuned per vendor TUI (`oh-my-claudecode/src/team/tmux-session.ts#L3198-L3227`), which is brittle.
- **Two team runtimes with no shared result** (`/team` native vs `omc team` tmux) (`oh-my-claudecode/skills/team/SKILL.md#L1-L11`, `oh-my-claudecode/README.md#L148`).
- **Two implementations of key hooks** (TypeScript `src/` and shipped `scripts/*.mjs`) drift. #4256: hard-max fixes never reached the `.mjs` until October 2026.
- **Keyword false-triggers** start persistent loops (#3411, #4111). By design the soft cap extends forever until the user cancels (`oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1506-L1509`). The budget guard is shadow by default (`oh-my-claudecode/scripts/budget-guard.mjs#L8-L15`).
- **tmux/cmux coupling.** It fails outside tmux and inside cmux (#4252, #4261).
- **Host API churn.** Native `TeamCreate` was removed upstream and forced a redesign.
- **Remote judgement egress** (Jev to `api.typesafe.ai`) is opt-in, but it sends prompt excerpts to a third party when enabled (`oh-my-claudecode/src/hooks/jev/config.ts#L1-L34`).
- **Complexity.** `runtime-v2.ts` is 6,577 lines and `tmux-session.ts` 4,227 lines. Hardening flags (M1–M6) mark a history of incidents.

## Cites self-check

I re-opened 88 cited ranges at the end, in two batches of 50 and 38, checking each for a key string. 86 held at the cited range; two needed a corrected range. Separately, two ranges written in the first draft were re-checked against nearby lines and corrected before saving. The load-bearing ones:

| # | Cite | Checked for | Result |
|---|---|---|---|
| 1 | `ruflo/v3/@claude-flow/cli/src/commands/swarm.ts#L1046-L1047` | "Use Claude Code Task tool" | held |
| 2 | `ruflo/v3/@claude-flow/cli/src/commands/hive-mind.ts#L163-L169` | "Do NOT use Claude native Task/Agent tools" | held |
| 3 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L83-L105` | BFT ⌊2n/3⌋+1 | held |
| 4 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L444-L450` | "Three execution paths" | held |
| 5 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L467-L474` | "nothing dispatched work" | held |
| 6 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-execute-core.ts#L245-L262` | `max_tokens … 1024` | held |
| 7 | `ruflo/v3/@claude-flow/cli/package.json#L100-L143` | no `@claude-flow/swarm` in deps/optional | held |
| 8 | `ruflo/v3/@claude-flow/cli/src/services/global-ai-budget.ts#L43-L48` | `maxLaunchesPerHour: 2` | held |
| 9 | `ruflo/v3/@claude-flow/cli/src/services/repo-supervisor.ts#L35-L40` | stale 3 min | held |
| 10 | `ruflo/v3/@claude-flow/cli/src/services/worker-daemon.ts#L111-L135` | `aiWorkersEnabled` | held |
| 11 | `ruflo/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts#L445-L459` | "distinct worktrees" | held |
| 12 | `ruflo/.claude/helpers/router.cjs#L1-L14` | "*not* a learned model" | held |
| 13 | `ruflo/.claude/helpers/intelligence.cjs#L700-L733` | TOP_K 5 / implicit success | held |
| 14 | `ruflo/.claude/helpers/hook-handler.cjs#L426-L475` | sponsored capacity | held |
| 15 | `ruflo/v3/@claude-flow/cli/src/memory/memory-initializer.ts#L2609-L2617` | "do NOT carry real semantics" | held |
| 16 | `ruflo/v3/@claude-flow/embeddings/src/rvf-embedding-service.ts#L319-L349` | FNV-1a full-text hash | held |
| 17 | `ruflo/docs/reviews/intelligence-system-audit-2026-05-29.md#L6-L13` | `Math.random()` | held |
| 18 | `ruflo/docs/benchmarks/multi-agent/multiagent-mock-2026-06-22T17-02-43-846Z.json#L1-L20` | `"backend": "mock"` | held |
| 19 | `ruflo/README.md#L397`, `#L400` | Raft/Byzantine/Gossip; 89 % | held |
| 20 | `oh-my-claudecode/src/team/state/tasks.ts#L311-L318` | `missing_delegation_compliance_evidence` | held |
| 21 | `oh-my-claudecode/src/team/state/tasks.ts#L170-L223` | 15-min lease | held |
| 22 | `oh-my-claudecode/src/team/worker-bootstrap.ts#L336-L344` | "You are NOT the leader" | held |
| 23 | `oh-my-claudecode/src/team/tmux-session.ts#L3198-L3227` | "esc to interrupt" | held |
| 24 | `oh-my-claudecode/src/team/merge-orchestrator.ts#L1-L21` | refuse main/master | held |
| 25 | `oh-my-claudecode/src/hooks/persistent-mode/index.ts#L1506-L1509` | `max_iterations += 10` | held |
| 26 | `oh-my-claudecode/src/lib/security-config.ts#L39-L57` | hard max 500/200 | held |
| 27 | `oh-my-claudecode/scripts/budget-guard.mjs#L1-L23` | "stops the stop" | held |
| 28 | `oh-my-claudecode/src/graph/runtime/remote-approval.ts#L1-L21` | default denied | held |
| 29 | `oh-my-claudecode/skills/team/SKILL.md#L1-L11` | TeamCreate removed | **corrected** (was #L1-L10; the sentence is on L11) |
| 30 | `oh-my-claudecode/src/hooks/jev/config.ts#L1-L34` | endpoint constant | **corrected** (was #L1-L33; the endpoint is on L34) |
| 31 | `ruflo/.claude/agents/core/coder.md#L10-L19` | "Authoritative project documents" | **corrected** in the draft (was #L8-L18) |
| 32 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L762-L774` | `authorizeHive` on propose | **corrected** in the draft (was #L765-L774) |
| 33 | `ruflo/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L365-L369` | `swarm_status` wiring comment (#2085) | held |
| 34 | `theuth/crates/tasks/src/ledger.rs#L1405-L1424` | "conflict: checked out by" | held |
| 35 | `theuth/crates/tasks/src/ledger.rs#L2120-L2131` | `LivenessProbe` | held |
| 36 | `theuth/crates/roster/src/lib.rs#L1464-L1475` | paused at cap | held |

**Not verified:**
- the federation, AIDefence and wiki internals (README or directory level only);
- whether any Ruflo plugin, rather than the CLI, imports `@claude-flow/swarm` at runtime. The search excluded `node_modules` and `dist`; only `v3/index.ts` and `testing` import it;
- user-reported frequency beyond the issue titles found by semantic search;
- omc's total issue count (open count 11, highest number seen ~#4261).
