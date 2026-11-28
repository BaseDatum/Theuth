import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'

// Switch between real React `useSyncExternalStore` (for SSR `renderToStaticMarkup` tests)
// and a client-mode driver that calls `subscribe` + `getSnapshot` in Node without jsdom.
let clientMode = false
const activeUnsubs: (() => void)[] = []
let notifyCount = 0

vi.mock('react', async (original) => {
  const actual = await original<typeof import('react')>()
  return {
    ...actual,
    useSyncExternalStore<T>(
      subscribe: (onStoreChange: () => void) => () => void,
      getSnapshot: () => T,
      getServerSnapshot?: () => T,
    ): T {
      if (!clientMode) {
        return actual.useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
      }
      const unsub = subscribe(() => {
        notifyCount += 1
      })
      activeUnsubs.push(unsub)
      return getSnapshot()
    },
  }
})

let mockDesktop = false
let mockIos = false
vi.mock('@/lib/platform', () => ({
  isDesktop: () => mockDesktop,
  isIos: () => mockIos,
}))

let hwQueryResolver: ((v: boolean) => void) | null = null
let hwEventCb: ((connected: boolean) => void) | null = null
const hwUnlisten = vi.fn()
const mockHardwareKeyboardConnected = vi.fn(
  () =>
    new Promise<boolean>((resolve) => {
      hwQueryResolver = resolve
    }),
)
const mockOnHardwareKeyboard = vi.fn(async (cb: (connected: boolean) => void) => {
  hwEventCb = cb
  return hwUnlisten
})

vi.mock('@/lib/tauri', () => ({
  hardwareKeyboardConnected: () => mockHardwareKeyboardConnected(),
  onHardwareKeyboard: (cb: (connected: boolean) => void) => mockOnHardwareKeyboard(cb),
}))

import {
  _resetHardwareKeyboardForTest,
  initHardwareKeyboard,
  useHardwareKeyboard,
  useNeedsTerminalKeys,
  usePointerCoarse,
} from './mobile'

type MqListener = () => void
let coarseMatches = false
let mobileMatches = false
const coarseListeners = new Set<MqListener>()
const mobileListeners = new Set<MqListener>()

function installMatchMedia() {
  const mql = (query: string) => {
    const isCoarse = query.includes('pointer: coarse')
    const listeners = isCoarse ? coarseListeners : mobileListeners
    return {
      media: query,
      get matches() {
        return isCoarse ? coarseMatches : mobileMatches
      },
      addEventListener(_type: string, cb: MqListener) {
        listeners.add(cb)
      },
      removeEventListener(_type: string, cb: MqListener) {
        listeners.delete(cb)
      },
    }
  }
  const win = { matchMedia: mql }
  Object.defineProperty(globalThis, 'window', { configurable: true, writable: true, value: win })
  Object.defineProperty(globalThis, 'matchMedia', { configurable: true, writable: true, value: mql })
}

function clearMatchMedia() {
  // @ts-expect-error clean up Node test globals
  delete globalThis.window
  // @ts-expect-error clean up Node test globals
  delete globalThis.matchMedia
}

const flushMicrotasks = async () => {
  await Promise.resolve()
  await Promise.resolve()
}

function readClient<T>(hook: () => T): T {
  clientMode = true
  try {
    return hook()
  } finally {
    clientMode = false
  }
}

function cleanupSubs() {
  while (activeUnsubs.length > 0) activeUnsubs.pop()?.()
}

beforeEach(() => {
  vi.clearAllMocks()
  _resetHardwareKeyboardForTest()
  cleanupSubs()
  notifyCount = 0
  mockDesktop = false
  mockIos = false
  coarseMatches = false
  mobileMatches = false
  coarseListeners.clear()
  mobileListeners.clear()
  hwQueryResolver = null
  hwEventCb = null
  installMatchMedia()
})

afterEach(() => {
  cleanupSubs()
  _resetHardwareKeyboardForTest()
  clearMatchMedia()
})

test('iOS without hardware keyboard shows terminal keys even on wide iPad viewport with fine pointer', async () => {
  mockIos = true
  mobileMatches = false
  coarseMatches = false

  // Subscribe lazily triggers initHardwareKeyboard()
  expect(readClient(useNeedsTerminalKeys)).toBe(false) // pending IPC fallback: coarse || mobile
  expect(mockOnHardwareKeyboard).toHaveBeenCalledTimes(1)
  expect(mockHardwareKeyboardConnected).toHaveBeenCalledTimes(1)

  hwQueryResolver!(false)
  await flushMicrotasks()

  expect(notifyCount).toBeGreaterThan(0)
  expect(readClient(useHardwareKeyboard)).toBe(false)
  expect(readClient(useNeedsTerminalKeys)).toBe(true)
})

test('iOS with hardware keyboard hides terminal keys even with coarse pointer or narrow viewport', async () => {
  mockIos = true
  mobileMatches = true
  coarseMatches = true

  initHardwareKeyboard()
  hwQueryResolver!(true)
  await flushMicrotasks()

  expect(readClient(useHardwareKeyboard)).toBe(true)
  expect(readClient(useNeedsTerminalKeys)).toBe(false)
})

test('iOS live connect and disconnect events update subscribers and are not overwritten by stale initial query', async () => {
  mockIos = true
  mobileMatches = false
  coarseMatches = true

  // Subscribe before initial query resolves
  expect(readClient(useNeedsTerminalKeys)).toBe(true)
  const beforeEvent = notifyCount

  // Event arrives BEFORE the initial hardwareKeyboardConnected() promise resolves
  hwEventCb!(true)
  expect(notifyCount).toBeGreaterThan(beforeEvent)
  expect(readClient(useHardwareKeyboard)).toBe(true)
  expect(readClient(useNeedsTerminalKeys)).toBe(false)

  // Stale initial query resolves with `false` — must NOT overwrite the newer event
  hwQueryResolver!(false)
  await flushMicrotasks()
  expect(readClient(useHardwareKeyboard)).toBe(true)
  expect(readClient(useNeedsTerminalKeys)).toBe(false)

  // Live disconnect flips toolbar back on
  const beforeDisconnect = notifyCount
  hwEventCb!(false)
  expect(notifyCount).toBeGreaterThan(beforeDisconnect)
  expect(readClient(useHardwareKeyboard)).toBe(false)
  expect(readClient(useNeedsTerminalKeys)).toBe(true)
})

test('iOS pending IPC falls back to coarse || mobile until resolved', () => {
  mockIos = true

  coarseMatches = false
  mobileMatches = false
  expect(readClient(useHardwareKeyboard)).toBeNull()
  expect(readClient(useNeedsTerminalKeys)).toBe(false)

  coarseMatches = true
  mobileMatches = false
  expect(readClient(useNeedsTerminalKeys)).toBe(true)

  coarseMatches = false
  mobileMatches = true
  expect(readClient(useNeedsTerminalKeys)).toBe(true)
})

test('desktop shell never shows terminal keys even with touch/coarse pointer and narrow viewport', () => {
  mockDesktop = true
  coarseMatches = true
  mobileMatches = true

  expect(readClient(useNeedsTerminalKeys)).toBe(false)
  expect(mockHardwareKeyboardConnected).not.toHaveBeenCalled()
  expect(mockOnHardwareKeyboard).not.toHaveBeenCalled()
})

test('web container uses coarse pointer or mobile viewport (iPad Safari vs desktop browser)', () => {
  mockDesktop = false
  mockIos = false

  coarseMatches = false
  mobileMatches = false
  expect(readClient(useNeedsTerminalKeys)).toBe(false)

  coarseMatches = true
  mobileMatches = false
  expect(readClient(useNeedsTerminalKeys)).toBe(true)

  coarseMatches = false
  mobileMatches = true
  expect(readClient(useNeedsTerminalKeys)).toBe(true)
})

test('SSR snapshot is safe without window/matchMedia and returns conservative defaults', () => {
  clearMatchMedia()
  mockIos = true

  let ssrCoarse: boolean | undefined
  let ssrHw: boolean | null | undefined
  let ssrNeeds: boolean | undefined

  function Probe() {
    ssrCoarse = usePointerCoarse()
    ssrHw = useHardwareKeyboard()
    ssrNeeds = useNeedsTerminalKeys()
    return createElement('span', null, String(ssrNeeds))
  }

  const html = renderToStaticMarkup(createElement(Probe))
  expect(html).toBe('<span>false</span>')
  expect(ssrCoarse).toBe(false)
  expect(ssrHw).toBeNull()
  expect(ssrNeeds).toBe(false)
})
