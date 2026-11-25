// The chat's Tools… sheet (⋯ menu, `/tools`, `/mcp`): which chat has it open, and on which tab. The sheet itself is
// `ToolsSheet.tsx`. The composer's popover shares `tab`, so the sheet opens on the tab the popover showed.

import { create } from 'zustand'

export type ToolsTab = 'categories' | 'builtins' | 'mcp'

type State = {
  /** The chat whose sheet is open; keyed by chat, so a split pane opens only its own. */
  session: string | null
  tab: ToolsTab
  setTab(tab: ToolsTab): void
  /** Open `session`'s sheet, on `tab` if given, else on the current one. */
  show(session: string, tab?: ToolsTab): void
  hide(): void
}

export const useToolsSheet = create<State>((set) => ({
  session: null,
  tab: 'categories',
  setTab: (tab) => set({ tab }),
  show: (session, tab) => set(tab ? { session, tab } : { session }),
  hide: () => set({ session: null }),
}))
