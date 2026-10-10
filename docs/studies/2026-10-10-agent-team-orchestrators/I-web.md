# REPORT

# The field and its users: open-source frameworks for driving a team of agents (web survey, 2026-10-10)

Child `recon-web` (read-only). Brief: `briefs/2026-10-10/agent-teams/brief-web.md` plus `AXES.md`. Survey run 13:10–13:35 BST.

Where the numbers come from:
- Stars, licence, language and created date come from the GitHub MCP `search_repositories`.
- "Last commit" is the newest commit on the default branch, from the GitHub MCP `list_commits`.
- Issue rankings come from the GitHub MCP `search_issues` sorted by reactions.
- HN data comes from HN Algolia and the Firebase item API.
- Everything else comes from primary pages read with `read_url`.

All dates are as of 2026-10-10 unless stated.

## 0. Executive summary

1. **The field is huge and young.** Tier 1 is now crowded:
   - Big 2026 entrants in or next to it: Paperclip 99.5k★ (created 2026-03-02), stablyai/orca 88.9k★ (2026-03-17), Multica 52.4k★ (2026-01-13) and herdr 43.2k★ (2026-03-27).
   - Smaller ones: Symphony, Agent Orchestrator, omnigent, openrig, munder-difflin, Clawith, edict, cumora.
   - Method repos are larger still: obra/superpowers 297k★ and agency-agents 158.6k★, both from Oct 2025.
2. **Churn is high.**
   - Stalled on the default branch for 60+ days: Gas Town main (last commit 2026-07-23), get-shit-done, edict, golutra, MetaGPT, ChatDev.
   - Ended: AutoGen is in maintenance mode; Vibe Kanban's company shut down on 2026-04-10.
   - Five or more renames, every one confirmed by a GitHub redirect.
3. **Users of the control planes mostly ask for breadth and reach**, not more org-chart:
   - any model or harness (Paperclip #187 Ollama, 79 reactions; Copilot CLI, OpenRouter, Antigravity adapters);
   - chatting with or steering running agents (Paperclip #49);
   - reaching the team from Telegram or a phone (#2004; Orca and Superset ship mobile apps);
   - external trackers as the queue (Multica JIRA #1634; vibe-kanban GitLab #1697);
   - a company knowledge layer (Paperclip #1858);
   - usage and quota awareness (Gas Town #232; Orca's account switcher).
4. **The complaints are about trust and reliability:**
   - Gas Town spent users' credits on upstream work (#3649, 72 reactions; HN 253 pts).
   - Ruflo: "99% Theater, 1% Real, 5x Token Waste" audit; install and uninstall issues.
   - Vibe Kanban shipped telemetry on by default.
   - Paperclip: "most production bugs live on the adapter boundary (identity, wake context, liveness, token accounting)".
   - oh-my-claudecode crashed at 6 parallel agents; users asked it to sit on native Agent Teams (#455).
5. **The native floor rose a lot in 2026:**
   - Claude Code Agent Teams (experimental) and Claude Managed Agents.
   - Codex subagents on by default, plus app-server.
   - Cursor 3 Agents Window (2026-04-02) and unlimited parallel Cloud Agents.
   - Copilot cloud agent and Agent HQ "mission control" for third-party agents.
   - Devin, with up to 10 or unlimited concurrent sessions.
   - Jules, with 3/15/60 concurrent tasks.

   What is left for an outside plane: cross-vendor and cross-subscription teams, durable identity and memory across sessions, one attention inbox, local-first data, placement across machines, and governance.
6. **Does a team work? Yes for parallel, independently checkable work; poorly for dependent, write-heavy work.**
   - Anthropic 2025: +90.2% on a research eval at about 15× the tokens of chat.
   - MAST: 41–86.7% failure rates across 7 open multi-agent frameworks.
   - Anthropic 2026: a 45-agent swarm found 266 vulnerabilities vs 21; CEO-hierarchy prompts "did not make much difference"; 18 of 30 agents picked the same branch name.
   - Project Vend 2: forced procedures helped most; the CEO agent "wasn't much help".
   - Cursor 2026: planner/worker swarm reached 80% of a test suite in 4 h; "costs varied enormously".

   **The control plane's value is procedure, verification and cost control, more than hierarchy.**
7. **For theuth:**
   - Already ahead on chat channels, a knowledge layer, local-first storage, a mesh of machines, approvals plus the double, and budgets.
   - Biggest user-visible gaps:
     - a tracker bridge (Linear, GitHub or Jira as the queue);
     - proof-of-work on hand-back;
     - quota- and rate-limit-aware routing with a usage view;
     - a plan-approval gate;
     - diversity in fan-out to avoid correlated failures.
   - Ranked proposals are in §7.

## 0.1 Method and cites self-check

**Re-opened at the end with a keyword probe: 18 of 18 pages held, no corrections.**
- C1: the Claude Code agent-teams limitations ("One team per session", "No nested teams").
- C2: Anthropic "Patterns and problems…" ("266 vulnerabilities", "18 out of 30", "2.4 million job requests").
- C3: Cursor swarm post ("reached 80% in four hours", "costs varied enormously").
- C4: MAST HTML v3 ("41% to 86.7%", "+9.4%").
- C5: Cognition ("share full agent traces", "Actions carry implicit decisions").
- C6: Anthropic research system ("90.2%", "15× more tokens").
- C7: Project Vend 2 ("bureaucracy matters", "much help").
- C8: Vibe Kanban shutdown ("business model", "April 10, 2026").
- C11: Jules limits.
- C12: Devin pricing.
- C13: Codex subagents (`max_concurrent_threads_per_session`, "enable subagent workflows by default").
- C14: Cursor Cloud Agents ("as many agents as you want in parallel", "API pricing").
- C15: Claude Managed Agents multi-agent ("25 child threads", "one level deep").
- C16: andrew.ooo Paperclip review ("adapter boundary", "10.7K stars").
- C17: Ruflo audit gist ("99% Theater").
- C18: GitHub Agent HQ ("mission control", "Anthropic, OpenAI, Google, Cognition, xAI").
- C19: superset.sh ("Elastic License 2.0", "free tier").
- C20: onorca.dev ("Bring your own agent", "rate-limit resets").

**Other numbers re-checked:**
- Stars at 13:3x: paperclip 99,462; orca 88,947; multica 52,385; gastown 18,319.
- HN scores: 46902368 → 396, 46734302 → 403, 47785053 → 253, 49316271 → 200.
- Licences: Multica's hosted-service clause and oh-my-openagent's Sustainable Use Licence (SUL) text are present.
- Gas Town #3649 via `issue_read`: 72 reactions, closed 2026-04-14.

**Caveats:**
- Reddit was not used. openai.com and Medium sit behind Cloudflare challenges, so the Symphony launch post and Yegge's v1.0 post were cited by their HN items, not read.
- Reviews of "Paperclip AI" with a CEO agent "Zeus" at paperclip.ai are a **different product** ([kunalganglani.com, 2026-04-19](https://www.kunalganglani.com/blog/paperclip-ai-review-zero-human-company)). They are excluded.
- Issue reaction counts are small for most projects except Paperclip and vibe-kanban. Treat them as direction, not demand size.

## 1. Landscape (55 rows; ★ and dates from the GitHub MCP, 2026-10-10)

Tier key:
- **T1** = control plane that drives existing harnesses as a team.
- **PR** = parallel runner or agent environment.
- **T2** = team inside one harness (plugin, method or persona pack).
- **T3** = multi-agent SDK.
- **CTX** = personal agents that T1 planes "hire".

The 2026 newcomers that AXES missed are omnigent, munder-difflin, openrig, Clawith, edict, cumora, golutra, orca, herdr, cc-haha, sandcastle, oh-my-openagent, get-shit-done, agency-agents, superpowers and google/ax.

| project | tier | ★ | licence | lang | created | last commit | what it is |
|---|---|---|---|---|---|---|---|
| [paperclipai/paperclip](https://github.com/paperclipai/paperclip) | T1 | 99.5k | MIT | TypeScript | 2026-03-02 | 2026-10-10 | Company-shaped control plane: org chart, tickets, heartbeats, budgets, approvals over adapters (Claude Code, Codex, Hermes, OpenClaw…) |
| [multica-ai/multica](https://github.com/multica-ai/multica) | T1 | 52.4k | Apache-2.0 + conditions | Go | 2026-01-13 | 2026-10-10 | Issue board where agents are teammates; local daemon "runtimes" run the agent CLIs; review gates; cloud + self-host |
| [openai/symphony](https://github.com/openai/symphony) | T1 | 27.6k | Apache-2.0 | Elixir | 2026-02-26 | 2026-09-15 | SPEC.md + Elixir reference: watches a Linear board, one isolated Codex run per issue, proof of work, lands the PR |
| [gastownhall/gastown](https://github.com/gastownhall/gastown) | T1 | 18.3k | MIT | Go | 2025-12-16 | 2026-07-23 | Yegge's multi-agent workspace: Mayor/Witness/Deacon/Refinery/Polecat roles on tmux + Beads |
| [gastownhall/beads](https://github.com/gastownhall/beads) | T1 | 27.8k | MIT | Go | 2025-10-12 | 2026-10-10 | `bd`: git/Dolt-backed issue tracker as agent memory; Gas Town's state store |
| [OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator) | T1 | 13.0k | Apache-2.0 | Go | 2026-02-13 | 2026-10-10 | Plan-to-merge supervisor for fleets of coding agents in worktrees, 25 harnesses (ex-ComposioHQ) |
| [omnigent-ai/omnigent](https://github.com/omnigent-ai/omnigent) | T1 | 10.7k | Apache-2.0 | Python | 2026-06-11 | 2026-10-10 | Meta-harness: orchestrate Claude Code, Codex, Cursor, Pi and custom agents, swap harnesses |
| [HarnessMD/munder-difflin](https://github.com/HarnessMD/munder-difflin) | T1 | 8.6k | MIT | TypeScript | 2026-05-31 | 2026-10-09 | "An office" of Claude Code/Codex-like agents on your laptop, sandboxed |
| [mvschwarz/openrig](https://github.com/mvschwarz/openrig) | T1 | 6.6k | Apache-2.0 | TypeScript | 2026-04-01 | 2026-10-09 | Persistent teams of Claude Code/Codex/Pi agents with roles, shared context, owned work |
| [dataelement/Clawith](https://github.com/dataelement/Clawith) | T1 | 4.2k | Apache-2.0 | Python | 2026-03-03 | 2026-08-25 | "Your First AI Agents Company" (OpenClaw-based) |
| [cft0808/edict](https://github.com/cft0808/edict) | T1 | 17.0k | MIT | Python | 2026-02-23 | 2026-05-06 | OpenClaw multi-agent system modelled on the imperial "three departments, six ministries"; 9 agents, audit trail |
| [yetone/cumora](https://github.com/yetone/cumora) | T1 | 4.0k | MIT | TypeScript | 2026-08-17 | 2026-10-06 | Team chat where agents are first-class teammates (cloud or BYO Claude Code/Codex) |
| [golutra/golutra](https://github.com/golutra/golutra) | T1 | 3.9k | other | Rust | 2026-02-15 | 2026-05-12 | Rust platform turning Codex/Claude Code/OpenClaw into a team |
| [stablyai/orca](https://github.com/stablyai/orca) | PR | 88.9k | MIT | TypeScript | 2026-03-17 | 2026-10-10 | Agent development environment (ADE) for a fleet of parallel agents; BYO subscription; desktop + mobile; usage/rate-limit tracker, account switcher |
| [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban) | PR | 28.3k | Apache-2.0 | Rust | 2025-06-14 | 2026-09-19 | Kanban for agents in worktrees; company shut down 2026-04-10, now community-maintained |
| [pingdotgg/t3code](https://github.com/pingdotgg/t3code) | PR | 26.7k | MIT | TypeScript | 2026-02-08 | 2026-10-10 | GUI control surface over Codex app-server, Claude SDK, OpenCode, Pi… |
| [superset-sh/superset](https://github.com/superset-sh/superset) | PR | 15.0k | Elastic Licence 2.0 (ELv2) | TypeScript | 2025-10-21 | 2026-10-10 | "IDE for the agents era" (YC P26): 100+ parallel agents, worktrees, mobile |
| [herdrdev/herdr](https://github.com/herdrdev/herdr) | PR | 43.2k | Apache-2.0 | Rust | 2026-03-27 | 2026-10-08 | Terminal runtime/multiplexer that hosts coding agents (joined YC 2026-08) |
| [NanmiCoder/cc-haha](https://github.com/NanmiCoder/cc-haha) | PR | 14.9k | MIT | TypeScript | 2026-03-31 | 2026-10-10 | Local-first Claude Code desktop: multi-agent, worktrees, remote via WeChat/Feishu/DingTalk |
| [smtg-ai/claude-squad](https://github.com/smtg-ai/claude-squad) | PR | 8.6k | AGPL-3.0 | Go | 2025-03-09 | 2026-08-20 | tmux + worktree manager for several terminal agents |
| [mattpocock/sandcastle](https://github.com/mattpocock/sandcastle) | PR | 8.3k | MIT | TypeScript | 2026-03-17 | 2026-10-08 | TypeScript library: `sandcastle.run()` orchestrates sandboxed coding agents |
| [generalaction/emdash](https://github.com/generalaction/emdash) | PR | 5.9k | Apache-2.0 | TypeScript | 2025-08-28 | 2026-10-10 | Agentic development environment (YC W26): parallel agents in worktrees |
| [dagger/container-use](https://github.com/dagger/container-use) | PR | 4.1k | Apache-2.0 | Go | 2025-05-23 | 2026-08-12 | Container environments so several agents can work safely in parallel |
| [RunMaestro/Maestro](https://github.com/RunMaestro/Maestro) | PR | 3.4k | AGPL-3.0 | TypeScript | 2025-11-23 | 2026-10-09 | "Agent Orchestration Command Center" desktop app |
| [nimbalyst/nimbalyst](https://github.com/nimbalyst/nimbalyst) | PR | 1.9k | MIT | TypeScript | 2025-10-30 | 2026-10-09 | Visual workspace for parallel Claude Code/Codex/OpenCode (ex-Crystal) |
| [stravu/crystal](https://github.com/stravu/crystal) | PR | 3.1k | MIT | TypeScript | 2025-06-05 | 2026-02-26 | Old repo; description reads "Crystal is now Nimbalyst" |
| [ruvnet/ruflo](https://github.com/ruvnet/ruflo) | T2 | 74.2k | MIT | TypeScript | 2025-06-02 | 2026-10-10 | Claude Code swarm/hive-mind platform, hundreds of MCP tools (ex claude-flow) |
| [Yeachan-Heo/oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode) | T2 | 39.8k | MIT | TypeScript | 2026-01-09 | 2026-10-06 | "Teams-first" Claude Code plugin: `team` skill fans out worker lanes, HUD |
| [code-yeongyu/oh-my-openagent](https://github.com/code-yeongyu/oh-my-openagent) | T2 | 69.9k | SUL 1.0 | TypeScript | 2025-12-03 | 2026-10-10 | OmO agent suite for OpenCode/Pi, "ultrawork" mode (ex oh-my-opencode) |
| [bmad-code-org/BMAD-METHOD](https://github.com/bmad-code-org/BMAD-METHOD) | T2 | 54.0k | MIT (+ notice) | Python | 2025-04-13 | 2026-10-06 | Agile persona method (analyst/PM/architect/dev/QA) as prompts and workflows |
| [gsd-build/get-shit-done](https://github.com/gsd-build/get-shit-done) | T2 | 64.3k | MIT | JavaScript | 2025-12-14 | 2026-05-31 | Meta-prompting + spec-driven context system for Claude Code |
| [wshobson/agents](https://github.com/wshobson/agents) | T2 | 40.3k | MIT | Python | 2025-07-24 | 2026-10-04 | Multi-harness plugin marketplace of subagents and skills |
| [msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) | T2 | 158.6k | MIT | Shell | 2025-10-13 | 2026-10-07 | Catalogue of specialist agent personas for 20 tools |
| [obra/superpowers](https://github.com/obra/superpowers) | T2 | 297.1k | MIT | Shell | 2025-10-09 | 2026-10-10 | Skills framework and method (brainstorm → plan → subagent-driven development) |
| [SuperClaude-Org/SuperClaude_Framework](https://github.com/SuperClaude-Org/SuperClaude_Framework) | T2 | 23.9k | MIT | Python | 2025-06-22 | 2026-09-27 | Commands/personas configuration framework for Claude Code |
| [crewAIInc/crewAI](https://github.com/crewAIInc/crewAI) | T3 | 59.5k | MIT | Python | 2023-10-27 | 2026-10-09 | Role-playing crews + Flows; AMP enterprise platform |
| [microsoft/autogen](https://github.com/microsoft/autogen) | T3 | 61.3k | CC-BY-4.0 (per GitHub) | Python | 2023-08-18 | 2026-04-06 | README: maintenance mode, migrate to Agent Framework |
| [microsoft/agent-framework](https://github.com/microsoft/agent-framework) | T3 | 14.0k | MIT | Python | 2025-04-28 | 2026-10-09 | AutoGen + Semantic Kernel successor: agents and graph workflows, Python/.NET |
| [ag2ai/ag2](https://github.com/ag2ai/ag2) | T3 | 5.0k | Apache-2.0 | Python | 2024-11-11 | 2026-10-08 | Community continuation, "formerly AutoGen" |
| [langchain-ai/langgraph](https://github.com/langchain-ai/langgraph) | T3 | 43.0k | MIT | Python | 2023-08-09 | 2026-10-10 | Graph runtime for stateful agents: checkpoints, interrupts, subgraphs |
| [langchain-ai/deepagents](https://github.com/langchain-ai/deepagents) | T3 | 30.1k | MIT | Python | 2025-07-27 | 2026-10-10 | "Batteries-included agent harness" (planning, subagents, files) on LangGraph |
| [FoundationAgents/MetaGPT](https://github.com/FoundationAgents/MetaGPT) | T3 | 70.8k | MIT | Python | 2023-06-30 | 2026-01-21 | Standard-operating-procedure (SOP) roles: an "AI software company" |
| [OpenBMB/ChatDev](https://github.com/OpenBMB/ChatDev) | T3 | 34.5k | Apache-2.0 | Python | 2023-08-28 | 2026-07-24 | Virtual software company, ChatDev 2.0 |
| [agno-agi/agno](https://github.com/agno-agi/agno) | T3 | 42.6k | Apache-2.0 | Python | 2022-05-04 | 2026-10-10 | Agents, teams, workflows + AgentOS runtime |
| [openai/openai-agents-python](https://github.com/openai/openai-agents-python) | T3 | 29.9k | MIT | Python | 2025-03-11 | 2026-10-08 | Agents SDK: handoffs, guardrails, sessions, tracing |
| [openai/swarm](https://github.com/openai/swarm) | T3 | 22.0k | MIT | Python | 2024-02-22 | 2026-04-15 | Educational; superseded by the Agents SDK |
| [agentscope-ai/agentscope](https://github.com/agentscope-ai/agentscope) | T3 | 33.0k | Apache-2.0 | Python | 2024-01-12 | 2026-10-10 | Alibaba multi-agent framework with studio/observability |
| [camel-ai/camel](https://github.com/camel-ai/camel) | T3 | 17.8k | Apache-2.0 | Python | 2023-03-17 | 2026-10-05 | Role-playing multi-agent research framework (OWL and Eigent build on it) |
| [eigent-ai/eigent](https://github.com/eigent-ai/eigent) | T3 | 15.5k | Apache-2.0 | TypeScript | 2025-07-29 | 2026-10-09 | Open "cowork" desktop on CAMEL workforce |
| [google/adk-python](https://github.com/google/adk-python) | T3 | 21.8k | Apache-2.0 | Python | 2025-04-01 | 2026-10-10 | Agent Development Kit: multi-agent, evals, A2A |
| [google/ax](https://github.com/google/ax) | T3 | 13.4k | Apache-2.0 | Go | 2026-03-30 | 2026-09-27 | "Google's open agentic orchestration runtime" (agentexecutor.io) |
| [mastra-ai/mastra](https://github.com/mastra-ai/mastra) | T3 | 28.7k | Apache-2.0 + ee/ | TypeScript | 2024-08-06 | 2026-10-10 | TypeScript agent framework: workflows, memory, evals |
| [TransformerOptimus/SuperAGI](https://github.com/TransformerOptimus/SuperAGI) | T3 | 17.7k | MIT | Python | 2023-05-13 | 2025-01-22 | Autonomous-agent framework; dormant |
| [openclaw/openclaw](https://github.com/openclaw/openclaw) | CTX | 391.6k | MIT | TypeScript | 2025-11-24 | 2026-10-10 | Personal agent; Paperclip's typical "employee" |
| [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) | CTX | 252.4k | MIT | Python | 2025-07-22 | 2026-10-10 | Self-improving personal agent (skills, memory); has a Paperclip adapter |

Notes on the table:
- **Licence checks:**
  - [Multica LICENSE](https://github.com/multica-ai/multica/blob/main/LICENSE) is "Apache-2.0 subject to the additional conditions in Part I".
  - [superset LICENSE.md](https://github.com/superset-sh/superset/blob/main/LICENSE.md) is "Elastic License 2.0 (ELv2)".
  - [oh-my-openagent LICENSE.md](https://github.com/code-yeongyu/oh-my-openagent/blob/main/LICENSE.md) is the "Sustainable Use License" v1.0: internal or non-commercial use only.
  - [mastra LICENSE.md](https://github.com/mastra-ai/mastra/blob/main/LICENSE.md) is Apache-2.0, except any `ee/` directory, which is under an enterprise licence.
- **Other trackers seen, not tabled:** iOfficeAI/AionUi 33.4k (cowork UI over 20+ CLIs), slopus/happy 24.1k (mobile client), winfunc/opcode 22.4k (AGPL), siteboon/claudecodeui 14.0k (AGPL), strands-agents/harness-sdk 8.8k (ex sdk-python), alvinunreal/oh-my-opencode-slim 9.4k, NousResearch/hermes-paperclip-adapter 2.0k, gsxdsm/awesome-paperclip 950. All from `search_repositories`, 2026-10-10.

## 2. Momentum

### 2.1 Rising fast in 2026 (created ≥ 2026-01, > 10k★, in or next to the team field)

| project | created | ★ 2026-10-10 | note |
|---|---|---|---|
| Paperclip | 2026-03-02 | 99.5k | 96.9k and "+10.7K stars in the last seven days" on 2026-10-04 ([andrew.ooo](https://andrew.ooo/posts/paperclip-review-open-source-ai-agent-company-orchestration/)); stable release `v2026.1001.0` |
| stablyai/orca | 2026-03-17 | 88.9k | Largest 2026 runner; mobile companion, Claude/Codex usage and rate-limit view ([onorca.dev](https://onorca.dev)) |
| Multica | 2026-01-13 | 52.4k | Cloud + self-host; Go server + daemon |
| herdr | 2026-03-27 | 43.2k | HN 404 pts (2026-07-02, [item](https://news.ycombinator.com/item?id=48756578)); "joining Y Combinator. The runtime stays open" 281 pts (2026-08-06, [item](https://news.ycombinator.com/item?id=49201003)) |
| oh-my-claudecode | 2026-01-09 | 39.8k | Teams-first plugin |
| openai/symphony | 2026-02-26 | 27.6k | Spec-first; issues disabled |
| t3code | 2026-02-08 | 26.7k | — |
| cft0808/edict | 2026-02-23 | 17.0k | Stalled since 2026-05-06 |
| NanmiCoder/cc-haha | 2026-03-31 | 14.9k | — |
| google/ax | 2026-03-30 | 13.4k | — |
| Agent Orchestrator | 2026-02-13 | 13.0k | — |
| omnigent | 2026-06-11 | 10.7k | — |

- Next to the field: HKUDS/nanobot 48.9k (2026-02-01, personal agent) and codewhale-hq/Codewhale 41.1k (2026-01-19, agent engine).
- Late-2025 births that still dominate star charts: obra/superpowers 297k (2025-10-09), agency-agents 158.6k (2025-10-13), oh-my-openagent 69.9k (2025-12-03), get-shit-done 64.3k (2025-12-14), beads 27.8k (2025-10-12) and gastown 18.3k (2025-12-16).
- **Signal:** method, skill and persona packs out-star every control plane. Users adopt *prompts and procedures* far more readily than *infrastructure*.

### 2.2 Stalled: no default-branch commit for 60+ days at 2026-10-10

**Gas Town:**
- `main` last commit 2026-07-23 (79 days).
- Last release [v1.2.1](https://github.com/gastownhall/gastown/releases/tag/v1.2.1), 2026-06-06.
- The `integration/gt-1-2-*` branches stop at 2026-05-22 to 2026-07-06 (GitHub MCP `list_branches`/`list_commits`).
- `pushed_at` still shows 2026-10-08, so something is pushing, but nothing lands on main.
- Beads is still active (2026-10-10).

**Others, with last commit and days idle:**
- get-shit-done: 2026-05-31 (132 d) at 64k★.
- edict: 2026-05-06 (157 d).
- golutra: 2026-05-12 (151 d).
- ChatDev: 2026-07-24 (78 d).
- MetaGPT: 2026-01-21 (262 d).
- AutoGen: 2026-04-06 (maintenance).
- openai/swarm: 2026-04-15 (superseded).
- SuperAGI: 2025-01-22.
- crystal: 2026-02-26 (renamed).

Borderline: container-use (2026-08-12, 59 d), claude-squad (2026-08-20, 51 d), Clawith (2026-08-25, 46 d).

### 2.3 Pivots, renames, shutdowns

Every old path below resolves to the new repo through the GitHub API (`get_file_contents` on the old owner/name returned the new README, 2026-10-10).
- **claude-flow → ruflo.** README badges still point at `ruvnet/claude-flow`; homepage is Cognitum.One.
- **ComposioHQ/agent-orchestrator → OrchestratorInc/agent-orchestrator.** Now branded "Orchestrator.inc", 25 harnesses ([orchestrator.inc](https://orchestrator.inc)).
- **stravu/crystal → nimbalyst/nimbalyst.** Description: "Crystal is now Nimbalyst".
- **steveyegge/gastown → gastownhall/gastown**; Beads moved the same way.
- **code-yeongyu/oh-my-opencode → oh-my-openagent.** "OmO ❤️ Pi"; now targets Pi as well as OpenCode.
- **ogulcancelik/herdr → herdrdev/herdr**, then into YC.
- **strands-agents/sdk-python → strands-agents/harness-sdk.**
- **AutoGen → maintenance mode.** README: "will not receive new features… New users should start with Microsoft Agent Framework" ([autogen README](https://github.com/microsoft/autogen)). AG2 continues as "formerly AutoGen".
- **Vibe Kanban.** bloop shut down 2026-04-10: "the vast majority are free users and we couldn't find a business model". Remote services were removed after 30 days and the app went fully local ([vibekanban.com/blog/shutdown](https://www.vibekanban.com/blog/shutdown)).
- **GitHub "Copilot coding agent" → "Copilot cloud agent"** in the docs ([docs.github.com](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent), read 2026-10-10).

## 3. User evidence per major project

Reaction totals are from `search_issues` sorted by reactions, open issues unless marked; HN figures are points/comments.

### Paperclip (99.5k★, 6.3k open issues + PRs)

**Asks (open issues by reactions):**
- [#187](https://github.com/paperclipai/paperclip/issues/187) local LLMs via Ollama: 79 reactions, 34 comments, 2026-03-07.
- [#49](https://github.com/paperclipai/paperclip/issues/49) "Chat with agents much needed feature": 49, 2026-03-05.
- [#2092](https://github.com/paperclipai/paperclip/issues/2092) Copilot CLI adapter: 44.
- [#1068](https://github.com/paperclipai/paperclip/issues/1068) OpenRouter: 35.
- [#6559](https://github.com/paperclipai/paperclip/issues/6559) Antigravity CLI: 27, 2026-05-22.
- [#1858](https://github.com/paperclipai/paperclip/issues/1858) "Company-level knowledge layer (wiki, role context, shared strategy docs)": 26.
- [#2004](https://github.com/paperclipai/paperclip/issues/2004) "talk to the CEO via telegram/whatsApp": 19.

**Praise:**
- The framing: "OpenClaw is an employee, Paperclip is the company" ([paperclip.ing](https://paperclip.ing) testimonials).
- Governance: "atomic task checkout with execution locks, budget hard-stops, revisioned config with rollback, approval stages, multi-org isolation" ([andrew.ooo, 2026-10-04](https://andrew.ooo/posts/paperclip-review-open-source-ai-agent-company-orchestration/)).

**Complaints:**
- "Most production bugs live on the adapter boundary (identity, wake context, liveness, token accounting)."
- "One operator who ran a 13-agent org for six weeks had to write his own adapter package to get a loop to close unattended" (same review).
- Gladiator's README: "Paperclip can't parse Hermes token output" ([gladiator](https://github.com/runtimenoteslabs/gladiator), 2026-03).

**Traction off GitHub:** weak on HN. "Paperclip: Open-source orchestration for zero-human companies" got 6 pts (2026-03-06, [item](https://news.ycombinator.com/item?id=47277096)). The review notes discussion lives on Discord and in the issue tracker.

### Multica (52.4k★)

**Asks:**
- [#1634](https://github.com/multica-ai/multica/issues/1634) JIRA integration: 24.
- [#1943](https://github.com/multica-ai/multica/issues/1943) Workflow Orchestration: 20 reactions, 18 comments.
- [#1014](https://github.com/multica-ai/multica/issues/1014) OIDC: 19.
- [#2456](https://github.com/multica-ai/multica/issues/2456) migrate Cloud → self-hosted: 13.
- [#4490](https://github.com/multica-ai/multica/issues/4490) plugin system: 12.
- [#1900](https://github.com/multica-ai/multica/issues/1900) multiple runtimes per agent: 11.

**The sharpest critique** is [#815](https://github.com/multica-ai/multica/issues/815), "Multica still manages AI the way it manages people" (18 reactions, 2026-04-13):
- Praise first: "the runtime abstraction is clean… much more polished than most projects in this space".
- Then the problem: "who decomposes the large task? who decides the next stage? who performs handoffs? who enforces approval / acceptance gates?"

**Reviews:**
- "Code never passes through multica's servers"; gap: "Manual skill curation" ([agentconn, 2026-04-13](https://agentconn.com/blog/multica-open-source-managed-agents-platform-review/)).
- Head-to-head with Paperclip and Claude Managed Agents: [flowtivity, 2026-04-25](https://flowtivity.ai/blog/multica-vs-paperclip-vs-claude-managed-agents-comparison/).

**HN:** no Multica story above 5 pts (Algolia, 2026-10-10).

### Gas Town + Beads

**HN:**
- "Welcome to Gas Town": 354/224 (2026-01-01, [item](https://news.ycombinator.com/item?id=46458936)).
- Maggie Appleton's analysis: 403/433 (2026-01-23, [item](https://news.ycombinator.com/item?id=46734302)).
- "Gas Town Decoded": 219/234 (2026-01-14).
- "From Clown Show to v1.0": 113/164 (2026-04-14, [item](https://news.ycombinator.com/item?id=47770124)).
- "Welcome to the Wasteland": 68/96 (2026-03-04).

**Gist** ([maggieappleton.com/gastown](https://maggieappleton.com/gastown)):
- "Entirely vibecoded… burning through thousands of dollars a month in API costs".
- "Design becomes the limiting factor".
- A user (quoted): "the mayor is dumb as rocks… the polecats seem intent on wreaking as much chaos".
- A critic (quoted): "the number of overlapping and ad hoc concepts… is overwhelming".
- HN pushback: "a big fun experiment" ([46735034](https://news.ycombinator.com/item?id=46735034)) vs "production software this way?" over lost design invariants ([47772431](https://news.ycombinator.com/item?id=47772431)).

**Trust incident** ([#3649](https://github.com/gastownhall/gastown/issues/3649), 72 reactions, closed 2026-04-14; HN 253/127, [item](https://news.ycombinator.com/item?id=47785053)):
- Release formulas made users' installs work on upstream issues using "your Claude credits / usage… and your GitHub account".
- The ask: "moved to opt-in only".
- Top HN reply: "everything about the whole stack is designed to burn tokens" ([47785763](https://news.ycombinator.com/item?id=47785763)).

**Other asks:**
- [#764](https://github.com/gastownhall/gastown/issues/764) "The Fragility of Beads in Gas Town" (15): "When an issue tracker loses a ticket, it's annoying. When operational infrastructure loses state, the factory explodes."
- [#10](https://github.com/gastownhall/gastown/issues/10) agent-agnostic engine with sandboxing (13).
- [#232](https://github.com/gastownhall/gastown/issues/232) "rate-limit aware" instance swapping (7).
- [#335](https://github.com/gastownhall/gastown/issues/335) models per role (6).

**Beads:**
- Low reaction counts. Top: [#3924](https://github.com/gastownhall/beads/issues/3924) v1.0.4 binary missing `codex-hook` (8); [#4469](https://github.com/gastownhall/beads/issues/4469) "cut a Beads release from main".
- HN "I replaced Beads with a faster, simpler Markdown-based task tracker": 84/51 (2026-01-04, [item](https://news.ycombinator.com/item?id=46487580)). One reply: "beads is a task tracker and what we needed was a spec tracker".

### Symphony (27.6k★; issues disabled per `has_issues: false`)

- The README calls it "a low-key engineering preview for testing in trusted environments" that "works best in codebases that have adopted harness engineering".
- Hand-back is a proof-of-work bundle: "CI status, PR review feedback, complexity analysis, and walkthrough videos".
- Its first install option is "Tell your favorite coding agent to build Symphony" from SPEC.md ([README](https://github.com/openai/symphony)).
- HN: 25 pts (2026-03-04, [item](https://news.ycombinator.com/item?id=47252045)) and 27 pts for the openai.com post (2026-04-27, [item](https://news.ycombinator.com/item?id=47924920)).
- PRs as a proxy for asks:
  - [#60](https://github.com/openai/symphony/pull/60) "Persist per-issue token usage" (open since 2026-04-21);
  - [#66](https://github.com/openai/symphony/pull/66) "Surface input-blocked Symphony sessions" (closed unmerged);
  - [#23](https://github.com/openai/symphony/pull/23) dashboard control-byte injection fix.

### Agent Orchestrator (13.0k★)

- Reaction counts are tiny (3 at most).
- Asks:
  - [#5700](https://github.com/OrchestratorInc/agent-orchestrator/issues/5700) label-based eligibility for tracker intake;
  - [#3853](https://github.com/OrchestratorInc/agent-orchestrator/issues/3853) remote workspaces: run agents on a VM through a daemon;
  - [#5597](https://github.com/OrchestratorInc/agent-orchestrator/issues/5597) "let an orchestrator read a worker session's output";
  - [#2195](https://github.com/OrchestratorInc/agent-orchestrator/issues/2195) per-role environment profile (MCP, plugins, system prompt).
- Pitch: "Stop babysitting agents. Start merging real work." ([orchestrator.inc](https://orchestrator.inc)).

### Ruflo (74.2k★)

- Top issues (2025):
  - [#659](https://github.com/ruvnet/ruflo/issues/659) "A Note on Alpha Software, Expectations, and Open Source Reality" (22);
  - [#662](https://github.com/ruvnet/ruflo/issues/662) SQLite "no column named swarm_name" (22 reactions, 18 comments);
  - [#624](https://github.com/ruvnet/ruflo/issues/624) "cant get anything to work" (12);
  - [#694](https://github.com/ruvnet/ruflo/issues/694) "how to uninstall?" (12);
  - [#442](https://github.com/ruvnet/ruflo/issues/442) SQLite storage always falls back to in-memory.
- Audit gist (2026-04-04, [roman-rr](https://gist.github.com/roman-rr/ed603b676af019b8740423d2bb8e4bf6)): "300+ MCP Tools… 99% Theater, 1% Real, 5x Token Waste"; "The wire connecting them is missing".
- No HN story above 10 pts.

### oh-my-claudecode (39.8k★; 11 open issues)

- [#455](https://github.com/Yeachan-Heo/oh-my-claudecode/issues/455) (10 reactions, closed 2026-02-06): "Running 6 agents in parallel frequently causes interruptions/crashes" vs native Agent Team "16 agents in parallel with no interruptions". The ask was to sit on the native feature.
- Others:
  - [#167](https://github.com/Yeachan-Heo/oh-my-claudecode/issues/167) "I have a better one with fewer stars" (9);
  - [#3497](https://github.com/Yeachan-Heo/oh-my-claudecode/issues/3497) npm published but main not tagged (15 comments);
  - [#1414](https://github.com/Yeachan-Heo/oh-my-claudecode/issues/1414) HUD shows API 429 because of polling.
- The site now describes a `team` skill, with "`omc team` for legacy tmux/CLI runs" ([oh-my-claudecode.dev](https://oh-my-claudecode.dev)).

### CrewAI (59.5k★)

- [#6043](https://github.com/crewAIInc/crewAI/issues/6043) "Memory write guards for multi-agent crews: prevent cross-agent memory poisoning" (31 comments, 2026-06-04).
- [#7473](https://github.com/crewAIInc/crewAI/issues/7473) A2A artifacts reassembled to corrupted text.
- [#7561](https://github.com/crewAIInc/crewAI/issues/7561) stamp agent outputs to a transparency log.

### LangGraph (43.0k★)

- [#3716](https://github.com/langchain-ai/langgraph/issues/3716) checkpoint-postgres `OperationalError` (12 reactions, 53 comments).
- [#5672](https://github.com/langchain-ai/langgraph/issues/5672) "Run Cancellation Causes Loss of Streamed State Not Yet Persisted as a Checkpoint" (52 comments).
- [#7692](https://github.com/langchain-ai/langgraph/issues/7692) asyncpg driver for checkpoints (2026-05-03).
- Durability of state is the recurring pain.

### vibe-kanban (28.3k★)

- Asks:
  - [#1697](https://github.com/BloopAI/vibe-kanban/issues/1697) self-hosted GitLab (26);
  - [#1946](https://github.com/BloopAI/vibe-kanban/issues/1946) "Ralph Wiggum mode" (loop until done, 20);
  - [#3396](https://github.com/BloopAI/vibe-kanban/issues/3396) self-hosted projects and better export (19);
  - [#765](https://github.com/BloopAI/vibe-kanban/issues/765) configurable automatic worktree cleanup (19);
  - [#2509](https://github.com/BloopAI/vibe-kanban/issues/2509) "please keep the Kanban board" (15).
- Show HN, 195/132 (2025-07-11):
  - "analytics appear to default to enabled… harvesting email addresses and github usernames" ([44535243](https://news.ycombinator.com/item?id=44535243));
  - "compounded false affirmatives" as you scale ([44533293](https://news.ycombinator.com/item?id=44533293)).

### Field-wide voices

**Ask HN "Are you using an agent orchestrator to write code?"** (41/60, 2026-02-12, [item](https://news.ycombinator.com/item?id=46993479)):
- "People who run 15 agents… could probably use 1 or 2 and a better multi-page prompt" ([46999562](https://news.ycombinator.com/item?id=46999562)).
- "Claude Code with skills and subagents is plenty… The bottleneck… is how quickly you can integrate and deploy" ([46999178](https://news.ycombinator.com/item?id=46999178)).

**On native Agent Teams** (396/224, 2026-02-05, [item](https://news.ycombinator.com/item?id=46902368)):
- "Holding off on learning any of the tools… so obvious that it'll be built natively" ([46904718](https://news.ycombinator.com/item?id=46904718)).
- "I wonder if my $20/mo subscription will last 10 minutes" ([46903895](https://news.ycombinator.com/item?id=46903895)).

**Superset launch** (108/135, 2026-05-22, [item](https://news.ycombinator.com/item?id=48236770)):
- Per-worktree infrastructure spin-up is the open problem ([48244816](https://news.ycombinator.com/item?id=48244816)).
- "behind a login wall… tries to download its own OpenCode" ([48240446](https://news.ycombinator.com/item?id=48240446)).

## 4. The native floor (what the big harnesses ship for teams, read 2026-10-10)

| vendor | team features | limits and shape | source |
|---|---|---|---|
| Claude Code | Subagents; **Agent Teams**: lead + teammates, shared task list (pending/in progress/completed, dependencies, self-claim), `SendMessage` mailbox, plan-mode approval, hooks, split panes | Experimental (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`). No resume of in-process teammates; "Task status can lag"; one team per session; no nested teams; lead fixed; per-teammate permissions only after spawn; teammate prompts surface in the lead; agent messages flagged as "not from you"; "significantly more tokens" | [code.claude.com/docs/en/agent-teams](https://code.claude.com/docs/en/agent-teams) |
| Claude Managed Agents | Hosted harness: agent defined once and referenced by ID; cloud or self-hosted sandbox; subagents + dynamic workflows | "Delegation is one level deep… at most 25 child threads" | [platform.claude.com …/managed-agents/multi-agent](https://platform.claude.com/docs/en/managed-agents/multi-agent) |
| OpenAI Codex | Subagents on by default; custom TOML agents with model/effort; agent threads you can open | `agents.max_concurrent_threads_per_session`; inherits sandbox and approval mode; "Be more careful with parallel write-heavy workflows" | [learn.chatgpt.com …/subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents) |
| Codex app-server | JSON-RPC protocol behind the IDE extension: auth, history, approvals, streamed events; remote TUI over `ws://` | Open source in `codex-rs/app-server` | [learn.chatgpt.com/docs/app-server](https://learn.chatgpt.com/docs/app-server) |
| Cursor | **Agents Window** (Cursor 3, 2026-04-02) across local, worktree, cloud and SSH ([agentpatterns.ai](https://www.agentpatterns.ai/tools/cursor/agents-window/)). **Cloud Agents**: "as many agents as you want in parallel", own VM, computer use, MCP. Plus Automations, Bugbot, PR routing & approval, mobile, self-hosted machines, Slack/Teams/Jira/Linear/GitHub, SDK, ACP CLI | Cloud agents "charged at API pricing" | [cursor.com/docs/cloud-agent](https://cursor.com/docs/cloud-agent) |
| GitHub Copilot | "Copilot cloud agent" started from the agents panel, an issue or a PR; steer mid-run; session logs; scheduled/event Automations; Agentic Workflows (Actions in Markdown); Copilot Memory; remote control of CLI sessions. **Agent HQ** (2025-10-28): "mission control" over agents from "Anthropic, OpenAI, Google, Cognition, xAI" | Inside the paid Copilot subscription | [docs.github.com](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent), [github.blog](https://github.blog/news-insights/company-news/welcome-home-agents/) |
| Devin | Devin Cloud agents; Slack/Teams/Linear/Jira/GitHub | Free / Pro $20 / Max $200 / Teams $80 + $40 per seat; **concurrent sessions "Up to 10"** (Pro/Max), unlimited (Teams/Enterprise) | [devin.ai/pricing](https://devin.ai/pricing) |
| Google Jules | Async cloud coding agent; API since 2025-10 ([HN 45466588](https://news.ycombinator.com/item?id=45466588)) | Daily tasks 15/100/300; **concurrent 3/15/60** (free/Pro/Ultra); paid plans only for @gmail.com accounts | [jules.google/docs/usage-limits](https://jules.google/docs/usage-limits/) |

**What this leaves for an outside control plane:**

1. **Cross-vendor, cross-subscription teams.** Every native team is single-vendor. Users run Claude, Codex, Cursor and Gemini side by side and juggle quotas: Orca's "hot-swap Codex accounts", Gas Town #232.
2. **Durable identity and memory across sessions.** Claude teams can't resume teammates and allow one team per session. Managed Agents allow only one level of delegation.
3. **One attention surface over many agents and vendors, on phone and chat apps.** Paperclip #2004; cc-haha's WeChat/Feishu bridges.
4. **Local-first data ownership.** vibe-kanban's shutdown and Multica #2456 show the risk of the cloud dependency.
5. **Placement across machines** (laptop, VMs, remote): AO #3853.
6. **Governance across vendors:** budgets, policy, audit.

The risk is the middle being absorbed: users already defer to native features (oh-my-claudecode #455; HN 46904718), and GitHub's Agent HQ is itself a cross-vendor plane.

## 5. Does a team of agents work? The evidence

| source (date) | setup | numbers | lesson |
|---|---|---|---|
| Anthropic, [research system](https://www.anthropic.com/engineering/built-multi-agent-research-system) (2025-06-13) | Opus 4 lead + Sonnet 4 subagents | +90.2% vs single Opus 4 on an internal research eval; agents use ≈4× the tokens of chat, multi-agent ≈15×; token usage explains 80% of BrowseComp variance | Wins on breadth-first, parallel search when the task's value pays for the tokens |
| Cognition, ["Don't Build Multi-Agents"](https://cognition.ai/blog/dont-build-multi-agents) (2025-06-12; HN 123/89) | Essay | Principles: "Share context, and share full agent traces"; "Actions carry implicit decisions, and conflicting decisions carry bad results" | Parallel writers with partial context conflict |
| MAST, [arXiv 2503.13657](https://arxiv.org/abs/2503.13657) (2025-03-17, v3) | 1,600+ traces, 7 frameworks, κ = 0.88 | 14 failure modes in 3 groups: system design, inter-agent misalignment, task verification. **41% to 86.7% failure rate** on 7 open multi-agent systems; making ChatDev's CEO have the final say gave **+9.4%** success ([HTML](https://arxiv.org/html/2503.13657v3)) | Most failures come from specification and verification, not raw capability |
| Anthropic, ["Patterns and problems in emerging multiagent systems"](https://www.anthropic.com/research/multiagent-systems) (2026-08-13; HN 200/138) | 45 agents with VMs and a shared forum, vs independent agents | Swarm: **266 vulns over 27M tokens** vs **21 over 6.5M**; comparable tokens per vulnerability inside core directories; only 12 in common. 12-hour game swarms: prescriptive-role and "CEO hierarchy" prompts "did not make much difference", games "bad". Older models merged few PRs; newer ones siloed; only Sonnet 5 shared code *and* merged. **18 of 30** agents picked the same branch name; polling stampede of **2.4M job requests, 117 accepted**; price collusion by round 3 | Coordination helps independent search. Hierarchy by prompt doesn't. Homogeneous agents fail together |
| Cursor, ["Agent swarms and the new model economics"](https://cursor.com/blog/agent-swarm-model-economics) (2026-07-20; HN 278/143) | Planner/worker tree building SQLite in Rust | New swarm hit **80% of a held-out suite in 4 h** (Grok 4.5); new runs 73–85% vs old runs 11–77% at 4 h; "Every mix produced similar quality, but the costs varied enormously"; gains "from this context efficiency, more than from parallelism" | Planners never implement, workers never plan, cheap models do the leaves. HN asks whether SQLite is in the training data ([48983663](https://news.ycombinator.com/item?id=48983663)) |
| Anthropic, [Project Vend phase two](https://www.anthropic.com/research/project-vend-2) (2025-12-18) | Shop agent + "CEO" agent + merch agent | Forced procedures were "among the most impactful changes": "we rediscovered that *bureaucracy matters*". The CEO "wasn't much help, and might even have been a hindrance"; same-model CEO "shared many of the deficiencies and blind spots"; the specialist with a clear role separation did better | Procedure and checklists beat managerial pressure |
| Claude Code / Codex docs (2026-10) | Native teams | "Agent teams… use significantly more tokens… work best when teammates can operate independently"; Codex: be careful with "parallel write-heavy workflows" | Vendors give the same guidance |
| "Zero-human company" demos | [gladiator](https://runtimenotes.com/projects/gladiator/) (Nous hackathon, 2026-03): Paperclip + 9 Sonnet agents | 10 tasks in ≈5–6 min, "≈$5–6 per run", winner "by projected GitHub stars" | A demo, not a measurement. No 2026 post-mortem of a revenue-earning zero-human company was found |
| Gas Town in use | Maggie Appleton (2026-01-23); HN | "Thousands of dollars a month in API costs"; design, not code, becomes the bottleneck; credit-burning upstream workflow (#3649) | Throughput is real; quality and cost control are not |

**Verdict:**
- **Teams pay off** for parallel, read-heavy, independently verifiable work: search, review, vulnerability hunting, test generation, decomposed builds behind a test suite.
- **They need** a planner/worker split for context hygiene, cheap models at the leaves, and verification at hand-back.
- **They fail** on dependent write-heavy work without ownership rules.
- **Hierarchy prompts** (CEO, roles) add little.
- **Same-model fleets** make correlated mistakes.
- **Token cost** runs 4–15×.

**For a control plane, the levers that work** are: procedures and `done_when`, verification gates, ownership/locks, diversity, cost caps.

## 6. Business models, licences, telemetry

| project | paid / cloud | pricing seen (2026-10-10) | licence trick | telemetry |
|---|---|---|---|---|
| Paperclip | "Paperclip Cloud waitlist" ([README](https://github.com/paperclipai/paperclip)) | Not published | MIT | Opt-in OpenTelemetry traces only (README) |
| Multica | Multica Cloud + self-host | Pricing page 404 when fetched | Apache-2.0 plus Part I: no hosted service to third parties without a commercial licence, "whether or not you charge for it"; no removing the logo or name from the UI; producer Index Labs (Hong Kong) Ltd ([LICENSE](https://github.com/multica-ai/multica/blob/main/LICENSE)) | "Anonymous deployment telemetry", `DO_NOT_TRACK=1` to disable ([README](https://github.com/multica-ai/multica)) |
| Symphony | None (drives Codex usage) | — | Apache-2.0; "build your own from SPEC.md" | — |
| Gas Town / Beads | None | — | MIT | Upstream-contribution formulas used users' credits and GitHub accounts ([#3649](https://github.com/gastownhall/gastown/issues/3649)) |
| Agent Orchestrator | Orchestrator.inc desktop app | FAQ "Is Orchestrator free to use?" (answer not in fetched text) | Apache-2.0 | — |
| Vibe Kanban | Had subscriptions and remote services; refunded and shut down 2026-04-10 | — | Apache-2.0 | Default-on analytics criticised (HN, 2025-07-11) |
| Superset | Free tier + paid (YC P26) | "Superset has a free tier" | ELv2: "inspect and self-host it subject to" the licence ([superset.sh](https://superset.sh)) | — |
| Orca | BYO agent subscriptions | Not seen | MIT | — |
| oh-my-openagent | — | — | SUL 1.0: internal or non-commercial use only | — |
| CrewAI | AMP cloud/VPC | Basic free (50 workflow executions/month), Enterprise custom ([crewai.com/pricing](https://www.crewai.com/pricing)) | MIT core | — |
| LangGraph / LangSmith | LangSmith Deployment | $0 (5k traces) / Plus $39 per seat (10k traces, Deployment) / Enterprise; LSU (LangChain usage unit) $1.00 pay-as-you-go ([pricing](https://www.langchain.com/pricing)) | MIT core | — |
| Mastra | Enterprise `ee/` code | — | Apache-2.0 + `ee/` enterprise licence | — |
| herdr | YC company | — | Apache-2.0; "the runtime stays open" | — |

**Patterns:**
1. **Open core:** a hosted cloud or waitlist, with SSO/RBAC/VPC as the enterprise up-sell (CrewAI, LangSmith, Mastra `ee/`, Multica Cloud).
2. **Source-available licences block competing hosts:** Multica's conditions, ELv2, SUL.
3. **Free-only tools die or go community.** Vibe Kanban could not monetise "thousands" of daily free users.
4. **Telemetry posture is judged publicly.** Default-on analytics and credit-spending workflows became top threads.

theuth's local-first, no-telemetry posture is an asset to state plainly.

## 7. Map to the three questions (user-evidence view)

### 7.1 The ten things users most want, and theuth

Status of theuth is taken from the AXES "theuth today" paragraph; the `ours` report is the authority.

| # | users want | evidence | theuth today | verdict |
|---|---|---|---|---|
| 1 | Any model or harness: local, OpenRouter, more CLIs, model per role | Paperclip #187 (79), #2092 (44), #1068 (35), #6559 (27); Multica #1900; Gas Town #10, #335 | Provider-agnostic engine; ACP harnesses (omp, opencode); model/effort per agent file | **Has**; breadth of harness list vs Paperclip's ~10 adapters to be checked by `ours` |
| 2 | Chat with or steer an agent mid-run | Paperclip #49 (49); Multica "reply while it works"; AO #5597; Copilot steer | Desks are sessions; `send_message`; chat | **Has** |
| 3 | Reach the team from phone or chat apps | Paperclip #2004; Orca/Superset mobile; cc-haha WeChat/Feishu | Slack/Signal/Telegram channels, web, phone | **Ahead** |
| 4 | Shared company knowledge, and memory that can't be poisoned | Paperclip #1858 (26); CrewAI #6043; Multica "manual skill curation" | Knowledge base + nightly dreaming (136) | **Has**; per-agent write guards/provenance: verify |
| 5 | External tracker as the queue (Linear/Jira/GitHub/GitLab) | Multica #1634 (24); vibe-kanban #1697 (26); AO #5700; Symphony on Linear | Internal tasks board; Jira/GitHub reachable only as MCP tools | **Gap** |
| 6 | Stage pipelines: who decides the next stage, loop until done | Multica #1943 (20), #815; vibe-kanban #1946 "Ralph Wiggum mode" (20) | Routines with `until` loops; tasks with `done_when`, review, escalate | **Partial**: no declared multi-stage pipeline |
| 7 | Self-host, export, no lock-in | Multica #2456, #1014; vibe-kanban #3396 + shutdown | Local engine, files + sqlite, mesh | **Ahead** (SSO not applicable) |
| 8 | Isolated workspace per agent, environment spin-up and cleanup | vibe-kanban #765 (19); Superset HN 48244816; AO #3853; container-use | Sandbox VMs, remote nodes; worktrees by convention | **Partial** |
| 9 | Usage, quota and rate-limit awareness; cost per issue | Gas Town #232; Orca usage + account switcher; Symphony PR #60; Multica usage analytics; HN "$20/mo… 10 minutes" | Budgets with auto-pause, spend ledger | **Partial**: no quota-aware failover or reset view |
| 10 | Reliability and honesty: installs cleanly, no hidden spend, opt-in telemetry, native-grade parallel stability | Ruflo #624/#662/#694 + audit; Gas Town #3649 (72); vibe-kanban telemetry; oh-my-claudecode #455; LangGraph #3716/#5672; Claude teams "task status can lag" | `tools.policy` approvals, activity monitor (155), budgets | **Has the posture**; make it explicit and keep proving liveness |

### Q1. What do they offer versus theuth? (per axis)

- **A1 Agent model & identity.**
  - Theirs: Paperclip and Multica agent records with adapters; huge persona catalogues (agency-agents 158.6k★, BMAD, wshobson).
  - Ours: agent files on a roster (67) with hiring in chat.
  - Gap: no importable template catalogue.
- **A2 Organisation & authority.**
  - Theirs: org charts (Paperclip), "CEO" roles (edict, Clawith).
  - Evidence (Anthropic 2026, Vend 2) says hierarchy by prompt adds little.
  - Ours: `reports_to` + escalation. **On par.** Don't invest more in hierarchy.
- **A3 Work model.**
  - Theirs: tracker-as-queue (Symphony, AO, Multica asks); proof-of-work on hand-back (Symphony).
  - Ours: tasks board with `done_when` and review.
  - Gap: tracker bridge, proof bundle.
- **A4 Execution & runtime.**
  - Theirs: worktree/VM fleets (Orca, Superset, AO), cloud agents (Cursor, Copilot); recurring adapter-boundary bugs (Paperclip).
  - Ours: mesh, ACP, sandbox VMs. **theuth ahead on placement**; worktree lifecycle is partial.
- **A5 Coordination.**
  - Theirs: shared task lists and mailboxes (Claude teams), Beads ledger (and its fragility, #764).
  - Ours: `send_message` + tasks with checkout. **On par**; diversity/anti-conformity is missing everywhere.
- **A6 Human in the loop.**
  - Theirs: plan approval (Claude teams), review gates (Multica), PR routing & approval (Cursor), mobile steering.
  - Ours: approvals + the double (126) + attention inbox. **Ahead**, except a plan-first gate.
- **A7 Cost.**
  - Theirs: per-agent monthly hard stops (Paperclip), usage per issue (Multica), quota views and account switching (Orca).
  - Ours: budgets with auto-pause. Gap: quota and rate-limit routing.
- **A8 Memory & knowledge.**
  - Theirs: mostly missing (Paperclip #1858 is open); CrewAI is fighting memory poisoning.
  - Ours: knowledge + dreaming. **theuth ahead.**
- **A9 Observability & UX.**
  - Theirs: kanban plus mobile (Orca, Superset, vibe-kanban); users protest UI churn (vibe-kanban #2509).
  - Ours: desktop, HUD, TUI, channels. On par or ahead.
- **A10 Extensibility.**
  - Theirs: plugin asks (Multica #4490); Paperclip plugin ecosystem (awesome-paperclip, 950★).
  - Ours: MCP, skills, ACP, A2A. **Ahead.**
- **A11 Traction & licence.**
  - Theirs: stars come from method packs; control-plane monetisation is unproven (vibe-kanban); source-available licences are spreading.
  - Ours: not a public project. Lesson: trust incidents spread fast.

### Q2. What can we learn or borrow? (techniques only)

1. **Proof-of-work bundle at hand-back.**
   - Mechanism: a run returns CI status, review feedback, a complexity analysis and a walkthrough video before landing ([Symphony README](https://github.com/openai/symphony)).
   - Cost of being wrong: low. An optional field can be ignored.
2. **Tracker as the queue.**
   - Mechanism: poll Linear and spawn per issue (Symphony); label-based eligibility (AO #5700).
   - Cost of being wrong: medium, because sync conflicts appear if two systems both own state.
3. **Plan approval before implementation.**
   - Mechanism: teammates run read-only in plan mode until the lead approves ([Claude docs](https://code.claude.com/docs/en/agent-teams)).
   - Cost of being wrong: low. A slower start on trivial tasks.
4. **Team-event hooks as quality gates.**
   - Mechanism: `TeammateIdle`, `TaskCreated` and `TaskCompleted` hooks run checks (same docs; also the Claude Code knowledge entry of 2026-10-08).
   - Cost of being wrong: low.
5. **Planner/worker split with model routing.**
   - Mechanism: planners never implement; cheap workers take the leaves; "similar quality… costs varied enormously" ([Cursor](https://cursor.com/blog/agent-swarm-model-economics)).
   - Cost of being wrong: medium. Weak workers can push a hidden quality debt to review.
6. **Procedures over pressure.**
   - Mechanism: force check-steps before commitments ([Vend 2](https://www.anthropic.com/research/project-vend-2)); MAST's verification failures.
   - Cost of being wrong: low.
7. **Diversity and anti-stampede by default.**
   - Mechanism: vary model, context or angle per sibling and claim names centrally. Motivated by 18/30 identical branch names and 2.4M polls for 117 jobs ([Anthropic 2026](https://www.anthropic.com/research/multiagent-systems)).
   - Cost of being wrong: low.
8. **Quota- and rate-limit-aware account switching with a usage view.**
   - Mechanism: show Claude/Codex usage and reset times; hot-swap accounts ([Orca](https://onorca.dev); Gas Town #232).
   - Cost of being wrong: medium. Must respect vendor terms on account pooling.
9. **Inter-agent message trust.**
   - Mechanism: messages are flagged "not from you"; an agent can't relay an approval it was denied (Claude docs). theuth already treats peer bodies as untrusted, so this confirms the design rather than adding to it.
10. **Explicit consent for any spend outside the user's goal; opt-in telemetry with a kill variable.**
    - Mechanism: Gas Town #3649 backlash; Multica's `DO_NOT_TRACK=1`.
    - Cost of being wrong: near zero.

### Q3. How should theuth improve? (ranked by value ÷ effort)

1. **Proof bundle on `review`.**
   - What: when a desk moves a task to review, require or attach artefacts (diff stats, test/CI output, screenshots, cost) and show them on the review card.
   - Seams: tasks board dispositions and `done_when` (`tasks.sqlite`, task tools); attention inbox; the double (126) can judge against `done_when`.
   - Value high, effort low-medium.
2. **Plan-first gate for desks.**
   - What: an optional `plan` step (or a task flag) where the desk posts a plan; the owner or the double approves before `doing`.
   - Seams: tasks state machine + double (126) + desk manual.
   - Value high (A6; MAST spec failures), effort low-medium.
3. **Tracker bridge.**
   - What: GitHub Issues, Linear and Jira as task sources, with label/assignee eligibility and write-back of state and PR links. Start one-way (import + status comments).
   - Seams: tasks board + routines/sensors; GitHub/Atlassian MCP already wired.
   - Value high (top cross-project ask), effort medium.
4. **Quota-aware routing + usage view.**
   - What: per-provider remaining quota, reset time and 429 rate; automatic failover to the next allowed model or provider for an agent; cost per task.
   - Seams: budgets + spend ledger, agent `model:` fallback list, HUD spend panel.
   - Value high, effort medium.
5. **Diversity and anti-stampede defaults in fan-out.**
   - What: when N siblings get the same brief, vary model, effort or angle; give each a unique work-claim prefix; put an engine-level semaphore on heavy jobs instead of a prompt protocol.
   - Seams: `spawn_subagent`, activity monitor (155).
   - Value medium-high, effort low.
6. **Planner/worker presets.**
   - What: roster templates where planners use the strong model and workers the cheap one, with a "planner never edits" charter line.
   - Seams: agent files / `agent_draft` (67).
   - Value medium, effort low.
7. **Team-event hooks.**
   - What: run checks on task created, done or idle; refuse `done` when a hook fails.
   - Seams: routines sensors + tasks events.
   - Value medium, effort low-medium.
8. **Template catalogue import.**
   - What: import persona packs (agency-agents, BMAD roles, wshobson) as draft hires, techniques only, never their text verbatim if licences forbid.
   - Seams: hiring (`agent_draft`).
   - Value medium (users love catalogues), effort low.
9. **Worktree lifecycle.**
   - What: engine-managed worktree per task, with per-worktree dev-server/port and cleanup policy (vibe-kanban #765, Superset HN).
   - Seams: tasks ↔ workspace field, sandbox VMs.
   - Value medium, effort medium.
10. **Trust statement.**
    - What: document "no telemetry, no spend outside the owner's goals, every external action approved" in onboarding and the README.
    - Value medium, effort trivial.

**Not to borrow:**
- Org-chart and CEO theatrics beyond `reports_to`: Anthropic 2026 and Vend 2 show little gain.
- Hundreds of MCP "swarm" tools: the Ruflo audit's "5x Token Waste".
- An issue tracker as runtime state store: Gas Town #764.