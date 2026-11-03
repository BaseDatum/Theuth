# 162 as-built — ENG (engine, RPC, TUI, Signal, Telegram)

Branch `feat/162-eng`, base `9ee516ad`. Spec: `docs/design/162-providers-unify.md`. Anchors are `path#Lline` at the tip of this branch.

## Commits

| sha | what |
|---|---|
| `005ff3bd` | D5: live vault key for custom providers (`Auth::gateway`) |
| `b8e620b8` | D3/D4/D6/D7: `RefreshProviderModels`, the single `models.json` writer, `ProviderInfo.refreshable`, `models::merge` fix, wording |
| `bd518b35` | RPC `refresh_provider_models` + `RefreshedModels` export |
| `d80e32a9` | TUI / Signal / Telegram `/provider refresh <name>`, custom-provider wording |
| (this) | as-built |

## What changed, per file

### crates/models/src/lib.rs
- `Provider.id` / `Provider.name` are `#[serde(default, skip_serializing_if = "String::is_empty")]` (`#L21-L26`). A user block may hold only `models`.
- `merge` (`#L494-L514`): an empty user `name` or `env` keeps the bundled one; an empty `id` becomes the map key. Before this, a block with no `id`/`name` made the whole user file unreadable. A block written with `name: <id>` and `env: []` overwrote the bundled name and env.
- Test `models_only_user_block_keeps_bundled_provider_metadata` (`#L668-L699`): a `{"opencode":{"models":{…}}}` user block keeps the bundled name, env, model count +1, and the bundled models' `protocol` and limits.

### crates/agent/src/providers.rs
- Module and `Gateway` docs say "custom provider" (`#L1-L22`); the type keeps its name (D10).
- `Listing::{Custom, Builtin(&Catalog)}` (`#L152-L160`) and `write_models(data_dir, name, ids, listing) -> (added, removed)` (`#L162-L220`). This is the one writer for add and refresh:
  - an empty listing returns `Err("<name> listed no models")` before anything is read or written;
  - an unreadable `models.json` returns an error and is never overwritten (the old code fell back to `unwrap_or_default` and would have rewritten the file with only this provider);
  - custom: `retain` keeps listed ids unchanged (hand edits included) and the dropped ids become `removed`; new ids get `listed_model` (context 0);
  - built-in: an id is added only when neither the user block nor `catalog.get("<name>/<id>")` has it. A new built-in block is written with no `name` or `env`, so the bundled metadata survives `merge`. Nothing is removed;
  - when nothing was added or removed, nothing is written, so the file stays byte-identical. Otherwise the file is written through `atomic_write`.
- `save` (`#L258-L284`) no longer takes models; it writes the vault key and the yaml row. The vault note for new adds is `custom provider <name> key`.
- Tests: `save_roundtrip_and_remove` (re-add keeps hand edits, file byte-identical) `#L362`, `custom_listing_keeps_adds_and_removes` `#L420`, `builtin_listing_only_adds_unknown_ids` `#L450`, `empty_listing_or_broken_file_leaves_models_json_alone` `#L501`.

### crates/agent/src/lib.rs
- `ProviderInfo.refreshable` with `#[serde(default)]` (`#L2163-L2165`). Docs on `ListProviders`, `ProviderInfo`, `AddProvider` and `RemoveProvider` now say custom provider.
- `RefreshProviderModels { name }` (`#L2186-L2191`) and `RefreshedModels { provider, added, removed }` with ts export (`#L2193-L2203`), as in the contract.
- `RefreshedModels::line()` (`#L2205-L2222`) gives the D7 text `rwx: 14 models (+2, -1)` or `rwx: 14 models, no change`. The TUI, Signal and Telegram all use it.

### crates/agent/src/supervisor.rs
- `OPENROUTER_ENDPOINT` (`#L8533`). `Agents::provider_info(name)` (`#L8538-L8566`) builds one row for ListProviders, AddProvider and Refresh. `refreshable` = `configured`, which is always true for a custom provider and true for opencode/openrouter only when they have a key (D6).
- `Agents::refresh_provider_models` (`#L8568-L8621`), D3:
  - `opencode` → `AdapterKind::OpenAI` at `ZEN_ENDPOINT` with `opencode_key()`;
  - `openrouter` → `AdapterKind::OpenRouter` at `https://openrouter.ai/api/v1/` with `openrouter_key()`;
  - custom → `AdapterKind::OpenAI` at its base URL with the key from `Auth::gateway` (live, D5);
  - a catalog provider id, OAuth namespace or genai adapter name → `<name>'s models come from the built-in catalog`; anything else → `no provider <name>`;
  - no key or an empty key → `<name> has no key; set it first`;
  - a listing failure → `listing <name>'s models: <genai error>`, with no change;
  - then `write_models` (custom / built-in), `Catalog::load`, `costs.set_catalog`, and the reply `RefreshedModels`.
- ListProviders (`#L8623-L8647`): built-ins first, then custom providers sorted by name, all through `provider_info`.
- AddProvider (`#L8649-L8702`): validate → list → `save` → map insert → `write_models(Custom)`. A failed or empty listing is logged with `keeping the existing list`, and the provider is still saved. The reply comes from `provider_info`, so `models` is the catalog count after the reload.
- `impl Message<RefreshProviderModels>` (`#L8704-L8714`).
- `Auth::gateway` (`#L9185-L9195`), D5: overlays `Auth::key(ns)` (the live vault value) on the cached `Gateway`. The cached key is used only when the vault has no non-empty value. Both 160 seams are unchanged: `backend::target_for` still gets a gateway, so `auth: "gateway"` attribution stays, and `limits::presence` still iterates `auth.gateways` keys.
- The unconfigured-provider error now reads `… add a key, log in, or add a custom provider first` (`#L4376`).
- Tests: `refresh_provider_models_lists_with_the_live_key` (`#L15714-L15800`) and `refreshed_models_line` (`#L15802-L15821`). The first runs against a wiremock `/v1/models` that matches only `Bearer sk-new`, with the vault holding `sk-new` and the cached key `sk-old`. It checks the custom add, a no-change refresh, the catalog reload, a refused key (error, `models.json` byte-identical), the built-in catalog / unknown / no-key errors, and openrouter `refreshable` once its vault key is set.

### crates/rpc
- `api.rs#L1418-L1434`: provider doc strings say custom provider. `refresh_provider_models(name) -> RefreshedModels`, placed after `remove_provider` as the contract says. The impl is at `#L6461-L6463`.
- `export.rs#L45`: exports `agent::RefreshedModels`. `methods.json` is generated from the module, so it picks up the new method automatically.

### crates/tui
- `model.rs`:
  - `Command::RefreshProvider(String)` (`#L713`);
  - `/provider` help (`#L1842-L1846`);
  - `provider_cmd` `refresh <name>` and the usage string `usage: /provider [add [name [url [key]]] | refresh <name> | rm <name>]` (`#L8511-L8568`);
  - `provider_add_wizard_then_one_shot` also checks `/provider refresh rwx` → `RefreshProvider("rwx")` and that a bare `refresh` gives the usage error (`#L15021-L15030`).
- `lib.rs#L968-L970`: `RefreshProvider` → `refresh_provider_models` → `UiEvent::Info(r.line())`. Errors use the existing error path.

### crates/signal, crates/telegram (`commands.rs`)
- Help row: `[add [name [url [key]]] | refresh <name> | rm <name>]`, "built-in and custom providers".
- `provider_cmd`:
  - the dead `rows.is_empty()` "no custom providers" branch is gone;
  - the list gains a `built-in` / `custom` column;
  - `refresh <name>` asks `RefreshProviderModels` and replies `r.line()`;
  - the usage string is the D8 one.
- Test `provider_refresh_needs_a_name` in each crate checks that `refresh`, `rm` and an unknown subcommand return the usage string without calling the engine.

## Gates (`BUILD_LABEL=162E`)

| # | command | exit | result |
|---|---|---|---|
| 1 | `build-lock mbx check -p agent -p models -p rpc -p tui -p signal -p telegram --all-targets` (`/tmp/162e-1.log`) | 0 | Finished. Warnings: only the known ones (`thinking_tests.rs:52` `fetch_update`, plus `link` / `mcp-server` deprecations in dependencies) |
| 2 | `build-lock mbx clippy … --all-targets --no-deps -- -D warnings` (`/tmp/162e-2.log`) | 101 | 1 error: the known `crates/agent/src/loop_/thinking_tests.rs:52` `fetch_update`. A `--keep-going` run (`/tmp/162e-2k.log`) linted telegram, signal, rpc and tui with no other error |
| 3a | `mbx test -p agent --lib --no-fail-fast -- providers backend:: limits:: refresh` | 0 | 47 passed, 0 failed, 1 ignored (live Anthropic) |
| 3b | `mbx test -p models` | 0 | 26 + 1 passed, 0 failed |
| 3c | `mbx test -p rpc --lib --no-fail-fast` | 0 | 59 passed, 0 failed |
| 3d | `mbx test -p tui --lib --no-fail-fast -- provider` | 0 | 5 passed, 0 failed |
| 3e | `mbx test -p signal --lib --no-fail-fast -- provider` | 0 | 1 passed, 0 failed |
| 3f | `mbx test -p telegram --lib --no-fail-fast -- provider` | 0 | 1 passed, 0 failed |
| 4 | `BUILD_SLOTS=1 … mbx test -p rpc --features ts --lib --quiet export_bindings` (`/tmp/162e-4.log`) | 0 | 8 passed. Wrote `bindings/` (gitignored, 940 files) and rewrote `crates/rpc/bindings/` (tracked, byte-identical). `bindings/RefreshedModels.ts` exists, `bindings/ProviderInfo.ts` has `refreshable: boolean`, `bindings/methods.json` lists `refresh_provider_models` |
| 5 | `cargo fmt -p agent -p models -p rpc -p tui -p signal -p telegram` | 0 | no diff |

Every log was grepped for `test result: FAILED`, `panicked` and `error[`; none were found.

## Deviations from the design, with reasons

1. **D5 fallback.** `Vault::get` returns `Option` and cannot tell a failed read from a missing entry. The cached key is therefore used whenever the vault has no non-empty value, not only when the read fails. Consequence: a custom key *deleted* in Secrets keeps working from the cached copy until restart. A *replaced* key applies at once, which is what D5 asks for.
2. **D4 merge fix.** `models::Provider` has no provider-level `npm`, `protocol` or `api` fields (protocol is per model), so the bundled metadata at risk was `id`/`name`/`env`. The fix: optional `id`/`name` in user blocks, and `merge` keeps the bundled `name`/`env` when the user block leaves them empty. A user block that sets them still wins.
3. **Extra safety in the writer.** An unreadable `models.json` is now an error rather than being silently replaced. A no-change listing writes nothing.
4. **Error texts not fixed by D3:** `listing <name>'s models: <error>` and `<name> listed no models`. Built-in-catalog vs unknown is decided by catalog provider id, OAuth namespace or genai adapter name.
5. **`RefreshedModels::line()`** was added in `agent` so the three text frontends print the same D7 line. The contract's field shape is unchanged. Field doc comments were added because the workspace's `missing_docs` would fail clippy; ts-rs emits them as JSDoc, and the TS type shape matches the contract.
6. **AddProvider reply `models`** is the catalog count after the reload, not the listing length. The two are equal for a fresh add; on a re-add whose listing failed, it shows the kept list.
7. **Signal/Telegram** provider lists gained a kind column (`built-in` / `custom`).
8. **Commit order:** D5 is committed before the writer and refresh commit, because the refresh test exercises the rotated key.

## Open items (outside this worker's ownership)

- `crates/agent/src/limits/mod.rs#L673` still renders a custom provider's limits row *label* as `"<name> · gateway"`. The brief scopes limits to "only if a test needs it". D1 covers the desktop costs panel through the UI mapping of the `auth` label, but this `label` string is a separate user-visible field.
- Existing vault entries keep their old note `gateway <name> key`; only new adds write the new note.
- `crates/agent/bindings/ProviderInfo.ts` (stale per recon) is not regenerated: the gate writes `/bindings` and `crates/rpc/bindings`.
- [INFERENCE] The Zen listing `GET https://opencode.ai/zen/v1/models` returning OpenAI `data[].id` was not verified live; the opencode/openrouter refresh paths are covered by the writer tests, not by a live call.
