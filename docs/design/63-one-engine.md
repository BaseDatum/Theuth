# 63 — One engine per data dir: `t` attaches to a running engine instead of starting a second one

Status: design, 2026-09-22. Extends 18 (desktop launch: probe/spawn the daemon, §3 "a live daemon is reused"), 25 §5 (the terminal and the app share `~/.theuth`), 12 (tools are per engine), 61 (PTY jobs), 47 §4.5 (input origin), 58 (Windows/Linux hosts have the same daemon question). Amends 18 §3 (the probe reads the engine record, not `rpc.listen`).

## 0. Decisions in one screen

1. **One engine per data dir.** Every process that would run the harness core (`t` TUI, `t --headless`, `t --serve`, `t --acp`, `t -p`, `t --resume`, the desktop app's daemon) first takes an advisory lock on `<data_dir>/engine.lock`. Holding it means "I am the engine"; not getting it means "an engine exists — attach or refuse". Never two engines on one store.
2. **The engine record.** The lock holder writes `<data_dir>/engine.json` `{ "pid", "listen", "started", "host": "tui"|"headless"|"desktop"|"acp", "version" }` and removes it on clean exit. The desktop app's daemon (today on an ephemeral port nobody else can find) becomes discoverable through it. The token stays `<data_dir>/token`, unchanged.
3. **`t` attaches by default.** If an engine is alive, `t` runs the TUI as a JSON-RPC client of it: the same `rpc::api::Api` calls the TUI already makes in-process, over the WebSocket, with events from `subscribe_events` mapped onto the same `UiEvent`s. Jobs, PTYs, the Python runtime, MCP servers, approvals and asks are then shared with the desktop app — two views of one engine. The TUI status line says `attached · pid N · desktop`.
4. **Engine gone → the TUI takes over.** When an attached TUI loses the socket (the app quit), it takes the lock and starts the engine itself, reopening the same session; the desktop app's probe finds it next time. Symmetric: the app attaches to a TUI-started engine (`owned: false`, as 18 §3 already provides).
5. **Refuse, never race.** Modes that cannot attach (`--headless`, `--serve`, `--install-service` running, `--link-signal`) exit 3 with `engine already running (pid N, tui, since HH:MM, ws://127.0.0.1:P) — run \`t\` to attach, or set THEUTH_DATA_DIR for a separate store`. No `--force`; a stale record is not a running engine (§2.3).
6. **Jobs are owned.** A job row records the engine instance that started it (`engine_id` = lock-holder start uuid). An engine only ever signals jobs it owns; rows from a previous instance are marked `lost` at startup (their pty/pipes died with that process) — killed only if the recorded pid is dead and the process still exists (re-parented children), never a live engine's. This is the direct fix for today's incident (§1).
7. **Cross-platform from day one.** The lock is `fd-lock` (flock / LockFileEx); the pid check is `platform::process::alive(pid)` once 58 step 1 lands, `libc::kill(pid, 0)` behind `cfg(unix)` until then. Windows gets the same behaviour with `t.exe`.
8. **No new surface.** No new RPC methods; one new file pair in the data dir; two new CLI outcomes (attached; refused). Settings untouched.

## 1. Why now (2026-09-22, 22:23)

With the desktop app's engine running (`/Applications/theuth.app … t --serve` on an ephemeral port, the orchestrator session with two ssh PTYs and an omp PTY inside it), `t` was started in a terminal. It found no lock, started a second full engine on the same `~/.theuth` (all plugins, all MCP servers, its own Signal provisioning loop, its own browser), loaded the same session, and ran the user's turn there. Observed:

- **Latency** — a full engine start plus loading a ~430 k-token session before the first token.
- **Missing state** — the turn saw the transcript and tools but none of the PTY jobs or the runtime, which are process-local to the first engine.
- **Collateral kill** — the omp PTY in the first engine died with SIGTERM (exit 143) while its two `bash` siblings survived (interactive bash ignores SIGTERM): the second engine signalled job pids it had read from the shared store but did not own.
- **Double writer** — both engines wrote the session (title regenerated at 22:31 by the second), the log, and the jobs table.

18 §3 already says "a live daemon is reused, not duplicated" — but only the desktop app probes, it probes the *configured* `rpc.listen` while its own daemon listens on an ephemeral port, and the TUI never probes at all.

## 2. Shape

### 2.1 `rpc::engine` (new module, ~150 lines)

```
pub struct Record { pid: u32, listen: String, started: DateTime<Utc>, host: Host, version: String, id: Uuid }
pub enum Host { Tui, Headless, Desktop, Acp, Prompt }
pub struct Lease { _lock: fd_lock::RwLockWriteGuard<File>, record: Record }  // Drop = unlink engine.json

pub fn acquire(data_dir, host, listen) -> Result<Lease, Running>   // try_write on engine.lock; write engine.json
pub fn discover(data_dir) -> Option<Running>                         // read engine.json; pid alive; TCP connect ≤ 500 ms
pub struct Running { record: Record, url: String /* ws://listen/?token= */ }
```

`acquire` before `Runtime::start`; `listen` is known then because the rpc plugin's config is read from the same `Config` (the desktop passes `THEUTH_RPC__LISTEN`). `discover` requires all three (file, live pid, open port); anything less is stale and `acquire` proceeds (the flock is what actually arbitrates; the record is for humans and clients).

### 2.2 `t` startup (crates/theuth/src/main.rs)

```
match (mode, engine::discover(&data_dir)) {
  (Tui | Prompt | Resume | Acp, Some(running)) => attach(running)      // §2.3
  (_, Some(running))                            => refuse(running)      // exit 3, message §0.5
  (_, None)                                     => acquire(); start engine as today
}
```

### 2.3 TUI client mode (crates/tui)

The TUI is already structured for this (lib.rs header: "every command is an in-process call on `rpc::api::Server`, the same `rpc::api::Api` the socket exposes"; a `Bridge` actor turns bus events into `UiEvent`s). Client mode swaps the two ends:

- **Commands**: `run_commands` takes `Arc<dyn ApiFacade>`; the in-process `Server` and a jsonrpsee `WsClient` (`ApiClient`, generated from the same `#[rpc]` trait, `with_extensions` methods called with no extensions — the server fills `RpcPeer` from the authenticated connection) both implement it. Bearer token from `<data_dir>/token` on the upgrade; 127.0.0.1 only.
- **Events**: `subscribe_events` yields the JSON the rpc bridge already serialises for every wire-visible event type; a `RemoteBridge` deserialises each back into the typed event (same serde types; the bridge tags the type — reuse `rpc::bridge`'s tagging, add it if it's positional) and calls the same `UiEvent` mapping the bus bridge uses. Events without a wire form (none of the 25 the TUI subscribes to are in that set; verify) are noted in the as-built.
- **PTY jobs in the TUI**: `subscribe_pty`/`send` already exist on the API; the TUI's `/jobs` view works unchanged.
- **Disconnect**: on socket close, the TUI shows `engine gone — starting one…`, calls `engine::acquire`, starts the runtime in-process (the existing path) and re-selects the current session. If `acquire` fails because another client won the race, it re-attaches to the winner. Bounded: 3 attempts then a plain error.
- **`t -p` / `--resume` / `--acp`**: same facade, no UI changes. ACP (`rpc::acp`) already sits on the `Api`; attaching means an editor and the desktop app share one engine.

### 2.4 Desktop (crates/desktop/src/launch.rs)

`probe()` calls `rpc::engine::discover(data_dir)` first and connects to *that* `listen` (attaching to a TUI-started engine); falls back to spawning as today. The spawned daemon takes the lock itself (§2.1) — the app no longer needs `configured_listen` for discovery. `Connection.owned` semantics unchanged. When the app quits an owned daemon, attached TUIs fall back per §2.3.

### 2.5 Jobs ownership (crates/tools/src/jobs.rs)

Add `engine_id` to the persisted job record; at engine start, rows whose `engine_id` ≠ ours → state `lost` with a one-line `<job_result exit="lost">` injected to their session the next time it runs (like `Shutdown` in 61/`CancelOrigin`); signal only when `recorded_pid` is alive *and* its parent is gone (a true orphan). `jobs()` lists `lost` rows for one day.

## 3. Not in v1

- Attaching to an engine on another machine — that is `nodes` (32); this design is one data dir on one host.
- Multiple engines on one data dir by design (e.g. a read-only viewer) — refuse.
- Session-level locking inside the store; with one engine the actor model already serialises.
- A TUI "which engine am I on" switcher — `THEUTH_DATA_DIR` is the switch.

## 4. Verification

1. Desktop app running → `t` in a terminal: status line `attached · pid … · desktop`; `/jobs` lists the app's PTYs; a `start` from the TUI appears in the app; `time` to first paint < 1 s (no engine start).
2. Quit the app → TUI prints `engine gone — starting one…`, `engine.json` now says `host: tui`; relaunch the app → it attaches (`owned: false`), no second engine (`pgrep -f 't --serve'` = 0).
3. `t --headless` while an engine runs → exit 3 with the message; `engine.json` untouched.
4. Kill the engine with `kill -9` → `engine.json` stale; `t` starts an engine (flock was released by the kernel); the stale record is overwritten.
5. Jobs: start a PTY, `kill -9` the engine, start a new one → the job row is `lost`, its process (if any) untouched unless orphaned; no SIGTERM reaches live processes of another engine (the §1 scenario cannot recur because the second engine never starts).
6. Windows/Linux: `cargo check -p rpc -p tui -p theuth --target x86_64-pc-windows-msvc` for the pure-Rust parts; `fd-lock` on both.

## 5. Steps

| # | Scope | Size |
|---|---|---|
| 1 | `rpc::engine` (lock, record, discover), `t` refuse path, desktop probe reads the record, jobs `engine_id` + `lost` | S–M |
| 2 | TUI client mode (facade, remote bridge, disconnect takeover), `-p`/`--resume`/`--acp` over the facade, status line | M–L |

Step 1 alone removes the double-engine hazard (§1 cannot recur); step 2 delivers the attach experience.
