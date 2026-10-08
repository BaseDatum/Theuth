import { useQuery } from '@tanstack/react-query'
import { Fragment, type KeyboardEvent, type ReactNode, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { fmtBytes } from '@/lib/files'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { type GuardTrips, type SessionMemory, type ViewLine, memoryOf } from '@/rpc/endless-stubs'
import { MenuRow } from './HeaderMenu'

/** `id+n`, the line's address in the view (137 §5.1); `prefix` keeps a recall hit apart from the same line in the view. */
export const lineKey = (l: ViewLine, prefix = ''): string => `${prefix}${l.id}+${l.n}`

/** Bytes of the view as rendered (`id+n|text\n` per line, 137 §7.2). */
export const viewBytes = (view: readonly ViewLine[]): number => view.reduce((n, l) => n + new TextEncoder().encode(l.text).length + lineKey(l).length + 2, 0)

/** The badge's tooltip (137 §11.3): `Endless memory · view 61 KB · 71 lines · up to date` / `… settling 3 lines` / `… backfill 12/80`. */
export function badgeTip(m: SessionMemory): string {
  const state = m.backfill
    ? `backfill ${m.backfill.done}/${m.backfill.total}`
    : m.settling
      ? `settling ${m.unbuilt} line${m.unbuilt === 1 ? '' : 's'}`
      : 'up to date'
  return `Endless memory · view ${fmtBytes(viewBytes(m.view))} · ${m.view.length} line${m.view.length === 1 ? '' : 's'} · ${state}`
}

export const guardTotal = (g: GuardTrips): number => g.copy + g.grounding + g.status + g.format + g.empty + g.size

/** Children fetched per line key: two lines of `n/2`, or the whole unit for `n = 1`. */
export type Kids = Record<string, ViewLine[] | undefined>

export type Row = { line: ViewLine; depth: number; key: string; parent: string | null }

/** The visible rows in keyboard order: a line, then (when open) its children, recursively. */
export function flatten(view: readonly ViewLine[], kids: Kids, open: ReadonlySet<string>, prefix = '', depth = 0, parent: string | null = null): Row[] {
  const out: Row[] = []
  for (const line of view) {
    const key = lineKey(line, prefix)
    out.push({ line, depth, key, parent })
    const below = kids[key]
    if (line.n > 1 && below && open.has(key)) out.push(...flatten(below, kids, open, prefix, depth + 1, key))
  }
  return out
}

/** The panel's tree: fetched children per key, the open keys, and the current recall hits. */
export type Tree = { kids: Kids; open: ReadonlySet<string>; hits: { query: string; lines: ViewLine[] } | null }
export const INITIAL: Tree = { kids: {}, open: new Set(), hits: null }

export type Action =
  | { type: 'toggle'; key: string }
  | { type: 'loaded'; key: string; lines: ViewLine[] }
  /** The zoom failed: the line closes again (the toast is the component's). */
  | { type: 'failed'; key: string }
  | { type: 'hits'; hits: Tree['hits'] }

export function reduce(t: Tree, a: Action): Tree {
  switch (a.type) {
    case 'toggle': {
      const open = new Set(t.open)
      if (!open.delete(a.key)) open.add(a.key)
      return { ...t, open }
    }
    case 'loaded':
      return { ...t, kids: { ...t.kids, [a.key]: a.lines } }
    case 'failed': {
      const open = new Set(t.open)
      open.delete(a.key)
      return { ...t, open }
    }
    case 'hits':
      return { ...t, hits: a.hits }
  }
}

/** `text` with every word of `query` (2+ chars, case-insensitive) wrapped in `<mark>`; the plain text when nothing matches. */
export function highlight(text: string, query: string): ReactNode {
  const words = [...new Set(query.toLowerCase().split(/\s+/).filter((w) => w.length >= 2))]
  if (words.length === 0) return text
  const re = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi')
  const parts = text.split(re)
  if (parts.length === 1) return text
  return parts.map((p, i) =>
    i % 2 === 1 ? (
      <mark key={i} className="rounded-sm bg-info-bg px-0.5 text-foreground">
        {p}
      </mark>
    ) : (
      p
    ),
  )
}

/** `session_memory` (137 §11.2), shared by the badge and the panel; `useInvalidateOn` refetches it on `EndlessProgress`. */
export function useSessionMemory(session: string, enabled: boolean) {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.sessionMemory(session), queryFn: () => api.session_memory(session), enabled })
}

/**
 * The header's `∞` badge (137 §11.3): muted pill after the title, same height as the cost pill; tooltip with view size
 * and settle state; pulses softly (motion-safe) while the compactor is settling or backfilling; click toggles the panel.
 * `row`: the compact menu's line.
 */
export function MemoryBadge({ session, summary, row, open, onToggle }: { session: string; summary: SessionSummary | undefined; row?: boolean; open: boolean; onToggle(): void }) {
  const endless = summary !== undefined && memoryOf(summary) === 'endless'
  const memory = useSessionMemory(session, endless)
  if (!endless) return null
  const m = memory.data
  const busy = !!m && (m.settling || m.backfill !== null)
  if (row) return <MenuRow icon={<span className="size-4 shrink-0 text-center text-sm leading-4 text-fg-secondary">∞</span>} label="Endless memory" detail={m ? (busy ? 'settling' : `${m.view.length} lines`) : undefined} onSelect={onToggle} />
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label="Endless memory"
          aria-expanded={open}
          className={cn(
            'shrink-0 rounded-full border border-border-subtle px-2.5 py-0.5 text-sm leading-4 text-fg-secondary hover:text-foreground',
            busy && 'motion-safe:animate-pulse',
          )}
          onClick={onToggle}
        >
          ∞
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{m ? badgeTip(m) : memory.error ? `Endless memory · ${memory.error.message}` : 'Endless memory'}</TooltipContent>
    </Tooltip>
  )
}

/** Mode · compactor model · calls · guard trips (hover: per guard) · cost estimate. */
function Stats({ m }: { m: SessionMemory }) {
  const g = m.stats.guard_trips
  const trips = guardTotal(g)
  const usd = m.stats.usd_estimate
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
      <Badge variant="outline" className="text-fg-secondary">
        {m.mode}
      </Badge>
      <span className="truncate" title="Compactor model">
        {m.model}
      </span>
      <span className="tabular-nums">{m.stats.calls.toLocaleString('en-US')} calls</span>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="cursor-help underline decoration-dotted underline-offset-2 tabular-nums">
            {trips} guard trip{trips === 1 ? '' : 's'}
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          copy {g.copy} · grounding {g.grounding} · status {g.status} · format {g.format} · empty {g.empty} · size {g.size}
        </TooltipContent>
      </Tooltip>
      <span className="tabular-nums" title={`${m.stats.input_tokens.toLocaleString('en-US')} in (${m.stats.cache_read_tokens.toLocaleString('en-US')} cached) · ${m.stats.output_tokens.toLocaleString('en-US')} out`}>
        ≈ ${usd < 1 ? usd.toFixed(3) : usd.toFixed(2)}
      </span>
    </div>
  )
}

type NodeProps = {
  line: ViewLine
  prefix: string
  depth: number
  kids: Kids
  open: ReadonlySet<string>
  cursorKey: string | undefined
  query: string
  onToggle(line: ViewLine, key: string): void
}

/** One view line and, below it, its two children (or the whole message for `n = 1`) in an animated-height well. */
function LineNode({ line, prefix, depth, kids, open, cursorKey, query, onToggle }: NodeProps) {
  const key = lineKey(line, prefix)
  const below = kids[key]
  const isOpen = open.has(key)
  const loading = isOpen && !below
  return (
    <div role="none">
      <div
        role="treeitem"
        aria-level={depth + 1}
        aria-expanded={isOpen}
        aria-selected={cursorKey === key}
        data-key={key}
        className={cn(
          'flex cursor-pointer gap-2 rounded-md px-2 py-1 text-sm leading-5 hover:bg-accent/60',
          cursorKey === key && 'bg-accent',
          loading && 'motion-safe:animate-pulse',
        )}
        onClick={() => onToggle(line, key)}
      >
        <span className="w-16 shrink-0 text-right font-mono text-[11px] leading-5 text-fg-tertiary tabular-nums">
          {line.id}+{line.n}
        </span>
        <span className={cn('min-w-0 flex-1 wrap-anywhere', !line.built && 'text-fg-tertiary italic')}>{highlight(line.text, query)}</span>
      </div>
      <div className={cn('grid transition-[grid-template-rows] ease-out motion-safe:duration-200', isOpen && below ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]')}>
        <div className="min-h-0 overflow-hidden" inert={!isOpen || !below} aria-hidden={!isOpen}>
          {below &&
            (line.n === 1 ? (
              <pre className="mt-1 mb-2 ml-18 rounded-md border border-border-subtle bg-muted/40 p-2 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{below[0]?.text}</pre>
            ) : (
              <div role="group" className="ml-10 border-l border-border-subtle pl-2">
                {below.map((child) => (
                  <LineNode key={lineKey(child, prefix)} line={child} prefix={prefix} depth={depth + 1} kids={kids} open={open} cursorKey={cursorKey} query={query} onToggle={onToggle} />
                ))}
              </div>
            ))}
        </div>
      </div>
    </div>
  )
}

/** Determinate bar for settle (`built / lines`) or backfill (`done / total`, with the ETA). */
function Progress({ m }: { m: SessionMemory }) {
  const [label, done, total] = m.backfill
    ? [`Backfilling ${m.backfill.done} of ${m.backfill.total}${m.backfill.eta_secs !== null ? ` · ~${Math.ceil(m.backfill.eta_secs / 60)} min left` : ''}`, m.backfill.done, m.backfill.total]
    : [`Settling ${m.unbuilt} line${m.unbuilt === 1 ? '' : 's'}`, m.view.length - m.unbuilt, m.view.length]
  const pct = total > 0 ? Math.round((done / total) * 100) : 0
  return (
    <SheetFooter className="gap-1.5 border-t border-border-subtle">
      <div className="flex items-center justify-between text-xs text-fg-secondary">
        <span>{label}</span>
        <span className="tabular-nums">{pct}%</span>
      </div>
      <div role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={label} className="h-1 w-full overflow-hidden rounded-full bg-muted">
        <div className="h-full rounded-full bg-primary transition-[width] motion-safe:duration-500" style={{ width: `${pct}%` }} />
      </div>
    </SheetFooter>
  )
}

const HIT = 'hit:'

/**
 * Right-side sheet over an endless session's memory (137 §11.3): stats, recall search, the view grouped by band with
 * each line zoomable down to the message, and the compactor's progress. Keyboard on the tree: ↑/↓ move, → expand,
 * ← collapse (or up to the parent), Enter toggles, `/` focuses search, Esc closes (the sheet's own).
 */
export function MemoryPanel({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const api = useApi()
  const memory = useSessionMemory(session, open)
  const [tree, dispatch] = useReducer(reduce, INITIAL)
  const { kids, open: opened, hits } = tree
  const [query, setQuery] = useState('')
  const [searching, setSearching] = useState(false)
  const [cursor, setCursor] = useState(0)
  const search = useRef<HTMLInputElement>(null)
  const treeEl = useRef<HTMLDivElement>(null)

  const view = memory.data?.view
  const rows = useMemo(() => [...(hits ? flatten(hits.lines, kids, opened, HIT) : []), ...(view ? flatten(view, kids, opened) : [])], [hits, view, kids, opened])
  const cursorKey = rows[cursor]?.key

  useEffect(() => {
    treeEl.current?.querySelector(`[data-key="${CSS.escape(cursorKey ?? '')}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [cursorKey])

  const toggle = (line: ViewLine, key: string) => {
    setCursor(Math.max(0, rows.findIndex((r) => r.key === key)))
    dispatch({ type: 'toggle', key })
    if (opened.has(key) || kids[key]) return
    api
      .endless_zoom(session, line.id, line.n)
      .then((below) => {
        if (below.length === 0) throw new Error(`No line ${line.id}+${line.n}.`)
        dispatch({ type: 'loaded', key, lines: below })
      })
      .catch((e: unknown) => {
        dispatch({ type: 'failed', key })
        toast.error(e instanceof Error ? e.message : String(e))
      })
  }

  const recall = (q: string) => {
    const trimmed = q.trim()
    if (!trimmed) {
      dispatch({ type: 'hits', hits: null })
      return
    }
    setSearching(true)
    api
      .endless_recall(session, trimmed, 20)
      .then((lines) => {
        dispatch({ type: 'hits', hits: { query: trimmed, lines } })
        setCursor(0)
      })
      .catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
      .finally(() => setSearching(false))
  }

  const move = (i: number) => setCursor(Math.min(rows.length - 1, Math.max(0, i)))
  const onTreeKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const row = rows[cursor]
    switch (e.key) {
      case 'ArrowDown':
        move(cursor + 1)
        break
      case 'ArrowUp':
        move(cursor - 1)
        break
      case 'ArrowRight':
        if (!row) return
        if (opened.has(row.key) && kids[row.key]) move(cursor + 1)
        else toggle(row.line, row.key)
        break
      case 'ArrowLeft':
        if (!row) return
        if (opened.has(row.key)) toggle(row.line, row.key)
        else if (row.parent) move(rows.findIndex((r) => r.key === row.parent))
        break
      case 'Enter':
      case ' ':
        if (row) toggle(row.line, row.key)
        break
      default:
        return
    }
    e.preventDefault()
  }
  // `/` anywhere in the sheet but a text field: the search box.
  const onSheetKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== '/' || e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return
    e.preventDefault()
    search.current?.focus()
  }

  const m = memory.data
  const node = (line: ViewLine, prefix: string) => (
    <LineNode key={lineKey(line, prefix)} line={line} prefix={prefix} depth={0} kids={kids} open={opened} cursorKey={cursorKey} query={hits?.query ?? ''} onToggle={toggle} />
  )

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        className="w-full gap-0 sm:w-[560px] sm:max-w-[560px]"
        onKeyDown={onSheetKey}
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          treeEl.current?.focus()
        }}
      >
        <SheetHeader className="pb-3">
          <SheetTitle>Memory</SheetTitle>
          <SheetDescription className="sr-only">The session's view by band; open a line to see what it summarises.</SheetDescription>
          {m && m.mode === 'endless' && <Stats m={m} />}
        </SheetHeader>
        <div className="px-4 pb-3">
          <Input
            ref={search}
            type="text"
            placeholder="Recall: a word, an error, a path…"
            aria-label="Recall"
            value={query}
            disabled={!m || m.mode !== 'endless'}
            onChange={(e) => {
              setQuery(e.target.value)
              if (!e.target.value.trim()) dispatch({ type: 'hits', hits: null })
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') recall(query)
            }}
          />
        </div>
        <div ref={treeEl} role="tree" aria-label="Memory view" tabIndex={0} className="min-h-0 flex-1 overflow-y-auto px-2 pb-3 outline-none focus-visible:ring-1 focus-visible:ring-ring/40 focus-visible:ring-inset" onKeyDown={onTreeKey}>
          {memory.isPending && (
            <div className="flex flex-col gap-2 px-2 pt-1">
              {[0, 1, 2, 3, 4, 5].map((i) => (
                <Skeleton key={i} className="h-5 w-full" />
              ))}
            </div>
          )}
          {memory.isError && <p className="px-2 py-4 text-sm text-fg-tertiary">Memory details unavailable: {memory.error.message}</p>}
          {m && m.mode !== 'endless' && <p className="px-2 py-4 text-sm text-fg-tertiary">This session keeps its memory in epochs; `/endless on` switches it when idle.</p>}
          {hits && (
            <section aria-label="Recall hits" className="mb-3">
              <div className="mt-1 mb-0.5 px-2 text-[10px] font-medium tracking-wide text-fg-tertiary uppercase">
                {searching ? 'searching…' : `${hits.lines.length} hit${hits.lines.length === 1 ? '' : 's'} for “${hits.query}”`}
              </div>
              {hits.lines.length === 0 && !searching && <p className="px-2 py-1 text-sm text-fg-tertiary">Nothing matched.</p>}
              {hits.lines.map((l) => node(l, HIT))}
            </section>
          )}
          {view?.map((line, i) => (
            <Fragment key={lineKey(line)}>
              {(i === 0 || view[i - 1]?.n !== line.n) && (
                <div role="presentation" className={cn('mb-0.5 px-2 text-[10px] font-medium tracking-wide text-fg-tertiary uppercase tabular-nums', i > 0 && 'mt-2')}>
                  ×{line.n}
                </div>
              )}
              {node(line, '')}
            </Fragment>
          ))}
          {m && m.mode === 'endless' && m.view.length === 0 && <p className="px-2 py-4 text-sm text-fg-tertiary">Nothing in the view yet: it fills as the chat goes.</p>}
        </div>
        {m && m.mode === 'endless' && (m.settling || m.backfill) && <Progress m={m} />}
      </SheetContent>
    </Sheet>
  )
}
