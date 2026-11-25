// 217 §4–§5: what the status dot says. Connection and needs-you only; a notice's kind never colours it.

import type { ConnState } from '@/rpc/client'
import type { NoticeKind } from '@/store/notices'

export type Tone = 'ok' | 'attn' | 'idle' | 'down'
export type Dot = { tone: Tone; pulse: boolean }

export function dotOf(state: ConnState, needsYou: number, rate: number): Dot {
  if (state === 'open') return needsYou > 0 ? { tone: 'attn', pulse: false } : { tone: 'ok', pulse: rate > 0 }
  if (state === 'connecting') return { tone: 'idle', pulse: true }
  return { tone: 'down', pulse: false }
}

export const DOT_BG: Record<Tone, string> = {
  ok: 'bg-success',
  attn: 'bg-warning',
  idle: 'bg-fg-tertiary',
  down: 'bg-danger',
}

export const isDown = (state: ConnState) => state === 'reconnecting' || state === 'closed'

/** A frame received this recently proves the socket, whatever the last state event said. */
export const LIVE_MS = 5_000

/**
 * 217 §4: the state the bar shows, from evidence as well as events. Anything received in the last `LIVE_MS` reads as
 * `open`; so disconnected/reconnecting shows only when the client says so and the line has been silent that long.
 */
export const liveState = (state: ConnState, lastRx: number, now: number): ConnState => (now - lastRx < LIVE_MS ? 'open' : state)

export const KIND_TAG: Record<NoticeKind, { label: string; className: string }> = {
  info: { label: 'info', className: 'bg-layer-2 text-fg-secondary' },
  done: { label: 'done', className: 'bg-success-bg text-success' },
  warn: { label: 'warn', className: 'bg-warning-bg text-warning' },
  error: { label: 'error', className: 'bg-danger-bg text-danger' },
  progress: { label: 'working', className: 'bg-layer-2 text-fg-secondary' },
}

/** Whole seconds until `at`, never below zero. */
export const secondsUntil = (at: number, now: number) => Math.max(0, Math.ceil((at - now) / 1000))

/** The sparkline's polyline points in a `w`×`h` box, the busiest second at the top. */
export function sparkPoints(buckets: readonly number[], w: number, h: number): string {
  const max = Math.max(1, ...buckets)
  const step = buckets.length > 1 ? w / (buckets.length - 1) : 0
  return buckets.map((v, i) => `${(i * step).toFixed(1)},${(h - 1 - (v / max) * (h - 2)).toFixed(1)}`).join(' ')
}
