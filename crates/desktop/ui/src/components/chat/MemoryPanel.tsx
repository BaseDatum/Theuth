import { useQuerySupport } from '@/rpc/capabilities'
import { type QueryClient, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { LoaderCircle, TriangleAlert } from 'lucide-react'
import { Fragment, type KeyboardEvent, type ReactNode, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { toast } from 'sonner'
import { SummaryTestCall } from '@/components/settings/SummaryTestCall'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { ago } from '@/lib/costs/format'
import { fmtBytes } from '@/lib/files'
import { ENDLESS_DEFAULTS, EARLIER_HISTORY, isFoldedNote, tailMark, tokenEstimate } from '@/lib/endless'
import { routed } from '@/lib/owner'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/bindings/client'
import type { Backlog as EndlessBacklog } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { GuardTrips } from "@/rpc/bindings/method.agent.output"
import type { SessionMemory } from "@/rpc/bindings/method.agent.output"
import type { Stalled } from "@/rpc/bindings/method.agent.output"
import type { ViewLine } from "@/rpc/bindings/method.agent.output"
import type { TailMode, TailOverride } from "@/rpc/bindings/method.agent.output"
import { MenuRow } from './HeaderMenu'
import { ModelCombobox } from './ModelCombobox'
import { settings } from './slash'
import { useModels } from './ModelMenu'

/** `id+n`, the line's address in the view (137 §5.1); `prefix` keeps a recall hit apart from the same line in the view. */
export const lineKey = (l: ViewLine, prefix = ''): string => `${prefix}${l.id}+${l.n}`

/** Bytes of the view as rendered (`id+n|text\n` per line, 137 §7.2). */
export const viewBytes = (view: readonly ViewLine[]): number => view.reduce((n, l) => n + new TextEncoder().encode(l.text).length + lineKey(l).length + 2, 0)

/** The badge's tooltip (137 §11.3): `Fractal context · view 61 KB · 71 lines · up to date` / `… settling 3 lines` / `… backfill 12/80` / `… summaries failing` (137F) / `… settling paused` (137G). */
export function badgeTip(m: SessionMemory): string {
  const waiting = m.backlog?.lines ?? m.unbuilt
  const state = m.stalled
    ? 'summaries failing'
    : m.backlog?.paused
      ? 'settling paused'
      : m.backfill
        ? `backfill ${m.backfill.done}/${m.backfill.total}`
        : m.settling
          ? `settling ${waiting.toLocaleString('en-US')} line${waiting === 1 ? '' : 's'}`
          : 'up to date'
  return `Fractal context · view ${fmtBytes(viewBytes(m.view))} · ${m.view.length} line${m.view.length === 1 ? '' : 's'} · ${state}`
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
  const canQuery1 = useQuerySupport(["session_memory"])
  const api = useApi()
  return useQuery({ queryKey: queryKeys.sessionMemory(session), queryFn: () => api.session_memory(session), enabled: canQuery1 && (enabled) })
}

/**
 * The header's `∞` badge (137 §11.3): muted pill after the title, same height as the cost pill; tooltip with view size
 * and settle state; pulses softly (motion-safe) while the compactor is settling or backfilling; click toggles the panel.
 * 137F: a failing settle turns it to the warning tone and stops the pulse. `row`: the compact menu's line.
 */
export function MemoryBadge({ session, summary, row, open, onToggle }: { session: string; summary: SessionSummary | undefined; row?: boolean; open: boolean; onToggle(): void }) {
  const endless = summary?.memory === 'endless'
  const memory = useSessionMemory(session, endless)
  if (!endless) return null
  const m = memory.data
  const failing = !!m?.stalled
  const busy = !!m && !failing && !m.backlog?.paused && (m.settling || m.backfill !== null)
  if (row)
    return (
      <MenuRow
        icon={<span className={cn('size-4 shrink-0 text-center text-sm leading-4', failing ? 'text-warning' : 'text-fg-secondary')}>∞</span>}
        label="Fractal context"
        detail={m ? (failing ? <span className="text-warning">summaries failing</span> : busy ? 'settling' : `${m.view.length} lines`) : undefined}
        onSelect={onToggle}
      />
    )
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label="Fractal context"
          aria-expanded={open}
          className={cn(
            'shrink-0 rounded-full border border-border-subtle px-2.5 py-0.5 text-sm leading-4 text-fg-secondary hover:text-foreground',
            failing && 'border-warning-border bg-warning-bg text-warning hover:text-warning',
            busy && 'motion-safe:animate-pulse',
          )}
          onClick={onToggle}
        >
          ∞
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{m ? badgeTip(m) : memory.error ? `Fractal context · ${memory.error.message}` : 'Fractal context'}</TooltipContent>
    </Tooltip>
  )
}

/** R1b: the view budgets the compact control offers; a stored value outside the list is shown as its own row. */
export const VIEW_SIZES_KB = [32, 64, 128, 192, 256] as const

/** `SessionMemory.model_source` as the picker's hint: which link of the chain chose the summary model (137G). */
export const SOURCE_HINT: Record<string, string> = { chat: 'this chat', agent: 'agent default', settings: 'Settings', title: 'title model', session: 'chat model' }

/** The summary model picker's trigger id: the failing block's and the backlog row's "Change model" open it. */
export const SUMMARY_MODEL_ID = 'memory-summary-model'
const openSummaryModel = () => document.getElementById(SUMMARY_MODEL_ID)?.click()

/** Mutation options for the RPCs that answer the fresh `SessionMemory` (137F/137G): the reply replaces the cached memory; an error toasts and changes nothing.
 * 183 §4.4 (R1): they are the owner's calls, so each goes through `routed` — an unreachable owner parks it (no reply, nothing replaced). */
export const memoryMutation = <V = void,>(qc: QueryClient, session: string, mutationFn: (v: V) => Promise<SessionMemory>) => ({
  mutationFn: (v: V) => routed(session, () => mutationFn(v)),
  onSuccess: (next: SessionMemory | undefined) => next && qc.setQueryData(queryKeys.sessionMemory(session), next),
  onError: (e: Error) => void toast.error(e.message),
})
export const retryMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, () => api.endless_retry(session))
export const foldMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, () => api.endless_fold(session))
export const pauseMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, (paused: boolean) => api.endless_pause(session, paused))
export const modelMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, (model: string | null) => api.set_session_compactor_model(session, model))
/** 194: this chat's own summary effort; the reply (the fresh memory) also carries the effort a model change dropped. */
export const effortMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, (effort: string | null) => api.set_session_compactor_effort(session, effort))
/** 201 §4.2: this chat's own tail sizing (mode and its two marks); `null` = use the default again. */
export const tailMutation = (api: Api, qc: QueryClient, session: string) => memoryMutation(qc, session, (tail: TailOverride | null) => api.set_session_tail(session, tail))

/** 137G: this chat's summary model on the regular picker (124 §5); "Use default" clears the chat's own pick. 194: the chips belong to the chat's own pick — shown only beside it, at its own stored effort (the Default chip clears it). */
export function SummaryModel({ session, m }: { session: string; m: Pick<SessionMemory, 'model' | 'model_override' | 'model_source' | 'effort_override'> }) {
  const api = useApi()
  const qc = useQueryClient()
  const models = useModels(session || null)
  const setModel = useMutation(modelMutation(api, qc, session))
  const setEffort = useMutation(effortMutation(api, qc, session))
  const hint = m.model_source ? SOURCE_HINT[m.model_source] : undefined
  const current = m.model_override ? models.find((x) => x.id === m.model_override) : undefined
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <ModelCombobox
        id={SUMMARY_MODEL_ID}
        aria-label="Summary model"
        size="sm"
        className="h-6 max-w-56 gap-1 px-2 text-xs text-fg-secondary"
        models={models}
        value={m.model}
        none={{ label: 'Use default' }}
        disabled={setModel.isPending || setEffort.isPending}
        onChange={(id) => session ? setModel.mutate(id) : useDrafts.getState().setSettings({ compactorModel: id, compactorEffort: null })}
        effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: m.effort_override ?? null, onSelect: (level) => session ? setEffort.mutate(level) : useDrafts.getState().setSettings({ compactorEffort: level }) } : undefined}
      />
      {hint && <span className="shrink-0 text-fg-tertiary">{hint}</span>}
    </span>
  )
}

function ViewSize({ bytes, busy, onChange }: { bytes: number; busy?: boolean; onChange(bytes: number): void }) {
  const kb = bytes / 1000
  const sizes = VIEW_SIZES_KB.includes(kb as (typeof VIEW_SIZES_KB)[number]) ? VIEW_SIZES_KB : [...VIEW_SIZES_KB, kb].sort((a, b) => a - b)
  return (
    <Select value={String(kb)} disabled={busy} onValueChange={(v) => onChange(Number(v) * 1000)}>
      <SelectTrigger size="sm" className="h-6 gap-1 px-2 text-xs text-fg-secondary" aria-label="View size" title={tokenEstimate(bytes)}>
        <SelectValue>view {kb} KB</SelectValue>
      </SelectTrigger>
      <SelectContent align="start">
        {sizes.map((s) => (
          <SelectItem key={s} value={String(s)} className="text-xs">
            view {s} KB <span className="text-fg-tertiary">{tokenEstimate(s * 1000)}</span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

/** 201 §4.2: the per-session tail sizes the compact control offers, in each mode's wire unit; a stored override outside the list is shown as its own row. */
export const TAIL_PRESETS: readonly TailOverride[] = [
  { mode: 'fixed', low: 16_000, high: 48_000 },
  { mode: 'fixed', low: 24_000, high: 64_000 },
  { mode: 'fixed', low: 48_000, high: 128_000 },
  { mode: 'fixed', low: 96_000, high: 256_000 },
  { mode: 'scaled', low: 0.05, high: 0.15 },
  { mode: 'scaled', low: 0.1, high: 0.25 },
  { mode: 'scaled', low: 0.2, high: 0.5 },
]
const tailKey = (t: TailOverride) => `${t.mode}:${t.low}:${t.high}`
/** `24–64 KB` / `10–25 %`: a tail pair in its mode's unit. */
export const tailRange = (mode: TailMode, low: number, high: number): string => `${tailMark(mode, low).split(' ')[0]}–${tailMark(mode, high)}`
const DEFAULT_TAIL = 'default'

/** 201 §4.2: this chat's tail sizing beside its view size. "Use the default" clears the chat's own pick; the trigger names the marks in force. */
export function TailSize({ m, busy, onChange }: { m: Pick<SessionMemory, 'tail_mode' | 'tail_low' | 'tail_high' | 'tail_override'>; busy?: boolean; onChange(tail: TailOverride | null): void }) {
  const own = m.tail_override ?? null
  const options = own && !TAIL_PRESETS.some((p) => tailKey(p) === tailKey(own)) ? [...TAIL_PRESETS, own] : TAIL_PRESETS
  const mode = m.tail_mode ?? ENDLESS_DEFAULTS.tail_mode
  const inForce = own ? tailRange(own.mode, own.low, own.high) : m.tail_high > 0 ? tailRange('fixed', m.tail_low, m.tail_high) : mode
  return (
    <Select value={own ? tailKey(own) : DEFAULT_TAIL} disabled={busy} onValueChange={(v) => onChange(v === DEFAULT_TAIL ? null : (options.find((o) => tailKey(o) === v) ?? null))}>
      <SelectTrigger size="sm" className="h-6 gap-1 px-2 text-xs text-fg-secondary" aria-label="Tail size" title="Kept after a trim – trim at">
        <SelectValue>
          tail {inForce}
          {!own && <span className="text-fg-tertiary"> · default</span>}
        </SelectValue>
      </SelectTrigger>
      <SelectContent align="start">
        <SelectItem value={DEFAULT_TAIL} className="text-xs">
          Use the default
        </SelectItem>
        {options.map((t) => (
          <SelectItem key={tailKey(t)} value={tailKey(t)} className="text-xs">
            tail {tailRange(t.mode, t.low, t.high)} <span className="text-fg-tertiary">{t.mode === 'fixed' ? 'fixed' : 'of the model'}</span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

/** 201: the tail against its two marks, as a small bar with a tick at the low mark; text only while the marks are unknown (scaled, before P2). */
function TailBar({ m }: { m: SessionMemory }) {
  const kb = (b: number) => `${Math.round(b / 1000)} KB`
  if (!(m.tail_high > 0)) return <span className="tabular-nums">tail {kb(m.tail_bytes ?? 0)}</span>
  const fill = Math.min(100, ((m.tail_bytes ?? 0) / m.tail_high) * 100)
  return (
    <span className="flex items-center gap-1.5 tabular-nums" title={`Trimmed back to ${kb(m.tail_low)} once it passes ${kb(m.tail_high)}`}>
      tail {kb(m.tail_bytes ?? 0)}
      <span role="meter" aria-label="Tail size" aria-valuemin={0} aria-valuemax={m.tail_high} aria-valuenow={m.tail_bytes ?? 0} className="relative h-1.5 w-16 overflow-hidden rounded-full bg-muted">
        <span className="absolute inset-y-0 left-0 bg-primary/70" style={{ width: `${fill}%` }} />
        <span className="absolute inset-y-0 w-px bg-fg-tertiary" style={{ left: `${(m.tail_low / m.tail_high) * 100}%` }} />
      </span>
      <span className="text-fg-tertiary">{tailRange('fixed', m.tail_low, m.tail_high)}</span>
    </span>
  )
}

/**
 * 201: the live numbers of the session-memory status, one renderer per entry (`null` = nothing to show). Later phases
 * append theirs here (ledger counts, zooms this generation, a pending tool-list change).
 */
export const LIVE: readonly ((m: SessionMemory) => ReactNode)[] = [
  (m) => <TailBar m={m} />,
  (m) => (
    <span className="tabular-nums" title="Trims taken so far">
      generation {m.generation ?? 0}
    </span>
  ),
  (m) => (
    <span className="tabular-nums" title="Lines still on their quick draft, waiting for the summary model">
      {(m.drafts ?? 0).toLocaleString('en-US')} on a draft
    </span>
  ),
  (m) => (
    <span title="The summariser's profile in force">
      {m.profile ?? 'standard'} profile{m.profile_reason ? ` · ${m.profile_reason}` : ''}
    </span>
  ),
]

export function Live({ m }: { m: SessionMemory }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
      {LIVE.map((render, i) => (
        <Fragment key={i}>{render(m)}</Fragment>
      ))}
    </div>
  )
}

/** The same memory choices, held on the draft until its first action creates the chat. */
export function DraftMemory({ open, onOpenChange }: { open: boolean; onOpenChange(o: boolean): void }) {
  const api = useApi()
  const supported = useQuerySupport(['endless_settings_get'])
  const defaults = useQuery({ queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get(), enabled: supported && open })
  const pending = useDrafts((s) => s.settings)
  const set = useDrafts((s) => s.setSettings)
  const mode = pending.create?.memory ?? (defaults.data?.default_chat ? 'endless' : 'epochs')
  const bytes = pending.viewBytes ?? defaults.data?.view_bytes
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[560px] sm:max-w-[560px]">
        <SheetHeader>
          <SheetTitle>Context</SheetTitle>
          <SheetDescription>These choices apply before the first message.</SheetDescription>
        </SheetHeader>
        <div className="flex flex-col items-start gap-4 px-4">
          <Select value={mode} onValueChange={(memory) => useDrafts.getState().setCreate({ memory: memory as 'endless' | 'epochs' })}>
            <SelectTrigger aria-label="Context mode"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value="epochs">Epochs</SelectItem><SelectItem value="endless">Fractal context</SelectItem></SelectContent>
          </Select>
          <SummaryModel session="" m={{ model: pending.compactorModel ?? defaults.data?.model ?? '', model_override: pending.compactorModel ?? null, model_source: pending.compactorModel ? 'chat' : 'settings', effort_override: pending.compactorEffort ?? null }} />
          {bytes !== undefined && <ViewSize bytes={bytes} onChange={(viewBytes) => set({ viewBytes })} />}
          {defaults.error && <p className="text-sm text-destructive">{defaults.error.message}</p>}
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** Mode · summary model (137G picker) · view size (per-session, R1b) · calls · guard trips (hover: per guard) · cost estimate. */
function Stats({ session, m }: { session: string; m: SessionMemory }) {
  const api = useApi()
  const qc = useQueryClient()
  const setViewBytes = useMutation(memoryMutation(qc, session, (bytes: number) => api.set_session_view_bytes(session, bytes)))
  const setTail = useMutation(tailMutation(api, qc, session))
  const g = m.stats.guard_trips
  const trips = guardTotal(g)
  const usd = m.stats.usd_estimate
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
      <Badge variant="outline" className="text-fg-secondary">
        {m.mode}
      </Badge>
      <SummaryModel session={session} m={m} />
      <ViewSize bytes={m.view_bytes} busy={setViewBytes.isPending} onChange={(bytes) => setViewBytes.mutate(bytes)} />
      <TailSize m={m} busy={setTail.isPending} onChange={(tail) => setTail.mutate(tail)} />
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

const EXCERPT_TIP = 'Excerpt — not a model summary; recall finds the original'

/** One view line and, below it, its two children (or the whole message for `n = 1`) in an animated-height well. 137G: a mechanical line carries an "excerpt" tag. */
export function LineNode({ line, prefix, depth, kids, open, cursorKey, query, onToggle }: NodeProps) {
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
        <span className={cn('min-w-0 flex-1 wrap-anywhere', !line.built && 'text-fg-tertiary italic')}>
          {highlight(line.text, query)}
          {line.mechanical && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Badge variant="outline" className="ml-1.5 h-4 px-1 align-middle text-[10px] font-normal text-fg-tertiary" aria-label={EXCERPT_TIP}>
                  excerpt
                </Badge>
              </TooltipTrigger>
              <TooltipContent side="top">{EXCERPT_TIP}</TooltipContent>
            </Tooltip>
          )}
        </span>
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

/** `~4 min` / `~40 s` for the backfill's and the backlog's ETA. */
const eta = (secs: number): string => (secs >= 90 ? `~${Math.round(secs / 60)} min` : `~${Math.round(secs)} s`)

/** `Next try in 40 s` (`in a moment` once due); `Retrying now…` while the retry runs (`next_retry_at` null). */
export function nextTry(s: Stalled, now: number): string {
  if (!s.next_retry_at) return 'Retrying now…'
  const at = Date.parse(s.next_retry_at)
  return `Next try ${at - now >= 5_000 ? ago(at, now) : 'in a moment'}`
}

/** 137G Fold now, shared by the failing block and the backlog row; the footer owns the mutation. */
type Fold = { run(): void; pending: boolean }

function FoldButton({ fold }: { fold: Fold }) {
  return (
    <Button size="xs" variant="outline" disabled={fold.pending} onClick={fold.run}>
      {fold.pending && <LoaderCircle className="animate-spin" />}
      Fold now
    </Button>
  )
}

function ChangeModel() {
  return (
    <Button size="xs" variant="ghost" onClick={openSummaryModel}>
      Change model
    </Button>
  )
}

/** 137F: a view part keeps failing — what fails (full error on hover), when the summariser tries again, Retry now, Fold now (137G) and Change model (the header's picker). */
function Failing({ session, s, fold }: { session: string; s: Stalled; fold: Fold }) {
  const api = useApi()
  const qc = useQueryClient()
  const retry = useMutation(retryMutation(api, qc, session))
  const [now, setNow] = useState(Date.now)
  // The countdown ticks while the panel shows it; no timer while the retry runs.
  useEffect(() => {
    if (!s.next_retry_at) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1_000)
    return () => clearInterval(t)
  }, [s.next_retry_at])
  return (
    <div role="status" className="flex flex-col gap-1 rounded-md border border-warning-border bg-warning-bg px-3 py-2 text-xs">
      <Tooltip>
        <TooltipTrigger asChild>
          <p className="flex min-w-0 items-center gap-1.5 font-medium text-warning">
            <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
            <span className="truncate">
              {`Summaries are failing · ${s.failures} attempt${s.failures === 1 ? '' : 's'} · ${s.error}`}
            </span>
          </p>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-96 break-words">
          {s.error}
        </TooltipContent>
      </Tooltip>
      <p className="truncate text-fg-secondary">
        {`${nextTry(s, now)} · model ${s.model}`}
      </p>
      {s.blocking && <p className="text-fg-secondary">Newer lines wait behind this one.</p>}
      <p className="text-fg-secondary">Lines that fail 3 times are folded as excerpts automatically.</p>
      <div className="mt-1 flex flex-wrap gap-2">
        <Button size="xs" variant="outline" disabled={retry.isPending} onClick={() => retry.mutate()}>
          {retry.isPending && <LoaderCircle className="animate-spin" />}
          Retry now
        </Button>
        <FoldButton fold={fold} />
        <ChangeModel />
      </div>
      {/* 177 U5: the failing model, tried once on a fixed sample. */}
      <SummaryTestCall model={s.model} size="xs" />
    </div>
  )
}

/** Label left, detail right, then a determinate bar (`pct` 0–100) when given. */
function Bar({ label, detail, pct, strong }: { label: string; detail: string; pct?: number; strong?: boolean }) {
  return (
    <>
      <div className="flex items-center justify-between gap-2 text-xs text-fg-secondary">
        <span className={cn(strong && 'font-medium text-foreground')}>{label}</span>
        <span className="shrink-0 tabular-nums">{detail}</span>
      </div>
      {pct !== undefined && (
        <div role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={label} className="h-1 w-full overflow-hidden rounded-full bg-muted">
          <div className="h-full rounded-full bg-primary transition-[width] motion-safe:duration-500" style={{ width: `${pct}%` }} />
        </div>
      )}
    </>
  )
}

/** Determinate bar for settle (`built / lines`) or backfill (R1-D4: `done / total`, the ETA and the projected cost). */
function Progress({ m }: { m: SessionMemory }) {
  const b = m.backfill
  const [label, done, total] = b
    ? [`Building the view · ${b.done}/${b.total}${b.eta_secs !== null ? ` · ${eta(b.eta_secs)}` : ''} · ≈ $${b.usd_estimate < 1 ? b.usd_estimate.toFixed(3) : b.usd_estimate.toFixed(2)}`, b.done, b.total]
    : [`Settling ${m.unbuilt} line${m.unbuilt === 1 ? '' : 's'}`, m.view.length - m.unbuilt, m.view.length]
  const pct = total > 0 ? Math.round((done / total) * 100) : 0
  return <Bar label={label} detail={`${pct}%`} pct={pct} />
}

const help = 'text-[11px] leading-4 text-fg-tertiary'
const count = (n: number, one: string) => `${n.toLocaleString('en-US')} ${one}${n === 1 ? '' : 's'}`

/**
 * 137G: the waiting lines. Settling: count, ETA, batch size and a bar of the chat's lines summarized so far; Fold now,
 * Pause settling. Paused by the user: Resume settling. Paused by the day's spend cap: the cap, today's spend, Resume for today.
 */
function BacklogRow({ session, m, b, fold }: { session: string; m: SessionMemory; b: EndlessBacklog; fold: Fold }) {
  const canQuery3 = useQuerySupport(["endless_settings_get"])
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const cfg = useQuery({ enabled: canQuery3, queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() }).data
  const pause = useMutation(pauseMutation(api, qc, session))
  const toggle = (paused: boolean, label: string, primary = false) => (
    <Button size="xs" variant={primary ? 'default' : 'outline'} disabled={pause.isPending} onClick={() => pause.mutate(paused)}>
      {pause.isPending && <LoaderCircle className="animate-spin" />}
      {label}
    </Button>
  )
  if (b.paused_by === 'spend') {
    const cap = cfg?.max_usd_per_session_day ?? ENDLESS_DEFAULTS.max_usd_per_session_day
    return (
      <>
        <Bar strong label={`Paused: today's summary spend limit ($${cap.toFixed(2)}) reached`} detail={`$${(m.spend_today_usd ?? 0).toFixed(2)} spent`} />
        <p className={help}>{count(b.lines, 'line')} waiting. Resuming lifts the limit for this chat until midnight UTC.</p>
        <div className="flex flex-wrap items-center gap-2">
          {toggle(false, 'Resume for today', true)}
          <FoldButton fold={fold} />
          <Button size="xs" variant="link" className="px-1" onClick={() => void settings(navigate, 'memory', 'spend')}>
            Spend limit settings
          </Button>
        </div>
      </>
    )
  }
  if (b.paused)
    return (
      <>
        <Bar strong label="Settling paused" detail={`${count(b.lines, 'line')} waiting`} />
        <p className={help}>No summary calls run for this chat. Turns still fit: older waiting lines stay folded.</p>
        <div className="flex flex-wrap gap-2">
          {toggle(false, 'Resume settling', true)}
          <FoldButton fold={fold} />
          <ChangeModel />
        </div>
      </>
    )
  const batch = cfg?.batch_units ?? ENDLESS_DEFAULTS.batch_units
  const shown = count(b.lines - b.folded, 'line')
  return (
    <>
      <Bar
        strong
        label={`Settling ${count(b.lines, 'line')}`}
        detail={`${b.eta_secs > 0 ? `${eta(b.eta_secs)} · ` : ''}batches of ${batch}`}
        pct={m.units > 0 ? Math.round((Math.max(0, m.units - b.lines) / m.units) * 100) : 0}
      />
      <p className={help}>
        {b.folded > 0 ? `Newest ${shown} shown in full; older ones are folded until summarized.` : 'Waiting lines are shown in full until summarized.'} Model {m.model}.
      </p>
      <div className="flex flex-wrap gap-2">
        <FoldButton fold={fold} />
        {toggle(true, 'Pause settling')}
        <ChangeModel />
      </div>
      <p className={help}>Fold now: turns every waiting line into a short excerpt at once, at no cost. Recall still finds the originals.</p>
    </>
  )
}

/** The panel's footer: the failing settle (137F) above the backlog row (137G) or the settle/backfill bar; with no backlog, the view's excerpt lines (Fold now or the automatic fold); nothing once all are done. */
export function MemoryFooter({ session, m }: { session: string; m: SessionMemory }) {
  const api = useApi()
  const qc = useQueryClient()
  const foldRpc = useMutation(foldMutation(api, qc, session))
  const fold: Fold = { pending: foldRpc.isPending, run: () => foldRpc.mutate() }
  const b = m.backlog
  // Units the view shows as mechanical excerpts; they merge into model summaries as the pump goes on.
  const excerpts = m.view.reduce((n, l) => (l.mechanical ? n + l.n : n), 0)
  if (m.mode !== 'endless' || !(m.stalled || m.settling || m.backfill || b || excerpts)) return null
  return (
    <SheetFooter className="gap-1.5 border-t border-border-subtle">
      {m.stalled && <Failing session={session} s={m.stalled} fold={fold} />}
      {b && <BacklogRow session={session} m={m} b={b} fold={fold} />}
      {(m.backfill || (m.settling && !b)) && <Progress m={m} />}
      {excerpts > 0 && !b && (
        <>
          <Bar strong label={m.settling ? 'Folded' : 'Up to date'} detail={`${count(excerpts, 'line')} shown as excerpts`} />
          <p className={help}>Excerpts merge into summaries as usual from here.</p>
          <div className="flex gap-2">
            <ChangeModel />
          </div>
        </>
      )}
    </SheetFooter>
  )
}

const HIT = 'hit:'

/** A view line that is a note, not a zoomable band line: the capped backfill's `(earlier history …)` (R1-D4) and the two 137G placeholders (`isFoldedNote`). */
export function ViewNote({ line, className }: { line: ViewLine; className?: string }) {
  return (
    <p role="note" className={cn('mb-1 flex gap-2 px-2 py-1 text-xs text-fg-tertiary italic', className)}>
      <span className="w-16 shrink-0 text-right font-mono text-[11px] not-italic tabular-nums">
        {line.id}+{line.n}
      </span>
      {line.text}
    </p>
  )
}

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

  // R1-D4: a capped backfill leads the view with the `(earlier history …)` sentinel — a note, not a zoomable band line.
  const earlier = memory.data?.view[0]?.text === EARLIER_HISTORY ? memory.data.view[0] : undefined
  const view = useMemo(() => (earlier ? memory.data?.view.slice(1) : memory.data?.view), [memory.data, earlier])
  // 137G: the view's folded placeholder is a note too; neither is in the keyboard order.
  const rows = useMemo(() => [...(hits ? flatten(hits.lines, kids, opened, HIT) : []), ...(view ? flatten(view.filter((l) => !isFoldedNote(l.text)), kids, opened) : [])], [hits, view, kids, opened])
  const cursorKey = rows[cursor]?.key

  useEffect(() => {
    treeEl.current?.querySelector(`[data-key="${CSS.escape(cursorKey ?? '')}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [cursorKey])

  const toggle = (line: ViewLine, key: string) => {
    setCursor(Math.max(0, rows.findIndex((r) => r.key === key)))
    dispatch({ type: 'toggle', key })
    if (opened.has(key) || kids[key]) return
    // 183 §4.4: the owner's read; unreachable, it parks behind the chat's card (`undefined`) and the row stays unloaded.
    routed(session, () => api.endless_zoom(session, line.id, line.n))
      .then((below) => {
        if (below === undefined) return dispatch({ type: 'failed', key })
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
    routed(session, () => api.endless_recall(session, trimmed, 20))
      .then((lines) => {
        if (lines === undefined) return
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
          <SheetTitle>Context</SheetTitle>
          <SheetDescription className="sr-only">The session's view by band; open a line to see what it summarises.</SheetDescription>
          {m && m.mode === 'endless' && <Stats session={session} m={m} />}
          {m && m.mode === 'endless' && <Live m={m} />}
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
        <div ref={treeEl} role="tree" aria-label="Context view" tabIndex={0} className="min-h-0 flex-1 overflow-y-auto px-2 pb-3 outline-none focus-visible:ring-1 focus-visible:ring-ring/40 focus-visible:ring-inset" onKeyDown={onTreeKey}>
          {memory.isPending && (
            <div className="flex flex-col gap-2 px-2 pt-1">
              {[0, 1, 2, 3, 4, 5].map((i) => (
                <Skeleton key={i} className="h-5 w-full" />
              ))}
            </div>
          )}
          {memory.isError && <p className="px-2 py-4 text-sm text-fg-tertiary">Context details unavailable: {memory.error.message}</p>}
          {m && m.mode !== 'endless' && <p className="px-2 py-4 text-sm text-fg-tertiary">This session keeps its context in epochs; `/fractal on` switches it when idle.</p>}
          {hits && (
            <section aria-label="Recall hits" className="mb-3">
              <div className="mt-1 mb-0.5 px-2 text-[10px] font-medium tracking-wide text-fg-tertiary uppercase">
                {searching ? 'searching…' : `${hits.lines.length} hit${hits.lines.length === 1 ? '' : 's'} for “${hits.query}”`}
              </div>
              {hits.lines.length === 0 && !searching && <p className="px-2 py-1 text-sm text-fg-tertiary">Nothing matched.</p>}
              {hits.lines.map((l) => node(l, HIT))}
            </section>
          )}
          {earlier && <ViewNote line={earlier} />}
          {view?.map((line, i) =>
            isFoldedNote(line.text) ? (
              <ViewNote key={lineKey(line)} line={line} className="my-1 rounded-md bg-muted/50" />
            ) : (
              <Fragment key={lineKey(line)}>
                {(i === 0 || view[i - 1]?.n !== line.n) && (
                  <div role="presentation" className={cn('mb-0.5 px-2 text-[10px] font-medium tracking-wide text-fg-tertiary uppercase tabular-nums', i > 0 && 'mt-2')}>
                    ×{line.n}
                  </div>
                )}
                {node(line, '')}
              </Fragment>
            ),
          )}
          {m && m.mode === 'endless' && m.view.length === 0 && <p className="px-2 py-4 text-sm text-fg-tertiary">Nothing in the view yet: it fills as the chat goes.</p>}
        </div>
        {m && <MemoryFooter session={session} m={m} />}
      </SheetContent>
    </Sheet>
  )
}
