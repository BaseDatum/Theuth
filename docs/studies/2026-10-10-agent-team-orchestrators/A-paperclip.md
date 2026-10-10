# Paperclip at HEAD (eedf25655, 2026-10-10) and what changed since theuth copied it (7b7c4d417, 2026-09-23)

Read-only recon of `/Users/Sky.Moore/Develop/spike-agent-teams/paperclip`. Nothing was built, installed, run or edited. Cites are
`paperclip/<path>#L<a>-L<b>` at HEAD. Commit hashes are short SHAs from `git log 7b7c4d417..HEAD`. **[I]** marks my inference.
Facts already in the 2026-09-24 recon (`~/.theuth/scratch/paperclip-recon.md`) are not re-derived unless the code changed.

## Executive summary
- **What it is:** an MIT, TypeScript control plane that runs "AI-agent companies". It holds the agents, an org chart, issues, budgets and
  approvals, and wakes existing harnesses (Claude Code, Codex, Cursor, OpenCode, Pi, Hermes, OpenClaw…) in finite heartbeats. Traction:
  99.5k stars and 16.8k forks. There are 2,925 open issues **plus 3,403 open PRs**, so the "6.3k open issues" figure is issues and PRs together.
- **The 17 days since the copy:** 414 commits (201 `fix`, 61 `feat`), 1,884 files added and 1,834 modified, and the tree grew from 7,722 to 9,587 files.
  Most of it is hardening plus three product lines: **external agents and assistants joining over MCP** (OpenAI Dot, assistant
  invitations, MCP Events), **per-agent durable state** (Ed25519 identity, persistent agent files, lifecycle module, private tasks), and
  **governed connections** (memory providers, aggregators, reviewed connection instructions).
- **Idea 1 for theuth: lease-based assignment of external agents.** The Dot protocol uses mailbox references, an explicit accept within 10 minutes,
  a 2 h execution lease renewable up to a 24 h cap, request-ID idempotency, and "fence" notices whose acknowledgement records receipt only.
- **Idea 2 for theuth: fail-closed accounting.** New work is refused while finished runs await accounting or while usage has no reliable price.
  Side-model calls reserve budget before dispatch and settle idempotently.
- **Idea 3 for theuth: measured prompt diet.** They cut hiring templates, default instructions and planning skills to short role text, and
  qualified each cut with paid end-to-end evals. Their bug notes also show a wake-dedup key collision class that theuth's copied wake queue
  should be audited for.
- **The NAT gap is not closed for execution.** runnerd is still launched per run by the server (ADR status: Proposed, flag off by default). What is new is that a
  NAT'd machine's *assistant* can dial out to the public MCP endpoint as its human. It cannot dial out as a worker that takes runs.
- **Memory moved from ⚪ to 🟡.** It consists of external MCP providers (Mem0/Zep/Supermemory/Cognee/Honcho) behind a flag, plus a per-agent file directory.
  There is still no first-class memory, and organisational learning is ⚪.
- **Verdict:** keep copying Paperclip's *protocol hygiene* (leases, receipts, fail-closed gates, measured prompts). Do not copy its weight.
  theuth is ahead on placement (mesh), memory (knowledge + dreaming) and the desktop. Paperclip is ahead on external-agent onboarding,
  privacy ACLs and cost accounting rigour.

## What changed since 7b7c4d417

### Shape of the delta (all numbers from git, read-only)
- **Commits:** 414 in `7b7c4d417..HEAD`, dated 2026-09-24 → 2026-10-10. By conventional type: 201 fix, 61 feat, 59 unprefixed, 27 build,
  18 test, 15 refactor, 14 chore, 14 docs, 3 perf, 2 ci. The busiest scopes are ui 47, runner 28, deps 26, connections 20, server 13 and heartbeat 13.
- **Files:** `git diff --name-status --no-renames` gives 1,884 A, 1,834 M and 20 D. The whole tree went from 7,722 to 9,587 files.
- **Correction to the brief: `packages/paperclip-runner` is not new.** It had 857 files at 7b7c4d417 and has 1,195 now (+337 A / 260 M). The old recon
  already cited its README. `packages/teams-catalog` (12 M, 0 A), `packages/plugins` (28 M, 9 A), `doc/connections` (24 A, 14 M) and
  `tests/runner-e2e` (231 A, 72 M) also existed before. The genuinely new trees are `integrations/assistant-plugins` (17 A),
  `tests/lifecycle-baseline` (12 A), `feature-map/` (37 A) and 54 new plan files under `doc/plans/`.
- **The god file was split.** `server/src/services/heartbeat.ts` went from about 29.5k lines in the old recon to 8,142 lines now. A new
  `server/src/services/heartbeat/` directory holds 12 modules (queue, recovery, retries, run-completion, run-control, run-lifecycle,
  run-log, run-preparation, run-state, scheduling, task-markdown, workspaces), each with a test file. The refactor ran from commit 75918997a
  (2026-10-08) to 65965f0d3 (2026-10-09), plus c27ee928b, 2a5f65c9a, 87a7312cf and 51653becc.

### Runner (28 `runner` commits, f1a394bd3 2026-09-28 → be1bc5d49 2026-10-09)
- **What it is.** It is still an experimental adapter, `paperclip_runner`, behind "one default-off instance flag". Its qualified catalog is Codex, OpenCode, Claude Managed, AWS
  AgentCore and pinned ACPX profiles (`paperclip/doc/architecture/paperclip-runner.md#L20-L22`). The ADR status is still "Proposed"
  (`paperclip/doc/architecture/paperclip-runner.md#L3`).
- **Additions since the old recon:**
  - rich ACP transport and a durable interaction foundation (24beb0057);
  - Grok Build over native ACP (f1a394bd3);
  - Cursor production integration (a6306ba60);
  - Pi 1.0 (57e977be7 plus the 5-part doc series 2eb5187e4…73eee2c0d);
  - the OpenAI Dot provider (fc6304dfe);
  - restored task monitors and durable timed waits (8cfedd7df);
  - continuation of restart-interrupted Codex turns (63f3aa2db);
  - configurable listening ports (0fe47882c).
- **How it connects: unchanged.** The server opens a native run and launches a verified runnerd artifact in the realized execution environment. runnerd then
  opens the outbound PRP WebSocket at `/api/runner/v1/connect/:runId` (`paperclip/doc/architecture/paperclip-runner.md#L73-L76`,
  `paperclip/doc/architecture/paperclip-runner.md#L115-L119`).
- **Defaults are staged, not landed.** The plan is that new agents use Runner "where the harness and execution target are qualified" while existing agents keep their recorded
  runner (`paperclip/doc/plans/2026-10-06-default-agent-runner.md#L7-L10`). The current slice explicitly leaves "Agent defaults, saved
  runner choices, UI, schema, and provider qualification" unchanged (`paperclip/doc/plans/2026-10-06-default-agent-runner.md#L32-L39`).
- **NAT verdict: not fixed for execution.** A NAT'd machine still cannot volunteer as a worker. runnerd is per-run and server-launched, and I found no worker registry,
  capacity advertisement or pull-based claim. `rg -i 'register|daemon|tunnel|NAT|inbound|listen'` over the ADR and the runner README hits only
  release "platform daemons" and "persistent provider state". What changed is that two *other* principals can now dial in:
  - **Assistant connections.** A human's Claude Code, Codex, OpenCode or Claude web connector can dial in over OAuth, browser or device flow, through a stdio bridge,
    "as the consenting human". This is explicitly "No client-credentials or agent identity"
    (`paperclip/doc/plans/2026-10-05-assistant-invitations.md#L7-L10`, `paperclip/doc/plans/2026-10-05-assistant-invitations.md#L20-L23`).
  - **OpenAI Dot.** This is a cloud agent that Paperclip wakes with MCP Events. It *requires* Paperclip to have "a stable public HTTPS origin"
    (`paperclip/doc/openai-dot-runner.md#L9-L12`), so it moves the reachability requirement onto the server rather than removing it.
  - **[I]** For theuth this confirms the mesh-of-engines design as the differentiator. Paperclip still has no "my laptop joins the company and
    takes runs" path.

### External agents and assistants (Dot: fc6304dfe 10-07 → 556daf9a9 10-09; assistants: e34abee67, 22a3ea341, 2d0c13812, 99a9de994)
- **Dot pairing.** A one-use pairing code (15-minute expiry) plus OAuth binds a Dot to an agent record. The binding then has no inactivity expiry, and access tokens last 15 minutes
  (`paperclip/doc/openai-dot-runner.md#L26-L35`, `paperclip/doc/openai-dot-runner.md#L49-L54`).
- **Dot assignment protocol.** Events carry mailbox references. Dot drains after its saved cursor and explicitly accepts before executing tools. A webhook 2xx
  "does not mean Dot accepted". Every operation carries a UUID request ID: a pending result is reconciled or retried "with the same ID and
  arguments", and "an unknown write must never be retried under a new ID" (`paperclip/doc/openai-dot-runner.md#L120-L132`). Completion
  echoes the canonical accepted report. Work requests get a durable admission receipt keyed by binding, generation and request ID, and a Dot runs one
  assignment at a time (`paperclip/doc/openai-dot-runner.md#L134-L147`).
- **Dot leases and fences.** Acceptance must happen within 10 minutes. Execution starts with a 2 h lease, renewable up to a 24 h total, and 15 minutes idle shows "requiring attention"
  (`paperclip/doc/openai-dot-runner.md#L156-L158`). The code agrees: `acceptByUnixMs: Date.now() + 10 * 60_000, expiresAtUnixMs: … 2 * 60 * 60_000`
  (`paperclip/server/src/services/dot-runner-broker.ts#L224`), and renewal is bounded to the 2 h rolling and 24 h total limits
  (`paperclip/server/src/services/dot-runner-broker.ts#L258-L260`). Cancel, pause, reassign and revoke "fence Paperclip authority" but "do not
  confirm that Dot stopped". A fence ack records receipt only (`paperclip/doc/openai-dot-runner.md#L162-L164`). Recovery never re-executes a
  write whose outcome was not recorded (`paperclip/doc/openai-dot-runner.md#L165-L169`).
- **Public MCP with MCP Events.** The same endpoint speaks MCP 2.0 (`2026-07-28`) with `server/discover`, `events/list` and `events/subscribe`. It offers three
  task events: status_changed, comment_created and document_updated (`paperclip/doc/public-mcp.md#L318-L332`). The code agrees at
  `paperclip/server/src/routes/public-mcp.ts#L159-L166`. Delivery uses Standard Webhooks HMAC with DNS pinning and no redirects. Bodies carry
  references, not text, so "clients must read current authorized state before responding" (`paperclip/doc/public-mcp.md#L349-L354`).
  Subscriptions last at most 24 h (`paperclip/doc/public-mcp.md#L356-L358`), and verification concurrency is capped per grant, company and instance
  (`paperclip/doc/public-mcp.md#L341-L347`).
- **Assistant plugins** (`integrations/assistant-plugins/`, new). One shared set of workflow skills (`delegate-work`, `review-my-team`,
  `follow-up-results`) is built into an OpenAI (ChatGPT/Codex) package and a Claude package against one MCP endpoint, with "Agent impersonation … not
  included" (`paperclip/integrations/assistant-plugins/README.md#L3-L5`). Scopes are `paperclip:read paperclip:write offline_access`
  (`paperclip/integrations/assistant-plugins/README.md#L27-L31`).
- **Testing real clients exposed a bug class worth knowing.** Codex 0.153.4 needed RFC 8252 loopback-port matching. PID-file refresh locks were replaced by
  crash-released SQLite OS locks. The paid invitation matrix passed 15/15 (`paperclip/doc/plans/2026-10-05-assistant-invitations.md#L70-L74`).

### Agents: identity, lifecycle, files, primary, permissions, commentary
- **Cryptographic identity** (a590ab769, 2026-10-06). Each agent gets one persistent Ed25519 keypair, created in the agent-creation transaction
  (`paperclip/doc/AGENT-IDENTITY.md#L3-L6`). The private key is encrypted with the instance master key (`paperclip/packages/db/src/schema/agent_identity_keys.ts#L5-L11`).
  A wrong key fails the run and is never auto-replaced (`paperclip/doc/AGENT-IDENTITY.md#L24-L27`). Picking a runtime trusts its host with the key, and
  HTTP/gateway agents get no private key in v1 (`paperclip/doc/AGENT-IDENTITY.md#L50-L57`).
- **Lifecycle module** (2dd9811e8, 2026-10-09; plugins adopt it in 5badea479). One module owns hire, pause, resume and terminate, and each command owns its
  transaction (`paperclip/doc/AGENT-LIFECYCLE.md#L3-L7`). The new states are pending_approval → preparing → verifying → ready, then pausing → paused → resuming,
  and terminating → cleaning_up → terminated, plus rejected. "An old result cannot complete a newer operation" (`paperclip/doc/AGENT-LIFECYCLE.md#L38-L54`).
  The motivation in the commit: "A hire can become active before its execution environment is ready."
- **Persistent agent files** (3ca196b0a, 2026-09-29). Each agent has one current directory; `AGENTS.md` is one file in it, and the agent may keep notes or memory there.
  Peer access needs target-scoped configuration permission (`paperclip/doc/agent-files.md#L3-L12`). Per run:
  - the directory is restored into a private copy with a hash baseline;
  - at the end, under the agent lock, only files changed against that baseline are applied, and the last writer wins on the same file
    (`paperclip/doc/agent-files.md#L114-L134`).
  - These are per-file checkpoints, "not an atomic snapshot" (`paperclip/doc/agent-files.md#L136-L139`).
  - The earlier revision-history design was dropped (`paperclip/doc/agent-instruction-revisions.md#L3-L5`).
- **Personal primary agent** (ae6f95ed7, 2026-10-07). Each person has one crowned agent per company. It is used as the task-assignee fallback after explicit assignments and a recent
  eligible assignee, and as the default Chat entry. It does not change roles or permissions (`paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L21-L32`).
- **Permission defaults** (a1ab55a56). New standard agents get configuration access by default. Agent-authenticated callers cannot switch to the process adapter or
  change host command/cwd/env settings (`paperclip/doc/agent-permission-defaults.md#L21-L25`).
- **Complaints and suggestions** (b508a05c4, 2026-10-06). Agents get `complain` and `suggestion-box` skills that write attributed rows to
  `agent_commentary` (`paperclip/doc/agent-commentary.md#L3-L12`). A unique `(company, run, idempotencyKey)` index de-duplicates them
  (`paperclip/packages/db/src/schema/agent_commentary.ts#L9-L21`). There is "no feedback UI, read API, notification, automatic task creation"
  (`paperclip/doc/agent-commentary.md#L70-L73`).

### Prompt diet (cross-cutting: b17019e14 10-03, 862a5758b 10-02, 16b7db35f 10-06, a386a5999 10-05)
- **Hiring templates.** These were reduced "to short role descriptions" because they "can crowd out the task and the harness instructions". The CEO's fixed delegation route was
  removed (commit 862a5758b body; mirrored in `paperclip/packages/teams-catalog/MIGRATION.md#L13-L15`).
- **Default manual and shared prompts.** These were reduced, and real default-hire E2E coverage was added (b17019e14).
- **Planning skills.** These were shortened. "The runtime skill favors few tasks, but the catalog skill requires a child-task breakdown". A bounded Product E2E
  run measures saved outcomes and handoffs (16b7db35f).
- **The method.** `doc/plans/2026-10-08-wait-resume-ownership.md` maps each wait/resume operation to the code that owns it and decides
  which instruction is still needed (`paperclip/doc/plans/2026-10-08-wait-resume-ownership.md#L12-L19`). **[I]** This is the technique: move rules
  out of prompts once code enforces them, and prove it with an eval.

### Work model: private tasks, follow-ups, delegation (b67db12d9 / 3a726e676 / b31558064 2026-10-07; 32745f4e8 2026-10-10)
- **Private tasks and projects.** Work is "company-open by default", and a task can be made private (`paperclip/doc/ISSUE-PRIVACY.md#L3-L15`). The schema has
  `visibility text default 'open'` with a check constraint allowing `open|private` (`paperclip/packages/db/src/schema/issues.ts#L41`, `paperclip/packages/db/src/schema/issues.ts#L98`).
  Run authority is "the intersection of the agent and the run's responsible user". Non-members see only timing/status/token/cost metadata
  (`paperclip/doc/ISSUE-PRIVACY.md#L61-L66`). The docs state the residual risk honestly: an agent that read private work may write it into shared memory
  (`paperclip/doc/ISSUE-PRIVACY.md#L73-L79`).
- **Follow-up parents.** Follow-ups now keep their parents and show the creation source (32745f4e8).
- **Delegation handoff bug.** The parent-notification dedup key was "the parent/child pair", so "a consumed notification can therefore suppress a later
  completion of the same child". The fix scopes the key to the durable completion decision (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`).
  **[I]** This is directly relevant to theuth's copied coalesced wake queue.

### Budgets and costs (799e4d556 10-06, a7a244ab3 10-07, 98e3cec78 10-09)
- **Fail-closed admission.** `observedBlocks` trips on spend ≥ amount, *or* on pending (unsettled) runs, *or* on unpriced usage unless
  `unpricedUsagePolicy` is `allow` (`paperclip/server/src/services/budgets.ts#L224-L227`). Admission then says "cannot start work while completed runs
  await accounting" or "recorded usage has no reliable price" (`paperclip/server/src/services/budgets.ts#L572-L577`). Policies now carry
  `reservationCents`, `unpricedEventCount` and `pendingRunCount` (`paperclip/server/src/services/budgets.ts#L388-L391`). **[I]** The file's commit log
  suggests 799e4d556 ("make accounting durable") introduced this. I did not diff the blobs.
- **Decision models.** These are company-level, typed side-model calls with 1–20 boolean/choice questions, ≤ 32 KB and 30 s (`paperclip/doc/decision-models.md#L3-L7`,
  `paperclip/doc/decision-models.md#L37`). Each is audited in `decision_invocations` with no content stored (`paperclip/doc/decision-models.md#L41`). It reserves budget under the company
  accounting lock, keeps the hold on uncertain dispatches, and settles idempotently (`paperclip/doc/decision-models.md#L45`).
- **Subscriptions.** Monthly subscriptions are reported beside API spend, and subscription billing types are attributed to a subscription id (`paperclip/server/src/services/costs.ts#L17`).

### Memory, skills, connections
- **Memory.** It is 🟡 now, up from ⚪: "Experimental memory connections support Mem0, Zep, Supermemory, Cognee, and Honcho. The optional LLM Wiki
  plugin…" (`paperclip/ROADMAP.md#L117-L119`). These are "ordinary company-scoped tool connections. There is no separate memory page, automatic
  conversation upload, or background memory sync" (`paperclip/doc/connections/MEMORY.md#L3-L15`). Added in 18dac1e1e (2026-09-24).
- **Skills.**
  - GitHub-synced skill sources (d432dc7fa, new `SkillSources` page).
  - An agent `update_skill` tool with a version check and safe retry, under company policy and audit (eb049aebf).
  - Shorter planning skills (16b7db35f).
- **Connections** (20 commits).
  - Reviewed connection instructions: a connector declares `agentInstructions {id, version, text}` (≤ 2,000 chars) that reach only authorized agents' prompts.
    "Remote MCP initialization prose is never adopted as trusted instructions automatically"
    (`paperclip/doc/connections/CONNECTION-INSTRUCTIONS.md#L3-L18`).
  - AI connection routers, plugin-owned and default-off (`paperclip/doc/connections/AI-CONNECTION-ROUTERS.md#L3-L12`).
  - Composio/Arcade aggregators, verified MCP provider batches, usage-limit probes (7d59de611), and task-pinned AI routing (0e0b63e5a).

### Plugins and teams-catalog
- **Plugins.**
  - Lifecycle delivery and a resource baseline (1cebdd4c4).
  - Draining idle workers before automatic sleep (3d1d5294d, `packages/plugins/sdk/src/idle-drain.ts`).
  - The agent lifecycle contract used for plugin setup and status (5badea479).
  - The host–worker protocol is unchanged in shape (§A10).
- **teams-catalog.** The only change is 862a5758b (templates reduced to role descriptions). The catalog holds three bundled teams (`company-defaults/core-exec-team`,
  `product/product-design`, `software-development/product-engineering`) and one optional (`content`). A team is a folder: `TEAM.md`,
  `agents/<role>/AGENTS.md` and `projects/<p>/PROJECT.md`, with a seed `tasks/first-heartbeat/TASK.md`.

### Chat channels, UI
- **Slack.** Connect conversations and scheduled messages (b0155a681), automatic bot setup (d66acb7ac), and a require-@-mention setting (3fc64c515).
- **Voice.** An experimental Speko runtime (3ec7309d9). AgentMail became a default connection (6c1a75da4).
- **UI** (47 `ui` commits):
  - Chat leads the left nav, with agent work beside chats and a Combined Inbox + Task List flag (1c07b5903).
  - HTML artifacts render in a sandbox (1640c5b6a), CSV previews (083073703), and rich artifact cards with editable stories (24c58e479).
  - The primary-agent crown (ae6f95ed7).
  - New pages: AgentChats, McpConnect, SkillSources, apps/AssistantConnection and the Composio/Arcade/aggregator managers. `Costs.production.tsx` was deleted.
  - Storybook grew by 113 stories. Plans say "The user approved the revised Storybook design" before implementation
    (`paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L5-L7`).

## A1 Agent model & identity
**Still true from the old recon:** an agent is a DB row with one fixed `adapterType` and an opaque `adapterConfig`, a `reportsTo` self-FK, a fixed role enum and budget columns.
Instructions are a file bundle located through `adapterConfig`.

**New since 7b7c4d417:**
- **The agent home directory is now durable state.** The instructions bundle generalised into one persistent directory per agent, synced per run with hash baselines
  (`paperclip/doc/agent-files.md#L3-L12`, `paperclip/doc/agent-files.md#L114-L134`).
- **An identity independent of bearer auth.** Each agent has an Ed25519 key delivered as `PAPERCLIP_AGENT_KEY_ID/PUBLIC_KEY/PRIVATE_KEY` to managed runs.
  The session compatibility check includes the key id, so a warm process with an old key is replaced (`paperclip/doc/AGENT-IDENTITY.md#L29-L48`).
- **Lifecycle states beyond "status".** `preparing` and `verifying` sit between approval and `ready`. The host "tests the saved harness configuration" before the agent may work, and
  termination has a durable `cleaning_up` step for plugin resources (`paperclip/doc/AGENT-LIFECYCLE.md#L38-L54`).
- **External principals as agents.** OpenAI Dot is an agent record whose execution is an external cloud assistant reached through events
  (`paperclip/doc/openai-dot-runner.md#L3-L7`). It brings no provider usage or cost; billing is "external and unmetered" (`paperclip/doc/openai-dot-runner.md#L21-L23`).
- **Templates.** Teams are folder packages (TEAM.md, agents/*/AGENTS.md, projects, a seed task). Since 862a5758b the AGENTS.md files are short role descriptions, and
  procedures come from the harness and skills (`paperclip/packages/teams-catalog/MIGRATION.md#L13-L15`).
- **Multi-harness per agent is still not possible.** The roadmap claims "Multi-Model & Multi-Harness Teams" as ✅ (`paperclip/ROADMAP.md#L113-L115`), but that means per *agent*. The old
  weakness that a different harness per task means a different agent still holds. I found no per-issue adapter-type override.

**theuth contrast.** A theuth agent is a markdown file with model/effort/tools/mcp, so it is lighter and diffable. theuth has no lifecycle states between hire and
active, and no per-agent durable directory: desks keep memory in the session and the knowledge base.

## A2 Organisation & authority
**Unchanged:** a strict `reportsTo` tree with company-wide visibility *by default*, and scoped assignment grants.

**New:**
- **The first real isolation boundary** inside a company: private tasks and projects.
  - Readers are the responsible user, creator, assignee, task grantees and private-project members. Access flows down to children and handoffs.
    Sharing or assigning grants that task plus its descendants, not ancestors or siblings (`paperclip/doc/ISSUE-PRIVACY.md#L7-L15`).
  - Run authority = agent ∩ responsible user, so "reusing an agent does not transfer a different user's private-task permission"
    (`paperclip/doc/ISSUE-PRIVACY.md#L61-L66`).
  - Audited break-glass access exists (heading at `paperclip/doc/ISSUE-PRIVACY.md#L35`).
- **Principal grants.** The schema adds `principal_permission_grants`, `issue_access_grants` and `run_identity_contexts` (`packages/db/src/schema/`).
- **The human's assistant as a principal.** An assistant connection acts with the human's grants, and consent can drop write scope
  (`paperclip/integrations/assistant-plugins/README.md#L27-L31`).
- **Per-person defaults.** Each person has a primary agent per company. It does not touch roles or reporting lines (`paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L21-L25`).

**Still not built:** multi-manager, and agents proposing structural changes ("Self-Organization ⚪", `paperclip/ROADMAP.md#L129-L131`).

**theuth contrast.** theuth's `reports_to` and escalate-up-the-line are equivalent. theuth has knowledge `visibility` (private/team/shared) but no per-task ACL.
That matters only once theuth boards are shared between people.

## A3 Work model
**Unchanged:** issues with single assignee, statuses backlog/todo/in_progress/in_review/done/blocked/cancelled, blockers, work modes, goals
(company/team/agent/task levels) with goal fallback, and interactions (suggest_tasks, ask_user_questions, confirmations).

**New:**
- **Private visibility** on issues and projects (`paperclip/packages/db/src/schema/issues.ts#L41`).
- **Follow-up parent retention** (32745f4e8). Prompt-only tasks get agent-chosen names (018993140).
- **Completion feedback to parents** keyed by the completion decision (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`).
- **Planning guidance** reduced to ownership and dependency rules, "removes the required matrix and repeated checklist" (16b7db35f), measured by a decomposition eval.
- **Still ⚪:** queues and external trackers. "Work Queues ⚪" (`paperclip/ROADMAP.md#L125-L127`) and "Bring-your-own-ticket-system ⚪", where "Asana, Linear, and Jira can act as familiar
  on-ramps" (`paperclip/ROADMAP.md#L151-L153`). Asana exists only as a connection doc (`doc/connections/ASANA.md`), not as a queue.
- **Goals.** No goal-model commits since the copy. A grep of commit subjects for "goal" found none. The model stays a data relation (old recon).

**theuth contrast.** theuth's board with `done_when`, reviewer, dispositions and escalate is at parity. theuth lacks goals/projects as first-class parents, beyond the
`project` field, and both systems lack external-tracker queues.

## A4 Execution & runtime
**Heartbeat core (unchanged semantics, new structure).** Runs are finite: "Paperclip does not keep an agent process alive between turns"
(`paperclip/doc/architecture/durable-continuation-scheduler.md#L3-L6`).
- **Continuation has two mechanisms.**
  - Explicit continuation effects are written by native status arbitration as idempotent `agent_wakeup_requests` rows.
  - A stranded-issue reconciliation runs at startup and on every scheduler tick
    (`paperclip/doc/architecture/durable-continuation-scheduler.md#L8-L13`, `paperclip/doc/architecture/durable-continuation-scheduler.md#L36-L38`).
  - The tick defaults to 30,000 ms, clamped to ≥ 10,000 ms (`paperclip/doc/architecture/durable-continuation-scheduler.md#L20-L23`).
- **The god file is decomposed** into 12 modules under `server/src/services/heartbeat/`, and heartbeat.ts is down to 8,142 lines (see the delta).
- **The no-progress throttle is unchanged:** base 120 s, max 30 min (`paperclip/server/src/services/issue-rewake-throttle.ts#L31-L34`).

**Waits and resumes now have an ownership map.**
- `request_human_input` persists an interaction with a mutation receipt, and the status arbiter selects `in_review`.
- Answers commit with their delivery rows in one transaction, and `question-response-delivery.ts` owns claims, replay fences and continuation wake receipts.
- A provider's built-in question resumes the original paused request before a fresh-wake fallback
  (`paperclip/doc/plans/2026-10-08-wait-resume-ownership.md#L12-L19`).
- **[I]** This is the first synchronous-ish human-in-the-loop path; the old weakness #6 said there was none. It applies only to native runner providers.

**Runner and placement.**
- **Unchanged topology:** server-launched runnerd, outbound PRP, and a persist-before-send outbox (`paperclip/doc/architecture/paperclip-runner.md#L73-L76`).
- **More providers:** Grok, Cursor, Pi and Dot.
- **Restart-surviving behaviour hardened:** "continue restart-interrupted Codex turns" (63f3aa2db), "preserve tool outcomes through shutdown and
  restart" (94e8dec56), "require current-turn completion after connection continuations" (b5342febe).
- **Placement is still a static default.** Managed sandboxes are used for cloud Dot assignments (556daf9a9), and Daytona/exe.dev VM copy is new (bb73f2fe3).
- **The Dot workspace bridge.** It serialises file writes with hash checks, requires bubblewrap for commands with "no unrestricted fallback", and
  disables commands on macOS (`paperclip/doc/openai-dot-runner.md#L161`).

**Instance sleep (new).**
- Idle sleep safety is an instance-wide drain report for admins (`paperclip/doc/idle-sleep-safety.md#L3-L6`).
- Plugins drain idle workers before automatic sleep (3d1d5294d).
- A scheduler inventory (`doc/plans/2026-10-08-instance-sleep-and-scheduler-inventory.md`) lists what must be idle before a cloud instance may sleep.

**The reliability tail.** About 30 unprefixed commits are "retain bounded … diagnostics" or "keep proven … out of Sentry" (for example 128c95837, 3b47a6bef, 65b558180). **[I]** Recovery
remains whack-a-mole, and they invest in evidence capture rather than redesign.

**theuth contrast.**
- theuth is ahead on placement: any engine in the mesh, remote nodes and sandbox VMs. It is also ahead on long-lived desks.
- Paperclip is ahead on idempotency discipline at the external boundary: receipts, leases and fences.

## A5 Coordination between agents
**Unchanged:** coordination happens in the issue thread (comments, @mentions wake), single-assignee checkout with a 409 on conflict, blockers that auto-resume
dependants, a cross-issue write cap of 20, and the courier pattern.

**New:**
- **Delegation completion wakes are keyed by decision, not by pair.** See the delta (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`). Exact
  `task_watchdog` children keep a stable wake key "to avoid repeated watchdog loops" (same file, L20-L22).
- **Completed handoffs return to Agent Chat** (83076d7e7). "fix(native): wake parents after revised child completions" (fe9d86525).
- **Agent files are coordinated at file granularity.** The last sync of the same file wins under the agent lock, and unrelated new files survive
  (`paperclip/doc/agent-files.md#L125-L129`).
- **The Dot side is serial by construction.** "A Dot runs one assignment at a time; competing tasks stay queued regardless of the agent's configured concurrency"
  (`paperclip/doc/openai-dot-runner.md#L145-L147`). Mailbox writers and cursor reads serialise through the binding row, so a cursor cannot skip a result that
  commits later (`paperclip/doc/openai-dot-runner.md#L149-L150`).

**Still not built:** consensus, voting or swarm topologies. "MAXIMIZER MODE ⚪" (`paperclip/ROADMAP.md#L121-L123`) is the stated direction for more
aggressive delegation.

**theuth contrast.** theuth has direct peer messaging (`send_message`, A2A) that Paperclip lacks. Paperclip agents talk only through tasks. Paperclip's
mailbox-cursor serialisation is a technique theuth's peer intake could use for exactly-once tool results.

## A6 Human in the loop & governance
**Unchanged:** typed approvals (hire_agent, approve_ceo_strategy, budget_override_required, request_board_approval) with `revision_requested`,
interactions with resolver policies, and the board's unrestricted powers.

**New or changed:**
- **Hires gain post-approval gates.** `pending_approval → preparing → verifying → ready`, so approval no longer means "can work"
  (`paperclip/doc/AGENT-LIFECYCLE.md#L38-L44`).
- **Consent screens for every external principal.** A Dot pairing preview shows "the exact company, agent, permissions, and ongoing revocable access duration;
  previewing consumes no code and creates no grant" (`paperclip/doc/openai-dot-runner.md#L30-L33`). Assistant invitations: "Sharing an
  invitation grants no access" (`paperclip/doc/plans/2026-10-05-assistant-invitations.md#L7-L10`).
- **Durable questions.** Answers are delivered with replay fences, historical answers on completed tasks do not enqueue work, and "Clarification is not approval"
  (`paperclip/doc/plans/2026-10-08-wait-resume-ownership.md#L16-L18`). "stop repeated reminders for historical questions" (faa8e452c).
- **Background charges are explicit.** "Allow company-sponsored background decisions defaults on. Saving authorizes internal background features to
  charge the selected connection without a responsible human" (`paperclip/doc/decision-models.md#L5`). **[I]** This is a governance smell: on by default.
- **Privacy break-glass is audited** (heading at `paperclip/doc/ISSUE-PRIVACY.md#L35`). Non-members keep budget oversight through metadata only.
- **Agent feedback is captured but not surfaced.** Complaints and suggestions are stored with no UI (`paperclip/doc/agent-commentary.md#L70-L73`).
- **Chat channels.** Slack gained a require-at-mention setting (3fc64c515). Telegram/WhatsApp to the CEO is still a top request (#2004, 19 👍).

**theuth contrast.**
- theuth is ahead on synchronous approvals: tools.policy cards plus the double (126) answering as the owner, and the attention inbox/HUD.
- Paperclip is ahead on consent UX for external principals and on durable question delivery semantics.

## A7 Cost, budgets & runaway control
**Unchanged:** `budget_policies` (scope × window × warn% × hard stop), pausing and cancelling at the cap, an override approval, the rewake throttle and the cross-issue write cap.

**New:**
- **Fail-closed accounting.** See the delta (`paperclip/server/src/services/budgets.ts#L224-L227`, `paperclip/server/src/services/budgets.ts#L572-L577`).
  "Completed runs must finish accounting before resuming" also guards a budget resume (`budgets.ts` line 877).
  **[I]** This partly answers the old recon's weakness #8 ("costs are post-hoc"). A long run can still overshoot inside itself, but the *next* run cannot start on stale numbers.
- **Reservations for side-model calls** under a company accounting lock. Uncertain dispatch keeps the hold. "Startup/periodic accounting recovery marks stale
  invocations interrupted without resubmitting them" (`paperclip/doc/decision-models.md#L45`). Unknown prices are "never displayed as a free request"
  (`paperclip/doc/decision-models.md#L47`).
- **Subscription spend** is shown beside API spend and attributed to the subscription (98e3cec78; `paperclip/server/src/services/costs.ts#L17`). This answers top
  issue #339 ("Estimate cost for subscription-billed agents", 14 👍) at least partly.
- **On-demand usage-limit probes** for provider connections (7d59de611), and model capacity errors surfaced with automatic retry (f2715e02b).
- **Gaps.** Dot is "external and unmetered" (`paperclip/doc/openai-dot-runner.md#L21-L23`), with "No … provider usage or provider cost"
  (`paperclip/doc/openai-dot-runner.md#L159`). Budgets cannot see a Dot.

**theuth contrast.** theuth has budget auto-pause (copied) and a spend ledger. It lacks the unpriced-usage gate, settle-before-next-run, and reservations for its own
background model calls (dreaming, compactor, decide).

## A8 Memory, knowledge & learning
- **Memory:** 🟡. External MCP memory providers sit behind a flag as ordinary governed connections, with no memory page or background sync
  (`paperclip/doc/connections/MEMORY.md#L3-L15`). The LLM Wiki plugin (`packages/plugins/plugin-llm-wiki/`) offers a knowledge workflow. "A broader memory
  and knowledge surface … remains a direction for future work" (`paperclip/ROADMAP.md#L117-L119`).
- **Per-agent files:** a durable home directory that agents may use for "notes, memory, and other personal working material"
  (`paperclip/doc/agent-files.md#L3-L7`). There is no revision history by design (`paperclip/doc/agent-instruction-revisions.md#L3-L5`).
- **Context engineering:**
  - reviewed connection instructions (≤ 2,000 chars, versioned, authorized agents only, MCP init prose never trusted:
    `paperclip/doc/connections/CONNECTION-INSTRUCTIONS.md#L10-L18`);
  - the prompt diet (above);
  - "Measure native tool context and expand bounded workflow evals" (72ff3a9f2);
  - "Reduce repeated native completion guidance" (a386a5999).
- **Skills:** GitHub-synced sources, plus an agent `update_skill` with a version check (eb049aebf). This is how an agent can "learn" into a shared skill under policy and history.
- **Learning from outcomes:** still not built. "Automatic Organizational Learning ⚪ … capturing playbooks, recurring fixes, and decision patterns"
  (`paperclip/ROADMAP.md#L133-L135`). The nearest pieces are agent complaints/suggestions stored for operators (`paperclip/doc/agent-commentary.md#L3-L5`) and the
  `decision_training_examples` table (schema list). I did not trace what writes that table, so its use is unverified.
- **Evals:** the eval kernel package exists from before. New is a culture of paid, source-pinned qualification campaigns per change, with failures retained:
  - the invitations matrix, 15/15 then 14/15 (`paperclip/doc/plans/2026-10-05-assistant-invitations.md#L70-L83`);
  - hiring accounting replays (`doc/plans/2026-10-02-hiring-executable-accounting-replay.md`);
  - a stock-harness live comparison (`doc/plans/2026-10-02-stock-harness-live-comparison.md`, not read in depth).
- **Top user ask:** "Company-level knowledge layer (wiki, role context, shared strategy docs)", issue #1858, 26 reactions.

**theuth contrast.** theuth is clearly ahead: knowledge base with visibility, nightly dreaming (136) with replay gate and undo, learned skill sidecars.
Paperclip's useful contributions are the *input* channels (agent suggestion box) and the *trust rule* for injected instructions.

## A9 Observability & UX
- **Navigation re-centred on chat.** "Chat leads the left nav, agent work beside chats, and a Combined Inbox + Task List flag" (1c07b5903).
  New pages: `AgentChats.tsx`, `McpConnect.tsx`, `SkillSources.tsx`, and `apps/*` managers for aggregators, Composio, Arcade, Browser Use and the Assistant Connection
  (`git diff --name-status`, ui/src/pages).
- **Artifacts.**
  - HTML artifact previews render "in a secure sandbox" (1640c5b6a; `doc/html-artifact-previews.md`).
  - CSV table previews in task file tabs (083073703).
  - Rich artifact cards and editable stories (24c58e479).
- **Agent roster.**
  - The primary-agent crown, with the primary first in the Agents sidebar.
  - Star and leave buttons removed from the agent index (ac8f3eb14).
  - The icon rail retired (`paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L45-L64`).
- **Signals.**
  - Dot connection panel: 15 minutes without useful activity is shown as "requiring attention" (`paperclip/doc/openai-dot-runner.md#L156-L158`).
  - PR review waits made visible (228f0e280).
  - Private runs hidden from non-members but costed (`paperclip/doc/ISSUE-PRIVACY.md#L64-L66`).
- **Storybook-first review.** There are 113 new stories plus prototypes and fixtures. Features are approved by the user in Storybook with simulated state, then built
  (`paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L5-L17`).
- **Still ⚪:** desktop app (`paperclip/ROADMAP.md#L147-L149`). There is no TUI, and there is no mobile app beyond responsive web.

**theuth contrast.** theuth is ahead on surfaces: desktop, TUI, web, phone, Slack/Signal/Telegram, HUD and attention inbox. Paperclip's sandboxed HTML artifacts match
theuth's `show` pages. Its Storybook-approval loop matches theuth's "mock it as a page" rule.

## A10 Extensibility & integration
- **The plugin system** (✅ in the roadmap, "thin core and rich edges").
  - The host–worker protocol has `initialize, health, validateConfig, configChanged, onEvent, runJob, handleWebhook, getData,
    performAction, executeTool` (`paperclip/doc/plugins/PLUGIN_SPEC.md#L514-L643`, headings §13.1–13.10). There is a capability model with forbidden
    capabilities (§15, L790–L895), events (§16, L896), scheduled jobs (§17, L973), webhooks (§18, L986) and a UI extension SDK (§19, L1002).
  - New: lifecycle delivery (1cebdd4c4), idle drain (`packages/plugins/sdk/src/idle-drain.ts`), and the agent lifecycle contract for plugin setup/status (5badea479).
- **Plugin caveats** (the docs admit the gap, so trust these). UI bundles run "as same-origin JavaScript … Treat plugin UI as trusted code".
  "Manifest capabilities currently gate worker-side host RPC calls. They do not prevent plugin UI code from calling ordinary Paperclip HTTP
  APIs". Plugins are not cloud-ready (`paperclip/doc/plugins/PLUGIN_SPEC.md#L11-L35`).
- **Sandbox providers as plugins.** The `sandbox-providers` dir lists E2B, Cloudflare, Daytona, Modal, Novita and Kubernetes (`paperclip/ROADMAP.md#L53-L55`).
- **Public MCP server.** One endpoint (`/mcp/paperclip`) offers browser OAuth (CIMD + DCR + PKCE), device OAuth, a stdio bridge, an expanded direct tool catalog
  (2d0c13812) and MCP Events (`paperclip/doc/public-mcp.md#L318-L332`).
- **Assistant plugins.** The same workflow skills are packaged for Claude and OpenAI clients against that MCP (`paperclip/integrations/assistant-plugins/README.md#L22-L25`).
- **Connections.** There is a curated app catalog with reviewed `agentInstructions`, Composio/Arcade aggregators, AI routers (a plugin capability `ai.connections.route`,
  `paperclip/doc/connections/AI-CONNECTION-ROUTERS.md#L3-L6`), a governed MCP tool gateway, and verified MCP qualification batches with brand provenance JSON.
- **Chat and other channels:** Slack, AgentMail (email), GitHub App review bot, iMessage (Photon plan), voice (Speko, experimental).
- **Adapters:** 15+ built-in adapters, the external adapter plugin registration from before, and new pins (GPT-6.1 Sol 6c36c07a4, Sonnet 5.5 cab4263dc).

**theuth contrast.**
- theuth's MCP-as-files with risk gates, A2A, ACP harnesses and skills repos are comparable in reach.
- Paperclip is ahead in *being* an MCP server other assistants can join, with events.
- theuth is ahead in breadth of native channels (Signal, Telegram) and in desktop/native-app automation.

## A11 Engineering, licence & traction
- **Stack:** TypeScript/Node 24 server and React UI, Postgres via Drizzle (the schema list at HEAD includes new identity, decision, grant and
  lifecycle tables), pnpm workspace, and a Rust core inside `packages/paperclip-runner` (`rust-toolchain.toml`). Docker images and an npm CLI.
- **Size:** 9,587 tracked files. heartbeat.ts is 8,142 lines plus 12 extracted modules. `server/src/services` gained 279 files and `server/src/__tests__` gained 116 new test files in 17 days.
- **Licence:** MIT (GitHub API). There is a separate private `paperclip-cloud` repo ("Cloud routing PR: …/paperclip-cloud/pull/672",
  `paperclip/doc/plans/2026-10-05-assistant-invitations.md#L60-L62`). **[I]** The business model is open core plus a hosted cloud.
- **Traction** (GitHub MCP, 2026-10-10):
  - 99,462 stars, 16,782 forks; created 2026-03-02, so seven months old.
  - `open_issues_count` 6,328 = **2,925 open issues + 3,403 open PRs**; 128 open issues are labelled `bug`.
  - 626 issues opened since 2026-09-23, about 37 a day.
- **Release cadence:** calendar versions roughly weekly. v2026.1005.0 (10-06), v2026.1001.0 (10-02), v2026.916.1 (09-21), v2026.916.0 (09-16),
  v2026.831.0/.1 (09-02), v2026.824.0/.1 (08-25), v2026.817.0 (08-18). Master moves about 24 squash-merged PRs a day.
- **Top-reacted open issues:**
  - Ollama/local LLMs: #187, 79 reactions, 34 comments; #2979, 18.
  - Chat with agents: #49, 49.
  - GitHub Copilot CLI adapter: #2092, 44.
  - OpenRouter: #1068, 35.
  - Antigravity CLI: #6559, 27.
  - Company-level knowledge layer: #1858, 26.
  - Telegram/WhatsApp to the CEO: #2004, 19.
  - Z.AI adapter: #1130, 16.
  - SSO/OIDC: #3028, 16.
  - Slack chat: #7, 16.
  - Replace an agent with a human: #188, 15.
  - Subscription cost estimate: #339, 14.
  - "MacOS silicon not supported": #24, 12.
- **Most-discussed open bugs:**
  - #14093 "All Claude Code agents fail with 'ACP agent reported a terminal access failure'" (16 comments, 2026-09-26).
  - #7613 GitHub sign-in opens the repo page.
  - #10684 workspace sandbox fails on merged-/usr systems.
  - #7601 "Secret leak on UI".
  - #7790 "execution_workspaces never auto-close — stale active lifecycle leak".
  - #7661 "Gate successful_run_missing_state recovery on wakeNotBefore + per-issue cooldown".
- **How the project is built:** plans are agent-authored on `codex/*` branches with "the user approved/requested" checkpoints, frozen bases, paid-eval ceilings
  ("The original $250 ceiling", `paperclip/doc/plans/2026-10-06-default-agent-runner.md#L86-L91`) and "Co-Authored-By: Paperclip" trailers (a590ab769).
  Local rules: "Use Node 24 and one worker for focused local checks. Keep heavy builds in hosted CI. Do not build Docker images or Rust locally"
  (same cite). **[I]** Paperclip is largely built by its own agents. The 3.4k open PRs and the 201 fixes in 17 days are the visible cost.

## Map to the three questions

### Q1 What do they offer versus theuth?
- **A1 Agent model.** Theirs: DB row, fixed adapter, durable home dir, Ed25519 identity, lifecycle states, folder team templates. Ours: agent file + desk session.
  **Gap:** no `verifying`/`cleaning_up` states, no per-agent durable files, no team template packages (theuth has the hiring wizard only).
- **A2 Organisation.** Theirs: tree, company-open by default, private tasks/projects, run authority = agent ∩ user. Ours: tree, single user, knowledge visibility.
  **Gap:** per-task ACL, needed only for multi-human boards.
- **A3 Work model.** Theirs: issues + goals + interactions, private visibility, decision-keyed parent wakes. Ours: board with done_when, reviewer, dispositions.
  **Gap:** goals/projects as parents. Parity otherwise; neither has external-tracker queues.
- **A4 Execution.** Theirs: server-launched finite heartbeats, a hardened runner, leases for external agents, 30 s continuation reconciliation. Ours: mesh
  engines, long-lived desks, remote nodes, sandbox VMs. **theuth ahead** on placement and NAT. **Gap:** the receipt/lease/fence discipline at
  the external-harness boundary.
- **A5 Coordination.** Theirs: issue threads only, mailbox cursors for Dot. Ours: send_message/A2A plus task threads. **theuth ahead.** Borrow the
  cursor-serialisation and decision-keyed dedup.
- **A6 HITL.** Theirs: typed approvals, durable questions with replay fences, consent previews for external principals. Ours: policy cards, the double,
  attention inbox. **theuth ahead** on synchronous approval. **Gap:** consent UX for external principals joining.
- **A7 Cost.** Theirs: fail-closed accounting, reservations, unpriced = blocked, subscriptions. Ours: auto-pause and ledger. **Gap:** unpriced gate,
  settle-before-next, side-call reservations.
- **A8 Memory.** Theirs: external providers + agent files, organisational learning ⚪. Ours: knowledge + dreaming + learned skills. **theuth ahead.**
  Borrow: the suggestion-box input and the reviewed-instructions trust rule.
- **A9 UX.** Theirs: web only, chat-first nav, Storybook approval, sandboxed HTML artifacts. Ours: desktop/TUI/web/phone/HUD. **theuth ahead.**
- **A10 Extensibility.** Theirs: plugin RPC protocol, public MCP + Events, assistant plugins, curated connections. Ours: MCP files, A2A, ACP, skills.
  **Gap:** theuth as an MCP *server* that outside assistants can join and watch.
- **A11 Traction.** Theirs: 99.5k stars, weekly releases, 6.3k open issues+PRs. Ours: private. Not comparable; their issue tracker is a free
  requirements source (local models, chat, knowledge layer).

### Q2 What can we learn / borrow?
Techniques only. Items theuth took on 2026-09-24/25 are excluded.
1. **Lease-based assignment for external agents.**
   - Mechanism: mailbox reference → drain after cursor → explicit accept within 10 min → 2 h lease renewable to a 24 h cap → completion echoes
     the canonical report. Every write carries a request ID, and "an unknown write must never be retried under a new ID". Pause, cancel and revoke send fences, and an ack means
     receipt only.
   - Cites: `paperclip/doc/openai-dot-runner.md#L120-L169`, `paperclip/server/src/services/dot-runner-broker.ts#L224`, `paperclip/server/src/services/dot-runner-broker.ts#L258-L260`.
   - Cost of being wrong: medium. Extra state per assignment; a too-short lease kills slow honest agents.
2. **Decision-keyed wake and notification dedup.** Key "parent was told" on the completion decision id, not on the (parent, child) pair, so a consumed
   notice cannot swallow a later completion (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`). Exception: watchdog-style children keep a
   stable key to avoid loops. Cost of being wrong: tiny. It is an audit plus a key change.
3. **Fail-closed accounting admission.** Refuse new work while finished runs await accounting or while usage has no reliable price. `unpricedUsagePolicy` defaults to
   blocking (`paperclip/server/src/services/budgets.ts#L224-L227`, `paperclip/server/src/services/budgets.ts#L572-L577`). Cost of being wrong: false stops when a price
   table lags a new model. Mitigate with an explicit allow.
4. **Reservations for background model calls.** Hold the largest applicable reservation under an accounting lock, keep the hold on uncertain dispatch, settle
   idempotently, and recover stale holds without resubmitting (`paperclip/doc/decision-models.md#L45`). Cost: low. It is bookkeeping on calls theuth already makes.
5. **Measured prompt diet.** Reduce role templates, the default manual and planning skills to short role descriptions plus the rules code does not enforce. Qualify
   each cut with a paid E2E eval, and keep a per-wait "who owns this rule" table (`paperclip/doc/plans/2026-10-08-wait-resume-ownership.md#L12-L19`; commits
   862a5758b, b17019e14, 16b7db35f). Cost of being wrong: behaviour regressions if a removed rule was load-bearing. The eval is the guard.
6. **Lifecycle states with verification and cleanup.** `preparing → verifying → ready` means the host tests the saved harness config before first work, and
   `terminating → cleaning_up` frees resources durably. "An old result cannot complete a newer operation" (`paperclip/doc/AGENT-LIFECYCLE.md#L38-L54`).
   Cost: low.
7. **A trust rule for injected instructions.** Only reviewed, versioned, ≤ 2,000-char connection instructions reach authorized agents' prompts. Remote MCP
   initialization prose is never trusted automatically (`paperclip/doc/connections/CONNECTION-INSTRUCTIONS.md#L3-L18`). Cost: low. Some useful server hints
   get ignored until reviewed.
8. **An agent suggestion box.** Agents file attributed complaints and suggestions mid-run, de-duplicated by (run, key) and redacted. The store is separate from telemetry
   (`paperclip/doc/agent-commentary.md#L3-L12`, `paperclip/packages/db/src/schema/agent_commentary.ts#L9-L21`). Paperclip never reads it automatically;
   theuth's dreaming could. Cost: tiny, plus noise.
9. **MCP Events for task watching.** Expose task status, comment and document events with Standard Webhooks HMAC, DNS pinning, reference-only bodies, bounded
   subscriptions and verification quotas (`paperclip/doc/public-mcp.md#L318-L358`). Cost: medium. It adds an internet-facing surface.
10. **One workflow pack, two assistant ecosystems.** Shared delegate, review and follow-up skills are built into Claude and OpenAI plugin manifests against one
    OAuth MCP endpoint, with device flow plus stdio bridge for headless clients (`paperclip/integrations/assistant-plugins/README.md#L3-L31`,
    `paperclip/doc/plans/2026-10-05-assistant-invitations.md#L20-L23`). Cost: medium. It needs theuth to expose an MCP server.
11. **A per-agent durable home dir with baseline sync.** Restore into a private copy with hashes, then apply only changed files under the agent lock; last writer wins on the same file,
    with no revision history (`paperclip/doc/agent-files.md#L114-L134`). Cost: medium. theuth's knowledge base may make it redundant.
12. **Run authority as an intersection.** Agent ∩ responsible user, so reusing an agent never transfers another person's access. Non-members see cost metadata only
    (`paperclip/doc/ISSUE-PRIVACY.md#L61-L66`). Cost: medium. It is only worth it when boards are multi-human.
13. **Consent previews that cost nothing.** Show the exact company, agent, permissions and duration before a code is redeemed. A preview creates no grant
    (`paperclip/doc/openai-dot-runner.md#L30-L33`). Cost: tiny.

### Q3 How should theuth improve?
Ranked by value ÷ effort.
1. **Audit the coalesced wake queue key** for the consumed-notification collision (Q2.2).
   - Where: `crates/tasks` (wakes table) and `crates/roster` desk wake (design 70 DESK-PROTOCOL).
   - Effort: hours. Value: prevents silent lost completions on parent desks.
2. **Fail-closed unpriced usage and settle-before-next-run in budgets** (Q2.3).
   - Where: theuth's spend ledger and budget auto-pause (the 2026-09-25 copy; designs 67/69), plus the Agents page budget chip.
   - Effort: about a day.
3. **Desk-manual diet run as an autoresearch study** (Q2.5).
   - Mechanism: benchmark desk outcomes on a fixed task set, then trim `crates/roster/src/desk.rs` manual sections one at a time and keep only cuts that hold the score.
   - Where: design 140 is exactly this loop. Effort: one study and the benchmark.
   - Value: fewer competing instructions on every desk episode.
4. **`verifying` on hire and `cleaning_up` on termination** (Q2.6).
   - Mechanism: run a one-turn harness/model/tool smoke check before the first wake, and show the result on the hire card.
   - Where: roster (design 67), hiring wizard. Effort: a day.
5. **Reviewed per-MCP-server instructions; server init prose untrusted by default** (Q2.7).
   - Where: `~/.theuth/mcp/*.json` plus the mcp config.yaml gates and prompt assembly. Effort: about a day.
6. **Agent suggestion box feeding dreaming** (Q2.8).
   - Mechanism: a `suggest`/`complain` runtime function writes rows, and dreaming's Mine stage (design 136) reads them as cases.
   - Effort: small. Value: turns friction into skill edits, a learning loop Paperclip lacks.
7. **Reservations for theuth's own background calls** (Q2.4): dreaming, endless compactor, title model, decide.
   - Where: the model_calls/spend ledger. Effort: a day or two.
8. **A lease and fence protocol at the external-harness boundary** (Q2.1, Q2.13).
   - Mechanism: ACP/omp children and A2A peers get accept-by deadlines, renewable leases with a total cap, request-ID-idempotent writes and fence notices.
   - Where: design 50 send_message/A2A, task checkout. Effort: a week. Value: higher once remote and external agents take theuth tasks.
9. **theuth board as an MCP server with task events and a Claude/Codex plugin pack** (Q2.9–10). This lets a user's own assistant delegate to and watch theuth
   desks. Effort: weeks. Value depends on whether Sky wants outside assistants in.
10. **Later, only if multi-human:** per-task ACL with intersection authority (Q2.12), and team template packages as folders with a seed project and a first task
    (from teams-catalog).

## Their weaknesses and failure modes
1. **Churn and recovery whack-a-mole.**
   - 201 fixes in 17 days.
   - About 30 commits only add "bounded diagnostics" or keep "proven failures out of Sentry".
   - A fresh heartbeat refactor of about 21k lines moved into modules in two days (75918997a…65965f0d3).
   - **[I]** The risk is regression-prone recovery code, and their own plan docs list consumed-wake and watchdog-loop bugs
     (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L20-L24`).
2. **The runner is still experimental.** The ADR is "Proposed" and the flag is default-off (`paperclip/doc/architecture/paperclip-runner.md#L3`, `paperclip/doc/architecture/paperclip-runner.md#L20-L22`).
   The default switch is split into four slices, none landed (`paperclip/doc/plans/2026-10-06-default-agent-runner.md#L15-L20`). Two execution paths coexist: legacy
   adapters and native runner.
3. **Plugin security is honour-system on the UI side.** Same-origin JS, with capabilities that do not bind UI code, and no multi-node distribution
   (`paperclip/doc/plugins/PLUGIN_SPEC.md#L23-L29`).
4. **Privacy is control-plane only.** "task ACLs are not a sandbox or data-loss-prevention system", and a trusted agent can leak into shared memory
   (`paperclip/doc/ISSUE-PRIVACY.md#L73-L79`).
5. **External agents are opaque.**
   - Fences "do not confirm that Dot stopped" (`paperclip/doc/openai-dot-runner.md#L162-L164`).
   - No usage or cost (`paperclip/doc/openai-dot-runner.md#L159`).
   - macOS commands are disabled in the workspace bridge (`paperclip/doc/openai-dot-runner.md#L161`).
   - Self-hosted Dot needs a stable public HTTPS origin (`paperclip/doc/openai-dot-runner.md#L9-L12`).
6. **Background spend is on by default** for decision models (`paperclip/doc/decision-models.md#L5`).
7. **Memory and learning are still absent as product.** Memory is 🟡 via third parties, and organisational learning and self-organisation are ⚪ (`paperclip/ROADMAP.md#L117-L135`).
   The knowledge layer is a top-6 user ask (#1858).
8. **Static placement.** There is still no worker registry or "run where the repo is". The workspace-sync bugs show the cost:
   - "Preserve remote work when required workspace restore fails" (f1c44b7b5);
   - "retain stopped ephemeral workspaces after failed copy-back" (7e10dbe65);
   - open bug #7790, "execution_workspaces never auto-close".
9. **The harness long tail is unbounded.** The top asks are more adapters (Ollama, Copilot, OpenRouter, Antigravity, Z.AI), and the top bug is a whole-harness failure
   ("All Claude Code agents fail…", #14093). **[I]** Every harness upgrade is a qualification campaign for them.
10. **Docs drift.** SPEC.md still describes a small V1, ADRs stay "Proposed" while shipping, and plans live alongside product docs (54 new plan files in 17 days).
    Trust code and the caveat sections.
11. **Agent-built at scale.** There are 3,403 open PRs, and plan docs show agent branches waiting on "the user chooses to proceed"
    (`paperclip/doc/plans/2026-10-06-default-agent-runner.md#L93-L96`). **[I]** Review is the bottleneck, and it shows in the PR backlog.

## Cites self-check
I re-opened 50 load-bearing cites at the end of the run, reading each cited range from disk at HEAD eedf25655 and checking that its key phrase is there.
**50/50 held.** The only first-pass miss was `openai-dot-runner.md#L162-L164`, where "do / not confirm" wraps across a line with indentation; it held once
whitespace was normalised. **One correction** to the inputs rather than to my own cites: `packages/paperclip-runner` is not a new tree. It had 857 files
at 7b7c4d417 (`git ls-tree`), so the delta is +337 files added and 260 modified, not 1,195 new.
- `paperclip/doc/architecture/paperclip-runner.md#L20-L22`: "default-off": held
- `paperclip/doc/architecture/paperclip-runner.md#L3`: "Proposed": held
- `paperclip/doc/architecture/paperclip-runner.md#L73-L76`: "outbound PRP": held
- `paperclip/doc/architecture/paperclip-runner.md#L115-L119`: "/api/runner/v1/connect/:runId": held
- `paperclip/doc/plans/2026-10-06-default-agent-runner.md#L7-L10`: "Existing agents keep their recorded runner": held
- `paperclip/doc/plans/2026-10-06-default-agent-runner.md#L32-L39`: "Agent defaults, saved": held
- `paperclip/doc/plans/2026-10-06-default-agent-runner.md#L86-L91`: "$250 ceiling": held
- `paperclip/doc/plans/2026-10-05-assistant-invitations.md#L20-L23`: "No client-credentials or agent identity": held
- `paperclip/doc/plans/2026-10-05-assistant-invitations.md#L7-L10`: "consenting human": held
- `paperclip/doc/openai-dot-runner.md#L9-L12`: "stable public HTTPS origin": held
- `paperclip/doc/openai-dot-runner.md#L120-L132`: "must never": held
- `paperclip/doc/openai-dot-runner.md#L134-L147`: "one": held
- `paperclip/doc/openai-dot-runner.md#L156-L158`: "two-hour lease": held
- `paperclip/doc/openai-dot-runner.md#L162-L164`: "do not confirm": held (keyword split across a wrapped line; re-checked with whitespace normalised)
- `paperclip/doc/openai-dot-runner.md#L21-L23`: "unmetered": held
- `paperclip/doc/openai-dot-runner.md#L30-L33`: "previewing consumes no code": held
- `paperclip/server/src/services/dot-runner-broker.ts#L224`: "10 * 60_000": held
- `paperclip/server/src/services/dot-runner-broker.ts#L258-L260`: "24-hour total": held
- `paperclip/doc/public-mcp.md#L318-L332`: "events/subscribe": held
- `paperclip/doc/public-mcp.md#L349-L354`: "Standard Webhooks HMAC": held
- `paperclip/server/src/routes/public-mcp.ts#L159-L166`: "events/subscribe": held
- `paperclip/doc/AGENT-IDENTITY.md#L3-L6`: "Ed25519": held
- `paperclip/packages/db/src/schema/agent_identity_keys.ts#L5-L11`: "agent_identity_keys": held
- `paperclip/doc/AGENT-LIFECYCLE.md#L38-L54`: "cleaning_up": held
- `paperclip/doc/agent-files.md#L114-L134`: "last synchronization to acquire the lock wins": held
- `paperclip/doc/ISSUE-PRIVACY.md#L61-L66`: "intersection of the agent and the run's responsible user": held
- `paperclip/doc/ISSUE-PRIVACY.md#L73-L79`: "not a sandbox": held
- `paperclip/packages/db/src/schema/issues.ts#L41`: "visibility": held
- `paperclip/packages/db/src/schema/issues.ts#L98`: "private": held
- `paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`: "parent/child pair": held
- `paperclip/server/src/services/budgets.ts#L224-L227`: "unpricedUsagePolicy": held
- `paperclip/server/src/services/budgets.ts#L572-L577`: "await accounting": held
- `paperclip/server/src/services/budgets.ts#L388-L391`: "pendingRunCount": held
- `paperclip/doc/decision-models.md#L45`: "accounting lock": held
- `paperclip/doc/decision-models.md#L5`: "defaults on": held
- `paperclip/doc/connections/MEMORY.md#L3-L15`: "no separate memory": held
- `paperclip/ROADMAP.md#L117-L119`: "Memory / Knowledge": held
- `paperclip/ROADMAP.md#L133-L135`: "Automatic Organizational Learning": held
- `paperclip/doc/connections/CONNECTION-INSTRUCTIONS.md#L10-L18`: "never adopted as trusted": held
- `paperclip/doc/agent-commentary.md#L70-L73`: "no feedback UI": held
- `paperclip/packages/db/src/schema/agent_commentary.ts#L9-L21`: "idempotencyKey": held
- `paperclip/doc/plugins/PLUGIN_SPEC.md#L23-L24`: "same-origin": held
- `paperclip/server/src/services/issue-rewake-throttle.ts#L31-L34`: "120_000": held
- `paperclip/doc/architecture/durable-continuation-scheduler.md#L20-L23`: "30,000": held
- `paperclip/doc/plans/2026-10-08-wait-resume-ownership.md#L12-L19`: "request_human_input": held
- `paperclip/integrations/assistant-plugins/README.md#L3-L5`: "impersonation": held
- `paperclip/doc/agent-permission-defaults.md#L21-L25`: "process adapter": held
- `paperclip/packages/teams-catalog/MIGRATION.md#L13-L15`: "Short role description": held
- `paperclip/doc/plans/2026-10-07-personal-primary-agent.md#L21-L32`: "primary agent per company": held
- `paperclip/server/src/services/costs.ts#L17`: "subscription_included": held

**Not verified:**
- I did not diff blob contents between 7b7c4d417 and HEAD for budgets.ts. So "799e4d556 introduced fail-closed accounting" is inferred from the file's commit log.
- I did not trace what writes `decision_training_examples`.
- I did not read the stock-harness comparison or the hiring-replay evidence files in depth.
- Release dates come from the GitHub releases API. Issue and PR counts come from GitHub search via the MCP tools, as of about 13:15 BST on 2026-10-10.
