// Generic tiles: the payload of a `tile:<id>` pane leaf (`lib/panes.ts`), in memory like the pane tree. A tile is a
// surface a chat opened beside itself (an artifact, the terminals, a skill, …); its kind (`components/tiles/registry`)
// draws it. The id is `(kind, session, key)`, so opening the same thing again reuses the open tile with new props.

import { create } from 'zustand'
import { shallow } from 'zustand/shallow'
import { isMobileLayout } from '@/hooks/use-mobile'
import { isFrame, isTile, leaves, type Node, tileLeaf, tileOf, usePanes } from '@/lib/panes'
import { isWeb, useWebViews, webId } from '@/lib/web-views'

export type Tile = {
  id: string
  kind: string
  /** The chat (or draft leaf) that opened it: the tile goes beside it and closes with it. */
  session: string
  props: unknown
}

type TilesStore = {
  tiles: Record<string, Tile>
  put(tile: Tile): void
  remove(id: string): void
}

export const useTiles = create<TilesStore>()((set) => ({
  tiles: {},
  // The same payload again (a media or diagram tile refreshed with what it shows) keeps the object: no re-render.
  put: (tile) =>
    set((s) => {
      const old = s.tiles[tile.id]
      if (old && old.kind === tile.kind && old.session === tile.session && shallow(old.props, tile.props)) return s
      return { tiles: { ...s.tiles, [tile.id]: tile } }
    }),
  remove: (id) =>
    set((s) => {
      if (!(id in s.tiles)) return s
      const { [id]: _, ...rest } = s.tiles
      return { tiles: rest }
    }),
}))

/** Deterministic: the same kind, session and key name the same tile. */
export const tileId = (kind: string, session: string, key = ''): string =>
  [kind, session, key].map(encodeURIComponent).join(':')

/** The session a leaf belongs to: a tile's owner, else the leaf itself. */
const ownerOf = (leaf: string): string => (isTile(leaf) ? (useTiles.getState().tiles[tileOf(leaf)]?.session ?? leaf) : leaf)

/**
 * Open (or retarget) tile `kind` for the chat shown in leaf `host` — the leaf a trigger sits in (`useTileHost`); a tile
 * leaf counts as its owner's chat. An open tile only takes the new props. A new one takes over the selected pane when
 * that is a frame (a tile or web view) other than `host` — in place, the old one disposed, like a chat picked into a
 * picked pane; otherwise it goes below the owner's last open tile (web frames included), else right of the owner's
 * pane (the single chat on screen counts as one), else right of `host`. Returns the tile id, or `null` when neither the
 * owner nor `host` has a pane here (nothing is recorded).
 */
export function openTile(o: { kind: string; host: string; props: unknown; key?: string }): string | null {
  const session = ownerOf(o.host)
  const id = tileId(o.kind, session, o.key)
  const leaf = tileLeaf(id)
  const { root, place, selected } = usePanes.getState()
  const all = root ? leaves(root) : []
  const { tiles, put } = useTiles.getState()
  const open = all.includes(leaf)
  if (!open && root && !all.includes(session) && !all.includes(o.host)) return null
  put({ id, kind: o.kind, session, props: o.props })
  if (open) return id
  // `place` selects the newcomer, closes a web view it replaced; the replaced tile's payload goes with its leaf.
  if (selected && selected !== o.host && isFrame(selected) && all.includes(selected)) {
    place(selected, leaf, 'center')
    return id
  }
  const views = useWebViews.getState().views
  const mine = all.filter((l) => (isTile(l) ? tiles[tileOf(l)]?.session === session : isWeb(l) && views[webId(l)]?.session === session))
  const anchor = mine.at(-1)
  if (anchor) place(anchor, leaf, 'bottom')
  else place(!root || all.includes(session) ? session : o.host, leaf, 'right')
  return id
}

/** Close tile `id` for good: its leaf and its payload. */
export function closeTile(id: string): void {
  usePanes.getState().close(tileLeaf(id))
  useTiles.getState().remove(id)
}

/** The open tile of `kind` for `session` (and `key`), if its leaf is in the tree. */
export function openTileOf(kind: string, session: string, key?: string): Tile | undefined {
  const id = tileId(kind, session, key)
  const { root } = usePanes.getState()
  return root && leaves(root).includes(tileLeaf(id)) ? useTiles.getState().tiles[id] : undefined
}

/** Reactive `openTileOf(…) !== undefined`: a toggle's pressed state. */
export function useTileOpen(kind: string, session: string, key?: string): boolean {
  const leaf = tileLeaf(tileId(kind, session, key))
  return usePanes((s) => s.root !== null && leaves(s.root).includes(leaf))
}

/** A deleted chat's tiles go with it. */
export function closeSessionTiles(session: string): void {
  for (const t of Object.values(useTiles.getState().tiles)) if (t.session === session) closeTile(t.id)
}

/** Every tile closed, leaves and payloads: the engine they belong to was left, or the layout stopped tiling. */
export function closeAllTiles(): void {
  const { root } = usePanes.getState()
  for (const l of root ? leaves(root) : []) if (isTile(l)) closeTile(tileOf(l))
  useTiles.setState({ tiles: {} })
}

/**
 * Payloads live only while their leaf is in the tree: a tile closed, taken over or replaced by a sidebar pick leaves
 * nothing behind. A tile never closes because its chat left the tree — it holds its own session id and keeps working.
 */
function reconcile(root: Node | null): void {
  const all = root ? leaves(root) : []
  const { tiles } = useTiles.getState()
  const ids = Object.keys(tiles)
  const kept = ids.filter((id) => all.includes(tileLeaf(id)))
  if (kept.length !== ids.length) useTiles.setState({ tiles: Object.fromEntries(kept.map((id) => [id, tiles[id] as Tile])) })
}
usePanes.subscribe((s, prev) => s.root !== prev.root && reconcile(s.root))

/**
 * Draft `draft` became session `session` (its first send): its leaf becomes the session's, and every tile the draft
 * opened is retargeted to the session in the same pane (the Context tile becomes the session's Context tile); none stays
 * keyed to the dead draft id. A draft tile whose session twin is already open closes in its favour. When the session
 * already has a pane elsewhere, the draft's pane takes it (`place`), as before.
 */
export function promoteDraft(draft: string, session: string): void {
  const { root, place, rename } = usePanes.getState()
  const all = root ? leaves(root) : []
  const map: Record<string, string> = {}
  const clashes: string[] = []
  for (const t of Object.values(useTiles.getState().tiles)) {
    if (t.session !== draft) continue
    const key = decodeURIComponent(t.id.split(':')[2] ?? '')
    const id = tileId(t.kind, session, key)
    if (all.includes(tileLeaf(id))) clashes.push(t.id)
    else {
      useTiles.getState().put({ ...t, id, session })
      map[tileLeaf(t.id)] = tileLeaf(id)
    }
  }
  // Relabel first: the retargeted payloads must find their leaves in the tree before anything else changes it.
  if (!all.includes(session)) map[draft] = session
  rename(map)
  if (all.includes(session)) place(draft, session, 'center')
  for (const id of clashes) closeTile(id)
}

/**
 * Outside React (an async opener, a window event): `session`'s chat is on screen in a pane and the layout tiles, so
 * its surfaces open as tiles; otherwise they keep their phone sheet / dialog.
 */
export function tileable(session: string): boolean {
  if (isMobileLayout()) return false
  for (const el of document.querySelectorAll<HTMLElement>('[data-pane]')) if (el.dataset.pane === session) return true
  return false
}
