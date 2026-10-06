# 108 — as built, ENG (engine side)

Branch `feat/108-eng` off `8490bb5a`. Spec: `108-routines-ui.md` §7, §8, §9, §11, §12.

## Files changed

| file | what |
|---|---|
| `crates/agent/src/sensor/mod.rs` | `SensorArg` (serde + schemars + ts); `SensorMeta` gains `args`, `description`, `usage`; `resolve()` parses `# arg:` lines (`parse_arg`) and the module docstring (`docstring`); a malformed or repeated `# arg:` is `SensorError::Header`; `SensorRunner::shipped_version(name)` and `edited(name)` expose the install rule; `BUILTINS` gains `rss.py`; tests `arg_header_lines`, header-error table extended, install expectations bumped (`fs_exists 3`, `rss.py` written) |
| `assets/sensors/rss.py` | **placeholder** (§11 rule): header `# theuth-sensor: rss 1` + the three `# arg:` lines from §10, a docstring with the `when:` line, a `check` that never fires. RSS's file overwrites it at integration |
| `assets/sensors/pr_watch.py`, `fs_exists.py` | headers only: `# arg:` lines per §9.1; versions `pr_watch 2`, `fs_exists 3` |
| `assets/sensors/README.md` | §Header: the `# arg:` grammar paragraph; example version bumped to match the file |
| `crates/schedule/src/lib.rs` | `ListSensors`, `TestFile` messages; `SensorInfo`; `RunKind` + `RunKind::of`; `RunInfo.kind`; `ScheduleInfo` gains `sensor`, `for` (serde rename), `cooldown`, `timeout`, `model`, `run_timeout`, `created`; `list_sensors(&SensorRunner)` free fn (test `list_sensors_rows`: 68 schedule tests after it); `dur()` |
| `crates/schedule/src/store.rs` | `history()` computes `kind`; test `history_rows_carry_their_kind` |
| `crates/schedule/src/validate.rs` | `test_file(file, sensors)` (§9.4) shared by the RPC; test `meta()` gains the new fields |
| `crates/schedule/src/tools.rs` | `ROUTINES_SKILL`; `PROMPT` + "load the `routines` skill" sentence; `ScheduleArgs` gains `agent`, `project`; `render_args()` extracted (pub — `routine_draft` reuses the exact pre-write path); `enabled: a.enabled.unwrap_or(!watcher)` (D12); `ListSensorsTool` (`list_sensors`, ReadOnly) + `sensor_line`; tests |
| `crates/schedule/routines.md` | the skill (§7): mental model, key table, ask/don't-ask, sensors + YouTube rule, §2.2 grammar verbatim (lines 141–206 of the spec, `diff` empty), card + press table, validation, editing, five examples |
| `crates/app/src/routine_draft.rs` | `routine_draft` tool, `RoutineDraft`/`RoutineDraftArgs`, `approval()` → `Enabled`, `source()`, `parts()`, `validate()`; tests mirroring `agent_draft`'s |
| `crates/app/src/lib.rs`, `crates/app/Cargo.toml` | module + re-exports + registration beside `AgentDraftTool`; `schedule` dependency (+ `schedule/ts` in the `ts` feature) |
| `crates/rpc/src/api.rs` | `list_sensors`, `test_schedule_file` methods |
| `crates/rpc/src/export.rs` | `schedule::SensorInfo`, `schedule::RunKind`, `agent::sensor::SensorArg` |
| `crates/theuth/src/main.rs` | `("routines.md", ROUTINES_SKILL)` in `first_run`'s binary-owned list |

## Judgement calls beyond the doc

1. **`RunKind::of` gets a `task <id>` arm → `Run`.** An `agent:` fire records `session: None, result: "task <id>"` (`lib.rs` fire path, `insert_run_with`); the doc's rule would have classed it `Check`. Added and pinned in the store test.
2. **`ScheduleInfo.for_`/`cooldown`/`timeout` are `Some` only for watchers** (like `fire`/`mode`/`memory`), formatted with `humantime::format_duration` (`5m`, `0s`); the UI's "defaults are silent" rule compares against `0s`/`20s`. `expires` and `run_timeout` are formatted the same way; `model` is the string as written.
3. **D12 implemented as `enabled.unwrap_or(!watcher)`** without an extra `&& verdict.error.is_none()`: the dry run returns `Err` (not a `Verdict` with `error`) and refuses before the write, so the condition is implied. Test `failing_dry_run_writes_no_file` now passes `enabled: true` to pin that the explicit flag does not bypass the refusal.
4. **`render_args(&ScheduleArgs)` is public** and `routine_draft` builds a `ScheduleArgs` from the draft field by field, so the card's validation is the `schedule` tool's own render → parse path (and the compiler flags drift between the two structs).
5. **`routine_draft` resolves sensors with a bare `SensorRunner::new(data_dir, None, None, default)`** — `resolve()` only reads headers, so no pool or registry is needed; the app crate gets no new constructor parameter (registration mirrors `AgentDraftTool` exactly).
6. **Card `Trigger` label for a watcher is `every <d> when <file>`** (file only, as §8.2 says), not the full `when:` string; the sentence carries the args.
7. **`list_sensors` reads `NeedsCpython` as a row, not an error**: `resolve()` returns `Err(NeedsCpython)` before the header is handed back, so the row has `needs_cpython: true, cpython_available: false, error: None` and the header fields stay default (the UI disables it on the `needs_cpython && !cpython_available` rule, §5.1).
8. **`# arg:` duplicates are a header error** (`# arg: a repeated`) — not in the doc; a repeated key would make the form ambiguous.
9. **The skill's agent-routed example drops `missed: run`** from the §2.2 fixture sentence: `ScheduleArgs`/`RoutineDraft` have no `missed` (the tool hard-codes `missed_run: false`), so a card cannot set it; the skill says it is form-only.
10. **`docstring()` scans the whole file**, not just 16 header lines, for the first `"""` block: `usage` lines sit at lines 5–7 in the built-ins but the docstring is not bounded by `HEADER_LINES` in 40 §4.1.

## Gates

All with `export PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-108-eng-target`.

```
cargo test -p schedule -p app -p rpc -p agent --no-fail-fast
  agent:    test result: ok. 381 passed; 0 failed; 0 ignored
  app:      test result: ok. 43 passed; 0 failed; 0 ignored   (routine_draft: 3 new)
  app/publish.rs: ok. 1 passed
  rpc:      test result: ok. 36 passed; 0 failed; 0 ignored
  rpc/prompts.rs: ok. 1 passed
  schedule: test result: ok. 67 passed; 0 failed; 0 ignored   (after the one fix below)
cargo check --workspace
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 13m 08s
cargo clippy --workspace --all-targets -- -D warnings
  FAILS on PRE-EXISTING code outside this branch (untouched files, toolchain 1.98.1 lints):
    crates/talk/src/policy.rs:580 (iter_with_drain), crates/tasks/src/packet.rs:406,
    crates/tasks/src/tools.rs:480,481,741, crates/tasks/src/ledger.rs:4977,5052,5391,5470
  `git status crates/talk crates/tasks` is empty on this branch; last change to policy.rs is cde73913.
cargo clippy --no-deps -p agent -p schedule -p app -p rpc -p theuth --all-targets -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 23s        (clean)
just ts
  test result: ok. 8 passed; 0 failed   → bindings/{SensorArg,SensorInfo,RunKind,RunInfo,ScheduleInfo}.ts,
  methods.json gains "list_sensors", "test_schedule_file"
```

The first `cargo test` run had one failure, `tools::tests::render_args_enabled_rule_and_routing`:
my assertion expected `project: web`; `file::render` writes `project: "web"` (YAML string). Test
fixed, not the renderer.

**Generated TS vs §9.6**: identical in shape. `SensorArg { name: string, required: boolean,
example: string | null, help: string }`; `SensorInfo` has the 13 fields of §9.2 in order
(`Option` → `| null`, `Vec` → `Array<>`); `RunKind = "run" | "check" | "end" | "skip"`;
`RunInfo.kind: RunKind`; `ScheduleInfo` + `sensor`, `for`, `cooldown`, `timeout`, `model`,
`run_timeout` (`string | null`) and `created: string | null` (ts-rs renders `DateTime<Local>` as
`string`, like `next`/`last`). The UI tree's hand-written stubs are not in this worktree
(`crates/desktop/ui/src/rpc/bindings/` is absent here), so the diff against them is the
integrator's; the field list above is what they must match.

rustfmt: run on `crates/app/src/routine_draft.rs` (new file) only. Running it on the other
touched files reformatted pre-existing code (e.g. 41 lines of `main.rs`, 94 of `validate.rs`,
and `mcp.rs` through `main.rs`'s modules) — reverted to keep the diff to this wave's lines.

## Not exercised

- The **clean-verdict `enabled: true` write** end-to-end through `ScheduleTool::call` (needs a Monty worker pool for a real `check`): covered at the render level (`render_args_enabled_rule_and_routing`) plus the refusal path; the dry run itself is unchanged from 40.
- `routine_draft` through `commit_source` (the OpenUI helper): `source()`/`validate()`/`approval()` are unit-tested like `agent_draft`'s; the actor path is the same code `agent_draft` runs.
- `list_sensors` / `test_schedule_file` over a live RPC socket; the handlers are one `ask` each; `list_sensors()` itself is unit-tested (`list_sensors_rows`), `test_file()` only through `dry_run`'s existing tests (it is render + parse + `dry_run`).
- `rss.py`'s body (placeholder only); its header parses (`arg_header_lines` asserts three args).
