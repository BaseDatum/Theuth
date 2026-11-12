// The chat's Tools… sheet (⋯ menu, `/tools`, `/mcp`): which chat has it open. The sheet itself is `ToolsSheet.tsx`.

import { create } from 'zustand'

type State = {
  /** The chat whose sheet is open; keyed by chat, so a split pane opens only its own. */
  session: string | null
  show(session: string): void
  hide(): void
}

export const useToolsSheet = create<State>((set) => ({
  session: null,
  show: (session) => set({ session }),
  hide: () => set({ session: null }),
}))
