// 101: pure helpers of the decision & review surface — the code-ref grammar of §3, the D11 "next item" rule, and the
// in-app route a pointer opens. No React, no fetches.

import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Pointer } from '@/rpc/bindings/Pointer'

export type CodeRef = { path: string; start: number; end: number }

/** `src/a.rs#L10-L24` → `{ path, start: 10, end: 24 }`; `#L10` → start = end = 10; `null` when the ref is not of that shape. */
export function parseCodeRef(ref: string): CodeRef | null {
  const m = /^(.+?)#L(\d+)(?:-L(\d+))?$/.exec(ref)
  if (!m) return null
  const start = Number(m[2])
  const end = m[3] ? Number(m[3]) : start
  return start >= 1 && end >= start ? { path: m[1] as string, start, end } : null
}

/**
 * D11: the item the pane shows once `id` resolved — the one after it in the list (newest first), else the one before
 * it, else `null` (the empty state). `null` too when `id` is not in the list (already gone: the list refetched first).
 */
export function nextAfter(items: readonly AttentionItem[], id: string): AttentionItem | null {
  const i = items.findIndex((x) => x.id === id)
  if (i < 0) return null
  return items[i + 1] ?? items[i - 1] ?? null
}

export type PointerRoute =
  | { to: '/artifacts/$callId'; params: { callId: string } }
  | { to: '/notes/$id'; params: { id: string } }
  | { to: '/knowledge/$'; params: { _splat: string } }
  | { to: '/c/$session'; params: { session: string } }
  | { to: '/meetings/$id'; params: { id: string } }
  | { to: '/tasks/$id'; params: { id: string } }

/** The in-app route a pointer opens in its viewer; `null` for `url` (external), `code` and `diff` (inline evidence). */
export function pointerRoute(p: Pointer): PointerRoute | null {
  switch (p.kind) {
    case 'artifact':
      return { to: '/artifacts/$callId', params: { callId: p.ref } }
    case 'note':
      return { to: '/notes/$id', params: { id: p.ref } }
    case 'knowledge':
      return { to: '/knowledge/$', params: { _splat: p.ref } }
    case 'session':
      return { to: '/c/$session', params: { session: p.ref } }
    case 'meeting':
      return { to: '/meetings/$id', params: { id: p.ref } }
    case 'task':
      return { to: '/tasks/$id', params: { id: p.ref } }
    default:
      return null
  }
}

/** The host of a `url` pointer for its row, or the ref itself when it does not parse. */
export function urlHost(ref: string): string {
  try {
    return new URL(ref).host
  } catch {
    return ref
  }
}
