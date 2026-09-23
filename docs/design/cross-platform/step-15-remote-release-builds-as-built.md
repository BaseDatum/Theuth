# 58 step 15 — remote release builds from the Mac, as built, 2026-09-23

Branch `feature/build-remote` off `1a81f57`. Scope: `scripts/build-remote.sh <user@host> windows|linux`
ships the exact local commit to a Windows or Linux ssh host, builds the release artifacts there, pulls
them into `target/release/dist/` on the Mac; `scripts/release-s3.sh` then publishes everything from
`dist/` (bucket `staging/<ver>/` is only a fallback). macOS packaging is unchanged.

Sibling omp: `scripts/package-windows.ps1` (+ `tauri.windows.conf.json`, `onnx.rs` dll lookup). This
driver only runs it; the artifact contract below is what both sides build to.

## Artifact contract (`<ver>` = `crates/desktop/tauri.conf.json` version; all in `target/release/dist/`)

| file | producer | signed (`.sig`) | `latest.json` key |
|---|---|---|---|
| `theuth_<ver>_windows-x86_64-setup.exe` | `package-windows.ps1` (NSIS; no MSI) | yes | `windows-x86_64` |
| `theuth_<ver>_windows-x86_64.zip` — exactly `t.exe`, `theuth-app-parser-<hash>.exe`, `theuth-documents-<ver>.exe` | `package-windows.ps1` | yes | `cli-windows-x86_64` |
| `theuth_<ver>_linux-x86_64.AppImage` | `package-linux.sh` | yes | `linux-x86_64` |
| `theuth_<ver>_linux-x86_64.{deb,rpm}` | `package-linux.sh` | no (SHA256SUMS only) | — |
| `theuth_<ver>_linux-x86_64.tar.gz` — `t` + both helpers | `stage-linux.sh` (`THEUTH_LINUX_NATIVE=1`) | yes | `cli-linux-x86_64` |
| `theuth_<ver>_linux-aarch64.tar.gz` | `just stage-linux` on the Mac (Docker), as before | yes | `cli-linux-aarch64` |
| `theuth_<ver>_macos-aarch64.{app.tar.gz,tar.gz,dmg}` | `package-update-macos.sh`, as before | tgz yes | `darwin-aarch64`, `cli-macos-aarch64` |

`crates/update` asks for `cli-<std::env::consts::OS>-<ARCH>` → `cli-windows-x86_64` / `cli-linux-x86_64`
already; Tauri's updater asks for `<os>-<arch>` → `windows-x86_64`, `linux-x86_64`.

## Flow (`scripts/build-remote.sh`)

```
Mac                                   remote host (~/theuth-build/)
git archive <ref> ──tar──▶ <sha>/          fresh dir; <sha>/target → ../target (symlink | junction)
                          prerequisite check   exit 2 lists what is missing, installs nothing
                          build               package-linux.sh + stage-linux.sh | package-windows.ps1
dist/ ◀──scp── target/release/dist/theuth_<ver>_<os>-*
                          rm <sha>/           (--keep leaves it; target/ always stays)
```

1. **Ship.** `git archive --format=tar <ref>` of the exact commit — nothing is fetched from GitHub on
   the host. Default ref is `HEAD` and a dirty tree is refused; `--ref <commit|tag>` skips the dirty
   check. Linux: the tar is piped through `ssh host 'tar -xf -'`. Windows: the tar is `scp`'d to
   `%USERPROFILE%\theuth-build-<sha>.tar` and extracted by `tar -xf` (bsdtar, Windows 10+) —
   PowerShell does not pass binary stdin through to a native command, so no pipe there.
2. **Shared target dir.** `<sha>/target` is a symlink (Linux) / junction (Windows, no admin needed) to
   `~/theuth-build/target`. Judgement call vs the brief's `CARGO_TARGET_DIR`: same effect (one warm
   cargo cache across shas), but the package scripts use literal `target/release/...` paths
   (`package-linux.sh` copies `target/release/t`, reads `target/release/bundle`; the contract puts
   the output in `target\release\dist\`), which `CARGO_TARGET_DIR` alone would break. Before each
   build the driver deletes `target/release/dist/theuth_<ver>_*` on the host so a stale artifact of the
   same version cannot be pulled.
3. **Prerequisites.** Linux: `scripts/package-linux.sh --check` — every `HOST_PKGS` entry via
   `dpkg-query`, plus `cargo rustc bun jq just` and `cargo tauri`; exit 2 with the `apt install` list.
   The same check now runs at the top of every `package-linux.sh` build. Windows: `cargo`, `pwsh`,
   `bun`, `tar` on PATH and `cargo tauri --version`; exit 2. `%USERPROFILE%\.cargo\bin` and `.bun\bin`
   are prepended because an sshd session may not carry the user PATH.
4. **Build.** Linux: `THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh` (AppImage/deb/rpm), then
   `THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh` (CLI tarball built on the host:
   `cargo build --release -p theuth`, `build_helper`, `build_doc_helper`, same `target/linux-out/`
   layout and tar entries as the Docker path). Windows: `pwsh -NoProfile -File
   scripts\package-windows.ps1` with `THEUTH_SKIP_UPLOAD=1` in the environment.
5. **Pull.** `scp 'host:theuth-build/target/release/dist/theuth_<ver>_<os>-*' <dist>/` (sftp-mode
   glob), then name / size / sha256 of each, and exit 1 naming what is missing: windows
   `-setup.exe` + `.zip`; linux `.AppImage` + `.deb` + `.rpm` + `.tar.gz`.
6. **Clean.** Linux `rm -rf ~/theuth-build/<sha>` (the symlink is removed, not followed). Windows:
   `cmd /c rmdir <sha>\target` first (a junction removed as a link), then `Remove-Item -Recurse`.

Windows commands are sent as `pwsh -NoProfile -NonInteractive -EncodedCommand <base64 UTF-16LE>`, so
the script text never meets the quoting rules of whichever default shell sshd uses (cmd.exe or
powershell.exe). `$PSNativeCommandUseErrorActionPreference = $false` and explicit `$LASTEXITCODE`
checks, because that default flipped between PowerShell 7.3 and 7.4.

`THEUTH_REMOTE_DRY_RUN=1` prints every ssh/scp command and the full remote scripts (decoded) and runs
nothing. `just build-remote <host> <os> [--ref …] [--dist …] [--keep]`.

## release-s3.sh changes

- Gate 6: `fetch <name>` = present in `dist/` → use it; else `aws s3 ls staging/<ver>/<name>` → pull;
  else absent. Linux tarballs (both arches) are still required unless `THEUTH_RELEASE_LINUX=skip`.
  AppImage/deb/rpm (per arch) and both Windows files are optional: absent → one `WARN` each, key omitted.
- Gate 7 (clean `ubuntu:24.04` smoke test of each Linux tarball): unchanged when Docker is present.
  Without Docker it now **fails** unless `THEUTH_SKIP_LINUX_SMOKE=1` (was a warning). Judgement call:
  the brief asked for a named skip; an implicit skip on a missing binary is the thing the named env is
  meant to replace. Note the Ubuntu host already ran `t --version` natively (stage-linux native path)
  — the gate mostly protects the Docker-built aarch64 tarball.
- Gate 8: the Windows zip is listed with `unzip -Z1` and must be exactly
  `t.exe theuth-app-parser-<hash>.exe theuth-documents-<ver>.exe`.
- `SIGNED` (was `TARBALLS`): app tgz, CLI tgz, Linux tgz(s), AppImage(s), `-setup.exe`, Windows zip —
  each gets `cargo tauri signer sign` → `.sig`, is uploaded with its `.sig`, and is re-downloaded and
  minisign-verified. `SHA256SUMS` = `SIGNED` + DMG + deb/rpm. Content types: `.exe`
  `application/vnd.microsoft.portable-executable`, `.zip` `application/zip`.
- `latest.json` adds `linux-<arch>` (AppImage), `windows-x86_64` (`-setup.exe`), `cli-windows-x86_64`
  (zip) when the file exists. Resume path (`<ver>/` already has objects) requires `.sig` for every
  `SIGNED` entry.

## Host prerequisites

**Ubuntu 24.04 (x86_64, no display, no sound server, no Docker):** `scripts/package-linux.sh
--check` is the authority; its `HOST_PKGS`:
`curl ca-certificates git build-essential clang cmake llvm libclang-dev pkg-config perl unzip
libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev libxcb-randr0-dev libx11-dev
libxtst-dev libxdo-dev libxkbcommon-dev libdbus-1-dev libpipewire-0.3-dev libdrm-dev libgbm-dev
libwayland-dev libssl-dev libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
librsvg2-dev patchelf`, plus rustup (channel from `rust-toolchain.toml`), `cargo install tauri-cli
just`, bun, jq. sshd, user in `~/.cargo/bin`/`~/.bun/bin` (the driver adds both to PATH).
`bun install --frozen-lockfile` and the AppImage bundler (linuxdeploy download) need outbound network.

**Windows 11 (x86_64):** OpenSSH Server (sshd, any default shell), PowerShell 7 (`pwsh` on the system
PATH), rustup with the `x86_64-pc-windows-msvc` toolchain + Visual Studio Build Tools (C++ workload;
`link.exe` is found without vcvars), `cargo install tauri-cli`, bun, `tar` (in `System32` since
Windows 10 1803). Whatever else `package-windows.ps1` needs (NSIS is downloaded by `cargo tauri
build`; C build scripts of dependencies may want cmake/nasm/clang) is the sibling's list — the
driver surfaces its exit code and output verbatim.

## What the orchestrator runs to verify

On the Mac (this worktree, `export PATH="$(dirname "$(rustup which cargo)"):$PATH"`):

```
THEUTH_REMOTE_DRY_RUN=1 scripts/build-remote.sh <user>@<win-host> windows        # prints the 4 pwsh scripts + scp
THEUTH_REMOTE_DRY_RUN=1 scripts/build-remote.sh <user>@<linux-host> linux        # prints the tar|ssh pipe + 2 bash scripts
scripts/build-remote.sh <user>@<linux-host> linux          # real: expect [4/5] to list 4 files, exit 0
scripts/build-remote.sh <user>@<win-host> windows          # real: expect [4/5] to list 2 files, exit 0
ls -l target/release/dist/theuth_*_{windows,linux}-*
```

Expected on first run against a bare host: step `[2/5]` exits 2 and prints the missing packages/tools
(Linux: `error: missing on <host>: …` + `apt install: …`; Windows: `error: missing on <COMPUTERNAME>: …`).
Nothing is installed by the driver. `--keep` leaves `~/theuth-build/<sha>/` for inspection.

On the Ubuntu host directly (what the driver runs, for a hands-on repro):

```
cd ~/theuth-build/<sha> && scripts/package-linux.sh --check
THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh
THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh
ls -l target/release/dist/                               # via the symlink → ~/theuth-build/target/release/dist/
tar -tzf target/release/dist/theuth_<ver>_linux-x86_64.tar.gz   # t, theuth-app-parser-<hash>, theuth-documents-<ver>
dpkg-deb -c target/release/dist/theuth_<ver>_linux-x86_64.deb | grep usr/bin/
```

On the Windows host directly:

```
pwsh -NoProfile -File scripts\package-windows.ps1        # in %USERPROFILE%\theuth-build\<sha>\
dir %USERPROFILE%\theuth-build\target\release\dist\      # theuth_<ver>_windows-x86_64-setup.exe, theuth_<ver>_windows-x86_64.zip
tar -tf %USERPROFILE%\theuth-build\target\release\dist\theuth_<ver>_windows-x86_64.zip   # t.exe + two helper .exe
```

Then on the Mac, with the real macOS payloads in `dist/` (or the fake-dist recipe in the report):

```
THEUTH_S3_DRY_RUN=1 THEUTH_SKIP_LINUX_SMOKE=1 scripts/release-s3.sh   # gates 6/8 list the dist/ files; latest.json shows the 7 keys
```

## Not done / open

- Real Windows/Linux runs: not possible from the Mac; dry runs and a fake-dist `release-s3.sh` run
  are in `/tmp/enh-2026-09-22/build-remote-report.md`.
- `scp` in sftp mode is assumed (OpenSSH ≥ 9.0 on the Mac, sftp subsystem on Windows OpenSSH —
  both defaults). If a host runs a legacy scp-only server, the `[4/5]` glob still works because the
  legacy protocol expands it remotely.
- `release-s3.sh` summary still prints `${#UPLOADS[@]}` on the resume path (pre-existing; unset there).
- The Homebrew tap templates are untouched: Windows/Linux desktop artifacts are not tap objects.
