import { createContext, type ReactNode, useContext } from 'react'

/** True inside a tile body: nested views suppress window-level chrome (SidebarLead, macOS traffic-light cornerPad, data-tauri-drag-region). */
export const InsideTileCtx = createContext(false)

/** The enclosing tile's Close (`TileFrame`'s `onClose`); `null` outside a tile, or where the host has no close (a whiteboard card). */
const TileCloseCtx = createContext<(() => void) | null>(null)

export function InsideTileProvider({ children, onClose }: { children: ReactNode; onClose?: () => void }) {
  return (
    <InsideTileCtx.Provider value={true}>
      <TileCloseCtx.Provider value={onClose ?? null}>{children}</TileCloseCtx.Provider>
    </InsideTileCtx.Provider>
  )
}

export function useInsideTile(): boolean {
  return useContext(InsideTileCtx)
}

/** Close the tile this subtree renders in (a view inside a `view` tile leaving for good), if it has one. */
export function useTileClose(): (() => void) | null {
  return useContext(TileCloseCtx)
}
