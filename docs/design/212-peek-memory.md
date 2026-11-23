# 212 — peek_memory and set_status: memory peeks and a self-reported status line

Status: contract for the build (2026-11-23). This document is the fixed contract between the builders. Names, signatures, strings and file ownership below do not change without the orchestrator.

## 0. Why

Every `hint_rounds` rounds the harness injects a `<knowledge>` note listing up to `hint_limit` concept ids with one-line titles (10 §4.2, `crates/agent/src/loop_.rs` hint prelude, `knowledge::harvest::render_hints`). The model can only choose between ignoring a hint and calling `knowledge_get(id)`, which pulls the whole file (median ≈1.8 KB, up to 16 KiB) into context whether or not it helps. `peek_memory` puts a cheap local filter in between. The model names a question and the ids it is curious about. The local decision model (crate `decision`, gliner checkpoint, `Question::Bool`) judges each memory yes/no against the question. Only the memories judged relevant are returned in full; the rest come back as one line each.

```
<knowledge> nudge ──► model: peek_memory(question, ids)
                         │  per id: for_session → sees? → get → Bool(statement, state)
                         ▼
               decision::classify_bools (local, ~0.1–0.3 s/memory)
                         │
     relevant ──► full text in the result (counts as an open)
     not / unknown ──► one line: id, title, verdict, p
```

## 1. Ownership (no overlap)

| Builder | Owns | Does not touch |
|---|---|---|
| A (decision) | `crates/decision/**` | everything else |
| B (knowledge + nudge) | `crates/knowledge/**`, `crates/tools/src/builtins.rs`, `crates/agent/src/loop_.rs` (only the `knowledge_read` capture, §4.3), `crates/agent/src/context.rs` and `crates/agent/src/store/dreaming.rs` (tests only, if a fixture needs the new sentence), `crates/agent/src/loop_/meeting_diet_tests.rs` (only if needed), `crates/theuth/harness.md`, `docs/design/10-knowledge.md` (tool table row) | `crates/decision/**` |

B adds `decision = { workspace = true }` to `crates/knowledge/Cargo.toml` (no cycle: `knowledge → document → decision` already exists). A commits §2 first, before polishing tests, so B can compile against it; until then B codes against the signature below.

## 2. Decision helper (builder A) — new file `crates/decision/src/classify.rs`

A public in-process batch classifier for yes/no questions, the reusable core of what `Decide::run` does for `decide_many`, without the tool layer.

```rust
/// One yes/no question: does `statement` hold for `state`?
#[derive(Debug, Clone)]
pub struct BoolItem {
    pub statement: String,
    pub state: String,
}

/// The answer for one item, in input order.
#[derive(Debug, Clone, PartialEq)]
pub struct BoolVerdict {
    /// P("true") from the model; `None` when the row failed.
    pub p_true: Option<f32>,
    /// The model's choice; `None` when the row failed.
    pub choice: Option<bool>,
    /// The state was cut to fit the model window.
    pub truncated: bool,
    /// Why this row has no answer (model missing, timeout, disabled, hard error).
    pub error: Option<String>,
}

/// Classify `items` with the engine of `checkpoint` (e.g. "gliner") under `family`
/// (e.g. "tool.memory"; an unconfigured `tool.<x>` behaves as `tool`, as decide_many does).
/// `budget_ms` is per chunk. Rows are sorted by length and chunked by the tool batch size
/// (8) internally; results come back in input order.
/// Err only when nothing can run at all (no engine for the checkpoint, decision disabled);
/// per-row failures are `BoolVerdict { error: Some(..), .. }`.
pub async fn classify_bools(
    checkpoint: &str,
    family: &str,
    items: Vec<BoolItem>,
    budget_ms: u64,
) -> anyhow::Result<Vec<BoolVerdict>>;
```

- Re-exported as `decision::classify::{BoolItem, BoolVerdict, classify_bools}` (`pub mod classify;` in `lib.rs`).
- Engine lookup as in `meeting/src/oracle.rs`: `ActorRef::<Engine>::lookup(actor_name(checkpoint))`. Missing → `Err(anyhow!("decision model unavailable: checkpoint engine `{checkpoint}` is not running"))`. Empty `items` → `Ok(vec![])` without a lookup.
- `p_true` is the `"true"` entry of `Decision::probabilities`; a disposition of `Escalate(reason)` with empty probabilities becomes `error: Some(reason as text)`.
- Model release: mirror `decide_many` (release after the batch unless another live non-tool family holds the checkpoint). If that logic is only reachable through `Decide`/config, factor the minimum out so both paths share it; if that is not possible without config, rely on `idle_unload` and say so in the final report.
- Refactor `Decide::run` to use the shared pieces only if it is a small, safe change; do not change `decide_many` behaviour or output.
- Tests: unit tests for ordering/chunk/reassembly and error mapping that do not need model files (factor the pure parts so they are testable). No test may load a real model.

## 3. The tool (builder B) — `crates/knowledge/src/tools.rs`

### `peek_memory{question, ids}`

- `NAME = "peek_memory"` (export `pub const PEEK: &str = "peek_memory";` next to `GET`), `RISK = ReadOnly`, `DIRECT = false` (a runtime function, like `knowledge_get`), `CONSUMES_SECRETS = false`, `RETURNS = "str"`.
- Args (schemars, doc comments are the model-visible schema):
  - `question: String` — what you want to know; 1–500 chars after trim, else error `peek_memory: question must be 1–500 characters`.
  - `ids: Vec<String>` — concept ids from a `<knowledge>` hint or `knowledge_search()`; 1–12 after trim and dedupe (order kept), else error `peek_memory: give 1–12 ids`. `@commit` revisions are refused per row (`not peekable: revisions are read with knowledge_get`).
- Description (first sentence is the runtime-index summary): `Check memories against a question before pulling them into context: the local decision model reads each named concept and answers yes/no "does this help answer the question"; only the relevant ones come back in full, the rest as one line each. Use it on <knowledge> hint ids or knowledge_search() results when you are not sure a memory is worth reading; use knowledge_get(id) when you already know you need it.`
- Flow:
  1. `let k = self.knowledge.for_session(cx.session).await?;` (fails closed for isolated memory, design 196).
  2. Per id: `k.get(id)`; missing → row `missing`. Visibility: apply the session's `AgentScope::sees(scope, owner, visibility)` (add a crate-internal helper if `scope_for` is needed); not visible → row `missing` (same text — never reveal existence).
  3. State text = `title` + `\n` + `description` + `\n\n` + body without frontmatter, cut to 4096 bytes at a char boundary. Statement = `This memory helps answer the question: {question}`.
  4. `judge.judge(items).await` → `Vec<BoolVerdict>` (see seam below). Default judge calls `decision::classify::classify_bools(cfg.peek_checkpoint, "tool.memory", items, 3000)`.
  5. Verdict per row: `relevant` when `choice == Some(true)` and `p_true >= cfg.peek_threshold`; `not relevant` when the model answered otherwise; `undecided` when the row errored or the whole call returned `Err` (fail closed: undecided memories are NOT pulled in).
  6. For every `relevant` row: `record_opened(id, now)` unless `is_consolidating_session` (same rule as `GetTool`).
- Test seam: the tool struct holds `Arc<dyn MemoryJudge>`:
  ```rust
  #[async_trait::async_trait]
  pub trait MemoryJudge: Send + Sync + 'static {
      async fn judge(&self, items: Vec<decision::classify::BoolItem>)
          -> anyhow::Result<Vec<decision::classify::BoolVerdict>>;
  }
  ```
  `register()` wires the real one (`DecisionJudge { checkpoint, threshold }` or equivalent); tests use a fake.

### Receipt (`content`, exact format)

```
peek_memory: 2 of 5 relevant to "how is the build lock acquired"

### `procedure/theuth-build-lock-tmp-mkdir` — relevant (p=0.91)
<the file verbatim, exactly as knowledge_get returns it, incl. Links:/Cited by: lines>

### `procedure/detached-gate-script-for-the-theuth-build-lock` — relevant (p=0.74)
<…>

Not pulled in:
- `facts/x` — Title of x — not relevant (p=0.12)
- `facts/y` — Title of y — undecided (decision model unavailable: …); knowledge_get(id) reads it anyway
- `facts/z` — missing
```

- The headline counts relevant rows over distinct ids requested; the question is quoted, clipped to 80 chars with `…`.
- Relevant sections are in request order; `p` has two decimals. When no row is relevant the line `No memory was judged relevant.` replaces the sections.
- "Not pulled in:" is omitted when empty. `missing` rows have no title. All rows undecided because the model is unavailable → one extra line at the end: `The local decision model is unavailable; read with knowledge_get(id) instead.`
- `structured: None` (the python value is this string, like `knowledge_get`).
- Factor the verbatim-file rendering out of `GetTool` into one function both tools call, so the two never drift.

### Config — `KnowledgeConfig` in `crates/knowledge/src/lib.rs`

- `peek_threshold: f32` (default `0.5`): minimum P(true) for "relevant". The gliner Bool is uncalibrated; 0.5 means "the model said yes".
- `peek_checkpoint: String` (default `"gliner"`).
- Serde defaults so existing config files still load; add commented defaults next to the other `knowledge:` keys in `crates/theuth/src/main.rs` DEFAULT_CONFIG only if that block already lists `KnowledgeConfig` keys (B may edit those two lines; nothing else in `main.rs`).

### Registration and category

- `reg.ask(RegisterTool(Arc::new(PeekTool{..})))` in `knowledge::tools::register`.
- `crates/tools/src/builtins.rs`: add `"peek_memory"` to the knowledge category's `exact` list and to the test table.

## 4. The nudge and the prompt (builder B)

### 4.1 Hint lead sentence — `harvest::render_hints`

Old: ``Concepts that may be relevant; read one with `await knowledge_get(id)` in python before relying on it:``

New (one line; must not start with `` - ` ``):

``Concepts that may be relevant; not sure one helps? `await peek_memory(question="…", ids=[…])` checks them against your question locally and returns only those that answer it; `await knowledge_get(id)` reads one outright (python):``

Update `hints_clip_description_and_omit_empty` and any test that pins the full sentence. The context-view card label (first 80 chars) still starts `Concepts that may be relevant`; tests that use the short form stay green. Do not rewrite replay fixtures under `crates/agent/tests/fixtures/endless/`.

### 4.2 System prompt paragraph — `knowledge::tools::PROMPT`

Append one sentence to the existing constant (it stays a constant): ``Before reading hinted concepts you are unsure of, `peek_memory(question, ids)` (python) asks the local model which of them answer your question and returns only those in full.``

### 4.3 Compaction head — `loop_.rs` `knowledge_read` capture

Where `state.knowledge_read` collects ids from `knowledge_get` calls (filtered on `knowledge::tools::GET`), also collect the relevant ids of a `peek_memory` call by parsing its result lines `^### \`(<id>)\` — relevant`. No other change in `loop_.rs`.

## 5. Tests

- A: §2 unit tests, no model files.
- B, in `crates/knowledge`, with the fake judge:
  1. two relevant + one not + one missing → exact receipt above (golden string), `record_opened` called for exactly the two relevant ids;
  2. judge `Err` → all undecided, nothing pulled in, unavailable line present;
  3. arg validation errors (empty question, 0 ids, 13 ids, dedupe keeps order);
  4. a private concept of another owner is `missing` for an agent-scoped session;
  5. `@commit` id refused per row;
  6. threshold: `choice=true, p=0.4` with threshold 0.5 → not relevant;
  7. `render_hints` new sentence; the `knowledge_read` parse in `loop_.rs` (unit test on the parser function).
- Orchestrator gate: `cargo test -p decision -p knowledge -p tools`, the context/dreaming/meeting_diet tests in `agent`, `cargo clippy -p decision -p knowledge --no-deps`.

## 6. Not in this build (open)

- Capturing peek verdicts for `decide_label` training (`tool.memory` family) so the judge learns from what the model then actually used.
- Calibrating `peek_threshold` on a labelled sample of real hint→question pairs.
- Auto-peek: the harness running the judge on its own hints against the turn's question before showing them.


---

# Part 2 — set_status: the agent keeps its own status line and title

Added 2026-11-23 at the owner's request (same contract rules as above). The model reports what it is doing in a short status line that replaces the generic "thinking…/running tool…" word above the composer. It may also retitle its session in 3–5 words. A harness nudge reminds it every few tool calls. Once a session's agent has used `set_status`, the title model never regenerates that session's title again.

```
tool calls ──► counter ≥ status_every ──► <nudge> "status check …"
model ──► set_status(status, title?) ──► state.status, store title, agent_titled=1
      ──► AgentEvent::StatusChanged ──► desktop footer / TUI activity / Slack task line
      ──► AgentEvent::TitleChanged (existing) ──► sidebar, header
title model (maybe_title) ──► returns early when agent_titled or user_titled
```

## 7. Ownership (part 2)

| Builder | Owns |
|---|---|
| C (engine) | `crates/agent/**` (including `loop_.rs`, but only after builder B's `loop_.rs` commit is in `git log -- crates/agent/src/loop_.rs`; until then build everything else), the `Toolset` literals in `crates/autoresearch/src/binding.rs`, `crates/meeting/src/lib.rs`, `crates/meeting/src/calendar/prebrief.rs`, `crates/rpc-compose/tests/**` + `crates/rpc-core/src/stream_tests.rs` (fixtures/Replace lists), `crates/theuth/src/main.rs` (DEFAULT_CONFIG comment only), `crates/theuth/harness.md` (one line, after B's line), `crates/theuth/tests/fixtures/wire/openrpc-v1.json` (via `just wire-schema-accept`) |
| D (clients) | `crates/desktop/ui/src/**`, `crates/tui/**`, `crates/slack/**`, `crates/telegram/**`, `crates/signal/**`, and any other crate whose exhaustive `match` on `AgentEvent` breaks (ignore arm only) except `crates/agent` |

`graph.rs` and `roster` matches inside `crates/agent` are C's. D codes against the event in §9 before C lands it; D compiles once C's first commit (the event variant) exists.

## 8. The tool — loop-local, like `todo`

`set_status` is a loop-local direct tool (same shape as `todo`: spec in a new `crates/agent/src/status.rs`, listed via `listed(status::NAME)`, dispatched in the batch executor, bridged into python like `repl.todo`). It is direct because the nudge asks for it by name and it must be cheap; it is also callable from python as `set_status(...)`.

- `NAME = "set_status"`, read-only (it changes only this session's own presentation).
- Args:
  - `status: String` — what you are doing right now, 3–12 words, ≤ 80 chars after trim (longer is cut at a word boundary with `…`; empty is an error `set_status: status must not be empty`).
  - `title: Option<String>` — the session's overall task in 3–5 words, ≤ 48 chars after trim; omit unless the task changed or the current title no longer says it. Fewer than 2 or more than 6 words → error `set_status: title must be 3–5 words`.
- Description: `Update the one-line status the user sees above the chat input while you work (3–12 words, what you are doing now), and optionally retitle this session (3–5 words, the overall task). Call it when you start a new phase of work and whenever the harness asks for a status check.`
- Effect:
  1. `state.status = Some(status)`; `status_touched` counter reset (§10).
  2. Publish `AgentEvent::StatusChanged { session, status, title }` (§9).
  3. If `title` is given and the session is not `user_titled`: `store.set_title(...)` keeping tags, publish the existing `AgentEvent::TitleChanged`, mark `Session.titled = true`.
  4. Set `agent_titled = 1` for the session (persisted, §11) on the first call, whether or not a title was given.
- Result text (exact): `status set` or `status set · title "<title>"` or `status set · title kept: the user named this session`.
- Python bridge: like `todo`, a call from a cell updates the loop state after the cell and counts as touched.

## 9. Wire event

New variant on `AgentEvent` (topic `agent.events`):

```rust
/// The agent's own one-line status (design 212 §9); replaces the generic activity word
/// in clients until the turn ends.
StatusChanged {
    session: SessionId,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
},
```

- `policy()` → `Delta::Replace` (like `VisionProgress`); `session()` arm; every exhaustive match in `crates/agent` gets an arm (graph `activity_sessions` treats it as activity; roster ignores it).
- Fixtures: extend `crates/rpc-compose/tests/fixtures/wire/agent.events.json`, the Replace lists in `wire_parity_inc4.rs` and `rpc-core/src/stream_tests.rs`; `just ts-desktop`; `just wire-schema-accept`; `cargo test -p theuth --test wire_schema_snapshot`.

## 10. The nudge

- Counter `state.tool_calls_since_status: u32`: +1 per direct tool call and +1 per python inner tool call in a batch (`set_status` itself and `todo` excluded); reset to 0 by `set_status` and at turn start, along with `state.status = None`.
- In the round prelude (next to the `<todo>` reminder), when the session gets the nudge (below):
  - **first nudge:** no `set_status` yet this turn and `tool_calls_since_status >= 2`;
  - **then:** `tool_calls_since_status >= status_every`;
  - at most one status nudge per round; after a nudge the counter is reset so an ignored nudge repeats only after another `status_every` calls.
- Note: `self.note("nudge", text)` (the `nudge` kind is already a harness note everywhere). Text (exact):
  - `Status check: call set_status(status="…") with what you are doing now, 3–12 words. Add title="…" (3–5 words) if this session's overall task changed or its title no longer says it.`
  - when `user_titled`: the first sentence only.
- Who gets it: `chat` sessions (any depth) where `set_status` is listed, `status_every > 0`, not a consolidating (dreaming) session.
- Config: `agent.status_every: u32`, default `8`, `0` = off (field on the agent config struct, serde default). `Toolset.status_every: Option<u32>` overrides per session; `Some(0)` in the advisor, autoresearch, prebrief and meeting-live toolsets.
- Prompt: one constant sentence in the agent's tool guidance: `Keep the user informed: call set_status when you start a new phase of work; the harness reminds you every few tool calls.`

## 11. Title guards (persisted)

- Migration `crates/agent/migrations/0076_session_title_guard.sql`: engine-local side table `session_title_guard(session TEXT PRIMARY KEY, agent_titled INTEGER NOT NULL DEFAULT 0, user_titled INTEGER NOT NULL DEFAULT 0)` — not in `capture::ATTACHED`, so the replicated `sessions` schema does not change.
- Loaded at session load into `shared` atomics next to `title_pinned`.
- `agent_titled`: set by the first `set_status` call (§8). **`maybe_title` returns early when `agent_titled`** (next to the `title_pinned` check), so the title model stops for that session for good, including after an engine restart.
- `user_titled`: set by the `RenameSession` handler. `maybe_title` also returns early when `user_titled` (fixes the title model overwriting a user rename), and `set_status` never replaces a user's title.

## 12. Clients (builder D)

- **Desktop:** `SessionSlice.statusText?: string`, `statusTitle?` set by `StatusChanged`; cleared on `TurnStarted`, `TurnFinished`, `Error` and reseed. The footer in `Transcript.tsx` renders `statusText ?? (vision line) ?? WORD[activity]` with the existing `ActivityIndicator` glyph; `activity` is unchanged. The sidebar `SessionItem` activity glyph's tooltip/aria-label shows `statusText` when set.
- **TUI:** `model/status.rs` `activity()` returns the status first (after `compacting…`); the trailing row in `ui.rs` shows it while the turn runs (keep the spinner).
- **Slack:** the in-progress task title of the live bubble shows the status instead of `thinking`; no new posts.
- **Telegram / Signal / others:** ignore arm.
- Tests: reducer test (set, cleared on TurnFinished), TUI status test, slack bubble test for the task title.

## 13. Tests (builder C)

1. `set_status` sets state, publishes `StatusChanged`, sets `agent_titled`, writes title + `TitleChanged` when given.
2. `user_titled` session: title ignored, result text `title kept`.
3. `maybe_title` returns early for `agent_titled` and for `user_titled` (unit test on the guard).
4. Nudge cadence: first nudge after 2 calls without status; then every `status_every`; reset by `set_status`; none when `status_every = 0`, toolset `Some(0)`, or tool not listed.
5. Python bridge: a cell calling `set_status` updates state and resets the counter.
6. Wire: fixture + schema snapshot green.
