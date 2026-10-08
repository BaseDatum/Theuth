# 141 hand-off — W1 (TRANSPORT)

Branch `feat/141-w1`, cut from C's contract commit `0b5f8aa2`. Everything
below is inside `crates/acp/src/**` (not `lib.rs`, not `omp/**`),
`crates/acp/fixtures/**` and one swap in `crates/rpc/src/acp.rs`.

## 1. What landed

### 1.1 Modules (declared from `harness.rs`, not `lib.rs`)

`crates/acp/src/lib.rs` is C's, so the three new modules are submodules of
`harness` and live under `crates/acp/src/harness/`:

| path | public as | what |
|---|---|---|
| `harness/rfd.rs` | `acp::harness::rfd` | `announce(parent: &SessionId, own: Option<&str>, u: SubagentUpdate) -> ChildAnnounce` (§6.1) |
| `harness/opencode.rs` | `acp::harness::opencode` | `CHILD_UPDATE_METHOD`; `Children` (per-connection memory) with `child_update(&UntypedMessage) -> Vec<Update>` and `spawn_join(&SessionUpdate) -> Option<ChildAnnounce>`; `task_v1::{is_task_call, announce}` (§6.2, §6.3) |
| `harness/process.rs` | `pub(crate)` | `ChildProcess` (`ConnectTo<Client>` wrapper around `AcpAgent`), `ExitSlot`, `EXIT_GRACE` (§4.3) |

**Closer**: one optional line in `lib.rs` — `pub use harness::{opencode, rfd};`
— if the design's `acp::opencode` / `acp::rfd` paths are wanted. Nothing
needs it to compile.

### 1.2 `Link::spawn` handlers (§7.1)

- Typed `SessionNotification`: `ConfigOptionUpdate` side effect kept; then
  `SubagentUpdate(u)` → `Update::Subagent(rfd::announce(&n.session_id, own, u))`;
  every other update (own traffic, RFD child traffic, `session_message*`) →
  `Update::Session { session: <wire sessionId>, update }` unchanged. Before
  that `Update::Session`, `Children::spawn_join(&update)` may yield one
  `Update::Subagent` (opencode 2.x join, below).
- Untyped: `PeerMessage::parse` first; else `Children::child_update(&n)` →
  0..2 `Update`s. **No more flattening**: the chunk→thought rewrite and the
  `rootSessionId` attribution are gone.
- Permission handler unchanged from C (`Permission.session` = wire id).
- `initialize_request()` adds `.subagents(SubagentCapabilities::new())`
  (serialises as `"subagents": {}`); test
  `initialize_advertises_subagents_and_opencode_child_updates`.
- `Link.own: Arc<Mutex<Option<String>>>` is set by `connect` right after
  `open()` succeeds; `rfd::announce` uses it to emit `parent: None` for a
  depth-1 announce. An RFD announce that arrives **before** the session id
  is known (only possible inside `session/load` replay, before the load
  response) carries `parent: Some(<harness session id>)` — W2's fallback
  "unknown external parent → the ACP child" (§5.3) resolves it.

### 1.3 Fixture → `Update` sequences (for W2's `Fake`)

All frames are full JSON-RPC notifications; send `["params"]` through
`Fake::send` or the whole frame through an `UntypedMessage`.

**opencode 2.0.22** (`crates/acp/fixtures/opencode-2.0.22/`, child
`C = ses_ee4d9a752ffe2XT5Oz2Fldml2j`, root `R = ses_ee4d9b05effekZbnkfXPDosL35`;
re-recorded from `opencode-2.0.22-task.jsonl` lines 11/13/15/20/22/26/27/28):

| fixture | `Update`s emitted |
|---|---|
| `child_update.status.created.json` | `Subagent{source: OpencodeExt, child: C, parent: None, title: "Read a.txt first line", state: Pending, cost_rule: Unreported}` |
| `child_update.status.running.json` | `Subagent{child: C, title, state: Running, cost_rule: None}` |
| `child_update.tool_call.json` | `Session{session: C, ToolCall{tool_call_id: "C:toolu_014DGrCCVSsRX5h8vujs7SZh", title: "Read a.txt first line: read", _meta["opencode/child-session"]}}` — **if C was not announced yet on this connection, preceded by** `Subagent{child: C, title, state: Running, cost_rule: Unreported}` |
| `child_update.tool_call_update.completed.json` | `Session{session: C, ToolCallUpdate{status: completed}}` |
| `child_update.agent_message_chunk.json` | `Session{session: C, AgentMessageChunk{"alpha line one"}}` (assistant text on the **child**) |
| `child_update.status.completed.json` | `Subagent{child: C, state: Completed}` |
| `child_update.status.failed.json` (**synthetic**) | `Subagent{child: C, state: Failed{error: Some("rate limited")}, detail.error = {type, message}}` |
| `child_update.status.interrupted.json` (**synthetic**) | `Subagent{child: C, state: Cancelled}` |
| `subagent.tool_call_update.in_progress.json` (parent frame, line 11) | `Session{session: R, ToolCallUpdate{..}}` only; the join remembers `rawInput.{agent: "general", description}` by `toolCallId` |
| `subagent.tool_call_update.completed.json` (parent frame, line 28) | `Subagent{child: C, tool_call: Some("toolu_018wLewXWWb6vBSf6pqKcnLn"), description: Some("Read a.txt first line"), detail.agent: "general", state: None}` **then** `Session{session: R, ToolCallUpdate{completed}}`. Emitted only when C was announced on this connection (no phantom rows). |
| `request_permission.diff.json` | unchanged 2026-10-03 recording: `Permission{session: "ses_f00418429ffe8gc9EX6IvZHlP6", tool_call.tool_call_id: "ses_f00417af0ffewZH4KxaHpEZ653:toolu_…"}` — the `<child>:<id>` split (§6.2) is W2's; its child id is **not** C |

Status `created|running|completed|failed|interrupted` ↔ `Pending|Running|Completed|Failed|Cancelled`; `parentSessionId != rootSessionId` → `parent: Some(parentSessionId)`; unknown status → nothing (debug log); unparsable `update` → announce only.

**opencode 1.18.30** (`crates/acp/fixtures/opencode-1.18.30/task.*`, lines 15/16/21). These are the parent's own frames, so the transport emits only `Update::Session{session: own, ..}` for them; `acp_child` calls `acp::harness::opencode::task_v1::announce(&update)` when `is_opencode_v1()` (agentInfo `OpenCode` < 2) and feeds the result to its upsert:

| fixture | `task_v1::announce` |
|---|---|
| `task.tool_call.json` | `{source: OpencodeTask, child: "task:call_a062fdd9472f454ea6e45c9b", title: "task", state: Pending, tool_call, cost_rule: Unreported}` |
| `task.tool_call_update.in_progress.json` | `{…, title: "Read first line of a.txt", description: <prompt ≤200 chars>, detail.agent: "explore", state: Running}` |
| `task.tool_call_update.completed.json` | `{…, state: Completed, model: "opencode/big-pickle", detail.sessionId: "ses_ee4d89f0cffe7zXAvHsSIT0UsD", detail.parentSessionId, detail.result: "alpha line one"}` |

**Deviation from §6.3/§13.2**: the external id is `task:<toolCallId>` for the
row's whole life; the real opencode id rides in `detail.sessionId`. The
contract (`ChildAnnounce`, `ObservedUpsert`) has no rename operation, and
nothing ever addresses a 1.x child by its session id (no traffic, no cancel —
F16, D12). If the Closer wants `acp_session` rewritten, it is a store-side
rename keyed on `detail.sessionId`, not a transport change.

**RFD** (`crates/acp/fixtures/rfd/`, **all synthetic**, README inside; `sess_parent` = harness session, `sess_child_1` = child):

| fixture | `Update` |
|---|---|
| `subagent_update.running.json` | `Subagent{source: Rfd, child: sess_child_1, parent: None, title, description, can_cancel: Some(true), state: Running, cost_rule: DisplayOnly, detail.meta}` |
| `subagent_update.idle.json` | `Subagent{state: Idle{stop_reason: Some("end_turn"), usage: Some(1200/300/1500)}}`, everything else `None` |
| `subagent_update.cancelled.json` | `Subagent{state: Cancelled}` (idle + `stopReason: cancelled`) |
| `subagent_update.requires_action.json` / `.unknown.json` | `Subagent{state: RequiresAction}` / `{state: Unknown}` |
| `subagent_update.cleared.json` (all nulls) | `Subagent{title: Some("sess_child_1"), description: Some(""), can_cancel: Some(false), state: Unknown, detail.meta = null}` |
| `child.agent_message_chunk.json`, `child.tool_call.json` | `Session{session: "sess_child_1", ..}` |
| `session_message_chunk.json` | `Session{session: "sess_parent", SessionMessageChunk{sender: sess_child_1, recipient: sess_parent}}` |
| `request_permission.child.json` | `Permission{session: "sess_child_1", ..}` |

### 1.4 §4.3 mitigation — `EXIT_GRACE = 1500 ms`

`process.rs`: `ChildProcess` implements `ConnectTo<Client>` by delegating to
`AcpAgent::connect_to` and publishing the result into `ExitSlot`
(`Mutex<Option<Result<(), Error>>>` + `Notify`). `Link::spawn`'s `main`:
`shutdown_rx` → `Ok(())` at once (the kill is ours); `incoming_closed` →
`slot.wait()` up to `EXIT_GRACE`, returning the driver's `Err` (→ `exit_text`
= `error_text(&e)` = `Internal error: Process exited with exit status: N: <stderr>`)
or `Ok(())` (→ `process exited`). Dial transports have no slot. The SDK's own
stderr-EOF wait after a non-zero exit is 1 s, hence 1.5 s.

Did the `exit 17` test fail before the mitigation? — **Yes (settles R2A G1).**
With `main` returning `Ok(())` straight after `incoming_closed` (the C-commit
behaviour), `process_exit_status_and_stderr_reach_the_text` failed **10/10**
when the test binary was run directly and 1/3 under `cargo test` (a race):
the connect error was `Internal error: response to `initialize` never
received: oneshot canceled` — on 3.1.0 a child that dies before answering
surfaces as a dropped reply, **not** as an incoming-closed error, so the old
`explain()` never consulted the connection task. Two changes together fix it:
the `main` grace wait above, and `Link::explain` now waits up to `EXIT_GRACE`
for the connection task on **any** failed request (not only incoming-closed)
and uses its result when it arrives. With both: 0/10 failures direct, green
under `cargo test`.

### 1.5 §4.4

`crates/rpc/src/acp.rs` `serve()`: `connect_with(transport, incoming_closed)` →
`.connect_to(transport)`. The loopback test's client side still uses
`connect_with` (it is the client, not the served side).

### 1.6 Revision 1 D14 — MCP back-channel

- `agents.rs`: `Entry.mcp` doc rewritten (default **on**, `mcp: false` opts
  out) and `Entry::mcp_enabled() -> bool` (`unwrap_or(true)`). Test
  `mcp_back_channel_defaults_on_and_false_opts_out`.
- Note: `supervisor.rs:1027` (W2's file) already does
  `entry.mcp.unwrap_or(true)` since commit `883449cf` — the default was
  already on in the engine; the brief's "today `mcp: true` opts in" was
  stale. W2/Closer may switch that site to `entry.mcp_enabled()` (cosmetic).
- `harness.rs` `Link::open`: a `session/new` | `session/load` that fails
  while carrying `mcpServers` (not `AuthRequired`, not transport-closed) is
  retried once without them, `tracing::warn!("acp <agentInfo.name | command>: session/<new|load> with the theuth MCP back-channel failed (<e>); retrying without it")`.
  A `session/load` that still fails falls back to `session/new` **with** the
  back-channel again (a load miss is usually the session, not the shim), and
  that may in turn drop it once. Test `mcp_back_channel_is_retried_without_on_failure`
  (scripted in-memory agent via a `Dialer`).

## 2. Assumptions about other waves

- W2 resolves `ChildAnnounce.parent: Some(ext)` with an unknown `ext` to the
  ACP child (§5.3) — needed for RFD announces during load replay.
- W2 calls `acp::harness::opencode::task_v1::announce` on own-session frames
  when `is_opencode_v1()`; the transport does **not** emit 1.x announces.
- W2 does **not** need anything for the opencode 2.x spawn join — the
  transport emits it as an `Update::Subagent` (tool_call/description/agent).
- W2 handles the `<child>:<id>` permission split (§6.2) and `Permission.session
  != own` (RFD) itself; `Permission` carries only the wire session id.
- `ChildState::Idle` keeps `usage` on the enum; W2 maps it to `detail.usage`
  (I did not duplicate it into `detail`).

## 3. Gate output

All through `BUILD_LABEL=141-W1 build-lock`, cold target (the warm clone never
finished), stable toolchain, `CARGO_BUILD_JOBS=4`, after merging
`feat/141-acp-subagents` at `63281cc4`/`5e375374` (C done + W5 merged):

- `cargo test -p acp` → `45 passed; 0 failed` (+ 0 doctests), no warnings.
  New: `agents::tests::mcp_back_channel_defaults_on_and_false_opts_out`,
  `harness::rfd::tests::{running_announce_carries_identity_and_capabilities, idle_and_cancelled_states, nulls_clear_and_requires_action_maps, child_traffic_and_messages_deserialize_on_the_child_session}`,
  `harness::opencode::tests::{child_update_fixtures_become_announces_and_child_traffic, child_update_implicit_announce_failed_interrupted_and_nesting, spawn_join_binds_the_subagent_call_to_the_announced_child, task_v1_fixtures_announce_one_child_through_its_lifecycle, task_v1_error_states}`,
  `harness::tests::{initialize_advertises_subagents_and_opencode_child_updates, process_exit_status_and_stderr_reach_the_text, mcp_back_channel_is_retried_without_on_failure}`.
  Removed: `child_update_fixtures_become_session_updates` (its re-pin is
  `child_update_fixtures_become_announces_and_child_traffic` in `opencode.rs`).
- `cargo check -p rpc --all-targets` → `Finished` clean.
- `cargo test -p rpc --lib -- acp::` → `6 passed; 0 failed` (incl.
  `child_session_over_the_mesh_loopback`).
- `process_exit_status_and_stderr_reach_the_text` run 10× directly from the
  test binary: 0 failures.

## 4. For the Closer

- Optional `lib.rs` re-export (§1.1).
- `supervisor.rs:1027` → `entry.mcp_enabled()` (cosmetic, W2's file).
- opencode 1.x `acp_session` rewrite (§1.3 deviation) if wanted.
- `request_permission.diff.json` child id ≠ the `child_update.*` child; a W2
  permission test must announce `ses_f00417af0ffewZH4KxaHpEZ653` first or
  edit the fixture's ids (it is a verbatim 2026-10-03 recording; I left it).
