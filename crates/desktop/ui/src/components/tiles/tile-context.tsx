import { createContext, type ReactNode, useContext } from 'react'

/** True inside a tile body: nested views suppress window-level chrome (SidebarLead, macOS traffic-light cornerPad, data-tauri-drag-region). */
export const InsideTileCtx = createContext(false)

/** The enclosing tile's Close (`TileFrame`'s `onClose`); `null` outside a tile, or where the host has no close (a whiteboard card). */
const TileCloseCtx = createContext<(() => void) | null>(null)

/** Callback to sync the enclosing tile's display title (e.g. a Deck's live document title inside `ViewTile`). */
const TileTitleCtx = createContext<((title: string) => void) | null>(null)

export function InsideTileProvider({
  children,
  onClose,
  onTitle,
}: {
  children: ReactNode
  onClose?: () => void
  onTitle?: (title: string) => void
}) {
  return (
    <InsideTileCtx.Provider value={true}>
      <TileCloseCtx.Provider value={onClose ?? null}>
        <TileTitleCtx.Provider value={onTitle ?? null}>{children}</TileTitleCtx.Provider>
      </TileCloseCtx.Provider>
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

/** Update the enclosing tile's title when the nested view resolves a more specific subject title. */
export function useSetTileTitle(): ((title: string) => void) | null {
  return useContext(TileTitleCtx)
}
