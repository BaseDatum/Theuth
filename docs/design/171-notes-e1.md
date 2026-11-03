# 171 CELLS — notes of engine worker E1

Decisions taken where the design or the brief was silent, deviations, tests. Design: `171-cells.md`; ownership: `171-ownership.md` (row E1).

## Contract notes siblings must know

1. **`CellRef::Chat.entry` is the stored message `seq` as text** (e.g. `"42"`), not the desktop's `e<n>` entry id: those ids are client-side counters (`crates/desktop/ui/src/store/reduce.ts:24`) the engine never sees. The engine reads the message through `agent::ReadSession` and matches `HistoryEntry::Assistant` by `context.target = ContextTarget::Message{seq}`. **U1** must send `String(context.target.seq)` (`MessageAssistant`'s `context` prop) and treat a cell without a stored seq (still streaming) as not runnable. A non-numeric `entry` is refused `changed`.
2. **`cell_output` / `Cells::output(run, offset, limit, ansi)`**: `ansi: false` is the agent-safe copy — vault-redacted, credential-masked (§9.4 table), and refused `private` for private runs; `ansi: true` is the owner's own terminal view (`out.ansi`, vault values still placeholders, credentials kept, private allowed). **A1**'s `cell_output` runtime function must call it with `ansi: false`. The RPC exposes both to owner clients.
3. **`Cells::run_scope(&RunId) -> Option<Scope>`** added (my file): the RPC routes Stop/input/Ask/attach by it.
4. **Delivery (L3)**: `HarnessDeliver{session, kind: "cell <name or ordinal>", text: <body>, wrapped: <cell_result…>, wake}`. `wake` per §9.3 is computed with `agent::ActivityOf` (Running/Awaiting/Approval = a turn is in flight → `true`, delivery `joined`); idle + `review` → `true` (`woke`); else `false` (`parked`). Pass results before the pass end are always `wake: false`; the pass end wakes when the pass ran a `review` cell. Ask theuth → `true`. Parked rows are re-delivered `wake: false` at engine start; `consumed_ms` is set on `AgentEvent::TurnStarted`.
5. **Cell job names** (L1): `cell:<kind> k<12 hex>` (unique per kernel key; `start`'s name uniqueness check holds for two kernels in one session).

## Decisions

- **Stop after Ctrl-C.** An interactive shell drops the rest of the typed run line on SIGINT, so the `D` mark never prints. After Stop/Kill the kernel probes the PTY's foreground process group (`tools::pty::CellForeground`, `tcgetpgrp`); once the shell itself holds the foreground for 3 × 150 ms it types a line that prints `133;D;130` (`137` after Kill) plus OSC 7. A later stray `D` (no run in flight) is ignored. Kill that leaves the run going (the shell busy in a builtin) closes the kernel `kill_after_secs` later.
- **Fixed PTY size** via `PtyAttach{size: (pty_rows, pty_cols)}` right after the start (attach resizes); `StartPty` keeps its shape.
- **Job-log path**: the frozen `Kernel` trait has no log accessor; the actor starts shell kernels through `shell::launch` (local adapter) which returns the log path beside the kernel; `shell::start` (frozen) wraps it.
- **Live bytes for `cell_attach`**: the shell kernel forwards the PTY broadcast between the `133;C` and `133;D` byte sequences (its own cut, `LiveCut`); the actor keeps a 512 KiB tail per run for late attachers. The record is always rendered from the log range `[C, D)`.
- **`code_sha`** = the presented digest for an unedited whole-cell run, else `surface::cell::digest(code that ran)`. So only an unedited whole-cell run of the gate's current text arms an `after` (§7.6).
- **`mode` column** stores the `RunMode` JSON (keeps line/selection offsets), not the bare word.
- **Selections** are UTF-16 code-unit offsets (CodeMirror's positions) into the code (header excluded).
- **`runs/<run>/meta.json`** (beyond §8.4's list): name, label, private, review, edited code, expect items — what a re-built `<cell_result>` (Ask theuth, restart re-park) needs and the row does not hold.
- **Cancelled runs** are not delivered (`delivery = none`); a run that failed before reaching its kernel (kernel start failure) is delivered as `failed` with the reason as its summary.
- **Engine shutdown**: once `rt.shutdown` fires, finishing runs are left `running`; the next start settles them `lost` (109 R1) and delivers quietly (or waking for `review`).
- **Kernel root for job logs**: `rt.data_dir` (`SessionResources::durable`); private sessions' kernel logs also go there (no peer is available to `agent::Authorize`).
- **Login environment**: captured from `$SHELL` (fallback `/bin/zsh` on macOS, `/bin/sh`). On failure: engine env + login PATH, and the kernel strip label gains `· your shell's environment could not be read — Refresh` (`KernelState` has no warning field).
- **bash 3.2 hint**: in `Interpreter.why` (`… · bash 3.2 — no associative arrays, no ${x,,}`); `Chip` has no hint kind.
- **rc files** also `unset HISTFILE` (run lines never reach the user's history) and zsh's `.zshenv` remembers where the user's `.zshenv` points `ZDOTDIR` so the user's `.zshrc` is still found.
- **Mark carry**: 32 bytes for OSC 133 (design), 1024 for an OSC 7 prefix (paths are long).
- **`cell:` jobs** are also omitted from `ListTerminals` (an inline viewer must not resize a kernel) and are `inline` on `Finished` (no `<job_result>` ever rides the guard, belt to A1's braces).
- **`pressed_by`** = `<desktop|web|phone> · <device>`: `phone` for an `ios` client, `web` for a relay peer, device = engine hostname for local peers, `relay` for remote ones, the forwarding engine's peer id when forwarded. `typed` counts keystrokes under `owner` (the frozen `input` has no device).
- **Forwarded runs**: the RPC remembers `run → scope` for `cell_run` replies it forwarded (process-local, 4096 cap) so Stop/input/Ask/reads route back to the owner. Gap: runs minted by a forwarded Run-all pass and `cell_attach` across engines are not routed (local runs only).
- **Note scope** (L6): the actor calls `note::source` / `note::deliver` / `note::latest` / `interp::*` at `// 171:E2` lines and returns their errors untouched; member acknowledgement (inv. 6) is E2's.
- **Python input requests** (`KernelEvent::InputRequest`) have no wire carrier yet; ignored by the actor (E2/U2).

## STUB-DEVIATION / ADD

- No frozen file changed. Local adapters: `shell::launch` (log path), `actor/chat.rs` (a copy of `surface`'s crate-private CommonMark fence scanner — `surface::fences` is not public).

## Tests (names)

- `tools::jobs::tests::{cell_mark_scan_every_split, cell_mark_scan_bel_and_st, cell_mark_inside_redacted_region}`, `tools::pty::tests::{cell_start_rejects_reserved_name, cell_jobs_omitted_from_jobs_list, cell_send_and_cancel_refused, cell_kill_spares_shell_group, cell_program_honoured_and_env_cleared, cell_marks_reach_channel}`, `platform::env::tests::{login_env_parses_nul_separated_entries, login_env_times_out}`.
- `cells::render::tests::*` (`\r` progress, pulumi spinner + preview fixture, wide line, 10 000-row cut, SGR), `cells::redact::tests::*` (vault value, AKIA/ASIA, JWT, aws secret/session token, gh/xox, private key, counts), `cells::run::tests::*` (`armed_edited_preview_disarms`, `armed_old_preview_disarms`, expect, summaries), `cells::deliver::tests::*` (L2 attribute order, private form, head+tail cut line, edited + python forms), `cells::store::tests::*` (every §8.4 column, audit round trip, retention), `cells::shell::tests::*` (run file prologue, the run line, live cut across splits, versions/chips, resolution order).
- `cells::actor::tests::{press_refusals (digest guard, superseded, danger line/selection, missing cwd, invalid, lint never refuses), run_all_stops_before_danger_and_blocking_lint, wake_follows_the_section_9_3_table, after_gate_with_edited_and_old_previews, idempotent_run_ids, restart_settles_lost_without_replay, e2e_real_shells}` — the e2e runs real `/bin/bash` and `/bin/zsh` kernels (HOME = a scratch dir, no user rc) through the jobs actor: §5.6 CWD table, PIPESTATUS + trailing comment, unclosed `if`, exit 42, `read -p` via `input`, FIFO order, second press attaches, cancel of a queued run, Stop under `yes` < 500 ms, credential masks + private refusal, run file gone + `code` file, `RunFinished` only after the final row.
- `rpc::cells::tests::{internal_peer_is_refused_for_every_press_method, guest_peer_is_refused_for_every_press_method, forwarded_internal_peer_passes_the_guard, every_read_method_refuses_guests, no_peer_is_unauthorized, pressed_by_names_kind_and_device, refusals_carry_a_stable_reason}`.

## R1 (lead, 20:06): E2's FK finding

- **HANDOFF-Z-2** was already closed in `9e7844ec`: `Run` carries `cell: bool` and `run()` computes `inline = claimable || origin == Tool || j.cell`, so a kernel's exit, Close or kill ships `Finished{result: None}` to the guard — never `Settle::FinishedWithResult`, never `inject`. `CancelJob{origin: Ui}` is refused for kernels (L1), so the kill that remains is `Shutdown` (`CellKill{close}`, session close). Proof: `tools::pty::tests::{cell_kernel_exit_never_reaches_the_model, cell_kernel_kill_never_reaches_the_model}` assert on the activity ledger: a bare `Finished`, no `Result`, no `Idle`.
- **HANDOFF-Z-3** was already closed: `Launch` forces `idle_after = None` for every `cell` job, whatever the caller passes (`shell::launch` passes `None` too). Proof: `cell_kernel_exit_never_reaches_the_model` starts the kernel with `idle_after = 1 s`, keeps it quiet for 3 s and sees no `JobEvent::Idle` and no `Notice::JobIdle`.
- E2's `note::member_guard`/`member_edit`, `note::defaults` + `note::cell_cwd`, `interp::chip`, `monty::sufficient` are not in this tree; the actor's `// 171:E2` arms call only the frozen `note.rs`/`interp.rs` signatures. Z wires them after the merge.

## Known gaps

- Run-all passes over forwarded (desk) sessions route the pass but not later Stop/attach of its runs (see Forwarded runs).
- `HarnessDeliver.wake: false` parks only once A1 lands parking; until then the agent's `inject` treats it as a wake.
