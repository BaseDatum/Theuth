// Generic tiles: Trellis views `tile:<id>` of type `tile:<kind>` (`lib/panes.ts`). A tile is a surface a chat opened
// beside itself (an artifact, the terminals, a skill, …); its kind (`components/tiles/registry`) draws it. Its payload is
// the view's params (`session`, `props`), so it is persisted with the layout. The id is `(kind, session, key)`, so
// opening the same thing again reuses the open tile with new props.

import { useMemo } from 'react'
import type { ViewRecord } from '@danfessler/trellis'
import { shallow } from 'zustand/shallow'
import { isMobileLayout } from '@/hooks/use-mobile'
import { chatSpec, closeLeaves, forgetClosed, groupView, isDraft, isFrame, isTile, makeTileId, ownerOf, placeView, rename, revealView, tileLeaf, tileOf, tileSpec, usePanes, type ViewSpec } from '@/lib/panes'

export type Tile = {
  id: string
  kind: string
  /** The chat (or draft leaf) that opened it: the tile goes beside it. */
  session: string
  props: unknown
}

/** Deterministic: the same kind, session and key name the same tile. */
export const tileId = makeTileId

/** The tile view `leaf` records, if it is one. */
export function tileFrom(leaf: string, record: ViewRecord | undefined): Tile | undefined {
  if (!isTile(leaf) || !record?.type.startsWith('tile:')) return undefined
  return { id: tileOf(leaf), kind: record.type.slice('tile:'.length), session: ownerOf(leaf, record), props: record.params?.props }
}

/** Tile `id` while it is laid out (reactive). */
export function useTile(id: string): Tile | undefined {
  const leaf = tileLeaf(id)
  const record = usePanes((s) => (s.leaves.includes(leaf) ? s.views[leaf] : undefined))
  return useMemo(() => tileFrom(leaf, record), [leaf, record])
}

/** Every laid-out tile. */
export function tiles(): Tile[] {
  const { leaves, views } = usePanes.getState()
  return leaves.flatMap((l) => tileFrom(l, views[l]) ?? [])
}

/** Tile `id` shows `props` now (a terminal picked in its header). */
export function setTileProps(id: string, props: unknown): void {
  usePanes.getState().ws?.setParams(tileLeaf(id), { props })
}

/**
 * Open (or retarget) tile `kind` for the chat shown in leaf `host` — the leaf a trigger sits in (`useTileHost`); a tile
 * leaf counts as its owner's chat. An open tile takes the new props and becomes its panel's active tab (Trellis reveals
 * a view by its id: same key, same view); with `refresh` it only takes the props, and nothing opens. A new one joins as
 * the active tab the panel already holding a tile of its kind — of several, the one used last (`groupView`). Else it
 * joins the selected pane's panel as its active tab when that is a frame (a tile or web view) other than `host`: the
 * frame stays open behind it, never replaced; otherwise it goes below the owner's last open frame, else right of the
 * owner's pane, else right of `host`. Returns the tile id, or `null` when nothing tiles here or neither the owner nor
 * `host` has a pane (nothing is opened).
 */
export function openTile(o: { kind: string; host: string; props: unknown; key?: string; refresh?: boolean }): string | null {
  const { ws, leaves, views, selected } = usePanes.getState()
  const session = isTile(o.host) ? ownerOf(o.host, views[o.host]) || o.host : o.host
  const id = tileId(o.kind, session, o.key)
  const leaf = tileLeaf(id)
  if (!ws) return null
  if (leaves.includes(leaf)) {
    if (!shallow(views[leaf]?.params?.props, o.props)) setTileProps(id, o.props)
    if (!o.refresh) revealView(leaf)
    return id
  }
  if (o.refresh || (!leaves.includes(session) && !leaves.includes(o.host))) return null
  const spec = tileSpec(o.kind, session, o.props)
  if (groupView(leaf, spec)) return id
  if (selected && selected !== o.host && isFrame(selected) && leaves.includes(selected)) placeView(selected, leaf, 'tab', spec)
  else {
    const anchor = leaves.filter((l) => isFrame(l) && ownerOf(l, views[l]) === session).at(-1)
    if (anchor) placeView(anchor, leaf, 'bottom', spec)
    else placeView(leaves.includes(session) ? session : o.host, leaf, 'right', spec)
  }
  return id
}

/** Close tile `id` for good. */
export function closeTile(id: string): void {
  usePanes.getState().close(tileLeaf(id))
}

/** The open tile of `kind` for `session` (and `key`), if it is laid out. */
export function openTileOf(kind: string, session: string, key?: string): Tile | undefined {
  const leaf = tileLeaf(tileId(kind, session, key))
  const { leaves, views } = usePanes.getState()
  return leaves.includes(leaf) ? tileFrom(leaf, views[leaf]) : undefined
}

/** Reactive `openTileOf(…) !== undefined`: a toggle's pressed state. */
export function useTileOpen(kind: string, session: string, key?: string): boolean {
  const leaf = tileLeaf(tileId(kind, session, key))
  return usePanes((s) => s.leaves.includes(leaf))
}

/** A deleted chat's tiles go with it, not into Recently closed. A tile never closes because its chat's pane closed or
 * was replaced. */
export function closeSessionTiles(session: string): void {
  closeLeaves(tiles().filter((t) => t.session === session).map((t) => tileLeaf(t.id)), false)
  forgetClosed((c) => c.spec.params.session === session)
}

/** Every tile closed: the engine they belong to was left, or the layout stopped tiling (below 768 px). */
export function closeAllTiles(): void {
  const { leaves } = usePanes.getState()
  closeLeaves(leaves.filter((l) => isTile(l)), false)
}

/**
 * Draft `draft` became session `session` (its first send): its pane becomes the session's, and every tile the draft
 * opened is retargeted to the session in the same place (the Context tile becomes the session's Context tile); none
 * stays keyed to the dead draft id. A draft tile whose session twin is already open closes in its favour. When the
 * session already has a pane elsewhere, it moves into the draft's.
 */
export function promoteDraft(draft: string, session: string): void {
  if (!isDraft(draft)) return
  const { leaves } = usePanes.getState()
  const map: Record<string, { leaf: string; spec: ViewSpec }> = {}
  const clashes: string[] = []
  for (const t of tiles()) {
    if (t.session !== draft) continue
    const key = decodeURIComponent(t.id.split(':')[2] ?? '')
    const leaf = tileLeaf(tileId(t.kind, session, key))
    if (leaves.includes(leaf)) clashes.push(tileLeaf(t.id))
    else map[tileLeaf(t.id)] = { leaf, spec: tileSpec(t.kind, session, t.props) }
  }
  map[draft] = { leaf: session, spec: chatSpec(session) }
  rename(map)
  closeLeaves(clashes, false)
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
