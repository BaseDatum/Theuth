import { useSyncExternalStore } from 'react'

const MOBILE_BREAKPOINT = 768
/** Tailwind's `md:` negated: the sidebar's desktop/Sheet split (`hidden md:block`) and this hook read one breakpoint. */
const QUERY = `(max-width: ${MOBILE_BREAKPOINT - 1}px)`

const subscribe = (cb: () => void) => {
  const mql = window.matchMedia(QUERY)
  mql.addEventListener('change', cb)
  return () => mql.removeEventListener('change', cb)
}
/** `useIsMobile` outside React: an event handler deciding whether the layout tiles. */
export const isMobileLayout = () => typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia(QUERY).matches
const snapshot = isMobileLayout

/**
 * Under `md` (768 px): the sidebar is a Sheet and the chat header carries its only toggle. Read from the media
 * query — never `window.innerWidth` — and synchronously on the first render. iOS WKWebView reports a stale or
 * desktop-sized `innerWidth` after a resume or a rotation (WebKit 170595, 262207) while the CSS `md:` rules follow
 * the query; a JS/CSS disagreement hid both the desktop sidebar and the toggle, trapping the phone in the chat
 * (MOBILE-NAV). The first-render `false` of a state-plus-effect version painted a toggle-less header on every remount.
 */
export function useIsMobile(): boolean {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}
