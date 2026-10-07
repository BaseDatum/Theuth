# Recon A — product landscape: how products let one person supervise many coding agents

Design 130 recon. Written 2026-10-07 by subagent recon-130-products. Facts only; every claim carries a URL or is marked unverified. Status: COMPLETE (sections 1–5). Time box overrun by ~15 min; search providers were rate-limited, so product pages were read directly and GitHub issues were used in place of HN/Reddit threads.

## 1. One (getone.one) in full

Sources read in full: https://getone.one/ (home, FAQ, pricing), https://getone.one/docs/remote-machines ("Updated 5 October 2026"),
https://getone.one/privacy ("Effective 28 September 2026 · Version 0.6"), https://getone.one/support ("Updated 5 October 2026"),
https://herdr.dev/ and https://herdr.dev/docs/agents/. `map_site` of getone.one finds only six pages (/, /docs/remote-machines,
/support, /privacy, /terms, /remote); there is no public changelog page. /remote is an empty JS shell (the phone web remote), not read.
Bundle id is `com.kyzo.one`; older builds were called "agentpill" (`~/.config/agentpill/`) — https://getone.one/support.

### 1.1 Positioning (exact phrasing)
- Tagline: "Talk to your agents without switching context" — "One gives you one place to see, answer and talk to all your coding
  agents, on your Mac, your iPhone and your servers, so none of them sits waiting." (https://getone.one/)
- FAQ "What is One?": "A Mac app for developers who run coding agents such as Claude Code and Codex. A toolbar on the side of your
  screen shows your agents. Their questions and finished work collect in an Inbox, and One sends each task you type or say to the
  agent already on that work, or starts a new agent. There's also an iPhone app, and a connector for agents on Linux servers."
- FAQ "Is One another coding agent?": "No. The coding agents you already run … write the code. One shows their status, collects their
  questions and passes them your requests."
- Works on top of: Claude Code, Codex, OpenCode, Pi, Omp, Amp, Grok (home page list); support page requires "At least one coding
  agent installed: Claude Code, Codex, OpenCode, Pi, Omp or Grok".
- Setup claim: "No config files. One finds the agents you already run." Three steps: Download/sign in → connect machines (scan a code
  with iPhone, or paste one command on a Linux server) → "Reply to your first agent. Answer in the Inbox, or switch to Talk and say
  what you need. One sends it to the right agent and tells you when it's done." (https://getone.one/)

### 1.2 Surfaces
| Surface | What the site says | Source |
|---|---|---|
| Side toolbar ("pill") | "A toolbar on the side of your screen shows your agents." Support page calls the on-screen element "the pill" ("The pill shows a flat waveform" when mic is missing). Hero mock shows rows like `claude-7 · ~/shop/web` with `✻ Working…`. | https://getone.one/ ; https://getone.one/support |
| Inbox | "Every agent's questions and finished work, in one list. Press 1, 2 or 3 and the agent has its answer." Cards show agent name + question + numbered options, one marked "Recommended". Also carries system cards such as "Update One on <machine>" (connector update approval). | https://getone.one/ ; https://getone.one/docs/remote-machines |
| Router ("Talk") | "Double-tap Option and say what you need, or type it. One sends it to the right agent, or starts a new one." Mock: "Fix the flaky test" offered against claude-7 / codex-12 / opencode-3. | https://getone.one/ |
| iPhone app | "A notification when an agent asks or finishes. Answer from the same Inbox, wherever you are." Free TestFlight beta, iOS 17+, iPhone and iPad; Android/other phones use getone.one/remote in a browser "without notifications". Pairing: scan the code One shows on the Mac, press Allow. | https://getone.one/ (FAQ "Can I use it from my phone?") |
| Server connector | "Paste one command on a Linux server or VPS. Its agents join the same list and Inbox, end-to-end encrypted." Up to 8 servers; x64 or arm64; server opens no port. | https://getone.one/ ; https://getone.one/docs/remote-machines |

### 1.3 Interactions (exact phrasing)
- Answer by keystroke: "Press 1, 2 or 3 and the agent has its answer." (home)
- Undo window — "Take it back: Every answer waits 2 seconds. Press Escape to undo." (home)
- Keyboard navigation — "Keyboard first: J and K move between cards. E clears one." (home)
- Global trigger — "Double-tap Option and say what you need, or type it." Support page: "Double-tap the Option key quickly, on its own,
  without holding it"; needs Accessibility so "the gesture works from any app"; conflicts with utilities that capture Option.
- Screen pointing — "Point at it: Drag over your screen, then say what should happen." Privacy: "Your screen: a capture of what you
  are looking at, so the model can see what you are pointing at." Support: "Takes one screenshot per turn so the model can see what you
  are pointing at."
- Suggested replies — "Next steps: Finished work comes with up to two replies, ready to send." Remote machines: "No suggested replies.
  One doesn't suggest replies for agents on remote machines. The connector's setting for them is off by default."
- Actions available per agent (remote doc, same as local): "reply with text, photos and files, answer their questions, stop or kill
  them, and start new ones in a folder on that machine."
- Routing to an agent in a foreign terminal: "When you message an agent that's still open in another terminal, 'Move to herdr and
  send' closes it there and reopens the same conversation in herdr." (home FAQ)
- Inbox question staleness check (connector): "answers a numbered question with its digit key after checking the question is still the
  one you answered" (https://getone.one/docs/remote-machines).

### 1.4 What the AI layer does and does not do
- "Does One act on my agents by itself? No. One acts when you send it a request or answer in the Inbox. It does what you asked, shows a
  receipt of what it did, and stops. It doesn't keep working on its own, and suggested next steps reach an agent only when you pick
  them." (home FAQ)
- What each request carries: "Each request sends your words, that screenshot and your agents' recent activity, including their last
  terminal lines, through One's server to AI providers (straight to OpenAI if you add your own API key). The agent itself gets the
  screenshot only when the request needs it. In the background, excerpts of agent conversations go the same way to write titles,
  recaps and next steps. One's server keeps no copy." (home FAQ)
- Model routing: "On every plan, requests run on your ChatGPT subscription first, or on your own OpenAI API key …, and One's AI steps in
  if they can't be reached. Voice, titles and recaps use One's AI." (home FAQ "What does it cost?")
- Local state kept on the Mac: "conversation history, working memory, and agent-thread summaries" — files `history.json`, `memory.md`,
  `recaps.json` in `~/.config/one/` (privacy "What stays on your Mac"; support "Logs and reset").
- Providers named in the privacy policy: OpenAI (One requests + Codex), Anthropic (Claude), OpenRouter (routing + agent summaries),
  voice transcription "OpenAI, Groq, AssemblyAI, Google, or Apple", PostHog analytics (no off switch), Loops email.
- "One listens only while you record, and takes a screenshot only when you send it a request from your Mac." "It is not an always-on
  recorder." (home FAQ; privacy "In short")
- Temporary screenshots "are deleted automatically after 10 minutes and on reboot" (support).

### 1.5 Permissions required (macOS)
Table from https://getone.one/support:
- Microphone — "Records the utterance you speak during a turn."
- Screen Recording — "Takes one screenshot per turn so the model can see what you are pointing at." (macOS 15+ re-confirms monthly.)
- Accessibility — "Makes the Option key double-tap work from any app and lets One type into terminals."
- Automation — "Lets One talk to your terminal app (Terminal, iTerm, Ghostty, and so on) to find and focus agent panes." Missing →
  "Agents outside herdr are not detected, or the wrong window is focused."
iPhone: Microphone, Speech Recognition, Notifications (privacy policy). Sign-in: Apple or Google; optional ChatGPT account "if you want
Codex to run on your ChatGPT plan" (support).

### 1.6 How it detects and drives agents
- Dependency on herdr: "Do my agents have to run in herdr? To answer, stop or message them from One, yes. herdr is a free, open-source
  terminal for coding agents that keeps them running after you close the window. Setup installs it and reopens your idle terminal
  conversations in it … Agents in other terminals still show up in One, and you answer their questions there. … Sessions in the Claude
  and OpenCode desktop apps stay where they are. Every agent One starts runs in herdr." (home FAQ)
- Detection outside herdr: "For agents in a plain terminal, One needs the Automation permission for that terminal app and reads the
  agent's own session files (`~/.claude`, `~/.codex`, OpenCode, Pi)." (support)
- Connector mechanics (remote): "Reads herdr's list of agents and each agent's conversation, from Claude Code's and Codex's own session
  files, or from the terminal screen when there is none. It looks every 2.5 seconds while you're watching and every 10 seconds
  otherwise." Acts only on device-signed requests: "it types your message into an agent, answers a numbered question with its digit
  key …, presses Esc to stop an agent, closes its herdr pane to kill it, and starts a new agent in a folder the machine offers."
  Resource limits: "low priority, at most 200 MB of memory and a quarter of one CPU core". Checks Claude/Codex sign-in every 10 min.
  (https://getone.one/docs/remote-machines)
- Security model (remote): AES-256-GCM per machine, device-signed commands, device approval by code comparison, "What One's servers can
  see: which agents exist on the machine and their state (working, waiting or finished), when things happen, and the machine's name."
- herdr itself (https://herdr.dev, Apache 2.0, "42,441 github stars", "22 agent CLIs detected"): "a server running in the background,
  and the terminals live inside it"; "Herdr reads every pane and marks each agent working, blocked or idle. When one stops and needs an
  answer it says so, so you don't go pane by pane looking for whoever is waiting on you." Status comes from a per-agent "detection
  manifest" applied to "the live bottom of the pane"; "Blocked detection is deliberately strict … Herdr only marks `blocked` when the
  live bottom-buffer snapshot matches known visible approval, question, or permission UI"; unmatched screens fall back to `idle`
  (Codex to `unknown`). Agents may instead self-report state over herdr's CLI/socket. (https://herdr.dev/docs/agents/)
- herdr's "the herd" mock shows the same three states rolled up per workspace: `web-dashboard blocked · herdr working · data-pipeline idle`.

### 1.7 Pricing and platform minimums
- Free: "25 requests a day to One's AI, reset at midnight UTC." Pro: "$12.99 a month ($6.99 for the first month) or $119.88 a year, each
  after a 3-day free trial" — "Pro covers the models for you", "Premium routing". Lifetime: "a limited edition of 260, paid once: $179
  for the next 39, then $229, then $299" (steps $99 and $129 sold out) — "Bring your own API key", "Unlimited updates". 3-day full
  refund from Settings. (https://getone.one/)
- Platform: "macOS 13 Ventura or later. macOS 14+ is recommended. Apple silicon & Intel … universal build." "There's no Windows
  version." iOS 17+ for the phone app. Linux x64/arm64 for the connector, Node.js 22+ (installed privately if missing), systemd user
  service. (https://getone.one/support ; https://getone.one/docs/remote-machines)
- "Agents on your Mac need the Mac awake with One open. Agents on a connected Linux server stay reachable from your phone even when your
  Mac is off." (home FAQ)
- Status: "One is in beta" (support). The home page's "Join waitlist" link coexists with live checkout buttons.

### 1.8 Things not found / unverified
- No public changelog, screenshots gallery or keyboard reference beyond the home-page cards; the exact rail layout (position, size,
  glyphs) is only visible in the home mock and in-app — unverified.
- Whether "Point at it" sends a cropped region or the full screen with the drag rectangle — unverified (privacy says "a capture of what
  you are looking at").
- Grouping/quiet-hours notification policy for the iPhone app — not documented; only "A notification when an agent asks or finishes."

## 2. Competing / adjacent products

Verified on each product's own site/docs on 2026-10-07 unless marked. Products that could not be verified or no longer exist are
listed at the end. Where a site is a marketing page, only what the page literally shows is recorded.

### 2.1 Claude Code — agent view (`claude agents`), Desktop app (Code tab), Cowork
Source: https://code.claude.com/docs/en/agent-view ("research preview"), https://code.claude.com/docs/en/desktop, https://claude.com/product/cowork.
- **Session list**: "one screen for all your background sessions … lists every session grouped by state, with pinned sessions and the
  ones that need you at the top" — groups shown: Pinned, Ready for review, Needs input, Working, Completed. Each row: state icon,
  name (tinted by `/color`), one-line summary, age; `#N` PR label at the right edge coloured by PR status (yellow waiting/failed,
  green passing, purple merged, grey draft/closed).
- **State glyphs**: colour = Working (animated) / Needs input (yellow) / Idle (dimmed) / Completed (green) / Failed (red) / Stopped
  (grey); shape = `✻`/`✽` process running or needs input, `∙` process exited (restarts on reply), `✢` a `/loop` session sleeping.
- **Row summaries**: "generated by a Haiku-class model so the row can tell you what the session is doing, what it needs, or what it
  produced without opening the transcript"; refreshed ≤ every 15 s from output without a model call, rewritten at turn end and
  "every few minutes" during long turns; "a blocked row shows the question it's asking".
- **Answering a question**: `Space` opens a peek panel showing "the exact question it's asking, above the reply input" and `waiting
  3m`; "A question with predefined choices: the panel lists the choices by number. With the reply input empty, press a choice's
  number to fill it in, then Enter to send it"; a suggested reply fills with `Tab`; a reply to a working session joins its message
  queue; `/stop` stops; **permission prompts cannot be answered from the peek panel** — "replying doesn't answer it … To answer the
  dialog, attach with →". Undeliverable replies are saved and sent when the process restarts.
- **Keyboard model**: arrows select, `Space` peek, `Enter`/`→` attach, `←` detach/background, `Ctrl+X` stop, `Esc` leave; voice
  dictation in hold mode works in the reply and dispatch inputs.
- **Needs-you counter outside the view**: inside a regular session the footer shows `← 2 agents` waiting (refresh ~10 s, flashes on
  change, `← 2 done` when none need input); terminal tab title shows `2 awaiting input · claude agents`.
- **Notifications**: while agent view is open, a notification through the configured terminal channel "when a local background
  session starts needing your input, finishes, or fails"; `/loop` sessions "notify only when they need your input"; fires the
  `Notification` hook with `agent_needs_input` / `agent_completed`.
- **Attach recap**: "When you attach, Claude posts a short recap of what happened while you were away."
- **Scope limits**: interactive sessions in other terminals are not listed until backgrounded; subagents/teammates are not rows;
  each session "uses your subscription quota independently".
- **Desktop app (Code tab)**: sidebar of sessions run in parallel (`Cmd+N`, `Ctrl+Tab` cycling, `Cmd`-click to split two
  sessions), worktree option per session, auto-archive after PR merge/close, CI status bar with "Auto-fix CI & address comments" and
  "Auto-merge when ready", a usage ring (context per session, plan usage shared), computer use, and "Work across sessions": Claude
  "can list your other Code tab sessions, read what each has been doing, and send messages between them" (desktop-run sessions
  only; "Claude never lists the session you're asking from"; 20 most recent). Phone: "Dispatch sessions from your phone".
  `1`–`9` select items in open menus. No documented always-visible outside-the-window surface.
- **Cowork** (separate tab): "Give it a goal, and it works across your files and tools. You come back to polished work for your
  review." Runs on web/desktop/mobile; "Close your laptop, it keeps going. Schedule a task for any cadence"; built-in browser
  (2026-08-26 post). Marketing page only; no inbox/keystroke model documented.

### 2.2 OpenAI Codex app (macOS, Feb 2026) → now "ChatGPT desktop app"
Source: https://openai.com/index/introducing-the-codex-app/ (2026-02-02; "March 4, 2026 update: … now available on Windows"),
https://developers.openai.com/codex/app (page now titled "ChatGPT desktop app").
- Pitch: "a command center for agents … to effortlessly manage multiple agents at once, run work in parallel, and collaborate with
  agents over long-running tasks." "The core challenge has shifted from what agents can do to how people can direct, supervise, and
  collaborate with them at scale."
- Model: "Agents run in separate threads organized by projects … review the agent's changes in the thread, comment on the diff, and
  even open it in your editor"; "built-in support for worktrees"; picks up CLI/IDE session history.
- Review queue: "When an Automation finishes, the results land in a review queue so you can jump back in."
- Current docs page: "Keep every chat in view — Move between projects and long-running work without losing context"; Chat/Work
  toggle; scheduled tasks. No inbox-across-agents, keystroke answering or outside-window surface documented. Codex cloud tasks
  page (/codex/cloud) not read — unverified.
- GitHub issue trail (openai/codex): "Codex App: Add subagent monitoring to the UI" #10471 (closed), "Add notification for command
  execution approval prompts" #2961 (closed), "TUI(collab): Persistent 'Agents' sidepanel (active threads overview + quick
  actions)" #9902 (closed) — https://github.com/openai/codex/issues/<n>.

### 2.3 Cursor — Agents Window (Cursor 2.0 → GA in Cursor 3) and Cloud Agents
Source: https://cursor.com/blog/2-0 (2025-10-29), https://cursor.com/docs/agent/agents-window, https://cursor.com/docs/cloud-agent.
- "a new interface for working with many agents in parallel … centered around agents rather than files"; "powered by git worktrees or
  remote machines"; "having multiple models attempt the same problem and picking the best result significantly improves the final
  output"; new bottlenecks named: "reviewing code and testing the changes" → new diffs view + native browser tool.
- Agents Window: "Multi-workspace", "New diffs view: review and commit changes, and manage PRs", "Parallel agents: run many parallel
  agents in the cloud (and work with them from your phone, web, Slack, GitHub, and Linear)", "Easier handoff between local and
  cloud", "Cloud subagents … `/in-cloud`, or put a PR on `/autopilot`", "Worktrees". "Generally available with Cursor 3, released on
  April 2, 2026."
- Cloud Agents: "run as many agents as you want in parallel"; started from iOS app, web, Slack, etc. No single inbox or keystroke
  answering documented; the supervision surface is an IDE window, not an edge rail.

### 2.4 Conductor (conductor.build)
Source: https://conductor.build/, https://docs.conductor.build/ (concept pages /docs/concepts/parallel-agents and a notifications
reference returned 404 — unverified).
- "Run a team of coding agents in the cloud." "Conductor lets you run Claude Code, Codex, Cursor, and OpenCode in parallel. Each task
  gets its own workspace, branch, files, terminal, diff, and review path. When the work is ready, Conductor helps you review the
  diff, open a pull request, merge, and archive the workspace."
- Cloud sandboxes on "isolated microVMs" (Firecracker). Multiplayer: "Share a workspace link (⌘⇧C) with teammates, see who is active,
  follow work you care about, and prompt agents together in real-time." Conductor API (`create_session`), macOS app, iOS app
  (changelog 0.90.0 "Conductor for iOS", 2026-10-02), "Sign in with ChatGPT" (0.89.0). "Trusted by 100k+ builders".
- Supervision UX: workspace-per-task desktop app with diff viewer and checks; no inbox/keystroke model documented.

### 2.5 Warp — Oz
Source: https://www.warp.dev/oz.
- Positioned as "cloud agent platform"/"software factory": "Start agents from within any tool based on any trigger — an @warp
  mention or a specific event"; "Single pane of glass to view all agents across your whole team. See all agents running across your
  team in one interface. Join sessions with one click to audit agents. Sort by task originator"; routes across models and can
  "delegate tasks to Claude Code or Codex"; self-hosted or Warp-hosted. Team/enterprise framing; no personal inbox, keystrokes or
  notification policy on the page.

### 2.6 Zed — Agent Panel, Threads Sidebar, ACP
Source: https://zed.dev/docs/ai/external-agents, https://agentclientprotocol.com/overview/introduction.
- External agents (Claude, Codex, OpenCode, Copilot, Cursor, Pi, …) run via ACP; "Zed hosts the thread in the Agent Panel and
  Threads Sidebar, while the External Agent usually owns its own runtime, auth, model selection, tools"; installed from an "ACP
  Registry". Terminal Threads for CLIs/TUIs. The parallel-agents page (Threads Sidebar) was not read — notification/keystroke
  details unverified. Supervision is inside the editor.

### 2.7 Devin Desktop (Cognition; "Windsurf is now Devin Desktop")
Source: https://devin.ai/desktop.
- "Manage fleets of local and cloud agents from one surface. Plan, delegate, review, and ship without leaving your editor." The page
  mock shows a Sessions list with Board/List views and filters (Status, Space, Pull request, Agent) and state groups "Running 2 ·
  Waiting for review 2 · Done 3", rows like "PR is ready •", "Waiting for CI •", "Working...", ages "42m ago". Full IDE included;
  supports ACP ("Agent Client Protocol" section). Sessions-list docs page (/work-with-devin/parallel-devins) returned an empty shell
  — unverified.

### 2.8 Factory — Missions
Source: https://factory.com/product/missions.
- "Break large projects into subtasks and run them in parallel with multiple Droids"; "Automatic task planning"; "Progress tracking
  and coordination — Real-time visibility into each Droid's progress"; a terminal "Mission Control" mock. Droids reachable from
  "Terminal and IDE … Browser, Slack, Jira". Orchestration-first; no inbox/keystroke surface documented.

### 2.9 HumanLayer / CodeLayer
Source: https://www.humanlayer.dev/codelayer (page title "The multiplayer control plane for your software factory"; product now
branded HumanLayer).
- "a multiplayer coding agent IDE and cloud"; Tasks group "agent sessions, artifacts, and worktrees"; session states shown as
  RUNNING / DONE / IDLE / DRAFT; "Superhuman for claude code — Jump between tasks, sessions, and artifacts with a keystroke. A
  command palette for every action" with `⌘N` new task, `⌘K` jump to session, `⌘⇧A` open artifact, `⌘⏎` send to agent, `⌘/`
  comment on selection; local daemon + cloud daemons + mobile, "Agents keep working even if you close your laptop"; "Comment-driven
  refinement" on design docs before code.

### 2.10 LangChain Agent Inbox
Source: https://github.com/langchain-ai/agent-inbox.
- "An inbox UX for interacting with agents": a web UI over LangGraph deployments that lists `interrupt`s and sends back
  `HumanResponse` objects (accept / edit / respond / ignore shapes per the schema sections). Configured with graph id + deployment
  URL per inbox. Not coding-agent specific; the reference implementation of "interrupts collect in an inbox, human answers in bulk".

### 2.11 Vibe Kanban (BloopAI) — sunsetting
Source: https://www.vibekanban.com/ ("Vibe Kanban is sunsetting. The project will continue as open source and community maintained",
GitHub 28.3k). Thesis: "The new bottleneck is planning and review … switch your attention to whatever's ready to review." Kanban of
issues → agents (Claude Code, Codex, OpenCode …) in worktrees, diff comments, dev-server QA, PR status auto-updated; "100,000 PRs
created", "30,000 active users".

### 2.12 Crystal (Stravu) → Nimbalyst
Source: https://github.com/stravu/crystal ("Crystal is now Nimbalyst. Run multiple Codex and Claude Code AI sessions in parallel git
worktrees", MIT, 3.1k stars). Nimbalyst's own site (https://nimbalyst.com) rendered as an empty shell — current UX unverified.

### 2.13 Mux (coder.com) → Xum
Source: https://xum.coder.com/ (mux.coder.com redirects there). "run parallel coding agents, each with its own isolated workspace,
right from your browser or desktop"; "Supporting UI and keybinds for efficiently managing a suite of agents"; multi-model; AGPL-3.0
(techniques only, never open the clone). Keybinds page not read.

### 2.14 Sculptor (Imbue)
Source: https://imbue.com/sculptor/ (published 2025-09-26, updated 2026-04-30). Agents in containers (not worktrees); "Pairing Mode
… brings an agent's work from its container into your local repo"; "Merge without conflicts … hand them back to the agent to
resolve"; sessions saved with plans/chats/tool calls; macOS Apple Silicon + Linux. No inbox/notification model described.

### 2.15 Superset (superset.sh)
Source: https://superset.sh/ (GitHub 15k). "One workspace for Claude Code, Codex, and any coding agent"; sidebar mock groups
workspaces by machine (desktop 5, cloud 3, gpu-box 14, mobile 1, cli 4) with `+46 −1` diff stats per row; Automations table
(daily-triage, changelog-draft …); `superset status` prints "3 workspaces running · ⠋ nightly evals · claude · 42m / ✓ api hotfix ·
ready for review / ✓ docs refresh · PR opened"; "Superset Mobile is here"; Review tab with Approve/Comment. Keystroke/notification
model not documented on the page.

### 2.16 Emdash (emdash.com)
Source: https://emdash.com/ ("Open-source agentic development environment", "Over 1M downloads", "Backed by Y Combinator",
v1.2.0). Parallel tasks in worktrees with "branch, terminal, conversation, and review state together"; issues from Linear/Jira/
GitHub/Notion/Asana; in-app browser; diff/PR/CI review; scheduled Automations; remote SSH tasks; "25+ coding agents". Task rows show
`running · 4m` / `done · 8m ago`. No inbox/keystroke/notification model documented.

### 2.17 AgentManager (agentmgr.app) — closest to One's "waiting" problem
Source: https://agentmgr.app/ (macOS, `brew install --cask umechanhika/tap/agent-manager`; Free ≤ 2 sessions, Pro $4.99/mo).
- "Stop keeping Claude waiting. Tells you when a session is waiting. One click to jump in." Problems named: "Wasted wait. Claude sits
  on 'proceed?' behind whatever you're doing"; "Constant checking. You cycle through tabs"; "The silent crash. It died on an API
  error. You thought it was still running"; "Lost context. Context compacts mid-task".
- UX: "See why it's waiting — A quick yes/no, a plan to review, or a choice to make — the status dot itself shows which"; "Surfaces
  when you need it. Appears the moment any session needs input. Hides when everything is clear"; "Jump to the right terminal … the
  exact pane in iTerm2, the right tab in Terminal and Ghostty, the project window in your IDE"; toggle with `⌥ Space`; "Status in
  your menu bar — See how many sessions are waiting"; "Know before it compacts — A percentage appears once a session passes 80% of
  its context"; usage-limit banner; red lamp + "!" when a session dies on an API error and "the window comes forward"; sound alerts.
  It only observes and jumps — no answering from the window is described.

### 2.18 CCManager, clave, Yardmaster, Harness, jmux (small open-source managers)
- CCManager — https://github.com/kbwo/ccmanager (MIT, 1.3k stars): "Coding Agent Session Manager for Claude Code / Gemini CLI / Codex
  CLI / Cursor Agent / Copilot CLI / Cline CLI / OpenCode / Kimi CLI" (TUI, worktree-based). Detail pages not read.
- clave — https://github.com/antasphere/clave (MIT, 51 stars): "A macOS desktop app for managing multiple Claude Code sessions"
  (Electron).
- HN Show posts found via Algolia (low points, titles only): "Yardmaster – manage multiple Claude Code sessions from one terminal"
  (id 46975707), "Harness – Manage parallel Claude Code agents across Git worktrees" (id 47948379, 3 pts), "jmux – tmux-based
  development environment for humans and coding agents" (id 47650233, 10 pts). Contents unverified.

### 2.19 Not verifiable / defunct
- **Terragon** — https://www.terragonlabs.com/ now titled "Terragon Shutdown"; product no longer available.
- **Trail** — no product found under that name in the searches run (search providers were rate-limited); unverified.
- **Codex cloud tasks** page, **Zed Threads Sidebar** page, **Nimbalyst** site, **Devin parallel sessions** docs — not readable or
  not read in the time box; marked unverified above.

## 3. Pattern catalogue

| Pattern | Products using it (verified) | Cognitive-load problem it addresses | Evidence |
|---|---|---|---|
| Persistent edge/side rail outside the main window | One ("A toolbar on the side of your screen shows your agents"); AgentManager (window toggled with ⌥Space, "Appears the moment any session needs input. Hides when everything is clear", menu-bar count) | Knowing agent state without switching apps or windows | https://getone.one/ ; https://agentmgr.app/ |
| Glanceable per-agent state glyphs (working / blocked / idle / done) | Claude agent view (colour + shape table: animated/yellow/dimmed/green/red/grey; ✻ ∙ ✢); herdr (`working`/`blocked`/`idle` rolled up to tabs and workspaces); AgentManager ("the status dot itself shows which" kind of wait); Devin Desktop (Running / Waiting for review / Done groups); HumanLayer (RUNNING / DONE / IDLE / DRAFT) | "You never hunt for the stuck one" — replaces polling every pane | https://code.claude.com/docs/en/agent-view ; https://herdr.dev/ ; https://agentmgr.app/ ; https://devin.ai/desktop ; https://www.humanlayer.dev/codelayer |
| Single inbox across agents (questions + finished work in one list) | One Inbox ("Every agent's questions and finished work, in one list"); LangChain Agent Inbox (interrupts from any graph in one UI); Claude agent view groups "Needs input" and "Ready for review" at the top; Codex Automations "results land in a review queue" | One place to look instead of N windows; the list is the to-do | https://getone.one/ ; https://github.com/langchain-ai/agent-inbox ; https://code.claude.com/docs/en/agent-view ; https://openai.com/index/introducing-the-codex-app/ |
| Answer-by-keystroke (numbered options) | One ("Press 1, 2 or 3 and the agent has its answer"; connector types the digit after re-checking the question); Claude agent view peek panel ("press a choice's number to fill it in, then Enter"); Claude Desktop menus (`1`–`9` select item) | Decision cost ≈ one keypress; no retyping the option | https://getone.one/ ; https://getone.one/docs/remote-machines ; https://code.claude.com/docs/en/agent-view ; https://code.claude.com/docs/en/desktop |
| Suggested replies / next steps attached to finished work | One ("Finished work comes with up to two replies, ready to send"; off for remote machines by default); Claude agent view ("When the empty input shows a suggested reply, press Tab to fill it in") | Removes the blank-prompt cost when deciding what to do next | https://getone.one/ ; https://code.claude.com/docs/en/agent-view |
| Undo window after an answer | One ("Every answer waits 2 seconds. Press Escape to undo") | Makes one-keystroke answers safe against slips | https://getone.one/ |
| Batch answering / keyboard triage of a list | One ("J and K move between cards. E clears one"); Claude agent view (`↑`/`↓` peek adjacent sessions without closing the panel); HumanLayer (`⌘K` jump to session, "A command palette for every action") | Clear many waits in one sitting without the mouse | https://getone.one/ ; https://code.claude.com/docs/en/agent-view ; https://www.humanlayer.dev/codelayer |
| "Needs you" vs "FYI" separation | Claude agent view (Needs input vs Completed groups; `/loop` sessions "notify only when they need your input"; footer `← 2 agents` vs `← 2 done`); AgentManager (surfaces only when "any session needs input"); herdr (`blocked` is "deliberately strict" — only known approval/question/permission UI counts) | Avoids false urgency; interruptions only for blocking states | https://code.claude.com/docs/en/agent-view ; https://agentmgr.app/ ; https://herdr.dev/docs/agents/ |
| Notification policy (triggers, channel, sound) | Claude agent view (notify on needs-input / finished / failed via `preferredNotifChannel`, `Notification` hook types `agent_needs_input` / `agent_completed`); One iPhone ("A notification when an agent asks or finishes"); AgentManager (sound "the moment a session needs you", red lamp + window comes forward on API death); Codex `notify` originally only on completion (issue #2961 asked for approval prompts) | Decides what is worth an interrupt; grouping and quiet hours not documented by any product read | https://code.claude.com/docs/en/agent-view ; https://getone.one/ ; https://agentmgr.app/ ; https://github.com/openai/codex/issues/2961 |
| Global hotkey palette / router | One (double-tap Option → say or type; "One sends it to the right agent, or starts a new one"); AgentManager (`⌥ Space` toggle); HumanLayer (command palette) | Start or steer work from any app without finding the right window | https://getone.one/ ; https://agentmgr.app/ ; https://www.humanlayer.dev/codelayer |
| Voice input | One (Microphone; "say what you need"); Claude Code voice dictation in hold mode inside agent view | Lower effort than typing while hands are elsewhere | https://getone.one/support ; https://code.claude.com/docs/en/agent-view |
| Screen/pointer context | One ("Point at it: Drag over your screen, then say what should happen"; one screenshot per turn, Screen Recording permission) | Deixis: "this" means what is on screen | https://getone.one/ ; https://getone.one/support |
| Receipts (what the AI layer did) | One ("It does what you asked, shows a receipt of what it did, and stops"); Claude Desktop cross-session messages appear "as a card labeled with the sending session's title and a link back" | Trust in an intermediary that types into agents | https://getone.one/ ; https://code.claude.com/docs/en/desktop |
| Diff review inside the supervisor | Conductor (diff viewer, checks, PR, merge, archive); Cursor Agents Window ("New diffs view"); Codex app (comment on the diff in thread); Claude Desktop (diff pane, CI status bar, auto-fix/auto-merge); Vibe Kanban, Emdash, Superset (Review tab Approve/Comment) | Review is the new bottleneck (Vibe Kanban, Cursor 2.0 both say so) | https://docs.conductor.build/ ; https://cursor.com/docs/agent/agents-window ; https://code.claude.com/docs/en/desktop ; https://www.vibekanban.com/ |
| Cost / time / context per agent | Claude agent view (row age; finished age freezes at run time; `waiting 3m`); Claude Desktop usage ring; AgentManager (context % past 80 %, usage-limit banner); Superset (`+46 −1` diff stats per workspace, `42m`) | Knowing which agent is expensive or about to compact | https://code.claude.com/docs/en/agent-view ; https://code.claude.com/docs/en/desktop ; https://agentmgr.app/ ; https://superset.sh/ |
| Phone follow-up | One iPhone app + web remote; Claude Desktop "Dispatch sessions from your phone"; Cursor iOS; Conductor for iOS; Superset Mobile; HumanLayer mobile | Supervision continues away from the desk | https://getone.one/ ; https://code.claude.com/docs/en/desktop ; https://cursor.com/docs/cloud-agent ; https://conductor.build/ ; https://superset.sh/ |
| Multi-machine (agents elsewhere in the same list) | One connector (up to 8 Linux servers, E2E encrypted, same Inbox); herdr (`herdr machine add`); Superset (`superset connect gpu-box`); Emdash remote SSH; HumanLayer cloud daemons; Claude Desktop SSH/cloud sessions | One list regardless of where the process runs | https://getone.one/docs/remote-machines ; https://herdr.dev/ ; https://superset.sh/ ; https://emdash.com/ ; https://code.claude.com/docs/en/desktop |
| Attach-with-recap | Claude agent view ("When you attach, Claude posts a short recap of what happened while you were away") | Re-entry cost after a context switch | https://code.claude.com/docs/en/agent-view |
| Model-written one-line row summaries | Claude agent view (Haiku-class model, ≤ every 15 s heuristic, rewritten at turn end); One ("excerpts of agent conversations go … to write titles, recaps and next steps") | Row readable without opening the transcript | https://code.claude.com/docs/en/agent-view ; https://getone.one/ |
| Jump to the exact pane/window | AgentManager ("the exact pane in iTerm2, the right tab in Terminal and Ghostty, the project window in your IDE"); One ("Move to herdr and send"; Automation permission to "find and focus agent panes") | Finding the window is itself the cost | https://agentmgr.app/ ; https://getone.one/support |

## 4. Anti-patterns and complaints

Sources are GitHub issues (reaction counts as of 2026-10-07) and product pages that name the problem; HN/Reddit search providers
were rate-limited during this recon, so Reddit threads are not cited.

1. **Missing the prompt that blocks the agent.** "Claude sits on 'proceed?' behind whatever you're doing" and "You cycle through tabs
   just to see if anything stopped" — AgentManager's own problem statement (https://agentmgr.app/). openai/codex#2961 (14 reactions):
   "the notify configuration only triggers notifications when a session completes, but not when user approval is required … This would
   prevent users from missing approval prompts when switching windows." anthropics/claude-code#15795: sound requested for
   permission prompts "when they're working on other things".
2. **Notifications that fire unreliably** — anthropics/claude-code#9575: "it only fires for approximately 25% of permission prompts
   during a session, with no clear pattern" (closed).
3. **No push on mobile when approval is needed** — anthropics/claude-code#29438 (69 reactions, closed): "You have to keep the app
   open and actively watch the session to notice when it's blocked on a permission prompt … similar to how CI/CD systems notify on
   pipeline approval gates." #35637 (30 reactions, open): permission prompts "do not render on the mobile client. The session appears
   frozen/unresponsive from the mobile side".
4. **Not knowing which agent is waiting / no persistent overview** — openai/codex#9902: "After you enter a thread, there's no persistent
   view of other agents/threads that are still running, blocked, or completed. This creates lots of context switching back to the
   picker and scales poorly as the number of agents grows." openai/codex#10471: "the app does not provide any indication that a
   subagent has been spawned and is being waited on … users are only shown a 'Thinking' indicator".
5. **"Needs input" that cannot be dismissed without spending a model turn** — anthropics/claude-code#82328 (open): "Needs input is
   derived from the session's last message, so today there are only two ways out of it: reply … or delete the job … The first burns a
   model round-trip and tokens purely to change a label. The second loses history." Lesson: an inbox needs an explicit clear/dismiss
   (One's `E clears one`).
6. **Silent death looks like running** — AgentManager: "The silent crash. It died on an API error. You thought it was still running."
   Claude agent view documents the mirror case: a session mid-response at sleep "can come back unresponsive".
7. **Lost context on compaction / re-entry** — AgentManager: "Context compacts mid-task, and the premise is gone"; Claude's answer is
   the attach recap and the `80 %` context warning in AgentManager.
8. **Orphaned processes / no way to manage sessions from outside** — anthropics/claude-code#33979 (closed): "Claude Code has no
   built-in way to manage running sessions from outside the session itself … Orphaned processes eating RAM: #19045 — 20+ orphaned
   subagents after 1 hour, ~400MB each". (Agent view with its supervisor process is the shipped answer.)
9. **Blocked-state misdetection** — herdr admits screen-scraped detection is lossy: "unusual new prompts may initially show as `idle`
   instead of `blocked` until Herdr learns that screen shape"; Codex "falls back to `unknown` because its title and composer can look
   the same during an active turn and after a response" (https://herdr.dev/docs/agents/). One's connector guards the typed digit by
   "checking the question is still the one you answered" — stale-answer risk is real.
10. **Permission prompts not answerable from the overview** — Claude agent view: "A permission prompt or another dialog … replying
    doesn't answer it. Your reply waits in the queue. To answer the dialog, attach with →" — a documented hole that pushes the user
    back into the full window (https://code.claude.com/docs/en/agent-view).
11. **Review becomes the bottleneck once agents run in parallel** — Vibe Kanban: "the coding agents work on infinite tasks in
    parallel, but you still need to plan and review that work as a human"; Cursor 2.0: "two new bottlenecks emerge: reviewing code
    and testing the changes" (https://www.vibekanban.com/ ; https://cursor.com/blog/2-0).
12. **Permission and privacy cost of a system-wide layer** — One needs Microphone, Screen Recording (re-confirmed "roughly once a
    month" on macOS 15), Accessibility and Automation, and its analytics have "no switch to turn them off"; Option double-tap conflicts
    with "window managers and text expanders" (https://getone.one/support ; https://getone.one/privacy).
13. **Quota drain from many parallel sessions** — Claude agent view: "Each session uses your subscription quota independently, so see
    Limitations before dispatching many at once"; anthropics/claude-code#13354 (215 reactions, open) "Continue when the session limit
    reached".

## 5. Implications for theuth

(≤ 15 bullets; "has" refers to the surfaces named in the brief — Attention inbox 104, decision/review packets 101/103, point-and-say
96, island 53, subagent strip 92, HUD 70.)

1. **Table stakes (every serious product has them):** a per-agent state list grouped by Needs-input / Working / Done, a one-line
   model-written summary per row, notification on needs-input and finish, diff review reachable from the list, phone follow-up.
   theuth has the list (subagent strip, HUD, Agents pages), the inbox (Attention 104) and the phone link client; the one-line
   summary per agent and a strict "blocked" classification are what the rail would add.
2. **Differentiators One actually owns:** the surface is OUTSIDE every window (rail + Inbox + Option double-tap), answers are one
   keystroke with a 2-second Esc undo, finished work carries two ready replies, and the layer "shows a receipt … and stops". No other
   product read combines these; Claude agent view has the keystroke answer and suggested reply but lives in a terminal.
3. **Build on the Attention inbox, do not duplicate it:** One's Inbox = Attention's approvals + questions + finished-work items; the
   companion should be a second renderer of the same queue with keystroke semantics (1/2/3, J/K, E, Esc-undo), not a new store.
4. **Packets (101/103) are already "finished work with a recommendation"** — map One's "Next steps: up to two replies" onto the
   packet's options/recommended item; a review packet's "Walk through" is the diff-review hook the rail needs.
5. **Point-and-say (96) and the island (53) are the "Point at it" and rail primitives** — the Router can reuse the voice key and the
   notch panel instead of adding a third global-hotkey path; note One's collision warning about Option-key capture.
6. **Permission prompts must be answerable from the rail** — Claude agent view's documented hole ("replying doesn't answer it …
   attach") is the thing users complain about most (#2961, #29438, #35637); theuth's approvals already flow through Attention, so the
   rail can answer them where Claude's cannot.
7. **Dismiss without a model turn** (#82328) — every inbox item needs an explicit clear (`E`) that changes only the label.
8. **Stale-answer guard** — copy One's connector rule: deliver a keystroke answer only after re-checking the question id still
   matches; with a 2-second undo the check is cheap.
9. **"Needs you" vs "FYI" strictness** — herdr marks `blocked` only on known approval/question/permission UI; theuth has structured
   events, so blocked should be exact, and FYI (finished, failed) should never interrupt unless the user opts in. Quiet hours and
   grouping are undocumented everywhere — a free differentiator if done.
10. **Receipts** — when the Router or an auto-reply types into an agent, show what was sent and to whom (One's FAQ wording); theuth's
    tool-call cards are the natural receipt.
11. **Re-entry recap** — Claude's attach recap and AgentManager's 80 % context warning address the "lost context" complaint; a rail row
    expanded should show "what happened since you last looked".
12. **Traps:** (a) requiring Screen Recording/Accessibility for a first-run experience (One needs four permissions and re-prompts
    monthly on macOS 15); (b) screen-scraped state detection (herdr's `idle`/`unknown` fallbacks) — theuth should use engine events;
    (c) an inbox state derived from "last message" (#82328); (d) a second window that competes with the main one (Codex #9902's
    "context switching back to the picker") — the rail must be non-activating like the island; (e) always-on analytics without a
    switch (One privacy) — avoid.
13. **Multi-machine** is expected: One (8 servers), herdr, Superset, Emdash all list remote agents in the same list; theuth's mesh
    engines/sandboxes should appear as rows with a machine tag, not as a separate view.
14. **Swift vs webview:** One's observable UX that is hard in a webview is the non-activating rail, the modifier double-tap, the drag
    rectangle over the screen and the typed digit into other apps — all AppKit-level; the Inbox cards themselves are ordinary list UI
    and already exist in React (Attention), so the native part can be thin.
15. **Pricing/positioning fact for the design:** One charges for the AI layer (titles, recaps, next steps, routing) while agents stay on
    the user's own plans — theuth already has those models; the companion needs no new provider dependency.
