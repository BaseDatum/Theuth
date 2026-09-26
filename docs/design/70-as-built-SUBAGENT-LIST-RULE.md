# SUBAGENT-LIST-RULE as built — one ordering + cap rule for every subagent list

Branch `fix/subagent-list-rule`, worktree `theuth-sub5`, base `b7c38c6`. Code commit `e3aeeb7`, then this report.

## Ask

"can we apply the same max five subagents under any normal session as well, and subagents should always be listed most recently active on top, so the ones most likely to be actively doing something are shown first. so that ordering fix needs to be applied everywhere."

## The rule (`lib/session-tree.ts`)

- `live(state)` — `running | awaiting | approval` (moved here from `AgentTree.tsx`; finished = idle, cancelled, error).
- `Rank = { live, at, created }`, `byRank(a, b)`: live first, then `at` desc, then `created` desc.
- `rank(row, sessions)`: `at` is the newest of `row.activity.last_active`, the session list's `updated_at`, and the row's `started_at`/`finished_at`. **`last_active` is used and is the better stamp**: the engine defines it as `max(last_input, last_stop, turn start)` and falls back to the store's `updated_at` when cold (`agent/src/lib.rs` `SessionActivity`), so it is never older than `updated_at`. The max over all four also covers the gap between an event (the reducer patches `started_at`/`finished_at` on `TurnStarted`/`TurnFinished`) and the debounced `list_sessions` refetch. `created` is the summary's `created_at` (a row the list lacks ties at 0). Stamps compare as `Date.parse` numbers, not strings — RFC 3339 offsets differ between the engine and `new Date().toISOString()`.
- `rankSummary(s)`: the same rule over `SessionSummary` rows (`running` is the live bit) for lists without subtree rows.
- `cappedTree(rows, sessions, expanded, keep = ∅, limit = TREE_CAP)` → `{ shown, hidden }`: sorted by `byRank`; folded = first five **plus any `keep` row past the fifth, appended in rank order** (the open chat and its ancestors — chosen over auto-expanding so the fold stays five-ish and the button still says what is hidden); expanded = all. `hidden` is the count `Show more` reveals.
- `childrenOf(rows)` — subtree rows keyed by `parent`; `withAncestors(rowsById, ids)` — those ids that are rows plus every ancestor found in the subtree.

## Where it applies

| site | before | now |
|---|---|---|
| `sidebar/AgentTree.tsx` under a **chat row** (`SessionList` `{ tree }` row) | flat `list_agents` pre-order, uncapped | `Branch` per parent: each parent's children ranked and capped on their own, at every depth |
| `sidebar/AgentTree.tsx` under a **desk row** (`AppSidebar` `DeskRow`) | own `cappedTree` path via a `cap` prop, running-first / `updated_at` | same `AgentTree`, same `Branch`; `cap` prop and `NONE` removed — one implementation. `liveOnly` now keeps live rows **and their ancestors** so a live grandchild still hangs under its (finished) parent |
| collapsed chat row / desk row glyph (`SessionItem` `SessionSubagents`) | counts running descendants | unchanged — it orders nothing (one glyph + count) |
| `chat/BroadcastDialog.tsx` targets list | root + flat descendants in `list_activity` pre-order | root, then `Branch` over `childrenOf(g.children)` — the same ranked, capped, nested list (`unit` 16 px, 44 px fold row). `groupRows` unchanged (roots and orphan handling) |
| `agents/AgentDetail.tsx` `Runs` | `updated_at` desc | `byRank(rankSummary)` — running first, then most recent; no cap (a history tab, not a child list) |
| `chat/Header.tsx` | `Parent` breadcrumb only — lists no children | — |
| `chat/SessionMenu.tsx` | lists no children | — |
| `chat/CostPill.tsx` children table | engine order | unchanged — a cost breakdown (`CostBucket` has no state or activity to rank by) |
| `slash.ts` `/agents` | toast pointing at the sidebar | — |

`grep 'parent ===|\.children\b|subagents|AgentTree'` over `src`: the remaining hits are roster trees (`agents/tree.ts`, `AgentsView`), archive/JSON/element trees, DOM `children`, `parent === null` root filters (`AttachPicker`, `ChatPicker`, `buildRows`) and `lib/private.ts` scope math — none lists a session's subagents.

## `Branch` (`sidebar/AgentTree.tsx`, exported)

`<Branch parent byParent sessions keep? unit moreClass render(row, sub) />`: `useState(false)` fold per parent (component state, never persisted — every mount starts folded), rows from `cappedTree`, each rendered as `render(row, <Branch parent={row.id} …/>)`, then the `Show more (N)` / `Show less` button in `moreClass`, indented `8 + depth·unit` like a row of that depth. `AgentTree` computes `byParent = childrenOf(rows)` (rows filtered to live + ancestors when `liveOnly`), `keep = withAncestors(rows, [active])`, and passes the sidebar `Row`; the session list comes from the shared `queryKeys.sessions` cache (no new RPC).

## Virtualizer (MOBILE-FIXES scrollMargin)

Nothing to change: `SessionList` gives every `li` `ref={v.measureElement}` (TanStack virtual's ResizeObserver), so a fold inside the tree row re-measures it; `watchListOffset` observes the siblings above the list, not the list. Measured in the browser (below): total size 1768 → 1824 → 1768 px, `scrollMargin` 684 = menu height throughout, 0 px gaps between consecutive `li[data-index]` rects in all three states.

## Tests (`lib/session-tree.test.ts`)

- rank: running/awaiting/approval beat a newer idle row; idle rows newest first; `created_at` breaks an `updated_at` tie.
- rank stamps: `activity.last_active` beats an older `updated_at`; `finished_at` beats an older `updated_at`; a row unknown to the list ranks by `started_at`; `rankSummary` fields.
- cap: 7 rows → 5 shown, `hidden` 2; `keep` of a hidden row appends it (`hidden` 1); expanded shows all; ≤ 5 hides nothing.
- `childrenOf` keys by parent; `withAncestors` walks ancestors, ignores ids not in the subtree.
- `Branch` via `renderToStaticMarkup`: a parent with seven children renders five rows and `Show more (2)`; with `keep = {g, f1}` (an active grandchild and its parent) the fold shows `f1` past the cap with `g` under it and `Show more (1)`.

The old `cappedTree` test (desk-only, `Show N more`) is replaced by these. `live()` test unchanged, now against `lib/session-tree`.

## Verification

- `just ts-desktop` (bindings regenerated, no Rust changes); `bun install` in `crates/desktop/ui` and `packages/apps` (fresh worktree). `bunx tsc --noEmit` → 0. `bunx vite build` → exit 0. `CI=1 bunx vitest run` → **82 files, 499 tests passed**.
- Browser, `vite --port 5183` against the live engine (`127.0.0.1:63921`), headless Chromium, read-only (navigation and clicks only; nothing created or changed):
  - `/c/01a0d418…` ("Theuth wave 2 merge and cleanup", 7 children, all idle): tree shows `paperclip-recon-2 14h`, `echo-audit-69 1d`, `recon-69-docs 1d`, `recon-69 1d`, `stage2-recon 1d` + `Show more (2)` — newest first — `/tmp/69/sub5-folded.png`.
  - Click → 7 rows + `Show less`; click → back to 5 + `Show more (2)` — `/tmp/69/sub5-expanded.png`. Row rects contiguous; list height 1768 → 1824 → 1768; offset 684 stable.
  - `pushState` to the 7th child (`theuth-recon`, hidden when folded): the fold shows the five + `theuth-recon` highlighted as the sixth row and `Show more (1)` — `/tmp/69/sub5-active-child.png`.
  - Broadcast… dialog: 54 roots, one with a live child (`dr-review-verifier` indented 24 px under `DR pre-drill verification checklist`), no console errors — `/tmp/69/sub5-broadcast.png`. No root had > 5 live children, so the dialog's fold is covered by the `Branch` test.
  - No session had a running child during the smoke; live-first ordering is covered by the rank tests.

## Deviations / notes

- Active child: appended past the cap (not auto-expanded); documented above and in `cappedTree`'s doc comment.
- Label is `Show more (N)` (the brief's wording); DESK-SUBAGENTS had `Show N more`.
- `AgentDetail › Runs` gets the ordering only, no cap — it is the agent's run history, and its rows are `SessionSummary` (hence `rankSummary`).
- Desk rows still highlight when a subagent is the open chat; chat rows still do not (DESK-SUBAGENTS note stands).
- `Row` in `AgentTree` still indents by `row.depth` (12 px/level); nesting is structural in the DOM only through `Branch`, the flat `div` layout is unchanged.

DONE SUBAGENT-LIST-RULE
