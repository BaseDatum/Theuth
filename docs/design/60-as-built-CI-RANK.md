# 60 — as-built CI-RANK: graph weights and personalisation of `code_map` (2026-09-28)

Worktree `~/Develop/theuth-ci-rank`, branch `feature/codeindex-rank` off `22b39455`.
Brief `/tmp/enh-2026-09-28/omp-ci-rank.md`; amends design 60 §7 (graph, `mul`, personalisation),
§8 (`code_map` args/rows), §17 bullet 1 (`pub use` re-exports), §22.1 (the `side_call.rs` case).
Ideas from the Empryo recon (`~/.theuth/artifacts/dbc0092fb984e94e.md` §3 items 5, 7, 9); the
mechanisms were read there and designed here, no code taken.

## Commits

| Commit | Subject |
|---|---|
| `6c98bb3c` | feat(codeindex): route re-exports to definers, damp barrels/type-heavy files, seed the map from edited files and entry points |
| `d284d512` | test(codeindex): code_map edited/entry rows, side_call.rs off the top, tools/lib.rs top-20 |
| `68856e4b` | fix(codeindex): type-heavy before barrel; `module` tags are not definitions |
| (this note) | docs(60): as-built CI-RANK |

One feature commit for the three items, not three: the three items share `Graph::load`, `Focus` and `Graph::rank`, and
`rank.rs` does not compile with any one of them missing (`focus()`'s new signature, `walk::entry_points`).

## What changed

### Item 1 — re-export passthrough, barrel and type-heavy damping (`rank.rs`)

**Detecting a re-export without a schema change.** The `imports` table records `pub use x::N` and
`use x::N` identically (`lang/imports.rs` keeps no `pub`/`export` marker; `names` is a JSON array).
`Graph::load` (`rank.rs:181–272`) now loads every resolved non-`mod:` row as an `ImportRow {src,
dst, names, used}` (`rank.rs:106`). `used` (`rank.rs:216`) = the file resolved a reference to one of
the names, or to anything the target defines (`Bar::new()` resolves into `Bar`'s file under the name
`new`) — both read off the `refs` edges already loaded, no extra query. A row the file imports but
never resolves a reference from is a re-export: `pub use`, `export … from`, `__init__.py` passthrough.
The file republishes, it does not use.

**Passthrough (`rank.rs:402–488`).** A re-export row is *not* an edge from the re-exporter — this is
the §17 fix: `runtime/lib.rs`'s `pub use side_call::{CallKind, ModelRole, SideCall, SideReply}` no
longer carries lib.rs's rank into `side_call.rs`. A row the file does use lands, per imported name,
on the definer when the target re-exports that name (explicit names, or a glob re-export whose source
defines it), else on the target — one level, matching `resolve/mod.rs reexport()`'s v1 depth (that
function already retargets `cargo`-provenance rows at resolve time; this covers in-crate and TS/JS/Py
rows at rank time). `<import>` edges are deduplicated per `(src, dst)`.

**Shapes (`rank.rs:627–677`, `Shape::of`).** Per file: `defs` (every tag except `module` — `pub mod x;`
is a module-tree declaration, not a definition), `fns` (`function|method`), `types`
(`class|interface|type|constant` — Rust struct/enum/trait/type/const land on these kinds via the
tags queries), `reexported` (names re-exported, glob = 1).
- *Type-heavy* (decided first): `defs > 0 && fns == 0 && types ≥ 0.8·defs`.
- *Barrel*: `reexported ≥ 1 && fns == 0`, or `reexported > 0.6·(reexported + defs)` (brief's two rules).

Type-heavy first because `used` is derived from resolved references and Rust type-position uses
(`x: Language`, `Vec<Entry>`) are not refs in the tags queries: a type module's own `use crate::…`
rows read as re-exports, and a barrel's `refs` edges are undamped — it would have escaped the
damping meant for it (`68856e4b`; the unfocused top 15 moved by ≤ 0.0001 per file, so few files were
affected on this repo).

**Damping (`rank.rs:428–441`, applied at `:448`, `:484`).** `<import>` edges into a barrel ×`BARREL_IN = 0.3` (the residual the
passthrough could not attribute: glob imports, names the barrel defines itself). `refs` edges into a
barrel are **not** damped — a ref names a definition the file owns. Decided after measuring:
`crates/tools/src/lib.rs` (131 `pub use` names, 63 definitions → 67 % re-exports → barrel by the
brief's rule) defines `Tool`, `ToolOutput`, `Risk`; damping its `refs` edges ×0.3 dropped it from #14
to below #30 for no reason a reader would accept. Edges of either kind into a type-heavy file
×`TYPE_IN = 0.5`. Empryo uses 0.3; chose 0.5 because in Rust a data-only module (`replica/types.rs`,
`mesh/types.rs`) is often exactly what a brief must show — halve the type-reference signal, do not
bury it. Effect: `replica/types.rs` #6 → #28 (0.00851 → 0.00459); `mesh/types.rs` (has `impl`
methods → Plain) unchanged at #15.

### Item 2 — personalisation from session signals (`rank.rs`, `tools.rs`)

`Focus` (`rank.rs:139–147`) gained `seeds: HashMap<usize, f64>`: the teleport vector by file index.
`Graph::focus(entries, edited)` (`rank.rs:306–365`):
- a `focus` path or an identifier's definers: `SEED_MENTIONED = 1.0`, and (as before) the file joins
  `focus.files` (×50 on out-edges, one self-edge per definition);
- an `edited` file: `SEED_EDITED = 2.0`, also in `focus.files`;
- co-change partners of edited files (from `cochanges`, loaded into the graph): `SEED_COCHANGE = 0.5 ·
  min(count, 3)/3`, seeds only — no edge boost;
- unknown paths are ignored.

Multipliers: aider's edge multipliers are decades (×50 file, ×10 identifier); the seeds are a separate
axis (teleport share), kept one decade below and in round ratios: edited 2 : mentioned 1 : co-change
≤ 0.5. Empryo's 5 : 3 : ≤ 2 was the reference; the ordering is the same, the numbers rounder. The
personalisation vector (`rank.rs:530–544`) = `seeds` normalised when any seed exists, else uniform
plus entry seeds (item 3). Warm start from `files.rank` only when there are no seeds.

`code_map` (`tools.rs:592`, `633–661`): `edited: Option<Vec<PathBuf>>`, absolute paths under the root
are stripped to root-relative; `edited` alone triggers a personalised run like `focus` does;
`focus_files` in the envelope includes edited files. Smallest diff to the render path: none — CI-RENDER
owns it.

### Item 3 — entry-point seeds (`walk.rs`, `rank.rs`, `tools.rs`)

**Choice: computed at rank time, stored in `meta.entry_points` as JSON** — no `files.is_entry` column
(CI-SEARCH owns the schema). `walk::entry_points(root, manifests)` (`walk.rs:121–264`) reads, for each
indexed manifest: `Cargo.toml` with a `[package]` → `[lib] path` or `src/lib.rs`, `src/main.rs`,
`[[bin]] path`s, `src/bin/*.rs` and `src/bin/*/main.rs` (plain `toml`, not
`cargo_toml::Manifest::from_path`: its workspace discovery and completion cost ~4 ms per manifest,
0.24 s per rank on 55 manifests); `package.json` `main`, `bin` (string or map), every string leaf of
`exports`; `pyproject.toml` `[project.scripts]` and `[tool.poetry.scripts]` `pkg.mod:func` →
`pkg/mod.py` | `pkg/mod/__init__.py`, also under `src/`. Only paths that exist are returned. Go
`package main` files come from `files.package` in `Graph::load`. Manifests under a test path
(`walk::is_test_path`) are skipped: `tests/fixtures/rust/crates/*/Cargo.toml` are entry points of a
fixture, not of the repo.

`persist` (`rank.rs:689`) writes `meta.entry_points`; `code_map` reads it and adds `entry: true` to
rows (tags and `paths_only`, `tools.rs:698`, `715`) and `entry_points: [...]` to the envelope of a
personalised run. Seeding (`rank.rs:530–540`): with no `focus`/`edited`, an entry point's teleport
weight is `1 + SEED_ENTRY` over the uniform 1. `SEED_ENTRY = 1.0`, not Empryo's +4: at +4 the leaf
crate roots nobody references (`acp/lib.rs`, `platform/lib.rs`, the fixture crates) sat at #5–#7 with
identical ranks (an entry that is also a self-edge sink keeps its teleport mass). At +1 they are at
#30–#31 (0.00444 = 4.7× uniform, still visible but below the map cut). The persisted (unfocused)
ranks are entry-seeded, so `code_map()` without arguments stays a lookup.

## Numbers (this repository, debug build, 1069 parsed files; before = `22b39455`)

`code_map()` files, top 15 — **the §17/§22.1 case**:

| # | Before (`22b39455`) | After |
|---|---|---|
| 1 | runtime/lib.rs 0.09712 | runtime/lib.rs 0.01832 [entry] |
| 2 | **runtime/side_call.rs 0.08304** | desktop/ui/src/store/reduce.ts 0.01287 |
| 3 | desktop/ui/src/store/reduce.ts 0.01221 | desktop/ui/src/store/engine-scope.tsx 0.01186 |
| 4 | desktop/ui/src/store/engine-scope.tsx 0.01203 | tools/registry.rs 0.01101 |
| 5 | desktop/ui/src/rpc/api.ts 0.00938 | desktop/ui/src/rpc/api.ts 0.00960 |
| 6 | replica/types.rs 0.00851 | desktop/ui/src/store/sessions.ts 0.00863 |
| 7 | desktop/ui/src/store/sessions.ts 0.00810 | desktop/ui/src/components/sidebar/SessionList.test.ts 0.00806 |
| 8 | tools/registry.rs 0.00760 | desktop/ui/src/lib/attach.ts 0.00744 |
| 9 | desktop/ui/…/SessionList.test.ts 0.00748 | tools/jobs.rs 0.00701 |
| 10 | desktop/ui/…/settings/shared.tsx 0.00720 | desktop/ui/…/settings/shared.tsx 0.00697 |
| 11 | desktop/ui/src/rpc/extra.ts 0.00698 | desktop/ui/src/rpc/client.ts 0.00670 |
| 12 | desktop/ui/src/rpc/client.ts 0.00685 | tools/resources.rs 0.00662 |
| 13 | tools/jobs.rs 0.00681 | desktop/ui/src/lib/rpc.ts 0.00658 |
| 14 | tools/lib.rs 0.00670 | desktop/ui/src/lib/platform.ts 0.00643 |
| 15 | tools/resources.rs 0.00669 | mesh/types.rs 0.00642 |

`side_call.rs`: #2 (0.08304) → **#137 (0.00216)**. It had no `refs` edges; its rank was lib.rs's
`pub use` edge, and the pair held 18 % of all mass (lib.rs → side_call.rs → back into lib.rs's
names). Removing the re-export edge returns that mass to the rest of the graph, which is why every
other file rose ~10 %. `runtime/lib.rs` keeps #1 at a sane 0.018 (it defines `Runtime`, `SessionId`,
`Plugin`, `subscriber_mailbox`). `tools/lib.rs` #14 → #16 (0.00670 → 0.00620): about unchanged in absolute
terms, passed by risers. `replica/types.rs` #6 → #28 (0.00851 → 0.00459, type-heavy ×0.5);
`knowledge/types.rs` #19 (0.00577), `mesh/types.rs` #15 (has `impl` methods → Plain, unchanged).

`code_map()` tags, top 15 (after; before in §22.1 order was engine-scope `scoped`, api `Api`,
SessionList.test `keys`, runtime/lib.rs `subscriber_mailbox`, resources `durable_arc`, reduce `now`,
`RpcClient`, `LocalTransport`, mesh `EngineId`, `useAction`, smoke `CallArgs`, `RegisterTool`,
`resultLine`, firecrawl `new`, jobTail `pushChunk`):

```
0.01034  desktop/ui/src/store/engine-scope.tsx:95  scoped
0.00996  desktop/ui/src/rpc/api.ts:190  Api
0.00984  runtime/lib.rs:205  subscriber_mailbox [entry]
0.00910  desktop/ui/src/components/sidebar/SessionList.test.ts:29  keys
0.00723  tools/resources.rs:55  durable_arc
0.00715  mesh/types.rs:26  EngineId
0.00657  desktop/ui/src/store/reduce.ts:967  now
0.00632  desktop/ui/src/rpc/client.ts:51  RpcClient
0.00611  tools/registry.rs:249  RegisterTool
0.00582  desktop/ui/src/rpc/transport.ts:23  LocalTransport
0.00553  scripts/apps/smoke.ts:124  CallArgs
0.00522  desktop/ui/src/components/settings/shared.tsx:109  useAction
0.00505  desktop/ui/src/lib/describe.ts:53  resultLine
0.00467  platform/src/files.rs:22  create_private
0.00454  knowledge/src/store.rs:55  Knowledge
```

`code_map(focus=["Codeindex"])` files, top 15 (before → after): index.rs 0.62025 → 0.79821, store.rs
0.10024 → 0.12838, lang/mod.rs 0.08705 → out of top 15 (0.00128 with `edited`), parse.rs 0.08603 →
0.00106, lang/imports.rs 0.04875 → 0.00090, root.rs 0.02463 → 0.03149, walk.rs 0.02463 → 0.03280,
rank.rs 0.00266 → 0.00342, tools.rs 0.00133 → 0.00076, **runtime/lib.rs 0.00088 and side_call.rs
0.00075 → gone** (resolve/mod.rs 0.00093, git.rs 0.00075, tools/edit.rs 0.00033, tools/resources.rs
0.00020, tools/registry.rs 0.00014, tools/fs.rs 0.00013, tools/lib.rs 0.00010 [entry] fill the list).
`lang/mod.rs`, `parse.rs`, `lang/imports.rs` fell for two reasons, one measured, one inferred.
Measured (throwaway query on the index): index.rs's `use crate::lang::Language`, `crate::git::Git`,
`crate::rank`, `crate::resolve::{self, Workspace}`, `crate::{CodeindexConfig, IndexStats}` rows have
no resolved reference behind them (type positions and `rank::persist(..)`-style calls the resolver
did not attribute), so the `used` rule reads them as re-exports and drops those `<import>` edges
(the rows to `parse.rs` do have edges — `parse` ×2, `default` — and keep theirs). Inferred: before,
`lang/mod.rs` 0.087 / `parse.rs` 0.086 / `lang/imports.rs` 0.049 with almost no out-edges of their
own were a cycle that trapped ~22 % of the personalised mass (the same mechanism as
lib.rs ↔ side_call.rs); with the `<import>` edges into it gone the trap is gone. The files a
`Codeindex` brief needs are still the list — index.rs, store.rs, walk.rs, root.rs, rank.rs, parse.rs,
resolve/mod.rs, lang/imports.rs — the order changed. Tags top 15: `Codeindex` 0.28410 → 0.36595,
`known_files`/`set_meta` 0.040 → 0.052, `is_test_path` 0.02841 → 0.03722; the `query` tag from
lang/imports.rs (0.02868) left the list.

`code_map(edited=["crates/codeindex/src/rank.rs"])` — the argument did not exist before (baseline run:
identical to `code_map()`). After (HEAD `68856e4b`, so the co-change walk includes this branch's commits), files: rank.rs
0.79510, store.rs 0.07176, walk.rs 0.06099, tools.rs 0.04885, tools/registry.rs 0.00570, root.rs
0.00400, tools/jobs.rs 0.00217, lang/mod.rs 0.00199, runtime/lib.rs 0.00175 [entry], tools/lib.rs
0.00135 [entry], then tools/{resources, pty, activity, communication, mcp}.rs. store.rs/walk.rs/
tools.rs are both rank.rs's referents and its co-change partners (seeds ≤ 0.5; on the base commit
before my commits joined the history they read 0.06371/0.04428/0.03138 — the co-change seed is
visible). Tags: `estimate_tokens` (tools.rs:100, the one thing rank.rs calls across files) 0.03447,
then rank.rs's own definitions at 0.02437 each (the focus-file self-edges).

Timings (same load, `map_and_symbol` alone; machine load average 40–55 throughout, siblings
compiling): `rank::persist` cold 292 ms → 420 ms, `Graph::load` 195 ms → 257–361 ms, personalised
rank 71 ms/50 it → 102–155 ms/20 it. Profile of the addition: `imports` rows + JSON names 35 ms,
`cochanges` 14 ms, `entry_points` 17 ms, re-export maps/shapes/import routing ≈ 25 ms. First
attempts were rejected on this number: a `SELECT DISTINCT file_id, name FROM refs` + qualifier-head
pass cost 0.3 s (149k refs), a correlated `EXISTS … json_each` 4–9 s; the edge-derived `used` costs
nothing extra. `cargo_toml::Manifest::from_path` per manifest cost 0.24 s → plain `toml`. The
`persist < 0.5 s` assertion in `map_and_symbol` passed standalone (420 ms) and in the full suite once
the load average dropped under 25; under load 40+ with all 17 integration tests in parallel it read
870 ms once — the baseline reads 292 ms alone under the same load, so the margin is ~130 ms, not
gone.

## Tests written

- `rank.rs`: `reexport_edges_route_to_the_definer_and_barrels_are_damped` (five-file graph: with
  every row an edge `side_call.rs` outranks `impl_.rs`; with the re-export rule lib.rs > 2×
  side_call.rs, side_call.rs ranks like an unreferenced file and has no tag; `app`'s import through
  the `index.rs` barrel lands on `impl_.rs`; `shapes()` classifies Plain/TypeHeavy/Barrel),
  `shape_thresholds` (both barrel rules, the 80 % boundary, one method flips type-heavy, type-heavy
  wins over barrel rule 1 when a type module has a "re-export"),
  `type_heavy_in_edges_are_damped` (same multipliers → equal ranks; `class` kinds → half),
  `seeds_from_edited_cochange_and_entry_points` (seed weights, co-change cap, edited beats mentioned,
  entry seeding keeps the mass sum at 1, unknown edited path ignored). Existing `toy()` tests updated
  to the `ImportRow`/`focus(entries, edited)` shape.
- `walk.rs`: `entry_points_from_manifests` (temp tree: workspace root without package skipped,
  explicit `[[bin]] path`, implicit `src/lib.rs`/`src/main.rs`/`src/bin/*.rs`, `package.json`
  `main`/`bin` map/nested `exports` with a missing `types` leaf dropped, pyproject project + poetry
  scripts under `src/`).
- `tests/index.rs map_and_symbol`: `edited=[absolute path]` → `focus_files[0]` and top file are
  rank.rs; `entry_points` names `crates/codeindex/src/lib.rs` and `crates/theuth/src/main.rs`; every
  `entry: true` row is in `entry_points`; `side_call.rs` not in the top 30; `tools/lib.rs` in the
  top 20 (was "in top 15" — it is #16 now; re-pinned as a property, not a position).

`cargo test -p codeindex`: 29 + 17 + 1 pass (twice, at `d284d512` and `68856e4b`). `cargo clippy -p
codeindex --tests -- -D warnings` clean. `cargo fmt` clean. `cargo check --workspace` exit 0
(`Graph::focus` and `CodeMapArgs` changed shape; only codeindex constructs them).

## Deviations

- None outside the allow-list. `store.rs` untouched: `entry` is added to the row JSON in `tools.rs`
  rather than as a `Tag` field, because `store::ranked_tags` also builds `Tag`s.
- `resolve/mod.rs` untouched: `reexport()` keeps retargeting `cargo` rows at resolve time; the
  rank-time passthrough covers the rest without changing stored `imports.target_file` (which
  `code_outline`/`code_refs` show).
- `tools/lib.rs` "in top 15" assertion relaxed to top 20 (evidence above).

## Not built

- **`used` is derived from resolved edges.** An import whose only references are `ambiguous`/`none`
  (e.g. index.rs's uses of `lang/mod.rs`, `parse.rs` names the resolver does not attribute) is
  classified as a re-export: its `<import>` edge (weight 0.1) is dropped and its target is routed to
  as a definer. Harmless for routing (the definer is right), lossy for the importer's out-edge; the
  `focus=["Codeindex"]` list shows it. Fix when it matters: a `pub`/`export` flag on `imports`
  (schema, CI-SEARCH) — then `used` becomes `!reexport` exactly.
- Passthrough depth one; a barrel of barrels routes to the middle barrel (its rows are re-exports
  too, so no edge leaves it; the mass stops there damped ×0.3).
- Post-rank damping (Empryo halves type-heavy scores after PageRank). Edge damping only matters
  where a source has other out-edges to compete with (§17's normalisation caveat); on this repo it
  moved `replica/types.rs` 22 places, enough.
- Edited-file signal is an argument; nothing in the runtime passes `edit`/`write_file` history yet.
- Entry points from `go.mod` module roots (only `package main` files) and from `[[bin]]` names
  without a path other than the `src/bin` convention.
