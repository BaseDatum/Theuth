// 160 §7.11: every model call, newest first — server-paged `cost_calls` under the active filters, virtualised. Previous
// rows stay while a page arrives; the only load signal is a hairline that appears if a page takes > 150 ms.

import { useNavigate } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { BASES, BASIS_COLOR, BASIS_LABEL, type Filters, type Index, type Range, sessionLabel } from '@/lib/costs/cube'
import { compact, usd } from '@/lib/costs/format'
import { useCostCalls } from '@/lib/costs/queries'
import { cn } from '@/lib/utils'
import type { CostBasis } from '@/rpc/bindings/CostBasis'
import { Panel, Swatch, Tag } from './ui'

const ROW = 33
const PAGE = 200
const GRID = 'grid grid-cols-[92px_minmax(160px,1.2fr)_72px_minmax(150px,1fr)_64px_64px_64px_84px_150px_84px] items-center gap-x-2 px-2'
const HEAD = 'font-mono text-[10px] font-medium tracking-[1.2px] whitespace-nowrap text-fg-tertiary uppercase'
const NUM = 'text-right font-mono tabular-nums'

const pad2 = (n: number) => String(n).padStart(2, '0')
/** `16:40:08` today, else `2 Nov 16:40`. Local time. */
function time(ms: number, now: number): string {
  const d = new Date(ms)
  const hm = `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
  if (new Date(now).toDateString() === d.toDateString()) return `${hm}:${pad2(d.getSeconds())}`
  return `${d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })} ${hm}`
}

export const LedgerTable = memo(function LedgerTable({ ix, range, filters, now }: { ix: Index; range: Range; filters: Filters; now: number }) {
  const navigate = useNavigate()
  const query = useMemo(() => {
    const or = (a?: string[]) => (a?.length ? a : null)
    const names = new Set(filters.model)
    const owners = new Set(filters.owner)
    const projects = new Set(filters.project)
    const q = filters.q?.trim().toLowerCase()
    let roots: string[] | null = null
    if (owners.size || projects.size || q) {
      roots = []
      for (const r of ix.roots) {
        if (!r) continue
        if (owners.size && !owners.has(`${r.owner_kind}:${r.owner}`)) continue
        if (projects.size && !projects.has(r.project ?? '')) continue
        if (q && !sessionLabel(r).toLowerCase().includes(q)) continue
        roots.push(r.id)
      }
    }
    const bases = (filters.basis ?? []).filter((b): b is CostBasis => (BASES as readonly string[]).includes(b))
    return {
      from_ms: range.from,
      // A range that ends now stays open-ended, so the minute tick does not refetch the pages.
      to_ms: range.to >= now - 60_000 ? null : range.to,
      roots,
      providers: or(filters.provider),
      models: names.size ? ix.cube.models.filter((m) => names.has(m.name)).map((m) => m.id) : null,
      kinds: or(filters.kind),
      vias: or(filters.via),
      bases: bases.length ? bases : null,
      limit: PAGE,
    }
  }, [ix, range.from, range.to, filters, now])
  const r = useCostCalls(query)
  const calls = useMemo(() => r.data?.pages.flatMap((p) => p.calls) ?? [], [r.data])
  const titles = useMemo(() => new Map(ix.roots.flatMap((root) => (root ? [[root.id, sessionLabel(root)] as const] : []))), [ix])

  const box = useRef<HTMLDivElement>(null)
  const v = useVirtualizer({ count: calls.length, getScrollElement: () => box.current, estimateSize: () => ROW, overscan: 12 })
  const items = v.getVirtualItems()
  const lastIndex = items[items.length - 1]?.index ?? -1
  const { hasNextPage, isFetchingNextPage, fetchNextPage, isFetching } = r
  useEffect(() => {
    if (lastIndex >= calls.length - 20 && hasNextPage && !isFetchingNextPage) void fetchNextPage()
  }, [lastIndex, calls.length, hasNextPage, isFetchingNextPage, fetchNextPage])

  // §6.4: the hairline shows only when a fetch outlives 150 ms.
  const [slow, setSlow] = useState(false)
  useEffect(() => {
    if (!isFetching) {
      setSlow(false)
      return
    }
    const t = setTimeout(() => setSlow(true), 150)
    return () => clearTimeout(t)
  }, [isFetching])

  return (
    <Panel title="Ledger" note="every model call · newest first">
      <div className="relative">
        {slow && <div aria-hidden className="absolute inset-x-0 top-0 z-20 h-px animate-pulse bg-info" />}
        <div ref={box} className="h-[420px] overflow-auto">
          <div className="min-w-[900px]">
            <div className={cn(GRID, 'sticky top-0 z-10 h-7 border-b border-border-strong bg-card')}>
              <span className={HEAD}>Time</span>
              <span className={HEAD}>Session</span>
              <span className={HEAD}>Kind</span>
              <span className={HEAD}>Model</span>
              <span className={cn(HEAD, 'text-right')}>In</span>
              <span className={cn(HEAD, 'text-right')}>Cache</span>
              <span className={cn(HEAD, 'text-right')}>Out</span>
              <span className={cn(HEAD, 'text-right')}>Cost</span>
              <span className={HEAD}>Basis</span>
              <span className={HEAD}>Status</span>
            </div>
            {r.isError ? (
              <p className="px-2 py-3 text-[12px] text-fg-tertiary">The call ledger needs a newer engine.</p>
            ) : r.isSuccess && calls.length === 0 ? (
              <p className="px-2 py-3 text-[12px] text-fg-tertiary">No calls in this range.</p>
            ) : (
              <div className="relative w-full" style={{ height: v.getTotalSize() }}>
                {items.map((item) => {
                  const c = calls[item.index]!
                  const bad = c.status === 'failed' || c.status === 'cancelled'
                  const open = () => void navigate({ to: '/c/$session', params: { session: c.session } })
                  return (
                    <div
                      key={c.id}
                      role="link"
                      tabIndex={0}
                      onClick={open}
                      onKeyDown={(e) => e.key === 'Enter' && open()}
                      className={cn(GRID, 'absolute top-0 left-0 w-full cursor-pointer border-b border-border text-[12.5px] whitespace-nowrap outline-none hover:bg-layer-2/45 focus-visible:bg-layer-2/45')}
                      style={{ height: ROW, transform: `translateY(${item.start}px)` }}
                    >
                      <span className="font-mono text-[11px] text-fg-tertiary">{time(c.at_ms, now)}</span>
                      <span className="max-w-[160px] truncate text-foreground">{titles.get(c.root) ?? 'Unknown session'}</span>
                      <span className="truncate">{c.kind}</span>
                      <span className="truncate font-mono text-[11px]">{c.model?.replace(/^[^/]+\//, '') ?? '—'}</span>
                      <span className={NUM}>{c.input == null ? '—' : compact(c.input)}</span>
                      <span className={NUM}>{c.cache_read == null ? '—' : compact(c.cache_read)}</span>
                      <span className={NUM}>{c.output == null ? '—' : compact(c.output)}</span>
                      <span className={NUM}>{c.nanos == null ? '—' : usd(c.nanos / 1e9)}</span>
                      <span className="flex gap-1">
                        <Tag>
                          <Swatch color={BASIS_COLOR[c.basis]} className="mr-1" />
                          {BASIS_LABEL[c.basis]}
                        </Tag>
                        {c.repriced && <Tag>repriced</Tag>}
                      </span>
                      <span title={c.reason ?? undefined}>{c.status !== 'settled' && <Tag tone={bad ? 'danger' : 'muted'}>{c.status}</Tag>}</span>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </Panel>
  )
})
