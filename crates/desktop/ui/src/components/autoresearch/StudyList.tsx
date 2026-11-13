import { Link, useRouterState } from '@tanstack/react-router'
import { Trash2 } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { DraftSummary } from "@/rpc/bindings/method.autoresearch.output"
import type { StopReason } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyInfo } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyState } from "@/rpc/bindings/method.autoresearch.output"
import { STATE_CLASS, delta, experimentsText, matches, sortStudies, stateLabel } from './model'
import { useDiscardDraft, useDrafts, useStudies } from './queries'
import { discardMessage } from './setup'

/** 159 W7 H7: a meta study's inner study (`created_by: meta:<outer>/…`); the rail hides those unless asked. */
const isInner = (s: StudyInfo) => s.created_by?.startsWith('meta:') ?? false

/**
 * The rail body: the setting-up drafts first (151 D6), then every study as a row — slug, state pill, headline delta,
 * experiments n/N, the goal. ponytail: not virtualized; studies are tens, not thousands.
 * `showInner` is the toggle's initial state (false; a test flips it).
 */
export function StudyList({ q, active, showInner: initialInner = false }: { q: string; active: string | undefined; showInner?: boolean }) {
  const list = useStudies()
  const drafts = useDrafts()
  const [showInner, setShowInner] = useState(initialInner)
  const inner = useMemo(() => (list.data ?? []).filter(isInner).length, [list.data])
  const rows = useMemo(() => sortStudies((list.data ?? []).filter((s) => (showInner || !isInner(s)) && matches(s, q))), [list.data, q, showInner])
  const setting = useMemo(() => (drafts.data ?? []).filter((d) => matches({ slug: d.draft_id, goal: d.goal }, q)), [drafts.data, q])
  // The open setup chat (`/autoresearch/new?chat=<origin>`) marks its draft's row.
  const chat = useRouterState({ select: (s): string | null => (typeof s.location.search.chat === 'string' ? s.location.search.chat : null) })
  const toggle = inner > 0 && (
    <Button variant="ghost" size="xs" className="mx-1 my-1 text-fg-tertiary" aria-pressed={showInner} onClick={() => setShowInner((v) => !v)}>
      {showInner ? 'Hide inner studies' : `Show ${inner} inner ${inner === 1 ? 'study' : 'studies'}`}
    </Button>
  )
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
  if (list.data.length === 0 && setting.length === 0) {
    return (
      <p className="p-3 text-sm text-fg-tertiary">
        No studies yet. Press <b>New study</b>, or ask theuth in any chat: <code className="font-mono text-[12px]">/autoresearch &lt;goal&gt;</code>
      </p>
    )
  }
  if (rows.length === 0 && setting.length === 0) {
    return (
      <>
        <p className="p-3 text-sm text-fg-tertiary">No matches</p>
        {toggle}
      </>
    )
  }
  return (
    <nav className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1" aria-label="Studies">
      {setting.map((d) => (
        <DraftRow key={d.draft_id} draft={d} active={active === 'new' && chat === d.origin} />
      ))}
      {rows.map((s) => (
        <Row key={s.slug} info={s} active={active === s.slug} />
      ))}
      {toggle}
    </nav>
  )
}

/** D6: a draft without a study — dashed amber dot, `Setting up · n of m settled`, Discard on hover (the label says the consequence; no second confirmation). */
function DraftRow({ draft: d, active }: { draft: DraftSummary; active: boolean }) {
  const api = useApi()
  // §4.6: the chat hears about the discard too, so the agent stops drafting.
  const discard = useDiscardDraft(() => {
    if (d.origin) void api.send_input(d.origin, discardMessage(d.draft_id), [], null)
  })
  return (
    <div className={cn('group relative rounded-[10px] text-sm hover:bg-layer-2', active && 'bg-layer-3 hover:bg-layer-3')} data-draft={d.draft_id}>
      <Link to="/autoresearch/new" search={d.origin ? { chat: d.origin } : {}} className="flex flex-col gap-0.5 px-2.5 py-2">
        <div className="flex items-center gap-2">
          <span aria-hidden className="size-2 shrink-0 rounded-full border border-dashed border-warning" />
          <span className="min-w-0 flex-1 truncate font-medium">{d.draft_id}</span>
          <span className="inline-flex h-5 shrink-0 items-center rounded-full border border-warning-border/40 bg-warning-bg px-2 text-[11px] font-medium whitespace-nowrap text-warning group-hover:invisible">{d.ready ? 'ready' : 'setting up'}</span>
        </div>
        <p className="text-xs text-fg-secondary tabular-nums">
          Setting up · {d.settled} of {d.total} settled
        </p>
        <p className="truncate text-xs text-fg-tertiary" title={d.goal || d.repo}>
          {d.goal || d.repo}
        </p>
      </Link>
      <Button variant="ghost" size="xs" className="absolute top-1.5 right-1.5 hidden text-fg-secondary group-hover:inline-flex" disabled={discard.isPending} onClick={() => discard.mutate(d.draft_id)} title="Deletes the draft; the chat stays">
        <Trash2 /> Discard draft
      </Button>
    </div>
  )
}

/** The pill of a study's state; shared by the rail, the header and the routines row. */
export function StatePill({ state, currentN, stop, short, className }: { state: StudyState; currentN?: number | null; stop?: StopReason | null; /** The bare word (the rail); the header spells out the reason. */ short?: boolean; className?: string }) {
  return <span className={cn('inline-flex h-5 shrink-0 items-center rounded-full border px-2 text-[11px] font-medium whitespace-nowrap', STATE_CLASS[state.state], className)}>{short ? state.state : stateLabel(state, currentN, stop)}</span>
}

/**
 * The one Δ rendering (140 §5.2 Revision 2): the raw signed change (`−2.6 %`; negative = the value went down), coloured
 * and worded (hover) by the metric's direction — a sign is never flipped. `muted` keeps the number grey (a run that was
 * not kept).
 */
export function Delta({ direction, delta_pct, muted, className }: { direction: 'lower' | 'higher'; delta_pct: number | null | undefined; muted?: boolean; className?: string }) {
  const d = delta(direction, delta_pct)
  const tone = muted || d.tone === 'muted' ? 'text-fg-secondary' : d.tone === 'success' ? 'text-success' : 'text-danger'
  return (
    <span className={cn('tabular-nums', tone, className)} title={d.word ? `${d.word} — ${direction} is better` : undefined}>
      {d.text}
    </span>
  )
}

const Row = memo(function Row({ info: s, active }: { info: StudyInfo; active: boolean }) {
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
        {s.delta_pct === null ? <span className="font-medium text-fg-secondary">{s.state.state === 'preparing' ? 'measuring baseline' : '—'}</span> : <Delta direction={s.metric.direction} delta_pct={s.delta_pct} className="font-medium" />}
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
