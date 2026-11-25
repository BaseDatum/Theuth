// 221 §9.4: an agent's call needs a locked Bitwarden connection. `ExternalUnlockRequested` adds a card,
// `ExternalUnlockResolved` (answered here, on another app, or timed out) drops it. Not replicated: the unlock happens on
// the owning engine, so only the active engine's events feed it. Events are lost across a reconnect, a `Lagged` gap and
// an engine switch, so `App` clears the store on a switch and `replace`s it with the engine's `list_unlock_requests`
// whenever the event stream (re)starts; `UNLOCK_TTL_MS` drops a card whose `Resolved` was missed in between.

import { create } from 'zustand'
import type { SecretsEvent } from '@/rpc/bindings/event.rpc_2estream.output'

/** One waiting unlock: the `ExternalUnlockRequested` fields, and a `list_unlock_requests` row. */
export type UnlockRequestView = Omit<Extract<SecretsEvent, { type: 'ExternalUnlockRequested' }>, 'type'>

/** The engine's default human wait (`secrets.request_timeout_secs`, 1800 s) and a minute of slack. */
export const UNLOCK_TTL_MS = 31 * 60_000

const expiries = new Map<string, ReturnType<typeof setTimeout>>()

function forget(id: string): void {
  clearTimeout(expiries.get(id))
  expiries.delete(id)
}

function expire(id: string): void {
  forget(id)
  expiries.set(
    id,
    setTimeout(() => useUnlockRequests.getState().resolve(id), UNLOCK_TTL_MS),
  )
}

export const useUnlockRequests = create<{
  pending: UnlockRequestView[]
  add(request: UnlockRequestView): void
  resolve(id: string): void
  /** The engine's snapshot replaces every card. */
  replace(rows: readonly UnlockRequestView[]): void
  clear(): void
}>((set) => ({
  pending: [],
  add: ({ id, conn, backend, account, reason }) =>
    set((s) => {
      if (s.pending.some((p) => p.id === id)) return s
      expire(id)
      return { pending: [...s.pending, { id, conn, backend, account, reason }] }
    }),
  resolve: (id) => {
    forget(id)
    set((s) => ({ pending: s.pending.filter((p) => p.id !== id) }))
  },
  replace: (rows) => {
    for (const id of [...expiries.keys()]) forget(id)
    for (const r of rows) expire(r.id)
    set({ pending: rows.map(({ id, conn, backend, account, reason }) => ({ id, conn, backend, account, reason })) })
  },
  clear: () => {
    for (const id of [...expiries.keys()]) forget(id)
    set({ pending: [] })
  },
}))
