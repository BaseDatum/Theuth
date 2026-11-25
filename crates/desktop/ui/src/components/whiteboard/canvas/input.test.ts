import { describe, expect, it } from 'vitest'
import { MultiTap, PALM_MS, routePointer, type RouteState } from './input'

const base: RouteState = { mode: 'idle', fingerInk: false, nativeInk: false, readOnly: false, penDown: false, palmUntil: 0, now: 10_000 }
const s = (p: Partial<RouteState>): RouteState => ({ ...base, ...p })

describe('input routing (§8.4)', () => {
  it('pen inks in every mode except link (pointer) and erase; never passes to pan', () => {
    for (const mode of ['idle', 'draw', 'select', 'frame'] as const) expect(routePointer('pen', s({ mode }))).toBe('ink')
    expect(routePointer('pen', s({ mode: 'link' }))).toBe('pass')
    expect(routePointer('pen', s({ mode: 'erase' }))).toBe('erase')
  })

  it('pen never reaches pan: every pen route other than link mode is taken before React Flow', () => {
    for (const mode of ['idle', 'draw', 'erase', 'select', 'frame'] as const) {
      for (const nativeInk of [false, true]) for (const readOnly of [false, true]) {
        expect(routePointer('pen', s({ mode, nativeInk, readOnly }))).not.toBe('pass')
      }
    }
  })

  it('native ink swallows pen events (the plugin draws) except with the eraser', () => {
    expect(routePointer('pen', s({ nativeInk: true }))).toBe('swallow')
    expect(routePointer('pen', s({ nativeInk: true, mode: 'erase' }))).toBe('erase')
  })

  it('read-only: no ink from any pointer', () => {
    expect(routePointer('pen', s({ readOnly: true }))).toBe('swallow')
    expect(routePointer('touch', s({ readOnly: true, fingerInk: true, mode: 'draw' }))).toBe('pass')
    expect(routePointer('mouse', s({ readOnly: true, mode: 'draw' }))).toBe('pass')
  })

  it('touch navigates; draws only with fingerInk in draw/erase (R2: pen-only by default)', () => {
    expect(routePointer('touch', s({ mode: 'draw' }))).toBe('pass')
    expect(routePointer('touch', s({ mode: 'draw', fingerInk: true }))).toBe('ink')
    expect(routePointer('touch', s({ mode: 'erase', fingerInk: true }))).toBe('erase')
    expect(routePointer('touch', s({ mode: 'idle', fingerInk: true }))).toBe('pass')
  })

  it('mouse draws in draw/erase mode, else passes', () => {
    expect(routePointer('mouse', s({ mode: 'draw' }))).toBe('ink')
    expect(routePointer('mouse', s({ mode: 'erase' }))).toBe('erase')
    expect(routePointer('mouse', s({ mode: 'link' }))).toBe('pass')
  })

  it('palm window: while the pen is down and for 500 ms after, touches are ignored', () => {
    expect(routePointer('touch', s({ penDown: true }))).toBe('ignore')
    const up = 10_000
    expect(routePointer('touch', s({ palmUntil: up + PALM_MS, now: up + 499 }))).toBe('ignore')
    expect(routePointer('touch', s({ palmUntil: up + PALM_MS, now: up + 500 }))).toBe('pass')
    expect(routePointer('touch', s({ penDown: true, fingerInk: true, mode: 'draw' }))).toBe('ignore')
    // Mouse and pen are not palms.
    expect(routePointer('mouse', s({ penDown: true }))).toBe('pass')
  })
})

describe('multi-finger taps', () => {
  it('two-finger tap → 2 (undo), three-finger tap → 3 (redo)', () => {
    const m = new MultiTap()
    m.down(1, 0, 0, 0)
    m.down(2, 50, 0, 20)
    expect(m.up(1, 120)).toBe(0)
    expect(m.up(2, 140)).toBe(2)
    m.down(1, 0, 0, 1000)
    m.down(2, 50, 0, 1010)
    m.down(3, 90, 0, 1020)
    m.up(3, 1100)
    m.up(2, 1100)
    expect(m.up(1, 1150)).toBe(3)
  })

  it('a pinch (movement) or a slow press is not a tap; one finger is not either', () => {
    const m = new MultiTap()
    m.down(1, 0, 0, 0)
    m.down(2, 50, 0, 10)
    m.move(2, 90, 0)
    m.up(1, 100)
    expect(m.up(2, 110)).toBe(0)
    m.down(1, 0, 0, 1000)
    m.down(2, 50, 0, 1010)
    m.up(1, 1400)
    expect(m.up(2, 1400)).toBe(0)
    m.down(1, 0, 0, 2000)
    expect(m.up(1, 2050)).toBe(0)
  })
})
