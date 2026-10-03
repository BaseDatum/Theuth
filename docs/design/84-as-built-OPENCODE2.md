# 84 — as built: OPENCODE2 (theuth drives opencode 2.0.22 and 1.18.30 as an ACP child)

Branch `feat/opencode2`, base 2cb6a14e. Number 84: 83 is taken on `origin/main`
by FX-EFFORT and FX-VOICE (`docs/design/83-as-built-FX-*.md`), as the brief
anticipated. Design text: `docs/design/06-acp.md` §8.4 (new).

## What changed

| file:line | change |
|---|---|
| `crates/acp/src/harness.rs:161-171` | `Connect.model/effort` docs: applied after `session/new` **and** `session/load`; effort best-effort. |
| `crates/acp/src/harness.rs:276-293` | `Turn` doc (usage semantics by version); `Harness.agent: Option<Implementation>`. |
| `crates/acp/src/harness.rs:340-349, 484-536` | `Opened {session, resumed, options, agent}` returned by `Link::open` (carries `initialize.agentInfo`). |
| `crates/acp/src/harness.rs:380-394` | `UntypedMessage` handler forwards `opencode/session/child_update` via `child_update` as `Update::Session`. |
| `crates/acp/src/harness.rs:572-666` | `connect`: logs `acp harness <name> <version>` once; model set when current differs (no `!resumed` guard); effort set when the option exists, lists the value and current differs — a miss is `warn!`, not a failed spawn; `agent_info()`, `usage_is_lower_bound()`. |
| `crates/acp/src/harness.rs:758-834` | `current_value`, `select_value`, pub `usage_is_lower_bound(agent)` (name contains `opencode`, major < 2), `CHILD_UPDATE_METHOD`, pub `child_update` (tool_call/tool_call_update kept with `<child>:<id>` ids and `_meta`; `agent_message_chunk` → `agent_thought_chunk`; `status`/other → `None`). |
| `crates/acp/src/harness.rs:958-974` | `initialize_request`: `clientCapabilities._meta["opencode/child-session-updates"] = true`; still no `elicitation`. |
| `crates/acp/src/agents.rs:53-66` | `Entry::config` sets `OPENCODE_PRINT_LOGS=1` (entry `env` overrides) — stderr already lands in `logs/acp-<name>.log`. |
| `crates/agent/src/acp_child.rs:975-1019` | `finish`: `record_remote_turn(…, lower_bound)` from `Harness::usage_is_lower_bound`. |
| `crates/agent/src/acp_child.rs:1270-1276` | `tokens_of` doc: 2.x per-turn sum, 1.x last request. |
| `crates/agent/src/acp_child.rs:1655-1664` | mailbox arm: `session_info_update._meta` keys `opencode/*` logged at `debug` (no event slot). |
| `crates/agent/migrations/0031_remote_turns_lower_bound.sql` | `remote_turns.lower_bound INTEGER NOT NULL DEFAULT 0`, back-filled `1` for `harness = 'opencode'` rows (all pre-2.x). |
| `crates/agent/src/store/turn_usage.rs` | `Sums.lower_bound` (`MAX(lower_bound)` per group), `by_model` ORs it, `record_remote_turn(.., lower_bound)`. |
| `crates/agent/src/cost.rs:277-294` | `RemoteTokens.lower_bound: bool` (TS binding regenerated). |
| `crates/tui/src/model.rs:10939-10951` | ` · last request only` keyed on `r.lower_bound`, not the entry name. |
| `crates/desktop/ui/src/components/chat/CostPill.tsx:42-70`, `CostPill.test.ts` | `(last request only)` keyed on `r.lower_bound`. |
| `crates/agent/src/supervisor.rs:889-893` | `set_acp(id, &name, Some(&cwd_path), ..)`: the resolved spawn cwd is stored so a resume passes the session's own directory (opencode 2 requires `cwd == session dir` on load). |
| `crates/agent/src/peers.rs:50` | `spawn_subagent.effort` description: applied when offered, else the harness default (no longer "the spawn fails"). |
| `crates/theuth/src/main.rs:1127` | `DEFAULT_AGENTS` opencode description line mentions 1.18/2.x and `set_config_option`. |
| `crates/acp/fixtures/opencode-{2.0.22,1.18.30}/*.json` | recorded frames (below). |
| `docs/design/06-acp.md` §8.4 | the opencode 2 contract. |

Not changed: `~/.theuth/*`, `/opt/homebrew/bin/opencode`, `~/.config/opencode/*`
(read by opencode only), `SetEffort`/`SetModel`/default-model code, voice, compaction/meeting.

## Behaviours and their tests

| behaviour | test |
|---|---|
| `agentInfo` of both versions → 2.x per-turn usage, 1.x lower bound; `session/new` options (`model`+`mode` on both, `effort` on 2.x from the start, on 1.x only after the model has variants); model resolves by id; `effort: high` resolves after the switch on both; theuth's `medium` is a refusal naming the list | `acp::harness::tests::opencode_fixtures_initialize_and_config_options` |
| `opencode/session/child_update` → `SessionUpdate` with `<child>:<id>` id and `_meta`; completed update carries `locations`; child prose → thought; `status`/other methods → none | `acp::harness::tests::child_update_fixtures_become_session_updates` |
| 2.x permission ask with `diff` content + `locations` + prefixed id; option ids `once/always/reject` on both | `acp::harness::tests::permission_fixtures_deserialize_with_diff_and_locations` |
| recorded `PromptResponse.usage` sums to `totalTokens` on both; `usage_update {used,size,cost}` | `acp::harness::tests::usage_fixtures_deserialize` |
| the recorded usages land as `TurnFinished.usage` and one `remote_turns` row; only the 1.x row is `lower_bound`; no `usage_update` → `context 0`, `context_size None`, ledger row intact | `agent::acp_child::tests::opencode_usage_is_flagged_lower_bound_by_version` |
| a sub-agent call id with `:` pairs start/finish, persists as tool_use/tool rows, renders in `history()` | `agent::acp_child::tests::child_session_call_ids_with_a_colon_persist_and_pair` |
| `initialize` advertises `_meta["opencode/child-session-updates"]` and no `elicitation` | asserted in every Rig handshake (`Fake::handshake`) |
| `lower_bound` groups through `session_cost` | `agent::store::turn_usage::tests::remote_turns_sum_per_model_and_cascade` |
| CostPill marks `lower_bound` rows only | `CostPill.test.ts` |

## Live smoke (scratch, 2026-10-03)

`t` built from this worktree (`cargo build -p theuth`, debug), `t --data-dir /tmp/oc2-smoke --headless`
with `env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH`, `ANTHROPIC_API_KEY` from `~/.zshrc` sourced in the
launching shell (never printed), root model `anthropic/claude-haiku-4-5`, `acp/agents.yaml` entries
`opencode2` → `/tmp/opencode2/opencode acp` (downloaded zip, not installed) and `opencode` →
`/opt/homebrew/bin/opencode acp`, each through a `tee` wrapper capturing raw frames
(`/tmp/oc2-smoke/frames/run*/oc{1,2}-{in,out}.jsonl`). A root session was prompted to call
`spawn_subagent{agent, cwd: /tmp/oc2-smoke/work, model: anthropic/claude-haiku-4-5, effort: high, task}`.
Sky's V1 `~/.config/opencode/opencode.json` loaded on 2.0.22 without complaint (its `qartez` MCP and
providers were active in the child; its `plugin` entry does not run on V2).

Results (both versions): `agentInfo` `OpenCode 2.0.22` / `OpenCode 1.18.30` (engine log
`acp harness OpenCode 2.0.22 session=ses_…`); `HarnessModelChanged → anthropic/claude-haiku-4-5`;
`harness_options` → `[{model: anthropic/claude-haiku-4-5}, {effort: high}, {mode: build}]`;
`ContextUsed {context, size: 200000}`; `TurnFinished end_turn` with usage 2.x `{prompt 22040,
completion 391, cached 22020}` / 1.x `{prompt 38635, completion 44, cached 38501}`; `remote_turns`
rows `opencode2 … lower_bound 0`, `opencode … lower_bound 1`; `session_cost.direct.returned`
0.005063 USD (2.x) / 0.05288585 USD (1.x). Permission round-trip for `ls` on both
(`ApprovalRequested {tool: "ls", risk: mutating}` → `resolve_approval allow` → `ToolCallFinished`);
2.x's default agent allows `bash` silently, so the scratch wrapper set
`OPENCODE_CONFIG_CONTENT='{"permission":{"bash":"ask","edit":"ask"}}'`. The 2.x child's reply text was
polluted by the qartez MCP's "Code Mode catalog" instructions (Sky's config), 1.x replied `OK-OPENCODE`.

Sub-agent run (2.0.22, rebuilt `t` with `child_update`): `ToolCallStarted subagent`, then
`ToolCallStarted ses_f0035eeb…:toolu_01T9RN… "Create hello.txt file: write"`, `ApprovalRequested
"Create hello.txt file: /tmp/oc2-smoke/work/hello.txt"` (diff + locations on the wire) → allow →
`ToolCallFinished … "Created file successfully: hello.txt"`, `ToolCallFinished subagent`, stored rows
`user, assistant(tool_use subagent), assistant(tool_use ses_…:toolu_…), tool(ses_…:toolu_…), tool(subagent), assistant`;
the sub-agent's prose arrived as `ReasoningDelta`. `hello.txt` exists.

### Raw frames (trimmed; option lists collapsed to `{id, category, currentValue, n}`)

```
===== 2.0.22
→ initialize {"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"auth":{"terminal":true},"_meta":{"opencode/child-session-updates":true}},"clientInfo":{"name":"theuth","version":"2.4.0"}}
← {"protocolVersion":1,"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":true,"sse":false},"promptCapabilities":{"embeddedContext":true,"image":true},"sessionCapabilities":{"additionalDirectories":{},"close":{},"delete":{},"fork":{},"list":{},"resume":{}},"_meta":{"opencode/child-session-updates":true}},"authMethods":[{"id":"opencode-login",…}],"agentInfo":{"name":"OpenCode","version":"2.0.22"}}
→ session/new {"cwd":"/tmp/oc2-smoke/work","mcpServers":[{"name":"theuth","command":".../target/debug/t","args":["--data-dir","/tmp/oc2-smoke","mcp","--session","…"],"env":[]}]}
← {"sessionId":"ses_f0042766affezx4TaZ2PyOLYNm","configOptions":[{"id":"model","category":"model","currentValue":"github-copilot/gpt-6.1-sol","n":307},{"id":"effort","category":"thought_level","currentValue":"default","n":6},{"id":"mode","category":"mode","currentValue":"build","n":2}]}
→ session/set_config_option {"sessionId":"ses_f0042766…","configId":"model","value":"anthropic/claude-haiku-4-5"}
← {"configOptions":[{"id":"model",…,"currentValue":"anthropic/claude-haiku-4-5"},{"id":"effort",…,"currentValue":"default","n":3 /* high, max, default */},{"id":"mode",…}]}
→ session/set_config_option {"sessionId":"ses_f0042766…","configId":"effort","value":"high"}
← {"configOptions":[{"id":"model",…,"currentValue":"anthropic/claude-haiku-4-5"},{"id":"effort",…,"currentValue":"high"},{"id":"mode",…,"currentValue":"build"}]}
← session/update config_option_update (×2, model = anthropic/claude-haiku-4-5)
→ session/prompt {"sessionId":"ses_f0042766…","prompt":[{"type":"text","text":"Run the shell command `ls` in your working directory, then reply with exactly: OK-OPENCODE2"}]}
← session/request_permission {"sessionId":"ses_f0042766…","toolCall":{"toolCallId":"toolu_018z7FFkxgjjPN1mryBDydSm","title":"ls","kind":"execute","status":"pending","locations":[{"path":"/tmp/oc2-smoke/work"}],"rawInput":{"command":"ls","cwd":"/tmp/oc2-smoke/work"}},"options":[{"optionId":"once","kind":"allow_once"},{"optionId":"always","kind":"allow_always"},{"optionId":"reject","kind":"reject_once"}]}
→ {"outcome":{"outcome":"selected","optionId":"once"}}
← session/update usage_update {"used":12672,"size":200000,"cost":{"amount":0.005063,"currency":"USD"}}
← {"stopReason":"end_turn","usage":{"inputTokens":20,"outputTokens":213,"totalTokens":22431,"thoughtTokens":178,"cachedReadTokens":22020},"_meta":{}}
--- sub-agent run
← opencode/session/child_update {"rootSessionId":"ses_f00418429…","childSessionId":"ses_f00417af0…","parentSessionId":"ses_f00418429…","depth":1,"title":"Create hello.txt file","type":"update","update":{"sessionUpdate":"tool_call","toolCallId":"ses_f00417af0ffewZH4KxaHpEZ653:toolu_0179KkBvErCkm8J5u2hVjtLG","title":"Create hello.txt file: write","kind":"edit","status":"pending","locations":[],"rawInput":{},"_meta":{"opencode/child-session":{"id":"ses_f00417af0…","parentID":"ses_f00418429…","depth":1,"title":"Create hello.txt file"}}}}
← session/request_permission {"sessionId":"ses_f00418429…","toolCall":{"toolCallId":"ses_f00417af0ffewZH4KxaHpEZ653:toolu_0179KkBvErCkm8J5u2hVjtLG","title":"Create hello.txt file: /tmp/oc2-smoke/work/hello.txt","kind":"edit","status":"pending","locations":[{"path":"/tmp/oc2-smoke/work/hello.txt"}],"rawInput":{…},"content":[{"type":"diff","path":"/tmp/oc2-smoke/work/hello.txt","oldText":null,"newText":"hi"}]},"options":[…once/always/reject…]}
← {"stopReason":"end_turn","usage":{"inputTokens":20,"outputTokens":313,"totalTokens":22800,"thoughtTokens":214,"cachedReadTokens":18839,"cachedWriteTokens":3414},"_meta":{}}

===== 1.18.30
→ initialize (same)
← {"protocolVersion":1,"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":true,"sse":true},"promptCapabilities":{"embeddedContext":true,"image":true},"sessionCapabilities":{"close":{},"fork":{},"list":{},"resume":{}}},"authMethods":[{"id":"opencode-login",…}],"agentInfo":{"name":"OpenCode","version":"1.18.30"}}
→ session/new (same shape)
← {"sessionId":"ses_f00421da6ffeZb0nAVQXIZNWrd","configOptions":[{"id":"model","category":"model","currentValue":"opencode/big-pickle","n":266},{"id":"mode","category":"mode","currentValue":"build","n":4}]}
→ session/set_config_option {"configId":"model","value":"anthropic/claude-haiku-4-5"}
← {"configOptions":[{"id":"model",…,"currentValue":"anthropic/claude-haiku-4-5"},{"id":"effort","category":"thought_level","currentValue":"high","n":2 /* high, max */},{"id":"mode",…}]}
   (no effort request: current already `high`)
→ session/prompt {… "OK-OPENCODE"}
← session/request_permission {"toolCall":{"toolCallId":"toolu_01RKAp3qHeCqTGk4SjZh64Hn","title":"ls","kind":"execute","status":"pending","locations":[],"rawInput":{"command":"ls"}},"options":[…once/always/reject…]}
→ {"outcome":{"outcome":"selected","optionId":"once"}}
← session/update usage_update {"used":38635,"size":200000,"cost":{"amount":0.05288585,"currency":"USD"}}
← {"stopReason":"end_turn","usage":{"inputTokens":12,"outputTokens":10,"totalTokens":38679,"thoughtTokens":34,"cachedReadTokens":38501,"cachedWriteTokens":122},"_meta":{}}
```

## Gates (verbatim counts)

- `cargo test -p acp -p agent`: acp `23 passed; 0 failed`; agent `360 passed; 0 failed`; doc-tests 0.
- `cargo clippy -p acp -p agent --all-targets --no-deps -- -D warnings`: clean (no output).
- rustfmt on own hunks: `agents.rs`, `harness.rs`, `cost.rs`, `peers.rs` fmt-clean; `acp_child.rs`,
  `supervisor.rs`, `tui/model.rs`, `theuth/main.rs` keep exactly their base drift count (4/4/7/5) —
  no unrelated lines reformatted; `turn_usage.rs` keeps one base-drift line.
- `cargo check -p tui -p theuth`: clean. `cargo test -p rpc --features ts export_bindings`: 8 passed
  (bindings regenerated: `RemoteTokens.lower_bound: boolean`).
- `crates/desktop/ui`: `bun run check` (tsc) clean; `CI=1 bun run test`: `128 files, 708 tests passed`.

## Deviations

- Outside the named seams, smallest possible: `crates/agent/src/supervisor.rs:889-893` (store the
  resolved cwd; one argument), `crates/agent/src/peers.rs:50` (effort description sentence),
  `crates/tui/src/model.rs:10939-10951` and `crates/desktop/ui/src/components/chat/CostPill.tsx:42-70`
  + `CostPill.test.ts` (label keyed on the new `lower_bound`), `crates/theuth/src/main.rs:1127`
  (template line, as the brief allows), `crates/agent/migrations/0031_*.sql`, `cost.rs:277-294`.
- The brief described sub-agent updates as `session/update`s with `_meta` and prefixed ids; on the
  wire 2.0.22 sends them on the extension method `opencode/session/child_update` (only the permission
  ask is a normal `session/request_permission`). `acp::child_update` bridges them; the sub-agent's
  `agent_message_chunk` is forwarded as thought so it does not become the turn's result text.
- Effort: 75 §8's "unlisted value fails the spawn" is replaced by the brief's "match when listed, else
  default and log" (`harness.rs:618-629`). The `spawn_subagent` description was updated to match.
- The effort's `currentValue` is not persisted on the session row (no column exists); it is live via
  `harness_options` and refreshed by `config_option_update`. The model is persisted as before.
- The 2.x child's reply in the smoke was not the requested `OK-OPENCODE2` because Sky's `qartez` MCP
  (from `~/.config/opencode/opencode.json`) injects a catalog notice the model answered instead;
  protocol-level results were unaffected. 1.x replied `OK-OPENCODE`.
- The scratch smoke's first three runs used `t` built before `child_update` existed; the sub-agent
  run was repeated on the rebuilt binary (scratch `sessions.sqlite` was recreated for it because the
  0031 migration comment changed between builds).

## Open follow-ups

- `session_info_update._meta["opencode/retry" | "opencode/compaction"]` is only `debug`-logged; add a
  harness-note event when one exists (also `compaction_update`, post-2.0.22).
- Sub-agent `type: "status"` events (`created/running/completed/failed`) are dropped; a nested
  card/heading per child session would use them.
- `Loaded`/`SessionSummary` carry no effort; if the UI wants `harness · model · effort`, persist the
  thought-level current value beside `harness_model`.
- A resume of a child whose row predates this change (cwd NULL) still falls back to the engine cwd;
  opencode 2 will refuse `session/load` there and theuth starts a fresh session (existing fallback).
