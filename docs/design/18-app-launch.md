# 18 — App launch: a GUI starts the daemon with the terminal's environment

Status: verified in the app · 2026-09-15 (draft 2026-09-13) · extends `00-architecture.md` §2, §7, §12 row 4
(Tauri frontend); `01-config-and-subagents.md` §1.4 (MCP shell expansion).
Mechanism verified on this machine (§6) and then through `crates/desktop`
(25 §12: the daemon spawned by the `.app` under launchd's environment saw
the `~/.zshrc` exports and `PATH`).

## 0. The question in one screen

A Tauri `.app` (or any GUI) needs a running `wth --headless` to talk to over
`ws://127.0.0.1:<port>` with `<data_dir>/token`. That daemon must be **the
same process a user gets by opening a terminal and typing `wth`**: same
`PATH`, same exports, same certificates, same tty-dependent variables. Not
approximately - the daemon spawns tools, MCP servers and `$(gh auth token)`
subshells (01 §1.4), and each of those resolves binaries and reads variables
from the daemon's environment. Anything missing there is a bug report of the
form "works in the terminal, not in the app".

Constraints: no visible shell window, no configuration the user maintains,
no second copy of the environment to keep in sync.

## 1. Why the app's environment is wrong

An app launched from Finder, Dock or Spotlight is a child of `launchd`, not
of a shell. Its environment is what `launchd` gives a user session:
`HOME`, `USER`, `SHELL`, `TMPDIR`, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`,
a handful of Apple variables. None of `~/.zprofile`, `~/.zshrc`, Homebrew's
`shellenv`, rbenv/nvm/mise shims, `NODE_EXTRA_CA_CERTS`, `EDITOR`,
`GPG_TTY`. On Linux the desktop session is similar (systemd user manager
environment ≠ shell rc files).

Measured here (`/bin/zsh`, oh-my-zsh, `.zprofile` 139 B, `.zshrc` 6.8 KB):

| variable | `zsh -l -c env` (login only) | `zsh -l -i -c env` on a pty (what a terminal runs) |
|---|---|---|
| `PATH` | `/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:…` | `~/.rbenv/shims:~/.atuin/bin:/opt/homebrew/bin:/usr/local/sbin:…:~/.cargo/bin:~/.docker/bin:…` |
| `NODE_EXTRA_CA_CERTS` | `/usr/local/share/enterprise-certs/cacert.pem` | `/Users/Sky.Moore/cacert.pem` (the `.zshrc` override) |
| `EDITOR` | `true` | `vim` |
| `GPG_TTY` | `not a tty` | `/dev/ttys000` |
| `FPATH` | site-functions only | oh-my-zsh plugin dirs |

So the common shortcut - "run `$SHELL -l -c` once and copy `PATH`" (jarvis
`probe_login_shell_path`, VS Code's `shellEnv`, Zed's
`load_login_shell_environment`) - is wrong on this machine in four of five
rows. The `.zshrc` is where the environment actually lives, and `.zshrc` is
sourced only by interactive shells; some of its content (`GPG_TTY=$(tty)`,
ssh-agent, prompt tooling) additionally wants a controlling terminal.

## 2. Decision: be a terminal for one command

The app does not reconstruct the environment. It does what a terminal
emulator does, minus the window:

1. Open a pseudo-terminal (`openpty`; in Rust `portable-pty`, the crate
   WezTerm and Zed use). The slave side is the child's stdin/stdout/stderr
   and controlling tty; the master side stays with the app.
2. On that pty, run the user's shell as a **login, interactive** shell with
   a single command that replaces the shell:

   ```
   $SHELL -l -i -c 'exec wth --headless --data-dir <dir>'
   ```

   Environment handed to the shell: the app's own (launchd's) plus
   `TERM=xterm-256color`, `WTH_RPC__LISTEN=127.0.0.1:<port>` (the `WTH_<SECTION>__<KEY>` env
   override, 01 §1; the port the app chose). `$SHELL` comes from `passwd`
   (`SHELL` in the launchd env is set from it).
3. The shell sources exactly what a new terminal tab would (`zsh`:
   `.zshenv`, `.zprofile`, `.zshrc`, `.zlogin`; `bash -l -i`:
   `.bash_profile` and, if it chains it, `.bashrc`; `fish -l -i`:
   `config.fish`), then `exec` replaces the shell process with `wth`. No
   fork: the daemon **is** the process the shell produced - same
   environment, cwd (`$HOME`), umask, resource limits, controlling tty.
4. The app drains the pty master into a startup log (rc-file output such
   as `Identity added: …` from ssh-agent, `Starting ssh-agent …`, prompt
   tool banners). Nothing is parsed.
5. The app connects to `ws://127.0.0.1:<port>` with `<data_dir>/token`
   (RPC plugin, 00 §7) like every other client, retrying until the port
   accepts. Readiness is the port, not the log.

The daemon then spawns `shell`, MCP servers, ACP harnesses and the 01 §1.4
`$(…)` subshells from that inherited environment, so `gh`, `uvx`, `npx`,
`kubectl` resolve to the same binaries a terminal would, with the same
`KUBECONFIG`, `AWS_PROFILE`, CA bundles.

## 3. Lifecycle

- **App quits → daemon exits.** Closing the pty master sends `SIGHUP` to
  the session; `wth` has no handler for it (`main.rs` selects on `ctrl_c`
  only), so the default action terminates it, and the `Jobs` actor's
  process-group kill (14 §2) never runs. Acceptable for v0: the data dir
  holds every session; the next launch resumes. If orphaned tool processes
  turn out to matter, add `SIGHUP`/`SIGTERM` to the `main.rs` select so
  shutdown runs; that is the fix, not detaching the daemon. Quitting the
  app cleanly should still prefer `shutdown` over RPC first, then close
  the pty.
- **Daemon dies → app notices** the WebSocket close and the pty EOF; it
  shows the pty log tail (the shell's or `wth`'s last words) and offers
  relaunch. No auto-restart loop in v0.
- **Second app instance / a terminal `wth` already running** on the same
  data dir: nothing enforces one process per data dir today. The app
  should first try the configured port with the token; a live daemon is
  reused, not duplicated. `sessions.sqlite` is WAL and survives two
  writers, but two agents on one bus do not share state.
- **Port.** The app picks a free loopback port per launch and passes it
  as `WTH_RPC__LISTEN`; fixed `7777` collides with a terminal daemon.

## 4. Hazards, named

All come from running the user's real rc files, which is the point.

- **An rc file that waits for input** (`ssh-add` asking for a passphrase,
  `read`, a first-run wizard) blocks the shell before `exec`. The pty is
  a real tty, so the prompt is written there and waits forever. Mitigation:
  a readiness timeout (port not accepting within ~15 s) surfaces the pty
  transcript in the app with the message "your shell startup is waiting
  for input" - the user sees exactly what asked. The transcript is the
  diagnostic; do not try to answer prompts. Here: oh-my-zsh `ssh-agent`
  adds the key from the agent (`Identity added:` printed, no prompt),
  0.4 s.
- **An rc file that execs into something else** (`exec tmux`, oh-my-zsh
  `ZSH_TMUX_AUTOSTART=true`). The `-c` command never runs; tmux owns the
  pty. Same timeout, same transcript; the fix is the user's (`[[ -o
  interactive ]] && [[ -z "$WTH_LAUNCH" ]]` guards are conventional). The
  app sets `WTH_LAUNCH=1` in the child environment so a user can guard on
  it; nothing in `wth` reads it.
- **Slow rc files.** A terminal user pays it on every tab; the app pays it
  once per launch. Measured 0.4 s here. Not a design problem.
- **`SHELL` unset or not executable** (deleted Homebrew shell): fall back
  to `/bin/zsh` on macOS, `/bin/sh -l -c` elsewhere, and say so in the
  log.
- **Windows.** No launchd/rc-file split of this kind; PowerShell profiles
  run for `-Interactive` sessions only. Out of scope until there is a
  Windows build; the same shape applies (`pwsh -NoExit -Command "& wth"`
  on a ConPTY).

## 5. Rejected

- **Probe a shell, capture `env -0`, re-apply to the child** (VS Code,
  Zed, jarvis `probe_login_shell_path`): a second parse step; misses the
  tty-dependent variables unless the probe also has a pty, at which point
  it is this design plus a marker protocol; drifts whenever the shell does
  something `env` cannot show (functions, `ulimit`, `umask`). `exec` gives
  the whole state for free.
- **`launchctl setenv PATH …` / a LaunchAgent plist with a copied `PATH`**:
  a snapshot the user maintains by hand; wrong the day they install a new
  toolchain.
- **Ship the desktop app's daemon as a LaunchAgent (`launchd` service) the
  app talks to**: same environment problem, plus lifecycle the user cannot
  see. This rejection is scoped to the app's daemon. The background service
  this bullet reserved is 32 §5.1 (`wth --install-service`, a node on a
  server nobody sits at): its unit runs the same `$SHELL -l -i -c 'exec wth
  --headless'` command line, as §2 requires, with the pty dropped — under a
  service the journal / `<data_dir>/logs/service.log` is the transcript,
  and the systemd unit's `StandardInput=null` makes an rc file that waits
  for input fail fast instead of hanging (§4).
- **Open a real Terminal/Ghostty window running `wth --headless`**: works,
  visibly clunky, and depends on which terminal is installed and its
  AppleScript/CLI surface.
- **`$SHELL -l -c` without `-i`**: §1 table.
- **A user-editable `env:` block in `config.yaml`**: 01 §1 already has
  `WTH_<SECTION>__<KEY>` overrides for wth's own settings; a general env
  block would be the snapshot again.

## 6. Verification

Stand-in for the app (`python3`, `pty.fork`): environment reduced to
`HOME`, `USER`, `SHELL=/bin/zsh`, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`,
`TMPDIR`, `TERM`, `WTH_RPC__LISTEN=127.0.0.1:7802`, `RUST_LOG`; child
`zsh -l -i -c 'exec target/debug/wth --headless --data-dir /tmp/wth-exp'`.
After 20 s, `ps eww -p <pid>` on the daemon (same pid as the shell - the
`exec` held):

```
PATH=/Users/Sky.Moore/.rbenv/shims:/Users/Sky.Moore/.atuin/bin:/opt/homebrew/bin:/usr/local/sbin:/usr/local/bin:…
NODE_EXTRA_CA_CERTS=/Users/Sky.Moore/cacert.pem
GPG_TTY=/dev/ttys000
EDITOR=vim
```

pty transcript: two lines (`Identity added: …`, a job-control line from
the ssh-agent plugin). The daemon's MCP reconcile connected a stdio server
whose entry used `env.TOKEN: "t-$(printf 'gh auth'|wc -w|tr -d ' ')-${USER}"`
and an arg `arg-$(printf sub)-${HOME}`; the child saw
`TOKEN=t-2-Sky.Moore ARG=arg-sub-/Users/Sky.Moore`, i.e. the 01 §1.4
expansion ran in the inherited environment. `SHLVL` is `0` after the exec
(zsh decrements before exec) - cosmetic, noted so nobody "fixes" it.

Not exercised: bash and fish as `$SHELL`; a blocking rc file against the
timeout path (§4); Linux desktop launch; the Tauri sidecar bundling itself
(`Contents/MacOS/wth`, `tauri.conf.json` `externalBin`).

## 7. Build order (when the Tauri phase starts)

1. `crates/launch` (or a module in the Tauri crate): `spawn(data_dir,
   port) -> Launched { pty_master, pid }` using `portable-pty`; ~40 lines.
   One test: spawn `$SHELL -l -i -c 'exec env'` on a pty, assert `TERM` and
   `WTH_LAUNCH` present.
2. Tauri: startup screen streams the pty transcript until the port
   accepts; on timeout, keeps the transcript visible with the §4 message.
3. Quit: RPC `shutdown`, then close the master.
