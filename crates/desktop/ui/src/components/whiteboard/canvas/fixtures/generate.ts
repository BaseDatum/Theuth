// Load fixtures for the gate's screenshots and for perf checks (contract §9 B3): a 100-card whiteboard and 2 000
// strokes, deterministic for a seed. Writes go through the same shapes as `ops` (chunked, so each update stays small).
import * as Y from 'yjs'
import { pack, type Pt } from '../../ink/pack'
import { strokeValue } from '../../ink/stroke'
import { LOCAL, type CardKind, type InkColour, type InkTool, type Stroke } from '../model'
import { chunked } from '../ops'

/** mulberry32 */
export function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const CROCK = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
/** A deterministic ULID-shaped id. */
export function fakeUlid(rand: () => number, ms: number): string {
  let s = ''
  let t = ms
  for (let i = 0; i < 10; i++) {
    s = CROCK[t % 32] + s
    t = Math.floor(t / 32)
  }
  for (let i = 0; i < 16; i++) s += CROCK[Math.floor(rand() * 32)]
  return s
}

const KINDS: CardKind[] = ['sticky', 'diagram', 'excerpt', 'sticky', 'page', 'image']
const STICKY = ['Triage with one thumb', 'Never lose a decision', 'Group only when > 20 items', 'Swipe to snooze', 'Undo everywhere']

/** 100 cards in 10 rows inside 2 frames + loose, a few links. */
export function hundredCards(doc: Y.Doc, seed = 218): string[] {
  const rand = rng(seed)
  const ids: string[] = []
  const ops: (() => void)[] = []
  const t0 = 1795570000000
  const frames = [fakeUlid(rand, t0), fakeUlid(rand, t0 + 1)]
  ops.push(() => doc.getMap<unknown>('meta').set('v', 1))
  frames.forEach((id, i) => ops.push(() => {
    const m = new Y.Map<unknown>()
    for (const [k, v] of Object.entries({ title: i ? 'Directions' : 'Variants', x: -40, y: i * 1400 - 60, w: 2400, h: 1300, colour: i ? 'blue' : 'neutral', by: 'owner' })) m.set(k, v)
    doc.getMap<unknown>('frames').set(id, m)
  }))
  for (let i = 0; i < 100; i++) {
    const id = fakeUlid(rand, t0 + 10 + i)
    ids.push(id)
    const kind = KINDS[i % KINDS.length]!
    const row = Math.floor(i / 10)
    const col = i % 10
    const w = kind === 'page' ? 180 : kind === 'sticky' ? 220 : 300
    const h = kind === 'page' ? 390 : kind === 'sticky' ? 150 : 220
    const source =
      kind === 'sticky' ? { t: 'sticky', text: `- ${STICKY[i % STICKY.length]}\n- item ${i}` }
      : kind === 'diagram' ? { t: 'diagram', lang: 'mermaid', code: `graph LR\n  a${i} --> b${i}` }
      : kind === 'excerpt' ? { t: 'excerpt', path: `src/file${i}.ts`, start: 1, end: 12, text: Array.from({ length: 12 }, (_, k) => `const line${k} = ${k * i}`).join('\n'), lang: 'ts' }
      : kind === 'page' ? { t: 'page', session: '01a20fc8-92a6-73e6-a622-3ac9e51ae923', call_id: `toolu_fixture_${i}` }
      : { t: 'image', path: `/tmp/fixture-${i}.png` }
    const frame = row < 3 ? frames[0] : row < 6 ? frames[1] : undefined
    ops.push(() => {
      const m = new Y.Map<unknown>()
      const fields: Record<string, unknown> = {
        kind, x: col * 240, y: row * 460 + (row >= 3 ? 220 : 0), w, h, z: i + 1, title: `${kind} ${i + 1}`, state: i % 17 === 5 ? 'dimmed' : 'live', by: 'owner',
      }
      if (i % 17 === 5) fields.tag = 'rejected: too busy'
      if (frame) fields.frame = frame
      if (kind === 'page') fields.viewport = { w: 390, h: 844, preset: 'phone' }
      for (const [k, v] of Object.entries(fields)) m.set(k, v)
      const versions = new Y.Array<unknown>()
      m.set('versions', versions)
      doc.getMap<unknown>('cards').set(id, m)
      versions.push([{ source, at: t0 + i, by: 'owner' }])
    })
  }
  for (let i = 0; i < 20; i++) {
    const from = ids[i * 5]!
    const to = ids[i * 5 + 1]!
    ops.push(() => {
      const m = new Y.Map<unknown>()
      for (const [k, v] of Object.entries({ from, to, style: i % 3 ? 'solid' : 'dashed', arrow: 'end', by: 'owner', label: i % 2 ? 'refines' : undefined })) if (v !== undefined) m.set(k, v)
      doc.getMap<unknown>('links').set(fakeUlid(rand, t0 + 500 + i), m)
    })
  }
  chunked(doc, LOCAL, ops)
  return ids
}

const TOOLS: InkTool[] = ['pen', 'pen', 'monoline', 'marker']
const COLOURS: InkColour[] = ['ink', 'red', 'blue', 'ink', 'green']

/** `n` free hand-like strokes (40–150 points each) scattered over `area`. */
export function strokes(doc: Y.Doc, n = 2000, seed = 218, area: [number, number, number, number] = [-200, -200, 2800, 4600]): number {
  const rand = rng(seed)
  const t0 = 1795571000000
  const ops: (() => void)[] = []
  let bytes = 0
  for (let i = 0; i < n; i++) {
    const count = 40 + Math.floor(rand() * 110)
    let x = area[0] + rand() * area[2]
    let y = area[1] + rand() * area[3]
    let a = rand() * Math.PI * 2
    const pts: Pt[] = []
    for (let k = 0; k < count; k++) {
      a += (rand() - 0.5) * 0.6
      x += Math.cos(a) * 2.5
      y += Math.sin(a) * 2.5
      pts.push([x, y, 0.3 + rand() * 0.6, k * 8])
    }
    const o: [number, number] = [pts[0]![0], pts[0]![1]]
    const packed = pack(o, pts)
    bytes += packed.length
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity
    for (const [px, py] of pts) {
      x0 = Math.min(x0, px); y0 = Math.min(y0, py); x1 = Math.max(x1, px); y1 = Math.max(y1, py)
    }
    const tool = TOOLS[i % TOOLS.length]!
    const width = tool === 'marker' ? 6 : 2.8
    const s: Stroke = {
      id: fakeUlid(rand, t0 + i), pts: packed, n: packed.length / 6, o, bb: [x0 - width / 2, y0 - width / 2, x1 - x0 + width, y1 - y0 + width],
      tool, colour: COLOURS[i % COLOURS.length]!, width, p: 1, at: t0 + i * 900, by: 'owner',
    }
    ops.push(() => doc.getMap<unknown>('ink').set(s.id, strokeValue(s)))
  }
  chunked(doc, LOCAL, ops)
  return bytes
}
