# 102 as built — BROADCAST-DESKS

Branch `feat/102-broadcast-desks`, base `5dcfee1c`. Brief: `/tmp/mc-2026-10-05/omp-102-BROADCAST-DESKS.md`. Sky's ask: "the broadcast UI needs a section at the top of the list of sessions dedicated to desks, so I can just select all the desks and broadcast just to them."

## What was built

All in `crates/desktop/ui/src/components/`.

| Decision | Where |
|---|---|
| D1 `deskRoots(rows, sessions)` — roots whose `SessionSummary.desk` is non-empty, `rows` order, theuth first | `chat/broadcast.ts:41–47`; `deskOf` helper `:34` (the id → roster-name join, also used by rows and sections); `theuthFirst` comparator `:38` shared with D2 |
| D2 Two sections, tri-state header | `splitGroups` `chat/broadcast.ts:50–56`, `sectionState` `:59–62`; `Section` component `chat/BroadcastDialog.tsx:249–317` — header `<label>` is `min-h-11`, checkbox `checked` on `all`, `el.indeterminate` via ref `:285`, `n of m` in `text-fg-tertiary tabular-nums`; `flip` `:269` adds the section's roots unless all are ticked, then removes them, never touching the other section. Rendered `:192–193`: Desks (omitted when empty) then Sessions. Each section owns a `<ul aria-label="Desks"\|"Sessions">`; the old single list is gone |
| D3 Chips `All desks · Running roots · All roots · None` | `chat/BroadcastDialog.tsx:184–187`; `All desks` replaces the selection with exactly `deskIds`, hidden when there are no desks |
| D4 Desk rows: roster name as label + `desk` tag | `Row` `chat/BroadcastDialog.tsx:337–345` — `deskOf` only for `isRoot(row)`; tag reuses the state-pill chip classes with `text-[10px] uppercase tracking-wide`. Children via `Branch` unchanged |
| D5 Preset `'desks'` + sidebar button | `Request.preset` `chat/BroadcastDialog.tsx:39`; `initial()` `:113–120` resolves untouched selection to `deskRoots`, falling back to `runningRoots` when the session list has loaded and holds no desk; `sidebar/AppSidebar.tsx:67–79` ghost `icon-sm` `Megaphone` button, `aria-label`/`title` "Broadcast to desks", `ml-auto` inside the existing `SidebarGroupLabel` |
| D6 Phones | Same `Form` inside the bottom sheet; header rows `min-h-11`; nothing else changed |
| D7 Footer | Untouched |

## Tests (`chat/BroadcastDialog.test.ts`)

- `:146` `deskRoots` picks only roots with a desk, theuth first; a desk child (`a1` with a stray `desk`) and an empty `desk: ''` are not desk roots; `[]`/`undefined` sessions → `[]`.
- `:152` `splitGroups` partitions by the root's desk (`[D, B]` with B's child kept; rest `[A, C]`).
- `:161` `sectionState` all / some / none / empty.
- The 6 pre-existing tests are unchanged and green.

## Gates (run 2026-10-06, worktree `~/Develop/theuth-wt-102`)

```
export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$HOME/.bun/bin:/opt/homebrew/bin:$PATH
cd ~/Develop/theuth-wt-102 && just ts-desktop
  → test result: ok. 8 passed; 0 failed … cp -R bindings/. crates/desktop/ui/src/rpc/bindings/   exit 0
cd crates/desktop/ui && bun install && (cd ../../../packages/apps && bun install)
  → 619 packages installed / 484 packages installed                                          exit 0
bun run check 2>&1 | tail -3
  → $ tsc -b --noEmit                                                                        exit 0
CI=1 bun run test 2>&1 | tail -8
  → Test Files  141 passed (141)   Tests  797 passed (797)                                   exit 0
bunx vite build 2>&1 | tail -2
  → built; `git status --short` shows only the four owned files; routeTree.gen.ts unchanged  exit 0
```

## Deviations / notes

- `deskOf` is an extra export beyond the three named helpers: the row label (D4), the section split (D2) and the preset (D5) all need the same id → `desk` join, so it lives once in `broadcast.ts`.
- The desks preset waits for `list_sessions` before falling back to `runningRoots` (`initial()` returns the empty desk set while `sessions` is still `undefined`), so a slow session list does not flash a running-roots preview before the desks tick. Once the list is in and holds no desk, the fallback is `runningRoots` as D5 says.
- Section header checkbox carries `aria-label="All desks"` / `"All sessions"` (not in the brief; the visible label is a sibling span inside the same `<label>`, the aria-label keeps the control named on its own).
- No DOM test runtime in this package (no jsdom/happy-dom) and the UI is Tauri-bound, so the rendered dialog was not exercised in a browser here; coverage is the pure helpers plus tsc and the vite build.
