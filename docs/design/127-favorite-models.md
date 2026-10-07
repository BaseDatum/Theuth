# 127 — Favorite models

Status: building (2026-10-07 09:20). Builds on 124 (provider → model picker). Sky: "add the ability to favorite models too, favorites should be persistent across devices, so if I favorite in my desktop, that favorite is there on my phone too. … Favorites should just show up at the top outside of the nested list, one click access."

## 1. Where favorites live

Favorites are a property of the **engine**, not of a device: every device attached to an engine (desktop, phone over the relay, TUI) shows the same list and sees a change the moment it happens.

- Config: `agent.favorite_models` in `<data_dir>/config.yaml`, a YAML flow sequence of double-quoted canonical ids on one line, written with `runtime::write_config_value(path, "agent", "favorite_models", "[\"anthropic/claude-sonnet-4-5\", \"openai/gpt-5.4\"]")` so comments and the rest of the file survive byte-for-byte (same mechanism as `agent.model`). `AgentConfig.favorite_models: Vec<String>` (`#[serde(default)]`). Order = the order they were starred; no duplicates.
- Live state: `Agents.favorite_models: Vec<String>`, loaded from config at start.
- Messages (crates/agent): `GetFavoriteModels` → `Vec<String>`; `SetModelFavorite { model: String, favorite: bool }` → `Result<Vec<String>, String>` — `model` is resolved through the catalog like `SetModel` (aliases normalised, unknown ids rejected with the same error text), the list is updated (no-op when already in that state), persisted, and `AgentEvent::FavoriteModelsChanged { models: Vec<String> }` is published (global, `session()` = `None`, like `DefaultModelChanged`). The reply is the new list.
- Ids whose provider is currently unavailable stay on file; pickers show only favorites present in `list_models`, so a favorite reappears when its provider is keyed again.

## 2. RPC contract (crates/rpc)

- `Info.favorite_models: Vec<String>` with `#[serde(default)]` (older engines → empty).
- `#[method(name = "set_model_favorite")] async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>>` — returns the new list; `FavoriteModelsChanged` follows on the stream.
- Generated TS (ts-rs): `Info.favorite_models: Array<string>`; `AgentEvent` gains `{ "type": "FavoriteModelsChanged", models: Array<string> }`; `bindings/methods.json` gains `set_model_favorite` (crates/nodes' test checks it).

## 3. TUI (`crates/tui`)

- `Model.favorite_models: Vec<String>`: initial from `Info.favorite_models`, kept live by `AgentEvent::FavoriteModelsChanged` (also applied from the `set_model_favorite` reply).
- `/model` picker, providers level, rows in this order: **Favorites** (every favorite present in `items`, in favorites order; marker column `★`; `●` when it is the current model) → the **Current** row only when the current model is not a favorite → **Providers** (as in 124). The cursor starts on the first favorite when there is one, else where 124 put it.
- Models level and search results: the marker column becomes two characters — `●`/space for current, `★`/space for favorite — before the id.
- `Tab` on a Favorite / Current / Model row toggles that model's favorite: sends `Command::SetModelFavorite { model, favorite }` (`api.set_model_favorite`); the menu stays open and re-renders from the event. Title hints gain `⇥ ★ favorite` on every level. The `/model <id>` line and the ACP harness path are unchanged.

## 4. Desktop (`crates/desktop/ui`)

- `rpc/extra.ts` `Info.favorite_models?: string[]`; `rpc/api.ts` `set_model_favorite(model: string, favorite: boolean): Promise<string[]>` (+ the method-name list); `store/sessions.ts` dispatch: `FavoriteModelsChanged` → `conn.setInfo({ ...conn.info, favorite_models: e.models })` (same shape as `DefaultModelChanged`).
- `ModelPicker` gains `favorites?: string[]` and `onToggleFavorite?(id: string)`. Providers level, in order: **Favorites** group (cmdk items — so the highlight starts on the first favorite and Enter picks it: filled `Star`, name, provider at the right, context, check on the current one) → the Current lead row only when the current model is not a favorite → `none` row → Providers → Effort. Every model row (favorites group, models level, search results) carries a `Star` icon button at the far right (filled when favorite) that toggles without selecting (`stopPropagation`, `preventDefault`); always visible (phones have no hover). Keyboard: `Alt+Enter` toggles the favorite of the highlighted model row (read the highlighted value with `useCommandState`).
- `useModel()` exposes `favorites` (`info.favorite_models ?? []`) and `toggleFavorite(id)` → `api.set_model_favorite(id, !favorites.includes(id))` then `setInfo` with the returned list (the event makes every other device agree). `ModelMenu`/`ModelDialog`/`StartForm`/`ModelCombobox` pass favorites through, so forms and the meeting start form get the same Favorites group.
- Search results are not re-ranked by favorites (124 §2 stays pure); they are marked.

## 5. Gates

- Rust: `cargo fmt -p agent -p rpc -p tui`; `cargo clippy -p agent -p rpc -p tui --all-targets -- -D warnings`; `cargo test -p agent` (new: set/unset persists the flow sequence and dedups, unknown id rejected, event published, config round-trip), `cargo test -p rpc`, `cargo test -p tui` (new: favorites rows first, Tab toggles, current-not-duplicated, marker columns); `just ts-desktop` (BUILD_SLOTS=1) then `cargo test -p nodes` (methods.json).
- Desktop: `bun run check` 0 errors; scoped vitest (`src/lib src/components/chat src/components/meeting src/components/routines src/components/agents src/components/settings src/store`); full `bun run test`.

## 6. Out of scope

Per-device favorites, favorites ranking in search, favorites for ACP harness options, Slack/Telegram `/model`.
