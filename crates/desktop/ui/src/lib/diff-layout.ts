// A `show_diff` diff's layout (Unified | Split): the user's explicit choice, one global preference in localStorage
// shared by every diff tile, card and popped window; until they choose, by width (Split when wide enough).

import { create } from 'zustand'

export type DiffLayout = 'unified' | 'split'

const KEY = 'diff.layout'

/** Wider than this (px, the diff's own box) a diff with no remembered choice opens side by side. */
export const SPLIT_MIN_PX = 900

/** The layout a diff `width` px wide draws in: the remembered `chosen` one, else Split past `SPLIT_MIN_PX`. */
export const diffLayout = (chosen: DiffLayout | null, width: number): DiffLayout => chosen ?? (width > SPLIT_MIN_PX ? 'split' : 'unified')

const read = (raw: string | null | undefined): DiffLayout | null => (raw === 'unified' || raw === 'split' ? raw : null)

export const useDiffLayoutChoice = create<{ chosen: DiffLayout | null; choose(layout: DiffLayout): void }>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  chosen: read(globalThis.localStorage?.getItem(KEY)),
  choose(layout) {
    localStorage.setItem(KEY, layout)
    set({ chosen: layout })
  },
}))

// Another window (a popped diff) chose: follow it here too.
globalThis.addEventListener?.('storage', (e: StorageEvent) => {
  if (e.key === KEY) useDiffLayoutChoice.setState({ chosen: read(e.newValue) })
})
