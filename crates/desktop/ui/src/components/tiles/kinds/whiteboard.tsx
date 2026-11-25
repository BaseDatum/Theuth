import { popOutTile, type WhiteboardProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { WhiteboardView } from '@/components/whiteboard/WhiteboardView'

/**
 * A `show_whiteboard` document beside its chat (218 §8.8): the whiteboard view embedded, bound to the tile's session.
 * Open in new window takes the tile alone into the generic tile window, drawn by this kind over the same props.
 */
function Whiteboard({ tile, first, close }: TileRender) {
  const { doc } = tile.props as WhiteboardProps
  return (
    <TileFrame title={whiteboardTile.title(tile)} first={first} onClose={close} popOut={popOutTile(tile)}>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <WhiteboardView key={doc} doc={doc} session={tile.session} embedded />
      </div>
    </TileFrame>
  )
}

export const whiteboardTile: TileKind = {
  title: (tile) => (tile.props as WhiteboardProps).title || 'Whiteboard',
  render: (p) => <Whiteboard {...p} />,
}
