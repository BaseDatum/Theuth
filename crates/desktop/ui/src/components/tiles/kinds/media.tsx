import { Original } from '@/components/chat/Attachments'
import { RemoveButton } from '@/components/chat/MessageAssistant'
import type { MediaProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { privateOwner } from '@/lib/private'

/** An attachment's image or PDF beside its chat (the desktop lightbox), Remove from context in its header. */
function MediaTile({ tile, first, close }: TileRender) {
  const { item, session, active } = tile.props as MediaProps
  const removed = !!item.occurrence?.removed
  return (
    <TileFrame
      first={first}
      onClose={close}
      title={item.name}
      meta={removed ? 'Removed from context · archive only' : undefined}
      controls={session && item.occurrence && !removed && <RemoveButton session={session} target={item.occurrence.target} active={active} />}
      className="items-center justify-center bg-background p-4"
    >
      <Original item={item} session={privateOwner(session)} fit />
    </TileFrame>
  )
}

export const mediaTile: TileKind = {
  title: (tile) => (tile.props as MediaProps).item.name,
  render: (p) => <MediaTile {...p} />,
}
