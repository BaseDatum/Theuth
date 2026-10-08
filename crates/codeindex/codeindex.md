---
name: codeindex
description: 'How to use theuth''s code_find/code_outline/code_refs/code_symbol/code_map/code_impact runtime functions (design 60): when to prefer them over grep/read_file, briefing a child with a map, verifying a diff with code_impact, and how to read provenance/confidence/ambiguous. Load before structural work on a source tree.'
---

# codeindex — structural questions about a source tree, cheaply

Six runtime functions (Python coroutines, `ReadOnly`) over a tree-sitter symbol/reference index of a repository (design 60). The index lives under `~/.theuth/codeindex/<slug>-<hash>.sqlite`, never inside the repo; the first call on a root indexes it (theuth itself: ~5 s), later calls re-parse only changed files. Default root = the git top-level of the process cwd; pass `root=` for another checkout (a worktree, a clone under /tmp).

## When to reach for it
- **Before `grep`/`read_file`** when the question is structural: where is X defined, what does this file contain, who references Y, what does this diff touch.
- **Briefing a child** (omp/opencode/subagent): give it `code_map(focus=[...], budget=1500)["content"]` plus the `code_find`/`code_refs` rows for the symbols it will touch, instead of "look at the registry". Paste `m["content"]`, not whole files.
- **Checking a child's work**: `code_impact(git_range="main..HEAD")` (or `"WORKTREE"` for uncommitted changes, or one commit hash) lists the symbols changed, the files that reference them (`direct`, with `via` names), importers of those (`transitive`, depth ≤ 3) and the tests among them — then read only those.

## The tools
| call | returns (Python value = the envelope) |
|---|---|
| `code_find(name, kind=None, exact=False, root=None, budget=None)` | `{rows: [{symbol_id, name, kind, path, line_start, line_end, signature, exported, rank}], …}` — FTS prefix match on name/signature/docs unless `exact` |
| `code_outline(path, root=None, budget=None)` | `{path, language, lines, parse_errors, imports: [{specifier, target, provenance}], symbols: [nested by parent]}` |
| `code_refs(symbol, direction="in", kind=None, root=None, budget=None)` | `{definition, refs: [{path, line, from_symbol, kind, provenance, confidence}], ambiguous, by_file}`; `symbol` = name, `path:line`, or numeric `symbol_id`; several definitions → `candidates` and no refs (pick one) |
| `code_symbol(symbol, sections=None, root=None, budget=None)` | `{definition, body, callers, callees, cochange}` — one call instead of find+read+refs |
| `code_map(focus=None, paths_only=False, root=None, budget=None)` | ranked repo map (aider-style PageRank); `focus` = identifiers and/or paths to personalise toward; `content` is the text tree to paste into a brief |
| `code_impact(files=None, git_range=None, include_tests=False, root=None, budget=None)` | `{changed: [{path, symbols_changed}], direct: [{path, via}], transitive, tests, ambiguous_skipped, unindexed}` |

Every envelope carries `root`, `indexed_files`, `stale` (true when > 64 files changed and the rest is still indexing in the background — call again), `dropped` (rows cut by `budget`, estimated tokens = chars/3, default 4000).

## Reading `provenance` / `confidence`
| provenance | meaning | confidence |
|---|---|---|
| `same_file` | definition in the same file | 1.0 |
| `import` | the referencing file imports the defining file (Cargo workspace + `mod` tree, tsconfig/package.json, Python, go.mod) | 1.0 |
| `package` | one candidate in the same Cargo/npm/Python/Go package | 0.8 (0.4 if the call has a `.` receiver) |
| `unique` | one candidate in the whole root | 0.6 (0.3 with a receiver) |
| `ambiguous` | several candidates, none chosen — `candidates` says how many | 0.0 |
| `none` | no definition in the index (std, external crate, unindexed language) | 0.0 |

Rules: a reference is never fanned out to every same-named definition — an ambiguous count is the honest answer; receiver method calls (`x.clone()`, `s.push()`) on ubiquitous names are always ambiguous (there is no type inference — that is the LSP/SCIP seam). `code_impact` and the rank graph only use edges with confidence ≥ 0.5. Treat `unique`/`package` on a receiver call as a hint, `import`/`same_file` as fact.

## Limits to remember
- Languages parsed: Rust, TypeScript/TSX, JavaScript, Python, Go. YAML/TOML/JSON/Markdown are files only (they appear in `code_impact.changed` with `symbols_changed: []`).
- `agent/loop_.rs`-style files (big referencers that define little others use) rank low in `code_map`; use `focus=["crates/agent/src/loop_.rs"]` to pull them in.
- `code_refs("new")` on a common name → hundreds of candidates; pass `path:line` from `code_find` instead.
- `codeindex_stats(root?)` is an RPC method (desktop/CLI), not a runtime function.
- Nothing here edits anything; `read_file` still reads, `edit` still edits.

## Wave 2 additions (2026-09-28, design 60 as-built CI-SEARCH/IMPACT/RENDER/RANK + WAVE2-MERGE)
- **`code_find` is infix now**: tiers `exact → prefix → infix → text`, each row carries `match`; `"Registry"` finds `ToolRegistry`, `"tool registry"` (space-split) too. `exact=True` unchanged.
- **Every row has a `moniker`** — `<language>:<path>#<Qualified.Name>(<kind>)` (e.g. `rust:crates/codeindex/src/rank.rs#render(function)`, `#2` suffix for same-name siblings). It survives line moves; pass it as `symbol=` to `code_refs`/`code_symbol` instead of `path:line` when briefing a child or writing a board.
- **`code_map` rows carry badges**: `importers` (distinct files importing/referencing the file), `callers` (cross-file resolved refs to the symbol), `recent` (mtime < 48 h — every file on a fresh checkout), `entry` (crate/package entry point). Text render: `path (→N) [NEW]`, `  line: signature  ↑N`. Envelope adds `cached` (render cache keyed on files/mtime/HEAD/budget/focus/edited/mode), `coverage_added` (one line per large directory the budget would have hidden), `entry_points`.
- **`code_map(edited=[paths])`** personalises toward files edited this session (stronger than `focus`, co-change partners lightly). Use it when briefing about work in progress.
- **`code_map(mode="changed")`**: only files changed in the worktree vs HEAD + their direct importers, ordered by mtime, header `changed N files, M symbols` — a few hundred tokens to tell a child what is being worked on. Cache hit ≈ 80 ms; first WORKTREE diff is the expensive part.
- **Ranking**: `pub use`/barrel re-exports no longer absorb rank (`side_call.rs` #2 → #137); type-only modules damped ×0.5. Known loss: an import whose refs are all `ambiguous` is treated as a re-export (its 0.1 import edge is dropped).
- **`code_impact` adds `tests_by_subject`** (tests linked to a changed/direct file by naming convention or most-imported file — catches API/CLI tests that never name the symbol); `content` marks each test `(ref)`, `(subject)` or both. `code_symbol` gains `tests: [path]`.
- **`project_profile(root=None)`** — new read-only function: `{kind, package_manager, commands: {check, test, lint, format, build, typecheck}, test_runner, ci, manifests, scripts}` from justfile/Makefile/manifests/lockfiles (justfile recipe wins). Put its `commands` in a child's brief instead of guessing `cargo test`/`bun run test`.
- **Honesty fields**: if a reconcile would delete > 80 % of indexed files (moved root, unmounted volume) it refuses; every envelope then shows `stale: true, suspect_root: true, hint`. Nothing is silently emptied.
- **CodeMap panel (design 73)**: every `code_*` call publishes `CodeToolRan`, which makes the CodeMap button available in the desktop session (side pane / phone sheet / pop-out) — lenses Here (ranked tree + badges), Fit (callers → focus → callees, ≤ 20 nodes), Change (impact rings + tests). You do not drive it; it follows your calls. `edit`/`write_file` on an indexed root refresh it via `codemap_touch`.

## The map (design 147: `code_overview`)
The CodeMap has a stored **map** per indexed root: perspectives (`overall-architecture` always; `request-lifecycle`, `data-flow`, `dependency-map`, `storage`, … only when the index evidences them) → drillable elements, each one small diagram (≤ 12 nodes, every node anchored to indexed paths) plus prose (`description`, then `context`/`constraint`/`concern`/`todo`/`note` when present). The index decides the structure, sizes and every edge's evidence; a model names the groups, labels the edges and writes the prose — once, on the first open of the CodeMap panel, charged to that session. **`code_overview()` never generates and costs no tokens**: it reads what is stored.

- `code_overview(root=None, element=None, perspective=None, budget=None)` → `{perspectives: [{id, title, origin, elements, stale, tree: [top-level rows]}], map_state, generated, hint, …envelope, content}`. `content` is omm's `tree` form: one indented line per element (`name  path  (N inside)  stale  imported`). Pass `perspective=` to list one tree.
- `code_overview(element="overall-architecture/unit:crates/codeindex")` (an element id from the tree, or **a path** — `element="crates/codeindex/src/lens.rs"` resolves to the deepest element whose anchors contain it) → the element: `{id, name, path, fields, mermaid, children: [{id, name, path, leaf, stale}], edges: [{from, to, label, backed, evidence}], anchors, stale, provenance}`; `content` appends its prose and the diagram as a ` ```mermaid ` block.
- **When**: first contact with a repository (before `code_map`), before briefing a child (paste the element's `content` — name, paths, description, edges — as its "where to look"), when the user asks how something works or what a crate/directory is for.
- **Reading it**: an edge `backed` has index evidence (`manifest`, `imports N`, `refs N`, a call `site`); `inferred` is the model's claim with its `reason` — treat it as a hint. `stale` = the files under the element changed since it was written; the structure (children, anchors, counts) is live, the prose may lag. `map_state: structure` means nobody has opened the CodeMap yet: names are basenames, no prose — `hint` says so. `provenance` names the model and HEAD that wrote it; `origin: imported` elements come from a `<root>/.omm/` written by another tool and are unverified.
- **Next steps** from an element: `code_symbol`/`code_refs`/`code_outline` on its `anchors`; `mermaid` pastes into a board (skill `board`) or a reply as-is.
- The generated prose is model-written (`generated: true` in the envelope): same trust as `symbols.docs` — quote it as the map's claim, verify with `code_refs` before acting on it.
