import { PROMPT_NOTE, PromptBody } from '@/components/chat/SessionMenu'
import { popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'

/** The session's system prompt beside its chat (`PromptSheet`'s body). One per chat. */
export const promptTile: TileKind = {
  title: () => 'System prompt',
  render: ({ tile, first, close }) => (
    <TileFrame title="System prompt" chat={tile.session} meta={PROMPT_NOTE} first={first} onClose={close} popOut={popOutTile(tile)}>
      <PromptBody session={tile.session} className="mt-4" />
    </TileFrame>
  ),
}
