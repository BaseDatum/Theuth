# 136 — As-built F4 (ENGINE follow-ups from live proof P)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-f1`, branch `feat/136-f4`, cut from F1's `8f2d6f39` · owns `crates/agent/**`, `crates/knowledge/**`. Fixes P §6 gaps 1, 2, 4, 6. Commits: `16b48322` (the four fixes + tests), plus the as-built commit named at the end.

## 0. One screen

- **No `<knowledge>` note in the dreaming session** (gap 1): the hint path in `Turn` is skipped when `knowledge.is_consolidating_session(self.shared.id)` — no note, no `record_shown`, no `hinted` ids. The night registers its session at fire time (F2's wiring of F1's seam).
- **`knowledge_verify` needs evidence** (gap 2): new `evidence: Vec<String>` argument; the tool refuses (`Err`, exact text `verify needs evidence from a read: pass evidence with a path that exists, an http(s) URL, or a session:/meeting: id`) unless at least one item is an existing path (absolute, or relative to the session's data dir `cx.resources.root`), an `http://`/`https://` URL, `session:<uuid>` (parses as `SessionId`) or `meeting:<non-empty>`. Every item is unioned into the file's `evidence` (same `fm_strings`/`union`/`set_list` as merge and harvest).
- **Merge rewrites the citing files** (gap 4): `merge` no longer retargets `links` index-only. Every concept citing a folded id (other than the canonical and the folded ids) has its body's `](<target>)` rewritten to `](/<into>.md)`, is `touch`ed (`consolidated_at`) and goes into the same `write_concepts` batch — so it is in the ONE merge commit, in `Receipt.ids` (after the canonical and the stubs), and gets its own `KnowledgeChanged { id, op: Merge, source: Dreaming }` exactly like the primary ids. The `links` rows come from re-indexing the rewritten file; `restore(citer, before)` brings the old link back in file and index.
- **`Knowledge::restore_many`** (gap 6): `pub async fn restore_many(&self, pairs: &[(String, String)]) -> anyhow::Result<Change>` — every `(id, commit)` read from the repo first (any unreadable pair refuses the batch before a write), then one `write_concepts` call: files written, indexes regenerated once, ONE commit `restore <n> concepts` authored `theuth`, one `KnowledgeChanged { op: Put, source: User }` per id. `restore(id, commit)` is unchanged.

## 1. Gaps → file:line → test

| Gap | Where | Test |
|---|---|---|
| 1 hints in the night | `crates/agent/src/loop_.rs:5068-5072` (`&& !knowledge.is_consolidating_session(self.shared.id)` in the hint `if`) | `loop_::meeting_diet_tests::hint_limit_and_title_pin` extended: `run(None, true)` registers the turn's session → `None` (no note) and `uses.shown` unchanged (`meeting_diet_tests.rs:113-128`). Fails without the fix (`Some(3)`, verified by reverting the condition). |
| 2 verify without a read | `crates/knowledge/src/tools.rs:649-672` (`VerifyArgs.evidence`, `is_evidence`), `:689-699` (`ensure!` + pass-through); `store.rs:1983-2015` `verify(.., evidence)` unions; `lib.rs:288-291` `Verify.evidence` (`#[serde(default)]`) | `tools::tests::verify_needs_evidence_from_a_read` (six refusals with the exact message, `verified` untouched; relative path that exists → ok, both items recorded; absolute path, https URL, `session:<uuid>`, `meeting:06` each pass; evidence union of 6). `store::tests::supersede_archive_verify_retitle_effects_author_and_restore` re-pinned to the union. |
| 4 receipt completeness | `crates/knowledge/src/store.rs:1885-1917` (citers from `links`, body rewrite, `touch`, pushed into `files`) | `store::tests::merge_rewrites_citers_and_publishes_them` (A→B with C citing A twice: `Receipt.ids == [b, a, c]`; three `KnowledgeChanged{Merge, Dreaming}` events incl. C via a subscribed recorder actor; C's text and `links` retargeted; `restore(C, before)` restores text and `links`, A's `cited_by == [C]` again). `merge_folds_into_stubs_and_is_restorable` re-pinned: `r.ids` now ends with `facts/citer`. |
| 6 restore speed | `crates/knowledge/src/store.rs:1310-1359` `restore_many` | `store::tests::restore_many_restores_several_ids_in_one_commit` (three ids at v1 commits, rewritten to v2; `restore_many` → `history.len() == before + 1`, `history[0].commit == change.commit`, bodies equal the v1 texts, three `KnowledgeChanged{Put, User}`; an unreadable pair refuses the batch, no commit). |

## 2. Hand-offs

- **Dreaming undo → `restore_many`** (`crates/dreaming/src/undo.rs:61` today calls `k.restore(&t.id, Some(&t.commit_before))` per id): group the receipt's `(id, commit_before)` pairs and call `k.restore_many(&pairs)` once (or per op group). Pairs whose `commit_before` is `""` (the commit created the id — P's `53bbdc55`) must be excluded first: `restore_many` requires a real commit per pair and refuses the whole batch on any unreadable one. One `KnowledgeChanged` per id is still published, so the night's `Undone` accounting works unchanged.
- **Receipt for citers**: P's branch gives `KnowledgeChanged` a `commit` field (`53bbdc55`, not in this tree); `consolidate` publishes citers through the same loop as the primary ids, so they inherit it on merge. The night receipt dedups by id — a citer is restored to the commit before the merge, which is the pre-retarget file.
- **Skill/routine text** (F2, `crates/dreaming/src/skill.md`): `knowledge_verify` now takes `evidence: [..]`; the tool description names the rule. A night that passes no evidence gets the refusal string back as the tool error.

## 3. Deviations and judgement calls

- **Supersede retargets nothing**, before and after: `supersede` writes only the loser (`superseded_by`, `valid_until`, `hint: false`, paragraph) per 136 §3.1 item 4; no `links` rewrite existed to make receipt-complete. Left as is.
- **`source: user` citers are rewritten too**: D5 as documented in `store.rs` (`ensure_not_user`: "dreaming may retitle or link it") allows a link retarget; the alternative (skip them) would leave their links on the stub and the old index-only retarget — the exact gap. Their identity keys are untouched; `consolidated_at` is set.
- **Link rewrite is a text replace** of `](<raw target>)` → `](/<into>.md)` for each link `doc.links()` resolves to a folded id (okf exposes no offsets). Anchors on the old target are dropped (the section belonged to the folded file). A code span quoting the identical `](target)` would be rewritten too (`ponytail:` comment at `store.rs:1906`); a `[text](target "title")` form is not matched (its index row is still fixed by the re-index of any other change to that file — if a file has only titled links, the old index-only behaviour is lost for it; none exist in the live bundle's shapes F1 catalogued).
- **Evidence items are whitespace-free tokens**: the index column is space-joined (`evidence.join(" ")`) and `Meta.evidence` splits on it — pre-existing; the tool doc says paths/URLs/ids. Free text passed as evidence is not a read source anyway and does not pass the gate on its own.
- **Data dir for relative paths** = `cx.resources.root` (durable sessions: the data dir; private sessions never reach the tool — `writable` refuses first).
- **`restore_many` publishes `source: User`** like `restore` (the ordinary write path, authored `theuth`), not `Dreaming`.
- `parking_lot` added as a knowledge **dev**-dependency for the test recorder (repo rule: no `lock().unwrap()`).

## 4. Gates (all through `build-lock`, `CARGO_TARGET_DIR=…/W2`)

| Gate | Result |
|---|---|
| `cargo test -p knowledge` | 55 passed |
| `cargo test -p agent --lib` | 472 passed, 2 ignored |
| `cargo clippy -p knowledge --tests -- -D warnings` | clean |
| `cargo clippy -p agent --tests -- -D warnings` | clean |
| `cargo fmt -p knowledge -p agent -- --check` | clean |
| gap-1 test without the fix | `hint_limit_and_title_pin` fails at `meeting_diet_tests.rs:127` (`Some(3) != None`) |
