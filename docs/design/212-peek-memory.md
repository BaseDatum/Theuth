# 212 — peek_memory: check memories against a question before pulling them into context

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
