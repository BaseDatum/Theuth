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

export type HeaderId =
  | 'search'
  | 'sidebar'
  /** The talk view's "Back to chat" (TALK-UI-B B3): the same session's `/c/$session`. */
  | 'chat'
  | 'model'
  | 'title'
  /** 137 §11.3: the ∞ badge of an endless session (opens the Memory panel); behind the chevron on a narrow pane. */
  | 'memory'
  | 'parent'
  | 'owner'
  /** 79 §6.5: the owner engine is a sandbox VM — its desktop in a pop-out. */
  | 'desktop'
  | 'context'
  | 'cost'
  | 'terminals'
  | 'browser'
  | 'artifacts'
  | 'codemap'
  | 'approvalsOff'
  | 'pending'
  | 'popout'
  | 'more'
  | 'menu'
  | 'close'

/**
 * One header control or indicator; the component renders every layout from the same descriptor so the full bar and
 * the compact menu cannot drift. `slot` says where it lives when the pane is narrow: `bar` stays inline, `menu` moves
 * behind the chevron, `both` keeps a compact form inline and its details in the menu. `shed` items leave the bar at tier 2.
 */
export type HeaderItem = {
  id: HeaderId
  slot: 'bar' | 'menu' | 'both'
  shed?: boolean
}

/**
 * The header's items in display order. The title follows the model picker in the full bar and, with the picker behind
 * the chevron, opens the compact bar. `panel` is the one panel button the compact bar keeps inline (artifacts, the
 * session's output surface, over the live browser over terminals); the other two move behind the chevron. `popout`
 * (open in a new window) stays inline at every tier: it is how a narrow pane gets room — absent entirely where no
 * second window exists (`canPopout()` false, or this is already the popped window). `chat` (the talk view only) sits
 * top-left after the sidebar toggle and stays inline at every tier: it is the way back. `watching` (a watch-only
 * child, 141) keeps the `parent` item — the `Watching` chip — inline at every tier.
 */
export function headerOrder(panel: 'artifacts' | 'codemap' | 'browser' | 'terminals' | null, popout: boolean, chat = false, watching = false): HeaderItem[] {
  return [
    { id: 'search', slot: 'menu' },
    { id: 'sidebar', slot: 'bar' },
    ...(chat ? [{ id: 'chat', slot: 'bar' } as const] : []),
    { id: 'model', slot: 'menu' },
    { id: 'title', slot: 'bar' },
    { id: 'memory', slot: 'menu' },
    // 141 R3 T11: a watch-only child keeps its `Watching` chip inline at every tier; the crumb joins it in the full bar.
    { id: 'parent', slot: watching ? 'both' : 'menu' },
    { id: 'owner', slot: 'menu' },
    { id: 'desktop', slot: 'menu' },
    { id: 'context', slot: 'both', shed: true },
    { id: 'cost', slot: 'menu' },
    { id: 'terminals', slot: panel === 'terminals' ? 'bar' : 'menu', shed: true },
    { id: 'browser', slot: panel === 'browser' ? 'bar' : 'menu', shed: true },
    { id: 'artifacts', slot: panel === 'artifacts' ? 'bar' : 'menu', shed: true },
    { id: 'codemap', slot: panel === 'codemap' ? 'bar' : 'menu', shed: true },
    { id: 'approvalsOff', slot: 'menu' },
    { id: 'pending', slot: 'menu' },
    ...(popout ? [{ id: 'popout', slot: 'bar' } as const] : []),
    { id: 'more', slot: 'bar' },
    { id: 'menu', slot: 'bar' },
    { id: 'close', slot: 'bar' },
  ]
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

/**
 * 183 §4.5 / §12.2: whether the header shows its `approvals off` pill. The session's effective value on its owner
 * (`get_approvals(session)`: its pin, an ancestor's, else that machine's switch) when that answered; until it does
 * (or with the owner offline), this engine's own switch.
 */
export function approvalsOff(effective: { session: boolean } | undefined, own: { global: boolean; session: boolean } | null): boolean {
  return effective ? !effective.session : own !== null && (!own.global || !own.session)
}
