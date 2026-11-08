import { THEUTH } from '@/components/tasks/helpers'
import type { RemoteAgent } from '@/components/tasks/home'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import type { AgentStatus } from '@/rpc/bindings/AgentStatus'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { useAgentActions } from './queries'
import { budgetUsed } from './tree'

/** Paperclip derives a palette from a hash of the id; we hash the name into a hue so a card keeps its colour across nodes. */
export function hue(name: string): number {
  let h = 0
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360
  return h
}

/** First + last initial ("Chief of staff" → CS), else the first two characters. */
export function initials(title: string): string {
  const words = title.trim().split(/\s+/).filter(Boolean)
  const first = words[0]
  const last = words[words.length - 1]
  if (!first || !last) return '?'
  return (words.length === 1 ? first.slice(0, 2) : first[0]! + last[0]!).toUpperCase()
}

export function AgentAvatar({ name, title, size = 36, className }: { name: string; title: string; size?: number; className?: string }) {
  const root = name === THEUTH
  return (
    <span
      aria-hidden
      className={cn('inline-flex shrink-0 items-center justify-center rounded-full font-semibold select-none', root ? 'bg-primary text-primary-foreground' : 'text-white', className)}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.38), background: root ? undefined : `oklch(0.62 0.14 ${hue(name)})` }}
    >
      {root ? 'θ' : initials(title || name)}
    </span>
  )
}

/**
 * Sky's ruling: an agent is ACTIVE or PAUSED (the switch, `agent_pause`); what it is doing right now — working, idle,
 * blocked, unreachable — is an activity chip beneath, not a peer state. `proposed` lives only in the Hiring inbox.
 */
export const ACTIVITY: Partial<Record<AgentStatus, { label: string; dot: string; chip: string }>> = {
  idle: { label: 'idle', dot: 'bg-fg-tertiary/50', chip: 'border-border text-fg-secondary' },
  working: { label: 'working', dot: 'bg-success', chip: 'border-success/40 text-success' },
  blocked: { label: 'blocked', dot: 'bg-warning', chip: 'border-warning/40 text-warning' },
  unreachable: { label: 'unreachable', dot: 'bg-danger', chip: 'border-danger/40 text-danger' },
  invalid: { label: 'invalid file', dot: 'bg-danger', chip: 'border-danger/40 text-danger' },
}

/** 182: another machine's agent has only its file's word: idle, paused, proposed — or `offline` when its machine is. Same chip as the local activity. */
const REMOTE: Record<RemoteAgent['status'], { dot: string; chip: string }> = {
  idle: { dot: 'bg-fg-tertiary/50', chip: 'border-border text-fg-secondary' },
  paused: { dot: 'bg-warning', chip: 'border-warning/40 text-warning' },
  proposed: { dot: 'bg-info', chip: 'border-info/40 text-info' },
  offline: { dot: 'bg-fg-tertiary', chip: 'border-border text-fg-tertiary' },
}
export function RemoteChip({ status, className }: { status: RemoteAgent['status']; className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium', REMOTE[status].chip, className)}>
      <span className={cn('size-1.5 shrink-0 rounded-full', REMOTE[status].dot)} aria-hidden />
      {status}
    </span>
  )
}

/** The dot on an avatar: activity colour, grey when paused. */
export function statusDot(status: AgentStatus): string {
  return ACTIVITY[status]?.dot ?? 'bg-fg-tertiary'
}

/** 178: the children the desk's current wake waits on, as the chip says it ("waiting on scout +2"); `null` when none. */
export function waitingLabel(waiting: readonly string[] | undefined): string | null {
  const [first, ...rest] = waiting ?? []
  if (!first) return null
  return rest.length ? `waiting on ${first} +${rest.length}` : `waiting on ${first}`
}

export function ActivityChip({ status, waiting, className }: { status: AgentStatus; waiting?: readonly string[]; className?: string }) {
  const s = ACTIVITY[status]
  if (!s) return null
  const label = waitingLabel(waiting) ?? s.label
  return (
    <span
      className={cn('inline-flex min-w-0 max-w-56 items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium', s.chip, className)}
      title={waiting?.length ? waiting.join(', ') : undefined}
    >
      <span className={cn('size-1.5 shrink-0 rounded-full', s.dot)} aria-hidden />
      <span className="truncate">{label}</span>
    </span>
  )
}

/** 179: what a paused agent holds, as the chip says it ("Paused · 2 wakes held · suspended mid-turn"). */
export function pausedLabel(a: Pick<AgentSummary, 'held_wakes' | 'suspended'>): string {
  const n = a.held_wakes ?? 0
  const held = `Paused · ${n} wake${n === 1 ? '' : 's'} held`
  return a.suspended ? `${held} · suspended mid-turn` : held
}

/** 179: the chip of a paused agent, in place of its activity. */
export function PausedChip({ agent: a, className }: { agent: Pick<AgentSummary, 'held_wakes' | 'suspended'>; className?: string }) {
  return (
    <span
      className={cn('inline-flex min-w-0 max-w-72 items-center gap-1.5 rounded-full border border-border px-2 py-0.5 text-xs font-medium text-fg-tertiary', className)}
      title={a.suspended ? 'Its turn is suspended where it stopped; switching it back on resumes it.' : undefined}
    >
      <span className="size-1.5 shrink-0 rounded-full bg-fg-tertiary" aria-hidden />
      <span className="truncate">{pausedLabel(a)}</span>
    </span>
  )
}

/** Active ⇄ Paused, on the card and in the detail header. `theuth` is always active and has no switch. */
export function ActiveSwitch({ agent: a, withLabel = true, className }: { agent: AgentSummary; withLabel?: boolean; className?: string }) {
  const { pause } = useAgentActions()
  if (a.file.name === THEUTH) return null
  const paused = a.file.paused
  return (
    <span
      className={cn('inline-flex items-center gap-1.5 text-xs', paused ? 'text-fg-tertiary' : 'text-fg-secondary', className)}
      // Lives inside a card Link: the Switch toggles first (target), then this stops the link from navigating.
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
      }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <Switch
        size="sm"
        checked={!paused}
        disabled={pause.isPending}
        aria-label={paused ? 'Paused — switch on to activate' : 'Active — switch off to pause'}
        onCheckedChange={(on) => pause.mutate({ name: a.file.name, paused: !on })}
      />
      {withLabel && (paused ? 'Paused' : 'Active')}
    </span>
  )
}

/** Spend this month against the cap; nothing for `budget: none`. */
export function BudgetBar({ agent: a, withText = true, className }: { agent: AgentSummary; withText?: boolean; className?: string }) {
  const used = budgetUsed(a)
  if (used === null || a.file.budget.type !== 'monthly') return null
  const b = a.file.budget
  return (
    <span className={cn('flex min-w-0 items-center gap-1.5 text-xs text-fg-secondary', className)} title={`${a.spent_month} / ${b.amount} ${b.currency} this month`}>
      <span className="h-1 flex-1 overflow-hidden rounded-full bg-layer-3">
        <span className={cn('block h-full', used >= 1 ? 'bg-danger' : used >= 0.8 ? 'bg-warning' : 'bg-success')} style={{ width: `${used * 100}%` }} />
      </span>
      {withText && <span className="tabular-nums">{Math.round(used * 100)}%</span>}
    </span>
  )
}
