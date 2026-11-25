// §8.6 planSend / §2.7 commitSend, pure over a Board (the handle supplies the view and window geometry).
import * as Y from 'yjs'
import { cluster, freshStrokes, type Cluster } from '../ink/marks'
import type { PlannedImage, PlannedMark, PointDescription, SendPlan } from './api'
import { atLeast, cardBox, pad } from './geom'
import { ulid } from './ids'
import { LOCAL, type Board, type Box, type Card, type Id, type Region } from './model'
import { chunked } from './ops'

/** A mark under this share of its card's area gets a second, zoomed image. */
export const ZOOM_SHARE = 0.15
export const FREE_PAD = 24
export const FREE_MIN = 160
/** Card chrome geometry the captions are computed against (kept in step with cards/CardShell.tsx). */
export const CARD_HEADER = 30
export const EXCERPT_ROW = 18
export const EXCERPT_PAD = 8

/** A page caption kept per stroke at stroke end: the answer and where it was asked (card-local px). */
export type Caption = PointDescription & { at: [number, number] }

export type PlanInput = {
  /** The view in world units. */
  view: Box
  offScreen?: boolean
  /** stroke id → page caption */
  captions?: ReadonlyMap<Id, Caption>
  /** Window rect of a world box when it is fully on screen, else null. */
  windowRect?(world: Box): DOMRect | null
}

const centre = (b: Box): [number, number] => [b[0] + b[2] / 2, b[1] + b[3] / 2]

function regionOf(c: Cluster, card: Card | undefined, captions?: ReadonlyMap<Id, Caption>): Region | undefined {
  if (!card) return undefined
  if (card.kind === 'page' && captions) {
    const [mx, my] = centre(c.bb)
    let best: Caption | null = null
    let dist = Infinity
    for (const s of c.strokes) {
      const cap = captions.get(s)
      if (!cap) continue
      const d = Math.hypot(cap.at[0] - mx, cap.at[1] - my)
      if (d < dist) {
        dist = d
        best = cap
      }
    }
    return best ? { kind: 'page', label: best.label, selector: best.selector, role: best.role, rect: best.rect } : undefined
  }
  if (card.kind === 'excerpt') {
    const v = card.versions[card.versions.length - 1]?.source
    if (v?.t !== 'excerpt') return undefined
    const top = c.bb[1] - CARD_HEADER - EXCERPT_PAD
    const lineOf = (y: number) => Math.min(v.end, Math.max(v.start, v.start + Math.floor(y / EXCERPT_ROW)))
    const a = lineOf(top)
    const b = lineOf(top + c.bb[3])
    return { kind: 'lines', label: a === b ? `line ${a}` : `lines ${a}–${b}`, lines: [a, b] }
  }
  return undefined
}

export function planSend(board: Board, o: PlanInput): SendPlan {
  const fresh = freshStrokes(board, o.view)
  const take = o.offScreen ? [...fresh.inView, ...fresh.offScreen] : fresh.inView
  const clusters = cluster(board, take)
  const offScreen = o.offScreen ? 0 : cluster(board, fresh.offScreen).length
  const rect = (b: Box) => o.windowRect?.(b) ?? null

  const marks: PlannedMark[] = []
  const worldOf = new Map<Id, Box>()
  const byCard = new Map<Id, Id[]>()
  const free: Id[] = []
  for (const c of clusters) {
    const id = ulid(c.at)
    const card = c.card ? board.cards[c.card] : undefined
    const m: PlannedMark = { id, strokes: c.strokes, bb: c.bb }
    if (c.card) {
      m.card = c.card
      m.ver = c.ver ?? 0
    }
    const region = regionOf(c, card, o.captions)
    if (region) m.region = region
    marks.push(m)
    worldOf.set(id, c.world)
    if (c.card) byCard.set(c.card, [...(byCard.get(c.card) ?? []), id])
    else free.push(id)
  }

  const images: PlannedImage[] = []
  for (const [cid, ids] of byCard) {
    const card = board.cards[cid]!
    const world = cardBox(card)
    images.push({ card: cid, marks: ids, rect: rect(world), world })
    for (const id of ids) {
      const w = worldOf.get(id)!
      if (w[2] * w[3] < ZOOM_SHARE * card.w * card.h) {
        const zw = atLeast(pad(w, FREE_PAD), FREE_MIN, FREE_MIN)
        images.push({ card: cid, marks: [id], rect: rect(zw), world: zw, zoomOf: id })
      }
    }
  }
  for (const id of free) {
    const world = atLeast(pad(worldOf.get(id)!, FREE_PAD), FREE_MIN, FREE_MIN)
    images.push({ card: null, marks: [id], rect: rect(world), world })
  }
  return { marks, images, offScreen }
}

/**
 * §2.7 at send, as `wb-local` bookkeeping (chunked under the update cap): every `sent` mark → `answered`, then the
 * plan's marks are written as `sent`.
 */
export function commitSend(doc: Y.Doc, board: Board, plan: SendPlan, extra: { to: string | null; hw?: Record<Id, string>; blobs?: Id[]; now?: number }): void {
  const marks = doc.getMap<unknown>('marks')
  const now = extra.now ?? Date.now()
  const blobs = new Set(extra.blobs ?? [])
  const ops: (() => void)[] = []
  for (const m of Object.values(board.marks)) {
    if (m.state !== 'sent') continue
    ops.push(() => {
      const e = marks.get(m.id)
      if (e instanceof Y.Map) e.set('state', 'answered')
    })
  }
  for (const m of plan.marks) {
    ops.push(() => {
      const e = new Y.Map<unknown>()
      e.set('strokes', [...m.strokes])
      if (m.card !== undefined) {
        e.set('card', m.card)
        e.set('ver', m.ver ?? 0)
      }
      e.set('bb', [...m.bb])
      e.set('state', 'sent')
      if (m.region) e.set('region', JSON.parse(JSON.stringify(m.region)))
      const hw = extra.hw?.[m.id]
      if (hw) e.set('hw', hw.replace(/[\n\r[\]<>]/g, ' ').slice(0, 200))
      e.set('sent_at', now)
      if (extra.to) e.set('to', [extra.to])
      if (blobs.has(m.id)) e.set('blob', true)
      e.set('by', 'owner')
      marks.set(m.id, e)
    })
  }
  chunked(doc, LOCAL, ops)
}
