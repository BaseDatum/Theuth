// §8.5 level of detail: live ≥ 240 px on screen and within the view ± one screen; snapshot otherwise when one exists;
// chip below 60 px or with no snapshot. At most `budget` live page/terminal cards; a live page nobody entered for 30 s
// is demoted to its snapshot unless pinned. `web` is never live on the canvas (§12.6).
import { intersects } from './geom'
import type { Box, Card, CardKind, Id } from './model'

export type Lod = 'live' | 'snapshot' | 'chip'

export const LIVE_PX = 240
export const CHIP_PX = 60
export const IDLE_MS = 30_000
/** Kinds whose live view is costly (an iframe or a terminal) and therefore budgeted. */
export const BUDGETED: readonly CardKind[] = ['page', 'terminal']

/** The view grown by one screen on every side. */
export const reach = (view: Box): Box => [view[0] - view[2], view[1] - view[3], view[2] * 3, view[3] * 3]

/** The LOD a card would have on its own, before the budget. */
export function naturalLod(card: Pick<Card, 'kind' | 'x' | 'y' | 'w' | 'h' | 'snap' | 'placeholder'>, zoom: number, view: Box): Lod {
  const px = card.w * zoom
  const hasSnap = card.snap !== undefined
  if (card.placeholder !== undefined) return 'live' // the shimmer is cheap
  if (px < CHIP_PX) return 'chip'
  if (card.kind !== 'web' && px >= LIVE_PX && intersects([card.x, card.y, card.w, card.h], reach(view))) return 'live'
  return hasSnap ? 'snapshot' : 'chip'
}

export type LodState = {
  /** card → ms when it last became live or was entered */
  since: Record<Id, number>
  /** pages demoted for idling; cleared when entered or when they leave the live zone */
  demoted: Record<Id, true>
}

export const emptyLodState = (): LodState => ({ since: {}, demoted: {} })

/**
 * LODs for all cards with the budget applied. Candidates for a budgeted live slot are ranked: the entered card, pinned
 * cards, then the largest on screen. Returns the LODs and the next state (pure).
 */
export function assignLod(
  cards: readonly Card[],
  o: { zoom: number; view: Box; budget: number; entered: Id | null; now: number; state: LodState },
): { lod: Record<Id, Lod>; state: LodState } {
  const lod: Record<Id, Lod> = {}
  const since: Record<Id, number> = {}
  const demoted: Record<Id, true> = {}
  const fallback = (c: Card): Lod => (c.snap ? 'snapshot' : 'chip')
  const budgeted: Card[] = []
  for (const c of cards) {
    const n = naturalLod(c, o.zoom, o.view)
    if (n === 'live' && BUDGETED.includes(c.kind) && c.placeholder === undefined) budgeted.push(c)
    else lod[c.id] = n
  }
  const rank = (c: Card) => (c.id === o.entered ? 3e12 : 0) + (c.pin ? 2e12 : 0) + c.w * c.h
  budgeted.sort((a, b) => rank(b) - rank(a))
  let used = 0
  for (const c of budgeted) {
    const entered = c.id === o.entered
    const start = entered ? o.now : (o.state.since[c.id] ?? o.now)
    const idle = !entered && !c.pin && c.kind === 'page' && c.snap !== undefined && o.now - start >= IDLE_MS
    if (!entered && (o.state.demoted[c.id] || idle)) {
      demoted[c.id] = true
      lod[c.id] = fallback(c)
      continue
    }
    if (used >= o.budget && !entered) {
      lod[c.id] = fallback(c)
      continue
    }
    used++
    lod[c.id] = 'live'
    since[c.id] = start
  }
  return { lod, state: { since, demoted } }
}
