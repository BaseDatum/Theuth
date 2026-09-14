# 22 — PTY jobs: interactive processes the model can drive

Status: draft v0 · 2026-09-14 · extends `14-jobs.md` (jobs actor, log
files, `<job_result>`), `00-architecture.md` §6 (tools). Where this doc
disagrees with 14 it says so and wins; §10 lists the edits. Not started.

## 0. The problem

A job (14) is a process on two pipes: the model can start it, read its log,
and kill it. It cannot type into it, and anything that redraws the screen
(a REPL prompt, `vim`, a TUI, a progress bar, `sudo`'s password prompt, a
`git rebase -i` editor) is either dead on arrival — no controlling
terminal, `/dev/null` on fd 0 (00 §6) — or a `\r`-soup in the log. The
model has no way to run a REPL across several calls, drive an installer,
answer a `[y/N]`, or test a terminal program it just wrote.

oh-my-pi has this as the `hub` tool's process ops (scout report
2026-09-14, `agent://OmpPty`): a per-project broker process holding
`portable-pty` sessions; the model `start`s a named process with a PTY
(120×40, `TERM=xterm-256color`), `send`s text/keys/signals, `logs` a
byte-cursor'd, ANSI-stripped log, and `wait`s on a regex or readiness. Its
one real gap: the model only ever sees stripped concatenated bytes — a
headless terminal exists but feeds the TUI card, so a redrawing program
reads as noise to the model.

## 1. Decisions in one screen

1. **A PTY job is a job.** Same actor (`Jobs`, 14 §2), same `j{n}` id,
   same log file, same `jobs` / `cancel_job`, same `<job_result>` when it
   exits, same kill on shutdown. It differs in three things: it runs on a
   pty instead of pipes, it has stdin, and the harness keeps a rendered
   screen for it. Nothing else in 14 changes.
2. **The model sees the screen, not the bytes.** Every reply from the new
   tools ends with the current 120×40 screen (blank rows dropped,
   trailing spaces trimmed, cursor position on the last line). The raw
   bytes still go to the log file for `read_file` — right for
   line-oriented output, wrong for a TUI; the description says which is
   which.
3. **One round trip per interaction.** `send` returns the screen once the
   output has settled (no new bytes for 300 ms, at most 2 s), so "type
   `1+1`, read `2`" is one tool call. omp needs `send` then `logs`.
4. **Four small tools:** `start`, `send`, `screen`, `wait`. Listing and
   killing stay `jobs` / `cancel_job`. No `logs` (the log file and
   `screen` cover it), no `resize`, no `restart`, no `ps`.
5. **`$SHELL -lc <command>` on the pty, not brush.** brush is an in-process
   shell; a pty child needs `setsid` + `TIOCSCTTY` in the child, which is
   what `portable-pty`'s spawn does. The session shell's cwd is the
   default cwd; its variables and functions do not carry over and the
   job's `cd`/`export` do not come back (14 §0.5 last-finished-wins is
   pipes-only). The description says so in one sentence.
6. **No deadline.** `job_timeout_secs` (14 §0.7) catches a runaway nobody
   is watching; a PTY job is by definition one the model asked to keep
   (a server, a REPL). It dies on `cancel_job`, `/jobs kill`, or shutdown.
7. **Readiness is part of `start`.** `ready: { log?: regex, port?: u16,
   timeout?: secs }` makes `start` block until the regex matches the
   output so far and/or the port accepts a TCP connect — or the process
   exits, or the timeout (default 30 s) — and say which. Without `ready`,
   `start` waits the 14 grace (3 s) and returns the screen. Dev servers
   are the first use and "started" without readiness is the first thing
   models get wrong.
8. **The harness answers terminal queries.** A headless pty deadlocks any
   program that asks the terminal a question and waits: `ESC[6n`
   (cursor position), `ESC[5n`, `ESC[c` / `ESC[>c` (device attributes),
   OSC 10/11 (colours). The reader answers from the vt100 state (real
   cursor row/col, not omp's fixed `1;1`) and never logs the reply.
9. **Fixed 120×40, `TERM=xterm-256color`.** Big enough for anything the
   model will drive; resize is a later op if a transcript needs it.
10. **In-process.** wth is one daemon already (00 §2); `Jobs` is the
    broker, RPC is the socket, the bearer token is the auth. omp's
    separate broker process, project-scoped names, `persist`/`detached`
    tiers and restart back-off are not needed by a single-process harness
    and are not built.

## 2. Model-facing contract

### 2.1 Tools

```rust
// start — Risk::Destructive (the command string, like `shell`)
struct StartArgs {
    command: String,                 // run by `$SHELL -lc` on a pty
    name: Option<String>,            // label; `id` below accepts it in place of `j{n}`; unique among running jobs of the session
    cwd: Option<PathBuf>,            // default: the session shell's cwd
    env: Option<BTreeMap<String, String>>,
    ready: Option<Ready>,            // block until ready (§1.7)
}
struct Ready { log: Option<String>, port: Option<u16>, host: Option<String>, timeout: Option<u64> }

// send — Risk::Mutating (types into a process)
struct SendArgs {
    id: String,                      // `j4` or the `name`
    text: Option<String>,
    enter: Option<bool>,             // default true: append `\r` after `text`
    keys: Option<Vec<Key>>,          // after text: enter tab escape ctrl_c ctrl_d ctrl_z ctrl_l up down left right home end page_up page_down backspace delete
    signal: Option<Signal>,          // int term hup quit kill → killpg
}

// screen — Risk::ReadOnly
struct ScreenArgs { id: String }

// wait — Risk::ReadOnly
struct WaitArgs {
    id: String,
    pattern: Option<String>,         // regex over output since this call started; absent → wait for exit
    timeout: Option<u64>,            // seconds, default 30, max 120
}
```

Descriptions, one line each, in the tool JSON (cache-safe, static):

- `start`: "Run a command on a pseudo-terminal as a named job you can type
  into (REPLs, servers, TUIs, prompts). Returns the job id and the screen;
  with `ready`, waits for a log regex and/or a TCP port first. cwd defaults
  to the shell's; variables and functions do not carry over. Use `shell`
  for anything that just runs and exits."
- `send`: "Type into a PTY job: `text` (+ Enter unless `enter: false`),
  then `keys`, or deliver a `signal`. Returns the screen once output
  settles."
- `screen`: "The current screen of a PTY job (120×40, blank rows dropped,
  cursor on the last line). For line output the log file under
  `read_file` is the full history."
- `wait`: "Block until a PTY job prints something matching `pattern`, or
  exits; at most `timeout` seconds (default 30). Returns the match and the
  screen. Use it for prompts and readiness, not to babysit a build —
  end the turn and the `<job_result>` wakes you."

### 2.2 Replies

`start` (running):

```
job j4 (py) · running · pid 41822 · ready: log matched `>>> `
log: /Users/x/.wth/jobs/<session>/j4.log
screen:
Python 3.13.2 (main) [Clang 17.0.0]
>>> 
cursor: row 2 col 5
```

`ready` timed out → `not ready after 30s (waiting on: port 5173)` on the
first line, `is_error: false` (the process is still running; the model
decides). Exited during `start` → today's finished shape (`…\nexit code:
1`), `is_error` by code — that is the 14 grace path, `inline: true`.

`send` / `screen` / `wait`: `job j4 (py) · running · 41s` then `screen:` and
the rows; `wait` adds `matched: ">>> "` or `timed out after 30s`
(`is_error: false`) or `exited 0` (the `<job_result>` follows as usual).
`structured: { "job": "j4", "rows": [...], "cursor": [row, col], "matched": ... }`.

`<job_result>` (14 §1.2) is unchanged; the output body for a PTY job is
the **final screen**, not the log (the log of a TUI is unreadable, and the
model has been watching the screen all along). The `pty="true"` attribute
is added so the model knows which it is reading.

### 2.3 `jobs` table

The `jobs` tool / `/jobs` row gains the name and a pty marker:
`j4  running  41s  pty py: python3 -q  | >>> `. The tail column is the
last non-blank screen row for PTY jobs.

### 2.4 Cancel

`cancel_job j4` / `/jobs kill j4` → `SIGKILL` to the process group as
today, then the master is dropped (closes the slave; anything that
escaped the group gets `SIGHUP`). `send { signal: int }` is the polite
version and the one the description recommends first.

## 3. Crates

| Need | Crate | Why this one |
|---|---|---|
| open pty, spawn on the slave, read/write master, resize | `portable-pty` 0.9 | wezterm's; 15 M downloads; macOS/Linux (`openpty` + `TIOCSCTTY`), ConPTY for a later Windows; exactly what omp's native `PtySession` wraps. Blocking I/O — one reader thread per job. |
| rendered screen | `vt100` 0.16 | 11 M downloads; `Parser::process(&[u8])`, `Screen::contents()` / `rows()` / `cursor_position()` / `alternate_screen()`; scrollback. The piece omp lacks for the model. |

Rejected: `pty-process` (unix-only, thinner, one maintainer; `portable-pty`
is what the dominant terminal ships), `expectrl` (expect-style
send/expect scripts — right for tests, wrong for a persistent process the
model steers across turns), `alacritty_terminal` (a full emulator with
its own event loop; we need a grid), `avt` (asciinema's parser, smaller
and younger than vt100; fine, but vt100 has the diff/scrollback API
already and more eyes). `nix` (already a dep) does `killpg`.

## 4. Implementation (`crates/tools/src/pty.rs`, ~450 lines)

### 4.1 Spawn

```rust
let pair = native_pty_system().openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })?;
let mut cmd = CommandBuilder::new(shell);          // $SHELL, fallback /bin/sh
cmd.args(["-lc", &command]);
cmd.cwd(cwd); cmd.env("TERM", "xterm-256color"); for (k, v) in env { cmd.env(k, v) }
let child = pair.slave.spawn_command(cmd)?;        // setsid + TIOCSCTTY in the child
drop(pair.slave);
let pid = child.process_id();                       // == pgid (new session); recorded for killpg
let reader = pair.master.try_clone_reader()?; let writer = pair.master.take_writer()?;
```

`-l` because the daemon may have launchd's environment (18): a login
shell gives the pty child the same `PATH` the user's terminal has. Not
`-i`: interactive bash would print its own prompt and read the job's
stdin as its own.

### 4.2 Reader thread → job task

`std::thread::spawn`: `read` into a 64 KiB buffer, send `Vec<u8>` on a
bounded `tokio::sync::mpsc` (capacity 64; back-pressure lands in the OS
pty buffer, so a flooding child slows instead of the harness growing).
The job task (14 `run`, PTY variant) on each chunk: scan for terminal
queries (§4.4) and write replies; `parser.process(&chunk)`; append to
`Log` (14, unchanged); `output.notify_waiters()`; publish
`JobEvent::Screen` at most every 500 ms (§5). EOF on the master (child
closed) → `child.wait()` → exit code → the 14 finish path with `output =
screen text`.

Exit detection: `child.wait()` in `spawn_blocking` racing the reader EOF;
whichever first, then reap. `cancel` → `killpg(SIGKILL)` + drop master.

### 4.3 Screen text

```rust
fn screen_text(s: &vt100::Screen) -> (Vec<String>, (u16, u16)) {
    let rows: Vec<String> = s.rows(0, 120).map(|r| r.trim_end().to_owned()).collect();
    let last = rows.iter().rposition(|r| !r.is_empty()).map_or(0, |i| i + 1);
    (rows[..last].to_vec(), s.cursor_position())
}
```

≤ 40 rows × 120 cols = 4.8 KB worst case per reply; no spill needed. The
alternate screen (vim, less) is what `vt100` shows when the program is in
it, which is what the user would see.

### 4.4 Query responder

A 16-byte carry buffer across chunks; on each of these sequences in the
output stream, write the reply to the master and continue (the sequence
itself still goes to the parser, which ignores it):

| Query | Reply |
|---|---|
| `ESC[6n` | `ESC[{row+1};{col+1}R` from `cursor_position()` |
| `ESC[5n` | `ESC[0n` |
| `ESC[c`, `ESC[0c` | `ESC[?62;22c` (VT220 + ANSI colour) |
| `ESC[>c`, `ESC[>0c` | `ESC[>41;0;0c` |
| `OSC 10 ; ? ST`, `OSC 11 ; ? ST` | `OSC 10;rgb:ffff/ffff/ffff ST`, `OSC 11;rgb:0000/0000/0000 ST` |

Replies are not appended to the log. Test: `printf '\e[6n'; read -t 2 -rs
-d R r; echo "${r#*[}"` prints `1;1`.

### 4.5 `send`

`data = text.unwrap_or_default() + (enter ? "\r" : "") + keys.map(bytes).concat()`;
one `write_all` + `flush` on the master writer under the job's `Mutex`.
`signal` → `killpg(pgid, sig)`; `text`/`keys` and `signal` in one call is
allowed (signal after write). Then **settle**: wait for `output` to be
quiet for 300 ms (reset on every chunk) or 2 s total, whichever first;
reply with the screen. Key table:

`enter \r · tab \t · escape \x1b · ctrl_c \x03 · ctrl_d \x04 · ctrl_z \x1a
· ctrl_l \x0c · up \x1b[A · down \x1b[B · right \x1b[C · left \x1b[D ·
home \x1b[H · end \x1b[F · page_up \x1b[5~ · page_down \x1b[6~ · backspace
\x7f · delete \x1b[3~`

(`serde(rename_all = "snake_case")` enum, so an unknown key is a schema
error, not a runtime one.)

### 4.6 `wait`

Snapshot `log.len()` at call start; loop: `output.notified()` / exit /
timeout; on each wake, run the regex over the bytes since the snapshot
(lossy UTF-8, ANSI-stripped with the TUI's existing `strip` helper so
`>>> ` matches through colour codes); first match → reply. Regex dialect
is Rust `regex` (already a dep) — the description says "Rust regex, no
look-around". `timeout` clamped to 120 s: a longer wait is the turn ending
and the `<job_result>` waking the model (14 §0.3) — the two verbs stay
distinct. This is not the `wait_job` 14 §9 rejected: that was "block
until a build finishes", this is "block until the prompt is back", bounded
and returning a screen.

### 4.7 `ready`

After spawn, `start` runs the same loop as `wait` with the `ready.log`
regex, plus a `tokio::net::TcpStream::connect((host, port))` poll every
250 ms when `ready.port` is set; both must pass; timeout default 30 s,
max 300. Exit before ready → the finished shape. The 14 `claimable`
swap still decides inline vs event; a `start` that returns "running" is
`Running`, one that returns the finished shape is `Done`.

### 4.8 Actor surface

`Job` (14) gains `pty: Option<PtyHandle>` and `name: Option<String>`:

```rust
struct PtyHandle {
    writer: Mutex<Box<dyn Write + Send>>,
    parser: Arc<Mutex<vt100::Parser>>,
    output: Arc<Notify>,
    pid: u32,
}
// messages
struct StartPty { session, call_id, args: StartArgs, cwd: PathBuf }   // ask → StartReply (14) with the §2.2 text
struct Send { session, id: String, args: SendArgs }                    // ask → Result<ToolOutput, String>
struct Screen { session, id: String }                                  // ask → Result<ToolOutput, String>
struct Wait { session, id: String, args: WaitArgs }                    // ask → Result<ToolOutput, String>
```

`id` resolution: exact `j{n}` first, else a running job of the session
with that `name`; a finished job by name → `j4 (py) exited 0 — start it
again`. `JobView` gains `name: Option<String>, pty: bool`.

Events: `JobEvent::Screen { session, id, rows: Vec<String>, cursor: (u16, u16) }`,
coalesced like `Output`, published only for PTY jobs (`Output` is not
published for them: the bytes are not for reading).

## 5. Surfaces

- **TUI** (14 §4.1 live block): for a PTY job the block shows the last
  `JOB_LINES` non-empty rows of the latest `Screen` event instead of the
  `Output` tail. `/jobs` row per §2.3. The `<job_result pty="true">` card
  title gains `pty`.
- **RPC**: `JobEvent::Screen` forwarded (`forward!`), `JobView` fields in
  the bindings. No new methods — the tools are the API.
- **Signal / Slack**: `/jobs` row per §2.3; a `Screen` event is not
  rendered (a phone is not a terminal); the `<job_result>` reply is a
  turn like any other.
- **Python runtime (08)**: not exposed in v0; `shell()` stays pipes.

## 6. Config

```yaml
tools:
  pty_ready_timeout_secs: 30    # `start` readiness default when `ready` gives none
```

Nothing else: size and `TERM` are constants (§1.9); the deadline does not
apply (§1.6); the log dir is 14's.

## 7. Verification

1. `pty::tests::repl_round_trip` — `start { command: "python3 -q", ready: { log: ">>> " } }`
   → running, screen ends `>>> `; `send { text: "1+1" }` → screen contains
   `2` and a new `>>> `; `send { keys: [ctrl_d] }` → `<job_result exit="0">`.
2. `pty::tests::screen_is_rendered_not_bytes` — `printf 'abc\rX\n'` →
   screen row `Xbc`; the log holds the raw bytes.
3. `pty::tests::query_responder` — §4.4 script prints `1;1`; the log
   contains no `R` reply.
4. `pty::tests::wait_pattern_and_timeout` — `sh -c 'sleep 1; echo READY; sleep 30'`:
   `wait { pattern: "READY" }` matches in ≈1 s; `wait { pattern: "NEVER", timeout: 1 }`
   → `timed out`, `is_error: false`.
5. `pty::tests::ready_port` — `python3 -m http.server 0`-style fixed port:
   `ready: { port }` returns once the connect succeeds; with a wrong port
   → `not ready after Ns (waiting on: port …)`.
6. `pty::tests::name_resolves_and_cancel_kills` — `start { name: "srv" }`,
   `screen { id: "srv" }` works, `cancel_job` → `kill -0 pid` fails, master
   dropped.
7. `jobs::tests` unchanged (pipes path untouched).
8. Headless smoke, real model: "start a python REPL, compute 2**100 in
   it, then quit it" → three tool calls (`start`, `send`, `send ctrl_d`),
   one `<job_result>`; then "run `wth --data-dir /tmp/x` and tell me what
   the first screen says" → the harness drives its own TUI and reads the
   status line. Record in Status.

## 8. Order

Each step leaves `wth` green.

1. `pty.rs`: spawn, reader thread, parser, responder, `screen_text`;
   `StartPty`/`Send`/`Screen`/`Wait` on `Jobs`; the four tools registered
   next to `shell`; `JobView.name/pty`; `JobEvent::Screen`; tests 1–6.
2. Agent: `<job_result pty="true">` body = final screen (one branch in the
   wrapper).
3. TUI: `Screen` in the live block, `/jobs` row; RPC forward + bindings;
   Signal/Slack `/jobs` row.
4. Docs (§10), smoke (7.8), Status.

## 9. Rejected

- **`pty: true` on `shell`.** `shell`'s contract is "runs and exits within
  grace or becomes a job with a deadline" (14). A pty process has stdin,
  no deadline and a screen; giving it the same name hides three different
  behaviours behind a flag. Two tools, one sentence each about which to use.
- **A separate broker process** (omp). One daemon already; the second
  process exists in omp because many `omp` instances share a directory.
- **Project-scoped names, `persist`, `detached`, restart policies.** A
  session's jobs die with the harness (14 §5). Servers that must outlive
  it are launchd's job, not the model's.
- **A `logs` op with byte cursor and `follow`.** `read_file` on the log and
  `wait` on a pattern cover both halves.
- **Stripped-bytes view for the model** (omp). The whole point of the pty
  is programs that redraw; a screen is the only faithful reading.
- **`resize`.** 120×40 until a transcript shows a program that needs more.
- **Streaming `Screen` events into the model mid-turn.** Same as 14 §9:
  cache-busting; `screen`/`wait` are explicit.
- **brush on the pty.** brush has no way to hand a child a controlling
  terminal, and a job that inherits the session's `Shell` clone would
  block the pipes design's last-finished-wins on a process that never
  finishes.
- **Applying `job_timeout_secs` to PTY jobs.** A REPL left open for an
  hour is not a runaway; the shutdown kill and `cancel_job` bound it.

## 10. Edits to other docs

- **00 §6.** After the jobs sentence: "PTY jobs (22): `start`/`send`/
  `screen`/`wait` run a command on a pseudo-terminal with a rendered
  120×40 screen; same `Jobs` actor, no deadline." §10 dependency table:
  `portable-pty 0.9`, `vt100 0.16`. §12 phase row 13.
- **14.** §0.8 gains "PTY jobs (22) are jobs with stdin and a screen"; §9
  `services`/`start` and `wait_job` bullets get "revisited in 22 §1, §4.6"
  (the 22 versions are bounded and screen-returning, not foregrounding).
- **README.** Tools list: the four tools, one line; config key.
- **`harness.md` skill.** One bullet on when to use `start` vs `shell`.
