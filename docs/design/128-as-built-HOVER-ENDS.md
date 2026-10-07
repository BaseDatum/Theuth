# 128d as-built — HOVER-ENDS + CLOCK-TICK (UI-only, design 128 Revision 5)

Branch `feat/128d-hover-ends`, worktree `theuth-128d-hover`, cut from main `6ecd6b39`. Brief: `theuth-128-briefs/brief-HOVER-ENDS.md`.
Design: `docs/design/128-hud-scene.md` → `## Revision 5 — hover shows one flow and both its ends; clock tick`.

Sky, 2026-10-07 21:05 — "what is this little orange artifact behind the time in the top right?" · "why when I hover over any orange
line it only highlights the ones at the top? shouldn't hovering on any line highlight it's two ends or something like that?"

## Causes (as found on main)

1. `Scene.tsx` drew `<rect x=1852 y=9 width=3 height=10 fill=C.accent>` when needs-you > 0, under the right-anchored clock text
   (x 1896, `● HH:MM:SS` ≈ 60 px at 10 px) — the tick showed through the hour digits.
2. Hover was CSS only (`.hud-scene:has([data-flow]:hover) [data-flow] {opacity:.5}` / `:hover {opacity:1}`). Every needs-you item
   older than today was clamped to the arc's 00:00 point and all of them ran theuth-ring → that point along identical paths, so the
   browser gave the hover to the topmost (last-drawn) flow; a 1.1 px stroke is hard to hit; the hovered group was never lifted; the
   left origin (need dot / ring) was never emphasised.

## Per-file changes

### `crates/desktop/ui/src/components/hud/scene/model.ts`
- `EARLIER_H = 0.3`. New `staleFan(flows, now)`: flows with `time < startOfDay(now)` ranked oldest first (ties by id); the k-th of n
  gets `hour = −0.3 · (n − k) / n` (oldest furthest, never 0); every other flow untouched, order preserved. Generic over
  `{ id, time, hour }` so it runs on the pre-layout flow list.
- `ArcModel.earlier: { x, y, rot } | null`; `arc(now, routines, activityToday, g, stale = false)` sets it to the point at hour
  `−0.75` on `R − 4`, rotated like the hour labels, when `stale`.
- `scene()`: `fanned = staleFan([...needFlows, ...outcomes, ...running], now)` → `layoutEnds(fanned)`; `arc(…, fanned.some(h < 0))`.
  `hourOf` itself is unchanged (still the clamp; its doc now points at `staleFan`). Due connectors and hairlines are never stale.

### `crates/desktop/ui/src/components/hud/scene/Scene.tsx`
- Top bar: the rect is gone; the clock is `<text …><tspan fill={need > 0 ? C.accent : C.secondary}>●</tspan> HH:MM:SS</text>`.
  The dashed header line still ends at x 1840.
- `Scene({ …, hot?: string | null })` — `const [hot, setHot] = useState(seed)`; `hotFlow = m.flows.find(id === hot)`;
  `hotRing = hotFlow?.via?.id`.
- Flows: each `To` group (`data-flow`, `hud-flow-in`) gets `onPointerEnter/onPointerLeave/onFocus/onBlur` (focus bubbles from the
  inner link — keyboard parity) and inline `style={{ opacity: 0.35 }}` when another flow is hot. Its children: the hit path
  (`stroke="transparent" strokeWidth={8} fill="none" pointerEvents="stroke"`) first, then `<FlowBody fl need on={false}>`.
  After all flows: `{hotFlow && <g data-hot={id} pointerEvents="none"><FlowBody … on /></g>}` — the hot flow drawn **again** on top.
  A copy rather than a reorder: React would move the `<g>` (remove + insert) and that restarts the `hud-flow-in` CSS fade.
- `FlowBody({ fl, need, on })` (new, module-level): the visible stroke (`pointerEvents="none"`; `on` → width 1.8, opacity 1; due
  connectors keep their dash and go 0.5 → 1), the end dot (1.8 → 2.6), the label (`fl.color` → `C.primary`, `fontWeight 500`), and
  for a `need` its dot (ring stroke 1.2 → 2) and id label (`C.accent` → `C.primary`, 500). `needOf(m, fl)` resolves the dot.
- Rings: groups carry `data-ring={r.id}`; when `r.id === hotRing` the glyph is `RingGlyph … on` (stroke 2 — new optional prop) and
  both labels get `fontWeight 500` (a lapsed ring's name also goes primary while hot).
- Arc: `{m.arc.earlier && <text fontSize={8} fill={C.tertiary} textAnchor="middle" transform=rotate…>earlier</text>}`.

### `crates/desktop/ui/src/styles.css`
- The `[data-flow]` hover pair stays as the no-JS fallback; the dim is 0.35 (was 0.5) so it agrees with the React path. Comment updated.

### `SceneCompact.tsx`, `fixtures.ts`, `Hud.tsx`, `queries.ts`
- Unchanged. The fixture already has two stale needs (`blocked:…1203` −26 h, `secret:gh-token` −50 h).

### Tests
- `model.test.ts` — `R5 — staleFan`: five flows (1 h, −26 h, −50 h, midnight − 1 ms, midnight) → the three stale ones at −0.3 / −0.2 /
  −0.1 in age order, all in `[−0.3, 0)`, order preserved, in-window hours untouched; ties resolved by id; the fixture scene's two
  stale needs at −0.3 / −0.15 with distinct `d`; `arc.earlier` set and left of the 00:00 label point; `null` with the stale items
  filtered out and on a bare `arc()` call.
- `Scene.test.ts` — the main test now asserts `<tspan fill="var(--fg-accent)">●</tspan> 19:49:19` and no `<rect x="1852"`; the quiet
  test asserts the secondary tspan. `R5 — hover (default)`: every flow group (`m.flows.length`, due included) opens with
  `<title>`, the link, the 8 px transparent `pointer-events="stroke"` path, then the 1.1 px visible stroke with `pointer-events="none"`;
  no `data-hot`, no `opacity:0.35`, no `stroke-width="1.8"`. `R5 — hover (hot seeded)`: with `hot = need:approval:ap-4821` the
  `data-flow` order equals model order, the `data-hot` copy comes after the last flow group and is `pointer-events="none"`, with the
  1.8/opacity-1 gold stroke, the 2.6 end dot, the primary 500 label, the need dot ring at 2 and the id `builder` primary 500, and no
  hit path; the original group carries no opacity; exactly `flows − 1` groups carry `style="opacity:0.35"`; `ring:agent:builder`
  (the item's `via`) has the stroke-2 glyph and 500 labels, ops-desk has none; `font-weight="500"` appears 4 times. A hot outcome:
  malachite 1.8 copy without a need dot, its via ring at stroke 2. A hot due connector: the dashed tertiary path alone at 1.8 /
  opacity 1. An unknown id: nothing hot, nothing dimmed.
- Not exercised: the pointer events themselves. The suite is `renderToStaticMarkup` without a DOM (no jsdom/happy-dom/testing-library
  in the tree and no new packages allowed), so the brief's "pointerEnter on the hit path" is covered through the `hot` prop that seeds
  the same state the handlers set; the handlers are one-line `setHot` calls.

## Deviations from the brief

- B asked for "the hovered flow AGAIN last (or reorder so it is last)": the copy was chosen over the reorder because a DOM move
  restarts the `hud-flow-in` fade (the hovered line would blink in on every hover). The copy is `pointer-events: none` so the original
  keeps the pointer and the state does not flicker.
- B's "`hot-ring` class or inline style": inline (`RingGlyph on`, `fontWeight`) — a CSS descendant rule would also have widened the
  gauge track and satellite circles inside the same group.
- `hourOf` was left as the clamp; the fan is a separate pass over the flow list because the rank needs the whole stale set.

## Gates (from `crates/desktop/ui`, each one foreground command under `build-lock`, label `128d`, fresh bindings present)

1. `bun run check` — 0 errors (fresh `node_modules/.tmp/tsconfig.tsbuildinfo` written).
2. `bunx vitest run src/components/hud` — 4 files, **44 passed**.
3. `bunx vite build` — **built in 3.10 s** (the usual > 500 kB chunk warning).

Visual smoke (static render of the fixture `Scene` with the source `:root`/`.dark` tokens inlined, throwaway vitest file, deleted;
headless Chromium screenshots): idle — gold clock dot top right, no tick, the two stale needs crossing the arc at two separate points
before 00:00 under a faint rotated `earlier`; `hot = need:secret:gh-token` — that line drawn bright and wide from the GITHUB_T dot
through the `HUD visual overhaul` ring (bold labels) to a brightened `GITHUB_T · 2d`, everything else at 0.35; `hot =
need:approval:ap-4821` — the `builder` need dot with the 2 px ring and primary id, builder's ring at stroke 2 with bold labels,
the gold line at 1.8. Live pointer events were not exercised (no running app in this wave; `?sim=` needs the Tauri shell).

## Commits

- `87348f3c` model/Scene/styles/tests
- (this file + the Revision 5 section) — see `DONE HOVER-ENDS <sha>`.
