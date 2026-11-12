# 194 as-built — T (row 22: TUI model pickers)

Branch `feat/194-t`, base 1ded15b6. Only `crates/tui` changed. No engine, RPC, binding or desktop change; S1 not needed, not merged.

## Pickers found

The TUI has one picker widget, `Menu::Models` (`/model`), opened for three targets (`PickTarget`):

| Target | Opened by | Rows | Chips | Model pick | Chip pick |
|---|---|---|---|---|---|
| `Session` | `/model`, bare `/effort` | `list_models` | selected model's `ModelInfo.efforts`, Default first | `set_model` (draft before a session) | `set_effort` (`None` = Default), draft before a session |
| `Advisor` | `/advisor` | `list_models` + `Off` lead row | advisor model's efforts, Default first | `set_advisor(model, effort)` | `set_advisor(advisor model, effort)` |
| `Harness` (new) | `/model`, `/effort` on an ACP session | the harness's `model` select values (provider = `provider/` prefix, else the harness name); without a model select one row for what it runs | the `thought_level` option's values, **no Default chip** | `set_harness_option(<model option id>, value)` | `set_harness_option(<thought_level option id>, value)` |

`/title` and `/vision` take a typed id and have no picker; their efforts (B1's new `title_effort`/`vision_effort` RPCs) do not exist on this base, so the TUI does not set them (no RPC changes in this scope).

## What changed

- **Chip row.** `ModelRow::Efforts(EffortChips { levels: Vec<Option<String>>, active: Option<usize> })` is inserted directly after the picked model's row wherever it shows: Current, its Favorite, its provider's list, search results (`Model::models_rows`, `effort_chips`). No other row gets chips. `None` in `levels` is Default; `active` marks the stored effort (session: `Model.effort`; before a session: the draft's effort, `None` = Default; advisor: `AdvisorStatus.effort`; harness: the option's `current`).
- **Rendering** (`ui.rs` `effort_chips_row`): `   effort  Default   low   ●high `; `●` = active, `[…]` = highlighted chip while the cursor is on the row.
- **Separate effort step removed.** `Menu::Efforts`, `open_efforts`, `efforts_key` and `AdvisorReply::Pick` are gone. A model pick no longer chains into an effort list (`pick_model` draft path, `UiEvent::ModelSet`, the advisor reply).
- **`/effort` keeps working.** `/effort <level|clear>` is unchanged. Bare `/effort` opens the `/model` picker with the cursor on the chip row, on the active chip (`Model.pick_effort`, `open_models(.., chips)`); a model without levels is still the notice `<model> has no reasoning effort levels`. On an ACP session bare `/effort` opens the harness picker the same way; with no `thought_level` option it is `<harness> has no reasoning effort levels`.
- **Harness picker** (`harnesses.rs`: `open_harness`, `harness_groups` = desktop `harnessGroups`, `harness_models` = desktop `HarnessPicker` rows). A harness with neither a model select nor a `thought_level` option still prints `harness_options_lines`; the line printer (`lib.rs`, non-TTY) still prints the lines.
- **Parity fix (D6 "verify parity").** The Current row was hidden when the *session* model was a favorite, even in the advisor picker; it now follows the picked model (`pick_current()`), so the advisor's model shows once, either as its favorite or as Current.
- **Advisor model change (D4c analogue).** Picking a new advisor model sends the advisor's current effort when the new model's levels include it, else `None` (Default). Before 194 the chained effort list asked again.
- **Title never wraps.** The `/model` title is built from hint segments and fitted by `fit_title` to the box's inner width (width − 5): the first segment is clipped if it has to be, every later hint shows whole or not at all. The old note "wraps below width 74" is retired; `menu_box_is_tight_at_any_width` now asserts title + 10 rows at every width 60–120.

## Keys (the scheme)

The chip row is a row of the list:
- ↑↓ move between rows as before; landing on the chip row puts the chip cursor on the active chip (Default when none is active).
- ←→ on the chip row move between chips, clamped at both ends. ← there never goes back a level; ← elsewhere keeps its meaning (back to providers).
- ⏎ on a chip sets that effort and closes the picker, like a model pick. Default clears the effort, like `/effort clear`.
- ⇥ on the chip row does nothing. Typing still filters, and ⌫ still deletes or goes back.
- Title while on the chip row: `<noun> › effort  ←→ choose  ⏎ set  esc close`, where `<noun>` is `model`, `advisor` or the harness name.

No existing key changes meaning on any other row.

## Decisions where the contract is silent

- Harness chips show the option **values** (`low`, `xhigh`), not the display names. That way they read the same as `/effort <value>` and the harness lines.
- Harness rows have no context or vision column (context 0 shows blank). The harness picker has no favorites (⇥ off, no ★ rows), like the desktop `HarnessPicker`.
- The harness picker does not show the desktop's "· after this turn" pending labels. The TUI closes on a pick and announces `… after this turn` from `SetOptionResult.pending` as before.
- Options other than model and `thought_level` (for example `mode`) are not shown in the harness picker, as on the desktop. They are still listed when the harness offers neither model nor `thought_level`.
- Old design docs (02, 124, 125, 148) that describe the chained effort picker are not edited. They are history, and Z owns the 124 amendment.

## Tests

`crates/tui/src/model/tests.rs`:
- `effort_chips_sit_under_the_picked_model_wherever_it_shows` covers Current, favorite, provider list, search (also with no hits), Default active with no effort, the draft's effort before a session, and no chips for a model without levels.
- `effort_chip_keys_pick_the_level_and_close` covers ↑ onto the active chip, ←/→ clamping, ⏎ on Default → `SetEffort { effort: None }`, ⏎ on a level → `SetEffort`, ⇥ inert, ← on a provider level moving the chip and not the level, and no effort step after a model pick or `ModelSet`.
- `bare_effort_opens_the_picker_on_the_chip_row` covers bare `/effort` → `ListModels` → cursor on the active chip → `SetEffort`, and the next `/model` opening at home.
- `advisor_picker_chips_set_the_advisor_effort` covers chips under the advisor model, a chip → `SetAdvisor { model, effort }`, and a new model keeping an effort it takes.
- `acp_harness_picker_has_chips_without_default` covers rows by provider, chips without Default, no stars, chip → `SetHarnessOption(thinking)`, model → `SetHarnessOption(model)`, bare `/effort` on the chips, the no-`thought_level` notice, the one-row harness, and the lines fallback.
- Changed: `effort_sets_clears_and_picks`. Its `open_efforts` list section is replaced by the bare-`/effort` no-levels notice.

`crates/tui/src/ui.rs`:
- `effort_chips_render_under_the_picked_model` checks that the chip line sits right under the picked model and appears only once, that another model's levels are absent, the `[●high]` highlight, the chip-row title, and that at widths 40–120 the box is title + rows with nothing overflowing.
- `menu_box_is_tight_at_any_width` is tightened: exact height at every width.

## Gates (final tree)

- `cargo fmt -p tui`: on this base it reformats the whole crate (main is not rustfmt-clean, and the split `model/tests.rs` is dedented as a whole). Only my hunks were kept, merged three-way onto HEAD; every other file was restored.
- `mbx check -p tui` and `mbx check -p tui --tests`: 0 errors, 0 warnings.
- `mbx clippy -p tui --tests --no-deps -- -D warnings`: clean.
- `mbx test -p tui`: lib 179 passed, `no_emoji` 1, `dependency_limit` 1, 0 failed. The 8 new or changed tests ran a second time filtered: 8 passed.
- No live TUI smoke run. It needs an engine and `~/.theuth`, which this worker may not touch. The ui.rs render tests draw the real `menu()` element at widths 40–120.

## Left

Nothing in row 22.
