# 79 — As built, worker F1: guest image on Debian 13 (trixie), own systemd base, bootstrap on that base

Branch `fix/79-F1` (base `feature/79` @ `b4e5c38b`), worktree `theuth-79-F1`, 2026-09-30 08:57–09:20 BST. Owned: `deploy/vm/**`, the `vm-image*` recipes in `justfile`, `scripts/vm-*.sh`, and — alone — `docs/design/79-sandbox-vm.md` decision 3 / §6.1 / §16 image rows. Nothing pushed or published. Sky's rulings: "Debian 13 or 14, no Ubuntu bloat. Fix all the bugs."

## Commits

| sha | What |
|---|---|
| `04dd486d` | `feat(sandbox)`: `Dockerfile` on `debian:trixie-slim` with stages `base` (systemd, getty masked, `CMD systemd`) / `desktop` (packages on trixie, `schema=2`) / release (`t` by manifest, unchanged); `Dockerfile.dev` `FROM theuth-vm:desktop`; `just vm-image-base tag="trixie" push=""` (buildx `--target base` → `ghcr.io/basedatum/theuth-vm-base:<tag>`); `vm-image-dev` builds `--target desktop` |
| `b0a789a8` | `feat(sandbox)`: `bootstrap.sh` targets the `theuth-vm-base` guest; apt set = desktop stage (`libpipewire-0.3-0t64`); getty masks dropped (the base owns them) |
| `afdbaf47` | `docs(sandbox)`: `deploy/vm/README.md` — glibc table (12/13/14/Ubuntu), Debian 14 note, build/load/run/verify on trixie, base-only bootstrap image |
| `771453d1` | `fix(sandbox)`: `systemd-modules-load.service` masked in `base` (found in proof run 1: `is-system-running` = `degraded`) |
| `65ca0902` | `docs(79)`: decision 3 rewritten (trixie + own base; rejected `debian-systemd:12`, Ubuntu, forky), §6.1 base/package/bootstrap lines, §16 three image rows + open list; Scope "Debian 12" → "Debian 13" (one word) |
| (this) | `docs(79)`: this report |

## Deliverable 1 — `deploy/vm/Dockerfile`

- `base`: `FROM debian:trixie-slim` (Debian 13.7, glibc `2.41-12+deb13u4`, systemd 257.13); `apt-get install --no-install-recommends systemd systemd-sysv dbus ca-certificates`; `systemctl mask serial-getty@hvc0.service getty@tty1.service systemd-modules-load.service`; `CMD ["/lib/systemd/systemd"]`. No `ghcr.io/superradcompany/*` anywhere in `deploy/vm` (`grep` clean). 185 MB unpacked in Docker, **38.5 MiB** in `msb images`.
- `desktop`: the brief's package set, every name checked with `apt-get install --dry-run` on `trixie-slim` arm64 — all exist; one rename: `libpipewire-0.3-0` → **`libpipewire-0.3-0t64`** (time64 ABI; apt resolves the old name via Provides, the explicit name is used). Nodesource `setup_lts.x` works on trixie (Node 24.21.0; it replaces Debian's `libnode115`/`nodejs 20.19` that `novnc`'s deps pull in). `uv 0.12.21`. Same user/`/data`/placeholder passwd/tmpfiles/`COPY units/`/`systemctl enable` as before. `LABEL io.theuth.vm.schema=2`.
- release stage: unchanged (manifest → `cli-linux-<arch>`, minisign, `THEUTH_T_URL` override). Still unexercised: the release has no Linux CLI asset (I #10).
- `Dockerfile.dev`: `FROM theuth-vm:desktop` + `COPY --chmod=755 t` + `t --version` → `t 2.3.2` with the bookworm-built binary.

## Deliverable 2 — bootstrap on our base

- `just vm-image-base [tag] [--push]`: `docker buildx build --platform linux/amd64,linux/arm64 --target base -t ghcr.io/basedatum/theuth-vm-base:<tag> deploy/vm`; default tag `trixie`. Not run multi-arch (no buildx run in the timebox; the arm64 `--target base` build is what was proven).
- `bootstrap.sh`: header and apt set updated; idempotent as before (marker `/var/lib/theuth-vm/bootstrapped`, `id engine ||`, passwd only if missing, heredoc units overwrite, `enable --now`). The getty/modules masks are in the base image, not the script.
- `grep debian-systemd crates/sandbox/src` → **nothing**: no constant to change there, so no edit in `crates/`. The bootstrap base ref lives elsewhere — see "Edits outside owned files / for the integrator".

## Deliverable 4 — proof on this Mac (M-series, Docker Desktop 29.8.0 arm64, msb 0.7.4)

`t` = `/tmp/79/t-linux-aarch64-6a16493f-bookworm` (needs `GLIBC_2.35`) copied to `deploy/vm/t` (git-ignored, removed after).

| # | step | result |
|---|---|---|
| a | `docker build --platform linux/arm64 --target desktop` | 79.6 s wall the first time (apt layer 44 s), 69.3 s after the base fix; `theuth-vm:desktop` **1.74 GB** unpacked (bookworm was 2.02 GB); labels `{io.theuth.vm.schema:2}`, cmd `[/lib/systemd/systemd]` |
| b | `Dockerfile.dev` build | 22.7 s; `t 2.3.2`; `theuth-vm:dev` **2.68 GB** unpacked |
| c | `docker save theuth-vm:dev \| msb load` | 11.6 s / 13.4 s; `msb images` → `theuth-vm:dev` **631.9 MiB** (bookworm dev was 706.3 MiB) |
| d | `msb run -d --name vm-f1 --replace --init auto --cpus 2 --memory 2G --root-disk 8G -p 127.0.0.1:6081:6080 theuth-vm:dev -- sleep infinity` | returned in 1.77 s / 1.85 s |
| e | `curl -fsS http://127.0.0.1:6081/vnc.html` | **200 at 2.16 s / 2.6 s** from `msb run` start (0.4 s after it returned) |
| f | `systemctl is-active xvnc desktop novnc theuth` | `active active active active` at +6 s and still at +30 s; `journalctl -u theuth` has exactly one `Started theuth.service` line (no restart loop) |
| g | `t` really running | `ldd (Debian GLIBC 2.41-12+deb13u4) 2.41`; `/usr/local/bin/t --version` → `t 2.3.2`; `systemctl status theuth` → `active (running)`, Main PID `t`, 60 tasks, `Memory: 255.8M` (settles to ~182 MiB RSS main + 2 children ≈ 33 MiB each) |
| h | `systemctl is-system-running` | run 1: **`degraded`** — `systemd-modules-load.service` failed (`Failed to initialize kmod context: Operation not supported`; libkrun kernel 6.12.109 has no `/lib/modules`). Masked in `base` (`771453d1`). Run 2: **`running` at 2.82 s** from `msb run` start; `systemctl --failed` empty; `systemd-analyze`: kernel 155 ms + userspace 438 ms; `list-jobs` empty; `serial-getty@hvc0`/`getty@tty1` `masked` |
| i | `DISPLAY=:1 xrandr` | `VNC-0 1440x900`; `xrandr -s 1280x800` → `current 1280 x 800` at once |
| j | `xwininfo -root -tree` | 29 children; `xfwm4`, `xfdesktop`, `xfce4-panel` present (8 matching windows) |
| k | sizes / memory | guest `df /`: overlay 7.9 G, **22 M** used in the upper; `free -m`: 226–249 MiB used of 1985; `msb metrics` idle: **296.9–301.9 MiB / 2.0 GiB**, CPU 0.01–0.04 / 2c |
| l | `msb rm -f vm-f1` | removed; `msb ls` → none |

**Bootstrap route, same rig** (`theuth-vm-base:trixie` loaded, 38.5 MiB): `msb run -d --name vm-f1-boot --init auto … --script-path start-desktop:deploy/vm/bootstrap.sh theuth-vm-base:trixie -- sleep infinity` → PID 1 `systemd`, `is-system-running` = `running` within 3 s, idle **120.7 MiB**, upper 8.4 M. `msb exec vm-f1-boot -- start-desktop` → exit 0 in **1 m 55.6 s** (apt + nodesource + uv from the network), then `/vnc.html` 200, `xvnc desktop novnc` active, `theuth` `activating` (203/EXEC — no `t`, expected: bring-up puts it), `is-system-running` = `running`, upper 1.1 G, XFCE windows present, marker `2026-09-30T08:15:04Z`. Second `start-desktop` → `theuth-vm: already bootstrapped`. `msb rm -f`.

## Deliverable 5 — design doc

Decision 3 rewritten (three stages, why trixie, bootstrap on our base, rejected: `debian-systemd:12`, Ubuntu, forky, plus the earlier rejections kept); §6.1 `Dockerfile` and `bootstrap` paragraphs replaced with the built shape and the trixie numbers; §16 image row split into three (base, image, bootstrap base) and the open list updated. `79-contract.md` §2.12 still says `FROM ghcr.io/superradcompany/debian-systemd:12` — not my file; integrator (names in the contract win, but this is an image ref, not an API name).

## Gate

`bash -n deploy/vm/bootstrap.sh` ok · `scripts/vm-units-check.sh` → `vm units in sync` · `just --list` parses, shows `vm-image`, `vm-image-base`, `vm-image-dev` · step 4: `/vnc.html` 200 at 2.6 s, four units active incl. `theuth` running `t 2.3.2`, `is-system-running` = `running` (never `starting`, no longer `degraded`). No cargo run (no constant in `crates/sandbox/src`).

## Deviations and decisions (nobody to ask)

1. **`systemd-modules-load.service` masked** in `base` — not in the brief; found by the gate's `is-system-running` check (`degraded`). Root cause is the guest kernel, not a package; the mask is the base's job like the getty masks. Sky: "fix all the bugs".
2. **`libpipewire-0.3-0t64`** written explicitly (Dockerfile, bootstrap.sh, docs) instead of relying on apt's Provides for the old name.
3. **Stage names**: `base` (tiny systemd) / `desktop` / final. `Dockerfile.dev` and `vm-image-dev` moved from `theuth-vm:base` to `theuth-vm:desktop` — `base` now means the bootstrap image and the old name would mislead.
4. **Getty masks removed from `bootstrap.sh`**: the script targets our base, which masks them at build; keeping a second copy would be the drift `vm-units-check.sh` exists to prevent.
5. **`just vm-image-base` default tag `trixie`** (a base tracks the distro, not a `t` version); `--push` as the second positional like `vm-image`.
6. **Scope line** "Debian 12 + XFCE" → "Debian 13 + XFCE" in the design doc: one word outside decision 3/§6.1/§16, factual, flagged here.
7. Multi-arch `buildx` not run (as B): amd64 unbuilt; `debian:trixie-slim` and every package are multi-arch upstream.
8. Bootstrap proven with `msb exec … start-desktop` after boot rather than bring-up's `script("start-desktop")` — same file, same path (`/.msb/scripts/start-desktop`), no engine involved.

## Edits outside owned files

- `docs/design/79-sandbox-vm.md:5` — the one-word Scope fix (deviation 6). Nothing in `crates/`.

## For the integrator

- **Bootstrap base ref still `ghcr.io/superradcompany/debian-systemd:12`** in: `crates/theuth/src/main.rs:678` (`ImageRef::Bootstrap { base }` for `--bootstrap`), `:240` and `:936` (help/config comments), `crates/desktop/ui/src/components/settings/SandboxSheets.tsx:125` (`STOCK_BASE`), `sandbox-queries.ts:97` (fixture), `crates/sandbox/tests/vm.rs:17` and `examples/vm_spike.rs:23` (spike image), design §4.3/§4.7 examples (`:189`, `:278`), `79-contract.md:304`. The brief limited me to `crates/sandbox/src`, where there is none. Change to `ghcr.io/basedatum/theuth-vm-base:trixie` once published (or `theuth-vm-base:trixie` after a local `msb load`); the stock image would still boot but is bookworm and lacks the masks.
- Bring-up readiness: `is-system-running` now reaches `running` in < 3 s on this image, so the `is-active` poll (§16 readiness row) could go back to the design's check — optional.
- Image refs after `msb load`: `theuth-vm:dev` (`sha256:593d83c54ff0`), `theuth-vm-base:trixie`. The old bookworm `theuth-vm:dev` digest `593b8178dcfa` remains untagged in `msb images`; `docker images` still has the old `theuth-vm:base` (2.02 GB) — delete at will.
- Any Linux `t` for the guest must be built on glibc ≤ 2.41 (bookworm or trixie builders both work; `/tmp/79/deb-build.sh` on `rust:1-bookworm` still applies; a `rust:1-trixie` builder would match the guest exactly).
- The release stage (`cli-linux-*`) is still unexercised — no asset yet.
