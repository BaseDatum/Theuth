# deploy/vm — the sandbox VM image (design doc 79 §6.1, contract §2.12)

A Debian 13 (trixie) systemd guest for [microsandbox](https://microsandbox.dev)
that boots into `Xtigervnc :1` (VncAuth) → XFCE → `websockify`/noVNC on 6080 →
`t --headless` with `DISPLAY=:1`. Everything lives on the root disk,
including `/data`.

## Why Debian 13

| | glibc | runs the Linux `t`? |
|---|---|---|
| Debian 12 bookworm | 2.36 | no — the released `t` needs `GLIBC_2.39`; one built on Ubuntu 26.04 needs `2.43` |
| **Debian 13 trixie** | **2.41** | yes for anything built on ≤ 2.41 (bookworm- or trixie-built `t`, the release CLI) |
| Debian 14 forky | moving | testing — no fixed ABI, weekly breakage; not a base for a shipped image |
| Ubuntu | — | rejected (Sky: "I don't want Ubuntu bloat") |

A `t` built on a newer glibc than the guest's fails at exec with `GLIBC_2.xx not
found`; check with `ldd --version` in the guest and `objdump -T t | grep GLIBC_ | sort -u | tail -1`
on the binary. The base is our own (`debian:trixie-slim` + systemd); no third-party
`debian-systemd` image is used anywhere.

| File | What |
|---|---|
| `Dockerfile` | stage `base` = `debian:trixie-slim` + `systemd systemd-sysv dbus ca-certificates`, `serial-getty@hvc0` + `getty@tty1` masked, `CMD ["/lib/systemd/systemd"]`; stage `desktop` = packages, Node LTS, `uv`, user `engine` (uid 1000), placeholder VNC password, the four units enabled, `LABEL io.theuth.vm.schema=2`; final stage adds a minisign-verified `t` from the release manifest |
| `Dockerfile.dev` | `FROM theuth-vm:desktop` + `COPY t` (a Linux `t` for the host arch at `deploy/vm/t`, git-ignored) |
| `bootstrap.sh` | first-boot script for a *base-only* guest (`ghcr.io/basedatum/theuth-vm-base:<tag>`): same packages, user, units; marker `/var/lib/theuth-vm/bootstrapped`; no `t` (bring-up puts it) |
| `units/*.service` | `xvnc`, `desktop`, `novnc`, `theuth` — copied to `/etc/systemd/system`; `scripts/vm-units-check.sh` fails when `bootstrap.sh`'s copies drift |

## Build

```sh
just vm-image tag=2.3.3            # buildx linux/amd64,linux/arm64, local only
just vm-image tag=2.3.3 --push     # publish ghcr.io/basedatum/theuth-vm:2.3.3
just vm-image-base                 # `base` stage only → ghcr.io/basedatum/theuth-vm-base:trixie (add --push to publish)
just vm-image-dev                  # desktop stage + local ./t → theuth-vm:dev → msb load
```

`t` is resolved from `https://s3.rwx.dev/theuth/releases/latest.json` →
`platforms["cli-linux-<x86_64|aarch64>"]` and verified with the key in
`crates/update/pubkey.txt` (build-arg `THEUTH_PUBKEY`). While the release has
no Linux CLI asset the release build fails with a clear message; pass
`--build-arg THEUTH_T_URL=https://…/theuth_<v>_linux-<arch>.tar.gz` (its
`.sig` must sit beside it) or use the dev image.

Package names that differ from bookworm: `libpipewire-0.3-0` → `libpipewire-0.3-0t64`.
Everything else in the apt set exists under the same name on trixie.

## Load and run

```sh
export PATH=$HOME/.microsandbox/bin:$PATH
docker save theuth-vm:dev | msb load          # image ref inside msb: theuth-vm:dev
msb run -d --name vm-b --replace --init auto --cpus 2 --memory 2G --root-disk 8G \
  -p 127.0.0.1:6080:6080 theuth-vm:dev -- sleep infinity
```

`--init auto` probes and hands PID 1 to `/lib/systemd/systemd`; `systemctl` and
dbus work. For a bare-systemd image `msb run` *also* runs the image CMD (or the
trailing command) through agentd after boot and stops the sandbox when it
exits — `CMD ["/lib/systemd/systemd"]` exits 1 there ("Explicit --user argument
required"), so the CLI needs `-- sleep infinity`. The SDK's `Sandbox::create`
runs no startup command, so bring-up passes only `init("auto")`. `-p` publishes
guest 6080 (noVNC) only; RFB 5901 stays on the guest's loopback.

The bootstrap route boots the base-only image the same way and runs
`bootstrap.sh` as a first-boot script (`script("start-desktop", …)`); it needs
Debian's mirrors, nodesource and astral.sh reachable and takes minutes.

## Verify

```sh
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:6080/vnc.html   # 200
msb exec vm-b -- systemctl is-active xvnc desktop novnc theuth
msb exec vm-b -- systemctl is-system-running     # running or degraded; never stuck in starting
msb exec vm-b -- sh -lc 'ldd --version | head -1; /usr/local/bin/t --version'
msb exec vm-b -- sh -lc 'DISPLAY=:1 xdpyinfo | head -5; DISPLAY=:1 xwininfo -root -tree | grep -i xfce'
msb exec vm-b -- sh -lc 'DISPLAY=:1 xrandr -s 1280x800; DISPLAY=:1 xrandr | head -3'  # SetDesktopSize
open 'http://127.0.0.1:6080/vnc.html?autoconnect=1&resize=remote'   # password: changeme (placeholder)
msb rm -f vm-b
```

The VNC password baked into the image is the placeholder `changeme`; bring-up
replaces `/home/engine/.vnc/passwd` (`printf '%s' "$pw" | vncpasswd -f`) and
restarts `xvnc` (which restarts `desktop` through `Requires=`).

## Sizes and timings

See `docs/design/79-as-built-F1.md` (trixie) for the measured table: image
size, guest disk, idle memory, boot to `/vnc.html` 200, `t` running.
`79-as-built-B.md` has the bookworm numbers.
