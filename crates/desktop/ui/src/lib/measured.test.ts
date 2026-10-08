import { beforeEach, expect, test, vi } from 'vitest'
import type { Row } from '@/components/chat/rows'
import type { Entry } from '@/store/sessions'
import { bucket, counters, estimateRow, estimateText, heights, PERSIST_KEYS, persist, recall, remember } from './measured'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
})

let n = 0
/** A fresh session id per test: the module LRU is global. */
const fresh = () => `s${++n}`

beforeEach(() => mem.clear())

test('bucket rounds to 16 px', () => {
  expect(bucket(736)).toBe(736)
  expect(bucket(741)).toBe(736)
  expect(bucket(745)).toBe(752)
  expect(bucket(0)).toBe(0)
})

test('remember/recall at one bucket; a different bucket starts empty and never restores', () => {
  const s = fresh()
  remember(s, 740, 'a', 300)
  expect(recall(s, 736, 'a')).toBe(300)
  expect(recall(s, 741, 'a')).toBe(300)
  expect(recall(s, 500, 'a')).toBeUndefined()
  // The old bucket is gone, not restored when the width comes back.
  expect(recall(s, 736, 'a')).toBeUndefined()
})

test('the module LRU keeps 32 sessions', () => {
  const first = fresh()
  remember(first, 736, 'k', 100)
  for (let i = 0; i < 32; i++) remember(fresh(), 736, 'k', 100)
  // Evicted: a new `heights` for `first` reloads from storage (nothing persisted yet in this test → empty).
  expect(recall(first, 736, 'k')).toBeUndefined()
})

test('persist writes { w, h } and reloads only at the same bucket', () => {
  const s = fresh()
  remember(s, 736, 'a', 120)
  remember(s, 736, 'b', 240)
  persist(s)
  expect(JSON.parse(mem.get(`theuth_heights:${s}`)!)).toEqual({ w: 736, h: { a: 120, b: 240 } })
  // Evict `s` from the module LRU, then read back.
  for (let i = 0; i < 33; i++) heights(fresh(), 736)
  expect(recall(s, 736, 'b')).toBe(240)
  for (let i = 0; i < 33; i++) heights(fresh(), 736)
  expect(recall(s, 1024, 'b')).toBeUndefined()
})

test('persist keeps the most recent keys, a re-remembered key counts as recent', () => {
  const s = fresh()
  for (let i = 0; i < PERSIST_KEYS + 10; i++) remember(s, 736, `k${i}`, i)
  remember(s, 736, 'k0', 7)
  persist(s)
  const { h } = JSON.parse(mem.get(`theuth_heights:${s}`)!) as { h: Record<string, number> }
  expect(Object.keys(h)).toHaveLength(PERSIST_KEYS)
  expect(h.k0).toBe(7)
  expect(h.k1).toBeUndefined()
  expect(h[`k${PERSIST_KEYS + 9}`]).toBe(PERSIST_KEYS + 9)
})

test('persist is debounced ≥ 1 s after the last remember and swallows quota errors', () => {
  vi.useFakeTimers()
  const s = fresh()
  remember(s, 736, 'a', 1)
  vi.advanceTimersByTime(800)
  remember(s, 736, 'b', 2)
  vi.advanceTimersByTime(800)
  expect(mem.has(`theuth_heights:${s}`)).toBe(false)
  vi.advanceTimersByTime(300)
  expect(JSON.parse(mem.get(`theuth_heights:${s}`)!).h).toEqual({ a: 1, b: 2 })
  vi.useRealTimers()
  vi.stubGlobal('localStorage', {
    getItem: () => null,
    setItem: () => {
      throw new Error('QuotaExceededError')
    },
  })
  expect(() => persist(s)).not.toThrow()
  vi.stubGlobal('localStorage', { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v) })
})

const assistant = (text: string, open = false): Row => ({ key: 'e', entry: { id: 'e', kind: 'assistant', text, open } as Entry, prompts: null })

test('estimateRow: floor 96, fixed kinds, monotonic in text length', () => {
  expect(estimateRow({ key: 'g', gap: '1 h later' }, 736)).toBe(96)
  expect(estimateRow(assistant('hi'), 736)).toBe(96)
  expect(estimateRow({ key: 'r', run: [], prompts: null }, 736)).toBe(96)
  expect(estimateRow({ key: 'p', prompt: { id: 'p' } as never }, 736)).toBe(160)
  let prev = 0
  let text = ''
  for (let i = 0; i < 40; i++) {
    text += 'lorem ipsum dolor sit amet, '.repeat(3) + (i % 7 === 0 ? '\n\n' : '')
    const px = estimateRow(assistant(text), 736)
    expect(px).toBeGreaterThanOrEqual(prev)
    prev = px
  }
  // Narrower column → more lines → taller.
  expect(estimateRow(assistant(text), 360)).toBeGreaterThan(estimateRow(assistant(text), 736))
})

test('estimateRow (R1.1): one computation per entry object and width', () => {
  const row = assistant('lorem ipsum '.repeat(50))
  const before = counters.estimates
  const px = estimateRow(row, 736)
  expect(estimateRow(row, 736)).toBe(px)
  expect(estimateRow({ ...row }, 736)).toBe(px) // the row wrapper is rebuilt per render; the entry object is the key
  expect(counters.estimates).toBe(before + 1)
  expect(estimateRow(row, 360)).toBeGreaterThan(px)
  expect(counters.estimates).toBe(before + 2)
  // A new entry object (a streaming delta) is a new estimate.
  estimateRow(assistant('lorem ipsum '.repeat(50)), 360)
  expect(counters.estimates).toBe(before + 3)
})

test('estimateText: fences by language, tables by rows, images flat', () => {
  expect(estimateText('```mermaid\ngraph TD\nA-->B\n```', 736)).toBe(320)
  expect(estimateText('```json-render\n{}\n```', 736)).toBe(320)
  expect(estimateText('```openui\n<x/>\n```', 736)).toBe(240)
  expect(estimateText('```ts\n1\n2\n3\n```', 736)).toBe(3 * 20 + 40)
  // An unclosed (streaming) fence still counts.
  expect(estimateText('```ts\n1\n2', 736)).toBe(2 * 20 + 40)
  expect(estimateText('| a | b |\n|---|---|\n| 1 | 2 |', 736)).toBe(3 * 32 + 40)
  expect(estimateText('![alt](x.png)', 736)).toBe(240)
  // One short paragraph: one line plus paragraph margin.
  expect(estimateText('hello', 736)).toBe(26 + 8)
  // 300 chars at ~102 per line → 3 lines.
  expect(estimateText('x'.repeat(300), 736)).toBe(3 * 26 + 8)
})
