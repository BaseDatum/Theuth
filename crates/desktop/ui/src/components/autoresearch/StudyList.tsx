import { Link } from '@tanstack/react-router'
import { memo, useMemo } from 'react'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import type { StopReason } from '@/rpc/bindings/StopReason'
import type { StudyInfo } from '@/rpc/bindings/StudyInfo'
import type { StudyState } from '@/rpc/bindings/StudyState'
import { STATE_CLASS, experimentsText, improved, matches, pct, sortStudies, stateLabel } from './model'
import { useStudies } from './queries'

/** The rail body: every study as a row — slug, state pill, headline delta, experiments n/N, the goal. ponytail: not virtualized; studies are tens, not thousands. */
export function StudyList({ q, active }: { q: string; active: string | undefined }) {
  const list = useStudies()
  const rows = useMemo(() => sortStudies((list.data ?? []).filter((s) => matches(s, q))), [list.data, q])
  if (list.isPending) {
    return (
      <div className="space-y-2 p-3">
        <Skeleton className="h-[56px] w-full" />
        <Skeleton className="h-[56px] w-full" />
        <Skeleton className="h-[56px] w-2/3" />
      </div>
    )
  }
  if (list.isError) return <p className="p-3 text-sm text-danger">{list.error.message}</p>
  if (list.data.length === 0) {
    return (
      <p className="p-3 text-sm text-fg-tertiary">
        No studies yet. Ask theuth in any chat: <code className="font-mono text-[12px]">/autoresearch &lt;goal&gt;</code>
      </p>
    )
  }
  if (rows.length === 0) return <p className="p-3 text-sm text-fg-tertiary">No matches</p>
  return (
    <nav className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1" aria-label="Studies">
      {rows.map((s) => (
        <Row key={s.slug} info={s} active={active === s.slug} />
      ))}
    </nav>
  )
}

/** The pill of a study's state; shared by the rail, the header and the routines row. */
export function StatePill({ state, currentN, stop, short, className }: { state: StudyState; currentN?: number | null; stop?: StopReason | null; /** The bare word (the rail); the header spells out the reason. */ short?: boolean; className?: string }) {
  return <span className={cn('inline-flex h-5 shrink-0 items-center rounded-full border px-2 text-[11px] font-medium whitespace-nowrap', STATE_CLASS[state.state], className)}>{short ? state.state : stateLabel(state, currentN, stop)}</span>
}

const Row = memo(function Row({ info: s, active }: { info: StudyInfo; active: boolean }) {
  const better = improved(s.metric.direction, s.delta_pct)
  return (
    <Link
      to="/autoresearch/$slug"
      params={{ slug: s.slug }}
      search={{}}
      ref={active ? reveal : undefined}
      className={cn('flex flex-col gap-0.5 rounded-[10px] px-2.5 py-2 text-sm hover:bg-layer-2', active && 'bg-layer-3 hover:bg-layer-3')}
    >
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate font-medium">{s.slug}</span>
        <StatePill state={s.state} short />
      </div>
      <div className="flex items-center gap-2 text-xs text-fg-secondary">
        <span className={cn('tabular-nums font-medium', s.delta_pct !== null && (better ? 'text-success' : 'text-fg-secondary'))}>{s.delta_pct === null ? (s.state.state === 'preparing' ? 'measuring baseline' : '—') : pct(s.delta_pct)}</span>
        <span className="text-fg-tertiary">·</span>
        <span className="tabular-nums">{experimentsText(s)} experiments</span>
        <span className="min-w-0 flex-1 truncate text-fg-tertiary">{s.metric.name}</span>
      </div>
      <p className="truncate text-xs text-fg-tertiary" title={s.goal}>
        {s.goal}
      </p>
    </Link>
  )
})

/** Callback ref for the active row: scroll it into the rail when it (re)mounts or becomes active. */
function reveal(el: HTMLAnchorElement | null) {
  el?.scrollIntoView({ block: 'nearest' })
}
