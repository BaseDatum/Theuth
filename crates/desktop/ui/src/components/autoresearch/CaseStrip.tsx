import { useMemo, useState } from 'react'
import type { CaseRow } from "@/rpc/bindings/method.autoresearch.output"
import type { KindView } from "@/rpc/bindings/method.autoresearch.output"

type CasesView = Extract<KindView, { kind: 'cases' }>
const SPLITS = ['val', 'holdout', 'train'] as const
const CELL = 14
const LABEL = 150
const STAIR_H = 120

/** Mean of a split's scores at experiment `n` (0 = the base); `null` when no case of the split was scored at `n`. */
export function splitMeans(rows: readonly CaseRow[], split: string, ns: readonly number[]): Array<number | null> {
  return ns.map((n) => {
    const vals = rows.filter((r) => r.split === split).map((r) => r.values[n]).filter((v): v is number => typeof v === 'number')
    return vals.length ? vals.reduce((a, b) => a + b, 0) / vals.length : null
  })
}

/** The experiment numbers any row has a value for, ascending (0 = the base first). */
export function experimentNumbers(rows: readonly CaseRow[]): number[] {
  const ns = new Set<number>()
  for (const r of rows) for (const k of Object.keys(r.values)) ns.add(Number(k))
  return [...ns].sort((a, b) => a - b)
}

/** 0 → red-ish, 1 → green-ish, through the theme's warning tone; `null` → transparent. */
function paint(v: number | null): string {
  if (v === null) return 'transparent'
  const hue = Math.round(Math.max(0, Math.min(1, v)) * 120)
  return `hsl(${hue} 60% 45%)`
}

/**
 * 159 W2 §7: the case × experiment heat strip — one row per case grouped by split (val first, holdout, then train; the
 * user sees every split, the session never does), one column per experiment with the base as column 0, each cell
 * painted by the case's score — and the val and holdout means as two staircases above it. Hover a cell for the score.
 */
export function CaseStrip({ view }: { view: CasesView }) {
  const ns = useMemo(() => experimentNumbers(view.rows), [view.rows])
  const grouped = useMemo(
    () => SPLITS.map((s) => ({ split: s, rows: view.rows.filter((r) => r.split === s).sort((a, b) => a.id.localeCompare(b.id)) })).filter((g) => g.rows.length > 0),
    [view.rows],
  )
  const val = useMemo(() => splitMeans(view.rows, 'val', ns), [view.rows, ns])
  const holdout = useMemo(() => splitMeans(view.rows, 'holdout', ns), [view.rows, ns])
  const [hover, setHover] = useState<string | null>(null)
  if (ns.length === 0) {
    return (
      <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
        No cases scored yet — the baseline is being measured.
      </div>
    )
  }
  const W = LABEL + ns.length * CELL + 8
  const stair = (means: Array<number | null>) => {
    let d = ''
    means.forEach((m, i) => {
      if (m === null) return
      const x = LABEL + i * CELL + CELL / 2
      const y = 10 + (1 - m) * (STAIR_H - 20)
      d += d ? ` H${x} V${y}` : `M${x} ${y}`
    })
    return d
  }
  const lastVal = [...val].reverse().find((v) => v !== null) ?? null
  const lastHold = [...holdout].reverse().find((v) => v !== null) ?? null
  const totalRows = grouped.reduce((n, g) => n + g.rows.length, 0)
  return (
    <figure className="relative overflow-x-auto" data-kind={view.kind} data-testid="case-strip">
      <svg width={W} height={STAIR_H} className="block" role="img" aria-label="val and holdout means per experiment">
        {[0, 0.5, 1].map((t) => (
          <g key={t} fontSize={10} fill="currentColor" fillOpacity={0.55}>
            <line x1={LABEL} x2={W} y1={10 + (1 - t) * (STAIR_H - 20)} y2={10 + (1 - t) * (STAIR_H - 20)} stroke="currentColor" strokeOpacity={0.12} />
            <text x={LABEL - 6} y={10 + (1 - t) * (STAIR_H - 20) + 3} textAnchor="end">
              {t.toFixed(1)}
            </text>
          </g>
        ))}
        {ns.map((n, i) => (
          <text key={n} x={LABEL + i * CELL + CELL / 2} y={STAIR_H - 1} fontSize={9} textAnchor="middle" fill="currentColor" fillOpacity={0.55}>
            {n === 0 ? 'base' : n}
          </text>
        ))}
        <path d={stair(val)} fill="none" stroke="var(--status-success-fg)" strokeWidth={2} data-series="val" />
        <path d={stair(holdout)} fill="none" stroke="var(--fg-accent)" strokeWidth={2} strokeDasharray="4 3" data-series="holdout" />
      </svg>
      <svg width={W} height={totalRows * CELL + grouped.length * 16} className="block" role="img" aria-label={`${totalRows} cases by split across ${ns.length} columns`}>
        {(() => {
          let y = 0
          return grouped.map((g) => {
            const top = y
            y += 16 + g.rows.length * CELL
            return (
              <g key={g.split} data-split={g.split}>
                <text x={0} y={top + 12} fontSize={11} fill="currentColor" fillOpacity={0.7}>
                  {g.split} · {g.rows.length}
                </text>
                {g.rows.map((r, j) => (
                  <g key={r.id}>
                    <text x={LABEL - 6} y={top + 16 + j * CELL + CELL - 3} fontSize={9} textAnchor="end" fill="currentColor" fillOpacity={0.6}>
                      {r.id.length > 22 ? `${r.id.slice(0, 21)}…` : r.id}
                    </text>
                    {ns.map((n, i) => {
                      const v = r.values[n]
                      const has = typeof v === 'number'
                      const text = has ? `${r.id} · ${n === 0 ? 'base' : `#${n}`} · ${v.toFixed(2)}` : null
                      return (
                        <rect
                          key={n}
                          x={LABEL + i * CELL}
                          y={top + 16 + j * CELL}
                          width={CELL - 1}
                          height={CELL - 1}
                          fill={paint(has ? v : null)}
                          stroke={has ? 'none' : 'currentColor'}
                          strokeOpacity={0.08}
                          data-case={r.id}
                          data-n={n}
                          onMouseEnter={() => setHover(text)}
                          onMouseLeave={() => setHover(null)}
                        >
                          {text && <title>{text}</title>}
                        </rect>
                      )
                    })}
                  </g>
                ))}
              </g>
            )
          })
        })()}
      </svg>
      <figcaption className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-secondary">
        <span className="text-success">— val mean{lastVal !== null ? ` ${lastVal.toFixed(2)}` : ''}</span>
        <span className="text-fg-accent">- - holdout mean{lastHold !== null ? ` ${lastHold.toFixed(2)}` : ''}</span>
        <span>
          {view.split.train} train · {view.split.val} val · {view.split.holdout} holdout; cells 0 (red) → 1 (green), blank = not scored
        </span>
        {hover && <span className="text-fg">{hover}</span>}
      </figcaption>
    </figure>
  )
}
