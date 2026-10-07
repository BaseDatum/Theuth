// 123 (W3 owns this file): the `Heatmap` renderer, moved out of `registry.tsx` unchanged in step 0. W3 adds
// `palette: diverging` and `title` here (design 123 §4.1); keep the exported `Heatmap` name — `registry.tsx` mounts it.
import type { z } from 'zod'
import type { catalog } from '../catalog'

export type HeatmapData = z.infer<(typeof catalog)['data']['components']['Heatmap']['props']>

export function Heatmap({ props }: { props: HeatmapData }) {
  const max = props.max ?? Math.max(0, ...props.cells.flat())
  return (
    <div className="overflow-x-auto">
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
                const pct = max > 0 ? Math.round(Math.min(100, Math.max(0, (v / max) * 100))) : 0
                return (
                  <td
                    key={j}
                    title={`${r} · ${props.cols[j]}: ${v}`}
                    className="size-8 rounded-sm text-center"
                    style={{ backgroundColor: `color-mix(in oklab, var(--color-fg-accent) ${pct}%, transparent)` }}
                  >
                    {v}
                  </td>
                )
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
