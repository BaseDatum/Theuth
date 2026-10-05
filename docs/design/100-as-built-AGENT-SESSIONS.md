# 100 as built — AGENT-SESSIONS

Branch `feat/100-agent-sessions` off main `7e514107` (v2.7.0). Desktop UI (`crates/desktop/ui`) only; no Rust, no Tauri, no new dependencies.

## Ask

Sky (2026-10-05): "also agent's subsessions should show up live in the agent tree in the desktop/mobile UI." Asked which tree, Sky chose the Agents page org chart + list: each agent card/row grows a nested, live list of its sessions (desk → work sessions → their subagents): running dot, name, age; appears as they spawn, fades as they finish; tap opens the session. The sidebar's Desks group is unchanged.

## Decided design, as built

### D1. One data hook — `src/components/agents/useAgentSessions.ts`
`useAgentSessions(desk: string | null)` (`:14-28`): `useSessionField(desk ?? '', 'children')` and `useSessionField(desk ?? '', 'running')`; one `api.list_agents(desk)` → `useSessions.getState().setChildren(desk, …)` per desk id on mount (`:18-24`, rejection ignored, same shape as the sidebar's `AgentTree`). `rows` is the children record (a stable empty record without a desk), `live = liveRows(rows)` memoized on `rows`, `deskRunning = desk !== null && running === true`. `liveRows` lives in `src/components/agents/sessions.ts:11` as `stripRows(children, Infinity).shown` (the chat strip's rule, no cap).

### D2. Linger, then fade — `src/components/agents/sessions.ts` + `useLingering`
`LINGER_MS = 700` (`sessions.ts:5`), `Lingering = { ids: string[]; leaving: Record<string, number> }` (`:17`), `NO_LINGERING` (`:19`), `linger(prev, liveIds, now, ms = LINGER_MS)` (`:26-51`): ids kept in first-seen order; one that left is stamped in `leaving` at `now` and dropped once `now - left >= ms`; one that returns is un-marked; new live ids append; returns `prev` itself when nothing changed. `useLingering(liveIds)` (`useAgentSessions.ts:31-47`): re-reduces when `liveIds` changes and arms one `setTimeout` for the earliest leaving row's due time (re-armed per state change, so a batch that leaves together is one timer).

Leaving rows/dots: `opacity-0 transition-opacity duration-700`. Arriving rows and dots: the existing `subagent-dot-in` class (`styles.css:363` — the 160 ms `subagent-in` keyframe, already reduced-motion aware). No animate plugin is installed and no new CSS was needed.

### D3. List view — `src/components/agents/AgentSessionRows.tsx` + `AgentsView.tsx`
`AgentsView.tsx:123-150` `Row` returns a fragment: the agent `<tr>` as before, then `{a?.desk && <AgentSessionRows desk={a.desk} depth={depth} />}` (`:147`). `AgentSessionRows` (`AgentSessionRows.tsx:18-63`) returns `null` unless `deskRunning` or the lingering list is non-empty (`:33`); otherwise one `<tr className="border-0"><td colSpan={6} className="px-3 pt-0 pb-2">` holding a `max-w-sm` column indented `depth * 18 + 36` px (`:38`):
- desk line (`:39-43`): a `Link` to `/c/$session` (the desk), `GLYPH.running` pulsing when `deskRunning` else `GLYPH.idle`, the text `desk`, `ago(desk updated_at)` from the shared `queryKeys.sessions` list (empty when unknown);
- then `Branch` (`:44-59`) with `parent = desk`, `byParent = childrenOf(rows ∩ withAncestors(rows, lingering.ids))`, `unit = 12`, no `keep`, `moreClass` = the sidebar fold button's classes, rendering `SubagentRow` (`cost={undefined}`, `active={false}`) inside a `subagent-dot-in` wrapper that takes the fade classes when the row is leaving and has no shown children (an ancestor of a still-live row stays put).

`SubagentRow` is the sidebar row, exported from `src/components/sidebar/AgentTree.tsx:36` (`export const SubagentRow = memo(…)`, formerly the file-local `Row`; `AgentTree` uses it at `:179`; `Branch`, `GLYPH`, `ago` unchanged for `BroadcastDialog`). It links to `/c/$session`; the sessions `<tr>` has no `onClick` and is a sibling of the agent `<tr>`, so there is nothing to stop — verified in the browser: a row click navigated to `/c/<id>` only.

Phones (`useSidebar().isMobile`): `[&_a]:min-h-11 [&_button]:min-h-11` on the column (`:38`) — measured 44 px rows at a 390×844 viewport; desktop rows are the sidebar's `h-7` (28 px).

### D4. Org chart card strip — `src/components/agents/OrgChart.tsx`
`Card` (`:101-131`): root is now a `<div>` with the former `className`/`style` plus `draggable={false}` and `onPointerDown` stopPropagation (pan still starts on the canvas, not on cards); the former content sits in `<Link to="/agents/$name/$tab" className="flex flex-col gap-1.5">` (`:110-127`); the hover background stays on the card. Below the link: `<CardStrip desk={a?.desk ?? null} />` (`:128`).

`CardStrip` (`:137-155`): `flex h-5 items-center gap-1.5 text-[11px] text-fg-tertiary`; desk dot `size-2 rounded-full` + `bg-success animate-pulse` / `bg-fg-tertiary/40`, title `desk · running|idle` or `no desk`; then `StripDot` per lingering id present in `rows`, first `CARD_STRIP_CAP = 8` (`sessions.ts:8`) then `+N`; right-aligned `{n} live` / `idle` / `no desk`.

`StripDot` (`:157-176`): `<button type="button">` `size-5`, `onPointerDown` stopPropagation, `onClick → navigate({ to: '/c/$session' })`; inner `span.subagent-dot size-2 rounded-full` green (amber on `approval`), keyed on `beat` with `subagent-blink` exactly like `SubagentDot`; `title`/`aria-label` = `${name} · ${stateText} · ${ago}` (`stateText` copied to `sessions.ts:14` since the chat strip's is not exported and `components/chat/**` is off limits). Leaving dots take the fade classes.

`CARD.h` 92 → 112 (`tree.ts:47`); `layout()` untouched; `tree.test.ts` has no `CARD.h` expectation. Measured card `offsetHeight` = 112 with the strip, so edges meet card bottoms.

### D5. Fresh roster status — `src/lib/rpc.ts`
`queryKeys.roster = ['roster']` (`rpc.ts:21`); `components/tasks/queries.ts:21` now reads `agents: queryKeys.roster` (same key, one source). The debounced `sessions()` closure (`:142-148`) invalidates `queryKeys.sessions` and `queryKeys.roster` under the single 500 ms debounce; `on('TurnStarted', sessions)` added (`:161`); `SessionDeleted` (`:173-178`) invalidates the roster immediately next to sessions.

### D6. Mobile
Nothing beyond D3's 44 px rows; phones keep `Table`. Not touched: the `view === 'org' && !sidebar.isMobile` switch.

### D7. Agent detail page
`AgentDetail.tsx` untouched.

### D8. Mock mode
Not extended. Mock agents (`desk: null` or ids with no store rows) render an empty subtree: strip shows `no desk` / `idle`, list adds no `<tr>` (`useAgentSessions` returns the empty record; `list_agents` rejection is ignored).

## Files

New: `components/agents/sessions.ts`, `sessions.test.ts`, `useAgentSessions.ts`, `AgentSessionRows.tsx`.
Edited: `components/agents/AgentsView.tsx` (Row only), `OrgChart.tsx`, `tree.ts` (`CARD.h`), `components/sidebar/AgentTree.tsx` (Row → exported `SubagentRow`), `lib/rpc.ts`, `components/tasks/queries.ts` (one line, see Deviations).

## Gates

```
export PATH=$HOME/.bun/bin:$PATH
cd /Users/Sky.Moore/Develop/theuth-wt-100-sessions/crates/desktop/ui
bun run check 2>&1 | tail -5
$ tsc -b --noEmit
(no output, exit 0)

CI=1 bun run test 2>&1 | tail -15
 Test Files  1 failed | 137 passed (138)
      Tests  1 failed | 780 passed (781)
   Start at  17:30:33
   Duration  278.39s (import 64%, transform 35%, tests 1%)
 FAIL  src/lib/mobile-zoom.test.ts > no <Input>/<Textarea> shrinks its font below 16 px on mobile (iOS focus zoom)
 Error: Test timed out in 5000ms.

CI=1 bunx vitest run src/lib/mobile-zoom.test.ts
 Test Files  1 passed (1)
      Tests  1 passed (1)
   Duration  1.93s
```
`mobile-zoom.test.ts` is the known load flake (97 as-built notes the same 5 s timeout of a synchronous source scan at load average 50–65; this machine was at 45–58 during the runs). It passes alone. All 781 tests green across the full run + the solo re-run; `sessions.test.ts` adds 3 (part of the 781). `pairing.test.ts` was green in every run. Prep: `/tmp/wt100-prep.log` ended `TS_EXIT=0 BUN1_EXIT=0 BUN2_EXIT=0 PREP_DONE`.

## Visual check (done)

`bunx vite dev --port 5181` with `VITE_THEUTH_URL=ws://127.0.0.1:62840` / `VITE_THEUTH_TOKEN=$(cat ~/.theuth/token)` (the running engine from `~/.theuth/engine.json`; without them the page sits on "Starting…" waiting for the Tauri shell — that was the first screenshot). Headless Chrome 1400×900 screenshot of `/agents`: org chart with six cards, each with the strip (grey desk dot + `idle`), edges meeting card bottoms.

Every desk on this engine was idle, so the live path was exercised by seeding the engine's session store from the page (the `EngineContext` value found through a React fiber; `api.list_agents` stubbed for one desk so the sidebar's own re-seed did not overwrite the fake rows): a running work session with a 5-minute-old `started_at`, an `awaiting` one, an `approval` subagent under the running one (`cap: read_only`), one idle (finished) one, plus `running: true` on two desks.
- Org chart: theuth's card showed the pulsing green desk dot, two green dots + one amber, `3 live`; the second desk's dot pulsed with `idle` text (no descendants); dot `aria-label`s read `EKS control plane audit · running · 5m`, `ingress rewrite · awaiting theuth · 0s`, `scout · approval: bash · 0s`; clicking the amber dot navigated to `/c/fake-sub-1`.
- List view (`localStorage theuth.agents-view = list`): under theuth — `● desk 11m`, then `⧗ ingress rewrite`, `● EKS control plane audit 5m`, `⚠ scout read-only` indented one level; under the second agent only `● desk`; the idle (finished) row hidden. Flipping the awaiting row to idle: at 300 ms its wrapper had `opacity-0 transition-opacity duration-700` (computed opacity 0), at 1000 ms it was gone.
- Phone (390×844, touch emulation): `Table` rendered, the same block with every session row at 44 px.

Dev server stopped afterwards; the managed tab closed. Nothing was sent to any agent.

## Deviations

- **D5 import direction.** The brief said to import `keys` from `components/tasks/queries` into `lib/rpc.ts`. That created an `rpc.ts → queries.ts → rpc.ts` cycle which broke `SandboxVms.test.ts`'s `vi.mock('@/lib/rpc', …)` (the original-module import of `queries.ts` bound the unmocked `useApi`: "useApi before the connection opened", 2 tests red). Fixed by defining the key in `rpc.ts` (`queryKeys.roster`) and aliasing it from `queries.ts` (`agents: queryKeys.roster`) — one line in a file outside §3's list, with no behaviour change; the key string is unchanged.
- **Row fade-in.** Reused the existing `subagent-dot-in` class for rows instead of adding a second 160 ms keyframe; it is the same fade.
- **Fade of an ancestor.** A leaving row that still has a shown (live) descendant is not faded, so a work session whose own turn ended while its subagent runs stays opaque instead of blinking out and back.
- **Desk line is a link.** D3 described a line; it is a `Link` to the desk's chat (the ask's "tap opens the session"), with the row's hover style.
- **Block width.** The `colSpan={6}` cell made the sidebar rows stretch across the table (ages at the far right); the column is `max-w-sm`.
- **Strip dot `stopPropagation`.** Kept as decided even though the card `<div>` already stops pointer-down.

## Not verified

- A real spawn/finish over the wire (`SubagentSpawned` → reducer → strip/list; `TurnFinished` → linger): the store transitions were driven by hand, not by a live agent run.
- The roster refresh timing (D5) end to end — no agent turn was started; the wiring is the same debounced closure the sessions list uses.
- `pairing.test.ts` flake handling was not needed (green every run).
