# 162 as-built: UI (desktop)

Branch `feat/162-ui`, base `9ee516ad`; code commit `6a5fbb88`. Spec: `162-providers-unify.md` (D1, D2, D6/D7 UI use, D8 desktop slash, D9).

## Per file (`crates/desktop/ui/src/…`)
- `components/settings/Connections.tsx`
  - `Connections` (#L22-L30) renders `<Logins/> <Providers/> <Bedrock/>`; `BuiltIns` and `Gateways` are gone.
  - `Providers` (#L130-L289): one `Section title="Providers"` over `orderProviders(list_providers)`; actions `store` (set_secret, unchanged), `refresh` (`refresh_provider_models`, toast `refreshedText`), `add` (`add_provider`, unchanged), `remove` (`remove_provider`, unchanged). All four invalidate `providers` + `models` on success (`store` also `secrets`); errors toast verbatim through `useAction`.
- `components/settings/providers.ts`: `splitProviders` deleted; `orderProviders` (built-ins, then custom, engine order inside each) and `refreshedText` (the D7 toast) added; `BUILTIN_LABEL`, `builtinKeyNote` kept. Types now from `@/rpc/bindings/ProviderInfo` and `@/rpc/bindings/RefreshedModels`.
- `components/settings/providers.test.ts`: ordering; refresh text (change, removal only, no change); SSR render of `Connections` (row order, tags, key state vs base URL, Refresh models only on refreshable rows, Remove only on custom rows, form aria-label, empty line, no `/gateway/i` in the markup).
- `components/chat/slash.ts`: `/provider` menu hint `[add|refresh|rm <name>]`, description `providers (built-in and custom)` (#L151-L155); `refresh <name>` calls the RPC and toasts `refreshedText`, bare `refresh` opens Settings › Connections (#L604-L608); usage `usage: /provider [add [name [url [key]]] | refresh <name> | rm <name>]` (#L614).
- `components/chat/slash.test.ts`: `162 D8` test (counts toast, no-change toast, bare refresh navigates, engine error rethrown verbatim for the caller's toast, usage text).
- `components/costs/LimitsPanel.tsx` (#L160-L162): `authLabel('gateway')` → `custom`; `shownLabel` rewrites the engine label suffix ` · gateway` → ` · custom` for the card title and the one-line row. Display only; `auth`/row ids untouched.
- `rpc/api.ts`: `refresh_provider_models(name: string): Promise<RefreshedModels>` (#L967-L968) and `'refresh_provider_models'` in `METHODS` between `'redo'` and `'regenerate'` (alphabetical; `refresh` < `remove`). `ProviderInfo` now imported from `@/rpc/bindings/ProviderInfo`.
- `rpc/extra.ts`: hand-written `ProviderInfo` deleted (the file's own rule: delete once `bindings/` carries it; `export.rs#L44` exports it).

## Layout
- Row: status dot · `BUILTIN_LABEL[name] ?? name` · outline badge `built-in` | `custom`; description `` `name/…` `` · N models · `key set` | ``no key (secret `name`)`` (built-in) or the base URL in mono (custom).
- Row actions (right, one flex group): `Refresh models` (outline, `RefreshCw` icon spinning on the running row, all refresh buttons disabled while one runs) when `refreshable`; then `Set key…` / `Replace key…` (built-in, inline masked form as before) or the trash icon `Remove <name>` (custom).
- Below the rows: `No custom providers yet.` (muted `Empty`) when the list has no custom row; then the form, same grid as before: Name · `https://host/v1/` · API key · `Add custom provider` button.

## Strings
- Heading `Providers`; tags `built-in`, `custom`; button `Refresh models`; empty `No custom providers yet.`
- Form: `aria-label="Add custom provider"` (form), `Custom provider name`, `Custom provider base URL`, `Custom provider API key`; button `Add custom provider`.
- Toasts: `${name}: ${models} models (+${added}, −${removed})`, `${name}: ${models} models, no change`; add `Added ${name}: ${models} models` and set-key toasts unchanged; errors verbatim.

## Gates (vs baseline on the stale Oct 8 bindings plus the 162 stubs)
1. tsc `-b --force` (`/tmp/162u-1.log`): 823 `error TS` lines, baseline 823; identical after stripping `(line,col)` (4 pre-existing LimitsPanel errors moved 2 lines). 0 new, 0 fixed.
2. vitest (`/tmp/162u-2.log`): 1537 passed / 1 skipped, baseline 1533 / 1; the same 25 files fail to import in both (stale bindings). 0 new failures; +4 tests.
3. `grep -rni gateway crates/desktop/ui/src --include=*.tsx --include=*.ts` (`/tmp/162u-3.log`): remaining hits are the `gateway` data value mapping in LimitsPanel, the no-gateway assertions in `providers.test.ts`, and unrelated fixtures (`sandbox-queries.ts` demo DNS text, litellm window titles in two screen-context tests). No user-visible provider "gateway".

## Integration notes
- Hand-written, uncommitted stubs in `src/rpc/bindings/`: `ProviderInfo.ts` (adds `refreshable`), `RefreshedModels.ts` (new), and `refresh_provider_models` inserted in the local `methods.json`. Regenerate all three from ENG.
- `RefreshedModels` must be exported by ts-rs (contract has `ts(export)`), and `ProviderInfo.ts` must carry `refreshable`, or tsc fails on `providers.ts`/`api.ts`.

## Deviations
- "Existing confirm behaviour" for Remove: the old Gateways trash button had no confirm; kept as is (one click removes).
- Refresh invalidates the providers query on success only (`useAction`); a failed refresh changes nothing engine-side (D4).
- No live desktop smoke: the engine RPC does not exist in this worktree (no Rust builds); the UI was exercised by SSR render tests and the slash `run` path.
