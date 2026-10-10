# AXES — open-source frameworks for driving a team of agents vs theuth (2026-10-10)

## The ask (Sky, 2026-10-10 13:01, verbatim)
> can you research the best and most popular open source frameworks like paperclip for driving a team of agents. then do a study comparing them to ~/Develop/theuth/ what can we learn from them, how can we improve theuth?

## Why this study exists
theuth already copied Paperclip once: on 2026-09-24/25 its agent roster, org chart, hiring wizard, tasks board,
desk protocol (checkout, disposition, wake, write cap), routines and budget pause were modelled on
paperclipai/paperclip (see the 2026-09-24 recon `~/.theuth/scratch/paperclip-recon.md` and designs 64/67/69/70).
This study widens the lens to the whole field of open-source tools that run a TEAM of agents, and asks what
theuth still lacks, what it does better, and what to borrow next. Sky wants techniques only — never copied code,
whatever the licence.

## The field, by tier (GitHub stars on 2026-10-10)
- **Tier 1 — control planes that drive existing harnesses as a team** (Paperclip's own category):
  paperclipai/paperclip 99.5k (MIT, TS) · multica-ai/multica 52.4k (Apache-2.0 + extra conditions, Go+TS) ·
  openai/symphony 27.6k (Apache-2.0, Elixir + SPEC.md) · gastownhall/gastown 18.3k (MIT, Go) + gastownhall/beads 27.8k
  (MIT, Go) · OrchestratorInc/agent-orchestrator 13.0k (Apache-2.0, Go+TS; ex-Composio). Parallel runners
  (vibe-kanban 28.3k, superset 15k, claude-squad 8.6k, emdash 6k, t3code 26.7k) are adjacent.
- **Tier 2 — teams inside one harness (Claude Code plugins/swarms)**: ruvnet/ruflo (ex claude-flow) 74.2k (MIT) ·
  Yeachan-Heo/oh-my-claudecode 39.8k (MIT) · bmad-code-org/BMAD-METHOD 54k · wshobson/agents 40k ·
  Claude Code's native Agent Teams.
- **Tier 3 — multi-agent SDK frameworks (build your own team in code)**: crewAIInc/crewAI 59.5k · microsoft/autogen 61.3k →
  microsoft/agent-framework 14k · langchain-ai/langgraph 43k · FoundationAgents/MetaGPT 70.8k · OpenBMB/ChatDev 34.5k ·
  agno 42.6k · openai-agents-python 30k · agentscope 33k · camel/owl/eigent.
- **Context — personal agents Paperclip hires as employees**: openclaw 391k, NousResearch/hermes-agent 252k.

Clones (shallow unless noted) live in `/Users/Sky.Moore/Develop/spike-agent-teams/<name>/`:
paperclip (blobless, full history; the 2026-09-24 recon was at 7b7c4d417 — 414 commits ago), multica, symphony,
gastown, beads, agent-orchestrator, ruflo, oh-my-claudecode, crewai, langgraph, agent-framework.

## theuth today (one paragraph; the `ours` report is the authority)
theuth (`/Users/Sky.Moore/Develop/theuth`, main b4e5833a) is a Rust meta-harness: one engine per machine joined in a
leaderless mesh, with a desktop app, TUI, web and Slack/Signal/Telegram channels. An *agent* is a file
`~/.theuth/agents/<name>.md` (title, `reports_to`, charter, model/effort/tools/mcp/budget/capabilities, paused) on a
roster (design 67); a *desk* is that agent's long-lived session, opened on the mesh hub. Work is a tasks board
(`~/.theuth/tasks.sqlite`: backlog/todo/doing/review/blocked/done/cancelled, one owner, reviewer, `done_when`, checkout,
dispositions, escalate up the reporting line, threads). Routines are schedule files (cron/every/at, `until` loops,
sensors). Subagents are native theuth sessions or external harnesses over ACP (omp, opencode), peers talk by
`send_message` (design 50; A2A at the boundary). Approvals follow `tools.policy`; the *double* (126) answers
approval cards as the owner. Plus: knowledge base + nightly dreaming (136), attention inbox/HUD, activity monitor
(155), budgets with auto-pause, sandbox VMs and remote nodes, autoresearch studies (140).

## The axes (every report uses these numbers and headings)
- **A1 Agent model & identity** — what an agent IS (record/file, role, instructions, skills, model/harness binding,
  adapters), hiring/onboarding, templates/catalogs, export/import, lifecycle states.
- **A2 Organisation & authority** — org chart, reporting lines, delegation and escalation, permissions/visibility,
  multi-company/workspace isolation, where humans sit (board, reviewer, member).
- **A3 Work model** — tasks/issues/tickets, states, assignment, dependencies/blockers, goals/projects/milestones,
  acceptance criteria, review, planning/decomposition, external trackers (Linear/GitHub/Jira) as the queue.
- **A4 Execution & runtime** — how agents run: wake/heartbeat/scheduling/triggers, concurrency limits,
  session resume, workspaces (worktrees/sandboxes/remote), harness integration (CLI, ACP, SDK, app-server, tmux),
  placement across machines, liveness/stall detection, retries, reconciliation after crashes, idempotency.
- **A5 Coordination between agents** — messaging/mail/comments/mentions, handoffs, shared ledgers, locks and
  checkout, merge queues/conflict handling, swarm/topology patterns, consensus or voting, who decides who works next.
- **A6 Human in the loop & governance** — approvals, interrupts, permission prompts, inbox/attention,
  notifications/mobile, review gates, audit log, policy and guardrails.
- **A7 Cost, budgets & runaway control** — budgets and caps, quota/rate-limit handling, loop and no-progress defences,
  kill switches, model routing for cost, cost attribution.
- **A8 Memory, knowledge & learning** — per-agent memory, shared knowledge, skills, context engineering,
  learning from outcomes, evals.
- **A9 Observability & UX** — what the human sees: dashboard, kanban, org chart, run transcripts, activity feed,
  metrics/traces, desktop/web/mobile/CLI/TUI, onboarding to first useful result.
- **A10 Extensibility & integration** — plugins, adapters, MCP, A2A, APIs/SDKs, webhooks, chat channels.
- **A11 Engineering, licence & traction** — stack, size, tests, release cadence, licence, stars/contributors/issue
  volume, business model, maturity; their weaknesses and failure modes as users report them.

## Map to the three questions (every report ends with this section)
- **Q1 What do they offer versus theuth?** — per axis: theirs / ours / gap (or "theuth ahead").
- **Q2 What can we learn / borrow?** — techniques only, each with its mechanism and cite, and the cost of being wrong.
- **Q3 How should theuth improve?** — concrete proposals ranked by value ÷ effort, each naming the theuth seam it
  lands in (crate, design number, UI surface) when known.

## Common rules for every child
- READ-ONLY. Do not build, install, run tests, start servers or modify any clone or the theuth tree. The machine is
  shared and busy (load ~15 on 16 cores). Reading, `rg`, `git log`, `wc` are fine.
- Cite code as `<repo>/<path>#L<a>-L<b>` (repo = the clone folder name, e.g. `multica/server/...#L10-L24`; theuth
  cites as `theuth/crates/...#L..`). Quote at most 6 lines at a time. Docs claims that the code contradicts: trust the
  code and say so.
- Traction numbers: use the GitHub MCP tools (`search_repositories`, `list_issues`, `search_issues`) — unauthenticated
  api.github.com search is rate-limited.
- Skeleton first: write the report's headings to your report file in your first minutes, then fill sections, saving as
  you go. Target 400–700 lines, ≥ 15 load-bearing cites re-opened by you at the end (list them in "Cites self-check"
  with held/corrected). Time budget ~25 minutes.
- Licence: techniques only, never copy code.
