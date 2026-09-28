# 60 — as-built CI-RENDER: badges, rendered-map cache, `mode="changed"`, directory coverage (2026-09-28)

Worktree `~/Develop/theuth-ci-render`, branch `feature/codeindex-render` off `22b39455`.
Brief `/tmp/enh-2026-09-28/omp-ci-render.md`; ideas from the recon report
`~/.theuth/artifacts/dbc0092fb984e94e.md` §3 items 1, 4, 6, 8 (idea only; nothing read from
Empryo's source was translated). Amends 60 §7 (render) and §8 (`code_map` row).

## Commits

| Commit | Subject |
|---|---|
| `c8133393` | `feat(codeindex): code_map badges, rendered-map cache, mode=changed, directory coverage` |
| `31d069e4` | `test(codeindex): map render badges, cache hit/miss, changed mode on a fixture repository` |
| (this file) | `docs(60): as-built CI-RENDER` |

The three source files change together (the render signature, the store queries it needs and
the tool that calls it are one cutover), so the feature is one commit; tests and the note follow.

## What changed

### Item 1 — badges in the render and in `rows`

- `crates/codeindex/src/rank.rs:133` `MapFacts { importers: path → n, recent: paths, callers: (path, line) → n, files }`,
  loaded by `store.rs:1133 map_facts(pool, since_ns)` (four queries: importers =
  `count(DISTINCT src)` over `imports.target_file` ∪ `refs.to_symbol → symbols.file_id` at
  `confidence ≥ 0.5`, cross-file only and `mod:` rows excluded exactly like the rank graph;
  `recent` = `files.mtime_ns ≥ now − 48 h` (`rank.rs:32 RECENT_NS`); `callers` = cross-file
  resolved refs per `symbols.id`, keyed `(path, line_start)`; `files` = every parsed path).
- `rank.rs:146 file_line` → `path (→N) [NEW]`, `rank.rs:158 tag_line` → `  line: signature  ↑N`.
  Zero importers, zero callers and non-recent files print nothing (`badges_print_only_what_carries_information`).
- `tools.rs:643 badge_fields` adds `importers: i64`, `recent: bool`, `callers: i64` to every row
  of every mode (`paths_only` rows get `importers`/`recent`, `callers: 0`). The `paths_only`
  text line is `path (→N) [NEW]  rank  lines  language`.
- Decision: `callers` counts cross-file resolved references (the graph's edge definition), not
  distinct files and not same-file uses; `importers` is the distinct-file count. Recorded here
  because the brief left the unit open.

### Item 2 — rendered-map cache

- `tools.rs:670 call`: key `= {SCHEMA_VERSION}/{files count}/{max mtime_ns}/{HEAD or -}/{hour bucket}:b={budget};p={paths_only};m={mode};f={focus sorted, 0x1f-joined}`;
  `store.rs:1173 map_state` gives count + max mtime in one `SELECT`; HEAD comes from `Git::open(root).head()`
  in a `spawn_blocking` (cheap; not `meta.git_head`, which only moves when the co-change walk runs).
- Hit (`store.rs:1185 map_cache_get`, a `meta` row `map_cache:<key>`) → the stored envelope JSON
  with `cached: true`, before any graph load, rank or diff. Miss → render, then
  `store.rs:1192 map_cache_put` deletes every `map_cache:%` row whose key is not the current
  state prefix and upserts the new one (one transaction), so the table holds only renders of the
  current index state. `cached: false` in the envelope.
- Invalidation: any reconcile write changes `files` (count or `mtime_ns`), a commit changes HEAD.
  A stale answer (`Reconciled.stale`) is neither looked up nor stored. The hour bucket is in the
  key so `[NEW]` ages out within an hour of the 48 h boundary instead of never.
- No schema change: `meta (key, value)` as it is.

### Item 3 — `mode: "full" | "changed"`

- `tools.rs:601 CodeMapArgs.mode`, default `full`; anything else is a tool error, and
  `paths_only` with `changed` is a tool error (`"paths_only has no meaning with mode=\"changed\""`).
- `tools.rs:879 changed`: `Git::changes(Range::Worktree)` (the same call `code_impact(git_range="WORKTREE")`
  makes, run only on a cache miss), hunks mapped onto the stored outline rows with the existing
  `touched` (the reconcile just made the index describe the disk, which *is* the diff's new side, so
  no new-side re-parse and no `(name, kind)` fallback — a `description` change in one `impl` no
  longer lists every `description` in the file), direct importers from `store::import_edges`
  (first 5, then `(+N)`), blocks ordered by `files.mtime_ns` descending (deletions first), fitted
  block-wise to `budget` with `… +N files`. Header `changed N files, M symbols`; a clean tree
  renders exactly `changed 0 files, 0 symbols`.
- Block shape: `path (→N) [NEW] [added|deleted]` (nothing for modified — the default in a changed
  map), `  line: signature  ↑N` per touched definition, `  ← importer, importer`.
- Envelope: `rows` = touched definitions `{path, line, kind, name, signature, status, importers, recent, callers}`
  (no `rank`: they are not ranked), `changed_files: [{path, status, importers: [path]}]`, `mode`.
  `focus` is accepted and ignored in this mode (it stays in the cache key).

### Item 4 — directory coverage

- `rank.rs:473 component(path)`: depth-2 prefix (`crates/codeindex`, `src/store`); a file one
  directory deep → that directory; a root file → `""`. Manifest directories are **not** consulted
  (`files.package` is a crate name, not a path; depth-2 covers `crates/*`, `packages/*`, `src/*`).
- `rank.rs:489 render(tags, budget, facts) -> Rendered { text, kept: Vec<usize>, coverage_added }`:
  after the binary-search fit, every component holding ≥ 5 % of the parsed files
  (`rank.rs:35 COVERAGE_SHARE`) with no line in the tree gets `file_line` of its highest-ranked
  tag's file, appended in rank order. While the text exceeds `max(budget, fitted tokens)`, the
  lowest-ranked kept tag of the component with the most kept tags (ties: alphabetical first) is
  dropped — never a component's last tag; if nothing can be dropped, coverage lines are popped from
  the end. `kept` is no longer a prefix, so `rows`/`dropped` follow the index list.
- `coverage_added` in the envelope.

## Numbers (this repository, debug build, machine shared with sibling worktrees: load 5–55)

`code_map(focus=["Codeindex"], budget=1500)`:

| | Before (22b39455) | After |
|---|---|---|
| Estimated tokens / rows / dropped | 1 362 / 60 / 2 072 | 1 496 / 54 / 2 078 (+3 coverage lines) |
| Files in render order | `codeindex/src/index.rs` (16 tags), `codeindex/src/store.rs` (24), `lang/imports.rs`, `root.rs`, `walk.rs`, `rank.rs`, `tools.rs`, `git.rs`, `parse.rs`, `tools/src/edit.rs`, `resolve/mod.rs`, `tools/src/resources.rs`, `tools/src/registry.rs`, `tools/src/fs.rs`, `lang/mod.rs`, `runtime/src/lib.rs` | same order and files (rank untouched) up to `lang/mod.rs`, then coverage: **`crates/agent/src/lib.rs (→37)`, `crates/rpc/src/api.rs (→11)`, `crates/desktop/src/island/mod.rs (→1)`** — `runtime/src/lib.rs`'s two tags paid for them |
| First lines | `crates/codeindex/src/index.rs` / `  23: const POOL_LRU: usize = 8;` | `crates/codeindex/src/index.rs (→7) [NEW]` / `  23: const POOL_LRU: usize = 8;` / … / `  116: pub async fn reconcile(  ↑1` |
| Badges seen | — | `crates/tools/src/registry.rs (→46)` → `249: pub struct RegisterTool(pub DynTool);  ↑92`; `crates/tools/src/resources.rs (→29)` → `durable_arc  ↑28`; `root.rs (→6)`: `discover ↑5`, `under_root ↑4`, `rel_str ↑5`; `store.rs (→7)`: `meta ↑5`, `counts ↑5`, `known_files ↑4` |
| `[NEW]` | — | on every file: the worktree was checked out today, so every `mtime_ns` is < 48 h. Truthful to the brief's definition; on a long-lived checkout it marks recent edits only |

Timings (same call, warm index; wall clock, `Instant` around `Tool::call`, includes the reconcile walk):

| | Load ≈ 5 | Load ≈ 35–55 |
|---|---|---|
| Before, uncached (every call) | 296–380 ms | — |
| After, cache miss (rank + facts + render + store) | — | 575–580 ms |
| After, cache hit | 78–84 ms | 91–160 ms |
| `code_map(mode="changed")`, miss | 1.86 s | 3.4–9.3 s |
| `code_map(mode="changed")`, hit | 199 ms | — |
| `code_impact(git_range="WORKTREE")` for comparison | 2.0–2.5 s | 6.7 s |

A hit is the reconcile walk plus `gix` open + HEAD + two `SELECT`s. The `changed` miss is the
worktree diff's cost (every indexed HEAD blob read and hashed against disk), identical to
`code_impact(WORKTREE)`; the cache removes it for the repeat call. The miss is heavier than the
old uncached call by the four fact queries and the JSON store; a low-load after-miss figure was
not obtainable (siblings were compiling throughout).

`code_map(mode="changed", budget=1500)` on this worktree after the edits (798 estimated tokens; `[NEW]` everywhere for the reason above):

```
changed 5 files, 32 symbols
crates/codeindex/tests/zz_numbers.rs [NEW] [added]
  10: async fn numbers() {
crates/codeindex/tests/index.rs [NEW]
  1231: async fn map_and_symbol() {
  2013: async fn map_render_badges_cache_and_changed_mode() {
crates/codeindex/src/tools.rs (→5) [NEW]
  586: pub struct CodeMapArgs {
  605: const CHANGED_IMPORTERS: usize = 5;
  608: struct ChangedBlock<'a> {
  618: fn text(&self, facts: &rank::MapFacts) -> String {
  643: fn badge_fields(v: &mut serde_json::Value, facts: &rank::MapFacts, path: &str, line: i64) {
  666: fn description(&self) -> String {
  670: async fn call(&self, a: CodeMapArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
  787: async fn full(
  873: async fn changed(
  ← crates/codeindex/src/outline_hook.rs, crates/codeindex/src/rank.rs, crates/codeindex/tests/index.rs, crates/codeindex/tests/zz_numbers.rs
crates/codeindex/src/rank.rs (→3) [NEW]
  32: pub const RECENT_NS: i64 = 48 * 3600 * 1_000_000_000;
  35: const COVERAGE_SHARE: f64 = 0.05;
  133: pub struct MapFacts {
  146: pub fn file_line(&self, path: &str) -> String {
  158: pub fn tag_line(&self, path: &str, line: i64, signature: &str) -> String {
  167: pub fn callers(&self, path: &str, line: i64) -> Option<i64> {
  178: pub struct Rendered {
  473: pub fn component(path: &str) -> &str {
  489: pub fn render(tags: &[Tag], budget: u32, facts: &MapFacts) -> Rendered {  ↑1
  619: mod tests {
  770: fn tags(n: i64, path: impl Fn(i64) -> String) -> Vec<Tag> {
  785: fn render_fits_the_budget_within_slack() {
  806: fn badges_print_only_what_carries_information() {
  823: fn components_are_depth_two() {
  831: fn coverage_adds_one_line_per_large_silent_component_within_budget() {
  ← crates/codeindex/src/index.rs, crates/codeindex/src/tools.rs
crates/codeindex/src/store.rs (→7) [NEW]
  1133: pub async fn map_facts(pool: &SqlitePool, since_ns: i64) -> anyhow::Result<crate::rank::MapFacts> {  ↑1
  1173: pub async fn map_state(pool: &SqlitePool) -> anyhow::Result<(i64, i64)> {  ↑1
  1182: const MAP_CACHE: &str = "map_cache:";
  1185: pub async fn map_cache_get(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {  ↑1
  1192: pub async fn map_cache_put(  ↑1
  ← crates/codeindex/src/index.rs, crates/codeindex/src/locate.rs, crates/codeindex/src/rank.rs, crates/codeindex/src/resolve/mod.rs, crates/codeindex/src/tools.rs (+2)
```

(`zz_numbers.rs` was the throwaway probe that produced these numbers; deleted before the commit.
Before the index-row fix, the same map listed every `description`/`call` in `tools.rs` — 41 symbols.)

Fixture repository (`map_render_badges_cache_and_changed_mode`, 3 files, one commit):
`src/util.rs (→1) [NEW]` / `  1: pub fn helper() -> u32 {  ↑1`; second call `cached: true`, same
text; after editing `helper` and adding `src/extra.rs`: `cached: false`, then
`changed 2 files, 2 symbols` / `src/extra.rs [NEW] [added]` / `src/util.rs (→1) [NEW]` /
`  1: pub fn helper() -> u32 {  ↑1` / `  ← src/lib.rs`; `unrelated` absent.

## Tests written

- `rank.rs` unit: `render_fits_the_budget_within_slack` (adapted to `Rendered`),
  `badges_print_only_what_carries_information` (exact text; zero importers → no badge; `callers` of an
  uncalled line → `None`), `components_are_depth_two`,
  `coverage_adds_one_line_per_large_silent_component_within_budget` (one line for a 50 % component,
  paid by dropping the fat component's lowest tags when the fit overshoots, budget respected, nothing
  for a < 5 % component or one already shown).
- `tests/index.rs::map_render_badges_cache_and_changed_mode` (fixture git repository through
  `Tool::call`): row fields and text badges, cache hit / miss on other arguments / miss after an
  edit / hit on the repeat `changed` call, `changed` header on a clean tree, touched-only
  definitions, `[added]`, importers line and `changed_files`, both argument errors.
- `map_and_symbol` gained `mode: None`; nothing else in the existing suite changed.

Gate: `cargo test -p codeindex` 27 unit + 18 integration green; `cargo clippy -p codeindex
--all-targets -- -D warnings` clean; `cargo check --workspace` clean. `map_and_symbol`'s
pre-existing `rank persist < 0.5 s` assertion failed once under load 50 (persist untouched here)
and passed on the rerun.

## Deviations

- None outside the allow-list. `git.rs` untouched: `Git::changes(Range::Worktree)` was already
  shareable. `code_impact` untouched (`touched`, `Sym`, `parse_new_sides` reused as they are).
- `rank.rs` edits are confined to the new types after `Ranked`, the new constants, `component`,
  `render` and the test module; `Graph`, `Focus`, `rank`, `persist` and every weight are as CI-RANK
  left them.

## Not built

- Manifest-directory components (Cargo/package.json): depth-2 stands in; add a `files.package`
  → path join if a repo's crates live deeper than two levels.
- Caller *names* in the badge (Empryo's `[a, b, +N↑]`): count only, cheaper and enough to sort by.
- Cache eviction by age or size: rows of a superseded state are deleted on the next store; the
  current state keeps one row per distinct argument set.
- `[NEW]` relative to HEAD's commit time rather than `mtime_ns` (would fix the fresh-checkout
  noise); the brief specified `mtime_ns`.
- `changed` mode does not list unindexed changed files (docs, YAML); `code_impact` does.
