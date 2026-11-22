import { PROMPT_NOTE, PromptBody } from '@/components/chat/SessionMenu'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'

/** The session's system prompt beside its chat (`PromptSheet`'s body). One per chat. */
export const promptTile: TileKind = {
  title: () => 'System prompt',
  render: ({ tile, first, close }) => (
    <TileFrame title="System prompt" meta={PROMPT_NOTE} first={first} onClose={close}>
      <PromptBody session={tile.session} className="mt-4" />
    </TileFrame>
  ),
}
