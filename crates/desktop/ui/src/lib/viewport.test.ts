// `applyViewport` (112 §2): keyboard-open only when the visible band collapsed against this window's own
// idle height, never because the window is shorter than the screen (iPad Stage Manager, hardware keyboard).
import { readFileSync } from 'node:fs'
import { afterEach, expect, test, vi } from 'vitest'
import { applyViewport, remeasureViewport, SETTLE_MS, trackViewport } from './viewport'

class Input {}
class TextArea {}
class Element {}

const root = { band: '', open: false }
let win: { innerWidth: number; innerHeight: number; visualViewport: { height: number }; scrollX: number; scrollY: number; scrollTo(): void }
let active: unknown = null

function setup(opts: { touch: boolean; width?: number; height: number; visible?: number }) {
  win = { innerWidth: opts.width ?? 390, innerHeight: opts.height, visualViewport: { height: opts.visible ?? opts.height }, scrollX: 0, scrollY: 0, scrollTo: vi.fn() }
  active = null
  vi.stubGlobal('HTMLInputElement', Input)
  vi.stubGlobal('HTMLTextAreaElement', TextArea)
  vi.stubGlobal('HTMLElement', Element)
  vi.stubGlobal('window', win)
  vi.stubGlobal('matchMedia', () => ({ matches: opts.touch }))
  vi.stubGlobal('document', {
    get activeElement() {
      return active
    },
    documentElement: { style: { setProperty: (_: string, v: string) => (root.band = v) }, toggleAttribute: (_: string, on: boolean) => (root.open = on) },
  })
  applyViewport() // idle baseline for this window
}

const focus = () => (active = new TextArea())
const state = () => ({ open: root.open, band: root.band })

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

test('phone, visual viewport shrinks: open, band follows the visual viewport, closes on blur', () => {
  setup({ touch: true, height: 844 })
  focus()
  win.visualViewport.height = 844 - 336
  applyViewport()
  expect(state()).toEqual({ open: true, band: '508px' })
  active = null
  win.visualViewport.height = 844
  applyViewport()
  expect(state()).toEqual({ open: false, band: '844px' })
})

test('phone, layout viewport shrinks too (resizes-content): open against the idle height', () => {
  setup({ touch: true, height: 844 })
  focus()
  win.innerHeight = 508
  win.visualViewport.height = 508
  applyViewport()
  expect(state()).toEqual({ open: true, band: '508px' })
})

test('windowed iPad with a hardware keyboard: focus without a keyboard is a no-op, shortcuts bar included', () => {
  // The window is far shorter than the screen; the old screen comparison fired here.
  setup({ touch: true, width: 1024, height: 727 })
  focus()
  applyViewport()
  expect(state()).toEqual({ open: false, band: '727px' })
  // WebKit 247410: the floating shortcuts bar still takes ~69 px off the visual viewport.
  win.visualViewport.height = 658
  applyViewport()
  expect(state()).toEqual({ open: false, band: '727px' })
})

test('window resize while idle re-bases: the smaller window does not read as a keyboard on focus', () => {
  setup({ touch: true, width: 1024, height: 1000 })
  win.innerHeight = win.visualViewport.height = 600
  applyViewport()
  focus()
  applyViewport()
  expect(state()).toEqual({ open: false, band: '600px' })
  // A real keyboard against the new baseline still opens.
  win.visualViewport.height = 300
  applyViewport()
  expect(state()).toEqual({ open: true, band: '300px' })
})

test('rotation while editing re-bases (a keyboard never changes the width)', () => {
  setup({ touch: true, width: 390, height: 844 })
  focus()
  win.visualViewport.height = 508
  applyViewport()
  expect(root.open).toBe(true)
  win.innerWidth = 844
  win.innerHeight = win.visualViewport.height = 390
  applyViewport()
  expect(state()).toEqual({ open: false, band: '390px' })
})

test('desktop (no touch): the baseline is not consulted; a window resize while editing is just the new layout height', () => {
  setup({ touch: false, width: 1440, height: 900 })
  focus()
  win.innerHeight = win.visualViewport.height = 700
  applyViewport()
  expect(state()).toEqual({ open: false, band: '700px' })
})

/** `trackViewport` with listeners recorded per target, real timers faked; returns the dispatcher and disposer. */
function tracked(height: number) {
  setup({ touch: true, width: 1024, height })
  vi.useFakeTimers()
  const on = new Map<string, () => void>()
  const target = (name: string) => ({ addEventListener: (n: string, f: () => void) => on.set(`${name}:${n}`, f), removeEventListener: (n: string) => on.delete(`${name}:${n}`) })
  Object.assign(win, target('window'))
  Object.assign(win.visualViewport, target('viewport'))
  Object.assign(document, target('document'))
  vi.stubGlobal('requestAnimationFrame', (f: () => void) => setTimeout(f, 16) as unknown as number)
  vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id))
  const stop = trackViewport()
  return { fire: (key: string) => on.get(key)?.(), on, stop }
}

test('opening a chat re-reads the viewport with no window event: a band left stale by an iPad window change is corrected', () => {
  const { stop } = tracked(1366)
  expect(root.band).toBe('1366px')
  // Stage Manager shrank the window; no resize reached us (or it carried the old size).
  win.innerHeight = win.visualViewport.height = 900
  remeasureViewport()
  vi.advanceTimersByTime(16)
  expect(root.band).toBe('900px')
  stop()
})

test('a resize whose reading is still the old size is read again once it settles', () => {
  const { fire, stop } = tracked(1366)
  fire('viewport:resize')
  vi.advanceTimersByTime(16)
  expect(root.band).toBe('1366px')
  // WebKit lands the final size later, with no further event.
  win.innerHeight = win.visualViewport.height = 900
  vi.advanceTimersByTime(SETTLE_MS)
  expect(root.band).toBe('900px')
  stop()
})

test('coming back to the app (visibilitychange) re-reads the viewport; the disposer drops every listener', () => {
  const { fire, on, stop } = tracked(1366)
  win.innerHeight = win.visualViewport.height = 1024
  fire('document:visibilitychange')
  vi.advanceTimersByTime(SETTLE_MS)
  expect(root.band).toBe('1024px')
  stop()
  expect(on.size).toBe(0)
  win.innerHeight = win.visualViewport.height = 700
  remeasureViewport()
  vi.advanceTimersByTime(SETTLE_MS)
  expect(root.band).toBe('1024px')
})

test('the page (html, body, #root) takes the band like the shell, not its own viewport unit, and the root route re-reads it per path', () => {
  const css = readFileSync(new URL('../styles.css', import.meta.url), 'utf8')
  expect(css).toMatch(/html,\s*body,\s*#root \{\s*height: var\(--visible-band, 100dvh\);/)
  const root = readFileSync(new URL('../routes/__root.tsx', import.meta.url), 'utf8')
  expect(root).toContain('useEffect(remeasureViewport, [pathname])')
})
