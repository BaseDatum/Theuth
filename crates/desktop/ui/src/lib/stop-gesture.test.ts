import { expect, test } from 'vitest'
import { type Act, HOLD_MS, IDLE, type Stop, TAP_WINDOW_MS, progress, step, type Verb } from './stop-gesture'

const run = (acts: Act[]): [Stop, Verb[]] => {
  let s = IDLE
  const verbs: Verb[] = []
  for (const a of acts) {
    const [next, v] = step(s, a)
    s = next
    verbs.push(v)
  }
  return [s, verbs.filter(Boolean)]
}

const down = (t: number, repeat = false): Act => ({ type: 'down', t, repeat })
const up = (t: number): Act => ({ type: 'up', t })
const tick = (t: number): Act => ({ type: 'tick', t })

test('a tap stops the root once; the release adds nothing', () => {
  expect(run([down(0), up(80)])[1]).toEqual(['root'])
})

test('a hold stops the tree exactly once at HOLD_MS, by repeat or by tick', () => {
  expect(run([down(0), down(50, true), down(HOLD_MS - 1, true), down(HOLD_MS, true), down(HOLD_MS + 500, true), up(HOLD_MS + 600)])[1]).toEqual(['root', 'tree'])
  expect(run([down(0), tick(HOLD_MS), tick(HOLD_MS + 100)])[1]).toEqual(['root', 'tree'])
})

test('releasing before HOLD_MS fires nothing more; a late tick after the release is ignored', () => {
  expect(run([down(0), down(300, true), up(400), tick(HOLD_MS)])[1]).toEqual(['root'])
  expect(run([down(100, true)])[1]).toEqual([])
})

test('three taps inside the window stop the tree; the third is not also a root stop', () => {
  expect(run([down(0), up(50), down(300), up(350), down(600)])[1]).toEqual(['root', 'root', 'tree'])
})

test('the window slides: spaced taps never add up, and a tree stop spends the taps', () => {
  expect(run([down(0), up(50), down(TAP_WINDOW_MS), up(TAP_WINDOW_MS + 50), down(2 * TAP_WINDOW_MS)])[1]).toEqual(['root', 'root', 'root'])
  const [s, verbs] = run([down(0), up(50), down(100), up(150), down(200), up(250), down(300), up(350)])
  expect(verbs).toEqual(['root', 'root', 'tree', 'root'])
  expect(s.taps).toEqual([300])
})

test('progress fills over the hold and resets once the tree stopped', () => {
  const [held] = run([down(0)])
  expect(progress(held, HOLD_MS / 2)).toBeCloseTo(0.5)
  expect(progress(held, 2 * HOLD_MS)).toBe(1)
  const [fired] = run([down(0), tick(HOLD_MS)])
  expect(progress(fired, HOLD_MS)).toBe(0)
  expect(progress(IDLE, 10)).toBe(0)
})
