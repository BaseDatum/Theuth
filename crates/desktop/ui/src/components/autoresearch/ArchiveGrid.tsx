import type { CellRow } from "@/rpc/bindings/method.autoresearch.output"
import type { KindView } from "@/rpc/bindings/method.autoresearch.output"

type ArchiveView = Extract<KindView, { kind: 'archive' }>

/** `i<island>:<b1>[.<b2>]` → the bins (empty for a key without descriptors). */
export function binsOf(key: string): number[] {
  const after = key.split(':')[1]
  if (after === undefined || after === '') return []
  return after.split('.').map((b) => Number.parseInt(b, 10) || 0)
}

/** 0 at the champion's score, 1 at the farthest elite — direction-free, the champion is the best by definition. */
export function distanceFromChampion(cells: readonly CellRow[]): Map<string, number> {
  const metric = (c: CellRow) => Object.values(c.scores)[0] ?? Number.NaN
  const champ = cells.find((c) => c.champion) ?? cells[0]
  const out = new Map<string, number>()
  if (!champ) return out
  const best = metric(champ)
  const far = Math.max(0, ...cells.map((c) => Math.abs(metric(c) - best)).filter(Number.isFinite))
  for (const c of cells) {
    const d = Math.abs(metric(c) - best)
    out.set(c.key, far > 0 && Number.isFinite(d) ? d / far : 0)
  }
  return out
}

const fmt = (v: number) => (Math.abs(v) >= 100 ? v.toFixed(0) : v.toFixed(2).replace(/\.?0+$/, ''))

/**
 * 159 §9 W4: one grid per island, cells coloured by score (the champion's shade is full, the farthest elite faint),
 * the champion starred. Columns are the first descriptor's bins, rows the second's; a hover names the cell, its
 * experiment, score, descriptor values and children.
 */
export function ArchiveGrid({ view }: { view: ArchiveView }) {
  const [ax, ay] = view.axes
  const islands = Array.from({ length: Math.max(view.islands, 1) }, (_, i) => i)
  const bins = view.cells.map((c) => binsOf(c.key))
  const cols = Math.max(1, ...bins.map((b) => (b[0] ?? 0) + 1))
  const rows = Math.max(1, ...bins.map((b) => (b[1] ?? 0) + 1))
  const dist = distanceFromChampion(view.cells)
  const metricName = view.cells.map((c) => Object.keys(c.scores)[0]).find((k) => k !== undefined) ?? 'score'
  if (view.cells.length === 0) {
    return (
      <div className="text-sm text-fg-tertiary" data-kind={view.kind}>
        No elite yet: the base seeds every island at the first wave.
      </div>
    )
  }
  return (
    <div className="flex flex-wrap gap-4" data-kind={view.kind}>
      {islands.map((island) => {
        const mine = view.cells.filter((c) => (c.island ?? Number.parseInt(c.key.slice(1), 10)) === island)
        return (
          <section key={island} className="min-w-0" data-island={island}>
            <h4 className="mb-1 text-xs font-medium text-fg-secondary">
              island {island} · {mine.length} {mine.length === 1 ? 'elite' : 'elites'}
            </h4>
            <div className="flex items-stretch gap-1">
              {ay && (
                <div className="flex items-center" aria-hidden>
                  <span className="text-[10px] text-fg-tertiary [writing-mode:vertical-rl] rotate-180">{ay} ↑</span>
                </div>
              )}
              <div
                className="grid gap-1"
                style={{ gridTemplateColumns: `repeat(${cols}, minmax(2.75rem, 1fr))`, gridTemplateRows: `repeat(${rows}, 2.75rem)` }}
                role="grid"
                aria-label={`island ${island}`}
              >
                {Array.from({ length: rows * cols }, (_, i) => {
                  const col = i % cols
                  // Row 0 at the bottom: the second descriptor grows upward like a chart axis.
                  const row = rows - 1 - Math.floor(i / cols)
                  const cell = mine.find((c) => {
                    const b = binsOf(c.key)
                    return (b[0] ?? 0) === col && (b[1] ?? 0) === row
                  })
                  if (!cell) {
                    return <div key={i} className="rounded border border-dashed border-border/60" data-empty={`${col}.${row}`} role="gridcell" aria-label="empty cell" />
                  }
                  const score = Object.values(cell.scores)[0]
                  const shade = 1 - 0.8 * (dist.get(cell.key) ?? 0)
                  const desc = Object.entries(cell.descriptor)
                    .map(([k, v]) => `${k} ${fmt(v)}`)
                    .join(' · ')
                  const title = `${cell.key} · #${cell.n}${cell.champion ? ' · champion (re-measured at finalize)' : ''}\n${metricName} ${score === undefined ? '?' : fmt(score)}${desc ? `\n${desc}` : ''}\nchildren ${cell.children}\n${cell.sha.slice(0, 8)}`
                  return (
                    <div
                      key={cell.key}
                      role="gridcell"
                      tabIndex={0}
                      title={title}
                      aria-label={title.replaceAll('\n', ', ')}
                      data-cell={cell.key}
                      data-champion={cell.champion || undefined}
                      className="relative flex flex-col items-center justify-center rounded border border-border text-[11px] leading-tight"
                      style={{ background: `color-mix(in oklab, var(--status-success-fg) ${Math.round(shade * 55)}%, var(--bg-elevated))` }}
                    >
                      <span className="font-medium">
                        {cell.champion ? '★ ' : ''}#{cell.n}
                      </span>
                      <span className="text-fg-secondary">{score === undefined ? '?' : fmt(score)}</span>
                    </div>
                  )
                })}
              </div>
            </div>
            {ax && (
              <div className="mt-1 text-[10px] text-fg-tertiary" aria-hidden>
                {ax} →
              </div>
            )}
          </section>
        )
      })}
    </div>
  )
}
