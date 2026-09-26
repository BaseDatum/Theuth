# 70 — as built: THINKING-TRACES

Branch `fix/thinking-traces`. "Why does omp acp show me thinking traces on
Fable 5.1, but my theuth doesn't?"

## Root cause

theuth never asked Anthropic to think. With `agent.effort: ~` (the user's
config) and no `/effort`, the chain is:

| step | evidence |
|---|---|
| `AgentConfig::default().effort = None` → `sessions.effort` NULL (27 of the user's Fable sessions, all NULL) | `crates/agent/src/lib.rs:2003`; `sessions.sqlite` |
| `Current.effort: None` | `crates/agent/src/loop_.rs:641` |
| `Frame::request` adds `with_reasoning_effort` **only** `if let Some(e) = effort` | `crates/agent/src/frame.rs:206-208` (pre-fix) |
| genai's Anthropic adapter: `reasoning_effort() == None` → `from_model_name` (suffix only) → `None` → `insert_anthropic_reasoning` never called → **no `thinking` key in the body** | `~/.cargo/git/checkouts/rust-genai-…/a08e55d/src/adapter/adapters/anthropic/adapter_shared.rs:486-537` |
| Anthropic leaves extended thinking off unless the request enables it (Fable/Opus 4.x/Sonnet 4.x/Haiku; genai's `thinking_enabled_by_default` is true only for Opus/Sonnet 5.x) | `ant_model.rs:170-178` |

omp, by contrast, runs the user's `default: anthropic/claude-fable-5-1:high`
(`~/.omp/agent/config.yml`) and its Anthropic request builder emits
`thinking: {type: "adaptive", display: "summarized"}` + `output_config.effort`
whenever the model has `thinking.mode: anthropic-adaptive` (omp binary,
strings around `const ve = { type: "adaptive" }`; models.db `claude-fable-5-1`
→ `anthropic-adaptive`, efforts low…max).

Everything downstream already worked: genai's streamer → `ChatStreamEvent::
ReasoningChunk` → `AgentEvent::ReasoningDelta` (loop_.rs:2862) → RPC/ACP →
desktop `reduce.ts` `reasoning` entry → `MessageReasoning`; TUI
`Entry::Reasoning`. It was never fed.

Secondary gap found while smoking: reasoning was **live-only**. `HistoryEntry`
had no reasoning variant, so the desktop's post-turn `load_session` (and any
reload) dropped the block the stream had just shown.

## Fix

### `crates/agent/src/frame.rs` — `default_effort(&Resolved) -> Option<ReasoningEffort>`
`Some(Medium)` when the genai namespace is `anthropic`, or `bedrock_sigv4`
with an `anthropic.` publisher segment, **and** the catalog model has
`reasoning: true`. Everything else → `None` (provider default, nothing sent).
Applied in `stream_once` (`loop_.rs`): `current.effort.clone().or_else(||
default_effort(&current.model))`. Only main rounds: side calls (title,
vision) still pass `None`. `Medium` becomes, via genai: `thinking:
{adaptive, display: summarized}` + `output_config.effort: medium` on
adaptive models (Fable, Opus ≥ 4.6, Sonnet ≥ 4.6), `thinking: {enabled,
budget_tokens: 8000}` on budget-only models (Sonnet 4.5, Haiku 4.5) and on
Bedrock. Session `/effort` and `agent.effort` still win; `off` → `Zero` →
no `thinking` key (or `disabled` on default-on models).

### `crates/models/src/lib.rs` — `Model::efforts()` prepends `off` for every steerable model
Fable's catalog entry has no `toggle`, so `/effort off` was rejected by
`validate_effort`; with thinking on by default it must be undoable.

### `HistoryEntry::Reasoning { text, turn, context }` (`crates/agent/src/lib.rs`)
`supervisor::render` emits it from the stored assistant row's
`ReasoningContent` parts (the loop already persisted them — `captured_content`
carries `ThoughtSignature` + `ReasoningContent` before text/tool calls).
Consumers: desktop `fromHistory` → `{kind: reasoning, done: true}`; TUI
`from_history` → `Entry::Reasoning { done: true }`; `print_history` → dim
`💭` line; ACP replay → `AgentThoughtChunk`; `session_read` skips it.
Bindings regenerated (`just ts-desktop`).

### Desktop `MessageReasoning.tsx`
Collapsed by default, chevron always; while streaming: pulse + `Thinking…` +
the latest line as a truncated tail; done: `Thought`. Click expands the full
text (streams live when open).

### TUI `-p` print mode (`crates/tui/src/lib.rs`)
`ReasoningDelta` streams dim under a `💭` header; any other event closes it.

### Cost
No change needed: Anthropic's `output_tokens` includes thinking; genai maps
it to `completion_tokens`, which `observe()` bills as output
(`loop_.rs:2120`). Asserted in the round-trip test (`completion_tokens ==
57` with a thinking block in the fake reply).

## Tests

- `crates/agent/src/loop_/thinking_tests.rs` (wiremock `/v1/messages`,
  bodies captured):
  - `default_effort_is_medium_for_anthropic_thinking_models_only` — 7
    Anthropic-direct/Bedrock ids → `Medium`; `openai/gpt-5.6-sol`,
    `amazon-bedrock/amazon.nova-pro-v1:0`, `amazon-bedrock/us.openai.gpt-6-astra`,
    `google/gemini-2.5-pro`, `reasoning: false`, `model: None` → `None`; a
    non-Anthropic frame carries no `reasoning_effort`.
  - `anthropic_request_carries_thinking_per_effort` — Fable: None →
    `{adaptive, summarized}` + `effort: medium`; Zero → no `thinking`, no
    `output_config`; High → `effort: high`. Sonnet 4.5: None → `{enabled,
    8000}`; Budget(4096) → `{enabled, 4096}`; Zero → nothing.
  - `thinking_block_round_trips_with_signature_on_tool_use_turn` — the fake
    returns a signed thinking block + `tool_use`; round 2's body replays
    `{type: thinking, thinking, signature}` before `tool_use`;
    `supervisor::render` of the stored row yields `Reasoning` then `Tool`.
- `crates/models`: `efforts()` tests updated for the leading `off`.
- `crates/desktop/ui/src/store/reduce.test.ts`: `ReasoningDelta` → one
  streaming entry, closed by `TextDelta`, a later delta opens a new entry;
  `fromHistory(reasoning)` → done block.

## Verification

- `cargo test -p agent --lib`: 319 passed (before the history change), 3/3
  thinking tests after; `cargo test -p models`: 16 passed; `cargo test -p tui
  --lib`: 106 passed; `cargo test -p rpc --features ts export_bindings`: 5
  passed (bindings regenerated, `HistoryEntry.ts` has `role: "reasoning"`).
  `monty` not on PATH: the repl suite that needs it did not run.
- `crates/desktop/ui`: `bunx tsc --noEmit` exit 0 (after `bun install` in
  `packages/apps`, the known `@openuidev/*` baseline); `CI=1 bunx vitest run`
  82 files, 499 passed, 1 skipped; `bunx vite build` ✓.
- Live, `target/debug/t` against `THEUTH_DATA_DIR=/tmp/69/think-dd`
  (`ANTHROPIC_API_KEY` from the environment, `secrets.key_store: file`; the
  live `~/.theuth` untouched):
  - `t -p "…why is the sky blue? Think briefly first."`, `effort: ~`, Fable
    5.1 → `💭 This just needs a quick, one-sentence answer…` then the reply;
    `147↓`. Same with `THEUTH_AGENT__EFFORT=off` → no `💭`, `89↓`.
  - `t --headless` + Vite on 5187 + headless Chromium: a 24-puzzle prompt
    streams `› ● Thinking… I'm trying combinations with 2,3,5,8 …` (collapsed,
    live tail), settles to `› Thought`, which **stays** after `TurnFinished`
    and on reopening `/c/<session>`; click expands the text. Raw RPC capture
    of another turn: 12 `ReasoningDelta`, `load_session` returns
    `{role: "reasoning", turn: 0, text: …}` before the assistant row.
  - Fable at `medium` with `display: summarized` often returns **no**
    thinking for short prompts (three of six smoke turns) — that is the
    model's adaptive choice, not a pipeline gap; `/effort high` raises it.

## Not done / follow-ups

- The TUI's finished reasoning stays a one-line `…` row (no expand key);
  the request was to render it, which it did already.
- Effort labels: a session with `effort: NULL` shows no effort in headers
  although `medium` is sent to Anthropic. A `ModelInfo.default_effort` would
  let UIs print `medium (default)`.
- genai (fork, out of this tree): `Budget(n)` on an adaptive model emits
  `thinking.adaptive.budget_tokens`, which Anthropic rejects — pre-existing,
  noted in `models/src/lib.rs:108-111`.
