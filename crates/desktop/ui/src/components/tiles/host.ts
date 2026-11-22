import { createContext, useContext } from 'react'

/** The pane leaf a subtree renders in (`Panes`' `Leaf`), only while the layout tiles; `null` on phones and outside `Panes`. */
export const TileHost = createContext<string | null>(null)

/**
 * The leaf a trigger sits in when its surface should open as a tile beside it (`openTile({ host, … })`); `null` means
 * keep the phone sheet / dialog (a phone, a talk view, a search preview, a meeting's embedded chat).
 */
export const useTileHost = (): string | null => useContext(TileHost)
