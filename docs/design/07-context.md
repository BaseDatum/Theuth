# 07 — Context: append-only frames, epochs, compaction

Status: implemented v0 · 2026-09-10 · extends `00-architecture.md` §5,
`02-models-and-menus.md` §3 (header), `04-prompt-skills-mcp.md` §1.1,
`01-config-and-subagents.md` §2.5. Where this doc disagrees with those, this
doc wins; §13 lists the edits made there. Code: `crates/agent/src/{frame,
compact,spill,todo,loop_,store}.rs`.

Goal: a session can run for hours and hundreds of tool rounds without the
model losing the thread, and every model call after the first reads its
prefix from the provider cache. The two goals are the same goal: a context
that is never rewritten is both cheap and legible.

The design was written after reading jarvis's implementation
(`../jarvis/crates/jarvis-agent/src/{run,compact,hygiene,artifacts,epoch,
todos,phase}.rs`, `jarvis-context`, `docs/context-management-*.md`). §1 is
what it taught us; the rest is what we do instead.

## 0. The invariant

> **Between two consecutive model calls of one session, the request sent to
> the provider is the previous request plus appended messages.** Nothing
> before the previous request's last message changes: not the tools, not the
> system prompt, not a byte of any earlier message.

The only permitted exceptions are two named, counted, logged events:

- **rebase** — the system prompt is re-rendered at a *turn* start and differs
  from the previous turn's (the user edited `prompt.md`, a skill was added,
  the date rolled over). Messages are untouched.
- **compaction** — the message list is cut and a summary is put in front of
  the remainder. This starts a new **epoch**.

Everything else in this doc is a consequence: where volatile text may live
(only at the tail), how tool output is bounded (at append time, never later),
how the window is measured (from the provider's own usage report, never a
local tokenizer), and how compaction is paid for (as a cache-hot continuation
of the very frame it is compacting).

## 1. What jarvis does, and where it breaks

jarvis has excellent instincts and a `Vec<ChatMessage>` it mutates from
fifteen places. Every row below is verified against the code.

| jarvis mechanism | where | what goes wrong |
|---|---|---|
| Sticky todo / harness-status: `retain()` the old copy out of the middle, `push()` a fresh one at the tail, **every iteration** | `todos.rs:285-298`, `phase.rs:1125-1152` | The removal shifts every later message; the prefix cache is busted on every round even when the todo list is unchanged. Detection is `content.contains("<!-- jarvis:run_todos -->")`, so a user quoting the marker gets their message deleted. |
| Progressive prune: rewrite old tool bodies in place to `[old tool result cleared: …]` once the estimate passes 0.5×usable | `compact.rs:635-720`, called each iteration from `run.rs:4645` | A rewrite at index *i* invalidates everything after *i*; the gate trips repeatedly as results age out of the 40k protect zone, so the whole history is re-cached (1.25×) to save a few thousand tokens (0.1× when cached). Net negative. |
| `context_shelve` / `context_recall`: model-driven in-place stub replacement | `artifacts.rs:462-486` | Same invalidation, now at the model's whim; a `resync_stack_index_from_messages` exists only because prune and shelve overwrite each other's stubs. |
| Epoch delta as `messages[1]` System row, persisted and replayed inside history on later turns | `epoch.rs:552-576`, `conversation.rs:1571-1577` | `core.time` (minute clock) and `core.recall` are reconciled sources, so a delta lands almost every turn; every previous delta then re-appears interleaved in history. Position [1] changes per turn ⇒ nothing after the baseline is ever a cache hit. |
| Per-turn skill bodies and tool auto-activation as System messages **between** baseline and history | `epoch.rs:552-576`, `run.rs:1702-1760` | Volatile content placed before stable content. OpenAI adapter then coalesces all System messages into index 0 (`genai_backend.rs:713-746`), so one changed word rewrites the entire cached prefix. |
| Fresh system baseline rendered on every compaction, lineage text stamped `Utc::now().to_rfc3339()` | `compact.rs:1037,1675-1690`, `epoch.rs:632-745` | The in-memory boundary and the durably replayed one are different bytes for the same lineage; the new baseline needs a re-cache even though the inputs did not change. |
| Three token estimators (`estimate_message_row_tokens`, `estimate_stack_tokens`, `blended_context_estimate`) feeding five caps (`history 0.6×`, `prune 0.5×`, `compact 0.75×`, `tool_tokens_per_turn_max`, `tool_result_max_chars`) | `llm/mod.rs:970-999`, `compact.rs:28-96`, `hygiene.rs:37-77` | ≥3 full-stack tiktoken passes per iteration, each allocating the stack as one String; budgets are compared against different numbers; ~25 config keys govern one Vec. |
| Utility model summarises a re-serialised transcript (80k-char head/tail clip), plus a second "memory flush" call | `compact.rs:970-1008,1481-1520` | Both calls are cache-cold. The transcript is a lossy re-rendering of what the model actually saw. |
| `active_state.rs` post-compaction `<system-reminder>` | `active_state.rs` | Never wired (`append_reminder_block` is referenced only by its own tests). |
| Anthropic breakpoints on `messages[0]` + last two non-system | `genai_backend.rs:559-625` | Correct instinct (`MAX_CACHED_SYSTEM = 1`, "deltas must not get a breakpoint"), defeated by everything above. |

What we keep from jarvis, in one line each: a compaction threshold that
leaves headroom for the next completion rather than filling the window;
never split a `tool_use` from its `tool_result`; a structured summary
template that updates the previous summary; spill full tool output to disk
behind a retrieval hint; harness-owned truth (running subagents) appended
to the summary rather than recalled by the model; a durable, append-only
boundary so history folding needs no deletes; provider-reported usage as
the source of truth.

## 2. Frames

### 2.1 Anatomy

A **frame** is what one model call sends. Same order on every provider
(this is also the order Anthropic hashes its cache in: tools → system →
messages):

```
tools     ToolSpec[] sorted by name                          §5
system    baseline text                                      §4
messages  [ head ]  epoch summary, user role, absent in epoch 0   §8
          history…  every message since the epoch began, verbatim, in order
          (appended this round: assistant, tool results, steering, pressure note)
```

`Frame` is a pure function in `agent::frame`:

```rust
pub struct Frame<'a> {
    pub tools: &'a [genai::chat::Tool],
    pub system: &'a str,
    pub head: Option<&'a ChatMessage>,
    pub history: &'a [ChatMessage],
}
impl Frame<'_> {
    /// The request and options for `t`, cache marks placed (§3); `session` is the OpenAI `prompt_cache_key`.
    pub fn request(&self, t: Transport, session: SessionId) -> (ChatRequest, ChatOptions);
}
```

No state, no I/O. Cache marks (and the OpenAI cache key) are the only
per-transport difference.

### 2.2 Rules

1. **Append-only within an epoch.** `history` only grows: `persist()` pushes
   and inserts a row. There is no code path that assigns to `history[i]`,
   removes from the middle, or reorders. Compaction is `drain(..keep_from)`
   plus a new head, and is the *only* such path (§8). Reads honour
   `sessions.cut_at` (undo, §8.8) and return only visible rows; writes go
   through `Session::claim`, which commits a pending cut first, so the
   loop never sees a hidden row.
2. **Volatile text lives at the tail.** Anything that changes per turn or
   per round is appended as a message *after* everything the model already
   saw. It is then history: frozen, persisted, replayed identically on
   resume.
3. **The system prompt is frozen for a turn.** Rendered once when the turn
   starts (not per round, as the pre-07 loop did). A mid-turn `add_skill`
   or `prompt.md` edit is picked up by the next turn; the model can use a
   skill it just wrote via `load_skill` in the meantime.
4. **Nothing in the frame carries a clock finer than the day.** The date is
   in the baseline (as today); when it changes the next turn rebases. The
   time is not in the prompt at all; the shell tool has `date`.
5. **Tool descriptions and schemas are pure functions of configuration.**
   Not of the session, the turn, the roster or the clock. The tool list is
   fetched once per turn (§5), so it cannot change mid-turn.
6. **Tool output is bounded when it is appended, never later.** §6.
7. **Every mutation that is not an append is a named event** (`rebase`,
   `compaction`), logged with its cause, so an unexpected cache miss can
   always be explained from the log.
8. **Harness inputs are plain user messages.** `<subagent_result>`,
   `<job_result>` (14 §3) and peer `<message>` are appended like typed
   input; they carry no id and publish no `Input*` event (01 §2.3).

### 2.3 What changes, where it lives, what happens

| input | lives in | changes when | treatment |
|---|---|---|---|
| identity line, Claude Code identity (login-dependent), cwd | system | never within a process | — |
| date | system | midnight | rebase at the next turn start |
| `prompt.md`, `always` skill bodies, skills index | system | user or model edits | rebase at the next turn start (one miss, deliberate) |
| peer guidance (how to spawn/message), todo sentence | system | never | the `GUIDANCE` const in `frame::baseline`; the live roster is **removed** from the prompt (§10) |
| plugin prompt blocks (knowledge sentence, …) | system | a plugin registers/unregisters — in practice process start | `ask(ListPrompts)` in `Turn::rebase`; a block registered after the first turn's rebase lands with one rebase on the next turn; the knowledge *index* is deliberately not here (10 §4.3) |
| tool list | tools | MCP server connects/disconnects, gate change — only while the runtime is off; with it on, MCP tools are `direct = false` and never in the list (§5) | fetched once per turn in `Turn::rebase` (12 §4); a change is visible on the next turn, never mid-turn |
| user input, steering, `<message>`, `<subagent_result>`, `<job_result>` | history tail | per turn | append |
| assistant output, tool results | history tail | per round | append |
| harness notes: context pressure, todo reminder | history tail | pressure once per epoch; reminder rate-limited (§8.7, §9) | append |
| `<knowledge>` hints (10 §4) | history tail | turn start against the input, then every `hint_rounds = 4` rounds against what was appended — only when there are hits | append |
| harvest (10 §5) | a dead-end branch off the finished frame: history + one instruction, never persisted | every 3rd tool-using root turn (inline after `TurnFinished`), and right before a compaction cut | no effect on the session's prefix; the branch reads the cached prefix and writes one small mark |
| todo list | `SessionState` state; visible via `todo` tool output, reminder notes, compaction head | every `todo` call | append only; the list is never re-injected in place (§9) |
| runtime inventory (`08-repl.md` §6) | `Repl` state; visible in cell results and the compaction head | every `python` cell | append; harness truth |
| epoch summary | head | compaction | new epoch |
| model | request param | `/model` | outside the cached prefix; a different model is a different cache anyway |

## 3. Provider facts we rely on

Verified 2026-09-09 against the Anthropic and OpenAI prompt-caching pages
and genai 0.7.0-beta.23 source.

**Anthropic** (explicit `cache_control` breakpoints, ≤4 per request):
- The cache is a prefix hash over `tools → system → messages`. Changing a
  tool definition invalidates all three; changing system invalidates system
  and messages; changing message *i* invalidates from *i*.
- Lookup walks back **20 positions** from each breakpoint looking for an
  entry a previous request wrote. A run of consecutive `tool_use` blocks
  is one position, as is a run of `tool_result` blocks — a parallel tool
  batch does not push the previous entry out of the window.
- Writes happen only at breakpoints. A breakpoint on content that differs
  every request (a timestamp) never produces a read.
- TTL 5 min (`ephemeral`, write 1.25×) or 1 h (`ttl: "1h"`, write 2×);
  reads 0.1×. A 1 h entry must precede any 5 min entry in the prompt.
- Minimum cacheable prefix: 1024 tokens on Sonnet 4.5 / Opus 4.1, 2048 on
  Haiku 3.5, 4096 on Haiku 4.5 (a bare system prompt may fall under it; the
  first tool round always clears it).
- `tool_choice`, images, and thinking/effort parameters are part of the
  hash. We never set `tool_choice`; thinking config is per model, constant.
- Thinking blocks in earlier assistant turns are cached with the rest. On
  Opus 4.5+ / Sonnet 4.6+ they are preserved when a non-tool-result user
  message (steering, the pressure note) is appended; on earlier Sonnet/Opus
  and all Haiku models they are stripped at that point and the cache is
  invalidated from there. Provider behaviour, not ours; steering is rare
  and the miss is bounded to one round. Noted so §7's warning is read
  correctly on those models.
- Usage: `cache_read_input_tokens`, `cache_creation_input_tokens`,
  `input_tokens` (tokens after the last breakpoint). genai normalises
  `prompt_tokens = input + creation + read` and exposes the parts in
  `prompt_tokens_details { cached_tokens, cache_creation_tokens }`.

**OpenAI** (implicit prefix caching, on by default): exact-prefix match over
the rendered request; implicit breakpoint at the end of the latest
eligible message; `prompt_cache_key` routes requests with the same key to
the same cache; retention `in_memory` (5–10 min) or `24h` on models that
support it. Explicit breakpoints exist on gpt-5.6+; we do not use them in
v0. `cached_tokens` arrives in `prompt_tokens_details`.

**Gemini**: implicit caching is automatic for a stable prefix;
`cachedContentTokenCount` maps to `cached_tokens`. Nothing to place.

**genai surface** (`chat/chat_message.rs:81-147`, `chat_options.rs:238-248`,
`adapter/adapters/anthropic/adapter_shared.rs:126-180`):
- `ChatMessage::with_options(CacheControl::{Ephemeral, Ephemeral1h, …})` —
  message-level marks; on Anthropic a `ChatRole::System` message with a mark
  becomes a `system` block with `cache_control`. `ChatRequest::with_system`
  text cannot carry a mark ("the first System cannot have a cache control"),
  so the baseline goes in as a System-role message on Anthropic.
- `ChatOptions::with_cache_control(..)` — request-level; on Anthropic it
  auto-marks the last system block *unless* a message-level mark exists; on
  OpenAI it becomes `prompt_cache_retention` (`Ephemeral24h → "24h"`).
- `ChatOptions::with_prompt_cache_key(..)` — OpenAI routing key.
- Anthropic's `input_tokens` excludes cached tokens; genai adds them back,
  so `prompt_tokens + completion_tokens` is always "what the window holds".

Cache marks per transport, chosen from the model's genai `AdapterKind`
(`Client::resolve_service_target(model).model.adapter_kind`; the loop
already keys on the namespace string for Cursor):

| transport | system | marks |
|---|---|---|
| `Anthropic`, `MiniMax` (Anthropic protocol) | `ChatMessage::system(baseline).with_options(Ephemeral1h)` first in `messages` | last message `.with_options(Ephemeral)`. Two breakpoints: system (1 h — identical across sessions and days, worth the 2× write) and the tail (5 min). Order satisfies the 1h-before-5m rule. The 20-position lookback covers a round (assistant text + one `tool_use` run + one `tool_result` run + steering), so the tail mark always finds the previous request's entry. |
| `OpenAI`, `OpenAIResp`, and every adapter genai documents as "mostly OpenAI" (`Groq`, `Xai`, `DeepSeek`, `Together`, `Fireworks`, `OpenRouter`, `Nebius`, `Moonshot`, `Kimi`, `Mimo`, `Zai`, `Aliyun`, `Baidu`, `Aihubmix`, `AtlasCloud`, `Omlx`, `OpenCodeGo`) | `with_system` | `ChatOptions::with_prompt_cache_key(session_id).with_cache_control(Ephemeral24h)`; no message marks (implicit mode). Gateways that ignore the fields ignore them; genai already gates `prompt_cache_retention` by model. |
| `Gemini`, `GeminiIx`, `Vertex`, `BedrockApi`/`Bedrock`, `GithubCopilot`, `Ollama`, `OllamaCloud`, `Cohere`, custom `genai_*`, and the `cursor` namespace | `with_system` | none. Gemini and Bedrock cache implicitly on a stable prefix; the append-only frame is what makes that work. |

The Cursor transport reads `req.system` and System-role messages alike
(`cursor/src/history.rs:145-150`), so either shape works there.

## 4. Baseline

`agent::frame::baseline(data_dir, peer_line, plugin_blocks, claude_code) -> String` is
the old `system_prompt` with two changes: it is called once per **turn** by
`Turn::rebase` and stored on `SessionState.baseline`; and the peers block no
longer contains the roster (§10). Parts, blank-line separated, empty parts
skipped:

```
1. [Claude Code identity line, when the anthropic login is used]   (05 §2.3)
2. identity, working directory, date                              (unchanged)
3. <data_dir>/prompt.md
4. always-skill bodies, name order
5. skills index
6. peer line: `PeerContext { session }` → "## Peers\nYou are `x`, a subagent of session …" for children, else ""
7. plugin prompt blocks: `ask(ListPrompts)` on the tool registry, sorted by plugin name, joined by blank lines (00 §4); "" when none. Registered today: `agent` (data dir path, what each file is, "load the `harness` skill before editing", what is read at startup vs live), `knowledge` (when to use the store), `tools` (`mcp/` layout, shell expansion, `mcp/config.yaml` gate schema, `<server>__<tool>` naming, what `tools.policy` does and that the model cannot approve or log in). Each is a function of config only.
8. GUIDANCE const: the todo sentence (§9) + spawn_subagent / send_message / <subagent_result> text
```

`Turn::rebase` then appends `repl::prompt_block(ListTools)` (`08 §3.2`) when
the runtime pool is on. Parts 7 and 8 are static text within a process;
everything the roster used to contribute per turn is gone.

The long tail of harness knowledge - every `config.yaml` key, the MCP entry
shapes, what only the user can do - is the bundled `harness` skill
(`crates/wth/harness.md`, written to `<data_dir>/skills/harness.md` on every
start so it matches the binary): one index line in part 5, body on
`load_skill`. Rejected: putting it all in prompt blocks (~1.5k tokens on
every call for text needed in maybe one turn in fifty) and a `wth_help` tool
(a skill already is that).

`SessionState` keeps `baseline: String` across turns. At turn start the new
render is compared with it; on inequality (and a non-empty previous value):

```
tracing::info!(session, "rebase: system prompt changed ({} → {} bytes)", old, new)
```

and the new text is used. No event, no UI change: the next `TurnFinished`
shows a lower cache ratio and the log says why. A rebase does **not**
touch `head` or `history` — the summary from the last compaction stays.

Within the turn every round uses the stored string; `ShowPrompt` returns
it. The one behaviour lost against the original `04 §1.1` ("editing
`prompt.md` applies mid-turn on the next model call") is deliberate: a 30-round turn is
where the cache matters most, and a turn is seconds to minutes long.

`ListTools` is sorted by name (`tools/registry.rs`). `Turn::rebase` asks it
once per turn and `Turn::direct_tools` builds the direct list from that
(12 §4):

```
specs = ListTools
      .filter(|s| (!runtime || s.direct) && (s.name != spawn_subagent || depth < max_depth))
      + todo::spec()  + repl::spec() if runtime          // loop-local tools, §9, 08 §3.1
      sorted by name
```

The list is fixed for the turn, so the prefix never resets because of tools
mid-turn; a tool registered during a turn (an MCP server connecting, the
knowledge plugin's tools landing after the registry) is visible on the next
turn.

With the runtime on (`08-repl.md` §0.4) MCP tools carry `ToolSpec.direct =
false` (`tools/mcp.rs`) and the `retain(direct)` above drops them: the direct
list is native tools + `todo` + `python`, and MCP servers connecting or
flapping no longer touch it. The runtime's tool index lives in the baseline
and changes at turn boundaries as a rebase.

`todo` and `python` are **loop-local**: not in the registry, dispatched by
`Turn::round` itself (`Turn::todo`, `Turn::python`), so they bypass the
approval policy — both are `Risk::ReadOnly`, and `python`'s inner tool
calls go through the registry gate (`08 §4.3`).

Rule 2.2.5 is enforced by review, not code: a tool's `description()` must
not read the clock, the session, or the roster. `spawn_subagent`'s
description lists external harness names (`06 §2`) — static per process,
fine. `spawn_subagent` is hidden at `depth ≥ max_depth`; that is per
session, not per round, fine.

## 6. Tool output

Today `truncate(out, max_tool_output_chars)` keeps the head and appends a
marker. Replaced by `agent::spill::cap`:

```
if content.chars().count() <= max_tool_output_chars → unchanged
else:
  write full content to <data_dir>/spill/<session>/<call_id>.txt
  keep = max_tool_output_chars − marker.len()
  head = first 70 % of keep (char boundary), tail = last 30 %
  content = head + "\n[… {omitted} chars omitted; full output saved to {path} — read_file with offset/limit …]\n" + tail
```

Head + tail because build logs, test runs and stack traces put the verdict
at the end. The file path is absolute, so the model's existing `read_file`
(`offset`/`limit`, 64 KiB cap) is the retrieval tool; no `context_recall`.
Errors keep the `ERROR: ` prefix. The 2000-char UI truncation
(`DISPLAY_OUTPUT_CHARS`) is separate and unchanged.

Sweep: `AgentPlugin::start` deletes `spill/*/*` older than 7 days. One
`read_dir` walk; no index file.

The spill directory is also mounted read-only into the runtime as
`/spill` (`08-repl.md` §4.4), so a spilled output can be loaded whole into
a variable instead of paged with `read_file`.

This is the whole tool-output policy. There is no per-tool cap table, no
per-batch token budget, no bulk/minified detector, no later pruning
(jarvis has all five). The append-time cap bounds one round's results at
`calls × max_tool_output_chars` chars, all of which are counted by the
trigger *before* the next call (§8.1). Ceiling: on a 32k-window model a
four-call batch of 32k chars each is the whole window; lower
`max_tool_output_chars` there. If a transcript shows a tool that needs a
tighter cap, the tool truncates its own output — `read_file` already does.

## 7. Measurement

The provider tells us exactly what the window holds after each call. That
number, not a tokenizer, drives everything:

```rust
pub struct Usage {
    pub prompt: u32,      // Σ prompt_tokens over the turn (cached + written + fresh)
    pub completion: u32,  // Σ completion_tokens
    pub cached: u32,      // Σ prompt_tokens_details.cached_tokens
}
```

`SessionState.used: u32` = `prompt + completion` of the **last** call
(`TurnFinished.context`), kept across turns so the first call of the next
turn has a baseline. It is reset to 0 by compaction and by resume.

**Header** (`02 §3`, `tui::model::Model::header`): `claude-sonnet-4-5  ·
ctx 45.2k / 200k (23%)  ·  cache 94%` where `cache = cached / prompt` of the
last finished turn (omitted until one has reported usage). Print mode
(`-p`) prints the same ratio in its footer: `[end_turn] 12345↑ 678↓ cache
83%`. The ratio is the invariant's live readout: a healthy multi-round turn
sits above 90 %; the turn after a rebase or compaction dips once; a turn
that dips every round is a bug.

**Miss detection** — in `Turn::round`, after the usage arrives, when
`transport != Transport::Plain`:

```
if let Some((at, prev_prompt)) = state.prev_call     // any earlier call in this session
   && at.elapsed() < CACHE_TTL /* 5 min */ && !self.reset_this_round
   && prev_prompt >= CACHE_FLOOR /* 4096 */ && cached < prev_prompt * 9 / 10 {
    tracing::warn!(session, round, cached, prev_prompt,
        "prefix cache miss (no rebase/compaction/tool change since the previous call)");
}
```

`reset_this_round` is set by rebase, compaction and the tools-changed check
and cleared after the call. The check covers the first call of a turn too:
the previous turn's tail breakpoint is a hit when the user replies within
the TTL. Below the provider's minimum cacheable length `cached` is 0
legitimately, hence the `4096` floor. This is the one place the design
checks itself in production. It costs nothing.

**The only local estimate** is `est(chars) = chars / 4`, applied to the
messages appended since the last call (a few tool results), never to the
whole history. It exists solely to predict whether the *next* call fits
(§8.1). It is allowed to be 30 % wrong: the headroom absorbs it and the
overflow path (§8.5) catches the rest.

## 8. Compaction

### 8.1 Trigger

Before each model call:

```
window   = catalog.closest(model).limit.context          (0 when unknown)
appended = Σ est(history[since..])                         // persisted since the last call: user text, steering, tool results
projected = used + appended
debug!(session, round, window, used, appended, projected, "trigger check")
compact if window > 0 && compact_at > 0 && projected / window >= compact_at
```

`compact_at` defaults to 0.8. The results of the batch that just ran are
in `appended`, so the 20 % headroom is not for them; it covers the next
completion (bounded by the model's output limit) and the `chars/4` error
on the appended slice. On a 200k window that is 40k tokens. No separate
output reserve, no `usable` floor, no protect-token budget: one knob, one
formula. Smaller windows compact earlier in proportion.

`window == 0` (model not in the catalog, no `models.json` entry): no
automatic compaction; the overflow path (§8.5) still compacts once on a
provider error. Logged once per session at `info`.

A response with no `usage` (or `prompt_tokens == 0`; LiteLLM and similar
gateways strip it) sets `used` to chars/4 over the whole frame sent — tools
JSON, system, head, history — plus the reply's content and reasoning
(`Turn::est_frame`, `est_reply`). `Usage` totals stay at what the provider
reported. Same estimator as appended slices; no tokenizer dependency.

Also triggered by `/compact` (§8.6) and by overflow (§8.5), both with the
threshold ignored.

### 8.2 Cut

`history` is `M[0..n)`. `compact::keep_from(&M, limit)` with `limit =
compact_at × window` (the raw `window` when `compact_at == 0`, i.e. a
manual or overflow compaction with auto off):

```
budget    = limit / 8
valid(i)  = M[i].role != Tool             // never orphan a tool_result from its tool_use
walk i from n-1 down to 1, acc += est(M[i]):
    acc > budget               → stop
    valid(i)                   → keep_from = i   (keep walking: a larger tail may still fit)
no keep_from recorded          → keep_from = largest valid i in 1..n   (smallest possible tail)
no valid i in 1..n             → keep_from = 0: nothing to fold; skip ("nothing to compact")
```

The budget is an eighth of the *threshold*, not of the window. The draft
said `window / 8`; testing at `compact_at: 0.02` on a 1M-context model
showed why that is wrong: the tail budget (125k) exceeded the threshold
(20k), so every compaction folded exactly one message and the next round
compacted again. Sizing the tail to the threshold keeps the post-compaction
frame well under it at any `compact_at`.

The fallback keeps the last assistant message and its results even when
they alone exceed the budget; if they alone exceed the *window* the
compacted frame overflows too and §8.5 surfaces the error — nothing else
can help at that point.

Kept verbatim: `M[keep_from..n)`, at most ~1/8 of the threshold (20k on a
200k window at 0.8) — the tool outputs, file contents and error text the
model is *currently* working from. Folded: everything before it, plus the
previous head.
Cutting before an assistant message is fine (its tool results follow it);
cutting before a user message is fine; cutting before a Tool message is
not, hence `valid`. The first message of the kept tail may be a User
message directly after the User-role head — Anthropic merges consecutive
user turns, OpenAI accepts them.

### 8.3 Summariser: a cache-hot continuation

jarvis re-serialises the transcript and sends it cold to a utility model.
We send **the frame itself** — tools, system, head, all of `M`, unchanged —
plus one appended user message, `COMPACT_INSTRUCTION`. The prefix up to
`M[n-1]` carries the breakpoint from the previous call, so the provider
reads the entire conversation from cache at 0.1×; only the instruction and
the summary are billed at full rate. Same model as the session (no
`compact_model` knob: the cache hit makes the strong model the cheap one).
This is the `summary` style (`11-compaction-styles.md`). The new head wraps
the model text in `<context_summary epoch="n" style="summary">`. `cut` and
`snap` build a different head; they do not run this call.

```
COMPACT_INSTRUCTION (const, fixed bytes):

The conversation is about to be compacted: everything above this message will be replaced by
your summary, and only the most recent messages will be kept verbatim. Write the summary now.
Reply with text only — do not call tools. Use exactly these headings and short bullets:

## Goal
## Done  (facts established, files changed, commands run and their outcomes)
## Open  (in-progress work, unanswered questions, pending approvals)
## Next  (the immediate next step)
## Notes (user preferences, constraints, names and paths to remember, runtime variables you rely on and what they hold)

If a previous summary is present at the top of the conversation, fold it in: keep what is still
true, drop what is finished. Be specific — paths, identifiers, exact error text — and omit
chit-chat and tool-call boilerplate.
```

The tail `M[keep_from..)` is summarised too, redundantly with its verbatim
copy in the next epoch. Accepted: the alternative is a marker string in
the prompt or a shortened request that may miss the lookback window;
overlap between a 1–2k summary and a 25k tail costs nothing and reads
naturally.

If the response carries tool calls or empty text: retry once with the
instruction prefixed `Text only. ` (still cache-hot). Second failure →
finish as `cut` on the same `keep_from` (§8.5, 11 §3.2). Other errors emit
`AgentEvent::Error` and leave history untouched.

Nothing from the summariser call is persisted as a message. It publishes
`AgentEvent::Compacting { session, turn }` before and streams no deltas.

### 8.4 New epoch

```
summary   = model text
roster    = agents.ask(Roster { session }): "Subagents: grep-logs (running), fix-tests (idle)" or ""
todos     = todo::render(&state.todos) if non-empty                                   (§9)
runtime   = "Runtime: " + (repl.render_inventory() | repl::saved_inventory(data_dir, session))  (08 §6)
read      = "Knowledge: read " + state.knowledge_read.join(", ") if non-empty              (10 §7)
head      = ChatMessage::user("<context_summary epoch=\"{n}\" style=\"summary\">\n{summary}" + "\n{part}" for each present part + "\n</context_summary>")
tail_from = seq(M[keep_from])
at_seq    = seq(M[n-1])
store.compact(session, n, at_seq, tail_from, &head)        // one INSERT into epochs
history.drain(..keep_from); head = Some(head); used = 0; epoch = n; pressure_sent = false; hinted.clear(); knowledge_read.clear(); reset_this_round = true
info!(session, epoch, folded, before, "compaction")
publish Compacted { session, turn, epoch: n, before: projected, folded: keep_from, style }
```
Other styles (`cut`, `snap`) share this insert and differ only in the head
(11). `cut` has no model call. Heads with no `style` attribute are `summary`.

The roster line is harness-generated: the one piece of live state the
model would otherwise have to recall correctly across the boundary
(jarvis's `active_state.rs`, wired this time). Steering that arrived
during compaction was persisted before the cut and is in the tail.

The next round's frame is `[tools][system][head][M[keep_from..)] + new
messages`. One cache miss, expected, logged as `compaction`.

**Store.** Migration `0003_epochs.sql`:

```sql
CREATE TABLE epochs (
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    n          INTEGER NOT NULL,          -- 1, 2, … (epoch 0 has no row)
    at_seq     INTEGER NOT NULL,          -- last message seq when compaction ran (UI divider position)
    tail_from  INTEGER NOT NULL,          -- first message seq replayed after the head
    head       TEXT    NOT NULL,          -- ChatMessage JSON
    created_at TEXT    NOT NULL,
    PRIMARY KEY (session_id, n)
);
```

`messages` is unchanged and still append-only; nothing is ever deleted or
rewritten. `Store::append` gains `RETURNING seq` so history rows carry
their seq (`Vec<(i64, ChatMessage)>`), which is what `tail_from` needs.
`epochs.head` may hold base64 PNG parts for `snap` heads (13 §2.4; ~0.5 MiB
per epoch row at the default budget).

**Resume** (`--resume`, subagent lazy load, `LoadSession`):

```
epoch = latest epochs row (or none)
head  = epoch.head
history = messages WHERE seq >= epoch.tail_from   (all messages when no epoch)
used  = 0
```

The frame on resume is byte-identical to the one the model last saw, so
a resume inside the TTL is a cache hit as well.

**UI.** `Loaded.history` interleaves `HistoryEntry::Compacted { epoch,
summary }` after the entry for `at_seq`. The TUI renders it as a divider
`── context compacted (epoch 2) ──`, expandable with Ctrl+O to the
summary text, same card shape as a peer message. Live: `Compacting` shows
`compacting…` in the status line; `Compacted` inserts the divider ahead of
the current streaming text (the `Model::interject` path from `00 §8`).

### 8.5 Overflow

genai has no typed overflow error; classification is by substring on the
lowercased message, the same needle list jarvis converged on
(`"context length"`, `"context_length_exceeded"`, `"maximum context"`,
`"prompt is too long"`, `"input length and max_tokens exceed"`,
`"request too large"`, `"exceeds the maximum number of tokens"`, `"too
many tokens"`). On a match, once per turn: compaction with the threshold
ignored, then retry the round. If the *summariser* call itself overflows,
retry it once with `M[keep_from..)` removed from its frame (a strict
prefix of the previous request — smaller, and usually still within the
lookback window). If `summary` still fails after that retry (overflow, or
empty/tool-call after `Text only. `), finish as `cut` on the same
`keep_from` (11 §3.2); `Compacted.style` is `cut`. If `keep_from == 0`,
surface the original overflow and stop the turn.

### 8.6 Manual, events, surfaces

- `Compact { session, style: Option<CompactStyle> }` — `ask` → `Result<u32 /* epoch */, String>` on the
  `"agent"` supervisor (`supervisor.rs` `impl Message<Compact>`), which
  forwards `CompactNow` to the session actor. `style: None` = configured
  default (`agent.compact_style`, then 11 §3.1). Threshold ignored; `keep_from
  == 0` → `Err("nothing to compact")`; `Err("turn in flight")` while a turn
  runs.
- **A `/compact` is a mini-turn.** `Session`'s `CompactNow` handler takes
  the `SessionState`, sets `inflight`, and runs `Turn::compact_now` (rebase,
  tools, transport, then `Turn::compact` with `turn: None`) in a spawned
  task that hands the state back via `TurnDone`; the reply is a
  `DelegatedReply`. The supervisor's `Compact` handler is delegated too
  (`tokio::spawn` around `actor.ask(CompactNow)`). Reason: the summariser
  asks the supervisor for `Roster`; handling either message inline on its
  own mailbox deadlocked in testing. Side effects of the mini-turn shape:
  input arriving meanwhile queues as steering and starts a turn when the
  compaction hands back, and `IsRunning` is true during it.
- TUI `/compact [summary|cut|snap|style …]` (11) → `Command::Compact { session, style }` → `Info("context compacted
  (epoch n)")`; RPC `compact(session, style?)`. `/compact style` lists or
  persists the default and does not compact.
- Events: `Compacting { session, turn: Option<TurnId> }`, `Compacted {
  session, turn: Option<TurnId>, epoch: u32, before: u32, folded: u32, style: CompactStyle }`.
  `TurnFinished.usage.cached` (§7).

### 8.7 Harness notes

A **note** is a user-role message the harness appends at the tail before a
model call: `Turn::note(kind, text)` → `persist(ChatMessage::user(format!(
"<{kind}>\n{text}\n</{kind}>")))`. Notes are ordinary history afterwards:
persisted, frozen, replayed on resume, hidden by UIs the way `<message>`
wrappers are (`tui::model::injected`). They are the *only* way the harness
speaks to the model mid-turn, and they are append-only by construction —
this is what replaces jarvis's `retain()+push()` stickies.

Three kinds exist:

- `context_pressure` — once per epoch (`pressure_sent`, reset by
  compaction), when `compact_at > 0.1` and `projected / window >= compact_at
  − 0.1`: *"The context window is {pct}% full; compaction is near. Finish
  the current step and state results and next steps explicitly in your
  reply so they survive."* It makes the model write its working state down
  right before the summariser needs it. The `> 0.1` guard keeps a tiny
  test threshold from sending the note on round one.
- `todo` — the reminder in §9.
- `knowledge` — matching concept ids (`10-knowledge.md` §4): at turn start
  against the input, then every `hint_rounds` rounds against what was
  appended since; each id once per epoch (`hinted`); nothing when nothing
  matches.

A lost or crashed runtime is *not* a note: the `<runtime>` line opens the
result of the cell that discovered it (`08-repl.md` §5), where the model
is already looking.

Notes are a user text block following a `tool_result` block; on Haiku and
pre-4.6 Sonnet that strips earlier thinking blocks (§3), the same as
steering. Both kinds are rate-limited for that reason as much as for
tokens.

### 8.8 Undo/redo

`/undo` hides the last **turn** — a user message and everything after it —
from the frame and the transcript; `/redo` restores it. Editor semantics:
a new action clears the redo stack.

**One column.** `sessions.cut_at INTEGER NULL`. Messages with `seq >=
cut_at` and epochs with `at_seq >= cut_at` are hidden; `messages()`,
`messages_from()`, `epochs()`, `frame()` and `last_turn_text()` return only
visible rows. `Store::undo` moves `cut_at` back to the previous turn start
(returns the hidden turn's user text; `None` when nothing is left to hide);
`Store::redo` moves it forward to the next turn start, or to `NULL` when
the last hidden turn comes back (`None` when `cut_at` is already `NULL`).
Neither deletes. The first write after an undo — `Session::claim` calls
`Store::commit_cut` — deletes messages `seq >= cut_at` and epochs `at_seq
>= cut_at` in one transaction and sets `cut_at = NULL`; the new turn then
appends after the visible tail as usual. A no-op when `cut_at` is `NULL`.

**Turn start** (`store::is_turn_start(prev, m)`): a user-role message whose
predecessor is nothing or an assistant-role message. Steering and harness
notes follow Tool or User rows, so they never count; a queued batch is
several User rows in a row and only the first counts; a turn the harness
starts (`<subagent_result>`, `<job_result>`, peer `<message>` into an idle
session) counts, so undo hides a job's turn like any other.
`Store::hidden_turns` counts turn starts among the hidden rows; it is
`Loaded.undone` (0 when `cut_at` is `NULL`), which the TUI shows as *N
turn(s) undone — /redo restores*.

**Across an epoch boundary.** Undo past a compaction hides the epoch row
too (`at_seq >= cut_at`), so the latest *visible* epoch's head and tail are
what the model sees: exactly the frame before that compaction, which the
session had already sent. Nothing is re-summarised.

**Cache.** The next request after an undo is a strict prefix of one the
provider has already cached (tools, system and every visible message are
unchanged), so it is a hit up to the new tail — the same reason §8.3's
summariser is cache-hot.

**Refused while a turn is in flight** (`Err("turn in flight")`): the loop
holds `SessionState` for the duration of a turn and would otherwise
persist after a row the store had just hidden. `Undo`/`Redo` are `ask` on
`Agents` → `Result<String, String>`; `Ok` is the first line (≤80 chars) of
the hidden/restored turn's user text, for the info card.

Not doing: branching (a cut is linear; the hidden tail is gone after the
next write), undo to a steer point inside a turn (it would leave a frame
ending in tool results, which no provider accepts as a prefix for a new
user message), an event for other frontends (they see the cut at their next
`Loaded`, which already carries `undone`).

## 9. Todo list

A run-local todo list: the model plans multi-step work, marks progress,
and the list survives long tool loops and compaction. "Run-local" means
it belongs to one session's live actor — not a task store, not shared
between sessions, not a durable feature of the account. It lives across
turns of that session (the user says "continue" and the plan is still
there) and dies with the actor; after `--resume` the model sees its last
list in history or in the compaction head and re-`init`s if it wants to
keep working from it.

The cache question has one answer: **the list is state, not a message.**
jarvis keeps the list *as a message* and moves that message to the tail
every iteration (`todos.rs:285-298`), busting the prefix on every round.
Here the model reads the list in exactly three places, all appended, none
ever edited or moved:

1. **the output of every `todo` call** — the full rendered list is the
   tool result, so the most recent state is always in history at the point
   the model last touched it (this is how oh-my-pi's todo works);
2. **a `<todo>` note** (§8.7) when the model has open items and has not
   called `todo` for `TODO_REMIND_ROUNDS = 4` rounds — appended before the
   next call, then `reminded_round = round`. A change of state resets the
   counter (the tool output *is* the reminder). Cost: ≤ 20 lines every few
   rounds, zero cache cost;
3. **the compaction head** (§8.4) — rendered by the harness from its own
   state, so the list crosses the epoch boundary exactly (jarvis's
   `active_state.rs`, wired), and the summariser cannot drift it.

State, on `SessionState`:

```rust
pub struct Todo { pub text: String, pub status: Status }   // Pending | InProgress | Done | Dropped
todos: Vec<Todo>, todo_touched_round: u32                  // SessionState; `init` replaces the list
```

Tool `todo` (`agent::todo`; `Risk::ReadOnly`) is **loop-local**: it is not
in the registry. `Turn::tools` appends `todo::spec()` to the direct list and
`Turn::round` dispatches it to `Turn::todo`, which applies the op to
`state.todos` in place — no actor hop, no `Invoke`, so the approval policy
never sees it (it is read-only harness state). The design draft had it
`ask` the supervisor; owning the state on the turn is shorter and needs no
forwarding.

```rust
struct TodoArgs {
    op: Op,                    // init | add | start | done | drop | view
    items: Vec<String>,        // init, add
    task: Option<String>,      // start, done, drop — exact text of an existing item
}
// → the rendered list, or Err naming the unknown task / the 32-item cap
```

Semantics, deliberately small: `init` replaces the list; `add` appends;
`start`/`done`/`drop` set one item's status; after any change, if nothing
is `InProgress`, the first `Pending` item becomes `InProgress` (saves the
model a round). Items are identified by their text, which is what the model
has in front of it; no ids. Cap 32 items (`Err` beyond). No phases, no
blocking, no persistence column — each is a few lines when a transcript
asks for it, and none changes the cache story.

Rendering (text glyphs, the TUI rule), used by the tool output, the note
and the head:

```
Todo (2/5 done):
  ☑ read the failing test
  ☑ find the callsite
  ▶ patch the parser
  ☐ run the suite
  ✗ update the docs   (dropped)
```

The system prompt gains one static sentence, the first line of
`frame::baseline`'s `GUIDANCE` const: *"For work with three or more steps,
keep a `todo` list and mark items as you go."* Static text — no cache
effect.

UI: `todo` calls render as ordinary tool cards (the list is the output);
the `<todo>` note is hidden like other wrappers. No `/todos` command, no
event: the transcript already shows every change. Add a status-line item
when a UI wants one — it can derive it from `ToolCallFinished`.

## 10. Subagents

Children are separate sessions with their own frames; a child's first
message is its task. Two changes from `01 §2.5`:

1. **The live roster leaves the system prompt.** `- grep-logs running` in
   the baseline flipped to `idle` between turns and busted the parent's
   system cache every time a child finished. The model learns children's
   names from `spawn_subagent`'s output and their completion from
   `<subagent_result>`; both are already in history. After compaction the
   head carries the roster (§8.4), fetched with `Roster { session }` →
   `"Subagents: a (running), b (idle)"` or `""`. `PeerContext { session }`
   returns only the identity lines (`"## Peers\nYou are `x`, a subagent of
   session …"`), `""` for a root; the static spawn/send guidance moved into
   `frame::baseline`'s `GUIDANCE` const.
2. **Children carry their own compaction.** A child running 40 rounds
   compacts like any session. The parent receives only the child's final
   text, as before.

Cross-session sharing of the *system* cache entry is real but partial: two
top-level sessions on the same day with the same `prompt.md` and skills
have identical tools and baseline, so the second reads the first's 1 h
entry. Parent and child differ in the child's identity line and in
`spawn_subagent`'s presence at `depth ≥ max_depth`, so they do not share.
Making them share (always list `spawn_subagent`, reject at call time; move
the identity line into the task wrapper) saves a few thousand tokens per
spawn and is listed in §14 as a follow-up with its precondition.

## 11. Types, config, messages

As built (`crates/agent/src`):

```rust
// agent::frame
pub enum Transport { Anthropic, OpenAiLike, Plain }              // §3 table
impl Transport { pub fn of(kind: AdapterKind) -> Self; }         // `cursor` never reaches genai → caller uses Plain
pub struct Frame<'a> { pub tools: &'a [Tool], pub system: &'a str, pub head: Option<&'a ChatMessage>, pub history: &'a [ChatMessage] }
impl Frame<'_> { pub fn request(&self, t: Transport, session: SessionId) -> (ChatRequest, ChatOptions); }
pub fn tools_hash(tools: &[Tool]) -> u64;                        // §5
pub fn baseline(data_dir: &Path, peer_line: &str, plugin_blocks: &str, claude_code: bool) -> String;   // §4; GUIDANCE const appended
pub fn truncate(s: &str, max: usize) -> String;                  // UI copies only

// agent::spill
pub fn cap(session: SessionId, call_id: &str, content: &str, max_chars: usize, dir: &Path) -> String;
pub fn sweep(dir: &Path, max_age: Duration);

// agent::todo
pub const NAME: &str = "todo";  pub const TODO_REMIND_ROUNDS: u32 = 4;
pub struct Todo { pub text: String, pub status: Status }
pub enum Status { Pending, InProgress, Done, Dropped }
pub enum Op { Init, Add, Start, Done, Drop, View }               // serde snake_case
pub struct TodoArgs { pub op: Op, #[serde(default)] pub items: Vec<String>, pub task: Option<String> }
pub fn apply(list: &mut Vec<Todo>, args: TodoArgs) -> Result<(), String>;   // pure; §9 semantics
pub fn render(list: &[Todo]) -> String;
pub fn spec() -> ToolSpec;                                       // Risk::ReadOnly, direct: true; loop-local

// agent::compact
pub enum CompactStyle { Summary, Cut, Snap }                     // default Summary; 11
pub const COMPACT_INSTRUCTION: &str;
pub fn est(m: &ChatMessage) -> u32;                              // serialised chars / 4
pub fn keep_from(history: &[ChatMessage], limit: u32) -> usize;  // §8.2; limit = compact_at × window
pub fn is_overflow(err: &str) -> bool;                           // §8.5

// agent::loop_
pub struct SessionState { pub epoch: u32, pub head: Option<ChatMessage>, pub history: Vec<(i64, ChatMessage)>,
    pub used: u32, pub baseline: String, pub pressure_sent: bool, pub prev_call: Option<(Instant, u32 /* prompt */)>,
    pub todos: Vec<todo::Todo>, pub todo_touched_round: u32,
    pub hinted: BTreeSet<String>, pub knowledge_hinted_round: u32, pub knowledge_read: BTreeSet<String>, pub turns_since_harvest: u32 }  // 10 §10
impl SessionState { pub fn resume(epoch: u32, head: Option<ChatMessage>, history: Vec<(i64, ChatMessage)>) -> Self; }
pub struct CompactNow { pub style: Option<CompactStyle> }        // ask → DelegatedReply<Result<u32, String>> (§8.6)
pub struct TurnDone(SessionState);                               // tell, turn task → its Session
// Turn (private): turn: Option<TurnId>, usage, transport: Option<(String, Transport)>,
//   tools: Vec<Tool> (this turn's direct list, 12 §4), reset_this_round, overflowed, since: usize, window_logged, repl: Option<Repl>

// agent::supervisor
pub struct PeerContext { pub session: SessionId }                // ask → String: identity lines or ""
pub struct Roster { pub session: SessionId }                     // ask → String: "Subagents: …" or ""

// agent::store
pub struct Epoch { pub n: u32, pub at_seq: i64, pub tail_from: i64, pub head: ChatMessage }
pub async fn append(&self, id: SessionId, message: &ChatMessage) -> anyhow::Result<i64 /* seq */>;
pub async fn epochs(&self, id: SessionId) -> anyhow::Result<Vec<Epoch>>;
pub async fn compact(&self, id: SessionId, n: u32, at_seq: i64, tail_from: i64, head: &ChatMessage) -> anyhow::Result<()>;
pub async fn frame(&self, id: SessionId) -> anyhow::Result<(u32, Option<ChatMessage>, Vec<(i64, ChatMessage)>)>;   // visible rows only (seq < cut_at)
pub async fn undo(&self, id: SessionId) -> anyhow::Result<Option<String>>;    // §8.8; None = nothing to undo
pub async fn redo(&self, id: SessionId) -> anyhow::Result<Option<String>>;    // §8.8; None = nothing to redo
pub async fn commit_cut(&self, id: SessionId) -> anyhow::Result<()>;          // §8.8; first write after an undo; no-op when cut_at is NULL
pub async fn hidden_turns(&self, id: SessionId) -> anyhow::Result<u32>;       // §8.8; Loaded.undone
pub(crate) fn is_turn_start(prev: Option<&ChatMessage>, m: &ChatMessage) -> bool;

// agent (lib.rs)
pub struct Compact { pub session: SessionId, pub style: Option<CompactStyle> }  // ask → Result<u32, String>; None = cfg default
AgentEvent::Compacting { session, turn: Option<TurnId> }
AgentEvent::Compacted  { session, turn: Option<TurnId>, epoch: u32, before: u32, folded: u32, style: CompactStyle }
HistoryEntry::Compacted { epoch: u32, summary: String, style: CompactStyle }
pub struct QueuedInput { pub id: InputId, pub text: String }                    // 01 §2.3; InputId(pub uuid::Uuid) lives in runtime, minted by the caller (v7)
pub struct Steer { pub session: SessionId, pub id: InputId, pub text: String }  // ask → (); idle: starts a turn now, still publishes InputInjected
pub struct Withdraw { pub session: SessionId, pub id: InputId }                 // ask → Result<String, String>; Err("not queued")
pub struct ListQueued { pub session: SessionId }                                // ask → Vec<QueuedInput>
pub struct Undo { pub session: SessionId }                                      // ask → Result<String, String>; Err("turn in flight" | "nothing to undo") (§8.8)
pub struct Redo { pub session: SessionId }                                      // ask → Result<String, String>; Err("turn in flight" | "nothing to redo")
AgentEvent::InputQueued    { session, id: InputId, text: String }
AgentEvent::InputInjected  { session, id: InputId, turn: TurnId }               // before TurnStarted when queued inputs start a turn
AgentEvent::InputWithdrawn { session, id: InputId }
Loaded { …, queued: Vec<QueuedInput> /* empty when idle */, undone: u32 /* hidden turns; 0 when cut_at is NULL */ }
pub struct Usage { pub prompt: u32, pub completion: u32, pub cached: u32 }
AgentConfig { …, max_tool_output_chars: usize /* 32_000 */, compact_at: f32 /* 0.8 */, compact_style: CompactStyle /* Summary */, repl: ReplConfig }
```

Config, complete:

```yaml
agent:
  max_tool_output_chars: 32000   # existing; the only cap on tool output (§6)
  compact_at: 0.8                # fraction of the model's context window; 0 disables auto-compaction (§8.1)
  compact_style: summary         # summary | cut | snap; default for auto, overflow, bare `/compact` (11)
  repl: { monty: null }          # 08 §10; not part of this doc
```

Three keys here (plus 08's one). jarvis has twenty-five.

Removed from the loop: per-round `system_prompt()` (→ per turn), the
`peers` roster fetch, `truncate` for model-facing output (→ `spill::cap`;
`truncate` stays for the UI copy).

## 12. Verification

Each item is a runnable check, not a suite. Results as of 2026-09-10 follow
each item.

1. **Prefix oracle** (unit test in `agent::frame`): build frames for a
   scripted session — user, assistant+tool_use, tool results, steering,
   assistant text, next user turn — and assert for every consecutive pair
   that `system` is equal, `tools` JSON is equal, and the JSON of the
   earlier `messages` array (minus its closing bracket) is a byte prefix of
   the later one, on all three transports. Then compact and assert the
   *new* epoch's frames satisfy the same property among themselves. This
   is the invariant as a test; any future code path that edits history
   fails it. **Pass** — `frame::tests::{prefix_oracle_across_rounds_and_epochs,
   transport_marks}`.
2. **Cut rules** (unit test in `agent::compact`): `keep_from` never
   returns an index whose message is `Tool`; the kept estimate is ≤
   `limit/8` whenever any valid cut fits; when none fits it returns the
   largest valid index (smallest tail); it returns 0 only when no valid
   cut exists in `1..n`. **Pass** — `compact::tests::{cut_rules,
   zero_only_when_no_valid_cut, overflow_needles}`.
3. **Cap** (unit test in `agent::spill`): output over the cap yields head
   + marker + tail within the cap, the file exists with the full content,
   the marker names the path; output at the cap is unchanged. **Pass** —
   `spill::tests::cap_spills_head_marker_tail`.
3b. **Todo** (unit test in `agent::todo`): `apply` auto-promotes the first
   pending item when nothing is in progress; `done` on an unknown text is
   `Err`; a 33rd item is `Err`. Prefix oracle (12.1) extended with a
   `todo` call and a `<todo>` note in the scripted session — same
   byte-prefix assertion, proving the reminder path never moves anything.
   **Pass** — `todo::tests::{auto_promotes_first_pending,
   unknown_task_and_cap_are_errors}`; the oracle's script includes both.
4. **Live cache ratio** (headless e2e, real Anthropic key): a turn that
   runs ≥ 5 tool rounds reports `cached / prompt ≥ 0.85` in
   `TurnFinished.usage`, and the log contains no `prefix cache miss`
   warning. **Observed** — a 5-cell `python` turn reported `cache 83%`
   over the turn (the first call of a fresh session has nothing to read,
   so a per-turn mean of five calls sits just under the 0.85 bar); the
   first call after `--resume` following a compaction reported `cache
   97%`; no `prefix cache miss` warning in the logs. The OpenAI repeat was
   not run.
5. **Compaction e2e** (headless, small `compact_at` in a throwaway
   config): a long turn triggers `Compacting` → `Compacted`; `load_session`
   shows the divider at the right position; `--resume` reproduces the
   post-compaction frame (the first call after resume within 5 min reports
   `cached > 0`). **Pass** — run at `compact_at: 0.02` on a 1M-window
   model: `Compacting` → `Compacted`; the heads carried the summary plus the
   todo list; `--resume` reproduced the frame (97 % cached on the first
   call); `load_session` interleaved `compacted(n)` dividers. Two manual
   `compact` calls over RPC returned epochs 3 then 4 and the dividers
   landed at the right positions. This run is what surfaced the `window/8`
   budget bug (§8.2).
6. **Rebase**: edit `prompt.md` between two turns → log line `rebase`,
   the next `TurnFinished` cache ratio dips once, the turn after is back
   above 0.85. **Not exercised.**

## 13. Implementation order (done)

Each step left `wth` runnable.

1. **Frame + marks + measurement.** `agent::frame` (`Frame`, `Transport`,
   per-turn `baseline`), `Usage.cached`, header `cache NN%`, miss warning,
   tools-changed warning, roster out of `PeerContext`. Checked: §12.1, §12.4.
2. **Spill.** `agent::spill::cap` replaces `truncate` in `round`; sweep at
   `AgentPlugin::start`. Checked: §12.3.
3. **Compaction.** `epochs` migration, `Store::{append RETURNING seq,
   compact, frame}`, `agent::compact`, trigger + cut + summariser + new
   epoch, overflow retry, `Compact` command (delegated, §8.6), `/compact`,
   RPC `compact`, events, `HistoryEntry::Compacted`, TUI divider,
   `Turn::note` + pressure note. Checked: §12.2, §12.5.
3b. **Todo.** `agent::todo` as a loop-local tool (§9), reminder note, list
   in the compaction head, `GUIDANCE` sentence. Checked: §12.3b; the head
   carried the list in the §12.5 run.
3c. **Compaction styles.** `CompactStyle`, `compact_style`, `/compact
   [summary|cut|snap|style …]`; overflow summariser-failure → `cut`; `snap`
   named, not built. See 11.
4. **Docs** (this pass). `00 §5` (resume rule → §8.4; tables gain
   `epochs`), `02 §3` header format, `02 §4` command list (`/compact`), `04
   §1.1` ("per round" → "per turn; see 07 §4"), `01 §2.5` (roster removed;
   see §10), `01 §1.4` (per-round tool list only while the runtime is
   off), README config block (`compact_at`, `compact_style`, `repl.monty`).

## 14. Rejected

- **In-place pruning of old tool results** (jarvis `compact.rs:635-720`).
  Every rewrite re-caches the rest of the history at 1.25× to save tokens
  that cost 0.1× cached. Bound output at append time instead (§6); the
  compaction fold is the only place old bodies leave the window.
  `cut` and `snap` (11) are not this: they build a new head and drain
  `history` the same way as `summary`; they are not in-place rewrites.
- **Model-driven shelve/recall tools.** Same invalidation, plus a second
  stub protocol. `read_file` on the spill path is the recall.
- **Sticky re-injection** (jarvis todos, harness status, steering banners
  kept "fresh" by removing the old copy and re-appending). Replaced by §9:
  the todo list is harness state; the model reads it from tool output,
  from rate-limited appended notes, and from the compaction head. Nothing
  is ever moved.
- **A `ContextSource`/`Registry` reconcile algebra** (`jarvis-context`).
  Elegant, and the right shape for nine independently changing sources
  rendered into the prefix. We have five inputs and render them into one
  string once per turn; `!=` on the string is the reconcile. Deltas as
  persisted, replayed System rows are the part that broke caching and are
  not wanted at any size.
- **A separate utility/summary model.** The cache-hot continuation (§8.3)
  makes the session model cheaper than a cold call to a small one, and it
  has seen the actual messages rather than a re-rendered transcript.
- **Local tokenizer (tiktoken) for budgets.** Wrong for every non-OpenAI
  model, allocates the whole history per estimate, and unnecessary when
  the provider reports the exact count after every call. `chars/4` on the
  appended slice plus an overflow retry is enough (§7).
- **Turn envelopes** (a per-turn user message with date/time and "skills
  added/removed" deltas, keeping the system prompt frozen for the whole
  epoch). Considered at length; it is a second mechanism for what a rebase
  already does at the same cache cost, and it leaks a clock into history.
  The system prompt re-render per turn with `!=` is one mechanism.
- **Anthropic explicit marks on the previous request's tail as well** (a
  third breakpoint). The 20-position lookback with tool runs counted as one
  position already covers a round; add it if §7's miss warning ever fires
  after a large steering burst.
- **OpenAI explicit breakpoints** (gpt-5.6+ `prompt_cache_breakpoint`).
  Implicit mode places one at the latest eligible message, which is our
  tail. Revisit when genai's mapping settles.
- **Cross-session shared system cache for parent + children** (§10).
  Precondition: `spawn_subagent` always listed and rejected at depth, the
  child identity line moved into the task wrapper. Small gain per spawn;
  do it when a transcript shows a fan-out of tens of children.
- **Persisting the baseline per epoch.** `/prompt` shows the live one;
  resume re-renders; a stored copy is a second source of truth to drift.
- **A `usable` floor / output reserve / protect-turn count.** One
  threshold on the raw window (§8.1) with the tool cap bounding a round is
  the same protection with one knob.
- **Keeping MCP tools as direct tool definitions when the runtime is on.**
  See `08-repl.md` §0.4: they are runtime functions; the direct list is
  native tools + `python`, which also removes the most likely mid-turn
  tools-list reset (§5).
- **Tail budget as `window / 8`** (the draft's §8.2). Changed after testing:
  with a small `compact_at` on a 1M window the tail budget exceeded the
  threshold, one message folded per compaction and every round compacted
  again. The budget is `limit / 8` with `limit = compact_at × window`.
- **`todo` as a registry tool that `ask`s the supervisor** (the draft's
  §9). Loop-local dispatch owns the state where it lives; no actor hop.
