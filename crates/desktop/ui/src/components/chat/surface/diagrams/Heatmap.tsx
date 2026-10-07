// 123 §4.1 / §5.2 (W3): the `Heatmap` renderer stays HTML. Adds `title`, `palette: diverging` (negative cells mix
// `--chart-3`, positive the accent, intensity |v| / max, a `−max 0 +max` legend) and the §5.3 unit contract on cells.
// `registry.tsx` mounts the exported `Heatmap` by name.
import { type CSSProperties, useMemo } from 'react'
import type { z } from 'zod'
import { useUnits } from '@/lib/diagram/units'
import type { catalog } from '../catalog'

export type HeatmapData = z.infer<(typeof catalog)['data']['components']['Heatmap']['props']>

/** Unit ids in authored order: `${row}:${col}` indices of the cells present. */
export function heatmapUnits(props: HeatmapData): string[] {
  return props.rows.flatMap((_, i) => (props.cells[i] ?? []).map((_, j) => `${i}:${j}`))
}

export function Heatmap({ props, highlight, onSelect }: { props: HeatmapData; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => heatmapUnits(props), [props])
  // ponytail: `focusUnit` looks for the nearest <svg>, so arrow keys are a no-op in this HTML table; Tab still works.
  const u = useUnits({ ids, highlight, onSelect })
  const diverging = props.palette === 'diverging'
  const max = props.max ?? Math.max(0, ...props.cells.flat().map((v) => (diverging ? Math.abs(v) : v)))
  return (
    <div {...u.root} className="flex flex-col gap-1 overflow-x-auto">
      {props.title && <div className="text-sm font-medium">{props.title}</div>}
      <table className="border-separate border-spacing-0.5 text-xs tabular-nums">
        <thead>
          <tr>
            <th />
            {props.cols.map((c, j) => (
              <th key={j} className="px-1 pb-1 font-normal text-fg-secondary">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {props.rows.map((r, i) => (
            <tr key={i}>
              <th scope="row" className="pr-2 text-left font-normal text-fg-secondary">
                {r}
              </th>
              {props.cells[i]?.map((v, j) => {
                const id = `${i}:${j}`
                const pct = max > 0 ? Math.round(Math.min(100, Math.max(0, ((diverging ? Math.abs(v) : v) / max) * 100))) : 0
                const hue = diverging && v < 0 ? 'var(--chart-3, #3b5b8c)' : 'var(--color-fg-accent)'
                const paint = u.paint(id)
                const unit = u.unit(id, `${r} · ${props.cols[j]}: ${v}`)
                const style: CSSProperties = { ...unit.style, backgroundColor: `color-mix(in oklab, ${hue} ${pct}%, transparent)` }
                if (paint === 'active') style.outline = '2px solid var(--fg-accent)'
                return (
                  <td key={j} title={unit['aria-label']} {...unit} style={style} className={`size-8 rounded-sm text-center${paint === 'muted' ? ' opacity-50' : ''}`}>
                    {/* `title` first: registry.test.ts pins `title="…"[^>]*background-color` */}
                    {v}
                  </td>
                )
              })}
            </tr>
          ))}
        </tbody>
      </table>
      {diverging && (
        <div className="flex justify-between text-[10px] text-fg-tertiary tabular-nums" aria-hidden>
          <span>−{max}</span>
          <span>0</span>
          <span>+{max}</span>
        </div>
      )}
    </div>
  )
}
