# 157-CTX as built: the notebook chat injects its notebook once, then sends change notices; the 40k budget is gone

Worker `w157-ctx` · branch `feat/157-nbctx` (cut from `origin/main` 2de5c411) · brief `~/.theuth/briefs/2026-10-10/157/157-ctx.md`.
Rulings: knowledge `decisions/theuth-notebook-chat-context-rulings-2026-10-10` (Sky, 2026-10-10). Design amended: `docs/design/144-notebooks.md` §5.1, §5.2, §5.3, §9, §13.3, §14, writer's call W-h.

## What changed, by decision

**D1 — no notebook budget.**
- `crates/agent/src/notebooks.rs:52-68` `Budget { description_tokens }` only; `context_budget_tokens` removed. `#[serde(default)]` with no `deny_unknown_fields`, so a config that still carries the key loads (`crates/agent/tests/notebooks_block.rs` `old_context_budget_key_still_loads`).
- `crates/agent/src/notebook_context.rs:353` `plan(items, pins, capacity)` — same ordering, truncation and demotion, the third argument is now the session's capacity (D4).
- `crates/document/src/notebook.rs:376-394` `ContextEstimate` loses `budget_tokens`, gains `injected: bool` and `pending_changes: u32` (D9).
- `crates/rpc/src/api.rs:1218-1227, 5786-5803` the estimate RPC: doc rewritten, calls `estimate(&agents, session, &notebook, &self.rt)`.

**D2 / D7 — once, then notices; a dial change is a line.**
- `crates/agent/src/loop_.rs:5565-5627` the injection site. Round 0 of a chat turn in a bound session: the retried-round-0 guard now covers both shapes (`<notebook ` and `<notebook_changes`); then `notebooks::turn_message` decides what, if anything, to append. Appends only; nothing earlier is edited, excluded or hidden.
- `crates/agent/src/notebooks.rs:464-510` `turn_message(store, session, notebook, budget, capacity, epoch, history)`: parses the baseline from the epoch's user rows, fetches pins + items, and returns `next_message(...)`. A notebook that cannot be read yields the "could not be read" block only when the epoch has no block yet, and never a notice.
- `crates/agent/src/notebook_context.rs:863-884` `next_message`: no block in the epoch → `render` (epoch 0) or `manifest` (epoch > 0); else `diff` → `notice` when non-empty, else `None`.
- `crates/agent/src/notebook_context.rs:1126-1133, 1178-1185` the `notebook_context_set` / `notebook_context_estimate` tool descriptions: the first message carries the levels; after that a change is a `<notebook_changes>` line on the next turn, never a re-injection.

**D3 — the baseline lives in the history.**
- `crates/agent/src/notebooks.rs:100-126` `Item::fingerprint()` = `sha8(body)` (sessions/artifacts: `sha8(mention line)`); `Item::href()`; `sha8()` = first 8 hex of sha256 (W-i's hash, re-implemented in `agent` because it does not depend on `document`).
- `crates/agent/src/notebooks.rs:648-699` `render`: every `<item …>` carries `s="<sha8>"`; `<description s="<sha8>">`; a demoted item carries `dial="<chosen level>"` beside its rendered `level="mention"`.
- `crates/agent/src/notebook_context.rs:567-696` `Seen`, `Baseline::of(texts)`, `Baseline::read`, `attrs()`: the last `<notebook ` block resets the baseline, each `<notebook_changes` after it applies, a notice before any block is stale. Item bodies and the description text are skipped (state machine on `</item>` / `</description>`), so markdown that happens to contain `<item ` is not parsed.
- `crates/agent/src/notebook_context.rs:698-860` `Change`, `read_call`, `diff`, `notice`: one line per item for added, removed, renamed, edited (fingerprint moved — content or insights), dial moved (to or from `off`), plus one for the description. Never the content.

**D4 — the first injection is limited by the session.**
- `crates/agent/src/notebook_context.rs:886-918` `UNKNOWN_WINDOW`, `default_limit(window, compact_at)`, `capacity(limit, fixed, frame)` = `fixed + (HEAD_SHARE + TAIL_SHARE) % of headroom(limit, fixed) − frame`, saturating. One function; the loop and the estimate both call it.
- Loop (`loop_.rs:5576-5590`): `fixed = fixed_tokens()`, `frame = est_frame()` before any call else `projected`, `limit = threshold(window)` (override + clamp as the trigger uses) else `default_limit(window, cfg.compact_at)`; an endless turn with a live service uses `endless.config().view_bytes / 4`.
- Estimate (`notebook_context.rs:1030-1092`): the thread's model (or `GetDefaultModel` for a new chat) → `ListModels` window → `default_limit(window, agent.compact_at)` → `capacity(limit, 0, frame_tokens)`; once the block is in the epoch the plan runs at `u32::MAX` (the items are in context at their dial levels).

**D5 — the manifest after a compaction.**
- `crates/agent/src/notebooks.rs:704-734` `manifest`: `<notebook … manifest="true">`, the capped description, every non-off item as `<item kind id title level="<dial>" s>mention line</item>`, the sentence, the allow-list.
- Endless memory (137): `endless_start` clears `state.history` every turn, so the loop reads the epoch's rows from `store.frame(id)` when `self.view.is_some()` (`loop_.rs:5591-5611`). The block is one stored row; the view folds it like any other; the second and later turns find it in the store and send nothing or a notice. Proven by `loop_::notebook_ctx_tests::endless_session_does_not_reinject_when_it_folds`.

**D6 — the static prompt.** `crates/agent/src/notebooks.rs:24-40` `PROMPT` rewritten (once, manifest after a compaction, `<notebook_changes>`, "Before you answer a question about this notebook, look up what you need (`nb_search` …, `doc_read` …) so your answer reflects everything in it, not only what is already in context.", citation rules, never invent an id, quoted material). `prompt_blocks()` untouched → byte-identical outside a notebook (test kept). `crates/agent/src/attach.rs:33-36` the `notebook` row rewritten.

**D8 — Ask.** `crates/agent/src/ask.rs:36-37, 46, 378` `ask_context_tokens` (default 40 000), split by branch count exactly as before.

**D9 — estimate and readout.**
- `crates/agent/src/notebook_context.rs:938-978` `Estimate { items, total_tokens, omitted, empty, injected, pending_changes }`; `omitted` = `status == omitted` = `off` + did-not-fit before the block, `off` only after it.
- `crates/agent/src/notebook_context.rs:980-1023` `NotebookFrame(SessionId)` → `SessionFrame { model, notebook_rows, tokens }` on `Agents` (the smallest hook: the estimate needs the thread's model and its epoch's `<notebook*` rows).
- `crates/desktop/ui/src/lib/notebook-context.ts:27-40` `pendingWords`, `contextLine`: `in context: 4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k tokens · 1 omitted`, plus `2 changes go with your next message` (`1 change goes …`) once `injected`.
- `crates/desktop/ui/src/components/notebooks/ChatColumn.tsx` `function ContextLine`: the budget bar is gone (and with it the now-unused `cn` import on line 18 — see deviations).
- Fixture lines fixed in `ChatColumn.test.ts`, `NotesColumn.test.ts`, `sources/SourcesColumn.test.ts`, `lib/notebook-context.test.ts`.

## Decisions I took (not in the brief)

1. **Fingerprint = sha8 of the whole annotated markdown, not of the text at the rendered level.** Level and content are then orthogonal: a dial move is `level` only, an edit is `s` only; a `mention` item whose body changed is still reported (the agent may have read it). Sessions and artifacts (no body) hash their mention line, so only their title moves are seen.
2. **The baseline records the dial level, not the demoted one.** A won't-fit item renders `level="mention" dial="full" status="omitted"`; the parser takes `dial` over `level`. Without this the first turn after a demotion would report a spurious "now full".
3. **Notice lines are `<change kind id title level s>text</change>` elements**, parsed by the same attribute reader as `<item>`. `level="off"` on a change means "not in context any more" and removes the id from the baseline; "now off" (still in the notebook) and "removed from the notebook" differ in the text only. The description is `kind="notebook"`.
4. **An item not in the baseline that is now on reads "in context now at <level>"**, whether it was added to the notebook or switched back on from `off` — `off` items are not listed in the block, so the two are indistinguishable from the history, and the wording is true for both.
5. **Capacity = where a fold would leave the frame.** `fixed + 60 % of the headroom` (COMPACT-FLOOR's `HEAD_SHARE + TAIL_SHARE`), so the 40 % a fold keeps free for growth is the reply headroom the block respects and the trigger cannot fire on the turn right after the block. Unknown window (no catalog row, no override): `UNKNOWN_WINDOW = 128_000` behind `default_limit` — the loop cannot compact there either, and the block still has to stop somewhere (`ponytail:` comment names the ceiling).
6. **The estimate's capacity ignores the system prompt and tools (`fixed = 0`)** — they are not known outside a turn — and ignores a per-session `CompactionOverride`. It can therefore keep an item the loop demotes; the loop is authoritative, the UI line is an estimate. Recorded in the `estimate` doc comment.
7. **After the block is in the epoch the estimate plans at unlimited capacity.** Re-planning against the shrunken frame (the block is now in the history) would mark items "omitted" that the model already has; post-injection `omitted` counts `off` only.
8. **Endless sessions read the baseline from `store.frame`** (all rows, epoch 0 — no compaction under endless memory) instead of the cleared `state.history`. `ponytail:` an endless chat deserialises its whole row set once per turn; a `LIKE '<notebook%'` store query if that ever shows.
9. **The manifest's sentence is used for every epoch > 0**, including a chat bound to its notebook only after a compaction (the contents were never in that chat). Rare; the sentence still points at the right tools.
10. **A `context_edit` that removes the block from epoch 0** leaves the epoch without a block → the next turn re-injects the full block (the model lost it and the baseline with it). Appends only, so this is consistent with D2.
11. **Session titles / artifacts** are still mention-only; their change lines carry `session_read` / `artifact_read` as the read call.

## Deviations from the brief

- `crates/document/src/notebook.rs`: the brief lists "the comment", but D1/D9 require the `ContextEstimate` struct itself to lose `budget_tokens` and gain `injected` / `pending_changes`; the struct is edited (lines 376-394), nothing else in that file.
- `ChatColumn.tsx` line 18: the `cn` import became unused once the bar left `ContextLine`; `tsconfig` has `noUnusedLocals`, so the one import line is removed. Nothing else outside `function ContextLine` is touched.
- `ChatColumn.test.ts`: beyond the fixture lines, the two tests asserting the budget bar (`aria-valuenow`, `width:31%`, `bg-amber-500`, `bg-danger`) had to change — they test `ContextLine`, which I own; they now assert no `progressbar`, no `40k`, and the pending part.
- Bindings: `crates/desktop/ui/src/rpc/bindings/` is gitignored (`crates/desktop/.gitignore:5`), so there are no binding files to commit; the export + copy was run for the local tsc/vitest only (gate 4).
- A new source module was not added; the baseline/diff code lives in `notebook_context.rs` (the planner's home) and the loop tests in `crates/agent/src/loop_/notebook_ctx_tests.rs` (the harness is `pub(super)` inside `loop_`, so a `tests/` file cannot reach it).

## Tests

Unit (`crates/agent/src/notebook_context.rs` `mod tests`, `crates/agent/tests/notebooks_block.rs`): `plan()` with a capacity (`capacity_truncates_the_first_overflow_at_a_block_and_mentions_the_rest`, `budget_cuts_newest_first_then_demotes`), capacity arithmetic (`capacity_is_the_fold_point_minus_the_frame`), baseline parse (`baseline_parses_block_and_notices_in_order`), every change kind without content (`diff_reports_every_change_kind_without_content`), manifest rendering (`dial_levels_render_in_order_off_is_not_citable_and_the_estimate_matches`), the old key (`old_context_budget_key_still_loads`), the prompt contract (`plain_session_prompt_is_byte_identical_and_bound_one_gains_the_contract`).

Loop-level (`crates/agent/src/loop_/notebook_ctx_tests.rs`, real `Turn` at the wiremock provider, the notebook served by fake `notebook_get` / `source_get` / `doc_read` registry tools):
- `block_once_then_notices_then_manifest_after_compaction`: turn 1 injects the block (content included); turn 2 unchanged adds no `<notebook` row; a source rename + edit and a note edit before turn 3 add exactly one `<notebook_changes>` with both ids, the read calls, and none of the content; turn 4 unchanged adds nothing; `off` then `summary` dial changes each add one line; a `/clear`-style compaction (whole epoch folded) → the next turn gets the manifest only; the epoch after: nothing, then one description line. Every direct turn builds a fresh `Turn` from `store.frame` — a restart per turn — and none produced a spurious notice.
- `endless_session_does_not_reinject_when_it_folds`: the actor path with `test_view` set (endless frame, `state.history` cleared each turn): one block, two more turns add nothing, a note edit is one notice, a further turn adds nothing.

## The three example texts (printed by `block_once_then_notices_then_manifest_after_compaction`)

First block (turn 1):

```
<notebook id="NB157" title="DR verification">
<description s="a8db3d31">Verify the DR plan.</description>
<item kind="note" id="N1" title="Drill timeline" level="full" status="available" s="e0e7d934">
<!-- a -->
# Drill timeline

<!-- b -->
T+0 failover</item>
<item kind="source" id="S1" title="Runbook v7" origin="file" level="full" status="available" s="9a2204c6">
<!-- r1 -->
## Steps

<!-- r2 -->
Step one. RUNBOOK-STEP-FOUR-SECRET</item>
You may cite only these ids: N1, S1 (and the blocks inside them).
</notebook>
```

Change notice (turn 3, after the source was renamed and refreshed and the note edited):

```
<notebook_changes notebook="NB157">
<change kind="source" id="S1" title="Runbook v8" level="full" s="eebe3c53">renamed from "Runbook v7"; content changed (text or insights); read it with doc_read("source:S1") when it matters (cite theuth://sources/S1)</change>
<change kind="note" id="N1" title="Drill timeline" level="full" s="f0eee1c7">edited; read it with doc_read("N1") when it matters (cite theuth://notes/N1)</change>
</notebook_changes>
```

Post-compaction manifest (first turn of epoch 1; S1 had been dialled to `summary` meanwhile):

```
<notebook id="NB157" title="DR verification" manifest="true">
<description s="a8db3d31">Verify the DR plan.</description>
<item kind="note" id="N1" title="Drill timeline" level="full" s="f0eee1c7">mention only: 9 words; read with doc_read("N1") if needed</item>
<item kind="source" id="S1" title="Runbook v8" level="summary" s="eebe3c53">mention only: 7 words; read with doc_read("source:S1") if needed</item>
The contents of these items were in context earlier in this chat; read what you need with doc_read / nb_search.
You may cite only these ids: N1, S1 (and the blocks inside them).
</notebook>
```

## Gates (all through `build-lock`, label `157-ctx`)

1. `mbx test -p agent -p rpc -p document --no-run`, then bare `mbx test -p agent -p rpc -p document`: **780 passed, 0 failed, 2 ignored** (agent: lib 611 + 2 ignored, `notebooks_binding` 2, `notebooks_block` 7; document: lib 75, `actor` 11, `cite_check_links` 4, `index_search` 3, `ingest_audio` 1, `ingest_job` 5, `insights` 2, `replicate` 1; rpc: lib 56, `alerts` 1, `prompts` 1). No failure to compare against the base.
2. `mbx clippy -p agent -p rpc -p document --tests`: exit 0, zero warnings.
3. `cargo fmt -p agent -p rpc -p document`: applied (formatting-only diffs in the four Rust files).
4. `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings`: 8 passed; `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`; `ContextEstimate.ts` now `{ items, total_tokens, omitted, empty, injected, pending_changes }`. Both directories are gitignored — nothing to commit, no other drift to report.
5. UI: `tsc -b --noEmit` → 0 errors (the baseline `/tmp/157-ctx-tsc-base.txt` had 1507, all `Cannot find module '@/rpc/bindings/…'` from the missing generated bindings; none new). `vitest run` on the four files: 4 files, 10 tests passed.
