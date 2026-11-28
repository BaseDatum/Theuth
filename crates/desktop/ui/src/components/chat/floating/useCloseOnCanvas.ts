import { useEffect } from 'react'

/**
 * A menu over a canvas closes on a press on the canvas. Radix defers an outside press to its click, and React Flow's
 * d3-zoom stops the canvas's mousedown, so Radix reads the press as handled and leaves the menu open.
 */
export function useCloseOnCanvas(open: boolean, close: () => void, selector = '.wb-canvas') {
  useEffect(() => {
    if (!open) return
    const press = (e: PointerEvent) => e.target instanceof Element && e.target.closest(selector) && close()
    document.addEventListener('pointerdown', press, true)
    return () => document.removeEventListener('pointerdown', press, true)
  }, [open, close, selector])
}
