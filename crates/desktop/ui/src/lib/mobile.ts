/**
 * Coarse-pointer detection and the iOS keyboard inset for the editor's mobile toolbar (59 §10).
 * `lib/viewport.ts` shrinks the *shell* to the visible band; this is the complementary number a
 * `position: fixed` bar needs: how far the layout viewport's bottom is below the visual one.
 */
import { useEffect, useState, useSyncExternalStore } from 'react'

const COARSE = '(pointer: coarse)'

const subscribeCoarse = (cb: () => void) => {
  const mq = matchMedia(COARSE)
  mq.addEventListener('change', cb)
  return () => mq.removeEventListener('change', cb)
}

/** `pointer: coarse`, read at mount and re-read on `change` (a mouse plugged into an iPad). */
export const usePointerCoarse = (): boolean => useSyncExternalStore(subscribeCoarse, () => matchMedia(COARSE).matches)

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
