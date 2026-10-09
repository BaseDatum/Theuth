# herdr — field and user evidence (2026-10-09)

## Summary
- herdr (herdrdev/herdr, Apache-2.0 since the July 2026 relicense, v0.9.3 on 2026-09-29) is a VC-backed company now: YC (2026-08-06) and a $6M seed led by Bessemer (2026-09-08), still "the only person behind Herdr" plus bots and a handful of contributors at the time of the YC post ([blog](https://herdr.dev/blog/herdr-is-joining-y-combinator/), [seed](https://herdr.dev/blog/herdr-raised-a-seed/)).
- Traction is real and fast: 42,981 stars, 3,369 forks, 316 open issues, created 2026-03-27 ([GitHub API](https://api.github.com/repos/herdrdev/herdr)); the site claims 1,293,735 installs, 1,548 community plugins, 22 agent CLIs detected ([herdr.dev](https://herdr.dev/)). Three HN front pages (404, 281, 166 points).
- Users' loudest complaints are not about the agent model: a 60 Hz render cap (#1134, 32 👍), the AGPL→Apache relicense (#1340, 29 👍/39 comments), Windows input/mouse leaks (six of the top-20 by comments), and server CPU (#4506, "10–13 cores for hours" with 20 idle Claude panes).
- The agent-state axis (A1/A2) has a steady stream of precise bug reports: status latching on `working` (#198, 23 comments), AskUserQuestion shown as `idle` when Claude runs `--agent` (#4573), nix-wrapped Claude invisible (#803, 11 👍), Codex 0.157 shared daemon breaking per-pane identity (#4649), reported labels scoped to pane not process (#4732). Users rely on `herdr agent explain` output in their reports — explainability is load-bearing.
- herdr's "add herdr support" contract (env vars + `pane report-agent` with `--seq` and `resume_argv`) has been adopted by five agents (Command Code, Crush, Kiro CLI, Muse Code, Prime Agent) per the docs; Prime Agent's built-in reporter half-works in the field (#3184).
- The field has converged on hooks (Claude `Notification`/`Stop`, Codex `notify`) for state, and on OSC 9;4 as the only machine-readable terminal state channel — emitted by Claude Code (allow-listed terminals only) and Kimi, not by Codex (openai/codex#37032 open since 2026-08-05, zero maintainer comments). herdr itself is one of very few hosts that persists agent processes across its own UI; cmux, Superset, Conductor restore layout/sessions, not processes.
- Nothing comparable combines herdr's three properties (semantic state + process-owning server + agent-drivable API) in one open tool; getone.one "One" depends on herdr for exactly those.

## What herdr promises (site)
**Home** ([herdr.dev](https://herdr.dev/)): "Run them anywhere. Leave them running." — "Walk away and they keep working. Come back from any machine and they're where you left them." Counters: 42,441 stars · 1,293,735 installs · 1,548 community plugins · 22 agent CLIs detected. One-line install, macOS/Linux/Windows, Apache 2.0. No pricing or enterprise page exists (`/pricing/` serves the home page) — monetisation is unannounced.

**Compare** ([herdr.dev/compare](https://herdr.dev/compare/)): the matrix's sorting row is "Work survives its own UI closing": herdr "yes, the server owns the terminals"; tmux/zellij "yes, detach"; cmux/Warp "session restore"; Conductor/Emdash/Superset "while the app is open". Semantic state "blocked · working · done · idle" vs "attention cues" (cmux/Warp) and "workspace status" (managers). "Direct attach to one agent: yes" is claimed unique. API: "read · send · wait · split · attach".

**Concepts** ([docs](https://herdr.dev/docs/concepts/)): five states — `blocked` (needs input/approval/decision), `working`, `done` (finished, not yet looked at), `idle` (seen), `unknown`. "Each client tracks which completions it has displayed"; CLI/API use server-side seen state; "both `idle` and `done` mean ready for input." Server owns panes; multiple clients can view different tabs; last interacting client controls a shared tab's sizes.

**Agents** ([docs](https://herdr.dev/docs/agents/)): 23 "supported by Herdr" agents in the table (Claude Code, Codex, Copilot CLI, Cursor Agent, OpenCode, Pi, OMP, Droid, Devin, Kimi, Kilo, Hermes, Qoder, Qwen, Letta, MastraCode, Grok, Antigravity, Amp, Kiro, Maki, Gemini, Cline); 5 "supported by the agent" (Command Code, Crush, Kiro CLI, Muse Code, Prime Agent). "Herdr finds the agent's process in each pane and reads the live bottom of the pane … Rules in a detection manifest decide." "Blocked detection is deliberately strict": only known approval/question UI shapes; unknown prompts fall back to `idle` labelled `default_known_agent_idle_fallback`. Manifests are updated remotely from herdr.dev without restart (`[update] manifest_check = false` to disable); local overrides in `~/.config/herdr/agent-detection/<agent>.toml` always win. `HERDR_AGENT=<agent>` hint for sandbox wrappers; `HERDR_PROCESS_DETECTION=child-groups` opt-in.

**Integrations** ([docs](https://herdr.dev/docs/integrations/)): "Every integration tells Herdr which session the agent is in, so Herdr can resume it after a server restart." Only Pi, OMP, Kimi, OpenCode, Kilo, MastraCode report *state*; Codex reports turn state only with `--no-daemon`; Claude/Copilot/Devin/Droid/Qoder/Qwen hooks report *identity only* — "Claude Code state comes from Herdr's screen manifest detection." Claude integration v10 matches `SessionStart` sources `startup|resume|clear|compact|fork`. Integration versions are pinned per agent (Claude v6, Codex v5, OMP v3 …).

**Add Herdr support** ([docs](https://herdr.dev/docs/add-herdr-support/)): four env vars (`HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_BIN_PATH`, `HERDR_SOCKET_PATH`); three reports via `"$HERDR_BIN_PATH" pane report-agent` — state (`--state idle|working|blocked`, `--message`, `--source`, `--seq` monotonic "a timestamp works well"), resume command (`-- my-agent --resume $ID`, rules: first word a bare command on PATH, no apostrophes/control chars, ≤64 args / 8 KiB, else `invalid_resume_argv`; source must hold the pane first or `resume_not_accepted`), release on exit. Safety net: "If your agent exits without releasing, Herdr notices once the pane is back at its idle shell prompt." "Resume commands need Herdr 0.9.2 or later."

**Session state** ([docs](https://herdr.dev/docs/session-state/)): the four-row survival table — detach/reattach keeps processes; server restart loses processes, restores layout, screen only with `[experimental] pane_history = true` (off by default "because pane output can include secrets"), conversation only via native agent resume; `--handoff` update "best effort" keeps processes. systemd-logind shutdown inhibitor on Linux; 48 layout snapshots at ≤1 per 15 min; three recovery copies of unreadable `session.json`.

**Connecting machines** ([docs](https://herdr.dev/docs/connecting-machines/)): Local + saved SSH machines in one window; "The agent list includes connected machines"; lost connection shows cached state dimmed; reconnect backoff up to two minutes; Herdr does not enable `ForwardAgent`; IDs and agent names are scoped per server ("Two machines may both contain `w1:p1`").

**Agent automation / Socket API** ([automation](https://herdr.dev/docs/agent-automation/), [socket](https://herdr.dev/docs/socket-api/)): `agent start` returns only after detection sees the agent ready; `agent prompt --wait` refuses an already-`blocked` agent (`agent_blocked`); `agent wait --until blocked`; `pane wait-output --regex`; "Wait commands have no default timeout"; "`agent.wait` is server-owned and event-driven. It pins the resolved pane occupant so a replacement cannot satisfy the wait." `herdr api schema --json` prints the full JSON Schema covering requests, errors and events; `events.subscribe`/`events.wait`. Plugins declare `min_herdr_version`, event hooks validated against known event names at link time.

**Marketplace** ([docs](https://herdr.dev/docs/marketplace/)): "Share plugins from GitHub today and tag your repo to be listed when the marketplace launches" — the marketplace is not launched; the site's 1,548 count is GitHub-tagged repos.

## Traction
- Stars 42,981 · forks 3,369 · open issues 316 · created 2026-03-27 · pushed 2026-10-08 · discussions enabled ([api](https://api.github.com/repos/herdrdev/herdr)). Issue numbers reached #4814 by 2026-10-01 — roughly 25 issues/day over six months.
- Releases: v0.9.3 2026-09-29, v0.9.2 09-29, v0.9.1 09-16, v0.9.0 09-07, v0.8.2 08-19, v0.8.0 08-03, v0.7.5 07-21 — a stable every ~2 weeks plus near-daily `preview-YYYY-MM-DD-<sha>` prereleases ([releases](https://github.com/herdrdev/herdr/releases)).
- Contributors: ogulcancelik (Can), JJLiebig, akbash-bot, kangal-bot, github-actions[bot], minatoaquaMK2, dhh, caner-akca … ([contributors](https://api.github.com/repos/herdrdev/herdr/contributors)) — two bot accounts commit, consistent with an agent-run repo. Sponsors: not verified.
- Funding: YC announced 2026-08-06 ("it's Can, the only person behind Herdr"; "the TUI was never meant to be the only client") ([blog](https://herdr.dev/blog/herdr-is-joining-y-combinator/)); $6M seed led by Bessemer with YC, e2vc, Tobi Lütke, Dane Knecht, Görkem Yurtseven, 2026-09-08; hiring via `ssh` ([blog](https://herdr.dev/blog/herdr-raised-a-seed/)).
- Ecosystem spawned: Herdr Studio (HN 45 pts), NotchAgent/agsig, ColliePWA (mobile PWA over herdr/tmux/zellij, "started off as a weekend experiment right when herdr released the functionality to write custom plugins") ([HN](https://news.ycombinator.com/item?id=49775454)).

## User evidence (issues, by axis A1–A9)
Reactions = total reaction count; c = comments. Source: [GitHub search](https://github.com/herdrdev/herdr/issues?q=sort%3Areactions-%2B1-desc).

**A1 Agent state detection**
- Agent status latches on working and never returns to idle after a turn ends · #198 · 0/23 · closed · "a pane reaches idle at the end of a completed turn, then flips back to working on its own with no user or agent input … stuck working since the previous day" · https://github.com/herdrdev/herdr/issues/198
- Agent detection misses nix-wrapped Claude Code (.claude-unwrapped) · #803 · 11/16 · **open** · process name is the wrapper target; "The `SessionStart` hook still reports the session … it looks half-wired" · https://github.com/herdrdev/herdr/issues/803
- Claude Code AskUserQuestion dialog reports idle when the session runs as a custom agent (--agent), Claude Code 2.1.281 · #4573 · 4/7 · open · `herdr agent explain` shows `rule: osc_title_idle (region=osc_title priority=250) evidence: "✳ ui-tester"` — a title rule outvotes the visible question · https://github.com/herdrdev/herdr/issues/4573
- claude code is setting up the monitor and waiting, but the agent state in the header shows … · #1217 · 6/8 · closed · https://github.com/herdrdev/herdr/issues/1217
- omp no have running status · #1618 · 6/3 · closed · https://github.com/herdrdev/herdr/issues/1618
- omp panes show no live agent status when omp runs with --profile · #1243 · 4/4 · open · the extension is per-profile dir; copying `herdr-omp-agent-state.ts` into the profile fixes it · https://github.com/herdrdev/herdr/issues/1243
- pi agent detection lost after pi 0.84.3 update on Windows — pane stays unknown · #3186 · 6/12 · closed · https://github.com/herdrdev/herdr/issues/3186
- Reported labels: scope them to the process, name the `--source` on `agent get`, and let `agent prompt` address a reported agent · #4732 · 2/2 · open · "A pane reported `working` keeps reading `working` after the process it was reported about has exited" · https://github.com/herdrdev/herdr/issues/4732
- feat: sort agents panel by attention priority · #318 · 9/6 · closed · https://github.com/herdrdev/herdr/issues/318
- Notification stays and never goes away · #1827 · 5/7 · closed · https://github.com/herdrdev/herdr/issues/1827

**A2 Agent integrations & session identity**
- Codex 0.157 shared daemon breaks per-pane sessions and HERDR_* env (waiting on upstream) · #4649 · 5/5 · open · pane `working` but `agent_session` null; "the dependent status writer correctly refuses to bind a transcript to this pane without an authenticated session identity" · https://github.com/herdrdev/herdr/issues/4649
- Codex integration reports sessions to inherited or stale panes · #4814 · 3/1 · open · https://github.com/herdrdev/herdr/issues/4814
- Prime Agent built-in herdr reporter: state reports work, session/identity don't · #3184 · 3/8 · open · "State reports ARE accepted from any client (no pane-PID check)" · https://github.com/herdrdev/herdr/issues/3184
- 0.9.2: Claude Code stops emitting OSC 8 links after `TERM_PROGRAM=herdr` change · #4748 · 2/1 · open · herdr's own TERM_PROGRAM value changes what agents emit · https://github.com/herdrdev/herdr/issues/4748
- Remote sessions don't appear in `agents` section · #1170 · 18/7 · closed · https://github.com/herdrdev/herdr/issues/1170

**A3 Durability**
- `SSH_AUTH_SOCK` does not get updated when detaching and reconnecting with `--remote` · #1931 · 6/8 · closed · https://github.com/herdrdev/herdr/issues/1931
- Local Wayland clipboard unavailable in panes after reattaching to a persistent session · #2448 · 4/5 · open · https://github.com/herdrdev/herdr/issues/2448
- Feature: support mosh transport for herdr --remote (survive sleep / roaming) · #1779 · 5/1 · closed · https://github.com/herdrdev/herdr/issues/1779
- Updater announces 0.8.1 with a download URL that returns 404 · #2964 · 4/2 · closed · https://github.com/herdrdev/herdr/issues/2964
- macOS: herdr server has no grantable app identity, so panes lose Local Network access · #808 · 3/15 · open · https://github.com/herdrdev/herdr/issues/808

**A4 Agent-native control API**
- Add workspace.reorder (or workspace.move) to socket API + CLI · #323 · 17/3 · closed · https://github.com/herdrdev/herdr/issues/323
- `herdr agent focus` no longer moves the attached client's viewport (regression in 0.9.0) · #3760 · 6/7 · closed · https://github.com/herdrdev/herdr/issues/3760
- endpoint_busy locks the whole client after a slow "Delete worktree checkout" times out · #4578 · 3/1 · open · https://github.com/herdrdev/herdr/issues/4578

**A5 Supervision UX**
- 60Hz Limit · #1134 · **32**/3 · open · top-reacted issue: "Input echo and rapid pane updates should remain responsive on 120 Hz to 144 Hz displays" · https://github.com/herdrdev/herdr/issues/1134
- `theme = "terminal"` unreadable with Ghostty light theme · #1307 · 16/3 · open · https://github.com/herdrdev/herdr/issues/1307
- Temporary floating popup panes for plugins · #1125 · 15/11 · closed · https://github.com/herdrdev/herdr/issues/1125
- Can't navigate in the Tab name dialog · #1803 · 14/15 · closed · https://github.com/herdrdev/herdr/issues/1803
- Copy mode: allow held-key repeat for cursor-movement keys · #2371 · 11/5 · open · https://github.com/herdrdev/herdr/issues/2371
- feat: configurable agent status dots on tab bar labels · #303 · 6/1 · closed · https://github.com/herdrdev/herdr/issues/303
- Option to hide the tab bar when a workspace has only one tab · #448 · 7/11 · closed · https://github.com/herdrdev/herdr/issues/448

**A6 Multi-machine**
- machine add fails after SSH MFA on 0.9.0 · #3763 · 9/9 · closed · https://github.com/herdrdev/herdr/issues/3763
- Remote attach UI becomes sluggish in 0.9.0 · #3769 · 8/14 · closed · https://github.com/herdrdev/herdr/issues/3769
- Excessive remote bandwidth during CLI animations (~3.6 GB/hour per pane) · #4264 · 3/2 · open · https://github.com/herdrdev/herdr/issues/4264
- 0.9.0 regression: terminal popup typing over SSH sends ~234 KB per key · #3745 · 5/4 · closed · https://github.com/herdrdev/herdr/issues/3745
- herdr --remote against a Windows OpenSSH target fails with a raw PowerShell error · #3651 · 0/19 · closed · https://github.com/herdrdev/herdr/issues/3651
- machine add fails against tcsh login shells · #4258 · 3/3 · open · https://github.com/herdrdev/herdr/issues/4258

**A7 Terminal engine & rendering**
- Server burns 10+ CPU cores with many populated agent panes · #4506 · 2/15 · closed (pending-release, p1) · "about 20 agent panes … mostly idle Claude Code sessions, the herdr server process sustains 10-13 CPU cores for hours … p95 latency 90-450 ms" · https://github.com/herdrdev/herdr/issues/4506
- Line wrapped links are not properly highlighted and clickable · #1282 · 18/6 · closed · https://github.com/herdrdev/herdr/issues/1282
- Neovim wrapped rows show stale/duplicate cells in herdr's retained grid · #3329 · 10/7 · closed · https://github.com/herdrdev/herdr/issues/3329
- Revealing a pane re-queries all 256 palette colors, freezing the pane for seconds · #3266 · 6/18 · closed · https://github.com/herdrdev/herdr/issues/3266
- Clipboard copy silently fails when outer terminal ignores OSC 52 · #2399 · 8/16 · closed · https://github.com/herdrdev/herdr/issues/2399
- Host appearance changes are never propagated to pane children (OSC 11 + CSI ?996n frozen) · #4183 · 5/8 · open · https://github.com/herdrdev/herdr/issues/4183
- 0.8.2 local attach: DECRQM answers ?1016 SET but mouse reports stay cell-coordinate · #3295 · 4/22 · closed · https://github.com/herdrdev/herdr/issues/3295
- Windows input cluster, all closed, 15–22 comments each: multi-line paste submits at first newline #4251; empty bracketed-paste decode #4314; Enter dropped after long input #3187; mouse reporting left on after detach #3748; SGR mouse tail leaks #3911; Codex exit leaks CSI-u #2296; "suddenly cannot work properly on win11" #3430.
- Pane images (kitty graphics) do not render from iTerm #3941 · 3/2 · open.

**A8 Workspaces, worktrees & plugins**
- Support placeholders in [worktrees].directory for sibling-folder layouts · #261 · 24/11 · closed · https://github.com/herdrdev/herdr/issues/261
- Cannot delete workspace with submodules · #1797 · 8/15 · closed · https://github.com/herdrdev/herdr/issues/1797
- Feature Request: Jujutsu support · #218 · 7/2 · closed · https://github.com/herdrdev/herdr/issues/218
- Plugins installation across sessions · #1174 · 6/4 · closed · https://github.com/herdrdev/herdr/issues/1174
- Keybound plugin actions never receive selected_text · #3380 · 5/1 · open · https://github.com/herdrdev/herdr/issues/3380

**A9 Engineering practice**
- Relicensing herdr from AGPL-3.0 to Apache-2.0 — I need your permission · #1340 · 29/39 · closed · "Their company's open-source policy rejects AGPL outright … I want herdr to be loud" · https://github.com/herdrdev/herdr/issues/1340
- Windows support? · #134 · 3/14 · closed (planned → shipped as windows-beta) · https://github.com/herdrdev/herdr/issues/134
- Issue template forces "I confirm this is a reproducible bug … I reproduced this bug on the version and environment reported below"; labels `maintainer-needed`, `needs-upstream-fix`, `pending-release`, `p1/p2`, `agent-detection` are in active use (seen on #4506, #4649, #4732).

## Discussions
- HN "Herdr: One terminal to rule them all" — 404 points / 178 comments, 2026-07-02 ([thread](https://news.ycombinator.com/item?id=48756578)). Recurring: tmux can already do it with hooks — "four windows each with a status indicator … blue animated braille spinner when it is working, a red dot when it needs attention and a green tick when finished. Claude code exposes a bunch of hooks"; "the application can also try to rename the terminal window title with the OSC 2 escape sequence." Praise: "I can reconnect to it from multiple sessions"; "the killer feature is the git worktree management … tmux on steroid"; complaint: WezTerm selection cleared by constant redraw. jj/non-git VCS asked ([discussion 522](https://github.com/ogulcancelik/herdr/discussions/522)).
- HN "Herdr is joining Y Combinator. The runtime stays open" — 281 / 189, 2026-08-06 ([thread](https://news.ycombinator.com/item?id=49201003)). Dominant worry is enshittification: "the whole point of the funding is to undermine competitors with subsidized/free products and then raise prices"; counter "if you're a herdr user you're perfectly capable of maintaining a fork." cmux's maker appears in the thread.
- HN "Herdr: Agent multiplexer that lives in your terminal" — 166 / 110, 2026-06-29 ([thread](https://news.ycombinator.com/item?id=48714802)). The state-detection debate in miniature: "You use an agentic harness which terminates with bell character. This lights up tmux"; rebuttal "silence for N seconds is just that, the poll doesn't know if that really means waiting for input or something else."
- Also: "Herdr Studio" 45/20 ([49652188](https://news.ycombinator.com/item?id=49652188)); "Herdr Raised a $6M Seed" 7/0; "Herdr now connects to multiple servers with a single client" 4/3; NotchAgent 6/1.
- Reddit: not verified — reddit.com and old.reddit.com both returned challenge pages to this harness; search_web was degraded (Brave rate-limited, DDG/Google/Qwant blocked). X: not verified beyond @herdrdev link on the site.

## Field
| tool | state detection | survives restart | multi-machine | agent-drivable | source |
|---|---|---|---|---|---|
| herdr | process + screen-manifest rules on live bottom buffer; OSC title rules; integration hooks (state for Pi/OMP/Kimi/OpenCode/Kilo/MastraCode, identity-only for Claude/Codex-daemon/Copilot/…); `agent explain` | detach keeps processes; server restart = layout + native agent resume; `--handoff` best effort | saved SSH machines, combined agent list | CLI + socket, `agent wait`, `agent prompt --wait`, events, JSON schema | [agents](https://herdr.dev/docs/agents/), [session-state](https://herdr.dev/docs/session-state/) |
| claude-squad | tmux pane per instance; status heuristics (not documented); `--autoyes` experimental | tmux keeps processes; `r` resume paused | no | no API | [README](https://github.com/smtg-ai/claude-squad) |
| agent-deck | "Smart polling" → Running/Waiting/Idle/Error; waiting sessions in tmux status bar; Telegram/Slack/Discord remote | tmux; `R` restart; `switch-account` migrates conversation | "remote deck" rows, `docs/REMOTE-COMMANDS.md` | CLI, MCP manager, MCP socket pool | [README](https://github.com/asheshgoplani/agent-deck) |
| cmux | OSC 9/99/777 notifications + `cmux notify` CLI wired into agent hooks; sidebar shows latest notification text | restores layout, cwd, scrollback "best effort"; "does not checkpoint arbitrary live process state"; `cmux local-tmux` opt-in; hooks save native session IDs for 13 agents | `cmux ssh user@remote` workspace | CLI + socket API (workspaces, splits, keystrokes, browser) | [README](https://github.com/manaflow-ai/cmux) |
| Conductor | workspace status (per herdr compare); cloud + iOS + API | "while the app is open" (herdr's claim; Conductor runs in the cloud so unverified) | cloud | Conductor API `create_session` | [conductor.build](https://conductor.build/) |
| Superset | installs lifecycle hooks/command wrappers into each agent's config; "status only works for agents launched through Superset"; Claude reports finished+waiting, others completion only | "Running and PR state persist across restarts"; terminals "persistent sessions that survive restarts"; Resume Args per agent | connect another machine; iPhone (Pro, Remote Access) | CLI, TypeScript SDK, MCP server | [agent-status](https://docs.superset.sh/agent-status), [README](https://github.com/superset-sh/superset) |
| Vibe Kanban | executor/task status in a web kanban; MCP server | server process; tasks in DB (process survival not documented) | server-hosted | MCP | [README](https://github.com/BloopAI/vibe-kanban) |
| AgentManager (agentmgr.app) | watches Claude Code sessions; dot shows *why* waiting (yes/no, plan, choice); red "!" on API-error death; menu bar count; sound | observer only (sessions live in your terminal/IDE) | no | no | [agentmgr.app](https://agentmgr.app/) |
| getone.one "One" | runs agents inside herdr; Inbox/Toolbar/Router/iPhone; requires herdr | via herdr | "Connect your machines", remote servers | — | [getone.one](https://getone.one/) |
| Zellij | no agent state; 0.44.0 adds remote sessions, Windows, CLI automation (2026-03-23) | detach/resurrect | remote sessions | CLI/plugin API | [news](https://zellij.dev/news/) |
| tmux (HN folklore) | bell / OSC 2 title / hooks → pane colour | detach | ssh | scripting | [HN](https://news.ycombinator.com/item?id=48714802) |
| Warp | "attention cues" per herdr compare; docs page not fetched (404) — **unverified** | session restore | — | app APIs | [compare](https://herdr.dev/compare/) |
| Ghostty | renders OSC 9;4 progress (≥1.2.0) and OSC 9/777 notifications; no agent concept | — | — | — | [OSC 9](https://ghostty.org/docs/vt/osc/9) |
| Tabby plugin tabby-ai-progress | OSC 9;4 native; Codex via title adapter; OpenCode/Gemini/Aider "detection only — no invented title rules" | n/a | n/a | n/a | [README](https://github.com/stufently/tabby-ai-progress) |

## Agent → terminal state conventions
- **OSC 9;4 progress** (ConEmu origin): `ESC ] 9 ; 4 ; <state> [; <progress>] BEL`, states 0 clear, 1 determinate, 2 error, 3 indeterminate, 4 paused ([Windows Terminal](https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences)). Rendered by Windows Terminal ≥1.6, Ghostty 1.2.0, iTerm2 3.6.6, kitty 0.38→0.47, VS Code 1.97 as `${progress}`; foot discards; Alacritty won't fix ([tabby-ai-progress table, verified 2026-08-31](https://github.com/stufently/tabby-ai-progress)). Disambiguation: Ghostty says a notification title "should not begin with a number and then a semicolon" so `9;4` always parses as progress ([ghostty](https://ghostty.org/docs/vt/osc/9)).
- **Who emits it**: Claude Code — yes, but only when `TERM_PROGRAM`/`TERM_PROGRAM_VERSION` match a hard-coded allowlist (ConEmu, Ghostty ≥1.2.0, iTerm2 ≥3.6.6) and `terminalProgressBarEnabled` (undocumented setting); the allowlist was removed in 2.0.53 ("garish progress bar", anthropics/claude-code#12405) and reinstated version-aware ([tabby README](https://github.com/stufently/tabby-ai-progress)). Kimi CLI 0.31.1 — yes. Codex CLI — no: openai/codex#37032 "Emit OSC 9;4 progress reports so terminals can surface working/idle state" open since 2026-08-05, "zero maintainer comments"; the filer "build[s] a terminal workspace that hosts coding agents in PTYs and parses OSC 9;4" ([issue](https://github.com/openai/codex/issues/37032)). Herdr #4748 shows the flip side: herdr sets `TERM_PROGRAM=herdr`, so Claude's allowlist gating turns features off inside herdr.
- **Window title (OSC 0/2)**: Codex writes structured state into the title by default — braille spinner = working, `[ ! ] Action Required | project` = attention; `[tui] terminal_title = ["run-state","task-progress","project"]` yields literal `Working/Thinking/Waiting/Ready/Starting` and `Tasks 3/12` ([tabby README](https://github.com/stufently/tabby-ai-progress)). Claude Code's title (`✳ <agent-name>`) is what herdr's `osc_title_idle` rule reads — and mis-reads in #4573.
- **OSC 9 / OSC 777 / OSC 99 notifications**: Codex `[tui] notifications = ["agent-turn-complete","approval-requested"]`, `notification_method = auto|osc9|bel` ([danielvaughan, updated 2026-10-08](https://codex.danielvaughan.com/2026/04/13/codex-cli-notification-pipeline-osc9-hooks-alerts/)). cmux consumes OSC 9/99/777 as its notification source ([cmux](https://github.com/manaflow-ai/cmux)). Claude Code's native notification body "is always just 'Claude is waiting for your input' with no context" (cmux author).
- **Hooks**: Claude Code hook events `SessionStart/SessionEnd`, `UserPromptSubmit`, `Stop`, `StopFailure`, `PreToolUse/PostToolUse/PostToolUseFailure`, `PermissionRequest`, `PermissionDenied`, `Notification` with matchers `permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_url_dialog`, `elicitation_complete`, `elicitation_response`, `agent_needs_input`, `agent_completed`, `quota_auto_resume_*`, plus `SubagentStart` ([hooks reference](https://code.claude.com/docs/en/hooks)). Payload carries `session_id`, `prompt_id`, `transcript_path`, `cwd`, `permission_mode`. Codex external `notify` hook fires only for `agent-turn-complete` (not approvals; openai/codex #11808, #14813) ([danielvaughan](https://codex.danielvaughan.com/2026/04/13/codex-cli-notification-pipeline-osc9-hooks-alerts/)); Codex ≥0.157 runs a shared daemon that breaks per-pane env inheritance (herdr #4649). OpenCode/omp/Pi/Kilo/Kimi expose plugin/extension lifecycle events that herdr's integrations turn into authoritative state ([integrations](https://herdr.dev/docs/integrations/)); omp's is a per-profile extension file (#1243).
- **herdr's own contract** (`HERDR_ENV` + `pane report-agent --seq` + `resume_argv`): adopted natively by Command Code, Crush, Kiro CLI, Muse Code, Prime Agent per the docs ([agents](https://herdr.dev/docs/agents/)); Prime Agent 0.8.0 ships `herdr-agent-state.js` gated on `HERDR_ENV===1 && HERDR_SOCKET_PATH && HERDR_PANE_ID` but its identity reports are rejected (#3184). Superset and cmux run parallel, incompatible hook schemes (`~/.superset/bin` wrappers; `cmux hooks setup`). No cross-vendor standard exists; OSC 9;4 is the closest de-facto one and covers only working/idle, not blocked ("Approval prompts / questions should read as waiting (remove)" — #37032's own proposal collapses blocked into idle).
- Not verified: Warp's agent notification mechanism; Gemini CLI/Cline emission (tabby found "no verified, stable status format").

## What this means for theuth
1. Nobody's screen-reading is clean: herdr, with manifests, remote rule updates and `agent explain`, still ships #198/#4573/#803/#4732. theuth's ACP children already give exact state; the hard part is only PTY jobs — treat screen heuristics as fallback with an explain string, exactly as herdr labels `default_known_agent_idle_fallback`.
2. Arbitration must be per-process, monotonic and self-clearing: `--seq`, pane-vs-process scoping (#4732), "safety net clears when the shell prompt returns", and `agent.wait` pinning the pane occupant are the three rules users have forced on herdr.
3. If theuth hosts raw agent PTYs, read OSC 9;4 and titles, but spoof-gate carefully: Claude only emits with an allow-listed `TERM_PROGRAM`, and setting your own value (herdr's `TERM_PROGRAM=herdr`) silently drops features (#4748).
4. Hooks beat terminals for Claude Code (`Notification: permission_prompt|idle_prompt|agent_needs_input`, `Stop`, `PermissionRequest`); Codex needs `--no-daemon` or title parsing; approvals never reach Codex `notify`.
5. Durability is the differentiator herdr sells and the market buys ("Work survives its own UI closing"); for One-compatibility theuth either runs its PTY jobs inside herdr or implements herdr's `report-agent`/`resume_argv` contract so One's inbox can see theuth children.
6. Users punish rendering and input regressions far more than detection gaps (60 Hz, Windows paste, 10-core idle server) — a multi-client render path must be cheap when panes are idle.
7. Licensing and VC fear are live in this community; theuth's plugin/host story should not depend on herdr's roadmap (no pricing yet, marketplace unlaunched).

## Sources
1. https://api.github.com/repos/herdrdev/herdr
2. https://github.com/herdrdev/herdr/releases
3. https://api.github.com/repos/herdrdev/herdr/contributors
4. https://github.com/herdrdev/herdr/issues?q=sort%3Areactions-%2B1-desc (and `sort:comments-desc`, `is:open`)
5. https://github.com/herdrdev/herdr/issues/198 · /803 · /1134 · /1243 · /1340 · /3184 · /4506 · /4573 · /4649 · /4732 (bodies read)
6. https://herdr.dev/ · https://herdr.dev/compare/ · https://herdr.dev/pricing/ (serves home)
7. https://herdr.dev/blog/herdr-is-joining-y-combinator/
8. https://herdr.dev/blog/herdr-raised-a-seed/
9. https://herdr.dev/docs/concepts/ · /agents/ · /integrations/ · /add-herdr-support/ · /session-state/ · /persistence-remote/ · /connecting-machines/ · /agent-automation/ · /socket-api/ · /plugins/ · /marketplace/ (read from the repo copies at docs/next/website/src/content/docs/*.mdx, HEAD 2563803d)
10. https://hn.algolia.com/api/v1/search?query=herdr · ?query=herdr.dev
11. https://news.ycombinator.com/item?id=48756578 · 49201003 · 48714802 · 49652188 · 49775454
12. https://github.com/ogulcancelik/herdr/discussions/522
13. https://github.com/smtg-ai/claude-squad
14. https://github.com/asheshgoplani/agent-deck
15. https://github.com/manaflow-ai/cmux
16. https://github.com/BloopAI/vibe-kanban
17. https://github.com/superset-sh/superset · https://docs.superset.sh/agent-status · https://docs.superset.sh/agent-integration
18. https://conductor.build/
19. https://agentmgr.app/
20. https://getone.one/
21. https://zellij.dev/news/
22. https://github.com/openai/codex/issues/37032
23. https://codex.danielvaughan.com/2026/04/13/codex-cli-notification-pipeline-osc9-hooks-alerts/
24. https://code.claude.com/docs/en/hooks
25. https://github.com/stufently/tabby-ai-progress
26. https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences
27. https://ghostty.org/docs/vt/osc/9
28. Unreachable from this harness (marked unverified above): reddit.com/old.reddit.com search, docs.warp.dev agent notifications page, Brave/DDG/Google web search (rate-limited/blocked).