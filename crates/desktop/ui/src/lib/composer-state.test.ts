import { expect, test } from 'vitest'
import { interrupt, phase, press } from './composer-state'

test('phase: running with queued steers is its own state', () => {
  expect(phase(false, 0)).toBe('idle')
  expect(phase(false, 2)).toBe('idle')
  expect(phase(true, 0)).toBe('running')
  expect(phase(true, 1)).toBe('queued')
})

test('press: send when idle, steer while running, nothing on an empty box', () => {
  expect(press('idle', true)).toBe('send')
  expect(press('running', true)).toBe('steer')
  expect(press('queued', true)).toBe('steer')
  expect(press('idle', false)).toBeNull()
  expect(press('running', false)).toBeNull()
  expect(press('queued', false)).toBeNull()
})

test('interrupt: text interjects, an empty box flushes queued steers, never drops them', () => {
  expect(interrupt('running', true)).toBe('interject')
  expect(interrupt('queued', true)).toBe('interject')
  expect(interrupt('queued', false)).toBe('flush')
  expect(interrupt('running', false)).toBeNull()
  expect(interrupt('idle', true)).toBe('send')
  expect(interrupt('idle', false)).toBeNull()
})
