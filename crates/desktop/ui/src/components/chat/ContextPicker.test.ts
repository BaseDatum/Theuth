import { expect, test } from 'vitest'
import type { ContextItem } from '@/rpc/bindings/ContextItem'
import type { ContextTarget } from '@/rpc/bindings/ContextTarget'
import { expand } from './ContextPicker'

const m = (seq: number): ContextTarget => ({ kind: 'message', seq })
const b = (seq: number, part: number, image: number | null = null): ContextTarget => ({
  kind: 'binary',
  owner: { kind: 'message', seq },
  part,
  image,
})
const item = (target: ContextTarget, remove_with: ContextTarget[] = []): ContextItem => ({
  target,
  kind: target.kind === 'binary' ? 'image' : 'message',
  label: JSON.stringify(target),
  remove_with,
})

// Assistant 2 calls A/B, tool 3 answers both; a stray multi-row response 4 belongs to 3.
const items = [
  item(m(1)),
  item(m(2), [m(3)]),
  item(m(3), [m(2), m(4)]),
  item(m(4), [m(3)]),
  item(b(1, 1)),
  item(b(3, 0, 0)),
  item(b(3, 0, 1)),
]

test('closure follows remove_with to a fixed point and lists companions once', () => {
  const { targets, companions } = expand(items, new Set(['m2']))
  expect(targets).toEqual([m(2), m(3), m(4)])
  expect(companions.map((c) => c.target)).toEqual([m(3), m(4)])
})

test('a whole message subsumes its own binaries; unrelated binaries stay', () => {
  const { targets, companions } = expand(items, new Set(['m3/0.0', 'm2', 'm1/1']))
  // Insertion order: selected keys first, then closure; `m3/0.0` drops because m3 is in the set.
  expect(targets).toEqual([m(2), b(1, 1), m(3), m(4)])
  expect(companions.map((c) => c.target)).toEqual([m(3), m(4)])
})

test('binaries alone remove nothing else', () => {
  const { targets, companions } = expand(items, new Set(['m3/0.0', 'm3/0.1']))
  expect(targets).toEqual([b(3, 0, 0), b(3, 0, 1)])
  expect(companions).toEqual([])
})
