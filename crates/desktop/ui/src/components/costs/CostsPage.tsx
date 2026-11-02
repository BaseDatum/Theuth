// 160 §7: the /costs page. Every number comes synchronously from the in-memory cube (`useCostIndex`); the view lives in
// the URL (D9). The secondary panels read a deferred copy of the view so the controls and the main chart stay at frame
// rate while they catch up (§6.5).

import { getRouteApi } from '@tanstack/react-router'
import { Download, PanelLeft } from 'lucide-react'
import { useCallback, useDeferredValue, useEffect, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { type Dim, defaultTz, type Filters, HOUR_MS, type Metric, options, series, sessions, toCsv } from '@/lib/costs/cube'
import { int } from '@/lib/costs/format'
import { useCostIndex } from '@/lib/costs/queries'
import { type CostsSearch, effectiveGran, filtersOf, PRESET_LABEL, resolveRange, validateCostsSearch } from '@/lib/costs/search'
import { saveBlob } from '@/lib/files'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { Breakdowns } from './Breakdowns'
import { FilterChips } from './FilterChips'
import { Kpis } from './Kpis'
import { LedgerTable } from './LedgerTable'
import { LimitsPanel } from './LimitsPanel'
import { RangeBar } from './RangeBar'
import { SessionsTable } from './SessionsTable'
import { SpendChart } from './SpendChart'
import { Tokens, When } from './When'

const route = getRouteApi('/costs')

/** `Date.now()` once a minute on the minute (ranges end now; the hour buckets move). */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    let t: ReturnType<typeof setTimeout>
    const tick = () => {
      setNow(Date.now())
      t = setTimeout(tick, 60_000 - (Date.now() % 60_000))
    }
    t = setTimeout(tick, 60_000 - (Date.now() % 60_000))
    return () => clearTimeout(t)
  }, [])
  return now
}

const fmtDate = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' })

export function CostsPage() {
  const search = route.useSearch()
  const navigate = route.useNavigate()
  const api = useApi()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const tz = useMemo(defaultTz, [])
  const now = useNow()
  const ix = useCostIndex(tz)
  const meta = ix.cube.meta
  const firstMs = meta.first_call_ms ?? (ix.n ? ix.minHour * HOUR_MS : null)
  const { range, from, to } = search
  const r = useMemo(() => resolveRange({ range, from, to }, now, tz, firstMs), [range, from, to, now, tz, firstMs])
  const gran = effectiveGran(search, tz, r)
  const fkey = JSON.stringify(filtersOf(search))
  const filters = useMemo((): Filters => JSON.parse(fkey), [fkey])
  const by = search.by ?? 'model'
  const metric = search.metric ?? 'usd'
  const compare = search.compare !== false
  const rangeLabel = search.range === 'custom' ? 'range' : PRESET_LABEL[search.range ?? '30d']

  const set = useCallback((patch: Partial<CostsSearch>) => void navigate({ search: (prev) => validateCostsSearch({ ...prev, ...patch }) }), [navigate])
  const addFilter = useCallback(
    (dim: Dim, id: string) => navigate({ search: (prev) => validateCostsSearch({ ...prev, [dim]: prev[dim]?.includes(id) ? prev[dim] : [...(prev[dim] ?? []), id] }) }),
    [navigate],
  )
  const onGroupBy = useCallback((v: Dim) => set({ by: v }), [set])
  const onMetric = useCallback((v: Metric) => set({ metric: v }), [set])
  const onCompare = useCallback((on: boolean) => set({ compare: on }), [set])
  const onRange = useCallback((f: number, t: number) => set({ range: 'custom', from: f, to: t, gran: undefined }), [set])
  const out = useMemo(() => series(ix, { ...r, gran, groupBy: by, metric, filters }), [ix, r, gran, by, metric, filters])
  const opts = useMemo(() => options(ix, r, filters), [ix, r, filters])
  // The panels below the fold follow a deferred view (§6.5).
  const view = useMemo(() => ({ ix, r, gran, filters }), [ix, r, gran, filters])
  const d = useDeferredValue(view)
  const rows = useMemo(() => sessions(d.ix, d.r, d.filters), [d])

  const exportCsv = () => {
    const name = `theuth-costs-${new Date(r.from).toISOString().slice(0, 10)}-${new Date(r.to).toISOString().slice(0, 10)}-${gran}-${by}`
    saveBlob(api, new Blob([toCsv(out, metric, compare)], { type: 'text/csv' }), name, 'csv').catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  }

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header data-tauri-drag-region={mac || undefined} className={cn('flex h-[52px] shrink-0 items-center gap-2.5 border-b border-border-subtle px-3 md:px-4', !sidebar.open && mac && 'pl-20')}>
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h1 className="text-[15px] font-semibold tracking-[0.2px]">Costs</h1>
        <span className="hidden min-w-0 items-center gap-1.5 truncate font-mono text-[10px] tracking-[1.4px] text-fg-tertiary uppercase sm:inline-flex">
          <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-success motion-safe:animate-pulse" />
          live{firstMs !== null && ` · ledger since ${fmtDate(firstMs)}`}
          {meta.engine && ` · ${meta.engine}`}
        </span>
        <div className="flex-1" />
        <Button variant="outline" size="sm" onClick={exportCsv} disabled={!out.rows.length} title="Export the chart's buckets × series as CSV">
          <Download />
          CSV
        </Button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="@container mx-auto flex max-w-[1600px] flex-col gap-3.5 px-3 pt-3.5 pb-5 md:px-4">
          <RangeBar search={search} r={r} gran={gran} tz={tz} now={now} set={set} />
          <FilterChips options={opts} filters={filters} set={set} />
          {ix.n === 0 ? (
            <p className="rounded-xl border border-border bg-card px-4 py-10 text-center text-sm text-fg-tertiary">No model calls recorded yet.</p>
          ) : (
            <>
              <Kpis ix={ix} r={r} filters={filters} now={now} rangeLabel={rangeLabel} />
              <SpendChart
                out={out}
                metric={metric}
                groupBy={by}
                compare={compare}
                rangeLabel={rangeLabel}
                onGroupBy={onGroupBy}
                onMetric={onMetric}
                onCompare={onCompare}
                onFilter={addFilter}
                onRange={onRange}
              />
            </>
          )}
          <LimitsPanel now={now} />
          {ix.n > 0 && (
            <>
              <Breakdowns ix={d.ix} r={d.r} filters={d.filters} rangeLabel={rangeLabel} onFilter={addFilter} />
              <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,340px),1fr))]">
                <When ix={d.ix} r={d.r} gran={d.gran} filters={d.filters} rangeLabel={rangeLabel} />
                <Tokens ix={d.ix} r={d.r} gran={d.gran} filters={d.filters} rangeLabel={rangeLabel} />
              </div>
              <SessionsTable rows={rows} title={`Sessions · ${rangeLabel}`} now={now} onFilter={addFilter} />
              <LedgerTable ix={d.ix} range={d.r} filters={d.filters} now={now} />
            </>
          )}
          {(meta.pre_ledger_sessions > 0 || ix.n > 0) && (
            <footer className="flex flex-col gap-0.5 px-1 text-[11px] text-fg-tertiary">
              {meta.pre_ledger_sessions > 0 && firstMs !== null && (
                <span>
                  Cost records start {fmtDate(firstMs)} · {int(meta.pre_ledger_sessions)} earlier sessions have no usage data.
                </span>
              )}
              <span>The ledger is this engine&apos;s own{meta.engine ? ` (${meta.engine})` : ''}; other mesh engines keep their own books.</span>
              <span>Unpriced calls are counted at $0 — failed and cancelled attempts carry no tokens, so unpriced is a lower bound.</span>
            </footer>
          )}
        </div>
      </div>
    </article>
  )
}
