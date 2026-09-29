import { expect, test } from 'vitest'
import { type Act, type Drag, IDLE, SLOP, step, type Verb } from './session-drag'

const run = (acts: Act[]): [Drag, Verb[]] => {
  let d = IDLE
  const verbs: Verb[] = []
  for (const a of acts) {
    const [next, v] = step(d, a)
    d = next
    verbs.push(v)
  }
  return [d, verbs.filter(Boolean)]
}

const down = (pointerType: string, over: Partial<Extract<Act, { type: 'down' }>> = {}): Act => ({
  type: 'down',
  x: 0,
  y: 0,
  pointerType,
  button: 0,
  modifier: false,
  sheet: false,
  ...over,
})

test('a mouse drags after SLOP and drops on lift; under SLOP it is a click', () => {
  expect(run([down('mouse'), { type: 'move', x: SLOP - 1, y: 0 }, { type: 'up' }])).toEqual([IDLE, []])
  expect(run([down('mouse'), { type: 'move', x: SLOP, y: 0 }, { type: 'move', x: 40, y: 0 }, { type: 'up' }])).toEqual([IDLE, ['start', 'move', 'drop']])
  // A pen and a trackpad are the same gesture; the hold timer is never armed for them.
  expect(run([down('pen'), { type: 'timer' }, { type: 'move', x: 20, y: 0 }, { type: 'up' }])).toEqual([IDLE, ['start', 'drop']])
})

test('a finger arms the hold; holding still starts the drag, then move and lift drop', () => {
  expect(run([down('touch'), { type: 'move', x: 2, y: 2 }, { type: 'timer' }, { type: 'move', x: 60, y: 0 }, { type: 'up' }])).toEqual([
    IDLE,
    ['hold', 'start', 'move', 'drop'],
  ])
})

test('a finger that travels SLOP before the hold is a scroll: nothing, and the late timer is inert', () => {
  expect(run([down('touch'), { type: 'move', x: 0, y: SLOP }, { type: 'timer' }, { type: 'up' }])).toEqual([IDLE, ['hold']])
})

test('a quick tap lifts before the hold: no drag, the click navigates', () => {
  expect(run([down('touch'), { type: 'up' }, { type: 'timer' }])).toEqual([IDLE, ['hold']])
})

test('lifting after the hold without moving cancels quietly; pointercancel cancels a drag', () => {
  expect(run([down('touch'), { type: 'timer' }, { type: 'up' }])).toEqual([IDLE, ['hold', 'start', 'cancel']])
  expect(run([down('touch'), { type: 'timer' }, { type: 'move', x: 30, y: 0 }, { type: 'cancel' }])).toEqual([IDLE, ['hold', 'start', 'move', 'cancel']])
  // iOS starting a scroll cancels the sequence before the hold: nothing started, nothing to cancel.
  expect(run([down('touch'), { type: 'cancel' }, { type: 'timer' }])).toEqual([IDLE, ['hold']])
})

test('fingers never drag in the phone Sheet; a mouse there still does', () => {
  expect(run([down('touch', { sheet: true }), { type: 'timer' }, { type: 'move', x: 30, y: 0 }, { type: 'up' }])).toEqual([IDLE, []])
  expect(run([down('mouse', { sheet: true }), { type: 'move', x: 30, y: 0 }, { type: 'up' }])).toEqual([IDLE, ['start', 'drop']])
})

test('a secondary button or a modified press is not a drag', () => {
  expect(run([down('mouse', { button: 2 }), { type: 'move', x: 30, y: 0 }, { type: 'up' }])).toEqual([IDLE, []])
  expect(run([down('mouse', { modifier: true }), { type: 'move', x: 30, y: 0 }, { type: 'up' }])).toEqual([IDLE, []])
})
