# 79 — As built: C (mesh class `vm`, lease rank, declared class, `mesh_peers`, the agent pin)

Branch `feature/79-C`, base `9f282800` (K's contract commit), worktree `../theuth-79-C`. Four feature commits + one clippy fix. Contract §2.8–2.9 behaviour is in; the pin path runs end to end against K's `EnsureVm` stub (which blocks every pinned task on `sandbox:<name>` until A lands).

## Built, per commit

1. `621a4796 feat(mesh)` — `stats::probe(id, uptime, data_dir, workspaces, declared: Option<EngineClass>)`: `Some(c)` short-circuits detection; detection moved to `detect_class(battery, display)` unchanged. Both callers in `mesh/lib.rs` (`refresh_stats`, `MeshPlugin::start`) pass `cfg.class`. `mesh/tools.rs`: the `mesh_peers` line format extracted to `peer_line(peer, stats, hub)` (byte-identical output) so it is testable; the description names `vm` among the classes. `rpc/acp.rs`: the `RegisterNode` describe string extracted to `describe(peer, stats)` (same string; `{:?}` lowercased → `vm` falls out of the enum).
2. `21d6c642 feat(tasks)` — `place::Pin { Engine(String) }`, `Inputs.pin: Option<Pin>`. In `decide`: a pin is read first — the pinned engine is the only candidate; present → `Placed { where_: <pin>, harness: <rules d–e> }`, absent → `Verdict::Missing(<engine>)` (→ `place` returns `Err("engine:<engine>")`, the existing blocker shape). Rules (a)–(c) (`where`/`harness` on the task, repo narrowing, display/GPU narrowing) are skipped under a pin; (d) `read_only → native` and (e) `repo work → omp when advertised` still apply. `DeskRouter::pin_of(agent) → Result<Option<Pin>, String>` (default `Ok(None)`); `PlaceContextReply.pin` carries it (manual `Default`); `task_place` blocks the task on the `Err` blocker without consulting engines, and accepts `sandbox:` as a blocker prefix beside `engine:`; description says what a pin does.
3. `441ab10a feat(roster)` — new `roster/src/pin.rs`: `record_name(agent, runs_in)` (`vm` → `vm-<agent>`, `vm:<n>` → `n`), `blocker(name)` = `sandbox:<name>`, `blocked_on`, `takes_work(state)` (`Ready | Running`), `engine_name(info, mesh_status)` (record `engine` id → `Peer.name`, or `self_name`), `resolve(agent, runs_in)` (asks `sandbox::EnsureVm { name, agent: Some(agent) }`; every failure or not-yet-working state → `Err("sandbox:<name>")` with the detail in a `tracing::warn!/info!`). `Desks::pin_of` = file's `runs_in` → `pin::resolve`. `Roster` subscribes to `sandbox::SandboxEvent`; on `Changed { sandbox }` with `takes_work(state)` and the lease held, every `Blocked` task whose blockers name `sandbox:<name>` gets that blocker removed (`UpdateTask`, actor `system`) — the ledger's existing "blockers cleared → unblock, wake owner `answer`" path re-wakes the desk, which runs `task_place` again with the engine now present. `desk::identity` gains a **Runs in** paragraph for pinned agents: children run on the VM's engine via `task_place` → task `where` → `spawn_subagent agent: "<where>" | "<harness>@<where>"`. `roster/Cargo.toml`: `sandbox` dep (no cycle: `sandbox` → secrets/mesh/tools/agent/update; none of those depend on roster or tasks).
4. `631215bf fix(tasks)` — clippy `manual_map`.

## Deviations (decisions; nobody to ask)

- **No `Verdict::Blocked` variant.** A pinned engine that is absent is exactly `where: <x>` naming an absent engine, which already yields `Verdict::Missing(x)` → blocker `engine:x`. Reused; every `Verdict` match is unchanged. The contract's "`Blocked(reason)`" is the roster-side `Err("sandbox:<name>")` from `pin_of`, which never reaches `decide`.
- **Pin skips rules (a)–(c), keeps (d)–(e).** The design says the pin is a constraint theuth places *within*; a repo path or the word "browser" in a pinned agent's brief must not block it on `repo:`/`display` (the VM's repos are shares under `/shared/…`, never the host paths). `where`/`harness` set on the task lose to the pin (`harness` set on the task is still honoured as a fixed harness). Tested.
- **Blocker text is bare `sandbox:<name>`; the reason is a log line.** The task's `blockers` are matched exactly by the unblock path; folding "`unavailable: not built`" into it would break the match and change per failure. `task_place`'s tool output says "the owner is pinned to a sandbox VM that is not ready; task set blocked on `sandbox:<name>`".
- **`Stopped`/`Paused`/`Crashed`/`Gone`/`Failed` after `EnsureVm` are "not ready", not errors.** `EnsureVm` is specified to start a stopped VM, so any non-working state it returns means "wait for `Changed`"; the record's engine must also be a known peer (or self), else wait too.
- **"spawn_subagent defaults `agent` to the pinned engine" is a prompt default, not a tool default.** The desk chooses `agent` from the task's `where`/`harness` today (`desk::manual` step 1); the identity paragraph makes the pin explicit and forbids another engine. A per-session default inside `spawn_subagent` lives in `crates/agent` (not C's) and would need the engine name at desk-open time, which `EnsureVm` may not have yet.
- **`mesh_peers` and `acp` describe: extraction, not new formatting.** Both strings are unchanged; the test asserts the whole `mesh_peers` line and the `mesh · vm · harnesses: native, omp` string.
- **No microsandbox guest detection hint in `stats.rs`** (brief: optional). The declared key is the contract and bring-up writes it (§2.10).
- Unblocking runs only on the hub (`self.lease.holding`) — desks and dispatch are hub duties; two engines clearing the same blocker would race the replica for nothing.

## Not built

`EnsureVm`'s body (A). A `mesh::MeshEvent` peer-arrival trigger for tasks blocked on a sandbox whose VM is `Ready` but not yet paired (see follow-ups). UI "Runs in" (E). No `os`/`arch` in `EngineStats`.

## Tests written

- `mesh`: `stats::tests::declared_class_beats_detection`, `lease::tests::vm_ranks_with_laptop_and_never_above_it` (equal-availability tie falls to id, laptop uptime wins, desktop outranks vm, `hub: never` vm filtered), `tools::tests::peer_line_prints_class_vm`.
- `rpc`: `acp::tests::describe_names_a_vm_engine`.
- `tasks`: `place::tests::pin_wins_over_every_better_engine` (repo elsewhere, display elsewhere, `where`/`harness` on the task, `read_only → native`), `place::tests::pinned_engine_absent_is_missing_never_a_fallback` (also with `place_prefer` naming an available engine).
- `roster`: `pin::tests::{record_names_and_blockers, engine_name_needs_a_working_vm_with_a_known_engine}`, `desk::tests::identity_names_the_pin` (unpinned has no line; `vm` → `vm-ops`; `vm:lab` → `lab`).

## Gate

| command | result |
|---|---|
| `cargo check -p mesh -p tasks -p roster -p rpc --tests` | ok |
| `cargo clippy -p mesh -p tasks -p roster -- -D warnings` | ok (after `631215bf`) |
| `cargo test -p mesh --lib` | 23 passed |
| `cargo test -p tasks --lib place` | 11 passed |
| `cargo test -p roster --lib` | 46 passed (no `monty` needed) |
| `cargo test -p rpc --lib describe_names` (extra, for the acp test) | 1 passed |

`cargo` at `/opt/homebrew/opt/rustup/bin`.

## Edits outside C's files (flag)

- `crates/tasks/src/lib.rs` — `DeskRouter::pin_of` (the roster→tasks hook; the only way `task_place` learns the pin without a tasks→roster dependency).
- `crates/tasks/src/tools.rs` — `PlaceContextReply.pin`, `task_place` uses it and blocks on `sandbox:`; description text. Placement runs in this tool, not in `desk.rs` (the brief's "PlaceTask / tasks::place call sites in desk.rs" do not exist — the desk calls the `task_place` tool).
- `crates/roster/src/pin.rs` — new file in C's crate.
- `crates/roster/Cargo.toml` — `sandbox` dep.
- `crates/rpc/src/acp.rs` — `describe` extracted (brief allows the describe string only; the string is unchanged).

## Follow-ups for the integrator

- A: `EnsureVm` must return the record with `engine` set once paired and publish `SandboxEvent::Changed` on `Ready`/`Running`; C's unblock keys on `sandbox.name` and `takes_work(state)`. While A's stub answers `Err("not built")`, every pinned task blocks on `sandbox:<name>` (by design).
- A race the current code tolerates but does not close: `Changed { Ready }` may arrive before the VM's peer row replicates; the desk's `task_place` then re-blocks on `sandbox:<name>` and waits for the next `Changed` (e.g. `Running`). If A does not emit a later event, add a `MeshEvent::Peer` trigger in `Roster`'s handler (same unblock body).
- E: Agents view **Runs in** (K's `runs_in` field); no UI change here.
- H/I: `tasks::place::Pin` is re-exported only as `tasks::place::Pin`; nothing else needs it.
- Cargo.lock: `roster → sandbox` edge added.
