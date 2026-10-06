# 113 – Library › Tools: rows stack on phones; names wrap at underscores

Status: as-built · 2026-10-06 · amends 37 §4 (the `/tools` page). Branch `fix/tools-mobile-layout` in `~/Develop/theuth-tools-mobile`, base `3b891598` (main). `components/library/Tools.tsx` + `names.ts` only; no Rust, no `Transcript.tsx`/`styles.css`.

Commits (oldest first):
- `532b7952` fix(desktop): Tools rows stack on phones; names wrap at underscores (113)
- docs(design): 113 (this note)

## The bug (Sky, 2026-10-06 14:08, mobile)

"Whenever I open the Tools view, sometimes the tool names appear totally vertical — like there's no proper mobile formatting for the tools view."

Cause: `ToolRow` was `flex` with `div.min-w-0.flex-1` (name + description) beside `div.flex.shrink-0` (risk label, `GateSelect` `w-32`, `Switch`). At 390 px minus the column's `px-6` and the row's `ml-6` the controls (~300 px with `destructive* (ask)`) left the name column a few px; `wrap-anywhere` then obliged, one letter per line. Same mechanism as the squished chat tables (`fix/surface-table-scroll`, 2026-10-02). "Sometimes" = a long risk label or a narrow viewport. `ServerRow`'s header had the same shape (name + count + `⟳ repo` beside the `shrink-0` status + trash).

## What changed

1. **Names never break mid-word.** `names.ts::segments(name)` splits after each underscore run (`argocd_`, `prod_`, `dr__`, `get_`, …); `Tools.tsx::Name` renders a `<wbr/>` between segments. The spans are `break-words` (overflow-wrap: break-word) as the fallback for a segment wider than the row — unlike `anywhere` it does not shrink the flex item's min-content, so a name no longer collapses to a column of letters. Wrapping over truncation on purpose: an MCP name like `argocd_prod_dr__get_application_resource_tree` has to stay readable, and `title=` tooltips do not exist on a phone. Applied to the server name, the tool name and the `as <registered>` alias.
2. **Tool row stacks below `sm`.** `flex-col gap-2 sm:flex-row sm:items-start sm:gap-3`: name + alias + description, then risk label · gate select · switch on their own left-aligned line (`flex-wrap` so a long label can still fold). Above `sm` the layout is unchanged.
3. **Server row.** The trigger button is `flex-wrap sm:flex-nowrap`, so the count and repo badge drop under a long name on a phone while the trigger still covers the whole name line; the trash stays top-right. The status (`connected` / `login required` / …) is rendered twice by a tiny `Status` component: `hidden sm:flex` beside the name, `sm:hidden` as the first item of the existing controls row (On · Off here · Off always · gate · Login). Above `sm` nothing moved.
4. Checked at 390 px: the empty state wraps in three lines; the add popover is `w-[min(380px,calc(100vw-2rem))]` = 358 px, left 8 / right 366, no horizontal scroll (`scrollWidth` 390 everywhere). The "Open a chat first" hint is a `title=` on the disabled switches; the attribute is set, invisible on touch as before.

Not touched, worth a follow-up: `Skills.tsx` lists use `truncate` (names clip, never go vertical) — no shared row pattern to fix; `Secrets.tsx` has no `wrap-anywhere`/`shrink-0` control group.

## How it was verified

No engine in the worktree and routes render only once connected, so a throwaway vite entry (`harness.html` + `src/dev/tools-harness.tsx`, deleted before commit) mounted `ServerRow`/`AddPopover`/`Empty` with two fixture servers (one with the 46-char name, a `destructive* (ask)` tool, an `off` tool and an alias of 64 chars; one disconnected `login required`). Headless Chromium at 390×844 and 1280×900, expanded, screenshots taken; measured per name span: one client-rect line each at 390 px, the 64-char alias folded at `__`, `scrollWidth` = viewport. 1280 px: status once, beside the name; stacked rows gone.

Gates (`crates/desktop/ui`). First pass on the stale bindings copied from `~/Develop/theuth`; second pass once `~/Develop/theuth-bindings/BINDINGS-READY` appeared (fresh `rpc/bindings/` + `bindings/methods.json` copied in):
- `bun install` here and in `packages/apps`: ok.
- `bun run check`: stale 119 errors, none in `library/Tools.tsx`, `library/names.ts`, `library/names.test.ts`; fresh **0 errors**.
- `CI=1 bun run test`: stale 143 files / 831 pass, 1 fail — `src/rpc/api.test.ts` "METHODS equals the server method list" (331 methods in `api.ts` vs 321 in the stale `methods.json`); fresh: `api.test.ts` + the three `library/*.test.ts` pass (4 files / 9 tests).
- `bun run build`: stale `tsc -b` stopped on the same bindings errors (`bunx vite build` alone ✓); fresh **✓ built in 36.4 s**.
