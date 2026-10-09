# theuth — OURS on the herdr axes (A1–A9)

_Recon child `herdr-ours`, 2026-10-09, read-only over ~/Develop/theuth (main = origin/main dd19bf63, v2.13.0). Cites are `path#L<a>-L<b>` relative to that root; `branch:path#…` for non-main branches._

## Summary

theuth (v2.13.0, main dd19bf63) is a meta-harness: it orchestrates agents through ACP and its own
sessions, and its terminal layer exists for the *model* (`start/send/screen/wait`) rather than for a
human watching panes. Measured on herdr's nine axes:

**Three biggest gaps against a herdr-class runtime**
1. **A3 — nothing a PTY or shell job does survives the engine.** Jobs are ruled non-durable (109 D14);
   an update relaunch is a graceful quit with `KillAll`; no fd handoff, no detached keeper (§14 rejected
   for now). The orchestrator has recovered omp workers by hand six times in nine days (`omp --resume`
   in a fresh PTY, one resume line each).
2. **A1 — no per-program "blocked" detection for terminals.** The only PTY signal is "screen unchanged
   for N s" (61) and it wakes the model, never the human; OSC titles/progress are stripped; a PTY omp
   stuck on its own permission prompt looks exactly like one that finished. ACP children do carry
   harness-reported states (141), but only omp/opencode speak ACP — no Claude Code, Codex, Gemini.
3. **A4 — no state-wait primitive and no outside CLI.** `send_message{await_reply}` waits for a reply,
   `wait(pattern)` for a regex, `remind_me_in` for time; nothing waits for "that agent is
   idle/blocked/done". `t` has no `agents ls|wait|send`; A2A is off by default.

**Three places theuth is already ahead**
1. **A3/A6 — turn-level durability and a real mesh.** Journal + eager resume + safe replay,
   drain-and-suspend, 22-row crash matrix; owner-scoped replication across engines, mid-turn transfer,
   nodes, sandbox VMs, phones via relay — herdr has SSH panes.
2. **A5 — a typed attention model with 14 kinds** rendered by a native macOS companion (rail,
   one-keystroke answers, 3 s undo, breakpoint notifications), a Windows companion, HUD, Slack/Telegram
   boards and TUI, with rank-ordered rows and versioned dismissal.
3. **A2/A8 — the harness is a peer, not a pane:** `session/load` resume with delta hand-offs, skills
   pushed into the child's cwd and served over an MCP back-channel, theuth-to-theuth `_meta`, a
   tree-sitter code index and an extension model (skills, MCP, apps, routines, agents) richer than a
   plugin marketplace. Engine-managed worktrees are the one A8 miss (140 has the only git writer).

## A1 Agent state detection

**Built**

- Three populations, three detectors, no common "blocked" vocabulary for PTYs:
1. *theuth sessions and theuth subagents* — `AgentState` (design 19 §2.1) is `Idle | Running |
     Awaiting{from} | Approval{tool} | Error | Cancelled` (`crates/agent/src/lib.rs#L1391-L1414`). It is
     derived in `Agents::state_of` from four in-memory maps: an unanswered approval wins, then an open
     `send_message{await_reply}` wait, then `running`, else the last stop reason
     (`crates/agent/src/supervisor.rs#L5924-L5955`). "Blocked on a human" therefore exists only for
     approvals (and asks/handoffs via the attention layer); there is no "idle but has unread output"
     state.
2. *ACP children* (omp, opencode over ACP) — the child actor turns a harness
     `session/request_permission` into an `ExternalApproval` through the tool registry gate
     (`crates/agent/src/acp_child.rs#L1261-L1301`), which is what `state_of` reads as `Approval`. A
     harness that keeps working after its prompt returned opens a *continuation turn* so the roster
     shows it busy (`crates/agent/src/acp_child.rs#L935-L969`). Watch-only children a harness announces
     (141) carry the harness's own state: `ChildState` = `Pending | Running | RequiresAction |
     Idle{stop_reason,usage} | Completed | Failed{error}` (`crates/acp/src/harness.rs#L164-L189`),
     stored as `sessions.observed_state` (`ObservedState`, adds `Cancelled | Unknown`,
     `crates/agent/src/lib.rs#L2163-L2185`) and folded into the same `AgentState` by `state_of_row` —
     `RequiresAction` → `Approval`, `Failed` → `Error`, everything else → `Idle`
     (`crates/agent/src/supervisor.rs#L5957-L5982`). So for ACP children theuth already consumes a
     *hook-style report from the agent* (RFD `subagent_update`), the herdr "hooks" source — but only for
     omp/opencode children, not for Claude Code or Codex.
3. *PTY jobs* (any program on a terminal, incl. `omp` driven by `start`/`send`) — the only detector is
     **screen quiet**: design 61's `Idle` struct restarts a timer when the rendered rows differ (cursor
     excluded), is disarmed by firing, and doubles its interval 60 → 120 → … 3600 s while nobody acts
     (`crates/tools/src/pty.rs#L1972-L2038`; constants `IDLE_MIN` 10 s, `IDLE_BACKOFF_MAX` 3600 s,
     `crates/tools/src/pty.rs#L58-L61`). The default is `tools.pty_idle_secs` = 60, `0` = never
     (`crates/tools/src/pty.rs#L264-L267`). The product is a `<job_idle id=… quiet=… nudge=…>` harness
     input that wakes the *model*, not a state the *human* sees (`docs/design/61-pty-idle.md#L20-L49`).
     A `wait` in flight suppresses the fire (`crates/tools/src/pty.rs#L2024-L2029`).
- Readiness is explicit, not inferred: `start.ready` = log regex and/or TCP port with a timeout
  (`crates/tools/src/pty.rs#L270-L281`); `wait(pattern)` is a regex over output since the last `send`,
  30 s default / 120 s max (`crates/tools/src/pty.rs#L305-L316`).
- **No per-program screen-pattern detection and no OSC parsing.** The PTY layer answers a fixed table of
  terminal *queries* (DA, DSR, OSC 10/11 colour) so TUIs do not hang
  (`crates/tools/src/pty.rs#L2257-L2269`) and `strip()` *drops* OSC sequences before pattern matching
  (`crates/tools/src/pty.rs#L2324-L2338`); nothing reads OSC 0/2 titles, OSC 9;4 progress, or
  shell-integration marks. The omp "ready" rule (`○` glyph = waiting at prompt, `●` = busy, banner
  shape) lives in knowledge for the orchestrator to apply by hand, not in code
  (`facts/omp-pty-start-shape-theuth`).
- Arbitration: for ACP children an unanswered permission ask beats the harness-reported state; the
  restart sweep marks non-terminal observed rows `unknown` (`crates/agent/src/lib.rs#L2164-L2166`).
  There is no strictness policy, no confidence, no explainability ("blocked because pattern X matched").
- What the companion/HUD/Slack read: the same `AgentState` word — Slack board maps `RequiresAction` →
  `Approval` (`crates/slack/src/board.rs#L168-L168`), the TUI shows "waiting on you"
  (`crates/tui/src/model.rs#L12104-L12104`); see A5.

**Planned / in flight**

- 141 R4 (branch `fix/141-r4`, worktree `~/Develop/theuth-141r4`) tightens observed-state handling; 146
  D3 gates ACP journaling on "driven" session kinds so observed/advisor children are never respawned
  (branch `fix/146-restart-clarity`, commit 18153397).
- 149 COMPANION-NATIVE (One parity) needs per-agent status dots that say *why* — rulings in
  `decisions/theuth-design-149-companion-native-rulings-2026-10-09`; the engine side has no richer state
  to feed it than the six-word enum above.
- Nothing planned for PTY-side program recognition; design 61 §0 explicitly frames the quiet screen as
  the only signal available.

**Pain (evidence)**

- "omp PTYs never wake you on completion" — the orchestrator polls with reminders and reads `screen()`
  for `○` by eye
  (`procedures/theuth-recover-omp-acp-workers-after-engine-restart-by-pty-resume-2026-10-07`,
  `facts/theuth-runtime-quirks-2026-09-24`).
- Resumed subagents showed idle (○) after a restart while working: `Agents.running` was fed only by
  `TurnStarted` and 109 publishes `TurnResumed` — fixed as 109 LIVE b38fb334
  (`facts/theuth-resumed-subagents-show-idle-after-restart-root-cause-and-fix-2026-10-07`). The
  single-source `running` set is fragile exactly where herdr arbitrates several sources.
- A PTY-driven omp that is waiting on a *permission prompt inside its own TUI* is indistinguishable from
  one that finished: both are "quiet screen"; the `<job_idle>` body is the raw screen for the model to
  interpret.

**Lands in**

- A per-program "blocked / working / idle" classifier belongs in the PTY job task next to `Idle::saw`
  (`crates/tools/src/pty.rs#L1995-L2002`): a `Manifest { tail_patterns, title_patterns }` loaded per
  `start.command` basename from `~/.theuth/agents/pty/*.toml`, evaluated on the same 500 ms dirty tick,
  producing a new `Notice::JobState` beside `Notice::JobIdle` (`crates/tools/src/activity.rs#L191-L191`)
  and a `JobEvent::State` for the roster; `AgentState` gains `Blocked{why}` and `Agents::state_of` gets
  a fifth source (PTY state of the session's jobs). OSC 0/2/7/9;4 capture goes into `ingest()`
  (`crates/tools/src/pty.rs#L2244-L2255`) before `strip()` throws it away. The ACP path already has the
  hook-report seam (`ChildState`); extending it to Claude Code / Codex is A2.

## A2 Agent integrations & session identity

**Built**

- **Identity of an external harness = an `agents.yaml` key.** `~/.theuth/acp/agents.yaml` holds one
  `Entry` per harness: `command`, `args` that put it in ACP mode, `env`, `description`, display `model`,
  `skills`/`skills_dir`, `mcp` back-channel flag (`crates/acp/src/agents.rs#L20-L54`). Sky's file today
  lists `omp`, `opencode` (1.18.30) and `opencode2` (2.0.22) — no Claude Code, Codex, Gemini or Cursor;
  theuth only integrates harnesses that speak ACP over stdio.
- **Session identity = a theuth session row + the harness's own id.** `Store::set_acp` marks a session
  as external (`sessions.agent` = yaml key, `cwd`, `acp_skills`)
  (`crates/agent/src/store.rs#L1270-L1292`) and `set_acp_session` records the harness's session id once
  `session/new` returned (`crates/agent/src/store.rs#L1308-L1315`). `Connect.resume` carries that id
  into `session/load` when the agent advertises `loadSession`, else a new session
  (`crates/acp/src/harness.rs#L326-L356`, gate at `crates/acp/src/harness.rs#L746-L746`); a failed load
  falls back to `session/new` with a warning (`crates/acp/src/harness.rs#L824-L824`).
- **What a harness tells theuth about itself:** `agentInfo.name` from `initialize`
  (`crates/acp/src/harness.rs#L749-L753`), its `configOptions` (model, `thought_level`) which theuth
  drives via `set_config_option` after load (`crates/acp/src/harness.rs#L342-L352`), available commands,
  usage/meter updates (`crates/agent/src/acp_child.rs#L543-L568`), plans → todos, and — 141 — RFD
  `subagent_update` announcements of its own children (`crates/acp/src/harness.rs#L164-L189`). For omp
  theuth additionally **tails omp's own on-disk transcripts** under
  `~/.omp/agent/sessions/<cwd>/<ts>_<acpSessionId>/` because omp drops wire frames after the parent turn
  ends (`crates/acp/src/omp/tail.rs#L1-L7`, locate by `_<sid>.jsonl` suffix
  `crates/acp/src/omp/tail.rs#L34-L41`) — the closest thing theuth has to herdr's "agent reports its
  session ref".
- **theuth → theuth over ACP:** `SessionMeta` under `_meta.theuth` carries model, effort, risk ceiling,
  parent session/engine and `harness` ("run this `agents.yaml` entry of the node instead of a native
  session", the `<harness>@<engine>` form) (`crates/acp/src/meta.rs#L1-L40`).
- **Skills both ways (75):** theuth writes its skills into the child's cwd (`.agents/skills`,
  `.claude/skills` for Claude Code) when `skills` is on (`crates/acp/src/agents.rs#L38-L45`,
  `DEFAULT_SKILLS_DIR` `crates/acp/src/agents.rs#L79-L79`) and hands every harness a theuth MCP
  back-channel (`list_skills`, `load_skill`, `send_message`) in `mcpServers`, default on, retried once
  without it when the harness rejects it (`crates/acp/src/agents.rs#L46-L53`, `#L70-L75`).
- **Resume on main:** `respawn_acp_children` runs at boot only under `durability.resume = auto` and only
  for sessions that have a `child_result` wait owned here
  (`crates/agent/src/supervisor/acp_resume.rs#L20-L49`); `respawn_acp_child` gates on owned-here, local
  harness, readable `cwd` (`crates/agent/src/supervisor/acp_resume.rs#L51-L89`) and tells the actor
  `Respawn` → `session/load` + a restored note, or a composed delta hand-off when load fails (tests
  `crates/agent/src/acp_child.rs#L3227-L3317`).
- No hooks are installed into `~/.claude`, `~/.codex` or opencode config; no env var tells a child "you
  are session X of theuth" except through the MCP shim; no dedupe of the same harness session being
  adopted twice.

**Planned / in flight**

- **146 RESTART-CLARITY** (branch `fix/146-restart-clarity` → `land/146` → `final/146`; **not on
  origin/main dd19bf63** — only a 141 W4 commit mentions it): D1 respawns every mid-prompt ACP child at
  boot regardless of the parent's wait (`Store::midprompt_acp_children`), D2 ≤ 4 in parallel, D3
  `<child_lost>` to the parent, D9 an ACP prompt journal (`turn_steps input_accepted`), D10 graceful
  quit = kill-9 semantics for mid-prompt children with a "continue exactly where you left off" note, D11
  a RESUME brief (original brief + interrupted prompt + previous harness id, migration 0043
  `acp_previous`) when `session/load` fails
  (`fix/146-restart-clarity:docs/design/146-restart-clarity.md#L20-L35`). 41/41 smoke rows proven per
  `projects/theuth-design-146-restart-clarity-2026-10-08`.
- 141 R4 (`fix/141-r4`): observed-children follow-ups.
- The t3code borrow list's items 1 (capability profile per harness×version) and 4 (version probe) are
  **not built**; item 3 (delta hand-off) **is** (109 W4 / 146 D11); item 10 (SDK bridge for Claude
  Code/Cursor) not started (`facts/theuth-borrow-list-from-t3code-2026-10-03`).

**Pain (evidence)**

- Boot respawn followed only `child_result` waits, so three mid-prompt omp children of an idle
  orchestrator were never respawned after the 14:12Z restart on 2026-10-08
  (`facts/theuth-engine-restart-2026-10-08-acp-children-of-idle-parents-not-respawned`) — the 146
  trigger.
- omp driven in a PTY (not ACP) has no identity theuth knows: the orchestrator must harvest `Resume this
  session with omp --resume <id>` from the exit screen by hand and re-`start` it after a restart
  (`procedures/theuth-omp-pty-wave-pause-resume-across-harness-restart-2026-10-06`).
- `_meta.theuth` is ignored by opencode; usage semantics differ per harness version (borrow list item
  1).

**Lands in**

- A herdr-style *agent manifest* (how to detect, how to resume, hook install) would extend
  `acp::agents::Entry` (`crates/acp/src/agents.rs#L21-L54`) with `pty: { ready, blocked, resume_template
  }` for non-ACP harnesses and a `hooks:` block; the PTY `start` tool would look it up by command
  basename. The harness-reported session ref for PTY omp lands as a new `sessions.pty_resume` column set
  from an OSC/manifest capture, next to `acp_session` (`crates/agent/src/store.rs#L1308-L1315`). Claude
  Code / Codex integration = a `SessionMeta`-free `Entry` plus hooks written by a new `acp::hooks`
  module. Dedupe = a unique index on `(agent, acp_session)`.

## A3 Durability of running work

**Built** (what survives on main dd19bf63)

| thing | engine SIGKILL / crash | graceful quit / `t update` relaunch | host reboot |
|---|---|---|---|
| sessions, turns, tool calls, partial assistant text | yes — turn journal (`turn_steps`, `tool_calls`, `questions`, `waits`, `jobs`, `pty_screens`, migration 0033) + eager `ResumeTurn` at boot, safe replay (`facts/theuth-durability-109-as-built-semantics-2026-10-06`) | yes — drain ≤ `drain_secs` 10 s writes `suspended` (`crates/agent/src/durability/mod.rs#L31-L47`, `docs/design/109-durability.md#L916-L944`) | yes (same journal) |
| approvals / `ask_user` / handoffs / secret requests | re-published with original ids and remaining deadlines | same | same |
| `send_message{await_reply}` and child-result waits | re-armed; a spared finished result is delivered | same | same |
| reminders | yes (the only waiter that survived even pre-109) | yes | yes |
| schedules | files in `~/.theuth/schedules/` + run rows; a run in flight is marked lost | same | same |
| **shell jobs** | **no** — `<job_result exit="lost">` + log tail; leaked groups reaped at next start (`crates/agent/src/durability/jobs.rs#L46-L46`, `crates/tools/src/jobs.rs#L1616-L1616`) | **no** — `KillAll` after the drain window (`docs/design/109-durability.md#L936-L938`) | no |
| **PTY jobs** (incl. a PTY-driven omp) | **no** — process dies with the master fd; `screen()` serves the last `pty_screens` snapshot under a "process gone" banner (`docs/design/109-durability.md#L593-L614`) | **no** | no |
| **ACP children** (omp/opencode via `spawn_subagent`) | respawned via `session/load` + restored note or delta hand-off — but only for parents with an open `child_result` wait (`crates/agent/src/supervisor/acp_resume.rs#L20-L49`) | same | same (if the harness is local and `cwd` readable, `crates/agent/src/supervisor/acp_resume.rs#L82-L89`) |
| theuth subagents | their open turns are resumed like any session; parent wait re-armed | same | same |
| Monty python heap | dumped after every cell; survives | same | same |

- **One engine per data dir** (63): the holder of an exclusive advisory lock on `<data_dir>/engine.lock`
  is the engine and writes `engine.json` for clients to find (`crates/rpc/src/engine.rs#L1-L5`,
  `acquire` `crates/rpc/src/engine.rs#L182-L200`). A second `t` attaches over RPC rather than starting
  another engine; the desktop app's sidecar is the same binary.
- **Update flow (39):** CLI atomic-rename over the binary, "the running process keeps its mapped image";
  desktop `Install and restart` runs the RPC `shutdown` → `quit_ack` handshake then Tauri relaunches
  (`docs/design/39-update.md#L502-L511`). There is **no fd handoff and no socket the processes outlive
  the engine on**: a relaunch is a graceful quit, so shell and PTY jobs die (D14 ruling), ACP children
  are killed and respawned, waits are re-armed. Design 109 §14 names and *rejects for now* the
  herdr-equivalent — "a detached job keeper (`t jobd`, setsid, owning PTYs, jobs and ACP children so
  they outlive the engine)" — keeping it as a later stage behind the same `jobs`/`waits` rows
  (`docs/design/109-durability.md#L1193-L1203`).
- Verified by a 22-row crash-injection matrix (`THEUTH_CRASH_AT=<step>`, `just durability`)
  (`facts/theuth-engine-kill-survival-map-2026-10-06`).

**Planned / in flight**

- **146 RESTART-CLARITY** (not on main): respawn every mid-prompt ACP child whatever the parent's wait
  state, ≤ 4 in parallel, `<child_lost>` to the parent, one `resume:boot:<ts>` attention item
  ("Restarted 14:12 · 3 sessions resumed · 2 subagents back · 1 waiting · 3 lost"), flock refusal exits
  0 so launchd/systemd do not restart-loop, ACP prompt journal, RESUME brief on failed `session/load`
  (`fix/146-restart-clarity:docs/design/146-restart-clarity.md#L20-L35`).
- **139 MESH-DURABILITY** (on main): cross-engine waits and child results survive an engine restart on
  either side; `child_result` waits are the respawn trigger
  (`crates/agent/src/supervisor/acp_resume.rs#L20-L26`).
- No design covers PTY/job survival across an engine restart, or live handoff of PTY fds to a new engine
  process.

**Pain (evidence)**

- Every engine restart during a wave costs a manual five-minute recovery per omp worker: detect (job ids
  reset, `<job_result exit="lost">`), find the omp session id under `~/.omp/agent/sessions/`,
  `start(command="omp --resume <id>", cwd=…)`, wait for `○`, type a resume line
  (`procedures/theuth-recover-omp-acp-workers-after-engine-restart-by-pty-resume-2026-10-07`, done twice
  on 2026-10-07; `procedures/theuth-omp-pty-wave-pause-resume-across-harness-restart-2026-10-06`, 5
  workers 17:57 → 18:34).
- A sibling session's cleanup SIGTERMed another wave's omp workers; recovery again by `omp -r <id>` in
  PTYs (`facts/theuth-killed-sibling-omp-workers-by-time-window-2026-09-30`).
- `shell()` jobs die at a restart; the orchestrator's ssh PTY to build-B must be re-opened by Sky after
  every restart (`procedures/build-b-ssh-pty-re-login-after-engine-restart-2026-10-07`).
- 2026-10-08 14:12Z: two boots 3 s apart, children of idle parents not respawned, dangling `ToolCall`
  rendered as a running tool card (`facts/theuth-engine-restart-2026-10-08-1412z-forensics`).

**Lands in**

- herdr's *live handoff* (new server process inherits PTY masters) and *background server that outlives
  clients* have one natural seam: the rejected §14 keeper. Concretely a `t jobd` (new binary target in
  `crates/theuth`, setsid) owning `PtyHandle.master`/child and the shell-job process groups, talking to
  the engine over a Unix socket; `tools::pty::spawn` (`crates/tools/src/pty.rs#L1824-L1864`) and
  `jobs.rs` become clients; the `jobs`/`pty_screens` rows already carry the identity. Cheaper first step
  (S): on `t update`/relaunch, `SCM_RIGHTS`-pass the PTY master fds and child pids to the new engine
  over the `engine.lock` holder's socket instead of `KillAll` — same rows, no new daemon. Host reboot
  resume for PTY omp = the A2 `pty_resume` column + a boot pass next to `respawn_acp_children`.

## A4 Agent-native control API

**Built**

- **Model-facing runtime functions (inside a theuth session):** PTY — `start(command, name, cwd, env,
  ready, idle_secs)`, `send(id, text, enter, keys, signal)`, `screen(id)`, `wait(id, pattern, timeout)`
  (`crates/tools/src/pty.rs#L250-L316`), plus `jobs()`, `cancel_job()`, `list_terminals`. Peers —
  `spawn_subagent`, `send_message{to, await_reply, reply_to, request_id}`, `discover_peers`,
  `peer_message` (the caller's record of one exchange: accepted / consumed / replied / suppressed /
  failed) (`crates/agent/src/peers.rs#L4-L4`, `#L304-L304`, `#L407-L409`, `#L675-L675`).
  `send_message{await_reply:true}` blocks the caller until the addressee answers — that is the **only**
  "wait for another agent" primitive; it waits for a *reply*, not for a *state*. The roster's
  `AgentState::Awaiting{from}` is derived from it (`crates/agent/src/lib.rs#L1400-L1404`).
- **RPC surface:** `crates/rpc/src/api.rs` declares **408 `#[method]`s and 3 subscriptions**
  (`subscribe_pty` `#L313-L313`, `doc_subscribe` `#L985-L985`, `subscribe_events` `#L2271-L2271`) over
  jsonrpsee WebSocket. Agent-relevant methods: `new_session`/`send_input{session, text, files, delivery,
  id}` (idempotent by client `id`, `crates/rpc/src/api.rs#L184-L202`), `broadcast{selector}` +
  `list_activity` (62, `crates/rpc/src/api.rs#L239-L247`), `withdraw`/`list_queued`/`cancel{scope:
  root|tree}` (`#L248-L256`), `pty_screen`/`pty_input`/`pty_attach`/`pty_resize` (55), `resume_child`
  (`#L736-L736`), `attention_list` (`#L2137-L2137`), `shutdown` (`#L1384-L1384`).
  `subscribe_events{session, descendants}` streams every `AgentEvent` with a `PendingPrompts` snapshot
  first and `Lagged` resync (129) (`crates/rpc/src/api.rs#L2262-L2275`).
- **Versioning:** no wire version number; `just ts` runs `cargo test -p rpc --features ts
  export_bindings`, which writes `bindings/*.ts` for every wire type (ts-rs) and `bindings/methods.json`
  = the sorted list of every registered method (380 lines today), plus the surface catalog as JSON
  Schema (`crates/rpc/src/export.rs#L1-L5`). Compatibility is "rebuild the client from the bindings";
  `Info.version` carries `CARGO_PKG_VERSION` (`crates/rpc/src/api.rs#L122-L123`).
- **Third parties driving theuth:** (a) any WebSocket client can call the RPC once it finds
  `engine.json` and authenticates; (b) **A2A** (`crates/a2a`, 3 785 lines) exposes sessions as A2A tasks
  on a loopback listener and dials configured peers — **off by default** (`a2a.enabled: false` starts
  nothing, `crates/a2a/src/lib.rs#L1-L16`); (c) `t --acp` makes theuth itself an **ACP server** on stdio
  for any ACP client (Zed, another theuth), attaching to the running engine
  (`crates/theuth/src/main.rs#L1659-L1660`, `#L1676-L1683`); (d) `t mcp --session <id>` is the stdio MCP
  back-channel a child harness gets (`list_skills`, `load_skill`, `send_message`)
  (`crates/theuth/src/main.rs#L170-L177`); (e) the `t` CLI subcommands are `mesh`, `tasks`, `session`,
  `sandbox`, `mcp`, `double`, `dreaming` — each one RPC call (`crates/theuth/src/main.rs#L145-L189`).
  There is **no `t agents list`, `t wait`, `t send`, `t screen`** for a shell user or a foreign agent; a
  Claude Code running outside theuth cannot spawn a pane or wait on a theuth child except through the
  MCP shim theuth hands it.
- **Skill that teaches the API:** the system prompt's Runtime index (every function with its signature)
  plus `help(name)`; `harness`, `loops`, `codeindex` skills; ACP children get `list_skills`/`load_skill`
  over MCP (75 §9). No separate "herdr skill" document a foreign agent could read offline.

**Planned / in flight**

- 141 D14 made the MCP back-channel default-on for every harness; the t3code borrow list item 9
  (`delegate_task`/wait/steer/report_progress on that channel) is not built
  (`facts/theuth-borrow-list-from-t3code-2026-10-03`).
- 139 R2 cross-engine `inject` lets a `<child_lost>` reach a parent on another engine; no plan for a
  state-wait primitive.

**Pain (evidence)**

- "omp PTYs never wake you on completion — poll with reminders" (`facts/omp-pty-start-shape-theuth`,
  `facts/theuth-runtime-quirks-2026-09-24`): the orchestrator's wave loops are `remind_me_in` +
  `screen()` + regex by hand because there is no `wait(id, state="idle")`.
- `wait(id, pattern)` matched the echoed command line (fixed 206124b); `cancel_job` cannot free a PTY
  slot (`facts/theuth-runtime-quirks-2026-09-24`).
- A child's result is delivered as a `<message>` input and "a `<message>` written inside a body is just
  text" — safe, but there is no typed event stream an agent can subscribe to from Python.

**Lands in**

- `wait_state(ids, until: blocked|idle|done, timeout)` as a new tool in `crates/tools/src/pty.rs` beside
  `Wait` (`#L1553-L1597`), fed by the A1 classifier's `Notice::JobState`, and a sibling `wait_peer(name,
  until: idle|awaiting|approval|done)` in `crates/agent/src/peers.rs` that watches `Agents::state_of`
  transitions on the event bus — both ending the turn and waking with a `<job_state>`/`<peer_state>`
  input like `<job_idle>`. A shell CLI for foreign agents = `t agents ls|wait|send|screen` subcommands
  in `crates/theuth/src/main.rs#L149-L189` calling `list_activity`, the new wait RPC, `send_input`,
  `pty_screen`. A versioned schema = a `methods.json` hash exposed in `Info` next to `version`
  (`crates/rpc/src/api.rs#L122-L123`).

## A5 Supervision UX

**Built**

- **One attention model, many renderers.** `roster::attention::derive(inputs, states, now)` computes the
  inbox from stores every recheck (`crates/roster/src/attention.rs#L315-L337`); `AttentionKind` has 14
  kinds: `Approval, Decision, Hire, Blocked, Review, Budget, Node, Handoff, Secret, Ask, Show, Resume,
  Double, Finished` (`crates/roster/src/attention.rs#L39-L73`). Every item carries its session and desk,
  a stable id (dismiss/snooze key), `waiting_since_ms`, priority and a rank (132: project rank × 100 +
  user-set priority, `crates/roster/src/attention.rs#L338-L344`). `attention_list` RPC +
  `AttentionChanged` event; 119 made dismissal versioned.
- **Where a blocked child reaches Sky:** (1) desktop Attention page — master-detail screens per kind
  (`crates/desktop/ui/src/components/attention/{ApprovalScreen,AskScreen,DecisionScreen,ReviewScreen,ShowScreen}.tsx`),
  decision/review bodies are mini-boards (101); (2) HUD at `/` with an `AttentionBadge` and a live scene
  (128, `crates/desktop/ui/src/components/hud/`); (3) **macOS companion** (130, 48 Swift files / 7 553
  lines): edge rail of agent marks (`RailPanel.swift`, 233 lines), Attention panel with one-keystroke
  answers + 3 s undo (`AttentionPanel.swift`, 187 lines), breakpoint-deferred notifications
  (`crates/desktop/companion/Sources/Companion/Notifications/Notifier.swift#L24-L72` — an app switch or
  ≥ 20 s typing pause releases the banner), Ask bar, menu-bar extra
  (`docs/design/130-companion.md#L19-L36`); (4) **Windows companion** (134, WinUI, six waves landed);
  (5) desktop system notification only when the window is not focused/visible
  (`crates/desktop/ui/src/lib/notify.ts#L6-L12`); (6) Slack thread board — one row per descendant with
  state glyphs, `RequiresAction` → Approval (`crates/slack/src/board.rs#L1-L12`, `#L168-L168`); Telegram
  channel cards; `message_user` (24) for out-of-band files/progress; (7) TUI roster ("waiting on you",
  `crates/tui/src/model.rs#L12104-L12104`).
- **Status vocabulary shown:** the six `AgentState` words (A1). Blue/amber/green à la One is
  approximated by running / approval-or-awaiting / idle; there is no "has unread output" or "stuck for N
  min" mark, and no sound. Keyboard: companion J/K/E/1-2-3/Esc-undo (130); desktop keyboard navigation
  of the Attention rail.
- **Deliberately not shown:** a PTY job's quiet/busy state is a model input (`<job_idle>`), never an
  attention item; `Finished` is a receipt, not a needs-you; the double (126) can answer cards for the
  owner and the item then reads "double answered … undo".
- Push (42) to iPhone over the relay: design v0 only; `crates/relay/src` has no `push.rs` (files:
  cookie, frame, main, relay, web) — **not built**.

**Planned / in flight**

- **149 COMPANION-NATIVE** (worktree `~/Develop/theuth-149-int`, overnight unattended, three parallel
  parts from one contract): One-parity polish (agents panel grouped by host — "On your Mac and your VPS"
  — rows with status dot/spinner · harness icon · name · children · mic), point-and-say native,
  island/briefs native; per-agent mic = `send_input` to that session
  (`decisions/theuth-design-149-companion-native-rulings-2026-10-09`). The host-grouped agents panel is
  exactly herdr's "combined agent list" (A6) and needs an engine-side cross-engine `list_activity`.
- 130 W6 items from the agentbox borrow list (two-tier undo, advance-once, one banner per absence)
  handed over, not built; B12 "restart-not-reload" banner and B9 restore-place-on-reload still open
  (`facts/theuth-borrow-list-from-agentbox-attention-2026-10-07`).

**Pain (evidence)**

- The market complaint record theuth measured itself against: missed approval prompts, ~25 %
  notification fire rate, no mobile push, no overview inside a thread
  (`facts/getone-one-and-agent-supervision-product-landscape-2026-10-07`). theuth's own: a PTY-driven
  omp waiting on an in-TUI permission prompt never becomes an attention item (A1); the 2026-10-08
  restart showed dead children as a running tool card until 146's `resume:boot` item
  (`facts/theuth-engine-restart-2026-10-08-1412z-forensics`); the desktop went blank under 7 streams /
  12 sessions until 129 EVENT-FOCUS
  (`facts/theuth-desktop-blank-under-concurrency-root-cause-design-129-2026-10-07`).
- Sky's 149 words: "the Mac companion app is decent, but it's not the full One experience".

**Lands in**

- herdr's status marks for *any* terminal agent land as a new `AttentionKind::Blocked`-class item
  derived from the A1 `JobState` in `derive()` (`crates/roster/src/attention.rs#L315-L315`) with
  `Inputs` gaining the PTY state list from `list_terminals` (`crates/tools/src/pty.rs#L83-L105`); the
  companion rail reads it through the existing `attention_list`/`AttentionChanged` path with no Swift
  protocol change. OSC-9/BEL-triggered "agent rang the bell" = the same item with `detail` from the
  title. Sound/OS notification policy stays in `Notifier.swift`.

## A6 Multi-machine

**Built**

- **Three layers, all on main.** (1) **mesh** (67): every engine has an iroh endpoint; paired peers keep
  one connection each "kept up with backoff" (1 s → 60 s, `crates/mesh/src/net.rs#L1-L8`, `#L46-L50`), a
  stability-elected hub lease, gossip presence, and the mesh is the `replica::Transport` every other
  plugin uses (`crates/mesh/src/lib.rs#L1-L9`). (2) **nodes** (32): pair to harnesses on other machines,
  one control conn each, "so the model can run children there" — a device of every node, dialer for ACP
  children (`crates/nodes/src/lib.rs#L1-L17`). (3) **replica + sessionsync** (68): owner-scoped session
  replication over the mesh, so a second desktop sees the same sessions; **link/relay** (31) for phones
  and the web client through `relay.theuth.io`.
- **Remote machines over SSH:** `t mesh add --ssh [user@]host [--key NAME | --system-ssh]` installs `t`
  over SSH and pairs it; `--docker` and `--vm` (79 sandbox VMs, 9 041 lines) do the same for a container
  or an isolated Linux desktop (`crates/theuth/src/main.rs#L228-L267`). Credentials: a vault-stored
  private key for the built-in SSH client, or the system `ssh` (agent, ProxyJump) — SSH is bring-up
  only; steady-state traffic is iroh (QUIC, static-key identities), not an SSH tunnel.
- **Running an agent elsewhere:** `spawn_subagent{agent:"<harness>@<node>"}` — `parse_agent` splits
  `<harness>@<engine>` (`crates/agent/src/peers.rs#L124-L132`) and the node runs its own `agents.yaml`
  entry via `_meta.theuth.harness` (`crates/acp/src/meta.rs#L36-L39`). `discover_peers` lists other
  owner roots and sessions across the tree; `AgentInfo.agent` carries the `<harness>@<node>` string but
  there is **no `engine`/`host` field** on `AgentInfo` (`crates/agent/src/lib.rs#L1211-L1256`) — a
  combined cross-machine agent list exists only implicitly (replicated session rows, sessions owned
  `here` or not).
- **Reconnect / restart policy (139 MESH-DURABILITY, on main):** owner-scoped durability
  (`open_turns(Scope::Here)`), transfer mid-turn, resources across a transfer, child results and waits
  surviving an engine restart on either side; a replica renders the owner's live turn as "running on A"
  instead of "interrupted" (`docs/design/139-mesh-durability.md#L39-L58`). A child whose owner engine is
  gone is not respawned elsewhere (`crates/agent/src/supervisor/acp_resume.rs#L51-L54`).
- Security model: mutual pairing strings (transitive), static-key identities, `Node` attention item when
  a paired engine is unreachable (`crates/roster/src/attention.rs#L52-L53`); sandbox VMs see only
  explicitly shared directories.

**Planned / in flight**

- 149's agents panel "grouped by host — On your Mac and your VPS" needs a cross-engine `list_activity`
  with a host column (`decisions/theuth-design-149-companion-native-rulings-2026-10-09`).
- 139 Revision 2 HARNESS-ENVELOPE (`docs/design/139-mesh-durability.md#L1049-L1049`) and the smoke
  matrix on real engines (`docs/design/139-smoke.md`).

**Pain (evidence)**

- Relay unreachable behind Zscaler (403 on `wss://relay.theuth.io`) — not a bug, but there is no
  in-product explanation (`facts/theuth-relay-unreachable-behind-zscaler-diagnosis-2026-09-26`).
- Driving build-B (the Windows build host) is done through a hand-opened ssh PTY that dies on every
  engine restart, then through an ad-hoc HTTP build service — not through a theuth node
  (`procedures/build-b-ssh-pty-re-login-after-engine-restart-2026-10-07`,
  `facts/build-b-build-service-and-scratch-engine-design-134-2026-10-07`).

**Lands in**

- theuth is **ahead** of herdr here (iroh mesh, replication, VMs) except for the *combined agent list*:
  add `engine: Option<String>` + `here: bool` to `AgentInfo` (`crates/agent/src/lib.rs#L1211-L1256`)
  filled from `SessionSummary.owner`, make `list_activity` merge replicated rows, and the companion/HUD
  group by it. An SSH-only "remote terminal" mode (herdr style, no engine on the far side) would be a
  `Transport::Ssh` for PTY jobs in `crates/tools/src/pty.rs` — probably not wanted: a node is the theuth
  answer.

## A7 Terminal engine & multi-client rendering

**Built**

- **One emulator, in the engine, per PTY job.** `crates/tools/src/pty.rs` (4 298 lines) is the only user
  of `vt100` 0.16; `portable-pty` 0.9 is used there for jobs and in
  `crates/desktop/src/launch.rs#L22-L22` to launch the sidecar engine (workspace deps
  `Cargo.toml#L255-L257`; `crates/tools/Cargo.toml#L39-L40`). Grid **120×40** at `start`, resizable by a
  viewer to ≤ 400×200 (`crates/tools/src/pty.rs#L37-L43`); **512 rows of scrollback** retained per PTY,
  history pages ≤ 80 rows / 256 KiB (`crates/tools/src/pty.rs#L68-L73`); ≤ 4 snapshot renders in flight
  across all PTYs (`#L76-L77`); a 64 KiB reader thread (`#L66-L67`); `send` settles on 300 ms quiet / 2
  s max (`#L47-L50`). The job task renders rows on a 500 ms dirty tick, which is also the idle
  detector's clock (A1).
- **Server-side headless state is the design**, not vendored patches: the engine answers
  DA/DSR/OSC-colour queries itself so TUIs do not block (`crates/tools/src/pty.rs#L2257-L2269`), scrubs
  vault values from every screen, and journals `pty_screens` rows at every `screen()`/`send()`/`wait()`
  return (109 §5.3, `crates/tools/src/pty.rs#L1107-L1112`). No ghostty-vt, no graphics protocol, no
  mouse reporting (55 §7 scope), no vendored portable-pty changes (plain crates.io 0.9).
- **Two render paths to clients:** (1) **snapshot** — `pty_screen` returns a `PtyPage` of styled
  cells/runs (`PtyPage`, `StyledRun`, `Style`, `Color` `crates/tools/src/pty.rs#L146-L224`), polled at
  500 ms, 60 ms while a viewer has focus (55 §0.3); the TUI paints this. (2) **raw stream** —
  `pty_attach`/`subscribe_pty` (70 PTY-POPOUT): `PtyHandle::attach` subscribes to the byte broadcast and
  snapshots under one lock (`crates/tools/src/pty.rs#L621-L637`); `replay()` rebuilds the retained
  scrollback and live screen as escape sequences from the vt100 state
  (`crates/tools/src/pty.rs#L681-L704`); the wire is `PtyChunk::{Reset{data,rows,cols}, Data{data},
  Exit}` with a `Reset` resync when a client lags 256 chunks (`crates/tools/src/pty.rs#L1084-L1097`,
  `STREAM_CHUNKS` `#L44-L46`). The desktop paints that with **xterm.js 6** + fit addon
  (`crates/desktop/ui/src/components/chat/TerminalViewer.tsx#L1-L3`, `package.json` `@xterm/xterm
  ^6.0.0`).
- **Several viewers on one PTY:** any number may subscribe (broadcast receiver per attach); input from a
  human is `pty_input` with structured `Key`s encoded by the engine against the program's modes (55
  §0.1–0.2, `crates/tools/src/pty.rs#L318-L353`); a human typing marks the job `user attached` for 30 s
  so the model's tool replies say so (`#L51-L52`). Sizing is the open seam: "two viewers on one PTY
  would fight over it, so exactly one sizes it" — the popped window owns the grid and restores 120×40 on
  close (`crates/desktop/ui/src/components/chat/TerminalViewer.tsx#L27-L34`); the model's `screen()`
  reads whatever grid the viewer left.
- **ACP ⇄ PTY handoff (56):** exploratory v0 only, "nothing here is committed"
  (`docs/design/56-acp-tui-handoff.md#L3-L9`); no code. An ACP child has no terminal; a PTY omp has no
  ACP.

**Planned / in flight**

- None on the emulator. 149 moves surfaces native but keeps the engine's PTY model.

**Pain (evidence)**

- PTYs inherit no `COLORTERM`/`TERM_PROGRAM`, so omp draws 256-colour; the fix is env by hand
  (`facts/theuth-pty-start-env-for-omp-colour`).
- "`screen` returns rows" / `wait(pattern)` matched the echoed command
  (`facts/omp-pty-start-shape-theuth`); `cancel_job` cannot free a PTY slot
  (`facts/theuth-runtime-quirks-2026-09-24`).
- PTY budget: each handle reserves `(40+512)×120×96` B ≈ 6.3 MB against `jobs::PTY_BUDGET`
  (`crates/tools/src/pty.rs#L78-L81`) — a wave of 6–8 omps is near the cap.

**Lands in**

- herdr's per-client render deltas are already half here (`PtyChunk` stream + `replay`); what is missing
  is per-viewer size independence — a per-subscriber vt100 re-render (`PtySnapshot::snapshot(viewport)`
  `crates/tools/src/pty.rs#L564-L571` already takes a viewport) instead of resizing the one PTY.
  Alt-screen-aware reads for the A1 classifier come from `vt100::Screen::alternate_screen()` (used in
  `replay`, `#L687-L687`). A ghostty-vt swap would replace `vt100::Parser` at `#L1401-L1401` and the
  cell mapping `style_of`/`render` (`#L777-L948`) — M, no user-visible gain unless graphics or
  wide-grapheme fidelity is wanted.

## A8 Workspaces, worktrees & plugins

**Built**

- **Git worktrees: one engine-managed implementation, scoped to autoresearch (140), none for
  subagents.** `crates/autoresearch/src/git.rs` (1 302 lines) has `worktree_add` = `git worktree add -q
  -b autoresearch/<slug> <path> <base_sha>` with a dirty-checkout snapshot commit
  (`crates/autoresearch/src/git.rs#L203-L243`), `worktree_remove` = `remove --force --force` + `prune`
  (`#L245-L259`), branch naming helpers (`#L140-L155`), `resolve_base`, keep/discard/apply_patch. It is
  behind the `Git` trait of the autoresearch actor (`crates/autoresearch/src/actor.rs#L159-L171`) and
  used for experiment slots only. `spawn_subagent` has **no `workspace:` argument**; `grep worktree
  crates/` finds nothing outside autoresearch and a Slack test fixture. Every design wave (109, 122,
  139, 140, 144, 146, 147, 149 …) cuts `~/Develop/theuth-<n>-*` worktrees **by hand** from the
  orchestrator's shell — 26 worktrees of this repo exist right now (`git worktree list`), including
  stale ones (`theuth-109-durability` noted still present in
  `projects/theuth-design-109-durability-2026-10-06`).
- **Code index (60):** `crates/codeindex` (tree-sitter; `index.rs`, `rank.rs`, `map/`, `lens.rs`,
  `profile.rs`, `skill.rs`) exposes `code_find/outline/refs/symbol/map/impact` and `project_profile()`
  to the model; CodeMap perspectives landed as 147.
- **Extension model (versus herdr plugins/marketplace):** *skills* — markdown with front matter in
  `~/.theuth/skills/` (11 today) and cloned skill repos (`add_skill_repo`/`sync_skills`),
  load-on-demand, also written into ACP children's cwd and served over MCP (A2); *MCP servers* — JSON
  files in `~/.theuth/mcp/` (10 today: argocd ×5, atlassian, github, grafana ×2), hot-reloaded within a
  second, risk-gated per server/tool in `mcp/config.yaml`, plus `add_mcp_repo` for cloned servers;
  *apps* (46) — OpenUI Lang interactive apps; *routines* — schedule files with cron/every/at,
  `until`/`max_runs` loops, python sensors (`when:`) run in Monty with hard budgets
  (`decisions/theuth-watchers-design-direction-2026-09-28`); *agents* (67) — `agents/<name>.md` roster
  files hired from chat; *knowledge* — the durable store. Plugins in the Rust sense are compile-time
  crates (`crates/*` registered in `main.rs`' plugin list, `crates/theuth/src/main.rs#L1884-L1884`).
  There is no runtime-loaded binary plugin, no marketplace, no layout/custom-command system — a "layout"
  in theuth is a session tree + attention rail, not panes.

**Planned / in flight**

- t3code borrow list item 5, **engine-managed worktrees** (`spawn_subagent{workspace:
  worktree|existing|cwd}` → `../<repo>-wt-<name>` + branch, `{repo, path, branch, base_sha}` on the
  session, `workspace_list/remove` RPC, cleanup when merged), rated M on 2026-10-03 — **not built**;
  item 6 (per-turn hidden-ref checkpoints) not built
  (`facts/theuth-borrow-list-from-t3code-2026-10-03`). 140 now owns the only git writer and could be
  lifted.
- Design 140's `autoresearch/<slug>` branch convention and `include_dirty` base snapshot are the obvious
  template for a general `agent::workspace` module.

**Pain (evidence)**

- Every wave procedure repeats the same shell: `git worktree add`, APFS-clone a warm `target/`, remove
  the worktree, branch and target when merged
  (`procedures/theuth-omp-wave-seam-ownership-and-integration-2026-10-02`,
  `procedures/theuth-small-bug-fix-as-one-omp-acp-worker-on-cloned-target-2026-10-07`); a blind `mv` in
  a shared briefs folder swept a peer's directory
  (`procedures/theuth-claim-design-number-and-coordinate-seams-with-active-roots-2026-10-08`); stale
  worktrees and shared `target/` dirs ate the Data volume to ~300 MiB free
  (`procedures/theuth-prune-live-shared-cargo-target-in-place-2026-10-08`); a worktree seeded from Sky's
  stale generated bindings shows tsc drift until `just ts-desktop`
  (`facts/theuth-macbook-rebooted-mid-wave-omp-acp-children-resume-by-send-message-2026-10-07`).
- Design-number collisions between orchestrators (138 → 139) because the claim is a directory listing,
  not a lock.

**Lands in**

- Lift `autoresearch::git::{worktree_add, worktree_remove, resolve_base}` into a shared
  `crates/agent/src/workspace.rs` (or a tiny `crates/gitops`), add `workspace: Option<Workspace>` to
  `SpawnSubagent` args and `sessions.workspace` JSON (`{repo, path, branch, base_sha, target}`),
  `workspace_list`/`workspace_remove` RPC in `crates/rpc/src/api.rs`, and a Library view; cleanup policy
  fires on `TurnFinished` of a child whose branch is merged. herdr-style plugins have no theuth seam and
  should not get one: skills + MCP + routines + agents already cover the use cases; a "catalog of agent
  manifests" (A1/A2) is the one new data file worth syncing remotely, next to
  `~/.theuth/acp/agents.yaml`.

## A9 Engineering practice

**Built**

- **Tests:** 3 423 `#[test]`/`#[tokio::test]` functions across 571 Rust files (unit tests live inside
  the crates; 50 integration-test files under `crates/*/tests/`, e.g.
  `sessionsync/tests/mesh_durability.rs`); 216 vitest files under `crates/desktop/ui/src`; Swift tests
  under `crates/desktop/companion/Tests`. Named suites: `just durability` = the 109 crash-injection
  matrix by test-path filter, "~1 minute on an M-series laptop", `--test-threads=1` on the `tools` half
  for the sqlx teardown (`justfile#L29-L36`); `just durability-mesh [smoke]` = 139's two-engine rows
  in-process plus a scripted 7-minute smoke against two real engines on fixed ports, evidence in
  `docs/design/139-smoke.md` (`justfile#L46-L57`). Restart smoke for 146:
  `scripts/restart-clarity-smoke.sh` → `smoke.ts`, 41/41
  (`fix/146-restart-clarity:docs/design/146-restart-clarity.md#L88-L88`). No live-handoff or
  detach/restore test exists because the features do not.
- **Gates:** `just check` (`cargo check --workspace --all-targets`), `just test`, `just clippy -D
  warnings`, `just fmt`, `just ts`/`ts-desktop` (bindings regen + copy), `desktop-check`, 37 recipes in
  all (`justfile#L20-L28`, `#L58-L60`). Workers run crate-level gates only; CI-equivalent is the
  closer's one verification pass
  (`decisions/sky-closer-phase-waves-elect-leader-one-verification-pass-2026-10-08`).
- **Shared-machine build protocol (knowledge, not source):** `~/.theuth/bin/build-lock` v3.1 — heavy
  FIFO queue with 3 base slots (4th/5th on CPU/memory headroom), 2 light slots for bun/tsc/vitest, `mbx`
  (mr-boxington 1.22.0) gates bypass the heavy queue and meter rustc themselves; `CARGO_BUILD_JOBS=4`,
  reflinked managed targets, gc keys (`procedures/theuth-build-lock-v3-worker-rules-2026-10-08`,
  `facts/mbx-mr-boxington-setup-on-skys-mac-2026-10-08`). Sky ruled the source carries only the generic
  "Share the machine" paragraph; machine rules stay in knowledge
  (`decisions/theuth-shared-hardware-prompt-generic-in-source-machine-rules-in-knowledge-2026-10-08`).
- **Docs:** 507 files in `docs/design/` — numbered designs (01–149) each with `NN-as-built-<wave>.md`
  siblings, recon and smoke files; `docs/00-architecture.md` wins conflicts;
  `docs/release-notes-2.0.0.md` and `docs/release-notes-2.12.0.md` (draft) are the only changelogs —
  **no `CHANGELOG.md`**, no versioned docs site per release (the `site/` is marketing). **No `AGENTS.md`
  or `CLAUDE.md` at the repo root**; conventions reach workers through briefs
  (`~/Develop/theuth-<n>-briefs/`) and knowledge, and reach the model through `~/.theuth/skills/` (11)
  and `prompt.md`.
- **API schema generation:** ts-rs bindings + `methods.json` + surface JSON Schema from one `cargo test`
  (`crates/rpc/src/export.rs#L1-L5`); the desktop copies them in (`just ts-desktop`); drift shows as tsc
  errors, not a wire version.
- **Release / update channel:** `just bump-version`, `tag-release`, `release`, `release-s3`,
  `release-update`, `package-macos/linux/windows/ios`, `notarize-macos`, `testflight` (`justfile`
  recipes list); artefacts on rustfs `https://s3.rwx.dev` bucket `theuth`
  (`docs/update-setup.md#L12-L12`); the self-updater polls
  `https://s3.rwx.dev/theuth/releases/latest.json` (`crates/update/src/lib.rs#L29-L29`) — one channel,
  no beta/stable split. `Cargo.toml` says `version = "2.13.0"` while local tags stop at v2.9.3 — release
  tags are not all fetched into Sky's checkout.
- **Remote-updated data catalogs:** none (agents.yaml is hand-edited; models catalog ships with releases
  via `just release-models`).

**Planned / in flight**

- 146's as-built records "mbx-managed target (plain target/w removed), first-build hit/miss and stats" —
  the gates are moving onto mbx wave by wave (commit 6a1b3fd7 on `fix/146-restart-clarity`).
- Nothing planned for a root `AGENTS.md`, a `CHANGELOG.md`, or release-channel separation.

**Pain (evidence)**

- "agents waiting 30 minutes for a lint check slot, and the machine is at like 50 % usage" (Sky,
  2026-10-08 18:45) → build-lock v3 collision (four sessions editing the script at once) → mbx adoption
  the same evening; a 400 GB debug-build cleanup ordered
  (`facts/mbx-mr-boxington-setup-on-skys-mac-2026-10-08`).
- Test suites leak thousands of scratch dirs into `$TMPDIR` and time out on a full disk
  (`facts/theuth-tests-leak-tmpdir-scratch-dirs-2026-10-08`).
- Sky's checkout carries stale generated bindings → tsc drift in every worktree seeded from it
  (`facts/theuth-macbook-rebooted-mid-wave-omp-acp-children-resume-by-send-message-2026-10-07`).
- The design-doc corpus is the changelog: release notes are written by hand from it (2.12.0 is still
  "draft").

**Lands in**

- A herdr-style practice borrow is cheap and non-code: a root `AGENTS.md` generated from
  `docs/00-architecture.md` + the build-lock rules (the knowledge entry already is the brief block), a
  `CHANGELOG.md` assembled from `docs/design/*-as-built-*.md` headers at `just tag-release`
  (`justfile#L209-L218`), and a `methods.json` hash in `Info` (A4). Tests for the A3 keeper would join
  `just durability` (`justfile#L34-L36`) as a `jobs::keeper::` filter.

## Live designs on these seams

| design / branch | state (2026-10-09 01:30) | axis | what it already plans |
|---|---|---|---|
| 109 DURABILITY (`docs/design/109-durability.md`, as-built C/I/W1–W6/BANNER/REPORT/LIVE) | on main (ebac3603 → b38fb334) | A3, A1 | turn journal + eager resume, drain, jobs lost-not-adopted (D14), ACP respawn via `session/load`, PTY `pty_screens` snapshots; §14 rejects-for-now a `t jobd` keeper |
| 139 MESH-DURABILITY (+ R2 HARNESS-ENVELOPE, smoke) | on main (cd4c8b5c) | A3, A6 | owner-scoped resume, mid-turn transfer, cross-engine child waits/results, replica banner "running on A" |
| 141 ACP SUBAGENT VISIBILITY (`141-as-built.md`, recon R1/R1B/R2A/R2B) · `fix/141-r4` | on main (cac2e5a2); R4 in flight (`~/Develop/theuth-141r4`) | A1, A2 | `ChildState`/`ObservedState` from RFD `subagent_update`, omp transcript tailing, MCP back-channel default-on (D14) |
| 146 RESTART-CLARITY (`fix/146-restart-clarity` → `land/146` → `final/146`) | **not on origin/main**; 41/41 proven; landing via the batch pass | A3, A2, A5 | respawn every mid-prompt ACP child, ≤ 4 parallel, `<child_lost>`, `resume:boot` attention item, flock exit 0, ACP prompt journal, RESUME brief + `acp_previous` (0043), `↻ Resume` pill = boot respawn |
| 149 COMPANION-NATIVE (`integrate/149`, `~/Develop/theuth-149-int`, briefs `~/Develop/theuth-149-briefs`) | design + waves overnight, unattended | A5, A6 | One-parity rail/inbox, agents panel grouped by host, per-agent mic → `send_input`, point-and-say and island/briefs native |
| 130 COMPANION (W0–W6, PARITY) · 134 COMPANION-WINDOWS (W1–W6) | on main | A5 | Swift rail/panel/notifier/ask bar; WinUI twin; W6 agentbox items pending |
| 132 ATTENTION-ROWS-AND-ORDER · 104 · 119 · 101 | on main | A5 | row wording, one rank for inbox + dispatch, versioned dismiss, mini-board packets |
| 61 PTY-IDLE · 22 PTY · 55 INTERACTIVE-TERMINALS · 70 PTY-POPOUT/COLOR | on main | A1, A7 | quiet-screen `<job_idle>`, `pty_input` structured keys, raw `subscribe_pty` + xterm.js viewer |
| 56 ACP ⇄ PTY HANDOFF | exploratory v0, no code | A7, A2 | an external harness's own TUI on a theuth PTY, model picks ACP or PTY |
| 140 AUTORESEARCH (`feat/140-*`, `fix/140-tab-agents`) | on main dd19bf63 | A8 | the only in-engine `git worktree add/remove` (study slots) |
| 63 ONE-ENGINE · 39 UPDATE | on main | A3 | `engine.lock` flock + `engine.json`; quit-handshake relaunch, no fd handoff |
| 60 CODEINDEX · 147 CODEMAP | on main | A8 | tree-sitter index, `project_profile`, generated architecture maps |
| 42 PUSH | design v0 only | A5 | APNs through the relay — not built |
| 67 AGENTS · 32 NODES · 79 SANDBOX-VM · 31 LINK | on main | A6 | mesh/hub lease, nodes, VMs, phones via relay |

## Numbers

- Repo: 526 854 lines of Rust under `crates/` (56 crates), `crates/rpc/src/api.rs` 8 844 lines with 408
  methods + 3 subscriptions, `bindings/methods.json` 380 lines; `crates/tools/src/pty.rs` 4 298;
  `crates/agent/src/acp_child.rs` 5 568; `crates/agent/src/supervisor.rs` 14 672;
  `crates/roster/src/attention.rs` 3 080; `crates/acp/src/harness.rs` 1 787;
  `crates/autoresearch/src/git.rs` 1 302; companion 48 Swift files / 7 553 lines; mesh 4 679 · nodes 1
  661 · link 5 396 · relay 1 349 · replica 3 223 · sessionsync 4 704 · sandbox 9 041 lines.
- Tests: 3 423 Rust test fns in 571 files, 50 `crates/*/tests` files, 216 vitest files; `just
  durability` ≈ 1 min, `just durability-mesh smoke` ≈ 7 min, 146 smoke 41 rows, 109 crash matrix 22
  rows.
- Docs: 507 files in `docs/design/` (182 designs/recons/smokes + 325 as-builts); 2 release-notes files;
  0 `CHANGELOG.md`, 0 `AGENTS.md`/`CLAUDE.md`.
- PTY: 120×40 default, ≤ 400×200, 512 scrollback rows, ≈ 6.3 MB reserved per PTY against a 64 MiB
  engine-wide budget (≈ 10 live PTYs), idle default 60 s (min 10, backoff to 3 600 s), `send` settle 300
  ms / 2 s, `wait` 30 s / 120 s, `ready` ≤ 300 s, stream lag resync at 256 chunks.
- Durability: `drain_secs` 10, `resume_parallel` 4, 146 `acp_respawn_parallel` 4; mesh reconnect backoff
  1 → 60 s, dial timeout 30 s.
- Harnesses in `~/.theuth/acp/agents.yaml`: 3 (omp, opencode, opencode2); skills 11; MCP server files
  10; schedules 2; worktrees of this repo on disk: 26.
- Recorded manual recoveries of omp workers after engine restarts/reboots: 2026-09-30 (sibling SIGTERM),
  2026-10-06 ×2 (pause/resume, 5 + 1 workers), 2026-10-07 ×2 (10:19 restart, ~11:00 reboot), 2026-10-08
  14:12Z (146 trigger) — six incidents in nine days.

## Open questions

1. Does Sky want PTY/job survival across an engine restart at all? 109 D14 said jobs are not durable and
   rejected adoption; a herdr-style keeper (`t jobd`) is "a later stage" in §14. The recovery knowledge
   shows the cost of that ruling is paid roughly every other day — is it time to revisit, or is "respawn
   by `omp --resume`" (an A2 manifest with a resume template) the cheaper answer?
2. Which population matters for state detection: PTY-driven omps (where the manifest/OSC technique
   lands) or ACP children (where theuth already has hook-style reports)? The waves run omp over ACP by
   default since 109; PTY omps are the fallback after restarts.
3. Should `AgentState` grow `Blocked{why}` (one more word every renderer must learn — companion, Slack,
   Telegram, TUI, HUD) or should PTY states stay a separate `JobState` surfaced only as attention items?
4. Is a shell CLI for foreign agents (`t agents ls|wait|send`) wanted when the design direction is
   "everything inside theuth or a node"? The MCP shim already gives a child `send_message`; a `wait`
   there is the minimal version.
5. 146 is proven but unlanded: does the batch pass land it tonight, and does it move the 141 seam
   (`acp_child.rs:1151` one-line touch) in a way R4 must fold?
6. Engine-managed worktrees: lift 140's `git.rs` now (S–M) or wait for a design? Every running wave
   would benefit; the risk is the APFS-cloned `target/` and mbx managed-target policy, which `git.rs`
   does not know about.
7. Is a `methods.json` hash / wire version worth exposing before third parties exist?
