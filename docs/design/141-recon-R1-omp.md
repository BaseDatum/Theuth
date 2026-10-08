# Recon R1 — omp subagents as seen by an ACP client

Sources: `/tmp/recon-141/omp-18.3.1` (tag v18.3.1, 6204b75), `/tmp/recon-141/omp-18.8.4` (tag v18.8.4, 40e9368). Installed binary: omp 18.3.1. Paths below are clone-relative; `[18.3.1]`/`[18.8.4]` marks the tree.

## 1. ACP mapping of the `task` tool
Mapper: `packages/coding-agent/src/modes/acp/acp-event-mapper.ts` [18.3.1]; **byte-identical in 18.8.4** (`diff` = 0 lines). Driven from `AcpAgent.#handlePromptEvent` (acp-agent.ts:1420-1481) which is subscribed only for the lifetime of a `session/prompt` turn (acp-agent.ts:873-875; unsubscribed in `#finishPrompt`, :1631).

| event | emitted frame | code |
|---|---|---|
| `tool_execution_start` | `tool_call{toolCallId, title, kind, status:"pending", rawInput: args, content?, locations?}` | mapper:198-208 → `buildToolCallStartUpdate` :455-480 |
| `tool_execution_update` | `tool_call_update{toolCallId, status:"in_progress", rawOutput: partialResult, content?, locations?}` | mapper:209-229 |
| `tool_execution_end` | `tool_call_update{toolCallId, status: isError?"failed":"completed", rawOutput: result, content?, locations?}` (+ a `plan` update for `todo`) | mapper:230-257 |

For `task` specifically:
- `title` = the tool call's `intent` arg when present (mapper:580-583; capture: `"Spawning parallel file readers"`), else `task` has no `path/command/pattern/query` → bare `"task"` (:585-597).
- `kind` = `"other"` (`mapToolKind` :160-185 has no `task` case).
- `rawInput` = the raw task args (`{context, tasks:[…]}`) verbatim.
- `content` on updates = `[{type:"content", content:{type:"text", text}}]` built from `rawOutput.content[].text` (`extractToolCallContent` :744-761), capped at `ACP_TEXT_LIMIT = 4000` chars (:137, :1084-1086). `rawOutput` itself is **not** truncated — the full `details.progress[]` rides every frame.
- `_meta`: never set by the mapper (no `_meta` anywhere in acp-event-mapper.ts; capture confirms).
- `locations`: none for task (`extractToolLocations` finds no path args).

Filtering: `isInternalAgentMessageTool` (mapper:150-152; the brief's "isInternalHubMessageTool" does not exist under that name in either tag) drops `write` calls whose target scheme has `write.scope === "coordination"` — i.e. `write agent://<id>` / `agent://all` peer messages (`AgentProtocolHandler.spec.write.scope`, internal-urls/agent-protocol.ts:78). Nothing else is filtered; `wait`, `read proc://…`, `write proc://<id>/kill` all pass through as ordinary tool calls (capture shows `wait`).

Throttling: none in the ACP layer (every `tool_execution_update` becomes a frame, mapper:209; `#handlePromptEvent` awaits each `sessionUpdate` serially, acp-agent.ts:1451-1462). Upstream, the executor coalesces per-subagent progress to ≥150 ms (`PROGRESS_COALESCE_MS = 150`, executor.ts:1459, 1499-1523) but flushes immediately on tool start/end, message_end, retry events and agent_end (:1813, :1954, :1966, :2007, :2019). With N subagents each flush produces one frame carrying all N rows, so bursts of N frames a few ms apart are normal (capture t=8639-8647: 5 frames).

Background-mode lifecycle on the wire (18.3.1 default, see §2): `tool_call` → 1-3 `in_progress` → **`completed` within ms** (the spawn acknowledgement) → **28 further `in_progress` frames for the same `toolCallId`** carrying live progress → last frame text `"Background task <id> complete."` with `details.async.state:"completed"`, still `status:"in_progress"`. There is never a second terminal status. The `completed` frame's `details.progress` is a stale pre-start snapshot (rows `pending`) because the result object was built at index.ts:1018 before the jobs' first `markRunning` tick (index.ts:1158-1163) ran.

Turn boundary [INFERENCE from code, not exercised — the model in the capture blocked on `wait`]: if the model ends its turn while background subagents still run, `agent_end` is emitted with `isTerminal:false, yielded:true` (agent-session.ts:4060-4062, 3698-3702); the ACP agent ignores those flags (no `isTerminal`/`yielded` reference in acp-agent.ts) and resolves the prompt (acp-agent.ts:1469-1479) → `#finishPrompt` unsubscribes (:1631). All later task progress frames, and the autonomous follow-up turn started by the async-result injection (agent-session.ts:1874-1877), stream with no `promptTurn` and are dropped by `#handlePromptEvent`'s guard (:1422-1424); the lifetime subscription only forwards `thinking_level_changed`/`model_changed` (:1368-1371). ACP mode never consults `hasPendingAsyncWork()` (only rpc-mode/interactive-mode/tan do: rpc-session-settle.ts:26,81; interactive-mode.ts:6308).

18.8.4 differences: mapper unchanged; acp-agent.ts diff is 46 lines (agentInfo.name `"oh-my-pi"`→`"omp"` :636; `subagentCost` passthrough in `#cloneUsageStatistics` :2206; ask-dialog timeout helper; config-update dedupe). Task tool internals changed substantially (index.ts 532 changed lines): spawns may **start speculatively while the model is still streaming the `tasks[]` array** (`#startSpeculative`, index.ts:762-780, `TaskLaunchSession`), so subagent work can begin before the ACP `tool_call` frame is emitted; the async forwarder now also copies `currentToolArgs/currentToolArgsKey/currentToolIntent/currentToolStartMs/completionPercent` (index.ts:1299-1330). New `AgentProgress` fields: `currentToolArgsKey?`, `currentToolIntent?`, `recentTools[].argsKey?/intent?/isError?`, `completionPercent?` (tui/src/tools/task.ts:2316+; `task.completionProbe` is interactive-only — "print, RPC, ACP and SDK runs never probe", task/settings.ts:313-322). `TaskToolDetails` unchanged (task.ts:2509-2522).


## 2. Partial and final result shapes (`details`, `AgentProgress`)
Types live in `packages/tui/src/tools/task.ts` [18.3.1] (re-exported to coding-agent). The task tool returns `AgentToolResult<TaskToolDetails>`; `rawOutput` on the wire is `{content, details}`.

### `TaskToolDetails` (task.ts:2012-2025)
| field | type | set when |
|---|---|---|
| `projectAgentsDir` | `string\|null` | always (null in capture) |
| `results` | `SingleResult[]` | sync/blocking spawns only — populated when the inline spawn finishes (index.ts:1056). **Async spawns never land here**: `results` stays `[]` for the whole job lifetime (capture, all 31 frames). |
| `totalDurationMs` | ms since the task call started (index.ts:914) | every snapshot |
| `usage?` | aggregated `Usage` of *sync* spawns (index.ts:915 `syncUsage`) | absent for async spawns |
| `outputPaths?` | `string[]` | sync spawns |
| `progress?` | `AgentProgress[]` — one entry per spawn, in `index` order (index.ts:917) | every snapshot |
| `async?` | `{state:"running"\|"completed"\|"failed", jobId, type:"task"}` (index.ts:918-922) | present when ≥1 async spawn; `state` flips when **all** jobs settle, `failed` if any failed; `jobId` = first registered job id (≈ first agent id), not per-agent |

### `AgentProgress` (task.ts:1832-1913) — unit/semantics from the source comments
| field | type | notes |
|---|---|---|
| `index` | number | position in the `tasks[]` array |
| `id` | string | agent id = `tasks[].name` or generated AdjectiveNoun, de-duplicated `base`, `base-2`, … via `AgentOutputManager.allocate` (output-manager.ts:84-91, seeded from files already in the artifacts dir :55-81). Unique per spawn within a parent session; stable for the agent's lifetime; **also** the job id, the `agent://`/`history://` id and the transcript filename stem. Nested children get `<parentId>.<child>` (output-manager.ts:90; executor.ts:3901 passes `parentTaskPrefix: id`). |
| `agent` | string | agent type (`scout`, `task`, …) |
| `agentSource` | `"bundled"\|"user"\|"project"` | |
| `status` | `"pending"\|"running"\|"completed"\|"failed"\|"aborted"` | async: pending→running at index.ts:1159 (after semaphore), terminal at :1233 (`aborted` if `singleResult.aborted`, `failed` if exitCode≠0/error/missing result, else `completed`); `aborted` also on abort-before-start :1153; `failed` on executor throw :1273 |
| `task` | string | rendered subagent user prompt (`"Complete assignment thoroughly:\n\n"+assignment`) |
| `assignment?` | string | raw `tasks[].task` |
| `description?` | string | tiny-model generated label, set asynchronously (executor.ts:1531-1537); absent in capture |
| `lastIntent?` | string | last tool call's `intent` arg (executor.ts:1692-1695) |
| `currentTool?`, `currentToolArgs?`, `currentToolStartMs?` | | set on `tool_execution_start` (executor.ts:1683-1691), cleared on end (:1719-1721). Not seen in capture frames (reads finished within the 150 ms coalesce window). |
| `recentTools` | `{tool,args,endMs}[]` | newest first, max 5 (executor.ts:1709-1717); `endMs` = epoch ms |
| `recentOutput` | `string[]` | last ≤8 non-blank lines of the current assistant text, newest first, from an 8 KiB tail (executor.ts:1246,1470-1475); reset at each assistant `message_start` (:1666) |
| `toolCount` | number | incremented on `tool_execution_start` (:1682); includes `yield` |
| `requests` | number | assistant `message_end` count (:1860) |
| `tokens` | number | **cumulative input+output+cacheWrite, excludes cacheRead** (task.ts:1850; executor.ts:1926) — lifetime billing volume, not context |
| `contextTokens?` | number | latest assistant message `usage.totalTokens` (executor.ts:1930-1933) |
| `contextWindow?` | number | model context window |
| `cost` | USD | cumulative `usage.cost.total` across assistant messages (executor.ts:1922) |
| `durationMs` | ms | `Date.now()-startTime` on each emit (executor.ts:1479); async path copies the final value at index.ts:1234. Mid-run async frames showed `durationMs:0` because the forwarder (index.ts:1172-1192) does not copy it. |
| `modelOverride?`, `modelRole?` | | `modelRole` = role alias (`smol` for scout in capture) |
| `resolvedModel?` | `provider/id[:thinking]` | e.g. `anthropic/claude-fable-5-1:high` |
| `resolvedModelIdentity?`, `resolvedThinkingLevel?`, `resolvedModelIsFallback?`, `resolvedModelRoute?` | | fallback flag set on model fallback (executor.ts:1991) |
| `advisor?` | boolean | true only when a live advisor is attached (executor.ts:1965) |
| `extractedToolData?` | `Record<string,unknown[]>` | per-tool handler extractions; `yield` → `[{data,status}]`; set at completion (index.ts:1240) |
| `retryState?` | `{attempt,maxAttempts,delayMs,errorMessage,startedAtMs}` | while sleeping between provider retries (executor.ts:1999-2007), cleared on `auto_retry_end` (:2012) |
| `retryFailure?` | `{attempt,errorMessage}` | terminal retry give-up (:2014) |
| `inflightTaskDetails?` | `TaskToolDetails` | nested: snapshot of the child's own in-flight `task` call (executor.ts:1823-1829); cleared when that call ends (:1726) |

`SingleResult` (task.ts:1916-2009) adds `exitCode, output, stderr, truncated, structuredOutput, usage, outputPath, error, aborted, abortReason, isolated, patchPath, branchName, …` — only reachable for sync spawns (or inside `wait`'s `details.jobs[].resultText/structured`).

### Execution modes (index.ts:749-846)
- `async.enabled` (tools/settings.ts:846-850, default **true**; `protocolDefault: ["rpc"]` only — ACP inherits the user's value) + a registered `AsyncJobManager` → every non-`blocking` agent type becomes a **background job** (index.ts:929). The tool returns within milliseconds (`"Spawned N background agents…"`, :1007-1019). Only agent types declaring `blocking: true` run inline (`#runSyncSpawns`, :1035).
- Sync fallback (`#executeSyncFanout`, :782/:1303): async disabled or no job manager. Then `results[]`/`usage` are filled and the final `tool_call_update` is the real end.
- Batch: `task.batch` (task/settings.ts:195-199, default true, protocolDefault rpc/acp) → schema `{context, tasks:[{name?,agent,task,outputSchema?,schemaMode?,tools?,isolated?}]}` (types.ts:48-96). Flat form `{name?,agent,task,…}` still accepted at runtime (task.ts:1741-1768).
- Mixed (some blocking): tool returns after inline ones finish; progress rows for them are settled from results (index.ts:1062-1075).

### How async spawns report later
Each job's `reportProgress` (job-manager.ts:369-380) calls the register-option `onProgress` (index.ts:1290-1292) → the **tool's original `onUpdate` callback** → agent-loop pushes `tool_execution_update` with the original `toolCallId` (agent-loop.ts:3341-3349) → ACP `tool_call_update{status:"in_progress"}` **after** the `completed` frame. Per-subagent progress is forwarded by `forwardSyncProgress` (index.ts:1164-1197) copying metrics into the shared `spawn.progress` row, then `buildDetails()` snapshots all rows (index.ts:911-923). Completion delivery to the model is a `custom_message` `async-result` on the yield queue (agent-session.ts:1874-1877, 2673), never a wire frame.

Status/retry: provider retries surface only as `retryState`/`retryFailure` on the row; the executor caps structured-output retries and `yield` errors (executor.ts:1803-1808, `MAX_YIELD_TOOL_ERRORS`).

### Examples
Mid-run and final `rawOutput` examples: see §8 "Verbatim shapes" (real frames at t=7500 and t=9781).

## 3. Nesting
- Gate: `task.maxRecursionDepth` (task/settings.ts:258-276), default **2**, `-1` unlimited, `0` none; `protocolDefault: ["rpc","acp"]` → ACP hosts use 2 regardless of user settings. `canSpawnAtDepth` (types.ts:214-216): a child at `taskDepth < maxRecursionDepth` keeps the `task` tool. Child depth = parent+1 (executor.ts:3470-3472); `atMaxDepth` strips the tool.
- Nested ids: `<parent>.<child>` (output-manager.ts:90 via `parentTaskPrefix: id`, executor.ts:3901). On disk (verified): `…/<parentSession>/GsSessionModels/GsSessionModels.ModelFacts.jsonl` with header `parentSession: …/GsSessionModels.jsonl` — 17 such depth-2 files exist under `~/.omp/agent/sessions`.
- Surface to the top-level call: the child's progress row carries `inflightTaskDetails` (a full `TaskToolDetails` of the grandchild batch) while the child is inside its `task` call (executor.ts:1823-1829), cleared at that call's end (:1726); afterwards finalized data lives in `extractedToolData.task`. **But** the async forwarder (index.ts:1172-1192) copies an explicit field list that does **not** include `inflightTaskDetails`, `description`, `currentToolArgs`, `currentToolStartMs`, `durationMs` — so over ACP (background mode) nested progress is **not visible** mid-run; `extractedToolData` arrives only at completion (index.ts:1240). In sync mode (`#executeSync` onProgress, index.ts:1533-1540) the whole row is copied, so `inflightTaskDetails` would be on the wire. [INFERENCE: not exercised live.]
- The whole spawn tree shares the root session's observability EventBus (executor.ts:3895-3897); frames on `task:subagent:event|progress|lifecycle` channels (types.ts:3-5,35) carry `parentToolCallId`, `sessionFile`, `detached`, `index` (executor.ts:1484-1495, 2621-2632) — in-process only, no ACP emitter.

## 4. Subagent transcripts on disk
Layout (`~/.omp/agent/sessions`, dirs.ts:900-902):
```
~/.omp/agent/sessions/<encoded-cwd>/<ISO-ts>_<sessionId>.jsonl          parent transcript
~/.omp/agent/sessions/<encoded-cwd>/<ISO-ts>_<sessionId>/              artifacts dir = session file minus ".jsonl" (session-manager.ts:127-130, 2564-2567)
    <agentId>.jsonl      subagent transcript (executor.ts:3433-3435)
    <agentId>.md         final yield/output text (executor.ts:2532-2536)
    <agentId>.json       structured-output sidecar when a schema produced data (executor.ts:2549-2558)
    .<agentId>.jsonl.lock.os   single-writer lock, created at spawn (observed 11:53:52.40 = job start)
    <agentId>/<agentId>.<child>.jsonl   grandchildren (child session's own artifacts dir)
    __advisor.jsonl      advisor transcript when enabled (output-manager.ts:35-38)
```
Encoded cwd: `/tmp/recon-141/omp-scratch` → `--private-tmp-recon-141-omp-scratch--` (macOS realpath `/private/tmp`, session-paths.ts:86-96); `~/Develop/theuth` → `-Develop-theuth`. Session filename: `${fileSafeTimestamp}_${sessionId}.jsonl` (session-manager.ts:1544). **ACP `sessionId` == omp session id** (capture: `01a11b25-d8f7-…` appears in both).

Capture session on disk: `~/.omp/agent/sessions/--private-tmp-recon-141-omp-scratch--/2026-10-08T10-53-47-639Z_01a11b25-d8f7-71b3-a309-a306e24281dc/{ReadA,ReadB}.{jsonl,md,json}`. `ReadA.jsonl` = 12 entries; types in order: `title`, `session` (`{version:3, id:<own uuid7>, cwd, parentSession:<abs path of parent .jsonl>}`), `model_change`, `thinking_level_change`, `session_init` (system prompt), `message`(user), `message`(assistant, with `usage{input,output,cacheRead,cacheWrite,totalTokens,cost{…}}`, `model`, `provider`, `stopReason`, `responseId`, `duration`, `ttft`), `custom` (`customType:"tool_execution_start"`, `data{toolCallId,toolName,startedAt,args,intent}`), `message`(toolResult), … Same format as the parent (same `SessionManager`, opened via `SessionManager.open(sessionFile, …, {parentSession, suppressBreadcrumb:true})`, executor.ts:3722-3727). Entries carry per-entry timestamps 2.5 s apart, consistent with live append [INFERENCE: append-as-you-go; not watched during the run]. `.md`/`.json` are written at the end (mtime = completion).

Correlation chain for theuth: ACP `sessionId` → glob `~/.omp/agent/sessions/*/*_<sessionId>.jsonl` → strip `.jsonl` → `<dir>/<progress[].id>.jsonl`. `toolCallId` is **not** stored in the subagent file; it is only in the parent transcript (`custom tool_execution_start` entry, `toolResult.toolCallId`) alongside `toolResult.details.progress[].id`, and on the ACP wire as `rawOutput.details.progress[].id`. The subagent header's `parentSession` points back to the parent file (depth-2 files point at the depth-1 file).

Parent transcript: the task `toolResult` entry (parent jsonl entry 7) stores the **early** `details` snapshot (both rows `pending`, counters 0) — the final state never gets persisted into the parent transcript for async spawns (agent-session.ts:2863-2867 keeps only an in-memory `#activeToolExecutionUpdates` cache). The async-result `custom_message` (entry 14) holds the delivered `<task-result id=… status=… duration=…>` text and `details.jobs[]`.


## 5. Control: per-subagent cancel, parent cancel
### ACP ext methods (acp-agent.ts [18.3.1] `extMethod`, :1131-1209)
| method | line | purpose |
|---|---|---|
| `SPEECH_MODELS_LIST_METHOD` (constant) | :1133 | speech model catalog |
| `_omp/sessions/listAll` | :1135 | list all sessions (`limit`) |
| `_omp/projects/list` | :1144 | sessions bucketed by project |
| `_omp/chats/byCwd` | :1172 | sessions for a cwd |
| `_omp/usage` | :1180 | provider usage reports (`fetchUsageReports`) |
| `_omp/extensions` | :1189 | list extensions |
| `_omp/extensions/toggle` | :1196 | enable/disable provider |
| default | :1207 | `Unknown ACP ext method` |

**No ext method touches jobs or subagents.** No `extNotification` handler in acp-agent.ts (not found). 18.8.4: same list (diff shows no extMethod changes).

### Per-subagent cancel
Only from inside the model's own turn: the parent model writes `proc://<agentId>/kill` (proc-protocol.ts:32-38, 172-176; `contentOptional` for `/kill`, :75) → `AsyncJobManager.cancel` (job-manager.ts:430-432 sets `status:"cancelled"`, aborts the job's `AbortController`) → `runSignal` aborts the executor (`requestAbort("signal")`, executor.ts:1407-1414) → row `status:"aborted"` (index.ts:1233) and a final `"Background task <id> failed."`-style progress frame. Cross-owner cancel is rejected (job-manager.ts:420-424 comment). theuth cannot trigger this over ACP except by prompting the model.

### ACP `session/cancel` of the parent
`cancel` (acp-agent.ts:1075-1088) → `#beginCancelCleanup` (:1097-1111): aborts the turn controller, unsubscribes, resolves the prompt with `stopReason:"cancelled"`, then `session.abort({reason: USER_INTERRUPT_LABEL})` (:1119) with a bounded timeout (`ACP_CANCEL_CLEANUP_TIMEOUT_MS`, :621).
- **Sync/blocking spawns** share the tool call's `record.signal` (agent-loop.ts:3340) → aborted → `SingleResult.aborted:true`, progress `status:"aborted"` (index.ts:1066-1073).
- **Async/background spawns keep running.** `AgentSession.abort()` (agent-session.ts:8588-8673) never calls `#cancelOwnAsyncJobs`; that only happens on new-session/reset/dispose paths (:5352, :8702, :10422, :10555, :5004 via `#disposeOwnedAsyncJobs`). Their completions are later injected as async-result follow-ups (agent-session.ts:1874-1877), which in ACP would start an autonomous turn invisible to the client (§1 "Turn boundary"). [INFERENCE: code path, not exercised live.] Closing the session (`session/close`, `#closeManagedSession`) disposes the AgentSession → `cancelAll({ownerId})` with the shutdown reason (agent-session.ts:5004, job-manager.ts:587-597) → subagents are **parked**, not tombstoned (executor.ts:1411 maps the shutdown reason to `"shutdown"`).

## 6. Cost/usage inclusion
Source of `usage_update.cost` and `PromptResponse.usage`: `sessionManager.getUsageStatistics()` (acp-agent.ts:2177-2185, 1478 via `#buildTurnUsage` = delta from the turn baseline :866). `UsageStatistics` is accumulated in the session index from `entryUsage(entry)` (session-manager.ts:361-368 [18.3.1]): assistant messages' `usage`, **plus `toolResult` entries with `toolName === "task"` → `details.usage`** (`taskUsageFrom`, :355-359), summed by `addUsage` (:370-382).
- `details.usage` exists **only for sync/blocking spawns** (index.ts:915 `syncUsage`, filled at :1057 from `mergeSyncPayloads`). Async spawns never write it (their `toolResult` is the early spawn ack; async-result `custom_message` entries carry no `usage`).
- Therefore in **background mode (default) subagent tokens/cost are NOT included** in `usage_update.cost`, `usage_update.used`, or `PromptResponse.usage`. Capture: `usage_update.cost.amount = 0.13948325` while the two subagent rows ended at `cost 0.12128025 + 0.12170525 = 0.2429855`; parent assistant message 1 alone cost `0.10464425`. `PromptResponse.usage.totalTokens = 55951` is parent-only context volume. theuth must **add** `progress[].cost`/`tokens` from the task frames (final values arrive in the last frame per job; take the max per `id`, rows are cumulative) — no double counting risk in this mode.
- In **sync mode** (async disabled or `blocking:true` agents) the inverse holds: `details.usage` (aggregate of the children) *is* folded into the parent's `usage_update.cost`/`PromptResponse.usage` → adding `progress[].cost` again **would** double count. Distinguish by `details.async` presence / `details.results.length > 0` with `details.usage` set.
- 18.8.4 adds `UsageStatistics.subagentCost` (session-entries.ts:349-350) accumulated from the **same** task-toolResult `details.usage` (session-manager.ts:546-552), i.e. still sync-only; ACP passes it through `#cloneUsageStatistics` (acp-agent.ts:2206) but `usage_update` still sends only `cost` (:2181-2185 unchanged) — not on the wire.
- Units: `cost` USD; `tokens` = input+output+cacheWrite (no cacheRead); `contextTokens` = latest turn total. The subagent transcript's assistant entries carry the exact per-request `usage{input,output,cacheRead,cacheWrite,totalTokens,cost{input,output,cacheRead,cacheWrite,total}}` (ReadA.jsonl entry 6) for an after-the-fact authoritative sum.

## 7. Native ACP RFD support
- `subagent_update`, `session_message`, `session_message_chunk`, `clientCapabilities.subagents`, `opencode/child-session-updates`: **not found** in 18.3.1, 18.8.4, or `main` @ `ed1faf4a` (2026-10-08) — `grep -rn` over `packages/**/*.ts` matches only unrelated `parentSessionId` fields in `vibe/*`, `tan-command-controller.ts`, `browser/relay/bridge.ts`.
- `ClientCapabilities` type (packages/utils/src/acp/protocol.ts:146-152 [18.3.1]): `fs`, `terminal`, `auth`, `elicitation`, `_meta` — unknown keys like `subagents` are ignored (initialize result in §8 has no echo).
- `SessionUpdate` union (protocol.ts:320-336): `agent_message_chunk|agent_thought_chunk|user_message_chunk|tool_call|tool_call_update|plan|current_mode_update|config_option_update|available_commands_update|session_info_update|usage_update`. Runtime validator `sessionNotification` (schema.ts:86-121) rejects any other `sessionUpdate` — used on the **client-side receive path only**; the agent-side `AgentSideConnection.sessionUpdate` (connection.ts:86-88) is an unvalidated `notify("session/update", params)`, and `notify()`/`extNotification()` (connection.ts:119-129) can send arbitrary methods. So an upstream patch can emit new update kinds without touching the schema.
- omp uses **its own ACP implementation**: `packages/utils/src/acp/{connection,protocol,schema,stream,transport}.ts`, header comment "Behavior-compatible reimplementation of @agentclientprotocol/sdk's used protocol surface" (protocol.ts:1). `@agentclientprotocol/sdk` does not appear in any `package.json` of 18.3.1, 18.8.4 or main (grep: no matches). `PROTOCOL_VERSION = 1` (protocol.ts:13).
- In-process observability that an RFD emitter could tap already exists: EventBus channels `task:subagent:event` (raw `AgentSessionEvent` per subagent, `{id, event}`, types.ts:35-41, executor.ts:1593-1596), `task:subagent:progress` (`{index, agent, agentSource, task, parentToolCallId, detached, assignment, progress, sessionFile}`, executor.ts:1484-1495) and `task:subagent:lifecycle` (`{…, status:"started"|"completed"|"failed"|"aborted", sessionFile, index}`, executor.ts:2621-2632, 4036-4042) — the whole spawn tree publishes on the root session's bus (executor.ts:3895-3897). The TUI's Agent Hub consumes these; ACP mode does not subscribe (no reference to these channels under `modes/acp/`).


## 8. Live capture (omp 18.3.1)
Client: `/tmp/recon-141/capture.py` (python3; bun and node also present). Recording: `/tmp/recon-141/omp-18.3.1-task.jsonl` (55 frames, pretty dump `/tmp/recon-141/capture-pretty.txt`). stderr empty. One run, no retry needed. ACP session id `01a11b25-d8f7-71b3-a309-a306e24281dc`; whole prompt turn 10.9 s (t=359 → t=11290 ms). `agentInfo.version` = `18.3.1`. No `session/request_permission` was ever sent (scout subagents only `read`).

### Frame kinds (counts, direction)
- out: `initialize` 1, `session/new` 1, `session/prompt` 1
- in: result 3; `session/update` → `available_commands_update` 1, `session_info_update` 2, `tool_call` 3, `tool_call_update` 36, `agent_message_chunk` 6, `usage_update` 1
- `_meta` appears in exactly one frame: our own `initialize` request. omp emitted **no** `_meta`, no non-standard method, no `subagent_update`, no `session_message*`. `initialize` result: `agentCapabilities: {loadSession, mcpCapabilities{http,sse}, promptCapabilities{embeddedContext,image}, sessionCapabilities{list,fork,resume,close}}` — no `subagents` key; our `clientCapabilities.subagents` / `_meta["opencode/child-session-updates"]` were silently ignored.

### Ordered timeline (ms since spawn)
| t | frame |
|---|---|
| 412 | `available_commands_update`, `session_info_update` |
| 4946 | `tool_call` toolu_013x… `title:"Spawning parallel file readers"`, `kind:"other"`, `status:"pending"`, `rawInput` = task args (`context`, `tasks:[{name:"ReadA",agent:"scout",task,outputSchema},{name:"ReadB",…}]`) |
| 4949 | `tool_call_update` in_progress, rawOutput.content text `"Spawned 2 agents..."`, details.progress both `status:"pending"`, `details.async:{state:"running",jobId:"ReadA",type:"task"}` |
| 4949 | in_progress `"Running background task ReadA..."` (ReadA running, ReadB pending) |
| 4951 | in_progress `"Running background task ReadB..."` (both running) |
| **4951** | **`tool_call_update` status `completed`** for the task call — text `"Spawned 2 background agents using scout.\n- ReadA (job ReadA)\n- ReadB (job ReadB)…"`; details.progress both **back to `pending`** with zero counters (stale snapshot), `details.async.state:"running"`. The task tool returned ~5 ms after spawn: **18.3.1 spawns in background by default**. |
| 5028 … 9781 | **27 more `tool_call_update` with `status:"in_progress"` for the already-completed toolu_013x…** — background progress re-uses the parent tool call id (see §1 for the code path). Each carries full `details.progress[]` snapshots. |
| 6196 | `tool_call` toolu_01F4… `"Awaiting subagent results"` (tool `wait`), `rawInput:{}` |
| 6302…8307 | `wait` in_progress updates, `details:{op:"wait", jobs:[{id,type:"task",status,label,durationMs,resolvedModel,resolvedModelIdentity,resolvedThinkingLevel,agentUrlId}]}` |
| 8648 | `wait` completed: `details.jobs[1]` ReadB `status:"completed"`, `resultText` (task-result XML), `structured:{source:"caller",mode:"permissive",status:"valid",data:{first_line:"bravo line one"}}`; `details.meta.source:{type:"report",value:"background jobs snapshot"}` |
| 9781 | last task update: text `"Background task ReadA complete."`, both progress entries `status:"completed"`, `details.async.state:"completed"` |
| 9973 | `tool_call` toolu_01NV… `"Awaiting ReadA result"` + immediate `tool_call_update` **`failed`** with `details:{__interrupted:true, source:"interrupt_skipped", execution:"started"}` — the second `wait` was skipped because a background completion was queued. |
| 11287–11289 | 6 × `agent_message_chunk` (`"- a.txt: `alpha line one`\n- b.txt: `bravo line one`"`), `messageId` set |
| 11290 | `usage_update {size:1000000, used:14445, cost:{amount:0.13948325, currency:"USD"}}`, `session_info_update`, prompt result `{stopReason:"end_turn", usage:{inputTokens:10, outputTokens:532, totalTokens:55951, cachedReadTokens:47333, cachedWriteTokens:8076}}` |

Delivery of ReadA's result to the model happened via a `custom_message` (`customType:"async-result"`) appended to the parent transcript (parent jsonl entry 14, `details.meta.source.value:"background job delivery"`) — **it was not emitted on the ACP wire** (no frame between t=9973 and t=11287).

### Verbatim shapes
Task `tool_call` (t=4946):
```json
{"sessionUpdate":"tool_call","toolCallId":"toolu_013x6rgBK7MEUsQq79DoL4YK","title":"Spawning parallel file readers","kind":"other","status":"pending",
 "rawInput":{"context":"# Goal\nReport the first line of a text file in cwd …","tasks":[{"name":"ReadA","agent":"scout","task":"# Target\n`a.txt` in cwd.…","outputSchema":{"type":"object","properties":{"first_line":{"type":"string"}},"required":["first_line"]}},{"name":"ReadB","agent":"scout","task":"…b.txt…","outputSchema":{…}}]}}
```
Mid-run `tool_call_update` (t=7500, after the task call already reported `completed`):
```json
{"sessionUpdate":"tool_call_update","toolCallId":"toolu_013x6rgBK7MEUsQq79DoL4YK","status":"in_progress",
 "rawOutput":{"content":[{"type":"text","text":"Running agent ReadA..."}],
  "details":{"projectAgentsDir":null,"results":[],"totalDurationMs":2553,
   "progress":[
    {"index":0,"id":"ReadA","agent":"scout","agentSource":"bundled","modelRole":"smol","status":"running",
     "task":"Complete assignment thoroughly:\n\n# Target\n`a.txt`…","assignment":"# Target\n`a.txt`…",
     "recentTools":[{"tool":"read","args":"a.txt:1-1","endMs":1791456834916}],"recentOutput":[],
     "toolCount":1,"requests":1,"tokens":8962,"cost":0.1149025,"durationMs":0,
     "resolvedModel":"anthropic/claude-fable-5-1:high","resolvedModelIdentity":"anthropic/claude-fable-5-1","resolvedThinkingLevel":"high",
     "contextTokens":8962,"contextWindow":1000000,"lastIntent":"Reading first line"},
    {"index":1,"id":"ReadB", … "recentTools":[{"tool":"read","args":"b.txt:1-1:raw","endMs":1791456834430}],"toolCount":1,"requests":1,"tokens":8965,"cost":0.1150525, …}],
   "async":{"state":"running","jobId":"ReadA","type":"task"}}},
 "content":[{"type":"content","content":{"type":"text","text":"Running agent ReadA..."}}]}
```
Final task update (t=9781): same envelope, text `"Background task ReadA complete."`, each progress entry adds `"status":"completed"`, `recentTools:[{tool:"yield",args:"",endMs},{tool:"read",…}]`, `toolCount:2, requests:2, tokens:9118, cost:0.12128025, durationMs:4819, contextTokens:9037`, `extractedToolData:{"yield":[{"data":{"first_line":"alpha line one"},"status":"success"}]}`; `details.results` stays `[]`; `details.async.state:"completed"`. The last frame with `status:"completed"` for the task id is the early one at t=4951 — the `completed` frame is *not* the final state.

### Subagents' own tool calls on the wire?
No. The subagents' `read` tool call ids (e.g. `toolu_01N97dmhtms4GH34fEciDfVT`, from ReadA.jsonl entry 7) never appear in the capture; only the parent's three tool calls do. Subagent activity is visible solely inside `rawOutput.details.progress[].recentTools/lastIntent/toolCount` of the parent's task tool call (and `wait`'s `details.jobs`).

### Throttling observed
Task progress frames arrive in bursts (e.g. 8639/8642/8644/8647/8647) — not throttled on the ACP side; see §1 for the emitter.


## Gaps and recommendations

### What theuth can show live today (18.3.1, unmodified omp)
From the parent's `task` `tool_call_update.rawOutput.details.progress[]` frames (one frame per ≥150 ms per subagent, all rows in every frame), **while the parent's `session/prompt` is in flight**:
- identity: `id` (= job id = transcript stem), `index`, `agent`, `agentSource`, `modelRole`, `assignment`; nested ids `a.b`
- status: `pending|running|completed|failed|aborted`; batch state `details.async.state`
- model: `resolvedModel`, `resolvedModelIdentity`, `resolvedThinkingLevel`, `resolvedModelIsFallback`
- usage: `tokens` (in+out+cacheWrite), `contextTokens/contextWindow`, `cost` USD, `requests`, `toolCount`
- activity: `lastIntent`, `recentTools[≤5]{tool,args,endMs}`, `recentOutput[≤8]`, `currentTool` (rare — cleared within the coalesce window), `retryState/retryFailure`
- result: `extractedToolData.yield[0].data/status` on completion; `wait` frames add `details.jobs[].resultText/structured/durationMs/agentUrlId`
Keying: `(sessionId, toolCallId, progress[].id)`; rows are cumulative snapshots, take the latest per `id`. Treat `status:"completed"` on the task call as *spawn acknowledged*, not finished; finished = every row terminal or `details.async.state !== "running"`.

### Only after the fact / on disk
- Full subagent transcript: `~/.omp/agent/sessions/<encoded-cwd>/<ts>_<sessionId>/<id>.jsonl` (same jsonl format as the parent; `custom tool_execution_start` entries give the child's own tool calls with ids, args, intents; assistant entries give exact per-request usage). Tail it live (the writer appends per entry; RPC's own `get_subagent_messages` does exactly this with `fromByte`, rpc-subagents.ts:68-105) — but ACP gives no `sessionFile`; theuth must derive the path from `sessionId` + `id` (§4). Final output `<id>.md`, structured `<id>.json`.
- Parent transcript's persisted task `toolResult.details` is the stale pre-start snapshot (§4) — useless for reload; the async-result `custom_message.details.jobs[]` has final status/duration/structured per job.

### Impossible without an omp change
1. **Anything after the parent's turn ends.** Background jobs that outlive the prompt (model replies before `wait`/delivery) → their progress frames and the autonomous follow-up turn are dropped (acp-agent.ts:1422-1424, 1631; agent-session.ts:4060-4062). theuth sees the subagents frozen in their last pre-end state and never learns they completed, nor sees the parent's follow-up text. (Default-on since `async.enabled` has no ACP protocolDefault, tools/settings.ts:848.)
2. **Subagents' individual tool calls, thoughts, text** are not on the wire (only `recentTools`/`recentOutput` digests; no `toolCallId`s).
3. **Per-subagent cancel** from the client (no ext method; `proc://<id>/kill` is model-only, §5). `session/cancel` leaves background subagents running (§5).
4. **Nested (depth-2) live progress** in background mode: `inflightTaskDetails` is not forwarded (index.ts:1172-1192) — only the final `extractedToolData.task`.
5. **Transcript path / sessionFile** is never transmitted over ACP (it is in the in-process `SubagentProgressPayload.sessionFile`, session-observer-registry.ts:18).
6. Subagent cost is excluded from `usage_update` in background mode and *included* in sync mode (§6) — theuth must branch on `details.async`/`details.usage` to avoid double counting.

### Smallest upstream change (omp) for RFD `subagent_update` + child-addressed `session/update`
All plumbing already exists for RPC; ACP only lacks a subscriber. Hook points [18.3.1 lines; 18.8.4 equivalents shift but exist]:
1. **Bus access** — `createAcpSessionFactory` (main.ts:492-559) builds `eventBus` (:504) but not `subagentEventBus`; `createAgentSession` (sdk.ts:1503) then mints a private one. Create a `subagentEventBus = new EventBus()` per ACP session in the factory, pass it at :517-533, and hand it back alongside `{session, setToolUIContext}` (:558) so `AcpAgent.#createManagedSessionRecord` (acp-agent.ts:1331) can keep it on the `ManagedSessionRecord`. (Alternative: expose `ToolSession.subagentEventBus`, tools/index.ts:308, through `AgentSession`.)
2. **Emitter** — mirror `RpcSubagentRegistry` (rpc-subagents.ts:107-128): subscribe to `TASK_SUBAGENT_LIFECYCLE_CHANNEL`, `TASK_SUBAGENT_PROGRESS_CHANNEL`, `TASK_SUBAGENT_EVENT_CHANNEL` and map:
   - lifecycle `started|completed|failed|aborted` (`SubagentLifecyclePayload`, session-observer-registry.ts:24-40: `id, agent, agentSource, description, status, sessionFile, parentToolCallId, index, detached`) → `session/update {sessionId: <parent>, update:{sessionUpdate:"subagent_update", subagentId:id, parentToolCallId, status, agent, sessionFile, _meta}}`;
   - progress (`SubagentProgressPayload` :10-21, `progress: AgentProgress`) → `subagent_update` with the model/usage/activity fields;
   - `task:subagent:event` (`{id, event: AgentSessionEvent}`, types.ts:38-41) → run the **existing mapper** `mapAgentSessionEventToAcpSessionUpdates(event, childSessionId, {cwd})` (acp-event-mapper.ts:188) and send each notification with `sessionId = childSessionId` (child-addressed `session/update`, i.e. the subagent's own `tool_call`/`tool_call_update`/`agent_message_chunk`/`agent_thought_chunk`). Child session id: use the subagent's own session uuid (`session` header `id` in `<id>.jsonl`; in-process via the child `AgentSession.sessionId`) or deterministically `${parentSessionId}/${agentId}`; announce the mapping in the lifecycle `started` frame.
   This subscription must live on the **session-lifetime** subscription (acp-agent.ts:2105-2120, next to `#handleLifetimeEvent`), not on `promptTurn`, so frames flow after the parent's turn ends. Send path is unvalidated: `this.#connection.sessionUpdate(...)` (connection.ts:86-88) — no schema change required on the agent side; add the kinds to `SessionUpdate` (protocol.ts:320-336) and `sessionNotification` (schema.ts:97-120) for omp's own client use.
3. **Gate on capability** — read `params.clientCapabilities.subagents` / `_meta` in `initialize` (acp-agent.ts:636) and advertise `agentCapabilities.subagents` in the result.
4. **Turn boundary (recommended)** — in `#handlePromptEvent`'s `agent_end` branch (acp-agent.ts:1469-1479) honour `event.yielded && !event.isTerminal` (agent-session.ts:3700-3701): keep forwarding while `record.session.hasPendingAsyncWork()` (agent-session.ts:2609-2611), like `RpcSessionSettleWatcher` (rpc-session-settle.ts:80-82). Without it, item 1 under "Impossible" remains even with `subagent_update`.
5. **Per-subagent cancel** — add `_omp/subagents/cancel {sessionId, subagentId}` to `extMethod` (acp-agent.ts:1131) calling `AsyncJobManager.cancel(subagentId, {ownerId})` (job-manager.ts:420-432) — ≈10 lines.

Estimated diff for 1-3: ~150-200 lines in `modes/acp/` + ~10 in `main.ts`; zero changes to `task/` or the executor. Cost handling for theuth is unchanged by this patch (still sum `progress[].cost` in background mode).

