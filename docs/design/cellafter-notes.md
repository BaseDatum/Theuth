# CELLAFTER — `after:` gates warn, Run anyway runs past them

Sky (2026-11-06): a line ▶ on `commands` (`#| after: prepare`) showed "the prepare run is more than 1 h old — run it again", a
disabled "▷ after: prepare", and ran nothing. Ruling: keep the check, warn, and let a second press past the warning run.
Design 171 §7.6 carries the behaviour; this file records the calls made where the brief left room.

## What changed

- **Engine.** `cell_run` takes a 7th positional param `confirmed: Option<bool>` (omitted / `null` / `false` = as
  before). It reaches `RunRequest.confirmed` → `prepare(…, confirmed)` → `refuse(…, confirmed)`, which skips only
  the `Disarmed` check and the danger `mode != cell` check: changed/superseded, invalid, missing cwd, member ack and
  interpreter resolution run unchanged. Run all calls `prepare(…, false)` and still stops before every danger cell
  (`pass_boundary`): no batch override.
- **Record.** Migration `0005_cells_ran_past.sql` adds the nullable `cell_runs.ran_past TEXT`; `CellRun.ran_past:
  Option<String>` (`#[serde(default)]`). The agent's `cell_runs` rows carry it too.
- **Delivery.** `<cell_result>`'s body starts with the `ran_past` line (private runs included).
- **State.** `CellState.disarmed: Option<String>` — the engine's reason, so the UI can warn before any press.
- **UI.** `view.ts` `disarmedReason(state, cell)` is the one gate test. `Cell` shows the reason as a red strip and the
  button `Run anyway` (outline + warning tokens; `destructive` kept on danger cells), the reason as `title` and
  `aria-description`; one press runs. `line-run.ts` `pastGate` is the pure two-press rule (`WARN_MS` = 5 s) used by the
  note overlay ► / pill and the chat gutter ► / pill; the warning `<reason> — press ▶ again to run anyway` sits on
  the pressed line. Keys (Shift/Mod+Enter) only hint `press Run anyway to run this`. History entries append the
  `ran_past` line. `CellFence` passes `runnable` unchanged and the reason as `disarmed` to `CellEditor`.

## Calls

1. **One generic wire name, `confirmed`** (first `after_override`; renamed in the same round when the danger addendum
   arrived, so one flag means "the user's second press past a warning" for both rules). `cell_run` serves cell, line
   and selection presses (its `mode`), so one param covers all three.
2. **Stored line, not the bare reason.** `ran_past` holds `ran past after: <name> — <why>` built by
   `run::ran_past(name, disarmed_reason)`: it drops the advice tail ("— run it again" / "— run it first") and the
   repeated `after: <name>` head, so it never reads "after: prepare — after: prepare — run it first". The history
   entry and `<cell_result>` print it verbatim; the reason is in it. A new column, because no existing field fits
   (`meta.json` is not read by history).
3. **`cell_state` carries the reason.** Before, the reason reached the UI only as a refused press's error. The warning
   must be on screen before the press ("that press IS the second press"), so the engine sends it; a missing field (an
   older engine) falls back to `after: <name> is not armed`.
4. **The host derives the override from the gate**, not per path: while `disarmedReason` is non-null every press that
   reaches `cell_run` sends `confirmed: true`. Only the button and a ▶ past its warning get there; keyboard paths
   stop at the hint first. If the UI's view is stale (it shows armed, the engine has since disarmed) the press is
   refused exactly as before, the red notice says why, and the refresh then shows Run anyway.
5. **The gate wins the button text** over `label`, "Run again" and "Run Alice's edit" while unarmed; `member_ack` is
   still sent and the member row still shows.
6. **The strip shows whenever the gate is unarmed and the cell can be pressed** (not on invalid, superseded, guest or
   read-only cells, where it would only add noise).
7. **Warning text.** `<reason> — press ▶ again to run anyway`, except that a reason ending "— run it again" drops that
   advice first ("the prepare run is more than 1 h old — press ▶ again to run anyway"; ruling of 00:29).
8. **"The same ▶"** = the same line number (a selection counts as its first line), within 5 s of the warning; another
   line warns afresh. The window check is pure (`pastGate`, clock passed in); the display timer only hides the text.
9. **Forwarding.** The 7-tuple goes to the chat owner / note hub as is. [INFERENCE] An older remote engine's jsonrpsee
   positional parsing ignores the trailing param, so it refuses as before — no worse than today.
10. **Prompt and skill.** The prompt keeps "an apply gets `#| after:` naming its preview" and adds "(only an apply:
    `after:` is no ordering hint for read-only steps)". The skill's options row says the cell warns and can run anyway;
    its run-sheet rule no longer turns stop lines into `after:` (Run all stops at a failed step); the DR example's
    read-only `verify-writer` loses its `after: promote-replica`.
11. **Test fake.** `actor/tests.rs` `Blocks` now answers `markdown` with its blocks as fences, so a note cell's `after:`
    finds the other cell's name (it validated as `after_unknown` before). Existing note tests are unaffected (their
    blocks carry no names).
12. **Docs.** 171 §1 (what changes), §3.2 `after` row, §3.5, §3.8, §7.6, §10.1 (prompt copy), §10.2 outline, §11.2
    disarmed row, §12.1 inv. 8, §15.2 `cell_run` row. §7.2 (Run all stops before an unarmed `after`) is unchanged.

## Addendum: danger cells get the line/selection ▶, and ask (Sky, 00:01 and 00:05)

"why don't I have the little play button to run a line in the danger cells" (00:01), then (00:05, replacing the
two-press version of 00:01): "the play buttons in danger cells should ask for confirmation, and have a don't ask me
again for this cell checkbox". Before, §7.1 hid the ▶ and the pill on danger cells and the engine refused
`mode != cell` for them (`CellsError::Danger`).

- **Engine.** That refusal now yields to `confirmed` (call 1). The whole-cell button still needs no flag; keys and Run
  all are unchanged (keys never reach `cell_run` on a danger cell; Run all checks `pass_boundary` before preparing).
- **UI.** The note overlay ► / pill and the chat gutter ► / pill show on danger cells, danger-toned (`text-danger`;
  the pill drops its `⌘↵`). `line-run.ts` `lineGuard(danger, askSkipped, disarmedReason)` decides each pointer press:
  `confirm` (danger, not skipped; carries the gate's reason when also disarmed), `warn` (disarmed, non-danger or
  skipped: the two-press hint of §7.6), `run`. `LineConfirm.tsx`: "Run this line of <title>?" / "this selection",
  the reason, "Don't ask again for this cell", **Run line** / **Run selection**, **Cancel**; Esc cancels.
- **Calls.**
  - **Two components, not one.** The non-danger after hint stays the inline two-press text (briefed, and it needs no
    checkbox); the confirm serves danger only. A disarmed danger cell gets one confirm with the reason and no hint.
  - **"Don't ask again" storage.** No per-user persisted UI-preference store exists (`lib/tool-runs` keeps only the
    collapse flag in localStorage, the rest in memory), so it is localStorage, like the theme:
    `cells.skip-danger-line-confirm:<identity>`, per device. `cellIdentity` = `<scope key>/<name>` (survives edits,
    reloads, a re-presented chat cell), else the ref (`refKey`: note block id / chat position). It skips the ask
    only: an unarmed gate still warns first; the button, the keys and Run all ignore it.
  - **Not a modal.** The confirm is an inline absolutely-positioned box (`contentEditable={false}`, keys and mousedown
    kept inside), no Radix popover/dialog, no `aria-live`: Radix `hideOthers` never runs inside the editor (CELLHIST).
    The two-press hint lost its `role="status"` for the same reason.
  - `<title>` is the cell's title (`label`, else `name`, else `cell`), the same name the header shows.
  - **Test reach.** The note ▶ is pointer state and the pill needs a live BlockNote selection; the server-render
    harness has neither (`editor` is null there), so the render-side test is CellEditor's `gutterMark` (the decision
    the CodeMirror gutter renders) and the note ▶/confirm are proven live.

## Addendum: the ▶ runs a command (Sky, 00:05)

"I can get a play button for each line, but it's just one command across 3 lines" — one soft-wrapped `gh run list …
--jq '…'` with a ▶ on a wrapped row.

- `units.ts` `commandUnits(code, lang)` (pure, tested) splits the code lines into commands: shells join `\`
  continuations, a trailing `|`/`&&` (added: same "still one command" shape), open quotes, heredocs (`<<`, `<<-`,
  quoted tags; not `<<<`), open `(`/`{`, `if…fi`, `for|while|until|select…done`, `case…esac` (an unmatched `)` is a
  case pattern); python joins open brackets, triple quotes, `\`, a `:` header with its indented body and its
  `elif`/`else`/`except`/`finally`, a decorator with its `def`; pwsh joins backtick continuations and open brackets;
  JS/TS stay per line. Conservative: an unclosed construct runs to the end. Keywords (F5, 00:56) count only in
  command position — a line start not joined by `\`, after `;` `&` `|` `(` `{` or a case pattern's `)`, or after
  `if`/`while`/`until`/`then`/`do`/`else`/`elif`/`!`/`time` — never as an argument, in quotes or in a `#` comment, so
  `echo done` or `grep fi file` inside a loop keeps it open.
- A one-line command runs as `line` (unchanged wire); a longer one as `selection` of exactly its lines
  (`unitMode`, UTF-8 byte offsets), so the engine needs nothing new. The mark and ✓/✗ sit on the command's first
  line. Soft wrap: hovering any row maps to its logical line → its command → one ▶ at the command's first line
  (`lineTop` of that line, not the hovered row). Mod+Enter on a caret line runs the caret's command too (call: the
  keyboard and the ▶ agree; a selection still runs exactly the selection).

## Proof

- Gates: `mbx check -p cells -p rpc --tests` 0 warnings; `mbx test -p cells` 102 passed (new:
  `actor::tests::run_anyway_skips_only_the_after_gate`, `actor::tests::run_all_still_stops_before_an_unarmed_after`,
  `run::tests::ran_past_names_the_gate_and_why_once`); bindings exported (996 files) and copied, `tsc -b` 0 errors;
  vitest (cells, TerminalViewer, disclosure) 20 files / 127 tests; `vite build` ok.
- Live: `target/debug/t` from this branch, data dir `/tmp/cellafter/eng`, RPC `127.0.0.1:18571`, vite `18572`, headless
  Chrome over raw CDP (`/tmp/cellafter/p2.ts`, scratch). Note "CELLAFTER proof": `prepare` (`echo prepare ok`) and
  `commands` (`#| after: prepare`, `echo commands ok` / `echo second line`). Screenshots `/tmp/cellafter/shots/`:
  `01` strip "after: prepare — run it first" + enabled Run anyway; `02` one press ran it, history "00:26 · exit 0 · 0 s ·
  ran past after: prepare — it had not run"; `03` line 2 ▶ first press: "after: prepare — run it first — press ▶
  again to run anyway" by the line, no run recorded; `04` second press ran line 2 only (record `mode: line 2`, output
  `second line`); `05` after prepare ran: "armed by prepare ✓ 00:27 · prepare ok", normal `Run`; `06` Run all from the
  top: one pass, prepare then commands, both done; `07` light, the history with both ran-past entries.
- Addendum gates: check 0 warnings; `mbx test -p cells` 102 passed (`run_anyway_skips_only_the_after_gate` now also
  proves a danger line/selection is refused without `confirmed` and prepared with it); bindings re-exported (996) and
  copied, `tsc -b` 0 errors; vitest 20 files / 129 tests (new: `line-run.test.ts` danger `warnFor`, `Cell.test.ts`
  danger `gutterMark`); `vite build` ok.
- Addendum live (rebuilt `t`): note "CELLAFTER danger proof", `wipe` (`#| label: Wipe scratch`, `#| danger`, `echo
  danger one` / `echo danger two`). `08` hovered line 2 shows the danger-toned ▶; `09` first press: "Wipe scratch is a
  danger step — press ▶ again to run this line", no run recorded; `10` second press ran line 2 only (record
  `mode: line 2`, output `danger two`). A raw `cell_run` line press without `confirmed` (`false` and `null`) is refused
  `-32020 "press Wipe scratch to run this"`.
- Addendum 2 gates: no Rust change since the addendum-1 gates (`git diff --stat d611ac19 -- '*.rs' '*.sql'` empty);
  `tsc -b` 0 errors; vitest 21 files / 138 tests (new: `units.test.ts` ×8 — Sky's soft-wrapped gh line, shell
  continuations/pipes/quotes, heredocs, blocks/brackets/case, python brackets/blocks/clauses/decorators/triple quotes,
  pwsh, the selection byte range; `line-run.test.ts` `lineGuard` and the "Don't ask again" identity/storage);
  `vite build` ok (14720 modules).
- Addendum 2 live, danger confirm (note "CELLAFTER confirm proof": `wipe2` danger, `apply2` danger + `after: wipe2`,
  then 30 prose sections): `11` the ▶ on line 2 opens "Run this line of Wipe scratch?" under the line; with it open
  3 s: TaskDuration 0.034 s, 0 mutations inside `.ProseMirror`, 0 `aria-hidden` on editor blocks, 0 `aria-live` in
  the editor; Esc closed it, nothing ran; Run line ran line 2 only; `12` "Don't ask again" ticked → run; `13` the
  next ▶ ran at once, and again after a reload (localStorage); `14` `apply2`: one confirm carrying "after: wipe2 was
  edited since it ran — run it again", no separate hint.
- Addendum 2 live, command units (note "CELLAFTER command proof" at 860 px): Sky's gh line (behind `echo`, never run
  against an account) wraps to 5 rows; hovering each row shows the same single ▶ "Run line 1" at the line's top
  (`15`); it ran `line 1` and printed the whole command (`16`). A 3-line `\` continuation: hovering line 3 shows one
  ▶ "Run lines 1–3" on line 1 (`17`); it ran `selection 0..26`, output `one two three` (`18`). The chat cell's
  confirm (CellEditor) shares `lineGuard`/`LineConfirm` but has no live proof: a scratch chat needs a model reply.
- Round 00:29 (hint trim, on fe716870): all four gates re-run — check 0 warnings; `mbx test -p cells` 102 passed;
  bindings 996 + `tsc -b` 0 errors; vitest 21 files / 138 tests; `vite build` ok. A and B re-proven on this code
  (same scripts, fresh notes: `11`–`18` refreshed; open confirm 3 s: TaskDuration 0.003 s, 0 editor mutations, 0
  `aria-hidden`, 0 `aria-live`). `19`: a failed gate's line ▶ hint reads "after: gate3 did not succeed — press ▶
  again to run anyway" (the strip keeps the engine's full reason).

## Review fold-in (00:40, part 1 approved)

- **F1 explicit override.** Nothing derives `confirmed` any more: `pressNote(…, confirmed = false)`, NoteCell
  `run(mode, confirmed = false)`, CellFence `run(mode, next, confirmed = false)`, `CellCallbacks.onRun(mode,
  confirmed?)` (it replaced the never-used `edited?` argument) and CellEditor's `onRun(mode, next, confirmed)`. Only
  these pass `true`: the Run anyway button (`Cell`: `onRun(cell, gate !== null)` — the button is Run anyway exactly
  then), the second same-line ▶ of a warned press (`lineGuard` `warn`), the danger confirm's Run, and a danger ▶ whose
  cell is on "Don't ask again" (that consent). Keys pass nothing. Tests: `pressNote` without the flag sends `false`
  (`NoteCell.test.ts`); keys on a disarmed cell never call run (`Cell.test.ts` `keyRun`); `rpc::cells::tests::
  cell_run_without_confirmed_is_an_unconfirmed_press` — six params parse and are answered exactly like `null`/`false`.
- **F2 member edits.** Unarmed + a member edit: the button reads `Run Alice's edit anyway` (`member_ack` still sent),
  and the keyboard hint names that button. Test in `Cell.test.ts`.
- **F3** `ran_past` is in the `cell_runs` tool description, 171 §9.5 and `cell_runs_rows_carry_every_key` (value
  asserted).
- **F4** 171 §12.2's stale-preview row: warn + Run anyway (recorded `ran_past`), the danger ▶ asks first, Run all stops.
- Migration 0005 unchanged. The scratch engine only ever used `/tmp/cellafter/eng`.
- Fold-in gates: check 0 warnings; `mbx test -p cells` 102 passed; rpc `cell_run_without_confirmed_is_an_unconfirmed_press`
  ok; bindings 996 + `tsc -b` 0 errors; vitest 21 files / 139 tests; `vite build` ok. Live (`/tmp/cellafter/p8.ts`):
  the Run anyway button ran with `ran_past` recorded; a line ▶ on the disarmed cell — first press ran nothing, second
  ran line 2 past the gate; A (confirm, Esc, don't-ask, reload, danger + disarmed) and B (soft wrap, continuation)
  re-run green (open confirm 3 s: TaskDuration 0.001 s, 0 editor mutations, 0 `aria-hidden`, 0 `aria-live`).
- Round 00:56: F5 above (tests in `units.test.ts`: `for f in *; do echo done; done`, `if grep -q fi x; then echo ok;
  fi`, a `# done` comment, quoted `'esac' "fi"`, keywords after `|`/`&&`, a `\` join). F6 was already in 567d8c59
  (`lineGuard` test with the 1 h reason). F1–F4 in fd336a96.

## Review of part 2 (01:06)

- **M1 stale confirm.** An open ask now carries the cell text of its ▶ press (`line-run.ts` `Ask.text`); Run sends
  that text (notes: its digest is `seen`; chat: it is the `edited` code), so a cell changed since is refused "this
  cell changed" (inv. 5). Both hosts also close the ask on any text change (an effect on `code` through `askStep`
  `code`), and the box closes on a mousedown outside it. `askStep` is the one pure state machine (run / cancel / code).
  Live: an edit while the box was open closed it, nothing ran; a raw `cell_run` with the press-time `seen` on the
  edited block answered `-32020 "this cell changed — look again"`.
- **S1** focus goes to Cancel (`[data-cancel]`, `preventScroll`). **S2** the question names the command: "Run this
  line of X?", "Run lines 3–5 of X?", "Run this selection of X?" (`confirmText`); the box sits under the command's last
  line. **S3** the two-press warning is spoken through CELLHIST's `announce()` (the one live region on `<body>`).
- **S4** (no DOM in vitest, so a pure host function plus server render): `LineConfirm.test.ts` — no `aria-live`, Cancel
  is the focus target, the question texts; `askStep`: Cancel/Esc/outside never runs, a changed text closes, Run is
  confirmed on the press-time text and `pressNote` sends its digest (not the current one), a ticked skip persists and
  the next ▶ runs at once.
- **S5** `units.ts`: bash `$'…'` with `\'` escapes; a python comment ending in `\` joins nothing; a comment-only
  (even column-0) line never ends a python block; pwsh joins a trailing `|` or `,` (outside comments). Tests in
  `units.test.ts`.
- No Rust change in this round (engine gates from 2f4fe190 stand).
