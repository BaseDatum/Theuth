// Settings › Chat › Collapse tool calls by default (70 TOOL-COLLAPSE): a client-only display preference, persisted
// like the theme, plus the in-memory set of runs and thinking blocks the user has opened. Nothing here changes the
// session or its context.

import { create } from 'zustand'

const KEY = 'collapse-tool-calls'

type ToolRunStore = {
  /** Consecutive tool cards fold into one `ToolRunCard`; on unless the user opted out. */
  collapse: boolean
  setCollapse(on: boolean): void
  /** Runs the user expanded, keyed by the run's first call id. Not persisted. */
  open: Record<string, boolean>
  toggle(id: string): void
  /** Thinking blocks the user opened, keyed by entry id, so a remount (fold, virtualizer slot move) cannot close
   * one (fix/thinking-open-while-streaming). Not persisted. */
  thinking: Record<string, boolean>
  /** Blocks opened while they still streamed: `foldRuns` never folds one into a tool run, so the block the reader is
   * watching stays where it is when the next tool call arrives. Closing keeps the pin. Same `Set` while unchanged. */
  pinned: ReadonlySet<string>
  toggleThinking(id: string, streaming: boolean): void
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
  thinking: {},
  pinned: new Set(),
  toggleThinking(id, streaming) {
    set((s) => {
      const open = !s.thinking[id]
      const pinned = open && streaming && !s.pinned.has(id) ? new Set(s.pinned).add(id) : s.pinned
      return { thinking: { ...s.thinking, [id]: open }, pinned }
    })
  },
}))
