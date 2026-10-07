# 132 — ATTENTION-ROWS-AND-ORDER — as built (UI, `feat/132-ui`)

Base `7b518912` (v2.12.0 + the design). Everything under `crates/desktop/ui/src/` plus this note. Rust untouched; no new
routes. Bindings stubbed locally per design §2.4 (gitignored, not committed): `Priority.ts`; `Task.priority: Priority`,
`TaskCreate.priority?`, `TaskPatch.priority?`, `Packet.if_wrong?: string | null`, `AttentionItem.priority?: Priority | null`
+ `AttentionItem.rank: number`. `UI/` = `crates/desktop/ui/src/`.

## File map (path:line)

D1 Row — `UI/components/attention/Row.tsx`
- `:12 PriorityChip({ priority })` — `urgent` → `border-fg-accent text-fg-accent` (gold-ochre), `high` → `border-warning
  text-warning`; medium/low/null render nothing. Sits after the kind chip, before the double's chip.
- `:21 Row` — `item.title` stays the two-line lead (now the ask / the outcome, engine-composed); `item.detail` renders as
  one truncated secondary line only for `decision` and `review` (the byline); the from/agent/age line is unchanged.
  Nothing is composed client-side.

D4 Rail order — `UI/components/attention/model.ts`
- `:34 groupItems(items)` — every `priority === 'urgent'` item goes to a leading `{ key: 'urgent', label: 'Urgent' }`
  group and leaves its kind group; the kind groups follow `GROUPS` order; empty groups omitted; inside every group the
  engine's order is kept (`filter` only — the `sort((a, b) => b.at_ms - a.at_ms)` of 104 is gone).

D4 Priority control — `UI/components/attention/shared.tsx`, `UI/components/hud/queries.ts`
- `shared.tsx:117 PRIORITIES`, `:124 PrioritySelect({ item })` — the repo's `Select` (`size="sm"`, borderless, styled
  like the Footer's ghost buttons), label "Priority" + value, `urgent · high · medium · low`; value `item.priority ??
  'medium'`; `null` when `item.task` is null. Placed as a Footer child in `DecisionScreen.tsx:156` and
  `ReviewScreen.tsx:144` (after Send back) — the Footer is `flex-wrap`, so it wraps at 390 px.
- `hud/queries.ts:222 useAttentionActions().priority` — `api.task_update(id, { priority })`, invalidating
  `ATTENTION_KEY`, `tasks`, `agents` (the shared `keys`).

D2 If wrong — `shared.tsx:75 IfWrong({ text })` — one callout line `If wrong — <text>` (`bg-layer-2`, bordered);
`DecisionScreen.tsx:124` directly above the option cards (after the Options/Overall title); `ReviewScreen.tsx:115` at the
top of the sticky bottom bar, above the send-back form and the buttons. Renders nothing without `packet.if_wrong`.

D3 Review next moves — `UI/components/attention/ReviewScreen.tsx`, `shared.tsx`, `hud/queries.ts`, `rpc/api.ts`
- `shared.tsx:89 OptionCards({ options, picked, recommended, onPick })` — the Decision screen's option-card markup
  lifted out verbatim (○/● · label · `recommended` badge · description, full width) and shared; `DecisionScreen.tsx:125`
  uses it with `picked = typed ? null : choice`.
- `ReviewScreen.tsx:33-45` — `picked` state (`undefined` = untouched → the packet's `recommended` applies when it lands,
  the 101 D8 idiom); `pick` is the lit index only when it indexes a real option; `move('done')` sends `next: pick`
  (`undefined` with no pick → no 4th param on the wire). `:107` "Next move" section with the cards (a second tap on the
  lit card clears the pick); `:137` the primary button reads `Approve → <picked label>` or `Approve → done`. Send back
  unchanged.
- `hud/queries.ts:226 move` — `{ id, state, note, next? }`; `next === undefined` → `api.task_move(id, state, note)`
  (three params, as before), else `api.task_move(id, state, note, next)`.
- `rpc/api.ts:550 task_move(id, state, note, next?: number | null)` — same method name, `METHODS` unchanged.

D6 Toast — `hud/queries.ts:207` — `answer_ask` → `false` with answers: "That question had already closed — your answer
went to the session as a message."; with `null` (Dismiss) the old "That question was already answered" stays (nothing
went anywhere).

D7 Tray — `UI/App.tsx:421-432, 464-468` — `badge(n)` unchanged; the number is the `attention_list` cache's length
(`qc.getQueryData<AttentionItem[]>(ATTENTION_KEY)?.length ?? 0`, the same list `badgeCount` reads for the sidebar),
observed live with a `QueryObserver(qc, { queryKey: ATTENTION_KEY, enabled: false })` — never fetched from here; the
sidebar's `useAttention` owns the fetch. The `st.approvals.subscribe` badge is gone; companion handling as 130 D8.

D10 Crowded world — `UI/components/settings/sandbox-queries.ts`, `hud/queries.ts`, `attention/queries.ts`
- `sandbox-queries.ts:236 crowdedFlag()` — `?mock=crowded`. `:249 CROWD` — 60 `[kind, priority, title, detail, agent]`
  rows: 15 decisions (questions as first lines, byline `who · task`), 12 reviews (outcomes, byline `“task” · by who`),
  6 asks, 5 approvals, 4 shows, 3 hand-offs, 3 secrets, 2 resumes, 2 hires, 2 budgets, 2 blocked, 1 node, 1 double,
  2 finished; every priority on every task-born kind, urgent across six kinds. `:316 ATTENTION_CROWDED` — ids
  `<kind>:crowded-<i>`, `rank` = 9/7/5/2 by priority, sorted rank desc then `at_ms` desc (the engine's order);
  decisions/reviews point at the sim packets (`decision:apr-7f3a`, `review:<sim task>`) so their screens are full.
- `hud/queries.ts:53 useAttention` — `sim ? ATTENTION_SIM : crowded ? ATTENTION_CROWDED : api.attention_list()`.
- `attention/queries.ts:9 usePacket` — `PACKET_SIM` answers under `?sim=` and `?mock=crowded`.
- `PACKET_SIM` gains `if_wrong` on the two decisions and the review (`:399`, `:439`, `:475`) and the review's three
  next moves with `recommended: 0` (`:469`). `ATTENTION_SIM` rows carry `priority`/`rank` (the dns decision is `high`).

## What was verified and how

- `bun run check` (tsc, under build-lock): 0 errors in every owned file; 6 errors remain in four files outside this
  worker's ownership, all caused by the contract's two REQUIRED fields (`Task.priority`, `AttentionItem.rank`) on
  literals that predate them — see Open items. The same six will appear on the merged tree with the real bindings.
- `bunx vitest run src/components/attention src/components/hud src/rpc`: 10 files, 68 passed, 1 skipped (pre-existing).
  New/changed: `model.test.ts` — engine order kept inside groups (`a1,a2`, `h,s`, `f1,f2`), Urgent group first across
  decision/ask/budget while high/low stay in their kinds, the crowded fixture (60 rows, 14 kinds, 4 priorities, Urgent
  group holds every urgent row and only those, nothing lost, ids unique); `double.test.ts` — double rows in engine order.
- `bun run build` / `vite build`: see the gate line in the final message.
- Screenshots (vite dev + Chrome): see the final message for paths or the skip note.

## Deviations from the design

- Steps 4–7 of the brief (priority select, if_wrong, next moves, toast) landed in one commit (`c93e06c4`): they share
  `shared.tsx`/`hud/queries.ts` hunks and splitting them by hand would have meant partial-file staging.
- Tests that encoded the 104 newest-first client re-sort were changed to the design (engine order):
  `model.test.ts` (`a2,a1` → `a1,a2`; `s,h` → `h,s`; `f2,f1` → `f1,f2`) and `double.test.ts` (`double:b, double:appr-1`
  → `double:appr-1, double:b`).
- D6 toast: the new text is shown only when answers were sent (`answers !== null`); a late Dismiss (`null`) keeps the old
  text, because per §2.3 nothing is delivered for `None`. Smaller reading of "where the UI handles `answer_ask` returning
  `false`".
- D3 "recommended pre-picked": a second tap on the lit next-move card clears the pick (so `Approve → done` without
  `next` is reachable without an extra control). Not in the design; one line.
- The `Priority` select was placed as a Footer child (Decision: alone; Review: after Send back) rather than a separate
  row under the header — the brief allowed either.
- Crowded fixture: every row carries a priority (session rows via "a session that is a task checkout", §2.2), so the
  Urgent group demonstrably spans six kinds; the real engine fills `null` for session rows outside a checkout.

## Open items

- tsc on the merged tree: add `priority: 'medium'` to the `Task` literal builders in `UI/components/tasks/mock.ts:22`
  and `UI/components/tasks/helpers.test.ts:6`, `UI/components/hud/scene/fixtures.ts:27`, `UI/components/hud/model.test.ts:17`;
  add `rank: 5` to the `AttentionItem` builders in `UI/components/hud/scene/fixtures.ts:110` and
  `UI/components/hud/model.test.ts:44`. Four files outside this worker's ownership (one of them the 128 scene); six
  one-token edits. Reported to the orchestrator during the run.
- `UI/components/chat/AskCard.tsx:206` carries a second late-answer toast ("the question was already answered", the chat
  card's own path). Outside ownership; the D6 wording should land there too.
- `packages/apps` had no `node_modules` in this worktree; `bun install --frozen-lockfile` there was needed before tsc
  could resolve `@openuidev/*` (no file changes).
