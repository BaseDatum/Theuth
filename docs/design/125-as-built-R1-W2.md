# 125 R1 as-built — W2 (TUI)

Branch `feat/125-r1-w2`, cut from the contract commit `d243f863`. Spec: `docs/design/125-advisor.md` R1.2 (TUI column), R1.3, R1.7, R1.8/R1.9 "W2".
Only `crates/tui/src/{model.rs, ui.rs, lib.rs}` and this note were touched.

## Built (file-level)

- `crates/tui/src/model.rs`
  - `advisor_status_lines`: fifth line `consults: N this turn · M total · cap C` from `AdvisorStatus.consults` (one line when off, unchanged).
  - `advisor_cmd`: `consults <n>` arm → `Command::SetAdvisorMaxConsults(n)` for `n` in 0-32 (no session needed — the cap is an engine
    default); anything else → `Entry::Error("/advisor consults: expected a number 0-32, got …")`. `COMMANDS` row text gains `consults <n>`.
  - `advisory_title`: `◎ consulted: <question>` for `AdvisoryKind::Consultation` (whitespace runs, including newlines, collapsed to one
    space); the review title is unchanged.
  - `advisory_rows` (new): the card's body rows — `(Some(severity), note)` per note for a review, `(None, line)` per answer line for a
    consultation; `advisory_row_count` (new, allocation-free) for `Entry::height` and the Ctrl+O guard in `toggle_card`;
    `advisory_lines` (printer) is built on `advisory_rows`, so a consultation prints its answer lines unbadged.
  - `set_advisor_status`: after `advisor → <model>`, prints `advisor: <warning>` as one `Entry::Info` when `AdvisorStatus.warning` is
    set and either the link changed (model / off state) or the warning text itself changed; counter-only `AdvisorChanged`s print nothing.
  - `Command::SetAdvisorMaxConsults(u8)`.
- `crates/tui/src/ui.rs`: `Entry::Advisory` arm renders `advisory_rows` — badged `MixedText` for review notes, plain wrapped `Text` for
  answer lines; `… +N more lines` (consultation) / `… +N more notes` (review); title/colour via `advisory_title`/`advisory_severity`, so
  a consultation card is SAND (`severity_color(Advice) = SAND`, now `pub(crate)` with `SAND` for the model test).
- `crates/tui/src/lib.rs`: `run_commands` arm `SetAdvisorMaxConsults(n)` → `api.set_advisor_max_consults(n)` → `UiEvent::Info("advisor
  consults → n")` (the reply's `max_consults`). The line printer needs no new arm: `print_history`/`print_event` already go through
  `advisory_title`/`advisory_lines`, and `UiEvent::AdvisorStatus` through `advisor_status_lines`.
- Tests: `model.rs` `advisor_status_prints_five_lines` (renamed from `…four_lines`), `advisor_cmd_consults_sets_cap_and_rejects_out_of_range`,
  `consultation_card_titles_with_question`, `consultation_card_collapses_answer_lines`, `advice_severity_is_sand_and_never_promotes`,
  `pairing_warning_prints_once_per_link`; `ui.rs` `consultation_card_is_tight_at_any_width` (the `advisory_card_is_tight_at_any_width`
  pattern, widths 60-120). `tests/no_emoji.rs` unchanged.

## Gates (exact result lines)

- `cargo test -p tui` → unittests `src/lib.rs`: `test result: ok. 152 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` ·
  `tests/no_emoji.rs`: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` · doc-tests: `0 passed; 0 failed`.
- `cargo clippy -p tui --all-targets -- -D warnings` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 7m 40s` — no warnings.
- `cargo fmt -p tui -- --check` → exit 0 (after one `cargo fmt -p tui`).
- Build-environment note: the APFS-cloned target carried path-bound build-script outputs from Sky's checkout (`btls-sys`,
  `sentencepiece-sys` CMakeCache, `apple-cf`/`apple-metal`/`screencapturekit` Swift module caches, `boring-sys`, `knf-rs-sys`); each failed
  with "compiled with module cache path …/Develop/theuth/target…" until `cargo clean -p <pkg>` (one package at a time, under `build-lock`).
  The 60-minute fallback build also raced the still-running clone once (`btls-sys` "Path … exists"). Worth a line in the setup script.

## Deviations from the design (and why)

1. **Collapse unit for a consultation is answer *lines*, not notes**: the answer is one `Advice` note with several lines (≤ 12 asked of the
   advisor); collapsing by note would never collapse. `ADVISORY_COLLAPSED` (3) lines show until Ctrl+O; the `… +N more` row says `lines`.
2. **`/advisor consults <n>` needs no session**: the cap is `advisor.max_consults`, an engine default set by `set_advisor_max_consults(n)`
   (no `SessionId` in the RPC), so the command works before the first message, unlike the other `/advisor` arms.
3. **The `advisor consults → n` info line comes from the RPC reply** (`AdvisorDefaults.max_consults`), not from `AdvisorChanged`: the
   change is a default, not a link event, and may arrive with no link at all.
4. **The pairing warning prints as `advisor: <warning text>`** (the engine's string, prefixed) — the design gives no exact wording for the
   TUI line, only "one `Entry::Info` line after `advisor → <model>`". It re-prints if the engine changes the warning text for the same
   link; otherwise once per link (model change / off→on).
5. **`consultation_card_titles_with_question`** also pins that `advisory_lines` of a review is still `[sev] note` — the printer shares the
   row function.

## Hand-offs for the closer

- None required for compile. Smoke (R1.9 closer step 3): `/advisor status` prints `consults: 0 this turn · 1 total · cap 6` as the fifth
  line once W1 fills `AdvisorStatus.consults`; the TUI only renders what the engine sends.
- The TUI prints `AdvisorStatus.warning` verbatim after `advisor: ` — W1's wording lands on screen as is.

## Open items

- No new key chord: Ctrl+O still toggles the newest expandable entry; a consultation whose answer has ≤ 3 lines is skipped like a review
  card with ≤ 3 notes.
