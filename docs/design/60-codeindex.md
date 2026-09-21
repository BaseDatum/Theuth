# 60 — Code index: symbols, references and a ranked repo map from tree-sitter tags

Status: **proposed** (2026-09-21). Nothing in this doc is built. Extends `00-architecture.md` §1.8 (amended below), §4 (tools), §9 (data dir), §10 (crate table), §12 (phases); `12-course-correction.md` §10 (the `grep`/`edit` ruling it generalises); `08-repl.md` §0.4 (runtime-only tools); `10-knowledge.md` §3 (derived sqlite convention it copies). Wins over 00 §1.8 where they disagree; nothing else. Reference implementation for design only: `~/Develop/qartez-mcp` (v0.11.0, `7fdb509`; dual-licensed — read, never copy).

Scope: a plugin crate `codeindex` that answers structural questions about a source tree on this machine — *where is `X` defined, who references it, what is in this file, which files matter for this task, what does this diff touch* — from a derived SQLite index built with tree-sitter's shipped tag queries. Tools are runtime-only, `ReadOnly`, return structured values, and never write into the user's repository. The orchestrator uses them to brief ACP children and to check their work without paging whole files through the context window (08 §0). It is not an IDE: no language servers, no type inference, no refactoring, no editing.

Not in v1 (and the seams left for them): a file watcher/daemon (v1 reconciles by mtime on each call — the `Index::reconcile` entry point is where `notify` plugs in); SCIP ingest for compiler-precise Rust (`rust-analyzer scip` → `index.scip`; the `symbol_refs.provenance` column has the value `scip` reserved); embeddings (10 §3 rejected them for knowledge; same answer here; the schema has no vector column and needs none); Java/C/C++/Go beyond what a one-line grammar addition gives; refactor tools (`rename`, `move` — qartez has them, 00 §1.8 keeps them out); security/smell/clone scanners; a fifth `SearchCategory` for human search (49 §4.1 is closed; a later doc may amend it); multi-root "workspaces" of unrelated repos (v1: one root per index, many indexes).

## 0. Decisions in one screen

1. **A plugin crate `codeindex`** (functional name, 00 §1.1), not an extension of `tools::fs`. It has its own store on disk, its own actor, its own tools and config section — the `plugins.md:16` row for "a plugin", not the `:13` row for "a tool in the plugin that owns the state". Rejected: adding symbol awareness to `grep`/`read_file` (no place for an index to live; `tools` would grow a store). Rejected: running qartez or codegraph as an MCP server as *the* answer — both remain one `mcp/*.json` away for anyone who wants them, but qartez's licence forbids embedding (`~/Develop/qartez-mcp/LICENSE`, `COMMERCIAL.md`), it is single-author and dormant since 2026-07-03, and its import graph came out empty on this workspace (§1); codegraph is a Node daemon per project with telemetry on by default and its intelligence in TypeScript. Neither can share the bus, the runtime's structured returns, or the `Risk` contract (unannotated MCP tools default to `ask`, 12 §5).
2. **00 §1.8 is amended, not broken.** The ruling in 12 §10 — "§1.8 rules out *coding-specific* tools, not searching and editing files" (`12-course-correction.md:220`) — is extended one step: *understanding* files (symbols, references, structure) is assistant work the same way searching them is. LSP, notebooks, git clients, diff review and refactoring stay out; §15 has the new wording. Rejected: an LSP client (`async-lsp` + `lsp-types`) — per-language server installs, a process per project, and the exact thing §1.8 names.
3. **Parser = `tree-sitter` 0.27 + `tree-sitter-tags` 0.27 running the grammars' shipped `TAGS_QUERY`** (00 §1.2: off-the-shelf first). Every tree-sitter competitor hand-wrote per-language node walkers — qartez `src/index/languages/rust_lang.rs` is 3.2k lines, codegraph's `c-cpp.ts` 1,793 — over the same grammar crates whose authors already ship `queries/tags.scm` with `@definition.*`/`@reference.*` captures. We write one small *import* query per language (tags queries do not cover imports) and nothing else per language. Rejected: hand-written walkers (the reinvented wheel); `tree-sitter-graph` (dormant since 2024-12); `github/stack-graphs` (archived 2025-09-09, four languages, grammar versions frozen).
4. **Languages in v1: Rust, TypeScript/TSX/JavaScript, Python, Go.** Those are the trees this harness is pointed at today (theuth; the SRE FastAPI apps; TS frontends; Go tooling). Each is one grammar crate depending only on `tree-sitter-language ^0.1`, so core can move independently (§2.B). YAML/TOML/JSON have no tags queries and are indexed as files only (they appear in the map by path, contribute no symbols). Rejected: qartez's 30 linked grammars — compile time and binary size for languages nobody here reads; 00 §1.6.
5. **Resolution is import-first and workspace-aware, and never guesses.** Cross-file edges come from (a) import resolution — Cargo workspace members via `cargo_toml` + `globset`, the Rust `mod` tree from `mod x;` declarations, `oxc_resolver` for TS/JS (`tsconfig` paths, `package.json` exports), a hand-written relative/absolute resolver for Python, Go by module path — then (b) name resolution restricted to the files the referencing file imports, its own file, its own package, or a globally unique definition. Every `symbol_refs` row carries `provenance` and `confidence`; an ambiguous name is stored **unresolved with its candidate count**, not linked to every same-named definition. Rejected: qartez's rule that bare method calls link to every same-named method (`src/index/mod.rs:1017-1060`, accepted inflation) — the orchestrator would rather be told "ambiguous, 4 candidates" than be handed a false caller list. Rejected: type inference — that is the LSP/SCIP seam.
6. **The repo map is aider's algorithm**: a file graph with referencer → definer edges weighted by identifier rarity, personalised PageRank, rank spread per `(file, identifier)`, rendered top-down until a token budget is met. Own ~60-line sparse power iteration. Rejected: `petgraph::algo::page_rank` — unweighted, no personalisation vector, documented O(n·|V|²·|E|). Rejected: qartez's file PageRank over the import graph alone — on a Cargo workspace it had 118 edges to work with (§1).
7. **One derived SQLite per root under `<data_dir>/codeindex/`**, sqlx, WAL, `CREATE … IF NOT EXISTS`, no migrations — 10 §3 verbatim ("a schema change is `rm`"). File name `<slug>-<hash8>.sqlite` where slug is the canonical root path with `/` → `-` and hash8 the first 8 hex of its SHA-256, so `ls` is readable and two roots never collide. Nothing is written inside the user's repository. Rejected: `.codeindex/` in the repo (what `.qartez/` did to this tree — 14 MB of WAL that needed a global gitignore); one shared database for all roots (WAL contention across unrelated repos; deleting one root's index becomes a `DELETE`, not an `rm`).
8. **Freshness by reconcile-on-call, no daemon.** Every tool call walks the root with `ignore` (gitignore-aware, the same walker `grep` uses, `crates/tools/src/fs.rs:246`), compares `(mtime_ns, size)` per file with the `files` table (10 §3.2's sync key), and re-parses only the changed set. ≤ 64 changed files: synchronously, then answer. More: answer from the current index with `stale: true` and the count, and finish in a `spawn_blocking` task (codegraph's staleness banner, made structured). Rejected: `notify` watcher per root in v1 — roots are chosen per call and may be many; the seam is one function. Rejected: qartez's serve-before-indexed behaviour without saying so.
9. **Six tools, all `Risk::ReadOnly`, `DIRECT = false`, structured returns**: `code_map`, `code_find`, `code_outline`, `code_refs`, `code_symbol` (compound: definition + body + callers + callees + co-change — qartez `understand` without the round trips), `code_impact` (changed files or a git range → symbols touched, their referencers, transitive files, tests). Each takes an optional `root` and a `budget` (estimated tokens, `chars / 3` as qartez does; default 4000); rows are priority-ordered and cut at the budget with a `dropped` count. Rejected: qartez's 43 tools and tiering — the runtime already hides tools behind signatures (08 §4.2), and every tool the model might reach for is a line of system prompt. Rejected: `code_read` — `read_file` with `offset`/`limit` already is that, and `code_symbol` returns spans.
10. **Root defaults to the git top-level of the process cwd** (`gix::discover`), else cwd. `ToolCx` carries no cwd (`crates/tools/src/lib.rs:243-255`); `grep` uses `std::env::current_dir()` (`fs.rs:219-222`) and so does this. Roots outside the user's home are allowed (a checkout under `/tmp`), roots that are not directories are an error.
11. **Git signals from `gix`, already in the workspace** (`Cargo.toml:14`): co-change from a first-parent walk of the last 300 commits (2–20 files per commit, qartez's bounds), changed files for `code_impact` from a tree diff. Rejected: `git2` (a second git library) and shelling out to `git`.
12. **One prompt paragraph, pure function of config** (`RegisterPrompt`, `registry.rs:332-340`): what the six functions are for and that they are runtime coroutines. Longer guidance (when to brief a child with a map, how to read `provenance`) goes in a skill under `~/.theuth/skills/`, as `plugins.md:228` says.

## 1. Why

The orchestrator's job on a code task is to brief a child precisely, then check what came back (00 §1.8, 06). Today both halves are done with `grep`, `read_file` (64 KiB cap, `fs.rs:14`) and `list_dir` (one level, non-recursive, `fs.rs:173-187`): finding where a type is registered is three or four greps and two file reads, and each read lands 10–60 KB in the runtime and a slice of it in the transcript. A child that receives "look at the registry" instead of "`ToolRegistry::start` at `crates/tools/src/registry.rs:1366-1439` registers 13 builtins; `RegisterTool` is asked at `knowledge/src/tools.rs:33`" spends its first ten minutes rediscovering the second sentence. Verifying a diff means reading every changed file to see what else refers to what changed.

Sky ran qartez against this repository on 2026-09-12 and found it valuable — `find`, `outline`, `refs` answer in ~50–500 tokens what grep answers in thousands. The same run shows the limit of its approach on this tree (read from `.qartez/index.db` on 2026-09-21): 173 files indexed against ~290 `.rs` files in `crates/` today; 10,275 symbols and 15,052 symbol references — but **118 file edges, all `super::`; `co_changes` = 0; `file_clusters` = 0**. Its Rust import resolver handles `crate::`, `super::` and `self::` only (`src/index/mod.rs:1508-1560`); every `use tools::…`, `use runtime::…`, `use knowledge::…` across the 36 workspace crates resolved to nothing, so `map`, `impact`, `deps`, `hotspots`, `boundaries` — everything built on the file graph — was empty here. The symbol layer worked; the graph did not. That is the gap this design closes first, and the reason the reference implementation is a reference and not a dependency.

Why native rather than MCP, in 38's terms (38 §0.5–0.6): one binary, no daemon, no second process to install; the tools return `structured` so results are Python values (08 §4.2, `agent/src/repl.rs:1283-1289`), not text to re-parse; `Risk::ReadOnly` is declared, not inferred from missing annotations; and the index can later feed things only an in-process plugin can — a `KnowledgeChanged`-style event, a `code_map` in a subagent brief template, the `decision` oracle's readability gate (57).

## 2. Facts relied on (recon 2026-09-21)

### A. This tree (`~/Develop/theuth` @ `feaf6b1`)

- `Tool` trait: `crates/tools/src/lib.rs:257-290` — `NAME`, `RISK`, `Args: DeserializeOwned + JsonSchema`, `RETURNS = "str"` (`:268`, "name its shape" when `structured` is filled), `DIRECT = false` (`:272`), `description()`, `async fn call(&self, args, cx: ToolCx) -> anyhow::Result<ToolOutput>`. `Risk` `:85-98`. `ToolOutput { content, is_error, structured: Option<Value>, files }` `:206-220`, `ok`/`err` `:224-233`. `ToolCx { session, call_id, quiet, resources }` `:243-255` — no cwd, no project root.
- Registry: `REGISTRY_ACTOR = "tools"` `lib.rs:77`; `RegisterTool(pub DynTool)` `registry.rs:231-232`; `RegisterPrompt { plugin: &'static str, text: String }` `:332-340` ("Pure function of config … anything dynamic busts the system cache"); builtins `:1393-1411`; policy default `read_only: allow` `lib.rs:450-458`, `main.rs:285-290`.
- Plugin shape to copy: `crates/knowledge/src/lib.rs:296-334` (`rt.section("knowledge")` → `if !cfg.enabled { return Ok(()) }` → open store → `tokio::spawn` + `rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR)` → `tools::register`); `crates/knowledge/src/tools.rs:24-44` (`RegisterPrompt` then seven `RegisterTool`); config struct `lib.rs:52-65` (`#[serde(default)]`, `Default { enabled: true }`).
- Tool template: `crates/scrape/src/tools.rs:76-153` (`ScrapeUrlArgs` with schemars doc comments, `RETURNS = "dict"`, `Risk::ReadOnly`, `DIRECT = false`); `structured()` helper `:41-46`; `RETURNS = "list[dict]"` `:213`; PROMPT `:16-22`; compile-time `assert!(!<T as Tool>::DIRECT)` `:359-363`.
- `grep`: `fs.rs:1-4` ("Paths are resolved against the process cwd; `~` is not expanded"), root = `std::env::current_dir()` `:219-222`, `ignore::WalkBuilder` + `grep-searcher` `:238-286`, `GREP_LIMIT = 200` `:232`. `read_file` cap `MAX_BYTES = 64 * 1024` `:14`, markers `:19-22`. `list_dir` one level `:173-187`.
- Store convention: `crates/knowledge/src/store.rs:1-5` ("Files are the truth; `knowledge.sqlite` is derived … can be deleted at any time"); `open_db` `:377-387` (`create_if_missing`, `journal_mode(Wal)`, `busy_timeout 5s`, `max_connections(4)`, `CREATE … IF NOT EXISTS`); corrupt index → remove `""`, `-wal`, `-shm` and rebuild `:429-431`; sync key `(mtime as unix nanos, size)` `:162-163`; `DIR = "knowledge"`, `INDEX_FILE = "knowledge.sqlite"` `lib.rs:44-46`. Turn-start resync is a direct call from the agent loop, `crates/agent/src/loop_.rs:2378-2399`.
- Runtime: `Runtime { bus, config, data_dir: PathBuf, shutdown }` `crates/runtime/src/lib.rs:22-33`; `section()` `:67`; `lookup_when()` `:73` (50 ms poll); `Plugin { name, start }` `:213-218` ("must not block on other plugins").
- Python side: `structured` replaces `content` as the return value, `crates/agent/src/repl.rs:1283-1289`; `RUN_OUTPUT_CHARS = 8_000` `:39`; `py_type`: `"array"` → `list[…]`, `"object"` → `dict` `:243-247`, so `RETURNS` strings `dict` / `list[dict]` are the vocabulary.
- Wiring: plugin vector `crates/theuth/src/main.rs:530-562` (`scrape::ScrapePlugin` among the first twelve; `decision::DecisionPlugin` at `:562`); `DEFAULT_CONFIG` `:128`, `# scrape:` block `:172-181` is the format for an optional plugin section; RPC pattern `crates/rpc/src/api.rs:1154-1171` (`lookup`, `ask`), `knowledge()` wrapper `:1285-1290`; `bridge.rs` `forward!` `:78-116`.
- Workspace `Cargo.toml`: `members = ["crates/*"]` `:3` (a new crate is a member automatically); `gix = { version = "0.87", default-features = false, features = ["revision", "blob-diff", "sha1", "parallel"] }` `:14`; `tokio` `:108`; `schemars = "1"` `:131`; `notify = "8"` `:139`; `sqlx = { version = "0.9", … features = ["runtime-tokio", "sqlite", …] }` `:148`; `ignore = "0.4.33"`, `grep-regex`, `grep-searcher` `:185-188`. **Absent**: `tree-sitter*`, `rusqlite`, `globset`, `cargo_toml`, `oxc_resolver`.
- Constitution: 00 §1.2 `:17-18`, §1.6 `:29`, §1.8 `:32-40`; §9 storage paragraph `:705-731`; §10 rows `:761` (knowledge), `:762` (search), `:766-767` (firecrawl-native, scrape), `:782` (decision); dependency line `:798`; §12 table header `:976-977`; §13 returned-as table `:1047-1055`. 12 §0 rules `:7-14`; 12 §10 `:218-239`. `docs/plugins.md:25-35` — the nine things a plugin doc must contain. No file under `crates/` or `docs/` mentions tree-sitter, `tags.scm`, "code index" or "call graph" (grep, 2026-09-21). Next free doc number: 60.

### B. Upstream (verified 2026-09-21; URLs in `facts/code-intelligence-landscape-2026-09` and the recon it cites)

- `tree-sitter` 0.27.0 (2026-08-30, MIT, `rust-version 1.90`), `TREE_SITTER_LANGUAGE_VERSION 15`, `MIN_COMPATIBLE 13` (`lib/include/tree_sitter/api.h` @ v0.27.0). `tree-sitter-language` 0.1.8: one `#[repr(transparent)] LanguageFn`; grammar crates depend on it, not on core.
- `tree-sitter-tags` 0.27.0 (2026-08-30, MIT; dep `tree-sitter ^0.27`). `TagsConfiguration::new(language, tags_query, locals_query)`; `TagsContext::generate_tags(&config, source, cancel: Option<&AtomicUsize>) -> (impl Iterator<Item = Result<Tag>>, has_errors: bool)`; `Tag { range, name_range, line_range, span: Range<Point>, docs, is_definition, syntax_type_id }`; `syntax_type_name(id)` returns the suffix of `@definition.X` / `@reference.X`. Locals filtering applies only to patterns carrying `(#not-is? local)` — none of the shipped queries do, so v1 gets local filtering only where we add that predicate (§5.3).
- Grammar crates (all MIT; runtime dep `tree-sitter-language ^0.1`; ABI in brackets): `tree-sitter-rust` 0.24.2 [15] — `TAGS_QUERY` yes, captures `definition.{class,method,function,interface,module,macro}`, `reference.{call,implementation}`; `tree-sitter-typescript` 0.23.2 [14] — TS-only supplement (`definition.{function,method,class,module,interface}`, `reference.{type,class}`), must be concatenated with `tree-sitter-javascript` 0.25.0 [15] `TAGS_QUERY` (`definition.{method,class,function,constant}`, `reference.{call,class}`, `@doc`) as the grammar's own `tree-sitter.json` does; `tree-sitter-python` 0.25.0 [15] — `definition.{constant,class,function}`, `reference.call`; `tree-sitter-go` 0.25.0 [15] — `definition.{function,method,type}`, `reference.{call,type}`, `@doc`. Also available for one line each: java 0.23.5 [14], c 0.24.2 [15], cpp 0.23.4 [14]. `tree-sitter-yaml` 0.7.2, `tree-sitter-toml-ng` 0.7.0, `tree-sitter-json` 0.24.8 ship no tags query. Unverified: that JS 0.25's query compiles against TS 0.23.2 unchanged — step 1 tests it and vendors a copy if not.
- `cargo_toml` 1.0.1 (2026-09-01, MIT/Apache): `Manifest { package, workspace: Option<Workspace { members: Vec<String>, exclude, … }>, dependencies: DepsSet, lib: Option<Product>, … }`; members are raw strings — **no glob expansion** — hence `globset` 0.4.20 (ripgrep's, Unlicense/MIT). `cargo_metadata` needs `cargo` on PATH, which this harness's sandbox lacks on Sky's Mac (`facts/namgq1y4930l0-cargo-path`).
- `oxc_resolver` 11.24.3 (2026-08-24, MIT): enhanced-resolve port; `tsconfig` `extends`/`paths`/`baseUrl`/`references`, `package.json` `exports` (set `condition_names`); no other oxc crate in its dependency list.
- Python: no standalone crate (`ty_module_resolver` drags salsa + ruff_db). Hand-write.
- PageRank: `petgraph` 0.8.3 `algo::page_rank(graph, damping, iters)` is unweighted, unpersonalised, O(n·|V|²·|E|). aider `repomap.py`: edges referencer → definer, `weight = mul · √refs`, `mul` ×10 for mentioned idents, ×10 for long snake/camel names (≥ 8), ×0.1 for `_`-prefixed, ×0.1 when defined in > 5 files, ×50 from chat files; personalisation `100 / n_files` on chat/mentioned files, also used as the dangling vector; rank spread per `(definer, ident)`; binary search on tag count to fit the budget within 15 %.
- `sqlx` 0.9 feature `sqlite` = `sqlite-bundled` → `libsqlite3-sys` 0.37 bundled build compiles `-DSQLITE_ENABLE_FTS5` unconditionally. FTS5 is available; knowledge already relies on it (`store.rs:38-42`).
- `gix` 0.87.1: `Tree::changes()?.for_each_to_obtain_tree(&other, |change| …)` / `Repository::diff_tree_to_tree` for changed paths; `repo.rev_walk([head]).first_parent_only().all()` for the commit walk; set `object_cache_size` for diff performance.
- `notify` 8.2.0 (workspace pin `"8"`); FSEvents is one stream per root, recursive, does not cross mount points. Seam only in v1.
- Token estimate: OpenAI's rule is ≈ 4 chars/token for English; qartez uses `chars / 3` for code (`src/server/helpers.rs:139-145`) and one uniform `budget_render` that cuts at row boundaries and appends `... +N more` (`:165-180`). `tiktoken-rs` exists (8.8 MB of BPE ranks, OpenAI vocabularies only) — wrong yardstick, not taken.

### C. Reference implementation (`~/Develop/qartez-mcp`)

Schema `src/storage/schema.rs:6-135` (`files`, `symbols`, `symbol_refs(from, to, kind)`, `edges(from_file, to_file, 'import', specifier)`, `co_changes`, `type_hierarchy`, FTS5 `symbols_fts(name, kind, file_path; porter unicode61)`); PageRank `damping 0.85, iterations 20`, warm start (`src/graph/pagerank.rs:19-20, 29`); `impact` = direct importers + transitive blast + top-10 co-change, tests filtered by path (`src/server/tools/impact.rs:26, 71-109`); `understand` sections `definition, calls, refs, cochange` with the budget split equally (`src/server/tools/understand.rs:43, 196`); agent workflow `src/server/mcp_instructions.md` ("orient → locate → read → assess → edit → verify"). Rust import resolver `src/index/mod.rs:1508-1560`. None of this is copied; the schema below is written from the requirements and differs where §0 says so.

## 3. Shape

```
crates/codeindex/
  Cargo.toml          tree-sitter, tree-sitter-tags, tree-sitter-{rust,typescript,javascript,python,go},
                      cargo_toml, globset, oxc_resolver, sqlx (workspace), gix (workspace), ignore (workspace),
                      tokio, kameo, schemars, serde, serde_json, anyhow, tracing, sha2
  src/lib.rs          CodeindexPlugin, CodeindexConfig, CodeindexActor (Handle, Reconcile, Stats), CODEINDEX_ACTOR = "codeindex"
  src/store.rs        open_db, DDL (§4), typed queries; one SqlitePool per open root, LRU of 8
  src/root.rs         root discovery (gix::discover | cwd), slug+hash file name, per-root lock
  src/walk.rs         ignore::WalkBuilder → Vec<(path, mtime_ns, size, Language)>; skip > 1 MiB, binary, non-UTF-8
  src/lang/mod.rs     Language enum, extension map, TagsConfiguration per language (built once, OnceLock)
  src/lang/imports.rs one tree-sitter Query per language for import statements (§5.2)
  src/parse.rs        parse_file → ParsedFile { symbols, refs, imports, has_errors }; spawn_blocking pool
  src/resolve/mod.rs  ImportResolver trait; resolve_refs (§6)
  src/resolve/rust.rs Cargo workspace model (cargo_toml + globset), mod tree, `use` paths
  src/resolve/ts.rs   oxc_resolver wrapper
  src/resolve/py.rs   relative + root-based module resolution
  src/resolve/go.rs   go.mod module path + directory = package
  src/rank.rs         weighted personalised PageRank (sparse), map rendering to a budget (§7)
  src/git.rs          gix: changed files for a range; co-change walk
  src/tools.rs        six Tool impls, PROMPT, budget_render, structured()
```

Dependency direction: `runtime ← tools ← codeindex` (codeindex depends on `tools` for the trait and on `runtime` for `Plugin`; nothing depends on codeindex). No new bus event in v1; the actor exists so RPC and later consumers can `ask` it (`Stats`, `Reconcile`) without touching the tools.

Data flow for one call:

```
code_refs(symbol="RegisterTool")
  → root = args.root | gix::discover(cwd).work_dir | cwd
  → store = open(<data_dir>/codeindex/<slug>-<hash8>.sqlite)      (DDL if new)
  → reconcile(root): walk → diff (mtime_ns,size) → changed ⊆ files
       ≤ 64 changed: parse + resolve now (spawn_blocking), commit, stale=false
       > 64 or first index: if index empty → index everything now (first call pays);
                           else answer now, stale=true, finish in background
  → query symbols/symbol_refs → rows sorted by priority → budget_render → ToolOutput { content, structured }
```

## 4. Data on disk

`<data_dir>/codeindex/` holds one `<slug>-<hash8>.sqlite` (plus `-wal`, `-shm`) per root ever indexed, and nothing else. Derived and deletable: `rm ~/.theuth/codeindex/*.sqlite` costs one re-index per root on next use. Schema changes are a new `SCHEMA_VERSION` in `meta`; on mismatch the file is removed and rebuilt (10 §11's corrupt-index rule, `store.rs:429-431`, applied to version too). No migrations.

```sql
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  -- schema_version, root (canonical path), created_at, last_reconcile_at, git_head, cochange_head

CREATE TABLE IF NOT EXISTS files (
  id        INTEGER PRIMARY KEY,
  path      TEXT NOT NULL UNIQUE,          -- root-relative, forward slashes
  mtime_ns  INTEGER NOT NULL,
  size      INTEGER NOT NULL,
  language  TEXT NOT NULL,                 -- rust|typescript|tsx|javascript|python|go|other
  package   TEXT,                          -- Cargo package / npm package / Python top package / Go package path
  lines     INTEGER NOT NULL,
  parse_errors INTEGER NOT NULL DEFAULT 0, -- tree-sitter `has_errors`
  is_test   INTEGER NOT NULL DEFAULT 0,    -- path heuristic: tests/, *_test.go, test_*.py, *.test.ts, #[cfg(test)] file-level
  rank      REAL NOT NULL DEFAULT 0.0      -- last unpersonalised PageRank (for `code_map` without focus)
);

CREATE TABLE IF NOT EXISTS symbols (
  id        INTEGER PRIMARY KEY,
  file_id   INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name      TEXT NOT NULL,
  kind      TEXT NOT NULL,                 -- tags syntax type: function|method|class|interface|module|macro|type|constant
  parent_id INTEGER REFERENCES symbols(id) ON DELETE SET NULL,   -- enclosing definition by byte range
  line_start INTEGER NOT NULL, line_end INTEGER NOT NULL,
  byte_start INTEGER NOT NULL, byte_end   INTEGER NOT NULL,
  signature TEXT NOT NULL,                 -- the `line_range` text of the tag, trimmed, ≤ 200 chars
  docs      TEXT,                          -- Tag.docs when the query has @doc
  exported  INTEGER NOT NULL DEFAULT 0,    -- rust `pub`, ts `export`, go Uppercase, py not `_`-prefixed
  rank      REAL NOT NULL DEFAULT 0.0
);
CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);

CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
  name, signature, docs, path UNINDEXED, symbol_id UNINDEXED,
  tokenize = 'porter unicode61 remove_diacritics 2'        -- same tokenizer as knowledge (store.rs:42)
);

CREATE TABLE IF NOT EXISTS imports (
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  specifier  TEXT NOT NULL,                -- as written: `tools::registry::ToolRegistry`, `./foo`, `pkg.mod`
  target_file INTEGER REFERENCES files(id) ON DELETE SET NULL,  -- NULL = external or unresolved
  names      TEXT,                         -- JSON array of imported names, or null for glob/whole-module
  line       INTEGER NOT NULL,
  provenance TEXT NOT NULL                 -- cargo|mod|tsconfig|package|relative|gomod|unresolved|external
);
CREATE INDEX IF NOT EXISTS imports_target ON imports(target_file);

CREATE TABLE IF NOT EXISTS refs (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  from_symbol INTEGER REFERENCES symbols(id) ON DELETE SET NULL, -- enclosing definition, NULL at module scope
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,                -- call|type|class|implementation
  line       INTEGER NOT NULL,
  to_symbol  INTEGER REFERENCES symbols(id) ON DELETE SET NULL,  -- NULL when unresolved/ambiguous
  candidates INTEGER NOT NULL DEFAULT 0,   -- how many definitions matched before the rule picked (0 = none)
  provenance TEXT NOT NULL,                -- same_file|import|package|unique|ambiguous|none|scip(reserved)
  confidence REAL NOT NULL                 -- 1.0 same_file/import, 0.8 package, 0.6 unique, 0.0 otherwise
);
CREATE INDEX IF NOT EXISTS refs_to ON refs(to_symbol);
CREATE INDEX IF NOT EXISTS refs_name ON refs(name);

CREATE TABLE IF NOT EXISTS cochanges (
  file_a INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  file_b INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  count  INTEGER NOT NULL,
  PRIMARY KEY (file_a, file_b)
);
```

Differences from the reference, on purpose: `refs.provenance`/`confidence`/`candidates` (decision 5) instead of an implicit link; `imports` keeps unresolved rows with a reason instead of dropping them (so `code_impact` can say "3 imports unresolved in this file"); no `type_hierarchy`, `unused_exports`, `file_clusters`, `shape_hash`, embeddings (not in v1); `files.package` so "same package" is a join, not a path prefix guess.

## 5. Indexing

### 5.1 Walk

`ignore::WalkBuilder::new(root)` with defaults (`.gitignore`, `.ignore`, hidden skipped) plus `.codeindexignore` if present — the same crate and settings as `grep` (`fs.rs:246-249`), so what the model can grep is what gets indexed. Skip: files > 1 MiB, files whose first 8 KiB contain NUL, non-UTF-8 after a lossy check, and paths matching the generated-file heuristics (`*.min.js`, `*.pb.go`, `*_generated.*`, `node_modules/`, `target/`, `vendor/` when a `Cargo.toml`/`go.mod` claims it). Language from extension: `.rs`, `.ts`, `.tsx`, `.js`/`.mjs`/`.cjs`/`.jsx`, `.py`, `.go`; everything else is `other` (a `files` row, no parse).

### 5.2 Parse

Per file, in `tokio::task::spawn_blocking` over a pool of `available_parallelism()` workers, each owning one `TagsContext` (it holds the `Parser`): `generate_tags(&cfg, &bytes, Some(&cancel))` → for each `Tag`: `is_definition` → `symbols` row (`kind = syntax_type_name`, `signature = source[line_range].trim()`, `parent_id` = innermost definition whose byte range contains this one — computed after the file's tags are collected); else → `refs` row (`kind`, `from_symbol` = innermost enclosing definition). `has_errors` → `files.parse_errors = 1`; tags from an erroring tree are kept (tree-sitter recovers; a half-indexed file beats none — codegraph's practice).

Imports are the one per-language addition. One `tree_sitter::Query` each, executed with a `QueryCursor` on the same tree:

| Language | Query nodes | Yields `specifier`, `names` |
|---|---|---|
| Rust | `use_declaration` (argument: `scoped_identifier`, `use_list`, `use_as_clause`, `use_wildcard`), `mod_item` without body, `extern_crate_declaration` | `tools::registry::ToolRegistry` → specifier `tools::registry`, names `["ToolRegistry"]`; `mod foo;` → specifier `mod:foo` |
| TS/TSX/JS | `import_statement` (`source`, `import_clause`), `export_statement` with `source`, `call_expression` where function is `require`/`import` with a string argument | `./registry` names `["ToolRegistry"]` or null for `* as` |
| Python | `import_statement`, `import_from_statement` (`module_name`, relative `import_prefix`), `aliased_import` | `..store` names `["open_db"]` |
| Go | `import_spec` (`path`, optional `name`) | `"github.com/x/y/pkg"` |

Each query is ~10 lines of S-expression plus ~30 lines of Rust to read captures. That is the entire per-language code in the crate; a language with no query still gets symbols and same-file references from its tags.

### 5.3 Locals

Tags queries emit `@reference.call` for every call including local closures and parameters. In v1 we append `(#not-is? local)` to the `reference.call` patterns of Rust, JS/TS and Python at build time (string edit of the shipped query before `TagsConfiguration::new`) and supply the grammar's `LOCALS_QUERY` where one ships (JS, TS); for Rust and Python, which ship none, a 6-line locals query marking `parameters` and `let_declaration`/`assignment` patterns as `@local.definition` inside `function_item`/`function_definition` scopes. Effect: a call to a parameter named `f` is not a reference to every `f` in the repo. Check in step 1: the count of `refs` rows for theuth drops without any `definition` disappearing.

### 5.4 Reconcile and write

One `BEGIN … COMMIT` per reconcile: delete rows for removed/changed files (cascades), insert new `files`/`symbols`/`refs`/`imports`, then §6 resolution for the changed files **and** for every file whose `imports.target_file` or `refs.to_symbol` pointed into a changed file (the affected set; qartez snapshots and restores instead — we recompute, it is cheap at this scale and always right), then `symbols_fts` delete+insert for changed files (FTS5 has no UPSERT; `store.rs:933`), then §7 rank if anything changed. `busy_timeout 5s` and a per-root `tokio::sync::Mutex` serialise writers within the process; two theuth processes on the same root are serialised by SQLite's WAL locking and both get correct, possibly repeated, work — no `.lock` file (qartez's stale `index.lock.pid` on this tree is the argument).

## 6. Resolution

### 6.1 Imports → files

- **Rust.** On first index of a root, build a `CargoWorkspace`: `cargo_toml::Manifest::from_path(root/Cargo.toml)`; if `[workspace]`, expand `members` with `globset` minus `exclude`, load each member's manifest; record `package.name`, `[lib] name`/`path` (default `src/lib.rs`), `[[bin]]` paths, `[dependencies]` with `path =` and `package =` renames. Module tree per package: walk from the lib/bin root file following `mod x;` → `x.rs` | `x/mod.rs` (and `#[path = "…"]` when present as a plain string), so every `.rs` file gets a module path (`tools::registry`) — this is `files.package` + a `modules` map held in memory during resolution (not stored; derivable). A `use a::b::C`: head `crate` → this package's root; `super`/`self` → relative to this file's module; otherwise a package whose lib name (`-`→`_`) or a renamed dependency matches `a` → that package's root, then `b` down the module tree → `target_file`, `names = ["C"]`, `provenance = cargo` (or `mod` for the module-tree part). `std`/`core`/`alloc` and unknown heads → `external`. `mod x;` rows link the parent to the child file. Known miss: `pub use` re-exports one level deep are followed once (`imports` of the target with `names` containing the symbol); deeper chains resolve as `external` and are counted.
- **TS/TSX/JS.** `oxc_resolver::Resolver` with `ResolveOptions { tsconfig: root/tsconfig.json if present, condition_names: ["types","import","require","node","default"], extensions: [".ts",".tsx",".js",".mjs",".cjs",".jsx",".d.ts"], main_fields: ["types","module","main"] }`; `resolve(dir_of_file, specifier)`. Result inside root → `target_file`, `provenance = tsconfig|package|relative` by what matched; inside `node_modules` → `external`.
- **Python.** Roots: the repo root, `src/` if it exists, any directory named in `pyproject.toml` `[tool.setuptools.packages.find] where` / `[tool.poetry] packages`. `from ..a import b`: strip dots against the file's package directory; `import a.b` / `from a.b import c`: try `<root>/a/b.py`, `<root>/a/b/__init__.py` per root in order. Unmatched → `external`.
- **Go.** `go.mod` `module` line; an import `M/x/y` with prefix `M` → directory `x/y` → all `.go` files in it (one `imports` row per file, names null; Go imports are package-level). Others → `external`.

### 6.2 Names → symbols

For each `refs` row with name `n` in file `F`, candidates = `symbols` with `name = n` and a kind compatible with the reference kind (`call` → function|method|macro|class (constructors); `type`/`class`/`implementation` → class|interface|type|module). Rules, first match wins, each records `provenance`, `confidence`, `candidates = |set before the rule|`:

1. **same_file** (1.0): a candidate in `F`; nearest enclosing scope by byte range wins when several.
2. **import** (1.0): candidates in files that `F` imports where `names` is null or contains `n`; for Rust a qualified reference `a::b::n` is first mapped through the import that brought `a` in, then the module tree.
3. **package** (0.8): exactly one candidate in `files.package = F.package`.
4. **unique** (0.6): exactly one candidate in the whole root.
5. **ambiguous** (0.0): `to_symbol = NULL`, `candidates = k`.
6. **none** (0.0): no definition anywhere (external or unindexed language).

Method calls on a receiver (`x.foo()`, `self.foo()`) get no special receiver-type logic in v1 (that is type inference); they go through the same rules and will often end `ambiguous` with the honest count. `code_refs` reports `ambiguous` separately so a caller can decide to ask a child to confirm, and `code_symbol` shows `confidence` next to each caller.

## 7. Ranking and the map

Graph: nodes = files; for each identifier `n` defined in file `D` and referenced from file `R ≠ D` (resolved `refs` only — provenance ≠ ambiguous/none), one edge `R → D` with `weight = mul · √count`. `mul` per aider: ×10 if `n ∈ focus` (the tool's `focus` identifiers), ×10 if `n` is snake/camel-case with length ≥ 8, ×0.1 if `n` starts with `_`, ×0.1 if `n` is defined in > 5 files, ×50 if `R ∈ focus_files`. Definitions with no incoming reference get a self-edge of 0.1 so isolated files keep a rank. Personalisation: `100 / |files|` on `focus_files` (and on files named in `focus` when they are paths), used also as the dangling distribution; damping 0.85, ≤ 50 iterations, stop at L1 change < 1e-6 (warm-started from `files.rank` when unpersonalised). Rank is then spread from each `R` over its out-edges by weight and accumulated per `(D, n)`; the ranked `(file, symbol)` list is the map.

Rendering to a budget: tags rendered per file as `path` then indented `signature` lines (aider's tree, one line each); binary-search the number of ranked tags until `estimate_tokens ≤ budget` (`chars / 3`), accept within 15 %. `structured.rows` carries the same list with `rank`, `path`, `line`, `kind`, `signature`. Unpersonalised ranks are stored back to `files.rank`/`symbols.rank` after each reconcile so `code_map` without `focus` is a lookup.

## 8. Tools

All six: `Risk::ReadOnly`, `DIRECT = false` (compile-time asserted as in `scrape/src/tools.rs:359-363`), `CONSUMES_SECRETS = false` (nothing here takes text a secret could hide in — arguments are names, paths and budgets), errors are content (`ToolOutput::err`) shaped as advice: an unindexed root returns `{ "indexed": false, "files": 0, "hint": "first call indexes; call again" }` only when the first index exceeded the synchronous limit; otherwise the first call simply indexes and answers. Common args:

```rust
/// Repository root; default: the git top-level of the process cwd, else the cwd.
pub root: Option<PathBuf>,
/// Output budget in estimated tokens (chars/3); rows beyond it are dropped and counted. Default 4000.
pub budget: Option<u32>,
```

Common structured envelope: `{ root, indexed_files, stale: bool, pending: n, dropped: n, … }`. `content` is a compact text render of the same rows (what a non-runtime caller — MCP over `rpc`, a future direct tool — would see); the Python value is the envelope (`repl.rs:1283-1289`).

| Tool | Args beyond common | `RETURNS` | Rows / fields |
|---|---|---|---|
| `code_map` | `focus: Option<Vec<String>>` (identifiers or paths to personalise toward), `paths_only: bool` | `dict` | `rows: [{path, line, kind, name, signature, rank}]`, grouped by file in `content`; `paths_only` → ranked `[{path, rank, lines, language}]` |
| `code_find` | `name: String`, `kind: Option<Kind>`, `exact: bool` (default false → FTS5 prefix on `name`, `signature`, `docs`) | `list[dict]` | `[{symbol_id, name, kind, path, line_start, line_end, signature, exported, rank}]` ordered by exact-name first, then rank |
| `code_outline` | `path: PathBuf` | `dict` | `{path, language, lines, parse_errors, imports: [{specifier, target, provenance}], symbols: [{name, kind, line_start, line_end, signature, children}]}` nested by `parent_id` |
| `code_refs` | `symbol: String` (name or `path:line` or `symbol_id`), `direction: in|out` (default `in`), `kind: Option<call|type|…>` | `dict` | `{definition, refs: [{path, line, from_symbol, kind, provenance, confidence}], ambiguous: n, by_file: [{path, count}]}` |
| `code_symbol` | `symbol: String`, `sections: Option<Vec<definition|body|callers|callees|cochange>>` | `dict` | `{definition, body?: str (≤ 200 lines, else head/tail with a cut marker), callers, callees, cochange: [{path, count}]}`; budget split equally over requested sections (qartez `understand.rs:196`) |
| `code_impact` | `files: Option<Vec<PathBuf>>` or `git_range: Option<String>` (`main..HEAD`, `HEAD~3`), `include_tests: bool` | `dict` | `{changed: [{path, symbols_changed: [name]}], direct: [{path, via: [name]}], transitive: [path], tests: [path], unresolved_imports: n}` — direct = files whose resolved refs/imports point into the changed symbols/files; transitive = importers of those, depth ≤ 3; tests = files with `is_test` among direct ∪ transitive |

`code_impact` with `git_range` reads the diff with `gix` (changed paths and hunks) and maps hunk line ranges to `symbols` by span — so "what did the child change" is a symbol list, not a file list. Ambiguous refs are never counted as impact; the envelope reports how many were skipped (`ambiguous_skipped`).

Prompt paragraph (registered once, plugin `codeindex`):

> `code_map`, `code_find`, `code_outline`, `code_refs`, `code_symbol` and `code_impact` are runtime functions over a tree-sitter index of a source tree (default root: the git repo of the cwd; first call indexes). Use them before `grep`/`read_file` when the question is structural: where something is defined (`code_find`), what a file contains (`code_outline`), who references a symbol (`code_refs`; `provenance`/`confidence` say how sure), a ranked map to brief a subagent (`code_map(focus=[…], budget=1500)`), what a diff touches (`code_impact(git_range="main..HEAD")`). Results are values; put `m["content"]` into a brief instead of re-reading files.

Skill `codeindex.md` (optional, in `~/.theuth/skills/`): the brief-then-verify recipe, how to read `ambiguous`, when `stale` matters.

## 9. Config

```yaml
# codeindex:                 # design doc 60; indexes live in <data_dir>/codeindex/<slug>-<hash>.sqlite
#   enabled: true            # false: no plugin, no code_* functions, nothing on disk
#   sync_limit: 64           # changed files re-indexed synchronously before answering; more → answer stale, finish in background
#   max_file_bytes: 1048576  # larger files are listed, not parsed
#   cochange_commits: 300    # first-parent commits walked for co-change; 0 disables the git walk
#   languages: [rust, typescript, javascript, python, go]   # subset to parse; others are files only
```

`CodeindexConfig` with `#[serde(default)]` and a `defaults_match_design` test as in `scrape/src/lib.rs:108-120`. `enabled: false` leaves any existing `codeindex/*.sqlite` in place (derived; harmless; `rm` at will). Env overlay `THEUTH_CODEINDEX__SYNC_LIMIT=…` for free (00 §9).

## 10. Commands, events, RPC, UI

Actor `CodeindexActor` registered as `CODEINDEX_ACTOR = "codeindex"`. Messages (`ask`): `Handle → Arc<Codeindex>` (the same pattern knowledge uses, `knowledge/src/lib.rs:288-294`), `Stats { root: Option<PathBuf> } → IndexStats { root, files, symbols, refs, resolved, ambiguous, last_reconcile, stale }`, `Reconcile { root } → IndexStats` (force). No bus events in v1 (nothing subscribes; add `CodeindexChanged` when something does — a `forward!` line in `rpc/src/bridge.rs:88-116`). RPC: one method, `codeindex_stats(root?)`, following `api.rs:1285-1290`, so the desktop can show "index: 290 files, 11k symbols, 2 min ago" on a project card; no other UI in v1 (no slash command, no TUI panel).

## 11. Threats and limits

| Threat / limit | Handling |
|---|---|
| Source text is untrusted content (28): a comment saying "ignore previous instructions" is indexed into `docs`/`signature` and returned | Same status as `read_file`/`grep` output today; when 28 lands, `code_*` tools carry the same `UNTRUSTED` label as `read_file`. No tool here is a sink. |
| Secrets in source (`.env` committed, tokens in tests) appear in `signature`/`body` | `code_symbol.body` is the only tool returning more than one line of source; it obeys `ignore` rules (which exclude `.env` in most repos) and the redaction path `read_file` uses — none today; tracked in 37, not here. |
| Path escape: `root` or `path` pointing outside the intended tree, symlinks out of the root | `root` is canonicalised; `path` args are joined to `root` and must canonicalise under it; `ignore` does not follow symlinks by default and we keep that. |
| Huge roots (monorepos, `~`) | Walk stops at `max_files` (50 000, hard-coded v1) with `structured.truncated_walk = true`; first index over `sync_limit` runs in the background with progress in `Stats`. |
| Two processes writing one index | WAL + `busy_timeout`; work is idempotent per file (delete then insert); no lock file. |
| Wrong answers with a straight face | `provenance` and `confidence` on every edge; ambiguous is a count, not a link; `parse_errors` per file; `stale` per response. |
| Grammar/query skew (JS 0.25 query on TS 0.23 grammar) | `TagsConfiguration::new` fails at startup → language disabled with a `tracing::warn!`, plugin continues (`plugins.md:167-173`); step 1 test pins the working pair. |
| Binary size / build time | Four grammar C files (+ TS's two) compiled once; measure in step 1 and record. |

## 12. What we knowingly give up

Type-aware resolution (traits, generics, dynamic dispatch, receiver types) — SCIP seam; Java/C/C++ until someone needs them (one grammar line each, no query work); a watcher (reconcile is per call); refactor/edit tools; security/smell/clone/health scanners and architecture "wiki" generation (qartez has them; §1.8 keeps them out); embeddings; human search integration (49 closed); Windows path canonicalisation quirks until 58's work lands (v1 developed and checked on macOS; Linux expected to work unchanged).

## 13. Alternatives considered

*qartez-mcp as an MCP server* (`mcp/qartez.json`) — works today, licence fine for an individual, and it is what Sky has used. Rejected as the native answer: cannot be embedded (licence), cross-crate Rust imports unresolved (`src/index/mod.rs:1508-1560`), one author and no commits since July, writes `.qartez/` into the repo, 43 tools where six do. Kept as the reference implementation and as an option for anyone who prefers it.
*codegraph as an MCP server* — largest community, MIT, good explore ergonomics; rejected as native: Node daemon per project, intelligence in TypeScript, kernel is unpublished napi glue, telemetry default-on.
*Vendor codanna (Apache-2.0) under 38's rules* — the only Rust library candidate; rejected: `fastembed` (→ `ort`, removed from this tree on 2026-09-21 — `facts/theuth-decision-engine-candle-as-built`), `rmcp`, `axum`, `tokio full` are unconditional dependencies; tantivy would be a second search engine beside FTS5 (10 §3.4 rejected a second ranker); hand-written parsers; and its Rust resolver has no workspace awareness (`src/parsing/rust/behavior.rs:371` "TODO: … based on Cargo.toml"). Its `--json` output shape and "resolution-verified edges" idea informed §6/§8.
*Serena / LSP bridges* — precise, GPL-3 (Serena), per-language servers, a process per project; §1.8 names LSP. No.
*stack-graphs* — the principled resolver; archived 2025-09-09, four languages. No.
*Embeddings* — answer "find the retry logic", not "who calls Y"; 10 §3 already said no; the LLM handles vocabulary.
*Putting the index in the repo (`.codeindex/`)* — pollutes the user's tree; 00 §9 keeps state under `<data_dir>`.
*A single shared database* — see decision 7.

## 14. Licensing

Every dependency is MIT and/or Apache-2.0 (tree-sitter and grammars MIT; `cargo_toml` MIT/Apache; `oxc_resolver` MIT; `globset` Unlicense/MIT; `sqlx`, `gix` MIT/Apache). No code is copied from qartez-mcp (dual licence) or codegraph (MIT — not needed). This crate is `license = "MIT OR Apache-2.0"` like the rest of the workspace; no `NOTICE`.

## 15. Amendments to 00 (same commit as step 1)

- §1.8, replace the sentence "We do not ship coding-specific natives (LSP, notebooks, git, diff review): those are a use of the harness, not a product line (12 §10)." with: "We do not ship coding-specific natives (LSP, notebooks, git clients, diff review, refactoring): those are a use of the harness, not a product line (12 §10). Searching, editing and *understanding* files — `grep`, `edit`, and the `codeindex` symbol/reference index (60) — are assistant work, not coding-agent work; the line is drawn at tools that need a language toolchain or perform edits by symbol." Alternative beaten: qartez/codegraph over MCP (60 §13).
- §9, after the `artifacts.sqlite` clause: "`codeindex/<slug>-<hash>.sqlite` (derived tree-sitter symbol indexes, one per repository root; delete to rebuild — 60 §4),".
- §10, new row after `scrape/`: `codeindex/          plugin: tree-sitter tags → sqlite symbol/reference index per repo root, workspace-aware import resolution, aider-style ranked map, code_* runtime tools (60)`; dependency line `:798` gains `tools ← codeindex`.
- §12, new phase row: `| 33 | codeindex (60) | on this repo: code_find("ToolRegistry") → registry.rs; code_refs("RegisterTool") lists knowledge/scrape/… call sites with provenance=import; code_map(budget=1500) ≤ 1725 est. tokens; code_impact("HEAD~1") names the symbols the last commit touched | — |`.
- §13: no row (nothing here existed in jarvis).
- README crate list: one line, same text as the §10 row.

## 16. Build steps (one commit each, `cargo check` green, each ends in a runnable check)

| Step | Delivers | Done when |
|---|---|---|
| 1. Crate, store, parse, `code_find`, `code_outline` | `crates/codeindex` with §3 layout minus resolve/rank/git; DDL §4; walk + tags parse for the four languages (TS = JS + TS queries; fallback vendored copy if the pair fails); reconcile-on-call with `sync_limit`; plugin wired at `main.rs` after `scrape`; `# codeindex:` block in `DEFAULT_CONFIG`; 00 amendments | On this repo from a clean `~/.theuth/codeindex/`: first `code_find(name="ToolRegistry")` indexes ≥ 290 `.rs` files (log the wall time and the binary size delta) and returns `crates/tools/src/registry.rs` with `kind=class`; `code_outline("crates/tools/src/fs.rs")` lists `ReadFile`, `Grep`, `grep`, `MAX_BYTES`… with nested methods; touching one file and calling again re-parses exactly one file; `refs` count for the repo is lower with §5.3 locals than without, `symbols` count identical. `cargo test -p codeindex` has fixture trees for each language. |
| 2. Imports and resolution, `code_refs` | §5.2 import queries; §6.1 resolvers (Cargo workspace + mod tree; `oxc_resolver`; Python; Go); §6.2 rules with provenance/confidence; affected-set recompute | On this repo: `imports` rows with `provenance=cargo` > 500 and `unresolved` < 5 % of non-external; `code_refs("RegisterTool")` returns the call sites in `knowledge/src/tools.rs`, `scrape/src/tools.rs` and every other plugin with `provenance=import, confidence=1.0`, and `ambiguous` reports the method-call residue honestly; on a TS fixture with `paths` aliases the alias resolves. |
| 3. Rank and map, `code_symbol` | §7 PageRank + budget renderer; `files.rank`/`symbols.rank` persisted; `code_map` with/without `focus`; `code_symbol` sections | `code_map(budget=1500)` on this repo renders ≤ 1725 estimated tokens and its top files include `crates/tools/src/lib.rs`, `crates/runtime/src/lib.rs`, `crates/agent/src/loop_.rs`; `code_map(focus=["ScrapeUrlTool"])` moves `crates/scrape/*` to the top; `code_symbol("open_db")` returns definition + body + callers in one call under the default budget. |
| 4. Git: `code_impact`, co-change | `gix` diff for `files`/`git_range`; hunk → symbol mapping; transitive importers; tests; co-change walk into `cochanges`; `codeindex_stats` RPC | `code_impact(git_range="HEAD~1")` on the commit that lands step 3 names the symbols step 3 added and the files that import them; `cochanges` on this repo is non-empty (qartez had 0) and `code_symbol(…, sections=["cochange"])` lists partners; the RPC returns stats for the current root. |

Effort: step 1 is the largest (crate skeleton, four grammars, tags plumbing, store); 2 is the one with judgement in it (the Cargo model); 3 and 4 are small. A child harness (omp in a PTY, `procedures/omp-interactive-pty`) can take each step from this doc; the orchestrator verifies the "done when" column against the running binary.

## 17. Open

- Whether `code_impact` should also read the *working tree* diff (uncommitted changes) — likely yes via `gix` status + blob diff; decide in step 4 when the API is in hand.
- Whether the first index of a large root should block the call (simple, honest) or always go background (never a slow tool call). v1: block up to `sync_limit` files or an empty index, else background; revisit with numbers from step 1.
- Rust `pub use` re-export depth (v1: one level). Measure the `unresolved` share on this repo before adding depth.
- Whether the map's `focus` should accept a free-text task string and pick identifiers from it (aider's `mentioned_idents` come from the chat). v1: identifiers and paths only; the model can extract words itself.
- Java/C/C++/Bash: add when a tree here needs them; Bash has no tags query at all.

## 18. Status

Proposed 2026-09-21. Nothing exercised. Reference clone at `~/Develop/qartez-mcp` (`7fdb509`); recon in `facts/qartez-mcp-recon`, `facts/codegraph-colbymchenry-recon`, `facts/code-intelligence-landscape-2026-09`, `facts/theuth-code-intelligence-boundary`, `decisions/theuth-codeindex-native-plugin`.
