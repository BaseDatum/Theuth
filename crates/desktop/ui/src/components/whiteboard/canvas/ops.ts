// §8.3 owner mutations. Each is one `doc.transact(…, OWNER)` (undoable); bulk writes go through `chunked` so no single
// update passes the provider's 48 KiB cap (§2.9).
import * as Y from 'yjs'
import { strokeValue } from '../ink/stroke'
import { shifted } from '../ink/anchor'
import { ulid } from './ids'
import {
  LOCAL, OWNER,
  type Board, type CardKind, type DeviceViewport, type Frame, type FrameColour, type Id, type Link, type Meta, type Port, type Source, type Stroke,
} from './model'
import { GROUP } from './undo'

/** Entry writes per transaction (§2.9). */
export const CHUNK_WRITES = 60
/** Approximate payload per transaction (§2.9). */
export const CHUNK_BYTES = 30_000
/** Kinds that edit in place: exactly one version, replaced. */
export const IN_PLACE: readonly CardKind[] = ['sticky', 'note', 'diagram', 'deck']
export const STICKY_MAX = 280

/** A plain value without `undefined` fields (Yjs plain values are JSON-like). */
function plain<T extends object>(o: T): T {
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(o)) if (v !== undefined) out[k] = v
  return out as T
}

function nested(fields: Record<string, unknown>): Y.Map<unknown> {
  const m = new Y.Map<unknown>()
  for (const [k, v] of Object.entries(fields)) if (v !== undefined) m.set(k, v)
  return m
}

/** Sets each key; `undefined` deletes it (an unset optional field is absent, never null). */
function patchMap(m: Y.Map<unknown>, patch: Record<string, unknown>) {
  for (const [k, v] of Object.entries(patch)) {
    if (v === undefined) m.delete(k)
    else m.set(k, v)
  }
}

const entry = (doc: Y.Doc, map: 'cards' | 'links' | 'frames' | 'marks', id: Id): Y.Map<unknown> | null => {
  const v = doc.getMap<unknown>(map).get(id)
  return v instanceof Y.Map ? v : null
}

function maxZ(doc: Y.Doc): number {
  let z = 0
  for (const v of doc.getMap<unknown>('cards').values()) {
    if (v instanceof Y.Map) {
      const n = v.get('z')
      if (typeof n === 'number' && n > z) z = n
    }
  }
  return z
}

let groups = 0
let held: number | null = null
const nextGroup = () => held ?? ++groups

/**
 * Makes every owner write until the returned release one undo step (an eraser drag, a lasso delete of cards and
 * ink). Each write keeps its own bounded transaction; Undo replays them all, one per transaction.
 */
export function holdGroup(): () => void {
  const g = ++groups
  held = g
  return () => {
    if (held === g) held = null
  }
}

/**
 * Runs `ops` in as few transactions as the bounds allow: ≤ 60 entry writes and ≈ 30 kB of update payload each. The
 * payload is measured as the transaction goes (the diff against the state vector taken before it).
 *
 * Undo replays a chunk as one transaction (undo.ts), so the bound must hold for the inverse too: `inverse[i]` is the
 * bytes op `i`'s inverse re-inserts (a deleted stroke's points, say), which the forward update does not show. All
 * chunks of one call share a `GROUP` id, so one Undo press replays them all, chunk by chunk (review M1).
 */
export function chunked(doc: Y.Doc, origin: unknown, ops: (() => void)[], inverse?: readonly number[]): void {
  const group = nextGroup()
  let i = 0
  while (i < ops.length) {
    const sv = Y.encodeStateVector(doc)
    doc.transact((tr) => {
      tr.meta.set(GROUP, group)
      let n = 0
      let size = 0
      let back = 0
      while (i < ops.length && n < CHUNK_WRITES) {
        ops[i]!()
        back += inverse?.[i] ?? 0
        i++
        n++
        if (i >= ops.length) break
        if (back + (inverse?.[i] ?? 0) >= CHUNK_BYTES) break
        // Stop before the next write would likely pass the bound (the last write's size as the estimate).
        const now = Y.encodeStateAsUpdate(doc, sv).length
        if (now + (now - size) >= CHUNK_BYTES) break
        size = now
      }
    }, origin)
  }
}

export type NewCard = {
  kind: CardKind; title: string; x: number; y: number; w: number; h: number
  source?: Source; viewport?: DeviceViewport; frame?: Id
  /** `fit`: sized to its content once it renders (fit.ts); `sized`: an explicit size, never fitted on its own. */
  fit?: boolean; sized?: boolean
}

export function addCard(doc: Y.Doc, c: NewCard): Id {
  const id = ulid()
  doc.transact(() => {
    const versions = new Y.Array<unknown>()
    if (c.source) versions.push([{ source: plain(c.source), at: Date.now(), by: 'owner' }])
    const m = nested({
      kind: c.kind, x: c.x, y: c.y, w: c.w, h: c.h, z: maxZ(doc) + 1, title: c.title.slice(0, 120),
      viewport: c.viewport ? { ...c.viewport } : undefined, state: 'live', frame: c.frame, by: 'owner',
      fit: c.fit || undefined, sized: c.sized || undefined,
    })
    m.set('versions', versions)
    doc.getMap<unknown>('cards').set(id, m)
  }, OWNER)
  return id
}

export type CardPatch = Partial<{
  x: number; y: number; w: number; h: number; z: number; title: string; viewport: DeviceViewport | undefined
  state: 'live' | 'dimmed' | 'archived'; tag: string | undefined; frame: Id | undefined; pin: boolean | undefined; placeholder: string | undefined
  fit: boolean | undefined; sized: boolean | undefined
}>

export function patchCard(doc: Y.Doc, id: Id, patch: CardPatch): void {
  const m = entry(doc, 'cards', id)
  if (!m) return
  doc.transact(() => patchMap(m, patch as Record<string, unknown>), OWNER)
}

/** Raise to the top of the stack. */
export function raise(doc: Y.Doc, id: Id): void {
  const m = entry(doc, 'cards', id)
  if (!m) return
  const top = maxZ(doc)
  if (m.get('z') === top) return
  doc.transact(() => m.set('z', top + 1), OWNER)
}

function writeVersion(m: Y.Map<unknown>, source: Source, note?: string) {
  let versions = m.get('versions')
  if (!(versions instanceof Y.Array)) {
    versions = new Y.Array<unknown>()
    m.set('versions', versions)
  }
  const arr = versions as Y.Array<unknown>
  const v = plain({ source: plain(source), at: Date.now(), by: 'owner', note })
  if (IN_PLACE.includes(m.get('kind') as CardKind)) {
    if (arr.length > 0) arr.delete(0, arr.length)
    arr.insert(0, [v])
  } else arr.push([v])
  m.delete('placeholder')
}

export function pushVersion(doc: Y.Doc, id: Id, source: Source, note?: string): void {
  const m = entry(doc, 'cards', id)
  if (!m || m.get('kind') !== source.t) return
  doc.transact(() => writeVersion(m, source, note), OWNER)
}

export function setSticky(doc: Y.Doc, id: Id, text: string): void {
  const m = entry(doc, 'cards', id)
  if (!m || m.get('kind') !== 'sticky') return
  doc.transact(() => writeVersion(m, { t: 'sticky', text: text.slice(0, STICKY_MAX) }), OWNER)
}

export function dim(doc: Y.Doc, ids: Id[], tag: string): void {
  chunked(doc, OWNER, ids.map((id) => () => {
    const m = entry(doc, 'cards', id)
    if (m) patchMap(m, { state: 'dimmed', tag: tag.trim() ? tag.slice(0, 120) : undefined })
  }))
}

export function archive(doc: Y.Doc, ids: Id[]): void {
  chunked(doc, OWNER, ids.map((id) => () => entry(doc, 'cards', id)?.set('state', 'archived')))
}

export function restore(doc: Y.Doc, ids: Id[]): void {
  chunked(doc, OWNER, ids.map((id) => () => {
    const m = entry(doc, 'cards', id)
    if (m) patchMap(m, { state: 'live', tag: undefined })
  }))
}

export function addLink(doc: Y.Doc, l: { from: Id; to: Id; label?: string; style?: Link['style']; arrow?: Link['arrow']; from_port?: Port; to_port?: Port }): Id {
  const id = ulid()
  doc.transact(() => {
    doc.getMap<unknown>('links').set(id, nested({
      from: l.from, to: l.to, from_port: l.from_port, to_port: l.to_port, label: l.label?.slice(0, 80) || undefined,
      style: l.style ?? 'solid', arrow: l.arrow ?? 'end', by: 'owner',
    }))
  }, OWNER)
  return id
}

export function patchLink(doc: Y.Doc, id: Id, patch: Partial<Omit<Link, 'id' | 'by'>> | Record<string, unknown>): void {
  const m = entry(doc, 'links', id)
  if (!m) return
  doc.transact(() => patchMap(m, patch as Record<string, unknown>), OWNER)
}

export function removeLink(doc: Y.Doc, id: Id): void {
  doc.transact((tr) => {
    tr.meta.set(GROUP, nextGroup()) // joins a held group (a lasso delete) as one undo step
    doc.getMap<unknown>('links').delete(id)
  }, OWNER)
}

export function addFrame(doc: Y.Doc, f: { title: string; x: number; y: number; w: number; h: number; colour?: FrameColour; preset?: Frame['preset']; cards?: Id[] }): Id {
  const id = ulid()
  doc.transact(() => {
    doc.getMap<unknown>('frames').set(id, nested({
      title: f.title.slice(0, 80), x: f.x, y: f.y, w: f.w, h: f.h, colour: f.colour ?? 'neutral', preset: f.preset, by: 'owner',
    }))
    for (const c of f.cards ?? []) entry(doc, 'cards', c)?.set('frame', id)
  }, OWNER)
  return id
}

export function patchFrame(doc: Y.Doc, id: Id, patch: Partial<Omit<Frame, 'id' | 'by'>> | Record<string, unknown>): void {
  const m = entry(doc, 'frames', id)
  if (!m) return
  doc.transact(() => patchMap(m, patch as Record<string, unknown>), OWNER)
}

/** Deletes the frame and clears `frame` on its cards; the cards stay. */
export function removeFrame(doc: Y.Doc, id: Id): void {
  const ops: (() => void)[] = [() => doc.getMap<unknown>('frames').delete(id)]
  for (const [cid, v] of doc.getMap<unknown>('cards').entries()) {
    if (v instanceof Y.Map && v.get('frame') === id) ops.push(() => entry(doc, 'cards', cid)?.delete('frame'))
  }
  chunked(doc, OWNER, ops)
}

/** A frame's new rect and the membership that follows it (binding `resizeMembership`): one undo step. */
export function resizeFrame(doc: Y.Doc, id: Id, rect: { x: number; y: number; w: number; h: number }, members: { join: Id[]; leave: Id[] }): void {
  const m = entry(doc, 'frames', id)
  if (!m) return
  const ops: (() => void)[] = [() => patchMap(m, { x: rect.x, y: rect.y, w: rect.w, h: rect.h })]
  for (const c of members.join) ops.push(() => entry(doc, 'cards', c)?.set('frame', id))
  for (const c of members.leave) ops.push(() => entry(doc, 'cards', c)?.delete('frame'))
  chunked(doc, OWNER, ops)
}

/** Owner long-press: Pin (`sent`/`answered` → `pinned`) or Unpin (→ `answered`). */
export function pinMark(doc: Y.Doc, id: Id, pinned: boolean): void {
  const m = entry(doc, 'marks', id)
  if (!m) return
  doc.transact(() => m.set('state', pinned ? 'pinned' : 'answered'), OWNER)
}

/** One stroke per transaction (§2.9), all under OWNER; the parts of one call are one undo step (one `GROUP`). */
export function addStrokes(doc: Y.Doc, strokes: Stroke[]): void {
  const ink = doc.getMap<unknown>('ink')
  const group = nextGroup()
  for (const s of strokes) {
    doc.transact((tr) => {
      tr.meta.set(GROUP, group)
      ink.set(s.id, strokeValue(s))
    }, OWNER)
  }
}

/** Bytes an erased stroke's undo re-inserts: its points plus the other fields. */
const STROKE_OVERHEAD = 200

export function eraseStrokes(doc: Y.Doc, ids: Id[]): void {
  const ink = doc.getMap<unknown>('ink')
  const back = ids.map((id) => {
    const v = ink.get(id)
    const pts = v && typeof v === 'object' && 'pts' in v && v.pts instanceof Uint8Array ? v.pts.length : 0
    return pts + STROKE_OVERHEAD
  })
  chunked(doc, OWNER, ids.map((id) => () => ink.delete(id)), back)
}

export function setBg(doc: Y.Doc, bg: Meta['bg']): void {
  doc.transact(() => doc.getMap<unknown>('meta').set('bg', bg), OWNER)
}

/** `card.snap` after a capture (bookkeeping, not undoable). */
export function setSnap(doc: Y.Doc, id: Id, snap: { v: number; at: number; theme: 'light' | 'dark' }): void {
  const m = entry(doc, 'cards', id)
  if (!m) return
  doc.transact(() => m.set('snap', { ...snap }), LOCAL)
}

export type Move = { id: Id; x: number; y: number; frame?: Id | null /* null clears, undefined keeps */ }

/**
 * A drag end (§8.4, §2.4): card positions (absolute), frame positions, membership and the free strokes that ride with a
 * moved card — one owner transaction (chunked beyond 60 writes). `board` is the state before the move.
 */
export function moveItems(doc: Y.Doc, board: Board, moves: { cards: Move[]; frames: { id: Id; x: number; y: number }[]; riders: Stroke[] }): void {
  const ops: (() => void)[] = []
  for (const f of moves.frames) ops.push(() => {
    const m = entry(doc, 'frames', f.id)
    if (m) patchMap(m, { x: f.x, y: f.y })
  })
  const delta = new Map<Id, [number, number]>()
  for (const c of moves.cards) {
    const before = board.cards[c.id]
    if (before) delta.set(c.id, [c.x - before.x, c.y - before.y])
    ops.push(() => {
      const m = entry(doc, 'cards', c.id)
      if (!m) return
      patchMap(m, { x: c.x, y: c.y })
      if (c.frame === null) m.delete('frame')
      else if (c.frame !== undefined) m.set('frame', c.frame)
    })
  }
  const ink = doc.getMap<unknown>('ink')
  for (const s of moves.riders) {
    // A rider touches exactly one card: the one whose delta applies.
    const d = [...delta.entries()].find(([id]) => {
      const c = board.cards[id]
      return c && s.bb[0] <= c.x + c.w && c.x <= s.bb[0] + s.bb[2] && s.bb[1] <= c.y + c.h && c.y <= s.bb[1] + s.bb[3]
    })?.[1]
    if (!d || (d[0] === 0 && d[1] === 0)) continue
    ops.push(() => ink.set(s.id, strokeValue(shifted(s, d[0], d[1]))))
  }
  chunked(doc, OWNER, ops)
}
