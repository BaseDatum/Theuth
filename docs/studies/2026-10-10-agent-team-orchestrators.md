# Open-source frameworks for driving a team of agents, compared with theuth

| | |
|---|---|
| **Date** | 2026-10-10 |
| **Question (Sky, 13:01, verbatim)** | "can you research the best and most popular open source frameworks like paperclip for driving a team of agents. then do a study comparing them to ~/Develop/theuth/ what can we learn from them, how can we improve theuth?" |
| **Status** | Study, docs only. No design number claimed. The next step is Sky's rulings on §10. |
| **Licence stance** | Techniques only, never copied code, whatever the licence (Multica is Apache-2.0 plus extra conditions; the rest are MIT or Apache-2.0). |
| **Evidence** | Nine recon reports (appendices A–I: 5,505 lines and 2,332 `path#L` cites, plus 18 web pages re-probed; every author re-opened their own cites). The orchestrator re-opened 40 load-bearing cites in both trees, and all 40 held. The theuth task-board census was re-run read-only. |
| **theuth tree** | main df1e8204 (team code unchanged since b4e5833a, where the recon read it) |

## 1. Verdict

**Q1 — What do they offer versus theuth?**
- **The field has three tiers.**
  - **Tier 1, control planes that drive existing harnesses as a team.** This is Paperclip's category, and theuth's: Paperclip, Multica, Symphony, Gas Town + Beads, Agent Orchestrator.
  - **Tier 2, teams inside one harness.** Ruflo and oh-my-claudecode, plus method packs that out-star everything.
  - **Tier 3, SDKs for coding a team.** CrewAI, LangGraph, Microsoft Agent Framework.
- **theuth is a tier-1 plane, and on *where and how agents run* it is ahead of every project surveyed.** No tier-1 tool has more than external memory providers, and none has an owner model. theuth has:
  - a leaderless mesh with per-task placement of machine and harness;
  - sandbox VMs and external harnesses over ACP;
  - the double;
  - decision packets with mini-boards and an attention inbox;
  - knowledge with nightly dreaming, and autoresearch;
  - chat channels.
- **It is behind on Paperclip's newer protocol rigour:** completion checked against durable evidence, leases for external agents, fail-closed accounting.
- **The coding planes' work-to-merge loop is out of scope** (tracker as queue for code, a worktree per task, PR/CI feedback, a merge queue). Sky ruled at 14:10 that theuth does not manage worktrees and that a task is not a codebase change (§12, Revision 1). From those tools theuth takes only their general mechanisms: reconcile, stall detection, typed receipts, attention that clears itself, and external events routed to a task's owner.

**Q2 — What can we learn?**
- theuth's own record says its failures are not missing primitives. They sit at the **edges**:
  - wakes delivered to sessions whose state lives in memory;
  - authority read from the session root;
  - rules that live only in prompts;
  - one human reviewer. 74 of the 75 open tasks wait on Sky, and the board has not moved since 2026-10-06 18:34 (§4.2).
- The best ideas in the field are edge mechanisms. For each, the mechanism theuth lacks:

| Area | Where theuth falls short | What the best of the field does |
|---|---|---|
| Runs | wakes keep no run record | durable run rows with typed failures and claim leases (Multica) |
| Wakes | any word equal to an agent name wakes it | typed mentions with a pre-send wake preview (Multica) |
| Message delivery | a message is reported `accepted` | one guarded delivery path with typed receipts (Agent Orchestrator) |
| Attention | rows are cleared by hand | rows that resolve themselves from the fact that raised them (Agent Orchestrator) |
| Liveness | stalled runs are not detected; a moved task is not reconciled | reconcile-before-dispatch and stall kill-and-retry (Symphony) |
| Runaways | no respawn caps or mass-death pause | respawn caps and mass-death pause (Gas Town) |
| Completion | checks against `done_when` are free text | criteria checked against durable evidence (Paperclip) |
| Progress | `no_progress_count` notices only unchanged fields | a structured progress judgement (Agent Framework's Magentic) |

**Q3 — How should theuth improve?**
- **Fix the edges first.** Wave 1 is ten items, each small. Then:
  - Wave 2, durable runs and liveness;
  - Wave 3, done means evidence, so fewer reviews need Sky; the evidence gate and the double close the rest;
  - Wave 4, outside work and signals: Jira or GitHub issues as task sources, and watched external events routed to the task's owner.
- **Do not invest in more hierarchy.** The 2026 evidence shows CEO-style prompts add little. Procedure, verification and cost control are where a control plane pays (§5).

## 2. Method

The method follows the competitor-recon procedure: shallow clones in `~/Develop/spike-agent-teams/`, plus one `AXES.md` holding the ask verbatim, the field by tier, eleven numbered axes and the rules. Each child got its own brief. Nine theuth subagents were spawned at 13:10, and all nine reports landed between 13:22 and 13:28. Every child was read-only: no builds and no runs.

| | Child | Slice (HEAD) | Report | Lines | `#L` cites | Author re-opened |
|---|---|---|---|---|---|---|
| A | recon-paperclip | paperclipai/paperclip eedf25655, plus the delta since 7b7c4d417 | [A-paperclip](2026-10-10-agent-team-orchestrators/A-paperclip.md) | 650 | 211 | 50, all held |
| B | recon-multica | multica-ai/multica c8d6a1f | [B-multica](2026-10-10-agent-team-orchestrators/B-multica.md) | 729 | 407 | 45, all held |
| C | recon-gastown | gastownhall/gastown 649b832 + beads eb20436 | [C-gastown-beads](2026-10-10-agent-team-orchestrators/C-gastown-beads.md) | 484 | 351 | 61, 1 corrected |
| D | recon-symphony | openai/symphony be10a1b | [D-symphony](2026-10-10-agent-team-orchestrators/D-symphony.md) | 587 | 378 | 89, all held |
| E | recon-agent-orchestrator | OrchestratorInc/agent-orchestrator 4273bec | [E-agent-orchestrator](2026-10-10-agent-team-orchestrators/E-agent-orchestrator.md) | 599 | 300 | 185, 2 drifts fixed |
| F | recon-swarms | ruvnet/ruflo 6c04654 + Yeachan-Heo/oh-my-claudecode 454bae0 | [F-ruflo-omc](2026-10-10-agent-team-orchestrators/F-ruflo-omc.md) | 472 | 254 | 88, 2 corrected |
| G | recon-frameworks | crewAI 6b93fa0, langgraph b4991f1, agent-framework fd52de7 | [G-frameworks](2026-10-10-agent-team-orchestrators/G-frameworks.md) | 638 | 244 | 153, 6 widened |
| H | recon-ours | theuth main b4e5833a | [H-theuth](2026-10-10-agent-team-orchestrators/H-theuth.md) | 745 | 187 | 59, all held |
| I | recon-web (read_only) | the field, users, native floor, evidence | [I-web](2026-10-10-agent-team-orchestrators/I-web.md) | 600 | 55-row table | 18 pages re-probed |

**Orchestrator checks:**
- I re-opened 40 load-bearing ranges across both trees, each checked against a keyword. All 40 held.
- I read the decisive Paperclip mechanism myself before the reports landed: the evidence classifier.
- I re-ran the theuth task-board census with `sqlite3 -readonly`.

**Shortlist rule:** the most-starred open-source projects whose job is running *several* agents as a team, one or two per tier. Stars come from the GitHub MCP on 2026-10-10.

## 3. Theirs, in one screen

Cites are `<clone>/<path>#L…` into `~/Develop/spike-agent-teams/` at the HEADs in §2. theuth cites are repo-relative.

**Paperclip** (MIT, TypeScript, 99.5k★; 2,925 open issues and 3,403 open PRs). Paperclip is a company-shaped control plane: agents with adapters, an org chart, issues, heartbeats, budgets and approvals. It has gained 414 commits since theuth copied it, and the decisive new mechanisms are protocol hygiene:
- **Completion contract.** A finished run claims each criterion of a revisioned contract, pointing at **durable evidence**: run events, work products, approvals, interactions, attachments. A deterministic classifier then marks each claim accepted, missing, rejected or unverifiable, and a stale revision is rejected outright (`paperclip/server/src/services/native-runtime/evidence-classifier.ts#L271-L310`).
- **Lease protocol for external agents.** An external agent (the "Dot") must accept within 10 minutes and then holds a 2 h lease, renewable up to a 24 h cap. Writes are idempotent by request ID. Pause and cancel "fences" are acknowledged on receipt only (`paperclip/doc/openai-dot-runner.md#L120-L169`).
- **Fail-closed accounting.** Admission refuses new work while finished runs still await accounting, or while usage has no reliable price (`paperclip/server/src/services/budgets.ts#L224-L227`, `#L572-L577`).
- **Wake dedup keyed on the decision.** It is keyed on the completion decision, not on the (parent, child) pair (`paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17`).
- **Lifecycle checks.** States include `verifying` before the first assignment and `cleaning_up` after the last (`paperclip/doc/AGENT-LIFECYCLE.md#L38-L54`).

Runs are still launched by the server, so a NAT'd machine still cannot volunteer as a worker. Memory is external providers behind a flag.

**Multica** (Apache-2.0 plus a no-hosting/no-embedding condition; Go; 52.4k★). An issue board where agents are teammates and a daemon on your machine runs their CLIs.
- **A durable run row for every wake.** The claim is one `FOR UPDATE SKIP LOCKED` statement, serialised per agent and issue, with a 15 s prepare lease (`multica/server/pkg/db/queries/agent.sql#L749-L810`).
- **Only typed links wake agents.** A wake needs a `mention://agent/<uuid>` link (`multica/server/internal/util/mention.go#L11`). One predicate decides who will wake, for both the save and the composer's preview.
- **Agents set their own wake rules.** A rule can carry an expiry and stops after 20 fires by default (`multica/server/internal/service/issue_wakeup_activity.go#L20`).
- **Authority comes from the human at the top of the chain.** No per-tool approvals and no spend caps.

**Symphony** (Apache-2.0, Elixir with a language-neutral `SPEC.md`; 27.6k★; an "engineering preview").
- **The tracker is both the queue and the console.** The repo-owned `WORKFLOW.md` sets active and terminal states, polling, hooks and concurrency, and holds a strict prompt template (`symphony/elixir/WORKFLOW.md#L7-L17`). It is hot-reloaded and keeps the last good version (`symphony/elixir/lib/symphony_elixir/workflow_store.ex#L143-L176`).
- **Reconcile before every dispatch tick** (`symphony/SPEC.md#L819-L839`).
- **Stall kill-and-retry.** A run with no Codex event for 5 minutes is killed and retried with backoff (`symphony/elixir/lib/symphony_elixir/orchestrator.ex#L581-L639`).
- **Bounded continuation.** Up to 20 turns per run (`symphony/elixir/lib/symphony_elixir/agent_runner.ex#L101-L154`).
- **The weakness is a cautionary tale.** Clean exits re-dispatch an issue forever while it stays active, and claims live only in memory.

**Gas Town + Beads** (MIT, Go; 18.3k★ and 27.8k★). Gas Town's main branch has been frozen since 2026-07-23; Beads is active.
- **Hook and GUPP.** A "hook" is a durable pointer from an agent to its work, and GUPP is the rule "hooked work must run". The hook is re-primed after compaction. Supervision distinguishes "alive, hooked, no progress for 30 min" from "dead with work" (`gastown/internal/daemon/lifecycle.go#L1089-L1153`).
- **Claiming ready work.** Beads takes the next ready item atomically, using the same filter as the listing (`beads/issueops/readyclaimer.go#L5-L44`).
- **Typed dependencies:** blocks, conditional-on-failure, waits-for (`beads/internal/types/types.go#L1427-L1457`).
- **A merge queue.** The refinery batches changes, then bisects to the culprit on a red batch (`gastown/internal/refinery/batch.go#L10-L35`).
- **The substrate is the anti-pattern:**
  - it types into terminal UIs;
  - it installs hooks per harness;
  - it runs with `--dangerously-skip-permissions`;
  - it spent users' credits on upstream work (issue #3649).

**Agent Orchestrator** (Apache-2.0, Go; 13k★; ex-Composio). Each worker is one task, one agent, one worktree and one PR loop.
- **Derived status.** Status and board lane are computed by one pure function, never stored (`agent-orchestrator/backend/pkg/contract/status.go#L107-L137`).
- **CI and review feedback go straight back to the owning worker.** CI failures, each review comment and merge conflicts are de-duplicated and capped, with no LLM in the path (`agent-orchestrator/backend/internal/lifecycle/reactions.go#L158-L267`).
- **One guarded path for every write into an agent.** It refuses to type into a pending permission prompt and returns typed "suppressed" outcomes (`agent-orchestrator/backend/internal/sessionguard/guard.go#L1-L75`).
- **Notifications clear themselves.** Each one resolves when the fact that caused it changes (`agent-orchestrator/backend/internal/notify/manager.go#L110-L148`).

**Ruflo + oh-my-claudecode** (MIT; 74.2k★ and 39.8k★).
- **Ruflo's swarm is records, not agents.** Its own CLI says real work needs Claude Code's Task tool (`ruflo/v3/@claude-flow/cli/src/commands/swarm.ts#L1046-L1047`), and its "consensus" is vote counting (`ruflo/v3/@claude-flow/cli/src/mcp-tools/hive-mind-tools.ts#L83-L105`).
- **oh-my-claudecode has three things worth taking:**
  - it refuses completion without verification evidence (`oh-my-claudecode/src/team/state/tasks.ts#L311-L318`);
  - a merge orchestrator merges worker branches one at a time (`oh-my-claudecode/src/team/merge-orchestrator.ts#L1-L21`);
  - new guards ship in shadow mode first (`oh-my-claudecode/scripts/budget-guard.mjs#L1-L23`).

**CrewAI, LangGraph, Microsoft Agent Framework** (MIT). These are libraries, not control planes. The useful parts are primitives:
- **Magentic's progress ledger.** Each round answers done, looping, progressing, who next and what instruction; when the stall counter passes 3 the manager replans (`agent-framework/python/packages/orchestrations/agent_framework_orchestrations/_magentic.py#L205-L244`).
- **LangGraph's two clocks.** Each step has a run timeout and an idle timeout (`langgraph/libs/langgraph/langgraph/types.py#L461-L490`).
- **LangGraph's mid-run input rule.** Input arriving while busy is rejected, interrupts, rolls back or is enqueued (`langgraph/libs/sdk-py/langgraph_sdk/schema.py#L81-L88`).
- **CrewAI learns from review.** It pre-reviews output with recalled lessons, then distils the reviewer's feedback into lessons (`crewai/lib/crewai/src/crewai/flow/human_feedback.py#L279-L296`).

## 4. The axes: theirs, ours, gap

### 4.1 Axis table

| Axis | Best in the field (who) | theuth today | Verdict |
|---|---|---|---|
| A1 Agent model | Adapters, Ed25519 identity, lifecycle with `verifying` (Paperclip); agent = name + provider + runtime, plus skills (Multica) | Agent files on a roster: charter, model/effort/tools/mcp/budget. Harness chosen per task, not bound to the agent. Hiring in chat plus a wizard | **On par.** Gaps: template catalogue, smoke check on hire |
| A2 Organisation & authority | Org chart; private tasks with intersection authority (Paperclip); authority carried by the originating human (Multica) | `reports_to`, escalate with packets, write cap, the double | **Ahead on decisions.** Gap: authority read from the actor string (`crates/tasks/src/ledger.rs#L1619-L1622`). Do not add hierarchy |
| A3 Work model | Tracker as queue (Symphony, AO); typed dependencies, gates, templates (Beads); completion contract (Paperclip) | 7 states, one owner, reviewer, three blocker kinds, parent tree, `done_when` + `done_because`, review packets | **Behind** on tracker, goals, templates and evidence-checked done |
| A4 Execution & runtime | Run rows with leases and typed failures (Multica); reconcile and stall (Symphony); detached hosts that survive upgrades (AO) | Wake ledger, mesh-hub dispatch, orphan sweep with liveness probe, epochs, placement, VMs, ACP | **Ahead on placement; behind** on durable run state, stall detection, reconcile, claim time (`crates/tasks/migrations/0001_tasks.sql#L34-L44`) |
| A5 Coordination | Guarded delivery with typed receipts (AO); typed mentions (Multica) | Ledger, threads, peer messages, checkout lock | **Behind** on delivery semantics and mentions (`crates/tasks/src/ledger.rs#L1543-L1548`) |
| A6 Human in the loop | Self-resolving notifications (AO); plan approval (Claude Agent Teams); review gates (Multica) | Risk policy, approvals, the double, packets with `if_wrong`, attention inbox, alerts, companions, channels | **Ahead.** Gaps: review routing, a working Unblock, self-resolving rows |
| A7 Cost & runaway | Fail-closed accounting and reservations (Paperclip); respawn caps and mass-death pause (Gas Town); launch/quota fuse (Ruflo) | Monthly per-agent budget pause, write cap, back-offs, chain bound | **Behind** on scopes, per-task cost and loop guards on background calls |
| A8 Memory & learning | External providers (Paperclip); unified memory, lessons from review (CrewAI) | Knowledge with git history, dreaming, double, autoresearch, endless memory | **theuth ahead** |
| A9 Observability & UX | Derived "whose turn" lanes (AO); execution log + usage per issue (Multica); usage view (Orca) | Org chart, board, HUD, attention, routines, transcripts, companions, voice | **Ahead in breadth.** Gaps: run cost, wake timeline |
| A10 Extensibility | Plugins, MCP events, outside assistants joining (Paperclip) | ACP, MCP, A2A, skills, sensors, RPC | **Ahead** (ACP as the adapter contract). Gap: inbound webhook → task |
| A11 Engineering & traction | Paperclip 99.5k★ with 6.3k open issues and PRs; Multica 1k open issues | 558k lines of Rust and 136k of TypeScript, 3.4k commits in a month; private, one user | Not comparable. The field's lesson: trust incidents spread fast |

### 4.2 theuth facts the verdict rests on

**All eleven Paperclip borrows of 2026-09-24 are in code.** Several go further than Paperclip:
- the checkout carries session, engine and epoch;
- there are extra dispositions, `delegated` and `continuing`;
- routines can loop and watch sensors.

The exception is the budget auto-pause, which is only partial: it is monthly and per agent only (appendix H).

**What breaks in use comes from the knowledge record, 2026-09-25 to 10-10 (appendix H, "How the team features behave in anger"):**
- Cold desks ignore peer messages that reported `accepted`.
- Claims went stale after restarts (fixed by WAKE-LIVE-CHILD).
- `escalate` was refused with "you are the top of the line" for re-rooted desk sessions.
- A note that named five desks woke all five.
- Attention's "Unblock" is refused whenever blockers exist.
- Desks self-started backlog work twice. The only guard is in the prompt; the ledger allows it (`crates/tasks/src/ledger.rs#L891`).
- The endless compactor retried a deterministic provider 400 every ~10 s, because no loop guard covers background calls.

**The human is the bottleneck.** The task board on 2026-10-10 at 13:31 (`sqlite3 -readonly ~/.theuth/tasks.sqlite`):
- 80 tasks: backlog 24, blocked 10, review 41, cancelled 3, done 2.
- **74 of the 75 open tasks name `user` as reviewer.**
- The last update anywhere was 2026-10-06 18:34.

That was the end of the DR backlog round. For a while, 14 of its reviews were hidden by an Attention dismissal bug, since fixed by design 119. The structural point stands regardless: the desks can do the work, but nothing except Sky can close it.

**Governance is untested on the real team.** The five live agent files set only `title`, `model` and `capabilities`. None sets `reports_to`, `budget`, `tools` or `mcp`, so reporting lines, caps and ceilings have never been exercised.

**Review checks are free text.** `checks` on a review hand-over are `{text, done, note}` per `done_when` line, and `pointers` carry evidence inline (`crates/tasks/src/tools.rs#L559-L567`). Code excerpts on the mini-board are validated against the tree. Nothing ties a criterion to a durable record the engine can check.

## 5. Users and the evidence

**What users ask for** (open issues by reactions, appendix I §3):
- **Any model or harness:** Paperclip #187 Ollama (79 reactions), #2092 Copilot CLI (44), #1068 OpenRouter (35).
- **Chat with or steer running agents:** Paperclip #49 (49).
- **Reach the team from a phone or Telegram:** Paperclip #2004.
- **External trackers as the queue:** Multica #1634 JIRA (24); vibe-kanban GitLab.
- **A company knowledge layer:** Paperclip #1858 (26).
- **Stage pipelines.** Multica #815 asks "who decomposes the large task? who decides the next stage? who enforces approval / acceptance gates?"
- **Quota and usage awareness:** Gas Town #232.

**What users complain about is trust and reliability, not features:**
- Gas Town #3649 (72 reactions, HN 253 points): release formulas spent users' credits and GitHub accounts on upstream work.
- Ruflo's "99% Theater, 1% Real, 5x Token Waste" audit.
- Vibe Kanban's telemetry, which was on by default.
- Paperclip: "most production bugs live on the adapter boundary (identity, wake context, liveness, token accounting)".

**The native floor rose in 2026:**
- Claude Code Agent Teams: experimental, one team per session, no resume, no nesting.
- Claude Managed Agents: one level deep, at most 25 child threads.
- Codex: subagents on by default, plus app-server.
- Cursor 3: Agents Window plus Cloud Agents.
- GitHub: Copilot cloud agent and Agent HQ.
- Devin and Jules, with concurrency caps.

What is left for an outside plane: cross-vendor and cross-subscription teams, durable identity and memory, one attention surface on phone and chat, local-first data, placement across machines, and governance. **theuth covers all six.**

**Does a team of agents work?**
- **Anthropic research system (2025):** +90.2% at about 15× the tokens of chat, on breadth-first research.
- **MAST:** failure rates of 41–86.7% across 7 open multi-agent systems. Failures come from specification and verification, not capability.
- **Anthropic, 2026-08-13:** a 45-agent swarm found 266 vulnerabilities, against 21 for independent agents. CEO-hierarchy prompts "did not make much difference", and 18 of 30 agents picked the same branch name.
- **Project Vend 2:** forced procedures helped most; the CEO agent "wasn't much help".
- **Cursor's 2026 planner/worker swarm:** "costs varied enormously".

**The conclusion:** a control plane earns its keep through **procedure, verification and cost control**. Parallel, independently checkable work wins; dependent, write-heavy work loses. Hierarchy adds little.

## 6. Borrows, ranked and grouped by where they land

Effort: S = a day or less, M = a few days, L = a week or more. Cost if wrong is the harm when the borrow is mistaken or mis-tuned. Every row is a technique; none copies code.

### 6.1 Wave 1: fix the edges (`crates/tasks`, `crates/roster`, agent communication, attention, schedule)

| # | Borrow | From | theuth seam | Effort | Cost if wrong |
|---|---|---|---|---|---|
| 1 | Guard backlog→todo in code: only the manager, theuth or the user may start backlog | theuth's own incident record (H) | `crates/tasks/src/ledger.rs#L891` | S | Low: a desk asks instead of starting |
| 2 | Escalation authority from the desk's identity, not the `agent:` actor string | H; Multica's originator authority (B) | `crates/tasks/src/ledger.rs#L1619-L1622` + roster caller identity | S | Low |
| 3 | Attention "Unblock" clears resolvable blockers, or opens the decision | H | `crates/roster/src/attention.rs` + HUD/companion actions | S | Low |
| 4 | Self-resolving attention rows: each row is keyed to the fact that raised it, resolved when that fact changes, reconciled at boot | AO `agent-orchestrator/backend/internal/notify/manager.go#L110-L148` | Attention inbox (104/132), activity (155) | S | Low: a row may close early |
| 5 | Typed mentions, a "will wake: …" preview and a `/note` prefix; plain names never wake | Multica `multica/server/internal/util/mention.go#L11` | `crates/tasks/src/ledger.rs#L1543-L1548` + the task composer | S–M | Low |
| 6 | A message to a cold desk raises a ledger wake. One guarded delivery path returns typed receipts (delivered / queued-cold / suppressed: awaiting_user, busy, restarting) instead of `accepted` | AO `agent-orchestrator/backend/internal/sessionguard/guard.go#L1-L75`; H pain point 1 | Peer intake `crates/agent/src/supervisor.rs#L1872-L1879` → roster wake | S–M | Medium: more wakes, so it needs the back-off |
| 7 | `claimed_ms` on `wakes`, plus a "wake held > N min" attention item | Multica's lease (B); H | `crates/tasks/migrations/0001_tasks.sql#L34-L44` + attention | S | Low |
| 8 | Audit the coalesced-wake key: dedup on the completion decision, not on (parent, child) | Paperclip `paperclip/doc/plans/2026-10-06-native-delegation-handoff.md#L14-L17` | Ledger wake coalescing | S | Low |
| 9 | Runaway caps: a per-task redispatch/respawn cap, then escalate; mass-death auto-pause; failure-rate auto-pause for routines; a machine-wide launch and quota fuse; a loop guard plus a budget reservation on background model calls | Gas Town (C), Multica (B), Ruflo #2661 (F), Paperclip `doc/decision-models.md#L45` (A) | Ledger release/back-off, roster dispatch, `crates/schedule`, spend ledger | S each | Medium: a false stop on a legitimate burst |
| 10 | Shadow mode for every new blocking guard: log "would have blocked" before enforcing | omc `oh-my-claudecode/scripts/budget-guard.mjs#L1-L23` | `tools.policy`, write caps, sweep, double rungs | S | Low |

### 6.2 Wave 2: durable runs and liveness (`crates/tasks`, roster wake path, agent supervision, budgets)

| # | Borrow | From | theuth seam | Effort | Cost if wrong |
|---|---|---|---|---|---|
| 11 | A `runs` table under `wakes`: attempt, typed failure reason and whether it is retryable, session and workdir, claim and lease heartbeat, lineage. A poisoned session is never resumed | Multica `multica/server/pkg/db/queries/agent.sql#L749-L810`; Paperclip runs; Beads leases | `crates/tasks` store + migration; in-memory desk state moves to rows | M | Low: additive |
| 12 | Reconcile on move: stop the run when its task leaves the active set or changes owner | Symphony `symphony/SPEC.md#L819-L839` | `task_move` → roster interrupt | S–M | Low |
| 13 | Two-clock timeouts (run, and idle refreshed by activity) for subagents, ACP children and desk episodes, with a stuck taxonomy: working, stalled, hooked-not-progressing, orphan, no-signal | LangGraph `langgraph/libs/langgraph/langgraph/types.py#L461-L490`; Symphony `orchestrator.ex#L581-L639`; Gas Town `lifecycle.go#L1089-L1153`; AO | Agent supervision, ACP, activity (155) | S–M | Medium: killing a slow honest run; use generous defaults |
| 14 | A progress-ledger judgement on no-progress wakes, escalating with the ledger instead of only backing off | Agent Framework `_magentic.py#L205-L244` | `crates/tasks/src/ledger.rs#L2075-L2089` + desk protocol | M | Low: one cheap structured call |
| 15 | Re-inject the checked-out task (title, `done_when`, last progress note) after every memory fold | Gas Town `gt prime` and GUPP (C) | Endless render (137) + desk manual | S | Low |
| 16 | Cost per task and project, budget scopes (project, lifetime), fail-closed accounting for unpriced usage | Paperclip `paperclip/server/src/services/budgets.ts#L224-L227`; Multica usage per issue | `crates/roster/src/budget.rs` + `model_calls` | M | Low–medium: false stops when a price table lags a new model |

### 6.3 Wave 3: done means evidence; reviews flow (tasks tools and packets, double, dreaming)

| # | Borrow | From | theuth seam | Effort | Cost if wrong |
|---|---|---|---|---|---|
| 17 | Evidence-backed done. Each `done_when` line gets an id and a revision. `checks` carry evidence refs, resolved against durable records (tool-call ids, artifacts, documents, messages sent, job logs, external links) and classified accepted / missing / rejected / unverifiable on the review card. An amendment ledger records changed criteria | Paperclip `evidence-classifier.ts#L271-L310`; omc `tasks.ts#L311-L318`; Symphony proof of work | `crates/tasks/src/tools.rs#L559-L567`, `packet.rs`, review card | M | Medium: friction; ship in shadow first (#10) |
| 18 | Shrink what needs Sky's review, without a manager hop. Sky ruled on 2026-10-06 18:38 that a review addressed to him lands in Attention on its own, never via the manager. Three ways: (a) a desk may close its own task against an explicit `done_when` when every criterion's evidence is *accepted* (#17); this extends today's self-review rule; (b) the double answers review classes as Sky, by its trust ladder and 30 s grace; (c) Attention offers batched review digests | H census; Paperclip evidence gate (A); the double (126) | Effective reviewer in `ledger.rs`, `packet.rs`, `crates/double`, attention | M | Medium: a wrong close; the evidence classes and the double's ladder bound it |
| 19 | Lessons from review dispositions become dreaming candidates, kept only if replay passes | CrewAI `human_feedback.py#L279-L296`; Paperclip suggestion box (A) | Review transitions → `crates/dreaming` | M | Low: the replay gate filters |
| 20 | Workpad convention: one progress note updated in place, holding plan, criteria checklist, validation and a Confusions section | Symphony `WORKFLOW.md#L295-L329` (D) | Desk manual + `task_note` | S | Low |

### 6.4 Wave 4: outside work and signals (sensors, MCP, tasks). Revised 14:10, see §12

| # | Borrow | From | theuth seam | Effort | Cost if wrong |
|---|---|---|---|---|---|
| 21 | **Dropped** (Sky, 14:10): engine-managed worktrees. theuth does not manage worktrees; a harness it orchestrates owns its own workspace | — | — | — | — |
| 22 | External events to the task's owner. A watched signal wakes the owner with a typed reason, de-duplicated and capped, with no LLM in the path. Signals include a Jira or GitHub comment, a monitor alert, mail, or a PR/CI result when the task happens to involve code | AO `agent-orchestrator/backend/internal/lifecycle/reactions.go#L158-L267` (the mechanism, not its code scope) | Sensors (`crates/schedule`) → owner wake reason | M | Low |
| 23 | Work trackers as task sources: Jira first (the SRE desks), then GitHub issues. Import eligible items as tasks with `external`, map their states, and write back host-side through the MCP tools | Symphony's two-read kernel (D); Multica #1634 ask | Sensors + `task_create` + `external` field | M–L | Medium: sync conflicts; start one-way |
| 24 | **Dropped** (Sky, 14:10): a merge queue. Not a theuth feature | — | — | — | — |

### 6.5 Later, or only if needed
- **Lease and fence protocol for external agents and A2A peers that take theuth tasks.** From Paperclip `openai-dot-runner.md#L120-L169`. Worth doing once remote or external agents take work.
- **Task templates that expand into a task DAG, with distillation back into a template.** Typed blockers `waits-for`, `if-failed` and `gate` (Beads).
- **Agent-set conditional wakeups** with an expiry and a fire cap (Multica).
- **A smoke check (`verifying`) on hire** before the first wake (Paperclip).
- **A measured prompt diet for the desk manual** (`crates/roster/src/desk.rs#L51-L95`), run as an autoresearch study (Paperclip).
- **theuth's board as an MCP server**, so outside assistants can delegate to desks (Paperclip's assistant invitations).
- **An optional plan-first gate** (Claude Agent Teams).
- **Diversity defaults in fan-out**, varying model or angle and using unique claim names (Anthropic 2026).
- **Quota-aware routing and a usage view** (Orca, Gas Town #232).
- **A documentation map for `docs/design`** (AO).

## 7. Do not borrow

| Pattern | Seen in | Why not |
|---|---|---|
| Binding an agent to one harness; runs started by the server | Paperclip | Standing theuth decision: placement picks the harness per task, and desks run on the mesh |
| A CLI backend or hook install per harness | Multica (26 backends), Gas Town (9 harness hook sets) | Sky's ACP-first ruling of 2026-10-09: PTY is a compatibility mode |
| Typing into terminal UIs; liveness scraped from the screen | Gas Town, oh-my-claudecode | Breaks on every harness release (Gas Town #4818) |
| Swarm topologies, hive mind, consensus, hundreds of MCP tools | Ruflo | Records rather than agents, and its own audit calls it theatre |
| CEO and org-chart theatrics beyond `reports_to` | Paperclip companies, edict, Clawith | Anthropic 2026 and Vend 2: little gain from hierarchy prompts |
| Auto-approval and bypass-permission defaults | Multica, Gas Town, Symphony (`approval_policy: never`) | theuth's approvals and the double are its lead; keep them |
| An issue tracker as the runtime state store | Gas Town on Dolt (72 doctor checks) | The fragility shows in its issue tracker |
| Spending outside the owner's goal; telemetry on by default | Gas Town #3649, Vibe Kanban | Trust incidents spread fast; theuth's local-first, no-telemetry stance is an asset |
| An SDK framework as the runtime; a manager LLM with only delegate and ask tools | CrewAI hierarchical | The checkout and dispatch path already does this deterministically |
| Unbounded continuation while a task stays active | Symphony | The runaway hole (D, finding 4) |
| Engine-managed worktrees, a PR/CI loop or a merge queue as product features | AO, Symphony, Multica, Gas Town, omc | Sky, 2026-10-10 14:10: theuth is not a coding agent, and a task is not a codebase change |

## 8. Their weaknesses (what not to repeat)

- **Paperclip.**
  - Its own review says the bugs cluster on the adapter boundary: identity, wake context, liveness, token accounting.
  - Weight: 2,925 open issues plus 3,403 open PRs, and one `heartbeat.ts` that was about 29.5k lines until it was split on 2026-10-08/09.
  - The NAT gap is still open (appendix A).
- **Multica.**
  - No per-tool approvals and no spend caps.
  - A separate backend per CLI.
  - Flat squads instead of reporting lines.
  - Licence conditions block hosting it as a service or embedding it (B).
- **Symphony.**
  - Re-dispatches without bound, claims held only in memory, one instance per board.
  - Review gate in the prompt only; the tracker token can do anything (D).
- **Gas Town.**
  - Frozen main branch (energy moved to Gas City).
  - A TUI-typing substrate, `--dangerously-skip-permissions`, cleanup jobs that killed live work, and the credit-spend incident (C).
- **Agent Orchestrator.**
  - Frozen "waiting for input" sessions (#3738).
  - `ao send` reporting success before delivery (#3634), the same lie as theuth's `accepted`.
  - A 3,779-line agent-switching file (E).
- **Ruflo.** Fabricated benchmarks, mock embeddings labelled as a real model, and install and uninstall issues (F).
- **oh-my-claudecode.** Its protocol is kept by prompt only, and it crashed at 6 parallel agents (F, I).
- **The SDKs.**
  - CrewAI's docs contradict its code on memory.
  - LangGraph's supervisor library is archived.
  - Agent Framework's own Monty README admits labels do not reach tool calls made inside generated code (G).
- **theuth itself (H).**
  - Truth that lives in memory: head-of-line blocking (`crates/tasks/src/ledger.rs#L1922-L1926`), identity by string, protocol carried by prompt.
  - God-files: `ledger.rs` is 6,237 lines.
  - A single owner.

## 9. Sequencing

| Wave | Items | Size | Lands in | Why this order |
|---|---|---|---|---|
| 1. Fix the edges | 1–10 | Ten S items, about one omp wave | `crates/tasks`, `crates/roster` (attention, desk), agent communication, `crates/schedule` | Each one retires a recorded incident. They need no new data model, and they make the team usable unattended |
| 2. Durable runs + liveness | 11–16 | M | `crates/tasks` (runs table, migration), roster wake path, agent/ACP supervision, endless render, budgets | Moves the in-memory truth onto the ledger. Waves 3 and 4 need run ids and typed failures to point at |
| 3. Evidence + review flow | 17–20 | M | tasks tools and packet, review card, double (126), dreaming (136), desk manual | Needs run and tool-call ids as evidence refs (wave 2). It removes the review bottleneck the census shows |
| 4. Outside work and signals | 22–23 | M–L | Sensors, Atlassian/GitHub MCP, `task_create` | Worth most once desks can finish, prove and hand back work without Sky (waves 1–3) |
| Later | §6.5 | — | per item | Only when external agents, templates or multiple users arrive |

Waves 1 and 2 touch `crates/tasks/src/ledger.rs`, `crates/roster` and `crates/agent`. Another orchestrator's endless-backoff wave is active in `crates/agent/src/endless/` today. A design for wave 1 should claim its number and name its files against what is in flight when it starts.

## 10. Open questions for Sky

1. **How much of the review queue may close without you?** Your 2026-10-06 ruling keeps reviews off the manager: they land in your Attention. The options, which can be combined:
   - none, as today, with only batched digests;
   - a desk closes its own task when every `done_when` criterion has *accepted* evidence and `if_wrong` is low;
   - the double answers review classes as you, by its trust ladder.

   This decides whether wave 3 changes who closes tasks or only how fast you can.
2. **Tracker bridge: Jira first or GitHub issues first? One-way import or two-way?** Jira serves the SRE desks. Two-way is where sync conflicts live.
3. ~~Engine-managed worktree per code task~~: **answered 14:10, no.** theuth does not manage worktrees, and a task is not a codebase change.
4. **Evidence-backed done: a hard refusal or an advisory badge?** The recommendation is shadow first (#10), then refuse `review` without evidence on tasks that have `done_when`.
5. ~~Merge queue~~: **answered 14:10, not a theuth feature.**

## 11. Appendices

Raw reports in [`2026-10-10-agent-team-orchestrators/`](2026-10-10-agent-team-orchestrators/):
- [AXES](2026-10-10-agent-team-orchestrators/AXES.md): the ask, the field by tier, the eleven axes and the rules every child followed.
- [A-paperclip](2026-10-10-agent-team-orchestrators/A-paperclip.md): Paperclip at HEAD, and what changed in 414 commits.
- [B-multica](2026-10-10-agent-team-orchestrators/B-multica.md)
- [C-gastown-beads](2026-10-10-agent-team-orchestrators/C-gastown-beads.md)
- [D-symphony](2026-10-10-agent-team-orchestrators/D-symphony.md)
- [E-agent-orchestrator](2026-10-10-agent-team-orchestrators/E-agent-orchestrator.md)
- [F-ruflo-omc](2026-10-10-agent-team-orchestrators/F-ruflo-omc.md)
- [G-frameworks](2026-10-10-agent-team-orchestrators/G-frameworks.md): CrewAI, LangGraph, Microsoft Agent Framework.
- [H-theuth](2026-10-10-agent-team-orchestrators/H-theuth.md): theuth on the same axes, the Paperclip borrows that landed, and its behaviour in real use.
- [I-web](2026-10-10-agent-team-orchestrators/I-web.md): a 55-row landscape, user evidence, the native floor, the multi-agent evidence, business models.

**Superseded in an appendix:** appendix H's Q3 item 6 proposes routing reviews to the owner's manager desk. That contradicts Sky's ruling of 2026-10-06 18:38 (a review addressed to him lands in his Attention on its own), and §6.3 item 18 replaces it.

Clones were read at the HEADs in §2, in `~/Develop/spike-agent-teams/` (local only, deleted after the study). Knowledge entries:
- `facts/agent-team-orchestrators-landscape-2026-10-10`
- `facts/agent-team-orchestrators-source-recon-2026-10-10`
- `facts/theuth-team-features-inventory-vs-agent-team-axes-2026-10-10`
- `facts/theuth-borrow-list-from-agent-team-orchestrators-2026-10-10`
- `projects/theuth-agent-team-orchestrators-study-2026-10-10`

## 12. Amendments

- **2026-10-10 13:50.** main 2b2430ad (another session's endless-backoff fix) makes a failing endless-compactor node back off, doubling up to 30 min. It also moves the genai pin so Haiku 5+ gets adaptive thinking. This closes the incident in §4.2 and covers the endless compactor part of §6.1 item 9. The other background callers still lack a loop guard and a budget reservation: dreaming, the title model and decide.
- **2026-10-10 14:10, Revision 1 (Sky).** "We don't manage worktrees, this is not a coding agent, it may orchestrate them, but that's not its main role. A task does not equal a codebase change." Changes in this revision:
  - **Dropped:** §6.4 items 21 (engine-managed worktrees) and 24 (merge queue).
  - **Generalised:** item 22, from PR/CI feedback to external events routed to the task's owner; and item 17's evidence refs, from commits and CI runs to any durable record.
  - **Unchanged in substance:** item 23 is now framed as work trackers as task sources.
  - **Verdict:** "behind on the work-to-merge loop" is withdrawn in §1, and the coding planes' loop is now listed under do-not-borrow in §7.
  - **Questions:** 3 and 5 in §10 are answered.

  Recorded as `decisions/theuth-not-a-coding-agent-tasks-are-not-code-changes-2026-10-10`. The raw reports in the appendices are unchanged; E (Agent Orchestrator) and D (Symphony) still describe coding-specific loops, and only their general mechanisms apply.
