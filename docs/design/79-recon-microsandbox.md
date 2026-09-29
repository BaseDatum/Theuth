<!-- Recon fact sheet for design 79 (microsandbox v0.7.4, docs + repo @ c677090), produced by a read-only subagent on 2026-09-29; cited by 79-sandbox-vm.md §2. Reported facts, not decisions. -->

# microsandbox fact sheet (checked 2026-09-29, v0.7.4)

The research is complete. All 12 sections are below, plus an assumption check and an open/unknown list. I read every page in the docs index except the cloud API endpoint pages (183 of 183), plus the repo at `main` c677090, crates.io, docs.rs, GitHub issues/releases and the `guest-images` repo.

**Not done:** the knowledge-base entry I meant to save for this recon did not save (a runtime cell failed its checks). Nothing is recorded under `facts/microsandbox-recon-*`. If you want it kept, record it from this sheet.

**URL shorthand:** D/x = https://docs.microsandbox.dev/x · G/x = https://github.com/superradcompany/microsandbox/blob/main/x (read at c677090) · I/n = https://github.com/superradcompany/microsandbox/issues/n

**Why "not in the reference" appears below:** the Rust reference on D/sdk/rust/* leaves out many methods that exist in the source and in the guide pages. docs.rs has no rendered docs for any release.

## 1. Rust SDK

**Package and release**
- **Official crate:** `microsandbox`, at `sdk/rust`. The library is `sdk/rust/lib/lib.rs`; there is also a `[[bin]] microsandbox` at `bin/main.rs` (needs the `local` feature) (G/sdk/rust/Cargo.toml).
- **Install:** `cargo add microsandbox` (D/sdk/overview).
- **crates.io** (https://crates.io/api/v1/crates/microsandbox):
  - v0.7.4, published 2026-09-29T06:13Z; Apache-2.0; edition 2024; no `rust_version` set.
  - 57 versions, first published 2025-04-02.
  - 31,622 downloads total, 14,666 recent.
  - Published by `appcypher` (Stephen Akinyemi).
- **docs.rs** (https://docs.rs/crate/microsandbox/latest):
  - Failed to build every listed release, 0.3.0 through 0.7.4 (54 versions).
  - The 0.7.4 log says `failed to create runtime home /.microsandbox: Permission denied`, from build.rs:29. The default `download-binaries` feature installs the runtime at Cargo build time (https://docs.rs/crate/microsandbox/latest/builds/4639211).

**Cargo features** (G/sdk/rust/README.md)
- On by default: `local`, `cloud`, `keyring`, `net`, and `download-binaries` ("Install a matching official `msb` + `libkrunfw` pair during Cargo builds; implies `local`").
- Off by default: `embed-binaries` and `ssh`. The Rust SSH page says "Requires the `ssh` feature." (D/sdk/rust/ssh)
- Examples (verbatim):
  ```
  cargo add microsandbox --no-default-features --features cloud,net
  cargo add microsandbox --no-default-features --features local,net,download-binaries,keyring
  cargo add microsandbox --no-default-features --features local,net
  ```

**How it talks to the runtime**
- No daemon, server, port or API key locally: "Running locally, creating a sandbox spawns a VM as a child process; there is no daemon to install and no server to connect to." (D/sdk/overview)
- An API key is only for cloud: `MSB_BACKEND=cloud` plus `MSB_API_KEY` (D/operations/backends).
- The `msb` binary is required:
  - "Local sandboxes need `msb` and `libkrunfw`… Sandbox creation does not download it automatically." (D/sdk/setup)
  - Setup helpers: `setup::ensure_runtime(&GlobalConfig::default(), InstallOptions::default())`, `resolve_runtime`, `is_runtime_installed`, `install_runtime`.
  - Install sources: `release_download` (default), `directory`, `archive`, `embedded_archive`.
  - Overrides: `MSB_PATH`, `MSB_LIBKRUNFW_PATH`, `MSB_AGENTD_PATH`, or the process-wide `set_sdk_msb_path()` / `set_sdk_libkrunfw_path()`.
  - Search order: explicit paths → `home` / `MSB_HOME` / `~/.microsandbox` → bundled (D/sdk/setup).
- libkrun is **not** loaded into the application process:
  - The SDK depends on `microsandbox-runtime` with the `client` feature only (G/sdk/rust/Cargo.toml).
  - The `runner` feature, which pulls in `msb_krun` ("Native Rust API for libkrun microVMs", pinned `=0.1.39`), lives in the runtime (G/crates/runtime/Cargo.toml, G/Cargo.toml, https://crates.io/crates/msb_krun).
- Process protocol (G/COMPATIBILITY.md §3–5, §12):
  - The private launcher is `msb machine` (v0.7.0). On Unix it passes launch JSON on file descriptor 96, the watchdog on 97, startup JSON on 98 and the lifecycle lock on 99.
  - It runs a JSON-lines control socket (named pipe on Windows) for live changes to CPU, memory and secrets.
  - It runs a CBOR-framed agent socket → relay → libkrun virtio-console → guest `agentd`.
  - MSB_HOME holds a SQLite database.
- Lifetime rules:
  - `create()` is attached: local handles "stop the sandbox when the client process exits".
  - `.detached(true)`, `create_detached()` and `sb.detach()` let it survive the process (D/sdk/rust/sandbox).
  - `Drop` cleans up (D/sdk/errors).

**Minimal example (verbatim, D/sdk/overview)**
```rust
use std::error::Error;

use microsandbox::Sandbox;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let sb = Sandbox::builder("hello")
        .image("python")
        .create()
        .await?;

    let output = sb.exec("python", ["-c", "print('Hello, World!')"]).await?;
    println!("{}", output.stdout()?);

    sb.stop().await?;
    Ok(())
}
```

**API surface**
- **`Sandbox`, static functions** (D/sdk/rust/sandbox):
  - `builder(name)`: names are at most 128 UTF-8 bytes.
  - `get(name) -> SandboxHandle`.
  - `list() -> SandboxPage`: newest first, 20 per page.
  - `list_with(|l| l.limit(1..=100).cursor().label().labels())`.
  - `remove`, `start` (attached), `start_detached`.
- **`Sandbox`, instance methods:**
  - Lifecycle: `stop`, `stop_with_timeout`, `stop_and_wait`, `request_stop`, `kill`, `kill_with_timeout`, `request_kill`, `drain`, `request_drain`, `restart`, `restart_with`, `destroy`, `destroy_with`, `remove_persisted`, `detach`.
  - Waiting: `wait`, `wait_until_stopped`, `wait_for_status`.
  - Health and state: `ping`, `touch`, `config`, `id`, `name`, `owns_lifecycle`.
  - Other: `metrics`, `metrics_stream(Duration)`, `logs(&LogOptions)`, `fs()`, `modify()`, `restore_warnings`.
- **Execution** (D/sdk/rust/execution): `exec(cmd, args) -> ExecOutput`, `exec_with`, `shell(script)`, `shell_with`, `exec_stream -> ExecHandle` (`recv`, `wait`, `collect`, `signal`, `kill`, `resize`, `take_stdin`), `exec_stream_with`, `shell_stream`, `attach -> i32`, `attach_with`, `attach_shell`.
- **`SandboxBuilder`, documented in the reference:**
  - Creating: `create`, `connect_or_create`, `detached(bool)`, `create_detached`, `build`, `overlay(SandboxConfigPatch)`, `replace`, `slug` (cloud only).
  - Resources: `cpus(u8)`, `max_cpus(u8)`, `memory(MiB)`, `max_memory`, `thp`.
  - Image: `image(impl IntoImage)` (OCI ref, local directory or disk image, auto-detected), `image_with(|i| i.oci("python:3.12").root_disk(8.gib()))`, `pull_policy`, `registry`.
  - Workload: `entrypoint`, `cmd`, `init`, `init_with`, `env`, `workdir`, `user`, `shell`, `script(name, body)`, `patch`.
  - Networking: `network(|n| …)`, `port(host, guest)`, `port_bind(ip, h, g)`, `port_udp`, `port_udp_bind`, `proxy`, `disable_network`, `hostname`.
  - Other: `secret`, `secret_env`, `volume(guest, |m| …)`, `idle_timeout(secs)`, `max_duration(secs)`, `log_level`.
- **In the source or guides but not in the reference** (G/sdk/rust/lib/sandbox/{builder,mod,handle,pause,modify}.rs):
  - Builder: `label`, `labels`, `security`, `cpu_placement`, `placement_profile`, `deployment_profile`, `replace_with_timeout`, `vsock*`, `root_disk`, `root_disk_with`, `ephemeral(bool)`, `create_with_progress`, `create_with_pull_progress`, `rlimit`, `metrics_sample_interval`.
  - Sandbox: `pause`, `resume`, `pause_with_guest_flush`, `log_stream`, `follow_logs`, `compact`, `exec_default*`.
  - Handle: `snapshot(name)`, `refresh`, `status_snapshot`, `active_config`.
  - Branching: `branch` / `branch_many` (G/sdk/rust/lib/sandbox/branch.rs; D/sandboxes/snapshots).
- **`SandboxHandle`:** `status`, `config`, `config_json`, `connect` (errors if no response within 10 s), `connect_with_timeout`, `connect_or_start(_detached)`, `start(_detached)`, `stop*`, `kill*`, `restart*`, `destroy*`, `remove`, `metrics`, `logs`, `modify`, `ping`, `touch`, `wait_for_status`, `wait_until_stopped`, `created_at`, `updated_at`.
- **`SandboxModificationBuilder`:** `cpus`, `max_cpus`, `memory`, `memory_mib`, `max_memory`, `max_memory_mib`, `env`, `remove_env`, `label`, `remove_label`, `workdir`, `secret`, `remove_secret`, `next_start`, `restart`, `dry_run`, `apply`. `root_disk_size(MiB)` exists in the source and the Live Modify guide only (D/sandboxes/tuning).
- **Volumes** (D/sdk/rust/volumes):
  - `Volume::builder(..).directory()/.disk().size().quota().create()`, plus `get`, `list`, `remove`, `get_default` (cloud only).
  - `MountBuilder`: `bind`, `named`, `named_with`, `tmpfs`, `disk`, `format`, `fstype`, `readonly`, `noexec`, `nosuid`, `nodev`, `stat_virtualization`, `host_permissions`, `owner(uid, gid)`, `size`; `owned` / `owned_with` appear in the guide.
- **Networking** (D/sdk/rust/networking): `NetworkPolicy::builder/none/allow_all/from_profiles`. `NetworkBuilder`: `policy`, `port*`, `dns`, `tls`, `trust_host_cas`, `strict`, `max_tcp_connections`, `max_udp_connections`, `rate_limiter`, `ipv4_pool`, `ipv6_pool`, `interface`, `enabled`, `secret`.
- **Snapshots** (D/sdk/rust/snapshots, D/sandboxes/snapshots):
  - `Snapshot::builder(name).from_sandbox().group().full().guest_flush().record_integrity().create()`, plus `create_archive`, `list`, `open`, `remove`, `save`, `load*`, `group_head`.
  - Restore: `Sandbox::restore("group:member").name().forked().disk_only().volume().restore()`, `restore_ref(snapshot.reference())`, `restore_with_progress()`.
- **SSH** (needs the `ssh` feature): `sb.ssh()` → `connect`, `open_client`, `connect_with`, `server`, `prepare_server`. `SshClient`: `exec`, `attach`, `sftp`, `close`.
- **Images** (D/sdk/rust/images): `Image::get/list/inspect/remove/prune/load/save` plus `*_local` variants. There is no pull function; images are pulled on create according to `PullPolicy` (`Always`, `IfMissing` (default), `Never`).
- **Metrics:** `all_sandbox_metrics()`, `all_sandbox_metrics_reports_local`, `sandbox_metrics_report_local` (D/sandboxes/metrics).
- **Backend selection:** `set_default_backend(CloudBackend::from_env()? | LocalBackend::lazy()?)` (D/operations/backends).

**Rust compared with the other SDKs**
- The docs index lists the same page set (Sandbox, Execution, SSH, Filesystem, Volumes, Networking, Proxies, VSock, Secrets, Snapshots, Images, Agent client) for TypeScript, Rust, Python and Go. Ruby has no Proxies, Secrets or Agent-client pages (D/llms.txt).
- Rust-only: "Metrics reports are currently available only in the Rust SDK." (D/sandboxes/metrics)
- The Rust reference omits `log_stream`, create-with-progress and `ephemeral`, which the TypeScript/Python references document; they exist in the Rust source.
- I found no documented feature that Rust lacks.

## 2. CLI

- **Command forms:** `msb run` = `msb sandbox run` = `msb sbx run`. Aliases: `ls`, `ps`, `rm`, `cp`, `mod` (D/cli/sandbox-commands).
- **`msb run` flags asked about** (from the doc table plus G/crates/cli/lib/commands/common.rs and run.rs):
  - `-n/--name`: "If omitted, the sandbox is ephemeral."
  - `--replace`, and `--replace-with-timeout DURATION` (defaults to 10 s).
  - `-c/--cpus`, `-m/--memory` (e.g. `512M`, `1G`).
  - `--root-disk` (`8G`, `tmpfs:2G`, `flat:8G,clone=auto`, `./rootfs.qcow2:format=qcow2,fstype=ext4`).
  - `-p/--port` (`8000:8000` binds loopback; `0.0.0.0:8000:8000`; `/udp`).
  - `--script-path NAME:PATH`: "Same destination as `--script`; the file's contents are read verbatim at launch time". This one is in the source only, not the doc table.
  - `--entrypoint EXECUTABLE`: "With `msb run`, pass its arguments after the image and `--`."
  - `--conf` (repeatable).
  - `-d/--detach`: "Run the resolved image command in the background and print the sandbox name".
  - `--init PATH|auto` with `--init-arg` and `--init-env`. These are missing from the sandbox-commands flag table; I/1676 notes the website omits `--init`.
  - `-v/--volume SOURCE:DEST[:OPTIONS]`, `-e/--env`, `-w/--workdir`.
- **Other `msb run` flags:**
  - Resources: `--max-cpus`, `--max-memory`, `--thp`, `--max-duration`, `--idle-timeout`.
  - Mounts: `--mount-dir`, `--mount-file`, `--mount-named`, `--mount-disk`, `--mount-owned`, `--tmpfs`.
  - Scripts and patches: `--script NAME=BODY`, `--script-raw`, `--copy`, `--copy-file`, `--copy-dir`, `--mkdir`, `--rm`.
  - Identity: `-u/--user`, `-H/--hostname`, `--pull`.
  - Network: `--no-net`, `--net`, `--net-rule`, `--net-strict`, `--proxy`, `--vsock`, `--deployment-profile`.
  - Other: `--secret`, `--label`, `--no-tty`, `--log-level`, the scoped `--*-conf` files.
- **`msb create`:** same flags as `run` except `--detach`; boots without running a command.
- **`msb exec`:** `-t/--tty`, `--no-tty`, `-e`, `-w`, `-u`, `--timeout`, `--rlimit`, `-q`. A stopped sandbox is started temporarily and stopped again.
- **`msb logs`:** `--tail`, `--since`, `--until`, `-f/--follow`, `--timestamps`, `--source stdout|stderr|output|system|all`, `--grep`, `--json`, `--raw`, `--show-id`, `--color-sessions`, `--color`.
- **`msb stop`:** `--force/-f`, `-t` (seconds; "expiry fails without killing"), `--label`. **`msb start NAME | --label`.** **`msb restart`:** `-t` defaults to 10. **`msb rm`:** `-f` stops then removes; `--label`.
- **Listing and inspection:**
  - `msb ls`: `--running`, `--stopped`, `--label`, `--format json`, `-q`.
  - `msb ps`: `-a`, `--format json`; the CPUS and MEM columns show "current allocation / maximum resize capacity".
  - `msb inspect --format json`.
  - `msb metrics`: `--watch`, `--follow` (JSON Lines), `--interval`, `--all`, `--sort`.
- **Other lifecycle commands:** `msb wait [--timeout] [--format json]`, `msb ping [--touch]`, `msb touch`, `msb pause`, `msb resume`, `msb branch -n | --names`, `msb modify`, `msb copy` (alias `cp`) (all D/cli/sandbox-commands).
- **Snapshots** (D/cli/snapshot-commands). Long form `msb snapshot` also works, and the old top-level `restore` / `save` / `load` are still accepted (D/cli/overview):
  ```
  msb snap create ready --sandbox worker [--full] [--group G] [-o x.msb] [--integrity] [--guest-flush auto|required|skip]
  msb snap restore worker:ready --name restored [--forked|--disk-only] [-v ..] [-p ..] [-c ..] [-m ..]
  msb snap ls [--group G] [--format json] [-q]   ·   msb snap inspect worker:ready [--verify]   ·   msb snap verify worker:ready
  msb snap rm worker:ready [-f]   ·   msb snap head worker[:member]   ·   msb snap export worker:ready --output ready.msb [--with-image]
  msb snap import ready.msb [--group G] [--base ..] [--set-head]   ·   msb snap reindex [DIR]
  ```
- **`msb ssh`** (D/sandboxes/ssh, D/cli/ssh-commands):
  - Needs no sshd in the guest: "SSH gives a sandbox a familiar interface without putting an SSH daemon inside the guest. microsandbox speaks SSH on the host side".
  - Subcommands: `msb ssh [connect] NAME [-- cmd]`, `msb ssh authorize --file|--key|--stdin`, `msb ssh serve NAME [--host 127.0.0.1] [--port 2222] [--stdio]`.
  - Default inactivity timeout is 10 minutes; no reverse (`-R`) forwarding.
- **`msb doctor` / `msb self doctor`:** "Check runtime files, supported host virtualization prerequisites, and non-blocking host performance capabilities". `--fix` opens an elevated PowerShell on Windows to enable WHP (D/cli/management).
- **`msb server`:** does not exist now (0 hits in all 183 docs pages). It existed before the rewrite: the v0.2.4 README shows `msb server start --dev` (G/v0.2.4 README.md). The v0.3 rewrite was "around an embeddable SDK" (D/changelog/2026-04-03). `msb shell` was removed; use `msb exec --tty <sandbox> -- bash` (D/changelog/2026-04-10).
- **Images:**
  - `msb pull [--materialize layered|flat|all] [--insecure] [--ca-certs]`, `msb load`, `msb save [--format docker|oci]`, `msb images`, `msb image inspect`, `msb rmi`, `msb image prune`, `msb registry login|logout|list` (D/cli/image-commands, D/cli/registry-commands).
  - No `msb build` (0 hits). "microsandbox cannot read Docker's private local image store directly. First, build the image with Docker" (D/examples/docker/local-images).
- **Also available:** `msb install` / `uninstall`, `msb context`, `msb self update` / `downgrade` / `uninstall`, `msb completion` (D/cli/management, D/cli/overview).

## 3. Resources and live resize

- **Defaults:** 1 vCPU, 512 MiB memory; `max_cpus` and `max_memory` default to the starting size; shell `/bin/sh`; THP `madvise` (D/sandboxes/overview). With no disk size set, the managed root disk is 4096 MiB (D/configuration, `upper_size_mib` row).
- **Maximums:** not documented. The types are `u8` for CPUs and `u32` MiB for memory (D/sdk/rust/sandbox). On cloud, "A maximum that differs from the final size is rejected." (D/configuration)
- **(b) Live resize while running: documented and supported (local only).**
  - "CPU and memory | Live, within the boot-time `max_*` ceilings | The VM and workload keep running" (D/sandboxes/tuning).
  - Lowering is live too: "You can lower CPU or memory live without pre-planning, then grow back up to the booted ceiling later."
  - "The ceilings default to the starting size, so omitting them leaves no live growth headroom."
  - Mechanism: the guest "converges asynchronously (onlining CPUs, plugging memory blocks)"; states are `Accepted`, `Converging`, `Applied`, `GuestRefused`, `Failed` (D/sdk/rust/sandbox, `ResourceResizeStatus`).
  - Source: memory is resized "through virtio-mem"; "CPUs above `vcpus` boot parked for later hotplug"; a VM booted without headroom returns `memory_resize_unavailable` (G/sdk/rust/lib/sandbox/modify.rs, G/crates/runtime/lib/runner/vm.rs, …/runner/control/executor.rs).
  - virtio-balloon is used only for memory statistics (D/changelog/2026-06-19).
  - Platform differences for live resize: not documented.
  - Verbatim:
  ```rust
  let sb = Sandbox::builder("worker").image("python").cpus(2).memory(1024).max_cpus(8).max_memory(4096).create().await?;
  let plan = sb.modify().cpus(4).memory(2048).label("tier", "web").apply().await?;
  sb.modify().root_disk_size(8192).apply().await?;
  ```
  ```bash
  msb create python --name worker --cpus 2 --memory 1G --max-cpus 8 --max-memory 4G
  msb modify worker --cpus 4 --memory 2G --label tier=web
  msb modify worker --max-memory 16G --dry-run
  ```
- **(a) Changes at restart:**
  - Raising `max_cpus` / `max_memory` needs `--restart` or `--next-start`.
  - "On a stopped sandbox, changes are saved for the next boot."
  - Root disk: `msb modify worker --root-disk 8G` works live, or with `--restart` / `--next-start`. "Managed and flat ext4 roots grow only; tmpfs changes on boot"; user-supplied disk images are never resized. "A stopped sandbox grows before the new size is persisted." (D/sandboxes/tuning)
- **Other limits:**
  - THP "cannot change with `msb modify`" (D/sandboxes/optimization).
  - A full-snapshot restore must keep the captured CPU and memory, including the maximums; `--disk-only` can change them (D/sandboxes/snapshots).
- **Metrics per sandbox:** yes, local only.
  - `SandboxMetrics`: `cpu_percent`, `memory_bytes`, `memory_limit_bytes`, `disk_read_bytes`, `disk_write_bytes`, `net_rx_bytes`, `net_tx_bytes`, `upper_used_bytes`, `upper_free_bytes`, `upper_host_allocated_bytes`, `uptime`, `timestamp` (D/sdk/rust/sandbox).
  - CLI: `msb metrics`. Export: the `msb-metrics` OTLP sidecar (D/sandboxes/metrics, D/observability/msb-metrics).
- **Open issues:**
  - I/1578: after `converging` there is no completion signal.
  - I/1512: the `memory.limit` metric stays at the boot size after a live resize.

## 4. Persistence and snapshots

- **Stop / start:**
  - "Stopping keeps its configuration and filesystem… Starting it again boots a new VM; it does not resume the old processes." (D/sandboxes/lifecycle)
  - Pause and resume keep RAM (D/sandboxes/lifecycle).
- **`/tmp`, conflicting sources:**
  - The docs say that for disk-backed OCI roots, "`/tmp` uses the root disk… Disk-backed temporary files can survive stop/start" (D/sandboxes/volumes); v0.7.0 lists "Disk-backed temporary storage" (D/changelog/v0.7.0).
  - Open issue I/1377 (filed 2026-08-17, before 0.7) reports an automatic tmpfs `/tmp` sized `min(memory_mib / 4, 512)` MiB.
- **What `rm` deletes:** the record, config and logs, the managed writable root `upper.ext4`, and generated scripts. It keeps image caches, named volumes, snapshots, bind-mounted host files and user disk images (D/sandboxes/lifecycle).
- **Host layout** (MSB_HOME, default `~/.microsandbox`):
  - `bin/msb`; `lib/libkrunfw.{so,dylib,dll}`.
  - `cache/`: `layers/` (EROFS), `fsmeta/`, `vmdk/`; `db/`.
  - `sandboxes/`: per sandbox, `logs/{exec.log,runtime.log,kernel.log,boot-error.json}` and `ssh/host_ed25519`.
  - `volumes/<name>/`, `snapshots/`, `logs/`, `secrets/`, `ssh/authorized_keys`, `config.json`.
  - Sources: D/configuration (paths), D/images/overview, D/sandboxes/logs, D/cli/ssh-commands.
- **Image cache:** content-addressed and de-duplicated; layers are pinned at first pull (D/images/overview).
- **Bind mounts / volumes:** supported, including on macOS. See section 11.
- **Snapshot types** (D/sandboxes/snapshots):
  - Disk (default): "Files and owned volumes | Boots a new VM".
  - Full: "Disk, memory, and running processes | Resumes execution".
  - "there is no memory-only snapshot."
- **Snapshots as templates:** yes. "Each restore creates a new sandbox and leaves the source unchanged."
  - `--forked` shares copy-on-write memory across full restores.
  - `msb branch` forks a live sandbox.
  - Warm-worker pattern: many restores from one snapshot (D/examples/sandboxing/warm-workers).
- **Restore behaviour:**
  - `--name` is required; "does not accept a startup command"; "Restore creates an idle detached sandbox" (D/cli/sandbox-commands, D/examples/sandboxing/warm-workers).
  - Host mounts and ports are not inherited; supply `-v` / `-p` again. Branch children "cannot inherit published host ports".
  - Installed members are immutable since v0.7.
  - Disk restore loses `--init`: open issue I/1676.
- **Sizes:** not documented. `size_bytes()` exists in the API.
- **VNC page, verbatim (the whole "install once, snapshot, restore" text):** "For repeated launches, stop the prepared sandbox, create a snapshot, and boot desktops from that snapshot instead of reinstalling the packages." The page gives no commands for this (D/examples/development/vnc-desktop).

## 5. Networking

- **Defaults:**
  - "sandboxes can reach the public internet but cannot reach private networks, loopback, link-local addresses, or cloud metadata endpoints. Only published ports accept inbound traffic." (D/networking/overview)
  - The host is denied by default too (D/security/network).
  - Traffic goes through a user-space stack (smoltcp): "There is no host kernel routing or NAT in the path." (D/security/network, D/changelog/2026-04-03)
- **Guest view of the host:** "`host.microsandbox.internal` resolves to the host machine". You must allow the `host` group, e.g. `--net "public,host"`. `loopback` means the sandbox's own 127.0.0.1 (D/networking/overview).
- **Guest addressing:** each sandbox gets its own `/30` subnet from the pool `172.16.0.0/12`; IPv6 pool `fd42:6d73:62::/48`; MTU 1500; the gateway IPs back the `host` group. A fixed gateway IP is not documented (D/sdk/rust/networking).
- **Between sandboxes:** "Two sandboxes interact only through mechanisms you set up explicitly, like a shared named volume or a published port… Absent that, they can't see or reach one another." (D/security/isolation). Direct sandbox-to-sandbox networking is requested in I/1331 (open).
- **Port publishing:** bind defaults to `127.0.0.1`; `-p 0.0.0.0:8080:80` exposes it; UDP variants exist (D/networking/overview); TCP accept queue is 1024 (G/COMPATIBILITY.md §5).
- **Changing ports after creation:** not in `modify` (not in the supported-changes table or the flag list) (D/sandboxes/tuning, D/cli/sandbox-commands). Restore and branch accept new `-p` (D/cli/sandbox-commands). Changing network rules live is requested in I/1538 (open).
- **Egress controls:**
  - Profiles: `--net public|private|host|all|none`.
  - Rules: `--net-rule "<action>[:<direction>]@<target>[:<proto>[:<ports>]]"`, `--net-default*`, `--no-net`.
  - Strict hostname mode is on by default (since v0.7.3).
  - Also: DNS rebinding protection, TLS interception, host-held secrets, rate limits, SOCKS proxy, `--deployment-profile multi-tenant` (D/cli/sandbox-commands, D/networking/overview, D/changelog/v0.7.3).

## 6. Guest init and desktop

- **Default PID 1:** "By default the microsandbox agent runs as PID 1 inside the guest" (D/sandboxes/bootstrap).
- **`--init auto`:**
  - Uses a known init at the start of the image ENTRYPOINT (e.g. `/init`), otherwise probes `/sbin/init`, `/lib/systemd/systemd`, `/usr/lib/systemd/systemd`; fails with an error in `kernel.log` if none is found.
  - Needed for "long-lived daemons, system service tests, anything that talks to dbus or expects `systemctl` to work."
  - systemd's idle footprint is about 50 MiB (D/sandboxes/bootstrap, D/examples/guest/systemd-services).
- **`ghcr.io/superradcompany/debian-systemd:12`:**
  - `FROM debian:bookworm`; installs `systemd systemd-sysv dbus ca-certificates`; `CMD ["/lib/systemd/systemd"]`.
  - Multi-arch amd64 and arm64; rebuilt weekly.
  - Siblings: `ubuntu-systemd:24.04`, `fedora-systemd:40`, `alpine-openrc:3.20`.
  - Sources: https://github.com/superradcompany/guest-images (README, `debian-systemd/Dockerfile`).
- **Scripts:**
  - `--script` "is wrapped with a shebang derived from `--shell` (default `/bin/sh`) and made executable at `/.msb/scripts/<name>`; the directory is on `PATH`."
  - `--script-path NAME:PATH` reads the host file verbatim at launch (G/crates/cli/lib/commands/common.rs).
  - VNC page: "`--script-path` installs this file as the executable `start-desktop` command inside the guest."
- **VNC start script (verbatim, D/examples/development/vnc-desktop):**
```sh
#!/bin/sh
set -eu

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  breeze-icon-theme ca-certificates dbus-x11 lxqt-core novnc openbox \
  qterminal tigervnc-standalone-server websockify
rm -rf /var/lib/apt/lists/*

mkdir -p /root/.config/lxqt /root/.config/openbox /tmp/runtime-root
chmod 700 /tmp/runtime-root
sed -e "s/,quicklaunch//" -e "/^\[quicklaunch\]/,/^$/d" \
  /usr/share/lxqt/panel.conf > /root/.config/lxqt/panel.conf
sed "s#<name>Clearlooks</name>#<name>Breeze-ob</name>#" \
  /etc/xdg/openbox/rc.xml > /root/.config/openbox/rc.xml

Xtigervnc :1 -SecurityTypes None -geometry 1440x900 -depth 24 >/tmp/xvnc.log 2>&1 &
vnc_pid=$!
sleep 2
if ! kill -0 "$vnc_pid"; then
  cat /tmp/xvnc.log >&2
  exit 1
fi
cat /tmp/xvnc.log

DISPLAY=:1 XDG_RUNTIME_DIR=/tmp/runtime-root \
  dbus-launch --exit-with-session sh -c "
    startlxqt &
    session_pid=\$!
    sleep 4
    kill -0 \"\$session_pid\"
    qterminal &
    wait \"\$session_pid\"
  " >/tmp/lxqt.log 2>&1 &
desktop_pid=$!
sleep 5
if ! kill -0 "$desktop_pid"; then
  cat /tmp/lxqt.log >&2
  exit 1
fi

exec websockify --web=/usr/share/novnc 0.0.0.0:6080 localhost:5901
```
- **Final launch command (verbatim):**
```sh
msb run -d --name vnc-desktop --replace \
  --cpus 2 --memory 2G --root-disk 5G \
  -p 127.0.0.1:6080:6080 \
  --script-path start-desktop:./start-desktop.sh \
  --entrypoint start-desktop \
  debian:bookworm-slim
```
- **Rest of the page:**
  - Follow startup with `msb logs -f vnc-desktop`; check with `curl -fsS http://127.0.0.1:6080/vnc.html`.
  - Open `http://127.0.0.1:6080/vnc.html?autoconnect=1&resize=scale`.
  - Warning: VNC auth is disabled, so the listener is restricted to 127.0.0.1.
  - Clean up with `msb rm -f vnc-desktop`.
- **First-boot size and time (verbatim):** "The first install downloads about 183 MB, uses roughly 734 MB of guest disk, and can take a few minutes."
- **Display resizing, audio, clipboard, GPU:**
  - Across all 183 docs pages there are no mentions of clipboard, audio, PulseAudio/PipeWire, xrandr/RandR, `resize=remote`, virtio-gpu, virgl or venus.
  - The only display settings are `resize=scale` in the URL and `-geometry 1440x900 -depth 24`.
  - GPU: "modern CPU-only Linux desktop"; "VNC does not require a physical display or GPU"; "It is not suited to graphics-performance workloads."
  - GPU support is requested in I/291 (open since 2025-07-07).

## 7. Host requirements and install

- **macOS:** "require Apple Silicon. Intel Macs are not supported"; Rosetta does not help; no extra hypervisor feature is needed (D/troubleshooting/macos). The installer errors "x86_64 macOS is not supported. Microsandbox requires Apple Silicon (M1+)." with the comment "(libkrun HVF backend is aarch64-only)" (https://install.microsandbox.dev). The binary is code-signed with the hypervisor entitlement (D/security/isolation).
- **Linux:** glibc 2.28 or newer, x64 or ARM64, and read/write access to `/dev/kvm` (usually via the `kvm` group). No distributions are named; "Musl-only environments, such as a bare Alpine host, cannot run the local runtime" (D/troubleshooting/linux).
- **Nested virtualization:** "If you are inside a cloud VM, CI runner, or another hypervisor, the outer environment must expose nested virtualization before `/dev/kvm` can appear." (D/troubleshooting/linux). There is also a Docker image `ghcr.io/superradcompany/microsandbox:latest` run with `--device /dev/kvm` (D/examples/docker/docker).
- **Windows:** preview; Windows 10 or later, x64 or ARM64, with the `HypervisorPlatform` optional feature (WHP via libkrun) (D/troubleshooting/windows). The Rust SDK README says Windows 11 instead (G/sdk/rust/README.md).
- **Install commands:**
  - `curl -fsSL https://install.microsandbox.dev | sh`
  - `irm https://install.microsandbox.dev/windows | iex`
  - `brew install superradcompany/tap/microsandbox`
  - `npm i -g microsandbox`, `uv tool install microsandbox`, `cargo install microsandbox` (G/README.md)
  - `npx microsandbox run debian` (D/getting-started/introduction)
- **Root:** not needed. The script installs to `${MSB_HOME:-$HOME/.microsandbox}` `bin/` and `lib/`, links into `~/.local/bin`, and contains no `sudo` (https://install.microsandbox.dev; D/cli/overview). The per-sandbox host process "does not need to be root" (D/security/isolation).
- **What `msb doctor` checks:**
  - Runtime files and virtualization prerequisites, e.g. "✓ KVM device /dev/kvm", "✓ KVM access read/write" (D/examples/docker/docker).
  - AVIC / APICv reporting on Linux x86 (D/troubleshooting/linux).
  - A temporary clone probe for flat roots (D/cli/management).

## 8. Images

- **Registries:** "Docker Hub, GHCR, ECR, GCR, any OCI-compatible registry works." Private auth, insecure (plain-HTTP) registries and custom CAs are supported locally (D/images/overview).
- **Choosing an image:** `msb run <ref>` / `msb create <ref>`. A local directory or `.qcow2` / `.raw` / `.vmdk` path also works as the root (D/sandboxes/overview, D/images/overview).
- **Building:** no Dockerfile / `msb build`. Build with Docker, then `docker save my-image:latest | msb load`, or push to a local registry (D/examples/docker/local-images).
- **Project-published images:** only the init-system bases in `guest-images` (section 6). No desktop images are listed.
- **Pre-seeding a script:** there is no mechanism to bake a script into an image. Options are:
  - `--script` / `--script-path` or YAML `scripts:` at launch.
  - Pre-boot rootfs patches (`--copy`, `--copy-file`, `--copy-dir`, `--mkdir`, `--rm`).
  - A snapshot: warm-workers carries `verify-worker` into restored workers.
  - Baking with Docker.
  - Bootstrap page: scripts are "a clean way to bundle setup procedures… without baking them into the image" (D/sandboxes/bootstrap, D/examples/sandboxing/warm-workers).

## 9. Project health

- **GitHub** (search API, 2026-09-29 ~21:30 UTC): 8,478 stars, 459 forks, Apache-2.0, created 2024-10-03.
- **Last commit:** 2026-09-29T19:40:47Z (c677090, #1700).
- **Open work:** 64 open issues and 38 open PRs; the API's `open_issues_count` is 102.
- **Releases:** 30 GitHub releases from v0.5.3 (2026-05-30) to v0.7.4 (2026-09-29), about two a week. v0.7.0 was 2026-09-16, v0.7.3 2026-09-24 (G/releases).
- **Contributors:** 62 entries from the contributors API (with anonymous, including 3 bots). Top committers are appcypher (514) and toksdotdev (344); next is 9.
- **Maturity:** "Microsandbox is still **beta software**. Expect breaking changes" (G/README.md).
- **Cloud versus local:** cloud is a private beta (D/cloud/overview). Local-only features:
  - `modify` / live resize, published ports, custom DNS/TLS/rate limits.
  - Resource metrics, full snapshots, branching, snapshot groups, archives.
  - Kill, drain, ping, touch, historical logs, `msb ssh serve`.
- **VNC page marking:** "Local-only": "This example publishes noVNC to a port on the computer running the CLI, which is not available on microsandbox cloud."

## 10. Assumption check

| Assumption | Verdict | Evidence |
|---|---|---|
| Each sandbox is a hardware-isolated microVM via libkrun with its own kernel | **Confirmed** | "Its own **Linux kernel**… built from libkrunfw… KVM on Linux, Apple's Hypervisor.framework on macOS… through the libkrun VMM" (D/security/isolation); WHP via libkrun (D/troubleshooting/windows) |
| Default bind is 127.0.0.1 | **Confirmed** | Published ports (D/networking/overview, D/sdk/rust/sandbox `port()`); `msb ssh serve` also defaults to 127.0.0.1:2222 |
| `msb run -d` returns before the entrypoint finishes | **Confirmed** | "Detached mode returns as soon as the VM starts, before the desktop packages are ready." (D/examples/development/vnc-desktop); run.rs prints the name and returns |
| Named sandboxes persist their disk; ephemeral ones die with the command | **Confirmed (CLI)** | "Unnamed sandboxes are removed when the command finishes; named sandboxes persist." (D/cli/sandbox-commands). Unnamed `-d` runs are also ephemeral (G/crates/cli/lib/commands/run.rs comment). SDK sandboxes always have a name; attached ones *stop* (not removed) when the process exits (D/sandboxes/lifecycle) |
| Snapshot CLI lives under `msb snap` | **Confirmed** | D/cli/snapshot-commands, since v0.7.3; `msb snapshot` and the old top-level `restore` / `save` / `load` still work |
| Official desktop is X11 + TigerVNC + noVNC, CPU only, no virtio-GPU | **Confirmed** | CPU-only LXQt/TigerVNC/noVNC with the Xtigervnc X server (vnc-desktop page); guest devices are only virtio-console/net/fs/blk/rng (D/security/isolation); I/291 open. "virtio-GPU" appears nowhere in the docs |
| Official example floor is `--cpus 2 --memory 2G --root-disk 5G` | **Partly / unknown** | These are exactly the VNC example's values, but no page calls them a minimum. Other examples: systemd `--memory 1G --cpus 2`; warm workers `--root-disk 4G` |
| LXQt example: ~183 MB download, ~734 MB guest disk | **Confirmed** | Verbatim on the vnc-desktop page |

## 11. Volumes and shared directories (D/sandboxes/volumes read in full)

- **CLI syntax:**
  - `-v/--volume SOURCE:DEST[:OPTIONS]` takes a host path or a named volume, e.g. `./src:/app:ro`.
  - Explicit forms: `--mount-dir`, `--mount-file`, `--mount-named NAME:DEST[:kind=dir|disk,size=..,quota=..]`, `--mount-disk`, `--mount-owned DEST[:..]`, `--tmpfs PATH[:SIZE[:OPTIONS]]`.
  - Read-only is `ro` / `readonly`. Other options: `rw`, `noexec`, `nosuid`, `nodev`, `stat-virt=strict|relaxed|off`, `host-perms=private|mirror`, `quota=<size>`, paired `uid=<N>,gid=<N>`.
  - YAML adds `follow-root-symlinks` (D/cli/configuration).
- **YAML `--conf` key:** `mounts:` (verbatim):
  ```yaml
  mounts:
    - "./src:/app"
    - "./certs:/etc/app/certs:ro,noexec"
    - "./workspace:/workspace:uid=1000,gid=1000"
  ```
  An object form (`bind`, `named`, `tmpfs`, `disk` plus `target`) also exists (D/cli/configuration).
- **Rust equivalent:** `SandboxBuilder::volume(guest_path, |v: MountBuilder| …)` plus `Volume::builder(name)`. Verbatim:
  ```rust
  let sb = Sandbox::builder("dev")
      .image("python")
      .volume("/app/src", |v| v.bind("./src").readonly().noexec())
      .volume("/app/data", |v| v.bind("./data"))
      .create()
      .await?;
  ```
  ```bash
  msb create python --name dev \
    --mount-dir ./src:/app/src:ro,noexec \
    --mount-dir ./data:/app/data
  msb create alpine --name config-reader \
    --mount-file ./config.toml:/etc/app/config.toml:ro,nodev
  msb create python --name full \
    --mount-dir ./src:/app/src:ro,noexec \
    --mount-file ./pyproject.toml:/app/pyproject.toml:ro \
    --mount-named pip-cache:/root/.cache/pip:stat-virt=relaxed \
    --mount-disk ./dataset.qcow2:/dataset:ro,format=qcow2,fstype=ext4 \
    --tmpfs /tmp:1G:noexec,nosuid,nodev
  ```
- **Mechanism:**
  - Bind mounts go through virtio-fs on every host OS: "exposed through virtio-fs, a filesystem passthrough"; "Directory volumes mount through virtiofs. Disk volumes… mount through virtio-blk" (D/security/filesystem, D/sandboxes/volumes). 9p is not mentioned.
  - macOS vs Linux: path containment uses `openat2` with `RESOLVE_BENEATH` on Linux 5.6+; "On older kernels and on macOS, the broker falls back to `O_NOFOLLOW`" (D/security/filesystem).
- **Adding or removing volumes on a running sandbox:** not documented. Mounts are absent from `modify`; "Named volumes, mount tmpfs, and user disk images | Outside `modify`" refers to capacity (D/sandboxes/tuning). Restore and branch accept new `-v`. Changing vsock routes live is requested in I/1540.
- **Ownership and uid mapping:**
  - "By default the guest doesn't see your host's real uid/gid/mode on shared files. A stat-virtualization layer presents sandbox-local ownership" (D/security/filesystem).
  - The default `HostPermissions::Private` keeps guest `chmod` in an overlay; `Mirror` copies the rwx bits to the host.
  - `owner(uid, gid)` changes only the guest fallback and "does not `chown` the host files" (D/sdk/rust/volumes, D/sandboxes/volumes).
  - Who owns guest-root-created files on the host: not documented. The host process runs "as the **same user that launched it**" (D/security/isolation).
- **Performance:**
  - Mounts are "significantly faster than the filesystem API" (D/sandboxes/volumes).
  - Open issues: I/1702 (one slow virtio-fs request blocks the mount), I/1701 (first `statfs` walks the whole host tree), I/1683 (macOS `SEEK_DATA`/`SEEK_HOLE` swapped), I/1567 (macOS FSKit/exFAT ENOENT), I/1638 (Windows rename breaks the file until restart).
- **Symlinks:** root-symlink protection is on by default, with a per-mount `follow-root-symlinks` opt-out (D/changelog/2026-07-10). Symlink escapes out of the shared root are blocked (D/security/filesystem).
- **Case sensitivity:** not documented.
- **Must the host path exist?** Not documented for binds. The SDK's `named()` requires an existing volume; CLI `-v name:/path` creates it (D/sdk/rust/volumes, D/sandboxes/volumes).
- **`~` paths:** not documented. YAML relative paths resolve against the config file. A bind path "must be valid UTF-8 and must not contain `,`, `:`, or `;`" (D/sdk/rust/volumes).
- **Limits:** no mount-count limit is documented. Writable file mounts "receive the same default quota as directory binds", but the default quota value is not stated (D/sandboxes/volumes).
- **Ruby:** "Ruby currently supports named-volume management, but does not expose sandbox mount configuration."

## 12. Agent / programmatic control

- **List with status and settings:**
  - Rust: `Sandbox::list()` / `list_with()` → `SandboxPage { sandboxes: Vec<SandboxHandle>, next_cursor }`, then `h.status()`, `h.config()`, `h.config_json()` (D/sdk/rust/sandbox).
  - CLI: `msb ls --format json`, `msb ps --format json`, `msb inspect --format json`.
- **Metrics:** `sb.metrics()`, `metrics_stream(Duration)`, `all_sandbox_metrics()`; CLI `msb metrics --follow` (JSON Lines).
- **Logs:**
  - `logs(&LogOptions)` is a synchronous read of the on-disk `exec.log`.
  - `log_stream(&LogStreamOptions)` exists in the source ("Local streams the on-disk JSON Lines files… cloud streams the msb-cloud SSE logs endpoint", G/sdk/rust/lib/sandbox/mod.rs) but is not in the Rust reference.
  - CLI: `msb logs -f`.
- **Readiness without polling:**
  - "Creating a sandbox starts it and waits until it is ready for commands." (D/sandboxes/lifecycle). `Running` means "Guest agent is ready".
  - `wait_for_status(SandboxStatus)` ("no built-in timeout"), `wait_until_stopped()`, `msb wait`.
  - `create_with_progress()` (source).
  - Workload-level readiness (for example a port being open): not documented. The VNC example uses `msb logs -f` plus `curl`.
- **API spec:**
  - Local: no HTTP API; private process protocols only (G/COMPATIBILITY.md).
  - Cloud REST: `https://api.microsandbox.dev`, e.g. `GET /v1/sandboxes`, with a `Bearer msb_...` key.
  - OpenAPI: D/api-reference/openapi.json and D/api-reference/openapi.personal.json.
  - Sandbox endpoints: list, create, get/delete/start/stop by name or id, update, get metrics, list mounted volumes. Also snapshots, volumes (including file operations), quotas, billing, audit, organizations.
  - Exec, files and SSH are not in REST: they "ride the sandbox command channel" (D/api-reference/overview, D/cloud/overview).
  - No OpenRPC.
  - An MCP server is a separate repo: `npx -y microsandbox-mcp` (G/README.md).
- **Errors:**
  - Rust `microsandbox::Error` variants include `BootStart { name, err }`, `RuntimeNotInstalled`, `RuntimeIncomplete`, `LibkrunfwNotFound`, `WindowsHostSetup`, `SandboxAlreadyExists`, `SandboxReplaced`, `StopTimeout`, `ExecTimeout`, `ExecFailed`, `Io`, `Runtime`, `Database` (G/sdk/rust/lib/error.rs, D/sdk/errors).
  - `BootError { t, stage, errno, message }` is read from `boot-error.json`. Stages: `mount`, `build_vm`, `config`, `network`, `image`, `other`; the stage is classified by message text (G/crates/runtime/lib/client/boot_error.rs).
  - No dedicated error for missing KVM, a full disk or host OOM is documented; the exact KVM error text is not documented.
  - Exec spawn failure kinds include `OutOfMemory` and `ResourceLimit`. CLI exit codes: 127 (`NotFound`), 126 (`PermissionDenied` / `NotExecutable`), otherwise 1.
  - `Crashed` status covers "kernel panic, OOM, etc."
  - Cloud errors are `{error:{code,message,details}}`.

## Open / unknown
- Maximum CPU and memory values; platform-specific limits on live resize (macOS / Windows).
- Snapshot sizes; the exact commands to snapshot and restore the VNC desktop. Restore runs no startup command and needs `-p` again.
- `/tmp` behaviour on v0.7.4: the docs conflict with open issue I/1377.
- Display resize (RandR / `resize=remote`), audio and clipboard are not documented; GPU is unsupported (I/291).
- Bind mounts: whether the host path must exist, `~` expansion, case sensitivity, mount-count limit, the default quota size, and host ownership of guest-created files.
- Workload readiness signal; the exact error text for missing KVM, a full disk or OOM.
- Inconsistencies between pages:
  - Flat roots and snapshots: supported per D/sandboxes/bootstrap and v0.7.0; "Not supported" per the D/sandboxes/optimization table.
  - Local stop timeout: D/sdk/errors says the SDK force-kills; D/sdk/rust/sandbox and D/sandboxes/lifecycle say it does not.
  - Windows version: Windows 10+ (docs) vs Windows 11 (Rust README).
  - `--init` is missing from the CLI flag table.
- The Rust reference is incomplete against the source, and docs.rs is unavailable.

**Next step:** spot-check on a real host whatever the design depends on. The obvious candidates are live resize on macOS, the `/tmp` backing, and host ownership of files written into bind mounts. Also save this sheet to the knowledge base if it should persist.