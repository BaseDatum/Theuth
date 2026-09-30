# 79 as built — F6: the microsandbox runtime embedded in `t`

Branch `fix/79-embed-runtime` (worktree `theuth-79-F6`, base `main` @ `0c50b8b9`). Sky's ruling 2026‑09‑30 13:05: "Yes, embed binaries." Timebox 2.5 h.

## 1. What `embed-binaries` does (read from `microsandbox-0.7.4/build.rs` and `lib/setup/runtime.rs`)

**Build time** (`build.rs:13-23, 106-152`). `main()` prints `rerun-if-env-changed` for `MSB_HOME`, `MSB_EMBED_ARTIFACTS_DIR`, `MSB_EMBED_RUNTIME_BUNDLE_PATH`; under `embed-binaries` it calls `stage_embedded_runtime()`, which must produce `$OUT_DIR/runtime-bundle.tar.gz`, in this order:
1. `MSB_EMBED_RUNTIME_BUNDLE_PATH` — a file copied verbatim (the release archive `microsandbox-<os>-<arch>.tar.gz` is exactly that format: `install_archive_bytes` at run time and `install_archive` at build time both consume it).
2. `MSB_EMBED_ARTIFACTS_DIR` — a directory holding `msb`/`msb.exe` + `libkrunfw.5.dylib` / `libkrunfw.so.5.6.1` / `libkrunfw.dll` (names per **target OS only**, `microsandbox_utils::{msb_binary_filename, libkrunfw_filename}`), tarred by `create_archive` (`:434-453`). Panics when the pair is missing.
3. The crate's own repo `build/` directory (`CARGO_MANIFEST_DIR/../../build`) — for the registry copy that is `~/.cargo/registry/src/build`, never present.
4. With `download-binaries` also on: `download_release_archive()` (`:185-211`) fetches `bundle_download_url(PREBUILT_VERSION, CARGO_CFG_TARGET_ARCH, CARGO_CFG_TARGET_OS)` = `https://github.com/superradcompany/microsandbox/releases/download/v0.7.4/microsandbox-{darwin|linux|windows}-{arch}.tar.gz` over `ureq`, and caches it in `target/<profile>/.microsandbox-runtime-cache/microsandbox-runtime-0.7.4-<os>-<arch>.tar.gz` (survives build‑script reruns, gone with `cargo clean`).
5. Without `download-binaries` and none of the above: `panic!("embed-binaries requires MSB_EMBED_RUNTIME_BUNDLE_PATH, MSB_EMBED_ARTIFACTS_DIR, a workspace build/ pair, or download-binaries")`. **So `embed-binaries` alone never builds from a clean checkout** — someone has to supply the archive.

Targets: release v0.7.4 has assets for `darwin-aarch64`, `linux-x86_64`, `linux-aarch64`, `windows-x86_64`, `windows-aarch64` (HEAD → 302 on all five, checked 2026‑09‑30); `darwin-x86_64` → 404 (theuth does not ship it). Every theuth release target is covered, so no per‑target gating was needed. Windows: `msb.exe` + `libkrunfw.dll`, `set_executable`/`create_library_links` are `not(unix)` no‑ops, `install_archive_bytes` has no unix‑only code — it compiles by cfg reading; not built here.

**Side effect of `download-binaries`** (`build.rs:25-104`): its `install_runtime_at_build_time()` runs on every build script run and *installs the pair into `MSB_HOME`/`~/.microsandbox`* when that home has no pair, or a pair of another version (it compares `msb --version` for host builds and **replaces** a foreign version via `install_pair`, `:378-432`). Sky's Mac holds 0.7.4 → no‑op. A developer running their own `msb 0.8` gets it downgraded by `cargo build`; the workaround is `MSB_HOME=<scratch> cargo build …` or the offline overrides. Recorded in the `Cargo.toml` comment.

**Run time** (`runtime.rs:25-31, 143-199, 377-539`). `EMBEDDED_RUNTIME_ARCHIVE = Some(include_bytes!(OUT_DIR/runtime-bundle.tar.gz))`. `install_runtime(cfg, InstallOptions { source: EmbeddedArchive, .. })` copies the bytes, untars into a temp dir inside `cfg.home()`, then `publish_pair` (under `<home>/.runtime-install.lock`): **complete pair present and `force: false` → `Ok(())` without touching it** (no version check in the SDK); **partial pair → error "refusing to repair partial installation" regardless of `force`**; else stage → rename library → `libkrunfw.dylib` symlink → rename `msb` last (the completion marker). `verify: true` then checks the published pair. `ensure_runtime` only installs on `RuntimeNotInstalled`. Nothing in the SDK compares the installed version with the embedded one — that is theuth's job (§2).

## 2. Decisions (nobody to ask)

| # | Decision | Why |
|---|---|---|
| 1 | Features `["local", "net", "embed-binaries", "download-binaries"]` — cargo fetches the archive itself | Alternative was `embed-binaries` + `.cargo/config.toml [env] MSB_EMBED_RUNTIME_BUNDLE_PATH` + a fetch script every dev/CI/Docker build must run first (fresh checkout = panic). Automatic + cached + the two offline overrides is the shortest path; the `~/.microsandbox` side effect is the price, documented. Design's rejection of `download-binaries` ("breaks offline builds and docs.rs") is superseded by the ruling: embedding needs the archive at build time by definition. |
| 2 | Home policy: **shared when versions match, private otherwise** (`install::effective_home`) | Brief's pick. Empty default home → extract there; same version → use as is (same bytes); another version or half a pair → `<data_dir>/sandbox/msb-home`, warning `runtime_home`. Explicit `sandbox.vm.home` always wins. Chosen once per process, at `SandboxActor::new`. |
| 3 | The `PATH` `msb` lookup (S2's `msb_on_path`) is deleted | It existed to avoid a 30 MB download when Homebrew had msb elsewhere. Extraction is free and version‑exact; a `PATH` pair of another version would silently win over the embedded one. `MSB_PATH` (the SDK's own override) still works → `Installed`. |
| 4 | `RuntimeState::Embedded { extracted: true }` also for the user's own same‑version pair | Indistinguishable bytes; the card says "(embedded, in <home>)". `Installed { version }` = another version (explicit home or `MSB_PATH`), used as is, never overwritten. |
| 5 | `Missing` kept, now `installable: false`, message "This build of t carries no microsandbox runtime." | Only produced by the actor's `vm::runtime()`‑failed fallback and the probe‑panic fallback; nothing can install it. |
| 6 | `runtime_extract_failed` is remembered in `Microsandbox.extract_failed` (process‑local) and cleared by the next successful extraction | Preflight is a pure probe; the failure has to live somewhere the button can clear. |
| 7 | `Incomplete` stays `installable: true` but `install()` refuses with the two files to remove | The SDK never repairs a partial pair (`publish_pair`), so `force` would fail anyway; the fix text is the honest one. |
| 8 | Contract updated (`RuntimeState`, `installable` comment) although not in my file list | One enum line; leaving it stale would mislead the next worker. Flagged in §6. |

## 3. Per commit

1. `9c5739f1` feat(sandbox): `Cargo.toml` features + comment; `Cargo.lock` (ureq & co. for the SDK's build script); `vm/mod.rs` `RuntimeState::Embedded { version, extracted, home }`, `Blocker` docs; `vm/install.rs` rewritten (`embedded_version`, `config`, `effective_home`, `other_msb`, `resolve`, `msb_path`, `install` = extract, 2 tests); `vm/preflight.rs` (`Facts.extract_failed`, `Facts.other_msb`, states, blockers `runtime_extract_failed`/`runtime_incomplete`/`runtime_missing`, warning `runtime_home`, tests); `vm/microsandbox.rs` (`extract_failed`, `setup_runtime`, `create` extracts on first use, `install` = `setup_runtime`, preflight carries the failure); `actor.rs` `new()` sets `cfg.vm.home = effective_home(..)`; `tools.rs` `sandbox_install_runtime` description/refusal/success texts.
2. `7add4005` feat(desktop): `SandboxVms.tsx` runtime line + **Set up runtime** button (also while `Embedded { extracted: false }`), no download sentence; fixtures; tests (+1 for the not‑yet‑extracted card).
3. `7b774362` docs(79): decision 2, §4.6 tool rows, §5.1, §7 button, §16 row; contract `RuntimeState`.
4. `ff606490` docs(79): as‑built draft.
5. (last) build: `[profile.release.package.microsandbox] codegen-units = 1` (4 → 2 copies of the archive, −64 MiB; §4); as‑built final.

## 4. Measured (this Mac, M‑series 16 cores / 64 GiB, `cargo build --release -p theuth`)

| Measure | Result |
|---|---|
| Release archive embedded | `microsandbox-runtime-0.7.4-macos-aarch64.tar.gz` 29,918,400 B (28.5 MiB), cached in `target/release/.microsandbox-runtime-cache/` |
| `target/release/t` without the feature (`["local","net"]`, same tree) | **567,866,208 B** |
| `target/release/t` with the feature, default profile | **688,508,144 B** (+120,641,936 B = +115 MiB): `__TEXT,__const` grew 4× the archive — the SDK's `const EMBEDDED_RUNTIME_ARCHIVE: Option<&[u8]> = Some(include_bytes!(…))` is materialised once per codegen unit that uses it; `grep` of the archive's first 4 KiB found **4 copies** |
| with `[profile.release.package.microsandbox] codegen-units = 1` (commit 5) | **621,718,336 B** (+53,852,128 B = **+51.4 MiB**, 2 copies) |
| Where the 2 remaining copies are (`nm -n`) | one `_anon.e995…` in **`libsandbox`** (theuth's own `install()` awaits `setup::install_runtime`; the `pub async fn` body is cross‑crate MIR‑inlined, so the const is instantiated in the caller crate); one in `libmicrosandbox` next to `snapshot::verify::hash_zeroes::ZEROES` (the SDK's own `runtime/spawn.rs:318-326`: with `embed-binaries` **every spawn calls `ensure_runtime(EmbeddedArchive)` itself**). Getting to one copy needs an upstream change (`static` + a `#[inline(never)]` accessor); not reachable from theuth |
| Release build wall time | 21 m 25 s cold after the feature flip; 4 m 40 s for the profile change (microsandbox + dependents) |
| First‑use extraction (`create`, empty `MSB_HOME`) | **0.3 s** (`bin/msb` 36,110,304 B + `lib/libkrunfw.5.dylib` 24,801,440 B + `libkrunfw.dylib` symlink) |
| Unit test extraction into a temp home (`extraction_is_versioned_and_idempotent`) | whole `vm::` suite 1.05 s |

## 5. Smoke (release `t` copied to `/tmp/79/t-f6-embed`, data dir `/tmp/79f6smoke`, rig per `79-smoke.md`: `mesh.name f6-host`, `sandbox.vm { enabled, image: theuth-vm, loopback_mesh: true }`, `rpc 127.0.0.1:7986`, `secrets.key_store: file`, `/tmp/79/rpc.ts`)

| # | Step | Result |
|---|---|---|
| 1 | `MSB_HOME=/tmp/79/msbhome-f6` (empty) → `sandbox_preflight` | `runtime: { state: embedded, version: 0.7.4, extracted: false, home: /tmp/79/msbhome-f6 }`, `virt ok hvf`, **no blockers, no warnings** (28 ms) |
| 2 | Image: `~/.microsandbox/bin/msb image save theuth-vm:dev` → `MSB_HOME=… msb image load -i … -t theuth-vm:dev` (the shared binary, the new home's cache; `bin/` stays empty → still `extracted: false`) | `theuth-vm:dev sha256:78988e32c59d 1.8 GiB` |
| 3 | `sandbox_create { name: lab, image: theuth-vm:dev }` | Progress: `create microsandbox theuth-vm:dev` → `install setting up the embedded microsandbox 0.7.4 in /tmp/79/msbhome-f6 (no download)` → `install extracted …/bin/msb + …/lib/libkrunfw.5.dylib in 0.3 s` → pull (cache) → `startup: Activating` → detect/config/binary/first_run/daemon → `pair lab (6cb30312…)` → `verify connected`, desktop `http://127.0.0.1:52861/vnc.html` → **`ready` 15.3 s after the create** |
| 4 | `sandbox_preflight` after | `extracted: true`; `MSB_HOME=/tmp/79/msbhome-f6 /tmp/79/msbhome-f6/bin/msb ls` → `lab theuth-vm:dev running`; `msb --version` → `msb 0.7.4` |
| 5 | `sandbox_install_runtime` on the extracted home | `null` in 0 ms; progress `runtime 0.7.4 already set up in /tmp/79/msbhome-f6` (idempotent) |
| 6 | `sandbox_destroy lab` → the extracted `msb ls` | 3.4 s; `No sandboxes found.`; `sandboxes_list` → `[]` |
| 7 | Foreign shared home: `MSB_HOME=/tmp/79/msbhome-foreign` with a fake `bin/msb` + `lib/libkrunfw.5.dylib` | `runtime: { embedded, extracted: false, home: /tmp/79f6smoke/sandbox/msb-home }`, warning `runtime_home`: "theuth's VMs live in /tmp/79f6smoke/sandbox/msb-home; your own msb (unknown in the default home) will not list them."; the fake files untouched |
| 8 | `MSB_HOME` unset, `sandbox.vm.home` unset (the system `~/.microsandbox` 0.7.4) | `runtime: { embedded, version 0.7.4, extracted: true, home: /Users/Sky.Moore/.microsandbox }`, no warnings — the user's install is used |

Rig notes: (a) an engine started with a plain `&` from a tool shell died silently when that shell returned — long‑lived processes need the service mode; (b) the scratch engine's update plugin **replaced `/tmp/79/t-f6-embed` on disk with the released 2.3.3** during the smoke (`UpdateReady 2.3.3 applied`); `update.enabled: false` was added to the scratch config afterwards — every 79 smoke rig should carry it.

## 6. Edits outside my owned files (flagged)

- `crates/sandbox/src/tools.rs` — `sandbox_install_runtime` description, the "already set up" refusal (now also `Embedded { extracted: true }` when no blocker is installable), success text. Needed for the tool to stay truthful.
- `crates/sandbox/src/actor.rs` `SandboxActor::new` — one line: `cfg.vm.home = effective_home(..)` (the brief allowed the create path only; the home must be fixed before the backend is built, which is `new`).
- `docs/design/79-contract.md` — `RuntimeState` gains `Embedded`; `installable` comment.
- `Cargo.lock` — the SDK's build script deps (`ureq`, `rustls-platform-verifier` for it, …) as a consequence of `download-binaries`.

## 7. Not built / deviations

- No per‑target gating (all five archives exist). Windows compiles by cfg reading only.
- `deploy/vm/README.md`, `scripts/package-*.sh|ps1`, `deploy/update/Dockerfile`: **no lines to add** for an online build — cargo fetches the archive. For an offline/hermetic build the release owner adds, before `cargo build --release -p theuth`: `curl -fsSLo /tmp/msb.tgz https://github.com/superradcompany/microsandbox/releases/download/v0.7.4/microsandbox-<darwin|linux|windows>-<aarch64|x86_64>.tar.gz && export MSB_EMBED_RUNTIME_BUNDLE_PATH=/tmp/msb.tgz` (Docker: `ENV MSB_EMBED_RUNTIME_BUNDLE_PATH=/tmp/msb.tgz` + the `curl` in a `RUN`). The Docker build's `~/.microsandbox` side effect lands in the throwaway build container.
- `effective_home` is decided at actor start; installing/removing a foreign `msb` in `~/.microsandbox` while `t` runs is seen after a restart.
- `runtime_extract_failed` is process‑local (not persisted): after a restart the state is `extracted: false` again and the next create retries — the right behaviour anyway.
- The version read (`setup::resolve_runtime_version`) is a binary scan; an unreadable/odd `msb` reads "unknown" and counts as foreign (private home). Tested with a fake file (smoke 7, unit test).

## 8. Gates

- `cargo check -p sandbox -p rpc -p theuth --tests` — `Finished dev profile` in 5 m 37 s, no warnings, no errors
- `cargo clippy -p sandbox -- -D warnings` — clean (42 s)
- `cargo test -p sandbox --lib` — **44 passed**, 0 failed (1.29 s) — includes the two new `vm::install` tests (real embedded archive extracted into a temp home; foreign/partial pairs) and the preflight tests `runtime_states`, `another_msb_in_the_default_home_warns`
- UI: `bun run check` (tsc) — pass (after `bun install` in `crates/desktop/ui` and `packages/apps`; bindings regenerated with `cargo test -p rpc --features ts --quiet export_bindings` and copied, as `just ts-desktop` does); `bunx vitest run src/components/settings/SandboxVms.test.ts` — 10 passed.

## 9. Follow‑ups for the integrator

1. `just ts-desktop` after merge (the `RuntimeState` binding changed; bindings are gitignored).
2. Release owner: note `download-binaries`' build‑time write into `~/.microsandbox` in the release notes for developers; consider `MSB_HOME=$PWD/target/msb-build-home` in `scripts/package-*.sh` if a builder box runs its own msb.
3. `docs/design/79-smoke.md` check 1 describes `runtime_missing` on an empty `MSB_HOME`; it is now `Embedded { extracted: false }` with no blocker (this report §5). Not edited (not my file).
4. Windows `cargo check` of `sandbox` with the embed — unverified here.
