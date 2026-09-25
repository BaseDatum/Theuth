# 67 — As built, worker S: `crates/sandbox` (make an engine)

*Branch `feature/67-agents-sandbox`, 2026-09-24. Design 67 §13 / decision 18, 64 §4 kept except the last step (mesh join instead of `nodes::Add`).*

## What was built, per commit

### Commit 1 — `feat(sandbox): Sandbox trait, ssh backend (russh + system ssh)`
- `crates/sandbox/src/lib.rs`: the `Sandbox` trait (`kind`, `create`, `exec`, `put`, `durable_roots`, `daemon`, provided `capabilities`, `destroy`), `Cmd { line, stdin, timeout }`, `ExecOutput { status, stdout, stderr }` (+ `reason()` = last stderr line), `Handle { id }`, `Capabilities { os, arch, durable_roots, daemon }`, `DaemonKind { Entrypoint, Service }`, `Progress` (stdout / log / silent sink), `detect()` = `uname -sm` then a PowerShell `OSArchitecture` fallback, `parse_uname`/`parse_arch` (release-asset spelling: `linux|macos|windows`, `x86_64|aarch64`), `sq()` shell quoting. `Target` (the CLI's request), `MakeEngine`/`Engines` messages, `SandboxActor` registered as `SANDBOX_ACTOR = "sandbox"`, `ActorJoin` (`MeshJoin` over `mesh::actor()`), `make_engine()` (the one entry both CLI and actor call: build backend → `bringup::run` → write record), `SandboxPlugin`, `actor()`.
- `crates/sandbox/src/ssh.rs`: `Target::parse("[user@]host[:port]")` (user/port stay `None` when absent so the system `ssh` takes them from `~/.ssh/config`); `SshSandbox::builtin(target, key_pem)` — `russh` 0.63, key decoded in memory from the vault value (OpenSSH / PKCS#8 / PKCS#1 PEM), host keys checked against the user's `known_hosts`, unknown learned with the fingerprint logged, changed key = hard error, exec on a split channel (writer task + reader loop, so multi-MB stdin cannot deadlock the window), 120 s inactivity timeout with 15 s keepalives; `SshSandbox::system(target)` — `ssh -o BatchMode=yes [-p port] [user@]host -- line` with stdin piped. `put` = `mkdir -p && cat > path && chmod` over `exec` for both transports. `destroy` = `t --uninstall-service` (+ `rm -rf ~/.theuth` unless `preserve_data`).
- `crates/sandbox/src/bringup.rs`: `MeshJoin` trait (`add_peer`, `status`), `Spec { name, local_t, pair_tries, verify_tries, verify_every }`, `run()`, `parse_pairing()`, `BIN_DIR = $HOME/.local/bin`.
- `crates/sandbox/src/release.rs`: `fetch(os, arch)` — `latest.json` (`update::URL`) → `platforms["cli-<os>-<arch>"]` → tarball streamed through a minisign verifier (`update::PUBKEY`) → every regular entry (`t` first, then the helpers shipped beside it); `extract()`.
- `crates/sandbox/src/record.rs`: `computers/<name>.yaml` (`Computer { name, backend, handle, engine, created_ms }`), `write/read/remove/list`.
- `crates/sandbox/src/docker.rs` landed in this commit too (see commit 2) so the crate compiles at every commit; `lib.rs` names the module.
- Workspace: `Cargo.toml` gained `sandbox = { path = "crates/sandbox" }` and `russh = "0.63"`, `bollard = "0.21"`; `members = ["crates/*"]` picks the crate up.

### Commit 2 — `feat(sandbox): docker backend (bollard) and deploy/computer image`
- `deploy/computer/Dockerfile`: `ubuntu:24.04` + `ca-certificates curl git python3 ripgrep tar zstd minisign`; build args `THEUTH_VERSION`, `THEUTH_PUBKEY` (the Tauri-base64 `crates/update/pubkey.txt`); downloads `theuth_<ver>_linux-<arch>.tar.gz` + `.sig` from the release bucket (arch from `dpkg --print-architecture`), verifies with `minisign -Vq` before extracting into `/usr/local/bin`; user `engine` uid 1000; `/data`, `/workspace` volumes; `ENTRYPOINT ["t","--headless","--data-dir","/data"]`, `THEUTH_DATA_DIR=/data`.
- `deploy/computer/Dockerfile.dev`: same image with `COPY t /usr/local/bin/t` from the build context.
- `crates/sandbox/src/docker.rs`: `DockerSandbox::connect(name, image, local_t)` — `DOCKER_HOST`, else `~/.docker/run/docker.sock` (Docker Desktop), else the local default; `ping`. Image: `--image REF` (pulled when absent), `--local-t PATH` → `Dockerfile.dev` tagged `theuth-engine:dev-<sha256[..12]>`, else `Dockerfile` tagged `theuth-engine:<this version>`; both Dockerfiles are `include_str!`'d so the binary carries them and the build context is an in-memory tar. Container `theuth-engine-<name>`, labels `io.theuth.engine=<name>`, `io.theuth.schema=1`, `io.theuth.image=<image id>`; a labelled container whose schema/image differ is force-removed (volumes kept) and recreated; a matching stopped one is started and adopted. Volumes `theuth-<name>-data:/data`, `theuth-<name>-workspace:/workspace`; `restart: unless-stopped`; no ports. `exec` = `create_exec ["/bin/sh","-lc", line]` with stdin fed on a task, stdout/stderr split, exit code from `inspect_exec`. `put` = `mkdir -p` + `upload_to_container` of a one-file tar (uid/gid 1000). `destroy` removes the container and, unless `preserve_data`, both volumes.
- `crates/sandbox/tests/docker.rs` (`#[ignore]`, needs a daemon and `SANDBOX_TEST_T=<linux t>`).

### Commit 3 — `feat(sandbox): bring-up → mesh join; CLI`
- `bringup::run` sequence: `create` → `capabilities` (detect; `windows` refused with a message) → **binary**: Entrypoint = "in the image"; Service = `--local-t` bytes, else keep a present `t --version`, else `release::fetch` — every file `put` to `$HOME/.local/bin/<name>` 0755 → **first_run**: `t --secret-list` (any one-shot runs `main`'s `first_run` prologue and proves the binary loads on that host, glibc included) → **daemon**: Entrypoint nothing; Service `t --install-service --no-pair --name <name>` (32's installer, launchd / systemd user / Task Scheduler; it waits for the RPC port itself) → **pair**: `t mesh pair` on the new engine, retried `pair_tries` (6) × `verify_every` (10 s) while the daemon comes up, JSON string parsed, `MeshJoin::add_peer` (= `mesh::AddPeer`) → **verify**: `MeshJoin::status` (= `mesh::Status`) polled `verify_tries` (3) until the peer is `reachable`. Every `t` invocation is `PATH="$HOME/.local/bin:$PATH" t …`.
- CLI (`crates/theuth/src/main.rs`): `MeshCmd::Add` now takes `pairing` *or* `--ssh TARGET [--key NAME | --system-ssh]` *or* `--docker [--image I | --local-t PATH]`, plus `--name N`. `--ssh`/`--docker` go to `make_engine()` in `main.rs`: bring-up runs in the CLI process with progress lines on stdout; the key is read with `secrets::Vault::open` (the same in-process open `--secret-set` uses; the value is decoded into a key object and never written anywhere); `RpcJoin` implements `sandbox::MeshJoin` with the `mesh_add` / `mesh_status` RPC methods on the running engine — the same messages every other `t mesh` call sends. Success prints `engine <name> (<id>) joined the mesh`; failure prints the step's one-line reason and exits 1.
- `sandbox::SandboxPlugin` added to `engine_plugins` after `tasks`; `crates/theuth/Cargo.toml` gained `sandbox` and `async-trait`.
- `computers/<name>.yaml` written after verify, only for compute this crate created.
- This document.

## Deviations from the brief (decided alone; nobody to ask)
1. **No `russh-sftp`.** `put` is `cat > path` over `exec` with the bytes on stdin — one code path for both SSH transports; the brief allowed "russh + scp over exec".
2. **Windows targets**: detected (PowerShell fallback) and refused at bring-up with a message. The Windows CLI is a `.zip`, `put` via `cat` has no PowerShell twin, and 32's Task Scheduler path is untested from here; a later doc.
3. **First run** is `t --secret-list`: there is no `--first-run` one-shot (64 §4.4 wanted one) and `main.rs` flags are outside my allow-list; every invocation runs `first_run` before anything else.
4. **Which `t`**: the *latest* release for the target triple (`latest.json` carries only that one's signature), not this binary's exact version. Both auto-update. The parent downloads and verifies, then `put`s — the target needs neither `curl` nor `minisign`.
5. **Docker image**: no Node/`uv` (64 §4.3 wanted them for stdio MCP servers; not needed to be an engine), user is `engine` (no branding in identifiers, uid 1000 fixed so uploads are owned right), no `io.theuth.owner` label (this engine's id is not known until `mesh::Status` works). `--image REF` skips the version-window check 64 §4.3 asked for.
6. **Where bring-up runs**: in the CLI process. The engine-side `SandboxActor` exists (`MakeEngine`, `Engines`) and shares `make_engine`, but no RPC method drives it (`crates/rpc` is not mine); the UI worker can add `sandbox_make_engine` → `sandbox::actor().ask(MakeEngine(target))` later. Consequence: `t mesh add --ssh|--docker` still needs a running `t --headless` here (for `mesh_add`/`mesh_status`), like every other `t mesh` call.
7. **Config**: no `sandbox:` section (64 §4.7). Timeouts are constants (`STEP_TIMEOUT` 600 s, verify 3 × 10 s); nothing else there applied.
8. **No `config.local.yaml` push** (64 §4.4 step 4): the mesh default relay is baked in, and the vault/MCP/skills arrive by replication (67 §13).
9. **Host key on the built-in transport**: the user's `~/.ssh/known_hosts` (learn on first use, refuse a change) instead of a per-record pin — same guarantee, no new file.
10. `docker.rs` is in commit 1 rather than commit 2 so each commit builds.

## Not built
- Windows bring-up (above). `snapshot`/`restore`, `recreate`, `status/start/stop` from 64 §4.1 (not in the brief's trait list). The bundle phase (64 §4.4 steps 7–12) — replaced by replication in 67. Tools (64 §4.6), RPC/UI. `t mesh remove` does not call `destroy` (the mesh worker owns `remove`; `Sandbox::destroy` is there for a `t mesh destroy <name>` later).

## Tests written (not run — orchestrator's job)
- `lib.rs`: `uname_and_powershell_spellings`, `reason_prefers_stderr_last_line`.
- `bringup.rs` (mock `Sandbox` + mock `MeshJoin`): `service_host_with_local_binary_runs_every_step_in_order` (exact exec order, put path/mode, pairing string forwarded, verify polls until reachable), `present_binary_is_kept_and_container_skips_install`, `pair_retries_while_daemon_starts_and_windows_is_refused`, `unreachable_after_polls_is_an_error`, `pairing_string_forms`.
- `ssh.rs`: `target_forms`. `release.rs`: `extracts_t_first_then_helpers`. `record.rs`: `round_trip_and_list`. `docker.rs`: `contexts_carry_the_expected_entries`.
- `tests/ssh.rs` (`#[ignore]`, `SANDBOX_TEST_SSH=user@host`, optional `SANDBOX_TEST_KEY_FILE`): detect, exit status + stdout/stderr split, a 3 MiB `put` round trip with mode.
- `tests/docker.rs` (`#[ignore]`, daemon + `SANDBOX_TEST_T`): build dev image, create, detect, `t --version`, put + ownership/mode, adopt on second create, destroy with volumes.

## `cargo check` results (one per commit, as allowed)
- Commit 1: `cargo check -p sandbox --tests` → 3 errors (two `map_err` on kameo `Reply` types — `ask` already unwraps the handler's `Err` into `SendError::HandlerError` — and one partial move in a test); fixed before committing, not re-checked for this commit.
- Commit 2: `cargo check -p sandbox --tests` → `Finished dev profile [unoptimized + debuginfo] target(s) in 7.10s`, no errors, no warnings.
- Commit 3: `cargo check -p theuth` → first attempt died in `spqr`/`boring-sys` build scripts because the supervised process had no Homebrew on PATH (`protoc`/`cmake` missing; nothing of mine compiled); rerun with the full PATH → `Finished dev profile [unoptimized + debuginfo] target(s) in 8m 36s`, no errors, no warnings from `main.rs`.
- Unit tests were not run (orchestrator's job).

## One-line edits outside `crates/sandbox` / `deploy/computer` / the CLI (disclosed)
- `Cargo.toml`: `sandbox = { path = "crates/sandbox" }`; `russh = "0.63"`, `bollard = "0.21"` under `[workspace.dependencies]`. `Cargo.lock` updated accordingly.
- `crates/theuth/Cargo.toml`: `sandbox = { workspace = true }`, `async-trait = { workspace = true }`.
- `crates/theuth/src/main.rs`: `Box::new(sandbox::SandboxPlugin)` after `tasks::TasksPlugin` in `engine_plugins`.
- `crates/mesh`: untouched — `mesh::AddPeer(String)` takes the pairing string as printed by `t mesh pair`; no variant was needed.

DONE S
