import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, ChevronRight, X } from 'lucide-react'
import { useEffect, useId, useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { requirements as attentionRequirements } from '@/components/attention/requirements'
import { settings } from '@/components/chat/slash'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { Button } from '@/components/ui/button'
import { GROUPS } from '@/lib/nav'
import { type ChecklistItem, checklistDone, checklistView, progressLabel } from '@/lib/onboarding/checklist'
import { type ChecklistId, useOnboarding } from '@/lib/onboarding/state'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport, useSupported } from '@/rpc/capabilities'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { on, useSessions } from '@/store/sessions'
import { openLinkPhone } from './LinkPhoneDialog'
import { useChecklistShown, useGuidesHere } from './visibility'

/** Settings › Devices' query key: the checklist (and the end-of-tour phone offer) share its cache. */
export const DEVICES = ['mesh', 'devices'] as const
/** Live state, but nothing here needs to be fresher than a minute or a window focus. */
const STALE = 60_000
/** How long the one-time "all set" state stays before the card hides itself. */
const CELEBRATE_MS = 8_000
const COLLAPSED_KEY = 'theuth-onboarding-checklist-collapsed'

/**
 * 215 §Checklist: "Get started" in the sidebar footer, above Settings. Items tick themselves off from live engine
 * state; each undone one has Show me (a focused tour, the wizard, the phone dialog or a deep link). Hide dismisses it
 * (Settings › General brings it back). All done: a quiet completion line, then it hides for good. Shown only after
 * the wizard (finished or skipped), in the expanded sidebar: never the collapsed rail, a pop-out, a phone or a
 * remote engine.
 */
export function Checklist() {
  return useGuidesHere() ? <Gate /> : null
}

function Gate() {
  return useChecklistShown() ? <Card /> : null
}

function Card() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const discovery = useConnection((s) => s.discovery)
  const visited = useOnboarding((s) => s.visited)
  const canDevices = useQuerySupport(['devices_list'])
  const canSlack = useQuerySupport(['slack_status'])
  const canTelegram = useQuerySupport(['telegram_status'])
  const canSignal = useQuerySupport(['signal_status'])
  const canSessions = useQuerySupport(['list_sessions'])
  const models = useModels()
  const live = useSessions((s) => Object.keys(s.sessions).length)
  // The sidebar's own `list_sessions` cache; read, never fetched from here.
  const listed = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: false }).data?.length ?? 0
  const devices = useQuery({ enabled: canDevices, queryKey: DEVICES, queryFn: () => api.devices_list(), staleTime: STALE, retry: false })
  const slack = useQuery({ enabled: canSlack, queryKey: queryKeys.slack, queryFn: () => api.slack_status(), staleTime: STALE, retry: false })
  const telegram = useQuery({ enabled: canTelegram, queryKey: queryKeys.telegram, queryFn: () => api.telegram_status(), staleTime: STALE, retry: false })
  const signal = useQuery({ enabled: canSignal, queryKey: queryKeys.signal, queryFn: () => api.signal_status(), staleTime: STALE, retry: false })
  useEffect(() => {
    const refresh = () => void qc.invalidateQueries({ queryKey: DEVICES })
    const offs = [on('link.events@1', 'DevicePaired', refresh), on('link.events@1', 'DevicesChanged', refresh)]
    return () => offs.forEach((off) => off())
  }, [qc])

  const available: Partial<Record<ChecklistId, boolean>> = {
    model: useQuerySupport(['list_models']),
    chat: canSessions,
    inbox: useSupported(attentionRequirements),
    costs: useSupported(costsRequirements),
    library: GROUPS.library.some((t) => supports(discovery, t.required)),
    phone: canDevices,
    channel: canSlack || canTelegram || canSignal,
  }
  const view = checklistView(
    checklistDone({
      models: models.data?.length,
      sessions: Math.max(live, listed),
      visited,
      devices: devices.data,
      slack: slack.data,
      telegram: telegram.data,
      signal: signal.data,
    }),
    available,
  )

  const [collapsed, setCollapsed] = useState(() => globalThis.localStorage?.getItem(COLLAPSED_KEY) === '1')
  const toggle = () => {
    const next = !collapsed
    setCollapsed(next)
    try {
      localStorage.setItem(COLLAPSED_KEY, next ? '1' : '0')
    } catch {
      // Blocked storage: the choice lasts this run.
    }
  }
  // Complete: the "all set" line once, then the card dismisses itself (the dismissal is stored, so it stays gone).
  useEffect(() => {
    if (!view.complete) return
    const t = setTimeout(() => useOnboarding.getState().dismissChecklist(), CELEBRATE_MS)
    return () => clearTimeout(t)
  }, [view.complete])

  const showMe = (id: ChecklistId) => {
    const s = useOnboarding.getState()
    if (id === 'model') s.openWizard('model')
    else if (id === 'phone') openLinkPhone()
    else if (id === 'channel') void settings(navigate, 'slack')
    else s.requestTour(id)
  }

  const listId = useId()
  const next = view.rows.find((r) => !r.done)?.item.id

  if (view.complete) {
    return (
      <section data-tour="onboarding-checklist" aria-label="Get started" role="status" className={CARD}>
        <div className="flex items-start gap-2.5 px-3 py-2.5">
          <Ring done={view.done} total={view.total} />
          <div className="min-w-0 flex-1">
            <p className="text-[13px] font-medium">You’re all set</p>
            <p className="mt-0.5 text-xs leading-snug text-fg-tertiary">Everything is connected. The welcome tour is under Settings › General.</p>
          </div>
        </div>
      </section>
    )
  }

  return (
    <section data-tour="onboarding-checklist" aria-label="Get started" className={CARD}>
      <div className="flex items-center gap-1 pr-1">
        <button
          type="button"
          aria-expanded={!collapsed}
          aria-controls={listId}
          onClick={toggle}
          className="flex h-10 min-w-0 flex-1 items-center gap-2.5 rounded-[10px] pl-3 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <Ring done={view.done} total={view.total} />
          <span className="truncate text-[13px] font-medium">Get started</span>
          <span className="text-xs text-fg-tertiary tabular-nums">{progressLabel(view)}</span>
          <ChevronDown aria-hidden className={cn('ml-auto size-3.5 shrink-0 text-fg-tertiary transition-transform motion-reduce:transition-none', collapsed && '-rotate-90')} />
        </button>
        <Button variant="ghost" size="icon-xs" className="text-fg-tertiary" aria-label="Hide getting started" title="Hide (bring it back from Settings › General)" onClick={() => useOnboarding.getState().dismissChecklist()}>
          <X />
        </Button>
      </div>
      {!collapsed && (
        <ul id={listId} className="flex flex-col px-1 pb-1.5">
          {view.rows.map(({ item, done }) => (
            <Row key={item.id} item={item} done={done} next={item.id === next} onShow={() => showMe(item.id)} />
          ))}
        </ul>
      )}
    </section>
  )
}

const CARD = 'rounded-xl border border-border-subtle bg-elevated/70 shadow-xs'

/** Done: a filled ochre check. Not done: an empty ring (ochre for the one to do next). A status, not a control. */
function Status({ done, next }: { done: boolean; next: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        'mt-px grid size-3.5 shrink-0 place-items-center rounded-full',
        done ? 'bg-[var(--fg-accent)] text-[var(--fg-inverse)]' : cn('border-[1.5px]', next ? 'border-[var(--fg-accent)]' : 'border-border-strong'),
      )}
    >
      {done && <Check className="size-2.5" strokeWidth={3.5} />}
    </span>
  )
}

/**
 * One item. Its label wraps rather than truncates; an undone row is itself the Show me button (the chevron appears
 * on hover and focus), so nothing competes with the label for the card's width.
 */
function Row({ item, done, next, onShow }: { item: ChecklistItem; done: boolean; next: boolean; onShow(): void }) {
  const label = (
    <span className="min-w-0 flex-1 leading-snug">
      {item.label}
      {item.optional && <span className="ml-1.5 rounded-sm bg-layer-2 px-1 py-px align-[1px] text-[10px] text-fg-tertiary">optional</span>}
    </span>
  )
  if (done) {
    return (
      <li className="flex items-start gap-2.5 px-2 py-1.5 text-[13px] text-fg-tertiary">
        <Status done next={false} />
        {label}
        <span className="sr-only">done</span>
      </li>
    )
  }
  return (
    <li>
      <button
        type="button"
        onClick={onShow}
        title="Show me"
        className={cn(
          'group/item flex w-full items-start gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] outline-none hover:bg-layer-2/60 focus-visible:bg-layer-2/60 focus-visible:ring-2 focus-visible:ring-ring/50',
          next ? 'text-foreground' : 'text-fg-secondary',
        )}
      >
        <Status done={false} next={next} />
        {label}
        <span className="sr-only">, not done. Show me</span>
        <ChevronRight aria-hidden className="mt-px size-3.5 shrink-0 text-fg-tertiary opacity-0 transition-opacity group-hover/item:opacity-100 group-focus-visible/item:opacity-100 motion-reduce:transition-none" />
      </button>
    </li>
  )
}

/** Progress as a small ochre ring. */
function Ring({ done, total }: { done: number; total: number }) {
  const r = 7
  const c = 2 * Math.PI * r
  const share = total ? done / total : 0
  return (
    <svg viewBox="0 0 18 18" className="size-[18px] shrink-0 -rotate-90" aria-hidden>
      <circle cx="9" cy="9" r={r} fill="none" stroke="var(--border-default)" strokeWidth="2" />
      <circle
        cx="9"
        cy="9"
        r={r}
        fill="none"
        stroke="var(--fg-accent)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={c * (1 - share)}
        className="transition-[stroke-dashoffset] duration-500 motion-reduce:transition-none"
      />
    </svg>
  )
}
