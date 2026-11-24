// 217 §6: the desktop's quiet ledger under sidebar and column. Never a drag region; hidden on the phone layout, where
// `PhoneStatusLine` speaks instead.

import { useNavigate } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { formatRate } from '@/store/event-rate'
import { useNotices } from '@/store/notices'
import { isDown, secondsUntil, sparkPoints } from './model'
import { NoticeHistory } from './NoticeHistory'
import { type EngineStatus, KindTag, StatusDot, act, useEngineStatus, useShownNotice, useStatusClock } from './parts'

/** `guest`: the `/d` page has no engine scope; notices only. */
export function StatusBar({ guest = false }: { guest?: boolean }) {
  const mobile = useIsMobile()
  if (mobile) return null
  return guest ? <GuestBar /> : <EngineBar />
}

function EngineBar() {
  const now = useStatusClock()
  const s = useEngineStatus()
  const down = isDown(s.state)
  return (
    <Shell
      down={down}
      left={
        <span className="flex min-w-0 shrink-0 items-center gap-1.5" title={`${s.name} · ${s.state}`}>
          <StatusDot dot={s.dot} />
          <span className={cn('max-w-48 truncate', down && 'text-danger')}>{down ? `${s.name} disconnected` : s.name}</span>
        </span>
      }
      slot={(open) => (down ? <Reconnect s={s} now={now} /> : <NoticeSlot onOpen={open} />)}
      right={
        <>
          {s.needsYou > 0 && <NeedsYou n={s.needsYou} />}
          <Sep />
        </>
      }
      tail={<Rate s={s} />}
    />
  )
}

function GuestBar() {
  useStatusClock(false)
  return <Shell slot={(open) => <NoticeSlot onOpen={open} />} />
}

function Shell({
  down = false,
  left,
  slot,
  right,
  tail,
}: {
  down?: boolean
  left?: ReactNode
  slot(open: () => void): ReactNode
  right?: ReactNode
  tail?: ReactNode
}) {
  const [open, setOpen] = useState(false)
  const unread = useNotices((st) => st.unreadErrors)
  return (
    <NoticeHistory open={open} onOpenChange={setOpen}>
      <footer
        data-status-bar
        aria-label="Status"
        className={cn(
          'flex h-[22px] shrink-0 items-center gap-2 border-t border-border-subtle bg-sidebar px-2.5 font-sans text-[11px] leading-none text-fg-tertiary tabular-nums select-none',
          down && 'bg-[color-mix(in_oklab,var(--status-danger-fg)_7%,var(--sidebar))]',
        )}
      >
        {left}
        {left && <Sep />}
        <div className="flex min-w-0 flex-1 items-center">{slot(() => setOpen((o) => !o))}</div>
        {right}
        {unread > 0 && (
          <button type="button" className="shrink-0 font-mono text-danger" aria-label={`${unread} unread errors`} onClick={() => setOpen(true)}>
            ● {unread}
          </button>
        )}
        {tail}
      </footer>
    </NoticeHistory>
  )
}

const Sep = () => <span aria-hidden className="h-3 w-px shrink-0 bg-border-subtle" />

function NoticeSlot({ onOpen }: { onOpen(): void }) {
  const { notice, leaving } = useShownNotice()
  if (!notice)
    return <button type="button" aria-label="Recent notices" className="h-full min-w-0 flex-1 self-stretch" onClick={onOpen} />
  return (
    <div
      key={`${notice.id}:${notice.at}`}
      data-notice-slot
      className={cn('notice-in group/notice flex min-w-0 flex-1 items-center gap-1.5 transition-opacity duration-200', leaving && 'opacity-0')}
    >
      <button type="button" className="flex min-w-0 items-center gap-1.5 text-left" onClick={onOpen}>
        <KindTag kind={notice.kind} />
        <span className="truncate text-fg-secondary">{notice.text}</span>
        {notice.description && <span className="hidden truncate lg:inline">· {notice.description}</span>}
      </button>
      {notice.action && (
        <button type="button" className="shrink-0 text-fg-secondary underline underline-offset-2 hover:text-foreground" onClick={() => act(notice, notice.action!)}>
          {notice.action.label}
        </button>
      )}
      {notice.sticky && (
        <button
          type="button"
          aria-label="Dismiss"
          className="shrink-0 opacity-0 group-hover/notice:opacity-100 focus-visible:opacity-100 hover:text-foreground"
          onClick={() => useNotices.getState().dismiss(notice.id)}
        >
          <X className="size-3" />
        </button>
      )}
      <button type="button" aria-hidden tabIndex={-1} className="h-full min-w-4 flex-1 self-stretch" onClick={onOpen} />
    </div>
  )
}

function Reconnect({ s, now }: { s: EngineStatus; now: number }) {
  const wait = s.retryAt !== null && s.retryAt > now ? `Reconnecting in ${secondsUntil(s.retryAt, now)}s` : 'Reconnecting…'
  return (
    <span className="flex min-w-0 items-center gap-1 truncate text-danger">
      {wait}
      {s.attempt > 0 && <span> · attempt {s.attempt}</span>}
      {s.retryAt !== null && (
        <>
          <span> · </span>
          <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={s.retryNow}>
            Retry now
          </button>
        </>
      )}
    </span>
  )
}

function NeedsYou({ n }: { n: number }) {
  const navigate = useNavigate()
  return (
    <button type="button" className="shrink-0 text-warning hover:underline" onClick={() => void navigate({ to: '/attention' })}>
      <span className="font-mono">{n}</span> need you
    </button>
  )
}

function Rate({ s }: { s: EngineStatus }) {
  const live = s.state === 'open'
  return (
    <span className="flex shrink-0 items-center gap-1.5" title="Engine events per second, last 30 s">
      <svg width={46} height={12} viewBox="0 0 46 12" aria-hidden className={cn(!live && 'opacity-40')}>
        <polyline points={sparkPoints(s.buckets, 46, 12)} fill="none" stroke="var(--status-success-fg)" strokeWidth={1} strokeLinejoin="round" />
      </svg>
      <span className="font-mono">{live ? formatRate(s.rate) : '–'} ev/s</span>
    </span>
  )
}
