// 217 §6: the desktop's quiet ledger under sidebar and column. Never a drag region; hidden on the phone layout, where
// `PhoneStatusLine` speaks instead.

import { useNavigate } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { motionAllowed } from '@/lib/animations'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { CloudCreatingChip } from '@/components/cloud/CloudStatusChip'
import { StatusStrip } from '@/components/plugins/StatusStrip'
import { BUCKETS, formatRate } from '@/store/event-rate'
import { useNotices } from '@/store/notices'
import { SPARK_COMPACT_W, SPARK_FULL_W, SPARK_H, isDown, secondsUntil, sparkPath } from './model'
import { NoticeHistory } from './NoticeHistory'
import { type EngineStatus, KindTag, StatusDot, act, closeNotice, useEngineStatus, useShownNotice, useStatusClock } from './parts'

/** About 8 % carnelian over the sidebar: the bar says "down" without shouting. */
const DOWN_TINT = 'linear-gradient(color-mix(in oklab, var(--status-danger-fg) 8%, transparent), color-mix(in oklab, var(--status-danger-fg) 8%, transparent))'

/** `guest`: the `/d` page has no engine scope; notices only. `defaultOpen` opens the history (dev fixture). */
export function StatusBar({ guest = false, defaultOpen = false }: { guest?: boolean; defaultOpen?: boolean }) {
  const mobile = useIsMobile()
  if (mobile) return null
  return guest ? <GuestBar defaultOpen={defaultOpen} /> : <EngineBar defaultOpen={defaultOpen} />
}

function EngineBar({ defaultOpen }: { defaultOpen: boolean }) {
  useStatusClock()
  const s = useEngineStatus()
  const down = isDown(s.state)
  return (
    <Shell
      defaultOpen={defaultOpen}
      down={down}
      left={
        <span className="flex min-w-0 shrink-0 items-center gap-1.5" title={`${s.name} · ${s.state}`}>
          <StatusDot dot={s.dot} />
          <span className={cn('max-w-48 truncate', down && 'text-danger')}>{down ? `${s.name} disconnected` : s.name}</span>
        </span>
      }
      slot={(open) => <NoticeSlot onOpen={open} after={down ? <Reconnect s={s} /> : null} />}
      right={
        <>
          <CloudCreatingChip />
          {s.needsYou > 0 && <NeedsYou n={s.needsYou} />}
          {/* 216: plugin status placements (their dots never enter `dotOf`). */}
          <StatusStrip />
          <Sep />
        </>
      }
      tail={<Rate s={s} />}
    />
  )
}

function GuestBar({ defaultOpen }: { defaultOpen: boolean }) {
  useStatusClock(false)
  return <Shell defaultOpen={defaultOpen} slot={(open) => <NoticeSlot onOpen={open} />} />
}

function Shell({
  defaultOpen,
  down = false,
  left,
  slot,
  right,
  tail,
}: {
  defaultOpen: boolean
  down?: boolean
  left?: ReactNode
  slot(open: () => void): ReactNode
  right?: ReactNode
  tail?: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  const unread = useNotices((st) => st.unreadErrors)
  return (
    <NoticeHistory open={open} onOpenChange={setOpen}>
      {/* iOS (`ios:`, keyed on `<html data-container>`): the bar is the home-indicator band itself, content centred in
       * it, inset past the screen's rounded corners (~18pt on an iPad), and compact type so a ~20pt band holds it.
       * `--statusbar-h` in routes/__root.tsx carries the same height. */}
      <footer
        data-status-bar
        data-down={down || undefined}
        aria-label="Status"
        className="flex h-[22px] shrink-0 items-center gap-2 border-t border-border-subtle bg-sidebar px-2.5 font-sans text-[11px] leading-none text-fg-tertiary tabular-nums select-none ios:h-[max(var(--safe-bottom),18px)] ios:pr-[max(var(--safe-right),28px)] ios:pl-[max(var(--safe-left),28px)] ios:text-[10px] ios:[&_.status-dot]:size-[6px]"
        style={down ? { backgroundImage: DOWN_TINT } : undefined}
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

const Sep = () => <span aria-hidden className="h-3 w-px shrink-0 bg-border-subtle ios:h-2.5" />

/**
 * One persistent live region whose content is swapped, so a screen reader hears each notice; an error speaks as an
 * alert. The title keeps its room: the description gives way first, and the title truncates only on its own.
 * `after` (the reconnect countdown) follows the notice, or stands alone.
 */
function NoticeSlot({ onOpen, after }: { onOpen(): void; after?: ReactNode }) {
  const { notice, leaving } = useShownNotice()
  return (
    <div className="flex h-full min-w-0 flex-1 items-center gap-1.5">
      <div role="status" aria-live="polite" aria-atomic="true" className="flex min-w-0 items-center">
        {notice && (
          <div
            key={notice.id}
            data-notice-slot
            className={cn('notice-in group/notice flex min-w-0 items-center gap-1.5 transition-opacity duration-200', leaving && 'opacity-0')}
          >
            <button type="button" data-notice-open className="flex min-w-0 items-center gap-1.5 text-left" onClick={onOpen}>
              <KindTag kind={notice.kind} />
              <span role={notice.kind === 'error' ? 'alert' : undefined} className="min-w-0 truncate text-fg-secondary">
                {notice.text}
              </span>
              {notice.description && <span className="min-w-0 flex-[0_1000_auto] truncate">· {notice.description}</span>}
            </button>
            {notice.action && (
              <button type="button" className="shrink-0 text-fg-secondary underline underline-offset-2 hover:text-foreground" onClick={() => act(notice, notice.action!)}>
                {notice.action.label}
              </button>
            )}
            {notice.sticky && (
              <button
                type="button"
                aria-label={notice.cancel ? notice.cancel.label : 'Dismiss'}
                className="shrink-0 opacity-0 group-hover/notice:opacity-100 focus-visible:opacity-100 hover:text-foreground"
                onClick={() => closeNotice(notice)}
              >
                <X className="size-3" />
              </button>
            )}
          </div>
        )}
      </div>
      {notice && after && <span aria-hidden className="shrink-0">·</span>}
      {after}
      <button
        type="button"
        {...(notice ? { 'aria-hidden': true, tabIndex: -1 } : { 'data-notice-open': true, 'aria-label': 'Recent notices' })}
        className="h-full min-w-4 flex-1 self-stretch"
        onClick={onOpen}
      />
    </div>
  )
}

function Reconnect({ s }: { s: EngineStatus }) {
  const now = Date.now()
  const wait = s.retryAt !== null && s.retryAt > now ? `Reconnecting in ${secondsUntil(s.retryAt, now)}s` : 'Reconnecting…'
  return (
    <span data-reconnect className="flex shrink-0 items-center gap-1 whitespace-nowrap text-fg-secondary">
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
    <button type="button" className="shrink-0 text-warning hover:underline" onClick={() => void navigate({ to: '/attention', search: true })}>
      <span className="font-mono">{n}</span> need you
    </button>
  )
}

/**
 * Smoothed 4× events-per-second ribbon (`SPARK_FULL_W` idle → `SPARK_COMPACT_W` while a notice occupies the bar).
 * Uses C · Plucked Cord spring physics: releasing the left end near the notification whips the cord back toward the
 * right `ev/s` anchor with a damped traveling wave. Zero idle RAF — the loop runs only while snapping or stretching
 * and stops the instant the spring settles; under Reduce Motion or Appearance › Animations off, widths snap cleanly.
 */
function Rate({ s }: { s: EngineStatus }) {
  const live = s.state === 'open'
  const compact = useNotices((st) => st.current !== null)
  const { boxRef, svgRef, lineRef, fillRef, initialW, initial } = usePluckedSpark(s.buckets, compact)
  return (
    <span className="flex shrink-0 items-center gap-1.5" title={`Engine events per second, last ${BUCKETS} s`}>
      <span ref={boxRef} className="inline-flex h-[14px] items-center justify-end overflow-visible" style={{ width: initialW }}>
        <svg
          ref={svgRef}
          data-sparkline
          data-compact={compact ? 'true' : undefined}
          width={initialW}
          height={SPARK_H}
          viewBox={`0 0 ${initialW} ${SPARK_H}`}
          aria-hidden
          className={cn('block overflow-visible', !live && 'opacity-40')}
        >
          <path ref={fillRef} d={initial.fill} fill="color-mix(in oklab, var(--status-success-fg) 11%, transparent)" />
          <path ref={lineRef} d={initial.line} fill="none" stroke="var(--status-success-fg)" strokeWidth={1.25} strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </span>
      <span className="font-mono">{live ? formatRate(s.rate) : '–'} ev/s</span>
    </span>
  )
}

function usePluckedSpark(buckets: readonly number[], compact: boolean) {
  const boxRef = useRef<HTMLSpanElement>(null)
  const svgRef = useRef<SVGSVGElement>(null)
  const lineRef = useRef<SVGPathElement>(null)
  const fillRef = useRef<SVGPathElement>(null)
  const bucketsRef = useRef(buckets)
  bucketsRef.current = buckets

  const initialW = compact ? SPARK_COMPACT_W : SPARK_FULL_W
  const stateRef = useRef<{ w: number; vel: number; targetW: number; age: number; dir: -1 | 0 | 1; raf: number; lastTs: number }>({
    w: initialW,
    vel: 0,
    targetW: initialW,
    age: 99,
    dir: 0,
    raf: 0,
    lastTs: 0,
  })
  const prevCompact = useRef(compact)

  const paint = (w: number, snap?: { age: number; dir: -1 | 0 | 1 }) => {
    const layoutW = Math.max(SPARK_COMPACT_W - 8, Math.min(SPARK_FULL_W + 6, w))
    const wStr = layoutW.toFixed(1)
    if (boxRef.current) boxRef.current.style.width = `${wStr}px`
    if (svgRef.current) {
      svgRef.current.setAttribute('width', wStr)
      svgRef.current.setAttribute('viewBox', `0 0 ${wStr} ${SPARK_H}`)
    }
    const { line, fill } = sparkPath(bucketsRef.current, layoutW, SPARK_H, snap)
    if (lineRef.current) lineRef.current.setAttribute('d', line)
    if (fillRef.current) fillRef.current.setAttribute('d', fill)
  }

  // Trigger Plucked Cord spring whenever a notice arrives or clears.
  useEffect(() => {
    if (prevCompact.current === compact) return
    prevCompact.current = compact
    const st = stateRef.current
    const target = compact ? SPARK_COMPACT_W : SPARK_FULL_W
    st.targetW = target

    if (!motionAllowed() || typeof requestAnimationFrame !== 'function') {
      if (st.raf) {
        cancelAnimationFrame(st.raf)
        st.raf = 0
      }
      st.w = target
      st.vel = 0
      st.age = 99
      st.dir = 0
      paint(target)
      return
    }

    st.age = 0
    st.dir = compact ? -1 : 1
    st.vel = compact ? -640 : 120
    st.lastTs = performance.now()

    if (st.raf) return
    const step = (ts: number) => {
      const dt = Math.min(0.05, Math.max(0.001, (ts - st.lastTs) / 1000))
      st.lastTs = ts
      st.age += dt

      const retracting = st.targetW < SPARK_FULL_W
      const stiffness = retracting ? 300 : 115
      const damping = retracting ? 16 : 18
      const force = (st.targetW - st.w) * stiffness
      st.vel = (st.vel + force * dt) * Math.exp(-damping * dt)
      st.w = Math.max(24, st.w + st.vel * dt)

      const energy = st.dir === -1 ? Math.exp(-st.age * 5.8) : Math.exp(-st.age * 7.5) * 0.25
      const settled = Math.abs(st.targetW - st.w) < 0.25 && Math.abs(st.vel) < 0.8 && energy < 0.008

      if (settled) {
        st.w = st.targetW
        st.vel = 0
        st.dir = 0
        st.raf = 0
        paint(st.targetW)
        return
      }

      paint(st.w, { age: st.age, dir: st.dir })
      st.raf = requestAnimationFrame(step)
    }
    st.raf = requestAnimationFrame(step)
  }, [compact])

  // Static data ticks when the spring is at rest.
  useEffect(() => {
    const st = stateRef.current
    if (st.raf === 0) paint(st.w)
  }, [buckets])

  useEffect(
    () => () => {
      if (stateRef.current.raf) cancelAnimationFrame(stateRef.current.raf)
    },
    [],
  )

  return { boxRef, svgRef, lineRef, fillRef, initialW, initial: sparkPath(buckets, initialW, SPARK_H) }
}
