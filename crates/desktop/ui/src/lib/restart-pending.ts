import { create } from 'zustand'

/** 188: one saved change that takes effect when theuth restarts. `key` identifies the row (`double.enabled`). */
export type PendingRestart = { key: string; label: string }

type State = {
  pending: PendingRestart[]
  /** Mark a saved change as waiting for a restart; a second mark for the same key replaces the label. */
  mark(key: string, label: string): void
  /** The change was undone, or the engine now runs with it. */
  clear(key: string): void
  /** After a restart: nothing is pending any more. */
  reset(): void
}

/** 188: Settings shows one banner for every change that waits for a restart, with Restart theuth where the app owns the engine. */
export const useRestartPending = create<State>((set) => ({
  pending: [],
  mark: (key, label) => set((s) => ({ pending: [...s.pending.filter((p) => p.key !== key), { key, label }] })),
  clear: (key) => set((s) => ({ pending: s.pending.filter((p) => p.key !== key) })),
  reset: () => set({ pending: [] }),
}))
