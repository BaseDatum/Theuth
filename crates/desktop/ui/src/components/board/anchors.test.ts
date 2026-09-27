import { expect, test } from 'vitest'
import { parseAnchor } from '@/components/chat/surface/anchor'
import { type Anchor, activeOf, contains, createAnchorStore, formatAnchor, intersect, overlaps } from './anchors'

const a = (s: string): Anchor => {
  const r = parseAnchor(s)
  if (typeof r === 'string') throw new Error(r)
  return r
}

// Design 71 §2.3 resolution: same path and overlapping ranges; a bare path covers the file; blocks match by id.
// Ten fixtures, each `[left, right, overlap?]`; the rule is symmetric, so every pair is checked both ways.
const FIXTURES: [string, string, boolean][] = [
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L71-L84', true], // identical
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L80-L90', true], // partial overlap
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L72-L73', true], // one inside the other
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L84-L100', true], // touching at one line
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L85-L100', false], // adjacent, disjoint
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L73', true], // single line inside
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs#L70', false], // single line just outside
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/lib.rs', true], // bare path covers the file
  ['crates/surface/src/lib.rs#L71-L84', 'crates/surface/src/fences.rs#L71-L84', false], // other file, same lines
  ['block:b12', 'block:b12', true], // blocks by id
]

test('the ten overlap fixtures, both ways', () => {
  for (const [l, r, want] of FIXTURES) {
    expect(overlaps(a(l), a(r)), `${l} ~ ${r}`).toBe(want)
    expect(overlaps(a(r), a(l)), `${r} ~ ${l}`).toBe(want)
  }
  expect(overlaps(a('block:b12'), a('block:b13'))).toBe(false)
  expect(overlaps(a('block:b12'), a('crates/surface/src/lib.rs'))).toBe(false)
  expect(overlaps(a('README.md'), a('README.md'))).toBe(true)
})

test('an Excerpt is the code window for the anchors its range contains', () => {
  const ex = a('crates/surface/src/fences.rs#L74-L87')
  expect(contains(ex, a('crates/surface/src/fences.rs#L83'))).toBe(true)
  expect(contains(ex, a('crates/surface/src/fences.rs#L74-L87'))).toBe(true)
  expect(contains(ex, a('crates/surface/src/fences.rs#L80-L90'))).toBe(false) // overlaps but spills out
  expect(contains(ex, a('crates/surface/src/fences.rs'))).toBe(false) // a whole file is never inside a window
  expect(contains(ex, a('crates/surface/src/lib.rs#L80'))).toBe(false)
  expect(contains(a('crates/surface/src/fences.rs'), a('crates/surface/src/fences.rs#L1'))).toBe(true)
  expect(intersect(a('crates/surface/src/fences.rs#L80-L90'), [74, 87])).toEqual([80, 87])
  expect(intersect(a('crates/surface/src/fences.rs#L88-L90'), [74, 87])).toBeNull()
  expect(intersect(a('crates/surface/src/fences.rs'), [74, 87])).toBeNull()
})

test('formatAnchor round-trips the grammar', () => {
  for (const s of ['a/b.rs#L7', 'a/b.rs#L7-L9', 'a/b.rs', 'block:x']) expect(formatAnchor(a(s))).toBe(s)
})

test('precedence: tour step over pin over hover; unpin falls back to hover', () => {
  const store = createAnchorStore()
  const s = () => store.getState()
  expect(activeOf(s())).toBeNull()
  s().setHover(a('x.rs#L1'))
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L1')
  s().pin(a('x.rs#L5'), 'el-1', 'the sentence')
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L5')
  s().setHover(null)
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L5')
  const stop = (s: string) => ({ anchor: a(s), kind: 'unit' as const, label: s, detail: null, stage: null })
  s().openTour([stop('x.rs#L9'), stop('x.rs#L10'), { ...stop('x.rs#L11'), anchor: null }], 1, 'el-2', 'b9')
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L10')
  s().stepTour(99)
  expect(activeOf(s())).toBeNull() // clamped to the last stop, an explanation stop lights nothing
  s().stepTour(-1)
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L9')
  s().closeTour()
  s().setHover(a('x.rs#L1'))
  s().unpin()
  expect(formatAnchor(activeOf(s())!)).toBe('x.rs#L1')
  const off = s().register('el-3', { anchor: a('x.rs#L1'), kind: 'span', blockId: 'b1' })
  expect(s().registry.get('el-3')?.kind).toBe('span')
  off()
  expect(s().registry.has('el-3')).toBe(false)
})
