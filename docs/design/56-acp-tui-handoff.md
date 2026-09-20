# 56 — External harnesses in their own TUI: ACP ⇄ PTY handoff

Status: **exploratory** v0 · 2026-09-20 · builds on `55-interactive-terminals.md`
(the interactive PTY viewer and `pty_input` are prerequisites, not repeated
here); extends `06-acp.md` §3 (a second transport for an external child),
`19-subagent-board.md` §2 (one picker key), `25-desktop.md` §7. Nothing here
is committed; §7 spikes decide whether §3 survives as written. Every
file:line below was read on 2026-09-20 (scout report
`agent://AcpSupervisorScout`; upstream sources cited inline).

Asked for: (1) ACP subagents launched in their harness's interactive TUI on a
PTY, with the model choosing to drive them over ACP or over the PTY; (2) two
ways for the human to observe an external child.

Verdict: **possible, with one correction to (1).** A harness process cannot
speak ACP and draw its TUI at the same time (§1.1). What *is* true, and
verified in the adapters' source, is that the session identity is shared: the
id ACP hands back is the id the harness's own CLI resumes (§1.2). So an
external child has **one transport at a time — ACP or PTY — over one harness
session**, and switching is a handoff, not a second connection.

Open questions this doc does not close (the reason it is exploratory):
whether the handoff is worth its `Live` arm versus "the model just runs
`start {command: "claude --resume …"}`" (§8 last bullet); which harnesses
beyond Claude Code and Codex have a resumable id (§1.2); whether a
model-callable switch is wanted (§2.3); how TUI-era turns get into wth
history (§3.4).

## 0. Decisions in one screen (provisional)

1. **One transport per external child at a time: `acp` or `pty`.** The wth
   session row, its `agent`, `cwd` and `acp_session` columns, the roster
   entry and approval routing are the same in both. `acp` is today's
   `AcpChild` (structured events, `send_message`, results to the parent).
   `pty` is the harness's own TUI on a `Jobs` PTY job (a screen, keystrokes,
   the human can type — 55). Rejected: a "dual" child driving one process
   both ways (no harness offers it, §1.1); a structured layer faked over a
   TUI (§8).
2. **Handoff is kill-then-spawn on the shared session id.** ACP→PTY: refuse
   while a turn runs, close the ACP process, `start` the entry's `tui.resume`
   command with `{session}` = `acp_session`, `cwd` = the row's cwd, as a PTY
   job owned by the *parent* session and named after the child. PTY→ACP is
   automatic: when the TUI exits the `Live::Pty` marker is dropped and the
   existing "idle (process gone) → respawn with `session/load`" path (06
   §3.2, `supervisor.rs:645-656`) takes over on the next `send_message`.
3. **Every harness process for one wth child is exclusive.** Two processes
   on one Claude JSONL / Codex thread corrupt it. The `Live` map is the lock:
   a `Pty` entry blocks `enqueue_peer`'s respawn (`supervisor.rs:1026-1099`),
   an `Acp` entry blocks `set_transport(pty)` unless idle. Test: no state
   with an `AcpChild` and a `tui` job alive for the same child.
4. **The model drives a `pty` child with the four PTY tools it has**
   (`send`/`screen`/`wait`, id = child name; `cancel_job` to end it).
   `send_message` to a `pty` child is refused with one sentence naming the
   job. No turn boundary exists on a TUI, so no `<message from=session:…>`
   result is synthesised; the parent gets the ordinary `<job_result
   pty="true">` (final screen) when the TUI exits.
5. **Two observations of an external child:** the transcript/board from
   translated `AgentEvent`s while the transport is `acp`; the harness's
   native TUI in `/terminals` / the desktop terminal dialog while it is
   `pty`. The picker shows which (`claude · acp` / `claude · pty`).
6. **`pty`-first spawn works only when the entry can pin the id.**
   `spawn_subagent{transport:"pty"}` renders `tui.new` with `{session}` = a
   fresh UUID and `{task}` = the task; if the template uses `{session}`
   (Claude Code `--session-id`) the id is stored as `acp_session` and ACP can
   pick the conversation up later; if not (Codex has no id-pinning flag) the
   child is `pty`-only until its TUI exits, and the next `send_message`
   starts a fresh ACP session with the warning the code already prints
   (`acp_child.rs:833-857`).

## 1. Facts this design rests on

### 1.1 ACP and a TUI are mutually exclusive on one process

ACP is JSON-RPC over the agent process's stdin/stdout (v1, and the v2 draft:
`docs/protocol/v2/overview.mdx`). The SDK we pin spawns the harness with all
three stdio piped, in its own process group, no PTY
(`agent-client-protocol-2.1.0/src/acp_agent.rs:250-307`); wth never sees the
pid (`harness.rs:156-161`). The adapters are not TUIs: `claude-agent-acp`
wraps the Claude Agent SDK, `codex-acp` wraps `codex app-server`, `opencode
acp` starts a *private* per-process server (opencode.ai/v2/docs/cli/acp). No
harness exposes "my TUI is running, also talk ACP to it". v2 adds
*display-only* terminals inside `session/update` — the agent showing the
client a terminal it runs; the opposite direction (§8).

### 1.2 The ACP session id is the harness's own session id

- `claude-agent-acp` `src/acp-agent.ts:7819-7830`: `sessionId = randomUUID()`
  for a new session, `= creationOpts.resume` for a load; `:8200`
  `options.sessionId = … : sessionId` hands that same id to the SDK. Claude
  Code's CLI resumes it: `claude --resume <id>` (same `CLAUDE_CONFIG_DIR`,
  same cwd — sessions are filed per project directory). `claude --session-id
  <uuid>` pins the id of a new TUI session. `session/load` on the adapter
  passes `resume: sessionId` to the SDK (`:7746-7747`), so a session the CLI
  created loads over ACP too. Caveat (`:2841`): the JSONL is written on the
  first turn — a `session/new` that was never prompted cannot be resumed.
- `codex-acp` `src/CodexAcpServer.ts:1584, 2512, 2698, 2704`:
  `threadId: sessionState.sessionId` — the ACP session id *is* the Codex
  thread id; `codex resume <id>` opens it in the TUI. No flag pins a new
  thread id (§0.6).
- Gemini CLI (`gemini --resume <id>`?) and OpenCode (`opencode -s <id>`;
  `opencode acp` writes to the shared on-disk store, so the TUI should list
  its sessions) — plausible, **unverified**; spike §7.3. Unknown harness →
  no `tui` block → `set_transport(pty)` refused: "no tui command configured
  for <name>".

### 1.3 What wth already has

| Need | Exists | Where |
|---|---|---|
| ACP child actor, `session/load` on respawn, `acp_session` column | yes | `acp_child.rs`, `supervisor.rs:618-687`, `migrations/0006_acp.sql` |
| Runner enum | `LiveActor::{Native, Acp}` | `supervisor.rs:218-283`; 7 exhaustive matches (`:237-283, :1462-1473`), 15 `.native()?` sites |
| Respawn guard | `Agents::session` returns the existing `Live` when present | `supervisor.rs:440-582` |
| Job finish → owner's model | `Settle::FinishedWithResult` → `Agents::settle` | `supervisor.rs:1605-1638` |
| `agents.yaml` entry | `command, args, env, description` only, no `deny_unknown_fields` | `agents.rs:21-33` |
| `spawn_subagent` args | `task, name, agent, cwd, model, effort` | `peers.rs:34-53`; description line `agents.rs:97-116` |
| Start a PTY job outside a tool call | **no** — `StartPty` assumes `call_id`/`Activity` from a tool | `pty.rs:699-701`, `jobs.rs:317-327` |
| Harness-native session id read from `session_info_update` | **no**, and not needed (§1.2) | `acp_child.rs:667-749` |
| Interactive viewer, `pty_input` | 55 | — |

## 2. Model-facing contract

### 2.1 `spawn_subagent`

```rust
struct SpawnSubagentArgs {
    task, name, agent, cwd, model, effort,           // as today
    /// External harness only. `acp` (default): structured turns, send_message,
    /// results delivered. `pty`: the harness's own TUI on a terminal the user
    /// can also type into; drive it with send/screen/wait on job `<name>`;
    /// nothing is delivered until it exits.
    #[serde(default)] transport: Option<Transport>,   // acp | pty
}
```

`transport` without `agent`, or `pty` for an entry without `tui.new`, is a
tool error. The `External harnesses:` line marks entries with a `tui` block
`(tui)`.

### 2.2 Operating a `pty` child

The job is owned by the **parent** session (the operator), `name` = child
name, so `send {id: "claude", text: "…"}`, `screen {id: "claude"}`, `wait {id:
"claude", pattern: "❯"}` and `cancel_job` work with zero resolver changes
(`Jobs::pty(session, &id)`, `pty.rs:763-800`). `jobs` lists it as `j3 running
41s pty claude: claude --resume 6acc… | ❯`. The 55 `user attached` trailer
tells the model when the human is typing into it.

`send_message{to: <pty child>}` → `Err("<name> is attached to a terminal; use
send/screen/wait on job <name>, or cancel_job to end it")`.

### 2.3 Switching transport from the model

Not in v1. `spawn_subagent.transport` chooses; the human switches (§3.3); the
TUI exiting switches back. A model-callable `set_transport` is the same RPC
method registered as a tool — add it when a transcript shows the model
wanting to open a TUI it did not spawn as one.

## 3. Handoff

### 3.1 `agents.yaml` entry

```yaml
claude:
  command: npx
  args: [-y, "@agentclientprotocol/claude-agent-acp"]
  tui:                                   # optional; absent → acp only
    new:    [claude, --session-id, "{session}", "{task}"]
    resume: [claude, --resume, "{session}"]
    ready:  "❯"                          # optional readiness regex for `start`
codex:
  command: codex-acp
  tui:
    new:    [codex, "{task}"]            # no {session} → pty-only child (§0.6)
    resume: [codex, resume, "{session}"]
```

`Entry` gains `tui: Option<Tui>`, `Tui { new: Vec<String>, resume:
Vec<String>, ready: Option<String> }`. `{session}`/`{task}` are the only
placeholders; substitution is per argument, no shell. The command runs as
`$SHELL -lc` on the PTY exactly like `start` (22 §4.1), argv shell-quoted into
one string, `env` from the entry, `cwd` from the row.

`/acp add <registry-id>` writes a `tui` block for the ids it knows —
`claude`, `codex-acp`, `gemini`, `opencode` — from a four-row table in
`registry.rs`; the registry itself has no TUI field. User-editable, hot-reloaded
like the rest of the file. Rejected: a table consulted at spawn (06 §3.1
already rejected hidden tables; the yaml is where the user sees what will run).

### 3.2 `Live` gains a third arm

```rust
enum LiveActor {
    Native(ActorRef<Session>),
    Acp(ActorRef<AcpChild>),
    /// The harness's TUI holds the harness session. `owner` is the parent
    /// session whose Jobs row this is.
    Pty { owner: SessionId, job: JobId },
}
```

Arms: `is_alive` → job running (ask `Jobs`); `tell_input`/`ask_peer` →
`Err(PTY_ATTACHED)` (§2.2 text); `tell_cancel`/`kill` → `CancelJob { session:
owner, id: job }`; `is_running` → `false`; `native()` → `Err(NOT_NATIVE)`.
The 15 `.native()?` sites inherit the refusal unchanged. `cancel_peer_scope`
(`supervisor.rs:1116-1143`) treats `Pty` like `Acp`.

### 3.3 `set_transport(session, transport)` — `rpc::Api`, `Agents::SetTransport`

ACP → PTY, in the `Agents` mailbox:

1. Row must have `agent` and `tui.resume`; `acp_session` non-null;
   `is_running` false → else `Err` ("cancel the running turn first" / "no
   harness session yet" / "no tui command configured for <name>").
2. Build `StartPty { session: parent, args: StartArgs { command, name:
   child_name, cwd: row.cwd, env, ready }, .. }` and `ask` `Jobs`; on `Err`
   nothing changed. Then swap the arm to `Pty { owner, job }`, then `kill()`
   the old `AcpChild`. Between spawn and kill the TUI has had no input and
   the ACP side is idle by (1); read/read on the transcript is fine.
   **Seam:** `StartPty` today assumes a tool context (`call_id`, `Activity`);
   the activity must be admitted on the parent as a `start` call would be
   (`admit_job(parent, sink, owner, Kind::Pty)`, `jobs.rs:317-327`) from the
   supervisor — the one non-trivial piece of plumbing.
3. Publish `TransportChanged { session, transport }` — one new `AgentEvent`
   variant (the picker and board render it; 19 §1 "derive, don't
   duplicate"); `list_agents` rows carry `transport` so a late frontend
   needs no event.
4. Reply `(owner, job)` so the caller can open the 55 viewer on it.

PTY → ACP is not a method. `Agents::settle` (`supervisor.rs:1605-1638`), on
`JobEvent::Finished` for `(owner, job)`, finds the `Pty` child referencing
that job, removes its `Live` entry, publishes `TransportChanged { acp }`; the
row-based respawn does the rest on demand. The `<job_result pty="true">`
already goes to the owner — that *is* the result of the terminal session, in
the parent model's hands.

`spawn_subagent{transport: pty}` (`SpawnChild.transport`) is steps 2–4 with
`tui.new`, no ACP actor to kill, `acp_session` set to the minted UUID only if
`new` contains `{session}`, and no `StartChildTurn` (the task rode in as
`{task}`).

`Cancel` fan-out and private-tree close already cancel tree-owned PTY jobs;
the job is owned by the parent, which is in the tree. ACP children are
refused in private trees (51 §4.1); `pty` children inherit that.

### 3.4 History (open)

While the transport is `pty`, wth writes no messages for the child. On the
next ACP respawn `session/load` replays, but `acp_child.rs` persists the
replay only when local history is empty (06 §3.6). TUI-era turns end up in
the harness's transcript and on the final screen the parent received, not in
wth's rows. Options: dedupe replay against the last persisted
`user_message_chunk` (one function); or accept the gap. Decide after spike 1
shows what `session/load` replays for Claude in practice.

### 3.5 `terminal`-type auth (06 §3.7) comes free

`/acp` probe → `auth_required` with a `terminal` method `{args, env}` →
`start` the configured command + those args on a PTY job owned by the
current session, named `<name> login`, open the 55 viewer interactive. Exit
0 → re-probe. Replaces 06 §3.7's "the spawn fails with a tool error naming
the command". This piece does not depend on §3.1–3.4 and could ship alone.

## 4. Where "open in terminal" lives

- `/agents` picker (19): row shows `· acp` / `· pty`; key `t` on an external
  child → `set_transport(pty)` then opens `Menu::Terminal` on `(owner, job)`
  in interactive mode. On a `pty` child `t` just opens it.
- `/terminals` and PTY cards: unchanged entry points.
- Desktop: the agents board row gets a terminal icon → `set_transport` then
  `TerminalViewer` with that key selected and focused.
- Signal/Slack: nothing (22 §5).

## 5. Crates

None new. `agent-client-protocol` =2.1.0 (v1, `session/load`) is enough; v2
`session/resume` adds nothing this needs.

## 6. Config

None new. `tools.pty_ready_timeout_secs` (22 §6) bounds `tui.ready`. The
`tui` block lives in `~/.wth/acp/agents.yaml`.

## 7. Spikes (before any of §3 is built)

1. **Claude round trip.** `spawn_subagent{agent: claude, task}` → turn ends →
   kill the adapter → `claude --resume <acp_session>` in the row's cwd on a
   `start` job shows the conversation → type a follow-up → `/exit` →
   `send_message` to the child → `session/load` replays both turns,
   `TurnStarted{model: "claude"}` follows. Check the JSONL has one writer at
   a time. Everything here can be done by hand with today's tools — no code.
2. **Codex round trip**, same shape, `codex resume <id>`.
3. **Gemini / OpenCode flags**: does `gemini --resume <acp id>` exist and
   accept it; does `opencode -s <id>` see a session created by `opencode acp`.
   Outcome → the `/acp add` table rows, or "acp only".
4. **Kill safety.** `Harness::close()` ends in the SDK's `ChildGuard`
   SIGKILL of the process group (`acp_agent.rs:312-333`). Confirm an idle
   adapter killed that way leaves Claude's/Codex's persisted state intact
   (it should — persistence is per message/turn), or whether a `session/close`
   / SIGTERM grace is needed first.
5. **The cheap alternative** (§8 last bullet): with 55 shipped, does "the
   model runs `start {command: "claude --resume <id>"}` itself" cover the
   user-visible need well enough that the `Live` arm is not worth it? Try it
   on the same transcript as spike 1.

Tests that would stay if §3 is built:

- `supervisor::tests::transport_exclusive` — `set_transport(pty)` on a
  running child refused; on an idle child the `AcpChild` is dead before the
  reply; `enqueue_peer` to a `Pty` child does not spawn an `AcpChild`; job
  finish removes the `Live` entry and the next `enqueue_peer` respawns with
  `resume = acp_session`.
- `agents::tests::tui_template` — `{session}`/`{task}` substitution per
  argument; `new` without `{session}` leaves `acp_session` null.
- Headless smoke, real model: "spawn claude over pty to add a README, then
  read its screen and tell me when it's done" → `spawn_subagent{transport:
  pty}`, `wait {id, pattern: "❯"}`, `screen`; a human typing midway shows
  `user attached` in the model's next reply.

## 8. Rejected, and the alternative still on the table

- **Dual transport on one process.** Does not exist (§1.1). The closest real
  thing — OpenCode `serve` + `attach` + its HTTP API — is a per-vendor
  protocol, the `cursor` pattern 06 told us not to repeat.
- **Turn boundaries inferred from the screen** (settle N s → `TurnFinished`).
  A TUI thinking, blocked on a permission prompt, or idle at its input look
  alike from outside; a wrong `TurnFinished` delivers a half result and
  confuses the ledger (51 §6). The parent gets the final screen on exit and
  can `wait` on the prompt regex itself.
- **Running the ACP adapter inside a PTY** so the human "sees" the child.
  The screen would be JSON-RPC.
- **`PeerInput` → keystrokes.** A peer exchange row with a `request_id` and
  no possible reply; `send` already is the keystroke tool.
- **A model tool to switch transport in v1** (§2.3).
- **ACP v2 display-only terminals** as a third observation. The pinned SDK is
  v1; when 06 moves to v2 those `terminal` updates can flow into the same
  `pty_screen`-shaped page type as read-only remote terminals — same viewer,
  no input. Separate amendment then.
- **Still open — no `Live` arm at all.** Once 55 exists, the model can
  already do `start {command: "claude --resume <acp_session>", name:
  "claude"}` after the child is idle, and the human can type into it. What
  §3 adds over that: the exclusivity lock (§0.3 — without it a `send_message`
  during the TUI session respawns the adapter onto a live JSONL), the
  `acp_session` lookup (today not on `SessionSummary`; a `list_agents` field
  would do), the roster showing the transport, and `spawn_subagent{transport:
  pty}`. If spike 5 shows the lock is the only thing that matters, the
  minimal build is: expose `acp_session` in `list_agents`, and make
  `enqueue_peer` refuse to respawn while a PTY job named after the child is
  running in the parent — no `LiveActor::Pty`, no `set_transport`, no
  `TransportChanged`.

## 9. Edits to other docs (if built)

- **06 §3.2/§3.6:** `Live` third arm; `Entry.tui`; `SpawnSubagentArgs.transport`;
  `list_agents` rows carry `transport`; `AgentEvent::TransportChanged`.
  §3.7: `terminal` auth runs on a PTY job (§3.5). §3.8 unchanged.
- **19 §2:** picker row shows transport; key `t`.
- **25 §7:** `set_transport`, `TransportChanged`.
- **README:** `transport` field, `t` in `/agents`, `tui:` in agents.yaml.
- **`harness.md` skill:** "pty children are driven with send/screen/wait by
  name; send_message is for acp".

## 10. Order (if built)

1. Spikes 1–5; record outcomes in Status; choose §3 or the §8 minimal build.
2. §3.5 `terminal`-auth on a PTY job (independent, ships with 55).
3. `Entry.tui`, `/acp add` table, `LiveActor::Pty`, `set_transport`, `settle`
   hook, `TransportChanged`, `SpawnSubagentArgs.transport`,
   `list_agents.transport`; tests.
4. Picker `t`, board icon; docs (§9); smoke.
