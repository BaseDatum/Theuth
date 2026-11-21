// Settings › General › Appearance › Sidebar: how long a chat that stopped working stays at the bottom of the
// sidebar's Active section before it files back into its place. A client-only display preference, persisted
// like the theme; 0 turns lingering off.

import { create } from 'zustand'

const KEY = 'sidebar-active-linger-hours'

/** Hours a recently active chat stays in Active unless the user picked otherwise. */
export const LINGER_DEFAULT_HOURS = 4
/** Upper bound of the setting: one week. */
export const LINGER_MAX_HOURS = 168

/** The stored value as hours: a number in `[0, LINGER_MAX_HOURS]`, else the default. */
export function lingerHours(raw: string | null | undefined): number {
  if (raw == null || raw.trim() === '') return LINGER_DEFAULT_HOURS
  const n = Number(raw)
  return Number.isFinite(n) && n >= 0 ? Math.min(n, LINGER_MAX_HOURS) : LINGER_DEFAULT_HOURS
}

type LingerStore = {
  /** Linger window in hours; 0 = off. */
  hours: number
  setHours(hours: number): void
}

export const useLinger = create<LingerStore>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  hours: lingerHours(globalThis.localStorage?.getItem(KEY)),
  setHours(hours) {
    const v = lingerHours(String(hours))
    localStorage.setItem(KEY, String(v))
    set({ hours: v })
  },
}))
