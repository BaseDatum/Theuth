import { createElement } from 'react'
import { renderToString } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import {
  BAR,
  covered,
  mountBar,
  nativeChromeSettled as flush,
  onNativeTap,
  probeNativeChrome,
  resetNativeChrome,
  setBarTitle,
  setScrollButton,
  syncPageBackground,
  trackScrollButton,
  useNativeBar,
  useNativeChrome,
} from './native-chrome'

const plugin = vi.hoisted(() => ({
  calls: [] as [string, unknown][],
  supported: true as boolean | Error,
  tap: null as ((p: { id: string }) => void) | null,
  listeners: 0,
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: async (cmd: string, args?: unknown) => {
    if (cmd === 'plugin:glass|is_supported') {
      if (plugin.supported instanceof Error) throw plugin.supported
      return { supported: plugin.supported }
    }
    plugin.calls.push([cmd.replace('plugin:glass|', ''), args])
  },
  addPluginListener: async (name: string, event: string, cb: (p: { id: string }) => void) => {
    expect([name, event]).toEqual(['glass', 'tap'])
    plugin.listeners++
    plugin.tap = cb
    return { unregister: async () => {} }
  },
}))

/** The `<body>` children observer and the `<html>` class observer the bridge starts while a header owns the bar. */
let observers: (() => void)[] = []
let dialogs: { popover: boolean }[] = []
let dark = false

function stubIos(userAgent = 'Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X)', mobile = true): void {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {}, matchMedia: () => ({ matches: mobile, addEventListener() {}, removeEventListener() {} }) })
  vi.stubGlobal('navigator', { userAgent, maxTouchPoints: 5 })
  vi.stubGlobal(
    'MutationObserver',
    class {
      constructor(private cb: () => void) {}
      observe() {
        observers.push(this.cb)
      }
      disconnect() {
        observers = observers.filter((o) => o !== this.cb)
      }
    },
  )
  vi.stubGlobal('document', {
    body: {},
    documentElement: { classList: { contains: (c: string) => c === 'dark' && dark } },
    querySelectorAll: () => dialogs.map((d) => ({ closest: (s: string) => (s === '[data-radix-popper-content-wrapper]' && d.popover ? {} : null) })),
  })
}
const mutate = async () => {
  for (const o of [...observers]) o()
  await flush()
}

beforeEach(() => {
  resetNativeChrome()
  plugin.calls = []
  plugin.supported = true
  plugin.tap = null
  plugin.listeners = 0
  observers = []
  dialogs = []
  dark = false
})
afterEach(() => vi.unstubAllGlobals())

test('fallback: a browser or a desktop shell never calls the plugin and stays unsupported', async () => {
  vi.stubGlobal('window', {})
  plugin.supported = new Error('must not be asked')
  expect(await probeNativeChrome()).toBe(false)
  expect(useNativeChrome.getState().supported).toBe(false)
  resetNativeChrome()
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', maxTouchPoints: 0 })
  expect(await probeNativeChrome()).toBe(false)
})

test('iOS: the plugin decides; below iOS 26 (false) or without the plugin (a rejection) the web chrome stays', async () => {
  stubIos()
  expect(await probeNativeChrome()).toBe(true)
  expect(useNativeChrome.getState().supported).toBe(true)
  for (const answer of [false, new Error('Plugin glass not initialized')]) {
    resetNativeChrome()
    plugin.supported = answer
    expect(await probeNativeChrome()).toBe(false)
    expect(useNativeChrome.getState().supported).toBe(false)
  }
})

test('useNativeBar: only supported, in the phone layout, for the header that owns the window (iPad wide = web)', () => {
  const Probe = ({ own }: { own: boolean }) => createElement('i', null, String(useNativeBar(own)))
  // SSR renders zustand's initial state (the server snapshot), so the test writes that object.
  const initial = useNativeChrome.getInitialState()
  try {
    stubIos()
    initial.supported = true
    expect(renderToString(createElement(Probe, { own: true }))).toContain('true')
    expect(renderToString(createElement(Probe, { own: false }))).toContain('false')
    stubIos('Mozilla/5.0 (Macintosh)', false)
    expect(renderToString(createElement(Probe, { own: true }))).toContain('false')
    stubIos()
    initial.supported = false
    expect(renderToString(createElement(Probe, { own: true }))).toContain('false')
  } finally {
    initial.supported = false
  }
})

test('the owning header shows the bar; a sheet or dialog hides it, a popover does not; unmount hides it', async () => {
  stubIos()
  dark = true
  const unmount = mountBar('Quarterly numbers')
  await flush()
  expect(plugin.calls).toEqual([['configure', { title: 'Quarterly numbers', dark: true }]])

  dialogs = [{ popover: true }]
  await mutate()
  expect(plugin.calls).toHaveLength(1)
  dialogs = [{ popover: true }, { popover: false }]
  await mutate()
  dialogs = []
  await mutate()
  expect(plugin.calls.slice(1)).toEqual([
    ['set_hidden', { hidden: true }],
    ['set_hidden', { hidden: false }],
  ])

  setBarTitle('Quarterly numbers')
  setBarTitle('Q3 numbers')
  unmount()
  await flush()
  expect(plugin.calls.slice(3)).toEqual([
    ['set_title', { title: 'Q3 numbers' }],
    ['set_hidden', { hidden: true }],
  ])
  expect(observers).toHaveLength(0)
})

test('a header mounting while a sheet is open configures, then hides at once', async () => {
  stubIos()
  dialogs = [{ popover: false }]
  mountBar('t')()
  await flush()
  expect(plugin.calls).toEqual([
    ['configure', { title: 't', dark: false }],
    ['set_hidden', { hidden: true }],
  ])
})

test('the scroll button is sent only when it changes; positions are whole px, never negative', async () => {
  stubIos()
  setScrollButton(false, 80)
  setScrollButton(true, 80.4)
  setScrollButton(true, 80.2)
  setScrollButton(true, -5)
  setScrollButton(false, 0)
  setScrollButton(false, 10)
  await flush()
  expect(plugin.calls).toEqual([
    ['set_scroll_button', { visible: true, y: 80 }],
    ['set_scroll_button', { visible: true, y: 0 }],
    ['set_scroll_button', { visible: false, y: 0 }],
  ])
})

test('taps: one plugin listener serves every subscriber until it unsubscribes', async () => {
  stubIos()
  const a: string[] = []
  const b: string[] = []
  const offA = onNativeTap((id) => a.push(id))
  onNativeTap((id) => b.push(id))
  plugin.tap?.({ id: 'drawer' })
  offA()
  plugin.tap?.({ id: 'scroll_bottom' })
  expect(plugin.listeners).toBe(1)
  expect(a).toEqual(['drawer'])
  expect(b).toEqual(['drawer', 'scroll_bottom'])
})

test('the native button follows the composer on screen, whichever of the keyboard and the web layout moves first', async () => {
  stubIos()
  // An 852 pt iPhone. `edge` is the transcript's bottom = the composer's top edge (layout px); the visual viewport
  // has its own resize/scroll listeners; the scroller's ResizeObserver fires when the shell or the composer moves it.
  let edge = 732
  const vv = { height: 852, offsetTop: 0, listeners: [] as (() => void)[] }
  Object.assign(window, {
    visualViewport: { ...vv, get offsetTop() { return vv.offsetTop }, addEventListener: (_: string, f: () => void) => vv.listeners.push(f), removeEventListener: (_: string, f: () => void) => (vv.listeners = vv.listeners.filter((g) => g !== f)) },
  })
  let resized: (() => void) | null = null
  vi.stubGlobal('ResizeObserver', class { constructor(cb: () => void) { resized = cb } observe() {} disconnect() { resized = null } })
  const scroller = { getBoundingClientRect: () => ({ bottom: edge }) } as unknown as Element
  const at = (...y: number[]) => y.map((v) => ['set_scroll_button', { visible: true, y: v }])

  const stop = trackScrollButton(scroller, true)
  await flush()
  expect(plugin.calls).toEqual(at(720))
  // Keyboard up: the shell shrinks to the 516 px band and the composer rises with it.
  edge = 396
  resized!()
  // Keyboard dismissed while the web still has the composer raised: the visual viewport is full again but the
  // composer has not moved. The button stays above the composer (was: dropped onto it with the keyboard guide).
  for (const f of vv.listeners) f()
  await flush()
  expect(plugin.calls).toEqual(at(720, 384))
  // The shell catches up: the composer is back at the bottom, and so is the button.
  edge = 732
  resized!()
  // WebKit scrolled the visual viewport 40 px: on screen the composer sits 40 px higher.
  vv.offsetTop = 40
  for (const f of vv.listeners) f()
  await flush()
  expect(plugin.calls).toEqual(at(720, 384, 720, 680))
  stop()
  expect(resized).toBeNull()
  expect(vv.listeners).toHaveLength(0)
  expect(BAR).toBe(52)
})

test('iOS (any version): the native background follows the page’s <html> colour on every theme change; elsewhere nothing', async () => {
  let css = 'rgba(0, 0, 0, 0)'
  vi.stubGlobal('getComputedStyle', () => ({ backgroundColor: css }))
  stubIos('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)')
  Object.assign(window, { addEventListener() {}, removeEventListener() {} })
  plugin.supported = false
  const stop = syncPageBackground()
  // Before theme.css applies <html> is transparent: nothing sent.
  css = 'rgb(232, 220, 200)'
  await mutate()
  css = 'rgb(18, 16, 14)'
  await mutate()
  await mutate()
  expect(plugin.calls).toEqual([
    ['set_background', { red: 232, green: 220, blue: 200 }],
    ['set_background', { red: 18, green: 16, blue: 14 }],
  ])
  stop()
  expect(observers).toHaveLength(0)

  plugin.calls = []
  vi.stubGlobal('window', {})
  syncPageBackground()
  await flush()
  expect(plugin.calls).toEqual([])
})

test('covered: any dialog outside a popper counts', () => {
  const el = (popper: boolean) => ({ closest: () => (popper ? {} : null) })
  expect(covered({ querySelectorAll: () => [] as never })).toBe(false)
  expect(covered({ querySelectorAll: () => [el(true)] as never })).toBe(false)
  expect(covered({ querySelectorAll: () => [el(true), el(false)] as never })).toBe(true)
})
