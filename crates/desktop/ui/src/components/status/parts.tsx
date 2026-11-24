import { LoaderCircle } from 'lucide-react'
import { useEffect, useState } from 'react'
import { LOCAL, LOCAL_NAME, useEngines } from '@/link/engines'
import { cn } from '@/lib/utils'
import type { ConnState } from '@/rpc/client'
import { useApprovals, useAsks, useSecretRequests } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { rate, useEventRate } from '@/store/event-rate'
import { type Notice, useNotices } from '@/store/notices'
import { DOT_BG, type Dot, KIND_TAG, dotOf } from './model'

/** §5: approvals, asks and secret requests waiting on the active engine. */
export function useNeedsYou(): number {
  const approvals = useApprovals((s) => s.pending.length)
  const asks = useAsks((s) => s.pending.length)
  const secrets = useSecretRequests((s) => s.pending.length)
  return approvals + asks + secrets
}

export type EngineStatus = {
  name: string
  state: ConnState
  retryAt: number | null
  attempt: number
  retryNow(): void
  buckets: number[]
  rate: number
  needsYou: number
  dot: Dot
}

/** The active engine as the bar shows it. Link engines dial through the same `RpcClient` backoff, so one source. */
export function useEngineStatus(): EngineStatus {
  const active = useEngines((s) => s.active)
  const listed = useEngines((s) => s.engines.find((h) => h.id === s.active)?.name)
  const info = useConnection((s) => s.info?.name)
  const state = useConnection((s) => s.state)
  const retryAt = useConnection((s) => s.retryAt)
  const attempt = useConnection((s) => s.attempt)
  const retryNow = useConnection((s) => s.retryNow)
  const buckets = useEventRate((s) => s.buckets)
  const needsYou = useNeedsYou()
  const r = rate(buckets)
  const name = info ?? listed ?? (active === LOCAL ? LOCAL_NAME : 'Node')
  return { name, state, retryAt, attempt, retryNow, buckets, rate: r, needsYou, dot: dotOf(state, needsYou, r) }
}

/**
 * The bar's one clock: closes the event-rate second, expires the slot's notice and re-renders the countdown, once a
 * second. Readers take `Date.now()` at render; the tick only forces the render.
 */
export function useStatusClock(rate = true): void {
  const [, setTick] = useState(0)
  useEffect(() => {
    const id = window.setInterval(() => {
      const t = Date.now()
      if (rate) useEventRate.getState().roll(t)
      useNotices.getState().expire(t)
      setTick((n) => n + 1)
    }, 1_000)
    return () => clearInterval(id)
  }, [rate])
  // A notice's lifetime ends on time, not on the next whole-second tick.
  const expiresAt = useNotices((s) => s.current?.expiresAt ?? null)
  useEffect(() => {
    if (expiresAt === null) return
    const id = window.setTimeout(() => useNotices.getState().expire(), Math.max(0, expiresAt - Date.now()))
    return () => clearTimeout(id)
  }, [expiresAt])
}

/** The slot's notice, held a moment after it leaves so it can fade. */
export function useShownNotice(): { notice: Notice | null; leaving: boolean } {
  const current = useNotices((s) => s.current)
  const [held, setHeld] = useState<Notice | null>(current)
  useEffect(() => {
    if (current) {
      setHeld(current)
      return
    }
    const id = window.setTimeout(() => setHeld(null), 200)
    return () => clearTimeout(id)
  }, [current])
  return { notice: current ?? held, leaving: !current && held !== null }
}

export function StatusDot({ dot, small = false }: { dot: Dot; small?: boolean }) {
  return (
    <span
      aria-hidden
      data-tone={dot.tone}
      data-pulse={dot.pulse || undefined}
      className={cn('status-dot inline-block aspect-square shrink-0 rounded-full', small ? 'size-[5px]' : 'size-[7px]', DOT_BG[dot.tone])}
    />
  )
}

export function KindTag({ kind }: { kind: Notice['kind'] }) {
  const tag = KIND_TAG[kind]
  return (
    <span className={cn('inline-flex h-[14px] shrink-0 items-center gap-0.5 rounded-full px-1.5 text-[10px] leading-none font-medium', tag.className)}>
      {kind === 'progress' && <LoaderCircle aria-hidden className="size-2.5 animate-spin motion-reduce:animate-none" />}
      {tag.label}
    </span>
  )
}

/** Runs an action and settles its notice: acting on a sticky notice dismisses it. */
export function act(notice: Notice, a: NonNullable<Notice['action']>): void {
  a.onClick()
  useNotices.getState().dismiss(notice.id)
}

/** The × on a sticky notice: its `cancel` runs (an offer's "Not now" records the answer), then it is dismissed. */
export function closeNotice(notice: Notice): void {
  notice.cancel?.onClick()
  useNotices.getState().dismiss(notice.id)
}
