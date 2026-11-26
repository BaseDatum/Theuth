// The native layer of a web view frame (205 §4.4, §9): the shell's view `web-<id>` is laid over a DOM placeholder and
// follows it. Shared by the tile (`WebFrame`, iPad and desktop) and the phone's full-screen page (`PhoneWebView`, 209 §7).

import { useOptionalView } from '@danfessler/trellis-react'
import { type RefObject, useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { usePanes } from '@/lib/panes'
import * as native from '@/lib/web-view-native'
import { useWebViews } from '@/lib/web-views'
import { SIDEBAR_SLIDE_EVENT } from '@/lib/sidebar-slide'

/** Open things a native web view would draw over (it sits above all DOM): radix dialogs, sheets, menus, popovers,
 * selects, the modal backdrops and the notice history. Tooltips are not among them: they come and go with the pointer. */
const OVERLAYS = [
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[data-radix-popper-content-wrapper]',
  '[data-slot="sheet-content"]',
  '[data-slot$="-overlay"]',
  '[data-notice-popover]',
].join(',')

const isTooltip = (el: Element) => el.querySelector('[role="tooltip"],[data-slot="tooltip-content"]') !== null

const intersects = (a: DOMRect, b: DOMRect) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom

/** A mutation inside the transcript's virtual rows (streaming text, cards) never opens an overlay. */
const inTranscript = (n: Node) => (n instanceof Element ? n : n.parentElement)?.closest('[data-index]') != null

/** Some open overlay intersects `ref`'s box. Watches `document.body` for overlays mounting, unmounting or changing
 * state (`data-state`), one check per animation frame. */
function useOverlayOver(ref: RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const [over, setOver] = useState(false)
  useEffect(() => {
    if (!enabled) {
      setOver(false)
      return
    }
    let frame = 0
    const check = () => {
      frame = 0
      const el = ref.current
      if (!el) return
      const box = el.getBoundingClientRect()
      setOver([...document.querySelectorAll(OVERLAYS)].some((o) => !el.contains(o) && !isTooltip(o) && intersects(o.getBoundingClientRect(), box)))
    }
    const mo = new MutationObserver((records) => {
      if (!frame && records.some((r) => !inTranscript(r.target))) frame = requestAnimationFrame(check)
    })
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-state'] })
    check()
    return () => {
      mo.disconnect()
      cancelAnimationFrame(frame)
    }
  }, [ref, enabled])
  return over
}

/** The Trellis view this frame renders in is on screen: the active tab of its panel, and that panel not framed away
 * by zoom navigation. Trellis keeps a hidden view mounted at its panel's rectangle (only `visibility: hidden`), so the
 * rectangle alone cannot tell. Always outside a workspace (a phone, a pop-out window). */
function useViewShown(): boolean {
  const view = useOptionalView()
  const subscribe = useCallback((changed: () => void) => view?.on('visibility', changed) ?? (() => undefined), [view])
  return useSyncExternalStore(subscribe, () => view?.visible ?? true)
}

/** The native layer must not show: its view is not on screen (an inactive tab, a pane framed away), a pane drag is in
 * flight (a sidebar row's or a Trellis one: its previews are DOM), zoom navigation is scaling the panes (a native view
 * cannot scale with them; it comes back at the settled rectangle), or an overlay covers the frame. The layer is only
 * hidden, never detached: the page keeps its state. */
function useOccluded(ref: RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const moving = usePanes((s) => s.drag !== null || s.dragging || s.zooming)
  const shown = useViewShown()
  return useOverlayOver(ref, enabled) || (enabled && (moving || !shown))
}

/** `el`'s viewport rectangle, as the shell takes it. */
export const rectOf = (el: HTMLElement): native.Rect => {
  const r = el.getBoundingClientRect()
  return { x: r.left, y: r.top, width: r.width, height: r.height }
}
const sameRect = (a: native.Rect, b: native.Rect | null) => b !== null && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height

/** Unchanged frames before `followRect` drops from every frame to `FOLLOW_IDLE_MS` polls. */
export const FOLLOW_FRAMES = 30
export const FOLLOW_IDLE_MS = 250

/**
 * Call `moved` with `el`'s rectangle whenever it changes, until the returned stop. Every frame while it moves, and for
 * `FOLLOW_FRAMES` frames after; then a poll every `FOLLOW_IDLE_MS`, so an idle frame does not keep a display-rate loop
 * running (on iPad that held the compositor busy for as long as a web view was open). A resize of `el`, a window resize,
 * a scroll anywhere, a transition or animation starting, or the sidebar's slide (`SIDEBAR_SLIDE_EVENT`: Web Animations
 * fire no event of their own) goes straight back to every frame; the poll catches what moves without any of those, one
 * poll late at worst. `moved` does not get the starting rectangle.
 */
export function followRect(el: HTMLElement, moved: (r: native.Rect) => void): () => void {
  let last = rectOf(el)
  let still = 0
  let frame = 0
  let poll: ReturnType<typeof setTimeout> | undefined
  const check = () => {
    const r = rectOf(el)
    if (sameRect(r, last)) return false
    last = r
    moved(r)
    return true
  }
  const tick = () => {
    frame = 0
    still = check() ? 0 : still + 1
    if (still < FOLLOW_FRAMES) frame = requestAnimationFrame(tick)
    else poll = setTimeout(idle, FOLLOW_IDLE_MS)
  }
  const idle = () => {
    poll = undefined
    if (check()) wake()
    else poll = setTimeout(idle, FOLLOW_IDLE_MS)
  }
  const wake = () => {
    still = 0
    if (frame) return
    clearTimeout(poll)
    poll = undefined
    frame = requestAnimationFrame(tick)
  }
  const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(wake) : null
  ro?.observe(el)
  const opts = { capture: true, passive: true } as const
  const events = ['resize', 'scroll', 'transitionrun', 'animationstart', SIDEBAR_SLIDE_EVENT] as const
  for (const e of events) window.addEventListener(e, wake, opts)
  window.visualViewport?.addEventListener('resize', wake)
  frame = requestAnimationFrame(tick)
  return () => {
    cancelAnimationFrame(frame)
    clearTimeout(poll)
    ro?.disconnect()
    for (const e of events) window.removeEventListener(e, wake, opts)
    window.visualViewport?.removeEventListener('resize', wake)
  }
}

export const warn = (what: string) => (e: unknown) => console.warn(what, e)

/**
 * Lay frame `id`'s native view over the placeholder `ref` while `live`: mounting attaches it at the placeholder's
 * rectangle (an existing view is moved and shown, never navigated), `followRect` follows the placeholder while it
 * shows, an overlay or a pane drag hides it, unmounting hides it (`detach`); the store's `close` alone destroys it.
 * `onAttachError` gets a refused attach. Answers whether the layer is occluded (the placeholder then shows its face).
 */
export function useNativeLayer(id: string, ref: RefObject<HTMLElement | null>, live: boolean, onAttachError: (e: unknown) => void): boolean {
  const occluded = useOccluded(ref, live)
  const failed = useRef(onAttachError)
  failed.current = onAttachError

  useEffect(() => {
    const el = ref.current
    if (!live || !el) return
    const start = useWebViews.getState().views[id]
    native.attach(id, start?.current || start?.url || '', rectOf(el)).catch((e: unknown) => failed.current(e))
    return () => void native.detach(id).catch(warn('web_view_visible'))
  }, [id, live, ref])

  // Follow the placeholder (resizes, splits, scrolls, sidebar moves, the keyboard) while it shows (`followRect`). The
  // rectangle is sent once first, before the layer shows again (the effect below): it may have changed while the layer
  // was hidden (an overlay, a zoom, another tab in front, a rotation under it), and showing again would otherwise put
  // the view at its old frame.
  useEffect(() => {
    const el = ref.current
    if (!live || occluded || !el) return
    void native.bounds(id, rectOf(el)).catch(warn('web_view_bounds'))
    return followRect(el, (r) => void native.bounds(id, r).catch(warn('web_view_bounds')))
  }, [id, live, occluded, ref])

  // Hide under an overlay, a drag or a zoom, or behind another tab; show again once that is over. The module sends
  // only what changed.
  useEffect(() => {
    if (live) void native.visible(id, !occluded).catch(warn('web_view_visible'))
  }, [id, live, occluded])

  return occluded
}
