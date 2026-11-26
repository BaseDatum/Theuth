// Present mode and "no uninvited chrome": the cluster never rises on a rest or with Hand, Present locks Hand and gives
// the latched tool back, Esc leaves Present first, and the cursor/exit timers only run on moves.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createWhiteboardStore } from './canvas/store'
import { awake, CURSOR_IDLE_MS, presentKey, presentMode, revealsCluster, revealsExit } from './present'

const R = { left: 0, top: 0, width: 1000, height: 800 }
const key = (key: string, mods: Partial<Record<'shiftKey' | 'metaKey' | 'ctrlKey' | 'altKey', boolean>> = {}) => ({ key, shiftKey: false, metaKey: false, ctrlKey: false, altKey: false, ...mods })

describe('revealsCluster', () => {
  it('rises on a rightward move in the right-edge strip, lower half', () => {
    expect(revealsCluster({ x: 950, y: 700 }, R, 3, 'idle', false)).toBe(true)
  })
  it('never with Hand, never while presenting', () => {
    expect(revealsCluster({ x: 950, y: 700 }, R, 3, 'hand', false)).toBe(false)
    expect(revealsCluster({ x: 950, y: 700 }, R, 3, 'idle', true)).toBe(false)
  })
  it('not for a still pointer, a leftward move, or a move elsewhere in the old quadrant', () => {
    expect(revealsCluster({ x: 950, y: 700 }, R, 0, 'idle', false)).toBe(false)
    expect(revealsCluster({ x: 950, y: 700 }, R, -4, 'idle', false)).toBe(false)
    expect(revealsCluster({ x: 600, y: 700 }, R, 3, 'idle', false)).toBe(false)
    expect(revealsCluster({ x: 950, y: 200 }, R, 3, 'idle', false)).toBe(false)
  })
})

describe('revealsExit', () => {
  it('only the top-right corner', () => {
    expect(revealsExit({ x: 950, y: 40 }, R)).toBe(true)
    expect(revealsExit({ x: 500, y: 40 }, R)).toBe(false)
    expect(revealsExit({ x: 950, y: 400 }, R)).toBe(false)
  })
})

describe('presentMode', () => {
  it('locks Hand, clears the selection and the entered card, and gives the latched ink tool back on exit', () => {
    const store = createWhiteboardStore({ mode: 'draw', tool: 'marker', selection: ['a'], entered: 'a' })
    const p = presentMode(store)
    p.on()
    expect(store.getState()).toMatchObject({ present: true, mode: 'hand', selection: [], entered: null })
    // A Pencil double-tap, a stray shortcut, a click on a card: put back at once.
    store.getState().set({ mode: 'erase' })
    store.getState().set({ selection: ['b'] })
    store.getState().set({ entered: 'b' })
    expect(store.getState()).toMatchObject({ mode: 'hand', selection: [], entered: null })
    p.toggle()
    expect(store.getState()).toMatchObject({ present: false, mode: 'draw', tool: 'marker' })
    // Off: no lock.
    store.getState().set({ mode: 'select', selection: ['c'] })
    expect(store.getState()).toMatchObject({ mode: 'select', selection: ['c'] })
    p.dispose()
  })
  it('on twice keeps the tool from before the first', () => {
    const store = createWhiteboardStore({ mode: 'link' })
    const p = presentMode(store)
    p.on()
    p.on()
    p.off()
    expect(store.getState().mode).toBe('link')
    p.dispose()
  })
})

describe('presentKey', () => {
  it('Shift+P toggles; plain P stays the pen shortcut', () => {
    expect(presentKey(key('P', { shiftKey: true }), false)).toBe('toggle')
    expect(presentKey(key('P', { shiftKey: true }), true)).toBe('toggle')
    expect(presentKey(key('p'), false)).toBe(null)
  })
  it('Esc exits Present first; outside Present it is left to the canvas (the ink latch)', () => {
    expect(presentKey(key('Escape'), true)).toBe('exit')
    expect(presentKey(key('Escape'), false)).toBe(null)
  })
  it('while presenting, tool/add/delete keys are swallowed; chords and Space pass', () => {
    for (const k of ['v', 'h', 'p', 'e', 'n', 'o', 'Delete', 'Backspace', 'Enter']) expect(presentKey(key(k), true)).toBe('swallow')
    expect(presentKey(key(' '), true)).toBe(null)
    expect(presentKey(key('0', { metaKey: true }), true)).toBe(null)
  })
})

describe('awake', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())
  it('on while poked, off after the idle time, each move restarting it', () => {
    const seen: boolean[] = []
    const a = awake({ ms: CURSOR_IDLE_MS, onChange: (on) => seen.push(on) })
    a.poke()
    vi.advanceTimersByTime(CURSOR_IDLE_MS - 1)
    a.poke()
    vi.advanceTimersByTime(CURSOR_IDLE_MS - 1)
    expect(seen).toEqual([true])
    vi.advanceTimersByTime(1)
    expect(seen).toEqual([true, false])
    // Idle stays idle: nothing turns it back on but a poke.
    vi.advanceTimersByTime(60_000)
    expect(seen).toEqual([true, false])
    a.poke()
    a.stop()
    expect(seen).toEqual([true, false, true, false])
    a.dispose()
  })
})
