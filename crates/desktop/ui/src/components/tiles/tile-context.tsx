import { createContext, type ReactNode, useContext } from 'react'

/** True inside a tile body: nested views suppress window-level chrome (SidebarLead, macOS traffic-light cornerPad, data-tauri-drag-region). */
export const InsideTileCtx = createContext(false)

export function InsideTileProvider({ children }: { children: ReactNode }) {
  return <InsideTileCtx.Provider value={true}>{children}</InsideTileCtx.Provider>
}

export function useInsideTile(): boolean {
  return useContext(InsideTileCtx)
}
