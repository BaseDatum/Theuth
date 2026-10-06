import { expect, test } from 'vitest'
import type { Item } from '@/rpc/bindings/Item'
import { allAnswered, allRecommended, bundleAnswer, head, slug } from './Bundle'

const item = (id: string, labels: string[], recommended: number | null): Item => ({ id, title: id, description: null, options: labels.map((label) => ({ label, description: null })), recommended })
const items = [item('apex', ['Switch', 'Keep'], 0), item('api', ['Switch', 'After canary'], 1), item('mx', ['Leave', 'Move'], null)]

test('bundleAnswer: one numbered line per item (pick label or typed text), the top-level answer last, nothing for a missing pick', () => {
  expect(bundleAnswer(items, [0, 1, 'leave for now'])).toBe('1. apex: Switch\n2. api: After canary\n3. mx: leave for now')
  expect(bundleAnswer(items, [0, 1, 'leave for now'], 'Apply tonight')).toBe('1. apex: Switch\n2. api: After canary\n3. mx: leave for now\nApply tonight')
  expect(bundleAnswer(items, [1, null, '  '], '  ')).toBe('1. apex: Keep\n2. api: \n3. mx: ')
  expect(bundleAnswer(items, [7, 0, 0])).toBe('1. apex: \n2. api: Switch\n3. mx: Leave')
})

test('allAnswered: every item has an option or non-blank text', () => {
  expect(allAnswered(items, [0, 1, 'x'])).toBe(true)
  expect(allAnswered(items, [0, 1, null])).toBe(false)
  expect(allAnswered(items, [0, 1, '  '])).toBe(false)
  expect(allAnswered(items, [0, 1])).toBe(false)
  expect(allAnswered([], [])).toBe(true)
})

test('allRecommended: every item has a recommendation and still sits on it', () => {
  const two = items.slice(0, 2)
  expect(allRecommended(two, [0, 1])).toBe(true)
  expect(allRecommended(two, [1, 1])).toBe(false) // changed
  expect(allRecommended(two, [0, 'typed'])).toBe(false) // typed over
  expect(allRecommended(items, [0, 1, 0])).toBe(false) // mx has no recommendation
  expect(allRecommended([], [])).toBe(false)
})

test('head: first non-blank line, cut at 140 chars', () => {
  expect(head('\n\n  Approve the DNS bundle?  \nmore')).toBe('Approve the DNS bundle?')
  expect(head('')).toBe('')
  const long = 'x'.repeat(200)
  expect(head(long)).toHaveLength(140)
  expect(head(long).endsWith('…')).toBe(true)
  expect(head('x'.repeat(140))).toBe('x'.repeat(140))
})

test('slug: lower-case, runs of non-alphanumerics → one dash, trimmed', () => {
  expect(slug('portal.example.com A record')).toBe('portal-example-com-a-record')
  expect(slug("  Let's Encrypt (CAA) ")).toBe('let-s-encrypt-caa')
  expect(slug('---')).toBe('')
})
