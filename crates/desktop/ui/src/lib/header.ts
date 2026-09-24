import type { ReactNode } from 'react'

/**
 * The pane header's layout tiers, chosen from the pane's own width (not the viewport): 0 = the full bar; 1 = the
 * compact bar (title · context % · one panel button · chevron · ⋯) with the rest behind the chevron; 2 = compact with
 * the sheddable items (context chip, panel button) also behind the chevron, for panes near the 240 px floor.
 */
export const TIERS = 3

/** Hysteresis (px): a wider layout comes back only once the pane exceeds what that layout last needed by this much. */
export const HYSTERESIS = 32

/**
 * One step of the tier decision. `need[t]` is the natural (max-content) width tier `t` last measured at; a tier never
 * rendered has no entry and is never returned to. Climbs while the current tier overflows, falls back one tier when
 * the wider layout would fit with `HYSTERESIS` to spare; the caller re-measures after every render, so a stale
 * `need` corrects itself within the same frame.
 */
export function decideTier(tier: number, width: number, need: readonly (number | undefined)[]): number {
  const cur = need[tier]
  if (cur !== undefined && width < cur && tier < TIERS - 1) return tier + 1
  const wider = need[tier - 1]
  if (tier > 0 && wider !== undefined && width >= wider + HYSTERESIS) return tier - 1
  return tier
}

/** How a header item renders: inline in the full bar, inline in the compact bar, or as a labelled row in the chevron menu. */
export type Mode = 'full' | 'bar' | 'row'

/**
 * One header control or indicator, rendered by every layout from the same descriptor so the full bar and the compact
 * menu cannot drift. `slot` says where it lives when the pane is narrow: `bar` stays inline, `menu` moves behind the
 * chevron, `both` keeps a compact form inline and its details in the menu. `shed` items leave the bar at tier 2.
 */
export type HeaderItem = {
  id: string
  slot: 'bar' | 'menu' | 'both'
  shed?: boolean
  render(mode: Mode): ReactNode
}

/** Which items the bar shows and which the chevron menu lists at `tier`. Tier 0 lists nothing: every item is inline. */
export function layout<T extends Pick<HeaderItem, 'slot' | 'shed'>>(items: readonly T[], tier: number): { bar: T[]; menu: T[] } {
  if (tier === 0) return { bar: [...items], menu: [] }
  return {
    bar: items.filter((i) => i.slot !== 'menu' && !(tier > 1 && i.shed)),
    menu: items.filter((i) => i.slot !== 'bar' || (tier > 1 && i.shed)),
  }
}

/** The chevron panel's widest, and its inset from the pane's and viewport's edges. */
export const PANEL_MAX = 360
export const PANEL_GAP = 8

/**
 * Where the chevron panel goes: centred on the pane, `PANEL_GAP` inside both the pane and the viewport. `left`/`right`
 * are the pane's viewport x-edges; `viewport` its width. A pane narrower than the panel plus gaps gets the panel
 * shrunk to fit; a pane hanging off the viewport (mid-drag, a scrolled shell) is clamped to the visible part.
 */
export function panelGeometry(left: number, right: number, viewport: number): { left: number; width: number } {
  const lo = Math.max(left, 0) + PANEL_GAP
  const hi = Math.min(right, viewport) - PANEL_GAP
  const width = Math.max(0, Math.min(PANEL_MAX, hi - lo))
  const centred = (lo + hi) / 2 - width / 2
  return { left: Math.round(Math.min(Math.max(centred, lo), hi - width)), width: Math.round(width) }
}
