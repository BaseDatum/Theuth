// The native layer of a web view frame (205 §4.4, §9): the shell's view `web-<id>` is laid over a DOM placeholder and
// follows it. Shared by the tile (`WebFrame`, iPad and desktop) and the phone's full-screen page (`PhoneWebView`, 209 §7).

import { type RefObject, useEffect, useRef, useState } from 'react'
import { usePanes } from '@/lib/panes'
import * as native from '@/lib/web-view-native'
import { useWebViews } from '@/lib/web-views'

/** Open things a native web view would draw over (it sits above all DOM): radix dialogs, sheets, menus, popovers,
 * selects, the modal backdrops and toasts. Tooltips are not among them: they come and go with the pointer. */
const OVERLAYS = [
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[data-radix-popper-content-wrapper]',
  '[data-slot="sheet-content"]',
  '[data-slot$="-overlay"]',
  '[data-sonner-toast]',
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

/** The native layer must not show: a pane drag is in flight (a sidebar row's or a Trellis one: its previews are DOM)
 * or an overlay covers the frame. */
function useOccluded(ref: RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const dragging = usePanes((s) => s.drag !== null || s.dragging)
  return useOverlayOver(ref, enabled) || (enabled && dragging)
}

/** `el`'s viewport rectangle, as the shell takes it. */
export const rectOf = (el: HTMLElement): native.Rect => {
  const r = el.getBoundingClientRect()
  return { x: r.left, y: r.top, width: r.width, height: r.height }
}
const sameRect = (a: native.Rect, b: native.Rect | null) => b !== null && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height

export const warn = (what: string) => (e: unknown) => console.warn(what, e)

/**
 * Lay frame `id`'s native view over the placeholder `ref` while `live`: mounting attaches it at the placeholder's
 * rectangle (an existing view is moved and shown, never navigated), a per-frame check follows the placeholder while it
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

  // Hide under an overlay or a drag, show again once it is gone; the module sends only what changed.
  useEffect(() => {
    if (live) void native.visible(id, !occluded).catch(warn('web_view_visible'))
  }, [id, live, occluded])

  // Follow the placeholder (resizes, splits, scrolls, sidebar moves, the keyboard) once per frame while it shows. The
  // rectangle is sent once first: it may have changed while the layer was hidden (an overlay, a rotation under it), and
  // showing again would otherwise put the view at its old frame.
  useEffect(() => {
    const el = ref.current
    if (!live || occluded || !el) return
    let last: native.Rect | null = rectOf(el)
    void native.bounds(id, last).catch(warn('web_view_bounds'))
    let frame = requestAnimationFrame(function tick() {
      const r = rectOf(el)
      if (!sameRect(r, last)) {
        last = r
        void native.bounds(id, r).catch(warn('web_view_bounds'))
      }
      frame = requestAnimationFrame(tick)
    })
    return () => cancelAnimationFrame(frame)
  }, [id, live, occluded, ref])

  return occluded
}
