import type { ReactNode } from 'react'
import type { Tile } from '@/lib/tiles'

/** What a kind's `render` gets: its tile (`props` are the kind's own, cast by the kind), whether it is the top-left
 * leaf (`TileFrame`'s `first`), and `close` (the tile for good). */
export type TileRender = { tile: Tile; first: boolean; close(): void }

/** One tile kind (`registry.tsx`): `title` names it where its body is not drawn (the drag ghost, a missing tile);
 * `render` draws `TileFrame` with the kind's header controls around its body. */
export type TileKind = {
  title(tile: Tile): string
  render(p: TileRender): ReactNode
}
