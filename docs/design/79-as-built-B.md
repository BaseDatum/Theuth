# 79 — As built, worker B: the sandbox VM image (`deploy/vm/`)

Branch `feature/79-B` (base `14801163`). Owned: `deploy/vm/**`, the two `vm-image*` recipes at the end of `justfile`, `scripts/vm-units-check.sh`. `crates/**` untouched. Nothing pushed or published.

## Commits

| sha | What |
|---|---|
| `1c04ea78` | `feat(sandbox)`: `deploy/vm/{Dockerfile,Dockerfile.dev,bootstrap.sh,README.md,.gitignore,units/*.service}`, `just vm-image` / `vm-image-dev`, `scripts/vm-units-check.sh` |
| `7679e14a` | `fix(sandbox)`: `tigervnc-tools` (ships `vncpasswd` on bookworm — `tigervnc-standalone-server` does not); `desktop.service` `ExecStartPre` waits for `:1` |
| (this) | `docs(79)`: README run/verify sections, this report |

## Deliverable 1 — runtime on the Mac

`curl -fsSL https://install.microsandbox.dev | sh` → `msb 0.7.4` at `~/.microsandbox/bin/msb`, linked into `~/.local/bin`, no root. `msb doctor`:

```
info Platform: macOS aarch64 / Version: v0.7.4 / MSB_HOME: /Users/Sky.Moore/.microsandbox
   ✓ msb          /Users/Sky.Moore/.microsandbox/bin/msb
   ✓ libkrunfw    /Users/Sky.Moore/.microsandbox/bin/../lib/libkrunfw.5.dylib
   ✓ Root clone   reflink supported
   ✓ Architecture Apple silicon (arm64)
done Host setup is ready.
```

## Deliverables 2–4 — what is in `deploy/vm/`

- `Dockerfile`: two stages. `base` = apt set (`--no-install-recommends`) + Node LTS (`deb.nodesource.com/setup_lts.x`) + `uv` (`astral.sh/uv/install.sh`, `UV_INSTALL_DIR=/usr/local/bin`) in one layer; user `engine` uid 1000 (`/home/engine`, owns `/data` and `~/.vnc`); placeholder `~/.vnc/passwd` from `printf changeme | vncpasswd -f`; `/etc/tmpfiles.d/theuth-vm.conf` → `d /run/user/1000 0700 engine engine -`; `COPY units/` + `systemctl enable` of the four; `LABEL io.theuth.vm.schema=1`; `ENV THEUTH_DATA_DIR=/data`; `CMD` inherited. Final stage: `t` from `latest.json` → `platforms["cli-linux-<x86_64|aarch64>"]` (`url` + inline `signature`, python3 parses), minisign-verified with the same key as `deploy/computer/Dockerfile` (`ARG THEUTH_PUBKEY`, default = `crates/update/pubkey.txt` content; `just vm-image` passes the file), `tar -xzf` into `/usr/local/bin`, `t --version`. `ARG THEUTH_T_URL` overrides the tarball URL (`.sig` fetched beside it); `ARG THEUTH_MANIFEST` overrides the manifest URL.
- `Dockerfile.dev`: `FROM theuth-vm:base` + `COPY --chmod=755 t /usr/local/bin/t` + `t --version`. `deploy/vm/t` is git-ignored.
- `units/`: `xvnc` (`User=engine`, brief's ExecStart incl. `-ac`, `Restart=always`, `RestartSec=1`, `After=systemd-tmpfiles-setup.service`); `desktop` (`After=/Requires=xvnc`, env `DISPLAY=:1 XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/engine`, `ExecStartPre` polls `xdpyinfo -display :1` up to 10 s, `ExecStart=dbus-launch --exit-with-session startxfce4`, `Restart=always`); `novnc` (root, `websockify --web=/usr/share/novnc 0.0.0.0:6080 localhost:5901`, `After=xvnc`); `theuth` (`After=network-online.target desktop.service`, `Wants=network-online.target`, `User=engine`, env `DISPLAY=:1 THEUTH_DATA_DIR=/data THEUTH_LAUNCH=1 HOME=/home/engine XDG_RUNTIME_DIR=/run/user/1000`, `ExecStart=/usr/local/bin/t --headless`, `Restart=always`, `RestartSec=2`).
- `bootstrap.sh`: POSIX sh, root, marker `/var/lib/theuth-vm/bootstrapped`; same apt set, nodesource, uv, user, passwd, tmpfiles (`systemd-tmpfiles --create`), the four units as heredocs, `daemon-reload`, `enable --now`. Does **not** fetch `t` — bring-up `put`s it (`release.rs` header says the target needs neither curl nor minisign); `theuth.service` restarts every 2 s until it appears.
- `scripts/vm-units-check.sh`: `bash -n bootstrap.sh` + diff of `units/*.service` (comments stripped) against the heredocs. The one check for the one duplicated artefact.
- `justfile` (appended, nothing reordered): `vm-image tag push=""` → `docker buildx build --platform linux/amd64,linux/arm64 --build-arg THEUTH_PUBKEY="$(cat crates/update/pubkey.txt)" -t ghcr.io/basedatum/theuth-vm:<tag> <push> deploy/vm` (just interpolations for `tag` and `push`) (`just vm-image tag=X --push` publishes; default local); `vm-image-dev` → build `--target base` as `theuth-vm:base`, build `Dockerfile.dev` as `theuth-vm:dev`, `docker save theuth-vm:dev | msb load`.

## Deliverable 5 — proof on this Mac (M-series, Docker Desktop 29.8.0 arm64, msb 0.7.4)

| # | Step | Result |
|---|---|---|
| a | `docker info` | answered in 2 s; no hang, no Docker restart needed |
| b | release manifest | `latest.json` = 2.3.3 with only `darwin-aarch64` and `cli-macos-aarch64` → **no Linux `t` at all**; built the **dev image** with a placeholder `t` (sh script printing `t 0.0.0-placeholder`). The desktop stack is what is proven; the release `t` step is exercised only up to its clear failure message |
| b | build (arm64) | `base` apt+node+uv layer ≈ 33 s wall (fast mirror); full `base` rebuild after the fix 58 s; dev image seconds. `docker images`: **504 MB content, 2.02 GB unpacked** |
| c | `docker save theuth-vm:dev \| msb load` | 8.3 s; `msb images` shows **`theuth-vm:dev`** (480.5 MiB) — the ref is exactly the docker tag |
| d | `msb run -d --name vm-b --replace --init auto --cpus 2 --memory 2G --root-disk 8G -p 127.0.0.1:6080:6080 theuth-vm:dev` | **stopped after 0.2 s** (3 tries incl. `--init /lib/systemd/systemd`, and the bare base image): systemd *was* PID 1, but `msb run` also runs the image CMD `/lib/systemd/systemd` through agentd as the "startup command" → `Explicit --user argument required to run as user manager.` exit 1 → sandbox stops. Appending **`-- sleep infinity`** fixes it (the docs' "bare systemd images still run your trailing command through agentd after boot"). The SDK `create()` path runs no startup command (spike-1's runtime.log has no `starting startup command` line) — [INFERENCE] the integrator needs nothing beyond `init("auto")` |
| e | `msb run` → `curl /vnc.html` 200 | **1.47 s** first run, **3.9 s** on the rebuilt image (cold sandbox, warm image) |
| f | `systemctl is-active xvnc desktop novnc theuth` | `active active active activating` — `theuth` cycles `active`→`activating` because the placeholder exits 0 and `Restart=always`/`RestartSec=2` respawn it; `systemctl --failed` empty; `/proc/1/comm` = `systemd` |
| g | `xdpyinfo` / `xrandr` / `xwininfo -root -tree` | X.Org 1.21.1.6 on `:1`; `VNC-0 1440x900` with 14 RandR modes; tree has `xfwm4`, `xfce4-panel` (306×49 dock at bottom), `xfdesktop`, `xfsettingsd`, `Thunar`, `xfce4-session` (31 children) |
| h | `xrandr -s 1280x800` | `current 1280 x 800` immediately; back to 1440×900 likewise. Real RFB through websockify: headless Chromium on `vnc.html?autoconnect=1&password=changeme&resize=remote` authenticated (title `engine@vm-b - noVNC`), rendered the XFCE desktop, and `SetDesktopSize` changed the guest to `1500x960` (the tab's viewport) — screenshot taken, panel + desktop icons visible |
| i | sizes | image 504 MB / 2.02 GB (above); guest `df -h /`: overlay 7.9 G, **17 M** used in the upper (lower layers are EROFS, not counted); `free -m`: 196 MiB used of 1985; `msb metrics vm-b` at 28 s uptime: **262.5 MiB / 2.0 GiB**, CPU 0.04/2c |
| j | `msb stop` → `msb start` → 200 | stop 0.93 s; **start to `/vnc.html` 200: 2.79 s**; all four units up again, XFCE windows present, no failed units |
| k | `msb rm -f vm-b` | removed (0.6 s); `msb ls` shows only S's `spike-1` |

Gate: `bash -n deploy/vm/bootstrap.sh` ok; `just --list` parses (both recipes listed); `scripts/vm-units-check.sh` → `vm units in sync`. No cargo run.

## Deviations and decisions (no one to ask)

1. **`tigervnc-tools` added** to the apt set (Dockerfile + bootstrap.sh): on bookworm `vncpasswd` lives there, not in `tigervnc-standalone-server`; bring-up's `vncpasswd -f` needs it too.
2. **`desktop.service` gained an `ExecStartPre`** that polls `xdpyinfo -display :1` (≤ 10 s). Without it `dbus-launch --exit-with-session` started before Xtigervnc accepted connections, its babysitter lost X and killed the session bus, and `xfce4-session` sat on a modal "Unable to contact settings server" (seen via noVNC). `After=xvnc.service` only orders after exec, not readiness. `$$` escaping in the unit line is deliberate (systemd expands `$var`).
3. **`-ac` on Xtigervnc** (brief) kept; contract §2.12 omits it. Root `msb exec` and the engine share `:1` without xauth; RFB is still behind VncAuth and bound to guest loopback.
4. **`/run/user/1000` via tmpfiles.d**, not `enable-linger`: no `user@1000` manager is wanted (dbus-launch provides the session bus); `RuntimeDirectory=` on one unit would be torn down when that unit stops. Works: `drwx------ engine /run/user/1000` at boot.
5. **Two-stage Dockerfile; `Dockerfile.dev` is `FROM theuth-vm:base`** rather than a copy of the apt block — one package list, no drift. Consequence: `Dockerfile.dev` alone needs `theuth-vm:base` built first; `just vm-image-dev` does both.
6. **Release `t` step**: manifest has no `cli-linux-*` key today, so `just vm-image` fails at the `t` layer with `release 2.3.3 has no cli-linux-aarch64 asset; pass --build-arg THEUTH_T_URL=` (message from the python one-liner). `THEUTH_T_URL` + `.sig` beside it is the documented override; dev image is the fallback. Not exercised end-to-end for lack of an asset.
7. **`bootstrap.sh` does not download `t`** (design §6.1 leaves it to bring-up's `put`). `theuth.service` restarts every 2 s until the binary exists — harmless, noisy in `journalctl -u theuth`.
8. **Placeholder VNC password** is `changeme` (brief). Bring-up must replace `/home/engine/.vnc/passwd` and `systemctl restart xvnc` (this also restarts `desktop` via `Requires=`; noVNC clients reconnect).
9. **`--init auto` works** (no `--init /lib/systemd/systemd` needed); what was needed was a trailing command for the CLI (row d).
10. **`/tmp` is on the root disk**: `findmnt /tmp` empty in the guest, `df /tmp` = the root overlay; base image ships no `tmp.mount`. Open item §12.4 resolved: no `TMPDIR` override needed. Note `tmpfiles.d/tmp.conf` has `D /tmp` — /tmp is *cleared* at each boot.
11. **`network-online.target`** is reached at once in the guest (no network manager; agentd configures the NIC before systemd) — the `After=` is inert but harmless.
12. Node LTS via `setup_lts.x` pulls `gnupg` + `apt-transport-https` as its own deps; accepted rather than hand-writing the keyring.

## Not built / not measured

- Multi-arch `buildx --platform linux/amd64,linux/arm64` build not run (would need the release `t` for both arches or `THEUTH_T_URL` per arch; the recipe is verbatim from the brief plus the pubkey build-arg). amd64 untested.
- `computer_screenshot` from a `t` session inside (contract §3 B DoD): needs a real Linux `t`; none exists in the release. Covered indirectly: X.Org on `:1`, xcap/enigo need only `DISPLAY`.
- `bootstrap.sh` not run on a stock guest (timebox; it is the Dockerfile's commands in script form, `bash -n` + unit diff checked). Follow-up for step 6 ("bootstrap on a clean Debian mirror").
- Cold **image** pull timing (registry) — image was loaded locally.

## Edits outside owned files

- `justfile`: two recipes appended at the end (allowed by the brief). Nothing else.

## For the integrator (A / I)

- Image ref after `msb load` = the docker tag verbatim: `theuth-vm:dev`, `ghcr.io/basedatum/theuth-vm:<tag>`.
- Create with `init("auto")` only; **do not** set a startup command. If the actor uses the CLI/`msb run -d` anywhere, append `-- sleep infinity`.
- Ports: publish guest **6080/tcp** → `127.0.0.1:<ephemeral>` (noVNC + websockify). 5901 is guest-loopback only. Later: 7800/udp for `mesh.bind` (§6.3, S's spike decides).
- Bring-up's `config` step: write `/home/engine/.vnc/passwd` (`vncpasswd -f`, mode 600, owner engine) then `systemctl restart xvnc`; put `t` at `/usr/local/bin/t` (0755) — `theuth.service` picks it up within 2 s; `/data` is owned by uid 1000.
- Expect `/vnc.html` 200 ≈ 1.5–4 s after `Running` (design guessed 10–20 s); XFCE windows ≈ 5 s later.
- `theuth.service` env has `XDG_RUNTIME_DIR=/run/user/1000` in addition to the contract's three keys — harmless extra; remove if the contract is strict.
