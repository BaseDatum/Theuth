import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Shield, ShieldCheck, ShieldOff } from 'lucide-react'
import { useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useMesh } from '@/components/tasks/queries'
import { approvalsOff } from '@/lib/header'
import { costTitle, costTotal, moneyParts } from '@/lib/money'
import { engineLabel, ownerUnreachable, routed, useOwnerOffline } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import { RpcError } from '@/rpc/client'
import type { CostBucket } from '@/rpc/bindings/method.agent.output'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { type ApprovalLevel, type ApprovalsChoice, approvalsChoice, setApprovalsLevel } from './approvals-level'
import { band, useContextReading } from './ContextMeter'
import { useCost } from './CostPill'

/**
 * The composer strip's cost: one ballpark figure, `~` always (it is a glance, the dialog has the exact sum). Whole units
 * from 1 up (`1.62` → `~$2`, `44.09` → `~$44`), cents only below (`~$0.38`). `null`: nothing priced, or mixed currencies
 * (they do not add up; the strip shows nothing and the dialog keeps the split).
 */
export function costBallpark(value: number, prefix = '$'): string {
  return value >= 1 ? `~${prefix}${Math.round(value)}` : `~${prefix}${value.toFixed(2)}`
}

/** `costBallpark` of a `session_cost` bucket, with the exact total for the tooltip. */
export function costGlance(bucket: CostBucket): { short: string; exact: string } | null {
  const t = costTotal(bucket)
  if (!t) return null
  const { prefix, value } = moneyParts(t.money)
  const exact = `${t.approx ? '~' : ''}${prefix}${value.toFixed(2)}`
  return { short: costBallpark(value, prefix), exact }
}

/** Context % tone: muted, amber from 75 %, red from 90 % (the meter's bands). */
const PCT_TONE = { normal: '', warning: 'text-warning', danger: 'text-danger', critical: 'text-danger' } as const

const ktok = (n: number) => `${Math.round(n / 1000)}k`

/** What the composer's top row shows: the context reading (percent and its tooltip) and the cost glance. */
export type MetaData = {
  pct: number | null
  estimated: boolean
  ctxTitle?: string
  cost: { short: string; exact: string; title: string } | null
}

/**
 * The composer's `13%  ~$44` for `session`, or `null` when there is none to show (no session: the new-chat draft; no
 * context reading and nothing priced). The percent is the context meter's own reading (`useContextReading`), the cost
 * the breakdown dialog's `session_cost` query (`useCost`).
 */
export function useComposerMeta(session: string | null | undefined): MetaData | null {
  const canSessions = useQuerySupport(['list_sessions'])
  const api = useApi()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canSessions && !!session })
  const memory = sessions.data?.find((s) => s.id === session)?.memory
  const reading = useContextReading(session ?? '', memory)
  const cost = useCost(session ?? '')
  if (!session) return null
  const glance = cost ? costGlance(cost.total) : null
  const pct = reading?.pct ?? null
  if (pct === null && !glance) return null
  return {
    pct,
    estimated: !!reading?.estimated,
    ctxTitle: reading
      ? [`${reading.estimated ? '≈ ' : ''}${ktok(reading.used)} / ${ktok(reading.limit)} tokens`, reading.cache === null ? null : `Cache ${reading.cache}%`].filter(Boolean).join('\n')
      : undefined,
    cost: glance && cost ? { ...glance, title: [glance.exact, costTitle(cost.total)].filter(Boolean).join('\n') } : null,
  }
}

/**
 * The thin row inside the top of the composer box, above the text: `13%  ~$44`, right-aligned, subdued, borderless,
 * both always shown. The percent's tooltip carries the tokens and the cache hit rate; the cost's the exact total, and a
 * click opens the breakdown (`CostDialog`, `/cost`). `hidden` (a voice note recording): the row keeps its height,
 * empty, so the box does not jump.
 */
export function ComposerMeta({ session, meta, hidden = false }: { session: string; meta: MetaData; hidden?: boolean }) {
  return (
    <div data-composer-meta="" aria-hidden={hidden || undefined} className={cn('flex h-4 items-center justify-end gap-2 px-1 text-[11px] leading-4 text-fg-tertiary tabular-nums', hidden && 'invisible')}>
      {meta.pct !== null && (
        <span data-meta="context" className={cn('cursor-default', PCT_TONE[band(meta.pct)])} title={meta.ctxTitle}>
          {meta.estimated && '≈'}
          {meta.pct}%
        </span>
      )}
      {meta.cost && (
        <button
          type="button"
          data-meta="cost"
          tabIndex={hidden ? -1 : undefined}
          className="rounded-sm outline-none hover:text-fg-secondary focus-visible:ring-2 focus-visible:ring-ring/50"
          title={meta.cost.title}
          aria-label={`Cost ${meta.cost.exact}`}
          onClick={() => window.dispatchEvent(new CustomEvent('theuth:open-cost', { detail: { session } }))}
        >
          {meta.cost.short}
        </button>
      )}
    </div>
  )
}

export type ShieldState = 'on' | 'off' | 'waiting'

/** The shield's state: a pending approval of this chat outranks the level; else off or on (asks). */
export function shieldState(waiting: number, off: boolean): ShieldState {
  return waiting > 0 ? 'waiting' : off ? 'off' : 'on'
}

const SHIELD = {
  on: { icon: ShieldCheck, tone: 'text-fg-tertiary', label: 'Approvals on: asks before risky tools' },
  off: { icon: ShieldOff, tone: 'text-warning', label: 'Approvals off: runs freely' },
  waiting: { icon: Shield, tone: 'text-danger motion-safe:animate-pulse', label: 'Waiting on your approval' },
} as const

/**
 * The shield and its popover, without data: the state's icon (muted shield-check, amber struck shield, filled red and
 * pulsing while an approval waits), and the levels the engine has (`approvalsChoice`). `waiting` puts a jump row first.
 */
export function ApprovalShieldView({
  state,
  choice,
  waiting,
  onPick,
  onJump,
  open,
  onOpenChange,
}: {
  state: ShieldState
  choice: ApprovalsChoice
  waiting: number
  onPick(level: ApprovalLevel): void
  onJump(): void
  open?: boolean
  onOpenChange?(open: boolean): void
}) {
  const { icon: Icon, tone, label } = SHIELD[state]
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-shield={state}
          aria-label={label}
          title={label}
          className={cn('grid size-9 shrink-0 place-items-center rounded-lg outline-none hover:bg-foreground/6 focus-visible:ring-2 focus-visible:ring-ring/50', tone)}
        >
          <Icon className="size-4" fill={state === 'waiting' ? 'currentColor' : 'none'} />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" side="top" className="w-64 p-1 text-[13px]">
        {waiting > 0 && (
          <button type="button" data-level="jump" className="flex w-full items-center rounded-md px-2 py-1.5 text-left text-danger hover:bg-foreground/6" onClick={onJump}>
            {waiting} waiting: jump to it
          </button>
        )}
        <div className="px-2 pt-1.5 pb-1 text-[11px] text-fg-tertiary">Approvals for this chat{choice.note ? ` · ${choice.note}` : ''}</div>
        <div role="radiogroup" aria-label="Approvals for this chat">
          {choice.options.map((o) => (
            <button
              key={o.value}
              type="button"
              role="radio"
              aria-checked={choice.value === o.value}
              data-level={o.value}
              disabled={choice.disabled}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-foreground/6 disabled:pointer-events-none disabled:opacity-50"
              onClick={() => onPick(o.value)}
            >
              <Check className={cn('size-3.5 shrink-0', choice.value !== o.value && 'invisible')} />
              <span className="min-w-0 flex-1 truncate">{o.long}</span>
            </button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )
}

/** Scroll the transcript to the approval card `id` (`data-prompt-card`, set by `ApprovalCard`). */
export function jumpToPrompt(id: string): boolean {
  const el = document.querySelector(`[data-prompt-card="${CSS.escape(id)}"]`)
  if (!el) return false
  const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  el.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' })
  return true
}

/**
 * Left of the composer bar, after the MCP button: this chat's approval level at a glance. Reads `get_approvals`
 * through the owner like the ⋯ menu (same query prefix, so either setter refreshes both) and sets it with the same RPC
 * (`setApprovalsLevel`, routed through the owner).
 */
export function ApprovalShield({ session }: { session: string }) {
  const canApprovals = useQuerySupport(['get_approvals'])
  const canSessions = useQuerySupport(['list_sessions'])
  const api = useApi()
  const queryClient = useQueryClient()
  const info = useConnection((s) => s.info)
  const [open, setOpen] = useState(false)
  const effective = useQuery({ queryKey: ['approvals', session, info?.approvals?.global], queryFn: () => api.get_approvals(session), enabled: canApprovals, retry: false })
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canSessions })
  const row = sessions.data?.find((s) => s.id === session)
  const mesh = useMesh()
  const parked = useOwnerOffline((s) => s.by[session] !== undefined)
  const waitingIds = useApprovals((s) => s.pending.filter((p) => p.session === session).map((p) => p.id).join('\n'))
  const waiting = waitingIds ? waitingIds.split('\n') : []
  const owner = row && !row.here ? engineLabel(row.owner, mesh.data?.peers) : undefined
  const choice = approvalsChoice({
    owner,
    offline: parked || mesh.data?.peers.find((p) => p.id === row?.owner)?.reachable === false || !!ownerUnreachable(effective.error),
    oldOwner: owner !== undefined && effective.error instanceof RpcError && effective.error.code === -32601,
    approvals: effective.data,
  })
  const state = shieldState(waiting.length, approvalsOff(effective.data, info?.approvals ?? null))
  const pick = (level: ApprovalLevel) => {
    setOpen(false)
    void routed(session, () => setApprovalsLevel(api, session, level, effective.data?.global))
      .then(() => queryClient.invalidateQueries({ queryKey: ['approvals', session] }))
      .catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  }
  const jump = () => {
    setOpen(false)
    if (waiting[0] && !jumpToPrompt(waiting[0])) toast.message('The approval card is not in this view')
  }
  return <ApprovalShieldView state={state} choice={choice} waiting={waiting.length} onPick={pick} onJump={jump} open={open} onOpenChange={setOpen} />
}
