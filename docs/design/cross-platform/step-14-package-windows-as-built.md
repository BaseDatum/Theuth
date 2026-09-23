# 58 step 14 — Windows packaging (NSIS installer + CLI zip), as built, 2026-09-23

Branch `feature/package-windows` off `1a81f57`. Scope: `scripts/package-windows.ps1` (PowerShell 7,
runs on the Windows host, `just package-windows`), `crates/desktop/tauri.windows.conf.json`, one
justfile recipe, this note. `crates/models/src/onnx.rs` is unchanged (see "ORT search order").
Written on the Mac without a Windows box: the script parses and passes PSScriptAnalyzer, its pure
PowerShell pieces (version/hash/env parsing, config rendering, zip member check, `Exec` exit-code
propagation, ORT zip fetch + sha256 + member extraction) ran under pwsh 7.6 on macOS; the cargo, bun
and Tauri steps run for the first time on the Windows host. `cargo check -p desktop --target
x86_64-pc-windows-msvc` from the Mac (with Homebrew's `llvm-rc` on PATH for tauri-winres) gets
tauri-build past the merged config and the `.ico` resource; it stops at `ring` / `aws-lc-sys` C
build scripts, as expected.

## Artifacts (`target/release/dist/`, the contract both packaging omps build to)

| file | content |
|---|---|
| `theuth_<ver>_windows-x86_64-setup.exe` | Tauri NSIS installer, unsigned. No MSI. |
| `theuth_<ver>_windows-x86_64.zip` | exactly `t.exe`, `theuth-app-parser-<hash>.exe`, `theuth-documents-<ver>.exe` (flat, no directory) |

`<ver>` is `crates/theuth/Cargo.toml`'s version; the script refuses to run if
`crates/desktop/tauri.conf.json` disagrees. `.sig` files are made by `release-s3.sh` on the Mac
(`cargo tauri signer sign`), like the Linux tarballs; `latest.json` keys `windows-x86_64` and
`cli-windows-x86_64` are that script's business.

## Installed layout

`installMode: currentUser` → `%LOCALAPPDATA%\theuth\` (Tauri's NSIS template: `$LOCALAPPDATA\${PRODUCTNAME}`;
`RequestExecutionLevel user`, no UAC prompt, so the in-app updater can replace it without elevation).

```
%LOCALAPPDATA%\theuth\
  theuth-desktop.exe                       main binary (tauri.conf.json mainBinaryName)
  t.exe                                    externalBin binaries/t → launch.rs::cli_bin (beside the exe)
  theuth-app-parser-<hash>.exe             externalBin → app::helper_path (beside t.exe; std Command appends .exe)
  theuth-documents-<ver>.exe               externalBin → document::helper::helper_path (same)
  onnxruntime.dll                          resource → models::onnx::find_ort_lib probe #2, <exe dir>\onnxruntime.dll
  uninstall.exe
```

Start-menu and desktop shortcuts to `theuth-desktop.exe`; Add/Remove Programs row under HKCU.
App data is Tauri's `%LOCALAPPDATA%\io.theuth.mac` (the identifier is shared with macOS — not this step's call);
the daemon's data dir is `~/.theuth` as on every platform.

## NSIS choices (`crates/desktop/tauri.windows.conf.json`)

- `targets: ["nsis"]` — no MSI (contract). Tauri auto-merges the platform file on Windows builds,
  as `tauri.linux.conf.json` / `tauri.macos.conf.json` already do.
- `installMode: currentUser` — per-user, passwordless updates. `both` would add an install-mode
  page and the multi-user registry dance for no user benefit tonight; `perMachine` would need admin
  for every update.
- `webviewInstallMode: downloadBootstrapper, silent` — the installer fetches the WebView2 bootstrapper
  (≈2 MB download at install time) only when the runtime is missing. Windows 11 ships WebView2, so
  this is a no-op there; `embedBootstrapper`/`offlineInstaller` would add 2–150 MB to every download.
- `icon` overlay lists `icons/icon.ico` (the base config only has PNG + icns; JSON merge replaces the
  array, so `tauri.conf.json` is untouched). `installerIcon: icons/icon.ico`.
- Unsigned: no Authenticode certificate. SmartScreen will warn on first run ("unknown publisher");
  `/S` silent installs do not trigger it. Signing is a follow-up (a cert plus `bundle.windows.signCommand`).
- `createUpdaterArtifacts: false` stays: the updater consumes the raw `-setup.exe` + detached `.sig`
  (tauri-plugin-updater runs an NSIS installer in its silent update mode); the `.sig` is release-s3.sh's.

## How the bundle carries the pieces

- **Sidecar + helpers = externalBin**, rendered into a config file at build time
  (`target/tauri.windows.bundle.conf.json`, passed as `cargo tauri build --config <path>`) exactly as
  `package-linux.sh` renders its inline JSON: the helper names carry the library hash / protocol
  version, so they cannot live in a committed config, and tauri-build would fail `cargo check`
  without the files. Tauri appends `-x86_64-pc-windows-msvc.exe` when looking up
  `binaries/<name>` and installs each as `<name>.exe` in `$INSTDIR` — beside `t.exe`, which is where
  `helper_path` looks. A file, not inline JSON, so the config never crosses Windows argument quoting.
- **onnxruntime.dll = resource** `{ "binaries/onnxruntime.dll": "onnxruntime.dll" }`. Tauri's NSIS
  template installs resources with `SetOutPath $INSTDIR` + `File /oname=<target>`, so a single-file
  mapping to a bare filename lands in the install dir root (verified against tauri-utils 2.9.3
  `resources.rs` and tauri-bundler's `installer.nsi`). externalBin was rejected for the dll because
  it appends `.exe`. Source: Microsoft's `onnxruntime-win-x64-<ORT_VERSION>.zip` (CPU build;
  `ORT_SHA256_WIN_X64` from `scripts/ort-version.env`), cached at `target/onnxruntime-win-x64-<v>.zip`,
  sha256-checked, one member extracted. The WebGPU plugin DLLs are **not** bundled — the app downloads
  them from the models bucket (step 13).

## ORT search order (`crates/models/src/onnx.rs`, unchanged)

`find_ort_lib` already probes, for every OS, `<exe dir>/../Frameworks/<LIB>` then `<exe dir>/<LIB>`
**before** `<data_dir>/models/onnxruntime/<LIB>`, then `$ORT_DYLIB_PATH`, then Homebrew. With
`LIB = onnxruntime.dll` on Windows (`platform::ort::ort_library_name`), `t.exe` in the install dir
finds the bundled dll second (`..\Frameworks\onnxruntime.dll` is harmlessly absent). No code change,
no new test: there is no search-order test for macOS to mirror, and the probe is one `is_file` on a
joined path.

## Helper builds on Windows — judgement call

`bun build --compile` appends `.exe` to a suffix-less `--outfile` on Windows. Both
`packages/*/scripts/build-helper.ts` then `copyFileSync(stable, versioned)` the suffix-less name
(and `packages/documents` self-tests with `cwd: "/tmp"`), so `bun run build:helper` exits non-zero
on Windows **after** the compile succeeded. Those scripts are outside this step's allow-list, so
`package-windows.ps1` empties `dist/` first, runs `bun run build:helper`, reports (does not fail on)
its exit code, then requires `dist/<stable>.exe` to exist and applies `app-parser-env.sh`'s checks
itself: `--version` contains the library hash / equals `theuth-documents <ver>`, and the exe exits 0
on stdin EOF. The bundle probe (forbidden-module check) in the TS scripts still runs on Windows before
the compile, so a bad dependency closure still fails the build (no exe). Follow-up, outside this
branch: make `build-helper.ts` suffix-aware (`process.platform === "win32" ? ".exe" : ""`, `os.tmpdir()`).

`HelperStatus.found` (`crates/document/src/helper.rs`) tested `is_file()` on the suffix-less path
and read `false` on Windows. Fixed on `fix/desktop-launch-windows` (below).

## Launch on Windows (`crates/desktop/src/launch.rs`, branch `fix/desktop-launch-windows`, 2026-09-23)

Observed with the installed 1.13.4: launching the app opened a stray console window and the webview
showed `no daemon on 127.0.0.1:62961 after 15 s: your shell startup is waiting for input` over a
transcript that was conhost's pseudo-console handshake (`\x1b[?9001h \x1b[?1004h \x1b[?25h
\x1b]0;…\t.exe \x1b[6n`), not `t` output. Control: `Start-Process t.exe --headless --data-dir …` with
`THEUTH_LAUNCH=1 THEUTH_RPC__LISTEN=127.0.0.1:62962` and no pty listened after 528 ms. So the daemon
was fine and the ConPTY launch (`portable-pty` `openpty` + `spawn_command`) was the failure.

As built:

- **No pty on Windows.** `spawn` is `#[cfg(windows)]`: `std::process::Command` for
  `t.exe --headless --data-dir <dir>` with `creation_flags(CREATE_NO_WINDOW)` (0x08000000, std's
  `CommandExt`, no new dependency), stdin null, stdout and stderr piped, cwd `$HOME`, the same env as
  the pty path (`TERM`, `THEUTH_LAUNCH=1`, `THEUTH_RPC__LISTEN`). Two drain threads (one per pipe)
  feed the same 64 KiB ring and `launch-log` events, so the transcript UI is unchanged. The pty was
  only ever there to inherit a login shell's environment (18 §2), which Windows has none of (58 §0.3).
- **`Daemon`** keeps one struct: `child` is `Box<dyn portable_pty::Child>` on Unix and
  `std::process::Child` on Windows (`master` is `#[cfg(unix)]`). `shutdown` still waits the 2 s
  grace after the webview's RPC `shutdown`, then `Child::kill` (TerminateProcess). There is nothing to
  hang up on Windows; stdin is already null.
- **Timeout diagnosis.** `wait_ready` now strips CSI/OSC/DCS/PM/APC sequences and C0 bytes from the
  transcript (`strip_controls`) and only says "your shell startup is waiting for input" on Unix when
  the stripped text ends in a partial line (`Password: `, `(y/n) `). Every timeout message quotes the
  last five transcript lines. Unit tests pin the conhost transcript above to *not* trigger it.
- **Helper presence.** `app::helper_path` and `document::helper::helper_path` append
  `std::env::consts::EXE_SUFFIX`, so `is_file()` (which gated the spawn as well as
  `HelperStatus.found`) sees `theuth-app-parser-<hash>.exe` / `theuth-documents-<ver>.exe`.

Not verified on Windows from the Mac; `cargo check -p desktop --target x86_64-pc-windows-msvc` stops at
the C build scripts before the desktop crate. The `cfg(windows)` code was reviewed by eye.

## Verifying on the Windows host

From the repo root in pwsh 7 (rustup MSVC toolchain, `cargo install tauri-cli`, bun, NSIS is fetched
by Tauri):

```powershell
pwsh -NoProfile -File scripts/package-windows.ps1      # or: just package-windows
dir target\release\dist
# expect: theuth_<ver>_windows-x86_64-setup.exe, theuth_<ver>_windows-x86_64.zip, sizes printed by the script

# silent per-user install, then the layout
.\target\release\dist\theuth_<ver>_windows-x86_64-setup.exe /S
dir $env:LOCALAPPDATA\theuth
# expect: theuth-desktop.exe t.exe theuth-app-parser-<hash>.exe theuth-documents-<ver>.exe onnxruntime.dll uninstall.exe

# the CLI from the install dir, and the bundled runtime being the one loaded
& "$env:LOCALAPPDATA\theuth\t.exe" --version
& "$env:LOCALAPPDATA\theuth\t.exe" --bench-models
# --bench-models prints the ONNX Runtime path it loaded (bench_models.rs) - expect %LOCALAPPDATA%\theuth\onnxruntime.dll;
# with no ~/.theuth/models/onnxruntime/onnxruntime.dll present, running at all proves the beside-exe probe.

# the zip
Expand-Archive target\release\dist\theuth_<ver>_windows-x86_64.zip C:\tmp\theuth-cli
C:\tmp\theuth-cli\t.exe --version
C:\tmp\theuth-cli\theuth-app-parser-<hash>.exe --version
C:\tmp\theuth-cli\theuth-documents-<ver>.exe --version

# uninstall
& "$env:LOCALAPPDATA\theuth\uninstall.exe" /S
```

`THEUTH_SKIP_BUN_INSTALL=1` skips the three `bun install`s on reruns.

## Not done / follow-ups

- Authenticode signing (installer + exes): needs a certificate; `bundle.windows.signCommand`.
- `build-helper.ts` Windows suffix bug (above) — other owner. `HelperStatus.found` is fixed (Launch on Windows).
- `latest.json` `windows-x86_64` / `cli-windows-x86_64` rows and `.sig` — `release-s3.sh` (other omp).
- Identifier `io.theuth.mac` on Windows (app-data folder name) — cosmetic, not touched.
