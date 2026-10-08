# 141 — ACP SUBAGENT VISIBILITY

Status: DESIGN (2026-10-08). Branch `feat/141-acp-subagents` (cut from main after the 139-R2 merge `cd4c8b5c`; `crates/agent/migrations/0040_harness_outbox.sql` is already on this branch, so 0041 is free). Recon inputs, copied verbatim into this directory: `141-recon-R1-omp.md` (R1), `141-recon-R1B-opencode.md` (R1B), `141-recon-R2A-sdk.md` (R2A), `141-recon-R2B-model-ui.md` (R2B). Live captures: `/tmp/recon-141/omp-18.3.1-task.jsonl` (55 frames), `/tmp/recon-141/opencode-2.0.22-task.jsonl` (35), `/tmp/recon-141/opencode-1.18.30-task.jsonl` (30); on-disk omp subagent transcripts `~/.omp/agent/sessions/--private-tmp-recon-141-omp-scratch--/2026-10-08T10-53-47-639Z_01a11b25-d8f7-71b3-a309-a306e24281dc/{ReadA,ReadB}.{jsonl,md,json}`. Every theuth `path:line` below was re-read on this branch; recon cites that drifted are given with the branch's line.

## 0. Decisions in one screen

| # | Decision |
|---|---|
| D1 | **SDK.** `agent-client-protocol = { version = "=3.1.0", features = ["process", "stdio", "unstable_end_turn_token_usage", "unstable_subagents"] }` at `Cargo.toml:267`, lock updated, landed in the CONTRACT commit. "Lose nothing" = R2A's 30 parity rows become §4's checklist with a proof each; R2A §5.1 (3.1.0 `connect_with` no longer awaits the `AcpAgent` driver → crashed child's exit status/stderr tail degrade to `process exited`) is fixed with the newtype `ConnectTo` + `map_future` + bounded wait; the `t --acp` final-flush regression is fixed (`Builder::connect_to`); R2A's two regression tests are added (`sh -c "exit 17"` child; a raw `opencode/session/child_update` frame through a live `Fake`). |
| D2 | **Watch-only child sessions.** Every subagent a harness runs inside itself is a `sessions` row with `kind = 'observed'` (`SessionKind::Observed`), `parent` = the theuth session that owns it (the ACP child, or the observed parent for nested subagents), **no `agent`**, `acp_session` = the external id. UI word "Watching"/watch-only; code word `observed`. Migration `0041_observed_children.sql` adds `observed_source` (rfd \| opencode_ext \| opencode_task \| omp_task), `observed_state` (pending \| running \| requires_action \| idle \| completed \| failed \| cancelled \| unknown), `observed_stop`, `observed_cancel`, `observed_description`, `observed_tool_call`, `observed_detail` (JSON), `observed_cursor`, plus `CREATE INDEX IF NOT EXISTS sessions_parent ON sessions(parent)`. Identity = (parent, observed_source, acp_session). One guard in `Agents::session()` (`supervisor.rs:740-789`) refuses to drive observed rows; it checks `kind == Observed || observed_source IS NOT NULL`. |
| D3 | **Events.** New `AgentEvent::ObservedChildChanged { parent, child, name, model: Option<String>, observed: ObservedMeta }` (upsert; also on every state/detail change; detail-only changes throttled ≥ 250 ms per child). Child transcript content reuses `TurnStarted/TextDelta/ReasoningDelta/ToolCallStarted/ToolCallFinished/TurnFinished` on the child id via acp_child's `translate()` (`acp_child.rs:1372`), persisted like any history. `SubagentSpawned` is **not** reused (`tui/model.rs:5951-5956` destructures without `..`; Slack/Telegram bind reply threads on it). rpc `admitted`/`wanted` (`api.rs:6935/6964`) gain twin arms; `list_agents`/`list_sessions`/`load_session` carry `kind` + `observed`; hand-written `src/rpc/extra.ts` `AgentInfo` gains the fields. |
| D4 | **Source RFD (any ACP agent).** theuth advertises `clientCapabilities.subagents: {}` (`.subagents(SubagentCapabilities::new())` in `harness.rs:962` `initialize_request`). `subagent_update` = upsert (title → name, description, `capabilities.cancel` → observed_cancel, `state` running \| idle{stopReason, usage} \| requires_action \| unknown → observed_state/stop; idle.usage shown on the child). Child-addressed `session/update`s are routed by `params.sessionId` (`harness.rs:375` drops it today) into the child's transcript; nesting by the outer sessionId; `session_message`/`session_message_chunk` render as directed-message lines in sender and recipient transcripts. Child `session/request_permission` asks map to the child. Per-child stop = `session/cancel` with the child id, only when `observed_cancel`. Draft-name adapters (claude-agent-acp / codex-acp `subagent_spawned`/`subagent_state_update`) are out of scope. |
| D5 | **Source opencode 2.x** (`opencode/session/child_update`, opt-in `_meta["opencode/child-session-updates"]`): **stop flattening** (`harness.rs:811-829`). Envelope `{rootSessionId, childSessionId, parentSessionId, depth, title?}`: `type:"update"` → the child's own transcript (`agent_message_chunk` stays assistant text in the **child**, not thought in the parent); `type:"status"` created \| running \| completed \| failed \| interrupted(+error) → observed_state; `parentSessionId != root` → nested under the observed parent; child permission asks on the root with toolCallId `<child>:<id>` → the child; spawning tool call joined via the parent's `rawOutput.metadata.sessionID`. Child tokens/cost/model are not on the wire → "—". |
| D6 | **Source opencode 1.18.30**: only the task tool lifecycle reaches ACP. A minimal watch-only child from the task call (name from `rawInput.description`/`subagent_type`; running while the call runs; completed/failed at the end; child sessionId + model from the completed `rawOutput.metadata`), no transcript, note "opencode 1.x does not stream subagent activity". No HTTP route. |
| D7 | **Source omp** (`task` tool; no RFD support in 18.3.1/18.8.4/main). Two engine-side feeds in a new omp adapter in `crates/acp/src/omp/`: (1) live — every `tool_call_update` of a task call (also **after** its `completed`) → `rawOutput.details.progress[]` rows keyed (sessionId, toolCallId, progress.id), latest snapshot wins, status pending\|running\|completed\|failed\|aborted, model `resolvedModel`, live fields into observed_detail, nested ids `a.b` → parent `a`; (2) transcript + completion — tail `~/.omp/agent/sessions/<encoded-cwd>/<ts>_<acpSessionId>/<progress.id>.jsonl` (parent file by glob `*/*_<acpSessionId>.jsonl`, strip `.jsonl`; nested `<id>/<id>.<child>.jsonl`), map jsonl entries to transcript events, persist the byte cursor; detect completion from a terminal progress row, the `<id>.md` sidecar, or the parent jsonl's `custom_message async-result details.jobs[]` — required because omp drops all frames after the parent's turn ends. No per-child stop. |
| D8 | **Cost**, per child, never double counted: omp background (async) spawns are excluded from the parent's `usage_update` → child cost **counts** (meter rows with `observed` provenance); omp sync spawns are included in the parent → display-only; opencode (both) not reported → "—"; RFD display-only. The cost UI names the rule applied. |
| D9 | **UI/UX** (desktop/mobile/web, match the mock): (a) sidebar AgentTree + Agents page rows: eye glyph + source tag ("omp task", "opencode", "ACP"), state dot (blink on activity like design 93), cost; (b) the spawning tool card becomes a **roster card** (one row per child: dot/check, name, agent/description, current activity, tools, tokens, cost, chevron → child); (c) subagent strip includes live watch-only children (tooltip "watching"); (d) watch-only session view = `LiveSession` with **no Composer**: header "Watching" chip + model + tokens/cost + crumb "inside <parent> · <source>", the transcript, an `ObservedBar` (state, "<harness> runs this subagent, so you can watch it but not message it.", Open parent, Stop subagent only when `observed_cancel`); `ModelMenu` disabled; (e) TUI board rows `◎` + "watch · <src>", Enter opens read-only, Esc stop only when `observed_cancel`; (f) Slack/Telegram: board rows, no per-child threads. |
| D10 | **Not drivable, not messageable**: excluded from permit pools, `max_runtime`, durability resume, `send_message`/`discover_peers` targets (listed `send:false` or not at all); desk `live_child`/roster inherit (acceptable). Dreaming/search may index their transcripts. |
| D11 | **Idempotency/restart**: upserts keyed (parent, source, external id); `session/load` replay updates rather than duplicates; omp tail resumes at `observed_cursor`; after an engine restart, non-terminal watch-only children of a harness no longer connected become `unknown` ("lost contact") until a new observation arrives. |
| D12 | **Out of scope** (§13): theuth-as-harness emitting `subagent_update` over its own relay (`rpc/acp.rs`); claude/codex draft names; opencode HTTP route; per-child stop for omp/opencode. |
| D13 | **Upstream proposals** (Sky decides): omp — mirror `RpcSubagentRegistry` in `modes/acp` to emit RFD `subagent_update` + child-addressed updates (~150-200 lines) and stop dropping post-turn frames; opencode — emit RFD frames (it already bundles the schema). |

## 1. Why

Sky, 2026-10-08 11:19: *"When ACP subagents spawn subagents within their harness, we really don't have any visibility into that. Is there a way to do that in agent client protocol?"* — 11:46: *"I think it makes sense for us to upgrade to the latest rust sdk as long as we lose nothing. If we can implement how opencode and omp send subagent info now and integrate that, that would be ideal, full UI/UX and visibility too."*

What is invisible today, on this branch:

1. **The transport throws the child's identity away.** `crates/acp/src/harness.rs:375` forwards `Update::Session(n.update)` and drops `n.session_id`, so an RFD child's `session/update` (outer `sessionId` = child) would be indistinguishable from the parent's. The SDK pin `=2.2.0` (`Cargo.toml:267`) has no `subagent_update` variant at all: an RFD frame is a parse error that the SDK logs and drops before theuth's untyped handler (R2A §3.2) — on 2.2.0 an RFD harness can announce nothing.
2. **opencode 2.x children are flattened into the parent.** `harness.rs:811-829` `child_update()` keeps only `type == "update"` frames (status frames `created|running|completed|failed|interrupted` → `None`, :812), discards `rootSessionId/childSessionId/parentSessionId/depth/title` (:815 reads only `update`), and rewrites the child's `agent_message_chunk` to `agent_thought_chunk` (:816-822) so the child's answer shows as the parent's thinking. The capture shows two concurrent children whose only trace in theuth is `<child>:<toolu>` tool-card ids.
3. **omp children only exist inside one tool card's `rawOutput`.** Every subagent fact omp sends — status, model, tokens, cost, last intent, recent tools — rides `tool_call_update.rawOutput.details.progress[]` of the parent's `task` call (R1 §1, capture lines 9-44), and 27 of those updates arrive **after** the call reported `completed` (line 12 at t=4951, then `in_progress` frames to t=9781). `acp_child.rs:1434-1453` turns a `tool_call_update` into an event only on Completed/Failed; the `None => Vec::new()` arm at :1453 drops everything else. theuth shows "Spawned 2 background agents" and then nothing.
4. **opencode 1.x shows a 27.8 s hole.** Capture `opencode-1.18.30-task.jsonl` lines 19-22: child B's task call goes `in_progress` at t=9.51 and `completed` at t=37.27 with no frame in between; the child's session id and model appear only in the completed `rawOutput.metadata`.
5. **Nothing in the model can hold a non-drivable row.** `SessionKind` is `Chat | Advisor` (`crates/agent/src/lib.rs:2009-2015`); `AgentInfo` has `agent`, `model`, `state`, `kind` but no watch-only notion (`lib.rs:1172-1212`); `Agents::session()` lazily spawns an actor for any stored row (`supervisor.rs:740-789`) — a row standing for a harness-internal subagent would get a harness process or an LLM loop the moment anyone opened it.
6. **A remote theuth's subagents are invisible to the parent engine** too: the relay `crates/rpc/src/acp.rs:787` sends `SubagentSpawned` and grandchild events to `_ => {}`. Recorded as D12 follow-up.

## 2. Facts relied on

Each fact names its source (recon §, capture line, or path:line re-read on this branch).

| # | Fact | Source |
|---|---|---|
| F1 | theuth pins `agent-client-protocol = "=2.2.0"`, features `["unstable_end_turn_token_usage"]`; schema 1.9.1 has no subagent types. | `Cargo.toml:267`; R2A §1, R1B §8 |
| F2 | 3.1.0 gates native transports behind `process` (`AcpAgent`, `AcpAgentConfig`, `LineDirection`) and `stdio` (`Stdio`); `unstable_subagents` adds `SessionUpdate::{SubagentUpdate, SessionMessage, SessionMessageChunk}` and `ClientCapabilities.subagents`. Zero source breaks once the two features are on. | R2A §2.1, §3 |
| F3 | theuth's `connect_with` + `main` returning on `incoming_closed` (`harness.rs:414-423,437,445`) relied on 2.2.0 awaiting the transport driver; 3.1.0 passes `wait_owned_transport = false` and races, so a crashed child's `exit status: N` + stderr tail can degrade to `"process exited"` (`harness.rs:1009-1011` `exit_text`). | `harness.rs:414-461,1008-1015`; R2A §5.1 |
| F4 | `t --acp` serves via `connect_with(transport, incoming_closed)` (`rpc/acp.rs:418`) over `Stdio::new()` (`:90`); on 3.1.0 the final stdout flush is no longer awaited at stdin EOF; `Builder::connect_to(transport)` restores it. | `crates/rpc/src/acp.rs:90,301,418`; R2A §5.3 |
| F5 | Every theuth `match` on a schema enum has a wildcard (the enums are `#[non_exhaustive]`); new `SessionUpdate` variants fall to `_ => Vec::new()` in `translate` (`acp_child.rs:1459`) and to the `Update::Session(update)` catch-all (`acp_child.rs:1781`). | `acp_child.rs:1372-1465,1751-1812`; R2A §4.2 |
| F6 | `harness.rs:366-379` typed handler drops `n.session_id`; `:383-393` untyped handler routes `child_update(&n)` as the parent's `Update::Session`; `:394-404` the permission handler drops `r.session_id` too. | `harness.rs:364-413` |
| F7 | `child_update()` returns `Option<SessionUpdate>`; status frames → `None`; `agent_message_chunk` → `agent_thought_chunk`; envelope discarded. | `harness.rs:811-829` |
| F8 | `initialize_request()` advertises `terminal(false)`, `auth.terminal(true)`, `_meta["opencode/child-session-updates"]=true`, no `subagents`. | `harness.rs:962-974` |
| F9 | `Harness::cancel()` sends `CancelNotification::new(self.session.clone())` — only the owning session id. | `harness.rs:746-750` |
| F10 | RFD v1 (schema 1.10.2): `subagent_update {sessionId, title?, description?, capabilities?{cancel?}, state?}` on the **parent** session; first update for an unknown id announces the child and precedes child traffic; child traffic then arrives as `session/update` with outer `sessionId` = child; states `running \| idle{stopReason, usage} \| requires_action \| unknown`; no completed/failed state; `clientCapabilities.subagents: {}` must be advertised first. | R1B §8 |
| F11 | No shipped agent emits RFD v1 today: omp none (18.3.1/18.8.4/main), opencode none (2.0.22/v2/main), claude-agent-acp/codex-acp emit the **draft** names `subagent_spawned`/`subagent_state_update`. | R1 §7, R1B §2, §7 |
| F12 | opencode 2.0.22 `opencode/session/child_update` params: `{rootSessionId, childSessionId, parentSessionId, depth, title?} & ({type:"update", update: SessionUpdate} \| {type:"status", status: created\|running\|completed\|failed\|interrupted, error?})`; advertised back as `agentCapabilities._meta["opencode/child-session-updates"]: true`. | capture oc2 lines 2,13,15,20-27; R1B §1 |
| F13 | opencode 2.x child `toolCallId`s are `<childSessionId>:<toolCallId>`; every forwarded update carries `_meta["opencode/child-session"] = {id, parentID, depth, title}`; no child usage/model/tokens on the wire; child permission asks arrive on the **root** with the prefixed toolCallId. | capture oc2 lines 20-22; R1B §1 |
| F14 | opencode 2.x parent tool is `subagent`; its `tool_call_update completed` carries `rawOutput.metadata.sessionID` = the child id and no title/kind/rawInput; the `in_progress` update is the only place `rawInput.{agent, description, prompt}` appear. Order: child `status:completed` → parent `tool_call_update completed` (t=5.21 both). | capture oc2 lines 11,27,28 |
| F15 | opencode 2.x background children keep streaming `child_update` after the parent turn; root cancel cascades only to foreground children (status `interrupted`); no per-child cancel over ACP. | R1B §1 "Lifetime", §4 |
| F16 | opencode 1.18.30: `task` tool_call → `in_progress` with `rawInput {description, prompt, subagent_type}` → `completed` with `rawOutput.metadata {parentSessionId, sessionId, model{modelID, providerID}, truncated}`; no child-originated frame of any kind; `_meta` capability ignored. | capture oc1 lines 15,16,21; R1B §3, §6.2 |
| F17 | omp's `task` tool over ACP: `tool_call{title: intent, kind:"other", rawInput:{context, tasks[]}}`; updates carry full `rawOutput.details{projectAgentsDir, results[], totalDurationMs, usage?, progress[], async?}`; `content` text capped at 4000 chars but `rawOutput` untruncated; no `_meta`. | capture omp lines 8,12,27,44; R1 §1-2 |
| F18 | omp background mode (default): the task call is `completed` within ms of the spawn (line 12, t=4951, progress rows **stale `pending`**), then 27 more `in_progress` updates on the same `toolCallId` (lines 13-44) carry live rows; the final frame (line 44) has every row `completed` and `details.async.state:"completed"` but `status:"in_progress"`. | capture omp lines 12-44; R1 §1, §8 |
| F19 | omp `AgentProgress` row fields: `index, id, agent, agentSource, modelRole, status, task, assignment, recentTools[{tool,args,endMs}], recentOutput[], toolCount, requests, tokens (in+out+cacheWrite), cost (USD), durationMs, resolvedModel, resolvedModelIdentity, resolvedThinkingLevel, contextTokens, contextWindow, lastIntent, extractedToolData?, currentTool?, retryState?`; nested ids `<parent>.<child>`. | capture omp line 27 row 0; R1 §2 |
| F20 | omp drops every frame after the parent's turn ends (`#handlePromptEvent` subscribed per prompt); subagents' own tool calls/thoughts never reach the wire; no per-subagent cancel over ACP; `session/cancel` leaves background jobs running. | R1 §1 "Turn boundary", §5, §8 "Impossible" |
| F21 | omp on disk: `~/.omp/agent/sessions/<encoded-cwd>/<ISO-ts>_<sessionId>.jsonl` (parent) and `<same path minus .jsonl>/<agentId>.jsonl` (child), `<agentId>.md` final output, `<agentId>.json` structured output, nested `<agentId>/<agentId>.<child>.jsonl`; ACP `sessionId` == omp session id. Child jsonl entry types seen: `title, session{version, id, cwd, parentSession}, model_change, thinking_level_change, session_init, message(user), message(assistant{usage, model, provider, stopReason}), custom{customType:"tool_execution_start", data{toolCallId, toolName, startedAt, args, intent}}, message(toolResult{toolCallId, toolName, details, isError})`. | `ReadA.jsonl` entries 1-12 (re-read); R1 §4 |
| F22 | omp delivers an async job's result to the parent as a parent-jsonl `custom_message{customType:"async-result", details.jobs[{jobId, type, label, durationMs, schema{status, data}}]}` (parent entry 15), never as a wire frame. | parent jsonl entry 15 (re-read); R1 §8 |
| F23 | omp cost: `usage_update.cost` excludes async-spawn cost (0.13948325 vs children 0.12128025 + 0.12170525) and includes sync-spawn cost via `details.usage`; 18.8.4 identical on the wire. | capture omp line 53; R1 §6 |
| F24 | opencode cost: child cost is never in `usage_update.cost` in either version (2.0.22 root 0.002590045, each child 0.001741405 in sqlite); roll-up PR #43645 closed unmerged. | R1B §5 |
| F25 | `SessionKind = Chat \| Advisor`; `kind_str`/`kind_of` map strings (`store/advisor.rs:18-30`, unknown → `Chat`); `create_session_kind` inserts the row and `ensure_owner_in` (`store.rs:997-1026`); `set_acp_session` is the only `acp_session` write and `acp_session(id)` the only read — no reverse lookup. | `lib.rs:2009-2015`; `store/advisor.rs:18-30`; `store.rs:997-1026,1185-1204` |
| F26 | No index on `sessions(parent)` exists (the only parent index is `cost_owners_parent`, `0015_session_lifecycle.sql:16`). | `grep -i index crates/agent/migrations/*.sql` |
| F27 | `Agents::session()` returns the live actor or lazily creates one for any stored row; the only kind branch is the advisor bookkeeping (`:772-775`); `harness = acp.or(row.agent)` (`:776-779`) decides ACP vs native. | `supervisor.rs:740-789` |
| F28 | `Agents::Cancel` resolves the id set (`Root` or `Tree` via `descendants`) and tells each live actor; an id without a live actor is a silent no-op. | `supervisor.rs:3171-3209` |
| F29 | `SubagentSpawned{parent, child, name, cap}` is the only new-child event; `AgentEvent::session()` returns `child` for it; `tui/model.rs:5951-5956` destructures all four fields without `..`; `tui/model.rs:6152-6486` and `slack/channel.rs:2478-2682` are exhaustive matches with no wildcard. | `lib.rs:2480-2491,2859`; `tui/model.rs:5951,6152,6483`; `slack/channel.rs:2478,2642-2674` |
| F30 | rpc `admitted` (`api.rs:6935-6950`) and `wanted` (`:6964-6986`) special-case `SubagentSpawned` to enrol `child` in a private tree / a `descendants` subscription. | `api.rs:6935,6941,6964,6982` |
| F31 | Desktop: `LiveSession` renders `Header, …, Transcript, SubagentStrip, …, Composer` unconditionally (`c.$session.tsx:135-148`); `ModelMenu` decides ACP by `const acp = !!agent && !!session` (`ModelMenu.tsx:136`); `AgentInfo` in `src/rpc/extra.ts:95-121` is hand-written; children are learned only via `SubagentSpawned` (`reduce.ts:1308`, `lib/rpc.ts:183`). | re-read |
| F32 | `AcpChild::permission` builds `ExternalApproval{session: self.id, tool, args, risk}` from `p.tool_call.fields` — no session attribution beyond `self.id`. | `acp_child.rs:1242-1254` |
| F33 | `Connected` handler records `set_acp_session`, model, zero meter baseline — the natural place to seed the observed map from the store. | `acp_child.rs:1556-1599` |
| F34 | `MeterUpdate{origin, external_session, cumulative, confirmed_new_session, report_id}`; `CostScope::OpaqueRemoteInclusive = "opaque_remote_inclusive"`; `apply_meter` (`store/accounting.rs:629`), `session_cost` (`:785`), `ensure_owner_in` (`:63`) require the parent to be an owner already. | `cost.rs:210,453-464`; `store/accounting.rs:63,629,785` |
| F35 | Desktop files to touch exist: `components/sidebar/AgentTree.tsx` (`GLYPH` :16, `SubagentRow` :36, `AgentTree` :139), `components/chat/SubagentStrip.tsx` (`SubagentStrip` :16), `components/agents/AgentSessionRows.tsx`, `lib/session-tree.ts`, `store/reduce.ts`, `store/sessions.ts` (`children: Record<string, AgentInfo>` :249). | `ls`/grep on this branch |

## 3. Vocabulary

- **Harness** — the external ACP agent process theuth drives (`agents.yaml` entry: omp, opencode, opencode2, a remote theuth). Its theuth row is the **ACP child** (`agent` set, `acp_session` = its session id).
- **Watch-only child / observed child** — a subagent the harness runs *inside itself*. In theuth: a `sessions` row with `kind = 'observed'`, `parent` = the ACP child (or another observed row when nested), no `agent`. "Watching"/watch-only is the UI word; `observed` is the code word (`SessionKind::Observed`, `ObservedMeta`, `observed_*` columns, `AgentEvent::ObservedChildChanged`).
- **Source** (`observed_source`) — which feed produced the row: `rfd` (ACP RFD `subagent_update` + child-addressed updates), `opencode_ext` (`opencode/session/child_update`), `opencode_task` (opencode 1.x task-call lifecycle only), `omp_task` (omp `task` progress rows + on-disk tail). UI tag: `ACP`, `opencode`, `opencode`, `omp task`.
- **External id** (`acp_session` on an observed row) — the harness's own id for the child: RFD `subagentUpdate.sessionId`, opencode `childSessionId`, opencode 1.x `rawOutput.metadata.sessionId`, omp `progress[].id` (also its job id and transcript stem; nested `a.b`).
- **Spawn tool call** (`observed_tool_call`) — the parent-transcript tool call that created the child: omp `task` (`toolCallId` of the frame carrying the row), opencode `subagent`/`task` (joined via `rawOutput.metadata.sessionID`/`sessionId`). It is what the **roster card** replaces.
- **Roster card** — the spawn tool call rendered as one row per watch-only child (D9b).
- **Feeds** — the two omp inputs: *live* (progress rows on the wire while the parent's turn is open) and *tail* (the child's jsonl on disk, which also carries completion after the turn ended).
- **Announce** — the normalized `Update::Subagent(ChildAnnounce)` the acp crate emits for any source: an upsert of identity/state/detail. **Child traffic** — `Update::Session{session: <child external id>, update}`.
- **Cost rule** (`ObservedMeta.cost_rule`) — `counted` (child cost is its own meter rows and rolls up), `inside_parent` (already inside the parent's meter; display-only), `display_only` (reported by the harness, not aggregated), `unreported` ("—").
- **Lost contact** — `observed_state = 'unknown'` set by the engine when the owning harness is gone and the child was not terminal (D11), or by an RFD `state: unknown`.

## 4. SDK 3.1 upgrade and the lose-nothing checklist

### 4.1 Feature set and lock

```toml
# Cargo.toml:267
agent-client-protocol = { version = "=3.1.0", features = ["process", "stdio", "unstable_end_turn_token_usage", "unstable_subagents"] }
```

- `process` — `AcpAgent`, `AcpAgentConfig`, `LineDirection` used at `harness.rs:33,130,428-429` and `agents.rs:60-65` (R2A §2.1).
- `stdio` — `Stdio::new()` at `rpc/acp.rs:90` (R2A §2.1).
- `unstable_end_turn_token_usage` — unchanged name/semantics; also enables `IdleStateUpdate.usage` in 1.10.2 (D4 idle.usage).
- `unstable_subagents` — the RFD types (F2, F10). Not `unstable` (drags notices/compaction/fork/mcp_over_acp), not `schemars` (nothing needs it; R2A §1.4, R5).
- Lock: `cargo update -p agent-client-protocol --precise 3.1.0` moves exactly `agent-client-protocol 2.2.0→3.1.0`, `-derive 2.2.0→3.1.0`, `-schema 1.9.1→1.10.2`; the `schema→tracing` and `→schemars` edges disappear; no new crates (R2A §2.3). The contract wave checks `git diff Cargo.lock` shows only those three packages ± edges, and `cargo tree -p acp -i agent-client-protocol -e features` lists the four features and no `schemars`.
- `crates/acp`, `crates/agent`, `crates/rpc` take `workspace = true`; features are additive, so one line covers all three (R2A §2.1).

### 4.2 Parity checklist (R2A §5, 30 rows)

Class: **U** unaffected, **M** mechanical (feature only), **R** behavioural risk. "Proof" is what the Closer/Z waves run; test names are `cargo test` filters from R2A §5 (not re-run by this design).

| # | Behaviour | Class | Proof |
|---|---|---|---|
| 1 | `initialize` client capabilities (`terminal:false`, `auth.terminal`, no fs/elicitation, `_meta["opencode/child-session-updates"]`) + `clientInfo theuth/<ver>` — now also `subagents: {}` | U (+1 field) | `acp_child::tests::Rig::build` → `Fake::handshake` asserts `_meta`/no `elicitation` (R2A row 1); W1 extends the assertion with `subagents` |
| 2 | `agentInfo` → `Harness::agent_info`, `usage_is_lower_bound` for opencode <2 | U | `opencode_fixtures_initialize_and_config_options`, `opencode_usage_is_flagged_lower_bound_by_version` |
| 3 | Auth: `AuthRequired` → agent-type `authenticate` once; terminal-type → `not logged in — run <cmd>` | U | `login_command_appends_terminal_method`; live smoke §13 "logged-out omp" |
| 4 | `session/new {cwd, _meta.theuth, mcpServers}`; `_meta` omitted for non-theuth | U | `meta::tests::*`, `child_session_over_the_mesh_loopback` |
| 5 | `mcpServers` shim (`McpServer::Stdio` theuth exe) on new/load | U | live smoke: omp/opencode child calls `list_skills` through the shim |
| 6 | `session/load` when `loadSession`, fallback to `session/new`; `resumed()`; respawn hand-off | U | `shutdown_then_resume_loads_stored_session`, `respawn_after_successful_load_prompts_restored_note`, `respawn_after_failed_load_prompts_delta_handoff`, `respawn_without_stored_session_prompts_delta_handoff` |
| 7 | `set_config_option` model/effort at connect, alias resolution, refusal text | U | `resolve_finds_options_by_id_or_alias_and_checks_values`, `current_model_reads_the_model_select`, `set_option_when_idle_applies_and_updates_the_harness_model`, `set_option_refuses_a_missing_option_or_an_unlisted_value_with_the_list` |
| 8 | `set_config_option` queued mid-turn | U | `set_option_mid_turn_is_queued_and_applied_at_prompt_done` |
| 9 | `session/prompt` → `Turn`; stop reason serde name; `PromptResponse.usage` → `TurnFinished.usage` + ledger | U | `stop_reasons_are_serde_names`, `usage_fixtures_deserialize`, `prompt_response_usage_is_the_turn_usage_and_a_ledger_row` |
| 10 | Busy refusal retried with backoff | U | `busy_errors_are_recognised`, `busy_prompt_is_retried_until_accepted`, `busy_retries_stop_at_the_deadline`, `busy_retries_stop_at_the_busy_cap_before_the_deadline` |
| 11 | `session/cancel` + kill timer | U | `shutdown_then_resume_loads_stored_session` (asserts `session/cancel`), `kill_timer_drains_the_queue` |
| 12 | `request_permission` → `ExternalApproval`; outcome preference; cancelled without decision | U | `permission_outcome_prefers_once_and_cancels_without_decision`, `permission_with_no_turn_reaches_approval`, `permission_fixtures_deserialize_with_diff_and_locations` |
| 13 | Catch-all request → `method_not_found` | U | live smoke: omp `read` works (uses its own fs) |
| 14 | `session/update` → events (chunks, tool start/finish, `<child>:<id>` ids, output render) | U | `message_chunks_accumulate_text`, `tool_call_carries_kind_and_finishes_on_terminal_status`, `tool_calls_persist_in_order_with_text`, `unfinished_call_is_closed_with_an_error_at_turn_end`, `child_session_call_ids_with_a_colon_persist_and_pair`, `render::tests::*` |
| 15 | Continuation turns; replay-before-prompt dropped | U | `updates_after_the_turn_open_a_continuation_turn` |
| 16 | `plan` → todo snapshot | U | `plan_is_structured_state_not_transcript_text`, `stored_acp_plan_keeps_source_status_and_reopens` |
| 17 | `usage_update` with/without a turn → context, size, meter | U | `manifest_and_usage_apply_without_a_turn`, `cumulative_meter_attributes_high_water_once`, `usage_fixtures_deserialize` |
| 18 | `available_commands_update` → `HarnessCommandsChanged` | U | `manifest_and_usage_apply_without_a_turn` |
| 19 | `config_option_update` → option list + harness model | U | `prompt_response_usage_is_the_turn_usage_and_a_ledger_row` + `Rig::model_changed` |
| 20 | `session_info_update._meta["opencode/*"]` debug log | U | n/a (debug log) |
| 21 | opencode `child_update` extension reaches theuth (typed handler declines, untyped handler parses) | U — **but §6.2 changes what it becomes** | `child_update_fixtures_become_session_updates` re-pinned to the new `Update::Subagent`/`Update::Session{session: child}` shapes; **new** `Rig` test sending `crates/acp/fixtures/opencode-2.0.22/child_update.tool_call.json` verbatim via `Fake::send` and asserting `ToolCallStarted{call_id: "ses_…:toolu_…"}` on the **observed child's** id (D1 regression test 2) |
| 22 | `_theuth/peer_message` both directions | U | `peer::tests::*`; live smoke: remote theuth `send_message{to:"parent"}` |
| 23 | Connection lifetime: close/last-drop ends the task and kills the process group; `incoming_closed` → `Update::Closed` | **R** for `Transport::Process` | `dial_failure_is_the_connect_error`, `supervisor::tests::harness_exit_releases_the_external_permit` (dial side); **new** `exit 17` test (§4.3) for the process side |
| 24 | `Update::Closed` text = `exit status: N` + stderr tail on non-zero exit | **R** | **new** `#[cfg(unix)]` test: `Transport::Process(AcpAgentConfig::new("sh").arg("-c").arg("exit 17"))` → connect error contains `exit status: 17`; second case: child answers `initialize`+`session/new` then exits 3 → `Update::Closed(text)` contains `exit status: 3` (fails on bare 3.1.0 if the race is lost; passes with §4.3) |
| 25 | `probe()` texts (`ok` / `login required` / `spawn failed: <why>`); missing-executable text | **R** (same race) | the §4.3 test with a bogus command → `spawn failed: Internal error: No such file or directory (os error 2)` not `spawn failed: process exited`; live `/acp` probe |
| 26 | Remote theuth over the mesh (`MeshDialer` → `line_duplex` → `Lines`) | U (favourable: `Lines` finishes without waiting for remote EOF, R2A §5.2) | `child_session_over_the_mesh_loopback`, `mesh::tests::frames_one_message_per_line` |
| 27 | `t --acp` agent side: capabilities, `session/new|load|list|prompt|cancel`, `session/update`, `request_permission` | U | `session_new_meta_becomes_create_options`, `acp_cost_only_for_complete_single_currency`, `plan_keeps_blocked_and_dropped_honest`, `image_block_becomes_sha_named_file`, `child_session_over_the_mesh_loopback`; live smoke: omp as client of `t --acp` |
| 28 | `t --acp` stdout flush at stdin EOF | **R** (minor) | §4.4 swap to `connect_to`; live smoke: close the client, theuth exits cleanly and the last `session/update` was received |
| 29 | Child stderr → `<data_dir>/logs/acp-<name>.log` via `with_debug(LineDirection::Stderr)` | M | live smoke: log grows for omp/opencode/opencode2 |
| 30 | Fixture deserialization of recorded opencode frames | U | `opencode_fixtures_initialize_and_config_options`, `child_update_fixtures_become_session_updates`, `permission_fixtures_deserialize_with_diff_and_locations`, `usage_fixtures_deserialize` |

### 4.3 R2A §5.1 mitigation — `ConnectTo` newtype with `map_future` + bounded wait

Problem (F3): 3.1.0 normalizes `AcpAgent` to an opaque driver and `connect_with` does not await it; after theuth's `main` returns on `incoming_closed`, `run_until_connection_close` polls the background once, so the child's `finish_child_exit` error (`exit status: N` + stderr tail) is usually lost and `exit_text` yields the bare `"process exited"` (`harness.rs:1011`).

Design (W1, `crates/acp/src/harness.rs` + a new `crates/acp/src/process.rs`):

```rust
/// `AcpAgent` whose driver result is kept for `main` (R2A §5.1).
pub(crate) struct ChildProcess {
    inner: AcpAgent,
    exit: Arc<ExitSlot>,          // Mutex<Option<Result<(), Error>>> + tokio::sync::Notify
}
impl ConnectTo<Client> for ChildProcess {
    fn into_channel_and_future(self) -> (Channel, Option<ConnectionDriver>) {
        let (channel, driver) = self.inner.into_channel_and_future();
        let slot = self.exit.clone();
        (channel, driver.map(|d| d.map_future(move |fut| async move {
            let r = fut.await;
            slot.set(r.clone());      // publishes the driver's exit result and notifies
            r
        })))
    }
}
```

`main` (`harness.rs:414-423`) becomes: after `incoming_closed` fires, wait up to `EXIT_GRACE` (1.5 s) on `exit.notified()`; if the slot holds `Err(e)` return `Err(e)` (→ `exit_text` = `error_text(&e)`, the 2.2.0 text), else `Ok(())`. The `shutdown_rx` branch (theuth closing) does not wait — the kill is ours. `Transport::Dial` is unchanged (`Lines` already finishes cooperatively, R2A §5.2). The `probe()`/`explain()` paths (`harness.rs:469-476`) need no change: they already await `done`.

Why not mitigation 1 (tail our own stderr log): it loses the numeric exit status and reads a file the SDK is still writing. Why not wait for upstream `connect_with_owned`: not in 3.1.0 (R2A §5.1 item 3).

### 4.4 `t --acp` final flush

`crates/rpc/src/acp.rs:418-422` `connect_with(transport, async |cx| { cx.incoming_closed().await; Ok(()) })` → `Builder::connect_to(transport)`: same semantics with `wait_owned_transport = true`, so the `Stdio` driver's last stdout write is awaited at stdin EOF (R2A §5.3, R5). `serve()` uses no `cx` outside handlers, so it is a one-line swap. Owner: W1 (`crates/rpc/src/acp.rs` SDK-compat edits).

### 4.5 Regression tests added with the bump

1. **Process-exit text** (`crates/acp/src/harness.rs` tests, `#[cfg(unix)]`): the two cases of row 24 plus the bogus-command case of row 25.
2. **Raw `child_update` through a live `Fake`** (`crates/agent/src/acp_child.rs` tests): the frame `crates/acp/fixtures/opencode-2.0.22/child_update.tool_call.json` sent verbatim; expect `ObservedChildChanged{observed.source: OpencodeExt, observed.external: "ses_ee4d9a752ffe2XT5Oz2Fldml2j"}` then `ToolCallStarted{session: <child row>, call_id: "ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh"}`.
3. **Unknown `sessionUpdate` tolerance** (`acp_child.rs` tests): `Fake::send` a `session/update` with `"sessionUpdate": "totally_unknown"` → the connection survives and the next `end_turn` still finishes the turn (R2A §3.2).

## 5. The watch-only child model

### 5.1 Row shape

A watch-only child is an ordinary `sessions` row, so `descendants`, `list_agents`, `list_sessions`, `subtrees()`, `delete_session` cascade, search and dreaming work unchanged (R2B §9.1). Columns in use:

| column | value | note |
|---|---|---|
| `id` | theuth `SessionId`, minted at first observation | stable for the row's life |
| `kind` | `'observed'` | `SessionKind::Observed`; `kind_str`/`kind_of` (`store/advisor.rs:18-30`) learn it. Older engines map it to `Chat` — hence the guard also checks `observed_source` (D2) |
| `parent` | the ACP child's id, or the observed parent's id when nested | the only tree link; `CREATE INDEX sessions_parent` (F26) |
| `agent` | **NULL** | otherwise `Agents::session()` would spawn a harness (`supervisor.rs:776-788`) and `directory()` would call it `PeerKind::Acp` |
| `acp_session` | the external id (§3) | identity component; unique with (parent, observed_source) |
| `name` | display name: RFD `title`, opencode `title` (= the `description` input), omp `progress[].id` (`ReadA`) | shown in tree/strip/roster |
| `title` | same as `name` at creation; RFD/opencode title updates rewrite it | `TitleChanged` reuse |
| `model` | `harness_model` of the parent at creation (display fallback) | `AgentInfo.model` |
| `harness_model` | the child's own model when known: omp `resolvedModel`, opencode 1.x `metadata.model.providerID/modelID`; RFD none; opencode 2.x none | `HarnessModelChanged` reuse |
| `observed_source` | `rfd \| opencode_ext \| opencode_task \| omp_task` | |
| `observed_state` | `pending \| running \| requires_action \| idle \| completed \| failed \| cancelled \| unknown` | table below |
| `observed_stop` | stop reason or error text | RFD `idle.stopReason`; opencode `error.message`; omp `retryFailure.errorMessage` / `"aborted"`; engine `"lost contact"` |
| `observed_cancel` | 0/1 | RFD `capabilities.cancel` present; always 0 for opencode/omp |
| `observed_description` | RFD `description`; opencode `rawInput.description`/`prompt` head; omp `assignment` head (`agent` type in detail) | roster card second line |
| `observed_tool_call` | spawn tool call id when known | roster card join |
| `observed_detail` | JSON, source-specific (§6 mapping tables) | roster/ObservedBar live fields |
| `observed_cursor` | omp child jsonl byte offset | tail resume |
| `created_at/updated_at` | first/last observation | tree ordering |

`ObservedMeta` (contract type, `crates/agent/src/lib.rs`, ts-exported):

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ObservedMeta {
    pub source: ObservedSource,          // rfd | opencode_ext | opencode_task | omp_task
    pub external: String,                // the harness's id for the child (= sessions.acp_session)
    pub state: ObservedState,            // 8 values, snake_case
    #[cfg_attr(feature = "ts", ts(optional = nullable))] pub stop: Option<String>,
    pub can_cancel: bool,
    #[cfg_attr(feature = "ts", ts(optional = nullable))] pub description: Option<String>,
    #[cfg_attr(feature = "ts", ts(optional = nullable))] pub tool_call: Option<String>,
    pub cost_rule: CostRule,             // counted | inside_parent | display_only | unreported
    #[serde(default)] pub detail: serde_json::Value,   // §6 per-source JSON
}
```

### 5.2 States and transitions

| `observed_state` | meaning | `AgentState` shown (`state_of`) | entered from |
|---|---|---|---|
| `pending` | announced, not started | `Idle` (UI label "pending") | omp row `status:"pending"`; opencode `status:"created"` |
| `running` | the harness is working on it | `Running` | RFD `state:running`; opencode `status:"running"` or first `type:"update"`; omp `status:"running"`; omp tail sees a new assistant/tool entry |
| `requires_action` | waits on a human (permission/elicitation) | `Approval{tool}` | RFD `state:requires_action`; a child-attributed `request_permission` from opencode/RFD |
| `idle` | turn ended, child may be prompted again by the harness | `Idle` | RFD `state:idle{stopReason}` (terminal only from the harness's view when it stops announcing) |
| `completed` | finished successfully | `Idle` | opencode `status:"completed"`; opencode 1.x task `completed` with `state="completed"` in output; omp `status:"completed"`, `<id>.md` sidecar, or parent `async-result` job |
| `failed` | finished with an error | `Error` | opencode `status:"failed"` (+error); opencode 1.x task `failed` / `state="error"`; omp `status:"failed"` or `retryFailure` |
| `cancelled` | stopped by a cancel | `Cancelled` | opencode `status:"interrupted"`; omp `status:"aborted"`; RFD `idle{stopReason:"cancelled"}` |
| `unknown` | lost contact / activity unobservable | `Idle` (UI label "lost contact") | RFD `state:unknown`; engine restart with the harness gone (D11); parent harness `Update::Closed` while the child was non-terminal and not omp-tailable |

Transitions are upserts: any observation may move any state to any other (the harness is authoritative). Terminal states (`completed`, `failed`, `cancelled`) are kept on a later `unknown` mark (the restart sweep touches non-terminal rows only). A transcript turn is open on the child while `running`/`requires_action`; it is closed with `TurnFinished{stop_reason}` on leaving them (`stop_reason` = `end_turn` for idle/completed, `error` for failed, `cancelled`, `unknown` for lost contact).

### 5.3 Identity and invariants

- **Identity** = `(parent, observed_source, acp_session)`; enforced by the partial unique index in §11. The upsert resolves the row by this triple before inserting; a second announce for the same triple updates. Two sources never describe the same child (omp has one feed pair; opencode 2.x `opencode_ext` and the parent's `subagent` tool join on the same `childSessionId` → one row).
- **Nesting**: `parent` is resolved from the external parent id (`parentSessionId` ≠ root, RFD outer sessionId of a `subagent_update`, omp `a.b` → `a`) through the same lookup; an unknown external parent falls back to the ACP child (flat) and is corrected on the next announce that names it.
- **No `agent`, no `turn_steps`, no permits, no timer**: an observed row never appears in `child_turns`, `queued`, `TreePools`, `max_runtime` (`lib.rs:3159`), `resume_scan` (`supervisor/resume.rs:92`), `respawn_acp_children` (`supervisor/acp_resume.rs:27`) — all of them key on things an observed row never gets (D10, R2B §3).
- **Cost owner exists**: `create_session_kind` runs `ensure_owner_in(id, parent)` (`store.rs:1023`); the parent (ACP child) is already an owner, so `cost_owners` has the row and `session_cost` rolls up whatever meter rows the cost rule allows (§7.6).
- **Transcript rows before events** (`acp_child.rs:1800-1805` pattern): `Store::append(child_id, …)` then `publish`.
- **Never driven**: see §5.4.

### 5.4 The supervisor guard

One check in `Agents::session()` (`supervisor.rs:740-789`), placed after the row read (`:767`) and before the harness branch (`:776`):

```rust
// 141 §5.4: a watch-only child is the harness's; nothing here drives it.
if kind == SessionKind::Observed || row.as_ref().is_some_and(|r| r.observed.is_some()) {
    anyhow::bail!("{OBSERVED}");
}
const OBSERVED: &str = "watch-only: the harness runs this subagent; open or message its parent";
```

This single choke point covers `SendInput`/`Steer`/`UseSkill`/`LateAnswer`, `Undo`/`Redo`/`Compact`, `actor()`, `replay`, `ResumeChild` (R2B §3 table). The `row.observed.is_some()` half protects a row written by this engine but read by an older binary whose `kind_of` maps `'observed'` to `Chat` (D2). Additional kind arms (not guards): `resolve()` (`supervisor.rs:1436`) and `enqueue_peer()` (`:1630`) extend their Advisor refusal to Observed with the reason "watch-only harness subagent — message its parent"; `directory()` (`:4898`) lists observed rows with `send: false`; `Cancel` (`:3171`) gets the forward described in §7.7; `state_of` (`:5657`) gets the table above.

## 6. Sources -> model

Wire excerpts are verbatim from the capture files (line numbers are the jsonl line; `…` marks only long prose fields elided by this document).

### 6.1 RFD (`unstable_subagents`, any compliant agent)

No capture exists — no shipped agent emits it (F11). Shapes from schema 1.10.2 (R1B §8), test `client.rs:4375-4393`:

```json
{"sessionId": "parent", "update": {"sessionUpdate": "subagent_update", "sessionId": "child"}}
{"sessionId":"sess_child_1","capabilities":{"cancel":{}}}
```

Flow: theuth advertises `clientCapabilities.subagents: {}` (F10). The agent sends `subagent_update` on the **immediate parent** (outer `sessionId` = the ACP session for depth 1, an observed child's id for depth 2). Child traffic arrives as plain `session/update` with outer `sessionId` = child. `session_message`/`session_message_chunk` arrive in the transcript of the session that sent/received, naming the other party via `senderSessionId`/`recipientSessionId`.

| wire | column / event |
|---|---|
| outer `params.sessionId` of the `subagent_update` | `parent` (lookup by external id; the harness's own session → the ACP child) |
| `subagentUpdate.sessionId` | `acp_session` (external id) |
| `title` (`MaybeUndefined`: absent = unchanged, null = clear) | `name`/`title` → `TitleChanged` |
| `description` | `observed_description` |
| `capabilities.cancel` present | `observed_cancel = 1`; absent key = unchanged; null = 0 |
| `state.state = running` | `observed_state = running`; open child turn |
| `state.state = idle`, `stopReason`, `usage` | `observed_state = idle` (or `cancelled` when `stopReason == "cancelled"`), `observed_stop = stopReason`; `usage` → `observed_detail.usage {inputTokens, outputTokens, totalTokens, cachedReadTokens, cachedWriteTokens}` (display-only, D8); close child turn with `TurnFinished{usage}` |
| `state.state = requires_action` | `observed_state = requires_action` |
| `state.state = unknown` | `observed_state = unknown`, `observed_stop = null` |
| `state` null | `observed_state = unknown` (schema: activity unknown/unset) |
| `state` absent | unchanged |
| `_meta` | `observed_detail.meta` (opaque) |
| child-addressed `session/update` (`sessionId` = known child) | `translate()` on the child's turn → child transcript events |
| child-addressed `session/update` for an **unknown** `sessionId` | buffered 2 s then dropped with a warn (the RFD requires the announce first; adapters may race) |
| `session_message{messageId, senderSessionId?, recipientSessionId?, content[]}` / `_chunk` on session S | one `MessageDelivered`-style directed line in S's transcript: `"→ <recipient name>: <text>"` when S is the sender, `"← <sender name>: <text>"` when S is the recipient; chunks accumulate per `messageId` into one line (persisted as a `HistoryEntry::Info`-class message on S) |
| child `session/request_permission` (`params.sessionId` = child) | `ExternalApproval{session: child_row}`; `observed_state = requires_action` while pending |

`observed_source = rfd`, `cost_rule = display_only`, `harness_model` unknown (the RFD carries no model; `model` = parent's harness model as fallback).

### 6.2 opencode 2.x (`opencode/session/child_update`)

Capture `opencode-2.0.22-task.jsonl`, child A. Announce (status) frames, lines 13 and 15:

```json
{"jsonrpc": "2.0", "method": "opencode/session/child_update", "params": {"rootSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "childSessionId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line", "type": "status", "status": "created"}}
{"jsonrpc": "2.0", "method": "opencode/session/child_update", "params": {"rootSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "childSessionId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line", "type": "status", "status": "running"}}
```

Child traffic, lines 20 and 26:

```json
{"jsonrpc": "2.0", "method": "opencode/session/child_update", "params": {"rootSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "childSessionId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line", "type": "update", "update": {"sessionUpdate": "tool_call", "toolCallId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh", "title": "Read a.txt first line: read", "kind": "read", "status": "pending", "locations": [], "rawInput": {}, "_meta": {"opencode/child-session": {"id": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentID": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line"}}}}}
{"jsonrpc": "2.0", "method": "opencode/session/child_update", "params": {"rootSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "childSessionId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line", "type": "update", "update": {"sessionUpdate": "agent_message_chunk", "messageId": "msg_11b265c90001y8C3GiljPe61do", "content": {"type": "text", "text": "alpha line one"}, "_meta": {"opencode/child-session": {"id": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentID": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line"}}}}}
```

Completion (line 27), the parent's `subagent` tool call closing right after it (line 28), and — quoted last for reference — that call's earlier `in_progress` update (line 11), the only frame carrying `rawInput`:

```json
{"jsonrpc": "2.0", "method": "opencode/session/child_update", "params": {"rootSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "childSessionId": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "parentSessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "depth": 1, "title": "Read a.txt first line", "type": "status", "status": "completed"}}
{"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "update": {"sessionUpdate": "tool_call_update", "toolCallId": "toolu_018wLewXWWb6vBSf6pqKcnLn", "status": "completed", "locations": [], "content": [{"type": "content", "content": {"type": "text", "text": "<subagent sessionID=\"ses_ee4d9a752ffe2XT5Oz2Fldml2j\" state=\"completed\">\nalpha line one\n</subagent>"}}], "rawOutput": {"metadata": {"sessionID": "ses_ee4d9a752ffe2XT5Oz2Fldml2j", "status": "completed", "truncated": false}}}}}
{"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "ses_ee4d9b05effekZbnkfXPDosL35", "update": {"sessionUpdate": "tool_call_update", "toolCallId": "toolu_018wLewXWWb6vBSf6pqKcnLn", "status": "in_progress", "kind": "think", "title": "subagent", "locations": [], "rawInput": {"agent": "general", "description": "Read a.txt first line", "prompt": "Read the file a.txt in the working directory /tmp/…"}}}}
```

Mapping (replaces `child_update()` `harness.rs:811-829`, D5):

| wire | column / event |
|---|---|
| `childSessionId` | `acp_session` |
| `parentSessionId == rootSessionId` | `parent` = the ACP child; else `parent` = the observed row whose `acp_session == parentSessionId` (depth-2; opencode caps at `experimental.subagent_depth`, default 1) |
| `title` | `name`/`title`; absent on later frames = unchanged |
| `type:"status"` `created` | `observed_state = pending` |
| `running` | `running`; open child turn |
| `completed` | `completed`; close turn `end_turn` |
| `failed` + `error{type, message}` | `failed`; `observed_stop = error.message`; `observed_detail.error = {type, message}`; close turn `error` |
| `interrupted` | `cancelled`; close turn `cancelled` |
| `type:"update"` `update.*` | **unchanged** `SessionUpdate` (no chunk→thought rewrite) → `translate()` on the child's turn: `agent_message_chunk` → `TextDelta` (child assistant text), `agent_thought_chunk` → `ReasoningDelta`, `tool_call`/`tool_call_update` → `ToolCallStarted/Finished` with the **`<child>:<id>` id kept** (unique; the replay dedupe key), `session_info_update._meta["opencode/retry" \| "opencode/compaction"]` → debug log; a `type:"update"` for a child never announced → implicit announce `pending→running` (opencode sends `created` first, but the fixture set starts mid-stream) |
| parent `subagent` tool `tool_call_update completed` with `rawOutput.metadata.sessionID` | `observed_tool_call = toolCallId` on the row with that `acp_session`; roster card join. The earlier `in_progress` `rawInput.{agent, description}` → `observed_detail.agent`, `observed_description` (matched by toolCallId once the join exists) |
| root `session/request_permission` with `toolCall.toolCallId = "<child>:<id>"` | split on the first `:`; prefix = known child → `ExternalApproval{session: child_row}`; else the parent as today |
| tokens/cost/model | not on the wire → `cost_rule = unreported`, "—" |

`observed_source = opencode_ext`. opencode 1.x never sends this method (capture oc1: zero), so the two opencode sources never collide.

### 6.3 opencode 1.18.30 (task tool lifecycle only)

Capture `opencode-1.18.30-task.jsonl` lines 15, 16, 21:

```json
{"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "ses_ee4d8bb5dffeS1akqW7B497imb", "update": {"sessionUpdate": "tool_call", "toolCallId": "call_a062fdd9472f454ea6e45c9b", "title": "task", "kind": "think", "status": "pending", "locations": [], "rawInput": {}}}}
{"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "ses_ee4d8bb5dffeS1akqW7B497imb", "update": {"sessionUpdate": "tool_call_update", "toolCallId": "call_a062fdd9472f454ea6e45c9b", "status": "in_progress", "kind": "think", "title": "Read first line of a.txt", "locations": [], "rawInput": {"description": "Read first line of a.txt", "prompt": "Read the file /private/tmp/recon-141/oc-scratch/a.…", "subagent_type": "explore"}}}}
{"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "ses_ee4d8bb5dffeS1akqW7B497imb", "update": {"sessionUpdate": "tool_call_update", "toolCallId": "call_a062fdd9472f454ea6e45c9b", "status": "completed", "title": "Read first line of a.txt", "content": [{"type": "content", "content": {"type": "text", "text": "<task id=\"ses_ee4d89f0cffe7zXAvHsSIT0UsD\" state=\"completed\">\n<task_result>\nalpha line one\n</task_result>\n</task>"}}], "rawOutput": {"output": "<task id=\"ses_ee4d89f0cffe7zXAvHsSIT0UsD\" state=\"completed\">\n<task_result>\nalpha line one\n</task_result>\n</task>", "metadata": {"parentSessionId": "ses_ee4d8bb5dffeS1akqW7B497imb", "sessionId": "ses_ee4d89f0cffe7zXAvHsSIT0UsD", "model": {"modelID": "big-pickle", "providerID": "opencode"}, "truncated": false}}}}}
```

Recognition: harness `agentInfo.name == "OpenCode"` with major version < 2 (`usage_is_lower_bound` already parses it, `harness.rs:786-796`) and a `tool_call`/`tool_call_update` whose `title == "task"` (first frame) or whose `rawInput` has `subagent_type` (the in_progress frame). Keyed by `toolCallId` until the completed frame supplies the real external id.

| wire | column / event |
|---|---|
| `in_progress` `rawInput.description` | `name` (provisional external id = `task:<toolCallId>` until completion) |
| `rawInput.subagent_type` | `observed_detail.agent` |
| `rawInput.prompt` | `observed_description` (first 200 chars) |
| `in_progress` | `observed_state = running` (no transcript turn — nothing will arrive) |
| `completed` `rawOutput.metadata.sessionId` | `acp_session` rewritten from `task:<toolCallId>` to the real id (same row) |
| `metadata.model {providerID, modelID}` | `harness_model = "<providerID>/<modelID>"` |
| `metadata.parentSessionId` | sanity: must equal the ACP session; else flat under the ACP child |
| `rawOutput.output` `<task … state="completed">` / `state="error"` / tool `status: failed` | `observed_state = completed` / `failed` (+ `<task_error>` text → `observed_stop`) |
| `rawOutput.output` `<task_result>…</task_result>` | the child's only transcript content: one assistant message with that text, persisted on the child so the view is not empty |
| `observed_tool_call` | the `toolCallId` |

`observed_source = opencode_task`, `cost_rule = unreported`. The ObservedBar shows "opencode 1.x does not stream subagent activity". No HTTP route (D6).

### 6.4 omp (`task` tool + on-disk transcript)

Capture `omp-18.3.1-task.jsonl`. The spawn call (line 8; `context`/`task` prose elided):

```json
{"sessionUpdate": "tool_call", "toolCallId": "toolu_013x6rgBK7MEUsQq79DoL4YK", "title": "Spawning parallel file readers", "kind": "other", "status": "pending", "rawInput": {"context": "# Goal\nReport the first line of a text file in cwd `/tmp/rec…", "tasks": [{"name": "ReadA", "agent": "scout", "task": "# Target\n`a.txt` in cwd.\n# Change\nRead i…", "outputSchema": {"type": "object", "properties": {"first_line": {"type": "string"}}, "required": ["first_line"]}}, {"name": "ReadB", "agent": "scout", "task": "# Target\n`b.txt` in cwd.\n# Change\nRead i…", "outputSchema": {"type": "object", "properties": {"first_line": {"type": "string"}}, "required": ["first_line"]}}]}}
```

The `completed` frame 5 ms later (line 12, t=4951) — a spawn acknowledgement with **stale** rows:

```json
"status": "completed", "rawOutput": {"content": [{"type": "text", "text": "Spawned 2 background agents using scout.\n- `ReadA` (job `ReadA`)\n- `ReadB` (job `ReadB`)\n\nResults auto-deliver; NEVER poll. …"}], "details": {"projectAgentsDir": null, "results": [], "totalDurationMs": 1, "progress": [{"index": 0, "id": "ReadA", "agent": "scout", "agentSource": "bundled", "modelRole": "smol", "status": "pending", "task": "…", "assignment": "…", "recentTools": [], "recentOutput": [], "toolCount": 0, "requests": 0, "tokens": 0, "cost": 0, "durationMs": 0}, {"index": 1, "id": "ReadB", …}], "async": {"state": "running", "jobId": "ReadA", "type": "task"}}}
```

A mid-run update **after** that `completed` (line 27, t=7500), row 0 complete:

```json
{"sessionUpdate": "tool_call_update", "toolCallId": "toolu_013x6rgBK7MEUsQq79DoL4YK", "status": "in_progress", "rawOutput": {"content": [{"type": "text", "text": "Running agent ReadA..."}], "details": {"projectAgentsDir": null, "results": [], "totalDurationMs": 2553, "progress": [{"index": 0, "id": "ReadA", "agent": "scout", "agentSource": "bundled", "modelRole": "smol", "status": "running", "task": "…", "assignment": "…", "recentTools": [{"tool": "read", "args": "a.txt:1-1", "endMs": 1791456834916}], "recentOutput": [], "toolCount": 1, "requests": 1, "tokens": 8962, "cost": 0.1149025, "durationMs": 0, "resolvedModel": "anthropic/claude-fable-5-1:high", "resolvedModelIdentity": "anthropic/claude-fable-5-1", "resolvedThinkingLevel": "high", "contextTokens": 8962, "contextWindow": 1000000, "lastIntent": "Reading first line"}, {"index": 1, "id": "ReadB", …}], "async": {"state": "running", "jobId": "ReadA", "type": "task"}}}, "content": [{"type": "content", "content": {"type": "text", "text": "Running agent ReadA..."}}]}
```

The last frame (line 44, t=9781), still `"status": "in_progress"`, row 0:

```json
{"index": 0, "id": "ReadA", "agent": "scout", "agentSource": "bundled", "modelRole": "smol", "status": "completed", "task": "…", "assignment": "…", "recentTools": [{"tool": "yield", "args": "", "endMs": 1791456837192}, {"tool": "read", "args": "a.txt:1-1", "endMs": 1791456834916}], "recentOutput": [], "toolCount": 2, "requests": 2, "tokens": 9118, "cost": 0.12128025, "durationMs": 4819, "resolvedModel": "anthropic/claude-fable-5-1:high", "resolvedModelIdentity": "anthropic/claude-fable-5-1", "resolvedThinkingLevel": "high", "contextTokens": 9037, "contextWindow": 1000000, "lastIntent": "Reading first line", "extractedToolData": {"yield": [{"data": {"first_line": "alpha line one"}, "status": "success"}]}}
… "async": {"state": "completed", "jobId": "ReadA", "type": "task"}
```

Parent usage (line 53) excludes the children (F23): `{"sessionUpdate": "usage_update", "size": 1000000, "used": 14445, "cost": {"amount": 0.13948325, "currency": "USD"}}`.

**Feed 1 — live rows** (`crates/acp/src/omp/progress.rs`). Recognition is structural: any `tool_call_update` (or `tool_call`) whose `rawOutput.details.progress` is an array of objects with string `id` and `status` is a task frame, regardless of `status` and regardless of whether the call already completed (F18). Keyed `(acp sessionId, toolCallId, progress.id)`; each frame is a full snapshot so the latest wins per id.

| row field | column / event |
|---|---|
| `id` | `acp_session` (= job id = transcript stem); `name`; nested `a.b` → `parent` = row with `acp_session == "a"` |
| `toolCallId` of the frame | `observed_tool_call` |
| `agent`, `agentSource`, `modelRole`, `index` | `observed_detail.{agent, agentSource, modelRole, index}` |
| `assignment` (else `task`) | `observed_description` (first 200 chars) |
| `status` pending\|running\|completed\|failed\|aborted | `observed_state` pending\|running\|completed\|failed\|cancelled |
| `resolvedModel` | `harness_model` |
| `tokens`, `cost`, `contextTokens`, `contextWindow`, `requests`, `toolCount`, `durationMs` | `observed_detail.{tokens, cost, contextTokens, contextWindow, requests, toolCount, durationMs}` |
| `lastIntent`, `currentTool`, `currentToolArgs`, `recentTools[≤5]`, `recentOutput[≤8]` | `observed_detail.{lastIntent, currentTool, currentToolArgs, recentTools, recentOutput}` (roster "current activity"/"tools") |
| `retryState{attempt, maxAttempts, delayMs, errorMessage}` / `retryFailure` | `observed_detail.retry`; `retryFailure.errorMessage` → `observed_stop` |
| `extractedToolData.yield[0].data` | `observed_detail.result` |
| `details.async` present | `cost_rule = counted` (background: excluded from the parent's `usage_update`, F23) |
| `details.async` absent and `details.usage` present | `cost_rule = inside_parent` (sync: folded into the parent) |
| `details.async.state` | parent-side roster footer ("2 running" / "all done") |
| the `completed` frame's rows | ignored when a later snapshot exists for the id (stale, F18); applied otherwise |

**Feed 2 — tail** (`crates/acp/src/omp/tail.rs`). Locate: `glob("~/.omp/agent/sessions/*/*_<acpSessionId>.jsonl")` → parent file; strip `.jsonl` → artifacts dir; child file `<dir>/<id>.jsonl`, nested `<dir>/<a>/<a>.<b>.jsonl` (F21). On this machine `~` is the engine's home; the harness must be a local `Transport::Process` (a dialled remote theuth has no omp files here → feed 2 off, feed 1 only). Entries (`ReadA.jsonl`, re-read):

| jsonl entry | transcript event on the child |
|---|---|
| `session {id, cwd, parentSession}` | `observed_detail.ompSession = id`, `observed_detail.cwd`; `parentSession` basename must match the located parent (sanity) |
| `model_change {model}` / `thinking_level_change` | `harness_model` if `resolvedModel` unknown |
| `message role:user content[].text` | `TurnStarted{text}` for the child's first turn (the rendered assignment); later user entries → `MessageDelivered`-style info line |
| `message role:assistant content[]` `{type:"text"}` | `TextDelta` |
| … `{type:"toolCall", id, name, arguments, intent}` | `ToolCallStarted{call_id: id, name, args: arguments}` (the `custom tool_execution_start` that follows carries the same id; whichever comes first wins, the other is the dedupe no-op via `Turn::call_started` idempotency) |
| `message role:assistant usage{input, output, cacheRead, cacheWrite, totalTokens, cost{total}}`, `model`, `provider` | `observed_detail.turns[] {totalTokens, cost}` and `record_remote_turn(child, …)` tokens (money stays with the cost rule, §7.6) |
| `message role:toolResult {toolCallId, toolName, content[].text, isError}` | `ToolCallFinished{call_id, output, is_error}` |
| `custom tool_execution_start` | `ToolCallStarted` if not yet seen |
| `title`, `session_init`, other | ignored |
| `<id>.md` appears | `observed_state = completed` if not terminal; `.md` text → final assistant message if the jsonl had no `yield` |
| `<id>.json` appears | `observed_detail.result` |
| parent jsonl `custom_message{customType:"async-result", details.jobs[].jobId}` | `observed_state = completed` for that job if still non-terminal (parent entry 15 in the capture) |

The tail is polled every 500 ms while any omp child of the parent is non-terminal and for 10 s after the last change (the `.md` mtime equals completion, R1 §4); the byte cursor is persisted to `observed_cursor` after each applied batch; a partial trailing line is left unread. Why required (D7): omp drops every frame after the parent's turn ends (F20); the capture's parent happened to `wait`, but a parent that answers first leaves every child frozen in theuth without this feed.

`observed_source = omp_task`. No per-child stop (F20).

## 7. Engine

### 7.1 acp crate: normalized types (contract, `crates/acp/src/harness.rs`, re-exported from `crates/acp/src/lib.rs:19-22`)

```rust
pub enum Update {
    /// A `session/update`. `session` is the wire `params.sessionId`: the
    /// harness's own id for the driven session, a child's external id for
    /// RFD child traffic or an opencode `child_update{type:"update"}`.
    Session { session: String, update: SessionUpdate },
    /// A subagent announce/upsert from any source (§6).
    Subagent(ChildAnnounce),
    /// A `session/request_permission`; `session` as above (child asks carry the child id).
    Permission(Permission),            // Permission gains `pub session: String`
    Peer(PeerMessage),
    Closed(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildAnnounce {
    pub source: ChildSource,               // Rfd | OpencodeExt | OpencodeTask | OmpTask
    pub child: String,                     // external id
    pub parent: Option<String>,            // external id of the parent; None = the driven session
    pub title: Option<String>,             // None = unchanged
    pub description: Option<String>,
    pub model: Option<String>,
    pub state: Option<ChildState>,         // None = unchanged
    pub can_cancel: Option<bool>,          // None = unchanged
    pub tool_call: Option<String>,         // spawning tool call id
    pub cost_rule: Option<CostRule>,       // None = unchanged
    pub detail: serde_json::Map<String, serde_json::Value>,   // merged into observed_detail (shallow)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    Pending, Running, RequiresAction,
    Idle { stop_reason: Option<String>, usage: Option<agent_client_protocol::schema::v1::Usage> },
    Completed, Failed { error: Option<String> }, Cancelled, Unknown,
}

impl Harness {
    /// `session/cancel` for a child session the harness announced with `capabilities.cancel` (RFD).
    pub fn cancel_session(&self, child: &str) -> Result<(), String>;   // CancelNotification::new(SessionId(child.into()))
}
```

`Link::spawn` (`harness.rs:364-413`) changes:
- typed `SessionNotification` handler (`:366-379`): keep the `ConfigOptionUpdate` option-list side effect; then `match n.update { SessionUpdate::SubagentUpdate(u) => Update::Subagent(rfd::announce(&n.session_id, u)), SessionUpdate::SessionMessage(_) | SessionUpdate::SessionMessageChunk(_) | other => Update::Session { session: n.session_id.0.to_string(), update: other } }` — `session_message*` stay `Update::Session` on the session they were sent to; acp_child renders them (§6.1).
- untyped handler (`:383-393`): `PeerMessage::parse` first; else `opencode::child_update(&n) -> Vec<Update>` (status → `Update::Subagent`, update → `Update::Subagent` (implicit announce, only for a first-seen child; the adapter keeps a per-link `HashSet<String>` of announced ids) + `Update::Session{session: childSessionId, update}`); else drop.
- permission handler (`:394-404`): `Permission{ session: r.session_id.0.to_string(), tool_call, options, responder }`.
- `initialize_request()` (`:962-974`): `.subagents(SubagentCapabilities::new())` next to the existing `.meta(..)`.
- `cancel_session` next to `cancel` (`:746`).
- The omp task frames are **not** split in the transport (they are ordinary `tool_call_update`s the parent transcript must still see); the acp crate exposes the adapter and acp_child calls it (§7.3) — one frame yields the parent's tool event **and** N announces.

Modules (W1 unless noted): `crates/acp/src/rfd.rs` (`announce(parent: &SessionId, u: SubagentUpdate) -> ChildAnnounce`, `MaybeUndefined` → `Option` with `Null` → explicit clear values), `crates/acp/src/opencode.rs` (`child_update`, `task_v1` for 1.x frames: `is_task_call`, `announce_from_update`), `crates/acp/src/omp/{mod,progress,tail}.rs` (W5; trait signatures stubbed by C):

```rust
// crates/acp/src/omp/mod.rs (C stubs the signatures; W5 implements)
pub fn progress_rows(update: &SessionUpdate) -> Option<TaskFrame>;      // None = not a task frame
pub struct TaskFrame { pub tool_call_id: String, pub background: bool, pub batch_state: Option<String>, pub rows: Vec<Progress> }
#[derive(Deserialize)] pub struct Progress { pub id: String, pub status: String, pub agent: Option<String>, /* F19 fields, all Option */ … }
impl Progress { pub fn announce(&self, frame: &TaskFrame) -> ChildAnnounce; }   // nested id → parent

pub struct Tailer { /* home, parent file, artifacts dir */ }
impl Tailer {
    pub fn locate(home: &Path, acp_session: &str) -> Option<Tailer>;           // glob */*_<sid>.jsonl
    pub fn child_path(&self, id: &str) -> PathBuf;                             // a.b → <dir>/a/a.b.jsonl
    pub fn read(&self, id: &str, cursor: u64) -> io::Result<(Vec<Entry>, u64)>; // whole lines only
    pub fn sidecar_done(&self, id: &str) -> bool;                              // <id>.md exists
    pub fn settled_jobs(&self, parent_cursor: u64) -> io::Result<(Vec<String>, u64)>;  // async-result jobIds
}
pub enum Entry { Session{..}, Model(String), User(String), Assistant{ text: String, calls: Vec<Call>, usage: Option<Usage>, model: Option<String> }, ToolStart(Call), ToolResult{ call_id, name, text, is_error }, Other }
pub fn to_updates(entries: &[Entry]) -> Vec<SessionUpdate>;   // user → nothing (TurnStarted handled by acp_child), text → agent_message_chunk, call → tool_call, result → tool_call_update completed/failed
```

`to_updates` returns SDK `SessionUpdate`s so acp_child's `translate()` (`acp_child.rs:1372`) is reused verbatim (D3).

### 7.2 acp_child ingestion (`crates/agent/src/acp_child.rs` + new `acp_child/observed.rs`, W2)

State on `AcpChild`: `observed: Observed` =

```rust
pub(crate) struct Observed {
    by_external: HashMap<String, ObservedChild>,     // external id → row
    pending_traffic: HashMap<String, (Instant, Vec<SessionUpdate>)>,  // RFD traffic before its announce (2 s)
    omp: Option<OmpFeeds>,                           // tailer + per-child cursors; None when not omp/local
}
struct ObservedChild { id: SessionId, name: String, model: Option<String>, meta: ObservedMeta, turn: Option<Turn>, dirty: Option<tokio::task::JoinHandle<()>>, last_emit: Instant }
```

Dispatch (`impl Message<Update>`, `acp_child.rs:1751-1812`), new arms **before** the generic one (`:1781`):

1. `Update::Subagent(a)` → `self.observed.upsert(a)`: resolve `parent` (`a.parent` None → `self.id`; Some(ext) → `by_external[ext].id`, else `self.id`); resolve/create the row (`Store::upsert_observed`, §11.2) → `(SessionId, created)`; merge fields; state transition → open/close the child `Turn` (`continue_turn`-like: `TurnStarted{session: child, model: child model or harness model, turn_seq: 0}`; `TurnFinished{stop_reason}` on leaving running/requires_action); publish `ObservedChildChanged` (immediately on `created` or any state/title/cancel change; detail-only changes coalesced ≥ 250 ms per child via the `dirty` timer, same pattern as the `Quiet` timer at `:1797`). `TitleChanged`/`HarnessModelChanged` on the child when those fields change.
2. `Update::Session{session, update}` with `session != h.session_id()` → `self.observed.traffic(session, update)`: known child → `translate(update, &mut child.turn)` (opening a turn if none; child state to `running` if it was pending/idle/unknown — traffic proves activity), persist `take(turn.messages)` via `Store::append(child.id, …)`, publish events on the child id. Unknown child → buffer 2 s (RFD announce-before-traffic may be violated by adapters), then warn and drop. `session_message*` → the directed line (§6.1). The parent's **replay gate** applies to child traffic too: before the first prompt on this connection (`!self.worked`) child traffic is `session/load` replay and is dropped; announces always pass (idempotent).
3. `Update::Session{session, update}` with `session == own` → as today **plus**, before the generic arm, `if let Some(frame) = acp::omp::progress_rows(&update) && self.is_omp()` → `for row in frame.rows { self.observed.upsert(row.announce(&frame)) }` and start the omp tail for new ids; then fall through so the parent's tool card still gets its `ToolCallStarted/Finished` (the roster card replaces it in the UI, not in the store). Also `opencode::task_v1` for opencode <2 frames → announces (§6.3).
4. `Update::Permission(p)`: `p.session != own` → the RFD child; else parse `p.tool_call.tool_call_id` for a `<child>:<id>` prefix matching a known child (§6.2) → `ExternalApproval{session: child.id, …}` and `observed_state = requires_action` until the responder fires (then back to `running`). Otherwise as today (`:1242-1254`).
5. `Connected` (`:1556-1599`): seed `observed` from `Store::observed_children(self.id)`; if the harness is omp (`agent_info.name ∈ {"oh-my-pi", "omp"}`) and `Transport::Process`, build `OmpFeeds` with `Tailer::locate(home, h.session_id())` (may be `None` until the first task frame; retried then).
6. `finish` (`:983`) / `closed` (`:1165`): leave states alone (children outlive the parent's turn: opencode background, omp async, RFD); on `closed`, children that are non-terminal and have no omp tail become `unknown` ("lost contact"); omp children keep tailing for 10 s after the last change, then `unknown` if still non-terminal.

`is_omp()`/`is_opencode_v1()` read `Harness::agent_info()` (already used by `usage_is_lower_bound`, `harness.rs:786`).

### 7.3 omp feeds inside the actor

`OmpFeeds { tailer: Option<Tailer>, cursors: HashMap<String, u64>, parent_cursor: u64, poll: Option<JoinHandle<()>> }`. The poll task sends `OmpTick` to the actor every 500 ms while any omp child is non-terminal (and 10 s past the last change). `OmpTick` handler: for each non-terminal child, `tailer.read(id, cursor)` → `to_updates` → the same path as traffic (2.) → `set_observed_cursor`; `sidecar_done(id)` → `Completed`; `settled_jobs(parent_cursor)` → `Completed` for named jobs. Byte cursors are persisted after each applied batch; a restart resumes from `observed_cursor` (D11). Transcript entries already applied are never re-translated because the cursor only moves forward; a truncated/rotated file (cursor > len) resets to 0 and relies on `Turn::call_started`/`call_finished` idempotency plus `Store::append` dedupe by `(session, call_id)` for tool rows — text rows could duplicate in that pathological case; accepted (§14 Q6).

### 7.4 Persistence

- Row create/update: `Store::upsert_observed` (§11.2), one write transaction, returns `(id, created)`.
- Transcript: `Store::append(child_id, ChatMessage)` (`store.rs` `append`, as the parent at `acp_child.rs:968-971`) → `messages` rows → `load_session` history for free.
- Tokens: omp tail assistant entries → `record_remote_turn(child, …)` (`store/turn_usage.rs:96`) so `RemoteTokens`/`HarnessTokens` show the child's own counts; opencode: none; RFD idle.usage → the same with `turn_id` = the child turn.
- Cost: §7.6.
- `last_stop`/`finished_at`: not written through `turn_steps` (would wake durability); `set_observed_state` writes `updated_at` and the UI derives "finished" from `observed_state`. `Agents::turn_finished` still runs for the published `TurnFinished` (in-memory `running`/`last_stop`, search invalidation).

### 7.5 Events

`AgentEvent::ObservedChildChanged { parent, child, name, model, observed }` (`lib.rs`, after `SubagentSpawned` `:2480`); `AgentEvent::session()` (`:2817`) returns `child`. Transcript events on the child id are the existing variants. Publication order per observation: `ObservedChildChanged` (row exists) → `TurnStarted` (if opened) → translated events → `TurnFinished` (if closed). Throttle: detail-only (`observed_detail`, `model` unchanged, state unchanged) changes emit at most every 250 ms per child; state/title/cancel/create changes emit immediately and reset the timer.

### 7.6 Cost rule (D8)

| source / mode | `cost_rule` | engine action | UI |
|---|---|---|---|
| omp background (`details.async` present) | `counted` | `apply_meter(child, MeterUpdate{ origin: harness name, external_session: "<acp sid>/<progress.id>", cumulative: Money(cost), confirmed_new_session: true, report_id: None })` on every row change (cumulative → high-water); meter rows carry `CostScope::Observed = "observed"` (new variant in `crates/agent/src/cost.rs`, C) so `session_cost().children[]` names them; rolls up through `cost_owners` into the ACP child's and root's `descendants`/`total` and into budgets | child cost in tree/roster/pill; Breakdown row "counted (omp background: not in the harness's own total)" |
| omp sync (`details.usage` present, no `async`) | `inside_parent` | no meter rows; `observed_detail.cost` only | child cost shown greyed; Breakdown "inside parent (omp sync)" |
| opencode 2.x / 1.x | `unreported` | nothing | "—"; Breakdown "not reported by opencode" |
| RFD `idle.usage` | `display_only` | `record_remote_turn` tokens; no money | tokens shown; Breakdown "reported by the harness, not counted" |

Never both: the `counted` branch is the only one that writes money, and it is chosen exactly when the parent's `usage_update` provably excludes the child (F23). `budget_spent` sees `counted` rows through the owner tree as it does for any child.

### 7.7 Permissions and cancel

- Permission attribution: §7.2 item 4. The approval card is keyed to the child row, so `state_of(child)` = `Approval{tool}` and the parent's existing "subagent X is waiting on you" prompt (desktop `PromptCard`, R2B §5d) appears because the child's `AgentInfo.state` is `approval`.
- Cancel: `Agents::Cancel` (`supervisor.rs:3171-3209`): for an id with no live actor whose row is observed → `self.sessions.get(&row.parent)` live `LiveActor::Acp(a)` → `a.tell(CancelObserved{ child: row.id })`; `AcpChild` → if `meta.can_cancel` → `Harness::cancel_session(external)`; else publish `AgentEvent::Error{session: child, turn: None, message: "<harness> cannot stop this subagent; stop <parent name>"}`. `CancelScope::Tree` on the parent includes observed descendants the same way (RFD cancellable ones get `session/cancel`; others are left to the harness's own cascade: opencode foreground children interrupt, F15; omp background jobs keep running, F20).

### 7.8 Restart and replay (D11)

- Upsert identity (§5.3) makes every announce idempotent; `session/load` replay of `subagent_update` (RFD) updates in place.
- On engine start (`Agents` init, W2): `Store::mark_observed_lost(live_parents)` → non-terminal observed rows whose parent is not a live ACP child become `unknown`/`"lost contact"`; `ObservedChildChanged` is published for each so open UIs update.
- On the parent's `Connected`: seed from the store; RFD harnesses re-announce (RFD: child events arrive on the same connection without load/resume); opencode 2.x re-streams only children still open (R1B §1 "Lifetime"); omp: the tail resumes from `observed_cursor` and the sidecar/async-result checks settle children that finished while theuth was down.
- `respawn_acp_children` (`acp_resume.rs:27`) ignores observed rows (no `agent`, no wait row).

## 8. RPC and events

### 8.1 Contract types (`crates/agent/src/lib.rs`, C)

| type | change |
|---|---|
| `SessionKind` (`:2009`) | `+ Observed` (`"observed"`) |
| `ObservedSource`, `ObservedState`, `CostRule`, `ObservedMeta` | new (§5.1), `snake_case`, ts-exported |
| `AgentEvent` (`:2327`) | `+ ObservedChildChanged { parent: SessionId, child: SessionId, name: String, model: Option<String>, observed: ObservedMeta }`; `session()` → `child` |
| `AgentInfo` (`:1172`) | `+ #[serde(default)] observed: Option<ObservedMeta>` (`ts(optional = nullable)`) |
| `SessionSummary` (`:2020`) | `+ #[serde(default)] observed: Option<ObservedMeta>` |
| `Loaded` (`:996`) | `+ #[serde(default)] kind: SessionKind`, `+ observed: Option<ObservedMeta>` — so the session view hides the composer without a second RPC |
| `CostScope` (`cost.rs:210`) | `+ Observed = "observed"` |

### 8.2 RPC surface (`crates/rpc/src/api.rs`)

No new methods. Changed payloads: `list_agents` (`:490`), `list_sessions` (`:486`), `load_session` (`:497`) carry the fields above; `cancel(session, scope)` (`:256`) accepts an observed id (routed per §7.7); `session_cost` children rows may carry `scope: "observed"`. `subscribe_events`: `admitted` (`:6935-6950`) and `wanted` (`:6964-6986`) get the twin arm — `type == "ObservedChildChanged" && parent ∈ tree → tree.insert(child)` / private enrolment — so a `descendants` subscription on the root receives the child's transcript events, exactly as for `SubagentSpawned`. `is_delta` unchanged (the child's `TextDelta`s are deltas and obey focus, 129).

Refusals: `send_input`/`steer`/`undo`/`redo`/`compact`/`resume_child` on an observed id surface the §5.4 error text; `send_message{to: <observed name>}` → "watch-only harness subagent — message its parent"; `discover_peers` lists it `send: false`.

### 8.3 Bindings

`cargo test -p rpc --features ts export_bindings` + `just ts` / `ts-desktop` regenerate `crates/agent/bindings/*.ts` and `crates/desktop/ui/src/rpc/bindings/*.ts` (`SessionKind.ts`, `AgentEvent.ts`, `SessionSummary.ts`, `Loaded.ts`, new `ObservedMeta.ts`, `ObservedSource.ts`, `ObservedState.ts`, `CostRule.ts`). The hand-written `crates/desktop/ui/src/rpc/extra.ts` `AgentInfo` (`:95-121`) gains `observed?: ObservedMeta | null` (import from bindings) and the doc line for `kind` gains `observed`. `api.test.ts` METHODS pin is unchanged (no new method).

### 8.4 Relay (`crates/rpc/src/acp.rs`)

`ObservedChildChanged` and the child's events fall to `_ => {}` (`:787`) — a remote theuth's observed children stay invisible to its parent engine (D12 follow-up: emit RFD `subagent_update` there).

## 9. Desktop UI/UX

Stack and files per R2B §5 (re-verified names in F35). All strings use "Watching"/"watch-only"; never "observed" in the UI.

### 9.1 Data plumbing (`src/store/*`, `src/lib/*`)

- `store/reduce.ts` `route()`: `ObservedChildChanged` → upsert the child slice `{ name, parent, kind: 'observed', observed, harnessModel: model }` and a `children[child]` `AgentInfo` row on **every ancestor** (walk `slice.parent`, as `SubagentSpawned` does at `:1308-1345`) with `state` derived from `observed.state` (table §5.2), `agent: null`, `model`, `observed`; **no** `↳ spawned` interjection (the roster card is the parent-transcript artefact). `trackRow` keeps beating on the child's `TextDelta`/`ToolCall*` (design 93 `beat`).
- `store/sessions.ts`: `SessionSlice` gains `kind?: SessionKind`, `observed?: ObservedMeta | null`; `loadHistory` copies `Loaded.kind`/`observed`; `setChildren` keeps observed rows (only `advisor` is dropped, `:550-557`).
- `lib/rpc.ts:183`: `on('ObservedChildChanged', sessions)` invalidates `list_sessions` like `SubagentSpawned`.
- `lib/session-tree.ts`: `live()` counts `observed.state ∈ {running, requires_action}`; `stripRows` includes observed rows; a new `sourceTag(observed)` → `'omp task' | 'opencode' | 'ACP'`.

### 9.2 Components

| surface | file | change |
|---|---|---|
| (a) Sidebar tree row | `components/sidebar/AgentTree.tsx` `SubagentRow` (`:36`) | when `row.observed`: leading `Eye` icon (lucide) with `aria-label="watch-only"` before the state glyph; source tag chip after the name; state dot colour by `observed.state` (`running` success, `requires_action` warning, `failed` destructive, `unknown` muted-dashed, terminal → check), blink keyed on `row.beat` (93); cost from `session_cost().children` as today (`counted` rows roll up; others show `—` or greyed `observed.detail.cost`) |
| (a) Agents page rows | `components/agents/AgentSessionRows.tsx` | reuses `SubagentRow` → inherits; mobile 44 px rows unchanged |
| (b) Roster card | new `components/chat/RosterCard.tsx`; dispatch in `components/chat/MessageTool.tsx` (`MessageTool` :165-188) | a tool row whose `call_id` matches any `children[*].observed.tool_call` renders `RosterCard` instead of `PlainTool`: header = tool title + batch footer ("2 running · 1 done"), then one row per child sorted by `observed.detail.index ?? name`: state dot/check · `name` · `observed.detail.agent ?? observed.description` · current activity (`detail.lastIntent ?? detail.currentTool ?? last`) · tools (`detail.toolCount`) · tokens (`detail.tokens ?? detail.contextTokens`) · cost (per cost rule) · chevron `Link to="/c/$session"`. Rows beat with the child. Empty state (call known, no children yet): the plain card body |
| (c) Strip | `components/chat/SubagentStrip.tsx` `SubagentDot` (`:45`) | observed rows included by `stripRows`; dot title `${name} · watching · ${state}`; same colours as (a) |
| (d) Session view | `routes/c.$session.tsx` `LiveSession` (`:78-151`) | `const watching = slice.kind === 'observed'`; render `Composer` (`:148`) only when `!watching`; render `<ObservedBar session={session} />` in its place |
| (d) Header | `components/chat/Header.tsx` | when watching: "Watching" chip (eye icon) + model (`harnessModel ?? '—'`) + tokens/cost pill (`CostPill` with the child's `session_cost`) + crumb `inside <parent title/name> · <source tag>` linking to the parent (reuses the `Parent` breadcrumb `↳` component) |
| (d) ObservedBar | new `components/chat/ObservedBar.tsx` | `role="status"`; left: state label ("running", "waiting on you", "finished", "failed", "stopped", "lost contact", "pending"); middle: `"<harness> runs this subagent, so you can watch it but not message it."` (+ for `opencode_task`: "opencode 1.x does not stream subagent activity"); right: **Open parent** (Link) and **Stop subagent** (`api.cancel(session, 'root')`), the latter rendered only when `observed.can_cancel`, otherwise a disabled button with `title`/`aria-disabled` "this harness cannot stop one subagent — stop the parent" |
| (d) Model menu | `components/chat/ModelMenu.tsx` `useModel` (`:125-216`, `acp` at `:136`) | `if (slice.kind === 'observed')` → menu disabled, label `harnessModel ?? '—'`, no `harness_options`/`resume_child` calls |
| (h) Cost | `components/chat/CostPill.tsx` `Breakdown` (`:93-130`) | children rows with `scope === 'observed'` get the eye icon; a one-line rule note from `observed.cost_rule` (§7.6 UI column) |
| Search hit | `components/chat/SearchSession.tsx` | unchanged (already composer-less); opening the live view from a hit lands in (d) |

### 9.3 States, empty and edge cases

- Child announced before its spawn tool call is known (opencode 2.x `created` precedes the parent's `completed` metadata, F14): the child shows in tree/strip immediately; the roster card attaches when `observed.tool_call` arrives (slice update re-renders the card).
- omp: the spawn call's own `ToolCallFinished` arrives 5 ms after start (F18) while children keep going — the roster card ignores the tool's `status` and uses the children's states; footer from `observed.detail.batch` if present.
- `unknown`: dashed dot, label "lost contact", ObservedBar explains "theuth lost the harness; the last known state is shown".
- Deleted parent: cascade deletes observed rows (`DeleteSession` uses `descendants`); open child view navigates away on `SessionDeleted` as today.
- Private sessions: observed rows under a private tree inherit private filtering through the `admitted` twin arm (§8.2).
- Watch-only child of a watch-only child (depth 2): same components; crumb shows the immediate parent.
- Keyboard: ObservedBar buttons are tabbable; Esc in the watch-only view does nothing (no composer to cancel). Screen readers: eye icon `aria-label`, dots get `sr-only` state text, roster rows are `<a>` with full text.
- Mobile (<768 px): roster rows wrap to two lines (name+state / activity+numbers); ObservedBar sticks where the composer was; Header crumb collapses to the source tag.

### 9.4 Tests (vitest, `.ts` only — SSR via `renderToStaticMarkup`)

- `lib/session-tree.test.ts`: observed rows are in `subtrees`/`stripRows`; `live()` counts `running`/`requires_action` only; `sourceTag` mapping.
- `store/reduce.test.ts`: `ObservedChildChanged` creates the child slice + ancestor `children` rows without an interjection; a second event updates state in place (no duplicate row); the child's `TextDelta` beats its row (93).
- `components/chat/RosterCard.test.ts`: rows per child, cost cell per `cost_rule` (`counted` → money, `inside_parent` → greyed, `unreported` → `—`), chevron links.
- `components/chat/ObservedBar.test.ts`: Stop rendered enabled only when `can_cancel`; message text per source.
- `components/sidebar/SessionList.test.ts`: `runningSubagents` counts a running observed child.

## 10. TUI, Slack, Telegram

### 10.1 TUI (`crates/tui`, W4)

- `model.rs` roster match (`:5921-6135`): new arm `ObservedChildChanged{parent, child, name, model, observed}` → insert/update `AgentRow` (`depth` = parent.depth+1 like `:5957-5961`; `agent: None`; `model` from the event; new `AgentRow.observed: Option<ObservedMeta>`); no `↳ spawned` interjection.
- Transcript match (`:6152-6486`, exhaustive): real arm — when `self.session == Some(child)` and the state changed, interject `Entry::Info("watching · <state>")`; otherwise nothing (the board shows it).
- `track()` (`:6492-6557`): child `TurnStarted/ToolCallStarted/TurnFinished` already drive the row; `ObservedChildChanged` with a terminal/unknown state sets the row's state label directly.
- `agent_row()` (`:11686`): glyph `◎` for observed rows, detail `watch · <src>` (`omp task` / `opencode` / `ACP`) then `observed.detail.lastIntent` when present; `/agents` picker shows the same.
- Read-only open: `resume(id)` (`:6935`) on an observed row loads it as today; `input_focused()` (`:3931-3942`) returns false when the shown session is observed; the status line shows "watch-only — <harness> runs this subagent · Esc stops it" or "· cannot be stopped here"; typing is swallowed with that hint. Header (`:7334-7341`): `◎ <name>  watching · <model>`.
- Esc (`:6963-6973`): on an observed session → `Command::Cancel` only when `observed.can_cancel`, else the hint; triple-Esc `CancelTree` on the parent unchanged.
- Tests: `board_tracks_child_events_and_interjects` sibling for `ObservedChildChanged`; `header_shows_harness_and_its_model_for_an_acp_child` sibling for the watching header; an `input_focused` test for the observed gate.

### 10.2 Slack (`crates/slack`, W4)

`route()` (`channel.rs:1551-1583`): `ObservedChildChanged` → `board.apply_observed(..)` (a `Row` with the `◎` glyph, name, `watch · <src>`, state) — **no** `spawned()` → no `Thread::new(child)`, no reply binding, no Cancel button on that row. `Thread::apply` (`:2478-2682`): `ObservedChildChanged { .. }` joins the "nothing to say in a thread" arm (`:2642-2674`); the child's `TextDelta`/tool events never reach a thread because no thread is bound to the child id. Board row for a terminal child lingers as other rows do.

### 10.3 Telegram (`crates/telegram`, W4)

`ChildState::apply` (`channel.rs:196-231`) gains the observed row (glyph/label as Slack); `spawned()` (`:2473-2535`) is **not** called for observed children (no topic, no flat marker); `commands.rs` cancel-by-name resolves an observed name → "watch-only subagent of <parent>; stop the parent" unless `can_cancel`, in which case `api.cancel(child)`.

## 11. Store and migration

### 11.1 `crates/agent/migrations/0041_observed_children.sql` (verbatim)

```sql
-- 141 ACP SUBAGENT VISIBILITY: watch-only children a harness runs inside
-- itself. Rows have kind = 'observed', parent = the ACP child (or an observed
-- parent when nested), NO agent (nothing here drives them), and
-- acp_session = the harness's own id for the child.
ALTER TABLE sessions ADD COLUMN observed_source TEXT NULL;        -- rfd | opencode_ext | opencode_task | omp_task
ALTER TABLE sessions ADD COLUMN observed_state TEXT NULL;         -- pending | running | requires_action | idle | completed | failed | cancelled | unknown
ALTER TABLE sessions ADD COLUMN observed_stop TEXT NULL;          -- stop reason / error text
ALTER TABLE sessions ADD COLUMN observed_cancel INTEGER NOT NULL DEFAULT 0;   -- the harness accepts session/cancel for this child
ALTER TABLE sessions ADD COLUMN observed_description TEXT NULL;
ALTER TABLE sessions ADD COLUMN observed_tool_call TEXT NULL;     -- the spawning tool call id (roster card), when known
ALTER TABLE sessions ADD COLUMN observed_detail TEXT NULL;        -- JSON: source-specific live fields (141 §6)
ALTER TABLE sessions ADD COLUMN observed_cursor INTEGER NULL;     -- omp: byte offset read so far in the child's transcript
-- descendants_incl and the observed lookups filter on parent; nothing indexed it before.
CREATE INDEX IF NOT EXISTS sessions_parent ON sessions(parent);
-- Identity of a watch-only child: one row per (parent, source, external id).
CREATE UNIQUE INDEX IF NOT EXISTS sessions_observed_identity
    ON sessions(parent, observed_source, acp_session)
    WHERE observed_source IS NOT NULL;
```

Ordering: 0040 is on this branch (139-R2 merged); sqlx checks order/checksums, so this file is 0041 and never renumbered after it lands.

### 11.2 Store API (`crates/agent/src/store.rs`, C; `store/advisor.rs` `kind_str`/`kind_of` + `Observed`)

```rust
pub struct ObservedUpsert<'a> { pub parent: SessionId, pub source: ObservedSource, pub external: &'a str,
    pub name: Option<&'a str>, pub model: Option<&'a str>, pub state: Option<ObservedState>, pub stop: Option<Option<&'a str>>,
    pub can_cancel: Option<bool>, pub description: Option<&'a str>, pub tool_call: Option<&'a str>,
    pub cost_rule: Option<CostRule>, pub detail: Option<&'a serde_json::Map<String, serde_json::Value>> }   // None = unchanged
impl Store {
    pub async fn upsert_observed(&self, u: ObservedUpsert<'_>) -> anyhow::Result<(SessionId, bool /*created*/)>;
    //   SELECT id FROM sessions WHERE parent=? AND observed_source=? AND acp_session=?  → UPDATE … ; else
    //   create_session_kind(id, model or parent.harness_model or parent.model, Some(parent), name, None, SessionKind::Observed)
    //   + UPDATE sessions SET acp_session=?, observed_* … ; `detail` merges shallowly into the stored JSON.
    pub async fn observed_children(&self, parent: SessionId) -> anyhow::Result<Vec<(SessionSummary, ObservedMeta)>>;
    pub async fn observed_child(&self, parent: SessionId, source: ObservedSource, external: &str) -> anyhow::Result<Option<SessionId>>;
    pub async fn set_observed_cursor(&self, id: SessionId, cursor: u64) -> anyhow::Result<()>;
    pub async fn mark_observed_lost(&self, live_parents: &[SessionId]) -> anyhow::Result<Vec<SessionId>>;
    //   UPDATE sessions SET observed_state='unknown', observed_stop='lost contact', updated_at=?
    //   WHERE observed_source IS NOT NULL AND observed_state IN ('pending','running','requires_action','idle') AND parent NOT IN (…)
}
```

`SessionRow` (`:381-403`) and `session_cols!` (`:426-438`) gain `acp_session` and the eight `observed_*` columns; `SessionRow::into_summary` builds `observed: Option<ObservedMeta>` when `observed_source` is set. `kind_str`/`kind_of` (`store/advisor.rs:18-30`) add `"observed"`. `list_sessions_incl`'s `kind != 'advisor'` filter (`:2377`) leaves observed rows visible; `descendants_incl` (`:2403`) returns them in pre-order under the ACP child.

### 11.3 Store tests (C)

- `observed_upsert_is_idempotent_per_parent_source_external`: two upserts → one row, state updated, `created` false the second time; a different source with the same external id → a second row.
- `observed_children_lists_under_the_acp_child_in_order` and `descendants_is_preorder_with_depth` sibling with an observed grandchild.
- `kind_of_observed_round_trips` (`"observed"` ↔ `SessionKind::Observed`; unknown still `Chat`).
- `mark_observed_lost_touches_non_terminal_rows_only`.
- `observed_cursor_round_trip`.
- Migration check: `open_memory_is_migrated_and_disk_free` keeps passing; `sqlite_master` has `sessions_parent` and `sessions_observed_identity`.

## 12. Waves

Contract first (C lands alone, workspace compiles with the new types and the 3.1.0 SDK), then W1–W5 in parallel on **disjoint file sets**, then the Closer, then Z. Each wave writes its hand-off to `docs/design/141-handoff-<wave>.md` (what landed, what it assumed from other waves, gate output); the Closer folds them into `141-as-built.md` and removes them. Gates are crate-level commands; machine-specific build rules live in the wave briefs, not here.

### C — CONTRACT (sequential, first)

| owns (files) | delivers | gate |
|---|---|---|
| `Cargo.toml` (`:267`), `Cargo.lock`; `crates/agent/src/lib.rs` (§8.1 types); `crates/agent/src/cost.rs` (`CostScope::Observed`); `crates/agent/migrations/0041_observed_children.sql`; `crates/agent/src/store.rs` + `crates/agent/src/store/advisor.rs` (§11.2 API + §11.3 tests); `crates/acp/src/lib.rs` + the **type contract only** in `crates/acp/src/harness.rs` (`Update::Session{session, update}`, `Update::Subagent(ChildAnnounce)`, `ChildAnnounce`, `ChildState`, `ChildSource`, `Permission.session`, `Harness::cancel_session` signature with a working body — it is one notification) and `crates/acp/src/omp/mod.rs` **stub signatures** (`progress_rows` → `None`, `Tailer::locate` → `None`, `to_updates` → empty); compile-only stub arms `=> {}` at `crates/tui/src/model.rs:6152` match and `crates/slack/src/channel.rs:2478` match (+ `crates/telegram` only if a match turns exhaustive); `crates/rpc/src/api.rs` `admitted`/`wanted` twin arms (`:6935`, `:6964`) + test sibling of `:7332`; bindings regen (`crates/agent/bindings/*.ts`, `crates/desktop/ui/src/rpc/bindings/*.ts`) + `crates/desktop/ui/src/rpc/extra.ts` | SDK 3.1.0 pinned (§4.1 lock checks); every new type/variant exists; existing `acp_child.rs` call sites adjusted mechanically to `Update::Session{..}` (payload unchanged); store API + migration + tests; workspace compiles; **no behaviour change** beyond compile | `cargo check --workspace --all-targets`; `cargo test -p agent --lib -- store::`; `cargo test -p acp`; `cargo test -p rpc --lib -- api::`; `cargo test -p rpc --features ts export_bindings`; desktop `tsc --noEmit` |

Hand-off: the exact type definitions as landed (copy of §7.1/§8.1 with any renames), the migration number confirmed, the list of stub sites W1/W4/W5 replace.

### W1 — TRANSPORT

| owns (files) | delivers | gate |
|---|---|---|
| `crates/acp/src/**` **except** `crates/acp/src/omp/**` and `lib.rs` (harness.rs behaviour, new `rfd.rs`, `opencode.rs`, `process.rs`); `crates/rpc/src/acp.rs` SDK-compat edits only (§4.4 `connect_to`); `crates/acp/fixtures/opencode-1.18.30/**`, `crates/acp/fixtures/opencode-2.0.22/**` (+ status frames, permission `<child>:<id>` ask from opencode's test shape), new `crates/acp/fixtures/rfd/**` (hand-written from schema 1.10.2 shapes, marked synthetic) | §7.1 handler changes (session id propagation, `child_update` → announces + child traffic, `SubagentUpdate` → `ChildAnnounce`, `initialize_request` `subagents`, opencode 1.x task recogniser); §4.3 mitigation; §4.5 test 1; `child_update_fixtures_become_session_updates` re-pinned | `cargo test -p acp`; `cargo check -p rpc --all-targets`; `cargo test -p rpc --lib -- acp::` |

Hand-off: which fixture → which `Update` sequence (W2 uses them in its `Fake`); the `EXIT_GRACE` value; whether the `exit 17` test failed before the mitigation (settles R2A G1).

### W2 — INGEST

| owns (files) | delivers | gate |
|---|---|---|
| `crates/agent/src/acp_child.rs`, new `crates/agent/src/acp_child/observed.rs`; `crates/agent/src/supervisor.rs`, `crates/agent/src/supervisor/*.rs`; `crates/agent/src/store/accounting.rs`, `crates/agent/src/store/turn_usage.rs` (cost meters, `observed` scope rows); their tests | §7.2–7.8: upsert/traffic/permission/cancel/restart; supervisor guard §5.4 and kind arms; cost rule §7.6; §4.5 tests 2 and 3 (they live in `acp_child.rs` tests, hence W2 not W1); `Rig`/`Fake` tests emitting `Update::Subagent` + child-tagged `Update::Session` for each source incl. omp frames via `progress_rows` (uses C's stub until W5 lands → W2 tests feed `ChildAnnounce`s directly and one test with a real omp frame is marked for the Closer) | `cargo test -p agent --lib -- acp_child:: supervisor:: store::accounting store::turn_usage`; `cargo check -p rpc --all-targets` |

Hand-off: event ordering actually emitted per source (for W3/W4 to trust), the guard error text, the `CancelObserved` message name.

### W3 — DESKTOP

| owns (files) | delivers | gate |
|---|---|---|
| `crates/desktop/ui/src/**` **except** `src/rpc/extra.ts` and `src/rpc/bindings/**` (C's) | §9 end to end against the generated types; tests §9.4; mock parity (eye glyph, source tags, roster card, ObservedBar, Watching header) | desktop `tsc --noEmit`; `vitest run`; `vite build` |

Hand-off: component list + screenshots of the four surfaces (tree, roster, strip, watch view) from a fixture-driven story or the smoke; any field it needed and did not find (→ Closer).

### W4 — TUI + CHANNELS

| owns (files) | delivers | gate |
|---|---|---|
| `crates/tui/**`, `crates/slack/**`, `crates/telegram/**` | §10: real arms replacing C's stubs, `◎` rows, read-only open, Esc routing, board rows without threads/topics; tests named in §10.1 | `cargo test -p tui`; `cargo test -p slack`; `cargo test -p telegram` |

Hand-off: the exact board/row strings (for the smoke doc).

### W5 — OMP

| owns (files) | delivers | gate |
|---|---|---|
| `crates/acp/src/omp/**` (replacing C's stubs); `crates/acp/fixtures/omp-18.3.1/**` recorded from `/tmp/recon-141/omp-18.3.1-task.jsonl` (lines 8, 12, 27, 44 at least) and the on-disk `ReadA.jsonl`/`ReadA.md`/`ReadA.json` + parent `async-result` entry (copied with prose fields trimmed) | §6.4 feed 1 (`progress_rows`, nested ids, stale `completed` rule, background/sync detection) and feed 2 (`Tailer`, `to_updates`, sidecar/async-result settlement); unit tests on the fixtures: snapshot-latest-wins, `a.b` → parent `a`, cursor stops at a partial line, `.md` → completed, settled job ids | `cargo test -p acp -- omp::` |

Hand-off: the fixture inventory and the `Entry` → `SessionUpdate` table as implemented.

### Closer

| owns (files) | delivers | gate |
|---|---|---|
| integration edits anywhere after W1–W5 land (resolving hand-off gaps, the W2 omp-frame test), `docs/design/141-as-built.md`, removal of `141-handoff-*.md` | one coherent tree; as-built doc with deltas from this design | the one workspace-class gate: `just check && just test && just clippy`; desktop `tsc --noEmit && vitest run && vite build`; `git diff Cargo.lock` limited to the three SDK packages ± edges |

### Z — SMOKE

| owns (files) | delivers | gate |
|---|---|---|
| `docs/design/141-smoke.md`; scripts under `/tmp` only | the live matrix §13.3 run against real omp 18.3.1, opencode 1.18.30, opencode2 2.0.22, a remote theuth over ACP, `t --acp` as agent; engine restart and UI reload checks; parity rows 3, 5, 13, 22, 24, 25, 28, 29 observed | every matrix cell recorded with the observed text/screenshot; any ✗ is a Closer ticket, not a doc note |

## 13. Verification

### 13.1 Gate order

`acp` → `agent` → `rpc` → `tui`/`slack`/`telegram` → `theuth` (binary), then desktop `tsc --noEmit` → `vitest run` → `vite build`; finally `just check && just test && just clippy`. Each wave runs only its crate gate (§12); the Closer runs the chain once.

### 13.2 Fixture replays (deterministic)

| fixture | asserts |
|---|---|
| `opencode-2.0.22/child_update.{tool_call, tool_call_update.completed, agent_message_chunk}.json` + new `status.{created,running,completed,failed,interrupted}.json` | `Update::Subagent` states per §6.2; child traffic on `ses_ee4d9a752…`; `agent_message_chunk` → `TextDelta` on the child (not `ReasoningDelta` on the parent) |
| `opencode-2.0.22/request_permission.diff.json` (`<child>:<toolu>` ask) | `ExternalApproval{session: child}` |
| `opencode-1.18.30/task.{tool_call, in_progress, completed}.json` (new) | one `opencode_task` child: running → completed, `acp_session` rewritten to `ses_ee4d89f0…`, `harness_model = "opencode/big-pickle"` |
| `omp-18.3.1/task.{tool_call, completed_stale, mid, final}.json` | two `omp_task` children; stale `completed` rows ignored once a later snapshot exists; final rows `completed`, `cost 0.12128025`/`0.12170525` as `counted` meter rows; parent `usage_update 0.13948325` unchanged → subtree total 0.38246875 |
| `omp-18.3.1/ReadA.jsonl` (+ `.md`, `.json`, parent `async-result`) | tail produces `TurnStarted`, `ToolCallStarted(read)`, `ToolCallFinished`, `ToolCallStarted(yield)`, `ToolCallFinished`; cursor at EOF; `.md` → completed; `async-result` job `ReadA` → completed |
| `rfd/subagent_update.*.json` (synthetic) | announce/upsert, `capabilities.cancel` → `can_cancel`, `idle{stopReason, usage}` → idle + display-only usage, `unknown`; child traffic by outer `sessionId`; `session_message_chunk` directed lines |
| `exit 17` child (§4.5) | `exit status: 17` in the connect error; `exit status: 3` in `Update::Closed` |

### 13.3 Live smoke matrix (Z)

| step | omp 18.3.1 | opencode 1.18.30 | opencode2 2.0.22 | remote theuth over ACP | `t --acp` as agent |
|---|---|---|---|---|---|
| Parity rows 3, 5, 13, 22, 24, 25, 28, 29 (R2A §6.3 cells) | ✓ | ✓ | ✓ | rows 22, 26 | rows 27, 28 |
| Two-subagent prompt → children appear in sidebar tree + strip within 1 s of the harness's first frame, with source tag and state dot | `omp task`, `pending`→`running`→`completed`; cost per child ≈ 0.12 USD counted; subtree total > parent `usage_update` | `opencode`, `running`→`completed`, model `opencode/big-pickle`, cost `—`, ObservedBar 1.x note | `opencode`, `pending`→`running`→`completed`, transcript shows `read` card + `alpha line one` as assistant text, cost `—` | its native subagents stay invisible here (D12) — record as expected | n/a |
| Roster card replaces the spawn tool card; rows link to children | ✓ (ReadA/ReadB, activity "Reading first line", tools 2, tokens 9118) | ✓ (one row per task call) | ✓ (join via `metadata.sessionID`) | — | — |
| Open a child: no composer, Watching header, crumb, ObservedBar; ModelMenu disabled; `send_input` via RPC refused with §5.4 text | ✓ | ✓ | ✓ | — | — |
| Parent answers **before** children finish (prompt omp not to `wait`) → children still reach `completed` via the tail within ~10 s | ✓ (feed 2) | n/a | ✓ (child_update continues after turn) | — | — |
| Stop subagent: button absent/disabled for omp/opencode; parent Stop (tree) → opencode foreground child `cancelled`, omp children keep running and complete | ✓ | ✓ | ✓ | — | — |
| Engine restart mid-run → children `unknown`/"lost contact" until re-observed; omp children settle from `.md`/async-result on resume; no duplicate rows | ✓ | ✓ (stays as last state; completes on the task completed frame only if the turn is still open) | ✓ | — | — |
| UI reload → `load_session` of a child returns its history + `kind: observed`; tree rebuilt from `list_agents` with `observed` | ✓ | ✓ | ✓ | — | — |
| TUI: `◎` rows, Enter read-only, Esc hint; Slack/Telegram board rows, no thread/topic | ✓ | ✓ | ✓ | — | — |
| Kill the harness mid-run → parent turn error text has `exit status`/signal + stderr tail (row 24) and children → `unknown` | ✓ | ✓ | ✓ | drop peer → `connection closed …` | close stdin → clean exit |

### 13.4 Out of scope (D12)

- theuth-as-harness emitting RFD `subagent_update` + child-addressed updates over its own relay (`crates/rpc/src/acp.rs:704-787`) so a parent engine sees a remote theuth's subagents — follow-up design.
- claude-agent-acp / codex-acp draft names (`subagent_spawned`/`subagent_state_update`) — theuth does not drive them; the RFD path covers them once they ship v1.
- opencode 1.x HTTP route (`opencode acp --port`) for live children/cost — not built.
- Per-child stop for omp/opencode — no protocol surface (F15, F20); RFD only.
- omp 18.8.4 speculative spawns (children may start before the `tool_call` frame) — handled by the structural recogniser (any frame with `progress[]`) but not smoked (binary installed is 18.3.1).

## 14. Open questions

None blocks the contract; defaults in brackets.

1. **Q1 — `idle` for RFD children.** The RFD has no terminal state; an `idle` child may be prompted again by its harness. Should the tree keep showing idle children as live-ish (greyed, not "done")? [default: idle → `Idle` state, check glyph only for `completed`; the ObservedBar says "idle — the harness may resume it".]
2. **Q2 — omp sync spawns.** Show the child's cost greyed as "inside parent", or hide the number entirely to avoid misreading? [default: greyed with the rule label.]
3. **Q3 — omp `tokens` semantics.** `tokens` = input+output+cacheWrite excluding cacheRead (R1 §2). Show `tokens` (billing volume) or `contextTokens` (latest context) in the roster? [default: `tokens` in the roster, `contextTokens/contextWindow` in the ObservedBar meter.]
4. **Q4 — Roster card scope.** Replace the spawn tool card fully, or render the roster **under** the plain card so the model's own `content` text (omp's "Spawned 2 background agents…") stays visible? [default: roster replaces the body; the card header keeps the title and a disclosure to the raw text.]
5. **Q5 — `requires_action` from opencode.** Map a child-attributed permission ask to `requires_action` for opencode too (not only RFD)? [default: yes, for both; it reverts to `running` when answered.]
6. **Q6 — omp tail cursor reset.** On a rotated/truncated child file (cursor > len) reset to 0 and accept possible duplicate text rows, or stop tailing and mark `unknown`? [default: reset; tool rows dedupe by call id, text may duplicate.]
7. **Q7 — Dreaming/search.** Include observed transcripts in dreaming's ledger and search by default (D10 says "may")? [default: yes for search (read-only open), yes for dreaming as "child".]
8. **Q8 — Upstream (D13).** File the omp `modes/acp` RFD emitter proposal (R1 "Smallest upstream change", ~150-200 lines incl. honouring `yielded && !isTerminal`) and the opencode RFD-frames proposal now, or after Z proves the client side? [default: after Z, with the fixtures as the test vectors.]
9. **Q9 — 18.8.4 field additions** (`currentToolIntent`, `completionPercent`, `recentTools[].isError`). Surface `completionPercent` as a progress bar when present? [default: yes, in `observed_detail` → roster row, no bar when absent.]
10. **Q10 — Private trees.** Observed children of a private ACP child inherit private filtering via `admitted`; should their transcripts also be kept out of the durable store like private members? [default: no special-casing — an ACP child cannot exist in a private session today (`supervisor.rs:781-783`).]

## 15. Revision history

| date | change |
|---|---|
| 2026-10-08 | Initial design from recon R1/R1B/R2A/R2B and rulings D1–D13 (Sky, 11:19/11:46). Recon reports copied verbatim as `141-recon-*.md`. |
