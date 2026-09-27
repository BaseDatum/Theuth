import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import { parseAnchor } from '@/components/chat/surface/anchor'
import { type Anchor, formatAnchor } from './anchors'
import { type Diagram, type DocBlock, classifyCode } from './blocks'
import { boardStops, diagramStops, highlightOf, unitBy, unitsOf } from './units'

const a = (s: string): Anchor => {
  const r = parseAnchor(s)
  if (typeof r === 'string') throw new Error(r)
  return r
}

/** The fixture board's two diagrams (a 6-node `Graph`, a 7-step `Sequence`). */
const FIXTURE = readFileSync(new URL('./fixtures/surface-fence-parsing.md', import.meta.url), 'utf8')
const fences = [...FIXTURE.matchAll(/```json-render\n([\s\S]*?)\n```/g)].map((m) => m[1]!)
const codeBlock = (id: string, language: string, text: string): DocBlock =>
  ({ id, type: 'codeBlock', props: { language }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock
const diagram = (i: number): Diagram => {
  const c = classifyCode(codeBlock(`b${i}`, 'json-render', fences[i]!) as Extract<DocBlock, { type: 'codeBlock' }>)
  if (c.kind !== 'diagram') throw new Error(c.kind)
  return c.diagram
}
const GRAPH = diagram(0)
const SEQ = diagram(1)

test('units come in authored order: Graph nodes by id, Sequence steps by index with `from → to`', () => {
  const g = unitsOf(GRAPH)
  expect(g.map((u) => u.id)).toEqual(['lib', 'fences', 'fence', 'opening', 'closing', 'unindented'])
  expect(g.every((u) => u.anchor && u.detail === null)).toBe(true)
  expect(formatAnchor(g[4]!.anchor!)).toBe('crates/surface/src/fences.rs#L89-L94')
  const s = unitsOf(SEQ)
  expect(s.map((u) => u.id)).toEqual(['0', '1', '2', '3', '4', '5', '6'])
  expect(s[0]).toMatchObject({ label: 'text', detail: 'caller → fences()' })
  expect(s[1]!.detail).toBe('fences() → fences()')
})

test('highlight = the units whose anchor overlaps the active one (M4); a bare path lights the whole file', () => {
  expect(highlightOf(unitsOf(GRAPH), a('crates/surface/src/fences.rs#L90-L94'))).toEqual(['closing'])
  expect(highlightOf(unitsOf(SEQ), a('crates/surface/src/fences.rs#L90-L94'))).toEqual(['5'])
  expect(highlightOf(unitsOf(SEQ), a('crates/surface/src/fences.rs#L38'))).toEqual(['4'])
  expect(highlightOf(unitsOf(GRAPH), a('crates/surface/src/fences.rs'))).toEqual(['fences', 'fence', 'opening', 'closing', 'unindented'])
  expect(highlightOf(unitsOf(GRAPH), a('crates/surface/src/lib.rs#L1'))).toEqual([])
  expect(highlightOf(unitsOf(GRAPH), null)).toEqual([])
})

test('a diagram tour has one stop per unit; a unit without a ref (or with a bad one) is an explanation stop', () => {
  const d: Diagram = {
    type: 'Graph',
    props: {
      nodes: [
        { id: 'a', label: 'A', ref: 'x.rs#L1-L2' },
        { id: 'b', label: 'B' },
        { id: 'c', label: 'C', ref: '../x.rs' },
      ],
      edges: [],
    },
  }
  const stops = diagramStops(d, 'blk')
  expect(stops.map((s) => [s.label, s.anchor && formatAnchor(s.anchor), s.stage])).toEqual([
    ['A', 'x.rs#L1-L2', { blockId: 'blk', unit: 'a' }],
    ['B', null, { blockId: 'blk', unit: 'b' }],
    ['C', null, { blockId: 'blk', unit: 'c' }],
  ])
  expect(diagramStops(SEQ, 's')).toHaveLength(7)
  expect(unitBy('blk', '3')).toBe('unit:blk:3')
})

const para = (id: string, ...runs: (string | [string, string])[]): DocBlock =>
  ({
    id,
    type: 'paragraph',
    props: {},
    content: runs.map((r) => (typeof r === 'string' ? { type: 'text', text: r, styles: {} } : { type: 'link', href: r[1], content: [{ type: 'text', text: r[0], styles: {} }] })),
    children: [],
  }) as unknown as DocBlock
const heading = (id: string, level: number, text: string): DocBlock =>
  ({ id, type: 'heading', props: { level }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock

test('Tour this board: every anchored element in document order; stage = the section diagram, else the covering excerpt', () => {
  const blocks = [
    heading('h1', 1, 'Title'),
    heading('s1', 2, 'Map'),
    para('p1', 'see ', ['fences()', 'code:crates/surface/src/fences.rs#L24-L65'], ' and ', ['docs', 'https://example.com']),
    codeBlock('g', 'json-render', fences[0]!), // the Graph: 6 unit stops
    heading('s2', 2, 'How it works'),
    para('p2', ['closing()', 'code:crates/surface/src/fences.rs#L90-L94'], ' matches'),
    codeBlock('e', 'json-render', fences[3]!), // Excerpt L89–95 covers the span
    heading('s3', 2, 'Decisions'),
    para('p3', ['elsewhere', 'code:crates/surface/src/lib.rs#L1-L3']),
  ]
  const stops = boardStops(blocks)
  expect(stops.map((s) => s.kind)).toEqual(['span', ...Array(6).fill('unit'), 'span', 'excerpt', 'span'])
  // §1: the span precedes the diagram in the document, but its stage is the section's diagram.
  expect(stops[0]).toMatchObject({ label: 'fences()', detail: 'see fences() and docs', stage: { blockId: 'g', unit: null } })
  expect(stops[1]!.stage).toEqual({ blockId: 'g', unit: 'lib' })
  // §2: no diagram → the span stands on the excerpt that covers it; the excerpt stop stands on itself.
  expect(stops[7]).toMatchObject({ label: 'closing()', stage: { blockId: 'e', unit: null } })
  expect(stops[8]).toMatchObject({ kind: 'excerpt', stage: { blockId: 'e', unit: null } })
  // §3: nothing to stand on.
  expect(stops[9]).toMatchObject({ label: 'elsewhere', stage: null })
  expect(formatAnchor(stops[9]!.anchor!)).toBe('crates/surface/src/lib.rs#L1-L3')
})
