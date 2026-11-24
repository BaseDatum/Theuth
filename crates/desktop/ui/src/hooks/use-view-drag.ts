import type { HTMLAttributes, PointerEvent } from 'react'
import { useTileHost } from '@/components/tiles/host'
import { usePanes } from '@/lib/panes'

/** Presses on these never start a pane drag: header buttons, the address field, selects, menus and tabs stay usable. */
export const INTERACTIVE =
  'button,a,input,select,textarea,label,[role=button],[role=combobox],[role=menuitem],[role=option],[role=tab],[role=switch],[data-no-tile-drag]'

/** The press landed on an interactive element inside the handle (not the handle itself). */
export function onInteractive(target: EventTarget | null, handle: Element): boolean {
  const hit = target instanceof Element ? target.closest(INTERACTIVE) : null
  return hit !== null && hit !== handle && handle.contains(hit)
}

/**
 * A pane's own header (a chat's, a web frame's toolbar, a tile's) as the Trellis drag handle for its view: a press on
 * it is handed to the view's tab (hidden while the view is alone in its panel, `tabbar: "auto"`), so Trellis runs the
 * drag — its 6 px slop (a plain click still clicks), drop previews, Escape to cancel. Only while the workspace holds
 * more than one view: then `region` is false and the header must drop `data-tauri-drag-region`, or macOS would take
 * the press to move the window. With one view (or outside the workspace, or on a phone) the header keeps moving the
 * window and `props` is empty. A press must start in the header's own DOM: React bubbles events out of portals (a
 * Select's list, a menu, a dialog mounted by the header), and those never drag.
 */
export function useViewDrag(): { region: boolean; props: HTMLAttributes<HTMLElement> } {
  const leaf = useTileHost()
  const many = usePanes((s) => s.ws !== null && leaf !== null && s.leaves.includes(leaf) && s.leaves.length > 1)
  if (!many || leaf === null) return { region: true, props: {} }
  return {
    region: false,
    props: {
      onPointerDown(e: PointerEvent<HTMLElement>) {
        if (e.button !== 0 || !e.currentTarget.contains(e.target as Node) || onInteractive(e.target, e.currentTarget)) return
        const root = usePanes.getState().ws?.element
        const tab = [...(root?.querySelectorAll<HTMLElement>('[data-trellis-part="tab"]') ?? [])].find((t) => t.dataset.view === leaf)
        if (!tab) return
        // Text under the header must not start a selection while the pane moves.
        e.preventDefault()
        // `MouseEvent` where the engine has no `PointerEvent` (jsdom): Trellis reads only the mouse fields and the id.
        const Press: typeof window.PointerEvent = typeof window.PointerEvent === 'function' ? window.PointerEvent : (window.MouseEvent as typeof window.PointerEvent)
        tab.dispatchEvent(
          new Press('pointerdown', {
            bubbles: true,
            cancelable: true,
            clientX: e.clientX,
            clientY: e.clientY,
            screenX: e.screenX,
            screenY: e.screenY,
            pointerId: e.pointerId,
            pointerType: e.pointerType,
            isPrimary: e.isPrimary,
            button: 0,
            buttons: 1,
          }),
        )
      },
    },
  }
}
