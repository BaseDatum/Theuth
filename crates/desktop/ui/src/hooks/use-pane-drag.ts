import type { HTMLAttributes, PointerEvent } from 'react'
import { useEffect, useRef } from 'react'
import { useTileHost } from '@/components/tiles/host'
import { usePressDrag } from '@/hooks/use-press-drag'
import { leaves, usePanes } from '@/lib/panes'

/** Presses on these never start a tile drag: header buttons, the address field, selects, menus and tabs stay usable. */
export const INTERACTIVE =
  'button,a,input,select,textarea,label,[role=button],[role=combobox],[role=menuitem],[role=option],[role=tab],[role=switch],[data-no-tile-drag]'

/** The press landed on an interactive element inside the handle (not the handle itself). */
export function onInteractive(target: EventTarget | null, handle: Element): boolean {
  const hit = target instanceof Element ? target.closest(INTERACTIVE) : null
  return hit !== null && hit !== handle && handle.contains(hit)
}

/**
 * A tile header as the drag handle for its own leaf (`Panes`' `Leaf` names it): the press-drag gesture
 * (`usePressDrag`: 6 px slop, so a plain click still clicks) feeds `usePanes.startDrag(…, move)`; the drop moves the
 * leaf to an edge of another or swaps it with the one under its middle. Only while the layout tiles and the tree has
 * more than one leaf: then `region` is false and the header must drop `data-tauri-drag-region`, or macOS would take
 * the press to move the window. With one leaf (or outside `Panes`, or on a phone) the header keeps moving the window
 * and `props` is empty. A press must start in the header's own DOM: React bubbles events out of portals (a Select's
 * list, a menu, a dialog mounted by the header), and those never drag. A drag in flight ends cancelled on Escape, when
 * the header unmounts, or when the tree stops being split.
 */
export function usePaneDrag(title: string): { region: boolean; props: HTMLAttributes<HTMLElement> } {
  const leaf = useTileHost()
  const split = usePanes((s) => s.root !== null && leaves(s.root).length > 1)
  const active = leaf !== null && split
  const { props, cancel } = usePressDrag({
    onStart: (x, y) => leaf !== null && usePanes.getState().startDrag(leaf, title, x, y, true),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd: (drop) => void usePanes.getState().endDrag(drop),
  })
  const cancelRef = useRef(cancel)
  cancelRef.current = cancel
  const dragging = usePanes((s) => s.drag?.move === true && s.drag.session === leaf) && active
  useEffect(() => {
    if (!dragging) return
    const key = (e: KeyboardEvent) => e.key === 'Escape' && cancelRef.current()
    window.addEventListener('keydown', key)
    return () => {
      window.removeEventListener('keydown', key)
      // Unmounted or no longer split mid-drag (after a normal drop the gesture is idle and this does nothing).
      cancelRef.current()
    }
  }, [dragging])
  if (!active) return { region: true, props: {} }
  // Not `data-drag-row` / `draggable` / `onDragStart`: the header holds inputs whose text must stay selectable.
  const { onPointerMove, onPointerUp, onPointerCancel, onClickCapture, onContextMenuCapture } = props
  return {
    region: false,
    props: {
      onPointerMove,
      onPointerUp,
      onPointerCancel,
      onClickCapture,
      onContextMenuCapture,
      onPointerDown(e: PointerEvent<HTMLElement>) {
        if (!e.currentTarget.contains(e.target as Node) || onInteractive(e.target, e.currentTarget)) return
        props.onPointerDown(e)
      },
    },
  }
}
