// Settings › Chat › Collapse tool calls by default (70 TOOL-COLLAPSE): a client-only display preference, persisted
// like the theme, plus the in-memory set of runs and thinking blocks the user has opened. Nothing here changes the
// session or its context.

import { useSyncExternalStore } from 'react'
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
  /** 171F B13: open state of any disclosure (`disclosureKey`), so a transcript re-render or remount cannot close
   * what the reader expanded. Not persisted. */
  disclosed: Record<string, boolean>
  setDisclosed(key: string, open: boolean): void
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
  disclosed: {},
  setDisclosed(key, open) {
    set((s) => ({ disclosed: { ...s.disclosed, [key]: open } }))
  },
}))

/** A `disclosed` key: the defined parts joined with `|` (`disclosureKey('s1', undefined, 3, 'x') === 's1|3|x'`). */
export function disclosureKey(...parts: Array<string | number | null | undefined>): string {
  return parts.filter((p) => p != null).join('|')
}

/** `useState` for a disclosure whose open state lives in `disclosed[key]` (`initial` while unset). Reads through
 * `useSyncExternalStore` with the live state as server snapshot, so a preset store shows in `renderToStaticMarkup`. */
export function useDisclosure(key: string, initial = false): [boolean, (next: boolean | ((open: boolean) => boolean)) => void] {
  const read = () => useToolRuns.getState().disclosed[key] ?? initial
  const open = useSyncExternalStore(useToolRuns.subscribe, read, read)
  const set = (next: boolean | ((open: boolean) => boolean)) => useToolRuns.getState().setDisclosed(key, typeof next === 'function' ? next(read()) : next)
  return [open, set]
}
