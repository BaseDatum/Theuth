// 207 §8.5: one card of the flat Attention list — what it is, where it came from, why it sits here, and the answer
// itself (Allow / Deny, a question's options, Enter secret, a reply to a stopped session) without opening the item.
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Check, ChevronDown, Clock, EyeOff, Minus, Network } from 'lucide-react'
import { type ComponentProps, type MouseEvent, type ReactNode, memo, useState } from 'react'
import { DoubleChip } from '@/components/chat/DoubleChip'
import { SecretRequestCard, eventFields } from '@/components/chat/SecretRequestCard'
import { ATTENTION_KEY, SECRET_REQUESTS_KEY, useSecretRequestRows } from '@/components/hud/queries'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { age } from '@/components/tasks/helpers'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { useSecretRequests } from '@/store/approvals'
import { KindActions, SnoozeDismiss } from './actions'
import { chipLabel, doubleIds } from './labels'
import { pathText, reasonLine, stackRisk, urlHost } from './model'
import { setItemError, useAttentionBulk, useAttentionPending, useBulkPending, useInboxActions } from './queries'
import { useCardSwipe } from './swipe'
import { ItemError } from './ItemError'
import { Reply } from './Reply'

/** 132 D1/D4: `urgent` (carnelian) and `high` (Egyptian blue, the warning hue) show a chip after the kind chip; medium/low show nothing. */
export function PriorityChip({ priority }: { priority: AttentionItem['priority'] | undefined }) {
  if (priority !== 'urgent' && priority !== 'high') return null
  return <span className={cn('w-fit shrink-0 rounded-full border px-1.5 text-[10px] font-medium uppercase tracking-wide', priority === 'urgent' ? 'border-danger-border text-danger' : 'border-warning text-warning')}>{priority}</span>
}

/** A stopped session whose turn failed or timed out reads as a problem; a clean finish or a cancel does not. */
const troubled = (item: AttentionItem) => item.stop?.outcome === 'failed' || item.stop?.outcome === 'timed_out'

/** Kinds whose answer needs the pane (a packet, an artifact, several questions, 216 a plugin's card with its lines): the card links there. */
function needsPane(item: AttentionItem): boolean {
  if (item.kind === 'decision' || item.kind === 'review' || item.kind === 'show' || item.kind === 'web_view' || item.kind === 'plugin') return true
  if (item.kind !== 'ask') return false
  const qs = item.ask?.questions ?? []
  return qs.length !== 1 || (qs[0]?.options.length ?? 0) === 0
}

export type SelectHandler = (mode: 'toggle' | 'range', key: string) => void

type FrameProps = {
  item: AttentionItem
  now: number
  /** The entry key the callbacks receive (an item's id, a stack's key); defaults to the item's id. */
  entryKey?: string
  stack?: number
  active?: boolean
  focused?: boolean
  /** `some`: a stack with members the user had not seen when selecting it. */
  selected?: boolean | 'some'
  selecting?: boolean
  onSelect?: SelectHandler
  onOpen?: (key: string) => void
  /** 208 §7.4: centre the item's session on the Attention graph (shown when set and the item carries a session). */
  onShowInGraph?: (session: string) => void
  detail?: ReactNode
  /** Ids whose inline action error shows on this card (a stack's members); defaults to the item's id. */
  errorIds?: readonly string[]
  children?: ReactNode
} & Omit<ComponentProps<'li'>, 'onSelect' | 'children'>

/** The interactive things inside a card; a click on one of them is theirs, not the card's. */
const OWN_CLICK = 'a,button,input,textarea,select,form,label,[role="menu"],[role="menuitem"]'

/** Words a path segment already says, so the agent is not repeated after it ("ops desk", not "ops desk · ops"). */
const said = (segments: readonly string[], word: string) => segments.some((s) => s.toLowerCase().split(/[\s›·]+/).includes(word.toLowerCase()))

/**
 * The card's rows in scan order: what it is (kind · ×N · priority · double · done ··· age), the title, the one-line
 * facts (an approval's command in mono), where it came from (`project · root › child`, the agent when the path does not
 * already name it), why it sits here (the reason line, wrapping, never cut), then the actions.
 */
function Frame({ item, now, entryKey = item.id, errorIds, stack, active, focused, selected = false, selecting, onSelect, onOpen, onShowInGraph, detail, children, className, ...li }: FrameProps) {
  const session = typeof item.session === 'string' && item.session !== '' ? item.session : null
  const graph = onShowInGraph && session !== null && (
    <Button size="icon-xs" variant="ghost" aria-label="Show in graph" title="Show in graph" className="ml-auto text-fg-tertiary" onClick={() => onShowInGraph(session)}>
      <Network />
    </Button>
  )
  const path = pathText(item)
  const agent = item.agent && !said([...(item.path ?? []), item.project ?? ''], item.agent) ? item.agent : null
  const from = [path, agent].filter(Boolean).join(' · ')
  const reason = reasonLine(item, now)
  const pending = useAttentionPending(item.id)
  const swipe = useCardSwipe(item, stack === undefined)
  const click = (e: MouseEvent<HTMLDivElement>) => {
    if ((e.target as Element).closest(OWN_CLICK)) return
    if (e.shiftKey) onSelect?.('range', entryKey)
    else if (e.metaKey || e.ctrlKey) onSelect?.('toggle', entryKey)
    else onOpen?.(entryKey)
  }
  // A dismissing/snoozing card collapses to a red line with a gap sweeping right to left; the cache is untouched
  // until the call resolves, so a failure brings the card back with the error inline (never a toast).
  if (pending) {
    return (
      <li {...li} data-entry-key={entryKey} className={cn('group border-b border-border-subtle', className)}>
        <div className="px-3 py-2.5">
          <div className="inbox-dismiss-line" role="status" aria-label="Dismissing" />
        </div>
      </li>
    )
  }
  return (
    <li {...li} data-entry-key={entryKey} data-focused={focused || undefined} aria-current={active || undefined} className={cn('group relative border-b border-border-subtle', className)}>
      {/* Swipe right = dismiss, swipe left = snooze an hour; the underlay names the action in its colour. */}
      {swipe.dx !== 0 && (
        <div aria-hidden className={cn('absolute inset-0 flex items-center px-4', swipe.dx > 0 ? 'justify-start bg-danger-bg text-danger' : 'justify-end bg-warning-bg text-warning')}>
          {swipe.dx > 0 ? <EyeOff className="size-4" /> : <Clock className="size-4" />}
        </div>
      )}
      {/* Mouse only: the list's own key map (J/K, Enter, X) opens and selects the focused card. */}
      <div
        onClick={click}
        {...swipe.handlers}
        style={{ transform: swipe.dx !== 0 ? `translateX(${swipe.dx}px)` : undefined, transition: swipe.dx === 0 ? 'transform 150ms ease-out' : undefined, touchAction: 'pan-y' }}
        className={cn(
          'relative flex cursor-default gap-2.5 bg-background px-3 py-2.5 text-[13px] motion-safe:transition-colors hover:bg-layer-2/60',
          selected && 'bg-fg-accent/10 hover:bg-fg-accent/15',
          active && 'bg-accent hover:bg-accent',
          focused && 'shadow-[inset_3px_0_0_var(--color-fg-accent)]',
        )}
      >
        {onSelect && (
          <button
            type="button"
            role="checkbox"
            aria-checked={selected === 'some' ? 'mixed' : selected}
            aria-label={`Select ${item.title}`}
            onClick={(e) => onSelect(e.shiftKey ? 'range' : 'toggle', entryKey)}
            className={cn(
              'mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-[4px] border motion-safe:transition-opacity',
              selected ? 'border-primary bg-primary text-primary-foreground' : 'border-border-strong bg-background',
              !selected && !selecting && 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100',
            )}
          >
            {selected === true && <Check className="size-3" />}
            {selected === 'some' && <Minus className="size-3" />}
          </button>
        )}
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex min-w-0 items-center gap-1">
            <span className={cn('shrink-0 rounded-full px-1.5 py-px text-[10px] font-medium uppercase tracking-wide', troubled(item) ? 'bg-danger-bg text-danger' : 'bg-layer-2 text-fg-secondary')}>{chipLabel(item)}</span>
            {stack !== undefined && <span className="shrink-0 text-[11px] font-medium tabular-nums text-fg-secondary">×{stack}</span>}
            <PriorityChip priority={item.priority} />
            {/* 155 §7.5: done ≠ idle — the engine keeps `alert` on a finished item until this connection has looked at the session. */}
            {item.alert?.class === 'finished' && <span className="shrink-0 rounded-full border border-success-border px-1.5 text-[10px] uppercase tracking-wide text-success">done</span>}
            <DoubleChip ids={doubleIds(item)} />
            <span className="ml-auto shrink-0 pl-2 text-[11px] text-fg-tertiary tabular-nums">{age(item.at_ms, now)}</span>
          </div>
          <Link to="/attention/$id" params={{ id: item.id }} search={true} className="line-clamp-2 font-medium leading-snug hover:underline">
            {item.title}
          </Link>
          {detail}
          {from && <p className="truncate text-xs text-fg-tertiary">{from}</p>}
          {reason && <p className="text-xs text-fg-secondary tabular-nums">{reason}</p>}
          {(children || graph) && (
            <div className="flex min-w-0 flex-wrap items-center gap-1 pt-0.5">
              {children}
              {graph}
            </div>
          )}
          <ItemError ids={errorIds ?? [item.id]} />
        </div>
      </div>
    </li>
  )
}

/** One line of the card's facts: the approval's command in mono, a web page's host, a stopped session's summary and error, else `detail`. */
function Detail({ item }: { item: AttentionItem }) {
  if (item.kind === 'approval') return item.detail ? <p className="truncate font-mono text-xs text-fg-secondary">{item.detail}</p> : null
  if (item.kind === 'web_view') return item.detail ? <p className="truncate font-mono text-xs text-fg-secondary" title={item.detail}>{urlHost(item.detail)}</p> : null
  if (item.kind === 'finished') {
    const summary = item.summary ?? item.detail
    const error = item.stop?.error
    return (
      <>
        {summary && <p className="line-clamp-2 text-xs text-fg-secondary">{summary}</p>}
        {error && error !== summary && <p className={cn('truncate text-xs', troubled(item) ? 'text-danger' : 'text-fg-secondary')}>{error}</p>}
      </>
    )
  }
  // A double's detail is its grace instant (ISO), not words.
  if (item.kind === 'double' || !item.detail) return null
  return <p className="truncate text-xs text-fg-secondary">{item.detail}</p>
}

/**
 * 79 §6.6: the secret form inline — the replicated row (`secret_requests_list`) on every engine, the VM it was raised on
 * as its origin; the local `SecretRequested` fills in until the poll lands.
 */
export function SecretInline({ item }: { item: AttentionItem }) {
  const qc = useQueryClient()
  const sandboxes = useSandboxes()
  const id = item.id.replace(/^secret:/, '')
  const rows = useSecretRequestRows()
  const row = rows.data?.find((r) => r.id === id)
  const local = useSecretRequests((s) => s.pending.find((p) => p.id === id))
  const origin = row ? (sandboxes.data?.find((s) => s.engine === row.engine)?.name ?? item.detail) : item.detail
  const one = row?.fields.length === 1 ? row.fields[0] : undefined
  // The inbox never toasts: a failed answer shows under the item (`ItemError`), a good one clears it.
  const failed = (e: string) => setItemError([item.id], e)
  const answered = () => {
    setItemError([item.id], null)
    void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
    void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
  }
  if (row) return <SecretRequestCard id={row.id} title={row.title} note={one && one.label !== one.name ? one.label : undefined} fields={row.fields} expires={row.expires} origin={origin} destination={row.destination} onAnswered={answered} onError={failed} />
  if (local) return <SecretRequestCard id={local.id} title={local.title} note={local.title ? undefined : local.note} fields={eventFields(local)} expires={local.expires} origin={origin} destination={local.destination} onAnswered={answered} onError={failed} />
  return <p className="text-xs text-fg-tertiary">{rows.isPending ? 'Loading the request…' : `Answer it in the session${item.detail ? ` on ${item.detail}` : ''}.`}</p>
}

/** A single question with options: one button per option (the recommended one filled); the answer goes straight to the agent. */
function AskOptions({ item }: { item: AttentionItem }) {
  const act = useInboxActions()
  const bulkPending = useBulkPending()
  const ask = item.ask
  const q = ask?.questions[0]
  if (!ask || !q) return null
  return q.options.map((o, index) => (
    <Button key={o.label} size="xs" variant={q.recommended === index ? 'default' : 'outline'} disabled={act.ask.isPending || bulkPending} title={o.description ?? undefined} onClick={() => void act.ask.mutateAsync({ id: ask.id, answers: [{ kind: 'choice', index }] }).catch(() => undefined)}>
      {o.label}
    </Button>
  ))
}

/**
 * 207 review 14: cards re-render only when what they show changed — the list ticks every second while something counts
 * down, and the page hands a coarse clock to cards without a countdown. `style` is compared by its transform (a new
 * object each render), a stack's `items` member by member.
 */
function sameProps<P extends object>(prev: P, next: P): boolean {
  const a = prev as Record<string, unknown>
  const b = next as Record<string, unknown>
  const keys = new Set([...Object.keys(a), ...Object.keys(b)])
  for (const k of keys) {
    if (a[k] === b[k]) continue
    if (k === 'style') {
      if ((a[k] as { transform?: string } | undefined)?.transform !== (b[k] as { transform?: string } | undefined)?.transform) return false
    } else if (k === 'items') {
      const x = a[k] as unknown[]
      const y = b[k] as unknown[]
      if (x.length !== y.length || x.some((v, i) => v !== y[i])) return false
    } else return false
  }
  return true
}

export type CardProps = Omit<FrameProps, 'detail' | 'children' | 'stack'> & {
  /** The secret form starts open (tests; a deep link). */
  open?: boolean
}

/** One item's card. */
export const Card = memo(function Card({ open = false, ...props }: CardProps) {
  const { item } = props
  const [expanded, setExpanded] = useState(open)
  let actions: ReactNode
  if (item.kind === 'finished') {
    actions = (
      <>
        <Reply item={item} />
        <SnoozeDismiss item={item} size="xs" />
      </>
    )
  } else if (item.kind === 'secret') {
    actions = (
      <>
        <Button size="xs" variant={expanded ? 'secondary' : 'default'} aria-expanded={expanded} onClick={() => setExpanded((x) => !x)}>
          Enter secret…
        </Button>
        <SnoozeDismiss item={item} size="xs" />
        {expanded && (
          <div className="basis-full">
            <SecretInline item={item} />
          </div>
        )}
      </>
    )
  } else if (item.kind === 'ask' && !needsPane(item)) {
    actions = (
      <>
        <AskOptions item={item} />
        <SnoozeDismiss item={item} size="xs" />
      </>
    )
  } else if (needsPane(item)) {
    actions = (
      <>
        <Button asChild size="xs" variant="outline">
          <Link to="/attention/$id" params={{ id: item.id }} search={true}>
            Open
          </Link>
        </Button>
        <SnoozeDismiss item={item} size="xs" />
      </>
    )
  } else {
    actions = <KindActions item={item} size="xs" />
  }
  return (
    <Frame {...props} detail={<Detail item={item} />}>
      {actions}
    </Frame>
  )
}, sameProps)

export type StackCardProps = Omit<FrameProps, 'item' | 'detail' | 'children' | 'stack'> & {
  items: AttentionItem[]
  /** The member list is shown (the page keeps it, so the keyboard can open it); uncontrolled when absent. */
  expanded?: boolean
  onExpand?: (key: string, open: boolean) => void
}

/**
 * Approvals of one session and tool, collapsed (207 §6): the head's rows with the stack's highest risk, Allow all N /
 * Deny all N, and an expander listing every member with its own Allow / Deny. Snooze and Dismiss act on every member.
 */
export const StackCard = memo(function StackCard({ items, expanded, onExpand, ...props }: StackCardProps) {
  const head = { ...(items[0] as AttentionItem), risk: stackRisk(items) }
  const bulk = useAttentionBulk()
  const act = useInboxActions()
  const [local, setLocal] = useState(false)
  const open = expanded ?? local
  const key = props.entryKey ?? head.id
  const toggle = () => (onExpand ? onExpand(key, !open) : setLocal(!open))
  const ids = items.map((i) => i.id)
  const approvals = items.flatMap((i) => (i.approval ? [i.approval] : []))
  const busy = bulk.pending || act.approve.isPending
  return (
    <Frame {...props} item={head} stack={items.length} errorIds={ids} detail={<Detail item={head} />}>
      <Button size="xs" disabled={busy} onClick={() => void bulk.resolve(approvals, 'allow')}>
        Allow all {items.length}
      </Button>
      <Button size="xs" variant="outline" disabled={busy} onClick={() => void bulk.resolve(approvals, 'deny')}>
        Deny all {items.length}
      </Button>
      <Button size="xs" variant="ghost" aria-expanded={open} onClick={toggle}>
        {open ? 'Hide' : `Show ${items.length}`}
        <ChevronDown className={cn('motion-safe:transition-transform', open && 'rotate-180')} />
      </Button>
      <SnoozeDismiss item={head} ids={ids} size="xs" />
      {open && (
        <ul className="basis-full divide-y divide-border-subtle rounded-md border border-border-subtle">
          {items.map((m) => (
            <li key={m.id} className="flex min-w-0 items-center gap-2 px-2 py-1.5">
              <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg-secondary">{m.detail ?? m.title}</span>
              {m.risk === 'destructive' && <span className="shrink-0 text-[10px] uppercase tracking-wide text-danger">destructive</span>}
              {m.approval && (
                <>
                  <Button size="xs" disabled={busy} onClick={() => void act.approve.mutateAsync({ id: m.approval as string, allow: true }).catch(() => undefined)}>
                    Allow
                  </Button>
                  <Button size="xs" variant="outline" disabled={busy} onClick={() => void act.approve.mutateAsync({ id: m.approval as string, allow: false }).catch(() => undefined)}>
                    Deny
                  </Button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </Frame>
  )
}, sameProps)
