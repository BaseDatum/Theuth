# 124 as-built — DESKTOP (model pickers: provider → model navigation + word search)

Branch `feat/124-picker-desktop`, cut from main f016637e. All paths under `crates/desktop/ui/src/`.

## What was built

- `lib/model-search.ts` (new, 80 lines): `searchTerms` (7–9), `boundaryIndex` (12–17), `termTier` (20–36), `rankModels` (42–59), `providersOf` (62–68), `modelsOfProvider` (71–79). Pure, no React.
- `lib/model-search.test.ts` (new, 83 lines): the §2.3 fixture verbatim (19–30) and every table row as one test (32–55), the quoted tiers (57–68), `providersOf`/`modelsOfProvider` (70–83).
- `components/chat/ModelPicker.tsx` (new, 195 lines): `Row`/`View` types (13–27), `withExtra` (30–34, `extra` ids become rows under the `other` provider), `viewFor` (37–62, the pure level logic: providers / models / search / §3.5 shortcut / Current-row dedup / none row), `contextLabel` (65–67), `ModelPicker` (83–195: `Command shouldFilter={false}`, controlled `CommandInput`, Backspace-on-empty goes back reading `e.currentTarget.value`, Effort group on the providers level only, `No model matches.` as a disabled row).
- `components/chat/ModelPicker.test.ts` (new, 88 lines): providers level, models level, search mode, §3.5, forms (`none`/`extra`), `contextLabel`.
- `components/chat/ModelCombobox.tsx` (new, 60 lines): outline `Button` trigger (name → raw id → `none` label) + `Popover` + `ModelPicker` with `none={{ value: '__none__', … }}` mapped to `onChange(null)`.
- `components/chat/ModelMenu.tsx`: removed `modelItemValue`, `orderModels`, `ModelItems`, `CommandEmpty`/`ModelInfo`-only imports; `pinnedValue` (215–220) is harness-only; `ModelCommand` (235–246) renders `ModelPicker` with the effort group; `ModelMenu` popover (284–292) wraps only the harness body in `<Command filter=…>`; `ModelDialog` (312–334) keeps `CommandDialog` for the harness and uses `Dialog`/`DialogContent` + `ModelPicker` for models (the picker owns its `Command`, see Decisions). `HarnessGroup`/`HarnessCommand`, `pinFirst`, `pinnedFilter`, `harnessItemValue`, `useOpenOnSlash` unchanged.
- `components/chat/ModelMenu.test.ts`: dropped the `orderModels` and `modelItemValue` tests with the code; harness tests and `pinnedFilter` kept.
- `components/meeting/StartForm.tsx` (12, 112–119): popover body is `<ModelPicker models model onSelect>`; `Command*`/`ModelItems`/`pinnedFilter` imports gone.
- `components/routines/RoutineDetail.tsx` (8, 651): Model = `ModelCombobox` (none "Engine default"); `Select` import and the `NONE` sentinel removed (no other use in the file).
- `components/agents/RuntimeAccessFields.tsx` (3, 42): Model = `ModelCombobox` (none "Node default"); effort/tools `Select`s untouched.
- `components/settings/Loop.tsx` (2, 194–202, 208–216, 228–236, 250–258): the four model `Select`s (title, subagent, vision — on `visionModels` —, escalation) are `ModelCombobox`es with none "Off" (escalation keeps its `Off (inherits title model …)` label); the compact-style `Select` stays. Stored values unchanged: `onChange(null)` → `mutate('off')`, ids pass through.
- `components/ui/command.tsx`: untouched.

## Decisions taken beyond the design

1. **Highlight is not a controlled `Command value`.** cmdk 1.1.1 re-selects the first DOM item in a layout effect of its own after every search change (`setState('search')` → `selectFirstItem`) and whenever the highlighted item unmounts; with a controlled value that fires `onValueChange` *after* the parent's effects, so a parent-set highlight gets overwritten (verified against `node_modules/cmdk/dist/index.mjs`). Instead the §3 start row is literally the first DOM item: the Current / `none` / `← All providers` rows are rendered last inside `[cmdk-list-sizer]` (made `flex flex-col`) with `order-first`, so they display on top while cmdk's own "first item" rule lands on the first provider / first model / first result. Cost: arrow keys follow DOM order, so ↓ past the last row reaches the Current row and ↑ from the first provider does not. `viewFor` returns rows in display order.
2. `ModelPicker` owns its `<Command>` (not just the body) because `CommandDialog` wraps children in its own `Command` and the picker needs `shouldFilter={false}`; `ModelDialog`'s model branch therefore uses `Dialog`/`DialogContent` directly (sr-only title/description kept, input height matched with `[&_[cmdk-input]]:h-12`).
3. `ModelCombobox` has no `extra` prop: it lists `value` under `other` itself whenever `value` is not in `models`. That covers RoutineDetail/RuntimeAccessFields' unknown on-file id and Loop's `unavailable` vision id with no caller code. The Loop description still names the unavailable id; the trigger shows the raw id (no `(unavailable)` suffix).
4. The `none` row carries a check mark when `model` is unset (forms show "Engine default ✓").
5. Models-level rows show `200k · vision` (context rounded to thousands, omitted when 0).
6. Extra ids take part in search ranking (they are catalog rows with `name = id`, `provider = 'other'`).
7. `rankModels(models, '  ')` returns `models` unchanged (browse mode is the caller's branch via `searchTerms`).

## Tests

- `lib/model-search.test.ts`: `124 §2.3 vectors: every row of the table, ids in order`, `124 §2.3 scores: the tiers the table quotes`, `providersOf counts A→Z; modelsOfProvider sorts by name then id`.
- `components/chat/ModelPicker.test.ts`: `providers level: Current row, then providers A→Z with counts`, `models level: back row, the provider’s models by name, the current one checked`, `search mode: Current row first even when it does not match, never duplicated`, `§3.5 single provider: opens on its models with no back row`, `forms: the none row sits under Current and is checked when nothing is set; extra ids live under \`other\``, `contextLabel: thousands, empty when unknown`.
- `components/chat/ModelMenu.test.ts`: harness tests + `pinnedFilter` kept; `orderModels` and `modelItemValue` tests removed with their code (behaviour replaced by design).
- Smoke: `ModelPicker` server-rendered once with `react-dom/server` (providers level with Current, `none`, Effort) — markup as expected; no jsdom/browser run (no engine started).

Gate (from `crates/desktop/ui`, each through `build-lock`, `BUILD_LABEL=124-DESKTOP`):

```
$ build-lock bun run check            → tsc -b --noEmit: 0 errors (exit 0)
$ build-lock bunx vitest run src/lib src/components/chat src/components/meeting src/components/routines src/components/agents src/components/settings
   Test Files  96 passed (96)
        Tests  602 passed (602)
$ build-lock bun run test
   Test Files  149 passed (149)
        Tests  861 passed (861)
     Duration  73.06s
```

## Hand-offs

- None for TUI. No RPC/bindings change.

## Open items

- Keyboard order quirk from Decision 1 (↓ past the last row wraps visually to the Current row). A controlled highlight would need cmdk to stop auto-selecting on search change (upstream `shouldFilter={false}` still calls `selectFirstItem`).
- No rendering test for `ModelPicker`/`ModelCombobox` (no jsdom harness in this tree).
