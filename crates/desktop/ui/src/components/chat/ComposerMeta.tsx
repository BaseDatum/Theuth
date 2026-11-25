import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Images, Scissors, ScrollText, Shield, ShieldCheck, ShieldOff } from 'lucide-react'
import { useEffect, useState } from 'react'
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
import type { CompactStyle, CostBucket } from '@/rpc/bindings/method.agent.output'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { type ApprovalLevel, type ApprovalsChoice, approvalsChoice, setApprovalsLevel } from './approvals-level'
import { alwaysAskCount, useAlwaysAsk } from '@/lib/always-ask'
import { useToolsSheet } from './tools-sheet'
import { type ContextReading, useContextReading } from './ContextMeter'
import { compactWith, contextCommand } from './slash'
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

const ktok = (n: number) => `${Math.round(n / 1000)}k`

/** What the composer's top row shows: the cost glance (the context reading lives on the context button). */
export type MetaData = {
  cost: { short: string; exact: string; title: string }
}

/** The context meter's reading of `session` (its memory mode from `list_sessions`): the context button's ring and popover. */
function useSessionContext(session: string | null | undefined): ContextReading | null {
  const canSessions = useQuerySupport(['list_sessions'])
  const api = useApi()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canSessions && !!session })
  const memory = sessions.data?.find((s) => s.id === session)?.memory
  return useContextReading(session ?? '', memory)
}

/**
 * The composer's `~$44` for `session`, or `null` when there is none to show (no session: the new-chat draft; nothing
 * priced). The cost is the breakdown dialog's `session_cost` query (`useCost`).
 */
export function useComposerMeta(session: string | null | undefined): MetaData | null {
  const cost = useCost(session ?? '')
  if (!session || !cost) return null
  const glance = costGlance(cost.total)
  if (!glance) return null
  return { cost: { ...glance, title: [glance.exact, costTitle(cost.total)].filter(Boolean).join('\n') } }
}

/**
 * The thin row inside the top of the composer box, above the text: `~$44`, right-aligned, subdued, borderless (the
 * model picker is on the bar, left of talk/mic/send; the context reading is the bar's context button). The cost's
 * tooltip is the exact total, and a click opens the breakdown (`CostDialog`, `/cost`).
 * `hidden` (a voice note recording): the row keeps its height, empty, so the box does not jump.
 */
export function ComposerMeta({ session, meta, hidden = false }: { session: string; meta: MetaData; hidden?: boolean }) {
  return (
    <div data-composer-meta="" aria-hidden={hidden || undefined} className={cn('flex h-3 items-center justify-end gap-2 px-1 text-[11px] leading-3 text-fg-tertiary tabular-nums', hidden && 'invisible')}>
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
  off: { icon: ShieldOff, tone: 'text-danger', label: 'Approvals off: runs freely' },
  waiting: { icon: Shield, tone: 'text-danger motion-safe:animate-pulse', label: 'Waiting on your approval' },
} as const

/**
 * The shield and its popover, without data: the state's icon (muted shield-check, carnelian struck shield, filled carnelian and
 * pulsing while an approval waits), and the levels the engine has (`approvalsChoice`). `waiting` puts a jump row first.
 * 219 §6.1: `alwaysAsk` tools that ask even with approvals off add a line that opens the chat's Tools sheet.
 */
export function ApprovalShieldView({
  state,
  choice,
  waiting,
  alwaysAsk = 0,
  onPick,
  onJump,
  onAlwaysAsk,
  open,
  onOpenChange,
}: {
  state: ShieldState
  choice: ApprovalsChoice
  waiting: number
  alwaysAsk?: number
  onPick(level: ApprovalLevel): void
  onJump(): void
  onAlwaysAsk?(): void
  open?: boolean
  onOpenChange?(open: boolean): void
}) {
  const { icon: Icon, tone, label: plain } = SHIELD[state]
  const label = state === 'off' && alwaysAsk > 0 ? `Approvals off · ${tools(alwaysAsk)} always ask` : plain
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-shield={state}
          aria-label={label}
          title={label}
          className={cn('grid h-9 w-7 shrink-0 place-items-center rounded-lg outline-none hover:bg-foreground/6 focus-visible:ring-2 focus-visible:ring-ring/50', tone)}
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
        {alwaysAsk > 0 && (
          <button type="button" data-level="always-ask" className="flex w-full items-center rounded-md px-2 py-1.5 text-left hover:bg-foreground/6" onClick={onAlwaysAsk}>
            Always ask: {tools(alwaysAsk)}
          </button>
        )}
      </PopoverContent>
    </Popover>
  )
}

const tools = (n: number) => `${n} ${n === 1 ? 'tool' : 'tools'}`

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
  const alwaysAsk = alwaysAskCount(useAlwaysAsk(session))
  const openTools = () => {
    setOpen(false)
    useToolsSheet.getState().show(session)
  }
  return <ApprovalShieldView state={state} choice={choice} waiting={waiting.length} alwaysAsk={alwaysAsk} onPick={pick} onJump={jump} onAlwaysAsk={openTools} open={open} onOpenChange={setOpen} />
}

/** The ring's tone: muted, then warning from 80 % and danger from 95 % of the window. */
export function contextTone(pct: number | null): string {
  return pct === null || pct < 80 ? 'text-fg-tertiary' : pct < 95 ? 'text-warning' : 'text-danger'
}

const RING = 2 * Math.PI * 8

/** A faint full circle with the used fraction drawn over it clockwise from 12 o'clock (`pct`, clamped to 0–100). */
export function ContextRing({ pct, className }: { pct: number | null; className?: string }) {
  const used = Math.min(100, Math.max(0, pct ?? 0)) / 100
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.4} strokeLinecap="round" aria-hidden className={className}>
      <circle cx="12" cy="12" r="8" opacity={0.28} />
      {used > 0 && <circle data-ring-used="" cx="12" cy="12" r="8" strokeDasharray={`${used * RING} ${RING}`} transform="rotate(-90 12 12)" />}
    </svg>
  )
}

type CompactPick = Exclude<CompactStyle, 'clear'>

/** A popover row: one compaction style, the context picker, or clearing the conversation. */
export type ContextAction = { compact: CompactPick } | '/shake' | '/clear'

const COMPACT_STYLES: readonly { style: CompactPick; icon: typeof ScrollText; does: string }[] = [
  { style: 'summary', icon: ScrollText, does: 'fold older turns into a summary' },
  { style: 'cut', icon: Scissors, does: 'drop older turns, keep the recent ones' },
  { style: 'snap', icon: Images, does: 'keep older turns as images' },
]

/** How long a first tap on Clear stays armed before it disarms itself. */
export const CLEAR_CONFIRM_MS = 4000

const ROW = 'flex min-h-9 w-full flex-col items-start justify-center rounded-md px-2 py-1.5 text-left'

/**
 * The popover's rows. Compact carries one icon button per style; Clear arms on the first tap (danger-filled, "Tap again
 * to clear") and only the second within `CLEAR_CONFIRM_MS` clears. Mounted with the popover's content, so closing the
 * popover disarms it too; using another row disarms it.
 */
function ContextActions({ onAction }: { onAction(action: ContextAction): void }) {
  const [hint, setHint] = useState<CompactPick | null>(null)
  const [armed, setArmed] = useState(false)
  useEffect(() => {
    if (!armed) return
    const t = setTimeout(() => setArmed(false), CLEAR_CONFIRM_MS)
    return () => clearTimeout(t)
  }, [armed])
  const run = (action: ContextAction) => {
    setArmed(false)
    onAction(action)
  }
  const hinted = COMPACT_STYLES.find((s) => s.style === hint)
  return (
    <>
      <div data-context-row="compact" className="flex w-full flex-col rounded-md px-2 pt-0.5 pb-1.5">
        <div className="flex items-center gap-0.5">
          <span className="flex-1 font-medium">Compact</span>
          {COMPACT_STYLES.map(({ style, icon: Icon, does }) => (
            <button
              key={style}
              type="button"
              data-compact-style={style}
              aria-label={`Compact: ${style} — ${does}`}
              title={`Compact: ${style} — ${does}`}
              className="grid size-9 shrink-0 place-items-center rounded-md text-fg-secondary outline-none hover:bg-foreground/6 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
              onMouseEnter={() => setHint(style)}
              onMouseLeave={() => setHint(null)}
              onFocus={() => setHint(style)}
              onBlur={() => setHint(null)}
              onClick={() => run({ compact: style })}
            >
              <Icon className="size-4" aria-hidden />
            </button>
          ))}
        </div>
        <span data-compact-caption="" className="truncate text-[11px] text-fg-tertiary">
          {hinted ? `${hinted.style[0]!.toUpperCase()}${hinted.style.slice(1)} — ${hinted.does}` : 'Fold older turns: summary, cut or snap'}
        </span>
      </div>
      <button type="button" data-context-action="/shake" className={cn(ROW, 'hover:bg-foreground/6')} onClick={() => run('/shake')}>
        <span className="font-medium">Shake…</span>
        <span className="text-[11px] text-fg-tertiary">Pick what to drop: tool output, files, images</span>
      </button>
      <button
        type="button"
        data-context-action="/clear"
        data-confirm={armed || undefined}
        className={cn(ROW, armed ? 'bg-danger text-primary-foreground hover:bg-danger/90' : 'hover:bg-foreground/6')}
        onClick={() => (armed ? run('/clear') : setArmed(true))}
      >
        <span className={cn('font-medium', !armed && 'text-danger')}>{armed ? 'Tap again to clear' : 'Clear'}</span>
        <span className={cn('text-[11px]', armed ? 'text-primary-foreground/85' : 'text-fg-tertiary')}>
          {armed ? 'This drops the whole conversation' : "Drop the conversation, keep this chat's settings"}
        </span>
      </button>
    </>
  )
}

/**
 * The context button and its popover, without data: the fill ring (muted, warning from 80 %, danger from 95 %), and
 * a popover with `<used> of <window> · N%`, a thin meter and the ways to free room (compact by style, shake, clear).
 */
export function ContextButtonView({
  reading,
  onAction,
  open,
  onOpenChange,
}: {
  reading: ContextReading | null
  onAction(action: ContextAction): void
  open?: boolean
  onOpenChange?(open: boolean): void
}) {
  const pct = reading?.pct ?? null
  const tone = contextTone(pct)
  const label = pct === null ? 'Context' : `Context ${pct}% used`
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-context-button=""
          aria-label={label}
          title={label}
          className={cn('grid h-9 w-8 shrink-0 place-items-center rounded-lg outline-none hover:bg-foreground/6 focus-visible:ring-2 focus-visible:ring-ring/50 data-[state=open]:bg-foreground/6', tone)}
        >
          <ContextRing pct={pct} className="size-[18px]" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" side="top" className="w-72 p-1 text-[13px]">
        <div className="flex items-baseline justify-between gap-2 px-2 pt-1.5 font-medium">
          <span>Context</span>
          {reading && (
            <span className="truncate text-[11px] font-normal text-fg-tertiary tabular-nums">
              {reading.estimated && '≈ '}
              {ktok(reading.used)}
              {reading.limit > 0 && ` of ${ktok(reading.limit)}`}
              {pct !== null && ` · ${pct}%`}
            </span>
          )}
        </div>
        <div className="mx-2 mt-2 mb-1.5 h-1 overflow-hidden rounded-full bg-foreground/10">
          <div className={cn('h-full rounded-full bg-current', tone === 'text-fg-tertiary' ? 'text-fg-secondary' : tone)} style={{ width: `${Math.min(100, Math.max(0, pct ?? 0))}%` }} />
        </div>
        <ContextActions onAction={onAction} />
      </PopoverContent>
    </Popover>
  )
}

/**
 * Between the MCP button and the shield: how full this chat's context is (`useContextReading`), and a popover running
 * `/compact <style>` (`compactWith`), `/shake` (the context picker) and `/clear` exactly as typed (`contextCommand`).
 */
export function ContextButton({ session }: { session: string }) {
  const api = useApi()
  const reading = useSessionContext(session)
  const [open, setOpen] = useState(false)
  const act = (action: ContextAction) => {
    setOpen(false)
    if (action === '/shake' || action === '/clear') contextCommand(api, session, action)
    else compactWith(api, session, action.compact)
  }
  return <ContextButtonView reading={reading} onAction={act} open={open} onOpenChange={setOpen} />
}
