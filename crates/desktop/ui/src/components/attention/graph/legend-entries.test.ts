// 208: the legend is built from the renderer's own tables — every encoding the canvas uses has an entry with the same
// colour, and nothing in the legend is invented.
import { expect, test } from 'vitest'
import { EDGE, EDGE_ORDER, KIND, MARK, MARKERS, messageKey, PULSE, pulseTone, STATUS, STATUS_ORDER, statusOf } from './encoding'
import { GRAPH_FIXTURE } from './fixture'
import { legendSections } from './legend-entries'

const entries = legendSections().flatMap((s) => s.entries)
const byId = new Map(entries.map((e) => [e.id, e]))

test('every status, line, pulse and mark the canvas draws has one legend entry in the renderer’s colour', () => {
  expect(byId.size).toBe(entries.length)
  for (const k of STATUS_ORDER) expect(byId.get(`status:${k}`)?.swatch).toEqual({ kind: 'status', glyph: STATUS[k].glyph, tone: STATUS[k].tone })
  for (const k of EDGE_ORDER) expect(byId.get(`edge:${k}`)?.swatch).toEqual({ kind: 'edge', edge: k })
  for (const k of Object.keys(PULSE) as (keyof typeof PULSE)[]) expect(byId.get(`pulse:${k}`)?.swatch).toEqual({ kind: 'pulse', tone: PULSE[k].tone })
  for (const k of Object.keys(MARK)) expect(byId.has(`mark:${k}`)).toBe(true)
  expect(byId.get('mark:ring')?.swatch).toEqual({ kind: 'ring', tone: MARK.ring.tone })
  // Every kind tag a card can show is explained.
  const tags = new Set(entries.flatMap((e) => (e.swatch.kind === 'card' && e.swatch.tag ? [e.swatch.tag] : [])))
  for (const k of Object.values(KIND)) if (k.tag && k.tag !== KIND.root.tag) expect(tags.has(k.tag)).toBe(true)
})

test('the states the canvas maps nodes and edges to are all in the legend', () => {
  for (const n of GRAPH_FIXTURE.nodes) expect(byId.has(`status:${statusOf(n)}`)).toBe(true)
  for (const a of [false, true]) for (const b of [false, true]) expect(byId.has(`edge:${messageKey(a, b)}`)).toBe(true)
  const pulses = new Set(Object.values(PULSE).map((p) => p.tone))
  for (const kind of ['sent', 'delivered', 'replied', 'result', 'failed']) for (const tree of [false, true]) expect(pulses.has(pulseTone(kind, tree))).toBe(true)
  // Each edge style's arrowhead is defined, in its own colour.
  for (const k of EDGE_ORDER) expect(MARKERS.find((m) => m.id === EDGE[k].marker)?.fill).toBe(EDGE[k].stroke)
})

test('colours are theme tokens, never literals', () => {
  const tones = [...Object.values(STATUS).map((s) => s.tone), ...Object.values(EDGE).map((e) => e.stroke), ...Object.values(PULSE).map((p) => p.tone), MARK.ring.tone, MARK.selected.tone]
  for (const t of tones) expect(t).toMatch(/^var\(--[a-z0-9-]+\)$/)
})
