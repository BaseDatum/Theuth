# 79 — As built: I (integration of the seven branches)

Branch `feature/79`, worktree `../theuth-79`, start `9f282800` (K). Merge order per brief I (S2 → C → H → E → B → A → local `main`); every merge is a `--no-ff` commit, every fix its own commit. No feature work. `cargo` at `/opt/homebrew/opt/rustup/bin`.

## Merge order (first-parent log, oldest first)

| sha | merge / fix | conflicts |
|---|---|---|
| `040a3b2f` | merge S2 `feature/79-S` @ `e2ec907f` | none |
| `137ea78e` | merge C `feature/79-C` @ `c3d3d90f` | none (`Cargo.lock` auto-merged) |
| `dc6b86c9` | merge H `feature/79-H` @ `8258f123` | none |
| `1720595f` | merge E `feature/79-E` @ `8c6dc9a5` | none |
| `5450e7de` | merge B `feature/79-B` @ `6a846f0b` (base `14801163`, merges clean over K) | none |
| `7f4c4feb` | fix: `deploy/vm/{Dockerfile,bootstrap.sh}` mask `serial-getty@hvc0.service` (S1 #8; B had not) | — |
| `26fd47da` | fix: E↔H `answer_secret` call shape (below) | — |
| `b4b45d75` | merge A `feature/79-A` @ `1e7a4065` (landed 01:40, ~50 min after my start) | `crates/sandbox/Cargo.toml`: S2's `microsandbox` line vs A's `rand`/`data-encoding` lines — kept both |
| `f7844ebd` | fix: A↔S2 seams (below) | — |
| `6a16493f` | merge local `main` @ `e69439bb` (six mobile-UI commits) | none |
| `157074aa` | fix: gate failures (below) | — |
| `c119c2ae` | docs: this file (draft) | — |
| `ca792df9` | fix: `deploy/vm` apt set + `libgbm1 libpipewire-0.3-0` (below) | — |

Merged worktrees' `target/` dirs removed in the background (`nohup rm -rf`) after `git branch --merged` listed each branch; worktrees themselves left for the orchestrator. B's `target` did not exist.

## Cross-seam fixes (each the smallest change; seam named)

1. **B ↔ S1 #8** (`7f4c4feb`): `systemctl mask serial-getty@hvc0.service` appended to the Dockerfile's `systemctl enable` RUN and after `enable --now` in `bootstrap.sh`. Verified in the rebuilt `theuth-vm:base`: `/etc/systemd/system/serial-getty@hvc0.service -> /dev/null`, `is-enabled` → `masked`; the four units `enabled`. `scripts/vm-units-check.sh` → `vm units in sync`.
2. **E ↔ H `answer_secret`** (`26fd47da`): H's RPC is positional `(id, value: Option<String>, values: Option<HashMap>)`; E sent a form as the *second* positional (`string | Record | null`), which jsonrpsee would reject as `Option<String>`. `rpc/api.ts` now `answer_secret(id, value: string | null, values?: Record<string,string> | null)`; `SecretRequestCard.tsx` sends `(id, value, null)` for one field, `(id, null, form)` for a form, `(id, null, null)` to decline. Only call site. (`tui` already passed three.)
3. **A ↔ S2 restore name** (`f7844ebd`): A alternated `lab` ↔ `lab~`; the SDK's name validator rejects `~` (S2). `actor::alternate` now yields `lab-r`; test asserts updated. Edge left open: a record named `<x>-r` would collide with the restore alternate of `<x>` (name check at `Create` covers records + peers, not alternates).
4. **`loopback_mesh` default → `false`** (`f7844ebd`, `types.rs`, `DEFAULT_CONFIG`, README): S1 #4b — a host→guest datagram ≥ 1500 B wedges the guest NIC and `crates/mesh` pins no QUIC `max_udp_payload_size` (grep: none). A's tests assert the loopback pairing path, so the test rig sets `cfg.vm.loopback_mesh = true` explicitly. Consequence for the smoke: pairing uses the relay path; the guest needs the relay reachable.
5. **`Blocker` doc** (`f7844ebd`): `not_built` dropped, `vm_disabled` (A's code) named. `api.rs` "answers `not built` until A lands" comment rewritten.
6. **`secrets::vault::writable_name`** (`157074aa`): A widened it so *every* `/` family accepted multi-segment names (`oauth/a/b`, `mesh/a/b`), failing H's `vault::tests::names`. Only `sandbox/<vm>/<key>` is multi-segment now; `oauth/`/`mesh/` stay one segment (A's as-built intent). Two asserts added for the sandbox family.
7. **UI fixture** (`157074aa`): `store/approvals.test.ts` `SecretRequested` literal gained H's `title: null, fields: []` (E branched before H's bindings).

## Cross-seam checks that needed no change

- **A ↔ S2 wiring**: `SandboxActor::new` calls `vm::runtime(&cfg.vm)` when `sandbox.vm.enabled`; `Err` is kept in the actor and surfaces as a `runtime_missing`/`vm_disabled` blocker. A canonicalises share paths (`validate_shares`), picks both host loopback ports itself (bind-and-release; S2's `resolve_ports` not needed), `Start` re-issues `set_mounts` + `set_ports` from the record when `pending.shares|ports` (survives an engine restart), `Snapshot { full: true }` → `Err` naming S1 #3, `init: Auto`, readiness = `systemctl is-active theuth xvnc desktop novnc`, auto snapshots pruned oldest-first (`sort_by_key(at_ms)`), `Destroy { keep_snapshots: false }` deletes every listed snapshot.
- **A ↔ C**: `EnsureVm` returns the record; bring-up's `verify` step waits for `Peer.reachable` in `join.status()` *before* publishing `Changed { Ready }`, so on the host engine the peer row exists when C's roster unblocks; the health tick (`health_interval_secs`, 60 s) publishes `Changed { Running }` again on the Ready→Running transition. No extra `MeshEvent` trigger added.
- **A ↔ H**: `sandbox_takeover` (A) and `desktop_handoff` (H) both registered; `handoffs_list`/`handoff_resolve` are H's roster asks; `grep 'not built\|not_built' crates/sandbox/src crates/rpc/src/api.rs` → only A's test strings.
- **A ↔ B**: A execs `vncpasswd -f > /home/engine/.vnc/passwd`, `systemctl restart xvnc`, `runuser -u engine`, `THEUTH_DATA_DIR=/data`, `/usr/local/bin/t`; the image ships `tigervnc-tools`, user `engine`, `runuser` (`/usr/sbin`, base image), those unit names and env. Image ref after `msb load` = docker tag (`theuth-vm:dev`).
- **E ↔ all**: `just ts-desktop` on the merged tree (8 export tests) → `bun run check` ok; `rpc/api.test.ts` (METHODS = `methods.json`) 2 passed.
- **Preflight surfacing** (smoke on `target/debug/t`, `--data-dir /tmp/79i`, `MSB_HOME=/tmp/nowhere`, `msb` removed from `PATH` — S2's `install::config` otherwise adopts an `msb` found on `PATH`, which is by design): `sandbox_preflight` → `runtime: missing`, `blockers: [{ code: runtime_missing, message: "The microsandbox runtime is not installed.", fix: "Install runtime (downloads ~30 MB from GitHub)", installable: true }]`, warning `docker_missing`; `sandbox_create {name: lab}` → `-32000 "The microsandbox runtime is not installed. — Install runtime (…)"`; `t mesh add --vm --name x` → `cannot create x:` + that sentence, exit 1; `t sandbox ls` → `no sandboxes`. Intel Mac / no-KVM covered by S2's unit tests `vm::preflight::tests::{intel_mac_is_unsupported, linux_kvm_and_glibc, …}` (present, pass).
- With `msb` on `PATH` and `MSB_HOME=/tmp/nowhere`, preflight reports `installed 0.7.4` (S2's PATH adoption) and `sandbox_create` proceeded to `creating{create}` against `ghcr.io/basedatum/theuth-vm:2.3.2` (unpublished) — killed and wiped; noted so the smoke worker uses the dev image.

## Gate (`/tmp/79/gate.sh`, detached; `/tmp/79/gate.log`)

| command | result |
|---|---|
| `bun install --frozen-lockfile` (packages/apps, crates/desktop/ui) | ok, no changes |
| `just ts-desktop` | ok, 8 export tests, 178 s |
| `cargo check --workspace --tests` | ok, 341 s (only the pre-existing `block v0.1.6` future-incompat note) |
| `cargo clippy -p sandbox -p rpc -p mesh -p roster -p tasks -p secrets -p theuth -p tui -- -D warnings` | ok, 97 s |
| `cargo build -p theuth` (+ `target/debug/monty` symlink) | ok, 3 m 39 s |
| `cargo test -p sandbox -p mesh -p roster -p tasks -p secrets -p rpc --lib` | mesh 23 · roster 48 · rpc 36 · sandbox 40 · secrets 22 (after fix 6; 21+1 fail before) · tasks (ran in `gate1`: `place` 11) |
| per-branch gates on the merged tree (`/tmp/79/gate1.log`, before A) | sandbox lib 26, `tests/vm.rs` 1 (+1 ignored), mesh 23, tasks place 11, roster 48, secrets 22 |
| `cd crates/desktop/ui && bun run check` | ok after fix 7 (1 error before) |
| `CI=1 bun run test < /dev/null` | 123 files, 688 passed (first run: `link/pairing.test.ts` "presence flips reconnect" failed under load; passed alone and in the full rerun — flake) |
| `bun run build` | ok, 13609 modules |

`sandbox_*` engine smoke above; `B` gate: `bash -n bootstrap.sh`, `just --list` (both `vm-image*` recipes), `scripts/vm-units-check.sh`.

## Guest `t` and the dev image

- `git push origin feature/79` (`skymoore/wth`) at `6a16493f` (= S2+C+H+E+B+A + local main; the two later fix commits touch the vault name check and a UI fixture only).
- VM `sky@192.168.8.65` (Ubuntu 26.04, glibc 2.43): the non-login ssh `PATH` has `/usr/bin/cargo` 1.93.1 (fails: `sqlx-sqlite`/`ty_*` need ≥ 1.96); `~/.cargo/bin` rustup + `rust-toolchain.toml` give 1.98.1 — re-run with `PATH=$HOME/.cargo/bin:$PATH`. Second failure: `rust-lld: unable to find library -lcap-ng` (the microsandbox SDK links libcap-ng; `libcap-ng0` present, no `-dev`, no passwordless sudo) — fixed without touching flags: `~/lib/libcap-ng.so -> /usr/lib/aarch64-linux-gnu/libcap-ng.so.0` + `LIBRARY_PATH=$HOME/lib` for the link. Built in ≈ 22 min total; `t 2.3.2`, 712 MB, `ldd` clean on the VM, no `libonnxruntime`. Copied to `/tmp/79/t-linux-aarch64-6a16493f`.
- **That binary cannot run in the guest**: `ldd` inside `theuth-vm:base` (Debian 12, glibc 2.36) → `GLIBC_2.38/2.39/2.43 not found`. The brief's fallback `/tmp/79/t-linux-aarch64-751b1be6` has the same three (also built on that VM). B's proof used a placeholder `t`, so nobody had hit it. Decision: build against bookworm's glibc in Docker on this Mac (`/tmp/79/deb-build.sh`: `rust:1-bookworm` arm64, rustc 1.98.1, apt dev set mirrored from the VM + `libcap-ng-dev`, `CARGO_BUILD_JOBS=6` for Docker's 8 GB, `CMAKE_POLICY_VERSION_MINIMUM=3.5`; volumes `theuth79-target`, `theuth79-cargo` — reusable for the next build). Cold build 16 m 04 s + apt; peak 6.8 GB. Result **`/tmp/79/t-linux-aarch64-6a16493f-bookworm`** (`t 2.3.2`, 712 MB, sha `6a16493f`; also at `deploy/vm/t`, git-ignored): `ldd` in the base image → only `libgbm.so.1`, `libpipewire-0.3.so.0` missing, no glibc complaint.
- Those two libs (`libgbm1`, `libpipewire-0.3-0`) added to the Dockerfile and `bootstrap.sh` apt sets (`ca792df9`). The Ubuntu-built binary is kept only as evidence; the release pipeline's `cli-linux-*` asset (B's `Dockerfile` final stage) must likewise be built on glibc ≤ 2.36 or the guest image must move to a newer base — open item.
- Image: `theuth-vm:base` rebuilt from the merged `deploy/vm` (mask + libs; `docker build --platform linux/arm64 --target base`, 27 s), `theuth-vm:dev` = base + the bookworm `t` (`Dockerfile.dev`'s `t --version` → `t 2.3.2`), 2.97 GB unpacked. `docker save theuth-vm:dev | ~/.microsandbox/bin/msb load` → **`theuth-vm:dev`** `sha256:593b8178dcfa`, 706.3 MiB in `msb images`. Create with `init: auto` only (B: no startup command); the SDK path A uses sets none.

## Deviations accepted (design/contract)

- `loopback_mesh` off by default (design decision 10 makes the shortcut optional; S1 #4b).
- Restore alternate `<name>-r`, not the design's `<name>~`.
- `Verdict::Blocked` not added (C: reused `Missing`); handoff messages on the `Attention` actor (H); `Computer.engine: Option<EngineId>` (A); `sandbox/<vm>/<key>` vault names via `set_if_version` only (A) — all as the workers' as-builts state; nothing re-decided here.
- `writable_name` for `oauth/`/`mesh/` kept single-segment (A's code had widened it unintentionally).

## Open items for the smoke worker

1. First real bring-up: nothing in 79 ran end to end against a live VM (A tested on `MockVmRuntime`, S2 on its spike). Expect `create` → `config` → `binary` → `first_run` → `daemon` → `pair` → `verify` in this order; `desktop.enabled` adds the noVNC GET in `verify`.
2. `image` default is `ghcr.io/basedatum/theuth-vm:<version>`, unpublished → create with the dev image (`{ kind: oci, ref: "theuth-vm:dev" }` or `--image theuth-vm:dev`), or `--bootstrap`.
3. Pairing uses the relay (`loopback_mesh: false`); set `sandbox.vm.loopback_mesh: true` to test the loopback path (S1 #4b caveat).
4. C's race (Ready before the peer row on a *replica*) is untested live; symptom = pinned task stays `sandbox:<name>` until the next health tick.
5. A: `Stop`'s busy-session check and `Takeover` see only host-owned sessions; `sandbox_retry` has no tool; the pop-out Terminal item is disabled (E → needs a `sandbox_terminal` RPC).
6. `SnapshotInfo` TS name collision (`export.rs`: sandbox vs documents) — E casts through `VmSnapshot`; a `#[ts(rename = "VmSnapshotInfo")]` would remove the cast (K/I follow-up, not done: changes a contract type's binding).
7. H: `secrets` learns its engine id via `SetEngine` from the roster; a `mesh`-side tell would be cleaner (C/I follow-up, not done).
8. Guest files land on the host as uid 501 / 0600 (S1 #9) — UI help text does not say so yet.
9. **glibc**: any Linux `t` for the guest must be built on glibc ≤ 2.36 (Debian 12) — the arm64 VM (Ubuntu 26.04) produces binaries the guest cannot load. `--local-t` / `--bootstrap` paths with a VM-built binary fail at the `binary` step with `GLIBC_2.43 not found`; use `/tmp/79/t-linux-aarch64-6a16493f-bookworm` (or rerun `/tmp/79/deb-build.sh`, warm volumes). Release fix: build `cli-linux-*` in a bookworm container, or rebase the image (B).
10. The `Dockerfile`'s `latest.json` release stage is still unexercised (no `cli-linux-*` asset exists; B).
11. Build-machine notes: the VM's `~/lib/libcap-ng.so` symlink and `LIBRARY_PATH` are needed for every `cargo build -p theuth` there until `libcap-ng-dev` is installed (sudo).
