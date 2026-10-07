# 127 as-built — DESKTOP (favorite models in every picker + phone dialog placement)

Branch `feat/127-fav-desktop`, cut from main f093e3b8. All paths under `crates/desktop/ui/src/`. Two workers wrote this (the second resumed from f0c13b5e; the tree was already complete, the gate had not been run).

## What was built

- `rpc/extra.ts` (26–27): `Info.favorite_models?: string[]` (optional: absent on an older engine; every reader defaults to `[]`).
- `rpc/api.ts` (752–753, 1108): `set_model_favorite(model, favorite): Promise<string[]>` + its `METHODS` entry (sorted slot, after `set_model`).
- `store/sessions.ts` (702–705): `FavoriteModelsChanged` → `conn.setInfo({ ...conn.info, favorite_models: e.models })` on the publishing engine's store only, next to `DefaultModelChanged`/`DefaultEffortChanged`.
- `store/favorite-models.test.ts` (new, 24 lines): modelled on `default-model.test.ts`.
- `components/chat/ModelPicker.tsx`: `Row` gains `favorite` (20–21) and `model.favorite` (23); `viewFor` takes `favorites` (42–74: favorite rows first in starred order, catalog-only, then the Current lead row ONLY when the current model is not among them, then `none`, then providers; models level and search rows carry `favorite` but are never re-ranked); `favoriteTarget` (78–82, the `Alt+Enter` target from a cmdk value); props `favorites` / `onToggleFavorite` / `listClassName` (100–106); `SearchInput` (110–134) is an inner component so `useCommandState((s) => s.value)` can read the highlighted row — `Alt+Enter` toggles a `model:`/`fav:` row and `stopPropagation`s so cmdk's root Enter does not also select; `star()` (177–191) is an always-visible `<button>` (filled `Star` when favorite, `aria-label` Add/Remove from favorites) whose `onClick` does `preventDefault` + `stopPropagation` + toggle; favorite rows (203–210: name, `provider · 200k` at the right, check when current, filled star); model rows get the star (220); the Favorites `CommandGroup` is rendered ABOVE the lead-row `div` (241–242) so it holds the first cmdk items (highlight starts there, Enter picks one); `CommandList` class is `cn('max-h-96', listClassName)` (240).
- `components/chat/ModelMenu.tsx`: `dialogPlacement(touch)` (58–71, §6 classes); `useFavorites()` (73–88: `info?.favorite_models ?? []` + `toggleFavorite` = `api.set_model_favorite(id, !includes)` then `setInfo` with the reply, errors → `toast.error`); `Model` carries `favorites`/`toggleFavorite` (106–108, 194–197); `HarnessCommand`/`ModelCommand` take `listClassName` (231–236, 274–284); `ModelDialog` (358–376) applies `dialogPlacement(touch())` to both branches' `DialogContent`/`CommandDialog` and their lists.
- `components/chat/ModelCombobox.tsx` (10, 39, 54–56) and `components/meeting/StartForm.tsx` (12, 48, 117–119): `useFavorites()` + the band-aware popover list `max-h-[min(24rem,calc(var(--visible-band)-6rem))]`.
- Gitignored stubs (NOT committed): `rpc/bindings/AgentEvent.ts` has `{ "type": "FavoriteModelsChanged", models: Array<string>, }`; `rpc/bindings/Info.ts` has `favorite_models: Array<string>,` after `default_effort`.
- `components/ui/dialog.tsx` / `command.tsx`: untouched (`className` already reaches `DialogContent`; `cn` is tailwind-merge compatible, so `top-[max(…)]`/`translate-y-0` override the centred defaults).

## Decisions taken beyond the design

1. **Favorites + toggle live in one hook, `useFavorites()` in `ModelMenu.tsx`**, read by `useModel()`, `ModelCombobox` and `StartForm` (the brief's "read `useConnection` inside the combobox or accept props" → the combobox reads the store via the hook; its callers — RoutineDetail, RuntimeAccessFields, Loop — are unchanged). It reads `useConnection.getState()` at reply time so a stale closure cannot clobber a `FavoriteModelsChanged` that arrived first.
2. The favorite row's right-hand text is `provider · 200k` (provider first, as the brief asks; context omitted when 0), the models-level row keeps 124's `200k · vision`.
3. `Alt+Enter` is handled on the input's `onKeyDown` with `stopPropagation` (cmdk 1.1.1 selects on any Enter in its root `onKeyDown`, no modifier check). The star's click never reaches cmdk's item `onClick` (React propagation stopped at the button); no pointer-down ref guard was needed.
4. `dialogPlacement` is a pure function of `touch()` so the §6 class choice is unit-tested; the harness `CommandDialog` branch gets the same placement (same sheet, same keyboard). On touch the list class is `max-h-none min-h-0 flex-1`, i.e. the `max-h-96` is dropped as asked.
5. `favorite_models` stays optional in `extra.ts` (older engine) even though the generated `Info` makes it required.

## Tests

- `components/chat/ModelPicker.test.ts`: `127 favorites: first, in starred order, catalog-only; the Current row only when it is not a favorite` (favorites order, current-in-favorites shows once, current-not-in-favorites shows Current after favorites, missing-from-catalog skipped, models level + search rows carry `favorite`), `favoriteTarget: Alt+Enter toggles model and favorite rows only`.
- `components/chat/ModelMenu.test.ts`: `127 §6 dialogPlacement: touch anchors the dialog to the top of the visible band and lets the list fill it; else centred`; the 124 harness tests kept green.
- `store/favorite-models.test.ts`: `FavoriteModelsChanged replaces the publishing engine Info list only` (other engine untouched, empty list kept, `info === null` → no-op).
- No pre-existing test changed by design. `rpc/api.test.ts > METHODS is sorted and unique` failed on the first gate because `set_model_favorite` had been inserted out of order; fixed by moving the entry (api.ts 1108).

### Gate (from `crates/desktop/ui`, `BUILD_LABEL=127-DESKTOP`, each through `build-lock`)

```
build-lock bun run check
```
97 `error TS` lines, 0 in this wave's lines. All stem from the gitignored bindings copy predating main: `Cannot find module '@/rpc/bindings/PagePreviewAnswer' | PreviewId | Client | PagePreviewRequested | PagePreviewResolved` (api.ts 87–88, events.ts 19–20, lib/page-preview.ts) and the implicit-`any` / `Loose<…>` cascade from them in `store/reduce.ts`, `store/reduce.test.ts`, `store/sessions.ts:687` (`PendingPrompts.secrets`), `lib/graph-layout.ts`, `lib/speech.ts:483`. Not fixed (not owned; real bindings arrive with `just ts-desktop`).

```
build-lock bunx vitest run src/lib src/components/chat src/components/meeting src/components/routines src/components/agents src/components/settings src/store
 Test Files  1 failed | 126 passed (127)
      Tests  7 failed | 989 passed (996)
```
The 7 failures are `components/chat/surface/catalog.test.ts` (parity with the gitignored stale `bindings/surface-catalog.json`: 38 vs 57 components) — environmental, untouched by this wave.

```
build-lock bun run test
 Test Files  2 failed | 172 passed (174)
      Tests  8 failed | 1179 passed (1187)
```
(final run, after the METHODS reorder; see the line below for the exact summary). Failures: the same 7 catalog tests + `rpc/api.test.ts > METHODS equals the server method list`, whose diff is exactly `+ "answer_page_preview"` (pre-existing: the stale `methods.json` predates it) and `+ "set_model_favorite"` (expected until RUST regenerates the bindings). `src/link/pairing.test.ts` passed in both full runs (no flake rerun needed).

## Hand-offs

- RUST: `just ts-desktop` must emit `Info.favorite_models: Array<string>`, the `FavoriteModelsChanged { models }` event variant, and `set_model_favorite` in `methods.json`; the `METHODS equals the server method list` test then passes with no desktop change.
- Build-lock note: the second worker's commands were backgrounded by its harness (not by choice) and collected once each; no build was killed or bypassed.

## Open items

- No rendering test for the star button / `Alt+Enter` / phone placement (no jsdom harness in this tree, same as 124); the pure parts (`viewFor`, `favoriteTarget`, `dialogPlacement`) are tested.
- Phone placement is reasoned, not browser-run: Radix `DialogContent` is `position: fixed`; `lib/viewport.ts` undoes page scroll while the keyboard is up, so `top-[max(0.5rem,env(safe-area-inset-top))]` sits at the top of the visible band and `max-h-[calc(var(--visible-band)-1rem)]` keeps the list above the keyboard. Worth one look on a device at integration.
