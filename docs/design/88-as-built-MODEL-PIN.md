# 88 as-built — MODEL-PIN: the current model is always the first row of the cmdk model picker and no search hides it

Branch `feat/88-model-pin` (worktree `theuth-wt-88b`, off main `fca080be`). UI only; no Rust. Brief: `/tmp/mc-2026-10-03/omp-MODEL-PIN.md`.

## What changed

`crates/desktop/ui/src/components/chat/ModelMenu.tsx`
- `:47` `modelItemValue(m)` → `${m.name} ${m.id}` — the cmdk value of a model row; `ModelItems` and `pinnedFilter` share it.
- `:52` `harnessItemValue(heading, v)` → `${heading} ${v.name} ${v.value}` — the cmdk value of a harness option row.
- `:57` `pinFirst(items, is)` — the matching item first, the rest in order, no duplicate; the input array unchanged when nothing matches or the match is already first.
- `:63` `orderModels(models, current)` — `pinFirst` on `id === current`; `current === undefined` → unchanged.
- `:70` `pinnedFilter(pinned)` — a `CommandFilter`: the row whose trimmed, lower-cased value equals `pinned` scores 1, every other row goes through cmdk's `defaultFilter`.
- `:76-88` `ModelItems` renders `orderModels(models, model)`; the current row gets `forceMount`. Row look unchanged (name, provider, `Check`).
- `:209-222` `HarnessGroup` takes `pin?: boolean`; with it the option whose `value === option.current` renders first with `forceMount`. `HarnessCommand` `:232` passes `pin` to the **Model** group only; Effort untouched.
- `:248` `pinnedValue(m)` — ACP child: `harnessItemValue('Model', currentOption)` via `harnessGroups(m.harness ?? []).model`; else `modelItemValue(m.current)`; `undefined` when there is no current.
- `:338` `ModelMenu` popover root and `:364` `ModelDialog` get `filter={pinnedFilter(pinnedValue(m))}`.

`crates/desktop/ui/src/components/ui/command.tsx`
- `:17` `type CommandFilter = NonNullable<React.ComponentProps<typeof CommandPrimitive>['filter']>` (cmdk does not export its `CommandFilter`), exported at `:181`.
- `:42,:50,:62` `CommandDialog` accepts and forwards `filter` next to `shouldFilter`.

`crates/desktop/ui/src/components/meeting/StartForm.tsx`
- `:12` imports `modelItemValue`, `pinnedFilter`; `:47` hoists `current = models.data?.find((m) => m.id === model)` (the button label at `:108` reuses it); `:113` `<Command filter={pinnedFilter(current && modelItemValue(current))}>`.

`crates/desktop/ui/src/components/chat/ModelMenu.test.ts`
- `orderModels`: current first, current absent/unset/unknown → the same array, no duplicate, empty list.
- `pinnedFilter`: pinned value with a non-matching search → 1, also with other case / surrounding whitespace; another value non-matching → 0; another value matching → `> 0` and `=== defaultFilter(...)`; `pinnedFilter(undefined)` ≡ `defaultFilter` on three inputs.
- `modelItemValue` / `harnessItemValue`: the exact strings.

## cmdk 1.1.1 mechanics relied on

`Command` takes `filter: (value, search, keywords?) => number`; with a search present every item is scored, items scoring 0 are hidden, items inside a group are re-ordered by score descending with a stable sort, groups by their best item's score. The pinned row is rendered first and scores 1 (the maximum), so it stays first and visible; with an empty search nothing is re-ordered and render order (`orderModels`/`pinFirst`) wins. `forceMount` on the current `CommandItem` keeps it rendered even where a `Command` root lacks the filter. When the search matches nothing else the list shows only the pinned row and `CommandEmpty` does not appear — intended.

## Gates

Bindings: `just ts-desktop` (toolchain 1.98.1) →
```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 4.81s
EXIT=0
```

`cd crates/desktop/ui && bun install && bun run check` →
```
$ tsc -b --noEmit
CHECK_EXIT=0
```

`CI=1 bun run test < /dev/null` →
```
 Test Files  130 passed (130)
      Tests  722 passed (722)
   Duration  20.08s (import 62%, transform 33%, tests 5%)
TEST_EXIT=0
```

## Deviations / notes

- `@openuidev/*` was unresolved on the first `bun run check`; `bun install` in `packages/apps` fixed it as the brief anticipated.
- The first full `bun run test` run failed one untouched file, `src/link/pairing.test.ts` (`session: IK over the relay … large records split/reassemble`), a timing flake under full-suite load: it passed alone and the full rerun above is green with no code change in between.
- `orderOptions` from the brief step 4 is the shared generic `pinFirst` (used by both `orderModels` and `HarnessGroup`) rather than a second copy.

## Commits

- `8915c77c` feat(desktop): pin the current model first in every cmdk model picker
- `8ef78f25` test(desktop): orderModels, pinnedFilter and item value helpers
- fix(desktop): pinFirst without an unchecked index (tsc noUncheckedIndexedAccess)
- docs(design): 88 as-built MODEL-PIN
