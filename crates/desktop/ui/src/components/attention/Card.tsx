// 207 §8.5: one card of the flat Attention list — what it is, where it came from, why it sits here, and the answer
// itself (Allow / Deny, a question's options, Enter secret, a reply to a stopped session) without opening the item.
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Check, ChevronDown } from 'lucide-react'
import { type ComponentProps, type MouseEvent, type ReactNode, useState } from 'react'
import { DoubleChip } from '@/components/chat/DoubleChip'
import { SecretRequestCard } from '@/components/chat/SecretRequestCard'
import { ATTENTION_KEY, SECRET_REQUESTS_KEY, useAttentionActions, useSecretRequestRows } from '@/components/hud/queries'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { age } from '@/components/tasks/helpers'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { useSecretRequests } from '@/store/approvals'
import { KindActions, SnoozeDismiss } from './actions'
import { chipLabel, doubleIds } from './labels'
import { pathText, reasonLine } from './model'
import { useAttentionBulk } from './queries'
import { Reply } from './Reply'

/** 132 D1/D4: `urgent` and `high` show a chip after the kind chip (gold-ochre · the warning hue); medium/low show nothing. */
export function PriorityChip({ priority }: { priority: AttentionItem['priority'] | undefined }) {
  if (priority !== 'urgent' && priority !== 'high') return null
  return <span className={cn('w-fit shrink-0 rounded-full border px-1.5 text-[10px] font-medium uppercase tracking-wide', priority === 'urgent' ? 'border-fg-accent text-fg-accent' : 'border-warning text-warning')}>{priority}</span>
}

/** A stopped session whose turn failed or timed out reads as a problem; a clean finish or a cancel does not. */
const troubled = (item: AttentionItem) => item.stop?.outcome === 'failed' || item.stop?.outcome === 'timed_out'

/** Kinds whose answer needs the pane (a packet, an artifact, several questions): the card links there. */
function needsPane(item: AttentionItem): boolean {
  if (item.kind === 'decision' || item.kind === 'review' || item.kind === 'show') return true
  if (item.kind !== 'ask') return false
  const qs = item.ask?.questions ?? []
  return qs.length !== 1 || (qs[0]?.options.length ?? 0) === 0
}

export type SelectHandler = (mode: 'toggle' | 'range') => void

type FrameProps = {
  item: AttentionItem
  now: number
  stack?: number
  active?: boolean
  focused?: boolean
  selected?: boolean
  selecting?: boolean
  onSelect?: SelectHandler
  onOpen?: () => void
  detail?: ReactNode
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
function Frame({ item, now, stack, active, focused, selected, selecting, onSelect, onOpen, detail, children, className, ...li }: FrameProps) {
  const path = pathText(item)
  const agent = item.agent && !said([...(item.path ?? []), item.project ?? ''], item.agent) ? item.agent : null
  const from = [path, agent].filter(Boolean).join(' · ')
  const reason = reasonLine(item, now)
  const click = (e: MouseEvent<HTMLDivElement>) => {
    if ((e.target as Element).closest(OWN_CLICK)) return
    if (e.shiftKey) onSelect?.('range')
    else if (e.metaKey || e.ctrlKey) onSelect?.('toggle')
    else onOpen?.()
  }
  return (
    <li {...li} aria-selected={focused || undefined} className={cn('group border-b border-border-subtle', className)}>
      {/* Mouse only: the list's own key map (J/K, Enter, X) opens and selects the focused card. */}
      <div
        onClick={click}
        className={cn(
          'flex cursor-default gap-2.5 px-3 py-2.5 text-[13px] motion-safe:transition-colors hover:bg-layer-2/60',
          selected && 'bg-fg-accent/10 hover:bg-fg-accent/15',
          active && 'bg-accent hover:bg-accent',
          focused && 'shadow-[inset_3px_0_0_var(--color-fg-accent)]',
        )}
      >
        {onSelect && (
          <button
            type="button"
            role="checkbox"
            aria-checked={selected ?? false}
            aria-label={`Select ${item.title}`}
            onClick={(e) => onSelect(e.shiftKey ? 'range' : 'toggle')}
            className={cn(
              'mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-[4px] border motion-safe:transition-opacity',
              selected ? 'border-primary bg-primary text-primary-foreground' : 'border-border-strong bg-background',
              !selected && !selecting && 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100',
            )}
          >
            {selected && <Check className="size-3" />}
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
          {children && <div className="flex min-w-0 flex-wrap items-center gap-1 pt-0.5">{children}</div>}
        </div>
      </div>
    </li>
  )
}

/** One line of the card's facts: the approval's command in mono, a stopped session's summary and error, else `detail`. */
function Detail({ item }: { item: AttentionItem }) {
  if (item.kind === 'approval') return item.detail ? <p className="truncate font-mono text-xs text-fg-secondary">{item.detail}</p> : null
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
  const answered = () => {
    void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
    void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
  }
  if (row) return <SecretRequestCard id={row.id} title={row.title} note={one && one.label !== one.name ? one.label : undefined} fields={row.fields} origin={origin} onAnswered={answered} />
  if (local) return <SecretRequestCard id={local.id} note={local.note} fields={[{ name: local.name, label: local.name, masked: true, current: local.current }]} origin={origin} onAnswered={answered} />
  return <p className="text-xs text-fg-tertiary">{rows.isPending ? 'Loading the request…' : `Answer it in the session${item.detail ? ` on ${item.detail}` : ''}.`}</p>
}

/** A single question with options: one button per option (the recommended one filled); the answer goes straight to the agent. */
function AskOptions({ item }: { item: AttentionItem }) {
  const act = useAttentionActions()
  const ask = item.ask
  const q = ask?.questions[0]
  if (!ask || !q) return null
  return q.options.map((o, index) => (
    <Button key={o.label} size="xs" variant={q.recommended === index ? 'default' : 'outline'} disabled={act.ask.isPending} title={o.description ?? undefined} onClick={() => void act.ask.mutateAsync({ id: ask.id, answers: [{ kind: 'choice', index }] }).catch(() => undefined)}>
      {o.label}
    </Button>
  ))
}

export type CardProps = Omit<FrameProps, 'detail' | 'children' | 'stack'> & {
  /** The secret form starts open (tests; a deep link). */
  open?: boolean
}

/** One item's card. */
export function Card({ open = false, ...props }: CardProps) {
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
}

export type StackCardProps = Omit<FrameProps, 'item' | 'detail' | 'children' | 'stack'> & { items: AttentionItem[] }

/**
 * Approvals of one session and tool, collapsed (207 §6): the head's rows, Allow all N / Deny all N, and an expander
 * listing every member with its own Allow / Deny. Snooze and Dismiss act on every member.
 */
export function StackCard({ items, ...props }: StackCardProps) {
  const head = items[0] as AttentionItem
  const bulk = useAttentionBulk()
  const act = useAttentionActions()
  const [open, setOpen] = useState(false)
  const ids = items.map((i) => i.id)
  const approvals = items.flatMap((i) => (i.approval ? [i.approval] : []))
  return (
    <Frame {...props} item={head} stack={items.length} detail={<Detail item={head} />}>
      <Button size="xs" disabled={bulk.pending} onClick={() => void bulk.resolve(approvals, 'allow')}>
        Allow all {items.length}
      </Button>
      <Button size="xs" variant="outline" disabled={bulk.pending} onClick={() => void bulk.resolve(approvals, 'deny')}>
        Deny all {items.length}
      </Button>
      <Button size="xs" variant="ghost" aria-expanded={open} onClick={() => setOpen((x) => !x)}>
        {open ? 'Hide' : `Show ${items.length}`}
        <ChevronDown className={cn('motion-safe:transition-transform', open && 'rotate-180')} />
      </Button>
      <SnoozeDismiss item={head} ids={ids} size="xs" />
      {open && (
        <ul className="basis-full divide-y divide-border-subtle rounded-md border border-border-subtle">
          {items.map((m) => (
            <li key={m.id} className="flex min-w-0 items-center gap-2 px-2 py-1.5">
              <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg-secondary">{m.detail ?? m.title}</span>
              {m.approval && (
                <>
                  <Button size="xs" disabled={act.approve.isPending} onClick={() => void act.approve.mutateAsync({ id: m.approval as string, allow: true }).catch(() => undefined)}>
                    Allow
                  </Button>
                  <Button size="xs" variant="outline" disabled={act.approve.isPending} onClick={() => void act.approve.mutateAsync({ id: m.approval as string, allow: false }).catch(() => undefined)}>
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
}
