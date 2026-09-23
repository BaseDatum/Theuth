import { expect, test } from 'vitest'
import type { GraphData } from './graph-layout'
import { LruCache, structuralHash, wrapLabel } from './graph-layout'

const base: GraphData = {
  direction: 'LR',
  groups: [{ id: 'core', label: 'Core' }],
  nodes: [
    { id: 'api', label: 'API', kind: 'rounded', group: 'core', state: null },
    { id: 'db', label: 'Postgres  main', kind: 'cylinder', group: null, state: 'active' },
  ],
  edges: [{ from: 'api', to: 'db', label: 'reads', style: null }],
}

test('structuralHash ignores label whitespace and sees an edge change', () => {
  const h = structuralHash(base)
  expect(h).toMatch(/^[0-9a-f]{8}$/)
  const spaced = { ...base, nodes: base.nodes.map((n) => ({ ...n, label: `  ${n.label.replace(/\s+/g, '   ')} ` })) }
  expect(structuralHash(spaced)).toBe(h)
  expect(structuralHash({ ...base, edges: [{ ...base.edges[0]!, to: 'api' }] })).not.toBe(h)
  expect(structuralHash({ ...base, nodes: [base.nodes[0]!, { ...base.nodes[1]!, state: 'danger' }] })).not.toBe(h)
  expect(structuralHash({ ...base, direction: 'TB' })).not.toBe(h)
})

test('LruCache evicts the least recently used entry past its capacity', () => {
  const c = new LruCache<number>(64)
  for (let i = 0; i < 64; i++) c.set(`k${i}`, i)
  expect(c.size).toBe(64)
  expect(c.get('k0')).toBe(0) // refreshed: k1 is now the oldest
  c.set('k64', 64)
  expect(c.size).toBe(64)
  expect(c.has('k0')).toBe(true)
  expect(c.has('k1')).toBe(false)
  expect(c.has('k64')).toBe(true)
})

test('wrapLabel wraps at 24 characters by word, then by character', () => {
  expect(wrapLabel('short')).toEqual(['short'])
  expect(wrapLabel('the quick brown fox jumps over the lazy dog')).toEqual(['the quick brown fox', 'jumps over the lazy dog'])
  expect(wrapLabel('x'.repeat(30))).toEqual(['x'.repeat(24), 'x'.repeat(6)])
})
