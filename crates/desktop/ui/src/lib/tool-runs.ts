// Settings › Chat › Collapse tool calls by default (70 TOOL-COLLAPSE): a client-only display preference, persisted
// like the theme, plus the in-memory set of runs the user has opened. Nothing here changes the session or its context.

import { create } from 'zustand'

const KEY = 'collapse-tool-calls'

type ToolRunStore = {
  /** Consecutive tool cards fold into one `ToolRunCard`; on unless the user opted out. */
  collapse: boolean
  setCollapse(on: boolean): void
  /** Runs the user expanded, keyed by the run's first call id. Not persisted. */
  open: Record<string, boolean>
  toggle(id: string): void
}

export const useToolRuns = create<ToolRunStore>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  collapse: globalThis.localStorage?.getItem(KEY) !== 'false',
  setCollapse(on) {
    localStorage.setItem(KEY, String(on))
    set({ collapse: on })
  },
  open: {},
  toggle(id) {
    set((s) => ({ open: { ...s.open, [id]: !s.open[id] } }))
  },
}))
