# 79 — as built, worker F2 (sandbox engine bugs from the live smoke)

Branch `fix/79-F2` (the brief says `fix/79-F2-sandbox`; the worktree was created on `fix/79-F2`, kept), base `b4e5c38b`, worktree `theuth-79-F2`. Bugs by `79-smoke.md` numbering. Cargo is not on `PATH` in this shell; every command below ran with `PATH=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin:~/.cargo/bin:$PATH`.

## Commits

| sha | what |
|---|---|
| `e08bca64` | bug 8 + bug 2 (vm layer): `VmRuntime::start(h, out)`; recreate steps published; `boot()` rebuilds the SDK backend on any failure and hints "restart the engine" on the FK text; `snapshot()` falls back to `Snapshot::reindex_default()`; preflight probe panic → blocker; `VmMetrics` doc; `examples/f2_wedge.rs` rig |
| `91d57088` | bugs 6 + 4 (actor): `all_snapshots(name)` over `<name>` and `<name>-r`, used by Snapshot/Restore/auto_snapshot/health tick; Destroy sweeps both runtime names and both snapshot groups; Create prunes stale `auto-*`; tests |
| `b12128d3` | bug 7 hardening: bring-up body in its own `tokio::spawn` (panic → `tracing::error!`, `BringupDone` still sent); health tick logs a dead actor |
| `4147c247` | guest/host prompt guidance (secret paste, `/` in names); memory gauge sentence in `sandbox_list` and `sandbox_resize` text |
| `83968635` | `Terminal` actor message + `sandbox_terminal` RPC |

Bugs 8 and 2 share one commit (and 6/4 another): their hunks interleave in `microsandbox.rs`/`actor.rs` and splitting them would have left intermediate commits that do not compile. Deviation from "each its own commit", noted.

## Bug 2 — `insert run: FOREIGN KEY constraint failed` after a failed boot

**Not reproduced.** `examples/f2_wedge.rs` replays the smoke's sequence in one process on this Mac (image `theuth-vm:dev`, names `f2-*`): create `f2-a` → live resize → `max_memory 65536` for the next start → `set_mounts` → stop → start (recreate: disk snapshot, `Sandbox::remove`, boot from the snapshot) → the boot dies in the runtime child with `SetUserMemoryRegion(MemoryMap)` exactly as in the smoke → a plain create of `f2-c` **succeeds** (three runs: 422 ms, 253 ms, 745 ms), and `snapshot` + `msb snapshot ls` agree. Both before and after the fix. So the wedge needs something the smoke had and the rig does not (a long-lived engine with many concurrent SDK users: health tick metrics, `exec` from bring-up, an `override_snapshot` restore right after) `[INFERENCE]`.

SDK reading (scout, file:line under `~/.cargo/registry/src/index.crates.io-*/`):
- `insert run` is in the **runtime child**, not the host: `microsandbox-runtime-0.7.4/lib/runner/vm.rs:3153-3167`, called at `:693-718` right after the child opens its own `DbWriteConnection` on the same `msb.db`; FK `run.sandbox_id → sandbox.id ON DELETE CASCADE` (`microsandbox-db-0.7.4/lib/entity/run.rs:89-95`). The child failing there means it cannot see the `sandbox` row the host's `insert_sandbox_record_with_status` (`microsandbox-0.7.4/lib/backend/local/sandbox/create.rs:1947-1986`, a `db.transaction`) had just committed.
- The host's `LocalBackend` holds one `OnceCell<DbPools>` (`backend/local/mod.rs:71`, opened lazily `:149-166`); the write side is a **single-connection** sqlx pool (`microsandbox-db-0.7.4/lib/pool.rs:45-55`, `connection.rs:120`). sqlx-sqlite tracks `transaction_depth` itself and only changes it when BEGIN/COMMIT/ROLLBACK statements succeed (`sqlx-sqlite-0.9.0/src/connection/worker.rs:38, 213-315`); sea-orm nests as `SAVEPOINT` when depth > 0 (`sea-orm-2.0.4/src/database/transaction.rs:139-152`). One desynced depth on that connection (a COMMIT that returned SQLITE_BUSY against the dying child's own writes, or a `DatabaseTransaction` dropped mid-lock, `transaction.rs:402-405`) makes every later host write a savepoint inside a never-committed outer transaction: invisible to the child (FK), to `msb snapshot ls` (the index upsert at `backend/local/snapshot/create.rs:212-220` "succeeds" but only warns anyway) and to the `Sandbox::remove` row delete the smoke saw "return Ok" while the row stayed — all three smoke symptoms, one mechanism `[INFERENCE, code-grounded]`. The pool never recycles that connection, so only a process restart healed it.
- The SDK has no per-call backend setter on the builder; `set_default_backend`/`swap_default_backend` are a `RwLock` and repeatable (`backend/mod.rs:174, 186`); `Snapshot::reindex_default()` is public (`snapshot/api.rs:304`).

**Fix shipped (defensive, cheap):** `boot()` in `vm/microsandbox.rs` calls `reset_backend(home)` on every boot failure — a fresh `LocalBackend::builder().home(..).build_lazy()` swapped in with `swap_default_backend`; fresh pools, the suspect connection dropped. On the FK text the error adds "the SDK connection was rebuilt — retry, and if it persists restart the engine to recover". `snapshot()` checks the new id against `snapshots()` and calls `reindex_default()` when missing. `HOME` (a static `Mutex<String>`) remembers `cfg.home` for the rebuild. The proof above ran with the reset in place (`f2-c` created and snapshotted after `f2-a`'s failed boot, same process). What is **not** proven: that the reset heals the real wedge — it was never observed here.

**Upstream issue text (for `superradcompany/microsandbox`):** "After a `msb machine` child fails in `build_microvm` (e.g. `SetUserMemoryRegion(MemoryMap)` from an oversized `max_memory`), a long-lived SDK process can end up with every later `create` failing in the child with `insert run: FOREIGN KEY constraint failed`, `Sandbox::remove` returning Ok without deleting the row, and `snapshot_index` rows invisible to `msb snapshot ls` until `msb snapshot reindex` from another process. Restarting the SDK process fixes it. The single-connection write pool (`microsandbox-db/lib/pool.rs:45-55`) plus sqlx's self-tracked `transaction_depth` suggests a desynced transaction on that connection; consider `PRAGMA`/`sqlite3_get_autocommit` resync or recycling the write connection after a failed startup."

## Bug 4 — destroy of a `gone` record

`Destroy` (actor.rs) now: mesh peer removed → for each of `<name>`, `<name>-r`: if the runtime status is not `Gone`, stop (20 s) and `destroy` → with `keep_snapshots: false`, delete every snapshot returned by `all_snapshots(name)` (both groups) → vault row, metrics, record, `Removed`. `keep_snapshots: true` keeps every snapshot of both groups (named and `auto-*`), so a later `Create` of the same name can still restore them; that `Create` deletes the stale `auto-*` ones (they belong to a disk that no longer exists) and keeps the named ones. `Microsandbox::destroy` already handles a stopped msb row (`Sandbox::get` → `destroy`); a row stuck in `Starting` (SDK rollback failure) is still refused by the SDK — best effort, logged.

## Bug 6 — `vm.snapshots` empty after restore

`SandboxActor::all_snapshots(name)`: `runtime.snapshots()` for `VmHandle { name }` and `{ name }-r`, merged, sorted by `at_ms`, deduped. Used by `Snapshot`, `Restore`, `auto_snapshot` (retry/restore/destroy) and the reconcile pass (health tick: the record is rewritten and `Changed` published when the list differs). Ids keep the group they were created under (`lab:clean`, `lab-r:auto-restore-…`); `Microsandbox::snapshots` already filtered by `<name>:` prefix or `group == name`, so no runtime change was needed.

## Bug 8 — recreate progress

`VmRuntime::start(&self, h, out: &Progress)` (contract deviation, doc'd on the trait). `Microsandbox::boot` takes the step name (`create` / `start` / `restore`), so the pull/startup lines of a recreate arrive as `SandboxEvent::Progress { step: "start" }`: `disk snapshot lab:auto-reconfigure-…`, `removed lab; recreating with the new shares/ports`, `startup: PreparingSnapshot`, `startup: Activating` (seen live in the rig output). The actor's `start()` passes `self.progress(name, None)`. Mock, `tests/vm.rs`, `vm_spike` updated.

## Bug 7 — the silent host-engine exit

Audit, `crates/sandbox` (edited) and read-only `crates/rpc/src/acp.rs`, `crates/agent/src/supervisor.rs`, `crates/agent/src/acp_child.rs`, `crates/tools/src/approval.rs`:
- No `std::process::exit`/`abort` in `crates/sandbox`, none in the SDK's host side (only the runtime child `runner/vm.rs:593`, a separate process). SDK `Drop`s SIGTERM/SIGKILL the child VM pid only (`runtime/handle.rs:551-553`, `backend/local/sandbox/mod.rs:1125-1145`).
- The approval path is inert: `api.rs:3253 resolve_approval` → `tools::approval::Resolve` (`approval.rs:157-165`, a `HashMap::remove` + oneshot send); `acp.rs:867-907` and `acp_child.rs:1160-1191` only spawn tasks that reply to the harness.
- **The one path that ends the engine with a normal-looking log and no stderr:** `crates/theuth/src/main.rs:1477-1488` — `rt.shutdown.cancelled()` → 100 ms → `std::process::exit(0)`. Its triggers in a headless engine: the **`shutdown` RPC (`api.rs:4402-4405`, any authenticated peer, not logged)**, `ctrl_c`, `parent_gone()` (only with `THEUTH_PARENT_PID`). The `AcpPlugin` triggers (`acp.rs:84, 93`) are not in `engine_plugins`. The smoke's timing ("seconds after the first remote `resolve_approval`", exit 0, log ends normally) fits a client-side `shutdown` call or a signal, not a panic (panics go through the hook at `main.rs:1366-1370` → `tracing::error!("panic: …")`, which the smoke did not see) `[INFERENCE]`. Follow-up for the integrator (file I may not edit): `tracing::warn!("shutdown requested by rpc peer {id}")` in `api.rs:4402`, and a `tracing::info!` before `process::exit(0)` in `main.rs:1488` naming which select arm fired.
- Hardening shipped: bring-up runs in its own `tokio::spawn`; a panic is logged and the name freed (before, a panic left `inflight` set forever and no log). `Microsandbox::preflight` no longer `expect`s the probe join (a panic becomes a `preflight_failed` blocker). `spawn_health` logs `tracing::error!` once when the actor is gone. `reconcile` already `unwrap_or(Gone)`s every runtime call.

## Guest secret-paste guidance / host name rule

`tools.rs` `guest_prompt`: "a vault placeholder may be typed (computer_type) only into a masked password field or a `read -s` prompt — never into a visible terminal, editor or text field … and take no screenshot while a field shows a secret". `HOST_PROMPT`: "A secret_request name cannot contain `/`: use dots".

## Memory gauge semantics

`VmMetrics` doc comment (`vm/mod.rs`), the `sandbox_list` one-line row (`memory=<used>/<configured> (limit <memory_limit_bytes>, guest MemTotal stays at boot size)`) and a `note:` line on `sandbox_resize` results that applied `memory_mib`: the figure is the hypervisor allocation; the guest's `MemTotal` stays at the boot size by virtio-mem design — not a bug.

## `sandbox_terminal`

Actor `Terminal { name, session: SessionId, root: PathBuf } → Result<String /* job id */, String>` (actor.rs): refuses a record that is not `ready`/`running`; command `<msb> exec --tty <runtime_name> -- sh -c 'if command -v runuser …; then exec runuser -u engine -- bash -l; else exec bash -l; fi'` (`msb` from the new `vm::install::msb_path(home)` = the SDK's resolved binary); `tools::pty::StartPty` on `JOBS_ACTOR` with `name: "sandbox <name>"`, `idle_secs: 0` (no `<job_idle>` nags; nothing reads it as a model), `cwd = data_dir`; the job id comes from the reply's `structured.job`. RPC `sandbox_terminal(name, session?: SessionId) → { job, session }` (`api.rs`, `with_extensions`): `session` defaults to `owner_root()` and is passed through `authorize`, whose `resources.root` is the job root — the same admission every PTY RPC uses. Deviation from the brief's `sandbox_terminal(name) → { job }`: a PTY job must belong to a session the UI can address, so the session is returned (and may be supplied). **UI route:** `sandbox_terminal(name)` → `{ job, session }` → `subscribe_pty(session, job, rows, cols)` for the byte stream (`reset`/`data`/`exit`), `pty_input(session, job, …)` to type, `pty_resize` on resize — the design-55 terminal pop-out, no new type in `export.rs`. Not tested live (needs a running engine + jobs actor; `cargo check -p rpc -p theuth --tests` passes).

## Tests

`actor.rs`: `restore_alternates_runtime_name_and_destroy_removes_everything` now asserts the record's list holds `lab:clean` + `lab:auto-restore-*` after the first restore and `lab-r:auto-restore-*` after the second, and that `Destroy { keep_snapshots: false }` leaves both runtime names `Gone` and both groups empty; new `destroy_of_a_gone_record_sweeps_groups_and_create_prunes_auto`. The fake `Join::remove_peer` now forgets the peer (a re-create of the name was refused as "already in the mesh"). Mock tests updated for the `start` signature. `examples/f2_wedge.rs` is the live rig (kept, like `vm_spike`).

## Gates

- `cargo check -p sandbox -p rpc -p theuth --tests` → Finished (4 m 11 s).
- `cargo clippy -p sandbox -- -D warnings` → clean.
- `cargo test -p sandbox --lib` → 41 passed (40 + 1 new).
- Live: `cargo run -p sandbox --example f2_wedge` → sequence above, `f2-c` create ok after the forced failure, snapshot indexed; `msb ls` / `msb snapshot ls` empty afterwards; no `f2-*` left.

## Edits outside owned files (flagged)

- `crates/sandbox/tests/vm.rs:155`, `crates/sandbox/examples/vm_spike.rs:284` — the `start` signature (both in `crates/sandbox/**`, listed for completeness).
- `crates/rpc/src/api.rs` — trait method + body for `sandbox_terminal` (the brief's allowance). No `export.rs`/bindings change.
- Nothing in `crates/theuth`, `crates/agent`, `crates/tools`, `ui/`.

## Not built / follow-ups

- Bug 2's real wedge was not reproduced; the reset is unverified against it. Next smoke: run the long sequence in an engine and, if the FK returns, check whether the retry (which now runs on fresh pools) succeeds; if not, the upstream issue text above is ready.
- Bug 7: add the two log lines in `api.rs:4402` / `main.rs:1488` (integrator); rerun the smoke's approval round with `RUST_BACKTRACE=1`.
- `sandbox_terminal` needs the UI wiring (E's Terminal button → the three RPCs above) and one live check that `runuser` exists in `theuth-vm:dev` (the fallback covers its absence).
- `79-sandbox-vm.md` §16 table row and `79-contract.md` §2.1 (`start` signature) / §2.3 (`Terminal`) / §2.4 (`sandbox_terminal`) not edited — integrator's.
