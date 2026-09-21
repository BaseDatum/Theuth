# 55 — Interactive terminals: the human types into the model's PTY jobs

Status: **as built · 2026-09-21** (design v0 2026-09-20; §10) · extends `22-pty.md` §1.9/§9 (input from a
human; `resize` stays rejected), `51-session-lifecycle-costs-and-terminals.md`
§7 (reverses the "no PTY input/takeover" scope line; keeps snapshot-only
observation), `25-desktop.md` §7 (one RPC method). §8 lists the amendments.
Every file:line below was read on 2026-09-20 (scout reports
`agent://PtyJobsRpcScout`, `agent://FrontendTermScout`).

Asked for: the PTY sessions the model starts (`start`, 22) become terminals
the human can open and type into — from the TUI and the desktop — instead of
only watching them; no terminal emulator written here, mature packages only.

Verdict: **small.** The emulator, the redacted styled snapshot, the read-only
overlay in both frontends and the RPC session gate all exist (51 §7). What is
missing is one write path and a keyboard-capture mode in two viewers. The
companion doc `56-acp-tui-handoff.md` (exploratory) builds on this one to put
an external harness's own TUI on such a terminal.

Not in this doc: PTY resize (§7), raw ANSI streaming to frontends (§7),
mouse reporting, mobile keyboard ergonomics beyond plain text (§4.5),
anything about ACP children (56).

## 0. Decisions in one screen

1. **Human input is one RPC, `pty_input(session, job, text?, keys?)`, that
   writes bytes and returns.** No settle, no screen in the reply. Authorized
   by the same session gate as `pty_screen` and `cancel_job`
   (`api.rs:1026-1036`); `cancel_job` is the precedent for a human-mutating
   PTY action over RPC. Not a tool: the model has `send` (22 §2.1).
2. **Frontends send structured keys, never bytes.** The wire type is the
   existing `tools::Key` enum (`pty.rs:295-341`), grown with modifier, F-key
   and paste variants (§2.2). The only escape table stays in `pty.rs`, behind
   one `encode()` that consults the vt100 modes only the owner knows
   (`application_cursor`, `bracketed_paste`). Two per-frontend byte tables
   would drift and neither could see those modes.
3. **Observation stays the 51 §7 snapshot.** `pty_screen` — redacted, styled
   cells, bounded history — is what the interactive viewer paints; it just
   polls faster while it has focus (60 ms instead of 500). No raw stream, no
   second emulator, no `xterm.js` (§7). `Jobs` owns the only terminal
   emulator (51 §0.6), unchanged.
4. **The widgets already exist.** The Rust TUI is iocraft (not ratatui, so
   `tui-term` does not apply); its cell painter (`ui.rs:1144-1263`) is the
   widget. The desktop's `<pre>` + `<span>`-per-run painter
   (`TerminalViewer.tsx:42-80`) is the widget. Neither is "writing a
   terminal": `vt100` parses, `portable-pty` spawns, the frontends paint
   cells they are given.
5. **One shared writer, last byte wins, and the model is told.** No lock
   between human and model. `PtyHandle` records the last human write; every
   `send`/`screen`/`wait` reply carries `user attached · typed 3s ago` for
   30 s after it. That is the whole coordination story.
   Ordering is the client's job: each frontend keeps one `pty_input` in
   flight and sends the next only after the reply (the TUI's command loop
   runs every other command concurrently; the smoke test typed `7*6⏎` and
   Python received `*⏎67` before this lane existed).
6. **Fixed 120×40 stays** (22 §1.9). Both parties look at the same grid; the
   overlay clips and pans as today and says so. Resize is §7 with its hook
   named.

## 1. Facts this design rests on

| Need | Exists | Where |
|---|---|---|
| PTY job with 120×40 vt100 screen, 512-line history, redacted styled snapshot, byte budget, busy gate | yes | `pty.rs:370-381` (`PtyHandle`), `:973` (`Parser::new(40,120,512)`), `:551-720` (`render` + `secrets::redact_spans`), `:1118-1160` (`PtyScreen`, `SNAPSHOTS = 4`) |
| Write bytes into a PTY | tool-only, with 300 ms settle | `pty.rs:1641-1689` (`send`, `settle`); `PtyHandle.writer` is private |
| Key table | 17 variants, private `bytes()`, arrows always `\x1b[A..D` | `pty.rs:295-341` |
| Read-only TUI overlay, 500 ms poll, clip + pan, Esc closes | yes | `model.rs:1790-2013` (`TerminalView`), `:9932-9963` (`terminal_key`), `ui.rs:1144-1263` |
| Read-only React viewer, 500 ms react-query poll, per-run spans, Dialog/Sheet, nav keys only | yes | `TerminalViewer.tsx`, `lib/terminal.ts` |
| RPC session gate (durable: any authenticated peer; private: owner peer) | yes | `api.rs:1026-1036` → `supervisor.rs:5846-5882` |
| vt100 mode accessors | yes | `vt100-0.16.2/src/screen.rs:554-578` (`application_keypad`, `application_cursor`, `bracketed_paste`, `mouse_protocol_mode`), `:88` (`set_size`) |
| `menu_key` receives modifiers | **no** — `KeyCode` only | `model.rs:7386-7418`; modifiers are dropped at `ui.rs:269` |
| Master pty retained for `resize` | **no** — moved into the run task, dropped at exit | `pty.rs:1244, 1368` |
| Any RPC that writes to a PTY; any frontend key encoder; any binary/stream channel | **no** | scouts §2, §4; desktop transport is text JSON-RPC (`rpc/transport.ts:13-20`) |

## 2. Owner side (`crates/tools/src/pty.rs`)

### 2.1 `PtyInput`

```rust
// actor message on Jobs
pub struct PtyInput { pub session: SessionId, pub id: JobId, pub text: Option<String>, pub keys: Vec<Key> }
// ask → Result<(), String>: "no such job" | "j4 exited 0" | io error text
```

Resolves the *running* job under `session` (exact `j{n}`; names are a tool
convenience and stay out of this path). Under the parser lock, reads the
modes and `encode`s (§2.2); under the writer mutex, `write_all` + `flush`
(`spawn_blocking`, as `send` does). Stores `now_ms()` into
`PtyHandle.user_input: AtomicU64`. Returns. Not appended to the log by theuth;
the process's own echo lands in the log as today.

### 2.2 `Key` grows; one encoder

Add to `tools::Key` (serde snake_case; payload variants externally tagged,
e.g. `{"ctrl":"a"}`, `{"f":5}`, `{"paste":"…"}`):

| Variant | Bytes |
|---|---|
| `Ctrl(char)` (a–z, `[ \ ] ^ _ @`) | `c.to_ascii_uppercase() & 0x1f` |
| `Alt(char)` | `\x1b` + char |
| `F(u8)` 1–12 | `\x1bOP..\x1bOS`, `\x1b[15~ 17~ 18~ 19~ 20~ 21~ 23~ 24~` |
| `ShiftTab` | `\x1b[Z` |
| `Insert` | `\x1b[2~` |
| `Paste(String)` | `\x1b[200~ … \x1b[201~` when `screen.bracketed_paste()`, else the text raw |
| `Up/Down/Right/Left` (existing) | `\x1bO{A..D}` when `screen.application_cursor()`, else `\x1b[{A..D}` |

`CtrlC/CtrlD/CtrlZ/CtrlL` stay as aliases (existing model transcripts and
the four descriptions use them). `Key::bytes(self)` becomes
`fn encode(text: Option<&str>, keys: &[Key], screen: &vt100::Screen) -> Vec<u8>`;
`send` (`pty.rs:1641`) uses the same function — a REPL in application-cursor
mode gets correct arrows from the model too, a bug fix that falls out.
Unknown `ctrl` chars or `f > 12` are schema/`Err`, never silently dropped.

### 2.3 The `user attached` trailer

`reply()` (`pty.rs`, the screen block shared by `send`/`screen`/`wait`) adds
one final line when `now - user_input < 30_000`: `user attached · typed 3s
ago`. `structured` gains `"user_attached_ms": Option<u64>`. Tested in §6.

### 2.4 RPC

`crates/rpc/src/api.rs` trait, beside `pty_screen` (`:167-190`):

```rust
#[method(name = "pty_input", with_extensions)]
async fn pty_input(&self, session: SessionId, job: JobId, text: Option<String>, keys: Option<Vec<Key>>) -> RpcResult<()>;
```

Impl: `self.authorize(ext, session).await?` then `ask::<Jobs,_>(JOBS_ACTOR,
PtyInput { .. })`; `Err(s)` → invalid-params with `s`. `Key` already derives
`JsonSchema`; the TS binding regenerates. Nothing else in the API changes.

Private trees: `authorize` already restricts to the owner peer; a viewer that
is open when the tree closes gets `Unavailable` from `pty_screen` and
`"no such job"` from `pty_input` — both already rendered as "No longer
available."

## 3. TUI (`crates/tui`, iocraft)

`TerminalView` gains `interactive: bool` (default `false`).

- **View mode** — today's arms (`model.rs:9932-9963`) plus `i` or `Enter` →
  `interactive = true`, only when `page.exit.is_none()`.
- **Interactive mode** — every key → `Command::PtyInput { session, job, text,
  keys }` except the detach chord **Ctrl-]** → `interactive = false`. Esc goes
  to the program. After sending, `dirty = true` so the echo is fetched on the
  next wake. Job exits while interactive → drop to view mode (the final
  screen stays).
- **Status row** (`ui.rs:1144-1191`, the yellow line): `interactive · Ctrl-]
  to detach` / `120×40 · terminal too small, ←/→ pans` when clipped / today's
  text in view mode. F2 is already suppressed by `modal_menu()`
  (`model.rs:3308`).
- **Key mapping** (pure, `tui`): `KeyCode::Char(c)` with `CONTROL` → `Ctrl(c)`,
  with `ALT` → `Alt(c)`, plain → `text`; `Enter Tab Esc Backspace Delete Up
  Down Left Right Home End PageUp PageDown Insert F(n)` → the variants;
  `BackTab` → `ShiftTab`; `TerminalEvent::Paste(s)` → `Paste(s)`. Consecutive
  plain chars in one tick coalesce into one `text`.
- **Plumbing**: `menu_key` (`model.rs:7386-7418`) gets `KeyModifiers` from
  `ui.rs:269` (or a branch before `menu_key` for `Menu::Terminal` in
  interactive mode — pick whichever is fewer lines).
- **Poll**: `TERMINAL_POLL_MS` 500 stays for view mode;
  `TERMINAL_INTERACTIVE_POLL_MS = 60` while interactive and the viewport is
  `Live`. One request in flight at a time as today (`TerminalView::poll`,
  `model.rs:1962-1976`); "terminal busy" keeps the last page.
- **`Command::PtyInput`** → `lib.rs execute` → `api.pty_input(ext, ..)`; an
  `Err` shows in the status row and flips to view mode.

## 4. Desktop and mobile (`crates/desktop/ui`)

### 4.1 Focus target

A visually hidden `<textarea>` inside the focusable wrapper `div`
(`TerminalViewer.tsx:172-180`). Standard trick; it gives IME composition,
the mobile soft keyboard and paste for free.

### 4.2 Handlers

- `onKeyDown` → `Key`/text mapping (same table as §2.2, ~40 lines of TS, no
  escape bytes): `ctrlKey` + letter → `{ctrl}`, `altKey` → `{alt}`,
  `Shift+Tab` → `shift_tab`, `F1..F12`, named keys, printable → text.
  `preventDefault` on everything the terminal consumes so the dialog's own
  shortcuts and browser defaults do not fire.
- `onBeforeInput` / `onInput` for composed (IME) text → `text`.
- `onPaste` → `{paste}`.
- `Ctrl-]` blurs the textarea (detach). A visible **Interactive / Read-only**
  toggle exists for pointer users and screen readers; focus follows it.
- `api.pty_input(...)` per event; `queryClient.invalidateQueries(ptyScreen)`
  after each so the echo is fetched immediately.

### 4.3 Polling and fit

react-query `refetchInterval` 500 → 60 while the textarea has focus and the
viewport is `live` (`Body`, `TerminalViewer.tsx:96-108`); back to 500 on
blur; unchanged otherwise. Rendering unchanged (per-run spans, cursor cell).
Fit: scale `font-size` so 120 columns fit the dialog width (`ResizeObserver`,
one number); no wrapping, `min-w-max` goes.

### 4.4 State

`lib/terminal.ts` reducer gains `interactive: boolean`; `applyReply` with an
`exit` clears it. Nothing else.

### 4.5 Mobile

Same component in the bottom `Sheet`: text, Enter and Backspace work through
the textarea; a one-row toolbar `Esc Tab ↑ ↓ ← → Ctrl-C` (44 px targets,
51 §7.4) covers the rest. Nothing more in v1.

## 5. Crates

| Need | Crate | Status |
|---|---|---|
| PTY spawn, master write | `portable-pty` 0.9 | present (`Cargo.toml:222`) |
| screen, modes | `vt100` 0.16.2 | present (`Cargo.toml:223`) |
| TUI cell painting | iocraft `MixedText` | present (`ui.rs:1195-1263`) |
| desktop cell painting | React `<pre>`/`<span>` | present (`TerminalViewer.tsx`) |

No new dependency. Considered and not taken: `tui-term` 0.3 (ratatui widget;
we are iocraft), `termwiz` (wezterm's key encoder — heavy for 40 lines and a
second escape table beside `encode`), `@xterm/xterm` 5 (§7).

## 6. Verification

Spikes (each an hour):

1. **iocraft key intake**: Ctrl-], Ctrl+letter, Alt+letter, Shift+Tab,
   F-keys and `Paste` arrive distinguishably through `use_terminal_events`
   on the `mask-0.9` fork (`Cargo.toml:318-323`). If `Paste` does not, the
   host terminal's bracketed paste arrives as fast plain chars → `text`;
   acceptable.
2. **Typing feel**: 60 ms poll on a 120×40 page from the desktop over
   `LocalTransport` and over the relay `LinkTransport`; echo under ~150 ms
   locally, and whether the relay wants 120 ms.

Tests that stay:

- `pty::tests::encode_modes` — arrows in/out of application-cursor mode;
  `Paste` wrapped only when `bracketed_paste()`; `Ctrl('c')` == `CtrlC`.
- `pty::tests::pty_input_marks_user` — `PtyInput` writes and sets
  `user_input`; the next `screen` reply carries `user attached`; with the
  clock advanced 31 s it does not; `PtyInput` on an exited job is `Err`.
- TUI `terminal_interactive_forwards_and_detaches` — in interactive mode Esc
  yields `Command::PtyInput{keys:[escape]}`; Ctrl-] yields no command and
  flips the flag; `i` on an exited page does nothing.
- Desktop `terminal.test.ts` — `keyToInput(KeyboardEvent)` table rows for the
  modifier cases; reducer clears `interactive` on `exit`.
- Smoke, real terminal: `start {command: "python3 -q", name: "py"}` from a
  headless model turn; open `/terminals`, `i`, type `2**10⏎`, see `1024`;
  Ctrl-] detaches; model `screen {id: "py"}` reply ends with `user attached`.
  Same from the desktop dialog. Record in Status.

## 7. Rejected

- **Raw ANSI stream + `xterm.js`.** Needs a lossless ordered byte stream —
  the only push primitive is the lossy `subscribe_events` broadcast
  (`api.rs:983-990`) and the desktop transport is text JSON — and would
  bypass the vault redaction that `render` does on logical text across
  style runs (51 §7.2, `pty.rs:551-720`): `{{secret:…}}` expansions echo in
  these terminals. It is also a second emulator state (51 §0.6). The
  snapshot path exists in both frontends; polling faster is one constant.
- **`resize`.** Still 22 §9's rule: no transcript yet. It is also a policy
  question (model view vs. human viewport vs. several viewers — tmux's
  smallest-client rule) that the fixed grid sidesteps. Hook when it comes:
  keep the `MasterPty` in `PtyHandle` (`Mutex<Box<dyn MasterPty + Send>>`;
  today it dies in `Run.master`, `pty.rs:1244`), `master.resize(PtySize)`
  (`portable-pty-0.9.0/src/lib.rs:92`) + `parser.set_size` under the lock,
  `TerminalSummary.rows/cols` become live.
- **Reusing `SendInput` for humans.** Its 300 ms settle is right for one
  model round trip and wrong for a keystroke; and it returns a screen the
  viewer already polls.
- **Per-frontend key-to-bytes tables.** Drift, and blind to
  `application_cursor`/`bracketed_paste`.
- **A lock between human and model.** tmux does not have one either; the
  trailer tells the model a hand is on the keyboard, which is what it needs
  to stop typing.
- **Lowering `OUTPUT_EVERY` (500 ms `JobScreen` hints) while a viewer is
  attached.** Correctness already rests on polling (51 §7.3); the hint is
  text-only and nothing in the frontends consumes it for the viewer.
- **Mouse reporting.** vt100 exposes `mouse_protocol_mode`; encoding SGR
  mouse from a `<pre>` click and from iocraft mouse events is real work with
  no asked-for use. Later, same `Key`-style variant.

## 8. Edits to other docs

- **22 §1.9/§9:** `resize` stays rejected; input from a human is in scope via
  `pty_input`; `Key` gains `Ctrl/Alt/F/ShiftTab/Insert/Paste`; arrows encode
  by mode (bug fix for `send` too). §5 TUI: the overlay has an interactive
  mode. §11 as-built gets a line.
- **51 §1 scope line:** strike "PTY input … takeover"; keep "resize, raw
  terminal replay, unbounded scrollback". §7.4: "No keystroke is forwarded"
  → forwarded only in interactive mode via `pty_input`; Escape closes the
  overlay only in view mode. §8.1: add `pty_input`.
- **25 §7:** `pty_input`.
- **README:** `i` / Ctrl-] in the terminal overlay; the desktop toggle.
- **`harness.md` skill:** one bullet — "a `user attached` trailer on a PTY
  reply means a human is typing into that job; read the screen before
  sending more".

## 9. Order

Each step leaves `theuth` green and is independently useful.

1. `encode()` with modes, `Key` growth, `PtyInput`, `user_input` trailer,
   RPC method + binding; tests. (The desktop can type the moment step 3
   lands; the model gets correct application-mode arrows now.)
2. TUI interactive mode + modifier plumbing; spike 1.
3. Desktop textarea, toggle, mapping, fast poll, fit; spike 2.
4. Docs (§8), smoke, Status.

## 10. Status (as built, 2026-09-21)

Shipped as designed, one deviation and one addition:

- **Ordering lane (§0.5 addition).** The TUI command loop runs every command
  in its own task; the first smoke typed `7*6⏎` and Python got `*⏎67`.
  `PtyInput` commands now take one serial lane (`tui/src/lib.rs
  run_commands`); the desktop chains `pty_input` promises. Server side stays
  fire-per-request.
- **Desktop `interactive` is component state, not reducer state (§4.4
  deviation).** It is focus state of the textarea; the reducer is untouched.
  The trailer window is `attached_for()` in `pty.rs`, unit-tested directly
  instead of reaching into the actor.

Checks: `pty::tests::encode_follows_screen_modes`,
`pty::tests::pty_input_types_and_marks_user_attached`,
`tui model::tests::terminal_interactive_forwards_keys_and_detaches`,
desktop `terminal.test.ts` (`keyToInput`) and `api.test.ts` pass.
`pty::tests::query_responder` / `tty_holder_outliving_exit…` fail before and
after this change (zsh as `$SHELL`; admission race) — not touched.

Smoke, headless daemon with claude-haiku-4-5: model `start`ed `python3 -q`;
`pty_input("2**10", [enter])` returned in 3 ms and the next `pty_screen`
showed `1024`; `[{ctrl:"l"},{paste:"print('pasted')"},enter]` — Python 3.14's
REPL took the bracketed paste as paste mode (a second Enter ran it), i.e.
`bracketed_paste()` detection is live; `{f:13}` → `f13: only F1–F12 exist`,
unknown job → `no such job`. The model's following `screen` result ended with
`user attached · typed 1s ago`. Smoke, TUI on a real pty: `/terminals` → ⏎ →
`i` → `7*6⏎` painted `42` in the overlay; Esc reached Python and did not
close the overlay. Ctrl-] detach is covered by the unit test only (the
process harness could not inject 0x1d). Desktop typing was not exercised in
a browser; `tsc` and vitest are the evidence. Spike 2 (relay latency) not run.

## 11. Amendment (2026-09-21): the desktop runs a real terminal

Asked for: the desktop/mobile viewer should be a real terminal — native
scrollback, resize, selection — and the human should type the moment it
opens, no toggle. The owner accepted the redaction cost: **`subscribe_pty`
is not redacted.** Everything §7 rejected for that reason is now built for
the desktop; the TUI, the model's `screen`/`send`/`wait` and `pty_screen`
are unchanged (still redacted, still the 51 §7 snapshot).

- **Stream.** `PtyHandle.stream: broadcast<PtyOut>` (256 chunks). `ingest`
  sends every chunk under the parser lock; `run` sends `Exit`. `PtyAttach`
  subscribes and clones the screen under that same lock, so replay + stream
  is lossless; the replay (`pty.rs replay()`) is vt100's `rows_formatted`
  per scrollback row, `rows` newlines to push them off, then
  `state_formatted()` (screen, cursor, input modes). A lagged subscriber gets
  a fresh `Reset` instead of a gap; a finished job attaches to its last
  screen and gets `Exit` at once.
- **RPC.** `subscribe_pty(session, job, rows?, cols?)` → `PtyChunk`
  (`reset {data,rows,cols}` | `data {data}` | `exit {exit}`, base64 bytes);
  `pty_resize(session, job, rows, cols)`. Same session gate as `pty_input`.
- **Resize** (reverses 22 §9 / §7 here). The `MasterPty` lives on
  `PtyHandle` (`Mutex<Option<_>>`, taken by the job task at exit);
  `resize` = `parser.set_size` then `master.resize` (2–200 × 10–400).
  `render`, `TerminalSummary` and `PtyPage` report the live size, so the
  model's `screen` follows the human's viewport while a viewer is open. The
  viewer restores 120×40 on close. Policy: last resize wins; two viewers open
  at once see each other's width (tmux without aggressive-resize). Hook if it
  matters: a `resize` item on the stream.
- **Desktop** (`TerminalViewer.tsx`): `@xterm/xterm` 6 + `addon-fit`;
  `RpcClient.stream()` generalises `subscribe()` to any jsonrpsee pair
  (re-issued on reconnect → a fresh `reset`). `term.onData` → `pty_input`
  text, one call in flight; `term.onResize` → `pty_resize`; focus on open,
  Escape goes to the program (xterm stops propagation, the dialog stays).
  Mobile keeps the `Esc Tab ↑↓←→ ^C` strip over `Key`s. Deleted:
  `lib/terminal.ts` (reducer, `keyToInput`, palette), its tests, the
  `<pre>` painter, `queryKeys.ptyScreen`.

Check: `pty::tests::attach_replays_streams_and_resizes` — a second vt100
fed replay+stream matches the daemon's; `ListTerminals`/`PtyScreen` report
the resized grid; out-of-range refused; `Exit` closes the stream; a finished
job attaches but does not resize.
