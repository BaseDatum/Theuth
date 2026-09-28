# 60 — As built, CI-SEARCH: infix symbol search and stable monikers

*Branch `feature/codeindex-search`, base `22b39455`, 2026-09-28. Amends `60-codeindex.md` §4 (schema) and §8 (tools). Brief: `/tmp/enh-2026-09-28/omp-ci-search.md`.*

## Commits

### 1 — `28042a07` `feat(codeindex): infix code_find via FTS5 trigram and stable symbol monikers`
Both items in one commit: they share the single schema bump (§4: one DDL change, one `SCHEMA_VERSION`), and splitting `store.rs` hunk by hunk would have left commit 1 with a DDL that commit 2 rewrote.

### 2 — `5e8ea326` `test(codeindex): find tiers, monikers on every tool, line-move stability`

### 3 — `docs(60): as-built CI-SEARCH` (this document)

## What changed

### Item 1 — infix / substring symbol search
- `store.rs:65`: `symbols_tri` — `fts5(name, tokenize = 'trigram')`, rowid = `symbols.id`, **names only** (file contents stay grep's job). Written in `apply` beside `symbols_fts` (`store.rs:368-372`), deleted with the file's rows (`store.rs:280-285`).
- `store.rs:441-479` `find`: `exact: true` → `symbols.name = ?` only (unchanged semantics, rows say `match: "exact"`). Otherwise one query, `TIERED` (`store.rs:457`): candidates = exact name ∪ trigram `LIKE` ∪ the existing porter FTS over name/signature/docs; a `CASE` labels each row `exact` | `prefix` (`name LIKE 'tok%tok2%'`, case-insensitive) | `infix` (in the trigram hits) | `text`; `ORDER BY` tier, then `rank DESC`, path, line. The trigram hits are a `WITH … AS MATERIALIZED` CTE so they are computed once for the filter and the label.
- `store.rs:481` `like_pattern`: whitespace tokens → `%tok%tok2%`, `%` stripped from user text. **Space-split queries come for free**: `tool registry` → `%tool%registry%` → `ToolRegistry`. `_` is left as SQL's one-char wildcard (an identifier query like `open_db` over-matches `openXdb` at worst; escaping would disable FTS5's LIKE optimisation).
- `FindRow` (`store.rs:414`) gains `moniker` and `match` (field `matched`, renamed for sqlx and serde). `tools.rs:140,169`: `code_find` argument doc and description updated.

**Trigram choice — measured on this repository's index (19 954 symbols, warm, Python sqlite3 3.53, one process, 50 runs):**

| query | `symbols.name LIKE` scan | manual postings table (intersect + verify) | FTS5 trigram |
|---|---|---|---|
| `Registry` | 1.05 ms | 0.31 ms | **0.10 ms** |
| `render` | 1.10 ms | 0.45 ms | **0.11 ms** |
| `tool` | 1.12 ms | 0.25 ms | **0.08 ms** |
| on-disk | — | 4.6 MB (175 700 rows + index) | **1.4 MB** |
| build | — | 146 ms | 123 ms |

FTS5 trigram wins on every axis; picked. The bundled SQLite (libsqlite3-sys 0.36) has the tokenizer (needs ≥ 3.34; `MATERIALIZED` needs ≥ 3.35).

### Item 2 — stable monikers
- `parse.rs:364` `monikers(language, path, &[Symbol]) -> Vec<String>`: `<language>:<path>#<Qualified.Name>(<kind>)`; the qualified name is the parent chain joined by `.` (`Registry.new`, `inner.helper`), recursive because a Rust `impl` method's parent (its type, attached by `attach_impl_methods`) may come later in the file; depth-capped at 32 (`ponytail:` comment). Same base moniker within a file → `#2`, `#3`, … by source order.
- `<language>` is `files.language` (`rust`, `typescript`, `javascript`, `python`, `go`), not the brief's `ts:` example — one vocabulary with every other row in the index. Decision recorded here.
- `store.rs:53,57`: `symbols.moniker TEXT NOT NULL` + `symbols_moniker` index (plain, not `UNIQUE`: uniqueness is guaranteed by construction, and a constraint failure would abort a whole reconcile transaction for a cache). Written in `apply` (`store.rs:332,352`).
- Returned everywhere: `FindRow`, `OutlineRow` → outline `Node.moniker` (`tools.rs:243,260`), `Definition` via `def_cols!` (`store.rs:832-840`) → `code_refs.definition`, `code_symbol.definition`, `rank::Tag.moniker` (`rank.rs:114`, from `SymNode.moniker` `rank.rs:63` and `store::ranked_tags` `store.rs:1034`) → `code_map.rows`; `code_impact.changed[].monikers` (`tools.rs:1345`: the index rows of the hit definitions; hits mapped from a blob parse resolve to the index rows by `(name, kind)` as before).
- Accepted as `symbol`: `pick` (`tools.rs:402-418`) tries id → moniker (`contains('#') && contains(':')`) → `path:line` → name; `store::definition_by_moniker` (`store.rs:862`). The "has N definitions; pass a symbol_id, moniker or path:line" hint and both tools' argument docs/descriptions say so.

### Item 3 — schema
`SCHEMA_VERSION` `3` → `4` (`store.rs:21`). Convention confirmed (`store.rs:1-3`, `open` at `store.rs:96-128`): no migrations, a mismatch removes the file and rebuilds. Every existing index rebuilds on first call after this lands (~8–10 s for this repository, see Numbers).

## Numbers (this repository, 1 721 files / 1 070 parsed / 19 971 symbols; throwaway harness `tests/zz_bench_numbers.rs`, removed before commit; `store::find(.., exact=false, limit=1000)` through sqlx, warm = mean of 20 calls)

| | before (`22b39455`) | after |
|---|---|---|
| `code_find("Registry")` | 27 rows, 0.34 ms — `ToolRegistry` absent (porter token `toolregistri`) | 30 rows, 0.77 ms — `Registry` (exact) first, `ToolRegistry` as `infix` |
| `code_find("render")` | 163 rows, 1.37 ms | 168 rows, 2.59 ms (exact `render`s first, then `render_all`-style prefixes, then infix, then text) |
| `code_find("tool registry")` | 8 rows, 0.28 ms — top hits `suspend`, `rebase` (doc-word matches), no `ToolRegistry` | 9 rows, 0.73 ms — `crates/tools/src/registry.rs:41 ToolRegistry` first |
| index size on disk (after `wal_checkpoint(TRUNCATE)`) | 21 568 KB | 25 664 KB (+19 %: trigram table, moniker column + index) |
| full index time (walk + parse + write + resolve + rank, 4 workers) | 8.1 s | 8.5 s (10.8 s in a run concurrent with a `cargo clippy`) |

The per-call cost through sqlx roughly doubles (two extra columns to decode per row, the tier `CASE`, the third candidate subquery); the raw SQL is 0.26 ms / 0.67 ms / 0.35 ms for the three queries. All calls stay well under the reconcile that precedes them.

## Tests written
- `parse::tests::monikers_follow_the_parent_chain_and_number_siblings` — chain (`Live.render`, `inner.render`), parent declared after child, `#2` ordinal.
- `store::tests::like_pattern_joins_tokens_and_strips_wildcards`.
- `tests/index.rs::find_tiers_and_monikers` (rust fixture): `gistry` → `Registry` as `infix` with its moniker; `open db` → `open_db` as `prefix`; `new` → two `exact` rows first and tiers non-decreasing; `exact: true` → exact rows only; nested `inner.helper`; `code_refs`/`code_symbol` with `symbol = "rust:crates/a/src/lib.rs#Registry.new(method)"` pick that definition while the bare name is ambiguous; `code_outline` nodes and children carry monikers; every `code_map` row has one.
- `tests/index.rs::monikers_are_stable_across_line_moves`: temp crate, four symbols (`Foo`, `Foo.render`, `Foo.render_all`, `render`), insert three lines at the top, reconcile → identical monikers, every `line_start` + 3.
- `impact_on_a_fixture_repository`: `changed[src/util.rs].monikers == ["rust:src/util.rs#helper(function)"]`.
- `tools_envelope`: pinned hint text updated to the new wording.

Gate: `cargo test -p codeindex` 26 + 19 + 1 green; `cargo clippy -p codeindex --all-targets -- -D warnings` clean; `cargo check --workspace` green.

## Deviations
- `crates/codeindex/src/rank.rs` (not on the allow-list): `SymNode.moniker`, `Tag.moniker`, the `Graph::load` select, two test constructors — 9 lines. `code_map.rows` are `rank::Tag`s; the alternative was a second query in `tools.rs` to look monikers up by `(path, line)` per rendered row.
- Moniker language prefix is `files.language` (`typescript`, not `ts`) — see Item 2.
- Commit 1 carries both items (one schema bump); see Commits.
- `tools_envelope` asserted the old hint wording; updated rather than kept.

## Not built
- A manual trigram postings table — measured, slower and 3× larger; not kept.
- `UNIQUE` on `symbols.moniker` — by-construction uniqueness; see Item 2.
- `_` escaping in the infix pattern — costs the FTS5 LIKE optimisation for a one-char over-match.
- Monikers in `code_map` `focus` or `code_impact` `files` arguments — the brief scoped acceptance to `symbol:`.
- Design doc `60-codeindex.md` §4/§8 text itself is untouched (Sky's file); this note is the amendment.
