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
import { type ChartView, SpendChart } from './SpendChart'
import { Tokens, When } from './When'
import { useStaged } from './ui'

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
  // §6.5: the controls follow `search` at once; everything computed or drawn from the cube follows `view`, a deferred
  // copy, so a click commits the control first and the panels catch up in an interruptible render.
  const search = route.useSearch()
  const view = useDeferredValue(search)
  const navigate = route.useNavigate()
  const api = useApi()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const tz = useMemo(defaultTz, [])
  const now = useNow()
  const ix = useCostIndex(tz)
  const meta = ix.cube.meta
  const firstMs = meta.first_call_ms ?? (ix.n ? ix.minHour * HOUR_MS : null)
  // Urgent (controls).
  const rNow = useMemo(() => resolveRange({ range: search.range, from: search.from, to: search.to }, now, tz, firstMs), [search.range, search.from, search.to, now, tz, firstMs])
  const granNow = effectiveGran(search, tz, rNow)
  const fkeyNow = JSON.stringify(filtersOf(search))
  const filtersNow = useMemo((): Filters => JSON.parse(fkeyNow), [fkeyNow])
  const ctl = useMemo<ChartView>(() => ({ groupBy: search.by ?? 'model', metric: search.metric ?? 'usd', compare: search.compare !== false }), [search.by, search.metric, search.compare])
  // Deferred (panels).
  const r = useMemo(() => resolveRange({ range: view.range, from: view.from, to: view.to }, now, tz, firstMs), [view.range, view.from, view.to, now, tz, firstMs])
  const gran = effectiveGran(view, tz, r)
  const fkey = JSON.stringify(filtersOf(view))
  const filters = useMemo((): Filters => JSON.parse(fkey), [fkey])
  const chartView = useMemo<ChartView>(() => ({ groupBy: view.by ?? 'model', metric: view.metric ?? 'usd', compare: view.compare !== false }), [view.by, view.metric, view.compare])
  const { groupBy: by, metric, compare } = chartView
  const rangeLabel = view.range === 'custom' ? 'range' : PRESET_LABEL[view.range ?? '30d']

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
  // Below the main chart each card takes the new view a frame or more later (Breakdowns stages its five cards itself),
  // so a range change commits in frame-sized pieces instead of one long one.
  const panels = useMemo(() => ({ ix, r, gran, filters, rangeLabel }), [ix, r, gran, filters, rangeLabel])
  const p1 = useStaged(panels, 1)
  const pWhen = useStaged(panels, 7)
  const pTok = useStaged(panels, 8)
  const p3 = useStaged(panels, 9)
  const rows = useMemo(() => sessions(p3.ix, p3.r, p3.filters), [p3])

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
          <RangeBar search={search} r={rNow} gran={granNow} tz={tz} now={now} set={set} />
          <FilterChips options={opts} filters={filtersNow} set={set} />
          {ix.n === 0 ? (
            <p className="rounded-xl border border-border bg-card px-4 py-10 text-center text-sm text-fg-tertiary">No model calls recorded yet.</p>
          ) : (
            <>
              <Kpis ix={ix} r={r} filters={filters} now={now} rangeLabel={rangeLabel} />
              <SpendChart out={out} view={chartView} ctl={ctl} rangeLabel={rangeLabel} onGroupBy={onGroupBy} onMetric={onMetric} onCompare={onCompare} onFilter={addFilter} onRange={onRange} />
            </>
          )}
          <LimitsPanel now={now} />
          {ix.n > 0 && (
            <>
              <Breakdowns ix={p1.ix} r={p1.r} filters={p1.filters} rangeLabel={p1.rangeLabel} onFilter={addFilter} />
              <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,340px),1fr))]">
                <When ix={pWhen.ix} r={pWhen.r} filters={pWhen.filters} rangeLabel={pWhen.rangeLabel} />
                <Tokens ix={pTok.ix} r={pTok.r} gran={pTok.gran} filters={pTok.filters} rangeLabel={pTok.rangeLabel} />
              </div>
              <SessionsTable rows={rows} title={`Sessions · ${p3.rangeLabel}`} now={now} onFilter={addFilter} />
              <LedgerTable ix={p3.ix} range={p3.r} filters={p3.filters} now={now} />
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
