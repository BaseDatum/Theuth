# 58 step 1 (+2's mechanics) — as built, 2026-09-22

Branch `feature/platform-crate`. Scope: `crates/platform` per 58 §3.1, every
caller ported, `meeting` gated to macOS, `desktop` builds its Windows
resources. Behaviour on macOS/Linux is the move and nothing else.

## What moved where

| Was | Now |
|---|---|
| `tools::jobs` / `tools::pty`: `nix::killpg`, `Pid`, pgid `Vec<i32>` | `platform::ProcessTree` (Unix: the process group; Windows: a Job Object with `KILL_ON_JOB_CLOSE`, members assigned as brush's `on_spawn` reports them). `tree.kill()`, `tree.signal(Signal)`, `tree.alive()`; `platform::process::pid_alive` in tests |
| `tools::pty`: `$SHELL -lc` | `platform::pty_command(command, tools.pty_shell)` → `$SHELL -lc` / `pwsh.exe -NoLogo -Command` (else `powershell.exe`); new config key `tools.pty_shell` |
| `tools::shell::probe_with` (`setsid` pre_exec, 84202be) | `platform::env::login_path(shell, timeout)`; `platform::process::own_session` carries the `setsid`; Windows returns the current `PATH` |
| `tools::mcp::expand`, `schedule::until_check`: `$SHELL -c`/`-lc` | `platform::shell().command(script, login)`; `ShellKind::Brush` (Windows) is `None` today → MCP keeps the string as written with a warning, `until:` errors — brush in-process is 58 §11 step 3 |
| `0600` sites: `tools::mcp::oauth`, `secrets::key`, `secrets::vault`, `link::identity`, `rpc::load_token`, `oauth::Store::save` | `platform::files::{write_private, create_private}` (temp+rename, `0600`; owner-only DACL + sharing-violation retry on Windows). `rpc`'s token is now created private instead of write-then-chmod; `secrets.vault`'s temp file is `secrets.vault.tmp` (was `secrets.tmp`) |
| `agent::private::Scratch` `DirBuilder.mode(0o700)` | `platform::files::private_dir` (recursive) |
| `update`: `parent_id()==1 \|\| INVOCATION_ID`, `chmod 755`, `rename` over the running `t` | `platform::service::managed()`, `files::set_executable`, `files::replace_executable` (`self-replace` on Windows when `dest` is the running image, plain rename otherwise) |
| `theuth::service`: launchd / systemd code, `#[cfg(target_os)]` pairs | `platform::service::ServiceUnit::{LaunchAgent, SystemdUser, TaskScheduler, None}` with `install`/`uninstall`; `theuth` keeps only the argv (login shell on Unix, `t.exe --headless` directly on Windows) and the banner |
| `relay`: `tokio::signal::unix::signal(terminate)` | `platform::process::stop_signal()` (SIGTERM; `ctrl_close`/`ctrl_shutdown` on Windows) |
| `open`/`xdg-open` in `oauth`, `tui`, `computer` | `platform::open::{open_url, open_path, open_app}` (`ShellExecuteW` on Windows) |
| `models::onnx::LIB` | `= platform::ort::ort_library_name()` |
| `desktop::models::TARGET_TRIPLE` | `platform::target::target_triple()` |
| `desktop::update::writable` (`nix::access`) | `platform::files::writable` |
| `desktop::launch`: `$SHELL -l -i -c 'exec t …'` on the pty | `daemon_command`: the same on Unix; `t.exe --headless --data-dir …` directly on Windows (ConPTY via portable-pty); `cli_bin` uses `EXE_SUFFIX` |
| `meeting` in `theuth` | `[target.'cfg(target_os = "macos")'.dependencies]` + `#[cfg(target_os = "macos")]` on the `plugins()` push (58 §3.2) |
| — | `crates/desktop/icons/icon.ico` (tauri-build's Windows resource) |

`nix` is gone from the workspace. `libc` stays in `platform` (unix) and in
`tools` (unix, the `O_NONBLOCK` open in `images.rs`). New crates.io deps:
`windows-sys 0.61` (features: Foundation, Security, Security_Authorization,
Storage_FileSystem, System_Console, System_JobObjects, System_Threading,
UI_Shell, UI_WindowsAndMessaging) and `self-replace 1.5`, both
Windows-only.

## Checkout prerequisites (every host)

`cargo check --workspace --all-targets` needs `bindings/methods.json`
(gitignored): `crates/nodes/src/client.rs` `include_str!`s it in its lib
test. Generate it first with `just ts-desktop` (or `cargo test -p rpc
--features ts export_bindings`). Environment, not code — a fresh Windows
clone fails the `nodes` lib test without it.

## Windows implementations — unverified from the Mac

Each carries `// ponytail: unverified on Windows — <check>` at the module:

- `process.rs`: Job Object per tree, `TerminateJobObject(137)` for kill,
  `GenerateConsoleCtrlEvent(CTRL_C_EVENT, leader)` for `interrupt()`,
  `QueryInformationJobObject(ActiveProcesses)` for `alive()`. Check: 58 §11
  step 2 (`shell("timeout 30")` tree dead after cancel).
- `files.rs`: owner-only DACL (`SetNamedSecurityInfoW` with a protected
  DACL granting the token's user), `MoveFileExW(REPLACE_EXISTING)` with a
  short retry on `ERROR_SHARING_VIOLATION`; `self_replace` when the
  destination is the running image. Check: 58 §11 step 5 (`icacls`).
- `open.rs`: `ShellExecuteW(open)`. Check: step 4 (OAuth login opens the
  browser).
- `service.rs`: `schtasks /create /f /sc onlogon /rl limited /tn theuth /tr
  "<argv>"`, `/run`, `/end`, `/delete`. Check: step 4.
- `shell.rs`: `pwsh.exe` on `PATH` else `powershell.exe`, `-NoLogo -Command`.
- `env.rs`: `login_path` returns the current `PATH`.

## Judgement calls

- `ShellKind::expand()` (58 §3.1) is not in `platform`: it needs brush
  in-process, which is step 3 and would pull `brush_core` into the leaf.
  `ShellKind::command` returns `None` for `Brush`; the two callers say so.
- `agent::providers::atomic_write` keeps its `mode` parameter and `cfg(unix)`
  body: its sole caller writes `providers.yaml` at `0644`, not a private
  file; `write_private` would tighten it. Left as is, compiles on Windows.
- `theuth::service::is_executable` keeps a `cfg(unix)` pair (the `-i`
  decision needs the mode bits; Windows never reaches it).
- `computer_open`'s test no longer asserts the two OK arms: with
  `platform::open` the tool body *launches* the target, so only the argument
  errors are testable without side effects.
- Job Object members are assigned after spawn (brush reports the pid from
  `Child::id()`); a grandchild forked in that window escapes the job. Step 2
  measures whether that matters; the fix is `CREATE_SUSPENDED` in brush's
  spawner.

## Open items for step 2

- ConPTY: `pty_command` runs `pwsh -Command`; readiness regexes, `send`
  keys and `screen` colours against ConPTY output are unmeasured.
- `interrupt()` on a ConPTY child: it shares no console with us, so
  `GenerateConsoleCtrlEvent` is expected to fail; `send` should write `0x03`
  on the pty input instead.
- brush + uutils on `PATH` (step 3); `ShellKind::Brush` callers.
- `platform::service::managed()` is `false` on Windows until Task Scheduler
  start is verified (the updater never exits for restart there).
