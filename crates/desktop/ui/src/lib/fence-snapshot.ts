// Design 164 D3/D4: the `fence_snapshot` runner — `page-preview.ts`'s shape for channel pictures. On
// `FenceSnapshotRequested` the main window answers `started`, queues the request (two renders at a time), mounts the
// fence off-screen with the chat's own renderer (`components/chat/FenceSnapshot.tsx`, imported on first use), waits for
// the layout to settle, rasterizes it and answers `done` — or `failed` on any error or after `FAIL_MS`.
// `FenceSnapshotResolved` drops a late render. Queue, settle timing and the size ladder are pure functions below so
// vitest can drive them.

import { clientKind } from '@/lib/page-preview'
import { container } from '@/lib/platform'
import type { FenceSnapshotAnswer } from '@/rpc/bindings/FenceSnapshotAnswer'
import type { FenceSnapshotRequested } from '@/rpc/bindings/FenceSnapshotRequested'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

/** Renders at a time; the rest wait in arrival order. */
export const MAX_RUNNING = 2
/** From the request to `failed` (the engine gives up at 20 s, D3). */
export const FAIL_MS = 15_000
/** Quiet time after the last DOM change before the picture is taken. */
export const QUIET_MS = 300
/** Longest wait for the layout after mount; still pending then → `failed`. */
export const SETTLE_MS = 8_000
/** D4 caps. */
export const MAX_PNG_BYTES = 4 * 1024 * 1024
export const MAX_SIDE = 8000

/** Request ids rendering now and waiting, in arrival order. */
export type Queue = { running: readonly string[]; waiting: readonly string[] }
export const IDLE: Queue = { running: [], waiting: [] }

/** Fill the free render slots from the front of the line; `start` is who moved up. */
function pump(q: Queue): { queue: Queue; start: string[] } {
  const free = Math.max(0, MAX_RUNNING - q.running.length)
  const start = q.waiting.slice(0, free)
  return { queue: { running: [...q.running, ...start], waiting: q.waiting.slice(free) }, start }
}

/** A new request joins the line (a known id changes nothing). */
export function admit(q: Queue, id: string): { queue: Queue; start: string[] } {
  if (q.running.includes(id) || q.waiting.includes(id)) return { queue: q, start: [] }
  return pump({ ...q, waiting: [...q.waiting, id] })
}

/** A request finished, failed or was resolved elsewhere: out of the line, its slot to the next one. */
export function release(q: Queue, id: string): { queue: Queue; start: string[] } {
  return pump({ running: q.running.filter((r) => r !== id), waiting: q.waiting.filter((w) => w !== id) })
}

/**
 * When to look at a mounted fence: `QUIET_MS` after its last DOM change once its content is there, never past
 * `SETTLE_MS` after mount. At that time a ready fence is drawn, a pending one fails.
 */
export function settleAt(mounted: number, lastChange: number, ready: boolean): number {
  return ready ? Math.min(lastChange + QUIET_MS, mounted + SETTLE_MS) : mounted + SETTLE_MS
}

/** D4's ladder: the requested scale (1–3), then each lower step of 1.5 and 1. */
export function scales(requested: number): number[] {
  const first = Math.min(3, Math.max(1, requested))
  return [first, ...[1.5, 1].filter((s) => s < first)]
}

/** A PNG's pixel size from its IHDR chunk (bytes 16–23, big-endian). */
export function pngSize(png: Uint8Array): { width: number; height: number } {
  const v = new DataView(png.buffer, png.byteOffset, png.byteLength)
  return { width: v.getUint32(16), height: v.getUint32(20) }
}

/**
 * The PNG of a `width`×`height` CSS px picture within D4's caps: the first scale of the ladder whose sides stay
 * ≤ `MAX_SIDE` and whose bytes stay ≤ `MAX_PNG_BYTES`; a reason when none does.
 */
export async function fitPng(
  width: number,
  height: number,
  requested: number,
  draw: (scale: number) => Promise<Uint8Array>,
): Promise<{ png: Uint8Array } | { reason: string }> {
  let last = 'no scale fits'
  for (const s of scales(requested)) {
    if (Math.ceil(width * s) > MAX_SIDE || Math.ceil(height * s) > MAX_SIDE) {
      last = `${Math.ceil(width * s)}×${Math.ceil(height * s)} px exceeds ${MAX_SIDE} px`
      continue
    }
    const png = await draw(s)
    if (png.byteLength <= MAX_PNG_BYTES) return { png }
    last = `PNG of ${png.byteLength} bytes exceeds ${MAX_PNG_BYTES}`
  }
  return { reason: `picture too large at scale 1: ${last}` }
}

/** Standard base64, no `data:` prefix. */
export function base64(bytes: Uint8Array): string {
  let s = ''
  // Chunks keep `fromCharCode`'s argument list under the engines' limits.
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

/** One render's answer: `done`/`failed` without the client, which the runner adds. */
export type Outcome = { png: Uint8Array } | { reason: string }

/**
 * Draw one request; `finish` gets the outcome once. Returns the cancel (unmount without answering). Mount, settle
 * and rasterize live in the lazily imported host.
 */
function render(e: FenceSnapshotRequested, finish: (o: Outcome) => void): () => void {
  let cancel: (() => void) | null = null
  let over = false
  const fail = (err: unknown) => !over && finish({ reason: err instanceof Error ? err.message : String(err) })
  import('@/components/chat/FenceSnapshot').then(({ snapshotFence }) => {
    if (over) return
    cancel = snapshotFence(e, (o) => !over && finish(o))
  }, fail)
  return () => {
    over = true
    cancel?.()
  }
}

/** Wire the two events to the runner: desktop and web main windows only (never the phone, never a popout). */
export function installFenceSnapshot(): () => void {
  if (container() === 'ios') return () => {}
  const jobs = new Map<string, { e: FenceSnapshotRequested; timer: number; cancel: (() => void) | null }>()
  let queue = IDLE
  const answer = (id: string, a: FenceSnapshotAnswer) => void useConnection.getState().api?.answer_fence_snapshot(id, a).catch(() => {})

  const run = (ids: string[]) => {
    for (const id of ids) {
      const job = jobs.get(id)
      if (job) job.cancel = render(job.e, (o) => end(id, o))
    }
  }
  /** Tear `id` down; answer when an outcome is given (not for `Resolved`). */
  const end = (id: string, o?: Outcome) => {
    const job = jobs.get(id)
    if (!job) return
    jobs.delete(id)
    window.clearTimeout(job.timer)
    job.cancel?.()
    const next = release(queue, id)
    queue = next.queue
    if (o) {
      const client = clientKind()
      if ('png' in o) answer(id, { kind: 'done', png: base64(o.png), ...pngSize(o.png), client })
      else answer(id, { kind: 'failed', reason: o.reason, client })
    }
    run(next.start)
  }

  const offs = [
    on('FenceSnapshotRequested', (e) => {
      if (jobs.has(e.id) || !useConnection.getState().api) return
      answer(e.id, { kind: 'started', client: clientKind() })
      const timer = window.setTimeout(() => end(e.id, { reason: `not drawn within ${FAIL_MS / 1000} s` }), FAIL_MS)
      jobs.set(e.id, { e, timer, cancel: null })
      const next = admit(queue, e.id)
      queue = next.queue
      run(next.start)
    }),
    on('FenceSnapshotResolved', (e) => end(e.id)),
  ]
  return () => {
    for (const off of offs) off()
    for (const id of [...jobs.keys()]) end(id)
  }
}
