import { BrowserScreen } from '@/components/chat/BrowserLive'
import { browserRequirements } from '@/components/chat/requirements'
import { popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'
import { Capability } from '@/rpc/capabilities'

/** The session's live headless browser beside its chat (the desktop form of `BrowserLive`'s dialog); one per chat. */
export const browserTile: TileKind = {
  title: () => 'Live browser',
  render: ({ tile, first, close }) => (
    <TileFrame first={first} onClose={close} title="Live browser" chat={tile.session} popOut={popOutTile(tile)} className="items-center justify-center bg-background p-2">
      <Capability required={browserRequirements}>
        <BrowserScreen session={tile.session} fill />
      </Capability>
    </TileFrame>
  ),
}
