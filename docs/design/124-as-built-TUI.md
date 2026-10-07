# 124 as-built — TUI `/model` picker

Branch `feat/124-picker-tui` (from main f016637e). Implements design 124 §2, §3 (TUI parts), §3.5, §6 for `crates/tui`.

## What was built

`crates/tui/src/model.rs`
- 1644–1654 `Menu::Models` gains `provider: Option<String>` (the models-of-a-provider level when `Some`).
- 4448–4466 `UiEvent::Models`: opens on the providers level; a single-provider catalog opens on that provider's models level (§3.5); cursor placed by `models_home`.
- 8177–8211 `Model::models_rows() -> Vec<ModelRow>`: providers level (Current row, then `providers()`), models level (`provider_models()`), search mode (Current row first and removed from the ranked results).
- 8213–8221 `Model::models_home()`: cursor → 1 when a Current row leads a non-empty list, else 0 (first provider / first search hit / first model).
- 8319–8396 `Model::models_key()`: ↑↓ clamped; `Char` appends and re-homes; `Backspace` pops, or ascends when the filter is already empty (multi-provider only); `Left` ascends (no-op on the providers level / single provider); `Right`/`Enter` on a provider descends; `Enter` on a model sends `Command::SetModel` and closes; `Esc` closes.
- 9595–9611 `pub enum ModelRow<'a> { Current(&ModelInfo), Provider { id, count, current }, Model(&ModelInfo) }`.
- 9613–9706 pure functions replacing `filter_models` (deleted): `search_terms`, `boundary_match`, `term_tier` (tiers 100/90/80/70/65/60/40/30), `pub fn rank_models` (per-term gating, sum, stable sort by score desc then catalog order; browse mode returns the catalog), `pub fn providers` (BTreeMap → A→Z with counts), `pub fn provider_models` (name case-insensitive, then id).
- 17009–17290 tests (see below).

`crates/tui/src/ui.rs`
- 20 imports `ModelRow`, `providers`.
- 998–1043 `Menu::Models` arm of `menu()`: titles per level — providers `model  ⏎ open provider  type to search all models  esc close`; models `model › {provider} ({count})  ⏎ set  ←/⌫ back  esc close` (`←/⌫ back  ` dropped with one provider); search `model  search: {filter}▏  ⏎ set  esc close` plus `  no match` when only the Current row remains. Rows: `{●| } {id:<44} {name:<32} {k(context)}{ · vision}` for models, `{●| } {provider:<24} {count:>4} models` for providers. `fit` clipping and the `›` cursor glyph are unchanged.
- 2273–2279 `menu_box_is_tight_at_any_width` now builds the menu on `provider: Some("amazon-bedrock")` so it still shows ten rows.

## Decisions taken beyond the design

- Pure functions live in `model.rs` next to the old `filter_models` rather than a new `models.rs`: declaring a new module needs a line in `lib.rs`, which this worker does not own.
- `rank_models` with no terms (browse mode) returns every item in catalog order; `models_rows` never calls it in that state (the empty filter selects the browse levels), so this only fixes the function's contract for callers/tests.
- Boundary characters are "not ASCII alphanumeric" on the lowercased strings — identical to the design's `[a-z0-9]` word definition.
- One cursor-home rule for all three states: index 1 when a Current row leads a list of ≥2 rows, else 0. It yields exactly the design's "first provider" / "first result" / "first model" positions.
- `Right` on a Current/Model row and `Left` on the providers level are no-ops; `Backspace` on an empty filter at the providers level is a no-op (the menu stays open).

## Tests

New in `crates/tui/src/model.rs`:
- `rank_models_matches_the_design_vectors` — every row of §2.3 (ids in order) against the §2.3 fixture catalog, plus `xyz` → empty and `"  "` → browse mode (no terms; catalog returned as is).
- `providers_count_a_to_z_and_provider_models_sort_by_name_then_id`.
- `models_picker_opens_on_providers_and_navigates_levels` — Current row first, cursor on the first provider, ↑/↓ clamping, Enter/Right descend, Left/Backspace-on-empty ascend, Enter on a model sends `SetModel` and closes.
- `models_picker_search_covers_all_models_and_returns_to_the_level` — typing from the models level searches all models with the Current row on top (also when it does not match, and listed once when it does), cursor on the first result, clearing the filter returns to the remembered provider, Enter picks the best match.
- `single_provider_catalog_opens_on_its_models_level_and_stays_there` — §3.5; Left/Backspace stay; search still works.

Changed: `ui.rs::menu_box_is_tight_at_any_width` (menu constructed on the provider's models level; same assertions).

Gate (all through `build-lock`, `BUILD_LABEL=124-TUI`):

```
cargo fmt -p tui
  (no output)
cargo check -p tui --all-targets
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 33.10s
cargo clippy -p tui --all-targets -- -D warnings
  error: draining all elements of a collection into a new collection of the same type
     --> crates/talk/src/policy.rs:580:20
  error: could not compile `talk` (lib) due to 1 previous error
cargo clippy -p tui --all-targets --no-deps -- -D warnings      (evidence for this crate)
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.87s
cargo test -p tui
  running 129 tests
  test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
  Running tests/no_emoji.rs
  test no_emoji_in_terminal_or_chrome_strings ... FAILED
  1 emoji line(s): surface/src/slack.rs:177  ▶
  test result: FAILED. 0 passed; 1 failed
```

## Hand-offs

- `crates/talk/src/policy.rs:580` fails `clippy::drain_collect` under the 1.98.1 toolchain (`-D warnings`); it is a workspace dependency of `tui`, so the exact gate line fails before linting `tui`. Not touched (not owned). `tui` itself is clippy-clean (`--no-deps` run above).
- `crates/tui/tests/no_emoji.rs` flags `crates/surface/src/slack.rs:177` (`▶`). Pre-existing on main f016637e; not owned by this worker.

## Open items

- None for the TUI picker. Provider display names (`ProviderInfo.name`) stay out of scope per design §7.
