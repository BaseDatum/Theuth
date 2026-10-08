# 141 hand-off — W5 (OMP)

Branch `feat/141-w5`, base = contract `0b5f8aa2` (merged `feat/141-acp-subagents`
again before the gate — "Already up to date" — and before DONE, merge `ada7378d`, which also brought the integration tree's W3 files; gate re-run green after it).
Files: `crates/acp/src/omp/{mod,progress,tail}.rs` (C's single-file stub split
per §7.1's module list; every public path `acp::omp::*` is unchanged),
`crates/acp/fixtures/omp-18.3.1/**`, this file. Nothing outside the brief's list.

## 1. Fixture inventory (`crates/acp/fixtures/omp-18.3.1/`)

All recorded 2026-10-08 from omp 18.3.1 (`captures/omp-18.3.1-task.jsonl`,
`captures/omp-session-dir/`); prose fields (`context`, `tasks[].task`,
`task`, `assignment`, `content[].text`, `systemPrompt`, `title.pad`,
`requestControls`, the async-result `content`) trimmed to ≤60 chars. Frame
files are the full `session/update` notification like the opencode fixtures.

| file | source | what it pins |
|---|---|---|
| `task.tool_call.json` | capture line 8 | the spawn `tool_call` (`rawInput.tasks[]`, no `rawOutput`) → **not** a task frame |
| `task.completed_stale.json` | line 12 (t=4951) | `status: completed` acknowledgement, `details.async{state:running}`, two `pending` rows with zero counters (F18) |
| `task.mid.json` | line 27 (t=7500) | `in_progress` after the completed; rows `running`, `resolvedModel`, `recentTools[read]`, `lastIntent` |
| `task.final.json` | line 44 (t=9781) | `in_progress`, rows `completed`, cost 0.12128025 / 0.12170525, `extractedToolData.yield[0].data`, `async.state: completed` |
| `ReadA.jsonl` | `…/ReadA.jsonl` (12 entries) | the child transcript: title, session, model_change, thinking_level_change, session_init, user, assistant(toolCall read), tool_execution_start, toolResult, assistant(toolCall yield), tool_execution_start, toolResult |
| `ReadA.md`, `ReadA.json` | sidecars | completion + structured output |
| `parent.jsonl` | parent entries 2 (session) and 15 (`custom_message async-result`, jobs[].jobId = ReadA) | settlement |

Tests build a throwaway `~` (`$TMPDIR/acp-omp-<uuid>/.omp/agent/sessions/<encoded-cwd>/<ts>_<sid>{.jsonl,/}`)
from these and remove it on drop.

## 2. Feed 1 — `progress_rows` / `Progress::announce` (`progress.rs`)

- Recogniser is structural: `ToolCall` or `ToolCallUpdate` whose
  `rawOutput.details.progress` deserialises as `Vec<Progress>` (string `id` +
  `status` required, everything else optional). The call's own `status` is
  never read. `background = details.async is object`, `batch_state =
  details.async.state`.
- **Stale `completed` rule as implemented**: frames are full snapshots applied
  in wire order; the `completed` frame's rows announce `Pending` (not
  `Completed` — the call status is not a child state) and the next
  `in_progress` snapshot overwrites them through the store's upsert. No
  out-of-order detection is needed because the transport is ordered
  (`latest_snapshot_wins_per_id` folds stale → mid → final).
- `announce` fills: `parent` (`a.b` → `a`), `title` = id, `tool_call`,
  `model` = `resolvedModel`, `description` = `assignment` else `task` head
  (200 chars), `cost_rule` = `Counted` when background else `InsideParent`,
  `state` pending|running|completed|failed{retryFailure.errorMessage}|aborted→Cancelled|other→Unknown.
- `detail` keys (wire names; a key is present only when omp sent it, so the
  store's shallow merge leaves older values alone): `index, agent, agentSource,
  modelRole, tokens, cost, contextTokens, contextWindow, requests, toolCount,
  durationMs, lastIntent, currentTool, currentToolArgs, currentToolIntent,
  completionPercent, recentTools[≤5] ({tool, args, endMs?, intent?, isError?}),
  recentOutput[≤8], retry (retryFailure else retryState), result
  (`extractedToolData.yield[0].data`), batch (`details.async.state`)`.
  Q9 (18.8.4): `completionPercent`, `currentToolIntent`, `recentTools[].isError`
  / `.intent` added to `Progress`/`RecentTool` and surfaced when present.
- `TaskFrame.batch_state` is also copied into every row's `detail.batch` so
  the roster footer (§9.3) has it without a parent-side field.

## 3. Feed 2 — `Tailer` / `Entry` / `to_updates` (`tail.rs`)

- `locate(home, sid)`: `read_dir(<home>/.omp/agent/sessions)` × `read_dir(<cwd>)`,
  first file whose name ends with `_<sid>.jsonl`; `artifacts` = that path
  minus `.jsonl` (`strip_suffix`, not `with_extension`, so a dotted cwd
  encoding cannot break it). `None` when the dir or file is absent.
- `child_path("a.b")` → `<artifacts>/a/a.b.jsonl`; sidecars are `child_path(id).with_extension("md"|"json")`.
- `read(id, cursor)`: open, `len`; **`cursor > len` → `cursor = 0`** (Q6);
  seek, read to end, keep bytes up to the last `\n`; new cursor = old +
  consumed. Missing file → `(vec![], cursor)`. Unparseable complete lines are
  skipped but consumed.
- `sidecar_done(id)` = `<id>.md` is a file. Extras not in the stub (3-liners;
  W2 does not call them, the Closer may wire §6.4's ".md text → final
  assistant message" / ".json → observed_detail.result"): `sidecar_text(id)
  -> Option<String>`, `result_json(id) -> Option<Value>`.
- `settled_jobs(parent_cursor)`: same reader on `parent_file`; entries with
  `type == custom_message && customType == async-result` →
  `details.jobs[].jobId`.

`Entry` ← jsonl line:

| jsonl | `Entry` |
|---|---|
| `type: session` | `Session { id, cwd, parent_session }` |
| `type: model_change` | `Model(model)` |
| `type: custom, customType: tool_execution_start` | `ToolStart(Call { id: data.toolCallId, name: data.toolName, arguments: data.args as JSON text, intent })` |
| `message.role: user` | `User(text parts joined)` |
| `message.role: assistant` | `Assistant { text, calls: toolCall parts (arguments as JSON text, intent), usage {input, output, cacheRead, cacheWrite, totalTokens, cost.total}, model: "provider/model" }` |
| `message.role: toolResult` | `ToolResult { call_id, name: toolName, text, is_error }` |
| `title`, `session_init`, `thinking_level_change`, other | `Other` |

`Entry` → `SessionUpdate` (`to_updates`):

| `Entry` | `SessionUpdate` |
|---|---|
| `Assistant.text` (non-empty) | `agent_message_chunk {text}` |
| each `Assistant.calls[]` | `tool_call {toolCallId: id, title: name, kind: kind_of(name), status: in_progress, rawInput: arguments}` — `translate()` reads `title` as the card name and `rawInput` as args |
| `ToolResult` | `tool_call_update {toolCallId, status: completed \| failed (isError), content: [text]}` |
| `User`, `Session`, `Model`, `ToolStart`, `Other` | nothing |

**Deviation from §6.4** ("`custom tool_execution_start` → `ToolCallStarted`
if not yet seen"): `ToolStart` produces nothing. omp always persists the
assistant `toolCall` part before the start marker (`ReadA.jsonl` 7→8, 10→11),
and a stateless mapper cannot know whether the marker's call was already
emitted in an earlier cursor batch — emitting it would duplicate the card
whenever a batch boundary fell between the two lines. The `Entry::ToolStart`
is still parsed (carries `intent`) for anyone who wants it.

`kind_of`: read→Read; grep/glob/web_search→Search; edit/write/ast_edit→Edit;
bash/eval→Execute; web_fetch→Fetch; else Other (drives `acp::risk`).

## 4. Assumptions about other waves

- W2 applies frames in wire order through `Store::upsert_observed` (latest
  wins there; no reordering), opens the child turn itself (`User` entries are
  not replayed), persists cursors after each applied batch, and treats a
  cursor returned **smaller** than the one passed as the Q6 reset (text rows
  may duplicate; tool rows dedupe by call id in `Turn::call_started`).
- W2 keeps processing `tool_call_update` frames whose call already reported
  `completed` (F18); `progress_rows` accepts them, nothing here filters.
- `Tailer::locate` is retried by W2 on the first task frame when `Connected`
  found nothing (omp writes the parent file at `session/new`, so it is
  normally there).
- `home` is the engine's home (`dirs::home_dir()` as `platform::service`
  does); feed 2 only for `Transport::Process`.

## 5. Gate output

`BUILD_LABEL=141-W5 build-lock cargo test -p acp -- omp::` (stable toolchain,
`CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=$PWD/target`):

```
running 10 tests
test omp::progress::tests::nested_ids_name_their_parent ... ok
test omp::progress::tests::other_frames_are_not_task_frames ... ok
test omp::progress::tests::sync_spawns_are_inside_parent_and_states_map ... ok
test omp::progress::tests::newer_fields_surface_when_present_and_rosters_are_capped ... ok
test omp::progress::tests::task_frames_are_recognised_regardless_of_call_status ... ok
test omp::progress::tests::latest_snapshot_wins_per_id ... ok
test omp::tail::tests::locate_finds_the_parent_file_and_names_child_paths ... ok
test omp::tail::tests::read_a_transcript_replays_as_session_updates ... ok
test omp::tail::tests::cursor_stops_at_a_partial_line_and_resets_on_truncation ... ok
test omp::tail::tests::sidecar_and_async_result_settle_children ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 23 filtered out
```

`build-lock cargo clippy -p acp --tests` → `Finished`, no warnings.

Machine notes: `./target/.ready` never appeared (20 min wait, 25→31 G
cloned, then no writes); built on the partial clone after `cargo clean -p
indexmap` fixed one "crate `indexmap` required in rlib format but not found"
left by the incomplete copy. No other crate was cleaned.

## 6. For the Closer

- Wire `Tailer::sidecar_text` / `result_json` into the `.md`-appears and
  `.json`-appears rows of §6.4 if W2 did not (W2 built against C's stub,
  which had neither).
- The W2 test "one test with a real omp frame" can use
  `fixtures/omp-18.3.1/task.{completed_stale,mid,final}.json` via
  `progress_rows` — two children, final states `Completed`, costs as in §13.2.
