// 101: pure helpers of the decision & review surface — the code-ref grammar of §3, the D11 "next item" rule, and the
// in-app route a pointer opens. 104: the grouped rail and the context line's clamp. No React, no fetches.

import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Pointer } from '@/rpc/bindings/Pointer'
import type { HistoryEntry } from '@/rpc/extra'

type Kind = AttentionItem['kind']

/** 104 A4: the rail's groups in Sky's order; a kind outside the table never reaches the rail (every kind is listed). */
export const GROUPS: readonly { key: string; label: string; kinds: readonly Kind[] }[] = [
  { key: 'decisions', label: 'Decisions', kinds: ['decision'] },
  { key: 'reviews', label: 'Reviews', kinds: ['review'] },
  { key: 'questions', label: 'Questions', kinds: ['ask'] },
  { key: 'approvals', label: 'Approvals', kinds: ['approval'] },
  { key: 'shown', label: 'Shown to you', kinds: ['show'] },
  { key: 'hand', label: 'Needs a hand', kinds: ['handoff', 'secret', 'resume'] },
  { key: 'agents', label: 'Agents', kinds: ['hire', 'budget'] },
  { key: 'blocked', label: 'Blocked', kinds: ['blocked'] },
  { key: 'mesh', label: 'Mesh', kinds: ['node'] },
  // 126 §10: "answered for you" rows (undo during grace) and the L3 daily digest.
  { key: 'double', label: 'Answered by double', kinds: ['double'] },
  /** 130 E3: finished turns — receipts, nothing to decide. */
  { key: 'done', label: 'Done', kinds: ['finished'] },
]

export type Group = { key: string; label: string; items: AttentionItem[] }

/**
 * The inbox grouped for the rail: Urgent first (every `priority === 'urgent'` item, taken out of its kind group), then
 * the kinds in `GROUPS` order; empty groups omitted. 132 D4: inside every group the ENGINE's order stands (rank desc,
 * waiting-since asc, newest) — no client re-sort by `at_ms`.
 */
export function groupItems(items: readonly AttentionItem[]): Group[] {
  const urgent = items.filter((i) => i.priority === 'urgent')
  const rest = items.filter((i) => i.priority !== 'urgent')
  const groups = GROUPS.flatMap(({ key, label, kinds }) => {
    const mine = rest.filter((i) => kinds.includes(i.kind))
    return mine.length > 0 ? [{ key, label, items: mine }] : []
  })
  return urgent.length > 0 ? [{ key: 'urgent', label: 'Urgent', items: urgent }, ...groups] : groups
}

export const CONTEXT_MAX = 600

/** `text` cut to `max` chars on a word boundary when one is near, with "…"; shorter text verbatim. */
export function clampText(text: string, max = CONTEXT_MAX): string {
  const t = text.trim()
  if (t.length <= max) return t
  const cut = t.slice(0, max)
  const space = cut.lastIndexOf(' ')
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`
}

/**
 * 104 A7: the agent's last assistant text stored before `beforeMs` — a row without a timestamp counts as before (older
 * engines stored none); `null` when the page has no such row.
 */
export function lastAssistantBefore(history: readonly HistoryEntry[], beforeMs: number): string | null {
  for (let i = history.length - 1; i >= 0; i--) {
    const h = history[i]
    if (!h || h.role !== 'assistant' || !h.text.trim()) continue
    const at = h.context?.at ? Date.parse(h.context.at) : NaN
    if (Number.isNaN(at) || at <= beforeMs) return h.text
  }
  return null
}

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
