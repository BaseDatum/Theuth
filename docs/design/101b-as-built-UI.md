# 101b — MINI-BOARDS — as built (UI, `feat/101b-ui`)

Base `f83042b7` (main with 101 landed). Everything under `crates/desktop/ui/src/components/{attention,board,settings}`;
Rust untouched; no new routes (`routeTree.gen.ts` unchanged after `bunx vite build`). The binding stub is local only:
`just ts-desktop` copied the real 101 bindings (477 files), then `src/rpc/bindings/Packet.ts` got `board: string | null,`
by hand (gitignored, not committed; integration regenerates it from the ENG branch).

## What was built

- `components/attention/PacketBoard.tsx` (new) — `PacketBoard({ board, summary })`: `useDocument({ doc: board })`
  (`components/editor/useDocument.ts`, the same hook `NotePage.tsx:70` uses). `meta.kind === 'board'` and not `closed` →
  `<BoardReader provider>` inside `div.packet-board`, read-only (none of NotePage's title input / Edit toggle / share
  actions come along). No provider yet, or no `meta` while `connecting` → three `Skeleton` lines. Otherwise one line —
  "The board for this packet could not be opened" (`doc_open` failed: missing id → the provider goes `offline` with no
  `meta`), "…is gone" (`closed`), "…is not a board document" (`meta.kind !== 'board'`) — with `open the note` →
  `/notes/$id`, and `PacketMarkdown(summary)` under it.
  `WalkMeThrough` (same file): `useBlocks(provider)` + `useTourBoard(blocks)` (both already exported from
  `components/board/{blocks,Tour}`), a ghost `xs` button "Walk me through" in the reader's chrome row, `md:hidden`.
- `components/board/BoardReader.tsx:47` — `BoardReader({ provider, children })`; `{children}` renders at the end of the
  chrome row (`N anchors · Verify`, :188), i.e. under the anchor-store context the tour hook needs. NotePage passes
  nothing → unchanged (verified: `/notes/<board>` still has the absolute sheet, its own scroll, no Walk button).
- `components/attention/PacketBoard.css` (new) — the embedding overrides, all scoped to `.packet-board`; the reader's
  own `board.css` stays:
  - `.packet-board { width: 100cqi; margin-left: calc(50% - 50cqi); overflow-x: clip }` — the board spans the whole
    attention pane (the `Body` column is centred, so this is exact), prose stays at the reader's own 720 / 860 px via
    `board.css`; the screen's `px-4 md:px-6` is untouched for everything else.
  - `.board-scroll { overflow: visible; grid-row: 1 }` — the screen's `Body` is the scroller, the board flows between
    the question and the options/action bar.
  - Narrow (`@container (max-width: 919px)`): the reader's bottom sheet (`.board-pane`) becomes `grid-column: 1 / -1;
    grid-row: 1; align-self: end; position: sticky; bottom: 0; height: 40cqh; z-index: 30` — stuck to the scroller's
    bottom edge while the board is in view, scrolls away with the board's end so the Answer form / action bar are never
    covered. (`.board-shell` is an inline-size container, which also makes it the containing block for `fixed`; sticky
    against the Body scroller is what works.)
  - Wide (`@container (min-width: 920px)` = `PANE_MIN + 560`): the side pane — `grid-template-columns: … 10px
    min(var(--board-pane-width), calc(100cqi - 560px))` (the prose column keeps ≥ 560 px whatever width the user
    persisted in `/notes`), resizer shown, pane `sticky; top: 0; align-self: start; max-height: 100cqh`.
  - TOC: the reader's absolute pill at the board's top-left (visible at the top of the board); at ≤ 720 px the open
    card sits under the pill (`inset: 32px auto auto 24px`, `max-height: 60cqh`) instead of the reader's sheet at the
    board's far end (which, in flow, is off screen). The rail (`useRail`, ≥ 1360 px windows) sticks to the top.
  - `≤ 720 px: .board-prose code { overflow-wrap: anywhere }` — a long inline token widened the screen's scroller by
    95 px on the review board; `overflow-x: clip` catches the rest (a callout's long word, 14 px).
- `components/attention/shared.tsx:35` — `Body`'s scroller gains `[container-type:size]`, so `cqi`/`cqh` in the CSS
  above are the pane's width and visible height (the reader's own containers are inline-size only, so `cqh` falls
  through to the Body).
- `DecisionScreen.tsx:65` — `p.board ? <PacketBoard board summary/> : <PacketMarkdown summary/>`; header · question ·
  board · "Look at" `Pointers` · options + Answer · footer otherwise unchanged.
- `ReviewScreen.tsx:39,64` — the inlined scroller became `<Body>` (identical markup, now with the container class);
  board when set, then Acceptance, "Review these", the fixed action bar — unchanged otherwise.
- `components/settings/sandbox-queries.ts:191` — `SIM_BOARD_DECISION` / `SIM_BOARD_REVIEW`; both `PACKET_SIM` entries
  carry `board`.

## Sim and how it was verified

The browser cannot `doc_create(kind="board")` (`api.ts:311 doc_create(title, markdown)` only makes notes), but
`doc_list("board", 20)` on the local daemon (ws://127.0.0.1:58311, `~/.theuth/token`) returned three board notes. The
sim points the decision at `agqpgxemf5zcdnb45r3i6ryn3e` (RCA — P-PROD-APP1) and the review at
`agqol5d3h52ohgmj56t666drem` (RCA: SWIFT MQ). On another machine those ids do not exist and the screens show the
"could not be opened" line + summary (itself a state worth seeing). The missing-document and not-a-board fallbacks were
exercised by pointing the review sim at `nope-missing` and at a plain note id (`agqn4a3v2574dmwckfgtcx42q4`) in the dev
session only, then reverting (the committed sim has the board ids).

Exercised in the managed headless Chromium against the live daemon (`bunx vite --port 5182`, `?sim=sandbox-ok&mock=1`):

- 1280 × 900, sidebar open (pane 700 px → narrow mode): board at the top of both screens with the TOC pill, "Contents"
  card with **Tour** under the pill, Tour from the TOC (full-screen portal, 52 stops), pin an anchor → sheet stuck at the
  scroller's bottom (y 561–900), scroll on → "Look at" pointers, options, Answer, footer with the sheet gone.
- 1280 × 900, sidebar closed (pane 960 px → wide mode): pin → side pane 400 px (`960 − 560` clamp), column 550, resizer,
  pane `position: sticky` spanning the pane height.
- 390 × 844 (touch emulation): decision — chrome row with Verify + **Walk me through**, tap → tour (full screen, stacked
  layout), close, tap an anchor → sheet inline at the bottom 40 %, Answer reachable below; review — board, fixed action
  bar (Approve → done · Send back · Open task · Snooze · Dismiss) under the scroller, "Review these" diff/excerpt below
  the board, TOC card opens under the pill. `Body.scrollWidth === clientWidth` (no sideways scroll) on both.
- `/notes/agqpgxemf5zcdnb45r3i6ryn3e`: unchanged reader behaviour.

No mutation was clicked against the live daemon.

## Tests

No new pure helper: the side-pane threshold is the CSS container query above (the brief's `boardFits(width)` would
have needed a ResizeObserver to feed it). `model.test.ts` untouched; suite otherwise as on main.

## Gates (run from `crates/desktop/ui`, 2026-10-06 00:2x–00:4x)

```
just ts-desktop                       # exit 0, 477 files copied; then `board` added to src/rpc/bindings/Packet.ts by hand
bun install && (cd ../../../packages/apps && bun install)   # 619 + 484 packages
bun run check 2>&1 | tail -3          # tsc -b --noEmit, exit 0
CI=1 bun run test 2>&1 | tail -8      # Test Files 1 failed | 140 passed (141); Tests 1 failed | 793 passed (794); exit 1
bunx vite build 2>&1 | tail -2        # exit 0; routeTree.gen.ts unchanged; tree clean
```

The one failure is `src/lib/mobile-zoom.test.ts` — a source scan that **timed out** at the 5 s default (6.2–14.9 s per
run) with the machine at load average 45–57 from the sibling cargo runs; `bunx vitest run src/lib/mobile-zoom.test.ts
--testTimeout=120000` → 1 passed. Nothing in this branch adds an `<Input>`/`<Textarea>`. `api.test.ts › METHODS` is green
(101's `packet_get` is in `bindings/methods.json` on this base).

## Screenshots (DPR 1.25 → 1600×1125 / 488×1055 px)

- `/tmp/mc-2026-10-05/ui101b-decision.png` — `/attention/decision%3A0192a000-…0002%3Adecision%3Aapr-7f3a` 1280×900
- `/tmp/mc-2026-10-05/ui101b-review.png` — `/attention/review%3A0192a000-…0003` 1280×900
- `/tmp/mc-2026-10-05/ui101b-decision-sidepane.png` — the decision at 1280×900 with the sidebar closed and an anchor pinned (side pane)
- `/tmp/mc-2026-10-05/ui101b-decision-phone.png`, `ui101b-review-phone.png` — 390×844
- `/tmp/mc-2026-10-05/ui101b-review-phone-sheet.png` — 390×844 with an anchor pinned (inline sheet)

## Deviations from the brief / design

- **`BoardReader` gained an optional `children` prop** (brief: export-only changes in `board/**`). The phone tour button
  must call `useTourBoard`, which needs the anchor store the reader creates; a children slot in the chrome row is the
  smallest way in and changes nothing for NotePage.
- **No `boardFits(width)` helper / test** — the ≥ 920 px rule is a container query (no measuring code); the reader's own
  ≤ 1080 px sheet rule is overridden in the 920–1080 band with the pane clamped to `100cqi − 560px`.
- **The board is full-pane width**, not inside the 720 px `Body` column: the side pane cannot fit otherwise. The reader
  keeps its own 720 / 860 px prose limits inside; the screen's padding stays for the rest.
- **TOC pill is not sticky** — a sticky pill overlapped the prose on a 700 px pane; it stays where the reader puts it
  (top of the board). On phones **Walk me through** stands beside it in the chrome row.
- **Sim board ids are this daemon's** (no way to create a board from the browser); documented above.
- `Body` is now a `container-type: size` element; `PlainScreen` shares it (no visible change).

## Not verified

- Against a real `Packet.board` from the ENG branch (stubbed binding; D1 validation and the R2 skeleton are ENG).
- A board whose root verifies (both local boards report `verify failed: root … is not a git top-level`, so live anchor
  states / Ask to refresh were not seen in the embedding — unchanged reader code).
- The `useRail` rail (≥ 1360 px window) inside the attention pane — CSS only (`sticky; top: 0; max-height: 100cqh`), not
  rendered at that width.
- Tauri `openUrl` from board links; iOS safe area under the inline sheet on a device.
