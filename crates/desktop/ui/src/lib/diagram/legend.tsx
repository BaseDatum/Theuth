// Design 123 §5.1, §5.3 #5 — a horizontal bottom strip: swatch + label per series/state; click toggles locally
// (`aria-pressed`), export respects the toggle because hidden series are simply not drawn.
import { useCallback, useState } from 'react'
import type { DiagramTokens } from '../diagram-theme'

export interface LegendItem {
  id: string
  label: string
  color: string
  /** `line` draws a short stroke (dashed when `dashed`), `swatch` a filled square. */
  shape?: 'swatch' | 'line'
  dashed?: boolean
}

export const LEGEND_PX = 11
/** Height the strip takes under a plot. */
export const LEGEND_H = 22
const SWATCH = 10
const GAP = 14
const CHAR_W = 6.2

/** Local toggle state for legend entries: `hidden` ids, `toggle(id)`, `visible(id)`. */
export function useLegend(): { hidden: readonly string[]; toggle: (id: string) => void; visible: (id: string) => boolean } {
  const [hidden, setHidden] = useState<readonly string[]>([])
  const toggle = useCallback((id: string) => setHidden((h) => (h.includes(id) ? h.filter((x) => x !== id) : [...h, id])), [])
  const visible = useCallback((id: string) => !hidden.includes(id), [hidden])
  return { hidden, toggle, visible }
}

/** Approximate strip width for `items`, so a chart can centre it. */
export function legendWidth(items: readonly LegendItem[]): number {
  return items.reduce((w, it) => w + SWATCH + 5 + it.label.length * CHAR_W + GAP, 0) - GAP
}

/**
 * The strip at `(x, y)` (top-left). Entries are `role="button"` with `aria-pressed` = shown; `onToggle` absent
 * draws a static legend.
 */
export function Legend({ items, x, y, hidden = [], onToggle, tokens: t }: { items: readonly LegendItem[]; x: number; y: number; hidden?: readonly string[]; onToggle?: (id: string) => void; tokens: DiagramTokens }) {
  let cx = x
  return (
    <g transform={`translate(0 ${y})`} fontFamily={t.font} fontSize={LEGEND_PX}>
      {items.map((it) => {
        const off = hidden.includes(it.id)
        const left = cx
        cx += SWATCH + 5 + it.label.length * CHAR_W + GAP
        const press = onToggle ? () => onToggle(it.id) : undefined
        return (
          <g
            key={it.id}
            transform={`translate(${left} 0)`}
            opacity={off ? 0.45 : 1}
            data-legend={it.id}
            {...(press
              ? {
                  role: 'button',
                  tabIndex: 0,
                  'aria-pressed': !off,
                  'aria-label': `${it.label}${off ? ' (hidden)' : ''}`,
                  style: { cursor: 'pointer' },
                  onClick: press,
                  onKeyDown: (e: React.KeyboardEvent) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault()
                      press()
                    }
                  },
                }
              : {})}
          >
            <rect x={-4} y={0} width={SWATCH + 5 + it.label.length * CHAR_W + 8} height={LEGEND_H} fill="transparent" />
            {it.shape === 'line' ? (
              <line x1={0} y1={LEGEND_H / 2} x2={SWATCH + 4} y2={LEGEND_H / 2} stroke={it.color} strokeWidth={2} strokeDasharray={it.dashed ? '4 3' : undefined} />
            ) : (
              <rect x={0} y={(LEGEND_H - SWATCH) / 2} width={SWATCH} height={SWATCH} rx={2} fill={it.color} />
            )}
            <text x={SWATCH + 5} y={LEGEND_H / 2} dy="0.32em" fill={t.mutedForeground} textDecoration={off ? 'line-through' : undefined}>
              {it.label}
            </text>
          </g>
        )
      })}
    </g>
  )
}
