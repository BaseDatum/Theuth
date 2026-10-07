# 127 — As built, RUST (favorite models: engine state + RPC + TUI picker)

Branch `feat/127-fav-rust`, cut from main f093e3b8. Worktree `theuth-127-rust`.

## What was built

`crates/agent/src/lib.rs`
- 1146–1164 `GetFavoriteModels` (→ `Vec<String>`) and `SetModelFavorite { model, favorite }` (→ `Result<Vec<String>, String>`), doc-commented.
- 1932–1938 `AgentEvent::FavoriteModelsChanged { models: Vec<String> }` next to `DefaultModelChanged`; 2236–2238 `session()` → `None` for it.
- 2268–2271 `AgentConfig.favorite_models: Vec<String>` (struct-level `#[serde(default)]` already covers a missing key); 2335 default `Vec::new()`.

`crates/agent/src/supervisor.rs`
- 100 import; 126–127 `Agents.favorite_models` (live list); 8066 loaded from `cfg.favorite_models`; 9754 test constructor.
- 6673–6695 `impl Message<GetFavoriteModels>` / `impl Message<SetModelFavorite>`.
- 6697–6731 `Agents::set_model_favorite`: `catalog.resolve` like `set_model` (same `Err` text for an unknown id), no-op when already in the requested state (reply only, no write/event), push/remove keeping starring order, `write_config_value(config.yaml, "agent", "favorite_models", "[\"a/b\", \"c/d\"]")` (`[]` when empty), `publish(FavoriteModelsChanged { models })`, reply the list.
- 10173–10177 test helper `agent_section`; 10179–10268 test `favorites_persist_as_one_flow_sequence_line`.

`crates/rpc/src/api.rs`
- 40 import; 125–128 `Info.favorite_models` (`#[serde(default)]`); 611–615 `#[method(name = "set_model_favorite")] async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>>`; 2414/2445 `info()` asks `GetFavoriteModels`; 3443–3445 server impl (`ask` maps the actor's `Err(String)` to `ASK_FAILED` exactly as `set_model` does). No new type → `ts` export list unchanged; `methods.json` gains `set_model_favorite` from the trait.

`crates/tui/src/lib.rs`
- 143–149 reads `Info.favorite_models` (config fallback `cfg.favorite_models`) → `Links.favorite_models`; 643–645 `Command::SetModelFavorite` → `api.set_model_favorite` → `UiEvent::FavoriteModels`; 1373–1375 and 1454 line printer: `favorites → N models`.

`crates/tui/src/model.rs`
- 107–108 `UiEvent::FavoriteModels(Vec<String>)`; 523–529 `Command::SetModelFavorite { model, favorite }`; 3134–3137 `Model.favorite_models`.
- 4488 reply and 5504–5508 `AgentEvent::FavoriteModelsChanged` → `set_favorites` (8243–8251: sets the list and clamps an open picker's cursor to the new row count); 5945 exhaustive no-redraw arm.
- 8197–8241 `models_rows()`: providers level = `Favorite` rows (favorites order, only ids present in `items`) → `Current` only when the current model is not a favorite → `Provider` rows. Models level and search unchanged (markers come from `ui.rs`).
- 8252–8263 `models_home()`: unchanged formula — a leading `Favorite` row is not `Current`, so home is 0 (first favorite); else as 124.
- 8361–8448 `models_key()`: `Favorite` rows pick like `Current`/`Model` (Enter sets + closes, Right no-op); `Tab` on a Favorite/Current/Model row sends `Command::SetModelFavorite { model, favorite: !favorite_models.contains(model) }`, menu stays open; Tab on a Provider row is a no-op.
- 9659–9660 `ModelRow::Favorite(&ModelInfo)`.
- 17187 test helper arm (`★id`); 17367–17473 test `models_picker_lists_favorites_first_and_tab_toggles_them`.

`crates/tui/src/ui.rs`
- 36–37 `Links.favorite_models`; 117/127 seeded into the `Model`.
- 1009–1056 `Menu::Models` arm: titles gain `⇥ ★ favorite` on all three levels; two-char marker column `{●| }{★| }` before the id on every model row (Current / Favorite / Model) and `{●| } ` on provider rows so the columns line up.
- 2318–2378 test `model_rows_carry_current_and_favorite_markers`.

`crates/slack/src/channel.rs`
- 2540 one ignore arm `| AgentEvent::FavoriteModelsChanged { .. }` in the exhaustive `match` — see Decisions.

## Decisions taken beyond the design

- `crates/slack/src/channel.rs` is outside the brief's ownership list, but its exhaustive `match` on `AgentEvent` stops `slack` (a path dependency of `tui`) from compiling once the design-frozen variant exists; the one-line ignore arm is the only change there. The sibling (DESKTOP) owns nothing under `crates/slack`.
- `AgentConfig.favorite_models` carries no field-level `#[serde(default)]`: the struct already has `#[serde(default)]`, so a missing key is `[]` either way.
- Provider rows also got the two-char marker column (`● ` / `  `) so provider and favorite rows on the same level align; the design specifies it only for model rows.
- A no-op `SetModelFavorite` (already starred / already not) returns the list without writing config.yaml or publishing — the design says "no-op when already in that state"; the test pins that no event follows.
- "Unknown ids rejected with the same error" means *the resolver's* rejection: `Catalog::resolve` (and so `set_model`) passes an uncatalogued `provider/model` through (124/82 behaviour — a working model missing from the catalog is still selectable) and only errors on an ambiguous bare id. `set_model_favorite` is identical; being stricter would make a selectable model unstarrable, and §1 already keeps ids whose provider is unavailable on file. The agent test asserts the real rejection path (`ambiguous model \`eyes\`` with two configured providers) and that the file is untouched.
- On the providers level the Current row is hidden when `favorite_models` contains the current id. When that id is on file but its provider is not keyed, neither row exists (the current model is not in `items` either), which is the 124 behaviour.
- After a favorites change the open picker's cursor is clamped to the new row count (`set_favorites`); it is not moved to follow the toggled row.

## Tests

New:
- `agent` `favorites_persist_as_one_flow_sequence_line` — bare id canonicalises; order = starring order; starring twice does not duplicate; the written line is `  favorite_models: ["testvis/eyes", "testvis/blind"]`; the `agent:` section round-trips through `AgentConfig` (`serde_yaml`); ambiguous bare id → `Err("ambiguous model …")`, file byte-identical; unstar to `[]` round-trips; no-op unstar publishes nothing; the four `FavoriteModelsChanged` lists match the four changes.
- `tui` `models_picker_lists_favorites_first_and_tab_toggles_them` — favorites first (an unkeyed favorite skipped), Current row after them, cursor 0; Tab on a favorite → `favorite: false`; Tab on Current → `favorite: true`; Tab on a provider → nothing; `FavoriteModelsChanged` with the current model starred shows it once as `★`; Enter on a favorite → `SetModel` and close; the `FavoriteModels` reply applies and clamps the cursor; Tab on a provider's models level and in search results toggles the highlighted model.
- `tui` (ui.rs) `model_rows_carry_current_and_favorite_markers` — `●★` / ` ★` / `   ` columns and the `⇥ ★ favorite` title on the providers, models and search levels.

Changed (by design): `menu_box_is_tight_at_any_width` (ui.rs) — the models-level title is 69 cells with `⇥ ★ favorite` (was 55) and wraps onto two lines below width 74 (border 2 + padding 2), so the box there is 13 lines, not 12. The test now pins tightness directly — exactly ten row lines inside the box, the last ten inner lines all rows (no empty space under the list) — and keeps the `== 12` assertion for widths ≥ 74. Row clipping (`fit`) is unchanged.

Gate (all through `build-lock`, `BUILD_LABEL=127-RUST`):

```
cargo fmt -p agent -p rpc -p tui                              clean (also reformatted crates/agent/src/store/accounting.rs — pre-existing unformatted call, committed)
cargo check -p agent -p rpc -p tui --all-targets              Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.77s
cargo clippy -p agent -p rpc -p tui --all-targets -- -D warnings   Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 10s
cargo test -p agent --no-fail-fast                            test result: ok. 429 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 40.50s
cargo test -p rpc --no-fail-fast                              test result: ok. 41 passed; 0 failed; 0 ignored  /  ok. 1 passed  /  ok. 0 passed (doc)
cargo test -p tui --no-fail-fast                              lib: test result: ok. 131 passed; 0 failed; 0 ignored
                                                              tests/no_emoji.rs: FAILED — 21 emoji line(s), all in crates/surface/src (elk_family.rs, export.rs, render.rs: ▶ ⚠ ☹ ☺); pre-existing on main f093e3b8, surface untouched on this branch
BUILD_SLOTS=1 just ts-desktop                                 test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out; finished in 3.23s; bindings/ + crates/desktop/ui/src/rpc/bindings/ copied
cargo test -p nodes --no-fail-fast                            test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s (methods.json lists set_model_favorite)
git status --short                                            `M crates/rpc/bindings/Info.ts` — ts-rs writes there and the file IS tracked (56 files; cf. c57568a9 "regenerate Info.ts binding — default_effort (83)"); committed like 83 did. Root bindings/ and the UI copy are gitignored as the brief says.
```

`bindings/methods.json` lists `set_model_favorite`; `bindings/Info.ts` has `favorite_models: Array<string>`; `bindings/AgentEvent.ts` has `{ "type": "FavoriteModelsChanged", models: Array<string> }`.

## Hand-offs

- DESKTOP: `Info.favorite_models: Array<string>`, `AgentEvent` `{ type: "FavoriteModelsChanged", models: Array<string> }`, `set_model_favorite(model: string, favorite: boolean): Promise<string[]>` are in the regenerated `bindings/` (and the UI copy) exactly as the design freezes them. An uncatalogued `provider/model` is accepted (pass-through, like `set_model`); only an ambiguous bare id fails, as a JSON-RPC `ASK_FAILED` error with the `set_model` wording.
- `crates/slack` owner: the one-line ignore arm at `channel.rs:2540` is mechanical; nothing else in the crate changed.
- `crates/surface` owner: `crates/tui/tests/no_emoji.rs` fails on main — 21 lines in `surface/src/{elk_family,export,render}.rs` carry ▶ ⚠ ☹ ☺ (U+25B6, U+26A0, U+2639, U+263A). Not touched here (outside RUST ownership).

## Open items

- None in RUST scope.
