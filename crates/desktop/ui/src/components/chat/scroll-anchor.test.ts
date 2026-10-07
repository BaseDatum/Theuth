import { expect, test } from 'vitest'
import { type State, step } from './scroll-anchor'

const at = (slack: number, index = 10, offset = 12, target = 2000): State => ({ slack, target, anchor: { index, offset } })

test('grow above the anchor is absorbed by the slack, never written', () => {
  const [s, e] = step(at(1000), { kind: 'grow', index: 3, delta: 300, moved: 0 })
  expect(e).toEqual({ slack: 700, write: 0, library: false })
  expect(s.slack).toBe(700)
})

test('grow above the anchor at exactly the slack empties it; one more px goes to the library', () => {
  expect(step(at(300), { kind: 'grow', index: 3, delta: 300, moved: 0 })[1]).toEqual({ slack: 0, write: 0, library: false })
  const [s, e] = step(at(300), { kind: 'grow', index: 3, delta: 301, moved: 0 })
  expect(e).toEqual({ slack: 300, write: 0, library: true })
  expect(s.slack).toBe(300)
})

test('shrink above the anchor grows the slack', () => {
  expect(step(at(0), { kind: 'grow', index: 3, delta: -120, moved: 0 })[1]).toEqual({ slack: 120, write: 0, library: false })
})

test('anchor row: only the moved part is absorbed; unabsorbed movement shifts the anchor offset, never the library', () => {
  expect(step(at(500), { kind: 'grow', index: 10, delta: 400, moved: 150 })[1]).toEqual({ slack: 350, write: 0, library: false })
  const [s, e] = step(at(100), { kind: 'grow', index: 10, delta: 400, moved: 150 })
  expect(e).toEqual({ slack: 100, write: 0, library: false })
  expect(s.anchor).toEqual({ index: 10, offset: 162 })
})

test('a streaming anchor row (moved 0, repeatedly) and rows below the anchor never change anything', () => {
  let s = at(1000)
  for (let i = 0; i < 50; i++) {
    const [n, e] = step(s, { kind: 'grow', index: 10, delta: 40, moved: 0 })
    expect(e).toEqual({ slack: 1000, write: 0, library: false })
    s = n
  }
  expect(step(s, { kind: 'grow', index: 11, delta: 900, moved: 0 })[1]).toEqual({ slack: 1000, write: 0, library: false })
  expect(s).toEqual(at(1000))
})

test('no anchor: nothing', () => {
  const s: State = { slack: 500, target: 2000, anchor: null }
  expect(step(s, { kind: 'grow', index: 0, delta: 900, moved: 0 })).toEqual([s, { slack: 500, write: 0, library: false }])
})

test('prepend eats the slack, larger than the slack leaves zero for anchorTo:end', () => {
  expect(step(at(2000), { kind: 'prepend', estimated: 1500 })[1]).toEqual({ slack: 500, write: 0, library: false })
  expect(step(at(2000), { kind: 'prepend', estimated: 5000 })[1]).toEqual({ slack: 0, write: 0, library: false })
})

test('rest refills toward the target with a compensated write', () => {
  const [s, e] = step(at(500), { kind: 'rest', scrollTop: 3000 })
  expect(e).toEqual({ slack: 2000, write: 1500, library: false })
  expect(s.slack).toBe(2000)
})

test('rest with target 0 (more ended) removes the slack; the write is bounded so scrollTop stays ≥ 0', () => {
  expect(step(at(800, 10, 12, 0), { kind: 'rest', scrollTop: 3000 })[1]).toEqual({ slack: 0, write: -800, library: false })
  // Reader inside the blank slack: rows slide up into view rather than leaving a blank above the oldest row.
  expect(step(at(800, 10, 12, 0), { kind: 'rest', scrollTop: 300 })[1]).toEqual({ slack: 0, write: -300, library: false })
})

test('target is stored and applied at the next rest; rest at target writes nothing', () => {
  const [s, e] = step(at(700), { kind: 'target', target: 0 })
  expect(e).toEqual({ slack: 700, write: 0, library: false })
  expect(step(s, { kind: 'rest', scrollTop: 900 })[1]).toEqual({ slack: 0, write: -700, library: false })
  expect(step(at(2000), { kind: 'rest', scrollTop: 900 })[1]).toEqual({ slack: 2000, write: 0, library: false })
})
