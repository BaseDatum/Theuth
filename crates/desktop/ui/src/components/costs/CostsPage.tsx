// 160 §7: the /costs page. Every number comes synchronously from the in-memory cube (`useCostIndex`); the view lives in
// the URL (D9). The secondary panels read a deferred copy of the view so the controls and the main chart stay at frame
// rate while they catch up (§6.5).

import { useNavigate } from '@tanstack/react-router'
import { Download } from 'lucide-react'
import { memo, startTransition, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { SidebarLead } from '@/components/nav/SectionBand'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { type Dim, defaultTz, type Filters, HOUR_MS, type Index, type Metric, options, type Range, series, sessions, toCsv } from '@/lib/costs/cube'
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
import { Later, useStaged } from './ui'

/** `Date.now()` once a minute on the minute (ranges end now; the hour buckets move); paused while the page is hidden. */
function useNow(running: boolean): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!running) return
    let t: ReturnType<typeof setTimeout>
    const tick = () => {
      setNow(Date.now())
      t = setTimeout(tick, 60_000 - (Date.now() % 60_000))
    }
    // Coming back after a minute or more: catch up at once.
    if (Math.floor(Date.now() / 60_000) !== Math.floor(now / 60_000)) setNow(Date.now())
    t = setTimeout(tick, 60_000 - (Date.now() % 60_000))
    return () => clearTimeout(t)
  }, [running])
  return now
}

/** `v` while `live`, else the value it had when `live` went false (the resident page stays still while hidden). */
function useHeld<T>(v: T, live: boolean): T {
  const [held, setHeld] = useState(v)
  if (live && held !== v) setHeld(v)
  return live ? v : held
}

const fmtDate = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' })

/** The sessions table's rows are computed only once the section mounts. */
const Sessions = memo(function Sessions({ v, now, onFilter }: { v: { ix: Index; r: Range; filters: Filters; rangeLabel: string }; now: number; onFilter: (dim: Dim, id: string) => void }) {
  const rows = useMemo(() => sessions(v.ix, v.r, v.filters), [v])
  return <SessionsTable rows={rows} title={`Sessions · ${v.rangeLabel}`} now={now} onFilter={onFilter} />
})

/**
 * The costs page. Mounted once by `CostsResident` (and kept, hidden, when you leave); `search` is the /costs URL state,
 * `visible` whether it is on screen. While hidden nothing ticks or re-renders: the clock and the cube index are held.
 */
export function CostsPage({ search, visible }: { search: CostsSearch; visible: boolean }) {
  // §6.5 (R2): a click changes `pending` — the controls' own state — and nothing else; the URL write waits for the next
  // frame and runs in a transition. Everything computed or drawn from the cube follows `view`, a deferred copy of the
  // URL, so the panels catch up in interruptible renders after the control has painted.
  const [pending, setPending] = useState<{ id: number; s: CostsSearch } | null>(null)
  const live = pending?.s ?? search
  const view = useDeferredValue(search)
  const navigate = useNavigate()
  const api = useApi()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const tz = useMemo(defaultTz, [])
  // Deferred like the index: a minute tick (or catching up after being hidden) re-renders the panels after the paint.
  const now = useDeferredValue(useNow(visible))
  // The index changes with every cube delta; held while hidden and deferred, so showing the page or a live update
  // never re-renders the charts ahead of the paint.
  const ix = useDeferredValue(useHeld(useCostIndex(tz), visible))
  const meta = ix.cube.meta
  const firstMs = meta.first_call_ms ?? (ix.n ? ix.minHour * HOUR_MS : null)
  // Urgent (controls).
  const rNow = useMemo(() => resolveRange({ range: live.range, from: live.from, to: live.to }, now, tz, firstMs), [live.range, live.from, live.to, now, tz, firstMs])
  const granNow = effectiveGran(live, tz, rNow)
  const fkeyNow = JSON.stringify(filtersOf(live))
  const filtersNow = useMemo((): Filters => JSON.parse(fkeyNow), [fkeyNow])
  const ctl = useMemo<ChartView>(() => ({ groupBy: live.by ?? 'model', metric: live.metric ?? 'usd', compare: live.compare !== false }), [live.by, live.metric, live.compare])
  // Deferred (panels).
  const r = useMemo(() => resolveRange({ range: view.range, from: view.from, to: view.to }, now, tz, firstMs), [view.range, view.from, view.to, now, tz, firstMs])
  const gran = effectiveGran(view, tz, r)
  const fkey = JSON.stringify(filtersOf(view))
  const filters = useMemo((): Filters => JSON.parse(fkey), [fkey])
  const chartView = useMemo<ChartView>(() => ({ groupBy: view.by ?? 'model', metric: view.metric ?? 'usd', compare: view.compare !== false }), [view.by, view.metric, view.compare])
  const { groupBy: by, metric, compare } = chartView
  const rangeLabel = view.range === 'custom' ? 'range' : PRESET_LABEL[view.range ?? '30d']

  const latest = useRef(live)
  latest.current = live
  const seq = useRef(0)
  const commit = useCallback(
    (next: CostsSearch) => {
      const id = ++seq.current
      latest.current = next
      setPending({ id, s: next })
      requestAnimationFrame(() =>
        setTimeout(() =>
          startTransition(() =>
            void navigate({ to: '/costs', search: next }).finally(() => setPending((p) => (p?.id === id ? null : p))),
          ),
        ),
      )
    },
    [navigate],
  )
  const set = useCallback((patch: Partial<CostsSearch>) => commit(validateCostsSearch({ ...latest.current, ...patch })), [commit])
  const addFilter = useCallback(
    (dim: Dim, id: string) => {
      const prev = latest.current
      commit(validateCostsSearch({ ...prev, [dim]: prev[dim]?.includes(id) ? prev[dim] : [...(prev[dim] ?? []), id] }))
    },
    [commit],
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

  const exportCsv = () => {
    const name = `theuth-costs-${new Date(r.from).toISOString().slice(0, 10)}-${new Date(r.to).toISOString().slice(0, 10)}-${gran}-${by}`
    saveBlob(api, new Blob([toCsv(out, metric, compare)], { type: 'text/csv' }), name, 'csv').catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  }

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header data-tauri-drag-region={mac || undefined} className={cn('flex h-[52px] shrink-0 items-center gap-2.5 border-b border-border-subtle px-3 md:px-4', !sidebar.open && mac && 'pl-20')}>
        <SidebarLead />
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
      <div data-costs-scroll className="min-h-0 flex-1 overflow-y-auto">
        <div className="@container mx-auto flex max-w-[1600px] flex-col gap-3.5 px-3 pt-3.5 pb-5 md:px-4">
          <RangeBar search={live} r={rNow} gran={granNow} tz={tz} now={now} set={set} />
          <FilterChips options={opts} filters={filtersNow} set={set} />
          {ix.n === 0 ? (
            <p className="rounded-xl border border-border bg-card px-4 py-10 text-center text-sm text-fg-tertiary">No model calls recorded yet.</p>
          ) : (
            <>
              <Kpis ix={ix} r={r} filters={filters} now={now} rangeLabel={rangeLabel} />
              <SpendChart out={out} view={chartView} ctl={ctl} rangeLabel={rangeLabel} onGroupBy={onGroupBy} onMetric={onMetric} onCompare={onCompare} onFilter={addFilter} onRange={onRange} />
            </>
          )}
          <Later order={1} height={300}>
            <LimitsPanel now={now} visible={visible} />
          </Later>
          {ix.n > 0 && (
            <>
              {/* R2: below the fold, each section mounts in its own idle slice (or as a scroll nears it), its height held. */}
              <Later order={1} height={940}>
                <Breakdowns ix={p1.ix} r={p1.r} filters={p1.filters} rangeLabel={p1.rangeLabel} onFilter={addFilter} />
              </Later>
              <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,340px),1fr))]">
                <Later order={2} height={330}>
                  <When ix={pWhen.ix} r={pWhen.r} filters={pWhen.filters} rangeLabel={pWhen.rangeLabel} />
                </Later>
                <Later order={3} height={330}>
                  <Tokens ix={pTok.ix} r={pTok.r} gran={pTok.gran} filters={pTok.filters} rangeLabel={pTok.rangeLabel} />
                </Later>
              </div>
              <Later order={4} height={640}>
                <Sessions v={p3} now={now} onFilter={addFilter} />
              </Later>
              <Later order={5} height={520}>
                <LedgerTable ix={p3.ix} range={p3.r} filters={p3.filters} now={now} />
              </Later>
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
