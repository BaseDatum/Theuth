// Size to content: a content card placed or updated without an explicit size (`card.fit`, set by the engine's
// wb_place/wb_update or the owner's "+ Add") is sized by the owner's client to what its body renders — the full
// content, inner scroll regions (tables, code, overflow:auto) included — clamped to FIT_MIN…FIT_MAX. Content larger
// than the max is scaled down to fit (`card.scale`), never cropped. A card the owner resized (`card.sized`) is never
// sized on its own; "Fit to content" in its toolbar does it on request.
import * as Y from 'yjs'
import { FRAME_PAD } from './binding'
import { shownCards, union } from './geom'
import type { Board, Box, Card, CardKind, Id } from './model'
import { ZOOM_SURFACE } from './scroll'

export const FIT_MIN = { w: 240, h: 160 } as const
export const FIT_MAX = { w: 1600, h: 1200 } as const
/** Kinds sized to their content (the engine's `CardKind::fits_content`). */
export const FIT_KINDS: readonly CardKind[] = ['page', 'diagram', 'note', 'excerpt', 'image']
/** Clearance kept from other cards when a grown card has to move (place.rs `GAP`). */
export const FIT_GAP = 24

export const fitsContent = (c: Pick<Card, 'kind'>): boolean => FIT_KINDS.includes(c.kind)

/**
 * The card size for a body whose content is `natural` (CSS px at scale 1) under a `head` px title bar: the content at
 * full size when it fits FIT_MAX, else scaled down (keeping its aspect) until it does; never under FIT_MIN.
 */
export function fitSize(natural: { w: number; h: number }, head: number): { w: number; h: number; scale: number } {
  const nw = Math.max(1, natural.w)
  const nh = Math.max(1, natural.h)
  const scale = Math.min(1, FIT_MAX.w / nw, (FIT_MAX.h - head) / nh)
  return {
    w: Math.round(Math.max(FIT_MIN.w, nw * scale)),
    h: Math.round(Math.max(FIT_MIN.h, nh * scale + head)),
    scale: Math.round(scale * 1000) / 1000,
  }
}

/**
 * Where a card grown to `box` goes: where it is when it covers no other drawn card (with FIT_GAP clearance), else
 * the nearer of the first clear spot straight right or straight down (as placement scans). Nothing else moves.
 */
export function clearSpot(board: Board, id: Id, box: Box): { x: number; y: number } {
  const others = shownCards(board)
    .filter((c) => c.id !== id)
    .map((c): Box => [c.x - FIT_GAP, c.y - FIT_GAP, c.w + 2 * FIT_GAP, c.h + 2 * FIT_GAP])
  const hit = (x: number, y: number) => others.find((o) => x < o[0] + o[2] && o[0] < x + box[2] && y < o[1] + o[3] && o[1] < y + box[3])
  if (!hit(box[0], box[1])) return { x: box[0], y: box[1] }
  const scan = (axis: 0 | 1) => {
    let at = [box[0], box[1]] as [number, number]
    for (let i = 0; i < 500; i++) {
      const o = hit(at[0], at[1])
      if (!o) return at
      at = axis === 0 ? [o[0] + o[2], at[1]] : [at[0], o[1] + o[3]]
    }
    return at
  }
  const r = scan(0)
  const d = scan(1)
  const [x, y] = r[0] - box[0] <= d[1] - box[1] ? r : d
  return { x: Math.round(x), y: Math.round(y) }
}

/**
 * Writes a fit: the card's size (moved clear of its neighbours if it grew onto one), its content scale, `fit`
 * cleared, and its frame grown to keep it inside. `owner`: the owner's "Fit to content" (undoable; the card is
 * auto-sized again from then on, `sized` cleared); otherwise the client's own size-to-content (not undoable).
 */
export function applyFit(doc: Y.Doc, board: Board, id: Id, size: { w: number; h: number; scale: number }, origin: unknown, owner: boolean): void {
  const c = board.cards[id]
  const m = doc.getMap<unknown>('cards').get(id)
  if (!c || !(m instanceof Y.Map)) return
  const at = clearSpot(board, id, [c.x, c.y, size.w, size.h])
  const f = c.frame ? board.frames[c.frame] : undefined
  const fm = f ? doc.getMap<unknown>('frames').get(f.id) : undefined
  doc.transact(() => {
    m.set('x', at.x)
    m.set('y', at.y)
    m.set('w', size.w)
    m.set('h', size.h)
    // Pages and images scale themselves into the card; the other bodies render at 1/scale and are scaled down.
    if (size.scale < 1 && c.kind !== 'page' && c.kind !== 'image') m.set('scale', size.scale)
    else m.delete('scale')
    m.delete('fit')
    if (owner) m.delete('sized')
    if (f && fm instanceof Y.Map) {
      const g = union([f.x, f.y, f.w, f.h], [at.x - FRAME_PAD, at.y - FRAME_PAD, size.w + 2 * FRAME_PAD, size.h + 2 * FRAME_PAD])
      if (g[0] !== f.x || g[1] !== f.y || g[2] !== f.w || g[3] !== f.h) for (const [k, v] of [['x', g[0]], ['y', g[1]], ['w', g[2]], ['h', g[3]]] as const) fm.set(k, v)
    }
  }, origin)
}

/**
 * The natural size of a rendered card body: what it would take to show everything with no scrolling. An image is its
 * pixels; a diagram alone in its pan/zoom surface (mermaid, a Graph) its SVG's view box; anything else (a diagram with
 * a table under it, a note) the body's scroll size plus whatever each inner scroll container hides. Null while
 * nothing has rendered.
 */
export function measureBody(body: HTMLElement): { w: number; h: number } | null {
  const img = body.querySelector('img')
  if (img && img.naturalWidth > 0 && body.querySelectorAll('img').length === 1 && !body.querySelector('svg, table, pre')) {
    return { w: img.naturalWidth, h: img.naturalHeight }
  }
  const content = (body.firstElementChild as HTMLElement | null) ?? body
  const text = content.textContent?.trim() ?? ''
  const surfaces = content.querySelectorAll(ZOOM_SURFACE)
  if (surfaces.length === 1) {
    const svg = surfaces[0]!.querySelector<SVGSVGElement>('svg[viewBox]')
    const vb = svg?.viewBox.baseVal
    const outside = text.replace(surfaces[0]!.textContent?.trim() ?? '', '').trim()
    if (vb && vb.width > 0 && vb.height > 0 && outside === '') return { w: Math.ceil(vb.width), h: Math.ceil(vb.height) }
  }
  const svgs = content.querySelectorAll('svg')
  let w = Math.max(body.scrollWidth, content.scrollWidth)
  let h = Math.max(body.scrollHeight, content.scrollHeight)
  let extraW = 0
  let extraH = 0
  for (const el of content.querySelectorAll<HTMLElement>('*')) {
    const dx = el.scrollWidth - el.clientWidth
    const dy = el.scrollHeight - el.clientHeight
    if ((dx <= 1 && dy <= 1) || el.clientHeight === 0) continue
    const s = getComputedStyle(el)
    if (dx > 1 && /auto|scroll|hidden/.test(s.overflowX)) extraW = Math.max(extraW, dx)
    if (dy > 1 && /auto|scroll|hidden/.test(s.overflowY)) extraH += dy
  }
  w += extraW
  h += extraH
  return w > 0 && h > 0 && (text !== '' || svgs.length > 0 || img) ? { w: Math.ceil(w), h: Math.ceil(h) } : null
}
