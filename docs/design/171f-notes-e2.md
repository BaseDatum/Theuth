# 171F E2 notes (engine: record text, reply notes, label, terminal leaks, agent read refusals)

Decisions taken where the brief left room, and the limits that stay.

## B14 echo strip (`crates/cells/src/render.rs`)
- One regex built at first use from `shell::probe_line` (exit as `-?[0-9]+`) and the `D`-mark tail of
  `shell::run_line` (the text after the sourced file's `; `), for bash-family and pwsh. `probe_line` /
  `run_line` became `pub(crate)` (visibility only; they are functions, there are no string constants).
- The strip runs per text line (wrapped rows joined first), so an echo that wraps at the screen edge is
  still cut. A line that held an echo keeps only its plain text in `out.ansi` (its SGR is dropped): the
  echo can straddle a wrap and that row is theuth's own text plus the tty's `^C`.
- The actor-level test ("Stop `sleep 120`, no `133;D` in out.txt") belongs in `actor/tests.rs` (E1's
  file); the render-level test `stop_echo_is_cut_and_ctrl_c_stays` covers the same bytes (probe line and
  run-line tail, exits 130/137, bash/zsh/pwsh) plus `run::summary == "^C"`. Z: add the actor test if wanted.

## B17 / B3 render (`render.rs`)
- `out.ansi` is now one line per text line: a wrapped row continues its line, joined at `ESC[m` (each
  row's SGR starts from default). Before, a 250-column line was three ansi rows. The scrolled-out head
  (vt100 `on_scroll` + one `BasicFormattedCaptureState`) is logical lines by construction, so the whole
  render is consistent. UI renderers that wrap long lines show the same thing.
- Head cap `HEAD_CAP` = 8 MiB of formatted bytes; past it rows are counted and a line
  `… N more lines are in term.log` sits between the head and the retained scrollback. N counts rows
  (a wrapped long line counts once per row). `Rendered.dropped_lines` now means exactly that N.
- Known ceiling (ponytail comment): a wide line split exactly at the head/scrollback boundary shows as two
  lines (`RowContents` does not expose `wrapped`).
- `head_tail` (deliver.rs and note.rs) share `deliver::line_cuts`: head ends after its last `\n`, tail
  starts at the next line start (kept as is when already at one); byte cut when the window has no `\n`.
  note.rs's cut marker no longer adds a blank line when the head already ends with `\n`.

## B9 reply notes
- `Cells::store()` in `actor.rs` became `pub(crate)` (one word, outside the finish/Extra hunk) so the new
  crate-root `notes.rs` can reach the store as the brief asks. Deviation from "only the finish/Extra hunk".
- `cell_reply_note_get` answers `None` when the cells plugin is off (like `cell_live`);
  `cell_reply_note_set` errors "not running" like the other press methods.

## B20 label
- `label="…"` follows `name=`/`cell=` and precedes `exit=` (run stays first, L2); cut to 40 chars before
  escaping. The note path (`deliver::cell_result`, no `Extra`) has no label: the row does not carry it.

## B15
- Bridge: a `(session, job)` set fed by `JobStarted` with a `cell:` name; `JobOutput`/`JobScreen`/
  `JobIdle` of those are dropped, `JobFinished` drops and forgets. The bus is untouched.

## B23
- One check `Jobs::kernel(session, id)` (a `cell:` name, or any job — live, finished or lost — with that
  id and a `cell:` name) at the top of `SendInput`, `Screen` and `Wait`. It also closes the lost-kernel
  path (`send`/`screen` on a kernel a previous engine lost used to show its last screen). There is no other
  agent tool that reads a job by id (`jobs` lists without kernels; `cancel_job` already refused). UI-only
  messages (`PtyScreen`, `PtyInput`, `PtyAttach`, `PtyResize`) are not agent tools and stay open.

## B24
- No protected-path check existed in `crates/tools`; added one in `seat.rs` (the shared path gate):
  `refuse_cell_record(root, path)` (path canonicalised with the existing `real()`, so `..` and symlinks
  resolve) used by `read_file`, `write_file`, `edit`, `grep` (root) and `glob` (root); `grep`'s walk
  prunes cell records with `cell_record_filter` so `grep ~/.theuth` cannot read them.
- How a kernel log is told apart: kernel jobs now log to `<root>/jobs/<session>/<id>.cell.log` (pty.rs,
  `jobs::CELL_LOG`), so the check is a pure path test that survives engine restarts (the Jobs actor only
  knows the current engine's jobs). Every consumer takes the log path from the job, none rebuilds it.
  Logs written before this change keep `<id>.log` and stay readable until their session closes.
- Data dirs checked: the session's resource root and the engine data dir recorded at tools plugin start
  (`seat::set_data_dir`), because a private session's resource root is a scratch dir.
- `glob` refuses a root inside a record but still lists names under a broader root (names, not content);
  `list_dir` is unchanged for the same reason.
- Limit: `shell()` (and any program the agent runs) can still read these files; out of scope by the brief.

## B4 TUI
- `cell_results()` peels only leading `<cell_result …>` blocks: one `Entry::Info` line each
  (`↳ cell <label | name | cell ordinal> · exit <x> · expect <…>`), then the user's text as `you`.
- Live path: the TUI compares its locally echoed text with the turn text minus parked blocks, so the
  user's row is not duplicated; the `↳ cell` lines are inserted before that row.
