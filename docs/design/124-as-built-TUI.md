# 124 — As built, TUI (`/model` picker: provider → model navigation + word search)

Branch `feat/124-picker-tui`, cut from main f016637e. Worktree `theuth-124-tui`.

## What was built

`crates/tui/src/model.rs`
- 1644–1657 `Menu::Models` gains `provider: Option<String>` (`None` = providers level, `Some(p)` = models of `p`).
- 4452–4465 `UiEvent::Models(items)`: `provider = None`, or `Some(p)` when `providers(&items).len() == 1` (§3.5); cursor placed by `models_home()`.
- 8177–8212 `Model::models_rows() -> Vec<ModelRow>` — the three row sets (providers level with Current row; a provider's models by name; search = Current row then `rank_models` minus the current id).
- 8214–8222 `Model::models_home()` — cursor to the first row after a Current row (first provider / best match), else 0.
- 8320–8398 `Model::models_key()` — ↑/↓ clamped; Char → filter + home; Backspace pops, or on an empty filter with `provider.is_some()` and >1 provider goes up; Left goes up (no-op on the providers level / one provider); Right and Enter on a Provider row descend (cursor 0); Enter on Current/Model sends `Command::SetModel { session, model }` and closes; Esc closes.
- 9595–9611 `pub enum ModelRow<'a> { Current(&ModelInfo), Provider { id, count, current }, Model(&ModelInfo) }`.
- 9613–9690 search: `search_terms`, `boundary_match`, `term_tier` (tiers 100/90/80/70/65/60/40/30), `pub fn rank_models` (per-term gating of 40/30 when any model reaches ≥ 60; sum; stable sort by score desc, catalog order on ties).
- 9692–9706 `pub fn providers(items) -> Vec<(String, usize)>` (BTreeMap, A→Z) and `pub fn provider_models(items, provider)` (name case-insensitive, then id).
- `filter_models` and `Model::models_filtered` deleted.
- 17009–17311 tests (see below).

`crates/tui/src/ui.rs`
- 20 imports `ModelRow`, `providers`.
- 998–1043 `Menu::Models` arm of `menu()`: titles per level (`model  ⏎ open provider  type to search all models  esc close`; `model › {p} ({n})  ⏎ set  ←/⌫ back  esc close`, back words dropped with one provider; `model  search: {filter}▏  ⏎ set  esc close` + `  no match` when only the Current row remains); rows `{●| } {id:<44} {name:<32} {k(context)}{ · vision}` and `{●| } {provider:<24} {count:>4} models`. `fit` clipping and the `›` cursor glyph unchanged.
- 2273–2279 `menu_box_is_tight_at_any_width` builds the menu on `provider: Some("amazon-bedrock")` so its ten rows still show.

## Decisions taken beyond the design

- `ModelRow` carries the `current` flag for provider rows and the Current/Model split so `models_key` and `ui.rs` share one row list; no `Grouped<T>` heading rows (nothing is non-selectable here).
- Model rows gained a 2-char marker column (`● ` / `  `) in every level, so the current model is visible on its provider's level and in search results, not only as the Current row.
- Backspace on an empty filter on the providers level (or with one provider) is a no-op — it does not reset the cursor.
- Search terms via `split_whitespace()` (equivalent to trim + split on whitespace, empties dropped).
- `rank_models` with no terms returns the catalog unchanged (browse mode is decided by `filter.is_empty()` in `models_rows`, so this path is only reached by the §2.3 `"  "` vector).

## Tests

New, in `crates/tui/src/model.rs` `mod tests`:
- `rank_models_matches_the_design_vectors` — every row of §2.3 (13 queries incl. the empty `xyz`) against the fixture catalog verbatim, plus the `"  "` browse-mode case.
- `providers_count_a_to_z_and_provider_models_sort_by_name_then_id`
- `models_picker_opens_on_providers_and_navigates_levels` — opens on providers with Current first and cursor 1; Up/Down clamp; Enter/Right descend; Left/Backspace-on-empty ascend; Left no-op at top; Enter on a model sends `SetModel` and closes.
- `models_picker_search_covers_all_models_and_returns_to_the_level` — typing from a provider level searches every model, Current row kept though it does not match, cursor on first result; a query matching only the current model shows it once; clearing returns to the remembered provider; Enter picks the best match (`GPT-5.4` → `cursor/gpt-5.4`).
- `single_provider_catalog_opens_on_its_models_level_and_stays_there` — §3.5; Left/Backspace stay; search and clear return to the models level.

Changed: `ui.rs` `menu_box_is_tight_at_any_width` — same assertion, menu now on the provider's models level (the providers level would show 2 rows, not 10).

Gate (all through `build-lock`, `BUILD_LABEL=124-TUI`):

```
cargo fmt -p tui                                    → (clean, no diff)
cargo check -p tui --all-targets                    → Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 16s
cargo clippy -p tui --all-targets -- -D warnings    → error: could not compile `talk` (lib) due to 1 previous error
    (crates/talk/src/policy.rs:580 clippy::drain_collect — pre-existing on main f016637e, crate not mine; see Hand-offs)
cargo clippy -p tui --all-targets --no-deps -- -D warnings
                                                    → Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.71s
cargo test -p tui --no-fail-fast
    Running unittests src/lib.rs               → test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.62s
    Running tests/no_emoji.rs                  → test result: FAILED. 0 passed; 1 failed  (surface/src/slack.rs:177 `▶` — pre-existing, crate not mine; see Hand-offs)
    Doc-tests tui                              → test result: ok. 0 passed; 0 failed
```

## Hand-offs

- `crates/talk/src/policy.rs:580` — `self.pending.drain(..).collect::<Vec<_>>()` trips `clippy::drain_collect` under toolchain 1.98.1; `cargo clippy -p tui` lints workspace path deps and so fails before reaching `tui`. Fix is `std::mem::take(&mut self.pending)`. Unchanged from main; owner of `crates/talk`.
- `crates/surface/src/slack.rs:177` — `TimelineState::Active => "▶"` (U+25B6) fails `crates/tui/tests/no_emoji.rs` (`no_emoji_in_terminal_or_chrome_strings`). Unchanged from main; owner of `crates/surface`. Either swap the glyph or append `// no-emoji: allow`.
- Desktop (`crates/desktop/ui/**`) implements the same §2 spec in TypeScript; the Rust vectors here are the reference for `model-search.test.ts`.

## Open items

- None in TUI scope. `tui` itself is fmt/clippy(--no-deps)/unit-test clean; the two red gate lines above are in crates outside this brief's ownership.
