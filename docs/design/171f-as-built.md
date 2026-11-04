# 171F — as built (fix wave for design 171 "user cells", 2026-11-04)

Status: merged and gated on `integrate/171f` (base `5628e678` → C skeleton `0daf1005` → e1, e2, u1a, u1b, u2, u3 → Z). Not pushed, not merged into main. Per-worker decisions: `171f-notes-{c,e1,e2,u1a,u1b,u2,u3,z}.md`. Design text updated in `171-cells.md` §6.2, §6.3, §7.2, §7.3, §8.3, §11.2, §11.3.

## Per bug

Paths: `ui/` = `crates/desktop/ui/src/`, `cells/` = `ui/components/cells/`.

| bug | what changed | where |
|---|---|---|
| B1 runbook bar floats | solid `bg-layer-1` row in the message flow, mounted after the reply's LAST cell; no sticky/blur | `cells/RunbookBar.tsx`, `cells/CellFence.tsx` (U1a) |
| B2 run-line ▶ vanishes | `hoverAfter`: leaving text into the gutter keeps the hover; gutter `mousemove`; clear only on leaving the editor; note slot clears on its own `mouseleave` | `cells/CellEditor.tsx`, `cells/NoteCell.tsx` (U1b) |
| B3 two lines, huge box | engine drops trailing blank ansi rows; client `trimBlankTail` shrinks old runs | `crates/cells/src/render.rs` (E2), `cells/Cell.tsx` (U1b) |
| B4 raw `<cell_result>` in the user bubble, twice | `splitLeading` peels leading results; TurnStarted stamps the own bubble with `cells`; no peer card for `cell …` deliveries; `foldCells` on reload (Edit/Fork return); "N cell results sent with this message" row; TUI peels leading blocks into `↳ cell …` lines | `ui/lib/injected.ts`, `ui/store/reduce.ts`, `ui/store/sessions.ts`, `ui/components/chat/{MessageUser,SentResults,Message}.tsx` (U2); `crates/tui/src/model.rs` (E2) |
| B5 timeout says "stopped by you" | notice keys on state: stopped / "timed out after 5.3 s" / "killed — the kernel was closed…"; `exitText` names the state first; a later timeout never overwrites a user Stop/Kill (`stop_after`) | `cells/{Cell.tsx,store.ts,view.ts}` (U1b); `crates/cells/src/actor.rs` (E1) |
| B6 long pastes | `LongText`: >12 lines or >1200 chars clamps to 8 lines + fade, "Show all N lines"/"Show less", copy; full text stays in the DOM | `ui/components/chat/LongText.tsx`, `ui/lib/long-text.ts`, `MessageUser.tsx` (U2) |
| B7 queued chips below the kernel strip | `Composer` `strip` prop rendered right above the input box; `KernelStrip embedded` | `ui/components/chat/Composer.tsx`, `ui/routes/c.$session.tsx`, `cells/KernelStrip.tsx` (U1b) |
| B8 old bar ignores replacements | `followSuperseded`: steps follow `superseded_by` (successor ref + digest, watch-only register), live denominator | `cells/{CellFence,RunbookBar,store}.ts(x)` (U1a) |
| B9 Open as note twice → two notes | reply-note table (migration 0004) + `cell_reply_note_get/set`; UI in-flight guard, reuse when the doc still exists | `crates/cells/{migrations/0004_cells_reply_notes.sql,src/store.rs,src/notes.rs}`, `crates/rpc/src/cells.rs` (E2); `cells/open-as-note.ts` (U1a) |
| B10/B10b/B10c Monty refusal without reason | `monty_reason` (drops `unresolved-reference`, keeps imports), reason carried through `inspect_note`/`loop_`/`repl`, problem text `Monty can't run this cell: line N: …`, `monty_scan` reasons with the way out; UI **Use Python** → `#| python: auto`; Z: note cell re-reads its record after a header edit | `crates/cells/src/{monty.rs,actor.rs}`, `crates/surface/src/cell.rs`, `crates/agent/src/{loop_.rs,repl.rs}` (E1); `cells/NoteCell.tsx` (U1a, Z) |
| B11 Run all stops silently | `last_pass` per scope → `StateReply.pass` (`PassEnd`/`PassStop`) + `CellsChanged`; note header line, "Run all stopped here" row + ring, bar label from `pass` | `crates/cells/src/{types.rs (C),actor.rs (E1)}`; `cells/{NoteKernelPill,NoteCell,RunbookBar,store}.ts(x)` (U1a) |
| B12 live terminal twice too tall | starts at 12 rows, grows with used rows to 30 (`liveRows`, `onWriteParsed`), PTY never resized | `ui/components/chat/TerminalViewer.tsx`, `cells/Cell.tsx` (U1b) |
| B13 expanded things collapse while streaming | every transcript disclosure in `useToolRuns.disclosed` (`useDisclosure`/`disclosureKey`, `components/chat/disclosure.tsx`); stable peer signature; Streamdown mode once per mount; JsonRenderFence key; Card wrapper stable; cells sites converted; ratchet allowlist empty; Z: output toggles keyed by output index | `ui/lib/tool-runs.ts` (C), `ui/components/chat/**`, `ui/store/reduce.ts` (U3), `cells/**` (U1a/U1b), `cells/vendor/nteract/cell/OutputArea.tsx` (Z) |
| B14 shell hook leaks after Stop | `render()` strips the echoed probe line and the run line's D-mark tail (built from `shell::probe_line`/`run_line`) | `crates/cells/src/{render.rs,shell.rs}` (E2); actor test (Z) |
| B15 terminal icon lights for cells | `JobCounts`/`LiveCounts` skip cell jobs; bridge drops cell job events on the client feed | `crates/tools/src/jobs.rs`, `crates/rpc/src/bridge.rs` (E2) |
| B16 Kill does not end the cell | Kill closes the kernel (`CellKill{close:true}`), post-kill task removed; next run gets a fresh shell | `crates/cells/src/{shell.rs,actor.rs,kernel.rs doc}` (E1, Z) |
| B17 `cell_output` cannot reach line 1 | scrolled-out rows kept in front (8 MiB head cap, "N more lines are in term.log"); line-aligned `head_tail` | `crates/cells/src/{render.rs,deliver.rs,note.rs}` (E2) |
| B18 run ids differ by path | pass runs use 26-char Crockford ids | `crates/cells/src/actor.rs` (E1) |
| B19 missing interpreter shows enabled Run | `no_interpreter` error problem in `state()` | `crates/cells/src/actor.rs` (E1) |
| B20 result line mixes name and label | `label="…"` on `<cell_result>`; ResultCard shows `label ?? name ?? cell N` | `crates/cells/src/{deliver.rs,actor.rs}` (E2); `cells/ResultCard.tsx`, `ui/lib/injected.ts` (U2) |
| B21 chat cell cannot be edited | editable on click+type while runnable (`chatEditable`), edits in `cells/edits.ts` keyed by refKey, still sent as `edited` | `cells/{CellEditor,CellFence,view,edits}.ts(x)` (U1b) |
| B22 Ask theuth delivers twice | a wake delivery removes parked copies of its run; `join_quiet` dedupes by run | `crates/agent/src/{loop_.rs,supervisor.rs}` (E1) |
| B23 agent can read a kernel's screen | `Jobs::kernel()` refusal in `SendInput`/`Screen`/`Wait` | `crates/tools/src/{jobs.rs,pty.rs}` (E2) |
| B24 run files readable by file tools | `seat::refuse_cell_record` in read_file/write_file/edit/grep/glob (+ grep walk prune); kernel logs are `<id>.cell.log` | `crates/tools/src/{seat.rs,fs.rs,edit.rs,glob.rs,pty.rs,jobs.rs,lib.rs}` (E2) |
| B25 | not in any 171F brief; nothing changed | — |
| B26 bar kernel label wrong | `kernelLabel` from the reply's own steps' kernels | `cells/RunbookBar.tsx` (U1a) |
| B27 auto chip hides "auto" | `interpreterText` → `auto → python 3.14.8 (system)` (chat + note chip) | `cells/{view.ts,Cell.tsx,NoteCell.tsx}` (U1b) |
| B28 | not in any 171F brief; nothing changed | — |
| B29 skill vs doc_insert refusal | skill §5 now says doc_insert refuses the whole insert and names each problem; Monty quotes updated (engine unchanged) | `crates/cells/cells.md` (Z) |
| B30 `after_unknown` lists every name | ≤ 5 closest names then "… and N more"; "no cell named `x` in this chat/note" | `crates/surface/src/cell.rs` (E1); expectations in `crates/document/{src/note_cells.rs,tests/actor.rs}` (Z) |

## Merge record

| step | result |
|---|---|
| e1 | clean |
| e2 | clean (E1/E2 `actor.rs` hunks disjoint) |
| u1a | clean |
| u1b | 3 conflicts, all unions: `CellFence.tsx` imports (U1a `followSuperseded, runAllPairs, lastPass, scopeKey` + U1b `chatEditable`); `NoteCell.tsx` imports (U1a `lastPass, scopeKey, useCells` + U1b `interpreterText`); `store.test.ts` (both import lists; U1a's watch-only register test AND U1b's B5 `exitText` test) |
| u2 | clean (U2/U3 reduce.ts and Message.tsx hunks disjoint) |
| u3 | clean |

Cross-file items applied by Z: E1's document expectations, `Kernel::kill` doc, cells skill quotes; E2's actor-level B14 test (`e2e_real_shells`, stopped `sleep 5`: no `133;D`/`printf` in `out.txt`); U1a `ponytail:` → `limit:` in `open-as-note.ts`; U1b's output-index ceiling closed (`OutputArea` scopes `OutputDisclosure` per output; stream key drops `text.length`). Verified, no change needed: E2's `.cell.log` (only `pty.rs` builds a kernel log path; every consumer takes `job.log`), E2's joined ansi rows vs `trimBlankTail`, U2's `registeredCells` (still used by `CellFence`), U3's json-output (`usePathOpen` uses `useDisclosure`).

## Gates (merged tree)

| gate | result |
|---|---|
| `mbx check -p cells -p surface -p agent -p rpc -p tools -p tui -p document -p theuth` | 0 warnings, 0 errors |
| `mbx clippy` same set `--no-deps -- -D warnings` | 0 warnings |
| `mbx check -p desktop` | 0 warnings |
| `mbx test -p cells -p surface -p rpc -p tools -p tui -p document` | 778 passed, 0 failed, 3 ignored |
| `mbx test -p agent --lib` (`THEUTH_MONTY_WORKER=THEUTH_WORKER=target/debug/t`) | 677 passed, 3 failed: `repl::tests::capping_binds_out_name` (known red on main, same read_file/list_dir message); `supervisor::tests::human_input_releases_parked_items_as_steers` and `sensor::tests::monty_budgets_and_allowlist` (4.14 s budget) fail under load (load avg ~27) and pass alone, as does `human_input_promotes_a_queued_child`; `sensor/` is untouched by 171F |
| `mbx test -p desktop --lib` | 69 passed |
| bindings `export_bindings` | 8 passed; 989 files; `methods.json` lists `cell_reply_note_get/set`; `PassEnd.ts`/`PassStop.ts` present |
| `bunx tsc -b --noEmit` (TS 7.0.2) | 0 errors (a probe error file proved it type-checks) |
| `bunx vitest run` | 296 files, 1902 tests passed |
| `bun run build` (vite) | built |
| safety diff `5628e678 -- tools/registry.rs tools/approval.rs crates/double` | empty |

## Live proof

Scratch engine `/tmp/171f-z/bin/t` (this tree's debug build) + `theuth-documents-59.5`, data dir `/tmp/171f-z/engine` (rpc 127.0.0.1:7791, `secrets.key_store: file`, mesh relay/discovery off, network plugins off), started with `env -i HOME=/tmp/171f-z/home PATH=/tmp/171f-z/fakebin:/usr/bin:/bin:/opt/homebrew/bin SHELL=/bin/zsh AWS_CONFIG_FILE=/dev/null AWS_SHARED_CREDENTIALS_FILE=/dev/null`. Memory gate 10.8 GB free+speculative at start. `command -v aws pulumi terraform kubectl helm gh` resolved to `/tmp/171f-z/fakebin/*` in a plain shell, a zsh login shell and a bash login shell (the fake home's rc files re-prepend fakebin after `path_helper`), and again inside a cell (`isolation`). No provider keys, no model calls. Notes made with RPC `doc_create` (`doc_insert` is not on the RPC), cells pressed in the desktop UI (vite dev server on the scratch engine) in headless Chrome over CDP. Screenshots in `/tmp/171f-z/proof/`. All processes stopped afterwards.

| bug | result | evidence |
|---|---|---|
| B3 | pass: `echo one; echo two` → 2-line output, whole cell 247 px | `B3-two-lines.png` |
| B5 | pass: `timeout: 3s` → "timed out after 3 s", footer "✗ timed out", output `^C` | `B5-timeout.png` |
| B10 | pass: "Monty can't run this cell: line 1: no module named pandas — …" and "line 2: No overload of function `sum` matches arguments", each with "A regular Python can run it · Use Python"; Use Python → `#| python: auto`, chip `auto → python 3.9.6 (system)`, Run enabled (after Z's header re-read; before it the notice stayed until reload) | `B10-monty-reasons.png`, `B10-sum-before.png`, `B10-sum-after-use-python.png` |
| B11 | pass: header "Run all stopped before "step-two" (2/3): folder not found — …", "Run all stopped here" row + red ring; `cell_state.pass` `{total 3, ran 1, before: true}` | `B11-run-all-stopped.png` |
| B12 | pass: live box 168 px at 1.5 s, 420 px (30-row grid) at 7 s | `B12-live-early.png`, `B12-live-grown.png` |
| B14 | pass: Stop on `sleep 120` → `out.txt` = `^C`, summary `^C`, no `printf`/`133;D` | `B14-stop-no-printf.png` |
| B16 | pass: trap cell ignores Stop; Kill (tooltip "Kill — closes the kernel; the next run starts fresh") → "killed — the kernel was closed…", `out.txt` = `^C` (no `after-kill`); next cell prints `mark=fresh` | `B16-stop-ignored.png`, `B16-killed.png`, `B16-next-run-fresh.png` |
| B17 | pass: `seq 1 20000` → `cell_output(run, 0, 200)` starts `1\n2\n3`; `out.txt` 20000 lines, last `20000` | `B17-seq20k.png` |
| B1, B26 | pass (fixture): bar after cell 3, class without `sticky`, "2/3 bash · idle" | `B1-B26-bar-after-last-cell.png`, `fixture-chat-{desktop,phone}.png` |
| B21 | pass (fixture): click + type in a chat cell → "edited" chip, "theuth will see the code that actually ran." | `B21-click-type-edited.png` |
| B8 | fixture bar for the superseded reply reads "Run all · 1/2 · bash"; U1a tests | `fixture-chat-desktop.png` |
| B2, B7, B27 | worker screenshots (not re-shot) | `/tmp/171f-u1b/shots/{b2,b7,b27}-*.png` |
| B4, B6 | not run live (needs a model-driven chat); reducer/render tests | — |

## Open items
- The LIVE terminal of a running shell cell shows the echoed run line (`printf '\033]133;C…'; . …/run.sh; printf '…133;D…'`) on its first rows; the finished record is clean (B14 covers the record only). [INFERENCE] the live view is 171 W1 behaviour, not a 171F regression.
- B4/B6 have no live proof; "Edit of a message that carried results" is untested engine-side (U2).
- `shell()` (and any program the agent runs) can still read cell run records (E2, out of scope).
- U3 limits: json-render elements keyed by content; diagram fullscreen keyed by kind; closed card bodies no longer mount, so find-in-page does not reach them.
- E2 ceiling: a wide line split exactly at the head/scrollback boundary shows as two lines.
- `repl::tests::capping_binds_out_name` stays red (main).

## Deviations
- `NoteCell` re-reads its record 800 ms after its `#|` header changes (code keystrokes still never re-read): without it Use Python left the Monty notice up until a reload.
- `171-cells.md` §6.2, §7.2, §8.3 edited too (they quoted behaviour 171F changed).
- Ratchet: the `ALLOWED` set and its "only shrinks" check were deleted rather than left empty; the test now requires zero hits outside the CHROME list.
