import { type RefObject, useEffect, useRef } from 'react'
import type { SearchOrigin } from '@/lib/search'

/** Activations whose destination already took focus; `leave` makes a fresh `origin` per activation, so the same hit activated again is a new key. */
const focused = new WeakSet<SearchOrigin>()

/**
 * Hand focus to the destination of a search activation (49 §7): the returned ref goes on the screen's visible
 * heading or detail (`tabIndex={-1}`), and it is focused once, when `origin` is this screen's activation and
 * `ready` says the element is rendered. Refetches, re-renders and ordinary navigations back to the same screen
 * never move focus; the dialog's close-autofocus is already prevented after an activation, so nothing pulls it back.
 */
export function useSearchFocus<T extends HTMLElement>(origin: SearchOrigin | null, ready: boolean): RefObject<T | null> {
  const ref = useRef<T>(null)
  useEffect(() => {
    const el = ref.current
    if (!origin || !ready || !el || focused.has(origin)) return
    focused.add(origin)
    el.focus()
  }, [origin, ready])
  return ref
}
