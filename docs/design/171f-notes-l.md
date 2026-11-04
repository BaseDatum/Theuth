# 171F2-L — live terminal shows theuth's echoed run line (decisions)

- **Data path.** A running cell's xterm (`Cell.tsx` `LiveTerminal` → `Terminal` source `cell`) streams
  `cell_attach` only: a `reset` chunk with the run's buffered live bytes (`Live.buf`), then `data` chunks
  from the run's broadcast, then `exit`. Those bytes come from `shell.rs` `watch`, cut by `LiveCut`.
  No client sees the kernel's raw PTY; the fix is engine-only, so desktop, web and phone all get it.
- **Root cause (run line).** `LiveCut` already starts at the real `ESC]133;C BEL` in the bytes (the echo
  holds the literal text `\033]133;C\007`, never the sequence), but `on_mark(Start)` also called
  `cut.start()`. `tools::pty::ingest` sends the C mark on its own channel *before* broadcasting the
  chunk that carries C, yet *after* the run line's echo chunks; `watch`'s `select!` picks either ready
  branch, so the mark was often handled first, the cut switched on, and the echo's last chunks went
  live (reproduced 3/3 on the base build: `"$(hostname)" "$PWD"` above `start`). Fix: the mark no
  longer starts the cut; only the C bytes do (`LiveCut::start` deleted).
- **`Live.buf.clear()` on `Started` removed** (`actor.rs`): with the cut byte-driven, the C chunk can be
  handled before the C mark, so the first output bytes reach the actor before `Started`; the clear
  dropped them from a late attach's replay. Only shell kernels send `Bytes`, and only from C on.
- **Probe after Stop.** The watcher now ends the cut right before it types the probe line: once the
  shell has held the foreground for `PROBE_TICKS` after a Stop, the run is over for the user and what
  follows (empty prompt, the probe's echo, the D mark) is theuth's. Known edge: a cell that keeps the
  shell itself busy while ignoring SIGINT (shell-builtin loop under `trap '' INT`) stops updating the
  live view once the probe is typed; its output still lands in the record.
- **Kept `cut.end()` on the D mark**: after D the run is taken, so later bytes have no sink anyway;
  keeping it resets the cut if a broadcast lag swallowed the D bytes.
- **Finished-run attach** replays the run's `out.ansi` (B14-clean, redacted) instead of raw `term.log`
  (which keeps the echoed probe): an attach that lands just after the run ended no longer flashes it.
- **Visual proof** BEFORE used the 171F closer's base build `/tmp/171f-z/bin/t` (built 08:23; no `.rs`
  change on `integrate/171f` after 08:20, so its Rust equals `d43bd4ed`) instead of rebuilding base.
