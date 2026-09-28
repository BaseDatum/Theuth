# 60 as built — CI-IMPACT: tests that matter, project profile, honest reconcile

Branch `feature/codeindex-impact` off `22b39455`. Worktree `~/Develop/theuth-ci-impact`.

## Commits

1. `64821d2f feat(codeindex): reconcile guard keeps the index when >80% of files vanish`
2. `e4c7e361 feat(codeindex): test → subject linking; code_impact tests_by_subject, code_symbol tests`
3. `19337778 feat(codeindex): project_profile tool — how the project is checked, from manifests`
4. `1be6aa4b fix(codeindex): no test subject for fixture/test-data files or deep fixture trees`
5. `docs(60): as-built note CI-IMPACT` (this note)

## What changed

### Item 1 — test → subject linking

- `crates/codeindex/src/walk.rs:139` `subject_candidates(path)`: the test marker stripped from the name (`foo.test.ts`, `foo.spec.tsx`, `foo_test.go`, `test_foo.py`, `foo_test.py` → `foo.*`), looked for in the same directory, beside the `tests`/`__tests__`/`test`/`testdata` directory (`__tests__/foo.tsx → ../foo.tsx`), under a sibling `src` mirror (`tests/…/foo → src/…/foo`, `crates/x/tests/rank.rs → crates/x/src/rank.rs`), and for `.rs` under a `tests/` dir the crate's `src/lib.rs` then `src/main.rs`. One level of nesting under the test dir (`tests/unit/foo`) still tries the crate root; deeper trees do not.
- `crates/codeindex/src/walk.rs:121` `is_test_data(path)`: `fixtures`, `__fixtures__`, `testdata`, `snapshots`, `__snapshots__`, `golden` components — such files stay `is_test` (kept out of `direct`) but get no subject. Without this, `crates/codeindex/tests/fixtures/rust/crates/a/src/lib.rs` was linked to `crates/codeindex/src/lib.rs`.
- `crates/codeindex/src/index.rs:401` `test_subjects(pool)`: after every write's resolve step, for every test file with a parsed language and not test data: first candidate that is an indexed non-test file, else the non-test file it imports most (`store::test_import_counts`, ties → first path). Stored whole as `meta.test_subjects` JSON `{test_path: subject_path}` (`store.rs:1150-1175` `test_import_counts`, `set_test_subjects`, `test_subjects`). No new table, no schema bump. Recomputed whole each write: 263 test files on this repository, sub-millisecond next to resolve.
- `crates/codeindex/src/tools.rs:1436` `code_impact`: `tests_by_subject: [path]` = tests whose subject is a changed or direct file. `tests` (reference-based) is unchanged. `content` lists each test once under `-- tests: N files --` as `path (ref)`, `path (subject)` or `path (ref, subject)`; the header's `tests N` is the union. Description updated.
- `crates/codeindex/src/tools.rs:1059` `code_symbol`: `tests: [path]` = tests whose subject is the definition's file; `content` gains a `-- tests: N files (subject <path>) --` block when non-empty. Absent on the several-candidates return.

### Item 2 — `project_profile(root)`

- `crates/codeindex/src/profile.rs` (new): `detect(root) -> Profile {root, kind, package_manager, commands: {check, test, lint, format, build, typecheck}, test_runner, ci, manifests, scripts}` and `render`. Order of precedence per slot: justfile recipe (`just <name>`; aliases `check`, `test`, `lint|clippy`, `fmt|format`, `build`, `typecheck|type-check|tsc`; `_private` recipes skipped), then Makefile target (`make <name>`), then the manifest default. Cargo: `[workspace]` → `--workspace` on check/test/clippy/build. Node: lockfile → `bun|pnpm|yarn|npm` (default npm), `package.json` scripts by alias → `<pm> run <name>`, `tsconfig.json` without a typecheck script → `bunx|pnpm exec|yarn|npx tsc --noEmit`, `test_runner` from the test script text (`vitest|jest|mocha|ava|node --test|bun test`) else devDependencies else `bun test` for bun. Python: `pyproject.toml|setup.py|setup.cfg|requirements.txt`, lockfile → `uv|poetry|pipenv`, else `[tool.poetry]`/`[tool.uv]`/pip; `pytest` when mentioned or a `tests/` dir, `conftest.py` or `pytest.ini` exists; `ruff check|format`, else `flake8`/`black`; `mypy` else `pyright`. Go: `go vet|test|build ./...`, `gofmt -l .`, `golangci-lint run` when a `.golangci.*` exists. `kind` = the one detected language, `mixed` for several, `unknown` for none. `ci` = `.github/workflows/*.yml|yaml` sorted, `.gitlab-ci.yml`, `.circleci/config.yml`, `Jenkinsfile`. Nothing is executed; root-level manifests only.
- `crates/codeindex/src/tools.rs:1578` `ProjectProfileTool`: `NAME = "project_profile"`, `RETURNS = "dict"`, `Risk::ReadOnly`, `DIRECT = false`, args `{root?}`; `root::discover` like the other tools, `detect` on a blocking thread, output = the profile plus `content`. No index is opened, so no `indexed_files/stale/pending` envelope — there is nothing those fields would describe. Registered after `code_impact` (`tools.rs:69`); `lib.rs:30` `pub mod profile`; const `DIRECT` assert extended.
- Prompt paragraph (`tools.rs:24-38`): one clause — `project_profile()` reads the manifests, lockfiles, justfile/Makefile and CI files and returns how the project is checked; brief a child with those commands, do not guess them.
- Row for doc 60 §8's table:

  | Tool | Args beyond common | `RETURNS` | Rows / fields |
  |---|---|---|---|
  | `project_profile` | `root: Option<PathBuf>` only (no budget: the render is ≤ 1 KB) | `dict` | `{root, kind: cargo\|node\|python\|go\|mixed\|unknown, package_manager, commands: {check, test, lint, format, build, typecheck}, test_runner, ci: [path], manifests: [path], scripts?: {name: command}, content}` — detection from manifests/lockfiles/justfile/Makefile/CI files; a task-runner recipe of a slot's name wins over the manifest default; nothing runs; no index, so no freshness fields |

### Item 3 — reconcile guard

- `crates/codeindex/src/index.rs:27` `SUSPECT_GONE_PERCENT = 80`, `:451` `Diff::suspect()` = `gone * 100 > known * 80`. In `reconcile` (`index.rs:143`): when the diff is suspect and `force` is false, nothing is deleted or reparsed; `Reconciled {stale: true, suspect_root: true, hint: Some(SUSPECT_ROOT_HINT), indexed_files: <old count>}` is returned; `tracing::warn!` once per open root (`RootIndex.suspect_logged`). The background re-diff (`index.rs:172`) applies the same guard. `force = true` (the actor's `Reconcile` message, an explicit action) bypasses it — the escape hatch when a repository really did shrink.
- `Reconciled` gains `suspect_root: bool`, `hint: Option<&'static str>` (`index.rs:78-80`). `tools.rs` `Envelope` carries both; `code_find`, `code_outline`, `code_refs`, `code_symbol`, `code_impact` emit `suspect_root` and `hint` in their JSON and end `content` with `[stale: <hint>]` via `stale_note` (`tools.rs:109`) instead of `[stale: 0 files still indexing]`.

## Numbers (this repository, debug build)

- `code_impact(files=["crates/codeindex/src/rank.rs"])`: 30 definitions; direct 2, transitive 10; `tests` (ref-based, unchanged before/after): 6 — `crates/codeindex/tests/fixtures/rust/crates/a/src/store.rs`, `…/b/src/cli.rs`, `crates/codeindex/tests/index.rs`, `crates/codeindex/tests/locate.rs`, `crates/document/tests/actor.rs`, `crates/rpc/tests/prompts.rs`; `tests_by_subject`: 1 — `crates/codeindex/tests/index.rs` (rendered `(ref, subject)`). On this repository the subject link adds no test the reference walk missed for `rank.rs`: the crate's suite is one file that names its symbols. The fixture test (`test_subjects_link_api_tests`) shows the case it exists for: a `tests/cli.rs` that only runs the binary appears as `(subject)` and is absent from `tests`. 132 ms for the call, warm.
- Subjects on this repository: 263 `is_test` files, 138 with a subject after the test-data filter (148 before it; the 10 dropped were fixture `Cargo.toml`/`lib.rs` files mapped onto `crates/codeindex/{Cargo.toml,src/lib.rs}`). Sample: `crates/codeindex/tests/index.rs → crates/codeindex/src/index.rs`, `crates/codeindex/tests/locate.rs → crates/codeindex/src/locate.rs`.
- `project_profile()` on this repository, warm: 0 ms (`assert!(ms <= 20)` in the test). Output:

  ```
  cargo project (…/theuth-ci-impact); package manager: cargo; test runner: cargo test
  check: just check
  test: just test
  lint: just clippy
  format: just fmt
  build: just build
  manifests: justfile, Cargo.toml, Cargo.lock
  scripts (45): build, build-remote, bump-version, check, clippy, desktop, desktop-check, …, test, testflight, toolchain, ts, ts-desktop, web-build
  ```

  No `ci` (this repository has no `.github/workflows`), no `typecheck` (compiled language; `check` covers it).
- `project_profile("/tmp/empryo-recon")`, warm: 1 ms. Output:

  ```
  node project (/private/tmp/empryo-recon); package manager: bun; test runner: bun test
  test: bun run test
  lint: bun run lint
  format: bun run format
  build: bun run build
  typecheck: bun run typecheck
  manifests: package.json, bun.lock, tsconfig.json
  ci: .github/workflows/ci.yml, .github/workflows/locales.yml
  scripts (29): build, build:binary, …, lint, lint:fix, test, test:coverage, typecheck
  ```

## Tests written

- `crates/codeindex/tests/index.rs:809` `reconcile_refuses_to_empty_the_index`: 10 files indexed; 2 removed → normal delete (8 left); 7 of 8 removed → `stale`, `suspect_root`, `hint`, `removed = 0`, `files` still 8, refused again on the next call; `code_find` envelope carries `suspect_root: true`, `hint`, and `content` ends with `[stale: most indexed …]`; `force` → 7 removed, 1 left.
- `crates/codeindex/tests/index.rs:900` `test_subjects_link_api_tests`: Rust crate + TS pair fixture; exact subject map (`tests/api.rs → src/lib.rs`, `tests/cli.rs → src/lib.rs`, `tests/util.rs → src/util.rs`, `web/store.test.ts → web/store.ts`, `web/__tests__/misc.test.ts → web/store.ts` by import fallback); `code_impact(files=[src/lib.rs])`: `tests` has `api.rs` and not `cli.rs`, `tests_by_subject = [api.rs, cli.rs]`, content marks `api.rs (ref, subject)` and `cli.rs (subject)`, each once; `code_symbol("run").tests = [api.rs, cli.rs]` with the render block.
- `crates/codeindex/tests/index.rs:1043` `project_profile_on_this_repository`: kind/pm/slots on this repository, `desktop-check` in scripts, warm ≤ 20 ms; on `/tmp/empryo-recon` when present: node/bun, `bun run test|typecheck`, `bun test`.
- `crates/codeindex/src/walk.rs` `subject_candidates_by_convention`: every convention in the brief plus the depth rule and `is_test_data`.
- `crates/codeindex/src/profile.rs` unit tests: `just_recipes`/`make_targets` parsing (settings, assignments, attributes, `.PHONY`, pattern rules, private recipes skipped); cargo workspace + justfile precedence + GitHub CI; bun from lockfile and scripts; pnpm + vitest + `tsc` fallback mixed with Go/golangci; python uv + ruff + mypy; empty root → `unknown`.

Gate: `cargo test -p codeindex` 30 (lib) + 20 (tests/index.rs) + 1 (tests/locate.rs) passing in the full run before the final commit; `cargo clippy -p codeindex --tests -- -D warnings` clean; `cargo fmt -p codeindex --check` clean (the base was not fmt-clean; only my hunks were formatted); `cargo check --workspace` once, clean apart from the pre-existing `block v0.1.6` future-incompat note. In the last two runs the pre-existing `map_and_symbol` timing assertion (`rank::persist` cold < 500 ms, debug build) failed at 599–772 ms with the host at load average 52 from the sibling workers' builds; this branch does not touch `rank.rs` and the same test passed at 20/20 earlier in the session. Environmental, not a regression — re-run on a quiet host.

## Deviations

- `code_map`'s JSON envelope does not emit `suspect_root`/`hint` and its `content` still ends with `[stale: 0 files still indexing]` when the guard fires: `code_map` is CI-RENDER/CI-RANK's section of `tools.rs` this wave and the brief says not to touch it. `stale: true` is set there (it comes from `Reconciled`). One-line follow-up for whoever owns it: spread `e.suspect_root, e.hint` and call `stale_note(&r)`.
- `Envelope`/`Reconciled` field additions are public-struct changes in `index.rs` (`Reconciled` is re-exported from `lib.rs`); `cargo check --workspace` run once for it. No other crate constructs `Reconciled`.
- `force = true` bypasses the guard (decision, not in the brief): the actor's `Reconcile` is an explicit request; without a bypass a repository that legitimately lost > 80 % of its files could never re-index short of `rm`.

## Not built

- Test → subject as a PageRank edge (recon §3.3 mentions "so test files are not orphans"): not asked; `tests_by_subject` and `code_symbol.tests` were the brief.
- `project_profile` on nested packages (monorepo `packages/*/package.json`, Cargo members' own justfiles): root-level only; `kind: mixed` covers a root with several manifests. Add a `manifests` walk when an orchestrator briefs into a sub-package.
- A `test_subjects` table: the `meta` JSON covers the two consumers (`code_impact`, `code_symbol`) with one read; a table would only pay off with a per-test query, which nothing does.
- Windows `Justfile`/`makefile` case variants beyond the two spellings read; `Taskfile.yml`, `tox.ini`, `noxfile.py`, `deno.json`: not detected.
