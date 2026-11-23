import { DRAFT_NOTE, DraftMemoryBody, MemoryBody } from '@/components/chat/MemoryPanel'
import { memoryRequirements } from '@/components/chat/requirements'
import { popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'
import { isDraft } from '@/lib/panes'
import { Capability } from '@/rpc/capabilities'

/** The Context panel beside its chat: `MemoryPanel`'s body, or `DraftMemory`'s choices beside a draft. One per chat. */
export const memoryTile: TileKind = {
  title: () => 'Context',
  render: ({ tile, first, close }) =>
    isDraft(tile.session) ? (
      <TileFrame title="Context" meta={DRAFT_NOTE} first={first} onClose={close} popOut={popOutTile(tile)}>
        <DraftMemoryBody className="pt-4" />
      </TileFrame>
    ) : (
      <TileFrame title="Context" first={first} onClose={close} popOut={popOutTile(tile)}>
        <Capability required={memoryRequirements}>
          <MemoryBody session={tile.session} />
        </Capability>
      </TileFrame>
    ),
}
