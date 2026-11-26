import type * as Y from 'yjs'

export const WB_SCHEMA = 1
export const MAPS = ['meta', 'cards', 'links', 'frames', 'ink', 'marks', 'aliases'] as const
export const OWNER = 'wb-owner'   // local, undoable
export const LOCAL = 'wb-local'   // local, not undoable
export const ENGINE = 'engine'    // provider.ts#L16 literal

export type Id = string
export type By = 'owner' | `session:${string}`
export type CardKind = 'page' | 'diagram' | 'note' | 'excerpt' | 'diff' | 'terminal' | 'web' | 'image' | 'sticky' | 'session' | 'view'
export type Source =
  | { t: 'page'; session: string; call_id: string }
  | { t: 'diagram'; lang: 'json-render' | 'openui' | 'mermaid'; code: string }
  | { t: 'note'; doc: string }
  | { t: 'excerpt'; path: string; start: number; end: number; text: string; lang?: string }
  | { t: 'diff'; session: string; spec: unknown; frozen?: { session: string; call_id: string }; as_of?: number }
  | { t: 'terminal'; session: string; job: string }
  | { t: 'web'; url: string }
  | { t: 'image'; session?: string; call_id?: string; path?: string; blob?: string }
  | { t: 'sticky'; text: string }
  | { t: 'session'; session: string }
  /** A live app view: an app route (`/tasks/graph?project=…`) rendered inside the card. */
  | { t: 'view'; path: string }
export type Version = { source: Source; at: number; by: By; note?: string }
export type DevicePreset = 'phone' | 'tablet' | 'desktop' | 'auto' | 'custom'
export type DeviceViewport = { w: number; h: number; preset: DevicePreset }
export const PRESETS = { phone: [390, 844], tablet: [820, 1180], desktop: [1280, 800] } as const
export type CardState = 'live' | 'dimmed' | 'archived'
export type Card = {
  id: Id; kind: CardKind; x: number; y: number; w: number; h: number; z: number; title: string
  versions: Version[]; viewport?: DeviceViewport; state: CardState; tag?: string; frame?: Id
  placeholder?: string; snap?: { v: number; at: number; theme: 'light' | 'dark' }; pin?: boolean; by: By
  /** Size pending: the owner's client sizes the card to its content (fit.ts), then clears it. */
  fit?: boolean
  /** Sized explicitly (an agent's w/h, or the owner resized it): never sized to content on its own. */
  sized?: boolean
  /** Content scale (< 1) when the content is larger than the biggest card size-to-content makes. */
  scale?: number
}
export type Port = 'top' | 'right' | 'bottom' | 'left'
export type Link = { id: Id; from: Id; to: Id; from_port?: Port; to_port?: Port; label?: string; style: 'solid' | 'dashed'; arrow: 'end' | 'both' | 'none'; by: By }
export type FrameColour = 'neutral' | 'blue' | 'green' | 'amber' | 'red' | 'violet'
export type Frame = { id: Id; title: string; x: number; y: number; w: number; h: number; colour: FrameColour; preset?: 'phone' | 'tablet' | 'desktop'; by: By }
export type InkTool = 'monoline' | 'pen' | 'marker'
export type InkColour = 'ink' | 'red' | 'blue' | 'green' | 'amber' | 'violet' | `#${string}`
export type Box = [x: number, y: number, w: number, h: number]
export type Stroke = {
  id: Id; pts: Uint8Array; n: number; o: [number, number]; bb: Box; tool: InkTool; colour: InkColour; width: number; p: 0 | 1
  card?: Id; ver?: number; cs?: [number, number]; at: number; by: By
}
export type MarkState = 'sent' | 'answered' | 'pinned'
export type Region = { kind: 'page' | 'lines' | 'hunk' | 'text'; label: string; selector?: string; role?: string; rect?: Box; lines?: [number, number] }
export type Mark = { id: Id; strokes: Id[]; card?: Id; ver?: number; bb: Box; state: MarkState; region?: Region; hw?: string; sent_at: number; to?: string[]; blob?: boolean; baked?: boolean; by: By }
export type Meta = { v: number; created: number; bg: 'dots' | 'paper' | 'grid' | 'none'; focus?: { target: Id; spotlight: boolean; at: number; by: By } }
export type InkLife = 'fresh' | 'sent' | 'answered' | 'pinned'
/** A plain, immutable read of the whole document; rebuilt (structurally shared) on every Y change. */
export type Board = {
  meta: Meta; cards: Record<Id, Card>; links: Record<Id, Link>; frames: Record<Id, Frame>
  ink: Record<Id, Stroke>; marks: Record<Id, Mark>; aliases: Record<Id, string>
  /** stroke id → mark id (reverse index of marks[].strokes). */ markOf: Record<Id, Id>
}

// ---- tolerant readers (§2: unknown keys/enums ignored, unparseable entries skipped, never deleted) ----

type Raw = Record<string, unknown>

const CARD_KINDS: readonly CardKind[] = ['page', 'diagram', 'note', 'excerpt', 'diff', 'terminal', 'web', 'image', 'sticky', 'session', 'view']
const CARD_STATES: readonly CardState[] = ['live', 'dimmed', 'archived']
const PORTS: readonly Port[] = ['top', 'right', 'bottom', 'left']
const FRAME_COLOURS: readonly FrameColour[] = ['neutral', 'blue', 'green', 'amber', 'red', 'violet']
const TOOLS: readonly InkTool[] = ['monoline', 'pen', 'marker']
const NAMED_COLOURS = ['ink', 'red', 'blue', 'green', 'amber', 'violet'] as const
const MARK_STATES: readonly MarkState[] = ['sent', 'answered', 'pinned']
const BGS: readonly Meta['bg'][] = ['dots', 'paper', 'grid', 'none']
const REGION_KINDS: readonly Region['kind'][] = ['page', 'lines', 'hunk', 'text']

const num = (v: unknown): number | undefined =>
  typeof v === 'number' && Number.isFinite(v) ? v : typeof v === 'bigint' ? Number(v) : undefined
const str = (v: unknown): string | undefined => (typeof v === 'string' ? v : undefined)
const oneOf = <T extends string>(v: unknown, set: readonly T[]): T | undefined =>
  typeof v === 'string' && (set as readonly string[]).includes(v) ? (v as T) : undefined
const by = (v: unknown): By => (typeof v === 'string' && (v === 'owner' || v.startsWith('session:')) ? (v as By) : 'owner')
const box = (v: unknown): Box | undefined => {
  if (!Array.isArray(v) || v.length !== 4) return undefined
  const n = v.map(num)
  return n.every((x) => x !== undefined) ? (n as Box) : undefined
}
const pair = (v: unknown): [number, number] | undefined => {
  if (!Array.isArray(v) || v.length !== 2) return undefined
  const a = num(v[0])
  const b = num(v[1])
  return a === undefined || b === undefined ? undefined : [a, b]
}
const colour = (v: unknown): InkColour | undefined =>
  typeof v === 'string' && ((NAMED_COLOURS as readonly string[]).includes(v) || /^#[0-9a-fA-F]{6}$/.test(v)) ? (v as InkColour) : undefined

/** A nested Y.Map entry as a plain object; plain-object values (a peer that wrote the whole entry as one value) are accepted too. */
function fields(v: unknown): Raw | null {
  if (v && typeof v === 'object') {
    const m = v as { toJSON?: () => unknown; _map?: unknown }
    if (typeof m.toJSON === 'function' && m._map instanceof Map) {
      const out: Raw = {}
      for (const [k, x] of (v as Y.Map<unknown>).entries()) out[k] = x
      return out
    }
    if (!Array.isArray(v) && !(v instanceof Uint8Array)) return v as Raw
  }
  return null
}

function readSource(v: unknown, kind: CardKind): Source | null {
  const s = fields(v)
  if (!s || s.t !== kind) return null
  switch (kind) {
    case 'page': return str(s.call_id) !== undefined ? { t: 'page', session: str(s.session) ?? '', call_id: s.call_id as string } : null
    case 'diagram': {
      const lang = oneOf(s.lang, ['json-render', 'openui', 'mermaid'] as const)
      return lang && str(s.code) !== undefined ? { t: 'diagram', lang, code: s.code as string } : null
    }
    case 'note': return str(s.doc) !== undefined ? { t: 'note', doc: s.doc as string } : null
    case 'excerpt': {
      const start = num(s.start)
      const end = num(s.end)
      if (str(s.path) === undefined || start === undefined || end === undefined || str(s.text) === undefined) return null
      const out: Source = { t: 'excerpt', path: s.path as string, start, end, text: s.text as string }
      if (str(s.lang) !== undefined) out.lang = s.lang as string
      return out
    }
    case 'diff': {
      const out: Extract<Source, { t: 'diff' }> = { t: 'diff', session: str(s.session) ?? '', spec: s.spec }
      const fr = fields(s.frozen)
      if (fr && str(fr.session) !== undefined && str(fr.call_id) !== undefined) out.frozen = { session: fr.session as string, call_id: fr.call_id as string }
      const asOf = num(s.as_of)
      if (asOf !== undefined) out.as_of = asOf
      return out
    }
    case 'terminal': return str(s.job) !== undefined ? { t: 'terminal', session: str(s.session) ?? '', job: s.job as string } : null
    case 'web': return str(s.url) !== undefined ? { t: 'web', url: s.url as string } : null
    case 'image': {
      const out: Extract<Source, { t: 'image' }> = { t: 'image' }
      for (const k of ['session', 'call_id', 'path', 'blob'] as const) if (str(s[k]) !== undefined) out[k] = s[k] as string
      return out.call_id !== undefined || out.path !== undefined || out.blob !== undefined ? out : null
    }
    case 'sticky': return str(s.text) !== undefined ? { t: 'sticky', text: s.text as string } : null
    case 'session': return str(s.session) !== undefined ? { t: 'session', session: s.session as string } : null
    case 'view': return str(s.path) !== undefined ? { t: 'view', path: s.path as string } : null
  }
}

function readVersions(v: unknown, kind: CardKind): Version[] {
  const list: unknown[] = Array.isArray(v) ? v : v && typeof (v as Y.Array<unknown>).toArray === 'function' ? (v as Y.Array<unknown>).toArray() : []
  const out: Version[] = []
  for (const x of list) {
    const r = fields(x)
    if (!r) continue
    const source = readSource(r.source, kind)
    if (!source) continue
    const ver: Version = { source, at: num(r.at) ?? 0, by: by(r.by) }
    if (str(r.note) !== undefined) ver.note = r.note as string
    out.push(ver)
  }
  return out
}

export function readCard(id: Id, v: unknown): Card | null {
  const r = fields(v)
  if (!r) return null
  const kind = oneOf(r.kind, CARD_KINDS)
  const x = num(r.x), y = num(r.y), w = num(r.w), h = num(r.h)
  if (!kind || x === undefined || y === undefined || w === undefined || h === undefined) return null
  const card: Card = {
    id, kind, x, y, w, h, z: num(r.z) ?? 0, title: str(r.title) ?? '',
    versions: readVersions(r.versions, kind), state: oneOf(r.state, CARD_STATES) ?? 'live', by: by(r.by),
  }
  const vp = fields(r.viewport)
  if (vp) {
    const vw = num(vp.w), vh = num(vp.h)
    const preset = oneOf(vp.preset, ['phone', 'tablet', 'desktop', 'auto', 'custom'] as const)
    if (vw !== undefined && vh !== undefined && preset) card.viewport = { w: vw, h: vh, preset }
  }
  if (str(r.tag) !== undefined) card.tag = r.tag as string
  if (str(r.frame) !== undefined) card.frame = r.frame as string
  if (str(r.placeholder) !== undefined) card.placeholder = r.placeholder as string
  const snap = fields(r.snap)
  if (snap) {
    const sv = num(snap.v), at = num(snap.at)
    const theme = oneOf(snap.theme, ['light', 'dark'] as const)
    if (sv !== undefined && at !== undefined && theme) card.snap = { v: sv, at, theme }
  }
  if (r.pin === true) card.pin = true
  if (r.fit === true) card.fit = true
  if (r.sized === true) card.sized = true
  const scale = num(r.scale)
  if (scale !== undefined && scale > 0 && scale < 1) card.scale = scale
  return card
}

export function readLink(id: Id, v: unknown): Link | null {
  const r = fields(v)
  if (!r || str(r.from) === undefined || str(r.to) === undefined) return null
  const link: Link = {
    id, from: r.from as string, to: r.to as string,
    style: oneOf(r.style, ['solid', 'dashed'] as const) ?? 'solid',
    arrow: oneOf(r.arrow, ['end', 'both', 'none'] as const) ?? 'end',
    by: by(r.by),
  }
  const fp = oneOf(r.from_port, PORTS)
  const tp = oneOf(r.to_port, PORTS)
  if (fp) link.from_port = fp
  if (tp) link.to_port = tp
  if (str(r.label) !== undefined) link.label = r.label as string
  return link
}

export function readFrame(id: Id, v: unknown): Frame | null {
  const r = fields(v)
  if (!r) return null
  const x = num(r.x), y = num(r.y), w = num(r.w), h = num(r.h)
  if (x === undefined || y === undefined || w === undefined || h === undefined) return null
  const frame: Frame = { id, title: str(r.title) ?? '', x, y, w, h, colour: oneOf(r.colour, FRAME_COLOURS) ?? 'neutral', by: by(r.by) }
  const preset = oneOf(r.preset, ['phone', 'tablet', 'desktop'] as const)
  if (preset) frame.preset = preset
  return frame
}

export function readStroke(id: Id, v: unknown): Stroke | null {
  const r = fields(v)
  if (!r || !(r.pts instanceof Uint8Array)) return null
  const o = pair(r.o)
  const bb = box(r.bb)
  const tool = oneOf(r.tool, TOOLS)
  const width = num(r.width)
  if (!o || !bb || !tool || width === undefined) return null
  const stroke: Stroke = {
    id, pts: r.pts, n: num(r.n) ?? Math.floor(r.pts.length / 6), o, bb, tool, colour: colour(r.colour) ?? 'ink', width,
    p: num(r.p) === 1 ? 1 : 0, at: num(r.at) ?? 0, by: by(r.by),
  }
  if (str(r.card) !== undefined) {
    stroke.card = r.card as string
    stroke.ver = num(r.ver) ?? 0
    const cs = pair(r.cs)
    if (cs) stroke.cs = cs
  }
  return stroke
}

function readRegion(v: unknown): Region | undefined {
  const r = fields(v)
  if (!r) return undefined
  const kind = oneOf(r.kind, REGION_KINDS)
  if (!kind || str(r.label) === undefined) return undefined
  const region: Region = { kind, label: r.label as string }
  if (str(r.selector) !== undefined) region.selector = r.selector as string
  if (str(r.role) !== undefined) region.role = r.role as string
  const rect = box(r.rect)
  if (rect) region.rect = rect
  const lines = pair(r.lines)
  if (lines) region.lines = lines
  return region
}

export function readMark(id: Id, v: unknown): Mark | null {
  const r = fields(v)
  if (!r || !Array.isArray(r.strokes)) return null
  const state = oneOf(r.state, MARK_STATES)
  const bb = box(r.bb)
  if (!state || !bb) return null
  const mark: Mark = { id, strokes: r.strokes.filter((s): s is string => typeof s === 'string'), bb, state, sent_at: num(r.sent_at) ?? 0, by: by(r.by) }
  if (str(r.card) !== undefined) {
    mark.card = r.card as string
    mark.ver = num(r.ver) ?? 0
  }
  const region = readRegion(r.region)
  if (region) mark.region = region
  if (str(r.hw) !== undefined) mark.hw = r.hw as string
  if (Array.isArray(r.to)) mark.to = r.to.filter((s): s is string => typeof s === 'string')
  if (r.blob === true) mark.blob = true
  if (r.baked === true) mark.baked = true
  return mark
}

export function readMeta(m: Y.Map<unknown>): Meta {
  const meta: Meta = { v: num(m.get('v')) ?? WB_SCHEMA, created: num(m.get('created')) ?? 0, bg: oneOf(m.get('bg'), BGS) ?? 'dots' }
  const f = fields(m.get('focus'))
  if (f && str(f.target) !== undefined) meta.focus = { target: f.target as string, spotlight: f.spotlight === true, at: num(f.at) ?? 0, by: by(f.by) }
  return meta
}

export const mapsOf = (doc: Y.Doc) => ({
  meta: doc.getMap<unknown>('meta'),
  cards: doc.getMap<unknown>('cards'),
  links: doc.getMap<unknown>('links'),
  frames: doc.getMap<unknown>('frames'),
  ink: doc.getMap<unknown>('ink'),
  marks: doc.getMap<unknown>('marks'),
  aliases: doc.getMap<unknown>('aliases'),
})

function readAll<T>(m: Y.Map<unknown>, read: (id: Id, v: unknown) => T | null): Record<Id, T> {
  const out: Record<Id, T> = {}
  for (const [k, v] of m.entries()) {
    const x = read(k, v)
    if (x) out[k] = x
  }
  return out
}

export function indexMarks(marks: Record<Id, Mark>): Record<Id, Id> {
  const markOf: Record<Id, Id> = {}
  for (const m of Object.values(marks)) for (const s of m.strokes) markOf[s] = m.id
  return markOf
}

export function readBoard(doc: Y.Doc): Board {
  const m = mapsOf(doc)
  const aliases: Record<Id, string> = {}
  for (const [k, v] of m.aliases.entries()) if (typeof v === 'string') aliases[k] = v
  const marks = readAll(m.marks, readMark)
  return {
    meta: readMeta(m.meta),
    cards: readAll(m.cards, readCard),
    links: readAll(m.links, readLink),
    frames: readAll(m.frames, readFrame),
    ink: readAll(m.ink, readStroke),
    marks,
    aliases,
    markOf: indexMarks(marks),
  }
}

export function lifeOf(board: Board, stroke: Id): InkLife {
  const mark = board.markOf[stroke]
  const m = mark ? board.marks[mark] : undefined
  return m ? m.state : 'fresh'
}

/** The version a card shows: the scrubbed one (1-based) or the current (last). */
export function versionOf(card: Card, scrub?: number): Version | undefined {
  if (scrub && scrub >= 1 && scrub <= card.versions.length) return card.versions[scrub - 1]
  return card.versions[card.versions.length - 1]
}
