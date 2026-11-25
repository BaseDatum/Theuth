// 217 §2: the one ledger behind the status bar and the phone line. Global, not per engine: a notice outlives an engine
// switch. Pure state; the host component (`components/status/`) runs the single timer that calls `expire`.

import { create } from 'zustand'

export type NoticeKind = 'info' | 'done' | 'warn' | 'error' | 'progress'
export type NoticeAction = { label: string; onClick: () => void }

export type Notice = {
  id: string | number
  kind: NoticeKind
  text: string
  description?: string
  action?: NoticeAction
  cancel?: NoticeAction
  at: number
  /** Held for the user (an action, `duration: Infinity`, progress): it carries a ×, in the slot and in history. */
  sticky: boolean
  /** When it leaves the slot. Every notice has one (§2: nothing lingers); history keeps it after. */
  expiresAt: number
  /** Dismissed or acted on: it stays in history, without its ×. */
  dismissed?: boolean
}

export type NoticeInput = {
  /** The caller's id; same id replaces in place, and an id'd call is never deduped. */
  id?: string | number
  kind: NoticeKind
  text: string
  description?: string
  action?: NoticeAction
  cancel?: NoticeAction
  duration?: number
}

export const HISTORY_CAP = 50
export const DEDUPE_MS = 2_000
const LIFETIME: Record<Exclude<NoticeKind, 'progress'>, number> = { info: 4_000, done: 6_000, warn: 8_000, error: 8_000 }
/** The longest any notice holds the slot: one with an action or `duration: Infinity`. */
export const HELD_MS = 10_000
/** A progress notice is current state: it holds the slot while updated, and leaves after this long without an update. */
export const PROGRESS_IDLE_MS = 30_000

type NoticesState = {
  current: Notice | null
  /** Newest first, at most `HISTORY_CAP`. */
  history: Notice[]
  /** Errors pushed since the history was last opened. */
  unreadErrors: number
  push(n: NoticeInput): string | number
  /** No id: the one in the slot. A dismissed progress leaves history too, since it never finished. */
  dismiss(id?: string | number): void
  clear(): void
  markRead(): void
  /** Called by the host's timer: once `current`'s lifetime is over, the slot empties. */
  expire(now?: number): void
}

const lifetime = (n: NoticeInput, at: number): Pick<Notice, 'sticky' | 'expiresAt'> => {
  const sticky = n.kind === 'progress' || n.action !== undefined || n.duration === Infinity
  const ms = n.kind === 'progress' ? PROGRESS_IDLE_MS : Math.min(n.duration ?? (n.action ? HELD_MS : LIFETIME[n.kind]), HELD_MS)
  return { sticky, expiresAt: at + ms }
}

let seq = 0

export const useNotices = create<NoticesState>()((set, get) => ({
  current: null,
  history: [],
  unreadErrors: 0,
  push(n) {
    const at = Date.now()
    const { history, unreadErrors } = get()
    const same = n.id === undefined ? undefined : history.find((h) => h.id === n.id)
    const twin =
      n.id === undefined
        ? history.find(
            (h) =>
              h.kind === n.kind &&
              h.text === n.text &&
              h.description === n.description &&
              h.action?.label === n.action?.label &&
              at - h.at < DEDUPE_MS,
          )
        : undefined
    const prev = same ?? twin
    const notice: Notice = {
      id: prev?.id ?? n.id ?? `n${++seq}`,
      kind: n.kind,
      text: n.text,
      description: n.description,
      action: n.action,
      cancel: n.cancel,
      at,
      ...lifetime(n, at),
    }
    const rest = prev ? history.filter((h) => h !== prev) : history
    const fresh = n.kind === 'error' && prev?.kind !== 'error'
    set({
      current: notice,
      history: [notice, ...rest].slice(0, HISTORY_CAP),
      unreadErrors: unreadErrors + (fresh ? 1 : 0),
    })
    return notice.id
  },
  dismiss(id) {
    const { current, history } = get()
    const target = id === undefined ? current : (history.find((h) => h.id === id) ?? null)
    if (!target) return
    const next =
      target.kind === 'progress'
        ? history.filter((h) => h.id !== target.id)
        : history.map((h) => (h.id === target.id ? { ...h, dismissed: true } : h))
    set({ history: next, current: current?.id === target.id ? null : current })
  },
  clear: () => set({ current: null, history: [], unreadErrors: 0 }),
  markRead: () => set({ unreadErrors: 0 }),
  expire(now = Date.now()) {
    const { current } = get()
    if (current && now >= current.expiresAt) set({ current: null })
  },
}))
