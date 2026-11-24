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
  sticky: boolean
  expiresAt: number | null
}

export type NoticeInput = {
  id: string | number
  kind: NoticeKind
  text: string
  description?: string
  action?: NoticeAction
  cancel?: NoticeAction
  duration?: number
}

export const HISTORY_CAP = 50
export const DEDUPE_MS = 2_000
const LIFETIME: Record<NoticeKind, number> = { info: 4_000, done: 6_000, warn: 8_000, error: Infinity, progress: Infinity }

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
  /** Called by the host's timer: drops `current` once its lifetime is over. */
  expire(now?: number): void
}

const lifetime = (n: NoticeInput, at: number): Pick<Notice, 'sticky' | 'expiresAt'> => {
  const ms = n.duration ?? (n.action ? Infinity : LIFETIME[n.kind])
  return Number.isFinite(ms) ? { sticky: false, expiresAt: at + ms } : { sticky: true, expiresAt: null }
}

export const useNotices = create<NoticesState>()((set, get) => ({
  current: null,
  history: [],
  unreadErrors: 0,
  push(n) {
    const at = Date.now()
    const { history, unreadErrors } = get()
    const same = history.find((h) => h.id === n.id)
    const twin = same ? undefined : history.find((h) => h.kind === n.kind && h.text === n.text && at - h.at < DEDUPE_MS)
    const prev = same ?? twin
    const notice: Notice = {
      id: twin ? twin.id : n.id,
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
    set({
      current: current?.id === target.id ? null : current,
      history: target.kind === 'progress' ? history.filter((h) => h.id !== target.id) : history,
    })
  },
  clear: () => set({ current: null, history: [], unreadErrors: 0 }),
  markRead: () => set({ unreadErrors: 0 }),
  expire(now = Date.now()) {
    const { current } = get()
    if (current?.expiresAt != null && now >= current.expiresAt) set({ current: null })
  },
}))
