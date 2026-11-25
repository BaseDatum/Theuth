import { expect, test } from 'vitest'
import { HYSTERESIS, approvalsOff, PANEL_GAP, PANEL_MAX, TIERS, decideTier, headerOrder, layout, panelGeometry } from './header'

test('the full bar holds until it overflows, then the compact bar; a tier never measured is never entered', () => {
  expect(decideTier(0, 900, [700])).toBe(0)
  expect(decideTier(0, 699, [700])).toBe(1)
  // Nothing measured yet (first render): stay put rather than guess.
  expect(decideTier(0, 100, [])).toBe(0)
  expect(decideTier(1, 900, [])).toBe(1)
})

test('coming back needs the wider layout to fit with hysteresis, so the boundary does not flap', () => {
  const need = [700, 300]
  expect(decideTier(1, 700, need)).toBe(1)
  expect(decideTier(1, 700 + HYSTERESIS - 1, need)).toBe(1)
  expect(decideTier(1, 700 + HYSTERESIS, need)).toBe(0)
  // Around the flip point every width has one stable answer per direction.
  for (let w = 690; w < 700 + HYSTERESIS; w++) {
    expect(decideTier(1, w, need)).toBe(1)
    expect(decideTier(0, w, need)).toBe(w < 700 ? 1 : 0)
  }
})

test('tier 2 sheds when even the compact bar overflows, and the top tier is a ceiling', () => {
  expect(decideTier(1, 250, [700, 300])).toBe(2)
  expect(decideTier(2, 200, [700, 300, 220])).toBe(TIERS - 1)
  expect(decideTier(2, 300 + HYSTERESIS, [700, 300, 220])).toBe(1)
  expect(decideTier(2, 299 + HYSTERESIS, [700, 300, 220])).toBe(2)
})

const items = [
  { id: 'nav', slot: 'bar' },
  { id: 'cost', slot: 'menu' },
  { id: 'context', slot: 'both', shed: true },
  { id: 'panel', slot: 'bar', shed: true },
  { id: 'menu', slot: 'bar' },
] as const

const ids = (xs: readonly { id: string }[]) => xs.map((x) => x.id)

test('tier 0 shows every item inline and the chevron menu is empty', () => {
  const { bar, menu } = layout(items, 0)
  expect(ids(bar)).toEqual(['nav', 'cost', 'context', 'panel', 'menu'])
  expect(menu).toEqual([])
})

test('tier 1 keeps bar and both items inline; menu and both items go behind the chevron, in header order', () => {
  const { bar, menu } = layout(items, 1)
  expect(ids(bar)).toEqual(['nav', 'context', 'panel', 'menu'])
  expect(ids(menu)).toEqual(['cost', 'context'])
})

test('tier 2 moves shed items out of the bar and into the menu, so nothing is lost', () => {
  const { bar, menu } = layout(items, 2)
  expect(ids(bar)).toEqual(['nav', 'menu'])
  expect(ids(menu)).toEqual(['cost', 'context', 'panel'])
  // Every item is reachable somewhere at every tier.
  for (let t = 0; t < TIERS; t++) {
    const l = layout(items, t)
    expect([...new Set([...ids(l.bar), ...ids(l.menu)])].sort()).toEqual(ids(items).sort())
  }
})

test('215 C3: no model item; the title follows the sidebar toggle in the full bar and opens the compact bar', () => {
  const order = headerOrder(true)
  expect(ids(order)).not.toContain('model')
  const full = ids(layout(order, 0).bar)
  expect(full.indexOf('title')).toBe(full.indexOf('sidebar') + 1)
  // Compact and narrowest: title · popout · chevron · ⋯ stay inline; never the title or the popout button behind the chevron.
  for (const tier of [1, 2]) {
    const l = layout(order, tier)
    expect(ids(l.bar)).toEqual(['sidebar', 'title', 'popout', 'more', 'menu', 'close'])
    expect(ids(l.menu)).not.toContain('title')
    expect(ids(l.menu).indexOf('memory')).toBe(1)
  }
  // Open in new window is reachable without opening the chevron at every tier.
  for (let t = 0; t < TIERS; t++) expect(ids(layout(order, t).bar)).toContain('popout')
})

test('where no second window exists (iOS, or the popped window itself) the popout item is absent at every tier', () => {
  const order = headerOrder(false)
  for (let t = 0; t < TIERS; t++) {
    const l = layout(order, t)
    expect(ids(l.bar)).not.toContain('popout')
    expect(ids(l.menu)).not.toContain('popout')
  }
  // Everything else is where it was.
  expect(ids(layout(order, 2).bar)).toEqual(['sidebar', 'title', 'more', 'menu', 'close'])
})

test('the talk view keeps Back to chat inline top-left at every tier; the chat view has no such item', () => {
  const order = headerOrder(true, true)
  for (let t = 0; t < TIERS; t++) {
    const bar = ids(layout(order, t).bar)
    expect(bar.indexOf('chat')).toBe(bar.indexOf('sidebar') + 1)
    expect(bar.indexOf('chat')).toBeLessThan(bar.indexOf('title'))
  }
  expect(ids(headerOrder(true))).not.toContain('chat')
})

test('141 R3 T11: a watch-only child keeps its Watching chip (the parent item) inline at every tier, ahead of shed items', () => {
  const order = headerOrder(true, false, true)
  for (let t = 0; t < TIERS; t++) {
    const bar = ids(layout(order, t).bar)
    expect(bar).toContain('parent')
    expect(bar.indexOf('parent')).toBeGreaterThan(bar.indexOf('title'))
  }
  // Tier 2 still lists it in the menu as the "Watching inside" row too.
  expect(ids(layout(order, 2).menu)).toContain('parent')
  // A native child's parent crumb stays behind the chevron as before.
  expect(ids(layout(headerOrder(true), 1).bar)).not.toContain('parent')
})

test('the panel is centred on a wide pane and never wider than PANEL_MAX', () => {
  expect(panelGeometry(222, 1122, 1122)).toEqual({ left: 222 + 450 - PANEL_MAX / 2, width: PANEL_MAX })
})

test('a narrow pane shrinks the panel to its width minus the gaps, wherever the pane sits', () => {
  // Phone: the pane is the viewport.
  expect(panelGeometry(0, 375, 375)).toEqual({ left: PANEL_GAP, width: 375 - 2 * PANEL_GAP })
  // Far-left split pane.
  expect(panelGeometry(0, 241, 1375)).toEqual({ left: PANEL_GAP, width: 241 - 2 * PANEL_GAP })
  // Far-right split pane: right edge stays PANEL_GAP inside the pane and the viewport.
  const g = panelGeometry(1134, 1375, 1375)
  expect(g).toEqual({ left: 1134 + PANEL_GAP, width: 241 - 2 * PANEL_GAP })
  expect(g.left + g.width).toBe(1375 - PANEL_GAP)
})

test('a pane partly off the viewport is clamped to the visible part', () => {
  expect(panelGeometry(-100, 300, 1000)).toEqual({ left: PANEL_GAP, width: 300 - 2 * PANEL_GAP })
  expect(panelGeometry(900, 1300, 1000)).toEqual({ left: 900 + PANEL_GAP, width: 100 - 2 * PANEL_GAP })
  expect(panelGeometry(0, 10, 1000).width).toBe(0)
})

test('183 §4.5: the approvals pill follows the session\'s effective value on its owner, not this engine\'s switch', () => {
  const own = { global: true, session: true }
  // The owner says prompts are off for this chat (its pin or an ancestor\'s) though this machine\'s switch is on.
  expect(approvalsOff({ session: false }, own)).toBe(true)
  // Pinned on under a global-off switch: no pill.
  expect(approvalsOff({ session: true }, { global: false, session: false })).toBe(false)
  // Until the reader answers (or the owner is offline): this engine's own switch.
  expect(approvalsOff(undefined, { global: false, session: true })).toBe(true)
  expect(approvalsOff(undefined, own)).toBe(false)
  expect(approvalsOff(undefined, null)).toBe(false)
})

test('the context, cost and approvals-off chips and the panel buttons are gone from the header at every tier (composer bar and the bar above it); the rest stays', () => {
  const order = ids(headerOrder(true))
  for (const gone of ['context', 'cost', 'approvalsOff', 'terminals', 'browser', 'artifacts', 'codemap', 'desktop']) expect(order).not.toContain(gone)
  for (const kept of ['title', 'memory', 'parent', 'owner', 'shared', 'pending', 'popout', 'more', 'menu', 'close']) expect(order).toContain(kept)
})
