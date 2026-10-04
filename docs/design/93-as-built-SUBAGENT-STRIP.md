# As-built — 93 SUBAGENT-STRIP — `Subagents: ● ● ●` above the composer, one blinking dot per live subagent

Branch `feat/93-subagent-strip` in `~/Develop/theuth-wt-93`, based on local main `7d2f00ff` (includes 91 `Ctx.now`). Brief: `/tmp/mc-2026-10-04/omp-SUBAGENT-STRIP.md`. Desktop UI only; TUI and Rust untouched.

Commits (oldest first):
- `0a30de8c` feat(desktop): beat subtree rows on a subagent's streaming events (93)
- `20f17947` feat(desktop): subagent strip above the composer (93)
- docs(design): 93 as-built SUBAGENT-STRIP (this note)

## What changed, per file

**Store**
- `src/rpc/extra.ts:111-112` `AgentInfo.beat?: number` — "client clock (ms) of this subagent's last streaming event — UI only, never on the wire (93)".
- `src/store/reduce.ts:1096-1113` `BEATS` (a `Partial<Record<WireEvent["type"], true>>`: `TextDelta`, `ReasoningDelta`, `ReasoningDone`, `ToolCallPending`, `ToolCallStarted`, `ToolCallProgress`, `ToolCallFinished`, `TurnStarted`), `BEAT_MS = 250`, `beat(row, now)` (unchanged row when `now - row.beat < 250`, else `{ ...row, beat: now }`). `route()` (`:1174-1191`): the ancestor walk now runs when `trackRow(e)` returned something *or* the event beats; per ancestor the row goes through `track.patch` first, then `beat(…, ctx.now)`, and the slice is only replaced when the row actually changed (so a `ToolCallFinished` on a non-awaiting row inside the throttle window leaves the ancestor slice's identity alone). `TurnFinished` is not in `BEATS`: a finished row keeps its last `beat` and never blinks again (and is not live, so it leaves the strip anyway). The child's own slice gets no `beat`.

**Helper**
- `src/lib/session-tree.ts:66-79` `STRIP_CAP = 24`, `stripRows(children, limit = STRIP_CAP)` → `{ shown, more }`: `live` rows only, sorted by `started_at` ascending (rows without one last), name as the tie-break.

**Component**
- `src/components/chat/SubagentStrip.tsx` (new). `SubagentStrip({ session })`: `useSessionField(session, 'children')` → `stripRows` (memoised on the record) → `null` when empty, else one `h-7 shrink-0` row in the composer's frame (`mx-auto w-full md:max-w-[768px] px-4`), `flex items-center gap-2 text-xs text-fg-tertiary select-none`: `Subagents:` then the dots in `flex items-center gap-1.5 overflow-hidden whitespace-nowrap`, `+N` in `tabular-nums` after 24. `SubagentDot({ row })`: `Link to="/c/$session"` with `useSessionDrag(row.id, row.name)` spread, `size-6` (24 px hit area) around a `size-2 rounded-full` circle, `bg-success` (running/awaiting) or `bg-warning` (approval); `title`/`aria-label` = `${name} · ${state} · ${model}` with `awaiting ${from}` / `approval: ${tool}` as in `AgentTree`. The circle is `key={row.beat ?? 0}` and carries `subagent-blink` once it has a beat, so each beat remounts it and the one-shot animation restarts; no timers.
- `src/routes/c.$session.tsx:15,139` import + `<SubagentStrip session={session} />` between `TodoCard` and `Composer`. Not mounted in the talk view, notes chat or HUD.

**Styles** (`src/styles.css:362-396`, next to wave 89's block)
- `.subagent-dot-in` — `subagent-in` 160 ms ease-out (`opacity 0 → 1` on the 24 px link; the circle itself rests at 0.6, so the dot fades 0 → 0.6).
- `.subagent-dot { opacity: 0.6 }`; `.subagent-blink` — `subagent-blink` 380 ms ease-out, `50% { opacity: 1; transform: scale(1.35) }`.
- `@media (prefers-reduced-motion: reduce)`: fade-in off; the blink becomes `subagent-lit` 380 ms `step-end` (held at opacity 1 for the duration, no scale, then back to 0.6).

## Tests
- `src/store/reduce.test.ts` "93: a child's streaming events beat its row on every ancestor…": `TextDelta` at `now=1000` → parent's row `beat: 1000`; child's slice changed but has no `beat`; `now=1100` no bump and the parent slice keeps its identity; `now=1300` → `1300`; grandchild `ReasoningDelta` at `2000` beats its row on both ancestors and leaves the direct child's row at `1300`; `TurnFinished` → `idle` with `beat` still `1300`.
- `src/lib/session-tree.test.ts` "93 stripRows…": live filter (idle/error dropped), order `early(day2) < late(day3) < a-unknown < b-unknown`, `{}` → `{ shown: [], more: 0 }`, 30 rows → `[24, 6]` with `r0` first, `limit=3` → `more: 27`.
- A throwaway SSR render (vitest, `Link`/`useSessionDrag`/`useSessionField` stubbed) checked the markup: label, `subagent-blink` only on the beaten row, `bg-warning` on the approval row, the tree's title phrasing, the idle row absent, empty subtree → `''`. Deleted after the run; not a live Tauri run (no engine in the worktree), so the animation itself was not observed.

## Gates

1. `just ts-desktop` (cold; `/tmp/mc-2026-10-04/strip-ts.log`)
```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 2.73s
EXIT=0
```
(no tracked binding changed — `beat` is client-only)
2. `bun install && bun run check && CI=1 bun run test < /dev/null` (crates/desktop/ui; `bun install` in `packages/apps` first — `@openuidev/*` was unresolved in the fresh worktree, as the brief predicted)
```
$ tsc -b --noEmit
tsc EXIT=0
 Test Files  131 passed (131)
      Tests  732 passed (732)
vitest EXIT=0
```
(`src/link/pairing.test.ts` passed in the full run; no re-run needed)

## Deviations
- `BEATS` is a `Partial<Record<…, true>>` rather than a `Set` (project rule: static string-keyed tables are `Record`s). Same semantics.
- The ancestor walk now skips replacing a slice whose row did not change (`next !== row`), which is slightly tighter than before for `ToolCallFinished` on a non-awaiting row; needed so a throttled beat does not churn every ancestor slice per delta.
- Nothing else: file list, placement, classes, timings and tests are as the brief decided.
