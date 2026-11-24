import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, X } from 'lucide-react'
import { useEffect, useId, useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { requirements as attentionRequirements } from '@/components/attention/requirements'
import { settings } from '@/components/chat/slash'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
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
import { useOnboardingHere } from './OnboardingHost'

/** Settings › Devices' query key: the checklist shares its cache. */
const DEVICES = ['mesh', 'devices'] as const
/** Live state, but nothing here needs to be fresher than a minute or a window focus. */
const STALE = 60_000
/** How long the one-time "all set" state stays before the card hides itself. */
const CELEBRATE_MS = 8_000
const COLLAPSED_KEY = 'theuth-onboarding-checklist-collapsed'

/**
 * 215 §Checklist: "Get started" in the sidebar footer, above Settings. Items tick themselves off from live engine
 * state; each undone one has Show me (a focused tour, the wizard, the phone dialog or a deep link). Hide dismisses it
 * (Settings › General brings it back). All done: a quiet completion line, then it hides for good. The expanded
 * sidebar only: never the collapsed rail, a pop-out, a phone or a remote engine.
 */
export function Checklist() {
  const here = useOnboardingHere()
  const { isMobile } = useSidebar()
  const dismissed = useOnboarding((s) => s.checklistDismissed)
  const show = here && !isMobile && !dismissed
  return show ? <Card /> : null
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
        <ul id={listId} className="flex flex-col pb-1.5">
          {view.rows.map(({ item, done }) => (
            <Row key={item.id} item={item} done={done} next={item.id === next} onShow={() => showMe(item.id)} />
          ))}
        </ul>
      )}
    </section>
  )
}

const CARD = 'mb-1.5 rounded-xl border border-border-subtle bg-elevated/70 shadow-xs'

function Row({ item, done, next, onShow }: { item: ChecklistItem; done: boolean; next: boolean; onShow(): void }) {
  return (
    <li className="group/item flex min-h-8 items-center gap-2.5 px-3 text-[13px]">
      <span
        aria-hidden
        className={cn(
          'grid size-4 shrink-0 place-items-center rounded-full border',
          done ? 'border-transparent bg-[var(--fg-accent)] text-[var(--fg-inverse)]' : next ? 'border-[var(--fg-accent)]' : 'border-border-strong',
        )}
      >
        {done && <Check className="size-2.5" strokeWidth={3} />}
      </span>
      <span className={cn('min-w-0 flex-1 truncate', done ? 'text-fg-tertiary' : 'text-fg-secondary', next && 'text-foreground')} title={item.label}>
        {item.label}
        {item.optional && <span className="text-fg-tertiary"> (optional)</span>}
        <span className="sr-only">{done ? ', done' : ', not done'}</span>
      </span>
      {!done && (
        <Button
          variant="ghost"
          size="xs"
          aria-label={`Show me: ${item.label}`}
          className={cn('-mr-1.5 shrink-0 text-fg-tertiary hover:text-foreground', !next && 'opacity-0 group-hover/item:opacity-100 focus-visible:opacity-100')}
          onClick={onShow}
        >
          Show me
        </Button>
      )}
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
