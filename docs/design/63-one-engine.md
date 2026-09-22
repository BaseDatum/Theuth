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

## 6. As built (2026-09-23, branch `feature/one-engine`)

### 6.1 Step 1 — lock, record, refuse, desktop probe

- `rpc::engine` (`crates/rpc/src/engine.rs`): `Host`, `Record`, `Running`, `Lease`, `acquire`, `discover`, `alive`, plus `hold`/`release` — a process-wide slot so `main` drops the lease before `process::exit` (a `static`, because the TUI's takeover acquires from a tokio task `main` cannot reach). The lock is `std::fs::File::try_lock` (stable since 1.89; flock on unix, `LockFileEx` on Windows) — the same semantics `fd-lock` would give, no new dependency. `Lease::drop` unlinks `engine.json`; the kernel releases the lock with the fd.
- `Record.listen` is `Option<String>`: a plain `t` (no `--serve`) holds the lock but serves nothing, so nothing can attach to it. `Running`'s Display is the §0.5 line; with no socket it reads `… no socket) — start it with \`t --serve\` to attach, …`. `Host` gained `Signal` (`--link-signal` takes the lock too: it writes config and the Signal store).
- Ordering differs from §2.2: every mode tries `acquire` first and only reads the record when the lock is held. A stale record can never cause a false refusal (a reused pid could fool `discover`); `discover` is for the two callers that cannot hold the lock — the desktop probe and `t --link`, which now finds the desktop's ephemeral port through the record.
- `t --headless` spawned by the desktop app records `host: desktop` (it is the only headless run with `THEUTH_LAUNCH=1`, 18 §4).
- Desktop (`crates/desktop/src/launch.rs`): `probe()` reads `engine.json` itself — `pid` (alive via `nix` signal 0) and `listen` — then falls back to the configured `rpc.listen` for records written by older engines. The desktop crate does not (and must not, for build size) link `rpc`; the two-field read is the whole contract it needs. `Connection.owned` unchanged. A second daemon the app spawns against a live engine exits 3; the transcript shows the §0.5 line.
- **§2.5 jobs ownership: not built — nothing to build on.** On this base job rows live only in the `Jobs` actor's memory (`crates/tools/src/jobs.rs`, no jobs table, nothing persisted); a new engine cannot see, let alone signal, another engine's jobs. §1's collateral kill cannot recur because the second engine never starts (§4.5). `engine_id`/`lost` return with a persisted jobs store, if one ever exists.
- Windows: `alive` uses `windows-sys` (`OpenProcess` + `GetExitCodeProcess`) behind `cfg(windows)`; type-checked for `x86_64-pc-windows-msvc` in isolation (6.3), not run. `// ponytail:` note points at `platform::process::alive` (58).

### 6.2 Step 2 — client mode, takeover, `-p`/`--resume`/`--acp`

- **Facade = jsonrpsee's `ClientT`, not a hand-written trait.** `api::Api` is now `#[rpc(server, client)]`; the generated `ApiClient` gives every method on any `ClientT`. `rpc::client::Local(Methods)` implements `ClientT` by dispatching through the in-process `Methods` (owner peer in its extensions), `rpc::client::Engine { Local, Remote(Arc<WsClient>) }` delegates; the TUI and ACP call `api.method(args)` on an `Engine` and never see which end it is. Cost: an in-process call now round-trips through JSON (the same encoding the wire uses; `Methods::raw_json_request`), so the compile proves every reply type deserialises — it did without a single derive added. The 120-method Local/Remote impl pair of §2.3 does not exist.
- **Events: `rpc::client::mirror`.** `subscribe_events(null, null)` on the socket; each JSON event is decoded as the first bus type whose `type` tag fits (all 28 types the rpc bridge forwards, tags unique across the stream; most frequent first) and **published on the local bus**. The TUI's `Bridge` actor, the ACP `Relay` and every other bus subscriber run unchanged in client mode. No "RemoteBridge" mapping to `UiEvent`.
- **UiEvents without a wire form: none.** All 20 bus types the TUI subscribes to are `#[serde(tag = "type")]` and forwarded by the bridge. What an attached TUI does not get: (a) `{"type":"Lagged"}` — dropped at debug level; (b) events of another peer's private tree (51 §4.2 filters them server-side; the in-process TUI is the owner of everything, an attached one is a second peer) — those sessions are invisible, as designed.
- **Startup**: `main` tries `acquire`; on `Running` with a socket, TUI/`-p`/`--resume` run `TuiPlugin { attached: Some(Attached { client, running, start_engine }) }` and `--acp` runs `AcpPlugin { remote: Some(client) }` on a bare `Runtime` (no engine plugins). Status line suffix `attached · pid N · <host>` (`UiEvent::Engine`, `Model::attached`); in `-p` the label goes to stderr so stdout stays the reply. `Running` without a socket (a plain `t` holds the lock) refuses with exit 3 — nothing to attach to.
- **Takeover** (`tui::supervise`): `mirror` returns when the socket closes → `engine gone — starting one…` → `acquire(Host::Tui, Some(rpc.listen))` → `start_engine` (the plugin list `main` factored into `engine_plugins(headless=false, serve=true)` + `start_plugins`) → the calls' `Engine` is swapped to `Local` → `UiEvent::Engine(None)`; the UI reloads the shown session and its subtree from the new engine, the note `engine started here (pid N)` rides the reload like undo's (`Model::notice`). The taken-over engine serves RPC on the configured `rpc.listen`, so the app re-attaches (§4.2). Lost race → connect to the winner (15 s of 200 ms retries), swap to `Remote`, status line updates; three rounds, then a plain error.
- **`--acp`**: `rpc::acp` did *not* sit on the `Api` (it `ask`ed actors directly); it now calls `ApiClient` (`create_session{cwd}` for `session/new`, `load_session`, `list_sessions`, `send_input`, `cancel`, `session_cost`, `resolve_approval`) on the relay's `Engine`. Attached ACP has no takeover: the socket closing ends the process (the editor restarts it, and it attaches or becomes the engine).
- **Behaviour with no other engine**: same plugin list, same order; the TUI's calls go through `Local` (JSON round trip) instead of `&Server` + `Extensions` — replies are the same serde types, errors carry the same message. `t --link` prefers the record's `listen`.
- Not done / known: a `-p` whose engine dies mid-turn reloads the session after takeover but the dead turn is not re-sent; `rpc.listen: 127.0.0.1:0` is recorded literally (the record cannot learn the bound port before `Runtime::start`); the desktop app still cannot attach to a plain `t` (no socket) — `t --serve` is the attachable terminal engine, as 18 §3 always said.

### 6.3 Measured (debug build, M3 Max, `--data-dir /tmp/e63`, `rpc.listen 127.0.0.1:7863`, key_store file)

| § | Check | Result |
|---|---|---|
| 4.3 | second `t --headless` while one runs | exit 3, §0.5 line, `real 0.01 s`; `engine.json` byte-identical before/after |
| 4.4 | `kill -9` the engine, start another | stale record overwritten (new pid/id); clean `SIGINT` exit unlinks it |
| 4.1 | `t --resume <s>` against a headless engine | status `attached · pid 63910 · headless` at 2.24 s / 2.35 s (two runs); cold `t` first status paint 2.35 s — the debug binary's own start dominates, not the engine (release not measured) |
| 4.1 | `/jobs` attached | `j1  running  13s  pty smoke: sleep 600` — the PTY the headless engine's model had started |
| 4.2 | `kill -9` the headless engine under the attached TUI | `engine gone — starting one…` 0.1 s after the kill; `engine started here (pid 66746)` 0.11 s later; `engine.json` → `host: tui, listen 127.0.0.1:7863`; `t -p 'Reply with exactly: hi'` then answered `hi` with stderr `attached · pid 66746 · tui`; `pgrep -f 't --headless'` = 0 |
| — | `t -p` against a headless engine | `hi`, `real 1.60 s` total (model call included), stderr `attached · pid 63016 · headless`; one `t --headless` process |
| 4.5 | jobs | n/a on this base (6.1); the killed engine's PTY child went with its pty master, nothing signalled it |
| 4.6 | Windows target | `cargo check -p rpc --target x86_64-pc-windows-msvc` stops at `ring`/`aws-lc-sys`/`zstd-sys` build scripts (no Windows C toolchain here) before reaching any Rust; `engine.rs` alone in a scratch crate (chrono, serde, uuid, windows-sys) checks clean for that target — the `alive` Win32 path compiles |
