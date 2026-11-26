# 222 — Startup progress: no fixed cap, visible phases, relaunch never kills a starting engine

Status: contract for the build on `fix/mesh-icon-providers`. Owner report 2026-11-26: after an upgrade the desktop
start screen gives up after 15 s and the owner presses Relaunch 3–4 times.

## Root cause
- `crates/desktop/src/launch.rs` `READY_TIMEOUT = 15 s`; `wait_ready` polls only the TCP port, which the engine binds
  last (`RpcPlugin` is pushed last in `engine_plugins`, `crates/theuth/src/main.rs`), after every plugin and migration.
- Relaunch → `Launch::relaunch` → `start()` → `inner.daemon.take()` + `Daemon::stop` SIGHUPs/kills the engine mid-start;
  every press starts over (committed migrations carry over, which is why 3–4 presses work).
- The engine logs only to the log file, so the screen has nothing to show.

## Contract: `<data_dir>/startup.json`
Written by the engine, atomically (temp file + rename in the same dir), on every phase/step change and by a heartbeat.

```json
{
  "v": 1,
  "pid": 123,
  "started_ms": 1764160000000,
  "seq": 42,
  "hb_ms": 1764160003000,
  "state": "running",
  "phase": "migrate:agent",
  "label": "Migrating agent.db",
  "step": {"n": 3, "of": 78},
  "done": [{"phase": "config", "label": "Reading config", "ms": 12}],
  "error": null
}
```

- `state`: `running` | `ready` | `failed`. `ready` is written right after the RPC listener has bound
  (`crates/rpc-compose/src/bootstrap.rs`); `failed` with `error` (string) when startup returns an error before ready.
- `seq` increments ONLY on a phase or step change (progress). `hb_ms` is bumped every 1 s by a dedicated `std::thread`
  (not tokio) while `state == running` (liveness). The heartbeat stops at ready/failed.
- `phase` ids (stable, machine-readable): `lock`, `config`, `runtime-init`, `plugin:<name>` (step n/of = plugin index/count),
  `migrate:<db>` (step = migration n of pending, or omitted when only a pending count is known), `rpc-listen`, `ready`.
  `label` is human text. `done` lists finished phases in order with their duration in ms (cap the list at 200 entries).
- A new engine start overwrites the file (`pid`/`started_ms` identify the boot). The file is not deleted at ready.
- Readers must tolerate a missing file (older engine) and unknown fields/phases.

## Shell (crates/desktop/src)
- Remove the 15 s cap. The wait loop ends when the port answers (success), the child exits (failure), `startup.json` says
  `failed` for this pid (failure), or no heartbeat for `HARD_STALL` = 10 min (failure).
- `SOFT_STALL` = 90 s without a `seq` change while alive → the progress event carries `stalled: true`; the wait continues.
- Fallback: if no `startup.json` for this pid appears within 20 s (older engine), keep the old behaviour but with a 60 s cap.
- A tailer polls `startup.json` every 250 ms and emits the Tauri event `startup-progress` with payload
  `{ "status": <startup.json or null>, "alive": bool, "stalled": bool, "waited_ms": u64 }`.
  Command `startup_status()` returns the same payload (initial state for a late-mounted UI).
- Never kill a starting engine implicitly: `connection()` while a start is in flight joins it. `relaunch(force: bool)`:
  if the current engine is alive, not ready and its `seq` moved within `SOFT_STALL`, a non-forced relaunch returns the
  error string prefix `starting:` + label (e.g. `starting: Migrating agent.db (3/78)`) and does nothing; `force: true`
  stops it as today. A dead/failed/stalled engine relaunches without force.

## UI (crates/desktop/ui)
- `lib/tauri.ts`: `startupStatus()`, `onStartupProgress(cb)`, `relaunch(force?: boolean)`.
- Start screen: a phase list from `done` (label + duration) and the active phase with its n/of bar and a live elapsed
  timer; total elapsed. "Taking longer than usual — still working" text when `stalled`.
- Relaunch is offered only when the engine exited/failed, or when `stalled`. If relaunch answers `starting:` the screen
  shows "Still <label>; relaunching will interrupt it" with a confirm that calls `relaunch(true)`.
- No `startup.json` (older engine): keep today's 3-step view with the last log line.

## Ownership
- Engine: `crates/rpc/src/engine.rs` (or a new `crates/rpc/src/startup.rs`), `crates/theuth/src/main.rs`,
  `crates/rpc-compose/src/bootstrap.rs`, and the `sqlx::migrate!(..).run(..)` call sites of the large stores
  (agent, mesh, replica, secrets, tasks, roster at least) through one shared helper.
- Shell: `crates/desktop/src/launch.rs`, `commands.rs`, `lib.rs`, capabilities.
- UI: `crates/desktop/ui/src/lib/tauri.ts`, `components/launch/**`, `App.tsx` launch wiring, `dev/launch-fixture.tsx`,
  their tests and snapshots.
