/**
 * Coarse-pointer detection and the iOS keyboard inset for the editor's mobile toolbar (59 §10).
 * `lib/viewport.ts` shrinks the *shell* to the visible band; this is the complementary number a
 * `position: fixed` bar needs: how far the layout viewport's bottom is below the visual one.
 */
import { useEffect, useState, useSyncExternalStore } from 'react'
import { useIsMobile } from '@/hooks/use-mobile'
import { isDesktop, isIos } from '@/lib/platform'
import { hardwareKeyboardConnected, onHardwareKeyboard } from '@/lib/tauri'

const COARSE = '(pointer: coarse)'

const subscribeCoarse = (cb: () => void) => {
  if (typeof matchMedia !== 'function') return () => {}
  const mq = matchMedia(COARSE)
  mq.addEventListener('change', cb)
  return () => mq.removeEventListener('change', cb)
}

const readCoarse = () => typeof matchMedia === 'function' && matchMedia(COARSE).matches

/** `pointer: coarse`, read at mount and re-read on `change` (a mouse plugged into an iPad). */
export const usePointerCoarse = (): boolean => useSyncExternalStore(subscribeCoarse, readCoarse, () => false)

let hwKeyboard: boolean | null = null
let hwInitStarted = false
let hwEventSeen = false
let hwGeneration = 0
let hwUnlisten: (() => void) | null = null
const hwListeners = new Set<() => void>()

const setHardwareKeyboard = (connected: boolean) => {
  if (hwKeyboard === connected) return
  hwKeyboard = connected
  for (const fn of hwListeners) fn()
}

/**
 * Start tracking iOS hardware keyboard attach/detach state (`GCKeyboard`).
 * Idempotent; registers the event listener before querying initial state so an
 * event arriving mid-query is never overwritten by a stale initial query result.
 */
export function initHardwareKeyboard(): void {
  if (hwInitStarted || !isIos()) return
  hwInitStarted = true
  const gen = hwGeneration
  void onHardwareKeyboard((connected) => {
    if (gen !== hwGeneration) return
    hwEventSeen = true
    setHardwareKeyboard(connected)
  })
    .then((unlisten) => {
      if (gen !== hwGeneration) {
        unlisten()
        return
      }
      hwUnlisten = unlisten
    })
    .catch(() => undefined)
  void hardwareKeyboardConnected()
    .then((connected) => {
      if (gen !== hwGeneration || hwEventSeen) return
      setHardwareKeyboard(connected)
    })
    .catch(() => undefined)
}

const subscribeHardwareKeyboard = (cb: () => void) => {
  initHardwareKeyboard()
  hwListeners.add(cb)
  return () => {
    hwListeners.delete(cb)
  }
}

const readHardwareKeyboard = (): boolean | null => hwKeyboard

/** Whether an iOS hardware keyboard is attached (`null` while the initial IPC is pending or off iOS). */
export const useHardwareKeyboard = (): boolean | null =>
  useSyncExternalStore(subscribeHardwareKeyboard, readHardwareKeyboard, () => null)

/**
 * Whether a terminal needs the virtual accessory key toolbar (`Esc`, `Tab`, arrows, `^C`, keyboard toggle):
 * - Desktop shell: never (even on touchscreen laptops).
 * - iOS shell (iPhone / iPad): whenever no hardware keyboard is attached (`!hwKeyboard`);
 *   while the initial native check is in flight (`null`), falls back to `coarse || mobile`.
 * - Web (including iPad Safari): whenever pointer is coarse or layout is narrow (`coarse || mobile`).
 */
export function useNeedsTerminalKeys(): boolean {
  const mobile = useIsMobile()
  const coarse = usePointerCoarse()
  const hw = useHardwareKeyboard()
  if (isDesktop()) return false
  if (isIos()) return hw !== null ? !hw : coarse || mobile
  return coarse || mobile
}

/** Reset module-level hardware keyboard state between unit tests. */
export function _resetHardwareKeyboardForTest(): void {
  hwGeneration += 1
  hwUnlisten?.()
  hwUnlisten = null
  hwKeyboard = null
  hwInitStarted = false
  hwEventSeen = false
  hwListeners.clear()
}

/**
 * Pixels between the layout viewport's bottom edge and the visual viewport's: the keyboard's height when
 * WebKit shrinks only the visual viewport, 0 when it shrinks the layout too (or there is no keyboard).
 */
export function keyboardInset(innerHeight: number, viewport: { height: number; offsetTop: number } | null | undefined): number {
  if (!viewport) return 0
  return Math.max(0, Math.round(innerHeight - viewport.height - viewport.offsetTop))
}

declare global {
  interface Window {
    /** Test hook: a fixed inset in px (Puppeteer cannot shrink `visualViewport`). */
    __theuthKbInset?: number
  }
}

const readInset = () => window.__theuthKbInset ?? keyboardInset(window.innerHeight, window.visualViewport)

/** The keyboard inset in px, live on `visualViewport` `resize`/`scroll`. Safe area is added in CSS (`--safe-bottom`). */
export function useKeyboardInset(): number {
  const [inset, setInset] = useState(readInset)
  useEffect(() => {
    const vv = window.visualViewport
    const update = () => setInset(readInset())
    update()
    vv?.addEventListener('resize', update)
    vv?.addEventListener('scroll', update)
    window.addEventListener('resize', update)
    return () => {
      vv?.removeEventListener('resize', update)
      vv?.removeEventListener('scroll', update)
      window.removeEventListener('resize', update)
    }
  }, [])
  return inset
}
