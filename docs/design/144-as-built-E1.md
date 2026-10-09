# 144 as built — E1 (ENGINE-CONTEXT: the context dial, the pre-turn estimate, default insights, insight tools)

Worker 144-E1, branch `feat/144-e1`, cut from `feat/144-w2-base` 6ab1905f. Brief: `144-briefs/wave2/E1.md`;
contract: `docs/design/144-wave2-contract.md` §1; design §4.3, §5.1–5.3, §13.

## What landed (file:line)

### Storage (§5.1 W-g)
- `crates/agent/migrations/0045_session_context.sql`: the contract's `CREATE TABLE session_context (session, item,
  kind, level, hlc, PRIMARY KEY (session, item))`. Not in `store/capture.rs::ATTACHED`, so never captured or
  replicated (owner-only data). `hlc` = unix ms.
- `crates/agent/src/store.rs` `Store::context_set` (upsert on `(session, item)`), `context_clear`, `context_get`
  (`(item, kind, level)` rows, `ORDER BY hlc, item`) — beside `notebook_chat_counts`, on the single write connection
  like `set_notebook`.
- `crates/agent/src/notebook_context.rs` (new): `Agents` messages `ContextSet { session, item, kind, level }`,
  `ContextClear { session, item }`, `ContextSetAll { session, items: Vec<(item, kind)>, level }`,
  `ContextGet { session } → Vec<(item, kind, level)>`, `SessionTitles(Vec<SessionId>)`, `WakeSession(SessionId)`;
  the RPC bodies `context_set`, `context_clear`, `context_set_all(agents, session, notebook, kind?, level)` (resolves
  the notebook's items through the registry's `notebook_get` as the session, then one message), `pins`/`pins_of`.
  Levels are validated (`off|mention|summary|full`), kinds too; sessions and artifacts are clamped to `mention` at
  write time and again at plan time.

### The default rule, the planner, the estimate (§5.1–5.2)
- `notebook_context.rs::plan(items, pins, budget) -> Vec<Placed>` — the one source of truth. Effective level =
  pinned (clamped) else `summary` for a source whose annotated markdown has a `## Insights` heading, `full` for other
  sources and notes, `mention` for sessions/artifacts. Items are taken newest first per level in the order full →
  summary → mention → off; the running total of the injected text (chars / 4) stays under the budget; the first
  overflow is `truncated` at a `<!-- id -->` boundary with the §5.3 notice; later overflows fall to `mention` /
  `omitted`; `off` → `omitted`, 0 tokens, not rendered. Text per level: mention = the one line; summary = the line +
  the `## Insights` section's blocks (ids kept), or — a note, or a source without insights — its first heading and the
  400 chars after it; full = the whole annotated markdown. Output is in injection order (a demoted item sorts among
  the mentions).
- `estimate_of(placed, budget) -> Estimate { items, total_tokens, budget_tokens, omitted, empty }` and
  `estimate(agents, session: Option, notebook, budget)`: pins via `ContextGet` (none when `session: None`), items via
  `notebooks::fetch(None, session-or-fresh-id, notebook)` (the same registry reads the block makes), session titles
  via `SessionTitles`, then `plan`. No model call, nothing cached.
- `crates/agent/src/notebooks.rs`: `fetch(store: Option<&Store>, …)` (`pub(crate)`; the store only names session
  items), `block()` reads `store.context_get(session)` and passes the pins; `render(id, title, description, items,
  pins, budget)` renders `plan`'s output: `<item kind id title [origin] level status>` with `p.text`, `off` items
  absent, the allow-list sentence over the rendered ids in injection order, else
  `nothing in context — the agent sees only the description; there are no ids to cite.`; `<description>` cap
  unchanged. `PROMPT` names `summary` and `status="omitted"` and says switched-off items are not listed.

### Default insight at ingest (§4.3) and the insight tools
- `crates/document/src/ingest/mod.rs` `NotebooksConfig` gains `insight_model: Option<String>` (None),
  `insight_default: bool` (true), `insight_input_tokens: u32` (24 000).
- `crates/document/src/ingest/insights.rs` (rewritten): `Hooks { side_call: SideCaller, skill: SkillResolver }`
  installed once by `install()` (a `OnceLock`, the `codeindex::map::generator::install_side_caller` pattern);
  `InsightCall { source, notebooks, session, model, system, user, budget }` / `InsightReply { text, model }`.
  `after_ingest(ctx, source)` (spawned by `job.rs:76` after `completed`): when `insight_default`, one call with
  system = `DEFAULT_PROMPT` (the §4.3 wording verbatim) and user = the source's own markdown (its earlier `## Insights`
  stripped) cut to `insight_input_tokens × 4` chars; skipped when a `### Dense summary ·` section exists (a same-text
  refresh never adds a second); a failed call → `meta.insight_error` on the row + `Changed{Completed}`, the ingest
  stays `completed`. `append()` inserts `## Insights` (when absent) + `### <heading> · <model> · <YYYY-MM-DD>` + text
  at the end as `Principal::Helper` through `Edit{Insert at end}`, bumps `sources.insights`, clears `insight_error`,
  emits `SourceEvent::Changed { Completed }`, returns the `###` block id. `BUILTINS` = `key_insights` / `toc` /
  `open_questions` with the brief's instructions verbatim; `resolve(InsightInstruction)` → builtin or the installed
  skill resolver (exactly one of the two). `IngestActor` messages `RunInsight { source, instruction, session }` and
  `PromoteInsight { source, block, notebook, by }` (hub-gated, work spawned off the mailbox); entry points
  `insight_run(actor, source, instruction, session) -> InsightRun` and `insight_promote(actor, source, block,
  notebook?, by) -> DocId`. `promote` = `section_of(annotated, block)` (the `###` section holding the block, under
  `## Insights`) → `CreateNote { title: "<insight> — <source title>", markdown: the section's blocks + "\n\n*From
  [<source title>](theuth://sources/<id>#<block>) · <model> · <date>*" }` → `NotebookItemAdd` into `notebook` or
  every live notebook holding the source. Pure helpers: `has_insights`, `has_section`, `strip_insights`,
  `section_of`, `split_heading`.
- `crates/document/src/ingest/job.rs:76`: the one call site, `tokio::spawn(insights::after_ingest(ctx.clone(), id))`.
- `crates/document/src/notebook.rs:351-412`: `ContextItem { id, kind, title, level, pinned, tokens, status }`,
  `ContextEstimate { items, total_tokens, budget_tokens, omitted, empty }` (contract §1 fields, in place of wave 1's
  unused placeholders), `InsightInstruction { builtin?, skill? }`, `InsightRun { block }` appended.
- `crates/document/src/nb_tools.rs:527-611`: tools `insight_run(source, instruction)` → `{block}` and
  `insight_promote(source, block, notebook?)` → `{doc, url}` (Mutating), registered beside `nb_search`.
- `crates/agent/src/notebook_context.rs`: tools `notebook_context_set(item, kind, level, session?)` and
  `notebook_context_estimate(notebook, session?)` (ReadOnly; `session` defaults to the caller), registered from
  `AgentPlugin::start` (`lib.rs:3439` `notebook_context::register(rt)`). `insight_skills(dir)` (skills whose front
  matter has `notebook_insight: true`, via `skills::index` + a `gray_matter` parse of that one key),
  `insight_skill(dir, name) -> (heading = description or name, instruction = body)`.
  `insight_side_call(agents, session?, notebooks, model?, system, user, budget)`: `ModelRole::Named(model)` else
  `ModelRole::Title` (→ `agent.title_model` → the session model); the call is charged to the caller's session, else to
  the newest chat bound to one of the notebooks (woken through `WakeSession` when idle), else to a new durable chat
  bound to the first notebook (`CreateSession { notebook }`).
- `crates/agent/src/supervisor.rs:1410`: `Agents::actor` is `pub(crate)` (the `WakeSession` handler), like W3's
  `store_for`.
- `crates/rpc/src/api.rs`: `notebook_context_set`, `notebook_context_clear`, `notebook_context_set_all`,
  `notebook_context_estimate` (owner-only; the agent's `Estimate` serialises onto `document::ContextEstimate`),
  `insight_run`, `insight_promote` (owner-only, hub-only with `doc_forward`), `insight_skills`;
  `Server::new` calls `install_insight_hooks(rt)` (the side caller → `agent::notebook_context::insight_side_call`,
  the skill resolver → `insight_skill`). `export.rs` lists `InsightInstruction`, `InsightRun`.

### Tests
- `crates/agent/tests/notebooks_binding.rs::session_context_rows_round_trip_per_session` (migration applies,
  set/overwrite/clear per session, `pins_of` clamps and drops bad rows).
- `crates/agent/src/notebook_context.rs` unit tests: insights section / teaser extraction; the default rule, pins,
  clamps, `estimate_of` (omitted, empty, totals); budget cut newest first then demotion with the exact notice.
- `crates/agent/tests/notebooks_block.rs::dial_levels_render_in_order_off_is_not_citable_and_the_estimate_matches`
  (exact block with full/summary/mention, `off` absent from block and allow-list, estimate tokens = injected text).
- `crates/document/src/ingest/insights.rs` unit tests: `section_of` from the heading or an inner block, heading
  split, detection/strip, `resolve` refusals.
- `crates/document/tests/insights.rs` (helper-gated like `ingest_job.rs`; hooks mocked): default insight appended
  once, counted, two `Changed{Completed}` events, same-text refresh adds nothing, a failing model → `insight_error`;
  `insight_run` with `toc` and the `legal-review` skill (one `## Insights`, block ids, counts, `insight_model`
  forwarded), the four refusals; `insight_promote` note title/body/trailer/membership, explicit notebook, unknown block.

## Proof

## Gates

## Deviations
- **Wave-1 tests updated** (`crates/agent/tests/notebooks_block.rs`): the four `render` calls pass `&Pins::new()`;
  `block_shape_order_and_allow_list` — the allow-list now lists ids in injection order (`S1, N1, 01SESSION`, was item
  order); `description_is_capped…` — the empty sentence is the contract's `nothing in context — the agent sees only
  the description` (was `Nothing in context — you see only the description`). Everything else unchanged.
- `ContextItem` / `ContextEstimate` in `document/src/notebook.rs` were wave-1 placeholders (`{kind, item, level,
  status, tokens}` / `{items, tokens, budget, omitted}`) never read by any crate or UI file; they are replaced in
  place by the contract's field sets rather than appended as second types. TS name stays `NotebookContextItem`.
- `ContextSetAll`'s message carries resolved `(item, kind)` rows instead of `notebook` + `kind?`: the agent store has
  no notebook items, so the public fn `context_set_all(agents, session, notebook, kind?, level)` resolves them through
  the registry (as W3's block does) and the actor handler only writes. The RPC/tool shapes are the contract's.
- Budget-cut items keep wave 1's shape — `level="mention" status="omitted"`, rendered and citable (the model can
  still `doc_read` them; the design's §5.3 example lists S2); only `off` items are absent from the block and the
  allow-list. `omitted` in the estimate counts both.
- A `summary` pin on a source without insights (or a note) injects the first heading + 400 chars (the note rule).
- Items beyond the budget keep consuming it by their mention line (so the total the composer shows is what the block
  costs); this never changes which items fit.
- Rows are keyed by the session id given (no root resolution): the UI dials the thread it shows; `block()` reads the
  turn's own session id. Rows are not deleted with the session (no FK); hand-off below.
- `after_ingest` is spawned instead of awaited inside the job so the card never shows `queued` again while the model
  answers (the ingest actor keeps the source in `active` until the job returns).
- The side call needs a live session actor (`Agents`' `SideCall` handler): `insight_side_call` wakes the newest chat
  bound to a notebook holding the source (`WakeSession` → `Agents::actor`, now `pub(crate)`), and when the notebook
  has no chat yet it creates one bound to it (`CreateSession { notebook }`) — the thread the user opens anyway, and the
  cost owner. `CallKind` has no insight variant (`runtime/src/side_call.rs`, not mine): the call is tagged
  `CallKind::Title` — the same cheap slot. Hand-off below for a `CallKind::Insight`.
- The hooks are installed from `rpc::api::Server::new` (the crate that knows both `agent` and `document`), not from
  `crates/theuth/src/main.rs` (not mine). `Server::new` runs once per process (the TUI's in-process server included);
  a second install is a no-op.
- `document` gains `chrono = { workspace = true }` (the `· <date>` in headings and trailers; the workspace already
  pins it) — one Cargo line instead of a hand-rolled civil-date routine.
- The three insight config keys live on `document::ingest::NotebooksConfig` (W1's "W2/W3 add theirs here"), not in
  `agent/src/lib.rs`; `context_budget_tokens` stays on `agent::notebooks::Budget` (wave 1).
- `insight_run` as a tool passes the caller's session as the side call's cost owner; the RPC passes `None` (the UI has
  no session) and the notebook-chat rule applies.
- `insight_promote` strips the `<!-- id -->` lines and the `###` heading from the promoted blocks (the note's title
  carries the heading); the trailer's `<model>`/`<date>` are parsed from the heading (`split_heading`).

## Hand-offs
- **U1 — TS shapes** (hand-written stubs until the closer regenerates; file names as ts-rs will emit them):
  ```ts
  // src/rpc/bindings/ContextStatus.ts
  export type ContextStatus = "available" | "truncated" | "omitted";
  // src/rpc/bindings/NotebookContextItem.ts   (Rust `document::ContextItem`, ts(rename))
  export type NotebookContextItem = { id: string, kind: ItemKind, title: string, level: ContextLevel,
    pinned: boolean, tokens: number, status: ContextStatus };
  // src/rpc/bindings/ContextEstimate.ts
  export type ContextEstimate = { items: Array<NotebookContextItem>, total_tokens: number, budget_tokens: number,
    omitted: number, empty: boolean };
  // src/rpc/bindings/InsightInstruction.ts
  export type InsightInstruction = { builtin: string | null, skill: string | null };
  // src/rpc/bindings/InsightRun.ts
  export type InsightRun = { block: string };
  ```
  Methods (positional params, `with_extensions` = owner-only): `notebook_context_set(session, item, kind, level) →
  null`, `notebook_context_clear(session, item) → null`, `notebook_context_set_all(session, notebook, kind | null,
  level) → null`, `notebook_context_estimate(notebook, session | null) → ContextEstimate`, `insight_run(source,
  instruction) → InsightRun`, `insight_promote(source, block, notebook | null) → DocId (string)`, `insight_skills() →
  string[]`. After `insight_run` / the default insight a `SourceEvent { type: "Changed", source, status: "completed" }`
  arrives on `subscribe_events` — re-run the estimate on it. `ContextEstimate.items` is in injection order; `empty`
  drives the "nothing in context — the agent sees only the description" line before send.
- **Closer**: regenerate bindings (`mbx test -p rpc --features ts --lib export_bindings` + the copy line);
  `methods.json` gains the seven names above.
- **runtime (`side_call.rs`)**: a `CallKind::Insight` (`"insight"` in `as_str`/`FromStr`) so insight cost rows stop
  reading `title`; then `notebook_context::insight_side_call` sets `kind: CallKind::Insight`.
- **agent store**: `session_context` rows are not removed when a session is deleted; a `DELETE FROM session_context
  WHERE session = ?` beside the session delete would keep the table tidy.
- **E2 / `NotebookDetail.items[].title`** (Z's hand-off stands): the estimate and the block still make one
  `source_get` / `doc_read` per item through the registry.
