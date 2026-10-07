// Design 123 §5.1, §5.3 #2 — one portal tooltip per diagram: label, `sub`, values + unit, note, dates, guard.
// Shown for the hovered/focused unit (`useUnits().state.hover`); positioned from the unit's bounding box so pan/zoom
// inside `DiagramFrame` never misplaces it.
import { useLayoutEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { UNIT_ATTR } from './units'

export interface TooltipRow {
  label: string
  value?: string | number | null
}

const OFFSET = 8

/**
 * The tooltip for unit `hover` (null hides it): `title` bold, then `rows` as `label value`. `root` is the svg's
 * container (or any ancestor) the unit is looked up in; the box sits above the unit, flipped below near the top.
 */
export function DiagramTooltip({ hover, root, title, rows = [] }: { hover: string | null; root: Element | null; title: string; rows?: readonly TooltipRow[] }) {
  const [at, setAt] = useState<{ x: number; y: number; below: boolean } | null>(null)
  useLayoutEffect(() => {
    if (hover === null || !root) {
      setAt(null)
      return
    }
    const el = root.querySelector(`[${UNIT_ATTR}="${CSS.escape(hover)}"]`)
    const r = el?.getBoundingClientRect()
    if (!r) {
      setAt(null)
      return
    }
    const below = r.top < 64
    setAt({ x: r.left + r.width / 2, y: below ? r.bottom + OFFSET : r.top - OFFSET, below })
  }, [hover, root])
  if (!at || typeof document === 'undefined') return null
  return createPortal(
    <div
      role="tooltip"
      className="pointer-events-none fixed z-50 max-w-64 rounded-md border border-border-subtle bg-layer-1 px-2.5 py-1.5 text-xs shadow-md"
      style={{ left: at.x, top: at.y, transform: at.below ? 'translate(-50%, 0)' : 'translate(-50%, -100%)' }}
    >
      <div className="font-medium">{title}</div>
      {rows
        .filter((r) => r.value !== undefined && r.value !== null && r.value !== '')
        .map((r, i) => (
          <div key={i} className="flex justify-between gap-3 text-fg-secondary">
            <span>{r.label}</span>
            <span className="font-mono tabular-nums">{r.value}</span>
          </div>
        ))}
    </div>,
    document.body,
  )
}
