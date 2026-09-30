# 79 — As built: S2 (`VmRuntime` for microsandbox: runtime, install, preflight)

Branch `feature/79-S`, base `9f282800` (K), worktree `../theuth-79-S`. Owned files only:
`crates/sandbox/src/vm/{install,preflight,microsandbox}.rs`, the body of `vm::runtime()`,
`examples/vm_spike.rs`, `tests/vm.rs`, the `microsandbox` dependency lines, `Cargo.lock`.
Built on S1's measurements (`/tmp/79/S1-findings.md`, `/tmp/msb-spike-79/REPORT.md`); nothing
S1 settled was re-measured.

## Built, per commit

1. `1230aa51 build(sandbox)` — `[workspace.dependencies] microsandbox = { version = "=0.7.4", default-features = false, features = ["local", "net"] }`, `crates/sandbox/Cargo.toml` `microsandbox = { workspace = true }`. No build-time download (the SDK's `build.rs` is a no-op without `download-binaries`/`embed-binaries`). `libsqlite3-sys` unified on the vendored 0.36 as S1 predicted; `msb.db` opens fine through it (every spike step below ran on it). `cargo check -p sandbox` cold: 7 m 23 s under load (SDK + sea-orm 2 subtree).
2. `e2d86e7f feat(sandbox)` — `vm/install.rs`: `config(home) -> GlobalConfig` (`""` = `MSB_HOME`/`~/.microsandbox`; when the home has no runtime and an `msb` sits on `PATH` with `../lib/libkrunfw.5.dylib|.so.5` beside it, that pair is passed as `config.paths` — the SDK itself never searches `PATH`), `resolve(&cfg) -> RuntimeState` (`Installed { version, home, libkrunfw }` via `setup::resolve_runtime_version`, `RuntimeNotInstalled → Missing`, anything else → `Incomplete { why }`), `is_installed`, `install(&cfg, out)` (idempotent; `force: true` on `Incomplete`; download failure names the URL plus the brew and curl routes).
3. `00e6a028 feat(sandbox)` — `vm/preflight.rs`: `Facts` (host, runtime, kvm present/rw, glibc, docker on PATH) → `probe(cfg)` (std + `sysctl -n hw.memsize` / `/proc/meminfo`, `df -Pk <home>`, `ldd --version`; no new crate — `crates/platform` has no host-facts helper) and `evaluate(&facts, cfg) -> Preflight` with the brief's codes and sentences; `defaults = Resources::derive`. `disk_low` when `free < root_disk + 2 GiB image + 2 GiB`; `memory_low` and `docker_missing` are warnings. Five unit tests with injected facts.
4. `672792b8 feat(sandbox)` — `vm/microsandbox.rs`: `Microsandbox::new(cfg)` builds `LocalBackend::builder().home(..).build_lazy()` and `set_default_backend` once (`std::sync::Once`); the full `impl VmRuntime` (details below); `vm::runtime(cfg)` returns it for `runtime == "microsandbox"` (installed or not), `Err` otherwise.
5. `17207dd4 test(sandbox)` — `examples/vm_spike.rs`, `tests/vm.rs` (`runtime_builds_and_preflight_answers` not ignored; `vm_round_trip` ignored).
6. `fix(sandbox)` (last commit) — found by running: handle-level `pause`/`resume` (the agent `connect()` refuses a Paused sandbox), `destroy` resumes a paused VM first (the SDK refuses "still running"), `resize` refuses a smaller root disk itself (the SDK silently ignores it), `create` waits ≤ 15 s for `/proc/1/comm == systemd` (the agent is ready ~1 s before `init.krun` hands off), `delete_snapshot` moves the group head (`Snapshot::group_head("<vm>:<other>")`) before removing a head snapshot.

## `impl VmRuntime for Microsandbox` — how each method is done

- `create`: adopt by name when `Sandbox::get` finds one carrying `io.theuth.sandbox=<name>` with the same image reference (started if stopped); a different image → `destroy` + create; a same-named sandbox **without our label → error** (never destroys a user's own sandbox). Builder = `image_with(oci + root_disk).cpus/max_cpus/memory/max_memory.hostname.label(LABEL,name).labels.env.ephemeral.idle_timeout.max_duration.init("auto").patch(files).network(policy).volume(canonicalised host).port_bind/port_udp_bind.detached(true).create_with_progress()`; every `CreationProgress` event → `out.say("create", …)`. Pre-boot files use the SDK's rootfs patch API (`PatchBuilder::file(path, bytes, Some(mode), replace)`), baked into the upper before first boot — verified (`/etc/theuth-preboot` present at +21 ms). Ports with `host == 0` are resolved by `resolve_ports` (bind-and-release) and retried up to 3× on `Address already in use`. A failed boot removes the stopped record the SDK leaves behind.
- `status`: `Sandbox::get` → `Created|Stopped → Stopped`, `Starting|Running|Draining → Running`, `Paused`, `Crashed`; `SandboxNotFound → Gone`.
- `start`: `handle.start_detached()`, unless `set_mounts`/`set_ports` recorded something for this name → [`recreate`]: stop if needed → `handle.snapshot("auto-reconfigure-<ts>")` (disk) → `Sandbox::remove` → `SandboxBuilder::from(persisted config with mounts/ports/patches cleared).volume(..).port_*(..).override_snapshot("<vm>:<label>").detached(true)` → boot. Disk, systemd, hostname, labels, cpus/memory survive (spike: marker kept, both mounts, PID 1 systemd, 4 vCPU, 12 GiB). The helper snapshot stays with `auto: true` for the actor's pruning.
- `stop(grace)`: `stop_with_timeout(grace)`; `StopTimeout → kill()`.
- `pause`/`resume`: `SandboxHandle::pause/resume` (host control pipe).
- `destroy`: `handle.destroy()` (works on running; a paused VM is resumed first); `SandboxNotFound → Ok`.
- `exec`: `connect()` → `shell_with(line, timeout, stdin_bytes|stdin_null)`; a paused VM fails with the SDK's "not running (status: Paused)".
- `put`: `shell("mkdir -p <dir>")` → `fs().write(<path>.theuth-tmp)` → `fs().set_stat(mode)` → `fs().rename` (atomic replace; `fs().write` has no mode parameter).
- `resize`: shrink of `root_disk_mib` refused up front (current size from the config, or from `upper_used+free` rounded up to 256 MiB for snapshot-born disks). Then `modify().dry_run()`; changes with `disposition: Live` → `apply()` now, `NextStart|RequiresRestart` → `.next_start().apply()`, `Unsupported` → `Err` with the SDK's reason. `status` from `resize_status` (`GuestRefused|Failed → GuestRefused`), then the guest is polled (`nproc`, `MemTotal` within −8 %/+2 %) every 500 ms up to `resize_settle_secs` → `Applied`. `restart_required = next_start != default`.
- `set_mounts`/`set_ports`: recorded in a process-local map; `Pending { restart_required: status != Stopped }`. `set_ports` resolves `host == 0` at record time.
- `metrics`: `handle.metrics()` 1:1; `memory_bytes` falls back to `limit − available` when the SDK reports 0 (right after a resize).
- `logs(tail)`: SDK entries (exec stdout/stderr, runtime `system`) + `journalctl -n <tail> -o short-unix` via exec when systemd is PID 1 and the VM runs; merged by time, last `tail`.
- `snapshot`: `full && init auto → Err` (the sentence from S1); disk on a running VM = `exec("sync")` + `Snapshot::builder(label).from_sandbox(vm).guest_flush(GuestFlush::Skip)`; on a stopped VM the default flush. Id = `"<vm>:<label>"`.
- `restore(id, as_name, spec)`: `as_name` must be free; the full `spec` (renamed, `files` cleared — the disk has them) is booted with `override_snapshot(id)`; ports re-resolved. Never `Sandbox::restore` (drops `init`, resources, ports, mounts — S1).
- `snapshots(h)`: `Snapshot::list()` filtered by `group() == vm` or `name()` prefixed `"<vm>:"`; `auto = label starts with "auto-"`; `bytes = size_bytes()` (virtual size).
- `delete_snapshot`: `Snapshot::remove(id, force = true)`; the "cannot remove current head" refusal surfaces as the error (A: delete oldest-first).
- `list`: `Sandbox::list_with(limit 100, cursor)` pages, kept when the config's labels contain `io.theuth.sandbox` (the SDK's label filter is exact key+value, so it cannot do "has key").

## Measured on this Mac (`cargo run -p sandbox --example vm_spike`, final run, layers cached, 2026-09-30)

| step | ms | outcome |
|---|---:|---|
| preflight | 20 | Installed 0.7.4, home `~/.microsandbox`, virt Ok(hvf), no blockers, no warnings |
| create (TCP 6080 + UDP 7800 published, one bind mount, one pre-boot file) | 487 | Running, `runtime_id: local:31`; systemd PID 1 awaited |
| exec facts | 11 | PID 1 `systemd` · 2 vCPU · 1985 MiB · `/` 7991 MiB · pre-boot file present · host file visible in `/shared/a` |
| put + exec | 18 | `put-ok`, mode 755 |
| resize live cpus 2→4, memory 2048→3072 | 536 | `Applied`, `restart_required=false`, guest `nproc 4`, MemTotal 3009 MiB |
| disk grow 8→12 GiB | 153 | `Applied`, `df /` 12022 MiB immediately |
| resize max_memory 4→8 GiB | 4 | `next_start.max_memory_mib=8192`, `restart_required=true` |
| metrics | 1 | cpu 0 %, mem 91/3072 MiB (`memory_bytes` fallback path) |
| logs tail 5 | 19 | 5 lines (exec stdout) |
| stop (grace 30 s) | 512 | Stopped |
| set_mounts on stopped (+ `/shared/b` ro) | 0 | `Pending { restart_required: false }` |
| start = recreate with mounts | 397 | PID 1 systemd · marker kept · `/shared/a` + `/shared/b` · 4 vCPU · 12022 MiB · pre-boot file kept |
| UDP echo through the published port | 12043 | ok, mean RTT 0.25 ms (first run 0.13 ms; host `127.0.0.1:54931` → guest 7800); 12 s = `apt-get install python3` for the echo server (image has no socat) |
| snapshot disk, running | 423 | `spike-s2:clean` (virtual 12288 MiB, 359 ms in the SDK) |
| snapshot full | 1 | refused with the systemd/freezer sentence |
| restore as `spike-s2-r` | 332 | PID 1 systemd · hostname `spike-s2-r` · marker kept · 2 vCPU (the restore spec's) · `/shared/a` |
| snapshots + list | 2 | `["clean", "auto-reconfigure-…(auto)"]`, list `["spike-s2-r", "spike-s2"]` |
| pause / resume | 7 | `Paused → Running` |
| destroy both + snapshots | 964 | ok; `msb ls` / `msb snapshot ls` empty afterwards |
| total | 15972 | |

First run (before the fixes in commit 6): identical numbers within noise except `exec facts` showed PID 1 `init` at +21 ms and `resume` failed with `sandbox 'spike-s2' is not running (status: Paused)` from `connect()`; the failing cleanup then left a paused `spike-s2` that `destroy` refused ("still running") — both fixed and re-verified.

## Contract deviations (I reconciles with A)

- **`Port.host == 0` is not read back from `create`** — `VmHandle { name, runtime_id }` has no room and the SDK never reports the port it binds (S1 #1). `vm::microsandbox::resolve_ports(&[Port]) -> Vec<Port>` (and `free_port(udp)`) is `pub`: **A resolves the ports before `create` and persists them in the record**; `create` still tolerates `0` (picks internally, logs the picks) but the record would then not know them. `set_ports` resolves at record time for the same reason.
- **`restore` name `<name>~`** (design §5.6) is rejected by the SDK's name validator (`[A-Za-z0-9._-]`, alphanumeric first). The spike uses `spike-s2-r`; A picks a legal suffix.
- **`Pending` is process-local.** After an engine restart, A must re-issue `set_mounts`/`set_ports` from its record before `start`; otherwise `start` boots the old mount/port set (the SDK config keeps it).
- **`recreate` leaves an `auto-reconfigure-<unix>` disk snapshot** per mounts/ports change (SnapshotInfo.auto = true). Deleting it while it is the group head with newer members is refused by the SDK — prune oldest-first.
- **`create` waits for systemd** (≤ 15 s poll of `/proc/1/comm`) with `init: auto`; dbus may still be a second away — bring-up should `systemctl is-active <unit>` rather than `is-system-running` (S1 #8: `starting` lasts minutes on `debian-systemd:12` until `serial-getty@hvc0` is masked).
- **Full snapshots** return `Err` for `init: auto` guests (S1 #3); the UI hides the option.
- `VmRuntime::create` on a same-named sandbox **without** our label errors instead of replacing it.
- `Preflight` runs `probe` in `spawn_blocking` (two child processes + `df`); `HostFacts.disk_free_bytes` is for the `MSB_HOME` volume (or `$HOME` when the home does not exist yet).
- **Snapshot groups outlive the VM.** `destroy` keeps `snapshots/<name>/` (by contract); a *new* VM with the same name joins that group, and `snapshot(h, "clean")` then fails with `snapshot name 'clean' conflicts between snap_… and snap_…` (seen between test runs). A: with `keep_snapshots: false` delete every snapshot of the name (head last — `delete_snapshot` handles the head move), and treat a label clash as "pick another label".

## Not built

`VmBackend: Sandbox`, `Target::Vm`, the actor/record/bring-up wiring (A); the image and its `serial-getty` mask (B/I); the `sandbox.vm.loopback_mesh` flag and the QUIC datagram cap (A/I; S1 #4 — a host→guest UDP datagram ≥ 1500 B still wedges the NIC, nothing in this runtime prevents it); Windows/Intel paths beyond the blockers; a preflight cache file (`runtime.json`, §4.8).

## Tests

`vm::preflight::tests::{intel_mac_is_unsupported, apple_silicon_and_linux_ok, linux_kvm_and_glibc, runtime_states_are_installable, low_disk_blocks_and_low_memory_warns}`; `tests/vm.rs::runtime_builds_and_preflight_answers` (not ignored: `runtime()` builds, `preflight()` answers, unknown runtime errs); `tests/vm.rs::vm_round_trip` (`#[ignore]`: create/adopt/put/resize incl. shrink refusal/metrics/logs/stop/set_mounts/start/snapshot/full refusal/list/pause/resume/destroy/delete snapshots).

## Gate

| command | result |
|---|---|
| `cargo check -p sandbox --tests --examples` | ok |
| `cargo clippy -p sandbox --tests --examples -- -D warnings` | ok |
| `cargo test -p sandbox --lib vm` | 12 passed (5 new + K's 7) |
| `cargo test -p sandbox --test vm -- --ignored` | `vm_round_trip` ok (2.3 s on a warm store) — after three failing iterations that each fixed a real bug: `connect()` on Paused, the disk shrink the SDK ignores, head-snapshot deletion |
| `cargo run -p sandbox --example vm_spike` | table above, exit 0 |

`cargo` is at `/opt/homebrew/opt/rustup/bin`. No `spike-*` sandbox or snapshot left in `~/.microsandbox`; the runtime stays installed.

## Edits outside S2's files

None. (`Cargo.lock` and the two manifests are in the brief.)

## Follow-ups for the integrator

- A: call `vm::microsandbox::resolve_ports` before `create` and store the result; re-issue `set_mounts`/`set_ports` after a restart when the record has pending changes; prune `auto-reconfigure-*` snapshots oldest-first; `SnapshotInfo.bytes` is the virtual size (label it so in the UI or `du` the snapshot dir).
- A: readiness = `systemctl is-active theuth.service` polling, not `is-system-running`.
- B/I: `systemctl mask serial-getty@hvc0.service` in the image (S1 #8); add `socat` or keep python3 if the spike's UDP echo should stay cheap.
- I: drop `not_built` from the `Blocker` doc in `vm/mod.rs` when K's stub actor goes (K's follow-up; the line is in A's/K's file, untouched here).
- I: the `libsqlite3-sys` unification worked on this Mac (`msb.db` under the SQLCipher build); Linux CI should run `tests/vm.rs::runtime_builds_and_preflight_answers` once.
