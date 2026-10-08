import { type ReactNode, useMemo, useState } from 'react'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { OUTCOME_CLASS, type RunFilter, duration, filterRuns, metric, outcome, outcomeLabel, pct, shortSha, sigmas, usd } from './model'
import type { ExperimentRow } from './types'

const FILTERS: { id: RunFilter; label: string }[] = [
  { id: 'all', label: 'all' },
  { id: 'kept', label: 'kept' },
  { id: 'flagged', label: 'flagged' },
]

/** The verdict badge of a row: words from `outcomeLabel`, colour from `OUTCOME_CLASS`. */
export function VerdictBadge({ row, className }: { row: Pick<ExperimentRow, 'state' | 'verdict'>; className?: string }) {
  return <span className={cn('inline-flex h-5 items-center rounded-full px-2 text-[11px] font-semibold whitespace-nowrap', OUTCOME_CLASS[outcome(row.state, row.verdict)], className)}>{outcomeLabel(row.state, row.verdict)}</span>
}

/**
 * §13.1 the run table: #, verdict, value, Δ vs the incumbent, σ-multiple, idea [category], duration, cost, commit;
 * filters all / kept / flagged. A row is a button to its experiment. Newest first, as the RPC returns them.
 */
export function RunTable({ rows, metricName, pending, active, onPick }: { rows: ExperimentRow[] | undefined; metricName: string; pending: boolean; active: number | null; onPick: (n: number) => void }) {
  const [filter, setFilter] = useState<RunFilter>('all')
  const shown = useMemo(() => filterRuns(rows ?? [], filter), [rows, filter])
  return (
    <section className="rounded-[10px] border border-border-subtle bg-layer-1">
      <header className="flex flex-wrap items-center gap-2 px-3 py-2">
        <h3 className="text-[13px] font-semibold">Runs</h3>
        <span className="text-xs text-fg-tertiary tabular-nums">{rows ? `${shown.length}${filter === 'all' ? '' : ` of ${rows.length}`}` : ''}</span>
        <div className="ml-auto flex gap-1" role="group" aria-label="Filter runs">
          {FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              aria-pressed={filter === f.id}
              onClick={() => setFilter(f.id)}
              className={cn('h-6 rounded-full border px-2.5 text-xs transition-colors', filter === f.id ? 'border-foreground bg-foreground text-background' : 'border-border-subtle text-fg-secondary hover:bg-accent/60')}
            >
              {f.label}
            </button>
          ))}
        </div>
      </header>
      {pending ? (
        <div className="space-y-2 px-3 pb-3">
          <Skeleton className="h-6 w-full" />
          <Skeleton className="h-6 w-full" />
          <Skeleton className="h-6 w-3/4" />
        </div>
      ) : shown.length === 0 ? (
        <p className="px-3 pb-3 text-sm text-fg-tertiary">{rows && rows.length > 0 ? 'Nothing matches this filter.' : 'No experiments yet.'}</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-[13px]">
            <thead>
              <tr className="text-left text-[11.5px] font-semibold text-fg-tertiary">
                <Th>#</Th>
                <Th>verdict</Th>
                <Th className="text-right">{metricName}</Th>
                <Th className="text-right">Δ</Th>
                <Th className="hidden text-right md:table-cell" title="Standard deviations better than the incumbent">
                  σ×
                </Th>
                <Th>idea</Th>
                <Th className="hidden text-right md:table-cell">time</Th>
                <Th className="hidden text-right md:table-cell">cost</Th>
                <Th className="hidden 2xl:table-cell">commit</Th>
              </tr>
            </thead>
            <tbody>
              {shown.map((r) => (
                <tr
                  key={r.n}
                  tabIndex={0}
                  aria-current={active === r.n ? 'true' : undefined}
                  onClick={() => onPick(r.n)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault()
                      onPick(r.n)
                    }
                  }}
                  className={cn('cursor-pointer border-t border-border-subtle align-top outline-none hover:bg-layer-2 focus-visible:bg-layer-2', active === r.n && 'bg-layer-3 hover:bg-layer-3')}
                >
                  <Td className="tabular-nums text-fg-secondary">{r.n}</Td>
                  <Td>
                    <VerdictBadge row={r} />
                  </Td>
                  <Td className="text-right tabular-nums">{metric(r.metric, '')}</Td>
                  <Td className={cn('text-right tabular-nums', r.state === 'kept' && 'text-success')}>{pct(r.delta_pct)}</Td>
                  <Td className="hidden text-right tabular-nums md:table-cell">{sigmas(r.z)}</Td>
                  <Td className="max-w-[360px] min-w-[150px] whitespace-normal">
                    <span className="line-clamp-2">
                      {r.idea} <span className="text-[11px] text-fg-tertiary">[{r.category}]</span>
                      {r.deviations.length > 0 && (
                        <span className="ml-1 text-[11px] text-warning" title={r.deviations.join(', ')}>
                          · {r.deviations.length === 1 ? 'deviation' : `${r.deviations.length} deviations`}
                        </span>
                      )}
                    </span>
                  </Td>
                  <Td className="hidden text-right tabular-nums text-fg-secondary md:table-cell">{duration(r.duration_ms)}</Td>
                  <Td className="hidden text-right tabular-nums text-fg-secondary md:table-cell">{usd(r.cost_usd)}</Td>
                  <Td className="hidden font-mono text-xs text-fg-tertiary 2xl:table-cell">{shortSha(r.commit_sha)}</Td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function Th({ children, className, title }: { children: ReactNode; className?: string; title?: string }) {
  return (
    <th scope="col" title={title} className={cn('px-3 py-1.5 font-semibold', className)}>
      {children}
    </th>
  )
}

function Td({ children, className }: { children: ReactNode; className?: string }) {
  return <td className={cn('px-3 py-1.5 whitespace-nowrap', className)}>{children}</td>
}
