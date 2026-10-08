# Recon R1B — opencode subagents (1.18.30 / 2.0.22) and other ACP agents

Status: COMPLETE (2026-10-08). Sources (all shallow clones under /tmp/recon-141/): `opencode-1.18.30` (tag v1.18.30 = 3104c142), `opencode-2.0.22` (tag v2.0.22 = 527f0b93), `opencode-v2` (branch `v2` HEAD 0621ea9d, 2026-10-08 — the 2.x line; note the repo's default branch `dev`/`main` is still the 1.x line, 1.18.35), `claude-agent-acp` (6a162baf), `codex-acp` (4583cb39), `sdk-1.6.0/package` (npm @agentclientprotocol/sdk 1.6.0 tarball, extracted only), `schema-1.10.2/agent-client-protocol-schema-1.10.2` and `schema-1.9.1` (crates, extracted only). theuth paths are repo-relative. Live captures: `/tmp/recon-141/opencode-2.0.22-task.jsonl`, `/tmp/recon-141/opencode-1.18.30-task.jsonl` (+ `.stderr`); client `/tmp/recon-141/acp_client.py`. Nothing outside /tmp was modified except this file.

## 1. 2.0.22 `opencode/session/child_update`

Everything lives in `packages/cli/src/acp/` (2.0.22). On branch `v2` the same contract is split into `packages/cli/src/acp/child.ts:3-37` (`UpdateMethod`, `Update` type, `update()`) and `capabilities.ts:3` — field names unchanged.

**Emitter.** `turn.ts:154-157`: `case "ChildUpdate": return input.connection.extNotification(ACPTranslate.ChildSessionUpdateMethod, output.update)` (errors ignored). Method name constant `translate.ts:17` = `"opencode/session/child_update"`; capability key `translate.ts:16` = `"opencode/child-session-updates"`.

**Opt-in.** `service.ts:155-158`: `childSessionUpdates: params.clientCapabilities?._meta?.["opencode/child-session-updates"] === true`. The agent also advertises `agentCapabilities._meta["opencode/child-session-updates"]: true` (`service.ts:176`; seen live in the initialize response). The flag is read per prompt (`turn.ts:376`) and stored in the turn `Context.childUpdates` (`translate.ts:29`). Without it, child updates are **projected onto the parent transcript as plain `session/update`** with `toolCallId = "<childId>:<id>"`, title `"<childTitle>: <title>"` and `_meta["opencode/child-session"]` (`translate.ts:495-500`, `600-616`; test `turn.test.ts:116-147`). With it, nothing about the child goes to `session/update` at all (`turn.test.ts:167`).

**Params contract** (`translate.ts:48-69`):
```ts
{ rootSessionId: string; childSessionId: string; parentSessionId: string; depth: number; title?: string }
 & ( { type: "update"; update: SessionUpdate }
   | { type: "status"; status: "created"|"running"|"completed"|"failed"|"interrupted"; error?: { type: string; message: string } } )
```
No agent/model/description fields. `title` = the child session title = the subagent tool's `description` input (`packages/core/src/tool/plugin/subagent.ts:190`), carried on the `session.created` event (`translate.ts:141`). `depth` = 1 for a direct child, parent.depth+1 for deeper (`translate.ts:139-141`). `rootSessionId` is always the ACP session (`translate.ts:508`); `parentSessionId` is the real parent (grandchild → the child id). Grandchildren are tracked only if their parent is already a known child (`translate.ts:139-140`); test `turn.test.ts:149-194` shows `depth: 2, parentSessionId: "ses_child", rootSessionId: <root>`. Core caps nesting at `experimental.subagent_depth` (default 1 → depth ≥ 2 refused, `subagent.ts:129-133`), so in practice no grandchildren unless the user raises it.

**Status transitions.** `created` on `session.created` with parentID (`translate.ts:136-149`); `running` on `session.execution.started` (`:186-187`); `completed`/`interrupted`/`failed(+error)` on `session.execution.succeeded/interrupted/failed` (`:334-342`). No "cancelled" value — a cancelled child is `interrupted`. Live order per child: `status:created` → `status:running` → N×`update` → `status:completed`, and then the parent's `tool_call_update completed` for the subagent tool (capture t=5.214).

**Forwarded update kinds** (`type:"update"`, `update` is a full ACP `SessionUpdate` already projected by `projectChildUpdate`): `agent_message_chunk` (`:216`), `agent_thought_chunk` (`:225`), `tool_call` (`:243`), `tool_call_update` (in_progress/completed/failed, `:261`,`:278`,`:294`,`:312`), `session_info_update` with `_meta["opencode/retry"]` (`:193`,`:204`) and `_meta["opencode/compaction"]` (`:213`), and `tool_call_update failed "Cancelled"` from `abandonTools` after a cancel drain (`:382-400`). **Not forwarded for children:** `session.step.ended/failed` (token usage) are dropped when `child` is set (`:326-333`), so there is **no child usage_update, no child token counts, no child model** on the wire. Child `messageId`s are the child's own (`msg_…`), `toolCallId`s are prefixed `"<childSessionId>:<toolCallId>"` and every forwarded update also carries `_meta["opencode/child-session"] = {id, parentID, depth, title?}` (`:600-616`).

**Child permission requests** arrive as ordinary `session/request_permission` on the **root** `sessionId` with `toolCall.toolCallId = "<childSessionId>:<toolCallId>"` and title prefixed `"<childTitle>: …"` (`turn.ts:135-148` passes `toolCallPrefix: ask.child.id, titlePrefix: ask.child.title`; `permission.ts:50-59`; test `permission.test.ts:130-133`). Child form elicitations likewise (`elicitation.test.ts:446`). The reply goes to the child's server session (`permission.ts:72-75`). Permission asks are accepted from any known child regardless of the capability flag (`translate.ts:157-161`). None fired in the live run (read tool is auto-allowed; the `subagent` spawn permission `subagent.ts:138-151` was auto-allowed by default config).

**Lifetime.** Children that are still open when the parent turn ends (background subagents, or a cancelled turn) keep streaming `child_update`s from a background consumer forked onto the session scope (`turn.ts:297-315`, `translate.ts:30-31,154`; tests `turn.test.ts:196-267`). Children created *after* the turn ended are ignored (`turn.test.ts:237`). Closing the session stops it (`turn.test.ts:269-285`).

## 2. RFD `subagent_update` / `session_message` paths in 2.0.22 and main

**Not sent by any opencode version.** `grep -rn 'subagent_update\|session_message_chunk\|clientCapabilities?.subagents' packages/` over `opencode-2.0.22`, `opencode-v2` (HEAD 0621ea9d) and the default branch (`opencode-main`, 1.18.35) → not found outside i18n strings and a `stats.subagents` test. `service.ts:153-181` (2.0.22 initialize) only reads `_meta["opencode/child-session-updates"]`, `elicitation.form` and `_meta["terminal-auth"]`; `clientCapabilities.subagents` is never read. The only ACP update kinds opencode emits are the ones in `translate.ts` (§1) plus `usage_update`, `available_commands_update`, `config_option_update`, `user_message_chunk` (replay).

**Bundled SDK.** 2.0.22 and `v2`: `@agentclientprotocol/sdk` **1.6.0** (`packages/cli/package.json:27`). 1.18.30: **0.21.0** (`packages/opencode/package.json:57`). The strings `subagent_update` / `session_message_chunk` in the 2.0.22 binary come from the SDK's generated zod schema: `sdk-1.6.0/package/dist/schema/zod.gen.js`, `types.gen.d.ts` and `schema/schema.json` list `SessionUpdate` variants `[..., 'compaction_update', 'compaction_summary_chunk', 'subagent_update', 'session_message', 'session_message_chunk']` with `$defs.SubagentUpdate`, `SubagentCapabilities`, `SubagentSessionCapabilities`, `SessionMessage`, `SessionMessageChunk`, and `ClientCapabilities.subagents` — all marked `**UNSTABLE**`. opencode only *validates* against this schema; it has no code path producing those variants.


## 3. 1.18.30: task tool over ACP and other observation routes

**What the ACP client sees (live capture, `/tmp/recon-141/opencode-1.18.30-task.jsonl`).** Per task call, on the root session only:
1. `tool_call` `{toolCallId:"call_…", title:"task", kind:"think", status:"pending", rawInput:{}}` (t=8.03)
2. `tool_call_update` `{status:"in_progress", kind:"think", title:"Read first line of a.txt", rawInput:{description, prompt, subagent_type:"explore"}}` (sent twice, t=8.97)
3. `tool_call_update` `{status:"completed", title:"Read first line of a.txt", content:[{type:"content", content:{type:"text", text:"<task id=\"ses_ee4d89f0cffe7zXAvHsSIT0UsD\" state=\"completed\">\n<task_result>\nalpha line one\n</task_result>\n</task>"}}], rawOutput:{output:"<task …>", metadata:{parentSessionId:"ses_ee4d8bb5…", sessionId:"ses_ee4d89f0…", model:{modelID:"big-pickle", providerID:"opencode"}, truncated:false}}}` (t=13.98 / 37.27).

Source: `rawInput` = `Parameters` (`packages/opencode/src/tool/task.ts:43-62`: description, prompt, subagent_type, task_id?, command?, background?); `ctx.metadata({title: params.description, metadata})` with `metadata = {parentSessionId, sessionId, model, background?}` (`task.ts:185-195`; `jobId` added for background, `:269-273`); output rendered by `renderOutput` → `<task id=… state=running|completed|error>[<summary>…</summary>]<task_result|task_error>…` (`task.ts:64-79`). Although core sets the metadata at tool start, the ACP layer only emits `rawOutput` on the completed update (`packages/opencode/src/acp/tool.ts:197,223-233`); the in_progress update in the capture carries `title` + `rawInput` but no metadata, so the **child session id and model become visible to the client only when the task completes**. Child title convention in the DB: `"<description> (@<agent> subagent)"` (`task.ts:160`). No per-child tool parts, tokens or cost reach the client.

**Child-session events never reach the ACP client.** `packages/opencode/src/acp/event.ts:191-195` / `:214-217`: every `message.part.updated|delta` is gated by `this.input.session.tryGet(sessionId)` (`acp/session.ts:108`), which knows only sessions created by `session/new`/`session/load`; child sessions are dropped. Same for permissions: `acp/permission.ts:51-54` `if (!session) return` — a child's `permission.asked` is **silently dropped, never answered** (child permission is normally pre-derived from the parent's rules, `task.ts:139-142`, so it rarely fires). PR #25165 "route child session events to parent ACP session (#21802)" is **CLOSED, unmerged** (auto-closed 2026-04-30 for template non-compliance; `pr://anomalyco/opencode/25165`), and the 1.x default branch (`opencode-main` = 1.18.35) still has no `parentID` handling in `packages/opencode/src/acp/*.ts` (grep → not found). Capture confirms: zero child-originated messages in 1.18.30 (29 inbound lines, all root).

**Other observation routes.**
- *HTTP API (1.18.30):* `opencode acp` starts the HTTP server in-process (`packages/opencode/src/cli/cmd/acp.ts:20-29`: `Server.listen(opts)` with `withNetworkOptions` → `--port`/`--hostname` accepted, `cli/network.ts:7-14`). Auth: Basic with `OPENCODE_SERVER_PASSWORD` (`server/auth.ts:17-34`); with no password set `required()` is false → unauthenticated loopback. Useful endpoints (`server/routes/instance/httpapi/groups/session.ts:78-105`): `GET /session/:id/children` (`:82`, handler `handlers/session.ts:89-91` → `session.children`), `GET /session/:id/message`, `GET /session/:id`, `POST /session/:id/abort` (`:91`), plus the global SSE event stream the ACP layer itself consumes (`acp/event.ts:153` `sdk.global.event`). So theuth could run `opencode acp --port N` and poll/subscribe for children. *(2.0.22: not reachable — `opencode acp` spawns a private `opencode serve --stdio --port 0` with a random `OPENCODE_PASSWORD`, URL printed only on the child's stdout which the acp process consumes: `packages/cli/src/services/standalone.ts:18-34,36-58`, `commands/handlers/acp.ts:16-17`.)*
- *`opencode export [sessionID]`* (1.18.30 `cli/cmd/export.ts:223-292`): JSON `{info, messages}` for one session; must be called per child id (no recursion). 2.0.22 has `export <session>` too (`commands/commands.ts:444-450`). Offline only.
- *sqlite* (`sqlite3 -readonly ~/.local/share/opencode/opencode.db`, shared by both versions): 1.x tables `session` (`parent_id`, `title`, `agent`, `model` JSON, `cost`, `tokens_*`, `version`; index `session_parent_idx`), `message`, `part` (`data` JSON). 2.x tables `session_v2` (same + `fork_session_id`, `idle_outcome`, `time_idle`…; index `session_v2_parent_idx`) and `session_message` (`type` user|assistant|system|idle…, `seq`, `data` JSON with `content[]`, `cost`, `tokens`, `model`). Live rows: 1.18.30 root `ses_ee4d8bb5…` + two children with `parent_id`=root, title `"Read first line of a.txt (@explore subagent)"`, agent `explore`; 2.0.22 root `ses_ee4d9b05…` + children `ses_ee4d9a752…`/`ses_ee4d9a5c9…` agent `general`, `idle_outcome=succeeded`. The DB is written with WAL by a live process; read-only polling works but is a last resort.

## 4. Control: aborting one child

**Over ACP: no.** Neither version exposes a child as an ACP session. 2.0.22 `session/cancel {sessionId}` → `turn.cancel` → `FiberMap.remove(turns, params.sessionId)` (`turn.ts:399-401`); a child id has no registered turn → silent no-op. 1.18.30 `cancel` → `session.get(params.sessionId)` (`acp/service.ts:351-354`) → unknown id → error. `session/close` likewise targets ACP sessions only.

**Root cancel cascades to foreground children.** 2.0.22: cancel interrupts the turn fiber → `windDown` → `client.session.interrupt({sessionID: root})` (`turn.ts:255-271`); the server-side `subagent` tool's `jobs.block(...)` is interrupted → `Effect.onInterrupt(() => [sessions.interrupt(child.id), jobs.cancel(child.id)])` (`packages/core/src/tool/plugin/subagent.ts:234-240`) → child emits `session.execution.interrupted` → `child_update status:"interrupted"` (`translate.ts:337-339`), then root `stopReason:"cancelled"`. Background children (`background:true`) are **not** cancelled by a root cancel and keep streaming `child_update`s (`turn.ts:306-314`, test `turn.test.ts:240-267`). 1.18.30: cancel → `sdk.session.abort(root)` (`acp/service.ts:330-336`) → task tool `runTask().pipe(Effect.onInterrupt(() => ops.cancel(nextSession.id)))` (`task.ts:294`) → child aborted; background tasks (`OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true`, `task.ts:97-102`) survive.

**Per-child abort via HTTP** exists server-side in both: 1.18.30 `POST /session/:sessionID/abort` (`groups/session.ts:91`); 2.0.22 `POST /api/session/:sessionID/interrupt` (`packages/protocol/src/groups/session.ts:747-763`, core `session.ts:212`). Reachable only in 1.18.30 (see §3 HTTP note). Aborting a child this way makes the parent's task tool return an error/cancelled result (`subagent.ts:246-251`, `task.ts:213-223`) while the root turn continues.

## 5. Cost: child cost in root usage

**Not included in either version; no double-count risk today, but the root number is incomplete.**
- 2.0.22: `usage_update.cost = session.get(root).cost` (`turn.ts:338-347`); `session.cost` is incremented only for `event.data.sessionID` of each `session.step.ended` (`packages/core/src/session/projector.ts:58-73,686-691`) — children accumulate on their own rows. `PromptResponse.usage` sums `session.step.ended` tokens of the root only (`translate.ts:326-328` drops child steps; `:362-373`). **Live proof:** `usage_update.cost.amount = 0.002590045` == `session_v2.cost` of the root; each child row has `cost = 0.001741405` (sqlite query above). True spend = 0.00259 + 2 × 0.00174 = 0.00607 USD, i.e. the root figure under-reports by 57 %. Child tokens are also only in the DB (`session_v2.tokens_*`, per-message `session_message.data.cost/tokens`), never on the wire.
- 1.18.30: `usage_update.cost = totalSessionCost(messages of root)` (`packages/opencode/src/acp/usage.ts:111-115,188-216`) — root assistant messages only. Live: `cost 0` (free `opencode/big-pickle` model) so not numerically provable here, but `session` rows show children with their own `tokens_input` 4849/4841 vs root 36116; the response `usage.totalTokens 36096` matches the root alone.
- PR #43645 "roll up subagent cost into parent session" (`pr://anomalyco/opencode/43645`): base `dev`, **CLOSED unmerged** (auto-cleanup 2026-09-20, reopen requests through 2026-10-01, maintainers silent). Not in 1.18.30, 1.18.35 (`opencode-main`), 2.0.22 or `v2` HEAD — grep for `totalCost`/`session.cost` route → not found. Issue #39740 remains open. If it ever merges, `usage_update.cost` would become root+descendants and theuth would have to stop adding child costs itself; version-gate on `agentInfo.version`.


## 6. Live captures

**Setup.** Client: `/tmp/recon-141/acp_client.py` (python3 3.x at /opt/homebrew/bin/python3; bun and node also present). Spawns `<binary> acp` with `OPENCODE_PRINT_LOGS=1` (as `crates/acp/src/agents.rs:60-65` does; agents.yaml has `args: [acp]`, `env: {}`), cwd `/tmp/recon-141/oc-scratch` (a.txt/b.txt as specified). `initialize` exactly as the brief (protocolVersion 1, `terminal:false`, `auth.terminal:true`, `subagents:{}`, `_meta["opencode/child-session-updates"]:true`, clientInfo theuth-recon/0), `session/new {cwd, mcpServers:[]}`, the two-subagent prompt, auto-`allow_once` on any `session/request_permission`. Each version ran once, first try, no retries. Both binaries picked up the user's global opencode config (plugins such as Hindsight log in stderr; 1.18.30 default model `opencode/big-pickle`, agent `build`, subagent `explore`; 2.0.22 default `anthropic/claude-haiku-5-5`, subagent `general`).

Files: `/tmp/recon-141/opencode-2.0.22-task.jsonl` (35 lines, 32 inbound) + `.stderr`; `/tmp/recon-141/opencode-1.18.30-task.jsonl` (30 lines, 27 inbound) + `.stderr`. Line format `{t, dir: "in"|"out", msg}`.

### 6.1 opencode 2.0.22 — prompt→end_turn in 5.9 s (t=1.03→6.89)
Ordered kinds (count):
```
 1 resp initialize (t=0.24; agentCapabilities._meta["opencode/child-session-updates"]:true, loadSession, sessionCapabilities{additionalDirectories,close,delete,fork,list,resume})
 1 resp session/new (t=1.03; sessionId ses_ee4d9b05effekZbnkfXPDosL35)
 2 session/update available_commands_update   (t=1.03, 1.75)
 2 session/update config_option_update         (t=1.33, 1.62)
 2 session/update tool_call pending  title "subagent" kind "think" rawInput {}        (t=2.84, 3.33)
 4 session/update tool_call_update in_progress  rawInput {agent:"general", description, prompt}  (t=3.33, 3.35, 3.72, 3.74)
 2 child_update status created   (t=3.35 child A ses_ee4d9a752ffe2XT5Oz2Fldml2j; t=3.74 child B ses_ee4d9a5c9ffeOvC0TZT7VeqD09)
 2 child_update status running   (t=3.35, 3.74 — same ms as created)
 2 child_update update tool_call pending (read)             (t=4.17, 4.60)
 2 child_update update tool_call_update in_progress         (t=4.32, 4.78)
 2 child_update update tool_call_update completed           (t=4.33, 4.79)
 2 child_update update agent_message_chunk ("alpha line one" / "bravo line one")  (t=5.21, 5.52)
 2 child_update status completed (t=5.21, 5.54)
 2 session/update tool_call_update completed (subagent tool)   (t=5.21, 5.54 — immediately after each child's completed status)
 2 session/update agent_message_chunk          (t=6.86, 6.89)
 1 session/update usage_update {used:16408, size:1000000, cost:{amount:0.002590045, currency:"USD"}}  (t=6.89)
 1 resp session/prompt {stopReason:"end_turn", usage:{inputTokens:6, outputTokens:366, totalTokens:26914, thoughtTokens:234, cachedReadTokens:10042, cachedWriteTokens:16266}, _meta:{}}
 0 session/request_permission, 0 subagent_update / session_message*, 0 agent_thought_chunk (haiku), 0 child usage.
```
Subagent children ran concurrently (A created 3.35, B created 3.74, A completed 5.21, B 5.54).

Verbatim shapes (root-level parent tool call for child A):
```json
{"method":"session/update","params":{"sessionId":"ses_ee4d9b05effekZbnkfXPDosL35","update":{"sessionUpdate":"tool_call","toolCallId":"toolu_018wLewXWWb6vBSf6pqKcnLn","title":"subagent","kind":"think","status":"pending","locations":[],"rawInput":{}}}}
{"method":"session/update","params":{"sessionId":"ses_ee4d9b05…","update":{"sessionUpdate":"tool_call_update","toolCallId":"toolu_018wLewXWWb6vBSf6pqKcnLn","status":"in_progress","kind":"think","title":"subagent","locations":[],"rawInput":{"agent":"general","description":"Read a.txt first line","prompt":"Read the file a.txt in the working directory /tmp/recon-141/oc-scratch (use the read tool). Reply with only the exact text of its first line. …"}}}}
{"method":"session/update","params":{"sessionId":"ses_ee4d9b05…","update":{"sessionUpdate":"tool_call_update","toolCallId":"toolu_018wLewXWWb6vBSf6pqKcnLn","status":"completed","locations":[],"content":[{"type":"content","content":{"type":"text","text":"<subagent sessionID=\"ses_ee4d9a752ffe2XT5Oz2Fldml2j\" state=\"completed\">\nalpha line one\n</subagent>"}}],"rawOutput":{"metadata":{"sessionID":"ses_ee4d9a752ffe2XT5Oz2Fldml2j","status":"completed","truncated":false}}}}}
```
Note: the parent `tool_call_update completed` has **no `title`/`kind`/`rawInput`** and its `rawOutput.metadata.sessionID` is the only link from the parent tool call to the child (`subagent.ts:264`); the earlier in_progress update is the only place the description/agent appear. The 2.x tool is named `subagent` (`subagent.ts:17`), not `task`; its inputs are `agent, description, prompt, model?, sessionID?, background?` (`subagent.ts:29-48`).

child_update status + update + completion:
```json
{"method":"opencode/session/child_update","params":{"rootSessionId":"ses_ee4d9b05effekZbnkfXPDosL35","childSessionId":"ses_ee4d9a752ffe2XT5Oz2Fldml2j","parentSessionId":"ses_ee4d9b05effekZbnkfXPDosL35","depth":1,"title":"Read a.txt first line","type":"status","status":"created"}}
{"method":"opencode/session/child_update","params":{…same header…,"type":"status","status":"running"}}
{"method":"opencode/session/child_update","params":{…,"type":"update","update":{"sessionUpdate":"tool_call","toolCallId":"ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh","title":"Read a.txt first line: read","kind":"read","status":"pending","locations":[],"rawInput":{},"_meta":{"opencode/child-session":{"id":"ses_ee4d9a752ffe2XT5Oz2Fldml2j","parentID":"ses_ee4d9b05effekZbnkfXPDosL35","depth":1,"title":"Read a.txt first line"}}}}}
{"method":"opencode/session/child_update","params":{…,"type":"update","update":{"sessionUpdate":"tool_call_update","toolCallId":"ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh","status":"in_progress","kind":"read","title":"Read a.txt first line: read","locations":[{"path":"/tmp/recon-141/oc-scratch/a.txt"}],"rawInput":{"path":"/tmp/recon-141/oc-scratch/a.txt"},"_meta":{…}}}}
{"method":"opencode/session/child_update","params":{…,"type":"update","update":{"sessionUpdate":"tool_call_update","toolCallId":"ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh","status":"completed","locations":[{"path":"/tmp/recon-141/oc-scratch/a.txt"}],"content":[{"type":"content","content":{"type":"text","text":"Read file /tmp/recon-141/oc-scratch/a.txt, lines 1-2\n1: alpha line one\n2: alpha line two"}}],"rawOutput":{"metadata":{"truncated":false}},"_meta":{…}}}}
{"method":"opencode/session/child_update","params":{…,"type":"update","update":{"sessionUpdate":"agent_message_chunk","messageId":"msg_11b265c90001y8C3GiljPe61do","content":{"type":"text","text":"alpha line one"},"_meta":{…}}}}
{"method":"opencode/session/child_update","params":{…,"type":"status","status":"completed"}}
```
No child model, agent name, tokens or cost on the wire (DB has: agent `general`, model `anthropic/claude-haiku-5-5`, cost 0.001741405, tokens in/out 6/72, cache r/w 12518/12637 per child).

### 6.2 opencode 1.18.30 — prompt→end_turn in 41.7 s (t=1.72→43.37)
```
 1 resp initialize (t=0.90; no _meta on agentCapabilities; sessionCapabilities{close,fork,list,resume})
 1 resp session/new (t=1.72; ses_ee4d8bb5dffeS1akqW7B497imb)
 1 session/update available_commands_update (t=1.72; user's global commands/skills list)
11 session/update agent_thought_chunk (8 at t=7.6–8.0 before the task calls, 3 at t=43.0–43.1 before the answer)
 2 session/update tool_call pending  title "task" kind "think" rawInput {}   (t=8.03, 9.01)
 4 session/update tool_call_update in_progress title "<description>" rawInput {description, prompt, subagent_type:"explore"} (2 identical per call; t=8.97×2, 9.51×2)
 2 session/update tool_call_update completed  (t=13.98 child A done after 5.0 s; t=37.27 child B done after 27.8 s)
 3 session/update agent_message_chunk ("a.txt: `alpha" / " line one`\nb.txt: `br" / "avo line one`")  (t=43.1–43.3)
 1 session/update usage_update {used:36068, size:200000, cost:{amount:0, currency:"USD"}}  (t=43.37)
 1 resp session/prompt {stopReason:"end_turn", usage:{inputTokens:356, outputTokens:19, totalTokens:36096, thoughtTokens:9, cachedReadTokens:35712}, _meta:{}}
 0 request_permission, 0 child-originated messages of any kind, 0 child_update (capability ignored), 0 config_option_update.
```
Verbatim task shapes:
```json
{"method":"session/update","params":{"sessionId":"ses_ee4d8bb5dffeS1akqW7B497imb","update":{"sessionUpdate":"tool_call","toolCallId":"call_a062fdd9472f454ea6e45c9b","title":"task","kind":"think","status":"pending","locations":[],"rawInput":{}}}}
{"method":"session/update","params":{"sessionId":"ses_ee4d8bb5…","update":{"sessionUpdate":"tool_call_update","toolCallId":"call_a062fdd9472f454ea6e45c9b","status":"in_progress","kind":"think","title":"Read first line of a.txt","locations":[],"rawInput":{"description":"Read first line of a.txt","prompt":"Read the file /private/tmp/recon-141/oc-scratch/a.txt and report its first line exactly as-is. …","subagent_type":"explore"}}}}
{"method":"session/update","params":{"sessionId":"ses_ee4d8bb5…","update":{"sessionUpdate":"tool_call_update","toolCallId":"call_a062fdd9472f454ea6e45c9b","status":"completed","title":"Read first line of a.txt","content":[{"type":"content","content":{"type":"text","text":"<task id=\"ses_ee4d89f0cffe7zXAvHsSIT0UsD\" state=\"completed\">\n<task_result>\nalpha line one\n</task_result>\n</task>"}}],"rawOutput":{"output":"<task id=\"ses_ee4d89f0cffe7zXAvHsSIT0UsD\" state=\"completed\">\n<task_result>\nalpha line one\n</task_result>\n</task>","metadata":{"parentSessionId":"ses_ee4d8bb5dffeS1akqW7B497imb","sessionId":"ses_ee4d89f0cffe7zXAvHsSIT0UsD","model":{"modelID":"big-pickle","providerID":"opencode"},"truncated":false}}}}}
```
Both children ran in parallel server-side (sqlite: two child rows, parent_id = root) but the ACP client could not observe anything between in_progress (t=9.5) and completed (t=37.3) for child B — 27.8 s of silence.


## 7. claude-agent-acp / codex-acp compatibility table

Clones: `claude-agent-acp` (6a162baf, 2026-10-08) and `codex-acp` (4583cb39, 2026-10-07). Both are **TypeScript** adapters (codex-acp is the npm package `@agentclientprotocol/codex-acp`; no `Cargo.toml`/`*.rs` in the clone → the Rust-crate question is moot for it). Both implement the **draft** RFD (agent-client-protocol PR #1992) with local type shims "until the TypeScript SDK publishes PR #1992" (`claude-agent-acp/src/acp-subagents.ts:13-20`; `codex-acp/src/subagents/AcpSubagents.ts:10`). Neither emits the ratified `subagent_update` / `session_message` / `session_message_chunk` (grep both `src/` → not found). Detailed findings (scout-verified, key lines re-read by me) in `agent://AgentAdaptersSubagents`.

| | **opencode 2.0.22** (for comparison) | **claude-agent-acp** | **codex-acp** |
|---|---|---|---|
| Spawn wire | `opencode/session/child_update` ext notification `{rootSessionId, childSessionId, parentSessionId, depth, title?, type:"status", status:"created"}` (`translate.ts:48-69,148`) | `session/update` on parent: `{sessionUpdate:"subagent_spawned", subagentSessionId, name, task, prompt?, capabilities:{}, _meta?}` (`acp-subagents.ts:27-40`; emitted `native-subagents.ts:481-491`; replay `acp-agent.ts:7970-7980`) | same shape, no `prompt` field (`AcpSubagents.ts:17-24`; emitted `CodexSubagentEventRouter.ts:318-327`) |
| State wire | `child_update type:"status"` `running|completed|failed(+error)|interrupted` (`translate.ts:58-61`) | `{sessionUpdate:"subagent_state_update", subagentSessionId, state}` with `state ∈ completed|failed|cancelled|disconnected` on the parent (`acp-subagents.ts:42-49`; `native-subagents.ts:513-519`) | same, on `parentSessionId` (`AcpSubagents.ts:28-33`; `CodexSubagentEventRouter.ts:357-363`) |
| Child content/tool calls | wrapped: `child_update type:"update", update:<SessionUpdate>` with `toolCallId "<child>:<id>"` + `_meta["opencode/child-session"]` (`translate.ts:600-616`) | ordinary `session/update` whose **`sessionId` = child id** (buffered until spawned is announced) (`native-subagents.ts:98-172`); `_meta.claudeCode.parentToolUseId` stamped for non-AIR clients | ordinary `session/update` with `sessionId` = child Codex threadId (`CodexEventHandler.ts:373-407`) |
| Inter-session messages (`session_message*`) | none | none | none |
| Negotiation gate | `clientCapabilities._meta["opencode/child-session-updates"] === true` (`service.ts:156`); agent advertises same key in `agentCapabilities._meta` (`:176`) | `clientCapabilities.subagents` is any non-null non-array object **OR** `_meta.jetbrains.air.capabilities ∋ "nativeSubagentSessions"` (`acp-subagents.ts:108-117`, `air-extension.ts:1,106-118`); agent always advertises `sessionCapabilities.subagents:{}` (`acp-agent.ts:2706-2714`) | identical gate (`AcpSubagents.ts:39-50`); agent advertises `sessionCapabilities.subagents:{}` (`CodexAcpServer.ts:428-436`) |
| Without the gate | child updates projected onto the parent transcript (`translate.ts:495-500`) | Agent/Task stay plain tool calls; AIR gets `_meta.jetbrains.air.subagent:true` | `collabAgentToolCall`/`subAgentActivity` rendered as tool calls, root-attributed |
| Child session id as ACP session | no (`session/cancel`/`prompt` on child id: no-op / error, §4) | opaque id (`task_id`, `${taskId}:generation:N`, replay `${sessionId}:replay-subagent:${toolUseId}`), **not** in the sessions map: `session/prompt` → "Session not found" (`acp-agent.ts:3398-3401`), `session/cancel` silent no-op (`:7291-7295`) | child `threadId` / `${threadId}:generation:2`, not in sessions map (`CodexAcpServer.ts:1008`, cancel lookup `:3567-3576`) |
| Child permission routing | `session/request_permission` on **root** `sessionId`, `toolCallId "<child>:<id>"`, title prefixed (`permission.ts:50-59`) | `sessionId` = **child id** once announced (gate on + agentID present), else root (`acp-agent.ts:8461-8471,8599-8611`) | `sessionId` = child id after the child materializes; root if not gated; auto-`cancel` if child already terminal (`CodexSubagentSubscriptions.ts:84-145`) |
| Per-child cancel | no | no — `capabilities:{}` always; root cancel finishes all children `cancelled` (`acp-agent.ts:7312-7318`, `native-subagents.ts:280-299`) | no — `capabilities:{}`; docs say "does not support targeted child cancel or close" (`docs/subagent-sessions.md:19`) |
| Usage/cost rollup | root only; children never on wire (§5) | root `usage_update` from top-level frames only (`acp-agent.ts:6813-6826`); `cost.amount = result.total_cost_usd` (SDK-defined whether it includes subagents); `_meta.quota.model_usage` does count subagents (`:10185-10204`) | per-thread `usage_update` routed to the child session; root `lastTokenUsage` overwritten by child events `[INFERENCE]` (`CodexEventHandler.ts:1154-1174`) |
| Replay on `session/load` | 2.x replays only root messages (`service.ts:131-150`) | replays `subagent_spawned`/`subagent_state_update` (`acp-agent.ts:7856,7970-7980,8238-8252`) | replays native history when gated (`CodexAcpServer.ts:2299`) |
| ACP SDK | `@agentclientprotocol/sdk` 1.6.0 | 1.7.0 exact (`package.json:68`) | `^1.5.0` → lock 1.5.0 (`package.json:69`) |

Implication for theuth: a single "child session" model must accept (a) an envelope-wrapped vendor notification (opencode), (b) RFD-draft `subagent_spawned` + child-`sessionId` traffic (claude/codex), and eventually (c) RFD v1 `subagent_update` + child traffic. In dialect (b) child permission requests arrive with an unknown `sessionId`, so theuth's permission handler must resolve child ids → parent.


## 8. ACP RFD v1 wire types (agent-client-protocol-schema 1.10.2)

Source: `/tmp/recon-141/schema-1.10.2/agent-client-protocol-schema-1.10.2/src/v1/client.rs` (crate extracted, not built). All of it is behind Cargo feature **`unstable_subagents`** (`Cargo.toml:79`; included in the umbrella `unstable` feature, `Cargo.toml:59-69`), every type is `#[non_exhaustive]` and doc-marked `**UNSTABLE**`. Wire casing: `SessionUpdate` is `#[serde(tag = "sessionUpdate", rename_all = "snake_case")]` (`client.rs:103`), structs are `rename_all = "camelCase"`. Patch fields use `MaybeUndefined<T>` (`src/serde_util.rs:578-586`: `Undefined` = key absent = unchanged, `Null` = clear, `Value`) and are `skip_serializing_if = "MaybeUndefined::is_undefined"`.

**Versions.** theuth today: `agent-client-protocol = "=2.2.0"` with `features = ["unstable_end_turn_token_usage"]` (`Cargo.toml:267`) → `agent-client-protocol-schema 1.9.1` (`Cargo.lock:364-365`; acp 2.2.0 pins `=1.9.1`). **1.9.1 has none of these types** (extracted `schema-1.9.1`: `grep unstable_subagents Cargo.toml` → not found; `grep -c SubagentUpdate src/v1/client.rs` → 0). Schema 1.10.2 (released 2026-10-01) is pinned by `agent-client-protocol` **3.0.0** (2026-10-06) and **3.1.0** (2026-10-07), both with feature `unstable_subagents = ["agent-client-protocol-schema/unstable_subagents"]` (their Cargo.toml:66,76). So using the Rust types requires bumping theuth to acp 3.x (major bump; migration notes in `/tmp/recon-141/sdk-docs/migration-v3.md` from an earlier recon).

**`SessionUpdate` variants** (`client.rs:182-198`): `SubagentUpdate(SubagentUpdate)` → `"subagent_update"`, `SessionMessage(SessionMessage)` → `"session_message"`, `SessionMessageChunk(SessionMessageChunk)` → `"session_message_chunk"`. They ride inside a normal `session/update` notification whose outer `sessionId` is the **parent**; test `client.rs:4375-4393`:
```json
{"sessionId": "parent", "update": {"sessionUpdate": "subagent_update", "sessionId": "child"}}
```

**`SubagentUpdate`** (`client.rs:948-1002`; Rust → JSON):
| Rust field | JSON | type | semantics |
|---|---|---|---|
| `session_id: SessionId` | `sessionId` | string, **required** | child's opaque session id, used in all later ACP messages about the child |
| `title: MaybeUndefined<String>` | `title` | string\|null | display title; omitted = unchanged, null = clear |
| `description: MaybeUndefined<String>` | `description` | string\|null | role/purpose text |
| `capabilities: MaybeUndefined<SubagentSessionCapabilities>` | `capabilities` | object\|null | client-initiated mutations allowed on the child; never supplied ⇒ none allowed |
| `state: MaybeUndefined<StateUpdate>` | `state` | object\|null | whole-object snapshot; null = activity unknown/unset (not idle) |
| `meta: MaybeUndefined<Meta>` | `_meta` | object\|null | |
Doc contract (`:922-940`): sent on the immediate parent; first update for an unknown `sessionId` announces the child and MUST precede any live child traffic; child events then arrive automatically on the same connection (no load/resume/subscribe). Builder: `SubagentUpdate::new(id).title(..).description(..).capabilities(..).state(..)`.

**`SubagentSessionCapabilities`** (`:1075-1091`): `cancel: Option<SessionCancelCapabilities>` → `"cancel": {}` means the client may `session/cancel` the child without ending it; `_meta`. **`SessionCancelCapabilities`** (`:1131-1142`): only `_meta`. Example from test `:4245-4254`: `{"sessionId":"sess_child_1","capabilities":{"cancel":{}}}`.

**`StateUpdate`** (`:1178-1195`, `#[serde(tag = "state", rename_all = "snake_case")]`):
- `Running(RunningStateUpdate)` → `{"state":"running", "_meta"?}` (`:1205-1213`)
- `Idle(IdleStateUpdate)` → `{"state":"idle", "stopReason"?: StopReason, "usage"?: Usage, "_meta"?}` (`:1239-1260`; `usage` only with feature `unstable_end_turn_token_usage`, `:1248`). `StopReason` (`src/v1/agent.rs:3175-3200`, snake_case): `end_turn | max_tokens | max_turn_requests | refusal | cancelled`.
- `RequiresAction(RequiresActionStateUpdate)` → `{"state":"requires_action"}` (`:1301-1309`)
- `Unknown(UnknownStateUpdate)` → `{"state":"unknown"}` (`:1347-1355`): activity unobservable; client must stop showing the previous state as confirmed.
- `Other(OtherStateUpdate)` (`#[serde(untagged)]`, `:1381-1387`): `{state: String, ..fields}` for `_`-prefixed vendor states; `KNOWN_STATE_UPDATE_STATES = ["running","idle","requires_action","unknown"]` (`:1425`).
Idle does **not** terminate the child; the parent may prompt it again (`:1168-1172`). There is no "completed"/"failed" state — termination is modelled as idle+stopReason, failure as idle with `stopReason` or vendor `_meta`.

**`SessionMessageChunk`** (`:213-235`): `messageId: MessageId` (required), `senderSessionId?: SessionId|null`, `recipientSessionId?: SessionId|null` (omission retains the known value), `content: ContentBlock` (required, one block appended), `_meta?`. **`SessionMessage`** (`:288-318`): `messageId` (required), `senderSessionId?`, `recipientSessionId?`, `content: MaybeUndefined<Vec<ContentBlock>>` (omitted = unchanged, null/[] = clear, non-empty = replace all), `_meta: MaybeUndefined<Meta>`. Both are emitted in the transcript of the session that sent/received (outer `sessionId`), identifying the other party via sender/recipient — this is how a parent's prompt to a child and the child's reply are meant to be shown.

**Capability gate** (`:3109-3113`, `:3300-3309`): `ClientCapabilities.subagents: Option<SubagentCapabilities>`; JSON `"subagents": {}` (object with optional `_meta`) advertises support; omitted/null = unsupported. Doc (`:3290-3293`): the client must advertise it before the agent sends subagent updates or session-directed messages. Agent side: no corresponding `AgentCapabilities` field in 1.10.2 (`grep -n subagents src/v1/agent.rs` → not found); support is discovered only by receiving `subagent_update`.

JS mirror: `@agentclientprotocol/sdk` 1.6.0 `schema/schema.json` carries identical `$defs` (`SubagentUpdate`, `SubagentCapabilities`, `SubagentSessionCapabilities`, `SessionMessage`, `SessionMessageChunk`, `ClientCapabilities.subagents`) — it is what opencode 2.0.22 bundles (§2).


## Gaps and recommendations

**Three incompatible subagent dialects exist today; none is the ratified RFD v1 wire.**
1. opencode 2.x: vendor extension `opencode/session/child_update` (§1) — gated by `_meta["opencode/child-session-updates"]`, status values `created|running|completed|failed|interrupted`, updates nested in `params.update`, child ids not ACP sessions.
2. claude-agent-acp / codex-acp: draft-RFD dialect `subagent_spawned` / `subagent_state_update` inside normal `session/update` (§7) — gated by `clientCapabilities.subagents` (any object) or AIR meta; child updates then arrive as ordinary `session/update` with `sessionId` = child id (an id theuth has never seen in `session/new`).
3. RFD v1 as shipped in schema 1.10.2 / acp 3.x: `subagent_update` (+ `session_message*`), `state` snapshots `running|idle|requires_action|unknown`, `capabilities.cancel` (§8). **No agent emits it yet** (opencode: §2; claude/codex: §7).

**Recommendations for design 141 (opencode scope):**
- Keep `_meta["opencode/child-session-updates"]: true` (`crates/acp/src/harness.rs:962-974`) and **stop flattening**: `child_update()` (`harness.rs:811-829`) currently drops every `type:"status"` event, drops the `rootSessionId/childSessionId/parentSessionId/depth/title` envelope, rewrites child `agent_message_chunk` → thought, and discards `session_info_update` (retry/compaction). Replace with a new `Update::Child { root, child, parent, depth, title, event }` carrying the envelope + the full `SessionUpdate`, routed to an observed child session keyed by `childSessionId`. Lifecycle mapping: `created`→spawned (title = subagent `description`), `running`→running, `completed`→done, `failed(error)`→failed with message, `interrupted`→cancelled. Child `toolCallId` is `"<child>:<id>"` — strip the prefix when storing under the child, keep it when correlating permissions (next bullet).
- **Child permissions already arrive on the root** with `toolCall.toolCallId = "<childSessionId>:<id>"` and title `"<childTitle>: …"` (§1). Parse the prefix to attribute the ask to the child session in the UI; the responder still answers on the root (no change needed in transport).
- **Correlate parent tool call ↔ child** via the parent's `tool_call_update completed` `rawOutput.metadata.sessionID` (2.x, `subagent.ts:264`) or `rawOutput.metadata.sessionId` (1.x, `task.ts:185-195`). In 2.x the `created` status (t+0.02 s after the in_progress update) precedes it; an interim heuristic is "the most recent in_progress `subagent` tool call without a child" — ordering in the capture supports it (tool in_progress t=3.328 → created t=3.346; second pair 3.721 → 3.741) but it is a race under true parallel spawning. Prefer the completed-metadata join for the durable record.
- **Cost/tokens for children are not on the wire in either version** (§5). Options: (a) show children without cost and label the root `usage_update.cost` as "root only"; (b) for 1.18.30 run `opencode acp --port <free>` and query `GET /session/<child>` for `cost`/`tokens` (unauthenticated loopback when `OPENCODE_SERVER_PASSWORD` unset); (c) read-only sqlite (`session_v2.cost/tokens_*` by `parent_id`) for 2.x — works offline/after reload, but polls a WAL DB another process writes. No double-counting risk until PR #43645 (or a successor) merges; gate any future summing on `agentInfo.version`.
- **1.18.30 cannot show live children over ACP at all** (§3): only the `task` tool_call lifecycle. For `opencode` (1.x) design 141 should degrade to "child placeholder from `rawInput.description`, resolved on completion with `rawOutput.metadata.sessionId/model`" — or use the HTTP route above for live detail. The 1.x child permission drop (`acp/permission.ts:53-54`) means a child that needs an interactive permission hangs under ACP; worth a note in docs.
- **Per-child cancel is impossible over ACP in both versions** (§4); only root cancel, which cascades to foreground children (status `interrupted`) and spares background ones. UI: no per-child stop button for opencode; root stop only.
- **RFD v1 adoption**: theuth is on acp 2.2.0 / schema 1.9.1 which lacks the types (§8). Not worth bumping to acp 3.x for 141 since no harness emits `subagent_update`; but design the internal child model (title/description, state enum including `requires_action`/`unknown`, per-child cancel capability flag, stopReason) so a later translation layer (`child_update` → RFD, `subagent_spawned` → RFD) is mechanical. If theuth later advertises `clientCapabilities.subagents: {}` to reach claude-agent-acp/codex-acp, it must also accept `session/update` for unknown `sessionId`s (child ids) — today `on_receive_notification` for `SessionNotification` (`harness.rs:366-379`) would forward them as the root's updates with no child attribution.

**Residual unknowns.**
- No permission ask fired in either live run (read tool auto-allowed; default model configs), so the `"<child>:<id>"` permission shape is shown from code + opencode's own tests (`permission.test.ts:130-133`), not from a capture. A follow-up capture with a write/bash subagent task would exercise it.
- 2.x grandchildren (`depth ≥ 2`) require `experimental.subagent_depth ≥ 2`; covered by opencode's tests (`turn.test.ts:149-194`), not captured live.
- Whether claude-agent-acp's `cost.amount = result.total_cost_usd` includes subagent spend is SDK-defined and not visible in the adapter repo `[INFERENCE]`.
- `opencode-main` here is the repo default branch (1.x line, 1.18.35); the 2.x line is branch `v2`. Both were checked for RFD paths; neither has them as of 2026-10-08.
