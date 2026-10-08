# 136 — As-built W1 (KNOWLEDGE)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-w1`, branch `feat/136-w1`, cut from C's `f99f6d54` · fills the `crates/knowledge` behaviour of contract §3.1 against the seams of `136-as-built-C.md`.

## 0. One screen

- **Rank** (§5.3) lives in `Knowledge::ranked`, which both `search_for` and `hint_for` use: `score = bm25 × scope(1.5 | 1.0 | 0.5) × rank_weight`, with `pub fn rank_weight(type, age_days, shown, opened, &Lifetimes)` = `type_weight × freshness × use` (pure, unit-tested). `shown`/`opened` come from the durable `uses` table in one `IN (…)` query per search. `hint_for` additionally drops `hint: false`, non-live statuses and a zero rank (a record past `knowledge.lifetimes.record`); `search` keeps everything and the `knowledge_search` text line marks any non-live status `[archived]` / `[merged]` / `[stale]` / `[retired]` after the type.
- **Lifetimes** are read from `knowledge.lifetimes` (`KnowledgeConfig.lifetimes: Lifetimes {record 14, fact 30, procedure 90, entity 60}`, serde defaults) at `Knowledge::open` via `rt.section("knowledge")`. Knowledge's section wins over `dreaming.lifetimes` for the rank; dreaming's mirror drives only its own archive/verify sampling.
- **Five ops** are real store methods with one shared tail (`consolidate`): one gix commit `dreaming <op> <id>` authored `dreaming <dreaming@localhost>` (`repo::DREAMING_SIGNATURE`), `source: dreaming` + fresh `timestamp` stamped on every rewritten file, one `KnowledgeChanged{op, source: Dreaming}` per rewritten id, `Receipt{op, ids, commit}`.
- **`uses`**: C's `record_shown`/`record_opened`/`stats` were already real; unchanged. `knowledge_get` records an open (C's call site, now covered by a test).
- **Design 10 amendment** ("the index is deletable"): *`knowledge.sqlite` is derived except for the `uses` table — `concepts` and `links` rebuild from the files on every `sync`/`open`; `uses` (impressions and opens per id) is durable, is never touched by `sync`, survives a schema-mismatch rebuild of `concepts`, and is lost if the file is deleted.*

## 1. File by file

| File | What |
|---|---|
| `crates/knowledge/src/lib.rs` | `KnowledgeConfig.lifetimes: Lifetimes`; new `Lifetimes {record, fact, procedure, entity}` (u32 days, `#[serde(default)]`, `Default` = 14/30/90/60) |
| `crates/knowledge/src/repo.rs` | `DREAMING_SIGNATURE`; `Repo::commit_as(written, removed, message, author)`; `commit` delegates with `SIGNATURE`; private `commit(repo, tree, parent, message, author)`; test probe `Repo::author(commit) -> (name, email)` (`#[cfg(test)]`) |
| `crates/knowledge/src/store.rs` | `Knowledge.lifetimes` (from the config section in `open`); `scope_for` is `pub(crate)` (the merge tool resolves the session's scope); `write_concept` → wrapper over new `write_concepts(files, log_entry, message, author)` (N files, one `finish_files`, ONE commit as `author`); `commit` → `commit_as`; `ranked` applies the rank (`uses_of` fetches counters for the candidate rows); `hint_for` drops `score <= 0`; `age_days`, `pub fn rank_weight`; `merge` / `supersede` / `archive` / `verify` / `retitle` real; helpers `load`, `consolidate`, `union`, `set_list`, `append_paragraph`, `append_section`; tests `live_shapes_parse_unchanged`, `rank_weight_monotonicity`, `hint_rank_uses_opens_impressions_and_record_lifetime`, `merge_folds_into_stubs_and_is_restorable`, `supersede_archive_verify_retitle_effects_author_and_restore` (+ helper `commit_of`) |
| `crates/knowledge/src/tools.rs` | `knowledge_search` text marks non-live statuses; `knowledge_merge` passes the session's `AgentScope` (`k.scope_for(Some(cx.session))`, the user's when none) and prints the receipt's ids; gate test updated (`no such concept` instead of `not built: W1`); test `search_marks_archived_and_get_records_an_open` |
| `docs/design/136-as-built-W1.md` | this file |

Untouched: `types.rs`, `harvest.rs` (the `<knowledge>` note text is unchanged, as contracted), `search.rs`, `sync.rs`, `Cargo.toml`.

## 2. The rank as implemented

```text
score(hit) = (−bm25) × scope × rank_weight(type, age_days, shown, opened, lifetimes)

scope        = 1.5 if cwd starts with the concept's `scope:` dir · 0.5 if it has a `scope:` elsewhere · 1.0 otherwise  (unchanged from 10)
type_weight  = decision 1.3 · warning 1.3 · procedure 1.1 · fact 1.0 · entity | hosts | project 0.9
               · record 0.6, and 0 (whole score 0) when age_days > lifetimes.record · any other type (architecture, tool, …) 1.0
freshness    = 0.5 + 0.5·exp(−age_days / 45) for record and fact · 1.0 otherwise
use          = 1 + ln(1 + opened) − 0.4·[shown ≥ 50 ∧ opened = 0 ∧ type ≠ warning]
age_days     = (now − `timestamp:`) in days, 0 when absent/unparsable (the file's write time; the harness stamps it on every put)
```

`hint_for` filters before the 5-line budget: `exclude`, `hint: false`, `status ∉ {fresh, tidy}`, `score ≤ 0`; then the existing ≥ 2-term rule. The agent-scope ordering of 67 §9 (own items first) still sorts above the score. `search_for` returns every match (stubs for unreadable private items as before) in score order.

## 3. The five ops — exact file effects

All: the file must exist (`no such concept: <id>` otherwise, nothing written); frontmatter keys the op does not name are kept verbatim; `source: dreaming`, `timestamp: <now>`; one commit `dreaming <op> <subject>` as `dreaming <dreaming@localhost>`; `log.md` line = the commit subject; `KnowledgeChanged{id, op, source: dreaming}` per rewritten id; `Receipt{op, ids, commit}`.

| Op | Frontmatter | Body | `ids` |
|---|---|---|---|
| `merge(ids, into, body, evidence, scope)` | canonical: `tags` = canonical's ∪ each folded's (order kept), `evidence` = canonical's ∪ folded ∪ given, `status: tidy`, `merged_into` removed; a *new* canonical takes `type`/`title`/`description` from the first folded file, `owner` = `scope.owner`, `visibility` = `shared` for the user, `private` for an agent. Each folded id: `status: merged`, `merged_into: <into>`, `hint: false`, everything else kept | canonical = `body` verbatim; stub = `Merged into [<into>](/<into>.md)` | `[into, folded…]` |
| | `links` rows with `dst = <folded>` are retargeted to `<into>` (`UPDATE OR IGNORE` + delete leftovers). `into ∈ ids` is simply not folded; no folded id left → `merge: nothing to fold`. | | |
| `supersede(loser, winner, valid_until?, note?)` | `superseded_by: <winner>`, `valid_until` when given, `hint: false`; status untouched. Winner must exist; `loser == winner` refused | + paragraph `Superseded by [<winner>](/<winner>.md)` (`: <note>` appended when given) | `[loser]` |
| `archive(id, reason)` | `status: archived`, `hint: false` | + `## Archived` section (heading added once; a later reason joins it) with `reason` | `[id]` |
| `verify(id, ok, note?)` | `verified: <today UTC YYYY-MM-DD>` | unchanged (the note is not written for `ok`) | `[id]` |
| `verify(id, stale, note?)` | `status: stale` (`verified` kept) | + `## Stale` section with `note` (default "a read-tool check no longer confirms this") | `[id]` |
| `retitle(id, title, summary)` | `title: <title>` (trimmed, non-empty) | `## Summary\n\n<summary>` prepended (`summary` empty → the old title) then the old body; body cap enforced | `[id]` |

`knowledge_restore(id, commit)` undoes each: the test restores every single-file op from the pre-op commit and a merge stub from its previous commit. Note for W3's undo: `restore(id, None)` restores from the parent of the *latest* commit touching the file — after one restore that parent is the op commit again, so undo must pass the pre-night commit explicitly (the `(id, commit_before)` receipt the contract already asks for).

## 4. Deviations from the contract (and why)

1. **Author is always `dreaming`.** Contract item 4: author `dreaming <dreaming@localhost>` "when `Source::Dreaming`, else `theuth`". The store signatures (contract §2.1) and actor messages carry no `Source`, and `ToolCx` cannot tell the dreaming routine from an owner root — so the five ops (which are dreaming's by definition; the tools say "Dreaming only") always stamp `source: dreaming` and commit as `dreaming`. Signatures unchanged. If attribution of hand-run ops matters, add `source: Source` to the five actor messages and tools (closer's call; one-line change per op in `consolidate`).
2. **`knowledge_merge` scope** (C's deviation 4): the tool now passes the session's `AgentScope` from `k.scope_for(Some(cx.session))`, the user's scope when none. No signature change — `scope_for` became `pub(crate)`.
3. **`KnowledgeChanged` per rewritten id**, not one per op: a merge publishes one event for the canonical and one per stub (all `op: merge`), so list/UI subscribers refresh every row that changed. Single-file ops publish exactly one.
4. **Zero rank filters hints.** The contract says a record past `lifetimes.record` scores 0; `hint_for` also *drops* it (design §5.2: records are "hinted while ≤ 14 d old"). `search` still returns it with `score 0`.
5. **`search` marker**: non-live statuses other than `archived` are marked too (`[merged]`, `[stale]`, `[retired]`) — same one-liner, same place (the `knowledge_search` text output; `Hit.meta.status` carries it structurally).
6. **`supersede` writes a body paragraph** linking the winner (C's doc said "`note` goes under the loser's body"; the contract listed only the three keys). Side effect: a `links` row loser → winner, so the winner's `Cited by` names the loser.

## 5. Hand-offs

- **Z / W3**: `dreaming::undo` must restore from `commit_before` explicitly (see §3 note); `restore(id, None)` after one restore re-applies the op.
- **Z**: `KnowledgeConfig` gained `lifetimes`; `DEFAULT_CONFIG` in `crates/theuth/src/main.rs` may want a `knowledge.lifetimes:` block for discoverability (defaults apply without it). Dreaming's `DreamConfig.lifetimes` stays the mirror; knowledge's wins for the rank.
- **Z**: `links` retargeting on merge is index-only; the citing file's next `sync` (its mtime/size changing) rebuilds its rows from its body, pointing at the stub again — which resolves through `merged_into`. If dreaming wants the citing bodies rewritten, that is a Tidy step (`knowledge_put` of the citer), not a store op.
- **Z (optional)**: deviation 1 — add `source` to the five ops if hand-run consolidation should commit as `theuth`.

## 6. Verified / not verified

Verified (through build-lock, target dir `~/Develop/theuth-136-target/W1`): `cargo test -p knowledge --no-fail-fast` — 42 passed, 0 failed (C's 36 + the 6 new: `live_shapes_parse_unchanged`, `rank_weight_monotonicity`, `hint_rank_uses_opens_impressions_and_record_lifetime`, `merge_folds_into_stubs_and_is_restorable`, `supersede_archive_verify_retitle_effects_author_and_restore`, `search_marks_archived_and_get_records_an_open`); `cargo clippy -p knowledge --tests -- -D warnings` clean; `cargo fmt -p knowledge --check` clean (after `cargo fmt -p knowledge`, which touched only `store.rs`/`tools.rs`). The op tests assert the gix author `dreaming <dreaming@localhost>` and subject `dreaming <op> <id>` on each receipt's commit, one commit per op, and `restore` returning the pre-op bytes.

Not verified: the engine running with the rank (no `t` against `~/.theuth`); the rank over the 1,598 live files (unit fixtures only); `KnowledgeChanged` delivery to subscribers (published through `rt.publish` as every other write; not observed in a test); a merge across agent-owned private concepts.
