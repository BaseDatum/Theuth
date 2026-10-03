import { expect, test } from 'vitest'
import { type Act, AXIS_PX, CANCEL_PX, type Hold, IDLE, LOCK_PX, step, type Verb } from './hold'

const run = (acts: Act[]): [Hold, Verb[]] => {
  let h = IDLE
  const verbs: Verb[] = []
  for (const a of acts) {
    const [next, v] = step(h, a)
    h = next
    verbs.push(v)
  }
  return [h, verbs.filter(Boolean)]
}

const down: Act = { type: 'down', x: 100, y: 100 }
const held: Act[] = [down, { type: 'timer' }]

test('a quick release is a tap; wandering first is nothing', () => {
  expect(run([down, { type: 'up', x: 103, y: 102 }])).toEqual([IDLE, ['tap']])
  expect(run([down, { type: 'move', x: 130, y: 100 }, { type: 'up', x: 130, y: 100 }])).toEqual([IDLE, []])
})

test('the timer records; a timer after the release does not', () => {
  const [h, verbs] = run(held)
  expect(h).toEqual({ phase: 'held', x: 100, y: 100, axis: 'none', dx: 0, dy: 0 })
  expect(verbs).toEqual(['record'])
  expect(run([down, { type: 'up', x: 100, y: 100 }, { type: 'timer' }])).toEqual([IDLE, ['tap']])
})

test('the gesture commits to the first axis that travels AXIS_PX and ignores the other afterwards', () => {
  const [a] = run([...held, { type: 'move', x: 100 - AXIS_PX + 1, y: 100 - AXIS_PX + 1 }])
  expect(a).toMatchObject({ phase: 'held', axis: 'none', dx: -(AXIS_PX - 1), dy: AXIS_PX - 1 })
  const [x] = run([...held, { type: 'move', x: 100 - AXIS_PX, y: 100 }, { type: 'move', x: 80, y: 40 }])
  expect(x).toMatchObject({ phase: 'held', axis: 'x', dx: -20, dy: 0 })
  const [y] = run([...held, { type: 'move', x: 100, y: 100 - AXIS_PX }, { type: 'move', x: 20, y: 70 }])
  expect(y).toMatchObject({ phase: 'held', axis: 'y', dx: 0, dy: 30 })
})

test('sliding up LOCK_PX locks on the move; the finger lifting then changes nothing until reset', () => {
  const [h, verbs] = run([...held, { type: 'move', x: 100, y: 100 - LOCK_PX + 1 }, { type: 'move', x: 100, y: 100 - LOCK_PX }, { type: 'up', x: 100, y: 10 }])
  expect(h).toEqual({ phase: 'locked' })
  expect(verbs).toEqual(['record', 'lock'])
  expect(step(h, { type: 'move', x: 0, y: 0 })).toEqual([h, null])
  expect(step(h, { type: 'reset' })).toEqual([IDLE, null])
})

test('sliding left CANCEL_PX cancels on the move; the release that follows does nothing', () => {
  expect(run([...held, { type: 'move', x: 100 - CANCEL_PX + 1, y: 100 }])).toEqual([{ phase: 'held', x: 100, y: 100, axis: 'x', dx: -(CANCEL_PX - 1), dy: 0 }, ['record']])
  expect(run([...held, { type: 'move', x: 100 - CANCEL_PX, y: 100 }, { type: 'up', x: 100 - CANCEL_PX, y: 100 }])).toEqual([IDLE, ['record', 'cancel']])
})

test('a diagonal only does what its committed axis does', () => {
  // Committed to x: travelling far up afterwards never locks; far left still cancels.
  expect(run([...held, { type: 'move', x: 100 - AXIS_PX, y: 100 }, { type: 'move', x: 90, y: 100 - 2 * LOCK_PX }])).toMatchObject([{ phase: 'held', axis: 'x' }, ['record']])
  expect(run([...held, { type: 'move', x: 100 - AXIS_PX, y: 100 }, { type: 'move', x: 100 - CANCEL_PX, y: 0 }])).toEqual([IDLE, ['record', 'cancel']])
  // Committed to y: travelling far left afterwards never cancels; far up still locks.
  expect(run([...held, { type: 'move', x: 100, y: 100 - AXIS_PX }, { type: 'move', x: 100 - 2 * CANCEL_PX, y: 90 }])).toMatchObject([{ phase: 'held', axis: 'y' }, ['record']])
  expect(run([...held, { type: 'move', x: 100, y: 100 - AXIS_PX }, { type: 'move', x: -200, y: 100 - LOCK_PX }])).toEqual([{ phase: 'locked' }, ['record', 'lock']])
  // Both axes cross in one move: the larger travel wins.
  expect(run([...held, { type: 'move', x: 100 - AXIS_PX - 2, y: 100 - AXIS_PX - 1 }])).toMatchObject([{ axis: 'x' }, ['record']])
})

test('releasing anywhere while held sends — outside the control included', () => {
  expect(run([...held, { type: 'up', x: 100, y: 100 }])).toEqual([IDLE, ['record', 'send']])
  expect(run([...held, { type: 'move', x: 140, y: 160 }, { type: 'up', x: 140, y: 160 }])).toEqual([IDLE, ['record', 'send']])
  expect(run([...held, { type: 'move', x: 100 - CANCEL_PX + 1, y: 100 }, { type: 'up', x: 100 - CANCEL_PX + 1, y: 100 }])).toEqual([IDLE, ['record', 'send']])
})

test('an interruption locks a held note and drops a mere press', () => {
  expect(run([...held, { type: 'interrupt' }])).toEqual([{ phase: 'locked' }, ['record', 'lock']])
  expect(run([down, { type: 'interrupt' }, { type: 'timer' }])).toEqual([IDLE, []])
})

test('reset leaves every phase', () => {
  for (const acts of [[down], held, [...held, { type: 'move', x: 100, y: 0 } as Act]]) {
    expect(run([...acts, { type: 'reset' }])[0]).toEqual(IDLE)
  }
  expect(step(IDLE, { type: 'reset' })).toEqual([IDLE, null])
})
