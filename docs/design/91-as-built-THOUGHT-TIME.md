# As-built — 91 THOUGHT-TIME — a finished thinking block shows how long the model thought

Branch `feat/91-thought-time` in `~/Develop/theuth-wt-91`, based on local main `afb89eda`. Brief: `/tmp/mc-2026-10-04/omp-THOUGHT-TIME.md`.

Commits (oldest first):
- `4639b692` feat(agent): measure and persist thinking time (91 THOUGHT-TIME)
- `ec483887` fix(agent): ReasoningDone.ms exports as a TS number; slack ignores the event
- `02ca4098` feat(desktop): finished thinking blocks read "Thought 9s" (91 THOUGHT-TIME)
- docs(design): 91 as-built THOUGHT-TIME (this note)

## The measurement

`ms` = wall clock from the first `ChatStreamEvent::ReasoningChunk` of a model call to the first `Chunk`, `ToolCallChunk` or `End` of the same call (`Instant::elapsed`, truncated to whole ms). `Start`, `Heartbeat` and `ThoughtSignatureChunk` neither start nor end it. A call with no `ReasoningChunk` yields `None`; a retried call measures its own attempt only (`call` resets the out-param per attempt). Published as `AgentEvent::ReasoningDone { session, turn, ms }` *before* the `TextDelta` / `ToolCallPending` of the event that ended it, when the call is live.

## What changed, per file

**Engine (`crates/agent`)**
- `src/lib.rs:1590-1594` `HistoryEntry::Reasoning { …, thinking_ms: Option<u64> }` (`#[serde(default)]`, ts `number | null`, optional). `:1723-1734` `AgentEvent::ReasoningDone { session, turn, ms: u64 }` (ts `number`). `:2088` session routing arm.
- `src/loop_.rs:3071` `stream_once` keeps `thought: Option<Instant>`; `:3118` first `ReasoningChunk` sets it; `:3085-3101` the first text/tool/End event takes it, writes `*thinking = Some(ms)` and publishes `ReasoningDone`. New out-param `thinking: &mut Option<u64>` on `stream_once` (`:3052`) and `call` (`:2938`; `#[allow(clippy::too_many_arguments)]`, reset to `None` per attempt at `:2958`). `round` (`:3871`) captures it and persists the reply row through `persist_timed` (`:4042` text reply, `:4082` tool-use row). `persist_timed` (`:2732-2750`) = `persist` via `Store::append_thinking`. Every other `call` site passes a throwaway `&mut thinking` / `&mut None` (summariser `:5025`, vision `:5459`, tests).
- `migrations/0032_thinking_ms.sql`: `ALTER TABLE messages ADD COLUMN thinking_ms INTEGER NULL`.
- `src/store.rs:364` `Row.thinking_ms: Option<i64>`; `:1237` `Store::append_thinking(id, &msg, Option<u64>)`; `insert_in` (`:2702-2708`) takes `thinking_ms: Option<i64>` and binds `?5`; the three row selects (`hidden` `:1505`, `rows_in` `:2778`, `RawRow` `:2653`) carry the column; `context_history` (`:2186-2189`) builds `ContextHistory.thinking: BTreeMap<i64, u64>` (seq → ms, absent when NULL).
- `src/context.rs:291-292` `ContextHistory.thinking` (one field; see Deviations).
- `src/supervisor.rs:6826-6829` `render(messages, entries, thinking)`; `:6869` fills `HistoryEntry::Reasoning.thinking_ms`. Both callers (`:5299` load, `:5395` paged history) pass `h.thinking`.
- `src/acp_child.rs:196-198` `Turn.thinking: Option<Instant>`; `translate` (`:1301-1318`, `:1388-1391`) sets it on `AgentThoughtChunk` and, on the first `AgentMessageChunk` / `ToolCall` / `ToolCallUpdate` after it, prepends `ReasoningDone`. No persistence for ACP rows (the reload falls back to no time).
- `crates/slack/src/channel.rs:2370` ignores `ReasoningDone` (exhaustive match; compiler-pointed).

**Tests (`crates/agent`)**
- `src/loop_/thinking_tests.rs`: the fake gained `text` (text reply after the thinking) and `silent` (no thinking) modes; the thinking always streams as two deltas. `Harness::live_call` runs `Turn::call` live with a bus `Sink` and returns the events, the measured ms, the turn and the store. New: `thinking_time_is_measured_published_and_persisted` (events `delta, delta, done, text`; `ReasoningDone.ms == thinking`; `persist_timed` → `context_history().thinking[0]` → `render` → `Reasoning { thinking_ms: Some(ms) }`), `thinking_time_ends_at_the_first_tool_call_chunk` (`done` before `ToolCallPending`), `no_reasoning_means_no_thinking_time` (no `ReasoningDelta`/`ReasoningDone`, `thinking == None`, map empty, only an `Assistant` entry).
- `src/store.rs` `thinking_time_round_trips`: `pragma_table_info('messages')` lists `thinking_ms`; `append_thinking(Some(9400))` / `append_thinking(None)` / `append` → `context_history().thinking == {1: 9400}`.
- `src/acp_child.rs` `message_chunks_accumulate_text`: the message chunk after a thought chunk yields `[ReasoningDone, TextDelta]`, the next one `[TextDelta]` only.
- `render` callers in tests (`supervisor.rs`, `acp_child.rs`, `loop_/regen_stamp_tests.rs`) pass an empty map.

**Desktop (`crates/desktop/ui`)**
- `src/store/sessions.ts:92-100` reasoning entry `{ kind, text, done, ms?, startedAt? }`; `:340` the reducer ctx is `{ sent, now: Date.now() }`.
- `src/store/reduce.ts:15` `Ctx = { sent; now }`; `:386` `fromHistory` reasoning → `ms: h.thinking_ms ?? undefined`; `closeTail` (`:475-489`) closes the open block on any non-delta event with `ms = e.ms` for `ReasoningDone`, else `max(0, ctx.now - startedAt)` (fallback), dropping `startedAt`; `ReasoningDelta` opening a block records `startedAt: ctx.now` (`:796`); `turn` has a `ReasoningDone` arm (`:1012-1015`) so the closed tail is kept.
- `src/lib/time.ts:62-71` `fmtThought(ms)`: `< 1000` → `undefined`; `< 60 s` → `9s` (nearest second); `< 1 h` → `1m 12s` / `2m`; else `1h 2m` / `1h`.
- `src/components/chat/MessageReasoning.tsx`: prop `ms?`; when `done`, `Thought` + `<span class="text-fg-tertiary">{fmtThought(ms)}</span>` when defined. Nothing else in the block changed. `Message.tsx:42` passes `ms={entry.ms}`.
- Tests: `src/lib/time.test.ts` (`fmtThought` table), `src/store/reduce.test.ts` (`ReasoningDone` closes with `ms: 9000` and a later `TextDelta` keeps it; fallback deltas at `now = 1000` + `TextDelta` at `10400` → `ms: 9400`; `fromHistory({ thinking_ms: 9000 })` → `ms: 9000`, without it no `ms`).

## Fallback rule (desktop)

The engine's `ReasoningDone.ms` always wins and is never overwritten. When the block is closed by anything else — an engine from before 91, an ACP harness whose child did not synthesise the event — `ms` is the client clock between the first delta and the closing event. A reloaded row without `thinking_ms` (pre-91 rows, ACP rows) shows plain "Thought".

## Replication check

`crates/agent/src/store/capture.rs:15-25` lists `("messages", "session_id")` in `ATTACHED`; `TableMeta.cols` (`:96-110`) is read from `PRAGMA table_info` at runtime (`store.rs:2565 table_meta`), and changesets carry every column in table order. The new column therefore replicates (67/68) with no capture/apply change; the migration runs on every engine before the table is attached.

## Bindings

`just ts-desktop` regenerates `/bindings` and `crates/desktop/ui/src/rpc/bindings`; both are gitignored (`.gitignore:4`, `crates/desktop/.gitignore:5`). No tracked binding file changed, so there is no `chore(rpc): regenerate bindings` commit. (`crates/agent/bindings/*.ts` is a stale, unmaintained artefact from 2026-09-26 and is not produced by the recipe.)

## Gates

1. `cargo test -p agent -p rpc --no-fail-fast` (worker, gate1.log; repl tests ran with the tree's own `monty` resolution — see the worker's note)
```
test result: ok. 379 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 26.47s
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 11.60s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
2. `cargo clippy -p agent -p rpc --all-targets --no-deps -- -D warnings`
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.49s
clippy EXIT=0
```
3. `cargo check -p theuth -p tui -p nodes` — first run failed (`E0004: AgentEvent::ReasoningDone { .. } not covered`, crates/tui/src/model.rs:5684); fixed by the orchestrator with an ignore arm (`fix(tui): ignore ReasoningDone`), then:
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.88s
EXIT=0
```
4. `just ts-desktop`
```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 4.42s
tsd EXIT=0
```
(no tracked binding changed)
5. `bun install && bun run check && CI=1 bun run test < /dev/null` (crates/desktop/ui)
```
$ tsc -b --noEmit
tsc EXIT=0
 Test Files  131 passed (131)
      Tests  730 passed (730)
vitest EXIT=0
```

## Deviations

- `crates/agent/src/context.rs` gained one field (`ContextHistory.thinking`) — not on the brief's file list, but it is where `entries` lives and the brief's suggested "`BTreeMap<i64, u64>` beside `entries`" has no other home. `crates/slack/src/channel.rs` got the compiler-pointed match arm.
- `fmtThought` lives in the existing `lib/time.ts` (next to `formatAt`/`gapLabel`) rather than a new `lib/duration.ts`.
- No `chore(rpc): regenerate bindings` commit: nothing tracked changed (see Bindings).
- The `stream_once`-level test exercises `Turn::call` live (events + measured ms) and `persist_timed` → store → `render`; `round`'s two `persist_timed` calls are not driven by a test (a full `round` needs the process-global tool registry harness).
