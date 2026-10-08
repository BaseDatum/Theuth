# 144 as built — W3 (CITE+CHAT engine)

Worker 144-W3, branch `feat/144-w3`, cut from the contract skeleton `13bd3cc3`. Notebook-bound sessions, the
`<notebook>` block, the citation contract and `cite_check`, save/promote (design 144 §5, §6.2, §6.6, §8).

## What landed (file:line)

### Binding (§8 W-k)
- `crates/agent/migrations/0041_session_notebook.sql`: `ALTER TABLE sessions ADD COLUMN notebook TEXT NULL;` (root
  row only, like `project` in 0015).
- `crates/agent/src/store.rs:399` `SessionRow.notebook`; `session_cols!` selects `root_col!("notebook") AS notebook`
  (children inherit); `summary()` fills it; `Store::set_notebook` (:2621, root-resolving `UPDATE … RETURNING id`,
  unknown session is an error), `Store::effective_notebook` (:2647), `Store::notebook_chats` (:2660 — roots only,
  `ORDER BY created_at DESC, id DESC`).
- `crates/agent/src/lib.rs:2072` `SessionSummary.notebook: Option<String>` (`#[serde(default)]`, ts-rs exported with
  the struct). `crates/agent/src/lifecycle.rs:32` `CreateSessionOptions.notebook`; `supervisor.rs:9504` binds it
  after `create_durable` (durable roots only; a private/harness root ignores it).
- `crates/agent/src/notebooks.rs`: `SetSessionNotebook` / `NotebookChats` messages on `Agents` (:111, :131 — the
  handlers live here, not in `supervisor.rs`; `Agents::store_for` became `pub(crate)` for that), the four RPC
  bodies `notebook_chats` (:148), `session_set_notebook` (:159), `save_answer_as_note` (:173),
  `promote_answer` (:202). RPC `owner(ext)` already guards all four (C's `api.rs`).

### The `<notebook>` block (§5.3) and the `notebooks` prompt block (§6.2)
- `notebooks.rs:24` `PROMPT` (the §6.2 wording + what the block is, ~130 tokens); `:35` `prompt_blocks(blocks, bound)`
  — byte-identical outside a notebook, `+ "\n\n" + PROMPT` inside one. `loop_.rs:4114` (`Turn::rebase`) routes the
  plugin blocks through it with `bound = store.effective_notebook(id)`; a (un)binding mid-session changes the baseline
  like any other block (prefix reset).
- `notebooks.rs:46` `Budget { context_budget_tokens: 40 000, description_tokens: 2 000 }` from `rt.section("notebooks")`
  (§14; other keys ignored). `:75` `Item`; `:342` `block(store, session, notebook, budget)` fetches through the tool
  registry as the session (`notebook_get`, `doc_read("notebook:<id>", markdown)`, per item `source_get` +
  `doc_read("source:<id>")` / `doc_read(<note>)`; session titles from the store) and renders; a notebook that cannot
  be read still yields a block saying so. `:498` `render(...)`: `<notebook id title>`, `<description>` ≤
  `description_tokens` with `[truncated: n of m tokens shown; the rest with doc_read("notebook:<id>")]`, one
  `<item kind id title [origin] level status>` per item; notes and sources `full` newest first while under the budget,
  the first overflow `truncated` at a `<!-- id -->` boundary with
  `[truncated: n of m tokens shown; the rest with doc_read(blocks=[…]); first omitted block <id>]`, the rest
  `mention`/`omitted` (`mention only: N words; read with doc_read("…") if needed`); sessions/artifacts
  `mention`/`available`; order full → mention, newest first within a level; last line `You may cite only these ids:
  … (and the blocks inside them).` or `Nothing in context — you see only the description; there are no ids to cite.`
  Tokens = chars / 4.
- `loop_.rs:5435` (`Turn::round`, round 0 of a chat turn, bound session): the block is persisted as a user-role tail
  row (`Turn::persist`) after the user's input and before the `<knowledge>` hints; a retried round 0 (the last row
  already starts `<notebook `) does not repeat it.

### `cite_check` (§6.6)
- `crates/document/src/cite_check.rs:22` `check_theuth_links(markdown) -> Vec<TheuthAnchor>` (C's frozen signature:
  every `](theuth://…)` href that parses, once, document order); `:35` `links` (raw + parse); `:53`
  `check(actor, &mut markdown, by) -> Option<ToolOutput>`: malformed hrefs refused with the grammar error; the rest
  resolved by one `CiteResolve { hrefs, by }`; `:107` `judge(links, cites, near)` is the pure core — `unavailable` →
  `theuth://notes/<id>#q: note unavailable (trashed or not readable)`; `missing` block →
  `theuth://sources/<id>#7f2a: block not found; blocks near the end are 7f1e, 7f2b` (the document's last three
  `<!-- id -->` ids via `Markdown{annotate}`), `@mm:ss` → `no segment at that time (past the end)`, `#turn` → `no
  such turn`, whole id → `not found`; `same`/`changed`/`moved` accepted; `?s=<sha8>` appended to every block citation
  without one (`sha8` = `codeindex::locate::text_sha(text.trim_end())[..8]`, every occurrence of that href). Refusal
  header: `nothing changed: a theuth:// citation has no live target — cite a block you were given or read back the
  document and retry`. Fails open when the resolver cannot answer (off the hub; C's stub in this tree).
- Call sites `crates/document/src/tools.rs:192` (`Docs::edit`, every `Insert`/`Replace` markdown, rewritten in place
  before the `Edit` message) and `:823` (`doc_create` body — the save path).

### Save / Promote (§8)
- `notebooks.rs:173` `save_answer_as_note`: `ReadSession` → turn `n` (1-based, `session_read::turns`, now
  `pub(crate)`) → last non-empty assistant text → title from a `SideCall { role: Title, kind: Title, 20 s }` ("≤ 12
  words", quotes/period stripped; the answer's first line cut to 12 words when the session is not live or the model
  fails) → registry `doc_create { title, markdown, notebook }` (files the note via C's `NotebookItemAdd`) → `SavedAnswer
  { doc, url }`. Markdown = `provenance()` (:251): the answer verbatim +
  `*Answer from [chat](theuth://sessions/<id>#<turn>) · <model> · <YYYY-MM-DD>*`; `<model>` is the session's model
  (`SessionSummary.model`; no per-turn model is stored).
- `:202` `promote_answer`: the same markdown under `## From chat · <date>` → registry `doc_insert { doc: note, at:
  "end" }`. Both run as the chat session (`ToolCall.session`), so the document write is `Principal::Agent { session }`
  and "Undo last agent edit" reverts it; both pass `cite_check` inside the tool.

### Tests
- `crates/agent/tests/notebooks_binding.rs` (store: root binding through a child, inheritance, newest-first roots-only
  list, unbind, unknown session); `crates/agent/tests/notebooks_block.rs` (plain prompt byte-identical / bound prompt
  ends with the contract; exact block snapshot with order + escaping + allow-list; budget truncation at a block with
  the exact notice and `mention`/`omitted`; description cap notice + empty-notebook sentence; provenance line and
  fallback title); `crates/document/tests/cite_check_links.rs` (extraction + dedupe + malformed listed; `sha8`
  convention; exact refusal lines for missing/unavailable/turn/time/whole-id; `changed` accepted, `?s=` added only
  where absent and to every occurrence).

## Proof
- `cargo test -p agent -p document --tests`: agent lib 522 passed (2 ignored, pre-existing) + `notebooks_binding` 1 +
  `notebooks_block` 5; document lib 47 + `tests/actor.rs` 11 + `cite_check_links` 4 — all green, 0 failed.
- The live `<notebook>` fetch and save/promote end-to-end need C's phase-2 handlers (`NotebookGet`, `CiteResolve`),
  which are `144: not implemented yet` in this tree: `block()` then renders the "could not be read" description,
  `cite_check` fails open. The pure halves (`render`, `judge`, `provenance`) are what the tests pin.

## Gates
- `cargo fmt` clean. `cargo check -p agent -p document --tests` clean.
- `cargo test -p agent -p document --tests` green (numbers above).
- `cargo clippy -p agent -p document --tests -- -D warnings`: see below.
- `cargo test -p document -p link -p rpc` (59 regression): see below.

## Deviations
- Document reads/writes from `agent::notebooks` go through the tool registry (`Invoke`, `quiet: true`) because `agent`
  has no `document` edge (C's note in the stub). Consequence: `doc_create`/`doc_insert` are `Mutating`; with
  `tools.policy.mutating = ask` and approvals on, Save/Promote raise an approval card in the chat session (the user
  approves once; `Deny`/timeout surfaces as the RPC error). A gate-free path needs a body in `rpc` (which has both
  crates) — the orchestrator's call; not done here to keep C's `api.rs` untouched.
- `SideCall` is answered only by a live session actor; when the chat is not loaded the title falls back to the
  answer's first 12 words instead of failing the save.
- Note titles inside the block come from the note's first heading (else its first line, `seed_title`) because no
  registry tool returns a document's title by id (`doc_read` has no title; `NotebookDetail.items` carry none).
- The `notebooks` prompt block is not a `RegisterPrompt` row (those are global and filtered per toolset): it is
  appended to the plugin blocks in `Turn::rebase` only when the session is bound, so plain sessions are byte-identical.
- `SessionMetadataChanged` is not published on a binding change (its shape carries `project` only); the UI re-reads
  `notebook_chats` after `session_set_notebook`.
- `CreateSession { notebook }` applies to durable roots only (a notebook chat is the owner's durable thread).

## Hand-offs
- Closer / C: regenerate ts bindings (`SessionSummary.notebook`, `CreateSessionOptions.notebook`, `SavedAnswer`).
- Test-only `SessionSummary { … }` literals outside my crates need one line `notebook: None,` after `project`:
  `crates/slack/src/ui.rs:797`, `crates/telegram/src/commands.rs:1459`, `crates/tui/src/attach.rs:273`,
  `crates/tui/src/model.rs:17562` (`cargo test -p slack -p telegram -p tui --tests` otherwise fails to compile).
- C (`attach.rs:33` PROMPT row for `notebook`): the design's row reads *notebook → its items are already in the
  `<notebook>` block at the levels the user chose; `doc_read` any item for more*. Suggested replacement for the
  "first look" cell: ``its items are already in the `<notebook>` block at the levels the user chose; `notebook_get(<id>)`
  for the item list`` — the "then" cell unchanged.
- C (`NotebookDetail`): an `items[].title` (and `words`) would let the block skip one `doc_read`/`source_get` per
  item and name notes by their real title.
- 145 wave (`loop_.rs`, `supervisor.rs`, `lib.rs`): my hunks are `loop_.rs:4114-4127` (rebase: `prompt_blocks` wrap),
  `loop_.rs:5435-5450` (round 0: persist the block), `supervisor.rs:9146` (`pub(crate) fn store_for`),
  `supervisor.rs:9504-9510` (bind after `create_durable`), `lib.rs:2070-2072` (`SessionSummary.notebook`).
