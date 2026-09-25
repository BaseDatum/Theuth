# 69 MONTY — monty 1.0.0-beta.2 → 1.0.0 (as built)

Branch `chore/monty-1-0-0`, worktree `/Users/Sky.Moore/Develop/theuth-monty`.

## Commits

### 1. `b9dfe74 chore(runtime): monty 1.0.0`
- `Cargo.toml`: the five monty pins `=1.0.0-beta.2` → `=1.0.0` (`monty-pool`, `monty-types`, `monty-fs`, `monty-proto[worker]`, `monty-alloc[exit-code]`). The brief named four; `monty-alloc` is the worker's global allocator and must match the others (`cargo update` refuses a mixed set), so it moved too.
- `Cargo.lock`: `cargo update -p monty-pool -p monty-types -p monty-fs -p monty-proto -p monty-alloc` — nine monty crates 1.0.0-beta.2 → 1.0.0, adds `minicbor 2.3.0` + `minicbor-serde 0.7.1` (CBOR dumps, #913).
- `crates/agent/src/repl.rs:1423`: `TurnEvent::ResolveFutures { pending_call_ids }` → `{ pending_call_ids, .. }`. The only compile error in the workspace: #909 added `position: Option<SourceRange>` to every suspension event. `FunctionCall`/`OsCall` arms already used `..`.
- `docs/design/08-repl.md`: header pointer + new §16 "As-built 2026-09-25: monty 1.0.0" (API delta, unchanged contracts, snapshot compatibility).
- `crates/theuth/src/worker.rs` (the `t subprocess` worker), `crates/tools`: compiled unchanged. (The brief says "crates/runtime repl.rs/worker.rs"; in this tree `repl.rs` is `crates/agent/src/repl.rs` and `worker.rs` is `crates/theuth/src/worker.rs`; `crates/runtime` is the YAML-config runtime crate and has no monty dependency.)

### 2. docs: `docs/design/69-as-built-MONTY.md` (this file).

## beta.2 → 1.0.0 API delta (from crate sources in `~/.cargo/registry`, diffed pub items)
- `monty-proto`: `PROTOCOL_VERSION` 5 → 5 (unchanged). New `pub struct SourceRange`, `pub enum Persistence`. `os_call_to_proto` signature reformatted only.
- `monty` (worker side): `DUMP_VERSION` 12 → 13, `MIN_SUPPORTED_DUMP_VERSION = DUMP_VERSION` (still reads exactly one version).
- `monty-pool`: `ReplConfig.persistence: Persistence` (`ServerDefault | Ephemeral | Stored`, `#[non_exhaustive]`), `Checkout::session_id() -> Option<&[u8]>`, `Checkout::worker_id() -> Option<u64>`; `PoolError::Shutdown { dump }` auto-resume path (`after_shutdown`/`can_resume`) — all relay/WebSocket-only (#933). A local subprocess worker: `session_id()` is `None`, `persistence` ignored; theuth's `ReplConfig` literal ends in `..ReplConfig::default()` so nothing changed.
- `monty-types`: `SourceRange`, `SourceRange::new/unknown`, `MAX_FILENAME_LEN`, `allocate_into_baseline` (#928 lazy type checker charged to the worker baseline), `MontyUuid` serde-bytes attribute.
- `monty-fs`, `monty-alloc`: no pub-item change.

## Behaviour changes 1.0.0 introduces (checked against source and tests)
- **Dump format 12 → 13** (CBOR, #913/#922). A beta.2 `~/.theuth/runtime/<session>.monty` is refused on `Load` with `dump format version 12 is older than 13, the oldest this build reads` (`monty-1.0.0/src/dump_format.rs:244`; text format unchanged from beta.2, the numbers differ). The existing §5 path handles it (rename `.stale`, one `warn!`, `<runtime> saved state could not be restored … It held: …` from `.names.json`) — the same path the corrupt-dump case in `capping_binds_out_name` exercises. [INFERENCE] the exact refusal string was read from source, not produced from a real beta.2 dump (none on this box).
- **Sleep semantics: unchanged.** `SleepMode::System(max)` still hands `TurnEvent::OsCall { system_sleep }` to the host; `drive` waits ≤ `MAX_SLEEP` 300 s per call with the permit released; the pool charges `max_total_sleep` 600 s per checkout; the crossing sleep raises `TimeoutError: sleep limit exceeded`. `Checkout::restore` still zeroes `sleep_used` around `Load` and restores it. `sleeps_are_waited_host_side` passes.
- **Suspension positions** (#909): every `FunctionCall`/`OsCall`/`ResolveFutures` carries `position`. Ignored by the bridge; could later point error hints at the cell line.
- **Error text**: no monty error string the bridge matches on changed (`gather() does not yet support keyword arguments`, `MemoryError: memory limit exceeded`, `protocol violation: failed to load session`) — the `repl` tests asserting them pass.
- Wire protocol 5, nested-mount refusal + `/spill` skip, CWD = first mount, `OsPolicy` clock/timezone, `set_hard_limit(memory_limit_with_headroom(…))`: all unchanged and preserved.

## Deviations
- Task text (line 15) overrides the template's "verification is not your job": it names `cargo check --workspace --tests`, `cargo build -p theuth --bin t`, symlink + `cargo test -p runtime -p agent --lib`. All were run.
- `cargo check --workspace --tests` initially failed in `crates/nodes` (test target `include_str!("../../../bindings/methods.json")`, a gitignored generated file). Generated with `cargo test -p rpc --features ts export_bindings` (not committed; `/bindings` is gitignored). Unrelated to monty.
- First full `cargo test -p runtime -p agent --lib` ran concurrently with the workspace check and the `t` build: ~45 `store::*`/`providers`/`search`/`loop_` tests failed and the five worker-backed `repl` tests exceeded 60 s. Rerun on an idle machine: **agent 307 passed / 0 failed, runtime 2 passed / 0 failed** (11.5 s). `store::tests::log_api_round_trip` alone passed in 0.15 s. Load-induced flakiness, not a monty regression.
- Report copied to `docs/design/69-as-built-MONTY.md` (the template says `67-…`; this is the 69 series, matching `68-as-built-*.md`).
- `monty-alloc` bumped although the brief listed four crates (see commit 1).

## Not built
- No use of `Persistence`/`session_id`/`SourceRange`: relay features and positions have no consumer in theuth today.

## Tests written
None new. No behaviour changed on theuth's side; the existing 18 `repl` tests (worker-backed via `target/debug/t` as `monty`) cover the port.

## Exact verification results
- `cargo check --workspace --tests`: exit 0; only pre-existing warnings (`crates/document/tests/actor.rs` unused `vlink`/`elink`; `block v0.1.6` future-incompat note).
- `cargo build -p theuth --bin t`: `Finished dev profile … in 11m 35s`.
- `ln -sf target/debug/t /tmp/69/montybin/monty; PATH=/tmp/69/montybin:$PATH cargo test -p runtime -p agent --lib`:
  `test result: ok. 307 passed; 0 failed` (agent), `test result: ok. 2 passed; 0 failed` (runtime).

## Knowledge paragraph
theuth pins monty `=1.0.0` (from 1.0.0-beta.2, 2026-09-25) for `monty-pool`, `monty-types`, `monty-fs`, `monty-proto[worker]`, `monty-alloc[exit-code]`. Wire protocol 5 unchanged; dump format 12 → 13 (CBOR), so beta.2 `.monty` snapshots are refused ("dump format version 12 is older than 13") and land on the existing `.stale` + `.names.json` recovery path. The only source change was `TurnEvent::ResolveFutures { pending_call_ids, .. }` in `crates/agent/src/repl.rs` (new `position: Option<SourceRange>` on every suspension event). `ReplConfig` gained `persistence` (relay-only session IDs/forking/ephemeral/auto-resume; a local `t subprocess` worker ignores it). The host-side sleep contract (300 s/call `MAX_SLEEP`, 600 s/turn `MAX_TOTAL_SLEEP`, host waits and resumes `None`), `OsPolicy` fixed-offset clock, the nested `/cwd`–`/spill` mount skip, and `set_hard_limit(memory_limit_with_headroom(…))` in `crates/theuth/src/worker.rs` all carry over unchanged. Verified: `cargo check --workspace --tests` clean; agent 307/307 and runtime 2/2 with `target/debug/t` symlinked as `monty` on PATH.

DONE MONTY
