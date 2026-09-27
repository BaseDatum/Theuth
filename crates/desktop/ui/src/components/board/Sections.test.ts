import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import type { DocBlock } from './blocks'
import { freshIds } from './Reveal'
import { countAnchors, sectionsOf, tocOf } from './Sections'
import { activeHeading } from './Toc'

const FIXTURE = readFileSync(new URL('./fixtures/surface-fence-parsing.md', import.meta.url), 'utf8')
const fences = [...FIXTURE.matchAll(/```json-render\n([\s\S]*?)\n```/g)].map((m) => m[1]!)

const heading = (id: string, level: number, text: string): DocBlock => ({ id, type: 'heading', props: { level }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock
const para = (id: string, links: string[] = []): DocBlock =>
  ({ id, type: 'paragraph', props: {}, content: links.map((href) => ({ type: 'link', href, content: [{ type: 'text', text: 'x', styles: {} }] })), children: [] }) as unknown as DocBlock
const code = (id: string, text: string): DocBlock => ({ id, type: 'codeBlock', props: { language: 'json-render' }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock
const fm: DocBlock = { id: 'fm', type: 'frontmatter', props: { yaml: 'root: /x' }, content: undefined, children: [] } as unknown as DocBlock

const board = [
  fm,
  heading('t', 1, 'Title'),
  para('lead', ['code:a.rs#L1-L2']),
  heading('s1', 2, 'What and why'),
  para('p1'),
  heading('s1a', 3, 'Detail'),
  para('p2', ['code:a.rs#L3', 'https://example.com']),
  heading('s2', 2, 'Map'),
  code('g', fences[0]!), // Graph: nodes with refs
  code('e', fences[2]!), // Excerpt
]

test('sections split on ## with a preamble; anchors count links, excerpts and unit refs', () => {
  const s = sectionsOf(board)
  expect(s.map((x) => x.heading?.id ?? null)).toEqual([null, 's1', 's2'])
  expect(s[0]!.blocks.map((b) => b.id)).toEqual(['fm', 't', 'lead'])
  expect(s[1]!.blocks.map((b) => b.id)).toEqual(['p1', 's1a', 'p2'])
  expect(s[0]!.anchors).toBe(1)
  expect(s[1]!.anchors).toBe(1) // the https link is not an anchor
  const refs = (JSON.stringify(JSON.parse(fences[0]!)).match(/"ref":/g) ?? []).length
  expect(refs).toBeGreaterThan(0)
  expect(s[2]!.anchors).toBe(refs + 1)
  expect(countAnchors([])).toBe(0)
})

test('a board with no ## has one preamble section; a frontmatter-only lead is dropped', () => {
  expect(sectionsOf([fm, heading('t', 1, 'T')]).map((x) => x.heading)).toEqual([null])
  expect(sectionsOf([fm, heading('s', 2, 'S')]).map((x) => x.heading?.id)).toEqual(['s'])
})

test('TOC entries are #/##/### in order, each ### under its ##', () => {
  expect(tocOf(board)).toEqual([
    { id: 't', level: 1, text: 'Title', section: null },
    { id: 's1', level: 2, text: 'What and why', section: 's1' },
    { id: 's1a', level: 3, text: 'Detail', section: 's1' },
    { id: 's2', level: 2, text: 'Map', section: 's2' },
  ])
})

test('active heading follows the M6 scroll rule: first at/below the top if in the top half, else the previous', () => {
  // tops relative to the scroller top; viewport 800.
  expect(activeHeading([-500, -100, 200, 900], 800)).toBe(2) // 200 in the top half → itself
  expect(activeHeading([-500, -100, 500, 900], 800)).toBe(1) // 500 below the half → the previous
  expect(activeHeading([0, 600], 800)).toBe(0) // at the top exactly
  expect(activeHeading([-900, -400], 800)).toBe(1) // scrolled past every heading → the last
  expect(activeHeading([600], 800)).toBe(0) // nothing before it to fall back to
  expect(activeHeading([], 800)).toBe(-1)
})

test('reveal: the first render is the baseline; later unseen ids are fresh, once', () => {
  const a = freshIds(null, [para('1'), para('2')])
  expect([...a.fresh]).toEqual([])
  const b = freshIds(a.seen, [para('1'), para('2'), para('3'), para('4')])
  expect([...b.fresh]).toEqual(['3', '4'])
  const c = freshIds(b.seen, [para('1'), para('2'), para('3'), para('4')])
  expect(c.fresh.size).toBe(0)
  const d = freshIds(c.seen, [para('1'), para('3')]) // a removal frees nothing, marks nothing
  expect(d.fresh.size).toBe(0)
})
