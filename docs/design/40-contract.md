# 40 — Contract: seams for the watchers build

Status: contract commit on `feature/40-watchers` (2026-09-28). Read this
before `40-triggers.md` §9. Everything here compiles and the existing tests
pass; nothing fires yet. Workers branch from this commit and stay inside
their column.

## Seam ownership

| worker | owns (may edit) | must not touch |
|---|---|---|
| W1 file | crates/schedule/src/file.rs (rules, set_created, tests), tools.rs arg → Spec mapping and error strings, `describe`/ScheduleInfo.when rendering | lib.rs tick, store.rs, agent/sensor |
| W2 sensor | crates/agent/src/sensor/**, assets/sensors/README.md + fs_exists.py + .shim.py, install_builtins, sensors version table (its own sqlite or a file — W2 decides, in agent's data dir), find_cpython | schedule crate, rpc, ui |
| W3 gate | crates/schedule/src/lib.rs (tick_when, gate, queue/skip/restart, strikes, expires, once, memory: continue, BindSession), store.rs (only new fns it needs beyond the contract) | file.rs rules, tools.rs, validate.rs, agent/sensor, ui |
| W4 surfaces | crates/schedule/src/validate.rs (new: dry-run twice, probe, Verdict), tools.rs dry-run call + reply text + schedule_status last check, rpc TestSchedule handler, crates/tui `/schedules test` + when column, crates/desktop/ui Settings › Schedules column + Test button | lib.rs tick internals (call BindSession/TestSchedule through the actor API only), file.rs rules, agent/sensor |
| W5 pr_watch | assets/sensors/pr_watch.py + tests under assets/sensors/tests/ (CPython test with a mocked tool bridge) | all Rust |

Every `// W1:` … `// W4:` comment in the tree marks a hook point for that
worker; grep for `W<n>:` to find yours.

## What exists (file:line as committed)

### crates/schedule/src/file.rs

| name | line | note |
|---|---|---|
| `pub const MIN_EVERY_WHEN: Duration` = 30 s | 16 | W1 enforces |
| `pub const DEFAULT_SENSOR_TIMEOUT: Duration` = 20 s | 18 | applied as the `timeout` default |
| `pub const MAX_SENSOR_TIMEOUT: Duration` = 60 s | 20 | W1 enforces |
| `pub struct Sensor { file: String, args: BTreeMap<String, String> }` | 24 | `Sensor::parse(&str)` :34 (structural: `.py` first token, `key=value` rest), `Sensor::render()` :59 |
| `pub enum Fire { OnChange, WhileTrue, Once }` | 74 | serde `snake_case`, `Default = OnChange`, `Display` |
| `pub enum Mode { Skip, Queue, Restart }` | 87 | serde `snake_case`, `Default = Skip`, `Display` |
| `pub enum Memory { Continue, Clear }` | 100 | serde `snake_case`, `Default = Continue`, `Display` |
| `Spec` gains `sensor: Option<Sensor>`, `fire`, `for_`, `cooldown`, `mode`, `memory`, `expires: Option<Duration>`, `timeout: Duration`, `created: Option<DateTime<Local>>` | 251–267 | defaults applied in `parse` |
| `FrontMatter` keys `when, fire, for, cooldown, mode, memory, expires, timeout, created` | 286–295 | `deny_unknown_fields` kept |
| `parse` watcher block | 335–451 | rules implemented: `when` requires `every`; exclusive with `cron` (`when: exclusive with cron`) / `at` (`at: a one-shot has no cadence to poll on`); `fire/for/cooldown/mode/memory/timeout` require `when` (`<key>: requires when`); enum wording `fire: expected on_change\|while_true\|once` etc.; `created: expected RFC 3339` |
| `FileArgs` / `ScheduleFile` gain the nine keys as `Option<&str>` / `Option<String>` (`for_` serde-renamed `for`) | 571–588, 627–645 | `render` writes them only when present (:768–795) |
| `// W1: set_created` hook | 802 | signature suggested there |

### crates/schedule/migrations/0003_watch.sql

`watch(name PK, state '{}', last_hash, last_fired_at, last_check_at, last_outcome, last_elapsed_ms, holding_since, strikes 0, fired_count 0, session_id)`; `ALTER TABLE runs ADD COLUMN envelope TEXT`.

### crates/schedule/src/store.rs (real sqlx, tested)

| name | line |
|---|---|
| `insert_run_with(name, session, fired_at, result, envelope: Option<&str>) -> i64` | 86 (`insert_run` delegates with `None`) |
| `history` now fills `RunInfo.envelope` | 177 |
| `watch_get(name) -> Option<WatchRow>` | 203 |
| `watch_put(&WatchRow)` | 216 |
| `watch_commit(name, state, hash, fired_at)` — upsert, `fired_count+1`, clears `holding_since`/`strikes` | 241 |
| `watch_strike(name) -> u32` — upsert, returns new count | 264 |
| `watch_reset_strikes(name)` | 276 |
| `watch_last_check(name, at, outcome, elapsed_ms)` — upsert | 286 |
| `watch_delete(name)` | 309 |
| `pub struct WatchRow { name, state, last_hash, last_fired_at, last_check_at, last_outcome, last_elapsed_ms: Option<u64>, holding_since, strikes: u32, fired_count: u32, session_id: Option<SessionId> }` | 320 |

### crates/agent/src/sensor/mod.rs (`pub mod sensor` in agent/lib.rs:43)

| name | line | status |
|---|---|---|
| `pub const DIR = "sensors"` | 22 | final |
| `pub struct SensorMeta { name, path, version, needs_cpython, hosts, has_probe }` | 26 | final |
| `pub struct SensorRun { fire, payload, state: serde_json::Value, next: Option<u64>, elapsed, truncated }` | 43 | final |
| `pub enum SensorError { NotFound, NeedsCpython, Header, Timeout, Budget, Malformed, StateTooLarge, Failed, Unavailable }` + `Display` | 62 | wording final: `when: no such sensor: <f>` · `when: needs cpython, none found` · `sensor header: …` · `timeout after Ns` · `over budget: …` · `malformed result: …` · `state too large: N KiB` · `Failed`/`Unavailable` verbatim. The gate prefixes `check broken: ` |
| `pub struct SensorProfile { max_memory 64 MiB, max_suspensions 8, max_total_sleep 0, max_fetch_bytes 1 MiB, max_payload 4 KiB, max_state 16 KiB, max_concurrent 4 }` + `Default` | 102 | final |
| `pub struct SensorRunner` | 136 | fields: `data_dir`, `pool: Option<Arc<monty_pool::Pool>>`, `tools: Option<ActorRef<tools::ToolRegistry>>`, `profile`, `slots: Arc<tokio::sync::Semaphore>` |
| `SensorRunner::new(data_dir, pool, tools, profile)` | 151 | final |
| `sensors_dir()`, `profile()`, `pool()`, `tools()`, `slots()` | 168–189 | final |
| `resolve(&self, file) -> Result<SensorMeta, SensorError>` | 196 | stub: exists → defaults; **W2** header parse |
| `async check(&self, meta, args, state, cwd, timeout) -> Result<SensorRun, SensorError>` | 215 | stub `Err(Unavailable("sensor runtime not built (W2)"))`; **W2** |
| `async probe(&self, meta, args, cwd, timeout) -> Result<Option<String>, SensorError>` | 235 | stub as above; **W2** |
| `install_builtins(&self) -> io::Result<Vec<String>>` | 251 | stub: mkdir, `[]`; **W2** |
| `pub fn find_cpython() -> Option<PathBuf>` | 261 | stub `None`; **W2** |
| `pub fn payload_hash(&str) -> String` (sha256 hex) | 267 | final, tested |
| `pub fn parse_envelope(&str) -> BTreeMap<String, String>` | 280 | final, tested |

`assets/sensors/README.md` — header + `check`/`probe` contract; `assets/sensors/tests/.gitkeep` tracks W5's dir.

### crates/schedule/src/lib.rs

| name | line | status |
|---|---|---|
| `pub struct TestSchedule { name }` → `Result<Verdict, String>` | 133 | handler :1483 answers `Err("W4: /schedules test not built")` after `no such schedule` / parse error / `not a watcher` |
| `pub struct BindSession { name }` → `Result<SessionId, String>` | 143 | handler :1499 answers `Err("W3: bind at creation not built")` after the same checks |
| `pub enum CheckOutcome { Broken(String), Quiet, Unchanged, Holding{held,need}, Cooldown{left}, Busy, Queued{coalesced}, Fired, Restarted }` + `Display` | 151 | strings: `check broken: …` · `quiet` · `unchanged` · `holding: 2m of 10m` · `cooldown: 3m left` · `skipped: busy` · `queued` / `queued (3 coalesced)` · `fired` · `restarted` |
| `pub struct LastCheck { at, outcome: String, elapsed_ms: u64, hash: Option<String> }` (TS-exported, `elapsed_ms` as TS `number`) | 206 | final |
| `pub struct Verdict { sensor, elapsed_ms, fire, hash, flaps, probe, would_fire, needs_approvals_off, error }` (TS-exported, `Default`, `elapsed_ms` as TS `number`) | 222 | final |
| `ScheduleInfo` gains `when_sensor: Option<String>`, `fire`, `mode`, `memory`, `expires: Option<String>`, `last_check: Option<LastCheck>` | 311–321 | filled by `info()` :1299–1358 from `Spec` and `store.watch_get` |
| `RunInfo.envelope: Option<String>` | 341 | filled by `store.history` |
| `async fn tick_when(sensors, store, spec, now) -> CheckOutcome` | 414 | stub → `Quiet`, `#[allow(dead_code)]`; **W3** wires it into `Tick` and gives it the real signature it needs (`&mut self` is fine — the stub is free-standing only so it compiles unused) |
| `Schedule.sensors: Arc<agent::sensor::SensorRunner>` | 538 | constructed in `SchedulePlugin::start` :1557 (`pool: None`, `tools: None`; `install_builtins()` called), passed to `Schedule::load` :571 and `tools::register` :1629 |

### crates/schedule/src/tools.rs

| name | line |
|---|---|
| `ScheduleArgs` gains `when, fire, for (for_), cooldown, mode, memory, expires, timeout: Option<String>` with schemars docs | 99–118 |
| `ScheduleTool { dir, sensors: Arc<SensorRunner> }` | 123 |
| `pub async fn validate(spec: &Spec, sensors: &SensorRunner) -> Result<Verdict, String>` stub → `Ok(Verdict{sensor, ..default})` | 134; called and ignored at :196–201 (`// W4: validate before write (40 §1.12)`) |
| `register(reg, dir, sensors)` | 41 |

### crates/rpc

`Api::test_schedule(name) -> RpcResult<schedule::Verdict>` (api.rs:1416, impl :4650 → actor `TestSchedule`). `export.rs` exports `LastCheck`, `Verdict`. `bindings/` and `crates/desktop/ui/src/rpc/bindings/` are gitignored: regenerate with `just ts-desktop` after checkout (the UI gate `bunx tsc --noEmit` needs the copy). `crates/desktop/ui/src/rpc/api.ts` declares `test_schedule` and lists it in `METHODS`.

## Decisions the design left open (or collided with the code)

1. **`Spec.when` was taken.** `Spec.when: When` is the clock clause (cron/every/at) since 34. The 40 field is `Spec.sensor: Option<Sensor>`; the front-matter key is still `when:`. Same on the wire: `ScheduleInfo.when: String` stays the clock text; the sensor line is `ScheduleInfo.when_sensor: Option<String>` (the name §6 uses). W1 renders `every 60s when pr_watch.py` into `ScheduleInfo.when` / `describe` for watchers; the TUI/desktop `when` column needs no new field for that.
2. **`Sensor.args` is a `BTreeMap`**, so `render()` sorts keys; the file keeps the user's order (the raw `when:` string is what `ScheduleFile.when` carries). If W1 wants order preserved in `Spec`, switch to `Vec<(String, String)>` — its call sites are `info()` and `validate`.
3. **`insert_run` kept its signature**; `insert_run_with(…, envelope)` is the watcher path (fewer touched callers than a fifth parameter on twenty call sites).
4. **`SensorRunner.tools` is `Option<ActorRef<ToolRegistry>>`**, `None` at construction: the registry is not up when the schedule plugin starts (the existing code looks it up in a spawned task). §4.3's bridge calls the tool structs directly anyway; W2 decides whether to keep the handle, look it up lazily, or drop the field.
5. **`SensorRunner` is built by the schedule plugin** (`SchedulePlugin::start`), not `main.rs`: `Schedule` has no `new`, and the plugin already owns the data dir. `first_run` in main.rs still owns nothing sensor-related; W2 may call `install_builtins` from either place — it runs at plugin start today.
6. **`watch_commit` also clears `holding_since` and `strikes`** (a committed fire is a successful check); `watch_strike` / `watch_last_check` upsert so W3 never has to `watch_put` first.
7. **`tick_when` stub is a free function** with `(&SensorRunner, &Store, &Spec, now)`; the design's `&mut self` signature is W3's to adopt when it moves into `impl Schedule`.
8. **`CheckOutcome::Holding` / `Cooldown` format whole seconds** through `humantime` (`2m 3s of 10m`); the doc's `2m of 10m` is that with seconds elided — W3 may round.
9. **`LastCheck` is read from the `watch` row on every `info()`** for watcher entries (one query per watcher per list); acceptable for the list sizes 34 expects.
10. **`validate` lives in tools.rs for now** so the stub compiles without a new module; W4 moves it to `validate.rs` and re-exports or calls it from there.
