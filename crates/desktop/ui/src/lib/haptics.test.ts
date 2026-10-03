import { afterEach, expect, test, vi } from 'vitest'
import { haptics } from './haptics'

vi.mock('@tauri-apps/plugin-haptics', () => {
  throw new Error('no haptics plugin in this build')
})

afterEach(() => vi.unstubAllGlobals())

const every = () => {
  for (const fx of [haptics.tick, haptics.lock, haptics.cancel, haptics.send]) expect(fx).not.toThrow()
}

test('a browser without navigator.vibrate: every effect is a silent no-op', () => {
  vi.stubGlobal('window', {})
  vi.stubGlobal('navigator', {})
  every()
})

test('a phone shell whose plugin import fails: every effect is a silent no-op (the rejection is swallowed)', () => {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
  vi.stubGlobal('matchMedia', () => ({ matches: true }))
  vi.stubGlobal('navigator', {})
  every()
})

test('a browser with navigator.vibrate gets the patterns', () => {
  const vibrate = vi.fn()
  vi.stubGlobal('window', {})
  vi.stubGlobal('navigator', { vibrate })
  haptics.cancel()
  haptics.lock()
  expect(vibrate.mock.calls).toEqual([[[10, 40, 10]], [20]])
})
