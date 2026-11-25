// 221 §9.4: an agent's call needs a locked Bitwarden connection. `ExternalUnlockRequested` adds a card,
// `ExternalUnlockResolved` (answered here, on another app, or timed out) drops it. Not replicated: the unlock happens on
// the owning engine, so only this engine's events feed it.

import { create } from 'zustand'
import type { SecretsEvent } from '@/rpc/bindings/event.rpc_2estream.output'

export type UnlockRequested = Extract<SecretsEvent, { type: 'ExternalUnlockRequested' }>

export const useUnlockRequests = create<{
  pending: UnlockRequested[]
  add(request: UnlockRequested): void
  resolve(id: string): void
}>((set) => ({
  pending: [],
  add: (request) => set((s) => (s.pending.some((p) => p.id === request.id) ? s : { pending: [...s.pending, request] })),
  resolve: (id) => set((s) => ({ pending: s.pending.filter((p) => p.id !== id) })),
}))
