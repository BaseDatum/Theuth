import { expect, test } from 'vitest'
import { type Act, CANCEL_PX, type Hold, IDLE, LOCK_PX, step, type Verb } from './hold'

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

test('a quick release is a tap; wandering first is nothing', () => {
  expect(run([{ type: 'down', x: 0, y: 0 }, { type: 'up', x: 3, y: 2, inside: true }])).toEqual([IDLE, ['tap']])
  expect(run([{ type: 'down', x: 0, y: 0 }, { type: 'move', x: 30, y: 0 }, { type: 'up', x: 30, y: 0, inside: true }])).toEqual([IDLE, []])
})

test('holding records; releasing over the control sends', () => {
  expect(run([{ type: 'down', x: 0, y: 100 }, { type: 'timer' }, { type: 'move', x: 0, y: 90 }, { type: 'up', x: 0, y: 90, inside: true }])).toEqual([
    IDLE,
    ['record', 'send'],
  ])
})

test('sliding down past the threshold, or releasing outside, cancels', () => {
  expect(run([{ type: 'down', x: 0, y: 100 }, { type: 'timer' }, { type: 'up', x: 0, y: 100 + CANCEL_PX, inside: false }])).toEqual([IDLE, ['record', 'cancel']])
  expect(run([{ type: 'down', x: 0, y: 100 }, { type: 'timer' }, { type: 'up', x: 80, y: 100, inside: false }])).toEqual([IDLE, ['record', 'cancel']])
})

test('sliding up past the threshold locks; the finger lifting then changes nothing until reset', () => {
  const [h, verbs] = run([
    { type: 'down', x: 0, y: 100 },
    { type: 'timer' },
    { type: 'move', x: 0, y: 100 - LOCK_PX + 1 },
    { type: 'move', x: 0, y: 100 - LOCK_PX },
    { type: 'up', x: 0, y: 100 - LOCK_PX, inside: false },
  ])
  expect(h).toEqual({ phase: 'locked' })
  expect(verbs).toEqual(['record', 'lock'])
  expect(step(h, { type: 'reset' })).toEqual([IDLE, null])
})

test('the timer after release does not record', () => {
  expect(run([{ type: 'down', x: 0, y: 0 }, { type: 'up', x: 0, y: 0, inside: true }, { type: 'timer' }])).toEqual([IDLE, ['tap']])
})
